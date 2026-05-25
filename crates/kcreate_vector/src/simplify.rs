//! Path simplification, smoothing, and parallel-offset.
//!
//! Operates on the polyline approximation of a [`VectorPath`]
//! (flattened to lines via kurbo). This keeps the algorithms simple
//! and shape-agnostic — they work uniformly across lines, quad and
//! cubic Beziers — at the cost of curves being re-emitted as
//! line segments. For Phase 5 that trade is the right one: the
//! editor stores the post-simplify path as the source of truth, and
//! the user is in control of whether they want curves preserved.

use kurbo::{flatten, BezPath, ParamCurve, PathSeg, Point};

use crate::path::{FillRule, PathPoint, PathSegment, VectorPath};

const FLATTEN_TOL: f64 = 0.5;

fn flatten_to_polyline(path: &VectorPath) -> Vec<Vec<Point>> {
    let bez = path.to_kurbo();
    let mut polylines: Vec<Vec<Point>> = Vec::new();
    let mut current: Vec<Point> = Vec::new();
    flatten(bez.iter(), FLATTEN_TOL, |el| match el {
        kurbo::PathEl::MoveTo(p) => {
            if current.len() > 1 {
                polylines.push(std::mem::take(&mut current));
            } else {
                current.clear();
            }
            current.push(p);
        }
        kurbo::PathEl::LineTo(p)
        | kurbo::PathEl::QuadTo(_, p)
        | kurbo::PathEl::CurveTo(_, _, p) => current.push(p),
        kurbo::PathEl::ClosePath => {
            if let Some(first) = current.first().copied() {
                if current.last().is_some_and(|last| {
                    (last.x - first.x).abs() > 1e-9 || (last.y - first.y).abs() > 1e-9
                }) {
                    current.push(first);
                }
            }
            if !current.is_empty() {
                polylines.push(std::mem::take(&mut current));
            }
        }
    });
    if current.len() > 1 {
        polylines.push(current);
    }
    polylines
}

fn polylines_to_vector(polylines: &[Vec<Point>], closed: bool, fill_rule: FillRule) -> VectorPath {
    let mut commands: Vec<PathSegment> = Vec::new();
    for poly in polylines {
        if poly.is_empty() {
            continue;
        }
        commands.push(PathSegment::MoveTo(PathPoint::new(poly[0].x, poly[0].y)));
        for pt in poly.iter().skip(1) {
            commands.push(PathSegment::LineTo(PathPoint::new(pt.x, pt.y)));
        }
        if closed {
            commands.push(PathSegment::Close);
        }
    }
    let mut out = VectorPath::new(commands);
    out.closed = closed;
    out.fill_rule = fill_rule;
    out
}

/// Ramer–Douglas–Peucker simplification.
///
/// `tolerance` is the max allowed perpendicular distance between the
/// original polyline and the simplified one, in the same coordinate
/// space as the path. Returns at minimum two points per sub-polyline.
#[must_use]
pub fn simplify(path: &VectorPath, tolerance: f64) -> VectorPath {
    let polylines = flatten_to_polyline(path);
    let simplified: Vec<Vec<Point>> = polylines.iter().map(|poly| rdp(poly, tolerance)).collect();
    polylines_to_vector(&simplified, path.closed, path.fill_rule)
}

fn rdp(points: &[Point], tolerance: f64) -> Vec<Point> {
    if points.len() < 3 {
        return points.to_vec();
    }
    let mut keep = vec![false; points.len()];
    keep[0] = true;
    *keep.last_mut().expect("non-empty") = true;
    rdp_recursive(points, 0, points.len() - 1, tolerance, &mut keep);
    points
        .iter()
        .zip(keep.iter())
        .filter_map(|(p, k)| if *k { Some(*p) } else { None })
        .collect()
}

fn rdp_recursive(points: &[Point], lo: usize, hi: usize, tolerance: f64, keep: &mut [bool]) {
    if hi <= lo + 1 {
        return;
    }
    let a = points[lo];
    let b = points[hi];
    let mut max_d = 0.0f64;
    let mut max_i = lo;
    for (i, p) in points.iter().enumerate().take(hi).skip(lo + 1) {
        let d = perpendicular_distance(*p, a, b);
        if d > max_d {
            max_d = d;
            max_i = i;
        }
    }
    if max_d > tolerance {
        keep[max_i] = true;
        rdp_recursive(points, lo, max_i, tolerance, keep);
        rdp_recursive(points, max_i, hi, tolerance, keep);
    }
}

