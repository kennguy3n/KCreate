//! Phase 8 Block C: job-first export presets.
//!
//! Verifies that every [`JobType`] returns a non-empty preset
//! list and that each preset has a valid format.

use kcreate_export::job_presets::{every_job, presets_for_job};

#[test]
fn every_job_type_returns_non_empty_presets() {
    for job in every_job() {
        let result = presets_for_job(job);
        assert!(
            !result.presets.is_empty(),
            "job type {job:?} returned empty presets"
        );
    }
}

#[test]
fn all_presets_have_valid_format_and_scale() {
    for job in every_job() {
        let result = presets_for_job(job);
        for preset in &result.presets {
            assert!(
                preset.scale > 0.0,
                "preset '{}' for job {job:?} has non-positive scale",
                preset.name,
            );
            assert!(
                !preset.name.is_empty(),
                "preset has empty name for job {job:?}"
            );
        }
    }
}

#[test]
fn app_ui_presets_include_density_buckets() {
    use kcreate_export::job_presets::JobType;
    let presets = presets_for_job(JobType::AppOrWebsiteUi);
    let names: Vec<&str> = presets.presets.iter().map(|p| p.name.as_str()).collect();
    assert!(names.contains(&"PNG @1x"));
    assert!(names.contains(&"PNG @2x"));
    assert!(names.contains(&"PNG @3x"));
}
