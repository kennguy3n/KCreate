//! Native GPU gradient rasteriser.
//!
//! Evaluates a linear or radial colour gradient analytically on
//! the GPU via [`GRADIENT_WGSL`](super::GRADIENT_WGSL) and reads
//! the RGBA8 result back. This is the first real GPU *source-
//! generation* pass in the renderer: unlike the blur / levels /
//! unsharp filters it takes no pixel input, it synthesises the
//! gradient directly from the stop list inside a wgpu compute
//! pass instead of the current CPU-rasterise-then-upload round-
//! trip through `tiny-skia`.
//!
//! A bit-for-bit-equivalent scalar CPU reference
//! ([`cpu_render_gradient`]) lives alongside it and serves two
//! roles: the parity oracle the GPU output is checked against
//! (within ±1 / 255 per channel — the GPU's `mix` / `clamp`
//! rounding differs from scalar `f32` arithmetic by at most one
//! LSB), and the graceful fallback when no GPU is available. The
//! public [`gradient_image`] dispatcher picks the GPU path when a
//! context is present and falls back to the CPU reference
//! otherwise, so callers never branch on GPU availability —
//! exactly the contract the filter methods follow.
//!
//! Scope note: this is a self-contained gradient generator. It is
//! deliberately **not** swapped into the document present path,
//! whose gradient fills go through `tiny-skia` (premultiplied
//! interpolation + dithering) and are locked byte-exact by the
//! render-parity tests. Matching `tiny-skia` bit-for-bit on the
//! GPU is out of scope here; this provides the GPU gradient
//! building block the `gpu.rs` Phase 0 round-trip is intended to
//! grow into.

use wgpu::util::DeviceExt;

use super::{
    bytemuck_f32_slice, bytemuck_one, unpack_u32_to_rgba, ComputeError, GpuComputeContext,
    WORKGROUP_DIM,
};

const MODE_LINEAR: u32 = 0;
const MODE_RADIAL: u32 = 1;

/// Size of the `GradientParams` uniform block, exposed so tests
/// can catch WGSL/host layout drift (must stay a multiple of 16
/// for a uniform buffer and match the WGSL `GradientParams`).
pub const GRADIENT_PARAMS_SIZE: u64 = std::mem::size_of::<GradientParams>() as u64;

const _: () = {
    assert!(GRADIENT_PARAMS_SIZE == 48);
    // Offset contract vs the WGSL `GradientParams`: the four leading
    // `u32`s fill bytes 0..16, so each `vec2<f32>` lands on its 8-byte
    // WGSL alignment boundary. The size assert above catches size drift;
    // these catch a field reorder that would silently break the layout
    // match without changing the total size.
    assert!(std::mem::offset_of!(GradientParams, width) == 0);
    assert!(std::mem::offset_of!(GradientParams, height) == 4);
    assert!(std::mem::offset_of!(GradientParams, mode) == 8);
    assert!(std::mem::offset_of!(GradientParams, stop_count) == 12);
    assert!(std::mem::offset_of!(GradientParams, p0) == 16);
    assert!(std::mem::offset_of!(GradientParams, p1) == 24);
    assert!(std::mem::offset_of!(GradientParams, center) == 32);
    assert!(std::mem::offset_of!(GradientParams, radius) == 40);
};

/// A single colour stop: `offset` along the gradient axis in
/// `[0, 1]` and a straight-alpha RGBA colour with channels in
/// `[0, 1]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GradientStop {
    pub offset: f32,
    pub color: [f32; 4],
}

/// Gradient geometry. Coordinates are in the output pixel grid's
/// space; the rasteriser samples each pixel at its centre
/// (`x + 0.5`, `y + 0.5`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GradientKind {
    /// Linear gradient running from `from` to `to`. The colour at
    /// a pixel is the clamped projection of the pixel centre onto
    /// the `from → to` axis.
    Linear { from: [f32; 2], to: [f32; 2] },
    /// Radial gradient centred at `center` with the given
    /// `radius`. The colour at a pixel is the clamped distance
    /// from the centre divided by the radius.
    Radial { center: [f32; 2], radius: f32 },
}

