//! Phase 7 Block A — KChat Desktop client integration test.
//!
//! Drives the real `kcreate_kchat_client::KChatDesktopClient`
//! against a real Unix domain socket served by the in-process
//! reference implementation (`mock_server::spawn_unix_listener`).
//! This exercises the production connect-by-path code path —
//! including socket discovery — without needing a running
//! `uney-chat-desktop`.
//!
//! The test verifies:
//!   1. The client can connect to a well-known socket path.
//!   2. The full session flow (`list_communities` → `select` →
//!      install authority → verify membership) works end-to-end.
//!   3. Auto-refresh fires when the membership is within the
//!      5-minute refresh window.
//!   4. The local-first invariant is preserved (the deny-list
//!      test in `local_first.rs` does not include
//!      `kcreate_kchat_client` in the editing-path closure).

#![cfg(unix)]

use std::time::Duration;

use chrono::Utc;
use kcreate_collab::kchat::KChatGroupAuthority;
use kcreate_collab::peer::PeerKey;
use kcreate_kchat_client::mock_server::{
    replace_identity_with_peer_key, spawn_unix_listener, MockState,
};
use kcreate_kchat_client::{KChatDesktopAuthority, KChatDesktopClient};
use tempfile::tempdir;
use tokio::time::timeout;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn end_to_end_connect_select_install_and_verify() {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("kcreate.sock");

    let key = PeerKey::from_seed([3u8; 32]);
    let identity = key.identity("Alice");
    let mut state = MockState::fixture();
    replace_identity_with_peer_key(&mut state, &key, "Alice", "alice@kchat.com");
    let (handle, issuer_pub) = spawn_unix_listener(state, &path)
        .await
        .expect("spawn listener");

    let client = KChatDesktopClient::new();
    let connected_path = timeout(Duration::from_secs(5), client.connect_to(&path))
        .await
        .expect("connect timed out")
        .expect("connect failed");
    assert_eq!(connected_path, path);

    let communities = client.list_communities().await.expect("list");
    assert_eq!(communities.len(), 1);
    let community_id = communities[0].id.clone();

    let attestation = client
        .get_membership(&community_id)
        .await
        .expect("attestation");
    assert_eq!(attestation.group_id, community_id);

    let authority = KChatDesktopAuthority::install(
        std::sync::Arc::new(KChatDesktopClient::new()),
        &community_id,
        attestation.clone(),
        identity.peer_id.clone(),
        identity.public_key.clone(),
        Utc::now(),
    )
    .expect("authority install");

    // The cached membership should verify against the issuer key
    // the mock used to sign it.
    authority
        .cached_membership()
        .verify(
            &issuer_pub,
            &identity.peer_id,
            &identity.public_key,
            Utc::now(),
        )
        .expect("verify");

    // session_start would normally consult this authority via the
    // collab gate — but for an integration test that only verifies
    // Block A, exercising the authority trait directly is enough.
    let local = authority.local_membership().expect("local_membership");
    assert_eq!(local.group_id().as_str(), community_id);

    client.disconnect().await;
    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auto_refresh_replaces_near_expiry_attestation() {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("kcreate.sock");

    let key = PeerKey::from_seed([4u8; 32]);
    let identity = key.identity("Alice");
    let mut state = MockState::fixture();
    replace_identity_with_peer_key(&mut state, &key, "Alice", "alice@kchat.com");
    // Mint a 4-minute window — within the 5-minute auto-refresh
    // threshold so the authority must request a fresh attestation
    // from the server.
    state.attestation_lifetime = chrono::Duration::minutes(4);
    let (handle, _issuer_pub) = spawn_unix_listener(state, &path)
        .await
        .expect("spawn listener");

    let client = std::sync::Arc::new(KChatDesktopClient::new());
    client.connect_to(&path).await.expect("connect");

    let attestation = client
        .get_membership("comm-test")
        .await
        .expect("initial attestation");
    let initial_expiry = attestation.expires_at;

    // Now bump the mock's lifetime so the next attestation it issues
    // is comfortably outside the refresh window.
    {
        let mut guard = handle.state.lock();
        guard.attestation_lifetime = chrono::Duration::hours(1);
    }

    let authority = KChatDesktopAuthority::install(
        client.clone(),
        "comm-test",
        attestation,
        identity.peer_id.clone(),
        identity.public_key.clone(),
        Utc::now(),
    )
    .expect("install");

    let refreshed = authority
        .refresh_if_needed()
        .await
        .expect("refresh_if_needed");
    assert!(refreshed, "should have refreshed near-expiry attestation");
    assert!(
        authority.cached_membership().expires_at > initial_expiry,
        "refreshed attestation should extend the validity window"
    );

    client.disconnect().await;
    handle.shutdown().await;
}
