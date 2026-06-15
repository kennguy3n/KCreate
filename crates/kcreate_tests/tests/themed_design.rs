//! G3 — Gamma-style prompt → full themed multi-page design.
//!
//! End-to-end coverage for [`ai_generate_themed_design`]: a short
//! brief becomes a coherent, themed, multi-page document applied to
//! the open project, with a title card plus content cards (deck) or a
//! single structured page (one-pager).
//!
//! These exercises drive the *deterministic* planner (the local LLM
//! sidecar is never started in the test process, so `usedLlm` is
//! always `false`) — the feature must produce a real, populated
//! design with no model loaded. The bridge entry points operate on
//! the process-global workspace singleton, so the suite serialises
//! against the other bridge tests with `serial_test`.

use kcreate_bridge::document::{
    canvas_create_rect, document_get_tree, project_close, project_create, project_save,
};
use kcreate_bridge::phase10::{
    ai_generate_themed_design, export_pdf_multi, ThemedDesignApplyResult,
};
use serial_test::serial;
use tempfile::TempDir;

fn open_project(name: &str) -> TempDir {
    project_close();
    let dir = TempDir::new().expect("tmpdir");
    let info = project_create(name, dir.path()).expect("project_create");
    assert_eq!(info.name, name);
    project_save().expect("project_save");
    dir
}

