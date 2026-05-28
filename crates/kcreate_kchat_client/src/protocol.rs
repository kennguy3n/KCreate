//! KChat backend REST API — wire-format types.
//!
//! KCreate talks to the shared KChat / Mattermost backend over
//! HTTPS. This module defines the JSON shapes of every request +
//! response body exchanged with that backend, plus the
//! domain types (identity, community, conversation, attestation,
//! invite card) that the bridge re-exports to the renderer through
//! `apps/desktop/shared/scene.ts`.
//!
//! ## REST contract (summary)
//!
//! All endpoints are versioned under `/api/v1/`. The
//! `Authorization: Bearer <token>` header carries the access token
//! returned by `POST /api/v1/auth/login`. The client transparently
//! refreshes the access token on `401` using the cached refresh
//! token (see `auth.rs`).
//!
//! | Method  | Path                                                    | Purpose                                          |
//! | ------- | ------------------------------------------------------- | ------------------------------------------------ |
//! | POST    | `/api/v1/auth/login`                                    | Exchange credentials for access + refresh tokens |
//! | POST    | `/api/v1/auth/refresh`                                  | Exchange refresh token for a new access token    |
//! | GET     | `/api/v1/identity`                                      | Local user's identity (JID + display + pubkey)   |
//! | GET     | `/api/v1/communities`                                   | Communities the local user belongs to            |
//! | GET     | `/api/v1/communities/{id}/members`                      | Member list with roles                           |
//! | POST    | `/api/v1/communities/{id}/attestation`                  | Request a signed membership attestation          |
//! | GET     | `/api/v1/communities/{id}/conversations`                | Channels / DMs in the community                  |
//! | POST    | `/api/v1/conversations/{id}/messages`                   | Post a rich-card invite to a conversation        |
//! | GET     | `/api/v1/communities/{id}/events?since={cursor}`        | Roster-change polling endpoint                   |
//!
//! ### Out of repo scope (follows in a separate PR)
//!
//! The backend currently exposes communities, members,
//! conversations, and messages. It does **not** yet sign
//! `POST /api/v1/communities/{id}/attestation`. KCreate's client
//! reports a typed
//! [`ClientError::AttestationEndpointNotProvisioned`](crate::error::ClientError::AttestationEndpointNotProvisioned)
//! when the backend returns `404` or `501` for this route so the
//! renderer can surface a clear "backend has not shipped the
//! attestation endpoint yet — falling back to dev-mint if
//! `kchat-dev-issuer` is enabled" message.
//!
//! The axum fixture server in `fixture.rs` implements every route
//! above (including signing attestations with a freshly-generated
//! Ed25519 keypair) so the test suite exercises the full
//! production code path end-to-end.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Wire-format version stamped into request headers so the backend
/// can refuse traffic from an incompatibly-newer KCreate build.
/// Independent of the workspace version so KCreate and the
/// backend can rev independently.
pub const PROTOCOL_VERSION: u32 = 1;

/// HTTP header carrying [`PROTOCOL_VERSION`] on every request.
pub const PROTOCOL_VERSION_HEADER: &str = "X-KCreate-Protocol-Version";

/// HTTP header used on every request to identify the calling
/// product. Lets the backend log + filter KCreate traffic
/// independently from Desktop/Mobile/Web clients.
pub const USER_AGENT_HEADER_VALUE: &str = concat!("KCreate/", env!("CARGO_PKG_VERSION"));

// ----------------- Auth ----------------------------------------

/// Body of `POST /api/v1/auth/login`. Sent over HTTPS only; the
/// REST client refuses to send credentials over `http://` outside
/// the test fixture.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginRequest {
    /// Login id (XMPP bare JID for KChat). Mattermost installations
    /// accept either the username or the email — the backend
    /// disambiguates server-side.
    pub login_id: String,
    /// User password.
    pub password: String,
    /// Optional TOTP code when the account has 2FA enabled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub totp: Option<String>,
}

