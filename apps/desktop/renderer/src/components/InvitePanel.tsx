// InvitePanel — Phase 7 (Task 10) document-share invite acceptance UI.
//
// Shows pending invites that arrive through a KChat Desktop conversation.
// The user pastes the invite JSON (or it arrives via the clipboard) and
// the panel validates community match + sender membership, then dials
// the owner peer through the regular `session.join()` path.
//
// Layout:
//  ┌──────────────────────────────────┐
//  │ 📩 Pending invite                │
//  │ Project: "Ken's wireframes"      │
//  │ Owner:   Ken (admin)             │
//  │ Community: Design Team           │
//  │ [Join Session]  [Dismiss]        │
//  └──────────────────────────────────┘
//
// The panel is rendered inside `EditorPage` alongside the existing
// `PresencePanel` when `window.kcreate.kchatBackend.available()` is
// true and the user has an active community selected.

import { useCallback, useEffect, useState } from "react";

import type {
  KChatAcceptedInvite,
  KChatMembershipStatus,
} from "../../../shared/scene";

// ---------------------------------------------------------------------------
// Invite payload shape (mirrors InviteCardPayload from the Rust side).
// We keep a local type so the panel doesn't import protocol internals.
// ---------------------------------------------------------------------------
interface InvitePayload {
  schemaVersion: number;
  projectId: string;
  projectName: string;
  ownerPeerId: string;
  ownerPublicKey: string;
  ownerDisplayName: string;
  certFingerprint: string;
  ownerSocketAddr: string;
  communityId: string;
  conversationId: string;
  issuedAt: string;
}

/**
 * Decode a `kcreate://join?payload=<base64url(invite_json)>` URL
 * into the raw invite JSON, suitable for stuffing into the panel's
 * textarea (the standard validation effect picks it up from there).
 *
 * Returns `null` for any non-`kcreate://join` URL or any URL whose
 * payload is missing / not base64url / not valid JSON. We
 * deliberately don't surface a parse error to the user here — a
 * stray click on a malformed link shouldn't yank focus to the
 * panel with an error message; the panel only reacts when there's
 * an actual invite to act on.
 *
 * Mirrors `buildJoinDeeplink` in
 * `apps/kchat-extension/src/store.ts` — keep both sides in sync if
 * the URL grammar changes.
 */
function decodeJoinDeeplink(url: string): string | null {
  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    return null;
  }
  if (parsed.protocol !== "kcreate:") return null;
  // `new URL("kcreate://join?...")` puts "join" in `hostname`, but
  // a few Windows shell variations route the whole tail into
  // `pathname` with no hostname — accept both.
  const route = parsed.hostname || parsed.pathname.replace(/^\/+/u, "");
  if (route !== "join") return null;
  const payloadParam = parsed.searchParams.get("payload");
  if (!payloadParam) return null;
  // base64url -> base64 -> bytes -> UTF-8 string.
  const padded =
    payloadParam.replaceAll("-", "+").replaceAll("_", "/") +
    "=".repeat((4 - (payloadParam.length % 4)) % 4);
  let json: string;
  try {
    // `atob` decodes base64 to a byte-string; we then re-decode
    // that as UTF-8 so non-ASCII characters in the payload survive
    // (e.g. an owner display name with accents).
    const binary = atob(padded);
    const bytes = Uint8Array.from(binary, (c) => c.charCodeAt(0));
    json = new TextDecoder("utf-8").decode(bytes);
  } catch {
    return null;
  }
  // Sanity-check that the decoded blob is JSON before handing it
  // off to the panel. A non-JSON payload would just bounce off the
  // textarea validator, but failing here keeps the visible error
  // bracketed to the panel.
  try {
    JSON.parse(json);
  } catch {
    return null;
  }
  return json;
}

