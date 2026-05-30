//! Phase 11 Block E Task 28 — certificate pinning for the KChat
//! backend REST client.
//!
//! When the bridge supplies a SHA-256 fingerprint of the backend's
//! leaf certificate, the client refuses to complete a TLS handshake
//! unless **both**:
//!
//!   1. The presented certificate chain validates against the
//!      Mozilla WebPKI root store (the same anchors `reqwest` uses
//!      by default), AND
//!   2. The leaf certificate's DER encoding hashes to exactly the
//!      pinned 32-byte SHA-256.
//!
//! Order matters: chain validation runs first so a forged
//! self-signed cert is rejected on a normal failure path (the
//! attacker doesn't even learn that pinning is enabled). The pin
//! check is the second layer — a defence-in-depth against a
//! mis-issued cert from a public CA (e.g. a compromised
//! intermediate, an unwitting trust-store mis-installation, or a
//! TLS-interceptor that has somehow obtained a cert chaining to a
//! WebPKI root).
//!
//! ## Usage
//!
//! The verifier is wrapped in a [`rustls::ClientConfig`] which is
//! handed to `reqwest::Client::builder().use_preconfigured_tls()`.
//! When no pin is configured, the REST client uses reqwest's
//! default rustls config (system roots, no pin) — this module is
//! not on the hot path for unpinned deployments.
//!
//! ## Error surface
//!
//! Both failure modes manifest as `rustls::Error::General(...)`
//! with a human-readable message. The REST wrapper converts these
//! into [`crate::ClientError::CertificatePinMismatch`] so the
//! bridge can surface a distinct UI prompt ("Certificate mismatch
//! — possible MITM. Contact your KChat administrator.") rather
//! than a generic transport failure.

use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::client::WebPkiServerVerifier;
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{
    ClientConfig, DigitallySignedStruct, Error as TlsError, RootCertStore, SignatureScheme,
};
use sha2::{Digest, Sha256};

use crate::error::ClientError;

/// Length of a SHA-256 digest in bytes (used to size the pinned
/// fingerprint buffer).
pub const PIN_SHA256_LEN: usize = 32;

/// Parse a hex-encoded SHA-256 fingerprint into a fixed-size byte
/// array. Accepts upper or lower case digits and is whitespace-
/// tolerant so users can paste `openssl x509 -fingerprint` output
/// directly (which inserts colons every byte) — those non-hex
/// separators are stripped before parsing.
pub(crate) fn parse_pin_hex(input: &str) -> Result<[u8; PIN_SHA256_LEN], ClientError> {
    let cleaned: String = input
        .chars()
        .filter(|c| !c.is_ascii_whitespace() && *c != ':')
        .collect();
    if cleaned.len() != PIN_SHA256_LEN * 2 {
        return Err(ClientError::InvalidPinnedCertificate {
            message: format!(
                "expected 64 hex digits (32-byte SHA-256), got {} non-separator chars",
                cleaned.len()
            ),
        });
    }
    let mut out = [0u8; PIN_SHA256_LEN];
    for (i, byte) in out.iter_mut().enumerate() {
        let hi = hex_nibble(cleaned.as_bytes()[i * 2])?;
        let lo = hex_nibble(cleaned.as_bytes()[i * 2 + 1])?;
        *byte = (hi << 4) | lo;
    }
    Ok(out)
}

fn hex_nibble(b: u8) -> Result<u8, ClientError> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(ClientError::InvalidPinnedCertificate {
            message: format!("not a hex digit: 0x{b:02x}"),
        }),
    }
}

/// Hex-encode a SHA-256 digest for inclusion in error messages.
/// Lower-case to match what `openssl x509 -fingerprint -sha256` and
/// most tools emit by default.
fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(nibble_to_hex(b >> 4));
        out.push(nibble_to_hex(b & 0x0f));
    }
    out
}

fn nibble_to_hex(n: u8) -> char {
    match n {
        0..=9 => (b'0' + n) as char,
        10..=15 => (b'a' + (n - 10)) as char,
        _ => '?',
    }
}

