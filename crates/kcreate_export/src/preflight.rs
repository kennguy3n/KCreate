//! PDF preflight engine — print-readiness checks.
//!
//! Walks the document graph and reports issues that would degrade a
//! professional print run: insufficient bleed, missing fonts,
//! low-resolution raster images, RGB-only colors when CMYK is
//! intended, unintended transparency, or non-standard page sizes.
//!
//! The engine is pure: it inspects the graph and returns a
//! `Vec<PreflightIssue>`. UI surfaces presentation; no file I/O.

use std::sync::{Arc, OnceLock};

use parking_lot::RwLock;

use kcreate_core::document::DocumentGraph;
use kcreate_core::node::{
    BlendMode, FillStyle, Node, NodeType, PageLayout, PageSize, PAGE_LAYOUT_METADATA_KEY,
};
use kcreate_text::FontManager;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::scene_metadata::{raster_image_meta, text_layer_meta};

#[cfg(test)]
use crate::scene_metadata::{RASTER_IMAGE_METADATA_KEY, TEXT_LAYER_METADATA_KEY};

/// 25.4 mm in 1 inch — used to convert between physical and pixel
/// units when only one is known.
const MM_PER_INCH: f64 = 25.4;

/// Default DPI assumption when the document has no [`PageLayout`]
/// metadata; matches the 300 DPI artboard presets in
/// [`kcreate_core::node::standard_presets`].
const FALLBACK_PRINT_DPI: f64 = 300.0;

/// Severity of a [`PreflightIssue`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreflightSeverity {
    /// Blocking issue — must be fixed before print.
    Error,
    /// Likely to degrade output quality.
    Warning,
    /// Informational note (e.g. non-standard but intentional size).
    Info,
}

/// Which preflight check produced an issue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreflightCheck {
    BleedMargin,
    FontEmbed,
    ImageResolution,
    ColorSpace,
    Transparency,
    PageSize,
    /// Gradient / PDF shading-pattern validity. Covers issues that
    /// would either crash the Phase 2 / Phase 4 PDF shading
    /// injector (`kcreate_export::pdf_shading::inject_shadings`) or
    /// produce a wrong-looking export: too-few stops, unsorted /
    /// out-of-range offsets, identical-color degenerate stops, and
    /// CMYK overrides on an RGB-only export target (which the
    /// injector silently flattens to DeviceRGB).
    Shading,
    /// Per-codepoint glyph-coverage check for `TextLayer` nodes.
    /// The `FontEmbed` check only verifies the *family* resolves;
    /// this one verifies the resolved face actually carries glyphs
    /// for every codepoint in the rendered text. A font that
    /// resolves but lacks (say) the apostrophe codepoint will
    /// silently substitute `.notdef` (a hollow rectangle) at print
    /// time — exactly the kind of "looks fine on screen, broken on
    /// the proof" failure preflight exists to catch.
    FontGlyphCoverage,
    /// Total ink coverage (TIC) check for CMYK fills + CMYK
    /// gradients. The default cap is 300% (GRACoL / SWOP commercial
    /// offset) — exceeding it causes drying / blocking issues on
    /// press. Configurable via
    /// [`PreflightOptions::target_total_ink_coverage`].
    TotalInkCoverage,
}

impl PreflightCheck {
    /// Stable string id for UI badges / persistence.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::BleedMargin => "bleed_margin",
            Self::FontEmbed => "font_embed",
            Self::ImageResolution => "image_resolution",
            Self::ColorSpace => "color_space",
            Self::Transparency => "transparency",
            Self::PageSize => "page_size",
            Self::Shading => "shading",
            Self::FontGlyphCoverage => "font_glyph_coverage",
            Self::TotalInkCoverage => "total_ink_coverage",
        }
    }
}

/// A single issue produced by [`run_preflight`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreflightIssue {
    pub check: PreflightCheck,
    pub severity: PreflightSeverity,
    pub message: String,
    pub affected_node_id: Option<Uuid>,
    /// Optional id of the page the issue applies to (useful for the
    /// UI to scope-jump).
    pub page_id: Option<Uuid>,
}

/// Target color space for the preflight run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ColorSpaceTarget {
    /// Print run — RGB fills will produce warnings.
    #[default]
    Cmyk,
    /// Screen output — RGB fills are expected.
    Rgb,
}

/// Tunables for a preflight run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(clippy::derive_partial_eq_without_eq)]
#[serde(default, rename_all = "camelCase")]
pub struct PreflightOptions {
    /// Target resolution in dots per inch for raster images.
    pub target_dpi: f64,
    /// Required bleed in millimetres beyond the page edge.
    pub require_bleed_mm: f64,
    /// Whether transparent / non-Normal blend layers are acceptable.
    pub allow_transparency: bool,
    /// Color space the output is being prepared for.
    pub target_color_space: ColorSpaceTarget,
    /// Total ink coverage cap as a fraction (1.0 = 100%, 3.0 = 300%).
    /// 300% is the GRACoL / SWOP commercial offset default; web /
    /// newsprint targets use lower caps (240% — 280%). The check
    /// fires when a CMYK fill's component sum exceeds this value;
    /// gradient stops are checked individually.
    pub target_total_ink_coverage: f64,
}

impl Default for PreflightOptions {
    fn default() -> Self {
        Self {
            target_dpi: 300.0,
            require_bleed_mm: 3.0,
            allow_transparency: false,
            target_color_space: ColorSpaceTarget::Cmyk,
            target_total_ink_coverage: 3.0,
        }
    }
}

/// Backing storage for the cached [`FontManager`]. Wrapped in a
/// `RwLock<Option<...>>` so [`clear_cached_font_manager`] can drop the
/// instance — without resetting the `OnceLock` itself, which Rust does
/// not allow.
///
/// Uses `parking_lot::RwLock`, not `std::sync::RwLock`. `parking_lot`
/// does not poison on panic, which matters here because every other
/// long-lived lock in the workspace (`document.rs::slot`,
/// `phase2.rs::batch_table`, `wasm_runtime.rs::module_cache`, …)
/// also uses `parking_lot`. Mixing the two would mean a panic inside
/// `FontManager::new()` permanently bricks the preflight panel for
/// the lifetime of the process even though the rest of the editor
/// keeps running. Per Devin Review 3289344249.
fn font_manager_cell() -> &'static RwLock<Option<Arc<FontManager>>> {
    static CACHE: OnceLock<RwLock<Option<Arc<FontManager>>>> = OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(None))
}

/// Process-wide cached `FontManager`. `FontManager::new()` scans the
/// entire system font tree (~50ms on macOS, ~200ms on Windows). The
/// preflight panel calls `run_preflight` once per click, and many
/// clicks per session, so rebuilding the manager every time burned
/// real wall-time. We share an `Arc<FontManager>` keyed in
/// [`font_manager_cell`] so [`clear_cached_font_manager`] can drop it
/// (e.g. after the user installs a new font and re-opens the panel).
fn cached_font_manager() -> Arc<FontManager> {
    let cell = font_manager_cell();
    if let Some(existing) = cell.read().as_ref() {
        return existing.clone();
    }
    let mut guard = cell.write();
    if let Some(existing) = guard.as_ref() {
        return existing.clone();
    }
    let mgr = Arc::new(FontManager::new());
    *guard = Some(mgr.clone());
    mgr
}

/// Drop the cached [`FontManager`]. The next [`run_preflight`] call
/// will re-scan the system font directories. Call this after the user
/// indicates they have installed or removed fonts (e.g. via a "Rescan
/// fonts" action in the preflight panel).
pub fn clear_cached_font_manager() {
    *font_manager_cell().write() = None;
}

/// Run preflight against the supplied pages. When `pages` is empty,
/// every `Page` node in `document` is checked.
///
/// Returns the issues in the order they were produced: by page, then
/// by check, then by node id (`Uuid`'s `Ord` is byte-wise stable).
#[must_use]
pub fn run_preflight(
    document: &DocumentGraph,
    pages: &[Uuid],
    options: &PreflightOptions,
) -> Vec<PreflightIssue> {
    let page_ids = if pages.is_empty() {
        document
            .iter()
            .filter(|(_, n)| n.node_type == NodeType::Page)
            .map(|(id, _)| *id)
            .collect::<Vec<_>>()
    } else {
        pages.to_vec()
    };

    let mut issues: Vec<PreflightIssue> = Vec::new();
    // `FontManager::new()` scans the entire system font directory tree,
    // which can take ~50ms on macOS and >200ms on Windows. The bridge
    // calls `run_preflight` once per "Run Preflight" click, so we share
    // a single instance for the lifetime of the process. The font list
    // is effectively static while the app is running; users who install
    // new fonts can restart the app or invalidate via
    // `clear_cached_font_manager`.
    let fonts = cached_font_manager();
    for page_id in page_ids {
        let Some(page) = document.get_node(page_id) else {
            continue;
        };
        if page.node_type != NodeType::Page {
            continue;
        }
        let layout = read_page_layout(page);
        let dims = page_dimensions(page, layout.as_ref());

        check_page_size(layout.as_ref(), page_id, &mut issues);

        let descendants = collect_descendants(document, page_id);
        for node_id in &descendants {
            let Some(node) = document.get_node(*node_id) else {
                continue;
            };
            check_node_for_bleed(node, page_id, &dims, options, &mut issues);
            check_node_color_space(node, page_id, options, &mut issues);
            check_node_transparency(node, page_id, options, &mut issues);
            check_node_shading(node, page_id, options, &mut issues);
            check_node_total_ink_coverage(node, page_id, options, &mut issues);
            if matches!(node.node_type, NodeType::TextLayer) {
                check_node_font_embed(node, page_id, fonts.as_ref(), &mut issues);
            }
            if matches!(node.node_type, NodeType::RasterLayer) {
                check_node_image_resolution(node, page_id, &dims, options, &mut issues);
            }
        }
    }
    issues
}

