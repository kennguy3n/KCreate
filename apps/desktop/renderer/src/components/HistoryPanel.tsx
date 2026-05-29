// Phase 9 Block C Task 17 — Operation log viewer + AI action filter.
//
// PROPOSAL.md §6.3: "History — operation log, including AI actions,
// with filter + jump-to."
//
// Calls `window.kcreate.phase9.operationLogFilter` for the paginated
// list. Each row shows timestamp, actor, command name, AI badge,
// and the affected node IDs. Filter controls let the user narrow to
// AI-only / manual-only and switch the result count.

import { useCallback, useEffect, useMemo, useState } from "react";
import type {
  OperationInfo,
  OperationLogFilter,
} from "../../../shared/scene";
import { colors, font, radius, spacing } from "../styles/tokens";

type ScopeFilter = "all" | "ai" | "manual";

interface HistoryPanelProps {
  /**
   * Optional callback fired when the user clicks "Jump to" on a row.
   * The shell highlights the affected nodes on the canvas (or no-ops
   * if the host doesn't know how to surface a selection yet).
   */
  onJumpTo?: (nodeIds: string[]) => void;
}

export function HistoryPanel({ onJumpTo }: HistoryPanelProps): JSX.Element {
  const [scope, setScope] = useState<ScopeFilter>("all");
  const [limit, setLimit] = useState<number>(50);
  const [entries, setEntries] = useState<OperationInfo[]>([]);
  const [phase, setPhase] = useState<
    "idle" | "loading" | "loaded" | "error"
  >("idle");
  const [error, setError] = useState<string | undefined>(undefined);

  const filter: OperationLogFilter = useMemo(
    () => ({
      aiOnly: scope === "ai",
      manualOnly: scope === "manual",
      since: null,
      until: null,
      limit,
    }),
    [scope, limit],
  );

  const refresh = useCallback(async () => {
    setPhase("loading");
    setError(undefined);
    try {
      const list = await window.kcreate.phase9.operationLogFilter(filter);
      setEntries(list);
      setPhase("loaded");
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setError(message);
      setPhase("error");
    }
  }, [filter]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  return (
    <section style={containerStyle} data-testid="kcreate-history-panel">
      <header style={headerStyle}>
        <h3 style={titleStyle}>History</h3>
        <div style={filterRowStyle}>
          <ScopeButton
            current={scope}
            value="all"
            label="All"
            onChange={setScope}
          />
          <ScopeButton
            current={scope}
            value="ai"
            label="AI only"
            onChange={setScope}
          />
          <ScopeButton
            current={scope}
            value="manual"
            label="Manual only"
            onChange={setScope}
          />
          <select
            value={limit}
            onChange={(e) => setLimit(Number(e.target.value))}
            style={selectStyle}
            data-testid="kcreate-history-limit"
          >
            <option value={25}>25</option>
            <option value={50}>50</option>
            <option value={100}>100</option>
            <option value={250}>250</option>
          </select>
          <button
            type="button"
            onClick={() => void refresh()}
            disabled={phase === "loading"}
            style={refreshButtonStyle}
          >
            {phase === "loading" ? "…" : "Refresh"}
          </button>
        </div>
      </header>

      {error !== undefined && (
        <p role="alert" style={errorStyle}>
          {error}
        </p>
      )}

      {entries.length === 0 && phase === "loaded" ? (
        <p style={emptyStyle}>No operations match the current filter.</p>
      ) : (
        <ol style={listStyle}>
          {entries.map((op) => (
            <li
              key={op.id}
              style={itemStyle}
              data-testid="kcreate-history-entry"
              data-ai-generated={op.aiGenerated ? "true" : "false"}
            >
              <div style={rowHeaderStyle}>
                <span style={commandStyle}>{op.command}</span>
                {op.aiGenerated && (
                  <span style={aiBadgeStyle} aria-label="AI generated">
                    AI
                  </span>
                )}
                <span style={timestampStyle}>{formatTimestamp(op.timestamp)}</span>
              </div>
              <div style={metaStyle}>
                <span style={actorStyle}>{op.actor || "anonymous"}</span>
                {op.affectedNodes.length > 0 && (
                  <button
                    type="button"
                    onClick={() => onJumpTo?.(op.affectedNodes)}
                    style={jumpButtonStyle}
                    data-testid="kcreate-history-jump"
                  >
                    Jump to ({op.affectedNodes.length})
                  </button>
                )}
              </div>
            </li>
          ))}
        </ol>
      )}
    </section>
  );
}

function ScopeButton({
  current,
  value,
  label,
  onChange,
}: {
  current: ScopeFilter;
  value: ScopeFilter;
  label: string;
  onChange: (s: ScopeFilter) => void;
}): JSX.Element {
  const active = current === value;
  return (
    <button
      type="button"
      onClick={() => onChange(value)}
      data-testid={`kcreate-history-scope-${value}`}
      data-active={active ? "true" : "false"}
      style={{
        background: active ? colors.accent : "transparent",
        color: active ? "white" : colors.text,
        border: `1px solid ${active ? colors.accent : colors.border}`,
        borderRadius: radius.sm,
        padding: "4px 10px",
        fontSize: 12,
        cursor: "pointer",
        fontWeight: active ? 600 : 500,
      }}
    >
      {label}
    </button>
  );
}

function formatTimestamp(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleString();
}

const containerStyle: React.CSSProperties = {
  display: "flex",
  flexDirection: "column",
  gap: spacing.sm,
  padding: spacing.md,
  background: colors.bg,
  color: colors.text,
  fontFamily: font.family,
  fontSize: 13,
};

const headerStyle: React.CSSProperties = {
  display: "flex",
  flexDirection: "column",
  gap: spacing.sm,
};

const titleStyle: React.CSSProperties = {
  margin: 0,
  fontSize: 14,
  fontWeight: 600,
};

const filterRowStyle: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: 6,
  flexWrap: "wrap",
};

