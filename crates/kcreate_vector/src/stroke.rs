//! Variable-width stroke expansion.
//!
//! `expand_variable_stroke` walks the centreline polyline and, for
//! each pair of adjacent points, looks up the half-width from a
//! `(t, width)` profile (linearly interpolated), then emits an
//! offset polygon. The result is a closed [`VectorPath`] that the
//! renderer fills like any other shape.

use kurbo::{flatten, Point};

use crate::path::{PathPoint, PathSegment, VectorPath};
use crate::simplify::outline_offset;

const FLATTEN_TOL: f64 = 0.5;

/// Linear interpolation on a sorted `(t, width)` profile.
///
/// `t = 0.0` returns the first profile entry; `t = 1.0` returns the
/// last. Empty profiles fall back to the supplied `default`.
#[must_use]
pub fn sample_profile(profile: &[(f64, f64)], t: f64, default: f64) -> f64 {
    if profile.is_empty() {
        return default;
    }
    if profile.len() == 1 {
        return profile[0].1;
    }
    let clamped = t.clamp(0.0, 1.0);
    if clamped <= profile[0].0 {
        return profile[0].1;
    }
    if clamped >= profile[profile.len() - 1].0 {
        return profile[profile.len() - 1].1;
    }
    for window in profile.windows(2) {
        let (t0, w0) = window[0];
        let (t1, w1) = window[1];
        if clamped >= t0 && clamped <= t1 {
            let dt = t1 - t0;
            if dt.abs() < f64::EPSILON {
                return w0;
            }
            let alpha = (clamped - t0) / dt;
            return w0 + (w1 - w0) * alpha;
        }
    }
    default
}

fn segment_normal(a: Point, b: Point) -> (f64, f64) {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let len = dx.hypot(dy);
    if len < f64::EPSILON {
        return (0.0, 0.0);
    }
    (-dy / len, dx / len)
}

/// Expand a centreline path with a `(t, width)` profile into a filled
/// outline.
///
/// When the profile is empty the function falls back to a uniform
/// stroke of `default_width`, producing a constant-width outline via
/// [`crate::simplify::outline_offset`].
#[must_use]
pub fn expand_variable_stroke(
    centreline: &VectorPath,
    profile: &[(f64, f64)],
    default_width: f64,
) -> VectorPath {
    if profile.is_empty() {
        return outline_offset(centreline, default_width * 0.5);
    }

    let bez = centreline.to_kurbo();
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
    });
    if current.len() > 1 {
        polylines.push(current);
    }

    let mut commands: Vec<PathSegment> = Vec::new();
    for poly in &polylines {
        if poly.len() < 2 {
            continue;
        }
        // Arc-length parameterisation: each vertex's t is the
        // fraction of total polyline length up to that vertex.
        let mut cumlen = Vec::with_capacity(poly.len());
        cumlen.push(0.0);
        let mut total = 0.0;
        for w in poly.windows(2) {
            let dx = w[1].x - w[0].x;
            let dy = w[1].y - w[0].y;
            total += dx.hypot(dy);
            cumlen.push(total);
        }
        if total < f64::EPSILON {
            continue;
        }
        // Emit left side, then right side reversed, then Close.
        let mut left: Vec<Point> = Vec::with_capacity(poly.len());
        let mut right: Vec<Point> = Vec::with_capacity(poly.len());
        for i in 0..poly.len() {
            let t = cumlen[i] / total;
            let half_w = sample_profile(profile, t, default_width * 0.5) * 0.5;
            let (nx_in, ny_in) = if i == 0 {
                segment_normal(poly[0], poly[1])
            } else {
                segment_normal(poly[i - 1], poly[i])
            };
            let (nx_out, ny_out) = if i + 1 == poly.len() {
                segment_normal(poly[poly.len() - 2], poly[poly.len() - 1])
            } else {
                segment_normal(poly[i], poly[i + 1])
            };
            let mut nx = (nx_in + nx_out) * 0.5;
            let mut ny = (ny_in + ny_out) * 0.5;
            let n_len = nx.hypot(ny);
            if n_len > f64::EPSILON {
                nx /= n_len;
                ny /= n_len;
            } else {
                nx = nx_out;
                ny = ny_out;
            }
            left.push(Point::new(poly[i].x + nx * half_w, poly[i].y + ny * half_w));
            right.push(Point::new(poly[i].x - nx * half_w, poly[i].y - ny * half_w));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn line_path(len: f64) -> VectorPath {
        VectorPath::new(vec![
            PathSegment::MoveTo(PathPoint::new(0.0, 0.0)),
            PathSegment::LineTo(PathPoint::new(len, 0.0)),
        ])
    }

    #[test]
    fn sample_profile_endpoints() {
        let p = vec![(0.0, 1.0), (1.0, 5.0)];
        assert!((sample_profile(&p, 0.0, 0.0) - 1.0).abs() < 1e-9);
        assert!((sample_profile(&p, 1.0, 0.0) - 5.0).abs() < 1e-9);
        assert!((sample_profile(&p, 0.5, 0.0) - 3.0).abs() < 1e-9);
    }

    #[test]
    fn empty_profile_uses_uniform_default() {
        let line = line_path(10.0);
        let out = expand_variable_stroke(&line, &[], 4.0);
        assert!(out.closed);
        assert!(out.commands.len() >= 4);
    }

    #[test]
    fn expanded_stroke_is_closed_polygon() {
        let line = line_path(20.0);
        let out = expand_variable_stroke(&line, &[(0.0, 2.0), (1.0, 8.0)], 4.0);
        assert!(out.closed);
        assert!(matches!(out.commands.last(), Some(PathSegment::Close)));
        // Bounding height should reflect the larger end width.
        let b = out.bounds();
        assert!(b.height() >= 6.0);
    }
}
