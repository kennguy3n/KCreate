import { useCallback, useEffect, useState } from "react";
import { InviteCard } from "./InviteCard";
import {
  listMyCommunities,
  listRecentProjects,
  listShareInvitesInConversation,
  type Community,
  type RecentProject,
  type ShareInvite,
} from "./store";

interface PanelProps {
  // The host passes the active community + conversation context
  // (mirrors §6.18 "Active view bridge" in the host architecture
  // doc). Both are optional because the panel may be open outside a
  // community context.
  activeCommunityId?: string;
  activeConversationId?: string;
}

interface PanelState {
  communities: Community[];
  selectedCommunityId: string | undefined;
  recentProjects: RecentProject[];
  invites: ShareInvite[];
  loadState: "idle" | "loading" | "loaded";
  error: string | undefined;
}

const INITIAL: PanelState = {
  communities: [],
  selectedCommunityId: undefined,
  recentProjects: [],
  invites: [],
  loadState: "idle",
  error: undefined,
};

export function Panel({
  activeCommunityId,
  activeConversationId,
}: PanelProps): JSX.Element {
  const [state, setState] = useState<PanelState>(INITIAL);

  const refresh = useCallback(async () => {
    setState((s) => ({ ...s, loadState: "loading", error: undefined }));
    try {
      const communities = await listMyCommunities();
      const selected =
        communities.find((c) => c.id === activeCommunityId)?.id ??
        communities[0]?.id;
      const [recentProjects, invites] = await Promise.all([
        selected ? listRecentProjects(selected) : Promise.resolve([]),
        activeConversationId
          ? listShareInvitesInConversation(activeConversationId)
          : Promise.resolve([]),
      ]);
      setState({
        communities,
        selectedCommunityId: selected,
        recentProjects,
        invites,
        loadState: "loaded",
        error: undefined,
      });
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setState((s) => ({ ...s, loadState: "loaded", error: message }));
    }
  }, [activeCommunityId, activeConversationId]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  return (
    <section data-testid="kcreate-companion-panel" style={panelStyle}>
      <header style={panelHeaderStyle}>
        <h2 style={panelTitleStyle}>KCreate</h2>
        <button
          type="button"
          onClick={() => void refresh()}
          disabled={state.loadState === "loading"}
          style={refreshButtonStyle}
          data-testid="kcreate-refresh"
        >
          {state.loadState === "loading" ? "…" : "Refresh"}
        </button>
      </header>

      {state.error !== undefined && (
        <p role="alert" style={panelErrorStyle} data-testid="kcreate-panel-error">
          {state.error}
        </p>
      )}

      <h3 style={sectionTitleStyle}>Recent projects</h3>
      {state.recentProjects.length === 0 ? (
        <p style={emptyStyle}>No recent projects in this community.</p>
      ) : (
        <ul style={recentListStyle}>
          {state.recentProjects.map((p) => (
            <li
              key={p.projectId}
              style={recentItemStyle}
              data-testid="kcreate-recent-project"
              data-project-id={p.projectId}
            >
              <span>{p.projectName}</span>
              <span style={timestampStyle}>{formatTimestamp(p.lastOpenedAt)}</span>
            </li>
          ))}
        </ul>
      )}

      <h3 style={sectionTitleStyle}>Share invites</h3>
      {state.invites.length === 0 ? (
        <p style={emptyStyle}>
          {activeConversationId
            ? "No KCreate shares in this conversation."
            : "Open a conversation to see KCreate shares."}
        </p>
      ) : (
        <ol style={inviteListStyle}>
          {state.invites.map((invite) => (
            // The canonical wire format doesn't carry an `inviteId`;
            // a `projectId + issuedAt` pair uniquely identifies a
            // share-invite card within a conversation (each share
            // mints a fresh timestamp on the host side).
            <li
              key={`${invite.projectId}:${invite.issuedAt}`}
              style={inviteItemStyle}
            >
              <InviteCard invite={invite} />
            </li>
          ))}
        </ol>
      )}
    </section>
  );
}

function formatTimestamp(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) {
    return iso;
  }
  return d.toLocaleString();
}

const panelStyle = {
  padding: 12,
  background: "#0e1013",
  color: "#e7e8ea",
  fontSize: 13,
  fontFamily:
    'system-ui, -apple-system, "Segoe UI", Roboto, Helvetica, Arial, sans-serif',
  minHeight: "100%",
  boxSizing: "border-box" as const,
};

const panelHeaderStyle = {
  display: "flex",
  alignItems: "center",
  justifyContent: "space-between",
  marginBottom: 10,
};

const panelTitleStyle = {
  margin: 0,
  fontSize: 16,
  fontWeight: 700,
};

const refreshButtonStyle = {
  background: "transparent",
  color: "#3a8ee6",
  border: "1px solid #3a8ee6",
  borderRadius: 4,
  padding: "2px 8px",
  cursor: "pointer",
  fontSize: 12,
};

const panelErrorStyle = {
  padding: "6px 8px",
  borderRadius: 4,
  background: "#3a1f23",
  color: "#ff8a8a",
  fontSize: 12,
  marginBottom: 8,
};

const sectionTitleStyle = {
  margin: "12px 0 6px",
  fontSize: 12,
  fontWeight: 600,
  textTransform: "uppercase" as const,
  letterSpacing: 0.5,
  opacity: 0.7,
};

const emptyStyle = {
  margin: 0,
  fontSize: 12,
  opacity: 0.6,
};

const recentListStyle = {
  listStyle: "none",
  margin: 0,
  padding: 0,
};

const recentItemStyle = {
  display: "flex",
  justifyContent: "space-between",
  alignItems: "center",
  padding: "4px 0",
  borderBottom: "1px solid #1c1f23",
};

const timestampStyle = {
  fontSize: 11,
  opacity: 0.6,
};

const inviteListStyle = {
  listStyle: "none",
  margin: 0,
  padding: 0,
  display: "grid",
  gap: 8,
};

const inviteItemStyle = {
  margin: 0,
};
