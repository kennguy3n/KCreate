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

use kcreate_bridge::document::{document_get_tree, project_close, project_create};
use kcreate_bridge::phase2::{template_instantiate, template_list, template_thumbnail};
use kcreate_core::{bundled_templates, TemplateCategory};
use serial_test::serial;
use tempfile::TempDir;

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
    // Point the bridge marketplace singleton at a private, empty dir
    // BEFORE the first bridge call so seeding (copy-if-empty) writes
    // the bundled catalog here instead of into the real
    // ~/.kcreate/templates. Kept alive for the whole test.
    let tmp = TempDir::new().expect("tmpdir");
    std::env::set_var("KCREATE_TEMPLATE_DIR", tmp.path());

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
        let dir = tmp.path().join(t.dir_name);
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
