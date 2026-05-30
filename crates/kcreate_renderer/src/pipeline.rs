//! Render pipeline: turns a `Scene` into a cached `DisplayList`.
//!
//! Phase 0 pipeline:
//!   1. Hash the scene (every field that affects pixels: geometry data,
//!      color values, stroke widths, path commands, translations, z, etc.).
//!   2. On cache hit, return the previous viewport-independent display list.
//!   3. On cache miss, walk the scene graph and emit one [`DisplayCommand`]
//!      per visible object in z-order. The display list contains *every*
//!      visible object — viewport culling is deferred to render time so a
//!      pan does not invalidate the cache.
//!
//! The cache key intentionally covers *all* scene state that the rasterizer
//! observes. Earlier iterations only hashed structural discriminants, which
//! produced false cache hits when callers mutated a rect's size, a fill
//! color, or a path's commands without changing object counts/ids.
//!
//! Per-command bounds are stored on the display list ([`DisplayList::cmd_bounds`])
//! so the rasterizer can scissor against the visible viewport rect without
//! re-walking the scene graph.

use std::hash::{Hash, Hasher};

use crate::display_list::{DisplayCommand, DisplayList};
use crate::geometry::{Color, PathCommand, Rect, Stroke, Style};
use crate::scene::{Object, ObjectKind, Scene};
use crate::viewport::Viewport;

/// Hash of every byte of scene state that affects rendered pixels.
///
/// Includes (per object): id, z, visibility, translation, geometry data,
/// fill colour, stroke colour and width, path commands. Plus the clear
/// colour. f32 values are hashed via [`f32::to_bits`] so NaN-distinct
/// scenes produce distinct hashes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SceneFingerprint(u64);

impl SceneFingerprint {
    fn of(scene: &Scene) -> Self {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        hash_color(scene.clear_color, &mut h);
        scene.objects.len().hash(&mut h);
        for obj in &scene.objects {
            hash_object(obj, &mut h);
        }
        Self(h.finish())
    }
}

fn hash_color(c: Color, h: &mut impl Hasher) {
    c.r.to_bits().hash(h);
    c.g.to_bits().hash(h);
    c.b.to_bits().hash(h);
    c.a.to_bits().hash(h);
}

fn hash_stroke(s: Stroke, h: &mut impl Hasher) {
    hash_color(s.color, h);
    s.width.to_bits().hash(h);
}

fn hash_style(s: Style, h: &mut impl Hasher) {
    match s.fill {
        Some(c) => {
            1u8.hash(h);
            hash_color(c, h);
        }
        None => 0u8.hash(h),
    }
    match s.stroke {
        Some(stroke) => {
            1u8.hash(h);
            hash_stroke(stroke, h);
        }
        None => 0u8.hash(h),
    }
}

fn hash_path(cmds: &[PathCommand], h: &mut impl Hasher) {
    cmds.len().hash(h);
    for c in cmds {
        match c {
            PathCommand::MoveTo(p) => {
                0u8.hash(h);
                p.x.to_bits().hash(h);
                p.y.to_bits().hash(h);
            }
            PathCommand::LineTo(p) => {
                1u8.hash(h);
                p.x.to_bits().hash(h);
                p.y.to_bits().hash(h);
            }
            PathCommand::QuadTo { ctrl, end } => {
                2u8.hash(h);
                ctrl.x.to_bits().hash(h);
                ctrl.y.to_bits().hash(h);
                end.x.to_bits().hash(h);
                end.y.to_bits().hash(h);
            }
            PathCommand::CubicTo { c1, c2, end } => {
                3u8.hash(h);
                c1.x.to_bits().hash(h);
                c1.y.to_bits().hash(h);
                c2.x.to_bits().hash(h);
                c2.y.to_bits().hash(h);
                end.x.to_bits().hash(h);
                end.y.to_bits().hash(h);
            }
            PathCommand::Close => {
                4u8.hash(h);
            }
        }
    }
}

