//! AI "icon-ify" action — vector simplification + grid normalisation.
//!
//! Phase 9 (Task 19). Given a collection of vector polylines from a
//! user's selection, produce a cleaned-up version suitable for use
//! as an icon at the requested grid size (24×24, 48×48, …).
//!
//! Algorithm (real, deterministic — no AI inference required):
//!
//! 1. Compute the union bounding box of every input polyline.
//! 2. Uniformly scale every point into a square of side
//!    `grid_size`, preserving the source aspect ratio (letterboxed
//!    inside the square).
//! 3. Round every point to the nearest half-pixel on the icon
//!    grid. This is what gives icons crisp 1-pixel strokes when
//!    rendered at 1× or 2×.
//! 4. Run Ramer-Douglas-Peucker with a tolerance proportional to
//!    the grid size to drop redundant points.
//! 5. Drop microscopic paths (length < 1.5 px on the icon grid).
//!
//! Returns the simplified paths together with their target stroke
//! width — a recommended value derived from the grid size that
//! callers can use when creating the new vector node.

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum IconifyError {
    #[error("grid_size must be in [8, 1024], got {0}")]
    InvalidGridSize(u32),
    #[error("no input paths provided")]
    EmptyInput,
    #[error("input contains no finite coordinates")]
    NoFinitePoints,
}

/// 2D point used by the iconify pipeline. Plain `f32` to keep the
/// public API independent of `kcreate_core` (which has its own
/// `Point2D`). The bridge converts in both directions.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[allow(clippy::derive_partial_eq_without_eq)]
pub struct IconPoint {
    pub x: f32,
    pub y: f32,
}

/// One polyline (open or closed).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(clippy::derive_partial_eq_without_eq)]
pub struct IconPath {
    pub points: Vec<IconPoint>,
    pub closed: bool,
}

/// Iconify options.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IconifyOptions {
    /// Side length of the square icon grid in pixels. Standard
    /// values: 16, 20, 24, 32, 48, 64.
    pub grid_size: u32,
    /// Padding around the icon inside the grid, in grid pixels.
    /// `0` packs the icon to the edge; `1` is a comfortable
    /// margin for 24px icons.
    pub padding: f32,
    /// Drop paths whose total polyline length (after rescale)
    /// falls below this many grid pixels.
    pub min_length_px: f32,
    /// RDP simplification tolerance as a fraction of the grid
    /// size. `0.02` is conservative; `0.05` is aggressive.
    pub simplify_fraction: f32,
}

impl Default for IconifyOptions {
    fn default() -> Self {
        Self {
            grid_size: 24,
            padding: 1.0,
            min_length_px: 1.5,
            simplify_fraction: 0.02,
        }
    }
}

/// Result of iconifying a selection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(clippy::derive_partial_eq_without_eq)]
pub struct IconifyResult {
    pub paths: Vec<IconPath>,
    /// Recommended stroke width in grid pixels — `1.0` for 16/20px,
    /// `1.5` for 24px, `2.0` for >= 32px. Mirrors the Lucide /
    /// Material icon-set conventions.
    pub recommended_stroke_width: f32,
    /// Side length of the target grid.
    pub grid_size: u32,
}

/// Iconify `paths` according to `opts`. The input is moved through
/// the simplify + rescale + snap pipeline; the output is a
/// drop-in-ready set of paths plus a recommended stroke width.
pub fn iconify(paths: &[IconPath], opts: &IconifyOptions) -> Result<IconifyResult, IconifyError> {
    if !(8..=1024).contains(&opts.grid_size) {
        return Err(IconifyError::InvalidGridSize(opts.grid_size));
    }
    if paths.is_empty() {
        return Err(IconifyError::EmptyInput);
    }
    let bbox = union_bbox(paths).ok_or(IconifyError::NoFinitePoints)?;
    let grid = opts.grid_size as f32;
    // Aspect-preserving fit into (grid - 2*padding).
    let usable = (grid - 2.0 * opts.padding).max(1.0);
    let span = (bbox.w.max(bbox.h)).max(f32::EPSILON);
    let scale = usable / span;
    let ox = opts.padding + (usable - bbox.w * scale) * 0.5;
    let oy = opts.padding + (usable - bbox.h * scale) * 0.5;
    let tol = opts.simplify_fraction * grid;

    let mut out_paths = Vec::with_capacity(paths.len());
    for path in paths {
        // 1. Rescale + translate into the icon grid.
        let mut pts: Vec<IconPoint> = path
            .points
            .iter()
            .map(|p| IconPoint {
                x: (p.x - bbox.x) * scale + ox,
                y: (p.y - bbox.y) * scale + oy,
            })
            .collect();
        // 2. Snap to half-pixel grid so 1-px strokes render crisply.
        for p in &mut pts {
            p.x = (p.x * 2.0).round() / 2.0;
            p.y = (p.y * 2.0).round() / 2.0;
        }
        // 3. RDP simplify.
        let simplified = rdp_simplify(&pts, tol);
        // 4. Length filter.
        if polyline_length(&simplified) < opts.min_length_px {
            continue;
        }
        out_paths.push(IconPath {
            points: simplified,
            closed: path.closed,
        });
    }

    let stroke_width = match opts.grid_size {
        0..=20 => 1.0,
        21..=27 => 1.5,
        _ => 2.0,
    };
    Ok(IconifyResult {
        paths: out_paths,
        recommended_stroke_width: stroke_width,
        grid_size: opts.grid_size,
    })
}

