//! GPU compute filters (Phase 11 Block B Tasks 9-11).
//!
//! Hosts the WGSL compute pipelines for the three filters that
//! dominate the editor's CPU pixel-budget:
//!
//! - Gaussian blur (two-pass separable)
//! - Levels / Curves (per-pixel 256-entry LUT lookup)
//! - Unsharp mask (compose original + amount * (original - blurred))
//!
//! The pipelines are designed against the downlevel-default wgpu
//! limits: no storage textures, only storage buffers. Pixels move
//! across the host/device boundary as packed `u32` RGBA in the
//! same layout the renderer already uses for offscreen readback
//! (`R << 24 | G << 16 | B << 8 | A`). This costs one `u32 →
//! [u8; 4]` byteorder swap on the host either side of a dispatch
//! and keeps the pipeline portable on Intel UHD, M-series
//! integrated GPUs, and the wgpu software adapter.
//!
//! The module exposes:
//!
//! - [`GpuComputeContext::try_new`] — initialise a wgpu device
//!   and the three compute pipelines. Returns `Ok(None)` when no
//!   adapter is available (CI runner without GPU, headless test).
//! - [`GpuComputeContext::gaussian_blur`] — two-pass blur of an
//!   RGBA byte buffer.
//! - [`GpuComputeContext::levels_curves`] — apply a 256-entry LUT.
//! - [`GpuComputeContext::unsharp_mask`] — unsharp-mask compose
//!   pass (re-uses [`Self::gaussian_blur`] for the blurred input).
//!
//! Each entry point falls back to the CPU implementation in
//! `kcreate_raster::filters` when the context is `None`, so the
//! bridge can call them unconditionally without branching on GPU
//! availability at every call site.

use std::borrow::Cow;
use std::num::NonZeroU64;

use wgpu::util::DeviceExt;

const GAUSSIAN_WGSL: &str = include_str!("gaussian_blur.wgsl");
const LEVELS_CURVES_WGSL: &str = include_str!("levels_curves.wgsl");
const UNSHARP_WGSL: &str = include_str!("unsharp_mask.wgsl");

const WORKGROUP_DIM: u32 = 8;

/// Maximum blur sigma accepted by the GPU pipeline. The compute
/// shader's per-pixel inner loop runs `2 * ceil(3 * sigma) + 1`
/// iterations, so we cap sigma to keep that bounded on integrated
/// GPUs. The CPU reference path (`kcreate_raster::filters::gaussian_blur`)
/// happily handles arbitrary sigmas; callers that ask for more
/// than `MAX_BLUR_SIGMA` fall back to CPU.
///
/// Photoshop caps Gaussian-blur radius at 1000 px; 64 is the
/// design ceiling for *interactive* preview, which matches the
/// envelope the editor exposes through the bridge.
pub const MAX_BLUR_SIGMA: f32 = 64.0;

/// Backwards-compatible alias for `MAX_BLUR_SIGMA`. The bridge
/// historically refers to this constant as a u32 cap.
pub const MAX_BLUR_RADIUS: u32 = MAX_BLUR_SIGMA as u32;

/// LUT size for Levels / Curves. The shader indexes the LUT by
/// the input channel's quantised byte value, so the natural size
/// is 256.
pub const LUT_SIZE: usize = 256;

/// Re-export of [`wgpu::Backend`] so downstream crates (tests, the
/// bridge) can match on it without needing to add `wgpu` as a
/// direct dependency.
pub use wgpu::Backend as WgpuBackend;

/// Re-export of [`wgpu::DeviceType`] so downstream crates can
/// distinguish discrete / integrated / software adapters without
/// linking `wgpu` directly.
pub use wgpu::DeviceType as WgpuDeviceType;

