// Phase 7 Block D — security & privacy hardening integration tests.
//
// Scope: Tasks 19–24. Covers the cross-crate security invariants
// in one place so a regression in `kcreate_collab`'s ACL,
// clipboard, rate-limit, or key-rotation surfaces shows up here
// even when each component's unit tests still pass.
//
// Unlike the unit tests in `kcreate_collab::{acl,clipboard,session}`,
// these tests pull the public API as it appears to downstream
// callers (the bridge layer) — same imports, same paths the
// renderer-facing code path uses. That makes it impossible to ship
// a "tests-only" workaround that hides a real regression behind a
// crate-private helper.

use ed25519_dalek::SigningKey;
use kcreate_collab::acl::{AclDecision, AclEntry, AclMode, AclPermission, ProjectAcl};
use kcreate_collab::clipboard::{
    decrypt_clipboard_payload, derive_x25519_from_ed25519_public, encrypt_clipboard_payload,
    ClipboardCryptoError,
};
use kcreate_collab::peer::PeerKey;
use kcreate_collab::session::{ProjectSession, RateBudgetDecision, RateLimitKind, SessionConfig};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a `ProjectSession` for use as a security-policy harness.
///
/// The KChat group authority defaults to `NoKChatGroupAuthority`
/// (i.e. "multiplayer locked") which is fine for the surfaces we
/// exercise here — none of these tests seal/open envelopes or
/// call the Hello/Welcome path that actually consults the
/// authority. Skipping the test-support `InProcessKChatAuthority`
/// also means we don't depend on a feature flag the production
/// build doesn't enable.
fn make_session(seed: u8) -> (ProjectSession, Uuid) {
    let project = Uuid::new_v4();
    let session = ProjectSession::new(
        PeerKey::from_seed([seed; 32]),
        format!("peer-{seed}"),
        project,
        SessionConfig::default(),
        [seed; 8],
    );
    (session, project)
}

// ---------------------------------------------------------------------------
// Task 21 — ACL enforcement
// ---------------------------------------------------------------------------

#[test]
fn acl_open_mode_admits_unknown_peer() {
    // Open mode treats the ACL as advisory — the community gate
    // (not exercised here) is the real authorisation surface.
    let acl = ProjectAcl::default();
    let stranger = PeerKey::from_seed([55; 32]).identity("stranger");
    assert_eq!(
        acl.evaluate(&stranger),
        AclDecision::Allow(AclPermission::Editor)
    );
}

#[test]
fn acl_enforce_mode_rejects_peer_not_on_allowlist() {
    let allowed = PeerKey::from_seed([1; 32]).identity("alice");
    let stranger = PeerKey::from_seed([2; 32]).identity("stranger");
    let acl = ProjectAcl {
        mode: AclMode::Enforce,
        entries: vec![AclEntry {
            public_key: allowed.public_key.clone(),
            display_name: "Alice".into(),
            permission: AclPermission::Editor,
        }],
    };
    assert_eq!(
        acl.evaluate(&allowed),
        AclDecision::Allow(AclPermission::Editor)
    );
    assert_eq!(acl.evaluate(&stranger), AclDecision::Deny);
}

#[test]
fn acl_enforce_mode_preserves_per_peer_permission() {
    // Mixed editor/viewer roster — make sure the per-entry
    // permission flows through `evaluate` instead of always
    // defaulting to Editor.
    let alice = PeerKey::from_seed([3; 32]).identity("alice");
    let bob = PeerKey::from_seed([4; 32]).identity("bob");
    let acl = ProjectAcl {
        mode: AclMode::Enforce,
        entries: vec![
            AclEntry {
                public_key: alice.public_key.clone(),
                display_name: "Alice".into(),
                permission: AclPermission::Editor,
            },
            AclEntry {
                public_key: bob.public_key.clone(),
                display_name: "Bob".into(),
                permission: AclPermission::Viewer,
            },
        ],
    };
    assert_eq!(
        acl.evaluate(&alice),
        AclDecision::Allow(AclPermission::Editor)
    );
    assert_eq!(
        acl.evaluate(&bob),
        AclDecision::Allow(AclPermission::Viewer)
    );
}

#[test]
fn acl_round_trips_through_json_serialisation() {
    // The bridge persists the ACL to `<project>/acl.json` — verify
    // the on-disk representation round-trips losslessly so a
    // session restarted with the same project sees identical
    // policy.
    let alice = PeerKey::from_seed([5; 32]).identity("alice");
    let original = ProjectAcl {
        mode: AclMode::Enforce,
        entries: vec![AclEntry {
            public_key: alice.public_key,
            display_name: "Alice".into(),
            permission: AclPermission::Editor,
        }],
    };
    let json = serde_json::to_string(&original).unwrap();
    let back: ProjectAcl = serde_json::from_str(&json).unwrap();
    assert_eq!(original, back);
}

