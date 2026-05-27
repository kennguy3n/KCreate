// McpSettingsPanel — control the loopback MCP server (start / stop)
// and manage per-(client, tool) permissions persisted to disk by
// `kcreate_mcp::McpPermissionStore`.

import { useCallback, useEffect, useMemo, useState } from "react";

import type {
  McpPermission,
  McpPermissionGrant,
  McpStatus,
} from "../../../shared/scene";
import { colors, radius, spacing } from "../styles/tokens";

const GRANT_OPTIONS: McpPermissionGrant[] = ["once", "always", "denied"];

export interface McpSettingsPanelProps {
  onStatus?: (msg: string | null) => void;
}

export function McpSettingsPanel({
  onStatus,
}: McpSettingsPanelProps): JSX.Element {
  const [status, setStatus] = useState<McpStatus | null>(null);
  const [perms, setPerms] = useState<McpPermission[]>([]);
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(async () => {
    try {
      const [s, p] = await Promise.all([
        window.kcreate.mcpPermission.status(),
        window.kcreate.mcpPermission.list(),
      ]);
      setStatus(s);
      setPerms(p);
    } catch (e) {
      onStatus?.(`mcp: ${errMsg(e)}`);
    }
  }, [onStatus]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const startServer = useCallback(async () => {
    setBusy(true);
    try {
      const port = await window.kcreate.mcp.start();
      onStatus?.(`MCP: listening on 127.0.0.1:${port}.`);
      await refresh();
    } catch (e) {
      onStatus?.(`mcp start failed: ${errMsg(e)}`);
    } finally {
      setBusy(false);
    }
  }, [onStatus, refresh]);

  const stopServer = useCallback(async () => {
    setBusy(true);
    try {
      await window.kcreate.mcp.stop();
      onStatus?.("MCP: stopped.");
      await refresh();
    } catch (e) {
      onStatus?.(`mcp stop failed: ${errMsg(e)}`);
    } finally {
      setBusy(false);
    }
  }, [onStatus, refresh]);

  const setGrant = useCallback(
    async (clientId: string, toolName: string, grant: McpPermissionGrant) => {
      try {
        await window.kcreate.mcpPermission.grant(clientId, toolName, grant);
        await refresh();
      } catch (e) {
        onStatus?.(`grant failed: ${errMsg(e)}`);
      }
    },
    [onStatus, refresh],
  );

  const revoke = useCallback(
    async (clientId: string, toolName: string) => {
      try {
        await window.kcreate.mcpPermission.revoke(clientId, toolName);
        await refresh();
      } catch (e) {
        onStatus?.(`revoke failed: ${errMsg(e)}`);
      }
    },
    [onStatus, refresh],
  );

  const grouped = useMemo(() => groupByClient(perms), [perms]);

  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        gap: spacing.md,
      }}
    >
      <header
        style={{
          display: "flex",
          alignItems: "center",
          gap: spacing.sm,
        }}
      >
        <h2 style={{ margin: 0, fontSize: 14, fontWeight: 600 }}>
          MCP server
        </h2>
        <ServerStatusBadge status={status} />
        <div style={{ marginLeft: "auto", display: "flex", gap: 6 }}>
          <button
            type="button"
            onClick={() => {
              void startServer();
            }}
            disabled={busy || status?.running === true}
            style={primaryButton(busy || status?.running === true)}
          >
            Start
          </button>
          <button
            type="button"
            onClick={() => {
              void stopServer();
            }}
            disabled={busy || status?.running !== true}
            style={secondaryButton(busy || status?.running !== true)}
          >
            Stop
          </button>
        </div>
      </header>

      <h3 style={{ margin: 0, fontSize: 13, fontWeight: 600 }}>Permissions</h3>
      {grouped.size === 0 ? (
        <p style={{ margin: 0, fontSize: 12, color: colors.textMuted }}>
          No permissions granted yet. Clients will request permission on
          their first tool call.
        </p>
      ) : (
        Array.from(grouped.entries()).map(([clientId, entries]) => (
          <ClientBlock
            key={clientId}
            clientId={clientId}
            entries={entries}
            onGrant={setGrant}
            onRevoke={revoke}
          />
        ))
      )}
    </div>
  );
}

function ServerStatusBadge({
  status,
}: {
  status: McpStatus | null;
}): JSX.Element {
  if (!status) {
    return (
      <span style={{ fontSize: 11, color: colors.textMuted }}>checking…</span>
    );
  }
  if (status.running) {
    return (
      <span
        style={{
          padding: "1px 6px",
          background: "#16A34A22",
          color: "#16A34A",
          borderRadius: radius.pill,
          fontSize: 10,
          fontWeight: 600,
        }}
      >
        running · 127.0.0.1:{status.port}
      </span>
    );
  }
  return (
    <span
      style={{
        padding: "1px 6px",
        background: colors.bgSoft,
        color: colors.textMuted,
        borderRadius: radius.pill,
        fontSize: 10,
        fontWeight: 600,
      }}
    >
      stopped
    </span>
  );
}

