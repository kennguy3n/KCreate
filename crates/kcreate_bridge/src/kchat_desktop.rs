//! Phase 7 KChat Desktop bridge surface.
//!
//! Wraps [`kcreate_kchat_client`] so the renderer can drive the
//! "connect → list communities → install attestation" flow via
//! plain JSON calls. Every public function here is exposed as an
//! N-API entry point in `lib.rs` behind the `kchat-desktop`
//! feature flag, and every wire-format type is mirrored in
//! `apps/desktop/shared/scene.ts`.
//!
//! The bridge owns a single `KChatDesktopClient` instance behind a
//! `tokio::Mutex` so concurrent renderer-side callers (e.g. a
//! roster-sync tick + a user-driven "share to channel" call) are
//! serialised against the underlying socket. The Tokio runtime is
//! created lazily by `runtime()` and reused across calls — keeping
//! the runtime alive is important because the client task pumps
//! reads and writes from background tasks that must outlive any
//! single bridge invocation.
//!
//! ## Mapping to `kcreate_collab`
//!
//! `kchat_desktop_select_community` is the production replacement
//! for the dev-only `kchat_dev_mint_membership` flow: it calls
//! `kchat.communities.getMembership` over IPC, verifies the wire
//! attestation against the issuer trust root reported by the
//! desktop app, and installs the resulting authority into the
//! global slot via `collab::install_kchat_authority_direct`. After
//! that the regular collab gate (`session_start` / `session_join`)
//! works exactly as it does for the dev-mint flow.

use std::sync::Arc;
use std::sync::OnceLock;

use chrono::Utc;
use kcreate_collab::kchat::SharedKChatAuthority;
use kcreate_collab::peer::PeerId;
use parking_lot::Mutex as PlMutex;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::runtime::Runtime;

use kcreate_kchat_client::{
    decode_verifying_key, membership_from_attestation, KChatCommunity, KChatCommunityMember,
    KChatConversation, KChatDesktopAuthority, KChatDesktopClient, KChatIdentity, PostMessageParams,
    PostMessageResult,
};

use crate::collab::{install_kchat_authority_direct, KChatMembershipStatus};

/// Typed error surface returned to the N-API layer. Mapped to a
/// JSON `{ "kind": ..., "message": ... }` envelope by `lib.rs`.
#[derive(Debug, Error)]
pub enum KChatDesktopBridgeError {
    /// Tokio runtime construction failed. Should be impossible on
    /// supported platforms.
    #[error("kchat desktop bridge runtime init failed: {0}")]
    Runtime(String),
    /// Underlying client error (IO, RPC, protocol mismatch).
    #[error("kchat desktop client error: {0}")]
    Client(#[from] kcreate_kchat_client::ClientError),
    /// `select_community` failed to convert the wire attestation to
    /// a verified `KChatMembership` (e.g. signature mismatch, peer
    /// binding mismatch, expired window).
    #[error("kchat desktop attestation invalid: {0}")]
    Attestation(String),
    /// The user has not connected to KChat Desktop yet; the renderer
    /// must call `kchat_desktop_connect` first.
    #[error("kchat desktop is not connected")]
    NotConnected,
    /// The community returned by KChat Desktop has no member entry
    /// for the local identity — refuses to install an attestation
    /// the local peer can't satisfy.
    #[error("local identity has no member entry in community {community_id}")]
    LocalMemberMissing { community_id: String },
}

/// Wire-format result of `kchat_desktop_status`. Mirrored in
/// `apps/desktop/shared/scene.ts` as `KChatDesktopStatus`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KChatDesktopStatus {
    pub connected: bool,
    /// Local socket path the client is connected to. `None` when
    /// `connected == false`.
    pub socket_path: Option<String>,
    /// Identity reported by `kchat.identity.get` after a successful
    /// connect. `None` until the renderer calls
    /// `kchat_desktop_status` for the first time on an active
    /// connection (cached internally so subsequent calls are cheap).
    pub identity: Option<KChatIdentity>,
}

