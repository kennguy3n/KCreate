//! Phase 7 (Task 21): per-document access-control list.
//!
//! Each `.kstudio/` project gains an `acl.json` file in its metadata
//! directory listing the peer public keys allowed to participate in
//! a collaborative session against that document. The session's host
//! consults the ACL during the Hello handshake: a peer whose public
//! key is not on the allow-list (and who isn't otherwise trusted
//! via the KChat community gate) is rejected with a typed reason.
//!
//! The ACL is structured around **peer public keys** (base64url
//! encoded, matching the wire format the protocol already uses on
//! [`crate::peer::PeerIdentity`]) rather than peer ids because the
//! UI surface that builds the ACL (`AccessControlPanel.tsx`) sees
//! peers via their KChat community-members roster, which carries
//! the public key. Peer ids are derived (BLAKE3 of the public key)
//! and can also be stored alongside for fast lookup.

use serde::{Deserialize, Serialize};

use crate::peer::{PeerId, PeerIdentity};

/// One entry in a project ACL: a peer's public key + the maximum
/// permission level the project owner has granted them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AclEntry {
    /// Base64url (no padding) of the peer's Ed25519 verifying key.
    /// Matches `PeerIdentity::public_key`.
    pub public_key: String,
    /// Optional human-readable label, surfaced by the renderer's
    /// `AccessControlPanel` so the owner can re-identify the entry
    /// without decoding the key.
    #[serde(default)]
    pub display_name: String,
    /// Permission level granted to this peer.
    pub permission: AclPermission,
}

/// Permission level granted to an ACL entry. Maps to
/// [`crate::session`]-level enforcement: editors may broadcast
/// `OperationBroadcast`, viewers may not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AclPermission {
    /// Full read-write access. May broadcast operations and claim
    /// locks.
    Editor,
    /// Read-only access. Receives operations + presence but the
    /// session host silently drops any outbound `OperationBroadcast`
    /// from this peer.
    Viewer,
}

/// The full ACL for one project.
///
/// `mode` controls how the ACL is consulted during Hello:
///
/// * [`AclMode::Open`] — community gate alone is enough; the ACL is
///   purely informational (the renderer may still show it but no
///   peer is rejected for being absent).
/// * [`AclMode::Enforce`] — a peer's public key must appear in
///   `entries` (or the local owner's key) to be admitted. Peers
///   present only via the community gate but absent from the ACL
///   are rejected at Hello-time with reason `"not authorized"`.
///
/// The default is [`AclMode::Open`] so legacy projects (no
/// `acl.json` file) keep working without forcing the user to
/// enumerate every peer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectAcl {
    pub mode: AclMode,
    pub entries: Vec<AclEntry>,
}

impl Default for ProjectAcl {
    fn default() -> Self {
        Self {
            mode: AclMode::Open,
            entries: Vec::new(),
        }
    }
}

/// See [`ProjectAcl`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AclMode {
    Open,
    Enforce,
}

/// Decision returned by [`ProjectAcl::evaluate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AclDecision {
    /// The peer is admitted with the given permission. For
    /// [`AclMode::Open`] the permission defaults to
    /// [`AclPermission::Editor`].
    Allow(AclPermission),
    /// The peer must be rejected at Hello-time.
    Deny,
}

impl ProjectAcl {
    /// Decide whether the given peer is admissible. The host calls
    /// this from its Hello handler before sending the Welcome.
    #[must_use]
    pub fn evaluate(&self, identity: &PeerIdentity) -> AclDecision {
        match self.mode {
            AclMode::Open => AclDecision::Allow(AclPermission::Editor),
            AclMode::Enforce => self
                .entries
                .iter()
                .find(|e| e.public_key == identity.public_key)
                .map_or(AclDecision::Deny, |e| AclDecision::Allow(e.permission)),
        }
    }

    /// Insert or update an entry by public key. Returns the
    /// previous permission (if any).
    pub fn upsert(&mut self, entry: AclEntry) -> Option<AclPermission> {
        if let Some(slot) = self
            .entries
            .iter_mut()
            .find(|e| e.public_key == entry.public_key)
        {
            let prev = slot.permission;
            *slot = entry;
            Some(prev)
        } else {
            self.entries.push(entry);
            None
        }
    }

    /// Remove the entry with the given public key, if any. Returns
    /// the removed permission level.
    pub fn remove(&mut self, public_key: &str) -> Option<AclPermission> {
        let idx = self
            .entries
            .iter()
            .position(|e| e.public_key == public_key)?;
        Some(self.entries.remove(idx).permission)
    }

