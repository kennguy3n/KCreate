//! Error type for the LAN transport.

use kcreate_collab::SessionError;
use thiserror::Error;

/// Errors surfaced by the transport. Distinct from
/// [`kcreate_collab::SessionError`] because anything that touches the
/// network has an additional failure mode the protocol layer does not.
#[derive(Debug, Error)]
pub enum TransportError {
    /// The transport failed to bind a UDP socket for QUIC. This is
    /// the only fatal startup error — without it neither the
    /// transport nor mDNS can do anything useful.
    #[error("failed to bind QUIC endpoint on {addr}: {source}")]
    Bind {
        addr: std::net::SocketAddr,
        #[source]
        source: std::io::Error,
    },

    /// Generic I/O error from quinn or tokio.
    #[error("transport I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// QUIC connect / accept failure (handshake error, version
    /// mismatch, cert verification rejected, …). The displayed
    /// message comes from quinn's own `Display` impl.
    #[error("QUIC connection error: {0}")]
    Quic(String),

    /// Rustls configuration failure — only fires at startup if the
    /// crypto provider can't be initialised or rcgen produces a cert
    /// the rustls builder rejects.
    #[error("TLS setup failed: {0}")]
    Tls(String),

    /// Failure generating the ephemeral self-signed certificate.
    /// Distinct from `Tls` because the underlying error is from
    /// rcgen, not rustls.
    #[error("certificate generation failed: {0}")]
    Cert(String),

    /// mDNS responder / browser failure. Discovery is best-effort —
    /// the host degrades gracefully to "explicit dial only" if mDNS
    /// is unavailable.
    #[error("mDNS error: {0}")]
    Mdns(String),

    /// A frame exceeded [`crate::wire::MAX_FRAME_BYTES`]. Treated as
    /// a hard error and the offending stream is reset.
    #[error("frame size {size} exceeds maximum {max}")]
    FrameTooLarge { size: usize, max: usize },

    /// A frame was malformed (truncated, non-JSON, …). The transport
    /// closes the connection on this; the application layer's own
    /// nonce / signature checks would catch most cases of corruption
    /// too, but we fail fast.
    #[error("malformed wire frame: {0}")]
    Malformed(String),

    /// The peer's certificate did not match the SHA-256 fingerprint
    /// we pinned (from the mDNS TXT record). Treated as a hostile
    /// MITM attempt and the connection is dropped.
    #[error("TLS certificate fingerprint mismatch — expected {expected}, got {actual}")]
    CertFingerprintMismatch { expected: String, actual: String },

    /// The peer presented a TXT record we can't parse, or
    /// advertised a `proto_version` we don't speak.
    #[error("unsupported peer advertisement: {0}")]
    UnsupportedAdvertisement(String),

    /// Bubble-up from the underlying protocol layer (sig mismatch,
    /// replay, untrusted peer, …). Most of these are already handled
    /// before they reach this layer; this variant exists so the host
    /// can surface them through the same channel.
    #[error("collab session error: {0}")]
    Session(#[from] SessionError),

    /// The host actor is no longer running (e.g. after `shutdown`).
    #[error("transport has been shut down")]
    Shutdown,
}

impl From<quinn::ConnectionError> for TransportError {
    fn from(value: quinn::ConnectionError) -> Self {
        Self::Quic(value.to_string())
    }
}

impl From<quinn::ConnectError> for TransportError {
    fn from(value: quinn::ConnectError) -> Self {
        Self::Quic(value.to_string())
    }
}

impl From<quinn::ReadError> for TransportError {
    fn from(value: quinn::ReadError) -> Self {
        Self::Quic(value.to_string())
    }
}

impl From<quinn::ReadExactError> for TransportError {
    fn from(value: quinn::ReadExactError) -> Self {
        Self::Quic(value.to_string())
    }
}

impl From<quinn::WriteError> for TransportError {
    fn from(value: quinn::WriteError) -> Self {
        Self::Quic(value.to_string())
    }
}
