//! KChat group membership — gate for KCreate multiplayer.
//!
//! KCreate's multiplayer story (LAN sessions, presence, selection,
//! edit locks, persistence + reconnect) is only allowed to run when
//! every connected peer presents a valid membership attestation
//! issued by a **KChat group**. KChat is KCreate's future chat
//! product (not yet defined). This module defines the protocol
//! anchor today so the surrounding pipeline can be implemented and
//! shipped behind a hard runtime gate — when the KChat client lands,
//! it plugs a real [`KChatGroupAuthority`] in via the bridge entry
//! point and multiplayer flips from "locked" to "available" without
//! any further code changes in the collab/protocol layers.
//!
//! ## Trust model
//!
//! A [`KChatMembership`] is an Ed25519-signed attestation that says
//! "the peer holding public key `peer_public_key` is a member of
//! group `group_id` from `issued_at` to `expires_at`". The
//! attestation is bound to the peer's verifying key (so it can't be
//! lifted onto a different peer) and to a specific group (so a
//! membership in group X can't sneak a peer into a multiplayer
//! session for group Y).
//!
//! The signing key (`issuer`) is the KChat group server's identity.
//! KCreate cannot mint attestations; it only **verifies** them
//! against an issuer trust root that the [`KChatGroupAuthority`]
//! impl provides. With no trust root configured (the shipped
//! default), every attestation fails verification and multiplayer
//! is locked.
//!
//! ## Layering
//!
//! * [`KChatGroupAuthority`] — trait the rest of the stack calls
//!   into. Holds the local membership (if any) and verifies remote
//!   attestations against its issuer trust root.
//! * [`NoKChatGroupAuthority`] — default-deny impl. Returns `None`
//!   for `local_membership`, rejects every remote attestation.
//!   Shipped today; multiplayer effectively disabled.
//! * [`BoundKChatGroupAuthority`] — production-shape impl that
//!   carries a verified local membership + an issuer trust root.
//!   Built from a wire-format [`KChatMembership`] via
//!   [`BoundKChatGroupAuthority::install`]. The future KChat client
//!   will call this once it has a fresh attestation from the group
//!   server.
//! * [`InProcessKChatAuthority`] — test-only helper used by the
//!   `kcreate_tests` integration tests to drive the multiplayer
//!   pipeline end-to-end against a deterministic issuer key. **Not
//!   exposed via the bridge.**
//!
//! ## What this module deliberately is NOT
//!
//! * It is not a transport. No I/O. No KChat client. No web tokens.
//! * It does not parse OIDC / JWT / SAML. The attestation format is
//!   a small, KCreate-specific JSON-with-Ed25519 envelope so the
//!   collab crate can verify it without pulling in a JWT crate.
//! * It does not store anything on disk. Persistence (e.g. caching
//!   the membership across launches) belongs to the bridge layer,
//!   which can decide its own policy.

use std::sync::Arc;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{DateTime, Utc};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::peer::PeerId;

/// Maximum length of a [`KChatGroupId`] string. The KChat product
/// will mint ids; KCreate just stores them. 128 bytes is generous
/// for any sane scheme (UUID, ULID, slug) and keeps the wire small.
pub const MAX_GROUP_ID_LEN: usize = 128;

/// Stable identifier for a KChat group. Restricted to a URL-safe
/// alphabet so it can be embedded in QR codes / share links / logs
/// without escaping.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct KChatGroupId(String);

impl KChatGroupId {
    /// Construct a new group id from a string. Returns
    /// [`KChatAuthError::InvalidGroupId`] when the string is empty,
    /// too long, or contains characters outside the URL-safe set
    /// (`A-Z a-z 0-9 - _ .`).
    pub fn new(s: impl Into<String>) -> Result<Self, KChatAuthError> {
        let s = s.into();
        if s.is_empty() || s.len() > MAX_GROUP_ID_LEN {
            return Err(KChatAuthError::InvalidGroupId);
        }
        if !s
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            return Err(KChatAuthError::InvalidGroupId);
        }
        Ok(Self(s))
    }

    /// Borrow the underlying string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for KChatGroupId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Wire-format attestation that the peer holding `peer_public_key`
