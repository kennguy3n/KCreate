//! Phase 9 Block E — performance, security, robustness.
//!
//! Covers:
//! * Task 25: memory-pressure watchdog (event drain + simulated emit).
//! * Task 26: project auto-save with crash-recovery marker round-trip.
//! * Task 27: export validation report shape + warnings.
//!
//! Bridge state is process-global; `serial_test` is therefore
//! mandatory.

use kcreate_bridge::autosave::{
    autosave_dismiss_recovery, autosave_force_now, autosave_recover, autosave_recovery_available,
    autosave_start, autosave_status, autosave_stop,
};
use kcreate_bridge::document::{artboard_create, project_close, project_create, project_save};
use kcreate_bridge::perf::{
    drain_memory_events, memory_pressure_emit_for_test, MemoryPressureEvent,
};
use kcreate_bridge::phase9::export_validate;
use kcreate_export::validate::{ExportSeverity, ExportValidationRequest};
use serial_test::serial;
use tempfile::TempDir;

fn open_project(name: &str) -> TempDir {
    project_close();
    let dir = TempDir::new().expect("tmpdir");
    project_create(name, dir.path()).expect("project_create");
    project_save().expect("project_save");
    dir
}

// ---------------------------------------------------------------------------
// Task 25: memory pressure
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn memory_event_queue_drains_in_fifo_order() {
    // Clear any leftovers.
    let _ = drain_memory_events();
    memory_pressure_emit_for_test(MemoryPressureEvent::Entered {
        available_mb: 100,
        threshold_mb: 500,
    });
    memory_pressure_emit_for_test(MemoryPressureEvent::Released {
        available_mb: 800,
        threshold_mb: 500,
    });
    let events = drain_memory_events();
    assert_eq!(events.len(), 2);
    if let MemoryPressureEvent::Entered {
        available_mb,
        threshold_mb,
    } = events[0]
    {
        assert_eq!(available_mb, 100);
        assert_eq!(threshold_mb, 500);
    } else {
        panic!("expected Entered first, got {:?}", events[0]);
    }
    if let MemoryPressureEvent::Released {
        available_mb,
        threshold_mb,
    } = events[1]
    {
        assert_eq!(available_mb, 800);
        assert_eq!(threshold_mb, 500);
    } else {
        panic!("expected Released second, got {:?}", events[1]);
    }
    // Drain again — second call must be empty (events are consumed).
    assert!(drain_memory_events().is_empty(), "queue must be drained");
}

#[test]
#[serial]
fn memory_event_queue_caps_at_32_entries() {
    let _ = drain_memory_events();
    for i in 0..50u64 {
        memory_pressure_emit_for_test(MemoryPressureEvent::Entered {
            available_mb: i,
            threshold_mb: 500,
        });
    }
    let events = drain_memory_events();
    assert!(
        events.len() <= 32,
        "queue must be capped to 32, got {}",
        events.len()
    );
    // Oldest entries should have been popped — the *last* event we
    // pushed (49) must still be in the queue.
    let has_last = events.iter().any(|e| {
        matches!(
            e,
            MemoryPressureEvent::Entered {
                available_mb: 49,
                ..
            }
        )
    });
    assert!(has_last, "most recent event must survive the cap");
}

// ---------------------------------------------------------------------------
// Task 26: autosave + recovery
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn autosave_start_is_idempotent() {
    let _dir = open_project("autosave-idempotent");
    let first = autosave_start();
    let second = autosave_start();
    assert!(
        first || second,
        "first autosave_start should report true, got {first} / {second}",
    );
    assert!(!(first && second), "second autosave_start must be a no-op");
    autosave_stop();
    project_close();
}

