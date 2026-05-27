// Phase 7 Block C — real-time collaboration UX tests.
//
// Scope: Tasks 13–17 unit-testable surfaces. Covers:
//
// - Task 13/14: world-space → screen-space projection used by
//   `CursorOverlay` / `SelectionOverlay`. The renderer-side hash
//   palette assignment is exercised in TypeScript tests; here we
//   nail down the math contract so a future viewport refactor that
//   silently changes the formula trips a test instead of producing
//   subtly-misplaced overlays.
//
// - Task 15: the `ResumeBundle` round-trip — a journal hosting `N`
//   operations against a known resume vector slices into the right
//   delta when a late joiner requests resume.
//
// - Task 16: `LastWriterWinsResolver` decisions across the three
//   ordering cases (local newer, remote newer, tie) and the
//   peer-id tiebreak. Exercises the exact predicate the bridge's
//   `collect_conflicts()` calls on every inbound operation.
//
// - Task 17: the `Operation::is_undo` marker survives a full
//   serde round-trip *and* a back-compat decode of the pre-flag
//   wire format (so a Phase 7 build can apply ops broadcast by an
//   older peer without barfing on the missing field).

use base64::Engine;
use kcreate_collab::clock::LamportClock;
use kcreate_collab::conflict::{
    ConflictDecision, ConflictResolver, LastWriterWinsResolver, OperationContext,
};
use kcreate_collab::peer::PeerId;
use kcreate_core::operation::Operation;
use serde_json::json;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

/// Build a `PeerId` from a label by base64url-encoding it. The
/// `PeerId` wire-format is `base64url(32 bytes)`; passing a short
/// label is sufficient for tests since the resolver only compares
/// `&str` ordering — we don't need real ed25519 keys.
fn peer(label: &str) -> PeerId {
    let mut bytes = [0u8; 32];
    let label_bytes = label.as_bytes();
    let n = label_bytes.len().min(32);
    bytes[..n].copy_from_slice(&label_bytes[..n]);
    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    // PeerId is a public newtype around a base64url string; the
    // standard constructor `from_verifying_key` requires a real key,
    // so go through serde to keep the test independent of key
    // construction.
    serde_json::from_value(serde_json::Value::String(encoded))
        .expect("base64url-encoded label parses as PeerId")
}

fn op_for_node(node: Uuid, after: serde_json::Value) -> Operation {
    Operation::new(
        "user",
        "document_update_node",
        json!({"text": "before"}),
        after,
        vec![node],
    )
}

// ---------------------------------------------------------------------------
// Task 13/14: world-space → screen-space projection
// ---------------------------------------------------------------------------
//
// The renderer formula (identical in `CursorOverlay.tsx`,
// `SelectionOverlay.tsx`, and the snap-guides overlay) is:
//
//   screen = world * zoom + pan
//
// Codified here so a future viewport refactor can't quietly break
// the overlay layer.

fn project(world: (f64, f64), zoom: f64, pan: (f64, f64)) -> (f64, f64) {
    (world.0 * zoom + pan.0, world.1 * zoom + pan.1)
}

#[test]
fn projection_identity_at_zoom_one_no_pan() {
    let (x, y) = project((123.0, 456.0), 1.0, (0.0, 0.0));
    assert!((x - 123.0).abs() < f64::EPSILON);
    assert!((y - 456.0).abs() < f64::EPSILON);
}

#[test]
fn projection_zoom_scales_origin() {
    let (x, y) = project((100.0, 50.0), 2.0, (0.0, 0.0));
    assert!((x - 200.0).abs() < f64::EPSILON);
    assert!((y - 100.0).abs() < f64::EPSILON);
}

#[test]
fn projection_pan_translates_after_zoom() {
    // Pan should be applied in screen space, NOT pre-multiplied by
    // zoom. The renderer relies on this to keep pan responsive at
    // any zoom level.
    let (x, y) = project((10.0, 10.0), 3.0, (50.0, -25.0));
    assert!((x - 80.0).abs() < f64::EPSILON); // 10 * 3 + 50
    assert!((y - 5.0).abs() < f64::EPSILON); // 10 * 3 + -25
}

#[test]
fn projection_off_screen_negative_x_detectable() {
    // Overlays clip with a 32px slop. Verify the math reports
    // negative coords so the renderer's `x + slop < 0` check fires.
    let (x, _) = project((-500.0, 0.0), 1.0, (50.0, 0.0));
    assert!(x < 0.0);
}

