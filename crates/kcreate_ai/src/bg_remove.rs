//! Background removal — two interchangeable backends.
//!
//! [`Threshold`] is the always-available Phase 0 algorithm: detect the
//! dominant edge colour and knock out every pixel within a tunable
//! distance. Useful for solid-backdrop product photos.
//!
//! [`OnnxU2net`] (Phase 1, opt-in via the `onnx_bg_removal` Cargo
//! feature) runs the u²-net segmentation model through ONNX Runtime
//! and applies the predicted mask to the alpha channel. The runtime
//! is loaded at startup if the `onnx_bg_removal` feature is on; the
//! model file is **not** bundled — callers point [`OnnxU2net`] at a
//! local `.onnx` file. When the file is missing or the runtime can't
//! load it we fall back to [`Threshold`] automatically and surface
//! the reason in [`BgRemoveError::OnnxFallback`].
//!
//! [`Threshold`]: BgRemovalBackend::Threshold
//! [`OnnxU2net`]: BgRemovalBackend::OnnxU2net

use std::path::{Path, PathBuf};

use thiserror::Error;

/// Errors from [`remove_background`].
#[derive(Debug, Error)]
pub enum BgRemoveError {
    #[error(
        "pixel buffer length {got} does not match expected {expected} for {width}x{height} RGBA"
    )]
    InvalidBuffer {
        got: usize,
        expected: usize,
        width: u32,
        height: u32,
    },
    #[error("image too small: {width}x{height}")]
    TooSmall { width: u32, height: u32 },
    /// The ONNX backend was requested but the runtime/model could not
    /// be loaded. The threshold output is still returned; the field
    /// is just for observability.
    #[error("onnx backend unavailable: {reason} (fell back to threshold)")]
    OnnxFallback { reason: String },
}

/// Which segmentation backend to use for [`remove_background_with_backend`].
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub enum BgRemovalBackend {
    /// Phase 0 dominant-edge threshold. Always available, no model
    /// file required. Use this for solid-background product shots.
    #[default]
    Threshold,
    /// Phase 1 u²-net via ONNX Runtime. Set `model_path` to a local
    /// `.onnx` file. Falls back to [`Threshold`] when the runtime or
    /// model are unavailable.
    OnnxU2net {
        /// Path to the `.onnx` model file. Must exist at call time.
        model_path: PathBuf,
    },
}

/// Knobs for the threshold algorithm.
#[derive(Debug, Clone, Copy)]
pub struct BgRemoveOptions {
    /// 0..=255. Pixels within this Euclidean RGB distance of the
    /// edge-dominant colour are knocked out.
    pub tolerance: u8,
    /// 0..=64. Width of the soft-alpha falloff band beyond
    /// `tolerance`. Pixels in this band are linearly faded.
    pub feather: u8,
}

impl Default for BgRemoveOptions {
    fn default() -> Self {
        Self {
            tolerance: 24,
            feather: 16,
        }
    }
}

/// Remove the dominant edge colour. Returns the new RGBA buffer with
/// alpha modulated by distance from the detected background.
pub fn remove_background(
    input_rgba: &[u8],
    width: u32,
    height: u32,
    options: BgRemoveOptions,
) -> Result<Vec<u8>, BgRemoveError> {
    let expected = (width as usize)
        .saturating_mul(height as usize)
        .saturating_mul(4);
    if input_rgba.len() != expected {
        return Err(BgRemoveError::InvalidBuffer {
            got: input_rgba.len(),
            expected,
            width,
            height,
        });
    }
    if width < 2 || height < 2 {
        return Err(BgRemoveError::TooSmall { width, height });
    }

    let (br, bg, bb) = dominant_edge_color(input_rgba, width, height);
    let tol = u32::from(options.tolerance);
    let feather = u32::from(options.feather).max(1);

    let mut out = input_rgba.to_vec();
    for px in out.chunks_exact_mut(4) {
        let dr = i32::from(px[0]) - i32::from(br);
        let dg = i32::from(px[1]) - i32::from(bg);
        let db = i32::from(px[2]) - i32::from(bb);
        let dist = f64::from(dr * dr + dg * dg + db * db).sqrt();
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let dist_u = dist as u32;
        if dist_u <= tol {
            px[3] = 0;
        } else if dist_u <= tol + feather {
            let above = dist_u - tol;
            let alpha = (above * 255 / feather).min(255);
            #[allow(clippy::cast_possible_truncation)]
            let alpha_u8 = alpha as u8;
            // Keep the smaller of (current alpha, new alpha) so we
            // never *increase* opacity. This matters when the input
            // already has alpha < 255 (e.g. pre-masked sprite).
            px[3] = px[3].min(alpha_u8);
        }
    }
    Ok(out)
}

