//! Phase 8 bridge entry points.
//!
//! These are the workspace-level helpers backing the N-API wrappers in
//! `lib.rs` for the Phase 8 features:
//!
//! - Design-token binding (Task 21).
//! - Constraint-driven frame resize (Task 19).
//! - Smart text auto-fit (Task 23).
//! - Page-numbering tokens + section numbering (Tasks 13 + 14).
//! - Job-first export presets (Task 17).
//! - Brand-kit versioning (Task 15).
//!
//! Following the convention in `document.rs`, every public function
//! here either operates on the open workspace (gated by
//! `with_workspace` / `with_workspace_mut`) or is a pure helper that
//! does not touch the singleton. The N-API marshalling lives in
//! `lib.rs`.

use chrono::Utc;
use kcreate_core::node::{Bounds, Constraints};
use kcreate_core::operation::Operation;
use kcreate_core::token_binding;
use kcreate_export::job_presets::{presets_for_job, JobExportPresets, JobType};
use kcreate_layout::constraints::apply_constraints;
use kcreate_storage::brand_versions::{
    diff_brand_kit_versions, list_brand_kit_versions, restore_brand_kit_version,
    save_brand_kit_version, BrandKitDiff, BrandKitVersion,
};
use kcreate_text::tokens::{encode_page_number_token, PageNumberFormat};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::document::{with_workspace, with_workspace_mut, DocumentBridgeError, Result};

/// Metadata key on a [`kcreate_core::node::Node`] storing the
/// boolean autofit-enabled flag for text layers. Lives in the
/// generic `metadata` map (not `TextFrameOptions`) so existing
/// `text_frame_*` APIs are not affected.
pub const TEXT_AUTOFIT_METADATA_KEY: &str = "text_autofit";

/// Bind a style property on `node_id` to a design-token named
/// `token_name`. Returns immediately if the node or token is missing.
///
/// Records an undoable [`Operation`] so subsequent `document_undo`
/// reverses the binding.
pub fn document_bind_token(node_id: Uuid, property: &str, token_name: &str) -> Result<()> {
    with_workspace_mut(|ws| {
        let tokens = ws.project.design_tokens.clone();
        let before;
        let after;
        {
            let node = ws
                .project
                .document
                .get_node_mut(node_id)
                .ok_or(DocumentBridgeError::NodeNotFound(node_id))?;
            before =
                serde_json::to_value(&node.style.token_bindings).unwrap_or(serde_json::Value::Null);
            token_binding::bind_token(&mut node.style, property, token_name, &tokens)
                .map_err(|e| DocumentBridgeError::Internal(format!("bind_token failed: {e}")))?;
            after =
                serde_json::to_value(&node.style.token_bindings).unwrap_or(serde_json::Value::Null);
        }
        ws.project.modified_at = Utc::now();
        let op = Operation::new("user", "document_bind_token", before, after, vec![node_id]);
        ws.project.execute_operation(op);
        Ok(())
    })
}

/// Remove a token binding for `property` on `node_id`. No-op if the
/// binding doesn't exist.
pub fn document_unbind_token(node_id: Uuid, property: &str) -> Result<()> {
    with_workspace_mut(|ws| {
        let before;
        let after;
        {
            let node = ws
                .project
                .document
                .get_node_mut(node_id)
                .ok_or(DocumentBridgeError::NodeNotFound(node_id))?;
            before =
                serde_json::to_value(&node.style.token_bindings).unwrap_or(serde_json::Value::Null);
            token_binding::unbind_token(&mut node.style, property);
            after =
                serde_json::to_value(&node.style.token_bindings).unwrap_or(serde_json::Value::Null);
        }
        ws.project.modified_at = Utc::now();
        let op = Operation::new(
            "user",
            "document_unbind_token",
            before,
            after,
            vec![node_id],
        );
        ws.project.execute_operation(op);
        Ok(())
    })
}

/// Propagate the value of `token_name` to every node that has a
/// binding to it. Returns the number of nodes that were touched.
///
/// Called by the brand-kit editor when a token changes value; the
/// PROPOSAL.md §4.6 acceptance criterion requires this to complete
/// in < 100 ms even with thousands of bound nodes.
pub fn document_propagate_token(token_name: &str) -> Result<usize> {
    with_workspace_mut(|ws| {
        let tokens = ws.project.design_tokens.clone();
        let touched =
            token_binding::propagate_single_token(&mut ws.project.document, token_name, &tokens);
        if touched > 0 {
            ws.project.modified_at = Utc::now();
            let op = Operation::new(
                "system",
                "document_propagate_token",
                serde_json::Value::String(token_name.to_string()),
                serde_json::json!({ "touched": touched }),
                Vec::new(),
            );
            ws.project.execute_operation(op);
        }
        Ok(touched)
    })
}

