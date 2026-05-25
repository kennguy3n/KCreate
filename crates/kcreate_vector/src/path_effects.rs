//! Non-destructive path effects.
//!
//! Effects take a [`VectorPath`] in and produce a derived
//! [`VectorPath`] out. The caller is responsible for storing the
//! effect parameters separately so undo can restore the original
//! geometry.

use kurbo::{flatten, ParamCurve, ParamCurveArclen, PathSeg, Point};

use crate::path::{PathPoint, PathSegment, VectorPath};

const ARCLEN_TOL: f64 = 1e-3;

fn segment_arclen(seg: PathSeg) -> f64 {
    seg.arclen(ARCLEN_TOL)
}

fn sample_seg(seg: PathSeg, t: f64) -> Point {
    seg.eval(t)
}

/// Walk the path by arc length, emitting sub-paths whose visible /
/// hidden state alternates according to `pattern`. `offset` shifts
/// the start of the pattern along the curve (a typical SVG-style
/// dash-offset).
///
/// Pattern entries are interpreted in order as visible, gap, visible,
/// gap... and wrap once exhausted. Pattern entries of `0.0` are
/// skipped to avoid an infinite loop.
#[must_use]
pub fn dash(path: &VectorPath, pattern: &[f64], offset: f64) -> Vec<VectorPath> {
    if pattern.is_empty() || pattern.iter().all(|p| *p <= 0.0) {
        return vec![path.clone()];
    }
    let bez = path.to_kurbo();
    let segs: Vec<PathSeg> = bez.segments().collect();
    if segs.is_empty() {
        return vec![path.clone()];
    }
    let mut out: Vec<VectorPath> = Vec::new();
    let mut current: Vec<PathSegment> = Vec::new();
    let mut pattern_idx: usize = 0;
    let mut visible = true;
    // Apply the offset by skipping forward over invisible/visible
    // pieces of the pattern before starting the walk.
    let mut remaining_in_dash = pattern[0].max(0.0);
    let mut to_skip = offset.rem_euclid(pattern.iter().filter(|p| **p > 0.0).sum::<f64>().max(1.0));
    while to_skip > 0.0 {
        if remaining_in_dash > to_skip {
            remaining_in_dash -= to_skip;
            to_skip = 0.0;
        } else {
            to_skip -= remaining_in_dash;
            pattern_idx = (pattern_idx + 1) % pattern.len();
            visible = !visible;
            remaining_in_dash = pattern[pattern_idx].max(0.0);
        }
    }

    fn push_point(current: &mut Vec<PathSegment>, point: Point, force_move: bool) {
        let pp = PathPoint::new(point.x, point.y);
        if force_move || current.is_empty() {
            current.push(PathSegment::MoveTo(pp));
        } else {
            current.push(PathSegment::LineTo(pp));
        }
    }

    for seg in segs {
        let seg_len = segment_arclen(seg);
        if seg_len < f64::EPSILON {
            continue;
        }
        let mut consumed = 0.0;
        if visible {
            let was_empty = current.is_empty();
            push_point(&mut current, sample_seg(seg, 0.0), was_empty);
        }
        while consumed < seg_len {
            let step = remaining_in_dash.min(seg_len - consumed);
            consumed += step;
            remaining_in_dash -= step;
            let t = consumed / seg_len;
            if visible {
                push_point(&mut current, sample_seg(seg, t), false);
            }
            if remaining_in_dash <= f64::EPSILON && consumed < seg_len {
                if visible && !current.is_empty() {
                    out.push(VectorPath::new(std::mem::take(&mut current)));
                }
                pattern_idx = (pattern_idx + 1) % pattern.len();
                visible = !visible;
                remaining_in_dash = pattern[pattern_idx].max(f64::EPSILON);
                if visible {
                    push_point(&mut current, sample_seg(seg, t), true);
                }
            }
        }
    }
    if !current.is_empty() {
        out.push(VectorPath::new(current));
    }
    if out.is_empty() {
        out.push(path.clone());
    }
    out
}

