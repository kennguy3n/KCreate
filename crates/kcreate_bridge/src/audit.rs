//! Bridge layer for the audit log (`kcreate_audit`).
//!
//! The audit store is a process-global singleton backed by SQLite,
//! separate from the project DB. It opens lazily on first access and
//! survives project open/close cycles — every project the user touches
//! in a session shares the same audit timeline.
//!
//! All public functions in this module are consumed by the N-API
//! wrappers in `lib.rs` and by other bridge modules (`document.rs`,
//! `phase2.rs`) that record events as a side-effect of user actions.

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use kcreate_audit::{AuditEvent, AuditQuery, AuditStore, AuditStoreError};
use serde::Serialize;
use uuid::Uuid;

use crate::document::DocumentBridgeError;

type Result<T> = std::result::Result<T, DocumentBridgeError>;

// ---------------------------------------------------------------------------
// Singleton
// ---------------------------------------------------------------------------

fn audit_store() -> &'static Mutex<AuditStore> {
    static STORE: OnceLock<Mutex<AuditStore>> = OnceLock::new();
    STORE.get_or_init(|| {
        let path = audit_db_path();
        let store = AuditStore::open(&path).unwrap_or_else(|e| {
            log::warn!(
                "audit store open failed at {}: {e} — falling back to in-memory",
                path.display()
            );
            AuditStore::open_in_memory().expect("in-memory audit store must not fail")
        });
        Mutex::new(store)
    })
}

fn audit_db_path() -> PathBuf {
    std::env::var("KCREATE_AUDIT_DB").map_or_else(|_| AuditStore::default_path(), PathBuf::from)
}

/// Reset the singleton for tests — each test gets a fresh in-memory
/// store so test isolation is guaranteed.
#[cfg(test)]
pub(crate) fn reset_audit_for_tests() {
    let mut guard = audit_store().lock().expect("audit lock poisoned");
    *guard = AuditStore::open_in_memory().expect("in-memory audit store must not fail");
}

// ---------------------------------------------------------------------------
// Public API — consumed by lib.rs N-API wrappers
// ---------------------------------------------------------------------------

/// Record a single audit event. Returns the event's UUID as a string.
pub fn audit_record(event: &AuditEvent) -> Result<Uuid> {
    let mut store = audit_store().lock().expect("audit lock poisoned");
    store.record(event).map_err(audit_err)
}

/// Query audit events. Returns the matching rows as a serialised
/// JSON array for the N-API layer to forward to the renderer.
pub fn audit_query(filter: &AuditQuery) -> Result<AuditQueryReport> {
    let store = audit_store().lock().expect("audit lock poisoned");
    let events = store.query(filter).map_err(audit_err)?;
    let total = store.count().map_err(audit_err)?;
    Ok(AuditQueryReport { events, total })
}

/// Total number of rows in the audit log.
pub fn audit_count() -> Result<u64> {
    let store = audit_store().lock().expect("audit lock poisoned");
    store.count().map_err(audit_err)
}

/// Delete rows strictly older than `cutoff`. Returns the number of
/// rows removed.
pub fn audit_purge_before(cutoff: kcreate_audit::Timestamp) -> Result<u64> {
    let mut store = audit_store().lock().expect("audit lock poisoned");
    store.purge_before(cutoff).map_err(audit_err)
}

/// Filesystem path of the currently open audit DB.
pub fn audit_path() -> String {
    let store = audit_store().lock().expect("audit lock poisoned");
    store.path().display().to_string()
}

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

/// Report returned by [`audit_query`] — events + total count so the
/// renderer's AuditPanel can show "showing N of TOTAL".
#[derive(Debug, Clone, Serialize)]
pub struct AuditQueryReport {
    pub events: Vec<AuditEvent>,
    pub total: u64,
}

// ---------------------------------------------------------------------------
// Error mapping
// ---------------------------------------------------------------------------

