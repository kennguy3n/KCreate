//! Cross-crate proof of the shared-memory present handoff.
//!
//! The native / shared-memory present path lets the Electron renderer
//! process read each rendered frame directly out of an mmap'd ring
//! instead of receiving a per-frame structured-clone over IPC. This test
//! drives the bridge's *public* state API the way the Electron host does:
//!
//!   main process    : `shared_present_enable(w, h)` -> descriptor
//!   renderer process: `shared_reader_open(descriptor)`
//!   every frame     : main `render(...)` publishes into the ring,
//!                     renderer `shared_reader_read_into(..)` reads it
//!
//! Here the two roles are co-resident (one process), but they communicate
//! only through the descriptor + the backing file — exactly the
//! production contract. It asserts (a) a full frame round-trips
//! byte-exact, (b) full-frame churn (a pan via `set_viewport`) flows over
//! shared memory with no IPC, and (c) the path degrades gracefully to the
//! dirty-rect IPC path when the ring is disabled.
//!
//! Gated on `native_canvas`; run with
//! `cargo test -p kcreate_tests --features native_canvas`.
#![cfg(feature = "native_canvas")]

use kcreate_bridge::state;
use serial_test::serial;

// A red rect on a dark background, sized to land inside a 64x48 target at
// the identity viewport so the frame is provably non-blank.
const SCENE: &str = r#"{
    "clear_color": [0.05, 0.05, 0.08, 1.0],
    "objects": [{
        "id": 1, "z": 0, "translation": [0.0, 0.0],
        "style": { "fill": [0.95, 0.2, 0.25, 1.0], "stroke": null },
        "kind": { "type": "rect", "x": 8.0, "y": 8.0, "width": 32.0, "height": 24.0 }
    }]
}"#;

fn nonzero_pixels(bytes: &[u8], clear: [u8; 4]) -> usize {
    bytes
        .chunks_exact(4)
        .filter(|px| px[0] != clear[0] || px[1] != clear[1] || px[2] != clear[2])
        .count()
}

#[test]
#[serial]
fn full_frame_round_trips_and_pan_flows_over_shared_memory() {
    state::shutdown();
    state::init(64, 48).expect("init renderer");

    // Main process: stand up the ring and hand the renderer its map.
    let descriptor = state::shared_present_enable(64, 48).expect("enable shared present");
    assert_eq!((descriptor.width, descriptor.height), (64, 48));
    // The mapped region holds a header + N pixel slots, so it is strictly
    // larger than a single frame's pixel bytes.
    assert!(
        descriptor.len > u64::from(64u32 * 48 * 4),
        "region spans header + ring slots"
    );

    // Renderer process: map the same backing file.
    state::shared_reader_open(&descriptor).expect("open reader");
    let frame_len = state::shared_reader_frame_len().expect("reader open");
    assert_eq!(frame_len, 64 * 48 * 4);

    // Main process: render publishes the full frame into the ring.
    let first = state::render(SCENE).expect("render");

    // Renderer process: read the frame straight out of shared memory.
    let mut dest = vec![0u8; frame_len];
    let meta = state::shared_reader_read_into(None, &mut dest)
        .expect("read")
        .expect("a fresh frame is available");
    assert_eq!(meta.frame_id, first.0);
    assert_eq!((meta.width, meta.height), (64, 48));
    assert!(meta.full, "publisher ships full frames");

    // Byte-exact against the IPC path: both present the same framebuffer.
    let ipc_bytes = state::get_frame_bytes()
        .expect("frame bytes")
        .expect("a frame is published");
    assert_eq!(dest, ipc_bytes, "shared frame must match the IPC frame");
    assert!(
        nonzero_pixels(&dest, [13, 13, 20, 255]) > 0,
        "the rendered rect must be present (not a blank frame)"
    );

    // Already-consumed frame is not re-copied.
    assert!(
        state::shared_reader_read_into(Some(meta.frame_id), &mut dest)
            .expect("read")
            .is_none(),
        "no copy when nothing newer is published"
    );

    // Full-frame churn: a pan repaints the whole surface. The new frame
    // must flow to the renderer over shared memory (zero IPC).
    state::set_viewport(12.0, 7.0, 1.5).expect("pan");
    state::render_current().expect("render after pan");
    let panned = state::shared_reader_read_into(Some(meta.frame_id), &mut dest)
        .expect("read")
        .expect("the pan produced a newer frame");
    assert!(
        panned.frame_id > meta.frame_id,
        "pan must advance the published frame ({} -> {})",
        meta.frame_id,
        panned.frame_id
    );
    assert!(panned.full, "a pan is a full-frame present");

    state::shutdown();
}

#[test]
#[serial]
fn falls_back_to_ipc_when_ring_disabled() {
    state::shutdown();
    state::init(48, 32).expect("init renderer");

    // No ring enabled: a reader read is a clean no-op, and rendering still
    // publishes a frame the IPC path can pick up — no regression.
    let mut scratch = vec![0u8; 48 * 32 * 4];
    assert!(
        state::shared_reader_read_into(None, &mut scratch)
            .expect("read with no reader open")
            .is_none(),
        "reading with no ring open yields nothing, not an error"
    );

    let id = state::render(SCENE).expect("render without shared present");
    assert!(id.0 > 0);
    let info = state::get_frame_info()
        .expect("frame info")
        .expect("the IPC path still publishes a frame");
    assert_eq!(info.frame_id, id.0);

    // Enabling then disabling the ring returns to the IPC path cleanly.
    let descriptor = state::shared_present_enable(48, 32).expect("enable");
    assert!(state::shared_present_enabled());
    state::shared_reader_open(&descriptor).expect("open reader");
    state::shared_present_disable();
    state::shared_reader_close();
    assert!(!state::shared_present_enabled());

    // Still repaints after teardown via the dirty-rect IPC path: a
    // viewport change marks the renderer dirty, so a fresh frame is
    // published over IPC even with the ring gone.
    state::set_viewport(9.0, 5.0, 2.0).expect("pan");
    state::render_current().expect("render after disable");
    let after = state::get_frame_info()
        .expect("frame info")
        .expect("the IPC fallback still publishes frames");
    assert!(
        after.frame_id > id.0,
        "fallback present must keep advancing frames ({} -> {})",
        id.0,
        after.frame_id
    );

    state::shutdown();
}
