//! Polygon boolean operations.
//!
//! Bezier curves are flattened to polylines (via kurbo) before being
//! handed to [`i_overlay`]; the result is returned as line-only
//! [`VectorPath`]s. This matches what Inkscape, Figma, and Penpot do
//! at the operation boundary — curves are reconstructed on the fly
//! only for display, never for boolean math.

use std::fmt;

use i_overlay::core::fill_rule::FillRule as IFillRule;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::single::SingleFloatOverlay;
use kurbo::{flatten, BezPath};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::path::{FillRule, PathPoint, PathSegment, VectorPath};

/// Curve flattening tolerance, in path-space units.
const FLATTEN_TOL: f64 = 0.25;

/// Booleans supported by [`boolean_operation`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BooleanOp {
    /// Union (`A ∪ B`).
    Union,
    /// Difference (`A \ B`).
    Subtract,
    /// Intersection (`A ∩ B`).
    Intersect,
    /// Symmetric difference (`A ⊕ B`).
    Exclude,
}

impl fmt::Display for BooleanOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Union => f.write_str("union"),
            Self::Subtract => f.write_str("subtract"),
            Self::Intersect => f.write_str("intersect"),
            Self::Exclude => f.write_str("exclude"),
        }
    }
}

/// Errors from boolean ops.
#[derive(Debug, Error)]
pub enum VectorBooleanError {
    #[error("input path was empty")]
    EmptyPath,
}

/// Perform `op` on `a` and `b`, returning the resulting set of (line-only)
/// vector paths. Open subpaths are closed prior to the operation —
/// boolean ops only make sense on closed regions.
pub fn boolean_operation(
    op: BooleanOp,
    a: &VectorPath,
    b: &VectorPath,
) -> Result<Vec<VectorPath>, VectorBooleanError> {
    if a.is_empty() || b.is_empty() {
        return Err(VectorBooleanError::EmptyPath);
    }
    let subj = to_polygons(a);
    let clip = to_polygons(b);
    if subj.is_empty() || clip.is_empty() {
        return Err(VectorBooleanError::EmptyPath);
    }
    let rule = match a.fill_rule {
        FillRule::NonZero => IFillRule::NonZero,
        FillRule::EvenOdd => IFillRule::EvenOdd,
    };
    let overlay_rule = match op {
        BooleanOp::Union => OverlayRule::Union,
        BooleanOp::Subtract => OverlayRule::Difference,
        BooleanOp::Intersect => OverlayRule::Intersect,
        BooleanOp::Exclude => OverlayRule::Xor,
    };
    let shapes = subj.overlay(&clip, overlay_rule, rule);
    Ok(shapes_to_paths(&shapes, a.fill_rule))
}

/// Flatten the path to one or more closed polygon contours.
fn to_polygons(path: &VectorPath) -> Vec<Vec<[f64; 2]>> {
    let bez: BezPath = path.to_kurbo();
    let mut out: Vec<Vec<[f64; 2]>> = Vec::new();
    let mut current: Vec<[f64; 2]> = Vec::new();
    flatten(bez.iter(), FLATTEN_TOL, |el| match el {
        kurbo::PathEl::MoveTo(p) => {
            if current.len() >= 3 {
                out.push(std::mem::take(&mut current));
            }
            current.clear();
            current.push([p.x, p.y]);
        }
        kurbo::PathEl::LineTo(p) => {
            current.push([p.x, p.y]);
        }
        kurbo::PathEl::ClosePath => {
            if current.len() >= 3 {
                out.push(std::mem::take(&mut current));
            }
            current.clear();
        }
        // `flatten` only yields MoveTo/LineTo/ClosePath
        kurbo::PathEl::QuadTo(..) | kurbo::PathEl::CurveTo(..) => {}
    });
    if current.len() >= 3 {
        out.push(current);
    }
    out
}

