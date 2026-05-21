//! CPU backend: real software rasterizer based on [`tiny_skia`].
//!
//! Engaged when no GPU adapter is available, when the `cpu-only` feature is
//! enabled, or in headless CI. Produces the same byte layout as the GPU
//! backend so consumers don't need backend-specific code.
//!
//! Output buffer is non-premultiplied straight-alpha RGBA8, row-major,
//! `width * height * 4` bytes, exactly matching what an Electron
//! `<canvas>` expects via `ImageData`.

use tiny_skia::{FillRule, Paint, Path, PathBuilder, Pixmap, Stroke as SkStroke, Transform};

use crate::display_list::{DisplayCommand, DisplayList};
use crate::geometry::{Color, PathCommand, Point2, Style};
use crate::scene::Scene;
use crate::viewport::Viewport;
use crate::{RendererError, Result};

#[derive(Debug)]
pub struct CpuBackend {
    pixmap: Pixmap,
}

impl CpuBackend {
    pub fn new(width: u32, height: u32) -> Self {
        let pixmap = Pixmap::new(width.max(1), height.max(1)).expect("pixmap alloc");
        Self { pixmap }
    }

    pub fn resize(&mut self, width: u32, height: u32) -> Result<()> {
        let new = Pixmap::new(width.max(1), height.max(1)).ok_or_else(|| {
            RendererError::Wgpu(format!("pixmap alloc failed for {width}x{height}"))
        })?;
        self.pixmap = new;
        Ok(())
    }

    pub fn render(
        &mut self,
        scene: &Scene,
        viewport: &Viewport,
        display_list: &DisplayList,
        out: &mut Vec<u8>,
        size: (u32, u32),
    ) -> Result<()> {
        let (w, h) = size;
        if self.pixmap.width() != w || self.pixmap.height() != h {
            self.resize(w, h)?;
        }
        self.pixmap.fill(scene.clear_color.to_tiny_skia());

        let transform = Transform::from_scale(viewport.zoom, viewport.zoom).post_translate(
            -viewport.pan.x * viewport.zoom,
            -viewport.pan.y * viewport.zoom,
        );

        // Per-frame scissor culling: visible scene rect lives in world
        // coordinates; commands whose world bounds don't intersect it can
        // be skipped. The display list itself is viewport-independent.
        let visible = viewport.visible_scene_rect((w, h));

        for (cmd, bounds) in display_list
            .commands
            .iter()
            .zip(display_list.cmd_bounds.iter())
        {
            if let Some(b) = bounds {
                if !visible.intersects(b) {
                    continue;
                }
            }
            match cmd {
                DisplayCommand::Clear => {
                    self.pixmap.fill(scene.clear_color.to_tiny_skia());
                }
                DisplayCommand::FillRect { rect, style } => {
                    if let Some(pb) = path_for_rect(*rect) {
                        self.draw_path(&pb, *style, transform);
                    }
                }
                DisplayCommand::FillCircle {
                    center,
                    radius,
                    style,
                } => {
                    if let Some(pb) = path_for_circle(*center, *radius) {
                        self.draw_path(&pb, *style, transform);
                    }
                }
                DisplayCommand::FillLine { start, end, style } => {
                    // Lines are stroked, not filled — pretend any "fill"
                    // request maps to a hairline stroke.
                    let Some(pb) = path_for_line(*start, *end) else {
                        continue;
                    };
                    let stroke_style = if style.stroke.is_some() {
                        *style
                    } else if let Some(c) = style.fill {
                        Style::stroked(crate::geometry::Stroke::new(c, 1.0))
                    } else {
                        *style
                    };
                    self.draw_path(&pb, stroke_style, transform);
                }
                DisplayCommand::FillPath { commands, style } => {
                    if let Some(pb) = path_for_commands(commands) {
                        self.draw_path(&pb, *style, transform);
                    }
                }
            }
        }

        let need = (w as usize) * (h as usize) * 4;
        out.clear();
        out.reserve(need);
        out.extend_from_slice(self.pixmap.data());
        // tiny-skia outputs premultiplied alpha. Electron's
        // `ImageData`/`putImageData` expects straight alpha — unmultiply.
        unpremultiply_in_place(out);
        Ok(())
    }

    fn draw_path(&mut self, path: &Path, style: Style, transform: Transform) {
        if let Some(fill) = style.fill {
            let mut paint = Paint {
                anti_alias: true,
                ..Paint::default()
            };
            paint.set_color(fill.to_tiny_skia());
            self.pixmap
                .fill_path(path, &paint, FillRule::Winding, transform, None);
        }
        if let Some(stroke) = style.stroke {
            let mut paint = Paint {
                anti_alias: true,
                ..Paint::default()
            };
            paint.set_color(stroke.color.to_tiny_skia());
            let sk_stroke = SkStroke {
                width: stroke.width.max(f32::EPSILON),
                ..SkStroke::default()
            };
            self.pixmap
                .stroke_path(path, &paint, &sk_stroke, transform, None);
        }
    }
}

// All `path_for_*` helpers return `Option<Path>` because `tiny_skia`'s
// `PathBuilder::finish` returns `None` for degenerate geometry (zero-size
// rects, zero-radius circles, zero-length lines, paths containing
// NaN/Inf, etc.). Returning `None` lets the caller skip the command
// instead of panicking — which is the correct response for a renderer
// that must stay alive in the face of bad inputs from the host.
fn path_for_rect(r: crate::geometry::Rect) -> Option<Path> {
    if r.is_empty() || !r.width.is_finite() || !r.height.is_finite() {
        return None;
    }
    let mut pb = PathBuilder::new();
    pb.move_to(r.x, r.y);
    pb.line_to(r.max_x(), r.y);
    pb.line_to(r.max_x(), r.max_y());
    pb.line_to(r.x, r.max_y());
    pb.close();
    pb.finish()
}

