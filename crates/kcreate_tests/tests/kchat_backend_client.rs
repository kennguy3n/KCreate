//! Phase 7 Block A/B — KChat backend REST client integration test.
//!
//! Drives the real `kcreate_kchat_client::KChatBackendClient`
//! against the in-process axum-backed `FixtureServer` to exercise
//! the end-to-end sign-in flow:
//!
//!   1. The client signs in to the backend over HTTP (the test
//!      fixture is plain-HTTP on `127.0.0.1`; the production
//!      client refuses anything but `https://`, which is unit
//!      tested in [`kcreate_kchat_client::rest`]).
//!   2. The full session flow (`list_communities` →
//!      `get_membership_attestation` → install authority → verify
//!      membership) works end-to-end against the live REST
//!      fixture.
//!   3. Auto-refresh fires when the membership is within the
//!      5-minute refresh window.
//!   4. The local-first invariant is preserved (the deny-list
//!      test in `local_first.rs` keeps `kcreate_kchat_client`
//!      out of the editing-path closure — that crate is only
//!      compiled into the bridge behind the off-by-default
//!      `kchat-backend` feature flag).

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use kcreate_collab::kchat::KChatGroupAuthority;
use kcreate_kchat_client::fixture::{FixtureBehavior, FixtureServer};
use kcreate_kchat_client::{
    KChatBackendAuthority, KChatBackendClient, KChatRole, LoginRequest,
};

fn login_body(server: &FixtureServer) -> LoginRequest {
    LoginRequest {
        login_id: server.login_id.clone(),
        password: server.password.clone(),
        totp: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn end_to_end_signin_select_install_and_verify() {
    let server = FixtureServer::spawn(FixtureBehavior::happy()).await;
    let client = KChatBackendClient::new_for_tests(&server.base_url).expect("client");

    // Sign in.
    let identity = client.login(&login_body(&server)).await.expect("login");
    assert_eq!(identity.peer_id, server.local_peer_id.as_str());

    // List communities.
    let communities = client.list_communities().await.expect("list communities");
    let community = communities
        .iter()
        .find(|c| c.id == "comm-design")
        .expect("design community");
    assert_eq!(community.role, KChatRole::Owner);

    // Fetch attestation for the local peer in this community.
    let attestation = client
        .get_membership_attestation(&community.id, &identity.public_key)
        .await
        .expect("attestation");
    assert_eq!(attestation.group_id, community.id);
    assert_eq!(attestation.peer_public_key, server.local_public_key_b64);
    assert_eq!(attestation.peer_id, server.local_peer_id.as_str());

    // Install the authority. This verifies the signature against
    // the issuer trust root and binds it to the local peer.
    let client_arc = Arc::new(client);
    let authority = KChatBackendAuthority::install(
        client_arc.clone(),
        &community.id,
        attestation.clone(),
        server.local_peer_id.clone(),
        server.local_public_key_b64.clone(),
        Utc::now(),
    )
    .expect("install authority");

    // Cached membership round-trips through the trait.
    let local = authority.local_membership().expect("local membership");
    assert_eq!(local.group_id.as_str(), community.id);
    assert_eq!(local.peer_id.as_str(), server.local_peer_id.as_str());

    // verify_remote accepts another signed membership minted by
    // the same issuer. The fixture only ships with the local
    // identity, so we re-verify against the local peer id/key
    // bound to the attestation we just installed.
    let remote_attestation = client_arc
        .get_membership_attestation(&community.id, &server.local_public_key_b64)
        .await
        .expect("attestation 2");
    let remote_membership =
        kcreate_kchat_client::membership_from_attestation(remote_attestation)
            .expect("decode membership");
    authority
        .verify_remote(
            &server.local_peer_id,
            &server.local_public_key_b64,
            &remote_membership,
            Utc::now(),
        )
        .expect("verify remote");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auto_refresh_fires_when_within_window() {
    let mut behavior = FixtureBehavior::happy();
    behavior.attestation_ttl_secs = 2;
    let server = FixtureServer::spawn(behavior).await;
    let client = KChatBackendClient::new_for_tests(&server.base_url).expect("client");
    let identity = client.login(&login_body(&server)).await.expect("login");
    let attestation = client
        .get_membership_attestation("comm-design", &identity.public_key)
        .await
        .expect("attestation");

    let client_arc = Arc::new(client);
    let authority = KChatBackendAuthority::install(
        client_arc,
        "comm-design",
        attestation,
        server.local_peer_id.clone(),
        server.local_public_key_b64.clone(),
        Utc::now(),
    )
    .expect("install")
    .with_refresh_window(Duration::from_secs(10));

    let refreshed = authority.refresh_if_needed().await.expect("refresh");
    assert!(refreshed, "expected refresh to fire");
}
