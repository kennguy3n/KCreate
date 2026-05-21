//! Vector path representation and math.
//!
//! [`VectorPath`] is the on-the-wire and on-disk format. It uses an
//! explicit segment-command list (rather than a closed flattened
//! polyline) so that bezier curves round-trip without quantization.
//! For numeric work we convert to [`kurbo::BezPath`] which provides
//! high-quality bounds, length, and parameterization routines.

use std::fmt;

use kurbo::{BezPath, ParamCurve, ParamCurveArclen, Shape};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

/// A 2D point with f64 precision. Distinct from
/// [`kcreate_core::node::Point2D`] only so that this crate can be
/// consumed without depending on the full node module — the two are
/// transparently convertible.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PathPoint {
    pub x: f64,
    pub y: f64,
}

impl PathPoint {
    #[must_use]
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

impl From<kurbo::Point> for PathPoint {
    fn from(p: kurbo::Point) -> Self {
        Self::new(p.x, p.y)
    }
}

impl From<PathPoint> for kurbo::Point {
    fn from(p: PathPoint) -> Self {
        Self::new(p.x, p.y)
    }
}

/// Axis-aligned bounding box.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BoundingBox {
    pub min_x: f64,
    pub min_y: f64,
    pub max_x: f64,
    pub max_y: f64,
}

impl BoundingBox {
    #[must_use]
    pub const fn new(min_x: f64, min_y: f64, max_x: f64, max_y: f64) -> Self {
        Self {
            min_x,
            min_y,
            max_x,
            max_y,
        }
    }

    /// An empty box that is "less than" any real box for `union`.
    #[must_use]
    pub const fn empty() -> Self {
        Self::new(
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        )
    }

    /// Width = `max_x - min_x` (clamped to 0 for an empty box).
    #[must_use]
    pub fn width(self) -> f64 {
        (self.max_x - self.min_x).max(0.0)
    }

    /// Height = `max_y - min_y` (clamped to 0 for an empty box).
    #[must_use]
    pub fn height(self) -> f64 {
        (self.max_y - self.min_y).max(0.0)
    }

    /// `true` when the box is degenerate (empty or zero area).
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.max_x <= self.min_x || self.max_y <= self.min_y
    }

    /// Smallest box containing both `self` and `other`.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self {
            min_x: self.min_x.min(other.min_x),
            min_y: self.min_y.min(other.min_y),
            max_x: self.max_x.max(other.max_x),
            max_y: self.max_y.max(other.max_y),
        }
    }

    /// Largest box contained in both `self` and `other`. May be empty.
    #[must_use]
    pub const fn intersection(self, other: Self) -> Self {
        Self {
            min_x: self.min_x.max(other.min_x),
            min_y: self.min_y.max(other.min_y),
            max_x: self.max_x.min(other.max_x),
            max_y: self.max_y.min(other.max_y),
        }
    }

    /// `true` if `(x, y)` falls inside (inclusive).
    #[must_use]
    pub const fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.min_x && x <= self.max_x && y >= self.min_y && y <= self.max_y
    }

    /// Build from a `kurbo::Rect`.
    #[must_use]
    pub const fn from_rect(r: kurbo::Rect) -> Self {
        Self::new(r.x0, r.y0, r.x1, r.y1)
    }
}

impl Default for BoundingBox {
    fn default() -> Self {
        Self::empty()
    }
}

/// SVG-style fill rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FillRule {
    #[default]
    NonZero,
    EvenOdd,
}

/// A single path command. Matches the SVG `<path>` command set, plus
/// quadratic bezier (Q) for completeness.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum PathSegment {
    MoveTo(PathPoint),
    LineTo(PathPoint),
    QuadTo {
        ctrl: PathPoint,
        end: PathPoint,
    },
    CubicTo {
        ctrl1: PathPoint,
        ctrl2: PathPoint,
        end: PathPoint,
    },
    Close,
}

/// Errors from path operations.
#[derive(Debug, Error)]
pub enum PathError {
    #[error("empty path")]
    Empty,
    #[error("path parameter out of range: {0}")]
    OutOfRange(f64),
}

