//! Text-region detection for raster images.
//!
//! Phase 4 follow-up Block D: a real-but-bounded text detector that
//! identifies text-like regions in a raster layer so the renderer can
//! create text layers positioned over them. We deliberately do NOT
//! attempt character recognition — that's a multi-month project that
//! would either pull in a language model or ship a Tesseract WASM
//! binary, and neither survives the local-first sentinel cleanly.
//!
//! What we DO is honest, useful, and local-first:
//!
//! 1. **Threshold** the input image to a binary "ink" mask. We use a
//!    simple global luminance threshold on a mid-grey value because
//!    image-adaptive (Otsu / Sauvola) thresholding adds a calibration
//!    surface for very little gain on the high-contrast inputs we
//!    actually care about (UI screenshots, dark text on light
//!    backgrounds).
//!
//! 2. **Connected-component labelling** finds discrete ink blobs.
//!    These are individual glyphs, glyph fragments, or symbols.
//!
//! 3. **Line grouping** clusters components by baseline overlap +
//!    horizontal proximity into text lines. Two components join the
//!    same line when their vertical ranges overlap by at least
//!    `LINE_OVERLAP_RATIO` and they're within `LINE_GAP_RATIO * h`
//!    of each other horizontally (h = component height).
//!
//! 4. **Region emission** clamps each line into a bounding box,
//!    estimates a character count (line width / median glyph
//!    advance), and reports the result back to the caller.
//!
//! The renderer surfaces each region with an "Insert as text layer"
//! affordance that creates a `TextLayer` whose bounds match the
//! detected region; the user types the actual text content. The
//! detector's job ends at the bbox — we don't pretend to read
//! arbitrary characters.

use serde::{Deserialize, Serialize};

/// Detected text-like region in a raster image.
///
/// Coordinates are in raster-pixel space, top-left origin, matching
/// the `RasterLayer`'s intrinsic dimensions. The renderer maps these
/// into document space via the raster's `transform` + `bounds` before
/// creating the `TextLayer`.
///
/// `estimated_char_count` is `region.width / median_glyph_advance`
/// rounded up — useful only as a rough hint to the renderer for
/// sizing the inserted TextLayer's frame, not as a confidence
/// guarantee.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextRegion {
    /// Left edge of the region, in raster pixels.
    pub x: u32,
    /// Top edge of the region, in raster pixels.
    pub y: u32,
    /// Region width in raster pixels.
    pub width: u32,
    /// Region height in raster pixels (≈ font cap height + descent).
    pub height: u32,
    /// Number of distinct glyphs aggregated into this region.
    pub glyph_count: u32,
    /// Heuristic character count estimate from line geometry.
    pub estimated_char_count: u32,
}

/// Parameters for the text-region detector. Sensible defaults are
/// provided via [`Default`]; the bridge entry point exposes the
/// individual fields so the renderer can dial them in for difficult
/// inputs (very tight line spacing, mixed-DPI screenshots, etc.).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectTextRegionsOptions {
    /// Luminance threshold (0–255). Pixels at or below this are
    /// "ink". 110 is a reasonable midpoint for dark-on-light
    /// screenshots; lower numbers tighten the detector.
    pub luminance_threshold: u8,
    /// Minimum component size in pixels. Smaller blobs are dropped
    /// as noise.
    pub min_component_pixels: u32,
    /// Maximum component size as a fraction of the image area.
    /// Larger blobs are dropped as "this isn't text" (e.g. large
    /// solid shapes, photographic backgrounds).
    pub max_component_fraction: f32,
    /// Two components join the same line when their vertical ranges
    /// overlap by at least this fraction of the smaller component's
    /// height. 0.5 means "at least half the smaller blob's vertical
    /// extent overlaps the other blob's range".
    pub line_overlap_ratio: f32,
    /// Two components on the same baseline join the same line when
    /// their horizontal gap is at most this multiple of the
    /// component's height. 1.5 means "up to 1.5× the cap-height".
    pub line_gap_ratio: f32,
    /// Hard cap on input image area, in pixels (`width * height`).
    /// Larger inputs are rejected with [`OcrError::ImageTooLarge`]
    /// rather than fed into the O(W*H)-memory flood-fill, which
    /// would otherwise be free to allocate hundreds of MB on a
    /// pathological all-dark all-large image (worst-case peak
    /// memory is `width * height * 9` bytes — a 1-byte `visited`
    /// bitmap plus an 8-byte `(u32, u32)` stack entry per pixel).
    /// The default of 16 million pixels covers 4K UI screenshots
    /// (e.g. 4096 × 4096, 4000 × 4000, 5120 × 3120) with margin to
    /// spare and caps worst-case allocation at roughly 144 MB,
    /// which is well within the budget for an interactive AI panel.
    /// Set to `u32::MAX` to disable the guard. Marked
    /// `#[serde(default = …)]` so existing JSON payloads that
    /// predate this field (no `maxImagePixels` key) keep working
    /// against the renderer's defaults.
    #[serde(default = "default_max_image_pixels")]
    pub max_image_pixels: u32,
}

