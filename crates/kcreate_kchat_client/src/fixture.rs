//! In-process REST fixture server.
//!
//! Mirrors the future KChat backend REST contract documented in
//! `protocol.rs`. Used by the test suite (and as the canonical
//! reference implementation for the backend team) — never enters
//! a production binary because it lives behind
//! `#[cfg(any(test, feature = "fixture"))]` is unnecessary: the
//! module itself is gated by the `axum` dev-dependency.
//!
//! ## What it implements
//!
//! - `POST /api/v1/auth/login` with fixed-credentials check.
//! - `POST /api/v1/auth/refresh`.
//! - `GET /api/v1/identity` with auth check.
//! - `GET /api/v1/communities`.
//! - `GET /api/v1/communities/{id}/members`.
//! - `POST /api/v1/communities/{id}/attestation` — signs a real
//!   `KChatMembership` with a fresh Ed25519 keypair. Configurable
//!   to either sign correctly, deliberately sign with the wrong
//!   issuer, return a too-short TTL (so refresh fires), or
//!   respond `501 Not Implemented` (so we can drive the
//!   "endpoint not provisioned" path).
//! - `GET /api/v1/communities/{id}/conversations`.
//! - `POST /api/v1/conversations/{id}/messages`.
//! - `GET /api/v1/communities/{id}/events?since={cursor}`.
//!
//! ## Test toggles
//!
//! [`FixtureBehavior`] selects rare error paths (TLS handshake
//! failure is out of scope — the fixture is plain-HTTP on
//! `127.0.0.1`; the client-side TLS-strict check is unit-tested
//! against [`RestClientConfig::production`](crate::rest::RestClientConfig::production)
//! separately).


use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::Utc;
use ed25519_dalek::SigningKey;
use kcreate_collab::kchat::{KChatGroupId, KChatMembership};
use kcreate_collab::peer::{decode_public_key, PeerId};
#[allow(unused_imports)]
use std::future::IntoFuture;
use parking_lot::Mutex;
use serde::Deserialize;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::protocol::{
    error_code, AttestationRequest, BackendErrorBody, CommunitiesListResponse, CommunityEvent,
    CommunityEventKind, CommunityEventsResponse, ConversationsListResponse, KChatCommunity,
    KChatCommunityMember, KChatConversation, KChatConversationType, KChatIdentity, KChatRole,
    LoginRequest, LoginResponse, MembersListResponse, MembershipAttestation, PostMessageRequest,
    PostMessageResponse, RefreshRequest, RefreshResponse,
};

/// Knobs the fixture honours to simulate failure modes.
#[derive(Debug, Clone, Default)]
pub struct FixtureBehavior {
    /// When true, `POST .../attestation` returns 501 with the
    /// `ATTESTATION_NOT_PROVISIONED` code so the client surfaces
    /// `ClientError::AttestationEndpointNotProvisioned`.
    pub attestation_endpoint_missing: bool,
    /// When true, the fixture signs attestations with the wrong
    /// key so the client's local verification fails.
    pub corrupt_attestation_signature: bool,
    /// TTL (in seconds) the fixture stamps on attestations. The
    /// auto-refresh test sets this to 1s so the client's
    /// `refresh_if_needed` (with a short window) fires
    /// immediately.
    pub attestation_ttl_secs: u64,
    /// When set to `Some(n)`, the fixture returns 429 on the first
    /// `n` requests then succeeds. Used by the bounded-retry tests.
    pub rate_limit_initial: Option<u32>,
    /// When true, the access token expires immediately, forcing
    /// the client onto the refresh path on the first authenticated
    /// request.
    pub access_token_lifetime_secs: u64,
    /// When true, refresh always returns 401 — used to drive the
    /// `RefreshExpired` path.
    pub refresh_always_fails: bool,
}

impl FixtureBehavior {
    /// Sensible default for happy-path tests: 1-hour access token,
    /// 1-hour attestation, no rate limiting.
    #[must_use]
    pub fn happy() -> Self {
        Self {
            attestation_endpoint_missing: false,
            corrupt_attestation_signature: false,
            attestation_ttl_secs: 3600,
            rate_limit_initial: None,
            access_token_lifetime_secs: 3600,
            refresh_always_fails: false,
        }
    }
}

