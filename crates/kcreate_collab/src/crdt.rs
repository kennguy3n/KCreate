//! Operational CRDT semantics layered on top of [`kcreate_core::operation::Operation`].
//!
//! Phase 3 task. The existing [`crate::conflict::LastWriterWinsResolver`] is
//! deterministic but coarse: any concurrent edit to overlapping affected
//! nodes drops one side. That's wrong for the common cases on a LAN
//! editing session:
//!
//! * Two designers change *disjoint* properties of the same node (one
//!   moves it, the other recolours it) — neither edit should be lost.
//! * Two designers reparent the same node — one of the two has to win,
//!   but the winner must be the same on every peer (otherwise the
//!   trees diverge).
//! * One designer deletes a node while another is mid-edit — the delete
//!   is the user-meaningful action and must win regardless of clock.
//!
//! This module provides:
//!
//! * [`OperationCategory`] — a stable classifier over `Operation::command`
//!   strings recorded by `kcreate_bridge::document` /
//!   `kcreate_bridge::phase2`.
//! * [`CrdtDecision`] — a superset of [`crate::conflict::ConflictDecision`]
//!   that additionally carries a `Merge(Operation)` variant for cases
//!   where the two sides can be combined into a single synthetic op.
//! * [`CrdtResolver`] — the resolver every transport should use. Falls
//!   back to LWW for unrecognised commands, so adding new bridge ops
//!   never silently breaks the merge layer.
//!
//! The resolver is `pub(crate)`-callable from [`crate::session`] via
//! [`crate::session::ProjectSession::resolve_crdt`] so the bridge can
//! ingest a remote op, ask the session what to do, and apply the
//! returned [`CrdtDecision`] atomically.

use kcreate_core::operation::Operation;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use uuid::Uuid;

use crate::conflict::{ConflictDecision, OperationContext};
use crate::peer::PeerId;

/// Coarse classification of an [`Operation`] by its `command` string.
///
/// The classifier is deliberately conservative: an unknown command is
/// classed as [`OperationCategory::Other`], which falls back to LWW.
/// New bridge entry points must add a case here (or accept LWW) before
/// being able to participate in property-level merging.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationCategory {
    /// A node was deleted. Delete-vs-edit conflicts always pick the
    /// delete (`KeepRemote` if remote is the delete, `KeepLocal`
    /// otherwise). Recognised by commands containing `_delete` or
    /// equal to `node_remove`.
    Delete,
    /// A node was reparented / re-ordered in the tree
    /// (`document_reparent`, `page_reorder`, `layout_recompute`,
    /// `master_page_apply`, `master_page_detach`). Concurrent moves
    /// resolve to a single deterministic winner — peer with larger
    /// Lamport clock, peer-id tiebreak. We never merge two tree moves
    /// because the tree is a single shared structure.
    TreeMove,
    /// A property on a node was updated. Concurrent edits to *disjoint*
    /// JSON keys of the same node are merged (we synthesise a new
    /// operation whose `after_patch` is the union of both sides).
    /// Concurrent edits to overlapping keys fall back to LWW.
    PropertyUpdate,
    /// A document-wide setting (`color_settings_update`,
    /// `runtime_config_update`, …). Always treated as overlapping with
    /// every other document-wide change.
    DocumentSetting,
    /// A creation operation (`document_create_node`, `artboard_create`,
    /// `artboard_duplicate`, `page_add`, `page_duplicate`,
    /// `component_*`). Creates touch their own newly-minted node id,
    /// so they're usually disjoint from anything else in flight; when
    /// they aren't (rare — same node id minted on two peers, only
    /// possible with hostile clients), we keep both because losing a
    /// creation is much worse than keeping a duplicate.
    Create,
    /// Anything not otherwise classified. Falls back to LWW.
    Other,
}

impl OperationCategory {
    /// Classify a recorded operation. Stable wire mapping — UI labels
    /// can pull `serde_json::to_string(&category).unwrap_or_default()`.
    #[must_use]
    pub fn classify(op: &Operation) -> Self {
        let cmd = op.command.as_str();
        if is_delete_command(cmd) {
            Self::Delete
        } else if is_tree_move_command(cmd) {
            Self::TreeMove
        } else if is_document_setting_command(cmd) {
            Self::DocumentSetting
        } else if is_create_command(cmd) {
            Self::Create
        } else if is_property_update_command(cmd) {
            Self::PropertyUpdate
        } else {
            Self::Other
        }
    }
}

