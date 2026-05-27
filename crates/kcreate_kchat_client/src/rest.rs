//! Low-level REST wrapper around [`reqwest::Client`].
//!
//! All higher-level methods on [`crate::client::KChatBackendClient`]
//! go through [`RestClient`] for:
//!
//!   - TLS-strict scheme enforcement (refuses `http://` outside test
//!     mode).
//!   - Per-request timeout and connection-pool config.
//!   - `Authorization: Bearer <token>` injection from
//!     [`TokenStore`].
//!   - Transparent 401-then-refresh-then-retry.
//!   - 429 retry with `Retry-After` honour, bounded.
//!   - JSON decode of typed responses, including the backend's
//!     `{"code","message"}` error envelope.

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use reqwest::{Method, StatusCode};
use serde::{de::DeserializeOwned, Serialize};
use tokio::time::sleep;
use url::Url;

use crate::auth::TokenStore;
use crate::error::ClientError;
use crate::protocol::{
    error_code, BackendErrorBody, RefreshRequest, RefreshResponse, PROTOCOL_VERSION,
    PROTOCOL_VERSION_HEADER, USER_AGENT_HEADER_VALUE,
};

/// Per-request timeout. 30s is generous for any single REST call
/// — only `POST /api/v1/auth/login` has any chance of getting
/// close (TOTP rate limiters can take 2-3s in the wild).
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Bounded retry budget for HTTP 429 responses. The client honours
/// `Retry-After` when present, clipped to a sane max.
pub const MAX_RATE_LIMIT_RETRIES: u32 = 3;

/// Cap on the `Retry-After` interpretation so a misbehaving backend
/// can't pin a client forever on a single request.
pub const MAX_RATE_LIMIT_BACKOFF: Duration = Duration::from_secs(10);

/// Configuration toggles for the REST wrapper. The bridge always
/// constructs with [`RestClientConfig::production`]; tests use
/// [`RestClientConfig::for_tests`] which enables the
/// `allow_http_for_tests` escape hatch needed by the axum fixture
/// (which binds 127.0.0.1 plain-HTTP).
#[derive(Debug, Clone)]
pub struct RestClientConfig {
    /// Base URL of the backend (e.g. `https://kchat.example.com`).
    pub base_url: Url,
    /// Per-request timeout.
    pub request_timeout: Duration,
    /// If true, the client accepts `http://` base URLs. Production
    /// builds set this to `false`; the test fixture sets it to
    /// `true` so it can bind to `127.0.0.1` without provisioning
    /// a TLS cert.
    pub allow_http_for_tests: bool,
}

impl RestClientConfig {
    /// Production config. Validates the URL eagerly so the bridge
    /// surfaces "you typed the URL wrong" before any traffic.
    pub fn production(base_url: &str) -> Result<Self, ClientError> {
        let url = Url::parse(base_url).map_err(|e| ClientError::InvalidBaseUrl {
            url: base_url.into(),
            message: e.to_string(),
        })?;
        if url.scheme() != "https" {
            return Err(ClientError::InsecureTransport {
                url: base_url.into(),
            });
        }
        Ok(Self {
            base_url: url,
            request_timeout: REQUEST_TIMEOUT,
            allow_http_for_tests: false,
        })
    }

    /// Test-fixture config. Only used by the axum-backed test
    /// suite — the production bridge never sets this.
    #[doc(hidden)]
    pub fn for_tests(base_url: &str) -> Result<Self, ClientError> {
        let url = Url::parse(base_url).map_err(|e| ClientError::InvalidBaseUrl {
            url: base_url.into(),
            message: e.to_string(),
        })?;
        if url.scheme() != "https" && url.scheme() != "http" {
            return Err(ClientError::InvalidBaseUrl {
                url: base_url.into(),
                message: format!("unsupported scheme `{}`", url.scheme()),
            });
        }
        Ok(Self {
            base_url: url,
            request_timeout: Duration::from_secs(5),
            allow_http_for_tests: true,
        })
    }
}

