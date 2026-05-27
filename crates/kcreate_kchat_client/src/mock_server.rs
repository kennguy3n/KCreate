//! In-process reference implementation of the KChat Desktop server
//! side.
//!
//! The mock server is the canonical Rust implementation of the
//! protocol defined in [`super::protocol_spec.md`]. It is used by
//! KCreate's own test suite to exercise the client end-to-end
//! without needing a running `uney-chat-desktop`, and it is
//! intentionally shaped so the uney-chat-desktop team can read it
//! as a behavioural spec while writing the production server side.
//!
//! The mock signs membership attestations with a deterministic
//! Ed25519 key so tests can reproduce the same wire bytes across
//! runs.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use ed25519_dalek::SigningKey;
use kcreate_collab::kchat::{KChatGroupId, KChatMembership};
use kcreate_collab::peer::PeerId;
use parking_lot::Mutex;
use serde_json::Value as JsonValue;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::protocol::{
    CommunitiesListResult, CommunityEvent, ConversationsListParams, ConversationsListResult,
    ErrorCode, EventsSubscribeParams, EventsSubscribeResult, EventsUnsubscribeParams,
    GetMembersParams, GetMembersResult, GetMembershipParams, KChatCommunity, KChatCommunityMember,
    KChatConversation, KChatConversationType, KChatIdentity, KChatRole, MembershipAttestation,
    PostMessageParams, PostMessageResult, RpcError, RpcNotification, RpcRequest, RpcResponse,
    JSONRPC_VERSION,
};
use crate::transport::{make_error_response, make_ok_response};

/// All state the mock server tracks. Snapshot-cloneable so the
/// renderer's UI tests can pre-seed a deterministic configuration.
#[derive(Debug, Clone)]
pub struct MockState {
    pub identity: KChatIdentity,
    pub communities: Vec<KChatCommunity>,
    pub members_by_community: HashMap<String, Vec<KChatCommunityMember>>,
    pub conversations_by_community: HashMap<String, Vec<KChatConversation>>,
    /// Lifetime applied to issued attestations. The mock signs every
    /// attestation with the deterministic key below.
    pub attestation_lifetime: chrono::Duration,
    /// Override the `issued_at` of every minted attestation. `None`
    /// (default) uses `Utc::now()`. Tests use this to mint
    /// near-expired attestations and verify auto-refresh.
    pub attestation_issued_at: Option<DateTime<Utc>>,
    /// Counter for mock-server-side ids (messages, subscriptions).
    pub next_id: u64,
}

impl MockState {
    /// Build a minimal but non-trivial fixture: one community, one
    /// channel, two members (the local user and one peer).
    #[must_use]
    pub fn fixture() -> Self {
        let identity = KChatIdentity {
            jid: "alice@kchat.com".into(),
            display_name: "Alice".into(),
            public_key: "Q3Iv8AbLOXuIVu7uy_oWXqIRA8DEdLBvIRfeczcM3Lo".into(),
            peer_id: "alice-peer-id".into(),
        };
        let community = KChatCommunity {
            id: "comm-test".into(),
            name: "Test Community".into(),
            description: Some("Community used by the mock server".into()),
            member_count: 2,
            role: KChatRole::Owner,
        };
        let alice_member = KChatCommunityMember {
            jid: identity.jid.clone(),
            display_name: identity.display_name.clone(),
            public_key: identity.public_key.clone(),
            peer_id: identity.peer_id.clone(),
            role: KChatRole::Owner,
        };
        let bob_member = KChatCommunityMember {
            jid: "bob@kchat.com".into(),
            display_name: "Bob".into(),
            public_key: "BkSe7p1pPdvgrIIRZHRgIm1xQ-Q98xsg2vU9_pSdIBs".into(),
            peer_id: "bob-peer-id".into(),
            role: KChatRole::Member,
        };
        let conversation = KChatConversation {
            id: "conv-general".into(),
            name: "general".into(),
            community_id: community.id.clone(),
            conversation_type: KChatConversationType::Channel,
        };
        let mut members_by_community = HashMap::new();
        members_by_community.insert(community.id.clone(), vec![alice_member, bob_member]);
        let mut conversations_by_community = HashMap::new();
        conversations_by_community.insert(community.id.clone(), vec![conversation]);

        Self {
            identity,
            communities: vec![community],
            members_by_community,
            conversations_by_community,
            attestation_lifetime: chrono::Duration::hours(1),
            attestation_issued_at: None,
            next_id: 1,
        }
    }
}