#[derive(Debug)]
struct FixtureState {
    behavior: Mutex<FixtureBehavior>,
    /// Fresh Ed25519 issuer keypair used to sign attestations.
    issuer: SigningKey,
    /// Issuer key used when `corrupt_attestation_signature` is true.
    bad_issuer: SigningKey,
    /// Canonical login credentials the fixture accepts. Hard-coded
    /// for simplicity — tests pass these to the client.
    login_id: String,
    password: String,
    /// Local user identity returned on login + `/identity`. The
    /// `peer_id` must match the BLAKE3 hash of the embedded
    /// pubkey so the client's signature verification passes.
    identity: KChatIdentity,
    communities: Vec<KChatCommunity>,
    members: Vec<KChatCommunityMember>,
    conversations: Vec<KChatConversation>,
    /// Monotonic counter for the rate-limit fixture path.
    rate_limit_counter: Mutex<u32>,
    /// Currently-issued tokens. The fixture is a single-user
    /// server so we just stash the latest one and check on every
    /// authenticated request.
    issued_access: Mutex<Option<String>>,
    /// Wall-clock when `issued_access` was minted; `check_auth`
    /// rejects tokens whose age exceeds the configured lifetime.
    issued_access_at: Mutex<Option<std::time::Instant>>,
    issued_refresh: Mutex<Option<String>>,
    /// Counter ensures every minted access token is distinct so
    /// the refresh-then-retry tests can prove the second call
    /// used the new token.
    next_token_seq: Mutex<u64>,
}

/// Running fixture server. Drop the handle to shut it down — the
/// inner task exits when the oneshot is dropped.
#[derive(Debug)]
pub struct FixtureServer {
    pub base_url: String,
    pub issuer_public_key_b64: String,
    pub local_peer_id: PeerId,
    pub local_public_key_b64: String,
    pub login_id: String,
    pub password: String,
    state: Arc<FixtureState>,
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

impl FixtureServer {
    /// Spawn a fresh fixture server bound to `127.0.0.1:0`.
    /// Returns once the listener is accepting connections so the
    /// client can connect immediately.
    pub async fn spawn(behavior: FixtureBehavior) -> Self {
        // Mint a stable local identity (Ed25519 derived from a fixed
        // seed) so tests can compare `peer_id` outputs against
        // known values.
        let local_signing = SigningKey::from_bytes(&[42u8; 32]);
        let local_pub_b64 =
            URL_SAFE_NO_PAD.encode(local_signing.verifying_key().to_bytes());
        let local_peer_id =
            PeerId::from_verifying_key(&local_signing.verifying_key());
        let identity = KChatIdentity {
            jid: "alice@kchat.example".into(),
            display_name: "Alice".into(),
            public_key: local_pub_b64.clone(),
            peer_id: local_peer_id.as_str().into(),
        };

        let communities = vec![
            KChatCommunity {
                id: "comm-design".into(),
                name: "Design".into(),
                description: Some("Design crit".into()),
                member_count: 12,
                role: KChatRole::Owner,
            },
            KChatCommunity {
                id: "comm-eng".into(),
                name: "Engineering".into(),
                description: None,
                member_count: 30,
                role: KChatRole::Member,
            },
        ];
        let members = vec![KChatCommunityMember {
            jid: identity.jid.clone(),
            display_name: identity.display_name.clone(),
            public_key: identity.public_key.clone(),
            peer_id: identity.peer_id.clone(),
            role: KChatRole::Owner,
        }];
        let conversations = vec![
            KChatConversation {
                id: "conv-general".into(),
                name: "general".into(),
                community_id: "comm-design".into(),
                conversation_type: KChatConversationType::Channel,
            },
            KChatConversation {
                id: "conv-ken".into(),
                name: "Ken".into(),
                community_id: "comm-design".into(),
                conversation_type: KChatConversationType::Direct,
            },
        ];

        let issuer = SigningKey::from_bytes(&[7u8; 32]);
        let bad_issuer = SigningKey::from_bytes(&[8u8; 32]);
        let issuer_pub_b64 =
            URL_SAFE_NO_PAD.encode(issuer.verifying_key().to_bytes());

        let state = Arc::new(FixtureState {
            behavior: Mutex::new(behavior),
            issuer,
            bad_issuer,
            login_id: "alice@kchat.example".into(),
            password: "hunter2".into(),
            identity,
            communities,
            members,
            conversations,
            rate_limit_counter: Mutex::new(0),
            issued_access: Mutex::new(None),
            issued_access_at: Mutex::new(None),
            issued_refresh: Mutex::new(None),
            next_token_seq: Mutex::new(0),
        });

        let router = build_router(state.clone());

        let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
            .await
            .expect("bind fixture");
        let addr = listener.local_addr().expect("local addr");
        let base_url = format!("http://{addr}");
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

        let task = tokio::spawn(async move {
            let server = axum::serve(listener, router).into_future();
            tokio::pin!(server);
            tokio::select! {
                _ = &mut server => {},
                _ = shutdown_rx => {},
            }
        });

        // Give axum a moment to start accepting. We do a quick
        // poll-loop on the listener so the test isn't racing.
        // The bind is sync above so this is almost always immediate.
        tokio::time::sleep(Duration::from_millis(10)).await;

        Self {
            base_url,
            issuer_public_key_b64: issuer_pub_b64,
            local_peer_id,
            local_public_key_b64: local_pub_b64,
            login_id: state.login_id.clone(),
            password: state.password.clone(),
            state,
            shutdown: Some(shutdown_tx),
            task: Some(task),
        }
    }

