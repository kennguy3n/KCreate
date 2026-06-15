//! Bundled, ready-made template library — the curated set of
//! professionally-designed starter designs that ship with KCreate so
//! the marketplace is *populated on a fresh install* instead of empty.
//!
//! ## Why this lives in `kcreate_core`
//!
//! `kcreate_core` is the renderer-independent foundation, so the
//! catalog (pure data) and the on-disk *seeding* logic can run in any
//! headless context (tests, exporters, the bridge's first-run hook)
//! without pulling in wgpu or napi. The catalog is authored in Rust
//! with small ergonomic builders rather than committed as hand-written
//! JSON / binary blobs: it is type-checked, DRY, diff-friendly, and
//! avoids an `include_dir`/`include_bytes!` dependency.
//!
//! ## Single source of truth
//!
//! Each template is described by a [`TemplateContent`] — a flat list of
//! absolute-positioned [`TemplateItem`]s (rect / ellipse / line / text)
//! plus the design's `width` × `height`. The wire shape of each item is
//! a structural mirror of `CanvasBatchItem` in
//! `crates/kcreate_bridge/src/document.rs` (and `apps/desktop/shared/
//! scene.ts`), so the **same** `content.json` drives *both*:
//!
//! 1. **Thumbnail rendering** — the bridge builds a standalone document
//!    from the items and rasterises it through the `kcreate_export` PNG
//!    pipeline (`template_thumbnail`).
//! 2. **Apply / "Start from template"** — the bridge re-parents the
//!    same items under a fresh artboard in the open workspace
//!    (`template_instantiate`).
//!
//! Because both paths consume one description, the gallery preview and
//! the populated canvas are pixel-identical. A cross-crate test
//! (`crates/kcreate_tests/tests/template_library.rs`) deserialises every
//! bundled item into the bridge's `CanvasBatchItem` to lock the two
//! definitions in step (AGENTS.md rule 4).

use std::path::Path;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::marketplace::{MarketplaceError, TemplateManifest};
use crate::node::{FillStyle, RgbaColor};
use crate::project::TemplateCategory;

/// Font family every bundled template authors against. `"sans-serif"`
/// is a generic family that the renderer's font DB
/// (`kcreate_text::font_db::resolve_face`) always resolves to a real
/// installed face (DejaVu / Liberation / Free Sans on Linux, system
/// sans elsewhere), so text never falls back to blank glyphs.
const FONT: &str = "sans-serif";

/// Fraction of the font size used to convert an author-friendly
/// *visual top* Y into the renderer's text origin.
///
/// The text rasteriser places a layer's first-glyph **baseline** at the
/// node's top-left Y (`kcreate_bridge::scene_sync::node_text` sets the
/// origin to the node bounds' `(x, y)`, and
/// `kcreate_renderer::text::shape_to_path_commands` returns glyph
/// outlines whose origin is that baseline). Authoring in baseline space
/// is unintuitive and error-prone, so the [`Sheet`] text helpers accept
/// the glyph's visual top and shift the stored origin down by the
/// ascent (`size * TEXT_ASCENT_RATIO`). Calibrated against the bundled
/// fonts so cap-height text sits inside its intended band.
const TEXT_ASCENT_RATIO: f64 = 0.8;

/// Average glyph advance as a fraction of the font size, used only to
/// *center* a text run horizontally (button labels, hero titles). Exact
/// shaped metrics live behind the bridge in `kcreate_text`; this
/// proportional-sans approximation is intentionally conservative so a
/// centered label stays visually balanced without measuring glyphs.
const TEXT_ADVANCE_RATIO: f64 = 0.52;