fn audit_err(e: AuditStoreError) -> DocumentBridgeError {
    DocumentBridgeError::Internal(e.to_string())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use kcreate_audit::{AuditEvent, AuditEventKind, AuditQuery, ProjectAction};
    use serial_test::serial;

    #[test]
    #[serial]
    fn record_and_query_round_trip() {
        reset_audit_for_tests();
        let project = Uuid::new_v4();
        let op = kcreate_core::operation::Operation::new(
            "user",
            "node_update",
            serde_json::Value::Null,
            serde_json::Value::Null,
            vec![Uuid::new_v4()],
        );
        let event = AuditEvent::from_operation(Some(project), &op);
        let id = audit_record(&event).unwrap();
        assert_eq!(id, event.id);

        let report = audit_query(&AuditQuery::default()).unwrap();
        assert_eq!(report.events.len(), 1);
        assert_eq!(report.total, 1);
        assert_eq!(report.events[0].id, id);
        match &report.events[0].kind {
            AuditEventKind::Operation(rec) => {
                assert_eq!(rec.command, "node_update");
            }
            _ => panic!("expected Operation kind"),
        }
    }

    #[test]
    #[serial]
    fn ai_action_recorded_and_queryable() {
        reset_audit_for_tests();
        let event = AuditEvent::ai_action(
            None,
            "upscale",
            "esrgan-v1",
            "cpu",
            None,
            vec![Uuid::new_v4()],
        );
        audit_record(&event).unwrap();
        let report = audit_query(&AuditQuery {
            kind: Some("ai_action".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(report.events.len(), 1);
        assert_eq!(report.events[0].kind_tag(), "ai_action");
    }

    #[test]
    #[serial]
    fn project_lifecycle_events() {
        reset_audit_for_tests();
        let project = Uuid::new_v4();
        let open_event = AuditEvent::project_action(
            Some(project),
            "user",
            ProjectAction::Open {
                path: "/tmp/test.kstudio".into(),
            },
        );
        let save_event = AuditEvent::project_action(Some(project), "user", ProjectAction::Save);
        audit_record(&open_event).unwrap();
        audit_record(&save_event).unwrap();

        let report = audit_query(&AuditQuery {
            kind: Some("project".into()),
            project_id: Some(project),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(report.events.len(), 2);
        assert_eq!(report.total, 2);
    }

    #[test]
    #[serial]
    fn count_reflects_total_rows() {
        reset_audit_for_tests();
        assert_eq!(audit_count().unwrap(), 0);
        for i in 0..5 {
            let event =
                AuditEvent::ai_action(None, format!("action_{i}"), "model", "cpu", None, vec![]);
            audit_record(&event).unwrap();
        }
        assert_eq!(audit_count().unwrap(), 5);
    }

    #[test]
    #[serial]
    fn purge_removes_old_rows() {
        reset_audit_for_tests();
        let now = chrono::Utc::now();
        let mut old = AuditEvent::project_action(None, "user", ProjectAction::Save);
        old.timestamp = now - chrono::Duration::days(60);
        let mut recent = AuditEvent::project_action(None, "user", ProjectAction::Save);
        recent.timestamp = now;
        audit_record(&old).unwrap();
        audit_record(&recent).unwrap();
        assert_eq!(audit_count().unwrap(), 2);

        let cutoff = now - chrono::Duration::days(30);
        let removed = audit_purge_before(cutoff).unwrap();
        assert_eq!(removed, 1);
        assert_eq!(audit_count().unwrap(), 1);
    }

    #[test]
    #[serial]
    fn audit_path_returns_string() {
        reset_audit_for_tests();
        let p = audit_path();
        assert!(!p.is_empty());
    }

    #[test]
    #[serial]
    fn query_report_serialises_to_json() {
        reset_audit_for_tests();
        let event = AuditEvent::ai_action(None, "bg_remove", "u2net", "cpu", None, vec![]);
        audit_record(&event).unwrap();
        let report = audit_query(&AuditQuery::default()).unwrap();
        let json = serde_json::to_string(&report).expect("report must serialise");
        assert!(json.contains("bg_remove"));
        assert!(json.contains("\"total\":1"));
    }
}
