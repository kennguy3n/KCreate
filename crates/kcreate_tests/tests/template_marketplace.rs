//! Cross-crate integration coverage for the Phase 3 local template
//! marketplace (Tasks 11–12).
//!
//! The marketplace surface lives in two layers:
//! 1. `kcreate_core::marketplace::LocalMarketplace` — file-system
//!    operations (scan / install_local / remove) backed by a
//!    `~/.kcreate/templates/` root.
//! 2. `kcreate_bridge::phase2::{template_list, template_install_local,
//!    template_remove}` — the renderer-facing entry points wired
//!    through to the N-API surface.
//!
//! The bridge layer is process-singletoned and serial_test-guarded
//! in its own crate's tests, so here we exercise the *core* layer
//! through `LocalMarketplace` directly with realistic `.ktemplate/`
//! folders on disk. That guarantees the renderer's IPC contract
//! (template_list / install / remove) lands against a working core
//! implementation regardless of whether the bridge singleton is
//! reset between calls.

use std::path::Path;

use kcreate_core::{
    LocalMarketplace, MarketplaceError, TemplateCategory, TemplateManifest, TemplateSource,
};
use tempfile::tempdir;
use uuid::Uuid;

fn manifest(id: Uuid, name: &str, category: TemplateCategory, tags: &[&str]) -> TemplateManifest {
    TemplateManifest {
        id,
        name: name.into(),
        description: format!("Integration test fixture: {name}"),
        category,
        tags: tags.iter().map(|t| (*t).to_string()).collect(),
        thumbnail: Some("thumb.png".into()),
        page_count: 4,
        author: Some("kcreate integration".into()),
        version: "1.0.0".into(),
        source: None,
    }
}

fn write_ktemplate(root: &Path, dir: &str, manifest: &TemplateManifest) {
    let dir = root.join(dir);
    std::fs::create_dir_all(&dir).expect("create ktemplate dir");
    let json = serde_json::to_string_pretty(manifest).expect("serialize manifest");
    std::fs::write(dir.join("manifest.json"), json).expect("write manifest");
    // Drop a placeholder thumbnail so the install path copies more
    // than just the manifest — install_local does a recursive
    // directory copy and we want the integration test to validate
    // that auxiliary files survive the copy.
    std::fs::write(dir.join("thumb.png"), b"PNG-stub").expect("write thumb");
}

#[test]
fn scan_then_list_filter_and_search_round_trip() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();

    let deck_id = Uuid::new_v4();
    let proposal_id = Uuid::new_v4();
    let brochure_id = Uuid::new_v4();
    write_ktemplate(
        root,
        "alpha-deck.ktemplate",
        &manifest(
            deck_id,
            "Alpha Deck",
            TemplateCategory::PitchDeck,
            &["sales"],
        ),
    );
    write_ktemplate(
        root,
        "beta-proposal.ktemplate",
        &manifest(
            proposal_id,
            "Beta Proposal",
            TemplateCategory::Proposal,
            &["client"],
        ),
    );
    write_ktemplate(
        root,
        "gamma-brochure.ktemplate",
        &manifest(
            brochure_id,
            "Gamma Brochure",
            TemplateCategory::Brochure,
            &["marketing", "tri-fold"],
        ),
    );

    let mut mp = LocalMarketplace::new(root);
    let count = mp.scan().unwrap();
    assert_eq!(count, 3, "all three .ktemplate folders are discovered");

    let listed = mp.list();
    assert_eq!(listed.len(), 3);
    // `list()` is sorted by name, which is a load-bearing
    // affordance for the renderer's TemplateMarketplace panel.
    assert_eq!(listed[0].name, "Alpha Deck");
    assert_eq!(listed[1].name, "Beta Proposal");
    assert_eq!(listed[2].name, "Gamma Brochure");

    // Filter by category — matches the bridge's
    // `template_list(category=..., query=None)` path.
    let decks = mp.filter_by_category(TemplateCategory::PitchDeck);
    assert_eq!(decks.len(), 1);
    assert_eq!(decks[0].id, deck_id);

    // Search by name (case-insensitive) — matches
    // `template_list(category=None, query=...)`.
    let results = mp.search("BETA");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].id, proposal_id);

    // Search by tag is just as important as search by name.
    let by_tag = mp.search("tri-fold");
    assert_eq!(by_tag.len(), 1);
    assert_eq!(by_tag[0].id, brochure_id);

    // Search by description (which the panel's keyword filter
    // also covers).
    let by_desc = mp.search("integration test");
    assert_eq!(by_desc.len(), 3);

    // Every entry has its source populated to the install dir so
    // the renderer can open the folder if it wants to.
    for t in mp.list() {
        match t
            .source
            .as_ref()
            .expect("scan populates source for every entry")
        {
            TemplateSource::Local { path } => {
                assert!(path.exists());
                assert!(path.join("manifest.json").exists());
            }
        }
    }
}

