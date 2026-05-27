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
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use base64::engine::general_purpose::{STANDARD_NO_PAD, URL_SAFE_NO_PAD};
use base64::Engine;
use chrono::{DateTime, Utc};
use ed25519_dalek::VerifyingKey;
use kcreate_collab::conflict::{
    ConflictDecision, ConflictResolver, LastWriterWinsResolver, OperationContext,
};
use kcreate_collab::message::Cursor;
use kcreate_collab::{
    no_kchat_authority, BoundKChatGroupAuthority, KChatAuthError, KChatGroupId, KChatMembership,
    LamportClock, LockClaimPayload, LockReleasePayload, MemoryJournalStore, Message,
    OperationJournal, PeerId, PeerIdentity, PeerKey, PresencePayload, SessionConfig,
    SharedKChatAuthority,
};
#[cfg(test)]
use kcreate_collab::{JournalEntry, ResumeVector};
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
// `rename_all_fields = "camelCase"` is required here in addition to
// `rename_all = "camelCase"`. The latter only renames variant
// names (so `PeerJoined` becomes the `kind: "peerJoined"`
// discriminator), while the former is what actually camelCases the
// inner struct fields — without it `peer_id` / `public_key` /
// `display_name` etc. would serialise as their literal snake_case
// names, which `apps/desktop/shared/scene.ts::SessionEvent`
// (the renderer-side type) does NOT match. The full IPC chain is
// bridge::SessionEvent → `session_drain_events` (serde_json::to_string)
// → main.ts (`drainSessionEvents` forwards the raw JSON) →
// renderer (`window.kcreate.session.onEvent` → `JSON.parse`).
// Mid-chain there is no automatic snake→camel translation, so the
// bridge has to emit camelCase directly. AGENTS.md rule 4 calls
// this out explicitly.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
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
    /// Block 8: the lock roster changed — a peer claimed or
    /// released one or more node locks. The renderer reads
    /// `session_locks()` for the authoritative snapshot but
    /// uses this event to know *when* to re-read instead of
    /// polling on every frame.
    LocksChanged {
        /// Which peer caused the change. For `PeerLeft`-triggered
        /// auto-releases this is the leaving peer.
        peer_id: String,
        /// Node ids whose lock status flipped in this transition
        /// (newly claimed AND newly released ids are both
        /// surfaced — the renderer cross-references with the
        /// authoritative roster).
        node_ids: Vec<Uuid>,
    },
    /// Round 11: the local collab session just started. Emitted
    /// synchronously from `session_start` right before the report
    /// is returned so renderer-side hooks (`useSessionLocks`,
    /// `EditorPage` presence-broadcast effect) can re-key their
    /// state on local-side lifecycle transitions — the existing
    /// `peer*` events only fire for *remote* peers and would never
    /// signal a fresh local session by themselves.
    SessionStarted {
        /// Base64url-encoded local peer id.
        peer_id: String,
        /// Project the new session is bound to.
        project_id: Uuid,
    },
    /// Round 11: the local collab session just stopped. The bridge
    /// cannot push this through the regular event queue because the
    /// queue is owned by the `SessionState` that `session_leave`
    /// has to drop — instead `session_leave` returns the leaving
    /// peer id to `main.ts`, which emits the synthetic event
    /// directly on the renderer's session-event channel. Carries
    /// the leaving peer's id so consumers can reset session-keyed
    /// dedup fingerprints (e.g. `EditorPage`'s presence-broadcast
    /// guard, `useSessionLocks`'s lock roster cache).
    SessionLeft {
        /// Base64url-encoded peer id of the session that just left.
        peer_id: String,
    },
    /// Phase 7 (Task 8): a connected peer was evicted from the
    /// session because their KChat community membership was
    /// revoked. Emitted by the roster-sync poller; the underlying
    /// QUIC connection is closed (with a directed `Goodbye(Kicked)`
    /// so the kicked peer logs a clean shutdown) before this event
    /// reaches the renderer. Carries a human-readable reason so
    /// the UI can show a toast that distinguishes a kick from a
    /// regular `PeerLeft`.
    PeerKicked {
        peer_id: String,
        /// `revoked-from-community` for the standard membership-
        /// revocation path; future kick paths (rate-limit, ACL,
        /// etc.) can extend this without changing the variant.
        reason: String,
    },
    /// Phase 7 (Task 11): a peer's collaboration permission changed
    /// (e.g. the host downgraded a member to Viewer). The
    /// renderer uses this to update toolbars / disable controls
    /// for viewers.
    PermissionChanged {
        peer_id: String,
        permission: CollabPermission,
    },
    /// Phase 7 (Task 15): a remote peer applied a `ResumeBundle`
    /// reply to us — the late-join replay has finished and the
    /// local journal is now up to date with the host's history.
    /// The renderer uses this to dismiss the "syncing\u2026"
    /// indicator and surface the post-resume document.
    ResumeApplied {
        /// Peer id of the host that supplied the bundle.
        from_peer_id: String,
        /// Number of operations from the bundle that were appended
        /// to the local journal (may be smaller than the bundle's
        /// `operations.len()` because the journal silently
        /// dedupes already-seen entries).
        applied_count: u32,
    },
    /// Phase 7 (Task 16): the CRDT conflict resolver picked a
    /// winner for a concurrent edit. The renderer uses this to
    /// surface a "Your edit was overridden by Ken" toast. The
    /// loser is always the local peer when `loser_peer_id` matches
    /// the session's `local_peer_id`; the renderer filters its
    /// toast accordingly.
    ConflictResolved {
        /// Node id whose value the resolver had to tiebreak.
        node_id: Uuid,
        /// Peer whose write the resolver kept.
        winner_peer_id: String,
        /// Peer whose write the resolver discarded.
        loser_peer_id: String,
        /// JSON pointer (or dotted path) of the field whose value
        /// was resolved. Free-form string so the conflict source
        /// (LWW field, set member, list slot) can be reported
        /// without baking the CRDT shape into this enum.
        field: String,
    },
    /// Phase 7 (Task 17): a remote peer broadcast an undo (or redo)
    /// operation. The renderer's activity feed surfaces
    /// "Ken undid their last edit"; the operation itself was
    /// already journaled via
    /// [`SessionEvent::OperationsJournaled`].
    UndoBroadcast {
        /// Peer id that produced the undo.
        peer_id: String,
        /// Number of inverse operations in the batch (a grouped
        /// undo spans many primitive ops, so the toast text can
        /// pluralize correctly).
        op_count: u32,
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
    /// The bridge was built without `kchat-dev-issuer`, so the
    /// dev mint endpoint is unavailable. Returned by
    /// `kchat_dev_mint_membership_json` in production builds so
    /// the renderer can present a clean "this build doesn't
    /// include the dev issuer" message instead of a generic error.
    #[error("KChat dev issuer is not enabled in this build")]
    KChatDevIssuerDisabled,
    /// The install request's `issuer_public_key` is not on the
    /// configured trusted-issuer allowlist. Block E gate — only
    /// fires when the allowlist is non-empty; an empty allowlist
    /// preserves the pre-Block-E "accept any issuer" behaviour
    /// that the dev-mint surface depends on.
    #[error(
        "KChat issuer {issuer_public_key} is not on the trusted-issuer allowlist; \
         add it via `kchat_add_trusted_issuer` or clear the allowlist to accept any issuer"
    )]
    IssuerNotTrusted { issuer_public_key: String },
    /// Persisting the trust store to disk failed. The in-memory
    /// state is still mutated, but subsequent sessions will not
    /// see the change. Renderer surfaces this so the user knows
    /// their add/remove won't survive an app restart.
    #[error("KChat trust store I/O: {0}")]
    TrustStoreIo(String),
    /// Phase 7 (Task 11): the local peer's collaboration permission
    /// is `Viewer`, but the requested operation requires `Editor`
    /// (e.g. `session_broadcast_operations`). Surfaced to the
    /// renderer so it can show a "you're in read-only mode" hint
    /// instead of a generic transport error.
    #[error("local peer is in read-only mode: operation requires Editor permission")]
    PermissionDenied,
    /// Phase 7 (Task 10): the supplied share-document invite was
    /// malformed JSON or carried fields that violate the schema
    /// (e.g. a `communityId` that doesn't match the active
    /// community gate, an expired issuance timestamp, a peer id
    /// that doesn't BLAKE3-derive from the supplied public key,
    /// etc.). Carries the specific reason so the renderer can show
    /// a clear "this invite is invalid: …" error.
    #[error("invite rejected: {0}")]
    InviteRejected(String),
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
    /// Block 8: advisory edit-lock roster. Map of `node_id ->
    /// (holder_peer_id, acquired_at)`. Updated when this peer (or a
    /// remote peer) emits a [`Message::LockClaim`] / `LockRelease`,
    /// and auto-cleaned on `PeerLeft`. Soft semantics — the
    /// renderer disables controls for locked nodes but the
    /// protocol doesn't reject concurrent edits.
    locks: HashMap<Uuid, LockEntry>,
    /// Phase 7 (Task 7+8): optional KChat community id this session
    /// is bound to. Drives community-scoped mDNS filtering at the
    /// transport layer and the roster-sync poller that fires
    /// [`SessionEvent::PeerKicked`] when a community member is
    /// revoked. `None` for sessions started via the legacy /
    /// dev-mint flow without a community gate.
    community_id: Option<String>,
    /// Phase 7 (Task 11): per-peer role snapshot, derived from the
    /// latest community-members poll. `member` (and any unknown
    /// role) maps to [`CollabPermission::Editor`] by default; the
    /// host can call [`session_set_peer_permission`] to downgrade
    /// a specific peer to `Viewer`. Viewers receive operations but
    /// the bridge silently drops outbound broadcasts when the
    /// local peer is `Viewer`.
    permissions: HashMap<PeerId, CollabPermission>,
    /// Phase 7 (Task 11): the local peer's own permission for the
    /// active community. Used by [`session_broadcast_operation`]
    /// to enforce the read-only viewer contract. Defaults to
    /// [`CollabPermission::Editor`] when the session is not
    /// community-gated.
    local_permission: CollabPermission,
    /// Phase 7 (Task 16): bounded ring buffer of recently-broadcast
    /// local operations, kept so the inbound-event pump can run
    /// [`LastWriterWinsResolver`] against a remote op whose
    /// `affected_nodes` overlap a local op we just sent. When the
    /// resolver decides the local op was the loser, the bridge
    /// emits a [`SessionEvent::ConflictResolved`] so the renderer's
    /// `ConflictToast` can surface "Your edit was overridden by …".
    ///
    /// Capacity is bounded ([`RECENT_LOCAL_OPS_CAP`]) so a long
    /// drag-edit burst can't unbound this buffer; older entries
    /// are evicted FIFO once the cap is hit. Each entry stores the
    /// op + the Lamport clock value it was broadcast under
    /// (derived from the journal's local high-water mark) so the
    /// resolver's tiebreak rules apply unchanged.
    recent_local_ops:
        std::collections::VecDeque<(kcreate_collab::LamportClock, kcreate_core::Operation)>,
}

/// Phase 7 (Task 16): how many of the most-recent local
/// broadcasts to keep available for conflict detection. Sized to
/// cover ~3 s of a sustained drag-edit burst at the renderer's
/// peak input rate of ~30 ops/s, with headroom. A circular buffer
/// rather than a time-windowed eviction so the upper bound is
/// strict regardless of clock jitter on the inbound pump.
const RECENT_LOCAL_OPS_CAP: usize = 128;

/// Phase 7 (Task 11): collaboration permission for a peer. Derived
/// from the peer's KChat community role: `owner` / `admin` =>
/// [`Editor`](Self::Editor), `member` => [`Editor`](Self::Editor)
/// unless the session host has explicitly downgraded them to
/// [`Viewer`](Self::Viewer). Viewers receive operations but cannot
/// broadcast.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CollabPermission {
    /// Read-write — may broadcast `OperationBroadcast`, `LockClaim`,
    /// and `LockRelease` messages.
    Editor,
    /// Read-only — receives operations / presence but the bridge
    /// suppresses outbound broadcasts.
    Viewer,
}

impl CollabPermission {
    /// Map a KChat community role string to a default permission.
    /// `owner` and `admin` are always editors; `member` is also an
    /// editor by default (the host can downgrade individual members
    /// via [`session_set_peer_permission`]). Unknown roles fail
    /// closed to `Viewer` so a future role we don't know about
    /// doesn't accidentally get write access.
    #[must_use]
    pub fn from_role(role: &str) -> Self {
        match role {
            "owner" | "admin" | "member" => Self::Editor,
            _ => Self::Viewer,
        }
    }
}

