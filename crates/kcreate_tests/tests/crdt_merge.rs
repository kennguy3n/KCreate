//! Operational CRDT merge tests.
//!
//! These tests exercise the Phase-3 CRDT layer end-to-end against
//! real `kcreate_core::operation::Operation` records that mirror what
//! `kcreate_bridge::document` produces in production — same command
//! strings, same JSON patch shape. The goal is to catch regressions
//! in the classifier (an unknown command silently routing to LWW),
//! the property-merge synthesiser (missing or overlapping keys
//! producing the wrong decision), and the delete-vs-edit guarantee
//! (a stale local edit resurrecting a deleted node).
//!
//! These tests sit in `kcreate_tests` rather than `kcreate_collab`'s
//! own `tests/` so they don't drag the `kcreate_core` dev-dependency
//! into the collab crate itself.

use kcreate_collab::conflict::OperationContext;
use kcreate_collab::crdt::{CrdtDecision, CrdtResolver, OperationCategory};
use kcreate_collab::peer::PeerKey;
use kcreate_collab::{ConflictDecision, LamportClock, PeerId, ProjectSession, SessionConfig};
use kcreate_core::operation::Operation;
use serde_json::{json, Value};
use uuid::Uuid;

fn peer(label: &str) -> PeerId {
    serde_json::from_value(Value::String(label.into())).unwrap()
}

fn ctx<'a>(op: &'a Operation, clock: u64, author: &'a PeerId) -> OperationContext<'a> {
    OperationContext {
        op,
        clock: LamportClock::from_raw(clock),
        author,
    }
}

fn update_node(node: Uuid, after: Value) -> Operation {
    Operation::new(
        "user",
        "document_update_node",
        json!({"bounds": {"x": 0.0, "y": 0.0, "w": 100.0, "h": 100.0}}),
        after,
        vec![node],
    )
}

fn delete_node(node: Uuid) -> Operation {
    Operation::new(
        "user",
        "document_delete_node",
        json!({"id": node}),
        json!(null),
        vec![node],
    )
}

fn reparent(node: Uuid, new_parent: Uuid) -> Operation {
    Operation::new(
        "user",
        "document_reparent",
        json!({"parent": null, "index": 0}),
        json!({"parent": new_parent, "index": 0}),
        vec![node],
    )
}

#[test]
fn classifier_covers_every_bridge_command() {
    // Sample one command per bridge category. This catches regressions
    // where a new bridge entry point lands and silently routes through
    // OperationCategory::Other (LWW) instead of the right rule.
    let n = Uuid::new_v4();
    let cases: &[(Operation, OperationCategory)] = &[
        (delete_node(n), OperationCategory::Delete),
        (
            Operation::new("user", "interaction_remove", json!({}), json!({}), vec![n]),
            OperationCategory::Delete,
        ),
        (
            Operation::new("user", "document_reparent", json!({}), json!({}), vec![n]),
            OperationCategory::TreeMove,
        ),
        (
            Operation::new("user", "page_reorder", json!({}), json!({}), vec![n]),
            OperationCategory::TreeMove,
        ),
        (
            Operation::new(
                "user",
                "color_settings_update",
                json!({}),
                json!({}),
                vec![],
            ),
            OperationCategory::DocumentSetting,
        ),
        (
            Operation::new(
                "user",
                "document_create_node",
                json!(null),
                json!({}),
                vec![n],
            ),
            OperationCategory::Create,
        ),
        (
            Operation::new("user", "artboard_create", json!(null), json!({}), vec![n]),
            OperationCategory::Create,
        ),
        (
            update_node(n, json!({"opacity": 0.5})),
            OperationCategory::PropertyUpdate,
        ),
        (
            Operation::new("user", "text_frame_update", json!({}), json!({}), vec![n]),
            OperationCategory::PropertyUpdate,
        ),
        (
            Operation::new("user", "artboard_resize", json!({}), json!({}), vec![n]),
            OperationCategory::PropertyUpdate,
        ),
        (
            Operation::new(
                "user",
                "totally_made_up_unknown_command",
                json!({}),
                json!({}),
                vec![n],
            ),
            OperationCategory::Other,
        ),
    ];
    for (op, expected) in cases {
        assert_eq!(
            OperationCategory::classify(op),
            *expected,
            "wrong category for {}",
            op.command,
        );
    }
}