/// Dimensions of a page, both physical (mm) and in document pixels.
#[derive(Debug, Clone, Copy)]
struct PageDimensions {
    width_px: f64,
    height_px: f64,
    px_per_mm: f64,
}

impl PageDimensions {
    fn px_to_mm(self, px: f64) -> f64 {
        if self.px_per_mm > 0.0 {
            px / self.px_per_mm
        } else {
            0.0
        }
    }
}

fn read_page_layout(page: &Node) -> Option<PageLayout> {
    page.metadata
        .get(PAGE_LAYOUT_METADATA_KEY)
        .and_then(|v| serde_json::from_value::<PageLayout>(v.clone()).ok())
}

fn page_dimensions(page: &Node, layout: Option<&PageLayout>) -> PageDimensions {
    let (width_px, height_px) = (page.bounds.width, page.bounds.height);
    let px_per_mm = layout.map_or(FALLBACK_PRINT_DPI / MM_PER_INCH, |l| {
        let (w_mm, _) = l.dimensions_mm();
        if w_mm > 0.0 {
            width_px / w_mm
        } else {
            FALLBACK_PRINT_DPI / MM_PER_INCH
        }
    });
    PageDimensions {
        width_px,
        height_px,
        px_per_mm,
    }
}

fn collect_descendants(document: &DocumentGraph, root: Uuid) -> Vec<Uuid> {
    let mut out: Vec<Uuid> = Vec::new();
    let mut stack: Vec<Uuid> = Vec::new();
    if let Some(node) = document.get_node(root) {
        stack.extend(node.children.iter().copied());
    }
    while let Some(id) = stack.pop() {
        let Some(node) = document.get_node(id) else {
            continue;
        };
        out.push(id);
        stack.extend(node.children.iter().copied());
    }
    out.sort_by_key(|id| *id.as_bytes());
    out
}

/// Bleed check.
///
/// A layer must either stay outside the bleed band (its inner edge is
/// more than `require_bleed_mm` from the page edge) or extend past the
/// page edge to cover the bleed. Layers that *just* touch the page
/// edge — extending into the bleed zone but not past it — are flagged.
fn check_node_for_bleed(
    node: &Node,
    page_id: Uuid,
    dims: &PageDimensions,
    options: &PreflightOptions,
    issues: &mut Vec<PreflightIssue>,
) {
    if !is_content_layer(node.node_type) {
        return;
    }
    if dims.px_per_mm <= 0.0 || options.require_bleed_mm <= 0.0 {
        return;
    }
    let bleed_px = options.require_bleed_mm * dims.px_per_mm;
    let b = &node.bounds;
    let left = b.x;
    let right = b.x + b.width;
    let top = b.y;
    let bottom = b.y + b.height;

    let touches_left = left < bleed_px && left >= 0.0;
    let touches_right = right > dims.width_px - bleed_px && right <= dims.width_px;
    let touches_top = top < bleed_px && top >= 0.0;
    let touches_bottom = bottom > dims.height_px - bleed_px && bottom <= dims.height_px;

    // Enumerate *all* sides the layer touches, not just the first
    // match. A corner element can enter the bleed zone on two sides
    // simultaneously and the user needs to know about both to extend
    // it correctly. Per Devin Review
    // ANALYSIS_pr-review-job-790e7860e5c745e0bee13295709290f4_0005.
    let mut sides: Vec<&'static str> = Vec::with_capacity(4);
    if touches_left {
        sides.push("left");
    }
    if touches_right {
        sides.push("right");
    }
    if touches_top {
        sides.push("top");
    }
    if touches_bottom {
        sides.push("bottom");
    }
    if !sides.is_empty() {
        let sides_label = match sides.as_slice() {
            [a] => (*a).to_string(),
            [a, b] => format!("{a} and {b}"),
            many => {
                let (last, rest) = many.split_last().expect("non-empty");
                format!("{}, and {last}", rest.join(", "))
            }
        };
        issues.push(PreflightIssue {
            check: PreflightCheck::BleedMargin,
            severity: PreflightSeverity::Warning,
            message: format!(
                "Layer '{name}' enters the {sides_label} bleed zone ({bleed_mm:.1} mm) without extending past the page edge.",
                name = node.name,
                bleed_mm = options.require_bleed_mm,
            ),
            affected_node_id: Some(node.id),
            page_id: Some(page_id),
        });
    }
}

fn is_content_layer(node_type: NodeType) -> bool {
    matches!(
        node_type,
        NodeType::VectorLayer
            | NodeType::RasterLayer
            | NodeType::TextLayer
            | NodeType::ComponentLayer
    )
}

/// Font-embed + glyph-coverage check.
///
/// Two-tier check:
///   1. **`FontEmbed` (Error)** — the `font_family` must resolve in
///      the local fontdb. A missing family means the print pipeline
///      will silently substitute a fallback face and produce wrong
///      letterforms on the proof.
///   2. **`FontGlyphCoverage` (Warning)** — when the family *does*
///      resolve, probe the resolved face for every codepoint in the
///      rendered text. Any codepoint without a glyph (`None` or
///      `.notdef`) surfaces as a warning so the user knows the
///      character will print as a hollow rectangle.
///
/// The two checks are stacked rather than mutually exclusive: a
/// resolved-but-incomplete font still produces an `Error` for the
/// missing family (it doesn't), only a `Warning` per missing glyph.
/// This matches the severity ladder used elsewhere — errors block
/// export, warnings inform.
fn check_node_font_embed(
    node: &Node,
    page_id: Uuid,
    fonts: &FontManager,
    issues: &mut Vec<PreflightIssue>,
) {
    let Some(meta) = text_layer_meta(node) else {
        return;
    };
    if fonts.find_family(&meta.font_family).is_empty() {
        issues.push(PreflightIssue {
            check: PreflightCheck::FontEmbed,
            severity: PreflightSeverity::Error,
            message: format!(
                "Text layer '{name}' references missing font family '{family}'.",
                name = node.name,
                family = meta.font_family,
            ),
            affected_node_id: Some(node.id),
            page_id: Some(page_id),
        });
        // Coverage probing would just return another NotFound, so
        // skip — the export-blocking `FontEmbed` error is the
        // actionable signal for the user.
        return;
    }
    // The family resolves; check glyph coverage. We deliberately
    // ignore the `Err(NotFound)` branch here — `find_family` already
    // returned a non-empty list, so the only way `missing_glyphs`
    // can fail is `FaceData` (unreadable face bytes), which is
    // not a preflight-actionable error.
    let Ok(missing) = fonts.missing_glyphs(&meta.font_family, &meta.text) else {
        return;
    };
    if missing.is_empty() {
        return;
    }
    // Emit ONE issue per text layer rather than one per missing
    // codepoint. A document that uses an em-dash, a curly apostrophe,
    // and a degree sign in a font that has none of them would
    // otherwise generate 3 issues that all say "fix this font",
    // burying every other issue in the panel. The message lists up
    // to 6 representative codepoints so the user can copy-paste
    // them into a font lookup; the full count goes in the prefix.
    let preview_cap = 6;
    let preview: String = missing
        .iter()
        .take(preview_cap)
        .map(|ch| format!("U+{:04X} '{ch}'", u32::from(*ch)))
        .collect::<Vec<_>>()
        .join(", ");
    let suffix = if missing.len() > preview_cap {
        format!(", … (+{} more)", missing.len() - preview_cap)
    } else {
        String::new()
    };
    issues.push(PreflightIssue {
        check: PreflightCheck::FontGlyphCoverage,
        severity: PreflightSeverity::Warning,
        message: format!(
            "Text layer '{name}' uses {n} codepoint(s) the font '{family}' has no glyph for ({preview}{suffix}). These will print as `.notdef` (hollow rectangle).",
            name = node.name,
            family = meta.font_family,
            n = missing.len(),
        ),
        affected_node_id: Some(node.id),
        page_id: Some(page_id),
    });
}

/// Image resolution check.
///
/// Computes the effective DPI of each raster layer from its pixel
/// dimensions and rendered size on the page, flagging anything below
/// `target_dpi`.
fn check_node_image_resolution(
    node: &Node,
    page_id: Uuid,
    dims: &PageDimensions,
    options: &PreflightOptions,
    issues: &mut Vec<PreflightIssue>,
) {
    let Some(meta) = raster_image_meta(node) else {
        return;
    };
    if dims.px_per_mm <= 0.0 || node.bounds.width <= 0.0 || node.bounds.height <= 0.0 {
        return;
    }
    let display_w_mm = dims.px_to_mm(node.bounds.width);
    let display_h_mm = dims.px_to_mm(node.bounds.height);
    if display_w_mm <= 0.0 || display_h_mm <= 0.0 {
        return;
    }
    let dpi_x = f64::from(meta.width) / (display_w_mm / MM_PER_INCH);
    let dpi_y = f64::from(meta.height) / (display_h_mm / MM_PER_INCH);
    let effective_dpi = dpi_x.min(dpi_y);
    if effective_dpi + 0.5 < options.target_dpi {
        issues.push(PreflightIssue {
            check: PreflightCheck::ImageResolution,
            severity: PreflightSeverity::Error,
            message: format!(
                "Raster layer '{name}' is {effective_dpi:.0} DPI (target {target:.0}). Use a larger source image or shrink the layer.",
                name = node.name,
                target = options.target_dpi,
            ),
            affected_node_id: Some(node.id),
            page_id: Some(page_id),
        });
    }
}

