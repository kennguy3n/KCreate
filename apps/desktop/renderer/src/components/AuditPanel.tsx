// AuditPanel — Phase 6 (Tasks 13–14).
//
// Displays the persistent audit log backed by `kcreate_audit`. The
// panel queries the bridge on mount and whenever the user changes the
// filter controls (kind selector, date range, search-by-project /
// node). Rows are newest-first. A "purge" button lets the user delete
// entries older than a chosen retention window.

import { useCallback, useEffect, useMemo, useState } from "react";

import type {
  AuditEvent,
  AuditEventKind,
  AuditEventKindTag,
  AuditQuery,
  AuditQueryReport,
} from "../../../shared/scene";
import { colors, radius, spacing } from "../styles/tokens";

export interface AuditPanelProps {
  onStatus?: (msg: string | null) => void;
}

const KIND_OPTIONS: { value: AuditEventKindTag | ""; label: string }[] = [
  { value: "", label: "All events" },
  { value: "operation", label: "Operations" },
  { value: "ai_action", label: "AI actions" },
  { value: "project", label: "Project lifecycle" },
  { value: "collab", label: "Collaboration" },
  { value: "other", label: "Other" },
];

const RETENTION_OPTIONS = [
  { label: "7 days", days: 7 },
  { label: "30 days", days: 30 },
  { label: "90 days", days: 90 },
  { label: "1 year", days: 365 },
];

function kindLabel(kind: AuditEventKind): string {
  switch (kind.type) {
    case "operation":
      return `Op: ${kind.command}`;
    case "ai_action":
      return `AI: ${kind.action_type}`;
    case "project":
      return `Project: ${kind.action}`;
    case "collab":
      return collabActionLabel(kind);
    case "other":
      return kind.label;
  }
}

function collabActionLabel(kind: Extract<AuditEventKind, { type: "collab" }>): string {
  switch (kind.action) {
    case "session_started":
      return kind.community_id
        ? `Collab: started (${kind.community_id})`
        : "Collab: started";
    case "session_left":
      return "Collab: left";
    case "peer_joined":
      return `Collab: ${kind.display_name} joined`;
    case "peer_left":
      return `Collab: ${kind.peer_id.slice(0, 8)}… left`;
    case "peer_kicked":
      return `Collab: ${kind.peer_id.slice(0, 8)}… kicked (${kind.reason})`;
    case "operation_received":
      return `Collab: ${kind.peer_id.slice(0, 8)}… sent ${kind.op_count} ops`;
    case "conflict_resolved":
      return `Collab: conflict on ${kind.node_id.slice(0, 8)}…`;
    case "kchat_desktop_status":
      return `KChat Desktop: ${kind.status}`;
  }
}

function formatTimestamp(iso: string): string {
  try {
    const d = new Date(iso);
    return d.toLocaleString(undefined, {
      month: "short",
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
    });
  } catch {
    return iso;
  }
}

