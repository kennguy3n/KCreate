//! SVG export — produces clean, developer-friendly SVG.
//!
//! No `<g>` wrappers, no `transform=` attributes, no redundant
//! attributes. Numeric output is formatted with up to 4 decimal places
//! (configurable via [`SvgWriteOptions::precision`]).

use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use thiserror::Error;

use crate::path::{FillRule, PathSegment, VectorPath};

/// Errors from SVG export.
#[derive(Debug, Error)]
pub enum SvgExportError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("fmt: {0}")]
    Fmt(#[from] std::fmt::Error),
}

/// Output formatting options.
#[derive(Debug, Clone, Copy)]
pub struct SvgWriteOptions {
    /// Decimal precision for coordinates. Default 4.
    pub precision: u8,
}

impl Default for SvgWriteOptions {
    fn default() -> Self {
        Self { precision: 4 }
    }
}

/// Render `paths` into a complete `<svg>` document of the given pixel
/// dimensions.
#[must_use]
pub fn export_svg(paths: &[VectorPath], width: f64, height: f64) -> String {
    export_svg_with_options(paths, width, height, &SvgWriteOptions::default())
}

/// Render `paths` with custom options.
#[must_use]
pub fn export_svg_with_options(
    paths: &[VectorPath],
    width: f64,
    height: f64,
    opts: &SvgWriteOptions,
) -> String {
    let mut s = String::new();
    let _ = write!(
        s,
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{}\" height=\"{}\" viewBox=\"0 0 {} {}\">",
        format_num(width, opts.precision),
        format_num(height, opts.precision),
        format_num(width, opts.precision),
        format_num(height, opts.precision),
    );
    for p in paths {
        write_path(&mut s, p, *opts);
    }
    s.push_str("</svg>");
    s
}

/// Write `paths` to `path` as an SVG file. Returns
/// [`SvgExportError::Io`] on I/O failure.
pub fn export_svg_to_file(
    paths: &[VectorPath],
    width: f64,
    height: f64,
    path: &Path,
) -> Result<(), SvgExportError> {
    let svg = export_svg(paths, width, height);
    fs::write(path, svg)?;
    Ok(())
}

fn write_path(out: &mut String, p: &VectorPath, opts: SvgWriteOptions) {
    if p.is_empty() {
        return;
    }
    out.push_str("<path d=\"");
    let mut prev_op = '\0';
    for cmd in &p.commands {
        match *cmd {
            PathSegment::MoveTo(pt) => {
                if prev_op != '\0' {
                    out.push(' ');
                }
                let _ = write!(
                    out,
                    "M{} {}",
                    format_num(pt.x, opts.precision),
                    format_num(pt.y, opts.precision)
                );
                prev_op = 'M';
            }
            PathSegment::LineTo(pt) => {
                out.push(' ');
                if prev_op == 'L' {
                    let _ = write!(
                        out,
                        "{} {}",
                        format_num(pt.x, opts.precision),
                        format_num(pt.y, opts.precision)
                    );
                } else {
                    let _ = write!(
                        out,
                        "L{} {}",
                        format_num(pt.x, opts.precision),
                        format_num(pt.y, opts.precision)
                    );
                    prev_op = 'L';
                }
            }
            PathSegment::QuadTo { ctrl, end } => {
                out.push(' ');
                let _ = write!(
                    out,
                    "Q{} {} {} {}",
                    format_num(ctrl.x, opts.precision),
                    format_num(ctrl.y, opts.precision),
                    format_num(end.x, opts.precision),
                    format_num(end.y, opts.precision)
                );
                prev_op = 'Q';
            }
            PathSegment::CubicTo { ctrl1, ctrl2, end } => {
                out.push(' ');
                let _ = write!(
                    out,
                    "C{} {} {} {} {} {}",
                    format_num(ctrl1.x, opts.precision),
                    format_num(ctrl1.y, opts.precision),
                    format_num(ctrl2.x, opts.precision),
                    format_num(ctrl2.y, opts.precision),
                    format_num(end.x, opts.precision),
                    format_num(end.y, opts.precision)
                );
                prev_op = 'C';
            }
            PathSegment::Close => {
                out.push_str(" Z");
                prev_op = 'Z';
            }
        }
    }
    out.push('"');
    if matches!(p.fill_rule, FillRule::EvenOdd) {
        out.push_str(" fill-rule=\"evenodd\"");
    }
    out.push_str("/>");
}

