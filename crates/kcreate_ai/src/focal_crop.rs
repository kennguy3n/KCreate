//! Content-aware focal-point crop for Magic Resize.
//!
//! When an artboard that contains a raster image changes aspect ratio
//! (square → 9:16 story → A4), naively stretching the image to the new
//! bounds distorts the subject. Instead we crop the source image to a
//! rectangle of the *target* aspect ratio, centred on a detected focal
//! point, so the subject stays framed and the pixels are never
//! squashed.
//!
//! The focal point comes from **center-weighted edge saliency**, built
//! on the same primitives the screenshot-to-layout heuristic already
//! uses:
//!
//! 1. Downsample the image to a bounded working size (so the cost is
//!    independent of the source resolution — important for 12-megapixel
//!    photos).
//! 2. Convert to grayscale and run the 3×3 Sobel edge detector
//!    ([`crate::screenshot_to_layout::sobel`]).
//! 3. Take the centroid of the edge pixels, weighted by a broad
//!    Gaussian **center prior** — flat backgrounds carry no edges, so
//!    the centroid is pulled toward the textured subject, while the
//!    prior reflects the photographer's bias of keeping subjects near
//!    the middle and stops a single noisy corner from dominating.
//! 4. The fraction of edge pixels becomes a `[0, 1]` confidence. When
//!    there is no edge signal at all (a flat image) the detector
//!    **degrades to the geometric center** with zero confidence, which
//!    yields a plain center-crop.
//!
//! Everything here is pure and offline: no I/O, no network, no global
//! state. The only inputs are the RGBA8 pixel buffer and the target
//! dimensions.

use crate::screenshot_to_layout::{sobel, to_grayscale};

/// Longest edge of the working buffer the saliency pass runs on. The
/// focal point is reported in normalized coordinates, so downsampling
/// to this bound does not change the result meaningfully while keeping
/// the cost `O(MAX_SALIENCY_DIM²)` regardless of the source size.
const MAX_SALIENCY_DIM: u32 = 384;

/// Standard deviation (in half-image units, where the image spans
/// `[-1, 1]` on each axis) of the Gaussian center prior. ~0.75 keeps a
/// gentle bias: a clearly off-center subject still wins, but the prior
/// damps lone edges in the corners.
const CENTER_PRIOR_SIGMA: f32 = 0.75;

/// Edge-pixel fraction that maps to full (`1.0`) confidence. Natural
/// photos with a clear subject sit well above this; flat or nearly
/// flat images fall toward zero and trigger the center-crop fallback.
const FULL_CONFIDENCE_EDGE_FRACTION: f32 = 0.08;

/// A crop rectangle in source-image **pixel** coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FocalCrop {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Result of [`focal_point`]: a normalized `[0, 1]` focal coordinate
/// plus a `[0, 1]` confidence. `confidence == 0.0` means "no signal —
/// fall back to the center".
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FocalPoint {
    pub x: f32,
    pub y: f32,
    pub confidence: f32,
}

impl FocalPoint {
    /// The neutral focal estimate: dead center, no confidence.
    const CENTER: Self = Self {
        x: 0.5,
        y: 0.5,
        confidence: 0.0,
    };
}

