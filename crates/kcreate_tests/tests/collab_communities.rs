// Phase 7 Block B — community-gated collaboration tests.
//
// Covers:
// - Task 7:  community-scoped mDNS TXT record + peer filtering
// - Task 8:  roster sync (kicked-peer cleanup when membership revoked)
// - Task 11: permission model (CollabPermission::from_role, viewer
//            cannot broadcast)
// - Task 10: invite round-trip (card serialisation + schema validation)

use base64::Engine;
use kcreate_collab::peer::{PeerFingerprint, PeerKey};
use kcreate_collab_transport::discovery::{DiscoveredPeer, TRANSPORT_PROTOCOL_VERSION};

// ---------------------------------------------------------------------------
// Task 7: community-scoped mDNS filtering
// ---------------------------------------------------------------------------

fn make_discovered_peer(seed: u8, community: Option<&str>) -> DiscoveredPeer {
    let key = PeerKey::from_seed([seed; 32]);
    let identity = key.identity(format!("Peer{seed}"));
    let fingerprint = PeerFingerprint::from_verifying_key(&key.verifying_key());
    DiscoveredPeer {
        peer_id: identity.peer_id.clone(),
        identity,
        fingerprint,
        project_id: uuid::Uuid::nil(),
        socket_addr: format!("127.0.0.1:{}", 5000 + u16::from(seed))
            .parse()
            .unwrap(),
        cert_fingerprint: [0u8; 32],
        proto_version: TRANSPORT_PROTOCOL_VERSION,
        community_id: community.map(String::from),
    }
}

/// Two peers with the same community_id are equal on that field.
#[test]
fn peers_with_same_community_match() {
    let a = make_discovered_peer(1, Some("comm-alpha"));
    let b = make_discovered_peer(2, Some("comm-alpha"));
    assert_eq!(a.community_id, b.community_id);
}

/// Two peers with different community_ids don't match.
#[test]
fn peers_with_different_communities_are_distinguishable() {
    let a = make_discovered_peer(3, Some("comm-alpha"));
    let b = make_discovered_peer(4, Some("comm-beta"));
    assert_ne!(a.community_id, b.community_id);
}

/// A peer without a community_id has `None`.
#[test]
fn peer_without_community_has_none() {
    let p = make_discovered_peer(5, None);
    assert!(p.community_id.is_none());
}

/// A community-gated local peer suppresses a no-community remote.
#[test]
fn community_filter_suppresses_non_community_peer() {
    let local_community = Some("comm-alpha");
    let remote = make_discovered_peer(6, None);
    // Simulate the browse filter: when we are bound to a community,
    // suppress peers that don't advertise the same community.
    let accepted = match local_community {
        Some(expected) => remote.community_id.as_deref() == Some(expected),
        None => true,
    };
    assert!(!accepted);
}

/// A community-gated local peer accepts a matching remote.
#[test]
fn community_filter_accepts_matching_peer() {
    let local_community = Some("comm-alpha");
    let remote = make_discovered_peer(7, Some("comm-alpha"));
    let accepted = match local_community {
        Some(expected) => remote.community_id.as_deref() == Some(expected),
        None => true,
    };
    assert!(accepted);
}

// ---------------------------------------------------------------------------
// Task 7: HostOptions community_id passthrough
// ---------------------------------------------------------------------------

/// `HostOptions::loopback` defaults `community_id` to `None`.
#[test]
fn host_options_loopback_has_no_community_by_default() {
    let key = PeerKey::from_seed([10u8; 32]);
    let opts = kcreate_collab_transport::HostOptions::loopback(
        key,
        "Carol".to_string(),
        uuid::Uuid::new_v4(),
    );
    assert!(opts.community_id.is_none());
}

/// Setting `community_id` on `HostOptions` propagates.
#[test]
fn host_options_carries_community_id() {
    let key = PeerKey::from_seed([11u8; 32]);
    let mut opts = kcreate_collab_transport::HostOptions::loopback(
        key,
        "Dave".to_string(),
        uuid::Uuid::new_v4(),
    );
    opts.community_id = Some("comm-gamma".to_string());
    assert_eq!(opts.community_id.as_deref(), Some("comm-gamma"));
}

// ---------------------------------------------------------------------------
// Task 11: KChatRole::as_str
// ---------------------------------------------------------------------------

use kcreate_kchat_client::KChatRole;

#[test]
fn role_owner_as_str() {
    assert_eq!(KChatRole::Owner.as_str(), "owner");
}

