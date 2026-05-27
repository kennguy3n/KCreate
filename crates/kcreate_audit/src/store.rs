//! Persistent SQLite-backed audit store.
//!
//! Lives outside the project DB so audit history survives across
//! projects, project deletes, and project moves.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OpenFlags, Row};
use thiserror::Error;
use uuid::Uuid;

use crate::event::{AuditEvent, AuditEventKind, AuditQuery, Timestamp};

/// Errors from audit store operations.
#[derive(Debug, Error)]
pub enum AuditStoreError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("filesystem error at {path}: {source}")]
    Filesystem {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not serialize event payload: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error(
        "audit query limit too large: {0} (max {})",
        AuditStore::MAX_QUERY_LIMIT
    )]
    LimitTooLarge(u32),
}

/// Owned SQLite connection wrapper for the audit log.
///
/// Construction is fallible (filesystem + schema migration) so we
/// only expose `open*` constructors.
#[derive(Debug)]
pub struct AuditStore {
    conn: Connection,
    path: PathBuf,
}

impl AuditStore {
    /// Hard cap on a single query's row count. The default panel
    /// view loads ~200 rows; even an aggressive backfill should
    /// never need more than this in one call.
    pub const MAX_QUERY_LIMIT: u32 = 5_000;

    /// Default audit DB location: `~/.kcreate/audit/audit.sqlite`.
    /// Falls back to `$CWD/audit.sqlite` if no home directory is
    /// resolvable (Windows + POSIX both checked).
    #[must_use]
    pub fn default_path() -> PathBuf {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| ".".into());
        PathBuf::from(home)
            .join(".kcreate")
            .join("audit")
            .join("audit.sqlite")
    }

    /// Open (and create if missing) the audit DB at `path`. Runs the
    /// schema migration to ensure the `audit_events` table + indexes
    /// exist. The parent directory is created if absent.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, AuditStoreError> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| AuditStoreError::Filesystem {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let conn = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        )?;
        let store = Self { conn, path };
        store.migrate()?;
        Ok(store)
    }

    /// Open an in-memory audit DB for tests. The schema is migrated
    /// the same way an on-disk store is.
    pub fn open_in_memory() -> Result<Self, AuditStoreError> {
        let conn = Connection::open_in_memory()?;
        let store = Self {
            conn,
            path: PathBuf::from(":memory:"),
        };
        store.migrate()?;
        Ok(store)
    }

    /// On-disk path of this audit DB (`":memory:"` for in-memory).
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn migrate(&self) -> Result<(), AuditStoreError> {
        // Single, idempotent schema. We don't bother with a
        // `_migrations` table because the audit log is append-only
        // and forward-compatible: any future column lives in the
        // `payload` JSON column instead of a schema bump.
        self.conn.execute_batch(
            r"
            CREATE TABLE IF NOT EXISTS audit_events (
                id            TEXT PRIMARY KEY,
                timestamp     TEXT NOT NULL,
                actor         TEXT NOT NULL,
                project_id    TEXT,
                kind          TEXT NOT NULL,
                affected_nodes TEXT NOT NULL,
                payload       TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_audit_timestamp
                ON audit_events(timestamp);
            CREATE INDEX IF NOT EXISTS idx_audit_kind
                ON audit_events(kind);
            CREATE INDEX IF NOT EXISTS idx_audit_project
                ON audit_events(project_id);
            ",
        )?;
        Ok(())
    }

    /// Insert one event. Returns the event's id.
    pub fn record(&mut self, event: &AuditEvent) -> Result<Uuid, AuditStoreError> {
        let kind = event.kind_tag();
        let project_id = event.project_id.map(|u| u.to_string());
        let affected = serde_json::to_string(&event.affected_nodes)?;
        let payload = serde_json::to_string(&event.kind)?;
        self.conn.execute(
            r"INSERT INTO audit_events
               (id, timestamp, actor, project_id, kind, affected_nodes, payload)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                event.id.to_string(),
                event.timestamp.to_rfc3339(),
                event.actor,
                project_id,
                kind,
                affected,
                payload,
            ],
        )?;
        Ok(event.id)
    }

    /// Batch-insert events in a single transaction. Significantly
    /// faster than calling [`record`](Self::record) in a loop because
    /// it avoids the per-statement fsync.
    pub fn record_batch(&mut self, events: &[AuditEvent]) -> Result<(), AuditStoreError> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                r"INSERT INTO audit_events
                   (id, timestamp, actor, project_id, kind, affected_nodes, payload)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for event in events {
                let project_id = event.project_id.map(|u| u.to_string());
                let affected = serde_json::to_string(&event.affected_nodes)?;
                let payload = serde_json::to_string(&event.kind)?;
                stmt.execute(params![
                    event.id.to_string(),
                    event.timestamp.to_rfc3339(),
                    event.actor,
                    project_id,
                    event.kind_tag(),
                    affected,
                    payload,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Query audit events matching `filter`. Results are ordered
    /// newest first by `timestamp`. The default limit is 500 rows.
    pub fn query(&self, filter: &AuditQuery) -> Result<Vec<AuditEvent>, AuditStoreError> {
        if let Some(limit) = filter.limit {
            if limit > Self::MAX_QUERY_LIMIT {
                return Err(AuditStoreError::LimitTooLarge(limit));
            }
        }
        // Build the parametrised WHERE incrementally so empty
        // filters don't degrade to a full table scan with a
        // hand-rolled SQL injection vector.
        let mut sql = String::from(
            "SELECT id, timestamp, actor, project_id, kind, affected_nodes, payload \
             FROM audit_events",
        );
        let mut clauses: Vec<&'static str> = Vec::new();
        let mut params_v: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(since) = filter.since {
            clauses.push("timestamp >= ?");
            params_v.push(Box::new(since.to_rfc3339()));
        }
        if let Some(until) = filter.until {
            clauses.push("timestamp < ?");
            params_v.push(Box::new(until.to_rfc3339()));
        }
        if let Some(kind) = filter.kind.as_ref() {
            clauses.push("kind = ?");
            params_v.push(Box::new(kind.clone()));
        }
        if let Some(project_id) = filter.project_id {
            clauses.push("project_id = ?");
            params_v.push(Box::new(project_id.to_string()));
        }
        // affected_node is filtered AFTER the SQL pass because it
        // lives inside a JSON column and JSON1 isn't enabled in our
        // bundled rusqlite features. The SQL pass narrows by every
        // other axis, then we post-filter in Rust.
        if !clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&clauses.join(" AND "));
        }
        sql.push_str(" ORDER BY timestamp DESC, rowid DESC");
        let limit = filter.limit.unwrap_or(500).min(Self::MAX_QUERY_LIMIT);
        write!(sql, " LIMIT {limit}").expect("writing to String never fails");

        let mut stmt = self.conn.prepare(&sql)?;
        let param_refs: Vec<&dyn rusqlite::ToSql> = params_v
            .iter()
            .map(|p| p.as_ref() as &dyn rusqlite::ToSql)
            .collect();
        let rows = stmt.query_map(param_refs.as_slice(), row_to_event)?;
        let mut out = Vec::new();
        for row in rows {
            let event = row?;
            if let Some(node_id) = filter.affected_node {
                if !event.affected_nodes.contains(&node_id) {
                    continue;
                }
            }
            out.push(event);
        }
        Ok(out)
    }

    /// Number of rows in the audit log. Useful for the panel's
    /// "showing N of TOTAL" line.
    pub fn count(&self) -> Result<u64, AuditStoreError> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM audit_events", [], |row| row.get(0))?;
        Ok(n.max(0) as u64)
    }

    /// Delete every audit row strictly older than `cutoff`. Returns
    /// the number of rows removed. Exposed for a future retention
    /// task — call sites are expected to surface this destructively
    /// behind explicit user confirmation.
    pub fn purge_before(&mut self, cutoff: Timestamp) -> Result<u64, AuditStoreError> {
        let n = self.conn.execute(
            "DELETE FROM audit_events WHERE timestamp < ?1",
            params![cutoff.to_rfc3339()],
        )?;
        Ok(n as u64)
    }
}