/// Detect a focal point in an RGBA8 image via center-weighted edge
/// saliency. Returns normalized coordinates in `[0, 1]`.
///
/// Degrades to [`FocalPoint::CENTER`] (the geometric center, zero
/// confidence) when the input is empty/malformed or carries no edge
/// signal, so a flat image cleanly becomes a center-crop.
#[must_use]
pub fn focal_point(pixels: &[u8], width: u32, height: u32) -> FocalPoint {
    let total = (width as usize) * (height as usize);
    if total == 0 || pixels.len() != total * 4 || width < 3 || height < 3 {
        return FocalPoint::CENTER;
    }

    // 1. Downsample to a bounded working buffer. `step` is the integer
    //    stride that brings the longest edge at or below the cap.
    let step = (width.max(height)).div_ceil(MAX_SALIENCY_DIM).max(1);
    let sw = (width.div_ceil(step)).max(1);
    let sh = (height.div_ceil(step)).max(1);
    let small = if step == 1 {
        // Already small enough — borrow the input directly.
        DownsampledImage::Borrowed(pixels)
    } else {
        DownsampledImage::Owned(downsample_rgba(pixels, width, height, step, sw, sh))
    };
    let small_pixels = small.as_slice();

    // Need at least a 3×3 working buffer for Sobel to produce signal.
    if sw < 3 || sh < 3 {
        return FocalPoint::CENTER;
    }

    // 2. Grayscale → Sobel edge map (binary, thresholded).
    let gray = to_grayscale(small_pixels, sw, sh);
    let edges = sobel(&gray, sw, sh);

    // 3. Center-weighted centroid of the edge pixels.
    let w = sw as usize;
    let h = sh as usize;
    let inv_w = 1.0 / (sw as f32 - 1.0).max(1.0);
    let inv_h = 1.0 / (sh as f32 - 1.0).max(1.0);
    let two_sigma_sq = 2.0 * CENTER_PRIOR_SIGMA * CENTER_PRIOR_SIGMA;

    let mut weight_sum = 0.0f32;
    let mut wx = 0.0f32;
    let mut wy = 0.0f32;
    let mut edge_count: u64 = 0;
    for y in 0..h {
        // Normalized vertical position in [-1, 1].
        let ny = (y as f32 * inv_h - 0.5) * 2.0;
        for x in 0..w {
            if edges[y * w + x] == 0 {
                continue;
            }
            edge_count += 1;
            let nx = (x as f32 * inv_w - 0.5) * 2.0;
            let prior = (-(nx * nx + ny * ny) / two_sigma_sq).exp();
            weight_sum += prior;
            wx += prior * (x as f32 * inv_w);
            wy += prior * (y as f32 * inv_h);
        }
    }

    if weight_sum <= f32::EPSILON || edge_count == 0 {
        return FocalPoint::CENTER;
    }

    let edge_fraction = edge_count as f32 / (w * h) as f32;
    let confidence = (edge_fraction / FULL_CONFIDENCE_EDGE_FRACTION).clamp(0.0, 1.0);
    FocalPoint {
        x: (wx / weight_sum).clamp(0.0, 1.0),
        y: (wy / weight_sum).clamp(0.0, 1.0),
        confidence,
    }
}

/// Compute the largest crop rectangle of aspect `target_w : target_h`
/// that fits inside a `width × height` source image, centred on the
/// detected focal point and clamped to stay fully inside the image.
///
/// Returns `None` only for degenerate input (zero dimensions or a
/// pixel buffer whose length doesn't match `width × height × 4`). When
/// the image carries no saliency signal the crop is centred (a plain
/// center-crop) via [`focal_point`]'s fallback.
#[must_use]
pub fn content_aware_crop(
    pixels: &[u8],
    width: u32,
    height: u32,
    target_w: u32,
    target_h: u32,
) -> Option<FocalCrop> {
    let total = (width as usize) * (height as usize);
    if total == 0 || pixels.len() != total * 4 || target_w == 0 || target_h == 0 {
        return None;
    }

    let focal = focal_point(pixels, width, height);
    Some(crop_for_focal(
        width, height, target_w, target_h, focal.x, focal.y,
    ))
}

/// Pure geometry: the max-area crop of aspect `target_w:target_h`
/// inside `width × height`, centred on the normalized focal point and
/// clamped inside the image. Split out so it can be unit-tested
/// without constructing pixel buffers.
#[must_use]
pub fn crop_for_focal(
    width: u32,
    height: u32,
    target_w: u32,
    target_h: u32,
    focal_x: f32,
    focal_y: f32,
) -> FocalCrop {
    let w = f64::from(width);
    let h = f64::from(height);
    let target_aspect = f64::from(target_w) / f64::from(target_h);
    let source_aspect = w / h;

    // Max rectangle of `target_aspect` fitting inside the source.
    let (cw, ch) = if source_aspect > target_aspect {
        // Source is wider than the target: full height, narrower width.
        let cw = (h * target_aspect).round();
        (cw.min(w), h)
    } else {
        // Source is taller/narrower: full width, shorter height.
        let ch = (w / target_aspect).round();
        (w, ch.min(h))
    };
    let cw = (cw.round() as u32).clamp(1, width);
    let ch = (ch.round() as u32).clamp(1, height);

    // Centre the crop on the focal point, then clamp so it stays
    // entirely inside the image.
    let focal_px = f64::from(focal_x.clamp(0.0, 1.0)) * w;
    let focal_py = f64::from(focal_y.clamp(0.0, 1.0)) * h;
    let max_x = width - cw;
    let max_y = height - ch;
    let x = (focal_px - f64::from(cw) / 2.0)
        .round()
        .clamp(0.0, f64::from(max_x)) as u32;
    let y = (focal_py - f64::from(ch) / 2.0)
        .round()
        .clamp(0.0, f64::from(max_y)) as u32;

    FocalCrop {
        x,
        y,
        width: cw,
        height: ch,
    }
}

