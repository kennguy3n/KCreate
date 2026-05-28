//! Phase 7 KChat **backend** bridge surface (REST over HTTPS).
//!
//! Wraps [`kcreate_kchat_client`] so the renderer can drive the
//! "sign in → list communities → install attestation" flow against
//! the shared KChat / Mattermost backend that uney-chat-desktop
//! also speaks to. Every public function here is exposed as an
//! N-API entry point in `lib.rs` behind the `kchat-backend`
//! feature flag, and every wire-format type is mirrored in
//! `apps/desktop/shared/scene.ts`.
//!
//! ## Flow
//!
//! - The renderer collects the user's backend URL + credentials in
//!   `KChatSignInPanel` and calls [`kchat_backend_connect`], which
//!   logs in and caches the resulting [`KChatBackendClient`] in
//!   this module's process-global slot.
//! - [`kchat_backend_list_communities`] / [`kchat_backend_select_community`]
//!   pull the user's communities and install the active community's
//!   signed attestation as the global collab authority.
//! - [`kchat_backend_share_to_conversation`] posts a rich-card
//!   invite to a KChat conversation; [`kchat_backend_accept_invite`]
//!   parses such a card on the joiner side and triggers
//!   `session_join` to dial the host peer.
//!
//! A separate `.kcz` companion extension ships inside KChat Desktop
//! (`apps/kchat-extension/`) and surfaces recent projects + share
//! invites via the host's procedures registry — it does not proxy
//! this bridge; both apps independently talk to the same backend.
//!
//! ## Mapping to `kcreate_collab`
//!
//! [`kchat_backend_select_community`] is the production replacement
//! for the dev-only `kchat_dev_mint_membership` flow: it calls the
//! backend's per-peer attestation endpoint, verifies the wire
//! attestation against the issuer trust root advertised in the
//! response, and installs the resulting authority into the global
//! collab slot via `collab::install_kchat_authority_direct`. After
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
    decode_verifying_key, membership_from_attestation, KChatBackendAuthority, KChatBackendClient,
    KChatCommunity, KChatCommunityMember, KChatConversation, KChatIdentity, LoginRequest,
    PostMessageRequest, PostMessageResponse,
};

use crate::collab::{install_kchat_authority_direct, KChatMembershipStatus};

/// Typed error surface returned to the N-API layer. Mapped to a
/// JSON `{ "kind": ..., "message": ... }` envelope by `lib.rs`.
#[derive(Debug, Error)]
pub enum KChatBackendBridgeError {
    /// Tokio runtime construction failed. Should be impossible on
    /// supported platforms.
    #[error("kchat backend bridge runtime init failed: {0}")]
    Runtime(String),
    /// Underlying REST client error (transport, auth, protocol).
    #[error("kchat backend client error: {0}")]
    Client(#[from] kcreate_kchat_client::ClientError),
    /// `select_community` failed to convert the wire attestation to
    /// a verified `KChatMembership` (e.g. signature mismatch, peer
    /// binding mismatch, expired window). Reserved for failures
    /// inside the attestation pipeline itself — *not* for collab
    /// session lifecycle failures, which surface as [`Self::Session`]
    /// so the renderer can distinguish "your KChat token is stale"
    /// from "you tried to accept an invite without a running
    /// session".
    #[error("kchat backend attestation invalid: {0}")]
    Attestation(String),
    /// The user has not signed in to the KChat backend yet; the
    /// renderer must call [`kchat_backend_connect`] first.
    #[error("not signed in to kchat backend")]
    NotConnected,
    /// No project is currently open in the bridge. Returned by the
    /// artifact-publishing entry points which need
    /// [`crate::document::project_info`] to stamp the source-project
    /// id / name into the artifact metadata.
    #[error("no project is open — call project_create or project_open first")]
    NoOpenProject,
    /// The community returned by the backend has no member entry
    /// for the local identity — refuses to install an attestation
    /// the local peer can't satisfy.
    #[error("local identity has no member entry in community {community_id}")]
    LocalMemberMissing { community_id: String },
    /// A downstream call into the collab session bridge failed
    /// (e.g. `session_join`, `session_apply_community_roster`).
    /// Wraps the typed [`crate::collab::SessionBridgeError`] so the
    /// renderer can distinguish session-lifecycle failures
    /// (`NotRunning`, `NotInKChatGroup`, transport errors) from
    /// attestation failures, which surface as [`Self::Attestation`].
    /// The `#[from]` impl lets bridge methods use `?` directly on
    /// session calls.
    #[error("kchat backend collab session error: {0}")]
    Session(#[from] crate::collab::SessionBridgeError),
}

/// Wire-format result of [`kchat_backend_status`]. Mirrored in
/// `apps/desktop/shared/scene.ts` as `KChatBackendStatus`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KChatBackendStatus {
    /// True when a sign-in has succeeded and the client still has
    /// valid (or refreshable) tokens.
    pub connected: bool,
    /// HTTPS base URL the client is signed in to. `None` when
    /// `connected == false`.
    pub base_url: Option<String>,
    /// Identity returned by the login response. Cached so
    /// subsequent `status` calls don't roundtrip to the backend.
    /// `None` until the renderer has signed in.
    pub identity: Option<KChatIdentity>,
}

/// Wire-format sign-in request the renderer hands to
/// [`kchat_backend_connect`]. Mirrors `LoginRequest` from the
/// client crate but keeps the bridge surface self-contained.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KChatBackendSignInRequest {
    pub base_url: String,
    pub login_id: String,
    pub password: String,
    #[serde(default)]
    pub totp: Option<String>,
}

