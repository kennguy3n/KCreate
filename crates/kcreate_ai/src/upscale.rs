//! Lanczos3 image upscaling.
//!
//! Real separable Lanczos3 resampling implemented in pure Rust. Used
//! by the AI tasks bridge to upscale raster layers without round-tripping
//! through an ONNX neural model — the neural alternative (ESRGAN-style)
//! is a Phase 3 optional model pack.
//!
//! Algorithm: produce the upscaled image in two passes.
//! 1. Horizontal pass — for each output pixel, compute a weighted sum
//!    of `2 * kernel_radius` source samples via the Lanczos3 windowed
//!    sinc kernel; alpha is premultiplied to keep edge pixels correct.
//! 2. Vertical pass — same kernel applied across rows of the
//!    intermediate buffer.
//!
//! `rayon` is used to parallelise the per-row / per-column work.

use rayon::prelude::*;
use thiserror::Error;

const LANCZOS_RADIUS: f32 = 3.0;
const KERNEL_SAMPLES_PER_TAP: usize = 6; // 2 * radius for radius == 3

/// Errors from [`upscale_lanczos`].
#[derive(Debug, Error, PartialEq, Eq)]
pub enum UpscaleError {
    #[error(
        "invalid dimensions: width and height must be > 0 and pixels.len() == width * height * 4"
    )]
    InvalidDimensions,
    #[error("invalid scale: {0}; must be > 1.0 and finite")]
    InvalidScale(String),
    #[error("output dimensions overflow")]
    Overflow,
}