/// Apply a [`FocalCrop`] to an RGBA8 buffer, returning the cropped
/// pixels row-by-row. The crop rectangle is clamped to `(width, height)`
/// (it is produced by [`content_aware_crop`] for exactly these
/// dimensions), and each row is bounds-checked against the actual buffer
/// length — so a `pixels` slice shorter than `width * height * 4` (a
/// malformed caller) stops the copy early rather than panicking. The
/// function is therefore total for every input.
#[must_use]
pub fn apply_crop(pixels: &[u8], width: u32, height: u32, crop: FocalCrop) -> Vec<u8> {
    let src_w = width as usize;
    let x0 = (crop.x as usize).min(src_w);
    let y0 = (crop.y as usize).min(height as usize);
    let cw = (crop.width as usize).min(src_w - x0);
    let ch = (crop.height as usize).min(height as usize - y0);
    let mut out = Vec::with_capacity(cw * ch * 4);
    for row in 0..ch {
        let src_row = (y0 + row) * src_w + x0;
        let start = src_row * 4;
        let end = start + cw * 4;
        // Row offsets are monotonically increasing, so the first row that
        // runs past a short buffer means every later row would too — bail
        // out instead of indexing out of bounds.
        match pixels.get(start..end) {
            Some(slice) => out.extend_from_slice(slice),
            None => break,
        }
    }
    out
}

/// Borrowed-or-owned downsample buffer so the common "already small"
/// path avoids a copy.
enum DownsampledImage<'a> {
    Borrowed(&'a [u8]),
    Owned(Vec<u8>),
}

impl DownsampledImage<'_> {
    fn as_slice(&self) -> &[u8] {
        match self {
            Self::Borrowed(s) => s,
            Self::Owned(v) => v,
        }
    }
}

