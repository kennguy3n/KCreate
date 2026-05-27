//! High-level KChat backend REST client.
//!
//! Sits on top of [`crate::rest::RestClient`] and exposes one
//! typed method per REST endpoint. The bridge layer
//! (`kcreate_bridge::kchat_backend`) instantiates a single
//! [`KChatBackendClient`] per process and re-uses it across
//! N-API calls.
//!
//! The client is `Send + Sync` and clone-cheap (`reqwest::Client`
//! is `Arc`-shared internally and the token store wraps a
//! `parking_lot::RwLock`). All methods are `async` — the bridge
//! runs them on a single multi-thread Tokio runtime kept alive
//! for the lifetime of the bridge module.

use std::sync::Arc;

use chrono::Utc;
use reqwest::Method;

use crate::auth::{TokenSet, TokenStore};
use crate::error::ClientError;
use crate::protocol::{
    AttestationRequest, CommunitiesListResponse, CommunityEventsResponse, ConversationsListResponse,
    KChatCommunity, KChatCommunityMember, KChatConversation, KChatIdentity, LoginRequest,
    LoginResponse, MembersListResponse, MembershipAttestation, PostMessageRequest,
    PostMessageResponse,
};
use crate::rest::{RestClient, RestClientConfig};

/// High-level KChat backend REST client.
#[derive(Debug, Clone)]
pub struct KChatBackendClient {
    rest: RestClient,
}

impl KChatBackendClient {
    /// Build a production client. `base_url` must be `https://`.
    pub fn new(base_url: &str) -> Result<Self, ClientError> {
        let tokens = Arc::new(TokenStore::new());
        let rest = RestClient::new(RestClientConfig::production(base_url)?, tokens)?;
        Ok(Self { rest })
    }

    /// Build a test client allowed to talk to `http://` endpoints.
    /// Used by the axum-backed fixture tests only.
    #[doc(hidden)]
    pub fn new_for_tests(base_url: &str) -> Result<Self, ClientError> {
        let tokens = Arc::new(TokenStore::new());
        let rest = RestClient::new(RestClientConfig::for_tests(base_url)?, tokens)?;
        Ok(Self { rest })
    }

    /// Borrow the token store. Used by the bridge to surface
    /// "are we logged in?" + cached identity through
    /// `kchat_backend_status` without an extra round trip.
    pub fn tokens(&self) -> &Arc<TokenStore> {
        self.rest.tokens()
    }

    /// Currently-cached identity, if logged in.
    pub fn cached_identity(&self) -> Option<KChatIdentity> {
        self.tokens().snapshot().map(|t| t.identity)
    }

    /// `POST /api/v1/auth/login`
    pub async fn login(&self, body: &LoginRequest) -> Result<KChatIdentity, ClientError> {
        let resp: LoginResponse = self
            .rest
            .request_unauthed(Method::POST, "/api/v1/auth/login", Some(body))
            .await?;
        let identity = resp.identity.clone();
        let tokens = TokenSet::from_login(resp, Utc::now());
        self.tokens().replace(tokens);
        Ok(identity)
    }

    /// Forget cached tokens. Subsequent authenticated calls return
    /// [`ClientError::NotAuthenticated`].
    pub fn logout(&self) {
        self.tokens().clear();
    }

    /// HTTPS base URL the client is configured against. Surfaced
    /// through `kchat_backend_status` so the renderer can display
    /// the active backend host without re-storing the URL itself.
    pub fn base_url(&self) -> String {
        self.rest.base_url().to_string()
    }

    /// `GET /api/v1/identity`
    pub async fn get_identity(&self) -> Result<KChatIdentity, ClientError> {
        self.rest
            .request_authed::<(), KChatIdentity>(Method::GET, "/api/v1/identity", None)
            .await
    }

    /// `GET /api/v1/communities`
    pub async fn list_communities(&self) -> Result<Vec<KChatCommunity>, ClientError> {
        let resp: CommunitiesListResponse = self
            .rest
            .request_authed::<(), _>(Method::GET, "/api/v1/communities", None)
            .await?;
        Ok(resp.communities)
    }

