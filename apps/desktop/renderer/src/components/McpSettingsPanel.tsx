// McpSettingsPanel — govern the loopback MCP automation server.
//
// Three responsibilities, all over `window.kcreate.mcp*`:
//   * lifecycle — start / stop the loopback-only `tiny_http` server and
//     show where it is bound (always 127.0.0.1, never the network);
//   * the master switch — a single kill-switch that refuses *every*
//     tool call while leaving the per-tool grants intact, so an
//     operator can pause automation without losing their decisions;
//   * permission governance — an approval inbox for tool calls a client
//     attempted but that have no decision on record yet (Allow once /
//     Always / Deny), plus an audit of every granted scope grouped by
//     client with per-tool revoke.
//
// Pending prompts and server status are polled on a short interval so a
// tool call an agent makes surfaces here without the operator having to
// reopen the panel.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import type {
  McpPendingRequest,
  McpPermission,
  McpPermissionGrant,
  McpStatus,
} from "../../../shared/scene";
import { colors, radius, spacing } from "../styles/tokens";

const GRANT_OPTIONS: McpPermissionGrant[] = ["once", "always", "denied"];

/// How often (ms) to re-poll status + the pending-prompt inbox so a
/// tool call an external agent makes appears without manual refresh.
/// Loopback + single-tenant, so this is cheap; it is paused while the
/// server is stopped (nothing can enqueue a prompt then).
const POLL_INTERVAL_MS = 2500;

export interface McpSettingsPanelProps {
  onStatus?: (msg: string | null) => void;
}

