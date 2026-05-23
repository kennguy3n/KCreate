//! Per-project collaboration session state machine.
//!
//! The session is the small ledger each peer keeps to make sense of
//! the LAN it's on. It tracks:
//!
//! * The local peer's identity and signing key.
//! * The project id this session is scoped to (envelopes that
//!   reference a different project id are dropped).
//! * A roster of known remote peers and their last-known Lamport
//!   clocks.
//! * A bounded set of recently-seen `(peer, nonce)` pairs so replays
//!   are detected and rejected.
//!
//! The session deliberately knows **nothing** about transports.
//! `seal_message` returns a JSON envelope string that the caller can
//! hand to any byte-oriented channel (QUIC, TCP, in-memory test
//! harness); `ingest_envelope_json` parses a string back and applies
//! all of the verification + Lamport observation logic.

use std::collections::{HashMap, VecDeque};

use chrono::Utc;
use uuid::Uuid;

use crate::clock::LamportClock;
use crate::envelope::{CollabError, Envelope, NONCE_BYTES};
use crate::kchat::{KChatAuthError, NoKChatGroupAuthority, SharedKChatAuthority};
use crate::message::Message;
use crate::peer::{PeerId, PeerIdentity, PeerKey};

/// Configuration knobs for a [`ProjectSession`]. Defaults are sane
/// for the LAN-on-a-few-peers regime KCreate targets.
#[derive(Debug, Clone)]
pub struct SessionConfig {
    /// How many recent nonces to remember per peer for replay
    /// detection. At 1 ms per envelope this is ~30 s of window at
    /// the default.
    pub replay_window: usize,
    /// Cap on the number of remote peers a session will track. Keeps
    /// memory bounded against a hostile peer announcing thousands of
    /// fake identities.
    pub max_peers: usize,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            replay_window: 32_000,
            max_peers: 256,
        }
    }
}

/// Per-peer state tracked by the session.
#[derive(Debug, Clone)]
struct PeerState {
    identity: PeerIdentity,
    last_clock: LamportClock,
    recent_nonces: VecDeque<[u8; NONCE_BYTES]>,
}

/// A collaboration session for one project. Holds the local key,
/// the project id, and the roster of known peers.
pub struct ProjectSession {
    local_key: PeerKey,
    local_identity: PeerIdentity,
    project_id: Uuid,
    clock: LamportClock,
    config: SessionConfig,
    peers: HashMap<PeerId, PeerState>,
    /// Counter used to generate non-overlapping nonces. Each session
    /// gets a fresh random-ish prefix at construction; the suffix
    /// monotonically counts up. We use this instead of pulling in
    /// a `rand` dependency because we already require the caller to
    /// provide the long-lived signing seed.
    nonce_counter: u64,
    nonce_prefix: [u8; 8],
    /// KChat group authority consulted on every Hello/Welcome
    /// boundary. The default (set by [`ProjectSession::new`]) is
    /// [`NoKChatGroupAuthority`], which fails closed and locks
    /// multiplayer. Callers wire in a real authority via
    /// [`ProjectSession::with_kchat_authority`] (or
    /// [`ProjectSession::new_with_authority`]) once the user signs
    /// into a KChat group.
    kchat_authority: SharedKChatAuthority,
}

impl std::fmt::Debug for ProjectSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Avoid printing the signing key.
        f.debug_struct("ProjectSession")
            .field("local_identity", &self.local_identity)
            .field("project_id", &self.project_id)
            .field("clock", &self.clock)
            .field("known_peers", &self.peers.len())
            .finish()
    }
}

impl ProjectSession {
    /// Construct a new session.
    ///
    /// `nonce_seed` should be a fresh per-session value (e.g.
    /// derived from the project id and a wall-clock-derived counter)
    /// so two concurrent sessions on the same machine don't issue
    /// colliding nonces.
    pub fn new(
        local_key: PeerKey,
        display_name: impl Into<String>,
        project_id: Uuid,
        config: SessionConfig,
        nonce_seed: [u8; 8],
    ) -> Self {
        Self::new_with_authority(
            local_key,
            display_name,
            project_id,
            config,
            nonce_seed,
            std::sync::Arc::new(NoKChatGroupAuthority),
        )
    }