fn is_delete_command(cmd: &str) -> bool {
    // Explicit allow-list: only bridge commands that actually delete a
    // node from the document tree count as `Delete`. Earlier revisions
    // used `cmd.ends_with("_delete") || cmd.ends_with("_remove")` as a
    // catch-all, but that misclassified document-level mutations such
    // as `spot_color_remove` (which modifies `SpotColorLibrary` with no
    // affected nodes) as node deletions, causing concurrent edits to
    // be dropped under the delete-wins rule. Anything new that isn't
    // in this list falls through to `Other` (LWW) by design — adding a
    // case here is a deliberate opt-in.
    matches!(
        cmd,
        "document_delete_node"
            | "node_delete"
            | "node_remove"
            | "artboard_delete"
            | "page_delete"
            | "page_remove"
            | "component_delete"
            | "interaction_remove"
            | "slice_delete"
    )
}

fn is_tree_move_command(cmd: &str) -> bool {
    matches!(
        cmd,
        "document_reparent"
            | "page_reorder"
            | "page_move"
            | "layout_recompute"
            | "master_page_apply"
            | "master_page_detach"
            | "layout_template_apply"
            | "node_reorder"
    )
}

fn is_document_setting_command(cmd: &str) -> bool {
    matches!(
        cmd,
        "color_settings_update"
            | "runtime_config_update"
            | "brand_kit_update"
            | "design_tokens_update"
            | "spot_color_library_update"
            | "spot_color_upsert"
            | "spot_color_remove"
            | "spot_color_load_catalog"
    )
}

fn is_create_command(cmd: &str) -> bool {
    // Creation commands mint new node ids; concurrent ones are almost
    // always disjoint, and where they're not (e.g. same id minted on two
    // peers, only possible with hostile clients) we'd rather keep both
    // than drop one.
    //
    // The clipboard / import populate commands are listed here even
    // though they're typically single-peer or single-session operations:
    // (a) `clipboard_paste` mints fresh UUIDs for every pasted root, so
    //     a remote peer pasting at the same time as us will mint a
    //     disjoint id set — `KeepBoth` is the right outcome.
    // (b) `figma_import_populate_page` / `sketch_import_populate_page`
    //     also append freshly-minted ids to an existing page; if two
    //     peers were to run an import simultaneously we want both
    //     trees to land (deduping is the user's problem, not ours).
    //
    // Registering them explicitly here (rather than letting them fall
    // through to `Other` and rely on the empty-affected_nodes
    // shortcut) makes the intent legible and guards against a future
    // maintainer adding a clipboard-like op that *reuses* source UUIDs
    // — that would silently hit the LWW fallback and drop one peer's
    // work without this explicit registration making the contract
    // visible at the classifier.
    matches!(
        cmd,
        "document_create_node"
            | "artboard_create"
            | "artboard_duplicate"
            | "page_add"
            | "page_duplicate"
            | "master_page_create"
            | "component_create_from_selection"
            | "component_instantiate"
            | "component_add_variant"
            | "interaction_add"
            | "slice_create"
            | "clipboard_paste"
            | "figma_import_populate_page"
            | "sketch_import_populate_page"
    )
}

/// Ordering invariant: `classify_command` checks `is_delete_command`,
/// `is_tree_move_command`, `is_document_setting_command`, and
/// `is_create_command` BEFORE this function. Commands matching those
/// categories (e.g. `"color_settings_update"` → DocumentSetting) are
/// already routed before the broad suffix/prefix patterns below fire.
///
/// When adding a new bridge command that matches `_update`, `apply_*`,
/// or `set_*`, register it in the appropriate specific classifier
/// first so it doesn't silently fall through to property-merge.
fn is_property_update_command(cmd: &str) -> bool {
    // The exact-match arms come first so a future renamer doesn't
    // accidentally lose the explicit registration when changing the
    // suffix/prefix matchers below. `layer_color_set` is listed
    // explicitly because the `set_*` matcher only fires for commands
    // that *start with* `set_`; a command ending with `_set` (like
    // `layer_color_set`) would otherwise fall through to `Other`.
    cmd == "document_update_node"
        || cmd == "text_frame_update"
        || cmd == "text_opentype_features_update"
        || cmd == "page_set_layout"
        || cmd == "artboard_resize"
        || cmd == "component_switch_variant"
        || cmd == "component_detach"
        || cmd == "layer_color_set"
        || cmd.ends_with("_update")
        || cmd.starts_with("apply_")
        || cmd.starts_with("set_")
}