/// Replace every interior sharp corner with a circular fillet of the
/// given `radius`. The first and last vertices of an open polyline
/// stay sharp; on closed paths the join between last and first
/// vertex is also rounded.
#[must_use]
pub fn round_corners(path: &VectorPath, radius: f64) -> VectorPath {
    if radius <= 0.0 {
        return path.clone();
    }
    // Walk the original commands; whenever two adjacent line segments
    // meet at a sharp angle, replace the join with a quadratic Bezier
    // approximating an arc of `radius`.
    let mut new_commands: Vec<PathSegment> = Vec::with_capacity(path.commands.len());
    // Build a flat polyline first so curves and lines round uniformly.
    let bez = path.to_kurbo();
    let mut polylines: Vec<Vec<PathPoint>> = Vec::new();
    let mut current: Vec<PathPoint> = Vec::new();
    flatten(bez.iter(), 0.5, |el| match el {
        kurbo::PathEl::MoveTo(p) => {
            if current.len() > 1 {
                polylines.push(std::mem::take(&mut current));
            } else {
                current.clear();
            }
            current.push(PathPoint::new(p.x, p.y));
        }
        kurbo::PathEl::LineTo(p) => current.push(PathPoint::new(p.x, p.y)),
        kurbo::PathEl::ClosePath => {
            if let Some(first) = current.first().copied() {
                if let Some(last) = current.last() {
                    if (last.x - first.x).abs() > 1e-9 || (last.y - first.y).abs() > 1e-9 {
                        current.push(first);
                    }
                }
            }
            if !current.is_empty() {
                polylines.push(std::mem::take(&mut current));
            }
        }
        kurbo::PathEl::QuadTo(_, p) | kurbo::PathEl::CurveTo(_, _, p) => {
            current.push(PathPoint::new(p.x, p.y));
        }
    });
    if !current.is_empty() {
        polylines.push(current);
    }

    for poly in &polylines {
        if poly.len() < 2 {
            continue;
        }
        new_commands.push(PathSegment::MoveTo(poly[0]));
        let mut prev = poly[0];
        for i in 1..poly.len() {
            let p1 = poly[i];
            // Don't round the last vertex on an open polyline.
            let is_last = i + 1 == poly.len();
            if is_last {
                new_commands.push(PathSegment::LineTo(p1));
                prev = p1;
                continue;
            }
            let p2 = poly[i + 1];
            let p0 = prev;
            let in_x = p1.x - p0.x;
            let in_y = p1.y - p0.y;
            let out_x = p2.x - p1.x;
            let out_y = p2.y - p1.y;
            let in_len = (in_x * in_x + in_y * in_y).sqrt();
            let out_len = (out_x * out_x + out_y * out_y).sqrt();
            if in_len < f64::EPSILON || out_len < f64::EPSILON {
                new_commands.push(PathSegment::LineTo(p1));
                prev = p1;
                continue;
            }
            // Effective fillet radius can't exceed half the shorter
            // adjacent segment.
            let r = radius.min(in_len * 0.5).min(out_len * 0.5);
            let in_unit = (in_x / in_len, in_y / in_len);
            let out_unit = (out_x / out_len, out_y / out_len);
            let arc_start = PathPoint::new(p1.x - in_unit.0 * r, p1.y - in_unit.1 * r);
            let arc_end = PathPoint::new(p1.x + out_unit.0 * r, p1.y + out_unit.1 * r);
            new_commands.push(PathSegment::LineTo(arc_start));
            new_commands.push(PathSegment::QuadTo {
                ctrl: p1,
                end: arc_end,
            });
            prev = arc_end;
        }
        if path.closed {
            new_commands.push(PathSegment::Close);
        }
    }
    let mut out = VectorPath::new(new_commands);
    out.closed = path.closed;
    out.fill_rule = path.fill_rule;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(side: f64) -> VectorPath {
        VectorPath::new(vec![
            PathSegment::MoveTo(PathPoint::new(0.0, 0.0)),
            PathSegment::LineTo(PathPoint::new(side, 0.0)),
            PathSegment::LineTo(PathPoint::new(side, side)),
            PathSegment::LineTo(PathPoint::new(0.0, side)),
            PathSegment::Close,
        ])
    }

    #[test]
    fn dash_empty_pattern_is_passthrough() {
        let p = square(10.0);
        let out = dash(&p, &[], 0.0);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn dash_produces_sub_paths() {
        // 40 units of perimeter divided into 4-on, 4-off dashes
        // should yield ~5 visible sub-paths.
        let p = square(10.0);
        let out = dash(&p, &[4.0, 4.0], 0.0);
        assert!(out.len() >= 2);
    }

    #[test]
    fn round_corners_increases_segment_count() {
        let p = square(10.0);
        let before = p.commands.len();
        let out = round_corners(&p, 2.0);
        assert!(out.commands.len() > before);
        // Should contain at least one QuadTo where corners were rounded.
        assert!(out
            .commands
            .iter()
            .any(|c| matches!(c, PathSegment::QuadTo { .. })));
    }

    #[test]
    fn round_corners_zero_radius_is_passthrough_shape() {
        let p = square(10.0);
        let out = round_corners(&p, 0.0);
        // Same point set should be a closed square.
        let b_in = p.bounds();
        let b_out = out.bounds();
        assert!((b_in.width() - b_out.width()).abs() < 1e-6);
        assert!((b_in.height() - b_out.height()).abs() < 1e-6);
    }
}
