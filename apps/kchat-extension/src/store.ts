// KCreate companion extension: typed store + host-procedure
// consumers.
//
// All DTOs are Zod-validated where they cross the host boundary.
// The runtime is the host-injected procedure registry; this file is
// the only place that talks to it from the panel.
import { z } from "zod";
import { invokeProcedure, openDeeplink } from "./host";

// ----- DTOs ----------------------------------------------------------------

export const CommunityRoleSchema = z.enum(["owner", "admin", "member"]);
export type CommunityRole = z.infer<typeof CommunityRoleSchema>;

export const CommunitySchema = z.object({
  id: z.string().min(1),
  name: z.string().min(1),
  role: CommunityRoleSchema,
});
export type Community = z.infer<typeof CommunitySchema>;

export const CommunityMemberSchema = z.object({
  jid: z.string().min(1),
  displayName: z.string().min(1),
  publicKey: z.string().min(1),
  role: CommunityRoleSchema,
});
export type CommunityMember = z.infer<typeof CommunityMemberSchema>;

// A share-invite card the user sent or received in this conversation.
//
// The card payload schema mirrors `InviteCardPayload` in
// `crates/kcreate_kchat_client/src/protocol.rs` (the canonical wire
// shape) and `KChatShareInvite` in
// `crates/kcreate_bridge/src/kchat_backend.rs` so the panel can hand
// the parsed payload straight to KCreate's bridge through the
// `kcreate://join?payload=...` deeplink without any field
// reshuffling.
export const ShareInviteSchema = z.object({
  // Pinned to 1; the bridge rejects accept-invite calls with a
  // different schema version so the panel mirrors that contract.
  schemaVersion: z.literal(1),
  // UUID v4 string; the host validates downstream.
  projectId: z.string().min(1),
  projectName: z.string().min(1),
  ownerPeerId: z.string().min(1),
  ownerPublicKey: z.string().min(1),
  ownerDisplayName: z.string().min(1),
  certFingerprint: z.string().min(1),
  // `<ip>:<port>`. Required for the joiner to dial the host
  // through KCreate's QUIC transport — the deeplink path is dead
  // without this field.
  ownerSocketAddr: z.string().min(1),
  communityId: z.string().min(1),
  conversationId: z.string().min(1),
  // ISO 8601 UTC.
  issuedAt: z.string().min(1),
});
export type ShareInvite = z.infer<typeof ShareInviteSchema>;

// A "recent KCreate project" entry surfaced by the host so the panel
// can show projects the user has touched recently in the standalone
// KCreate app, scoped to the active community. The host fills this
// from the user's KChat profile metadata; if the host can't supply
// it the array is empty.
export const RecentProjectSchema = z.object({
  projectId: z.string().min(1),
  projectName: z.string().min(1),
  // ISO 8601 UTC.
  lastOpenedAt: z.string().min(1),
  // Optional community scope. `undefined` = visible to all
  // communities; otherwise pin to one community id.
  communityId: z.string().min(1).optional(),
});
export type RecentProject = z.infer<typeof RecentProjectSchema>;

// ----- Response schemas ----------------------------------------------------

const CommunitiesListResponseSchema = z.object({
  communities: z.array(CommunitySchema),
});

const MembersListResponseSchema = z.object({
  members: z.array(CommunityMemberSchema),
});

const MessagesQueryResponseSchema = z.object({
  // The host returns generic chat messages; the extension filters
  // for share-invite cards by content type.
  messages: z.array(
    z.object({
      messageId: z.string().min(1),
      conversationId: z.string().min(1),
      senderJid: z.string().min(1),
      contentType: z.string().min(1),
      // The host has already JSON-decoded the content payload for
      // recognised content types; for others it returns the raw
      // body as a string.
      content: z.unknown(),
      // ISO 8601 UTC.
      postedAt: z.string().min(1),
    }),
  ),
});

const PostMessageResponseSchema = z.object({
  messageId: z.string().min(1),
  postedAt: z.string().min(1),
});