/// Run background removal with the chosen backend.
///
/// The threshold backend always succeeds (subject to input
/// validation). The ONNX backend either succeeds, *or* falls back to
/// threshold and surfaces the reason via the `on_fallback` callback —
/// callers can ignore the callback if they don't care.
///
/// # Errors
///
/// - [`BgRemoveError::InvalidBuffer`] / [`BgRemoveError::TooSmall`]
///   propagate from the underlying threshold pass.
///
/// # Backend selection
///
/// `BgRemovalBackend::OnnxU2net { model_path }` is honoured iff:
///   1. The `onnx_bg_removal` Cargo feature is enabled, AND
///   2. The file at `model_path` exists, AND
///   3. The ONNX Runtime can load the model.
///
/// Otherwise the threshold path runs and `on_fallback` is invoked
/// with a short reason string.
pub fn remove_background_with_backend(
    backend: &BgRemovalBackend,
    input_rgba: &[u8],
    width: u32,
    height: u32,
    options: BgRemoveOptions,
    on_fallback: &mut dyn FnMut(&str),
) -> Result<Vec<u8>, BgRemoveError> {
    match backend {
        BgRemovalBackend::Threshold => remove_background(input_rgba, width, height, options),
        BgRemovalBackend::OnnxU2net { model_path } => {
            match run_onnx_u2net(model_path, input_rgba, width, height) {
                Ok(masked) => Ok(masked),
                Err(reason) => {
                    on_fallback(&reason);
                    remove_background(input_rgba, width, height, options)
                }
            }
        }
    }
}

/// Apply an alpha mask to an RGBA image. Pure helper used by both
/// the threshold and ONNX paths; exposed for tests of the ONNX
/// post-processing step without needing the runtime.
///
/// `mask` is row-major `width * height` bytes, 0 = background,
/// 255 = foreground. Output is `input.len()` bytes with alpha set to
/// `min(input.alpha, mask[i])`.
#[must_use]
pub fn apply_alpha_mask(input_rgba: &[u8], mask: &[u8]) -> Vec<u8> {
    debug_assert_eq!(input_rgba.len(), mask.len() * 4);
    let mut out = input_rgba.to_vec();
    for (i, px) in out.chunks_exact_mut(4).enumerate() {
        let m = mask[i];
        px[3] = px[3].min(m);
    }
    out
}

/// Resample an RGBA image to `target_size × target_size` bytes using
/// nearest-neighbour. Used as the ONNX preprocessing step; we avoid
/// pulling a heavyweight resampling crate because u²-net is robust
/// to the small quality loss vs the cost of bringing in
/// `image::imageops::resize`.
#[must_use]
#[cfg_attr(not(feature = "onnx_bg_removal"), allow(dead_code))]
fn resize_nearest_to_rgb_f32(input_rgba: &[u8], width: u32, height: u32, target: u32) -> Vec<f32> {
    let mut out = Vec::with_capacity((target * target * 3) as usize);
    for ty in 0..target {
        for tx in 0..target {
            let sx = (tx * width / target).min(width - 1);
            let sy = (ty * height / target).min(height - 1);
            let i = ((sy * width + sx) * 4) as usize;
            out.push(f32::from(input_rgba[i]) / 255.0);
            out.push(f32::from(input_rgba[i + 1]) / 255.0);
            out.push(f32::from(input_rgba[i + 2]) / 255.0);
        }
    }
    out
}

