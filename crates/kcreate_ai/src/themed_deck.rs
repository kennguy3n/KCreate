//! Brief → themed multi-page design (Gamma-style generator).
//!
//! Turns a short free-form brief into a *complete, professionally
//! themed* multi-page document: a deck (title card + section cards)
//! or a structured one-pager. The output carries a consistent theme
//! (palette + type scale + spacing) across every page and the layout
//! is solved with [`kcreate_layout`]'s flex solver rather than hand-
//! placed coordinates.
//!
//! Two stages:
//!
//! 1. [`outline_from_brief`] — deterministic, offline content
//!    planner. It extracts a title/subject from the brief, picks a
//!    section template (pitch vs. generic), and folds any user-
//!    supplied talking points into the section bullets. This is the
//!    always-available path used when no LLM model is loaded.
//! 2. [`generate_design`] — turns an outline + a [`Theme`] + a
//!    [`DesignFormat`] into a fully positioned [`GeneratedDesign`]
//!    (themed rectangles + wrapped text runs at world coordinates).
//!
//! The bridge ([`crate`] consumer `kcreate_bridge::phase10`) may
//! enrich the outline with a GBNF-constrained LLM call when the
//! sidecar is ready, but the feature never *requires* it: the
//! deterministic planner alone produces a real, populated design.
//!
//! Everything here is pure and side-effect free — no document
//! mutation, no networking, no globals — so it is trivially testable
//! and safe in the editing path.

use kcreate_core::node::Bounds;
use kcreate_layout::{layout_flex, Alignment, CrossAlignment, FlexDirection, FlexLayout, Padding};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

/// Output document shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum DesignFormat {
    /// A 16:9 multi-slide deck (title card + section cards).
    #[default]
    Deck,
    /// A single, vertically-structured page (title block + sections).
    OnePager,
    /// A matched pair of social posts: a square feed tile plus a
    /// vertical 9:16 story.
    SocialPost,
    /// A marketing landing page: hero + feature grid + call-to-action
    /// band, on one tall canvas.
    WebPage,
    /// A multi-page document / report: cover + paginated body sections
    /// with running header, footer, and page numbers.
    Document,
}

impl DesignFormat {
    /// Every format, in UI order. Drives the picker and exhaustive
    /// tests.
    #[must_use]
    pub fn all() -> [Self; 5] {
        [
            Self::Deck,
            Self::OnePager,
            Self::SocialPost,
            Self::WebPage,
            Self::Document,
        ]
    }

    /// Stable wire token. Mirrors the serde `camelCase` rename so the
    /// bridge / TypeScript surface and this method never drift.
    #[must_use]
    pub fn wire(self) -> &'static str {
        match self {
            Self::Deck => "deck",
            Self::OnePager => "onePager",
            Self::SocialPost => "socialPost",
            Self::WebPage => "webPage",
            Self::Document => "document",
        }
    }

    /// Whether this format has a hero/section image slot the diffusion
    /// path can populate (degrading to a vector gradient placeholder
    /// when no model is available). Decks and one-pagers stay purely
    /// vector so their tuned output never changes.
    #[must_use]
    pub fn supports_imagery(self) -> bool {
        matches!(self, Self::SocialPost | Self::WebPage | Self::Document)
    }
}

/// Page geometry for one-pager output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum OnePagerSize {
    Letter,
    #[default]
    A4,
    Square,
}

impl OnePagerSize {
    /// Dimensions in pixels at 96 DPI.
    #[must_use]
    pub fn dimensions(self) -> (f64, f64) {
        match self {
            Self::Letter => (816.0, 1056.0),
            Self::A4 => (794.0, 1123.0),
            Self::Square => (1024.0, 1024.0),
        }
    }
}

/// Identifier of a built-in professional theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum ThemeId {
    /// Deep indigo background, violet accent — confident and modern.
    #[default]
    Midnight,
    /// Warm cream background, terracotta accent — editorial and soft.
    Sunrise,
    /// Crisp white background, emerald accent — clean and corporate.
    Forest,
    /// Near-black background, amber accent — bold and high-contrast.
    Ember,
    /// Light slate background, blue accent — neutral and professional.
    Slate,
}

impl ThemeId {
    /// All built-in themes, in display order.
    #[must_use]
    pub fn all() -> [Self; 5] {
        [
            Self::Midnight,
            Self::Sunrise,
            Self::Forest,
            Self::Ember,
            Self::Slate,
        ]
    }

    /// Resolve a wire id string (`"midnight"`, `"sunrise"`, …) to a
    /// theme id, falling back to [`ThemeId::Midnight`] on anything
    /// unrecognised so the generator never fails on a stray value.
    #[must_use]
    pub fn from_wire(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "sunrise" => Self::Sunrise,
            "forest" => Self::Forest,
            "ember" => Self::Ember,
            "slate" => Self::Slate,
            _ => Self::Midnight,
        }
    }

    /// Lower-case wire token for this theme.
    #[must_use]
    pub fn wire(self) -> &'static str {
        match self {
            Self::Midnight => "midnight",
            Self::Sunrise => "sunrise",
            Self::Forest => "forest",
            Self::Ember => "ember",
            Self::Slate => "slate",
        }
    }
}

/// A resolved theme: palette + font families. Type sizes live in
/// [`TypeScale`] (which depends on the output format), so a theme is
/// purely about colour and typeface choice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Theme {
    pub id: ThemeId,
    pub name: String,
    /// Full-bleed page/slide background.
    pub background: String,
    /// Card / panel surface drawn on content slides.
    pub surface: String,
    /// Primary accent (accent bars, bullet markers, title flourish).
    pub primary: String,
    /// Secondary accent (figures, dividers).
    pub secondary: String,
    /// Heading text colour.
    pub heading: String,
    /// Body text colour.
    pub body: String,
    /// Muted text colour (subtitles, captions, footers).
    pub muted: String,
    /// Title-card text colour (drawn on `background`, not `surface`).
    pub on_background: String,
    pub heading_font: String,
    pub body_font: String,
}

impl Theme {
    /// Every colour the theme can paint onto a node, as hex strings.
    /// Used by the bridge to seed a brand kit and by tests to assert
    /// theme consistency.
    #[must_use]
    pub fn palette(&self) -> Vec<String> {
        vec![
            self.primary.clone(),
            self.secondary.clone(),
            self.surface.clone(),
            self.background.clone(),
            self.heading.clone(),
            self.body.clone(),
        ]
    }

    /// The set of colours legitimately applied to *text* nodes.
    #[cfg(test)]
    #[must_use]
    fn text_colors(&self) -> [&str; 5] {
        [
            &self.heading,
            &self.body,
            &self.muted,
            &self.on_background,
            &self.primary,
        ]
    }
}

/// Resolve a [`ThemeId`] to its concrete [`Theme`].
#[must_use]
pub fn theme(id: ThemeId) -> Theme {
    let (name, bg, surface, primary, secondary, heading, body, muted, on_bg) = match id {
        ThemeId::Midnight => (
            "Midnight", "#0B1020", "#161C32", "#7C5CFF", "#34D8FF", "#F4F6FF", "#C2C8E0",
            "#8A90AE", "#F4F6FF",
        ),
        ThemeId::Sunrise => (
            "Sunrise", "#FBF6EF", "#FFFFFF", "#E2603B", "#F2A65A", "#2B2118", "#4F4639", "#9A8F7E",
            "#2B2118",
        ),
        ThemeId::Forest => (
            "Forest", "#FFFFFF", "#F2F7F3", "#1E8E5A", "#0F6E6E", "#10261B", "#3A4A40", "#7E8C84",
            "#10261B",
        ),
        ThemeId::Ember => (
            "Ember", "#121212", "#1E1B18", "#FF8A3D", "#FFC857", "#FFF7EE", "#D8CDC2", "#9A8E80",
            "#FFF7EE",
        ),
        ThemeId::Slate => (
            "Slate", "#EEF2F7", "#FFFFFF", "#2563EB", "#0EA5E9", "#0F1B2D", "#3C4A60", "#7A879B",
            "#0F1B2D",
        ),
    };
    Theme {
        id,
        name: name.to_string(),
        background: bg.to_string(),
        surface: surface.to_string(),
        primary: primary.to_string(),
        secondary: secondary.to_string(),
        heading: heading.to_string(),
        body: body.to_string(),
        muted: muted.to_string(),
        on_background: on_bg.to_string(),
        heading_font: "Inter".to_string(),
        body_font: "Inter".to_string(),
    }
}

/// Consistent type scale for a format. Sizes are deliberately
/// identical across every page of a document so headings/body read
/// uniformly — only the format (deck vs. one-pager) changes the
/// absolute sizes, because the canvas dimensions differ by ~2.4×.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TypeScale {
    pub title: f32,
    pub heading: f32,
    pub subheading: f32,
    pub body: f32,
    pub caption: f32,
}

impl TypeScale {
    #[must_use]
    fn for_format(format: DesignFormat) -> Self {
        match format {
            DesignFormat::Deck => Self {
                title: 92.0,
                heading: 56.0,
                subheading: 34.0,
                body: 30.0,
                caption: 22.0,
            },
            DesignFormat::OnePager => Self {
                title: 46.0,
                heading: 27.0,
                subheading: 19.0,
                body: 16.0,
                caption: 12.0,
            },
            DesignFormat::SocialPost => Self {
                title: 76.0,
                heading: 40.0,
                subheading: 34.0,
                body: 30.0,
                caption: 24.0,
            },
            DesignFormat::WebPage => Self {
                title: 72.0,
                heading: 40.0,
                subheading: 26.0,
                body: 20.0,
                caption: 15.0,
            },
            DesignFormat::Document => Self {
                title: 40.0,
                heading: 24.0,
                subheading: 17.0,
                body: 13.0,
                caption: 10.0,
            },
        }
    }

    /// Scale every size by `factor` (clamped to a tasteful ±10% band
    /// so layout variety never produces illegible or oversized type).
    /// The scaled scale is stored on the [`GeneratedDesign`] so the
    /// palette/scale invariant continues to hold against it.
    #[must_use]
    fn scaled(self, factor: f32) -> Self {
        let f = factor.clamp(0.9, 1.1);
        Self {
            title: self.title * f,
            heading: self.heading * f,
            subheading: self.subheading * f,
            body: self.body * f,
            caption: self.caption * f,
        }
    }

    /// Every size in the scale, for consistency assertions.
    #[cfg(test)]
    #[must_use]
    fn all(&self) -> [f32; 5] {
        [
            self.title,
            self.heading,
            self.subheading,
            self.body,
            self.caption,
        ]
    }
}

/// A planned section: one slide (deck) or one block (one-pager).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlideOutline {
    pub heading: String,
    pub bullets: Vec<String>,
}

/// A planned document, independent of theme/format.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeckOutline {
    pub title: String,
    pub subtitle: String,
    pub slides: Vec<SlideOutline>,
}

