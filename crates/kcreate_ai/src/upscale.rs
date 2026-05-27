//! Lanczos3 image upscaling + optional ONNX (ESRGAN) neural backend.
//!
//! [`upscale_lanczos`] is a real separable Lanczos3 resampling
//! implementation in pure Rust. It runs everywhere — no native deps,
//! no GPU, no model file.
//!
//! [`upscale_with_backend`] is the dispatcher used by the bridge. It
//! picks between the Lanczos3 path and the [`UpscaleBackend::Esrgan`]
//! ONNX path. The ONNX path is gated behind the `onnx_upscale`
//! feature so default builds — including every editing-path crate
//! that depends on `kcreate_ai` — stay free of the ONNX Runtime
//! native binary. When the feature is off the dispatcher returns
//! [`UpscaleError::BackendUnavailable`] for `Esrgan` so the caller
//! can fall back gracefully.
//!
//! Lanczos3 algorithm: produce the upscaled image in two passes.
//! 1. Horizontal pass — for each output pixel, compute a weighted sum
//!    of `2 * kernel_radius` source samples via the Lanczos3 windowed
//!    sinc kernel; alpha is premultiplied to keep edge pixels correct.
//! 2. Vertical pass — same kernel applied across rows of the
//!    intermediate buffer.
//!
//! `rayon` is used to parallelise the per-row / per-column work.

#[cfg(feature = "onnx_upscale")]
use std::path::Path;

use rayon::prelude::*;
use thiserror::Error;

const LANCZOS_RADIUS: f32 = 3.0;
const KERNEL_SAMPLES_PER_TAP: usize = 6; // 2 * radius for radius == 3

/// Real-ESRGAN ships as a 4× super-resolution network with a 128 px
/// receptive field; we tile the input with an 8 px overlap on each
/// side so seams between adjacent tiles blend out. Exposed as module
/// constants (rather than function-local) so the per-tile crop logic
/// is unit-testable without needing an ONNX model on disk.
///
/// `#[allow(dead_code)]` is applied because the constants are only
/// referenced from the `onnx_upscale`-gated ESRGAN code path and from
/// the test module. Default builds (no `onnx_upscale`, no `--tests`)
/// would otherwise warn — incorrectly, since the constants are part
/// of the documented module API for the ESRGAN tile geometry.
#[allow(dead_code)]
pub(crate) const ESRGAN_NET_SCALE: u32 = 4;
#[allow(dead_code)]
pub(crate) const ESRGAN_TILE: u32 = 128;
#[allow(dead_code)]
pub(crate) const ESRGAN_OVERLAP: u32 = 8;

/// Per-tile crop in upscaled-output pixels. Returns
/// `(left, top, right, bottom)` — the number of pixels at the start
/// and end of each axis that should be discarded before writing the
/// tile's output into the global buffer.
///
/// Adjacent tiles overlap by `OVERLAP * NET_SCALE` upscaled pixels on
/// the shared side. Cropping that region from both tiles eliminates
/// the seam that would otherwise be visible from the zero-padding of
/// the network's receptive field. At image boundaries there's no
/// neighbouring tile to fill the cropped region, so the crop on those
/// sides MUST be zero — otherwise the boundary pixels stay at the
/// `0.0` initial value of the output buffer, rendering them as
/// solid black.
///
/// This was previously hard-coded to a fixed crop on all four sides,
/// producing an 8-source-pixel-wide black border on every output and
/// turning images smaller than 16 px wide into all-black tiles. See
/// BUG-0001 in the Devin Review history on PR #16.
#[allow(dead_code)]
#[must_use]
pub(crate) fn esrgan_tile_crop(
    tx: u32,
    ty: u32,
    step: u32,
    width: u32,
    height: u32,
) -> (u32, u32, u32, u32) {
    let crop = ESRGAN_OVERLAP * ESRGAN_NET_SCALE;
    let crop_left = if tx > 0 { crop } else { 0 };
    let crop_top = if ty > 0 { crop } else { 0 };
    let crop_right = if tx + step < width { crop } else { 0 };
    let crop_bottom = if ty + step < height { crop } else { 0 };
    (crop_left, crop_top, crop_right, crop_bottom)
}