/// Wire-format invite payload posted to a KChat conversation by
/// `kchat_desktop_share_to_conversation`. Mirrors the
/// `InviteCardPayload` shape but is exposed here as a top-level
/// bridge type so the renderer doesn't need to import protocol
/// types directly.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KChatShareInvite {
    pub project_id: uuid::Uuid,
    pub project_name: String,
    pub owner_peer_id: String,
    pub owner_public_key: String,
    pub owner_display_name: String,
    pub cert_fingerprint: String,
    pub owner_socket_addr: String,
    pub community_id: String,
    pub conversation_id: String,
}

impl KChatShareInvite {
    /// Convert into the wire-format invite card payload, stamping
    /// the schema version + issuance timestamp.
    fn into_card(self) -> kcreate_kchat_client::InviteCardPayload {
        kcreate_kchat_client::InviteCardPayload {
            schema_version: kcreate_kchat_client::INVITE_SCHEMA_VERSION,
            project_id: self.project_id,
            project_name: self.project_name,
            owner_peer_id: self.owner_peer_id,
            owner_public_key: self.owner_public_key,
            owner_display_name: self.owner_display_name,
            cert_fingerprint: self.cert_fingerprint,
            owner_socket_addr: self.owner_socket_addr,
            community_id: self.community_id,
            conversation_id: self.conversation_id,
            issued_at: Utc::now(),
        }
    }
}

/// Process-global slot for the active KChat Desktop client.
fn client_slot() -> &'static PlMutex<Option<Arc<KChatDesktopClient>>> {
    static S: OnceLock<PlMutex<Option<Arc<KChatDesktopClient>>>> = OnceLock::new();
    S.get_or_init(|| PlMutex::new(None))
}

/// Process-global slot for the active KChat Desktop authority.
/// Holds a strong reference so the roster-sync task in Block B can
/// fetch the same authority `session_*` is using and trigger
/// refreshes.
fn authority_slot() -> &'static PlMutex<Option<Arc<KChatDesktopAuthority>>> {
    static S: OnceLock<PlMutex<Option<Arc<KChatDesktopAuthority>>>> = OnceLock::new();
    S.get_or_init(|| PlMutex::new(None))
}

/// Lazily constructed Tokio runtime shared by all bridge calls. We
/// use a multi-threaded runtime so the read pump and the
/// notification fan-out can run on separate worker threads. Stored
/// behind a `OnceLock<Result<...>>` so a construction failure is
/// surfaced once and never retried (re-trying would silently leak
/// resources if the failure was platform-level).
fn runtime() -> Result<&'static Runtime, KChatDesktopBridgeError> {
    static RT: OnceLock<Result<Runtime, String>> = OnceLock::new();
    let slot = RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_io()
            .enable_time()
            .thread_name("kchat-desktop")
            .worker_threads(2)
            .build()
            .map_err(|e| e.to_string())
    });
    slot.as_ref()
        .map_err(|msg| KChatDesktopBridgeError::Runtime(msg.clone()))
}

/// Snapshot the active client (cloning the `Arc`). Returns
/// [`KChatDesktopBridgeError::NotConnected`] when no connect call
/// has been made.
fn require_client() -> Result<Arc<KChatDesktopClient>, KChatDesktopBridgeError> {
    client_slot()
        .lock()
        .clone()
        .ok_or(KChatDesktopBridgeError::NotConnected)
}

