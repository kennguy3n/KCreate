//! Plugin execution context.
//!
//! The Phase 2 host ABI (Task 13) adds three intrinsics that need
//! more than the input/output buffer pair the basic ABI exposed:
//!
//! * `kcreate_read_document(ptr, len) -> u32` — reads a JSON query
//!   (`{"type": "list_nodes" | "get_node", ...}`) and writes a
//!   JSON response into the plugin's output buffer.
//! * `kcreate_read_asset(hash_ptr, hash_len, buf_ptr, buf_len) -> u32`
//!   — reads an asset blob by BLAKE3 hash into plugin memory.
//! * `kcreate_write_proposal(ptr, len) -> u32` — submits a JSON
//!   proposal for a document mutation. The host validates and
//!   applies later (see [`crate::context::PluginProposal`]).
//!
//! All three are gated by [`PluginPermission`]:
//!
//! | Intrinsic                | Required permission         |
//! |--------------------------|-----------------------------|
//! | `kcreate_read_document`  | `PluginPermission::ReadDocument`  |
//! | `kcreate_read_asset`     | `PluginPermission::ReadAssets`    |
//! | `kcreate_write_proposal` | `PluginPermission::WriteDocument` |
//!
//! Missing permission → the intrinsic returns `0` so the plugin sees
//! "denied / empty response" rather than crashing. The runtime emits
//! a single log line per denial so the UI can surface
//! permission-related failures.
//!
//! Proposals follow the `agent_contract` pattern: the plugin
//! *proposes* changes, the host validates each one (permissions,
//! node-id resolution, parent-id resolution), and then applies them
//! as operations the user can undo. This is the security boundary
//! between sandboxed plugin code and the project workspace.

use std::collections::HashSet;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::manifest::PluginPermission;

/// Read-only handle for an asset blob. The plugin host produces these
/// from its content-addressed blob store; the closure returns the raw
/// bytes or `None` when the asset isn't reachable.
///
/// `Arc` so multiple plugin executions can share the same asset
/// resolver without cloning a (potentially large) closure state.
pub type AssetLoader = Arc<dyn Fn(&str) -> Option<Vec<u8>> + Send + Sync + 'static>;

/// One concrete mutation the plugin wants to apply. These are
/// validated by the host before being turned into recordable
/// operations.
///
/// `Eq` is intentionally not derived because the inner
/// `serde_json::Value` payloads can hold `f64` numbers; `PartialEq`
/// is all callers (and tests) need.
#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProposedMutation {
    /// Create a child node under `parent_id`. `node_type` is one of
    /// the strings accepted by `kcreate_bridge::document::document_create_node`
    /// (e.g. `"text_layer"`, `"vector_layer"`); `props` carries any
    /// initial property overrides (name, bounds, metadata) as a JSON
    /// object that mirrors `CreateNodeProps`.
    CreateNode {
        parent_id: Uuid,
        node_type: String,
        props: serde_json::Value,
    },
    /// Apply changes to an existing node. `changes` mirrors
    /// `UpdateNodeProps` (name / visible / locked / metadata).
    UpdateNode {
        node_id: Uuid,
        changes: serde_json::Value,
    },
    /// Remove a node (and its children) from the document.
    DeleteNode { node_id: Uuid },
}

/// A batch of [`ProposedMutation`] entries submitted by a single
/// plugin during one execution. The host applies these in order,
/// recording each accepted mutation as an `Operation` so undo /
/// redo works as for a human-authored change.
#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginProposal {
    pub plugin_id: String,
    pub mutations: Vec<ProposedMutation>,
}