/// Wire-format invite payload posted to a KChat conversation by
/// [`kchat_backend_share_to_conversation`]. Mirrors the
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

/// Process-global slot for the active KChat backend client.
fn client_slot() -> &'static PlMutex<Option<Arc<KChatBackendClient>>> {
    static S: OnceLock<PlMutex<Option<Arc<KChatBackendClient>>>> = OnceLock::new();
    S.get_or_init(|| PlMutex::new(None))
}

/// Process-global slot for the active KChat backend authority.
/// Holds a strong reference so the roster-sync task in Block B can
/// fetch the same authority `session_*` is using and trigger
/// refreshes.
fn authority_slot() -> &'static PlMutex<Option<Arc<KChatBackendAuthority>>> {
    static S: OnceLock<PlMutex<Option<Arc<KChatBackendAuthority>>>> = OnceLock::new();
    S.get_or_init(|| PlMutex::new(None))
}

/// Lazily constructed Tokio runtime shared by all bridge calls. We
/// use a multi-threaded runtime so concurrent renderer-side callers
/// (e.g. a roster-sync tick + a user-driven "share to channel" call)
/// can run on separate worker threads. Stored behind a
/// `OnceLock<Result<...>>` so a construction failure is surfaced
/// once and never retried (re-trying would silently leak resources
/// if the failure was platform-level).
fn runtime() -> Result<&'static Runtime, KChatBackendBridgeError> {
    static RT: OnceLock<Result<Runtime, String>> = OnceLock::new();
    let slot = RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_io()
            .enable_time()
            .thread_name("kchat-backend")
            .worker_threads(2)
            .build()
            .map_err(|e| e.to_string())
    });
    slot.as_ref()
        .map_err(|msg| KChatBackendBridgeError::Runtime(msg.clone()))
}

/// Snapshot the active client (cloning the `Arc`). Returns
/// [`KChatBackendBridgeError::NotConnected`] when no sign-in has
/// happened.
fn require_client() -> Result<Arc<KChatBackendClient>, KChatBackendBridgeError> {
    client_slot()
        .lock()
        .clone()
        .ok_or(KChatBackendBridgeError::NotConnected)
}

/// `pub(crate)` re-export of [`require_client`] for the sibling
/// artifact module. Kept as a separate function (not just
/// `pub(crate)` on the existing one) so production callers inside
/// this module continue to use the same private helper and a
/// future refactor of the auth-token cache can change the
/// internal helper without rippling into `kchat_artifact.rs`.
pub(crate) fn require_client_for_artifacts(
) -> Result<Arc<KChatBackendClient>, KChatBackendBridgeError> {
    require_client()
}

/// `pub(crate)` accessor for the shared Tokio runtime; used by
/// the artifact module so the multipart upload runs on the same
/// pool as the rest of the bridge's REST calls (otherwise two
/// runtimes would compete for worker threads).
pub(crate) fn runtime_for_artifacts() -> Result<&'static Runtime, KChatBackendBridgeError> {
    runtime()
}

