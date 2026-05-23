//! Signed, clocked envelope wrapping every message between peers.
//!
//! An [`Envelope`] is the on-the-wire representation of one peer
//! sending a typed payload to another. It carries:
//!
//! * **protocol_version** — bumped whenever the wire format changes.
//!   Mismatched versions are rejected outright by [`Envelope::open`].
//! * **from** — the sender's [`PeerId`].
//! * **clock** — the sender's Lamport clock value at send time.
//! * **nonce** — 16 random bytes; replays are detected by the session
//!   layer keeping a recent-nonce window per peer.
//! * **payload** — any `Serialize` type chosen by the caller, encoded
//!   as `serde_json::Value` so the envelope itself stays generic.
//! * **signature** — Ed25519 signature over a canonical encoding of
//!   the previous four fields. Verification fails if any byte
//!   changes.
//!
//! The signing payload is constructed deterministically: we serialise
//! a fixed-field struct in declaration order, so two peers that
//! produce the same envelope produce the same bytes and the same
//! signature. This is critical — non-canonical encoding would silently
//! invalidate signatures across serde versions.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::clock::LamportClock;
use crate::peer::{PeerId, PeerKeyError};

/// Current on-the-wire protocol version. Bump this whenever the
/// envelope shape, payload variants, or signing canonicalisation
/// changes.
pub const PROTOCOL_VERSION: u32 = 1;

/// 16 bytes of replay-protection nonce. The session layer remembers
/// a sliding window of recently-seen nonces per peer.
pub const NONCE_BYTES: usize = 16;

/// Generic signed envelope used to carry any payload between peers.
/// `T` is intentionally generic so the same code path handles all
/// [`crate::message::Message`] variants, plus future extensions, with
/// one signing implementation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Envelope<T> {
    /// Wire protocol version. Receivers reject anything that doesn't
    /// match [`PROTOCOL_VERSION`].
    pub protocol_version: u32,
    /// Stable peer id of the sender (matches the public key bound to
    /// `signature`).
    pub from: PeerId,
    /// Sender's Lamport clock at the moment of send.
    pub clock: LamportClock,
    /// Base64url-encoded 16-byte nonce.
    pub nonce: String,
    /// The wrapped payload. Stored as `T` so static typing flows
    /// through; signed via its canonical JSON form.
    pub payload: T,
    /// Base64url-encoded Ed25519 signature over the signing payload.
    pub signature: String,
}

/// Canonical view-only struct used to compute the signature. We
/// re-serialise this on both sides (sender and receiver) so that the
/// exact byte sequence under the signature is unambiguous.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SigningView<'a, T> {
    protocol_version: u32,
    from: &'a PeerId,
    clock: LamportClock,
    nonce: &'a str,
    payload: &'a T,
}

/// A signed payload that doesn't yet have an envelope (used inside
/// [`crate::message::Message`] variants that carry sub-signed blobs,
/// e.g. operation broadcasts where each operation can be signed
/// independently so it survives being relayed by an intermediate
/// peer).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignedPayload<T> {
    pub payload: T,
    pub signature: String,
}