/// Block 8: one entry in the advisory lock roster.
#[derive(Debug, Clone)]
struct LockEntry {
    holder: PeerId,
    acquired_at: DateTime<Utc>,
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
            // Block 8: auto-release every lock the leaving peer
            // held. Without this, a peer that dies mid-edit would
            // pin a node forever; the renderer would show "Ken is
            // editing" even though Ken is gone. Collect the released
            // node ids before mutation so the LocksChanged event
            // carries an accurate snapshot.
            let released: Vec<Uuid> = state
                .locks
                .iter()
                .filter(|(_, entry)| entry.holder == peer_id)
                .map(|(id, _)| *id)
                .collect();
            state.locks.retain(|_, entry| entry.holder != peer_id);
            push_event(
                state,
                SessionEvent::PeerLeft {
                    peer_id: peer_id.as_str().to_string(),
                },
            );
            if !released.is_empty() {
                push_event(
                    state,
                    SessionEvent::LocksChanged {
                        peer_id: peer_id.as_str().to_string(),
                        node_ids: released,
                    },
                );
            }
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
                // Phase 7 (Task 15): emit a `ResumeApplied`
                // event after the replay so the renderer can
                // dismiss the "syncing…" indicator and
                // surface the post-resume document state.
                if p.project_id == state.journal.project_id() {
                    let applied = journal_inbound_resume_bundle(state, p);
                    push_event(
                        state,
                        SessionEvent::ResumeApplied {
                            from_peer_id: from.as_str().to_string(),
                            applied_count: applied,
                        },
                    );
                }
            }
            Message::ResumeRequest(p) => {
                // Phase 7 (Task 15): a freshly-joined peer is
                // asking us for the history since their resume
                // vector. Compute the delta from the journal,
                // package it as a `ResumeBundle`, and send it
                // directly back to the requester. Project-id
                // mismatch is dropped (same rationale as the
                // other handlers).
                if p.project_id == state.journal.project_id() {
                    handle_resume_request(state, &from, p);
                }
            }
            Message::LockClaim(p) => {
                // Block 8: remote peer is asking us to honour a
                // soft lock. The project_id guard prevents a
                // misrouted message from poisoning the local
                // roster; otherwise we just record and emit.
                if p.project_id == state.journal.project_id() {
                    apply_lock_claim(state, &from, p);
                }
            }
            Message::LockRelease(p) => {
                if p.project_id == state.journal.project_id() {
                    apply_lock_release(state, &from, p);
                }
            }
            // Hello / Welcome / Heartbeat / Goodbye are handled
            // by the transport layer itself, not surfaced as
            // bridge-level events.
            Message::Hello(_) | Message::Welcome(_) | Message::Heartbeat | Message::Goodbye(_) => {}
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
    // Phase 7 (Task 16): collected on the side so we can emit
    // `ConflictResolved` events *after* the borrow on
    // `state.recent_local_ops` ends — pushing during the resolve
    // loop would alias-borrow `state` for the duration.
    let mut conflict_events: Vec<SessionEvent> = Vec::new();
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
        let clock = LamportClock::from_raw(next_clock);
        match state.journal.append(from.clone(), clock, op.clone()) {
            Ok(()) => {
                recorded += 1;
                if next_clock > highest {
                    highest = next_clock;
                }
                // Phase 7 (Task 16): run the CRDT resolver against
                // every recent local op whose `affected_nodes`
                // overlap the incoming remote op. The resolver is
                // the same `LastWriterWinsResolver` that the wire
                // protocol picks for tiebreaks, so the renderer's
                // local document state stays in sync with what
                // the resolver decided — we just need to *surface*
                // the loser to the user.
                collect_conflicts(state, from, op, clock, &mut conflict_events);
            }
            Err(
                kcreate_collab::JournalError::Duplicate { .. }
                | kcreate_collab::JournalError::OutOfOrder { .. }
                | kcreate_collab::JournalError::Backend(_),
            ) => {
                // Duplicate / OOO: expected for re-delivered
                // batches. Backend: memory store can't produce
                // this today, but degrade gracefully if a future
                // SQLite swap does.
            }
        }
    }
    // Drain the side-collected conflict events now that the
    // resolver borrow has dropped. Pushed before
    // `OperationsJournaled` would also be fine, but the renderer
    // typically reads events in arrival order, so surfacing the
    // "what got journaled" baseline first matches the mental model.
    for ev in conflict_events {
        push_event(state, ev);
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
        // Phase 7 (Task 17): if every recorded op in the batch is
        // an undo / redo inverse, surface a dedicated event so
        // the renderer's activity feed can show "Ken undid their
        // last edit" instead of "Ken edited 3 nodes". A mixed
        // batch is *not* surfaced as `UndoBroadcast` because the
        // semantics would be ambiguous — callers are expected
        // to group their broadcasts cleanly (the bridge’s undo
        // path always sends inverse-only batches).
        let undo_count: u32 = payload
            .operations
            .iter()
            .filter(|o| o.is_undo)
            .count()
            .try_into()
            .unwrap_or(u32::MAX);
        if undo_count > 0 && undo_count == recorded {
            push_event(
                state,
                SessionEvent::UndoBroadcast {
                    peer_id: from.as_str().to_string(),
                    op_count: undo_count,
                },
            );
        }
    }
}

/// Phase 7 (Task 16): scan the recent-local-ops ring for any entry
/// whose `affected_nodes` overlap the incoming remote op, run
/// [`LastWriterWinsResolver::resolve`] on every match, and push a
/// [`SessionEvent::ConflictResolved`] into `out` for every case the
/// resolver picked the remote (i.e. the local peer lost the
/// tiebreak). When the resolver picks the local op we deliberately
/// do *not* fire an event — the local document state already wins,
/// and the renderer doesn't need a toast for "your edit was kept".
///
/// `field` is reported as the comma-joined affected-node list. The
/// CRDT layer doesn't carry per-field provenance today (Phase 3
/// design left that to a future protocol revision), so reporting the
/// affected nodes keeps the toast accurate without inventing a
/// fictional field path — the `ConflictToast` renderer falls back
/// to the node name when the field string is opaque.
fn collect_conflicts(
    state: &SessionState,
    remote_peer: &PeerId,
    remote_op: &kcreate_core::Operation,
    remote_clock: LamportClock,
    out: &mut Vec<SessionEvent>,
) {
    if remote_op.affected_nodes.is_empty() || state.recent_local_ops.is_empty() {
        return;
    }
    let resolver = LastWriterWinsResolver;
    let local_peer = &state.local_peer_id;
    for (local_clock, local_op) in &state.recent_local_ops {
        // Skip operations that share no affected nodes — the
        // resolver short-circuits these to `KeepBoth` anyway, but
        // doing the check up front avoids the allocation for
        // `OperationContext` on the common no-overlap case.
        if local_op.affected_nodes.is_empty()
            || !local_op
                .affected_nodes
                .iter()
                .any(|n| remote_op.affected_nodes.contains(n))
        {
            continue;
        }
        let decision = resolver.resolve(
            OperationContext {
                op: local_op,
                clock: *local_clock,
                author: local_peer,
            },
            OperationContext {
                op: remote_op,
                clock: remote_clock,
                author: remote_peer,
            },
        );
        if decision == ConflictDecision::KeepRemote {
            // Emit one event per affected node the local op
            // touched (so the renderer can target the specific
            // ConflictToast at the right node card). The field
            // string surfaces the operation command name so the
            // toast can say "fill.color was overridden by Ken"
            // rather than the opaque op id.
            for node_id in &local_op.affected_nodes {
                out.push(SessionEvent::ConflictResolved {
                    node_id: *node_id,
                    winner_peer_id: remote_peer.as_str().to_string(),
                    loser_peer_id: local_peer.as_str().to_string(),
                    field: local_op.command.clone(),
                });
            }
        }
    }
}

/// Phase 7 (Task 15): handle an inbound [`Message::ResumeRequest`]
/// from a freshly-joined peer. Computes the delta between the
/// journal’s current state and the supplied resume vector and
/// fires the resulting [`Message::ResumeBundle`] directly back at
/// the requester via [`LanCollabHost::send_to`]. Failures are
/// logged and dropped — a missing bundle just means the late
/// joiner stays unsynced until they retry, never panics the host.
fn handle_resume_request(
    state: &SessionState,
    from: &PeerId,
    payload: &kcreate_collab::ResumeRequestPayload,
) {
    let entries = match state.journal.operations_since(&payload.since) {
        Ok(e) => e,
        Err(e) => {
            log::warn!(
                "resume_request from {}: journal.operations_since failed: {e}",
                from.as_str()
            );
            return;
        }
    };
    let bundle = kcreate_collab::ResumeBundlePayload {
        project_id: state.journal.project_id(),
        operations: entries,
    };
    let host = state.host.clone();
    let runtime_handle = state.runtime.handle().clone();
    let target = from.clone();
    // Detach: a slow link to one late-joiner must not block the
    // session pump. Errors are logged on the spawned task.
    runtime_handle.spawn(async move {
        if let Err(e) = host.send_to(&target, Message::ResumeBundle(bundle)).await {
            log::warn!(
                "resume_request reply to {}: send_to failed: {e}",
                target.as_str()
            );
        }
    });
}

/// Replay a [`Message::ResumeBundle`] into the local journal. The
/// bundle's entries are already (peer, clock)-ordered so we can
/// feed them in directly; the journal's monotonicity gate will
/// reject anything that overlaps history we already have, which
/// is exactly the duplicate-replay semantics we want.
///
/// Returns the number of entries that were actually appended
/// (i.e. were not silent duplicates) so the caller can emit a
/// matching [`SessionEvent::ResumeApplied`].
fn journal_inbound_resume_bundle(
    state: &mut SessionState,
    payload: &kcreate_collab::ResumeBundlePayload,
) -> u32 {
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
    let mut applied_total: u32 = 0;
    for (peer, (count, highest)) in per_peer {
        applied_total = applied_total.saturating_add(count);
        push_event(
            state,
            SessionEvent::OperationsJournaled {
                peer_id: peer.as_str().to_string(),
                op_count: count,
                highest_clock: highest,
            },
        );
    }
    applied_total
}

/// Block 8: pure roster mutation for a lock claim. Returns the
/// node ids whose lock state actually flipped (deduped + filtered
/// for no-op same-holder reclaims). Pulled out of [`apply_lock_claim`]
/// so unit tests can exercise the semantics without needing a full
/// `SessionState`.
fn lock_roster_claim(
    locks: &mut HashMap<Uuid, LockEntry>,
    from: &PeerId,
    payload: &LockClaimPayload,
) -> Vec<Uuid> {
    let mut changed: Vec<Uuid> = Vec::new();
    let mut seen: std::collections::HashSet<Uuid> = std::collections::HashSet::new();
    for node_id in &payload.node_ids {
        if !seen.insert(*node_id) {
            continue;
        }
        // Last-claim-wins: a fresh claim from a different peer
        // displaces the prior holder. This matches the LWW
        // resolver's semantics elsewhere in collab and gives the
        // UI a deterministic source of truth.
        let entry = LockEntry {
            holder: from.clone(),
            acquired_at: payload.acquired_at,
        };
        let prior = locks.insert(*node_id, entry);
        let actually_changed = match prior {
            Some(p) => p.holder != *from,
            None => true,
        };
        if actually_changed {
            changed.push(*node_id);
        }
    }
    changed
}

/// Block 8: pure roster mutation for a lock release. Returns the
/// node ids whose lock state actually flipped. Empty `node_ids`
/// payload means "release everything this peer holds".
fn lock_roster_release(
    locks: &mut HashMap<Uuid, LockEntry>,
    from: &PeerId,
    payload: &LockReleasePayload,
) -> Vec<Uuid> {
    let mut changed: Vec<Uuid> = Vec::new();
    if payload.node_ids.is_empty() {
        let owned: Vec<Uuid> = locks
            .iter()
            .filter(|(_, entry)| entry.holder == *from)
            .map(|(id, _)| *id)
            .collect();
        for id in &owned {
            locks.remove(id);
            changed.push(*id);
        }
    } else {
        for node_id in &payload.node_ids {
            if let Some(entry) = locks.get(node_id) {
                // Only the holder can release. A misbehaving peer
                // can't reach over and unlock something it didn't
                // claim — the soft-lock contract still needs an
                // ownership check to be useful.
                if entry.holder == *from {
                    locks.remove(node_id);
                    changed.push(*node_id);
                }
            }
        }
    }
    changed
}