/// Decision returned by [`CrdtResolver::resolve_crdt`]. Superset of
/// [`ConflictDecision`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CrdtDecision {
    /// Keep the local operation; discard the remote.
    KeepLocal,
    /// Replace the local operation with the remote one.
    KeepRemote,
    /// Apply both. Used when the affected node sets are disjoint, or
    /// when both sides are creations.
    KeepBoth,
    /// Replace both with this synthetic merged operation. Produced
    /// when two property updates touch disjoint JSON keys; the merge
    /// is the union of the two `after_patch`s with the original
    /// `before_patch` preserved so the operation is still undoable.
    Merge(Box<Operation>),
}

impl CrdtDecision {
    /// Adapt back to the plain [`ConflictDecision`] for callers that
    /// don't know how to apply a synthesised merge yet. A `Merge`
    /// collapses to `KeepRemote` because the merged op carries the
    /// remote side's after-state plus the local additions, and the
    /// remote peer is the authoritative observer of the union state.
    #[must_use]
    pub fn into_conflict_decision(&self) -> ConflictDecision {
        match self {
            Self::KeepLocal => ConflictDecision::KeepLocal,
            Self::KeepRemote | Self::Merge(_) => ConflictDecision::KeepRemote,
            Self::KeepBoth => ConflictDecision::KeepBoth,
        }
    }
}

/// Operational-CRDT resolver. Implements the three Phase 3 rules.
///
/// 1. **Concurrent property updates on disjoint JSON keys** → produce a
///    `Merge(Operation)` that unions the two `after_patch`s into a
///    single synthesised op. Operations touching overlapping keys fall
///    back to Lamport-clock LWW.
/// 2. **Concurrent tree moves** → Lamport-clock LWW with peer-id
///    tiebreak. The losing side is dropped on every peer because the
///    tree is one shared structure.
/// 3. **Concurrent deletes vs edits** → the delete wins regardless of
///    clock. Two simultaneous deletes also resolve to a delete (the
///    one carried by the larger clock / peer id, just so the synthesised
///    history is deterministic).
///
/// Anything not covered by these three rules falls back to the existing
/// [`LastWriterWinsResolver`](crate::conflict::LastWriterWinsResolver)
/// semantics so adding new bridge entry points never regresses.
#[derive(Debug, Default, Clone, Copy)]
pub struct CrdtResolver;