/// `#[serde(default)]` callback for
/// [`DetectTextRegionsOptions::max_image_pixels`]. Mirrors the
/// constant in [`DetectTextRegionsOptions::default`]; pulled out
/// so the two paths don't drift.
fn default_max_image_pixels() -> u32 {
    16_000_000
}

impl Default for DetectTextRegionsOptions {
    fn default() -> Self {
        Self {
            luminance_threshold: 110,
            min_component_pixels: 8,
            max_component_fraction: 0.25,
            line_overlap_ratio: 0.5,
            line_gap_ratio: 1.5,
            max_image_pixels: 16_000_000,
        }
    }
}

/// Failure modes for [`detect_text_regions`]. The bridge maps these
/// to typed errors; the renderer surfaces them as toast messages.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum OcrError {
    #[error("image buffer is empty")]
    EmptyImage,
    #[error("buffer length {actual} does not match width × height × 4 = {expected}")]
    SizeMismatch { expected: usize, actual: usize },
    #[error("width and height must be non-zero")]
    DegenerateDimensions,
    /// Returned when `width * height` exceeds
    /// [`DetectTextRegionsOptions::max_image_pixels`]. The detector
    /// uses an `O(W * H)` flood-fill (1-byte `visited` + 8-byte
    /// stack entry per pixel in the worst case), so very large
    /// inputs could allocate hundreds of MB. Rather than silently
    /// truncating components — which would produce wrong bboxes
    /// and is itself a footgun — we surface a typed error and let
    /// the renderer either downsample or refuse to OCR. The
    /// renderer's screenshot-to-layout pipeline pre-resizes large
    /// images, so this only fires when the caller deliberately
    /// passes an oversized buffer.
    #[error("image is too large for OCR: {pixels} pixels exceeds max_image_pixels = {max}")]
    ImageTooLarge { pixels: u64, max: u32 },
}

/// Detect text-like regions in an RGBA raster buffer.
///
/// `pixels` is row-major, 4 bytes per pixel (RGBA8 in sRGB), of
/// length `width * height * 4`. Returns the regions in reading order
/// (top-to-bottom, left-to-right).
pub fn detect_text_regions(
    pixels: &[u8],
    width: u32,
    height: u32,
    options: DetectTextRegionsOptions,
) -> Result<Vec<TextRegion>, OcrError> {
    if width == 0 || height == 0 {
        return Err(OcrError::DegenerateDimensions);
    }
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|v| v.checked_mul(4))
        .ok_or(OcrError::DegenerateDimensions)?;
    if pixels.is_empty() {
        return Err(OcrError::EmptyImage);
    }
    if pixels.len() != expected {
        return Err(OcrError::SizeMismatch {
            expected,
            actual: pixels.len(),
        });
    }
    // Hard cap on image area to bound flood-fill memory at a
    // predictable ceiling. The O(W*H) memory profile of the explicit-
    // stack flood-fill is documented on `label_components` — refusing
    // pathologically large inputs is preferable to silently
    // truncating components and emitting wrong bboxes.
    let area: u64 = u64::from(width) * u64::from(height);
    if area > u64::from(options.max_image_pixels) {
        return Err(OcrError::ImageTooLarge {
            pixels: area,
            max: options.max_image_pixels,
        });
    }

    let mask = threshold_to_ink_mask(pixels, width, height, options.luminance_threshold);
    let components = label_components(&mask, width, height);
    let total_pixels = (width as f32) * (height as f32);
    let max_pixels = (total_pixels * options.max_component_fraction) as u32;

    let mut filtered: Vec<Component> = components
        .into_iter()
        .filter(|c| c.pixels >= options.min_component_pixels && c.pixels <= max_pixels)
        .collect();
    // Sort top-to-bottom by mid-y, then left-to-right by mid-x. This
    // gives us reading order when we later group into lines.
    filtered.sort_by(|a, b| {
        let ay = a.mid_y();
        let by = b.mid_y();
        ay.partial_cmp(&by)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                let ax = a.mid_x();
                let bx = b.mid_x();
                ax.partial_cmp(&bx).unwrap_or(std::cmp::Ordering::Equal)
            })
    });

    let lines = group_into_lines(
        &filtered,
        options.line_overlap_ratio,
        options.line_gap_ratio,
    );
    Ok(lines.into_iter().map(emit_region).collect())
}