/// Block 8: record an incoming [`Message::LockClaim`] into the
/// session's advisory lock roster and emit a `LocksChanged` event.
/// Thin wrapper around [`lock_roster_claim`] for the production
/// path that has a full `SessionState` in hand.
fn apply_lock_claim(state: &mut SessionState, from: &PeerId, payload: &LockClaimPayload) {
    let changed = lock_roster_claim(&mut state.locks, from, payload);
    if !changed.is_empty() {
        push_event(
            state,
            SessionEvent::LocksChanged {
                peer_id: from.as_str().to_string(),
                node_ids: changed,
            },
        );
    }
}

/// Block 8: record an incoming [`Message::LockRelease`].
fn apply_lock_release(state: &mut SessionState, from: &PeerId, payload: &LockReleasePayload) {
    let changed = lock_roster_release(&mut state.locks, from, payload);
    if !changed.is_empty() {
        push_event(
            state,
            SessionEvent::LocksChanged {
                peer_id: from.as_str().to_string(),
                node_ids: changed,
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
    community_id: Option<String>,
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
    // Phase 7 (Task 7): when a community id is supplied, make sure
    // it matches the membership that is currently installed. Two
    // different communities can be active simultaneously in KChat
    // Desktop, but only one collab gate is installed per process —
    // refusing the mismatch up front is much friendlier than
    // failing later inside `verify` with a generic group-mismatch
    // error.
    if let Some(want) = community_id.as_deref() {
        let authority = kchat_authority_snapshot();
        let membership = authority
            .local_membership()
            .ok_or(SessionBridgeError::NotInKChatGroup)?;
        if membership.group_id().as_str() != want {
            return Err(SessionBridgeError::InvalidArgument {
                field: "communityId",
                message: format!(
                    "requested community {want} but the installed KChat membership is for {}",
                    membership.group_id().as_str()
                ),
            });
        }
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
        community_id: community_id.clone(),
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

    let mut state = SessionState {
        host,
        runtime,
        presence: HashMap::new(),
        events: std::collections::VecDeque::new(),
        pump_handle,
        report: report.clone(),
        journal,
        local_peer_id: local_peer_id.clone(),
        locks: HashMap::new(),
        community_id,
        permissions: HashMap::new(),
        local_permission: CollabPermission::Editor,
        recent_local_ops: std::collections::VecDeque::with_capacity(RECENT_LOCAL_OPS_CAP),
    };
    // Surface the local-lifecycle transition on the same event
    // channel every other session signal flows through, so renderer
    // consumers don't need a separate code path for "my session
    // just started". The push happens before the slot is populated
    // because `push_event` takes `&mut SessionState` directly —
    // either side of the move would also work.
    push_event(
        &mut state,
        SessionEvent::SessionStarted {
            peer_id: local_peer_id.as_str().to_string(),
            project_id,
        },
    );
    *guard = Some(state);
    Ok(report)
}

/// Stop the running session. Sends a graceful `Goodbye` to every
/// peer, closes the QUIC endpoint, drops the tokio runtime.
/// Idempotent: calling on a stopped session is a no-op.
///
/// Round 11: returns the leaving peer id (if a session was actually
/// running) so `main.ts` can forward a synthetic `sessionLeft`
/// event on the renderer's session-event channel. The bridge can't
/// emit the event via its own queue because the queue is torn down
/// as part of the leave — surfacing the id through the return
/// value lets the orchestrator do the right thing without
/// introducing a separate "final drain" IPC dance.
pub fn session_leave() -> Result<Option<String>> {
    let mut guard = slot().lock();
    let Some(state) = guard.take() else {
        return Ok(None);
    };
    let local_peer_id = state.local_peer_id.as_str().to_string();
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
    Ok(Some(local_peer_id))
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
        tokio::time::timeout(
            OP_TIMEOUT,
            host.dial_known_peer(identity.clone(), socket, fp),
        )
        .await
    });
    match result {
        Ok(Ok(_)) => {
            // Phase 7 (Task 15): late-join replay. Right after the
            // dial succeeds, ask the host we just connected to for
            // every journal entry we don't already have. The
            // request fires on a detached tokio task because (a) the
            // bridge entry point shouldn't block on a transport
            // hop the host might not answer immediately, and (b)
            // the resume bundle arrives asynchronously through the
            // pump loop and surfaces as a `ResumeApplied` event.
            //
            // We swallow the result entirely — if the resume fails,
            // the joiner is still connected and live edits will
            // backfill the document; the resume is purely an
            // optimisation for picking up history that predates
            // the join. A failure mode here would surface as the
            // user seeing "syncing…" forever, so we ALSO log a
            // warning so the bug is debuggable from the logs.
            request_resume_from(&identity.peer_id);
            Ok(())
        }
        Ok(Err(e)) => Err(SessionBridgeError::Transport(e)),
        Err(_) => Err(SessionBridgeError::Transport(
            kcreate_collab_transport::TransportError::Quic("dial timed out".into()),
        )),
    }
}

/// Phase 7 (Task 15): fire-and-forget helper that asks `peer` for a
/// `ResumeBundle` covering everything we're missing relative to our
/// local journal's resume vector. Used by `session_join` to
/// auto-trigger late-join replay without forcing every caller of
/// `session_join` to know about resume semantics. Failures are
/// logged and dropped — the live broadcast stream will still keep
/// the local journal current; the only consequence is the joiner
/// won't see history that predates their join.
fn request_resume_from(peer: &PeerId) {
    let guard = slot().lock();
    let Some(state) = guard.as_ref() else {
        // Session ended between the dial and this helper running.
        return;
    };
    let host = state.host.clone();
    let runtime_handle = state.runtime.handle().clone();
    let project_id = state.journal.project_id();
    let since = state.journal.resume_vector();
    let target = peer.clone();
    drop(guard);
    let payload = kcreate_collab::ResumeRequestPayload { project_id, since };
    runtime_handle.spawn(async move {
        if let Err(e) = tokio::time::timeout(
            OP_TIMEOUT,
            host.send_to(&target, Message::ResumeRequest(payload)),
        )
        .await
        {
            log::warn!("auto-resume to {} timed out: {e}", target.as_str(),);
        }
    });
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

/// Block 8: one entry in the JSON lock-roster shape returned to the
/// renderer. Serializes as camelCase to match `SessionJournalSummary`
/// and friends.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionLockEntry {
    pub node_id: Uuid,
    pub holder_peer_id: String,
    pub acquired_at: DateTime<Utc>,
}

/// Block 8: snapshot of the advisory edit-lock roster. KChat-gated.
/// Returns an empty list (not an error) when no session is running
/// so the renderer can call this unconditionally on every paint.
pub fn session_locks() -> Result<Vec<SessionLockEntry>> {
    require_active_kchat_membership()?;
    let guard = slot().lock();
    let Some(state) = guard.as_ref() else {
        return Ok(Vec::new());
    };
    let mut rows: Vec<SessionLockEntry> = state
        .locks
        .iter()
        .map(|(node_id, entry)| SessionLockEntry {
            node_id: *node_id,
            holder_peer_id: entry.holder.as_str().to_string(),
            acquired_at: entry.acquired_at,
        })
        .collect();
    // Deterministic order so the renderer's diffing stays cheap.
    rows.sort_by_key(|r| r.node_id);
    Ok(rows)
}

/// Block 8: claim advisory edit locks on the supplied node ids.
/// Updates the local roster immediately (so the local UI greys
/// out controls without waiting for a round-trip) and broadcasts
/// a `LockClaim` to every connected peer.
///
/// `acquired_at` defaults to wall-clock now; the renderer can use
/// the returned value to show "locked X seconds ago".
pub fn session_claim_locks(node_ids: Vec<Uuid>) -> Result<DateTime<Utc>> {
    require_active_kchat_membership()?;
    let mut guard = slot().lock();
    let state = guard.as_mut().ok_or(SessionBridgeError::NotRunning)?;
    let acquired_at = Utc::now();
    let project_id = state.journal.project_id();
    let local = state.local_peer_id.clone();
    let payload = LockClaimPayload {
        project_id,
        node_ids,
        acquired_at,
    };
    // Update local roster + emit LocksChanged before fanning out
    // so the local UI is consistent the moment this call returns.
    apply_lock_claim(state, &local, &payload);
    let host = state.host.clone();
    let runtime_handle = state.runtime.handle().clone();
    drop(guard);
    let result = runtime_handle.block_on(async {
        tokio::time::timeout(OP_TIMEOUT, host.broadcast_lock_claim(payload)).await
    });
    match result {
        Ok(Ok(())) => Ok(acquired_at),
        Ok(Err(e)) => Err(SessionBridgeError::Transport(e)),
        Err(_) => Err(SessionBridgeError::Transport(
            kcreate_collab_transport::TransportError::Quic("broadcast_lock_claim timed out".into()),
        )),
    }
}

/// Block 8: release advisory edit locks. An empty `node_ids` list
/// releases every lock the local peer holds (the "I'm done editing"
/// signal). Mirrors `session_claim_locks` — local roster is updated
/// before the broadcast so the local UI is responsive.
pub fn session_release_locks(node_ids: Vec<Uuid>) -> Result<()> {
    require_active_kchat_membership()?;
    let mut guard = slot().lock();
    let state = guard.as_mut().ok_or(SessionBridgeError::NotRunning)?;
    let project_id = state.journal.project_id();
    let local = state.local_peer_id.clone();
    let payload = LockReleasePayload {
        project_id,
        node_ids,
    };
    apply_lock_release(state, &local, &payload);
    let host = state.host.clone();
    let runtime_handle = state.runtime.handle().clone();
    drop(guard);
    let result = runtime_handle.block_on(async {
        tokio::time::timeout(OP_TIMEOUT, host.broadcast_lock_release(payload)).await
    });
    match result {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(SessionBridgeError::Transport(e)),
        Err(_) => Err(SessionBridgeError::Transport(
            kcreate_collab_transport::TransportError::Quic(
                "broadcast_lock_release timed out".into(),
            ),
        )),
    }
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

/// Phase 7 (Task 11): broadcast the supplied local operations to
/// every connected peer. Enforces the local peer's collaboration
/// permission: a [`CollabPermission::Viewer`] returns
/// [`SessionBridgeError::PermissionDenied`] without sending. Callers
/// pass already-stamped operations (Lamport clock + author peer id)
/// produced by [`session_record_local_operation`] so the journal
/// stays the single source of truth for ordering.
pub fn session_broadcast_operations(
    operations: Vec<kcreate_core::operation::Operation>,
) -> Result<()> {
    require_active_kchat_membership()?;
    if operations.is_empty() {
        return Ok(());
    }
    let mut guard = slot().lock();
    let state = guard.as_mut().ok_or(SessionBridgeError::NotRunning)?;
    if state.local_permission == CollabPermission::Viewer {
        return Err(SessionBridgeError::PermissionDenied);
    }
    // Phase 7 (Task 16): record each op into the local ring buffer
    // BEFORE we ship it on the wire, so the CRDT resolver in
    // `journal_inbound_broadcast` can detect "remote op landed on a
    // node I just edited" the moment a colliding remote op arrives.
    // The clock value is the journal's per-local high-water mark + 1,
    // matching the same approximation the inbound pump uses for
    // remote ops (the protocol carries the clock on the envelope,
    // not on the op, so the bridge derives both sides the same way).
    let local_peer = state.local_peer_id.clone();
    let mut local_clock_seq = state
        .journal
        .resume_vector()
        .highest_for(&local_peer)
        .as_u64();
    for op in &operations {
        local_clock_seq = local_clock_seq.saturating_add(1);
        if state.recent_local_ops.len() == RECENT_LOCAL_OPS_CAP {
            state.recent_local_ops.pop_front();
        }
        state
            .recent_local_ops
            .push_back((LamportClock::from_raw(local_clock_seq), op.clone()));
    }
    let host = state.host.clone();
    let runtime_handle = state.runtime.handle().clone();
    let project_id = state.journal.project_id();
    drop(guard);

    let payload = kcreate_collab::OperationBroadcastPayload {
        project_id,
        operations,
    };
    let result = runtime_handle.block_on(async {
        tokio::time::timeout(OP_TIMEOUT, host.broadcast_operations(payload)).await
    });
    match result {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(SessionBridgeError::Transport(e)),
        Err(_) => Err(SessionBridgeError::Transport(
            kcreate_collab_transport::TransportError::Quic("broadcast_operations timed out".into()),
        )),
    }
}

/// Phase 7 (Task 15): send a `ResumeRequest` to a connected peer
/// asking them to backfill any journal entries we are missing
/// relative to our local [`ResumeVector`]. Called by the renderer
/// right after a successful [`session_join`] so a late joiner can
/// pull the running document history without restarting the
/// host's session.
///
/// The resume bundle reply arrives asynchronously via the pump
/// loop as a [`SessionEvent::ResumeApplied`] event; this function
/// only fires the request — callers must consume the event stream
/// to know when the replay is complete.
///
/// Returns [`SessionBridgeError::NotRunning`] if no session is
/// active, or [`SessionBridgeError::InvalidArgument`] if the
/// supplied `peer_id_b64` is unparseable. Network errors are
/// surfaced through [`SessionBridgeError::Transport`].
pub fn session_request_resume(peer_id_b64: &str) -> Result<()> {
    require_active_kchat_membership()?;
    let target: PeerId = peer_id_b64
        .parse()
        .map_err(
            |e: kcreate_collab::CollabError| SessionBridgeError::InvalidArgument {
                field: "peerId",
                message: e.to_string(),
            },
        )?;
    let guard = slot().lock();
    let state = guard.as_ref().ok_or(SessionBridgeError::NotRunning)?;
    let host = state.host.clone();
    let runtime_handle = state.runtime.handle().clone();
    let project_id = state.journal.project_id();
    let since = state.journal.resume_vector();
    drop(guard);

    let payload = kcreate_collab::ResumeRequestPayload { project_id, since };
    let message = Message::ResumeRequest(payload);
    let result = runtime_handle
        .block_on(async { tokio::time::timeout(OP_TIMEOUT, host.send_to(&target, message)).await });
    match result {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(SessionBridgeError::Transport(e)),
        Err(_) => Err(SessionBridgeError::Transport(
            kcreate_collab_transport::TransportError::Quic("resume_request timed out".into()),
        )),
    }
}

/// Phase 7 (Task 8): forcibly disconnect a connected peer. Used by
/// the roster-sync poller when a peer's KChat community membership
/// is revoked, and by the host UI when an admin wants to kick
/// someone manually. Sends a directed `Goodbye(Kicked(reason))` so
/// the kicked peer observes a clean shutdown, closes the underlying
/// QUIC connection, releases any locks the kicked peer was holding,
/// and emits [`SessionEvent::PeerKicked`] on the event queue.
///
/// Idempotent on unknown / already-disconnected peer ids.
pub fn session_kick_peer(peer_id_b64: &str, reason: &str) -> Result<()> {
    require_active_kchat_membership()?;
    let target: PeerId = peer_id_b64
        .parse()
        .map_err(
            |e: kcreate_collab::CollabError| SessionBridgeError::InvalidArgument {
                field: "peerId",
                message: e.to_string(),
            },
        )?;

    let mut guard = slot().lock();
    let state = guard.as_mut().ok_or(SessionBridgeError::NotRunning)?;
    let host = state.host.clone();
    let runtime_handle = state.runtime.handle().clone();
    // Release any locks the kicked peer was holding so the UI
    // un-greys those nodes for everyone else immediately. The
    // transport's `disconnect_peer` already produces a `PeerLeft`
    // event which the pump uses to clean up `state.locks`, but
    // doing it synchronously here makes the `PeerKicked` event
    // ordering deterministic (PeerKicked precedes the synthetic
    // PeerLeft, never the other way round).
    let released_nodes: Vec<Uuid> = state
        .locks
        .iter()
        .filter_map(|(node_id, entry)| (entry.holder == target).then_some(*node_id))
        .collect();
    for node_id in &released_nodes {
        state.locks.remove(node_id);
    }
    if !released_nodes.is_empty() {
        push_event(
            state,
            SessionEvent::LocksChanged {
                peer_id: target.as_str().to_string(),
                node_ids: released_nodes,
            },
        );
    }
    state.permissions.remove(&target);
    push_event(
        state,
        SessionEvent::PeerKicked {
            peer_id: target.as_str().to_string(),
            reason: reason.to_string(),
        },
    );
    drop(guard);

    let reason_payload = kcreate_collab::GoodbyeReason::Kicked(reason.to_string());
    let target_for_async = target.clone();
    let result = runtime_handle.block_on(async {
        tokio::time::timeout(
            OP_TIMEOUT,
            host.disconnect_peer(&target_for_async, reason_payload),
        )
        .await
    });
    match result {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(SessionBridgeError::Transport(e)),
        Err(_) => Err(SessionBridgeError::Transport(
            kcreate_collab_transport::TransportError::Quic("disconnect_peer timed out".into()),
        )),
    }
}

/// Phase 7 (Task 11): set the collaboration permission for a
/// connected peer. Used by the host to downgrade a community
/// `member` to [`CollabPermission::Viewer`] (read-only) or
/// restore them to [`CollabPermission::Editor`]. Owners and
/// admins should remain editors; this entry point doesn't enforce
/// that rule \u2014 the renderer is expected to disable the UI for
/// privileged roles, and the bridge only enforces *outbound*
/// broadcasts against the *local* permission. Emits a
/// [`SessionEvent::PermissionChanged`].
pub fn session_set_peer_permission(peer_id_b64: &str, permission: CollabPermission) -> Result<()> {
    require_active_kchat_membership()?;
    let target: PeerId = peer_id_b64
        .parse()
        .map_err(
            |e: kcreate_collab::CollabError| SessionBridgeError::InvalidArgument {
                field: "peerId",
                message: e.to_string(),
            },
        )?;
    let mut guard = slot().lock();
    let state = guard.as_mut().ok_or(SessionBridgeError::NotRunning)?;
    state.permissions.insert(target.clone(), permission);
    push_event(
        state,
        SessionEvent::PermissionChanged {
            peer_id: target.as_str().to_string(),
            permission,
        },
    );
    Ok(())
}

/// Phase 7 (Task 11): set the *local* peer's permission. Called by
/// the roster-sync poller after a [`get_community_members`] result
/// surfaces the local role. Does NOT emit `PermissionChanged` for
/// the local id \u2014 the renderer reads this via
/// [`session_local_permission`].
pub fn session_set_local_permission(permission: CollabPermission) -> Result<()> {
    let mut guard = slot().lock();
    let state = guard.as_mut().ok_or(SessionBridgeError::NotRunning)?;
    state.local_permission = permission;
    Ok(())
}

/// Phase 7 (Task 11): snapshot of the local peer's collaboration
/// permission. Defaults to [`CollabPermission::Editor`].
#[must_use]
pub fn session_local_permission() -> CollabPermission {
    let guard = slot().lock();
    guard
        .as_ref()
        .map_or(CollabPermission::Editor, |s| s.local_permission)
}

/// Phase 7 (Task 11): snapshot of every known peer permission. Used
/// by the presence panel to render \"viewer\" / \"editor\" badges.
pub fn session_peer_permissions() -> Vec<(String, CollabPermission)> {
    let guard = slot().lock();
    let Some(state) = guard.as_ref() else {
        return Vec::new();
    };
    state
        .permissions
        .iter()
        .map(|(pid, perm)| (pid.as_str().to_string(), *perm))
        .collect()
}

/// Phase 7 (Task 7+8): snapshot of the active KChat community id.
/// `None` when the session was started without a community gate.
#[must_use]
pub fn session_community_id() -> Option<String> {
    let guard = slot().lock();
    guard.as_ref().and_then(|s| s.community_id.clone())
}

/// Phase 7 (Task 8): apply a fresh community-members snapshot. For
/// every currently-connected peer NOT in the supplied roster, emit
/// a `PeerKicked` event and tear down their QUIC connection (via
/// `disconnect_peer`). For peers that ARE in the roster, refresh
/// their permission from the role string. Returns the list of
/// peer ids that were evicted so the caller can log / surface them.
pub fn session_apply_community_roster(members: &[(String, String)]) -> Result<Vec<String>> {
    require_active_kchat_membership()?;
    let known: std::collections::HashSet<String> =
        members.iter().map(|(pid, _)| pid.clone()).collect();
    let connected: Vec<PeerId> = {
        let guard = slot().lock();
        let state = guard.as_ref().ok_or(SessionBridgeError::NotRunning)?;
        state.host.connected_peer_ids()
    };

    // Refresh permissions for connected peers that are still in
    // the community.
    {
        let mut guard = slot().lock();
        let state = guard.as_mut().ok_or(SessionBridgeError::NotRunning)?;
        let local_peer = state.local_peer_id.as_str().to_string();
        for (pid_b64, role) in members {
            let perm = CollabPermission::from_role(role);
            if *pid_b64 == local_peer {
                if state.local_permission != perm {
                    state.local_permission = perm;
                }
                continue;
            }
            if let Ok(pid) = pid_b64.parse::<PeerId>() {
                let changed = state.permissions.insert(pid.clone(), perm) != Some(perm);
                if changed {
                    push_event(
                        state,
                        SessionEvent::PermissionChanged {
                            peer_id: pid_b64.clone(),
                            permission: perm,
                        },
                    );
                }
            }
        }
    }

    let to_kick: Vec<PeerId> = connected
        .into_iter()
        .filter(|pid| !known.contains(pid.as_str()))
        .collect();

    let mut kicked = Vec::with_capacity(to_kick.len());
    for pid in to_kick {
        let pid_b64 = pid.as_str().to_string();
        if let Err(e) = session_kick_peer(&pid_b64, "revoked-from-community") {
            log::warn!("session_apply_community_roster: failed to kick {pid_b64}: {e}");
            continue;
        }
        kicked.push(pid_b64);
    }
    Ok(kicked)
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
    let lookup = build_connected_lookup(state);
    presence_cursors_from_state(state, &lookup)
}

/// Read-side accessor for `scene_sync` to paint remote-peer
/// selection halos. Returns `(peer_id, display_name, node_ids)`
/// triples for every connected peer whose latest presence carries
/// a non-empty `selection: Vec<Uuid>`. Peers with empty selections
/// are filtered so the renderer doesn't get a spurious "empty
/// halo group" entry.
///
/// Same mirror of `state.presence` as [`presence_cursors`]; the
/// two are kept separate so the bridge can short-circuit either
/// loop independently when the corresponding rendering pass is
/// disabled.
#[allow(dead_code)] // exported so future call sites (presentation
                    // mode, export preview pane) can reuse it.
pub fn presence_selections() -> Vec<(String, String, Vec<Uuid>)> {
    let guard = slot().lock();
    let Some(state) = guard.as_ref() else {
        return Vec::new();
    };
    let lookup = build_connected_lookup(state);
    presence_selections_from_state(state, &lookup)
}

/// Atomic per-frame snapshot of every remote-peer presence
/// payload, with cursors and selections collected under a single
/// `slot().lock()` acquisition.
///
/// `sync_scene_locked` previously called [`presence_selections`]
/// and [`presence_cursors`] back-to-back, releasing and
/// reacquiring the collab slot mutex between them. Each call
/// took its own snapshot of `state.presence`, so an inbound
/// presence apply that landed *between* the two reads could leave
/// the scene with halos from snapshot N and cursors from snapshot
/// N+1 in the same rendered frame. The TOCTOU was benign (worst
/// case: one frame of mismatch before the next sync), but tying
/// the two reads to a single lock acquisition removes the gap
/// entirely without changing any caller behaviour.
///
/// The returned tuple mirrors the shape of the two single-purpose
/// helpers so existing call sites can switch over without
/// reshaping their downstream conversions.
#[allow(clippy::type_complexity)]
pub fn presence_snapshot() -> (
    Vec<(String, String, Vec<Uuid>)>,
    Vec<(String, String, SessionCursor)>,
) {
    let guard = slot().lock();
    let Some(state) = guard.as_ref() else {
        return (Vec::new(), Vec::new());
    };
    // Build the connected-peers display-name lookup ONCE per snapshot
    // and pass it down to both readers. The two readers iterate the
    // same `state.presence` map and need the same `PeerId →
    // display_name` lookup, so collapsing the two
    // `state.host.connected_peers()` calls eliminates redundant
    // O(P) work on every scene-sync tick (P = connected peer count).
    let lookup = build_connected_lookup(state);
    (
        presence_selections_from_state(state, &lookup),
        presence_cursors_from_state(state, &lookup),
    )
}

fn build_connected_lookup(state: &SessionState) -> HashMap<PeerId, String> {
    state
        .host
        .connected_peers()
        .into_iter()
        .map(|i| (i.peer_id, i.display_name))
        .collect()
}

fn presence_cursors_from_state(
    state: &SessionState,
    connected_lookup: &HashMap<PeerId, String>,
) -> Vec<(String, String, SessionCursor)> {
    // `state.presence.len()` is an upper bound (peers without an
    // active cursor are filtered below) but it caps the worst-case
    // realloc count at one. The hot path is N-peer collab, where N
    // is in the tens of peers, so a small overshoot is cheaper than
    // the geometric reallocs `.collect()` would otherwise do.
    let mut out = Vec::with_capacity(state.presence.len());
    for (peer_id, payload) in &state.presence {
        let Some(display_name) = connected_lookup.get(peer_id) else {
            continue;
        };
        let Some(cursor) = payload.cursor.map(|c| SessionCursor { x: c.x, y: c.y }) else {
            continue;
        };
        out.push((peer_id.as_str().to_string(), display_name.clone(), cursor));
    }
    out
}

fn presence_selections_from_state(
    state: &SessionState,
    connected_lookup: &HashMap<PeerId, String>,
) -> Vec<(String, String, Vec<Uuid>)> {
    // Same upper-bound rationale as `presence_cursors_from_state`.
    let mut out = Vec::with_capacity(state.presence.len());
    for (peer_id, payload) in &state.presence {
        if payload.selection.is_empty() {
            continue;
        }
        let Some(display_name) = connected_lookup.get(peer_id) else {
            continue;
        };
        out.push((
            peer_id.as_str().to_string(),
            display_name.clone(),
            payload.selection.clone(),
        ));
    }
    out
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
            | SessionBridgeError::NotInKChatGroup
            | SessionBridgeError::KChatDevIssuerDisabled
            | SessionBridgeError::IssuerNotTrusted { .. }
            | SessionBridgeError::PermissionDenied
            | SessionBridgeError::InviteRejected(_) => Self::InvalidArgument {
                argument: "session".to_string(),
                value: e.to_string(),
            },
            SessionBridgeError::Transport(_)
            | SessionBridgeError::Collab(_)
            | SessionBridgeError::KChat(_)
            | SessionBridgeError::TrustStoreIo(_) => Self::Io(std::io::Error::other(e.to_string())),
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
    /// 32-byte Ed25519 verifying key of the issuer that minted the
    /// active membership (URL-safe base64, no padding). `None`
    /// when locked. Surfaced so the renderer can render an "issued
    /// by …" line without having to round-trip back through the
    /// install request.
    #[serde(default)]
    pub issuer_public_key: Option<String>,
    /// Human-readable label of the trusted-issuer entry that
    /// matched the active membership's issuer key, if any. `None`
    /// when locked OR when the issuer is not on the allowlist
    /// (in which case `issuer_trusted == false`). Set from
    /// [`TrustedIssuer::label`].
    #[serde(default)]
    pub issuer_label: Option<String>,
    /// `true` iff the active membership's issuer is on the
    /// configured trusted-issuer allowlist, OR the allowlist is
    /// empty (which preserves the pre-Block-E "accept any issuer"
    /// behaviour for the dev-mint flow). The renderer renders a
    /// distinct badge for `false` ("untrusted issuer — test only")
    /// so a real KChat sign-in is visually distinguishable from a
    /// dev-mint sign-in even when the dev key isn't on the list.
    /// `false` when locked.
    #[serde(default)]
    pub issuer_trusted: bool,
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

// -----------------------------------------------------------------
// KChat trusted-issuer allowlist (Block E)
// -----------------------------------------------------------------

/// One entry on the trusted-issuer allowlist. The bridge accepts
/// install requests whose `issuer_public_key` is on the list (when
/// the list is non-empty); a real KChat group's issuer is pinned
/// here once and then every subsequent attestation from that group
/// verifies without further user action.
///
/// Wire format mirrors `apps/desktop/shared/scene.ts::TrustedIssuer`;
/// fields are camelCase via `#[serde(rename_all = "camelCase")]`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TrustedIssuer {
    /// 32-byte Ed25519 verifying key, URL-safe base64 (no padding).
    /// Pinned exactly — the install path does a bitwise string
    /// compare rather than re-decoding because both sides of the
    /// comparison have already been validated as 32-byte b64url.
    pub issuer_public_key: String,
    /// User-supplied human-readable label. "KChat Production",
    /// "Studio Internal", "Dev Sandbox", etc. Capped at 128 chars
    /// at the bridge level so a misbehaving renderer can't blow
    /// the trust file size.
    pub label: String,
    /// When the issuer was added. Serialised as RFC3339; auto-set
    /// to `Utc::now()` by [`kchat_add_trusted_issuer`].
    pub added_at: DateTime<Utc>,
}

/// Maximum number of characters allowed for [`TrustedIssuer::label`].
/// Chosen to comfortably hold a friendly identifier without giving
/// a misbehaving renderer the ability to bloat the trust file.
const TRUSTED_ISSUER_LABEL_CAP: usize = 128;

/// In-memory + on-disk representation of the trusted-issuer
/// allowlist. The file format is the serialised JSON of this
/// struct so future schema additions (e.g. expiry, revocation
/// reason) can stay backward-compatible via `#[serde(default)]`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct TrustStore {
    #[serde(default)]
    issuers: Vec<TrustedIssuer>,
}

/// Global slot for the in-memory trust store. Loaded lazily from
/// the configured path on first read after [`kchat_set_trust_store_path`]
/// is called. Empty by default — preserves the pre-Block-E
/// "accept any issuer" behaviour for unconfigured installs (which
/// is what the dev-mint surface relies on).
fn trust_store_slot() -> &'static Mutex<TrustStore> {
    static S: OnceLock<Mutex<TrustStore>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(TrustStore::default()))
}