fn row_to_event(row: &Row<'_>) -> rusqlite::Result<AuditEvent> {
    let id: String = row.get(0)?;
    let timestamp: String = row.get(1)?;
    let actor: String = row.get(2)?;
    let project_id: Option<String> = row.get(3)?;
    let _kind: String = row.get(4)?;
    let affected: String = row.get(5)?;
    let payload: String = row.get(6)?;
    let id = Uuid::parse_str(&id).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let timestamp = chrono::DateTime::parse_from_rfc3339(&timestamp)
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(e))
        })?
        .with_timezone(&chrono::Utc);
    let project_id = match project_id {
        Some(s) => Some(Uuid::parse_str(&s).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(e))
        })?),
        None => None,
    };
    let affected_nodes: Vec<Uuid> = serde_json::from_str(&affected).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(5, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let kind: AuditEventKind = serde_json::from_str(&payload).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(6, rusqlite::types::Type::Text, Box::new(e))
    })?;
    Ok(AuditEvent {
        id,
        timestamp,
        actor,
        project_id,
        affected_nodes,
        kind,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::ProjectAction;
    use chrono::Duration;

    fn op_event(actor: &str, command: &str, node: Option<Uuid>) -> AuditEvent {
        let mut op = kcreate_core::operation::Operation::new(
            actor,
            command,
            serde_json::Value::Null,
            serde_json::Value::Null,
            node.map(|n| vec![n]).unwrap_or_default(),
        );
        op.timestamp = chrono::Utc::now();
        AuditEvent::from_operation(None, &op)
    }

    #[test]
    fn record_then_query_round_trip() {
        let mut store = AuditStore::open_in_memory().unwrap();
        let node = Uuid::new_v4();
        let event = op_event("user", "node_update", Some(node));
        let event_id = event.id;
        store.record(&event).unwrap();
        let rows = store.query(&AuditQuery::default()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, event_id);
        assert_eq!(store.count().unwrap(), 1);
    }

    #[test]
    fn query_filters_by_kind_and_project() {
        let mut store = AuditStore::open_in_memory().unwrap();
        let project_a = Uuid::new_v4();
        let project_b = Uuid::new_v4();
        let mut a_op = op_event("user", "node_update", None);
        a_op.project_id = Some(project_a);
        let mut b_op = op_event("user", "page_add", None);
        b_op.project_id = Some(project_b);
        let mut ai =
            AuditEvent::ai_action(Some(project_a), "bg_remove", "u2net", "cpu", None, vec![]);
        ai.timestamp = chrono::Utc::now();
        store.record_batch(&[a_op, b_op, ai]).unwrap();

        let only_ops = store
            .query(&AuditQuery {
                kind: Some("operation".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(only_ops.len(), 2);
        assert!(only_ops.iter().all(|e| e.kind_tag() == "operation"));

        let only_project_a = store
            .query(&AuditQuery {
                project_id: Some(project_a),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(only_project_a.len(), 2);
        assert!(only_project_a
            .iter()
            .all(|e| e.project_id == Some(project_a)));

        let only_ai = store
            .query(&AuditQuery {
                kind: Some("ai_action".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(only_ai.len(), 1);
        assert_eq!(only_ai[0].kind_tag(), "ai_action");
    }

    #[test]
    fn query_filters_by_affected_node() {
        let mut store = AuditStore::open_in_memory().unwrap();
        let n1 = Uuid::new_v4();
        let n2 = Uuid::new_v4();
        let e1 = op_event("user", "node_update", Some(n1));
        let e2 = op_event("user", "node_update", Some(n2));
        let n1_id = n1;
        store.record_batch(&[e1, e2]).unwrap();
        let rows = store
            .query(&AuditQuery {
                affected_node: Some(n1_id),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].affected_nodes, vec![n1_id]);
    }

    #[test]
    fn query_filters_by_time_range() {
        let mut store = AuditStore::open_in_memory().unwrap();
        let now = chrono::Utc::now();
        let mut old = op_event("user", "node_update", None);
        old.timestamp = now - Duration::hours(2);
        let mut mid = op_event("user", "node_update", None);
        mid.timestamp = now - Duration::minutes(30);
        let mut new = op_event("user", "node_update", None);
        new.timestamp = now;
        store.record_batch(&[old, mid, new]).unwrap();

        let cutoff = now - Duration::hours(1);
        let recent = store
            .query(&AuditQuery {
                since: Some(cutoff),
                ..Default::default()
            })
            .unwrap();
        // `mid` (30m ago) and `new` (now) survive the cutoff;
        // `old` (2h ago) is filtered out.
        assert_eq!(recent.len(), 2);
        assert!(recent.iter().all(|e| e.timestamp >= cutoff));
    }

    #[test]
    fn query_orders_newest_first() {
        let mut store = AuditStore::open_in_memory().unwrap();
        let now = chrono::Utc::now();
        let mut a = op_event("user", "first", None);
        a.timestamp = now - Duration::seconds(10);
        let mut b = op_event("user", "second", None);
        b.timestamp = now;
        let b_id = b.id;
        store.record_batch(&[a, b]).unwrap();
        let rows = store.query(&AuditQuery::default()).unwrap();
        assert_eq!(rows.len(), 2);
        // newest first
        assert_eq!(rows[0].id, b_id);
    }

    #[test]
    fn query_respects_limit_and_rejects_oversized() {
        let mut store = AuditStore::open_in_memory().unwrap();
        for _ in 0..10 {
            store.record(&op_event("user", "x", None)).unwrap();
        }
        let limited = store
            .query(&AuditQuery {
                limit: Some(3),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(limited.len(), 3);

        let oversized = store.query(&AuditQuery {
            limit: Some(AuditStore::MAX_QUERY_LIMIT + 1),
            ..Default::default()
        });
        assert!(matches!(oversized, Err(AuditStoreError::LimitTooLarge(_))));
    }

    #[test]
    fn purge_before_drops_older_rows() {
        let mut store = AuditStore::open_in_memory().unwrap();
        let now = chrono::Utc::now();
        let mut old = op_event("user", "node_update", None);
        old.timestamp = now - Duration::days(30);
        let mut new = op_event("user", "node_update", None);
        new.timestamp = now;
        let new_id = new.id;
        store.record_batch(&[old, new]).unwrap();
        let removed = store.purge_before(now - Duration::days(7)).unwrap();
        assert_eq!(removed, 1);
        let remaining = store.query(&AuditQuery::default()).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, new_id);
    }

    #[test]
    fn open_creates_parent_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nested").join("audit.sqlite");
        let store = AuditStore::open(&path).unwrap();
        assert!(path.exists(), "open() must create the audit DB file");
        assert!(path.parent().unwrap().exists(), "and its parent directory");
        assert_eq!(store.count().unwrap(), 0);
    }

    #[test]
    fn project_actions_round_trip() {
        let mut store = AuditStore::open_in_memory().unwrap();
        let project = Uuid::new_v4();
        let open_event = AuditEvent::project_action(
            Some(project),
            "user",
            ProjectAction::Open {
                path: "/tmp/poster.kstudio".into(),
            },
        );
        let save_event = AuditEvent::project_action(Some(project), "user", ProjectAction::Save);
        let export_event = AuditEvent::project_action(
            Some(project),
            "user",
            ProjectAction::Export {
                format: "pdf".into(),
                destination: "/tmp/poster.pdf".into(),
            },
        );
        let close_event = AuditEvent::project_action(Some(project), "user", ProjectAction::Close);
        store
            .record_batch(&[open_event, save_event, export_event, close_event])
            .unwrap();
        let rows = store
            .query(&AuditQuery {
                kind: Some("project".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(rows.len(), 4);
        // Ordering is newest-first by timestamp; events created with
        // `Utc::now()` in the same test run share a timestamp, so we
        // only assert membership rather than order.
        assert!(rows
            .iter()
            .any(|e| matches!(&e.kind, AuditEventKind::Project(ProjectAction::Save))));
        assert!(rows.iter().any(|e| matches!(
            &e.kind,
            AuditEventKind::Project(ProjectAction::Export { .. })
        )));
    }
}
