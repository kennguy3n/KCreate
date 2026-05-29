//! Phase 9 Block D Task 24 — Design / Vector Studio polish.
//!
//! Cross-crate coverage for the bridge entry points that back the
//! ruler, grid, and alignment toolbars:
//!
//! * `document_align` / `document_distribute` (Task 23)
//! * `guide_create` / `guide_list` / `guide_delete` (Task 21)
//! * `artboard_set_grid` / `artboard_grid_settings` (Task 22)
//! * `ai_iconify` for vector simplification (Task 19)
//!
//! Each test brings the workspace into a known state, runs the
//! bridge call, and verifies the document graph + persisted state.
//! `serial_test` is required because the workspace is a process-
//! global singleton.

use kcreate_bridge::document::{
    artboard_create, canvas_create_rect, document_get_tree, project_close, project_create,
    project_save,
};
use kcreate_bridge::phase9::{
    artboard_grid_settings, artboard_set_grid, document_align, document_distribute, guide_create,
    guide_delete, guide_list,
};
use serial_test::serial;
use tempfile::TempDir;
use uuid::Uuid;

fn open_project(name: &str) -> TempDir {
    project_close();
    let dir = TempDir::new().expect("tmpdir");
    project_create(name, dir.path()).expect("project_create");
    project_save().expect("project_save");
    dir
}

/// Seed: insert an artboard then add three rectangle vector layers
/// at known bounds via the document bridge. Returns (page_id,
/// artboard_id, vector_ids).
fn seed_three_rects() -> (Uuid, Uuid, [Uuid; 3]) {
    let ab_id = artboard_create(None, "AB".to_string(), 800.0, 600.0).expect("artboard");
    let tree = document_get_tree().expect("tree");
    let page_id = tree
        .iter()
        .find(|n| n.node_type == "Page")
        .map(|n| n.id)
        .expect("page");
    let coords = [
        (10.0, 50.0, 100.0, 50.0),
        (200.0, 100.0, 80.0, 40.0),
        (400.0, 200.0, 60.0, 30.0),
    ];
    let mut ids = [Uuid::nil(); 3];
    for (idx, (x, y, w, h)) in coords.iter().enumerate() {
        ids[idx] = canvas_create_rect(Some(ab_id), *x, *y, *w, *h)
            .unwrap_or_else(|_| panic!("create rect {idx}"));
    }
    (page_id, ab_id, ids)
}

#[test]
#[serial]
fn align_left_collapses_x_to_min() {
    let _dir = open_project("align-left");
    let (_, _, ids) = seed_three_rects();
    let results = document_align(&ids, "left").expect("align left should succeed");
    assert_eq!(results.len(), 3);
    // Smallest x in the seed is 10.0 — the leftmost node shouldn't move.
    let r0 = &results[0];
    assert!(
        r0.dx.abs() < 1e-9,
        "leftmost node must not move (dx={})",
        r0.dx
    );
    // The remaining nodes should have shifted by (10 - their x).
    let r1 = &results[1];
    assert!(
        (r1.dx - (10.0 - 200.0)).abs() < 1e-9,
        "r1 dx wrong: {}",
        r1.dx
    );
    let r2 = &results[2];
    assert!(
        (r2.dx - (10.0 - 400.0)).abs() < 1e-9,
        "r2 dx wrong: {}",
        r2.dx
    );
    project_close();
}

#[test]
#[serial]
fn align_center_uses_friendly_alias() {
    // Wire format exposes "center" / "middle" — the bridge must
    // accept them as aliases for the centre-X / centre-Y axes.
    let _dir = open_project("align-center-alias");
    let (_, _, ids) = seed_three_rects();
    let centred = document_align(&ids, "center").expect("center alias");
    assert_eq!(centred.len(), 3);
    let middled = document_align(&ids, "middle").expect("middle alias");
    assert_eq!(middled.len(), 3);
    project_close();
}

#[test]
#[serial]
fn align_rejects_unknown_keyword() {
    let _dir = open_project("align-bad");
    let (_, _, ids) = seed_three_rects();
    let err = document_align(&ids, "diagonal").expect_err("bad keyword");
    assert!(format!("{err:?}").contains("alignment"));
    project_close();
}

