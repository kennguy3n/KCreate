//! Trust store + Ed25519 signature verification for plugins.
//!
//! # Trust model
//!
//! Plugins ship two files side-by-side: `manifest.json` (the manifest
//! body, unchanged from Block C) and `manifest.json.sig` (a JSON
//! sidecar with `{ "key_id": "...", "signature_b64": "..." }`). The
//! signed message is the **raw bytes of `manifest.json`** as they
//! exist on disk — no canonical-JSON re-serialisation, no field
//! omission games. This is deliberate: any agreement between two
//! parties on "what got signed" has to survive both parties using
//! different JSON libraries, and the simplest thing that does that is
//! "literally these bytes".
//!
//! The trust store itself is a JSON file the host application loads
//! at startup, mapping `key_id` to a base64-encoded Ed25519 public
//! key. By design the registry refuses to load a [`crate::PluginType::Native`]
//! plugin unless it carries a `manifest.json.sig` whose signature
//! verifies under a key the store knows. Sandboxed plugin types
//! (`Wasm`, `JsPanel`) MAY carry signatures (for provenance) but are
//! not required to.
//!
//! All verification uses [`VerifyingKey::verify_strict`] so malleable
//! signatures, signatures with non-canonical `s`, and signatures from
//! the small-subgroup of the Edwards curve are rejected. We do not
//! call [`VerifyingKey::verify`] anywhere.

use std::collections::HashMap;
use std::path::Path;

use base64::Engine;
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Bytes per Ed25519 public key. The constant lives here so the
/// length checks below are self-documenting.
const ED25519_PUBLIC_KEY_LEN: usize = 32;
/// Bytes per Ed25519 signature.
const ED25519_SIGNATURE_LEN: usize = 64;

/// One entry in the on-disk trust file. The `id` is a stable
/// identifier the plugin's `manifest.json.sig` references in its
/// `key_id` field; `comment` is purely informational and surfaced in
/// the UI's "trusted authorities" list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustedKey {
    /// Opaque identifier (typically a reverse-DNS string such as
    /// `com.kcreate.official`). The signing tool puts this string in
    /// the `key_id` field of the produced signature.
    pub id: String,
    /// Base64-URL (no padding) of the 32-byte Ed25519 public key.
    /// We use URL-safe base64 because some package managers and CI
    /// systems mangle the `+` / `/` of standard base64.
    pub public_key_b64: String,
    /// Free-form human-readable label. Not used for verification —
    /// the registry surfaces it through the bridge so the UI can
    /// display "Signed by Acme Corp (official build key)".
    #[serde(default)]
    pub comment: String,
}

/// In-memory trust store. Loaded once at host startup from a JSON
/// file shaped like `[ TrustedKey, ... ]`. Cheap to clone; the
/// `VerifyingKey` type is itself 32 bytes plus a precomputed point.
#[derive(Debug, Default, Clone)]
pub struct TrustStore {
    keys: HashMap<String, VerifyingKey>,
    comments: HashMap<String, String>,
}