/// Connect to the local `uney-chat-desktop` IPC socket. Tries the
/// platform default paths in order (XDG runtime dir first on Unix,
/// `$HOME/.kchat/...` fallback) and stores the resulting client in
/// the process-global slot. Returns the [`KChatDesktopStatus`]
/// snapshot.
///
/// If a previous client is still in the slot it is disconnected
/// gracefully before the new client is installed.
pub fn kchat_desktop_connect() -> Result<KChatDesktopStatus, KChatDesktopBridgeError> {
    let rt = runtime()?;
    let new_client = Arc::new(KChatDesktopClient::new());
    // Tear down any prior client before connecting a fresh one.
    // Take the value out of the slot synchronously so the mutex
    // guard does not live across the subsequent `await` —
    // `client_slot()` is a `parking_lot::Mutex` and holding it
    // across `.await` is both a clippy `await_holding_lock`
    // violation and a real risk of deadlock.
    let prev = client_slot().lock().take();
    let connected_path = rt.block_on(async {
        if let Some(prev) = prev {
            prev.disconnect().await;
        }
        new_client.connect().await
    })?;
    let identity = rt.block_on(new_client.get_identity()).ok();
    *client_slot().lock() = Some(new_client);
    Ok(KChatDesktopStatus {
        connected: true,
        socket_path: Some(connected_path.display().to_string()),
        identity,
    })
}

/// Disconnect from the local IPC socket. Idempotent — calling on
/// an already-disconnected client returns the locked status. Also
/// clears the installed authority so a stale membership doesn't
/// outlive the connection it was minted from.
pub fn kchat_desktop_disconnect() -> Result<KChatDesktopStatus, KChatDesktopBridgeError> {
    let rt = runtime()?;
    let client = client_slot().lock().take();
    if let Some(client) = client {
        rt.block_on(client.disconnect());
    }
    *authority_slot().lock() = None;
    crate::collab::kchat_clear_authority();
    Ok(KChatDesktopStatus {
        connected: false,
        socket_path: None,
        identity: None,
    })
}

/// Return the current connection state + cached identity.
pub fn kchat_desktop_status() -> Result<KChatDesktopStatus, KChatDesktopBridgeError> {
    let Some(client) = client_slot().lock().clone() else {
        return Ok(KChatDesktopStatus {
            connected: false,
            socket_path: None,
            identity: None,
        });
    };
    let rt = runtime()?;
    let (connected, socket_path, identity) = rt.block_on(async {
        let connected = client.is_connected().await;
        let socket_path = client
            .connected_path()
            .await
            .map(|p| p.display().to_string());
        let identity = client.get_identity().await.ok();
        (connected, socket_path, identity)
    });
    Ok(KChatDesktopStatus {
        connected,
        socket_path,
        identity,
    })
}

/// Return the list of communities the local user belongs to.
pub fn kchat_desktop_list_communities() -> Result<Vec<KChatCommunity>, KChatDesktopBridgeError> {
    let client = require_client()?;
    let rt = runtime()?;
    Ok(rt.block_on(client.list_communities())?)
}

/// Select a community and install its attestation as the active
/// authority. Calls `kchat.communities.getMembership` over IPC,
/// verifies the wire attestation against the issuer trust root
/// supplied by KChat Desktop, and installs the resulting authority
/// into the global collab slot via
/// [`install_kchat_authority_direct`]. Returns the new
/// [`KChatMembershipStatus`].
pub fn kchat_desktop_select_community(
    community_id: &str,
) -> Result<KChatMembershipStatus, KChatDesktopBridgeError> {
    let client = require_client()?;
    let rt = runtime()?;

    // Fetch identity + attestation + roster together so the
    // verifier has the bound peer id + public key to match against
    // the wire attestation.
    let (identity, attestation, members) = rt.block_on(async {
        let identity = client.get_identity().await?;
        let attestation = client.get_membership(community_id).await?;
        let members = client.get_members(community_id).await?;
        Ok::<_, kcreate_kchat_client::ClientError>((identity, attestation, members))
    })?;

    // Cross-check: the local identity must appear in the community
    // roster, with a peer id that matches what we'll bind the
    // attestation to. Refusing here is defence-in-depth — the
    // collab gate would catch a mismatch on the first
    // `session_start`, but the bridge can surface a clearer error.
    if !members.iter().any(|m| m.peer_id == identity.peer_id) {
        return Err(KChatDesktopBridgeError::LocalMemberMissing {
            community_id: community_id.to_string(),
        });
    }

    let peer_id: PeerId =
        serde_json::from_value(serde_json::Value::String(identity.peer_id.clone()))
            .map_err(|e| KChatDesktopBridgeError::Attestation(format!("invalid peerId: {e}")))?;

    let authority = KChatDesktopAuthority::install(
        client,
        community_id,
        attestation.clone(),
        peer_id,
        identity.public_key,
        Utc::now(),
    )
    .map_err(|e| KChatDesktopBridgeError::Attestation(e.to_string()))?;

    let membership = membership_from_attestation(attestation)
        .map_err(|e| KChatDesktopBridgeError::Attestation(e.to_string()))?;

    let authority_arc = Arc::new(authority);
    let shared: SharedKChatAuthority = authority_arc.clone();
    *authority_slot().lock() = Some(authority_arc);

    Ok(install_kchat_authority_direct(shared, &membership))
}