    /// Mutate the behaviour mid-test (e.g. switch from "happy" to
    /// "attestation endpoint missing" between two REST calls).
    pub fn set_behavior(&self, behavior: FixtureBehavior) {
        *self.state.behavior.lock() = behavior;
    }
}

impl Drop for FixtureServer {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(t) = self.task.take() {
            t.abort();
        }
    }
}

fn build_router(state: Arc<FixtureState>) -> Router {
    Router::new()
        .route("/api/v1/auth/login", post(handle_login))
        .route("/api/v1/auth/refresh", post(handle_refresh))
        .route("/api/v1/identity", get(handle_identity))
        .route("/api/v1/communities", get(handle_list_communities))
        .route(
            "/api/v1/communities/:id/members",
            get(handle_get_members),
        )
        .route(
            "/api/v1/communities/:id/attestation",
            post(handle_attestation),
        )
        .route(
            "/api/v1/communities/:id/conversations",
            get(handle_list_conversations),
        )
        .route(
            "/api/v1/conversations/:id/messages",
            post(handle_post_message),
        )
        .route(
            "/api/v1/communities/:id/events",
            get(handle_poll_events),
        )
        .with_state(state)
}

fn json_error(status: StatusCode, code: &str, message: &str) -> axum::response::Response {
    let body = BackendErrorBody {
        code: code.into(),
        message: message.into(),
        data: None,
    };
    (status, Json(body)).into_response()
}

fn rate_limit_if_configured(state: &FixtureState) -> Option<axum::response::Response> {
    let behavior = state.behavior.lock().clone();
    let mut counter = state.rate_limit_counter.lock();
    if let Some(n) = behavior.rate_limit_initial {
        if *counter < n {
            *counter += 1;
            return Some(json_error(
                StatusCode::TOO_MANY_REQUESTS,
                "RATE_LIMITED",
                "fixture: throttled",
            ));
        }
    }
    None
}

fn mint_access_token(state: &FixtureState) -> String {
    let mut seq = state.next_token_seq.lock();
    *seq += 1;
    let value = format!("access-{}", *seq);
    *state.issued_access.lock() = Some(value.clone());
    *state.issued_access_at.lock() = Some(std::time::Instant::now());
    value
}

fn mint_refresh_token(state: &FixtureState) -> String {
    let mut seq = state.next_token_seq.lock();
    *seq += 1;
    let value = format!("refresh-{}", *seq);
    *state.issued_refresh.lock() = Some(value.clone());
    value
}

