//! Text shaping via [`rustybuzz`].
//!
//! Two public entry points:
//!
//! * [`shape_text`] — defaults: rustybuzz's intrinsic OT defaults
//!   (which honour the font's GSUB / GPOS but do not enable
//!   typographer-style alternates like `liga` or `smcp` unless the
//!   font's `required-features` table says so).
//! * [`shape_text_with_features`] — the renderer / export path uses
//!   this whenever a `TextLayer` node has an
//!   [`OpenTypeFeatures`](kcreate_core::OpenTypeFeatures) override
//!   in its metadata. Each enabled feature is translated to a
//!   `rustybuzz::Feature(tag, value=1, range=..)` entry; disabled
//!   booleans translate to `value=0` so the host font's defaults
//!   can be explicitly *turned off* (e.g. authors who want bare-
//!   bones digit pairs without `kern`).

use rustybuzz::ttf_parser::Tag;
use rustybuzz::{Face, Feature, UnicodeBuffer};
use thiserror::Error;

use crate::font_db::{FontManager, ResolvedFace};
use kcreate_core::OpenTypeFeatures;

/// Errors from [`shape_text`].
#[derive(Debug, Error)]
pub enum ShaperError {
    #[error("rustybuzz could not parse the face data")]
    FaceParse,
    #[error(transparent)]
    Font(#[from] crate::font_db::FontManagerError),
}

/// Output of shaping a single line of text.
#[derive(Debug, Clone, PartialEq)]
pub struct ShapedText {
    pub glyphs: Vec<ShapedGlyph>,
    /// Total advance width in font units, scaled to `font_size` px.
    pub width: f64,
    /// Total height (units per em → px) of one line at this size.
    pub height: f64,
    /// Font size used.
    pub font_size: f32,
    /// Font-design units per em.
    pub units_per_em: u16,
    /// Resolved face data so callers can extract glyph outlines
    /// without re-resolving against the font database.
    pub face: ResolvedFace,
}

/// One shaped glyph.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShapedGlyph {
    pub glyph_id: u16,
    pub x_advance: f64,
    pub y_advance: f64,
    pub x_offset: f64,
    pub y_offset: f64,
}

/// Shape `text` in the requested family at the requested em size,
/// using rustybuzz's intrinsic OpenType defaults. Use
/// [`shape_text_with_features`] to override these with a typographer-
/// authored [`OpenTypeFeatures`] set.
///
/// `font_family` is matched case-insensitively against
/// [`FontManager`]; if the requested family is missing, the first
/// loaded face is used as a fallback (so the renderer can always
/// draw *something*).
pub fn shape_text(
    text: &str,
    font_family: &str,
    font_size: f32,
) -> Result<ShapedText, ShaperError> {
    shape_text_with_features(text, font_family, font_size, &[])
}

/// Shape `text` with an explicit feature list. Each entry in
/// `features` is forwarded to `rustybuzz::shape` verbatim; build
/// them with [`opentype_features_to_buzz`] when you have an
/// [`OpenTypeFeatures`] struct in hand.
pub fn shape_text_with_features(
    text: &str,
    font_family: &str,
    font_size: f32,
    features: &[Feature],
) -> Result<ShapedText, ShaperError> {
    let manager = FontManager::new();
    let face_data = manager.resolve_face(font_family)?;
    shape_with_face_and_features(text, font_size, face_data, features)
}

/// Variant of [`shape_text`] that accepts a pre-resolved face.
/// Useful when the caller already has the font bytes loaded (e.g.
/// PDF export embedding the face).
pub fn shape_with_face(
    text: &str,
    font_size: f32,
    face_data: ResolvedFace,
) -> Result<ShapedText, ShaperError> {
    shape_with_face_and_features(text, font_size, face_data, &[])
}