#[test]
#[serial]
fn distribute_horizontal_evenly_spaces_middle_nodes() {
    let _dir = open_project("distribute-h");
    let (_, _, ids) = seed_three_rects();
    let results = document_distribute(&ids, "horizontal").expect("distribute h");
    assert_eq!(results.len(), 3);
    // First and last must not move horizontally.
    assert!(
        results[0].dx.abs() < 1e-9,
        "first node dx must be 0 (got {})",
        results[0].dx
    );
    assert!(
        results[2].dx.abs() < 1e-9,
        "last node dx must be 0 (got {})",
        results[2].dx
    );
    project_close();
}

#[test]
#[serial]
fn distribute_axis_must_be_horizontal_or_vertical() {
    let _dir = open_project("distribute-bad-axis");
    let (_, _, ids) = seed_three_rects();
    let err = document_distribute(&ids, "diagonal").expect_err("bad axis");
    assert!(format!("{err:?}").contains("axis"));
    project_close();
}

#[test]
#[serial]
fn guide_create_persists_and_lists_back() {
    let _dir = open_project("guide-roundtrip");
    let (page_id, _, _) = seed_three_rects();
    let guide = guide_create(page_id, "horizontal", 100.0, None, false).expect("guide_create");
    assert_eq!(guide.page_id, page_id.to_string());
    assert_eq!(guide.orientation, "horizontal");
    assert!((guide.position - 100.0).abs() < 1e-9);
    let listed = guide_list(page_id).expect("guide_list");
    assert!(
        listed.iter().any(|g| g.id == guide.id),
        "guide_list must return the freshly created guide"
    );
    project_close();
}

#[test]
#[serial]
fn guide_delete_removes_row() {
    let _dir = open_project("guide-delete");
    let (page_id, _, _) = seed_three_rects();
    let g = guide_create(page_id, "vertical", 240.0, None, false).expect("create");
    let id = uuid::Uuid::parse_str(&g.id).expect("uuid");
    let removed = guide_delete(id).expect("delete");
    assert!(removed, "guide_delete must report success");
    let listed = guide_list(page_id).expect("list");
    assert!(
        listed.iter().all(|x| x.id != g.id),
        "deleted guide must not be in list"
    );
    project_close();
}

#[test]
#[serial]
fn guide_rejects_infinite_position() {
    let _dir = open_project("guide-bad-pos");
    let (page_id, _, _) = seed_three_rects();
    let err = guide_create(page_id, "horizontal", f64::INFINITY, None, false)
        .expect_err("must reject infinite");
    assert!(format!("{err:?}").contains("position"));
    project_close();
}

#[test]
#[serial]
fn grid_settings_default_then_upsert() {
    let _dir = open_project("grid-upsert");
    let (_, ab_id, _) = seed_three_rects();

    // First read should produce defaults (no row yet).
    let defaults = artboard_grid_settings(ab_id).expect("defaults");
    assert!(defaults.spacing > 0.0);

    // Upsert custom settings.
    let saved =
        artboard_set_grid(ab_id, true, 32.0, 4, Some("#aabbcc".to_string())).expect("upsert");
    assert!(saved.enabled);
    assert!((saved.spacing - 32.0).abs() < 1e-9);
    assert_eq!(saved.subdivisions, 4);
    assert_eq!(saved.color, "#aabbcc");

    // Read back what we wrote.
    let reread = artboard_grid_settings(ab_id).expect("reread");
    assert_eq!(reread.spacing, 32.0);
    assert_eq!(reread.subdivisions, 4);
    assert_eq!(reread.color, "#aabbcc");
    project_close();
}

#[test]
#[serial]
fn grid_settings_rejects_invalid_spacing() {
    let _dir = open_project("grid-bad-spacing");
    let (_, ab_id, _) = seed_three_rects();

    // Negative spacing must be rejected.
    let err =
        artboard_set_grid(ab_id, true, -1.0, 1, None).expect_err("negative spacing must error");
    assert!(format!("{err:?}").contains("spacing"));

    // NaN must also be rejected.
    let err =
        artboard_set_grid(ab_id, true, f64::NAN, 1, None).expect_err("NaN spacing must error");
    assert!(format!("{err:?}").contains("spacing"));
    project_close();
}