    /// Same as [`Self::new`] but with an explicit KChat group
    /// authority. The transport / bridge calls this once it has
    /// asked the (future) KChat client for a fresh attestation;
    /// the test harnesses call it with an
    /// [`crate::kchat::InProcessKChatAuthority`] driving a
    /// deterministic issuer key.
    pub fn new_with_authority(
        local_key: PeerKey,
        display_name: impl Into<String>,
        project_id: Uuid,
        config: SessionConfig,
        nonce_seed: [u8; 8],
        kchat_authority: SharedKChatAuthority,
    ) -> Self {
        let local_identity = local_key.identity(display_name);
        Self {
            local_key,
            local_identity,
            project_id,
            clock: LamportClock::default(),
            config,
            peers: HashMap::new(),
            nonce_counter: 0,
            nonce_prefix: nonce_seed,
            kchat_authority,
        }
    }

    /// Swap the session's KChat authority. The caller normally
    /// installs the real authority before the first Hello flies, but
    /// the swap is supported so a long-running session can refresh
    /// its attestation when one is close to expiry without tearing
    /// down the transport.
    pub fn set_kchat_authority(&mut self, authority: SharedKChatAuthority) {
        self.kchat_authority = authority;
    }

    /// Borrow the current KChat authority. Useful for the bridge
    /// surface that mirrors authority state to the UI lock CTA.
    #[must_use]
    pub fn kchat_authority(&self) -> &SharedKChatAuthority {
        &self.kchat_authority
    }

    /// The local peer's public identity (cheap clone — the
    /// underlying strings are short).
    #[must_use]
    pub fn local_identity(&self) -> &PeerIdentity {
        &self.local_identity
    }

    /// The project id this session is bound to.
    #[must_use]
    pub const fn project_id(&self) -> Uuid {
        self.project_id
    }

    /// The local Lamport clock value as it stands right now.
    #[must_use]
    pub const fn clock(&self) -> LamportClock {
        self.clock
    }

    /// The current number of known remote peers.
    #[must_use]
    pub fn peer_count(&self) -> usize {
        self.peers.len()
    }

    /// Iterate over the known remote peer identities. Useful for the
    /// presence UI.
    pub fn peer_identities(&self) -> impl Iterator<Item = &PeerIdentity> {
        self.peers.values().map(|p| &p.identity)
    }

    /// Manually trust a peer identity. Production wiring will call
    /// this from the trust UI after the user confirms the
    /// fingerprint match.
    pub fn trust_peer(&mut self, identity: PeerIdentity) -> Result<(), SessionError> {
        if self.peers.len() >= self.config.max_peers && !self.peers.contains_key(&identity.peer_id)
        {
            return Err(SessionError::TooManyPeers {
                max: self.config.max_peers,
            });
        }
        // Sanity-check that the identity's public key actually
        // matches the peer id it claims. A maliciously-formed Hello
        // could otherwise advertise one id but sign with another.
        let vk = identity.verifying_key().map_err(CollabError::PeerKey)?;
        let derived = PeerId::from_verifying_key(&vk);
        if derived != identity.peer_id {
            return Err(SessionError::PeerIdMismatch);
        }
        self.peers
            .entry(identity.peer_id.clone())
            .or_insert_with(|| PeerState {
                identity,
                last_clock: LamportClock::default(),
                recent_nonces: VecDeque::with_capacity(self.config.replay_window.min(64)),
            });
        Ok(())
    }

    /// Drop a peer from the roster (e.g. on Goodbye).
    pub fn forget_peer(&mut self, peer_id: &PeerId) {
        self.peers.remove(peer_id);
    }