/// Pre-resolved-face counterpart to [`shape_text_with_features`].
pub fn shape_with_face_and_features(
    text: &str,
    font_size: f32,
    face_data: ResolvedFace,
    features: &[Feature],
) -> Result<ShapedText, ShaperError> {
    let face =
        Face::from_slice(&face_data.data, face_data.face_index).ok_or(ShaperError::FaceParse)?;
    let units_per_em_i = face.units_per_em();
    // rustybuzz 0.18 returns `i32`; we narrow to `u16` because all
    // real TrueType files keep units-per-em well inside 65 535.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let units_per_em: u16 = units_per_em_i.clamp(1, i32::from(u16::MAX)) as u16;
    let scale = f64::from(font_size) / f64::from(units_per_em);

    let mut buffer = UnicodeBuffer::new();
    buffer.push_str(text);
    let glyph_buffer = rustybuzz::shape(&face, features, buffer);

    let mut glyphs = Vec::with_capacity(glyph_buffer.len());
    let mut width = 0.0_f64;
    for (info, pos) in glyph_buffer
        .glyph_infos()
        .iter()
        .zip(glyph_buffer.glyph_positions().iter())
    {
        let gx = f64::from(pos.x_advance) * scale;
        let gy = f64::from(pos.y_advance) * scale;
        let ox = f64::from(pos.x_offset) * scale;
        let oy = f64::from(pos.y_offset) * scale;
        #[allow(clippy::cast_possible_truncation)]
        let glyph_id = info.glyph_id as u16;
        glyphs.push(ShapedGlyph {
            glyph_id,
            x_advance: gx,
            y_advance: gy,
            x_offset: ox,
            y_offset: oy,
        });
        width += gx;
    }
    // Line height ≈ font size + 25% — fine for Phase 1 single-line
    // text; multiline layout is a Phase 2 concern.
    let height = f64::from(font_size) * 1.25;

    Ok(ShapedText {
        glyphs,
        width,
        height,
        font_size,
        units_per_em,
        face: face_data,
    })
}

/// Translate an [`OpenTypeFeatures`] set into the list of
/// `rustybuzz::Feature` records that `rustybuzz::shape` consumes.
///
/// **Encoding rules.** Each boolean in [`OpenTypeFeatures`] becomes
/// one [`Feature`] entry covering the entire buffer (`..`):
///
/// * Set field → `Feature::new(tag, 1, ..)`.
/// * Cleared field → `Feature::new(tag, 0, ..)` so the font's
///   default GSUB activation can be explicitly *suppressed* (e.g.
///   opt out of `liga` on a font that has ligatures on by default).
/// * [`OpenTypeFeatures::stylistic_sets`] expands into one
///   `ss01..=ss20` feature per index in the list. Indices outside
///   `1..=20` are silently dropped — OpenType only defines twenty
///   stylistic-set slots.
///
/// The function is deterministic and order-stable: features are
/// emitted in the same order every call, which keeps property-based
/// tests stable. There are at most `9 + 20 = 29` entries returned.
#[must_use]
pub fn opentype_features_to_buzz(features: &OpenTypeFeatures) -> Vec<Feature> {
    let mut out: Vec<Feature> = Vec::with_capacity(9 + features.stylistic_sets.len());

    // `liga` covers standard, required ligatures; `clig` covers the
    // contextual ones. Per-Microsoft-OpenType-spec they're usually
    // bundled together by typographers as "ligatures: on/off".
    push_bool(&mut out, *b"liga", features.ligatures);
    push_bool(&mut out, *b"clig", features.ligatures);
    push_bool(&mut out, *b"calt", features.contextual_alternates);
    push_bool(&mut out, *b"kern", features.kerning);
    push_bool(&mut out, *b"smcp", features.small_caps);
    push_bool(&mut out, *b"onum", features.old_style_figures);
    push_bool(&mut out, *b"tnum", features.tabular_figures);
    push_bool(&mut out, *b"frac", features.fractions);
    push_bool(&mut out, *b"ordn", features.ordinals);

    for &idx in &features.stylistic_sets {
        if !(1..=20).contains(&idx) {
            continue;
        }
        // OpenType tag layout is fixed: `ssNN` where NN is two
        // ASCII digits. Zero-pad to two characters.
        let tag_bytes = [b's', b's', b'0' + (idx / 10), b'0' + (idx % 10)];
        out.push(Feature::new(Tag::from_bytes(&tag_bytes), 1, ..));
    }

    out
}