/// Color space check.
///
/// When the target is CMYK, any chromatic *RGB* fill is flagged as a
/// warning so the user knows the export pipeline will run an
/// approximate sRGB → CMYK conversion (full ICC transforms are Phase
/// 3). A `Color::Cmyk` override or grayscale RGB fill is accepted
/// without comment; a `Color::Lab` / `Color::Hsl` override is also
/// flagged because emission goes through the same sRGB→CMYK path.
fn check_node_color_space(
    node: &Node,
    page_id: Uuid,
    options: &PreflightOptions,
    issues: &mut Vec<PreflightIssue>,
) {
    if options.target_color_space != ColorSpaceTarget::Cmyk {
        return;
    }
    if !node_needs_cmyk_conversion(node) {
        return;
    }
    issues.push(PreflightIssue {
        check: PreflightCheck::ColorSpace,
        severity: PreflightSeverity::Warning,
        message: format!(
            "Layer '{name}' uses an RGB fill but the target is CMYK; conversion is approximate until Phase 3 ICC support lands.",
            name = node.name,
        ),
        affected_node_id: Some(node.id),
        page_id: Some(page_id),
    });
}

/// Does this node carry a chromatic non-CMYK color that will go
/// through the sRGB→CMYK approximation when exported to a CMYK target?
fn node_needs_cmyk_conversion(node: &Node) -> bool {
    // A CMYK override means the user already authored in the target
    // space; nothing to convert. Any other override variant (sRGB,
    // Lab, HSL) ends up going through srgb_to_cmyk and so qualifies
    // as a chromatic RGB fill for the purposes of this check.
    if let Some(over) = &node.style.color_override {
        return match over {
            kcreate_core::color::Color::Cmyk { .. } => false,
            kcreate_core::color::Color::Srgb { r, g, b, .. } => !is_grayscale(*r, *g, *b),
            kcreate_core::color::Color::Hsl { s, .. } => *s > 0.01,
            kcreate_core::color::Color::Lab { a_star, b_star, .. } => {
                a_star.abs() > 1.0 || b_star.abs() > 1.0
            }
        };
    }
    fill_has_chromatic_rgb(&node.style.fill)
}

fn fill_has_chromatic_rgb(fill: &FillStyle) -> bool {
    match fill {
        FillStyle::None => false,
        FillStyle::Solid(c) => !is_grayscale(c.r, c.g, c.b),
        FillStyle::Gradient(g) => {
            let stops = match g {
                kcreate_core::node::GradientKind::Linear { stops, .. }
                | kcreate_core::node::GradientKind::Radial { stops, .. } => stops,
            };
            stops
                .iter()
                .any(|s| !is_grayscale(s.color.r, s.color.g, s.color.b))
        }
    }
}

fn is_grayscale(r: f32, g: f32, b: f32) -> bool {
    let tolerance = 0.01_f32;
    (r - g).abs() < tolerance && (g - b).abs() < tolerance
}

/// Shading-pattern check.
///
/// PR #9 introduced real PDF Type 2/3 shading patterns for gradient
/// fills (see `kcreate_export::pdf_shading`). The injector enforces
/// hard invariants at write time: `TooFewStops` (< 2) and
/// `PageOutOfRange` are both surfaced as `PdfShadingError` and abort
/// the export. This check catches the *fixable* subset
/// (insufficient stops, malformed offsets, degenerate identical
/// stops) at preflight time so the user sees the issue in the
/// PreflightPanel instead of as a cryptic export-time failure.
///
/// It also flags soft issues that the injector handles silently but
/// where the user's intent gets lost: a CMYK `color_override` on a
/// Gradient is flattened to DeviceRGB when the export target is
/// RGB-only, so we warn the user rather than letting them export an
/// RGB PDF and wonder why their print proof looks off.
fn check_node_shading(
    node: &Node,
    page_id: Uuid,
    options: &PreflightOptions,
    issues: &mut Vec<PreflightIssue>,
) {
    let FillStyle::Gradient(gradient) = &node.style.fill else {
        return;
    };
    let (stops, kind_label) = match gradient {
        kcreate_core::node::GradientKind::Linear { stops, .. } => (stops, "linear"),
        kcreate_core::node::GradientKind::Radial { stops, .. } => (stops, "radial"),
    };

    // Hard-stop: too few stops. PDF Type 2/3 functions require
    // ≥ 2 stops; the injector errors out otherwise so the export
    // wouldn't even produce a file. Severity is `Error` to match
    // the export-blocking severity used elsewhere.
    if stops.len() < 2 {
        issues.push(PreflightIssue {
            check: PreflightCheck::Shading,
            severity: PreflightSeverity::Error,
            message: format!(
                "Layer '{name}' has a {kind} gradient with only {n} stop(s); PDF shading patterns require at least 2 stops.",
                name = node.name,
                kind = kind_label,
                n = stops.len(),
            ),
            affected_node_id: Some(node.id),
            page_id: Some(page_id),
        });
        // No further per-stop checks are meaningful when the stop
        // count is below the minimum — bail to keep the issue
        // list focused.
        return;
    }

    // Offsets must be in `[0, 1]`. The injector's stitching
    // function builds piecewise sub-functions over `[t_i, t_{i+1}]`
    // and a stop with offset outside that range produces a
    // physically wrong domain when the dict is parsed by a PDF
    // viewer (Acrobat clamps but Preview / Skia do not).
    for stop in stops {
        if !(0.0..=1.0).contains(&stop.offset) {
            issues.push(PreflightIssue {
                check: PreflightCheck::Shading,
                severity: PreflightSeverity::Error,
                message: format!(
                    "Layer '{name}' has a {kind} gradient stop at offset {off:.3}; PDF shading offsets must be in [0, 1].",
                    name = node.name,
                    kind = kind_label,
                    off = stop.offset,
                ),
                affected_node_id: Some(node.id),
                page_id: Some(page_id),
            });
        }
    }

    // Strictly-ascending offsets. The stitching-function bounds
    // array (`Bounds`) requires the inner stop offsets to be
    // strictly ascending; the injector slices `&shading.stops[1..n-1]`
    // and writes those offsets verbatim. Out-of-order or equal
    // offsets break the function lookup in real viewers.
    let mut prev = stops[0].offset;
    for stop in stops.iter().skip(1) {
        if stop.offset <= prev {
            issues.push(PreflightIssue {
                check: PreflightCheck::Shading,
                severity: PreflightSeverity::Error,
                message: format!(
                    "Layer '{name}' has out-of-order gradient stops ({prev:.3} ≥ {next:.3}); PDF shading requires strictly-ascending offsets.",
                    name = node.name,
                    prev = prev,
                    next = stop.offset,
                ),
                affected_node_id: Some(node.id),
                page_id: Some(page_id),
            });
            // Don't update `prev` past a violating stop so we keep
            // the violation framed against the last-valid offset.
            continue;
        }
        prev = stop.offset;
    }

    // Endpoints. A well-formed gradient places stops at `0.0` and
    // `1.0` so the shading covers the full domain. Stops that
    // start at e.g. `0.2` produce undefined behaviour in the
    // injector's Domain[0 1] when the function is evaluated below
    // the first stop. Soft-warn so the user can choose to extend
    // the stops or accept the clamping behaviour.
    let endpoint_tolerance = 1e-6_f64;
    let first = stops[0].offset;
    let last = stops[stops.len() - 1].offset;
    if (first - 0.0).abs() > endpoint_tolerance {
        issues.push(PreflightIssue {
            check: PreflightCheck::Shading,
            severity: PreflightSeverity::Warning,
            message: format!(
                "Layer '{name}' gradient starts at offset {first:.3}; consider adding a stop at 0.0 — PDF viewers clamp the function below the first stop, which can produce a visible band.",
                name = node.name,
            ),
            affected_node_id: Some(node.id),
            page_id: Some(page_id),
        });
    }
    if (last - 1.0).abs() > endpoint_tolerance {
        issues.push(PreflightIssue {
            check: PreflightCheck::Shading,
            severity: PreflightSeverity::Warning,
            message: format!(
                "Layer '{name}' gradient ends at offset {last:.3}; consider adding a stop at 1.0 — PDF viewers clamp the function above the last stop, which can produce a visible band.",
                name = node.name,
            ),
            affected_node_id: Some(node.id),
            page_id: Some(page_id),
        });
    }

    // Degenerate: every stop has the same colour. The injector
    // emits a wrapper shading + N-1 sub-functions for what could
    // be a single Solid fill. Not wrong, but a Warning so the
    // user can simplify.
    let first_color = stops[0].color;
    let all_same = stops.iter().all(|s| {
        (s.color.r - first_color.r).abs() < 0.005
            && (s.color.g - first_color.g).abs() < 0.005
            && (s.color.b - first_color.b).abs() < 0.005
            && (s.color.a - first_color.a).abs() < 0.005
    });
    if all_same {
        issues.push(PreflightIssue {
            check: PreflightCheck::Shading,
            severity: PreflightSeverity::Warning,
            message: format!(
                "Layer '{name}' has a {kind} gradient where every stop is the same colour; consider a solid fill instead — the export will still emit a PDF shading dict, which bloats the file with no visible difference.",
                name = node.name,
                kind = kind_label,
            ),
            affected_node_id: Some(node.id),
            page_id: Some(page_id),
        });
    }

    // CMYK override on RGB-only export. `pdf_shading::color_space_for_mode`
    // collapses Cmyk colour overrides on Gradient fills to DeviceRGB
    // whenever the target colour space is RGB. The injector does this
    // silently; the user only sees the issue at print time. Surface it
    // here as an Info note (RGB-only is a deliberate choice for screen
    // export, so it's not a Warning — but the user should know the
    // CMYK is being thrown away).
    if matches!(options.target_color_space, ColorSpaceTarget::Rgb) {
        if let Some(kcreate_core::color::Color::Cmyk { .. }) = &node.style.color_override {
            issues.push(PreflightIssue {
                check: PreflightCheck::Shading,
                severity: PreflightSeverity::Info,
                message: format!(
                    "Layer '{name}' has a CMYK colour override on a gradient fill but the export target is RGB; the override will be flattened to DeviceRGB at export time.",
                    name = node.name,
                ),
                affected_node_id: Some(node.id),
                page_id: Some(page_id),
            });
        }
    }
}