/// Low-level REST wrapper. Owns the `reqwest::Client` and the
/// [`TokenStore`]. Methods take typed request + response bodies
/// and return [`ClientError`] on every failure mode the bridge
/// needs to distinguish.
#[derive(Debug, Clone)]
pub struct RestClient {
    http: reqwest::Client,
    config: RestClientConfig,
    tokens: Arc<TokenStore>,
}

impl RestClient {
    /// Build a fresh REST wrapper. The reqwest client is built
    /// with rustls-tls and no native fallback (we deliberately do
    /// **not** want OpenSSL anywhere in the editing-path closure).
    pub fn new(config: RestClientConfig, tokens: Arc<TokenStore>) -> Result<Self, ClientError> {
        if config.base_url.scheme() == "http" && !config.allow_http_for_tests {
            return Err(ClientError::InsecureTransport {
                url: config.base_url.to_string(),
            });
        }
        let http = reqwest::Client::builder()
            .timeout(config.request_timeout)
            .user_agent(USER_AGENT_HEADER_VALUE)
            // The collab transport already pins rustls; the REST
            // client uses rustls too so the workspace ships with
            // exactly one TLS backend.
            .build()
            .map_err(|e| ClientError::Transport(format!("build reqwest client: {e}")))?;
        Ok(Self {
            http,
            config,
            tokens,
        })
    }

    /// Token store, exposed for the bridge's `status` introspection.
    pub fn tokens(&self) -> &Arc<TokenStore> {
        &self.tokens
    }

    /// Configured base URL.
    pub fn base_url(&self) -> &Url {
        &self.config.base_url
    }

    /// Resolve a relative path against the configured base URL.
    /// Returns an `InvalidBaseUrl` error on join failure (which
    /// should be impossible for well-formed callers — every
    /// production path is a string literal).
    fn join(&self, path: &str) -> Result<Url, ClientError> {
        self.config.base_url.join(path).map_err(|e| {
            ClientError::InvalidBaseUrl {
                url: format!("{} + {}", self.config.base_url, path),
                message: e.to_string(),
            }
        })
    }

    /// Build a `reqwest::RequestBuilder` with the protocol-version
    /// header pre-set. Used as the start of every outgoing
    /// request.
    fn builder(&self, method: Method, path: &str) -> Result<reqwest::RequestBuilder, ClientError> {
        let url = self.join(path)?;
        let proto_version =
            HeaderValue::from_str(&PROTOCOL_VERSION.to_string()).expect("PROTOCOL_VERSION ASCII");
        let mut headers = HeaderMap::new();
        headers.insert(PROTOCOL_VERSION_HEADER, proto_version);
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        Ok(self.http.request(method, url).headers(headers))
    }

    /// Issue an unauthenticated request. Used for `auth/login`
    /// and `auth/refresh`.
    pub async fn request_unauthed<P, R>(
        &self,
        method: Method,
        path: &str,
        body: Option<&P>,
    ) -> Result<R, ClientError>
    where
        P: Serialize + ?Sized + Sync,
        R: DeserializeOwned + Send,
    {
        self.request_unauthed_typed(method, path, body, false).await
    }

    async fn request_unauthed_typed<P, R>(
        &self,
        method: Method,
        path: &str,
        body: Option<&P>,
        is_refresh: bool,
    ) -> Result<R, ClientError>
    where
        P: Serialize + ?Sized + Sync,
        R: DeserializeOwned + Send,
    {
        let mut attempts: u32 = 0;
        loop {
            let mut builder = self.builder(method.clone(), path)?;
            if let Some(body) = body {
                builder = builder.json(body);
            }
            let resp = match builder.send().await {
                Ok(r) => r,
                Err(e) => {
                    return Err(map_transport_error(e, path));
                }
            };
            let status = resp.status();
            if status == StatusCode::TOO_MANY_REQUESTS && attempts < MAX_RATE_LIMIT_RETRIES {
                attempts += 1;
                let wait = parse_retry_after(&resp).unwrap_or_else(|| {
                    Duration::from_millis(200u64 * 2u64.pow(attempts.min(6)))
                });
                let wait = wait.min(MAX_RATE_LIMIT_BACKOFF);
                tracing::debug!(
                    target: "kcreate_kchat_client::rest",
                    "429 on {path}, retrying after {wait:?} (attempt {attempts})",
                );
                sleep(wait).await;
                continue;
            }
            return self.decode_response(resp, path, is_refresh).await;
        }
    }