/// Nearest-neighbour downsample of an RGBA8 buffer by an integer
/// `step`, producing an `sw × sh` buffer. Cheap and deterministic —
/// adequate because the output only feeds a coarse saliency centroid.
fn downsample_rgba(pixels: &[u8], width: u32, height: u32, step: u32, sw: u32, sh: u32) -> Vec<u8> {
    let src_w = width as usize;
    let mut out = Vec::with_capacity((sw as usize) * (sh as usize) * 4);
    for sy in 0..sh {
        let src_y = ((sy * step) as usize).min(height as usize - 1);
        for sx in 0..sw {
            let src_x = ((sx * step) as usize).min(src_w - 1);
            let idx = (src_y * src_w + src_x) * 4;
            out.extend_from_slice(&pixels[idx..idx + 4]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an RGBA image with a flat background and a bright,
    /// high-contrast textured block placed at the given pixel rect.
    fn image_with_subject(
        w: u32,
        h: u32,
        bg: [u8; 3],
        rect: (u32, u32, u32, u32),
        fg: [u8; 3],
    ) -> Vec<u8> {
        let (rx, ry, rw, rh) = rect;
        let mut px = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let in_rect = x >= rx && x < rx + rw && y >= ry && y < ry + rh;
                let c = if in_rect {
                    // Checkerboard so the block carries lots of edges.
                    if (x + y) % 2 == 0 {
                        fg
                    } else {
                        [255 - fg[0], 255 - fg[1], 255 - fg[2]]
                    }
                } else {
                    bg
                };
                px.extend_from_slice(&[c[0], c[1], c[2], 255]);
            }
        }
        px
    }

    #[test]
    fn focal_point_tracks_an_off_center_subject() {
        // Subject in the lower-right quadrant of a flat image.
        let w = 200;
        let h = 200;
        let img = image_with_subject(w, h, [240, 240, 240], (130, 130, 50, 50), [10, 10, 10]);
        let focal = focal_point(&img, w, h);
        assert!(focal.confidence > 0.0, "textured subject must register");
        // The centroid must land in the lower-right half, biased by the
        // center prior but still clearly off-center toward the subject.
        assert!(
            focal.x > 0.5,
            "focal.x={} should be right of center",
            focal.x
        );
        assert!(focal.y > 0.5, "focal.y={} should be below center", focal.y);
    }

    #[test]
    fn flat_image_degrades_to_center() {
        let w = 128;
        let h = 128;
        let flat: Vec<u8> = std::iter::repeat_n([120u8, 120, 120, 255], (w * h) as usize)
            .flatten()
            .collect();
        let focal = focal_point(&flat, w, h);
        assert_eq!(focal, FocalPoint::CENTER);
    }

    #[test]
    fn crop_keeps_the_subject_in_frame() {
        // A wide source cropped to a tall (portrait) target. The
        // subject sits lower-right; the resulting crop rectangle must
        // contain the whole subject rect.
        let w = 400;
        let h = 200;
        let subject = (300, 120, 60, 60); // lower-right
        let img = image_with_subject(w, h, [230, 230, 230], subject, [0, 0, 0]);
        // Target aspect 9:16 (portrait) — much taller than the source.
        let crop = content_aware_crop(&img, w, h, 1080, 1920).expect("crop");
        let (sx, sy, sw, sh) = subject;
        // The crop is a valid sub-rect of the source…
        assert!(crop.x + crop.width <= w);
        assert!(crop.y + crop.height <= h);
        // …with the target aspect (portrait → width < height).
        assert!(crop.width <= crop.height);
        // …and it fully contains the subject.
        assert!(
            crop.x <= sx && crop.x + crop.width >= sx + sw,
            "crop x-range [{}, {}] must contain subject x [{}, {}]",
            crop.x,
            crop.x + crop.width,
            sx,
            sx + sw,
        );
        assert!(
            crop.y <= sy && crop.y + crop.height >= sy + sh,
            "crop y-range must contain subject y",
        );
    }

    #[test]
    fn crop_geometry_matches_target_aspect_and_fits() {
        // Center focal, landscape target inside a square source.
        let crop = crop_for_focal(1000, 1000, 1600, 900, 0.5, 0.5);
        assert_eq!(crop.width, 1000); // width-limited
        assert_eq!(crop.height, (1000.0_f64 / (1600.0 / 900.0)).round() as u32);
        assert!(crop.x + crop.width <= 1000);
        assert!(crop.y + crop.height <= 1000);
        // Centered vertically.
        let expected_y = (1000 - crop.height) / 2;
        assert!((i64::from(crop.y) - i64::from(expected_y)).abs() <= 1);
    }

    #[test]
    fn apply_crop_extracts_the_rectangle() {
        let w = 4;
        let h = 4;
        // Distinct per-pixel red channel = x + y*w so we can verify.
        let mut px = Vec::new();
        for y in 0..h {
            for x in 0..w {
                px.extend_from_slice(&[(x + y * w) as u8, 0, 0, 255]);
            }
        }
        let crop = FocalCrop {
            x: 1,
            y: 1,
            width: 2,
            height: 2,
        };
        let out = apply_crop(&px, w, h, crop);
        assert_eq!(out.len(), 2 * 2 * 4);
        // Top-left of the crop is (x=1, y=1) → red = 1 + 1*4 = 5.
        assert_eq!(out[0], 5);
        // Next pixel (x=2, y=1) → red = 6.
        assert_eq!(out[4], 6);
        // Second row (x=1, y=2) → red = 1 + 2*4 = 9.
        assert_eq!(out[8], 9);
    }

    #[test]
    fn apply_crop_is_total_on_a_short_buffer() {
        // A caller passing dimensions larger than the actual buffer must
        // not panic: the copy stops at the last fully-present row. Here we
        // claim 4×4 but only supply two rows of pixels.
        let w = 4;
        let h = 4;
        let mut px = Vec::new();
        for y in 0..2 {
            for x in 0..w {
                px.extend_from_slice(&[(x + y * w) as u8, 0, 0, 255]);
            }
        }
        let crop = FocalCrop {
            x: 0,
            y: 0,
            width: 4,
            height: 4,
        };
        // No panic, and we get back exactly the rows that existed.
        let out = apply_crop(&px, w, h, crop);
        assert_eq!(out.len(), 2 * 4 * 4);
        assert_eq!(out, px);

        // A completely empty buffer yields an empty crop, still no panic.
        assert!(apply_crop(&[], w, h, crop).is_empty());
    }

    #[test]
    fn content_aware_crop_rejects_degenerate_input() {
        assert!(content_aware_crop(&[], 0, 0, 10, 10).is_none());
        assert!(content_aware_crop(&[0, 0, 0, 255], 1, 1, 0, 10).is_none());
        // Wrong-length buffer.
        assert!(content_aware_crop(&[0, 0, 0], 1, 1, 10, 10).is_none());
    }
}
