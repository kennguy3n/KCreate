//! Threshold-based background removal.
//!
//! Detect the dominant edge colour of an RGBA image, then mark every
//! pixel within a tunable LAB-ish distance as transparent. This is a
//! genuinely useful effect for solid-background product photos and
//! demonstrates the full pipeline without an ML model download.
//!
//! Phase 1 will swap [`remove_background`] for an ONNX `u2net` runner
//! while keeping the same in/out signature.

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
}