/// A vector path: a sequence of commands plus closed/fill metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VectorPath {
    pub id: Uuid,
    pub commands: Vec<PathSegment>,
    pub closed: bool,
    pub fill_rule: FillRule,
}

impl VectorPath {
    /// Build a fresh path from a list of commands.
    #[must_use]
    pub fn new(commands: Vec<PathSegment>) -> Self {
        let closed = matches!(commands.last(), Some(PathSegment::Close));
        Self {
            id: Uuid::new_v4(),
            commands,
            closed,
            fill_rule: FillRule::NonZero,
        }
    }

    /// True when there are no commands.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    /// Number of commands in the path.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.commands.len()
    }

    /// Tight bounding box of the curve geometry (not just the control
    /// points). Falls back to [`BoundingBox::empty`] for an empty path.
    #[must_use]
    pub fn bounds(&self) -> BoundingBox {
        if self.is_empty() {
            return BoundingBox::empty();
        }
        let bez = self.to_kurbo();
        BoundingBox::from_rect(bez.bounding_box())
    }

    /// Total arc length of the path. `0.0` for an empty path.
    #[must_use]
    pub fn length(&self) -> f64 {
        if self.is_empty() {
            return 0.0;
        }
        let bez = self.to_kurbo();
        bez.segments().map(|s| s.arclen(1e-3)).sum()
    }

    /// Sample a point along the path at normalized parameter
    /// `t \in [0.0, 1.0]`. Linearly interpolates across segments by
    /// arc length.
    pub fn point_at(&self, t: f64) -> Result<PathPoint, PathError> {
        if self.is_empty() {
            return Err(PathError::Empty);
        }
        if !(0.0..=1.0).contains(&t) {
            return Err(PathError::OutOfRange(t));
        }
        let bez = self.to_kurbo();
        let segments: Vec<_> = bez.segments().collect();
        if segments.is_empty() {
            return Err(PathError::Empty);
        }
        let arcs: Vec<f64> = segments.iter().map(|s| s.arclen(1e-3)).collect();
        let total: f64 = arcs.iter().copied().sum();
        if total == 0.0 {
            // Degenerate path (e.g. one MoveTo). Return the start point.
            return Ok(segments[0].start().into());
        }
        let target = t * total;
        let mut remaining = target;
        for (seg, len) in segments.iter().zip(arcs.iter().copied()) {
            if remaining <= len || (remaining - len).abs() < f64::EPSILON {
                let local_t = if len > 0.0 {
                    (remaining / len).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                return Ok(seg.eval(local_t).into());
            }
            remaining -= len;
        }
        // Numerical drift: fall through to the end of the last segment.
        let last = segments.last().expect("non-empty checked above");
        Ok(last.eval(1.0).into())
    }

    /// Reverse the direction of the path in place.
    pub fn reverse(&mut self) {
        let bez = self.to_kurbo();
        let reversed = bez.reverse_subpaths();
        *self = Self::from_kurbo(&reversed);
    }

    /// Reduce node count by collapsing collinear/near-collinear lines
    /// within `tolerance` (in path-space units).
    #[must_use]
    pub fn simplify(&self, tolerance: f64) -> Self {
        if self.commands.len() <= 2 {
            return self.clone();
        }
        let mut out = Vec::with_capacity(self.commands.len());
        let mut last_point: Option<PathPoint> = None;
        let mut pending_line_start: Option<PathPoint> = None;
        for &cmd in &self.commands {
            match cmd {
                PathSegment::MoveTo(p) => {
                    if let Some(start) = pending_line_start.take() {
                        if let Some(prev) = last_point {
                            out.push(PathSegment::LineTo(prev));
                            let _ = start;
                        }
                    }
                    out.push(cmd);
                    last_point = Some(p);
                }
                PathSegment::LineTo(p) => {
                    if let (Some(_), Some(prev)) = (out.last(), last_point) {
                        if pending_line_start.is_none() {
                            pending_line_start = Some(prev);
                        }
                        if let Some(start) = pending_line_start {
                            if collinear(start, prev, p, tolerance) {
                                // Drop the previous LineTo by replacing
                                // the most-recent emit.
                                if let Some(PathSegment::LineTo(_)) = out.last() {
                                    out.pop();
                                }
                                out.push(PathSegment::LineTo(p));
                                last_point = Some(p);
                                continue;
                            }
                        }
                    }
                    out.push(cmd);
                    last_point = Some(p);
                    pending_line_start = None;
                }
                PathSegment::QuadTo { end, .. } | PathSegment::CubicTo { end, .. } => {
                    out.push(cmd);
                    last_point = Some(end);
                    pending_line_start = None;
                }
                PathSegment::Close => {
                    out.push(cmd);
                    pending_line_start = None;
                }
            }
        }
        Self {
            id: self.id,
            commands: out,
            closed: self.closed,
            fill_rule: self.fill_rule,
        }
    }

    /// Convert to a `kurbo::BezPath` for numeric work.
    #[must_use]
    pub fn to_kurbo(&self) -> BezPath {
        let mut p = BezPath::new();
        for cmd in &self.commands {
            match *cmd {
                PathSegment::MoveTo(pt) => p.move_to(kurbo::Point::from(pt)),
                PathSegment::LineTo(pt) => p.line_to(kurbo::Point::from(pt)),
                PathSegment::QuadTo { ctrl, end } => {
                    p.quad_to(kurbo::Point::from(ctrl), kurbo::Point::from(end));
                }
                PathSegment::CubicTo { ctrl1, ctrl2, end } => {
                    p.curve_to(
                        kurbo::Point::from(ctrl1),
                        kurbo::Point::from(ctrl2),
                        kurbo::Point::from(end),
                    );
                }
                PathSegment::Close => p.close_path(),
            }
        }
        p
    }

    /// Build a `VectorPath` from a `kurbo::BezPath`.
    #[must_use]
    pub fn from_kurbo(path: &BezPath) -> Self {
        let mut commands = Vec::with_capacity(path.elements().len());
        let mut closed = false;
        for el in path.elements() {
            match *el {
                kurbo::PathEl::MoveTo(p) => commands.push(PathSegment::MoveTo(p.into())),
                kurbo::PathEl::LineTo(p) => commands.push(PathSegment::LineTo(p.into())),
                kurbo::PathEl::QuadTo(c, p) => commands.push(PathSegment::QuadTo {
                    ctrl: c.into(),
                    end: p.into(),
                }),
                kurbo::PathEl::CurveTo(c1, c2, p) => commands.push(PathSegment::CubicTo {
                    ctrl1: c1.into(),
                    ctrl2: c2.into(),
                    end: p.into(),
                }),
                kurbo::PathEl::ClosePath => {
                    commands.push(PathSegment::Close);
                    closed = true;
                }
            }
        }
        Self {
            id: Uuid::new_v4(),
            commands,
            closed,
            fill_rule: FillRule::NonZero,
        }
    }
}

