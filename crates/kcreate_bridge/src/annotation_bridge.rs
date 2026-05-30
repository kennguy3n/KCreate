//! Phase 8 (Task 4): annotation bridge entry points.
//!
//! Implements the four CRUD verbs exposed over the N-API surface
//! in [`crate::lib`] (`annotation_create`, `annotation_list`,
//! `annotation_resolve`, `annotation_delete`) plus a `reply` verb
//! for threaded comments. Each verb:
//!
//! 1. Mutates the project DB through the
//!    [`kcreate_storage::annotations`] helpers under
//!    [`crate::document::with_workspace_mut`].
//! 2. When the `collab` feature is enabled AND a collab session is
//!    active, broadcasts the mutation to connected peers via
//!    [`crate::collab::session_broadcast_annotation`] so every
//!    peer's local DB converges through the same upsert / delete
//!    helpers their inbound pump uses.
//!
//! Broadcast failure does NOT roll back the local write — the
//! local edit is authoritative, and peers will eventually reconcile
//! through resume bundles when the session reconnects. This
//! mirrors how `session_broadcast_operations` behaves.
//!
//! Permission pre-check. When a collab session is active AND the
//! local peer is in [`CollabPermission::Viewer`], the four mutating
//! verbs ([`annotation_create`], [`annotation_reply`],
//! [`annotation_resolve`], [`annotation_delete`]) return
//! [`DocumentBridgeError::PermissionDenied`] BEFORE touching the
//! project DB. Without the pre-check a Viewer's annotation would
//! land in their local DB, the outbound broadcast would silently
//! fail at [`crate::collab::session_broadcast_annotation`], and the
//! renderer would show a pin no other peer can see — the same
//! "I commented but no-one received it" trap that
//! [`crate::collab::session_broadcast_operations`] guards against
//! for operation broadcasts. With no collab session active the
//! pre-check is a no-op (local-first editing is always allowed).

use kcreate_core::annotation::{Annotation, AnnotationFilter, AnnotationPosition};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::document::{with_workspace, with_workspace_mut, DocumentBridgeError, Result};

/// Request payload for `annotation_create`. Wire-format mirror of
/// the same struct in `apps/desktop/shared/scene.ts`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AnnotationCreateRequest {
    pub page_id: Uuid,
    pub author_peer_id: String,
    pub author_name: String,
    pub position: AnnotationPosition,
    pub text: String,
}

/// Request payload for `annotation_reply`. Posts a reply attached
/// to an existing annotation thread root.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AnnotationReplyRequest {
    /// Parent annotation id. Either the thread head's id, or any
    /// reply within an existing thread — the storage layer walks
    /// to the thread root.
    pub parent_id: Uuid,
    pub author_peer_id: String,
    pub author_name: String,
    pub text: String,
}

/// Request payload for `annotation_list`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AnnotationListRequest {
    pub page_id: Uuid,
    /// Whether to include resolved annotations in the result. The
    /// renderer's filter UI flips this between true (show
    /// everything) and false (hide resolved).
    pub include_resolved: bool,
    /// Whether to include unresolved annotations. Almost always
    /// `true` from the UI — exposed as a separate flag so the
    /// "show only resolved" view (audit / archive) is reachable
    /// without a second list endpoint.
    pub include_unresolved: bool,
}

impl AnnotationListRequest {
    /// Convert the wire-side flags into the core
    /// [`AnnotationFilter`] used by the storage layer.
    #[must_use]
    pub const fn into_filter(self) -> AnnotationFilter {
        AnnotationFilter {
            include_resolved: self.include_resolved,
            include_unresolved: self.include_unresolved,
        }
    }
}

/// Response payload for `annotation_list`. Wraps the vector so the
/// N-API marshal layer can serialise a single JSON object instead
/// of a bare array (matches the convention used by every other
/// list-returning bridge entry point).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AnnotationListResponse {
    pub annotations: Vec<Annotation>,
}

/// Request payload for `annotation_resolve`. Used for both
/// resolve (true) and unresolve (false).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AnnotationResolveRequest {
    pub id: Uuid,
    pub resolved: bool,
}

/// Create a new top-level annotation. Persists the row, then (if
/// a collab session is active) broadcasts an upsert to peers.
pub fn annotation_create(request: AnnotationCreateRequest) -> Result<Annotation> {
    require_editor_permission()?;
    let ann = Annotation::new(
        request.page_id,
        request.author_peer_id,
        request.author_name,
        request.position,
        request.text,
    );
    let stored = ann.clone();
    with_workspace_mut(|ws| {
        kcreate_storage::annotations::upsert_annotation(ws.store.lock().connection(), &stored)
            .map_err(|e| DocumentBridgeError::Internal(format!("upsert_annotation: {e}")))?;
        Ok(())
    })?;
    broadcast_upsert(vec![ann.clone()]);
    Ok(ann)
}

/// Post a reply onto an existing annotation thread.
pub fn annotation_reply(request: AnnotationReplyRequest) -> Result<Annotation> {
    require_editor_permission()?;
    let parent = with_workspace(|ws| {
        kcreate_storage::annotations::load_annotation(
            ws.store.lock().connection(),
            request.parent_id,
        )
        .map_err(|e| DocumentBridgeError::Internal(format!("load_annotation: {e}")))?
        .ok_or_else(|| {
            DocumentBridgeError::Internal(format!(
                "annotation_reply: parent {} not found",
                request.parent_id
            ))
        })
    })?;
    let reply = Annotation::reply(
        &parent,
        request.author_peer_id,
        request.author_name,
        request.text,
    );
    let stored = reply.clone();
    with_workspace_mut(|ws| {
        kcreate_storage::annotations::upsert_annotation(ws.store.lock().connection(), &stored)
            .map_err(|e| DocumentBridgeError::Internal(format!("upsert_annotation: {e}")))?;
        Ok(())
    })?;
    broadcast_upsert(vec![reply.clone()]);
    Ok(reply)
}

