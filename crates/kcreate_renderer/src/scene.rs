//! Scene graph types consumed by the renderer.
//!
//! A [`Scene`] is the immutable, hashable snapshot the host hands us each
//! frame. Each [`Object`] has a stable [`ObjectId`] which the renderer uses
//! as the cache key in [`crate::display_list::DisplayList`].

use serde::{Deserialize, Serialize};

use crate::geometry::{Color, PathCommand, Point2, Rect, Style};

/// Stable identifier for a scene object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ObjectId(pub u64);

/// What the object is, geometrically.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ObjectKind {
    Rect(Rect),
    Circle {
        center: Point2,
        radius: f32,
    },
    Line {
        start: Point2,
        end: Point2,
    },
    Path(Vec<PathCommand>),
    /// Bitmap image. In Phase 0 the pixel buffer is inlined on the
    /// scene; Phase 1 will swap to a GPU texture handle keyed by
    /// content-addressed blob hash. The `pixels` buffer is RGBA8,
    /// row-major, straight alpha — exactly the format the host's
    /// `ImageData`/`putImageData` path uses.
    Image {
        /// Local-space rect the image is painted into. Width / height
        /// here may differ from `pixels` width / height (scaling).
        rect: Rect,
        /// Pixel buffer width in pixels.
        pixels_width: u32,
        /// Pixel buffer height in pixels.
        pixels_height: u32,
        /// RGBA8 pixel data. Length must equal
        /// `pixels_width * pixels_height * 4`.
        pixels: Vec<u8>,
        /// **Phase 11 Block A Task 3 — content-addressed fingerprint.**
        ///
        /// The first 8 bytes of the source blob's BLAKE3 hash
        /// (`RasterImageMeta.hash`). When present, the scene
        /// fingerprint hashes this 8-byte token instead of walking
        /// the (potentially 100MB) pixel buffer. Identical content
        /// always produces the same token, so caches can short-circuit
        /// without re-checksumming. `None` for synthetic raster
        /// layers built outside the blob store (tests, placeholders);
        /// the renderer falls back to chunked pixel hashing in that
        /// case.
        #[serde(default)]
        content_hash: Option<u64>,
    },
    /// A short string painted at `origin`. The renderer uses
    /// [`kcreate_text::shape_text`] to convert this into glyph paths
    /// at draw time. Style fill is used as the text color; stroke
    /// outlines glyphs.
    Text {
        /// Local-space baseline origin for the first glyph.
        origin: Point2,
        /// Text content (one short paragraph; multiline support is
        /// Phase 1).
        text: String,
        /// Family name; the text crate resolves this against the
        /// system font database.
        font_family: String,
        /// Em size in pixels (multiplied by viewport zoom at draw
        /// time).
        font_size: f32,
    },
}

impl ObjectKind {
    /// True for the structurally-large variants (`Image`). Used by
    /// debug logging and by the wire serializer to avoid dumping
    /// megabytes of pixel data.
    #[must_use]
    pub const fn is_heavy(&self) -> bool {
        matches!(self, Self::Image { .. })
    }
}

/// A single drawable object in the scene.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Object {
    pub id: ObjectId,
    pub kind: ObjectKind,
    pub style: Style,
    /// World-space transform (translation only in Phase 0; affine in Phase 1).
    pub translation: (f32, f32),
    pub visible: bool,
    /// Z-order (lower draws first).
    pub z: i32,
}

impl Object {
    /// Create a new object with an auto-generated id.
    pub fn new(kind: ObjectKind, style: Style) -> Self {
        Self {
            id: ObjectId(next_id()),
            kind,
            style,
            translation: (0.0, 0.0),
            visible: true,
            z: 0,
        }
    }

    #[must_use]
    pub const fn with_id(mut self, id: ObjectId) -> Self {
        self.id = id;
        self
    }

    #[must_use]
    pub const fn with_translation(mut self, dx: f32, dy: f32) -> Self {
        self.translation = (dx, dy);
        self
    }

    #[must_use]
    pub const fn with_z(mut self, z: i32) -> Self {
        self.z = z;
        self
    }