    /// Issue an authenticated request. Attaches the cached
    /// bearer token, pre-emptively refreshes if near-expiry, and
    /// transparently retries once on 401 after a fresh refresh.
    pub async fn request_authed<P, R>(
        &self,
        method: Method,
        path: &str,
        body: Option<&P>,
    ) -> Result<R, ClientError>
    where
        P: Serialize + ?Sized + Sync,
        R: DeserializeOwned + Send,
    {
        // Pre-emptive refresh.
        if let Some(tokens) = self.tokens.snapshot() {
            if tokens.needs_preemptive_refresh(Utc::now()) {
                let _ = self.refresh().await;
            }
        }

        let first_attempt = self.send_authed(method.clone(), path, body).await;
        match first_attempt {
            Err(ClientError::NotAuthenticated) => Err(ClientError::NotAuthenticated),
            Err(e) if is_auth_failure(&e) => {
                // 401 from the backend — try refresh, then retry
                // once.
                self.refresh().await?;
                self.send_authed(method, path, body).await
            }
            other => other,
        }
    }

    async fn send_authed<P, R>(
        &self,
        method: Method,
        path: &str,
        body: Option<&P>,
    ) -> Result<R, ClientError>
    where
        P: Serialize + ?Sized + Sync,
        R: DeserializeOwned + Send,
    {
        let tokens = self
            .tokens
            .snapshot()
            .ok_or(ClientError::NotAuthenticated)?;
        let mut attempts: u32 = 0;
        loop {
            let mut builder = self.builder(method.clone(), path)?;
            let auth_header = HeaderValue::from_str(&format!("Bearer {}", tokens.access_token))
                .map_err(|e| ClientError::Transport(format!("auth header: {e}")))?;
            builder = builder.header(AUTHORIZATION, auth_header);
            if let Some(body) = body {
                builder = builder.json(body);
            }
            let resp = match builder.send().await {
                Ok(r) => r,
                Err(e) => return Err(map_transport_error(e, path)),
            };
            let status = resp.status();
            if status == StatusCode::TOO_MANY_REQUESTS && attempts < MAX_RATE_LIMIT_RETRIES {
                attempts += 1;
                let wait = parse_retry_after(&resp)
                    .unwrap_or_else(|| Duration::from_millis(200u64 * 2u64.pow(attempts.min(6))))
                    .min(MAX_RATE_LIMIT_BACKOFF);
                sleep(wait).await;
                continue;
            }
            return self.decode_response(resp, path, false).await;
        }
    }

    async fn refresh(&self) -> Result<(), ClientError> {
        let tokens = self
            .tokens
            .snapshot()
            .ok_or(ClientError::NotAuthenticated)?;
        let req = RefreshRequest {
            refresh_token: tokens.refresh_token.clone(),
        };
        let resp: RefreshResponse = self
            .request_unauthed_typed(Method::POST, "/api/v1/auth/refresh", Some(&req), true)
            .await?;
        if self
            .tokens
            .apply_refresh(resp, Utc::now())
            .is_none()
        {
            return Err(ClientError::NotAuthenticated);
        }
        Ok(())
    }