/// Options for [`outline_from_brief`] / [`generate_design`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemedDesignOptions {
    pub format: DesignFormat,
    pub theme_id: ThemeId,
    pub one_pager_size: OnePagerSize,
    /// Desired number of *content* sections (excludes the title
    /// card). `None` → a sensible default per format. Clamped to a
    /// legible range.
    pub section_count: Option<u32>,
}

impl Default for ThemedDesignOptions {
    fn default() -> Self {
        Self {
            format: DesignFormat::Deck,
            theme_id: ThemeId::Midnight,
            one_pager_size: OnePagerSize::A4,
            section_count: None,
        }
    }
}

impl ThemedDesignOptions {
    /// Desired content-section count clamped to a legible range
    /// (`[3, max]`, where `max` depends on the format). Both the
    /// deterministic planner and the LLM-enrichment path resolve the
    /// section count through this method so a caller-supplied extreme
    /// (e.g. `0` or `99`) is treated identically on either path.
    #[must_use]
    pub fn resolved_section_count(&self) -> usize {
        let (default, min, max) = match self.format {
            DesignFormat::Deck => (6usize, 3usize, 11usize),
            DesignFormat::OnePager => (4usize, 3usize, 6usize),
            DesignFormat::SocialPost => (3usize, 2usize, 4usize),
            DesignFormat::WebPage => (3usize, 3usize, 5usize),
            DesignFormat::Document => (5usize, 4usize, 8usize),
        };
        let n = self.section_count.map_or(default, |v| v as usize);
        n.clamp(min, max)
    }
}

/// One painted element of the generated design. Coordinates are in
/// *page-local* space (origin at the page's top-left); the bridge
/// translates these into world coordinates when it tiles slides.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesignElement {
    pub role: ElementRole,
    pub kind: ElementKind,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    /// Present for text elements.
    pub text: Option<String>,
    /// Font size in px (0 for rectangles).
    pub font_size: f32,
    /// Font family ("" for rectangles).
    pub font_family: String,
    /// Fill colour (hex). For text this is the glyph colour; for
    /// rectangles the fill; for images the first gradient stop of the
    /// vector placeholder.
    pub fill: String,
    pub corner_radius: f64,
    /// Diffusion prompt for image elements. `Some` marks a real
    /// hero/section image the bridge will try to synthesise (falling
    /// back to a gradient placeholder offline); `None` keeps the
    /// element a purely decorative gradient panel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_prompt: Option<String>,
    /// Second gradient stop (hex) for image placeholders / two-tone
    /// accents. `None` keeps a flat fill.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill_secondary: Option<String>,
}

/// Semantic role of an element — drives the bridge's node naming and
/// lets tests reason about structure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ElementRole {
    Surface,
    AccentBar,
    Title,
    Subtitle,
    Heading,
    Body,
    BulletMarker,
    Figure,
    Footer,
}

/// Whether an element renders as text or a filled rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ElementKind {
    Text,
    Rect,
    /// A raster image slot — populated by diffusion when a model is
    /// available, otherwise rendered as a gradient placeholder.
    Image,
}

/// One page of the generated design.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneratedPage {
    pub title: String,
    pub width: f64,
    pub height: f64,
    pub background: String,
    pub elements: Vec<DesignElement>,
}

/// A fully laid-out, themed multi-page design.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneratedDesign {
    pub theme: Theme,
    pub format: DesignFormat,
    pub type_scale: TypeScale,
    pub pages: Vec<GeneratedPage>,
}

/// Errors from the generator.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ThemedDesignError {
    #[error("themed_deck: empty brief")]
    Empty,
}

// ---------------------------------------------------------------------------
// Stage 1 — content planning
// ---------------------------------------------------------------------------

/// Build a [`DeckOutline`] from a free-form brief, fully offline and
/// deterministically.
///
/// # Errors
///
/// Returns [`ThemedDesignError::Empty`] when the brief has no
/// non-whitespace content.
pub fn outline_from_brief(
    brief: &str,
    options: ThemedDesignOptions,
) -> Result<DeckOutline, ThemedDesignError> {
    let trimmed = brief.trim();
    if trimmed.is_empty() {
        return Err(ThemedDesignError::Empty);
    }

    let (raw_title, user_points) = split_brief(trimmed);

    let title = clean_title(&raw_title);
    let subject = derive_subject(&title);
    let kind = TemplateKind::detect(trimmed);
    let section_count = options.resolved_section_count();

    let subtitle = kind.subtitle(&subject);
    let sections = kind.sections(&subject, section_count);

    // Distribute user points round-robin across section bullets so a
    // richer brief produces richer, on-topic content.
    let mut slides: Vec<SlideOutline> = sections
        .into_iter()
        .map(|(heading, bullets)| SlideOutline { heading, bullets })
        .collect();
    if !user_points.is_empty() && !slides.is_empty() {
        let slide_len = slides.len();
        for (i, point) in user_points.iter().enumerate() {
            let slide = &mut slides[i % slide_len];
            // Keep slides legible: cap bullets per slide.
            if slide.bullets.len() < 5 {
                slide.bullets.push(point.clone());
            }
        }
    }

    Ok(DeckOutline {
        title,
        subtitle,
        slides,
    })
}

/// Validate an LLM-proposed outline (parsed from JSON elsewhere),
/// normalising it into a usable [`DeckOutline`]. Empty headings/
/// bullets are dropped; a missing title falls back to the brief. The
/// bridge calls this on a sidecar reply and falls back to
/// [`outline_from_brief`] when the reply is unusable.
#[must_use]
pub fn sanitize_outline(mut outline: DeckOutline, fallback_title: &str) -> Option<DeckOutline> {
    outline.title = clean_title(if outline.title.trim().is_empty() {
        fallback_title
    } else {
        &outline.title
    });
    outline.slides.retain_mut(|s| {
        s.heading = s.heading.trim().to_string();
        s.bullets = s
            .bullets
            .iter()
            .map(|b| b.trim().to_string())
            .filter(|b| !b.is_empty())
            .collect();
        !s.heading.is_empty()
    });
    if outline.slides.is_empty() {
        return None;
    }
    if outline.subtitle.trim().is_empty() {
        outline.subtitle = derive_subject(&outline.title);
    } else {
        outline.subtitle = outline.subtitle.trim().to_string();
    }
    Some(outline)
}

/// Placeholder token replaced with the brief subject in section
/// templates. Declared at module scope so it does not look like a
/// stray formatting argument next to an in-scope `subject` binding.
const SUBJECT_TOKEN: &str = "{subject}";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TemplateKind {
    Pitch,
    Generic,
}

impl TemplateKind {
    fn detect(brief: &str) -> Self {
        const PITCH_HINTS: [&str; 6] = [
            "pitch",
            "investor",
            "fundrais",
            "seed round",
            "series a",
            "startup",
        ];
        let lower = brief.to_ascii_lowercase();
        if PITCH_HINTS.iter().any(|h| lower.contains(h)) {
            Self::Pitch
        } else {
            Self::Generic
        }
    }

    fn subtitle(self, subject: &str) -> String {
        match self {
            Self::Pitch => format!("Investor pitch · {subject}"),
            Self::Generic => format!("{subject} · an overview"),
        }
    }

    /// Section headings + bullet templates, with `{subject}`
    /// substituted. Returns exactly `count` sections (the template
    /// list is long enough for the clamped maximum).
    fn sections(self, subject: &str, count: usize) -> Vec<(String, Vec<String>)> {
        let templates: &[(&str, &[&str])] = match self {
            Self::Pitch => &PITCH_SECTIONS,
            Self::Generic => &GENERIC_SECTIONS,
        };
        templates
            .iter()
            .take(count)
            .map(|(heading, bullets)| {
                (
                    heading.to_string(),
                    bullets
                        .iter()
                        .map(|b| b.replace(SUBJECT_TOKEN, subject))
                        .collect(),
                )
            })
            .collect()
    }
}

const PITCH_SECTIONS: [(&str, &[&str]); 11] = [
    (
        "The problem",
        &[
            "Today, reaching great {subject} is harder and slower than it should be.",
            "Customers settle for options that are inconsistent or impersonal.",
            "The status quo leaves real value on the table.",
        ],
    ),
    (
        "Our solution",
        &[
            "{subject}, reimagined around what people actually want.",
            "A focused experience that removes friction end to end.",
            "Designed to delight from the very first interaction.",
        ],
    ),
    (
        "How it works",
        &[
            "A simple, three-step journey from discovery to delivery.",
            "Thoughtful defaults so anyone can get value in minutes.",
            "Built to scale without losing the personal touch.",
        ],
    ),
    (
        "Why now",
        &[
            "Demand for {subject} is growing faster than supply.",
            "New tools make a better experience finally possible.",
            "Early movers will define the category.",
        ],
    ),
    (
        "Market opportunity",
        &[
            "A large and expanding audience actively seeking {subject}.",
            "Clear willingness to pay for quality and convenience.",
            "Multiple adjacent segments to expand into.",
        ],
    ),
    (
        "Business model",
        &[
            "Transparent pricing aligned with the value delivered.",
            "Recurring revenue with healthy unit economics.",
            "Expansion revenue as customers grow with us.",
        ],
    ),
    (
        "Traction",
        &[
            "Strong early signal from our first {subject} customers.",
            "Word-of-mouth growth and high repeat usage.",
            "A pipeline of partners ready to scale with us.",
        ],
    ),
    (
        "Competitive edge",
        &[
            "A product experience competitors can't easily copy.",
            "Deep focus on {subject} instead of doing everything.",
            "Brand and community that compound over time.",
        ],
    ),
    (
        "The team",
        &[
            "Founders with hands-on {subject} expertise.",
            "A team that has shipped and scaled before.",
            "Advisors who open doors in the industry.",
        ],
    ),
    (
        "Roadmap",
        &[
            "Launch and nail the core {subject} experience.",
            "Expand into adjacent offerings and channels.",
            "Invest in automation to widen our margins.",
        ],
    ),
    (
        "The ask",
        &[
            "We're raising to accelerate {subject} growth.",
            "Capital fuels product, team, and go-to-market.",
            "Join us in building the category leader.",
        ],
    ),
];

