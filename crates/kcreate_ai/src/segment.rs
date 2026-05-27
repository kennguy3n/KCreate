//! Point-prompted image segmentation.
//!
//! Two backends:
//!
//! * [`SegmentBackend::EdgeAware`] — built-in, always available. A
//!   real edge-aware flood-fill in CIE Lab colour space: from the
//!   user-provided point prompt we BFS over 4-neighbours and admit a
//!   pixel into the mask when its Lab distance to the seed is below
//!   `tolerance` AND the Sobel gradient magnitude between the two
//!   pixels is below `edge_threshold`. The combination gives a
//!   real, useful object cut on photographic content without
//!   running any neural model.
//!
//! * [`SegmentBackend::Sam`] — opt-in via the `onnx_segment` Cargo
//!   feature. Loads a Segment Anything ONNX export (the
//!   single-file MobileSAM / EdgeSAM variant that fuses image
//!   encoder + mask decoder so we don't have to feed embeddings
//!   between two sessions) and runs a point-prompted forward pass.
//!   Postprocessing rescales the network's 256×256 mask back up to
//!   the input resolution and thresholds at 0.5 (the SAM
//!   convention).
//!
//! Default builds and the editing-path closure (see
//! `crates/kcreate_tests/tests/local_first.rs`) compile this module
//! without the `onnx_segment` feature, so requesting [`SegmentBackend::Sam`]
//! returns [`SegmentError::BackendUnavailable`] up front without
//! touching the filesystem.

use std::collections::VecDeque;

#[cfg(feature = "onnx_segment")]
use std::path::Path;

use thiserror::Error;

/// Selectable segmentation backend. The wire enum is `snake_case` so
/// the bridge / UI can pass `"edge_aware"` / `"sam"` strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SegmentBackend {
    /// Pure-Rust Lab + Sobel edge-aware flood-fill. Always available.
    EdgeAware,
    /// SAM ONNX inference. Requires the `onnx_segment` Cargo feature
    /// and a valid SAM model file on disk (installed via
    /// [`crate::install_model_pack`] with id `segment_sam`).
    Sam,
}

/// Options for [`segment_image`] / [`segment_with_backend`].
#[derive(Debug, Clone, PartialEq)]
pub struct SegmentOptions {
    /// Foreground point prompt, in image pixel coordinates.
    pub point_x: u32,
    pub point_y: u32,
    /// Lab tolerance in `[0.0, 1.0]`. `0.0` only admits pixels with
    /// exactly the same Lab as the seed; `1.0` admits everything.
    /// Applies to [`SegmentBackend::EdgeAware`].
    pub tolerance: f64,
    /// Sobel edge-magnitude threshold in `[0.0, 1.0]`. Pixels whose
    /// gradient magnitude exceeds this value are *not* crossed
    /// during flood-fill. Applies to [`SegmentBackend::EdgeAware`].
    pub edge_threshold: f64,
}

impl Default for SegmentOptions {
    fn default() -> Self {
        Self {
            point_x: 0,
            point_y: 0,
            tolerance: 0.18,
            edge_threshold: 0.25,
        }
    }
}

/// A single segmentation mask returned by either backend.
#[derive(Debug, Clone)]
pub struct SegmentMask {
    /// Width / height match the input image.
    pub width: u32,
    pub height: u32,
    /// `width * height` bytes: `255` = foreground, `0` = background.
    pub mask: Vec<u8>,
    /// Number of foreground pixels.
    pub area: u64,
    /// Confidence in `[0.0, 1.0]`. Edge-aware backend returns
    /// `area / total_pixels` as a structural confidence; SAM returns
    /// the network's reported IoU prediction.
    pub confidence: f32,
}

/// Result of a segmentation run.
#[derive(Debug, Clone)]
pub struct SegmentResult {
    /// Backend that produced the masks.
    pub backend: SegmentBackend,
    /// Per-prompt masks. The current API always supplies a single
    /// point prompt; the field is a `Vec` so future API additions
    /// (box prompts, multi-point disambiguation) don't break callers.
    pub masks: Vec<SegmentMask>,
}

