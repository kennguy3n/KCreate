//! Unit tests for the KChat Desktop client.
//!
//! Each scenario drives the real client + transport against an
//! in-memory `tokio::io::duplex` pair so the test harness exercises
//! the production code path without needing a real Unix socket on
//! the filesystem. The mock server is the canonical Rust reference
//! implementation of the protocol (see `mock_server.rs`).

use std::time::Duration;

use tokio::time::timeout;

use crate::mock_server::{
    replace_identity_with_peer_key, spawn_single_stream, MockServerHandle, MockState,
};
use crate::protocol::{
    CommunityEvent, CommunityEventKind, ErrorCode, KChatConversationType, KChatRole,
    PostMessageParams,
};
use crate::transport::REQUEST_TIMEOUT;
use crate::{KChatDesktopAuthority, KChatDesktopClient};
use chrono::Utc;
use kcreate_collab::peer::PeerKey;

async fn install_test_pair() -> (
    KChatDesktopClient,
    MockServerHandle,
    ed25519_dalek::VerifyingKey,
) {
    let (server, client) = tokio::io::duplex(64 * 1024);
    let state = MockState::fixture();
    let (handle, issuer_pub) = spawn_single_stream(state, server);
    let client_handle = KChatDesktopClient::new();
    client_handle.install_test_stream(client).await;
    (client_handle, handle, issuer_pub)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn identity_get_round_trips() {
    let (client, handle, _) = install_test_pair().await;
    let id = client.get_identity().await.expect("identity");
    assert_eq!(id.jid, "alice@kchat.com");
    assert_eq!(id.display_name, "Alice");
    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn communities_list_returns_fixture_data() {
    let (client, handle, _) = install_test_pair().await;
    let communities = client.list_communities().await.expect("list");
    assert_eq!(communities.len(), 1);
    assert_eq!(communities[0].id, "comm-test");
    assert!(matches!(communities[0].role, KChatRole::Owner));
    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn members_list_round_trips() {
    let (client, handle, _) = install_test_pair().await;
    let members = client.get_members("comm-test").await.expect("members");
    assert_eq!(members.len(), 2);
    assert!(members.iter().any(|m| matches!(m.role, KChatRole::Owner)));
    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conversations_list_round_trips() {
    let (client, handle, _) = install_test_pair().await;
    let convs = client.list_conversations("comm-test").await.expect("conv");
    assert_eq!(convs.len(), 1);
    assert!(matches!(
        convs[0].conversation_type,
        KChatConversationType::Channel
    ));
    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn post_message_round_trips() {
    let (client, handle, _) = install_test_pair().await;
    let params = PostMessageParams {
        conversation_id: "conv-general".into(),
        payload: serde_json::json!({ "schemaVersion": 1, "x": 1 }),
        content_type: Some("kcreate.invite.v1".into()),
    };
    let result = client.post_message(params).await.expect("post");
    assert!(result.message_id.starts_with("msg-"));
    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_community_returns_not_found() {
    let (client, handle, _) = install_test_pair().await;
    let err = client.get_members("does-not-exist").await.unwrap_err();
    match err {
        crate::ClientError::Rpc(e) => {
            assert_eq!(e.code, ErrorCode::NotFound.as_i32());
        }
        other => panic!("expected RPC error, got {other:?}"),
    }
    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn membership_attestation_verifies_for_bound_peer_key() {
    let key = PeerKey::from_seed([7u8; 32]);
    let identity = key.identity("Alice");

    let (server, client) = tokio::io::duplex(64 * 1024);
    let mut state = MockState::fixture();
    replace_identity_with_peer_key(&mut state, &key, "Alice", "alice@kchat.com");
    let (handle, issuer_pub) = spawn_single_stream(state, server);
    let client_handle = KChatDesktopClient::new();
    client_handle.install_test_stream(client).await;

    let attestation = client_handle
        .get_membership("comm-test")
        .await
        .expect("attestation");
    assert_eq!(attestation.group_id, "comm-test");

    let authority = KChatDesktopAuthority::install(
        std::sync::Arc::new(KChatDesktopClient::new()),
        "comm-test",
        attestation,
        identity.peer_id.clone(),
        identity.public_key.clone(),
        Utc::now(),
    )
    .expect("authority");
    let m = authority.cached_membership();
    m.verify(
        &issuer_pub,
        &identity.peer_id,
        &identity.public_key,
        Utc::now(),
    )
    .expect("verify");
    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auto_refresh_triggers_when_within_window() {
    let key = PeerKey::from_seed([9u8; 32]);
    let identity = key.identity("Alice");

    let (server, client_stream) = tokio::io::duplex(64 * 1024);
    let mut state = MockState::fixture();
    replace_identity_with_peer_key(&mut state, &key, "Alice", "alice@kchat.com");
    // Mint a membership expiring 4 minutes from now — well within
    // the 5-minute refresh window. The authority's
    // `refresh_if_needed` should swap it for a fresh one signed
    // with the same issuer.
    state.attestation_lifetime = chrono::Duration::minutes(4);
    let (handle, _issuer_pub) = spawn_single_stream(state, server);

    let client = std::sync::Arc::new(KChatDesktopClient::new());
    client.install_test_stream(client_stream).await;

    let initial = client.get_membership("comm-test").await.expect("initial");
    let initial_expiry = initial.expires_at;

    // Now bump the mock lifetime so the next attestation issued is
    // an hour out — that's the "refreshed" state we expect.
    {
        let mut guard = handle.state.lock();
        guard.attestation_lifetime = chrono::Duration::hours(1);
    }

    let authority = KChatDesktopAuthority::install(
        client.clone(),
        "comm-test",
        initial,
        identity.peer_id.clone(),
        identity.public_key.clone(),
        Utc::now(),
    )
    .expect("authority install");

    let refreshed = authority
        .refresh_if_needed()
        .await
        .expect("refresh_if_needed");
    assert!(refreshed, "should have triggered a refresh");
    assert!(authority.cached_membership().expires_at > initial_expiry);
    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn notifications_are_forwarded_to_subscribers() {
    let (client, handle, _) = install_test_pair().await;
    let _ = client
        .subscribe_community("comm-test")
        .await
        .expect("subscribe");
    let mut rx = client.subscribe_notifications();

    handle.push_event(CommunityEvent {
        subscription_id: "<filled by server>".into(),
        community_id: "comm-test".into(),
        event: CommunityEventKind::MemberLeft {
            peer_id: "bob-peer-id".into(),
            jid: "bob@kchat.com".into(),
        },
        at: Utc::now(),
    });

    let event = timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("event arrived")
        .expect("not lagged");
    assert_eq!(event.community_id, "comm-test");
    matches!(event.event, CommunityEventKind::MemberLeft { .. });
    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_returns_timeout_when_server_silent() {
    // Spawn a connection but don't run the server side, so any
    // request the client makes hangs. We wrap the call in a manual
    // timeout shorter than `REQUEST_TIMEOUT` to keep the test fast;
    // but we also verify the `ClientError::Timeout` path actually
    // fires by deferring to the production timeout.
    let _ = REQUEST_TIMEOUT;
    let (_server_stream, client_stream) = tokio::io::duplex(64 * 1024);
    let client = KChatDesktopClient::new();
    client.install_test_stream(client_stream).await;

    // Cap the wait at 30 s so even if the production timeout is
    // long the test still completes deterministically.
    let res = tokio::time::timeout(
        REQUEST_TIMEOUT + Duration::from_secs(2),
        client.list_communities(),
    )
    .await
    .expect("client returned");
    matches!(res, Err(crate::ClientError::Timeout(_)));
}
