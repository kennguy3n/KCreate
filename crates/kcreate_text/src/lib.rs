//! `kcreate_text` — font discovery and text shaping.
//!
//! Two modules:
//!
//! - [`font_db`] wraps [`fontdb::Database`] and surfaces a small
//!   read-mostly API for "find a face by family name" plus an
//!   "add file / add directory" surface for embedded brand fonts.
//! - [`shaper`] runs [`rustybuzz`] over the chosen face and returns a
//!   list of shaped glyphs plus outline path commands.
//! - [`hyphenation`] implements Liang's algorithm (the same one
//!   that drives TeX) and embeds a public-domain English pattern
//!   set so the editing path stays network-free on first launch.
//! - [`paragraph`] composes the two into a multi-column line-breaker
//!   with overflow detection — the thing the renderer actually
//!   consumes when it has to lay text out inside a `TextLayer` frame.
//!
//! The crate is **network-free** by construction (fontdb has no
//! network features, rustybuzz is pure CPU, the hyphenation patterns
//! are `include_str!`ed at compile time). It is therefore safe to
//! depend on from the editing path; the local-first deny-list test
//! in `kcreate_tests` validates this on every build.

#![forbid(unsafe_op_in_unsafe_fn)]

pub mod font_db;
pub mod hyphenation;
pub mod outline;
pub mod paragraph;
pub mod shaper;

pub use font_db::{FontInfo, FontManager, FontManagerError};
pub use hyphenation::{HyphenationPatterns, EN_US_PATTERNS};
pub use outline::{outline_glyph, OutlineCommand, OutlineError};
pub use paragraph::{
    layout_paragraph, LayoutError, LayoutLine, ParagraphLayout, PositionedGlyph, TextStyle,
};
pub use shaper::{
    opentype_features_to_buzz, shape_text, shape_text_with_features, shape_with_face,
    shape_with_face_and_features, ShapedGlyph, ShapedText, ShaperError,
};

/// Combined error surface for the crate (re-exported for crates that
/// don't care which sub-step failed, e.g. `kcreate_renderer`).
#[derive(Debug, thiserror::Error)]
pub enum TextError {
    #[error(transparent)]
    Manager(#[from] FontManagerError),
    #[error(transparent)]
    Shaper(#[from] ShaperError),
    #[error(transparent)]
    Outline(#[from] OutlineError),
}
