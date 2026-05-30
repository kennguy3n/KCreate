//! AI brand → brochure template — Phase 10 Block D Task 19.
//!
//! Given a brand kit (colours), produce a structural plan for a
//! multi-page brochure layout: a cover page (logo + headline +
//! sub-headline), `n-2` content pages (heading + body + image
//! placeholder), and a back page (contact + colour swatches). The
//! plan is **pure data**: no I/O, no workspace coupling, no Electron
//! state. The bridge (`kcreate_bridge::phase10::ai_brand_to_brochure`)
//! is a thin adapter that loads the brand kit from the workspace,
//! calls [`plan_brochure`], and surfaces the result over N-API.
//!
//! Keeping the algorithm in `kcreate_ai` mirrors the layout of every
//! other Phase 10 AI feature (denoise, inpaint, auto_color,
//! stroke_match, glyph_extract, reformat, one_pager,
//! palette_harmonize, type_pairing) and matches the "Where new code
//! goes" table in `AGENTS.md`.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Default A4 portrait page size at 96 DPI, used when no explicit
/// page size is supplied.
pub const DEFAULT_PAGE_WIDTH: f64 = 794.0;
pub const DEFAULT_PAGE_HEIGHT: f64 = 1123.0;
pub const DEFAULT_PAGE_MARGIN: f64 = 64.0;

