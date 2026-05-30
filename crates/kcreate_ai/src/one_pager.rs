//! Brief → one-pager — Phase 10 Block B Task 10.
//!
//! Takes a free-form text brief (plain text or markdown) and
//! generates a single-page Layout Studio document plan: header,
//! body paragraphs, optional callouts, and image placeholders.
//!
//! When the LLM sidecar is available, the bridge replaces this
//! deterministic parser with a GBNF-constrained LLM call. The
//! function here is the always-available fallback used during tests
//! and when no model is installed.

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum OnePagerPageSize {
    Letter,
    #[default]
    A4,
    Square,
    Custom { width: u32, height: u32 },
}


impl OnePagerPageSize {
    /// Return the page's dimensions in pixels (assuming 96 DPI).
    #[must_use]
    pub fn dimensions(self) -> (f64, f64) {
        match self {
            // Letter at 96 DPI: 8.5" x 11" → 816 x 1056
            Self::Letter => (816.0, 1056.0),
            // A4 at 96 DPI: 8.27" x 11.69" → ~794 x 1123
            Self::A4 => (794.0, 1123.0),
            Self::Square => (1024.0, 1024.0),
            Self::Custom { width, height } => (f64::from(width), f64::from(height)),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OnePagerSectionType {
    Header,
    Body,
    Callout,
    ImagePlaceholder,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OnePagerSection {
    pub section_type: OnePagerSectionType,
    pub text: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BriefToOnePagerOptions {
    pub page_size: OnePagerPageSize,
    /// Outer margin in pixels.
    pub margin: f64,
    /// Height reserved for the header. Falls back to a sensible
    /// proportion of the page height when set to 0.
    pub header_height: f64,
    /// Optional brand-kit suggestion seed name. Used only for the
    /// returned summary; the bridge applies the brand kit.
    pub brand_kit_suggestion: Option<u32>, // placeholder for future
}

impl Default for BriefToOnePagerOptions {
    fn default() -> Self {
        Self {
            page_size: OnePagerPageSize::default(),
            margin: 64.0,
            header_height: 0.0,
            brand_kit_suggestion: None,
        }
    }
}

impl BriefToOnePagerOptions {
    #[must_use]
    pub fn clamped(mut self) -> Self {
        let (w, h) = self.page_size.dimensions();
        if !self.margin.is_finite() || self.margin < 0.0 {
            self.margin = 0.0;
        }
        self.margin = self.margin.min(w.min(h) / 4.0);
        if !self.header_height.is_finite() || self.header_height < 0.0 {
            self.header_height = 0.0;
        }
        self.header_height = self.header_height.min(h / 3.0);
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BriefToOnePagerResult {
    pub sections: Vec<OnePagerSection>,
    pub page_width: f64,
    pub page_height: f64,
}

#[derive(Debug, Error)]
pub enum BriefToOnePagerError {
    #[error("brief_to_one_pager: empty brief")]
    Empty,
}

/// Layout a one-pager from a brief.
///
/// Strategy:
///
/// - First non-empty line becomes the header.
/// - Lines beginning with `>` (markdown blockquote) become callouts.
/// - Lines beginning with `![image]` markers become image placeholders.
/// - Remaining lines are concatenated into a single body block.
///
/// # Errors
///
/// Returns [`BriefToOnePagerError::Empty`] when the brief contains
/// no non-whitespace text.
pub fn brief_to_one_pager(
    brief: &str,
    options: BriefToOnePagerOptions,
) -> Result<BriefToOnePagerResult, BriefToOnePagerError> {
    let trimmed = brief.trim();
    if trimmed.is_empty() {
        return Err(BriefToOnePagerError::Empty);
    }
    let opts = options.clamped();
    let (page_w, page_h) = opts.page_size.dimensions();
    let content_x = opts.margin;
    let content_w = (page_w - 2.0 * opts.margin).max(1.0);

    // Find the header.
    let mut header: Option<String> = None;
    let mut callouts: Vec<String> = Vec::new();
    let mut image_placeholders: Vec<String> = Vec::new();
    let mut body_lines: Vec<String> = Vec::new();
    for raw_line in trimmed.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        if header.is_none() {
            // Strip markdown heading marks if present.
            let cleaned = line.trim_start_matches('#').trim();
            header = Some(cleaned.to_string());
            continue;
        }
        if let Some(rest) = line.strip_prefix('>') {
            callouts.push(rest.trim().to_string());
            continue;
        }
        if let Some(rest) = line
            .strip_prefix("![image]")
            .or_else(|| line.strip_prefix("![img]"))
        {
            image_placeholders.push(rest.trim().to_string());
            continue;
        }
        body_lines.push(line.to_string());
    }

    // Compute section heights.
    let header_h = if opts.header_height > 0.0 {
        opts.header_height
    } else {
        page_h * 0.12
    };
    let header_y = opts.margin;
    let mut sections: Vec<OnePagerSection> = Vec::new();
    sections.push(OnePagerSection {
        section_type: OnePagerSectionType::Header,
        text: header.unwrap_or_else(|| "Untitled".into()),
        x: content_x,
        y: header_y,
        width: content_w,
        height: header_h,
    });

    let mut cursor_y = header_y + header_h + opts.margin / 2.0;

    // Image placeholders stack first (visually impactful).
    for img in &image_placeholders {
        let h = (page_h * 0.25).min(page_h - cursor_y - opts.margin);
        if h <= 0.0 {
            break;
        }
        sections.push(OnePagerSection {
            section_type: OnePagerSectionType::ImagePlaceholder,
            text: img.clone(),
            x: content_x,
            y: cursor_y,
            width: content_w,
            height: h,
        });
        cursor_y += h + opts.margin / 4.0;
    }

    // Callouts as right-aligned banners.
    for c in &callouts {
        let h = 80.0;
        if cursor_y + h > page_h - opts.margin {
            break;
        }
        sections.push(OnePagerSection {
            section_type: OnePagerSectionType::Callout,
            text: c.clone(),
            x: content_x,
            y: cursor_y,
            width: content_w,
            height: h,
        });
        cursor_y += h + opts.margin / 4.0;
    }

    // Body fills remaining space.
    if !body_lines.is_empty() {
        let body_text = body_lines.join("\n");
        let body_y = cursor_y;
        let body_h = (page_h - opts.margin - body_y).max(40.0);
        sections.push(OnePagerSection {
            section_type: OnePagerSectionType::Body,
            text: body_text,
            x: content_x,
            y: body_y,
            width: content_w,
            height: body_h,
        });
    }

    Ok(BriefToOnePagerResult {
        sections,
        page_width: page_w,
        page_height: page_h,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_brief_errors() {
        let err = brief_to_one_pager("   ", BriefToOnePagerOptions::default()).unwrap_err();
        assert!(matches!(err, BriefToOnePagerError::Empty));
    }

    #[test]
    fn header_extracted_from_first_line() {
        let r = brief_to_one_pager(
            "My Product Launch\nLine 1 of body\nLine 2 of body",
            BriefToOnePagerOptions::default(),
        )
        .unwrap();
        let header = &r.sections[0];
        assert_eq!(header.section_type, OnePagerSectionType::Header);
        assert_eq!(header.text, "My Product Launch");
    }

    #[test]
    fn markdown_heading_marks_stripped() {
        let r = brief_to_one_pager(
            "## Hello world\nbody",
            BriefToOnePagerOptions::default(),
        )
        .unwrap();
        assert_eq!(r.sections[0].text, "Hello world");
    }

    #[test]
    fn blockquote_becomes_callout() {
        let r = brief_to_one_pager(
            "Title\n> Important callout text\nrest of body",
            BriefToOnePagerOptions::default(),
        )
        .unwrap();
        assert!(r
            .sections
            .iter()
            .any(|s| s.section_type == OnePagerSectionType::Callout
                && s.text == "Important callout text"));
    }

    #[test]
    fn image_marker_becomes_placeholder() {
        let r = brief_to_one_pager(
            "Title\n![image] hero shot\nbody",
            BriefToOnePagerOptions::default(),
        )
        .unwrap();
        assert!(r
            .sections
            .iter()
            .any(|s| s.section_type == OnePagerSectionType::ImagePlaceholder));
    }

    #[test]
    fn body_aggregates_remaining_lines() {
        let r = brief_to_one_pager(
            "Title\nline 1\nline 2\nline 3",
            BriefToOnePagerOptions::default(),
        )
        .unwrap();
        let body = r
            .sections
            .iter()
            .find(|s| s.section_type == OnePagerSectionType::Body)
            .unwrap();
        assert!(body.text.contains("line 1"));
        assert!(body.text.contains("line 2"));
        assert!(body.text.contains("line 3"));
    }

    #[test]
    fn page_dimensions_consistent_with_choice() {
        let (lw, lh) = OnePagerPageSize::Letter.dimensions();
        assert!((lh - 1056.0).abs() < 1.0);
        assert!((lw - 816.0).abs() < 1.0);
        let (sw, sh) = OnePagerPageSize::Square.dimensions();
        assert!((sw - sh).abs() < 1e-6);
    }

    #[test]
    fn custom_page_size_round_trips() {
        let opts = BriefToOnePagerOptions {
            page_size: OnePagerPageSize::Custom {
                width: 2000,
                height: 1500,
            },
            ..Default::default()
        };
        let r = brief_to_one_pager("Title\nbody", opts).unwrap();
        assert!((r.page_width - 2000.0).abs() < 1e-6);
        assert!((r.page_height - 1500.0).abs() < 1e-6);
    }
}