/// Resize a frame and apply [`Constraints`] to every direct child.
///
/// `new_bounds` is in the document's coordinate space. The frame's
/// `bounds` are written; children's bounds are recomputed per their
/// constraint setting and written back. A single undoable operation
/// captures the entire batch so undo restores the previous layout
/// atomically.
pub fn document_resize_frame(frame_id: Uuid, new_bounds: Bounds) -> Result<()> {
    with_workspace_mut(|ws| {
        let parent_old: Bounds;
        let child_updates: Vec<(Uuid, Bounds, Bounds, Constraints)>;
        {
            let parent = ws
                .project
                .document
                .get_node(frame_id)
                .ok_or(DocumentBridgeError::NodeNotFound(frame_id))?;
            parent_old = parent.bounds;
            child_updates = parent
                .children
                .iter()
                .filter_map(|child_id| {
                    let child = ws.project.document.get_node(*child_id)?;
                    let resized =
                        apply_constraints(child.bounds, child.constraints, parent_old, new_bounds);
                    Some((*child_id, child.bounds, resized, child.constraints))
                })
                .collect();
        }
        let before = serde_json::json!({
            "frame": parent_old,
            "children": child_updates
                .iter()
                .map(|(id, old, _, _)| serde_json::json!({ "id": id.to_string(), "bounds": old }))
                .collect::<Vec<_>>(),
        });
        {
            let parent = ws
                .project
                .document
                .get_node_mut(frame_id)
                .ok_or(DocumentBridgeError::NodeNotFound(frame_id))?;
            parent.bounds = new_bounds;
        }
        let mut affected = vec![frame_id];
        for (child_id, _, new_child_bounds, _) in &child_updates {
            if let Some(child) = ws.project.document.get_node_mut(*child_id) {
                child.bounds = *new_child_bounds;
                affected.push(*child_id);
            }
        }
        let after = serde_json::json!({
            "frame": new_bounds,
            "children": child_updates
                .iter()
                .map(|(id, _, new_b, _)| serde_json::json!({ "id": id.to_string(), "bounds": new_b }))
                .collect::<Vec<_>>(),
        });
        ws.project.modified_at = Utc::now();
        let op = Operation::new("user", "document_resize_frame", before, after, affected);
        ws.project.execute_operation(op);
        Ok(())
    })
}

/// Set a node's `auto_fit` flag (Phase 8 Task 23) on its metadata.
///
/// Returns the previous value. Errors if the node is not a text layer.
pub fn text_set_auto_fit(node_id: Uuid, enabled: bool) -> Result<bool> {
    with_workspace_mut(|ws| {
        let previous;
        {
            let node = ws
                .project
                .document
                .get_node_mut(node_id)
                .ok_or(DocumentBridgeError::NodeNotFound(node_id))?;
            if node.node_type != kcreate_core::node::NodeType::TextLayer {
                return Err(DocumentBridgeError::WrongNodeType {
                    expected: kcreate_core::node::NodeType::TextLayer,
                    got: node.node_type,
                });
            }
            previous = node
                .metadata
                .get(TEXT_AUTOFIT_METADATA_KEY)
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            node.metadata.insert(
                TEXT_AUTOFIT_METADATA_KEY.into(),
                serde_json::Value::Bool(enabled),
            );
        }
        ws.project.modified_at = Utc::now();
        let op = Operation::new(
            "user",
            "text_set_auto_fit",
            serde_json::Value::Bool(previous),
            serde_json::Value::Bool(enabled),
            vec![node_id],
        );
        ws.project.execute_operation(op);
        Ok(previous)
    })
}

/// Build the Unicode sentinel string for a page-number token in
/// `format`. The renderer inserts the returned string into its text
/// editor at the caret; the shaper expands tokens against per-page
/// context at shape time.
#[must_use]
pub fn page_number_token(format: PageNumberFormat) -> String {
    encode_page_number_token(format)
}

/// Set the section-numbering metadata on a page. `start_number` and
/// `prefix` are both optional — passing `None` clears the value.
pub fn page_set_section(
    page_id: Uuid,
    start_number: Option<u32>,
    prefix: Option<String>,
) -> Result<()> {
    with_workspace_mut(|ws| {
        let before;
        let after;
        {
            let node = ws
                .project
                .document
                .get_node_mut(page_id)
                .ok_or(DocumentBridgeError::NodeNotFound(page_id))?;
            if node.node_type != kcreate_core::node::NodeType::Page {
                return Err(DocumentBridgeError::WrongNodeType {
                    expected: kcreate_core::node::NodeType::Page,
                    got: node.node_type,
                });
            }
            let mut layout = node.page_layout().unwrap_or_default();
            before = serde_json::to_value(&layout).unwrap_or(serde_json::Value::Null);
            layout.section_start = start_number;
            layout.section_prefix = prefix;
            node.set_page_layout(&layout);
            after = serde_json::to_value(&layout).unwrap_or(serde_json::Value::Null);
        }
        ws.project.modified_at = Utc::now();
        let op = Operation::new("user", "page_set_section", before, after, vec![page_id]);
        ws.project.execute_operation(op);
        Ok(())
    })
}