/// Upscale an RGBA8 image by `scale` using Lanczos3 resampling.
///
/// `scale` may be any value `> 1.0`. The common cases are `2.0` and
/// `4.0`; values in between (1.5, 3.0, etc.) are also accepted. The
/// returned buffer is `(new_w, new_h)` pixels in RGBA8 byte order.
///
/// Note: this is a genuine resampling kernel — not a neural model and
/// not nearest-neighbour. A horizontal solid line in the source remains
/// a sharp line in the output, but soft features pick up the
/// characteristic Lanczos3 ringing that gives the algorithm its name.
pub fn upscale_lanczos(
    pixels: &[u8],
    width: u32,
    height: u32,
    scale: f64,
) -> Result<(Vec<u8>, u32, u32), UpscaleError> {
    // `scale` is `f64` so values arriving from JavaScript (which only
    // has `f64` numbers) survive the FFI boundary intact. Casting to
    // `f32` at the bridge layer rounded values just above 1.0 down to
    // exactly 1.0 and made the `> 1.0` validation below reject
    // otherwise-legitimate inputs. Per Devin Review
    // ANALYSIS_pr-review-job-0594c03f68c24589ba78a32926e3874f_0004.
    if width == 0 || height == 0 {
        return Err(UpscaleError::InvalidDimensions);
    }
    let expected_len = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4))
        .ok_or(UpscaleError::Overflow)?;
    if pixels.len() != expected_len {
        return Err(UpscaleError::InvalidDimensions);
    }
    if !scale.is_finite() || scale <= 1.0 {
        return Err(UpscaleError::InvalidScale(format!("{scale}")));
    }

    let new_w_f = f64::from(width) * scale;
    let new_h_f = f64::from(height) * scale;
    if !new_w_f.is_finite() || !new_h_f.is_finite() || new_w_f > f64::from(u32::MAX) {
        return Err(UpscaleError::Overflow);
    }
    let new_w = (new_w_f.round() as u32).max(1);
    let new_h = (new_h_f.round() as u32).max(1);

    // Premultiply RGBA into f32 for resampling stability.
    let mut src_f: Vec<f32> = Vec::with_capacity((width as usize) * (height as usize) * 4);
    for chunk in pixels.chunks_exact(4) {
        let a = f32::from(chunk[3]) / 255.0;
        src_f.push(f32::from(chunk[0]) / 255.0 * a);
        src_f.push(f32::from(chunk[1]) / 255.0 * a);
        src_f.push(f32::from(chunk[2]) / 255.0 * a);
        src_f.push(a);
    }

    // Horizontal pass — produce an intermediate of size (new_w x height).
    let h_taps = build_taps(width, new_w, scale);
    let src_last_x = (width as usize).saturating_sub(1);
    let mut intermediate: Vec<f32> = vec![0.0; (new_w as usize) * (height as usize) * 4];
    intermediate
        .par_chunks_mut((new_w as usize) * 4)
        .enumerate()
        .for_each(|(y, row)| {
            let src_row = &src_f[(y * width as usize * 4)..((y + 1) * width as usize * 4)];
            for x in 0..new_w as usize {
                let (start, weights) = &h_taps[x];
                let mut acc = [0.0f32; 4];
                for (i, w) in weights.iter().enumerate() {
                    let sx = (*start + i).min(src_last_x);
                    let p = &src_row[(sx * 4)..(sx * 4 + 4)];
                    acc[0] += p[0] * w;
                    acc[1] += p[1] * w;
                    acc[2] += p[2] * w;
                    acc[3] += p[3] * w;
                }
                row[x * 4] = acc[0];
                row[x * 4 + 1] = acc[1];
                row[x * 4 + 2] = acc[2];
                row[x * 4 + 3] = acc[3];
            }
        });

    // Vertical pass — produce final (new_w x new_h).
    let v_taps = build_taps(height, new_h, scale);
    let src_last_y = (height as usize).saturating_sub(1);
    let mut out_f: Vec<f32> = vec![0.0; (new_w as usize) * (new_h as usize) * 4];
    out_f
        .par_chunks_mut((new_w as usize) * 4)
        .enumerate()
        .for_each(|(y, row)| {
            let (start, weights) = &v_taps[y];
            for x in 0..new_w as usize {
                let mut acc = [0.0f32; 4];
                for (i, w) in weights.iter().enumerate() {
                    let sy = (*start + i).min(src_last_y);
                    let src = &intermediate
                        [(sy * new_w as usize * 4 + x * 4)..(sy * new_w as usize * 4 + x * 4 + 4)];
                    acc[0] += src[0] * w;
                    acc[1] += src[1] * w;
                    acc[2] += src[2] * w;
                    acc[3] += src[3] * w;
                }
                row[x * 4] = acc[0];
                row[x * 4 + 1] = acc[1];
                row[x * 4 + 2] = acc[2];
                row[x * 4 + 3] = acc[3];
            }
        });

    // Convert back to u8, un-premultiplying alpha.
    let mut out = Vec::with_capacity((new_w as usize) * (new_h as usize) * 4);
    for chunk in out_f.chunks_exact(4) {
        let a = chunk[3].clamp(0.0, 1.0);
        let (r, g, b) = if a > 0.0 {
            (
                (chunk[0] / a).clamp(0.0, 1.0),
                (chunk[1] / a).clamp(0.0, 1.0),
                (chunk[2] / a).clamp(0.0, 1.0),
            )
        } else {
            (0.0, 0.0, 0.0)
        };
        out.push((r * 255.0).round().clamp(0.0, 255.0) as u8);
        out.push((g * 255.0).round().clamp(0.0, 255.0) as u8);
        out.push((b * 255.0).round().clamp(0.0, 255.0) as u8);
        out.push((a * 255.0).round().clamp(0.0, 255.0) as u8);
    }

    Ok((out, new_w, new_h))
}