/// Upsample a `target × target` mask to `width × height` using
/// nearest-neighbour. Pure function, no allocs beyond the output.
#[must_use]
#[cfg_attr(not(feature = "onnx_bg_removal"), allow(dead_code))]
fn upsample_mask_nearest(mask: &[u8], target: u32, width: u32, height: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((width * height) as usize);
    for y in 0..height {
        for x in 0..width {
            let sx = (x * target / width).min(target - 1);
            let sy = (y * target / height).min(target - 1);
            out.push(mask[(sy * target + sx) as usize]);
        }
    }
    out
}

/// When the `onnx_bg_removal` feature is enabled this calls into
/// ONNX Runtime via `ort`; otherwise it returns an immediate
/// "feature disabled" error so the caller can fall back to
/// threshold. Either way the function is *infallible from the
/// caller's perspective for the threshold path* — see
/// [`remove_background_with_backend`].
#[cfg(feature = "onnx_bg_removal")]
fn run_onnx_u2net(
    model_path: &Path,
    input_rgba: &[u8],
    width: u32,
    height: u32,
) -> Result<Vec<u8>, String> {
    use ort::session::Session;
    use ort::value::TensorRef;

    if !model_path.exists() {
        return Err(format!("model file not found: {}", model_path.display()));
    }
    let session = Session::builder()
        .map_err(|e| format!("ort builder: {e}"))?
        .commit_from_file(model_path)
        .map_err(|e| format!("ort load: {e}"))?;

    // u²-net expects a 320x320 RGB float tensor normalised to [0, 1].
    const TARGET: u32 = 320;
    let chw_data = resize_nearest_to_rgb_f32(input_rgba, width, height, TARGET);
    // ort 2.x wants NCHW (N=1, C=3, H=W=320). The data above is HWC;
    // transpose into a fresh buffer.
    let mut nchw = vec![0f32; (3 * TARGET * TARGET) as usize];
    let plane = (TARGET * TARGET) as usize;
    for y in 0..TARGET {
        for x in 0..TARGET {
            let src = ((y * TARGET + x) * 3) as usize;
            let dst = (y * TARGET + x) as usize;
            nchw[dst] = chw_data[src];
            nchw[plane + dst] = chw_data[src + 1];
            nchw[2 * plane + dst] = chw_data[src + 2];
        }
    }

    let shape = [1_i64, 3, i64::from(TARGET), i64::from(TARGET)];
    let input_tensor = TensorRef::from_array_view((shape.as_slice(), nchw.as_slice()))
        .map_err(|e| format!("ort tensor: {e}"))?;
    let inputs = ort::inputs![input_tensor];
    let outputs = session.run(inputs).map_err(|e| format!("ort run: {e}"))?;

    let (_, raw) = outputs[0]
        .try_extract_tensor::<f32>()
        .map_err(|e| format!("ort extract: {e}"))?;
    // u²-net outputs `d0` first which is the final mask, normalised.
    // Restrict the min/max scan to the `d0` plane: if a future model
    // emits a multi-plane tensor (e.g. all of d0..d6 concatenated),
    // including auxiliary planes in the range would compress the
    // mask's dynamic range and silently degrade output. The mask is
    // built from exactly `&raw[..plane]`, so that's the range we
    // normalise against.
    let mask_plane = &raw[..plane];
    let mut min = f32::INFINITY;
    let mut max = f32::NEG_INFINITY;
    for &v in mask_plane {
        if v < min {
            min = v;
        }
        if v > max {
            max = v;
        }
    }
    let range = (max - min).max(1e-6);
    let mut mask_lo = Vec::with_capacity(plane);
    for &v in mask_plane {
        let normalised = ((v - min) / range).clamp(0.0, 1.0);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let byte = (normalised * 255.0) as u8;
        mask_lo.push(byte);
    }
    let mask = upsample_mask_nearest(&mask_lo, TARGET, width, height);
    Ok(apply_alpha_mask(input_rgba, &mask))
}