fn path_for_circle(center: Point2, radius: f32) -> Option<Path> {
    if !(radius.is_finite() && radius > 0.0 && center.x.is_finite() && center.y.is_finite()) {
        return None;
    }
    let mut pb = PathBuilder::new();
    pb.push_circle(center.x, center.y, radius);
    pb.finish()
}

fn path_for_line(start: Point2, end: Point2) -> Option<Path> {
    if !(start.x.is_finite() && start.y.is_finite() && end.x.is_finite() && end.y.is_finite()) {
        return None;
    }
    if (start.x - end.x).abs() < f32::EPSILON && (start.y - end.y).abs() < f32::EPSILON {
        // Zero-length line — tiny-skia rejects, and there's nothing to draw.
        return None;
    }
    let mut pb = PathBuilder::new();
    pb.move_to(start.x, start.y);
    pb.line_to(end.x, end.y);
    pb.finish()
}

fn path_for_commands(commands: &[PathCommand]) -> Option<Path> {
    if commands.is_empty() {
        return None;
    }
    let mut pb = PathBuilder::new();
    for c in commands {
        match c {
            PathCommand::MoveTo(p) => pb.move_to(p.x, p.y),
            PathCommand::LineTo(p) => pb.line_to(p.x, p.y),
            PathCommand::QuadTo { ctrl, end } => pb.quad_to(ctrl.x, ctrl.y, end.x, end.y),
            PathCommand::CubicTo { c1, c2, end } => {
                pb.cubic_to(c1.x, c1.y, c2.x, c2.y, end.x, end.y);
            }
            PathCommand::Close => pb.close(),
        }
    }
    pb.finish()
}

/// tiny-skia stores premultiplied RGBA. The renderer's contract is
/// straight-alpha RGBA so the host can `putImageData` directly.
fn unpremultiply_in_place(buf: &mut [u8]) {
    for px in buf.chunks_exact_mut(4) {
        let a = px[3];
        if a == 0 || a == 255 {
            continue;
        }
        let af = f32::from(a) / 255.0;
        px[0] = ((f32::from(px[0]) / af).min(255.0)) as u8;
        px[1] = ((f32::from(px[1]) / af).min(255.0)) as u8;
        px[2] = ((f32::from(px[2]) / af).min(255.0)) as u8;
    }
}

// Silence the unused `Color` import warning when no color helpers are referenced
// from this module's body (we only use Color via Style/Stroke).
#[allow(dead_code)]
const fn _color_marker(_c: Color) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display_list::DisplayCommand;
    use crate::geometry::{Color, Rect, Stroke, Style, Vec2};

    #[test]
    fn fill_rect_writes_expected_pixels() {
        let mut backend = CpuBackend::new(32, 32);
        let scene = Scene::new(Color::rgba(0.0, 0.0, 0.0, 1.0));
        let mut list = DisplayList::new();
        let rect = Rect::new(8.0, 8.0, 8.0, 8.0);
        list.push_raw(DisplayCommand::Clear, None);
        list.push_raw(
            DisplayCommand::FillRect {
                rect,
                style: Style::filled(Color::rgba(0.0, 1.0, 0.0, 1.0)),
            },
            Some(rect),
        );
        let mut out = Vec::new();
        backend
            .render(
                &scene,
                &Viewport::new(Vec2::ZERO, 1.0),
                &list,
                &mut out,
                (32, 32),
            )
            .unwrap();
        let idx = (10 * 32 + 10) * 4;
        assert!(
            out[idx + 1] > 200,
            "expected green at center, got {}",
            out[idx + 1]
        );
    }

    #[test]
    fn stroke_drawn_with_width() {
        let mut backend = CpuBackend::new(32, 32);
        let scene = Scene::new(Color::rgba(0.0, 0.0, 0.0, 1.0));
        let mut list = DisplayList::new();
        let start = Point2::new(0.0, 16.0);
        let end = Point2::new(32.0, 16.0);
        list.push_raw(DisplayCommand::Clear, None);
        list.push_raw(
            DisplayCommand::FillLine {
                start,
                end,
                style: Style::stroked(Stroke::new(Color::rgba(1.0, 1.0, 1.0, 1.0), 2.0)),
            },
            Some(Rect::new(
                start.x.min(end.x),
                start.y.min(end.y) - 1.0,
                (end.x - start.x).abs(),
                ((end.y - start.y).abs()).max(2.0),
            )),
        );
        let mut out = Vec::new();
        backend
            .render(
                &scene,
                &Viewport::new(Vec2::ZERO, 1.0),
                &list,
                &mut out,
                (32, 32),
            )
            .unwrap();
        let idx = (16 * 32 + 16) * 4;
        assert!(out[idx] > 200, "line should brighten center pixel");
    }

    #[test]
    fn unpremultiply_leaves_opaque_and_fully_transparent_alone() {
        let mut buf = vec![100, 50, 25, 255, 0, 0, 0, 0];
        unpremultiply_in_place(&mut buf);
        assert_eq!(buf, vec![100, 50, 25, 255, 0, 0, 0, 0]);
    }

    #[test]
    fn unpremultiply_recovers_straight_alpha() {
        // premultiplied (50% gray at 50% alpha) -> straight (full gray at 50% alpha)
        let mut buf = vec![64, 64, 64, 128];
        unpremultiply_in_place(&mut buf);
        assert!(
            buf[0] > 120 && buf[0] < 132,
            "recovered gray channel: {}",
            buf[0]
        );
        assert_eq!(buf[3], 128);
    }
}