/// Pre-compute the Lanczos3 kernel taps for each output index.
///
/// Returns `(start_src_index, weights[KERNEL_SAMPLES_PER_TAP])` per
/// output pixel. The kernel always reads `KERNEL_SAMPLES_PER_TAP`
/// source pixels starting at `start_src_index`; boundary handling uses
/// the standard "clamp to edge" rule — out-of-range indices fold into
/// the nearest valid pixel by accumulating their weight onto that
/// pixel. Final weights are renormalised to sum to 1.0.
fn build_taps(
    src_len: u32,
    dst_len: u32,
    scale: f64,
) -> Vec<(usize, [f32; KERNEL_SAMPLES_PER_TAP])> {
    // Compute kernel centers in `f64` so a `scale` of 1.0000001 isn't
    // silently snapped to 1.0 before the inverse. The Lanczos kernel
    // itself stays in `f32` — pixel weights don't need 53-bit
    // mantissa precision. Per Devin Review
    // ANALYSIS_pr-review-job-0594c03f68c24589ba78a32926e3874f_0004.
    let inv_scale = 1.0_f64 / scale;
    let mut out = Vec::with_capacity(dst_len as usize);
    let src_last_idx = src_len.saturating_sub(1) as i32;
    for d in 0..dst_len {
        let center_f64 = (f64::from(d) + 0.5) * inv_scale - 0.5;
        let center = center_f64 as f32;
        let left = (center_f64 - f64::from(LANCZOS_RADIUS)).floor() as i32;
        // Anchor the tap window inside [0, src_len-KERNEL]; smaller
        // images still produce a valid window where indices repeat the
        // edge pixel.
        let max_start = (src_last_idx + 1) - KERNEL_SAMPLES_PER_TAP as i32;
        let start = left.clamp(0, max_start.max(0)) as usize;
        let mut weights = [0.0f32; KERNEL_SAMPLES_PER_TAP];
        let mut sum = 0.0;
        for i in 0..KERNEL_SAMPLES_PER_TAP {
            // What source index would the un-clamped kernel read here?
            let virt = left + i as i32;
            let clamped = virt.clamp(0, src_last_idx) as usize;
            // Weight is from the un-clamped Lanczos kernel.
            let dx = (virt as f32) - center;
            let w = lanczos(dx, LANCZOS_RADIUS);
            // Fold into the tap slot that holds the clamped pixel.
            let tap_slot = clamped
                .saturating_sub(start)
                .min(KERNEL_SAMPLES_PER_TAP - 1);
            weights[tap_slot] += w;
            sum += w;
        }
        if sum.abs() > 1e-6 {
            for w in &mut weights {
                *w /= sum;
            }
        }
        out.push((start, weights));
    }
    out
}