fn perpendicular_distance(p: Point, a: Point, b: Point) -> f64 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let len_sq = dx * dx + dy * dy;
    if len_sq < f64::EPSILON {
        return (p.x - a.x).hypot(p.y - a.y);
    }
    let t = ((p.x - a.x) * dx + (p.y - a.y) * dy) / len_sq;
    let t = t.clamp(0.0, 1.0);
    let cx = a.x + t * dx;
    let cy = a.y + t * dy;
    (p.x - cx).hypot(p.y - cy)
}

/// Chaikin's corner-cutting smoothing.
///
/// One iteration emits two new points along each edge, at 1/4 and 3/4
/// of the segment. Repeated iterations approximate a quadratic
/// B-spline. The polyline length stays bounded by the original.
#[must_use]
pub fn smooth(path: &VectorPath, iterations: u32) -> VectorPath {
    if iterations == 0 {
        return path.clone();
    }
    let polylines = flatten_to_polyline(path);
    let smoothed: Vec<Vec<Point>> = polylines
        .into_iter()
        .map(|p| chaikin_iter(p, iterations, path.closed))
        .collect();
    polylines_to_vector(&smoothed, path.closed, path.fill_rule)
}

fn chaikin_iter(mut points: Vec<Point>, iterations: u32, closed: bool) -> Vec<Point> {
    for _ in 0..iterations {
        if points.len() < 2 {
            break;
        }
        let mut next = Vec::with_capacity(points.len() * 2);
        if closed {
            for i in 0..points.len() {
                let a = points[i];
                let b = points[(i + 1) % points.len()];
                next.push(Point::new(a.x * 0.75 + b.x * 0.25, a.y * 0.75 + b.y * 0.25));
                next.push(Point::new(a.x * 0.25 + b.x * 0.75, a.y * 0.25 + b.y * 0.75));
            }
        } else {
            next.push(points[0]);
            for w in points.windows(2) {
                let a = w[0];
                let b = w[1];
                next.push(Point::new(a.x * 0.75 + b.x * 0.25, a.y * 0.75 + b.y * 0.25));
                next.push(Point::new(a.x * 0.25 + b.x * 0.75, a.y * 0.25 + b.y * 0.75));
            }
            next.push(*points.last().expect("len >= 2"));
        }
        points = next;
    }
    points
}

/// Parallel offset.
///
/// For closed paths, a positive `distance` insets (offset to the
/// interior); for open paths it offsets to the *left* of the curve
/// direction. Implementation: walk the flattened polyline, emit a
/// new point perpendicular to each segment.
#[must_use]
pub fn offset(path: &VectorPath, distance: f64) -> VectorPath {
    if distance.abs() < f64::EPSILON {
        return path.clone();
    }
    let polylines = flatten_to_polyline(path);
    let offset_polys: Vec<Vec<Point>> = polylines
        .into_iter()
        .map(|p| offset_polyline(&p, distance, path.closed))
        .collect();
    polylines_to_vector(&offset_polys, path.closed, path.fill_rule)
}

fn segment_normal(a: Point, b: Point) -> (f64, f64) {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let len = dx.hypot(dy);
    if len < f64::EPSILON {
        return (0.0, 0.0);
    }
    // Left-perpendicular normal of (dx, dy) is (-dy, dx) / len.
    (-dy / len, dx / len)
}

fn offset_polyline(points: &[Point], distance: f64, closed: bool) -> Vec<Point> {
    if points.len() < 2 {
        return points.to_vec();
    }
    let mut out = Vec::with_capacity(points.len());
    let n = points.len();
    for i in 0..n {
        // Average the normals of the incident edges (miter join).
        let (nx_in, ny_in) = if i == 0 {
            if closed {
                segment_normal(points[n - 1], points[0])
            } else {
                segment_normal(points[0], points[1])
            }
        } else {
            segment_normal(points[i - 1], points[i])
        };
        let (nx_out, ny_out) = if i + 1 == n {
            if closed {
                segment_normal(points[n - 1], points[0])
            } else {
                segment_normal(points[n - 2], points[n - 1])
            }
        } else {
            segment_normal(points[i], points[i + 1])
        };
        let nx = (nx_in + nx_out) * 0.5;
        let ny = (ny_in + ny_out) * 0.5;
        // Re-normalise (miter shortens when corners are sharp; this
        // keeps offsets perpendicular at the joint).
        let len = nx.hypot(ny);
        let (nxr, nyr) = if len < f64::EPSILON {
            (nx_out, ny_out)
        } else {
            (nx / len, ny / len)
        };
        out.push(Point::new(
            points[i].x + nxr * distance,
            points[i].y + nyr * distance,
        ));
    }
    out
}

