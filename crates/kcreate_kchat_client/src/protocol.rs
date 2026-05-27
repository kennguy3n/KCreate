//! KChat Desktop Local IPC Protocol — JSON-RPC 2.0 wire types.
//!
//! This module defines the request and response types KCreate sends
//! to and receives from `uney-chat-desktop` over a local Unix domain
//! socket (`~/.kchat/kcreate.sock`) or Windows named pipe
//! (`\\.\pipe\kchat-kcreate`).
//!
//! See `protocol_spec.md` (same directory) for the comprehensive
//! protocol contract documented for the uney-chat-desktop team.
//!
//! ## Lockstep with `apps/desktop/shared/scene.ts`
//!
//! Every type defined here that crosses the bridge to the renderer
//! has a matching TypeScript declaration in
//! `apps/desktop/shared/scene.ts`. Adding a field to one requires
//! adding it to the other.
//!
//! ## Mapping to uney-chat-desktop's domain
//!
//! | KChat Desktop concept             | KCreate type                     |
//! | --------------------------------- | -------------------------------- |
//! | XMPP bare JID                     | `KChatIdentity::jid`             |
//! | MLS identity key (Ed25519 pubkey) | `KChatIdentity::public_key`      |
//! | Community                         | `KChatCommunity`                 |
//! | Community member + role           | `KChatCommunityMember`           |
//! | Conversation / channel            | `KChatConversation`              |
//! | Signed membership attestation     | `MembershipAttestation`          |

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// JSON-RPC 2.0 version literal.
pub const JSONRPC_VERSION: &str = "2.0";

/// Protocol version advertised in the handshake. Independent of the
/// KCreate workspace version so KCreate and uney-chat-desktop can
/// rev independently.
pub const PROTOCOL_VERSION: u32 = 1;

// ----------------- JSON-RPC 2.0 envelope ------------------------

/// JSON-RPC 2.0 request envelope. Carries `method` + `params`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcRequest {
    pub jsonrpc: String,
    /// Method id (e.g. `"kchat.identity.get"`).
    pub method: String,
    /// Optional structured params. Defaults to a JSON `null` when
    /// the method takes no parameters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
    /// Caller-supplied id used for response correlation. Always a
    /// string in KCreate's wire format (uuid-shaped) so JS clients
    /// don't need to coerce numbers.
    pub id: String,
}

/// JSON-RPC 2.0 success / error response envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcResponse {
    pub jsonrpc: String,
    /// Echo of `RpcRequest::id`.
    pub id: String,
    /// Success payload (mutually exclusive with `error`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    /// Failure payload (mutually exclusive with `result`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

/// JSON-RPC 2.0 error object.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RpcError {
    /// Numeric error code (see [`ErrorCode`]).
    pub code: i32,
    /// Human-readable message.
    pub message: String,
    /// Optional structured data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

/// Server-initiated notification — sent without a request, never
/// expects a response. Used for the `kchat.events.subscribe` stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcNotification {
    pub jsonrpc: String,
    /// Notification method id (e.g. `"kchat.events.notify"`).
    pub method: String,
    /// Structured payload.
    pub params: serde_json::Value,
}

/// Error codes returned by the KChat Desktop server.
///
/// Standard JSON-RPC reserves -32700 .. -32000. KChat-specific
/// errors live in the -32099 .. -32000 implementation-defined
/// range as required by the spec. Codes < -32000 are reserved
/// and never returned by this server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum ErrorCode {
    /// JSON parsing failed (`-32700`).
    ParseError = -32700,
    /// Request not a valid JSON-RPC 2.0 request (`-32600`).
    InvalidRequest = -32600,
    /// Method not found (`-32601`).
    MethodNotFound = -32601,
    /// Invalid params (`-32602`).
    InvalidParams = -32602,
    /// Internal server error (`-32603`).
    InternalError = -32603,
    /// Caller is not authenticated (no active KChat user session).
    NotAuthenticated = -32001,
    /// Caller does not have permission for this resource.
    PermissionDenied = -32002,
    /// Referenced resource (community, conversation, member) not
    /// found.
    NotFound = -32003,
    /// Subscription already active for this community.
    AlreadySubscribed = -32004,
    /// Server is shutting down.
    Shutdown = -32005,
}

impl ErrorCode {
    /// Numeric value for serialization.
    #[must_use]
    pub fn as_i32(self) -> i32 {
        self as i32
    }
}

// ----------------- Method param + result types ------------------

/// `kchat.identity.get` — empty params.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IdentityGetParams {}

/// `kchat.identity.get` result — local user identity.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct KChatIdentity {
    /// XMPP bare JID (e.g. `"user@kchat.com"`).
    pub jid: String,
    /// Human display name.
    pub display_name: String,
    /// Ed25519 public key (derived from MLS identity), base64url no
    /// padding, 32 bytes after decode.
    pub public_key: String,
    /// BLAKE3-derived peer id matching `kcreate_collab::peer::PeerId`
    /// computed from `public_key`.
    pub peer_id: String,
}