/// Body of `POST /api/v1/auth/login` response on success.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LoginResponse {
    /// Short-lived bearer token. Used on every subsequent
    /// authenticated request.
    pub access_token: String,
    /// Long-lived refresh token. Stored in memory only — never
    /// persisted to disk by KCreate.
    pub refresh_token: String,
    /// Lifetime of the access token in seconds. The client uses
    /// this to schedule pre-emptive refresh.
    pub expires_in_seconds: u64,
    /// Identity of the now-authenticated user.
    pub identity: KChatIdentity,
}

/// Body of `POST /api/v1/auth/refresh`. The client sends this
/// transparently on 401 with the cached refresh token.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshRequest {
    pub refresh_token: String,
}

/// Body of `POST /api/v1/auth/refresh` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshResponse {
    pub access_token: String,
    /// The backend MAY rotate the refresh token on refresh — when
    /// it does, the client stores the new value.
    pub refresh_token: String,
    pub expires_in_seconds: u64,
}

// ----------------- Domain DTOs ---------------------------------

/// Local user identity returned by `GET /api/v1/identity` and as
/// part of [`LoginResponse`].
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

/// Response body for `GET /api/v1/communities`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommunitiesListResponse {
    pub communities: Vec<KChatCommunity>,
}

/// One community returned by `GET /api/v1/communities`.
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

/// Role of a community member. Maps 1:1 to KChat's community role
/// enum.
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
    /// by the bridge to map roles through `CollabPermission` on the
    /// `collab` side.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Admin => "admin",
            Self::Member => "member",
        }
    }
}

/// Response body for `GET /api/v1/communities/{id}/members`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MembersListResponse {
    pub members: Vec<KChatCommunityMember>,
}

/// One member returned by `GET /api/v1/communities/{id}/members`.
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

/// Body of `POST /api/v1/communities/{id}/attestation`. The peer
/// public key is supplied by the calling client so the backend can
/// bind the attestation to the exact local identity that will be
/// used for collab.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttestationRequest {
    /// Ed25519 verifying key of the local peer, base64url no
    /// padding. The backend signs an attestation that binds this
    /// key to the community id, so a stolen attestation can't be
    /// replayed against a different KCreate install.
    pub peer_public_key: String,
}

/// Signed membership attestation returned by
/// `POST /api/v1/communities/{id}/attestation`. Drop-in compatible
/// with `kcreate_collab::kchat::KChatMembership` after rebuild via
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

/// Response body for `GET /api/v1/communities/{id}/conversations`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationsListResponse {
    pub conversations: Vec<KChatConversation>,
}

/// One conversation returned by
/// `GET /api/v1/communities/{id}/conversations`.
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

/// Body of `POST /api/v1/conversations/{id}/messages`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostMessageRequest {
    /// Application-defined payload object. For KCreate document
    /// share invites, this carries the [`InviteCardPayload`].
    pub payload: serde_json::Value,
    /// Optional payload kind discriminator (e.g.
    /// `"kcreate.invite.v1"`). When set, KChat Desktop's `.kcz`
    /// extension renders the message as a rich card using its
    /// custom-content registry.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
}

/// Backwards-compatible alias for the previous Phase 7
/// `PostMessageParams` JSON-RPC shape. The bridge constructs an
/// `PostMessageRequest` from a `conversation_id` + `payload` +
/// `content_type` triple — we keep the legacy alias so existing
/// renderer wiring continues to compile after the REST pivot
/// without touching every call site.
pub type PostMessageParams = PostMessageRequest;

/// Response body for `POST /api/v1/conversations/{id}/messages`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostMessageResponse {
    /// Server-assigned message id.
    pub message_id: String,
    /// Server timestamp.
    pub posted_at: DateTime<Utc>,
}

/// Backwards-compatible alias for the previous `PostMessageResult`
/// JSON-RPC response shape. See [`PostMessageParams`].
pub type PostMessageResult = PostMessageResponse;

