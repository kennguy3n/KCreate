//! Native swapchain surface for Phase 1 direct presentation.
//!
//! The Phase 0 path renders into an offscreen [`crate::surface::OffscreenSurface`],
//! reads back to a CPU buffer, ships the bytes across the N-API
//! boundary, and the Electron renderer paints them to a 2D `<canvas>`
//! via `putImageData`. That round-trip costs us one full readback per
//! frame.
//!
//! Phase 1 collapses that round-trip: the renderer creates a
//! `wgpu::Surface` directly from the OS window handle (Metal layer on
//! macOS, HWND on Windows, Wayland / X11 surfaces on Linux) via
//! [`raw_window_handle`], renders into the swapchain's current
//! texture, and presents it. No readback, no IPC, no
//! `putImageData`.
//!
//! This module just builds the surface scaffolding. Wiring it through
//! Electron requires platform-specific child-window embedding (a
//! [`BrowserView`] or `WCO` overlay on each platform), which is the
//! _next_ task (Block I follow-up) — the goal here is to land the
//! renderer-side primitive so the Electron side has something stable
//! to wire to.

use std::sync::Arc;

use raw_window_handle::{HasDisplayHandle, HasWindowHandle};

use crate::{RendererError, Result};

/// Wraps a `wgpu::Surface` configured for direct presentation against
/// a native window. Owns the surface configuration so callers can
/// resize without rebuilding the entire pipeline.
///
/// The surface's lifetime is tied to the underlying window handle.
/// We use `Arc<Window>`-style ownership (a generic `H` that is
/// `'static`) so the surface can be moved into the renderer thread
/// without forcing a lifetime on every downstream type.
#[derive(Debug)]
pub struct NativeSurface {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
}

impl NativeSurface {
    /// Create a native surface from a window handle.
    ///
    /// The window handle must outlive the returned `NativeSurface`.
    /// The `'static` bound on `H` keeps that invariant honest: a
    /// `BrowserView` / `NSView` / `HWND` wrapper that owns its
    /// underlying handle will satisfy it; a borrowed handle pointing
    /// into a stack-allocated event-loop scratch buffer will not.
    ///
    /// `width` / `height` are the swapchain extent in physical
    /// pixels. The caller is responsible for tracking DPI scaling
    /// and re-configuring on `WM_DPICHANGED` / `NSWindowDidChangeBackingProperties`
    /// / Wayland fractional-scale events.
    pub fn from_window<H>(
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        handle: Arc<H>,
        width: u32,
        height: u32,
    ) -> Result<Self>
    where
        H: HasWindowHandle + HasDisplayHandle + Send + Sync + 'static,
    {
        if width == 0 || height == 0 {
            return Err(RendererError::InvalidDimensions { width, height });
        }
        let surface = instance
            .create_surface(handle)
            .map_err(|e| RendererError::Wgpu(format!("create_surface: {e}")))?;
        let caps = surface.get_capabilities(adapter);
        let format = pick_surface_format(&caps).ok_or_else(|| {
            RendererError::Wgpu(
                "no compatible surface format (need an sRGB-capable format)".to_string(),
            )
        })?;
        let alpha_mode = pick_alpha_mode(&caps);
        let present_mode = pick_present_mode(&caps);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width,
            height,
            present_mode,
            alpha_mode,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(device, &config);
        Ok(Self { surface, config })
    }

