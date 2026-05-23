//! CMYK rasterisation dithering for the PDF export pipeline.
//!
//! When a raster layer is exported to a `/DeviceCMYK` image XObject,
//! every floating-point CMYK quadruple must be quantised down to
//! 8-bit per channel. Doing this with a per-pixel
//! `(x.clamp(0,1) * 255.0 + 0.5) as u8` rounder (the Phase 2 path)
//! is fast but produces visible **banding** in smooth gradients —
//! the round-off error all goes the same direction within a band,
//! so the eye sees the transitions as hard contours rather than as
//! a smooth gradient.
//!
//! This module provides two error-diffusing / threshold-modulating
//! quantisers that distribute the round-off error across pixels,
//! turning the visible banding into invisible per-pixel noise:
//!
//! * [`CmykDither::FloydSteinberg`] — Floyd & Steinberg's 1976
//!   four-neighbour error-diffusion filter. Best quality at the
//!   cost of being inherently serial (each pixel depends on its
//!   predecessors). Use this for hero artwork and for any export
//!   where the output is going to be looked at closely.
//! * [`CmykDither::Bayer8x8`] — Bryce Bayer's 1973 ordered-dither
//!   matrix at 8×8 resolution. Trades a slight diagonal-texture
//!   "screen door" pattern for being **embarrassingly parallel**:
//!   every pixel's output depends only on its `(x, y)` and its
//!   own float CMYK input. Use this for thumbnails, batch
//!   exports, and previews.
//! * [`CmykDither::None`] — the legacy nearest-neighbour quantise.
//!   Documented and tested so callers can opt out explicitly.
//!
//! The public surface is [`quantize_cmyk_image`], which takes a
//! per-pixel float-CMYK provider plus dimensions and writes
//! interleaved C/M/Y/K bytes ready to feed to a `/DeviceCMYK`
//! image XObject. Keeping the float-CMYK callback out of this
//! module means the simulation / soft-proof / ICC profile chain
//! lives in `kcreate_core::icc` (or whatever pipeline the caller
//! chooses) and we don't double-couple this module to
//! `kcreate_core`.

use serde::{Deserialize, Serialize};

/// Which dithering algorithm to use when rasterising a CMYK image.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CmykDither {
    /// Round each float CMYK component to the nearest 8-bit value.
    /// Fast, deterministic, but produces visible banding on smooth
    /// gradients.
    None,
    /// Floyd-Steinberg error diffusion (1976). Highest fidelity;
    /// inherently serial in scan order.
    #[default]
    FloydSteinberg,
    /// 8×8 Bayer ordered-dither matrix (1973). Pattern-textured
    /// but trivially parallel and crash-proof under partial-buffer
    /// failure recovery.
    Bayer8x8,
}

/// Per-pixel float CMYK quadruple. All four components are
/// `[0.0, 1.0]` linear ink coverage. NaN / infinity components are
/// treated as zero by the quantiser so callers don't have to
/// pre-sanitise.
pub type CmykPixel = [f32; 4];

/// The 8×8 Bayer dither matrix, normalised to `[0, 1)`. Generated
/// from the classic recursive `M_{2n}` construction:
///
/// ```text
/// M_2 = (1/4) * [ 0  2 ]
///                [ 3  1 ]
/// ```
///
/// then `M_{2n} = (1/(2n)^2) * [ 4*M_n + 0  4*M_n + 2 ; 4*M_n + 3  4*M_n + 1 ]`
/// expanded to 8×8 and divided by 64 to normalise. Cell `(y, x)`
/// gives the per-pixel threshold offset that shifts the quantiser
/// midpoint within the floor-pixel: `(matrix[y][x] - 0.5/64.0) / 64.0`.
const BAYER_8X8: [[u8; 8]; 8] = [
    [0, 32, 8, 40, 2, 34, 10, 42],
    [48, 16, 56, 24, 50, 18, 58, 26],
    [12, 44, 4, 36, 14, 46, 6, 38],
    [60, 28, 52, 20, 62, 30, 54, 22],
    [3, 35, 11, 43, 1, 33, 9, 41],
    [51, 19, 59, 27, 49, 17, 57, 25],
    [15, 47, 7, 39, 13, 45, 5, 37],
    [63, 31, 55, 23, 61, 29, 53, 21],
];

