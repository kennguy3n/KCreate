//! Phase 7 (Task 23): secure peer-to-peer clipboard sharing.
//!
//! When a user copies nodes in a collaborative session they can
//! choose "share selection with <peer>". The clipboard payload is
//! encrypted **client-side** with a key derived from the sender's
//! and recipient's long-lived Ed25519 identities. Relay layers
//! (host LAN, KChat hub) only ever see ciphertext; only the named
//! recipient can decrypt.
//!
//! ## Construction
//!
//! 1. Both peers' Ed25519 keys are converted to their X25519
//!    counterparts. ed25519-dalek's `hazmat::ExpandedSecretKey`
//!    exposes the clamped scalar that `X25519` needs; the
//!    verifying key is converted via the standard Edwards →
//!    Montgomery point map (`VerifyingKey::to_montgomery()`).
//! 2. The sender computes the X25519 shared secret using its own
//!    secret scalar and the recipient's Montgomery point.
//! 3. The shared secret is fed through a BLAKE3 keyed hash with
//!    a fixed-domain label so it cannot collide with any other
//!    secret derived from the same shared secret. The output is
//!    a 32-byte ChaCha20Poly1305 key + 12-byte nonce.
//! 4. The plaintext is encrypted with ChaCha20Poly1305 (AEAD) so
//!    tampering is detected on the receiver side.
//!
//! The wire envelope ([`crate::message::ClipboardSharePayload`])
//! carries the ciphertext + nonce + a short preview label (e.g.
//! `"3 nodes"`) the recipient renders in the accept/reject prompt.

use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key as ChaChaKey, Nonce as ChaChaNonce};
use curve25519_dalek::montgomery::MontgomeryPoint;
use curve25519_dalek::scalar::{clamp_integer, Scalar};
use ed25519_dalek::{SigningKey, VerifyingKey};
use thiserror::Error;

/// Domain separation context fed into `blake3::derive_key` so the
/// derived AEAD key never collides with any other secret a future
/// caller might derive from the same X25519 shared secret.
///
/// We use `blake3::derive_key` rather than `blake3::Hasher::new_keyed`
/// here because `derive_key` is the BLAKE3-specified KDF construction
/// (RFC-style HKDF cousin): the context string is hashed into the
/// key material via the dedicated key-derivation-function mode, which
/// is the correct primitive when the input is a Diffie–Hellman shared
/// secret. `new_keyed` is BLAKE3's MAC mode and is only correct when
/// the caller already has a uniformly random secret key — using it
/// for KDF the way the previous implementation did (hash the
/// context, then use the hash as the MAC key, then MAC the shared
/// secret) is a non-standard construction even though it produces a
/// pseudorandom 32-byte output. `derive_key` makes the intent
/// explicit and matches the BLAKE3 spec's recommendation.
const KDF_CONTEXT: &str = "kcreate-clipboard-share v1 2025-05-27 X25519->ChaCha20-Poly1305 AEAD key";

/// Successfully decrypted clipboard payload — what the renderer
/// hands to the paste pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardPlaintext {
    pub bytes: Vec<u8>,
}

/// Errors raised during clipboard encryption/decryption.
#[derive(Debug, Error)]
pub enum ClipboardCryptoError {
    /// The supplied Ed25519 secret/public key bytes could not be
    /// decoded into a valid X25519 keypair.
    #[error("invalid Ed25519 key material")]
    InvalidKey,
    /// AEAD encryption / decryption failed. For decryption this
    /// indicates either a wrong key (the message was for someone
    /// else) or tampering.
    #[error("AEAD error: {0}")]
    Aead(String),
    /// The decoded nonce was the wrong length.
    #[error("nonce is not 12 bytes")]
    BadNonceLength,
}

/// Derive the X25519 Montgomery point corresponding to a given
/// Ed25519 verifying key. Pure birational mapping — no key
/// material is leaked. Used by both sender (to encrypt to a
/// recipient public key) and the test harness.
///
/// The conversion is the textbook Edwards → Montgomery map exposed
/// by curve25519-dalek as
/// [`VerifyingKey::to_montgomery`](ed25519_dalek::VerifyingKey::to_montgomery).
#[must_use]
pub fn derive_x25519_from_ed25519_public(vk: &VerifyingKey) -> [u8; 32] {
    vk.to_montgomery().to_bytes()
}

/// Encrypt `plaintext` so only the holder of the matching
/// `recipient_public` Ed25519 key can decrypt. The 12-byte AEAD
/// `nonce` is an **input** the caller must generate fresh (e.g.
/// via `getrandom`) and transmit separately alongside the
/// returned ciphertext — this function only returns the
/// ChaCha20-Poly1305 ciphertext + tag, not the nonce.
pub fn encrypt_clipboard_payload(
    sender_signing: &SigningKey,
    recipient_public: &VerifyingKey,
    plaintext: &[u8],
    nonce: [u8; 12],
) -> Result<Vec<u8>, ClipboardCryptoError> {
    let key = derive_shared_aead_key(sender_signing, recipient_public);
    let cipher = ChaCha20Poly1305::new(ChaChaKey::from_slice(&key));
    cipher
        .encrypt(ChaChaNonce::from_slice(&nonce), plaintext)
        .map_err(|e| ClipboardCryptoError::Aead(e.to_string()))
}

