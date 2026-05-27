//! KCreate audit log — Phase 6 (Tasks 13–14).
//!
//! A persistent record of every user / AI action performed on a
//! KCreate project. Unlike the operation log inside `.kstudio/`
//! (which is project-scoped and lives in the project's own SQLite
//! database under the `operations` table) the audit log:
//!
//! * lives in a **separate** SQLite database (`audit.sqlite`) under
//!   `~/.kcreate/audit/` so it survives project deletes / moves and
//!   spans every project the user touches,
//! * stores structured events (operation / AI action / project
//!   open-close / export) rather than just operation patches,
//! * supports indexed queries by date range, event kind, project id,
//!   and affected node id — so the renderer's `AuditPanel` can answer
//!   "what AI actions ran in the last 24 hours?" without scanning
//!   every project file.
//!
//! Architectural notes:
//!
//! * The audit DB is **append-only** from the bridge's perspective.
//!   No public mutation of an existing row exists. A future GDPR /
//!   retention task may add `purge_before(timestamp)` but that's
//!   strictly opt-in.
//! * Each event carries a `payload: serde_json::Value` so future
//!   event kinds don't require a schema migration — only adding a
//!   variant to [`AuditEventKind`] and updating the SQL filter
//!   helpers.
//! * The store does **not** depend on `kcreate_storage` to keep the
//!   audit pipeline operational even if the project DB schema is
//!   migrating — the two databases are intentionally independent.
//!
//! No network. No filesystem-outside-the-audit-dir. Everything is
//! local-first.

pub mod event;
pub mod store;

pub use event::{
    AuditEvent, AuditEventKind, AuditQuery, OperationRecord, ProjectAction, Timestamp,
};
pub use store::{AuditStore, AuditStoreError};