impl<T> Envelope<T>
where
    T: Serialize + DeserializeOwned + Clone,
{
    /// Seal a payload from `from` with `signing_key`, attaching the
    /// supplied `clock` and `nonce`. The signing key must match the
    /// `from` peer id; the call does not enforce this (the session
    /// layer holds the matched (key, id) pair) but verification on the
    /// receiving end will reject mismatches.
    pub fn seal(
        from: PeerId,
        clock: LamportClock,
        nonce: [u8; NONCE_BYTES],
        payload: T,
        signing_key: &SigningKey,
    ) -> Result<Self, CollabError> {
        let nonce_str = URL_SAFE_NO_PAD.encode(nonce);
        let signing_bytes = canonical_signing_bytes(&SigningView {
            protocol_version: PROTOCOL_VERSION,
            from: &from,
            clock,
            nonce: &nonce_str,
            payload: &payload,
        })?;
        let sig = signing_key.sign(&signing_bytes);
        Ok(Self {
            protocol_version: PROTOCOL_VERSION,
            from,
            clock,
            nonce: nonce_str,
            payload,
            signature: URL_SAFE_NO_PAD.encode(sig.to_bytes()),
        })
    }

    /// Verify the envelope's signature against `verifying_key` and
    /// the protocol version. On success, return a borrowed view of
    /// the inner payload — the envelope itself is unchanged.
    pub fn open<'a>(
        &'a self,
        verifying_key: &VerifyingKey,
    ) -> Result<&'a T, CollabError> {
        if self.protocol_version != PROTOCOL_VERSION {
            return Err(CollabError::ProtocolVersionMismatch {
                expected: PROTOCOL_VERSION,
                got: self.protocol_version,
            });
        }
        let signing_bytes = canonical_signing_bytes(&SigningView {
            protocol_version: self.protocol_version,
            from: &self.from,
            clock: self.clock,
            nonce: &self.nonce,
            payload: &self.payload,
        })?;
        let sig_bytes = URL_SAFE_NO_PAD
            .decode(self.signature.as_bytes())
            .map_err(|_| CollabError::BadSignatureEncoding)?;
        let sig_arr: [u8; 64] = sig_bytes
            .as_slice()
            .try_into()
            .map_err(|_| CollabError::BadSignatureLength)?;
        let sig = Signature::from_bytes(&sig_arr);
        verifying_key
            .verify(&signing_bytes, &sig)
            .map_err(|_| CollabError::SignatureMismatch)?;
        Ok(&self.payload)
    }

    /// Decode the nonce back into bytes. Used by the session layer to
    /// maintain its replay-protection set.
    pub fn nonce_bytes(&self) -> Result<[u8; NONCE_BYTES], CollabError> {
        let bytes = URL_SAFE_NO_PAD
            .decode(self.nonce.as_bytes())
            .map_err(|_| CollabError::BadNonceEncoding)?;
        bytes
            .as_slice()
            .try_into()
            .map_err(|_| CollabError::BadNonceLength)
    }
}

/// Convenience alias for envelopes carried as opaque bytes (e.g. for
/// hashing into transport-layer logs).
pub type EnvelopeBytes = Vec<u8>;

/// Canonical JSON encoding of the signing view. `serde_json` does
/// **not** guarantee canonical output across versions for arbitrary
/// `Value`s (object key order, number formatting), but for a fixed
/// struct laid out in declaration order with no embedded `Value`s of
/// dynamic shape it's deterministic in practice. We pin the shape
/// here to keep cross-peer signatures verifiable.
fn canonical_signing_bytes<T: Serialize>(view: &SigningView<'_, T>) -> Result<Vec<u8>, CollabError> {
    serde_json::to_vec(view).map_err(|e| CollabError::Encode(e.to_string()))
}

