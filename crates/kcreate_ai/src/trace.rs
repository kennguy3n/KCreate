//! Raster-to-vector tracing.
//!
//! Phase 9 (Task 12): "Trace this raster" action. Takes an RGBA8
//! image, extracts edge contours, and returns a list of polylines
//! suitable for converting into KCreate vector paths.
//!
//! Pipeline:
//!
//! 1. RGBA → grayscale luminance (Rec. 601 weights).
//! 2. Optional Gaussian smoothing (3×3 box approx) to suppress
//!    JPEG ringing.
//! 3. Threshold to a binary mask. The threshold is configurable;
//!    `auto` runs Otsu's method on the grayscale histogram.
//! 4. Marching squares to extract iso-contours along the
//!    threshold boundary, deduplicating each contour exactly once.
//! 5. Ramer-Douglas-Peucker simplification to drop redundant
//!    polyline points within `simplify_tolerance` pixels.
//!
//! The output is a `Vec<TracedPath>` — each path is a closed or open
//! polyline in image-pixel coordinates. The bridge layer turns each
//! path into a `kcreate_core::node::Node` of type `VectorPath`.

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TraceError {
    #[error("invalid image dimensions {width}x{height} for buffer of {len} bytes (need {expected} bytes)")]
    InvalidDimensions {
        width: u32,
        height: u32,
        len: usize,
        expected: usize,
    },
    #[error("image dimensions must be at least 2x2; got {width}x{height}")]
    TooSmall { width: u32, height: u32 },
    #[error("simplify tolerance must be non-negative, got {0}")]
    InvalidTolerance(f32),
    #[error("threshold must be in [0, 255], got {0}")]
    InvalidThreshold(i32),
}

/// Threshold strategy for binarising the grayscale image.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum TraceThreshold {
    /// Auto-pick using Otsu's between-class-variance maximiser.
    #[default]
    Auto,
    /// Hard threshold in `[0, 255]`. Pixels with luminance >=
    /// `value` are foreground.
    Fixed { value: u8 },
}

/// Tracing options.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceOptions {
    /// Threshold strategy.
    pub threshold: TraceThreshold,
    /// Ramer-Douglas-Peucker simplification tolerance in pixels.
    /// `0.0` disables simplification entirely; larger values
    /// produce sparser paths.
    pub simplify_tolerance: f32,
    /// Drop contours with fewer than this many points (after
    /// simplification). Filters out single-pixel noise.
    pub min_path_points: u32,
    /// Apply a single 3×3 box-blur pass before thresholding. Helps
    /// when the source has JPEG ringing or speckle noise.
    pub smooth: bool,
}

impl Default for TraceOptions {
    fn default() -> Self {
        Self {
            threshold: TraceThreshold::default(),
            simplify_tolerance: 1.0,
            min_path_points: 4,
            smooth: true,
        }
    }
}

/// A single traced contour.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(clippy::derive_partial_eq_without_eq)]
pub struct TracedPath {
    /// Polyline points in image-pixel coordinates. The last point
    /// is duplicated for closed paths (so `points.first() ==
    /// points.last()`).
    pub points: Vec<TracedPoint>,
    /// Whether the contour is closed (its start equals its end).
    pub closed: bool,
}

/// One vertex on a traced contour.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[allow(clippy::derive_partial_eq_without_eq)]
pub struct TracedPoint {
    pub x: f32,
    pub y: f32,
}

/// Trace `rgba` (4 bytes per pixel) and return the contours
/// extracted at the threshold boundary.
pub fn trace_raster(
    rgba: &[u8],
    width: u32,
    height: u32,
    opts: &TraceOptions,
) -> Result<Vec<TracedPath>, TraceError> {
    if width < 2 || height < 2 {
        return Err(TraceError::TooSmall { width, height });
    }
    let expected = (width as usize) * (height as usize) * 4;
    if rgba.len() != expected {
        return Err(TraceError::InvalidDimensions {
            width,
            height,
            len: rgba.len(),
            expected,
        });
    }
    if opts.simplify_tolerance < 0.0 || !opts.simplify_tolerance.is_finite() {
        return Err(TraceError::InvalidTolerance(opts.simplify_tolerance));
    }
    if let TraceThreshold::Fixed { value } = opts.threshold {
        // u8 cannot be out of range, but mirror the wire-format
        // contract: the bridge accepts a signed i32 because that is
        // what N-API hands us.
        let _ = value;
    }

    let mut gray = rgba_to_grayscale(rgba, width, height);
    if opts.smooth {
        box_blur_3x3(&mut gray, width, height);
    }
    let threshold = match opts.threshold {
        TraceThreshold::Auto => otsu_threshold(&gray),
        TraceThreshold::Fixed { value } => value,
    };
    let mask = threshold_to_mask(&gray, threshold);
    let raw = moore_neighbor_trace(&mask, width, height);
    let mut out = Vec::with_capacity(raw.len());
    for path in raw {
        let simplified = if opts.simplify_tolerance > 0.0 {
            rdp_simplify(&path.points, opts.simplify_tolerance)
        } else {
            path.points
        };
        if simplified.len() < opts.min_path_points as usize {
            continue;
        }
        out.push(TracedPath {
            points: simplified,
            closed: path.closed,
        });
    }
    Ok(out)
}