/// Build a fully-offset outline (both sides) suitable for variable
/// stroke expansion. The result is a closed path enclosing the
/// original curve.
#[must_use]
pub fn outline_offset(path: &VectorPath, half_width: f64) -> VectorPath {
    let polylines = flatten_to_polyline(path);
    let mut commands: Vec<PathSegment> = Vec::new();
    for poly in &polylines {
        if poly.len() < 2 {
            continue;
        }
        let left = offset_polyline(poly, half_width, false);
        let right = offset_polyline(poly, -half_width, false);
        if left.is_empty() || right.is_empty() {
            continue;
        }
        commands.push(PathSegment::MoveTo(PathPoint::new(left[0].x, left[0].y)));
        for p in left.iter().skip(1) {
            commands.push(PathSegment::LineTo(PathPoint::new(p.x, p.y)));
        }
        for p in right.iter().rev() {
            commands.push(PathSegment::LineTo(PathPoint::new(p.x, p.y)));
        }
        commands.push(PathSegment::Close);
    }
    let mut out = VectorPath::new(commands);
    out.closed = true;
    out
}

#[allow(dead_code)]
fn segment_endpoints(seg: PathSeg) -> (Point, Point) {
    match seg {
        PathSeg::Line(line) => (line.p0, line.p1),
        PathSeg::Quad(q) => (q.p0, q.p2),
        PathSeg::Cubic(c) => (c.p0, c.p3),
    }
}

#[allow(dead_code)]
fn ensure_kurbo_has_segments(_bez: &BezPath) {
    // Kept as documentation: kurbo's `BezPath::segments` was the
    // upstream API we rely on for arc-length walks in `path_effects`.
    // The unused-import warning would otherwise hide the dependency.
}

#[allow(dead_code)]
fn point_at_t(seg: PathSeg, t: f64) -> Point {
    seg.eval(t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path::PathPoint;

    fn line_path(steps: usize) -> VectorPath {
        let mut cmds = vec![PathSegment::MoveTo(PathPoint::new(0.0, 0.0))];
        for i in 1..=steps {
            cmds.push(PathSegment::LineTo(PathPoint::new(i as f64, 0.0)));
        }
        VectorPath::new(cmds)
    }

    fn square_path(side: f64) -> VectorPath {
        VectorPath::new(vec![
            PathSegment::MoveTo(PathPoint::new(0.0, 0.0)),
            PathSegment::LineTo(PathPoint::new(side, 0.0)),
            PathSegment::LineTo(PathPoint::new(side, side)),
            PathSegment::LineTo(PathPoint::new(0.0, side)),
            PathSegment::Close,
        ])
    }

    #[test]
    fn simplify_collinear_returns_two_points() {
        let p = line_path(10);
        let out = simplify(&p, 0.001);
        // Two MoveTo+LineTo (one polyline of 2 unique points).
        let line_count = out
            .commands
            .iter()
            .filter(|c| matches!(c, PathSegment::LineTo(_)))
            .count();
        assert_eq!(line_count, 1);
    }

    #[test]
    fn smooth_increases_point_count() {
        let p = square_path(10.0);
        let before = p.commands.len();
        let smoothed = smooth(&p, 2);
        assert!(smoothed.commands.len() > before);
    }

    #[test]
    fn offset_zero_is_identity_polyline() {
        let p = square_path(10.0);
        let out = offset(&p, 0.0);
        assert_eq!(out.commands.len(), p.commands.len());
    }

    #[test]
    fn offset_inward_shrinks_bbox() {
        let p = square_path(20.0);
        let inset = offset(&p, 5.0);
        let b_before = p.bounds();
        let b_after = inset.bounds();
        assert!(b_after.width() < b_before.width());
        assert!(b_after.height() < b_before.height());
    }
}