    /// Decode a response body, mapping non-2xx statuses to typed
    /// `ClientError` variants. `is_refresh` shifts the 401 mapping
    /// to `RefreshExpired` (so the renderer prompts for a full
    /// re-login instead of trying another refresh round trip).
    async fn decode_response<R>(
        &self,
        resp: reqwest::Response,
        path: &str,
        is_refresh: bool,
    ) -> Result<R, ClientError>
    where
        R: DeserializeOwned,
    {
        let status = resp.status();
        if status.is_success() {
            let bytes = resp
                .bytes()
                .await
                .map_err(|e| ClientError::Transport(format!("read body: {e}")))?;
            return serde_json::from_slice::<R>(&bytes).map_err(|e| {
                ClientError::Deserialization {
                    path: path.into(),
                    message: e.to_string(),
                }
            });
        }

        // Drain body so we can interpret backend error envelopes.
        let body_bytes = resp.bytes().await.unwrap_or_default();
        let body: Option<BackendErrorBody> = if body_bytes.is_empty() {
            None
        } else {
            serde_json::from_slice(&body_bytes).ok()
        };

        Err(classify_failure(status, body, path, is_refresh))
    }
}

/// True if an error should trigger the auth-retry path. Includes
/// the 401-with-no-body case so a misbehaving backend still gives
/// us a chance to refresh.
fn is_auth_failure(err: &ClientError) -> bool {
    matches!(
        err,
        ClientError::InvalidCredentials { .. }
            | ClientError::Backend {
                status: 401, ..
            }
    )
}

/// Map status + parsed body into a `ClientError` variant. Pulled
/// into a free function so unit tests can exercise the table
/// without spinning up reqwest.
pub(crate) fn classify_failure(
    status: StatusCode,
    body: Option<BackendErrorBody>,
    path: &str,
    is_refresh: bool,
) -> ClientError {
    if status == StatusCode::TOO_MANY_REQUESTS {
        return ClientError::RateLimited;
    }
    if status == StatusCode::UNAUTHORIZED {
        let message = body.as_ref().map_or_else(
            || String::from("unauthorized"),
            |b| b.message.clone(),
        );
        if is_refresh {
            return ClientError::RefreshExpired { message };
        }
        return ClientError::InvalidCredentials { message };
    }
    if status == StatusCode::FORBIDDEN {
        let message = body.as_ref().map_or_else(
            || String::from("permission denied"),
            |b| b.message.clone(),
        );
        return ClientError::PermissionDenied { message };
    }
    if status == StatusCode::NOT_FOUND || status == StatusCode::NOT_IMPLEMENTED {
        let is_attestation_path = path.ends_with("/attestation");
        let code = body.as_ref().map(|b| b.code.as_str());
        if is_attestation_path
            || code == Some(error_code::ATTESTATION_NOT_PROVISIONED)
        {
            let message = body.as_ref().map_or_else(
                || String::from("attestation endpoint not yet provisioned by backend"),
                |b| b.message.clone(),
            );
            return ClientError::AttestationEndpointNotProvisioned { message };
        }
        let message = body
            .as_ref()
            .map_or_else(|| format!("{path} not found"), |b| b.message.clone());
        return ClientError::NotFound { message };
    }
    if status.is_server_error() {
        let message = body.as_ref().map_or_else(
            || status.canonical_reason().unwrap_or("server error").into(),
            |b| b.message.clone(),
        );
        return ClientError::Server {
            status: status.as_u16(),
            message,
        };
    }
    if let Some(body) = body {
        return ClientError::Backend {
            status: status.as_u16(),
            body,
        };
    }
    ClientError::Backend {
        status: status.as_u16(),
        body: BackendErrorBody {
            code: format!("HTTP_{}", status.as_u16()),
            message: status.canonical_reason().unwrap_or("").into(),
            data: None,
        },
    }
}

fn map_transport_error(err: reqwest::Error, path: &str) -> ClientError {
    if err.is_timeout() {
        return ClientError::Timeout { path: path.into() };
    }
    if err.is_decode() {
        return ClientError::Deserialization {
            path: path.into(),
            message: err.to_string(),
        };
    }
    ClientError::Transport(err.to_string())
}