const selectStyle: React.CSSProperties = {
  background: colors.bgSoft,
  color: colors.text,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.sm,
  padding: "4px 6px",
  fontSize: 12,
};

const refreshButtonStyle: React.CSSProperties = {
  background: "transparent",
  color: colors.text,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.sm,
  padding: "4px 10px",
  fontSize: 12,
  cursor: "pointer",
};

const errorStyle: React.CSSProperties = {
  margin: 0,
  color: "#B91C1C",
  fontSize: 12,
};

const emptyStyle: React.CSSProperties = {
  margin: 0,
  fontSize: 12,
  color: colors.textMuted,
  fontStyle: "italic",
};

const listStyle: React.CSSProperties = {
  listStyle: "none",
  margin: 0,
  padding: 0,
  display: "flex",
  flexDirection: "column",
  gap: 6,
  overflowY: "auto",
  maxHeight: "60vh",
};

const itemStyle: React.CSSProperties = {
  display: "flex",
  flexDirection: "column",
  gap: 2,
  padding: 8,
  background: colors.bgSoft,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.sm,
};

const rowHeaderStyle: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: 6,
};

const commandStyle: React.CSSProperties = {
  fontWeight: 600,
  color: colors.text,
};

const aiBadgeStyle: React.CSSProperties = {
  background: colors.accent,
  color: "white",
  fontSize: 10,
  fontWeight: 700,
  padding: "1px 6px",
  borderRadius: 999,
  letterSpacing: 0.5,
};

const timestampStyle: React.CSSProperties = {
  marginLeft: "auto",
  fontSize: 11,
  color: colors.textMuted,
};

const metaStyle: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: 6,
  fontSize: 11,
  color: colors.textMuted,
};

const actorStyle: React.CSSProperties = { color: colors.textMuted };

const jumpButtonStyle: React.CSSProperties = {
  marginLeft: "auto",
  background: "transparent",
  color: colors.accent,
  border: `1px solid ${colors.accent}`,
  borderRadius: radius.sm,
  padding: "2px 6px",
  fontSize: 11,
  cursor: "pointer",
};
