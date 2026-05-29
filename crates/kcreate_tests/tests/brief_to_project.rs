//! Phase 9 Block B Task 7 — `brief_to_project` integration test.
//!
//! Exercises the full Rust-side orchestrator that takes a structured
//! LLM brief and produces an artboard, brand kit, and starter layers
//! inside the currently open project.
//!
//! The bridge entry point is process-global (it operates on the
//! workspace singleton), so we serialize against the other bridge
//! tests with `serial_test`.

use kcreate_bridge::document::{project_close, project_create, project_save};
use kcreate_bridge::phase9::{brief_to_project, BriefPlan, BriefStarterLayer};
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

fn standard_plan() -> BriefPlan {
    BriefPlan {
        artboard_preset: "Instagram Post".to_string(),
        palette: vec![
            "#1f4e79".to_string(),
            "#ff7f50".to_string(),
            "#264653".to_string(),
        ],
        starter_layers: vec![
            BriefStarterLayer {
                name: "Hero headline".to_string(),
                kind: "text".to_string(),
                suggested_content: Some("Grand opening this Saturday".to_string()),
            },
            BriefStarterLayer {
                name: "Background frame".to_string(),
                kind: "shape".to_string(),
                suggested_content: None,
            },
        ],
    }
}

#[test]
#[serial]
fn brief_to_project_creates_artboard_brand_kit_and_layers() {
    let _dir = open_project("brief-happy-path");
    let plan = standard_plan();
    let result = brief_to_project(&plan).expect("brief_to_project");
    assert!(
        !result.artboard_id.is_empty(),
        "artboard id must be populated"
    );
    assert!(
        !result.brand_kit_id.is_empty(),
        "brand kit id must be populated"
    );
    assert_eq!(
        result.layer_ids.len(),
        plan.starter_layers.len(),
        "one layer id per starter layer"
    );
    // Layers are unique ids.
    let mut seen = result.layer_ids.clone();
    seen.sort();
    seen.dedup();
    assert_eq!(seen.len(), result.layer_ids.len(), "no duplicate layer ids");
    project_close();
}

#[test]
#[serial]
fn brief_to_project_accepts_camel_case_preset_name() {
    // The LLM is prompted with display names like "Instagram Post"
    // but is not guaranteed to echo them verbatim — older revisions
    // of `BriefModal` even told it to produce camelCase tokens.
    // `brief_to_project` must tolerate reasonable variations so a
    // single drift between the prompt and the Rust matcher doesn't
    // break the entire brief→project flow end-to-end.
    let _dir = open_project("brief-camelcase-preset");
    let mut plan = standard_plan();
    plan.artboard_preset = "instagramPost".into();
    let result = brief_to_project(&plan).expect("camelCase preset must resolve");
    assert!(!result.artboard_id.is_empty());
    project_close();
}

#[test]
#[serial]
fn brief_to_project_accepts_kebab_case_preset_name() {
    let _dir = open_project("brief-kebab-preset");
    let mut plan = standard_plan();
    plan.artboard_preset = "instagram-post".into();
    let result = brief_to_project(&plan).expect("kebab preset must resolve");
    assert!(!result.artboard_id.is_empty());
    project_close();
}

#[test]
#[serial]
fn brief_to_project_rejects_unknown_preset() {
    let _dir = open_project("brief-bad-preset");
    let mut plan = standard_plan();
    plan.artboard_preset = "Definitely Not A Real Preset".into();
    let err = brief_to_project(&plan).expect_err("unknown preset must error");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("artboard_preset") || msg.contains("InvalidArgument"),
        "error should name the offending argument: {msg}"
    );
    project_close();
}

#[test]
#[serial]
fn brief_to_project_rejects_empty_palette() {
    let _dir = open_project("brief-empty-palette");
    let mut plan = standard_plan();
    plan.palette.clear();
    let err = brief_to_project(&plan).expect_err("empty palette must error");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("palette") || msg.contains("InvalidArgument"),
        "error should name the offending argument: {msg}"
    );
    project_close();
}

#[test]
#[serial]
fn brief_to_project_rejects_bad_layer_kind() {
    let _dir = open_project("brief-bad-kind");
    let mut plan = standard_plan();
    plan.starter_layers.push(BriefStarterLayer {
        name: "Mystery layer".into(),
        kind: "definitely_not_a_kind".into(),
        suggested_content: None,
    });
    let err = brief_to_project(&plan).expect_err("bad kind must error");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("kind") || msg.contains("InvalidArgument"),
        "error should mention the bad kind: {msg}"
    );
    project_close();
}

#[test]
#[serial]
fn brief_to_project_reapplies_brand_kit_in_place() {
    // Running the brief twice in the same project should upsert the
    // same brand kit by name rather than spawning duplicates.
    let _dir = open_project("brief-upsert");
    let plan = standard_plan();
    let first = brief_to_project(&plan).expect("first brief");
    let second = brief_to_project(&plan).expect("second brief");
    assert_eq!(
        first.brand_kit_id, second.brand_kit_id,
        "brand kit id must be stable across re-runs (upsert by name)"
    );
    project_close();
}
