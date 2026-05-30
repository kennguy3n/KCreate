//! Adobe Illustrator (`.ai`) import — Phase 10 Block C Task 17.
//!
//! Post-CS-era `.ai` files are PDF containers with an Illustrator
//! private-data dictionary plus, in many cases, an embedded SVG
//! payload. We try the SVG path first because it preserves layer
//! names + path semantics that the PDF path-walker would have to
//! reconstruct. If we can't locate the SVG payload, we fall back to
//! the existing PDF importer.
//!
//! Detection strategy:
//!
//! 1. Sniff the first KB for `%PDF-`. If it's missing, treat the
//!    file as raw SVG (legacy `.ai` v8 saves are pure PostScript;
//!    we can't import those, so we return an error in that branch).
//! 2. Scan the file for an `<?xml ... ?>` followed by `<svg`. AI
//!    embeds the SVG between Illustrator markers — we capture the
//!    contiguous SVG document and import it through `usvg`.
//! 3. If no SVG marker is found, delegate to [`crate::pdf_import`].

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiImportSummary {
    /// Which extraction path actually fired.
    pub path: AiImportPath,
    /// Width of the imported document in pt, when known.
    pub width_pt: Option<f64>,
    /// Height of the imported document in pt, when known.
    pub height_pt: Option<f64>,
    /// Number of SVG nodes / PDF pages produced.
    pub object_count: u32,
    /// Optional human-readable hint surfaced to the user.
    pub message: Option<String>,
    /// Raw bytes of the SVG payload (base64-encoded) so the bridge
    /// can hand them to the existing SVG importer without re-reading
    /// the source file. Empty for the PDF-fallback path.
    pub svg_payload_base64: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AiImportPath {
    Svg,
    Pdf,
}

#[derive(Debug, Error)]
pub enum AiImportError {
    #[error("ai_import: file is empty")]
    Empty,
    #[error("ai_import: legacy AI8 / PostScript files are not supported")]
    LegacyPostScript,
    #[error("ai_import: SVG payload is malformed: {0}")]
    BadSvg(String),
    #[error("ai_import: PDF fallback failed: {0}")]
    PdfFallback(String),
}

/// Inspect `bytes` and produce an [`AiImportSummary`] describing how
/// to construct a document from it.
///
/// # Errors
///
/// Returns [`AiImportError::Empty`] for an empty buffer,
/// [`AiImportError::LegacyPostScript`] for pre-CS Illustrator saves
/// that contain no PDF container, [`AiImportError::BadSvg`] when an
/// extracted SVG payload fails to parse, or
/// [`AiImportError::PdfFallback`] when the PDF importer rejects the
/// container.
pub fn import_illustrator_bytes(bytes: &[u8]) -> Result<AiImportSummary, AiImportError> {
    if bytes.is_empty() {
        return Err(AiImportError::Empty);
    }
    let head_len = bytes.len().min(2048);
    let head = std::str::from_utf8(&bytes[..head_len]).unwrap_or("");
    if !head.starts_with("%PDF-") {
        // Legacy AI8 files are PostScript without a PDF wrapper.
        return Err(AiImportError::LegacyPostScript);
    }
    if let Some(svg_payload) = extract_svg_payload(bytes) {
        // Validate by attempting to parse with usvg (re-exported
        // from `resvg`). This catches malformed AI exports before
        // we hand the payload to the SVG importer.
        let opt = resvg::usvg::Options::default();
        let tree = resvg::usvg::Tree::from_data(&svg_payload, &opt)
            .map_err(|e| AiImportError::BadSvg(format!("{e}")))?;
        let size = tree.size();
        let count = count_group(tree.root());
        use base64::Engine as _;
        let payload_b64 = base64::engine::general_purpose::STANDARD.encode(&svg_payload);
        return Ok(AiImportSummary {
            path: AiImportPath::Svg,
            width_pt: Some(f64::from(size.width())),
            height_pt: Some(f64::from(size.height())),
            object_count: count,
            message: Some("Imported SVG payload from Illustrator container".into()),
            svg_payload_base64: payload_b64,
        });
    }
    // PDF fallback — we don't run the full PDF importer here because
    // it needs file-path access. We return a summary describing what
    // the bridge should hand to `pdf_import::import_pdf`.
    let page_count = sniff_pdf_page_count(bytes).unwrap_or(1);
    Ok(AiImportSummary {
        path: AiImportPath::Pdf,
        width_pt: None,
        height_pt: None,
        object_count: page_count,
        message: Some("No SVG payload found; will import via PDF fallback".into()),
        svg_payload_base64: String::new(),
    })
}

fn extract_svg_payload(bytes: &[u8]) -> Option<Vec<u8>> {
    // Two patterns appear in the wild: `<?xml … ?><svg …>…</svg>`
    // and a bare `<svg …>…</svg>` block stuck into the AI private-
    // dictionary stream. We accept either.
    let needle_xml = b"<?xml";
    let needle_svg = b"<svg";
    let close_svg = b"</svg>";

    // Search the whole buffer (file may be large; this is O(n*m)
    // but `m` is 6 bytes so it's fine).
    let start_xml = find_subslice(bytes, needle_xml);
    let start_svg = find_subslice(bytes, needle_svg);
    let end_svg = find_subslice(bytes, close_svg)?;
    let start = match (start_xml, start_svg) {
        (Some(x), Some(s)) => x.min(s),
        (Some(x), None) => x,
        (None, Some(s)) => s,
        (None, None) => return None,
    };
    if start >= end_svg {
        return None;
    }
    let payload = &bytes[start..end_svg + close_svg.len()];
    Some(payload.to_vec())
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|w| w == needle)
}

