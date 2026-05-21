//! Canvas hit testing: screen point → document node uuid.
//!
//! The canvas is a 2-D viewport with pan + uniform zoom. A click at
//! `(screen_x, screen_y)` corresponds to the world point
//! `((screen_x - pan_x) / zoom, (screen_y - pan_y) / zoom)`. We then
//! walk the scene in reverse z order (topmost first) and return the
//! first object whose AABB contains the world point. The
//! [`SceneSync`] map turns the renderer's `ObjectId` back into the
//! document's `Uuid` so the host can use it for selection /
//! property-panel binding.
//!
//! Selection-highlight objects (allocated by [`SceneSync`] with ids in
//! the top of the `u64` range) are deliberately excluded from
//! hit-testing — clicking on a selection outline should not "hit" the
//! outline itself; we want the underlying node.

use kcreate_renderer::{Scene, Vec2};
use uuid::Uuid;

use crate::scene_sync::{is_selection_highlight_id, SceneSync};

/// Camera transform shared with the renderer's `Viewport`. We accept
/// it as a plain struct here to keep `hit_test` independent of the
/// renderer's process-global viewport state — callers pass whatever
/// is on screen at hit time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewport {
    pub pan: Vec2,
    pub zoom: f32,
}

impl Viewport {
    #[must_use]
    pub const fn new(pan: Vec2, zoom: f32) -> Self {
        Self { pan, zoom }
    }

    /// Screen → world coordinate transform. Inverse of the renderer's
    /// view matrix.
    #[must_use]
    pub fn screen_to_world(&self, x: f32, y: f32) -> (f32, f32) {
        let z = if self.zoom.abs() < f32::EPSILON {
            1.0
        } else {
            self.zoom
        };
        ((x - self.pan.x) / z, (y - self.pan.y) / z)
    }
}

/// Locate the topmost selectable object at `(screen_x, screen_y)`.
///
/// Returns the document [`Uuid`] of the hit node, or `None` if no
/// scene object contains the world point.
#[must_use]
pub fn hit_test(
    scene_sync: &SceneSync,
    scene: &Scene,
    screen_x: f32,
    screen_y: f32,
    viewport: Viewport,
) -> Option<Uuid> {
    let (wx, wy) = viewport.screen_to_world(screen_x, screen_y);

    // Walk in reverse z order: scene_sync emits objects depth-first,
    // so the *last* visible object on top is the one we want.
    let mut sorted: Vec<&kcreate_renderer::Object> = scene.objects.iter().collect();
    sorted.sort_by_key(|cand| std::cmp::Reverse(cand.z));
    for obj in sorted {
        if !obj.visible {
            continue;
        }
        let bounds = obj.world_bounds();
        if !point_in_rect(bounds, wx, wy) {
            continue;
        }
        // Skip selection highlight overlays. Highlights are appended
        // to the scene with ids drawn from the reserved high range
        // (`HIGHLIGHT_ID_THRESHOLD..=u64::MAX`), separate from the
        // monotonic low-range allocator that backs real document
        // objects. The id-range check is exact — unlike the previous
        // "unfilled stroked rect" style heuristic, it never
        // false-positives on a user-created outline-only rect.
        if is_selection_highlight_id(obj.id) {
            continue;
        }
        if let Some(uuid) = scene_sync.uuid_for_object_id(obj.id) {
            return Some(uuid);
        }
    }
    None
}

fn point_in_rect(rect: kcreate_renderer::Rect, x: f32, y: f32) -> bool {
    x >= rect.x && x <= rect.x + rect.width && y >= rect.y && y <= rect.y + rect.height
}

#[cfg(test)]
mod tests {
    use super::*;
    use kcreate_core::document::DocumentGraph;
    use kcreate_core::node::{Bounds, FillStyle, Node, NodeType, RgbaColor};
    use kcreate_vector::{PathPoint, PathSegment, VectorPath};