#[derive(Debug, Clone, Copy)]
struct BBox {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

fn union_bbox(paths: &[IconPath]) -> Option<BBox> {
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for path in paths {
        for p in &path.points {
            if !p.x.is_finite() || !p.y.is_finite() {
                continue;
            }
            if p.x < min_x {
                min_x = p.x;
            }
            if p.y < min_y {
                min_y = p.y;
            }
            if p.x > max_x {
                max_x = p.x;
            }
            if p.y > max_y {
                max_y = p.y;
            }
        }
    }
    if !min_x.is_finite() || !max_x.is_finite() {
        return None;
    }
    Some(BBox {
        x: min_x,
        y: min_y,
        w: (max_x - min_x).max(0.0),
        h: (max_y - min_y).max(0.0),
    })
}

fn polyline_length(points: &[IconPoint]) -> f32 {
    if points.len() < 2 {
        return 0.0;
    }
    let mut total = 0.0;
    for w in points.windows(2) {
        let dx = w[1].x - w[0].x;
        let dy = w[1].y - w[0].y;
        total += dx.hypot(dy);
    }
    total
}

fn rdp_simplify(points: &[IconPoint], epsilon: f32) -> Vec<IconPoint> {
    if points.len() < 3 || epsilon <= 0.0 {
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

fn rdp_recurse(points: &[IconPoint], lo: usize, hi: usize, epsilon: f32, keep: &mut [bool]) {
    if hi <= lo + 1 {
        return;
    }
    let a = points[lo];
    let b = points[hi];
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

fn perpendicular_distance(p: IconPoint, a: IconPoint, b: IconPoint) -> f32 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let len_sq = dx.mul_add(dx, dy * dy);
    if len_sq <= f32::EPSILON {
        return (p.x - a.x).hypot(p.y - a.y);
    }
    let num = ((p.x - a.x) * dy - (p.y - a.y) * dx).abs();
    num / len_sq.sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_grid() {
        let path = IconPath {
            points: vec![IconPoint { x: 0.0, y: 0.0 }, IconPoint { x: 10.0, y: 10.0 }],
            closed: false,
        };
        let err = iconify(
            &[path],
            &IconifyOptions {
                grid_size: 4,
                ..IconifyOptions::default()
            },
        )
        .unwrap_err();
        assert!(matches!(err, IconifyError::InvalidGridSize(4)));
    }

    #[test]
    fn rejects_empty_input() {
        let err = iconify(&[], &IconifyOptions::default()).unwrap_err();
        assert!(matches!(err, IconifyError::EmptyInput));
    }

    #[test]
    fn rejects_only_nans() {
        let path = IconPath {
            points: vec![
                IconPoint {
                    x: f32::NAN,
                    y: f32::NAN,
                },
                IconPoint {
                    x: f32::INFINITY,
                    y: 0.0,
                },
            ],
            closed: false,
        };
        let err = iconify(&[path], &IconifyOptions::default()).unwrap_err();
        assert!(matches!(err, IconifyError::NoFinitePoints));
    }

    #[test]
    fn rescales_into_grid() {
        // 100x100 square → 24x24 icon, expect points within grid.
        let path = IconPath {
            points: vec![
                IconPoint { x: 0.0, y: 0.0 },
                IconPoint { x: 100.0, y: 0.0 },
                IconPoint { x: 100.0, y: 100.0 },
                IconPoint { x: 0.0, y: 100.0 },
                IconPoint { x: 0.0, y: 0.0 },
            ],
            closed: true,
        };
        let result = iconify(
            &[path],
            &IconifyOptions {
                grid_size: 24,
                padding: 1.0,
                ..IconifyOptions::default()
            },
        )
        .unwrap();
        assert_eq!(result.grid_size, 24);
        assert!((result.recommended_stroke_width - 1.5).abs() < f32::EPSILON);
        for p in &result.paths[0].points {
            assert!(p.x >= 0.0 && p.x <= 24.0, "x={} outside grid", p.x);
            assert!(p.y >= 0.0 && p.y <= 24.0, "y={} outside grid", p.y);
        }
    }

    #[test]
    fn drops_tiny_paths() {
        // Two paths in one selection: a big one (spans the bbox)
        // and a tiny one (microscopic relative to the bbox). The
        // big one defines the bbox so the tiny one survives at
        // sub-pixel scale post-rescale → must be dropped.
        let big = IconPath {
            points: vec![
                IconPoint { x: 0.0, y: 0.0 },
                IconPoint { x: 1000.0, y: 0.0 },
                IconPoint {
                    x: 1000.0,
                    y: 1000.0,
                },
                IconPoint { x: 0.0, y: 1000.0 },
                IconPoint { x: 0.0, y: 0.0 },
            ],
            closed: true,
        };
        let tiny = IconPath {
            points: vec![
                IconPoint { x: 500.0, y: 500.0 },
                IconPoint {
                    x: 500.01,
                    y: 500.0,
                },
            ],
            closed: false,
        };
        let result = iconify(
            &[big, tiny],
            &IconifyOptions {
                grid_size: 24,
                padding: 0.5,
                min_length_px: 2.0,
                ..IconifyOptions::default()
            },
        )
        .unwrap();
        // Big square survives; tiny segment is below the min.
        assert_eq!(
            result.paths.len(),
            1,
            "tiny paths must be dropped (got {} paths)",
            result.paths.len()
        );
    }

    #[test]
    fn stroke_width_scales_with_grid() {
        let path = IconPath {
            points: vec![IconPoint { x: 0.0, y: 0.0 }, IconPoint { x: 1.0, y: 0.0 }],
            closed: false,
        };
        for (grid, expected) in [(16u32, 1.0_f32), (24, 1.5), (48, 2.0)] {
            let result = iconify(
                std::slice::from_ref(&path),
                &IconifyOptions {
                    grid_size: grid,
                    padding: 0.5,
                    min_length_px: 0.0,
                    ..IconifyOptions::default()
                },
            )
            .unwrap();
            assert!((result.recommended_stroke_width - expected).abs() < f32::EPSILON);
        }
    }
}