/// Host-side GPU compute context. Holds the device, queue, and
/// cached pipelines so repeat dispatches don't re-compile WGSL.
#[allow(missing_debug_implementations)]
pub struct GpuComputeContext {
    device: wgpu::Device,
    queue: wgpu::Queue,
    backend: wgpu::Backend,
    device_type: wgpu::DeviceType,
    blur_pipeline: wgpu::ComputePipeline,
    blur_bind_layout: wgpu::BindGroupLayout,
    lut_pipeline: wgpu::ComputePipeline,
    lut_bind_layout: wgpu::BindGroupLayout,
    unsharp_pipeline: wgpu::ComputePipeline,
    unsharp_bind_layout: wgpu::BindGroupLayout,
}

#[derive(Debug, thiserror::Error)]
pub enum ComputeError {
    #[error("input pixel buffer length {got} does not match width*height*4 = {expected}")]
    BadInputSize { got: usize, expected: usize },
    #[error("blur radius {got} exceeds the max of {max}")]
    RadiusTooLarge { got: u32, max: u32 },
    #[error("LUT must contain exactly {expected} entries; got {got}")]
    BadLutSize { got: usize, expected: usize },
    #[error("gpu readback failed: {0}")]
    Readback(String),
    #[error("wgpu device error: {0}")]
    Device(String),
}

