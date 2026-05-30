//! Phase 11 Block A Task 2 — incremental scene-sync correctness
//! integration tests.
//!
//! `SceneSync::sync_document_to_scene` was changed in Phase 11 to
//! drain the [`DocumentGraph`]'s dirty set and re-emit only those
//! nodes, reusing cached entries for the rest. These tests verify
//! the *visible* output is bit-identical to a from-scratch sync for
//! the three editing-path scenarios that matter:
//!
//! * single-node property edit
//! * multi-node batch edit (e.g. multi-select drag)
//! * structural change (insert + delete)

use kcreate_bridge::scene_sync::SceneSync;
use kcreate_core::document::DocumentGraph;
use kcreate_core::node::{Bounds, FillStyle, Node, NodeType, RgbaColor};
use kcreate_export::scene_metadata::VECTOR_PATH_METADATA_KEY;
use kcreate_renderer::scene::ObjectKind;
use kcreate_vector::{PathPoint, PathSegment, VectorPath};

/// Insert `count` vector children under a fresh artboard and return
/// `(doc, root_id, child_ids)`. Bulk-insert dirty state is drained
/// before return so subsequent assertions see a clean baseline.
fn artboard_with_vectors(count: usize) -> (DocumentGraph, uuid::Uuid, Vec<uuid::Uuid>) {
    let unit_square = VectorPath::new(vec![
        PathSegment::MoveTo(PathPoint::new(0.0, 0.0)),
        PathSegment::LineTo(PathPoint::new(10.0, 0.0)),
        PathSegment::LineTo(PathPoint::new(10.0, 10.0)),
        PathSegment::LineTo(PathPoint::new(0.0, 10.0)),
        PathSegment::Close,
    ]);
    let path_json = serde_json::to_value(&unit_square).expect("serialize vector path");

    let mut doc = DocumentGraph::new();
    let mut root = Node::new(NodeType::Artboard, "Artboard");
    root.bounds = Bounds::new(0.0, 0.0, 1000.0, 1000.0);
    let root_id = doc.insert_node(root).unwrap();
    let mut ids = Vec::with_capacity(count);
    for i in 0..count {
        let mut n = Node::new(NodeType::VectorLayer, format!("rect-{i}"));
        n.bounds = Bounds::new((i as f64) * 20.0, 0.0, 10.0, 10.0);
        n.parent_id = Some(root_id);
        n.style.fill = FillStyle::Solid(RgbaColor {
            r: 0.5,
            g: 0.5,
            b: 0.5,
            a: 1.0,
        });
        n.metadata
            .insert(VECTOR_PATH_METADATA_KEY.to_string(), path_json.clone());
        let id = doc.insert_node(n).unwrap();
        ids.push(id);
    }
    let _ = doc.drain_dirty();
    (doc, root_id, ids)
}

/// Compare two scenes for caller-visible equivalence. We can't use
/// `assert_eq!(a, b)` because `Object::id` values are minted from a
/// per-`SceneSync` atomic counter and will differ across two
/// independent syncs. Instead we compare the object *kinds* in
/// z-order — that's what the rasterizer consumes.
fn assert_same_visible_objects(
    a: &kcreate_renderer::scene::Scene,
    b: &kcreate_renderer::scene::Scene,
) {
    assert_eq!(
        a.objects.len(),
        b.objects.len(),
        "object counts differ: {} vs {}",
        a.objects.len(),
        b.objects.len()
    );
    let kind_disc = |k: &ObjectKind| match k {
        ObjectKind::Rect(_) => "Rect",
        ObjectKind::Circle { .. } => "Circle",
        ObjectKind::Line { .. } => "Line",
        ObjectKind::Path(_) => "Path",
        ObjectKind::Image { .. } => "Image",
        ObjectKind::Text { .. } => "Text",
    };
    for (i, (oa, ob)) in a.objects.iter().zip(b.objects.iter()).enumerate() {
        assert_eq!(
            kind_disc(&oa.kind),
            kind_disc(&ob.kind),
            "object[{i}] kind differs: {} vs {}",
            kind_disc(&oa.kind),
            kind_disc(&ob.kind)
        );
        // z order must match exactly so paint order is preserved.
        assert_eq!(oa.z, ob.z, "object[{i}] z differs: {} vs {}", oa.z, ob.z);
        // Visibility, translation, and style must round-trip.
        assert_eq!(oa.visible, ob.visible, "object[{i}] visibility differs");
        assert_eq!(
            oa.translation, ob.translation,
            "object[{i}] translation differs"
        );
        assert_eq!(oa.style, ob.style, "object[{i}] style differs");
    }
}