/// Every `TextLayer` node carries a `"text"` metadata payload whose
/// `text` field is the rendered string. Collect them so tests can
/// assert recognizable, on-topic content actually reached the
/// document (never blank rectangles).
fn text_runs() -> Vec<String> {
    document_get_tree()
        .expect("tree")
        .into_iter()
        .filter(|n| n.node_type == "TextLayer")
        .filter_map(|n| {
            n.metadata
                .get("text")
                .and_then(|v| v.get("text"))
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .collect()
}

fn count_node_type(kind: &str) -> usize {
    document_get_tree()
        .expect("tree")
        .into_iter()
        .filter(|n| n.node_type == kind)
        .count()
}

/// Count root pages the generator stamped as its own output. A re-run
/// replaces these in place, so this must stay at exactly `1` no matter
/// how many times the generator runs against the same project.
fn count_generated_pages() -> usize {
    document_get_tree()
        .expect("tree")
        .into_iter()
        .filter(|n| n.node_type == "Page")
        .filter(|n| {
            n.metadata
                .get("kcreate:themedGenerated")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
        })
        .count()
}

#[test]
#[serial]
fn deck_brief_produces_title_plus_content_slides() {
    let _dir = open_project("themed-deck-happy");
    let result: ThemedDesignApplyResult = ai_generate_themed_design(
        "Pitch deck for an indie coffee roaster",
        r#"{"format":"deck","themeId":"ember","useLlm":false}"#,
    )
    .expect("ai_generate_themed_design");

    // A deck is a title card + several content cards.
    assert!(
        result.slide_count >= 4,
        "a deck should have a title card plus several content cards, got {}",
        result.slide_count
    );
    assert_eq!(
        result.artboard_ids.len() as u32,
        result.slide_count,
        "one artboard id per slide"
    );
    assert!(!result.page_id.is_empty(), "container page id populated");
    assert!(!result.brand_kit_id.is_empty(), "brand kit seeded");
    assert_eq!(result.format, "deck");
    assert_eq!(result.theme_id, "ember");
    assert!(
        !result.used_llm,
        "no sidecar in the test process — deterministic planner must run"
    );

    // The artboards actually landed in the document graph.
    assert_eq!(
        count_node_type("Artboard"),
        result.slide_count as usize,
        "every slide artboard must exist in the document"
    );

    // Recognizable, on-topic content — not blank rectangles.
    let runs = text_runs();
    assert!(
        runs.len() as u32 > result.slide_count,
        "each slide should carry multiple text runs (heading + body), got {} runs for {} slides",
        runs.len(),
        result.slide_count
    );
    let joined = runs.join("\n").to_lowercase();
    assert!(
        joined.contains("coffee"),
        "the brief subject must surface in the generated copy; got:\n{joined}"
    );

    project_close();
}

#[test]
#[serial]
fn one_pager_brief_produces_single_structured_page() {
    let _dir = open_project("themed-one-pager");
    let result = ai_generate_themed_design(
        "One-pager for a neighborhood bakery grand opening",
        r#"{"format":"onePager","themeId":"sunrise","onePagerSize":"a4"}"#,
    )
    .expect("ai_generate_themed_design");

    assert_eq!(result.slide_count, 1, "a one-pager is a single page");
    assert_eq!(result.artboard_ids.len(), 1);
    assert_eq!(result.format, "onePager");
    assert_eq!(result.theme_id, "sunrise");
    assert_eq!(count_node_type("Artboard"), 1);

    // A one-pager still carries a structured, multi-section body.
    let runs = text_runs();
    assert!(
        runs.len() >= 4,
        "one-pager should have a title plus several sections, got {} runs",
        runs.len()
    );
    let joined = runs.join("\n").to_lowercase();
    assert!(
        joined.contains("bakery") || joined.contains("opening"),
        "subject must surface in the one-pager copy; got:\n{joined}"
    );

    project_close();
}

#[test]
#[serial]
fn empty_options_default_to_a_deck() {
    // An empty options string must not error — it falls back to the
    // default request (deck / midnight / A4).
    let _dir = open_project("themed-default-options");
    let result = ai_generate_themed_design("Investor deck for a solar microgrid startup", "")
        .expect("apply");
    assert_eq!(result.format, "deck");
    assert_eq!(result.theme_id, "midnight");
    assert!(result.slide_count >= 4);
    project_close();
}

#[test]
#[serial]
fn empty_brief_is_rejected() {
    let _dir = open_project("themed-empty-brief");
    let err = ai_generate_themed_design("   ", "{}").expect_err("empty brief must error");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("brief") || msg.contains("InvalidArgument"),
        "error should name the offending argument: {msg}"
    );
    project_close();
}

#[test]
#[serial]
fn section_count_controls_slide_total() {
    let _dir = open_project("themed-section-count");
    let result = ai_generate_themed_design(
        "Pitch deck for an indie coffee roaster",
        r#"{"format":"deck","themeId":"forest","sectionCount":5}"#,
    )
    .expect("apply");
    // Title card + 5 content cards.
    assert_eq!(
        result.slide_count, 6,
        "section_count=5 should yield a title card plus five content cards"
    );
    project_close();
}

#[test]
#[serial]
fn rerun_upserts_brand_kit_in_place() {
    // Running the generator twice in the same project with the same
    // theme should reuse the theme brand kit rather than spawning
    // duplicates (upsert by name).
    let _dir = open_project("themed-upsert");
    let first = ai_generate_themed_design(
        "Pitch deck for an indie coffee roaster",
        r#"{"format":"deck","themeId":"slate"}"#,
    )
    .expect("first");
    let second = ai_generate_themed_design(
        "Pitch deck for an indie coffee roaster",
        r#"{"format":"deck","themeId":"slate"}"#,
    )
    .expect("second");
    assert_eq!(
        first.brand_kit_id, second.brand_kit_id,
        "theme brand kit id must be stable across re-runs (upsert by name)"
    );
    project_close();
}

#[test]
#[serial]
fn rerun_replaces_prior_generated_deck_without_accumulating() {
    // Re-running the generator (e.g. to try a different theme) must
    // replace its own previous output in place, not stack a second
    // tiled deck beside the first. Without the generator-owned stamp
    // the second run would `document_has_content_layers() == true` and
    // append, doubling the page/artboard count every time.
    let _dir = open_project("themed-rerun-replace");
    let first = ai_generate_themed_design(
        "Pitch deck for an indie coffee roaster",
        r#"{"format":"deck","themeId":"ember","sectionCount":6}"#,
    )
    .expect("first");
    assert_eq!(count_generated_pages(), 1, "first run creates one deck");
    assert_eq!(count_node_type("Artboard"), first.slide_count as usize);

    let second = ai_generate_themed_design(
        "Investor deck for a solar microgrid startup",
        r#"{"format":"deck","themeId":"midnight","sectionCount":8}"#,
    )
    .expect("second");

    assert_eq!(
        count_generated_pages(),
        1,
        "a re-run must replace the prior generated deck, not accumulate a second one"
    );
    assert_eq!(
        count_node_type("Artboard"),
        second.slide_count as usize,
        "only the latest deck's artboards should remain after a re-run"
    );
    // The replacement really swapped content: midnight + 8 sections.
    assert_eq!(second.theme_id, "midnight");
    assert_eq!(second.slide_count, 9);
    let joined = text_runs().join("\n").to_lowercase();
    assert!(
        joined.contains("solar") || joined.contains("microgrid"),
        "the new brief's subject must replace the old copy; got:\n{joined}"
    );
    project_close();
}

#[test]
#[serial]
fn regenerate_preserves_user_authored_content() {
    // The replace-on-rerun logic keys off a generator-owned stamp, so
    // it must never remove pages/layers the user authored themselves.
    // A user rectangle drawn before generating (and a re-generate
    // afterwards) must survive untouched.
    let _dir = open_project("themed-preserve-user");
    let user_rect = canvas_create_rect(None, 32.0, 32.0, 200.0, 120.0).expect("user rect");

    ai_generate_themed_design(
        "Pitch deck for an indie coffee roaster",
        r#"{"format":"deck","themeId":"forest"}"#,
    )
    .expect("first");
    assert_eq!(count_generated_pages(), 1);
    assert!(
        document_get_tree()
            .expect("tree")
            .iter()
            .any(|n| n.id == user_rect),
        "user content must survive the first generate"
    );

    ai_generate_themed_design(
        "Pitch deck for an indie coffee roaster",
        r#"{"format":"deck","themeId":"slate"}"#,
    )
    .expect("second");
    assert_eq!(
        count_generated_pages(),
        1,
        "re-generate replaces only the generated deck"
    );
    assert!(
        document_get_tree()
            .expect("tree")
            .iter()
            .any(|n| n.id == user_rect),
        "user content must survive a re-generate — only generator output is replaced"
    );
    project_close();
}

#[test]
#[serial]
fn extreme_section_count_is_clamped() {
    // `section_count` is clamped to a legible range ([3, 11] for a
    // deck) by the shared `resolved_section_count`, which both the
    // deterministic planner and the LLM-enrichment path now honour.
    // An absurd request must not produce an absurd slide total.
    let _dir = open_project("themed-clamp-high");
    let high = ai_generate_themed_design(
        "Pitch deck for an indie coffee roaster",
        r#"{"format":"deck","themeId":"ember","sectionCount":99}"#,
    )
    .expect("high");
    assert_eq!(
        high.slide_count, 12,
        "section_count=99 clamps to 11 content cards + 1 title card"
    );

    let _dir2 = open_project("themed-clamp-low");
    let low = ai_generate_themed_design(
        "Pitch deck for an indie coffee roaster",
        r#"{"format":"deck","themeId":"ember","sectionCount":0}"#,
    )
    .expect("low");
    assert_eq!(
        low.slide_count, 4,
        "section_count=0 clamps to 3 content cards + 1 title card"
    );
    project_close();
}

#[test]
#[serial]
fn generated_deck_exports_one_pdf_page_per_slide() {
    // The multi-artboard export path must emit exactly one PDF page
    // per tiled slide. This is the regression guard for the
    // origin-aware `compose_page_svg_in_frame` viewBox fix: before
    // it, every tile past the first (world x > 0) rendered blank and
    // the page count / content was wrong.
    let _dir = open_project("themed-deck-export");
    let result = ai_generate_themed_design(
        "Pitch deck for an indie coffee roaster",
        r#"{"format":"deck","themeId":"midnight","sectionCount":6}"#,
    )
    .expect("apply");

    let out = std::env::temp_dir().join(format!(
        "kcreate-themed-deck-{}-{}.pdf",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    ));
    let opts = r#"{"includeToc":false,"includeBookmarks":true,"includeHyperlinks":false,"rasterDpi":96.0}"#;
    let report = export_pdf_multi(opts, out.to_str().expect("utf8 path")).expect("export pdf");
    assert_eq!(
        report.page_count, result.slide_count,
        "one PDF page per tiled slide"
    );

    // Structurally valid PDF with the right page count.
    let doc = lopdf::Document::load(&out).expect("load PDF");
    assert_eq!(doc.get_pages().len() as u32, result.slide_count);

    std::fs::remove_file(&out).ok();
    project_close();
}
