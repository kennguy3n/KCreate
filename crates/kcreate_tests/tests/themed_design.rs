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
    canvas_create_rect, document_get_tree, document_redo, document_undo, project_close,
    project_create, project_save,
};
use kcreate_bridge::phase10::{
    ai_generate_themed_design, ai_refine_themed_design, export_pdf_multi, ThemedDesignApplyResult,
};
use kcreate_bridge::thumbnails::ensure_page_thumbnail;
use serial_test::serial;
use std::collections::HashSet;
use std::path::PathBuf;
use tempfile::TempDir;
use uuid::Uuid;

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

/// Lower-cased concatenation of every text run, for subject-surfaces
/// assertions.
fn joined_text() -> String {
    text_runs().join("\n").to_lowercase()
}

/// Whether a node with the given (string) id is currently in the tree.
fn tree_contains(id_str: &str) -> bool {
    let id: Uuid = id_str.parse().expect("node id is a uuid");
    document_get_tree()
        .expect("tree")
        .iter()
        .any(|n| n.id == id)
}

/// Standard (not URL-safe) base64 decode — matches the encoding of
/// [`kcreate_bridge::thumbnails::ThumbnailBytes::bytes_base64`].
fn base64_decode(s: &str) -> Vec<u8> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .expect("decode thumbnail base64")
}

/// Count distinct RGBA colours in a PNG, capped at `cap`. A blank or
/// flat frame collapses to one colour; a real multi-element themed
/// design renders many, so a high count proves the page actually drew.
fn distinct_colors(png: &[u8], cap: usize) -> usize {
    let img = image::load_from_memory(png).expect("decode proof PNG");
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

/// Directory where proof PNGs are written. Honours `KCREATE_PROOF_DIR`
/// (so a harness can collect them for attachment) and otherwise falls
/// back to the git-ignored workspace `target/` dir.
fn proof_dir() -> PathBuf {
    let dir = std::env::var_os("KCREATE_PROOF_DIR").map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/h4_ai_proof"),
        PathBuf::from,
    );
    std::fs::create_dir_all(&dir).expect("create proof dir");
    dir
}

