//! Peer identity, keys, and fingerprints.
//!
//! Each peer in a LAN collaboration session has a long-lived Ed25519
//! keypair. The **peer id** is the first 16 bytes of a BLAKE3 hash of
//! the public key, base64url-encoded — a deterministic, short,
//! human-readable identifier that the trust UI displays alongside a
//! display name.
//!
//! Trust is established the same way `kcreate_plugin`'s native plugin
//! signing works: the user explicitly trusts another peer's public
//! key (presented as a fingerprint) before any operations from that
//! peer are accepted into the local document. There is no central
//! authority.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use ed25519_dalek::{SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};

/// Length of the BLAKE3-derived [`PeerId`] prefix, in bytes. 16 bytes
/// = 128 bits of entropy, ample for the small-N (≤ a few dozen)
/// peer-on-LAN regime KCreate targets.
const PEER_ID_BYTES: usize = 16;

/// Short, deterministic identifier for a peer. Computed as the first
/// 16 bytes of `BLAKE3(verifying_key.to_bytes())`.
///
/// Stable across sessions for the same keypair, so the trust UI can
/// display a peer's id once and the user can recognise them
/// thereafter.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PeerId(String);

impl PeerId {
    /// Compute the peer id for a given Ed25519 public key.
    #[must_use]
    pub fn from_verifying_key(vk: &VerifyingKey) -> Self {
        let hash = blake3::hash(vk.as_bytes());
        let bytes = &hash.as_bytes()[..PEER_ID_BYTES];
        Self(URL_SAFE_NO_PAD.encode(bytes))
    }

    /// Borrow the underlying base64url string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for PeerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::str::FromStr for PeerId {
    type Err = crate::CollabError;

    /// Parse a peer id from its base64url-no-pad rendering (the
    /// exact wire format produced by [`PeerId::as_str`]). Used by
    /// the bridge layer when the renderer hands a peer id back
    /// across the IPC boundary as an opaque string -- e.g.
    /// `session_kick_peer(peer_id)` or
    /// `session_set_peer_permission(peer_id, perm)`.
    ///
    /// Validates that the input decodes to exactly
    /// [`PEER_ID_BYTES`] bytes so a typo can't spoof a different
    /// peer id by accident.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let raw = URL_SAFE_NO_PAD
            .decode(s.as_bytes())
            .map_err(|e| crate::CollabError::InvalidEncoding(format!("peerId base64: {e}")))?;
        if raw.len() != PEER_ID_BYTES {
            return Err(crate::CollabError::InvalidEncoding(format!(
                "peerId is {} bytes after base64 decode, expected {PEER_ID_BYTES}",
                raw.len()
            )));
        }
        Ok(Self(s.to_string()))
    }
}

/// Long-form, human-presentable rendering of a peer's public key.
/// Designed to be displayed in the trust UI alongside the display
/// name, in groups of four uppercase hex digits separated by spaces,
/// e.g. `1A2B 3C4D 5E6F 7081 …` — the same convention used for
/// WireGuard / Signal safety numbers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PeerFingerprint(String);

impl PeerFingerprint {
    /// Render the fingerprint for a given public key.
    #[must_use]
    pub fn from_verifying_key(vk: &VerifyingKey) -> Self {
        let hash = blake3::hash(vk.as_bytes());
        // 8 groups of 4 hex digits = 32 hex chars = 16 bytes —
        // matches PeerId's entropy budget without making the user
        // squint at a 64-char hash.
        let mut s = String::with_capacity(8 * 5);
        for (i, byte_pair) in hash.as_bytes()[..16].chunks(2).enumerate() {
            if i > 0 {
                s.push(' ');
            }
            for byte in byte_pair {
                use std::fmt::Write;
                let _ = write!(&mut s, "{byte:02X}");
            }
        }
        Self(s)
    }

    /// Borrow the rendered string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for PeerFingerprint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Public-facing identity advertised in [`crate::message::HelloPayload`].
/// Carries the public key plus a display name, but never the private
/// signing key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerIdentity {
    /// Stable short id derived from `public_key`.
    pub peer_id: PeerId,
    /// User-chosen display name (e.g. "Ken on the iMac"). Up to 64
    /// chars after trimming; longer strings are rejected by
    /// [`PeerIdentity::new`].
    pub display_name: String,
    /// Base64url-encoded Ed25519 public key (32 bytes).
    pub public_key: String,
}

impl PeerIdentity {
    /// Build a peer identity from a verifying key + display name.
    /// Trims and bounds the display name to 64 chars to keep the wire
    /// envelope small and the trust UI predictable.
    pub fn new(vk: &VerifyingKey, display_name: impl Into<String>) -> Self {
        let mut name = display_name.into();
        name = name.trim().to_string();
        if name.chars().count() > 64 {
            name = name.chars().take(64).collect();
        }
        Self {
            peer_id: PeerId::from_verifying_key(vk),
            display_name: name,
            public_key: URL_SAFE_NO_PAD.encode(vk.as_bytes()),
        }
    }

    /// Decode the public-key string back into a [`VerifyingKey`].
    pub fn verifying_key(&self) -> Result<VerifyingKey, PeerKeyError> {
        let bytes = URL_SAFE_NO_PAD
            .decode(self.public_key.as_bytes())
            .map_err(|_| PeerKeyError::BadEncoding)?;
        let arr: [u8; 32] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| PeerKeyError::WrongLength)?;
        VerifyingKey::from_bytes(&arr).map_err(|_| PeerKeyError::Invalid)
    }
}