fn sniff_pdf_page_count(bytes: &[u8]) -> Option<u32> {
    // Count `/Type /Page` occurrences. Not robust but good enough
    // to surface a reasonable page count in the summary; the actual
    // import goes through the rich `pdf_import` path.
    let needle = b"/Type /Page";
    let mut count = 0u32;
    let mut cursor = 0;
    while cursor + needle.len() <= bytes.len() {
        if &bytes[cursor..cursor + needle.len()] == needle {
            count += 1;
            cursor += needle.len();
        } else {
            cursor += 1;
        }
    }
    if count == 0 {
        None
    } else {
        Some(count)
    }
}

fn count_group(group: &resvg::usvg::Group) -> u32 {
    let mut count = 1u32;
    for child in group.children() {
        count += count_node(child);
    }
    count
}

fn count_node(node: &resvg::usvg::Node) -> u32 {
    match node {
        resvg::usvg::Node::Group(g) => count_group(g),
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_errors() {
        assert!(matches!(
            import_illustrator_bytes(&[]).unwrap_err(),
            AiImportError::Empty
        ));
    }

    #[test]
    fn legacy_postscript_rejected() {
        let buf = b"%!PS-Adobe-3.0\n%%For: Adobe Illustrator 8\n";
        assert!(matches!(
            import_illustrator_bytes(buf).unwrap_err(),
            AiImportError::LegacyPostScript
        ));
    }

    #[test]
    fn svg_payload_inside_pdf_container_is_extracted() {
        let mut buf = b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n".to_vec();
        buf.extend_from_slice(b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"100\" height=\"50\"><rect width=\"10\" height=\"10\"/></svg>");
        buf.extend_from_slice(b"\n%%EOF\n");
        let s = import_illustrator_bytes(&buf).unwrap();
        assert_eq!(s.path, AiImportPath::Svg);
        assert_eq!(s.width_pt, Some(100.0));
        assert!(s.object_count >= 1);
        assert!(!s.svg_payload_base64.is_empty());
    }

    #[test]
    fn falls_back_to_pdf_when_no_svg_payload() {
        let mut buf = b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n".to_vec();
        buf.extend_from_slice(b"/Type /Page\n/Type /Page\n");
        buf.extend_from_slice(b"%%EOF\n");
        let s = import_illustrator_bytes(&buf).unwrap();
        assert_eq!(s.path, AiImportPath::Pdf);
        assert_eq!(s.object_count, 2);
    }

    #[test]
    fn malformed_svg_payload_errors() {
        let mut buf = b"%PDF-1.4\n".to_vec();
        // unclosed tag
        buf.extend_from_slice(b"<svg width=\"oops\">unterminated</svg>");
        let err = import_illustrator_bytes(&buf).unwrap_err();
        assert!(matches!(err, AiImportError::BadSvg(_)));
    }
}