/// Handle to a running mock server task. Dropping the handle does
/// not stop the server; call [`Self::shutdown`] to terminate
/// gracefully.
#[allow(missing_debug_implementations)]
pub struct MockServerHandle {
    pub state: Arc<Mutex<MockState>>,
    /// Channel for the harness to push a notification onto every
    /// connected client (after they have subscribed).
    notify_tx: mpsc::UnboundedSender<CommunityEvent>,
    join: JoinHandle<()>,
    shutdown_tx: tokio::sync::watch::Sender<bool>,
}

impl MockServerHandle {
    /// Push a community event to every connected client whose
    /// subscription matches the event's `communityId`.
    pub fn push_event(&self, event: CommunityEvent) {
        let _ = self.notify_tx.send(event);
    }

    /// Initiate graceful shutdown.
    pub async fn shutdown(self) {
        let _ = self.shutdown_tx.send(true);
        let _ = self.join.await;
    }
}

/// Spawn the mock server bound to a single connected stream. Used
/// in unit tests where the client side runs in the same process and
/// we use `tokio::io::duplex` to splice the two ends together.
///
/// Returns the handle and the deterministic Ed25519 signing key used
/// to mint attestations (callers need the `verifying_key()` to seed
/// the authority's trust root).
pub fn spawn_single_stream<S>(
    state: MockState,
    stream: S,
) -> (MockServerHandle, ed25519_dalek::VerifyingKey)
where
    S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
    let issuer = mock_issuer();
    let issuer_public = issuer.verifying_key();
    let state = Arc::new(Mutex::new(state));
    let (notify_tx, mut notify_rx) = mpsc::unbounded_channel::<CommunityEvent>();
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);

    let conn_state = state.clone();
    let join = tokio::spawn(async move {
        let (read_half, write_half) = tokio::io::split(stream);
        let mut buf = BufReader::with_capacity(64 * 1024, read_half);
        let writer = Arc::new(tokio::sync::Mutex::new(write_half));

        // Per-connection subscription roster. Stored locally so each
        // test stream sees only its own subscriptions.
        let subs: Arc<Mutex<HashMap<String, String>>> = Arc::new(Mutex::new(HashMap::new()));

        let writer_for_notify = writer.clone();
        let subs_for_notify = subs.clone();
        let notify_task: JoinHandle<()> = tokio::spawn(async move {
            while let Some(event) = notify_rx.recv().await {
                let subscription = {
                    let map = subs_for_notify.lock();
                    map.iter()
                        .find(|(_, community)| community.as_str() == event.community_id)
                        .map(|(sub_id, _)| sub_id.clone())
                };
                let Some(sub_id) = subscription else { continue };
                let mut tagged = event.clone();
                tagged.subscription_id = sub_id;
                let notif = RpcNotification {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    method: "kchat.events.notify".into(),
                    params: serde_json::to_value(&tagged).unwrap_or(JsonValue::Null),
                };
                let mut line = match serde_json::to_vec(&notif) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                line.push(b'\n');
                let mut w = writer_for_notify.lock().await;
                if w.write_all(&line).await.is_err() {
                    break;
                }
                if w.flush().await.is_err() {
                    break;
                }
            }
        });

        let mut line = Vec::with_capacity(2048);
        loop {
            line.clear();
            let read = tokio::select! {
                res = buf.read_until(b'\n', &mut line) => res,
                res = shutdown_rx.changed() => {
                    if res.is_err() || *shutdown_rx.borrow() {
                        break;
                    }
                    continue;
                }
            };
            match read {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    if line.last() == Some(&b'\n') {
                        line.pop();
                    }
                    if line.is_empty() {
                        continue;
                    }
                    let resp = handle_request(&line, &conn_state, &issuer, &subs);
                    let mut bytes = match serde_json::to_vec(&resp) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };
                    bytes.push(b'\n');
                    let mut w = writer.lock().await;
                    if w.write_all(&bytes).await.is_err() {
                        break;
                    }
                    if w.flush().await.is_err() {
                        break;
                    }
                }
            }
        }
        notify_task.abort();
    });

    (
        MockServerHandle {
            state,
            notify_tx,
            join,
            shutdown_tx,
        },
        issuer_public,
    )
}

