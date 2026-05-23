//! Loopback integration tests for the LAN transport.
//!
//! These tests deliberately disable mDNS (`advertise_mdns: false`)
//! because the CI runners we use can't always issue multicast on the
//! loopback interface. Connectivity is established via the
//! out-of-band [`LanCollabHost::dial_known_peer`] entrypoint, which
//! is also what the "paste peer link" UI flow will exercise in
//! production. The discovery layer is covered by its own unit tests
//! in `src/discovery.rs`.

use std::sync::Arc;
use std::time::Duration;

use kcreate_collab::kchat::InProcessKChatAuthority;
use kcreate_collab::{KChatGroupId, Message, PeerKey, PresencePayload};
use kcreate_collab_transport::{HostOptions, InboundEvent, LanCollabHost};
use tokio::time::timeout;
use uuid::Uuid;

const RECV_TIMEOUT: Duration = Duration::from_secs(5);

/// Shared in-process KChat issuer seed for the loopback test suite.
/// All hosts started by `start_host` share this issuer + group so
/// every Hello/Welcome attestation cross-verifies successfully.
const TEST_ISSUER_SEED: [u8; 32] = [0xCA; 32];
const TEST_GROUP_ID: &str = "loopback-group";

fn fresh_key(seed_byte: u8) -> PeerKey {
    let mut seed = [0u8; 32];
    seed[0] = seed_byte;
    // Use the byte position so two test peers with seed 1 and 2 are
    // demonstrably distinct without depending on RNG.
    PeerKey::from_seed(seed)
}

async fn start_host(seed_byte: u8, display_name: &str, project_id: Uuid) -> LanCollabHost {
    let key = fresh_key(seed_byte);
    let identity = key.identity(display_name.to_string());
    let issued = chrono::Utc::now() - chrono::Duration::minutes(1);
    let expires = chrono::Utc::now() + chrono::Duration::hours(1);
    let auth = Arc::new(
        InProcessKChatAuthority::for_peer(
            TEST_ISSUER_SEED,
            KChatGroupId::new(TEST_GROUP_ID).unwrap(),
            identity.peer_id,
            identity.public_key,
            issued,
            expires,
        )
        .expect("issue in-process KChat membership"),
    );
    let mut opts = HostOptions::loopback(key, display_name.to_string(), project_id);
    opts.kchat_authority = auth;
    LanCollabHost::start(opts)
        .await
        .expect("transport host should start on loopback")
}

