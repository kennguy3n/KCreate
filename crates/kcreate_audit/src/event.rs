//! Audit event types — what gets recorded, what the renderer queries.
//!
//! Events are stable, serializable, and forward-compatible: any new
//! event kind is added as a variant on [`AuditEventKind`] alongside
//! a `payload: serde_json::Value` so the on-disk schema doesn't have
//! to migrate every time a new bridge entry point is added.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// RFC 3339 UTC timestamp.
pub type Timestamp = DateTime<Utc>;

/// One audit-log row. Persisted into the `audit_events` table and
/// surfaced to the renderer through the bridge.
//
// `Eq` would be desirable but `AuditEventKind::Other` carries a
// `serde_json::Value` payload, and `Value` (because of `f64`) does
// not implement `Eq`. `PartialEq` is enough for tests.
#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditEvent {
    /// Stable identifier for the event. Allocated by the store at
    /// insert time so the caller doesn't have to coordinate.
    pub id: Uuid,
    /// When the event occurred (the timestamp the caller observed,
    /// not the insert time). For a wall-clock-independent insert
    /// time, sort the table by `rowid` instead — the store assigns
    /// rowids monotonically.
    pub timestamp: Timestamp,
    /// Who performed the action. `"user"` for direct user input,
    /// `"ai:<model>"` for AI-generated, `"plugin:<id>"` for plugin
    /// actions, peer id strings for collaborative sessions.
    pub actor: String,
    /// The project this event belongs to, if any. `None` for global
    /// events (e.g. open-app, close-app).
    pub project_id: Option<Uuid>,
    /// Document nodes touched by this event. Empty for events that
    /// don't have a node target (open / close / export).
    pub affected_nodes: Vec<Uuid>,
    /// What happened. The discriminator + payload below.
    pub kind: AuditEventKind,
}

/// What kind of action a row records. Each variant carries a typed
/// payload that the renderer can interpret without a second query.
#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum AuditEventKind {
    /// A document operation (the same shape the bridge's
    /// `Operation` log records, condensed for the audit row).
    Operation(OperationRecord),
    /// An AI inference action (background removal, upscale,
    /// segmentation, smart-select, palette, layout suggestion, …).
    /// `model` identifies the model pack id; `compute_device`
    /// captures whether it ran on CPU / GPU / sidecar.
    AiAction {
        action_type: String,
        model: String,
        compute_device: String,
        prompt: Option<String>,
    },
    /// Project lifecycle (open / close / save / export). Kept as a
    /// single variant with an inner discriminator because they all
    /// share the same payload shape and the renderer typically
    /// filters them as a single bucket ("project lifecycle").
    Project(ProjectAction),
    /// Anything else. Forward compatibility hook — a future event
    /// kind landing on an old SQLite file deserializes as `Other`
    /// rather than failing.
    Other {
        label: String,
        payload: serde_json::Value,
    },
}

/// Condensed operation record — the parts of
/// `kcreate_core::operation::Operation` worth persisting beyond
/// the project DB.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationRecord {
    /// The original operation id (matches the project DB's
    /// `operations.id` so an auditor can cross-reference). Not a
    /// foreign key because the project DB may live on a different
    /// machine.
    pub op_id: Uuid,
    /// The `command` string (`"node_update"`, `"page_add"`, …).
    pub command: String,
    /// Whether this op was AI-generated.
    pub ai_generated: bool,
}

/// Variants for `AuditEventKind::Project`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "action")]
pub enum ProjectAction {
    Open { path: String },
    Close,
    Save,
    Export { format: String, destination: String },
}

/// Filter passed to [`crate::AuditStore::query`].
///
/// All fields are optional; an empty `AuditQuery` matches every row.
/// Filters combine with `AND` semantics.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditQuery {
    /// Inclusive lower bound on `timestamp`.
    pub since: Option<Timestamp>,
    /// Exclusive upper bound on `timestamp`.
    pub until: Option<Timestamp>,
    /// Match the event kind by discriminator string. Use
    /// `"operation"`, `"ai_action"`, `"project"`, `"other"`. Matches
    /// the `serde` `rename_all = "snake_case"` tag.
    pub kind: Option<String>,
    /// Restrict to a specific project.
    pub project_id: Option<Uuid>,
    /// Restrict to events that touched this node.
    pub affected_node: Option<Uuid>,
    /// Maximum rows returned. The store also has a hard cap.
    pub limit: Option<u32>,
}