/// Parse a `Retry-After` header value into a `Duration`. Supports
/// the integer-seconds form only (the HTTP-date form is uncommon
/// for rate-limit responses and the cost of supporting it isn't
/// worth a chrono dep here).
fn parse_retry_after(resp: &reqwest::Response) -> Option<Duration> {
    let v = resp.headers().get(reqwest::header::RETRY_AFTER)?;
    let s = v.to_str().ok()?;
    s.trim().parse::<u64>().ok().map(Duration::from_secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_429_returns_rate_limited() {
        let err = classify_failure(StatusCode::TOO_MANY_REQUESTS, None, "/api/v1/x", false);
        assert!(matches!(err, ClientError::RateLimited));
    }

    #[test]
    fn classify_401_initial_returns_invalid_credentials() {
        let err = classify_failure(StatusCode::UNAUTHORIZED, None, "/api/v1/x", false);
        assert!(matches!(err, ClientError::InvalidCredentials { .. }));
    }

    #[test]
    fn classify_401_on_refresh_returns_refresh_expired() {
        let err = classify_failure(StatusCode::UNAUTHORIZED, None, "/api/v1/auth/refresh", true);
        assert!(matches!(err, ClientError::RefreshExpired { .. }));
    }

    #[test]
    fn classify_404_on_attestation_returns_endpoint_not_provisioned() {
        let err = classify_failure(
            StatusCode::NOT_FOUND,
            None,
            "/api/v1/communities/x/attestation",
            false,
        );
        assert!(matches!(
            err,
            ClientError::AttestationEndpointNotProvisioned { .. }
        ));
    }

    #[test]
    fn classify_404_outside_attestation_returns_not_found() {
        let err = classify_failure(
            StatusCode::NOT_FOUND,
            None,
            "/api/v1/communities/x",
            false,
        );
        assert!(matches!(err, ClientError::NotFound { .. }));
    }

    #[test]
    fn classify_404_with_attestation_code_returns_endpoint_not_provisioned() {
        let body = BackendErrorBody {
            code: error_code::ATTESTATION_NOT_PROVISIONED.into(),
            message: "wait for backend update".into(),
            data: None,
        };
        let err = classify_failure(
            StatusCode::NOT_IMPLEMENTED,
            Some(body),
            "/api/v1/communities/x/some-other-path",
            false,
        );
        assert!(matches!(
            err,
            ClientError::AttestationEndpointNotProvisioned { .. }
        ));
    }

    #[test]
    fn classify_500_returns_server_error() {
        let err = classify_failure(StatusCode::INTERNAL_SERVER_ERROR, None, "/x", false);
        assert!(matches!(err, ClientError::Server { status: 500, .. }));
    }

    #[test]
    fn classify_unknown_4xx_returns_backend_envelope() {
        let body = BackendErrorBody {
            code: "WEIRD".into(),
            message: "huh".into(),
            data: None,
        };
        let err = classify_failure(StatusCode::IM_A_TEAPOT, Some(body), "/x", false);
        assert!(matches!(err, ClientError::Backend { status: 418, .. }));
    }

    #[test]
    fn rest_config_production_refuses_http() {
        let res = RestClientConfig::production("http://kchat.example.com");
        assert!(matches!(res, Err(ClientError::InsecureTransport { .. })));
    }

    #[test]
    fn rest_config_production_accepts_https() {
        let cfg = RestClientConfig::production("https://kchat.example.com").unwrap();
        assert_eq!(cfg.base_url.scheme(), "https");
        assert!(!cfg.allow_http_for_tests);
    }

    #[test]
    fn rest_config_for_tests_accepts_http() {
        let cfg = RestClientConfig::for_tests("http://127.0.0.1:12345").unwrap();
        assert!(cfg.allow_http_for_tests);
    }
}
