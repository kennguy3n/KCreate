//! Text shaping via [`rustybuzz`].

use rustybuzz::{Face, UnicodeBuffer};
use thiserror::Error;

use crate::font_db::{FontManager, ResolvedFace};

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

/// Shape `text` in the requested family at the requested em size.
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
    let manager = FontManager::new();
    let face_data = manager.resolve_face(font_family)?;
    shape_with_face(text, font_size, face_data)
}

/// Variant of [`shape_text`] that accepts a pre-resolved face.
/// Useful when the caller already has the font bytes loaded (e.g.
/// PDF export embedding the face).
pub fn shape_with_face(
    text: &str,
    font_size: f32,
    face_data: ResolvedFace,
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
    let glyph_buffer = rustybuzz::shape(&face, &[], buffer);

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
}
