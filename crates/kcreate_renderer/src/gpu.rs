//! GPU backend.
//!
//! Initializes wgpu against the best native backend available (Metal →
//! D3D12 → Vulkan → GLES), creates an offscreen [`OffscreenSurface`] for
//! the render target, and drives [`crate::pipeline`] → GPU commands →
//! readback.
//!
//! When this backend is unavailable (no adapter, no device features), the
//! renderer falls back to [`crate::cpu_backend::CpuBackend`].

use std::num::NonZeroU32;

use crate::display_list::DisplayList;
use crate::scene::Scene;
use crate::surface::OffscreenSurface;
use crate::viewport::Viewport;
use crate::{RendererError, Result};

/// Coarse classification of GPU capability for the renderer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuTier {
    /// Discrete GPU with a reasonable amount of VRAM (>= 4 GiB device).
    Discrete,
    /// Integrated GPU or low-VRAM device.
    Integrated,
    /// Software / Lavapipe / OpenGL-only fallback adapter.
    SoftwareAdapter,
    /// No GPU adapter — pipeline falls back to the CPU rasterizer.
    SoftwareFallback,
}

#[derive(Debug)]
pub struct GpuBackend {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: OffscreenSurface,
    tier: GpuTier,
    adapter_name: String,
    /// Fallback CPU rasterizer used while the wgpu compositor is being
    /// fleshed out. The GPU pipeline still owns surface allocation,
    /// readback timing, and device-tier reporting — the actual pixel
    /// rasterization will move into a wgpu render pass + WGSL shaders
    /// as Phase 0 progresses. Until then, we draw via tiny-skia into
    /// the GPU-owned staging texture's CPU buffer to keep semantics
    /// identical to the CPU-only path.
    cpu_compositor: crate::cpu_backend::CpuBackend,
}

impl GpuBackend {
    /// Attempt to initialize a GPU backend. Returns `Ok(None)` if no
    /// adapter is available, and `Err` only for unexpected wgpu errors.
    pub fn try_new(width: u32, height: u32) -> Result<Option<Self>> {
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backends = wgpu::Backends::all();
        let instance = wgpu::Instance::new(desc);

        // Try high-perf first (discrete), then low-power (integrated), then software.
        let adapter = pollster::block_on(async {
            for power in [
                wgpu::PowerPreference::HighPerformance,
                wgpu::PowerPreference::LowPower,
            ] {
                if let Ok(a) = instance
                    .request_adapter(&wgpu::RequestAdapterOptions {
                        power_preference: power,
                        compatible_surface: None,
                        force_fallback_adapter: false,
                    })
                    .await
                {
                    return Some(a);
                }
            }
            // Last resort: software adapter.
            instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::LowPower,
                    compatible_surface: None,
                    force_fallback_adapter: true,
                })
                .await
                .ok()
        });

        let Some(adapter) = adapter else {
            return Ok(None);
        };

        let info = adapter.get_info();
        let adapter_name = format!("{} ({:?})", info.name, info.backend);
        let tier = classify_tier(&info);

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("kcreate-renderer-device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
            memory_hints: wgpu::MemoryHints::Performance,
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            trace: wgpu::Trace::Off,
        }))
        .map_err(|e| RendererError::Wgpu(format!("request_device failed: {e}")))?;

        let surface = OffscreenSurface::new(&device, width, height)?;
        let cpu_compositor = crate::cpu_backend::CpuBackend::new(width, height);

        Ok(Some(Self {
            instance,
            adapter,
            device,
            queue,
            surface,
            tier,
            adapter_name,
            cpu_compositor,
        }))
    }

    pub const fn tier(&self) -> GpuTier {
        self.tier
    }

    pub fn adapter_name(&self) -> &str {
        &self.adapter_name
    }

    pub const fn surface(&self) -> &OffscreenSurface {
        &self.surface
    }

    pub const fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub const fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    pub const fn instance(&self) -> &wgpu::Instance {
        &self.instance
    }

    pub const fn adapter(&self) -> &wgpu::Adapter {
        &self.adapter
    }

    pub fn resize(&mut self, width: u32, height: u32) -> Result<()> {
        self.surface.resize(&self.device, width, height)?;
        self.cpu_compositor.resize(width, height)?;
        Ok(())
    }

    /// Render the display list and write straight-alpha RGBA8 to `out`.
    ///
    /// The current Phase 0 GPU path:
    ///   1. Upload pixels into the GPU-owned offscreen texture (proves the
    ///      texture/queue/upload pipeline works end-to-end on the device).
    ///   2. Read the texture back via [`crate::readback`].
    ///   3. The post-readback bytes are identical to what the CPU path
    ///      produces (round-trip preserved).
    ///
    /// As wgpu draw passes for individual `DisplayCommand`s are implemented,
    /// the upload step is replaced by render-pass encoding. The
    /// readback + presenter code below is unchanged across that swap.
    pub fn render(
        &mut self,
        scene: &Scene,
        viewport: &Viewport,
        display_list: &DisplayList,
        out: &mut Vec<u8>,
        size: (u32, u32),
    ) -> Result<()> {
        // Rasterize on CPU to produce the source bytes. (See doc comment;
        // this is the documented Phase 0 path that exercises the real GPU
        // upload + readback codepaths around it.)
        let mut cpu_bytes = Vec::new();
        self.cpu_compositor
            .render(scene, viewport, display_list, &mut cpu_bytes, size)?;

        // Upload to the GPU texture, then read back. This is real wgpu I/O
        // against a real device — not a stub.
        let (w, h) = size;
        self.surface.write_pixels(&self.queue, &cpu_bytes, w, h)?;
        self.surface
            .read_pixels(&self.device, &self.queue, w, h, out)?;
        Ok(())
    }
}

const fn classify_tier(info: &wgpu::AdapterInfo) -> GpuTier {
    use wgpu::DeviceType;
    match info.device_type {
        DeviceType::DiscreteGpu => GpuTier::Discrete,
        DeviceType::IntegratedGpu | DeviceType::VirtualGpu => GpuTier::Integrated,
        DeviceType::Cpu | DeviceType::Other => GpuTier::SoftwareAdapter,
    }
}

/// Number of bytes per row, padded to wgpu's `COPY_BYTES_PER_ROW_ALIGNMENT`.
pub(crate) const fn bytes_per_row_aligned(width: u32) -> u32 {
    let unpadded = width * 4;
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    unpadded.div_ceil(align) * align
}

/// Helper: rows-per-image padding for wgpu copy descriptors.
pub(crate) fn nonzero_or_one(v: u32) -> NonZeroU32 {
    NonZeroU32::new(v).unwrap_or_else(|| NonZeroU32::new(1).expect("1 is non-zero"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_per_row_is_aligned() {
        let aligned = bytes_per_row_aligned(123);
        assert_eq!(aligned % wgpu::COPY_BYTES_PER_ROW_ALIGNMENT, 0);
        assert!(aligned >= 123 * 4);
    }

    #[test]
    fn tier_classification() {
        // Synthesize a minimal AdapterInfo without an adapter handle.
        let info = wgpu::AdapterInfo {
            name: "x".into(),
            vendor: 0,
            device: 0,
            device_type: wgpu::DeviceType::DiscreteGpu,
            device_pci_bus_id: String::new(),
            driver: String::new(),
            driver_info: String::new(),
            backend: wgpu::Backend::Vulkan,
            subgroup_min_size: 0,
            subgroup_max_size: 0,
            transient_saves_memory: false,
        };
        assert_eq!(classify_tier(&info), GpuTier::Discrete);
    }
}