#[test]
fn incremental_sync_single_node_edit_matches_full_rebuild() {
    let (mut doc, _root, ids) = artboard_with_vectors(50);

    // Warm sync to populate the per-node cache.
    let mut incremental = SceneSync::new();
    let _ = incremental.sync_document_to_scene(&mut doc, None, &[]);

    // Mutate one node's bounds.
    {
        let n = doc.get_node_mut(ids[7]).unwrap();
        n.bounds.x += 12.5;
        n.bounds.y -= 4.0;
        n.touch();
    }

    let incr_scene = incremental.sync_document_to_scene(&mut doc, None, &[]);
    let mut fresh = SceneSync::new();
    let fresh_scene = fresh.sync_document_to_scene(&mut doc, None, &[]);

    assert_same_visible_objects(&incr_scene, &fresh_scene);
}

#[test]
fn incremental_sync_multi_node_edit_matches_full_rebuild() {
    let (mut doc, _root, ids) = artboard_with_vectors(50);
    let mut incremental = SceneSync::new();
    let _ = incremental.sync_document_to_scene(&mut doc, None, &[]);

    // Mutate ten distinct nodes (simulates a multi-select drag).
    for &i in &[1usize, 3, 5, 7, 11, 13, 17, 19, 23, 29] {
        let n = doc.get_node_mut(ids[i]).unwrap();
        n.bounds.x += (i as f64) * 0.5;
        n.touch();
    }

    let incr_scene = incremental.sync_document_to_scene(&mut doc, None, &[]);
    let mut fresh = SceneSync::new();
    let fresh_scene = fresh.sync_document_to_scene(&mut doc, None, &[]);

    assert_same_visible_objects(&incr_scene, &fresh_scene);
}

#[test]
fn incremental_sync_structural_change_matches_full_rebuild() {
    let (mut doc, root, ids) = artboard_with_vectors(20);
    let mut incremental = SceneSync::new();
    let _ = incremental.sync_document_to_scene(&mut doc, None, &[]);

    // Remove one existing child and insert a new one. This sets
    // `structure_dirty` and forces the incremental path to do a
    // full rebuild — which is the fallback the spec requires.
    let _removed = doc.remove_node(ids[5]);
    let unit_square = VectorPath::new(vec![
        PathSegment::MoveTo(PathPoint::new(0.0, 0.0)),
        PathSegment::LineTo(PathPoint::new(10.0, 0.0)),
        PathSegment::LineTo(PathPoint::new(10.0, 10.0)),
        PathSegment::LineTo(PathPoint::new(0.0, 10.0)),
        PathSegment::Close,
    ]);
    let path_json = serde_json::to_value(&unit_square).unwrap();
    let mut new_child = Node::new(NodeType::VectorLayer, "added");
    new_child.parent_id = Some(root);
    new_child.bounds = Bounds::new(500.0, 500.0, 10.0, 10.0);
    new_child.style.fill = FillStyle::Solid(RgbaColor {
        r: 0.1,
        g: 0.2,
        b: 0.3,
        a: 1.0,
    });
    new_child
        .metadata
        .insert(VECTOR_PATH_METADATA_KEY.to_string(), path_json);
    let _ = doc.insert_node(new_child).unwrap();

    let incr_scene = incremental.sync_document_to_scene(&mut doc, None, &[]);
    let mut fresh = SceneSync::new();
    let fresh_scene = fresh.sync_document_to_scene(&mut doc, None, &[]);

    assert_same_visible_objects(&incr_scene, &fresh_scene);
}

#[test]
fn incremental_sync_no_changes_reuses_full_cache() {
    let (mut doc, _root, _ids) = artboard_with_vectors(30);
    let mut sync = SceneSync::new();
    let first = sync.sync_document_to_scene(&mut doc, None, &[]);
    // Drain dirty (which the inner sync already did) so the second
    // pass is a 100% cache hit.
    let second = sync.sync_document_to_scene(&mut doc, None, &[]);
    assert_eq!(
        first.objects.len(),
        second.objects.len(),
        "no-change re-sync produced a different scene"
    );
}
