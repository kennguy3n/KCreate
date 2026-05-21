//! Renderer-side text adapter.
//!
//! The CPU backend (and, in Phase 2, the GPU backend) converts a
//! [`ObjectKind::Text`](crate::scene::ObjectKind::Text) into glyph
//! outlines via [`kcreate_text`]. This module owns the glue: it
//! shapes the string, walks the shaped glyph cursor, and emits one
//! flat list of [`PathCommand`] entries positioned in the local
//! coordinate space of the text object.

use kcreate_text::{outline_glyph, shape_text, OutlineCommand};

use crate::geometry::{PathCommand, Point2};

/// Convert shaped text into renderer path commands.
///
/// Shapes `text` with `font_family` at `font_size` (px) and walks the
/// resulting glyph cursor, emitting one flat list of [`PathCommand`]
/// entries. Returns `None` when no font is available or shaping fails
/// — the caller should skip painting in that case.
///
/// The returned commands are in **local** coordinates with the
/// origin at the baseline of the first glyph. Callers translate by
/// the object's world origin before pushing into the display list.
#[must_use]
pub fn shape_to_path_commands(
    text: &str,
    font_family: &str,
    font_size: f32,
) -> Option<Vec<PathCommand>> {
    if text.is_empty() {
        return None;
    }
    let shaped = shape_text(text, font_family, font_size).ok()?;
    if shaped.glyphs.is_empty() {
        return None;
    }
    let mut commands = Vec::with_capacity(shaped.glyphs.len() * 8);
    let mut cursor_x = 0.0_f64;
    let mut cursor_y = 0.0_f64;
    for g in &shaped.glyphs {
        let Ok(outline) = outline_glyph(&shaped.face, g.glyph_id, font_size) else {
            cursor_x += g.x_advance;
            cursor_y += g.y_advance;
            continue;
        };
        #[allow(clippy::cast_possible_truncation)]
        let origin_x = (cursor_x + g.x_offset) as f32;
        #[allow(clippy::cast_possible_truncation)]
        let origin_y = (cursor_y + g.y_offset) as f32;
        push_outline(&outline, origin_x, origin_y, &mut commands);
        cursor_x += g.x_advance;
        cursor_y += g.y_advance;
    }
    if commands.is_empty() {
        None
    } else {
        Some(commands)
    }
}

fn push_outline(outline: &[OutlineCommand], dx: f32, dy: f32, out: &mut Vec<PathCommand>) {
    for c in outline {
        match *c {
            OutlineCommand::MoveTo { x, y } => {
                out.push(PathCommand::MoveTo(Point2::new(x + dx, y + dy)))
            }
            OutlineCommand::LineTo { x, y } => {
                out.push(PathCommand::LineTo(Point2::new(x + dx, y + dy)))
            }
            OutlineCommand::QuadTo { cx, cy, x, y } => out.push(PathCommand::QuadTo {
                ctrl: Point2::new(cx + dx, cy + dy),
                end: Point2::new(x + dx, y + dy),
            }),
            OutlineCommand::CubicTo {
                c1x,
                c1y,
                c2x,
                c2y,
                x,
                y,
            } => out.push(PathCommand::CubicTo {
                c1: Point2::new(c1x + dx, c1y + dy),
                c2: Point2::new(c2x + dx, c2y + dy),
                end: Point2::new(x + dx, y + dy),
            }),
            OutlineCommand::Close => out.push(PathCommand::Close),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_text_returns_none() {
        assert!(shape_to_path_commands("", "sans-serif", 16.0).is_none());
    }
}
