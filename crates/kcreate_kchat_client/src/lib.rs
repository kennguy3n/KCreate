//! KChat backend REST client used by the KCreate bridge to source
//! community membership attestations, list communities + members,
//! and post document-share invites.
//!
//! See [`protocol`] for the REST contract this client speaks.
//!
//! ## Architecture (Option C pivot — May 2026)
//!
//! Previously this crate spoke a local Unix-socket / named-pipe
//! JSON-RPC protocol to a hypothetical `uney-chat-desktop` server
//! that does not exist. After ken's architectural review, the
//! correct integration model is:
//!
//!   - KCreate runs as a **standalone process**.
//!   - It talks to the **shared KChat / Mattermost backend** over
//!     HTTPS REST — the same backend `uney-chat-desktop` also uses.
//!   - A separate `.kcz` **companion extension** ships inside
//!     KChat Desktop (see `apps/kchat-extension/`) and contributes
//!     a sidebar showing recent KCreate projects + share invites.
//!     That extension uses the host's procedures registry — it
//!     does **not** proxy this REST client.
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
    decode_verifying_key, membership_from_attestation, KChatBackendAuthority,
    REFRESH_BEFORE_EXPIRY,
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