fn lanczos(x: f32, a: f32) -> f32 {
    if x.abs() < 1e-6 {
        return 1.0;
    }
    if x.abs() >= a {
        return 0.0;
    }
    let pix = std::f32::consts::PI * x;
    let pix_a = pix / a;
    (pix.sin() / pix) * (pix_a.sin() / pix_a)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: u32, height: u32, rgba: [u8; 4]) -> Vec<u8> {
        let mut v = Vec::with_capacity((width as usize) * (height as usize) * 4);
        for _ in 0..(width as usize) * (height as usize) {
            v.extend_from_slice(&rgba);
        }
        v
    }

    #[test]
    fn rejects_zero_dimensions() {
        assert!(matches!(
            upscale_lanczos(&[], 0, 1, 2.0),
            Err(UpscaleError::InvalidDimensions)
        ));
    }

    #[test]
    fn rejects_buffer_size_mismatch() {
        let pixels = vec![0u8; 12];
        assert!(matches!(
            upscale_lanczos(&pixels, 4, 4, 2.0),
            Err(UpscaleError::InvalidDimensions)
        ));
    }

    #[test]
    fn rejects_scale_le_one() {
        let pixels = solid(2, 2, [255, 255, 255, 255]);
        assert!(matches!(
            upscale_lanczos(&pixels, 2, 2, 1.0),
            Err(UpscaleError::InvalidScale(_))
        ));
        assert!(matches!(
            upscale_lanczos(&pixels, 2, 2, 0.5),
            Err(UpscaleError::InvalidScale(_))
        ));
        assert!(matches!(
            upscale_lanczos(&pixels, 2, 2, f64::NAN),
            Err(UpscaleError::InvalidScale(_))
        ));
    }

    #[test]
    fn upscale_2x_preserves_dimensions() {
        let pixels = solid(8, 4, [50, 100, 200, 255]);
        let (out, w, h) = upscale_lanczos(&pixels, 8, 4, 2.0).expect("upscale");
        assert_eq!(w, 16);
        assert_eq!(h, 8);
        assert_eq!(out.len(), 16 * 8 * 4);
    }

    #[test]
    fn upscale_4x_preserves_dimensions() {
        let pixels = solid(4, 4, [0, 0, 0, 255]);
        let (out, w, h) = upscale_lanczos(&pixels, 4, 4, 4.0).expect("upscale");
        assert_eq!(w, 16);
        assert_eq!(h, 16);
        assert_eq!(out.len(), 16 * 16 * 4);
    }

    #[test]
    fn solid_color_upscale_stays_solid() {
        // A solid image must remain solid after upscale (every output
        // pixel matches the input colour).
        let pixels = solid(8, 8, [200, 100, 50, 255]);
        let (out, _, _) = upscale_lanczos(&pixels, 8, 8, 2.0).expect("upscale");
        for chunk in out.chunks_exact(4) {
            // Allow ±2 LSB drift from float round-trip.
            for (i, expected) in [200u8, 100, 50, 255].iter().enumerate() {
                let diff = i32::from(chunk[i]) - i32::from(*expected);
                assert!(
                    diff.abs() <= 2,
                    "channel {i} drifted: got {} expected ~{}",
                    chunk[i],
                    expected
                );
            }
        }
    }

    #[test]
    fn checkerboard_upscale_produces_non_nearest_neighbour_pixels() {
        // 4×4 black/white checkerboard, upscaled 2x. A nearest-neighbour
        // upscale would only produce {0, 255}. Lanczos3 produces
        // intermediate values across the borders.
        let mut pixels = Vec::with_capacity(4 * 4 * 4);
        for y in 0..4u32 {
            for x in 0..4u32 {
                let v = if (x + y) % 2 == 0 { 255 } else { 0 };
                pixels.extend_from_slice(&[v, v, v, 255]);
            }
        }
        let (out, _, _) = upscale_lanczos(&pixels, 4, 4, 2.0).expect("upscale");
        let intermediate_count = out
            .chunks_exact(4)
            .filter(|c| c[0] > 5 && c[0] < 250)
            .count();
        assert!(
            intermediate_count > 0,
            "Lanczos3 should produce intermediate luminance values"
        );
    }

    #[test]
    fn transparent_pixels_stay_transparent() {
        let pixels = solid(4, 4, [255, 0, 0, 0]);
        let (out, _, _) = upscale_lanczos(&pixels, 4, 4, 2.0).expect("upscale");
        for chunk in out.chunks_exact(4) {
            assert_eq!(chunk[3], 0);
        }
    }

    #[test]
    fn alpha_gradient_upscale_preserves_alpha_range() {
        let mut pixels = Vec::with_capacity(16 * 4);
        for i in 0..16u8 {
            pixels.extend_from_slice(&[128, 64, 32, i * 16]);
        }
        let (out, _, _) = upscale_lanczos(&pixels, 16, 1, 2.0).expect("upscale");
        // Alpha range in output should span roughly [0, 240].
        // The alpha channel sits at byte indices 3, 7, 11, ... — i.e.
        // `skip(3).step_by(4)`. Iterator adapters do NOT commute here:
        // `step_by(4).skip(3)` first picks 0, 4, 8, ... (the R channel)
        // then drops the first three of those, which yielded the R
        // channel of pixel 3 onward and made this assertion meaningless.
        let min = out.iter().skip(3).step_by(4).copied().min().unwrap_or(0);
        let max = out.iter().skip(3).step_by(4).copied().max().unwrap_or(0);
        assert!(
            i32::from(max) - i32::from(min) > 100,
            "alpha spread should be wide"
        );
    }
}