#[derive(Debug, Error)]
pub enum TrustError {
    #[error("trust store io: {0}")]
    Io(#[from] std::io::Error),
    #[error("trust store json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("trust store: key '{0}' has invalid base64: {1}")]
    InvalidKeyEncoding(String, String),
    #[error("trust store: key '{id}' has wrong public-key length: expected {expected}, got {actual}")]
    WrongKeyLength {
        id: String,
        expected: usize,
        actual: usize,
    },
    #[error("trust store: key '{0}' is not a valid Ed25519 public key: {1}")]
    InvalidKey(String, String),
    #[error("trust store: duplicate key id '{0}'")]
    DuplicateKey(String),
    #[error("signature: unknown key id '{0}'")]
    UnknownKeyId(String),
    #[error("signature: invalid base64 in sidecar: {0}")]
    InvalidSignatureEncoding(String),
    #[error("signature: wrong length: expected {expected}, got {actual}")]
    WrongSignatureLength { expected: usize, actual: usize },
    #[error("signature: verification failed")]
    VerificationFailed,
}

impl TrustStore {
    /// Build a store from an in-memory list of trusted keys. This is
    /// the primary entry point used by tests and by callers that
    /// embed the trust list rather than reading it from disk.
    pub fn from_keys(keys: Vec<TrustedKey>) -> Result<Self, TrustError> {
        let mut by_id: HashMap<String, VerifyingKey> = HashMap::with_capacity(keys.len());
        let mut comments: HashMap<String, String> = HashMap::with_capacity(keys.len());
        for k in keys {
            if by_id.contains_key(&k.id) {
                return Err(TrustError::DuplicateKey(k.id));
            }
            let bytes = decode_b64(&k.public_key_b64)
                .map_err(|e| TrustError::InvalidKeyEncoding(k.id.clone(), e))?;
            if bytes.len() != ED25519_PUBLIC_KEY_LEN {
                return Err(TrustError::WrongKeyLength {
                    id: k.id,
                    expected: ED25519_PUBLIC_KEY_LEN,
                    actual: bytes.len(),
                });
            }
            let arr: [u8; ED25519_PUBLIC_KEY_LEN] = bytes
                .try_into()
                .expect("just checked length matches ED25519_PUBLIC_KEY_LEN");
            let vk = VerifyingKey::from_bytes(&arr)
                .map_err(|e| TrustError::InvalidKey(k.id.clone(), e.to_string()))?;
            by_id.insert(k.id.clone(), vk);
            comments.insert(k.id, k.comment);
        }
        Ok(Self {
            keys: by_id,
            comments,
        })
    }

    /// Load a JSON-encoded trust file. The expected shape is a top-
    /// level array of [`TrustedKey`] records. A missing file is *not*
    /// an error caller-side — the host application can call
    /// [`Self::default`] to start with an empty store.
    pub fn load_from_path(path: &Path) -> Result<Self, TrustError> {
        let bytes = std::fs::read(path)?;
        let keys: Vec<TrustedKey> = serde_json::from_slice(&bytes)?;
        Self::from_keys(keys)
    }

    /// Iterator over `(key_id, comment)` pairs in arbitrary order.
    /// Used by the bridge to surface the trust list to the UI.
    pub fn entries(&self) -> impl Iterator<Item = (&str, &str)> + '_ {
        self.comments
            .iter()
            .map(|(id, c)| (id.as_str(), c.as_str()))
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// `true` if a key with this id is in the store.
    pub fn contains(&self, key_id: &str) -> bool {
        self.keys.contains_key(key_id)
    }

    /// Verify `signature_b64` against `message` for `key_id`. Uses
    /// `verify_strict`, so any signature that is malleable, has a
    /// non-canonical scalar, or whose component lives in a small
    /// subgroup is rejected. This is the only public verification
    /// entry point — there is intentionally no `verify_loose` /
    /// `verify_legacy` helper.
    pub fn verify(
        &self,
        key_id: &str,
        message: &[u8],
        signature_b64: &str,
    ) -> Result<(), TrustError> {
        let vk = self
            .keys
            .get(key_id)
            .ok_or_else(|| TrustError::UnknownKeyId(key_id.to_string()))?;
        let sig_bytes = decode_b64(signature_b64).map_err(TrustError::InvalidSignatureEncoding)?;
        if sig_bytes.len() != ED25519_SIGNATURE_LEN {
            return Err(TrustError::WrongSignatureLength {
                expected: ED25519_SIGNATURE_LEN,
                actual: sig_bytes.len(),
            });
        }
        let sig_arr: [u8; ED25519_SIGNATURE_LEN] = sig_bytes
            .try_into()
            .expect("just checked length matches ED25519_SIGNATURE_LEN");
        let signature = Signature::from_bytes(&sig_arr);
        vk.verify_strict(message, &signature)
            .map_err(|_| TrustError::VerificationFailed)
    }
}

fn decode_b64(s: &str) -> Result<Vec<u8>, String> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(s.as_bytes())
        .map_err(|e| e.to_string())
}