function tryParseInvite(raw: string): InvitePayload | null {
  try {
    const obj = JSON.parse(raw) as Record<string, unknown>;
    if (
      typeof obj.schemaVersion === "number" &&
      typeof obj.projectId === "string" &&
      typeof obj.projectName === "string" &&
      typeof obj.ownerPeerId === "string" &&
      typeof obj.ownerPublicKey === "string" &&
      typeof obj.ownerDisplayName === "string" &&
      typeof obj.certFingerprint === "string" &&
      typeof obj.ownerSocketAddr === "string" &&
      typeof obj.communityId === "string" &&
      typeof obj.conversationId === "string" &&
      typeof obj.issuedAt === "string"
    ) {
      return obj as unknown as InvitePayload;
    }
  } catch {
    // Not valid JSON — fall through.
  }
  return null;
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

interface InvitePanelProps {
  /** Currently-installed KChat membership. Used to gate the panel
   *  and cross-check community match before showing the invite. */
  membership: KChatMembershipStatus;
  /** Called after a successful join so the parent can refresh the
   *  session state (e.g. update the presence panel). */
  onJoined?: (result: KChatAcceptedInvite) => void;
}

export function InvitePanel({ membership, onJoined }: InvitePanelProps) {
  const [raw, setRaw] = useState("");
  const [pending, setPending] = useState<InvitePayload | null>(null);
  const [joining, setJoining] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [success, setSuccess] = useState<KChatAcceptedInvite | null>(null);

  // Parse the raw input whenever it changes.
  useEffect(() => {
    if (!raw.trim()) {
      setPending(null);
      setError(null);
      return;
    }
    const parsed = tryParseInvite(raw);
    if (!parsed) {
      setPending(null);
      setError("Not a valid KCreate invite JSON.");
      return;
    }
    // Community mismatch check (client-side preview — the bridge
    // re-checks on accept).
    if (membership.groupId && parsed.communityId !== membership.groupId) {
      setPending(null);
      setError(
        `Invite is for community "${parsed.communityId}" but you are ` +
          `signed into "${membership.groupId}".`,
      );
      return;
    }
    setError(null);
    setPending(parsed);
  }, [raw, membership.groupId]);

  // Phase 7 (Block E): when KChat Desktop fires a
  // `kcreate://join?payload=<base64url(json)>` deeplink the main
  // process forwards it on `kcreate/deeplink/received`. Decode the
  // payload and stuff it into the raw textarea — the validation
  // effect above takes care of the rest. The community mismatch
  // check still fires, and accept-on-click is left to the user so
  // they retain a confirmation step before dialling the host.
  useEffect(() => {
    const unsubscribe = window.kcreate.deeplink.onUrl((url) => {
      const decoded = decodeJoinDeeplink(url);
      if (decoded !== null) {
        setRaw(decoded);
      }
    });
    return unsubscribe;
  }, []);

  const handleAccept = useCallback(async () => {
    if (!raw.trim()) return;
    setJoining(true);
    setError(null);
    try {
      const result = await window.kcreate.kchatBackend.acceptInvite(raw);
      setSuccess(result);
      setPending(null);
      setRaw("");
      onJoined?.(result);
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : String(err);
      setError(msg);
    } finally {
      setJoining(false);
    }
  }, [raw, onJoined]);

  const handleDismiss = useCallback(() => {
    setRaw("");
    setPending(null);
    setError(null);
    setSuccess(null);
  }, []);

  // Gate: only render when the user is signed into a KChat community.
  if (membership.locked) return null;

  return (
    <div
      style={{
        padding: "12px 16px",
        borderRadius: 8,
        border: "1px solid var(--border-secondary, #333)",
        background: "var(--surface-secondary, #1e1e1e)",
        fontFamily: "inherit",
        fontSize: 13,
      }}
    >
      <div style={{ fontWeight: 600, marginBottom: 8 }}>
        Accept invite
      </div>

      {success && (
        <div
          style={{
            padding: "8px 12px",
            background: "var(--status-success-bg, #1a3a2a)",
            borderRadius: 6,
            marginBottom: 8,
          }}
        >
          Joined <strong>{success.ownerDisplayName}</strong>&apos;s project{" "}
          &ldquo;{success.projectName}&rdquo;.
        </div>
      )}

      <textarea
        rows={4}
        placeholder="Paste invite JSON from a KChat conversation..."
        value={raw}
        onChange={(e) => setRaw(e.target.value)}
        disabled={joining}
        style={{
          width: "100%",
          fontFamily: "monospace",
          fontSize: 11,
          padding: 8,
          borderRadius: 6,
          border: "1px solid var(--border-primary, #444)",
          background: "var(--surface-primary, #111)",
          color: "inherit",
          resize: "vertical",
        }}
      />

      {error && (
        <div
          style={{
            color: "var(--status-error, #f44)",
            fontSize: 12,
            marginTop: 4,
          }}
        >
          {error}
        </div>
      )}

      {pending && (
        <div
          style={{
            marginTop: 8,
            padding: "8px 12px",
            background: "var(--surface-tertiary, #262626)",
            borderRadius: 6,
          }}
        >
          <div>
            <strong>Project:</strong> {pending.projectName}
          </div>
          <div>
            <strong>Owner:</strong> {pending.ownerDisplayName}
          </div>
          <div>
            <strong>Community:</strong> {pending.communityId}
          </div>
          <div style={{ fontSize: 11, opacity: 0.6, marginTop: 2 }}>
            Issued: {new Date(pending.issuedAt).toLocaleString()}
          </div>
        </div>
      )}

      <div style={{ display: "flex", gap: 8, marginTop: 8 }}>
        <button
          onClick={handleAccept}
          disabled={!pending || joining}
          style={{
            flex: 1,
            padding: "6px 12px",
            borderRadius: 6,
            border: "none",
            background:
              pending && !joining
                ? "var(--accent, #3b82f6)"
                : "var(--surface-tertiary, #333)",
            color: "white",
            cursor: pending && !joining ? "pointer" : "not-allowed",
            fontWeight: 600,
            fontSize: 12,
          }}
        >
          {joining ? "Joining..." : "Join Session"}
        </button>
        <button
          onClick={handleDismiss}
          disabled={joining}
          style={{
            padding: "6px 12px",
            borderRadius: 6,
            border: "1px solid var(--border-primary, #444)",
            background: "transparent",
            color: "inherit",
            cursor: "pointer",
            fontSize: 12,
          }}
        >
          Dismiss
        </button>
      </div>
    </div>
  );
}
