// Phase 9 Block A Task 5 — KChat companion community activity feed.
//
// Surfaces recent design activity from community members by joining
// the host's recent messages stream with the KCreate-stamped
// invite + artifact cards (`kcreate.invite.v1` / `kcreate.artifact.v1`).
// Each entry links to the relevant deeplink (project or artifact).
import { useCallback, useEffect, useState } from "react";
import { openDeeplink } from "./host";
import {
  KCREATE_SHARE_INVITE_CONTENT_TYPE,
  ShareInviteSchema,
  type ShareInvite,
  buildJoinDeeplink,
} from "./store";
import {
  ArtifactCardPayloadSchema,
  KCREATE_ARTIFACT_CONTENT_TYPE,
  buildArtifactDeeplink,
  type ArtifactCardPayload,
} from "./ArtifactCard";
import { invokeProcedure } from "./host";
import { z } from "zod";

interface ActivityFeedProps {
  /** Conversation context (required — feed is conversation-scoped). */
  conversationId: string;
  /** How many recent messages to scan. */
  limit?: number;
  /** Override the deeplink dispatcher (tests). */
  onActivate?: (url: string) => Promise<void>;
}

type ActivityEntry =
  | {
      kind: "invite";
      messageId: string;
      senderJid: string;
      postedAt: string;
      payload: ShareInvite;
    }
  | {
      kind: "artifact";
      messageId: string;
      senderJid: string;
      postedAt: string;
      payload: ArtifactCardPayload;
    };

const MessageSchema = z.object({
  messageId: z.string().min(1),
  conversationId: z.string().min(1),
  senderJid: z.string().min(1),
  contentType: z.string().min(1),
  content: z.unknown(),
  postedAt: z.string().min(1),
});

const MessagesResponseSchema = z.object({
  messages: z.array(MessageSchema),
});

async function queryActivity(
  conversationId: string,
  limit: number,
): Promise<ActivityEntry[]> {
  const r = await invokeProcedure(
    "kchat.query_messages",
    { conversationId, limit },
    MessagesResponseSchema,
  );
  const entries: ActivityEntry[] = [];
  for (const m of r.messages) {
    if (m.contentType === KCREATE_SHARE_INVITE_CONTENT_TYPE) {
      const parsed = ShareInviteSchema.safeParse(m.content);
      if (parsed.success) {
        entries.push({
          kind: "invite",
          messageId: m.messageId,
          senderJid: m.senderJid,
          postedAt: m.postedAt,
          payload: parsed.data,
        });
      }
    } else if (m.contentType === KCREATE_ARTIFACT_CONTENT_TYPE) {
      const parsed = ArtifactCardPayloadSchema.safeParse(m.content);
      if (parsed.success) {
        entries.push({
          kind: "artifact",
          messageId: m.messageId,
          senderJid: m.senderJid,
          postedAt: m.postedAt,
          payload: parsed.data,
        });
      }
    }
  }
  return entries.sort(
    (a, b) => new Date(b.postedAt).getTime() - new Date(a.postedAt).getTime(),
  );
}

function formatActor(jid: string): string {
  // KChat JIDs look like `alice@kchat`; show the localpart.
  const at = jid.indexOf("@");
  return at > 0 ? jid.slice(0, at) : jid;
}

function formatRelative(iso: string, now: Date): string {
  const t = new Date(iso).getTime();
  if (Number.isNaN(t)) return iso;
  const diffMs = now.getTime() - t;
  const minutes = Math.floor(diffMs / 60_000);
  if (minutes < 1) return "just now";
  if (minutes < 60) return `${minutes} min ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} h ago`;
  const days = Math.floor(hours / 24);
  return `${days} d ago`;
}