const GENERIC_SECTIONS: [(&str, &[&str]); 11] = [
    (
        "Overview",
        &[
            "A clear introduction to {subject} and why it matters.",
            "What you'll take away from the pages that follow.",
            "Framing the opportunity in plain terms.",
        ],
    ),
    (
        "Why it matters",
        &[
            "{subject} solves a real, everyday need.",
            "The cost of doing nothing keeps rising.",
            "Getting this right unlocks outsized value.",
        ],
    ),
    (
        "How it works",
        &[
            "A straightforward approach anyone can follow.",
            "Sensible defaults that work out of the box.",
            "Room to customise as needs grow.",
        ],
    ),
    (
        "Key benefits",
        &[
            "Faster results with far less effort.",
            "A consistent, high-quality {subject} experience.",
            "Confidence that the details are handled.",
        ],
    ),
    (
        "What's included",
        &[
            "Everything needed to get started with {subject}.",
            "Clear guidance at each step of the way.",
            "Support resources when you need a hand.",
        ],
    ),
    (
        "Highlights",
        &[
            "The moments that make {subject} stand out.",
            "Proof points that build trust quickly.",
            "Details people remember and share.",
        ],
    ),
    (
        "Use cases",
        &[
            "Where {subject} delivers the most value today.",
            "Scenarios that map to real goals.",
            "Examples you can adapt to your own context.",
        ],
    ),
    (
        "Roadmap",
        &[
            "What's shipping now and what's coming next.",
            "How {subject} evolves over the year.",
            "Milestones to hold us accountable.",
        ],
    ),
    (
        "Results",
        &[
            "Outcomes that demonstrate real impact.",
            "Metrics that matter, tracked over time.",
            "Stories behind the numbers.",
        ],
    ),
    (
        "FAQ",
        &[
            "Answers to the questions we hear most about {subject}.",
            "Practical guidance to remove any doubt.",
            "Where to learn more.",
        ],
    ),
    (
        "Next steps",
        &[
            "A simple call to action to move forward with {subject}.",
            "How to get started today.",
            "Who to reach out to for help.",
        ],
    ),
];

fn strip_list_marker(line: &str) -> &str {
    line.trim_start_matches(['-', '*', '•', '#', '>', '–'])
        .trim()
}

/// Upper bound on words kept for a generated title so a long run-on
/// brief can never blow past the title card. Generous enough that a
/// natural headline ("Pitch deck for an indie coffee roaster") is kept
/// whole.
const TITLE_MAX_WORDS: usize = 12;

/// Split a free-form brief into a concise title source plus a list of
/// talking points.
///
/// A multi-line brief keeps its first line as the title and folds the
/// remaining lines (list markers stripped) into points. A single
/// paragraph is segmented on sentence boundaries: the first sentence
/// (trimmed to its leading clause) becomes the title and every
/// remaining sentence is broken into comma-separated clauses that
/// become points. This way a one-line prompt yields a tight headline
/// plus on-topic content instead of dumping the whole paragraph into
/// the title card.
fn split_brief(brief: &str) -> (String, Vec<String>) {
    let mut lines = brief.lines().map(str::trim).filter(|l| !l.is_empty());
    let first = lines.next().unwrap_or("");
    let rest_lines: Vec<&str> = lines.collect();

    if !rest_lines.is_empty() {
        let points = rest_lines
            .into_iter()
            .map(strip_list_marker)
            .filter(|p| !p.is_empty())
            .map(ToString::to_string)
            .collect();
        return (leading_clause(first), points);
    }

    let sentences = split_sentences(first);
    let title = leading_clause(sentences.first().map_or(first, String::as_str));
    let points = sentences
        .iter()
        .skip(1)
        .flat_map(|s| split_clauses(s))
        .collect();
    (title, points)
}

/// Segment text into sentences on `.`/`!`/`?`, keeping non-empty
/// trimmed fragments. Abbreviations are not special-cased — the
/// planner only needs rough segmentation.
fn split_sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in text.chars() {
        if matches!(ch, '.' | '!' | '?') {
            let s = cur.trim();
            if !s.is_empty() {
                out.push(s.to_string());
            }
            cur.clear();
        } else {
            cur.push(ch);
        }
    }
    let s = cur.trim();
    if !s.is_empty() {
        out.push(s.to_string());
    }
    out
}

/// Break a sentence into clean talking points on commas, dropping a
/// leading conjunction and common lead-in verbs ("cover", "including",
/// …) so "Cover our origin story, the beans we source, and the ask"
/// folds into tidy phrases.
fn split_clauses(sentence: &str) -> Vec<String> {
    sentence
        .split(',')
        .map(|clause| {
            let clause = clause.trim();
            let clause = clause
                .strip_prefix("and ")
                .or_else(|| clause.strip_prefix("And "))
                .unwrap_or(clause)
                .trim();
            strip_leadin(clause)
        })
        .filter(|clause| clause.split_whitespace().count() >= 2)
        .map(ToString::to_string)
        .collect()
}

/// Strip a leading imperative lead-in verb from a clause so the
/// remaining phrase reads as a noun-led bullet.
fn strip_leadin(clause: &str) -> &str {
    const LEADINS: [&str; 12] = [
        "cover ",
        "covering ",
        "include ",
        "including ",
        "discuss ",
        "discussing ",
        "highlight ",
        "highlighting ",
        "explain ",
        "explaining ",
        "showcase ",
        "present ",
    ];
    let lower = clause.to_ascii_lowercase();
    for prefix in LEADINS {
        if lower.starts_with(prefix) {
            return clause[prefix.len()..].trim_start();
        }
    }
    clause
}

