//! WS-2 regression — a viewport change must repaint the renderer.
//!
//! The present surface (`CanvasHost.tsx`) pans / zooms by calling
//! `set_viewport` and then `render_current`. `set_viewport` marks the
//! renderer dirty *only when the viewport actually changes*, and
//! `render_current` re-renders the cached scene *only when dirty*
//! (otherwise `render_frame` short-circuits on `dirty.is_none() &&
//! sequence > 0` and republishes the cached frame). Devin Review #44
//! flagged that this `set_viewport -> dirty -> repaint` coupling was
//! implicit, with no test pinning it down. If a refactor ever dropped
//! the dirty mark in `set_viewport`, pan / zoom would silently freeze
//! the canvas with no compile error.
//!
//! This test exercises the real seam: publish a scene, confirm a
//! no-op `render_current` does NOT repaint (precise dirty tracking),
//! then change the viewport and assert the published frame id advances
//! — and that repeating the identical viewport does not.
//!
//! It lives in its own integration binary because the renderer is a
//! process-global singleton; a dedicated file gives it a clean renderer
//! no other test has driven.

use kcreate_bridge::state;
use serial_test::serial;

const SCENE: &str = r#"{"clear_color":[0.1,0.1,0.12,1.0],"objects":[]}"#;

#[test]
#[serial]
fn viewport_change_advances_published_frame() {
    // Clean slate: another test in this binary could have left a
    // renderer attached (defensive — this file currently holds one test).
    state::shutdown();
    state::init(256, 256).expect("init renderer");

    // Publish a scene. This renders the initial frame and consumes the
    // post-init dirty region, so the renderer is now clean.
    state::render(SCENE).expect("render scene");
    let baseline = state::get_frame_info()
        .expect("frame info")
        .expect("rendering a scene must publish a frame")
        .frame_id;

    // A `render_current` with no viewport change must be a cache hit:
    // the renderer is clean, so no new frame is published.
    state::render_current().expect("render_current (no-op)");
    let after_noop = state::get_frame_info()
        .expect("frame info")
        .expect("a frame must still be available")
        .frame_id;
    assert_eq!(
        baseline, after_noop,
        "render_current with an unchanged viewport must not repaint \
         (got {baseline} -> {after_noop})"
    );

    // Changing the viewport must mark the renderer dirty so the next
    // render_current produces a fresh frame.
    state::set_viewport(137.0, 89.0, 2.5).expect("set_viewport (change)");
    state::render_current().expect("render_current (after change)");
    let after_change = state::get_frame_info()
        .expect("frame info")
        .expect("a frame must be available after a viewport change")
        .frame_id;
    assert!(
        after_change > baseline,
        "a viewport change must repaint: frame id must advance \
         ({baseline} -> {after_change})"
    );

    // Re-applying the identical viewport must NOT repaint — the dirty
    // mark is gated on an actual change, so the canvas stays free in the
    // steady state.
    state::set_viewport(137.0, 89.0, 2.5).expect("set_viewport (identical)");
    state::render_current().expect("render_current (identical viewport)");
    let after_identical = state::get_frame_info()
        .expect("frame info")
        .expect("a frame must still be available")
        .frame_id;
    assert_eq!(
        after_change, after_identical,
        "re-applying the same viewport must not repaint \
         (got {after_change} -> {after_identical})"
    );

    state::shutdown();
}