/// Return the member list (with roles) for the given community.
pub fn kchat_desktop_get_community_members(
    community_id: &str,
) -> Result<Vec<KChatCommunityMember>, KChatDesktopBridgeError> {
    let client = require_client()?;
    let rt = runtime()?;
    Ok(rt.block_on(client.get_members(community_id))?)
}

/// Return the list of conversations/channels in the given community.
pub fn kchat_desktop_list_conversations(
    community_id: &str,
) -> Result<Vec<KChatConversation>, KChatDesktopBridgeError> {
    let client = require_client()?;
    let rt = runtime()?;
    Ok(rt.block_on(client.list_conversations(community_id))?)
}

/// Post a share-document invite to a KChat conversation. The body
/// is a JSON-serialised [`KChatShareInvite`] tagged with the
/// `kcreate.invite.v1` content type so the desktop app can render
/// it as a rich card.
pub fn kchat_desktop_share_to_conversation(
    conversation_id: &str,
    invite: KChatShareInvite,
) -> Result<PostMessageResult, KChatDesktopBridgeError> {
    let client = require_client()?;
    let rt = runtime()?;
    let card = invite.into_card();
    let params = PostMessageParams {
        conversation_id: conversation_id.to_string(),
        payload: serde_json::to_value(&card).expect("invite payload serialises"),
        content_type: Some(kcreate_kchat_client::INVITE_CONTENT_TYPE.to_string()),
    };
    Ok(rt.block_on(client.post_message(params))?)
}

/// Phase 7 (Task 8): roster-sync tick. Reads the current community
/// members from KChat Desktop, then asks the collab bridge to
/// reconcile the set against connected peers (kicks anyone not in
/// the roster, refreshes role -> permission mappings). Returns the
/// list of peer ids that were evicted on this tick so the caller
/// can log / surface them.
///
/// Renderer drives this on a fixed cadence (every 30s, see
/// `apps/desktop/main/src/main.ts::kchatRosterSyncTick`). The
/// bridge is the single owner of "what does the LAN session
/// currently look like", so the renderer never has to reach
/// directly into `session_kick_peer`.
pub fn kchat_desktop_sync_community_roster(
    community_id: &str,
) -> Result<KChatRosterSyncResult, KChatDesktopBridgeError> {
    let client = require_client()?;
    let rt = runtime()?;
    let members = rt.block_on(client.get_members(community_id))?;
    // `session_apply_community_roster` expects (peer_id_b64, role)
    // pairs. KChat Desktop returns the role as a lowercase
    // string ("owner" / "admin" / "member") which maps directly
    // through `CollabPermission::from_role`.
    let pairs: Vec<(String, String)> = members
        .iter()
        .map(|m| (m.peer_id.clone(), m.role.as_str().to_string()))
        .collect();
    let kicked = crate::collab::session_apply_community_roster(&pairs)
        .map_err(|e| KChatDesktopBridgeError::Attestation(e.to_string()))?;
    Ok(KChatRosterSyncResult {
        polled_members: members.len(),
        kicked,
    })
}