fn hash_object(o: &Object, h: &mut impl Hasher) {
    o.id.0.hash(h);
    o.z.hash(h);
    o.visible.hash(h);
    o.translation.0.to_bits().hash(h);
    o.translation.1.to_bits().hash(h);
    hash_style(o.style, h);
    match &o.kind {
        ObjectKind::Rect(r) => {
            0u8.hash(h);
            r.x.to_bits().hash(h);
            r.y.to_bits().hash(h);
            r.width.to_bits().hash(h);
            r.height.to_bits().hash(h);
        }
        ObjectKind::Circle { center, radius } => {
            1u8.hash(h);
            center.x.to_bits().hash(h);
            center.y.to_bits().hash(h);
            radius.to_bits().hash(h);
        }
        ObjectKind::Line { start, end } => {
            2u8.hash(h);
            start.x.to_bits().hash(h);
            start.y.to_bits().hash(h);
            end.x.to_bits().hash(h);
            end.y.to_bits().hash(h);
        }
        ObjectKind::Path(cmds) => {
            3u8.hash(h);
            hash_path(cmds, h);
        }
        ObjectKind::Image {
            rect,
            pixels_width,
            pixels_height,
            pixels,
            content_hash,
        } => {
            4u8.hash(h);
            rect.x.to_bits().hash(h);
            rect.y.to_bits().hash(h);
            rect.width.to_bits().hash(h);
            rect.height.to_bits().hash(h);
            pixels_width.hash(h);
            pixels_height.hash(h);
            // **Phase 11 Block A Task 3 — content-addressed fingerprint.**
            //
            // When the scene-sync layer attached a token derived from
            // the blob store's BLAKE3 hash, hash 8 bytes instead of
            // the (potentially 100MB) pixel buffer. For a 4000×3000
            // RGBA image this collapses ~48MB of byte-wise SipHash
            // into 8 bytes — the entire reason this field exists.
            //
            // Synthetic / in-memory rasters (no blob, no token) fall
            // back to chunked pixel hashing so the fingerprint is
            // still pixel-accurate.
            if let Some(token) = content_hash {
                token.hash(h);
            } else {
                pixels.len().hash(h);
                for chunk in pixels.chunks(4096) {
                    chunk.hash(h);
                }
            }
        }
        ObjectKind::Text {
            origin,
            text,
            font_family,
            font_size,
        } => {
            5u8.hash(h);
            origin.x.to_bits().hash(h);
            origin.y.to_bits().hash(h);
            text.hash(h);
            font_family.hash(h);
            font_size.to_bits().hash(h);
        }
    }
}

#[derive(Debug)]
pub struct Pipeline {
    cache: Option<(SceneFingerprint, DisplayList)>,
}

impl Default for Pipeline {
    fn default() -> Self {
        Self::new()
    }
}

impl Pipeline {
    pub const fn new() -> Self {
        Self { cache: None }
    }

    /// Build (or reuse) the display list for the given scene snapshot.
    ///
    /// The returned list is viewport-independent: it contains every
    /// visible scene object, with per-command bounds attached for
    /// downstream scissor culling at rasterization time.
    ///
    /// `_viewport` and `_pixel_size` are accepted to keep the call site
    /// stable across future Phase 1 changes (when viewport-dependent
    /// LOD selection enters the pipeline), but they do NOT affect the
    /// cached display list today.
    pub fn build_display_list(
        &mut self,
        scene: &Scene,
        _viewport: &Viewport,
        _pixel_size: (u32, u32),
    ) -> DisplayList {
        let fp = SceneFingerprint::of(scene);
        if let Some((cached_fp, cached)) = &self.cache {
            if *cached_fp == fp {
                return cached.clone();
            }
        }

        let mut list = DisplayList::new();
        list.push_raw(DisplayCommand::Clear, None);
        for obj in &scene.objects {
            if !obj.visible {
                continue;
            }
            let cmd = DisplayList::command_from_object(obj);
            list.push_from_object(cmd, obj);
        }
        // Phase 11 Block A Task 5 — coalesce runs of same-style
        // FillRects into BatchedRects to cut draw-call count on
        // artboard-heavy scenes. Pixel output is identical because
        // the rasterizer iterates the rect list with the same style.
        let list = batch_consecutive_rects(list);
        self.cache = Some((fp, list.clone()));
        list
    }
}

