//! Glyph extraction from a raster crop — Phase 10 Block B Task 8.
//!
//! Pipeline:
//!
//! 1. Crop a rectangular region out of the source raster (provided
//!    by the bridge in pixel coordinates).
//! 2. Convert RGBA to grayscale (BT.709 luma) and apply Otsu's
//!    method to find a threshold that separates the glyph from the
//!    background, with a contrast check to detect the foreground
//!    polarity (dark-on-light vs light-on-dark).
//! 3. Trace the resulting binary mask via the existing
//!    [`crate::trace`] module, which runs marching-squares + RDP
//!    simplification.
//! 4. Aggressively simplify and remap into a 1000-unit em-square
//!    suitable for use as a typographic glyph (`ascender`,
//!    `cap_height`, `x_height`, `baseline`, `descender`).
//!
//! This module produces normalized `TracedPath`s; the bridge takes
//! the result and inserts vector nodes into the document.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::trace::{trace_raster, TraceError, TraceOptions, TraceThreshold, TracedPath};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GlyphCrop {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GlyphExtractOptions {
    /// Em-square dimension to normalize into. Standard fonts use
    /// 1000 or 2048; we default to 1000 because the bridge converts
    /// into a single VectorLayer where the unit doesn't materially
    /// affect rendering.
    pub em_size: f64,
    /// Path-simplification tolerance, expressed in normalized
    /// em-square units (so it's resolution-independent).
    pub simplify_tolerance: f64,
}

impl Default for GlyphExtractOptions {
    fn default() -> Self {
        Self {
            em_size: 1000.0,
            simplify_tolerance: 4.0,
        }
    }
}

impl GlyphExtractOptions {
    #[must_use]
    pub fn clamped(mut self) -> Self {
        if !self.em_size.is_finite() || self.em_size <= 0.0 {
            self.em_size = 1000.0;
        }
        self.em_size = self.em_size.clamp(64.0, 16_384.0);
        if !self.simplify_tolerance.is_finite() || self.simplify_tolerance < 0.0 {
            self.simplify_tolerance = 0.0;
        }
        self.simplify_tolerance = self.simplify_tolerance.min(64.0);
        self
    }
}

/// Standard typographic metrics in em-square units. Reasonable
/// defaults for a Latin sans-serif — these are starting suggestions
/// that the bridge surfaces to the user.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GlyphMetrics {
    pub em: f64,
    pub ascender: f64,
    pub cap_height: f64,
    pub x_height: f64,
    pub baseline: f64,
    pub descender: f64,
}

