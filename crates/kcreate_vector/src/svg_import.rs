//! SVG import — backed by [`usvg`].
//!
//! `usvg` does the heavy lifting: it normalises shapes (rects, circles,
//! polygons, `<use>` etc.) into a clean tree of `Path` nodes with
//! absolute transforms and resolved CSS. We walk that tree, transform
//! each path's points into final coordinates, and emit one
//! [`VectorPath`] per `<path>` element.

use std::fs;
use std::path::Path;

use kcreate_core::RgbaColor;
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

/// A stroke paint resolved from an SVG `<path>`'s `stroke` /
/// `stroke-width` / `stroke-opacity`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StyledStroke {
    /// Stroke colour with `stroke-opacity` folded into the alpha
    /// channel.
    pub color: RgbaColor,
    /// Stroke width in the SVG's user units (already absolute — the
    /// element's transform scale is baked in by usvg).
    pub width: f64,
}

/// A single imported path together with the paint resolved from the
/// source SVG. Unlike [`import_svg`], this preserves the fill / stroke
/// colours so a bundled asset can be stamped onto the canvas as
/// editable, *recolorable* vector nodes that look like the original
/// artwork instead of defaulting to an opaque white fill.
#[derive(Debug, Clone, PartialEq)]
pub struct StyledPath {
    pub path: VectorPath,
    /// Resolved fill colour (`fill-opacity` folded into alpha), or
    /// `None` for `fill="none"` / pattern paints.
    pub fill: Option<RgbaColor>,
    /// Resolved stroke, or `None` when the element is unstroked.
    pub stroke: Option<StyledStroke>,
}

/// Parse an SVG byte buffer into a flat list of [`VectorPath`]s. Groups
/// are flattened, transforms are baked in. Paint is discarded — callers
/// that need fill / stroke colours should use [`import_svg_styled`].
///
/// This deliberately does **not** route through [`import_svg_styled`]:
/// resolving each path's fill / stroke paint (gradient-stop lookup,
/// opacity folding, stroke-width scale) is wasted work when the caller
/// only wants geometry. Both entry points share the tree parse and the
/// recursive walk; they differ only in the per-path converter.
pub fn import_svg(svg_data: &[u8]) -> Result<Vec<VectorPath>, SvgImportError> {
    let tree = parse_tree(svg_data)?;
    let mut out = Vec::new();
    walk_paths(tree.root(), &mut out, &convert_path);
    Ok(out)
}

/// Parse an SVG byte buffer into a flat list of [`StyledPath`]s,
/// preserving each path's resolved fill / stroke paint. Groups are
/// flattened and transforms (including scale) are baked into both the
/// geometry and the stroke width.
pub fn import_svg_styled(svg_data: &[u8]) -> Result<Vec<StyledPath>, SvgImportError> {
    let tree = parse_tree(svg_data)?;
    let mut out = Vec::new();
    walk_paths(tree.root(), &mut out, &convert_path_styled);
    Ok(out)
}

/// Parse an SVG file from disk.
pub fn import_svg_file(path: &Path) -> Result<Vec<VectorPath>, SvgImportError> {
    let bytes = fs::read(path)?;
    import_svg(&bytes)
}

/// Parse raw SVG bytes into a usvg tree with the default options.
fn parse_tree(svg_data: &[u8]) -> Result<usvg::Tree, SvgImportError> {
    let opt = usvg::Options::default();
    usvg::Tree::from_data(svg_data, &opt).map_err(|e| SvgImportError::Parse(e.to_string()))
}

/// Recursively flatten a usvg group, pushing one `T` per `<path>` for
/// which `convert` yields `Some`. Generic over the per-path converter
/// so geometry-only ([`convert_path`]) and styled ([`convert_path_styled`])
/// imports share the same traversal without the geometry-only path
/// paying for discarded paint resolution.
fn walk_paths<T>(
    group: &usvg::Group,
    out: &mut Vec<T>,
    convert: &impl Fn(&usvg::Path) -> Option<T>,
) {
    for node in group.children() {
        match node {
            usvg::Node::Group(g) => walk_paths(g, out, convert),
            usvg::Node::Path(p) => {
                if let Some(v) = convert(p) {
                    out.push(v);
                }
            }
            // Images / text are not vector paths; Phase 0 ignores them.
            usvg::Node::Image(_) | usvg::Node::Text(_) => {}
        }
    }
}

fn convert_path_styled(usvg_path: &usvg::Path) -> Option<StyledPath> {
    let path = convert_path(usvg_path)?;
    let fill = usvg_path
        .fill()
        .and_then(|f| paint_to_rgba(f.paint(), f.opacity().get()));
    let stroke = usvg_path.stroke().and_then(|s| {
        paint_to_rgba(s.paint(), s.opacity().get()).map(|color| StyledStroke {
            color,
            width: f64::from(s.width().get()) * abs_scale(&usvg_path.abs_transform()),
        })
    });
    Some(StyledPath { path, fill, stroke })
}