    /// Resize the swapchain. No-ops if the dimensions are unchanged
    /// so we don't trigger redundant reconfigures on every
    /// `WM_SIZE` flood.
    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) -> Result<()> {
        if width == 0 || height == 0 {
            return Err(RendererError::InvalidDimensions { width, height });
        }
        if width == self.config.width && height == self.config.height {
            return Ok(());
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(device, &self.config);
        Ok(())
    }

    /// Acquire the next swapchain texture for rendering. The caller
    /// must call [`Self::present`] on the returned texture once
    /// rendering is complete, otherwise the swapchain will stall.
    ///
    /// `Suboptimal` textures are returned successfully — the caller
    /// is expected to reconfigure on the next frame boundary. All
    /// other non-success states (`Timeout`, `Occluded`, `Outdated`,
    /// `Lost`, `Validation`) collapse to a `RendererError::Wgpu`
    /// with the descriptive variant name so callers can decide
    /// whether to skip the frame or rebuild the surface.
    pub fn get_current_texture(&self) -> Result<wgpu::SurfaceTexture> {
        match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => Ok(t),
            wgpu::CurrentSurfaceTexture::Timeout => {
                Err(RendererError::Wgpu("get_current_texture: timeout".into()))
            }
            wgpu::CurrentSurfaceTexture::Occluded => {
                Err(RendererError::Wgpu("get_current_texture: occluded".into()))
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                Err(RendererError::Wgpu("get_current_texture: outdated".into()))
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                Err(RendererError::Wgpu("get_current_texture: lost".into()))
            }
            wgpu::CurrentSurfaceTexture::Validation => Err(RendererError::Wgpu(
                "get_current_texture: validation error".into(),
            )),
        }
    }

    /// Present a previously-acquired swapchain texture. Equivalent
    /// to `SurfaceTexture::present()` but lives on `NativeSurface`
    /// so callers don't have to remember the wgpu API directly.
    pub fn present(texture: wgpu::SurfaceTexture) {
        texture.present();
    }

    /// Upload a CPU-rasterized RGBA8 buffer to the next swapchain
    /// texture and present it. This is the Phase 1 fast path: the
    /// renderer's CPU compositor produces RGBA8 bytes (same as the
    /// existing offscreen path), we hand them to the queue's
    /// `write_texture`, and present — no CPU-side readback / IPC
    /// `putImageData` round trip.
    ///
    /// `bytes` must be exactly `width * height * 4` long and match
    /// the surface's configured extent. Returns
    /// `RendererError::InvalidDimensions` otherwise. This path uses
    /// the *configured* surface dimensions, not arbitrary
    /// dimensions, because the swapchain texture extent is fixed by
    /// surface config.
    pub fn present_cpu_frame(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bytes: &[u8],
    ) -> Result<()> {
        let expected_len = (self.config.width as usize)
            .checked_mul(self.config.height as usize)
            .and_then(|n| n.checked_mul(4))
            .ok_or(RendererError::InvalidDimensions {
                width: self.config.width,
                height: self.config.height,
            })?;
        if bytes.len() != expected_len {
            return Err(RendererError::Wgpu(format!(
                "present_cpu_frame: byte buffer len {} != expected {expected_len} for {}x{}",
                bytes.len(),
                self.config.width,
                self.config.height,
            )));
        }
        let surface_tex = self.get_current_texture()?;
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &surface_tex.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(self.config.width * 4),
                rows_per_image: Some(self.config.height),
            },
            wgpu::Extent3d {
                width: self.config.width,
                height: self.config.height,
                depth_or_array_layers: 1,
            },
        );
        queue.submit(std::iter::empty());
        let _ = device; // device is required by `write_texture`'s lifetime hygiene; kept in the signature for symmetry with `OffscreenSurface::write_pixels`.
        surface_tex.present();
        Ok(())
    }

    pub const fn width(&self) -> u32 {
        self.config.width
    }

    pub const fn height(&self) -> u32 {
        self.config.height
    }

    pub const fn format(&self) -> wgpu::TextureFormat {
        self.config.format
    }

    pub const fn config(&self) -> &wgpu::SurfaceConfiguration {
        &self.config
    }
}

/// Prefer an sRGB-capable format so colors come out right without an
/// extra gamma pass in the shader. Falls back to the first reported
/// format if no sRGB option is advertised (e.g. some headless
/// adapters).
fn pick_surface_format(caps: &wgpu::SurfaceCapabilities) -> Option<wgpu::TextureFormat> {
    caps.formats
        .iter()
        .copied()
        .find(wgpu::TextureFormat::is_srgb)
        .or_else(|| caps.formats.first().copied())
}

/// Prefer `Opaque` (Metal / D3D12 / Wayland default) — the renderer
/// already clears to the document background, so we never need the
/// compositor to do alpha blending under us. Fall back to whatever
/// the surface supports if `Opaque` isn't on the list.
fn pick_alpha_mode(caps: &wgpu::SurfaceCapabilities) -> wgpu::CompositeAlphaMode {
    if caps.alpha_modes.contains(&wgpu::CompositeAlphaMode::Opaque) {
        wgpu::CompositeAlphaMode::Opaque
    } else {
        caps.alpha_modes
            .first()
            .copied()
            .unwrap_or(wgpu::CompositeAlphaMode::Auto)
    }
}