impl GlyphMetrics {
    #[must_use]
    pub fn standard(em: f64) -> Self {
        Self {
            em,
            ascender: em * 0.8,
            cap_height: em * 0.7,
            x_height: em * 0.52,
            baseline: 0.0,
            descender: -em * 0.2,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtractedGlyph {
    pub paths: Vec<TracedPath>,
    pub metrics: GlyphMetrics,
    pub bounding_box: (f64, f64, f64, f64),
}

#[derive(Debug, Error)]
pub enum GlyphExtractError {
    #[error("glyph_extract: empty image")]
    Empty,
    #[error("glyph_extract: crop region falls outside image bounds")]
    CropOutOfBounds,
    #[error("glyph_extract: pixel buffer length {got} != expected {expected}")]
    BufferSize { got: usize, expected: usize },
    #[error(transparent)]
    Trace(#[from] TraceError),
}

/// Extract glyph outlines from a raster region.
///
/// # Errors
///
/// Returns [`GlyphExtractError`] for empty inputs, buffer-size
/// mismatches, out-of-bounds crops, or trace failures.
pub fn extract_glyph(
    pixels: &[u8],
    width: u32,
    height: u32,
    crop: GlyphCrop,
    options: GlyphExtractOptions,
) -> Result<ExtractedGlyph, GlyphExtractError> {
    if width == 0 || height == 0 {
        return Err(GlyphExtractError::Empty);
    }
    let expected = (width as usize) * (height as usize) * 4;
    if pixels.len() != expected {
        return Err(GlyphExtractError::BufferSize {
            got: pixels.len(),
            expected,
        });
    }
    let opts = options.clamped();
    let cx = crop.x;
    let cy = crop.y;
    let cw = crop.width;
    let ch = crop.height;
    if cw == 0 || ch == 0 || cx + cw > width || cy + ch > height {
        return Err(GlyphExtractError::CropOutOfBounds);
    }

    // Slice out the crop into a contiguous buffer.
    let mut cropped = vec![0u8; (cw as usize) * (ch as usize) * 4];
    for row in 0..ch {
        let src_off = (((cy + row) * width + cx) * 4) as usize;
        let dst_off = ((row * cw) * 4) as usize;
        cropped[dst_off..dst_off + (cw as usize) * 4]
            .copy_from_slice(&pixels[src_off..src_off + (cw as usize) * 4]);
    }

    // Detect polarity: if the average luma is > 128 we're dark-on-light
    // (typical printed type). Otherwise flip the buffer before trace
    // so the foreground reads as "below threshold".
    let mut sum_y = 0u64;
    let total_px = (cw as usize) * (ch as usize);
    for chunk in cropped.chunks(4) {
        let y =
            0.299 * f32::from(chunk[0]) + 0.587 * f32::from(chunk[1]) + 0.114 * f32::from(chunk[2]);
        sum_y += y as u64;
    }
    let mean_y = sum_y as f32 / total_px as f32;
    if mean_y < 128.0 {
        // Light-on-dark; invert so trace's Otsu picks the glyph
        // region as foreground.
        for chunk in cropped.chunks_mut(4) {
            chunk[0] = 255 - chunk[0];
            chunk[1] = 255 - chunk[1];
            chunk[2] = 255 - chunk[2];
        }
    }

    let raw = trace_raster(
        &cropped,
        cw,
        ch,
        &TraceOptions {
            threshold: TraceThreshold::Auto,
            simplify_tolerance: opts.simplify_tolerance.max(1.0) as f32,
            // Letterforms can be as small as a 4-pixel-wide stem on
            // a tight crop. Asking for >= 4 points loses those
            // outlines after RDP simplification reduces them to a
            // bounding triangle.
            min_path_points: 3,
            // Letterforms have intentionally crisp edges; pre-blur
            // smears narrow horizontal/vertical stems into the
            // background, which then trips Otsu thresholding on
            // small crops. Trust the source contrast.
            smooth: false,
        },
    )?;

    // Compute glyph bounding box in source-crop units and normalize
    // every point into an em-square. Y is flipped so positive Y goes
    // up (the typographic convention) rather than down (image
    // convention).
    let (min_x, min_y, max_x, max_y) = bounding_box(&raw);
    let span_x = (max_x - min_x).max(1.0);
    let span_y = (max_y - min_y).max(1.0);
    let scale = opts.em_size / span_x.max(span_y);
    let normalized: Vec<TracedPath> = raw
        .into_iter()
        .map(|p| TracedPath {
            points: p
                .points
                .into_iter()
                .map(|pt| crate::trace::TracedPoint {
                    x: ((f64::from(pt.x) - min_x) * scale) as f32,
                    // Flip Y axis: in image coords Y grows down; in
                    // font coords Y grows up. We translate so the
                    // glyph's bottom sits at baseline = 0.
                    y: ((max_y - f64::from(pt.y)) * scale) as f32,
                })
                .collect(),
            closed: p.closed,
        })
        .collect();
    let bbox = (0.0, 0.0, span_x * scale, span_y * scale);
    let metrics = GlyphMetrics::standard(opts.em_size);
    Ok(ExtractedGlyph {
        paths: normalized,
        metrics,
        bounding_box: bbox,
    })
}

fn bounding_box(paths: &[TracedPath]) -> (f64, f64, f64, f64) {
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for p in paths {
        for pt in &p.points {
            let x = f64::from(pt.x);
            let y = f64::from(pt.y);
            if x < min_x {
                min_x = x;
            }
            if x > max_x {
                max_x = x;
            }
            if y < min_y {
                min_y = y;
            }
            if y > max_y {
                max_y = y;
            }
        }
    }
    if !min_x.is_finite() {
        // No points found — return a unit box so the caller still
        // gets sensible numbers.
        return (0.0, 0.0, 1.0, 1.0);
    }
    (min_x, min_y, max_x, max_y)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, c: [u8; 4]) -> Vec<u8> {
        let mut v = Vec::with_capacity((w as usize) * (h as usize) * 4);
        for _ in 0..(w as usize * h as usize) {
            v.extend_from_slice(&c);
        }
        v
    }

    #[test]
    fn empty_image_errors() {
        assert!(matches!(
            extract_glyph(
                &[],
                0,
                0,
                GlyphCrop {
                    x: 0,
                    y: 0,
                    width: 1,
                    height: 1
                },
                GlyphExtractOptions::default()
            ),
            Err(GlyphExtractError::Empty)
        ));
    }

    #[test]
    fn out_of_bounds_crop_errors() {
        let img = solid(8, 8, [255, 255, 255, 255]);
        assert!(matches!(
            extract_glyph(
                &img,
                8,
                8,
                GlyphCrop {
                    x: 4,
                    y: 4,
                    width: 100,
                    height: 100
                },
                GlyphExtractOptions::default()
            ),
            Err(GlyphExtractError::CropOutOfBounds)
        ));
    }

    #[test]
    fn dark_on_light_glyph_produces_paths_normalized_to_em_size() {
        // 16×16 white image with a 4×4 black square centred — the
        // "glyph" the user wants to extract.
        let w = 16u32;
        let h = 16u32;
        let mut img = solid(w, h, [255, 255, 255, 255]);
        for y in 6..10 {
            for x in 6..10 {
                let i = ((y * w + x) * 4) as usize;
                img[i] = 0;
                img[i + 1] = 0;
                img[i + 2] = 0;
            }
        }
        let crop = GlyphCrop {
            x: 5,
            y: 5,
            width: 6,
            height: 6,
        };
        let g = extract_glyph(
            &img,
            w,
            h,
            crop,
            GlyphExtractOptions {
                em_size: 1000.0,
                simplify_tolerance: 1.0,
            },
        )
        .unwrap();
        assert!(!g.paths.is_empty(), "should trace at least one path");
        // The bounding box should not exceed em_size on either axis.
        assert!(g.bounding_box.2 <= 1000.5);
        assert!(g.bounding_box.3 <= 1000.5);
        assert!((g.metrics.em - 1000.0).abs() < 1e-6);
    }

    #[test]
    fn options_clamping_keeps_em_size_in_range() {
        let opts = GlyphExtractOptions {
            em_size: -1.0,
            simplify_tolerance: -5.0,
        }
        .clamped();
        assert!(opts.em_size >= 64.0);
        assert!(opts.simplify_tolerance >= 0.0);
    }
}