#[test]
fn concurrent_disjoint_property_updates_merge_into_one_op() {
    let n = Uuid::new_v4();
    let local = update_node(n, json!({"opacity": 0.4}));
    let remote = update_node(n, json!({"rotation": 18.0, "blur": 2.0}));
    let alpha = peer("alpha");
    let bravo = peer("bravo");
    let decision = CrdtResolver.resolve_crdt(ctx(&local, 1, &alpha), ctx(&remote, 2, &bravo));
    let CrdtDecision::Merge(merged) = decision else {
        panic!("expected Merge, got {decision:?}");
    };
    let Value::Object(after) = &merged.after_patch else {
        panic!("merged after_patch must be an object");
    };
    assert!(after.contains_key("opacity"));
    assert!(after.contains_key("rotation"));
    assert!(after.contains_key("blur"));
    assert_eq!(merged.affected_nodes, vec![n]);
    assert_eq!(merged.actor, "crdt-merge");
    // Merge must be undoable — `before_patch` is the document state
    // that existed before either edit.
    assert!(matches!(&merged.before_patch, Value::Object(_)));
    // Same operation, different ids → callers can de-dup by id.
    assert_ne!(merged.id, local.id);
    assert_ne!(merged.id, remote.id);
}

#[test]
fn overlapping_property_updates_defer_to_lww() {
    let n = Uuid::new_v4();
    let local = update_node(n, json!({"opacity": 0.4, "rotation": 0.0}));
    let remote = update_node(n, json!({"opacity": 0.7, "blur": 4.0}));
    let alpha = peer("alpha");
    let bravo = peer("bravo");
    // Remote has the higher clock so it wins LWW.
    let decision = CrdtResolver.resolve_crdt(ctx(&local, 2, &alpha), ctx(&remote, 9, &bravo));
    assert_eq!(decision, CrdtDecision::KeepRemote);
    let decision = decision.into_conflict_decision();
    assert_eq!(decision, ConflictDecision::KeepRemote);
}

#[test]
fn concurrent_tree_moves_resolve_to_single_winner_lamport() {
    let n = Uuid::new_v4();
    let local = reparent(n, Uuid::new_v4());
    let remote = reparent(n, Uuid::new_v4());
    let alpha = peer("alpha");
    let bravo = peer("bravo");
    let decision = CrdtResolver.resolve_crdt(ctx(&local, 2, &alpha), ctx(&remote, 7, &bravo));
    assert_eq!(decision, CrdtDecision::KeepRemote);
}

#[test]
fn concurrent_tree_moves_break_ties_on_peer_id() {
    let n = Uuid::new_v4();
    let local = reparent(n, Uuid::new_v4());
    let remote = reparent(n, Uuid::new_v4());
    let alpha = peer("alpha");
    let zulu = peer("zulu");
    let decision = CrdtResolver.resolve_crdt(ctx(&local, 4, &alpha), ctx(&remote, 4, &zulu));
    assert_eq!(decision, CrdtDecision::KeepRemote);
    // Reverse: alpha < bravo, so when local is bravo and remote is
    // alpha at the same clock, local wins.
    let bravo = peer("bravo");
    let alpha2 = peer("alpha");
    let decision = CrdtResolver.resolve_crdt(ctx(&local, 4, &bravo), ctx(&remote, 4, &alpha2));
    assert_eq!(decision, CrdtDecision::KeepLocal);
}

#[test]
fn delete_always_wins_against_concurrent_edit() {
    let n = Uuid::new_v4();
    let local = update_node(n, json!({"opacity": 0.5}));
    let remote = delete_node(n);
    let alpha = peer("alpha");
    let bravo = peer("bravo");
    // Local has the much higher clock; the remote delete must still
    // win because it's the user's destruction.
    let decision =
        CrdtResolver.resolve_crdt(ctx(&local, 1_000_000, &alpha), ctx(&remote, 1, &bravo));
    assert_eq!(decision, CrdtDecision::KeepRemote);
}