#[tokio::test]
async fn two_peers_handshake_and_appear_in_each_others_rosters() {
    let project_id = Uuid::new_v4();
    let alice = start_host(1, "Alice", project_id).await;
    let bob = start_host(2, "Bob", project_id).await;

    let alice_events = alice.subscribe();
    let bob_events = bob.subscribe();

    // Bob dials Alice. We use the out-of-band API because mDNS is
    // disabled in the loopback test config.
    let alice_identity = alice.local_identity();
    let alice_addr = alice.local_addr();
    let alice_fp = alice.cert_fingerprint_b64();
    let alice_fp_bytes = decode_b64(&alice_fp);

    bob.dial_known_peer(alice_identity.clone(), alice_addr, alice_fp_bytes)
        .await
        .expect("dial should succeed");

    // Both sides should see a PeerJoined event.
    expect_peer_joined(alice_events, &bob.local_identity().peer_id).await;
    expect_peer_joined(bob_events, &alice.local_identity().peer_id).await;

    // Rosters should now contain each other.
    let bob_peer_id = bob.local_identity().peer_id;
    let alice_peer_id = alice.local_identity().peer_id;
    assert!(
        alice
            .connected_peer_ids()
            .into_iter()
            .any(|p| p == bob_peer_id),
        "alice should see bob"
    );
    assert!(
        bob.connected_peer_ids()
            .into_iter()
            .any(|p| p == alice_peer_id),
        "bob should see alice"
    );

    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn broadcast_round_trip_presence_message() {
    let project_id = Uuid::new_v4();
    let alice = start_host(11, "Alice", project_id).await;
    let bob = start_host(22, "Bob", project_id).await;

    let mut bob_events = bob.subscribe();
    let alice_fp_bytes = decode_b64(&alice.cert_fingerprint_b64());

    bob.dial_known_peer(alice.local_identity(), alice.local_addr(), alice_fp_bytes)
        .await
        .expect("dial should succeed");

    // Drain bob_events of join events first.
    drain_until_joined(&mut bob_events, &alice.local_identity().peer_id).await;

    let payload = PresencePayload {
        active_page: None,
        selection: vec![],
        cursor: None,
        sent_at: chrono::Utc::now(),
    };
    alice
        .broadcast(Message::Presence(payload.clone()))
        .await
        .expect("broadcast should succeed");

    // Bob should receive it. We only need to verify it round-trips
    // as a `Presence` (the sender is recoverable via the envelope's
    // `from` field, which the host promotes into `InboundEvent::Message`).
    let received = wait_for_message(&mut bob_events).await;
    match received {
        Message::Presence(p) => assert_eq!(p.selection, payload.selection),
        other => panic!("expected Presence, got {other:?}"),
    }

    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn dial_rejected_for_mismatched_project_id() {
    let alice = start_host(31, "Alice", Uuid::new_v4()).await;
    let bob = start_host(32, "Bob", Uuid::new_v4()).await;

    let alice_fp_bytes = decode_b64(&alice.cert_fingerprint_b64());
    let result = bob
        .dial_known_peer(alice.local_identity(), alice.local_addr(), alice_fp_bytes)
        .await;
    assert!(result.is_err(), "cross-project dial must fail");

    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn dial_rejected_for_wrong_cert_fingerprint() {
    let project_id = Uuid::new_v4();
    let alice = start_host(41, "Alice", project_id).await;
    let bob = start_host(42, "Bob", project_id).await;

    // Use a wrong fingerprint — should be rejected at TLS time.
    let wrong_fp = [0u8; 32];
    let result = bob
        .dial_known_peer(alice.local_identity(), alice.local_addr(), wrong_fp)
        .await;
    assert!(
        result.is_err(),
        "dial with wrong cert fingerprint must be rejected"
    );

    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn shutdown_disconnects_peer_cleanly() {
    let project_id = Uuid::new_v4();
    let alice = start_host(51, "Alice", project_id).await;
    let bob = start_host(52, "Bob", project_id).await;

    let alice_events = alice.subscribe();
    let alice_fp_bytes = decode_b64(&alice.cert_fingerprint_b64());

    bob.dial_known_peer(alice.local_identity(), alice.local_addr(), alice_fp_bytes)
        .await
        .expect("dial should succeed");

    expect_peer_joined(alice_events, &bob.local_identity().peer_id).await;
    bob.shutdown().await;

    // Alice should eventually drop bob from her roster as the QUIC
    // connection closes. We allow a short grace period because
    // PeerLeft is fired off the reader-loop task.
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while std::time::Instant::now() < deadline {
        let count = alice.connected_peer_ids().len();
        if count == 0 {
            alice.shutdown().await;
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("alice never observed bob's disconnect");
}

fn decode_b64(s: &str) -> [u8; 32] {
    // The transport's `cert_fingerprint_b64` uses
    // `STANDARD_NO_PAD` encoding (matches what's advertised over
    // mDNS), so the dialer-side decode must match.
    use base64::engine::general_purpose::STANDARD_NO_PAD;
    use base64::Engine;
    let bytes = STANDARD_NO_PAD
        .decode(s.as_bytes())
        .expect("cert fingerprint base64");
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    out
}

async fn expect_peer_joined(
    mut rx: tokio::sync::broadcast::Receiver<InboundEvent>,
    expected: &kcreate_collab::PeerId,
) {
    let deadline = tokio::time::sleep(RECV_TIMEOUT);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            () = &mut deadline => panic!("timed out waiting for PeerJoined({})", expected.as_str()),
            ev = rx.recv() => match ev {
                Ok(InboundEvent::PeerJoined(id)) if id.peer_id == *expected => return,
                Ok(_) => {}
                Err(e) => panic!("recv error: {e}"),
            }
        }
    }
}

async fn drain_until_joined(
    rx: &mut tokio::sync::broadcast::Receiver<InboundEvent>,
    expected: &kcreate_collab::PeerId,
) {
    let deadline = tokio::time::sleep(RECV_TIMEOUT);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            () = &mut deadline => panic!("timed out draining until PeerJoined({})", expected.as_str()),
            ev = rx.recv() => match ev {
                Ok(InboundEvent::PeerJoined(id)) if id.peer_id == *expected => return,
                Ok(_) => {}
                Err(e) => panic!("recv error: {e}"),
            }
        }
    }
}

async fn wait_for_message(rx: &mut tokio::sync::broadcast::Receiver<InboundEvent>) -> Message {
    let fut = async {
        loop {
            match rx.recv().await {
                Ok(InboundEvent::Message { message, .. }) => return *message,
                Ok(_) => {}
                Err(e) => panic!("recv error: {e}"),
            }
        }
    };
    timeout(RECV_TIMEOUT, fut)
        .await
        .expect("timed out waiting for inbound Message")
}