    fn make_doc_with_rect_at(x: f64, y: f64, w: f64, h: f64) -> (DocumentGraph, Uuid) {
        let mut doc = DocumentGraph::new();
        let path = VectorPath::new(vec![
            PathSegment::MoveTo(PathPoint::new(x, y)),
            PathSegment::LineTo(PathPoint::new(x + w, y)),
            PathSegment::LineTo(PathPoint::new(x + w, y + h)),
            PathSegment::LineTo(PathPoint::new(x, y + h)),
            PathSegment::Close,
        ]);
        let mut node = Node::new(NodeType::VectorLayer, "r");
        node.bounds = Bounds {
            x,
            y,
            width: w,
            height: h,
        };
        node.style.fill = FillStyle::Solid(RgbaColor {
            r: 1.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        });
        node.metadata.insert(
            crate::scene_sync::VECTOR_PATH_METADATA_KEY.to_string(),
            serde_json::to_value(&path).unwrap(),
        );
        let id = doc.insert_node(node).unwrap();
        (doc, id)
    }

    #[test]
    fn click_inside_object_returns_uuid() {
        let (doc, id) = make_doc_with_rect_at(10.0, 10.0, 20.0, 20.0);
        let mut sync = SceneSync::new();
        let scene = sync.sync_document_to_scene(&doc, None, &[]);
        let vp = Viewport::new(Vec2::new(0.0, 0.0), 1.0);
        let hit = hit_test(&sync, &scene, 15.0, 15.0, vp);
        assert_eq!(hit, Some(id));
    }

    #[test]
    fn click_outside_returns_none() {
        let (doc, _id) = make_doc_with_rect_at(10.0, 10.0, 20.0, 20.0);
        let mut sync = SceneSync::new();
        let scene = sync.sync_document_to_scene(&doc, None, &[]);
        let vp = Viewport::new(Vec2::new(0.0, 0.0), 1.0);
        let hit = hit_test(&sync, &scene, 1.0, 1.0, vp);
        assert!(hit.is_none());
    }

    #[test]
    fn topmost_object_wins() {
        let mut doc = DocumentGraph::new();
        let mut sync = SceneSync::new();
        let path_a = VectorPath::new(vec![
            PathSegment::MoveTo(PathPoint::new(0.0, 0.0)),
            PathSegment::LineTo(PathPoint::new(50.0, 0.0)),
            PathSegment::LineTo(PathPoint::new(50.0, 50.0)),
            PathSegment::LineTo(PathPoint::new(0.0, 50.0)),
            PathSegment::Close,
        ]);
        let mut a = Node::new(NodeType::VectorLayer, "a");
        a.bounds = Bounds {
            x: 0.0,
            y: 0.0,
            width: 50.0,
            height: 50.0,
        };
        a.metadata.insert(
            crate::scene_sync::VECTOR_PATH_METADATA_KEY.to_string(),
            serde_json::to_value(&path_a).unwrap(),
        );
        let id_a = doc.insert_node(a).unwrap();

        let path_b = VectorPath::new(vec![
            PathSegment::MoveTo(PathPoint::new(10.0, 10.0)),
            PathSegment::LineTo(PathPoint::new(30.0, 10.0)),
            PathSegment::LineTo(PathPoint::new(30.0, 30.0)),
            PathSegment::LineTo(PathPoint::new(10.0, 30.0)),
            PathSegment::Close,
        ]);
        let mut b = Node::new(NodeType::VectorLayer, "b");
        b.bounds = Bounds {
            x: 10.0,
            y: 10.0,
            width: 20.0,
            height: 20.0,
        };
        b.metadata.insert(
            crate::scene_sync::VECTOR_PATH_METADATA_KEY.to_string(),
            serde_json::to_value(&path_b).unwrap(),
        );
        let id_b = doc.insert_node(b).unwrap();

        let scene = sync.sync_document_to_scene(&doc, None, &[]);
        let vp = Viewport::new(Vec2::new(0.0, 0.0), 1.0);
        // Click inside both: must hit B (drawn last → highest z).
        let hit = hit_test(&sync, &scene, 20.0, 20.0, vp);
        assert_eq!(hit, Some(id_b));
        // Click outside B but inside A: returns A.
        let hit_a = hit_test(&sync, &scene, 5.0, 5.0, vp);
        assert_eq!(hit_a, Some(id_a));
    }

