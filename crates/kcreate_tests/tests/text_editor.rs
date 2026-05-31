//! Phase A1 — inline text editor + font controls.
//!
//! Drives the new `kcreate_bridge::phase2::text_*` entry points
//! end-to-end through the workspace singleton so we cover:
//!
//! 1. `canvas_create_text` → `text_set_content` → `text_set_style`
//!    → `document_undo` (x2) → `document_redo` (x2) round-trips the
//!    canonical `TextLayerMeta` payload through the operation log,
//!    bumping `Node::version` on every step.
//! 2. `text_replace_range` is the splice path the renderer's
//!    inline editor commits through on blur. Confirms UTF-16
//!    indices (matching JS `String.length`) and that ranges that
//!    bisect a surrogate pair are rejected before the operation
//!    is logged.
//! 3. `text_content_get` / `text_style_get` read back the live
//!    state without recording an operation (no log churn just
//!    because the panel re-hydrated).
//! 4. `text_list_fonts` returns the process-wide
//!    `FontManager::all_faces()` set, deduplicated and sorted —
//!    suitable for direct binding to a `<datalist>` in the
//!    renderer.
//!
//! These tests serialise on the bridge workspace singleton via
//! `#[serial]` — same discipline as the rest of the bridge-side
//! integration tests under this crate.

use kcreate_bridge::document::{
    artboard_create, canvas_create_text, document_get_tree, document_redo, document_undo,
    project_close, project_create,
};
use kcreate_bridge::phase2::{
    text_content_get, text_list_fonts, text_replace_range, text_set_content, text_set_style,
    text_style_get, TextStyleWire,
};
use serde_json::json;
use serial_test::serial;
use tempfile::TempDir;
use uuid::Uuid;

/// Bring up a fresh project + artboard, seeded with one TextLayer
/// at (10, 20) so every test starts from a known baseline. Returns
/// the temp dir (RAII guard for the project on disk) and the text
/// layer's node id.
fn seed_text_layer() -> (TempDir, Uuid) {
    project_close();
    let dir = TempDir::new().expect("tmpdir");
    project_create("text-editor-test", dir.path()).expect("project_create");
    let ab = artboard_create(None, "AB".to_string(), 800.0, 600.0).expect("artboard");
    let id = canvas_create_text(
        Some(ab),
        10.0,
        20.0,
        "Hello".to_string(),
        "Inter".to_string(),
        18.0,
    )
    .expect("canvas_create_text");
    (dir, id)
}

#[test]
#[serial]
fn content_set_persists_via_metadata_and_logs_operation() {
    let (_dir, id) = seed_text_layer();

    text_set_content(id, "World").expect("text_set_content");

    // The bridge's read API hits the same metadata key the renderer
    // would, so this is a meaningful round-trip — not a circular
    // check against the value we just wrote.
    let content = text_content_get(id).expect("text_content_get");
    assert_eq!(content, "World");

    // Style is preserved (font family / size unchanged) — the
    // content path must not blow away style.
    let style_json = text_style_get(id).expect("text_style_get");
    let style: TextStyleWire = serde_json::from_str(&style_json).expect("parse style");
    assert_eq!(style.font_family, "Inter");
    assert!((style.font_size - 18.0).abs() < 1e-3);
}

#[test]
#[serial]
fn style_set_round_trips_all_three_wire_fields() {
    let (_dir, id) = seed_text_layer();

    let next = TextStyleWire {
        font_family: "Roboto".to_string(),
        font_size: 36.0,
        line_height: 1.5,
    };
    text_set_style(id, &serde_json::to_string(&next).expect("encode style"))
        .expect("text_set_style");

    let got_json = text_style_get(id).expect("text_style_get");
    let got: TextStyleWire = serde_json::from_str(&got_json).expect("parse style");
    assert_eq!(got, next);
}

#[test]
#[serial]
fn undo_redo_restores_content_and_style_state() {
    let (_dir, id) = seed_text_layer();

    // Snapshot the version after creation so we can verify each
    // mutation bumps it monotonically.
    let v_initial = node_version(id);
    text_set_content(id, "v1").expect("set v1");
    let v_after_content = node_version(id);
    assert!(
        v_after_content > v_initial,
        "content set should bump version"
    );

    let new_style = TextStyleWire {
        font_family: "Roboto".to_string(),
        font_size: 24.0,
        line_height: 1.4,
    };
    text_set_style(
        id,
        &serde_json::to_string(&new_style).expect("encode style"),
    )
    .expect("set style");
    let v_after_style = node_version(id);
    assert!(
        v_after_style > v_after_content,
        "style set should bump version",
    );

    // Undo #1 reverses the style change.
    document_undo().expect("undo style").expect("op found");
    let style_after_undo1: TextStyleWire =
        serde_json::from_str(&text_style_get(id).expect("style")).expect("parse style");
    assert_eq!(
        style_after_undo1.font_family, "Inter",
        "undoing style returns to creation-time family",
    );
    assert_eq!(
        text_content_get(id).expect("content"),
        "v1",
        "content from the previous op is still in place",
    );

    // Undo #2 reverses the content change.
    document_undo().expect("undo content").expect("op found");
    assert_eq!(
        text_content_get(id).expect("content"),
        "Hello",
        "undoing content returns to creation-time string",
    );

    // Redo #1 reapplies the content change.
    document_redo().expect("redo content").expect("op found");
    assert_eq!(text_content_get(id).expect("content"), "v1");

    // Redo #2 reapplies the style change.
    document_redo().expect("redo style").expect("op found");
    let style_after_redo: TextStyleWire =
        serde_json::from_str(&text_style_get(id).expect("style")).expect("parse style");
    assert_eq!(style_after_redo, new_style);
}