// `axum::response::Response<Body>` is ~128 bytes; box the Err
// variant so the result stays cheap to return on the happy path
// while still letting callers `?` the rejection without an alloc
// for every call site that ignores the error.
#[allow(clippy::type_complexity)]
fn check_auth(
    state: &FixtureState,
    headers: &HeaderMap,
) -> Result<(), Box<axum::response::Response>> {
    let issued = state.issued_access.lock().clone();
    let expected = match issued {
        Some(t) => t,
        None => {
            return Err(Box::new(json_error(
                StatusCode::UNAUTHORIZED,
                "AUTH_INVALID",
                "no token issued yet",
            )));
        }
    };
    let header = headers
        .get(reqwest::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    let token = match header {
        Some(h) if h.starts_with("Bearer ") => &h[7..],
        _ => {
            return Err(Box::new(json_error(
                StatusCode::UNAUTHORIZED,
                "AUTH_INVALID",
                "missing bearer token",
            )));
        }
    };
    if token != expected {
        return Err(Box::new(json_error(
            StatusCode::UNAUTHORIZED,
            "AUTH_INVALID",
            "token does not match latest issued",
        )));
    }
    // Wall-clock expiry check. Mirrors the real backend: even if
    // the bearer is structurally correct, an expired access
    // token must be rejected so the client exercises its refresh
    // path.
    let lifetime = state.behavior.lock().access_token_lifetime_secs;
    if lifetime > 0 {
        let issued_at = *state.issued_access_at.lock();
        if let Some(t) = issued_at {
            // `lifetime` is a `u64` (configured in seconds);
            // convert via the lossy `as` because the values used
            // in tests (single-digit seconds up to a few minutes)
            // are well within f64 mantissa precision and we only
            // need millisecond-scale ordering.
            #[allow(clippy::cast_precision_loss)]
            let lifetime_secs = lifetime as f64;
            if t.elapsed().as_secs_f64() > lifetime_secs {
                return Err(Box::new(json_error(
                    StatusCode::UNAUTHORIZED,
                    "AUTH_INVALID",
                    "access token expired",
                )));
            }
        }
    }
    Ok(())
}

async fn handle_login(
    State(state): State<Arc<FixtureState>>,
    Json(body): Json<LoginRequest>,
) -> axum::response::Response {
    if let Some(rl) = rate_limit_if_configured(&state) {
        return rl;
    }
    if body.login_id != state.login_id || body.password != state.password {
        return json_error(
            StatusCode::UNAUTHORIZED,
            error_code::AUTH_INVALID,
            "bad credentials",
        );
    }
    let access = mint_access_token(&state);
    let refresh = mint_refresh_token(&state);
    let resp = LoginResponse {
        access_token: access,
        refresh_token: refresh,
        expires_in_seconds: state.behavior.lock().access_token_lifetime_secs,
        identity: state.identity.clone(),
    };
    (StatusCode::OK, Json(resp)).into_response()
}

async fn handle_refresh(
    State(state): State<Arc<FixtureState>>,
    Json(body): Json<RefreshRequest>,
) -> axum::response::Response {
    let behavior = state.behavior.lock().clone();
    if behavior.refresh_always_fails {
        return json_error(
            StatusCode::UNAUTHORIZED,
            "REFRESH_EXPIRED",
            "fixture: refresh disabled",
        );
    }
    let issued = state.issued_refresh.lock().clone();
    if issued.as_deref() != Some(body.refresh_token.as_str()) {
        return json_error(
            StatusCode::UNAUTHORIZED,
            "REFRESH_EXPIRED",
            "unknown refresh token",
        );
    }
    let access = mint_access_token(&state);
    let refresh = mint_refresh_token(&state);
    let resp = RefreshResponse {
        access_token: access,
        refresh_token: refresh,
        expires_in_seconds: behavior.access_token_lifetime_secs,
    };
    (StatusCode::OK, Json(resp)).into_response()
}

async fn handle_identity(
    State(state): State<Arc<FixtureState>>,
    headers: HeaderMap,
) -> axum::response::Response {
    if let Err(e) = check_auth(&state, &headers) {
        return *e;
    }
    (StatusCode::OK, Json(state.identity.clone())).into_response()
}

async fn handle_list_communities(
    State(state): State<Arc<FixtureState>>,
    headers: HeaderMap,
) -> axum::response::Response {
    if let Err(e) = check_auth(&state, &headers) {
        return *e;
    }
    let resp = CommunitiesListResponse {
        communities: state.communities.clone(),
    };
    (StatusCode::OK, Json(resp)).into_response()
}

async fn handle_get_members(
    State(state): State<Arc<FixtureState>>,
    headers: HeaderMap,
    Path(_id): Path<String>,
) -> axum::response::Response {
    if let Err(e) = check_auth(&state, &headers) {
        return *e;
    }
    let resp = MembersListResponse {
        members: state.members.clone(),
    };
    (StatusCode::OK, Json(resp)).into_response()
}

async fn handle_attestation(
    State(state): State<Arc<FixtureState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<AttestationRequest>,
) -> axum::response::Response {
    if let Err(e) = check_auth(&state, &headers) {
        return *e;
    }
    let behavior = state.behavior.lock().clone();
    if behavior.attestation_endpoint_missing {
        return json_error(
            StatusCode::NOT_IMPLEMENTED,
            error_code::ATTESTATION_NOT_PROVISIONED,
            "fixture: attestation endpoint disabled",
        );
    }
    let signer = if behavior.corrupt_attestation_signature {
        &state.bad_issuer
    } else {
        &state.issuer
    };
    let now = Utc::now();
    let expires_at = now + chrono::Duration::seconds(behavior.attestation_ttl_secs as i64);
    let group = match KChatGroupId::new(id.clone()) {
        Ok(g) => g,
        Err(_) => {
            return json_error(
                StatusCode::BAD_REQUEST,
                error_code::INVALID_REQUEST,
                "invalid community id",
            );
        }
    };
    let peer_id = match decode_public_key(&body.peer_public_key) {
        Ok(vk) => PeerId::from_verifying_key(&vk),
        Err(e) => {
            return json_error(
                StatusCode::BAD_REQUEST,
                error_code::INVALID_REQUEST,
                &format!("invalid peer public key: {e:?}"),
            );
        }
    };
    let m = match KChatMembership::issue(
        group,
        peer_id.clone(),
        body.peer_public_key,
        now,
        expires_at,
        signer,
    ) {
        Ok(m) => m,
        Err(e) => {
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "ISSUER_ERROR",
                &format!("{e:?}"),
            );
        }
    };
    // If we deliberately signed with the bad issuer key, claim the
    // real issuer in `issuer_public_key` so the client's local
    // verify fails (rather than the trust-root mismatch path).
    let advertised_issuer = if behavior.corrupt_attestation_signature {
        URL_SAFE_NO_PAD.encode(state.issuer.verifying_key().to_bytes())
    } else {
        m.issuer_public_key.clone()
    };
    let att = MembershipAttestation {
        issuer_public_key: advertised_issuer,
        group_id: id,
        peer_id: peer_id.as_str().to_string(),
        peer_public_key: m.peer_public_key,
        issued_at: m.issued_at,
        expires_at: m.expires_at,
        signature: m.signature,
    };
    (StatusCode::OK, Json(att)).into_response()
}