fn format_num(value: f64, precision: u8) -> String {
    if value.is_finite() {
        let p = precision as usize;
        let s = format!("{value:.p$}");
        // Trim trailing zeros / lone dot.
        if s.contains('.') {
            let trimmed = s.trim_end_matches('0').trim_end_matches('.');
            if trimmed.is_empty() || trimmed == "-" {
                "0".to_string()
            } else {
                trimmed.to_string()
            }
        } else {
            s
        }
    } else {
        // Non-finite values are not representable in SVG; clamp to 0.
        "0".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path::PathPoint;

    fn rect() -> VectorPath {
        VectorPath::new(vec![
            PathSegment::MoveTo(PathPoint::new(0.0, 0.0)),
            PathSegment::LineTo(PathPoint::new(10.0, 0.0)),
            PathSegment::LineTo(PathPoint::new(10.0, 5.0)),
            PathSegment::LineTo(PathPoint::new(0.0, 5.0)),
            PathSegment::Close,
        ])
    }

    #[test]
    fn exports_valid_svg_header() {
        let svg = export_svg(&[rect()], 10.0, 5.0);
        assert!(svg.starts_with("<?xml"));
        assert!(svg.contains("xmlns=\"http://www.w3.org/2000/svg\""));
        assert!(svg.contains("viewBox=\"0 0 10 5\""));
        assert!(svg.ends_with("</svg>"));
    }

    #[test]
    fn exports_path_d_attribute() {
        let svg = export_svg(&[rect()], 10.0, 5.0);
        assert!(svg.contains("M0 0"));
        assert!(svg.contains('Z'));
    }

    #[test]
    fn roundtrip_with_svg_import() {
        let svg = export_svg(&[rect()], 10.0, 5.0);
        let parsed = crate::svg_import::import_svg(svg.as_bytes()).expect("import");
        assert_eq!(parsed.len(), 1);
        let b1 = rect().bounds();
        let b2 = parsed[0].bounds();
        assert!((b1.min_x - b2.min_x).abs() < 1e-3);
        assert!((b1.max_x - b2.max_x).abs() < 1e-3);
        assert!((b1.min_y - b2.min_y).abs() < 1e-3);
        assert!((b1.max_y - b2.max_y).abs() < 1e-3);
    }

    #[test]
    fn empty_path_is_omitted() {
        let p = VectorPath::new(Vec::new());
        let svg = export_svg(&[p], 100.0, 100.0);
        assert!(!svg.contains("<path"));
    }

    #[test]
    fn even_odd_fill_rule_emitted() {
        let mut p = rect();
        p.fill_rule = FillRule::EvenOdd;
        let svg = export_svg(&[p], 10.0, 5.0);
        assert!(svg.contains("fill-rule=\"evenodd\""));
    }

    #[test]
    fn precision_is_respected() {
        let p = VectorPath::new(vec![
            PathSegment::MoveTo(PathPoint::new(0.123_456, 0.123_456)),
            PathSegment::LineTo(PathPoint::new(1.0, 0.0)),
        ]);
        let svg = export_svg_with_options(&[p], 10.0, 10.0, &SvgWriteOptions { precision: 2 });
        assert!(svg.contains("M0.12 0.12"));
    }

    #[test]
    fn export_svg_to_file_writes_disk() {
        let p = rect();
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("out.svg");
        export_svg_to_file(&[p], 10.0, 5.0, &path).expect("write");
        let on_disk = std::fs::read_to_string(&path).expect("read");
        assert!(on_disk.starts_with("<?xml"));
    }
}