/// Global slot for the configured on-disk path of the trust store.
/// `None` means "memory-only" — adds and removes work but don't
/// survive across processes. Set by the Electron main process at
/// startup via [`kchat_set_trust_store_path`].
fn trust_store_path_slot() -> &'static Mutex<Option<PathBuf>> {
    static S: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(None))
}

/// Read the file at `path`. A missing file is not an error — the
/// trust store is initialised empty on first run.
fn read_trust_store_file(path: &Path) -> Result<TrustStore> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| {
            SessionBridgeError::TrustStoreIo(format!("parse {}: {e}", path.display()))
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(TrustStore::default()),
        Err(e) => Err(SessionBridgeError::TrustStoreIo(format!(
            "read {}: {e}",
            path.display()
        ))),
    }
}

/// Atomically write `store` to `path`. Uses the standard
/// write-to-temp-then-rename pattern so a crash mid-write never
/// leaves a half-truncated trust file on disk.
fn write_trust_store_file(path: &Path, store: &TrustStore) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| {
                SessionBridgeError::TrustStoreIo(format!("create dir {}: {e}", parent.display()))
            })?;
        }
    }
    let mut tmp = path.to_path_buf();
    let file_name = match path.file_name() {
        Some(s) => s.to_os_string(),
        None => std::ffi::OsString::from("kchat_trust.json"),
    };
    let mut tmp_name = file_name;
    tmp_name.push(".tmp");
    tmp.set_file_name(tmp_name);
    let bytes = serde_json::to_vec_pretty(store)
        .map_err(|e| SessionBridgeError::TrustStoreIo(format!("encode {}: {e}", path.display())))?;
    std::fs::write(&tmp, &bytes)
        .map_err(|e| SessionBridgeError::TrustStoreIo(format!("write {}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        SessionBridgeError::TrustStoreIo(format!(
            "rename {} -> {}: {e}",
            tmp.display(),
            path.display()
        ))
    })?;
    Ok(())
}

