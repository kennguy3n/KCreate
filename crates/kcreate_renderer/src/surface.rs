//! Offscreen render target. Wraps a `wgpu::Texture` that we draw into and
//! later read back via [`crate::readback`].
//!
//! Format is `Rgba8Unorm` (linear). The CPU buffer we hand back to the
//! host is straight-alpha RGBA8, which the host writes to an
//! `ImageData` / `OffscreenCanvas`.

use crate::gpu::{bytes_per_row_aligned, nonzero_or_one};
use crate::{RendererError, Result};

#[derive(Debug)]
pub struct OffscreenSurface {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    width: u32,
    height: u32,
}

impl OffscreenSurface {
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> Result<Self> {
        Self::create(device, width, height)
    }

    fn create(device: &wgpu::Device, width: u32, height: u32) -> Result<Self> {
        if width == 0 || height == 0 {
            return Err(RendererError::InvalidDimensions { width, height });
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("kcreate-offscreen-target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Ok(Self {
            texture,
            view,
            width,
            height,
        })
    }

    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) -> Result<()> {
        if width == self.width && height == self.height {
            return Ok(());
        }
        *self = Self::create(device, width, height)?;
        Ok(())
    }

    pub const fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }

    pub const fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    pub const fn width(&self) -> u32 {
        self.width
    }

    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Upload straight-alpha RGBA8 pixels to the offscreen texture.
    /// `pixels.len()` must equal `width * height * 4`.
    pub fn write_pixels(
        &self,
        queue: &wgpu::Queue,
        pixels: &[u8],
        width: u32,
        height: u32,
    ) -> Result<()> {
        if width != self.width || height != self.height {
            return Err(RendererError::InvalidDimensions { width, height });
        }
        let expected = (width as usize) * (height as usize) * 4;
        if pixels.len() != expected {
            return Err(RendererError::Wgpu(format!(
                "write_pixels: buffer len {} != expected {expected} for {width}x{height}",
                pixels.len()
            )));
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        queue.submit(std::iter::empty());
        Ok(())
    }

    /// Read back the offscreen texture to a tightly-packed RGBA8 buffer
    /// (no row padding), straight alpha, length `width * height * 4`.
    pub fn read_pixels(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        out: &mut Vec<u8>,
    ) -> Result<()> {
        if width != self.width || height != self.height {
            return Err(RendererError::InvalidDimensions { width, height });
        }
        crate::readback::read_texture_to_vec(device, queue, &self.texture, width, height, out)
    }

    /// Total VRAM footprint in bytes (texture + readback buffer alignment).
    pub fn approx_vram_bytes(&self) -> u64 {
        let padded = u64::from(bytes_per_row_aligned(self.width));
        let _ = nonzero_or_one(self.height);
        padded * u64::from(self.height) * 2
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_device() -> Option<(wgpu::Device, wgpu::Queue)> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(async {
            instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::LowPower,
                    compatible_surface: None,
                    force_fallback_adapter: true,
                })
                .await
                .ok()
        })?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("test"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_defaults(),
            memory_hints: wgpu::MemoryHints::Performance,
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            trace: wgpu::Trace::Off,
        }))
        .ok()?;
        Some((device, queue))
    }

    #[test]
    fn create_succeeds_or_skips_when_no_adapter() {
        let Some((device, _q)) = test_device() else {
            // No adapter available in this env — skip.
            return;
        };
        let surface = OffscreenSurface::new(&device, 64, 32).expect("create surface");
        assert_eq!(surface.width(), 64);
        assert_eq!(surface.height(), 32);
        assert!(surface.approx_vram_bytes() > 0);
    }

    #[test]
    fn write_and_read_round_trip() {
        let Some((device, queue)) = test_device() else {
            return;
        };
        let surface = OffscreenSurface::new(&device, 8, 4).unwrap();
        let mut pixels = vec![0u8; 8 * 4 * 4];
        for (i, px) in pixels.chunks_exact_mut(4).enumerate() {
            px.copy_from_slice(&[(i as u8).wrapping_mul(7), 0, 0, 255]);
        }
        surface.write_pixels(&queue, &pixels, 8, 4).unwrap();
        let mut out = Vec::new();
        surface
            .read_pixels(&device, &queue, 8, 4, &mut out)
            .unwrap();
        assert_eq!(out.len(), 8 * 4 * 4);
        // Round-tripped data should match (allowing tiny differences in
        // odd cases on software adapters, but Rgba8Unorm is exact).
        assert_eq!(out, pixels);
    }
}