/// Sign in to a KChat backend. Tears down any prior client + the
/// installed authority before installing the new client so a stale
/// session doesn't outlive the sign-in it was minted from. Returns
/// the resulting [`KChatBackendStatus`].
///
/// The base URL must be `https://...` in production builds —
/// [`KChatBackendClient::new`] refuses plain HTTP. The renderer's
/// Developer Mode allows pointing at a local fixture via
/// [`kchat_backend_connect_for_tests`].
pub fn kchat_backend_connect(
    request: KChatBackendSignInRequest,
) -> Result<KChatBackendStatus, KChatBackendBridgeError> {
    let rt = runtime()?;
    let new_client = Arc::new(KChatBackendClient::new(&request.base_url)?);
    let login = LoginRequest {
        login_id: request.login_id,
        password: request.password,
        totp: request.totp,
    };
    let identity = rt.block_on(new_client.login(&login))?;
    // Atomic install: swap the new client into the slot under a
    // single lock acquire so any concurrent call (e.g. a future
    // background roster-sync timer) sees either the old client or
    // the new one — never a transient `None` that would surface as
    // `NotConnected`. The old client is logged out *after* the
    // lock is released because `logout()` is purely in-memory
    // (clears tokens, no backend round-trip) so we don't need to
    // hold the slot lock through it.
    let prev = client_slot().lock().replace(new_client);
    *authority_slot().lock() = None;
    let _ = rt;
    crate::collab::kchat_clear_authority();
    if let Some(prev) = prev {
        prev.logout();
    }
    Ok(KChatBackendStatus {
        connected: true,
        base_url: Some(request.base_url),
        identity: Some(identity),
    })
}

/// Test-only `kchat_backend_connect` that accepts plain-HTTP
/// `base_url`s pointing at the in-process axum fixture. Builds the
/// REST client via [`KChatBackendClient::new_for_tests`] (which
/// skips the production HTTPS check) and installs it in the
/// process-global client slot exactly the way
/// [`kchat_backend_connect`] does, so all of the regular
/// bridge entry points (`kchat_backend_share_to_conversation`,
/// `kchat_backend_publish_artifact`, etc.) see a real signed-in
/// session.
///
/// The renderer's Developer Mode wires this up so contributors
/// can point KCreate at a local fixture; integration tests in
/// `crates/kcreate_tests/` call it directly to drive the
/// bridge against the same `FixtureServer` the client tests use.
#[doc(hidden)]
pub fn kchat_backend_connect_for_tests(
    request: KChatBackendSignInRequest,
) -> Result<KChatBackendStatus, KChatBackendBridgeError> {
    let rt = runtime()?;
    let new_client = Arc::new(KChatBackendClient::new_for_tests(&request.base_url)?);
    let login = LoginRequest {
        login_id: request.login_id,
        password: request.password,
        totp: request.totp,
    };
    let identity = rt.block_on(new_client.login(&login))?;
    let prev = client_slot().lock().replace(new_client);
    *authority_slot().lock() = None;
    let _ = rt;
    crate::collab::kchat_clear_authority();
    if let Some(prev) = prev {
        prev.logout();
    }
    Ok(KChatBackendStatus {
        connected: true,
        base_url: Some(request.base_url),
        identity: Some(identity),
    })
}

/// Sign out of the KChat backend. Idempotent — calling on an
/// already-disconnected client returns the locked status. Also
/// clears the installed authority so a stale membership doesn't
/// outlive the sign-in it was minted from.
pub fn kchat_backend_disconnect() -> Result<KChatBackendStatus, KChatBackendBridgeError> {
    let client = client_slot().lock().take();
    if let Some(client) = client {
        client.logout();
    }
    *authority_slot().lock() = None;
    crate::collab::kchat_clear_authority();
    Ok(KChatBackendStatus {
        connected: false,
        base_url: None,
        identity: None,
    })
}

/// Return the current sign-in state + cached identity.
pub fn kchat_backend_status() -> Result<KChatBackendStatus, KChatBackendBridgeError> {
    let Some(client) = client_slot().lock().clone() else {
        return Ok(KChatBackendStatus {
            connected: false,
            base_url: None,
            identity: None,
        });
    };
    let identity = client.cached_identity();
    Ok(KChatBackendStatus {
        connected: identity.is_some(),
        base_url: Some(client.base_url()),
        identity,
    })
}