/// Convert RGBA8 to grayscale luminance via the Rec. 601 weights.
fn rgba_to_grayscale(rgba: &[u8], width: u32, height: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((width as usize) * (height as usize));
    let _ = (width, height);
    for chunk in rgba.chunks_exact(4) {
        let r = f32::from(chunk[0]);
        let g = f32::from(chunk[1]);
        let b = f32::from(chunk[2]);
        let a = f32::from(chunk[3]) / 255.0;
        // Composite over a white background so transparent pixels
        // count as "foreground free".
        let lum = (0.299 * r + 0.587 * g + 0.114 * b).mul_add(a, 255.0 * (1.0 - a));
        out.push(lum.round().clamp(0.0, 255.0) as u8);
    }
    out
}

/// In-place 3×3 box-blur. Edges use clamp-to-border sampling so the
/// output is the same size as the input.
fn box_blur_3x3(buf: &mut [u8], width: u32, height: u32) {
    let w = width as usize;
    let h = height as usize;
    let src = buf.to_vec();
    for y in 0..h {
        for x in 0..w {
            let mut sum: u32 = 0;
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    let sx = (x as i32 + dx).clamp(0, w as i32 - 1) as usize;
                    let sy = (y as i32 + dy).clamp(0, h as i32 - 1) as usize;
                    sum += u32::from(src[sy * w + sx]);
                }
            }
            buf[y * w + x] = (sum / 9) as u8;
        }
    }
}

/// Otsu's between-class-variance maximisation. Returns the threshold
/// in `[0, 255]` that best separates the histogram into two classes.
fn otsu_threshold(gray: &[u8]) -> u8 {
    let mut hist = [0u32; 256];
    for &g in gray {
        hist[g as usize] += 1;
    }
    let total = gray.len() as f64;
    let mut sum_total: f64 = 0.0;
    for (i, &h) in hist.iter().enumerate() {
        sum_total += (i as f64) * f64::from(h);
    }
    let mut sum_bg = 0.0;
    let mut weight_bg = 0.0;
    let mut max_var = 0.0;
    let mut best = 0u8;
    for (t, &h) in hist.iter().enumerate() {
        weight_bg += f64::from(h);
        if weight_bg <= 0.0 {
            continue;
        }
        let weight_fg = total - weight_bg;
        if weight_fg <= 0.0 {
            break;
        }
        sum_bg += (t as f64) * f64::from(h);
        let mean_bg = sum_bg / weight_bg;
        let mean_fg = (sum_total - sum_bg) / weight_fg;
        let var_between = weight_bg * weight_fg * (mean_bg - mean_fg).powi(2);
        if var_between > max_var {
            max_var = var_between;
            best = t as u8;
        }
    }
    best
}

/// Map grayscale → binary foreground mask (1 = foreground / dark
/// regions / under threshold, 0 = background). Foreground is the
/// minority class in most design assets (logo on white), so we
/// flag pixels with luminance < threshold as "1".
fn threshold_to_mask(gray: &[u8], threshold: u8) -> Vec<u8> {
    // Otsu's convention: pixels with value <= t belong to the
    // "below-threshold" class. Darker pixels are foreground
    // (logos / icons on light backgrounds), so we map that class
    // to `1`.
    gray.iter().map(|&g| u8::from(g <= threshold)).collect()
}

#[derive(Debug, Clone)]
struct RawPath {
    points: Vec<TracedPoint>,
    closed: bool,
}

/// Standard 8-neighbour offsets in clockwise order starting from
/// "east". The Moore-neighbor boundary trace rotates an index
/// modulo 8 against this table.
const MOORE_OFFSETS: [(i32, i32); 8] = [
    (1, 0),   // 0: E
    (1, 1),   // 1: SE
    (0, 1),   // 2: S
    (-1, 1),  // 3: SW
    (-1, 0),  // 4: W
    (-1, -1), // 5: NW
    (0, -1),  // 6: N
    (1, -1),  // 7: NE
];

