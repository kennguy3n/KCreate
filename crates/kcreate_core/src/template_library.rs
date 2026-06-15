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
/// 22 designs spanning 10 [`TemplateCategory`] families: mobile-app UI
/// kits, presentation slides, a pitch one-pager, social posts, posters,
/// a flyer, a résumé, and business covers.
#[must_use]
pub fn bundled_templates() -> Vec<BundledTemplate> {
    vec![
        mobile_onboarding(),
        mobile_login(),
        mobile_feed(),
        mobile_profile(),
        mobile_settings(),
        mobile_music(),
        deck_title(),
        deck_agenda(),
        deck_metrics(),
        deck_team(),
        pitch_onepager(),
        social_ig_post(),
        social_ig_story(),
        social_linkedin(),
        social_quote(),
        poster_event(),
        poster_concert(),
        flyer_sale(),
        resume_cv(),
        report_cover(),
        brochure_cover(),
        proposal_cover(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_has_expected_breadth() {
        let all = bundled_templates();
        assert!(
            all.len() >= 18,
            "catalog should ship >= 18 templates, got {}",
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
            cats.len() >= 6,
            "catalog should span >= 6 categories, got {}",
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