/// Custom content-type identifier for KCreate document share
/// invites. The `.kcz` companion extension in
/// `apps/kchat-extension/` renders these as rich cards via the
/// host's content-renderer registry.
pub const INVITE_CONTENT_TYPE: &str = "kcreate.invite.v1";

/// Schema version stamped into [`InviteCardPayload::schema_version`]
/// when minting a fresh invite. The bridge rejects accept-invite
/// calls whose payload declares a different version so a future
/// schema bump can't be silently consumed by an older binary that
/// would skip the new fields.
pub const INVITE_SCHEMA_VERSION: u32 = 1;

/// Schema for the KCreate document-share invite payload. The same
/// JSON shape is consumed by the renderer's `InvitePanel.tsx`
/// component when a user opens a shared invite — and by the
/// `.kcz` companion extension's `InviteCard.tsx` renderer.
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

// ----------------- Roster polling ------------------------------

/// Response body for
/// `GET /api/v1/communities/{id}/events?since={cursor}`. KCreate
/// polls this every 30s during an active collab session to detect
/// member-joined / member-left / role-changed events without
/// needing a long-lived streaming connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommunityEventsResponse {
    /// Events that occurred after `since`. Always returned in
    /// `at` ascending order; an empty list means no changes.
    pub events: Vec<CommunityEvent>,
    /// Opaque cursor to pass as `since` on the next poll.
    pub next_cursor: String,
}

/// One community event. The same `CommunityEventKind` discriminator
/// the previous JSON-RPC notification carried — REST polling is
/// just a different delivery channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommunityEvent {
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

// ----------------- Backend error envelope -----------------------

/// JSON envelope the backend returns on 4xx/5xx responses. KCreate
/// surfaces both fields verbatim through
/// [`ClientError::Backend`](crate::error::ClientError::Backend) so
/// the renderer can display the backend's error message directly.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BackendErrorBody {
    /// Stable error code (e.g. `"AUTH_INVALID"`, `"NOT_FOUND"`,
    /// `"COMMUNITY_NOT_FOUND"`, `"ATTESTATION_NOT_PROVISIONED"`).
    pub code: String,
    /// Human-readable error message.
    pub message: String,
    /// Optional structured payload.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

// ----------------- Artifact publishing -------------------------

/// Wire format of an artifact's MIME / file kind. Carried across
/// the multipart upload as a string field so the backend can pick
/// the right preview pipeline (raster vs vector vs ZIP), and
/// echoed back on the `GET /api/v1/conversations/{id}/artifacts`
/// list response.
///
/// Values are emitted lowercase to match the surrounding REST
/// contract (every other enum in this module is `lowercase`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArtifactKind {
    /// Rasterised canvas export. Bytes are a PNG image.
    Png,
    /// Vector canvas export. Bytes are a UTF-8 SVG document.
    Svg,
    /// Vector + raster export. Bytes are a PDF document.
    Pdf,
    /// Compressed rasterised canvas export. Bytes are a WebP image.
    Webp,
    /// JPEG fallback (e.g. when the renderer asked for an opaque
    /// photo-grade compression).
    Jpeg,
    /// `.kbrand` ZIP archive (Brand Kit publish).
    BrandKit,
}

impl ArtifactKind {
    /// Conventional MIME type carried in the multipart Content-Type
    /// header for the primary artifact part. The backend uses this
    /// to gate preview generation — uploads with an unknown MIME
    /// surface as `415 Unsupported Media Type`.
    #[must_use]
    pub const fn mime(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Svg => "image/svg+xml",
            Self::Pdf => "application/pdf",
            Self::Webp => "image/webp",
            Self::Jpeg => "image/jpeg",
            Self::BrandKit => "application/zip",
        }
    }

    /// Conventional file-name suffix (no leading dot). Used by the
    /// fixture + by the bridge when assembling the multipart part
    /// filename for the artifact bytes.
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Svg => "svg",
            Self::Pdf => "pdf",
            Self::Webp => "webp",
            Self::Jpeg => "jpeg",
            Self::BrandKit => "kbrand",
        }
    }
}