/// is a member of `group_id` from `issued_at` to `expires_at`.
///
/// The signature covers a canonical view (declaration-order JSON)
/// of every field except `signature` itself. The verifier
/// reconstructs the same view and checks the signature against
/// `issuer_public_key`, which must match the
/// [`KChatGroupAuthority`]'s configured trust root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KChatMembership {
    /// KChat group this attestation belongs to.
    pub group_id: KChatGroupId,
    /// Peer id derived from `peer_public_key`. Stored explicitly so
    /// the wire format is self-checking — the verifier asserts
    /// `derived_peer_id == peer_id` and refuses tampering.
    pub peer_id: PeerId,
    /// Base64url-encoded Ed25519 public key (32 bytes) the membership
    /// is bound to. Must match the local peer's public key when
    /// installed locally, and the remote peer's public key when
    /// presented over the wire.
    pub peer_public_key: String,
    /// Issuance time; clients reject attestations from the future
    /// to bound clock skew.
    pub issued_at: DateTime<Utc>,
    /// Expiry time; clients reject expired attestations. KChat
    /// servers SHOULD issue short-lived attestations (hours, not
    /// months) so revocation is cheap.
    pub expires_at: DateTime<Utc>,
    /// Base64url-encoded Ed25519 verifying key of the KChat group
    /// server that signed this attestation.
    pub issuer_public_key: String,
    /// Base64url-encoded Ed25519 signature (64 bytes) over the
    /// signing view.
    pub signature: String,
}

/// View-only struct used to compute the canonical signing bytes for
/// a [`KChatMembership`]. Identical to the signed fields, in
/// declaration order. Re-serialised on both sides so the bytes
/// under the signature are unambiguous.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MembershipSigningView<'a> {
    group_id: &'a KChatGroupId,
    peer_id: &'a PeerId,
    peer_public_key: &'a str,
    issued_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    issuer_public_key: &'a str,
}

impl KChatMembership {
    /// Mint a fresh attestation. Used by tests and by future
    /// out-of-tree KChat server tooling. The desktop app never
    /// signs its own attestations.
    pub fn issue(
        group_id: KChatGroupId,
        peer_id: PeerId,
        peer_public_key: String,
        issued_at: DateTime<Utc>,
        expires_at: DateTime<Utc>,
        issuer: &SigningKey,
    ) -> Result<Self, KChatAuthError> {
        if issued_at >= expires_at {
            return Err(KChatAuthError::InvalidValidity);
        }
        let issuer_public_key = URL_SAFE_NO_PAD.encode(issuer.verifying_key().as_bytes());
        let view = MembershipSigningView {
            group_id: &group_id,
            peer_id: &peer_id,
            peer_public_key: &peer_public_key,
            issued_at,
            expires_at,
            issuer_public_key: &issuer_public_key,
        };
        let bytes = serde_json::to_vec(&view).map_err(|e| KChatAuthError::Encode(e.to_string()))?;
        let sig = issuer.sign(&bytes);
        Ok(Self {
            group_id,
            peer_id,
            peer_public_key,
            issued_at,
            expires_at,
            issuer_public_key,
            signature: URL_SAFE_NO_PAD.encode(sig.to_bytes()),
        })
    }

    /// Borrow the group id.
    #[must_use]
    pub fn group_id(&self) -> &KChatGroupId {
        &self.group_id
    }

    /// Borrow the peer id.
    #[must_use]
    pub fn peer_id(&self) -> &PeerId {
        &self.peer_id
    }