/// `kchat.communities.list` — empty params.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CommunitiesListParams {}

/// One community returned by `kchat.communities.list`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct KChatCommunity {
    /// Community identifier (URL-safe; reused as the KChat group id
    /// for membership attestations).
    pub id: String,
    /// Human-readable community name.
    pub name: String,
    /// Optional description / topic.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Current member count (informational; not enforced).
    pub member_count: u32,
    /// Local user's role in this community.
    pub role: KChatRole,
}

/// `kchat.communities.list` result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommunitiesListResult {
    pub communities: Vec<KChatCommunity>,
}

/// Role of a community member. Maps 1:1 to uney-chat-desktop's
/// community role enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KChatRole {
    /// Community creator. Full read-write, can kick, can manage ACL.
    Owner,
    /// Promoted admin. Same effective collab privileges as `Owner`.
    Admin,
    /// Regular member. Editor by default, host may downgrade to viewer.
    Member,
}

impl KChatRole {
    /// Lowercase string form matching the serde serialization. Used
    /// by the bridge to map roles through
    /// [`CollabPermission::from_role`](crate::CollabPermission) on
    /// the `collab` side.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Admin => "admin",
            Self::Member => "member",
        }
    }
}

/// `kchat.communities.getMembers` params.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetMembersParams {
    pub community_id: String,
}

/// One member returned by `kchat.communities.getMembers`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct KChatCommunityMember {
    /// XMPP bare JID.
    pub jid: String,
    pub display_name: String,
    /// Ed25519 public key, base64url no padding.
    pub public_key: String,
    /// BLAKE3-derived peer id from `public_key`.
    pub peer_id: String,
    pub role: KChatRole,
}

/// `kchat.communities.getMembers` result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetMembersResult {
    pub members: Vec<KChatCommunityMember>,
}

/// `kchat.communities.getMembership` params.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetMembershipParams {
    pub community_id: String,
}

/// Signed membership attestation returned by
/// `kchat.communities.getMembership`. Drop-in compatible with
/// `kcreate_collab::kchat::KChatMembership` after rebuild via
/// [`crate::attestation::membership_from_attestation`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MembershipAttestation {
    /// Community-wide signing key (Ed25519, derived from MLS group
    /// identity). base64url no padding.
    pub issuer_public_key: String,
    /// Community id (reused as `KChatGroupId`).
    pub group_id: String,
    /// BLAKE3-derived peer id of the local user.
    pub peer_id: String,
    /// Local user's Ed25519 verifying key, base64url no padding.
    pub peer_public_key: String,
    /// Issuance timestamp.
    pub issued_at: DateTime<Utc>,
    /// Expiry timestamp.
    pub expires_at: DateTime<Utc>,
    /// Ed25519 signature over the canonical view (matches
    /// `kcreate_collab::kchat::KChatMembership` signing layout).
    pub signature: String,
}

/// `kchat.conversations.list` params.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationsListParams {
    pub community_id: String,
}

/// One conversation returned by `kchat.conversations.list`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct KChatConversation {
    /// Conversation identifier.
    pub id: String,
    /// Human-readable name (channel topic or DM peer name).
    pub name: String,
    /// Owning community id.
    pub community_id: String,
    /// Channel vs direct-message classification.
    pub conversation_type: KChatConversationType,
}

/// Classifies a conversation as a community channel or a 1:1 DM.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KChatConversationType {
    /// Group channel inside a community.
    Channel,
    /// Direct message between two community members.
    Direct,
}

/// `kchat.conversations.list` result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationsListResult {
    pub conversations: Vec<KChatConversation>,
}

/// `kchat.conversations.postMessage` params.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostMessageParams {
    /// Target conversation.
    pub conversation_id: String,
    /// Application-defined payload object. For KCreate document
    /// share invites, this carries the [`InviteCardPayload`].
    pub payload: serde_json::Value,
    /// Optional payload kind discriminator (e.g.
    /// `"kcreate.invite.v1"`). When set, uney-chat-desktop renders
    /// the message as a rich card using its custom-content registry.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
}

/// `kchat.conversations.postMessage` result.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostMessageResult {
    /// Server-assigned message id.
    pub message_id: String,
    /// Server timestamp.
    pub posted_at: DateTime<Utc>,
}

/// Custom content-type identifier for KCreate document share invites.
/// uney-chat-desktop renders these as rich cards via the extension
/// platform's content-renderer registry.
pub const INVITE_CONTENT_TYPE: &str = "kcreate.invite.v1";

