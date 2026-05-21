//! `kcreate_text` — font discovery and text shaping.
//!
//! Two modules:
//!
//! - [`font_db`] wraps [`fontdb::Database`] and surfaces a small
//!   read-mostly API for "find a face by family name" plus an
//!   "add file / add directory" surface for embedded brand fonts.
//! - [`shaper`] runs [`rustybuzz`] over the chosen face and returns a
//!   list of shaped glyphs plus outline path commands.
//!
//! The crate is **network-free** by construction (fontdb has no
//! network features, rustybuzz is pure CPU). It is therefore safe to
//! depend on from the editing path; the local-first deny-list test in
//! `kcreate_tests` validates this on every build.

#![forbid(unsafe_op_in_unsafe_fn)]

pub mod font_db;
pub mod outline;
pub mod shaper;

pub use font_db::{FontInfo, FontManager, FontManagerError};
pub use outline::{outline_glyph, OutlineCommand, OutlineError};
pub use shaper::{shape_text, ShapedGlyph, ShapedText, ShaperError};

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