// ---------------------------------------------------------------------------
// Task 22 — Rate limiting
// ---------------------------------------------------------------------------

#[test]
fn rate_limit_admits_traffic_under_budget() {
    let (mut session, _project) = make_session(10);
    let peer = PeerKey::from_seed([11; 32]).identity("bob");
    let peer_id = peer.peer_id.clone();
    session.trust_peer(peer).unwrap();
    let now = std::time::Instant::now();
    for i in 0..50 {
        let dec = session.record_rate_event(
            &peer_id,
            RateLimitKind::Operation,
            now + std::time::Duration::from_millis(i),
        );
        assert_eq!(
            dec,
            RateBudgetDecision::Ok,
            "event #{i} unexpectedly over budget"
        );
    }
}

#[test]
fn rate_limit_warns_then_escalates_to_kick_threshold() {
    // Default `rate_limit_disconnect_after` is 3 — three
    // consecutive overflow windows is the kick threshold. We use
    // a cap of 2 here so we can force overflow without sending
    // 100 events per window.
    let config = SessionConfig {
        max_ops_per_second: 2,
        ..SessionConfig::default()
    };
    let project = Uuid::new_v4();
    let mut session = ProjectSession::new(
        PeerKey::from_seed([20; 32]),
        "host",
        project,
        config.clone(),
        [20; 8],
    );
    let peer = PeerKey::from_seed([21; 32]).identity("flooder");
    let peer_id = peer.peer_id.clone();
    session.trust_peer(peer).unwrap();

    let t0 = std::time::Instant::now();
    for second in 0..config.rate_limit_disconnect_after {
        let t = t0 + std::time::Duration::from_secs(u64::from(second));
        // Eat the budget in this window.
        for _ in 0..config.max_ops_per_second {
            assert_eq!(
                session.record_rate_event(&peer_id, RateLimitKind::Operation, t),
                RateBudgetDecision::Ok
            );
        }
        // Third event overflows. The streak should grow by 1 for
        // every consecutive overflow window.
        let dec = session.record_rate_event(&peer_id, RateLimitKind::Operation, t);
        assert_eq!(
            dec,
            RateBudgetDecision::OverBudget {
                consecutive_overflow_windows: second + 1
            }
        );
    }
}

#[test]
fn rate_limit_independent_budgets_for_operations_and_presence() {
    // Burning the operations budget must not affect the presence
    // budget for the same peer. Otherwise a misbehaving editor
    // could starve other peers' cursor updates.
    let config = SessionConfig {
        max_ops_per_second: 1,
        max_presence_per_second: 5,
        ..SessionConfig::default()
    };
    let project = Uuid::new_v4();
    let mut session = ProjectSession::new(
        PeerKey::from_seed([30; 32]),
        "host",
        project,
        config,
        [30; 8],
    );
    let peer = PeerKey::from_seed([31; 32]).identity("bob");
    let peer_id = peer.peer_id.clone();
    session.trust_peer(peer).unwrap();

    let t = std::time::Instant::now();
    assert_eq!(
        session.record_rate_event(&peer_id, RateLimitKind::Operation, t),
        RateBudgetDecision::Ok
    );
    assert!(matches!(
        session.record_rate_event(&peer_id, RateLimitKind::Operation, t),
        RateBudgetDecision::OverBudget { .. }
    ));
    // Presence budget is still fresh.
    for _ in 0..5 {
        assert_eq!(
            session.record_rate_event(&peer_id, RateLimitKind::Presence, t),
            RateBudgetDecision::Ok
        );
    }
}

// ---------------------------------------------------------------------------
// Task 19 — Key rotation acknowledgement
// ---------------------------------------------------------------------------

#[test]
fn key_rotation_tracks_per_peer_acked_epoch() {
    let (mut session, _project) = make_session(40);
    let peer = PeerKey::from_seed([41; 32]).identity("bob");
    let peer_id = peer.peer_id.clone();
    session.trust_peer(peer).unwrap();

    // No acks yet — every future epoch is outstanding.
    assert_eq!(session.peers_missing_key_rotation(1), vec![peer_id.clone()]);
    assert!(session.record_key_rotation_ack(&peer_id, 1));
    assert!(session.peers_missing_key_rotation(1).is_empty());

    // Acks are monotonic — replaying an older ack must not
    // regress the recorded epoch.
    assert!(session.record_key_rotation_ack(&peer_id, 0));
    assert!(session.peers_missing_key_rotation(1).is_empty());

    // But the next rotation is still outstanding until acked.
    assert_eq!(session.peers_missing_key_rotation(2), vec![peer_id]);
}