/// Per-execution context. Owned by the caller (the bridge layer) and
/// mounted onto the wasmi `Store` so the extended host functions can
/// reach the document snapshot, asset loader, and permission set
/// from inside the plugin sandbox.
///
/// The context is immutable from the plugin's perspective — proposals
/// pile up on `proposals` during execution but are only *applied*
/// after the plugin returns and the host has validated them.
pub struct PluginContext {
    /// Identifier of the plugin currently running. Stamped onto every
    /// proposal so the host can attribute proposals to the right
    /// signer in the operation log.
    pub plugin_id: String,
    /// JSON snapshot of the document graph, serialized by the bridge
    /// layer before execution. Plugins read this through
    /// `kcreate_read_document`.
    pub document_snapshot: serde_json::Value,
    /// Asset loader closure. Plugins read assets through
    /// `kcreate_read_asset(hash, buf)`; the loader maps a BLAKE3 hex
    /// hash to raw bytes.
    pub asset_loader: AssetLoader,
    /// Permissions granted to this plugin. Each intrinsic checks the
    /// set on entry and denies (returns `0`) when the relevant
    /// permission is missing.
    pub permissions: HashSet<PluginPermission>,
    /// Accumulator for `kcreate_write_proposal` calls. The host
    /// drains this after the plugin returns.
    pub proposals: Vec<ProposedMutation>,
}

impl std::fmt::Debug for PluginContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginContext")
            .field("plugin_id", &self.plugin_id)
            .field(
                "document_snapshot_len",
                &self.document_snapshot.to_string().len(),
            )
            .field("permissions", &self.permissions)
            .field("proposals", &self.proposals)
            .finish_non_exhaustive()
    }
}

impl PluginContext {
    /// Build a context with no asset loader. Plugins that call
    /// `kcreate_read_asset` will get `0` (asset not found) for every
    /// hash. Useful for tests.
    #[must_use]
    pub fn empty(plugin_id: impl Into<String>) -> Self {
        Self {
            plugin_id: plugin_id.into(),
            document_snapshot: serde_json::Value::Null,
            asset_loader: Arc::new(|_| None),
            permissions: HashSet::new(),
            proposals: Vec::new(),
        }
    }

    /// Replace the document snapshot. Returns `self` to chain.
    #[must_use]
    pub fn with_snapshot(mut self, snapshot: serde_json::Value) -> Self {
        self.document_snapshot = snapshot;
        self
    }

    /// Replace the asset loader.
    #[must_use]
    pub fn with_asset_loader(mut self, loader: AssetLoader) -> Self {
        self.asset_loader = loader;
        self
    }

    /// Grant a permission.
    #[must_use]
    pub fn grant(mut self, permission: PluginPermission) -> Self {
        self.permissions.insert(permission);
        self
    }

    /// True iff `permission` was granted to this plugin.
    #[must_use]
    pub fn has(&self, permission: PluginPermission) -> bool {
        self.permissions.contains(&permission)
    }
}

/// JSON query shape accepted by `kcreate_read_document`. The shape is
/// tagged on `type` so additional query forms can be added without
/// breaking older plugins.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DocumentQuery {
    /// Return a JSON array of every node id (strings) in the
    /// document, in arbitrary order.
    ListNodes,
    /// Return the JSON for a single node, or `null` if the id is
    /// unknown.
    GetNode { id: Uuid },
    /// Return the JSON for the document's root.
    GetRoot,
}

/// Resolve a [`DocumentQuery`] against an opaque JSON snapshot of
/// the document graph. Returns the JSON response — never errors;
/// unknown ids resolve to `null` so plugins can treat the response
/// uniformly.
///
/// The shape of `snapshot` is whatever
/// `kcreate_core::document::DocumentGraph::serialize_for_ai` produced
/// at the time the plugin run started. We use `&serde_json::Value`
/// rather than the typed `DocumentGraph` so this crate stays
/// editing-path-independent.
#[must_use]
pub fn resolve_document_query(
    snapshot: &serde_json::Value,
    query: &DocumentQuery,
) -> serde_json::Value {
    match query {
        DocumentQuery::ListNodes => list_node_ids(snapshot),
        DocumentQuery::GetNode { id } => {
            find_node_by_id(snapshot, &id.to_string()).unwrap_or(serde_json::Value::Null)
        }
        DocumentQuery::GetRoot => snapshot.clone(),
    }
}

fn list_node_ids(snapshot: &serde_json::Value) -> serde_json::Value {
    let mut out: Vec<serde_json::Value> = Vec::new();
    walk_nodes(snapshot, &mut |n| {
        if let Some(id) = n.get("id").and_then(|v| v.as_str()) {
            out.push(serde_json::Value::String(id.to_string()));
        }
    });
    serde_json::Value::Array(out)
}

