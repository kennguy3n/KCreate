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
use std::sync::OnceLock;
use std::time::Duration;

use base64::engine::general_purpose::{STANDARD_NO_PAD, URL_SAFE_NO_PAD};
use base64::Engine;
use chrono::{DateTime, Utc};
use kcreate_collab::message::Cursor;
use kcreate_collab::{Message, PeerId, PeerIdentity, PeerKey, PresencePayload, SessionConfig};
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
}

fn slot() -> &'static Mutex<Option<SessionState>> {
    static S: OnceLock<Mutex<Option<SessionState>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(None))
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
        InboundEvent::Message { from, message } => {
            if let Message::Presence(p) = message.as_ref() {
                state.presence.insert(from.clone(), p.clone());
                push_event(
                    state,
                    SessionEvent::PresenceUpdated {
                        peer_id: from.as_str().to_string(),
                        presence: SessionPresence::from(p),
                    },
                );
            }
        }
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
    *guard = Some(SessionState {
        host,
        runtime,
        presence: HashMap::new(),
        events: std::collections::VecDeque::new(),
        pump_handle,
        report: report.clone(),
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

/// Broadcast the local user's presence (active page, selection,
/// cursor). Called by the renderer on selection / canvas pointer
/// events.
pub fn session_send_presence(
    active_page: Option<Uuid>,
    selection: Vec<Uuid>,
    cursor: Option<SessionCursor>,
) -> Result<()> {
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
            | SessionBridgeError::InvalidArgument { .. } => Self::InvalidArgument {
                argument: "session".to_string(),
                value: e.to_string(),
            },
            SessionBridgeError::Transport(_) | SessionBridgeError::Collab(_) => {
                Self::Io(std::io::Error::other(e.to_string()))
            }
        }
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
}