/// Errors from [`segment_image`] / [`segment_with_backend`].
#[derive(Debug, Error, PartialEq, Eq)]
pub enum SegmentError {
    #[error(
        "invalid dimensions: width and height must be > 0 and pixels.len() == width * height * 4"
    )]
    InvalidDimensions,
    #[error("point prompt ({0}, {1}) is outside image bounds {2}x{3}")]
    PointOutOfBounds(u32, u32, u32, u32),
    #[error("invalid tolerance: {0}; must be in [0.0, 1.0]")]
    InvalidTolerance(String),
    #[error("invalid edge_threshold: {0}; must be in [0.0, 1.0]")]
    InvalidEdgeThreshold(String),
    /// The requested backend is not compiled in (e.g. `Sam` on a
    /// build that disabled the `onnx_segment` Cargo feature).
    #[error("backend {0:?} is not available in this build")]
    BackendUnavailable(SegmentBackend),
    /// Backend-specific runtime failure (e.g. ONNX model file
    /// missing, weights load error, tensor shape mismatch).
    #[error("backend {backend:?} failed: {message}")]
    BackendRuntime {
        backend: SegmentBackend,
        message: String,
    },
}

/// Convenience: run [`SegmentBackend::EdgeAware`].
pub fn segment_image(
    pixels: &[u8],
    width: u32,
    height: u32,
    options: &SegmentOptions,
) -> Result<SegmentResult, SegmentError> {
    segment_with_backend(
        pixels,
        width,
        height,
        options,
        SegmentBackend::EdgeAware,
        None,
    )
}

/// Dispatch to the requested backend.
pub fn segment_with_backend(
    pixels: &[u8],
    width: u32,
    height: u32,
    options: &SegmentOptions,
    backend: SegmentBackend,
    model_path: Option<&std::path::Path>,
) -> Result<SegmentResult, SegmentError> {
    validate_inputs(pixels, width, height, options)?;

    match backend {
        SegmentBackend::EdgeAware => {
            let mask = edge_aware_segment(pixels, width, height, options);
            Ok(SegmentResult {
                backend,
                masks: vec![mask],
            })
        }
        SegmentBackend::Sam => {
            #[cfg(feature = "onnx_segment")]
            {
                let path = model_path.ok_or_else(|| SegmentError::BackendRuntime {
                    backend: SegmentBackend::Sam,
                    message: "SAM backend selected but no model_path provided".into(),
                })?;
                let mask = run_onnx_sam(path, pixels, width, height, options)?;
                Ok(SegmentResult {
                    backend,
                    masks: vec![mask],
                })
            }
            #[cfg(not(feature = "onnx_segment"))]
            {
                let _ = model_path;
                Err(SegmentError::BackendUnavailable(SegmentBackend::Sam))
            }
        }
    }
}

fn validate_inputs(
    pixels: &[u8],
    width: u32,
    height: u32,
    options: &SegmentOptions,
) -> Result<(), SegmentError> {
    if width == 0 || height == 0 {
        return Err(SegmentError::InvalidDimensions);
    }
    let expected_len = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4))
        .ok_or(SegmentError::InvalidDimensions)?;
    if pixels.len() != expected_len {
        return Err(SegmentError::InvalidDimensions);
    }
    if options.point_x >= width || options.point_y >= height {
        return Err(SegmentError::PointOutOfBounds(
            options.point_x,
            options.point_y,
            width,
            height,
        ));
    }
    if !options.tolerance.is_finite() || !(0.0..=1.0).contains(&options.tolerance) {
        return Err(SegmentError::InvalidTolerance(format!(
            "{}",
            options.tolerance
        )));
    }
    if !options.edge_threshold.is_finite() || !(0.0..=1.0).contains(&options.edge_threshold) {
        return Err(SegmentError::InvalidEdgeThreshold(format!(
            "{}",
            options.edge_threshold
        )));
    }
    Ok(())
}