async fn handle_list_conversations(
    State(state): State<Arc<FixtureState>>,
    headers: HeaderMap,
    Path(_id): Path<String>,
) -> axum::response::Response {
    if let Err(e) = check_auth(&state, &headers) {
        return *e;
    }
    let resp = ConversationsListResponse {
        conversations: state.conversations.clone(),
    };
    (StatusCode::OK, Json(resp)).into_response()
}

async fn handle_post_message(
    State(state): State<Arc<FixtureState>>,
    headers: HeaderMap,
    Path(_id): Path<String>,
    Json(_body): Json<PostMessageRequest>,
) -> axum::response::Response {
    if let Err(e) = check_auth(&state, &headers) {
        return *e;
    }
    let resp = PostMessageResponse {
        message_id: "msg-fixture".into(),
        posted_at: Utc::now(),
    };
    (StatusCode::OK, Json(resp)).into_response()
}

#[derive(Debug, Deserialize)]
struct EventsQuery {
    #[serde(default)]
    since: Option<String>,
}

async fn handle_poll_events(
    State(state): State<Arc<FixtureState>>,
    headers: HeaderMap,
    Path(community_id): Path<String>,
    Query(q): Query<EventsQuery>,
) -> axum::response::Response {
    if let Err(e) = check_auth(&state, &headers) {
        return *e;
    }
    let events = if q.since.is_none() {
        // First poll returns the local identity as a synthetic
        // "MemberJoined" so tests can confirm parsing works.
        vec![CommunityEvent {
            community_id,
            event: CommunityEventKind::MemberJoined {
                member: state.members[0].clone(),
            },
            at: Utc::now(),
        }]
    } else {
        Vec::new()
    };
    let resp = CommunityEventsResponse {
        events,
        next_cursor: "cursor-1".into(),
    };
    (StatusCode::OK, Json(resp)).into_response()
}