impl CrdtResolver {
    /// Resolve a local vs remote pair.
    pub fn resolve_crdt(
        &self,
        local: OperationContext<'_>,
        remote: OperationContext<'_>,
    ) -> CrdtDecision {
        // Disjoint affected nodes → both sides are safe to apply.
        // Same rule as the LWW resolver — keeps the behaviour stable
        // for the common "two designers edit different objects" case.
        if !local.op.affected_nodes.is_empty()
            && !remote.op.affected_nodes.is_empty()
            && !any_overlap(&local.op.affected_nodes, &remote.op.affected_nodes)
        {
            return CrdtDecision::KeepBoth;
        }

        let local_kind = OperationCategory::classify(local.op);
        let remote_kind = OperationCategory::classify(remote.op);

        // Rule 3: delete-vs-edit. The delete always wins, regardless
        // of clock — deleting a node is a user-visible decision, and
        // re-applying an edit to a tombstone would resurrect it. A
        // delete-vs-delete pair resolves to a single delete, using
        // the larger clock / peer id only as a determinism tiebreak.
        match (local_kind, remote_kind) {
            (OperationCategory::Delete, OperationCategory::Delete) => {
                return lww_keep(local, remote);
            }
            (OperationCategory::Delete, _) => return CrdtDecision::KeepLocal,
            (_, OperationCategory::Delete) => return CrdtDecision::KeepRemote,
            _ => {}
        }

        // Creates against creates: never lose a creation. Disjoint
        // affected-nodes was already short-circuited above, so the
        // only way both creates affect the same id is hostile / buggy
        // peers — we keep both anyway and let the document graph
        // dedupe by reassigning ids if the operation tries to mint a
        // duplicate.
        if matches!(local_kind, OperationCategory::Create)
            && matches!(remote_kind, OperationCategory::Create)
        {
            return CrdtDecision::KeepBoth;
        }

        // Rule 2: tree moves. Always single-winner.
        if matches!(local_kind, OperationCategory::TreeMove)
            || matches!(remote_kind, OperationCategory::TreeMove)
        {
            return lww_keep(local, remote);
        }

        // Rule 1: property updates with disjoint keys → merge. The
        // merge synthesises a new `after_patch` whose top-level keys
        // are the union of both sides; this is the right model for
        // `document_update_node` (full node snapshot) because every
        // remote field that isn't named in the local patch is
        // untouched by the local edit, and vice-versa.
        if matches!(local_kind, OperationCategory::PropertyUpdate)
            && matches!(remote_kind, OperationCategory::PropertyUpdate)
        {
            if let Some(merged) = merge_property_updates(local.op, remote.op) {
                return CrdtDecision::Merge(Box::new(merged));
            }
        }

        // Anything left falls back to LWW.
        lww_keep(local, remote)
    }
}

/// Public entry point used by [`crate::session::ProjectSession::resolve_crdt`].
#[must_use]
pub fn classify(op: &Operation) -> OperationCategory {
    OperationCategory::classify(op)
}

fn any_overlap(a: &[Uuid], b: &[Uuid]) -> bool {
    a.iter().any(|x| b.contains(x))
}

fn lww_keep(local: OperationContext<'_>, remote: OperationContext<'_>) -> CrdtDecision {
    match remote.clock.cmp(&local.clock) {
        std::cmp::Ordering::Greater => CrdtDecision::KeepRemote,
        std::cmp::Ordering::Less => CrdtDecision::KeepLocal,
        std::cmp::Ordering::Equal => {
            if cmp_peer(remote.author, local.author) == std::cmp::Ordering::Greater {
                CrdtDecision::KeepRemote
            } else {
                CrdtDecision::KeepLocal
            }
        }
    }
}

fn cmp_peer(a: &PeerId, b: &PeerId) -> std::cmp::Ordering {
    // PeerId is `Ord` because its inner field is a string and
    // string lexical compare is total. The conflict module already
    // relies on this — the CRDT module mirrors that contract.
    a.cmp(b)
}