/// One canvas primitive in a bundled template's `content.json`.
///
/// Structurally mirrors `CanvasBatchItem` in the bridge
/// (`crates/kcreate_bridge/src/document.rs`) and `CanvasBatchItem` in
/// `apps/desktop/shared/scene.ts`. `parent` is always serialised (the
/// bridge's deserializer requires the field to be present; bundled
/// items are always root-level, so it is `null`), while `fill` and
/// `name` are omitted when absent to match the bridge's
/// `#[serde(default)]` fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TemplateItem {
    /// Axis-aligned rectangle. Closed, filled path.
    Rect {
        parent: Option<Uuid>,
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fill: Option<FillStyle>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    /// Ellipse centred at `(cx, cy)` with radii `(rx, ry)`.
    Ellipse {
        parent: Option<Uuid>,
        cx: f64,
        cy: f64,
        rx: f64,
        ry: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fill: Option<FillStyle>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    /// Straight segment from `(x1, y1)` to `(x2, y2)`.
    Line {
        parent: Option<Uuid>,
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fill: Option<FillStyle>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    /// Single text run. `x`/`y` is the glyph baseline origin (the
    /// [`Sheet`] helpers convert from a visual top — see
    /// [`TEXT_ASCENT_RATIO`]).
    Text {
        parent: Option<Uuid>,
        x: f64,
        y: f64,
        body: String,
        family: String,
        size: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fill: Option<FillStyle>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
}

/// A complete template design: a canvas size plus the ordered list of
/// primitives that compose it. Item order is paint order (first item is
/// the back-most layer — by convention a full-bleed background rect so
/// the thumbnail and the applied canvas share an identical backdrop).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TemplateContent {
    pub width: f64,
    pub height: f64,
    pub items: Vec<TemplateItem>,
}

/// A bundled template ready to be written to disk: the destination
/// folder name (`*.ktemplate`), its [`TemplateManifest`], and its
/// [`TemplateContent`].
#[derive(Debug, Clone)]
pub struct BundledTemplate {
    pub dir_name: &'static str,
    pub manifest: TemplateManifest,
    pub content: TemplateContent,
}

/// Build the stable, deterministic UUID for bundled template `n`.
///
/// Hardcoding stable ids keeps seeding idempotent (the marketplace
/// keys templates by id) and lets `template_instantiate` / external
/// references address a template reliably across installs. The bytes
/// form a valid v4-shaped UUID (`feeded00-0000-4000-8000-0000000000NN`)
/// with the version (`0x40`) and variant (`0x80`) nibbles set.
const fn template_id(n: u8) -> Uuid {
    Uuid::from_bytes([
        0xFE, 0xED, 0xED, 0x00, 0x00, 0x00, 0x40, 0x00, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, n,
    ])
}

fn color(hex: &str) -> RgbaColor {
    RgbaColor::from_hex(hex).unwrap_or(RgbaColor::BLACK)
}

fn fill(hex: &str) -> FillStyle {
    FillStyle::Solid(color(hex))
}

fn fill_a(hex: &str, alpha: f32) -> FillStyle {
    let mut c = color(hex);
    c.a = alpha;
    FillStyle::Solid(c)
}

/// A mutable design surface that accumulates [`TemplateItem`]s. All
/// geometry is in absolute canvas coordinates; the builders keep
/// authoring terse and readable so each template reads as a flat list
/// of layout decisions.
struct Sheet {
    w: f64,
    h: f64,
    items: Vec<TemplateItem>,
}

impl Sheet {
    fn new(w: f64, h: f64) -> Self {
        Self {
            w,
            h,
            items: Vec::new(),
        }
    }

    /// Full-bleed background rect. Conventionally the first item so the
    /// thumbnail and applied canvas share an identical backdrop.
    fn bg(&mut self, hex: &str) {
        self.items.push(TemplateItem::Rect {
            parent: None,
            x: 0.0,
            y: 0.0,
            w: snap(self.w),
            h: snap(self.h),
            fill: Some(fill(hex)),
            name: Some("Background".to_string()),
        });
    }

    fn rect(&mut self, x: f64, y: f64, w: f64, h: f64, hex: &str, name: &str) {
        self.items.push(TemplateItem::Rect {
            parent: None,
            x: snap(x),
            y: snap(y),
            w: snap(w),
            h: snap(h),
            fill: Some(fill(hex)),
            name: Some(name.to_string()),
        });
    }

    #[allow(clippy::too_many_arguments)] // positional geometry + colour for a terse builder
    fn rect_a(&mut self, x: f64, y: f64, w: f64, h: f64, hex: &str, alpha: f32, name: &str) {
        self.items.push(TemplateItem::Rect {
            parent: None,
            x: snap(x),
            y: snap(y),
            w: snap(w),
            h: snap(h),
            fill: Some(fill_a(hex, alpha)),
            name: Some(name.to_string()),
        });
    }

    fn ellipse(&mut self, cx: f64, cy: f64, rx: f64, ry: f64, hex: &str, name: &str) {
        self.items.push(TemplateItem::Ellipse {
            parent: None,
            cx: snap(cx),
            cy: snap(cy),
            rx: snap(rx),
            ry: snap(ry),
            fill: Some(fill(hex)),
            name: Some(name.to_string()),
        });
    }

    #[allow(clippy::too_many_arguments)] // positional geometry + colour for a terse builder
    fn ellipse_a(&mut self, cx: f64, cy: f64, rx: f64, ry: f64, hex: &str, alpha: f32, name: &str) {
        self.items.push(TemplateItem::Ellipse {
            parent: None,
            cx: snap(cx),
            cy: snap(cy),
            rx: snap(rx),
            ry: snap(ry),
            fill: Some(fill_a(hex, alpha)),
            name: Some(name.to_string()),
        });
    }

    fn circle(&mut self, cx: f64, cy: f64, r: f64, hex: &str, name: &str) {
        self.ellipse(cx, cy, r, r, hex, name);
    }

    /// Left-aligned text. `top_y` is the glyph's visual top. The stored
    /// `y` is the text baseline (top plus an ascent offset) and is
    /// pixel-snapped so authored designs serialize to exact, stable
    /// integer coordinates.
    fn text(&mut self, x: f64, top_y: f64, size: f64, hex: &str, body: &str, name: &str) {
        self.items.push(TemplateItem::Text {
            parent: None,
            x: snap(x),
            y: snap(top_y + size * TEXT_ASCENT_RATIO),
            body: body.to_string(),
            family: FONT.to_string(),
            size,
            fill: Some(fill(hex)),
            name: Some(name.to_string()),
        });
    }

    /// Horizontally-centred text around `cx`. `top_y` is the visual top.
    fn text_center(&mut self, cx: f64, top_y: f64, size: f64, hex: &str, body: &str, name: &str) {
        let width = approx_text_width(body, size);
        self.text(cx - width / 2.0, top_y, size, hex, body, name);
    }

    fn finish(self) -> TemplateContent {
        TemplateContent {
            width: self.w,
            height: self.h,
            items: self.items,
        }
    }
}

fn approx_text_width(body: &str, size: f64) -> f64 {
    size * (body.chars().count().max(1) as f64) * TEXT_ADVANCE_RATIO
}

/// Pixel-snap a coordinate to the nearest integer. Template geometry is
/// authored in absolute device pixels; snapping keeps strokes/edges
/// crisp and guarantees the serialized `content.json` round-trips
/// through JSON exactly (integer-valued `f64` are exact under any
/// conforming parser), so the seeded file, the rendered thumbnail, and
/// the applied canvas stay byte-for-byte deterministic.
fn snap(v: f64) -> f64 {
    v.round()
}

fn manifest(
    n: u8,
    name: &str,
    description: &str,
    category: TemplateCategory,
    tags: &[&str],
) -> TemplateManifest {
    TemplateManifest {
        id: template_id(n),
        name: name.to_string(),
        description: description.to_string(),
        category,
        tags: tags.iter().map(|t| (*t).to_string()).collect(),
        // Rendered lazily by the bridge on first request and cached to
        // `thumbnail.png` inside the template folder.
        thumbnail: Some("thumbnail.png".to_string()),
        page_count: 1,
        author: Some("KCreate".to_string()),
        version: "1.0.0".to_string(),
        source: None,
    }
}

/// The full curated catalog of bundled templates.
///
/// 122 professional designs spanning all 11 [`TemplateCategory`]
/// families: mobile-app UI kits, presentation slides, investor pitch
/// decks, social posts for every major network, posters, flyers,
/// résumés, business reports, brochures, proposals, and a "custom"
/// shelf of print/marketing collateral (business cards, invoices,
/// certificates, menus, gift cards, tickets, web heroes, …).
///
/// Authored in Rust with the [`Sheet`] builders so the catalog stays
/// type-checked, DRY, and diff-friendly. Each entry carries a stable
/// [`template_id`] (the curated set occupies ordinals `1..=122`; new
/// additions must take the next free ordinal — see [`template_id`]).
#[must_use]
pub fn bundled_templates() -> Vec<BundledTemplate> {
    vec![
        // -- Mobile-app UI kits ---------------------------------------
        mobile_onboarding(),
        mobile_login(),
        mobile_feed(),
        mobile_profile(),
        mobile_settings(),
        mobile_music(),
        mobile_checkout(),
        mobile_signup(),
        mobile_search(),
        mobile_chat(),
        mobile_notifications(),
        mobile_dashboard(),
        mobile_wallet(),
        mobile_paywall(),
        mobile_product(),
        mobile_cart(),
        mobile_delivery(),
        mobile_fitness(),
        mobile_weather(),
        mobile_splash(),
        // -- Presentation slides --------------------------------------
        deck_title(),
        deck_agenda(),
        deck_metrics(),
        deck_team(),
        deck_section(),
        deck_quote(),
        deck_chart_bar(),
        deck_comparison(),
        deck_timeline(),
        deck_closing(),
        deck_bignum(),
        deck_process(),
        deck_pricing(),
        deck_testimonial(),
        deck_image_text(),
        deck_table(),
        // -- Investor pitch decks -------------------------------------
        pitch_onepager(),
        pitch_cover(),
        pitch_problem(),
        pitch_solution(),
        pitch_market(),
        pitch_business_model(),
        pitch_competition(),
        pitch_financials(),
        pitch_ask(),
        // -- Social media ---------------------------------------------
        social_ig_post(),
        social_ig_story(),
        social_linkedin(),
        social_quote(),
        social_ig_announcement(),
        social_ig_giveaway(),
        social_ig_tips(),
        social_ig_testimonial(),
        social_ig_product(),
        social_ig_carousel(),
        social_story_countdown(),
        social_story_poll(),
        social_reels_cover(),
        social_linkedin_post(),
        social_x_header(),
        social_x_post(),
        social_yt_thumbnail(),
        social_fb_cover(),
        social_pinterest_pin(),
        social_podcast_cover(),
        social_ig_event(),
        social_ig_motivation(),
        // -- Posters --------------------------------------------------
        poster_event(),
        poster_concert(),
        poster_movie(),
        poster_art_expo(),
        poster_gym(),
        poster_food(),
        poster_real_estate(),
        poster_workshop(),
        poster_travel(),
        poster_typographic(),
        // -- Flyers ---------------------------------------------------
        flyer_sale(),
        flyer_restaurant(),
        flyer_open_house(),
        flyer_fitness_class(),
        flyer_grand_opening(),
        flyer_club_night(),
        flyer_community(),
        flyer_product_launch(),
        // -- Résumés / CVs --------------------------------------------
        resume_cv(),
        resume_modern(),
        resume_minimal(),
        resume_creative(),
        resume_executive(),
        resume_developer(),
        // -- Business reports -----------------------------------------
        report_cover(),
        report_cover_minimal(),
        report_exec_summary(),
        report_data_page(),
        report_section(),
        report_financials(),
        // -- Brochures ------------------------------------------------
        brochure_cover(),
        brochure_trifold(),
        brochure_real_estate(),
        brochure_travel(),
        brochure_product(),
        // -- Proposals ------------------------------------------------
        proposal_cover(),
        proposal_cover_light(),
        proposal_scope(),
        proposal_pricing(),
        proposal_about(),
        // -- Custom print / marketing collateral ----------------------
        custom_business_card(),
        custom_business_card_back(),
        custom_letterhead(),
        custom_invoice(),
        custom_certificate(),
        custom_menu(),
        custom_gift_card(),
        custom_ticket(),
        custom_postcard(),
        custom_web_hero(),
        custom_infographic(),
        custom_email_header(),
        custom_coupon(),
        custom_name_badge(),
        custom_price_list(),
    ]
}

/// Write the bundled catalog into `root` *only if* no `.ktemplate`
/// folder already exists there (copy-if-empty). Returns the number of
/// templates written (`0` when the directory is already populated).
///
/// Idempotent: a populated directory — whether seeded previously or
/// hand-curated by the user — is left untouched, so user edits and
/// third-party installs are never clobbered. `root` is created if
/// missing. The caller chooses `root` (the bridge honours
/// `KCREATE_TEMPLATE_DIR`), so this also respects that override.
pub fn seed_bundled_templates(root: &Path) -> Result<usize, MarketplaceError> {
    std::fs::create_dir_all(root)?;
    let already_populated = std::fs::read_dir(root)?.flatten().any(|entry| {
        entry.path().is_dir()
            && entry
                .file_name()
                .to_str()
                .is_some_and(|n| n.ends_with(".ktemplate"))
    });
    if already_populated {
        return Ok(0);
    }

    let mut written = 0usize;
    for template in bundled_templates() {
        let dir = root.join(template.dir_name);
        std::fs::create_dir_all(&dir)?;
        let manifest_json = serde_json::to_string_pretty(&template.manifest)
            .map_err(|e| MarketplaceError::Serialize(e.to_string()))?;
        std::fs::write(dir.join("manifest.json"), manifest_json)?;
        let content_json = serde_json::to_string_pretty(&template.content)
            .map_err(|e| MarketplaceError::Serialize(e.to_string()))?;
        std::fs::write(dir.join("content.json"), content_json)?;
        written += 1;
    }
    Ok(written)
}

// ---------------------------------------------------------------------
// Catalog — mobile-app UI kits (1080 × 2160, portrait phone)
// ---------------------------------------------------------------------

const PHONE_W: f64 = 1080.0;
const PHONE_H: f64 = 2160.0;

/// Draw a faux status bar (time + indicator pills) at the top of a
/// phone screen using `ink` for the glyph colour.
fn phone_status_bar(s: &mut Sheet, ink: &str) {
    s.text(72.0, 40.0, 34.0, ink, "9:41", "Status / time");
    s.rect(PHONE_W - 230.0, 48.0, 44.0, 26.0, ink, "Status / signal");
    s.rect(PHONE_W - 172.0, 48.0, 44.0, 26.0, ink, "Status / wifi");
    s.rect(PHONE_W - 110.0, 46.0, 64.0, 30.0, ink, "Status / battery");
}

/// Draw a five-slot bottom tab bar with the first slot active.
fn phone_tab_bar(s: &mut Sheet, bar_hex: &str, active: &str, idle: &str) {
    let bar_y = PHONE_H - 170.0;
    s.rect(0.0, bar_y, PHONE_W, 170.0, bar_hex, "Tab bar");
    let slots = 5;
    let step = PHONE_W / f64::from(slots);
    for i in 0..slots {
        let cx = step * (f64::from(i) + 0.5);
        let hex = if i == 0 { active } else { idle };
        s.circle(cx, bar_y + 70.0, 26.0, hex, "Tab icon");
        s.rect(cx - 34.0, bar_y + 112.0, 68.0, 12.0, hex, "Tab label");
    }
}

fn mobile_onboarding() -> BundledTemplate {
    let mut s = Sheet::new(PHONE_W, PHONE_H);
    s.bg("#ECFDF5");
    // Hero panel with layered decorative circles.
    s.rect(0.0, 0.0, PHONE_W, 1160.0, "#10B981", "Hero panel");
    s.ellipse_a(880.0, 180.0, 360.0, 360.0, "#FFFFFF", 0.12, "Hero glow");
    s.ellipse_a(180.0, 980.0, 280.0, 280.0, "#064E3B", 0.18, "Hero shadow");
    phone_status_bar(&mut s, "#ECFDF5");
    // Illustration placeholder: stacked cards.
    s.rect_a(
        330.0,
        360.0,
        420.0,
        520.0,
        "#FFFFFF",
        0.18,
        "Illustration back",
    );
    s.rect(360.0, 410.0, 360.0, 440.0, "#FFFFFF", "Illustration card");
    s.rect(360.0, 410.0, 360.0, 150.0, "#34D399", "Illustration header");
    s.circle(540.0, 690.0, 70.0, "#10B981", "Illustration mark");
    s.rect(420.0, 800.0, 240.0, 18.0, "#D1FAE5", "Illustration line");
    // Copy block.
    s.text_center(540.0, 1290.0, 84.0, "#064E3B", "Plan your day", "Title 1");
    s.text_center(540.0, 1390.0, 84.0, "#064E3B", "your way", "Title 2");
    s.text_center(
        540.0,
        1540.0,
        38.0,
        "#475569",
        "Organize tasks, set reminders, and",
        "Body 1",
    );
    s.text_center(
        540.0,
        1600.0,
        38.0,
        "#475569",
        "stay focused with a calm workspace.",
        "Body 2",
    );
    // Pager dots.
    s.circle(470.0, 1740.0, 18.0, "#10B981", "Dot active");
    s.circle(540.0, 1740.0, 14.0, "#A7F3D0", "Dot 2");
    s.circle(602.0, 1740.0, 14.0, "#A7F3D0", "Dot 3");
    // Primary button.
    s.rect(120.0, 1850.0, 840.0, 150.0, "#059669", "Primary button");
    s.text_center(
        540.0,
        1895.0,
        46.0,
        "#FFFFFF",
        "Get Started",
        "Button label",
    );
    BundledTemplate {
        dir_name: "mobile-onboarding.ktemplate",
        manifest: manifest(
            1,
            "Onboarding — Calm Tasks",
            "Mobile onboarding screen with hero illustration, pager dots, and a primary call-to-action.",
            TemplateCategory::MobileApp,
            &["mobile", "onboarding", "app", "ui kit", "welcome"],
        ),
        content: s.finish(),
    }
}

fn mobile_login() -> BundledTemplate {
    let mut s = Sheet::new(PHONE_W, PHONE_H);
    s.bg("#0F172A");
    s.ellipse_a(900.0, 120.0, 420.0, 420.0, "#6366F1", 0.35, "Glow A");
    s.ellipse_a(120.0, 760.0, 360.0, 360.0, "#22D3EE", 0.18, "Glow B");
    phone_status_bar(&mut s, "#E2E8F0");
    // Brand mark.
    s.circle(540.0, 470.0, 96.0, "#6366F1", "Logo");
    s.rect(496.0, 426.0, 88.0, 88.0, "#0F172A", "Logo cutout");
    s.circle(540.0, 470.0, 34.0, "#A5B4FC", "Logo dot");
    s.text_center(540.0, 640.0, 66.0, "#F8FAFC", "Welcome back", "Title");
    s.text_center(
        540.0,
        730.0,
        36.0,
        "#94A3B8",
        "Sign in to continue",
        "Subtitle",
    );
    // Email field.
    s.rect(120.0, 900.0, 840.0, 130.0, "#1E293B", "Email field");
    s.text(
        170.0,
        945.0,
        38.0,
        "#64748B",
        "Email address",
        "Email placeholder",
    );
    // Password field.
    s.rect(120.0, 1070.0, 840.0, 130.0, "#1E293B", "Password field");
    s.text(
        170.0,
        1115.0,
        38.0,
        "#64748B",
        "Password",
        "Password placeholder",
    );
    s.circle(900.0, 1135.0, 22.0, "#475569", "Eye toggle");
    s.text(
        120.0,
        1250.0,
        32.0,
        "#818CF8",
        "Forgot password?",
        "Forgot link",
    );
    // Sign-in button.
    s.rect(120.0, 1360.0, 840.0, 150.0, "#6366F1", "Sign-in button");
    s.text_center(540.0, 1405.0, 46.0, "#FFFFFF", "Sign In", "Button label");
    // Divider + social.
    s.rect(120.0, 1600.0, 360.0, 4.0, "#334155", "Divider left");
    s.rect(600.0, 1600.0, 360.0, 4.0, "#334155", "Divider right");
    s.text_center(540.0, 1582.0, 30.0, "#64748B", "or", "Divider label");
    s.rect(120.0, 1690.0, 400.0, 130.0, "#1E293B", "Google button");
    s.rect(560.0, 1690.0, 400.0, 130.0, "#1E293B", "Apple button");
    s.circle(250.0, 1755.0, 30.0, "#EF4444", "Google mark");
    s.circle(690.0, 1755.0, 30.0, "#F8FAFC", "Apple mark");
    s.text_center(
        540.0,
        1930.0,
        32.0,
        "#94A3B8",
        "New here?  Create an account",
        "Footer",
    );
    BundledTemplate {
        dir_name: "mobile-login.ktemplate",
        manifest: manifest(
            2,
            "Login — Welcome Back",
            "Dark mobile sign-in screen with email/password fields, social login, and a gradient-style glow.",
            TemplateCategory::MobileApp,
            &["mobile", "login", "auth", "sign in", "app"],
        ),
        content: s.finish(),
    }
}

fn mobile_feed() -> BundledTemplate {
    let mut s = Sheet::new(PHONE_W, PHONE_H);
    s.bg("#F8FAFC");
    phone_status_bar(&mut s, "#0F172A");
    // Header.
    s.text(72.0, 110.0, 64.0, "#0F172A", "Discover", "Header title");
    s.circle(980.0, 150.0, 52.0, "#E2E8F0", "Avatar");
    s.circle(980.0, 150.0, 24.0, "#6366F1", "Avatar dot");
    // Search bar.
    s.rect(72.0, 250.0, 936.0, 110.0, "#FFFFFF", "Search bar");
    s.circle(140.0, 305.0, 24.0, "#94A3B8", "Search icon");
    s.text(
        190.0,
        285.0,
        36.0,
        "#94A3B8",
        "Search ideas, people, tags",
        "Search placeholder",
    );
    // Chips.
    let chips = ["For you", "Design", "Travel", "Food"];
    let mut cx = 72.0;
    for (i, label) in chips.iter().enumerate() {
        let wdt = approx_text_width(label, 32.0) + 70.0;
        let active = i == 0;
        let bg = if active { "#6366F1" } else { "#FFFFFF" };
        let fg = if active { "#FFFFFF" } else { "#475569" };
        s.rect(cx, 420.0, wdt, 80.0, bg, "Chip");
        s.text(cx + 35.0, 444.0, 32.0, fg, label, "Chip label");
        cx += wdt + 24.0;
    }
    // Feed cards.
    let cards = [
        ("#FDE68A", "#92400E", "Sunlit studio", "Interior · 12 min"),
        ("#BFDBFE", "#1E3A8A", "Coastline trip", "Travel · 8 min"),
        ("#FBCFE8", "#9D174D", "Plated brunch", "Food · 5 min"),
    ];
    let mut cy = 560.0;
    for (img, ink, title, meta) in cards {
        s.rect(72.0, cy, 936.0, 460.0, "#FFFFFF", "Feed card");
        s.rect(72.0, cy, 936.0, 300.0, img, "Card image");
        s.circle(180.0, cy + 150.0, 60.0, ink, "Card focal");
        s.text(110.0, cy + 330.0, 44.0, "#0F172A", title, "Card title");
        s.text(110.0, cy + 400.0, 30.0, "#64748B", meta, "Card meta");
        s.circle(940.0, cy + 360.0, 30.0, "#6366F1", "Save");
        cy += 500.0;
    }
    phone_tab_bar(&mut s, "#FFFFFF", "#6366F1", "#CBD5E1");
    BundledTemplate {
        dir_name: "mobile-feed.ktemplate",
        manifest: manifest(
            3,
            "Social Feed",
            "Mobile discovery feed with search, filter chips, image cards, and a bottom tab bar.",
            TemplateCategory::MobileApp,
            &["mobile", "feed", "social", "cards", "app"],
        ),
        content: s.finish(),
    }
}

fn mobile_profile() -> BundledTemplate {
    let mut s = Sheet::new(PHONE_W, PHONE_H);
    s.bg("#F8FAFC");
    // Cover.
    s.rect(0.0, 0.0, PHONE_W, 620.0, "#6366F1", "Cover");
    s.ellipse_a(900.0, 80.0, 320.0, 320.0, "#FFFFFF", 0.12, "Cover glow");
    phone_status_bar(&mut s, "#EEF2FF");
    // Avatar.
    s.circle(540.0, 560.0, 150.0, "#FFFFFF", "Avatar ring");
    s.circle(540.0, 560.0, 130.0, "#C7D2FE", "Avatar");
    s.circle(540.0, 560.0, 60.0, "#6366F1", "Avatar mark");
    s.text_center(540.0, 760.0, 60.0, "#0F172A", "Jordan Rivera", "Name");
    s.text_center(
        540.0,
        840.0,
        34.0,
        "#6366F1",
        "Product Designer · @jrivera",
        "Handle",
    );
    // Stats row.
    let stats = [
        ("248", "Posts"),
        ("18.2k", "Followers"),
        ("312", "Following"),
    ];
    let step = PHONE_W / 3.0;
    for (i, (num, label)) in stats.iter().enumerate() {
        let cx = step * (i as f64 + 0.5);
        s.text_center(cx, 960.0, 52.0, "#0F172A", num, "Stat number");
        s.text_center(cx, 1030.0, 30.0, "#64748B", label, "Stat label");
    }
    s.rect(72.0, 1110.0, 4.0, 0.0, "#E2E8F0", "spacer");
    // Buttons.
    s.rect(72.0, 1110.0, 580.0, 120.0, "#6366F1", "Follow button");
    s.text_center(362.0, 1146.0, 40.0, "#FFFFFF", "Follow", "Follow label");
    s.rect(688.0, 1110.0, 320.0, 120.0, "#E2E8F0", "Message button");
    s.text_center(848.0, 1146.0, 40.0, "#334155", "Message", "Message label");
    // Gallery grid.
    let cells = [
        "#FDE68A", "#BFDBFE", "#FBCFE8", "#BBF7D0", "#DDD6FE", "#FED7AA",
    ];
    for (i, hex) in cells.iter().enumerate() {
        let col = (i % 3) as f64;
        let row = (i / 3) as f64;
        let gx = 72.0 + col * 312.0;
        let gy = 1320.0 + row * 312.0;
        s.rect(gx, gy, 288.0, 288.0, hex, "Gallery cell");
    }
    phone_tab_bar(&mut s, "#FFFFFF", "#6366F1", "#CBD5E1");
    BundledTemplate {
        dir_name: "mobile-profile.ktemplate",
        manifest: manifest(
            4,
            "Profile Screen",
            "Mobile profile with cover, avatar, follower stats, action buttons, and a photo grid.",
            TemplateCategory::MobileApp,
            &["mobile", "profile", "account", "social", "app"],
        ),
        content: s.finish(),
    }
}

fn mobile_settings() -> BundledTemplate {
    let mut s = Sheet::new(PHONE_W, PHONE_H);
    s.bg("#F1F5F9");
    phone_status_bar(&mut s, "#0F172A");
    s.text(72.0, 120.0, 64.0, "#0F172A", "Settings", "Header");
    // Account card.
    s.rect(72.0, 250.0, 936.0, 220.0, "#FFFFFF", "Account card");
    s.circle(200.0, 360.0, 78.0, "#6366F1", "Account avatar");
    s.text(
        320.0,
        300.0,
        44.0,
        "#0F172A",
        "Jordan Rivera",
        "Account name",
    );
    s.text(
        320.0,
        370.0,
        32.0,
        "#64748B",
        "jordan@example.com",
        "Account email",
    );
    s.text(900.0, 345.0, 48.0, "#CBD5E1", ">", "Chevron");
    // Settings rows grouped.
    let rows = [
        ("Notifications", "#F59E0B", true),
        ("Privacy & Security", "#10B981", false),
        ("Appearance", "#6366F1", true),
        ("Language", "#EC4899", false),
        ("Storage", "#06B6D4", false),
        ("Help & Support", "#8B5CF6", false),
    ];
    let mut ry = 540.0;
    for (label, icon, toggle_on) in rows {
        s.rect(72.0, ry, 936.0, 150.0, "#FFFFFF", "Setting row");
        s.rect(120.0, ry + 45.0, 60.0, 60.0, icon, "Setting icon");
        s.text(220.0, ry + 52.0, 40.0, "#1E293B", label, "Setting label");
        // Toggle.
        let track = if toggle_on { "#6366F1" } else { "#CBD5E1" };
        s.rect(840.0, ry + 50.0, 110.0, 56.0, track, "Toggle track");
        let knob_x = if toggle_on { 896.0 } else { 846.0 };
        s.circle(knob_x + 24.0, ry + 78.0, 24.0, "#FFFFFF", "Toggle knob");
        ry += 168.0;
    }
    // Sign out.
    s.rect(72.0, ry + 40.0, 936.0, 140.0, "#FEE2E2", "Sign out");
    s.text_center(
        540.0,
        ry + 82.0,
        42.0,
        "#DC2626",
        "Sign Out",
        "Sign out label",
    );
    phone_tab_bar(&mut s, "#FFFFFF", "#6366F1", "#CBD5E1");
    BundledTemplate {
        dir_name: "mobile-settings.ktemplate",
        manifest: manifest(
            5,
            "Settings Screen",
            "Mobile settings list with grouped rows, colorful icons, toggles, and a sign-out action.",
            TemplateCategory::MobileApp,
            &["mobile", "settings", "preferences", "app"],
        ),
        content: s.finish(),
    }
}

fn mobile_music() -> BundledTemplate {
    let mut s = Sheet::new(PHONE_W, PHONE_H);
    s.bg("#181425");
    s.ellipse_a(540.0, 300.0, 520.0, 520.0, "#7C3AED", 0.30, "Glow");
    phone_status_bar(&mut s, "#E9D5FF");
    s.text_center(540.0, 130.0, 32.0, "#C4B5FD", "NOW PLAYING", "Eyebrow");
    // Album art.
    s.rect(150.0, 320.0, 780.0, 780.0, "#7C3AED", "Album art");
    s.ellipse_a(540.0, 710.0, 300.0, 300.0, "#C4B5FD", 0.35, "Album ring");
    s.circle(540.0, 710.0, 90.0, "#181425", "Album hole");
    s.circle(540.0, 710.0, 28.0, "#C4B5FD", "Album spindle");
    // Track meta.
    s.text_center(
        540.0,
        1200.0,
        60.0,
        "#F5F3FF",
        "Midnight Drive",
        "Track title",
    );
    s.text_center(540.0, 1290.0, 38.0, "#A78BFA", "The Wanderers", "Artist");
    // Progress bar.
    s.rect(150.0, 1420.0, 780.0, 12.0, "#3B3550", "Progress track");
    s.rect(150.0, 1420.0, 460.0, 12.0, "#A78BFA", "Progress fill");
    s.circle(610.0, 1426.0, 22.0, "#F5F3FF", "Progress knob");
    s.text(150.0, 1460.0, 30.0, "#8B82A8", "1:48", "Elapsed");
    s.text(852.0, 1460.0, 30.0, "#8B82A8", "3:52", "Duration");
    // Transport controls.
    s.circle(300.0, 1640.0, 44.0, "#C4B5FD", "Prev");
    s.circle(780.0, 1640.0, 44.0, "#C4B5FD", "Next");
    s.circle(540.0, 1640.0, 96.0, "#A78BFA", "Play");
    s.rect(516.0, 1596.0, 22.0, 88.0, "#181425", "Play bar 1");
    s.rect(556.0, 1596.0, 22.0, 88.0, "#181425", "Play bar 2");
    // Secondary controls.
    s.circle(190.0, 1860.0, 34.0, "#6D6488", "Shuffle");
    s.circle(400.0, 1860.0, 34.0, "#6D6488", "Repeat");
    s.circle(680.0, 1860.0, 34.0, "#6D6488", "Heart");
    s.circle(890.0, 1860.0, 34.0, "#6D6488", "Queue");
    BundledTemplate {
        dir_name: "mobile-music.ktemplate",
        manifest: manifest(
            6,
            "Music Player",
            "Dark mobile music player with album art, scrubber, transport controls, and secondary actions.",
            TemplateCategory::MobileApp,
            &["mobile", "music", "player", "audio", "app"],
        ),
        content: s.finish(),
    }
}

// ---------------------------------------------------------------------
// Catalog — presentation slides (1920 × 1080, 16:9)
// ---------------------------------------------------------------------

const SLIDE_W: f64 = 1920.0;
const SLIDE_H: f64 = 1080.0;

fn deck_title() -> BundledTemplate {
    let mut s = Sheet::new(SLIDE_W, SLIDE_H);
    s.bg("#0F172A");
    s.rect(0.0, 0.0, 720.0, SLIDE_H, "#4338CA", "Sidebar");
    s.ellipse_a(640.0, 220.0, 420.0, 420.0, "#6366F1", 0.45, "Glow A");
    s.ellipse_a(120.0, 980.0, 360.0, 360.0, "#22D3EE", 0.22, "Glow B");
    // Brand row.
    s.circle(150.0, 150.0, 44.0, "#A5B4FC", "Logo");
    s.text(220.0, 122.0, 40.0, "#E0E7FF", "NORTHWIND", "Brand");
    // Sidebar marks.
    s.rect(150.0, 560.0, 420.0, 14.0, "#A5B4FC", "Rule");
    s.text(
        150.0,
        620.0,
        34.0,
        "#C7D2FE",
        "Series A · 2025",
        "Sidebar meta",
    );
    // Headline on the dark side.
    s.text(820.0, 360.0, 130.0, "#F8FAFC", "Northwind", "Title 1");
    s.text(820.0, 500.0, 130.0, "#818CF8", "Logistics", "Title 2");
    s.rect(824.0, 680.0, 220.0, 14.0, "#22D3EE", "Accent rule");
    s.text(
        824.0,
        740.0,
        44.0,
        "#CBD5E1",
        "Moving the supply chain forward",
        "Subtitle",
    );
    s.text(
        824.0,
        940.0,
        32.0,
        "#64748B",
        "Investor Pitch — Confidential",
        "Footer",
    );
    BundledTemplate {
        dir_name: "deck-title.ktemplate",
        manifest: manifest(
            7,
            "Pitch Deck — Title",
            "16:9 title slide with a color sidebar, brand lockup, and bold headline for an investor deck.",
            TemplateCategory::Presentation,
            &["presentation", "deck", "slide", "title", "pitch"],
        ),
        content: s.finish(),
    }
}

fn deck_agenda() -> BundledTemplate {
    let mut s = Sheet::new(SLIDE_W, SLIDE_H);
    s.bg("#FFFFFF");
    s.rect(0.0, 0.0, SLIDE_W, 16.0, "#4338CA", "Top rule");
    s.text(140.0, 130.0, 40.0, "#6366F1", "01 — AGENDA", "Eyebrow");
    s.text(140.0, 200.0, 96.0, "#0F172A", "What we'll cover", "Title");
    s.rect(146.0, 340.0, 180.0, 12.0, "#22D3EE", "Accent");
    let items = [
        ("01", "The problem", "Why logistics is broken today"),
        ("02", "Our solution", "An autonomous routing network"),
        ("03", "Market & traction", "$48B market, 3x QoQ growth"),
        ("04", "The ask", "Raising $6M to scale operations"),
    ];
    let mut y = 470.0;
    for (num, title, desc) in items {
        s.circle(220.0, y + 60.0, 60.0, "#EEF2FF", "Number chip");
        s.text_center(220.0, y + 26.0, 52.0, "#4338CA", num, "Number");
        s.text(340.0, y, 56.0, "#0F172A", title, "Item title");
        s.text(340.0, y + 78.0, 34.0, "#64748B", desc, "Item desc");
        s.rect(340.0, y + 140.0, 1400.0, 3.0, "#E2E8F0", "Divider");
        y += 165.0;
    }
    BundledTemplate {
        dir_name: "deck-agenda.ktemplate",
        manifest: manifest(
            8,
            "Pitch Deck — Agenda",
            "Clean 16:9 agenda slide with numbered sections and supporting captions.",
            TemplateCategory::Presentation,
            &["presentation", "agenda", "slide", "outline"],
        ),
        content: s.finish(),
    }
}

fn deck_metrics() -> BundledTemplate {
    let mut s = Sheet::new(SLIDE_W, SLIDE_H);
    s.bg("#0F172A");
    s.text(140.0, 120.0, 40.0, "#818CF8", "03 — TRACTION", "Eyebrow");
    s.text(
        140.0,
        190.0,
        92.0,
        "#F8FAFC",
        "Growth that compounds",
        "Title",
    );
    let cards = [
        ("#6366F1", "3.2x", "Revenue YoY"),
        ("#22D3EE", "48k", "Active shippers"),
        ("#34D399", "98%", "On-time delivery"),
        ("#FBBF24", "$12M", "ARR run-rate"),
    ];
    let card_w = 380.0;
    let gap = 60.0;
    let total = card_w * 4.0 + gap * 3.0;
    let start_x = (SLIDE_W - total) / 2.0;
    for (i, (hex, num, label)) in cards.iter().enumerate() {
        let x = start_x + i as f64 * (card_w + gap);
        s.rect(x, 420.0, card_w, 420.0, "#1E293B", "Metric card");
        s.rect(x, 420.0, card_w, 16.0, hex, "Metric accent");
        s.circle(x + 70.0, 520.0, 36.0, hex, "Metric dot");
        s.text(x + 40.0, 600.0, 110.0, "#F8FAFC", num, "Metric number");
        s.text(x + 40.0, 740.0, 34.0, "#94A3B8", label, "Metric label");
    }
    s.text_center(
        960.0,
        920.0,
        32.0,
        "#64748B",
        "Trailing twelve months, audited",
        "Footnote",
    );
    BundledTemplate {
        dir_name: "deck-metrics.ktemplate",
        manifest: manifest(
            9,
            "Pitch Deck — Metrics",
            "Dark 16:9 KPI slide with four accent metric cards for traction storytelling.",
            TemplateCategory::Presentation,
            &["presentation", "metrics", "kpi", "traction", "slide"],
        ),
        content: s.finish(),
    }
}

fn deck_team() -> BundledTemplate {
    let mut s = Sheet::new(SLIDE_W, SLIDE_H);
    s.bg("#FFFFFF");
    s.rect(0.0, 0.0, SLIDE_W, 16.0, "#4338CA", "Top rule");
    s.text(140.0, 120.0, 40.0, "#6366F1", "05 — TEAM", "Eyebrow");
    s.text(
        140.0,
        190.0,
        92.0,
        "#0F172A",
        "The people behind it",
        "Title",
    );
    let team = [
        ("#C7D2FE", "#4338CA", "Maya Chen", "Co-founder & CEO"),
        ("#BBF7D0", "#047857", "Liam Osei", "Co-founder & CTO"),
        ("#FBCFE8", "#9D174D", "Sofia Marín", "Head of Ops"),
        ("#FDE68A", "#92400E", "Noah Park", "Head of Product"),
    ];
    let card_w = 380.0;
    let gap = 60.0;
    let total = card_w * 4.0 + gap * 3.0;
    let start_x = (SLIDE_W - total) / 2.0;
    for (i, (ring, mark, name, role)) in team.iter().enumerate() {
        let x = start_x + i as f64 * (card_w + gap);
        let cx = x + card_w / 2.0;
        s.rect(x, 400.0, card_w, 470.0, "#F8FAFC", "Member card");
        s.circle(cx, 540.0, 110.0, ring, "Avatar ring");
        s.circle(cx, 540.0, 56.0, mark, "Avatar mark");
        s.text_center(cx, 690.0, 46.0, "#0F172A", name, "Member name");
        s.text_center(cx, 750.0, 30.0, "#64748B", role, "Member role");
    }
    BundledTemplate {
        dir_name: "deck-team.ktemplate",
        manifest: manifest(
            10,
            "Pitch Deck — Team",
            "16:9 team slide with four member cards, avatars, names, and roles.",
            TemplateCategory::Presentation,
            &["presentation", "team", "people", "slide"],
        ),
        content: s.finish(),
    }
}

// ---------------------------------------------------------------------
// Catalog — pitch one-pager + business covers (A4 portrait, 1240 × 1754)
// ---------------------------------------------------------------------

const A4_W: f64 = 1240.0;
const A4_H: f64 = 1754.0;

fn pitch_onepager() -> BundledTemplate {
    let mut s = Sheet::new(A4_W, A4_H);
    s.bg("#F8FAFC");
    // Header band.
    s.rect(0.0, 0.0, A4_W, 360.0, "#4338CA", "Header band");
    s.ellipse_a(1120.0, 60.0, 260.0, 260.0, "#FFFFFF", 0.12, "Header glow");
    s.circle(110.0, 130.0, 40.0, "#A5B4FC", "Logo");
    s.text(178.0, 104.0, 38.0, "#E0E7FF", "NORTHWIND", "Brand");
    s.text(
        80.0,
        190.0,
        76.0,
        "#FFFFFF",
        "Autonomous logistics",
        "Title",
    );
    s.text(
        80.0,
        280.0,
        36.0,
        "#C7D2FE",
        "Series A one-pager · 2025",
        "Subtitle",
    );
    // Two-column body.
    s.text(80.0, 430.0, 40.0, "#4338CA", "Problem", "H Problem");
    s.text(
        80.0,
        500.0,
        30.0,
        "#334155",
        "Freight routing wastes 30% of",
        "Problem 1",
    );
    s.text(
        80.0,
        544.0,
        30.0,
        "#334155",
        "miles. Legacy tools are manual,",
        "Problem 2",
    );
    s.text(
        80.0,
        588.0,
        30.0,
        "#334155",
        "slow, and blind to live demand.",
        "Problem 3",
    );
    s.text(660.0, 430.0, 40.0, "#4338CA", "Solution", "H Solution");
    s.text(
        660.0,
        500.0,
        30.0,
        "#334155",
        "A self-optimizing routing network",
        "Solution 1",
    );
    s.text(
        660.0,
        544.0,
        30.0,
        "#334155",
        "that re-plans in real time and cuts",
        "Solution 2",
    );
    s.text(
        660.0,
        588.0,
        30.0,
        "#334155",
        "empty miles by up to 22%.",
        "Solution 3",
    );
    // Metric strip.
    s.rect(80.0, 720.0, 1080.0, 280.0, "#EEF2FF", "Metric strip");
    let metrics = [
        ("3.2x", "Revenue YoY"),
        ("48k", "Shippers"),
        ("$12M", "ARR"),
    ];
    for (i, (num, label)) in metrics.iter().enumerate() {
        let cx = 80.0 + 1080.0 / 3.0 * (i as f64 + 0.5);
        s.text_center(cx, 790.0, 96.0, "#4338CA", num, "Metric number");
        s.text_center(cx, 910.0, 32.0, "#475569", label, "Metric label");
    }
    // Roadmap bars.
    s.text(80.0, 1070.0, 40.0, "#4338CA", "Why now", "H Why");
    let bars = [
        ("Market tailwind", 0.9, "#6366F1"),
        ("Tech readiness", 0.75, "#22D3EE"),
        ("Unit economics", 0.62, "#34D399"),
    ];
    let mut by = 1150.0;
    for (label, frac, hex) in bars {
        s.text(80.0, by, 30.0, "#334155", label, "Bar label");
        s.rect(80.0, by + 50.0, 1080.0, 28.0, "#E2E8F0", "Bar track");
        s.rect(80.0, by + 50.0, 1080.0 * frac, 28.0, hex, "Bar fill");
        by += 130.0;
    }
    // CTA footer.
    s.rect(0.0, A4_H - 180.0, A4_W, 180.0, "#0F172A", "Footer");
    s.text(
        80.0,
        A4_H - 130.0,
        38.0,
        "#F8FAFC",
        "Raising $6M",
        "Footer ask",
    );
    s.text(
        80.0,
        A4_H - 80.0,
        28.0,
        "#94A3B8",
        "hello@northwind.co",
        "Footer contact",
    );
    s.rect(900.0, A4_H - 130.0, 260.0, 90.0, "#6366F1", "CTA button");
    s.text_center(
        1030.0,
        A4_H - 110.0,
        34.0,
        "#FFFFFF",
        "Book a call",
        "CTA label",
    );
    BundledTemplate {
        dir_name: "pitch-onepager.ktemplate",
        manifest: manifest(
            11,
            "Startup One-Pager",
            "A4 investor one-pager: problem/solution columns, metric strip, why-now bars, and a CTA footer.",
            TemplateCategory::PitchDeck,
            &["pitch", "one-pager", "startup", "investor", "summary"],
        ),
        content: s.finish(),
    }
}

// ---------------------------------------------------------------------
// Catalog — social media
// ---------------------------------------------------------------------

fn social_ig_post() -> BundledTemplate {
    let mut s = Sheet::new(1080.0, 1080.0);
    s.bg("#DB2777");
    s.ellipse_a(880.0, 200.0, 360.0, 360.0, "#FFFFFF", 0.14, "Glow A");
    s.ellipse_a(180.0, 920.0, 320.0, 320.0, "#7C1D43", 0.30, "Glow B");
    s.rect(90.0, 90.0, 200.0, 64.0, "#FFFFFF", "Tag");
    s.text_center(190.0, 104.0, 32.0, "#DB2777", "NEW DROP", "Tag label");
    s.text(90.0, 360.0, 120.0, "#FFFFFF", "Summer", "Title 1");
    s.text(90.0, 490.0, 120.0, "#FBCFE8", "Collection", "Title 2");
    s.rect(96.0, 660.0, 240.0, 12.0, "#FCD34D", "Accent rule");
    s.text(
        90.0,
        720.0,
        40.0,
        "#FCE7F3",
        "Up to 40% off everything",
        "Subtitle",
    );
    // Price badge.
    s.circle(860.0, 800.0, 150.0, "#FCD34D", "Badge");
    s.text_center(860.0, 740.0, 44.0, "#9D174D", "FROM", "Badge top");
    s.text_center(860.0, 790.0, 96.0, "#9D174D", "$29", "Badge price");
    s.text(
        90.0,
        960.0,
        32.0,
        "#FBCFE8",
        "@yourbrand · shop now",
        "Handle",
    );
    BundledTemplate {
        dir_name: "social-ig-post.ktemplate",
        manifest: manifest(
            12,
            "Instagram Post — Promo",
            "1:1 promotional Instagram post with bold type, accent rule, and a price badge.",
            TemplateCategory::SocialMedia,
            &["social", "instagram", "post", "promo", "sale"],
        ),
        content: s.finish(),
    }
}

fn social_ig_story() -> BundledTemplate {
    let mut s = Sheet::new(1080.0, 1920.0);
    s.bg("#0F172A");
    s.rect(0.0, 0.0, 1080.0, 760.0, "#F97316", "Top block");
    s.ellipse_a(900.0, 160.0, 320.0, 320.0, "#FFFFFF", 0.16, "Glow");
    // Story progress segments.
    for i in 0..4 {
        let seg_w = 230.0;
        let x = 40.0 + f64::from(i) * (seg_w + 20.0);
        let alpha = if i == 0 { 1.0 } else { 0.35 };
        s.rect_a(x, 40.0, seg_w, 10.0, "#FFFFFF", alpha, "Progress segment");
    }
    s.text(70.0, 320.0, 120.0, "#FFFFFF", "FLASH", "Title 1");
    s.text(70.0, 450.0, 120.0, "#0F172A", "SALE", "Title 2");
    s.text_center(540.0, 900.0, 60.0, "#F8FAFC", "48 hours only", "Subtitle");
    // Center medallion.
    s.circle(540.0, 1230.0, 250.0, "#F97316", "Medallion");
    s.circle(540.0, 1230.0, 200.0, "#0F172A", "Medallion inner");
    s.text_center(540.0, 1150.0, 50.0, "#F8FAFC", "SAVE", "Medallion top");
    s.text_center(540.0, 1200.0, 150.0, "#F97316", "50%", "Medallion percent");
    // Swipe-up pill.
    s.rect(290.0, 1650.0, 500.0, 130.0, "#F97316", "CTA pill");
    s.text_center(
        540.0,
        1690.0,
        44.0,
        "#0F172A",
        "Swipe up to shop",
        "CTA label",
    );
    s.circle(540.0, 1560.0, 28.0, "#F8FAFC", "Swipe chevron");
    BundledTemplate {
        dir_name: "social-ig-story.ktemplate",
        manifest: manifest(
            13,
            "Instagram Story — Sale",
            "9:16 story with progress segments, flash-sale headline, percent medallion, and swipe-up CTA.",
            TemplateCategory::SocialMedia,
            &["social", "instagram", "story", "sale", "vertical"],
        ),
        content: s.finish(),
    }
}

fn social_linkedin() -> BundledTemplate {
    let mut s = Sheet::new(1584.0, 396.0);
    s.bg("#0F172A");
    s.rect(0.0, 0.0, 560.0, 396.0, "#0EA5E9", "Left block");
    s.ellipse_a(1380.0, 120.0, 280.0, 280.0, "#38BDF8", 0.30, "Glow");
    s.circle(120.0, 198.0, 70.0, "#FFFFFF", "Avatar ring");
    s.circle(120.0, 198.0, 50.0, "#0EA5E9", "Avatar mark");
    s.text(230.0, 150.0, 52.0, "#FFFFFF", "Jordan Rivera", "Name");
    s.text(230.0, 220.0, 30.0, "#E0F2FE", "Product Designer", "Role");
    s.rect(700.0, 150.0, 12.0, 96.0, "#38BDF8", "Accent bar");
    s.text(
        740.0,
        150.0,
        44.0,
        "#F8FAFC",
        "Designing calm, useful",
        "Tagline 1",
    );
    s.text(
        740.0,
        210.0,
        44.0,
        "#94A3B8",
        "products for everyday work",
        "Tagline 2",
    );
    BundledTemplate {
        dir_name: "social-linkedin.ktemplate",
        manifest: manifest(
            14,
            "LinkedIn Banner",
            "1584×396 LinkedIn header with avatar lockup, name/role, and a tagline.",
            TemplateCategory::SocialMedia,
            &["social", "linkedin", "banner", "header", "cover"],
        ),
        content: s.finish(),
    }
}

fn social_quote() -> BundledTemplate {
    let mut s = Sheet::new(1080.0, 1080.0);
    s.bg("#FEF3C7");
    s.rect(0.0, 0.0, 1080.0, 24.0, "#B45309", "Top rule");
    s.rect(0.0, 1056.0, 1080.0, 24.0, "#B45309", "Bottom rule");
    s.text(110.0, 200.0, 220.0, "#F59E0B", "\u{201C}", "Quote mark");
    s.text(
        120.0,
        400.0,
        64.0,
        "#451A03",
        "Simplicity is the",
        "Quote 1",
    );
    s.text(120.0, 490.0, 64.0, "#451A03", "ultimate", "Quote 2");
    s.text(120.0, 580.0, 64.0, "#B45309", "sophistication.", "Quote 3");
    s.rect(120.0, 720.0, 120.0, 10.0, "#B45309", "Byline rule");
    s.text(120.0, 760.0, 38.0, "#78350F", "Leonardo da Vinci", "Byline");
    s.circle(900.0, 880.0, 90.0, "#F59E0B", "Brand dot");
    s.text(
        110.0,
        980.0,
        30.0,
        "#92400E",
        "@dailydesignquotes",
        "Handle",
    );
    BundledTemplate {
        dir_name: "social-quote.ktemplate",
        manifest: manifest(
            15,
            "Quote Card",
            "1:1 quote card with oversized quotation mark, layered headline, and byline.",
            TemplateCategory::SocialMedia,
            &["social", "quote", "post", "typography"],
        ),
        content: s.finish(),
    }
}

// ---------------------------------------------------------------------
// Catalog — posters & flyer (1080 × 1350, 4:5 print-friendly)
// ---------------------------------------------------------------------

const POSTER_W: f64 = 1080.0;
const POSTER_H: f64 = 1350.0;

fn poster_event() -> BundledTemplate {
    let mut s = Sheet::new(POSTER_W, POSTER_H);
    s.bg("#1E1B4B");
    s.rect(0.0, 0.0, POSTER_W, 70.0, "#FBBF24", "Top rule");
    s.text(80.0, 150.0, 36.0, "#A5B4FC", "DESIGN CONFERENCE", "Eyebrow");
    s.text(80.0, 230.0, 150.0, "#FFFFFF", "FORM", "Title 1");
    s.text(80.0, 380.0, 150.0, "#FBBF24", "&FLOW", "Title 2");
    s.text(
        80.0,
        580.0,
        40.0,
        "#C7D2FE",
        "A summit on craft, systems,",
        "Subtitle 1",
    );
    s.text(
        80.0,
        632.0,
        40.0,
        "#C7D2FE",
        "and the future of design",
        "Subtitle 2",
    );
    // Detail rows.
    s.rect(80.0, 780.0, 920.0, 4.0, "#4338CA", "Divider");
    s.text(80.0, 820.0, 44.0, "#FFFFFF", "OCT 24 — 26", "Date");
    s.text(80.0, 890.0, 32.0, "#A5B4FC", "Berlin · Kraftwerk", "Venue");
    s.rect(80.0, 970.0, 920.0, 4.0, "#4338CA", "Divider 2");
    // Speaker dots.
    let dots = ["#F472B6", "#22D3EE", "#34D399", "#FBBF24"];
    for (i, hex) in dots.iter().enumerate() {
        s.circle(120.0 + i as f64 * 90.0, 1080.0, 44.0, hex, "Speaker");
    }
    s.text(
        420.0,
        1058.0,
        32.0,
        "#C7D2FE",
        "+ 24 speakers",
        "Speaker count",
    );
    // Ticket button.
    s.rect(80.0, 1180.0, 920.0, 110.0, "#FBBF24", "Ticket button");
    s.text_center(
        540.0,
        1212.0,
        42.0,
        "#1E1B4B",
        "Get tickets — formflow.io",
        "Ticket label",
    );
    BundledTemplate {
        dir_name: "poster-event.ktemplate",
        manifest: manifest(
            16,
            "Event Poster",
            "4:5 conference poster with massive display type, event details, speaker dots, and a ticket CTA.",
            TemplateCategory::Poster,
            &["poster", "event", "conference", "summit"],
        ),
        content: s.finish(),
    }
}

fn poster_concert() -> BundledTemplate {
    let mut s = Sheet::new(POSTER_W, POSTER_H);
    s.bg("#111827");
    s.ellipse_a(540.0, 470.0, 430.0, 430.0, "#F43F5E", 0.85, "Sun");
    s.ellipse_a(540.0, 470.0, 430.0, 430.0, "#FB7185", 0.0, "Sun ring");
    s.rect(0.0, 700.0, POSTER_W, 650.0, "#111827", "Lower mask");
    // Horizon bars over the sun.
    for i in 0..6 {
        let y = 520.0 + f64::from(i) * 34.0;
        s.rect(110.0, y, 860.0, 16.0, "#111827", "Horizon bar");
    }
    s.text_center(540.0, 120.0, 36.0, "#FCA5A5", "LIVE IN CONCERT", "Eyebrow");
    s.text_center(540.0, 820.0, 150.0, "#F8FAFC", "NEON", "Title 1");
    s.text_center(540.0, 970.0, 150.0, "#F43F5E", "TIDES", "Title 2");
    s.text_center(
        540.0,
        1160.0,
        38.0,
        "#E5E7EB",
        "Sat · Nov 9 · The Warehouse",
        "Details",
    );
    s.rect(340.0, 1240.0, 400.0, 90.0, "#F43F5E", "Ticket button");
    s.text_center(
        540.0,
        1265.0,
        36.0,
        "#111827",
        "Tickets $35",
        "Ticket label",
    );
    BundledTemplate {
        dir_name: "poster-concert.ktemplate",
        manifest: manifest(
            17,
            "Concert Poster",
            "4:5 gig poster with a retro sunset motif, layered title, and ticket details.",
            TemplateCategory::Poster,
            &["poster", "concert", "music", "gig", "event"],
        ),
        content: s.finish(),
    }
}

fn flyer_sale() -> BundledTemplate {
    let mut s = Sheet::new(POSTER_W, POSTER_H);
    s.bg("#FFFFFF");
    s.rect(0.0, 0.0, POSTER_W, 520.0, "#16A34A", "Header block");
    s.ellipse_a(940.0, 80.0, 260.0, 260.0, "#FFFFFF", 0.14, "Glow");
    s.text(80.0, 110.0, 40.0, "#DCFCE7", "WEEKEND ONLY", "Eyebrow");
    s.text(80.0, 180.0, 170.0, "#FFFFFF", "MEGA", "Title 1");
    s.text(80.0, 340.0, 170.0, "#FACC15", "SALE", "Title 2");
    // Big discount disc.
    s.circle(840.0, 560.0, 200.0, "#FACC15", "Discount disc");
    s.text_center(840.0, 470.0, 54.0, "#166534", "UP TO", "Disc top");
    s.text_center(840.0, 520.0, 150.0, "#166534", "70%", "Disc percent");
    s.text_center(840.0, 670.0, 44.0, "#166534", "OFF", "Disc bottom");
    // Product rows.
    let rows = [
        ("Footwear", "from $24"),
        ("Outerwear", "from $39"),
        ("Accessories", "from $9"),
    ];
    let mut y = 820.0;
    for (label, price) in rows {
        s.rect(80.0, y, 920.0, 130.0, "#F1F5F9", "Product row");
        s.circle(160.0, y + 65.0, 44.0, "#16A34A", "Product dot");
        s.text(250.0, y + 38.0, 46.0, "#0F172A", label, "Product label");
        s.text(740.0, y + 42.0, 42.0, "#16A34A", price, "Product price");
        y += 150.0;
    }
    // Footer.
    s.rect(0.0, POSTER_H - 110.0, POSTER_W, 110.0, "#0F172A", "Footer");
    s.text_center(
        540.0,
        POSTER_H - 78.0,
        36.0,
        "#F8FAFC",
        "123 Market St · Open 9–9 daily",
        "Footer",
    );
    BundledTemplate {
        dir_name: "flyer-sale.ktemplate",
        manifest: manifest(
            18,
            "Sale Flyer",
            "4:5 retail sale flyer with a discount disc, product rows with prices, and a store footer.",
            TemplateCategory::Flyer,
            &["flyer", "sale", "retail", "promo", "discount"],
        ),
        content: s.finish(),
    }
}

// ---------------------------------------------------------------------
// Catalog — résumé + business covers (A4 portrait)
// ---------------------------------------------------------------------

fn resume_cv() -> BundledTemplate {
    let mut s = Sheet::new(A4_W, A4_H);
    s.bg("#FFFFFF");
    // Left sidebar.
    s.rect(0.0, 0.0, 430.0, A4_H, "#0E7490", "Sidebar");
    s.circle(215.0, 240.0, 130.0, "#ECFEFF", "Avatar ring");
    s.circle(215.0, 240.0, 100.0, "#67E8F9", "Avatar");
    s.circle(215.0, 240.0, 44.0, "#0E7490", "Avatar mark");
    s.text_center(215.0, 420.0, 30.0, "#A5F3FC", "CONTACT", "Sidebar H1");
    s.rect(70.0, 470.0, 290.0, 3.0, "#22D3EE", "Sidebar rule 1");
    let contact = [
        "jordan@mail.com",
        "+1 555 0142",
        "linkedin/jrivera",
        "San Francisco",
    ];
    let mut cy = 510.0;
    for line in contact {
        s.circle(95.0, cy + 16.0, 12.0, "#67E8F9", "Contact dot");
        s.text(130.0, cy, 26.0, "#E0F7FA", line, "Contact line");
        cy += 70.0;
    }
    s.text_center(215.0, 860.0, 30.0, "#A5F3FC", "SKILLS", "Sidebar H2");
    s.rect(70.0, 910.0, 290.0, 3.0, "#22D3EE", "Sidebar rule 2");
    let skills = [
        ("Product design", 0.92),
        ("Prototyping", 0.84),
        ("Design systems", 0.78),
        ("User research", 0.7),
    ];
    let mut sy = 950.0;
    for (label, frac) in skills {
        s.text(70.0, sy, 26.0, "#E0F7FA", label, "Skill label");
        s.rect(70.0, sy + 44.0, 290.0, 18.0, "#155E75", "Skill track");
        s.rect(70.0, sy + 44.0, 290.0 * frac, 18.0, "#67E8F9", "Skill fill");
        sy += 110.0;
    }
    // Main column.
    s.text(490.0, 130.0, 84.0, "#0F172A", "Jordan Rivera", "Name");
    s.text(
        490.0,
        240.0,
        40.0,
        "#0E7490",
        "Senior Product Designer",
        "Role",
    );
    s.rect(490.0, 320.0, 670.0, 4.0, "#E2E8F0", "Header rule");
    s.text(
        490.0,
        360.0,
        30.0,
        "#475569",
        "Designer with 8+ years crafting calm,",
        "Summary 1",
    );
    s.text(
        490.0,
        400.0,
        30.0,
        "#475569",
        "accessible products end to end.",
        "Summary 2",
    );
    s.text(490.0, 500.0, 38.0, "#0E7490", "EXPERIENCE", "Section");
    let jobs = [
        (
            "Lead Designer — Northwind",
            "2021–Present",
            "Owned the design system and core flows.",
        ),
        (
            "Product Designer — Lumen",
            "2018–2021",
            "Shipped onboarding that lifted activation 28%.",
        ),
        (
            "UX Designer — Studio Co",
            "2016–2018",
            "Designed mobile apps for early-stage startups.",
        ),
    ];
    let mut jy = 570.0;
    for (title, dates, desc) in jobs {
        s.circle(505.0, jy + 18.0, 12.0, "#0E7490", "Job bullet");
        s.text(540.0, jy, 36.0, "#0F172A", title, "Job title");
        s.text(540.0, jy + 48.0, 26.0, "#0E7490", dates, "Job dates");
        s.text(540.0, jy + 92.0, 28.0, "#475569", desc, "Job desc");
        jy += 200.0;
    }
    s.text(
        490.0,
        jy + 20.0,
        38.0,
        "#0E7490",
        "EDUCATION",
        "Education H",
    );
    s.text(
        540.0,
        jy + 90.0,
        34.0,
        "#0F172A",
        "B.A. Design — RISD",
        "Education line",
    );
    s.text(
        540.0,
        jy + 138.0,
        26.0,
        "#0E7490",
        "2012–2016",
        "Education dates",
    );
    BundledTemplate {
        dir_name: "resume-cv.ktemplate",
        manifest: manifest(
            19,
            "Resume / CV",
            "A4 two-column résumé with a teal sidebar (contact + skill bars) and an experience timeline.",
            TemplateCategory::Resume,
            &["resume", "cv", "job", "career", "two-column"],
        ),
        content: s.finish(),
    }
}

fn report_cover() -> BundledTemplate {
    let mut s = Sheet::new(A4_W, A4_H);
    s.bg("#F8FAFC");
    s.rect(0.0, 0.0, A4_W, 980.0, "#1D4ED8", "Cover block");
    s.ellipse_a(1080.0, 160.0, 320.0, 320.0, "#FFFFFF", 0.10, "Glow");
    s.circle(110.0, 140.0, 40.0, "#93C5FD", "Logo");
    s.text(178.0, 116.0, 36.0, "#DBEAFE", "MERIDIAN", "Brand");
    s.text(80.0, 420.0, 36.0, "#93C5FD", "ANNUAL REPORT", "Eyebrow");
    s.text(80.0, 500.0, 130.0, "#FFFFFF", "2025", "Year");
    s.text(
        80.0,
        680.0,
        50.0,
        "#DBEAFE",
        "Growth, resilience,",
        "Subtitle 1",
    );
    s.text(
        80.0,
        745.0,
        50.0,
        "#DBEAFE",
        "and the road ahead",
        "Subtitle 2",
    );
    // Highlights.
    let highlights = [("Revenue", "$248M"), ("Customers", "12,400"), ("NPS", "72")];
    for (i, (label, num)) in highlights.iter().enumerate() {
        let x = 80.0 + i as f64 * 370.0;
        s.rect(x, 1080.0, 320.0, 240.0, "#FFFFFF", "Highlight card");
        s.rect(x, 1080.0, 320.0, 12.0, "#1D4ED8", "Highlight accent");
        s.text(x + 40.0, 1130.0, 70.0, "#1D4ED8", num, "Highlight number");
        s.text(x + 40.0, 1230.0, 30.0, "#64748B", label, "Highlight label");
    }
    s.rect(80.0, 1440.0, 1080.0, 4.0, "#CBD5E1", "Footer rule");
    s.text(
        80.0,
        1480.0,
        30.0,
        "#475569",
        "Prepared for shareholders · Confidential",
        "Footer",
    );
    BundledTemplate {
        dir_name: "report-cover.ktemplate",
        manifest: manifest(
            20,
            "Report Cover",
            "A4 annual-report cover with a deep-blue header, oversized year, and highlight cards.",
            TemplateCategory::Report,
            &["report", "cover", "annual", "business"],
        ),
        content: s.finish(),
    }
}

fn brochure_cover() -> BundledTemplate {
    let mut s = Sheet::new(A4_W, A4_H);
    s.bg("#FFFFFF");
    // Diagonal-feel color blocks (stacked rects).
    s.rect(0.0, 0.0, A4_W, 760.0, "#0F766E", "Top block");
    s.rect(0.0, 760.0, A4_W, 120.0, "#14B8A6", "Mid band");
    s.ellipse_a(980.0, 220.0, 300.0, 300.0, "#5EEAD4", 0.30, "Glow");
    s.circle(110.0, 150.0, 40.0, "#5EEAD4", "Logo");
    s.text(178.0, 126.0, 36.0, "#CCFBF1", "EVERGREEN", "Brand");
    s.text(80.0, 360.0, 96.0, "#FFFFFF", "Sustainable", "Title 1");
    s.text(80.0, 470.0, 96.0, "#99F6E4", "by design", "Title 2");
    s.text(
        80.0,
        800.0,
        38.0,
        "#042F2E",
        "Product & services brochure 2025",
        "Subtitle",
    );
    // Body teasers.
    let teasers = [
        ("Our mission", "Building products that last a lifetime."),
        (
            "What we do",
            "Design, manufacture, and recycle — in a loop.",
        ),
        ("Why it matters", "Less waste, better materials, fair work."),
    ];
    let mut y = 980.0;
    for (title, desc) in teasers {
        s.rect(80.0, y + 8.0, 16.0, 90.0, "#14B8A6", "Teaser bar");
        s.text(130.0, y, 44.0, "#0F172A", title, "Teaser title");
        s.text(130.0, y + 60.0, 30.0, "#475569", desc, "Teaser desc");
        y += 180.0;
    }
    s.rect(0.0, A4_H - 120.0, A4_W, 120.0, "#0F766E", "Footer");
    s.text(
        80.0,
        A4_H - 82.0,
        30.0,
        "#CCFBF1",
        "evergreen.co · hello@evergreen.co",
        "Footer",
    );
    BundledTemplate {
        dir_name: "brochure-cover.ktemplate",
        manifest: manifest(
            21,
            "Brochure Cover",
            "A4 brochure cover with teal color blocks, a bold headline, and three content teasers.",
            TemplateCategory::Brochure,
            &["brochure", "cover", "marketing", "company"],
        ),
        content: s.finish(),
    }
}

fn proposal_cover() -> BundledTemplate {
    let mut s = Sheet::new(A4_W, A4_H);
    s.bg("#111827");
    s.rect(
        80.0,
        80.0,
        A4_W - 160.0,
        A4_H - 160.0,
        "#0B1220",
        "Inner panel",
    );
    s.rect(80.0, 80.0, A4_W - 160.0, 16.0, "#F59E0B", "Top accent");
    s.circle(170.0, 220.0, 44.0, "#F59E0B", "Logo");
    s.text(244.0, 194.0, 38.0, "#FDE68A", "ATLAS STUDIO", "Brand");
    s.text(150.0, 560.0, 36.0, "#94A3B8", "PROJECT PROPOSAL", "Eyebrow");
    s.text(150.0, 640.0, 110.0, "#F8FAFC", "Brand", "Title 1");
    s.text(150.0, 770.0, 110.0, "#F59E0B", "Redesign", "Title 2");
    s.rect(156.0, 940.0, 220.0, 10.0, "#F59E0B", "Rule");
    s.text(
        150.0,
        1000.0,
        36.0,
        "#CBD5E1",
        "Prepared for Northwind Logistics",
        "Prepared for",
    );
    s.text(150.0, 1060.0, 32.0, "#64748B", "June 2025 · v1.0", "Meta");
    // Footer detail blocks.
    let blocks = [
        ("Prepared by", "Atlas Studio"),
        ("Contact", "hello@atlas.co"),
        ("Valid until", "Aug 2025"),
    ];
    for (i, (label, value)) in blocks.iter().enumerate() {
        let x = 150.0 + i as f64 * 320.0;
        s.text(x, 1480.0, 26.0, "#F59E0B", label, "Footer label");
        s.text(x, 1520.0, 32.0, "#E5E7EB", value, "Footer value");
    }
    BundledTemplate {
        dir_name: "proposal-cover.ktemplate",
        manifest: manifest(
            22,
            "Proposal Cover",
            "A4 project-proposal cover with a framed dark panel, amber accents, and prepared-for details.",
            TemplateCategory::Proposal,
            &["proposal", "cover", "business", "client"],
        ),
        content: s.finish(),
    }
}

// ---------------------------------------------------------------------
// Catalog — additional mobile-app screens (1080 × 2160)
// ---------------------------------------------------------------------

fn mobile_checkout() -> BundledTemplate {
    let mut s = Sheet::new(PHONE_W, PHONE_H);
    s.bg("#F8FAFC");
    phone_status_bar(&mut s, "#0F172A");
    s.text(72.0, 150.0, 56.0, "#0F172A", "Checkout", "Title");
    s.text(72.0, 240.0, 32.0, "#64748B", "Step 3 of 3 · Payment", "Step");
    // Order summary card.
    s.rect(72.0, 320.0, 936.0, 520.0, "#FFFFFF", "Summary card");
    s.rect(72.0, 320.0, 936.0, 12.0, "#6366F1", "Summary accent");
    let rows = [
        ("Wireless headphones", "$129.00"),
        ("USB-C cable (2m)", "$18.00"),
        ("Express shipping", "$9.00"),
    ];
    let mut ry = 380.0;
    for (label, price) in rows {
        s.circle(124.0, ry + 34.0, 34.0, "#EEF2FF", "Item thumb");
        s.text(190.0, ry, 36.0, "#0F172A", label, "Item label");
        s.text(800.0, ry, 36.0, "#475569", price, "Item price");
        ry += 120.0;
    }
    s.rect(112.0, ry + 6.0, 856.0, 3.0, "#E2E8F0", "Divider");
    s.text(112.0, ry + 40.0, 44.0, "#0F172A", "Total", "Total label");
    s.text(760.0, ry + 38.0, 48.0, "#6366F1", "$156.00", "Total value");
    // Payment method.
    s.text(72.0, 900.0, 36.0, "#0F172A", "Payment method", "Pay H");
    s.rect(72.0, 960.0, 936.0, 150.0, "#FFFFFF", "Card row");
    s.rect(112.0, 1010.0, 110.0, 70.0, "#1E293B", "Card chip");
    s.text(260.0, 1000.0, 36.0, "#0F172A", "Visa •••• 4242", "Card no");
    s.text(260.0, 1052.0, 28.0, "#64748B", "Expires 09/27", "Card exp");
    s.circle(940.0, 1035.0, 26.0, "#10B981", "Card check");
    // Address.
    s.rect(72.0, 1160.0, 936.0, 150.0, "#FFFFFF", "Address row");
    s.text(112.0, 1200.0, 36.0, "#0F172A", "Jordan Rivera", "Addr name");
    s.text(112.0, 1252.0, 28.0, "#64748B", "128 Market St, SF", "Addr line");
    // Pay button.
    s.rect(72.0, 1900.0, 936.0, 160.0, "#6366F1", "Pay button");
    s.text_center(540.0, 1948.0, 48.0, "#FFFFFF", "Pay $156.00", "Pay label");
    BundledTemplate {
        dir_name: "mobile-checkout.ktemplate",
        manifest: manifest(
            23,
            "Mobile Checkout",
            "Mobile checkout screen with an itemized order summary, saved payment method, address, and a pay CTA.",
            TemplateCategory::MobileApp,
            &["mobile", "checkout", "payment", "ecommerce", "app"],
        ),
        content: s.finish(),
    }
}

fn mobile_signup() -> BundledTemplate {
    let mut s = Sheet::new(PHONE_W, PHONE_H);
    s.bg("#FFFFFF");
    s.rect(0.0, 0.0, PHONE_W, 760.0, "#7C3AED", "Header panel");
    s.ellipse_a(120.0, 120.0, 320.0, 320.0, "#FFFFFF", 0.12, "Glow A");
    s.ellipse_a(980.0, 640.0, 280.0, 280.0, "#4C1D95", 0.30, "Glow B");
    phone_status_bar(&mut s, "#F5F3FF");
    s.text(72.0, 360.0, 76.0, "#FFFFFF", "Create", "Title 1");
    s.text(72.0, 460.0, 76.0, "#DDD6FE", "account", "Title 2");
    s.text(72.0, 600.0, 34.0, "#E9D5FF", "Join 2M+ creators today", "Subtitle");
    let fields = [
        ("Full name", "Jordan Rivera"),
        ("Email address", "jordan@mail.com"),
        ("Password", "••••••••••"),
    ];
    let mut fy = 880.0;
    for (label, value) in fields {
        s.text(72.0, fy, 28.0, "#7C3AED", label, "Field label");
        s.rect(72.0, fy + 44.0, 936.0, 120.0, "#F5F3FF", "Field box");
        s.text(120.0, fy + 78.0, 36.0, "#334155", value, "Field value");
        fy += 220.0;
    }
    s.rect(72.0, 1620.0, 936.0, 150.0, "#7C3AED", "Sign up button");
    s.text_center(540.0, 1664.0, 46.0, "#FFFFFF", "Create account", "Button");
    s.rect(112.0, 1840.0, 856.0, 3.0, "#E2E8F0", "Divider");
    s.text_center(540.0, 1900.0, 32.0, "#64748B", "Already have an account? Log in", "Footer");
    BundledTemplate {
        dir_name: "mobile-signup.ktemplate",
        manifest: manifest(
            24,
            "Mobile Sign-up",
            "Mobile registration screen with a violet header, name/email/password fields, and a create-account CTA.",
            TemplateCategory::MobileApp,
            &["mobile", "signup", "register", "auth", "app"],
        ),
        content: s.finish(),
    }
}

fn mobile_search() -> BundledTemplate {
    let mut s = Sheet::new(PHONE_W, PHONE_H);
    s.bg("#0B1120");
    phone_status_bar(&mut s, "#E2E8F0");
    s.text(72.0, 150.0, 56.0, "#F8FAFC", "Explore", "Title");
    // Search field.
    s.rect(72.0, 250.0, 936.0, 130.0, "#1E293B", "Search field");
    s.circle(140.0, 315.0, 26.0, "#38BDF8", "Search icon");
    s.text(200.0, 290.0, 36.0, "#94A3B8", "Search photos, people…", "Search hint");
    // Filter chips.
    let chips = ["All", "Design", "Nature", "Travel", "Food"];
    let mut cx = 72.0;
    for (i, label) in chips.iter().enumerate() {
        let w = 60.0 + (label.len() as f64) * 22.0;
        let bg = if i == 0 { "#38BDF8" } else { "#1E293B" };
        let ink = if i == 0 { "#0B1120" } else { "#CBD5E1" };
        s.rect(cx, 430.0, w, 80.0, bg, "Chip");
        s.text(cx + 26.0, 452.0, 30.0, ink, label, "Chip label");
        cx += w + 24.0;
    }
    // Result grid (masonry-ish).
    let tiles = [
        ("#F472B6", 560.0, 520.0),
        ("#34D399", 560.0, 360.0),
        ("#FBBF24", 920.0, 520.0),
        ("#818CF8", 920.0, 360.0),
        ("#22D3EE", 1140.0, 520.0),
        ("#FB7185", 1300.0, 360.0),
    ];
    for (hex, y, h) in tiles {
        s.rect(72.0, y, 440.0, h - 200.0, hex, "Result tile L");
        s.rect(568.0, y, 440.0, h - 200.0, hex, "Result tile R");
    }
    phone_tab_bar(&mut s, "#111827", "#38BDF8", "#475569");
    BundledTemplate {
        dir_name: "mobile-search.ktemplate",
        manifest: manifest(
            25,
            "Mobile Search & Explore",
            "Dark explore screen with a search field, filter chips, a two-column result grid, and a tab bar.",
            TemplateCategory::MobileApp,
            &["mobile", "search", "explore", "discover", "app"],
        ),
        content: s.finish(),
    }
}

fn mobile_chat() -> BundledTemplate {
    let mut s = Sheet::new(PHONE_W, PHONE_H);
    s.bg("#EAF2F8");
    s.rect(0.0, 0.0, PHONE_W, 300.0, "#2563EB", "Header");
    phone_status_bar(&mut s, "#DBEAFE");
    s.circle(150.0, 210.0, 56.0, "#93C5FD", "Avatar");
    s.text(240.0, 160.0, 42.0, "#FFFFFF", "Maya Chen", "Name");
    s.text(240.0, 220.0, 28.0, "#BFDBFE", "Online now", "Status");
    // Incoming bubbles (left), outgoing (right).
    let msgs = [
        (false, "Hey! Did you see the new mockups?", 420.0),
        (true, "Yes — they look incredible 🔥", 560.0),
        (false, "The hero section really pops now.", 700.0),
        (true, "Agreed. Shipping the build tonight.", 840.0),
        (false, "Perfect. I'll prep the release notes.", 980.0),
    ];
    for (incoming, body, y) in msgs {
        let w = 120.0 + (body.len() as f64) * 17.0;
        if incoming {
            s.rect(72.0, y, w.min(820.0), 110.0, "#FFFFFF", "Bubble in");
            s.text(112.0, y + 34.0, 30.0, "#1E293B", body, "Msg in");
        } else {
            let x = PHONE_W - 72.0 - w.min(820.0);
            s.rect(x, y, w.min(820.0), 110.0, "#2563EB", "Bubble out");
            s.text(x + 40.0, y + 34.0, 30.0, "#FFFFFF", body, "Msg out");
        }
    }
    // Composer.
    s.rect(0.0, PHONE_H - 200.0, PHONE_W, 200.0, "#FFFFFF", "Composer");
    s.rect(72.0, PHONE_H - 160.0, 760.0, 120.0, "#EAF2F8", "Input");
    s.text(120.0, PHONE_H - 130.0, 34.0, "#94A3B8", "Message…", "Input hint");
    s.circle(930.0, PHONE_H - 100.0, 60.0, "#2563EB", "Send");
    BundledTemplate {
        dir_name: "mobile-chat.ktemplate",
        manifest: manifest(
            26,
            "Mobile Chat",
            "Messaging screen with a blue header, incoming/outgoing chat bubbles, and a composer with send button.",
            TemplateCategory::MobileApp,
            &["mobile", "chat", "messaging", "conversation", "app"],
        ),
        content: s.finish(),
    }
}

fn mobile_notifications() -> BundledTemplate {
    let mut s = Sheet::new(PHONE_W, PHONE_H);
    s.bg("#F8FAFC");
    phone_status_bar(&mut s, "#0F172A");
    s.text(72.0, 150.0, 56.0, "#0F172A", "Notifications", "Title");
    s.text(820.0, 168.0, 30.0, "#6366F1", "Mark all", "Action");
    let items = [
        ("#6366F1", "New comment", "Maya replied to your post.", "2m"),
        ("#10B981", "Payment received", "$129.00 from Acme Co.", "18m"),
        ("#F59E0B", "Reminder", "Design review at 3:00 PM.", "1h"),
        ("#EC4899", "New follower", "Leo started following you.", "3h"),
        ("#06B6D4", "Backup complete", "Project synced to device.", "5h"),
        ("#8B5CF6", "Update ready", "Version 2.4 is available.", "1d"),
    ];
    let mut y = 280.0;
    for (hex, title, body, time) in items {
        s.rect(72.0, y, 936.0, 230.0, "#FFFFFF", "Notif card");
        s.rect(72.0, y, 12.0, 230.0, hex, "Notif accent");
        s.circle(160.0, y + 80.0, 44.0, hex, "Notif icon");
        s.text(240.0, y + 36.0, 38.0, "#0F172A", title, "Notif title");
        s.text(240.0, y + 96.0, 30.0, "#64748B", body, "Notif body");
        s.text(900.0, y + 36.0, 26.0, "#94A3B8", time, "Notif time");
        y += 270.0;
    }
    BundledTemplate {
        dir_name: "mobile-notifications.ktemplate",
        manifest: manifest(
            27,
            "Mobile Notifications",
            "Notifications inbox with color-coded activity cards (comments, payments, reminders, follows) and timestamps.",
            TemplateCategory::MobileApp,
            &["mobile", "notifications", "inbox", "activity", "app"],
        ),
        content: s.finish(),
    }
}

fn mobile_dashboard() -> BundledTemplate {
    let mut s = Sheet::new(PHONE_W, PHONE_H);
    s.bg("#0F172A");
    phone_status_bar(&mut s, "#E2E8F0");
    s.text(72.0, 150.0, 34.0, "#94A3B8", "Good morning,", "Greeting");
    s.text(72.0, 200.0, 56.0, "#F8FAFC", "Jordan", "Name");
    s.circle(960.0, 200.0, 56.0, "#6366F1", "Avatar");
    // Balance hero.
    s.rect(72.0, 320.0, 936.0, 360.0, "#6366F1", "Balance card");
    s.ellipse_a(960.0, 360.0, 240.0, 240.0, "#A5B4FC", 0.25, "Card glow");
    s.text(120.0, 380.0, 32.0, "#E0E7FF", "Total balance", "Bal label");
    s.text(120.0, 440.0, 96.0, "#FFFFFF", "$48,920", "Bal value");
    s.text(120.0, 580.0, 32.0, "#C7D2FE", "+12.4% this month", "Bal delta");
    // Stat tiles.
    let stats = [
        ("#10B981", "Income", "$12.4k"),
        ("#F59E0B", "Spending", "$6.1k"),
        ("#EC4899", "Savings", "$3.0k"),
    ];
    for (i, (hex, label, value)) in stats.iter().enumerate() {
        let x = 72.0 + i as f64 * 320.0;
        s.rect(x, 740.0, 296.0, 280.0, "#1E293B", "Stat tile");
        s.circle(x + 60.0, 800.0, 34.0, hex, "Stat dot");
        s.text(x + 36.0, 870.0, 30.0, "#94A3B8", label, "Stat label");
        s.text(x + 36.0, 920.0, 44.0, "#F8FAFC", value, "Stat value");
    }
    // Bar chart.
    s.text(72.0, 1100.0, 36.0, "#F8FAFC", "Weekly activity", "Chart H");
    let bars = [0.5, 0.7, 0.4, 0.9, 0.6, 0.8, 0.55];
    for (i, frac) in bars.iter().enumerate() {
        let x = 110.0 + i as f64 * 120.0;
        let h = 360.0 * frac;
        s.rect(x, 1560.0 - h, 80.0, h, "#818CF8", "Bar");
    }
    phone_tab_bar(&mut s, "#1E293B", "#818CF8", "#475569");
    BundledTemplate {
        dir_name: "mobile-dashboard.ktemplate",
        manifest: manifest(
            28,
            "Mobile Finance Dashboard",
            "Dark finance home with a balance hero card, income/spending/savings tiles, a weekly bar chart, and a tab bar.",
            TemplateCategory::MobileApp,
            &["mobile", "dashboard", "finance", "wallet", "app"],
        ),
        content: s.finish(),
    }
}

fn mobile_wallet() -> BundledTemplate {
    let mut s = Sheet::new(PHONE_W, PHONE_H);
    s.bg("#F1F5F9");
    phone_status_bar(&mut s, "#0F172A");
    s.text(72.0, 150.0, 56.0, "#0F172A", "My cards", "Title");
    // Stacked cards.
    let cards = [
        ("#1E293B", "#38BDF8", "•••• 4242", 280.0),
        ("#4338CA", "#A5B4FC", "•••• 8830", 420.0),
        ("#0F766E", "#5EEAD4", "•••• 1195", 560.0),
    ];
    for (bg, accent, no, y) in cards {
        s.rect(72.0, y, 936.0, 420.0, bg, "Card");
        s.ellipse_a(960.0, y + 60.0, 200.0, 200.0, accent, 0.30, "Card glow");
        s.rect(120.0, y + 60.0, 110.0, 80.0, accent, "Card chip");
        s.text(120.0, y + 230.0, 48.0, "#FFFFFF", no, "Card number");
        s.text(120.0, y + 320.0, 28.0, "#CBD5E1", "JORDAN RIVERA", "Card holder");
    }
    // Quick actions.
    let actions = [
        ("#6366F1", "Send"),
        ("#10B981", "Request"),
        ("#F59E0B", "Top up"),
        ("#EC4899", "More"),
    ];
    for (i, (hex, label)) in actions.iter().enumerate() {
        let x = 110.0 + i as f64 * 230.0;
        s.circle(x + 70.0, 1180.0, 64.0, hex, "Action");
        s.text_center(x + 70.0, 1270.0, 28.0, "#334155", label, "Action label");
    }
    // Transactions.
    s.text(72.0, 1380.0, 36.0, "#0F172A", "Recent", "Recent H");
    let tx = [("Spotify", "-$9.99"), ("Salary", "+$4,200"), ("Groceries", "-$64.20")];
    let mut ty = 1450.0;
    for (label, amount) in tx {
        s.rect(72.0, ty, 936.0, 140.0, "#FFFFFF", "Tx row");
        s.circle(150.0, ty + 70.0, 40.0, "#EEF2FF", "Tx icon");
        s.text(230.0, ty + 46.0, 34.0, "#0F172A", label, "Tx label");
        s.text(820.0, ty + 46.0, 34.0, "#334155", amount, "Tx amount");
        ty += 160.0;
    }
    BundledTemplate {
        dir_name: "mobile-wallet.ktemplate",
        manifest: manifest(
            29,
            "Mobile Wallet",
            "Wallet screen with stacked payment cards, quick-action buttons, and a recent transactions list.",
            TemplateCategory::MobileApp,
            &["mobile", "wallet", "cards", "payments", "app"],
        ),
        content: s.finish(),
    }
}

fn mobile_paywall() -> BundledTemplate {
    let mut s = Sheet::new(PHONE_W, PHONE_H);
    s.bg("#1E1B4B");
    s.ellipse_a(540.0, 360.0, 520.0, 520.0, "#7C3AED", 0.45, "Glow");
    phone_status_bar(&mut s, "#E0E7FF");
    s.circle(540.0, 360.0, 120.0, "#FBBF24", "Crown");
    s.text_center(540.0, 560.0, 72.0, "#FFFFFF", "Go Premium", "Title");
    s.text_center(540.0, 670.0, 36.0, "#C7D2FE", "Unlock every pro feature", "Subtitle");
    let perks = [
        "Unlimited exports in 4K",
        "Remove watermarks",
        "Premium template library",
        "Priority cloud backup",
    ];
    let mut py = 800.0;
    for perk in perks {
        s.circle(140.0, py + 22.0, 26.0, "#34D399", "Check");
        s.text(200.0, py, 36.0, "#E0E7FF", perk, "Perk");
        py += 110.0;
    }
    // Plan toggle.
    s.rect(72.0, 1320.0, 456.0, 220.0, "#312E81", "Plan monthly");
    s.text_center(300.0, 1360.0, 30.0, "#A5B4FC", "Monthly", "Plan label");
    s.text_center(300.0, 1420.0, 52.0, "#FFFFFF", "$9.99", "Plan price");
    s.rect(552.0, 1320.0, 456.0, 220.0, "#7C3AED", "Plan yearly");
    s.rect(552.0, 1320.0, 456.0, 12.0, "#FBBF24", "Best value");
    s.text_center(780.0, 1360.0, 30.0, "#F5F3FF", "Yearly", "Plan label");
    s.text_center(780.0, 1420.0, 52.0, "#FFFFFF", "$59.99", "Plan price");
    // CTA.
    s.rect(72.0, 1720.0, 936.0, 160.0, "#FBBF24", "CTA");
    s.text_center(540.0, 1768.0, 46.0, "#1E1B4B", "Start free trial", "CTA label");
    s.text_center(540.0, 1940.0, 28.0, "#A5B4FC", "7 days free, cancel anytime", "Fine print");
    BundledTemplate {
        dir_name: "mobile-paywall.ktemplate",
        manifest: manifest(
            30,
            "Mobile Paywall",
            "Premium upgrade screen with a glowing crown, perk checklist, monthly/yearly plan toggle, and a trial CTA.",
            TemplateCategory::MobileApp,
            &["mobile", "paywall", "subscription", "premium", "app"],
        ),
        content: s.finish(),
    }
}

fn mobile_product() -> BundledTemplate {
    let mut s = Sheet::new(PHONE_W, PHONE_H);
    s.bg("#FFFFFF");
    // Product image area.
    s.rect(0.0, 0.0, PHONE_W, 1080.0, "#FEF3C7", "Image bg");
    s.ellipse_a(540.0, 560.0, 360.0, 360.0, "#FBBF24", 0.35, "Halo");
    s.circle(540.0, 540.0, 280.0, "#F59E0B", "Product");
    s.circle(620.0, 460.0, 80.0, "#FCD34D", "Product hi");
    phone_status_bar(&mut s, "#92400E");
    s.circle(150.0, 210.0, 50.0, "#FFFFFF", "Back");
    s.circle(930.0, 210.0, 50.0, "#FFFFFF", "Fav");
    // Detail sheet.
    s.rect(0.0, 1080.0, PHONE_W, 1080.0, "#FFFFFF", "Sheet");
    s.rect(480.0, 1120.0, 120.0, 14.0, "#E2E8F0", "Grabber");
    s.text(72.0, 1180.0, 56.0, "#0F172A", "Aria Lounge Chair", "Name");
    s.text(72.0, 1270.0, 40.0, "#F59E0B", "$249.00", "Price");
    s.text(760.0, 1270.0, 34.0, "#64748B", "★ 4.9 (320)", "Rating");
    // Color swatches.
    let swatches = ["#0F172A", "#F59E0B", "#10B981", "#EF4444"];
    for (i, hex) in swatches.iter().enumerate() {
        s.circle(120.0 + i as f64 * 110.0, 1400.0, 42.0, hex, "Swatch");
    }
    s.text(72.0, 1500.0, 32.0, "#475569", "Sculpted oak frame with a soft", "Desc 1");
    s.text(72.0, 1550.0, 32.0, "#475569", "boucle seat — built to last.", "Desc 2");
    // Add to cart.
    s.rect(72.0, 1860.0, 600.0, 160.0, "#0F172A", "Add to cart");
    s.text_center(372.0, 1908.0, 42.0, "#FFFFFF", "Add to cart", "Cart label");
    s.rect(712.0, 1860.0, 296.0, 160.0, "#F59E0B", "Buy now");
    s.text_center(860.0, 1908.0, 42.0, "#FFFFFF", "Buy", "Buy label");
    BundledTemplate {
        dir_name: "mobile-product.ktemplate",
        manifest: manifest(
            31,
            "Mobile Product Detail",
            "Product page with a hero image, price, rating, color swatches, description, and add-to-cart / buy actions.",
            TemplateCategory::MobileApp,
            &["mobile", "product", "ecommerce", "detail", "app"],
        ),
        content: s.finish(),
    }
}

fn mobile_cart() -> BundledTemplate {
    let mut s = Sheet::new(PHONE_W, PHONE_H);
    s.bg("#F8FAFC");
    phone_status_bar(&mut s, "#0F172A");
    s.text(72.0, 150.0, 56.0, "#0F172A", "Your cart", "Title");
    s.text(72.0, 240.0, 32.0, "#64748B", "3 items", "Count");
    let items = [
        ("#FDE68A", "Lounge Chair", "$249.00", "Qty 1"),
        ("#A7F3D0", "Floor Lamp", "$89.00", "Qty 2"),
        ("#BFDBFE", "Side Table", "$129.00", "Qty 1"),
    ];
    let mut y = 320.0;
    for (img, name, price, qty) in items {
        s.rect(72.0, y, 936.0, 280.0, "#FFFFFF", "Cart row");
        s.rect(112.0, y + 40.0, 200.0, 200.0, img, "Item image");
        s.text(350.0, y + 50.0, 38.0, "#0F172A", name, "Item name");
        s.text(350.0, y + 110.0, 36.0, "#6366F1", price, "Item price");
        s.rect(350.0, y + 180.0, 200.0, 70.0, "#EEF2FF", "Qty pill");
        s.text(390.0, y + 196.0, 32.0, "#334155", qty, "Qty");
        y += 310.0;
    }
    // Promo.
    s.rect(72.0, 1280.0, 936.0, 130.0, "#ECFDF5", "Promo");
    s.text(120.0, 1318.0, 34.0, "#047857", "Promo: SPRING20 applied", "Promo text");
    // Totals.
    let totals = [("Subtotal", "$556.00"), ("Shipping", "$12.00"), ("Discount", "-$111.20")];
    let mut ty = 1470.0;
    for (label, value) in totals {
        s.text(72.0, ty, 34.0, "#64748B", label, "Total label");
        s.text(820.0, ty, 34.0, "#334155", value, "Total value");
        ty += 70.0;
    }
    s.rect(72.0, ty + 10.0, 936.0, 3.0, "#E2E8F0", "Divider");
    s.text(72.0, ty + 50.0, 44.0, "#0F172A", "Total", "Grand label");
    s.text(760.0, ty + 48.0, 48.0, "#6366F1", "$456.80", "Grand value");
    s.rect(72.0, 1900.0, 936.0, 160.0, "#0F172A", "Checkout button");
    s.text_center(540.0, 1948.0, 46.0, "#FFFFFF", "Checkout", "Checkout label");
    BundledTemplate {
        dir_name: "mobile-cart.ktemplate",
        manifest: manifest(
            32,
            "Mobile Shopping Cart",
            "Shopping cart with item rows, quantity pills, an applied promo, an itemized total, and a checkout CTA.",
            TemplateCategory::MobileApp,
            &["mobile", "cart", "ecommerce", "shopping", "app"],
        ),
        content: s.finish(),
    }
}

fn mobile_delivery() -> BundledTemplate {
    let mut s = Sheet::new(PHONE_W, PHONE_H);
    s.bg("#E8F5E9");
    // Map area.
    s.rect(0.0, 0.0, PHONE_W, 1120.0, "#C8E6C9", "Map");
    for i in 0..5 {
        s.rect(0.0, 180.0 + f64::from(i) * 220.0, PHONE_W, 6.0, "#A5D6A7", "Map road");
        s.rect(120.0 + f64::from(i) * 200.0, 0.0, 6.0, 1120.0, "#A5D6A7", "Map road v");
    }
    // Route + pins.
    s.circle(280.0, 880.0, 36.0, "#1B5E20", "Pin start");
    s.circle(820.0, 320.0, 44.0, "#2563EB", "Pin dest");
    s.rect(300.0, 600.0, 520.0, 10.0, "#2563EB", "Route");
    phone_status_bar(&mut s, "#1B5E20");
    // Status sheet.
    s.rect(0.0, 1120.0, PHONE_W, 1040.0, "#FFFFFF", "Sheet");
    s.rect(480.0, 1160.0, 120.0, 14.0, "#E2E8F0", "Grabber");
    s.text(72.0, 1220.0, 36.0, "#16A34A", "Arriving in 12 min", "ETA");
    s.text(72.0, 1290.0, 54.0, "#0F172A", "Order #A1029", "Order");
    // Progress steps.
    let steps = ["Confirmed", "Preparing", "On the way", "Delivered"];
    for (i, label) in steps.iter().enumerate() {
        let x = 150.0 + i as f64 * 260.0;
        let done = i <= 2;
        let hex = if done { "#16A34A" } else { "#CBD5E1" };
        s.circle(x, 1440.0, 32.0, hex, "Step dot");
        if i < 3 {
            s.rect(x + 32.0, 1432.0, 196.0, 12.0, hex, "Step line");
        }
        s.text_center(x, 1500.0, 24.0, "#475569", label, "Step label");
    }
    // Courier card.
    s.rect(72.0, 1620.0, 936.0, 220.0, "#F1F5F9", "Courier card");
    s.circle(190.0, 1730.0, 80.0, "#A5D6A7", "Courier avatar");
    s.text(320.0, 1670.0, 40.0, "#0F172A", "Marcus D.", "Courier name");
    s.text(320.0, 1730.0, 30.0, "#64748B", "★ 4.95 · Toyota Prius", "Courier meta");
    s.circle(870.0, 1730.0, 56.0, "#16A34A", "Call");
    s.rect(72.0, 1900.0, 936.0, 150.0, "#16A34A", "Track button");
    s.text_center(540.0, 1944.0, 44.0, "#FFFFFF", "Track order", "Track label");
    BundledTemplate {
        dir_name: "mobile-delivery.ktemplate",
        manifest: manifest(
            33,
            "Mobile Delivery Tracking",
            "Live order tracking with a map + route, a progress stepper, courier card, and a track-order CTA.",
            TemplateCategory::MobileApp,
            &["mobile", "delivery", "tracking", "map", "app"],
        ),
        content: s.finish(),
    }
}

fn mobile_fitness() -> BundledTemplate {
    let mut s = Sheet::new(PHONE_W, PHONE_H);
    s.bg("#0B1120");
    phone_status_bar(&mut s, "#E2E8F0");
    s.text(72.0, 150.0, 34.0, "#64748B", "Today", "Date");
    s.text(72.0, 200.0, 56.0, "#F8FAFC", "Activity", "Title");
    // Big progress ring.
    s.circle(540.0, 640.0, 280.0, "#1E293B", "Ring track");
    s.circle(540.0, 640.0, 280.0, "#22D3EE", "Ring fill");
    s.circle(540.0, 640.0, 210.0, "#0B1120", "Ring inner");
    s.text_center(540.0, 560.0, 96.0, "#F8FAFC", "78%", "Ring pct");
    s.text_center(540.0, 690.0, 32.0, "#94A3B8", "of daily goal", "Ring label");
    // Metric tiles.
    let metrics = [
        ("#34D399", "Steps", "8,420"),
        ("#FBBF24", "Calories", "612 kcal"),
        ("#F472B6", "Distance", "6.1 km"),
    ];
    for (i, (hex, label, value)) in metrics.iter().enumerate() {
        let y = 1020.0 + i as f64 * 200.0;
        s.rect(72.0, y, 936.0, 170.0, "#1E293B", "Metric tile");
        s.circle(160.0, y + 85.0, 44.0, hex, "Metric dot");
        s.text(250.0, y + 38.0, 32.0, "#94A3B8", label, "Metric label");
        s.text(250.0, y + 88.0, 44.0, "#F8FAFC", value, "Metric value");
    }
    // Start workout.
    s.rect(72.0, 1680.0, 936.0, 170.0, "#22D3EE", "Start button");
    s.text_center(540.0, 1730.0, 46.0, "#0B1120", "Start workout", "Start label");
    phone_tab_bar(&mut s, "#111827", "#22D3EE", "#475569");
    BundledTemplate {
        dir_name: "mobile-fitness.ktemplate",
        manifest: manifest(
            34,
            "Mobile Fitness Tracker",
            "Activity screen with a large progress ring, steps/calories/distance tiles, and a start-workout CTA.",
            TemplateCategory::MobileApp,
            &["mobile", "fitness", "health", "activity", "app"],
        ),
        content: s.finish(),
    }
}

fn mobile_weather() -> BundledTemplate {
    let mut s = Sheet::new(PHONE_W, PHONE_H);
    s.bg("#1D4ED8");
    s.rect(0.0, 1080.0, PHONE_W, 1080.0, "#1E40AF", "Lower");
    s.ellipse_a(820.0, 360.0, 360.0, 360.0, "#93C5FD", 0.30, "Sun glow");
    phone_status_bar(&mut s, "#DBEAFE");
    s.text_center(540.0, 240.0, 40.0, "#DBEAFE", "San Francisco", "City");
    // Big sun + temp.
    s.circle(540.0, 560.0, 150.0, "#FBBF24", "Sun");
    s.text_center(540.0, 760.0, 160.0, "#FFFFFF", "21°", "Temp");
    s.text_center(540.0, 940.0, 38.0, "#BFDBFE", "Mostly sunny · H 24° L 16°", "Cond");
    // Hourly strip.
    let hours = [("09", "20°"), ("12", "23°"), ("15", "24°"), ("18", "21°"), ("21", "18°")];
    for (i, (hr, t)) in hours.iter().enumerate() {
        let x = 72.0 + i as f64 * 190.0;
        s.rect(x, 1120.0, 168.0, 320.0, "#2563EB", "Hour tile");
        s.text_center(x + 84.0, 1160.0, 32.0, "#BFDBFE", hr, "Hour");
        s.circle(x + 84.0, 1280.0, 40.0, "#FBBF24", "Hour icon");
        s.text_center(x + 84.0, 1360.0, 34.0, "#FFFFFF", t, "Hour temp");
    }
    // Daily forecast.
    let days = [("Mon", "24°", "16°"), ("Tue", "22°", "15°"), ("Wed", "19°", "14°"), ("Thu", "25°", "17°")];
    let mut y = 1520.0;
    for (day, hi, lo) in days {
        s.text(100.0, y, 36.0, "#FFFFFF", day, "Day");
        s.rect(360.0, y + 16.0, 360.0, 12.0, "#3B82F6", "Range track");
        s.rect(440.0, y + 16.0, 200.0, 12.0, "#FBBF24", "Range fill");
        s.text(760.0, y, 34.0, "#DBEAFE", lo, "Lo");
        s.text(900.0, y, 34.0, "#FFFFFF", hi, "Hi");
        y += 130.0;
    }
    BundledTemplate {
        dir_name: "mobile-weather.ktemplate",
        manifest: manifest(
            35,
            "Mobile Weather",
            "Weather screen with a big current temperature, an hourly strip, and a 4-day forecast with range bars.",
            TemplateCategory::MobileApp,
            &["mobile", "weather", "forecast", "app"],
        ),
        content: s.finish(),
    }
}

fn mobile_splash() -> BundledTemplate {
    let mut s = Sheet::new(PHONE_W, PHONE_H);
    s.bg("#0F172A");
    s.ellipse_a(540.0, 760.0, 560.0, 560.0, "#6366F1", 0.30, "Glow 1");
    s.ellipse_a(540.0, 760.0, 360.0, 360.0, "#22D3EE", 0.20, "Glow 2");
    phone_status_bar(&mut s, "#E2E8F0");
    // Logomark.
    s.circle(540.0, 760.0, 180.0, "#6366F1", "Logo disc");
    s.rect(470.0, 690.0, 140.0, 140.0, "#0F172A", "Logo cut");
    s.circle(540.0, 760.0, 56.0, "#22D3EE", "Logo dot");
    s.text_center(540.0, 1040.0, 84.0, "#F8FAFC", "Nimbus", "Wordmark");
    s.text_center(540.0, 1160.0, 36.0, "#94A3B8", "Design, anywhere", "Tagline");
    // Loading bar.
    s.rect(320.0, 1860.0, 440.0, 16.0, "#1E293B", "Loader track");
    s.rect(320.0, 1860.0, 280.0, 16.0, "#6366F1", "Loader fill");
    s.text_center(540.0, 1920.0, 26.0, "#64748B", "v2.4.0", "Version");
    BundledTemplate {
        dir_name: "mobile-splash.ktemplate",
        manifest: manifest(
            36,
            "Mobile Splash",
            "App splash / launch screen with a centered glowing logomark, wordmark, tagline, and loading bar.",
            TemplateCategory::MobileApp,
            &["mobile", "splash", "launch", "branding", "app"],
        ),
        content: s.finish(),
    }
}

// ---------------------------------------------------------------------
// Catalog — additional presentation slides (1920 × 1080)
// ---------------------------------------------------------------------

fn deck_section() -> BundledTemplate {
    let mut s = Sheet::new(SLIDE_W, SLIDE_H);
    s.bg("#0F172A");
    s.rect(0.0, 0.0, 24.0, SLIDE_H, "#F59E0B", "Edge rule");
    s.ellipse_a(1640.0, 220.0, 420.0, 420.0, "#6366F1", 0.30, "Glow");
    s.text(160.0, 360.0, 44.0, "#F59E0B", "SECTION 02", "Eyebrow");
    s.text(160.0, 440.0, 150.0, "#F8FAFC", "Go-to-market", "Title 1");
    s.text(160.0, 600.0, 150.0, "#94A3B8", "strategy", "Title 2");
    s.rect(160.0, 800.0, 360.0, 8.0, "#F59E0B", "Underline");
    s.text(160.0, 860.0, 36.0, "#CBD5E1", "How we reach the first 10,000 customers", "Caption");
    BundledTemplate {
        dir_name: "deck-section.ktemplate",
        manifest: manifest(
            37,
            "Slide — Section Divider",
            "16:9 section divider slide with an oversized title, eyebrow label, and accent rule.",
            TemplateCategory::Presentation,
            &["slide", "deck", "section", "divider", "presentation"],
        ),
        content: s.finish(),
    }
}

fn deck_quote() -> BundledTemplate {
    let mut s = Sheet::new(SLIDE_W, SLIDE_H);
    s.bg("#F8FAFC");
    s.rect(0.0, 0.0, SLIDE_W, 16.0, "#6366F1", "Top rule");
    s.text(150.0, 120.0, 220.0, "#C7D2FE", "\u{201C}", "Quote mark");
    s.text(300.0, 360.0, 72.0, "#0F172A", "Design is not just what it", "Quote 1");
    s.text(300.0, 470.0, 72.0, "#0F172A", "looks like and feels like.", "Quote 2");
    s.text(300.0, 580.0, 72.0, "#6366F1", "Design is how it works.", "Quote 3");
    s.rect(300.0, 740.0, 120.0, 8.0, "#6366F1", "Rule");
    s.circle(330.0, 860.0, 50.0, "#A5B4FC", "Author avatar");
    s.text(410.0, 820.0, 40.0, "#0F172A", "Steve Jobs", "Author");
    s.text(410.0, 880.0, 30.0, "#64748B", "Co-founder, Apple", "Author role");
    BundledTemplate {
        dir_name: "deck-quote.ktemplate",
        manifest: manifest(
            38,
            "Slide — Quote",
            "16:9 quote slide with a large pull-quote, accent color, and an attributed author.",
            TemplateCategory::Presentation,
            &["slide", "deck", "quote", "testimonial", "presentation"],
        ),
        content: s.finish(),
    }
}

fn deck_chart_bar() -> BundledTemplate {
    let mut s = Sheet::new(SLIDE_W, SLIDE_H);
    s.bg("#FFFFFF");
    s.text(150.0, 110.0, 64.0, "#0F172A", "Revenue by quarter", "Title");
    s.text(150.0, 200.0, 32.0, "#64748B", "FY2025 · in millions USD", "Subtitle");
    // Axis.
    s.rect(150.0, 880.0, 1620.0, 6.0, "#E2E8F0", "X axis");
    let bars = [
        ("Q1", 0.45, "$42M"),
        ("Q2", 0.62, "$58M"),
        ("Q3", 0.78, "$73M"),
        ("Q4", 0.95, "$89M"),
    ];
    for (i, (label, frac, value)) in bars.iter().enumerate() {
        let x = 320.0 + i as f64 * 380.0;
        let h = 560.0 * frac;
        s.rect(x, 880.0 - h, 220.0, h, "#6366F1", "Bar");
        s.rect(x, 880.0 - h, 220.0, 16.0, "#A5B4FC", "Bar cap");
        s.text_center(x + 110.0, 880.0 - h - 60.0, 36.0, "#0F172A", value, "Bar value");
        s.text_center(x + 110.0, 920.0, 34.0, "#64748B", label, "Bar label");
    }
    s.text(150.0, 1000.0, 30.0, "#10B981", "▲ 112% YoY growth", "Note");
    BundledTemplate {
        dir_name: "deck-chart-bar.ktemplate",
        manifest: manifest(
            39,
            "Slide — Bar Chart",
            "16:9 data slide with a four-quarter bar chart, value labels, axis, and a growth note.",
            TemplateCategory::Presentation,
            &["slide", "deck", "chart", "data", "presentation"],
        ),
        content: s.finish(),
    }
}

fn deck_comparison() -> BundledTemplate {
    let mut s = Sheet::new(SLIDE_W, SLIDE_H);
    s.bg("#0F172A");
    s.text_center(960.0, 110.0, 64.0, "#F8FAFC", "Us vs. the old way", "Title");
    // Two panels.
    s.rect(150.0, 260.0, 760.0, 720.0, "#1E293B", "Panel old");
    s.text(210.0, 320.0, 44.0, "#94A3B8", "The old way", "Old H");
    let old = ["Manual exports", "No version history", "Locked file formats", "Slow approvals"];
    let mut oy = 440.0;
    for line in old {
        s.circle(240.0, oy + 18.0, 18.0, "#EF4444", "X dot");
        s.text(290.0, oy, 34.0, "#CBD5E1", line, "Old line");
        oy += 120.0;
    }
    s.rect(1010.0, 260.0, 760.0, 720.0, "#4338CA", "Panel new");
    s.text(1070.0, 320.0, 44.0, "#E0E7FF", "With KCreate", "New H");
    let new = ["One-click batch export", "Full undo timeline", "Open .kstudio format", "Instant remix"];
    let mut ny = 440.0;
    for line in new {
        s.circle(1100.0, ny + 18.0, 18.0, "#34D399", "Check dot");
        s.text(1150.0, ny, 34.0, "#FFFFFF", line, "New line");
        ny += 120.0;
    }
    BundledTemplate {
        dir_name: "deck-comparison.ktemplate",
        manifest: manifest(
            40,
            "Slide — Comparison",
            "16:9 two-column comparison slide contrasting an old approach with the new one using check/cross markers.",
            TemplateCategory::Presentation,
            &["slide", "deck", "comparison", "versus", "presentation"],
        ),
        content: s.finish(),
    }
}

fn deck_timeline() -> BundledTemplate {
    let mut s = Sheet::new(SLIDE_W, SLIDE_H);
    s.bg("#FFFFFF");
    s.text(150.0, 110.0, 64.0, "#0F172A", "Product roadmap", "Title");
    s.rect(220.0, 560.0, 1480.0, 8.0, "#E2E8F0", "Spine");
    let nodes = [
        ("#6366F1", "Q1", "Beta launch"),
        ("#06B6D4", "Q2", "Mobile apps"),
        ("#10B981", "Q3", "Team spaces"),
        ("#F59E0B", "Q4", "AI assistant"),
    ];
    for (i, (hex, q, label)) in nodes.iter().enumerate() {
        let x = 320.0 + i as f64 * 380.0;
        s.circle(x, 564.0, 40.0, hex, "Node");
        s.rect(x - 3.0, if i % 2 == 0 { 360.0 } else { 600.0 }, 6.0, 160.0, hex, "Stem");
        let cy = if i % 2 == 0 { 300.0 } else { 760.0 };
        s.text_center(x, cy, 40.0, hex, q, "Quarter");
        s.text_center(x, cy + 56.0, 32.0, "#475569", label, "Milestone");
    }
    BundledTemplate {
        dir_name: "deck-timeline.ktemplate",
        manifest: manifest(
            41,
            "Slide — Timeline",
            "16:9 roadmap slide with a horizontal spine and alternating quarterly milestone nodes.",
            TemplateCategory::Presentation,
            &["slide", "deck", "timeline", "roadmap", "presentation"],
        ),
        content: s.finish(),
    }
}

fn deck_closing() -> BundledTemplate {
    let mut s = Sheet::new(SLIDE_W, SLIDE_H);
    s.bg("#4338CA");
    s.ellipse_a(300.0, 900.0, 460.0, 460.0, "#312E81", 0.55, "Glow A");
    s.ellipse_a(1700.0, 180.0, 420.0, 420.0, "#6366F1", 0.45, "Glow B");
    s.text_center(960.0, 380.0, 150.0, "#FFFFFF", "Thank you", "Title");
    s.rect(810.0, 580.0, 300.0, 8.0, "#A5B4FC", "Rule");
    s.text_center(960.0, 640.0, 40.0, "#C7D2FE", "Let's build something great together", "Subtitle");
    let contacts = ["hello@kcreate.app", "kcreate.app", "@kcreate"];
    for (i, c) in contacts.iter().enumerate() {
        let x = 520.0 + i as f64 * 320.0;
        s.circle(x, 860.0, 28.0, "#A5B4FC", "Contact dot");
        s.text(x + 50.0, 838.0, 30.0, "#E0E7FF", c, "Contact");
    }
    BundledTemplate {
        dir_name: "deck-closing.ktemplate",
        manifest: manifest(
            42,
            "Slide — Closing",
            "16:9 thank-you closing slide with a centered title, accent rule, and contact row.",
            TemplateCategory::Presentation,
            &["slide", "deck", "closing", "thank you", "presentation"],
        ),
        content: s.finish(),
    }
}

fn deck_bignum() -> BundledTemplate {
    let mut s = Sheet::new(SLIDE_W, SLIDE_H);
    s.bg("#FFFFFF");
    s.rect(0.0, 0.0, SLIDE_W, 16.0, "#10B981", "Top rule");
    s.text(150.0, 200.0, 40.0, "#10B981", "THE OPPORTUNITY", "Eyebrow");
    s.text_center(960.0, 360.0, 320.0, "#0F172A", "92%", "Big number");
    s.text_center(960.0, 760.0, 48.0, "#475569", "of teams say their current tools", "Body 1");
    s.text_center(960.0, 830.0, 48.0, "#475569", "slow down creative work", "Body 2");
    s.text_center(960.0, 960.0, 28.0, "#94A3B8", "Source: 2025 Creative Workflow Survey, n=4,200", "Source");
    BundledTemplate {
        dir_name: "deck-bignum.ktemplate",
        manifest: manifest(
            43,
            "Slide — Big Number",
            "16:9 stat slide built around one oversized headline number with supporting copy and a source line.",
            TemplateCategory::Presentation,
            &["slide", "deck", "stat", "number", "presentation"],
        ),
        content: s.finish(),
    }
}

fn deck_process() -> BundledTemplate {
    let mut s = Sheet::new(SLIDE_W, SLIDE_H);
    s.bg("#0B1120");
    s.text(150.0, 110.0, 64.0, "#F8FAFC", "How it works", "Title");
    let steps = [
        ("#6366F1", "1", "Import", "Drop in any design or .kstudio file"),
        ("#06B6D4", "2", "Remix", "Edit layers, colors, and copy freely"),
        ("#10B981", "3", "Export", "Ship PNG, SVG, or print-ready PDF"),
    ];
    for (i, (hex, num, title, body)) in steps.iter().enumerate() {
        let x = 150.0 + i as f64 * 560.0;
        s.rect(x, 320.0, 500.0, 520.0, "#1E293B", "Step card");
        s.circle(x + 100.0, 440.0, 70.0, hex, "Step badge");
        s.text_center(x + 100.0, 408.0, 64.0, "#FFFFFF", num, "Step num");
        s.text(x + 50.0, 560.0, 44.0, "#F8FAFC", title, "Step title");
        s.text(x + 50.0, 640.0, 30.0, "#94A3B8", body, "Step body");
        if i < 2 {
            s.text(x + 510.0, 540.0, 60.0, hex, "→", "Arrow");
        }
    }
    BundledTemplate {
        dir_name: "deck-process.ktemplate",
        manifest: manifest(
            44,
            "Slide — 3-Step Process",
            "16:9 process slide with three numbered step cards connected by arrows.",
            TemplateCategory::Presentation,
            &["slide", "deck", "process", "steps", "presentation"],
        ),
        content: s.finish(),
    }
}

fn deck_pricing() -> BundledTemplate {
    let mut s = Sheet::new(SLIDE_W, SLIDE_H);
    s.bg("#F8FAFC");
    s.text_center(960.0, 100.0, 64.0, "#0F172A", "Simple pricing", "Title");
    let plans = [
        ("#FFFFFF", "#0F172A", "Starter", "$0", "For individuals"),
        ("#4338CA", "#FFFFFF", "Pro", "$12", "For professionals"),
        ("#FFFFFF", "#0F172A", "Team", "$29", "For growing teams"),
    ];
    for (i, (bg, ink, name, price, who)) in plans.iter().enumerate() {
        let x = 230.0 + i as f64 * 500.0;
        let y = if i == 1 { 270.0 } else { 310.0 };
        let h = if i == 1 { 660.0 } else { 600.0 };
        s.rect(x, y, 440.0, h, bg, "Plan card");
        if i == 1 {
            s.rect(x, y, 440.0, 14.0, "#FBBF24", "Popular");
        }
        s.text(x + 50.0, y + 60.0, 40.0, ink, name, "Plan name");
        s.text(x + 50.0, y + 150.0, 96.0, ink, price, "Plan price");
        s.text(x + 50.0, y + 280.0, 30.0, if i == 1 { "#C7D2FE" } else { "#64748B" }, who, "Plan who");
        let feats = ["Unlimited projects", "Cloud backup", "Export everything"];
        let mut fy = y + 360.0;
        for f in feats {
            s.circle(x + 70.0, fy + 14.0, 12.0, if i == 1 { "#34D399" } else { "#6366F1" }, "Feat dot");
            s.text(x + 100.0, fy, 26.0, if i == 1 { "#E0E7FF" } else { "#475569" }, f, "Feat");
            fy += 60.0;
        }
    }
    BundledTemplate {
        dir_name: "deck-pricing.ktemplate",
        manifest: manifest(
            45,
            "Slide — Pricing",
            "16:9 pricing slide with three plan cards, a highlighted popular tier, and feature lists.",
            TemplateCategory::Presentation,
            &["slide", "deck", "pricing", "plans", "presentation"],
        ),
        content: s.finish(),
    }
}

fn deck_testimonial() -> BundledTemplate {
    let mut s = Sheet::new(SLIDE_W, SLIDE_H);
    s.bg("#111827");
    s.rect(0.0, 0.0, 760.0, SLIDE_H, "#4338CA", "Left panel");
    s.ellipse_a(380.0, 540.0, 280.0, 280.0, "#6366F1", 0.45, "Avatar glow");
    s.circle(380.0, 480.0, 180.0, "#A5B4FC", "Avatar");
    s.text_center(380.0, 720.0, 44.0, "#FFFFFF", "Mara Lopez", "Name");
    s.text_center(380.0, 780.0, 30.0, "#C7D2FE", "Head of Design, Northwind", "Role");
    s.text(880.0, 280.0, 120.0, "#312E81", "\u{201C}", "Quote mark");
    s.text(880.0, 420.0, 54.0, "#F8FAFC", "KCreate replaced four tools", "Quote 1");
    s.text(880.0, 500.0, 54.0, "#F8FAFC", "in our stack and cut our", "Quote 2");
    s.text(880.0, 580.0, 54.0, "#F8FAFC", "production time in half.", "Quote 3");
    for i in 0..5 {
        s.circle(900.0 + f64::from(i) * 70.0, 740.0, 24.0, "#FBBF24", "Star");
    }
    BundledTemplate {
        dir_name: "deck-testimonial.ktemplate",
        manifest: manifest(
            46,
            "Slide — Testimonial",
            "16:9 testimonial slide with a portrait panel, large customer quote, and a 5-star rating.",
            TemplateCategory::Presentation,
            &["slide", "deck", "testimonial", "customer", "presentation"],
        ),
        content: s.finish(),
    }
}

fn deck_image_text() -> BundledTemplate {
    let mut s = Sheet::new(SLIDE_W, SLIDE_H);
    s.bg("#FFFFFF");
    // Image half.
    s.rect(0.0, 0.0, 960.0, SLIDE_H, "#0EA5E9", "Image panel");
    s.ellipse_a(480.0, 360.0, 320.0, 320.0, "#FFFFFF", 0.18, "Glow");
    s.circle(420.0, 520.0, 200.0, "#38BDF8", "Shape A");
    s.rect(540.0, 560.0, 280.0, 280.0, "#0284C7", "Shape B");
    s.circle(700.0, 380.0, 120.0, "#BAE6FD", "Shape C");
    // Text half.
    s.text(1060.0, 320.0, 40.0, "#0EA5E9", "FEATURE", "Eyebrow");
    s.text(1060.0, 400.0, 80.0, "#0F172A", "Real-time", "Title 1");
    s.text(1060.0, 500.0, 80.0, "#0F172A", "collaboration", "Title 2");
    s.text(1060.0, 640.0, 34.0, "#475569", "See teammates' cursors, comments,", "Body 1");
    s.text(1060.0, 690.0, 34.0, "#475569", "and edits the moment they happen.", "Body 2");
    s.rect(1060.0, 800.0, 360.0, 110.0, "#0EA5E9", "CTA");
    s.text_center(1240.0, 832.0, 36.0, "#FFFFFF", "Learn more", "CTA label");
    BundledTemplate {
        dir_name: "deck-image-text.ktemplate",
        manifest: manifest(
            47,
            "Slide — Image + Text",
            "16:9 feature slide with a split layout: a graphic panel on the left and headline/body/CTA on the right.",
            TemplateCategory::Presentation,
            &["slide", "deck", "feature", "split", "presentation"],
        ),
        content: s.finish(),
    }
}

fn deck_table() -> BundledTemplate {
    let mut s = Sheet::new(SLIDE_W, SLIDE_H);
    s.bg("#F8FAFC");
    s.text(150.0, 110.0, 64.0, "#0F172A", "Feature matrix", "Title");
    // Header row.
    s.rect(150.0, 240.0, 1620.0, 100.0, "#0F172A", "Header row");
    let cols = ["Capability", "Free", "Pro", "Team"];
    for (i, c) in cols.iter().enumerate() {
        let x = 190.0 + i as f64 * 405.0;
        s.text(x, 268.0, 36.0, "#F8FAFC", c, "Col head");
    }
    let rows = [
        ("Projects", "3", "∞", "∞"),
        ("Cloud backup", "—", "✓", "✓"),
        ("Team spaces", "—", "—", "✓"),
        ("Priority support", "—", "✓", "✓"),
        ("Audit log", "—", "—", "✓"),
    ];
    for (r, (cap, a, b, c)) in rows.iter().enumerate() {
        let y = 340.0 + r as f64 * 130.0;
        let bg = if r % 2 == 0 { "#FFFFFF" } else { "#EEF2F8" };
        s.rect(150.0, y, 1620.0, 130.0, bg, "Row");
        s.text(190.0, y + 40.0, 34.0, "#0F172A", cap, "Cap");
        let cells = [a, b, c];
        for (i, cell) in cells.iter().enumerate() {
            let x = 595.0 + i as f64 * 405.0;
            let value: &str = cell;
            let ink = if value == "✓" { "#10B981" } else { "#475569" };
            s.text(x, y + 40.0, 34.0, ink, value, "Cell");
        }
    }
    BundledTemplate {
        dir_name: "deck-table.ktemplate",
        manifest: manifest(
            48,
            "Slide — Comparison Table",
            "16:9 feature-matrix slide with a dark header row and zebra-striped capability rows.",
            TemplateCategory::Presentation,
            &["slide", "deck", "table", "matrix", "presentation"],
        ),
        content: s.finish(),
    }
}

// ---------------------------------------------------------------------
// Catalog — pitch-deck slides (1920 × 1080)
// ---------------------------------------------------------------------

fn pitch_cover() -> BundledTemplate {
    let mut s = Sheet::new(SLIDE_W, SLIDE_H);
    s.bg("#0B1120");
    s.ellipse_a(1640.0, 880.0, 520.0, 520.0, "#7C3AED", 0.40, "Glow A");
    s.ellipse_a(220.0, 160.0, 360.0, 360.0, "#06B6D4", 0.25, "Glow B");
    s.circle(180.0, 200.0, 44.0, "#7C3AED", "Logo");
    s.text(250.0, 168.0, 40.0, "#F8FAFC", "Lumen", "Brand");
    s.text(160.0, 420.0, 130.0, "#F8FAFC", "The operating system", "Title 1");
    s.text(160.0, 560.0, 130.0, "#A78BFA", "for solo founders", "Title 2");
    s.rect(160.0, 740.0, 300.0, 8.0, "#06B6D4", "Rule");
    s.text(160.0, 800.0, 38.0, "#CBD5E1", "Seed round · 2026", "Sub");
    s.text(160.0, 980.0, 30.0, "#64748B", "Confidential — do not distribute", "Footer");
    BundledTemplate {
        dir_name: "pitch-cover.ktemplate",
        manifest: manifest(
            49,
            "Pitch — Cover",
            "Investor pitch cover slide with brand mark, bold positioning headline, and round label.",
            TemplateCategory::PitchDeck,
            &["pitch", "deck", "cover", "startup", "investor"],
        ),
        content: s.finish(),
    }
}

fn pitch_problem() -> BundledTemplate {
    let mut s = Sheet::new(SLIDE_W, SLIDE_H);
    s.bg("#FFFFFF");
    s.text(150.0, 110.0, 40.0, "#EF4444", "THE PROBLEM", "Eyebrow");
    s.text(150.0, 190.0, 80.0, "#0F172A", "Founders drown in busywork", "Title");
    let pains = [
        ("#FEE2E2", "#B91C1C", "11 tools", "to run one small business"),
        ("#FEF3C7", "#B45309", "60% of time", "spent on admin, not building"),
        ("#EDE9FE", "#6D28D9", "$0 budget", "for a full operations team"),
    ];
    for (i, (bg, ink, big, small)) in pains.iter().enumerate() {
        let x = 150.0 + i as f64 * 540.0;
        s.rect(x, 360.0, 480.0, 520.0, bg, "Pain card");
        s.text(x + 50.0, 430.0, 72.0, ink, big, "Pain big");
        s.text(x + 50.0, 560.0, 32.0, "#475569", small, "Pain small 1");
    }
    BundledTemplate {
        dir_name: "pitch-problem.ktemplate",
        manifest: manifest(
            50,
            "Pitch — Problem",
            "Pitch problem slide with three pain-point cards using bold stats and supporting lines.",
            TemplateCategory::PitchDeck,
            &["pitch", "deck", "problem", "pain", "investor"],
        ),
        content: s.finish(),
    }
}

fn pitch_solution() -> BundledTemplate {
    let mut s = Sheet::new(SLIDE_W, SLIDE_H);
    s.bg("#ECFEFF");
    s.text(150.0, 110.0, 40.0, "#0891B2", "OUR SOLUTION", "Eyebrow");
    s.text(150.0, 190.0, 80.0, "#0F172A", "One workspace, everything", "Title");
    s.rect(150.0, 340.0, 820.0, 600.0, "#0E7490", "Showcase");
    s.rect(200.0, 400.0, 720.0, 90.0, "#155E75", "Window bar");
    s.circle(250.0, 445.0, 18.0, "#F87171", "Dot");
    s.circle(310.0, 445.0, 18.0, "#FBBF24", "Dot");
    s.circle(370.0, 445.0, 18.0, "#34D399", "Dot");
    s.rect(200.0, 520.0, 340.0, 380.0, "#22D3EE", "Panel A");
    s.rect(560.0, 520.0, 360.0, 180.0, "#67E8F9", "Panel B");
    s.rect(560.0, 720.0, 360.0, 180.0, "#A5F3FC", "Panel C");
    let feats = [
        ("Invoicing & payments", "Get paid in two taps"),
        ("Client CRM", "Every contact in one place"),
        ("Automations", "Let routine work run itself"),
    ];
    let mut y = 400.0;
    for (t, b) in feats {
        s.circle(1060.0, y + 24.0, 20.0, "#0891B2", "Bullet");
        s.text(1110.0, y, 40.0, "#0F172A", t, "Feat title");
        s.text(1110.0, y + 56.0, 30.0, "#475569", b, "Feat body");
        y += 180.0;
    }
    BundledTemplate {
        dir_name: "pitch-solution.ktemplate",
        manifest: manifest(
            51,
            "Pitch — Solution",
            "Pitch solution slide pairing a product mockup with three headline capabilities.",
            TemplateCategory::PitchDeck,
            &["pitch", "deck", "solution", "product", "investor"],
        ),
        content: s.finish(),
    }
}

fn pitch_market() -> BundledTemplate {
    let mut s = Sheet::new(SLIDE_W, SLIDE_H);
    s.bg("#0F172A");
    s.text(150.0, 110.0, 40.0, "#A78BFA", "MARKET SIZE", "Eyebrow");
    s.text(150.0, 190.0, 80.0, "#F8FAFC", "A $48B opportunity", "Title");
    // Concentric TAM/SAM/SOM.
    s.circle(620.0, 660.0, 320.0, "#312E81", "TAM");
    s.circle(620.0, 720.0, 220.0, "#4338CA", "SAM");
    s.circle(620.0, 780.0, 120.0, "#7C3AED", "SOM");
    let legend = [
        ("#312E81", "TAM", "$48B — global SMB software"),
        ("#4338CA", "SAM", "$12B — solo & micro teams"),
        ("#7C3AED", "SOM", "$900M — reachable in 5 yrs"),
    ];
    let mut y = 420.0;
    for (hex, tag, body) in legend {
        s.rect(1120.0, y, 60.0, 60.0, hex, "Legend swatch");
        s.text(1210.0, y, 44.0, "#F8FAFC", tag, "Legend tag");
        s.text(1210.0, y + 60.0, 28.0, "#94A3B8", body, "Legend body");
        y += 170.0;
    }
    BundledTemplate {
        dir_name: "pitch-market.ktemplate",
        manifest: manifest(
            52,
            "Pitch — Market Size",
            "Pitch market-size slide with concentric TAM/SAM/SOM circles and a labeled legend.",
            TemplateCategory::PitchDeck,
            &["pitch", "deck", "market", "tam", "investor"],
        ),
        content: s.finish(),
    }
}

fn pitch_business_model() -> BundledTemplate {
    let mut s = Sheet::new(SLIDE_W, SLIDE_H);
    s.bg("#FFFFFF");
    s.text(150.0, 110.0, 40.0, "#059669", "BUSINESS MODEL", "Eyebrow");
    s.text(150.0, 190.0, 80.0, "#0F172A", "Predictable, recurring revenue", "Title");
    let tiers = [
        ("#ECFDF5", "#047857", "Free", "$0", "Acquisition engine"),
        ("#D1FAE5", "#065F46", "Pro", "$12/mo", "Core revenue driver"),
        ("#A7F3D0", "#064E3B", "Team", "$29/mo", "Expansion & upsell"),
    ];
    for (i, (bg, ink, name, price, role)) in tiers.iter().enumerate() {
        let x = 150.0 + i as f64 * 540.0;
        s.rect(x, 360.0, 480.0, 480.0, bg, "Tier card");
        s.text(x + 50.0, 420.0, 44.0, ink, name, "Tier name");
        s.text(x + 50.0, 510.0, 72.0, "#0F172A", price, "Tier price");
        s.text(x + 50.0, 640.0, 30.0, "#475569", role, "Tier role");
    }
    s.text(150.0, 920.0, 34.0, "#059669", "Net revenue retention: 124%", "Metric");
    BundledTemplate {
        dir_name: "pitch-business-model.ktemplate",
        manifest: manifest(
            53,
            "Pitch — Business Model",
            "Pitch business-model slide showing free/pro/team tiers and a retention metric.",
            TemplateCategory::PitchDeck,
            &["pitch", "deck", "business model", "revenue", "investor"],
        ),
        content: s.finish(),
    }
}

fn pitch_competition() -> BundledTemplate {
    let mut s = Sheet::new(SLIDE_W, SLIDE_H);
    s.bg("#0B1120");
    s.text(150.0, 110.0, 40.0, "#38BDF8", "COMPETITIVE LANDSCAPE", "Eyebrow");
    s.text(150.0, 190.0, 80.0, "#F8FAFC", "We own the top-right", "Title");
    // Quadrant.
    s.rect(520.0, 360.0, 900.0, 620.0, "#111827", "Quad bg");
    s.rect(960.0, 360.0, 6.0, 620.0, "#1E293B", "V axis");
    s.rect(520.0, 660.0, 900.0, 6.0, "#1E293B", "H axis");
    s.text(540.0, 320.0, 26.0, "#64748B", "All-in-one →", "Axis x");
    s.text(440.0, 380.0, 26.0, "#64748B", "Easy ↑", "Axis y");
    let dots = [
        ("#64748B", 660.0, 840.0, "Legacy A"),
        ("#64748B", 760.0, 560.0, "Legacy B"),
        ("#64748B", 1040.0, 880.0, "Point tool"),
        ("#7C3AED", 1280.0, 460.0, "Lumen"),
    ];
    for (hex, x, y, label) in dots {
        let r = if label == "Lumen" { 44.0 } else { 28.0 };
        s.circle(x, y, r, hex, "Competitor");
        s.text(x + 40.0, y - 16.0, 26.0, "#CBD5E1", label, "Comp label");
    }
    BundledTemplate {
        dir_name: "pitch-competition.ktemplate",
        manifest: manifest(
            54,
            "Pitch — Competition",
            "Pitch competitive-landscape slide with a 2x2 quadrant positioning the company top-right.",
            TemplateCategory::PitchDeck,
            &["pitch", "deck", "competition", "quadrant", "investor"],
        ),
        content: s.finish(),
    }
}

fn pitch_financials() -> BundledTemplate {
    let mut s = Sheet::new(SLIDE_W, SLIDE_H);
    s.bg("#FFFFFF");
    s.text(150.0, 110.0, 40.0, "#6366F1", "FINANCIALS", "Eyebrow");
    s.text(150.0, 190.0, 80.0, "#0F172A", "Path to profitability", "Title");
    s.rect(150.0, 860.0, 1620.0, 6.0, "#E2E8F0", "X axis");
    let cols = [
        ("2024", 0.18, "$0.4M"),
        ("2025", 0.34, "$1.2M"),
        ("2026", 0.55, "$3.8M"),
        ("2027", 0.78, "$9.1M"),
        ("2028", 0.96, "$21M"),
    ];
    for (i, (yr, frac, rev)) in cols.iter().enumerate() {
        let x = 260.0 + i as f64 * 300.0;
        let h = 540.0 * frac;
        s.rect(x, 860.0 - h, 180.0, h, "#6366F1", "Bar");
        s.text_center(x + 90.0, 860.0 - h - 50.0, 30.0, "#0F172A", rev, "Rev");
        s.text_center(x + 90.0, 900.0, 32.0, "#64748B", yr, "Year");
    }
    // Break-even marker.
    s.rect(150.0, 560.0, 1620.0, 4.0, "#10B981", "Break-even");
    s.text(1500.0, 500.0, 28.0, "#10B981", "Break-even 2026", "BE label");
    BundledTemplate {
        dir_name: "pitch-financials.ktemplate",
        manifest: manifest(
            55,
            "Pitch — Financials",
            "Pitch financials slide with a five-year revenue bar chart and a break-even marker line.",
            TemplateCategory::PitchDeck,
            &["pitch", "deck", "financials", "projections", "investor"],
        ),
        content: s.finish(),
    }
}

fn pitch_ask() -> BundledTemplate {
    let mut s = Sheet::new(SLIDE_W, SLIDE_H);
    s.bg("#4338CA");
    s.ellipse_a(1660.0, 900.0, 460.0, 460.0, "#312E81", 0.60, "Glow");
    s.text(150.0, 200.0, 40.0, "#C7D2FE", "THE ASK", "Eyebrow");
    s.text(150.0, 300.0, 170.0, "#FFFFFF", "Raising $3M", "Title");
    s.text(150.0, 500.0, 44.0, "#E0E7FF", "to reach 100k users in 18 months", "Sub");
    let uses = [
        ("40%", "Engineering"),
        ("30%", "Growth & marketing"),
        ("20%", "Operations"),
        ("10%", "Runway buffer"),
    ];
    let mut y = 660.0;
    for (pct, label) in uses {
        s.rect(150.0, y, 90.0, 90.0, "#A5B4FC", "Use chip");
        s.text_center(195.0, y + 22.0, 34.0, "#1E1B4B", pct, "Use pct");
        s.text(280.0, y + 22.0, 38.0, "#FFFFFF", label, "Use label");
        y += 110.0;
    }
    BundledTemplate {
        dir_name: "pitch-ask.ktemplate",
        manifest: manifest(
            56,
            "Pitch — The Ask",
            "Pitch closing ask slide stating the raise amount, milestone, and a use-of-funds breakdown.",
            TemplateCategory::PitchDeck,
            &["pitch", "deck", "ask", "funding", "investor"],
        ),
        content: s.finish(),
    }
}

// ---------------------------------------------------------------------
// Catalog — social media (mixed native sizes)
// ---------------------------------------------------------------------

fn social_ig_announcement() -> BundledTemplate {
    let mut s = Sheet::new(1080.0, 1080.0);
    s.bg("#111827");
    s.ellipse_a(900.0, 180.0, 360.0, 360.0, "#F97316", 0.45, "Glow A");
    s.ellipse_a(160.0, 940.0, 320.0, 320.0, "#6366F1", 0.40, "Glow B");
    s.rect(80.0, 120.0, 260.0, 70.0, "#F97316", "Badge");
    s.text(110.0, 138.0, 34.0, "#111827", "BIG NEWS", "Badge text");
    s.text(80.0, 320.0, 96.0, "#FFFFFF", "We just", "Title 1");
    s.text(80.0, 430.0, 96.0, "#FFFFFF", "launched", "Title 2");
    s.text(80.0, 540.0, 96.0, "#F97316", "version 2.0", "Title 3");
    s.text(80.0, 720.0, 36.0, "#CBD5E1", "Faster, smarter, and ready for teams.", "Body");
    s.rect(80.0, 840.0, 420.0, 120.0, "#FFFFFF", "CTA");
    s.text_center(290.0, 875.0, 38.0, "#111827", "Learn more", "CTA label");
    BundledTemplate {
        dir_name: "social-ig-announcement.ktemplate",
        manifest: manifest(
            57,
            "Instagram — Announcement",
            "1:1 Instagram announcement post with a news badge, bold headline, and call-to-action button.",
            TemplateCategory::SocialMedia,
            &["instagram", "social", "announcement", "post", "launch"],
        ),
        content: s.finish(),
    }
}

fn social_ig_giveaway() -> BundledTemplate {
    let mut s = Sheet::new(1080.0, 1080.0);
    s.bg("#FDF2F8");
    s.ellipse_a(540.0, 540.0, 520.0, 520.0, "#FBCFE8", 0.60, "Halo");
    s.circle(540.0, 360.0, 140.0, "#EC4899", "Gift");
    s.rect(470.0, 290.0, 140.0, 140.0, "#FDF2F8", "Gift cut");
    s.text_center(540.0, 540.0, 110.0, "#BE185D", "GIVEAWAY", "Title");
    s.text_center(540.0, 690.0, 40.0, "#9D174D", "Win a $250 gift card", "Prize");
    let steps = ["Follow @kcreate", "Like this post", "Tag two friends"];
    let mut y = 800.0;
    for (i, step) in steps.iter().enumerate() {
        s.circle(220.0, y + 18.0, 22.0, "#EC4899", "Step dot");
        s.text_center(232.0, y, 26.0, "#FFFFFF", &format!("{}", i + 1), "Step num");
        s.text(280.0, y, 34.0, "#831843", step, "Step text");
        y += 70.0;
    }
    BundledTemplate {
        dir_name: "social-ig-giveaway.ktemplate",
        manifest: manifest(
            58,
            "Instagram — Giveaway",
            "1:1 giveaway post with a gift motif, prize line, and a numbered how-to-enter list.",
            TemplateCategory::SocialMedia,
            &["instagram", "social", "giveaway", "contest", "post"],
        ),
        content: s.finish(),
    }
}

fn social_ig_tips() -> BundledTemplate {
    let mut s = Sheet::new(1080.0, 1080.0);
    s.bg("#064E3B");
    s.rect(0.0, 0.0, 1080.0, 240.0, "#065F46", "Header");
    s.text(80.0, 90.0, 56.0, "#FFFFFF", "5 tips for", "Header 1");
    s.text(80.0, 160.0, 56.0, "#6EE7B7", "better focus", "Header 2");
    let tips = [
        "Block your calendar in deep-work chunks",
        "Silence non-urgent notifications",
        "Work in 50-minute sprints",
        "Keep one tab, one task",
        "Review wins at the end of the day",
    ];
    let mut y = 320.0;
    for (i, tip) in tips.iter().enumerate() {
        s.circle(130.0, y + 30.0, 40.0, "#10B981", "Num bg");
        s.text_center(130.0, y + 6.0, 40.0, "#064E3B", &format!("{}", i + 1), "Num");
        s.text(200.0, y, 32.0, "#D1FAE5", tip, "Tip");
        y += 130.0;
    }
    s.text(80.0, 1000.0, 28.0, "#6EE7B7", "Save this for later · @kcreate", "Footer");
    BundledTemplate {
        dir_name: "social-ig-tips.ktemplate",
        manifest: manifest(
            59,
            "Instagram — Tips List",
            "1:1 educational carousel-style post with a numbered list of five tips.",
            TemplateCategory::SocialMedia,
            &["instagram", "social", "tips", "list", "educational"],
        ),
        content: s.finish(),
    }
}

fn social_ig_testimonial() -> BundledTemplate {
    let mut s = Sheet::new(1080.0, 1080.0);
    s.bg("#1E293B");
    s.text(80.0, 140.0, 200.0, "#334155", "\u{201C}", "Quote mark");
    s.text(80.0, 380.0, 52.0, "#F8FAFC", "This app completely", "Quote 1");
    s.text(80.0, 460.0, 52.0, "#F8FAFC", "changed how our", "Quote 2");
    s.text(80.0, 540.0, 52.0, "#38BDF8", "studio ships work.", "Quote 3");
    for i in 0..5 {
        s.circle(110.0 + f64::from(i) * 70.0, 700.0, 24.0, "#FBBF24", "Star");
    }
    s.circle(130.0, 880.0, 60.0, "#38BDF8", "Avatar");
    s.text(220.0, 840.0, 38.0, "#F8FAFC", "Priya N.", "Name");
    s.text(220.0, 900.0, 28.0, "#94A3B8", "Creative Director", "Role");
    BundledTemplate {
        dir_name: "social-ig-testimonial.ktemplate",
        manifest: manifest(
            60,
            "Instagram — Testimonial",
            "1:1 testimonial post with a large quote, 5-star rating, and an attributed reviewer.",
            TemplateCategory::SocialMedia,
            &["instagram", "social", "testimonial", "review", "post"],
        ),
        content: s.finish(),
    }
}

fn social_ig_product() -> BundledTemplate {
    let mut s = Sheet::new(1080.0, 1080.0);
    s.bg("#FFF7ED");
    s.circle(540.0, 470.0, 300.0, "#FB923C", "Product");
    s.circle(640.0, 380.0, 110.0, "#FDBA74", "Product hi");
    s.ellipse_a(540.0, 820.0, 360.0, 90.0, "#FDBA74", 0.50, "Shadow");
    s.rect(80.0, 80.0, 240.0, 80.0, "#EA580C", "Sale tag");
    s.text(110.0, 100.0, 40.0, "#FFFFFF", "-30%", "Sale");
    s.text_center(540.0, 850.0, 56.0, "#7C2D12", "Citrus Press", "Name");
    s.text_center(540.0, 930.0, 40.0, "#EA580C", "$39 · was $56", "Price");
    s.rect(330.0, 1000.0, 420.0, 60.0, "#7C2D12", "CTA");
    s.text_center(540.0, 1012.0, 30.0, "#FFFFFF", "Shop now", "CTA label");
    BundledTemplate {
        dir_name: "social-ig-product.ktemplate",
        manifest: manifest(
            61,
            "Instagram — Product Promo",
            "1:1 product promo post with a hero product, sale tag, price, and shop CTA.",
            TemplateCategory::SocialMedia,
            &["instagram", "social", "product", "sale", "promo"],
        ),
        content: s.finish(),
    }
}

fn social_ig_carousel() -> BundledTemplate {
    let mut s = Sheet::new(1080.0, 1080.0);
    s.bg("#312E81");
    s.ellipse_a(880.0, 880.0, 360.0, 360.0, "#6366F1", 0.45, "Glow");
    s.text(80.0, 160.0, 34.0, "#A5B4FC", "SWIPE TO READ →", "Hint");
    s.text(80.0, 340.0, 110.0, "#FFFFFF", "The 2026", "Title 1");
    s.text(80.0, 460.0, 110.0, "#FFFFFF", "design", "Title 2");
    s.text(80.0, 580.0, 110.0, "#C4B5FD", "trends", "Title 3");
    s.text(80.0, 780.0, 34.0, "#C7D2FE", "A 6-part breakdown for creators", "Sub");
    // Pager dots.
    for i in 0..6 {
        let hex = if i == 0 { "#FFFFFF" } else { "#6366F1" };
        s.circle(110.0 + f64::from(i) * 60.0, 960.0, 16.0, hex, "Dot");
    }
    BundledTemplate {
        dir_name: "social-ig-carousel.ktemplate",
        manifest: manifest(
            62,
            "Instagram — Carousel Cover",
            "1:1 carousel cover with a swipe hint, bold multi-line title, and pager dots.",
            TemplateCategory::SocialMedia,
            &["instagram", "social", "carousel", "cover", "post"],
        ),
        content: s.finish(),
    }
}

fn social_story_countdown() -> BundledTemplate {
    let mut s = Sheet::new(1080.0, 1920.0);
    s.bg("#0F172A");
    s.ellipse_a(540.0, 520.0, 520.0, 520.0, "#7C3AED", 0.40, "Glow");
    s.text_center(540.0, 360.0, 40.0, "#A78BFA", "STARTS IN", "Eyebrow");
    let units = [("02", "DAYS"), ("14", "HRS"), ("36", "MIN")];
    for (i, (num, label)) in units.iter().enumerate() {
        let x = 180.0 + i as f64 * 280.0;
        s.rect(x, 520.0, 220.0, 240.0, "#1E293B", "Unit box");
        s.text_center(x + 110.0, 560.0, 110.0, "#FFFFFF", num, "Unit num");
        s.text_center(x + 110.0, 700.0, 28.0, "#94A3B8", label, "Unit label");
    }
    s.text_center(540.0, 920.0, 80.0, "#FFFFFF", "Spring Sale", "Title");
    s.text_center(540.0, 1040.0, 40.0, "#C4B5FD", "Up to 50% off everything", "Sub");
    s.rect(240.0, 1640.0, 600.0, 130.0, "#7C3AED", "CTA");
    s.text_center(540.0, 1675.0, 40.0, "#FFFFFF", "Set reminder", "CTA label");
    BundledTemplate {
        dir_name: "social-story-countdown.ktemplate",
        manifest: manifest(
            63,
            "Story — Countdown",
            "9:16 story with a days/hours/minutes countdown, sale headline, and reminder CTA.",
            TemplateCategory::SocialMedia,
            &["story", "instagram", "countdown", "sale", "social"],
        ),
        content: s.finish(),
    }
}

fn social_story_poll() -> BundledTemplate {
    let mut s = Sheet::new(1080.0, 1920.0);
    s.bg("#F0FDFA");
    s.rect(0.0, 0.0, 1080.0, 700.0, "#0D9488", "Header");
    s.ellipse_a(160.0, 160.0, 280.0, 280.0, "#FFFFFF", 0.14, "Glow");
    s.text(80.0, 300.0, 80.0, "#FFFFFF", "Which one", "Q 1");
    s.text(80.0, 400.0, 80.0, "#CCFBF1", "do you prefer?", "Q 2");
    // Options.
    let opts = [("Option A", "#14B8A6"), ("Option B", "#0F766E")];
    for (i, (label, hex)) in opts.iter().enumerate() {
        let y = 860.0 + i as f64 * 280.0;
        s.rect(120.0, y, 840.0, 220.0, "#FFFFFF", "Opt card");
        s.rect(120.0, y, 24.0, 220.0, hex, "Opt accent");
        s.text(180.0, y + 70.0, 52.0, "#0F172A", label, "Opt label");
        s.circle(880.0, y + 110.0, 50.0, hex, "Opt circle");
    }
    s.text_center(540.0, 1640.0, 36.0, "#0F766E", "Tap to vote 👆", "Footer");
    BundledTemplate {
        dir_name: "social-story-poll.ktemplate",
        manifest: manifest(
            64,
            "Story — Poll",
            "9:16 engagement story with a question header and two tappable answer cards.",
            TemplateCategory::SocialMedia,
            &["story", "instagram", "poll", "engagement", "social"],
        ),
        content: s.finish(),
    }
}

fn social_reels_cover() -> BundledTemplate {
    let mut s = Sheet::new(1080.0, 1920.0);
    s.bg("#18181B");
    s.ellipse_a(540.0, 980.0, 520.0, 620.0, "#DB2777", 0.40, "Glow");
    s.circle(540.0, 760.0, 130.0, "#FFFFFF", "Play bg");
    s.text_center(560.0, 700.0, 120.0, "#DB2777", "\u{25B6}", "Play");
    s.text_center(540.0, 1040.0, 96.0, "#FFFFFF", "HOW I EDIT", "Title 1");
    s.text_center(540.0, 1160.0, 96.0, "#F9A8D4", "IN 60 SEC", "Title 2");
    s.rect(340.0, 1340.0, 400.0, 90.0, "#DB2777", "Tag");
    s.text_center(540.0, 1362.0, 36.0, "#FFFFFF", "TUTORIAL", "Tag label");
    s.text_center(540.0, 1700.0, 32.0, "#A1A1AA", "New reels every week · @kcreate", "Footer");
    BundledTemplate {
        dir_name: "social-reels-cover.ktemplate",
        manifest: manifest(
            65,
            "Reels — Cover",
            "9:16 reels/short cover with a play button, bold two-line hook, and a category tag.",
            TemplateCategory::SocialMedia,
            &["reels", "tiktok", "short", "cover", "social"],
        ),
        content: s.finish(),
    }
}

fn social_linkedin_post() -> BundledTemplate {
    let mut s = Sheet::new(1200.0, 627.0);
    s.bg("#FFFFFF");
    s.rect(0.0, 0.0, 16.0, 627.0, "#0A66C2", "Edge");
    s.rect(0.0, 0.0, 1200.0, 90.0, "#EFF6FF", "Top strip");
    s.circle(60.0, 45.0, 26.0, "#0A66C2", "Logo");
    s.text(110.0, 28.0, 30.0, "#0A66C2", "KCreate", "Brand");
    s.text(80.0, 170.0, 36.0, "#0A66C2", "LESSON #07", "Eyebrow");
    s.text(80.0, 240.0, 64.0, "#0F172A", "Ship small, ship often.", "Title");
    s.text(80.0, 360.0, 30.0, "#475569", "The teams that win aren't the ones with the", "Body 1");
    s.text(80.0, 410.0, 30.0, "#475569", "biggest plans — they're the ones who keep shipping.", "Body 2");
    s.circle(110.0, 540.0, 36.0, "#93C5FD", "Avatar");
    s.text(170.0, 510.0, 30.0, "#0F172A", "Dana Cole", "Author");
    s.text(170.0, 552.0, 24.0, "#64748B", "Founder, KCreate", "Author role");
    BundledTemplate {
        dir_name: "social-linkedin-post.ktemplate",
        manifest: manifest(
            66,
            "LinkedIn — Post",
            "1200×627 LinkedIn post graphic with a brand strip, lesson headline, and author byline.",
            TemplateCategory::SocialMedia,
            &["linkedin", "social", "post", "professional", "thought leadership"],
        ),
        content: s.finish(),
    }
}

fn social_x_header() -> BundledTemplate {
    let mut s = Sheet::new(1500.0, 500.0);
    s.bg("#0F1419");
    s.ellipse_a(1280.0, 250.0, 360.0, 360.0, "#1D9BF0", 0.40, "Glow");
    s.rect(0.0, 430.0, 1500.0, 70.0, "#16202A", "Lower bar");
    s.text(80.0, 150.0, 80.0, "#FFFFFF", "Building in public.", "Title");
    s.text(80.0, 270.0, 36.0, "#8B98A5", "Design, code, and the occasional hot take.", "Sub");
    s.rect(80.0, 360.0, 220.0, 12.0, "#1D9BF0", "Rule");
    s.text(1180.0, 450.0, 28.0, "#8B98A5", "@kcreate_app", "Handle");
    BundledTemplate {
        dir_name: "social-x-header.ktemplate",
        manifest: manifest(
            67,
            "X — Header",
            "1500×500 X / Twitter profile header with a tagline, accent rule, and handle.",
            TemplateCategory::SocialMedia,
            &["twitter", "x", "header", "banner", "social"],
        ),
        content: s.finish(),
    }
}

fn social_x_post() -> BundledTemplate {
    let mut s = Sheet::new(1200.0, 675.0);
    s.bg("#15202B");
    s.rect(60.0, 60.0, 1080.0, 555.0, "#1E2732", "Card");
    s.circle(130.0, 150.0, 44.0, "#1D9BF0", "Avatar");
    s.text(200.0, 120.0, 34.0, "#FFFFFF", "KCreate", "Name");
    s.text(200.0, 168.0, 28.0, "#8B98A5", "@kcreate_app", "Handle");
    s.text(110.0, 270.0, 48.0, "#FFFFFF", "We rebuilt our renderer in Rust", "Tweet 1");
    s.text(110.0, 340.0, 48.0, "#FFFFFF", "and exports got 9× faster. 🚀", "Tweet 2");
    s.text(110.0, 470.0, 28.0, "#8B98A5", "2:14 PM · Jun 12, 2026", "Meta");
    s.rect(110.0, 520.0, 980.0, 3.0, "#38444D", "Divider");
    s.text(110.0, 545.0, 28.0, "#8B98A5", "1.2K Reposts   4.8K Likes", "Stats");
    BundledTemplate {
        dir_name: "social-x-post.ktemplate",
        manifest: manifest(
            68,
            "X — Post Card",
            "1200×675 X / Twitter post card with avatar, handle, tweet copy, and engagement stats.",
            TemplateCategory::SocialMedia,
            &["twitter", "x", "post", "card", "social"],
        ),
        content: s.finish(),
    }
}

fn social_yt_thumbnail() -> BundledTemplate {
    let mut s = Sheet::new(1280.0, 720.0);
    s.bg("#0B1120");
    s.rect(0.0, 0.0, 720.0, 720.0, "#7C3AED", "Photo block");
    s.ellipse_a(360.0, 360.0, 280.0, 280.0, "#FFFFFF", 0.16, "Glow");
    s.circle(360.0, 340.0, 150.0, "#FBBF24", "Face");
    s.text_center(390.0, 470.0, 70.0, "#7C3AED", "!!!", "Expression");
    s.text(760.0, 120.0, 110.0, "#FFFFFF", "I TRIED", "Title 1");
    s.text(760.0, 240.0, 110.0, "#FBBF24", "120 NEW", "Title 2");
    s.text(760.0, 360.0, 110.0, "#FFFFFF", "TEMPLATES", "Title 3");
    s.rect(760.0, 540.0, 360.0, 90.0, "#EF4444", "Tag");
    s.text_center(940.0, 562.0, 44.0, "#FFFFFF", "INSANE", "Tag label");
    BundledTemplate {
        dir_name: "social-yt-thumbnail.ktemplate",
        manifest: manifest(
            69,
            "YouTube — Thumbnail",
            "1280×720 YouTube thumbnail with a face block, oversized punchy title, and a red tag.",
            TemplateCategory::SocialMedia,
            &["youtube", "thumbnail", "video", "social"],
        ),
        content: s.finish(),
    }
}

fn social_fb_cover() -> BundledTemplate {
    let mut s = Sheet::new(820.0, 312.0);
    s.bg("#1877F2");
    s.ellipse_a(700.0, 80.0, 220.0, 220.0, "#FFFFFF", 0.16, "Glow");
    s.rect(0.0, 250.0, 820.0, 62.0, "#0F5BD6", "Lower bar");
    s.circle(70.0, 130.0, 44.0, "#FFFFFF", "Logo");
    s.text(140.0, 96.0, 52.0, "#FFFFFF", "KCreate Studio", "Title");
    s.text(140.0, 168.0, 26.0, "#DBEAFE", "Design tools for everyone", "Sub");
    s.text(560.0, 268.0, 24.0, "#DBEAFE", "kcreate.app", "URL");
    BundledTemplate {
        dir_name: "social-fb-cover.ktemplate",
        manifest: manifest(
            70,
            "Facebook — Cover",
            "820×312 Facebook page cover with a logo, page name, tagline, and URL.",
            TemplateCategory::SocialMedia,
            &["facebook", "cover", "banner", "page", "social"],
        ),
        content: s.finish(),
    }
}

fn social_pinterest_pin() -> BundledTemplate {
    let mut s = Sheet::new(1000.0, 1500.0);
    s.bg("#FFF1F2");
    s.rect(0.0, 0.0, 1000.0, 760.0, "#E11D48", "Photo block");
    s.ellipse_a(500.0, 360.0, 320.0, 320.0, "#FFFFFF", 0.16, "Glow");
    s.circle(420.0, 360.0, 150.0, "#FDA4AF", "Shape A");
    s.rect(520.0, 280.0, 220.0, 220.0, "#FB7185", "Shape B");
    s.rect(120.0, 860.0, 760.0, 480.0, "#FFFFFF", "Text card");
    s.text(170.0, 920.0, 36.0, "#E11D48", "DIY GUIDE", "Eyebrow");
    s.text(170.0, 1000.0, 72.0, "#881337", "10 cozy fall", "Title 1");
    s.text(170.0, 1090.0, 72.0, "#881337", "home ideas", "Title 2");
    s.text(170.0, 1220.0, 30.0, "#9F1239", "Easy weekend projects · save for later", "Body");
    BundledTemplate {
        dir_name: "social-pinterest-pin.ktemplate",
        manifest: manifest(
            71,
            "Pinterest — Pin",
            "1000×1500 vertical Pinterest pin with a photo block and a text card title/subtitle.",
            TemplateCategory::SocialMedia,
            &["pinterest", "pin", "vertical", "social"],
        ),
        content: s.finish(),
    }
}

fn social_podcast_cover() -> BundledTemplate {
    let mut s = Sheet::new(1400.0, 1400.0);
    s.bg("#111827");
    s.ellipse_a(700.0, 700.0, 620.0, 620.0, "#F59E0B", 0.25, "Glow");
    s.rect(120.0, 120.0, 1160.0, 1160.0, "#1F2937", "Frame");
    s.rect(120.0, 120.0, 1160.0, 16.0, "#F59E0B", "Frame top");
    // Waveform.
    let bars = [0.4, 0.7, 0.5, 0.9, 0.6, 0.8, 0.45, 0.95, 0.55, 0.75];
    for (i, frac) in bars.iter().enumerate() {
        let x = 360.0 + i as f64 * 70.0;
        let h = 360.0 * frac;
        s.rect(x, 700.0 - h / 2.0, 40.0, h, "#F59E0B", "Wave bar");
    }
    s.text_center(700.0, 300.0, 40.0, "#FCD34D", "THE BUILD LOG", "Eyebrow");
    s.text_center(700.0, 920.0, 130.0, "#FFFFFF", "Makers", "Title 1");
    s.text_center(700.0, 1050.0, 130.0, "#F59E0B", "& Mavericks", "Title 2");
    s.text_center(700.0, 1200.0, 32.0, "#9CA3AF", "A weekly podcast for builders", "Sub");
    BundledTemplate {
        dir_name: "social-podcast-cover.ktemplate",
        manifest: manifest(
            72,
            "Podcast — Cover Art",
            "1400×1400 square podcast cover with a framed waveform motif and show title.",
            TemplateCategory::SocialMedia,
            &["podcast", "cover", "audio", "square", "social"],
        ),
        content: s.finish(),
    }
}

fn social_ig_event() -> BundledTemplate {
    let mut s = Sheet::new(1080.0, 1080.0);
    s.bg("#1E1B4B");
    s.ellipse_a(880.0, 200.0, 320.0, 320.0, "#F472B6", 0.40, "Glow A");
    s.ellipse_a(180.0, 900.0, 320.0, 320.0, "#22D3EE", 0.35, "Glow B");
    s.text(80.0, 130.0, 34.0, "#F9A8D4", "YOU'RE INVITED", "Eyebrow");
    s.text(80.0, 280.0, 96.0, "#FFFFFF", "Design", "Title 1");
    s.text(80.0, 390.0, 96.0, "#FFFFFF", "Meetup", "Title 2");
    s.text(80.0, 500.0, 96.0, "#22D3EE", "2026", "Title 3");
    s.rect(80.0, 680.0, 920.0, 3.0, "#4338CA", "Divider");
    let rows = [("📅", "Friday, July 18 · 6:30 PM"), ("📍", "The Foundry, San Francisco"), ("🎟", "Free · RSVP in bio")];
    let mut y = 730.0;
    for (icon, text) in rows {
        s.text(80.0, y, 36.0, "#F9A8D4", icon, "Row icon");
        s.text(160.0, y, 34.0, "#E0E7FF", text, "Row text");
        y += 80.0;
    }
    BundledTemplate {
        dir_name: "social-ig-event.ktemplate",
        manifest: manifest(
            73,
            "Instagram — Event Invite",
            "1:1 event invite post with a bold title and date/location/RSVP detail rows.",
            TemplateCategory::SocialMedia,
            &["instagram", "social", "event", "invite", "post"],
        ),
        content: s.finish(),
    }
}

fn social_ig_motivation() -> BundledTemplate {
    let mut s = Sheet::new(1080.0, 1080.0);
    s.bg("#FAFAF9");
    s.rect(0.0, 0.0, 1080.0, 24.0, "#0F172A", "Top rule");
    s.rect(0.0, 1056.0, 1080.0, 24.0, "#0F172A", "Bottom rule");
    s.text_center(540.0, 360.0, 90.0, "#0F172A", "Done is", "Quote 1");
    s.text_center(540.0, 470.0, 90.0, "#0F172A", "better than", "Quote 2");
    s.text_center(540.0, 580.0, 90.0, "#F59E0B", "perfect.", "Quote 3");
    s.rect(440.0, 740.0, 200.0, 8.0, "#0F172A", "Rule");
    s.text_center(540.0, 800.0, 30.0, "#78716C", "@kcreate", "Handle");
    BundledTemplate {
        dir_name: "social-ig-motivation.ktemplate",
        manifest: manifest(
            74,
            "Instagram — Motivation Quote",
            "1:1 minimalist motivational quote post with framing rules and an accent word.",
            TemplateCategory::SocialMedia,
            &["instagram", "social", "quote", "motivation", "post"],
        ),
        content: s.finish(),
    }
}

// ---------------------------------------------------------------------
// Catalog — posters (1080 × 1350, 4:5 print)
// ---------------------------------------------------------------------

fn poster_movie() -> BundledTemplate {
    let mut s = Sheet::new(POSTER_W, POSTER_H);
    s.bg("#0B0B0F");
    s.rect(0.0, 0.0, POSTER_W, 840.0, "#18181B", "Key art");
    s.ellipse_a(540.0, 420.0, 360.0, 360.0, "#DC2626", 0.55, "Glow");
    s.circle(540.0, 420.0, 150.0, "#0B0B0F", "Moon");
    s.circle(610.0, 380.0, 150.0, "#18181B", "Moon shadow");
    s.rect(0.0, 800.0, POSTER_W, 8.0, "#DC2626", "Hairline");
    s.text_center(540.0, 880.0, 40.0, "#DC2626", "A FILM BY A. RIVERA", "Credit");
    s.text_center(540.0, 950.0, 150.0, "#F4F4F5", "AFTER", "Title 1");
    s.text_center(540.0, 1090.0, 150.0, "#F4F4F5", "DARK", "Title 2");
    s.text_center(540.0, 1260.0, 26.0, "#A1A1AA", "IN THEATERS DECEMBER · RATED PG-13", "Footer");
    BundledTemplate {
        dir_name: "poster-movie.ktemplate",
        manifest: manifest(
            75,
            "Movie Poster",
            "4:5 cinematic movie poster with key art, an eclipse motif, and billing-block footer.",
            TemplateCategory::Poster,
            &["poster", "movie", "film", "cinema"],
        ),
        content: s.finish(),
    }
}

fn poster_art_expo() -> BundledTemplate {
    let mut s = Sheet::new(POSTER_W, POSTER_H);
    s.bg("#FAF5EB");
    s.rect(80.0, 80.0, POSTER_W - 160.0, POSTER_H - 160.0, "#FAF5EB", "Mat");
    s.rect(80.0, 80.0, POSTER_W - 160.0, 8.0, "#1C1917", "Frame top");
    s.rect(80.0, POSTER_H - 88.0, POSTER_W - 160.0, 8.0, "#1C1917", "Frame bottom");
    // Abstract composition.
    s.rect(180.0, 300.0, 320.0, 420.0, "#E11D48", "Block A");
    s.circle(660.0, 420.0, 170.0, "#2563EB", "Block B");
    s.rect(560.0, 560.0, 300.0, 200.0, "#F59E0B", "Block C");
    s.rect(180.0, 740.0, 680.0, 14.0, "#1C1917", "Rule");
    s.text(180.0, 800.0, 36.0, "#57534E", "MODERN ART EXHIBITION", "Eyebrow");
    s.text(180.0, 860.0, 110.0, "#1C1917", "Forms", "Title 1");
    s.text(180.0, 980.0, 110.0, "#E11D48", "in Motion", "Title 2");
    s.text(180.0, 1160.0, 32.0, "#57534E", "Apr 12 – Jun 30 · The Civic Gallery", "Details");
    BundledTemplate {
        dir_name: "poster-art-expo.ktemplate",
        manifest: manifest(
            76,
            "Art Exhibition Poster",
            "4:5 gallery poster with a framed Bauhaus-style abstract composition and show details.",
            TemplateCategory::Poster,
            &["poster", "art", "exhibition", "gallery", "expo"],
        ),
        content: s.finish(),
    }
}

fn poster_gym() -> BundledTemplate {
    let mut s = Sheet::new(POSTER_W, POSTER_H);
    s.bg("#0A0A0A");
    s.rect(-40.0, 360.0, POSTER_W + 80.0, 220.0, "#EAB308", "Stripe");
    s.text(80.0, 120.0, 40.0, "#EAB308", "NO EXCUSES", "Eyebrow");
    s.text(70.0, 380.0, 150.0, "#0A0A0A", "PUSH", "Title 1");
    s.text(70.0, 540.0, 150.0, "#F5F5F5", "YOUR", "Title 2");
    s.text(70.0, 690.0, 150.0, "#EAB308", "LIMITS", "Title 3");
    s.rect(80.0, 900.0, 920.0, 4.0, "#262626", "Divider");
    let perks = ["24/7 access · all locations", "Free intro session", "No joining fee this month"];
    let mut y = 950.0;
    for perk in perks {
        s.rect(80.0, y + 12.0, 26.0, 26.0, "#EAB308", "Tick");
        s.text(130.0, y, 32.0, "#D4D4D4", perk, "Perk");
        y += 70.0;
    }
    s.rect(80.0, 1190.0, 920.0, 100.0, "#EAB308", "CTA");
    s.text_center(540.0, 1216.0, 40.0, "#0A0A0A", "JOIN TODAY — IRONWORKS GYM", "CTA label");
    BundledTemplate {
        dir_name: "poster-gym.ktemplate",
        manifest: manifest(
            77,
            "Gym Motivation Poster",
            "4:5 high-energy fitness poster with bold display type, perks, and a membership CTA.",
            TemplateCategory::Poster,
            &["poster", "gym", "fitness", "motivation", "sport"],
        ),
        content: s.finish(),
    }
}

fn poster_food() -> BundledTemplate {
    let mut s = Sheet::new(POSTER_W, POSTER_H);
    s.bg("#7C2D12");
    s.ellipse_a(540.0, 470.0, 380.0, 380.0, "#FED7AA", 0.20, "Glow");
    s.circle(540.0, 470.0, 300.0, "#FFFBEB", "Plate");
    s.circle(540.0, 470.0, 220.0, "#F59E0B", "Food");
    s.circle(470.0, 410.0, 60.0, "#FBBF24", "Topping A");
    s.circle(620.0, 450.0, 50.0, "#EA580C", "Topping B");
    s.circle(540.0, 560.0, 44.0, "#B45309", "Topping C");
    s.text_center(540.0, 120.0, 40.0, "#FED7AA", "GRAND OPENING", "Eyebrow");
    s.text_center(540.0, 840.0, 130.0, "#FFFBEB", "Saffron", "Title 1");
    s.text_center(540.0, 970.0, 80.0, "#F59E0B", "Kitchen", "Title 2");
    s.text_center(540.0, 1120.0, 34.0, "#FED7AA", "Authentic flavors · open 11am–11pm", "Details");
    s.text_center(540.0, 1200.0, 30.0, "#FDBA74", "12 Market Street · saffron.kitchen", "Address");
    BundledTemplate {
        dir_name: "poster-food.ktemplate",
        manifest: manifest(
            78,
            "Restaurant Opening Poster",
            "4:5 food poster with an overhead plate illustration and grand-opening details.",
            TemplateCategory::Poster,
            &["poster", "food", "restaurant", "opening", "menu"],
        ),
        content: s.finish(),
    }
}

fn poster_real_estate() -> BundledTemplate {
    let mut s = Sheet::new(POSTER_W, POSTER_H);
    s.bg("#F8FAFC");
    s.rect(0.0, 0.0, POSTER_W, 720.0, "#1E3A8A", "Photo block");
    s.ellipse_a(880.0, 120.0, 240.0, 240.0, "#3B82F6", 0.40, "Glow");
    // House silhouette.
    s.rect(360.0, 420.0, 360.0, 220.0, "#E2E8F0", "House");
    s.rect(360.0, 320.0, 200.0, 120.0, "#CBD5E1", "Roof base");
    s.rect(470.0, 480.0, 90.0, 160.0, "#1E3A8A", "Door");
    s.rect(600.0, 470.0, 80.0, 80.0, "#93C5FD", "Window");
    s.rect(80.0, 110.0, 280.0, 70.0, "#FBBF24", "Badge");
    s.text(105.0, 128.0, 36.0, "#1E3A8A", "FOR SALE", "Badge text");
    s.text(80.0, 780.0, 110.0, "#0F172A", "$845,000", "Price");
    s.text(80.0, 910.0, 38.0, "#1E3A8A", "4 bed · 3 bath · 2,650 sqft", "Specs");
    s.rect(80.0, 980.0, 920.0, 3.0, "#CBD5E1", "Divider");
    s.text(80.0, 1020.0, 34.0, "#334155", "27 Lakeview Drive, Austin TX", "Address");
    s.text(80.0, 1090.0, 30.0, "#64748B", "Open house Sat & Sun · 1–4pm", "Open house");
    s.rect(80.0, 1180.0, 920.0, 100.0, "#1E3A8A", "CTA");
    s.text_center(540.0, 1206.0, 36.0, "#FFFFFF", "Jordan Lee · (555) 010-7788", "CTA label");
    BundledTemplate {
        dir_name: "poster-real-estate.ktemplate",
        manifest: manifest(
            79,
            "Real Estate Listing Poster",
            "4:5 property listing with a house hero, price, specs, address, and agent CTA.",
            TemplateCategory::Poster,
            &["poster", "real estate", "property", "listing", "house"],
        ),
        content: s.finish(),
    }
}

fn poster_workshop() -> BundledTemplate {
    let mut s = Sheet::new(POSTER_W, POSTER_H);
    s.bg("#ECFEFF");
    s.rect(0.0, 0.0, POSTER_W, 24.0, "#0E7490", "Top rule");
    s.circle(870.0, 250.0, 150.0, "#A5F3FC", "Accent circle");
    s.circle(940.0, 320.0, 80.0, "#22D3EE", "Accent circle 2");
    s.text(80.0, 150.0, 36.0, "#0E7490", "HANDS-ON WORKSHOP", "Eyebrow");
    s.text(80.0, 320.0, 120.0, "#083344", "Watercolor", "Title 1");
    s.text(80.0, 450.0, 120.0, "#0891B2", "Basics", "Title 2");
    s.text(80.0, 640.0, 34.0, "#155E75", "A relaxed 3-hour intro for total beginners.", "Body");
    s.rect(80.0, 740.0, 920.0, 3.0, "#67E8F9", "Divider");
    let rows = [("When", "Sat, May 17 · 10am–1pm"), ("Where", "Studio 4, Riverside Arts"), ("Cost", "$45 · materials included")];
    let mut y = 800.0;
    for (label, val) in rows {
        s.text(80.0, y, 30.0, "#0E7490", label, "Row label");
        s.text(360.0, y, 30.0, "#083344", val, "Row value");
        y += 90.0;
    }
    s.rect(80.0, 1170.0, 920.0, 100.0, "#0891B2", "CTA");
    s.text_center(540.0, 1196.0, 36.0, "#FFFFFF", "Reserve your seat — riversidearts.org", "CTA label");
    BundledTemplate {
        dir_name: "poster-workshop.ktemplate",
        manifest: manifest(
            80,
            "Workshop Poster",
            "4:5 class/workshop poster with friendly type and when/where/cost detail rows.",
            TemplateCategory::Poster,
            &["poster", "workshop", "class", "event", "education"],
        ),
        content: s.finish(),
    }
}

fn poster_travel() -> BundledTemplate {
    let mut s = Sheet::new(POSTER_W, POSTER_H);
    s.bg("#F59E0B");
    s.rect(0.0, 0.0, POSTER_W, 760.0, "#0EA5E9", "Sky");
    s.circle(820.0, 240.0, 110.0, "#FDE68A", "Sun");
    // Layered mountains.
    s.ellipse(300.0, 820.0, 420.0, 320.0, "#0369A1", "Mountain back");
    s.ellipse(780.0, 860.0, 460.0, 300.0, "#075985", "Mountain back 2");
    s.ellipse(540.0, 900.0, 520.0, 300.0, "#0C4A6E", "Mountain front");
    s.rect(0.0, 760.0, POSTER_W, 6.0, "#FFFFFF", "Horizon");
    s.text_center(540.0, 120.0, 36.0, "#E0F2FE", "VISIT", "Eyebrow");
    s.text_center(540.0, 980.0, 150.0, "#FFFBEB", "ICELAND", "Title");
    s.text_center(540.0, 1160.0, 36.0, "#78350F", "Land of fire, ice, and endless sky", "Sub");
    BundledTemplate {
        dir_name: "poster-travel.ktemplate",
        manifest: manifest(
            81,
            "Travel Poster",
            "4:5 vintage-style travel poster with layered mountains, sun, and destination title.",
            TemplateCategory::Poster,
            &["poster", "travel", "destination", "tourism", "scenery"],
        ),
        content: s.finish(),
    }
}

fn poster_typographic() -> BundledTemplate {
    let mut s = Sheet::new(POSTER_W, POSTER_H);
    s.bg("#FDE047");
    s.rect(0.0, 0.0, POSTER_W, 120.0, "#0A0A0A", "Top bar");
    s.rect(0.0, POSTER_H - 120.0, POSTER_W, 120.0, "#0A0A0A", "Bottom bar");
    s.text(80.0, 70.0, 30.0, "#FDE047", "VOLUME 01 — THE MANIFESTO", "Top label");
    s.text(70.0, 320.0, 170.0, "#0A0A0A", "MAKE", "Title 1");
    s.text(70.0, 500.0, 170.0, "#0A0A0A", "GOOD", "Title 2");
    s.text(70.0, 680.0, 170.0, "#DC2626", "WORK", "Title 3");
    s.rect(80.0, 900.0, 400.0, 12.0, "#0A0A0A", "Rule");
    s.text(80.0, 960.0, 34.0, "#0A0A0A", "and share it with people", "Body 1");
    s.text(80.0, 1010.0, 34.0, "#0A0A0A", "who care about the craft.", "Body 2");
    s.text(80.0, POSTER_H - 92.0, 30.0, "#FDE047", "kcreate.app", "Bottom label");
    BundledTemplate {
        dir_name: "poster-typographic.ktemplate",
        manifest: manifest(
            82,
            "Typographic Poster",
            "4:5 bold typographic poster with framing bars and an oversized three-line statement.",
            TemplateCategory::Poster,
            &["poster", "typography", "minimal", "statement", "quote"],
        ),
        content: s.finish(),
    }
}

// ---------------------------------------------------------------------
// Catalog — flyers (1080 × 1350, 4:5 print)
// ---------------------------------------------------------------------

fn flyer_restaurant() -> BundledTemplate {
    let mut s = Sheet::new(POSTER_W, POSTER_H);
    s.bg("#FFFBEB");
    s.rect(0.0, 0.0, POSTER_W, 360.0, "#166534", "Header");
    s.ellipse_a(960.0, 60.0, 220.0, 220.0, "#FFFFFF", 0.12, "Glow");
    s.text(80.0, 90.0, 34.0, "#BBF7D0", "FARM-TO-TABLE", "Eyebrow");
    s.text(80.0, 160.0, 100.0, "#FFFFFF", "Lunch Menu", "Title");
    s.rect(80.0, 440.0, 920.0, 4.0, "#D9F99D", "Divider");
    let items = [
        ("Garden bowl", "Quinoa, greens, citrus", "$12"),
        ("Wild mushroom toast", "Sourdough, thyme oil", "$10"),
        ("Roast pumpkin soup", "Sage, toasted seeds", "$9"),
        ("Heirloom tomato salad", "Basil, burrata", "$13"),
    ];
    let mut y = 500.0;
    for (name, desc, price) in items {
        s.text(80.0, y, 40.0, "#14532D", name, "Item name");
        s.text(80.0, y + 52.0, 28.0, "#4D7C0F", desc, "Item desc");
        s.text(880.0, y, 40.0, "#166534", price, "Item price");
        y += 150.0;
    }
    s.rect(0.0, POSTER_H - 110.0, POSTER_W, 110.0, "#166534", "Footer");
    s.text_center(540.0, POSTER_H - 86.0, 30.0, "#DCFCE7", "Open daily 11–3 · The Greenhouse · 88 Vine St", "Footer text");
    BundledTemplate {
        dir_name: "flyer-restaurant.ktemplate",
        manifest: manifest(
            83,
            "Restaurant Menu Flyer",
            "4:5 menu flyer with a header, priced item list with descriptions, and a footer.",
            TemplateCategory::Flyer,
            &["flyer", "restaurant", "menu", "food", "cafe"],
        ),
        content: s.finish(),
    }
}

fn flyer_open_house() -> BundledTemplate {
    let mut s = Sheet::new(POSTER_W, POSTER_H);
    s.bg("#FFFFFF");
    s.rect(0.0, 0.0, POSTER_W, 70.0, "#0F766E", "Top rule");
    s.text_center(540.0, 150.0, 40.0, "#0F766E", "YOU'RE INVITED TO OUR", "Eyebrow");
    s.text_center(540.0, 230.0, 130.0, "#0F172A", "Open House", "Title");
    s.circle(540.0, 560.0, 200.0, "#CCFBF1", "Disc");
    s.text_center(540.0, 470.0, 36.0, "#0F766E", "SAVE THE DATE", "Disc top");
    s.text_center(540.0, 520.0, 110.0, "#0F766E", "09", "Disc num");
    s.text_center(540.0, 640.0, 40.0, "#0F766E", "AUGUST", "Disc month");
    s.rect(140.0, 850.0, 800.0, 3.0, "#99F6E4", "Divider");
    let rows = [("Time", "2:00 – 6:00 PM"), ("Place", "Northside Community Center"), ("RSVP", "hello@northside.org")];
    let mut y = 910.0;
    for (label, val) in rows {
        s.text_center(540.0 - 200.0, y, 30.0, "#0F766E", label, "Row label");
        s.text_center(540.0 + 120.0, y, 30.0, "#0F172A", val, "Row value");
        y += 80.0;
    }
    s.text_center(540.0, 1200.0, 30.0, "#64748B", "Refreshments provided · all welcome", "Footer");
    BundledTemplate {
        dir_name: "flyer-open-house.ktemplate",
        manifest: manifest(
            84,
            "Open House Flyer",
            "4:5 open-house invite with a date disc and time/place/RSVP detail rows.",
            TemplateCategory::Flyer,
            &["flyer", "open house", "invite", "event", "community"],
        ),
        content: s.finish(),
    }
}

fn flyer_fitness_class() -> BundledTemplate {
    let mut s = Sheet::new(POSTER_W, POSTER_H);
    s.bg("#1E1B4B");
    s.ellipse_a(180.0, 200.0, 300.0, 300.0, "#8B5CF6", 0.45, "Glow A");
    s.ellipse_a(920.0, 1180.0, 320.0, 320.0, "#EC4899", 0.40, "Glow B");
    s.text(80.0, 120.0, 36.0, "#C4B5FD", "WEEKLY SCHEDULE", "Eyebrow");
    s.text(80.0, 200.0, 120.0, "#FFFFFF", "Move", "Title 1");
    s.text(80.0, 330.0, 120.0, "#A78BFA", "& Flow", "Title 2");
    let rows = [("MON", "Power Yoga", "6:00 PM"), ("WED", "HIIT Burn", "7:00 PM"), ("FRI", "Pilates Core", "6:30 PM"), ("SAT", "Sunrise Flow", "8:00 AM")];
    let mut y = 560.0;
    for (day, name, time) in rows {
        s.rect(80.0, y, 920.0, 130.0, "#312E81", "Row card");
        s.rect(80.0, y, 120.0, 130.0, "#8B5CF6", "Day chip");
        s.text_center(140.0, y + 44.0, 34.0, "#FFFFFF", day, "Day");
        s.text(240.0, y + 30.0, 40.0, "#FFFFFF", name, "Class");
        s.text(800.0, y + 36.0, 34.0, "#C4B5FD", time, "Time");
        y += 160.0;
    }
    s.text_center(540.0, 1270.0, 28.0, "#C4B5FD", "Pulse Studio · drop-ins welcome", "Footer");
    BundledTemplate {
        dir_name: "flyer-fitness-class.ktemplate",
        manifest: manifest(
            85,
            "Fitness Class Schedule Flyer",
            "4:5 class-schedule flyer with day chips, class names, and times in a card list.",
            TemplateCategory::Flyer,
            &["flyer", "fitness", "schedule", "class", "gym"],
        ),
        content: s.finish(),
    }
}

fn flyer_grand_opening() -> BundledTemplate {
    let mut s = Sheet::new(POSTER_W, POSTER_H);
    s.bg("#B91C1C");
    s.ellipse_a(540.0, 470.0, 460.0, 460.0, "#FCA5A5", 0.20, "Glow");
    // Bunting dots.
    for i in 0..7 {
        s.circle(120.0 + f64::from(i) * 140.0, 120.0, 30.0, "#FBBF24", "Bunting");
    }
    s.text_center(540.0, 230.0, 40.0, "#FECACA", "WE'RE OPEN!", "Eyebrow");
    s.text_center(540.0, 320.0, 120.0, "#FFFFFF", "GRAND", "Title 1");
    s.text_center(540.0, 450.0, 120.0, "#FBBF24", "OPENING", "Title 2");
    s.circle(540.0, 760.0, 180.0, "#FFFFFF", "Offer disc");
    s.text_center(540.0, 660.0, 40.0, "#B91C1C", "FIRST 50", "Offer top");
    s.text_center(540.0, 710.0, 90.0, "#B91C1C", "FREE", "Offer mid");
    s.text_center(540.0, 820.0, 36.0, "#B91C1C", "GIFTS", "Offer bottom");
    s.text_center(540.0, 1040.0, 40.0, "#FFFFFF", "Saturday, June 21 · 10 AM", "Date");
    s.text_center(540.0, 1110.0, 32.0, "#FECACA", "240 High Street · Bloom & Co.", "Address");
    BundledTemplate {
        dir_name: "flyer-grand-opening.ktemplate",
        manifest: manifest(
            86,
            "Grand Opening Flyer",
            "4:5 celebratory grand-opening flyer with bunting, big type, and a free-gift offer disc.",
            TemplateCategory::Flyer,
            &["flyer", "grand opening", "sale", "event", "retail"],
        ),
        content: s.finish(),
    }
}

fn flyer_club_night() -> BundledTemplate {
    let mut s = Sheet::new(POSTER_W, POSTER_H);
    s.bg("#09090B");
    s.ellipse_a(360.0, 420.0, 320.0, 320.0, "#7C3AED", 0.55, "Glow A");
    s.ellipse_a(760.0, 560.0, 320.0, 320.0, "#06B6D4", 0.50, "Glow B");
    s.text(80.0, 110.0, 36.0, "#22D3EE", "SATURDAY NIGHTS PRESENT", "Eyebrow");
    s.text(70.0, 380.0, 160.0, "#FFFFFF", "PULSE", "Title 1");
    s.text(70.0, 560.0, 160.0, "#A78BFA", "AFTER", "Title 2");
    s.text(70.0, 740.0, 160.0, "#22D3EE", "DARK", "Title 3");
    s.rect(80.0, 980.0, 920.0, 3.0, "#3F3F46", "Divider");
    s.text(80.0, 1020.0, 36.0, "#FFFFFF", "DJ NOVA · DJ KIRA · LIVE SET", "Lineup");
    s.text(80.0, 1090.0, 30.0, "#A1A1AA", "Doors 10 PM · The Vault · 21+", "Details");
    s.rect(80.0, 1180.0, 920.0, 100.0, "#7C3AED", "CTA");
    s.text_center(540.0, 1206.0, 36.0, "#FFFFFF", "Tickets at the door · $20", "CTA label");
    BundledTemplate {
        dir_name: "flyer-club-night.ktemplate",
        manifest: manifest(
            87,
            "Club Night Flyer",
            "4:5 nightlife flyer with neon glows, stacked display type, lineup, and ticket CTA.",
            TemplateCategory::Flyer,
            &["flyer", "club", "party", "music", "nightlife"],
        ),
        content: s.finish(),
    }
}

fn flyer_community() -> BundledTemplate {
    let mut s = Sheet::new(POSTER_W, POSTER_H);
    s.bg("#FFFFFF");
    s.rect(0.0, 0.0, POSTER_W, 420.0, "#0369A1", "Header");
    s.circle(880.0, 120.0, 120.0, "#38BDF8", "Accent");
    s.text(80.0, 110.0, 36.0, "#BAE6FD", "EVERYONE WELCOME", "Eyebrow");
    s.text(80.0, 190.0, 96.0, "#FFFFFF", "Community", "Title 1");
    s.text(80.0, 300.0, 96.0, "#FACC15", "Cleanup Day", "Title 2");
    s.text(80.0, 500.0, 34.0, "#0F172A", "Join your neighbors for a morning of", "Body 1");
    s.text(80.0, 552.0, 34.0, "#0F172A", "tidying up Riverside Park together.", "Body 2");
    s.rect(80.0, 660.0, 920.0, 3.0, "#E2E8F0", "Divider");
    let rows = [("When", "Sun, Sep 14 · 9 AM – 12 PM"), ("Where", "Riverside Park, main gate"), ("Bring", "Gloves & water — bags provided")];
    let mut y = 720.0;
    for (label, val) in rows {
        s.circle(110.0, y + 18.0, 14.0, "#0369A1", "Dot");
        s.text(150.0, y, 30.0, "#0369A1", label, "Row label");
        s.text(340.0, y, 30.0, "#0F172A", val, "Row value");
        y += 90.0;
    }
    s.rect(80.0, 1180.0, 920.0, 100.0, "#0369A1", "CTA");
    s.text_center(540.0, 1206.0, 34.0, "#FFFFFF", "Register free at riverside.org/cleanup", "CTA label");
    BundledTemplate {
        dir_name: "flyer-community.ktemplate",
        manifest: manifest(
            88,
            "Community Event Flyer",
            "4:5 community event flyer with a friendly header, body copy, and detail rows.",
            TemplateCategory::Flyer,
            &["flyer", "community", "volunteer", "event", "local"],
        ),
        content: s.finish(),
    }
}

fn flyer_product_launch() -> BundledTemplate {
    let mut s = Sheet::new(POSTER_W, POSTER_H);
    s.bg("#0F172A");
    s.ellipse_a(540.0, 560.0, 420.0, 420.0, "#22D3EE", 0.30, "Glow");
    s.text_center(540.0, 120.0, 36.0, "#67E8F9", "INTRODUCING", "Eyebrow");
    // Device silhouette.
    s.rect(360.0, 300.0, 360.0, 520.0, "#1E293B", "Device");
    s.rect(390.0, 340.0, 300.0, 420.0, "#0EA5E9", "Screen");
    s.circle(540.0, 790.0, 20.0, "#475569", "Home");
    s.text_center(540.0, 900.0, 100.0, "#FFFFFF", "Aura One", "Title");
    s.text_center(540.0, 1020.0, 34.0, "#94A3B8", "The smartphone that disappears", "Sub");
    s.rect(290.0, 1130.0, 500.0, 100.0, "#22D3EE", "CTA");
    s.text_center(540.0, 1156.0, 36.0, "#0F172A", "Pre-order · aura.tech", "CTA label");
    BundledTemplate {
        dir_name: "flyer-product-launch.ktemplate",
        manifest: manifest(
            89,
            "Product Launch Flyer",
            "4:5 product launch flyer with a device hero, product name, tagline, and pre-order CTA.",
            TemplateCategory::Flyer,
            &["flyer", "product", "launch", "tech", "promo"],
        ),
        content: s.finish(),
    }
}

// ---------------------------------------------------------------------
// Catalog — résumés / CVs (A4, 1240 × 1754)
// ---------------------------------------------------------------------

fn resume_modern() -> BundledTemplate {
    let mut s = Sheet::new(A4_W, A4_H);
    s.bg("#FFFFFF");
    s.rect(0.0, 0.0, A4_W, 320.0, "#1E293B", "Header");
    s.rect(0.0, 320.0, A4_W, 10.0, "#6366F1", "Accent rule");
    s.text(80.0, 110.0, 76.0, "#FFFFFF", "Maya Chen", "Name");
    s.text(80.0, 210.0, 38.0, "#A5B4FC", "Marketing Manager", "Role");
    s.text(720.0, 120.0, 26.0, "#CBD5E1", "maya.chen@mail.com", "Contact 1");
    s.text(720.0, 165.0, 26.0, "#CBD5E1", "+1 555 0193", "Contact 2");
    s.text(720.0, 210.0, 26.0, "#CBD5E1", "Chicago, IL", "Contact 3");
    s.text(80.0, 390.0, 38.0, "#1E293B", "PROFILE", "Section 1");
    s.rect(80.0, 448.0, 120.0, 4.0, "#6366F1", "Rule 1");
    s.text(80.0, 480.0, 28.0, "#475569", "Data-driven marketer with 7 years growing", "Profile 1");
    s.text(80.0, 520.0, 28.0, "#475569", "brands through content and lifecycle campaigns.", "Profile 2");
    s.text(80.0, 620.0, 38.0, "#1E293B", "EXPERIENCE", "Section 2");
    s.rect(80.0, 678.0, 120.0, 4.0, "#6366F1", "Rule 2");
    let jobs = [
        ("Marketing Manager — Brightly", "2021–Present", "Scaled organic traffic 3× in 18 months."),
        ("Growth Lead — Hatch", "2018–2021", "Built the lifecycle email program from zero."),
        ("Marketing Associate — Verve", "2016–2018", "Ran paid social across four markets."),
    ];
    let mut jy = 720.0;
    for (title, dates, desc) in jobs {
        s.text(80.0, jy, 34.0, "#0F172A", title, "Job title");
        s.text(80.0, jy + 46.0, 26.0, "#6366F1", dates, "Job dates");
        s.text(80.0, jy + 88.0, 26.0, "#475569", desc, "Job desc");
        jy += 190.0;
    }
    s.text(80.0, jy + 10.0, 38.0, "#1E293B", "SKILLS", "Section 3");
    s.rect(80.0, jy + 68.0, 120.0, 4.0, "#6366F1", "Rule 3");
    let skills = ["SEO", "Email", "Analytics", "Brand", "Paid social"];
    let mut sx = 80.0;
    for skill in skills {
        let w = approx_text_width(skill, 26.0) + 60.0;
        s.rect(sx, jy + 100.0, w, 60.0, "#EEF2FF", "Skill chip");
        s.text(sx + 30.0, jy + 116.0, 26.0, "#4338CA", skill, "Skill");
        sx += w + 24.0;
    }
    BundledTemplate {
        dir_name: "resume-modern.ktemplate",
        manifest: manifest(
            90,
            "Modern Resume",
            "A4 single-column résumé with a dark header, profile, experience, and skill chips.",
            TemplateCategory::Resume,
            &["resume", "cv", "modern", "job", "career"],
        ),
        content: s.finish(),
    }
}

fn resume_minimal() -> BundledTemplate {
    let mut s = Sheet::new(A4_W, A4_H);
    s.bg("#FFFFFF");
    s.text(100.0, 140.0, 80.0, "#111827", "Daniel Park", "Name");
    s.text(100.0, 250.0, 34.0, "#6B7280", "Software Engineer", "Role");
    s.text(100.0, 310.0, 26.0, "#6B7280", "daniel@mail.com · +1 555 0110 · Seattle", "Contact");
    s.rect(100.0, 380.0, A4_W - 200.0, 2.0, "#E5E7EB", "Rule top");
    let sections = [
        ("EXPERIENCE", &["Senior Engineer — Cloudbase · 2020–Present", "Engineer — Datafold · 2017–2020"][..]),
        ("EDUCATION", &["B.S. Computer Science — UW · 2013–2017"][..]),
        ("SKILLS", &["Rust · TypeScript · Go · Postgres · AWS"][..]),
    ];
    let mut y = 440.0;
    for (head, lines) in sections {
        s.text(100.0, y, 32.0, "#111827", head, "Section head");
        s.rect(100.0, y + 46.0, 60.0, 3.0, "#111827", "Section tick");
        y += 90.0;
        for line in lines {
            s.text(100.0, y, 28.0, "#374151", line, "Section line");
            y += 56.0;
        }
        y += 50.0;
    }
    BundledTemplate {
        dir_name: "resume-minimal.ktemplate",
        manifest: manifest(
            91,
            "Minimal Resume",
            "A4 minimalist résumé with quiet type, hairline rules, and clearly labeled sections.",
            TemplateCategory::Resume,
            &["resume", "cv", "minimal", "clean", "career"],
        ),
        content: s.finish(),
    }
}

fn resume_creative() -> BundledTemplate {
    let mut s = Sheet::new(A4_W, A4_H);
    s.bg("#FFF7ED");
    s.ellipse_a(1080.0, 180.0, 320.0, 320.0, "#FDBA74", 0.45, "Glow A");
    s.ellipse_a(160.0, 1620.0, 320.0, 320.0, "#FCA5A5", 0.40, "Glow B");
    s.circle(220.0, 240.0, 120.0, "#FB923C", "Avatar");
    s.text(380.0, 150.0, 76.0, "#7C2D12", "Lola Vega", "Name");
    s.text(380.0, 250.0, 36.0, "#EA580C", "Illustrator & Art Director", "Role");
    s.rect(80.0, 420.0, A4_W - 160.0, 4.0, "#FED7AA", "Rule");
    s.text(80.0, 470.0, 36.0, "#7C2D12", "ABOUT", "Section 1");
    s.text(80.0, 530.0, 28.0, "#9A3412", "I make playful, bold visuals for brands", "About 1");
    s.text(80.0, 570.0, 28.0, "#9A3412", "that want to stand out and have fun.", "About 2");
    s.text(80.0, 680.0, 36.0, "#7C2D12", "SELECTED WORK", "Section 2");
    let work = [
        ("Sunday Mag — cover series", "2024"),
        ("Bloom Co — brand illustration", "2023"),
        ("Festival Norte — key art", "2022"),
    ];
    let mut wy = 750.0;
    for (title, year) in work {
        s.circle(100.0, wy + 16.0, 14.0, "#EA580C", "Bullet");
        s.text(140.0, wy, 32.0, "#7C2D12", title, "Work title");
        s.text(980.0, wy, 28.0, "#EA580C", year, "Work year");
        wy += 100.0;
    }
    s.text(80.0, wy + 30.0, 36.0, "#7C2D12", "TOOLS", "Section 3");
    let tools = ["Procreate", "Illustrator", "Blender", "Risograph"];
    let mut tx = 80.0;
    for tool in tools {
        let w = approx_text_width(tool, 26.0) + 56.0;
        s.rect(tx, wy + 100.0, w, 58.0, "#FB923C", "Tool chip");
        s.text(tx + 28.0, wy + 116.0, 26.0, "#FFFFFF", tool, "Tool");
        tx += w + 22.0;
    }
    BundledTemplate {
        dir_name: "resume-creative.ktemplate",
        manifest: manifest(
            92,
            "Creative Resume",
            "A4 colourful creative résumé with warm glows, avatar, selected work, and tool chips.",
            TemplateCategory::Resume,
            &["resume", "cv", "creative", "designer", "portfolio"],
        ),
        content: s.finish(),
    }
}

fn resume_executive() -> BundledTemplate {
    let mut s = Sheet::new(A4_W, A4_H);
    s.bg("#FFFFFF");
    s.rect(0.0, 0.0, A4_W, 260.0, "#0F172A", "Header");
    s.rect(0.0, 256.0, A4_W, 6.0, "#B45309", "Accent");
    s.text_center(A4_W / 2.0, 90.0, 70.0, "#FFFFFF", "RICHARD HALE", "Name");
    s.text_center(A4_W / 2.0, 180.0, 32.0, "#FCD34D", "Chief Operating Officer", "Role");
    s.text(80.0, 320.0, 36.0, "#0F172A", "EXECUTIVE SUMMARY", "Section 1");
    s.rect(80.0, 378.0, A4_W - 160.0, 3.0, "#E2E8F0", "Rule 1");
    s.text(80.0, 410.0, 28.0, "#334155", "Operations leader with 15+ years scaling", "Summary 1");
    s.text(80.0, 450.0, 28.0, "#334155", "global teams and double-digit margin growth.", "Summary 2");
    s.text(80.0, 560.0, 36.0, "#0F172A", "KEY ACHIEVEMENTS", "Section 2");
    s.rect(80.0, 618.0, A4_W - 160.0, 3.0, "#E2E8F0", "Rule 2");
    let stats = [("+42%", "Revenue growth"), ("3", "Markets launched"), ("$120M", "P&L managed")];
    for (i, (num, label)) in stats.iter().enumerate() {
        let x = 80.0 + i as f64 * 370.0;
        s.rect(x, 660.0, 340.0, 180.0, "#F1F5F9", "Stat card");
        s.rect(x, 660.0, 340.0, 8.0, "#B45309", "Stat top");
        s.text_center(x + 170.0, 700.0, 64.0, "#0F172A", num, "Stat num");
        s.text_center(x + 170.0, 790.0, 26.0, "#64748B", label, "Stat label");
    }
    s.text(80.0, 900.0, 36.0, "#0F172A", "EXPERIENCE", "Section 3");
    s.rect(80.0, 958.0, A4_W - 160.0, 3.0, "#E2E8F0", "Rule 3");
    let jobs = [
        ("COO — Meridian Group", "2018–Present"),
        ("VP Operations — Apex", "2013–2018"),
        ("Director — Northgate", "2009–2013"),
    ];
    let mut jy = 1000.0;
    for (title, dates) in jobs {
        s.text(80.0, jy, 34.0, "#0F172A", title, "Job title");
        s.text(900.0, jy, 30.0, "#B45309", dates, "Job dates");
        s.rect(80.0, jy + 60.0, A4_W - 160.0, 2.0, "#F1F5F9", "Job rule");
        jy += 120.0;
    }
    BundledTemplate {
        dir_name: "resume-executive.ktemplate",
        manifest: manifest(
            93,
            "Executive Resume",
            "A4 executive résumé with a centred header, achievement stat cards, and an experience list.",
            TemplateCategory::Resume,
            &["resume", "cv", "executive", "leadership", "career"],
        ),
        content: s.finish(),
    }
}

fn resume_developer() -> BundledTemplate {
    let mut s = Sheet::new(A4_W, A4_H);
    s.bg("#0F172A");
    s.rect(0.0, 0.0, A4_W, A4_H, "#0F172A", "Bg fill");
    s.text(80.0, 110.0, 70.0, "#E2E8F0", "alex.dev", "Name");
    s.text(80.0, 210.0, 32.0, "#38BDF8", "// Full-Stack Engineer", "Role");
    s.rect(80.0, 280.0, A4_W - 160.0, 2.0, "#1E293B", "Rule top");
    s.text(80.0, 320.0, 28.0, "#64748B", "alex@dev.io · github.com/alexdev · Remote", "Contact");
    s.text(80.0, 420.0, 34.0, "#38BDF8", "$ experience", "Section 1");
    let jobs = [
        ("Senior Engineer — Vercel", "2021–Now", "Edge runtime + DX tooling."),
        ("Engineer — Supabase", "2019–2021", "Realtime + Postgres replication."),
        ("Engineer — Indie", "2017–2019", "Shipped 6 products solo."),
    ];
    let mut jy = 490.0;
    for (title, dates, desc) in jobs {
        s.text(80.0, jy, 32.0, "#E2E8F0", title, "Job title");
        s.text(900.0, jy, 26.0, "#38BDF8", dates, "Job dates");
        s.text(80.0, jy + 46.0, 26.0, "#94A3B8", desc, "Job desc");
        jy += 150.0;
    }
    s.text(80.0, jy + 10.0, 34.0, "#38BDF8", "$ stack", "Section 2");
    let stack = ["TypeScript", "Rust", "React", "Node", "Postgres", "Docker", "AWS"];
    let mut tx = 80.0;
    let mut ty = jy + 80.0;
    for tool in stack {
        let w = approx_text_width(tool, 26.0) + 52.0;
        if tx + w > A4_W - 80.0 {
            tx = 80.0;
            ty += 80.0;
        }
        s.rect(tx, ty, w, 60.0, "#1E293B", "Stack chip");
        s.rect(tx, ty, 6.0, 60.0, "#38BDF8", "Chip accent");
        s.text(tx + 26.0, ty + 16.0, 26.0, "#E2E8F0", tool, "Stack item");
        tx += w + 20.0;
    }
    BundledTemplate {
        dir_name: "resume-developer.ktemplate",
        manifest: manifest(
            94,
            "Developer Resume",
            "A4 dark developer résumé with a code-style accent, experience block, and a tech-stack chip grid.",
            TemplateCategory::Resume,
            &["resume", "cv", "developer", "engineer", "tech"],
        ),
        content: s.finish(),
    }
}

// ---------------------------------------------------------------------
// Catalog — business reports (A4, 1240 × 1754)
// ---------------------------------------------------------------------

fn report_cover_minimal() -> BundledTemplate {
    let mut s = Sheet::new(A4_W, A4_H);
    s.bg("#FFFFFF");
    s.rect(0.0, 0.0, 120.0, A4_H, "#111827", "Spine");
    s.text(220.0, 220.0, 32.0, "#6B7280", "ANNUAL REPORT", "Eyebrow");
    s.text(220.0, 300.0, 120.0, "#111827", "2025", "Year");
    s.rect(220.0, 470.0, 300.0, 8.0, "#111827", "Rule");
    s.text(220.0, 540.0, 56.0, "#111827", "Building for", "Title 1");
    s.text(220.0, 620.0, 56.0, "#111827", "the long term", "Title 2");
    s.text(220.0, 1480.0, 30.0, "#6B7280", "Northwind Industries, Inc.", "Company");
    s.text(220.0, 1530.0, 26.0, "#9CA3AF", "Prepared for shareholders · March 2026", "Meta");
    BundledTemplate {
        dir_name: "report-cover-minimal.ktemplate",
        manifest: manifest(
            95,
            "Minimal Report Cover",
            "A4 minimalist annual-report cover with a spine, big year, and title.",
            TemplateCategory::Report,
            &["report", "cover", "annual", "minimal", "business"],
        ),
        content: s.finish(),
    }
}

fn report_exec_summary() -> BundledTemplate {
    let mut s = Sheet::new(A4_W, A4_H);
    s.bg("#FFFFFF");
    s.rect(0.0, 0.0, A4_W, 180.0, "#1D4ED8", "Header");
    s.text(80.0, 60.0, 48.0, "#FFFFFF", "Executive Summary", "Header title");
    s.text(80.0, 230.0, 28.0, "#334155", "This report reviews fiscal year performance across", "Intro 1");
    s.text(80.0, 270.0, 28.0, "#334155", "revenue, operations, and product, and outlines the", "Intro 2");
    s.text(80.0, 310.0, 28.0, "#334155", "strategic priorities for the year ahead.", "Intro 3");
    let stats = [("$48.2M", "Total revenue", "#1D4ED8"), ("+18%", "YoY growth", "#0891B2"), ("92%", "Retention", "#059669")];
    for (i, (num, label, hex)) in stats.iter().enumerate() {
        let x = 80.0 + i as f64 * 370.0;
        s.rect(x, 400.0, 340.0, 200.0, "#F1F5F9", "Stat card");
        s.rect(x, 400.0, 340.0, 8.0, hex, "Stat top");
        s.text_center(x + 170.0, 450.0, 64.0, hex, num, "Stat num");
        s.text_center(x + 170.0, 540.0, 26.0, "#475569", label, "Stat label");
    }
    s.text(80.0, 680.0, 36.0, "#0F172A", "Key takeaways", "Section");
    s.rect(80.0, 738.0, 120.0, 4.0, "#1D4ED8", "Rule");
    let points = [
        "Revenue grew across all three regions, led by EMEA.",
        "Gross margin improved 4 points on supply-chain wins.",
        "The new platform now drives 40% of total bookings.",
        "Headcount efficiency rose with flat operating costs.",
    ];
    let mut y = 790.0;
    for point in points {
        s.circle(100.0, y + 16.0, 12.0, "#1D4ED8", "Bullet");
        s.text(140.0, y, 28.0, "#334155", point, "Point");
        y += 80.0;
    }
    BundledTemplate {
        dir_name: "report-exec-summary.ktemplate",
        manifest: manifest(
            96,
            "Report — Executive Summary",
            "A4 executive-summary page with intro copy, KPI stat cards, and key-takeaway bullets.",
            TemplateCategory::Report,
            &["report", "summary", "executive", "kpi", "business"],
        ),
        content: s.finish(),
    }
}

fn report_data_page() -> BundledTemplate {
    let mut s = Sheet::new(A4_W, A4_H);
    s.bg("#FFFFFF");
    s.text(80.0, 90.0, 44.0, "#0F172A", "Quarterly Revenue", "Title");
    s.text(80.0, 160.0, 28.0, "#64748B", "Revenue by quarter ($M), FY2025", "Subtitle");
    // Bar chart.
    s.rect(80.0, 280.0, 4.0, 560.0, "#CBD5E1", "Y axis");
    s.rect(80.0, 836.0, 1000.0, 4.0, "#CBD5E1", "X axis");
    let bars = [("Q1", 0.55, "#1D4ED8"), ("Q2", 0.68, "#1D4ED8"), ("Q3", 0.82, "#1D4ED8"), ("Q4", 0.97, "#0891B2")];
    for (i, (label, frac, hex)) in bars.iter().enumerate() {
        let x = 180.0 + i as f64 * 220.0;
        let h = 540.0 * frac;
        s.rect(x, 836.0 - h, 150.0, h, hex, "Bar");
        s.text_center(x + 75.0, 836.0 - h - 44.0, 30.0, "#0F172A", &format!("{:.0}", frac * 14.0), "Bar value");
        s.text_center(x + 75.0, 860.0, 28.0, "#475569", label, "Bar label");
    }
    // Data table.
    s.text(80.0, 960.0, 36.0, "#0F172A", "Breakdown by region", "Table title");
    let rows = [("North America", "$18.4M", "+12%"), ("EMEA", "$15.1M", "+24%"), ("APAC", "$9.8M", "+19%"), ("LATAM", "$4.9M", "+8%")];
    s.rect(80.0, 1020.0, 1000.0, 64.0, "#1D4ED8", "Table head");
    s.text(110.0, 1036.0, 28.0, "#FFFFFF", "Region", "Th 1");
    s.text(620.0, 1036.0, 28.0, "#FFFFFF", "Revenue", "Th 2");
    s.text(900.0, 1036.0, 28.0, "#FFFFFF", "Growth", "Th 3");
    let mut ry = 1084.0;
    for (i, (region, rev, growth)) in rows.iter().enumerate() {
        let bg = if i % 2 == 0 { "#F8FAFC" } else { "#FFFFFF" };
        s.rect(80.0, ry, 1000.0, 64.0, bg, "Row");
        s.text(110.0, ry + 16.0, 28.0, "#0F172A", region, "Cell region");
        s.text(620.0, ry + 16.0, 28.0, "#334155", rev, "Cell rev");
        s.text(900.0, ry + 16.0, 28.0, "#059669", growth, "Cell growth");
        ry += 64.0;
    }
    BundledTemplate {
        dir_name: "report-data-page.ktemplate",
        manifest: manifest(
            97,
            "Report — Data Page",
            "A4 data page with a labeled bar chart and a striped region breakdown table.",
            TemplateCategory::Report,
            &["report", "data", "chart", "table", "analytics"],
        ),
        content: s.finish(),
    }
}

fn report_section() -> BundledTemplate {
    let mut s = Sheet::new(A4_W, A4_H);
    s.bg("#0F172A");
    s.ellipse_a(1100.0, 1500.0, 420.0, 420.0, "#1D4ED8", 0.35, "Glow");
    s.text(80.0, 760.0, 240.0, "#1E293B", "02", "Big number");
    s.text(80.0, 820.0, 36.0, "#60A5FA", "SECTION TWO", "Eyebrow");
    s.text(80.0, 900.0, 72.0, "#FFFFFF", "Operations", "Title");
    s.text(80.0, 1010.0, 30.0, "#94A3B8", "How we scaled delivery while holding costs flat.", "Sub");
    s.rect(80.0, 1100.0, 300.0, 6.0, "#1D4ED8", "Rule");
    BundledTemplate {
        dir_name: "report-section.ktemplate",
        manifest: manifest(
            98,
            "Report — Section Divider",
            "A4 dark section-divider page with an oversized section number and title.",
            TemplateCategory::Report,
            &["report", "section", "divider", "chapter", "business"],
        ),
        content: s.finish(),
    }
}

fn report_financials() -> BundledTemplate {
    let mut s = Sheet::new(A4_W, A4_H);
    s.bg("#FFFFFF");
    s.rect(0.0, 0.0, A4_W, 160.0, "#064E3B", "Header");
    s.text(80.0, 50.0, 44.0, "#FFFFFF", "Financial Statements", "Header title");
    s.text(80.0, 220.0, 32.0, "#0F172A", "Income statement ($000s)", "Table title");
    let rows = [
        ("Revenue", "48,210", "40,860"),
        ("Cost of revenue", "(18,940)", "(17,120)"),
        ("Gross profit", "29,270", "23,740"),
        ("Operating expenses", "(19,510)", "(17,980)"),
        ("Operating income", "9,760", "5,760"),
        ("Net income", "7,420", "4,180"),
    ];
    s.rect(80.0, 290.0, 1000.0, 60.0, "#064E3B", "Table head");
    s.text(110.0, 304.0, 28.0, "#FFFFFF", "Line item", "Th 1");
    s.text(700.0, 304.0, 28.0, "#FFFFFF", "FY25", "Th 2");
    s.text(920.0, 304.0, 28.0, "#FFFFFF", "FY24", "Th 3");
    let mut ry = 350.0;
    for (i, (item, cur, prev)) in rows.iter().enumerate() {
        let emphasize = *item == "Gross profit" || *item == "Net income";
        let bg = if emphasize {
            "#ECFDF5"
        } else if i % 2 == 0 {
            "#F8FAFC"
        } else {
            "#FFFFFF"
        };
        s.rect(80.0, ry, 1000.0, 60.0, bg, "Row");
        let ink = if emphasize { "#064E3B" } else { "#0F172A" };
        s.text(110.0, ry + 14.0, 28.0, ink, item, "Cell item");
        s.text(700.0, ry + 14.0, 28.0, ink, cur, "Cell cur");
        s.text(920.0, ry + 14.0, 28.0, "#475569", prev, "Cell prev");
        ry += 60.0;
    }
    s.text(80.0, ry + 30.0, 24.0, "#94A3B8", "Figures unaudited · prepared under IFRS", "Footnote");
    BundledTemplate {
        dir_name: "report-financials.ktemplate",
        manifest: manifest(
            99,
            "Report — Financials",
            "A4 income-statement page with a two-year comparison table and emphasized subtotals.",
            TemplateCategory::Report,
            &["report", "financials", "income", "table", "accounting"],
        ),
        content: s.finish(),
    }
}

// ---------------------------------------------------------------------
// Catalog — brochures (A4, 1240 × 1754)
// ---------------------------------------------------------------------

fn brochure_trifold() -> BundledTemplate {
    let mut s = Sheet::new(A4_W, A4_H);
    s.bg("#F8FAFC");
    let panel = A4_W / 3.0;
    // Three panels with fold guides.
    s.rect(panel - 2.0, 0.0, 4.0, A4_H, "#E2E8F0", "Fold 1");
    s.rect(panel * 2.0 - 2.0, 0.0, 4.0, A4_H, "#E2E8F0", "Fold 2");
    // Panel 1 (cover).
    s.rect(0.0, 0.0, panel, A4_H, "#0EA5E9", "Cover panel");
    s.circle(panel / 2.0, 620.0, 130.0, "#7DD3FC", "Cover mark");
    s.text_center(panel / 2.0, 540.0, 30.0, "#E0F2FE", "WELCOME TO", "Cover eyebrow");
    s.text_center(panel / 2.0, 820.0, 56.0, "#FFFFFF", "Lakeside", "Cover title 1");
    s.text_center(panel / 2.0, 890.0, 56.0, "#FFFFFF", "Clinic", "Cover title 2");
    s.text_center(panel / 2.0, 1010.0, 26.0, "#BAE6FD", "Your health, our priority", "Cover sub");
    // Panel 2.
    s.text(panel + 50.0, 120.0, 36.0, "#0F172A", "About us", "P2 title");
    s.rect(panel + 50.0, 178.0, 100.0, 4.0, "#0EA5E9", "P2 rule");
    let p2 = ["We provide compassionate", "primary care for the whole", "family, close to home and", "open seven days a week."];
    let mut y = 220.0;
    for line in p2 {
        s.text(panel + 50.0, y, 26.0, "#475569", line, "P2 line");
        y += 46.0;
    }
    s.text(panel + 50.0, 520.0, 36.0, "#0F172A", "Services", "P2 title 2");
    s.rect(panel + 50.0, 578.0, 100.0, 4.0, "#0EA5E9", "P2 rule 2");
    let svc = ["General checkups", "Pediatrics", "Vaccinations", "Lab & diagnostics"];
    let mut sy = 620.0;
    for item in svc {
        s.circle(panel + 64.0, sy + 14.0, 10.0, "#0EA5E9", "Dot");
        s.text(panel + 96.0, sy, 26.0, "#334155", item, "Svc");
        sy += 60.0;
    }
    // Panel 3 (contact).
    s.rect(panel * 2.0, 0.0, panel, A4_H, "#0F172A", "Contact panel");
    s.text(panel * 2.0 + 50.0, 120.0, 36.0, "#FFFFFF", "Visit us", "P3 title");
    s.rect(panel * 2.0 + 50.0, 178.0, 100.0, 4.0, "#0EA5E9", "P3 rule");
    let p3 = ["12 Lakeside Avenue", "Open Mon–Sun, 8–8", "(555) 010-3322", "lakesideclinic.org"];
    let mut py = 230.0;
    for line in p3 {
        s.text(panel * 2.0 + 50.0, py, 26.0, "#CBD5E1", line, "P3 line");
        py += 56.0;
    }
    BundledTemplate {
        dir_name: "brochure-trifold.ktemplate",
        manifest: manifest(
            100,
            "Tri-Fold Brochure",
            "A4 tri-fold brochure with fold guides, a cover panel, services list, and contact panel.",
            TemplateCategory::Brochure,
            &["brochure", "tri-fold", "leaflet", "clinic", "print"],
        ),
        content: s.finish(),
    }
}

fn brochure_real_estate() -> BundledTemplate {
    let mut s = Sheet::new(A4_W, A4_H);
    s.bg("#FFFFFF");
    s.rect(0.0, 0.0, A4_W, 620.0, "#1C1917", "Hero photo");
    s.ellipse_a(980.0, 120.0, 280.0, 280.0, "#A8A29E", 0.25, "Glow");
    s.rect(360.0, 320.0, 520.0, 240.0, "#E7E5E4", "House");
    s.rect(360.0, 240.0, 300.0, 120.0, "#D6D3D1", "Roof");
    s.rect(80.0, 60.0, 220.0, 64.0, "#B45309", "Badge");
    s.text(105.0, 76.0, 30.0, "#FFFFFF", "JUST LISTED", "Badge text");
    s.text(80.0, 690.0, 64.0, "#1C1917", "The Aspen House", "Title");
    s.text(80.0, 780.0, 34.0, "#B45309", "$1,250,000 · 5 bd · 4 ba · 3,800 sqft", "Specs");
    s.rect(80.0, 850.0, A4_W - 160.0, 3.0, "#E7E5E4", "Rule");
    let features = ["Chef's kitchen with marble island", "Primary suite with spa bath", "Heated saltwater pool", "Three-car garage & workshop"];
    let mut y = 900.0;
    for f in features {
        s.circle(100.0, y + 14.0, 10.0, "#B45309", "Dot");
        s.text(140.0, y, 28.0, "#44403C", f, "Feature");
        y += 70.0;
    }
    s.rect(80.0, 1500.0, A4_W - 160.0, 120.0, "#1C1917", "CTA");
    s.text_center(A4_W / 2.0, 1536.0, 32.0, "#FFFFFF", "Schedule a tour · Harper & Co · (555) 010-7788", "CTA label");
    BundledTemplate {
        dir_name: "brochure-real-estate.ktemplate",
        manifest: manifest(
            101,
            "Real Estate Brochure",
            "A4 property brochure with a hero photo, price/specs, feature list, and agent CTA.",
            TemplateCategory::Brochure,
            &["brochure", "real estate", "property", "listing", "print"],
        ),
        content: s.finish(),
    }
}

fn brochure_travel() -> BundledTemplate {
    let mut s = Sheet::new(A4_W, A4_H);
    s.bg("#FEF3C7");
    s.rect(0.0, 0.0, A4_W, 560.0, "#0E7490", "Sky");
    s.circle(960.0, 180.0, 110.0, "#FDE68A", "Sun");
    s.ellipse(360.0, 620.0, 420.0, 240.0, "#155E75", "Hill back");
    s.ellipse(900.0, 660.0, 460.0, 240.0, "#0C4A6E", "Hill front");
    s.text(80.0, 110.0, 36.0, "#A5F3FC", "ESCAPE TO", "Eyebrow");
    s.text(80.0, 200.0, 96.0, "#FFFFFF", "The Coast", "Title");
    s.text(80.0, 720.0, 32.0, "#155E75", "Three days of sun, sea, and slow mornings.", "Intro");
    let pkgs = [("Weekend escape", "2 nights · from $320"), ("Coastal week", "5 nights · from $720"), ("Island hop", "7 nights · from $1,100")];
    let mut y = 820.0;
    for (name, price) in pkgs {
        s.rect(80.0, y, A4_W - 160.0, 150.0, "#FFFFFF", "Pkg card");
        s.rect(80.0, y, 14.0, 150.0, "#0E7490", "Pkg accent");
        s.text(130.0, y + 30.0, 40.0, "#0C4A6E", name, "Pkg name");
        s.text(130.0, y + 86.0, 28.0, "#0891B2", price, "Pkg price");
        y += 180.0;
    }
    s.text_center(A4_W / 2.0, 1560.0, 28.0, "#155E75", "Book at coastlinetours.com · 0800 555 010", "Footer");
    BundledTemplate {
        dir_name: "brochure-travel.ktemplate",
        manifest: manifest(
            102,
            "Travel Brochure",
            "A4 travel brochure with a coastal hero, intro line, and three package cards.",
            TemplateCategory::Brochure,
            &["brochure", "travel", "tourism", "packages", "vacation"],
        ),
        content: s.finish(),
    }
}

fn brochure_product() -> BundledTemplate {
    let mut s = Sheet::new(A4_W, A4_H);
    s.bg("#FFFFFF");
    s.rect(0.0, 0.0, A4_W, 60.0, "#4F46E5", "Top rule");
    s.text(80.0, 120.0, 36.0, "#4F46E5", "PRODUCT GUIDE", "Eyebrow");
    s.text(80.0, 190.0, 80.0, "#0F172A", "Meet Halo", "Title");
    s.text(80.0, 300.0, 30.0, "#475569", "The smart desk lamp that adapts to you.", "Sub");
    // Product render.
    s.rect(820.0, 140.0, 340.0, 360.0, "#EEF2FF", "Product panel");
    s.rect(960.0, 200.0, 24.0, 240.0, "#4F46E5", "Lamp stem");
    s.ellipse(990.0, 220.0, 120.0, 40.0, "#A5B4FC", "Lamp head");
    s.rect(900.0, 460.0, 160.0, 24.0, "#4338CA", "Lamp base");
    let feats = [
        ("Adaptive light", "Tunes warmth to the time of day."),
        ("Touch dimming", "Slide to set the perfect level."),
        ("Focus timer", "Gentle pulses keep you on track."),
        ("USB-C charging", "Power your devices from the base."),
    ];
    let mut y = 620.0;
    for (i, (title, desc)) in feats.iter().enumerate() {
        let x = if i % 2 == 0 { 80.0 } else { 640.0 };
        if i % 2 == 0 && i > 0 {
            y += 260.0;
        }
        s.circle(x + 28.0, y + 28.0, 28.0, "#4F46E5", "Icon");
        s.text(x + 80.0, y, 34.0, "#0F172A", title, "Feat title");
        s.text(x + 80.0, y + 52.0, 26.0, "#64748B", desc, "Feat desc");
    }
    s.rect(80.0, 1540.0, A4_W - 160.0, 110.0, "#4F46E5", "CTA");
    s.text_center(A4_W / 2.0, 1572.0, 32.0, "#FFFFFF", "Available now · $129 · halo.design", "CTA label");
    BundledTemplate {
        dir_name: "brochure-product.ktemplate",
        manifest: manifest(
            103,
            "Product Brochure",
            "A4 product brochure with a hero render, a 2×2 feature grid, and a buy CTA.",
            TemplateCategory::Brochure,
            &["brochure", "product", "guide", "features", "tech"],
        ),
        content: s.finish(),
    }
}

// ---------------------------------------------------------------------
// Catalog — proposals (A4, 1240 × 1754)
// ---------------------------------------------------------------------

fn proposal_cover_light() -> BundledTemplate {
    let mut s = Sheet::new(A4_W, A4_H);
    s.bg("#FFFFFF");
    s.rect(0.0, 0.0, A4_W, 18.0, "#7C3AED", "Top rule");
    s.ellipse_a(1120.0, 1640.0, 360.0, 360.0, "#DDD6FE", 0.5, "Glow");
    s.text(80.0, 220.0, 32.0, "#7C3AED", "PROJECT PROPOSAL", "Eyebrow");
    s.rect(80.0, 290.0, 160.0, 6.0, "#7C3AED", "Rule");
    s.text(80.0, 360.0, 96.0, "#0F172A", "Website", "Title 1");
    s.text(80.0, 470.0, 96.0, "#0F172A", "Redesign", "Title 2");
    s.text(80.0, 620.0, 32.0, "#64748B", "A proposal for a faster, on-brand web presence.", "Sub");
    s.rect(80.0, 1420.0, A4_W - 160.0, 2.0, "#E2E8F0", "Divider");
    s.text(80.0, 1460.0, 28.0, "#0F172A", "Prepared for: Northwind Co.", "Meta 1");
    s.text(80.0, 1505.0, 28.0, "#64748B", "Prepared by: Studio Atlas · June 2026", "Meta 2");
    BundledTemplate {
        dir_name: "proposal-cover-light.ktemplate",
        manifest: manifest(
            104,
            "Proposal Cover",
            "A4 clean proposal cover with a violet accent, big title, and prepared-for/by meta.",
            TemplateCategory::Proposal,
            &["proposal", "cover", "project", "pitch", "business"],
        ),
        content: s.finish(),
    }
}

fn proposal_scope() -> BundledTemplate {
    let mut s = Sheet::new(A4_W, A4_H);
    s.bg("#FFFFFF");
    s.text(80.0, 100.0, 48.0, "#0F172A", "Scope of Work", "Title");
    s.rect(80.0, 168.0, 140.0, 6.0, "#7C3AED", "Rule");
    let phases = [
        ("01", "Discovery", "Stakeholder interviews, audit, and goals."),
        ("02", "Design", "Wireframes, visual design, and prototypes."),
        ("03", "Build", "Front-end build, CMS, and integrations."),
        ("04", "Launch", "QA, migration, training, and go-live."),
    ];
    let mut y = 240.0;
    for (num, title, desc) in phases {
        s.rect(80.0, y, A4_W - 160.0, 240.0, "#F8FAFC", "Phase card");
        s.rect(80.0, y, 10.0, 240.0, "#7C3AED", "Phase accent");
        s.text(130.0, y + 40.0, 80.0, "#DDD6FE", num, "Phase num");
        s.text(320.0, y + 50.0, 44.0, "#0F172A", title, "Phase title");
        s.text(320.0, y + 120.0, 28.0, "#64748B", desc, "Phase desc");
        y += 280.0;
    }
    BundledTemplate {
        dir_name: "proposal-scope.ktemplate",
        manifest: manifest(
            105,
            "Proposal — Scope of Work",
            "A4 scope page with four numbered phase cards describing the engagement.",
            TemplateCategory::Proposal,
            &["proposal", "scope", "phases", "timeline", "plan"],
        ),
        content: s.finish(),
    }
}

fn proposal_pricing() -> BundledTemplate {
    let mut s = Sheet::new(A4_W, A4_H);
    s.bg("#FFFFFF");
    s.text(80.0, 100.0, 48.0, "#0F172A", "Investment", "Title");
    s.rect(80.0, 168.0, 140.0, 6.0, "#7C3AED", "Rule");
    let tiers = [
        ("Starter", "$4,800", "#F8FAFC", "#0F172A", "5-page site · 4 weeks"),
        ("Growth", "$9,600", "#7C3AED", "#FFFFFF", "12-page site · CMS · 8 weeks"),
        ("Scale", "$18,000", "#F8FAFC", "#0F172A", "Custom build · 12 weeks"),
    ];
    for (i, (name, price, bg, ink, desc)) in tiers.iter().enumerate() {
        let x = 80.0 + i as f64 * 370.0;
        s.rect(x, 260.0, 340.0, 520.0, bg, "Tier card");
        if i == 1 {
            s.rect(x, 260.0, 340.0, 12.0, "#5B21B6", "Tier flag");
            s.text_center(x + 170.0, 300.0, 24.0, "#DDD6FE", "MOST POPULAR", "Popular");
        }
        s.text_center(x + 170.0, 360.0, 40.0, ink, name, "Tier name");
        s.text_center(x + 170.0, 440.0, 72.0, ink, price, "Tier price");
        s.rect(x + 60.0, 560.0, 220.0, 2.0, if i == 1 { "#A78BFA" } else { "#E2E8F0" }, "Tier rule");
        s.text_center(x + 170.0, 600.0, 24.0, if i == 1 { "#EDE9FE" } else { "#64748B" }, desc, "Tier desc");
    }
    s.text(80.0, 860.0, 28.0, "#64748B", "All tiers include hosting setup and a 30-day support window.", "Footnote");
    BundledTemplate {
        dir_name: "proposal-pricing.ktemplate",
        manifest: manifest(
            106,
            "Proposal — Pricing",
            "A4 pricing page with three tier cards and a highlighted recommended plan.",
            TemplateCategory::Proposal,
            &["proposal", "pricing", "tiers", "investment", "quote"],
        ),
        content: s.finish(),
    }
}

fn proposal_about() -> BundledTemplate {
    let mut s = Sheet::new(A4_W, A4_H);
    s.bg("#0F172A");
    s.ellipse_a(160.0, 160.0, 320.0, 320.0, "#7C3AED", 0.4, "Glow");
    s.text(80.0, 120.0, 48.0, "#FFFFFF", "About Studio Atlas", "Title");
    s.rect(80.0, 188.0, 140.0, 6.0, "#A78BFA", "Rule");
    s.text(80.0, 240.0, 28.0, "#CBD5E1", "We're a small team of designers and engineers", "Body 1");
    s.text(80.0, 280.0, 28.0, "#CBD5E1", "building thoughtful digital products since 2014.", "Body 2");
    let stats = [("120+", "Projects shipped"), ("12", "Team members"), ("4.9", "Avg. client rating")];
    for (i, (num, label)) in stats.iter().enumerate() {
        let x = 80.0 + i as f64 * 370.0;
        s.rect(x, 380.0, 340.0, 200.0, "#1E293B", "Stat card");
        s.text_center(x + 170.0, 420.0, 64.0, "#A78BFA", num, "Stat num");
        s.text_center(x + 170.0, 510.0, 26.0, "#94A3B8", label, "Stat label");
    }
    s.text(80.0, 660.0, 36.0, "#FFFFFF", "Selected clients", "Section");
    let clients = ["Northwind", "Lumen", "Bloom & Co", "Vega Labs", "Harbor"];
    let mut cx = 80.0;
    for c in clients {
        let w = approx_text_width(c, 28.0) + 60.0;
        s.rect(cx, 720.0, w, 70.0, "#1E293B", "Client chip");
        s.text(cx + 30.0, 738.0, 28.0, "#E2E8F0", c, "Client");
        cx += w + 24.0;
    }
    BundledTemplate {
        dir_name: "proposal-about.ktemplate",
        manifest: manifest(
            107,
            "Proposal — About",
            "A4 dark about-us page with intro copy, credibility stat cards, and client chips.",
            TemplateCategory::Proposal,
            &["proposal", "about", "agency", "team", "credibility"],
        ),
        content: s.finish(),
    }
}

// ---------------------------------------------------------------------
// Catalog — custom / misc formats (sizes inline)
// ---------------------------------------------------------------------

fn custom_business_card() -> BundledTemplate {
    let mut s = Sheet::new(1050.0, 600.0);
    s.bg("#0F172A");
    s.ellipse_a(980.0, 540.0, 260.0, 260.0, "#6366F1", 0.4, "Glow");
    s.rect(0.0, 0.0, 18.0, 600.0, "#6366F1", "Edge accent");
    s.circle(120.0, 130.0, 48.0, "#6366F1", "Logo mark");
    s.text(190.0, 108.0, 42.0, "#FFFFFF", "Atlas", "Brand");
    s.text(80.0, 280.0, 52.0, "#FFFFFF", "Jordan Rivera", "Name");
    s.text(80.0, 350.0, 30.0, "#A5B4FC", "Creative Director", "Role");
    let lines = ["jordan@atlas.studio", "+1 555 0142", "atlas.studio"];
    let mut y = 440.0;
    for line in lines {
        s.circle(96.0, y + 12.0, 8.0, "#6366F1", "Dot");
        s.text(124.0, y, 26.0, "#CBD5E1", line, "Contact");
        y += 50.0;
    }
    BundledTemplate {
        dir_name: "custom-business-card.ktemplate",
        manifest: manifest(
            108,
            "Business Card — Front",
            "3.5×2in business card front with a logo mark, name, role, and contact lines.",
            TemplateCategory::Custom,
            &["business card", "card", "contact", "brand", "stationery"],
        ),
        content: s.finish(),
    }
}

fn custom_business_card_back() -> BundledTemplate {
    let mut s = Sheet::new(1050.0, 600.0);
    s.bg("#6366F1");
    s.ellipse_a(120.0, 80.0, 240.0, 240.0, "#FFFFFF", 0.12, "Glow");
    s.circle(525.0, 250.0, 90.0, "#FFFFFF", "Logo ring");
    s.circle(525.0, 250.0, 60.0, "#6366F1", "Logo inner");
    s.rect(495.0, 220.0, 60.0, 60.0, "#FFFFFF", "Logo mark");
    s.text_center(525.0, 400.0, 56.0, "#FFFFFF", "ATLAS", "Wordmark");
    s.text_center(525.0, 470.0, 28.0, "#C7D2FE", "design studio", "Tagline");
    BundledTemplate {
        dir_name: "custom-business-card-back.ktemplate",
        manifest: manifest(
            109,
            "Business Card — Back",
            "3.5×2in business card back with a centered logo lockup on a solid brand field.",
            TemplateCategory::Custom,
            &["business card", "card", "logo", "brand", "stationery"],
        ),
        content: s.finish(),
    }
}

fn custom_letterhead() -> BundledTemplate {
    let mut s = Sheet::new(A4_W, A4_H);
    s.bg("#FFFFFF");
    s.rect(0.0, 0.0, A4_W, 160.0, "#1E3A8A", "Header band");
    s.circle(110.0, 80.0, 40.0, "#FFFFFF", "Logo");
    s.text(180.0, 56.0, 40.0, "#FFFFFF", "Northwind Industries", "Brand");
    s.text(820.0, 64.0, 24.0, "#BFDBFE", "123 Harbor Rd · Boston, MA", "Address");
    s.text(80.0, 280.0, 28.0, "#0F172A", "June 15, 2026", "Date");
    s.text(80.0, 360.0, 28.0, "#0F172A", "Dear Ms. Chen,", "Salutation");
    let body = [
        "Thank you for your interest in partnering with Northwind",
        "Industries. We are delighted to outline the next steps for",
        "our collaboration and look forward to a productive year.",
        "",
        "Please find the proposed terms attached. We remain at",
        "your disposal for any questions you may have.",
    ];
    let mut y = 440.0;
    for line in body {
        s.text(80.0, y, 26.0, "#334155", line, "Body line");
        y += 50.0;
    }
    s.text(80.0, y + 40.0, 26.0, "#0F172A", "Warm regards,", "Closing");
    s.text(80.0, y + 100.0, 32.0, "#1E3A8A", "Richard Hale", "Sign name");
    s.text(80.0, y + 146.0, 24.0, "#64748B", "Chief Operating Officer", "Sign role");
    s.rect(0.0, A4_H - 80.0, A4_W, 80.0, "#1E3A8A", "Footer");
    s.text_center(A4_W / 2.0, A4_H - 56.0, 22.0, "#BFDBFE", "northwind.com · (555) 010-2200 · hello@northwind.com", "Footer text");
    BundledTemplate {
        dir_name: "custom-letterhead.ktemplate",
        manifest: manifest(
            110,
            "Letterhead",
            "A4 corporate letterhead with a branded header/footer and a ready-to-edit letter body.",
            TemplateCategory::Custom,
            &["letterhead", "stationery", "letter", "corporate", "business"],
        ),
        content: s.finish(),
    }
}

fn custom_invoice() -> BundledTemplate {
    let mut s = Sheet::new(A4_W, A4_H);
    s.bg("#FFFFFF");
    s.text(80.0, 90.0, 64.0, "#0F172A", "INVOICE", "Title");
    s.text(820.0, 100.0, 30.0, "#64748B", "No. INV-2026-014", "Number");
    s.text(820.0, 144.0, 26.0, "#64748B", "Date: Jun 15, 2026", "Date");
    s.rect(80.0, 200.0, A4_W - 160.0, 3.0, "#E2E8F0", "Rule top");
    s.text(80.0, 240.0, 26.0, "#94A3B8", "BILL TO", "Bill label");
    s.text(80.0, 280.0, 30.0, "#0F172A", "Acme Corp · 50 Market St · NYC", "Bill to");
    // Table head.
    s.rect(80.0, 380.0, A4_W - 160.0, 64.0, "#0F172A", "Table head");
    s.text(110.0, 396.0, 26.0, "#FFFFFF", "Description", "Th 1");
    s.text(700.0, 396.0, 26.0, "#FFFFFF", "Qty", "Th 2");
    s.text(820.0, 396.0, 26.0, "#FFFFFF", "Rate", "Th 3");
    s.text(1000.0, 396.0, 26.0, "#FFFFFF", "Amount", "Th 4");
    let rows = [
        ("Brand identity design", "1", "$3,200", "$3,200"),
        ("Website UI design", "1", "$4,800", "$4,800"),
        ("Illustration set", "12", "$120", "$1,440"),
        ("Project management", "20", "$90", "$1,800"),
    ];
    let mut ry = 444.0;
    for (i, (desc, qty, rate, amt)) in rows.iter().enumerate() {
        let bg = if i % 2 == 0 { "#F8FAFC" } else { "#FFFFFF" };
        s.rect(80.0, ry, A4_W - 160.0, 60.0, bg, "Row");
        s.text(110.0, ry + 14.0, 26.0, "#0F172A", desc, "Cell desc");
        s.text(700.0, ry + 14.0, 26.0, "#334155", qty, "Cell qty");
        s.text(820.0, ry + 14.0, 26.0, "#334155", rate, "Cell rate");
        s.text(1000.0, ry + 14.0, 26.0, "#0F172A", amt, "Cell amt");
        ry += 60.0;
    }
    s.rect(700.0, ry + 30.0, 380.0, 80.0, "#EEF2FF", "Total box");
    s.text(720.0, ry + 52.0, 30.0, "#4338CA", "Total due", "Total label");
    s.text(960.0, ry + 48.0, 38.0, "#4338CA", "$11,240", "Total value");
    s.text(80.0, ry + 180.0, 24.0, "#94A3B8", "Payment due within 30 days · ACH or card · thank you!", "Footnote");
    BundledTemplate {
        dir_name: "custom-invoice.ktemplate",
        manifest: manifest(
            111,
            "Invoice",
            "A4 invoice with bill-to block, a striped line-item table, and a highlighted total.",
            TemplateCategory::Custom,
            &["invoice", "billing", "table", "finance", "business"],
        ),
        content: s.finish(),
    }
}

fn custom_certificate() -> BundledTemplate {
    let mut s = Sheet::new(1754.0, 1240.0);
    s.bg("#FFFDF7");
    // Double border.
    s.rect(60.0, 60.0, 1634.0, 1120.0, "#FFFDF7", "Inner field");
    s.rect(60.0, 60.0, 1634.0, 10.0, "#B45309", "Border top");
    s.rect(60.0, 1170.0, 1634.0, 10.0, "#B45309", "Border bottom");
    s.rect(60.0, 60.0, 10.0, 1120.0, "#B45309", "Border left");
    s.rect(1684.0, 60.0, 10.0, 1120.0, "#B45309", "Border right");
    s.rect(110.0, 110.0, 1534.0, 4.0, "#E7C9A0", "Inset top");
    s.rect(110.0, 1126.0, 1534.0, 4.0, "#E7C9A0", "Inset bottom");
    s.circle(877.0, 230.0, 60.0, "#B45309", "Seal");
    s.circle(877.0, 230.0, 38.0, "#FCD34D", "Seal inner");
    s.text_center(877.0, 340.0, 34.0, "#B45309", "CERTIFICATE OF ACHIEVEMENT", "Eyebrow");
    s.text_center(877.0, 420.0, 28.0, "#78716C", "This certificate is proudly presented to", "Intro");
    s.text_center(877.0, 500.0, 96.0, "#1C1917", "Alex Morgan", "Recipient");
    s.rect(577.0, 640.0, 600.0, 3.0, "#E7C9A0", "Name rule");
    s.text_center(877.0, 690.0, 28.0, "#57534E", "for outstanding completion of the Advanced", "Body 1");
    s.text_center(877.0, 730.0, 28.0, "#57534E", "Design Program with distinction.", "Body 2");
    s.text_center(577.0, 980.0, 26.0, "#1C1917", "Jamie Lee", "Sig 1 name");
    s.rect(437.0, 970.0, 280.0, 2.0, "#A8A29E", "Sig 1 rule");
    s.text_center(577.0, 1020.0, 22.0, "#78716C", "Director", "Sig 1 role");
    s.text_center(1177.0, 980.0, 26.0, "#1C1917", "June 2026", "Sig 2 name");
    s.rect(1037.0, 970.0, 280.0, 2.0, "#A8A29E", "Sig 2 rule");
    s.text_center(1177.0, 1020.0, 22.0, "#78716C", "Date", "Sig 2 role");
    BundledTemplate {
        dir_name: "custom-certificate.ktemplate",
        manifest: manifest(
            112,
            "Certificate",
            "Landscape certificate of achievement with a gold border, seal, recipient, and signature lines.",
            TemplateCategory::Custom,
            &["certificate", "award", "diploma", "achievement", "recognition"],
        ),
        content: s.finish(),
    }
}

fn custom_menu() -> BundledTemplate {
    let mut s = Sheet::new(A4_W, A4_H);
    s.bg("#1C1917");
    s.ellipse_a(620.0, 120.0, 360.0, 200.0, "#B45309", 0.25, "Glow");
    s.text_center(A4_W / 2.0, 120.0, 36.0, "#D6A35C", "TRATTORIA", "Eyebrow");
    s.text_center(A4_W / 2.0, 180.0, 96.0, "#FFFFFF", "Dinner Menu", "Title");
    s.rect(A4_W / 2.0 - 80.0, 320.0, 160.0, 4.0, "#B45309", "Rule");
    let sections = [
        ("STARTERS", &[("Bruschetta", "9"), ("Calamari fritti", "13"), ("Burrata & peach", "14")][..]),
        ("MAINS", &[("Tagliatelle ragù", "22"), ("Branzino al forno", "28"), ("Risotto ai funghi", "21")][..]),
        ("DOLCI", &[("Tiramisù", "10"), ("Panna cotta", "9")][..]),
    ];
    let mut y = 400.0;
    for (head, items) in sections {
        s.text(120.0, y, 36.0, "#D6A35C", head, "Section head");
        y += 70.0;
        for (name, price) in items {
            s.text(120.0, y, 30.0, "#F5F5F4", name, "Dish");
            s.text(1080.0, y, 30.0, "#D6A35C", price, "Price");
            s.rect(120.0, y + 44.0, A4_W - 240.0, 1.0, "#44403C", "Dotted");
            y += 80.0;
        }
        y += 50.0;
    }
    s.text_center(A4_W / 2.0, A4_H - 90.0, 24.0, "#A8A29E", "Kitchen open 5–11pm · 88 Vine Street", "Footer");
    BundledTemplate {
        dir_name: "custom-menu.ktemplate",
        manifest: manifest(
            113,
            "Restaurant Menu",
            "A4 elegant dinner menu with sections, dotted price leaders, and a footer note.",
            TemplateCategory::Custom,
            &["menu", "restaurant", "food", "dinner", "cafe"],
        ),
        content: s.finish(),
    }
}

fn custom_gift_card() -> BundledTemplate {
    let mut s = Sheet::new(1050.0, 600.0);
    s.bg("#BE123C");
    s.ellipse_a(900.0, 80.0, 260.0, 260.0, "#FFFFFF", 0.12, "Glow A");
    s.ellipse_a(120.0, 560.0, 240.0, 240.0, "#FFFFFF", 0.1, "Glow B");
    s.text(80.0, 90.0, 34.0, "#FECDD3", "THE BLOOM ROOM", "Brand");
    s.text(80.0, 220.0, 90.0, "#FFFFFF", "Gift Card", "Title");
    s.rect(80.0, 360.0, 360.0, 120.0, "#FFFFFF", "Value chip");
    s.text_center(260.0, 388.0, 64.0, "#BE123C", "$50", "Value");
    s.text(80.0, 520.0, 26.0, "#FECDD3", "Redeemable in-store · no expiry", "Note");
    s.text(620.0, 520.0, 26.0, "#FECDD3", "Code: BLOOM-7H2K", "Code");
    BundledTemplate {
        dir_name: "custom-gift-card.ktemplate",
        manifest: manifest(
            114,
            "Gift Card",
            "Gift card with a brand mark, big denomination chip, and a redemption code.",
            TemplateCategory::Custom,
            &["gift card", "voucher", "retail", "card", "promo"],
        ),
        content: s.finish(),
    }
}

fn custom_ticket() -> BundledTemplate {
    let mut s = Sheet::new(1200.0, 450.0);
    s.bg("#0F172A");
    s.rect(0.0, 0.0, 880.0, 450.0, "#4338CA", "Main stub");
    s.ellipse_a(120.0, 80.0, 220.0, 220.0, "#818CF8", 0.4, "Glow");
    // Perforation dots.
    for i in 0..9 {
        s.circle(900.0, 40.0 + f64::from(i) * 48.0, 10.0, "#0F172A", "Perf");
    }
    s.text(60.0, 60.0, 30.0, "#C7D2FE", "LIVE MUSIC SERIES", "Eyebrow");
    s.text(60.0, 130.0, 80.0, "#FFFFFF", "Neon Tides", "Title");
    s.text(60.0, 250.0, 30.0, "#C7D2FE", "Sat · Nov 9 · 8:00 PM", "When");
    s.text(60.0, 310.0, 30.0, "#C7D2FE", "The Warehouse · Sec A · Row 4", "Where");
    s.text(60.0, 380.0, 26.0, "#A5B4FC", "Admit One · General Admission", "Admit");
    s.text_center(1040.0, 120.0, 26.0, "#94A3B8", "SEAT", "Stub label");
    s.text_center(1040.0, 160.0, 60.0, "#FFFFFF", "A4", "Stub seat");
    s.rect(960.0, 280.0, 160.0, 110.0, "#FFFFFF", "QR");
    s.rect(990.0, 310.0, 100.0, 50.0, "#0F172A", "QR inner");
    BundledTemplate {
        dir_name: "custom-ticket.ktemplate",
        manifest: manifest(
            115,
            "Event Ticket",
            "Event ticket with a perforated stub, event details, seat block, and a QR placeholder.",
            TemplateCategory::Custom,
            &["ticket", "event", "concert", "admission", "pass"],
        ),
        content: s.finish(),
    }
}

fn custom_postcard() -> BundledTemplate {
    let mut s = Sheet::new(1500.0, 1050.0);
    s.bg("#0EA5E9");
    s.rect(0.0, 700.0, 1500.0, 350.0, "#FDE68A", "Beach");
    s.circle(1200.0, 280.0, 130.0, "#FEF3C7", "Sun");
    s.ellipse(420.0, 760.0, 460.0, 220.0, "#0369A1", "Wave back");
    s.ellipse(1080.0, 800.0, 520.0, 200.0, "#075985", "Wave front");
    // Big greeting wordmark.
    s.text_center(750.0, 300.0, 200.0, "#FFFFFF", "ALOHA", "Greeting");
    s.text_center(750.0, 520.0, 40.0, "#E0F2FE", "Greetings from the islands", "Sub");
    s.text(80.0, 880.0, 30.0, "#1C1917", "Wish you were here! — M & J", "Note");
    BundledTemplate {
        dir_name: "custom-postcard.ktemplate",
        manifest: manifest(
            116,
            "Postcard",
            "Landscape travel postcard with a beach scene, oversized greeting, and a handwritten note line.",
            TemplateCategory::Custom,
            &["postcard", "travel", "greeting", "vacation", "card"],
        ),
        content: s.finish(),
    }
}

fn custom_web_hero() -> BundledTemplate {
    let mut s = Sheet::new(1920.0, 1080.0);
    s.bg("#0B1120");
    s.ellipse_a(1480.0, 320.0, 560.0, 560.0, "#6366F1", 0.35, "Glow A");
    s.ellipse_a(420.0, 860.0, 520.0, 520.0, "#0EA5E9", 0.25, "Glow B");
    // Nav bar.
    s.circle(96.0, 80.0, 22.0, "#6366F1", "Logo");
    s.text(140.0, 60.0, 34.0, "#FFFFFF", "Northstar", "Brand");
    let nav = ["Product", "Pricing", "Docs", "Blog"];
    let mut nx = 1320.0;
    for item in nav {
        s.text(nx, 62.0, 28.0, "#94A3B8", item, "Nav item");
        nx += 150.0;
    }
    s.rect(1760.0, 48.0, 110.0, 60.0, "#6366F1", "Nav CTA");
    s.text_center(1815.0, 64.0, 26.0, "#FFFFFF", "Sign up", "Nav CTA label");
    // Hero copy.
    s.rect(160.0, 360.0, 220.0, 56.0, "#1E293B", "Badge");
    s.text(186.0, 374.0, 26.0, "#A5B4FC", "NEW · v2.0", "Badge text");
    s.text(160.0, 450.0, 110.0, "#FFFFFF", "Ship faster with", "Headline 1");
    s.text(160.0, 580.0, 110.0, "#818CF8", "Northstar", "Headline 2");
    s.text(160.0, 740.0, 36.0, "#94A3B8", "The all-in-one platform for modern product teams.", "Sub");
    s.rect(160.0, 840.0, 300.0, 90.0, "#6366F1", "Primary CTA");
    s.text_center(310.0, 866.0, 32.0, "#FFFFFF", "Get started", "Primary label");
    s.rect(490.0, 840.0, 300.0, 90.0, "#1E293B", "Secondary CTA");
    s.text_center(640.0, 866.0, 32.0, "#E2E8F0", "Book a demo", "Secondary label");
    BundledTemplate {
        dir_name: "custom-web-hero.ktemplate",
        manifest: manifest(
            117,
            "Web Hero Section",
            "1920×1080 landing-page hero with a nav bar, badge, big headline, subcopy, and dual CTAs.",
            TemplateCategory::Custom,
            &["web", "hero", "landing", "saas", "header"],
        ),
        content: s.finish(),
    }
}

fn custom_infographic() -> BundledTemplate {
    let mut s = Sheet::new(1080.0, 1920.0);
    s.bg("#FFFFFF");
    s.rect(0.0, 0.0, 1080.0, 300.0, "#0F766E", "Header");
    s.text_center(540.0, 100.0, 34.0, "#99F6E4", "BY THE NUMBERS", "Eyebrow");
    s.text_center(540.0, 170.0, 72.0, "#FFFFFF", "Remote Work 2026", "Title");
    let stats = [
        ("74%", "of teams are now hybrid", "#0EA5E9"),
        ("3.2h", "saved per week on commute", "#8B5CF6"),
        ("58%", "report higher focus at home", "#F59E0B"),
        ("2×", "faster hiring across regions", "#EF4444"),
    ];
    let mut y = 420.0;
    for (i, (num, label, hex)) in stats.iter().enumerate() {
        let _ = i;
        s.rect(80.0, y, 920.0, 300.0, "#F8FAFC", "Stat card");
        s.rect(80.0, y, 920.0, 10.0, hex, "Stat top");
        s.circle(220.0, y + 150.0, 90.0, hex, "Stat disc");
        s.text_center(220.0, y + 108.0, 64.0, "#FFFFFF", num, "Stat num");
        s.text(360.0, y + 120.0, 38.0, "#0F172A", label, "Stat label");
        y += 350.0;
    }
    s.text_center(540.0, 1850.0, 24.0, "#94A3B8", "Source: KCreate Workplace Survey, n=4,200", "Source");
    BundledTemplate {
        dir_name: "custom-infographic.ktemplate",
        manifest: manifest(
            118,
            "Infographic",
            "Tall infographic with a header and four stat cards pairing big figures with captions.",
            TemplateCategory::Custom,
            &["infographic", "stats", "data", "report", "social"],
        ),
        content: s.finish(),
    }
}

fn custom_email_header() -> BundledTemplate {
    let mut s = Sheet::new(1200.0, 400.0);
    s.bg("#4F46E5");
    s.ellipse_a(1040.0, 60.0, 280.0, 280.0, "#FFFFFF", 0.12, "Glow A");
    s.ellipse_a(160.0, 380.0, 240.0, 240.0, "#FFFFFF", 0.1, "Glow B");
    s.circle(120.0, 90.0, 30.0, "#FFFFFF", "Logo");
    s.text(176.0, 70.0, 32.0, "#FFFFFF", "Monthly Digest", "Brand");
    s.text(80.0, 190.0, 72.0, "#FFFFFF", "What's new in June", "Title");
    s.text(80.0, 300.0, 30.0, "#C7D2FE", "Product updates, tips, and community picks", "Sub");
    BundledTemplate {
        dir_name: "custom-email-header.ktemplate",
        manifest: manifest(
            119,
            "Email Header",
            "1200×400 newsletter email header with a logo lockup, title, and subtitle.",
            TemplateCategory::Custom,
            &["email", "newsletter", "header", "banner", "marketing"],
        ),
        content: s.finish(),
    }
}

fn custom_coupon() -> BundledTemplate {
    let mut s = Sheet::new(1200.0, 600.0);
    s.bg("#FFFFFF");
    s.rect(40.0, 40.0, 1120.0, 520.0, "#FFFFFF", "Field");
    // Dashed border via segments.
    for i in 0..23 {
        let x = 40.0 + f64::from(i) * 50.0;
        s.rect(x, 40.0, 30.0, 6.0, "#DC2626", "Dash top");
        s.rect(x, 554.0, 30.0, 6.0, "#DC2626", "Dash bottom");
    }
    s.rect(760.0, 40.0, 6.0, 520.0, "#FCA5A5", "Split rule");
    s.text(110.0, 110.0, 32.0, "#DC2626", "LIMITED TIME OFFER", "Eyebrow");
    s.text(110.0, 200.0, 150.0, "#0F172A", "25% OFF", "Discount");
    s.text(110.0, 380.0, 32.0, "#475569", "Your next order over $50", "Terms");
    s.text(110.0, 470.0, 26.0, "#94A3B8", "Valid through Jul 31, 2026", "Expiry");
    s.text_center(963.0, 170.0, 28.0, "#475569", "USE CODE", "Code label");
    s.rect(810.0, 230.0, 306.0, 110.0, "#FEE2E2", "Code box");
    s.text_center(963.0, 258.0, 56.0, "#DC2626", "SAVE25", "Code");
    s.text_center(963.0, 420.0, 24.0, "#94A3B8", "Online & in-store", "Code note");
    BundledTemplate {
        dir_name: "custom-coupon.ktemplate",
        manifest: manifest(
            120,
            "Coupon",
            "Coupon with a dashed border, bold discount, redemption code box, and expiry.",
            TemplateCategory::Custom,
            &["coupon", "discount", "promo", "voucher", "retail"],
        ),
        content: s.finish(),
    }
}

fn custom_name_badge() -> BundledTemplate {
    let mut s = Sheet::new(1050.0, 750.0);
    s.bg("#FFFFFF");
    s.rect(0.0, 0.0, 1050.0, 200.0, "#2563EB", "Header");
    s.text_center(525.0, 70.0, 40.0, "#FFFFFF", "DESIGN SUMMIT 2026", "Event");
    s.text_center(525.0, 130.0, 26.0, "#BFDBFE", "Berlin · Hall 4", "Event sub");
    s.text_center(525.0, 280.0, 30.0, "#64748B", "HELLO, MY NAME IS", "Prompt");
    s.text_center(525.0, 350.0, 96.0, "#0F172A", "Sam Okafor", "Name");
    s.rect(325.0, 500.0, 400.0, 3.0, "#E2E8F0", "Rule");
    s.text_center(525.0, 530.0, 34.0, "#2563EB", "Product Designer · Lumen", "Role");
    s.rect(0.0, 650.0, 1050.0, 100.0, "#EFF6FF", "Footer band");
    s.circle(120.0, 700.0, 34.0, "#2563EB", "Logo");
    s.text(180.0, 682.0, 30.0, "#1E3A8A", "@samokafor", "Handle");
    s.text(760.0, 682.0, 30.0, "#1E3A8A", "SPEAKER", "Pass type");
    BundledTemplate {
        dir_name: "custom-name-badge.ktemplate",
        manifest: manifest(
            121,
            "Name Badge",
            "Conference name badge with an event header, large name, role line, and a footer band.",
            TemplateCategory::Custom,
            &["name badge", "conference", "event", "lanyard", "id"],
        ),
        content: s.finish(),
    }
}

fn custom_price_list() -> BundledTemplate {
    let mut s = Sheet::new(A4_W, A4_H);
    s.bg("#0F172A");
    s.ellipse_a(1080.0, 160.0, 320.0, 320.0, "#F472B6", 0.3, "Glow");
    s.text(80.0, 110.0, 34.0, "#F9A8D4", "SALON BELLE", "Brand");
    s.text(80.0, 180.0, 84.0, "#FFFFFF", "Price List", "Title");
    s.rect(80.0, 320.0, 140.0, 6.0, "#EC4899", "Rule");
    let sections = [
        ("HAIR", &[("Cut & style", "$55"), ("Color & gloss", "$120"), ("Balayage", "$180")][..]),
        ("NAILS", &[("Classic manicure", "$30"), ("Gel manicure", "$45"), ("Spa pedicure", "$55")][..]),
        ("SKIN", &[("Express facial", "$60"), ("Deep cleanse", "$90")][..]),
    ];
    let mut y = 400.0;
    for (head, items) in sections {
        s.text(80.0, y, 36.0, "#EC4899", head, "Section head");
        y += 70.0;
        for (name, price) in items {
            s.text(80.0, y, 30.0, "#E2E8F0", name, "Service");
            s.text(1040.0, y, 30.0, "#F9A8D4", price, "Price");
            s.rect(80.0, y + 46.0, A4_W - 160.0, 1.0, "#1E293B", "Divider");
            y += 80.0;
        }
        y += 50.0;
    }
    s.text_center(A4_W / 2.0, A4_H - 90.0, 24.0, "#94A3B8", "Book online · salonbelle.com · (555) 010-9090", "Footer");
    BundledTemplate {
        dir_name: "custom-price-list.ktemplate",
        manifest: manifest(
            122,
            "Price List",
            "A4 dark price list with sectioned services, dotted divider rows, and a booking footer.",
            TemplateCategory::Custom,
            &["price list", "salon", "services", "menu", "rates"],
        ),
        content: s.finish(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_has_expected_breadth() {
        let all = bundled_templates();
        assert!(
            all.len() >= 120,
            "catalog should ship >= 120 templates, got {}",
            all.len()
        );
    }

    #[test]
    fn ids_and_dirs_are_unique() {
        let all = bundled_templates();
        let mut ids: Vec<Uuid> = all.iter().map(|t| t.manifest.id).collect();
        let count = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), count, "template ids must be unique");

        let mut dirs: Vec<&str> = all.iter().map(|t| t.dir_name).collect();
        let dir_count = dirs.len();
        dirs.sort_unstable();
        dirs.dedup();
        assert_eq!(dirs.len(), dir_count, "template dir names must be unique");
    }

    #[test]
    fn every_template_is_well_formed() {
        for t in bundled_templates() {
            assert!(
                t.dir_name.ends_with(".ktemplate"),
                "{} must end with .ktemplate",
                t.dir_name
            );
            assert_eq!(t.manifest.thumbnail.as_deref(), Some("thumbnail.png"));
            assert!(t.content.width > 0.0 && t.content.height > 0.0);
            assert!(
                t.content.items.len() >= 5,
                "{} should be a real multi-layer design",
                t.dir_name
            );
            // First item is a full-bleed background covering the canvas.
            match &t.content.items[0] {
                TemplateItem::Rect { x, y, w, h, .. } => {
                    assert_eq!((*x, *y), (0.0, 0.0));
                    assert_eq!((*w, *h), (t.content.width, t.content.height));
                }
                other => panic!("{} item[0] should be a bg rect, got {other:?}", t.dir_name),
            }
        }
    }

    #[test]
    fn categories_are_varied() {
        let mut cats: Vec<TemplateCategory> = bundled_templates()
            .into_iter()
            .map(|t| t.manifest.category)
            .collect();
        cats.sort_by_key(|c| format!("{c:?}"));
        cats.dedup();
        assert!(
            cats.len() >= 10,
            "catalog should span >= 10 categories, got {}",
            cats.len()
        );
    }

    #[test]
    fn content_round_trips_through_json() {
        let t = mobile_login();
        let json = serde_json::to_string(&t.content).unwrap();
        let parsed: TemplateContent = serde_json::from_str(&json).unwrap();
        assert_eq!(t.content, parsed);
    }

    #[test]
    fn item_json_uses_wire_field_names() {
        // Lock the serialized shape against the bridge's CanvasBatchItem
        // wire contract (kind tag + snake_case fields, parent always
        // present).
        let rect = TemplateItem::Rect {
            parent: None,
            x: 1.0,
            y: 2.0,
            w: 3.0,
            h: 4.0,
            fill: Some(fill("#FFFFFF")),
            name: Some("bg".into()),
        };
        let v = serde_json::to_value(&rect).unwrap();
        assert_eq!(v["kind"], "rect");
        assert!(v.get("parent").is_some(), "parent must be serialized");
        assert_eq!(v["w"], 3.0);
        assert_eq!(v["fill"]["kind"], "solid");
    }

    #[test]
    fn seed_is_copy_if_empty_and_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("templates");

        let first = seed_bundled_templates(&root).unwrap();
        assert_eq!(first, bundled_templates().len());
        assert!(root.join("mobile-login.ktemplate/manifest.json").exists());
        assert!(root.join("mobile-login.ktemplate/content.json").exists());

        // Second run is a no-op (already populated).
        let second = seed_bundled_templates(&root).unwrap();
        assert_eq!(second, 0);
    }

    #[test]
    fn seeded_manifests_parse_and_match_catalog() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("templates");
        seed_bundled_templates(&root).unwrap();
        for t in bundled_templates() {
            let mpath = root.join(t.dir_name).join("manifest.json");
            let raw = std::fs::read_to_string(&mpath).unwrap();
            let parsed: TemplateManifest = serde_json::from_str(&raw).unwrap();
            assert_eq!(parsed.id, t.manifest.id);
            assert_eq!(parsed.name, t.manifest.name);
            assert_eq!(parsed.category, t.manifest.category);
        }
    }
}