/// Spawn a mock server bound to a Unix-domain socket at `path` so
/// the integration test suite can exercise the production
/// connect-by-path code path. Returns the handle plus the
/// deterministic issuer verifying key.
#[cfg(unix)]
pub async fn spawn_unix_listener(
    state: MockState,
    path: &Path,
) -> std::io::Result<(MockServerHandle, ed25519_dalek::VerifyingKey)> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    if path.exists() {
        tokio::fs::remove_file(path).await?;
    }
    let listener = tokio::net::UnixListener::bind(path)?;
    let issuer = mock_issuer();
    let issuer_public = issuer.verifying_key();
    let state = Arc::new(Mutex::new(state));
    let (notify_tx, _notify_rx) = mpsc::unbounded_channel::<CommunityEvent>();
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);

    let conn_state = state.clone();
    let notify_tx_for_task = notify_tx.clone();
    let join = tokio::spawn(async move {
        loop {
            let accept_fut = listener.accept();
            tokio::pin!(accept_fut);
            let stream = tokio::select! {
                res = &mut accept_fut => match res {
                    Ok((stream, _)) => stream,
                    Err(e) => {
                        tracing::warn!(error = %e, "mock-server: accept failed");
                        break;
                    }
                },
                res = shutdown_rx.changed() => {
                    if res.is_err() || *shutdown_rx.borrow() {
                        break;
                    }
                    continue;
                }
            };
            let per_conn_state = conn_state.clone();
            let per_conn_issuer = issuer.clone();
            let per_conn_notify = notify_tx_for_task.clone();
            tokio::spawn(serve_unix_conn(
                stream,
                per_conn_state,
                per_conn_issuer,
                per_conn_notify,
            ));
        }
    });

    Ok((
        MockServerHandle {
            state,
            notify_tx,
            join,
            shutdown_tx,
        },
        issuer_public,
    ))
}

#[cfg(unix)]
async fn serve_unix_conn(
    stream: tokio::net::UnixStream,
    state: Arc<Mutex<MockState>>,
    issuer: SigningKey,
    _notify_tx: mpsc::UnboundedSender<CommunityEvent>,
) {
    let (read_half, mut write_half) = stream.into_split();
    let mut buf = BufReader::with_capacity(64 * 1024, read_half);
    let subs: Arc<Mutex<HashMap<String, String>>> = Arc::new(Mutex::new(HashMap::new()));
    let mut line = Vec::with_capacity(2048);
    loop {
        line.clear();
        match buf.read_until(b'\n', &mut line).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                if line.last() == Some(&b'\n') {
                    line.pop();
                }
                if line.is_empty() {
                    continue;
                }
                let resp = handle_request(&line, &state, &issuer, &subs);
                let mut bytes = match serde_json::to_vec(&resp) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                bytes.push(b'\n');
                if write_half.write_all(&bytes).await.is_err() {
                    break;
                }
                if write_half.flush().await.is_err() {
                    break;
                }
            }
        }
    }
}

fn mock_issuer() -> SigningKey {
    SigningKey::from_bytes(&[0xab; 32])
}

