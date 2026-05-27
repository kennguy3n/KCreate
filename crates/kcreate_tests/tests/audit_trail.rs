//! Integration tests for `kcreate_audit` — the cross-project,
//! separate-SQLite audit trail (Phase 6, Tasks 13–14).
//!
//! These exercise the full crate surface from construction to query
//! and retention purge. They intentionally use on-disk SQLite (not
//! in-memory) so we also test the filesystem codepath (directory
//! creation, re-open of an existing DB, etc.).

use kcreate_audit::{AuditEvent, AuditEventKind, AuditQuery, AuditStore, ProjectAction};
use kcreate_core::operation::Operation;
use uuid::Uuid;

/// Helper: build a minimal operation and convert it to an audit event.
fn make_op_event(
    actor: &str,
    command: &str,
    project: Option<Uuid>,
    node: Option<Uuid>,
) -> AuditEvent {
    let op = Operation::new(
        actor,
        command,
        serde_json::Value::Null,
        serde_json::Value::Null,
        node.map(|n| vec![n]).unwrap_or_default(),
    );
    AuditEvent::from_operation(project, &op)
}

#[test]
fn on_disk_store_round_trip() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("audit.sqlite");
    let mut store = AuditStore::open(&db_path).unwrap();
    assert!(db_path.exists());

    let project = Uuid::new_v4();
    let node = Uuid::new_v4();
    let event = make_op_event("user", "node_update", Some(project), Some(node));
    let event_id = event.id;
    store.record(&event).unwrap();

    let rows = store
        .query(&AuditQuery {
            project_id: Some(project),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, event_id);
    assert_eq!(rows[0].affected_nodes, vec![node]);
    drop(store);

    // Re-open the DB to verify persistence.
    let store2 = AuditStore::open(&db_path).unwrap();
    let rows2 = store2.query(&AuditQuery::default()).unwrap();
    assert_eq!(rows2.len(), 1);
    assert_eq!(rows2[0].id, event_id);
}

#[test]
fn batch_record_mixed_event_types() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("audit.sqlite");
    let mut store = AuditStore::open(&db_path).unwrap();

    let project = Uuid::new_v4();

    let op_event = make_op_event("user", "page_add", Some(project), None);
    let ai_event = AuditEvent::ai_action(
        Some(project),
        "bg_remove",
        "u2net-v1",
        "cpu",
        Some("remove background".into()),
        vec![Uuid::new_v4()],
    );
    let project_open = AuditEvent::project_action(
        Some(project),
        "user",
        ProjectAction::Open {
            path: "/tmp/poster.kstudio".into(),
        },
    );
    let project_save = AuditEvent::project_action(Some(project), "user", ProjectAction::Save);
    let export_event = AuditEvent::project_action(
        Some(project),
        "user",
        ProjectAction::Export {
            format: "pdf".into(),
            destination: "/tmp/poster.pdf".into(),
        },
    );

    store
        .record_batch(&[op_event, ai_event, project_open, project_save, export_event])
        .unwrap();
    assert_eq!(store.count().unwrap(), 5);

    // Filter by kind.
    let ops = store
        .query(&AuditQuery {
            kind: Some("operation".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(ops.len(), 1);
    assert!(matches!(
        &ops[0].kind,
        AuditEventKind::Operation(rec) if rec.command == "page_add"
    ));

    let ai = store
        .query(&AuditQuery {
            kind: Some("ai_action".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(ai.len(), 1);
    assert_eq!(ai[0].actor, "ai:u2net-v1");

    let lifecycle = store
        .query(&AuditQuery {
            kind: Some("project".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(lifecycle.len(), 3);
}

#[test]
fn query_by_affected_node() {
    let mut store = AuditStore::open_in_memory().unwrap();
    let node_a = Uuid::new_v4();
    let node_b = Uuid::new_v4();
    let e1 = make_op_event("user", "node_update", None, Some(node_a));
    let e2 = make_op_event("user", "node_delete", None, Some(node_b));
    store.record_batch(&[e1, e2]).unwrap();

    let rows = store
        .query(&AuditQuery {
            affected_node: Some(node_a),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].affected_nodes, vec![node_a]);
}

#[test]
fn query_time_range() {
    let mut store = AuditStore::open_in_memory().unwrap();
    let now = chrono::Utc::now();
    let mut old = make_op_event("user", "old", None, None);
    old.timestamp = now - chrono::Duration::hours(3);
    let mut mid = make_op_event("user", "mid", None, None);
    mid.timestamp = now - chrono::Duration::minutes(30);
    let mut recent = make_op_event("user", "recent", None, None);
    recent.timestamp = now;
    store.record_batch(&[old, mid, recent]).unwrap();

    let since = now - chrono::Duration::hours(1);
    let rows = store
        .query(&AuditQuery {
            since: Some(since),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|e| e.timestamp >= since));

    let until = now - chrono::Duration::hours(1);
    let old_only = store
        .query(&AuditQuery {
            until: Some(until),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(old_only.len(), 1);
}

#[test]
fn purge_respects_cutoff() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("audit.sqlite");
    let mut store = AuditStore::open(&db_path).unwrap();
    let now = chrono::Utc::now();

    let mut old = make_op_event("user", "old_op", None, None);
    old.timestamp = now - chrono::Duration::days(60);
    let mut recent = make_op_event("user", "recent_op", None, None);
    recent.timestamp = now;
    store.record_batch(&[old, recent]).unwrap();
    assert_eq!(store.count().unwrap(), 2);

    let cutoff = now - chrono::Duration::days(30);
    let removed = store.purge_before(cutoff).unwrap();
    assert_eq!(removed, 1);
    assert_eq!(store.count().unwrap(), 1);

    let remaining = store.query(&AuditQuery::default()).unwrap();
    assert_eq!(remaining.len(), 1);
    assert!(matches!(
        &remaining[0].kind,
        AuditEventKind::Operation(rec) if rec.command == "recent_op"
    ));
}

#[test]
fn serde_round_trip_for_all_event_kinds() {
    let events = vec![
        make_op_event("user", "node_update", Some(Uuid::new_v4()), None),
        AuditEvent::ai_action(None, "upscale", "esrgan", "gpu", Some("4x".into()), vec![]),
        AuditEvent::project_action(None, "user", ProjectAction::Close),
    ];

    for event in &events {
        let json = serde_json::to_string(&event.kind).unwrap();
        let back: AuditEventKind = serde_json::from_str(&json).unwrap();
        assert_eq!(back, event.kind);
    }
}

#[test]
fn default_path_is_under_kcreate_audit() {
    let path = AuditStore::default_path();
    let path_str = path.to_string_lossy();
    assert!(
        path_str.contains(".kcreate")
            && path_str.contains("audit")
            && path_str.ends_with("audit.sqlite"),
        "default path should be ~/.kcreate/audit/audit.sqlite, got: {path_str}",
    );
}

#[test]
fn query_limit_enforced() {
    let mut store = AuditStore::open_in_memory().unwrap();
    for _ in 0..10 {
        store
            .record(&make_op_event("user", "x", None, None))
            .unwrap();
    }
    let limited = store
        .query(&AuditQuery {
            limit: Some(3),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(limited.len(), 3);

    let err = store.query(&AuditQuery {
        limit: Some(AuditStore::MAX_QUERY_LIMIT + 1),
        ..Default::default()
    });
    assert!(err.is_err());
}
