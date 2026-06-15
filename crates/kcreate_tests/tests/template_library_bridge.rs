//! Cross-crate integration coverage for the G2 ready-made template
//! library: the bundled catalog (`kcreate_core::template_library`)
//! seeded into a marketplace dir and driven end-to-end through the
//! bridge's renderer-facing entry points
//! (`kcreate_bridge::phase2::{template_list, template_instantiate,
//! template_thumbnail}`).
//!
//! The pure-core marketplace mechanics (scan / install / remove /
//! search) are already covered by `template_marketplace.rs`. This
//! file instead exercises the *bridge* layer that the renderer
//! actually calls, and — critically — closes the drift gap between
//! the catalog authored in `kcreate_core` and the
//! `CanvasBatchItem` wire enum the bridge deserializes `content.json`
//! into:
//!
//! * Every bundled `content.json` must deserialize into the bridge's
//!   `CanvasBatchItem` (otherwise `template_instantiate` /
//!   `template_thumbnail` error) — so a schema drift between the two
//!   crates fails this test rather than shipping a broken gallery.
//! * `template_instantiate(id)` must populate the open workspace with
//!   one node per authored item under a freshly created artboard.
//! * `template_thumbnail(id)` must render a **non-blank** PNG through
//!   the real export pipeline (≥ 2 distinct colors — a flat/blank
//!   preview would have exactly one).
//!
//! The bridge's template marketplace is a process-global `OnceLock`
//! singleton keyed off `KCREATE_TEMPLATE_DIR`. Each integration-test
//! file is its own binary, so setting the env var before the first
//! bridge call deterministically points the singleton at a private
//! temp dir. `#[serial]` keeps the shared workspace singleton from
//! racing the other suites.

use std::collections::HashSet;
use std::path::Path;
use std::sync::OnceLock;

use kcreate_bridge::document::{document_get_tree, project_close, project_create, NodeInfo};
use kcreate_bridge::phase2::{template_instantiate, template_list, template_thumbnail};
use kcreate_core::{bundled_templates, TemplateCategory};
use serial_test::serial;
use tempfile::TempDir;

/// Process-wide seeded marketplace directory. The bridge's template
/// marketplace is a `OnceLock` singleton whose root is fixed at first
/// access from `KCREATE_TEMPLATE_DIR` (see `phase2::template_dir`), so
/// every test in this binary must agree on one directory AND it must
/// outlive them all — a per-test `TempDir` would be dropped (deleted)
/// while the singleton still points at it, breaking later tests. We
/// create the dir once, point the env var at it before any bridge
/// call, and keep the `TempDir` alive for the whole process (cleaned
/// up at exit).
fn shared_template_dir() -> &'static Path {
    static DIR: OnceLock<TempDir> = OnceLock::new();
    DIR.get_or_init(|| {
        let tmp = TempDir::new().expect("tmpdir");
        std::env::set_var("KCREATE_TEMPLATE_DIR", tmp.path());
        tmp
    })
    .path()
}

/// Artboards in document space, in tree order. `template_instantiate`
/// only ever touches artboards, so this is the lens the reuse-vs-append
/// tests below assert against.
fn artboards(tree: &[NodeInfo]) -> Vec<&NodeInfo> {
    tree.iter().filter(|n| n.node_type == "Artboard").collect()
}

/// Count distinct RGBA colors in a PNG, capped at `cap` so a busy
/// design doesn't allocate a huge set. A blank/flat preview collapses
/// to a single color; anything `>= 2` proves the render actually drew
/// the design.
fn distinct_colors(png: &[u8], cap: usize) -> usize {
    let img = image::load_from_memory(png).expect("decode thumbnail PNG");
    let rgba = img.to_rgba8();
    let mut seen: HashSet<[u8; 4]> = HashSet::new();
    for px in rgba.pixels() {
        seen.insert(px.0);
        if seen.len() >= cap {
            break;
        }
    }
    seen.len()
}