    /// Verify the attestation against the supplied issuer trust root
    /// and expected peer binding. Fails if any of the following
    /// checks fail:
    ///
    /// * `issuer_public_key` does not match `issuer_trust_root`.
    /// * Signature does not verify (via Ed25519
    ///   [`VerifyingKey::verify_strict`] — rejects non-canonical
    ///   `s` and small-subgroup `R`, matching the trust posture of
    ///   `kcreate_plugin::trust`).
    /// * `peer_id` derived from `peer_public_key` doesn't match the
    ///   stored `peer_id`.
    /// * `peer_public_key` doesn't match the expected peer the
    ///   verifier is checking against.
    /// * `now` is outside `[issued_at, expires_at]`.
    pub fn verify(
        &self,
        issuer_trust_root: &VerifyingKey,
        expected_peer_id: &PeerId,
        expected_peer_public_key: &str,
        now: DateTime<Utc>,
    ) -> Result<(), KChatAuthError> {
        // Cross-bind peer_id ↔ peer_public_key.
        if &self.peer_id != expected_peer_id {
            return Err(KChatAuthError::PeerMismatch);
        }
        if self.peer_public_key != expected_peer_public_key {
            return Err(KChatAuthError::PeerMismatch);
        }
        // Validity window.
        if now < self.issued_at {
            return Err(KChatAuthError::NotYetValid);
        }
        if now > self.expires_at {
            return Err(KChatAuthError::Expired);
        }
        // Cross-check the embedded issuer key matches the trust root
        // we were given.
        let stored_issuer = decode_verifying_key(&self.issuer_public_key)?;
        if stored_issuer.to_bytes() != issuer_trust_root.to_bytes() {
            return Err(KChatAuthError::WrongIssuer);
        }
        // Cross-check the peer_public_key derives the claimed peer_id.
        let pk = decode_verifying_key(&self.peer_public_key)?;
        let derived = PeerId::from_verifying_key(&pk);
        if &derived != expected_peer_id {
            return Err(KChatAuthError::PeerKeyMismatch);
        }
        // Signature.
        let sig_bytes = URL_SAFE_NO_PAD
            .decode(self.signature.as_bytes())
            .map_err(|e| KChatAuthError::Decode(format!("signature base64url: {e}")))?;
        let sig_arr: [u8; 64] = sig_bytes
            .as_slice()
            .try_into()
            .map_err(|_| KChatAuthError::Decode("signature must be 64 bytes".into()))?;
        let signature = Signature::from_bytes(&sig_arr);
        let view = MembershipSigningView {
            group_id: &self.group_id,
            peer_id: &self.peer_id,
            peer_public_key: &self.peer_public_key,
            issued_at: self.issued_at,
            expires_at: self.expires_at,
            issuer_public_key: &self.issuer_public_key,
        };
        let signing_bytes =
            serde_json::to_vec(&view).map_err(|e| KChatAuthError::Encode(e.to_string()))?;
        issuer_trust_root
            .verify_strict(&signing_bytes, &signature)
            .map_err(|e| KChatAuthError::Signature(e.to_string()))?;
        Ok(())
    }
}

fn decode_verifying_key(s: &str) -> Result<VerifyingKey, KChatAuthError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(s.as_bytes())
        .map_err(|e| KChatAuthError::Decode(format!("base64url: {e}")))?;
    let arr: [u8; 32] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| KChatAuthError::Decode("public key must be 32 bytes".into()))?;
    VerifyingKey::from_bytes(&arr).map_err(|e| KChatAuthError::Decode(format!("ed25519: {e}")))
}

/// Errors raised by KChat membership verification and authority
/// lookup.
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum KChatAuthError {
    /// The local peer is not signed into a KChat group, so
    /// multiplayer is locked. This is the default state.
    #[error("multiplayer is locked: not signed into a KChat group")]
    NoKChatBinding,
    /// Wire-level decoding of the attestation failed.
    #[error("KChat membership decode failed: {0}")]
    Decode(String),
    /// Serialising the signing view failed (should be unreachable in
    /// practice; surfaced as a typed error rather than a panic so
    /// the failure mode is debuggable).
    #[error("KChat membership encode failed: {0}")]
    Encode(String),
    /// The Ed25519 signature did not verify under the configured
    /// issuer trust root (or the membership was tampered with after
    /// signing).
    #[error("KChat membership signature invalid: {0}")]
    Signature(String),
    /// The membership's embedded `issuer_public_key` does not match
    /// the trust root the verifier was configured with. KChat
    /// rotates its signing key by minting fresh memberships under
    /// the new trust root; clients drop pre-rotation memberships
    /// here.
    #[error("KChat membership issuer does not match configured trust root")]
    WrongIssuer,
    /// `peer_id` and `peer_public_key` inside the membership do not
    /// agree (the peer id is not the BLAKE3 prefix of the public
    /// key). Signals a tampered attestation.
    #[error("KChat membership peer id does not derive from embedded public key")]
    PeerKeyMismatch,
    /// The attestation is bound to a different peer than the one
    /// the caller is checking against.
    #[error("KChat membership is bound to a different peer")]
    PeerMismatch,
    /// `now < issued_at` — clock skew or a future-dated attestation.
    #[error("KChat membership is not yet valid")]
    NotYetValid,
    /// `now > expires_at` — attestation has aged out, the peer must
    /// refresh it from KChat.
    #[error("KChat membership has expired")]
    Expired,
    /// Group ids do not agree between local and remote memberships.
    #[error("KChat membership group mismatch: local={local}, remote={remote}")]
    GroupMismatch {
        /// Local peer's group.
        local: String,
        /// Remote peer's group.
        remote: String,
    },
    /// `KChatGroupId::new` rejected the input.
    #[error("invalid KChat group id")]
    InvalidGroupId,
    /// `issued_at >= expires_at` at issuance time.
    #[error("KChat membership has invalid validity window (issued_at >= expires_at)")]
    InvalidValidity,
}