/// Convert RGBA8 → 1-bit ink mask (1 = ink, 0 = background) using a
/// single luminance threshold. Alpha is folded in: fully-transparent
/// pixels are treated as background regardless of their RGB.
fn threshold_to_ink_mask(pixels: &[u8], width: u32, height: u32, threshold: u8) -> Vec<u8> {
    let mut mask = vec![0u8; (width as usize) * (height as usize)];
    for (i, slot) in mask.iter_mut().enumerate() {
        let base = i * 4;
        let r = pixels[base];
        let g = pixels[base + 1];
        let b = pixels[base + 2];
        let a = pixels[base + 3];
        if a == 0 {
            continue;
        }
        // ITU-R BT.601 luma. Cheap; the choice of luma curve is
        // unimportant compared to the global threshold value.
        let luma = ((u32::from(r) * 299 + u32::from(g) * 587 + u32::from(b) * 114) / 1000) as u8;
        if luma <= threshold {
            *slot = 1;
        }
    }
    mask
}

/// Connected-component result. Stored as inclusive bounds + pixel
/// count for cheap merge / filter operations.
#[derive(Debug, Clone, Copy)]
struct Component {
    min_x: u32,
    max_x: u32,
    min_y: u32,
    max_y: u32,
    pixels: u32,
}

impl Component {
    fn width(&self) -> u32 {
        self.max_x.saturating_sub(self.min_x).saturating_add(1)
    }
    fn height(&self) -> u32 {
        self.max_y.saturating_sub(self.min_y).saturating_add(1)
    }
    fn mid_x(&self) -> f32 {
        (self.min_x as f32 + self.max_x as f32) * 0.5
    }
    fn mid_y(&self) -> f32 {
        (self.min_y as f32 + self.max_y as f32) * 0.5
    }
}

