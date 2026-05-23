//! Wire payloads that peers exchange inside an
//! [`crate::envelope::Envelope`].
//!
//! Each variant of [`Message`] corresponds to one network "verb"
//! between peers. New variants are additive — adding a variant
//! requires bumping [`crate::envelope::PROTOCOL_VERSION`] so receivers
//! on older builds reject the unknown payload rather than try to
//! deserialise it half-way.
//!
//! All variants use `#[serde(rename_all = "camelCase")]` at the
//! envelope boundary so the JavaScript renderer can consume the same
//! JSON without a casing translation layer (matches the convention
//! used by every other wire type in KCreate).

use chrono::{DateTime, Utc};
use kcreate_core::operation::Operation;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::journal::{JournalEntry, ResumeVector};
use crate::kchat::KChatMembership;
use crate::peer::PeerIdentity;

/// One peer-to-peer message. Tagged via `serde`'s default external
/// tagging so the JSON encoding is `{ "Hello": { … } }`-style — easy
/// to inspect in a packet capture, and unambiguous when extending.
///
/// Note: this enum deliberately implements `PartialEq` but NOT `Eq`.
/// One variant ([`Message::Presence`]) transitively carries `f64`
/// cursor coordinates through [`PresencePayload::cursor`], and
/// `f64` is not `Eq` (it has `NaN`). Adding a hand-rolled `impl Eq`
/// would be technically unsound; the wire layer only ever needs
/// `PartialEq` for round-trip tests and equality checks, so we
/// stop there.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum Message {
    /// Sent once by the joining peer right after the transport
    /// connects. Carries the joiner's identity + project id +
    /// protocol version self-report.
    Hello(HelloPayload),
    /// Sent by the host peer in response to a `Hello`. Carries the
    /// accept/reject decision and, on accept, the host's current
    /// Lamport clock so the joiner can catch up.
    Welcome(WelcomePayload),
    /// One or more committed operations from the sender's local
    /// operation log, broadcast to peers so they can apply the same
    /// command. The wrapped [`Operation`] type is the *same* type the
    /// editing path uses — there is no separate "remote op" record.
    OperationBroadcast(OperationBroadcastPayload),
    /// Periodic cursor / selection / active-page update for
    /// awareness. Not undo-relevant.
    Presence(PresencePayload),
    /// Keep-alive ping. Receivers don't reply — the transport's own
    /// keep-alive handles liveness. This variant just gives the
    /// session layer a way to record "this peer is still here at
    /// this clock value".
    Heartbeat,
    /// Sender is leaving the session. Receivers should drop the peer
    /// from their roster but keep applied operations.
    Goodbye(GoodbyeReason),
    /// Block 7: a peer asks the host for every operation it's missing
    /// since the supplied resume vector. Sent immediately after a
    /// `Welcome::Accepted` on rejoin so the joiner can catch up on
    /// history persisted since its last disconnect. The host
    /// replies with a [`Message::ResumeBundle`] containing the
    /// missing entries.
    ResumeRequest(ResumeRequestPayload),
    /// Block 7: host's reply to a [`Message::ResumeRequest`]. Carries
    /// the journal entries the requester was missing, in
    /// `(peer_id, clock)` order so applying them in receive order
    /// produces the correct document state.
    ResumeBundle(ResumeBundlePayload),
    /// Block 8: peer claims an exclusive soft-edit lock on a set of
    /// node ids. The lock is *advisory* — receivers update their
    /// roster and surface a "Ken is editing this text frame" UI,
    /// but enforcement happens locally on each peer (the LWW
    /// resolver is still the authoritative conflict path).
    LockClaim(LockClaimPayload),
    /// Block 8: peer releases previously-claimed locks. Receivers
    /// drop the entries from their lock roster. The host also
    /// auto-releases every lock a peer holds when that peer
    /// disconnects (`PeerLeft`); this variant is the explicit
    /// "I'm done editing" signal.
    LockRelease(LockReleasePayload),
}