/// List annotations for a single page, filtered by resolved state.
pub fn annotation_list(request: AnnotationListRequest) -> Result<AnnotationListResponse> {
    let filter = request.into_filter();
    with_workspace(|ws| {
        let annotations = kcreate_storage::annotations::list_for_page(
            ws.store.lock().connection(),
            request.page_id,
            filter,
        )
        .map_err(|e| DocumentBridgeError::Internal(format!("list_for_page: {e}")))?;
        Ok(AnnotationListResponse { annotations })
    })
}

/// Toggle the resolved flag on an annotation. Returns the new
/// resolved state. Errors if the id is unknown.
pub fn annotation_resolve(request: AnnotationResolveRequest) -> Result<bool> {
    require_editor_permission()?;
    let new_state = with_workspace_mut(|ws| {
        let state = kcreate_storage::annotations::set_resolved(
            ws.store.lock().connection(),
            request.id,
            request.resolved,
        )
        .map_err(|e| DocumentBridgeError::Internal(format!("set_resolved: {e}")))?;
        state.ok_or_else(|| {
            DocumentBridgeError::Internal(format!(
                "annotation_resolve: id {} not found",
                request.id
            ))
        })
    })?;
    // Re-read the full row so the broadcast carries the
    // authoritative timestamp + resolved flag.
    let post = with_workspace(|ws| {
        kcreate_storage::annotations::load_annotation(ws.store.lock().connection(), request.id)
            .map_err(|e| DocumentBridgeError::Internal(format!("load_annotation: {e}")))
    })?;
    if let Some(ann) = post {
        broadcast_upsert(vec![ann]);
    }
    Ok(new_state)
}

/// Delete an annotation. Returns `true` if a row was removed,
/// `false` if the id was unknown.
pub fn annotation_delete(id: Uuid) -> Result<bool> {
    require_editor_permission()?;
    // Snapshot the row BEFORE deleting so the broadcast payload
    // carries the full annotation (page_id, thread_id, position)
    // even though the id is gone from the DB by the time we send.
    let snapshot = with_workspace(|ws| {
        kcreate_storage::annotations::load_annotation(ws.store.lock().connection(), id)
            .map_err(|e| DocumentBridgeError::Internal(format!("load_annotation: {e}")))
    })?;
    let removed = with_workspace_mut(|ws| {
        kcreate_storage::annotations::delete_annotation(ws.store.lock().connection(), id)
            .map_err(|e| DocumentBridgeError::Internal(format!("delete_annotation: {e}")))
    })?;
    if removed {
        if let Some(ann) = snapshot {
            broadcast_delete(vec![ann]);
        }
    }
    Ok(removed)
}

// --- Broadcast helpers --------------------------------------------------
//
// These are wrapped in `#[cfg(feature = "collab")]` so the bridge
// still builds (and tests) when the collab feature is disabled — the
// annotation CRUD itself is local-first and does not require
// networking. When collab is on, mutations are broadcast best-effort
// (errors are swallowed because the local write is authoritative and
// peers will reconcile on the next resume bundle).

#[cfg(feature = "collab")]
fn broadcast_upsert(annotations: Vec<Annotation>) {
    let _ = crate::collab::session_broadcast_annotation(
        annotations,
        kcreate_collab::AnnotationBroadcastKind::Upsert,
    );
}

#[cfg(feature = "collab")]
fn broadcast_delete(annotations: Vec<Annotation>) {
    let _ = crate::collab::session_broadcast_annotation(
        annotations,
        kcreate_collab::AnnotationBroadcastKind::Delete,
    );
}

#[cfg(not(feature = "collab"))]
fn broadcast_upsert(_annotations: Vec<Annotation>) {}

#[cfg(not(feature = "collab"))]
fn broadcast_delete(_annotations: Vec<Annotation>) {}

// --- Permission helpers -------------------------------------------------
//
// Returns `Ok(())` when the local peer is allowed to mutate
// annotations — either no collab session is active (local-first
// editing is always allowed) or the session has assigned this peer
// `CollabPermission::Editor` / `Owner`. Returns
// `Err(DocumentBridgeError::PermissionDenied)` when a session is
// active and the local peer is in `CollabPermission::Viewer`.
//
// Mirrors the check `crate::collab::session_broadcast_operations`
// performs before broadcasting, but raised to the bridge boundary so
// the local write itself is rejected (instead of succeeding locally
// and silently failing to propagate).

#[cfg(feature = "collab")]
fn require_editor_permission() -> Result<()> {
    if crate::collab::session_local_permission() == crate::collab::CollabPermission::Viewer {
        return Err(DocumentBridgeError::PermissionDenied);
    }
    Ok(())
}

#[cfg(not(feature = "collab"))]
#[allow(clippy::unnecessary_wraps)] // signature mirrors the collab variant which CAN fail
fn require_editor_permission() -> Result<()> {
    Ok(())
}