/// Curated export presets for the supplied job tile. Pure function —
/// does not touch the workspace.
#[must_use]
pub fn export_job_presets(job: JobType) -> JobExportPresets {
    presets_for_job(job)
}

/// Wire-format snapshot for a brand-kit version, returned by the
/// listing API.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrandKitVersionInfo {
    pub version_id: String,
    pub brand_kit_id: String,
    pub timestamp: String,
    pub description: String,
}

impl From<BrandKitVersion> for BrandKitVersionInfo {
    fn from(v: BrandKitVersion) -> Self {
        Self {
            version_id: v.version_id.to_string(),
            brand_kit_id: v.brand_kit_id.to_string(),
            timestamp: v.timestamp.to_rfc3339(),
            description: v.description,
        }
    }
}

/// Save a snapshot of the named brand kit. Returns the new version's
/// metadata.
pub fn brand_kit_save_version(
    brand_kit_id: Uuid,
    description: &str,
) -> Result<BrandKitVersionInfo> {
    with_workspace(|ws| {
        let brand_kit = ws
            .project
            .brand_kits
            .iter()
            .find(|bk| bk.id == brand_kit_id)
            .ok_or_else(|| {
                DocumentBridgeError::Internal(format!("brand kit {brand_kit_id} not found"))
            })?;
        let conn = ws.store.connection();
        let version = save_brand_kit_version(conn, brand_kit, description.to_string())
            .map_err(|e| DocumentBridgeError::Internal(format!("save_brand_kit_version: {e}")))?;
        Ok(BrandKitVersionInfo::from(version))
    })
}

/// List versions for a brand kit, newest first.
pub fn brand_kit_list_versions(brand_kit_id: Uuid) -> Result<Vec<BrandKitVersionInfo>> {
    with_workspace(|ws| {
        let conn = ws.store.connection();
        let versions = list_brand_kit_versions(conn, brand_kit_id)
            .map_err(|e| DocumentBridgeError::Internal(format!("list_brand_kit_versions: {e}")))?;
        Ok(versions
            .into_iter()
            .map(BrandKitVersionInfo::from)
            .collect())
    })
}

/// Restore a brand kit to the snapshot identified by `version_id`.
/// Returns the restored snapshot; also writes it to the in-memory
/// project so subsequent reads see the restored values immediately.
pub fn brand_kit_restore_version(version_id: Uuid) -> Result<kcreate_core::project::BrandKit> {
    with_workspace_mut(|ws| {
        let conn = ws.store.connection();
        let snapshot = restore_brand_kit_version(conn, version_id).map_err(|e| {
            DocumentBridgeError::Internal(format!("restore_brand_kit_version: {e}"))
        })?;
        if let Some(existing) = ws
            .project
            .brand_kits
            .iter_mut()
            .find(|bk| bk.id == snapshot.id)
        {
            *existing = snapshot.clone();
        } else {
            ws.project.brand_kits.push(snapshot.clone());
        }
        ws.project.modified_at = Utc::now();
        let op = Operation::new(
            "user",
            "brand_kit_restore_version",
            serde_json::Value::Null,
            serde_json::to_value(&snapshot).unwrap_or(serde_json::Value::Null),
            Vec::new(),
        );
        ws.project.execute_operation(op);
        Ok(snapshot)
    })
}

/// Compute the structured diff between two brand-kit snapshots
/// identified by their version ids.
pub fn brand_kit_diff(before_id: Uuid, after_id: Uuid) -> Result<BrandKitDiff> {
    with_workspace(|ws| {
        let conn = ws.store.connection();
        let before = kcreate_storage::brand_versions::load_brand_kit_version(conn, before_id)
            .map_err(|e| DocumentBridgeError::Internal(format!("load before: {e}")))?
            .ok_or_else(|| DocumentBridgeError::Internal(format!("before {before_id} missing")))?;
        let after = kcreate_storage::brand_versions::load_brand_kit_version(conn, after_id)
            .map_err(|e| DocumentBridgeError::Internal(format!("load after: {e}")))?
            .ok_or_else(|| DocumentBridgeError::Internal(format!("after {after_id} missing")))?;
        Ok(diff_brand_kit_versions(&before.snapshot, &after.snapshot))
    })
}

/// Resolved page contexts for the whole document — page index,
/// section total, and section prefix. Pure projection over the
/// current document so the renderer can substitute tokens.
#[must_use]
pub fn page_resolve_contexts() -> Vec<kcreate_text::tokens::PageContext> {
    with_workspace(|ws| {
        let pages: Vec<kcreate_text::tokens::PageDescriptor> = ws
            .project
            .document
            .iter()
            .filter(|(_, n)| n.node_type == kcreate_core::node::NodeType::Page)
            .map(|(id, n)| {
                let layout = n.page_layout().unwrap_or_default();
                kcreate_text::tokens::PageDescriptor {
                    id: *id,
                    section_start: layout.section_start,
                    section_prefix: layout.section_prefix,
                }
            })
            .collect();
        Ok(kcreate_text::tokens::resolve_page_contexts(&pages))
    })
    .unwrap_or_default()
}
