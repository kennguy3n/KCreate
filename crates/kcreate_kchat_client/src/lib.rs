//! KChat backend REST client used by the KCreate bridge to source
//! community membership attestations, list communities + members,
//! and post document-share invites.
//!
//! See [`protocol`] for the REST contract this client speaks.
//!
//! ## Integration model
//!
//! KCreate runs as a standalone process. Both KCreate and KChat
//! Desktop independently authenticate to the same KChat /
//! Mattermost backend over HTTPS REST — there is no external IPC
//! socket between the two desktop apps. A separate `.kcz`
//! companion extension ships inside KChat Desktop (see
//! `apps/kchat-extension/`) and contributes a sidebar showing
//! recent KCreate projects + share invites. That extension uses
//! the host's procedures registry — it does **not** proxy this
//! REST client.
//!
//! ## Crate isolation
//!
//! This crate links `reqwest` (rustls-tls). The bridge depends on
//! it only behind the off-by-default `kchat-backend` feature flag,
//! so the local-first deny-list sentinel in
//! `crates/kcreate_tests/tests/local_first.rs` keeps the default
//! build network-free.

pub mod attestation;
pub mod auth;
pub mod client;
pub mod error;
pub mod protocol;
pub mod rest;

#[cfg(feature = "test-fixture")]
pub mod fixture;

pub use attestation::{
    decode_verifying_key, membership_from_attestation, KChatBackendAuthority, REFRESH_BEFORE_EXPIRY,
};
pub use auth::{TokenSet, TokenStore, PREEMPTIVE_REFRESH_WINDOW};
pub use client::KChatBackendClient;
pub use error::ClientError;
pub use protocol::{
    error_code, AttestationRequest, BackendErrorBody, CommunitiesListResponse, CommunityEvent,
    CommunityEventKind, CommunityEventsResponse, ConversationsListResponse, InviteCardPayload,
    KChatCommunity, KChatCommunityMember, KChatConversation, KChatConversationType, KChatIdentity,
    KChatRole, LoginRequest, LoginResponse, MembersListResponse, MembershipAttestation,
    PostMessageParams, PostMessageRequest, PostMessageResponse, PostMessageResult, RefreshRequest,
    RefreshResponse, INVITE_CONTENT_TYPE, INVITE_SCHEMA_VERSION, PROTOCOL_VERSION,
    PROTOCOL_VERSION_HEADER, USER_AGENT_HEADER_VALUE,
};
pub use rest::{RestClient, RestClientConfig, MAX_RATE_LIMIT_BACKOFF, MAX_RATE_LIMIT_RETRIES};