impl GpuComputeContext {
    /// Try to build a GPU compute context from scratch (independent
    /// of the renderer's [`crate::gpu::GpuBackend`]). Returns
    /// `Ok(None)` when no adapter is available — the bridge should
    /// fall back to CPU filters in that case.
    pub fn try_new() -> Result<Option<Self>, ComputeError> {
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backends = wgpu::Backends::all();
        let instance = wgpu::Instance::new(desc);
        let adapter_opt = pollster::block_on(async {
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
            instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::LowPower,
                    compatible_surface: None,
                    force_fallback_adapter: true,
                })
                .await
                .ok()
        });
        let Some(adapter) = adapter_opt else {
            return Ok(None);
        };
        let info = adapter.get_info();
        let backend = info.backend;
        let device_type = info.device_type;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("kcreate-compute-device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
            memory_hints: wgpu::MemoryHints::Performance,
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            trace: wgpu::Trace::Off,
        }))
        .map_err(|e| ComputeError::Device(e.to_string()))?;
        Ok(Some(Self::from_device(device, queue, backend, device_type)))
    }

    /// Build a compute context that re-uses an existing device +
    /// queue (e.g. the one already owned by the renderer's
    /// `GpuBackend`). The renderer prefers this path so it doesn't
    /// open a second wgpu adapter for the compute work.
    pub fn from_existing(
        device: wgpu::Device,
        queue: wgpu::Queue,
        backend: wgpu::Backend,
        device_type: wgpu::DeviceType,
    ) -> Self {
        Self::from_device(device, queue, backend, device_type)
    }

    /// Backend the compute context was opened against. Tests can
    /// use this to skip GPU-perf assertions on the software
    /// adapter (`wgpu::Backend::Noop`).
    pub const fn backend(&self) -> wgpu::Backend {
        self.backend
    }

    /// Adapter device type — callers can detect `Cpu` to skip
    /// hardware-specific perf assertions on llvmpipe / Lavapipe /
    /// Microsoft Basic Render Driver.
    pub const fn device_type(&self) -> wgpu::DeviceType {
        self.device_type
    }

    /// True when the chosen adapter is a software / non-functional
    /// rasterizer (Lavapipe, llvmpipe, the `Noop` test backend, or
    /// any adapter reporting `DeviceType::Cpu`). Tests that depend
    /// on hardware acceleration should skip when this returns true.
    pub fn is_software_adapter(&self) -> bool {
        matches!(self.backend, wgpu::Backend::Noop)
            || matches!(self.device_type, wgpu::DeviceType::Cpu)
    }

    fn from_device(
        device: wgpu::Device,
        queue: wgpu::Queue,
        backend: wgpu::Backend,
        device_type: wgpu::DeviceType,
    ) -> Self {
        let (blur_pipeline, blur_bind_layout) =
            build_pipeline(&device, "blur", GAUSSIAN_WGSL, BindKind::FourBuffer);
        let (lut_pipeline, lut_bind_layout) =
            build_pipeline(&device, "levels-curves", LEVELS_CURVES_WGSL, BindKind::FourBuffer);
        let (unsharp_pipeline, unsharp_bind_layout) = build_pipeline(
            &device,
            "unsharp-mask",
            UNSHARP_WGSL,
            BindKind::FourBufferUnsharp,
        );
        Self {
            device,
            queue,
            backend,
            device_type,
            blur_pipeline,
            blur_bind_layout,
            lut_pipeline,
            lut_bind_layout,
            unsharp_pipeline,
            unsharp_bind_layout,
        }
    }

    /// Apply a two-pass separable Gaussian blur. `rgba` is row-
    /// major RGBA8 (length must equal `width * height * 4`).
    /// `sigma` matches the convention used by
    /// [`kcreate_raster::filters::gaussian_blur`] — it is the
    /// Gaussian standard deviation in pixels, and the kernel
    /// half-width is `ceil(3 * sigma)` on each side.
    pub fn gaussian_blur(
        &self,
        rgba: &[u8],
        width: u32,
        height: u32,
        sigma: f32,
    ) -> Result<Vec<u8>, ComputeError> {
        validate_pixels(rgba, width, height)?;
        if !sigma.is_finite() || sigma < 0.0 {
            return Err(ComputeError::RadiusTooLarge {
                got: u32::MAX,
                max: MAX_BLUR_RADIUS,
            });
        }
        if sigma > MAX_BLUR_SIGMA {
            return Err(ComputeError::RadiusTooLarge {
                got: sigma.round() as u32,
                max: MAX_BLUR_RADIUS,
            });
        }
        if sigma == 0.0 {
            return Ok(rgba.to_vec());
        }
        let half_width = (sigma * 3.0).ceil().max(1.0) as u32;

        let pixel_count = (width as usize) * (height as usize);
        let buf_size = (pixel_count * std::mem::size_of::<u32>()) as u64;
        let input_packed = pack_rgba_to_u32(rgba);

        let weights = build_gaussian_kernel(sigma);
        let weights_bytes = bytemuck_f32_slice(&weights);

        let input_buf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("compute-blur-input"),
            contents: bytemuck_u32_slice(&input_packed),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });
        let intermediate_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("compute-blur-intermediate"),
            size: buf_size,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let output_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("compute-blur-output"),
            size: buf_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let weights_buf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("compute-blur-weights"),
            contents: weights_bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        // Pass 1: horizontal blur, input -> intermediate.
        self.encode_blur_pass(
            &input_buf,
            &intermediate_buf,
            &weights_buf,
            width,
            height,
            half_width,
            0,
        );
        // Pass 2: vertical blur, intermediate -> output.
        self.encode_blur_pass(
            &intermediate_buf,
            &output_buf,
            &weights_buf,
            width,
            height,
            half_width,
            1,
        );

        let bytes = self.read_buffer_to_bytes(&output_buf, pixel_count)?;
        Ok(unpack_u32_to_rgba(&bytes))
    }

    #[allow(clippy::too_many_arguments)]
    fn encode_blur_pass(
        &self,
        input: &wgpu::Buffer,
        output: &wgpu::Buffer,
        weights: &wgpu::Buffer,
        width: u32,
        height: u32,
        half_width: u32,
        axis: u32,
    ) {
        let params = BlurParams {
            radius: half_width,
            axis,
            width,
            height,
        };
        let params_buf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("compute-blur-params"),
            contents: bytemuck_one(&params),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("compute-blur-bind"),
            layout: &self.blur_bind_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: weights.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: input.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: output.as_entire_binding(),
                },
            ],
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("compute-blur-encoder"),
            });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("compute-blur-pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.blur_pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            let groups_x = width.div_ceil(WORKGROUP_DIM);
            let groups_y = height.div_ceil(WORKGROUP_DIM);
            pass.dispatch_workgroups(groups_x, groups_y, 1);
        }
        self.queue.submit(Some(encoder.finish()));
    }

    /// Apply a 256-entry brightness LUT (per-channel) to an RGBA
    /// byte buffer.
    pub fn levels_curves(
        &self,
        rgba: &[u8],
        width: u32,
        height: u32,
        lut: &[f32],
        apply_alpha: bool,
    ) -> Result<Vec<u8>, ComputeError> {
        validate_pixels(rgba, width, height)?;
        if lut.len() != LUT_SIZE {
            return Err(ComputeError::BadLutSize {
                got: lut.len(),
                expected: LUT_SIZE,
            });
        }
        let pixel_count = (width as usize) * (height as usize);
        let buf_size = (pixel_count * std::mem::size_of::<u32>()) as u64;
        let input_packed = pack_rgba_to_u32(rgba);

        let input_buf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("compute-lut-input"),
            contents: bytemuck_u32_slice(&input_packed),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let lut_buf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("compute-lut-table"),
            contents: bytemuck_f32_slice(lut),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let output_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("compute-lut-output"),
            size: buf_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let params = AdjustParams {
            width,
            height,
            apply_alpha: u32::from(apply_alpha),
            _pad: 0,
        };
        let params_buf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("compute-lut-params"),
            contents: bytemuck_one(&params),
            usage: wgpu::BufferUsages::UNIFORM,
        });

        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("compute-lut-bind"),
            layout: &self.lut_bind_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: lut_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: input_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: output_buf.as_entire_binding(),
                },
            ],
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("compute-lut-encoder"),
            });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("compute-lut-pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.lut_pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(width.div_ceil(WORKGROUP_DIM), height.div_ceil(WORKGROUP_DIM), 1);
        }
        self.queue.submit(Some(encoder.finish()));

        let bytes = self.read_buffer_to_bytes(&output_buf, pixel_count)?;
        Ok(unpack_u32_to_rgba(&bytes))
    }

    /// Unsharp-mask compose pass. Internally runs Gaussian blur on
    /// `rgba` first (re-using [`Self::gaussian_blur`]) and then
    /// dispatches the unsharp compose shader. `sigma` matches the
    /// convention of `gaussian_blur`.
    pub fn unsharp_mask(
        &self,
        rgba: &[u8],
        width: u32,
        height: u32,
        sigma: f32,
        amount: f32,
        threshold: u8,
    ) -> Result<Vec<u8>, ComputeError> {
        validate_pixels(rgba, width, height)?;
        let blurred = self.gaussian_blur(rgba, width, height, sigma)?;

        let pixel_count = (width as usize) * (height as usize);
        let buf_size = (pixel_count * std::mem::size_of::<u32>()) as u64;
        let original_packed = pack_rgba_to_u32(rgba);
        let blurred_packed = pack_rgba_to_u32(&blurred);

        let original_buf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("compute-unsharp-original"),
            contents: bytemuck_u32_slice(&original_packed),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let blurred_buf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("compute-unsharp-blurred"),
            contents: bytemuck_u32_slice(&blurred_packed),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let output_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("compute-unsharp-output"),
            size: buf_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let params = UnsharpParams {
            width,
            height,
            amount,
            threshold: f32::from(threshold) / 255.0,
        };
        let params_buf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("compute-unsharp-params"),
            contents: bytemuck_one(&params),
            usage: wgpu::BufferUsages::UNIFORM,
        });

        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("compute-unsharp-bind"),
            layout: &self.unsharp_bind_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: original_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: blurred_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: output_buf.as_entire_binding(),
                },
            ],
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("compute-unsharp-encoder"),
            });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("compute-unsharp-pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.unsharp_pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(width.div_ceil(WORKGROUP_DIM), height.div_ceil(WORKGROUP_DIM), 1);
        }
        self.queue.submit(Some(encoder.finish()));

        let bytes = self.read_buffer_to_bytes(&output_buf, pixel_count)?;
        Ok(unpack_u32_to_rgba(&bytes))
    }

    fn read_buffer_to_bytes(
        &self,
        src: &wgpu::Buffer,
        pixel_count: usize,
    ) -> Result<Vec<u8>, ComputeError> {
        let size = (pixel_count * std::mem::size_of::<u32>()) as u64;
        let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("compute-readback"),
            size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("compute-readback-encoder"),
            });
        encoder.copy_buffer_to_buffer(src, 0, &staging, 0, size);
        self.queue.submit(Some(encoder.finish()));

        let slice = staging.slice(..);
        let (sender, receiver) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |res| {
            // Send result; ignore mailbox-full errors (cannot happen
            // here because the receiver is single-shot).
            let _ = sender.send(res);
        });
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| ComputeError::Readback(format!("device poll failed: {e}")))?;
        let map_result = receiver
            .recv()
            .map_err(|e| ComputeError::Readback(format!("readback channel closed: {e}")))?;
        map_result.map_err(|e| ComputeError::Readback(format!("buffer map failed: {e}")))?;
        let data = slice.get_mapped_range();
        let bytes = data.to_vec();
        drop(data);
        staging.unmap();
        Ok(bytes)
    }
}