export function ActivityFeed({
  conversationId,
  limit = 30,
  onActivate,
}: ActivityFeedProps): JSX.Element {
  const [entries, setEntries] = useState<ActivityEntry[]>([]);
  const [phase, setPhase] = useState<"idle" | "loading" | "loaded" | "error">(
    "idle",
  );
  const [error, setError] = useState<string | undefined>(undefined);
  // Wall-clock snapshot refreshed alongside each successful fetch so
  // the "X min ago" labels track newly-arriving entries without
  // re-rendering every tick.
  const [now, setNow] = useState<Date>(() => new Date());

  const refresh = useCallback(async () => {
    setPhase("loading");
    setError(undefined);
    try {
      const next = await queryActivity(conversationId, limit);
      setEntries(next);
      setNow(new Date());
      setPhase("loaded");
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setError(message);
      setPhase("error");
    }
  }, [conversationId, limit]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const handleActivate = useCallback(
    async (url: string) => {
      try {
        await (onActivate ?? openDeeplink)(url);
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err);
        setError(message);
      }
    },
    [onActivate],
  );

  return (
    <section data-testid="kcreate-activity-feed" style={containerStyle}>
      <header style={headerStyle}>
        <h3 style={titleStyle}>Community activity</h3>
        <button
          type="button"
          onClick={() => void refresh()}
          disabled={phase === "loading"}
          style={refreshButtonStyle}
          data-testid="kcreate-activity-refresh"
        >
          {phase === "loading" ? "…" : "Refresh"}
        </button>
      </header>

      {error !== undefined && (
        <p role="alert" style={errorStyle} data-testid="kcreate-activity-error">
          {error}
        </p>
      )}

      {entries.length === 0 && phase === "loaded" ? (
        <p style={emptyStyle}>No recent KCreate activity here.</p>
      ) : (
        <ol style={listStyle}>
          {entries.map((e) => (
            <li
              key={e.messageId}
              style={itemStyle}
              data-testid="kcreate-activity-entry"
              data-kind={e.kind}
            >
              {e.kind === "invite" ? (
                <button
                  type="button"
                  onClick={() => void handleActivate(buildJoinDeeplink(e.payload))}
                  style={itemButtonStyle}
                >
                  <span style={actorStyle}>{formatActor(e.senderJid)}</span>
                  <span> shared a project: </span>
                  <strong>{e.payload.projectName}</strong>
                  <span style={relativeStyle}>
                    {" · "}
                    {formatRelative(e.postedAt, now)}
                  </span>
                </button>
              ) : (
                <button
                  type="button"
                  onClick={() => void handleActivate(buildArtifactDeeplink(e.payload))}
                  style={itemButtonStyle}
                >
                  <span style={actorStyle}>{formatActor(e.senderJid)}</span>
                  <span> posted an artifact: </span>
                  <strong>{e.payload.artifactName}</strong>
                  <span style={relativeStyle}>
                    {" · "}
                    {formatRelative(e.postedAt, now)}
                  </span>
                </button>
              )}
            </li>
          ))}
        </ol>
      )}
    </section>
  );
}

const containerStyle: React.CSSProperties = {
  padding: 10,
  background: "#0e1013",
  color: "#e7e8ea",
  fontSize: 13,
  fontFamily:
    'system-ui, -apple-system, "Segoe UI", Roboto, Helvetica, Arial, sans-serif',
};

const headerStyle: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  justifyContent: "space-between",
  marginBottom: 8,
};

const titleStyle: React.CSSProperties = {
  margin: 0,
  fontSize: 14,
  fontWeight: 600,
};

const refreshButtonStyle: React.CSSProperties = {
  background: "#1f2329",
  color: "#e7e8ea",
  border: "1px solid #2c313a",
  borderRadius: 4,
  padding: "4px 8px",
  fontSize: 12,
  cursor: "pointer",
};

const errorStyle: React.CSSProperties = {
  margin: "4px 0",
  color: "#ff8585",
  fontSize: 12,
};

const emptyStyle: React.CSSProperties = {
  margin: "4px 0",
  color: "#9aa0a6",
  fontStyle: "italic",
  fontSize: 12,
};

const listStyle: React.CSSProperties = {
  listStyle: "none",
  margin: 0,
  padding: 0,
  display: "flex",
  flexDirection: "column",
  gap: 4,
};

const itemStyle: React.CSSProperties = { padding: 0 };

const itemButtonStyle: React.CSSProperties = {
  width: "100%",
  textAlign: "left",
  background: "#13161b",
  border: "1px solid #1f2329",
  borderRadius: 6,
  padding: "6px 10px",
  color: "#e7e8ea",
  cursor: "pointer",
  fontSize: 12,
};

const actorStyle: React.CSSProperties = { fontWeight: 600, color: "#79b8ff" };

const relativeStyle: React.CSSProperties = { color: "#9aa0a6", fontSize: 11 };