fn handle_request(
    raw: &[u8],
    state: &Arc<Mutex<MockState>>,
    issuer: &SigningKey,
    subs: &Arc<Mutex<HashMap<String, String>>>,
) -> RpcResponse {
    let req: RpcRequest = match serde_json::from_slice(raw) {
        Ok(v) => v,
        Err(_) => {
            return RpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: "0".into(),
                result: None,
                error: Some(RpcError {
                    code: ErrorCode::ParseError.as_i32(),
                    message: "invalid JSON-RPC request".into(),
                    data: None,
                }),
            };
        }
    };
    if req.jsonrpc != JSONRPC_VERSION {
        return make_error_response(
            req.id,
            ErrorCode::InvalidRequest,
            "unsupported jsonrpc version",
        );
    }
    match req.method.as_str() {
        "kchat.identity.get" => {
            // Pull the identity out under the lock first so the
            // `state.lock()` guard does not outlive the match
            // scrutinee (clippy::significant_drop_in_scrutinee).
            let identity = state.lock().identity.clone();
            match make_ok_response(req.id.clone(), &identity) {
                Ok(r) => r,
                Err(e) => make_error_response(req.id, ErrorCode::InternalError, e.to_string()),
            }
        }
        "kchat.communities.list" => {
            let result = CommunitiesListResult {
                communities: state.lock().communities.clone(),
            };
            make_ok_response(req.id.clone(), &result).unwrap_or_else(|e| {
                make_error_response(req.id, ErrorCode::InternalError, e.to_string())
            })
        }
        "kchat.communities.getMembers" => {
            let params: GetMembersParams = match parse_params(req.params.as_ref()) {
                Ok(p) => p,
                Err(e) => return make_error_response(req.id, ErrorCode::InvalidParams, e),
            };
            let members = state
                .lock()
                .members_by_community
                .get(&params.community_id)
                .cloned();
            match members {
                Some(members) => {
                    let res = GetMembersResult { members };
                    make_ok_response(req.id.clone(), &res).unwrap_or_else(|e| {
                        make_error_response(req.id, ErrorCode::InternalError, e.to_string())
                    })
                }
                None => make_error_response(req.id, ErrorCode::NotFound, "unknown community"),
            }
        }
        "kchat.communities.getMembership" => {
            let params: GetMembershipParams = match parse_params(req.params.as_ref()) {
                Ok(p) => p,
                Err(e) => return make_error_response(req.id, ErrorCode::InvalidParams, e),
            };
            let snapshot = state.lock().clone();
            let Some(_community) = snapshot
                .communities
                .iter()
                .find(|c| c.id == params.community_id)
            else {
                return make_error_response(req.id, ErrorCode::NotFound, "unknown community");
            };
            let issued_at = snapshot.attestation_issued_at.unwrap_or_else(Utc::now);
            let expires_at = issued_at + snapshot.attestation_lifetime;
            let attestation = match mint_attestation(
                issuer,
                &params.community_id,
                &snapshot.identity,
                issued_at,
                expires_at,
            ) {
                Ok(a) => a,
                Err(e) => {
                    return make_error_response(req.id, ErrorCode::InternalError, e);
                }
            };
            make_ok_response(req.id.clone(), &attestation).unwrap_or_else(|e| {
                make_error_response(req.id, ErrorCode::InternalError, e.to_string())
            })
        }
        "kchat.conversations.list" => {
            let params: ConversationsListParams = match parse_params(req.params.as_ref()) {
                Ok(p) => p,
                Err(e) => return make_error_response(req.id, ErrorCode::InvalidParams, e),
            };
            let conversations = state
                .lock()
                .conversations_by_community
                .get(&params.community_id)
                .cloned();
            match conversations {
                Some(conversations) => {
                    let res = ConversationsListResult { conversations };
                    make_ok_response(req.id.clone(), &res).unwrap_or_else(|e| {
                        make_error_response(req.id, ErrorCode::InternalError, e.to_string())
                    })
                }
                None => make_error_response(req.id, ErrorCode::NotFound, "unknown community"),
            }
        }
        "kchat.conversations.postMessage" => {
            let _params: PostMessageParams = match parse_params(req.params.as_ref()) {
                Ok(p) => p,
                Err(e) => return make_error_response(req.id, ErrorCode::InvalidParams, e),
            };
            let mut guard = state.lock();
            guard.next_id += 1;
            let id = guard.next_id;
            drop(guard);
            let result = PostMessageResult {
                message_id: format!("msg-{id}"),
                posted_at: Utc::now(),
            };
            make_ok_response(req.id.clone(), &result).unwrap_or_else(|e| {
                make_error_response(req.id, ErrorCode::InternalError, e.to_string())
            })
        }
        "kchat.events.subscribe" => {
            let params: EventsSubscribeParams = match parse_params(req.params.as_ref()) {
                Ok(p) => p,
                Err(e) => return make_error_response(req.id, ErrorCode::InvalidParams, e),
            };
            let id = Uuid::new_v4().simple().to_string();
            {
                let mut guard = subs.lock();
                if guard.values().any(|c| *c == params.community_id) {
                    return make_error_response(
                        req.id,
                        ErrorCode::AlreadySubscribed,
                        "community already subscribed",
                    );
                }
                guard.insert(id.clone(), params.community_id);
            }
            let result = EventsSubscribeResult {
                subscription_id: id,
            };
            make_ok_response(req.id.clone(), &result).unwrap_or_else(|e| {
                make_error_response(req.id, ErrorCode::InternalError, e.to_string())
            })
        }
        "kchat.events.unsubscribe" => {
            let params: EventsUnsubscribeParams = match parse_params(req.params.as_ref()) {
                Ok(p) => p,
                Err(e) => return make_error_response(req.id, ErrorCode::InvalidParams, e),
            };
            subs.lock().remove(&params.subscription_id);
            make_ok_response(
                req.id.clone(),
                &JsonValue::Object(serde_json::Map::default()),
            )
            .unwrap_or_else(|e| {
                make_error_response(req.id, ErrorCode::InternalError, e.to_string())
            })
        }
        other => make_error_response(
            req.id,
            ErrorCode::MethodNotFound,
            format!("unknown method {other}"),
        ),
    }
}