/// Initial handshake payload sent by the joining peer.
///
/// Carries the joiner's [`KChatMembership`] attestation so the host
/// can refuse the connection if (a) the joiner is not in the host's
/// KChat group, or (b) the attestation is forged / expired. This is
/// the protocol-level half of the KChat-gated multiplayer
/// contract; the bridge layer enforces the same gate before even
/// reaching this code, but the on-wire field exists so a future
/// out-of-tree transport cannot bypass it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HelloPayload {
    /// Joining peer's public identity.
    pub identity: PeerIdentity,
    /// The project id the joiner wants to attach to. Must match the
    /// host's currently-open project.
    pub project_id: Uuid,
    /// Application-level version string ("0.0.1+phase2"). For
    /// human-readable display only; protocol gating is via
    /// [`crate::envelope::PROTOCOL_VERSION`].
    pub app_version: String,
    /// Joiner's KChat group membership attestation. The host
    /// verifies this against its own [`crate::KChatGroupAuthority`]
    /// trust root + group; mismatches reject the handshake. Absent
    /// when the joiner is not bound to a KChat group, in which case
    /// the bridge refuses to construct a Hello in the first place
    /// (defense-in-depth — the field is `Option` for compatibility
    /// with the local-roundtrip tests that don't simulate KChat,
    /// but `kcreate_collab_transport` always populates it).
    #[serde(default)]
    pub kchat_attestation: Option<KChatMembership>,
}

/// Response sent by the host peer.
///
/// Mirrors [`HelloPayload::kchat_attestation`] on the way back so
/// the joiner can verify the host is in the same group. Without
/// this mutual check, a malicious joiner could shake hands with a
/// host outside its group by forging only the inbound half.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WelcomePayload {
    /// Whether the join was accepted.
    pub status: WelcomeStatus,
    /// The host's identity so the joiner can pin the public key for
    /// future signature verification.
    pub host_identity: PeerIdentity,
    /// Host's current Lamport clock at the moment of accepting. The
    /// joiner observes this so its clock is no smaller than the
    /// host's, ensuring its first sent operation orders after every
    /// already-applied host op.
    pub host_clock: crate::clock::LamportClock,
    /// On reject, a short human-readable reason. Empty on accept.
    #[serde(default)]
    pub reject_reason: String,
    /// Host's own KChat group membership attestation. Joiner
    /// verifies against its own trust root + group; mismatches
    /// abort the session.
    #[serde(default)]
    pub kchat_attestation: Option<KChatMembership>,
}

/// Outcome of a [`Message::Hello`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WelcomeStatus {
    /// Host accepted the joiner.
    Accepted,
    /// Host rejected the joiner. `reject_reason` carries detail.
    Rejected,
}

/// Payload of [`Message::OperationBroadcast`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationBroadcastPayload {
    /// Sender's view of the project id. Receivers MUST drop the
    /// broadcast if it doesn't match their currently-open project.
    pub project_id: Uuid,
    /// One or more operations in commit order. Batching is allowed
    /// so the LAN doesn't see a flood of one-op envelopes during
    /// rapid edits.
    pub operations: Vec<Operation>,
}

/// Payload of [`Message::Presence`].
///
/// Not `Eq` because it transitively contains [`Cursor`] (`f64` fields).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresencePayload {
    /// The page the peer is currently editing, or `None` if the peer
    /// is in a non-canvas view (settings, etc.).
    pub active_page: Option<Uuid>,
    /// Currently-selected node ids on that page.
    pub selection: Vec<Uuid>,
    /// Cursor in document coordinates, or `None` if not on canvas.
    pub cursor: Option<Cursor>,
    /// Wall-clock timestamp the sender attached when emitting the
    /// presence message. Receivers use this to age-out stale cursors
    /// when a peer disconnects without sending [`Message::Goodbye`].
    pub sent_at: DateTime<Utc>,
}

/// Cursor position in document coordinates (px). Kept separate from
/// presence so it's cheap to clone and so the renderer can diff it
/// against the last seen cursor for repaint culling.
///
/// Intentionally NOT `Eq`: the coordinates are `f64`, and IEEE-754
/// allows `NaN` which breaks reflexivity. The only consumers in the
/// editing path are `assert_eq!` in tests and serde round-trips,
/// both of which only need `PartialEq`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cursor {
    pub x: f64,
    pub y: f64,
}

/// Payload of [`Message::ResumeRequest`]. Sent by the joiner right
/// after receiving a `Welcome::Accepted` on rejoin. The host uses the
/// supplied vector to compute exactly which entries in its journal
/// the joiner is missing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResumeRequestPayload {
    /// Project the requester is asking about. Must match the
    /// session's open project; mismatches drop.
    pub project_id: Uuid,
    /// What the requester has already seen. Empty for a fresh
    /// joiner — equivalent to "send me everything".
    pub since: ResumeVector,
}