/// Phase 7 (Task 10): accept a share-document invite the user
/// received through a KChat Desktop conversation. Parses the
/// invite JSON, verifies the invite's community matches the
/// active session's community (defence-in-depth -- the collab
/// gate would otherwise reject the dial), and triggers
/// [`crate::collab::session_join`] to dial the owner peer.
///
/// Returns the dialed peer's identity on success so the renderer
/// can show a "joined Ken's project" toast.
pub fn kchat_desktop_accept_invite(
    invite_json: &str,
) -> Result<KChatAcceptedInvite, KChatDesktopBridgeError> {
    let card: kcreate_kchat_client::InviteCardPayload =
        serde_json::from_str(invite_json).map_err(|e| {
            KChatDesktopBridgeError::Attestation(format!("invite is not valid JSON: {e}"))
        })?;

    // The invite must declare the same schema version the bridge
    // was built against -- otherwise the field set might be
    // missing pieces (e.g. a future schema adds `mls_group_epoch`
    // and we'd silently dial without verifying it).
    if card.schema_version != kcreate_kchat_client::INVITE_SCHEMA_VERSION {
        return Err(KChatDesktopBridgeError::Attestation(format!(
            "invite schema_version {} is not supported (bridge expects {})",
            card.schema_version,
            kcreate_kchat_client::INVITE_SCHEMA_VERSION,
        )));
    }

    // Community match. If the local bridge doesn't have a community
    // active, defer to the collab gate -- `session_join` will
    // reject the dial if the installed membership doesn't match
    // the owner's peer binding.
    if let Some(local_community) = crate::collab::session_community_id() {
        if local_community != card.community_id {
            return Err(KChatDesktopBridgeError::Attestation(format!(
                "invite is for community {} but the local session is bound to {local_community}",
                card.community_id,
            )));
        }
    }

    // Cross-check: the sender must still be a member of the
    // community advertised in the invite. This guards against the
    // case where someone forwards an old invite after the sender
    // was revoked.
    let client = require_client()?;
    let rt = runtime()?;
    let members = rt.block_on(client.get_members(&card.community_id))?;
    if !members.iter().any(|m| m.peer_id == card.owner_peer_id) {
        return Err(KChatDesktopBridgeError::Attestation(format!(
            "invite owner {} is not (or no longer) a member of community {}",
            card.owner_peer_id, card.community_id,
        )));
    }

    // Dial through the regular session_join path. We don't pass an
    // explicit cert fingerprint -- the bridge expects it as a
    // separate argument to enforce binding at the QUIC handshake.
    crate::collab::session_join(
        &card.owner_peer_id,
        &card.owner_public_key,
        &card.owner_display_name,
        &card.owner_socket_addr,
        &card.cert_fingerprint,
    )
    .map_err(|e| KChatDesktopBridgeError::Attestation(e.to_string()))?;

    Ok(KChatAcceptedInvite {
        project_id: card.project_id,
        project_name: card.project_name,
        owner_peer_id: card.owner_peer_id,
        owner_display_name: card.owner_display_name,
        community_id: card.community_id,
        conversation_id: card.conversation_id,
    })
}

/// Wire-format DTO for [`kchat_desktop_sync_community_roster`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KChatRosterSyncResult {
    /// How many members KChat Desktop reported for the community on
    /// this tick. Useful for the audit trail (Phase 7 Task 20).
    pub polled_members: usize,
    /// Peer ids the bridge evicted from the session because they
    /// were no longer in the roster. Empty when no eviction was
    /// necessary.
    pub kicked: Vec<String>,
}

/// Wire-format DTO returned by [`kchat_desktop_accept_invite`] on
/// the happy path.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KChatAcceptedInvite {
    pub project_id: uuid::Uuid,
    pub project_name: String,
    pub owner_peer_id: String,
    pub owner_display_name: String,
    pub community_id: String,
    pub conversation_id: String,
}