/// Total ink coverage (TIC) check.
///
/// Sums the CMYK components for the *paint that will actually be laid
/// down on press* and warns when the sum exceeds the per-options cap.
///
/// The override-vs-fill precedence here mirrors
/// `pdf::resolve_fill_paint` exactly:
///
/// - If `style.color_override` is `Some(_)`, that override replaces the
///   fill at export time. We measure the override: `Color::Cmyk`
///   components are summed directly (authored as CMYK, no conversion
///   error); `Color::Srgb` / `Color::Hsl` / `Color::Lab` are routed
///   through `to_srgb()` → `srgb_to_cmyk` (the same path the PDF
///   exporter takes for non-CMYK overrides). The underlying
///   `style.fill` is **not** inspected in this branch because the
///   export pipeline never emits it — flagging it would surface a
///   false positive for ink that physically isn't on the page.
/// - If `style.color_override` is `None`, we walk `style.fill`:
///   `Solid` → `srgb_to_cmyk` once; `Gradient` → per stop with
///   worst-offender reporting (a gradient with N over-cap stops is
///   one design issue, not N).
///
/// The check is gated on `target_color_space == Cmyk`. On an RGB-only
/// export target there is no press to dry, so TIC is moot.
///
/// Range notes worth surfacing for future maintainers: the naive
/// `srgb_to_cmyk` in `kcreate_core::color` produces components whose
/// sum is bounded above by 3.0 (300%). At the default
/// `target_total_ink_coverage` of 3.0 with strict `>` comparison,
/// sRGB-sourced fills can never trip the check — by construction.
/// That's correct: with the naive conversion no sRGB color can
/// produce more than 300% on the press, so the warning would be
/// vacuous. The check is still meaningful for (a) explicit CMYK
/// overrides authored above the cap, and (b) users targeting tighter
/// caps (e.g. 240% for newsprint, 280% for web offset). When Phase 3
/// ICC profile chains land, sRGB sources will be able to legitimately
/// exceed 300%, and this check will activate without further work.
fn check_node_total_ink_coverage(
    node: &Node,
    page_id: Uuid,
    options: &PreflightOptions,
    issues: &mut Vec<PreflightIssue>,
) {
    if !matches!(options.target_color_space, ColorSpaceTarget::Cmyk) {
        return;
    }
    if !is_content_layer(node.node_type) {
        return;
    }
    let cap = options.target_total_ink_coverage;
    if !cap.is_finite() || cap <= 0.0 {
        return;
    }

    // 1. Override path. Any override variant replaces the fill at
    // export time; inspect ONLY the override here, never fall through
    // to the fill (see resolve_fill_paint in pdf.rs).
    if let Some(over) = &node.style.color_override {
        let sum = override_ink_sum(over);
        if sum > cap {
            let source = match over {
                kcreate_core::color::Color::Cmyk { .. } => "CMYK color override",
                kcreate_core::color::Color::Srgb { .. } => {
                    "sRGB color override (converted to CMYK)"
                }
                kcreate_core::color::Color::Hsl { .. } => "HSL color override (converted to CMYK)",
                kcreate_core::color::Color::Lab { .. } => "Lab color override (converted to CMYK)",
            };
            push_tic_issue(node, page_id, sum, cap, source, None, issues);
        }
        return;
    }

    // 2. Underlying fill. Only reachable when no override exists — i.e.
    // the fill is what the export pipeline will actually emit.
    match &node.style.fill {
        FillStyle::None => {}
        FillStyle::Solid(c) => {
            let sum = solid_color_ink_sum(c);
            if sum > cap {
                push_tic_issue(node, page_id, sum, cap, "solid fill", None, issues);
            }
        }
        FillStyle::Gradient(gradient) => {
            let stops = match gradient {
                kcreate_core::node::GradientKind::Linear { stops, .. }
                | kcreate_core::node::GradientKind::Radial { stops, .. } => stops,
            };
            // Walk every stop; report the highest offender to keep
            // the panel readable. A gradient with 10 over-cap stops
            // is a single design issue, not 10 separate ones.
            let mut worst: Option<(usize, f64)> = None;
            for (idx, stop) in stops.iter().enumerate() {
                let sum = solid_color_ink_sum(&stop.color);
                if sum > cap && worst.is_none_or(|(_, best)| sum > best) {
                    worst = Some((idx, sum));
                }
            }
            if let Some((idx, sum)) = worst {
                let location = format!("gradient stop {idx}");
                push_tic_issue(
                    node,
                    page_id,
                    sum,
                    cap,
                    "gradient fill",
                    Some(location),
                    issues,
                );
            }
        }
    }
}

/// Sum of CMYK components for an override color, in `[0, 4]`.
///
/// `Color::Cmyk` is read out directly (authored in target space, no
/// conversion needed). Every other variant routes through
/// `to_srgb()` → `srgb_to_cmyk()`, which is precisely the path
/// `pdf::resolve_fill_paint` uses for non-CMYK overrides. Keeping the
/// two helpers in lockstep is the whole point of pulling the check
/// out — if the exporter ever switches to a different conversion for
/// non-CMYK overrides, this helper has to follow.
fn override_ink_sum(over: &kcreate_core::color::Color) -> f64 {
    match over {
        kcreate_core::color::Color::Cmyk { c, m, y, k, .. } => {
            f64::from(*c) + f64::from(*m) + f64::from(*y) + f64::from(*k)
        }
        _ => {
            let (r, g, b, _alpha) = over.to_srgb();
            let (c, m, y, k) = kcreate_core::color::srgb_to_cmyk(r, g, b);
            f64::from(c) + f64::from(m) + f64::from(y) + f64::from(k)
        }
    }
}

/// Sum of CMYK components for a single solid RGBA fill, in `[0, 4]`.
/// Always goes through `srgb_to_cmyk` for the same reason the
/// `ColorSpace` check does — that's what the export pipeline applies,
/// so it's the ink the press will actually see.
///
/// Note: the naive `srgb_to_cmyk` produces components bounded by
/// `4 - (r+g+b)/max(r,g,b) - max(r,g,b)`, which approaches but never
/// reaches 3.0 (300%). With the default 300% cap and strict `>`,
/// this function can never trip the TIC check on its own — see the
/// extended note on `check_node_total_ink_coverage`.
fn solid_color_ink_sum(color: &kcreate_core::node::RgbaColor) -> f64 {
    let (c, m, y, k) = kcreate_core::color::srgb_to_cmyk(color.r, color.g, color.b);
    f64::from(c) + f64::from(m) + f64::from(y) + f64::from(k)
}

fn push_tic_issue(
    node: &Node,
    page_id: Uuid,
    sum: f64,
    cap: f64,
    source_label: &str,
    location: Option<String>,
    issues: &mut Vec<PreflightIssue>,
) {
    let sum_pct = sum * 100.0;
    let cap_pct = cap * 100.0;
    let location_suffix = location.map(|l| format!(" ({l})")).unwrap_or_default();
    issues.push(PreflightIssue {
        check: PreflightCheck::TotalInkCoverage,
        severity: PreflightSeverity::Warning,
        message: format!(
            "Layer '{name}' {source_label}{location_suffix} totals {sum_pct:.0}% ink, exceeding the {cap_pct:.0}% cap. Press operators reject jobs over this limit due to drying and blocking.",
            name = node.name,
        ),
        affected_node_id: Some(node.id),
        page_id: Some(page_id),
    });
}

/// Transparency check.
///
/// Print runs default to opaque; any layer with `opacity < 1.0` or a
/// blend mode other than Normal is flagged.
fn check_node_transparency(
    node: &Node,
    page_id: Uuid,
    options: &PreflightOptions,
    issues: &mut Vec<PreflightIssue>,
) {
    if options.allow_transparency {
        return;
    }
    let opacity_below_one = node.opacity < 0.999;
    let blend_not_normal = !matches!(node.blend_mode, BlendMode::Normal);
    if opacity_below_one || blend_not_normal {
        issues.push(PreflightIssue {
            check: PreflightCheck::Transparency,
            severity: PreflightSeverity::Warning,
            message: format!(
                "Layer '{name}' has opacity {opacity:.2} / blend {blend:?}; print runs default to opaque-Normal.",
                name = node.name,
                opacity = node.opacity,
                blend = node.blend_mode,
            ),
            affected_node_id: Some(node.id),
            page_id: Some(page_id),
        });
    }
}