/// Lower / upper bound on `num_pages`. A brochure must have at
/// least a cover + back (2), and large brochures past 32 pages stop
/// being a brochure and become a magazine.
pub const MIN_PAGES: u32 = 2;
pub const MAX_PAGES: u32 = 32;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrochureSection {
    pub section_kind: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub style_color_hex: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrochurePage {
    pub index: u32,
    pub page_type: String,
    pub sections: Vec<BrochureSection>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrochurePlan {
    pub pages: Vec<BrochurePage>,
    pub brand_kit_id: String,
}

#[derive(Debug, Clone, Copy)]
pub struct PageGeometry {
    pub width: f64,
    pub height: f64,
    pub margin: f64,
}

impl Default for PageGeometry {
    fn default() -> Self {
        Self {
            width: DEFAULT_PAGE_WIDTH,
            height: DEFAULT_PAGE_HEIGHT,
            margin: DEFAULT_PAGE_MARGIN,
        }
    }
}

#[derive(Debug, Error)]
pub enum BrandTemplateError {
    #[error("brand_template: at least one brand colour is required")]
    NoColors,
}

/// Build a structural brochure plan.
///
/// - `brand_kit_id` is echoed back into the plan so the caller can
///   correlate the plan with the source brand kit; the function does
///   not look anything up.
/// - `colors` is the ordered list of brand colours as hex strings
///   (e.g. `"#0a84ff"`). The first colour is treated as the primary
///   tint, the second as the secondary tint. Additional colours are
///   ignored by this template but are intentionally accepted so the
///   API stays forward-compatible.
/// - `num_pages` is clamped into `[MIN_PAGES, MAX_PAGES]`.
/// - `geometry` controls the page dimensions / margin used for the
///   bounding boxes. The caller can pass [`PageGeometry::default`]
///   for the standard A4-at-96-DPI canvas the renderer ships with.
///
/// The output is fully deterministic — given the same inputs the
/// plan is byte-for-byte stable. Tests rely on that.
pub fn plan_brochure(
    brand_kit_id: &str,
    colors: &[String],
    num_pages: u32,
    geometry: PageGeometry,
) -> Result<BrochurePlan, BrandTemplateError> {
    if colors.is_empty() {
        return Err(BrandTemplateError::NoColors);
    }
    let n = num_pages.clamp(MIN_PAGES, MAX_PAGES);
    let primary = colors.first().cloned();
    let secondary = colors.get(1).cloned();
    let PageGeometry {
        width: page_w,
        height: page_h,
        margin,
    } = geometry;

    let mut pages: Vec<BrochurePage> = Vec::with_capacity(n as usize);

    // Cover.
    pages.push(BrochurePage {
        index: 0,
        page_type: "cover".into(),
        sections: vec![
            BrochureSection {
                section_kind: "logo".into(),
                x: margin,
                y: margin,
                width: 240.0,
                height: 80.0,
                style_color_hex: primary.clone(),
            },
            BrochureSection {
                section_kind: "headline".into(),
                x: margin,
                y: page_h * 0.45,
                width: page_w - 2.0 * margin,
                height: 96.0,
                style_color_hex: primary.clone(),
            },
            BrochureSection {
                section_kind: "subheadline".into(),
                x: margin,
                y: page_h * 0.55,
                width: page_w - 2.0 * margin,
                height: 48.0,
                style_color_hex: secondary.clone(),
            },
        ],
    });

    // Content pages (none when n == 2).
    for i in 1..(n - 1) {
        pages.push(BrochurePage {
            index: i,
            page_type: "content".into(),
            sections: vec![
                BrochureSection {
                    section_kind: "heading".into(),
                    x: margin,
                    y: margin,
                    width: page_w - 2.0 * margin,
                    height: 72.0,
                    style_color_hex: primary.clone(),
                },
                BrochureSection {
                    section_kind: "body".into(),
                    x: margin,
                    y: margin + 96.0,
                    width: (page_w - 2.0 * margin) / 2.0 - 16.0,
                    height: page_h - 2.0 * margin - 96.0,
                    style_color_hex: None,
                },
                BrochureSection {
                    section_kind: "image_placeholder".into(),
                    x: margin + (page_w - 2.0 * margin) / 2.0 + 16.0,
                    y: margin + 96.0,
                    width: (page_w - 2.0 * margin) / 2.0 - 16.0,
                    height: page_h - 2.0 * margin - 96.0,
                    style_color_hex: secondary.clone(),
                },
            ],
        });
    }

    // Back.
    pages.push(BrochurePage {
        index: n - 1,
        page_type: "back".into(),
        sections: vec![
            BrochureSection {
                section_kind: "contact".into(),
                x: margin,
                y: margin,
                width: page_w - 2.0 * margin,
                height: 200.0,
                style_color_hex: primary,
            },
            BrochureSection {
                section_kind: "color_swatches".into(),
                x: margin,
                y: page_h - margin - 96.0,
                width: page_w - 2.0 * margin,
                height: 96.0,
                style_color_hex: secondary,
            },
        ],
    });

    Ok(BrochurePlan {
        pages,
        brand_kit_id: brand_kit_id.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn colors() -> Vec<String> {
        vec!["#0a84ff".into(), "#ff453a".into()]
    }

    #[test]
    fn rejects_empty_palette() {
        let err = plan_brochure("kit-1", &[], 4, PageGeometry::default()).unwrap_err();
        assert!(matches!(err, BrandTemplateError::NoColors));
    }

    #[test]
    fn clamps_below_min_pages() {
        // Asking for 0 or 1 page still produces the minimum cover +
        // back layout.
        for n in [0, 1] {
            let plan = plan_brochure("kit-1", &colors(), n, PageGeometry::default()).unwrap();
            assert_eq!(plan.pages.len(), MIN_PAGES as usize);
            assert_eq!(plan.pages.first().unwrap().page_type, "cover");
            assert_eq!(plan.pages.last().unwrap().page_type, "back");
        }
    }

    #[test]
    fn clamps_above_max_pages() {
        let plan =
            plan_brochure("kit-1", &colors(), MAX_PAGES + 50, PageGeometry::default()).unwrap();
        assert_eq!(plan.pages.len(), MAX_PAGES as usize);
    }

    #[test]
    fn standard_4_page_brochure_has_expected_structure() {
        let plan = plan_brochure("kit-1", &colors(), 4, PageGeometry::default()).unwrap();
        assert_eq!(plan.pages.len(), 4);
        assert_eq!(plan.pages[0].page_type, "cover");
        assert_eq!(plan.pages[1].page_type, "content");
        assert_eq!(plan.pages[2].page_type, "content");
        assert_eq!(plan.pages[3].page_type, "back");
        assert_eq!(plan.brand_kit_id, "kit-1");
        // Cover always has 3 sections; content has 3; back has 2.
        assert_eq!(plan.pages[0].sections.len(), 3);
        assert_eq!(plan.pages[1].sections.len(), 3);
        assert_eq!(plan.pages[3].sections.len(), 2);
    }

    #[test]
    fn primary_color_propagates_to_brand_sections() {
        let plan = plan_brochure("kit-1", &colors(), 3, PageGeometry::default()).unwrap();
        let cover_headline = plan.pages[0]
            .sections
            .iter()
            .find(|s| s.section_kind == "headline")
            .unwrap();
        assert_eq!(cover_headline.style_color_hex.as_deref(), Some("#0a84ff"));
        let back_swatch = plan.pages[2]
            .sections
            .iter()
            .find(|s| s.section_kind == "color_swatches")
            .unwrap();
        assert_eq!(back_swatch.style_color_hex.as_deref(), Some("#ff453a"));
    }

    #[test]
    fn single_color_palette_uses_none_for_secondary() {
        let plan = plan_brochure("kit-1", &["#0a84ff".into()], 2, PageGeometry::default()).unwrap();
        // Sub-headline is the secondary slot on the cover.
        let subhead = plan.pages[0]
            .sections
            .iter()
            .find(|s| s.section_kind == "subheadline")
            .unwrap();
        assert!(subhead.style_color_hex.is_none());
    }

    #[test]
    fn output_is_deterministic() {
        let a = plan_brochure("kit-1", &colors(), 5, PageGeometry::default()).unwrap();
        let b = plan_brochure("kit-1", &colors(), 5, PageGeometry::default()).unwrap();
        let aj = serde_json::to_string(&a).unwrap();
        let bj = serde_json::to_string(&b).unwrap();
        assert_eq!(aj, bj);
    }

    #[test]
    fn geometry_propagates_to_section_bounds() {
        let plan = plan_brochure(
            "kit-1",
            &colors(),
            3,
            PageGeometry {
                width: 1000.0,
                height: 500.0,
                margin: 50.0,
            },
        )
        .unwrap();
        // The headline on the cover stretches to (width - 2*margin).
        let head = plan.pages[0]
            .sections
            .iter()
            .find(|s| s.section_kind == "headline")
            .unwrap();
        assert!((head.width - 900.0).abs() < 1e-6);
        // The contact strip on the back page sits at (margin, margin).
        let contact = plan.pages[2]
            .sections
            .iter()
            .find(|s| s.section_kind == "contact")
            .unwrap();
        assert!((contact.x - 50.0).abs() < 1e-6);
        assert!((contact.y - 50.0).abs() < 1e-6);
    }
}