#[derive(Copy, Clone)]
enum BindKind {
    /// Bind layout used by blur + LUT: uniform params + storage
    /// weights/LUT + storage input + storage read_write output.
    FourBuffer,
    /// Bind layout used by unsharp-mask: uniform params + storage
    /// original + storage blurred + storage read_write output.
    /// Structurally identical to `FourBuffer` but kept separate so
    /// the field names in the diagnostic labels reflect the role.
    FourBufferUnsharp,
}

fn build_pipeline(
    device: &wgpu::Device,
    label: &str,
    wgsl: &str,
    _kind: BindKind,
) -> (wgpu::ComputePipeline, wgpu::BindGroupLayout) {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(wgsl)),
    });
    let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &[Some(&bind_layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(label),
        layout: Some(&pipeline_layout),
        module: &module,
        entry_point: Some("main"),
        compilation_options: wgpu::PipelineCompilationOptions::default(),
        cache: None,
    });
    (pipeline, bind_layout)
}

fn validate_pixels(rgba: &[u8], width: u32, height: u32) -> Result<(), ComputeError> {
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4))
        .unwrap_or(usize::MAX);
    if rgba.len() != expected {
        return Err(ComputeError::BadInputSize {
            got: rgba.len(),
            expected,
        });
    }
    Ok(())
}