/// Reduce a sentence to its leading clause (up to the first comma) and
/// cap it at [`TITLE_MAX_WORDS`] so the title card never overflows.
fn leading_clause(sentence: &str) -> String {
    let head = sentence.split(',').next().unwrap_or(sentence).trim();
    head.split_whitespace()
        .take(TITLE_MAX_WORDS)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Clean a raw first line into a presentable title: strip markdown
/// heading marks and trailing punctuation, then title-case it when
/// the author typed it all in lower case.
fn clean_title(raw: &str) -> String {
    let cleaned = raw.trim().trim_start_matches('#').trim();
    let cleaned = cleaned.trim_end_matches(['.', ':', ';', ',']).trim();
    if cleaned.is_empty() {
        return "Untitled".to_string();
    }
    if cleaned.chars().any(char::is_uppercase) {
        cleaned.to_string()
    } else {
        title_case(cleaned)
    }
}

/// Derive a short subject phrase from a title by stripping leading
/// document-type prefixes ("pitch deck for", "a presentation on", …).
fn derive_subject(title: &str) -> String {
    const PREFIXES: [&str; 14] = [
        "pitch deck for ",
        "pitch deck about ",
        "pitch deck on ",
        "a pitch deck for ",
        "deck for ",
        "deck about ",
        "presentation for ",
        "presentation on ",
        "presentation about ",
        "a presentation on ",
        "one pager for ",
        "one-pager for ",
        "overview of ",
        "intro to ",
    ];
    let lower = title.to_ascii_lowercase();
    for p in PREFIXES {
        if let Some(stripped) = lower.strip_prefix(p) {
            // Map back onto the original casing by length.
            let start = title.len() - stripped.len();
            let subject = title[start..].trim();
            if !subject.is_empty() {
                return subject.to_string();
            }
        }
    }
    title.to_string()
}

fn title_case(s: &str) -> String {
    s.split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

// ---------------------------------------------------------------------------
// Stage 2 — layout
// ---------------------------------------------------------------------------

/// Lay out an outline into a fully themed, positioned design.
#[must_use]
pub fn generate_design(outline: &DeckOutline, options: ThemedDesignOptions) -> GeneratedDesign {
    let theme = theme(options.theme_id);
    let variety = Variety::derive(outline, &theme, options.format);
    // Type-scale variety is applied only to the image-bearing formats
    // (social / web / document); decks and one-pagers keep their exact
    // tuned sizes so their geometry never regresses. The scaled scale
    // is stored on the design, so the palette/scale invariant holds.
    let base_scale = TypeScale::for_format(options.format);
    let scale = if options.format.supports_imagery() {
        base_scale.scaled(variety.scale_factor)
    } else {
        base_scale
    };
    let pages = match options.format {
        DesignFormat::Deck => layout_deck(outline, &theme, &scale, variety),
        DesignFormat::OnePager => {
            layout_one_pager(outline, &theme, &scale, options.one_pager_size, variety)
        }
        DesignFormat::SocialPost => layout_social_post(outline, &theme, &scale, variety),
        DesignFormat::WebPage => layout_web_page(outline, &theme, &scale, variety),
        DesignFormat::Document => layout_document(outline, &theme, &scale, variety),
    };
    GeneratedDesign {
        theme,
        format: options.format,
        type_scale: scale,
        pages,
    }
}

const DECK_W: f64 = 1920.0;
const DECK_H: f64 = 1080.0;
const DECK_MARGIN: f64 = 120.0;
const DECK_CARD_PAD: f64 = 84.0;

fn layout_deck(
    outline: &DeckOutline,
    theme: &Theme,
    scale: &TypeScale,
    variety: Variety,
) -> Vec<GeneratedPage> {
    let mut pages = Vec::with_capacity(outline.slides.len() + 1);
    pages.push(title_slide(outline, theme, scale, variety));
    let total = outline.slides.len();
    for (i, slide) in outline.slides.iter().enumerate() {
        pages.push(content_slide(slide, theme, scale, i + 1, total, variety));
    }
    pages
}

fn title_slide(
    outline: &DeckOutline,
    theme: &Theme,
    scale: &TypeScale,
    variety: Variety,
) -> GeneratedPage {
    let mut elements = Vec::new();
    let content = Bounds::new(
        DECK_MARGIN,
        DECK_MARGIN,
        DECK_W - 2.0 * DECK_MARGIN,
        DECK_H - 2.0 * DECK_MARGIN,
    );

    // Accent bar above the title — a primary-coloured flourish.
    let accent_h = 14.0;
    let accent_w = 200.0;
    elements.push(rect(
        ElementRole::AccentBar,
        content.x,
        content.y + content.height * 0.30,
        accent_w,
        accent_h,
        variety.accent(theme),
        accent_h / 2.0,
    ));

    // Title + subtitle stacked, vertically centred via flex.
    let title_h = line_height(scale.title)
        * f64::from(wrap_count(&outline.title, scale.title, content.width));
    let sub_h = line_height(scale.subheading)
        * f64::from(wrap_count(
            &outline.subtitle,
            scale.subheading,
            content.width,
        ));
    let id_title = Uuid::new_v4();
    let id_sub = Uuid::new_v4();
    let stack = Bounds::new(
        content.x,
        content.y + content.height * 0.30 + accent_h + 28.0,
        content.width,
        content.height * 0.55,
    );
    let placed = layout_flex(
        stack,
        &[
            (id_title, content.width, title_h),
            (id_sub, content.width, sub_h),
        ],
        &FlexLayout {
            direction: FlexDirection::Column,
            spacing: 28.0,
            padding: Padding::default(),
            alignment: Alignment::Start,
            cross_alignment: CrossAlignment::Start,
            wrap: false,
        },
    );
    push_text_block(
        &mut elements,
        ElementRole::Title,
        &outline.title,
        bounds_for(&placed, id_title, stack),
        scale.title,
        &theme.heading_font,
        &theme.on_background,
    );
    push_text_block(
        &mut elements,
        ElementRole::Subtitle,
        &outline.subtitle,
        bounds_for(&placed, id_sub, stack),
        scale.subheading,
        &theme.body_font,
        &theme.muted,
    );

    GeneratedPage {
        title: outline.title.clone(),
        width: DECK_W,
        height: DECK_H,
        background: theme.background.clone(),
        elements,
    }
}

fn content_slide(
    slide: &SlideOutline,
    theme: &Theme,
    scale: &TypeScale,
    index: usize,
    total: usize,
    variety: Variety,
) -> GeneratedPage {
    let mut elements = Vec::new();
    let card = Bounds::new(
        DECK_MARGIN,
        DECK_MARGIN,
        DECK_W - 2.0 * DECK_MARGIN,
        DECK_H - 2.0 * DECK_MARGIN,
    );
    // Surface card behind the content for the Gamma "card" look.
    elements.push(rect(
        ElementRole::Surface,
        card.x,
        card.y,
        card.width,
        card.height,
        &theme.surface,
        28.0,
    ));

    let inner = Bounds::new(
        card.x + DECK_CARD_PAD,
        card.y + DECK_CARD_PAD,
        card.width - 2.0 * DECK_CARD_PAD,
        card.height - 2.0 * DECK_CARD_PAD,
    );

    // Accent bar at the top of the card.
    let accent_h = 12.0;
    elements.push(rect(
        ElementRole::AccentBar,
        inner.x,
        inner.y,
        140.0,
        accent_h,
        variety.accent(theme),
        accent_h / 2.0,
    ));

    let marker_indent = 48.0;
    let body_width = inner.width - marker_indent;
    let text_region = Bounds::new(
        inner.x,
        inner.y + accent_h + 36.0,
        inner.width,
        inner.height - accent_h - 36.0 - line_height(scale.caption) - 12.0,
    );

    // Build flex blocks: heading, then one block per bullet (whose
    // height accounts for wrapped lines).
    let heading_lines = wrap_count(&slide.heading, scale.heading, inner.width);
    let heading_h = line_height(scale.heading) * f64::from(heading_lines);
    let mut block_ids: Vec<Uuid> = Vec::with_capacity(slide.bullets.len() + 1);
    let mut sizes: Vec<(Uuid, f64, f64)> = Vec::with_capacity(slide.bullets.len() + 1);
    let id_heading = Uuid::new_v4();
    block_ids.push(id_heading);
    sizes.push((id_heading, inner.width, heading_h));

    let wrapped: Vec<Vec<String>> = slide
        .bullets
        .iter()
        .map(|b| wrap_text(b, scale.body, body_width))
        .collect();
    for (bi, lines) in wrapped.iter().enumerate() {
        let id = Uuid::new_v4();
        block_ids.push(id);
        let h = line_height(scale.body) * f64::from(lines.len().max(1) as u32);
        sizes.push((id, body_width, h));
        let _ = bi;
    }

    let placed = layout_flex(
        text_region,
        &sizes,
        &FlexLayout {
            direction: FlexDirection::Column,
            spacing: 26.0,
            padding: Padding::default(),
            alignment: Alignment::Start,
            cross_alignment: CrossAlignment::Start,
            wrap: false,
        },
    );

    // Heading.
    push_text_block(
        &mut elements,
        ElementRole::Heading,
        &slide.heading,
        bounds_for(&placed, id_heading, text_region),
        scale.heading,
        &theme.heading_font,
        &theme.heading,
    );

    // Bullets: marker rect + wrapped body lines.
    let body_lh = line_height(scale.body);
    for (bi, lines) in wrapped.iter().enumerate() {
        let block = bounds_for(&placed, block_ids[bi + 1], text_region);
        let marker = 14.0;
        elements.push(rect(
            ElementRole::BulletMarker,
            block.x,
            block.y + (body_lh - marker) / 2.0,
            marker,
            marker,
            variety.accent(theme),
            marker / 2.0,
        ));
        for (li, line) in lines.iter().enumerate() {
            elements.push(text(
                ElementRole::Body,
                block.x + marker_indent,
                block.y + body_lh * li as f64,
                body_width,
                body_lh,
                line,
                scale.body,
                &theme.body_font,
                &theme.body,
            ));
        }
    }

    // Footer: slide index + title-subject.
    elements.push(text(
        ElementRole::Footer,
        inner.x,
        card.y + card.height - DECK_CARD_PAD + 8.0,
        inner.width,
        line_height(scale.caption),
        &format!("{index:02} / {total:02}"),
        scale.caption,
        &theme.body_font,
        &theme.muted,
    ));

    GeneratedPage {
        title: slide.heading.clone(),
        width: DECK_W,
        height: DECK_H,
        background: theme.background.clone(),
        elements,
    }
}

fn layout_one_pager(
    outline: &DeckOutline,
    theme: &Theme,
    scale: &TypeScale,
    size: OnePagerSize,
    variety: Variety,
) -> Vec<GeneratedPage> {
    let (page_w, page_h) = size.dimensions();
    let margin = 64.0;
    let mut elements = Vec::new();
    let content = Bounds::new(margin, margin, page_w - 2.0 * margin, page_h - 2.0 * margin);

    // Accent bar + title block.
    let accent_h = 8.0;
    elements.push(rect(
        ElementRole::AccentBar,
        content.x,
        content.y,
        96.0,
        accent_h,
        variety.accent(theme),
        accent_h / 2.0,
    ));

    let marker_indent = 22.0;
    let body_width = content.width - marker_indent;

    // Flex blocks: title, subtitle, then per-section (heading + each
    // wrapped bullet line as its own block) so the whole page is
    // solver-driven and never hand-placed.
    let mut ids: Vec<Uuid> = Vec::new();
    let mut roles: Vec<(ElementRole, String, f32, bool)> = Vec::new(); // (role, text, size, is_bullet)
    let mut sizes: Vec<(Uuid, f64, f64)> = Vec::new();

    let push_block = |role: ElementRole,
                      content_text: &str,
                      font_size: f32,
                      is_bullet: bool,
                      width: f64,
                      ids: &mut Vec<Uuid>,
                      roles: &mut Vec<(ElementRole, String, f32, bool)>,
                      sizes: &mut Vec<(Uuid, f64, f64)>| {
        let id = Uuid::new_v4();
        let lines = wrap_count(content_text, font_size, width);
        ids.push(id);
        roles.push((role, content_text.to_string(), font_size, is_bullet));
        sizes.push((id, width, line_height(font_size) * f64::from(lines)));
    };

    push_block(
        ElementRole::Title,
        &outline.title,
        scale.title,
        false,
        content.width,
        &mut ids,
        &mut roles,
        &mut sizes,
    );
    push_block(
        ElementRole::Subtitle,
        &outline.subtitle,
        scale.subheading,
        false,
        content.width,
        &mut ids,
        &mut roles,
        &mut sizes,
    );
    for slide in &outline.slides {
        push_block(
            ElementRole::Heading,
            &slide.heading,
            scale.heading,
            false,
            content.width,
            &mut ids,
            &mut roles,
            &mut sizes,
        );
        for bullet in &slide.bullets {
            for line in wrap_text(bullet, scale.body, body_width) {
                push_block(
                    ElementRole::Body,
                    &line,
                    scale.body,
                    true,
                    body_width,
                    &mut ids,
                    &mut roles,
                    &mut sizes,
                );
            }
        }
    }

    let text_region = Bounds::new(
        content.x,
        content.y + accent_h + 20.0,
        content.width,
        content.height - accent_h - 20.0,
    );
    let placed = layout_flex(
        text_region,
        &sizes,
        &FlexLayout {
            direction: FlexDirection::Column,
            spacing: 10.0,
            padding: Padding::default(),
            alignment: Alignment::Start,
            cross_alignment: CrossAlignment::Start,
            wrap: false,
        },
    );

    let body_lh = line_height(scale.body);
    for (i, id) in ids.iter().enumerate() {
        let (role, content_text, font_size, is_bullet) = &roles[i];
        let b = bounds_for(&placed, *id, text_region);
        let color = match role {
            ElementRole::Title | ElementRole::Heading => &theme.heading,
            ElementRole::Subtitle => &theme.muted,
            _ => &theme.body,
        };
        let font = if matches!(role, ElementRole::Body | ElementRole::Subtitle) {
            &theme.body_font
        } else {
            &theme.heading_font
        };
        if *is_bullet {
            let marker = 8.0;
            elements.push(rect(
                ElementRole::BulletMarker,
                b.x,
                b.y + (body_lh - marker) / 2.0,
                marker,
                marker,
                variety.accent(theme),
                marker / 2.0,
            ));
            elements.push(text(
                *role,
                b.x + marker_indent,
                b.y,
                body_width,
                b.height,
                content_text,
                *font_size,
                font,
                color,
            ));
        } else {
            push_text_block(
                &mut elements,
                *role,
                content_text,
                b,
                *font_size,
                font,
                color,
            );
        }
    }

    vec![GeneratedPage {
        title: outline.title.clone(),
        width: page_w,
        height: page_h,
        background: theme.background.clone(),
        elements,
    }]
}

// ---------------------------------------------------------------------------
// Layout variety
// ---------------------------------------------------------------------------

/// Deterministic, content-derived knobs that keep repeated generations
/// from looking identical without ever compromising legibility. Derived
/// from a stable hash of the outline + theme + format, so the same brief
/// always yields the same design (fully offline-reproducible).
#[derive(Debug, Clone, Copy)]
struct Variety {
    /// Type-scale multiplier in a tasteful band (image-bearing formats).
    scale_factor: f32,
    /// Use the theme's secondary colour for accents instead of primary.
    use_secondary_accent: bool,
    /// Flip the hero composition (image-leading vs text-leading).
    alt_parity: bool,
    /// Which call-to-action / eyebrow label to use.
    cta_index: usize,
}

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn fnv1a(seed: u64, bytes: &[u8]) -> u64 {
    let mut h = seed;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(FNV_PRIME);
    }
    h
}

impl Variety {
    fn derive(outline: &DeckOutline, theme: &Theme, format: DesignFormat) -> Self {
        let mut h = fnv1a(FNV_OFFSET, outline.title.as_bytes());
        h = fnv1a(h, outline.subtitle.as_bytes());
        for slide in &outline.slides {
            h = fnv1a(h, slide.heading.as_bytes());
        }
        h = fnv1a(h, theme.id.wire().as_bytes());
        h = fnv1a(h, format.wire().as_bytes());
        let steps = [0.94_f32, 0.97, 1.0, 1.03, 1.06];
        let scale_factor = steps[(h % steps.len() as u64) as usize];
        Self {
            scale_factor,
            use_secondary_accent: (h >> 8) & 1 == 1,
            alt_parity: (h >> 9) & 1 == 1,
            cta_index: ((h >> 10) % 3) as usize,
        }
    }

    /// The accent colour for this design — primary, or the theme's
    /// secondary when variety calls for it. Only ever applied to
    /// non-text elements (bars, markers), so it never affects the
    /// text-palette invariant.
    fn accent<'a>(&self, theme: &'a Theme) -> &'a str {
        if self.use_secondary_accent {
            &theme.secondary
        } else {
            &theme.primary
        }
    }
}