export function McpSettingsPanel({
  onStatus,
}: McpSettingsPanelProps): JSX.Element {
  const [status, setStatus] = useState<McpStatus | null>(null);
  const [perms, setPerms] = useState<McpPermission[]>([]);
  const [pending, setPending] = useState<McpPendingRequest[]>([]);
  const [masterOn, setMasterOn] = useState<boolean | null>(null);
  const [busy, setBusy] = useState(false);

  // Guard async resolves from a poll tick landing after unmount.
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  const refresh = useCallback(async () => {
    try {
      const [s, p, m, q] = await Promise.all([
        window.kcreate.mcpPermission.status(),
        window.kcreate.mcpPermission.list(),
        window.kcreate.mcpPermission.masterEnabled(),
        window.kcreate.mcpPermission.pendingList(),
      ]);
      if (!mounted.current) return;
      setStatus(s);
      setPerms(p);
      setMasterOn(m);
      setPending(q);
    } catch (e) {
      onStatus?.(`mcp: ${errMsg(e)}`);
    }
  }, [onStatus]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // Live poll while the server is running so prompts surface promptly.
  useEffect(() => {
    if (status?.running !== true) return undefined;
    const id = window.setInterval(() => {
      void refresh();
    }, POLL_INTERVAL_MS);
    return () => {
      window.clearInterval(id);
    };
  }, [status?.running, refresh]);

  const startServer = useCallback(async () => {
    setBusy(true);
    try {
      const port = await window.kcreate.mcp.start();
      onStatus?.(`MCP: listening on 127.0.0.1:${port}.`);
      await refresh();
    } catch (e) {
      onStatus?.(`mcp start failed: ${errMsg(e)}`);
    } finally {
      if (mounted.current) setBusy(false);
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
      if (mounted.current) setBusy(false);
    }
  }, [onStatus, refresh]);

  const toggleMaster = useCallback(
    async (next: boolean) => {
      // Optimistic flip so the switch feels instant; reconciled by the
      // refresh below (and reverted by it if the bridge rejected).
      setMasterOn(next);
      try {
        await window.kcreate.mcpPermission.setMasterEnabled(next);
        onStatus?.(
          next
            ? "MCP: automation enabled."
            : "MCP: automation paused — all tool calls refused.",
        );
        await refresh();
      } catch (e) {
        onStatus?.(`master switch failed: ${errMsg(e)}`);
        await refresh();
      }
    },
    [onStatus, refresh],
  );

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

  const dismissPending = useCallback(
    async (clientId: string, toolName: string) => {
      try {
        await window.kcreate.mcpPermission.pendingClear(clientId, toolName);
        await refresh();
      } catch (e) {
        onStatus?.(`dismiss failed: ${errMsg(e)}`);
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
          MCP automation server
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

      <p style={{ margin: 0, fontSize: 11, color: colors.textMuted }}>
        Lets a local AI agent drive KCreate over JSON-RPC. The server binds
        to <code style={codeStyle}>127.0.0.1</code> only and is never
        reachable from the network. Every tool call is gated by the master
        switch and the per-tool permissions below.
      </p>

      <MasterSwitchCard
        on={masterOn}
        busy={busy}
        onToggle={(next) => {
          void toggleMaster(next);
        }}
      />

      {pending.length > 0 && (
        <section style={{ display: "flex", flexDirection: "column", gap: spacing.sm }}>
          <div style={{ display: "flex", alignItems: "center", gap: spacing.sm }}>
            <h3 style={{ margin: 0, fontSize: 13, fontWeight: 600 }}>
              Approval inbox
            </h3>
            <span style={inboxCountBadge}>{pending.length}</span>
          </div>
          <p style={{ margin: 0, fontSize: 11, color: colors.textMuted }}>
            Tool calls waiting on your decision. Choosing here is recorded as a
            permission and the client&apos;s blocked call succeeds on its next
            attempt.
          </p>
          {pending.map((req) => (
            <PendingRow
              key={`${req.client_id}:${req.tool_name}`}
              req={req}
              onDecide={(grant) => {
                void setGrant(req.client_id, req.tool_name, grant);
              }}
              onDismiss={() => {
                void dismissPending(req.client_id, req.tool_name);
              }}
            />
          ))}
        </section>
      )}

      <h3 style={{ margin: 0, fontSize: 13, fontWeight: 600 }}>
        Granted scopes
      </h3>
      {grouped.size === 0 ? (
        <p style={{ margin: 0, fontSize: 12, color: colors.textMuted }}>
          No permissions granted yet. When a client makes its first tool call
          it appears in the approval inbox above.
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

function MasterSwitchCard({
  on,
  busy,
  onToggle,
}: {
  on: boolean | null;
  busy: boolean;
  onToggle: (next: boolean) => void;
}): JSX.Element {
  const enabled = on === true;
  const paused = on === false;
  return (
    <section
      style={{
        border: `1px solid ${paused ? colors.dangerBorder : colors.border}`,
        background: paused ? colors.dangerBgSoft : colors.bgSoft,
        borderRadius: radius.card,
        padding: spacing.md,
        display: "flex",
        alignItems: "center",
        gap: spacing.md,
      }}
    >
      <div style={{ display: "flex", flexDirection: "column", gap: 2 }}>
        <span style={{ fontSize: 12, fontWeight: 600, color: colors.text }}>
          Allow automation
        </span>
        <span style={{ fontSize: 11, color: colors.textMuted }}>
          {on === null
            ? "Checking…"
            : enabled
              ? "Tool calls are governed by the permissions below."
              : "Paused — every tool call is refused. Grants are kept and restored when you re-enable."}
        </span>
      </div>
      <div style={{ marginLeft: "auto" }}>
        <ToggleSwitch
          checked={enabled}
          disabled={busy || on === null}
          ariaLabel="Allow MCP automation"
          onChange={onToggle}
        />
      </div>
    </section>
  );
}

function ToggleSwitch({
  checked,
  disabled,
  ariaLabel,
  onChange,
}: {
  checked: boolean;
  disabled: boolean;
  ariaLabel: string;
  onChange: (next: boolean) => void;
}): JSX.Element {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={ariaLabel}
      disabled={disabled}
      onClick={() => {
        onChange(!checked);
      }}
      style={{
        position: "relative",
        width: 40,
        height: 22,
        flexShrink: 0,
        borderRadius: radius.pill,
        border: "none",
        background: checked ? colors.accent : colors.border,
        cursor: disabled ? "default" : "pointer",
        opacity: disabled ? 0.6 : 1,
        transition: "background 120ms ease",
        padding: 0,
      }}
    >
      <span
        style={{
          position: "absolute",
          top: 2,
          left: checked ? 20 : 2,
          width: 18,
          height: 18,
          borderRadius: "50%",
          background: colors.textInverse,
          transition: "left 120ms ease",
        }}
      />
    </button>
  );
}

function PendingRow({
  req,
  onDecide,
  onDismiss,
}: {
  req: McpPendingRequest;
  onDecide: (grant: McpPermissionGrant) => void;
  onDismiss: () => void;
}): JSX.Element {
  return (
    <div
      style={{
        border: `1px solid ${colors.warn}`,
        background: colors.warnBgSoft,
        borderRadius: radius.md,
        padding: spacing.sm,
        display: "flex",
        flexDirection: "column",
        gap: 6,
      }}
    >
      <div style={{ display: "flex", alignItems: "center", gap: spacing.sm }}>
        <code style={codeStyle}>{req.tool_name}</code>
        <span style={{ fontSize: 11, color: colors.textMuted }}>
          from <strong style={{ color: colors.text }}>{req.client_id}</strong>
        </span>
        {req.attempts > 1 && (
          <span style={{ fontSize: 10, color: colors.textMuted }}>
            · asked {req.attempts}×
          </span>
        )}
        <button
          type="button"
          aria-label="Dismiss request"
          title="Dismiss without recording a decision"
          onClick={onDismiss}
          style={{
            marginLeft: "auto",
            border: "none",
            background: "transparent",
            color: colors.textMuted,
            cursor: "pointer",
            fontSize: 14,
            lineHeight: 1,
            padding: "0 4px",
          }}
        >
          ×
        </button>
      </div>
      <div style={{ display: "flex", gap: 6 }}>
        <button
          type="button"
          onClick={() => {
            onDecide("once");
          }}
          style={inboxButton("once")}
        >
          Allow once
        </button>
        <button
          type="button"
          onClick={() => {
            onDecide("always");
          }}
          style={inboxButton("always")}
        >
          Always allow
        </button>
        <button
          type="button"
          onClick={() => {
            onDecide("denied");
          }}
          style={inboxButton("denied")}
        >
          Deny
        </button>
      </div>
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
          {entries.length} {entries.length === 1 ? "tool" : "tools"}
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
              <td style={td}>
                <code style={codeStyle}>{entry.tool_name}</code>
              </td>
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
                      {GRANT_LABELS[g]}
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

const GRANT_LABELS: Record<McpPermissionGrant, string> = {
  once: "Allow once",
  always: "Always allow",
  denied: "Denied",
};

function inboxButton(kind: McpPermissionGrant): React.CSSProperties {
  if (kind === "denied") {
    return {
      padding: "4px 10px",
      background: "transparent",
      color: colors.danger,
      border: `1px solid ${colors.dangerBorder}`,
      borderRadius: radius.pill,
      cursor: "pointer",
      fontSize: 11,
      fontWeight: 600,
    };
  }
  const solid = kind === "always";
  return {
    padding: "4px 10px",
    background: solid ? colors.accent : "transparent",
    color: solid ? colors.textInverse : colors.accent,
    border: `1px solid ${solid ? colors.accent : colors.border}`,
    borderRadius: radius.pill,
    cursor: "pointer",
    fontSize: 11,
    fontWeight: 600,
  };
}

const inboxCountBadge: React.CSSProperties = {
  padding: "0 7px",
  background: colors.warn,
  color: colors.textInverse,
  borderRadius: radius.pill,
  fontSize: 10,
  fontWeight: 700,
  lineHeight: "16px",
};

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

const codeStyle: React.CSSProperties = {
  fontFamily:
    'ui-monospace, SFMono-Regular, Menlo, Consolas, "Liberation Mono", monospace',
  fontSize: 11,
  padding: "1px 4px",
  background: colors.bg,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.sm,
};

function errMsg(e: unknown): string {
  if (e instanceof Error) return e.message;
  return String(e);
}