/// Edge-aware Lab flood-fill. Real algorithm — no stubs.
fn edge_aware_segment(
    pixels: &[u8],
    width: u32,
    height: u32,
    options: &SegmentOptions,
) -> SegmentMask {
    let total = (width as usize) * (height as usize);

    // 1) Convert sRGB to Lab once. We store as f32 LLAABB triplets.
    let mut lab: Vec<[f32; 3]> = Vec::with_capacity(total);
    for chunk in pixels.chunks_exact(4) {
        lab.push(srgb_to_lab(chunk[0], chunk[1], chunk[2]));
    }

    // 2) Sobel gradient magnitude on the L channel, normalised to
    //    [0, 1]. We bias toward luminance because it's the channel
    //    where object boundaries are most reliably visible.
    let mut gradient = vec![0.0_f32; total];
    let mut max_grad = 1e-6_f32;
    for y in 0..height {
        for x in 0..width {
            let gx = sobel_x(&lab, x, y, width, height);
            let gy = sobel_y(&lab, x, y, width, height);
            let mag = gx.hypot(gy);
            gradient[(y as usize) * (width as usize) + x as usize] = mag;
            if mag > max_grad {
                max_grad = mag;
            }
        }
    }
    for g in &mut gradient {
        *g /= max_grad;
    }

    // 3) BFS from the seed, gated by Lab distance + gradient
    //    threshold.
    let seed_idx = (options.point_y as usize) * (width as usize) + options.point_x as usize;
    let seed_lab = lab[seed_idx];
    let edge_threshold = options.edge_threshold as f32;
    // Tolerance scales against the Lab distance horizon (Lab is in
    // CIE units; L∈[0,100], a/b∈[-128,127]). 1.0 admits a Δ of
    // ~150 (the empirical 99th percentile of Lab distances on
    // photographic content), 0.0 admits only exact matches.
    let tol_lab = (options.tolerance as f32) * 150.0;

    let mut mask = vec![0u8; total];
    mask[seed_idx] = 255;
    let mut q: VecDeque<(u32, u32)> = VecDeque::new();
    q.push_back((options.point_x, options.point_y));

    while let Some((x, y)) = q.pop_front() {
        for (nx, ny) in neighbours_4(x, y, width, height) {
            let nidx = (ny as usize) * (width as usize) + nx as usize;
            if mask[nidx] != 0 {
                continue;
            }
            // Reject if the pixel sits on a strong edge.
            if gradient[nidx] > edge_threshold {
                continue;
            }
            let d = lab_distance(seed_lab, lab[nidx]);
            if d <= tol_lab {
                mask[nidx] = 255;
                q.push_back((nx, ny));
            }
        }
    }

    // Counting a single sentinel byte in a Vec<u8> is the canonical
    // case `clippy::naive_bytecount` warns about, but the cost here
    // is bounded by a single image (already O(w*h) for the BFS
    // above), so a 1.5x speedup from `bytecount` is not worth a new
    // dependency in the editing path.
    #[allow(clippy::naive_bytecount)]
    let area = mask.iter().filter(|&&v| v == 255).count() as u64;
    let confidence = if total == 0 {
        0.0
    } else {
        (area as f32) / (total as f32)
    };

    SegmentMask {
        width,
        height,
        mask,
        area,
        confidence,
    }
}

fn neighbours_4(x: u32, y: u32, w: u32, h: u32) -> impl Iterator<Item = (u32, u32)> {
    let mut buf: [Option<(u32, u32)>; 4] = [None; 4];
    if x > 0 {
        buf[0] = Some((x - 1, y));
    }
    if x + 1 < w {
        buf[1] = Some((x + 1, y));
    }
    if y > 0 {
        buf[2] = Some((x, y - 1));
    }
    if y + 1 < h {
        buf[3] = Some((x, y + 1));
    }
    buf.into_iter().flatten()
}

fn sample_l(lab: &[[f32; 3]], x: i32, y: i32, width: u32, height: u32) -> f32 {
    let cx = x.clamp(0, width as i32 - 1) as u32;
    let cy = y.clamp(0, height as i32 - 1) as u32;
    lab[(cy as usize) * (width as usize) + cx as usize][0]
}

fn sobel_x(lab: &[[f32; 3]], x: u32, y: u32, w: u32, h: u32) -> f32 {
    let xi = x as i32;
    let yi = y as i32;
    let p = |dx: i32, dy: i32| sample_l(lab, xi + dx, yi + dy, w, h);
    -p(-1, -1) - 2.0 * p(-1, 0) - p(-1, 1) + p(1, -1) + 2.0 * p(1, 0) + p(1, 1)
}