// ---------------------------------------------------------------------------
// Shared layout helpers for the image-bearing formats
// ---------------------------------------------------------------------------

/// One line-group in a vertical text stack.
struct TextBlock<'a> {
    role: ElementRole,
    text: &'a str,
    font_size: f32,
    font: &'a str,
    fill: &'a str,
}

/// Emit a sequence of wrapped text blocks stacked vertically from
/// `region.y`, advancing by each block's wrapped height plus `spacing`.
/// Returns the y just past the last line, so callers place whatever
/// comes next without overlap. Width comes from `region.width`.
fn stack_text(
    out: &mut Vec<DesignElement>,
    region: Bounds,
    spacing: f64,
    blocks: &[TextBlock],
) -> f64 {
    let mut y = region.y;
    for (i, block) in blocks.iter().enumerate() {
        if i > 0 {
            y += spacing;
        }
        let lines = wrap_text(block.text, block.font_size, region.width);
        let lh = line_height(block.font_size);
        for (li, line) in lines.iter().enumerate() {
            out.push(text(
                block.role,
                region.x,
                y + lh * li as f64,
                region.width,
                lh,
                line,
                block.font_size,
                block.font,
                block.fill,
            ));
        }
        y += lh * lines.len() as f64;
    }
    y
}

const CTA_LABELS: [&str; 3] = ["Learn more", "Get started", "See how it works"];
const WEB_EYEBROWS: [&str; 3] = ["INTRODUCING", "PRESENTING", "NOW AVAILABLE"];

fn cta_label(variety: Variety) -> &'static str {
    CTA_LABELS[variety.cta_index % CTA_LABELS.len()]
}

fn eyebrow_label(variety: Variety) -> &'static str {
    WEB_EYEBROWS[variety.cta_index % WEB_EYEBROWS.len()]
}

/// Push a rounded "pill" call-to-action (primary fill + on-background
/// label) with its top-left at (x, y). Width fits the label within a
/// tasteful band. Returns the pill height so callers can advance.
fn push_cta_pill(
    out: &mut Vec<DesignElement>,
    theme: &Theme,
    scale: &TypeScale,
    x: f64,
    y: f64,
    label: &str,
) -> f64 {
    let pad_x = 44.0;
    let lh = line_height(scale.body);
    let h = (lh + 44.0).max(72.0);
    let text_w = f64::from(scale.body) * AVG_CHAR_W * label.chars().count() as f64;
    let w = (text_w + pad_x * 2.0).clamp(220.0, 560.0);
    out.push(rect(
        ElementRole::Surface,
        x,
        y,
        w,
        h,
        &theme.primary,
        h / 2.0,
    ));
    out.push(text(
        ElementRole::Body,
        x + pad_x,
        y + (h - lh) / 2.0,
        w - 2.0 * pad_x,
        lh,
        label,
        scale.body,
        &theme.body_font,
        &theme.on_background,
    ));
    h
}

/// Build the diffusion prompt for a hero/section image. Honest and
/// fully deterministic; the bridge feeds it to the local sidecar when a
/// model is present and otherwise falls back to a gradient placeholder.
fn hero_prompt(outline: &DeckOutline, theme: &Theme, context: &str) -> String {
    format!(
        "{context} for {}, {} colour palette, professional, clean composition, high quality",
        outline.title.trim(),
        theme.name.to_ascii_lowercase()
    )
}

// ---------------------------------------------------------------------------
// Format: Social post set (square feed tile + 9:16 story)
// ---------------------------------------------------------------------------

const SOCIAL_SQUARE: f64 = 1080.0;
const SOCIAL_STORY_W: f64 = 1080.0;
const SOCIAL_STORY_H: f64 = 1920.0;
const SOCIAL_MARGIN: f64 = 84.0;

fn layout_social_post(
    outline: &DeckOutline,
    theme: &Theme,
    scale: &TypeScale,
    variety: Variety,
) -> Vec<GeneratedPage> {
    vec![
        social_square(outline, theme, scale, variety),
        social_story(outline, theme, scale, variety),
    ]
}

fn social_square(
    outline: &DeckOutline,
    theme: &Theme,
    scale: &TypeScale,
    variety: Variety,
) -> GeneratedPage {
    let mut elements = Vec::new();
    let m = SOCIAL_MARGIN;
    let region_w = SOCIAL_SQUARE - 2.0 * m;

    // Full-bleed hero band (diffusion target; gradient fallback).
    let img_h = 540.0;
    elements.push(image(
        0.0,
        0.0,
        SOCIAL_SQUARE,
        img_h,
        &theme.primary,
        &theme.secondary,
        0.0,
        Some(hero_prompt(outline, theme, "social media hero image")),
    ));

    // Accent flourish + punchy headline beneath the image.
    let accent_h = 14.0;
    let accent_y = img_h + 56.0;
    elements.push(rect(
        ElementRole::AccentBar,
        m,
        accent_y,
        132.0,
        accent_h,
        variety.accent(theme),
        accent_h / 2.0,
    ));
    let after = stack_text(
        &mut elements,
        Bounds::new(m, accent_y + accent_h + 30.0, region_w, 0.0),
        16.0,
        &[
            TextBlock {
                role: ElementRole::Heading,
                text: &outline.title,
                font_size: scale.heading,
                font: &theme.heading_font,
                fill: &theme.heading,
            },
            TextBlock {
                role: ElementRole::Subtitle,
                text: &outline.subtitle,
                font_size: scale.body,
                font: &theme.body_font,
                fill: &theme.muted,
            },
        ],
    );

    push_cta_pill(
        &mut elements,
        theme,
        scale,
        m,
        after + 44.0,
        cta_label(variety),
    );

    GeneratedPage {
        title: format!("{} — Post", outline.title),
        width: SOCIAL_SQUARE,
        height: SOCIAL_SQUARE,
        background: theme.background.clone(),
        elements,
    }
}

fn social_story(
    outline: &DeckOutline,
    theme: &Theme,
    scale: &TypeScale,
    variety: Variety,
) -> GeneratedPage {
    let mut elements = Vec::new();
    let m = SOCIAL_MARGIN;
    let region_w = SOCIAL_STORY_W - 2.0 * m;
    let accent = variety.accent(theme);

    let img_h = 720.0;
    elements.push(image(
        0.0,
        0.0,
        SOCIAL_STORY_W,
        img_h,
        &theme.primary,
        &theme.secondary,
        0.0,
        Some(hero_prompt(
            outline,
            theme,
            "vertical social story hero image",
        )),
    ));

    let mut y = img_h + 64.0;
    let accent_h = 16.0;
    elements.push(rect(
        ElementRole::AccentBar,
        m,
        y,
        148.0,
        accent_h,
        accent,
        accent_h / 2.0,
    ));
    y += accent_h + 36.0;

    let after = stack_text(
        &mut elements,
        Bounds::new(m, y, region_w, 0.0),
        20.0,
        &[
            TextBlock {
                role: ElementRole::Title,
                text: &outline.title,
                font_size: scale.title,
                font: &theme.heading_font,
                fill: &theme.on_background,
            },
            TextBlock {
                role: ElementRole::Subtitle,
                text: &outline.subtitle,
                font_size: scale.subheading,
                font: &theme.body_font,
                fill: &theme.muted,
            },
        ],
    );
    y = after + 44.0;

    // Highlight bullets drawn from the section headings.
    let body_lh = line_height(scale.body);
    let marker = 18.0;
    let bullet_x = m + 44.0;
    let bullet_w = region_w - 44.0;
    for slide in outline.slides.iter().take(3) {
        elements.push(rect(
            ElementRole::BulletMarker,
            m,
            y + (body_lh - marker) / 2.0,
            marker,
            marker,
            accent,
            marker / 2.0,
        ));
        let lines = wrap_text(&slide.heading, scale.body, bullet_w);
        for (li, line) in lines.iter().enumerate() {
            elements.push(text(
                ElementRole::Body,
                bullet_x,
                y + body_lh * li as f64,
                bullet_w,
                body_lh,
                line,
                scale.body,
                &theme.body_font,
                &theme.body,
            ));
        }
        y += body_lh * lines.len() as f64 + 18.0;
    }

    push_cta_pill(&mut elements, theme, scale, m, y + 24.0, cta_label(variety));

    GeneratedPage {
        title: format!("{} — Story", outline.title),
        width: SOCIAL_STORY_W,
        height: SOCIAL_STORY_H,
        background: theme.background.clone(),
        elements,
    }
}

// ---------------------------------------------------------------------------
// Format: Web page (hero + feature grid + CTA band)
// ---------------------------------------------------------------------------

const WEB_W: f64 = 1440.0;
const WEB_MARGIN: f64 = 100.0;
const WEB_GAP: f64 = 40.0;