// The host exposes a deterministic helper that maps community id ->
// recent KCreate projects. Returns an empty array when the host has
// no metadata for that community.
const RecentProjectsResponseSchema = z.object({
  projects: z.array(RecentProjectSchema),
});

// ----- Public API ----------------------------------------------------------

// Must match `INVITE_CONTENT_TYPE` in
// `crates/kcreate_kchat_client/src/protocol.rs` — the bridge stamps
// posted invite cards with this string and the panel filters by it.
export const KCREATE_SHARE_INVITE_CONTENT_TYPE = "kcreate.invite.v1";

export async function listMyCommunities(): Promise<Community[]> {
  const r = await invokeProcedure(
    "kchat.query_my_communities",
    {},
    CommunitiesListResponseSchema,
  );
  return r.communities;
}

export async function listMembers(communityId: string): Promise<CommunityMember[]> {
  const r = await invokeProcedure(
    "kchat.query_community_members",
    { communityId },
    MembersListResponseSchema,
  );
  return r.members;
}

/**
 * Read the latest messages in a conversation and pull out the
 * KCreate share-invite cards. Drops malformed payloads instead of
 * throwing so a single bad message doesn't blank the panel.
 */
export async function listShareInvitesInConversation(
  conversationId: string,
  limit = 50,
): Promise<ShareInvite[]> {
  const r = await invokeProcedure(
    "kchat.query_messages",
    { conversationId, limit },
    MessagesQueryResponseSchema,
  );
  const invites: ShareInvite[] = [];
  for (const m of r.messages) {
    if (m.contentType !== KCREATE_SHARE_INVITE_CONTENT_TYPE) {
      continue;
    }
    const parsed = ShareInviteSchema.safeParse(m.content);
    if (parsed.success) {
      invites.push(parsed.data);
    } else {
      console.warn(
        `[kcreate-companion] dropping malformed share-invite ${m.messageId}:`,
        parsed.error.format(),
      );
    }
  }
  return invites;
}

export async function listRecentProjects(
  communityId: string,
): Promise<RecentProject[]> {
  const r = await invokeProcedure(
    "kchat.query_recent_kcreate_projects",
    { communityId },
    RecentProjectsResponseSchema,
  );
  return r.projects;
}

export async function postShareInvite(
  conversationId: string,
  invite: ShareInvite,
): Promise<{ messageId: string; postedAt: string }> {
  return invokeProcedure(
    "kchat.post_message",
    {
      conversationId,
      contentType: KCREATE_SHARE_INVITE_CONTENT_TYPE,
      content: invite,
    },
    PostMessageResponseSchema,
  );
}

/**
 * Build the `kcreate://join?invite=<id>` deeplink for a share-invite
 * card and ask the host to dispatch it. The standalone KCreate
 * desktop app handles the URL via its registered protocol handler.
 */
export async function openInviteInKCreate(invite: ShareInvite): Promise<void> {
  const url = buildJoinDeeplink(invite);
  await openDeeplink(url);
}

/**
 * Build the `kcreate://join?payload=<base64url(json)>` deeplink for
 * a share-invite card.
 *
 * The full JSON payload is embedded in the URL (base64url, no
 * padding) rather than just IDs because the joiner needs the
 * cert fingerprint + socket address + owner public key to dial
 * the host's QUIC transport. Resolving those from the backend
 * would require a `GET /invite/<id>` endpoint that doesn't exist
 * yet; embedding the payload is self-contained and matches the
 * REST-client contract.
 *
 * Maximum URL length is well below typical OS limits (8 KB on
 * Windows, 64 KB on macOS/Linux); a typical invite is ~400 bytes
 * base64-encoded.
 */
export function buildJoinDeeplink(invite: ShareInvite): string {
  const json = JSON.stringify(invite);
  const b64 = Buffer.from(json, "utf8")
    .toString("base64")
    .replaceAll("+", "-")
    .replaceAll("/", "_")
    .replace(/=+$/u, "");
  const params = new URLSearchParams({ payload: b64 });
  return `kcreate://join?${params.toString()}`;
}