/// Render a generated page to PNG through the real document → scene →
/// encoder path, write it to the proof dir, and return the number of
/// distinct colours (a non-blank-ness proxy). `page_id` is the
/// container page id returned by the generator.
fn render_page_proof(page_id: &str, max_dim_px: u32, file: &str) -> usize {
    let id: Uuid = page_id.parse().expect("page id is a uuid");
    let thumb = ensure_page_thumbnail(id, max_dim_px).expect("render page thumbnail");
    let png = base64_decode(&thumb.bytes_base64);
    assert!(
        png.starts_with(b"\x89PNG\r\n\x1a\n"),
        "{file}: rendered bytes are not a PNG"
    );
    let colors = distinct_colors(&png, 256);
    std::fs::write(proof_dir().join(file), &png).expect("write proof png");
    colors
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

// ---------------------------------------------------------------------------
// H4 — additional output formats
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn social_post_set_produces_square_plus_story() {
    // A social-post brief yields a *set*: a 1:1 feed tile plus a 9:16
    // story, each a distinct artboard. No image-gen sidecar runs in
    // the test process, so the hero band degrades to a vector
    // placeholder and `used_image` is honestly `false`.
    let _dir = open_project("themed-social");
    let result = ai_generate_themed_design(
        "Launch announcement for a sustainable sneaker brand",
        r#"{"format":"socialPost","themeId":"sunrise","useImage":true}"#,
    )
    .expect("ai_generate_themed_design");

    assert_eq!(result.format, "socialPost");
    assert_eq!(result.theme_id, "sunrise");
    assert_eq!(
        result.slide_count, 2,
        "a social-post set is a square feed tile + a 9:16 story"
    );
    assert_eq!(result.artboard_ids.len(), 2);
    assert_eq!(count_node_type("Artboard"), 2);
    assert!(
        !result.used_llm,
        "deterministic planner must run without a sidecar"
    );
    assert!(
        !result.used_image,
        "no image-gen model in the test process — imagery must degrade to a placeholder"
    );
    // The placeholder hero is a vector layer, never an (unresolved)
    // raster — so no raster blob is dangling.
    assert_eq!(
        count_node_type("RasterLayer"),
        0,
        "offline hero must be a vector/gradient placeholder, not a raster"
    );
    assert!(
        count_node_type("VectorLayer") >= 2,
        "each post should carry a hero placeholder plus accents"
    );

    let joined = joined_text();
    assert!(
        joined.contains("sneaker") || joined.contains("sustainable"),
        "the brief subject must surface in the copy; got:\n{joined}"
    );
    project_close();
}

#[test]
#[serial]
fn web_page_produces_single_tall_scroll_with_sections() {
    // A web-page brief yields a single tall artboard (a hero + feature
    // sections + CTA), not a tiled multi-slide deck.
    let _dir = open_project("themed-web");
    let result = ai_generate_themed_design(
        "Landing page for a privacy-first password manager",
        r#"{"format":"webPage","themeId":"slate","sectionCount":4,"useImage":true}"#,
    )
    .expect("ai_generate_themed_design");

    assert_eq!(result.format, "webPage");
    assert_eq!(result.theme_id, "slate");
    assert_eq!(
        result.slide_count, 1,
        "a web page is one continuous scroll, not a tiled deck"
    );
    assert_eq!(count_node_type("Artboard"), 1);
    assert!(!result.used_image, "imagery degrades offline");
    assert_eq!(count_node_type("RasterLayer"), 0);

    let runs = text_runs();
    assert!(
        runs.len() >= 6,
        "a hero + feature sections + CTA should produce many text runs, got {}",
        runs.len()
    );
    let joined = joined_text();
    assert!(
        joined.contains("password") || joined.contains("privacy"),
        "the brief subject must surface in the copy; got:\n{joined}"
    );
    project_close();
}

#[test]
#[serial]
fn document_format_produces_cover_plus_paginated_body() {
    // A document/report brief yields a cover page plus one or more
    // paginated body pages — always at least two artboards.
    let _dir = open_project("themed-doc");
    let result = ai_generate_themed_design(
        "Quarterly impact report for an urban farming nonprofit",
        r#"{"format":"document","themeId":"forest","sectionCount":6,"useImage":true}"#,
    )
    .expect("ai_generate_themed_design");

    assert_eq!(result.format, "document");
    assert_eq!(result.theme_id, "forest");
    assert!(
        result.slide_count >= 2,
        "a report is a cover plus paginated body, got {} page(s)",
        result.slide_count
    );
    assert_eq!(count_node_type("Artboard"), result.slide_count as usize);
    assert!(!result.used_image, "imagery degrades offline");
    assert_eq!(count_node_type("RasterLayer"), 0);

    let joined = joined_text();
    assert!(
        joined.contains("farming") || joined.contains("impact") || joined.contains("report"),
        "the brief subject must surface in the copy; got:\n{joined}"
    );
    project_close();
}

// ---------------------------------------------------------------------------
// H4 — refine-with-AI loop
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn refine_rejects_when_no_design_has_been_generated() {
    // Refine operates on an already-generated design. With nothing
    // generated yet it must fail with a clear, actionable message
    // rather than silently no-op or panic.
    let _dir = open_project("themed-refine-empty");
    let err = ai_refine_themed_design("make it more minimal")
        .expect_err("refine without a design errors");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("no AI-generated design") || msg.contains("generate one first"),
        "refine should explain that a design must exist first; got: {msg}"
    );
    project_close();
}

#[test]
#[serial]
fn refine_rewrites_generated_design_in_place_and_round_trips() {
    // A refine reloads the originating spec, applies a deterministic
    // structural directive, and replaces the generated design in place
    // (never stacking a second one). Repeated refines accumulate off
    // the prior resolved section count, so the loop is stable.
    let _dir = open_project("themed-refine");
    let base = ai_generate_themed_design(
        "Pitch deck for an indie coffee roaster",
        r#"{"format":"deck","themeId":"ember","sectionCount":6}"#,
    )
    .expect("base generate");
    assert_eq!(base.slide_count, 7, "6 content cards + 1 title card");
    assert_eq!(count_generated_pages(), 1);

    // "more minimal" → Minimal directive → one fewer section.
    let minimal =
        ai_refine_themed_design("make it more minimal and punchy").expect("refine minimal");
    assert_eq!(minimal.format, "deck", "refine preserves the format");
    assert_eq!(minimal.theme_id, "ember", "refine preserves the theme");
    assert_eq!(
        count_generated_pages(),
        1,
        "refine replaces the design in place, never stacks a second"
    );
    assert_eq!(
        minimal.slide_count, 6,
        "a 'more minimal' refine drops one content card (5 + title)"
    );
    let joined = joined_text();
    assert!(
        joined.contains("coffee"),
        "refine keeps the original subject; got:\n{joined}"
    );

    // "add a pricing slide" → MoreContent → one more section, off the
    // count the minimal pass resolved to (5 → 6).
    let expanded = ai_refine_themed_design("add a pricing slide").expect("refine expand");
    assert_eq!(
        count_generated_pages(),
        1,
        "still exactly one generated design after a second refine"
    );
    assert_eq!(
        expanded.slide_count, 7,
        "a second refine accumulates off the prior resolved count (6 + title)"
    );
    project_close();
}