/// **Phase 11 Block A Task 5 — display-list rect batching.**
///
/// Walks `list` in order and folds maximal runs of
/// [`DisplayCommand::FillRect`] entries with the same [`Style`] into a
/// single [`DisplayCommand::BatchedRects`]. The world-bounds /
/// origins / `cmd_bounds` parallel arrays are kept in lockstep: the
/// batch's bounds is the union of the constituents' bounds, and the
/// batch inherits `origins[i] = None` so per-object lookup callers
/// (display-list cache invalidation, hit-testing) treat it as
/// derived.
///
/// Runs of length 1 stay as a plain [`DisplayCommand::FillRect`] so
/// the common single-rect case pays no overhead. Heterogeneous-style
/// neighbours pass through unchanged.
fn batch_consecutive_rects(list: DisplayList) -> DisplayList {
    let DisplayList {
        commands,
        world_bounds,
        origins,
        cmd_bounds,
    } = list;
    let mut out_commands: Vec<DisplayCommand> = Vec::with_capacity(commands.len());
    let mut out_origins: Vec<Option<crate::scene::ObjectId>> = Vec::with_capacity(origins.len());
    let mut out_bounds: Vec<Option<Rect>> = Vec::with_capacity(cmd_bounds.len());

    let mut i = 0;
    while i < commands.len() {
        match &commands[i] {
            DisplayCommand::FillRect { rect, style } => {
                let run_style = *style;
                let mut run_rects: Vec<Rect> = vec![*rect];
                let mut run_bounds = cmd_bounds[i];
                let mut j = i + 1;
                while j < commands.len() {
                    if let DisplayCommand::FillRect {
                        rect: r2,
                        style: s2,
                    } = &commands[j]
                    {
                        if *s2 == run_style {
                            run_rects.push(*r2);
                            run_bounds = match (run_bounds, cmd_bounds[j]) {
                                (Some(a), Some(b)) => Some(a.union(&b)),
                                (Some(a), None) => Some(a),
                                (None, Some(b)) => Some(b),
                                (None, None) => None,
                            };
                            j += 1;
                            continue;
                        }
                    }
                    break;
                }
                if run_rects.len() == 1 {
                    out_commands.push(commands[i].clone());
                    out_origins.push(origins[i]);
                    out_bounds.push(cmd_bounds[i]);
                    i += 1;
                } else {
                    out_commands.push(DisplayCommand::BatchedRects {
                        rects: run_rects,
                        style: run_style,
                    });
                    // Batched command isn't attributable to a single
                    // scene object — cache-invalidation already keys
                    // on the full SceneFingerprint, so a None origin
                    // is safe.
                    out_origins.push(None);
                    out_bounds.push(run_bounds);
                    i = j;
                }
            }
            _ => {
                out_commands.push(commands[i].clone());
                out_origins.push(origins[i]);
                out_bounds.push(cmd_bounds[i]);
                i += 1;
            }
        }
    }

    DisplayList {
        commands: out_commands,
        world_bounds,
        origins: out_origins,
        cmd_bounds: out_bounds,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{Color, Rect, Style, Vec2};
    use crate::scene::{Object, ObjectKind};

    fn red_rect_at(x: f32, y: f32) -> Object {
        Object::new(
            ObjectKind::Rect(Rect::new(x, y, 10.0, 10.0)),
            Style::filled(Color::rgba(1.0, 0.0, 0.0, 1.0)),
        )
    }

    #[test]
    fn cache_hit_when_scene_unchanged() {
        let mut p = Pipeline::new();
        let mut s = Scene::new(Color::rgba(0.0, 0.0, 0.0, 1.0));
        s.add_object(red_rect_at(0.0, 0.0));
        let vp = Viewport::new(Vec2::ZERO, 1.0);
        let a = p.build_display_list(&s, &vp, (100, 100));
        let b = p.build_display_list(&s, &vp, (100, 100));
        assert_eq!(a.len(), b.len());
        assert!(p.cache.is_some());
    }

    #[test]
    fn cache_invalidated_when_rect_resized() {
        // The pre-fix fingerprint only hashed kind discriminants, so a
        // rect width change went unnoticed and the stale display list
        // was returned. Guard against the regression here.
        let mut p = Pipeline::new();
        let mut s = Scene::new(Color::rgba(0.0, 0.0, 0.0, 1.0));
        let obj = red_rect_at(0.0, 0.0);
        let id = s.add_object(obj);
        let vp = Viewport::new(Vec2::ZERO, 1.0);
        let _ = p.build_display_list(&s, &vp, (100, 100));

        // Mutate the rect's width.
        let new_obj = Object::new(
            ObjectKind::Rect(Rect::new(0.0, 0.0, 250.0, 10.0)),
            Style::filled(Color::rgba(1.0, 0.0, 0.0, 1.0)),
        )
        .with_id(id);
        s.objects.clear();
        s.add_object(new_obj);

        let list = p.build_display_list(&s, &vp, (100, 100));
        match list.commands.last().expect("cmd") {
            DisplayCommand::FillRect { rect, .. } => {
                #[allow(clippy::float_cmp)]
                {
                    assert_eq!(rect.width, 250.0);
                }
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn cache_invalidated_when_fill_color_changes() {
        let mut p = Pipeline::new();
        let mut s = Scene::new(Color::rgba(0.0, 0.0, 0.0, 1.0));
        let obj = Object::new(
            ObjectKind::Rect(Rect::new(0.0, 0.0, 10.0, 10.0)),
            Style::filled(Color::rgba(1.0, 0.0, 0.0, 1.0)),
        );
        let id = s.add_object(obj);
        let vp = Viewport::new(Vec2::ZERO, 1.0);
        let _ = p.build_display_list(&s, &vp, (100, 100));

        // Mutate the fill color from red to green.
        s.objects.clear();
        s.add_object(
            Object::new(
                ObjectKind::Rect(Rect::new(0.0, 0.0, 10.0, 10.0)),
                Style::filled(Color::rgba(0.0, 1.0, 0.0, 1.0)),
            )
            .with_id(id),
        );

        let list = p.build_display_list(&s, &vp, (100, 100));
        let style = match list.commands.last().expect("cmd") {
            DisplayCommand::FillRect { style, .. } => *style,
            other => panic!("unexpected: {other:?}"),
        };
        let fill = style.fill.expect("fill");
        assert!(fill.g > 0.5 && fill.r < 0.5);
    }

    #[test]
    fn cache_invalidated_when_path_commands_change() {
        let mut p = Pipeline::new();
        let mut s = Scene::new(Color::rgba(0.0, 0.0, 0.0, 1.0));
        let initial = Object::new(
            ObjectKind::Path(vec![
                PathCommand::MoveTo(crate::geometry::Point2::new(0.0, 0.0)),
                PathCommand::LineTo(crate::geometry::Point2::new(10.0, 10.0)),
            ]),
            Style::filled(Color::rgba(1.0, 0.0, 0.0, 1.0)),
        );
        let id = s.add_object(initial);
        let vp = Viewport::new(Vec2::ZERO, 1.0);
        let _ = p.build_display_list(&s, &vp, (100, 100));

        // Change the endpoint of the line.
        s.objects.clear();
        s.add_object(
            Object::new(
                ObjectKind::Path(vec![
                    PathCommand::MoveTo(crate::geometry::Point2::new(0.0, 0.0)),
                    PathCommand::LineTo(crate::geometry::Point2::new(999.0, 999.0)),
                ]),
                Style::filled(Color::rgba(1.0, 0.0, 0.0, 1.0)),
            )
            .with_id(id),
        );

        let list = p.build_display_list(&s, &vp, (100, 100));
        match list.commands.last().expect("cmd") {
            DisplayCommand::FillPath { commands, .. } => match &commands[1] {
                PathCommand::LineTo(p) => assert_eq!((p.x, p.y), (999.0, 999.0)),
                other => panic!("unexpected: {other:?}"),
            },
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn viewport_pan_does_not_alter_cached_list() {
        // After the architectural fix, viewport changes do NOT alter or
        // invalidate the display list. Culling happens at render time.
        let mut p = Pipeline::new();
        let mut s = Scene::new(Color::rgba(0.0, 0.0, 0.0, 1.0));
        s.add_object(red_rect_at(1000.0, 1000.0)); // off-screen at zero pan
        let vp_a = Viewport::new(Vec2::ZERO, 1.0);
        let vp_b = Viewport::new(Vec2::new(900.0, 900.0), 1.0);
        let list_a = p.build_display_list(&s, &vp_a, (100, 100));
        let list_b = p.build_display_list(&s, &vp_b, (100, 100));
        // Same content — pan does NOT change the list.
        assert_eq!(list_a.len(), list_b.len());
        // The off-screen rect IS in the list (no premature culling).
        assert!(list_a
            .commands
            .iter()
            .any(|c| matches!(c, DisplayCommand::FillRect { .. })));
    }

    #[test]
    fn invisible_objects_excluded() {
        let mut p = Pipeline::new();
        let mut s = Scene::new(Color::rgba(0.0, 0.0, 0.0, 1.0));
        let mut obj = red_rect_at(0.0, 0.0);
        obj.visible = false;
        s.add_object(obj);
        let vp = Viewport::new(Vec2::ZERO, 1.0);
        let list = p.build_display_list(&s, &vp, (100, 100));
        // Only the Clear command — invisible rect dropped.
        assert_eq!(list.len(), 1);
    }
}