/// Build a 1-D Gaussian kernel matching the CPU reference in
/// `kcreate_raster::filters::gaussian_kernel_1d`. `sigma` is the
/// Gaussian standard deviation in pixels; the kernel covers
/// `±ceil(3 * sigma)` taps. Weights are normalised so they sum to
/// 1.0; the GPU shader also re-normalises (`acc / weight_sum`),
/// so an unnormalised buffer would still produce correct output
/// but at higher round-off cost.
pub fn build_gaussian_kernel(sigma: f32) -> Vec<f32> {
    if sigma <= 0.0 {
        return vec![1.0];
    }
    let r = (sigma * 3.0).ceil() as i32;
    let two_sigma_sq = 2.0 * sigma * sigma;
    let mut weights: Vec<f32> = (-r..=r)
        .map(|i| (-(i as f32 * i as f32) / two_sigma_sq).exp())
        .collect();
    let sum: f32 = weights.iter().sum();
    if sum > 0.0 {
        for w in &mut weights {
            *w /= sum;
        }
    }
    weights
}

/// Compile a Levels (black/white/gamma) operator to the 256-entry
/// LUT the GPU shader expects.
pub fn build_levels_lut(black_point: f32, white_point: f32, gamma: f32) -> Vec<f32> {
    let safe_black = black_point.clamp(0.0, 1.0);
    let safe_white = white_point.clamp(0.0, 1.0).max(safe_black + 1e-6);
    let span = safe_white - safe_black;
    let safe_gamma = if gamma.abs() < 1e-6 { 1.0 } else { gamma };
    let inv_gamma = 1.0 / safe_gamma;
    (0..LUT_SIZE)
        .map(|i| {
            let v = i as f32 / 255.0;
            let stretched = ((v - safe_black) / span).clamp(0.0, 1.0);
            stretched.powf(inv_gamma)
        })
        .collect()
}