    /// World-space axis-aligned bounding box (including stroke width).
    pub fn world_bounds(&self) -> Rect {
        let local = local_bounds(&self.kind);
        let stroke = self.style.stroke.map_or(0.0, |s| s.width * 0.5);
        let (dx, dy) = self.translation;
        Rect::new(
            local.x + dx - stroke,
            local.y + dy - stroke,
            2.0f32.mul_add(stroke, local.width),
            2.0f32.mul_add(stroke, local.height),
        )
    }
}

/// Immutable scene snapshot. Construction is cheap; the renderer
/// borrows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Scene {
    pub clear_color: Color,
    pub objects: Vec<Object>,
}

impl Scene {
    pub const fn new(clear_color: Color) -> Self {
        Self {
            clear_color,
            objects: Vec::new(),
        }
    }

    pub fn add_object(&mut self, obj: Object) -> ObjectId {
        let id = obj.id;
        self.objects.push(obj);
        self.objects.sort_by_key(|o| o.z);
        id
    }

    /// Push every object from `objs` into the scene, sorting by `z`
    /// only **once** at the end.
    ///
    /// Equivalent to calling [`add_object`] in a loop, but avoids
    /// the per-insert `sort_by_key` scan. Multi-peer presence
    /// overlays (cursors + selection halos) routinely add hundreds
    /// of objects per scene-sync; with `add_object` each insertion
    /// re-sorts the entire growing `objects` vec, making the loop
    /// O(N²·log N) on N inserts. Batching collapses that to a
    /// single O(N·log N) sort. Internally still uses a stable sort
    /// so objects with equal `z` preserve insertion order
    /// (`sort_by_key` is stable on contiguous slices).
    ///
    /// The same z-order invariant `add_object` upholds is
    /// preserved: after this call, `self.objects` is sorted by `z`
    /// ascending (the CPU + GPU backends walk it back-to-front
    /// and rely on that ordering).
    ///
    /// [`add_object`]: Self::add_object
    pub fn add_objects<I: IntoIterator<Item = Object>>(&mut self, objs: I) {
        let before = self.objects.len();
        self.objects.extend(objs);
        if self.objects.len() != before {
            self.objects.sort_by_key(|o| o.z);
        }
    }

    /// Combined bounds of every visible object (None if the scene is empty).
    pub fn world_bounds(&self) -> Option<Rect> {
        self.objects
            .iter()
            .filter(|o| o.visible)
            .map(Object::world_bounds)
            .reduce(|a, b| a.union(&b))
    }
}

fn local_bounds(kind: &ObjectKind) -> Rect {
    match kind {
        ObjectKind::Rect(r) => *r,
        ObjectKind::Circle { center, radius } => Rect::new(
            center.x - radius,
            center.y - radius,
            radius * 2.0,
            radius * 2.0,
        ),
        ObjectKind::Line { start, end } => {
            let x = start.x.min(end.x);
            let y = start.y.min(end.y);
            let max_x = start.x.max(end.x);
            let max_y = start.y.max(end.y);
            Rect::new(x, y, max_x - x, max_y - y)
        }
        ObjectKind::Path(cmds) => path_bounds(cmds),
        ObjectKind::Image { rect, .. } => *rect,
        ObjectKind::Text {
            origin,
            font_size,
            text,
            ..
        } => {
            // Rough text bounds without shaping. Used only for scissor
            // culling — a slight over-estimate is fine. The CPU
            // backend shapes the actual text at draw time.
            let line_h = *font_size * 1.25;
            let approx_w = (text.chars().count() as f32) * font_size * 0.6;
            Rect::new(origin.x, origin.y - line_h, approx_w, line_h)
        }
    }
}

fn path_bounds(cmds: &[PathCommand]) -> Rect {
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    let extend = |p: Point2, min_x: &mut f32, min_y: &mut f32, max_x: &mut f32, max_y: &mut f32| {
        *min_x = min_x.min(p.x);
        *min_y = min_y.min(p.y);
        *max_x = max_x.max(p.x);
        *max_y = max_y.max(p.y);
    };
    for c in cmds {
        match c {
            PathCommand::MoveTo(p) | PathCommand::LineTo(p) => {
                extend(*p, &mut min_x, &mut min_y, &mut max_x, &mut max_y);
            }
            PathCommand::QuadTo { ctrl, end } => {
                extend(*ctrl, &mut min_x, &mut min_y, &mut max_x, &mut max_y);
                extend(*end, &mut min_x, &mut min_y, &mut max_x, &mut max_y);
            }
            PathCommand::CubicTo { c1, c2, end } => {
                extend(*c1, &mut min_x, &mut min_y, &mut max_x, &mut max_y);
                extend(*c2, &mut min_x, &mut min_y, &mut max_x, &mut max_y);
                extend(*end, &mut min_x, &mut min_y, &mut max_x, &mut max_y);
            }
            PathCommand::Close => {}
        }
    }
    if min_x.is_infinite() {
        return Rect::new(0.0, 0.0, 0.0, 0.0);
    }
    Rect::new(min_x, min_y, max_x - min_x, max_y - min_y)
}