impl fmt::Display for VectorPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "VectorPath({} segs, closed={})",
            self.commands.len(),
            self.closed
        )
    }
}

/// True if `b` lies (approximately) on segment `ac`.
fn collinear(a: PathPoint, b: PathPoint, c: PathPoint, tolerance: f64) -> bool {
    let cross = (b.x - a.x).mul_add(c.y - a.y, -((b.y - a.y) * (c.x - a.x)));
    cross.abs() <= tolerance
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect_path(w: f64, h: f64) -> VectorPath {
        VectorPath::new(vec![
            PathSegment::MoveTo(PathPoint::new(0.0, 0.0)),
            PathSegment::LineTo(PathPoint::new(w, 0.0)),
            PathSegment::LineTo(PathPoint::new(w, h)),
            PathSegment::LineTo(PathPoint::new(0.0, h)),
            PathSegment::Close,
        ])
    }

    #[test]
    fn rect_bounds() {
        let r = rect_path(10.0, 5.0);
        let b = r.bounds();
        assert!((b.min_x - 0.0).abs() < 1e-6);
        assert!((b.min_y - 0.0).abs() < 1e-6);
        assert!((b.max_x - 10.0).abs() < 1e-6);
        assert!((b.max_y - 5.0).abs() < 1e-6);
        assert!((b.width() - 10.0).abs() < 1e-6);
        assert!((b.height() - 5.0).abs() < 1e-6);
    }

    #[test]
    fn rect_length() {
        let r = rect_path(10.0, 5.0);
        // Three explicit LineTo segs (10 + 5 + 10) + Close (5) = 30.
        assert!((r.length() - 30.0).abs() < 1e-3);
    }

    #[test]
    fn point_at_endpoints() {
        let r = rect_path(10.0, 5.0);
        let start = r.point_at(0.0).expect("start");
        let end = r.point_at(1.0).expect("end");
        assert!((start.x - 0.0).abs() < 1e-6 && (start.y - 0.0).abs() < 1e-6);
        // After Close we return to origin.
        assert!((end.x - 0.0).abs() < 1e-6 && (end.y - 0.0).abs() < 1e-6);
    }

    #[test]
    fn point_at_out_of_range_errors() {
        let r = rect_path(1.0, 1.0);
        assert!(matches!(r.point_at(-0.1), Err(PathError::OutOfRange(_))));
        assert!(matches!(r.point_at(1.1), Err(PathError::OutOfRange(_))));
    }

    #[test]
    fn point_at_empty_errors() {
        let p = VectorPath::new(Vec::new());
        assert!(matches!(p.point_at(0.5), Err(PathError::Empty)));
    }

    #[test]
    fn reverse_round_trip() {
        let r = rect_path(10.0, 5.0);
        let mut r2 = r.clone();
        r2.reverse();
        r2.reverse();
        // Geometry is the same — total length matches.
        assert!((r.length() - r2.length()).abs() < 1e-3);
    }

    #[test]
    fn from_to_kurbo_round_trip() {
        let r = rect_path(7.5, 2.5);
        let bez = r.to_kurbo();
        let r2 = VectorPath::from_kurbo(&bez);
        assert_eq!(r.commands.len(), r2.commands.len());
        for (a, b) in r.commands.iter().zip(r2.commands.iter()) {
            assert_eq!(std::mem::discriminant(a), std::mem::discriminant(b));
        }
    }

    #[test]
    fn simplify_drops_collinear_points() {
        let p = VectorPath::new(vec![
            PathSegment::MoveTo(PathPoint::new(0.0, 0.0)),
            PathSegment::LineTo(PathPoint::new(1.0, 0.0)),
            PathSegment::LineTo(PathPoint::new(2.0, 0.0)),
            PathSegment::LineTo(PathPoint::new(3.0, 0.0)),
        ]);
        let s = p.simplify(1e-6);
        // Should reduce to MoveTo + single LineTo at (3, 0).
        assert!(s.commands.len() <= 2 + 1);
        assert!(matches!(s.commands[0], PathSegment::MoveTo(_)));
    }

    #[test]
    fn bounds_empty_path() {
        let p = VectorPath::new(Vec::new());
        let b = p.bounds();
        assert!(b.is_empty());
    }

    #[test]
    fn bounds_union_and_intersection() {
        let a = BoundingBox::new(0.0, 0.0, 10.0, 10.0);
        let b = BoundingBox::new(5.0, 5.0, 15.0, 15.0);
        let u = a.union(b);
        assert_eq!(u, BoundingBox::new(0.0, 0.0, 15.0, 15.0));
        let i = a.intersection(b);
        assert_eq!(i, BoundingBox::new(5.0, 5.0, 10.0, 10.0));
        assert!(a.contains(1.0, 1.0));
        assert!(!a.contains(11.0, 11.0));
    }

    #[test]
    fn serde_round_trip() {
        let r = rect_path(10.0, 5.0);
        let s = serde_json::to_string(&r).expect("serialize");
        let r2: VectorPath = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(r, r2);
    }
}