/// A complete gradient-fill request: output dimensions, geometry,
/// and the colour-stop list (`Pad` spread; stops are clamped to
/// `[0, 1]` and sorted ascending before rasterisation).
#[derive(Clone, Debug, PartialEq)]
pub struct GradientSpec {
    pub width: u32,
    pub height: u32,
    pub kind: GradientKind,
    pub stops: Vec<GradientStop>,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct GradientParams {
    width: u32,
    height: u32,
    mode: u32,
    stop_count: u32,
    p0: [f32; 2],
    p1: [f32; 2],
    center: [f32; 2],
    radius: f32,
    _pad: f32,
}

impl GpuComputeContext {
    /// Rasterise a gradient fill to an RGBA8 buffer on the GPU.
    ///
    /// Returns `width * height * 4` bytes in `[R, G, B, A]` order.
    /// Errors when the dimensions are zero or the stop list is
    /// empty; both are recoverable — [`gradient_image`] falls back
    /// to the CPU reference in those cases.
    pub fn render_gradient(&self, spec: &GradientSpec) -> Result<Vec<u8>, ComputeError> {
        if spec.width == 0 || spec.height == 0 {
            return Err(ComputeError::ZeroGradientSize {
                width: spec.width,
                height: spec.height,
            });
        }
        if spec.stops.is_empty() {
            return Err(ComputeError::EmptyGradientStops);
        }

        // `prepare_stops` drops non-finite offsets; if that empties the
        // list, treat it like an empty request (recoverable — the
        // dispatcher falls back to the CPU reference, which paints
        // transparent) rather than dispatching with zero-size buffers.
        let stops = prepare_stops(&spec.stops);
        if stops.is_empty() {
            return Err(ComputeError::EmptyGradientStops);
        }
        let pixel_count = (spec.width as usize) * (spec.height as usize);
        let buf_size = (pixel_count * std::mem::size_of::<u32>()) as u64;

        // Stop buffers: tight `f32` offsets + tight `vec4<f32>`
        // colours (stride 16, matching the WGSL `array<vec4<f32>>`).
        let offsets: Vec<f32> = stops.iter().map(|s| s.offset).collect();
        let mut colors: Vec<f32> = Vec::with_capacity(stops.len() * 4);
        for s in &stops {
            colors.extend_from_slice(&s.color);
        }

        let offsets_buf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("compute-gradient-offsets"),
                contents: bytemuck_f32_slice(&offsets),
                usage: wgpu::BufferUsages::STORAGE,
            });
        let colors_buf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("compute-gradient-colors"),
                contents: bytemuck_f32_slice(&colors),
                usage: wgpu::BufferUsages::STORAGE,
            });
        let output_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("compute-gradient-output"),
            size: buf_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        let (mode, p0, p1, center, radius) = match spec.kind {
            GradientKind::Linear { from, to } => (MODE_LINEAR, from, to, [0.0, 0.0], 0.0),
            GradientKind::Radial { center, radius } => {
                (MODE_RADIAL, [0.0, 0.0], [0.0, 0.0], center, radius)
            }
        };
        let params = GradientParams {
            width: spec.width,
            height: spec.height,
            mode,
            stop_count: stops.len() as u32,
            p0,
            p1,
            center,
            radius,
            _pad: 0.0,
        };
        let params_buf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("compute-gradient-params"),
                contents: bytemuck_one(&params),
                usage: wgpu::BufferUsages::UNIFORM,
            });

        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("compute-gradient-bind"),
            layout: &self.gradient_bind_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: offsets_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: colors_buf.as_entire_binding(),
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
                label: Some("compute-gradient-encoder"),
            });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("compute-gradient-pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.gradient_pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(
                spec.width.div_ceil(WORKGROUP_DIM),
                spec.height.div_ceil(WORKGROUP_DIM),
                1,
            );
        }
        self.queue.submit(Some(encoder.finish()));

        let bytes = self.read_buffer_to_bytes(&output_buf, pixel_count)?;
        Ok(unpack_u32_to_rgba(&bytes))
    }
}

/// Render a gradient fill to an RGBA8 buffer, preferring the GPU
/// compute pipeline when `ctx` is available and falling back to
/// the scalar CPU reference otherwise (or when the GPU dispatch
/// returns a recoverable error). Callers invoke this
/// unconditionally without branching on GPU availability.
pub fn gradient_image(ctx: Option<&GpuComputeContext>, spec: &GradientSpec) -> Vec<u8> {
    if let Some(ctx) = ctx {
        match ctx.render_gradient(spec) {
            Ok(out) => return out,
            Err(err) => {
                eprintln!("gradient_image: GPU dispatch failed ({err}); using CPU reference");
            }
        }
    }
    cpu_render_gradient(spec)
}