/// Monotonically increasing object id allocator. Thread-safe.
fn next_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;
    use crate::geometry::{Stroke, Style};

    #[test]
    fn world_bounds_includes_stroke() {
        let obj = Object::new(
            ObjectKind::Rect(Rect::new(10.0, 10.0, 20.0, 20.0)),
            Style::fill_and_stroke(
                Color::rgba(1.0, 0.0, 0.0, 1.0),
                Stroke::new(Color::rgba(0.0, 0.0, 0.0, 1.0), 4.0),
            ),
        );
        let bounds = obj.world_bounds();
        assert_eq!(bounds, Rect::new(8.0, 8.0, 24.0, 24.0));
    }

    #[test]
    fn scene_sorts_by_z_on_insert() {
        let mut scene = Scene::new(Color::TRANSPARENT);
        scene.add_object(
            Object::new(
                ObjectKind::Rect(Rect::new(0.0, 0.0, 1.0, 1.0)),
                Style::filled(Color::rgba(1.0, 0.0, 0.0, 1.0)),
            )
            .with_z(5),
        );
        scene.add_object(
            Object::new(
                ObjectKind::Rect(Rect::new(0.0, 0.0, 1.0, 1.0)),
                Style::filled(Color::rgba(0.0, 1.0, 0.0, 1.0)),
            )
            .with_z(1),
        );
        scene.add_object(
            Object::new(
                ObjectKind::Rect(Rect::new(0.0, 0.0, 1.0, 1.0)),
                Style::filled(Color::rgba(0.0, 0.0, 1.0, 1.0)),
            )
            .with_z(3),
        );
        assert_eq!(
            scene.objects.iter().map(|o| o.z).collect::<Vec<_>>(),
            vec![1, 3, 5]
        );
    }

    #[test]
    fn add_objects_sorts_once_and_preserves_z_invariant() {
        let mut scene = Scene::new(Color::TRANSPARENT);
        let mk = |z: i32| {
            Object::new(
                ObjectKind::Rect(Rect::new(0.0, 0.0, 1.0, 1.0)),
                Style::filled(Color::rgba(1.0, 0.0, 0.0, 1.0)),
            )
            .with_z(z)
        };
        // Out-of-order batch — extend then single sort must produce
        // the same z ordering `add_object` would have, no matter the
        // insertion order.
        scene.add_objects([mk(5), mk(1), mk(3), mk(2), mk(4)]);
        assert_eq!(
            scene.objects.iter().map(|o| o.z).collect::<Vec<_>>(),
            vec![1, 2, 3, 4, 5]
        );
        // Subsequent batch merges with the existing sorted prefix.
        scene.add_objects([mk(0), mk(6)]);
        assert_eq!(
            scene.objects.iter().map(|o| o.z).collect::<Vec<_>>(),
            vec![0, 1, 2, 3, 4, 5, 6]
        );
    }

    #[test]
    fn add_objects_empty_batch_is_noop() {
        let mut scene = Scene::new(Color::TRANSPARENT);
        scene.add_objects(std::iter::empty());
        assert!(scene.objects.is_empty());
    }

    #[test]
    fn path_bounds_handles_curves() {
        let cmds = vec![
            PathCommand::MoveTo(Point2::new(0.0, 0.0)),
            PathCommand::CubicTo {
                c1: Point2::new(10.0, 10.0),
                c2: Point2::new(20.0, -5.0),
                end: Point2::new(30.0, 0.0),
            },
            PathCommand::Close,
        ];
        let b = path_bounds(&cmds);
        assert_eq!(b.x, 0.0);
        assert!((b.max_x() - 30.0).abs() < 0.001);
        assert!(b.y <= -5.0);
    }
}