/// Errors emitted by [`Envelope::seal`] and [`Envelope::open`]. The
/// session layer maps these to its own [`crate::SessionError`].
#[derive(Debug, thiserror::Error)]
pub enum CollabError {
    #[error("protocol version mismatch: expected {expected}, got {got}")]
    ProtocolVersionMismatch { expected: u32, got: u32 },
    #[error("envelope signature is not base64url")]
    BadSignatureEncoding,
    #[error("envelope signature is not 64 bytes")]
    BadSignatureLength,
    #[error("envelope signature does not verify against the peer's public key")]
    SignatureMismatch,
    #[error("envelope nonce is not base64url")]
    BadNonceEncoding,
    #[error("envelope nonce is not {NONCE_BYTES} bytes")]
    BadNonceLength,
    #[error("failed to encode payload for signing: {0}")]
    Encode(String),
    #[error(transparent)]
    PeerKey(#[from] PeerKeyError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peer::PeerKey;
    use serde::{Deserialize as De, Serialize as Se};

    #[derive(Debug, Clone, PartialEq, Eq, Se, De)]
    struct Hello {
        text: String,
    }

    fn key(seed: u8) -> PeerKey {
        PeerKey::from_seed([seed; 32])
    }

    #[test]
    fn seal_and_open_round_trip() {
        let k = key(11);
        let env = Envelope::seal(
            k.peer_id(),
            LamportClock::from_raw(5),
            [9u8; NONCE_BYTES],
            Hello {
                text: "hi".into(),
            },
            k.signing_key(),
        )
        .unwrap();
        let opened = env.open(&k.verifying_key()).unwrap();
        assert_eq!(opened.text, "hi");
        assert_eq!(env.nonce_bytes().unwrap(), [9u8; NONCE_BYTES]);
    }

    #[test]
    fn tampered_payload_fails_verification() {
        let k = key(12);
        let mut env = Envelope::seal(
            k.peer_id(),
            LamportClock::from_raw(1),
            [1u8; NONCE_BYTES],
            Hello {
                text: "ok".into(),
            },
            k.signing_key(),
        )
        .unwrap();
        env.payload.text = "tampered".into();
        assert!(matches!(
            env.open(&k.verifying_key()),
            Err(CollabError::SignatureMismatch)
        ));
    }

    #[test]
    fn tampered_clock_fails_verification() {
        let k = key(13);
        let mut env = Envelope::seal(
            k.peer_id(),
            LamportClock::from_raw(1),
            [1u8; NONCE_BYTES],
            Hello {
                text: "ok".into(),
            },
            k.signing_key(),
        )
        .unwrap();
        env.clock = LamportClock::from_raw(2);
        assert!(matches!(
            env.open(&k.verifying_key()),
            Err(CollabError::SignatureMismatch)
        ));
    }

    #[test]
    fn wrong_public_key_fails_verification() {
        let a = key(20);
        let b = key(21);
        let env = Envelope::seal(
            a.peer_id(),
            LamportClock::from_raw(1),
            [2u8; NONCE_BYTES],
            Hello {
                text: "ok".into(),
            },
            a.signing_key(),
        )
        .unwrap();
        assert!(matches!(
            env.open(&b.verifying_key()),
            Err(CollabError::SignatureMismatch)
        ));
    }

    #[test]
    fn protocol_version_mismatch_is_detected() {
        let k = key(30);
        let mut env = Envelope::seal(
            k.peer_id(),
            LamportClock::from_raw(0),
            [0u8; NONCE_BYTES],
            Hello {
                text: "ok".into(),
            },
            k.signing_key(),
        )
        .unwrap();
        env.protocol_version = PROTOCOL_VERSION + 1;
        assert!(matches!(
            env.open(&k.verifying_key()),
            Err(CollabError::ProtocolVersionMismatch { .. })
        ));
    }

    #[test]
    fn envelope_round_trips_through_json() {
        let k = key(40);
        let env = Envelope::seal(
            k.peer_id(),
            LamportClock::from_raw(7),
            [3u8; NONCE_BYTES],
            Hello {
                text: "round".into(),
            },
            k.signing_key(),
        )
        .unwrap();
        let s = serde_json::to_string(&env).unwrap();
        let back: Envelope<Hello> = serde_json::from_str(&s).unwrap();
        assert_eq!(env, back);
        // And signatures still verify after a JSON round-trip.
        let opened = back.open(&k.verifying_key()).unwrap();
        assert_eq!(opened.text, "round");
    }

    #[test]
    fn different_nonces_produce_different_signatures() {
        let k = key(50);
        let a = Envelope::seal(
            k.peer_id(),
            LamportClock::from_raw(1),
            [0u8; NONCE_BYTES],
            Hello {
                text: "x".into(),
            },
            k.signing_key(),
        )
        .unwrap();
        let b = Envelope::seal(
            k.peer_id(),
            LamportClock::from_raw(1),
            [1u8; NONCE_BYTES],
            Hello {
                text: "x".into(),
            },
            k.signing_key(),
        )
        .unwrap();
        assert_ne!(a.signature, b.signature);
        assert_ne!(a.nonce, b.nonce);
    }
}