/// Scalar CPU reference rasteriser — the parity oracle for the GPU
/// path and the fallback when no GPU is available. Produces
/// `width * height * 4` bytes in `[R, G, B, A]` order using the
/// same per-pixel-centre sampling, `Pad` spread, and straight-
/// alpha linear interpolation as the WGSL shader.
pub fn cpu_render_gradient(spec: &GradientSpec) -> Vec<u8> {
    let w = spec.width as usize;
    let h = spec.height as usize;
    let mut out = vec![0u8; w * h * 4];
    let stops = prepare_stops(&spec.stops);
    if stops.is_empty() {
        return out;
    }
    for y in 0..h {
        for x in 0..w {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let t = gradient_t(&spec.kind, px, py);
            let c = sample_color(&stops, t);
            let idx = (y * w + x) * 4;
            out[idx] = pack_channel(c[0]);
            out[idx + 1] = pack_channel(c[1]);
            out[idx + 2] = pack_channel(c[2]);
            out[idx + 3] = pack_channel(c[3]);
        }
    }
    out
}

/// Drop stops with non-finite offsets (`NaN`/`±inf`), clamp the rest
/// to `[0, 1]`, and sort ascending (stable), so both the GPU shader
/// and CPU reference can assume a sorted, finite, in-range stop list.
///
/// Filtering non-finite offsets keeps the two paths in lockstep: a
/// `NaN` offset would otherwise survive `clamp` (which propagates
/// `NaN`), sort to an arbitrary position, and then be skipped by the
/// shader's offset comparisons — yielding a malformed gradient. Both
/// paths now simply ignore such a stop.
fn prepare_stops(stops: &[GradientStop]) -> Vec<GradientStop> {
    let mut prepared: Vec<GradientStop> = stops
        .iter()
        .filter(|s| s.offset.is_finite())
        .map(|s| GradientStop {
            offset: s.offset.clamp(0.0, 1.0),
            color: s.color,
        })
        .collect();
    prepared.sort_by(|a, b| {
        a.offset
            .partial_cmp(&b.offset)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    prepared
}

/// Fraction along the gradient axis at pixel-centre `(px, py)`,
/// clamped to `[0, 1]` (`Pad` spread). Mirrors `gradient_t` in the
/// WGSL shader.
fn gradient_t(kind: &GradientKind, px: f32, py: f32) -> f32 {
    match *kind {
        GradientKind::Linear { from, to } => {
            let dx = to[0] - from[0];
            let dy = to[1] - from[1];
            let len2 = dx * dx + dy * dy;
            if len2 <= 1e-12 {
                0.0
            } else {
                (((px - from[0]) * dx + (py - from[1]) * dy) / len2).clamp(0.0, 1.0)
            }
        }
        GradientKind::Radial { center, radius } => {
            if radius <= 1e-6 {
                0.0
            } else {
                let dx = px - center[0];
                let dy = py - center[1];
                // Mirror the WGSL `length()` (= `sqrt(dot(v, v))`)
                // exactly rather than `f32::hypot`, so the CPU
                // reference stays bit-parity with the shader. Pixel
                // coordinates never approach the overflow range that
                // would make `hypot` worth its accuracy cost.
                #[allow(clippy::imprecise_flops)]
                let dist = (dx * dx + dy * dy).sqrt();
                (dist / radius).clamp(0.0, 1.0)
            }
        }
    }
}

/// Colour at parameter `t` across the pre-sorted stop list (must
/// be non-empty). Mirrors `sample_color` in the WGSL shader.
fn sample_color(stops: &[GradientStop], t: f32) -> [f32; 4] {
    let last = stops.len() - 1;
    if last == 0 || t <= stops[0].offset {
        return stops[0].color;
    }
    if t >= stops[last].offset {
        return stops[last].color;
    }
    for i in 0..last {
        let o0 = stops[i].offset;
        let o1 = stops[i + 1].offset;
        if t >= o0 && t <= o1 {
            let denom = o1 - o0;
            if denom < 1e-6 {
                return stops[i].color;
            }
            let local = (t - o0) / denom;
            let c0 = stops[i].color;
            let c1 = stops[i + 1].color;
            return [
                c0[0] + (c1[0] - c0[0]) * local,
                c0[1] + (c1[1] - c0[1]) * local,
                c0[2] + (c1[2] - c0[2]) * local,
                c0[3] + (c1[3] - c0[3]) * local,
            ];
        }
    }
    stops[last].color
}

/// Quantise a straight-alpha channel in `[0, 1]` to a byte using
/// the same `round-half-up + clamp` convention as the WGSL `pack`.
fn pack_channel(v: f32) -> u8 {
    (v * 255.0 + 0.5).clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stop(offset: f32, color: [f32; 4]) -> GradientStop {
        GradientStop { offset, color }
    }

    #[test]
    fn params_size_matches_wgsl() {
        // Must stay a multiple of 16 (uniform buffer) and match the
        // WGSL `GradientParams` struct layout.
        assert_eq!(GRADIENT_PARAMS_SIZE, 48);
        assert_eq!(GRADIENT_PARAMS_SIZE % 16, 0);
    }

    #[test]
    fn single_stop_fills_solid() {
        let spec = GradientSpec {
            width: 4,
            height: 4,
            kind: GradientKind::Linear {
                from: [0.0, 0.0],
                to: [4.0, 0.0],
            },
            stops: vec![stop(0.0, [0.25, 0.5, 0.75, 1.0])],
        };
        let out = cpu_render_gradient(&spec);
        let expected = [
            pack_channel(0.25),
            pack_channel(0.5),
            pack_channel(0.75),
            pack_channel(1.0),
        ];
        for px in out.chunks_exact(4) {
            assert_eq!(px, expected);
        }
    }

    #[test]
    fn linear_horizontal_ramp_is_monotonic() {
        let spec = GradientSpec {
            width: 256,
            height: 1,
            kind: GradientKind::Linear {
                from: [0.0, 0.0],
                to: [256.0, 0.0],
            },
            stops: vec![
                stop(0.0, [0.0, 0.0, 0.0, 1.0]),
                stop(1.0, [1.0, 1.0, 1.0, 1.0]),
            ],
        };
        let out = cpu_render_gradient(&spec);
        // Left edge darker than right edge.
        assert!(out[0] < out[255 * 4]);
        // Red channel is monotonic non-decreasing across the ramp.
        let mut prev = out[0];
        for x in 0..256 {
            let r = out[x * 4];
            assert!(r >= prev, "red must be non-decreasing at x={x}");
            prev = r;
        }
        // Alpha stays fully opaque.
        for px in out.chunks_exact(4) {
            assert_eq!(px[3], 255);
        }
    }

    #[test]
    fn linear_midpoint_is_halfway_color() {
        // 2px wide, black→white. Pixel centres at x=0.5 (t=0.25)
        // and x=1.5 (t=0.75) for a [0,2] axis.
        let spec = GradientSpec {
            width: 2,
            height: 1,
            kind: GradientKind::Linear {
                from: [0.0, 0.0],
                to: [2.0, 0.0],
            },
            stops: vec![
                stop(0.0, [0.0, 0.0, 0.0, 1.0]),
                stop(1.0, [1.0, 1.0, 1.0, 1.0]),
            ],
        };
        let out = cpu_render_gradient(&spec);
        assert_eq!(out[0], pack_channel(0.25));
        assert_eq!(out[4], pack_channel(0.75));
    }

    #[test]
    fn radial_brightens_toward_center() {
        let spec = GradientSpec {
            width: 64,
            height: 64,
            kind: GradientKind::Radial {
                center: [32.0, 32.0],
                radius: 32.0,
            },
            stops: vec![
                stop(0.0, [1.0, 1.0, 1.0, 1.0]),
                stop(1.0, [0.0, 0.0, 0.0, 1.0]),
            ],
        };
        let out = cpu_render_gradient(&spec);
        let center_idx = (32 * 64 + 32) * 4;
        assert!(
            out[center_idx] > out[0],
            "centre must be brighter than the corner"
        );
    }

    #[test]
    fn pad_spread_clamps_outside_range() {
        // Stops only span [0.25, 0.75]; pixels outside clamp to the
        // nearest stop colour.
        let spec = GradientSpec {
            width: 100,
            height: 1,
            kind: GradientKind::Linear {
                from: [0.0, 0.0],
                to: [100.0, 0.0],
            },
            stops: vec![
                stop(0.25, [0.2, 0.2, 0.2, 1.0]),
                stop(0.75, [0.8, 0.8, 0.8, 1.0]),
            ],
        };
        let out = cpu_render_gradient(&spec);
        assert_eq!(out[0], pack_channel(0.2));
        assert_eq!(out[99 * 4], pack_channel(0.8));
    }

    #[test]
    fn degenerate_geometry_collapses_to_first_stop() {
        // Zero-length linear axis → t == 0 everywhere.
        let spec = GradientSpec {
            width: 3,
            height: 3,
            kind: GradientKind::Linear {
                from: [1.0, 1.0],
                to: [1.0, 1.0],
            },
            stops: vec![
                stop(0.0, [0.1, 0.2, 0.3, 1.0]),
                stop(1.0, [0.9, 0.8, 0.7, 1.0]),
            ],
        };
        let out = cpu_render_gradient(&spec);
        let expected = [
            pack_channel(0.1),
            pack_channel(0.2),
            pack_channel(0.3),
            pack_channel(1.0),
        ];
        for px in out.chunks_exact(4) {
            assert_eq!(px, expected);
        }
    }

    #[test]
    fn unsorted_stops_are_sorted_before_sampling() {
        // Stops supplied out of order must produce the same ramp as
        // sorted input.
        let unsorted = GradientSpec {
            width: 32,
            height: 1,
            kind: GradientKind::Linear {
                from: [0.0, 0.0],
                to: [32.0, 0.0],
            },
            stops: vec![
                stop(1.0, [1.0, 0.0, 0.0, 1.0]),
                stop(0.0, [0.0, 0.0, 1.0, 1.0]),
            ],
        };
        let sorted = GradientSpec {
            stops: vec![
                stop(0.0, [0.0, 0.0, 1.0, 1.0]),
                stop(1.0, [1.0, 0.0, 0.0, 1.0]),
            ],
            ..unsorted
        };
        assert_eq!(cpu_render_gradient(&unsorted), cpu_render_gradient(&sorted));
    }

    #[test]
    fn non_finite_offsets_are_dropped_before_sampling() {
        // NaN / ±inf offsets must be filtered out (they would otherwise
        // survive `clamp` and sort to an arbitrary position), leaving
        // only the finite stops in sorted order.
        let prepared = prepare_stops(&[
            stop(0.75, [0.0, 0.0, 0.0, 1.0]),
            stop(f32::NAN, [1.0, 0.0, 0.0, 1.0]),
            stop(0.25, [0.0, 1.0, 0.0, 1.0]),
            stop(f32::INFINITY, [0.0, 0.0, 1.0, 1.0]),
            stop(f32::NEG_INFINITY, [1.0, 1.0, 0.0, 1.0]),
        ]);
        let offsets: Vec<f32> = prepared.iter().map(|s| s.offset).collect();
        assert_eq!(offsets, vec![0.25, 0.75]);
    }

    #[test]
    fn non_finite_stop_does_not_change_render() {
        // A stop with a non-finite offset must render identically to the
        // same spec with that stop removed — both the GPU shader and CPU
        // reference simply ignore it, keeping the two paths in lockstep.
        let base = GradientSpec {
            width: 48,
            height: 1,
            kind: GradientKind::Linear {
                from: [0.0, 0.0],
                to: [48.0, 0.0],
            },
            stops: vec![
                stop(0.0, [0.0, 0.0, 1.0, 1.0]),
                stop(1.0, [1.0, 0.0, 0.0, 1.0]),
            ],
        };
        let with_nan = GradientSpec {
            stops: vec![
                stop(0.0, [0.0, 0.0, 1.0, 1.0]),
                stop(f32::NAN, [0.0, 1.0, 0.0, 1.0]),
                stop(1.0, [1.0, 0.0, 0.0, 1.0]),
            ],
            ..base
        };
        assert_eq!(cpu_render_gradient(&with_nan), cpu_render_gradient(&base));
    }

    #[test]
    fn gradient_image_without_context_uses_cpu_reference() {
        // The public dispatcher must produce the CPU reference output
        // byte-for-byte when no GPU context is supplied — the offline
        // fallback contract callers rely on.
        let spec = GradientSpec {
            width: 16,
            height: 8,
            kind: GradientKind::Linear {
                from: [0.0, 0.0],
                to: [16.0, 0.0],
            },
            stops: vec![
                stop(0.0, [0.1, 0.2, 0.3, 1.0]),
                stop(1.0, [0.7, 0.6, 0.5, 1.0]),
            ],
        };
        assert_eq!(gradient_image(None, &spec), cpu_render_gradient(&spec));
    }
}