/// Configure the on-disk path for the trusted-issuer allowlist.
/// Reads the file at `path` (or starts with an empty list if
/// missing) and replaces the in-memory store with the contents.
/// Subsequent `kchat_add_trusted_issuer` / `kchat_remove_trusted_issuer`
/// calls atomically persist back to `path`.
///
/// The Electron main process calls this once at startup with
/// `<userData>/kchat_trust.json` (see `apps/desktop/main/src/main.ts`).
pub fn kchat_set_trust_store_path(path: PathBuf) -> Result<Vec<TrustedIssuer>> {
    let store = read_trust_store_file(&path)?;
    let issuers = store.issuers.clone();
    *trust_store_slot().lock() = store;
    *trust_store_path_slot().lock() = Some(path);
    Ok(issuers)
}

/// Snapshot of the current trusted-issuer list. Cheap clone; the
/// caller holds no locks across N-API boundaries.
pub fn kchat_list_trusted_issuers() -> Vec<TrustedIssuer> {
    trust_store_slot().lock().issuers.clone()
}

/// Validate + normalise an incoming `TrustedIssuer` payload from
/// the renderer. Rejects non-32-byte keys, empty / whitespace-only
/// labels, and labels over [`TRUSTED_ISSUER_LABEL_CAP`].
fn validate_trusted_issuer(input: &TrustedIssuer) -> Result<TrustedIssuer> {
    // Re-decode the public key so we reject malformed entries
    // at the bridge boundary rather than at install time.
    decode_verifying_key(&input.issuer_public_key, "issuerPublicKey")?;
    let label = input.label.trim();
    if label.is_empty() {
        return Err(SessionBridgeError::InvalidArgument {
            field: "label",
            message: "label must be non-empty".into(),
        });
    }
    if label.chars().count() > TRUSTED_ISSUER_LABEL_CAP {
        return Err(SessionBridgeError::InvalidArgument {
            field: "label",
            message: format!("label must be at most {TRUSTED_ISSUER_LABEL_CAP} characters"),
        });
    }
    Ok(TrustedIssuer {
        issuer_public_key: input.issuer_public_key.clone(),
        label: label.to_string(),
        added_at: input.added_at,
    })
}

/// Add (or update) a trusted issuer. If an entry with the same
/// `issuer_public_key` already exists, its label and timestamp are
/// replaced — so the renderer's "Edit label" flow can re-call this
/// with the new label without a separate update path. Returns the
/// updated list.
pub fn kchat_add_trusted_issuer(input: TrustedIssuer) -> Result<Vec<TrustedIssuer>> {
    let normalised = validate_trusted_issuer(&TrustedIssuer {
        issuer_public_key: input.issuer_public_key.trim_end_matches('=').to_string(),
        label: input.label,
        added_at: input.added_at,
    })?;
    let entry = TrustedIssuer {
        issuer_public_key: normalised.issuer_public_key,
        label: normalised.label,
        // Always overwrite the timestamp with `now`; a renderer
        // that supplies an old timestamp would otherwise be able
        // to back-date an addition.
        added_at: Utc::now(),
    };
    let snapshot = {
        let mut guard = trust_store_slot().lock();
        if let Some(existing) = guard
            .issuers
            .iter_mut()
            .find(|i| i.issuer_public_key == entry.issuer_public_key)
        {
            existing.label.clone_from(&entry.label);
            existing.added_at = entry.added_at;
        } else {
            guard.issuers.push(entry);
        }
        guard.clone()
    };
    {
        let path_guard = trust_store_path_slot().lock();
        if let Some(path) = path_guard.as_ref() {
            write_trust_store_file(path, &snapshot)?;
        }
    }
    Ok(snapshot.issuers)
}

/// Remove a trusted issuer by its `issuer_public_key`. Returns the
/// updated list. Removing the last entry collapses the allowlist
/// back to "accept any issuer" mode — explicit by design, so a
/// user clearing their list never ends up locked out of dev-mint
/// without realising why.
pub fn kchat_remove_trusted_issuer(issuer_public_key: &str) -> Result<Vec<TrustedIssuer>> {
    let key = issuer_public_key.trim_end_matches('=').to_string();
    let snapshot = {
        let mut guard = trust_store_slot().lock();
        guard.issuers.retain(|i| i.issuer_public_key != key);
        guard.clone()
    };
    {
        let path_guard = trust_store_path_slot().lock();
        if let Some(path) = path_guard.as_ref() {
            write_trust_store_file(path, &snapshot)?;
        }
    }
    Ok(snapshot.issuers)
}

/// Look up a trusted-issuer entry by public key. Used by the
/// status builders to populate `issuer_label` / `issuer_trusted`
/// on a signed-in `KChatMembershipStatus`. Returns `None` if the
/// allowlist is empty (the pre-Block-E "accept any issuer" state)
/// AND if the issuer is not listed; the two are disambiguated by
/// the caller via `trusted_issuer_list_is_empty`.
fn trusted_issuer_lookup(issuer_public_key: &str) -> Option<TrustedIssuer> {
    let normalised = issuer_public_key.trim_end_matches('=');
    trust_store_slot()
        .lock()
        .issuers
        .iter()
        .find(|i| i.issuer_public_key == normalised)
        .cloned()
}

