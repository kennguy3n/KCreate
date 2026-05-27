//! Membership attestation bridging.
//!
//! Maps the wire-format [`MembershipAttestation`] returned by
//! `kchat.communities.getMembership` into the
//! [`kcreate_collab::KChatMembership`] type the collab gate verifies
//! against, and exposes a [`KChatDesktopAuthority`] implementing
//! [`KChatGroupAuthority`] so the bridge can plug it straight into
//! the collab session.
//!
//! Auto-refresh: when an attestation is within
//! [`REFRESH_BEFORE_EXPIRY`] of expiring, the authority transparently
//! re-asks uney-chat-desktop for a fresh one. Callers (the collab
//! session) never see the old expired membership.

use std::sync::Arc;
use std::time::Duration;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{DateTime, Utc};
use ed25519_dalek::VerifyingKey;
use kcreate_collab::kchat::{
    KChatAuthError, KChatGroupAuthority, KChatGroupId, KChatMembership, SharedKChatAuthority,
};
use kcreate_collab::peer::PeerId;
use parking_lot::RwLock;

use crate::client::KChatDesktopClient;
use crate::error::ClientError;
use crate::protocol::MembershipAttestation;

/// Trigger a refresh when the remaining lifetime drops below this
/// window. Set to 5 minutes per the Phase 7 spec; configurable via
/// [`KChatDesktopAuthority::with_refresh_window`].
#[allow(clippy::duration_suboptimal_units)] // `from_mins` requires nightly.
pub const REFRESH_BEFORE_EXPIRY: Duration = Duration::from_secs(300);

/// Map a [`MembershipAttestation`] (wire format) into a
/// [`KChatMembership`] (collab type). The two structs are
/// field-compatible; this function only renames + decodes the group
/// id.
pub fn membership_from_attestation(
    att: MembershipAttestation,
) -> Result<KChatMembership, ClientError> {
    let group_id = KChatGroupId::new(att.group_id.clone())
        .map_err(|e| ClientError::AttestationInvalid(format!("invalid groupId: {e:?}")))?;
    // `PeerId` is `Deserialize` (transparent string newtype). Going
    // through serde keeps the wire-format parsing rules in one place
    // — KCreate's `kcreate_collab::peer` is the canonical source of
    // truth for what's a syntactically-valid peer id.
    let peer_id: PeerId = serde_json::from_value(serde_json::Value::String(att.peer_id.clone()))
        .map_err(|e| ClientError::AttestationInvalid(format!("invalid peerId: {e}")))?;

    Ok(KChatMembership {
        group_id,
        peer_id,
        peer_public_key: att.peer_public_key,
        issued_at: att.issued_at,
        expires_at: att.expires_at,
        issuer_public_key: att.issuer_public_key,
        signature: att.signature,
    })
}

/// Decode a base64url-no-pad Ed25519 verifying key into a
/// `VerifyingKey`. Used to feed the issuer trust root into the
/// authority.
pub fn decode_verifying_key(b64: &str) -> Result<VerifyingKey, ClientError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(b64.trim_end_matches('='))
        .map_err(|e| ClientError::AttestationInvalid(format!("issuer base64: {e}")))?;
    let arr: [u8; 32] = bytes.as_slice().try_into().map_err(|_| {
        ClientError::AttestationInvalid("issuer public key must be 32 bytes".into())
    })?;
    VerifyingKey::from_bytes(&arr).map_err(|e| {
        ClientError::AttestationInvalid(format!("issuer is not a valid Ed25519 key: {e}"))
    })
}

/// Production-shape KChat authority backed by a live uney-chat-desktop
/// connection.
///
/// Holds a cached [`KChatMembership`] + issuer trust root, plus a
/// handle to the [`KChatDesktopClient`] so it can refresh on demand.
/// Refresh is gated by [`Self::refresh_if_needed`] which the bridge
/// calls on a tick.
pub struct KChatDesktopAuthority {
    inner: Arc<RwLock<AuthorityInner>>,
    client: Arc<KChatDesktopClient>,
    community_id: String,
    refresh_window: Duration,
    local_peer_id: PeerId,
    local_public_key: String,
}