#[test]
#[serial]
fn bundled_catalog_seeds_lists_instantiates_and_renders() {
    // Point the bridge marketplace singleton at a private, seeded dir
    // BEFORE the first bridge call so seeding (copy-if-empty) writes
    // the bundled catalog here instead of into the real
    // ~/.kcreate/templates.
    let root = shared_template_dir();

    let catalog = bundled_templates();
    assert!(
        catalog.len() >= 18,
        "catalog should ship >= 18 templates, got {}",
        catalog.len()
    );

    // First bridge call seeds + scans the temp dir.
    let all = template_list(None, None).expect("template_list");
    assert_eq!(
        all.templates.len(),
        catalog.len(),
        "seeding + scan should surface every bundled template"
    );

    // The seeded `.ktemplate/` folders are physically on disk under
    // our override dir (proves seeding respected KCREATE_TEMPLATE_DIR).
    for t in &catalog {
        let dir = root.join(t.dir_name);
        assert!(
            dir.join("manifest.json").exists(),
            "{} manifest.json missing",
            t.dir_name
        );
        assert!(
            dir.join("content.json").exists(),
            "{} content.json missing",
            t.dir_name
        );
    }

    // Category filter narrows to exactly the catalog's members of that
    // category. MobileApp is the richest bucket (the UI-kit screens).
    let mobile_expected = catalog
        .iter()
        .filter(|t| t.manifest.category == TemplateCategory::MobileApp)
        .count();
    assert!(mobile_expected > 0, "catalog should include mobile UI kits");
    let mobile = template_list(Some(TemplateCategory::MobileApp), None).expect("filter");
    assert_eq!(mobile.templates.len(), mobile_expected);
    assert!(mobile
        .templates
        .iter()
        .all(|t| t.category == TemplateCategory::MobileApp));

    // Search narrows by name/tag/description. Every seeded template's
    // own name is a hit for itself.
    let first = &catalog[0];
    let by_name = template_list(None, Some(&first.manifest.name)).expect("search");
    assert!(
        by_name.templates.iter().any(|t| t.id == first.manifest.id),
        "search by exact name should find the template"
    );

    // End-to-end per template: instantiate into a fresh workspace and
    // render a non-blank thumbnail. This is the drift guard — if a
    // bundled content.json can't deserialize into the bridge's
    // CanvasBatchItem wire enum, both calls error here.
    for t in &catalog {
        let id = t.manifest.id;

        // --- Thumbnail: real render through the export pipeline. ---
        let thumb = template_thumbnail(id)
            .unwrap_or_else(|e| panic!("thumbnail {} ({}): {e}", t.dir_name, id));
        assert_eq!(thumb.mime, "image/png", "{} thumb mime", t.dir_name);
        assert!(
            thumb.width > 0 && thumb.height > 0,
            "{} thumb dims",
            t.dir_name
        );
        assert!(thumb.byte_size > 0, "{} thumb empty", t.dir_name);
        let colors = distinct_colors(&base64_decode(&thumb.bytes_base64), 16);
        assert!(
            colors >= 2,
            "{} thumbnail is blank (only {colors} distinct color)",
            t.dir_name
        );

        // --- Instantiate: one node per authored item under a new artboard. ---
        let _proj = open_project(t.dir_name);
        let report = template_instantiate(id)
            .unwrap_or_else(|e| panic!("instantiate {} ({}): {e}", t.dir_name, id));
        assert_eq!(
            report.node_ids.len(),
            t.content.items.len(),
            "{} should create one node per authored item",
            t.dir_name
        );
        let tree = document_get_tree().expect("document tree");
        assert!(
            tree.iter().any(|n| n.id == report.artboard_id),
            "{} artboard should be in the document tree",
            t.dir_name
        );
        project_close();
    }
}