/// Payload of [`Message::ResumeBundle`]. Host's reply to a
/// [`Message::ResumeRequest`]. Carries the journal entries the
/// requester didn't have, in `(peer_id, clock)` order; the requester
/// appends them to its own journal in receive order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResumeBundlePayload {
    /// Project the bundle belongs to. Receivers MUST drop if it
    /// doesn't match their open project.
    pub project_id: Uuid,
    /// The missing operations, deterministically ordered. May be
    /// empty if the requester was already up to date.
    pub operations: Vec<JournalEntry>,
}

/// Block 8: payload of [`Message::LockClaim`]. The sender wants to
/// hold an advisory edit lock on the supplied node ids until it
/// emits a matching [`Message::LockRelease`] (or disconnects).
///
/// Soft-lock semantics: receivers are expected to surface the
/// "someone else is editing" UI (greyed-out controls, "locked by
/// Ken" badge) and avoid emitting concurrent edits to the locked
/// nodes, but they are not protocol-prevented from doing so —
/// the LWW resolver remains the authoritative tiebreaker. The
/// lock just lowers the probability of UX-hostile races on
/// hot-zone nodes like text frames and table cells.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LockClaimPayload {
    /// Project the locks apply to. Receivers MUST drop the
    /// message if it doesn't match their open project.
    pub project_id: Uuid,
    /// Node ids the sender wants to lock. May be empty
    /// (no-op) or contain duplicates (receivers must dedupe).
    pub node_ids: Vec<Uuid>,
    /// Wall-clock timestamp the sender attached. Receivers
    /// use this as the lock's `acquired_at` for the UI; the
    /// session layer's own clock is the protocol-level ordering
    /// source. The renderer can display "Ken locked X 4s ago"
    /// without round-tripping to the host.
    pub acquired_at: DateTime<Utc>,
}

/// Block 8: payload of [`Message::LockRelease`]. Drops one or more
/// previously-claimed locks. Receivers remove the entries from
/// their lock roster and re-enable the corresponding controls.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LockReleasePayload {
    /// Project the releases apply to. Receivers MUST drop if
    /// mismatched.
    pub project_id: Uuid,
    /// Node ids to release. An empty list explicitly means
    /// "release everything I hold" — receivers walk their
    /// roster and drop every entry owned by the sender.
    pub node_ids: Vec<Uuid>,
}