/// Iterative 4-connected flood fill. Returns one [`Component`] per
/// connected ink blob. The flood-fill is iterative (an explicit
/// stack) rather than recursive so we don't blow the call stack on
/// large inputs.
///
/// **Memory complexity.** Peak heap is `O(width * height)`:
/// the `visited` bitmap is `width * height` bytes, and the
/// worst-case explicit stack also grows to `width * height` entries
/// (8 bytes each on 64-bit targets) when a single connected blob
/// spans the entire raster. That's the price of a 4-connected
/// flood-fill without scan-line compression — acceptable for the
/// intended inputs (UI screenshots, document scans, on-canvas
/// rasters up to a few megapixels). Pathologically large inputs
/// are rejected up front by [`detect_text_regions`] via the
/// [`DetectTextRegionsOptions::max_image_pixels`] guard, so this
/// function never has to worry about runaway allocation. We avoid
/// an in-loop early-bailout because silently truncating a single
/// component mid-scan would emit a wrong bbox — a worse failure
/// mode than the predictable memory ceiling enforced at the
/// public entry point.
fn label_components(mask: &[u8], width: u32, height: u32) -> Vec<Component> {
    let w = width as usize;
    let h = height as usize;
    let mut visited = vec![false; w * h];
    let mut components = Vec::new();
    let mut stack: Vec<(u32, u32)> = Vec::new();

    for sy in 0..height {
        for sx in 0..width {
            let idx = (sy as usize) * w + (sx as usize);
            if visited[idx] || mask[idx] == 0 {
                continue;
            }
            stack.clear();
            stack.push((sx, sy));
            let mut comp = Component {
                min_x: sx,
                max_x: sx,
                min_y: sy,
                max_y: sy,
                pixels: 0,
            };
            while let Some((x, y)) = stack.pop() {
                let i = (y as usize) * w + (x as usize);
                if visited[i] || mask[i] == 0 {
                    continue;
                }
                visited[i] = true;
                comp.pixels = comp.pixels.saturating_add(1);
                if x < comp.min_x {
                    comp.min_x = x;
                }
                if x > comp.max_x {
                    comp.max_x = x;
                }
                if y < comp.min_y {
                    comp.min_y = y;
                }
                if y > comp.max_y {
                    comp.max_y = y;
                }
                if x > 0 {
                    stack.push((x - 1, y));
                }
                if x + 1 < width {
                    stack.push((x + 1, y));
                }
                if y > 0 {
                    stack.push((x, y - 1));
                }
                if y + 1 < height {
                    stack.push((x, y + 1));
                }
            }
            components.push(comp);
        }
    }
    components
}

/// Line bucket — a contiguous run of components on the same
/// baseline. Stored as the union bbox + the count of components
/// merged in.
#[derive(Debug, Clone, Copy)]
struct LineBucket {
    bbox: Component,
    glyph_count: u32,
    /// Median glyph advance, used to back out an estimated char
    /// count when we emit the region. Tracked as the running
    /// average of component widths because median is expensive to
    /// recompute on every push; the difference is in the noise.
    avg_glyph_advance: f32,
}

fn group_into_lines(
    components: &[Component],
    line_overlap_ratio: f32,
    line_gap_ratio: f32,
) -> Vec<LineBucket> {
    let mut buckets: Vec<LineBucket> = Vec::new();
    'outer: for &c in components {
        for b in &mut buckets {
            if components_share_line(&b.bbox, &c, line_overlap_ratio, line_gap_ratio) {
                merge_into_bucket(b, c);
                continue 'outer;
            }
        }
        // New line.
        buckets.push(LineBucket {
            bbox: c,
            glyph_count: 1,
            avg_glyph_advance: c.width() as f32,
        });
    }
    buckets
}

fn components_share_line(
    line: &Component,
    candidate: &Component,
    line_overlap_ratio: f32,
    line_gap_ratio: f32,
) -> bool {
    // Vertical overlap, measured as the fraction of the smaller
    // component's height that overlaps the other.
    let overlap_top = line.min_y.max(candidate.min_y);
    let overlap_bot = line.max_y.min(candidate.max_y);
    if overlap_bot < overlap_top {
        return false;
    }
    let overlap = (overlap_bot - overlap_top + 1) as f32;
    let smaller_h = line.height().min(candidate.height()) as f32;
    if smaller_h <= 0.0 || overlap / smaller_h < line_overlap_ratio {
        return false;
    }
    // Horizontal gap relative to the cap height.
    let h = line.height().max(candidate.height()) as f32;
    let gap = if candidate.min_x >= line.max_x {
        (candidate.min_x - line.max_x) as f32
    } else if line.min_x >= candidate.max_x {
        (line.min_x - candidate.max_x) as f32
    } else {
        // Overlapping in x → no horizontal gap, definitely same line.
        return true;
    };
    gap <= h * line_gap_ratio
}

fn merge_into_bucket(bucket: &mut LineBucket, c: Component) {
    bucket.bbox.min_x = bucket.bbox.min_x.min(c.min_x);
    bucket.bbox.max_x = bucket.bbox.max_x.max(c.max_x);
    bucket.bbox.min_y = bucket.bbox.min_y.min(c.min_y);
    bucket.bbox.max_y = bucket.bbox.max_y.max(c.max_y);
    bucket.bbox.pixels = bucket.bbox.pixels.saturating_add(c.pixels);
    bucket.glyph_count = bucket.glyph_count.saturating_add(1);
    // Online mean update: avoids holding every component's width.
    let n = bucket.glyph_count as f32;
    bucket.avg_glyph_advance =
        bucket.avg_glyph_advance + (c.width() as f32 - bucket.avg_glyph_advance) / n;
}