/// Return the list of communities the local user belongs to.
pub fn kchat_backend_list_communities() -> Result<Vec<KChatCommunity>, KChatBackendBridgeError> {
    let client = require_client()?;
    let rt = runtime()?;
    Ok(rt.block_on(client.list_communities())?)
}

/// Select a community and install its attestation as the active
/// authority. Calls the backend's per-peer attestation endpoint,
/// verifies the wire attestation against the issuer trust root
/// supplied by the backend, and installs the resulting authority
/// into the global collab slot via
/// [`install_kchat_authority_direct`]. Returns the new
/// [`KChatMembershipStatus`].
pub fn kchat_backend_select_community(
    community_id: &str,
) -> Result<KChatMembershipStatus, KChatBackendBridgeError> {
    let client = require_client()?;
    let rt = runtime()?;

    let identity = client
        .cached_identity()
        .ok_or(KChatBackendBridgeError::NotConnected)?;

    let (attestation, members) = rt.block_on(async {
        let attestation = client
            .get_membership_attestation(community_id, &identity.public_key)
            .await?;
        let members = client.get_community_members(community_id).await?;
        Ok::<_, kcreate_kchat_client::ClientError>((attestation, members))
    })?;

    // Cross-check: the local identity must appear in the community
    // roster, with a peer id that matches what we'll bind the
    // attestation to. Refusing here is defence-in-depth — the
    // collab gate would catch a mismatch on the first
    // `session_start`, but the bridge can surface a clearer error.
    if !members.iter().any(|m| m.peer_id == identity.peer_id) {
        return Err(KChatBackendBridgeError::LocalMemberMissing {
            community_id: community_id.to_string(),
        });
    }

    let peer_id: PeerId =
        serde_json::from_value(serde_json::Value::String(identity.peer_id.clone()))
            .map_err(|e| KChatBackendBridgeError::Attestation(format!("invalid peerId: {e}")))?;

    let authority = KChatBackendAuthority::install(
        client,
        community_id,
        attestation.clone(),
        peer_id,
        identity.public_key,
        Utc::now(),
    )
    .map_err(|e| KChatBackendBridgeError::Attestation(e.to_string()))?;

    let membership = membership_from_attestation(attestation)
        .map_err(|e| KChatBackendBridgeError::Attestation(e.to_string()))?;

    let authority_arc = Arc::new(authority);
    let shared: SharedKChatAuthority = authority_arc.clone();
    *authority_slot().lock() = Some(authority_arc);

    Ok(install_kchat_authority_direct(shared, &membership))
}

/// Return the member list (with roles) for the given community.
pub fn kchat_backend_get_community_members(
    community_id: &str,
) -> Result<Vec<KChatCommunityMember>, KChatBackendBridgeError> {
    let client = require_client()?;
    let rt = runtime()?;
    Ok(rt.block_on(client.get_community_members(community_id))?)
}

/// Return the list of conversations/channels in the given community.
pub fn kchat_backend_list_conversations(
    community_id: &str,
) -> Result<Vec<KChatConversation>, KChatBackendBridgeError> {
    let client = require_client()?;
    let rt = runtime()?;
    Ok(rt.block_on(client.list_conversations(community_id))?)
}

/// Post a share-document invite to a KChat conversation. The body
/// is a JSON-serialised [`KChatShareInvite`] tagged with the
/// `kcreate.invite.v1` content type so the desktop app can render
/// it as a rich card.
pub fn kchat_backend_share_to_conversation(
    conversation_id: &str,
    invite: KChatShareInvite,
) -> Result<PostMessageResponse, KChatBackendBridgeError> {
    let client = require_client()?;
    let rt = runtime()?;
    let card = invite.into_card();
    let request = PostMessageRequest {
        payload: serde_json::to_value(&card).expect("invite payload serialises"),
        content_type: Some(kcreate_kchat_client::INVITE_CONTENT_TYPE.to_string()),
    };
    Ok(rt.block_on(client.post_message(conversation_id, &request))?)
}