/// Snapshot the active KChat Desktop authority. Used by Block B's
/// roster-sync tick to detect kicked peers via
/// `KChatDesktopAuthority::refresh_if_needed`.
#[must_use]
pub fn active_authority() -> Option<Arc<KChatDesktopAuthority>> {
    authority_slot().lock().clone()
}

/// Re-verify and (if needed) refresh the active authority's
/// attestation. No-op when no authority is installed. Returns
/// `true` when a refresh was performed.
pub async fn refresh_active_authority_if_needed() -> Result<bool, KChatDesktopBridgeError> {
    let Some(authority) = active_authority() else {
        return Ok(false);
    };
    let refreshed = authority
        .refresh_if_needed()
        .await
        .map_err(|e| KChatDesktopBridgeError::Attestation(e.to_string()))?;
    Ok(refreshed)
}

/// Helper used by the integration test in `kcreate_tests` to wire a
/// pre-existing client (e.g. one already connected via the in-memory
/// `tokio::io::duplex` mock-server harness) into the bridge slot.
/// Not exposed via N-API.
pub fn install_client_for_tests(client: Arc<KChatDesktopClient>) {
    *client_slot().lock() = Some(client);
}

/// Helper used by integration tests to clear the bridge slot
/// between cases without going through a full disconnect.
pub fn reset_client_for_tests() {
    *client_slot().lock() = None;
    *authority_slot().lock() = None;
}

/// Decode a base64url-no-pad Ed25519 public key. Re-exported from
/// `kcreate_kchat_client` so the bridge surface can stay
/// self-contained — callers shouldn't need to depend on the client
/// crate directly.
pub fn decode_issuer_key(
    b64: &str,
) -> Result<ed25519_dalek::VerifyingKey, KChatDesktopBridgeError> {
    decode_verifying_key(b64).map_err(|e| KChatDesktopBridgeError::Attestation(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use kcreate_collab::peer::PeerKey;
    use kcreate_kchat_client::mock_server::{
        replace_identity_with_peer_key, spawn_single_stream, MockState,
    };
    use serial_test::serial;

    /// Install a mock-backed client into the bridge slot and verify
    /// that `kchat_desktop_select_community` produces an
    /// `unlocked` `KChatMembershipStatus`. This is the equivalent
    /// of the integration-test "connect → list → select → unlocked"
    /// flow, kept in-crate so the bridge slot stays serialised
    /// against neighbouring `collab` tests via `#[serial]`.
    #[test]
    #[serial]
    fn select_community_installs_authority_and_unlocks_gate() {
        // Use the bridge-owned runtime so it shares the same
        // worker pool the real entry points use.
        let rt = runtime().expect("runtime");
        let key = PeerKey::from_seed([42u8; 32]);
        let identity = key.identity("Alice");

        let (server, client_io) = tokio::io::duplex(64 * 1024);
        let mut state = MockState::fixture();
        replace_identity_with_peer_key(&mut state, &key, "Alice", "alice@kchat.com");
        // `spawn_single_stream` calls `tokio::spawn` internally, which
        // requires the current-thread reactor context. Enter the
        // bridge runtime so the spawn binds to the same worker pool
        // that the real entry points run on.
        let (_handle, _issuer_pub) = rt.block_on(async { spawn_single_stream(state, server) });

        let client = Arc::new(KChatDesktopClient::new());
        rt.block_on(client.install_test_stream(client_io));
        install_client_for_tests(client);

        let status = kchat_desktop_select_community("comm-test").expect("select");
        assert!(!status.locked, "membership should unlock the gate");
        assert_eq!(status.group_id.as_deref(), Some("comm-test"));

        // Cleanup — clear the bridge slots and the underlying collab
        // gate so the next test starts from a known state.
        let _ = kchat_desktop_disconnect();
        crate::collab::kchat_clear_authority();
        let _ = identity; // silence unused
    }
}