/// Resolve a usvg [`Paint`](usvg::Paint) plus an opacity in `[0, 1]`
/// into an [`RgbaColor`]. Solid colours map directly; gradients fall
/// back to their first stop (a reasonable single-colour approximation
/// for a recolorable node); patterns have no single colour and yield
/// `None`.
fn paint_to_rgba(paint: &usvg::Paint, opacity: f32) -> Option<RgbaColor> {
    let color = match paint {
        usvg::Paint::Color(c) => *c,
        usvg::Paint::LinearGradient(g) => g.stops().first().map(usvg::Stop::color)?,
        usvg::Paint::RadialGradient(g) => g.stops().first().map(usvg::Stop::color)?,
        usvg::Paint::Pattern(_) => return None,
    };
    Some(RgbaColor::new(
        f32::from(color.red) / 255.0,
        f32::from(color.green) / 255.0,
        f32::from(color.blue) / 255.0,
        opacity.clamp(0.0, 1.0),
    ))
}

/// Average absolute scale factor of a transform, used to bake the
/// element scale into the stroke width (usvg leaves `stroke-width` in
/// pre-transform units). `sqrt(|det|)` is the uniform-scale equivalent
/// of the (possibly anisotropic) linear part.
fn abs_scale(xf: &usvg::Transform) -> f64 {
    let det = f64::from(xf.sx) * f64::from(xf.sy) - f64::from(xf.kx) * f64::from(xf.ky);
    det.abs().sqrt()
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
    fn styled_import_captures_fill_color() {
        let svg = r##"<?xml version="1.0"?>
        <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">
            <rect x="2" y="2" width="20" height="20" fill="#3366cc"/>
        </svg>"##;
        let styled = import_svg_styled(svg.as_bytes()).expect("import");
        assert_eq!(styled.len(), 1);
        let fill = styled[0].fill.expect("fill colour captured");
        assert!((fill.r - 0.2).abs() < 0.02, "r = {}", fill.r);
        assert!((fill.g - 0.4).abs() < 0.02, "g = {}", fill.g);
        assert!((fill.b - 0.8).abs() < 0.02, "b = {}", fill.b);
        assert!((fill.a - 1.0).abs() < 1e-3);
        assert!(styled[0].stroke.is_none(), "rect has no stroke");
    }

    #[test]
    fn styled_import_captures_stroke_and_no_fill() {
        let svg = r##"<?xml version="1.0"?>
        <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">
            <path d="M4 12 L20 12" fill="none" stroke="#000000" stroke-width="2"/>
        </svg>"##;
        let styled = import_svg_styled(svg.as_bytes()).expect("import");
        assert_eq!(styled.len(), 1);
        assert!(styled[0].fill.is_none(), "fill=none yields no fill");
        let stroke = styled[0].stroke.expect("stroke captured");
        assert!(
            (stroke.width - 2.0).abs() < 1e-3,
            "width = {}",
            stroke.width
        );
        assert!((stroke.color.r).abs() < 1e-3 && (stroke.color.a - 1.0).abs() < 1e-3);
    }

    #[test]
    fn styled_import_folds_fill_opacity_into_alpha() {
        let svg = r##"<?xml version="1.0"?>
        <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">
            <rect x="0" y="0" width="24" height="24" fill="#ff0000" fill-opacity="0.5"/>
        </svg>"##;
        let styled = import_svg_styled(svg.as_bytes()).expect("import");
        let fill = styled[0].fill.expect("fill colour captured");
        assert!((fill.a - 0.5).abs() < 0.01, "alpha = {}", fill.a);
    }

    #[test]
    fn invalid_returns_parse_error() {
        let err = import_svg(b"not an svg").expect_err("must err");
        assert!(matches!(err, SvgImportError::Parse(_)));
    }

    #[test]
    fn import_empty_svg_is_empty() {
        let svg = r#"<?xml version="1.0"?><svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"/>"#;
        let paths = import_svg(svg.as_bytes()).expect("import");
        assert!(paths.is_empty());
    }

    /// Every asset in the bundled Elements library must parse into at
    /// least one styled vector path, and every emitted path must carry a
    /// paint (fill or stroke) — otherwise it would insert as an invisible
    /// node. This is the importer-side guard for the offline library.
    #[test]
    fn every_bundled_asset_parses_into_painted_paths() {
        for asset in kcreate_core::assets::catalog() {
            let styled = import_svg_styled(asset.svg.as_bytes())
                .unwrap_or_else(|e| panic!("asset {:?} failed to parse: {e:?}", asset.id));
            assert!(
                !styled.is_empty(),
                "asset {:?} produced no vector paths",
                asset.id
            );
            for sp in &styled {
                assert!(
                    !sp.path.commands.is_empty(),
                    "asset {:?} produced an empty path",
                    asset.id
                );
                assert!(
                    sp.fill.is_some() || sp.stroke.is_some(),
                    "asset {:?} produced a path with no paint",
                    asset.id
                );
            }
        }
    }
}