    /// Seal a message into an envelope JSON string ready for the
    /// transport to send. The local Lamport clock is incremented
    /// once per call.
    ///
    /// KChat-gated handshakes: when `message` is a `Hello` or
    /// `Welcome` whose `kchat_attestation` is `None`, the call
    /// stamps the local authority's attestation into the payload.
    /// If the authority has no local membership (default
    /// `NoKChatGroupAuthority`) the call returns
    /// [`SessionError::KChat`] — the bridge layer is expected to
    /// surface this as the "multiplayer locked" CTA. A payload that
    /// already carries an attestation (e.g. a transport that
    /// constructed the handshake itself) passes through unchanged
    /// so the seal step doesn't re-attest unnecessarily.
    pub fn seal_message(&mut self, message: Message) -> Result<String, SessionError> {
        let message = self.attach_kchat_attestation(message)?;
        let clock = self.clock.tick();
        let nonce = self.next_nonce();
        let env = Envelope::seal(
            self.local_identity.peer_id.clone(),
            clock,
            nonce,
            message,
            self.local_key.signing_key(),
        )
        .map_err(SessionError::Collab)?;
        serde_json::to_string(&env).map_err(|e| SessionError::Encode(e.to_string()))
    }

    /// Stamp the local KChat attestation onto a Hello/Welcome
    /// payload, if the payload doesn't already carry one. Returns
    /// the (possibly transformed) payload, or an error if the local
    /// authority cannot mint an attestation.
    ///
    /// This is the canonical point where the bridge / transport
    /// hooks into the multiplayer gate; every outbound handshake
    /// flows through here.
    fn attach_kchat_attestation(&self, message: Message) -> Result<Message, SessionError> {
        match message {
            Message::Hello(mut payload) => {
                if payload.kchat_attestation.is_none() {
                    payload.kchat_attestation = Some(self.require_local_membership()?);
                }
                Ok(Message::Hello(payload))
            }
            Message::Welcome(mut payload) => {
                // Accepted welcomes must carry the host's
                // attestation so the joiner can verify the group
                // match. Rejected welcomes don't (and indeed must
                // not — the host may be rejecting precisely
                // *because* it has no KChat binding, and stalling
                // here would surface as a timeout on the joiner).
                if matches!(payload.status, crate::message::WelcomeStatus::Accepted)
                    && payload.kchat_attestation.is_none()
                {
                    payload.kchat_attestation = Some(self.require_local_membership()?);
                }
                Ok(Message::Welcome(payload))
            }
            other => Ok(other),
        }
    }

    fn require_local_membership(&self) -> Result<crate::kchat::KChatMembership, SessionError> {
        self.kchat_authority
            .local_membership()
            .ok_or(SessionError::KChat(KChatAuthError::NoKChatBinding))
    }

    /// Parse and validate an envelope coming off the transport.
    /// Returns the inner `Message` on success. The local Lamport
    /// clock is advanced via `observe` before the message is returned
    /// so the caller's subsequent sends order strictly after the
    /// received message.
    pub fn ingest_envelope_json(&mut self, raw: &str) -> Result<Message, SessionError> {
        let env: Envelope<Message> =
            serde_json::from_str(raw).map_err(|e| SessionError::Decode(e.to_string()))?;
        self.ingest_envelope(env)
    }

    /// Same as [`Self::ingest_envelope_json`] but accepts an already-
    /// parsed envelope. Exposed so test harnesses can avoid the
    /// JSON round-trip.
    pub fn ingest_envelope(&mut self, env: Envelope<Message>) -> Result<Message, SessionError> {
        let peer_state = self
            .peers
            .get(&env.from)
            .ok_or_else(|| SessionError::UntrustedPeer(env.from.clone()))?;
        let verifying = peer_state
            .identity
            .verifying_key()
            .map_err(CollabError::PeerKey)?;
        // Verify signature and protocol version.
        let _ = env.open(&verifying).map_err(SessionError::Collab)?;
        let nonce_bytes = env.nonce_bytes().map_err(SessionError::Collab)?;

        // Replay-protection: per-peer nonce window.
        if let Some(state) = self.peers.get_mut(&env.from) {
            if state.recent_nonces.contains(&nonce_bytes) {
                return Err(SessionError::Replay);
            }
            if state.recent_nonces.len() >= self.config.replay_window {
                state.recent_nonces.pop_front();
            }
            state.recent_nonces.push_back(nonce_bytes);
            state.last_clock = state.last_clock.max(env.clock);
        }

        // Project-scoping: drop messages that don't belong to this
        // project. The check is here, not in the transport, so the
        // session's invariants are self-contained.
        if let Some(message_project_id) = message_project_id(&env.payload) {
            if message_project_id != self.project_id {
                return Err(SessionError::WrongProject {
                    expected: self.project_id,
                    got: message_project_id,
                });
            }
        }

        // KChat-gated handshake check. Hello and Welcome both carry
        // a `kchat_attestation`; we verify it against the local
        // authority's trust root + group before letting the message
        // out of the session. Without this, a remote peer could
        // hand us a valid Hello signed by their own (untrusted)
        // issuer and we would happily admit them to the project.
        self.verify_remote_kchat(&env.from, &env.payload)?;

        let _ = self.clock.observe(env.clock);
        Ok(env.payload)
    }