/// Compile a sequence of `(input, output)` curve points (sorted by
/// `input`) into the 256-entry LUT the GPU shader expects.
/// Linear interpolation between control points; the endpoints are
/// implicitly anchored to `(0,0)` and `(1,1)` if not provided.
pub fn build_curves_lut(points: &[(f32, f32)]) -> Vec<f32> {
    let mut anchors: Vec<(f32, f32)> = points.to_vec();
    anchors.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    if anchors.first().is_none_or(|(x, _)| *x > 0.0) {
        anchors.insert(0, (0.0, 0.0));
    }
    if anchors.last().is_none_or(|(x, _)| *x < 1.0) {
        anchors.push((1.0, 1.0));
    }
    (0..LUT_SIZE)
        .map(|i| {
            let t = i as f32 / 255.0;
            // Binary search the segment containing `t`.
            let (left, right) = match anchors.binary_search_by(|p| {
                p.0.partial_cmp(&t).unwrap_or(std::cmp::Ordering::Equal)
            }) {
                Ok(idx) => return anchors[idx].1.clamp(0.0, 1.0),
                Err(idx) => {
                    let l = idx.saturating_sub(1).min(anchors.len() - 1);
                    let r = idx.min(anchors.len() - 1);
                    (l, r)
                }
            };
            let (x0, y0) = anchors[left];
            let (x1, y1) = anchors[right];
            if (x1 - x0).abs() < 1e-6 {
                return y0.clamp(0.0, 1.0);
            }
            let alpha = (t - x0) / (x1 - x0);
            (y0 + alpha * (y1 - y0)).clamp(0.0, 1.0)
        })
        .collect()
}

fn pack_rgba_to_u32(rgba: &[u8]) -> Vec<u32> {
    rgba.chunks_exact(4)
        .map(|c| {
            (u32::from(c[0]) << 24)
                | (u32::from(c[1]) << 16)
                | (u32::from(c[2]) << 8)
                | u32::from(c[3])
        })
        .collect()
}

fn unpack_u32_to_rgba(bytes: &[u8]) -> Vec<u8> {
    let len = bytes.len() / 4;
    let mut out = Vec::with_capacity(len * 4);
    for chunk in bytes.chunks_exact(4) {
        // The buffer holds packed-u32 in little-endian native order
        // (storage buffers are uploaded as raw bytes). Each u32 is
        // `R << 24 | G << 16 | B << 8 | A` in the WGSL packing, so
        // we have to unswizzle accordingly.
        let p = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        out.push(((p >> 24) & 0xFF) as u8);
        out.push(((p >> 16) & 0xFF) as u8);
        out.push(((p >> 8) & 0xFF) as u8);
        out.push((p & 0xFF) as u8);
    }
    out
}

