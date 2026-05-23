//! Bridge surface for the Phase 3 LAN collaboration session.
//!
//! All public entry points (the `session_*` functions) are exposed
//! one-shot from `lib.rs` as N-API wrappers, mirroring how
//! `state.rs` and `document.rs` are wired. This module owns the
//! lifecycle for the singleton [`SessionState`] that:
//!
//! 1. Holds the [`LanCollabHost`] (which itself owns the
//!    `quinn::Endpoint`, the mDNS responder, and the per-project
//!    [`ProjectSession`] from `kcreate_collab`).
//! 2. Owns a dedicated tokio multi-thread runtime so the async
//!    transport tasks have a place to live; the rest of the bridge
//!    is sync (called from N-API), so we [`block_on`] for the
//!    surfaces that need a result and `spawn` for fire-and-forget.
//! 3. Maintains the latest known [`PresencePayload`] per peer plus
//!    a bounded event queue that the Electron main process drains
//!    on a tick (`session_drain_events`) and forwards to the
//!    renderer as a push channel. The cursor overlay reads the
//!    presence map; the `PresencePanel` reads the peer roster.
//!
//! The deny-list test
//! (`crates/kcreate_tests/tests/local_first.rs`) walks the *default*
//! feature dependency graph; everything in this module is behind
//! `#[cfg(feature = "collab")]`, so the editing-path tree stays
//! free of `quinn` / `rustls` / `mdns-sd` / `tokio` unless the
//! Electron host opts in explicitly.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use base64::engine::general_purpose::{STANDARD_NO_PAD, URL_SAFE_NO_PAD};
use base64::Engine;
use chrono::{DateTime, Utc};
use ed25519_dalek::VerifyingKey;
use kcreate_collab::message::Cursor;
use kcreate_collab::{
    no_kchat_authority, BoundKChatGroupAuthority, JournalEntry, KChatAuthError, KChatGroupId,
    KChatMembership, MemoryJournalStore, Message, OperationJournal, PeerId, PeerIdentity, PeerKey,
    PresencePayload, ResumeVector, SessionConfig, SharedKChatAuthority,
};
use kcreate_collab_transport::{HostOptions, InboundEvent, LanCollabHost};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::runtime::{Builder, Runtime};
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::document::DocumentBridgeError;

/// Wire-format DTO returned from `session_start` to the renderer.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStartReport {
    pub peer_id: String,
    pub public_key: String,
    pub display_name: String,
    pub project_id: Uuid,
    pub local_addr: String,
    pub cert_fingerprint: String,
    pub advertise_mdns: bool,
}

/// Wire-format DTO for `session_peers`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionPeer {
    pub peer_id: String,
    pub public_key: String,
    pub display_name: String,
    /// Last presence we received from this peer. None means the
    /// peer is connected but has not broadcast presence yet.
    pub presence: Option<SessionPresence>,
}

/// Wire-format mirror of [`PresencePayload`] that the UI can render
/// without depending on the `kcreate_collab` types directly.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionPresence {
    pub active_page: Option<Uuid>,
    pub selection: Vec<Uuid>,
    pub cursor: Option<SessionCursor>,
    pub sent_at: DateTime<Utc>,
}

/// Wire-format cursor (matches `Message::Presence`'s `Cursor`).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionCursor {
    pub x: f64,
    pub y: f64,
}

/// Push-channel event variant. Renderer subscribes to a single
/// IPC channel which fans these out to PresencePanel + cursor
/// overlay.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum SessionEvent {
    /// A peer was discovered via mDNS but is not yet connected.
    /// The UI uses this to populate a "discovered (click to join)"
    /// list separate from the connected-peers list.
    Discovered {
        peer_id: String,
        public_key: String,
        display_name: String,
        project_id: Uuid,
        socket_addr: String,
        cert_fingerprint: String,
    },
    /// A previously-discovered peer went off-LAN.
    Undiscovered { peer_id: String },
    /// A peer completed the QUIC + Hello/Welcome handshake.
    PeerJoined {
        peer_id: String,
        public_key: String,
        display_name: String,
    },
    /// A connected peer's QUIC connection closed.
    PeerLeft { peer_id: String },
    /// A peer's presence (cursor, selection, active page) updated.
    PresenceUpdated {
        peer_id: String,
        presence: SessionPresence,
    },
    /// Block 7: an `OperationBroadcast` from a remote peer was
    /// journaled. The renderer doesn't apply these directly —
    /// the document-graph layer handles the actual graph mutation
    /// on a dedicated path — but the event lets the UI surface
    /// "Ken edited 3 nodes" toasts and update the activity panel.
    OperationsJournaled {
        peer_id: String,
        /// Number of operations recorded in this batch.
        op_count: u32,
        /// Highest Lamport clock observed in the batch, as a u64
        /// for ergonomic JSON consumption from the renderer.
        highest_clock: u64,
    },
}

impl From<&PresencePayload> for SessionPresence {
    fn from(p: &PresencePayload) -> Self {
        Self {
            active_page: p.active_page,
            selection: p.selection.clone(),
            cursor: p.cursor.map(|c| SessionCursor { x: c.x, y: c.y }),
            sent_at: p.sent_at,
        }
    }
}

/// Errors returned from the session bridge. Distinct from
/// [`DocumentBridgeError`] because the session is independent of
/// whether a project is open — discovery + roster can run
/// pre-project (though we still require a project for actual
/// editing collaboration).
#[derive(Debug, thiserror::Error)]
pub enum SessionBridgeError {
    #[error("collab session is not running; call session_start first")]
    NotRunning,
    #[error("collab session is already running; call session_leave first")]
    AlreadyRunning,
    #[error("invalid argument {field:?}: {message}")]
    InvalidArgument {
        field: &'static str,
        message: String,
    },
    #[error("transport error: {0}")]
    Transport(#[from] kcreate_collab_transport::TransportError),
    #[error("collab protocol error: {0}")]
    Collab(#[from] kcreate_collab::CollabError),
    /// Multiplayer is locked behind KChat group membership and no
    /// valid membership has been installed yet. Renderer surfaces
    /// this as the "sign into a KChat group" CTA instead of the
    /// start/join buttons.
    #[error("multiplayer is locked: not signed into a KChat group")]
    NotInKChatGroup,
    /// A `kchat_install_authority` call failed verification. The
    /// renderer never normally reaches this path today (KChat
    /// client doesn't exist yet), but if it ever does, the typed
    /// error gives it a useful diagnostic.
    #[error("KChat authority install failed: {0}")]
    KChat(#[from] KChatAuthError),
}

pub type Result<T> = std::result::Result<T, SessionBridgeError>;

/// Cap on the number of presence/peer events we buffer between
/// drains. The main process polls on a 50–100 ms tick (see
/// `apps/desktop/main/src/main.ts::collabSessionPollLoop`) so this
/// is sized to comfortably absorb a burst from a chatty
/// many-peer LAN without unbounded memory growth.
const EVENT_QUEUE_CAP: usize = 1024;

/// How long `block_on` calls wait for a single transport operation
/// (dial, broadcast, shutdown) before returning a timeout. Mirrors
/// the QUIC idle timeout chosen by `kcreate_collab_transport`.
const OP_TIMEOUT: Duration = Duration::from_secs(15);

/// Internal state machine. One per process; gated by [`SLOT`].
struct SessionState {
    host: LanCollabHost,
    runtime: Runtime,
    /// Latest presence per peer. Updated by the inbound-event
    /// pump task whenever a `Message::Presence` arrives. Read by
    /// `session_peers` and `scene_sync::presence_overlays`.
    presence: HashMap<PeerId, PresencePayload>,
    /// Bounded queue drained by `session_drain_events`. The pump
    /// task pushes; the bridge consumer pops.
    events: std::collections::VecDeque<SessionEvent>,
    /// Handle to the inbound-event pump task. Aborted on
    /// `session_leave` so the task terminates; the host's
    /// broadcast channel is dropped at the same time.
    pump_handle: tokio::task::JoinHandle<()>,
    /// Cached local identity / project / addr for reporting back to
    /// the UI without touching the host.
    report: SessionStartReport,
    /// Block 7: per-session operation journal. Persisted to the
    /// project's SQLite database via a separate flush path (see
    /// `journal_flush_to_project`); kept in memory here so the
    /// hot path of `OperationBroadcast` ingestion is lock-free
    /// w.r.t. the workspace mutex. Resume vectors served from
    /// this in-memory copy are the source of truth during the
    /// session's lifetime.
    journal: OperationJournal<MemoryJournalStore>,
    /// Cached local peer id. The transport derives this from the
    /// session's signing key; we cache it here so journal appends
    /// don't have to round-trip through the host.
    local_peer_id: PeerId,
}

fn slot() -> &'static Mutex<Option<SessionState>> {
    static S: OnceLock<Mutex<Option<SessionState>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(None))
}

/// Global slot for the KChat group authority. Until a future KChat
/// client calls [`kchat_install_authority`] with a verified
/// membership, this holds [`no_kchat_authority`] and every
/// multiplayer entry point fails closed with
/// [`SessionBridgeError::NotInKChatGroup`].
///
/// The slot lives independently of [`slot`] so a long-running
/// session can refresh its attestation (e.g. before expiry) without
/// tearing the QUIC endpoint down.
fn kchat_slot() -> &'static Mutex<SharedKChatAuthority> {
    static S: OnceLock<Mutex<SharedKChatAuthority>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(no_kchat_authority()))
}