/// The collab layer's view of "is multiplayer allowed right now?".
///
/// Implementations are responsible for two things: surfacing the
/// local user's current membership (or `None` if not signed in),
/// and verifying incoming membership attestations from remote
/// peers.
///
/// The trait requires `Send + Sync + Debug` so it can live inside
/// `ProjectSession` (which is held across `.await` points by the
/// transport) and be cloned via `Arc` across threads. The default
/// `verify_remote` impl handles the group-match + signature
/// verification path; impls only need to override it if they need
/// to add their own policy (e.g. revocation, allowlist).
pub trait KChatGroupAuthority: Send + Sync + std::fmt::Debug {
    /// Return the local peer's currently-active membership, if any.
    /// `None` means multiplayer is locked.
    fn local_membership(&self) -> Option<KChatMembership>;

    /// Return the issuer trust root this authority verifies
    /// memberships against. `None` means no trust root is
    /// configured and every remote attestation will fail
    /// verification.
    fn issuer_trust_root(&self) -> Option<VerifyingKey>;

    /// Verify a remote peer's membership against the local trust
    /// root and the local group. Default impl:
    ///
    /// 1. Local membership exists (else `NoKChatBinding`).
    /// 2. Issuer trust root exists (else `NoKChatBinding`).
    /// 3. Remote membership verifies against the trust root + the
    ///    remote peer's id/key.
    /// 4. Remote and local memberships are bound to the same group
    ///    (else `GroupMismatch`).
    fn verify_remote(
        &self,
        remote_peer_id: &PeerId,
        remote_peer_public_key: &str,
        membership: &KChatMembership,
        now: DateTime<Utc>,
    ) -> Result<(), KChatAuthError> {
        let local = self
            .local_membership()
            .ok_or(KChatAuthError::NoKChatBinding)?;
        let trust_root = self
            .issuer_trust_root()
            .ok_or(KChatAuthError::NoKChatBinding)?;
        membership.verify(&trust_root, remote_peer_id, remote_peer_public_key, now)?;
        if local.group_id != membership.group_id {
            return Err(KChatAuthError::GroupMismatch {
                local: local.group_id.as_str().to_string(),
                remote: membership.group_id.as_str().to_string(),
            });
        }
        Ok(())
    }
}

/// Default-deny implementation. Shipped with the desktop app today.
/// Always returns `None` from `local_membership` and `issuer_trust_root`;
/// every remote attestation fails verification with `NoKChatBinding`.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoKChatGroupAuthority;

impl KChatGroupAuthority for NoKChatGroupAuthority {
    fn local_membership(&self) -> Option<KChatMembership> {
        None
    }
    fn issuer_trust_root(&self) -> Option<VerifyingKey> {
        None
    }
}

/// Production-shape authority backed by a verified local
/// membership + an issuer trust root. Built via
/// [`BoundKChatGroupAuthority::install`] from a wire-format
/// attestation. Once installed, this authority returns
/// `Some(membership)` from `local_membership` and unblocks the
/// multiplayer pipeline.
#[derive(Debug, Clone)]
pub struct BoundKChatGroupAuthority {
    membership: KChatMembership,
    trust_root: VerifyingKey,
}