#[repr(C)]
#[derive(Copy, Clone)]
struct BlurParams {
    radius: u32,
    axis: u32,
    width: u32,
    height: u32,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct AdjustParams {
    width: u32,
    height: u32,
    apply_alpha: u32,
    _pad: u32,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct UnsharpParams {
    width: u32,
    height: u32,
    amount: f32,
    threshold: f32,
}

// Local `bytemuck`-free helpers — avoid pulling in a new dep just
// for the `Pod` trait. All three params structs are `#[repr(C)]`
// with `Copy` fields whose memory layout is well-defined, and
// `u32`/`f32` slices are equivalent to byte slices on every
// platform wgpu supports.
fn bytemuck_one<T: Copy>(value: &T) -> &[u8] {
    // SAFETY: `value` is a properly aligned `T` with a known
    // `#[repr(C)]` layout (all callers are the local Params
    // structs). The resulting `&[u8]` covers exactly the bytes of
    // the `T` and has the same lifetime, so no aliasing or
    // lifetime-extension is possible.
    unsafe {
        std::slice::from_raw_parts(std::ptr::from_ref(value).cast::<u8>(), std::mem::size_of::<T>())
    }
}

fn bytemuck_u32_slice(values: &[u32]) -> &[u8] {
    // SAFETY: `u32` and `u8` are both POD with well-defined
    // little-endian layout on every wgpu-supported platform. The
    // resulting slice has the same lifetime as the input and a
    // length that exactly matches the byte view.
    unsafe {
        std::slice::from_raw_parts(values.as_ptr().cast::<u8>(), std::mem::size_of_val(values))
    }
}

fn bytemuck_f32_slice(values: &[f32]) -> &[u8] {
    // SAFETY: same justification as `bytemuck_u32_slice`. `f32`
    // is POD and we only ever read the bytes back as `f32` on the
    // GPU.
    unsafe {
        std::slice::from_raw_parts(values.as_ptr().cast::<u8>(), std::mem::size_of_val(values))
    }
}

/// Minimum size hints used by the bind-layout builder helpers
/// above. We don't enforce them at the binding-layout level (we
/// use `min_binding_size: None`) because the params struct width
/// is the same on every backend, but we expose them for tests so
/// regressions in the WGSL layout become obvious.
pub const BLUR_PARAMS_SIZE: u64 = std::mem::size_of::<BlurParams>() as u64;
pub const ADJUST_PARAMS_SIZE: u64 = std::mem::size_of::<AdjustParams>() as u64;
pub const UNSHARP_PARAMS_SIZE: u64 = std::mem::size_of::<UnsharpParams>() as u64;

/// Sanity check used by tests: the storage buffer entry must have
/// at least `min_binding_size` bytes. Forces a const-eval failure
/// at compile-time if any of the params structs accidentally
/// shrinks below the WGSL struct size.
const _: () = {
    assert!(BLUR_PARAMS_SIZE >= 16);
    assert!(ADJUST_PARAMS_SIZE >= 16);
    assert!(UNSHARP_PARAMS_SIZE >= 16);
};

#[allow(dead_code)]
fn _ensure_nonzero_binding_compiles() {
    // `NonZeroU64::new(BLUR_PARAMS_SIZE)` would be a runtime call;
    // we only need to keep the type imported in case we tighten
    // `min_binding_size` in a follow-up.
    let _ = NonZeroU64::new(BLUR_PARAMS_SIZE);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gaussian_kernel_sums_to_one() {
        for sigma in [1.0, 2.0, 4.0, 8.0] {
            let k = build_gaussian_kernel(sigma);
            // CPU/GPU convention: half-width = ceil(3 * sigma).
            let expected = ((sigma * 3.0).ceil() as usize) * 2 + 1;
            assert_eq!(k.len(), expected, "sigma {sigma}");
            let sum: f32 = k.iter().sum();
            assert!((sum - 1.0).abs() < 1e-5, "sigma {sigma} sum {sum}");
        }
    }

    #[test]
    fn levels_identity_lut_is_linear() {
        let lut = build_levels_lut(0.0, 1.0, 1.0);
        assert_eq!(lut.len(), LUT_SIZE);
        for (i, v) in lut.iter().enumerate() {
            let expected = i as f32 / 255.0;
            assert!(
                (*v - expected).abs() < 1e-5,
                "idx {i}: got {v}, want {expected}"
            );
        }
    }

    #[test]
    fn curves_identity_passthrough() {
        // Empty control points + implicit endpoints (0,0),(1,1) =
        // identity ramp.
        let lut = build_curves_lut(&[]);
        for (i, v) in lut.iter().enumerate() {
            let expected = i as f32 / 255.0;
            assert!(
                (*v - expected).abs() < 1e-3,
                "idx {i}: got {v}, want {expected}"
            );
        }
    }

    #[test]
    fn pack_unpack_roundtrip() {
        let src: Vec<u8> = (0..32u8).collect();
        let packed = pack_rgba_to_u32(&src);
        // unpack_u32_to_rgba expects the byte view of the storage
        // buffer (little-endian native), so convert the packed
        // values back to bytes first.
        let mut bytes = Vec::with_capacity(packed.len() * 4);
        for v in packed {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        let round_tripped = unpack_u32_to_rgba(&bytes);
        assert_eq!(round_tripped, src);
    }
}
