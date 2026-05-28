//! Typed error surface for the KChat backend REST client.
//!
//! Every public method on [`crate::client::KChatBackendClient`]
//! returns `Result<_, ClientError>`. The bridge maps these to
//! `KChatBackendBridgeError` variants on the N-API surface so the
//! renderer can disambiguate "your credentials are wrong" from
//! "the backend is unreachable" from "the attestation signature
//! mismatched" without parsing free-form error strings.

use crate::protocol::BackendErrorBody;

/// Top-level error variants returned by
/// [`crate::client::KChatBackendClient`] methods.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// Base URL the client was constructed with is not a valid
    /// URL or uses an unsupported scheme. The client refuses
    /// `http://` outside test mode (see [`Self::InsecureTransport`]).
    #[error("invalid KChat backend base URL `{url}`: {message}")]
    InvalidBaseUrl { url: String, message: String },

    /// The configured base URL uses `http://` but the client is
    /// running outside the test fixture. KCreate refuses to send
    /// credentials over plaintext HTTP — surface this so the user
    /// switches to HTTPS.
    #[error("KChat backend base URL `{url}` uses plaintext HTTP — KCreate refuses to send credentials over an insecure transport. Configure an `https://` URL or enable the test fixture mode.")]
    InsecureTransport { url: String },

    /// Could not reach the backend. Includes DNS failures,
    /// connection refused, TLS handshake failures, idle timeouts.
    #[error("could not reach KChat backend: {0}")]
    Transport(String),

    /// We exceeded the per-request timeout waiting for a response.
    #[error("KChat backend did not respond within timeout ({path})")]
    Timeout { path: String },

    /// Caller used the client without logging in first. Renderer
    /// must call `kchat_backend_connect` (or, in tests,
    /// [`KChatBackendClient::login`](crate::client::KChatBackendClient::login))
    /// before any other entry point.
    #[error("KChat backend client is not authenticated; call login() first")]
    NotAuthenticated,

    /// Login rejected the supplied credentials. The user must
    /// re-enter their password (or TOTP).
    #[error("KChat backend rejected credentials: {message}")]
    InvalidCredentials { message: String },

    /// Refresh token expired or was revoked. The user must log in
    /// again from scratch.
    #[error("KChat backend refresh token is no longer valid: {message}")]
    RefreshExpired { message: String },

    /// Caller does not have permission for this resource (e.g.
    /// not a member of the requested community).
    #[error("KChat backend denied access: {message}")]
    PermissionDenied { message: String },

    /// Resource not found (community / conversation / member id
    /// unknown).
    #[error("KChat backend resource not found: {message}")]
    NotFound { message: String },

    /// Backend has not yet shipped the attestation-signing endpoint.
    /// Renderer surfaces this distinctly so the user can either
    /// (a) wait for the backend update or (b) fall back to the
    /// dev-mint flow if the build was compiled with
    /// `kchat-dev-issuer`.
    #[error(
        "KChat backend has not provisioned the membership attestation endpoint yet: {message}"
    )]
    AttestationEndpointNotProvisioned { message: String },

    /// The backend returned a 429 and the client's bounded retry
    /// budget is exhausted. Surface as a typed error so the
    /// renderer can show "rate limited — try again later".
    #[error("KChat backend rate-limited the client and retries are exhausted")]
    RateLimited,

    /// A 5xx error from the backend.
    #[error("KChat backend internal error: {status} {message}")]
    Server { status: u16, message: String },

    /// Generic typed error from the backend's
    /// `{"code", "message"}` envelope — covers anything not
    /// already mapped to a more specific variant.
    #[error("KChat backend error: {} (status {status})", .body.code)]
    Backend { status: u16, body: BackendErrorBody },

    /// Could not serialise an outgoing request body.
    #[error("KChat backend request serialization failed: {0}")]
    Serialization(serde_json::Error),

    /// Could not deserialise a response body. Carries the path so
    /// debugging "what endpoint started returning malformed JSON"
    /// is one log line.
    #[error("KChat backend response from `{path}` could not be deserialised: {message}")]
    Deserialization { path: String, message: String },

    /// Bridging the response into a `KChatMembership` failed (the
    /// attestation did not verify locally, the binding mismatch
    /// failed, etc.).
    #[error("KChat membership attestation invalid: {0}")]
    AttestationInvalid(String),

    /// Backend refused an artifact upload because its declared
    /// kind / MIME isn't in the supported set
    /// (`UNSUPPORTED_ARTIFACT_KIND`). Surfaced separately from
    /// [`Self::Backend`] so the renderer can show a clear "this
    /// format isn't accepted by your community" message.
    #[error("KChat backend refused unsupported artifact kind: {message}")]
    ArtifactKindUnsupported { message: String },

    /// Backend refused an artifact upload because the bytes exceed
    /// its per-upload cap (`ARTIFACT_TOO_LARGE`). The renderer
    /// can use this to suggest a lower-resolution export preset
    /// instead of failing silently.
    #[error("KChat backend refused artifact: payload too large ({message})")]
    ArtifactTooLarge { message: String },
}

impl ClientError {
    /// True if the error indicates the client must reauthenticate.
    /// Renderer should clear cached tokens and prompt for password.
    #[must_use]
    pub const fn requires_reauth(&self) -> bool {
        matches!(
            self,
            Self::InvalidCredentials { .. } | Self::RefreshExpired { .. } | Self::NotAuthenticated
        )
    }

    /// True if the error is transient and the caller should retry
    /// after a short backoff (not built into the high-level client
    /// because retry semantics are caller-specific).
    #[must_use]
    pub const fn is_transient(&self) -> bool {
        matches!(
            self,
            Self::Transport(_) | Self::Timeout { .. } | Self::RateLimited | Self::Server { .. }
        )
    }
}

impl From<serde_json::Error> for ClientError {
    fn from(value: serde_json::Error) -> Self {
        Self::Serialization(value)
    }
}