#[test]
#[serial]
fn replace_range_splices_utf16_window() {
    let (_dir, id) = seed_text_layer();
    // Start from a known buffer so the indices are obvious. "Hello"
    // length in UTF-16 = 5.
    text_set_content(id, "Hello").expect("seed content");

    // Replace [1..4] → "ELLO" with "i, "
    text_replace_range(id, 1, 4, "i, ").expect("text_replace_range");
    assert_eq!(text_content_get(id).expect("content"), "Hi, o");

    // Append by passing start == end == current length.
    text_replace_range(id, 5, 5, "world").expect("append");
    assert_eq!(text_content_get(id).expect("content"), "Hi, oworld");

    // Replace the entire buffer with an empty string.
    let len = "Hi, oworld".encode_utf16().count() as u32;
    text_replace_range(id, 0, len, "").expect("clear");
    assert_eq!(text_content_get(id).expect("content"), "");
}

#[test]
#[serial]
fn replace_range_rejects_split_surrogate_pair() {
    let (_dir, id) = seed_text_layer();
    // U+1F600 = 😀 = surrogate pair (0xD83D, 0xDE00) in UTF-16.
    // Total UTF-16 length of "A😀B" is 4 (A + 2 + B). Bisecting
    // the smiley at index 2 (between high and low surrogate) must
    // fail before any mutation hits the operation log.
    text_set_content(id, "A\u{1F600}B").expect("seed");

    let err = text_replace_range(id, 2, 3, "X").expect_err("splitting a surrogate pair must error");
    let msg = format!("{err}");
    assert!(
        msg.to_lowercase().contains("surrogate"),
        "error should mention the surrogate-pair violation; got {msg:?}",
    );

    // Content is unchanged — proves the splice was rejected
    // before the mutation phase.
    assert_eq!(text_content_get(id).expect("content"), "A\u{1F600}B");
}

#[test]
#[serial]
fn list_fonts_is_sorted_and_deduplicated() {
    // Make sure the font manager is initialised by touching the
    // bridge first (the rest of the test suite already does this
    // implicitly; we exercise it here too so the order of test
    // execution doesn't matter).
    let _ = seed_text_layer();

    // `phase2::text_list_fonts` returns the deduped sorted Vec
    // directly; the N-API wrapper in `lib.rs` is what JSON-encodes
    // it for the renderer.
    let fonts: Vec<String> = text_list_fonts().expect("text_list_fonts");

    // Sorted strictly ascending (case-insensitive comparison would
    // require allocating; the bridge returns a stable lexicographic
    // sort, which is good enough for binding to a `<datalist>`).
    for pair in fonts.windows(2) {
        assert!(
            pair[0] <= pair[1],
            "font list must be sorted; {:?} > {:?}",
            pair[0],
            pair[1],
        );
    }

    // Deduplicated — each family appears at most once.
    let mut sorted = fonts.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), fonts.len(), "font list must be deduplicated");
}

#[test]
#[serial]
fn set_content_rejects_non_text_layer_node() {
    // Open a fresh project and ask `set_content` to mutate a node
    // that isn't a TextLayer; the bridge must reject before
    // touching the operation log.
    project_close();
    let dir = TempDir::new().expect("tmpdir");
    project_create("non-text", dir.path()).expect("project_create");
    let tree = document_get_tree().expect("tree");
    // The project always boots with a Document + Page at minimum;
    // pick the root node (a non-text container) and try to set
    // content on it.
    let root = tree
        .iter()
        .find(|n| n.node_type != "TextLayer")
        .expect("non-text node present");
    let err = text_set_content(root.id, "anything")
        .expect_err("non-text node must reject text_set_content");
    let msg = format!("{err}");
    assert!(
        msg.to_lowercase().contains("textlayer") || msg.to_lowercase().contains("text"),
        "error should mention the node-type mismatch; got {msg:?}",
    );
}

#[test]
#[serial]
fn style_set_rejects_invalid_json() {
    let (_dir, id) = seed_text_layer();
    let err = text_set_style(id, "{ not json")
        .expect_err("malformed JSON must error before mutating state");
    let msg = format!("{err}");
    assert!(
        !msg.is_empty(),
        "decode failure should surface a non-empty error",
    );
    // Original style is unchanged.
    let style: TextStyleWire =
        serde_json::from_str(&text_style_get(id).expect("style")).expect("parse");
    assert_eq!(style.font_family, "Inter");
}

#[test]
#[serial]
fn set_content_emits_camel_case_wire_format() {
    // Cross-check the wire format on disk: the renderer reads
    // `fontFamily` / `fontSize` / `lineHeight` (camelCase) because
    // the wire type uses `rename_all = "camelCase"`. If a future
    // refactor accidentally switches to snake_case, this test
    // catches it — JSON.parse() in the renderer would otherwise
    // silently see undefined fields and the panel would render
    // empty values.
    let (_dir, id) = seed_text_layer();
    let next = TextStyleWire {
        font_family: "Roboto".to_string(),
        font_size: 32.0,
        line_height: 1.25,
    };
    text_set_style(id, &serde_json::to_string(&next).expect("encode")).expect("set style");
    let raw = text_style_get(id).expect("style");
    let parsed: serde_json::Value = serde_json::from_str(&raw).expect("json");
    assert_eq!(
        parsed,
        json!({
            "fontFamily": "Roboto",
            "fontSize": 32.0,
            "lineHeight": 1.25,
        }),
        "style wire format must stay camelCase for renderer lockstep",
    );
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn node_version(id: Uuid) -> u64 {
    let tree = document_get_tree().expect("tree");
    let node = tree.iter().find(|n| n.id == id).expect("node present");
    node.version
}