/// Local-only handle that owns the Ed25519 signing key. Not
/// `Serialize` — the secret key never leaves the process via the wire
/// format; it's persisted by the storage layer behind an OS keychain
/// the same way `kcreate_plugin`'s trust store is.
#[derive(Debug)]
pub struct PeerKey {
    signing: SigningKey,
}

impl PeerKey {
    /// Construct a peer key from a 32-byte seed. The seed is the
    /// canonical persistence form (matches `ed25519_dalek::SigningKey::from_bytes`).
    #[must_use]
    pub fn from_seed(seed: [u8; 32]) -> Self {
        Self {
            signing: SigningKey::from_bytes(&seed),
        }
    }

    /// The Ed25519 verifying (public) key.
    #[must_use]
    pub fn verifying_key(&self) -> VerifyingKey {
        self.signing.verifying_key()
    }

    /// The Ed25519 signing (secret) key. Use sparingly — exposing
    /// this defeats the trust model.
    #[must_use]
    pub fn signing_key(&self) -> &SigningKey {
        &self.signing
    }

    /// The peer id for this key.
    #[must_use]
    pub fn peer_id(&self) -> PeerId {
        PeerId::from_verifying_key(&self.verifying_key())
    }

    /// The fingerprint to show the user when establishing trust.
    #[must_use]
    pub fn fingerprint(&self) -> PeerFingerprint {
        PeerFingerprint::from_verifying_key(&self.verifying_key())
    }

    /// Build the public [`PeerIdentity`] for this key, with the given
    /// display name.
    pub fn identity(&self, display_name: impl Into<String>) -> PeerIdentity {
        PeerIdentity::new(&self.verifying_key(), display_name)
    }
}

/// Things that can go wrong while decoding a public key.
#[derive(Debug, thiserror::Error)]
pub enum PeerKeyError {
    /// The base64url payload was not decodable.
    #[error("peer public key is not valid base64url")]
    BadEncoding,
    /// The decoded payload was not exactly 32 bytes.
    #[error("peer public key is not 32 bytes")]
    WrongLength,
    /// `ed25519-dalek` rejected the key (e.g. low-order point).
    #[error("peer public key is not a valid Ed25519 point")]
    Invalid,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(seed: u8) -> PeerKey {
        PeerKey::from_seed([seed; 32])
    }

    #[test]
    fn peer_id_is_deterministic_for_same_key() {
        let key = k(7);
        assert_eq!(key.peer_id(), key.peer_id());
    }

    #[test]
    fn peer_id_differs_for_different_keys() {
        let a = k(1).peer_id();
        let b = k(2).peer_id();
        assert_ne!(a, b);
    }

    #[test]
    fn peer_id_string_is_url_safe_and_short() {
        let id = k(3).peer_id();
        let s = id.as_str();
        // 16 bytes -> 22 chars of base64url (no padding).
        assert_eq!(s.len(), 22);
        for ch in s.chars() {
            assert!(
                ch.is_ascii_alphanumeric() || ch == '-' || ch == '_',
                "peer id should be url-safe, got {ch:?}"
            );
        }
    }

    #[test]
    fn fingerprint_groups_are_uppercase_hex() {
        let fp = k(4).fingerprint();
        let s = fp.as_str();
        // 8 groups of 4 hex chars separated by 7 spaces = 39 chars.
        assert_eq!(s.len(), 8 * 4 + 7);
        for ch in s.chars() {
            assert!(
                ch == ' ' || ch.is_ascii_uppercase() || ch.is_ascii_digit(),
                "fingerprint contains unexpected char {ch:?}"
            );
        }
    }

    #[test]
    fn peer_identity_round_trips_through_json() {
        let key = k(5);
        let ident = key.identity("Ken on the iMac");
        let json = serde_json::to_string(&ident).unwrap();
        let back: PeerIdentity = serde_json::from_str(&json).unwrap();
        assert_eq!(ident, back);
        // And the verifying key recovers cleanly.
        let vk = back.verifying_key().unwrap();
        assert_eq!(vk.as_bytes(), key.verifying_key().as_bytes());
    }

    #[test]
    fn peer_identity_trims_and_caps_display_name() {
        let key = k(6);
        let ident = key.identity("   Ken on the iMac   ");
        assert_eq!(ident.display_name, "Ken on the iMac");

        let long = "a".repeat(200);
        let ident = key.identity(long);
        assert_eq!(ident.display_name.chars().count(), 64);
    }

    #[test]
    fn invalid_public_key_is_rejected() {
        let ident = PeerIdentity {
            peer_id: PeerId("zzzz".into()),
            display_name: "bad".into(),
            public_key: "not!valid!base64".into(),
        };
        assert!(matches!(
            ident.verifying_key(),
            Err(PeerKeyError::BadEncoding)
        ));

        let wrong_len = PeerIdentity {
            peer_id: PeerId("zzzz".into()),
            display_name: "bad".into(),
            public_key: URL_SAFE_NO_PAD.encode(b"too short"),
        };
        assert!(matches!(
            wrong_len.verifying_key(),
            Err(PeerKeyError::WrongLength)
        ));
    }
}
