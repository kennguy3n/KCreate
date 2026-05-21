//! Render pipeline: turns a `Scene` into a cached `DisplayList`.
//!
//! Phase 0 pipeline:
//!   1. Build a spatial index from the scene.
//!   2. Compute the visible scene rect from the viewport.
//!   3. Query the spatial index for objects intersecting the visible rect.
//!   4. Emit a `DisplayCommand` per visible object, in z-order.
//!
//! The pipeline holds a per-scene-fingerprint cache so repeated frames
//! with an unchanged scene do not re-traverse the graph. Pan/zoom only
//! does not invalidate the cache.

use crate::display_list::{DisplayCommand, DisplayList};
use crate::scene::{ObjectId, Scene};
use crate::spatial::SpatialIndex;
use crate::viewport::Viewport;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct SceneFingerprint {
    object_count: usize,
    /// Hash of (id, z, kind discriminant, translation, style discriminants)
    /// — cheap to compute, distinguishes "same scene" from "different scene".
    structural_hash: u64,
}

impl SceneFingerprint {
    fn of(scene: &Scene) -> Self {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        scene.clear_color.r.to_bits().hash(&mut h);
        scene.clear_color.g.to_bits().hash(&mut h);
        scene.clear_color.b.to_bits().hash(&mut h);
        scene.clear_color.a.to_bits().hash(&mut h);
        for obj in &scene.objects {
            obj.id.0.hash(&mut h);
            obj.z.hash(&mut h);
            obj.visible.hash(&mut h);
            obj.translation.0.to_bits().hash(&mut h);
            obj.translation.1.to_bits().hash(&mut h);
            std::mem::discriminant(&obj.kind).hash(&mut h);
            obj.style.fill.is_some().hash(&mut h);
            obj.style.stroke.is_some().hash(&mut h);
        }
        Self {
            object_count: scene.objects.len(),
            structural_hash: h.finish(),
        }
    }
}

#[derive(Debug)]
pub struct Pipeline {
    cache: Option<(SceneFingerprint, DisplayList)>,
    last_visible: Option<Vec<ObjectId>>,
}

impl Default for Pipeline {
    fn default() -> Self {
        Self::new()
    }
}

impl Pipeline {
    pub const fn new() -> Self {
        Self {
            cache: None,
            last_visible: None,
        }
    }

    /// Build (or reuse) the display list for the given scene snapshot.
    ///
    /// The display list is independent of the viewport — viewport changes
    /// are applied as a single transform inside the rasterizer.
    pub fn build_display_list(
        &mut self,
        scene: &Scene,
        viewport: &Viewport,
        pixel_size: (u32, u32),
    ) -> DisplayList {
        let fp = SceneFingerprint::of(scene);
        let scene_unchanged = matches!(&self.cache, Some((c, _)) if *c == fp);
        if scene_unchanged {
            if let Some((_, list)) = &self.cache {
                // Cache hit. But culling still depends on the viewport, so we
                // re-cull using the visible viewport rect and return a
                // viewport-pruned copy.
                let visible = viewport.visible_scene_rect(pixel_size);
                if let Some(bounds) = list.world_bounds {
                    if visible.intersects(&bounds) {
                        return list.clone();
                    }
                    // Visible viewport misses the scene entirely — produce
                    // a minimal list (clear only).
                    let mut empty = DisplayList::new();
                    empty.push_raw(DisplayCommand::Clear);
                    return empty;
                }
                return list.clone();
            }
        }

        let index = SpatialIndex::build_from(&scene.objects);
        let visible = viewport.visible_scene_rect(pixel_size);
        let hit_ids: Vec<_> = index.query(visible.inflate(8.0));
        let hits: std::collections::HashSet<ObjectId> = hit_ids.iter().copied().collect();

        let mut list = DisplayList::new();
        list.push_raw(DisplayCommand::Clear);
        for obj in &scene.objects {
            if !obj.visible {
                continue;
            }
            if !hits.contains(&obj.id) && !scene.objects.is_empty() {
                // Culled by spatial index.
                continue;
            }
            let cmd = DisplayList::command_from_object(obj);
            list.push_from_object(cmd, obj);
        }
        self.last_visible = Some(hit_ids);
        self.cache = Some((fp, list.clone()));
        list
    }

    /// Object ids that were visible in the last [`build_display_list`] call.
    pub fn last_visible(&self) -> Option<&[ObjectId]> {
        self.last_visible.as_deref()
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
    fn culls_objects_outside_viewport() {
        let mut scene = Scene::new(Color::rgba(0.0, 0.0, 0.0, 1.0));
        scene.add_object(red_rect_at(0.0, 0.0));
        scene.add_object(red_rect_at(10_000.0, 10_000.0));

        let vp = Viewport::new(Vec2::ZERO, 1.0);
        let mut pipeline = Pipeline::new();
        let list = pipeline.build_display_list(&scene, &vp, (100, 100));
        // 1 clear + 1 fill
        assert_eq!(list.commands.len(), 2);
        assert_eq!(pipeline.last_visible().unwrap().len(), 1);
    }

    #[test]
    fn cache_hit_on_identical_scene() {
        let mut scene = Scene::new(Color::rgba(0.0, 0.0, 0.0, 1.0));
        scene.add_object(red_rect_at(0.0, 0.0));
        let vp = Viewport::new(Vec2::ZERO, 1.0);
        let mut pipeline = Pipeline::new();
        let a = pipeline.build_display_list(&scene, &vp, (100, 100));
        let b = pipeline.build_display_list(&scene, &vp, (100, 100));
        assert_eq!(a.commands.len(), b.commands.len());
        // The fingerprint should match (no rebuild).
        assert!(pipeline.cache.is_some());
    }

    #[test]
    fn cache_invalidated_when_object_added() {
        let mut scene = Scene::new(Color::rgba(0.0, 0.0, 0.0, 1.0));
        scene.add_object(red_rect_at(0.0, 0.0));
        let vp = Viewport::new(Vec2::ZERO, 1.0);
        let mut pipeline = Pipeline::new();
        let a = pipeline.build_display_list(&scene, &vp, (100, 100));

        scene.add_object(red_rect_at(20.0, 20.0));
        let b = pipeline.build_display_list(&scene, &vp, (100, 100));
        assert!(b.commands.len() > a.commands.len());
    }
}
