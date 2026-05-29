// Phase 9 Block A Task 2 — KChat companion project browser sidebar.
//
// Shows the user's recent KCreate projects, scoped to the active
// community. Clicking a card asks the host to dispatch a
// `kcreate://open?project_id=…` deeplink which the standalone
// KCreate desktop app handles via its registered protocol handler.
//
// The host owns the project metadata (it caches the user's recent
// projects per community on the KChat profile); see
// `kchat.query_recent_kcreate_projects` in the extension manifest.
import { useCallback, useEffect, useMemo, useState } from "react";
import { openDeeplink } from "./host";
import { listMyCommunities, listRecentProjects, type Community, type RecentProject } from "./store";

interface ProjectBrowserPanelProps {
  /** The host's "active view" community context; defaults to the first joined community when unset. */
  activeCommunityId?: string;
  /** Override the deeplink dispatcher (tests). */
  onOpen?: (projectId: string) => Promise<void>;
}

type LoadPhase = "idle" | "loading" | "loaded" | "error";

interface PanelState {
  phase: LoadPhase;
  communities: Community[];
  selectedCommunityId: string | undefined;
  projects: RecentProject[];
  error: string | undefined;
}

const INITIAL: PanelState = {
  phase: "idle",
  communities: [],
  selectedCommunityId: undefined,
  projects: [],
  error: undefined,
};

/** Build the canonical `kcreate://open?project_id=<uuid>` deeplink. */
export function buildOpenProjectDeeplink(projectId: string): string {
  const params = new URLSearchParams({ project_id: projectId });
  return `kcreate://open?${params.toString()}`;
}

async function defaultOpenProject(projectId: string): Promise<void> {
  await openDeeplink(buildOpenProjectDeeplink(projectId));
}

export function ProjectBrowserPanel({
  activeCommunityId,
  onOpen,
}: ProjectBrowserPanelProps): JSX.Element {
  const [state, setState] = useState<PanelState>(INITIAL);

  const refresh = useCallback(async () => {
    setState((s) => ({ ...s, phase: "loading", error: undefined }));
    try {
      const communities = await listMyCommunities();
      const selected =
        communities.find((c) => c.id === activeCommunityId)?.id ??
        communities[0]?.id;
      const projects = selected ? await listRecentProjects(selected) : [];
      setState({
        phase: "loaded",
        communities,
        selectedCommunityId: selected,
        projects,
        error: undefined,
      });
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setState((s) => ({ ...s, phase: "error", error: message }));
    }
  }, [activeCommunityId]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const onPick = useCallback(
    async (projectId: string) => {
      try {
        await (onOpen ?? defaultOpenProject)(projectId);
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err);
        setState((s) => ({ ...s, phase: "error", error: message }));
      }
    },
    [onOpen],
  );

  const sortedProjects = useMemo(
    () =>
      [...state.projects].sort(
        (a, b) =>
          new Date(b.lastOpenedAt).getTime() -
          new Date(a.lastOpenedAt).getTime(),
      ),
    [state.projects],
  );

  return (
    <section data-testid="kcreate-project-browser" style={panelStyle}>
      <header style={headerStyle}>
        <h3 style={titleStyle}>Recent KCreate projects</h3>
        <button
          type="button"
          onClick={() => void refresh()}
          disabled={state.phase === "loading"}
          style={refreshButtonStyle}
          data-testid="kcreate-project-browser-refresh"
        >
          {state.phase === "loading" ? "…" : "Refresh"}
        </button>
      </header>

      {state.error !== undefined && (
        <p role="alert" style={errorStyle} data-testid="kcreate-project-browser-error">
          {state.error}
        </p>
      )}

      {sortedProjects.length === 0 && state.phase === "loaded" ? (
        <p style={emptyStyle}>No recent projects in this community.</p>
      ) : (
        <ul style={listStyle} data-testid="kcreate-project-browser-list">
          {sortedProjects.map((p) => (
            <li
              key={p.projectId}
              style={itemStyle}
              data-testid="kcreate-project-card"
              data-project-id={p.projectId}
            >
              <button
                type="button"
                onClick={() => void onPick(p.projectId)}
                style={itemButtonStyle}
              >
                <span style={projectNameStyle}>{p.projectName}</span>
                <span style={timestampStyle}>
                  Last opened {formatTimestamp(p.lastOpenedAt)}
                </span>
              </button>
            </li>
          ))}
        </ul>
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
  padding: 10,
  background: "#0e1013",
  color: "#e7e8ea",
  fontSize: 13,
  fontFamily:
    'system-ui, -apple-system, "Segoe UI", Roboto, Helvetica, Arial, sans-serif',
} as const;

const headerStyle = {
  display: "flex",
  alignItems: "center",
  justifyContent: "space-between",
  marginBottom: 8,
} as const;

const titleStyle = { margin: 0, fontSize: 14, fontWeight: 600 } as const;

const refreshButtonStyle = {
  background: "#1f2329",
  color: "#e7e8ea",
  border: "1px solid #2c313a",
  borderRadius: 4,
  padding: "4px 8px",
  fontSize: 12,
  cursor: "pointer",
} as const;

const errorStyle = {
  margin: "4px 0",
  color: "#ff8585",
  fontSize: 12,
} as const;

const emptyStyle = {
  margin: "4px 0",
  color: "#9aa0a6",
  fontStyle: "italic" as const,
  fontSize: 12,
} as const;

const listStyle = {
  listStyle: "none" as const,
  margin: 0,
  padding: 0,
  display: "flex",
  flexDirection: "column" as const,
  gap: 6,
} as const;

const itemStyle = { padding: 0 } as const;

const itemButtonStyle = {
  width: "100%",
  textAlign: "left" as const,
  background: "#13161b",
  border: "1px solid #1f2329",
  borderRadius: 6,
  padding: "8px 10px",
  color: "#e7e8ea",
  cursor: "pointer",
  display: "flex",
  flexDirection: "column" as const,
  gap: 4,
} as const;

const projectNameStyle = { fontWeight: 600 } as const;

const timestampStyle = { color: "#9aa0a6", fontSize: 11 } as const;