#[test]
fn key_rotation_isolates_per_peer_progress() {
    // Two trusted peers — ack from one peer must not be confused
    // with the other's. The "missing" set should contain only the
    // peer that hasn't acked.
    let (mut session, _project) = make_session(50);
    let bob = PeerKey::from_seed([51; 32]).identity("bob");
    let carol = PeerKey::from_seed([52; 32]).identity("carol");
    let bob_id = bob.peer_id.clone();
    let carol_id = carol.peer_id.clone();
    session.trust_peer(bob).unwrap();
    session.trust_peer(carol).unwrap();

    assert!(session.record_key_rotation_ack(&bob_id, 7));
    let missing = session.peers_missing_key_rotation(7);
    assert_eq!(missing.len(), 1);
    assert_eq!(missing[0], carol_id);
}

// ---------------------------------------------------------------------------
// Task 23 — Encrypted clipboard sharing
// ---------------------------------------------------------------------------

#[test]
fn clipboard_round_trip_preserves_plaintext() {
    let alice = SigningKey::from_bytes(&[60; 32]);
    let bob = SigningKey::from_bytes(&[61; 32]);
    let bob_pub = bob.verifying_key();
    let alice_pub = alice.verifying_key();

    let plaintext = b"3 nodes copied from page 2";
    let nonce = [0x77; 12];
    let ciphertext = encrypt_clipboard_payload(&alice, &bob_pub, plaintext, nonce).unwrap();
    assert_ne!(ciphertext.as_slice(), plaintext);
    let pt = decrypt_clipboard_payload(&bob, &alice_pub, &ciphertext, &nonce).unwrap();
    assert_eq!(pt.bytes, plaintext);
}

#[test]
fn clipboard_non_recipient_cannot_decrypt() {
    // The whole point of the X25519 derivation is that an
    // eavesdropper holding a third Ed25519 key derived from a
    // different seed cannot recover the AEAD key. Verify by
    // having `eve` attempt to decrypt a ciphertext intended for
    // `bob`.
    let alice = SigningKey::from_bytes(&[70; 32]);
    let bob = SigningKey::from_bytes(&[71; 32]);
    let eve = SigningKey::from_bytes(&[72; 32]);
    let bob_pub = bob.verifying_key();
    let alice_pub = alice.verifying_key();

    let nonce = [0x42; 12];
    let ciphertext = encrypt_clipboard_payload(&alice, &bob_pub, b"secret", nonce).unwrap();
    let err = decrypt_clipboard_payload(&eve, &alice_pub, &ciphertext, &nonce).unwrap_err();
    assert!(matches!(err, ClipboardCryptoError::Aead(_)));
}

#[test]
fn clipboard_tampered_ciphertext_fails_aead_check() {
    // Flipping a single bit in the ciphertext should make the
    // ChaCha20-Poly1305 tag fail — this is the AEAD authenticity
    // guarantee the protocol relies on.
    let alice = SigningKey::from_bytes(&[80; 32]);
    let bob = SigningKey::from_bytes(&[81; 32]);
    let nonce = [0x11; 12];
    let mut ciphertext =
        encrypt_clipboard_payload(&alice, &bob.verifying_key(), b"original", nonce).unwrap();
    let last = ciphertext.last_mut().unwrap();
    *last ^= 0x01;
    let err =
        decrypt_clipboard_payload(&bob, &alice.verifying_key(), &ciphertext, &nonce).unwrap_err();
    assert!(matches!(err, ClipboardCryptoError::Aead(_)));
}

#[test]
fn clipboard_bad_nonce_length_rejected() {
    let alice = SigningKey::from_bytes(&[90; 32]);
    let bob = SigningKey::from_bytes(&[91; 32]);
    let ciphertext =
        encrypt_clipboard_payload(&alice, &bob.verifying_key(), b"x", [0; 12]).unwrap();
    // Recipient passes the wrong nonce length — the helper must
    // refuse to even feed it to the cipher.
    let err =
        decrypt_clipboard_payload(&bob, &alice.verifying_key(), &ciphertext, &[0; 11]).unwrap_err();
    assert!(matches!(err, ClipboardCryptoError::BadNonceLength));
}

#[test]
fn clipboard_montgomery_conversion_is_pure_birational_map() {
    // The Edwards → Montgomery point map is the public key half
    // of the X25519 derivation. We verify the conversion is a
    // pure function (same input → same output) and rejects no
    // valid Ed25519 verifying key.
    let key = SigningKey::from_bytes(&[100; 32]).verifying_key();
    let a = derive_x25519_from_ed25519_public(&key);
    let b = derive_x25519_from_ed25519_public(&key);
    assert_eq!(a, b);
    // Different keys produce different Montgomery points
    let other = SigningKey::from_bytes(&[101; 32]).verifying_key();
    assert_ne!(a, derive_x25519_from_ed25519_public(&other));
}