/// Structured metadata sent alongside the artifact bytes on
/// `POST /api/v1/conversations/{id}/artifacts`. Serialised as JSON
/// into a multipart form-data part named `metadata`. Echoed back on
/// the `GET .../artifacts` list response (so the renderer can paint
/// the rich-card preview without re-fetching the bytes).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactMetadata {
    /// Originating project name. Surfaced verbatim on the rich
    /// preview card.
    pub project_name: String,
    /// Originating artboard / page name (or brand-kit name for
    /// `BrandKit` uploads). `None` for whole-project uploads.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artboard_name: Option<String>,
    /// Free-form export preset identifier (e.g.
    /// `"PNG @1x"`, `"PDF A4 300dpi"`). Surfaced as a chip on the
    /// rich preview card so reviewers can tell which preset they
    /// are looking at without re-downloading.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub export_preset: Option<String>,
    /// Rasterised pixel dimensions, when applicable. `None` for
    /// vector-only artifacts (SVG, PDF) and `.kbrand`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width_px: Option<u32>,
    /// See [`Self::width_px`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height_px: Option<u32>,
    /// Source project id (`Uuid`). Lets the backend dedupe re-uploads
    /// from the same project + preset, and lets the renderer
    /// surface "this came from your KCreate project".
    pub project_id: uuid::Uuid,
    /// Wire-format kind of the artifact bytes.
    pub kind: ArtifactKind,
}

/// Response body for `POST /api/v1/conversations/{id}/artifacts` on
/// success. Mirrored on the bridge surface as
/// `KChatArtifactPublishResult` in `apps/desktop/shared/scene.ts`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactPublishResult {
    /// Server-assigned artifact id.
    pub artifact_id: String,
    /// Conversation the artifact was published into.
    pub conversation_id: String,
    /// URL the renderer can hit to preview / download the artifact.
    /// Backend is responsible for short-circuiting auth on this URL
    /// (typically a signed short-lived link).
    pub preview_url: String,
    /// URL of the rendered thumbnail. May equal `preview_url` when
    /// the artifact is already small enough that no separate
    /// thumbnail was needed.
    pub thumbnail_url: String,
    /// Wire format / MIME the backend stored the artifact as.
    pub kind: ArtifactKind,
    /// Server-assigned timestamp.
    pub published_at: DateTime<Utc>,
}

/// Body of `GET /api/v1/conversations/{id}/artifacts`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactsListResponse {
    /// Most-recent-first list of artifacts published to the
    /// conversation.
    pub artifacts: Vec<PublishedArtifact>,
}

/// One artifact returned by [`ArtifactsListResponse`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PublishedArtifact {
    pub artifact_id: String,
    pub conversation_id: String,
    pub preview_url: String,
    pub thumbnail_url: String,
    pub kind: ArtifactKind,
    /// Server-side metadata snapshot captured at upload time.
    pub metadata: ArtifactMetadata,
    /// Size of the artifact bytes the backend has on file.
    pub byte_size: u64,
    pub published_at: DateTime<Utc>,
}

/// Multipart form-data field names. Pulled into constants so the
/// client + fixture + bridge all agree on the wire shape — a typo
/// in one place would otherwise surface as a confusing `400 Bad
/// Request` only on production traffic.
pub mod artifact_field {
    /// Primary artifact bytes (the export). MIME type is set on
    /// the part via [`super::ArtifactKind::mime`].
    pub const ARTIFACT: &str = "artifact";
    /// PNG-encoded thumbnail bytes.
    pub const THUMBNAIL: &str = "thumbnail";
    /// JSON-encoded [`super::ArtifactMetadata`].
    pub const METADATA: &str = "metadata";
}