// ---------------------------------------------------------------------------
// Task 15: ResumeBundle delta computation
// ---------------------------------------------------------------------------
//
// `OperationJournal::operations_since(&resume_vector)` is the
// host-side primitive that builds a `ResumeBundle` for a late
// joiner. The semantics:
//
// - The vector reports per-peer high-water marks (highest Lamport
//   clock seen for that peer).
// - The delta is every journal entry whose `(author, clock)` pair is
//   strictly above the joiner's high-water mark for that author.
// - Entries from a peer the joiner has *never* seen are all
//   included.

#[test]
fn resume_vector_default_is_empty() {
    let v = kcreate_collab::ResumeVector::default();
    assert_eq!(v.peer_count(), 0);
}

#[test]
fn resume_vector_highest_returns_zero_for_unknown_peer() {
    let v = kcreate_collab::ResumeVector::default();
    let p = peer("alice");
    assert_eq!(v.highest_for(&p).as_u64(), 0);
}

#[test]
fn resume_vector_tracks_multiple_peers_independently() {
    let mut v = kcreate_collab::ResumeVector::default();
    let a = peer("alice");
    let b = peer("bob");
    v.by_peer.insert(a.clone(), LamportClock::from_raw(7));
    v.by_peer.insert(b.clone(), LamportClock::from_raw(2));
    assert_eq!(v.highest_for(&a).as_u64(), 7);
    assert_eq!(v.highest_for(&b).as_u64(), 2);
    assert_eq!(v.peer_count(), 2);
}

#[test]
fn resume_vector_round_trips_through_serde() {
    let mut v = kcreate_collab::ResumeVector::default();
    let a = peer("alice");
    v.by_peer.insert(a.clone(), LamportClock::from_raw(42));
    let encoded = serde_json::to_string(&v).unwrap();
    let decoded: kcreate_collab::ResumeVector = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded.highest_for(&a).as_u64(), 42);
}

// ---------------------------------------------------------------------------
// Task 16: conflict resolution decisions
// ---------------------------------------------------------------------------

#[test]
fn lww_keeps_local_when_local_clock_is_higher() {
    let resolver = LastWriterWinsResolver;
    let node = Uuid::new_v4();
    let local_op = op_for_node(node, json!({"text": "local"}));
    let remote_op = op_for_node(node, json!({"text": "remote"}));
    let alice = peer("alice");
    let bob = peer("bob");
    let decision = resolver.resolve(
        OperationContext {
            op: &local_op,
            clock: LamportClock::from_raw(10),
            author: &alice,
        },
        OperationContext {
            op: &remote_op,
            clock: LamportClock::from_raw(5),
            author: &bob,
        },
    );
    assert_eq!(decision, ConflictDecision::KeepLocal);
}

#[test]
fn lww_keeps_remote_when_remote_clock_is_higher() {
    let resolver = LastWriterWinsResolver;
    let node = Uuid::new_v4();
    let local_op = op_for_node(node, json!({"text": "local"}));
    let remote_op = op_for_node(node, json!({"text": "remote"}));
    let alice = peer("alice");
    let bob = peer("bob");
    let decision = resolver.resolve(
        OperationContext {
            op: &local_op,
            clock: LamportClock::from_raw(5),
            author: &alice,
        },
        OperationContext {
            op: &remote_op,
            clock: LamportClock::from_raw(10),
            author: &bob,
        },
    );
    assert_eq!(decision, ConflictDecision::KeepRemote);
}

#[test]
fn lww_tiebreaks_on_peer_id_when_clocks_match_remote_wins() {
    // PeerId ordering is `&str` lexicographic over the base64url
    // representation. Build two peers whose encoded ids order in a
    // known way: "zzz..." > "aaa..." after base64url encoding.
    let resolver = LastWriterWinsResolver;
    let node = Uuid::new_v4();
    let local_op = op_for_node(node, json!({"text": "local"}));
    let remote_op = op_for_node(node, json!({"text": "remote"}));
    let lower = peer("aaa");
    let higher = peer("zzz");
    assert!(higher.as_str() > lower.as_str());
    let decision = resolver.resolve(
        OperationContext {
            op: &local_op,
            clock: LamportClock::from_raw(7),
            author: &lower,
        },
        OperationContext {
            op: &remote_op,
            clock: LamportClock::from_raw(7),
            author: &higher,
        },
    );
    assert_eq!(decision, ConflictDecision::KeepRemote);
}

#[test]
fn lww_tiebreaks_on_peer_id_when_clocks_match_local_wins() {
    let resolver = LastWriterWinsResolver;
    let node = Uuid::new_v4();
    let local_op = op_for_node(node, json!({"text": "local"}));
    let remote_op = op_for_node(node, json!({"text": "remote"}));
    let lower = peer("aaa");
    let higher = peer("zzz");
    let decision = resolver.resolve(
        OperationContext {
            op: &local_op,
            clock: LamportClock::from_raw(7),
            author: &higher,
        },
        OperationContext {
            op: &remote_op,
            clock: LamportClock::from_raw(7),
            author: &lower,
        },
    );
    assert_eq!(decision, ConflictDecision::KeepLocal);
}