impl AuditEvent {
    /// Build an [`AuditEvent::Operation`] event from a project op.
    /// The audit timestamp is the operation's own timestamp so
    /// reordered inserts still sort correctly in the timeline.
    #[must_use]
    pub fn from_operation(
        project_id: Option<Uuid>,
        op: &kcreate_core::operation::Operation,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            timestamp: op.timestamp,
            actor: op.actor.clone(),
            project_id,
            affected_nodes: op.affected_nodes.clone(),
            kind: AuditEventKind::Operation(OperationRecord {
                op_id: op.id,
                command: op.command.clone(),
                ai_generated: op.ai_generated,
            }),
        }
    }

    /// Build a generic AI-action audit row.
    #[must_use]
    pub fn ai_action(
        project_id: Option<Uuid>,
        action_type: impl Into<String>,
        model: impl Into<String>,
        compute_device: impl Into<String>,
        prompt: Option<String>,
        affected_nodes: Vec<Uuid>,
    ) -> Self {
        let model = model.into();
        Self {
            id: Uuid::new_v4(),
            timestamp: Utc::now(),
            actor: format!("ai:{model}"),
            project_id,
            affected_nodes,
            kind: AuditEventKind::AiAction {
                action_type: action_type.into(),
                model,
                compute_device: compute_device.into(),
                prompt,
            },
        }
    }

    /// Build a project lifecycle audit row.
    #[must_use]
    pub fn project_action(
        project_id: Option<Uuid>,
        actor: impl Into<String>,
        action: ProjectAction,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            timestamp: Utc::now(),
            actor: actor.into(),
            project_id,
            affected_nodes: Vec::new(),
            kind: AuditEventKind::Project(action),
        }
    }

    /// The serde tag string for this event's kind. Useful as the
    /// indexable `kind` column on the database row.
    #[must_use]
    pub fn kind_tag(&self) -> &'static str {
        match &self.kind {
            AuditEventKind::Operation(_) => "operation",
            AuditEventKind::AiAction { .. } => "ai_action",
            AuditEventKind::Project(_) => "project",
            AuditEventKind::Other { .. } => "other",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_operation_preserves_timestamp_and_command() {
        let mut op = kcreate_core::operation::Operation::new(
            "user",
            "node_update",
            serde_json::Value::Null,
            serde_json::Value::Null,
            vec![],
        );
        op.ai_generated = false;
        let project_id = Some(Uuid::new_v4());
        let row = AuditEvent::from_operation(project_id, &op);
        assert_eq!(row.timestamp, op.timestamp);
        assert_eq!(row.project_id, project_id);
        assert_eq!(row.actor, "user");
        match row.kind {
            AuditEventKind::Operation(rec) => {
                assert_eq!(rec.command, "node_update");
                assert_eq!(rec.op_id, op.id);
                assert!(!rec.ai_generated);
            }
            _ => panic!("expected Operation kind"),
        }
    }

    #[test]
    fn ai_action_actor_includes_model() {
        let row = AuditEvent::ai_action(
            None,
            "bg_remove",
            "u2net-v1",
            "cpu",
            Some("remove background".into()),
            vec![],
        );
        assert_eq!(row.actor, "ai:u2net-v1");
        assert_eq!(row.kind_tag(), "ai_action");
    }

    #[test]
    fn kind_tag_matches_serde_discriminator() {
        let op_event = AuditEvent::from_operation(
            None,
            &kcreate_core::operation::Operation::new(
                "user",
                "x",
                serde_json::Value::Null,
                serde_json::Value::Null,
                vec![],
            ),
        );
        let project_event = AuditEvent::project_action(None, "user", ProjectAction::Save);
        let ai_event = AuditEvent::ai_action(None, "a", "m", "cpu", None, vec![]);
        assert_eq!(op_event.kind_tag(), "operation");
        assert_eq!(project_event.kind_tag(), "project");
        assert_eq!(ai_event.kind_tag(), "ai_action");
    }
}