/// Build a merged property-update operation if and only if the two
/// patches touch disjoint top-level JSON keys on the same node.
///
/// Returns `None` when:
/// * Either patch is not a JSON object (we can't reason about its
///   keys).
/// * The two patches share at least one top-level key (overlap →
///   defer to LWW).
/// * The operations affect different nodes (caller should already
///   have ruled that out via the disjoint-set short-circuit).
///
/// The merged op:
/// * Carries a fresh UUID and current timestamp, so the operation log
///   doesn't show two ops with the same id on different peers.
/// * Marks the actor as `"crdt-merge"` so audit trails can filter
///   synthesised ops apart from real user actions.
/// * Preserves the `before_patch` of the local side (the document
///   state when the local user started editing).
/// * Carries the union `after_patch`. Keys from the *remote* side
///   take precedence when they happen to also appear on the local
///   side (they don't, by construction — the function only returns
///   `Some` when the keys are disjoint), so the merge is
///   deterministic on every peer.
fn merge_property_updates(local: &Operation, remote: &Operation) -> Option<Operation> {
    if local.affected_nodes != remote.affected_nodes {
        return None;
    }
    let Value::Object(local_after) = &local.after_patch else {
        return None;
    };
    let Value::Object(remote_after) = &remote.after_patch else {
        return None;
    };

    // Detect overlap by walking the smaller side.
    let (smaller, larger) = if local_after.len() <= remote_after.len() {
        (local_after, remote_after)
    } else {
        (remote_after, local_after)
    };
    for key in smaller.keys() {
        if larger.contains_key(key) {
            return None;
        }
    }

    // Union the two objects.
    let mut merged = Map::new();
    for (k, v) in local_after {
        merged.insert(k.clone(), v.clone());
    }
    for (k, v) in remote_after {
        merged.insert(k.clone(), v.clone());
    }

    // Synthesise the merged op. `before_patch` stays as the local
    // side's because that's the state both peers shared before either
    // edit; both sides are equally valid baselines and using the
    // local one keeps undo traversal coherent.
    Some(Operation::new(
        "crdt-merge",
        local.command.clone(),
        local.before_patch.clone(),
        Value::Object(merged),
        local.affected_nodes.clone(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::LamportClock;
    use serde_json::json;

    fn peer(label: &str) -> PeerId {
        serde_json::from_value(serde_json::Value::String(label.into())).unwrap()
    }

    fn update_op(node: Uuid, after_obj: serde_json::Value) -> Operation {
        Operation::new(
            "user",
            "document_update_node",
            json!({"bounds": {"x": 0.0, "y": 0.0, "w": 100.0, "h": 100.0}}),
            after_obj,
            vec![node],
        )
    }

    fn delete_op(node: Uuid) -> Operation {
        Operation::new(
            "user",
            "document_delete_node",
            json!({"id": node}),
            json!(null),
            vec![node],
        )
    }

    fn reparent_op(node: Uuid, new_parent: Uuid) -> Operation {
        Operation::new(
            "user",
            "document_reparent",
            json!({"parent": null, "index": 0}),
            json!({"parent": new_parent, "index": 0}),
            vec![node],
        )
    }

    #[test]
    fn classifier_recognises_delete_commands() {
        let node = Uuid::new_v4();
        assert_eq!(
            OperationCategory::classify(&delete_op(node)),
            OperationCategory::Delete
        );
        let custom = Operation::new(
            "user",
            "slice_delete",
            json!({}),
            json!({}),
            vec![Uuid::new_v4()],
        );
        assert_eq!(
            OperationCategory::classify(&custom),
            OperationCategory::Delete
        );
    }

    #[test]
    fn classifier_recognises_tree_moves() {
        let n = Uuid::new_v4();
        assert_eq!(
            OperationCategory::classify(&reparent_op(n, Uuid::new_v4())),
            OperationCategory::TreeMove
        );
    }

    #[test]
    fn classifier_recognises_property_updates() {
        let n = Uuid::new_v4();
        assert_eq!(
            OperationCategory::classify(&update_op(n, json!({"opacity": 0.5}))),
            OperationCategory::PropertyUpdate
        );
    }

    #[test]
    fn disjoint_property_keys_merge_into_synthetic_op() {
        let n = Uuid::new_v4();
        let local = update_op(n, json!({"opacity": 0.5}));
        let remote = update_op(n, json!({"rotation": 12.0}));
        let a = peer("alpha");
        let b = peer("bravo");
        let decision = CrdtResolver.resolve_crdt(
            OperationContext {
                op: &local,
                clock: LamportClock::from_raw(1),
                author: &a,
            },
            OperationContext {
                op: &remote,
                clock: LamportClock::from_raw(2),
                author: &b,
            },
        );
        match decision {
            CrdtDecision::Merge(merged) => {
                let Value::Object(after) = &merged.after_patch else {
                    panic!("expected object after_patch")
                };
                assert!(after.contains_key("opacity"));
                assert!(after.contains_key("rotation"));
                assert_eq!(merged.affected_nodes, vec![n]);
                assert_eq!(merged.actor, "crdt-merge");
                assert_eq!(merged.command, "document_update_node");
            }
            other => panic!("expected Merge, got {other:?}"),
        }
    }

    #[test]
    fn overlapping_property_keys_fall_back_to_lww() {
        let n = Uuid::new_v4();
        let local = update_op(n, json!({"opacity": 0.5, "rotation": 0.0}));
        let remote = update_op(n, json!({"opacity": 0.25, "blur": 4.0}));
        let a = peer("alpha");
        let b = peer("bravo");
        let decision = CrdtResolver.resolve_crdt(
            OperationContext {
                op: &local,
                clock: LamportClock::from_raw(1),
                author: &a,
            },
            OperationContext {
                op: &remote,
                clock: LamportClock::from_raw(5),
                author: &b,
            },
        );
        assert_eq!(decision, CrdtDecision::KeepRemote);
    }

    #[test]
    fn concurrent_tree_moves_pick_higher_clock_winner() {
        let n = Uuid::new_v4();
        let local = reparent_op(n, Uuid::new_v4());
        let remote = reparent_op(n, Uuid::new_v4());
        let a = peer("alpha");
        let b = peer("bravo");
        let decision = CrdtResolver.resolve_crdt(
            OperationContext {
                op: &local,
                clock: LamportClock::from_raw(2),
                author: &a,
            },
            OperationContext {
                op: &remote,
                clock: LamportClock::from_raw(7),
                author: &b,
            },
        );
        assert_eq!(decision, CrdtDecision::KeepRemote);
    }

    #[test]
    fn concurrent_tree_moves_break_tie_on_peer_id() {
        let n = Uuid::new_v4();
        let local = reparent_op(n, Uuid::new_v4());
        let remote = reparent_op(n, Uuid::new_v4());
        let a = peer("alpha");
        let b = peer("zulu");
        // Same clock, larger peer id (zulu) wins.
        let decision = CrdtResolver.resolve_crdt(
            OperationContext {
                op: &local,
                clock: LamportClock::from_raw(4),
                author: &a,
            },
            OperationContext {
                op: &remote,
                clock: LamportClock::from_raw(4),
                author: &b,
            },
        );
        assert_eq!(decision, CrdtDecision::KeepRemote);
    }

    #[test]
    fn delete_wins_over_concurrent_edit_regardless_of_clock() {
        let n = Uuid::new_v4();
        let local = update_op(n, json!({"opacity": 0.5}));
        let remote = delete_op(n);
        let a = peer("alpha");
        let b = peer("bravo");
        // Local has the much-larger clock but the remote delete must
        // still win.
        let decision = CrdtResolver.resolve_crdt(
            OperationContext {
                op: &local,
                clock: LamportClock::from_raw(1_000_000),
                author: &a,
            },
            OperationContext {
                op: &remote,
                clock: LamportClock::from_raw(1),
                author: &b,
            },
        );
        assert_eq!(decision, CrdtDecision::KeepRemote);
    }

    #[test]
    fn local_delete_wins_against_remote_edit() {
        let n = Uuid::new_v4();
        let local = delete_op(n);
        let remote = update_op(n, json!({"opacity": 0.5}));
        let a = peer("alpha");
        let b = peer("bravo");
        let decision = CrdtResolver.resolve_crdt(
            OperationContext {
                op: &local,
                clock: LamportClock::from_raw(1),
                author: &a,
            },
            OperationContext {
                op: &remote,
                clock: LamportClock::from_raw(1000),
                author: &b,
            },
        );
        assert_eq!(decision, CrdtDecision::KeepLocal);
    }

    #[test]
    fn disjoint_affected_nodes_keeps_both() {
        let n1 = Uuid::new_v4();
        let n2 = Uuid::new_v4();
        let local = update_op(n1, json!({"opacity": 0.5}));
        let remote = update_op(n2, json!({"rotation": 12.0}));
        let a = peer("alpha");
        let b = peer("bravo");
        let decision = CrdtResolver.resolve_crdt(
            OperationContext {
                op: &local,
                clock: LamportClock::from_raw(1),
                author: &a,
            },
            OperationContext {
                op: &remote,
                clock: LamportClock::from_raw(2),
                author: &b,
            },
        );
        assert_eq!(decision, CrdtDecision::KeepBoth);
    }

    #[test]
    fn two_creates_keep_both_even_when_overlapping_nodes() {
        let n = Uuid::new_v4();
        let local = Operation::new(
            "user",
            "document_create_node",
            json!(null),
            json!({"id": n}),
            vec![n],
        );
        let remote = Operation::new(
            "user",
            "document_create_node",
            json!(null),
            json!({"id": n}),
            vec![n],
        );
        let a = peer("alpha");
        let b = peer("bravo");
        let decision = CrdtResolver.resolve_crdt(
            OperationContext {
                op: &local,
                clock: LamportClock::from_raw(1),
                author: &a,
            },
            OperationContext {
                op: &remote,
                clock: LamportClock::from_raw(2),
                author: &b,
            },
        );
        assert_eq!(decision, CrdtDecision::KeepBoth);
    }

    #[test]
    fn merge_preserves_local_before_patch() {
        let n = Uuid::new_v4();
        let mut local = update_op(n, json!({"opacity": 0.5}));
        local.before_patch = json!({"opacity": 1.0, "rotation": 0.0});
        let remote = update_op(n, json!({"rotation": 12.0}));
        let a = peer("alpha");
        let b = peer("bravo");
        let decision = CrdtResolver.resolve_crdt(
            OperationContext {
                op: &local,
                clock: LamportClock::from_raw(1),
                author: &a,
            },
            OperationContext {
                op: &remote,
                clock: LamportClock::from_raw(2),
                author: &b,
            },
        );
        let CrdtDecision::Merge(merged) = decision else {
            panic!("expected merge");
        };
        assert_eq!(
            merged.before_patch,
            json!({"opacity": 1.0, "rotation": 0.0})
        );
    }

    #[test]
    fn decision_collapse_to_conflict_decision_preserves_intent() {
        let n = Uuid::new_v4();
        let local = update_op(n, json!({"opacity": 0.5}));
        let merge = CrdtDecision::Merge(Box::new(local));
        assert_eq!(merge.into_conflict_decision(), ConflictDecision::KeepRemote);
        assert_eq!(
            CrdtDecision::KeepBoth.into_conflict_decision(),
            ConflictDecision::KeepBoth
        );
        assert_eq!(
            CrdtDecision::KeepLocal.into_conflict_decision(),
            ConflictDecision::KeepLocal
        );
    }

    #[test]
    fn classifier_recognises_clipboard_and_import_populate_as_create() {
        // The clipboard paste and import populate commands all mint
        // fresh UUIDs and append to an existing tree. Classifying them
        // as `Create` guards future renames / additions from silently
        // dropping a peer's work — see the doc comment on
        // `is_create_command`.
        for cmd in [
            "clipboard_paste",
            "figma_import_populate_page",
            "sketch_import_populate_page",
        ] {
            let op = Operation::new("user", cmd, json!({}), json!({}), vec![Uuid::new_v4()]);
            assert_eq!(
                OperationCategory::classify(&op),
                OperationCategory::Create,
                "{cmd} must classify as Create",
            );
        }
    }

    #[test]
    fn classifier_recognises_layer_color_set_as_property_update() {
        // `layer_color_set` ends with `_set`; the `set_*` prefix
        // matcher only fires on commands that *start* with `set_`, so
        // without the explicit exact-match arm this op would fall
        // through to `Other` and skip the per-key merge path.
        let op = Operation::new(
            "user",
            "layer_color_set",
            json!({"color": "red"}),
            json!({"color": "blue"}),
            vec![Uuid::new_v4()],
        );
        assert_eq!(
            OperationCategory::classify(&op),
            OperationCategory::PropertyUpdate,
        );
    }

    #[test]
    fn concurrent_clipboard_paste_keeps_both_sides() {
        // Two peers paste at the same instant — the freshly-minted
        // UUIDs on each side are disjoint, so both pastes must land.
        // The disjoint-affected-nodes shortcut handles this even
        // without the explicit `Create` classification, but registering
        // the command as Create means the *second* layer of defence
        // (the `Create + Create -> KeepBoth` rule at lines ~352-356)
        // also fires; either path produces `KeepBoth`.
        let local = Operation::new(
            "user",
            "clipboard_paste",
            json!({}),
            json!({}),
            vec![Uuid::new_v4()],
        );
        let remote = Operation::new(
            "user",
            "clipboard_paste",
            json!({}),
            json!({}),
            vec![Uuid::new_v4()],
        );
        let a = peer("alpha");
        let b = peer("bravo");
        let decision = CrdtResolver.resolve_crdt(
            OperationContext {
                op: &local,
                clock: LamportClock::from_raw(1),
                author: &a,
            },
            OperationContext {
                op: &remote,
                clock: LamportClock::from_raw(2),
                author: &b,
            },
        );
        assert_eq!(decision, CrdtDecision::KeepBoth);
    }
}