fn parse_params<P: serde::de::DeserializeOwned>(params: Option<&JsonValue>) -> Result<P, String> {
    let value = params.cloned().unwrap_or(JsonValue::Null);
    serde_json::from_value(value).map_err(|e| e.to_string())
}

fn mint_attestation(
    issuer: &SigningKey,
    community_id: &str,
    identity: &KChatIdentity,
    issued_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
) -> Result<MembershipAttestation, String> {
    let group_id = KChatGroupId::new(community_id.to_string())
        .map_err(|e| format!("invalid groupId: {e:?}"))?;
    // We use the identity's peer_id as-is (the mock server doesn't
    // bother re-deriving from public_key — the production server will,
    // but the wire shape is the same).
    let peer_id_str = identity.peer_id.clone();
    let peer_id: PeerId = serde_json::from_value(JsonValue::String(peer_id_str))
        .map_err(|e| format!("invalid peerId: {e}"))?;
    let membership = KChatMembership::issue(
        group_id,
        peer_id,
        identity.public_key.clone(),
        issued_at,
        expires_at,
        issuer,
    )
    .map_err(|e| format!("issue failed: {e:?}"))?;
    Ok(MembershipAttestation {
        issuer_public_key: membership.issuer_public_key.clone(),
        group_id: membership.group_id.as_str().to_string(),
        peer_id: membership.peer_id.as_str().to_string(),
        peer_public_key: membership.peer_public_key.clone(),
        issued_at: membership.issued_at,
        expires_at: membership.expires_at,
        signature: membership.signature.clone(),
    })
}

/// Quick helper for tests: mint an identity that matches an
/// arbitrary `kcreate_collab::PeerKey`. Replaces the
/// [`MockState::fixture`] identity in-place so attestations are
/// bound to the same Ed25519 key the client uses to sign envelopes.
pub fn replace_identity_with_peer_key(
    state: &mut MockState,
    key: &kcreate_collab::peer::PeerKey,
    display_name: impl Into<String>,
    jid: impl Into<String>,
) {
    let display_name = display_name.into();
    let identity = key.identity(&display_name);
    let new_identity = KChatIdentity {
        jid: jid.into(),
        display_name,
        public_key: identity.public_key,
        peer_id: identity.peer_id.as_str().to_string(),
    };
    // Update the owner-row entry in every community's member list so
    // the roster stays consistent.
    for members in state.members_by_community.values_mut() {
        if let Some(slot) = members.iter_mut().find(|m| m.role == KChatRole::Owner) {
            *slot = KChatCommunityMember {
                jid: new_identity.jid.clone(),
                display_name: new_identity.display_name.clone(),
                public_key: new_identity.public_key.clone(),
                peer_id: new_identity.peer_id.clone(),
                role: KChatRole::Owner,
            };
        }
    }
    state.identity = new_identity;
    // Keep the lifetime defaults so attestations issued from this
    // state pass the live-window check.
    let _ = Duration::from_secs(0);
}
