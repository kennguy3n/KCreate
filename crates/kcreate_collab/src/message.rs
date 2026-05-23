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
}

/// Initial handshake payload sent by the joining peer.
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
}

/// Response sent by the host peer.
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
}