/// Errors from [`upscale_lanczos`] / [`upscale_with_backend`].
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
    /// The requested backend is not compiled in (e.g. `Esrgan` on a
    /// build that disabled the `onnx_upscale` Cargo feature). The
    /// caller is expected to fall back to a built-in backend (the
    /// dispatcher itself does not auto-fall-back — surfacing the
    /// missing backend explicitly avoids silent quality changes).
    #[error("backend {0:?} is not available in this build")]
    BackendUnavailable(UpscaleBackend),
    /// Backend-specific runtime failure (e.g. ONNX model file
    /// missing, weights load error, tensor shape mismatch, native
    /// runtime panic). The wrapped string is the diagnostic from
    /// the backend.
    #[error("backend {backend:?} failed: {message}")]
    BackendRuntime {
        backend: UpscaleBackend,
        message: String,
    },
}

/// Upscale backend selector.
///
/// `Lanczos3` is always available; `Esrgan` requires both the
/// `onnx_upscale` Cargo feature AND a valid ESRGAN ONNX model file
/// on disk (installed via [`crate::install_model_pack`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpscaleBackend {
    /// Built-in separable Lanczos3 resampling. Pure Rust, no native
    /// deps, runs everywhere.
    Lanczos3,
    /// Real-ESRGAN x4 ONNX inference. Requires the `upscale_esrgan`
    /// model pack and the `onnx_upscale` Cargo feature.
    Esrgan,
}

