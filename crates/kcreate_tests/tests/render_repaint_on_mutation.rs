//! WS-2 regression — a document mutation must repaint the renderer.
//!
//! Before the fix, `sync_scene_locked` rebuilt the renderer `Scene`
//! after every edit but never invalidated the renderer's dirty region.
//! `render_frame` therefore short-circuited (`dirty.is_none() &&
//! sequence > 0`) and kept republishing the stale initial frame, which
//! is what produced the blank editor canvas: objects existed in the
//! document graph but never reached the presented buffer.
//!
//! This test exercises the real seam end-to-end — open a project, init
//! the renderer, then mutate the document — and asserts that each
//! mutation advances the published frame id (i.e. an actual repaint
//! happened, not a cache hit).
//!
//! It deliberately lives in its own integration binary: the renderer
//! and workspace are process-global singletons, and a dedicated file
//! gives this test a clean renderer that no other test has driven.

use kcreate_bridge::document::{
    artboard_create, canvas_create_rect, project_close, project_create,
};
use kcreate_bridge::state;
use serial_test::serial;
use tempfile::TempDir;

#[test]
#[serial]
fn document_mutation_advances_published_frame() {
    project_close();
    let dir = TempDir::new().expect("tmpdir");
    project_create("repaint", dir.path()).expect("project_create");

    // The renderer must be live before mutations so `sync_scene_locked`
    // actually paints (otherwise `render_scene` returns NotInitialized,
    // which Part B intentionally swallows as a no-op).
    state::init(256, 256).expect("init renderer");

    let ab = artboard_create(None, "AB".to_string(), 200.0, 200.0).expect("artboard");

    // First mutation publishes a frame.
    canvas_create_rect(Some(ab), 10.0, 10.0, 50.0, 50.0).expect("create rect 1");
    let first = state::get_frame_info()
        .expect("frame info")
        .expect("a mutation must have published a frame");
    assert!(
        first.frame_id > 0,
        "the first mutation must publish a frame, got id {}",
        first.frame_id
    );

    // Second mutation must repaint — a fresh frame id, not the cached one.
    canvas_create_rect(Some(ab), 120.0, 120.0, 50.0, 50.0).expect("create rect 2");
    let second = state::get_frame_info()
        .expect("frame info")
        .expect("the second mutation must have published a frame");
    assert!(
        second.frame_id > first.frame_id,
        "each document mutation must repaint the renderer: frame id must advance ({} -> {})",
        first.frame_id,
        second.frame_id
    );

    // The published frame is a real readback of the expected size — a
    // genuine paint, not an empty handle.
    let frame = state::acquire_frame()
        .expect("acquire frame")
        .expect("a frame must be available after a mutation");
    assert_eq!(frame.width, 256);
    assert_eq!(frame.height, 256);
    assert_eq!(
        frame.bytes.len(),
        (frame.width * frame.height * 4) as usize,
        "readback must be a full RGBA8 buffer"
    );

    project_close();
}
