//! KChat group membership issuer (dev tooling).
//!
//! KCreate's multiplayer gate (`kcreate_collab::kchat::KChatGroupAuthority`)
//! is unlocked only when a valid Ed25519-signed [`KChatMembership`]
//! attestation is installed. In production, the membership is
//! issued out-of-tree by the KChat group server. This crate is the
//! **developer-side** counterpart: it lets a KCreate developer
//! mint test attestations against a deterministic issuer key so
//! the multiplayer pipeline can be driven end-to-end without a
//! live KChat server.
//!
//! ## Trust posture
//!
//! * Production builds **do not** depend on this crate. The bridge
//!   exposes its mint function only behind the off-by-default
//!   `kchat-dev-issuer` feature flag.
//! * The crate is kept out of the editing-path dependency tree (see
//!   `kcreate_tests/tests/local_first.rs`) — symmetric with
//!   `kcreate_collab` itself, since the same crypto crates power
//!   both verification and issuance.
//! * There is no networking in this crate. Issuance is a pure
//!   crypto operation over caller-supplied keys + group id.
//!
//! ## Typical use
//!
//! ```ignore
//! use kcreate_kchat::DevIssuer;
//! use std::time::Duration;
//!
//! let issuer = DevIssuer::from_seed([42u8; 32]);
//! let peer_seed = [7u8; 32];
//! let req = issuer
//!     .mint_install_request(
//!         "kchat-dev-group",
//!         &peer_seed,
//!         Duration::from_secs(60 * 60),
//!     )
//!     .expect("mint dev membership");
//! // `req` can be JSON-serialised and passed straight into the
//! // bridge's `kchat_install_authority(req_json)` IPC.
//! ```

use std::time::Duration;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{DateTime, Utc};
use ed25519_dalek::{SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};

pub use kcreate_collab::kchat::{KChatAuthError, KChatGroupId, KChatMembership, MAX_GROUP_ID_LEN};
pub use kcreate_collab::peer::PeerId;

/// Errors surfaced by the dev issuer.
#[derive(Debug, thiserror::Error)]
pub enum DevIssuerError {
    /// The group id failed `KChatGroupId::new` validation (empty,
    /// too long, or contains non-URL-safe characters).
    #[error("invalid group id: {0}")]
    InvalidGroupId(KChatAuthError),

    /// The peer seed was the wrong shape. Ed25519 signing keys
    /// require exactly 32 bytes of seed entropy.
    #[error("peer seed must be 32 bytes")]
    InvalidPeerSeed,

    /// The validity duration was zero or absurdly large (more than
    /// 365 days). The KChat product is expected to mint
    /// short-lived attestations (hours); the dev issuer caps the
    /// upper bound to make accidentally minting a never-expiring
    /// dev token harder.
    #[error("validity duration must be > 0 and <= 365 days")]
    InvalidValidity,

    /// Wrapping `KChatMembership::issue` failure. Should be rare
    /// (the underlying call only fails on invalid time windows,
    /// which we guard ahead of time).
    #[error("kchat issuance failed: {0}")]
    Issue(#[from] KChatAuthError),
}

/// Maximum validity window the dev issuer will mint. Production
/// KChat attestations should be much shorter (hours).
//
// `Duration::from_days` / `from_hours` are not yet stable as
// `const fn`, so we hand-roll the seconds arithmetic. The
// `duration_suboptimal_units` lint flags this but the const
// requirement wins — re-evaluate when `duration_constructors`
// stabilises.
#[allow(clippy::duration_suboptimal_units)]
pub const MAX_DEV_VALIDITY: Duration = Duration::from_secs(365 * 24 * 60 * 60);

/// Wire-format DTO that mirrors `kcreate_bridge::collab::KChatInstallRequest`
/// exactly. Returned by [`DevIssuer::mint_install_request`] so the
/// caller can JSON-serialise it and pass it straight back into the
/// bridge's `kchat_install_authority` call without going through any
/// intermediate type.
///
/// We don't depend on `kcreate_bridge` here (it's a cdylib + huge
/// dep graph), so this struct is re-declared with the same field
/// shape. A unit test in `kcreate_bridge::collab` pins the lockstep
/// by round-tripping a minted request through the bridge type.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DevInstallRequest {
    /// 32-byte Ed25519 verifying key of the dev issuer (this is
    /// what the bridge installs as its `issuer_trust_root`), URL-
    /// safe base64 (no padding).
    pub issuer_public_key: String,
    /// Group identifier.
    pub group_id: String,
    /// BLAKE3-derived peer id.
    pub peer_id: String,
    /// 32-byte Ed25519 verifying key of the local peer, URL-safe
    /// base64 (no padding).
    pub peer_public_key: String,
    /// Membership issuance time (RFC3339).
    pub issued_at: DateTime<Utc>,
    /// Membership expiry time (RFC3339).
    pub expires_at: DateTime<Utc>,
    /// 64-byte Ed25519 signature over the canonical signing view,
    /// URL-safe base64 (no padding).
    pub signature: String,
}

