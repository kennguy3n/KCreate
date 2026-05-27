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