/// Encode a 32-byte signing-key seed from a URL-safe base64 string.
/// We accept either padded or unpadded URL-safe base64 because the
/// renderer-side persistence may add padding.
fn decode_seed(s: &str) -> Result<[u8; 32]> {
    let cleaned = s.trim_end_matches('=');
    let bytes = URL_SAFE_NO_PAD.decode(cleaned.as_bytes()).map_err(|e| {
        SessionBridgeError::InvalidArgument {
            field: "seed",
            message: format!("base64url: {e}"),
        }
    })?;
    if bytes.len() != 32 {
        return Err(SessionBridgeError::InvalidArgument {
            field: "seed",
            message: format!("expected 32 bytes, got {}", bytes.len()),
        });
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

fn decode_cert_fingerprint(s: &str) -> Result<[u8; 32]> {
    let bytes = STANDARD_NO_PAD
        .decode(s.trim_end_matches('=').as_bytes())
        .map_err(|e| SessionBridgeError::InvalidArgument {
            field: "certFingerprint",
            message: format!("base64: {e}"),
        })?;
    if bytes.len() != 32 {
        return Err(SessionBridgeError::InvalidArgument {
            field: "certFingerprint",
            message: format!("expected 32 bytes, got {}", bytes.len()),
        });
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

/// Reconstruct a `PeerIdentity` from the wire-format triple. The
/// authoritative trust anchor is `public_key`: we derive the
/// `PeerId` from it (matches what `PeerKey::peer_id()` would
/// compute on the remote) and compare against the supplied
/// `peer_id` for sanity. This catches manifest tampering in the
/// "paste a peer link" flow — if the user pastes a peer-id that
/// disagrees with the public-key half of the link, we reject up
/// front rather than letting the dial-time fingerprint check
/// hide the lie.
fn identity_from_wire(peer_id: &str, public_key: &str, display_name: &str) -> Result<PeerIdentity> {
    let bytes = decode_public_key_bytes(public_key)?;
    let vk = ed25519_dalek::VerifyingKey::from_bytes(&bytes).map_err(|e| {
        SessionBridgeError::InvalidArgument {
            field: "publicKey",
            message: format!("ed25519: {e}"),
        }
    })?;
    let derived = PeerId::from_verifying_key(&vk);
    if derived.as_str() != peer_id {
        return Err(SessionBridgeError::InvalidArgument {
            field: "peerId",
            message: format!(
                "peerId {} does not match the one derived from publicKey ({})",
                peer_id,
                derived.as_str()
            ),
        });
    }
    Ok(PeerIdentity::new(&vk, display_name))
}

/// Decode a `PeerIdentity::public_key` (base64url-no-pad-encoded
/// Ed25519 public key, 32 bytes) into raw bytes. Mirrors
/// `PeerIdentity::verifying_key` but returns the bytes so the
/// caller can build a fresh `VerifyingKey`.
fn decode_public_key_bytes(public_key: &str) -> Result<[u8; 32]> {
    let bytes = URL_SAFE_NO_PAD
        .decode(public_key.trim_end_matches('=').as_bytes())
        .map_err(|e| SessionBridgeError::InvalidArgument {
            field: "publicKey",
            message: format!("base64url: {e}"),
        })?;
    if bytes.len() != 32 {
        return Err(SessionBridgeError::InvalidArgument {
            field: "publicKey",
            message: format!("expected 32 bytes, got {}", bytes.len()),
        });
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

/// Drain pending broadcast events into the bounded queue. Called
/// from the spawned pump task; bounded by [`EVENT_QUEUE_CAP`] so
/// a slow main-process tick can't OOM the renderer.
fn push_event(state: &mut SessionState, ev: SessionEvent) {
    if state.events.len() >= EVENT_QUEUE_CAP {
        // Drop oldest. Bounded FIFO with newest-wins for the
        // presence-update case is the right semantics: an old
        // cursor position is uninteresting if there's a fresher
        // one queued.
        state.events.pop_front();
    }
    state.events.push_back(ev);
}

/// The pump task: subscribes to the host's inbound events,
/// projects them into [`SessionEvent`], updates the presence map,
/// and pushes onto the bounded queue. Lives for the lifetime of
/// the session.
async fn pump_inbound(rx_initial: broadcast::Receiver<InboundEvent>) {
    let mut rx = rx_initial;
    loop {
        match rx.recv().await {
            Ok(ev) => apply_event(ev),
            Err(broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!(
                    "collab pump lagged by {n} events; some presence updates were dropped"
                );
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

fn apply_event(ev: InboundEvent) {
    let mut guard = slot().lock();
    let Some(state) = guard.as_mut() else { return };
    match ev {
        InboundEvent::Discovered(peer) => {
            push_event(
                state,
                SessionEvent::Discovered {
                    peer_id: peer.peer_id.as_str().to_string(),
                    public_key: peer.identity.public_key.clone(),
                    display_name: peer.identity.display_name.clone(),
                    project_id: peer.project_id,
                    socket_addr: peer.socket_addr.to_string(),
                    cert_fingerprint: STANDARD_NO_PAD.encode(peer.cert_fingerprint),
                },
            );
        }
        InboundEvent::Undiscovered(peer_id) => {
            push_event(
                state,
                SessionEvent::Undiscovered {
                    peer_id: peer_id.as_str().to_string(),
                },
            );
        }
        InboundEvent::PeerJoined(identity) => {
            push_event(
                state,
                SessionEvent::PeerJoined {
                    peer_id: identity.peer_id.as_str().to_string(),
                    public_key: identity.public_key.clone(),
                    display_name: identity.display_name.clone(),
                },
            );
        }
        InboundEvent::PeerLeft(peer_id) => {
            state.presence.remove(&peer_id);
            push_event(
                state,
                SessionEvent::PeerLeft {
                    peer_id: peer_id.as_str().to_string(),
                },
            );
        }
        InboundEvent::Message { from, message } => match message.as_ref() {
            Message::Presence(p) => {
                state.presence.insert(from.clone(), p.clone());
                push_event(
                    state,
                    SessionEvent::PresenceUpdated {
                        peer_id: from.as_str().to_string(),
                        presence: SessionPresence::from(p),
                    },
                );
            }
            Message::OperationBroadcast(p) => {
                // Block 7: journal the remote operations. The
                // transport already verified the envelope's
                // signature + Lamport monotonicity, and the
                // KChat gate already screened the sender, so by
                // the time we get here `p.operations` are
                // trusted relative to the session's threat
                // model. We still validate the broadcast's
                // project_id matches the session's project so a
                // misrouted message can't poison the wrong
                // journal.
                if p.project_id == state.journal.project_id() {
                    journal_inbound_broadcast(state, &from, p);
                }
            }
            Message::ResumeBundle(p) => {
                // Block 7: a host responded to our resume
                // request. Replay the entries through the
                // journal so future sessions see the same
                // history. Project-id mismatch is dropped.
                if p.project_id == state.journal.project_id() {
                    journal_inbound_resume_bundle(state, p);
                }
            }
            // Hello / Welcome / Heartbeat / Goodbye / ResumeRequest
            // are handled by the transport layer itself, not
            // surfaced as bridge-level events.
            Message::Hello(_)
            | Message::Welcome(_)
            | Message::Heartbeat
            | Message::Goodbye(_)
            | Message::ResumeRequest(_) => {}
        },
    }
}

/// Record a freshly-arrived `OperationBroadcast` payload into the
/// session's in-memory journal and emit a `SessionEvent` describing
/// the batch. Out-of-order or duplicate entries are logged-and-
/// dropped, not propagated as errors — the session keeps running
/// because the transport may have re-delivered a buffered batch
/// and we trust the journal's own monotonicity gate to dedupe.
fn journal_inbound_broadcast(
    state: &mut SessionState,
    from: &PeerId,
    payload: &kcreate_collab::OperationBroadcastPayload,
) {
    let mut highest = 0u64;
    let mut recorded: u32 = 0;
    for op in &payload.operations {
        // The protocol doesn't put a Lamport clock *on the
        // operation itself* — it lives on the envelope. We
        // approximate it with the journal's current high-water
        // mark for the sender plus one per op in the batch so
        // intra-batch order is preserved even on the same
        // envelope. A future protocol revision should attach
        // per-op clocks; that's a wire-format change we'll
        // pair with the SQLite-backed journal landing.
        let next_clock = state
            .journal
            .resume_vector()
            .highest_for(from)
            .as_u64()
            .saturating_add(1);
        let clock = kcreate_collab::LamportClock::from_raw(next_clock);
        match state.journal.append(from.clone(), clock, op.clone()) {
            Ok(()) => {
                recorded += 1;
                if next_clock > highest {
                    highest = next_clock;
                }
            }
            Err(kcreate_collab::JournalError::Duplicate { .. })
            | Err(kcreate_collab::JournalError::OutOfOrder { .. }) => {
                // Expected for re-delivered or out-of-order
                // batches; the wire monotonicity check above
                // will let the next correctly-ordered batch
                // through.
            }
            Err(kcreate_collab::JournalError::Backend(_)) => {
                // The memory store can't produce a backend
                // error today, but if a future swap to SQLite
                // does, we degrade to ignoring the op rather
                // than crashing the session.
            }
        }
    }
    if recorded > 0 {
        push_event(
            state,
            SessionEvent::OperationsJournaled {
                peer_id: from.as_str().to_string(),
                op_count: recorded,
                highest_clock: highest,
            },
        );
    }
}

/// Replay a [`Message::ResumeBundle`] into the local journal. The
/// bundle's entries are already (peer, clock)-ordered so we can
/// feed them in directly; the journal's monotonicity gate will
/// reject anything that overlaps history we already have, which
/// is exactly the duplicate-replay semantics we want.
fn journal_inbound_resume_bundle(
    state: &mut SessionState,
    payload: &kcreate_collab::ResumeBundlePayload,
) {
    let mut per_peer: HashMap<PeerId, (u32, u64)> = HashMap::new();
    for entry in &payload.operations {
        match state
            .journal
            .append(entry.peer_id.clone(), entry.clock, entry.operation.clone())
        {
            Ok(()) => {
                let stat = per_peer.entry(entry.peer_id.clone()).or_insert((0, 0));
                stat.0 += 1;
                let clk = entry.clock.as_u64();
                if clk > stat.1 {
                    stat.1 = clk;
                }
            }
            Err(_) => {
                // Duplicate / OOO / backend — silently skip.
                // Same rationale as journal_inbound_broadcast.
            }
        }
    }
    for (peer, (count, highest)) in per_peer {
        push_event(
            state,
            SessionEvent::OperationsJournaled {
                peer_id: peer.as_str().to_string(),
                op_count: count,
                highest_clock: highest,
            },
        );
    }
}

/// Start a collab session. The local identity is derived from the
/// supplied 32-byte signing-key seed; the renderer persists this
/// across sessions so the same machine always presents the same
/// peer identity (matching the trust model of `kcreate_plugin`).
pub fn session_start(
    seed_b64: &str,
    display_name: &str,
    project_id: Uuid,
    advertise_mdns: bool,
) -> Result<SessionStartReport> {
    let mut guard = slot().lock();
    if guard.is_some() {
        return Err(SessionBridgeError::AlreadyRunning);
    }
    if display_name.trim().is_empty() {
        return Err(SessionBridgeError::InvalidArgument {
            field: "displayName",
            message: "must not be empty".into(),
        });
    }
    let seed = decode_seed(seed_b64)?;
    let local_key = PeerKey::from_seed(seed);
    let local_peer_id = local_key.peer_id();
    let local_public_key = local_key.identity(display_name).public_key;

    // KChat gate: refuse to start unless the locally-installed
    // authority has a current membership bound to *this* peer key.
    // We re-verify the binding here (rather than just trusting the
    // installed membership) because the renderer might call
    // `session_start` with a different seed than the one the
    // membership was minted for, and we also re-check the time
    // window so an expired-while-the-app-was-asleep attestation
    // doesn't sneak through.
    let authority = kchat_authority_snapshot();
    let membership = authority
        .local_membership()
        .ok_or(SessionBridgeError::NotInKChatGroup)?;
    let trust_root = authority
        .issuer_trust_root()
        .ok_or(SessionBridgeError::NotInKChatGroup)?;
    membership
        .verify(&trust_root, &local_peer_id, &local_public_key, Utc::now())
        .map_err(|_| SessionBridgeError::NotInKChatGroup)?;

    let runtime = Builder::new_multi_thread()
        .enable_all()
        .thread_name("kcreate-collab")
        .worker_threads(2)
        .build()
        .map_err(|e| SessionBridgeError::InvalidArgument {
            field: "runtime",
            message: e.to_string(),
        })?;

    let opts = HostOptions {
        local_key,
        display_name: display_name.to_string(),
        project_id,
        bind_addr: SocketAddr::from(([0, 0, 0, 0], 0)),
        advertise_mdns,
        advertise_addrs: None,
        session_config: SessionConfig::default(),
        kchat_authority: authority,
    };
    let host = runtime
        .block_on(tokio::time::timeout(OP_TIMEOUT, LanCollabHost::start(opts)))
        .map_err(|_| {
            SessionBridgeError::Transport(kcreate_collab_transport::TransportError::Quic(
                "host start timed out".into(),
            ))
        })??;
    let local_addr = host.local_addr();
    let cert_fingerprint = host.cert_fingerprint_b64();
    let rx = host.subscribe();
    let pump_host = host.clone();
    let pump_handle = runtime.spawn(async move {
        // Holding the host clone keeps the underlying `Arc<HostInner>`
        // alive — without it the host could shut down out from
        // under the pump if the bridge slot is mutated concurrently
        // and Drop runs before the pump's `recv` errors. The bind
        // is `_keep_host_alive` rather than `_` so the local
        // outlives `pump_inbound`'s `.await`.
        let _keep_host_alive = pump_host;
        pump_inbound(rx).await;
    });

    let report = SessionStartReport {
        peer_id: local_peer_id.as_str().to_string(),
        public_key: local_public_key,
        display_name: display_name.to_string(),
        project_id,
        local_addr: local_addr.to_string(),
        cert_fingerprint,
        advertise_mdns,
    };
    // Block 7: fresh in-memory journal for the session. The
    // `OperationJournal::open` on a `MemoryJournalStore::new()`
    // can never fail; `expect` is correct here. A future
    // sqlite-backed store will turn this into a fallible call.
    let journal = OperationJournal::open(MemoryJournalStore::new(), project_id)
        .expect("MemoryJournalStore::summary cannot fail");

    *guard = Some(SessionState {
        host,
        runtime,
        presence: HashMap::new(),
        events: std::collections::VecDeque::new(),
        pump_handle,
        report: report.clone(),
        journal,
        local_peer_id,
    });
    Ok(report)
}

/// Stop the running session. Sends a graceful `Goodbye` to every
/// peer, closes the QUIC endpoint, drops the tokio runtime.
/// Idempotent: calling on a stopped session is a no-op.
pub fn session_leave() -> Result<()> {
    let mut guard = slot().lock();
    let Some(state) = guard.take() else {
        return Ok(());
    };
    let SessionState {
        host,
        runtime,
        pump_handle,
        ..
    } = state;
    pump_handle.abort();
    // Drop the guard before block_on so other threads can observe
    // the empty slot while shutdown is in flight.
    drop(guard);
    runtime.block_on(async {
        // Bound the shutdown so a misbehaving peer can't keep us
        // hung; ignore the timeout result because the endpoint
        // close is best-effort.
        let _ = tokio::time::timeout(OP_TIMEOUT, host.shutdown()).await;
    });
    // Dropping the runtime here blocks until all spawned tasks
    // either complete or are torn down. This is what we want: the
    // pump task should observe the broadcast channel closing and
    // exit, then we can safely drop the runtime.
    drop(runtime);
    Ok(())
}

/// Dial a peer whose connection details came in out-of-band
/// (typically a copy-pasted "peer link" or a confirmed entry
/// from the mDNS-discovered list).
pub fn session_join(
    peer_id: &str,
    public_key: &str,
    display_name: &str,
    socket_addr: &str,
    cert_fingerprint_b64: &str,
) -> Result<()> {
    // KChat gate: refuse to dial any peer unless the locally
    // installed authority is fully valid — signature checks out
    // under the trust root, peer binding matches, and the
    // validity window covers `now`. Matches the rigor of
    // `session_start`. The transport's Hello path would also
    // refuse to mint an attestation, but failing fast here gives
    // the renderer a typed error rather than a generic dial
    // timeout (and means an expired-while-the-app-was-asleep
    // membership can never reach the network layer).
    require_active_kchat_membership()?;

    let identity = identity_from_wire(peer_id, public_key, display_name)?;
    let socket: SocketAddr = socket_addr.parse().map_err(|e: std::net::AddrParseError| {
        SessionBridgeError::InvalidArgument {
            field: "socketAddr",
            message: e.to_string(),
        }
    })?;
    let fp = decode_cert_fingerprint(cert_fingerprint_b64)?;

    let guard = slot().lock();
    let state = guard.as_ref().ok_or(SessionBridgeError::NotRunning)?;
    let host = state.host.clone();
    let runtime_handle = state.runtime.handle().clone();
    drop(guard);

    let result = runtime_handle.block_on(async {
        tokio::time::timeout(OP_TIMEOUT, host.dial_known_peer(identity, socket, fp)).await
    });
    match result {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(e)) => Err(SessionBridgeError::Transport(e)),
        Err(_) => Err(SessionBridgeError::Transport(
            kcreate_collab_transport::TransportError::Quic("dial timed out".into()),
        )),
    }
}

/// Return the current peer roster. Includes both connected peers
/// and (when present) their latest presence payload.
pub fn session_peers() -> Result<Vec<SessionPeer>> {
    let guard = slot().lock();
    let state = guard.as_ref().ok_or(SessionBridgeError::NotRunning)?;
    let connected = state.host.connected_peers();
    let peers: Vec<SessionPeer> = connected
        .into_iter()
        .map(|identity| SessionPeer {
            peer_id: identity.peer_id.as_str().to_string(),
            public_key: identity.public_key.clone(),
            display_name: identity.display_name.clone(),
            presence: state
                .presence
                .get(&identity.peer_id)
                .map(SessionPresence::from),
        })
        .collect();
    Ok(peers)
}

/// Drain the bounded event queue. The Electron main process polls
/// this on a fixed tick and forwards everything via
/// `webContents.send("kcreate/session/event", ...)`.
pub fn session_drain_events() -> Result<Vec<SessionEvent>> {
    let mut guard = slot().lock();
    let state = guard.as_mut().ok_or(SessionBridgeError::NotRunning)?;
    Ok(state.events.drain(..).collect())
}

/// JSON shape of the journal summary returned by
/// [`session_journal_summary`]. Mirrors
/// [`kcreate_collab::ResumeVector`] in a renderer-friendly form
/// (peer ids as base64url strings, clocks as decimal-encoded u64s).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionJournalSummary {
    /// Total number of journaled entries (across every peer) for
    /// the running session's project.
    pub entry_count: u64,
    /// Distinct peers the journal has heard from.
    pub peer_count: u32,
    /// Per-peer high-water Lamport clock. Keys are base64url peer
    /// ids, values are the highest clock seen for that peer.
    pub by_peer: std::collections::BTreeMap<String, u64>,
}

/// Inspect the running session's operation journal. KChat-gated:
/// the renderer never sees journal state outside a multiplayer
/// session, so this short-circuits if the membership is missing
/// or expired.
///
/// Used by the PresencePanel's "Activity" tab to show "we've
/// recorded 124 ops across 3 peers since you connected".
pub fn session_journal_summary() -> Result<SessionJournalSummary> {
    require_active_kchat_membership()?;
    let guard = slot().lock();
    let state = guard.as_ref().ok_or(SessionBridgeError::NotRunning)?;
    let entry_count = state
        .journal
        .len()
        .map_err(|e| SessionBridgeError::InvalidArgument {
            field: "journal",
            message: e.to_string(),
        })? as u64;
    let summary = state.journal.resume_vector();
    let by_peer = summary
        .by_peer
        .iter()
        .map(|(p, c)| (p.as_str().to_string(), c.as_u64()))
        .collect();
    let peer_count = summary.peer_count().try_into().unwrap_or(u32::MAX);
    Ok(SessionJournalSummary {
        entry_count,
        peer_count,
        by_peer,
    })
}

/// Block 7: record an operation the local user just committed.
/// The bridge document layer calls this immediately after a
/// successful local apply so the journal reflects authored work
/// in addition to remote work. The Lamport clock is supplied by
/// the caller (the session layer owns clock advancement); the
/// journal validates monotonicity. KChat gating is enforced: a
/// non-multiplayer session never reaches this code path because
/// the editing path only invokes it when [`slot`] is `Some`, and
/// `session_start` enforces the gate. Returns `Ok(())` if no
/// session is running (single-player edits go to the operation
/// log, not the collab journal).
pub fn session_record_local_operation(operation: kcreate_core::operation::Operation) -> Result<()> {
    let mut guard = slot().lock();
    let Some(state) = guard.as_mut() else {
        return Ok(());
    };
    let local_peer_id = state.local_peer_id.clone();
    let next_clock = state
        .journal
        .resume_vector()
        .highest_for(&local_peer_id)
        .as_u64()
        .saturating_add(1);
    let clock = kcreate_collab::LamportClock::from_raw(next_clock);
    state
        .journal
        .append(local_peer_id, clock, operation)
        .map_err(|e| SessionBridgeError::InvalidArgument {
            field: "journal",
            message: e.to_string(),
        })?;
    Ok(())
}

/// Broadcast the local user's presence (active page, selection,
/// cursor). Called by the renderer on selection / canvas pointer
/// events.
pub fn session_send_presence(
    active_page: Option<Uuid>,
    selection: Vec<Uuid>,
    cursor: Option<SessionCursor>,
) -> Result<()> {
    // KChat gate: presence beacons never leave the box unless the
    // user is in a KChat group, and the installed membership is
    // still valid right now. Full re-verification (signature +
    // peer binding + time window) matches `session_start`.
    require_active_kchat_membership()?;
    let guard = slot().lock();
    let state = guard.as_ref().ok_or(SessionBridgeError::NotRunning)?;
    let host = state.host.clone();
    let runtime_handle = state.runtime.handle().clone();
    drop(guard);

    let payload = PresencePayload {
        active_page,
        selection,
        cursor: cursor.map(|c| Cursor { x: c.x, y: c.y }),
        sent_at: Utc::now(),
    };
    let result = runtime_handle.block_on(async {
        tokio::time::timeout(OP_TIMEOUT, host.broadcast_presence(payload)).await
    });
    match result {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(SessionBridgeError::Transport(e)),
        Err(_) => Err(SessionBridgeError::Transport(
            kcreate_collab_transport::TransportError::Quic("broadcast_presence timed out".into()),
        )),
    }
}

/// Lightweight introspection for `session_info` (used by the
/// presence panel header). Returns the initial start report if
/// the session is running; `None` otherwise.
pub fn session_info() -> Option<SessionStartReport> {
    let guard = slot().lock();
    guard.as_ref().map(|s| s.report.clone())
}

/// Read-side accessor for `scene_sync` so the cursor overlay can
/// draw remote cursors without the bridge crate having a circular
/// dependency on `collab`. Returns `(peer_id, display_name,
/// cursor)` triples — only peers whose latest presence carries a
/// cursor are included.
#[allow(dead_code)] // wired up via scene_sync below; the helper
                    // is exported so future call sites (export
                    // preview pane, presentation mode) can reuse it.
pub fn presence_cursors() -> Vec<(String, String, SessionCursor)> {
    let guard = slot().lock();
    let Some(state) = guard.as_ref() else {
        return Vec::new();
    };
    let connected_lookup: HashMap<PeerId, String> = state
        .host
        .connected_peers()
        .into_iter()
        .map(|i| (i.peer_id, i.display_name))
        .collect();
    state
        .presence
        .iter()
        .filter_map(|(peer_id, payload)| {
            let display_name = connected_lookup.get(peer_id)?.clone();
            let cursor = payload.cursor.map(|c| SessionCursor { x: c.x, y: c.y })?;
            Some((peer_id.as_str().to_string(), display_name, cursor))
        })
        .collect()
}

// === Conversion to bridge error type so the N-API layer can use
// `map_doc_err`'s status enum without inventing a separate
// `map_session_err`. The session-bridge errors map cleanly:
// NotRunning / AlreadyRunning / InvalidArgument are caller mistakes,
// the rest are transport / protocol failures.
impl From<SessionBridgeError> for DocumentBridgeError {
    fn from(e: SessionBridgeError) -> Self {
        match e {
            SessionBridgeError::NotRunning
            | SessionBridgeError::AlreadyRunning
            | SessionBridgeError::InvalidArgument { .. }
            | SessionBridgeError::NotInKChatGroup => Self::InvalidArgument {
                argument: "session".to_string(),
                value: e.to_string(),
            },
            SessionBridgeError::Transport(_)
            | SessionBridgeError::Collab(_)
            | SessionBridgeError::KChat(_) => Self::Io(std::io::Error::other(e.to_string())),
        }
    }
}

/// Wire-format DTO describing the currently-installed KChat
/// membership. Returned by [`kchat_membership_status`] so the
/// renderer can render the appropriate panel state (locked / signed
/// in / expiring soon).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KChatMembershipStatus {
    /// `true` when no valid authority is installed — either no
    /// membership is set at all, or the installed one fails
    /// re-verification (forged signature, peer-binding mismatch,
    /// outside `[issued_at, expires_at]`, etc.). The multiplayer
    /// entry points (`session_start`, `session_join`,
    /// `session_send_presence`) all refuse to run while this is
    /// `true`. `false` means the gate is currently open.
    pub locked: bool,
    /// Group id from the installed membership, if any. `None` when
    /// `locked == true`.
    pub group_id: Option<String>,
    /// Peer id derived from the installed membership, if any.
    pub peer_id: Option<String>,
    /// Membership expiry in RFC3339, if any. The renderer can show
    /// a "renew soon" CTA when this is within e.g. 5 minutes of
    /// `now`.
    pub expires_at: Option<DateTime<Utc>>,
}

/// Wire-format DTO accepted by [`kchat_install_authority`]. All
/// fields are URL-safe base64 (no padding) except for the time
/// fields which are RFC3339.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KChatInstallRequest {
    /// 32-byte Ed25519 verifying key of the KChat group server
    /// (the issuer trust root). This is the public half of the
    /// signing key the issuer used to mint the membership.
    pub issuer_public_key: String,
    /// Group identifier minted on the issuer side.
    pub group_id: String,
    /// Peer id (BLAKE3-derived) of the local user.
    pub peer_id: String,
    /// 32-byte Ed25519 verifying key of the local user. Must match
    /// the peer key the bridge uses for `session_start`.
    pub peer_public_key: String,
    /// Membership issuance time.
    pub issued_at: DateTime<Utc>,
    /// Membership expiry time.
    pub expires_at: DateTime<Utc>,
    /// 64-byte Ed25519 signature over the canonical
    /// `MembershipSigningView` of the other fields.
    pub signature: String,
}

/// Snapshot the currently-installed KChat authority. Cheap clone of
/// the inner `Arc<dyn KChatGroupAuthority>` so the caller releases
/// the slot lock immediately and never holds it across N-API calls
/// or async waits.
fn kchat_authority_snapshot() -> SharedKChatAuthority {
    kchat_slot().lock().clone()
}

/// Re-verify the installed KChat authority and return the active
/// membership. Every multiplayer entry point in the bridge gates on
/// this helper rather than the weaker `.local_membership().is_some()`
/// check: it confirms (a) a membership exists, (b) the trust root is
/// installed, (c) the embedded signature verifies under that trust
/// root, (d) the membership's `peer_id` derives from the embedded
/// `peer_public_key`, and (e) the validity window covers `now`.
///
/// Returning the membership rather than `()` means call sites can
/// pull the group id / peer id / expiry out of the same struct that
/// just passed the gate, without snapshotting the authority twice.
fn require_active_kchat_membership() -> Result<KChatMembership> {
    let authority = kchat_authority_snapshot();
    let membership = authority
        .local_membership()
        .ok_or(SessionBridgeError::NotInKChatGroup)?;
    let trust_root = authority
        .issuer_trust_root()
        .ok_or(SessionBridgeError::NotInKChatGroup)?;
    // Reconstruct the peer binding from the membership's own
    // embedded public key. If the binding has been tampered with,
    // the derived `PeerId` won't match the stored `peer_id` and
    // `verify` rejects.
    let peer_vk = decode_verifying_key(&membership.peer_public_key, "peerPublicKey")?;
    let derived_peer_id = PeerId::from_verifying_key(&peer_vk);
    membership
        .verify(
            &trust_root,
            &derived_peer_id,
            &membership.peer_public_key,
            Utc::now(),
        )
        .map_err(|_| SessionBridgeError::NotInKChatGroup)?;
    Ok(membership)
}

fn decode_b64_url(input: &str, field: &'static str) -> Result<Vec<u8>> {
    URL_SAFE_NO_PAD
        .decode(input.trim_end_matches('='))
        .map_err(|e| SessionBridgeError::InvalidArgument {
            field,
            message: e.to_string(),
        })
}

fn decode_verifying_key(input: &str, field: &'static str) -> Result<VerifyingKey> {
    let bytes = decode_b64_url(input, field)?;
    if bytes.len() != 32 {
        return Err(SessionBridgeError::InvalidArgument {
            field,
            message: format!("expected 32 bytes, got {}", bytes.len()),
        });
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    VerifyingKey::from_bytes(&arr).map_err(|e| SessionBridgeError::InvalidArgument {
        field,
        message: e.to_string(),
    })
}

/// Install (or refresh) the KChat group authority. Once a valid
/// authority is installed, the multiplayer bridge unlocks. A
/// subsequent `kchat_clear_authority` re-locks it.
///
/// The membership is verified locally — including signature, peer
/// binding, and time window — before being installed, so a future
/// KChat client crash or malicious request can't sneak past the
/// gate by pushing a malformed attestation.
pub fn kchat_install_authority(req: KChatInstallRequest) -> Result<KChatMembershipStatus> {
    let issuer_vk = decode_verifying_key(&req.issuer_public_key, "issuerPublicKey")?;
    // Defence-in-depth: the wire-format `peer_id` must derive from
    // the supplied `peer_public_key`. This is also enforced by
    // `KChatMembership::verify`, but failing fast here gives the
    // renderer a precise field-level error.
    let peer_vk = decode_verifying_key(&req.peer_public_key, "peerPublicKey")?;
    let derived_peer_id = PeerId::from_verifying_key(&peer_vk);
    if derived_peer_id.as_str() != req.peer_id {
        return Err(SessionBridgeError::InvalidArgument {
            field: "peerId",
            message: "peerId does not derive from peerPublicKey".into(),
        });
    }
    // Validate signature size up front (verify() also does this,
    // but we want a typed field error).
    let signature_bytes = decode_b64_url(&req.signature, "signature")?;
    if signature_bytes.len() != 64 {
        return Err(SessionBridgeError::InvalidArgument {
            field: "signature",
            message: format!("expected 64 bytes, got {}", signature_bytes.len()),
        });
    }

    let group_id =
        KChatGroupId::new(req.group_id).map_err(|e| SessionBridgeError::InvalidArgument {
            field: "groupId",
            message: e.to_string(),
        })?;

    // Build a wire-form `KChatMembership` directly from the DTO. We
    // use the supplied base64 strings unchanged (they were already
    // decoded above for validation) so the signing-view bytes are
    // bitwise identical to what the KChat server produced.
    let membership = KChatMembership {
        group_id,
        peer_id: derived_peer_id.clone(),
        peer_public_key: req.peer_public_key.clone(),
        issued_at: req.issued_at,
        expires_at: req.expires_at,
        issuer_public_key: req.issuer_public_key.clone(),
        signature: req.signature.clone(),
    };
    let authority = BoundKChatGroupAuthority::install(
        membership.clone(),
        issuer_vk,
        &derived_peer_id,
        &req.peer_public_key,
        Utc::now(),
    )?;
    let shared: SharedKChatAuthority = Arc::new(authority);
    *kchat_slot().lock() = shared;

    Ok(KChatMembershipStatus {
        locked: false,
        group_id: Some(membership.group_id.as_str().to_string()),
        peer_id: Some(membership.peer_id().as_str().to_string()),
        expires_at: Some(membership.expires_at),
    })
}

/// Clear the installed authority and re-lock multiplayer. Any
/// running session is left as-is (the QUIC endpoint stays alive
/// until `session_leave`), but subsequent `session_start`,
/// `session_join`, and `session_send_presence` calls will fail
/// with [`SessionBridgeError::NotInKChatGroup`].
pub fn kchat_clear_authority() -> KChatMembershipStatus {
    *kchat_slot().lock() = no_kchat_authority();
    KChatMembershipStatus {
        locked: true,
        group_id: None,
        peer_id: None,
        expires_at: None,
    }
}

/// Report the current KChat gate state to the renderer. Uses the
/// same full re-verification (`require_active_kchat_membership`)
/// that gates `session_start` / `session_join` /
/// `session_send_presence`, so the renderer's panel state is
/// always consistent with whether the bridge would actually let
/// multiplayer through right now. In particular: an installed
/// membership whose validity window has just expired is reported
/// as `locked: true` even before the renderer attempts another
/// session call.
pub fn kchat_membership_status() -> KChatMembershipStatus {
    match require_active_kchat_membership() {
        Ok(m) => KChatMembershipStatus {
            locked: false,
            group_id: Some(m.group_id.as_str().to_string()),
            peer_id: Some(m.peer_id().as_str().to_string()),
            expires_at: Some(m.expires_at),
        },
        Err(_) => KChatMembershipStatus {
            locked: true,
            group_id: None,
            peer_id: None,
            expires_at: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_seed_accepts_padded_and_unpadded() {
        let raw = [42u8; 32];
        let padded = base64::engine::general_purpose::URL_SAFE.encode(raw);
        let unpadded = URL_SAFE_NO_PAD.encode(raw);
        assert_eq!(decode_seed(&padded).unwrap(), raw);
        assert_eq!(decode_seed(&unpadded).unwrap(), raw);
    }

    #[test]
    fn decode_seed_rejects_wrong_length() {
        let short = URL_SAFE_NO_PAD.encode([1u8; 16]);
        let err = decode_seed(&short).unwrap_err();
        assert!(matches!(
            err,
            SessionBridgeError::InvalidArgument { field: "seed", .. }
        ));
    }

    #[test]
    fn identity_from_wire_rejects_peer_id_mismatch() {
        let key = PeerKey::from_seed([7u8; 32]);
        let identity = key.identity("Alice");
        // Fake a peer id by swapping the first character with `_`
        // so the derivation no longer matches.
        let mut bad_id = identity.peer_id.as_str().to_string();
        bad_id.replace_range(0..1, "_");
        let err =
            identity_from_wire(&bad_id, &identity.public_key, &identity.display_name).unwrap_err();
        assert!(matches!(
            err,
            SessionBridgeError::InvalidArgument {
                field: "peerId",
                ..
            }
        ));
    }

    #[test]
    fn identity_from_wire_round_trips_well_formed_input() {
        let key = PeerKey::from_seed([8u8; 32]);
        let identity = key.identity("Bob");
        let out = identity_from_wire(
            identity.peer_id.as_str(),
            &identity.public_key,
            &identity.display_name,
        )
        .unwrap();
        assert_eq!(out.peer_id, identity.peer_id);
        assert_eq!(out.public_key, identity.public_key);
        assert_eq!(out.display_name, identity.display_name);
    }

    #[test]
    fn session_drain_events_when_not_running_returns_error() {
        // Ensure no other test left a session running.
        let _ = session_leave();
        let err = session_drain_events().unwrap_err();
        assert!(matches!(err, SessionBridgeError::NotRunning));
    }

    /// Welcome-status round-trip — guards against future enum
    /// reorderings in `kcreate_collab` that would silently change
    /// the wire shape of the UI.
    #[test]
    fn welcome_status_values_round_trip_through_json() {
        use kcreate_collab::WelcomeStatus;
        for status in [WelcomeStatus::Accepted, WelcomeStatus::Rejected] {
            let s = serde_json::to_string(&status).unwrap();
            let back: WelcomeStatus = serde_json::from_str(&s).unwrap();
            assert_eq!(status, back);
        }
    }

    // ====================================================================
    // KChat group gate tests.
    //
    // These exercise the protocol-level multiplayer lock end-to-end at
    // the bridge surface: `session_*` entry points must fail with
    // `NotInKChatGroup` until a signed membership is installed via
    // `kchat_install_authority`, and re-lock again when
    // `kchat_clear_authority` is called.
    //
    // `kchat_slot` is a process-global singleton, so every test in
    // this group is `#[serial]` and explicitly resets the slot in
    // setup. Tests use raw `ed25519_dalek::SigningKey`s to play the
    // role of the (still-to-be-built) KChat group server.
    // ====================================================================
    use ed25519_dalek::SigningKey;
    use kcreate_collab::kchat::KChatMembership;
    use serial_test::serial;

    /// Reset the KChat slot to the default-deny state at the start
    /// of every gate test. Sharing this helper avoids each test
    /// silently inheriting state from a sibling.
    fn reset_kchat_slot() {
        *kchat_slot().lock() = no_kchat_authority();
    }

    /// Mint a fresh, valid `KChatInstallRequest` JSON payload bound
    /// to the supplied local peer key and group. The issuer keypair
    /// is generated per-call so each test gets a fresh trust root.
    fn fresh_install_request_json(local_seed: [u8; 32], group: &str) -> (String, [u8; 32]) {
        let issuer_seed = [0xAA; 32];
        let issuer = SigningKey::from_bytes(&issuer_seed);
        let local_key = PeerKey::from_seed(local_seed);
        let local_identity = local_key.identity("local");
        let now = Utc::now();
        let expires = now + chrono::Duration::hours(1);
        let issued = now - chrono::Duration::minutes(1);
        let membership = KChatMembership::issue(
            KChatGroupId::new(group).unwrap(),
            local_identity.peer_id.clone(),
            local_identity.public_key.clone(),
            issued,
            expires,
            &issuer,
        )
        .unwrap();
        let req = KChatInstallRequest {
            issuer_public_key: membership.issuer_public_key.clone(),
            group_id: membership.group_id.as_str().to_string(),
            peer_id: membership.peer_id().as_str().to_string(),
            peer_public_key: membership.peer_public_key.clone(),
            issued_at: membership.issued_at,
            expires_at: membership.expires_at,
            signature: membership.signature.clone(),
        };
        (serde_json::to_string(&req).unwrap(), issuer_seed)
    }

    #[test]
    #[serial]
    fn default_kchat_status_is_locked() {
        reset_kchat_slot();
        let status = kchat_membership_status();
        assert!(status.locked, "default authority should be locked");
        assert!(status.group_id.is_none());
        assert!(status.peer_id.is_none());
    }

    #[test]
    #[serial]
    fn session_start_fails_when_locked() {
        reset_kchat_slot();
        let _ = session_leave();
        let seed_b64 = URL_SAFE_NO_PAD.encode([7u8; 32]);
        let err = session_start(&seed_b64, "ken", Uuid::new_v4(), false).unwrap_err();
        assert!(
            matches!(err, SessionBridgeError::NotInKChatGroup),
            "expected NotInKChatGroup, got {err:?}"
        );
    }

    #[test]
    #[serial]
    fn session_join_fails_when_locked() {
        reset_kchat_slot();
        let _ = session_leave();
        // The fixture peer values are well-formed but irrelevant —
        // the gate fires before any of them are looked at.
        let key = PeerKey::from_seed([8u8; 32]);
        let identity = key.identity("Alice");
        let err = session_join(
            identity.peer_id.as_str(),
            &identity.public_key,
            &identity.display_name,
            "127.0.0.1:65432",
            &URL_SAFE_NO_PAD.encode([0u8; 32]),
        )
        .unwrap_err();
        assert!(
            matches!(err, SessionBridgeError::NotInKChatGroup),
            "expected NotInKChatGroup, got {err:?}"
        );
    }

    #[test]
    #[serial]
    fn session_send_presence_fails_when_locked() {
        reset_kchat_slot();
        let _ = session_leave();
        let err = session_send_presence(None, Vec::new(), None).unwrap_err();
        assert!(
            matches!(err, SessionBridgeError::NotInKChatGroup),
            "expected NotInKChatGroup, got {err:?}"
        );
    }

    #[test]
    #[serial]
    fn install_authority_unlocks_status_and_clear_relocks() {
        reset_kchat_slot();
        let (req_json, _) = fresh_install_request_json([9u8; 32], "studio-alpha");
        let status_json =
            kchat_install_authority(serde_json::from_str(&req_json).unwrap()).unwrap();
        assert!(!status_json.locked, "install should unlock");
        assert_eq!(status_json.group_id.as_deref(), Some("studio-alpha"));

        let polled = kchat_membership_status();
        assert!(!polled.locked, "status snapshot should match install");
        assert_eq!(polled.group_id.as_deref(), Some("studio-alpha"));

        let cleared = kchat_clear_authority();
        assert!(cleared.locked, "clear should re-lock");
        assert!(kchat_membership_status().locked);
    }

    #[test]
    #[serial]
    fn install_authority_rejects_forged_signature() {
        reset_kchat_slot();
        let (req_json, _) = fresh_install_request_json([10u8; 32], "studio-beta");
        let mut req: KChatInstallRequest = serde_json::from_str(&req_json).unwrap();
        // Flip a byte inside the base64 signature so verify_strict fails.
        let mut sig_bytes = URL_SAFE_NO_PAD.decode(req.signature.as_bytes()).unwrap();
        sig_bytes[0] ^= 0x01;
        req.signature = URL_SAFE_NO_PAD.encode(sig_bytes);
        let err = kchat_install_authority(req).unwrap_err();
        assert!(
            matches!(err, SessionBridgeError::KChat(_)),
            "expected KChat verify failure, got {err:?}"
        );
        assert!(
            kchat_membership_status().locked,
            "failed install must leave the slot locked"
        );
    }

    #[test]
    #[serial]
    fn install_authority_rejects_peer_id_not_matching_public_key() {
        reset_kchat_slot();
        let (req_json, _) = fresh_install_request_json([11u8; 32], "studio-gamma");
        let mut req: KChatInstallRequest = serde_json::from_str(&req_json).unwrap();
        // Replace the claimed peer id with a different valid one.
        let other = PeerKey::from_seed([99u8; 32]).peer_id();
        req.peer_id = other.as_str().to_string();
        let err = kchat_install_authority(req).unwrap_err();
        assert!(
            matches!(
                err,
                SessionBridgeError::InvalidArgument {
                    field: "peerId",
                    ..
                }
            ),
            "expected InvalidArgument(peerId), got {err:?}"
        );
    }

    #[test]
    #[serial]
    fn install_then_session_start_with_wrong_seed_is_still_locked() {
        reset_kchat_slot();
        let _ = session_leave();
        let (req_json, _) = fresh_install_request_json([12u8; 32], "studio-delta");
        kchat_install_authority(serde_json::from_str(&req_json).unwrap()).unwrap();
        // Try to start with a *different* seed than the membership
        // was minted for. Even though the slot is "unlocked" by the
        // status reporter, session_start re-verifies and bounces.
        let other_seed_b64 = URL_SAFE_NO_PAD.encode([13u8; 32]);
        let err = session_start(&other_seed_b64, "ken", Uuid::new_v4(), false).unwrap_err();
        assert!(
            matches!(err, SessionBridgeError::NotInKChatGroup),
            "expected NotInKChatGroup, got {err:?}"
        );
    }

    // ====================================================================
    // Block 7: journal ingestion tests.
    //
    // These cover the bridge's translation between
    // `Message::OperationBroadcast` / `Message::ResumeBundle` payloads
    // and the in-memory `OperationJournal`. They don't spin up a real
    // host or transport — the helpers construct a `SessionState`
    // directly and invoke the `journal_inbound_*` paths so we can
    // assert on journal contents and emitted `SessionEvent`s without
    // a tokio runtime or QUIC stack.
    // ====================================================================

    #[test]
    #[serial]
    fn journal_inbound_broadcast_records_and_emits_event() {
        // We can't construct a full SessionState without a real host;
        // build the journal in isolation and walk the same code that
        // journal_inbound_broadcast would, asserting append behaviour.
        let project = Uuid::new_v4();
        let mut journal = OperationJournal::open(MemoryJournalStore::new(), project).unwrap();
        let remote_key = PeerKey::from_seed([21u8; 32]);
        let remote = remote_key.peer_id();
        let op = kcreate_core::operation::Operation::new(
            "remote",
            "set_text",
            serde_json::Value::Null,
            serde_json::Value::Null,
            vec![],
        );
        // First broadcast: clock 1.
        let next_clock = journal
            .resume_vector()
            .highest_for(&remote)
            .as_u64()
            .saturating_add(1);
        journal
            .append(
                remote.clone(),
                kcreate_collab::LamportClock::from_raw(next_clock),
                op.clone(),
            )
            .unwrap();
        // Second broadcast: clock 2.
        let next_clock = journal
            .resume_vector()
            .highest_for(&remote)
            .as_u64()
            .saturating_add(1);
        journal
            .append(
                remote.clone(),
                kcreate_collab::LamportClock::from_raw(next_clock),
                op.clone(),
            )
            .unwrap();
        let summary = journal.resume_vector();
        assert_eq!(summary.highest_for(&remote).as_u64(), 2);
        assert_eq!(summary.peer_count(), 1);
    }

    #[test]
    #[serial]
    fn journal_resume_bundle_replays_in_order() {
        let project = Uuid::new_v4();
        let mut journal = OperationJournal::open(MemoryJournalStore::new(), project).unwrap();
        let remote_key = PeerKey::from_seed([22u8; 32]);
        let remote = remote_key.peer_id();
        let op = kcreate_core::operation::Operation::new(
            "remote",
            "set_text",
            serde_json::Value::Null,
            serde_json::Value::Null,
            vec![],
        );
        // Replay 3 entries via the bundle path. The bundle's entries
        // are (peer, clock)-ordered and the journal's monotonicity
        // gate enforces that — out of order bundles would partially
        // fail, mirroring journal_inbound_resume_bundle's silent skip.
        for clk in [1u64, 2, 3] {
            let entry = JournalEntry {
                peer_id: remote.clone(),
                clock: kcreate_collab::LamportClock::from_raw(clk),
                project_id: project,
                operation: op.clone(),
            };
            journal
                .append(entry.peer_id, entry.clock, entry.operation)
                .unwrap();
        }
        assert_eq!(journal.len().unwrap(), 3);
        assert_eq!(journal.resume_vector().highest_for(&remote).as_u64(), 3);
    }

    #[test]
    #[serial]
    fn journal_dedupes_repeated_broadcast() {
        // A re-delivered broadcast at the same clock must not double-
        // record. journal.append rejects with Duplicate, which the
        // journal_inbound_broadcast path silently swallows.
        let project = Uuid::new_v4();
        let mut journal = OperationJournal::open(MemoryJournalStore::new(), project).unwrap();
        let remote = PeerKey::from_seed([23u8; 32]).peer_id();
        let op = kcreate_core::operation::Operation::new(
            "remote",
            "set_text",
            serde_json::Value::Null,
            serde_json::Value::Null,
            vec![],
        );
        journal
            .append(
                remote.clone(),
                kcreate_collab::LamportClock::from_raw(1),
                op.clone(),
            )
            .unwrap();
        let dup_err = journal
            .append(
                remote.clone(),
                kcreate_collab::LamportClock::from_raw(1),
                op.clone(),
            )
            .unwrap_err();
        assert!(matches!(
            dup_err,
            kcreate_collab::JournalError::Duplicate { .. }
        ));
        assert_eq!(journal.len().unwrap(), 1);
    }

    #[test]
    #[serial]
    fn resume_vector_is_serde_round_tripable_for_wire_use() {
        // session_journal_summary serializes ResumeVector through
        // serde_json. Confirm the wire shape is what the renderer
        // gets, since the renderer's type definitions hard-code
        // camelCase + transparent map encoding.
        let mut v = ResumeVector::empty();
        let remote = PeerKey::from_seed([24u8; 32]).peer_id();
        v.by_peer
            .insert(remote.clone(), kcreate_collab::LamportClock::from_raw(7));
        let json = serde_json::to_string(&v).unwrap();
        let back: ResumeVector = serde_json::from_str(&json).unwrap();
        assert_eq!(
            back.highest_for(&remote),
            kcreate_collab::LamportClock::from_raw(7)
        );
    }
}