export function AuditPanel({ onStatus }: AuditPanelProps) {
  const [report, setReport] = useState<AuditQueryReport | null>(null);
  const [kindFilter, setKindFilter] = useState<AuditEventKindTag | "">("");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [dbPath, setDbPath] = useState<string>("");

  const loadData = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const filter: AuditQuery = {};
      if (kindFilter) filter.kind = kindFilter;
      filter.limit = 200;
      const result = await window.kcreate.audit.query(filter);
      setReport(result);
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      setError(msg);
      onStatus?.(`Audit load failed: ${msg}`);
    } finally {
      setLoading(false);
    }
  }, [kindFilter, onStatus]);

  useEffect(() => {
    void loadData();
  }, [loadData]);

  useEffect(() => {
    window.kcreate.audit.path().then(setDbPath).catch(() => {});
  }, []);

  const handlePurge = useCallback(
    async (days: number) => {
      const cutoff = new Date(Date.now() - days * 86_400_000).toISOString();
      try {
        const removed = await window.kcreate.audit.purge(cutoff);
        onStatus?.(`Purged ${removed} audit entries older than ${days} days`);
        await loadData();
      } catch (e) {
        const msg = e instanceof Error ? e.message : String(e);
        onStatus?.(`Purge failed: ${msg}`);
      }
    },
    [loadData, onStatus],
  );

  const events: AuditEvent[] = useMemo(
    () => report?.events ?? [],
    [report],
  );

  return (
    <div style={styles.container}>
      <div style={styles.header}>
        <h3 style={styles.title}>Audit Log</h3>
        <span style={styles.count}>
          {report
            ? `${events.length} of ${report.total} entries`
            : "Loading…"}
        </span>
      </div>

      {/* Filters */}
      <div style={styles.filterRow}>
        <select
          value={kindFilter}
          onChange={(e) =>
            setKindFilter(e.target.value as AuditEventKindTag | "")
          }
          style={styles.select}
        >
          {KIND_OPTIONS.map((o) => (
            <option key={o.value} value={o.value}>
              {o.label}
            </option>
          ))}
        </select>
        <button
          style={styles.refreshBtn}
          onClick={() => void loadData()}
          disabled={loading}
        >
          ↻ Refresh
        </button>
      </div>

      {/* Error */}
      {error && <div style={styles.error}>{error}</div>}

      {/* Table */}
      <div style={styles.tableWrap}>
        <table style={styles.table}>
          <thead>
            <tr>
              <th style={styles.th}>Time</th>
              <th style={styles.th}>Actor</th>
              <th style={styles.th}>Kind</th>
              <th style={styles.th}>Project</th>
              <th style={styles.th}>Nodes</th>
            </tr>
          </thead>
          <tbody>
            {events.map((evt) => (
              <tr key={evt.id} style={styles.tr}>
                <td style={styles.td}>{formatTimestamp(evt.timestamp)}</td>
                <td style={styles.td}>{evt.actor}</td>
                <td style={styles.td}>{kindLabel(evt.kind)}</td>
                <td style={styles.tdMono}>
                  {evt.project_id ? evt.project_id.slice(0, 8) : "—"}
                </td>
                <td style={styles.tdMono}>
                  {evt.affected_nodes.length > 0
                    ? evt.affected_nodes
                        .map((n) => n.slice(0, 8))
                        .join(", ")
                    : "—"}
                </td>
              </tr>
            ))}
            {events.length === 0 && !loading && (
              <tr>
                <td colSpan={5} style={styles.empty}>
                  No audit events found.
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>

      {/* Retention / Purge */}
      <div style={styles.footer}>
        <span style={styles.footerLabel}>Purge older than:</span>
        {RETENTION_OPTIONS.map((opt) => (
          <button
            key={opt.days}
            style={styles.purgeBtn}
            onClick={() => void handlePurge(opt.days)}
          >
            {opt.label}
          </button>
        ))}
      </div>

      {/* DB path */}
      {dbPath && (
        <div style={styles.dbPath} title={dbPath}>
          DB: {dbPath}
        </div>
      )}
    </div>
  );
}

const styles: Record<string, React.CSSProperties> = {
  container: {
    display: "flex",
    flexDirection: "column",
    gap: spacing.sm,
    padding: spacing.md,
    fontFamily:
      'Inter, -apple-system, system-ui, "Segoe UI", Roboto, sans-serif',
    color: colors.text,
  },
  header: {
    display: "flex",
    alignItems: "center",
    justifyContent: "space-between",
  },
  title: {
    margin: 0,
    fontSize: 16,
    fontWeight: 600,
  },
  count: {
    fontSize: 12,
    color: colors.textMuted,
  },
  filterRow: {
    display: "flex",
    gap: spacing.sm,
    alignItems: "center",
  },
  select: {
    flex: 1,
    padding: `${spacing.xs}px ${spacing.sm}px`,
    borderRadius: radius.sm,
    border: `1px solid ${colors.border}`,
    fontSize: 13,
    background: colors.bg,
    color: colors.text,
  },
  refreshBtn: {
    padding: `${spacing.xs}px ${spacing.sm}px`,
    borderRadius: radius.sm,
    border: `1px solid ${colors.border}`,
    background: colors.bgSoft,
    cursor: "pointer",
    fontSize: 13,
  },
  error: {
    padding: spacing.sm,
    background: colors.dangerBg,
    color: colors.danger,
    borderRadius: radius.sm,
    fontSize: 12,
  },
  tableWrap: {
    overflowX: "auto",
    border: `1px solid ${colors.border}`,
    borderRadius: radius.md,
  },
  table: {
    width: "100%",
    borderCollapse: "collapse",
    fontSize: 12,
  },
  th: {
    textAlign: "left",
    padding: `${spacing.xs}px ${spacing.sm}px`,
    borderBottom: `1px solid ${colors.border}`,
    fontWeight: 600,
    whiteSpace: "nowrap",
    color: colors.textMuted,
    fontSize: 11,
    textTransform: "uppercase",
    letterSpacing: "0.04em",
  },
  tr: {
    borderBottom: `1px solid ${colors.border}`,
  },
  td: {
    padding: `${spacing.xs}px ${spacing.sm}px`,
    whiteSpace: "nowrap",
  },
  tdMono: {
    padding: `${spacing.xs}px ${spacing.sm}px`,
    whiteSpace: "nowrap",
    fontFamily: "monospace",
    fontSize: 11,
    color: colors.textMuted,
  },
  empty: {
    padding: spacing.md,
    textAlign: "center",
    color: colors.textMuted,
  },
  footer: {
    display: "flex",
    gap: spacing.xs,
    alignItems: "center",
  },
  footerLabel: {
    fontSize: 12,
    color: colors.textMuted,
  },
  purgeBtn: {
    padding: `2px ${spacing.sm}px`,
    borderRadius: radius.sm,
    border: `1px solid ${colors.border}`,
    background: colors.bg,
    cursor: "pointer",
    fontSize: 11,
  },
  dbPath: {
    fontSize: 10,
    color: colors.textMuted,
    overflow: "hidden",
    textOverflow: "ellipsis",
    whiteSpace: "nowrap",
  },
};
