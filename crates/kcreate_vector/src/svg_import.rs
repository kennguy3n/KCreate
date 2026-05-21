//! SVG import — backed by [`usvg`].
//!
//! `usvg` does the heavy lifting: it normalises shapes (rects, circles,
//! polygons, `<use>` etc.) into a clean tree of `Path` nodes with
//! absolute transforms and resolved CSS. We walk that tree, transform
//! each path's points into final coordinates, and emit one
//! [`VectorPath`] per `<path>` element.

use std::fs;
use std::path::Path;

use thiserror::Error;
use usvg::tiny_skia_path::{PathSegment as SkiaSeg, Point as SkiaPt};

use crate::path::{FillRule, PathPoint, PathSegment, VectorPath};

/// Errors from SVG import.
#[derive(Debug, Error)]
pub enum SvgImportError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("svg parse error: {0}")]
    Parse(String),
}

/// Parse an SVG byte buffer into a flat list of [`VectorPath`]s. Groups
/// are flattened, transforms are baked in.
pub fn import_svg(svg_data: &[u8]) -> Result<Vec<VectorPath>, SvgImportError> {
    let opt = usvg::Options::default();
    let tree =
        usvg::Tree::from_data(svg_data, &opt).map_err(|e| SvgImportError::Parse(e.to_string()))?;
    let mut out = Vec::new();
    walk_group(tree.root(), &mut out);
    Ok(out)
}

/// Parse an SVG file from disk.
pub fn import_svg_file(path: &Path) -> Result<Vec<VectorPath>, SvgImportError> {
    let bytes = fs::read(path)?;
    import_svg(&bytes)
}

fn walk_group(group: &usvg::Group, out: &mut Vec<VectorPath>) {
    for node in group.children() {
        match node {
            usvg::Node::Group(g) => walk_group(g, out),
            usvg::Node::Path(p) => {
                if let Some(vp) = convert_path(p) {
                    out.push(vp);
                }
            }
            // Images / text are not vector paths; Phase 0 ignores them.
            usvg::Node::Image(_) | usvg::Node::Text(_) => {}
        }
    }
}

fn convert_path(usvg_path: &usvg::Path) -> Option<VectorPath> {
    let mut commands = Vec::new();
    let xf = usvg_path.abs_transform();
    let mut closed = false;
    for seg in usvg_path.data().segments() {
        match seg {
            SkiaSeg::MoveTo(p) => commands.push(PathSegment::MoveTo(map_pt(p, &xf))),
            SkiaSeg::LineTo(p) => commands.push(PathSegment::LineTo(map_pt(p, &xf))),
            SkiaSeg::QuadTo(c, p) => commands.push(PathSegment::QuadTo {
                ctrl: map_pt(c, &xf),
                end: map_pt(p, &xf),
            }),
            SkiaSeg::CubicTo(c1, c2, p) => commands.push(PathSegment::CubicTo {
                ctrl1: map_pt(c1, &xf),
                ctrl2: map_pt(c2, &xf),
                end: map_pt(p, &xf),
            }),
            SkiaSeg::Close => {
                commands.push(PathSegment::Close);
                closed = true;
            }
        }
    }
    if commands.is_empty() {
        return None;
    }
    let mut p = VectorPath::new(commands);
    p.closed = closed;
    if let Some(fill) = usvg_path.fill() {
        p.fill_rule = match fill.rule() {
            usvg::FillRule::NonZero => FillRule::NonZero,
            usvg::FillRule::EvenOdd => FillRule::EvenOdd,
        };
    }
    Some(p)
}

fn map_pt(pt: SkiaPt, xf: &usvg::Transform) -> PathPoint {
    let mut p = pt;
    xf.map_point(&mut p);
    PathPoint::new(f64::from(p.x), f64::from(p.y))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_simple_rect() {
        let svg = r#"<?xml version="1.0"?>
        <svg xmlns="http://www.w3.org/2000/svg" width="100" height="50">
            <rect x="10" y="5" width="80" height="40" fill="black"/>
        </svg>"#;
        let paths = import_svg(svg.as_bytes()).expect("import");
        assert_eq!(paths.len(), 1);
        let b = paths[0].bounds();
        assert!((b.min_x - 10.0).abs() < 1e-3);
        assert!((b.min_y - 5.0).abs() < 1e-3);
        assert!((b.max_x - 90.0).abs() < 1e-3);
        assert!((b.max_y - 45.0).abs() < 1e-3);
        assert!(paths[0].closed);
    }

    #[test]
    fn import_circle_produces_curves() {
        let svg = r#"<?xml version="1.0"?>
        <svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
            <circle cx="50" cy="50" r="40" fill="black"/>
        </svg>"#;
        let paths = import_svg(svg.as_bytes()).expect("import");
        assert_eq!(paths.len(), 1);
        let has_curve = paths[0]
            .commands
            .iter()
            .any(|c| matches!(c, PathSegment::CubicTo { .. } | PathSegment::QuadTo { .. }));
        assert!(has_curve, "circle should produce curve segments");
    }

    #[test]
    fn import_with_path_data() {
        let svg = r#"<?xml version="1.0"?>
        <svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
            <path d="M 10 10 L 90 10 L 90 90 L 10 90 Z" fill="black"/>
        </svg>"#;
        let paths = import_svg(svg.as_bytes()).expect("import");
        assert_eq!(paths.len(), 1);
        assert!(paths[0].closed);
    }

    #[test]
    fn import_group_with_transform() {
        let svg = r#"<?xml version="1.0"?>
        <svg xmlns="http://www.w3.org/2000/svg" width="200" height="200">
            <g transform="translate(50,50)">
                <rect x="0" y="0" width="20" height="20" fill="black"/>
            </g>
        </svg>"#;
        let paths = import_svg(svg.as_bytes()).expect("import");
        assert_eq!(paths.len(), 1);
        let b = paths[0].bounds();
        // After translate(50, 50) the rect should be at (50,50) - (70,70).
        assert!((b.min_x - 50.0).abs() < 1e-3, "min_x = {}", b.min_x);
        assert!((b.min_y - 50.0).abs() < 1e-3, "min_y = {}", b.min_y);
    }

    #[test]
    fn import_invalid_returns_parse_error() {
        let err = import_svg(b"not an svg").expect_err("must err");
        assert!(matches!(err, SvgImportError::Parse(_)));
    }

    #[test]
    fn import_empty_svg_is_empty() {
        let svg = r#"<?xml version="1.0"?><svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"/>"#;
        let paths = import_svg(svg.as_bytes()).expect("import");
        assert!(paths.is_empty());
    }
}