fn layout_web_page(
    outline: &DeckOutline,
    theme: &Theme,
    scale: &TypeScale,
    variety: Variety,
) -> Vec<GeneratedPage> {
    let mut elements = Vec::new();
    let content_w = WEB_W - 2.0 * WEB_MARGIN;
    let hero_img_h = 460.0;
    let mut y = 96.0;

    // Hero composition flips with variety: image-leading or text-leading.
    let text_first = variety.alt_parity;
    if !text_first {
        elements.push(image(
            WEB_MARGIN,
            y,
            content_w,
            hero_img_h,
            &theme.primary,
            &theme.secondary,
            28.0,
            Some(hero_prompt(outline, theme, "website hero image")),
        ));
        y += hero_img_h + 64.0;
    }

    let cap_lh = line_height(scale.caption);
    elements.push(text(
        ElementRole::Body,
        WEB_MARGIN,
        y,
        content_w,
        cap_lh,
        eyebrow_label(variety),
        scale.caption,
        &theme.body_font,
        &theme.primary,
    ));
    y += cap_lh + 18.0;

    let after = stack_text(
        &mut elements,
        Bounds::new(WEB_MARGIN, y, content_w, 0.0),
        22.0,
        &[
            TextBlock {
                role: ElementRole::Title,
                text: &outline.title,
                font_size: scale.title,
                font: &theme.heading_font,
                fill: &theme.heading,
            },
            TextBlock {
                role: ElementRole::Subtitle,
                text: &outline.subtitle,
                font_size: scale.subheading,
                font: &theme.body_font,
                fill: &theme.body,
            },
        ],
    );
    let cta_h = push_cta_pill(
        &mut elements,
        theme,
        scale,
        WEB_MARGIN,
        after + 36.0,
        cta_label(variety),
    );
    y = after + 36.0 + cta_h;

    if text_first {
        y += 64.0;
        elements.push(image(
            WEB_MARGIN,
            y,
            content_w,
            hero_img_h,
            &theme.primary,
            &theme.secondary,
            28.0,
            Some(hero_prompt(outline, theme, "website hero image")),
        ));
        y += hero_img_h;
    }

    // Feature grid.
    y += 88.0;
    let sec_lh = line_height(scale.caption);
    elements.push(text(
        ElementRole::Body,
        WEB_MARGIN,
        y,
        content_w,
        sec_lh,
        "WHAT YOU GET",
        scale.caption,
        &theme.body_font,
        &theme.primary,
    ));
    y += sec_lh + 30.0;

    let n = outline.slides.len().clamp(1, 6);
    let cols = n.min(3);
    let rows = n.div_ceil(cols);
    let card_w = (content_w - WEB_GAP * (cols as f64 - 1.0)) / cols as f64;
    let card_h = 460.0;
    let grid_top = y;
    let pad = 32.0;
    let panel_h = 176.0;
    for (i, slide) in outline.slides.iter().take(n).enumerate() {
        let r = i / cols;
        let c = i % cols;
        let cx = WEB_MARGIN + c as f64 * (card_w + WEB_GAP);
        let cy = grid_top + r as f64 * (card_h + WEB_GAP);
        elements.push(rect(
            ElementRole::Surface,
            cx,
            cy,
            card_w,
            card_h,
            &theme.surface,
            24.0,
        ));
        // Decorative gradient panel (no prompt → always a placeholder).
        elements.push(image(
            cx + pad,
            cy + pad,
            card_w - 2.0 * pad,
            panel_h,
            &theme.secondary,
            &theme.primary,
            16.0,
            None,
        ));
        let tw = card_w - 2.0 * pad;
        let body_line = slide
            .bullets
            .first()
            .map_or(slide.heading.as_str(), String::as_str);
        stack_text(
            &mut elements,
            Bounds::new(cx + pad, cy + pad + panel_h + 24.0, tw, 0.0),
            12.0,
            &[
                TextBlock {
                    role: ElementRole::Heading,
                    text: &slide.heading,
                    font_size: scale.heading,
                    font: &theme.heading_font,
                    fill: &theme.heading,
                },
                TextBlock {
                    role: ElementRole::Body,
                    text: body_line,
                    font_size: scale.body,
                    font: &theme.body_font,
                    fill: &theme.body,
                },
            ],
        );
    }
    y = grid_top + rows as f64 * card_h + (rows as f64 - 1.0) * WEB_GAP;

    // Call-to-action band.
    y += 72.0;
    let band_h = 300.0;
    elements.push(rect(
        ElementRole::Surface,
        WEB_MARGIN,
        y,
        content_w,
        band_h,
        &theme.surface,
        32.0,
    ));
    let band_pad = 64.0;
    let bx = WEB_MARGIN + band_pad;
    let bw = content_w - 2.0 * band_pad;
    let after = stack_text(
        &mut elements,
        Bounds::new(bx, y + band_pad, bw, 0.0),
        16.0,
        &[
            TextBlock {
                role: ElementRole::Heading,
                text: "Ready to get started?",
                font_size: scale.heading,
                font: &theme.heading_font,
                fill: &theme.heading,
            },
            TextBlock {
                role: ElementRole::Body,
                text: &outline.subtitle,
                font_size: scale.body,
                font: &theme.body_font,
                fill: &theme.muted,
            },
        ],
    );
    push_cta_pill(
        &mut elements,
        theme,
        scale,
        bx,
        after + 28.0,
        cta_label(variety),
    );
    y += band_h;

    vec![GeneratedPage {
        title: outline.title.clone(),
        width: WEB_W,
        height: y + 96.0,
        background: theme.background.clone(),
        elements,
    }]
}

// ---------------------------------------------------------------------------
// Format: Document / report (cover + paginated body)
// ---------------------------------------------------------------------------

const DOC_W: f64 = 816.0;
const DOC_H: f64 = 1056.0;
const DOC_MARGIN: f64 = 72.0;

fn layout_document(
    outline: &DeckOutline,
    theme: &Theme,
    scale: &TypeScale,
    variety: Variety,
) -> Vec<GeneratedPage> {
    let mut pages = vec![doc_cover(outline, theme, scale, variety)];

    let content_w = DOC_W - 2.0 * DOC_MARGIN;
    let body_top = DOC_MARGIN + 44.0;
    let body_bottom = DOC_H - DOC_MARGIN - 24.0;

    // Greedy pagination: pack whole sections onto pages. A page is empty
    // exactly when its cursor is still at `body_top`, so a single
    // oversized section never triggers a spurious page break.
    let mut page_groups: Vec<Vec<&SlideOutline>> = vec![Vec::new()];
    let mut y = body_top;
    for slide in &outline.slides {
        let sh = section_height(slide, scale, content_w);
        if y > body_top && y + sh > body_bottom {
            page_groups.push(Vec::new());
            y = body_top;
        }
        if let Some(group) = page_groups.last_mut() {
            group.push(slide);
        }
        y += sh + 30.0;
    }

    let total_pages = page_groups.len() + 1;
    for (i, group) in page_groups.iter().enumerate() {
        pages.push(doc_body_page(
            group,
            theme,
            scale,
            &outline.title,
            i + 2,
            total_pages,
            variety,
        ));
    }
    pages
}

/// Measured height of a body section (heading + accent rule + gaps +
/// wrapped paragraphs), used to drive pagination.
fn section_height(slide: &SlideOutline, scale: &TypeScale, width: f64) -> f64 {
    let mut h =
        line_height(scale.heading) * f64::from(wrap_count(&slide.heading, scale.heading, width));
    h += 8.0 + 4.0 + 16.0; // accent rule + gaps
    for bullet in &slide.bullets {
        h += line_height(scale.body) * f64::from(wrap_count(bullet, scale.body, width)) + 10.0;
    }
    h
}

fn doc_cover(
    outline: &DeckOutline,
    theme: &Theme,
    scale: &TypeScale,
    variety: Variety,
) -> GeneratedPage {
    let mut elements = Vec::new();
    let m = DOC_MARGIN;
    let region_w = DOC_W - 2.0 * m;

    let img_h = 380.0;
    elements.push(image(
        0.0,
        0.0,
        DOC_W,
        img_h,
        &theme.primary,
        &theme.secondary,
        0.0,
        Some(hero_prompt(outline, theme, "report cover illustration")),
    ));

    let accent_h = 10.0;
    let mut y = img_h + 80.0;
    elements.push(rect(
        ElementRole::AccentBar,
        m,
        y,
        120.0,
        accent_h,
        variety.accent(theme),
        accent_h / 2.0,
    ));
    y += accent_h + 36.0;

    let after = stack_text(
        &mut elements,
        Bounds::new(m, y, region_w, 0.0),
        20.0,
        &[
            TextBlock {
                role: ElementRole::Title,
                text: &outline.title,
                font_size: scale.title,
                font: &theme.heading_font,
                fill: &theme.heading,
            },
            TextBlock {
                role: ElementRole::Subtitle,
                text: &outline.subtitle,
                font_size: scale.subheading,
                font: &theme.body_font,
                fill: &theme.muted,
            },
        ],
    );

    elements.push(text(
        ElementRole::Footer,
        m,
        after + 40.0,
        region_w,
        line_height(scale.caption),
        "Prepared with KCreate",
        scale.caption,
        &theme.body_font,
        &theme.muted,
    ));

    GeneratedPage {
        title: format!("{} — Cover", outline.title),
        width: DOC_W,
        height: DOC_H,
        background: theme.background.clone(),
        elements,
    }
}

#[allow(clippy::too_many_arguments)]
fn doc_body_page(
    sections: &[&SlideOutline],
    theme: &Theme,
    scale: &TypeScale,
    doc_title: &str,
    page_no: usize,
    total_pages: usize,
    variety: Variety,
) -> GeneratedPage {
    let mut elements = Vec::new();
    let m = DOC_MARGIN;
    let content_w = DOC_W - 2.0 * m;
    let accent = variety.accent(theme);

    // Running header + rule.
    elements.push(text(
        ElementRole::Footer,
        m,
        m - 38.0,
        content_w,
        line_height(scale.caption),
        doc_title,
        scale.caption,
        &theme.body_font,
        &theme.muted,
    ));
    elements.push(rect(
        ElementRole::AccentBar,
        m,
        m - 6.0,
        content_w,
        2.0,
        accent,
        0.0,
    ));

    let mut y = m + 44.0;
    for slide in sections {
        let after_h = stack_text(
            &mut elements,
            Bounds::new(m, y, content_w, 0.0),
            0.0,
            &[TextBlock {
                role: ElementRole::Heading,
                text: &slide.heading,
                font_size: scale.heading,
                font: &theme.heading_font,
                fill: &theme.heading,
            }],
        );
        elements.push(rect(
            ElementRole::AccentBar,
            m,
            after_h + 8.0,
            64.0,
            4.0,
            accent,
            2.0,
        ));
        y = after_h + 8.0 + 4.0 + 16.0;

        let blocks: Vec<TextBlock> = slide
            .bullets
            .iter()
            .map(|b| TextBlock {
                role: ElementRole::Body,
                text: b.as_str(),
                font_size: scale.body,
                font: &theme.body_font,
                fill: &theme.body,
            })
            .collect();
        let after_b = stack_text(
            &mut elements,
            Bounds::new(m, y, content_w, 0.0),
            10.0,
            &blocks,
        );
        y = after_b + 30.0;
    }

    let footer = format!("Page {page_no} of {total_pages}");
    elements.push(text(
        ElementRole::Footer,
        m,
        DOC_H - m + 6.0,
        content_w,
        line_height(scale.caption),
        &footer,
        scale.caption,
        &theme.body_font,
        &theme.muted,
    ));

    GeneratedPage {
        title: format!("Page {page_no}"),
        width: DOC_W,
        height: DOC_H,
        background: theme.background.clone(),
        elements,
    }
}

// ---------------------------------------------------------------------------
// small helpers
// ---------------------------------------------------------------------------

/// Approximate line height for a font size (1.35×).
fn line_height(font_size: f32) -> f64 {
    f64::from(font_size) * 1.35
}

/// Average glyph advance as a fraction of the font size. Tuned for a
/// proportional sans (Inter/DejaVu) so wrapping looks natural.
const AVG_CHAR_W: f64 = 0.52;

/// Greedy word-wrap `text` to fit `width` at `font_size`, returning
/// one string per visual line (never empty — at least one line).
fn wrap_text(text: &str, font_size: f32, width: f64) -> Vec<String> {
    let max_chars = ((width / (f64::from(font_size) * AVG_CHAR_W)).floor() as usize).max(8);
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if current.is_empty() {
            current.push_str(word);
        } else if current.chars().count() + 1 + word.chars().count() <= max_chars {
            current.push(' ');
            current.push_str(word);
        } else {
            lines.push(std::mem::take(&mut current));
            current.push_str(word);
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(text.to_string());
    }
    lines
}

/// Number of visual lines `text` wraps to at `font_size`/`width`.
fn wrap_count(text: &str, font_size: f32, width: f64) -> u32 {
    wrap_text(text, font_size, width).len().max(1) as u32
}