/// Convert `i_overlay`'s `Vec<Vec<Vec<[f64; 2]>>>` shape list to
/// [`VectorPath`]s. Each outer shape becomes one path, with holes
/// emitted as separate subpaths (kurbo's standard convention).
fn shapes_to_paths(shapes: &[Vec<Vec<[f64; 2]>>], fill_rule: FillRule) -> Vec<VectorPath> {
    let mut out = Vec::with_capacity(shapes.len());
    for shape in shapes {
        let mut commands = Vec::new();
        for contour in shape {
            if contour.is_empty() {
                continue;
            }
            commands.push(PathSegment::MoveTo(PathPoint::new(
                contour[0][0],
                contour[0][1],
            )));
            for pt in &contour[1..] {
                commands.push(PathSegment::LineTo(PathPoint::new(pt[0], pt[1])));
            }
            commands.push(PathSegment::Close);
        }
        if commands.is_empty() {
            continue;
        }
        let mut p = VectorPath::new(commands);
        p.fill_rule = fill_rule;
        out.push(p);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f64, y: f64, w: f64, h: f64) -> VectorPath {
        VectorPath::new(vec![
            PathSegment::MoveTo(PathPoint::new(x, y)),
            PathSegment::LineTo(PathPoint::new(x + w, y)),
            PathSegment::LineTo(PathPoint::new(x + w, y + h)),
            PathSegment::LineTo(PathPoint::new(x, y + h)),
            PathSegment::Close,
        ])
    }

    #[test]
    fn union_of_overlapping_rects_produces_one_shape() {
        let a = rect(0.0, 0.0, 10.0, 10.0);
        let b = rect(5.0, 5.0, 10.0, 10.0);
        let result = boolean_operation(BooleanOp::Union, &a, &b).expect("union");
        assert_eq!(result.len(), 1, "expected one merged shape, got {result:?}");
        let bounds = result[0].bounds();
        assert!((bounds.min_x - 0.0).abs() < 1e-3);
        assert!((bounds.min_y - 0.0).abs() < 1e-3);
        assert!((bounds.max_x - 15.0).abs() < 1e-3);
        assert!((bounds.max_y - 15.0).abs() < 1e-3);
    }

    #[test]
    fn intersect_of_overlapping_rects() {
        let a = rect(0.0, 0.0, 10.0, 10.0);
        let b = rect(5.0, 5.0, 10.0, 10.0);
        let result = boolean_operation(BooleanOp::Intersect, &a, &b).expect("inter");
        assert_eq!(result.len(), 1);
        let bounds = result[0].bounds();
        assert!((bounds.max_x - bounds.min_x - 5.0).abs() < 1e-3);
        assert!((bounds.max_y - bounds.min_y - 5.0).abs() < 1e-3);
    }

    #[test]
    fn subtract_carves_corner() {
        let a = rect(0.0, 0.0, 10.0, 10.0);
        let b = rect(5.0, 5.0, 10.0, 10.0);
        let result = boolean_operation(BooleanOp::Subtract, &a, &b).expect("sub");
        assert_eq!(result.len(), 1);
        let bounds = result[0].bounds();
        // Subtraction leaves an "L" — bounding box of A.
        assert!((bounds.min_x - 0.0).abs() < 1e-3);
        assert!((bounds.max_x - 10.0).abs() < 1e-3);
    }

    #[test]
    fn exclude_returns_xor_region() {
        let a = rect(0.0, 0.0, 10.0, 10.0);
        let b = rect(5.0, 5.0, 10.0, 10.0);
        let result = boolean_operation(BooleanOp::Exclude, &a, &b).expect("xor");
        assert!(!result.is_empty());
    }

    #[test]
    fn non_overlapping_intersect_is_empty() {
        let a = rect(0.0, 0.0, 5.0, 5.0);
        let b = rect(100.0, 100.0, 5.0, 5.0);
        let result = boolean_operation(BooleanOp::Intersect, &a, &b).expect("inter");
        assert!(result.is_empty());
    }

    #[test]
    fn empty_input_errors() {
        let empty = VectorPath::new(Vec::new());
        let a = rect(0.0, 0.0, 1.0, 1.0);
        assert!(matches!(
            boolean_operation(BooleanOp::Union, &empty, &a),
            Err(VectorBooleanError::EmptyPath)
        ));
        assert!(matches!(
            boolean_operation(BooleanOp::Union, &a, &empty),
            Err(VectorBooleanError::EmptyPath)
        ));
    }
}