/// Helper exposed for the signing tool / tests: encode raw bytes as
/// URL-safe unpadded base64 in the exact shape the trust store
/// consumes.
pub fn encode_b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn signing_key_from_seed(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    fn trusted_pair(id: &str, sk: &SigningKey) -> TrustedKey {
        TrustedKey {
            id: id.to_string(),
            public_key_b64: encode_b64(sk.verifying_key().as_bytes()),
            comment: format!("test key {id}"),
        }
    }

    #[test]
    fn round_trip_verifies() {
        let sk = signing_key_from_seed(7);
        let store = TrustStore::from_keys(vec![trusted_pair("k1", &sk)]).unwrap();
        let msg = b"a real manifest payload, not just hello world";
        let sig = sk.sign(msg);
        let sig_b64 = encode_b64(&sig.to_bytes());
        store.verify("k1", msg, &sig_b64).unwrap();
    }

    #[test]
    fn wrong_key_rejects() {
        let signer = signing_key_from_seed(1);
        let other = signing_key_from_seed(2);
        let store = TrustStore::from_keys(vec![trusted_pair("k1", &other)]).unwrap();
        let msg = b"manifest body";
        let sig = signer.sign(msg);
        let sig_b64 = encode_b64(&sig.to_bytes());
        assert!(matches!(
            store.verify("k1", msg, &sig_b64),
            Err(TrustError::VerificationFailed)
        ));
    }

    #[test]
    fn tampered_message_rejects() {
        let sk = signing_key_from_seed(3);
        let store = TrustStore::from_keys(vec![trusted_pair("k1", &sk)]).unwrap();
        let msg = b"original message";
        let sig = sk.sign(msg);
        let sig_b64 = encode_b64(&sig.to_bytes());
        let tampered = b"original messagX";
        assert!(matches!(
            store.verify("k1", tampered, &sig_b64),
            Err(TrustError::VerificationFailed)
        ));
    }

    #[test]
    fn unknown_key_id_rejects() {
        let sk = signing_key_from_seed(4);
        let store = TrustStore::from_keys(vec![trusted_pair("known", &sk)]).unwrap();
        let msg = b"hello";
        let sig = sk.sign(msg);
        let sig_b64 = encode_b64(&sig.to_bytes());
        assert!(matches!(
            store.verify("unknown", msg, &sig_b64),
            Err(TrustError::UnknownKeyId(_))
        ));
    }

    #[test]
    fn malformed_signature_b64_rejects() {
        let sk = signing_key_from_seed(5);
        let store = TrustStore::from_keys(vec![trusted_pair("k", &sk)]).unwrap();
        assert!(matches!(
            store.verify("k", b"m", "!!!not base64!!!"),
            Err(TrustError::InvalidSignatureEncoding(_))
        ));
    }

    #[test]
    fn wrong_signature_length_rejects() {
        let sk = signing_key_from_seed(6);
        let store = TrustStore::from_keys(vec![trusted_pair("k", &sk)]).unwrap();
        let too_short = encode_b64(&[0u8; 32]); // only 32 bytes instead of 64
        assert!(matches!(
            store.verify("k", b"m", &too_short),
            Err(TrustError::WrongSignatureLength {
                expected: 64,
                actual: 32
            })
        ));
    }

    #[test]
    fn duplicate_key_id_rejects() {
        let sk = signing_key_from_seed(8);
        let err =
            TrustStore::from_keys(vec![trusted_pair("dup", &sk), trusted_pair("dup", &sk)]).unwrap_err();
        assert!(matches!(err, TrustError::DuplicateKey(_)));
    }

    #[test]
    fn wrong_key_length_rejects() {
        let bad = TrustedKey {
            id: "short".to_string(),
            public_key_b64: encode_b64(&[1u8; 16]), // 16 bytes instead of 32
            comment: String::new(),
        };
        let err = TrustStore::from_keys(vec![bad]).unwrap_err();
        assert!(matches!(
            err,
            TrustError::WrongKeyLength {
                expected: 32,
                actual: 16,
                ..
            }
        ));
    }

    #[test]
    fn load_from_disk_parses_array() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trust.json");
        let sk = signing_key_from_seed(9);
        let key = trusted_pair("disk-key", &sk);
        std::fs::write(&path, serde_json::to_vec(&vec![key]).unwrap()).unwrap();
        let store = TrustStore::load_from_path(&path).unwrap();
        assert!(store.contains("disk-key"));
        assert_eq!(store.len(), 1);
    }
}
