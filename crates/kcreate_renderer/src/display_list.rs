//! Cacheable display list: an intermediate, viewport-independent
//! description of what to draw. The same list is replayed on pan/zoom
//! without re-walking the scene graph.

use crate::geometry::{PathCommand, Point2, Rect, Style};
use crate::scene::{Object, ObjectId, ObjectKind};

/// A single drawing command. Pure data, trivially clonable.
#[derive(Debug, Clone)]
pub enum DisplayCommand {
    Clear, // The renderer is responsible for the clear color.
    FillRect {
        rect: Rect,
        style: Style,
    },
    FillCircle {
        center: Point2,
        radius: f32,
        style: Style,
    },
    FillLine {
        start: Point2,
        end: Point2,
        style: Style,
    },
    FillPath {
        commands: Vec<PathCommand>,
        style: Style,
    },
}

/// Ordered list of commands derived from a `Scene` snapshot.
///
/// The display list is viewport-independent: it contains every visible
/// scene object with per-command world bounds attached. The rasterizer
/// scissors against the visible viewport at draw time so a pan does not
/// invalidate the cache.
#[derive(Debug, Clone, Default)]
pub struct DisplayList {
    pub commands: Vec<DisplayCommand>,
    /// World-space bounds of every command in this list. Used for early
    /// scissor culling against the visible viewport.
    pub world_bounds: Option<Rect>,
    /// Stable id of the object each command came from (parallel array,
    /// `None` for non-object-derived commands like [`DisplayCommand::Clear`]).
    pub origins: Vec<Option<ObjectId>>,
    /// Per-command world bounds (parallel array). `None` for commands
    /// without spatial extent (e.g. [`DisplayCommand::Clear`]). The
    /// rasterizer skips a command when its bounds are entirely outside
    /// the visible viewport rect.
    pub cmd_bounds: Vec<Option<Rect>>,
}

impl DisplayList {
    pub fn new() -> Self {
        Self::default()
    }

    /// Push a command originating from a specific scene object.
    pub fn push_from_object(&mut self, cmd: DisplayCommand, object: &Object) {
        let bounds = object.world_bounds();
        self.world_bounds = Some(self.world_bounds.map_or(bounds, |b| b.union(&bounds)));
        self.commands.push(cmd);
        self.origins.push(Some(object.id));
        self.cmd_bounds.push(Some(bounds));
    }

    /// Push a non-object-derived command (e.g. background clear).
    /// `bounds` is `None` for commands without spatial extent.
    pub fn push_raw(&mut self, cmd: DisplayCommand, bounds: Option<Rect>) {
        if let Some(b) = bounds {
            self.world_bounds = Some(self.world_bounds.map_or(b, |w| w.union(&b)));
        }
        self.commands.push(cmd);
        self.origins.push(None);
        self.cmd_bounds.push(bounds);
    }

    pub const fn len(&self) -> usize {
        self.commands.len()
    }

    pub const fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    /// Convert an [`Object`] into the appropriate [`DisplayCommand`].
    pub(crate) fn command_from_object(object: &Object) -> DisplayCommand {
        let (dx, dy) = object.translation;
        match &object.kind {
            ObjectKind::Rect(r) => DisplayCommand::FillRect {
                rect: Rect::new(r.x + dx, r.y + dy, r.width, r.height),
                style: object.style,
            },
            ObjectKind::Circle { center, radius } => DisplayCommand::FillCircle {
                center: Point2::new(center.x + dx, center.y + dy),
                radius: *radius,
                style: object.style,
            },
            ObjectKind::Line { start, end } => DisplayCommand::FillLine {
                start: Point2::new(start.x + dx, start.y + dy),
                end: Point2::new(end.x + dx, end.y + dy),
                style: object.style,
            },
            ObjectKind::Path(cmds) => DisplayCommand::FillPath {
                commands: cmds.iter().map(|c| translate_path(c, dx, dy)).collect(),
                style: object.style,
            },
        }
    }
}

fn translate_path(c: &PathCommand, dx: f32, dy: f32) -> PathCommand {
    let t = |p: &Point2| Point2::new(p.x + dx, p.y + dy);
    match c {
        PathCommand::MoveTo(p) => PathCommand::MoveTo(t(p)),
        PathCommand::LineTo(p) => PathCommand::LineTo(t(p)),
        PathCommand::QuadTo { ctrl, end } => PathCommand::QuadTo {
            ctrl: t(ctrl),
            end: t(end),
        },
        PathCommand::CubicTo { c1, c2, end } => PathCommand::CubicTo {
            c1: t(c1),
            c2: t(c2),
            end: t(end),
        },
        PathCommand::Close => PathCommand::Close,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{Color, Style};

    #[test]
    fn command_from_rect_applies_translation() {
        let obj = Object::new(
            ObjectKind::Rect(Rect::new(10.0, 10.0, 5.0, 5.0)),
            Style::filled(Color::rgba(1.0, 0.0, 0.0, 1.0)),
        )
        .with_translation(100.0, 50.0);
        let cmd = DisplayList::command_from_object(&obj);
        match cmd {
            DisplayCommand::FillRect { rect, .. } => {
                assert_eq!(rect, Rect::new(110.0, 60.0, 5.0, 5.0));
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn world_bounds_unions_across_objects() {
        let mut list = DisplayList::new();
        let a = Object::new(
            ObjectKind::Rect(Rect::new(0.0, 0.0, 10.0, 10.0)),
            Style::filled(Color::rgba(1.0, 0.0, 0.0, 1.0)),
        );
        let b = Object::new(
            ObjectKind::Rect(Rect::new(100.0, 100.0, 5.0, 5.0)),
            Style::filled(Color::rgba(0.0, 1.0, 0.0, 1.0)),
        );
        list.push_from_object(DisplayList::command_from_object(&a), &a);
        list.push_from_object(DisplayList::command_from_object(&b), &b);
        let bounds = list.world_bounds.expect("bounds");
        assert_eq!(bounds, Rect::new(0.0, 0.0, 105.0, 105.0));
    }
}