fn bounds_for(placed: &[(Uuid, Bounds)], id: Uuid, fallback: Bounds) -> Bounds {
    placed
        .iter()
        .find(|(pid, _)| *pid == id)
        .map_or(fallback, |(_, b)| *b)
}

#[allow(clippy::too_many_arguments)]
fn text(
    role: ElementRole,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    body: &str,
    font_size: f32,
    font_family: &str,
    fill: &str,
) -> DesignElement {
    DesignElement {
        role,
        kind: ElementKind::Text,
        x,
        y,
        width,
        height,
        text: Some(body.to_string()),
        font_size,
        font_family: font_family.to_string(),
        fill: fill.to_string(),
        corner_radius: 0.0,
        image_prompt: None,
        fill_secondary: None,
    }
}

/// Push a (possibly multi-line) text block as one text element per
/// wrapped line so SVG/canvas — which do not auto-wrap a single
/// `<text>` — render real paragraphs.
#[allow(clippy::too_many_arguments)]
fn push_text_block(
    out: &mut Vec<DesignElement>,
    role: ElementRole,
    body: &str,
    bounds: Bounds,
    font_size: f32,
    font_family: &str,
    fill: &str,
) {
    let lh = line_height(font_size);
    for (i, line) in wrap_text(body, font_size, bounds.width).iter().enumerate() {
        out.push(text(
            role,
            bounds.x,
            bounds.y + lh * i as f64,
            bounds.width,
            lh,
            line,
            font_size,
            font_family,
            fill,
        ));
    }
}

fn rect(
    role: ElementRole,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    fill: &str,
    corner_radius: f64,
) -> DesignElement {
    DesignElement {
        role,
        kind: ElementKind::Rect,
        x,
        y,
        width,
        height,
        text: None,
        font_size: 0.0,
        font_family: String::new(),
        fill: fill.to_string(),
        corner_radius,
        image_prompt: None,
        fill_secondary: None,
    }
}

