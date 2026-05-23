//! Ephemeral self-signed TLS certificates + fingerprint-pinned verifier.
//!
//! QUIC requires TLS; we don't want a CA hierarchy. The classic
//! solution for peer-to-peer transports (also used by Tailscale and
//! Iroh) is to:
//!
//! 1. Generate a fresh self-signed leaf cert at startup.
//! 2. Advertise that cert's SHA-256 fingerprint out-of-band — in our
//!    case, in the mDNS TXT record alongside the long-lived
//!    Ed25519 peer fingerprint.
//! 3. When dialing, install a custom rustls
//!    [`rustls::client::danger::ServerCertVerifier`] that ignores
//!    PKI entirely and just compares the presented leaf cert DER
//!    against the pinned SHA-256 fingerprint.
//!
//! The Ed25519 peer identity remains the actual authentication
//! anchor: every protocol message is signed by the peer's long-lived
//! signing key and verified at the application layer via
//! [`kcreate_collab::envelope::Envelope::open`] in
//! [`kcreate_collab::session::ProjectSession::ingest_envelope`].
//! TLS here only protects confidentiality + integrity of the byte
//! stream and ensures we're actually talking to the box that
//! announced the matching fingerprint over mDNS.

use std::sync::Arc;

use base64::Engine;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, Error as TlsError, SignatureScheme};
use sha2::{Digest, Sha256};

use crate::error::TransportError;

/// A self-signed certificate + key bundle and the SHA-256 fingerprint
/// of its DER encoding. The fingerprint is what we advertise over
/// mDNS so peers can pin us against MITM.
///
/// Note: a hand-written `Clone` impl is required because
/// `PrivatePkcs8KeyDer` is intentionally not `Clone` (the rustls
/// authors want to discourage accidental key duplication). We use
/// the explicit [`PrivatePkcs8KeyDer::clone_key`] escape hatch so
/// the host can hand the bundle to both the QUIC server config and
/// the mDNS responder without juggling lifetimes.
#[derive(Debug)]
pub struct CertBundle {
    /// DER-encoded leaf certificate. Owned (`'static` lifetime in
    /// the rustls type) so the bundle can be shared across the
    /// QUIC server config without lifetime juggling.
    pub cert_der: CertificateDer<'static>,
    /// PKCS#8 DER-encoded private key. Owned.
    pub key_pkcs8: PrivatePkcs8KeyDer<'static>,
    /// SHA-256 fingerprint of `cert_der`. 32 bytes, lowercase
    /// hex / base64-pad-stripped when emitted into the mDNS TXT
    /// record (see [`cert_sha256_b64`]).
    pub cert_sha256: [u8; 32],
}

impl Clone for CertBundle {
    fn clone(&self) -> Self {
        Self {
            cert_der: self.cert_der.clone(),
            key_pkcs8: self.key_pkcs8.clone_key(),
            cert_sha256: self.cert_sha256,
        }
    }
}

impl CertBundle {
    /// Generate a fresh ephemeral self-signed cert. The `subject_alt_names`
    /// is for human inspection only — the verifier matches by
    /// fingerprint, not by name, so the value is informational.
    ///
    /// Note that rcgen's default keypair is ECDSA P-256; we don't
    /// reuse the Ed25519 peer key here because some QUIC stacks
    /// (notably the default rustls + ring combination) don't support
    /// Ed25519 for TLS signing. The cert is purely a transport
    /// concern; trust still flows through the long-lived Ed25519
    /// peer identity in [`kcreate_collab`].
    pub fn generate(subject_alt_names: Vec<String>) -> Result<Self, TransportError> {
        let rcgen::CertifiedKey { cert, signing_key } =
            rcgen::generate_simple_self_signed(subject_alt_names)
                .map_err(|e| TransportError::Cert(format!("rcgen failed: {e}")))?;
        let cert_der_bytes = cert.der().to_vec();
        let mut hasher = Sha256::new();
        hasher.update(&cert_der_bytes);
        let cert_sha256: [u8; 32] = hasher.finalize().into();
        let cert_der: CertificateDer<'static> = CertificateDer::from(cert_der_bytes);
        let key_pkcs8 = PrivatePkcs8KeyDer::from(signing_key.serialize_der());
        Ok(Self {
            cert_der,
            key_pkcs8,
            cert_sha256,
        })
    }

    /// Base64-encoded (no padding) SHA-256 fingerprint, the form
    /// the mDNS TXT record carries. Same encoding used by
    /// [`kcreate_collab::peer::PeerFingerprint`] so the two
    /// fingerprints render the same way in the UI.
    #[must_use]
    pub fn cert_fingerprint_b64(&self) -> String {
        cert_sha256_b64(&self.cert_sha256)
    }
}

/// Encode a 32-byte SHA-256 fingerprint as an unpadded base64
/// string — the wire form used in mDNS TXT records.
#[must_use]
pub fn cert_sha256_b64(fingerprint: &[u8; 32]) -> String {
    base64::engine::general_purpose::STANDARD_NO_PAD.encode(fingerprint)
}