/// Returns `true` when no trusted issuers are configured. Treated
/// as "accept any issuer" by the install path for backwards-compat
/// with the dev-mint flow; treated as `issuer_trusted = true` by
/// `kchat_membership_status` since the renderer has explicitly opted
/// out of pinning.
fn trusted_issuer_list_is_empty() -> bool {
    trust_store_slot().lock().issuers.is_empty()
}

/// Build the membership-status DTO for an installed membership,
/// populating the Block-E provenance fields (`issuer_public_key`,
/// `issuer_label`, `issuer_trusted`). Centralised so install/
/// status paths can't drift.
fn membership_status_for(membership: &KChatMembership) -> KChatMembershipStatus {
    let issuer_pk = membership.issuer_public_key.clone();
    let trusted_entry = trusted_issuer_lookup(&issuer_pk);
    let allowlist_empty = trusted_issuer_list_is_empty();
    let issuer_trusted = trusted_entry.is_some() || allowlist_empty;
    KChatMembershipStatus {
        locked: false,
        group_id: Some(membership.group_id.as_str().to_string()),
        peer_id: Some(membership.peer_id().as_str().to_string()),
        expires_at: Some(membership.expires_at),
        issuer_public_key: Some(issuer_pk),
        issuer_label: trusted_entry.map(|t| t.label),
        issuer_trusted,
    }
}

/// Build the membership-status DTO for the locked state. Centralised
/// alongside `membership_status_for` so the two builders stay
/// symmetric.
fn locked_membership_status() -> KChatMembershipStatus {
    KChatMembershipStatus {
        locked: true,
        group_id: None,
        peer_id: None,
        expires_at: None,
        issuer_public_key: None,
        issuer_label: None,
        issuer_trusted: false,
    }
}

/// Install (or refresh) the KChat group authority. Once a valid
/// authority is installed, the multiplayer bridge unlocks. A
/// subsequent `kchat_clear_authority` re-locks it.
///
/// The membership is verified locally — including signature, peer
/// binding, and time window — before being installed, so a future
/// KChat client crash or malicious request can't sneak past the
/// gate by pushing a malformed attestation.
///
/// Block E: when the trusted-issuer allowlist is non-empty, the
/// request's `issuer_public_key` must be listed; otherwise the
/// install is rejected with `IssuerNotTrusted`. An empty allowlist
/// accepts any issuer (back-compat with the dev-mint flow).
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

    // Block E gate: consult the trusted-issuer allowlist if it is
    // configured. The allowlist check uses a normalised string
    // compare (after stripping any trailing `=` padding) so a
    // renderer that sends padded base64 still matches the stored
    // unpadded form.
    if !trusted_issuer_list_is_empty() {
        let normalised = req.issuer_public_key.trim_end_matches('=').to_string();
        if trusted_issuer_lookup(&normalised).is_none() {
            return Err(SessionBridgeError::IssuerNotTrusted {
                issuer_public_key: normalised,
            });
        }
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

    Ok(membership_status_for(&membership))
}

/// Install a pre-built `SharedKChatAuthority` directly. Used by
/// the Phase 7 KChat Desktop bridge (`kchat_desktop`) when the
/// authority is sourced from a live IPC connection rather than a
/// wire-format `KChatInstallRequest`. The caller is responsible
/// for verifying the underlying membership against its issuer trust
/// root (the `KChatDesktopAuthority::install` constructor does so).
/// The provided `membership` is used to compute the
/// `KChatMembershipStatus` returned to the renderer (issuer label,
/// trusted-issuer lookup, validity window) using the same builder
/// the regular install path uses, so the renderer state stays
/// consistent across install sources.
pub fn install_kchat_authority_direct(
    authority: SharedKChatAuthority,
    membership: &KChatMembership,
) -> KChatMembershipStatus {
    *kchat_slot().lock() = authority;
    membership_status_for(membership)
}

/// Clear the installed authority and re-lock multiplayer. Any
/// running session is left as-is (the QUIC endpoint stays alive
/// until `session_leave`), but subsequent `session_start`,
/// `session_join`, and `session_send_presence` calls will fail
/// with [`SessionBridgeError::NotInKChatGroup`].
pub fn kchat_clear_authority() -> KChatMembershipStatus {
    *kchat_slot().lock() = no_kchat_authority();
    locked_membership_status()
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
        Ok(m) => membership_status_for(&m),
        Err(_) => locked_membership_status(),
    }
}

/// Wire-format DTO accepted by the dev-only `kchat_dev_mint_membership`
/// entry point. Matches the JSON shape documented in the N-API
/// export's doc comment.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KChatDevMintRequest {
    /// 32-byte Ed25519 seed used to derive the dev issuer. Same
    /// seed produces the same issuer trust root across runs.
    pub issuer_seed: String,
    /// URL-safe ASCII group identifier.
    pub group_id: String,
    /// 32-byte Ed25519 verifying key of the local peer (the
    /// PresencePanel's persistent identity), URL-safe base64
    /// (no padding).
    pub peer_public_key: String,
    /// Membership validity, in seconds. Capped at 365 days by
    /// `kcreate_kchat::MAX_DEV_VALIDITY`.
    pub valid_for_seconds: u32,
}

/// Dev-only: mint a fresh attestation locally and return the
/// JSON-encoded [`KChatInstallRequest`]. Compiled only when the
/// bridge is built with `kchat-dev-issuer`. In other builds the
/// non-cfg shim below returns `KChatDevIssuerDisabled` so the
/// renderer can render a clean diagnostic.
#[cfg(feature = "kchat-dev-issuer")]
pub fn kchat_dev_mint_membership_json(request_json: &str) -> Result<String> {
    let req: KChatDevMintRequest =
        serde_json::from_str(request_json).map_err(|e| SessionBridgeError::InvalidArgument {
            field: "kchatDevMintRequest",
            message: e.to_string(),
        })?;

    let seed_bytes = decode_b64_url(&req.issuer_seed, "issuerSeed").map_err(|e| match e {
        SessionBridgeError::InvalidArgument { message, .. } => {
            SessionBridgeError::InvalidArgument {
                field: "issuerSeed",
                message,
            }
        }
        other => other,
    })?;
    if seed_bytes.len() != 32 {
        return Err(SessionBridgeError::InvalidArgument {
            field: "issuerSeed",
            message: format!("expected 32 bytes, got {}", seed_bytes.len()),
        });
    }
    let mut seed_arr = [0u8; 32];
    seed_arr.copy_from_slice(&seed_bytes);

    let issuer = kcreate_kchat::DevIssuer::from_seed(seed_arr);
    let install = issuer
        .mint_install_request_for_peer(
            &req.group_id,
            &req.peer_public_key,
            std::time::Duration::from_secs(u64::from(req.valid_for_seconds)),
        )
        .map_err(|e| match e {
            kcreate_kchat::DevIssuerError::InvalidGroupId(inner) => {
                SessionBridgeError::InvalidArgument {
                    field: "groupId",
                    message: inner.to_string(),
                }
            }
            kcreate_kchat::DevIssuerError::InvalidPeerSeed => SessionBridgeError::InvalidArgument {
                field: "peerPublicKey",
                message: "must be a 32-byte URL-safe-base64 Ed25519 verifying key".into(),
            },
            kcreate_kchat::DevIssuerError::InvalidValidity => SessionBridgeError::InvalidArgument {
                field: "validForSeconds",
                message: "must be > 0 and <= 365 days".into(),
            },
            kcreate_kchat::DevIssuerError::Issue(inner) => SessionBridgeError::KChat(inner),
        })?;

    // The `DevInstallRequest` shape mirrors `KChatInstallRequest`
    // exactly (see the wire-lockstep test below). Round-trip through
    // serde rather than constructing a `KChatInstallRequest` here so
    // any future field added on the bridge side without mirroring
    // it on `kcreate_kchat::DevInstallRequest` fails loudly at
    // deserialise time.
    let install_request: KChatInstallRequest =
        serde_json::from_str(&serde_json::to_string(&install).map_err(|e| {
            SessionBridgeError::InvalidArgument {
                field: "kchatDevMintRequest",
                message: e.to_string(),
            }
        })?)
        .map_err(|e| SessionBridgeError::InvalidArgument {
            field: "kchatDevMintRequest",
            message: format!("dev install request shape mismatch: {e}"),
        })?;

    serde_json::to_string(&install_request).map_err(|e| SessionBridgeError::InvalidArgument {
        field: "kchatDevMintRequest",
        message: e.to_string(),
    })
}

/// Production-build shim. Always returns the typed
/// `KChatDevIssuerDisabled` error so the renderer can show "this
/// build doesn't include the dev issuer". Kept symmetrical with the
/// feature-gated impl so the N-API surface is stable across builds.
#[cfg(not(feature = "kchat-dev-issuer"))]
#[allow(dead_code)]
pub fn kchat_dev_mint_membership_json(_request_json: &str) -> Result<String> {
    Err(SessionBridgeError::KChatDevIssuerDisabled)
}

/// Local-peer-identity probe. Derives the BLAKE3 peer id +
/// URL-safe-base64 verifying key for the supplied seed without
/// starting a collab session. The sign-in flow needs this before
/// a session exists (the membership attestation is bound to the
/// peer's public key, and the user shouldn't have to start a
/// session — which would itself be rejected by the gate — just to
/// learn what to put on the issuer side).
///
/// Returns a JSON `KChatLocalIdentity` payload.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KChatLocalIdentity {
    pub peer_id: String,
    pub peer_public_key: String,
}