/// Developer-side KChat issuer. Wraps a deterministic Ed25519
/// signing key and lets the caller mint dev memberships bound to
/// arbitrary local peer keys.
#[derive(Debug, Clone)]
pub struct DevIssuer {
    signing_key: SigningKey,
}

impl DevIssuer {
    /// Build a dev issuer from a 32-byte seed. Same seed produces
    /// the same issuer key across runs (intended — useful for
    /// CI reproducibility).
    #[must_use]
    pub fn from_seed(seed: [u8; 32]) -> Self {
        Self {
            signing_key: SigningKey::from_bytes(&seed),
        }
    }

    /// Borrow the issuer's verifying key. This is the value the
    /// bridge installs as `issuer_trust_root` when accepting an
    /// attestation produced by this issuer.
    #[must_use]
    pub fn verifying_key(&self) -> VerifyingKey {
        self.signing_key.verifying_key()
    }

    /// URL-safe base64 (no padding) encoding of the issuer's
    /// verifying key — i.e. the value that goes into the
    /// `issuerPublicKey` field of the bridge install request.
    #[must_use]
    pub fn verifying_key_b64(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.verifying_key().as_bytes())
    }

    /// Mint a fresh membership attestation for a peer derived from
    /// `peer_seed`. The validity window is `[now, now + valid_for]`.
    ///
    /// Returns a [`DevInstallRequest`] whose JSON encoding is
    /// drop-in compatible with the bridge's
    /// `kchat_install_authority` IPC entry point.
    pub fn mint_install_request(
        &self,
        group_id: &str,
        peer_seed: &[u8],
        valid_for: Duration,
    ) -> Result<DevInstallRequest, DevIssuerError> {
        if peer_seed.len() != 32 {
            return Err(DevIssuerError::InvalidPeerSeed);
        }
        if valid_for.is_zero() || valid_for > MAX_DEV_VALIDITY {
            return Err(DevIssuerError::InvalidValidity);
        }

        let group =
            KChatGroupId::new(group_id.to_string()).map_err(DevIssuerError::InvalidGroupId)?;

        let mut peer_seed_arr = [0u8; 32];
        peer_seed_arr.copy_from_slice(peer_seed);
        let peer_signing = SigningKey::from_bytes(&peer_seed_arr);
        let peer_vk = peer_signing.verifying_key();
        let peer_id = PeerId::from_verifying_key(&peer_vk);
        let peer_public_key_b64 = URL_SAFE_NO_PAD.encode(peer_vk.as_bytes());

        let issued_at = Utc::now();
        let expires_at = issued_at
            + chrono::Duration::from_std(valid_for).map_err(|_| DevIssuerError::InvalidValidity)?;

        let membership = KChatMembership::issue(
            group,
            peer_id.clone(),
            peer_public_key_b64.clone(),
            issued_at,
            expires_at,
            &self.signing_key,
        )?;

        Ok(DevInstallRequest {
            issuer_public_key: self.verifying_key_b64(),
            group_id: membership.group_id().as_str().to_string(),
            peer_id: peer_id.as_str().to_string(),
            peer_public_key: peer_public_key_b64,
            issued_at,
            expires_at,
            signature: membership.signature,
        })
    }

    /// Mint a membership for an externally-derived peer key. Used
    /// when the caller already has an Ed25519 verifying key (e.g.
    /// reusing the PresencePanel's persistent seed) and only needs
    /// to bind a membership onto it.
    pub fn mint_install_request_for_peer(
        &self,
        group_id: &str,
        peer_public_key_b64: &str,
        valid_for: Duration,
    ) -> Result<DevInstallRequest, DevIssuerError> {
        if valid_for.is_zero() || valid_for > MAX_DEV_VALIDITY {
            return Err(DevIssuerError::InvalidValidity);
        }

        let group =
            KChatGroupId::new(group_id.to_string()).map_err(DevIssuerError::InvalidGroupId)?;

        let peer_vk_bytes = URL_SAFE_NO_PAD
            .decode(peer_public_key_b64.trim_end_matches('='))
            .map_err(|_| DevIssuerError::InvalidPeerSeed)?;
        if peer_vk_bytes.len() != 32 {
            return Err(DevIssuerError::InvalidPeerSeed);
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&peer_vk_bytes);
        let peer_vk =
            VerifyingKey::from_bytes(&arr).map_err(|_| DevIssuerError::InvalidPeerSeed)?;
        let peer_id = PeerId::from_verifying_key(&peer_vk);

        let issued_at = Utc::now();
        let expires_at = issued_at
            + chrono::Duration::from_std(valid_for).map_err(|_| DevIssuerError::InvalidValidity)?;

        let membership = KChatMembership::issue(
            group,
            peer_id.clone(),
            peer_public_key_b64.to_string(),
            issued_at,
            expires_at,
            &self.signing_key,
        )?;

        Ok(DevInstallRequest {
            issuer_public_key: self.verifying_key_b64(),
            group_id: membership.group_id().as_str().to_string(),
            peer_id: peer_id.as_str().to_string(),
            peer_public_key: peer_public_key_b64.to_string(),
            issued_at,
            expires_at,
            signature: membership.signature,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_issuer_mints_verifiable_membership() {
        let issuer = DevIssuer::from_seed([1u8; 32]);
        let req = issuer
            .mint_install_request("dev.group.kcreate", &[2u8; 32], Duration::from_hours(1))
            .expect("mint");
        // Rebuild the underlying KChatMembership and verify against
        // the issuer's trust root. PeerId is `#[serde(transparent)]`
        // so we round-trip through serde to reconstruct it from the
        // base64url string in the install request (the same shape
        // it sits in on the wire).
        let group = KChatGroupId::new(req.group_id.clone()).expect("group");
        let peer_id: PeerId =
            serde_json::from_str(&serde_json::to_string(&req.peer_id).unwrap()).expect("peer id");
        let m = KChatMembership {
            group_id: group,
            peer_id: peer_id.clone(),
            peer_public_key: req.peer_public_key.clone(),
            issued_at: req.issued_at,
            expires_at: req.expires_at,
            issuer_public_key: req.issuer_public_key.clone(),
            signature: req.signature.clone(),
        };
        m.verify(
            &issuer.verifying_key(),
            &peer_id,
            &req.peer_public_key,
            Utc::now(),
        )
        .expect("verify minted membership");
    }

    #[test]
    fn rejects_short_peer_seed() {
        let issuer = DevIssuer::from_seed([3u8; 32]);
        let err = issuer
            .mint_install_request("dev.group", &[0u8; 16], Duration::from_mins(1))
            .unwrap_err();
        assert!(matches!(err, DevIssuerError::InvalidPeerSeed));
    }

    #[test]
    fn rejects_zero_validity() {
        let issuer = DevIssuer::from_seed([4u8; 32]);
        let err = issuer
            .mint_install_request("dev.group", &[0u8; 32], Duration::from_secs(0))
            .unwrap_err();
        assert!(matches!(err, DevIssuerError::InvalidValidity));
    }

    #[test]
    fn rejects_over_one_year_validity() {
        let issuer = DevIssuer::from_seed([5u8; 32]);
        let err = issuer
            .mint_install_request(
                "dev.group",
                &[0u8; 32],
                MAX_DEV_VALIDITY + Duration::from_secs(1),
            )
            .unwrap_err();
        assert!(matches!(err, DevIssuerError::InvalidValidity));
    }

    #[test]
    fn rejects_invalid_group_id() {
        let issuer = DevIssuer::from_seed([6u8; 32]);
        let err = issuer
            .mint_install_request("invalid space!", &[0u8; 32], Duration::from_mins(1))
            .unwrap_err();
        assert!(matches!(err, DevIssuerError::InvalidGroupId(_)));
    }

    #[test]
    fn mint_for_external_peer_key_round_trips() {
        let issuer = DevIssuer::from_seed([7u8; 32]);
        let peer_signing = SigningKey::from_bytes(&[8u8; 32]);
        let peer_vk_b64 = URL_SAFE_NO_PAD.encode(peer_signing.verifying_key().as_bytes());
        let req = issuer
            .mint_install_request_for_peer("dev.group", &peer_vk_b64, Duration::from_hours(1))
            .expect("mint for external peer");
        assert_eq!(req.peer_public_key, peer_vk_b64);
        // Peer id derives from the verifying key.
        let expected = PeerId::from_verifying_key(&peer_signing.verifying_key());
        assert_eq!(req.peer_id, expected.as_str());
    }
}