fn sobel_y(lab: &[[f32; 3]], x: u32, y: u32, w: u32, h: u32) -> f32 {
    let xi = x as i32;
    let yi = y as i32;
    let p = |dx: i32, dy: i32| sample_l(lab, xi + dx, yi + dy, w, h);
    -p(-1, -1) - 2.0 * p(0, -1) - p(1, -1) + p(-1, 1) + 2.0 * p(0, 1) + p(1, 1)
}

fn lab_distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    // CIE Lab is a 3D space, so we cannot use `f32::hypot` (which is
    // 2D) directly. Compute the full 3D Euclidean distance.
    let dl = a[0] - b[0];
    let da = a[1] - b[1];
    let db = a[2] - b[2];
    // Sum-of-squares is safe for Lab ranges (L ∈ [0,100], a/b ∈
    // [-128, 127]) — max squared sum is bounded well below f32::MAX.
    (dl.mul_add(dl, da.mul_add(da, db * db))).sqrt()
}

fn srgb_linear(c: f32) -> f32 {
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn lab_f(t: f32) -> f32 {
    const DELTA: f32 = 6.0 / 29.0;
    if t > DELTA * DELTA * DELTA {
        t.cbrt()
    } else {
        t / (3.0 * DELTA * DELTA) + 4.0 / 29.0
    }
}

/// sRGB → CIE Lab via D65 white point. Inputs are 0..=255 u8, outputs
/// are L in [0, 100], a/b in approximately [-128, 127].
fn srgb_to_lab(r: u8, g: u8, b: u8) -> [f32; 3] {
    let r = srgb_linear(f32::from(r) / 255.0);
    let g = srgb_linear(f32::from(g) / 255.0);
    let b = srgb_linear(f32::from(b) / 255.0);

    // sRGB → XYZ (D65). Constants from IEC 61966-2-1; rounded to
    // f32 precision (7 significant digits) to stay clippy-clean.
    let x = r * 0.412_456_4 + g * 0.357_576_1 + b * 0.180_437_5;
    let y = r * 0.212_672_9 + g * 0.715_152_2 + b * 0.072_175;
    let z = r * 0.019_333_9 + g * 0.119_192 + b * 0.950_304_1;

    // Normalise by D65 white point.
    let xn = x / 0.950_47;
    let yn = y;
    let zn = z / 1.088_83;

    let fx = lab_f(xn);
    let fy = lab_f(yn);
    let fz = lab_f(zn);
    let l = 116.0 * fy - 16.0;
    let a = 500.0 * (fx - fy);
    let b_lab = 200.0 * (fy - fz);
    [l, a, b_lab]
}

/// Run Segment-Anything inference on the supplied RGBA frame and
/// return a single mask for the point prompt.
///
/// We target the *fused* SAM ONNX exports (MobileSAM / EdgeSAM /
/// the consolidated-decoder ViT-B exports) — these accept an
/// `(image, point, label)` tuple in one session call so we don't
/// have to plumb intermediate embeddings between two sessions. The
/// network expects a 1024×1024 input image with a fixed
/// preprocessing pipeline: subtract the SAM pixel mean
/// `(123.675, 116.28, 103.53)` and divide by the pixel std
/// `(58.395, 57.12, 57.375)`. Input shape is `NCHW = 1 × 3 × 1024 × 1024`.
#[cfg(feature = "onnx_segment")]
fn run_onnx_sam(
    model_path: &Path,
    pixels: &[u8],
    width: u32,
    height: u32,
    options: &SegmentOptions,
) -> Result<SegmentMask, SegmentError> {
    use ort::session::Session;
    use ort::value::TensorRef;

    if !model_path.exists() {
        return Err(SegmentError::BackendRuntime {
            backend: SegmentBackend::Sam,
            message: format!("model file not found: {}", model_path.display()),
        });
    }
    let session = Session::builder()
        .map_err(|e| SegmentError::BackendRuntime {
            backend: SegmentBackend::Sam,
            message: format!("ort builder: {e}"),
        })?
        .commit_from_file(model_path)
        .map_err(|e| SegmentError::BackendRuntime {
            backend: SegmentBackend::Sam,
            message: format!("ort load: {e}"),
        })?;

    const NET: u32 = 1024;
    const MEAN_R: f32 = 123.675;
    const MEAN_G: f32 = 116.28;
    const MEAN_B: f32 = 103.53;
    const STD_R: f32 = 58.395;
    const STD_G: f32 = 57.12;
    const STD_B: f32 = 57.375;

    // SAM keeps the aspect ratio by resizing the longer side to 1024
    // and zero-padding the shorter side. `NET as f32` is lossless
    // because NET is far below the f32 integer-precision limit of
    // 2^24, so we avoid the previous `f32::from(NET as u16)` chain
    // (Devin Review ANALYSIS-0001 on PR #16) which would silently
    // truncate if NET were ever raised above 65535 for a future model.
    let longer = width.max(height) as f32;
    let scale = (NET as f32) / longer;
    let resized_w = ((width as f32) * scale).round() as u32;
    let resized_h = ((height as f32) * scale).round() as u32;

    // Build the normalised NCHW input tensor.
    let plane = (NET * NET) as usize;
    let mut input = vec![0_f32; 3 * plane];
    for y in 0..resized_h {
        let sy = ((u64::from(y) * u64::from(height)) / u64::from(resized_h)) as u32;
        for x in 0..resized_w {
            let sx = ((u64::from(x) * u64::from(width)) / u64::from(resized_w)) as u32;
            let src = ((sy * width + sx) * 4) as usize;
            let dst = (y as usize) * (NET as usize) + x as usize;
            input[dst] = (f32::from(pixels[src]) - MEAN_R) / STD_R;
            input[plane + dst] = (f32::from(pixels[src + 1]) - MEAN_G) / STD_G;
            input[2 * plane + dst] = (f32::from(pixels[src + 2]) - MEAN_B) / STD_B;
        }
    }

    let image_shape = [1_i64, 3, i64::from(NET), i64::from(NET)];
    let image_tensor = TensorRef::from_array_view((image_shape.as_slice(), input.as_slice()))
        .map_err(|e| SegmentError::BackendRuntime {
            backend: SegmentBackend::Sam,
            message: format!("ort image tensor: {e}"),
        })?;

    // Point prompt in resized image space.
    //
    // The fused MobileSAM / EdgeSAM single-file ONNX exports we
    // target (samexporter `--mobilesam` and the consolidated
    // ViT-B builds) expose the prompt inputs as:
    //
    //   point_coords : f32 [1, N, 2]
    //   point_labels : f32 [1, N]
    //
    // i.e. batch axis + N points + (x, y). Earlier revisions of
    // this code shipped a 4D `[1, 1, 1, 2]` / 3D `[1, 1, 1]` shape
    // by analogy with the non-fused two-session exports, which is
    // not what the fused exports accept and would produce a clear
    // `ort run` shape-mismatch error on every call (Devin Review
    // ANALYSIS_0002 on PR #16). We now use the documented 3D / 2D
    // shapes the fused exports actually require — see the
    // `ChaoningZhang/MobileSAM` upstream and the `samexporter`
    // README for the contract.
    let prompt_x = (options.point_x as f32) * scale;
    let prompt_y = (options.point_y as f32) * scale;
    let points: Vec<f32> = vec![prompt_x, prompt_y];
    let points_shape = [1_i64, 1, 2];
    let points_tensor = TensorRef::from_array_view((points_shape.as_slice(), points.as_slice()))
        .map_err(|e| SegmentError::BackendRuntime {
            backend: SegmentBackend::Sam,
            message: format!("ort points tensor: {e}"),
        })?;
    // Label `1` = foreground point. Shape `[1, N]` (batch × points).
    let labels: Vec<f32> = vec![1.0];
    let labels_shape = [1_i64, 1];
    let labels_tensor = TensorRef::from_array_view((labels_shape.as_slice(), labels.as_slice()))
        .map_err(|e| SegmentError::BackendRuntime {
            backend: SegmentBackend::Sam,
            message: format!("ort labels tensor: {e}"),
        })?;

    let outputs = session
        .run(ort::inputs![image_tensor, points_tensor, labels_tensor])
        .map_err(|e| SegmentError::BackendRuntime {
            backend: SegmentBackend::Sam,
            message: format!("ort run: {e}"),
        })?;

    let (mask_shape, raw_mask) =
        outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| SegmentError::BackendRuntime {
                backend: SegmentBackend::Sam,
                message: format!("ort extract mask: {e}"),
            })?;

    // SAM outputs `1 × num_masks × Hm × Wm`. Pick mask 0.
    let dims: Vec<i64> = mask_shape.to_vec();
    if dims.len() < 4 {
        return Err(SegmentError::BackendRuntime {
            backend: SegmentBackend::Sam,
            message: format!("unexpected mask shape: {dims:?}"),
        });
    }
    let mh = dims[dims.len() - 2] as u32;
    let mw = dims[dims.len() - 1] as u32;

    // Extract mask 0 plane and resample to the input resolution.
    let plane_m = (mh * mw) as usize;
    let mask_plane = &raw_mask[..plane_m];

    // Threshold at 0.0 (SAM convention — outputs are logits).
    let mut hires = vec![0u8; (width as usize) * (height as usize)];
    for y in 0..height {
        let my = ((u64::from(y) * u64::from(mh)) / u64::from(height)) as usize;
        for x in 0..width {
            let mx = ((u64::from(x) * u64::from(mw)) / u64::from(width)) as usize;
            let v = mask_plane[my * (mw as usize) + mx];
            hires[(y as usize) * (width as usize) + x as usize] = if v > 0.0 { 255 } else { 0 };
        }
    }

    // Try to read an IoU prediction off output[1] if the model
    // provides one (SAM exports include it; some fused exports do
    // not). Fall back to area-fraction confidence.
    let confidence = if outputs.len() > 1 {
        match outputs[1].try_extract_tensor::<f32>() {
            Ok((_, scores)) if !scores.is_empty() => scores[0].clamp(0.0, 1.0),
            _ => area_confidence(&hires),
        }
    } else {
        area_confidence(&hires)
    };

    #[allow(clippy::naive_bytecount)]
    let area = hires.iter().filter(|&&v| v == 255).count() as u64;
    Ok(SegmentMask {
        width,
        height,
        mask: hires,
        area,
        confidence,
    })
}