/// Schema version stamped into [`InviteCardPayload::schema_version`]
/// when minting a fresh invite. The bridge rejects accept-invite
/// calls whose payload declares a different version so a future
/// schema bump (e.g. adding `mls_group_epoch` or `expires_at`)
/// can't be silently consumed by an older binary that would skip
/// the new fields.
pub const INVITE_SCHEMA_VERSION: u32 = 1;

/// Schema for the KCreate document-share invite payload. The same
/// JSON shape is consumed by the renderer's `InvitePanel.tsx`
/// component when a user opens a shared invite.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InviteCardPayload {
    /// Schema version. Pinned to 1; bump when the layout changes
    /// incompatibly.
    pub schema_version: u32,
    /// KCreate project id the invite points to.
    pub project_id: uuid::Uuid,
    /// Human-readable project name (so the renderer can show
    /// "Join Ken's project" without first dialing).
    pub project_name: String,
    /// Owning peer id (BLAKE3 hash of the owner's Ed25519 key).
    pub owner_peer_id: String,
    /// Owner's Ed25519 public key, base64url no padding.
    pub owner_public_key: String,
    /// Owner's display name as advertised on KCreate.
    pub owner_display_name: String,
    /// SHA-256 of the owner's QUIC leaf TLS cert (base64-no-pad),
    /// for pinned-fingerprint dialing.
    pub cert_fingerprint: String,
    /// Owner's QUIC socket address (`<ip>:<port>`).
    pub owner_socket_addr: String,
    /// Community id this invite is gated on. Joiner must be a
    /// member of the same community.
    pub community_id: String,
    /// Conversation id the invite was posted to (for back-reference
    /// in the renderer's audit trail).
    pub conversation_id: String,
    /// When the invite was minted (RFC 3339 UTC).
    pub issued_at: DateTime<Utc>,
}

/// `kchat.events.subscribe` params.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventsSubscribeParams {
    pub community_id: String,
}

/// `kchat.events.subscribe` synchronous result. Notifications are
/// pushed asynchronously via [`RpcNotification`] with the
/// `kchat.events.notify` method.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventsSubscribeResult {
    /// Server-assigned subscription id. The renderer references
    /// this in `kchat.events.unsubscribe` to tear down the stream.
    pub subscription_id: String,
}

/// `kchat.events.unsubscribe` params.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventsUnsubscribeParams {
    pub subscription_id: String,
}

/// `kchat.events.notify` payload — a single streaming event from a
/// subscribed community.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommunityEvent {
    /// Subscription this event belongs to.
    pub subscription_id: String,
    /// Community the event happened in.
    pub community_id: String,
    /// Event payload.
    pub event: CommunityEventKind,
    /// Server timestamp.
    pub at: DateTime<Utc>,
}

/// Variant body of a `CommunityEvent`. The `kind` discriminator is
/// emitted as the JSON `kind` field for renderer-friendly parsing.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum CommunityEventKind {
    /// A new member joined the community.
    MemberJoined { member: KChatCommunityMember },
    /// A member was removed (or left voluntarily).
    MemberLeft { peer_id: String, jid: String },
    /// A member's role changed (e.g. promoted to admin).
    MemberRoleChanged {
        peer_id: String,
        jid: String,
        new_role: KChatRole,
    },
    /// A member came online or went offline (presence ping).
    MemberPresence {
        peer_id: String,
        jid: String,
        online: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jsonrpc_envelope_round_trips() {
        let req = RpcRequest {
            jsonrpc: JSONRPC_VERSION.to_string(),
            method: "kchat.identity.get".into(),
            params: None,
            id: "1".into(),
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: RpcRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back.method, "kchat.identity.get");
        assert_eq!(back.id, "1");
    }

    #[test]
    fn rpc_error_round_trips() {
        let resp = RpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: "x".into(),
            result: None,
            error: Some(RpcError {
                code: ErrorCode::NotFound.as_i32(),
                message: "community not found".into(),
                data: None,
            }),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let back: RpcResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back.error.unwrap().code, -32003);
    }

    #[test]
    fn community_event_serializes_with_camel_case() {
        let event = CommunityEvent {
            subscription_id: "sub-1".into(),
            community_id: "comm-1".into(),
            event: CommunityEventKind::MemberLeft {
                peer_id: "peer-x".into(),
                jid: "x@kchat.com".into(),
            },
            at: Utc::now(),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["subscriptionId"], "sub-1");
        assert_eq!(json["communityId"], "comm-1");
        assert_eq!(json["event"]["kind"], "memberLeft");
        assert_eq!(json["event"]["peerId"], "peer-x");
    }

    #[test]
    fn role_serializes_lowercase() {
        let v = serde_json::to_value(KChatRole::Admin).unwrap();
        assert_eq!(v, serde_json::Value::String("admin".into()));
    }
}