impl std::fmt::Debug for KChatDesktopAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KChatDesktopAuthority")
            .field("community_id", &self.community_id)
            .field("local_peer_id", &self.local_peer_id.as_str())
            .field("refresh_window_secs", &self.refresh_window.as_secs())
            .finish()
    }
}

struct AuthorityInner {
    membership: KChatMembership,
    trust_root: VerifyingKey,
}

impl KChatDesktopAuthority {
    /// Install a fresh attestation against the supplied client and
    /// build the authority. The attestation is verified locally
    /// (signature + binding + validity window) before installation.
    pub fn install(
        client: Arc<KChatDesktopClient>,
        community_id: impl Into<String>,
        attestation: MembershipAttestation,
        local_peer_id: PeerId,
        local_public_key: String,
        now: DateTime<Utc>,
    ) -> Result<Self, ClientError> {
        let community_id = community_id.into();
        let trust_root = decode_verifying_key(&attestation.issuer_public_key)?;
        let membership = membership_from_attestation(attestation)?;

        membership
            .verify(&trust_root, &local_peer_id, &local_public_key, now)
            .map_err(|e| ClientError::AttestationInvalid(format!("install: {e}")))?;

        Ok(Self {
            inner: Arc::new(RwLock::new(AuthorityInner {
                membership,
                trust_root,
            })),
            client,
            community_id,
            refresh_window: REFRESH_BEFORE_EXPIRY,
            local_peer_id,
            local_public_key,
        })
    }

    /// Override the refresh window (default 5 minutes). Used by the
    /// integration tests to simulate near-expiry without actually
    /// waiting hours.
    #[must_use]
    pub fn with_refresh_window(mut self, window: Duration) -> Self {
        self.refresh_window = window;
        self
    }

    /// Wrap as a [`SharedKChatAuthority`] suitable for handing to
    /// `LanCollabHost::start`.
    #[must_use]
    pub fn into_shared(self) -> SharedKChatAuthority {
        Arc::new(self)
    }

    /// Borrow the currently-cached membership (snapshot — not held
    /// across awaits).
    pub fn cached_membership(&self) -> KChatMembership {
        self.inner.read().membership.clone()
    }

    /// Borrow the community id this authority is bound to.
    #[must_use]
    pub fn community_id(&self) -> &str {
        &self.community_id
    }

    /// If the cached membership is within the refresh window of
    /// expiry, fetch a fresh attestation, verify it, and swap it in.
    /// Otherwise this is a no-op.
    ///
    /// Returns `true` when a refresh actually happened.
    pub async fn refresh_if_needed(&self) -> Result<bool, ClientError> {
        let needs_refresh = {
            let inner = self.inner.read();
            let now = Utc::now();
            let remaining = inner.membership.expires_at.signed_duration_since(now);
            let window = chrono::Duration::from_std(self.refresh_window)
                .unwrap_or_else(|_| chrono::Duration::seconds(0));
            remaining <= window
        };
        if !needs_refresh {
            return Ok(false);
        }

        let fresh = self.client.get_membership(&self.community_id).await?;
        let trust_root = decode_verifying_key(&fresh.issuer_public_key)?;
        let membership = membership_from_attestation(fresh)?;
        let now = Utc::now();
        membership
            .verify(
                &trust_root,
                &self.local_peer_id,
                &self.local_public_key,
                now,
            )
            .map_err(|e| ClientError::AttestationInvalid(format!("refresh: {e}")))?;
        let mut guard = self.inner.write();
        guard.membership = membership;
        guard.trust_root = trust_root;
        Ok(true)
    }