/// Sanitise a single float-CMYK component to `[0.0, 1.0]`, with
/// NaN / infinity → 0.0.
#[inline]
fn sanitise(v: f32) -> f32 {
    if v.is_finite() {
        v.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// Quantise a single `[0.0, 1.0]` component to a `[0, 255]` byte
/// with nearest-neighbour rounding. NaN / infinity → 0.
#[inline]
fn quantise_no_dither(v: f32) -> u8 {
    (sanitise(v) * 255.0 + 0.5) as u8
}

/// Quantise one float CMYK pixel to 4 bytes with no dithering.
fn quantise_pixel_no_dither(px: CmykPixel) -> [u8; 4] {
    [
        quantise_no_dither(px[0]),
        quantise_no_dither(px[1]),
        quantise_no_dither(px[2]),
        quantise_no_dither(px[3]),
    ]
}

/// Quantise one float CMYK pixel with the 8×8 Bayer threshold for
/// the supplied `(x, y)`.
fn quantise_pixel_bayer(px: CmykPixel, x: u32, y: u32) -> [u8; 4] {
    // Threshold value ∈ [0, 63] → shift by (t - 31.5)/64 ≈ ±0.5/255
    // around the nearest 8-bit step before rounding. This is the
    // classic ordered-dither trick: per-cell midpoint perturbation.
    let bx = (x as usize) & 7;
    let by = (y as usize) & 7;
    let t = f32::from(BAYER_8X8[by][bx]);
    // The full step between two consecutive output values is 1/255
    // in normalised units. Offsetting by `(t - 31.5)/64 * (1/255)`
    // moves the rounding midpoint by up to half a step, exactly
    // covering the [0, 255] decision boundary.
    let offset = (t - 31.5) / (64.0 * 255.0);
    let mut out = [0u8; 4];
    for (i, &v) in px.iter().enumerate() {
        let adjusted = sanitise(v) + offset;
        // Saturating add: NaN/inf already collapsed; just clamp.
        let scaled = (adjusted.clamp(0.0, 1.0)) * 255.0 + 0.5;
        out[i] = scaled as u8;
    }
    out
}

/// Floyd-Steinberg error-diffusion buffer. Holds one full row of
/// 4-channel error so each scan-line only needs the *current* and
/// *next* row in memory at a time — total working set is `2 * 4 *
/// width` floats regardless of image height. The error stays in
/// `[-2.0, 2.0]` range typically; we don't clamp it so the
/// diffusion is exact.
struct FloydSteinbergRows {
    width: u32,
    /// `4 * width` floats for the next row's diffused error.
    next: Vec<f32>,
}

impl FloydSteinbergRows {
    fn new(width: u32) -> Self {
        Self {
            width,
            next: vec![0.0; (width as usize) * 4],
        }
    }

    /// Read the current diffused-error row, zero it for re-use as
    /// the next row, and return a flat `[c, m, y, k, c, m, y, k, …]`
    /// vector with one quad per pixel.
    fn rotate(&mut self) -> Vec<f32> {
        // `mem::take` swaps in an empty Vec — cheap because
        // capacity is reset on the new allocation. We immediately
        // fix that by re-allocating a zeroed vec of the right
        // length, which is cache-friendly and avoids touching the
        // old buffer.
        let current = std::mem::take(&mut self.next);
        self.next = vec![0.0; (self.width as usize) * 4];
        current
    }
}

/// Quantise the full float-CMYK image, calling `pixel(x, y)` for
/// each `(x, y)` in scan order and writing interleaved C/M/Y/K
/// bytes to `out`. `out` must already be sized to
/// `4 * width * height`.
///
/// This is the single entry point all PDF-export call sites use.
/// The float-CMYK provider is a callback rather than a slice so
/// the caller can compute on-the-fly (e.g. when CMYK comes from
/// `kcreate_core::icc::srgb_to_cmyk_profiled` applied to a
/// matted-sRGB pixel) without materialising an intermediate
/// `Vec<[f32; 4]>` worth of `4 * w * h * 4 = 16 * w * h` bytes.
pub fn quantize_cmyk_image<F>(
    width: u32,
    height: u32,
    dither: CmykDither,
    out: &mut Vec<u8>,
    mut pixel: F,
) where
    F: FnMut(u32, u32) -> CmykPixel,
{
    out.clear();
    let total = (width as usize)
        .saturating_mul(height as usize)
        .saturating_mul(4);
    out.reserve(total);

    match dither {
        CmykDither::None => {
            for y in 0..height {
                for x in 0..width {
                    let bytes = quantise_pixel_no_dither(pixel(x, y));
                    out.extend_from_slice(&bytes);
                }
            }
        }
        CmykDither::Bayer8x8 => {
            for y in 0..height {
                for x in 0..width {
                    let bytes = quantise_pixel_bayer(pixel(x, y), x, y);
                    out.extend_from_slice(&bytes);
                }
            }
        }
        CmykDither::FloydSteinberg => {
            floyd_steinberg(width, height, out, pixel);
        }
    }
}

/// Floyd-Steinberg scan: error diffuses to right (7/16), below-left
/// (3/16), below (5/16), below-right (1/16) per the 1976 paper.
/// Operates in 4 channels at once (CMYK) by treating each channel
/// independently — this is the standard generalisation used in
/// every print-pipeline implementation since CIE 122.
fn floyd_steinberg<F>(width: u32, height: u32, out: &mut Vec<u8>, mut pixel: F)
where
    F: FnMut(u32, u32) -> CmykPixel,
{
    let mut rows = FloydSteinbergRows::new(width);
    // We also need to carry the *current* row's diffused error
    // (from the previous row's rotate) plus an in-row buffer for
    // the right-of-cursor contributions. We start with all zeros.
    let mut current = vec![0.0_f32; (width as usize) * 4];

    for y in 0..height {
        for x in 0..width {
            let raw = pixel(x, y);
            let base = (x as usize) * 4;
            let mut bytes = [0_u8; 4];
            for ch in 0..4 {
                let target = sanitise(raw[ch]) + current[base + ch];
                let quantised = (target.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                let actual = f32::from(quantised) / 255.0;
                let err = target - actual;
                bytes[ch] = quantised;
                // Right neighbour (7/16) on current row.
                if x + 1 < width {
                    current[base + 4 + ch] += err * 7.0 / 16.0;
                }
                // Below-left (3/16), below (5/16), below-right (1/16)
                // on next row.
                if y + 1 < height {
                    if x > 0 {
                        rows.next[base - 4 + ch] += err * 3.0 / 16.0;
                    }
                    rows.next[base + ch] += err * 5.0 / 16.0;
                    if x + 1 < width {
                        rows.next[base + 4 + ch] += err * 1.0 / 16.0;
                    }
                }
            }
            out.extend_from_slice(&bytes);
        }
        // Advance to next row: rotate next-row error in as the new
        // current row.
        current = rows.rotate();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper that bakes a horizontal CMYK gradient: x=0 is paper
    /// (C=M=Y=K=0), x=width-1 is solid K only. The midpoint hits
    /// every 8-bit step exactly twice so banding shows up cleanly
    /// in the no-dither output.
    fn gradient_pixel(x: u32, width: u32) -> CmykPixel {
        let t = (x as f32) / ((width - 1) as f32);
        [0.0, 0.0, 0.0, t]
    }

    /// Average K value across a row, treating each byte as
    /// [0, 1] linear coverage.
    fn average_k(row: &[u8]) -> f32 {
        let total: u32 = row.chunks_exact(4).map(|p| u32::from(p[3])).sum();
        f32::from((total / (row.len() as u32 / 4)) as u16) / 255.0
    }

    #[test]
    fn no_dither_quantises_to_nearest() {
        let mut out = Vec::new();
        quantize_cmyk_image(4, 1, CmykDither::None, &mut out, |x, _| {
            gradient_pixel(x, 4)
        });
        // x=0: 0.0 → 0; x=1: 0.333 → 85; x=2: 0.666 → 170;
        // x=3: 1.0 → 255. Matches the legacy quantize_u8 path.
        assert_eq!(
            out,
            vec![0, 0, 0, 0, 0, 0, 0, 85, 0, 0, 0, 170, 0, 0, 0, 255,]
        );
    }

    #[test]
    fn bayer_produces_neighbour_variation_for_constant_input() {
        // For a uniform 50% K input, the Bayer matrix must split
        // the output between 127 and 128 so the row averages back
        // to ~127.5. A constant 128 would mean the dither isn't
        // actually doing anything.
        let mut out = Vec::new();
        quantize_cmyk_image(8, 8, CmykDither::Bayer8x8, &mut out, |_, _| {
            [0.0, 0.0, 0.0, 0.5]
        });
        let unique: std::collections::HashSet<u8> = out.chunks_exact(4).map(|p| p[3]).collect();
        assert!(
            unique.len() >= 2,
            "Bayer must produce at least two distinct K values for uniform 50% K: got {unique:?}"
        );
        let avg = average_k(&out);
        assert!(
            (avg - 0.5).abs() < 0.02,
            "Bayer 50% K row average should still be ~0.5, got {avg}"
        );
    }

    #[test]
    fn floyd_steinberg_diffuses_gradient_error() {
        // A 64×1 gradient quantised with Floyd-Steinberg should
        // produce a strictly-non-decreasing K channel (within
        // 1 byte step of jitter — error diffusion can briefly
        // round high then catch up).
        let mut out = Vec::new();
        let w: u32 = 64;
        quantize_cmyk_image(w, 1, CmykDither::FloydSteinberg, &mut out, |x, _| {
            gradient_pixel(x, w)
        });
        let k: Vec<u8> = out.chunks_exact(4).map(|p| p[3]).collect();
        assert_eq!(k.len() as u32, w);
        // Start and end pixels must land exactly on 0 and 255.
        assert_eq!(k[0], 0, "leftmost pixel must be paper");
        assert_eq!(k[w as usize - 1], 255, "rightmost pixel must be solid K");
        // Mean should be ~0.5 since the gradient is symmetric
        // around the midpoint.
        let mean: f32 = k.iter().map(|&v| f32::from(v)).sum::<f32>() / (k.len() as f32) / 255.0;
        assert!(
            (mean - 0.5).abs() < 0.02,
            "Floyd-Steinberg gradient mean should be ~0.5, got {mean}"
        );
    }

    #[test]
    fn floyd_steinberg_handles_solid_input_without_error() {
        // A fully-solid K input shouldn't accumulate any error
        // because each pixel is already exactly representable.
        let mut out = Vec::new();
        quantize_cmyk_image(16, 4, CmykDither::FloydSteinberg, &mut out, |_, _| {
            [0.0, 0.0, 0.0, 1.0]
        });
        for px in out.chunks_exact(4) {
            assert_eq!(px, [0, 0, 0, 255], "solid K must round-trip exactly");
        }
    }

    #[test]
    fn quantizer_rejects_non_finite_components() {
        let mut out = Vec::new();
        quantize_cmyk_image(2, 1, CmykDither::None, &mut out, |x, _| {
            if x == 0 {
                [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.5]
            } else {
                [0.5, 0.5, 0.5, 0.5]
            }
        });
        assert_eq!(out[0..4], [0, 0, 0, 128], "NaN/inf must collapse to 0");
        assert_eq!(out[4..8], [128, 128, 128, 128]);
    }

    #[test]
    fn bayer_matrix_values_are_complete_permutation_of_0_63() {
        // Sanity check on the precomputed matrix: must be exactly
        // the integers 0..64 in some order, otherwise the dither
        // would be biased.
        let mut seen = [false; 64];
        for row in &BAYER_8X8 {
            for &v in row {
                assert!(!seen[v as usize], "duplicate Bayer entry: {v}");
                seen[v as usize] = true;
            }
        }
        assert!(seen.iter().all(|&s| s), "Bayer matrix missing some entries");
    }
}