/// Build an image element. `fill`→`fill_secondary` is the gradient
/// used as the offline placeholder (and as the diffusion fallback);
/// `image_prompt` (when `Some`) is the prompt the bridge sends to the
/// local diffusion sidecar.
#[allow(clippy::too_many_arguments)]
fn image(
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    fill: &str,
    fill_secondary: &str,
    corner_radius: f64,
    image_prompt: Option<String>,
) -> DesignElement {
    DesignElement {
        role: ElementRole::Figure,
        kind: ElementKind::Image,
        x,
        y,
        width,
        height,
        text: None,
        font_size: 0.0,
        font_family: String::new(),
        fill: fill.to_string(),
        corner_radius,
        image_prompt,
        fill_secondary: Some(fill_secondary.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overlaps(a: &DesignElement, b: &DesignElement) -> bool {
        // A shared edge (e.g. consecutive paragraph lines stacked exactly one
        // line-height apart) is adjacency, not overlap; require more than a
        // sub-pixel intersection on both axes so float dust never trips it.
        const EPS: f64 = 0.05;
        a.x + EPS < b.x + b.width
            && b.x + EPS < a.x + a.width
            && a.y + EPS < b.y + b.height
            && b.y + EPS < a.y + a.height
    }

    #[test]
    fn empty_brief_errors() {
        let err = outline_from_brief("   \n  ", ThemedDesignOptions::default()).unwrap_err();
        assert_eq!(err, ThemedDesignError::Empty);
    }

    #[test]
    fn deck_has_title_plus_content_slides() {
        let opts = ThemedDesignOptions {
            section_count: Some(6),
            ..Default::default()
        };
        let outline = outline_from_brief("pitch deck for an indie coffee roaster", opts).unwrap();
        let design = generate_design(&outline, opts);
        // Title card + 6 content cards.
        assert_eq!(design.pages.len(), 7);
        // First page carries a Title element.
        assert!(design.pages[0]
            .elements
            .iter()
            .any(|e| e.role == ElementRole::Title));
        // Every content page has a heading and at least one body line.
        for page in &design.pages[1..] {
            assert!(page.elements.iter().any(|e| e.role == ElementRole::Heading));
            assert!(page.elements.iter().any(|e| e.role == ElementRole::Body));
        }
    }

    #[test]
    fn pitch_brief_uses_pitch_sections() {
        let opts = ThemedDesignOptions {
            section_count: Some(4),
            ..Default::default()
        };
        let outline = outline_from_brief("Pitch deck for a fintech startup", opts).unwrap();
        assert_eq!(outline.slides[0].heading, "The problem");
        assert!(outline.subtitle.starts_with("Investor pitch"));
    }

    #[test]
    fn generic_brief_uses_generic_sections() {
        let opts = ThemedDesignOptions {
            section_count: Some(4),
            ..Default::default()
        };
        let outline = outline_from_brief("Overview of our 2026 marketing plan", opts).unwrap();
        assert_eq!(outline.slides[0].heading, "Overview");
    }

    #[test]
    fn subject_prefix_is_stripped() {
        assert_eq!(
            derive_subject("Pitch deck for an indie coffee roaster"),
            "an indie coffee roaster"
        );
        assert_eq!(derive_subject("Overview of our launch"), "our launch");
    }

    #[test]
    fn lowercase_title_is_title_cased() {
        let outline =
            outline_from_brief("indie coffee roaster", ThemedDesignOptions::default()).unwrap();
        assert_eq!(outline.title, "Indie Coffee Roaster");
    }

    #[test]
    fn user_points_are_folded_into_bullets() {
        let brief =
            "Coffee roaster deck\n- We source single-origin beans\n- Roasted in small batches";
        let opts = ThemedDesignOptions {
            section_count: Some(3),
            ..Default::default()
        };
        let outline = outline_from_brief(brief, opts).unwrap();
        let all_bullets: Vec<&String> = outline.slides.iter().flat_map(|s| &s.bullets).collect();
        assert!(all_bullets
            .iter()
            .any(|b| b.contains("single-origin beans")));
        assert!(all_bullets.iter().any(|b| b.contains("small batches")));
    }

    #[test]
    fn single_paragraph_brief_yields_concise_title_and_folds_points() {
        let brief = "Pitch deck for an indie coffee roaster. Cover our origin \
                     story, the single-origin beans we source, our small-batch \
                     roasting process, the neighborhood cafe experience, early \
                     traction, and the investment ask.";
        let opts = ThemedDesignOptions {
            section_count: Some(6),
            ..Default::default()
        };
        let outline = outline_from_brief(brief, opts).unwrap();
        // The title is the leading headline, NOT the whole paragraph.
        assert_eq!(outline.title, "Pitch deck for an indie coffee roaster");
        assert!(outline.title.split_whitespace().count() <= TITLE_MAX_WORDS);
        // The trailing sentence's clauses fold into slide bullets, with
        // lead-in verbs and conjunctions stripped.
        let all_bullets: Vec<&String> = outline.slides.iter().flat_map(|s| &s.bullets).collect();
        assert!(all_bullets.iter().any(|b| b.contains("origin story")));
        assert!(all_bullets
            .iter()
            .any(|b| b.contains("single-origin beans")));
        assert!(all_bullets.iter().any(|b| b.contains("investment ask")));
        // No folded bullet keeps the "Cover " lead-in or the trailing
        // "and " conjunction.
        assert!(!all_bullets.iter().any(|b| b.starts_with("Cover ")));
        assert!(!all_bullets.iter().any(|b| b.starts_with("and ")));
    }

    #[test]
    fn long_run_on_title_is_capped_to_fit() {
        let brief = "An exhaustive end to end overview of our brand new fully \
                     integrated cloud native analytics platform launch";
        let outline = outline_from_brief(brief, ThemedDesignOptions::default()).unwrap();
        assert!(
            outline.title.split_whitespace().count() <= TITLE_MAX_WORDS,
            "title not capped: {}",
            outline.title
        );
    }

    #[test]
    fn title_never_overflows_title_card_for_paragraph_brief() {
        let brief = "Pitch deck for an indie coffee roaster. Cover our origin \
                     story, the single-origin beans we source, our small-batch \
                     roasting process, the neighborhood cafe experience, early \
                     traction, and the investment ask.";
        let opts = ThemedDesignOptions {
            section_count: Some(6),
            ..Default::default()
        };
        let outline = outline_from_brief(brief, opts).unwrap();
        let design = generate_design(&outline, opts);
        let title_page = &design.pages[0];
        for el in &title_page.elements {
            assert!(
                el.y + el.height <= title_page.height + 1.0,
                "title-card element overflows page bottom: {el:?}"
            );
        }
    }

    #[test]
    fn every_text_element_uses_theme_palette_and_scale() {
        for id in ThemeId::all() {
            let opts = ThemedDesignOptions {
                theme_id: id,
                section_count: Some(8),
                ..Default::default()
            };
            let outline = outline_from_brief("Pitch deck for a coffee roaster", opts).unwrap();
            let design = generate_design(&outline, opts);
            let th = &design.theme;
            let allowed_colors: Vec<String> =
                th.text_colors().iter().copied().map(String::from).collect();
            let allowed_sizes = design.type_scale.all();
            for page in &design.pages {
                for el in &page.elements {
                    if el.kind != ElementKind::Text {
                        continue;
                    }
                    assert!(
                        allowed_colors.contains(&el.fill),
                        "text fill {} not in theme palette {:?}",
                        el.fill,
                        allowed_colors
                    );
                    assert!(
                        allowed_sizes
                            .iter()
                            .any(|s| (*s - el.font_size).abs() < 0.01),
                        "font size {} not in type scale {:?}",
                        el.font_size,
                        allowed_sizes
                    );
                    assert!(el.font_family == th.heading_font || el.font_family == th.body_font);
                }
            }
        }
    }

    #[test]
    fn text_elements_do_not_overlap_each_other() {
        let opts = ThemedDesignOptions {
            section_count: Some(7),
            ..Default::default()
        };
        let outline = outline_from_brief(
            "Pitch deck for an indie coffee roaster with great espresso",
            opts,
        )
        .unwrap();
        let design = generate_design(&outline, opts);
        for page in &design.pages {
            let texts: Vec<&DesignElement> = page
                .elements
                .iter()
                .filter(|e| e.kind == ElementKind::Text)
                .collect();
            for i in 0..texts.len() {
                for j in (i + 1)..texts.len() {
                    assert!(
                        !overlaps(texts[i], texts[j]),
                        "text overlap on page '{}':\n  {:?}\n  {:?}",
                        page.title,
                        texts[i],
                        texts[j],
                    );
                }
            }
        }
    }

    #[test]
    fn all_elements_stay_within_page_bounds() {
        let opts = ThemedDesignOptions {
            section_count: Some(6),
            ..Default::default()
        };
        let outline = outline_from_brief("Pitch deck for a coffee roaster", opts).unwrap();
        let design = generate_design(&outline, opts);
        for page in &design.pages {
            for el in &page.elements {
                assert!(el.x >= -0.5 && el.y >= -0.5, "element off top-left: {el:?}");
                assert!(
                    el.x + el.width <= page.width + 1.0,
                    "element past right edge: {el:?}"
                );
                // Text may extend slightly via descenders; allow a row.
                assert!(el.y <= page.height + 1.0, "element below page: {el:?}");
            }
        }
    }

    #[test]
    fn one_pager_is_single_page_with_sections() {
        let opts = ThemedDesignOptions {
            format: DesignFormat::OnePager,
            section_count: Some(4),
            ..Default::default()
        };
        let outline = outline_from_brief("One pager for a coffee roaster", opts).unwrap();
        let design = generate_design(&outline, opts);
        assert_eq!(design.pages.len(), 1);
        let page = &design.pages[0];
        let (w, h) = OnePagerSize::A4.dimensions();
        assert!((page.width - w).abs() < 1.0);
        assert!((page.height - h).abs() < 1.0);
        assert!(page.elements.iter().any(|e| e.role == ElementRole::Title));
        assert!(page.elements.iter().any(|e| e.role == ElementRole::Heading));
        assert!(page.elements.iter().any(|e| e.role == ElementRole::Body));
    }

    #[test]
    fn section_count_is_clamped() {
        let low = ThemedDesignOptions {
            section_count: Some(1),
            ..Default::default()
        };
        assert_eq!(low.resolved_section_count(), 3);
        let high = ThemedDesignOptions {
            section_count: Some(99),
            ..Default::default()
        };
        assert_eq!(high.resolved_section_count(), 11);
    }

    #[test]
    fn sanitize_outline_drops_empty_and_keeps_good() {
        let outline = DeckOutline {
            title: "  ".to_string(),
            subtitle: String::new(),
            slides: vec![
                SlideOutline {
                    heading: "  ".to_string(),
                    bullets: vec!["x".to_string()],
                },
                SlideOutline {
                    heading: "Real heading".to_string(),
                    bullets: vec!["  ".to_string(), "kept".to_string()],
                },
            ],
        };
        let cleaned = sanitize_outline(outline, "Fallback Title").unwrap();
        assert_eq!(cleaned.title, "Fallback Title");
        assert_eq!(cleaned.slides.len(), 1);
        assert_eq!(cleaned.slides[0].bullets, vec!["kept".to_string()]);
        assert!(!cleaned.subtitle.is_empty());
    }

    #[test]
    fn wrap_text_never_empty_and_respects_width() {
        let lines = wrap_text("the quick brown fox jumps over the lazy dog", 30.0, 200.0);
        assert!(lines.len() > 1);
        assert!(lines.iter().all(|l| !l.is_empty()));
        let single = wrap_text("", 30.0, 200.0);
        assert_eq!(single.len(), 1);
    }

    // -- new-format invariants -------------------------------------------

    /// The same checks the Deck-only invariant tests enforce, applied to
    /// any design: populated pages, text colours from the theme palette,
    /// font sizes from the stored type scale, theme fonts, in-bounds, and
    /// no text-vs-text overlap.
    fn assert_design_invariants(design: &GeneratedDesign) {
        let th = &design.theme;
        let allowed_colors: Vec<String> =
            th.text_colors().iter().copied().map(String::from).collect();
        let allowed_sizes = design.type_scale.all();
        assert!(!design.pages.is_empty(), "design has no pages");
        for page in &design.pages {
            assert!(
                page.elements.iter().any(|e| e.kind == ElementKind::Text),
                "page '{}' has no text",
                page.title
            );
            for el in &page.elements {
                assert!(el.x >= -0.5 && el.y >= -0.5, "element off top-left: {el:?}");
                assert!(
                    el.x + el.width <= page.width + 1.0,
                    "element past right edge: {el:?}"
                );
                assert!(el.y <= page.height + 1.0, "element below page: {el:?}");
                if el.kind != ElementKind::Text {
                    continue;
                }
                assert!(
                    allowed_colors.contains(&el.fill),
                    "text fill {} not in palette {:?}",
                    el.fill,
                    allowed_colors
                );
                assert!(
                    allowed_sizes
                        .iter()
                        .any(|s| (*s - el.font_size).abs() < 0.01),
                    "font size {} not in scale {:?}",
                    el.font_size,
                    allowed_sizes
                );
                assert!(el.font_family == th.heading_font || el.font_family == th.body_font);
            }
            let texts: Vec<&DesignElement> = page
                .elements
                .iter()
                .filter(|e| e.kind == ElementKind::Text)
                .collect();
            for i in 0..texts.len() {
                for j in (i + 1)..texts.len() {
                    assert!(
                        !overlaps(texts[i], texts[j]),
                        "text overlap on page '{}':\n  {:?}\n  {:?}",
                        page.title,
                        texts[i],
                        texts[j],
                    );
                }
            }
        }
    }

    #[test]
    fn new_formats_satisfy_invariants_across_themes() {
        let formats = [
            DesignFormat::SocialPost,
            DesignFormat::WebPage,
            DesignFormat::Document,
        ];
        let briefs = [
            "Pitch deck for an indie coffee roaster with great espresso",
            "Overview of our 2026 product launch and go to market plan",
            "Annual sustainability report for a renewable energy startup",
        ];
        for format in formats {
            for id in ThemeId::all() {
                for brief in briefs {
                    let opts = ThemedDesignOptions {
                        format,
                        theme_id: id,
                        ..Default::default()
                    };
                    let outline = outline_from_brief(brief, opts).unwrap();
                    let design = generate_design(&outline, opts);
                    assert_eq!(design.format, format);
                    assert_design_invariants(&design);
                }
            }
        }
    }

    #[test]
    fn social_post_is_square_plus_story() {
        let opts = ThemedDesignOptions {
            format: DesignFormat::SocialPost,
            ..Default::default()
        };
        let outline = outline_from_brief("Promote our new oat milk latte", opts).unwrap();
        let design = generate_design(&outline, opts);
        assert_eq!(design.pages.len(), 2);
        // Square feed tile.
        assert!((design.pages[0].width - design.pages[0].height).abs() < 1.0);
        // Vertical 9:16 story.
        assert!(design.pages[1].height > design.pages[1].width);
        // Both carry a real hero image slot (prompt present).
        assert!(design.pages.iter().all(|p| p
            .elements
            .iter()
            .any(|e| e.kind == ElementKind::Image && e.image_prompt.is_some())));
    }

    #[test]
    fn web_page_is_single_tall_page_with_cards() {
        let opts = ThemedDesignOptions {
            format: DesignFormat::WebPage,
            section_count: Some(3),
            ..Default::default()
        };
        let outline = outline_from_brief("Landing page for a habit tracking app", opts).unwrap();
        let design = generate_design(&outline, opts);
        assert_eq!(design.pages.len(), 1);
        let page = &design.pages[0];
        assert!((page.width - WEB_W).abs() < 1.0);
        assert!(page.height > page.width, "web page should scroll tall");
        // A hero image (prompt) plus decorative card panels (no prompt).
        assert!(page
            .elements
            .iter()
            .any(|e| e.kind == ElementKind::Image && e.image_prompt.is_some()));
        assert!(page
            .elements
            .iter()
            .any(|e| e.kind == ElementKind::Image && e.image_prompt.is_none()));
    }

    #[test]
    fn document_has_cover_and_paginated_body() {
        let opts = ThemedDesignOptions {
            format: DesignFormat::Document,
            section_count: Some(8),
            ..Default::default()
        };
        let outline =
            outline_from_brief("Quarterly performance report for a SaaS company", opts).unwrap();
        let design = generate_design(&outline, opts);
        assert!(design.pages.len() >= 2, "cover + at least one body page");
        for page in &design.pages {
            assert!((page.width - DOC_W).abs() < 1.0 && (page.height - DOC_H).abs() < 1.0);
        }
        assert!(design.pages[0]
            .elements
            .iter()
            .any(|e| e.kind == ElementKind::Image));
        assert!(design.pages[0]
            .elements
            .iter()
            .any(|e| e.role == ElementRole::Title));
        assert!(design.pages[1]
            .elements
            .iter()
            .any(|e| e.role == ElementRole::Heading));
    }

    #[test]
    fn generation_is_deterministic_offline() {
        for format in DesignFormat::all() {
            let opts = ThemedDesignOptions {
                format,
                ..Default::default()
            };
            let outline =
                outline_from_brief("Launch plan for a modern productivity app", opts).unwrap();
            let a = generate_design(&outline, opts);
            let b = generate_design(&outline, opts);
            assert_eq!(
                format!("{a:?}"),
                format!("{b:?}"),
                "format {format:?} is not deterministic"
            );
        }
    }

    #[test]
    fn image_formats_scale_type_within_band() {
        for format in [
            DesignFormat::SocialPost,
            DesignFormat::WebPage,
            DesignFormat::Document,
        ] {
            let base = TypeScale::for_format(format);
            for id in ThemeId::all() {
                let opts = ThemedDesignOptions {
                    format,
                    theme_id: id,
                    ..Default::default()
                };
                let outline = outline_from_brief("Modern analytics platform launch", opts).unwrap();
                let design = generate_design(&outline, opts);
                let f = design.type_scale.title / base.title;
                assert!(
                    (0.9..=1.1).contains(&f),
                    "scale factor {f} out of band for {format:?}/{id:?}"
                );
                // Every size is scaled by the same factor.
                assert!((design.type_scale.body / base.body - f).abs() < 0.001);
            }
        }
    }

    #[test]
    fn variety_produces_distinct_layouts() {
        let format = DesignFormat::WebPage;
        let briefs = [
            "Indie coffee roaster brand site",
            "Productivity app launch page",
            "Renewable energy annual report",
        ];
        let mut factors = std::collections::HashSet::new();
        for id in ThemeId::all() {
            for brief in briefs {
                let opts = ThemedDesignOptions {
                    format,
                    theme_id: id,
                    ..Default::default()
                };
                let outline = outline_from_brief(brief, opts).unwrap();
                let design = generate_design(&outline, opts);
                factors.insert(format!("{:.3}", design.type_scale.title));
            }
        }
        assert!(factors.len() >= 2, "type-scale variety absent: {factors:?}");
    }

    #[test]
    fn deck_and_one_pager_geometry_is_unchanged_by_variety() {
        // Decks/one-pagers must keep their exact tuned type scale (no
        // scale-factor variety), so existing output never regresses.
        for format in [DesignFormat::Deck, DesignFormat::OnePager] {
            let base = TypeScale::for_format(format);
            let opts = ThemedDesignOptions {
                format,
                ..Default::default()
            };
            let outline = outline_from_brief("Pitch deck for a coffee roaster", opts).unwrap();
            let design = generate_design(&outline, opts);
            assert!((design.type_scale.title - base.title).abs() < 0.001);
            assert!((design.type_scale.body - base.body).abs() < 0.001);
        }
    }
}