/// Dispatch upscale to the requested backend.
///
/// For [`UpscaleBackend::Lanczos3`] this is identical to calling
/// [`upscale_lanczos`] directly. For [`UpscaleBackend::Esrgan`] the
/// function loads the ONNX model at `model_path` (a tile-based
/// inference is used internally to bound memory regardless of input
/// dimensions). The caller is expected to pass `scale = 4.0` for
/// ESRGAN; other values fall back to a post-network Lanczos3 resize.
///
/// On a build without the `onnx_upscale` feature, `Esrgan` returns
/// [`UpscaleError::BackendUnavailable`] without touching the file
/// system — the caller can detect this and fall back to Lanczos3
/// without paying for the lookup.
pub fn upscale_with_backend(
    pixels: &[u8],
    width: u32,
    height: u32,
    scale: f64,
    backend: UpscaleBackend,
    model_path: Option<&std::path::Path>,
) -> Result<(Vec<u8>, u32, u32), UpscaleError> {
    match backend {
        UpscaleBackend::Lanczos3 => upscale_lanczos(pixels, width, height, scale),
        UpscaleBackend::Esrgan => {
            #[cfg(feature = "onnx_upscale")]
            {
                let path = model_path.ok_or_else(|| UpscaleError::BackendRuntime {
                    backend: UpscaleBackend::Esrgan,
                    message: "ESRGAN selected but no model_path provided".into(),
                })?;
                run_onnx_esrgan(path, pixels, width, height, scale)
            }
            #[cfg(not(feature = "onnx_upscale"))]
            {
                let _ = model_path;
                let _ = (pixels, width, height, scale);
                Err(UpscaleError::BackendUnavailable(UpscaleBackend::Esrgan))
            }
        }
    }
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

/// Run Real-ESRGAN-x4-plus inference on the supplied RGBA frame and
/// return the upscaled buffer.
///
/// The reference Real-ESRGAN ONNX export uses a tiled inference
/// loop because the network is fully convolutional and intermediate
/// activations grow quadratically in the input size. We mirror that
/// here with a fixed 128×128 input tile and 8-pixel overlap so any
/// input dimension is handled with bounded memory.
///
/// Internally we always run the network at its native 4× scale and
/// then resample to the caller-requested `scale` with Lanczos3 if it
/// differs (post-network re-resampling is the standard ESRGAN
/// integration pattern; the ONNX network itself is fixed at 4×).
#[cfg(feature = "onnx_upscale")]
fn run_onnx_esrgan(
    model_path: &Path,
    input_rgba: &[u8],
    width: u32,
    height: u32,
    scale: f64,
) -> Result<(Vec<u8>, u32, u32), UpscaleError> {
    use ort::session::Session;
    use ort::value::TensorRef;

    // Validate dimensions before any allocation so we surface
    // structural errors without spinning up the ONNX runtime.
    if width == 0 || height == 0 {
        return Err(UpscaleError::InvalidDimensions);
    }
    let expected_len = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4))
        .ok_or(UpscaleError::Overflow)?;
    if input_rgba.len() != expected_len {
        return Err(UpscaleError::InvalidDimensions);
    }
    if !scale.is_finite() || scale <= 1.0 {
        return Err(UpscaleError::InvalidScale(format!("{scale}")));
    }

    if !model_path.exists() {
        return Err(UpscaleError::BackendRuntime {
            backend: UpscaleBackend::Esrgan,
            message: format!("model file not found: {}", model_path.display()),
        });
    }
    let session = Session::builder()
        .map_err(|e| UpscaleError::BackendRuntime {
            backend: UpscaleBackend::Esrgan,
            message: format!("ort builder: {e}"),
        })?
        .commit_from_file(model_path)
        .map_err(|e| UpscaleError::BackendRuntime {
            backend: UpscaleBackend::Esrgan,
            message: format!("ort load: {e}"),
        })?;

    // Real-ESRGAN ships as a 4× super-resolution network. Constants
    // live at module scope so the per-tile crop math is unit-testable
    // (see `esrgan_tile_crop` + the tests below).
    const NET_SCALE: u32 = ESRGAN_NET_SCALE;
    const TILE: u32 = ESRGAN_TILE;
    const OVERLAP: u32 = ESRGAN_OVERLAP;

    let out_w = width.saturating_mul(NET_SCALE);
    let out_h = height.saturating_mul(NET_SCALE);
    let out_pixels = (out_w as usize) * (out_h as usize);
    let mut out_rgb: Vec<f32> = vec![0.0; out_pixels * 3];

    // Extract straight (non-premultiplied) RGB in [0,1] for the
    // ESRGAN network. The network was trained on straight RGB so
    // feeding premultiplied values would darken semi-transparent
    // pixels and produce incorrect super-resolution output. Alpha
    // is upscaled separately via Lanczos3 below.
    let src_pixels = (width as usize) * (height as usize);
    let mut src_rgb: Vec<f32> = Vec::with_capacity(src_pixels * 3);
    for chunk in input_rgba.chunks_exact(4) {
        src_rgb.push(f32::from(chunk[0]) / 255.0);
        src_rgb.push(f32::from(chunk[1]) / 255.0);
        src_rgb.push(f32::from(chunk[2]) / 255.0);
    }

    // Tile iteration with overlap. We feed `TILE × TILE` patches
    // through the network and crop the central region back into the
    // output buffer so neighbouring tile seams disappear.
    let step = TILE.saturating_sub(OVERLAP * 2).max(1);
    for ty in (0..height).step_by(step as usize) {
        for tx in (0..width).step_by(step as usize) {
            let in_x = tx.saturating_sub(OVERLAP);
            let in_y = ty.saturating_sub(OVERLAP);
            let in_w = (TILE).min(width - in_x);
            let in_h = (TILE).min(height - in_y);

            // Build NCHW input tensor in [0,1] straight RGB.
            let mut tile = vec![0f32; (3 * TILE * TILE) as usize];
            let plane = (TILE * TILE) as usize;
            for y in 0..in_h {
                for x in 0..in_w {
                    let src_idx = (((in_y + y) * width + (in_x + x)) * 3) as usize;
                    let dst = (y * TILE + x) as usize;
                    tile[dst] = src_rgb[src_idx];
                    tile[plane + dst] = src_rgb[src_idx + 1];
                    tile[2 * plane + dst] = src_rgb[src_idx + 2];
                }
            }

            let shape = [1_i64, 3, i64::from(TILE), i64::from(TILE)];
            let input =
                TensorRef::from_array_view((shape.as_slice(), tile.as_slice())).map_err(|e| {
                    UpscaleError::BackendRuntime {
                        backend: UpscaleBackend::Esrgan,
                        message: format!("ort tensor: {e}"),
                    }
                })?;
            let outputs =
                session
                    .run(ort::inputs![input])
                    .map_err(|e| UpscaleError::BackendRuntime {
                        backend: UpscaleBackend::Esrgan,
                        message: format!("ort run: {e}"),
                    })?;

            let (_, raw) = outputs[0].try_extract_tensor::<f32>().map_err(|e| {
                UpscaleError::BackendRuntime {
                    backend: UpscaleBackend::Esrgan,
                    message: format!("ort extract: {e}"),
                }
            })?;

            // Output is NCHW (1 × 3 × TILE*4 × TILE*4). Crop the
            // overlap border only on sides that have a neighbouring
            // tile; at image boundaries the network output is the
            // only source of pixels and must not be discarded.
            let out_tile = TILE * NET_SCALE;
            let out_plane = (out_tile * out_tile) as usize;
            let (crop_left, crop_top, crop_right, crop_bottom) =
                esrgan_tile_crop(tx, ty, step, width, height);
            let y_end = (in_h * NET_SCALE).saturating_sub(crop_bottom);
            let x_end = (in_w * NET_SCALE).saturating_sub(crop_right);
            for y in crop_top..y_end {
                for x in crop_left..x_end {
                    let src = (y * out_tile + x) as usize;
                    let gx = (in_x * NET_SCALE + x) as usize;
                    let gy = (in_y * NET_SCALE + y) as usize;
                    if gx < out_w as usize && gy < out_h as usize {
                        let dst = (gy * out_w as usize + gx) * 3;
                        out_rgb[dst] = raw[src].clamp(0.0, 1.0);
                        out_rgb[dst + 1] = raw[out_plane + src].clamp(0.0, 1.0);
                        out_rgb[dst + 2] = raw[2 * out_plane + src].clamp(0.0, 1.0);
                    }
                }
            }
        }
    }

    // Alpha channel is upscaled with Lanczos3 because the network
    // never sees it.
    let mut alpha_rgba: Vec<u8> = Vec::with_capacity((width as usize) * (height as usize) * 4);
    for chunk in input_rgba.chunks_exact(4) {
        alpha_rgba.extend_from_slice(&[chunk[3], chunk[3], chunk[3], 255]);
    }
    let (alpha_up, _, _) = upscale_lanczos(&alpha_rgba, width, height, f64::from(NET_SCALE))?;

    // Compose straight RGB (from ESRGAN) + A (from Lanczos) into the
    // final RGBA output buffer. No un-premultiplication needed because
    // the network operated on straight RGB.
    let mut out = Vec::with_capacity(out_pixels * 4);
    for (i, rgb) in out_rgb.chunks_exact(3).enumerate() {
        let a_byte = alpha_up[i * 4];
        out.push((rgb[0] * 255.0).round().clamp(0.0, 255.0) as u8);
        out.push((rgb[1] * 255.0).round().clamp(0.0, 255.0) as u8);
        out.push((rgb[2] * 255.0).round().clamp(0.0, 255.0) as u8);
        out.push(a_byte);
    }

    // If the caller asked for a non-4× scale, resample post-network.
    let net_scale_f = f64::from(NET_SCALE);
    if (scale - net_scale_f).abs() < 1e-6 {
        Ok((out, out_w, out_h))
    } else {
        // Final resize from 4× output to requested scale via
        // Lanczos3 on the network's RGBA frame.
        let target_scale = scale / net_scale_f;
        if target_scale <= 1.0 {
            // Asked for less than 4× → downsample via point sampling
            // (Lanczos kernel below 1.0 produces ringing artifacts;
            // a simple nearest-neighbour is a defensible fallback
            // when the caller explicitly wants smaller-than-network).
            let final_w = (f64::from(width) * scale).round() as u32;
            let final_h = (f64::from(height) * scale).round() as u32;
            let mut down = Vec::with_capacity((final_w as usize) * (final_h as usize) * 4);
            for y in 0..final_h {
                for x in 0..final_w {
                    let sx = ((u64::from(x) * u64::from(out_w)) / u64::from(final_w)) as usize;
                    let sy = ((u64::from(y) * u64::from(out_h)) / u64::from(final_h)) as usize;
                    let idx = (sy * out_w as usize + sx) * 4;
                    down.extend_from_slice(&out[idx..idx + 4]);
                }
            }
            Ok((down, final_w, final_h))
        } else {
            upscale_lanczos(&out, out_w, out_h, target_scale)
        }
    }
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

    #[test]
    fn upscale_with_backend_lanczos3_matches_direct_lanczos() {
        let pixels = solid(8, 8, [50, 100, 150, 255]);
        let direct = upscale_lanczos(&pixels, 8, 8, 2.0).expect("direct upscale");
        let dispatch = upscale_with_backend(&pixels, 8, 8, 2.0, UpscaleBackend::Lanczos3, None)
            .expect("dispatch upscale");
        // Dispatcher must return the same buffer + dimensions as a
        // direct call when the backend is the built-in Lanczos3.
        assert_eq!(direct.0, dispatch.0);
        assert_eq!(direct.1, dispatch.1);
        assert_eq!(direct.2, dispatch.2);
    }

    #[cfg(not(feature = "onnx_upscale"))]
    #[test]
    fn esrgan_backend_unavailable_without_feature() {
        let pixels = solid(4, 4, [255, 0, 0, 255]);
        let err =
            upscale_with_backend(&pixels, 4, 4, 2.0, UpscaleBackend::Esrgan, None).unwrap_err();
        // Without `onnx_upscale`, requesting ESRGAN returns
        // `BackendUnavailable`. The dispatcher must NOT try to load
        // any file (so passing `None` for model_path is fine, and
        // the test passes regardless of filesystem state).
        assert_eq!(
            err,
            UpscaleError::BackendUnavailable(UpscaleBackend::Esrgan)
        );
    }

    #[cfg(feature = "onnx_upscale")]
    #[test]
    fn esrgan_backend_runtime_when_model_missing() {
        let pixels = solid(4, 4, [255, 0, 0, 255]);
        let bogus = std::path::PathBuf::from("/nonexistent/esrgan.onnx");
        let err = upscale_with_backend(&pixels, 4, 4, 4.0, UpscaleBackend::Esrgan, Some(&bogus))
            .unwrap_err();
        // With the feature on but no model file, we must surface a
        // `BackendRuntime` rather than silently fall back — the
        // caller asked for a specific backend.
        assert!(matches!(err, UpscaleError::BackendRuntime { .. }));
    }

    #[cfg(feature = "onnx_upscale")]
    #[test]
    fn esrgan_backend_runtime_when_no_model_path() {
        let pixels = solid(4, 4, [255, 0, 0, 255]);
        let err =
            upscale_with_backend(&pixels, 4, 4, 4.0, UpscaleBackend::Esrgan, None).unwrap_err();
        assert!(matches!(err, UpscaleError::BackendRuntime { .. }));
    }

    // BUG-0001 regression guard: the ESRGAN tile crop math used to
    // unconditionally discard `OVERLAP * NET_SCALE = 32` upscaled
    // pixels on every side of every tile, including image-boundary
    // tiles that had no neighbour to fill those pixels. That left a
    // 32-upscaled-pixel-wide black border on every output and
    // produced an all-black tile for any image dimension < 16 px.
    // The fix routes the crop through `esrgan_tile_crop`, which
    // zeroes the crop on edges that don't have a neighbour.
    #[test]
    fn esrgan_tile_crop_first_tile_has_no_left_or_top_crop() {
        // A 400-wide / 300-tall image has multiple tiles. The first
        // one (tx=0, ty=0) must not crop on the left or top.
        let step = ESRGAN_TILE - 2 * ESRGAN_OVERLAP;
        let (l, t, r, b) = esrgan_tile_crop(0, 0, step, 400, 300);
        assert_eq!(l, 0, "first tile must not crop on the left");
        assert_eq!(t, 0, "first tile must not crop on the top");
        // There IS a tile to the right + bottom, so cropping applies.
        assert_eq!(r, ESRGAN_OVERLAP * ESRGAN_NET_SCALE);
        assert_eq!(b, ESRGAN_OVERLAP * ESRGAN_NET_SCALE);
    }

    #[test]
    fn esrgan_tile_crop_last_tile_has_no_right_or_bottom_crop() {
        // 400×300 with step 112 ⇒ tx ∈ {0, 112, 224, 336}. The last
        // column (336) and last row (224) must not crop on the
        // outside edge — no neighbour to fill it.
        let step = ESRGAN_TILE - 2 * ESRGAN_OVERLAP;
        let (l, t, r, b) = esrgan_tile_crop(336, 224, step, 400, 300);
        assert_eq!(r, 0, "last tile must not crop on the right");
        assert_eq!(b, 0, "last tile must not crop on the bottom");
        // Has tiles to the left and top, so we DO crop those.
        assert_eq!(l, ESRGAN_OVERLAP * ESRGAN_NET_SCALE);
        assert_eq!(t, ESRGAN_OVERLAP * ESRGAN_NET_SCALE);
    }

    #[test]
    fn esrgan_tile_crop_tiny_image_keeps_every_pixel() {
        // A 10×10 image fits in one tile (TILE = 128). With the bug,
        // every side cropped → output entirely black. With the fix,
        // the only tile is both first AND last on every axis, so
        // crop is (0, 0, 0, 0) and every output pixel is written.
        let step = ESRGAN_TILE - 2 * ESRGAN_OVERLAP;
        let (l, t, r, b) = esrgan_tile_crop(0, 0, step, 10, 10);
        assert_eq!((l, t, r, b), (0, 0, 0, 0));
    }

    #[test]
    fn esrgan_tile_crop_middle_tile_crops_all_sides() {
        // A middle tile in a 500×500 image has neighbours on every
        // side and must crop on every side.
        let step = ESRGAN_TILE - 2 * ESRGAN_OVERLAP;
        let crop = ESRGAN_OVERLAP * ESRGAN_NET_SCALE;
        let (l, t, r, b) = esrgan_tile_crop(step, step, step, 500, 500);
        assert_eq!((l, t, r, b), (crop, crop, crop, crop));
    }

    #[test]
    fn esrgan_tile_crop_single_tile_when_image_fits_in_one_step() {
        // For width == step, the tile iteration `(0..step).step_by(step)`
        // only yields tx=0 and `tx + step == width`, so there's no
        // neighbouring tile to the right. The crop on left/right must
        // be zero. Same reasoning for the vertical axis.
        let step = ESRGAN_TILE - 2 * ESRGAN_OVERLAP;
        let (l, t, r, b) = esrgan_tile_crop(0, 0, step, step, step);
        assert_eq!((l, t, r, b), (0, 0, 0, 0));
    }

    #[test]
    fn esrgan_tile_crop_image_slightly_larger_than_step_has_two_tiles() {
        // For width == step + 1, the iteration yields tx=0 and tx=step.
        // The first tile must crop on the right (neighbour at step);
        // the second tile must not crop on the right (no neighbour).
        let step = ESRGAN_TILE - 2 * ESRGAN_OVERLAP;
        let crop = ESRGAN_OVERLAP * ESRGAN_NET_SCALE;
        let width = step + 1;
        let (_, _, r0, _) = esrgan_tile_crop(0, 0, step, width, 1000);
        assert_eq!(r0, crop, "left tile must crop into neighbour");
        let (l1, _, r1, _) = esrgan_tile_crop(step, 0, step, width, 1000);
        assert_eq!(l1, crop, "right tile must crop into neighbour on left");
        assert_eq!(r1, 0, "right tile must not crop on the right");
    }
}
