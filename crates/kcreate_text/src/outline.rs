//! Extract path commands from a glyph outline.
//!
//! [`rustybuzz`] re-exports `ttf-parser`'s [`OutlineBuilder`] trait;
//! we implement it once and collect [`OutlineCommand`] entries.

use rustybuzz::ttf_parser::{GlyphId, OutlineBuilder};
use rustybuzz::Face;
use thiserror::Error;

use crate::font_db::ResolvedFace;

/// Outline-extraction errors.
#[derive(Debug, Error)]
pub enum OutlineError {
    #[error("rustybuzz could not parse the face data")]
    FaceParse,
    #[error("glyph {0} has no outline")]
    NoOutline(u16),
}

/// A single path-building command in font-design units (caller
/// scales). Y axis follows the SVG convention (positive down) — we
/// flip the y on emission because TrueType is positive up.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OutlineCommand {
    MoveTo {
        x: f32,
        y: f32,
    },
    LineTo {
        x: f32,
        y: f32,
    },
    QuadTo {
        cx: f32,
        cy: f32,
        x: f32,
        y: f32,
    },
    CubicTo {
        c1x: f32,
        c1y: f32,
        c2x: f32,
        c2y: f32,
        x: f32,
        y: f32,
    },
    Close,
}

/// Extract the outline of `glyph_id` from the font carried by
/// `face_data`, returning the commands scaled to `font_size`
/// pixels per em (positive-y-down, ready for SVG / canvas
/// coordinates).
pub fn outline_glyph(
    face_data: &ResolvedFace,
    glyph_id: u16,
    font_size: f32,
) -> Result<Vec<OutlineCommand>, OutlineError> {
    let face =
        Face::from_slice(&face_data.data, face_data.face_index).ok_or(OutlineError::FaceParse)?;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let units_per_em = face.units_per_em().clamp(1, i32::from(u16::MAX)) as u16;
    let scale = font_size / f32::from(units_per_em);

    let mut sink = OutlineSink::new(scale);
    face.outline_glyph(GlyphId(glyph_id), &mut sink)
        .ok_or(OutlineError::NoOutline(glyph_id))?;
    Ok(sink.commands)
}

struct OutlineSink {
    scale: f32,
    commands: Vec<OutlineCommand>,
}

impl OutlineSink {
    const fn new(scale: f32) -> Self {
        Self {
            scale,
            commands: Vec::new(),
        }
    }
}

impl OutlineBuilder for OutlineSink {
    fn move_to(&mut self, x: f32, y: f32) {
        self.commands.push(OutlineCommand::MoveTo {
            x: x * self.scale,
            y: -y * self.scale,
        });
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.commands.push(OutlineCommand::LineTo {
            x: x * self.scale,
            y: -y * self.scale,
        });
    }
    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        self.commands.push(OutlineCommand::QuadTo {
            cx: cx * self.scale,
            cy: -cy * self.scale,
            x: x * self.scale,
            y: -y * self.scale,
        });
    }
    fn curve_to(&mut self, c1x: f32, c1y: f32, c2x: f32, c2y: f32, x: f32, y: f32) {
        self.commands.push(OutlineCommand::CubicTo {
            c1x: c1x * self.scale,
            c1y: -c1y * self.scale,
            c2x: c2x * self.scale,
            c2y: -c2y * self.scale,
            x: x * self.scale,
            y: -y * self.scale,
        });
    }
    fn close(&mut self) {
        self.commands.push(OutlineCommand::Close);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font_db::FontManager;

    #[test]
    fn outline_returns_commands_for_loaded_glyph() {
        let manager = FontManager::new();
        let Ok(face) = manager.resolve_face("sans-serif") else {
            // No fonts at all on this host (rare in CI but possible
            // in minimal containers). Skip — we already cover the
            // happy path in [`shaper::tests`].
            return;
        };
        // Walk a broad glyph range and consider the test passing as
        // soon as we find *one* glyph with a non-empty outline. We
        // search a wider range than just .notdef + ASCII because some
        // fonts (notably icon fonts) place useful glyphs higher up.
        let any_outline = (0..1024u16)
            .filter_map(|g| outline_glyph(&face, g, 32.0).ok())
            .any(|cmds| !cmds.is_empty());
        assert!(any_outline, "no glyph in the first 1024 had an outline");
    }
}