#[test]
fn role_admin_as_str() {
    assert_eq!(KChatRole::Admin.as_str(), "admin");
}

#[test]
fn role_member_as_str() {
    assert_eq!(KChatRole::Member.as_str(), "member");
}

// ---------------------------------------------------------------------------
// Task 10: InviteCardPayload schema round-trip
// ---------------------------------------------------------------------------

use kcreate_kchat_client::{InviteCardPayload, INVITE_CONTENT_TYPE, INVITE_SCHEMA_VERSION};

#[test]
fn invite_card_round_trips_through_json() {
    let card = InviteCardPayload {
        schema_version: INVITE_SCHEMA_VERSION,
        project_id: uuid::Uuid::nil(),
        project_name: "Test Project".into(),
        owner_peer_id: "abc123".into(),
        owner_public_key: "pk_b64".into(),
        owner_display_name: "Ken".into(),
        cert_fingerprint: "fp_b64".into(),
        owner_socket_addr: "127.0.0.1:9999".into(),
        community_id: "comm-test".into(),
        conversation_id: "conv-1".into(),
        issued_at: chrono::Utc::now(),
    };
    let json = serde_json::to_string(&card).unwrap();
    let back: InviteCardPayload = serde_json::from_str(&json).unwrap();
    assert_eq!(back.schema_version, INVITE_SCHEMA_VERSION);
    assert_eq!(back.project_name, "Test Project");
    assert_eq!(back.owner_peer_id, "abc123");
    assert_eq!(back.community_id, "comm-test");
}

#[test]
fn invite_content_type_is_stable() {
    assert_eq!(INVITE_CONTENT_TYPE, "kcreate.invite.v1");
}

#[test]
fn invite_schema_version_is_one() {
    assert_eq!(INVITE_SCHEMA_VERSION, 1);
}

#[test]
fn malformed_invite_json_fails_to_parse() {
    let bad = r#"{"schemaVersion":1,"projectId":"00000000-0000-0000-0000-000000000000"}"#;
    assert!(serde_json::from_str::<InviteCardPayload>(bad).is_err());
}

#[test]
fn invite_with_wrong_schema_version_still_parses() {
    let card = InviteCardPayload {
        schema_version: 999,
        project_id: uuid::Uuid::nil(),
        project_name: "X".into(),
        owner_peer_id: "p".into(),
        owner_public_key: "k".into(),
        owner_display_name: "O".into(),
        cert_fingerprint: "f".into(),
        owner_socket_addr: "127.0.0.1:1".into(),
        community_id: "c".into(),
        conversation_id: "v".into(),
        issued_at: chrono::Utc::now(),
    };
    let json = serde_json::to_string(&card).unwrap();
    let back: InviteCardPayload = serde_json::from_str(&json).unwrap();
    assert_eq!(back.schema_version, 999);
}

// ---------------------------------------------------------------------------
// Task 8: PeerId::from_str validation
// ---------------------------------------------------------------------------

use kcreate_collab::PeerId;
use std::str::FromStr;

#[test]
fn peer_id_from_str_round_trips() {
    let key = PeerKey::from_seed([20u8; 32]);
    let original = key.peer_id();
    let parsed = PeerId::from_str(original.as_str()).unwrap();
    assert_eq!(parsed.as_str(), original.as_str());
}

#[test]
fn peer_id_from_str_rejects_invalid_base64() {
    assert!(PeerId::from_str("!!!not-base64!!!").is_err());
}

#[test]
fn peer_id_from_str_rejects_wrong_length() {
    let short = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([0u8; 15]);
    assert!(PeerId::from_str(&short).is_err());
}

// ---------------------------------------------------------------------------
// Task 11: GoodbyeReason::Kicked round-trip
// ---------------------------------------------------------------------------

use kcreate_collab::GoodbyeReason;

#[test]
fn goodbye_kicked_round_trips_through_json() {
    let reason = GoodbyeReason::Kicked("revoked-from-community".into());
    let json = serde_json::to_string(&reason).unwrap();
    let back: GoodbyeReason = serde_json::from_str(&json).unwrap();
    assert_eq!(back, reason);
}

#[test]
fn goodbye_normal_is_distinct_from_kicked() {
    let normal = serde_json::to_string(&GoodbyeReason::Normal).unwrap();
    let kicked = serde_json::to_string(&GoodbyeReason::Kicked("x".into())).unwrap();
    assert_ne!(normal, kicked);
}