function ClientBlock({
  clientId,
  entries,
  onGrant,
  onRevoke,
}: {
  clientId: string;
  entries: McpPermission[];
  onGrant: (
    clientId: string,
    toolName: string,
    grant: McpPermissionGrant,
  ) => Promise<void>;
  onRevoke: (clientId: string, toolName: string) => Promise<void>;
}): JSX.Element {
  return (
    <section
      style={{
        border: `1px solid ${colors.border}`,
        borderRadius: radius.card,
        padding: spacing.md,
        display: "flex",
        flexDirection: "column",
        gap: spacing.sm,
      }}
    >
      <header
        style={{ display: "flex", alignItems: "center", gap: spacing.sm }}
      >
        <h4 style={{ margin: 0, fontSize: 12, fontWeight: 600 }}>
          {clientId}
        </h4>
        <span
          style={{
            fontSize: 11,
            color: colors.textMuted,
            marginLeft: "auto",
          }}
        >
          {entries.length} tools
        </span>
      </header>
      <table style={{ width: "100%", borderCollapse: "collapse" }}>
        <thead>
          <tr>
            <th style={th}>Tool</th>
            <th style={th}>Grant</th>
            <th style={th}>Granted</th>
            <th style={th}></th>
          </tr>
        </thead>
        <tbody>
          {entries.map((entry) => (
            <tr key={entry.tool_name}>
              <td style={td}>{entry.tool_name}</td>
              <td style={td}>
                <select
                  value={entry.granted}
                  onChange={(e) => {
                    void onGrant(
                      clientId,
                      entry.tool_name,
                      e.target.value as McpPermissionGrant,
                    );
                  }}
                  style={selectStyle}
                >
                  {GRANT_OPTIONS.map((g) => (
                    <option key={g} value={g}>
                      {g}
                    </option>
                  ))}
                </select>
              </td>
              <td style={td}>
                <span style={{ fontSize: 10, color: colors.textMuted }}>
                  {new Date(entry.granted_at).toLocaleString()}
                </span>
              </td>
              <td style={td}>
                <button
                  type="button"
                  onClick={() => {
                    void onRevoke(clientId, entry.tool_name);
                  }}
                  style={{
                    padding: "2px 6px",
                    background: "transparent",
                    color: colors.danger,
                    border: `1px solid ${colors.dangerBorder}`,
                    borderRadius: radius.pill,
                    cursor: "pointer",
                    fontSize: 10,
                    fontWeight: 600,
                  }}
                >
                  Revoke
                </button>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </section>
  );
}

function groupByClient(perms: McpPermission[]): Map<string, McpPermission[]> {
  const out = new Map<string, McpPermission[]>();
  for (const p of perms) {
    const list = out.get(p.client_id);
    if (list) list.push(p);
    else out.set(p.client_id, [p]);
  }
  return out;
}

function primaryButton(disabled: boolean): React.CSSProperties {
  return {
    padding: "4px 12px",
    background: disabled ? colors.bgSoft : colors.accent,
    color: disabled ? colors.textMuted : colors.textInverse,
    border: "none",
    borderRadius: radius.pill,
    cursor: disabled ? "default" : "pointer",
    fontSize: 11,
    fontWeight: 600,
  };
}

function secondaryButton(disabled: boolean): React.CSSProperties {
  return {
    padding: "4px 12px",
    background: "transparent",
    color: disabled ? colors.textMuted : colors.text,
    border: `1px solid ${colors.border}`,
    borderRadius: radius.pill,
    cursor: disabled ? "default" : "pointer",
    fontSize: 11,
    fontWeight: 600,
  };
}

const th: React.CSSProperties = {
  textAlign: "left",
  padding: "4px 6px",
  fontSize: 11,
  color: colors.textMuted,
  fontWeight: 600,
  borderBottom: `1px solid ${colors.border}`,
};

const td: React.CSSProperties = {
  padding: "4px 6px",
  fontSize: 12,
  borderBottom: `1px solid ${colors.border}`,
};

const selectStyle: React.CSSProperties = {
  padding: "2px 6px",
  fontSize: 11,
  border: `1px solid ${colors.border}`,
  borderRadius: 6,
  background: colors.bg,
};

function errMsg(e: unknown): string {
  if (e instanceof Error) return e.message;
  return String(e);
}
