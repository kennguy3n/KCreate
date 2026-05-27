//! Typed error surface for the KChat Desktop client.

use crate::protocol::RpcError;

/// Top-level error variants returned by [`crate::KChatDesktopClient`]
/// methods.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// Could not connect to the local KChat Desktop socket within
    /// the configured timeout. The renderer surfaces this as
    /// "uney-chat-desktop is not running" with a "retry" button.
    #[error("could not connect to KChat Desktop at {path}: {source}")]
    Connect {
        path: String,
        #[source]
        source: std::io::Error,
    },

    /// Underlying I/O failure on an open connection.
    #[error("KChat Desktop IPC I/O: {0}")]
    Io(#[from] std::io::Error),

    /// We exceeded the per-request timeout waiting for a response.
    #[error("KChat Desktop did not respond within timeout (method={0})")]
    Timeout(String),

    /// Connection closed unexpectedly while a request was in flight.
    #[error("KChat Desktop connection closed unexpectedly")]
    Disconnected,

    /// Server returned a JSON-RPC error.
    #[error("KChat Desktop returned RPC error {}: {}", .0.code, .0.message)]
    Rpc(RpcError),

    /// Protocol-level invariant violation (unframed payload, both
    /// result+error, etc.). Treat as fatal — the transport tears
    /// the connection down.
    #[error("KChat Desktop protocol error: {0}")]
    Protocol(String),

    /// Could not serialise an outgoing request body.
    #[error("KChat Desktop request serialization failed: {0}")]
    Serialization(serde_json::Error),

    /// Could not deserialise a response body.
    #[error("KChat Desktop response deserialization failed: {0}")]
    Deserialization(serde_json::Error),

    /// Caller attempted to use the client before connecting.
    #[error("KChat Desktop client is not connected; call connect() first")]
    NotConnected,

    /// Bridging the response into a `KChatMembership` failed (the
    /// attestation did not verify locally, the binding mismatch
    /// failed, etc.).
    #[error("KChat Desktop membership attestation invalid: {0}")]
    AttestationInvalid(String),
}

impl ClientError {
    /// True if the error indicates the connection is no longer
    /// usable. Caller should drop the client and reconnect.
    #[must_use]
    pub fn is_fatal(&self) -> bool {
        matches!(
            self,
            Self::Connect { .. } | Self::Disconnected | Self::Io(_) | Self::Protocol(_)
        )
    }
}