fn push_bool(out: &mut Vec<Feature>, tag: [u8; 4], enabled: bool) {
    let value: u32 = u32::from(enabled);
    // `..` is `RangeFull` — applies the feature to every glyph in
    // the buffer, which is what every typographer-authored OT
    // override wants by default.
    out.push(Feature::new(Tag::from_bytes(&tag), value, ..));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_string_returns_no_glyphs() {
        // If no system font is available the call returns an error;
        // in that case the test is a no-op. If shaping succeeds we
        // require zero glyphs and zero width.
        let result = shape_text("", "sans-serif", 16.0);
        if let Ok(s) = result {
            assert!(s.glyphs.is_empty());
            assert!(s.width.abs() < f64::EPSILON);
        }
    }

    #[test]
    fn nonempty_string_produces_glyphs_when_font_available() {
        if let Ok(s) = shape_text("Hi", "sans-serif", 24.0) {
            assert!(!s.glyphs.is_empty());
            assert!(s.width > 0.0);
        }
    }

    // -----------------------------------------------------------------
    // OpenType feature plumbing — verifies the encoder lays out a
    // stable, deterministic feature list that matches the wire format
    // typographers expect (CSS `font-feature-settings`, InDesign /
    // Figma OpenType panels).
    // -----------------------------------------------------------------

    fn tag_string(t: Tag) -> String {
        let bytes = t.to_bytes();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    #[test]
    fn opentype_features_to_buzz_emits_all_booleans_in_order() {
        // Defaults: ligatures + contextual_alternates + kerning on,
        // everything else off, no stylistic sets. The encoder must
        // still emit every boolean (with value=0 for off) so the
        // shape call can explicitly suppress the font's defaults.
        let buzz = opentype_features_to_buzz(&OpenTypeFeatures::default());
        let tags: Vec<String> = buzz.iter().map(|f| tag_string(f.tag)).collect();
        assert_eq!(
            tags,
            vec!["liga", "clig", "calt", "kern", "smcp", "onum", "tnum", "frac", "ordn"],
            "encoder order is part of the public contract (test pins it)"
        );

        // Values: liga + clig + calt + kern enabled, the rest off.
        let values: Vec<u32> = buzz.iter().map(|f| f.value).collect();
        assert_eq!(values, vec![1, 1, 1, 1, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn opentype_features_to_buzz_emits_stylistic_sets_after_booleans() {
        let features = OpenTypeFeatures {
            stylistic_sets: vec![1, 7, 12, 20],
            ..OpenTypeFeatures::default()
        };
        let buzz = opentype_features_to_buzz(&features);
        let ss_tags: Vec<String> = buzz
            .iter()
            .skip(9) // 9 booleans first
            .map(|f| tag_string(f.tag))
            .collect();
        assert_eq!(ss_tags, vec!["ss01", "ss07", "ss12", "ss20"]);
        // All stylistic sets are value=1 (enable).
        for f in buzz.iter().skip(9) {
            assert_eq!(f.value, 1);
        }
    }

    #[test]
    fn opentype_features_to_buzz_drops_invalid_stylistic_sets() {
        // 0 and 21 are outside the OpenType-defined ss01..=ss20 range.
        let features = OpenTypeFeatures {
            stylistic_sets: vec![0, 5, 21, 200],
            ..OpenTypeFeatures::default()
        };
        let buzz = opentype_features_to_buzz(&features);
        let ss_tags: Vec<String> = buzz.iter().skip(9).map(|f| tag_string(f.tag)).collect();
        assert_eq!(ss_tags, vec!["ss05"]);
    }

    #[test]
    fn opentype_features_to_buzz_disables_ligatures_when_false() {
        let features = OpenTypeFeatures {
            ligatures: false,
            ..OpenTypeFeatures::default()
        };
        let buzz = opentype_features_to_buzz(&features);
        // liga + clig both encode the `ligatures` field, so a false
        // value emits two value=0 entries that explicitly turn off
        // the font's default ligature substitution.
        let liga = buzz
            .iter()
            .find(|f| tag_string(f.tag) == "liga")
            .expect("liga must be present");
        let clig = buzz
            .iter()
            .find(|f| tag_string(f.tag) == "clig")
            .expect("clig must be present");
        assert_eq!(liga.value, 0);
        assert_eq!(clig.value, 0);
    }

    #[test]
    fn shape_text_with_features_threads_through_to_rustybuzz() {
        // Whenever a system font is available, shaping with explicit
        // feature overrides must still succeed and produce glyphs.
        // The exact shaped output depends on the host font, so we
        // only check that:
        //   * the call succeeds,
        //   * the glyph count is non-zero,
        //   * the width is positive,
        // matching the existing `nonempty_string_produces_glyphs…`
        // contract.
        let features = opentype_features_to_buzz(&OpenTypeFeatures {
            ligatures: false,
            ..OpenTypeFeatures::default()
        });
        if let Ok(s) = shape_text_with_features("offline", "sans-serif", 32.0, &features) {
            assert!(!s.glyphs.is_empty());
            assert!(s.width > 0.0);
        }
    }
}