/// Why the sender is leaving.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "detail")]
pub enum GoodbyeReason {
    /// Normal disconnect (user closed the project, app quit, etc.).
    Normal,
    /// Peer was kicked by the host. `detail` is a short reason
    /// string to surface in the UI.
    Kicked(String),
    /// Protocol-level error; the peer is leaving rather than
    /// silently corrupting state.
    Error(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::LamportClock;
    use crate::peer::PeerKey;
    use serde_json::json;

    fn key(seed: u8) -> PeerKey {
        PeerKey::from_seed([seed; 32])
    }

    #[test]
    fn hello_serialises_to_expected_shape() {
        let k = key(1);
        let msg = Message::Hello(HelloPayload {
            identity: k.identity("Ken"),
            project_id: Uuid::nil(),
            app_version: "0.0.1+phase2".into(),
            kchat_attestation: None,
        });
        let v: serde_json::Value = serde_json::to_value(&msg).unwrap();
        assert_eq!(v["kind"], "hello");
        assert_eq!(
            v["data"]["projectId"],
            "00000000-0000-0000-0000-000000000000"
        );
        assert_eq!(v["data"]["appVersion"], "0.0.1+phase2");
        assert_eq!(v["data"]["identity"]["displayName"], "Ken");
        let back: Message = serde_json::from_value(v).unwrap();
        assert_eq!(back, msg);
    }

    #[test]
    fn welcome_round_trips_with_accept_status() {
        let k = key(2);
        let msg = Message::Welcome(WelcomePayload {
            status: WelcomeStatus::Accepted,
            host_identity: k.identity("Host"),
            host_clock: LamportClock::from_raw(42),
            reject_reason: String::new(),
            kchat_attestation: None,
        });
        let s = serde_json::to_string(&msg).unwrap();
        let back: Message = serde_json::from_str(&s).unwrap();
        assert_eq!(back, msg);
    }

    #[test]
    fn operation_broadcast_uses_real_operation_type() {
        let op = Operation::new(
            "user",
            "set_text",
            json!({"text": "before"}),
            json!({"text": "after"}),
            vec![Uuid::nil()],
        );
        let msg = Message::OperationBroadcast(OperationBroadcastPayload {
            project_id: Uuid::new_v4(),
            operations: vec![op.clone()],
        });
        let s = serde_json::to_string(&msg).unwrap();
        let back: Message = serde_json::from_str(&s).unwrap();
        match back {
            Message::OperationBroadcast(p) => {
                assert_eq!(p.operations.len(), 1);
                assert_eq!(p.operations[0], op);
            }
            _ => panic!("wrong variant after round-trip"),
        }
    }

    #[test]
    fn presence_round_trip() {
        let msg = Message::Presence(PresencePayload {
            active_page: Some(Uuid::nil()),
            selection: vec![Uuid::nil()],
            cursor: Some(Cursor { x: 100.5, y: 200.0 }),
            sent_at: Utc::now(),
        });
        let s = serde_json::to_string(&msg).unwrap();
        let back: Message = serde_json::from_str(&s).unwrap();
        assert_eq!(back, msg);
    }

    #[test]
    fn heartbeat_has_no_payload() {
        let msg = Message::Heartbeat;
        let v = serde_json::to_value(&msg).unwrap();
        assert_eq!(v["kind"], "heartbeat");
        // No `data` key for unit variants under serde's tag+content rule.
        assert!(v.get("data").is_none());
        let back: Message = serde_json::from_value(v).unwrap();
        assert_eq!(back, msg);
    }

    #[test]
    fn goodbye_variants_round_trip() {
        for reason in [
            GoodbyeReason::Normal,
            GoodbyeReason::Kicked("rate-limited".into()),
            GoodbyeReason::Error("bad signature".into()),
        ] {
            let msg = Message::Goodbye(reason.clone());
            let s = serde_json::to_string(&msg).unwrap();
            let back: Message = serde_json::from_str(&s).unwrap();
            assert_eq!(back, msg);
        }
    }

    #[test]
    fn resume_request_round_trips() {
        let project_id = Uuid::new_v4();
        let mut since = ResumeVector::empty();
        since
            .by_peer
            .insert(key(3).identity("X").peer_id, LamportClock::from_raw(99));
        let msg = Message::ResumeRequest(ResumeRequestPayload {
            project_id,
            since: since.clone(),
        });
        let s = serde_json::to_string(&msg).unwrap();
        let back: Message = serde_json::from_str(&s).unwrap();
        match back {
            Message::ResumeRequest(p) => {
                assert_eq!(p.project_id, project_id);
                assert_eq!(p.since, since);
            }
            _ => panic!("wrong variant after round-trip"),
        }
    }

    #[test]
    fn lock_claim_round_trips() {
        let project_id = Uuid::new_v4();
        let node = Uuid::new_v4();
        let now = Utc::now();
        let msg = Message::LockClaim(LockClaimPayload {
            project_id,
            node_ids: vec![node],
            acquired_at: now,
        });
        let s = serde_json::to_string(&msg).unwrap();
        let back: Message = serde_json::from_str(&s).unwrap();
        match back {
            Message::LockClaim(p) => {
                assert_eq!(p.project_id, project_id);
                assert_eq!(p.node_ids, vec![node]);
                assert_eq!(p.acquired_at, now);
            }
            _ => panic!("wrong variant after round-trip"),
        }
    }

    #[test]
    fn lock_release_round_trips() {
        let project_id = Uuid::new_v4();
        let msg = Message::LockRelease(LockReleasePayload {
            project_id,
            node_ids: vec![],
        });
        let s = serde_json::to_string(&msg).unwrap();
        let back: Message = serde_json::from_str(&s).unwrap();
        match back {
            Message::LockRelease(p) => {
                assert_eq!(p.project_id, project_id);
                assert!(p.node_ids.is_empty());
            }
            _ => panic!("wrong variant after round-trip"),
        }
    }

    #[test]
    fn resume_bundle_round_trips() {
        let project_id = Uuid::new_v4();
        let op = Operation::new(
            "user",
            "set_fill",
            json!({"color": "before"}),
            json!({"color": "after"}),
            vec![Uuid::nil()],
        );
        let entry = crate::journal::JournalEntry {
            peer_id: key(4).identity("X").peer_id,
            clock: LamportClock::from_raw(7),
            project_id,
            operation: op,
        };
        let msg = Message::ResumeBundle(ResumeBundlePayload {
            project_id,
            operations: vec![entry.clone()],
        });
        let s = serde_json::to_string(&msg).unwrap();
        let back: Message = serde_json::from_str(&s).unwrap();
        match back {
            Message::ResumeBundle(p) => {
                assert_eq!(p.project_id, project_id);
                assert_eq!(p.operations, vec![entry]);
            }
            _ => panic!("wrong variant after round-trip"),
        }
    }
}