/// Phase 1 pick: prefer `Mailbox` for tear-free low-latency
/// presentation when supported, else `Fifo` (the only guaranteed
/// mode). We never pick `Immediate` because it can tear.
fn pick_present_mode(caps: &wgpu::SurfaceCapabilities) -> wgpu::PresentMode {
    if caps.present_modes.contains(&wgpu::PresentMode::Mailbox) {
        wgpu::PresentMode::Mailbox
    } else {
        wgpu::PresentMode::Fifo
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tests that the format picker prefers sRGB even when other
    /// formats are advertised first. Uses a `wgpu::SurfaceCapabilities`-
    /// shaped fixture so we don't need a live adapter.
    #[test]
    fn picks_srgb_format_over_linear() {
        let caps = wgpu::SurfaceCapabilities {
            formats: vec![
                wgpu::TextureFormat::Bgra8Unorm,
                wgpu::TextureFormat::Bgra8UnormSrgb,
            ],
            present_modes: vec![wgpu::PresentMode::Fifo],
            alpha_modes: vec![wgpu::CompositeAlphaMode::Opaque],
            usages: wgpu::TextureUsages::RENDER_ATTACHMENT,
        };
        assert_eq!(
            pick_surface_format(&caps),
            Some(wgpu::TextureFormat::Bgra8UnormSrgb)
        );
    }

    #[test]
    fn falls_back_to_first_format_when_no_srgb() {
        let caps = wgpu::SurfaceCapabilities {
            formats: vec![wgpu::TextureFormat::Rgba8Unorm],
            present_modes: vec![wgpu::PresentMode::Fifo],
            alpha_modes: vec![wgpu::CompositeAlphaMode::Opaque],
            usages: wgpu::TextureUsages::RENDER_ATTACHMENT,
        };
        assert_eq!(
            pick_surface_format(&caps),
            Some(wgpu::TextureFormat::Rgba8Unorm)
        );
    }

    #[test]
    fn prefers_opaque_alpha_mode() {
        let caps = wgpu::SurfaceCapabilities {
            formats: vec![wgpu::TextureFormat::Bgra8UnormSrgb],
            present_modes: vec![wgpu::PresentMode::Fifo],
            alpha_modes: vec![
                wgpu::CompositeAlphaMode::PreMultiplied,
                wgpu::CompositeAlphaMode::Opaque,
            ],
            usages: wgpu::TextureUsages::RENDER_ATTACHMENT,
        };
        assert_eq!(pick_alpha_mode(&caps), wgpu::CompositeAlphaMode::Opaque);
    }

    #[test]
    fn falls_back_to_first_alpha_mode_when_no_opaque() {
        let caps = wgpu::SurfaceCapabilities {
            formats: vec![wgpu::TextureFormat::Bgra8UnormSrgb],
            present_modes: vec![wgpu::PresentMode::Fifo],
            alpha_modes: vec![wgpu::CompositeAlphaMode::PreMultiplied],
            usages: wgpu::TextureUsages::RENDER_ATTACHMENT,
        };
        assert_eq!(
            pick_alpha_mode(&caps),
            wgpu::CompositeAlphaMode::PreMultiplied
        );
    }

    #[test]
    fn prefers_mailbox_present_mode() {
        let caps = wgpu::SurfaceCapabilities {
            formats: vec![wgpu::TextureFormat::Bgra8UnormSrgb],
            present_modes: vec![wgpu::PresentMode::Fifo, wgpu::PresentMode::Mailbox],
            alpha_modes: vec![wgpu::CompositeAlphaMode::Opaque],
            usages: wgpu::TextureUsages::RENDER_ATTACHMENT,
        };
        assert_eq!(pick_present_mode(&caps), wgpu::PresentMode::Mailbox);
    }

    #[test]
    fn falls_back_to_fifo_present_mode() {
        let caps = wgpu::SurfaceCapabilities {
            formats: vec![wgpu::TextureFormat::Bgra8UnormSrgb],
            present_modes: vec![wgpu::PresentMode::Fifo],
            alpha_modes: vec![wgpu::CompositeAlphaMode::Opaque],
            usages: wgpu::TextureUsages::RENDER_ATTACHMENT,
        };
        assert_eq!(pick_present_mode(&caps), wgpu::PresentMode::Fifo);
    }
}