    /// Return the entry for the given peer id, derived against
    /// every entry's public key (BLAKE3 hash). Useful when the
    /// caller only knows the peer id (e.g. an inbound message's
    /// `from` field).
    #[must_use]
    pub fn lookup_by_peer_id(&self, peer_id: &PeerId) -> Option<&AclEntry> {
        self.entries.iter().find(|e| {
            // Decode the public key and rederive the peer id.
            // Failures (bad encoding) just skip the entry.
            crate::peer::decode_public_key(&e.public_key)
                .ok()
                .is_some_and(|vk| PeerId::from_verifying_key(&vk) == *peer_id)
        })
    }
}

/// Phase 11 Block E Task 27 — magic header prefixed to the encrypted
/// ACL blob written to `acl.json.enc`. Two purposes:
///
/// 1. **File type detection** — the bridge can tell at a glance
///    whether `acl.json.enc` is a v1 ChaCha20-Poly1305 blob or
///    something foreign without parsing into the AEAD path.
/// 2. **Forward compatibility** — bumping the version byte (the last
///    nibble) lets future ACL encryption schemes (e.g. AES-GCM if we
///    pivot away from `chacha20poly1305`) reject mis-formatted
///    payloads without silent-fallback to a weaker reader.
pub const ACL_ENC_MAGIC: &[u8; 8] = b"KCAClv1\0";

/// Length of the per-blob random nonce. ChaCha20-Poly1305 mandates
/// 96-bit nonces; sampling them from the OS CSPRNG per blob is the
/// recommended pattern.
pub const ACL_NONCE_LEN: usize = 12;

/// Errors surfaced by [`encrypt_acl_bytes`] / [`decrypt_acl_bytes`].
/// Distinct from `AclError` (which is about ACL semantics) because
/// every crypto-failure mode in here is fatal: corrupt blob, wrong
/// key, or truncated payload — none of those should ever fall back
/// to a plaintext ACL silently.
#[derive(Debug, thiserror::Error)]
pub enum AclCryptoError {
    #[error("ACL blob is too short ({0} bytes) to contain magic + nonce + AEAD tag")]
    Truncated(usize),
    #[error("ACL blob magic header mismatch — not a KCAClv1 payload")]
    BadMagic,
    #[error("ACL blob nonce sampling failed: {0}")]
    NonceSamplingFailed(String),
    #[error("ACL blob AEAD authentication failed (wrong key, corrupt ciphertext, or tampered)")]
    DecryptFailed,
}

/// Encrypt the JSON-serialised ACL `plaintext` under the same
/// PBKDF2-derived key the SQLCipher database uses. Returns the
/// wire-format blob:
///
/// ```text
/// | magic (8) | nonce (12) | ciphertext+tag (16 + |plaintext|) |
/// ```
///
/// The output is what the bridge writes to `acl.json.enc`. Callers
/// MUST treat decrypt failures as fatal (no fallback to plaintext)
/// to avoid an attacker swapping a re-encrypted blob for plaintext.
///
/// # Errors
///
/// Returns `Err(AclCryptoError::NonceSamplingFailed)` if the OS
/// CSPRNG isn't available; production code on every supported
/// platform has a working CSPRNG, but the failure path is reported
/// rather than panicking so the bridge can surface a typed error.
pub fn encrypt_acl_bytes(key: &[u8; 32], plaintext: &[u8]) -> Result<Vec<u8>, AclCryptoError> {
    use chacha20poly1305::aead::{Aead, KeyInit};
    use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};

    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    let mut nonce_bytes = [0u8; ACL_NONCE_LEN];
    // Sample directly from the OS CSPRNG — same source `clipboard.rs`
    // documents callers should use for AEAD nonces. `getrandom` is
    // fallible on every supported platform (kernel CSPRNG unavailable,
    // sandboxed env without `/dev/urandom`, etc.) so the failure mode
    // is surfaced rather than panicked.
    getrandom::getrandom(&mut nonce_bytes)
        .map_err(|e| AclCryptoError::NonceSamplingFailed(e.to_string()))?;
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|_| AclCryptoError::DecryptFailed)?;
    let mut out = Vec::with_capacity(ACL_ENC_MAGIC.len() + ACL_NONCE_LEN + ciphertext.len());
    out.extend_from_slice(ACL_ENC_MAGIC);
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Inverse of [`encrypt_acl_bytes`]. Returns the original plaintext
/// bytes on success.
///
/// # Errors
///
/// * [`AclCryptoError::Truncated`] — blob is shorter than the
///   magic + nonce + AEAD tag (16 bytes) overhead.
/// * [`AclCryptoError::BadMagic`] — header doesn't match
///   [`ACL_ENC_MAGIC`]; this is the signature of a non-encrypted
///   file or one written by a future incompatible scheme.
/// * [`AclCryptoError::DecryptFailed`] — wrong key, tampered
///   ciphertext, or corrupt tag.
pub fn decrypt_acl_bytes(key: &[u8; 32], blob: &[u8]) -> Result<Vec<u8>, AclCryptoError> {
    use chacha20poly1305::aead::{Aead, KeyInit};
    use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};

    const AEAD_TAG_LEN: usize = 16;
    let min_len = ACL_ENC_MAGIC.len() + ACL_NONCE_LEN + AEAD_TAG_LEN;
    if blob.len() < min_len {
        return Err(AclCryptoError::Truncated(blob.len()));
    }
    let (magic, rest) = blob.split_at(ACL_ENC_MAGIC.len());
    if magic != ACL_ENC_MAGIC {
        return Err(AclCryptoError::BadMagic);
    }
    let (nonce_bytes, ciphertext) = rest.split_at(ACL_NONCE_LEN);
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    let nonce = Nonce::from_slice(nonce_bytes);
    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| AclCryptoError::DecryptFailed)
}

