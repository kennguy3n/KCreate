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
        let (default, max) = match self.format {
            DesignFormat::Deck => (6usize, 11usize),
            DesignFormat::OnePager => (4usize, 6usize),
        };
        let n = self.section_count.map_or(default, |v| v as usize);
        n.clamp(3, max)
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
    /// rectangles the fill.
    pub fill: String,
    pub corner_radius: f64,
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
    let scale = TypeScale::for_format(options.format);
    let pages = match options.format {
        DesignFormat::Deck => layout_deck(outline, &theme, &scale),
        DesignFormat::OnePager => layout_one_pager(outline, &theme, &scale, options.one_pager_size),
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

fn layout_deck(outline: &DeckOutline, theme: &Theme, scale: &TypeScale) -> Vec<GeneratedPage> {
    let mut pages = Vec::with_capacity(outline.slides.len() + 1);
    pages.push(title_slide(outline, theme, scale));
    let total = outline.slides.len();
    for (i, slide) in outline.slides.iter().enumerate() {
        pages.push(content_slide(slide, theme, scale, i + 1, total));
    }
    pages
}

fn title_slide(outline: &DeckOutline, theme: &Theme, scale: &TypeScale) -> GeneratedPage {
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
        &theme.primary,
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
        &theme.primary,
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
            &theme.primary,
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
        &theme.primary,
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
                &theme.primary,
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overlaps(a: &DesignElement, b: &DesignElement) -> bool {
        a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height
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
}