    /// `GET /api/v1/communities/{id}/members`
    pub async fn get_community_members(
        &self,
        community_id: &str,
    ) -> Result<Vec<KChatCommunityMember>, ClientError> {
        let path = format!(
            "/api/v1/communities/{}/members",
            urlencoding(community_id)
        );
        let resp: MembersListResponse = self
            .rest
            .request_authed::<(), _>(Method::GET, &path, None)
            .await?;
        Ok(resp.members)
    }

    /// `POST /api/v1/communities/{id}/attestation`
    ///
    /// `peer_public_key` is the Ed25519 verifying key of the local
    /// collab identity (base64url no padding). The backend binds
    /// the signed attestation to this key so a stolen attestation
    /// can't be replayed against a different KCreate install.
    pub async fn get_membership_attestation(
        &self,
        community_id: &str,
        peer_public_key: &str,
    ) -> Result<MembershipAttestation, ClientError> {
        let path = format!(
            "/api/v1/communities/{}/attestation",
            urlencoding(community_id)
        );
        let body = AttestationRequest {
            peer_public_key: peer_public_key.to_string(),
        };
        self.rest
            .request_authed::<AttestationRequest, MembershipAttestation>(
                Method::POST,
                &path,
                Some(&body),
            )
            .await
    }

    /// `GET /api/v1/communities/{id}/conversations`
    pub async fn list_conversations(
        &self,
        community_id: &str,
    ) -> Result<Vec<KChatConversation>, ClientError> {
        let path = format!(
            "/api/v1/communities/{}/conversations",
            urlencoding(community_id)
        );
        let resp: ConversationsListResponse = self
            .rest
            .request_authed::<(), _>(Method::GET, &path, None)
            .await?;
        Ok(resp.conversations)
    }

    /// `POST /api/v1/conversations/{id}/messages`
    pub async fn post_message(
        &self,
        conversation_id: &str,
        body: &PostMessageRequest,
    ) -> Result<PostMessageResponse, ClientError> {
        let path = format!(
            "/api/v1/conversations/{}/messages",
            urlencoding(conversation_id)
        );
        self.rest
            .request_authed::<PostMessageRequest, PostMessageResponse>(
                Method::POST,
                &path,
                Some(body),
            )
            .await
    }

    /// `GET /api/v1/communities/{id}/events?since={cursor}`
    pub async fn poll_events(
        &self,
        community_id: &str,
        since: Option<&str>,
    ) -> Result<CommunityEventsResponse, ClientError> {
        let path = match since {
            Some(cursor) => format!(
                "/api/v1/communities/{}/events?since={}",
                urlencoding(community_id),
                urlencoding(cursor),
            ),
            None => format!("/api/v1/communities/{}/events", urlencoding(community_id)),
        };
        self.rest
            .request_authed::<(), CommunityEventsResponse>(Method::GET, &path, None)
            .await
    }
}

/// Minimal RFC 3986 path-segment / query-component encoder.
///
/// We deliberately avoid pulling in `percent-encoding` (one more
/// dep for two call sites) and `urlencoding` (deprecated). The
/// inputs here are community / conversation ids which are
/// constrained server-side to URL-safe ascii, so we only need to
/// escape the byte set the backend would reject.
fn urlencoding(s: &str) -> String {
    const SAFE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ\
                          abcdefghijklmnopqrstuvwxyz\
                          0123456789-_.~";
    use std::fmt::Write as _;
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        if SAFE.contains(&b) {
            out.push(b as char);
        } else {
            // `write!` on a `String` only fails when the underlying
            // `Display` impl returns `Err`; `u8` formatting is
            // infallible, so the expect message is purely for the
            // unreachable branch.
            write!(out, "%{b:02X}").expect("writing to String cannot fail");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urlencoding_passes_safe_chars() {
        assert_eq!(urlencoding("abc.123-_.~"), "abc.123-_.~");
    }

    #[test]
    fn urlencoding_escapes_unsafe_chars() {
        assert_eq!(urlencoding("a/b c"), "a%2Fb%20c");
    }

    #[test]
    fn urlencoding_escapes_unicode() {
        // U+1F600 = 0xF0 0x9F 0x98 0x80
        assert_eq!(urlencoding("\u{1F600}"), "%F0%9F%98%80");
    }
}