/// "Start from template" almost always runs against a brand-new
/// scratch project, which `project_create` seeds with a single *empty*
/// default artboard at the origin. Instantiating must REUSE that
/// pristine artboard in place (resized + renamed to the template) so
/// the editor — which frames the origin — opens on the populated
/// design, rather than appending a second artboard off-screen to the
/// right and leaving the viewport on the empty default. Regression
/// guard for the live blank-canvas bug (PR #61).
#[test]
#[serial]
fn start_from_template_reuses_pristine_default_artboard() {
    shared_template_dir();
    // Ensure the marketplace singleton is seeded + scanned before we
    // resolve a template id (no-op if another test seeded first).
    let _ = template_list(None, None).expect("seed");

    let catalog = bundled_templates();
    let t = &catalog[0];
    let id = t.manifest.id;

    let _proj = open_project("reuse-default");

    // A fresh project seeds exactly one empty default artboard.
    let before = document_get_tree().expect("tree");
    let before_abs = artboards(&before);
    assert_eq!(
        before_abs.len(),
        1,
        "fresh project should seed exactly one default artboard"
    );
    assert!(
        before_abs[0].children.is_empty(),
        "the seeded default artboard should start empty"
    );

    let report = template_instantiate(id).expect("instantiate");

    let after = document_get_tree().expect("tree");
    let after_abs = artboards(&after);
    assert_eq!(
        after_abs.len(),
        1,
        "instantiating into a pristine scratch doc must REUSE the default \
         artboard, not append a second one"
    );
    let board = after_abs[0];
    assert_eq!(
        board.id, report.artboard_id,
        "the reused artboard is the one reported back to the renderer"
    );
    // Reused in place at the origin so the editor viewport lands on it.
    assert!(
        board.bounds.x.abs() < 1e-6 && board.bounds.y.abs() < 1e-6,
        "reused artboard should sit at the origin, got ({}, {})",
        board.bounds.x,
        board.bounds.y
    );
    // Resized + renamed to the template.
    assert!(
        (board.bounds.width - t.content.width).abs() < 1e-6
            && (board.bounds.height - t.content.height).abs() < 1e-6,
        "reused artboard should adopt the template's {}x{} canvas, got {}x{}",
        t.content.width,
        t.content.height,
        board.bounds.width,
        board.bounds.height
    );
    assert_eq!(
        board.name, t.manifest.name,
        "reused artboard should be renamed to the template"
    );
    // Populated: one child node per authored item, all parented to it.
    assert_eq!(
        board.children.len(),
        t.content.items.len(),
        "every authored item should be parented to the reused artboard"
    );
    assert_eq!(report.node_ids.len(), t.content.items.len());
    project_close();
}

/// The flip side of the reuse rule: once a document already holds real
/// content, a second "Start from template" must APPEND a new artboard
/// to the right (left-to-right layout) instead of clobbering the first
/// design. Guards the `None` branch of the reuse-vs-append match.
#[test]
#[serial]
fn start_from_template_appends_to_populated_document() {
    shared_template_dir();
    let _ = template_list(None, None).expect("seed");

    let catalog = bundled_templates();
    let a = &catalog[0];
    let b = &catalog[1];

    let _proj = open_project("append-populated");

    // First instantiate reuses the pristine default artboard.
    let report_a = template_instantiate(a.manifest.id).expect("instantiate a");
    let mid = document_get_tree().expect("tree");
    assert_eq!(
        artboards(&mid).len(),
        1,
        "first template reuses the default"
    );

    // Second instantiate must append a brand-new artboard.
    let report_b = template_instantiate(b.manifest.id).expect("instantiate b");
    assert_ne!(
        report_a.artboard_id, report_b.artboard_id,
        "appending must create a distinct artboard"
    );

    let after = document_get_tree().expect("tree");
    let after_abs = artboards(&after);
    assert_eq!(
        after_abs.len(),
        2,
        "second template should append a second artboard"
    );
    let board_a = after_abs
        .iter()
        .find(|n| n.id == report_a.artboard_id)
        .expect("artboard a present");
    let board_b = after_abs
        .iter()
        .find(|n| n.id == report_b.artboard_id)
        .expect("artboard b present");
    // First design untouched at the origin; second laid out to its right.
    assert!(
        board_a.bounds.x.abs() < 1e-6,
        "first design stays at origin"
    );
    assert!(
        board_b.bounds.x >= board_a.bounds.width,
        "appended artboard must sit to the right of the first (x {} >= width {})",
        board_b.bounds.x,
        board_a.bounds.width
    );
    assert_eq!(board_a.children.len(), a.content.items.len());
    assert_eq!(board_b.children.len(), b.content.items.len());
    project_close();
}

/// `ThumbnailBytes.bytes_base64` is standard (not URL-safe) base64.
fn base64_decode(s: &str) -> Vec<u8> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .expect("decode thumbnail base64")
}

fn open_project(name: &str) -> TempDir {
    project_close();
    let dir = TempDir::new().expect("project tmpdir");
    project_create(name, dir.path()).expect("project_create");
    dir
}