    #[test]
    fn viewport_pan_and_zoom_are_inverted() {
        let (doc, id) = make_doc_with_rect_at(100.0, 100.0, 10.0, 10.0);
        let mut sync = SceneSync::new();
        let scene = sync.sync_document_to_scene(&doc, None, &[]);
        // Pan world (100,100) onto screen (50,50) at 0.5x zoom:
        // screen = world * 0.5 + pan ⇒ 50 = 100 * 0.5 + 0 ⇒ pan = 0
        let vp = Viewport::new(Vec2::new(0.0, 0.0), 0.5);
        let hit = hit_test(&sync, &scene, 50.0, 50.0, vp);
        assert_eq!(hit, Some(id), "world(100,100) at zoom 0.5 ↦ screen(50,50)");
        // With pan 25,25 and zoom 1: screen = world + pan ⇒ click (125,125).
        let vp = Viewport::new(Vec2::new(25.0, 25.0), 1.0);
        let hit = hit_test(&sync, &scene, 130.0, 130.0, vp);
        assert_eq!(hit, Some(id));
    }

    #[test]
    fn empty_scene_returns_none() {
        let doc = DocumentGraph::new();
        let mut sync = SceneSync::new();
        let scene = sync.sync_document_to_scene(&doc, None, &[]);
        let vp = Viewport::new(Vec2::new(0.0, 0.0), 1.0);
        assert!(hit_test(&sync, &scene, 0.0, 0.0, vp).is_none());
    }

    /// Regression: an outline-only (stroke-no-fill) user rect must
    /// still be hit-testable. The previous heuristic skipped *any*
    /// stroked-unfilled rect on the assumption it was a selection
    /// highlight; the id-range check now reliably distinguishes
    /// document objects from highlight overlays.
    #[test]
    fn outline_only_user_rect_is_still_hittable() {
        let mut doc = DocumentGraph::new();
        let path = VectorPath::new(vec![
            PathSegment::MoveTo(PathPoint::new(0.0, 0.0)),
            PathSegment::LineTo(PathPoint::new(20.0, 0.0)),
            PathSegment::LineTo(PathPoint::new(20.0, 20.0)),
            PathSegment::LineTo(PathPoint::new(0.0, 20.0)),
            PathSegment::Close,
        ]);
        let mut node = Node::new(NodeType::VectorLayer, "outline-only");
        node.bounds = Bounds {
            x: 0.0,
            y: 0.0,
            width: 20.0,
            height: 20.0,
        };
        // No fill, but a stroke — same shape as the selection
        // highlight's *style*. Hit-testing must still return the
        // node's uuid because its ObjectId is in the low (real)
        // range, not the highlight range.
        node.style.fill = FillStyle::None;
        node.style.stroke = Some(kcreate_core::node::StrokeStyle {
            color: RgbaColor {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            },
            width: 1.0,
            dash: Vec::new(),
        });
        node.metadata.insert(
            crate::scene_sync::VECTOR_PATH_METADATA_KEY.to_string(),
            serde_json::to_value(&path).unwrap(),
        );
        let id = doc.insert_node(node).unwrap();

        let mut sync = SceneSync::new();
        let scene = sync.sync_document_to_scene(&doc, None, &[]);
        let vp = Viewport::new(Vec2::new(0.0, 0.0), 1.0);
        let hit = hit_test(&sync, &scene, 5.0, 5.0, vp);
        assert_eq!(
            hit,
            Some(id),
            "outline-only user rect must still hit (id-range check, not style heuristic)"
        );
    }

    /// Regression: a selection highlight must NOT be hit-testable,
    /// even though it occupies the same world bounds as the
    /// underlying selected node. Clicking inside the bounds of a
    /// selected node should return the *node's* uuid, not nothing
    /// and not a highlight uuid (highlights aren't in the
    /// `ObjectId` ↔ `Uuid` map at all).
    #[test]
    fn click_on_selected_node_returns_node_not_highlight() {
        let (doc, id) = make_doc_with_rect_at(10.0, 10.0, 20.0, 20.0);
        let mut sync = SceneSync::new();
        // Mark the node as selected so a highlight is appended.
        let scene = sync.sync_document_to_scene(&doc, None, &[id]);
        let vp = Viewport::new(Vec2::new(0.0, 0.0), 1.0);
        let hit = hit_test(&sync, &scene, 15.0, 15.0, vp);
        assert_eq!(
            hit,
            Some(id),
            "hit-test must walk through the highlight overlay"
        );
    }
}