    /// Explicitly install a new attestation (e.g. after a transport
    /// reconnect). Equivalent to [`Self::refresh_if_needed`] but
    /// unconditional.
    pub async fn force_refresh(&self) -> Result<(), ClientError> {
        let fresh = self.client.get_membership(&self.community_id).await?;
        let trust_root = decode_verifying_key(&fresh.issuer_public_key)?;
        let membership = membership_from_attestation(fresh)?;
        let now = Utc::now();
        membership
            .verify(
                &trust_root,
                &self.local_peer_id,
                &self.local_public_key,
                now,
            )
            .map_err(|e| ClientError::AttestationInvalid(format!("force_refresh: {e}")))?;
        let mut guard = self.inner.write();
        guard.membership = membership;
        guard.trust_root = trust_root;
        Ok(())
    }
}

impl KChatGroupAuthority for KChatDesktopAuthority {
    fn local_membership(&self) -> Option<KChatMembership> {
        Some(self.inner.read().membership.clone())
    }

    fn issuer_trust_root(&self) -> Option<VerifyingKey> {
        Some(self.inner.read().trust_root)
    }

    fn verify_remote(
        &self,
        remote_peer_id: &PeerId,
        remote_peer_public_key: &str,
        membership: &KChatMembership,
        now: DateTime<Utc>,
    ) -> Result<(), KChatAuthError> {
        // Same logic as the default impl, but we snapshot the
        // RwLock once instead of two separate calls (which would
        // race a refresh between them).
        let inner = self.inner.read();
        membership.verify(
            &inner.trust_root,
            remote_peer_id,
            remote_peer_public_key,
            now,
        )?;
        if inner.membership.group_id != membership.group_id {
            return Err(KChatAuthError::GroupMismatch {
                local: inner.membership.group_id.as_str().to_string(),
                remote: membership.group_id.as_str().to_string(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use kcreate_collab::peer::PeerKey;

    fn make_attestation(
        issuer: &SigningKey,
        group: &str,
        peer_seed: [u8; 32],
        issued_at: DateTime<Utc>,
        expires_at: DateTime<Utc>,
    ) -> MembershipAttestation {
        let peer_key = PeerKey::from_seed(peer_seed);
        let identity = peer_key.identity("test");
        let group_id = KChatGroupId::new(group.to_string()).expect("test group id is valid");
        let membership = KChatMembership::issue(
            group_id,
            identity.peer_id.clone(),
            identity.public_key,
            issued_at,
            expires_at,
            issuer,
        )
        .expect("issue must succeed");
        MembershipAttestation {
            issuer_public_key: membership.issuer_public_key.clone(),
            group_id: membership.group_id.as_str().to_string(),
            peer_id: membership.peer_id.as_str().to_string(),
            peer_public_key: membership.peer_public_key.clone(),
            issued_at: membership.issued_at,
            expires_at: membership.expires_at,
            signature: membership.signature.clone(),
        }
    }

    #[test]
    fn membership_from_attestation_round_trips_through_verify() {
        let issuer = SigningKey::from_bytes(&[1u8; 32]);
        let trust_root = issuer.verifying_key();
        let peer_key = PeerKey::from_seed([2u8; 32]);
        let identity = peer_key.identity("test");

        let issued_at = Utc::now() - chrono::Duration::seconds(10);
        let expires_at = issued_at + chrono::Duration::hours(1);
        let att = make_attestation(&issuer, "group-a", [2u8; 32], issued_at, expires_at);

        let m = membership_from_attestation(att).expect("conversion succeeds");
        m.verify(
            &trust_root,
            &identity.peer_id,
            &identity.public_key,
            Utc::now(),
        )
        .expect("verification succeeds after conversion");
    }

    #[test]
    fn invalid_group_id_is_rejected() {
        let att = MembershipAttestation {
            issuer_public_key: "AA".into(),
            group_id: "spaces are not allowed".into(),
            peer_id: "x".into(),
            peer_public_key: "y".into(),
            issued_at: Utc::now(),
            expires_at: Utc::now() + chrono::Duration::seconds(60),
            signature: "z".into(),
        };
        let err = membership_from_attestation(att).unwrap_err();
        assert!(matches!(err, ClientError::AttestationInvalid(_)));
    }
}