/// Phase 11 Block E Task 27 — distinguish at-a-glance whether a
/// byte buffer is an encrypted ACL payload (starts with the
/// `KCAClv1\0` magic) or a plaintext JSON ACL. Used by the bridge's
/// auto-migration path: when an encrypted project ships with a
/// stale plaintext `acl.json` we want to read it once, re-encrypt
/// it, and delete the plaintext copy.
#[must_use]
pub fn looks_like_encrypted_acl(blob: &[u8]) -> bool {
    blob.len() >= ACL_ENC_MAGIC.len() && &blob[..ACL_ENC_MAGIC.len()] == ACL_ENC_MAGIC
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peer::PeerKey;

    fn identity(seed: u8, name: &str) -> PeerIdentity {
        PeerKey::from_seed([seed; 32]).identity(name)
    }

    #[test]
    fn open_mode_allows_everyone_as_editor() {
        let acl = ProjectAcl::default();
        let ident = identity(1, "alice");
        assert_eq!(
            acl.evaluate(&ident),
            AclDecision::Allow(AclPermission::Editor)
        );
    }

    #[test]
    fn enforce_mode_rejects_unknown_peers() {
        let acl = ProjectAcl {
            mode: AclMode::Enforce,
            entries: vec![],
        };
        assert_eq!(acl.evaluate(&identity(1, "alice")), AclDecision::Deny);
    }

    #[test]
    fn enforce_mode_grants_listed_permission() {
        let alice = identity(1, "alice");
        let bob = identity(2, "bob");
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
    fn upsert_replaces_existing_entry() {
        let alice = identity(1, "alice");
        let mut acl = ProjectAcl {
            mode: AclMode::Enforce,
            entries: vec![AclEntry {
                public_key: alice.public_key.clone(),
                display_name: "Alice".into(),
                permission: AclPermission::Viewer,
            }],
        };
        let prev = acl.upsert(AclEntry {
            public_key: alice.public_key,
            display_name: "Alice (lead)".into(),
            permission: AclPermission::Editor,
        });
        assert_eq!(prev, Some(AclPermission::Viewer));
        assert_eq!(acl.entries.len(), 1);
        assert_eq!(acl.entries[0].permission, AclPermission::Editor);
        assert_eq!(acl.entries[0].display_name, "Alice (lead)");
    }

    #[test]
    fn remove_returns_previous_permission() {
        let alice = identity(1, "alice");
        let mut acl = ProjectAcl {
            mode: AclMode::Enforce,
            entries: vec![AclEntry {
                public_key: alice.public_key.clone(),
                display_name: "Alice".into(),
                permission: AclPermission::Editor,
            }],
        };
        assert_eq!(acl.remove(&alice.public_key), Some(AclPermission::Editor));
        assert!(acl.entries.is_empty());
        assert!(acl.remove(&alice.public_key).is_none());
    }

    #[test]
    fn lookup_by_peer_id_matches_derived_id() {
        let alice = identity(1, "alice");
        let acl = ProjectAcl {
            mode: AclMode::Enforce,
            entries: vec![AclEntry {
                public_key: alice.public_key.clone(),
                display_name: "Alice".into(),
                permission: AclPermission::Editor,
            }],
        };
        assert!(acl.lookup_by_peer_id(&alice.peer_id).is_some());
    }

    #[test]
    fn round_trip_serde() {
        let acl = ProjectAcl {
            mode: AclMode::Enforce,
            entries: vec![AclEntry {
                public_key: "ABC".into(),
                display_name: "Test".into(),
                permission: AclPermission::Editor,
            }],
        };
        let json = serde_json::to_string(&acl).unwrap();
        let back: ProjectAcl = serde_json::from_str(&json).unwrap();
        assert_eq!(back, acl);
    }

    /// Phase 11 Block E Task 27 — encrypt-then-decrypt round-trip on
    /// real plaintext recovers the original bytes exactly, and the
    /// ciphertext is meaningfully different from the input (the
    /// nonce + tag overhead alone is 28 bytes, and the body is
    /// XOR-stream-cipher randomised by the nonce).
    #[test]
    fn encrypt_acl_bytes_round_trip() {
        let key = [0x42u8; 32];
        let plaintext = b"{\"mode\":\"enforce\",\"entries\":[]}";
        let blob = encrypt_acl_bytes(&key, plaintext).expect("encrypt");
        assert!(looks_like_encrypted_acl(&blob));
        assert!(blob.len() > plaintext.len() + ACL_NONCE_LEN);
        let decrypted = decrypt_acl_bytes(&key, &blob).expect("decrypt");
        assert_eq!(decrypted, plaintext);
    }

    /// Two encryptions of the same plaintext under the same key
    /// MUST produce different ciphertexts (different nonces). If
    /// they were equal the nonce sampling would be deterministic
    /// and AEAD security would collapse.
    #[test]
    fn encrypt_acl_bytes_uses_fresh_nonce() {
        let key = [0xA5u8; 32];
        let plaintext = b"identical payload";
        let a = encrypt_acl_bytes(&key, plaintext).expect("a");
        let b = encrypt_acl_bytes(&key, plaintext).expect("b");
        assert_ne!(a, b, "nonce reuse would defeat ChaCha20-Poly1305");
        // But both decrypt back to the same plaintext.
        assert_eq!(decrypt_acl_bytes(&key, &a).unwrap(), plaintext);
        assert_eq!(decrypt_acl_bytes(&key, &b).unwrap(), plaintext);
    }

    /// A bit flipped anywhere in the ciphertext (or tag) must
    /// cause decryption to fail with `DecryptFailed`. Confirms
    /// the AEAD authentication tag is being verified rather than
    /// silently ignored.
    #[test]
    fn decrypt_rejects_tampered_ciphertext() {
        let key = [0x7Eu8; 32];
        let plaintext = b"important ACL content";
        let mut blob = encrypt_acl_bytes(&key, plaintext).expect("encrypt");
        // Flip a byte deep in the ciphertext (skip the magic + nonce prefix).
        let idx = blob.len() - 1;
        blob[idx] ^= 0x01;
        match decrypt_acl_bytes(&key, &blob) {
            Err(AclCryptoError::DecryptFailed) => {}
            other => panic!("expected DecryptFailed, got {other:?}"),
        }
    }

    /// Using the wrong 32-byte key on a valid blob must fail
    /// authentication. The AEAD construction means a wrong key
    /// can't even fall through to a partial plaintext recovery.
    #[test]
    fn decrypt_rejects_wrong_key() {
        let real_key = [0x01u8; 32];
        let wrong_key = [0x02u8; 32];
        let blob = encrypt_acl_bytes(&real_key, b"contents").expect("encrypt");
        match decrypt_acl_bytes(&wrong_key, &blob) {
            Err(AclCryptoError::DecryptFailed) => {}
            other => panic!("expected DecryptFailed, got {other:?}"),
        }
    }

    /// Plaintext JSON bytes (no magic header) must be rejected
    /// with `BadMagic` rather than producing nonsense. This is
    /// the primary signal the bridge uses to detect "this file
    /// is the legacy plaintext ACL" during auto-migration.
    #[test]
    fn decrypt_rejects_plaintext_input() {
        let key = [0u8; 32];
        let plain_json = br#"{"mode":"open","entries":[]}"#;
        match decrypt_acl_bytes(&key, plain_json) {
            // Either Truncated (if shorter than min) or BadMagic
            // (if it happens to be longer) — both are non-silent
            // failures, which is what we want.
            Err(AclCryptoError::BadMagic | AclCryptoError::Truncated(_)) => {}
            other => panic!("expected BadMagic/Truncated, got {other:?}"),
        }
        assert!(!looks_like_encrypted_acl(plain_json));
    }

    /// A blob shorter than the magic + nonce + AEAD tag must
    /// fail with `Truncated(N)`. Confirms we don't index past
    /// the buffer.
    #[test]
    fn decrypt_rejects_truncated_blob() {
        let key = [0u8; 32];
        let blob = b"KCAClv1";
        match decrypt_acl_bytes(&key, blob) {
            Err(AclCryptoError::Truncated(len)) => assert_eq!(len, blob.len()),
            other => panic!("expected Truncated, got {other:?}"),
        }
    }
}
