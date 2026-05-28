//! Integration tests for the KChat backend REST client.
//!
//! These tests drive the real [`KChatBackendClient`] against an
//! in-process [`fixture::FixtureServer`] (axum, plain-HTTP on
//! `127.0.0.1`). They exercise every documented REST endpoint
//! plus the 401-refresh-retry and 429-bounded-retry paths.

use std::time::Duration;

use kcreate_kchat_client::fixture::{FixtureBehavior, FixtureServer};
use kcreate_kchat_client::{
    ClientError, KChatBackendAuthority, KChatBackendClient, KChatConversationType, KChatRole,
    LoginRequest, PostMessageRequest,
};
use std::sync::Arc;

fn login_body(server: &FixtureServer) -> LoginRequest {
    LoginRequest {
        login_id: server.login_id.clone(),
        password: server.password.clone(),
        totp: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn login_returns_identity_and_caches_tokens() {
    let server = FixtureServer::spawn(FixtureBehavior::happy()).await;
    let client = KChatBackendClient::new_for_tests(&server.base_url).expect("client");
    let identity = client.login(&login_body(&server)).await.expect("login");
    assert_eq!(identity.jid, "alice@kchat.example");
    assert_eq!(identity.display_name, "Alice");
    assert!(client.cached_identity().is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn login_with_bad_password_returns_invalid_credentials() {
    let server = FixtureServer::spawn(FixtureBehavior::happy()).await;
    let client = KChatBackendClient::new_for_tests(&server.base_url).expect("client");
    let body = LoginRequest {
        login_id: server.login_id.clone(),
        password: "wrong".into(),
        totp: None,
    };
    let res = client.login(&body).await;
    assert!(matches!(res, Err(ClientError::InvalidCredentials { .. })));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unauthenticated_call_returns_not_authenticated() {
    let server = FixtureServer::spawn(FixtureBehavior::happy()).await;
    let client = KChatBackendClient::new_for_tests(&server.base_url).expect("client");
    let res = client.list_communities().await;
    assert!(matches!(res, Err(ClientError::NotAuthenticated)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_communities_returns_fixture_data() {
    let server = FixtureServer::spawn(FixtureBehavior::happy()).await;
    let client = KChatBackendClient::new_for_tests(&server.base_url).expect("client");
    client.login(&login_body(&server)).await.expect("login");
    let communities = client.list_communities().await.expect("list");
    assert_eq!(communities.len(), 2);
    assert_eq!(communities[0].id, "comm-design");
    assert_eq!(communities[0].role, KChatRole::Owner);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_conversations_returns_channel_and_direct() {
    let server = FixtureServer::spawn(FixtureBehavior::happy()).await;
    let client = KChatBackendClient::new_for_tests(&server.base_url).expect("client");
    client.login(&login_body(&server)).await.expect("login");
    let convs = client
        .list_conversations("comm-design")
        .await
        .expect("conversations");
    assert_eq!(convs.len(), 2);
    assert_eq!(convs[0].conversation_type, KChatConversationType::Channel);
    assert_eq!(convs[1].conversation_type, KChatConversationType::Direct);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn post_message_returns_message_id() {
    let server = FixtureServer::spawn(FixtureBehavior::happy()).await;
    let client = KChatBackendClient::new_for_tests(&server.base_url).expect("client");
    client.login(&login_body(&server)).await.expect("login");
    let body = PostMessageRequest {
        payload: serde_json::json!({ "hello": "world" }),
        content_type: Some("kcreate.invite.v1".into()),
    };
    let resp = client
        .post_message("conv-general", &body)
        .await
        .expect("post");
    assert_eq!(resp.message_id, "msg-fixture");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_membership_attestation_signs_and_verifies() {
    let server = FixtureServer::spawn(FixtureBehavior::happy()).await;
    let client = KChatBackendClient::new_for_tests(&server.base_url).expect("client");
    client.login(&login_body(&server)).await.expect("login");
    let att = client
        .get_membership_attestation("comm-design", &server.local_public_key_b64)
        .await
        .expect("attestation");
    assert_eq!(att.group_id, "comm-design");
    assert_eq!(att.peer_public_key, server.local_public_key_b64);
    // Install through the production-shape authority — verifies
    // the signature + binding locally.
    let client_arc = Arc::new(client);
    let authority = KChatBackendAuthority::install(
        client_arc,
        "comm-design",
        att,
        server.local_peer_id.clone(),
        server.local_public_key_b64.clone(),
        chrono::Utc::now(),
    )
    .expect("install");
    assert_eq!(authority.community_id(), "comm-design");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn attestation_endpoint_missing_returns_typed_error() {
    let mut behavior = FixtureBehavior::happy();
    behavior.attestation_endpoint_missing = true;
    let server = FixtureServer::spawn(behavior).await;
    let client = KChatBackendClient::new_for_tests(&server.base_url).expect("client");
    client.login(&login_body(&server)).await.expect("login");
    let res = client
        .get_membership_attestation("comm-design", &server.local_public_key_b64)
        .await;
    assert!(matches!(
        res,
        Err(ClientError::AttestationEndpointNotProvisioned { .. })
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn corrupt_attestation_signature_fails_install() {
    let mut behavior = FixtureBehavior::happy();
    behavior.corrupt_attestation_signature = true;
    let server = FixtureServer::spawn(behavior).await;
    let client = KChatBackendClient::new_for_tests(&server.base_url).expect("client");
    client.login(&login_body(&server)).await.expect("login");
    let att = client
        .get_membership_attestation("comm-design", &server.local_public_key_b64)
        .await
        .expect("attestation");
    let client_arc = Arc::new(client);
    let res = KChatBackendAuthority::install(
        client_arc,
        "comm-design",
        att,
        server.local_peer_id.clone(),
        server.local_public_key_b64.clone(),
        chrono::Utc::now(),
    );
    assert!(matches!(res, Err(ClientError::AttestationInvalid(_))));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rate_limit_retries_then_succeeds() {
    let mut behavior = FixtureBehavior::happy();
    behavior.rate_limit_initial = Some(2);
    let server = FixtureServer::spawn(behavior).await;
    let client = KChatBackendClient::new_for_tests(&server.base_url).expect("client");
    // The login itself is the first request that hits the 429
    // path; the bounded retry budget covers the two refusals.
    let identity = client
        .login(&login_body(&server))
        .await
        .expect("login should succeed after retries");
    assert_eq!(identity.display_name, "Alice");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rate_limit_exhausted_returns_rate_limited() {
    let mut behavior = FixtureBehavior::happy();
    // Exceeds MAX_RATE_LIMIT_RETRIES (3).
    behavior.rate_limit_initial = Some(10);
    let server = FixtureServer::spawn(behavior).await;
    let client = KChatBackendClient::new_for_tests(&server.base_url).expect("client");
    let res = client.login(&login_body(&server)).await;
    assert!(matches!(res, Err(ClientError::RateLimited)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_client_refuses_http_base_url() {
    let res = KChatBackendClient::new("http://kchat.example.com");
    assert!(matches!(res, Err(ClientError::InsecureTransport { .. })));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refresh_path_renews_access_token_on_short_lifetime() {
    let mut behavior = FixtureBehavior::happy();
    // 1-second access token TTL: well below the client's 30s
    // pre-emptive refresh window, so the next authed call must
    // trigger a refresh before sending. We also wait > 1s so
    // the fixture would reject the OLD token if the client
    // somehow skipped the refresh — making the test prove the
    // refresh actually fired.
    behavior.access_token_lifetime_secs = 1;
    let server = FixtureServer::spawn(behavior).await;
    let client = KChatBackendClient::new_for_tests(&server.base_url).expect("client");
    client.login(&login_body(&server)).await.expect("login");
    tokio::time::sleep(Duration::from_millis(1_100)).await;
    let communities = client.list_communities().await.expect("list");
    assert_eq!(communities.len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refresh_fails_when_backend_invalidates_refresh_token() {
    let mut behavior = FixtureBehavior::happy();
    behavior.access_token_lifetime_secs = 1;
    behavior.refresh_always_fails = true;
    let server = FixtureServer::spawn(behavior).await;
    let client = KChatBackendClient::new_for_tests(&server.base_url).expect("client");
    client.login(&login_body(&server)).await.expect("login");
    // > 1s so the fixture rejects the cached token, forcing the
    // client to attempt a refresh which the fixture is
    // configured to fail — surfacing `RefreshExpired`.
    tokio::time::sleep(Duration::from_millis(1_100)).await;
    let res = client.list_communities().await;
    assert!(matches!(res, Err(ClientError::RefreshExpired { .. })));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auto_refresh_authority_refreshes_when_inside_window() {
    let mut behavior = FixtureBehavior::happy();
    // Attestation TTL is short so the authority's
    // refresh_if_needed check (with a 1s window) fires.
    behavior.attestation_ttl_secs = 2;
    let server = FixtureServer::spawn(behavior).await;
    let client = KChatBackendClient::new_for_tests(&server.base_url).expect("client");
    client.login(&login_body(&server)).await.expect("login");
    let att = client
        .get_membership_attestation("comm-design", &server.local_public_key_b64)
        .await
        .expect("attestation");
    let client_arc = Arc::new(client);
    let authority = KChatBackendAuthority::install(
        client_arc,
        "comm-design",
        att,
        server.local_peer_id.clone(),
        server.local_public_key_b64.clone(),
        chrono::Utc::now(),
    )
    .expect("install")
    .with_refresh_window(Duration::from_secs(5));
    let refreshed = authority.refresh_if_needed().await.expect("refresh");
    assert!(refreshed, "expected refresh to fire");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn poll_events_returns_initial_member_joined() {
    let server = FixtureServer::spawn(FixtureBehavior::happy()).await;
    let client = KChatBackendClient::new_for_tests(&server.base_url).expect("client");
    client.login(&login_body(&server)).await.expect("login");
    let resp = client.poll_events("comm-design", None).await.expect("poll");
    assert!(!resp.events.is_empty());
    assert_eq!(resp.next_cursor, "cursor-1");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn poll_events_with_cursor_returns_empty() {
    let server = FixtureServer::spawn(FixtureBehavior::happy()).await;
    let client = KChatBackendClient::new_for_tests(&server.base_url).expect("client");
    client.login(&login_body(&server)).await.expect("login");
    let resp = client
        .poll_events("comm-design", Some("cursor-1"))
        .await
        .expect("poll");
    assert!(resp.events.is_empty());
}