/// Constant-time byte-slice comparison. Avoids leaking partial-
/// match timing to an attacker positioned between client and
/// server. Equivalent to `subtle::ConstantTimeEq` but inlined so
/// the crate doesn't grow another dep.
fn constant_time_eq(a: &[u8; PIN_SHA256_LEN], b: &[u8; PIN_SHA256_LEN]) -> bool {
    let mut acc = 0u8;
    for i in 0..PIN_SHA256_LEN {
        acc |= a[i] ^ b[i];
    }
    acc == 0
}

/// rustls `ServerCertVerifier` that runs the standard WebPKI chain
/// validation and, on success, additionally checks the leaf cert's
/// SHA-256 fingerprint against a pinned value.
///
/// Construction goes through [`PinnedCertVerifier::new`] which
/// owns the WebPKI verifier; we delegate every TLS-signature
/// verification call straight to it so the underlying CryptoProvider
/// remains the source of truth for supported algorithms.
pub struct PinnedCertVerifier {
    expected_fingerprint: [u8; PIN_SHA256_LEN],
    /// The standard WebPKI verifier built from the supplied root
    /// store. Wrapped here so its lifetime matches ours.
    inner: Arc<WebPkiServerVerifier>,
}

impl std::fmt::Debug for PinnedCertVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PinnedCertVerifier")
            .field(
                "expected_fingerprint_sha256",
                &hex_encode(&self.expected_fingerprint),
            )
            .field("inner_verifier", &"WebPkiServerVerifier")
            .finish()
    }
}

impl PinnedCertVerifier {
    /// Build a verifier that pins to `expected_fingerprint` after
    /// validating against `root_store`. Returns
    /// `ClientError::InvalidPinnedCertificate` if rustls rejects
    /// the supplied root store (e.g. empty) at builder time.
    pub fn new(
        expected_fingerprint: [u8; PIN_SHA256_LEN],
        root_store: Arc<RootCertStore>,
    ) -> Result<Arc<Self>, ClientError> {
        let inner = WebPkiServerVerifier::builder(root_store).build().map_err(|e| {
            ClientError::InvalidPinnedCertificate {
                message: format!("failed to build WebPKI verifier: {e}"),
            }
        })?;
        Ok(Arc::new(Self {
            expected_fingerprint,
            inner,
        }))
    }
}