    /// Verify the KChat attestation embedded in an inbound Hello /
    /// Welcome against the local authority's trust root + group.
    /// Non-handshake messages pass through untouched.
    fn verify_remote_kchat(&self, from: &PeerId, payload: &Message) -> Result<(), SessionError> {
        let (attestation, expected_peer_public_key) = match payload {
            Message::Hello(p) => (p.kchat_attestation.as_ref(), p.identity.public_key.as_str()),
            Message::Welcome(p) => {
                // The Welcome carries the *host's* identity, not
                // the receiving peer's. The host's attestation must
                // match the host's public key, so we cross-check
                // against `p.host_identity.public_key` here, not
                // `env.from`'s identity (they refer to the same
                // peer, but we want the binding to be obvious).
                (
                    p.kchat_attestation.as_ref(),
                    p.host_identity.public_key.as_str(),
                )
            }
            _ => return Ok(()),
        };
        let attestation = attestation.ok_or(SessionError::KChat(KChatAuthError::NoKChatBinding))?;
        self.kchat_authority
            .verify_remote(from, expected_peer_public_key, attestation, Utc::now())
            .map_err(SessionError::KChat)
    }

    fn next_nonce(&mut self) -> [u8; NONCE_BYTES] {
        let mut nonce = [0u8; NONCE_BYTES];
        nonce[..8].copy_from_slice(&self.nonce_prefix);
        self.nonce_counter = self.nonce_counter.wrapping_add(1);
        nonce[8..].copy_from_slice(&self.nonce_counter.to_be_bytes());
        nonce
    }
}

/// Pull the project id out of any [`Message`] that carries one.
/// `Heartbeat`, `Presence`, `Goodbye`, `Welcome` don't carry a
/// project id — they're scoped by the transport connection itself,
/// which is already 1-1 with a project.
const fn message_project_id(msg: &Message) -> Option<Uuid> {
    match msg {
        Message::Hello(p) => Some(p.project_id),
        Message::OperationBroadcast(p) => Some(p.project_id),
        Message::ResumeRequest(p) => Some(p.project_id),
        Message::ResumeBundle(p) => Some(p.project_id),
        Message::Welcome(_) | Message::Presence(_) | Message::Heartbeat | Message::Goodbye(_) => {
            None
        }
    }
}