fn find_node_by_id(snapshot: &serde_json::Value, target: &str) -> Option<serde_json::Value> {
    let mut hit: Option<serde_json::Value> = None;
    walk_nodes(snapshot, &mut |n| {
        if hit.is_some() {
            return;
        }
        if let Some(id) = n.get("id").and_then(|v| v.as_str()) {
            if id == target {
                hit = Some(n.clone());
            }
        }
    });
    hit
}

/// Walk every object in `value` that has an `"id"` key, treating
/// the JSON tree opaquely. Both `"children": [..]` and
/// `"nodes": [..]` are followed because different serialization
/// callers use different field names; unknown shapes simply skip
/// over non-object / non-array branches.
fn walk_nodes<F: FnMut(&serde_json::Value)>(value: &serde_json::Value, visitor: &mut F) {
    match value {
        serde_json::Value::Object(map) => {
            if map.contains_key("id") {
                visitor(value);
            }
            for v in map.values() {
                walk_nodes(v, visitor);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                walk_nodes(item, visitor);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_snapshot() -> serde_json::Value {
        // Minimal shape: one root with two children. Mirrors what
        // `document::serialize_for_ai` produces at the surface.
        serde_json::json!({
            "id": "11111111-1111-1111-1111-111111111111",
            "name": "Root",
            "children": [
                { "id": "22222222-2222-2222-2222-222222222222", "name": "A", "children": [] },
                { "id": "33333333-3333-3333-3333-333333333333", "name": "B", "children": [] }
            ]
        })
    }

    #[test]
    fn list_nodes_returns_every_id() {
        let snap = sample_snapshot();
        let out = resolve_document_query(&snap, &DocumentQuery::ListNodes);
        let arr = out.as_array().expect("array");
        let ids: Vec<&str> = arr.iter().filter_map(|v| v.as_str()).collect();
        assert!(ids.contains(&"11111111-1111-1111-1111-111111111111"));
        assert!(ids.contains(&"22222222-2222-2222-2222-222222222222"));
        assert!(ids.contains(&"33333333-3333-3333-3333-333333333333"));
        assert_eq!(arr.len(), 3, "expected three ids, got {arr:?}");
    }

    #[test]
    fn get_node_resolves_known_id() {
        let snap = sample_snapshot();
        let id: Uuid = "22222222-2222-2222-2222-222222222222".parse().unwrap();
        let out = resolve_document_query(&snap, &DocumentQuery::GetNode { id });
        assert_eq!(out.get("name").and_then(|v| v.as_str()), Some("A"));
    }

    #[test]
    fn get_node_returns_null_for_unknown_id() {
        let snap = sample_snapshot();
        let id: Uuid = "99999999-9999-9999-9999-999999999999".parse().unwrap();
        let out = resolve_document_query(&snap, &DocumentQuery::GetNode { id });
        assert_eq!(out, serde_json::Value::Null);
    }

    #[test]
    fn permissions_helpers_are_set_semantics() {
        let ctx = PluginContext::empty("test")
            .grant(PluginPermission::ReadDocument)
            .grant(PluginPermission::ReadDocument); // dup
        assert!(ctx.has(PluginPermission::ReadDocument));
        assert!(!ctx.has(PluginPermission::WriteDocument));
        assert_eq!(ctx.permissions.len(), 1);
    }

    #[test]
    fn proposed_mutation_round_trips_through_json() {
        let parent: Uuid = "11111111-1111-1111-1111-111111111111".parse().unwrap();
        let cases = vec![
            ProposedMutation::CreateNode {
                parent_id: parent,
                node_type: "text_layer".into(),
                props: serde_json::json!({"name": "Hello"}),
            },
            ProposedMutation::UpdateNode {
                node_id: parent,
                changes: serde_json::json!({"visible": false}),
            },
            ProposedMutation::DeleteNode { node_id: parent },
        ];
        for m in cases {
            let json = serde_json::to_string(&m).unwrap();
            let parsed: ProposedMutation = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, m);
        }
    }
}
