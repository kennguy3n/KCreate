//! KChat Desktop local IPC client.
//!
//! Connects to a running `uney-chat-desktop` instance over a Unix
//! domain socket (`~/.kchat/kcreate.sock`, with `$XDG_RUNTIME_DIR`
//! fallback) or a Windows named pipe
//! (`\\.\pipe\kchat-kcreate`) and speaks the JSON-RPC 2.0 protocol
//! documented in [`protocol_spec.md`](./protocol_spec.md).
//!
//! ## Modules
//!
//! - [`protocol`] — wire-format request/response/notification types.
//! - [`transport`] — bidirectional newline-delimited JSON pump.
//! - [`client`] — high-level connection lifecycle + typed methods.
//! - [`attestation`] — bridge between the wire-format membership
//!   attestation and the in-tree `kcreate_collab::KChatMembership`.
//! - [`error`] — typed error surface.
//! - [`mock_server`] — in-process JSON-RPC server used by the test
//!   harness (and as the canonical Rust reference implementation for
//!   the future uney-chat-desktop server side).
//!
//! ## Local-first invariant
//!
//! This crate is the only Phase 7 addition that needs `tokio::net`,
//! and it talks to a LOCAL socket only — no DNS, no HTTP, no QUIC.
//! It is excluded from `kcreate_tests::tests::local_first.rs`
//! `editing_path_crates()` for the same reason
//! `kcreate_collab_transport` is: the bridge only depends on it
//! behind the off-by-default `kchat-desktop` feature flag.

pub mod attestation;
pub mod client;
pub mod error;
pub mod mock_server;
pub mod protocol;
pub mod transport;

pub use attestation::{
    decode_verifying_key, membership_from_attestation, KChatDesktopAuthority, REFRESH_BEFORE_EXPIRY,
};
pub use client::{default_socket_paths, KChatDesktopClient, CONNECT_TIMEOUT, RECONNECT_BACKOFF};
pub use error::ClientError;
pub use protocol::{
    CommunityEvent, CommunityEventKind, ErrorCode, InviteCardPayload, KChatCommunity,
    KChatCommunityMember, KChatConversation, KChatConversationType, KChatIdentity, KChatRole,
    MembershipAttestation, PostMessageParams, PostMessageResult, INVITE_CONTENT_TYPE,
    INVITE_SCHEMA_VERSION, PROTOCOL_VERSION,
};
pub use transport::Transport;

#[cfg(test)]
mod tests;