/// Phase 7 (Task 8): roster-sync tick. Reads the current community
/// members from the KChat backend, then asks the collab bridge to
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
pub fn kchat_backend_sync_community_roster(
    community_id: &str,
) -> Result<KChatRosterSyncResult, KChatBackendBridgeError> {
    let client = require_client()?;
    let rt = runtime()?;
    let members = rt.block_on(client.get_community_members(community_id))?;
    let pairs: Vec<(String, String)> = members
        .iter()
        .map(|m| (m.peer_id.clone(), m.role.as_str().to_string()))
        .collect();
    let kicked = crate::collab::session_apply_community_roster(&pairs)?;
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
pub fn kchat_backend_accept_invite(
    invite_json: &str,
) -> Result<KChatAcceptedInvite, KChatBackendBridgeError> {
    let card: kcreate_kchat_client::InviteCardPayload =
        serde_json::from_str(invite_json).map_err(|e| {
            KChatBackendBridgeError::Attestation(format!("invite is not valid JSON: {e}"))
        })?;

    // The invite must declare the same schema version the bridge
    // was built against -- otherwise the field set might be
    // missing pieces (e.g. a future schema adds `mls_group_epoch`
    // and we'd silently dial without verifying it).
    if card.schema_version != kcreate_kchat_client::INVITE_SCHEMA_VERSION {
        return Err(KChatBackendBridgeError::Attestation(format!(
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
            return Err(KChatBackendBridgeError::Attestation(format!(
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
    let members = rt.block_on(client.get_community_members(&card.community_id))?;
    if !members.iter().any(|m| m.peer_id == card.owner_peer_id) {
        return Err(KChatBackendBridgeError::Attestation(format!(
            "invite owner {} is not (or no longer) a member of community {}",
            card.owner_peer_id, card.community_id,
        )));
    }

    // Dial through the regular session_join path.
    crate::collab::session_join(
        &card.owner_peer_id,
        &card.owner_public_key,
        &card.owner_display_name,
        &card.owner_socket_addr,
        &card.cert_fingerprint,
    )?;

    Ok(KChatAcceptedInvite {
        project_id: card.project_id,
        project_name: card.project_name,
        owner_peer_id: card.owner_peer_id,
        owner_display_name: card.owner_display_name,
        community_id: card.community_id,
        conversation_id: card.conversation_id,
    })
}

/// Wire-format DTO for [`kchat_backend_sync_community_roster`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KChatRosterSyncResult {
    /// How many members the backend reported for the community on
    /// this tick. Useful for the audit trail (Phase 7 Task 20).
    pub polled_members: usize,
    /// Peer ids the bridge evicted from the session because they
    /// were no longer in the roster. Empty when no eviction was
    /// necessary.
    pub kicked: Vec<String>,
}

/// Wire-format DTO returned by [`kchat_backend_accept_invite`] on
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

/// Snapshot the active KChat backend authority. Used by Block B's
/// roster-sync tick to detect kicked peers via
/// `KChatBackendAuthority::refresh_if_needed`.
#[must_use]
pub fn active_authority() -> Option<Arc<KChatBackendAuthority>> {
    authority_slot().lock().clone()
}

/// Re-verify and (if needed) refresh the active authority's
/// attestation. No-op when no authority is installed. Returns
/// `true` when a refresh was performed.
pub async fn refresh_active_authority_if_needed() -> Result<bool, KChatBackendBridgeError> {
    let Some(authority) = active_authority() else {
        return Ok(false);
    };
    let refreshed = authority
        .refresh_if_needed()
        .await
        .map_err(|e| KChatBackendBridgeError::Attestation(e.to_string()))?;
    Ok(refreshed)
}

/// Helper used by integration tests to install a pre-signed-in
/// client into the bridge slot directly (e.g. one already pointed
/// at the in-process axum fixture server). Not exposed via N-API.
pub fn install_client_for_tests(client: Arc<KChatBackendClient>) {
    *client_slot().lock() = Some(client);
}

/// Helper used by integration tests to clear the bridge slot
/// between cases without going through a full sign-out.
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
) -> Result<ed25519_dalek::VerifyingKey, KChatBackendBridgeError> {
    decode_verifying_key(b64).map_err(|e| KChatBackendBridgeError::Attestation(e.to_string()))
}