/// Errors emitted by [`ProjectSession`] methods.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    /// Underlying envelope encode / verify / decode failure.
    #[error(transparent)]
    Collab(#[from] CollabError),
    /// The envelope came from a peer we don't trust.
    #[error("envelope from untrusted peer {0}")]
    UntrustedPeer(PeerId),
    /// A peer announced an identity whose declared id doesn't match
    /// its public key — refuse the trust call.
    #[error("peer id does not match the supplied public key")]
    PeerIdMismatch,
    /// More peers than [`SessionConfig::max_peers`] would allow.
    #[error("session full: at most {max} peers")]
    TooManyPeers { max: usize },
    /// The envelope's nonce was already seen from this peer.
    #[error("envelope nonce replay detected")]
    Replay,
    /// The envelope refers to a project this session isn't bound to.
    #[error("envelope project id {got} does not match session project id {expected}")]
    WrongProject { expected: Uuid, got: Uuid },
    /// JSON encoding failed.
    #[error("envelope encode failed: {0}")]
    Encode(String),
    /// JSON decoding failed.
    #[error("envelope decode failed: {0}")]
    Decode(String),
    /// The KChat group authority rejected an inbound or outbound
    /// handshake — the local user is not signed into a KChat group,
    /// the remote peer's attestation doesn't match the local trust
    /// root or group, or the attestation is forged / expired. The
    /// bridge surfaces this as the "multiplayer locked" CTA.
    #[error("KChat gate rejected handshake: {0}")]
    KChat(#[from] KChatAuthError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kchat::{InProcessKChatAuthority, KChatGroupId};
    use crate::message::{
        GoodbyeReason, HelloPayload, OperationBroadcastPayload, PresencePayload, WelcomePayload,
        WelcomeStatus,
    };
    use kcreate_core::operation::Operation;
    use std::sync::Arc;

    /// Shared issuer seed used by every in-test KChat authority so
    /// the same trust root is recognised by every session, while
    /// the group id is varied per scenario to exercise cross-group
    /// rejection paths.
    const TEST_ISSUER_SEED: [u8; 32] = [0xAB; 32];

    fn test_group() -> KChatGroupId {
        KChatGroupId::new("test-group").unwrap()
    }

    fn make_authority(seed: u8, group: KChatGroupId) -> SharedKChatAuthority {
        let key = PeerKey::from_seed([seed; 32]);
        let identity = key.identity(format!("peer-{seed}"));
        let issued = Utc::now() - chrono::Duration::minutes(1);
        let expires = Utc::now() + chrono::Duration::hours(1);
        Arc::new(
            InProcessKChatAuthority::for_peer(
                TEST_ISSUER_SEED,
                group,
                identity.peer_id,
                identity.public_key,
                issued,
                expires,
            )
            .unwrap(),
        )
    }

    fn make_session(seed: u8, project: Uuid) -> ProjectSession {
        ProjectSession::new_with_authority(
            PeerKey::from_seed([seed; 32]),
            format!("peer-{seed}"),
            project,
            SessionConfig::default(),
            [seed; 8],
            make_authority(seed, test_group()),
        )
    }

    fn pair() -> (ProjectSession, ProjectSession, Uuid) {
        let project = Uuid::new_v4();
        let mut a = make_session(1, project);
        let mut b = make_session(2, project);
        let a_ident = a.local_identity().clone();
        let b_ident = b.local_identity().clone();
        a.trust_peer(b_ident).unwrap();
        b.trust_peer(a_ident).unwrap();
        (a, b, project)
    }

    #[test]
    fn hello_round_trip_advances_clocks() {
        let (mut a, mut b, project) = pair();
        let hello = Message::Hello(HelloPayload {
            identity: a.local_identity().clone(),
            project_id: project,
            app_version: "0.0.1+phase2".into(),
            kchat_attestation: None,
        });
        let env = a.seal_message(hello.clone()).unwrap();
        assert_eq!(a.clock().as_u64(), 1);
        let got = b.ingest_envelope_json(&env).unwrap();
        // The sealed message has the attestation stamped in by
        // `seal_message`; the input had `None`. Compare the
        // structural payload modulo the attestation field.
        match (&got, &hello) {
            (Message::Hello(got_p), Message::Hello(want_p)) => {
                assert_eq!(got_p.identity, want_p.identity);
                assert_eq!(got_p.project_id, want_p.project_id);
                assert_eq!(got_p.app_version, want_p.app_version);
                assert!(got_p.kchat_attestation.is_some());
            }
            _ => panic!("unexpected variant"),
        }
        assert!(b.clock().as_u64() >= a.clock().as_u64());
    }

    #[test]
    fn replays_are_rejected() {
        let (mut a, mut b, project) = pair();
        let msg = Message::Hello(HelloPayload {
            identity: a.local_identity().clone(),
            project_id: project,
            app_version: "v".into(),
            kchat_attestation: None,
        });
        let env = a.seal_message(msg).unwrap();
        b.ingest_envelope_json(&env).unwrap();
        // Same envelope twice — replay.
        let err = b.ingest_envelope_json(&env).unwrap_err();
        assert!(matches!(err, SessionError::Replay), "got {err:?}");
    }

    #[test]
    fn untrusted_peer_is_rejected() {
        let project = Uuid::new_v4();
        let mut a = make_session(10, project);
        let mut b = make_session(11, project);
        // A trusts B, but B does not trust A.
        a.trust_peer(b.local_identity().clone()).unwrap();
        let msg = Message::Heartbeat;
        let env = a.seal_message(msg).unwrap();
        let err = b.ingest_envelope_json(&env).unwrap_err();
        assert!(matches!(err, SessionError::UntrustedPeer(_)), "got {err:?}");
    }

    #[test]
    fn wrong_project_is_rejected() {
        let (mut a, mut b, _project) = pair();
        // Forge a hello for a different project id.
        let foreign = Uuid::new_v4();
        let msg = Message::Hello(HelloPayload {
            identity: a.local_identity().clone(),
            project_id: foreign,
            app_version: "v".into(),
            kchat_attestation: None,
        });
        let env = a.seal_message(msg).unwrap();
        let err = b.ingest_envelope_json(&env).unwrap_err();
        assert!(
            matches!(err, SessionError::WrongProject { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn kchat_unbound_local_cannot_seal_hello() {
        let project = Uuid::new_v4();
        // Default session (NoKChatGroupAuthority) — multiplayer locked.
        let mut a = ProjectSession::new(
            PeerKey::from_seed([5; 32]),
            "locked",
            project,
            SessionConfig::default(),
            [5; 8],
        );
        let msg = Message::Hello(HelloPayload {
            identity: a.local_identity().clone(),
            project_id: project,
            app_version: "v".into(),
            kchat_attestation: None,
        });
        let err = a.seal_message(msg).unwrap_err();
        assert!(
            matches!(err, SessionError::KChat(KChatAuthError::NoKChatBinding)),
            "got {err:?}"
        );
    }

    #[test]
    fn kchat_cross_group_rejects_remote_hello() {
        let project = Uuid::new_v4();
        // A is in group "alpha".
        let mut a = ProjectSession::new_with_authority(
            PeerKey::from_seed([20; 32]),
            "a",
            project,
            SessionConfig::default(),
            [20; 8],
            make_authority(20, KChatGroupId::new("alpha").unwrap()),
        );
        // B is in group "beta".
        let mut b = ProjectSession::new_with_authority(
            PeerKey::from_seed([21; 32]),
            "b",
            project,
            SessionConfig::default(),
            [21; 8],
            make_authority(21, KChatGroupId::new("beta").unwrap()),
        );
        a.trust_peer(b.local_identity().clone()).unwrap();
        b.trust_peer(a.local_identity().clone()).unwrap();
        let msg = Message::Hello(HelloPayload {
            identity: a.local_identity().clone(),
            project_id: project,
            app_version: "v".into(),
            kchat_attestation: None,
        });
        let env = a.seal_message(msg).unwrap();
        let err = b.ingest_envelope_json(&env).unwrap_err();
        assert!(
            matches!(
                err,
                SessionError::KChat(KChatAuthError::GroupMismatch { .. })
            ),
            "got {err:?}"
        );
    }

    #[test]
    fn operation_broadcast_round_trips_real_operation() {
        let (mut a, mut b, project) = pair();
        let op = Operation::new(
            "user",
            "set_text",
            serde_json::json!({}),
            serde_json::json!({"text": "hi"}),
            vec![Uuid::nil()],
        );
        let msg = Message::OperationBroadcast(OperationBroadcastPayload {
            project_id: project,
            operations: vec![op.clone()],
        });
        let env = a.seal_message(msg).unwrap();
        let got = b.ingest_envelope_json(&env).unwrap();
        match got {
            Message::OperationBroadcast(p) => {
                assert_eq!(p.operations.len(), 1);
                assert_eq!(p.operations[0], op);
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn presence_heartbeat_goodbye_welcome_skip_project_check() {
        let (mut a, mut b, _project) = pair();
        // Presence has no project_id, must be accepted.
        let msg = Message::Presence(PresencePayload {
            active_page: None,
            selection: vec![],
            cursor: None,
            sent_at: chrono::Utc::now(),
        });
        let env = a.seal_message(msg.clone()).unwrap();
        assert_eq!(b.ingest_envelope_json(&env).unwrap(), msg);

        // Heartbeat
        let env = a.seal_message(Message::Heartbeat).unwrap();
        assert_eq!(b.ingest_envelope_json(&env).unwrap(), Message::Heartbeat);

        // Goodbye
        let env = a
            .seal_message(Message::Goodbye(GoodbyeReason::Normal))
            .unwrap();
        assert_eq!(
            b.ingest_envelope_json(&env).unwrap(),
            Message::Goodbye(GoodbyeReason::Normal)
        );

        // Welcome — the seal step stamps the attestation in, so
        // compare structurally rather than by raw equality.
        let welcome = Message::Welcome(WelcomePayload {
            status: WelcomeStatus::Accepted,
            host_identity: a.local_identity().clone(),
            host_clock: a.clock(),
            reject_reason: String::new(),
            kchat_attestation: None,
        });
        let env = a.seal_message(welcome.clone()).unwrap();
        let got = b.ingest_envelope_json(&env).unwrap();
        match (&got, &welcome) {
            (Message::Welcome(g), Message::Welcome(w)) => {
                assert_eq!(g.status, w.status);
                assert_eq!(g.host_identity, w.host_identity);
                assert_eq!(g.host_clock, w.host_clock);
                assert_eq!(g.reject_reason, w.reject_reason);
                assert!(g.kchat_attestation.is_some());
            }
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn trust_rejects_mismatched_peer_id() {
        let project = Uuid::new_v4();
        let mut a = make_session(20, project);
        let real = PeerKey::from_seed([99u8; 32]).identity("real");
        let forged = PeerIdentity {
            peer_id: real.peer_id,
            display_name: "forged".into(),
            // Different public key (taken from a different seed).
            public_key: PeerKey::from_seed([100u8; 32]).identity("x").public_key,
        };
        assert!(matches!(
            a.trust_peer(forged),
            Err(SessionError::PeerIdMismatch)
        ));
    }

    #[test]
    fn trust_caps_at_max_peers() {
        let project = Uuid::new_v4();
        let mut a = ProjectSession::new_with_authority(
            PeerKey::from_seed([99u8; 32]),
            "host",
            project,
            SessionConfig {
                max_peers: 2,
                ..SessionConfig::default()
            },
            [0u8; 8],
            make_authority(99, test_group()),
        );
        for seed in 0..2u8 {
            a.trust_peer(PeerKey::from_seed([seed; 32]).identity("p"))
                .unwrap();
        }
        let extra = PeerKey::from_seed([3u8; 32]).identity("extra");
        let err = a.trust_peer(extra).unwrap_err();
        assert!(matches!(err, SessionError::TooManyPeers { max: 2 }));
    }

    #[test]
    fn each_seal_uses_a_fresh_nonce() {
        let project = Uuid::new_v4();
        let mut a = make_session(40, project);
        let mut seen = std::collections::HashSet::new();
        for _ in 0..1024 {
            let raw = a.seal_message(Message::Heartbeat).unwrap();
            let env: Envelope<Message> = serde_json::from_str(&raw).unwrap();
            let nonce = env.nonce_bytes().unwrap();
            assert!(seen.insert(nonce), "duplicate nonce from same session");
        }
    }

    #[test]
    fn forget_peer_drops_replay_window() {
        let (mut a, mut b, project) = pair();
        let msg = Message::Hello(HelloPayload {
            identity: a.local_identity().clone(),
            project_id: project,
            app_version: "v".into(),
            kchat_attestation: None,
        });
        let env = a.seal_message(msg).unwrap();
        b.ingest_envelope_json(&env).unwrap();
        b.forget_peer(&a.local_identity().peer_id);
        // After forgetting, the same envelope is now untrusted (not
        // replay) — we tested replay above; this tests the cleanup.
        let err = b.ingest_envelope_json(&env).unwrap_err();
        assert!(matches!(err, SessionError::UntrustedPeer(_)), "got {err:?}");
    }
}