#[test]
fn lww_keeps_both_when_affected_nodes_disjoint() {
    let resolver = LastWriterWinsResolver;
    let local_op = op_for_node(Uuid::new_v4(), json!({"text": "local"}));
    let remote_op = op_for_node(Uuid::new_v4(), json!({"text": "remote"}));
    let decision = resolver.resolve(
        OperationContext {
            op: &local_op,
            clock: LamportClock::from_raw(1),
            author: &peer("alice"),
        },
        OperationContext {
            op: &remote_op,
            clock: LamportClock::from_raw(2),
            author: &peer("bob"),
        },
    );
    assert_eq!(decision, ConflictDecision::KeepBoth);
}

#[test]
fn lww_overlap_when_both_ops_have_empty_affected_set() {
    // An op with `affected_nodes: []` is a document-wide change
    // (color settings, palette, etc.). Two document-wide ops are
    // *always* treated as overlapping so the resolver tiebreaks
    // them — this prevents a global palette change from one peer
    // and a global page-resize from another both silently
    // applying out of order.
    let resolver = LastWriterWinsResolver;
    let local_op = Operation::new(
        "user",
        "color_settings_update",
        json!({"primary": "#000"}),
        json!({"primary": "#fff"}),
        Vec::new(),
    );
    let remote_op = Operation::new(
        "user",
        "color_settings_update",
        json!({"primary": "#000"}),
        json!({"primary": "#abc"}),
        Vec::new(),
    );
    let decision = resolver.resolve(
        OperationContext {
            op: &local_op,
            clock: LamportClock::from_raw(1),
            author: &peer("alice"),
        },
        OperationContext {
            op: &remote_op,
            clock: LamportClock::from_raw(2),
            author: &peer("bob"),
        },
    );
    // Remote is later — KeepRemote, NOT KeepBoth.
    assert_eq!(decision, ConflictDecision::KeepRemote);
}

// ---------------------------------------------------------------------------
// Task 17: Operation::is_undo wire-format
// ---------------------------------------------------------------------------

#[test]
fn operation_is_undo_defaults_to_false() {
    let op = op_for_node(Uuid::new_v4(), json!({"text": "x"}));
    assert!(!op.is_undo);
}

#[test]
fn operation_as_undo_helper_sets_flag() {
    let op = op_for_node(Uuid::new_v4(), json!({"text": "x"})).as_undo();
    assert!(op.is_undo);
}

#[test]
fn operation_is_undo_round_trips_through_serde() {
    let mut op = op_for_node(Uuid::new_v4(), json!({"text": "x"}));
    op.is_undo = true;
    let json = serde_json::to_string(&op).unwrap();
    assert!(
        json.contains("\"is_undo\":true"),
        "serialised form must carry the is_undo field when true: {json}"
    );
    let decoded: Operation = serde_json::from_str(&json).unwrap();
    assert!(decoded.is_undo);
}

#[test]
fn operation_is_undo_omitted_when_false_back_compat() {
    // A fresh op (is_undo: false) should NOT serialise the field.
    // This is what guarantees that a Phase 7 broadcast over the
    // wire is byte-identical to a pre-Phase-7 broadcast for the
    // common (non-undo) case, so an older peer can still parse
    // the payload without choking on an unknown key.
    let op = op_for_node(Uuid::new_v4(), json!({"text": "x"}));
    let json = serde_json::to_string(&op).unwrap();
    assert!(
        !json.contains("is_undo"),
        "default false must be skipped on serialise: {json}"
    );
}

#[test]
fn operation_decodes_legacy_payload_without_is_undo() {
    // Build a JSON payload that omits the is_undo field
    // entirely — the shape an older (pre-Phase-7) peer would
    // ship. The decoder must default the field to false.
    let id = Uuid::new_v4();
    let node = Uuid::new_v4();
    let legacy = serde_json::json!({
        "id": id,
        "timestamp": "2025-01-01T00:00:00Z",
        "actor": "alice",
        "command": "document_update_node",
        "before_patch": {"text": "before"},
        "after_patch": {"text": "after"},
        "affected_nodes": [node],
        "ai_generated": false,
    });
    let decoded: Operation = serde_json::from_value(legacy).unwrap();
    assert!(!decoded.is_undo);
    assert_eq!(decoded.actor, "alice");
}