#[test]
#[serial]
fn autosave_force_now_persists_when_modified() {
    let _dir = open_project("autosave-tick");
    // Trigger a document mutation so `modified_at` advances and the
    // tick has something to save.
    let _ab = artboard_create(None, "Hero".to_string(), 1080.0, 1080.0).expect("artboard");
    let saved = autosave_force_now().expect("autosave_force_now");
    assert!(saved, "tick must report a successful save");

    let status = autosave_status();
    assert!(
        status.last_success_at.is_some(),
        "status must record the success timestamp"
    );
    assert!(status.counter >= 1, "counter must increment");
    project_close();
}

#[test]
#[serial]
fn autosave_recovery_marker_round_trip() {
    let _dir = open_project("autosave-recover");
    // Mutate + tick so a marker is written.
    artboard_create(None, "Hero".to_string(), 800.0, 600.0).expect("artboard");
    let saved = autosave_force_now().expect("force");
    assert!(saved);

    // Right after a tick the in-memory + marker timestamps are equal,
    // so recovery is NOT available.
    let available = autosave_recovery_available().expect("query recovery");
    assert!(
        available.is_none(),
        "no recovery should be advertised when marker equals state"
    );

    // Recover (= accept current state). Marker is rewritten.
    autosave_recover().expect("recover");

    // Dismiss removes the marker entirely.
    autosave_dismiss_recovery().expect("dismiss");
    let after = autosave_recovery_available().expect("query post-dismiss");
    assert!(after.is_none(), "dismiss must leave no marker");
    project_close();
}

// ---------------------------------------------------------------------------
// Task 27: export validation
// ---------------------------------------------------------------------------

fn base_req() -> ExportValidationRequest {
    ExportValidationRequest {
        node_ids: vec!["00000000-0000-0000-0000-000000000001".to_string()],
        format: "png".to_string(),
        width: Some(1024),
        height: Some(1024),
        jpeg_quality: None,
        transparent: false,
        force_oversized: false,
        has_text: false,
        missing_fonts: false,
    }
}

#[test]
fn export_validate_happy_path_is_ok() {
    let report = export_validate(base_req());
    assert!(
        report.ok,
        "vanilla 1024x1024 PNG must validate: {:?}",
        report.issues
    );
    assert!(report.issues.is_empty(), "should have no issues");
}

#[test]
fn export_validate_rejects_zero_dimensions() {
    let mut req = base_req();
    req.width = Some(0);
    let report = export_validate(req);
    assert!(!report.ok, "zero width must fail validation");
    assert!(report
        .issues
        .iter()
        .any(|i| { i.severity == ExportSeverity::Error && i.code == "ZERO_WIDTH" }));
}

#[test]
fn export_validate_rejects_unknown_format() {
    let mut req = base_req();
    req.format = "tiff".to_string();
    let report = export_validate(req);
    assert!(!report.ok, "unknown format must fail validation");
    assert!(report
        .issues
        .iter()
        .any(|i| i.code == "UNKNOWN_FORMAT" && i.severity == ExportSeverity::Error));
}

#[test]
fn export_validate_warns_on_oversized_without_force() {
    let mut req = base_req();
    req.width = Some(20_000);
    let report = export_validate(req);
    // Oversized is a warning, not an error — `ok` stays true.
    assert!(
        report
            .issues
            .iter()
            .any(|i| i.code == "OVERSIZED_WIDTH" && i.severity == ExportSeverity::Warning),
        "20k px should produce an oversized warning"
    );
}

#[test]
fn export_validate_suppresses_oversized_when_forced() {
    let mut req = base_req();
    req.width = Some(20_000);
    req.force_oversized = true;
    let report = export_validate(req);
    assert!(
        !report.issues.iter().any(|i| i.code == "OVERSIZED_WIDTH"),
        "force_oversized must suppress the warning"
    );
}

#[test]
fn export_validate_requires_at_least_one_node() {
    let mut req = base_req();
    req.node_ids.clear();
    let report = export_validate(req);
    assert!(!report.ok);
    assert!(report.issues.iter().any(|i| i.code == "NO_NODES"));
}
