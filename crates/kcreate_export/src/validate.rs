//! Export request validation (Phase 9 Task 27).
//!
//! Surface warnings + errors *before* the export pipeline starts so
//! the user can correct mistakes (e.g. zero-dim artboards, oversized
//! exports, unsupported format pairings) without paying the cost of
//! running a doomed export. This is a pure-data validator — no I/O,
//! no global state — so it is trivially unit-testable.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Format names recognised by the validator. We keep these as a
/// lower-case string on the wire to match the rest of the export
/// surface (`KChatArtifactKind`, etc.).
pub const FORMATS: &[&str] = &["png", "jpeg", "webp", "svg", "pdf", "brandKit"];

/// Default upper bound on a single export dimension. Anything
/// above this triggers a warning unless `force` is set on the
/// request.
pub const DEFAULT_MAX_DIMENSION: u32 = 10_000;

/// Default upper bound on the JPEG quality slider. JPEG quality is
/// `[1, 100]` — anything outside that is rejected as an error.
pub const JPEG_QUALITY_RANGE: std::ops::RangeInclusive<u32> = 1..=100;

#[derive(Debug, Error)]
pub enum ExportValidationError {
    #[error("export request must declare at least one node id")]
    NoNodes,
    #[error("unknown export format '{0}'")]
    UnknownFormat(String),
    #[error("dimension {dim} for axis '{axis}' must be > 0")]
    ZeroDimension { axis: &'static str, dim: u32 },
    #[error("jpeg quality {0} is outside the supported range 1..=100")]
    InvalidJpegQuality(u32),
    #[error("svg + jpeg are incompatible (SVG is vector-only, JPEG is raster-only)")]
    SvgJpegCombination,
    #[error("transparent backgrounds are not supported by JPEG")]
    JpegTransparency,
}

/// An export request. The bridge converts its JSON wire format
/// to this struct before calling [`validate_export_request`].
///
/// The four bool fields each describe an independent attribute of
/// the request (transparency, oversized opt-in, has-text content,
/// missing-fonts probe). Encoding them as separate booleans is
/// idiomatic for an N-API wire-format struct — the renderer
/// constructs the request from scattered React state, not from a
/// single state machine, so a packed enum would just push that
/// branching into the marshalling layer.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportValidationRequest {
    /// One or more node IDs to export. Empty is invalid.
    pub node_ids: Vec<String>,
    /// Target format, lower-case. See [`FORMATS`].
    pub format: String,
    /// Optional explicit output width in pixels. `0` triggers
    /// [`ExportValidationError::ZeroDimension`].
    pub width: Option<u32>,
    /// Optional explicit output height in pixels.
    pub height: Option<u32>,
    /// JPEG quality slider in `[1, 100]`, if format = "jpeg".
    pub jpeg_quality: Option<u32>,
    /// Whether the request wants a transparent background.
    pub transparent: bool,
    /// If true, suppress non-fatal warnings about oversized
    /// dimensions. The user has explicitly opted in to the cost.
    pub force_oversized: bool,
    /// True if any of the selected nodes has text content. The
    /// validator uses this to warn about missing fonts when the
    /// target is a vector format.
    pub has_text: bool,
    /// True if the bridge could not find a system-installed font
    /// that covers every glyph in the selection.
    pub missing_fonts: bool,
}

/// Severity of a validation result entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportSeverity {
    /// Fatal — the export pipeline must not run.
    Error,
    /// Non-fatal — the export can run but the user should be
    /// shown the message first (e.g. "this export is 20000px
    /// wide, are you sure?").
    Warning,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportValidationIssue {
    pub severity: ExportSeverity,
    pub code: String,
    pub message: String,
}

/// Validation result. Carries every error AND every warning so the
/// UI can decide whether to block or just show a banner.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportValidationReport {
    pub ok: bool,
    pub issues: Vec<ExportValidationIssue>,
}