/// Moore-neighbor contour tracing (a.k.a. "radial sweep").
///
/// Given a binary foreground mask, find every connected component
/// and walk its outer boundary clockwise. We return one
/// `TracedPath` per component, with points placed at integer pixel
/// coordinates (so paths are easy to verify in unit tests).
///
/// References: Pavlidis (1982), §7.5. The standard textbook
/// algorithm — finite, deterministic, and bounded by O(perimeter)
/// per component. We additionally tag each foreground pixel we
/// reach as visited so a second component starting near the
/// first is detected only once.
fn moore_neighbor_trace(mask: &[u8], width: u32, height: u32) -> Vec<RawPath> {
    let w = width as usize;
    let h = height as usize;
    let mut visited = vec![false; w * h];
    let mut out = Vec::new();

    let is_fg = |x: i32, y: i32| -> bool {
        if x < 0 || y < 0 || (x as usize) >= w || (y as usize) >= h {
            return false;
        }
        mask[(y as usize) * w + (x as usize)] != 0
    };

    // Sweep top-to-bottom, left-to-right looking for the first
    // foreground pixel that has a background pixel directly above
    // it — i.e. a boundary start point of a new component.
    for y in 0..h {
        for x in 0..w {
            if visited[y * w + x] {
                continue;
            }
            if !is_fg(x as i32, y as i32) {
                continue;
            }
            // The boundary-start condition: previous row at the
            // same x must be background (or out-of-bounds).
            if y > 0 && is_fg(x as i32, y as i32 - 1) {
                // This pixel is interior to a component we'll find
                // (or have found) via its top boundary — skip.
                continue;
            }
            // Walk the boundary clockwise.
            let start_x = x as i32;
            let start_y = y as i32;
            let mut cx = start_x;
            let mut cy = start_y;
            // Entry direction into the start pixel from "outside"
            // is "from above" → next sweep starts looking east.
            let mut dir: i32 = 6; // N
            let mut path = Vec::new();
            path.push(TracedPoint {
                x: cx as f32,
                y: cy as f32,
            });
            visited[(cy as usize) * w + (cx as usize)] = true;

            // Limit iterations to a multiple of the pixel count so
            // pathological inputs can't loop forever.
            let max_steps = (w * h * 8) + 8;
            let mut steps = 0;
            loop {
                // The next direction to scan starts one step
                // counter-clockwise of the direction we came in
                // from (i.e. "look just to the left of where the
                // background was").
                let scan_start = (dir + 2).rem_euclid(8);
                let mut found = false;
                for i in 0..8 {
                    let probe = (scan_start + i).rem_euclid(8) as usize;
                    let (dx, dy) = MOORE_OFFSETS[probe];
                    let nx = cx + dx;
                    let ny = cy + dy;
                    if is_fg(nx, ny) {
                        // Found the next boundary pixel.
                        dir = probe as i32;
                        cx = nx;
                        cy = ny;
                        if !visited[(cy as usize) * w + (cx as usize)] {
                            visited[(cy as usize) * w + (cx as usize)] = true;
                        }
                        path.push(TracedPoint {
                            x: cx as f32,
                            y: cy as f32,
                        });
                        found = true;
                        break;
                    }
                }
                if !found {
                    // Isolated pixel.
                    break;
                }
                steps += 1;
                if cx == start_x && cy == start_y {
                    break;
                }
                if steps >= max_steps {
                    break;
                }
            }

            // Components of a single pixel just produce one point.
            if path.len() < 2 {
                continue;
            }
            // Determine closure: the last point equals the start.
            let closed = {
                let first = path[0];
                let last = *path.last().unwrap();
                (first.x - last.x).abs() < 0.5 && (first.y - last.y).abs() < 0.5
            };
            out.push(RawPath {
                points: path,
                closed,
            });
        }
    }
    out
}

/// Ramer-Douglas-Peucker polyline simplification.
fn rdp_simplify(points: &[TracedPoint], epsilon: f32) -> Vec<TracedPoint> {
    if points.len() < 3 {
        return points.to_vec();
    }
    let mut keep = vec![false; points.len()];
    keep[0] = true;
    *keep.last_mut().unwrap() = true;
    rdp_recurse(points, 0, points.len() - 1, epsilon, &mut keep);
    points
        .iter()
        .zip(keep.iter())
        .filter_map(|(p, &k)| k.then_some(*p))
        .collect()
}

fn rdp_recurse(points: &[TracedPoint], lo: usize, hi: usize, epsilon: f32, keep: &mut [bool]) {
    if hi <= lo + 1 {
        return;
    }
    let (a, b) = (points[lo], points[hi]);
    let mut max_dist = 0.0_f32;
    let mut max_idx = lo;
    for (i, p) in points.iter().enumerate().take(hi).skip(lo + 1) {
        let d = perpendicular_distance(*p, a, b);
        if d > max_dist {
            max_dist = d;
            max_idx = i;
        }
    }
    if max_dist > epsilon {
        keep[max_idx] = true;
        rdp_recurse(points, lo, max_idx, epsilon, keep);
        rdp_recurse(points, max_idx, hi, epsilon, keep);
    }
}