/// Page-size check.
///
/// Standard ISO and US sizes are accepted. Custom and presentation
/// sizes produce info-level notes so the user can confirm they were
/// intentional.
fn check_page_size(layout: Option<&PageLayout>, page_id: Uuid, issues: &mut Vec<PreflightIssue>) {
    let Some(layout) = layout else {
        issues.push(PreflightIssue {
            check: PreflightCheck::PageSize,
            severity: PreflightSeverity::Info,
            message: "Page has no PageLayout metadata; preflight assumed 300 DPI for measurement."
                .to_string(),
            affected_node_id: None,
            page_id: Some(page_id),
        });
        return;
    };
    match layout.page_size {
        PageSize::A3
        | PageSize::A4
        | PageSize::A5
        | PageSize::Letter
        | PageSize::Legal
        | PageSize::Tabloid => {}
        PageSize::Presentation16x9 | PageSize::Presentation4x3 => {
            issues.push(PreflightIssue {
                check: PreflightCheck::PageSize,
                severity: PreflightSeverity::Info,
                message: "Page uses a presentation slide size; standard print sizes are A3/A4/A5/Letter/Legal/Tabloid.".to_string(),
                affected_node_id: None,
                page_id: Some(page_id),
            });
        }
        PageSize::Custom {
            width_mm,
            height_mm,
        } => {
            issues.push(PreflightIssue {
                check: PreflightCheck::PageSize,
                severity: PreflightSeverity::Warning,
                message: format!(
                    "Page uses a custom size ({width_mm:.1}×{height_mm:.1} mm) — print shops typically charge a setup fee for non-standard sizes."
                ),
                affected_node_id: None,
                page_id: Some(page_id),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene_metadata::TextLayerMeta;
    use kcreate_core::node::{
        Bounds, GradientKind, GradientStop, Node, NodeStyle, Point2D, RgbaColor,
    };
    use serde_json::json;

    fn page_with_layout(doc: &mut DocumentGraph, layout: PageLayout, bounds: Bounds) -> Uuid {
        let mut p = Node::new(NodeType::Page, "Page 1");
        p.bounds = bounds;
        p.metadata.insert(
            PAGE_LAYOUT_METADATA_KEY.to_string(),
            serde_json::to_value(&layout).unwrap(),
        );
        doc.insert_node(p).unwrap()
    }

    fn child(
        doc: &mut DocumentGraph,
        parent: Uuid,
        node_type: NodeType,
        name: &str,
        bounds: Bounds,
    ) -> Uuid {
        let mut n = Node::new(node_type, name);
        n.parent_id = Some(parent);
        n.bounds = bounds;
        doc.insert_node(n).unwrap()
    }

    fn a4_layout() -> PageLayout {
        PageLayout::new(PageSize::A4, kcreate_core::node::PageOrientation::Portrait)
    }

    fn a4_bounds() -> Bounds {
        // 210 × 297 mm at 300 DPI = 2480 × 3508 px
        Bounds::new(0.0, 0.0, 2480.0, 3508.0)
    }

    #[test]
    fn empty_document_emits_no_issues() {
        let doc = DocumentGraph::new();
        let issues = run_preflight(&doc, &[], &PreflightOptions::default());
        assert!(issues.is_empty());
    }

    #[test]
    fn bleed_zone_layer_emits_warning() {
        let mut doc = DocumentGraph::new();
        let page = page_with_layout(&mut doc, a4_layout(), a4_bounds());
        // bleed = 3 mm at 300dpi ≈ 35.43 px. Layer at x=10 enters the
        // left bleed zone without extending past the page edge.
        let _layer = child(
            &mut doc,
            page,
            NodeType::VectorLayer,
            "art",
            Bounds::new(10.0, 100.0, 200.0, 200.0),
        );
        let issues = run_preflight(&doc, &[], &PreflightOptions::default());
        assert!(issues
            .iter()
            .any(|i| i.check == PreflightCheck::BleedMargin));
    }

    #[test]
    fn bleed_zone_corner_layer_lists_all_sides() {
        // Layer touching both the left and top bleed zones must
        // mention "left and top", not just one side. Per Devin
        // Review ANALYSIS_pr-review-job-790e7860e5c745e0bee13295709290f4_0005.
        let mut doc = DocumentGraph::new();
        let page = page_with_layout(&mut doc, a4_layout(), a4_bounds());
        let _layer = child(
            &mut doc,
            page,
            NodeType::VectorLayer,
            "corner",
            // x=10, y=10 — both within the ≈35.43 px bleed band.
            // width/height keep the layer inside the page so it
            // doesn't extend past either edge.
            Bounds::new(10.0, 10.0, 200.0, 200.0),
        );
        let issues = run_preflight(&doc, &[], &PreflightOptions::default());
        let bleed: Vec<&PreflightIssue> = issues
            .iter()
            .filter(|i| i.check == PreflightCheck::BleedMargin)
            .collect();
        // Single issue per node, but its message names both sides.
        assert_eq!(
            bleed.len(),
            1,
            "expected exactly one bleed issue, got {bleed:?}",
        );
        let msg = &bleed[0].message;
        assert!(
            msg.contains("left") && msg.contains("top"),
            "expected both 'left' and 'top' sides in message, got: {msg}",
        );
        assert!(
            msg.contains("left and top"),
            "expected 'left and top' joining phrase, got: {msg}",
        );
    }

    #[test]
    fn safe_layer_does_not_warn_for_bleed() {
        let mut doc = DocumentGraph::new();
        let page = page_with_layout(&mut doc, a4_layout(), a4_bounds());
        // 50 px in from every edge, well outside the bleed band.
        let _layer = child(
            &mut doc,
            page,
            NodeType::VectorLayer,
            "art",
            Bounds::new(200.0, 200.0, 800.0, 800.0),
        );
        let issues = run_preflight(&doc, &[], &PreflightOptions::default());
        assert!(!issues
            .iter()
            .any(|i| i.check == PreflightCheck::BleedMargin));
    }

    #[test]
    fn missing_font_emits_error() {
        let mut doc = DocumentGraph::new();
        let page = page_with_layout(&mut doc, a4_layout(), a4_bounds());
        let mut text = Node::new(NodeType::TextLayer, "title");
        text.parent_id = Some(page);
        text.bounds = Bounds::new(300.0, 300.0, 800.0, 100.0);
        let meta = TextLayerMeta {
            text: "Hello".to_string(),
            font_family: "___definitely_not_a_real_font_family___".to_string(),
            font_size: 24.0,
        };
        text.metadata.insert(
            TEXT_LAYER_METADATA_KEY.to_string(),
            serde_json::to_value(meta).unwrap(),
        );
        doc.insert_node(text).unwrap();
        let issues = run_preflight(&doc, &[], &PreflightOptions::default());
        let fe = issues
            .iter()
            .find(|i| i.check == PreflightCheck::FontEmbed)
            .expect("font embed issue");
        assert_eq!(fe.severity, PreflightSeverity::Error);
    }

    #[test]
    fn low_dpi_raster_emits_error() {
        let mut doc = DocumentGraph::new();
        let page = page_with_layout(&mut doc, a4_layout(), a4_bounds());
        let mut raster = Node::new(NodeType::RasterLayer, "photo");
        raster.parent_id = Some(page);
        // Display the image full-page; provide only 100×100 px of
        // source. Effective DPI ≪ 300.
        raster.bounds = Bounds::new(0.0, 0.0, 2480.0, 3508.0);
        raster.metadata.insert(
            RASTER_IMAGE_METADATA_KEY.to_string(),
            json!({"blob_hash": "deadbeef", "width": 100, "height": 100}),
        );
        doc.insert_node(raster).unwrap();
        let issues = run_preflight(&doc, &[], &PreflightOptions::default());
        let r = issues
            .iter()
            .find(|i| i.check == PreflightCheck::ImageResolution)
            .expect("image resolution issue");
        assert_eq!(r.severity, PreflightSeverity::Error);
    }

    #[test]
    fn rgb_fill_warns_against_cmyk_target() {
        let mut doc = DocumentGraph::new();
        let page = page_with_layout(&mut doc, a4_layout(), a4_bounds());
        let mut node = Node::new(NodeType::VectorLayer, "red");
        node.parent_id = Some(page);
        node.bounds = Bounds::new(400.0, 400.0, 100.0, 100.0);
        node.style = NodeStyle {
            fill: FillStyle::Solid(RgbaColor::new(1.0, 0.2, 0.2, 1.0)),
            ..NodeStyle::default()
        };
        doc.insert_node(node).unwrap();
        let issues = run_preflight(&doc, &[], &PreflightOptions::default());
        assert!(issues.iter().any(|i| i.check == PreflightCheck::ColorSpace));
    }

    #[test]
    fn rgb_fill_is_silent_when_target_is_rgb() {
        let mut doc = DocumentGraph::new();
        let page = page_with_layout(&mut doc, a4_layout(), a4_bounds());
        let mut node = Node::new(NodeType::VectorLayer, "red");
        node.parent_id = Some(page);
        node.bounds = Bounds::new(400.0, 400.0, 100.0, 100.0);
        node.style = NodeStyle {
            fill: FillStyle::Solid(RgbaColor::new(1.0, 0.2, 0.2, 1.0)),
            ..NodeStyle::default()
        };
        doc.insert_node(node).unwrap();
        let opts = PreflightOptions {
            target_color_space: ColorSpaceTarget::Rgb,
            ..PreflightOptions::default()
        };
        let issues = run_preflight(&doc, &[], &opts);
        assert!(!issues.iter().any(|i| i.check == PreflightCheck::ColorSpace));
    }

    #[test]
    fn gradient_fill_picks_up_chromatic_stops() {
        let mut doc = DocumentGraph::new();
        let page = page_with_layout(&mut doc, a4_layout(), a4_bounds());
        let mut node = Node::new(NodeType::VectorLayer, "gradient");
        node.parent_id = Some(page);
        node.bounds = Bounds::new(400.0, 400.0, 100.0, 100.0);
        node.style = NodeStyle {
            fill: FillStyle::Gradient(GradientKind::Linear {
                from: Point2D::new(0.0, 0.0),
                to: Point2D::new(100.0, 0.0),
                stops: vec![
                    GradientStop {
                        offset: 0.0,
                        color: RgbaColor::new(0.5, 0.5, 0.5, 1.0),
                    },
                    GradientStop {
                        offset: 1.0,
                        color: RgbaColor::new(0.9, 0.1, 0.1, 1.0),
                    },
                ],
            }),
            ..NodeStyle::default()
        };
        doc.insert_node(node).unwrap();
        let issues = run_preflight(&doc, &[], &PreflightOptions::default());
        assert!(issues.iter().any(|i| i.check == PreflightCheck::ColorSpace));
    }

    #[test]
    fn opacity_under_one_warns_about_transparency() {
        let mut doc = DocumentGraph::new();
        let page = page_with_layout(&mut doc, a4_layout(), a4_bounds());
        let mut node = Node::new(NodeType::VectorLayer, "tinted");
        node.parent_id = Some(page);
        node.bounds = Bounds::new(400.0, 400.0, 100.0, 100.0);
        node.opacity = 0.5;
        doc.insert_node(node).unwrap();
        let issues = run_preflight(&doc, &[], &PreflightOptions::default());
        assert!(issues
            .iter()
            .any(|i| i.check == PreflightCheck::Transparency));
    }

    #[test]
    fn transparency_allowed_silences_warning() {
        let mut doc = DocumentGraph::new();
        let page = page_with_layout(&mut doc, a4_layout(), a4_bounds());
        let mut node = Node::new(NodeType::VectorLayer, "tinted");
        node.parent_id = Some(page);
        node.bounds = Bounds::new(400.0, 400.0, 100.0, 100.0);
        node.opacity = 0.5;
        doc.insert_node(node).unwrap();
        let opts = PreflightOptions {
            allow_transparency: true,
            ..PreflightOptions::default()
        };
        let issues = run_preflight(&doc, &[], &opts);
        assert!(!issues
            .iter()
            .any(|i| i.check == PreflightCheck::Transparency));
    }

    #[test]
    fn custom_page_size_warns() {
        let mut doc = DocumentGraph::new();
        let layout = PageLayout::new(
            PageSize::Custom {
                width_mm: 100.0,
                height_mm: 250.0,
            },
            kcreate_core::node::PageOrientation::Portrait,
        );
        let _page = page_with_layout(&mut doc, layout, Bounds::new(0.0, 0.0, 1181.0, 2953.0));
        let issues = run_preflight(&doc, &[], &PreflightOptions::default());
        assert!(issues.iter().any(
            |i| i.check == PreflightCheck::PageSize && i.severity == PreflightSeverity::Warning
        ));
    }

    #[test]
    fn presentation_page_size_emits_info() {
        let mut doc = DocumentGraph::new();
        let layout = PageLayout::new(
            PageSize::Presentation16x9,
            kcreate_core::node::PageOrientation::Landscape,
        );
        let _page = page_with_layout(&mut doc, layout, Bounds::new(0.0, 0.0, 3000.0, 1687.0));
        let issues = run_preflight(&doc, &[], &PreflightOptions::default());
        let info = issues
            .iter()
            .find(|i| i.check == PreflightCheck::PageSize)
            .expect("page size info");
        assert_eq!(info.severity, PreflightSeverity::Info);
    }

    #[test]
    fn empty_pages_slice_means_all_pages() {
        let mut doc = DocumentGraph::new();
        let _p1 = page_with_layout(&mut doc, a4_layout(), a4_bounds());
        let _p2 = page_with_layout(&mut doc, a4_layout(), a4_bounds());
        // Both pages run, both emit no errors.
        let issues = run_preflight(&doc, &[], &PreflightOptions::default());
        assert!(issues
            .iter()
            .all(|i| i.severity != PreflightSeverity::Error));
    }

    #[test]
    fn unknown_page_id_is_silently_skipped() {
        let doc = DocumentGraph::new();
        let bogus = Uuid::new_v4();
        let issues = run_preflight(&doc, &[bogus], &PreflightOptions::default());
        assert!(issues.is_empty());
    }

    /// Helper: insert a VectorLayer with a gradient fill and return
    /// the layer id. The page is A4 portrait sized exactly to a4_bounds.
    /// `stops` is taken verbatim — pass in whatever the test needs to
    /// exercise (too-few, out-of-order, out-of-range, etc.).
    fn gradient_node(
        doc: &mut DocumentGraph,
        page: Uuid,
        name: &str,
        stops: Vec<GradientStop>,
    ) -> Uuid {
        let mut n = Node::new(NodeType::VectorLayer, name);
        n.parent_id = Some(page);
        // Place well inside the page so other checks (bleed,
        // transparency) don't add issues that swamp our assertions.
        n.bounds = Bounds::new(500.0, 500.0, 400.0, 400.0);
        n.style = NodeStyle {
            fill: FillStyle::Gradient(GradientKind::Linear {
                from: Point2D::new(0.0, 0.0),
                to: Point2D::new(100.0, 0.0),
                stops,
            }),
            ..NodeStyle::default()
        };
        doc.insert_node(n).unwrap()
    }

    #[test]
    fn shading_check_rejects_single_stop_gradient() {
        let mut doc = DocumentGraph::new();
        let page = page_with_layout(&mut doc, a4_layout(), a4_bounds());
        let id = gradient_node(
            &mut doc,
            page,
            "lonely",
            vec![GradientStop {
                offset: 0.5,
                color: RgbaColor::new(0.5, 0.5, 0.5, 1.0),
            }],
        );
        let issues = run_preflight(&doc, &[], &PreflightOptions::default());
        let shading: Vec<_> = issues
            .iter()
            .filter(|i| i.check == PreflightCheck::Shading)
            .collect();
        assert_eq!(shading.len(), 1, "single-stop gradient must emit 1 issue");
        assert_eq!(shading[0].severity, PreflightSeverity::Error);
        assert_eq!(shading[0].affected_node_id, Some(id));
        // The injector's `TooFewStops` error fires below 2; the
        // preflight message must clearly attribute it to the layer.
        assert!(
            shading[0].message.contains("lonely"),
            "message must name the layer: {}",
            shading[0].message
        );
    }

    #[test]
    fn shading_check_rejects_stops_out_of_range() {
        let mut doc = DocumentGraph::new();
        let page = page_with_layout(&mut doc, a4_layout(), a4_bounds());
        let _id = gradient_node(
            &mut doc,
            page,
            "range",
            vec![
                GradientStop {
                    offset: -0.1,
                    color: RgbaColor::new(0.0, 0.0, 0.0, 1.0),
                },
                GradientStop {
                    offset: 1.3,
                    color: RgbaColor::new(1.0, 1.0, 1.0, 1.0),
                },
            ],
        );
        let issues = run_preflight(&doc, &[], &PreflightOptions::default());
        let range_errors: Vec<_> = issues
            .iter()
            .filter(|i| {
                i.check == PreflightCheck::Shading
                    && i.severity == PreflightSeverity::Error
                    && i.message.contains("must be in [0, 1]")
            })
            .collect();
        assert_eq!(
            range_errors.len(),
            2,
            "expected one offset-out-of-range issue per bad stop, got {}",
            range_errors.len()
        );
    }

    #[test]
    fn shading_check_rejects_out_of_order_offsets() {
        let mut doc = DocumentGraph::new();
        let page = page_with_layout(&mut doc, a4_layout(), a4_bounds());
        let _id = gradient_node(
            &mut doc,
            page,
            "unsorted",
            vec![
                GradientStop {
                    offset: 0.0,
                    color: RgbaColor::new(0.0, 0.0, 0.0, 1.0),
                },
                GradientStop {
                    offset: 0.7,
                    color: RgbaColor::new(0.3, 0.3, 0.3, 1.0),
                },
                // Goes backwards — would corrupt the stitching
                // function's `Bounds` array if it reached the
                // injector.
                GradientStop {
                    offset: 0.4,
                    color: RgbaColor::new(0.6, 0.6, 0.6, 1.0),
                },
                GradientStop {
                    offset: 1.0,
                    color: RgbaColor::new(1.0, 1.0, 1.0, 1.0),
                },
            ],
        );
        let issues = run_preflight(&doc, &[], &PreflightOptions::default());
        let order_errors: Vec<_> = issues
            .iter()
            .filter(|i| {
                i.check == PreflightCheck::Shading
                    && i.severity == PreflightSeverity::Error
                    && i.message.contains("out-of-order")
            })
            .collect();
        assert_eq!(
            order_errors.len(),
            1,
            "expected exactly one out-of-order issue, got {}",
            order_errors.len()
        );
    }

    #[test]
    fn shading_check_warns_on_missing_endpoint_stops() {
        let mut doc = DocumentGraph::new();
        let page = page_with_layout(&mut doc, a4_layout(), a4_bounds());
        let _id = gradient_node(
            &mut doc,
            page,
            "no-endpoints",
            vec![
                GradientStop {
                    offset: 0.2,
                    color: RgbaColor::new(0.1, 0.2, 0.3, 1.0),
                },
                GradientStop {
                    offset: 0.8,
                    color: RgbaColor::new(0.9, 0.8, 0.7, 1.0),
                },
            ],
        );
        let issues = run_preflight(&doc, &[], &PreflightOptions::default());
        let endpoint_warnings: Vec<_> = issues
            .iter()
            .filter(|i| {
                i.check == PreflightCheck::Shading && i.severity == PreflightSeverity::Warning
            })
            .collect();
        // 1 warning for missing 0.0 endpoint + 1 for missing 1.0 endpoint.
        assert_eq!(
            endpoint_warnings.len(),
            2,
            "expected 2 endpoint warnings, got {}",
            endpoint_warnings.len()
        );
    }

    #[test]
    fn shading_check_warns_on_all_identical_stops() {
        let mut doc = DocumentGraph::new();
        let page = page_with_layout(&mut doc, a4_layout(), a4_bounds());
        let _id = gradient_node(
            &mut doc,
            page,
            "degenerate",
            vec![
                GradientStop {
                    offset: 0.0,
                    color: RgbaColor::new(0.5, 0.5, 0.5, 1.0),
                },
                GradientStop {
                    offset: 0.5,
                    color: RgbaColor::new(0.5, 0.5, 0.5, 1.0),
                },
                GradientStop {
                    offset: 1.0,
                    color: RgbaColor::new(0.5, 0.5, 0.5, 1.0),
                },
            ],
        );
        let issues = run_preflight(&doc, &[], &PreflightOptions::default());
        let degenerate_warnings: Vec<_> = issues
            .iter()
            .filter(|i| {
                i.check == PreflightCheck::Shading
                    && i.severity == PreflightSeverity::Warning
                    && i.message.contains("same colour")
            })
            .collect();
        assert_eq!(
            degenerate_warnings.len(),
            1,
            "expected exactly 1 'same colour' warning, got {}",
            degenerate_warnings.len()
        );
    }

    #[test]
    fn shading_check_info_on_cmyk_override_with_rgb_target() {
        let mut doc = DocumentGraph::new();
        let page = page_with_layout(&mut doc, a4_layout(), a4_bounds());
        let mut n = Node::new(NodeType::VectorLayer, "cmyk-on-rgb");
        n.parent_id = Some(page);
        n.bounds = Bounds::new(500.0, 500.0, 400.0, 400.0);
        n.style = NodeStyle {
            fill: FillStyle::Gradient(GradientKind::Linear {
                from: Point2D::new(0.0, 0.0),
                to: Point2D::new(100.0, 0.0),
                stops: vec![
                    GradientStop {
                        offset: 0.0,
                        color: RgbaColor::new(0.0, 0.0, 0.0, 1.0),
                    },
                    GradientStop {
                        offset: 1.0,
                        color: RgbaColor::new(1.0, 1.0, 1.0, 1.0),
                    },
                ],
            }),
            color_override: Some(kcreate_core::color::Color::Cmyk {
                c: 0.5,
                m: 0.2,
                y: 0.0,
                k: 0.1,
                a: 1.0,
            }),
            ..NodeStyle::default()
        };
        doc.insert_node(n).unwrap();
        let opts = PreflightOptions {
            target_color_space: ColorSpaceTarget::Rgb,
            ..PreflightOptions::default()
        };
        let issues = run_preflight(&doc, &[], &opts);
        let info: Vec<_> = issues
            .iter()
            .filter(|i| {
                i.check == PreflightCheck::Shading
                    && i.severity == PreflightSeverity::Info
                    && i.message.contains("flattened to DeviceRGB")
            })
            .collect();
        assert_eq!(
            info.len(),
            1,
            "expected exactly 1 cmyk-on-rgb info note, got {}",
            info.len()
        );
    }

    #[test]
    fn shading_check_well_formed_gradient_emits_no_issues() {
        let mut doc = DocumentGraph::new();
        let page = page_with_layout(&mut doc, a4_layout(), a4_bounds());
        let _id = gradient_node(
            &mut doc,
            page,
            "ok",
            vec![
                GradientStop {
                    offset: 0.0,
                    color: RgbaColor::new(0.1, 0.1, 0.1, 1.0),
                },
                GradientStop {
                    offset: 0.5,
                    color: RgbaColor::new(0.5, 0.5, 0.5, 1.0),
                },
                GradientStop {
                    offset: 1.0,
                    color: RgbaColor::new(0.9, 0.9, 0.9, 1.0),
                },
            ],
        );
        let issues = run_preflight(&doc, &[], &PreflightOptions::default());
        let shading_issues: Vec<_> = issues
            .iter()
            .filter(|i| i.check == PreflightCheck::Shading)
            .collect();
        assert!(
            shading_issues.is_empty(),
            "well-formed gradient should emit no shading issues, got: {shading_issues:?}"
        );
    }

    #[test]
    fn cached_font_manager_returns_same_instance_across_calls() {
        // First call populates the cell, second call must hand back
        // the same Arc — that's the whole point of the cache (avoid
        // re-scanning system fonts on every preflight run).
        let a = cached_font_manager();
        let b = cached_font_manager();
        assert!(Arc::ptr_eq(&a, &b));
        // Invalidate and verify the next call rebuilds (different Arc).
        clear_cached_font_manager();
        let c = cached_font_manager();
        assert!(!Arc::ptr_eq(&a, &c));
    }

    #[test]
    fn tic_check_flags_cmyk_override_over_cap() {
        // CMYK override summing to 320% (= 3.2) is over the 300%
        // default cap → must produce a TotalInkCoverage warning.
        let mut doc = DocumentGraph::new();
        let page = page_with_layout(&mut doc, a4_layout(), a4_bounds());
        let mut n = Node::new(NodeType::VectorLayer, "heavy ink");
        n.parent_id = Some(page);
        n.bounds = Bounds::new(0.0, 0.0, 200.0, 200.0);
        n.style.color_override = Some(kcreate_core::color::Color::Cmyk {
            c: 0.9,
            m: 0.9,
            y: 0.9,
            k: 0.5,
            a: 1.0,
        });
        doc.insert_node(n).unwrap();
        let issues = run_preflight(&doc, &[], &PreflightOptions::default());
        let tic: Vec<&PreflightIssue> = issues
            .iter()
            .filter(|i| i.check == PreflightCheck::TotalInkCoverage)
            .collect();
        assert_eq!(tic.len(), 1, "expected one TIC warning, got {tic:?}");
        assert!(tic[0].message.contains("320%"), "msg: {}", tic[0].message);
    }

    #[test]
    fn tic_check_ignores_under_cap_cmyk() {
        // 250% ink — under the 300% default cap.
        let mut doc = DocumentGraph::new();
        let page = page_with_layout(&mut doc, a4_layout(), a4_bounds());
        let mut n = Node::new(NodeType::VectorLayer, "fine ink");
        n.parent_id = Some(page);
        n.bounds = Bounds::new(0.0, 0.0, 200.0, 200.0);
        n.style.color_override = Some(kcreate_core::color::Color::Cmyk {
            c: 0.8,
            m: 0.8,
            y: 0.5,
            k: 0.4,
            a: 1.0,
        });
        doc.insert_node(n).unwrap();
        let issues = run_preflight(&doc, &[], &PreflightOptions::default());
        assert!(
            !issues
                .iter()
                .any(|i| i.check == PreflightCheck::TotalInkCoverage),
            "under-cap fill must not trip TIC, got: {issues:?}"
        );
    }

    #[test]
    fn tic_check_skipped_for_rgb_target() {
        // Same over-cap CMYK as the positive test, but the export
        // target is RGB — no press, no TIC concern.
        let mut doc = DocumentGraph::new();
        let page = page_with_layout(&mut doc, a4_layout(), a4_bounds());
        let mut n = Node::new(NodeType::VectorLayer, "heavy ink");
        n.parent_id = Some(page);
        n.bounds = Bounds::new(0.0, 0.0, 200.0, 200.0);
        n.style.color_override = Some(kcreate_core::color::Color::Cmyk {
            c: 0.9,
            m: 0.9,
            y: 0.9,
            k: 0.5,
            a: 1.0,
        });
        doc.insert_node(n).unwrap();
        let opts = PreflightOptions {
            target_color_space: ColorSpaceTarget::Rgb,
            ..PreflightOptions::default()
        };
        let issues = run_preflight(&doc, &[], &opts);
        assert!(
            !issues
                .iter()
                .any(|i| i.check == PreflightCheck::TotalInkCoverage),
            "RGB target must skip TIC check entirely"
        );
    }

    #[test]
    fn tic_check_reports_worst_gradient_stop() {
        // Three stops; the middle one is over-cap. Test that exactly
        // one issue surfaces and that the message points to the
        // worst-offending stop index.
        let mut doc = DocumentGraph::new();
        let page = page_with_layout(&mut doc, a4_layout(), a4_bounds());
        let mut n = Node::new(NodeType::VectorLayer, "gradient bg");
        n.parent_id = Some(page);
        n.bounds = Bounds::new(0.0, 0.0, 200.0, 200.0);
        // Pure black sRGB → (0, 0, 0, 1) CMYK = 100% ink (fine).
        // Pure cyan-magenta-yellow mix (deep brown) → very high
        // total ink. Easiest way to engineer an over-cap stop in
        // sRGB space: (0.05, 0.05, 0.05) → produces (~0, ~0, ~0,
        // ~0.95) CMYK ≈ 95% ink (under). Need a more saturated
        // dark colour. Use (0.02, 0.05, 0.10) → ((0.10-0.02)/0.10
        // = 0.8 c, (0.10-0.05)/0.10 = 0.5 m, 0 y, 0.90 k) = 220%
        // (still under). To engineer over-cap reliably, use a
        // small RGB triple where one channel is very dark: (0.0,
        // 0.0, 0.10) → c=1.0, m=1.0, y=0.0, k=0.90 = 290%. We
        // want over-cap — push k by using (0.0, 0.0, 0.05):
        //   k = 1 - 0.05 = 0.95
        //   c = (1 - 0 - 0.95) / 0.05 = 1.0
        //   m = (1 - 0 - 0.95) / 0.05 = 1.0
        //   y = (1 - 0.05 - 0.95) / 0.05 = 0.0
        // = 1.0 + 1.0 + 0 + 0.95 = 295% — still under. Use
        // (0.005, 0.005, 0.02): k=0.98, c≈0.77, m≈0.77, y≈0 ≈
        // 252% — under. The clean way is just to express the
        // over-cap stop directly in CMYK via `color_override`,
        // but `RgbaColor` is sRGB-only. Instead, we use a custom
        // cap of 100% so any non-trivial chromatic fill trips.
        let red = RgbaColor::new(1.0, 0.0, 0.0, 1.0);
        let dark = RgbaColor::new(0.0, 0.05, 0.05, 1.0);
        let blue = RgbaColor::new(0.0, 0.0, 1.0, 1.0);
        n.style.fill = FillStyle::Gradient(GradientKind::Linear {
            from: Point2D::new(0.0, 0.0),
            to: Point2D::new(1.0, 0.0),
            stops: vec![
                GradientStop {
                    offset: 0.0,
                    color: red,
                },
                GradientStop {
                    offset: 0.5,
                    color: dark,
                },
                GradientStop {
                    offset: 1.0,
                    color: blue,
                },
            ],
        });
        doc.insert_node(n).unwrap();
        // 100% cap: every chromatic stop is over.
        let opts = PreflightOptions {
            target_total_ink_coverage: 1.0,
            ..PreflightOptions::default()
        };
        let issues = run_preflight(&doc, &[], &opts);
        let tic: Vec<&PreflightIssue> = issues
            .iter()
            .filter(|i| i.check == PreflightCheck::TotalInkCoverage)
            .collect();
        assert_eq!(
            tic.len(),
            1,
            "exactly one TIC warning per gradient (worst-offender semantics), got {tic:?}"
        );
        assert!(
            tic[0].message.contains("gradient stop "),
            "expected stop-index in message, got: {}",
            tic[0].message,
        );
    }

    #[test]
    fn tic_check_ignores_fill_when_under_cap_cmyk_override_replaces_it() {
        // Regression: previously the fall-through path checked
        // style.fill even when an under-cap CMYK override was present,
        // producing a false positive for ink the press will never see
        // (the override replaces the fill at export time per
        // pdf::resolve_fill_paint).
        //
        // We engineer this exactly: under-cap CMYK override + a fill
        // that WOULD trip the TIC check on its own at a tight cap.
        let mut doc = DocumentGraph::new();
        let page = page_with_layout(&mut doc, a4_layout(), a4_bounds());
        let mut n = Node::new(NodeType::VectorLayer, "override + heavy fill");
        n.parent_id = Some(page);
        n.bounds = Bounds::new(0.0, 0.0, 200.0, 200.0);
        // Override: 100% ink (well under the 150% cap below). This
        // is what the press will actually see.
        n.style.color_override = Some(kcreate_core::color::Color::Cmyk {
            c: 0.25,
            m: 0.25,
            y: 0.25,
            k: 0.25,
            a: 1.0,
        });
        // Fill: would convert to ~290% CMYK via srgb_to_cmyk — would
        // trip TIC at a tight cap if anyone checked it. We make that
        // assumption explicit by using a tight cap.
        n.style.fill = FillStyle::Solid(RgbaColor::new(1.0, 0.0, 0.0, 1.0));
        doc.insert_node(n).unwrap();
        let opts = PreflightOptions {
            target_total_ink_coverage: 1.5, // 150% — under override, would trip the fill
            ..PreflightOptions::default()
        };
        let issues = run_preflight(&doc, &[], &opts);
        let tic: Vec<&PreflightIssue> = issues
            .iter()
            .filter(|i| i.check == PreflightCheck::TotalInkCoverage)
            .collect();
        assert!(
            tic.is_empty(),
            "under-cap override hides the fill from the press; TIC must not fire on the fill, got: {tic:?}"
        );
    }

    #[test]
    fn tic_check_uses_srgb_override_after_cmyk_conversion() {
        // Regression: previously the override path matched only
        // Color::Cmyk; sRGB / Hsl / Lab overrides fell through and
        // the check inspected the underlying fill instead of the
        // override. The exporter converts non-CMYK overrides via
        // srgb_to_cmyk (see resolve_fill_paint), so the override is
        // what hits the press — we must measure it.
        let mut doc = DocumentGraph::new();
        let page = page_with_layout(&mut doc, a4_layout(), a4_bounds());
        let mut n = Node::new(NodeType::VectorLayer, "srgb override");
        n.parent_id = Some(page);
        n.bounds = Bounds::new(0.0, 0.0, 200.0, 200.0);
        // Pure red sRGB → srgb_to_cmyk: c=0, m=1, y=1, k=0 → 200%.
        n.style.color_override = Some(kcreate_core::color::Color::Srgb {
            r: 1.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        });
        // No fill — proves the new code reads the override directly.
        n.style.fill = FillStyle::None;
        doc.insert_node(n).unwrap();
        // Tight cap so the 200% override is over-cap.
        let opts = PreflightOptions {
            target_total_ink_coverage: 1.5,
            ..PreflightOptions::default()
        };
        let issues = run_preflight(&doc, &[], &opts);
        let tic: Vec<&PreflightIssue> = issues
            .iter()
            .filter(|i| i.check == PreflightCheck::TotalInkCoverage)
            .collect();
        assert_eq!(
            tic.len(),
            1,
            "sRGB override (converted) over-cap must produce one TIC warning, got: {tic:?}"
        );
        assert!(
            tic[0].message.contains("sRGB color override"),
            "expected sRGB-source label, got: {}",
            tic[0].message,
        );
    }

    #[test]
    fn tic_check_skips_fill_when_under_cap_srgb_override_replaces_it() {
        // Mirror of the CMYK case but for sRGB overrides — under-cap
        // sRGB override must hide the fill from TIC.
        let mut doc = DocumentGraph::new();
        let page = page_with_layout(&mut doc, a4_layout(), a4_bounds());
        let mut n = Node::new(NodeType::VectorLayer, "light srgb override");
        n.parent_id = Some(page);
        n.bounds = Bounds::new(0.0, 0.0, 200.0, 200.0);
        // Light gray → srgb_to_cmyk yields tiny components: max≈0.9,
        // k≈0.1, c=m=y=0. Total ~10% — far under any cap.
        n.style.color_override = Some(kcreate_core::color::Color::Srgb {
            r: 0.9,
            g: 0.9,
            b: 0.9,
            a: 1.0,
        });
        // Fill: red would be ~200% via srgb_to_cmyk. Would trip a
        // 150% cap if the fall-through bug were present.
        n.style.fill = FillStyle::Solid(RgbaColor::new(1.0, 0.0, 0.0, 1.0));
        doc.insert_node(n).unwrap();
        let opts = PreflightOptions {
            target_total_ink_coverage: 1.5,
            ..PreflightOptions::default()
        };
        let issues = run_preflight(&doc, &[], &opts);
        let tic: Vec<&PreflightIssue> = issues
            .iter()
            .filter(|i| i.check == PreflightCheck::TotalInkCoverage)
            .collect();
        assert!(
            tic.is_empty(),
            "under-cap sRGB override hides the fill; TIC must not fire on the fill, got: {tic:?}"
        );
    }

    #[test]
    fn font_glyph_coverage_emits_no_issue_for_unresolvable_family() {
        // When the family doesn't resolve at all, the existing
        // FontEmbed check fires; the glyph-coverage probe must NOT
        // additionally surface a phantom warning.
        let mut doc = DocumentGraph::new();
        let page = page_with_layout(&mut doc, a4_layout(), a4_bounds());
        let mut n = Node::new(NodeType::TextLayer, "headline");
        n.parent_id = Some(page);
        n.bounds = Bounds::new(0.0, 0.0, 200.0, 200.0);
        n.metadata.insert(
            TEXT_LAYER_METADATA_KEY.to_string(),
            serde_json::to_value(TextLayerMeta {
                text: "Hello".to_string(),
                font_family: "___definitely_not_a_real_font_family___".to_string(),
                font_size: 24.0,
            })
            .unwrap(),
        );
        doc.insert_node(n).unwrap();
        let issues = run_preflight(&doc, &[], &PreflightOptions::default());
        assert!(
            issues.iter().any(|i| i.check == PreflightCheck::FontEmbed),
            "missing family must trip FontEmbed, got: {issues:?}",
        );
        assert!(
            !issues
                .iter()
                .any(|i| i.check == PreflightCheck::FontGlyphCoverage),
            "missing family must NOT trip a phantom glyph-coverage warning, got: {issues:?}",
        );
    }

    #[test]
    fn preflight_options_serde_round_trip_includes_new_tic_field() {
        // Wire-format pin: the new target_total_ink_coverage field
        // must serialise to camelCase and round-trip cleanly so the
        // bridge → renderer IPC stays in lockstep with TypeScript.
        let opts = PreflightOptions::default();
        let json = serde_json::to_string(&opts).expect("serialise default");
        assert!(
            json.contains("\"targetTotalInkCoverage\":3.0"),
            "expected camelCase TIC field, got: {json}",
        );
        let round: PreflightOptions = serde_json::from_str(&json).expect("round-trip");
        assert_eq!(round.target_total_ink_coverage, 3.0);

        // Unknown / missing field must fall back to the default
        // (Serde's `#[serde(default)]` on the struct).
        let legacy = r#"{
            "targetDpi": 300.0,
            "requireBleedMm": 3.0,
            "allowTransparency": false,
            "targetColorSpace": "cmyk"
        }"#;
        let parsed: PreflightOptions = serde_json::from_str(legacy).expect("legacy parse");
        assert_eq!(parsed.target_total_ink_coverage, 3.0);
    }
}