#[test]
fn local_delete_wins_against_remote_edit_at_higher_clock() {
    let n = Uuid::new_v4();
    let local = delete_node(n);
    let remote = update_node(n, json!({"opacity": 0.5}));
    let alpha = peer("alpha");
    let bravo = peer("bravo");
    let decision =
        CrdtResolver.resolve_crdt(ctx(&local, 1, &alpha), ctx(&remote, 1_000_000, &bravo));
    assert_eq!(decision, CrdtDecision::KeepLocal);
}

#[test]
fn two_concurrent_deletes_resolve_to_a_single_delete_deterministically() {
    let n = Uuid::new_v4();
    let local = delete_node(n);
    let remote = delete_node(n);
    let alpha = peer("alpha");
    let zulu = peer("zulu");
    // Same clock → larger peer id wins, but it's still a single delete.
    let decision = CrdtResolver.resolve_crdt(ctx(&local, 1, &alpha), ctx(&remote, 1, &zulu));
    assert_eq!(decision, CrdtDecision::KeepRemote);
}

#[test]
fn disjoint_affected_nodes_keep_both() {
    let n1 = Uuid::new_v4();
    let n2 = Uuid::new_v4();
    let local = update_node(n1, json!({"opacity": 0.5}));
    let remote = update_node(n2, json!({"rotation": 12.0}));
    let alpha = peer("alpha");
    let bravo = peer("bravo");
    let decision = CrdtResolver.resolve_crdt(ctx(&local, 1, &alpha), ctx(&remote, 5, &bravo));
    assert_eq!(decision, CrdtDecision::KeepBoth);
}

#[test]
fn two_concurrent_creates_keep_both() {
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
    let alpha = peer("alpha");
    let bravo = peer("bravo");
    let decision = CrdtResolver.resolve_crdt(ctx(&local, 1, &alpha), ctx(&remote, 2, &bravo));
    assert_eq!(decision, CrdtDecision::KeepBoth);
}

#[test]
fn merge_synthesises_undoable_op() {
    let n = Uuid::new_v4();
    let mut local = update_node(n, json!({"opacity": 0.5}));
    local.before_patch = json!({"opacity": 1.0, "rotation": 0.0, "blur": 0.0});
    let remote = update_node(n, json!({"rotation": 25.0, "blur": 5.0}));
    let alpha = peer("alpha");
    let bravo = peer("bravo");
    let decision = CrdtResolver.resolve_crdt(ctx(&local, 1, &alpha), ctx(&remote, 2, &bravo));
    let CrdtDecision::Merge(merged) = decision else {
        panic!("expected merge");
    };
    // The `before_patch` is preserved as the local state — applying it
    // restores the document to where the user was before either edit
    // landed. That's the undo step.
    assert_eq!(
        merged.before_patch,
        json!({"opacity": 1.0, "rotation": 0.0, "blur": 0.0})
    );
    let after = merged.after_patch.as_object().expect("object after_patch");
    assert_eq!(after.get("opacity"), Some(&json!(0.5)));
    assert_eq!(after.get("rotation"), Some(&json!(25.0)));
    assert_eq!(after.get("blur"), Some(&json!(5.0)));
}

#[test]
fn project_session_resolve_crdt_uses_resolver() {
    // Smoke-test the wiring through `ProjectSession::resolve_crdt`.
    let local_key = PeerKey::from_seed([1u8; 32]);
    let project_id = Uuid::new_v4();
    let session = ProjectSession::new(
        local_key,
        "local",
        project_id,
        SessionConfig::default(),
        [0u8; 8],
    );
    let remote_peer = peer("remote-peer-id");
    let n = Uuid::new_v4();
    let local_op = update_node(n, json!({"opacity": 0.5}));
    let remote_op = update_node(n, json!({"rotation": 25.0}));
    let decision = session.resolve_crdt(
        &local_op,
        &remote_op,
        LamportClock::from_raw(3),
        &remote_peer,
        LamportClock::from_raw(5),
    );
    assert!(matches!(decision, CrdtDecision::Merge(_)));
}