#[cfg(not(feature = "onnx_bg_removal"))]
fn run_onnx_u2net(
    _model_path: &Path,
    _input_rgba: &[u8],
    _width: u32,
    _height: u32,
) -> Result<Vec<u8>, String> {
    Err("kcreate_ai built without `onnx_bg_removal` feature".to_string())
}

/// Average colour of the 1-px border ring (top + bottom + left +
/// right). Cheap, deterministic, and matches what photographers
/// expect when shooting on seamless backdrops.
#[must_use]
pub fn dominant_edge_color(rgba: &[u8], width: u32, height: u32) -> (u8, u8, u8) {
    let width_usize = width as usize;
    let height_usize = height as usize;
    let stride = width_usize * 4;
    let mut sum_r: u64 = 0;
    let mut sum_g: u64 = 0;
    let mut sum_b: u64 = 0;
    let mut count: u64 = 0;
    // top + bottom rows
    for x in 0..width_usize {
        for row in [0usize, height_usize - 1] {
            let i = row * stride + x * 4;
            sum_r += u64::from(rgba[i]);
            sum_g += u64::from(rgba[i + 1]);
            sum_b += u64::from(rgba[i + 2]);
            count += 1;
        }
    }
    // left + right columns (excluding corners we already counted)
    for y in 1..height_usize - 1 {
        for col in [0usize, width_usize - 1] {
            let i = y * stride + col * 4;
            sum_r += u64::from(rgba[i]);
            sum_g += u64::from(rgba[i + 1]);
            sum_b += u64::from(rgba[i + 2]);
            count += 1;
        }
    }
    if count == 0 {
        return (0, 0, 0);
    }
    #[allow(clippy::cast_possible_truncation)]
    let avg = |s: u64| (s / count) as u8;
    (avg(sum_r), avg(sum_g), avg(sum_b))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid_with_blob(w: u32, h: u32, bg: [u8; 3], blob: [u8; 3]) -> Vec<u8> {
        let mut v = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                if x > w / 4 && x < 3 * w / 4 && y > h / 4 && y < 3 * h / 4 {
                    v.extend_from_slice(&[blob[0], blob[1], blob[2], 0xFF]);
                } else {
                    v.extend_from_slice(&[bg[0], bg[1], bg[2], 0xFF]);
                }
            }
        }
        v
    }

    #[test]
    fn detects_solid_background() {
        let img = solid_with_blob(20, 20, [10, 20, 30], [200, 200, 200]);
        let (r, g, b) = dominant_edge_color(&img, 20, 20);
        assert!(r < 30 && g < 30 && b < 40);
    }

    #[test]
    fn removes_background_and_keeps_subject() {
        let img = solid_with_blob(20, 20, [240, 240, 240], [40, 40, 40]);
        let out = remove_background(
            &img,
            20,
            20,
            BgRemoveOptions {
                tolerance: 20,
                feather: 8,
            },
        )
        .expect("ok");
        // A corner pixel should be transparent.
        assert_eq!(out[3], 0);
        // A centre pixel should be (close to) opaque.
        let cy = 10usize;
        let cx = 10usize;
        let i = (cy * 20 + cx) * 4;
        assert!(out[i + 3] > 200);
    }

    #[test]
    fn rejects_bad_buffer() {
        let result = remove_background(&[0, 0, 0, 0], 2, 2, BgRemoveOptions::default());
        assert!(result.is_err());
    }

    #[test]
    fn backend_threshold_dispatches_to_remove_background() {
        let img = solid_with_blob(16, 16, [240, 240, 240], [40, 40, 40]);
        let mut fell_back = false;
        let out = remove_background_with_backend(
            &BgRemovalBackend::Threshold,
            &img,
            16,
            16,
            BgRemoveOptions {
                tolerance: 20,
                feather: 8,
            },
            &mut |_| fell_back = true,
        )
        .expect("ok");
        assert!(!fell_back);
        assert_eq!(out[3], 0); // corner transparent
    }

    #[test]
    fn backend_onnx_missing_file_falls_back_to_threshold() {
        let img = solid_with_blob(16, 16, [240, 240, 240], [40, 40, 40]);
        let mut reason: Option<String> = None;
        let out = remove_background_with_backend(
            &BgRemovalBackend::OnnxU2net {
                model_path: PathBuf::from("/no/such/u2net.onnx"),
            },
            &img,
            16,
            16,
            BgRemoveOptions::default(),
            &mut |r| reason = Some(r.to_string()),
        )
        .expect("threshold fallback ok");
        // We get a real masked output regardless.
        assert_eq!(out.len(), img.len());
        // And the callback was invoked with *some* reason — either
        // "feature disabled" (default build) or "model file not found"
        // (onnx_bg_removal enabled).
        assert!(reason.is_some(), "expected fallback reason");
    }

    #[test]
    fn apply_alpha_mask_clamps_to_min() {
        // 2x1 image, first pixel already half-transparent.
        let img = vec![10, 20, 30, 128, 40, 50, 60, 255];
        let mask = vec![200, 50];
        let out = apply_alpha_mask(&img, &mask);
        // px0: min(128, 200) = 128
        assert_eq!(out[3], 128);
        // px1: min(255, 50) = 50
        assert_eq!(out[7], 50);
    }

    #[test]
    fn resize_nearest_round_trip_dimensions() {
        let img = solid_with_blob(8, 8, [10, 20, 30], [240, 240, 240]);
        let v = resize_nearest_to_rgb_f32(&img, 8, 8, 4);
        assert_eq!(v.len(), 4 * 4 * 3);
        // Every component is in [0, 1].
        assert!(v.iter().all(|f| (0.0..=1.0).contains(f)));
    }

    #[test]
    fn upsample_nearest_round_trip_dimensions() {
        let mask = vec![0, 255, 255, 0];
        let up = upsample_mask_nearest(&mask, 2, 4, 4);
        assert_eq!(up.len(), 16);
    }

    /// Integration test that exercises the real ONNX runtime. Marked
    /// `#[ignore]` because the model file is not bundled — run with
    /// `cargo test --features onnx_bg_removal -- --ignored
    /// onnx_bg_removal_with_real_model` after exporting
    /// `KCREATE_TEST_U2NET_PATH=/path/to/u2net.onnx`.
    #[test]
    #[ignore = "requires KCREATE_TEST_U2NET_PATH and `onnx_bg_removal` feature"]
    #[cfg(feature = "onnx_bg_removal")]
    fn onnx_bg_removal_with_real_model() {
        let Some(p) = std::env::var_os("KCREATE_TEST_U2NET_PATH") else {
            return;
        };
        let img = solid_with_blob(32, 32, [255, 255, 255], [10, 10, 10]);
        let mut fb_reason: Option<String> = None;
        let out = remove_background_with_backend(
            &BgRemovalBackend::OnnxU2net {
                model_path: PathBuf::from(p),
            },
            &img,
            32,
            32,
            BgRemoveOptions::default(),
            &mut |r| fb_reason = Some(r.to_string()),
        )
        .expect("ok");
        assert_eq!(out.len(), img.len());
        // If the runtime is available the fallback should not fire.
        assert!(fb_reason.is_none(), "unexpected fallback: {fb_reason:?}");
    }
}