fn perpendicular_distance(p: TracedPoint, a: TracedPoint, b: TracedPoint) -> f32 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let len_sq = dx.mul_add(dx, dy * dy);
    if len_sq <= f32::EPSILON {
        let ddx = p.x - a.x;
        let ddy = p.y - a.y;
        return ddx.hypot(ddy);
    }
    let num = ((p.x - a.x) * dy - (p.y - a.y) * dx).abs();
    num / len_sq.sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid_rect_rgba(width: u32, height: u32, rect: (u32, u32, u32, u32)) -> Vec<u8> {
        let mut buf = vec![255u8; (width as usize) * (height as usize) * 4];
        // make background white
        for i in (0..buf.len()).step_by(4) {
            buf[i] = 255;
            buf[i + 1] = 255;
            buf[i + 2] = 255;
            buf[i + 3] = 255;
        }
        let (rx, ry, rw, rh) = rect;
        for y in ry..(ry + rh).min(height) {
            for x in rx..(rx + rw).min(width) {
                let i = ((y * width + x) * 4) as usize;
                buf[i] = 0;
                buf[i + 1] = 0;
                buf[i + 2] = 0;
                buf[i + 3] = 255;
            }
        }
        buf
    }

    #[test]
    fn rejects_too_small() {
        let opts = TraceOptions::default();
        let err = trace_raster(&[0u8; 4], 1, 1, &opts).unwrap_err();
        assert!(matches!(err, TraceError::TooSmall { .. }));
    }

    #[test]
    fn rejects_wrong_buffer_size() {
        let opts = TraceOptions::default();
        let err = trace_raster(&[0u8; 16], 4, 4, &opts).unwrap_err();
        assert!(matches!(err, TraceError::InvalidDimensions { .. }));
    }

    #[test]
    fn rejects_negative_tolerance() {
        let opts = TraceOptions {
            simplify_tolerance: -1.0,
            ..TraceOptions::default()
        };
        let buf = vec![0u8; 4 * 4 * 4];
        let err = trace_raster(&buf, 4, 4, &opts).unwrap_err();
        assert!(matches!(err, TraceError::InvalidTolerance(_)));
    }

    #[test]
    fn traces_solid_rectangle() {
        // Solid black 10x10 rectangle centred in a 32x32 white
        // canvas → one closed contour.
        let buf = solid_rect_rgba(32, 32, (10, 10, 12, 12));
        let opts = TraceOptions {
            simplify_tolerance: 0.5,
            min_path_points: 4,
            ..TraceOptions::default()
        };
        let paths = trace_raster(&buf, 32, 32, &opts).unwrap();
        assert!(!paths.is_empty(), "must produce at least one contour");
        let total_points: usize = paths.iter().map(|p| p.points.len()).sum();
        assert!(
            total_points >= 4,
            "rectangle contour must have at least 4 vertices (got {total_points})"
        );
        assert!(
            paths.iter().any(|p| p.closed),
            "at least one path must be closed"
        );
    }

    #[test]
    fn rdp_drops_collinear_points() {
        let line = vec![
            TracedPoint { x: 0.0, y: 0.0 },
            TracedPoint { x: 1.0, y: 0.0 },
            TracedPoint { x: 2.0, y: 0.0 },
            TracedPoint { x: 3.0, y: 0.0 },
            TracedPoint { x: 4.0, y: 0.0 },
        ];
        let simplified = rdp_simplify(&line, 0.01);
        assert_eq!(
            simplified.len(),
            2,
            "RDP must collapse collinear points to endpoints"
        );
    }

    #[test]
    fn otsu_finds_bimodal_threshold() {
        // 50 dark + 50 bright pixels → optimal threshold separates
        // the two classes. Otsu returns the value such that pixels
        // with value <= t are class A, > t are class B, so `t` can
        // be any value in `[10, 239]`. The mask helper uses `<= t`
        // so the dark cluster lands in the foreground class.
        let mut gray = vec![10u8; 50];
        gray.extend(vec![240u8; 50]);
        let t = otsu_threshold(&gray);
        assert!(
            (10..240).contains(&t),
            "Otsu must split the histogram between the two clusters (got {t})"
        );
        let mask = threshold_to_mask(&gray, t);
        // First 50 pixels are dark (foreground), next 50 bright (bg).
        assert!(mask[..50].iter().all(|&x| x == 1));
        assert!(mask[50..].iter().all(|&x| x == 0));
    }

    #[test]
    fn rejects_nan_tolerance() {
        let opts = TraceOptions {
            simplify_tolerance: f32::NAN,
            ..TraceOptions::default()
        };
        let buf = vec![0u8; 4 * 4 * 4];
        let err = trace_raster(&buf, 4, 4, &opts).unwrap_err();
        assert!(matches!(err, TraceError::InvalidTolerance(_)));
    }
}