impl BoundKChatGroupAuthority {
    /// Verify + install a fresh local membership. The caller
    /// supplies the issuer trust root out-of-band (KChat client
    /// pinning, baked-in constant, etc.) and the membership it
    /// received from the KChat group server. The membership must
    /// already be signed by `trust_root` and bound to
    /// `local_peer_id` / `local_peer_public_key`.
    pub fn install(
        membership: KChatMembership,
        trust_root: VerifyingKey,
        local_peer_id: &PeerId,
        local_peer_public_key: &str,
        now: DateTime<Utc>,
    ) -> Result<Self, KChatAuthError> {
        membership.verify(&trust_root, local_peer_id, local_peer_public_key, now)?;
        Ok(Self {
            membership,
            trust_root,
        })
    }

    /// Borrow the installed membership.
    #[must_use]
    pub fn membership(&self) -> &KChatMembership {
        &self.membership
    }

    /// Borrow the issuer trust root.
    #[must_use]
    pub fn trust_root(&self) -> &VerifyingKey {
        &self.trust_root
    }
}

impl KChatGroupAuthority for BoundKChatGroupAuthority {
    fn local_membership(&self) -> Option<KChatMembership> {
        Some(self.membership.clone())
    }

    fn issuer_trust_root(&self) -> Option<VerifyingKey> {
        Some(self.trust_root)
    }
}

/// Test-only authority that holds an in-process issuer signing key.
/// Used by the integration tests to drive a full Hello/Welcome
/// handshake end-to-end against a deterministic issuer without
/// requiring a real KChat server. **Never exposed via the bridge.**
#[cfg(any(test, feature = "test-support"))]
#[derive(Debug, Clone)]
pub struct InProcessKChatAuthority {
    membership: KChatMembership,
    trust_root: VerifyingKey,
}

#[cfg(any(test, feature = "test-support"))]
impl InProcessKChatAuthority {
    /// Build an in-process authority for a peer in a fresh group.
    /// `issuer_seed` and `group_id` are caller-provided so multiple
    /// test peers can land in the same group (use the same seed +
    /// group id) or in different groups (vary either).
    pub fn for_peer(
        issuer_seed: [u8; 32],
        group_id: KChatGroupId,
        peer_id: PeerId,
        peer_public_key: String,
        issued_at: DateTime<Utc>,
        expires_at: DateTime<Utc>,
    ) -> Result<Self, KChatAuthError> {
        let issuer = SigningKey::from_bytes(&issuer_seed);
        let trust_root = issuer.verifying_key();
        let membership = KChatMembership::issue(
            group_id,
            peer_id,
            peer_public_key,
            issued_at,
            expires_at,
            &issuer,
        )?;
        Ok(Self {
            membership,
            trust_root,
        })
    }
}

#[cfg(any(test, feature = "test-support"))]
impl KChatGroupAuthority for InProcessKChatAuthority {
    fn local_membership(&self) -> Option<KChatMembership> {
        Some(self.membership.clone())
    }
    fn issuer_trust_root(&self) -> Option<VerifyingKey> {
        Some(self.trust_root)
    }
}

/// Convenience alias used throughout the collab and bridge crates.
/// Authorities are always shared via `Arc` because the transport
/// holds the session in a `tokio::Mutex` across `.await` points and
/// needs cheap clones.
pub type SharedKChatAuthority = Arc<dyn KChatGroupAuthority>;