#[test]
#[serial]
fn generated_design_is_a_single_undoable_operation() {
    // Generating (and refining) a whole design must record exactly one
    // reversible operation: one Ctrl+Z removes the entire generated
    // page, and Ctrl+Y (redo) restores it verbatim.
    let _dir = open_project("themed-undo");
    let result = ai_generate_themed_design(
        "Pitch deck for an indie coffee roaster",
        r#"{"format":"deck","themeId":"midnight","sectionCount":5}"#,
    )
    .expect("generate");
    assert_eq!(count_generated_pages(), 1);
    let artboards_after_gen = count_node_type("Artboard");
    assert_eq!(artboards_after_gen, result.slide_count as usize);

    let undo = document_undo()
        .expect("undo ok")
        .expect("an operation to undo");
    assert_eq!(undo.command, "ai_generate_themed_design");
    assert_eq!(
        count_generated_pages(),
        0,
        "a single undo must remove the whole generated design"
    );
    assert!(
        result.artboard_ids.iter().all(|id| !tree_contains(id)),
        "every generated slide artboard is gone after one undo"
    );

    let redo = document_redo()
        .expect("redo ok")
        .expect("an operation to redo");
    assert_eq!(redo.command, "ai_generate_themed_design");
    assert_eq!(
        count_generated_pages(),
        1,
        "redo restores the generated design verbatim"
    );
    assert_eq!(
        count_node_type("Artboard"),
        artboards_after_gen,
        "redo restores every slide artboard"
    );
    assert!(
        result.artboard_ids.iter().all(|id| tree_contains(id)),
        "redo restores every generated slide artboard by its original id"
    );
    project_close();
}

// ---------------------------------------------------------------------------
// H4 — real-design render proof (PNG)
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn h4_formats_render_to_recognizable_png_proofs() {
    // The strongest proof for a *visual* tool: drive each format
    // end-to-end, render the generated page through the real
    // document → scene → PNG encoder, and assert the pixels form a
    // rich, multi-colour design (never a blank/flat frame). The PNGs
    // are written to the proof dir for visual inspection / attachment.
    let _dir = open_project("themed-proof");

    // Deck (existing format — must not regress).
    let deck = ai_generate_themed_design(
        "Pitch deck for an indie coffee roaster",
        r#"{"format":"deck","themeId":"ember","sectionCount":5}"#,
    )
    .expect("deck");
    let deck_colors = render_page_proof(&deck.page_id, 2200, "h4_deck_ember.png");
    assert!(
        deck_colors >= 16,
        "deck render looks blank ({deck_colors} distinct colours)"
    );

    // Social post set (square + story).
    let social = ai_generate_themed_design(
        "Launch announcement for a sustainable sneaker brand",
        r#"{"format":"socialPost","themeId":"sunrise","useImage":true}"#,
    )
    .expect("social");
    let social_colors = render_page_proof(&social.page_id, 1600, "h4_social_sunrise.png");
    assert!(
        social_colors >= 16,
        "social render looks blank ({social_colors} distinct colours)"
    );

    // Web page (single tall scroll).
    let web = ai_generate_themed_design(
        "Landing page for a privacy-first password manager",
        r#"{"format":"webPage","themeId":"slate","sectionCount":4,"useImage":true}"#,
    )
    .expect("web");
    let web_colors = render_page_proof(&web.page_id, 1600, "h4_web_slate.png");
    assert!(
        web_colors >= 16,
        "web render looks blank ({web_colors} distinct colours)"
    );

    // Document / report (cover + body).
    let doc = ai_generate_themed_design(
        "Quarterly impact report for an urban farming nonprofit",
        r#"{"format":"document","themeId":"forest","sectionCount":6,"useImage":true}"#,
    )
    .expect("doc");
    let doc_colors = render_page_proof(&doc.page_id, 1800, "h4_document_forest.png");
    assert!(
        doc_colors >= 16,
        "document render looks blank ({doc_colors} distinct colours)"
    );

    // Refine before/after proof: regenerate a deck, snapshot, then
    // refine and snapshot again so the change is visually verifiable.
    let before = ai_generate_themed_design(
        "Sales deck for a boutique cycling studio",
        r#"{"format":"deck","themeId":"midnight","sectionCount":6}"#,
    )
    .expect("refine-before");
    render_page_proof(&before.page_id, 2200, "h4_refine_before.png");
    let after = ai_refine_themed_design("make it more minimal").expect("refine");
    render_page_proof(&after.page_id, 2200, "h4_refine_after.png");
    assert!(
        after.slide_count < before.slide_count,
        "the 'more minimal' refine should visibly drop a slide ({} → {})",
        before.slide_count,
        after.slide_count
    );

    project_close();
}