impl ServerCertVerifier for PinnedCertVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        // 1. Run standard WebPKI chain + hostname + validity
        //    period validation. If this fails, the error is the
        //    normal "untrusted/expired/wrong-name" surface — we
        //    don't reveal the pin even existed yet.
        self.inner
            .verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)?;

        // 2. Additionally, the leaf must hash to the pinned
        //    fingerprint. Constant-time comparison so an attacker
        //    can't shave nanoseconds off measuring how close a
        //    forged cert is to a legitimate one.
        let mut hasher = Sha256::new();
        hasher.update(end_entity.as_ref());
        let actual: [u8; PIN_SHA256_LEN] = hasher.finalize().into();
        if !constant_time_eq(&actual, &self.expected_fingerprint) {
            return Err(TlsError::General(format!(
                "KCREATE_PIN_MISMATCH: expected SHA-256 {}, got {}",
                hex_encode(&self.expected_fingerprint),
                hex_encode(&actual),
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
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

/// Build a `rustls::ClientConfig` that pins the leaf certificate
/// to `expected_fingerprint`. The config uses the Mozilla WebPKI
/// root store (from `webpki-roots`) as its trust anchor and the
/// process-default `CryptoProvider` for ciphersuite negotiation.
///
/// Returned via `Arc` because `reqwest::ClientBuilder::use_preconfigured_tls`
/// internally clones the value but rustls makes this cheap.
pub(crate) fn build_pinned_tls_config(
    expected_fingerprint: [u8; PIN_SHA256_LEN],
) -> Result<ClientConfig, ClientError> {
    // Ensure a default provider is installed exactly once for this
    // process. Idempotent — if another component already installed
    // one (or installed an explicit ring/aws-lc provider), we just
    // continue with whatever is there.
    if CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }

    // Build a fresh root store from the bundled Mozilla CA list.
    let mut root_store = RootCertStore::empty();
    root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let root_store = Arc::new(root_store);

    let verifier = PinnedCertVerifier::new(expected_fingerprint, root_store)?;

    // `with_safe_default_protocol_versions` ⇒ TLS1.2 + TLS1.3.
    let config = ClientConfig::builder()
        .dangerous() // required to wire in a custom verifier
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_pin_hex_accepts_64_hex_chars() {
        let s = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let parsed = parse_pin_hex(s).unwrap();
        assert_eq!(parsed[0], 0x00);
        assert_eq!(parsed[15], 0xff);
        assert_eq!(parsed[16], 0x00);
        assert_eq!(parsed[31], 0xff);
    }

    #[test]
    fn parse_pin_hex_strips_colons() {
        // `openssl x509 -fingerprint -sha256` output format.
        let s = "00:11:22:33:44:55:66:77:88:99:AA:BB:CC:DD:EE:FF:\
                 00:11:22:33:44:55:66:77:88:99:AA:BB:CC:DD:EE:FF";
        let parsed = parse_pin_hex(s).unwrap();
        assert_eq!(parsed[0], 0x00);
        assert_eq!(parsed[31], 0xff);
    }

    #[test]
    fn parse_pin_hex_rejects_wrong_length() {
        match parse_pin_hex("deadbeef") {
            Err(ClientError::InvalidPinnedCertificate { message }) => {
                assert!(message.contains("64 hex digits"), "{message}");
            }
            other => panic!("expected InvalidPinnedCertificate, got {other:?}"),
        }
    }

    #[test]
    fn parse_pin_hex_rejects_non_hex_chars() {
        let s = "zz112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        match parse_pin_hex(s) {
            Err(ClientError::InvalidPinnedCertificate { message }) => {
                assert!(message.contains("not a hex digit"), "{message}");
            }
            other => panic!("expected InvalidPinnedCertificate, got {other:?}"),
        }
    }

    #[test]
    fn constant_time_eq_matches_naive() {
        let a = [0xAA; PIN_SHA256_LEN];
        let mut b = [0xAA; PIN_SHA256_LEN];
        assert!(constant_time_eq(&a, &b));
        b[17] ^= 0x01;
        assert!(!constant_time_eq(&a, &b));
        let zero = [0u8; PIN_SHA256_LEN];
        let one = [0u8; PIN_SHA256_LEN];
        assert!(constant_time_eq(&zero, &one));
    }

    #[test]
    fn build_pinned_tls_config_returns_usable_client_config() {
        // We don't assert anything about the rustls config struct
        // internals — those are private. Just verify that the
        // builder succeeds with a valid pin and a fresh process-
        // wide provider, which exercises the WebPKI verifier build
        // path against the bundled Mozilla roots.
        let pin = [0x01u8; PIN_SHA256_LEN];
        let _config = build_pinned_tls_config(pin).expect("config builds");
    }

    /// The pinned verifier rejects empty root stores at construction
    /// time so a misconfigured deployment can't silently fall through
    /// to "no chain validation, only pin" — the request fails closed
    /// at the point we'd otherwise lose half of the defence-in-depth.
    #[test]
    fn pinned_verifier_rejects_empty_root_store() {
        let pin = [0x02u8; PIN_SHA256_LEN];
        let empty_roots = Arc::new(rustls::RootCertStore::empty());
        match PinnedCertVerifier::new(pin, empty_roots) {
            Err(ClientError::InvalidPinnedCertificate { message }) => {
                assert!(
                    message.contains("WebPKI"),
                    "expected WebPKI builder error, got: {message}"
                );
            }
            other => panic!("expected InvalidPinnedCertificate, got {other:?}"),
        }
    }

    /// Hex encoding round-trips through `parse_pin_hex`. Quick
    /// regression coverage for the nibble/encoder pair so changes
    /// to one side don't silently disagree.
    #[test]
    fn hex_encode_round_trip() {
        let bytes: [u8; PIN_SHA256_LEN] = [
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d,
            0x0e, 0x0f, 0xf0, 0xe1, 0xd2, 0xc3, 0xb4, 0xa5, 0x96, 0x87, 0x78, 0x69, 0x5a, 0x4b,
            0x3c, 0x2d, 0x1e, 0xff,
        ];
        let s = hex_encode(&bytes);
        assert_eq!(s.len(), 64);
        let parsed = parse_pin_hex(&s).unwrap();
        assert_eq!(parsed, bytes);
    }
}