/// Decode a base64 (with or without padding) string back into a 32-byte
/// SHA-256 fingerprint. Returns `None` if the input is malformed or
/// the wrong length.
#[must_use]
pub fn cert_sha256_from_b64(input: &str) -> Option<[u8; 32]> {
    // Try the canonical no-pad first, then fall back to padded for
    // robustness against implementations that pad.
    let bytes = base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(input.trim_end_matches('='))
        .or_else(|_| base64::engine::general_purpose::STANDARD.decode(input))
        .ok()?;
    if bytes.len() != 32 {
        return None;
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Some(out)
}

/// rustls verifier that accepts the presented leaf cert if and only
/// if its DER encoding's SHA-256 matches `expected_fingerprint`.
///
/// This deliberately ignores chain validation, hostname matching, and
/// the system trust store — the trust decision was already made when
/// we observed the fingerprint over mDNS (or were told it by an
/// out-of-band channel) and we have no PKI in this design.
pub struct PinnedFingerprintVerifier {
    expected: [u8; 32],
    crypto_provider: Arc<CryptoProvider>,
}

impl std::fmt::Debug for PinnedFingerprintVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PinnedFingerprintVerifier")
            .field("expected_fingerprint", &cert_sha256_b64(&self.expected))
            .finish()
    }
}

impl PinnedFingerprintVerifier {
    /// Construct a verifier that pins to `expected_fingerprint`.
    pub fn new(expected: [u8; 32], crypto_provider: Arc<CryptoProvider>) -> Arc<Self> {
        Arc::new(Self {
            expected,
            crypto_provider,
        })
    }
}

impl ServerCertVerifier for PinnedFingerprintVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        let mut hasher = Sha256::new();
        hasher.update(end_entity.as_ref());
        let actual: [u8; 32] = hasher.finalize().into();
        // Constant-time comparison so a malicious peer can't gauge
        // how close their forged cert is to a legitimate one. The
        // overhead is negligible relative to the TLS handshake.
        if !constant_time_eq(&actual, &self.expected) {
            return Err(TlsError::General(format!(
                "TLS certificate fingerprint mismatch (expected {}, got {})",
                cert_sha256_b64(&self.expected),
                cert_sha256_b64(&actual),
            )));
        }
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.crypto_provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.crypto_provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.crypto_provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// Constant-time comparison for two 32-byte slices. Avoids subtle
/// side-channel attacks where the time to reject correlates with
/// the position of the first differing byte.
fn constant_time_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    let mut diff = 0u8;
    for i in 0..32 {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustls::pki_types::IpAddr;

    fn provider() -> Arc<CryptoProvider> {
        Arc::new(rustls::crypto::ring::default_provider())
    }

    #[test]
    fn generated_bundle_has_consistent_fingerprint() {
        let bundle = CertBundle::generate(vec!["kcreate-test".into()]).expect("generate");
        // Recompute SHA-256 of the cert DER and compare against the
        // stored fingerprint.
        let mut hasher = Sha256::new();
        hasher.update(bundle.cert_der.as_ref());
        let computed: [u8; 32] = hasher.finalize().into();
        assert_eq!(computed, bundle.cert_sha256);
        // base64 round-trip is lossless.
        let encoded = bundle.cert_fingerprint_b64();
        let decoded = cert_sha256_from_b64(&encoded).expect("decode");
        assert_eq!(decoded, bundle.cert_sha256);
    }

    #[test]
    fn verifier_accepts_matching_fingerprint() {
        let bundle = CertBundle::generate(vec!["kcreate-test".into()]).expect("generate");
        let verifier = PinnedFingerprintVerifier::new(bundle.cert_sha256, provider());
        let result = verifier.verify_server_cert(
            &bundle.cert_der,
            &[],
            &ServerName::IpAddress(IpAddr::try_from("127.0.0.1").unwrap()),
            &[],
            UnixTime::now(),
        );
        assert!(result.is_ok(), "valid cert must verify: {result:?}");
    }

    #[test]
    fn verifier_rejects_mismatched_fingerprint() {
        let bundle_a = CertBundle::generate(vec!["a".into()]).expect("generate a");
        let bundle_b = CertBundle::generate(vec!["b".into()]).expect("generate b");
        assert_ne!(
            bundle_a.cert_sha256, bundle_b.cert_sha256,
            "two fresh certs must have different fingerprints"
        );
        // Pin to A's fingerprint but present B's cert — must reject.
        let verifier = PinnedFingerprintVerifier::new(bundle_a.cert_sha256, provider());
        let result = verifier.verify_server_cert(
            &bundle_b.cert_der,
            &[],
            &ServerName::IpAddress(IpAddr::try_from("127.0.0.1").unwrap()),
            &[],
            UnixTime::now(),
        );
        let err = result.expect_err("must reject mismatch");
        assert!(
            err.to_string().contains("fingerprint mismatch"),
            "error must mention fingerprint mismatch: {err}"
        );
    }

    #[test]
    fn b64_decode_rejects_wrong_length() {
        assert!(cert_sha256_from_b64("aGVsbG8").is_none()); // too short
        assert!(cert_sha256_from_b64("!!!not-base64!!!").is_none());
    }
}