#[test]
fn install_local_copies_full_directory_and_idempotent_remove() {
    // Two-marketplace scenario: the user has an *external* template
    // directory (think a Dropbox/Documents/MyTemplates folder) and
    // wants to import a .ktemplate into the canonical
    // ~/.kcreate/templates root. This mirrors the renderer's
    // installLocal IPC: it hands the bridge an absolute source path
    // that lives outside the marketplace root.
    let external = tempdir().unwrap();
    let root = tempdir().unwrap();

    let id = Uuid::new_v4();
    write_ktemplate(
        external.path(),
        "import-me.ktemplate",
        &manifest(id, "Imported Deck", TemplateCategory::PitchDeck, &["v1"]),
    );

    let source = external.path().join("import-me.ktemplate");
    let mut mp = LocalMarketplace::new(root.path());
    mp.scan().unwrap();
    assert_eq!(mp.list().len(), 0);

    let installed = mp.install_local(&source).unwrap();
    assert_eq!(installed.id, id);
    assert_eq!(installed.name, "Imported Deck");

    // The .ktemplate folder must be physically present in the
    // marketplace root, and `source` must point at the *copy* not
    // the original.
    match installed
        .source
        .as_ref()
        .expect("install_local populates source")
    {
        TemplateSource::Local { path } => {
            assert!(path.starts_with(root.path()));
            assert!(path.join("manifest.json").exists());
            // Auxiliary files made it through the copy.
            assert!(path.join("thumb.png").exists());
        }
    }

    // Re-installing the same template id must fail — the renderer
    // surfaces this as a structured error so the user can either
    // remove the old copy or choose a different source.
    let dup = mp.install_local(&source).expect_err("duplicate install");
    assert!(matches!(dup, MarketplaceError::AlreadyInstalled(_)));

    // Remove is destructive: the .ktemplate folder is deleted off
    // disk so a subsequent scan() doesn't resurrect it.
    mp.remove(id).unwrap();
    assert_eq!(mp.list().len(), 0);

    let mut fresh = LocalMarketplace::new(root.path());
    fresh.scan().unwrap();
    assert_eq!(
        fresh.list().len(),
        0,
        "remove() deletes the on-disk template, not just the in-memory entry"
    );

    // Idempotency check: removing a no-longer-installed id must
    // return TemplateNotFound rather than silently succeeding.
    let missing = mp.remove(id).expect_err("second remove");
    assert!(matches!(missing, MarketplaceError::TemplateNotFound(_)));
}

#[test]
fn scan_skips_invalid_manifests_without_poisoning_others() {
    // One good + one corrupt manifest. The corrupt one must not
    // block discovery of the good one — the bridge logs the
    // skip but doesn't fail the entire list.
    let tmp = tempdir().unwrap();
    let root = tmp.path();

    let good_id = Uuid::new_v4();
    write_ktemplate(
        root,
        "good.ktemplate",
        &manifest(
            good_id,
            "Good Template",
            TemplateCategory::Report,
            &["valid"],
        ),
    );

    let bad = root.join("bad.ktemplate");
    std::fs::create_dir_all(&bad).unwrap();
    std::fs::write(bad.join("manifest.json"), "{ this is not json").unwrap();

    let mut mp = LocalMarketplace::new(root);
    let count = mp.scan().unwrap();
    assert_eq!(count, 1, "the corrupt manifest is silently skipped");
    assert!(mp.get(good_id).is_some());
}

#[test]
fn marketplace_default_dir_is_under_home() {
    // Sanity-check that the bridge's default template directory
    // resolves under $HOME, which is what the renderer assumes
    // when it shows the "templates root" status line.
    let dir = LocalMarketplace::default_dir();
    let dir_str = dir.to_string_lossy();
    assert!(
        dir_str.ends_with(".kcreate/templates") || dir_str.ends_with(".kcreate\\templates"),
        "default_dir = {dir_str}"
    );
}