/// Shorthand: build a [`SharedKChatAuthority`] holding a
/// [`NoKChatGroupAuthority`]. Used as the default when constructing
/// sessions / bridges that have not been bound to a KChat group.
#[must_use]
pub fn no_kchat_authority() -> SharedKChatAuthority {
    Arc::new(NoKChatGroupAuthority)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peer::PeerKey;

    fn make_issuer(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    fn make_peer(seed: u8) -> (PeerId, String) {
        let key = PeerKey::from_seed([seed; 32]);
        let identity = key.identity("test-peer");
        (identity.peer_id, identity.public_key)
    }

    #[test]
    fn group_id_rejects_empty_and_oversized() {
        assert_eq!(
            KChatGroupId::new("").unwrap_err(),
            KChatAuthError::InvalidGroupId
        );
        let big = "x".repeat(MAX_GROUP_ID_LEN + 1);
        assert_eq!(
            KChatGroupId::new(big).unwrap_err(),
            KChatAuthError::InvalidGroupId
        );
    }

    #[test]
    fn group_id_rejects_bad_chars() {
        assert_eq!(
            KChatGroupId::new("has space").unwrap_err(),
            KChatAuthError::InvalidGroupId
        );
        assert_eq!(
            KChatGroupId::new("emoji-\u{1F600}").unwrap_err(),
            KChatAuthError::InvalidGroupId
        );
    }

    #[test]
    fn group_id_accepts_url_safe() {
        for s in ["abc", "group-1", "g_2", "name.with.dots", "0123"] {
            KChatGroupId::new(s).unwrap();
        }
    }

    #[test]
    fn membership_round_trips_verify() {
        let issuer = make_issuer(1);
        let trust_root = issuer.verifying_key();
        let (peer_id, peer_pk) = make_peer(2);
        let group = KChatGroupId::new("group-1").unwrap();
        let issued = Utc::now() - chrono::Duration::minutes(1);
        let expires = Utc::now() + chrono::Duration::hours(1);
        let m = KChatMembership::issue(
            group,
            peer_id.clone(),
            peer_pk.clone(),
            issued,
            expires,
            &issuer,
        )
        .unwrap();
        m.verify(&trust_root, &peer_id, &peer_pk, Utc::now())
            .expect("freshly minted membership must verify");
    }

    #[test]
    fn membership_rejects_wrong_issuer() {
        let issuer = make_issuer(1);
        let bad_root = make_issuer(99).verifying_key();
        let (peer_id, peer_pk) = make_peer(2);
        let m = KChatMembership::issue(
            KChatGroupId::new("g").unwrap(),
            peer_id.clone(),
            peer_pk.clone(),
            Utc::now() - chrono::Duration::seconds(1),
            Utc::now() + chrono::Duration::hours(1),
            &issuer,
        )
        .unwrap();
        let err = m
            .verify(&bad_root, &peer_id, &peer_pk, Utc::now())
            .unwrap_err();
        assert_eq!(err, KChatAuthError::WrongIssuer);
    }

    #[test]
    fn membership_rejects_expired() {
        let issuer = make_issuer(1);
        let trust_root = issuer.verifying_key();
        let (peer_id, peer_pk) = make_peer(2);
        let m = KChatMembership::issue(
            KChatGroupId::new("g").unwrap(),
            peer_id.clone(),
            peer_pk.clone(),
            Utc::now() - chrono::Duration::hours(2),
            Utc::now() - chrono::Duration::hours(1),
            &issuer,
        )
        .unwrap();
        let err = m
            .verify(&trust_root, &peer_id, &peer_pk, Utc::now())
            .unwrap_err();
        assert_eq!(err, KChatAuthError::Expired);
    }

    #[test]
    fn membership_rejects_not_yet_valid() {
        let issuer = make_issuer(1);
        let trust_root = issuer.verifying_key();
        let (peer_id, peer_pk) = make_peer(2);
        let m = KChatMembership::issue(
            KChatGroupId::new("g").unwrap(),
            peer_id.clone(),
            peer_pk.clone(),
            Utc::now() + chrono::Duration::hours(1),
            Utc::now() + chrono::Duration::hours(2),
            &issuer,
        )
        .unwrap();
        let err = m
            .verify(&trust_root, &peer_id, &peer_pk, Utc::now())
            .unwrap_err();
        assert_eq!(err, KChatAuthError::NotYetValid);
    }

    #[test]
    fn membership_rejects_tampering() {
        let issuer = make_issuer(1);
        let trust_root = issuer.verifying_key();
        let (peer_id, peer_pk) = make_peer(2);
        let mut m = KChatMembership::issue(
            KChatGroupId::new("g").unwrap(),
            peer_id.clone(),
            peer_pk.clone(),
            Utc::now() - chrono::Duration::minutes(1),
            Utc::now() + chrono::Duration::hours(1),
            &issuer,
        )
        .unwrap();
        // Flip the group id; signature should fail.
        m.group_id = KChatGroupId::new("evil").unwrap();
        let err = m
            .verify(&trust_root, &peer_id, &peer_pk, Utc::now())
            .unwrap_err();
        assert!(matches!(err, KChatAuthError::Signature(_)), "got {err:?}");
    }

    #[test]
    fn membership_rejects_wrong_peer() {
        let issuer = make_issuer(1);
        let trust_root = issuer.verifying_key();
        let (peer_id, peer_pk) = make_peer(2);
        let (other_peer_id, other_pk) = make_peer(3);
        let m = KChatMembership::issue(
            KChatGroupId::new("g").unwrap(),
            peer_id,
            peer_pk,
            Utc::now() - chrono::Duration::minutes(1),
            Utc::now() + chrono::Duration::hours(1),
            &issuer,
        )
        .unwrap();
        let err = m
            .verify(&trust_root, &other_peer_id, &other_pk, Utc::now())
            .unwrap_err();
        assert_eq!(err, KChatAuthError::PeerMismatch);
    }

    #[test]
    fn no_kchat_authority_denies_everything() {
        let auth = NoKChatGroupAuthority;
        assert!(auth.local_membership().is_none());
        assert!(auth.issuer_trust_root().is_none());
        let issuer = make_issuer(1);
        let (peer_id, peer_pk) = make_peer(2);
        let m = KChatMembership::issue(
            KChatGroupId::new("g").unwrap(),
            peer_id.clone(),
            peer_pk.clone(),
            Utc::now() - chrono::Duration::minutes(1),
            Utc::now() + chrono::Duration::hours(1),
            &issuer,
        )
        .unwrap();
        let err = auth
            .verify_remote(&peer_id, &peer_pk, &m, Utc::now())
            .unwrap_err();
        assert_eq!(err, KChatAuthError::NoKChatBinding);
    }

    #[test]
    fn bound_authority_round_trips() {
        let issuer = make_issuer(7);
        let trust_root = issuer.verifying_key();
        let (peer_id, peer_pk) = make_peer(8);
        let group = KChatGroupId::new("real-group").unwrap();
        let m = KChatMembership::issue(
            group.clone(),
            peer_id.clone(),
            peer_pk.clone(),
            Utc::now() - chrono::Duration::minutes(1),
            Utc::now() + chrono::Duration::hours(1),
            &issuer,
        )
        .unwrap();
        let bound =
            BoundKChatGroupAuthority::install(m, trust_root, &peer_id, &peer_pk, Utc::now())
                .unwrap();
        let local = bound.local_membership().unwrap();
        assert_eq!(local.group_id, group);
        assert_eq!(
            bound.issuer_trust_root().unwrap().to_bytes(),
            trust_root.to_bytes()
        );
    }

    #[test]
    fn bound_authority_rejects_mismatched_local_binding() {
        let issuer = make_issuer(7);
        let trust_root = issuer.verifying_key();
        let (peer_id, peer_pk) = make_peer(8);
        let (other_peer_id, other_pk) = make_peer(9);
        let m = KChatMembership::issue(
            KChatGroupId::new("g").unwrap(),
            peer_id,
            peer_pk,
            Utc::now() - chrono::Duration::minutes(1),
            Utc::now() + chrono::Duration::hours(1),
            &issuer,
        )
        .unwrap();
        let err =
            BoundKChatGroupAuthority::install(m, trust_root, &other_peer_id, &other_pk, Utc::now())
                .unwrap_err();
        assert_eq!(err, KChatAuthError::PeerMismatch);
    }

    #[test]
    fn verify_remote_rejects_cross_group() {
        let issuer = make_issuer(7);
        let trust_root = issuer.verifying_key();
        let (peer_a, pk_a) = make_peer(8);
        let (peer_b, pk_b) = make_peer(9);
        let local = KChatMembership::issue(
            KChatGroupId::new("alpha").unwrap(),
            peer_a.clone(),
            pk_a.clone(),
            Utc::now() - chrono::Duration::minutes(1),
            Utc::now() + chrono::Duration::hours(1),
            &issuer,
        )
        .unwrap();
        let remote = KChatMembership::issue(
            KChatGroupId::new("beta").unwrap(),
            peer_b.clone(),
            pk_b.clone(),
            Utc::now() - chrono::Duration::minutes(1),
            Utc::now() + chrono::Duration::hours(1),
            &issuer,
        )
        .unwrap();
        let bound =
            BoundKChatGroupAuthority::install(local, trust_root, &peer_a, &pk_a, Utc::now())
                .unwrap();
        let err = bound
            .verify_remote(&peer_b, &pk_b, &remote, Utc::now())
            .unwrap_err();
        assert!(
            matches!(err, KChatAuthError::GroupMismatch { .. }),
            "got {err:?}"
        );
    }
}