/// Decrypt `ciphertext` produced by [`encrypt_clipboard_payload`]
/// on the matching sender side.
pub fn decrypt_clipboard_payload(
    recipient_signing: &SigningKey,
    sender_public: &VerifyingKey,
    ciphertext: &[u8],
    nonce: &[u8],
) -> Result<ClipboardPlaintext, ClipboardCryptoError> {
    if nonce.len() != 12 {
        return Err(ClipboardCryptoError::BadNonceLength);
    }
    let key = derive_shared_aead_key(recipient_signing, sender_public);
    let cipher = ChaCha20Poly1305::new(ChaChaKey::from_slice(&key));
    let bytes = cipher
        .decrypt(ChaChaNonce::from_slice(nonce), ciphertext)
        .map_err(|e| ClipboardCryptoError::Aead(e.to_string()))?;
    Ok(ClipboardPlaintext { bytes })
}

/// Derive the symmetric AEAD key shared between the local
/// (signing) party and the remote public key. The same value is
/// produced on both sides of the conversation because X25519 is
/// commutative.
fn derive_shared_aead_key(
    local_signing: &SigningKey,
    remote_public: &VerifyingKey,
) -> [u8; 32] {
    let scalar = ed25519_scalar_for_x25519(local_signing);
    let remote_point = MontgomeryPoint(derive_x25519_from_ed25519_public(remote_public));
    let shared = (remote_point * scalar).to_bytes();
    // Domain-separated KDF: `derive_key(context, ikm)` is the
    // BLAKE3-specified construction for turning a non-uniform
    // shared secret (here, the X25519 shared point) into a
    // uniformly random 32-byte key. The context string carries the
    // protocol + version + algorithm so two unrelated callers can
    // never derive the same AEAD key from the same shared secret.
    blake3::derive_key(KDF_CONTEXT, &shared)
}

/// Extract the clamped 32-byte X25519 scalar that an Ed25519
/// signing key implies. This is the standard derivation: SHA-512
/// the seed, take the first 32 bytes, and apply the X25519
/// clamping rules.
///
/// `SigningKey::to_scalar_bytes` in ed25519-dalek 2.x returns the
/// **unreduced, unclamped** first 32 bytes of `SHA-512(seed)`
/// — specifically intended as the corresponding
/// `x25519_dalek::StaticSecret` material, which itself applies
/// clamping on construction. We mirror that by piping the bytes
/// through `curve25519_dalek::scalar::clamp_integer` before
/// reducing into a `Scalar`, so the X25519 scalar multiplication
/// in [`derive_x25519_shared_key`] is symmetric across both ends
/// of the exchange. Dropping the explicit clamp produces a
/// different scalar and breaks the round-trip (see the
/// `shared_key_is_symmetric` / `round_trip_decrypts_to_original_plaintext`
/// tests below).
fn ed25519_scalar_for_x25519(signing: &SigningKey) -> Scalar {
    let clamped = clamp_integer(signing.to_scalar_bytes());
    Scalar::from_bytes_mod_order(clamped)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair() -> (SigningKey, VerifyingKey, SigningKey, VerifyingKey) {
        let a = SigningKey::from_bytes(&[7u8; 32]);
        let b = SigningKey::from_bytes(&[11u8; 32]);
        let a_pub = a.verifying_key();
        let b_pub = b.verifying_key();
        (a, a_pub, b, b_pub)
    }

    #[test]
    fn round_trip_decrypts_to_original_plaintext() {
        let (a, _, b, b_pub) = pair();
        let plaintext = b"top secret 3 nodes";
        let nonce = [9u8; 12];
        let ct = encrypt_clipboard_payload(&a, &b_pub, plaintext, nonce).unwrap();
        let pt = decrypt_clipboard_payload(&b, &a.verifying_key(), &ct, &nonce).unwrap();
        assert_eq!(pt.bytes, plaintext);
    }

    #[test]
    fn wrong_recipient_cannot_decrypt() {
        let (a, _, _b, b_pub) = pair();
        let intruder = SigningKey::from_bytes(&[42u8; 32]);
        let nonce = [1u8; 12];
        let ct = encrypt_clipboard_payload(&a, &b_pub, b"hi", nonce).unwrap();
        let result = decrypt_clipboard_payload(&intruder, &a.verifying_key(), &ct, &nonce);
        assert!(matches!(result, Err(ClipboardCryptoError::Aead(_))));
    }

    #[test]
    fn tampered_ciphertext_fails_to_decrypt() {
        let (a, _, b, b_pub) = pair();
        let nonce = [2u8; 12];
        let mut ct = encrypt_clipboard_payload(&a, &b_pub, b"hi", nonce).unwrap();
        ct[0] ^= 0xff;
        let result = decrypt_clipboard_payload(&b, &a.verifying_key(), &ct, &nonce);
        assert!(matches!(result, Err(ClipboardCryptoError::Aead(_))));
    }

    #[test]
    fn shared_key_is_symmetric() {
        let (a, _, b, b_pub) = pair();
        let key_ab = derive_shared_aead_key(&a, &b_pub);
        let key_ba = derive_shared_aead_key(&b, &a.verifying_key());
        assert_eq!(key_ab, key_ba);
    }

    #[test]
    fn bad_nonce_length_is_rejected() {
        let (a, _, b, b_pub) = pair();
        let ct = encrypt_clipboard_payload(&a, &b_pub, b"hi", [0u8; 12]).unwrap();
        let result = decrypt_clipboard_payload(&b, &a.verifying_key(), &ct, &[0u8; 11]);
        assert!(matches!(result, Err(ClipboardCryptoError::BadNonceLength)));
    }
}