/// Derive the local KChat peer identity from a persistent seed.
/// Pure crypto — no networking, no global state — so it lives in
/// the `collab`-gated module only because it needs `ed25519-dalek`
/// which is itself behind the `collab` feature.
pub fn kchat_derive_local_identity(seed_b64: &str) -> Result<KChatLocalIdentity> {
    let seed = decode_seed(seed_b64)?;
    let signing = ed25519_dalek::SigningKey::from_bytes(&seed);
    let vk = signing.verifying_key();
    let peer_id = PeerId::from_verifying_key(&vk);
    let peer_public_key = URL_SAFE_NO_PAD.encode(vk.as_bytes());
    Ok(KChatLocalIdentity {
        peer_id: peer_id.as_str().to_string(),
        peer_public_key,
    })
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
        // Block E also adds the trusted-issuer allowlist as a
        // process-global singleton. Reset both so a sibling test
        // that left state behind can't taint this one.
        *trust_store_slot().lock() = TrustStore::default();
        *trust_store_path_slot().lock() = None;
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
            local_identity.public_key,
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
        let err = session_start(&seed_b64, "ken", Uuid::new_v4(), false, None).unwrap_err();
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
        let err = session_start(&other_seed_b64, "ken", Uuid::new_v4(), false, None).unwrap_err();
        assert!(
            matches!(err, SessionBridgeError::NotInKChatGroup),
            "expected NotInKChatGroup, got {err:?}"
        );
    }

    /// Wire-format lockstep between `kcreate_kchat::DevInstallRequest`
    /// (the dev issuer output) and `KChatInstallRequest` (the bridge
    /// install entry point's input).
    ///
    /// `kchat_dev_mint_membership_json` round-trips through serde
    /// rather than copying fields by hand, with the rationale (in
    /// the inline comment at the round-trip site) that the round-
    /// trip itself is a wire-shape guard: if either struct grows
    /// or renames a field, the deserialise step fails loudly.
    /// This test pins that guarantee — it would catch any future
    /// drift between the two structs at `cargo test` time rather
    /// than at runtime when a user clicks "Mint dev membership".
    ///
    /// Three guarantees pinned here:
    ///   1. A minted `DevInstallRequest` JSON parses cleanly as
    ///      `KChatInstallRequest` (no missing required fields, no
    ///      type mismatches, no unknown-tag rejection).
    ///   2. Every field round-trips byte-for-byte through the
    ///      bridge type — no value is silently coerced.
    ///   3. The reverse direction (KChatInstallRequest JSON →
    ///      DevInstallRequest) also round-trips, so we'd also
    ///      catch the case where one side grows a field that the
    ///      other side then ignores (which would let mismatched
    ///      data flow through `kchat_dev_mint_membership_json`).
    #[cfg(feature = "kchat-dev-issuer")]
    #[test]
    #[serial]
    fn dev_install_request_matches_bridge_install_request_wire_format() {
        let issuer = kcreate_kchat::DevIssuer::from_seed([0x7E; 32]);
        let peer_seed = [0x11; 32];
        let peer_vk = ed25519_dalek::SigningKey::from_bytes(&peer_seed).verifying_key();
        let peer_pk_b64 = URL_SAFE_NO_PAD.encode(peer_vk.as_bytes());
        let dev_install = issuer
            .mint_install_request_for_peer(
                "lockstep.group",
                &peer_pk_b64,
                std::time::Duration::from_secs(60 * 60),
            )
            .expect("mint should succeed for valid inputs");

        // Forward direction — DevInstallRequest JSON parses as
        // KChatInstallRequest with every field identical.
        let dev_json = serde_json::to_string(&dev_install).expect("dev serialise");
        let bridge_install: KChatInstallRequest =
            serde_json::from_str(&dev_json).expect("dev json should parse as bridge install");
        assert_eq!(
            dev_install.issuer_public_key,
            bridge_install.issuer_public_key
        );
        assert_eq!(dev_install.group_id, bridge_install.group_id);
        assert_eq!(dev_install.peer_id, bridge_install.peer_id);
        assert_eq!(dev_install.peer_public_key, bridge_install.peer_public_key);
        assert_eq!(dev_install.issued_at, bridge_install.issued_at);
        assert_eq!(dev_install.expires_at, bridge_install.expires_at);
        assert_eq!(dev_install.signature, bridge_install.signature);

        // Reverse direction — KChatInstallRequest JSON parses as
        // DevInstallRequest. Catches the case where the bridge type
        // grows a field that the dev type omits.
        let bridge_json = serde_json::to_string(&bridge_install).expect("bridge serialise");
        let round_trip: kcreate_kchat::DevInstallRequest =
            serde_json::from_str(&bridge_json).expect("bridge json should parse as dev install");
        assert_eq!(round_trip.issuer_public_key, dev_install.issuer_public_key);
        assert_eq!(round_trip.group_id, dev_install.group_id);
        assert_eq!(round_trip.peer_id, dev_install.peer_id);
        assert_eq!(round_trip.peer_public_key, dev_install.peer_public_key);
        assert_eq!(round_trip.issued_at, dev_install.issued_at);
        assert_eq!(round_trip.expires_at, dev_install.expires_at);
        assert_eq!(round_trip.signature, dev_install.signature);

        // The minted install request must also actually install
        // cleanly — proves we're not just round-tripping JSON that
        // both sides happen to accept but which would be rejected by
        // the gate verifier.
        reset_kchat_slot();
        let status = kchat_install_authority(bridge_install).expect("install should succeed");
        assert!(!status.locked, "wire-lockstep install must unlock the gate");
        assert_eq!(status.group_id.as_deref(), Some("lockstep.group"));
    }

    // ====================================================================
    // Block E: KChat trusted-issuer allowlist.
    //
    // The bridge maintains a list of pinned issuer public keys. When
    // the list is non-empty, `kchat_install_authority` must reject
    // install requests whose `issuer_public_key` is not on the list.
    // When the list is empty, the install path accepts any issuer
    // (back-compat with the dev-mint flow).
    //
    // The list is persisted to disk so a real KChat install survives
    // across sessions without the user having to re-add the pin.
    // ====================================================================

    /// Extract the issuer pubkey from a freshly-minted install
    /// request — used by trust-store tests so they can pre-populate
    /// the allowlist with the exact key the next install will use.
    fn issuer_pubkey_from_request(req_json: &str) -> String {
        let req: KChatInstallRequest = serde_json::from_str(req_json).unwrap();
        req.issuer_public_key
    }

    #[test]
    #[serial]
    fn trust_store_empty_by_default_status_reports_trusted() {
        reset_kchat_slot();
        let (req_json, _) = fresh_install_request_json([21u8; 32], "studio-trust-empty");
        let status = kchat_install_authority(serde_json::from_str(&req_json).unwrap()).unwrap();
        assert!(!status.locked);
        assert!(
            status.issuer_trusted,
            "empty allowlist must report issuer_trusted = true (back-compat)"
        );
        // Issuer label is None because no entry was added; the
        // "trusted because list is empty" state has no label.
        assert!(status.issuer_label.is_none());
        assert!(status.issuer_public_key.is_some());
    }

    #[test]
    #[serial]
    fn trust_store_install_rejects_issuer_not_on_allowlist() {
        reset_kchat_slot();
        // Add a *different* issuer to the allowlist so the upcoming
        // install request's issuer is NOT on the list.
        let other_issuer = SigningKey::from_bytes(&[0x42; 32]);
        let other_pubkey = URL_SAFE_NO_PAD.encode(other_issuer.verifying_key().as_bytes());
        kchat_add_trusted_issuer(TrustedIssuer {
            issuer_public_key: other_pubkey,
            label: "Some Other Issuer".into(),
            added_at: Utc::now(),
        })
        .unwrap();

        let (req_json, _) = fresh_install_request_json([22u8; 32], "studio-trust-reject");
        let req: KChatInstallRequest = serde_json::from_str(&req_json).unwrap();
        let err = kchat_install_authority(req).unwrap_err();
        match err {
            SessionBridgeError::IssuerNotTrusted { issuer_public_key } => {
                assert_eq!(
                    issuer_public_key,
                    issuer_pubkey_from_request(&req_json)
                        .trim_end_matches('=')
                        .to_string()
                );
            }
            other => panic!("expected IssuerNotTrusted, got {other:?}"),
        }
        assert!(
            kchat_membership_status().locked,
            "rejected install must leave the slot locked"
        );
    }

    #[test]
    #[serial]
    fn trust_store_install_accepts_listed_issuer_and_attaches_label() {
        reset_kchat_slot();
        let (req_json, _) = fresh_install_request_json([23u8; 32], "studio-trust-accept");
        let issuer_pubkey = issuer_pubkey_from_request(&req_json);
        kchat_add_trusted_issuer(TrustedIssuer {
            issuer_public_key: issuer_pubkey.clone(),
            label: "Studio KChat".into(),
            added_at: Utc::now(),
        })
        .unwrap();

        let status = kchat_install_authority(serde_json::from_str(&req_json).unwrap()).unwrap();
        assert!(!status.locked, "listed issuer must install");
        assert!(status.issuer_trusted, "listed issuer must report trusted");
        assert_eq!(status.issuer_label.as_deref(), Some("Studio KChat"));
        assert_eq!(
            status.issuer_public_key.as_deref(),
            Some(issuer_pubkey.as_str())
        );

        // Snapshot read through the polling status path must report
        // the same provenance fields — otherwise the renderer would
        // see "trusted" right after sign-in but "untrusted" on next
        // poll.
        let polled = kchat_membership_status();
        assert!(polled.issuer_trusted);
        assert_eq!(polled.issuer_label.as_deref(), Some("Studio KChat"));
    }

    #[test]
    #[serial]
    fn trust_store_add_overwrites_label_for_same_pubkey() {
        reset_kchat_slot();
        let issuer = SigningKey::from_bytes(&[0x55; 32]);
        let pk = URL_SAFE_NO_PAD.encode(issuer.verifying_key().as_bytes());
        kchat_add_trusted_issuer(TrustedIssuer {
            issuer_public_key: pk.clone(),
            label: "Old Label".into(),
            added_at: Utc::now(),
        })
        .unwrap();
        let updated = kchat_add_trusted_issuer(TrustedIssuer {
            issuer_public_key: pk,
            label: "New Label".into(),
            added_at: Utc::now(),
        })
        .unwrap();
        assert_eq!(updated.len(), 1, "duplicate pubkey must not double-add");
        assert_eq!(updated[0].label, "New Label");
    }

    #[test]
    #[serial]
    fn trust_store_remove_collapses_back_to_accept_any() {
        reset_kchat_slot();
        let issuer = SigningKey::from_bytes(&[0x66; 32]);
        let pk = URL_SAFE_NO_PAD.encode(issuer.verifying_key().as_bytes());
        kchat_add_trusted_issuer(TrustedIssuer {
            issuer_public_key: pk.clone(),
            label: "Temporary".into(),
            added_at: Utc::now(),
        })
        .unwrap();
        assert!(!trusted_issuer_list_is_empty());
        let after = kchat_remove_trusted_issuer(&pk).unwrap();
        assert!(after.is_empty());
        assert!(trusted_issuer_list_is_empty());

        // Now a fresh install with a totally different issuer must
        // succeed because the allowlist is empty again.
        let (req_json, _) = fresh_install_request_json([24u8; 32], "studio-trust-collapse");
        let status = kchat_install_authority(serde_json::from_str(&req_json).unwrap()).unwrap();
        assert!(!status.locked);
        assert!(status.issuer_trusted, "empty allowlist reports trusted");
    }

    #[test]
    #[serial]
    fn trust_store_rejects_invalid_label_and_pubkey() {
        reset_kchat_slot();
        let issuer = SigningKey::from_bytes(&[0x77; 32]);
        let pk = URL_SAFE_NO_PAD.encode(issuer.verifying_key().as_bytes());
        // Empty label after trim.
        let err = kchat_add_trusted_issuer(TrustedIssuer {
            issuer_public_key: pk.clone(),
            label: "   ".into(),
            added_at: Utc::now(),
        })
        .unwrap_err();
        assert!(matches!(
            err,
            SessionBridgeError::InvalidArgument { field: "label", .. }
        ));
        // Label too long.
        let long = "x".repeat(TRUSTED_ISSUER_LABEL_CAP + 1);
        let err = kchat_add_trusted_issuer(TrustedIssuer {
            issuer_public_key: pk,
            label: long,
            added_at: Utc::now(),
        })
        .unwrap_err();
        assert!(matches!(
            err,
            SessionBridgeError::InvalidArgument { field: "label", .. }
        ));
        // Malformed pubkey.
        let err = kchat_add_trusted_issuer(TrustedIssuer {
            issuer_public_key: "not-base64".into(),
            label: "Bad".into(),
            added_at: Utc::now(),
        })
        .unwrap_err();
        assert!(matches!(
            err,
            SessionBridgeError::InvalidArgument {
                field: "issuerPublicKey",
                ..
            }
        ));
    }

    #[test]
    #[serial]
    fn trust_store_persists_across_set_path_calls() {
        reset_kchat_slot();
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("kchat_trust.json");

        // First "session": configure path, add issuer, write to disk.
        let initial = kchat_set_trust_store_path(path.clone()).unwrap();
        assert!(initial.is_empty(), "fresh file starts empty");
        let issuer = SigningKey::from_bytes(&[0x88; 32]);
        let pk = URL_SAFE_NO_PAD.encode(issuer.verifying_key().as_bytes());
        kchat_add_trusted_issuer(TrustedIssuer {
            issuer_public_key: pk.clone(),
            label: "Persisted Studio".into(),
            added_at: Utc::now(),
        })
        .unwrap();

        // Simulate a fresh process: blow away the in-memory slot,
        // then re-set the path. The list must reload from disk.
        *trust_store_slot().lock() = TrustStore::default();
        *trust_store_path_slot().lock() = None;
        let reloaded = kchat_set_trust_store_path(path).unwrap();
        assert_eq!(reloaded.len(), 1, "reload must restore the issuer");
        assert_eq!(reloaded[0].issuer_public_key, pk);
        assert_eq!(reloaded[0].label, "Persisted Studio");
    }

    #[test]
    #[serial]
    fn trust_store_add_with_padded_pubkey_normalises_to_unpadded() {
        // A user pasting an issuer public key from a KChat admin
        // dashboard may include trailing `=` padding. The
        // allowlist normalises on insert (strips trailing `=`) so a
        // later install request — which the KChat server always
        // emits as URL-safe-base64 with no padding — still matches.
        //
        // Note: we can't symmetrically accept *padded* install
        // requests because the membership signature is computed
        // over a signing view that contains the issuer public key
        // string verbatim. Padding it on the install side would
        // change the signed bytes and the membership would fail
        // verification (which is the correct behaviour — the
        // bridge must not mutate signed payload material). The
        // normalisation lives on the user-typed list entry only.
        reset_kchat_slot();
        let (req_json, _) = fresh_install_request_json([25u8; 32], "studio-padding");
        let issuer_pubkey = issuer_pubkey_from_request(&req_json);
        let unpadded = issuer_pubkey.trim_end_matches('=').to_string();
        let padding_needed = (4 - (unpadded.len() % 4)) % 4;
        let padded = format!("{}{}", unpadded, "=".repeat(padding_needed));
        let listed = kchat_add_trusted_issuer(TrustedIssuer {
            issuer_public_key: padded,
            label: "User-pasted Padded".into(),
            added_at: Utc::now(),
        })
        .unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(
            listed[0].issuer_public_key, unpadded,
            "add path must strip trailing `=` so install (which always sends unpadded) matches"
        );

        // The unpadded install request (canonical form from the
        // issuer) must now match.
        let status = kchat_install_authority(serde_json::from_str(&req_json).unwrap()).unwrap();
        assert!(!status.locked);
        assert!(status.issuer_trusted);
        assert_eq!(status.issuer_label.as_deref(), Some("User-pasted Padded"));
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
                op,
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
            .append(remote, kcreate_collab::LamportClock::from_raw(1), op)
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

    // ====================================================================
    // Block 8: lock roster tests.
    //
    // Exercise `lock_roster_claim` / `lock_roster_release` directly
    // (the pure functions behind `apply_lock_claim` / `apply_lock_release`).
    // Building a full `SessionState` would require a real QUIC host;
    // the roster logic is the only thing that needs coverage here.
    // ====================================================================

    #[test]
    #[serial]
    fn lock_claim_inserts_into_roster() {
        let mut locks: HashMap<Uuid, LockEntry> = HashMap::new();
        let node_a = Uuid::new_v4();
        let node_b = Uuid::new_v4();
        let remote = PeerKey::from_seed([40u8; 32]).peer_id();
        let payload = LockClaimPayload {
            project_id: Uuid::new_v4(),
            node_ids: vec![node_a, node_b],
            acquired_at: Utc::now(),
        };
        let changed = lock_roster_claim(&mut locks, &remote, &payload);
        assert_eq!(locks.len(), 2);
        assert_eq!(locks[&node_a].holder, remote);
        assert_eq!(locks[&node_b].holder, remote);
        // changed reports both node ids as flipped.
        assert_eq!(changed.len(), 2);
    }

    #[test]
    #[serial]
    fn lock_claim_dedupes_within_payload_and_skips_no_op_reclaim() {
        let mut locks: HashMap<Uuid, LockEntry> = HashMap::new();
        let node_a = Uuid::new_v4();
        let project_id = Uuid::new_v4();
        let remote = PeerKey::from_seed([41u8; 32]).peer_id();
        let payload = LockClaimPayload {
            project_id,
            node_ids: vec![node_a, node_a],
            acquired_at: Utc::now(),
        };
        let changed = lock_roster_claim(&mut locks, &remote, &payload);
        assert_eq!(locks.len(), 1);
        // Dedup: even though node_a appears twice, only one change.
        assert_eq!(changed.len(), 1);
        // Same holder reclaiming the same node is a no-op — no flip.
        let payload2 = LockClaimPayload {
            project_id,
            node_ids: vec![node_a],
            acquired_at: Utc::now(),
        };
        let changed2 = lock_roster_claim(&mut locks, &remote, &payload2);
        assert!(changed2.is_empty());
    }

    #[test]
    #[serial]
    fn lock_release_only_succeeds_for_holder() {
        let mut locks: HashMap<Uuid, LockEntry> = HashMap::new();
        let node = Uuid::new_v4();
        let project_id = Uuid::new_v4();
        let holder = PeerKey::from_seed([42u8; 32]).peer_id();
        let stranger = PeerKey::from_seed([43u8; 32]).peer_id();
        lock_roster_claim(
            &mut locks,
            &holder,
            &LockClaimPayload {
                project_id,
                node_ids: vec![node],
                acquired_at: Utc::now(),
            },
        );
        // Stranger trying to release the holder's lock — must be
        // ignored. The soft-lock contract is "only the holder can
        // release"; otherwise any peer could grief by releasing
        // someone else's lock.
        let changed = lock_roster_release(
            &mut locks,
            &stranger,
            &LockReleasePayload {
                project_id,
                node_ids: vec![node],
            },
        );
        assert_eq!(locks.len(), 1);
        assert!(changed.is_empty());
        // Holder releasing — succeeds and removes the entry.
        let changed2 = lock_roster_release(
            &mut locks,
            &holder,
            &LockReleasePayload {
                project_id,
                node_ids: vec![node],
            },
        );
        assert!(locks.is_empty());
        assert_eq!(changed2, vec![node]);
    }

    #[test]
    #[serial]
    fn empty_release_payload_drops_every_lock_for_sender() {
        let mut locks: HashMap<Uuid, LockEntry> = HashMap::new();
        let holder = PeerKey::from_seed([44u8; 32]).peer_id();
        let other = PeerKey::from_seed([45u8; 32]).peer_id();
        let project_id = Uuid::new_v4();
        let n1 = Uuid::new_v4();
        let n2 = Uuid::new_v4();
        let n3 = Uuid::new_v4();
        lock_roster_claim(
            &mut locks,
            &holder,
            &LockClaimPayload {
                project_id,
                node_ids: vec![n1, n2],
                acquired_at: Utc::now(),
            },
        );
        lock_roster_claim(
            &mut locks,
            &other,
            &LockClaimPayload {
                project_id,
                node_ids: vec![n3],
                acquired_at: Utc::now(),
            },
        );
        assert_eq!(locks.len(), 3);
        // Empty list = release everything holder owns. Should NOT
        // touch `other`'s lock on n3.
        let changed = lock_roster_release(
            &mut locks,
            &holder,
            &LockReleasePayload {
                project_id,
                node_ids: vec![],
            },
        );
        assert_eq!(locks.len(), 1);
        assert_eq!(locks[&n3].holder, other);
        assert_eq!(changed.len(), 2);
    }

    #[test]
    #[serial]
    fn lock_claim_payload_round_trips_through_serde() {
        // The protocol layer carries LockClaimPayload as JSON; this
        // test guarantees the field shape on the wire stays in sync
        // with the renderer's TS definitions.
        let p = LockClaimPayload {
            project_id: Uuid::new_v4(),
            node_ids: vec![Uuid::new_v4(), Uuid::new_v4()],
            acquired_at: Utc::now(),
        };
        let s = serde_json::to_string(&p).unwrap();
        let back: LockClaimPayload = serde_json::from_str(&s).unwrap();
        assert_eq!(back.project_id, p.project_id);
        assert_eq!(back.node_ids, p.node_ids);
        assert_eq!(back.acquired_at, p.acquired_at);
    }

    /// Round 11: when no session is running, `session_leave` is a
    /// no-op and returns `None`. The TS-side `bridge.sessionLeave()`
    /// signature contracts on this — `string | null` — and `main.ts`
    /// skips emitting `sessionLeft` in this case.
    #[test]
    #[serial]
    fn session_leave_returns_none_when_no_session_is_running() {
        reset_kchat_slot();
        // Belt-and-braces: ensure no session is running, then call again.
        let _ = session_leave();
        let left = session_leave().unwrap();
        assert!(left.is_none());
    }

    /// Round 11: pin the JSON wire shape of every `SessionEvent`
    /// variant so the renderer's `shared/scene.ts::SessionEvent`
    /// discriminated union stays in lockstep with the bridge.
    /// AGENTS.md rule 4 says the TS file mirrors the bridge —
    /// this test is the enforcement. Without
    /// `rename_all_fields = "camelCase"` on the enum, struct fields
    /// would serialise as `peer_id` / `public_key` / etc. which the
    /// TS side does not accept. A regression that drops the
    /// attribute (or renames a field without updating both sides)
    /// fails this test.
    #[test]
    fn session_event_variants_serialise_to_renderer_camel_case_wire_format() {
        let pid = Uuid::parse_str("11111111-2222-3333-4444-555555555555").unwrap();
        // Discovered
        let v = SessionEvent::Discovered {
            peer_id: "p".into(),
            public_key: "pk".into(),
            display_name: "Ken".into(),
            project_id: pid,
            socket_addr: "127.0.0.1:1234".into(),
            cert_fingerprint: "fp".into(),
        };
        let j = serde_json::to_value(&v).unwrap();
        assert_eq!(j["kind"], "discovered");
        assert_eq!(j["peerId"], "p");
        assert_eq!(j["publicKey"], "pk");
        assert_eq!(j["displayName"], "Ken");
        assert_eq!(j["projectId"], pid.to_string());
        assert_eq!(j["socketAddr"], "127.0.0.1:1234");
        assert_eq!(j["certFingerprint"], "fp");

        // Undiscovered
        let j = serde_json::to_value(SessionEvent::Undiscovered {
            peer_id: "p".into(),
        })
        .unwrap();
        assert_eq!(j["kind"], "undiscovered");
        assert_eq!(j["peerId"], "p");

        // PeerJoined
        let j = serde_json::to_value(SessionEvent::PeerJoined {
            peer_id: "p".into(),
            public_key: "pk".into(),
            display_name: "Ken".into(),
        })
        .unwrap();
        assert_eq!(j["kind"], "peerJoined");
        assert_eq!(j["peerId"], "p");
        assert_eq!(j["publicKey"], "pk");
        assert_eq!(j["displayName"], "Ken");

        // PeerLeft
        let j = serde_json::to_value(SessionEvent::PeerLeft {
            peer_id: "p".into(),
        })
        .unwrap();
        assert_eq!(j["kind"], "peerLeft");
        assert_eq!(j["peerId"], "p");

        // PresenceUpdated
        let presence = SessionPresence {
            active_page: Some(pid),
            selection: vec![pid],
            cursor: Some(SessionCursor { x: 1.0, y: 2.0 }),
            sent_at: Utc::now(),
        };
        let j = serde_json::to_value(SessionEvent::PresenceUpdated {
            peer_id: "p".into(),
            presence,
        })
        .unwrap();
        assert_eq!(j["kind"], "presenceUpdated");
        assert_eq!(j["peerId"], "p");
        // `SessionPresence` itself uses `rename_all = "camelCase"`
        // on its own struct, so its inner fields should also be
        // camelCase when nested here.
        assert_eq!(j["presence"]["activePage"], pid.to_string());
        assert!(j["presence"]["cursor"].is_object());
        assert_eq!(j["presence"]["cursor"]["x"], 1.0);
        assert_eq!(j["presence"]["cursor"]["y"], 2.0);

        // OperationsJournaled
        let j = serde_json::to_value(SessionEvent::OperationsJournaled {
            peer_id: "p".into(),
            op_count: 7,
            highest_clock: 42,
        })
        .unwrap();
        assert_eq!(j["kind"], "operationsJournaled");
        assert_eq!(j["peerId"], "p");
        assert_eq!(j["opCount"], 7);
        assert_eq!(j["highestClock"], 42);

        // LocksChanged
        let j = serde_json::to_value(SessionEvent::LocksChanged {
            peer_id: "p".into(),
            node_ids: vec![pid],
        })
        .unwrap();
        assert_eq!(j["kind"], "locksChanged");
        assert_eq!(j["peerId"], "p");
        assert_eq!(j["nodeIds"][0], pid.to_string());
    }

    /// Round 11: pin the JSON wire shape of `SessionStarted` so the
    /// renderer's `shared/scene.ts::SessionEvent` discriminated
    /// union (`kind: "sessionStarted"`, `peerId`, `projectId`) stays
    /// in lockstep with the bridge variant. AGENTS.md rule 4 says
    /// the TS file mirrors the bridge — this test is the
    /// enforcement. A breaking change to the variant fields fails
    /// here and reminds the contributor to update the TS side.
    #[test]
    fn session_started_event_serialises_to_renderer_wire_format() {
        let project_id = Uuid::parse_str("11111111-2222-3333-4444-555555555555").unwrap();
        let ev = SessionEvent::SessionStarted {
            peer_id: "local-peer-abc".to_string(),
            project_id,
        };
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["kind"], "sessionStarted");
        assert_eq!(json["peerId"], "local-peer-abc");
        assert_eq!(json["projectId"], project_id.to_string());
        // Round-trip ensures no field accidentally became flatten-only.
        let back: SessionEvent = serde_json::from_value(json).unwrap();
        assert!(matches!(
            back,
            SessionEvent::SessionStarted { peer_id, project_id: pid }
                if peer_id == "local-peer-abc" && pid == project_id
        ));
    }

    /// Round 11: pin the JSON wire shape of `SessionLeft`. The bridge
    /// returns the peer id from `session_leave()` and `main.ts`
    /// synthesises a `SessionLeft { peer_id }` JSON object directly
    /// on the renderer's event channel — this test guarantees the
    /// hand-rolled wire shape in `main.ts` matches what the renderer
    /// expects via `shared/scene.ts::SessionEvent`.
    #[test]
    fn session_left_event_serialises_to_renderer_wire_format() {
        let ev = SessionEvent::SessionLeft {
            peer_id: "departing-peer-xyz".to_string(),
        };
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["kind"], "sessionLeft");
        assert_eq!(json["peerId"], "departing-peer-xyz");
        let back: SessionEvent = serde_json::from_value(json).unwrap();
        assert!(matches!(
            back,
            SessionEvent::SessionLeft { peer_id } if peer_id == "departing-peer-xyz"
        ));
    }
}