/// Well-known backend error codes the client maps to typed error
/// variants. Anything not listed here surfaces as
/// [`ClientError::Backend`](crate::error::ClientError::Backend)
/// with the raw `code` + `message`.
pub mod error_code {
    /// `POST /api/v1/auth/login` rejected the credentials.
    pub const AUTH_INVALID: &str = "AUTH_INVALID";
    /// Caller does not have permission for this resource (e.g.
    /// not a member of the requested community).
    pub const PERMISSION_DENIED: &str = "PERMISSION_DENIED";
    /// Backend hasn't shipped the `/attestation` endpoint yet.
    pub const ATTESTATION_NOT_PROVISIONED: &str = "ATTESTATION_NOT_PROVISIONED";
    /// Community id does not exist or is no longer visible.
    pub const COMMUNITY_NOT_FOUND: &str = "COMMUNITY_NOT_FOUND";
    /// Conversation id does not exist or is no longer visible.
    pub const CONVERSATION_NOT_FOUND: &str = "CONVERSATION_NOT_FOUND";
    /// Caller sent a malformed body.
    pub const INVALID_REQUEST: &str = "INVALID_REQUEST";
    /// Caller posted an artifact whose declared kind / MIME isn't
    /// in the backend's supported set.
    pub const UNSUPPORTED_ARTIFACT_KIND: &str = "UNSUPPORTED_ARTIFACT_KIND";
    /// Artifact body exceeds the backend's per-upload byte cap.
    pub const ARTIFACT_TOO_LARGE: &str = "ARTIFACT_TOO_LARGE";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_request_round_trips_camel_case() {
        let req = LoginRequest {
            login_id: "alice@kchat.com".into(),
            password: "hunter2".into(),
            totp: Some("123456".into()),
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["loginId"], "alice@kchat.com");
        assert_eq!(json["totp"], "123456");
        let back: LoginRequest = serde_json::from_value(json).unwrap();
        assert_eq!(back.login_id, "alice@kchat.com");
    }

    #[test]
    fn login_response_round_trips() {
        let resp = LoginResponse {
            access_token: "a.b.c".into(),
            refresh_token: "r.t.k".into(),
            expires_in_seconds: 3600,
            identity: KChatIdentity {
                jid: "alice@kchat.com".into(),
                display_name: "Alice".into(),
                public_key: "AAA".into(),
                peer_id: "peer-alice".into(),
            },
        };
        let json = serde_json::to_string(&resp).unwrap();
        let back: LoginResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back.access_token, "a.b.c");
        assert_eq!(back.identity.display_name, "Alice");
    }

    #[test]
    fn community_event_serializes_with_camel_case() {
        let event = CommunityEvent {
            community_id: "comm-1".into(),
            event: CommunityEventKind::MemberLeft {
                peer_id: "peer-x".into(),
                jid: "x@kchat.com".into(),
            },
            at: Utc::now(),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["communityId"], "comm-1");
        assert_eq!(json["event"]["kind"], "memberLeft");
        assert_eq!(json["event"]["peerId"], "peer-x");
    }

    #[test]
    fn role_serializes_lowercase() {
        let v = serde_json::to_value(KChatRole::Admin).unwrap();
        assert_eq!(v, serde_json::Value::String("admin".into()));
    }

    #[test]
    fn backend_error_body_round_trips() {
        let err = BackendErrorBody {
            code: error_code::AUTH_INVALID.into(),
            message: "bad credentials".into(),
            data: Some(serde_json::json!({"attempt": 3})),
        };
        let json = serde_json::to_value(&err).unwrap();
        assert_eq!(json["code"], "AUTH_INVALID");
        let back: BackendErrorBody = serde_json::from_value(json).unwrap();
        assert_eq!(back.code, "AUTH_INVALID");
        assert_eq!(back.data.unwrap()["attempt"], 3);
    }

    #[test]
    fn conversation_type_camel_case_field_and_lowercase_value() {
        let conv = KChatConversation {
            id: "c1".into(),
            name: "general".into(),
            community_id: "comm".into(),
            conversation_type: KChatConversationType::Channel,
        };
        let json = serde_json::to_value(&conv).unwrap();
        assert_eq!(json["conversationType"], "channel");
    }
}