/// Run validation. Returns a report carrying every issue we found.
/// Use [`ExportValidationReport::ok`] to gate the export pipeline.
pub fn validate_export_request(req: &ExportValidationRequest) -> ExportValidationReport {
    let mut issues: Vec<ExportValidationIssue> = Vec::new();
    if req.node_ids.is_empty() {
        issues.push(error(
            "NO_NODES",
            ExportValidationError::NoNodes.to_string(),
        ));
    }
    let fmt_norm = req.format.to_ascii_lowercase();
    if !FORMATS.iter().any(|f| f.eq_ignore_ascii_case(&fmt_norm)) {
        issues.push(error(
            "UNKNOWN_FORMAT",
            ExportValidationError::UnknownFormat(req.format.clone()).to_string(),
        ));
    }
    if let Some(w) = req.width {
        if w == 0 {
            issues.push(error(
                "ZERO_WIDTH",
                ExportValidationError::ZeroDimension {
                    axis: "width",
                    dim: 0,
                }
                .to_string(),
            ));
        } else if w > DEFAULT_MAX_DIMENSION && !req.force_oversized {
            issues.push(warning(
                "OVERSIZED_WIDTH",
                format!(
                    "width {w}px exceeds {DEFAULT_MAX_DIMENSION}px — confirm the override flag"
                ),
            ));
        }
    }
    if let Some(h) = req.height {
        if h == 0 {
            issues.push(error(
                "ZERO_HEIGHT",
                ExportValidationError::ZeroDimension {
                    axis: "height",
                    dim: 0,
                }
                .to_string(),
            ));
        } else if h > DEFAULT_MAX_DIMENSION && !req.force_oversized {
            issues.push(warning(
                "OVERSIZED_HEIGHT",
                format!(
                    "height {h}px exceeds {DEFAULT_MAX_DIMENSION}px — confirm the override flag"
                ),
            ));
        }
    }
    if fmt_norm == "jpeg" {
        if let Some(q) = req.jpeg_quality {
            if !JPEG_QUALITY_RANGE.contains(&q) {
                issues.push(error(
                    "INVALID_JPEG_QUALITY",
                    ExportValidationError::InvalidJpegQuality(q).to_string(),
                ));
            } else if q < 60 {
                issues.push(warning(
                    "LOW_JPEG_QUALITY",
                    format!("JPEG quality {q} will produce visible compression artefacts"),
                ));
            }
        }
        if req.transparent {
            issues.push(error(
                "JPEG_TRANSPARENT",
                ExportValidationError::JpegTransparency.to_string(),
            ));
        }
    }
    if fmt_norm == "svg" && req.has_text && req.missing_fonts {
        issues.push(warning(
            "SVG_MISSING_FONT",
            "SVG export references text glyphs whose font is not installed; viewers may substitute fallback fonts".into(),
        ));
    }
    if fmt_norm == "svg" && req.format.eq_ignore_ascii_case("jpeg") {
        // unreachable in practice but guards against caller bugs.
        issues.push(error(
            "SVG_JPEG_MIX",
            ExportValidationError::SvgJpegCombination.to_string(),
        ));
    }
    let ok = !issues
        .iter()
        .any(|i| matches!(i.severity, ExportSeverity::Error));
    ExportValidationReport { ok, issues }
}

fn error(code: &str, message: String) -> ExportValidationIssue {
    ExportValidationIssue {
        severity: ExportSeverity::Error,
        code: code.to_string(),
        message,
    }
}

fn warning(code: &str, message: String) -> ExportValidationIssue {
    ExportValidationIssue {
        severity: ExportSeverity::Warning,
        code: code.to_string(),
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(format: &str) -> ExportValidationRequest {
        ExportValidationRequest {
            node_ids: vec!["node-1".into()],
            format: format.into(),
            width: Some(512),
            height: Some(512),
            jpeg_quality: None,
            transparent: false,
            force_oversized: false,
            has_text: false,
            missing_fonts: false,
        }
    }

    #[test]
    fn happy_png_validates() {
        let r = validate_export_request(&req("png"));
        assert!(r.ok, "expected ok, got {r:?}");
        assert!(r.issues.is_empty());
    }

    #[test]
    fn empty_node_ids_fails() {
        let mut r = req("png");
        r.node_ids.clear();
        let report = validate_export_request(&r);
        assert!(!report.ok);
        assert!(report
            .issues
            .iter()
            .any(|i| i.code == "NO_NODES" && i.severity == ExportSeverity::Error));
    }

    #[test]
    fn unknown_format_fails() {
        let r = req("bmp");
        let report = validate_export_request(&r);
        assert!(!report.ok);
        assert!(report.issues.iter().any(|i| i.code == "UNKNOWN_FORMAT"));
    }

    #[test]
    fn zero_dimension_fails() {
        let mut r = req("png");
        r.width = Some(0);
        let report = validate_export_request(&r);
        assert!(!report.ok);
        assert!(report.issues.iter().any(|i| i.code == "ZERO_WIDTH"));
    }

    #[test]
    fn oversized_dim_warns_unless_forced() {
        let mut r = req("png");
        r.width = Some(20000);
        let report = validate_export_request(&r);
        assert!(report.ok);
        assert!(report.issues.iter().any(|i| i.code == "OVERSIZED_WIDTH"));
        // Force flag suppresses the warning.
        r.force_oversized = true;
        let report = validate_export_request(&r);
        assert!(report.issues.is_empty());
    }

    #[test]
    fn jpeg_transparency_fails() {
        let mut r = req("jpeg");
        r.transparent = true;
        let report = validate_export_request(&r);
        assert!(!report.ok);
        assert!(report.issues.iter().any(|i| i.code == "JPEG_TRANSPARENT"));
    }

    #[test]
    fn jpeg_quality_out_of_range_fails() {
        let mut r = req("jpeg");
        r.jpeg_quality = Some(150);
        let report = validate_export_request(&r);
        assert!(!report.ok);
        assert!(report
            .issues
            .iter()
            .any(|i| i.code == "INVALID_JPEG_QUALITY"));
    }

    #[test]
    fn jpeg_low_quality_warns() {
        let mut r = req("jpeg");
        r.jpeg_quality = Some(40);
        let report = validate_export_request(&r);
        assert!(report.ok);
        assert!(report.issues.iter().any(|i| i.code == "LOW_JPEG_QUALITY"));
    }

    #[test]
    fn svg_missing_font_warns() {
        let mut r = req("svg");
        r.has_text = true;
        r.missing_fonts = true;
        let report = validate_export_request(&r);
        assert!(report.ok);
        assert!(report.issues.iter().any(|i| i.code == "SVG_MISSING_FONT"));
    }
}