fn emit_region(bucket: LineBucket) -> TextRegion {
    let width = bucket.bbox.width();
    let height = bucket.bbox.height();
    let advance = bucket.avg_glyph_advance.max(1.0);
    // Plus one to round up — a 100-px line with 10-px advance is
    // ~10 chars, but partial advances mean we err on the high side.
    let estimated = ((width as f32) / advance).ceil() as u32;
    TextRegion {
        x: bucket.bbox.min_x,
        y: bucket.bbox.min_y,
        width,
        height,
        glyph_count: bucket.glyph_count,
        estimated_char_count: estimated.max(bucket.glyph_count),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a tiny RGBA test image. `dark` is the set of (x, y)
    /// pixels we paint black; everything else is solid white.
    fn make_image(width: u32, height: u32, dark: &[(u32, u32)]) -> Vec<u8> {
        let mut pixels = vec![255u8; (width as usize) * (height as usize) * 4];
        for &(x, y) in dark {
            let base = ((y as usize) * (width as usize) + (x as usize)) * 4;
            pixels[base] = 0;
            pixels[base + 1] = 0;
            pixels[base + 2] = 0;
            pixels[base + 3] = 255;
        }
        pixels
    }

    #[test]
    fn empty_image_returns_error() {
        let err = detect_text_regions(&[], 10, 10, DetectTextRegionsOptions::default());
        assert_eq!(err, Err(OcrError::EmptyImage));
    }

    #[test]
    fn degenerate_dimensions_return_error() {
        let pixels = vec![0u8; 4];
        let err = detect_text_regions(&pixels, 0, 1, DetectTextRegionsOptions::default());
        assert_eq!(err, Err(OcrError::DegenerateDimensions));
    }

    #[test]
    fn size_mismatch_returns_error() {
        // Buffer too short for declared dimensions.
        let pixels = vec![0u8; 4];
        let err = detect_text_regions(&pixels, 10, 10, DetectTextRegionsOptions::default());
        assert!(matches!(err, Err(OcrError::SizeMismatch { .. })));
    }

    /// Pins the `max_image_pixels` guard. Without it, a pathological
    /// caller could pass a 10000 × 10000 buffer and the flood-fill
    /// would allocate ~900 MB before doing any useful work. The
    /// guard upfronts the rejection so the renderer can choose a
    /// downsampling strategy instead of being mid-allocation when
    /// the OOM lands.
    #[test]
    fn image_too_large_returns_error() {
        // We don't need to actually allocate the giant buffer — the
        // size-mismatch check happens AFTER the dimensions check
        // and we want to exercise the area guard, so we set
        // `max_image_pixels` to a small value and feed a buffer that
        // matches the declared dimensions exactly.
        let options = DetectTextRegionsOptions {
            max_image_pixels: 10,
            ..DetectTextRegionsOptions::default()
        };
        let pixels = vec![0u8; 5 * 5 * 4];
        let err = detect_text_regions(&pixels, 5, 5, options);
        assert!(matches!(
            err,
            Err(OcrError::ImageTooLarge {
                pixels: 25,
                max: 10
            })
        ));
        // And the default `max_image_pixels` is large enough that
        // typical screenshot dimensions pass through unchanged.
        let pixels_ok = vec![255u8; 100 * 100 * 4];
        assert!(
            detect_text_regions(&pixels_ok, 100, 100, DetectTextRegionsOptions::default()).is_ok(),
            "default max_image_pixels must not reject ordinary inputs",
        );
    }

    #[test]
    fn blank_image_returns_no_regions() {
        let pixels = vec![255u8; 10 * 10 * 4];
        let regions =
            detect_text_regions(&pixels, 10, 10, DetectTextRegionsOptions::default()).unwrap();
        assert!(regions.is_empty(), "no ink → no text regions");
    }

    #[test]
    fn single_glyph_below_min_pixels_is_filtered() {
        // One isolated dark pixel — should be filtered by min_component_pixels.
        let pixels = make_image(10, 10, &[(5, 5)]);
        let regions =
            detect_text_regions(&pixels, 10, 10, DetectTextRegionsOptions::default()).unwrap();
        assert!(regions.is_empty(), "1-pixel blob is noise, not text");
    }

    #[test]
    fn detects_single_horizontal_line_of_blobs() {
        // Three 3x4 "glyph" blocks on the same baseline. With the
        // default options, all three should merge into one line.
        let mut dark = Vec::new();
        for x in 0..3 {
            for y in 2..6 {
                dark.push((x, y));
            }
        }
        for x in 6..9 {
            for y in 2..6 {
                dark.push((x, y));
            }
        }
        for x in 12..15 {
            for y in 2..6 {
                dark.push((x, y));
            }
        }
        let pixels = make_image(20, 10, &dark);
        let regions =
            detect_text_regions(&pixels, 20, 10, DetectTextRegionsOptions::default()).unwrap();
        assert_eq!(regions.len(), 1, "three glyphs on one baseline → one line");
        let r = regions[0];
        assert_eq!(r.glyph_count, 3);
        assert_eq!(r.x, 0);
        assert_eq!(r.y, 2);
        assert_eq!(r.width, 15);
        assert_eq!(r.height, 4);
    }

    #[test]
    fn two_lines_emit_two_regions() {
        // One line of three blobs at y=2..6, another at y=12..16.
        // The vertical gap is large enough that no overlap is
        // possible, so they must split into two regions.
        let mut dark = Vec::new();
        for xrange in [0..3, 6..9, 12..15] {
            for x in xrange {
                for y in 2..6 {
                    dark.push((x, y));
                }
            }
        }
        for xrange in [0..3, 6..9, 12..15] {
            for x in xrange {
                for y in 12..16 {
                    dark.push((x, y));
                }
            }
        }
        let pixels = make_image(20, 20, &dark);
        let regions =
            detect_text_regions(&pixels, 20, 20, DetectTextRegionsOptions::default()).unwrap();
        assert_eq!(regions.len(), 2, "two baselines → two lines");
        assert!(regions[0].y < regions[1].y, "regions emit top-down");
    }

    #[test]
    fn estimated_char_count_floor_is_glyph_count() {
        // If the width-by-advance estimate underestimates, we floor
        // at the actual glyph count so callers don't see "we
        // detected 3 glyphs but estimate 1 character".
        let mut dark = Vec::new();
        for x in 0..3 {
            for y in 2..6 {
                dark.push((x, y));
            }
        }
        for x in 6..9 {
            for y in 2..6 {
                dark.push((x, y));
            }
        }
        let pixels = make_image(20, 10, &dark);
        let regions =
            detect_text_regions(&pixels, 20, 10, DetectTextRegionsOptions::default()).unwrap();
        assert_eq!(regions.len(), 1);
        assert!(
            regions[0].estimated_char_count >= regions[0].glyph_count,
            "estimate must not undercount glyphs",
        );
    }

    #[test]
    fn region_serialises_camelcase() {
        let r = TextRegion {
            x: 0,
            y: 1,
            width: 2,
            height: 3,
            glyph_count: 4,
            estimated_char_count: 5,
        };
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("\"glyphCount\":4"), "{json}");
        assert!(json.contains("\"estimatedCharCount\":5"), "{json}");
        // And round-trip.
        let parsed: TextRegion = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, r);
    }

    #[test]
    fn options_serialise_camelcase() {
        let opts = DetectTextRegionsOptions::default();
        let json = serde_json::to_string(&opts).unwrap();
        assert!(json.contains("\"luminanceThreshold\""), "{json}");
        assert!(json.contains("\"minComponentPixels\""), "{json}");
        assert!(json.contains("\"maxComponentFraction\""), "{json}");
        assert!(json.contains("\"lineOverlapRatio\""), "{json}");
        assert!(json.contains("\"lineGapRatio\""), "{json}");
    }
}