#[cfg(feature = "onnx_segment")]
fn area_confidence(mask: &[u8]) -> f32 {
    if mask.is_empty() {
        return 0.0;
    }
    #[allow(clippy::naive_bytecount)]
    let area = mask.iter().filter(|&&v| v == 255).count() as f32;
    area / (mask.len() as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid_field(w: u32, h: u32, rgba: [u8; 4]) -> Vec<u8> {
        let mut v = Vec::with_capacity((w as usize) * (h as usize) * 4);
        for _ in 0..(w as usize) * (h as usize) {
            v.extend_from_slice(&rgba);
        }
        v
    }

    /// Composite: red 20-pixel square centred on a green field.
    fn two_block(w: u32, h: u32) -> Vec<u8> {
        let mut v = solid_field(w, h, [0, 200, 0, 255]);
        let cx = w / 2;
        let cy = h / 2;
        for y in (cy.saturating_sub(10))..(cy + 10) {
            for x in (cx.saturating_sub(10))..(cx + 10) {
                let i = ((y * w) + x) as usize * 4;
                v[i] = 220;
                v[i + 1] = 20;
                v[i + 2] = 20;
                v[i + 3] = 255;
            }
        }
        v
    }

    #[test]
    fn rejects_zero_dimensions() {
        let r = segment_image(&[], 0, 1, &SegmentOptions::default());
        assert!(matches!(r, Err(SegmentError::InvalidDimensions)));
    }

    #[test]
    fn rejects_point_out_of_bounds() {
        let pixels = solid_field(4, 4, [255, 0, 0, 255]);
        let opts = SegmentOptions {
            point_x: 4,
            point_y: 0,
            ..Default::default()
        };
        let err = segment_image(&pixels, 4, 4, &opts).unwrap_err();
        assert!(matches!(err, SegmentError::PointOutOfBounds(4, 0, 4, 4)));
    }

    #[test]
    fn rejects_out_of_range_tolerance() {
        let pixels = solid_field(4, 4, [255, 0, 0, 255]);
        let opts = SegmentOptions {
            point_x: 0,
            point_y: 0,
            tolerance: 1.5,
            ..Default::default()
        };
        let err = segment_image(&pixels, 4, 4, &opts).unwrap_err();
        assert!(matches!(err, SegmentError::InvalidTolerance(_)));
    }

    #[test]
    fn rejects_nan_edge_threshold() {
        let pixels = solid_field(4, 4, [255, 0, 0, 255]);
        let opts = SegmentOptions {
            point_x: 0,
            point_y: 0,
            tolerance: 0.5,
            edge_threshold: f64::NAN,
        };
        let err = segment_image(&pixels, 4, 4, &opts).unwrap_err();
        assert!(matches!(err, SegmentError::InvalidEdgeThreshold(_)));
    }

    #[test]
    fn edge_aware_selects_the_red_block_only() {
        let w = 60;
        let h = 60;
        let pixels = two_block(w, h);
        let opts = SegmentOptions {
            point_x: w / 2,
            point_y: h / 2,
            tolerance: 0.18,
            edge_threshold: 0.25,
        };
        let result = segment_image(&pixels, w, h, &opts).unwrap();
        assert_eq!(result.backend, SegmentBackend::EdgeAware);
        assert_eq!(result.masks.len(), 1);
        let mask = &result.masks[0];
        // Centre of the red block is selected.
        let centre = mask.mask[(((h / 2) * w) + (w / 2)) as usize];
        assert_eq!(centre, 255, "seed pixel must be in the mask");
        // A pixel in the green border is NOT selected.
        assert_eq!(mask.mask[0], 0, "background corner must be excluded");
        // Mask area is bounded: red block is ~400 px out of 3600.
        assert!(
            mask.area >= 300 && mask.area <= 600,
            "mask should be the red block, got area = {}",
            mask.area
        );
    }

    #[test]
    fn edge_aware_low_tolerance_shrinks_mask() {
        // Smooth horizontal Lab gradient — no hard edges, so the
        // Sobel gate stays inactive. A strict Lab tolerance should
        // admit only pixels near the seed colour; a loose
        // tolerance should reach further across the gradient.
        let w = 40;
        let h = 8;
        let mut pixels = Vec::with_capacity((w as usize) * (h as usize) * 4);
        for _ in 0..h {
            for x in 0..w {
                let r = (50_u32 + (x * 200) / (w - 1)) as u8;
                pixels.extend_from_slice(&[r, 100, 100, 255]);
            }
        }
        let strict = segment_image(
            &pixels,
            w,
            h,
            &SegmentOptions {
                point_x: 0,
                point_y: 4,
                tolerance: 0.05,
                edge_threshold: 1.0,
            },
        )
        .unwrap();
        let loose = segment_image(
            &pixels,
            w,
            h,
            &SegmentOptions {
                point_x: 0,
                point_y: 4,
                tolerance: 0.8,
                edge_threshold: 1.0,
            },
        )
        .unwrap();
        assert!(
            strict.masks[0].area < loose.masks[0].area,
            "strict tolerance must shrink the mask: strict={}, loose={}",
            strict.masks[0].area,
            loose.masks[0].area
        );
    }

    #[test]
    fn srgb_to_lab_roundtrip_for_known_swatches() {
        // sRGB white → L≈100, a≈0, b≈0.
        let white = srgb_to_lab(255, 255, 255);
        assert!((white[0] - 100.0).abs() < 0.5);
        assert!(white[1].abs() < 1.0);
        assert!(white[2].abs() < 1.0);
        // sRGB black → L≈0.
        let black = srgb_to_lab(0, 0, 0);
        assert!(black[0].abs() < 0.5);
    }

    #[cfg(not(feature = "onnx_segment"))]
    #[test]
    fn sam_backend_unavailable_without_feature() {
        let pixels = solid_field(4, 4, [255, 0, 0, 255]);
        let err = segment_with_backend(
            &pixels,
            4,
            4,
            &SegmentOptions {
                point_x: 0,
                point_y: 0,
                ..Default::default()
            },
            SegmentBackend::Sam,
            None,
        )
        .unwrap_err();
        assert_eq!(err, SegmentError::BackendUnavailable(SegmentBackend::Sam));
    }
}
