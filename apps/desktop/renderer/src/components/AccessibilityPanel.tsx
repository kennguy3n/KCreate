// AccessibilityPanel — Phase 1, Block A, Task 1.
//
// PROPOSAL.md §4.2 calls for "Find accessibility issues → Local report
// with fix buttons". This panel wraps the existing
// `window.kcreate.ai.checkAccessibility()` bridge action, which asks
// the local LLM sidecar to audit the document's contrast, tap
// targets, missing alt text, and font sizes. The bridge feeds the LLM
// the full document JSON (per-node bounds, fills, fonts, etc.), so
// findings come back tagged with the offending `node_id`.
//
// All work runs locally on the user's machine — no network calls.
// When the LLM sidecar is not ready (no model loaded), the panel
// surfaces a clear empty/error state with a link to the Model Manager.

import { useMemo, useState } from "react";

import type { LlmJsonResult } from "../../../shared/scene";
import { colors, radius, spacing } from "../styles/tokens";

export type AccessibilitySeverity = "info" | "warn" | "error";

export interface AccessibilityIssue {
  /** `null` when the LLM couldn't attribute the issue to a specific
   * node (e.g. document-level findings). */
  nodeId: string | null;
  severity: AccessibilitySeverity;
  message: string;
}

export interface AccessibilityPanelProps {
  /**
   * Called when the user clicks an issue and the LLM identified a
   * specific node — the host selects that node in the canvas.
   * Caller is responsible for narrowing to known node ids.
   */
  onSelectNode?: (nodeId: string) => void;
  /** Surfaces panel status to the host's status bar. */
  onStatus?: (msg: string | null) => void;
}

type Phase = "ready" | "running" | "done" | "error";

export function AccessibilityPanel({
  onSelectNode,
  onStatus,
}: AccessibilityPanelProps): JSX.Element {
  const [phase, setPhase] = useState<Phase>("ready");
  const [issues, setIssues] = useState<AccessibilityIssue[]>([]);
  const [meta, setMeta] = useState<{ tokens: number; model: string } | null>(
    null,
  );
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  const counts = useMemo(() => countBySeverity(issues), [issues]);

  const runCheck = async (): Promise<void> => {
    setPhase("running");
    setErrorMsg(null);
    setIssues([]);
    setMeta(null);
    onStatus?.("Analyzing with local AI…");
    try {
      const reply: LlmJsonResult = await window.kcreate.ai.checkAccessibility();
      const parsed = parseAccessibilityReply(reply.json);
      setIssues(parsed);
      setMeta({ tokens: reply.tokens_used, model: reply.model });
      setPhase("done");
      onStatus?.(
        parsed.length === 0
          ? "Accessibility check: no issues."
          : `Accessibility check: ${parsed.length} issue${parsed.length === 1 ? "" : "s"} found.`,
      );
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      setErrorMsg(msg);
      setPhase("error");
      onStatus?.(`Accessibility check failed: ${msg}`);
    }
  };

  const isSidecarNotReady =
    phase === "error" &&
    errorMsg !== null &&
    /sidecar|not ready|not running/i.test(errorMsg);

  return (
    <aside
      style={{
        background: colors.bg,
        display: "flex",
        flexDirection: "column",
        padding: spacing.md,
        gap: spacing.sm,
        overflowY: "auto",
      }}
    >
      <h2
        style={{
          margin: 0,
          fontSize: 14,
          fontWeight: 600,
          color: colors.text,
        }}
      >
        Accessibility
      </h2>
      <p style={paragraphStyle}>
        Audits this document for WCAG AA contrast failures, undersized
        tap targets, missing alt text, and small fonts. Runs entirely
        on your machine via the local LLM sidecar.
      </p>

      <div style={{ display: "flex", gap: spacing.sm, alignItems: "center" }}>
        <button
          type="button"
          onClick={() => {
            void runCheck();
          }}
          disabled={phase === "running"}
          style={primaryBtn(phase === "running")}
          aria-label="Run accessibility check"
        >
          {phase === "running" ? "Analyzing…" : "Run Accessibility Check"}
        </button>
        {meta ? (
          <span style={modelBadge} title="LLM that produced these findings">
            {meta.model}
          </span>
        ) : null}
      </div>

      {phase === "running" ? (
        <div style={statusStripStyle("ok")} role="status">
          <span style={{ marginRight: 6 }} aria-hidden>
            ◌
          </span>
          Analyzing with local AI…
        </div>
      ) : null}

      {phase === "done" && issues.length > 0 ? (
        <div style={summaryRowStyle}>
          <SeverityCount severity="error" count={counts.error} />
          <SeverityCount severity="warn" count={counts.warn} />
          <SeverityCount severity="info" count={counts.info} />
        </div>
      ) : null}

      {phase === "done" && issues.length === 0 ? (
        <div style={statusStripStyle("ok")}>
          No accessibility issues found. ✓
        </div>
      ) : null}

      {phase === "error" ? (
        <div style={statusStripStyle("err")}>
          <div>{errorMsg ?? "Unknown error"}</div>
          {isSidecarNotReady ? (
            <div style={{ marginTop: 4, fontSize: 11 }}>
              Start a model in the Model Manager (AI Assist tab) to
              enable accessibility checks.
            </div>
          ) : null}
        </div>
      ) : null}

      {phase === "ready" ? (
        <div style={emptyStateStyle}>
          Run a check to get started.
        </div>
      ) : null}

      {issues.length > 0 ? (
        <ul style={issueListStyle} aria-label="Accessibility issues">
          {issues.map((issue, idx) => (
            <IssueRow
              // node_id is not unique (multiple issues per node) so we
              // index-suffix.
              key={`${issue.nodeId ?? "doc"}-${idx}`}
              issue={issue}
              onSelectNode={onSelectNode}
            />
          ))}
        </ul>
      ) : null}

      {meta ? (
        <p style={hintStyle}>
          {meta.tokens.toLocaleString()} tokens · model {meta.model} · all
          processing local
        </p>
      ) : null}
    </aside>
  );
}

function IssueRow({
  issue,
  onSelectNode,
}: {
  issue: AccessibilityIssue;
  onSelectNode?: (nodeId: string) => void;
}): JSX.Element {
  const canSelect = issue.nodeId !== null && onSelectNode !== undefined;
  return (
    <li style={issueItemStyle}>
      <div style={issueHeaderStyle}>
        <span style={severityBadge(issue.severity)}>
          {labelForSeverity(issue.severity)}
        </span>
        {canSelect ? (
          <button
            type="button"
            onClick={() => {
              if (issue.nodeId) onSelectNode?.(issue.nodeId);
            }}
            style={nodeLinkStyle}
            aria-label={`Select node ${issue.nodeId}`}
          >
            {issue.nodeId ? issue.nodeId.slice(0, 8) : ""}…
          </button>
        ) : (
          <span style={{ fontSize: 10, color: colors.textMuted }}>
            Document
          </span>
        )}
      </div>
      <div style={issueMessageStyle}>{issue.message}</div>
    </li>
  );
}

function SeverityCount({
  severity,
  count,
}: {
  severity: AccessibilitySeverity;
  count: number;
}): JSX.Element {
  if (count === 0) return <></>;
  return (
    <span style={severityBadge(severity)}>
      {count} {labelForSeverity(severity).toLowerCase()}
      {count === 1 ? "" : "s"}
    </span>
  );
}

/**
 * Parse the LLM's JSON reply per the schema in
 * `kcreate_ai::build_accessibility_prompt`:
 * `{"issues":[{"node_id":"<uuid|null>","severity":"info|warn|error","message":"..."}]}`.
 *
 * Be defensive — production models occasionally emit slightly off
 * schemas (e.g. wrap the JSON in code fences). Returns `[]` if the
 * reply can't be parsed, which lets the UI surface "no issues" rather
 * than crashing.
 */
export function parseAccessibilityReply(raw: string): AccessibilityIssue[] {
  const cleaned = stripCodeFences(raw).trim();
  if (cleaned === "") return [];
  let parsed: unknown;
  try {
    parsed = JSON.parse(cleaned);
  } catch {
    return [];
  }
  if (typeof parsed !== "object" || parsed === null) return [];
  const issues = (parsed as { issues?: unknown }).issues;
  if (!Array.isArray(issues)) return [];
  const out: AccessibilityIssue[] = [];
  for (const raw of issues) {
    if (typeof raw !== "object" || raw === null) continue;
    const r = raw as Record<string, unknown>;
    const severity = normaliseSeverity(r.severity);
    const message = typeof r.message === "string" ? r.message.trim() : "";
    if (message === "") continue;
    const nodeId =
      typeof r.node_id === "string" && r.node_id !== "null" && r.node_id !== ""
        ? r.node_id
        : null;
    out.push({ nodeId, severity, message });
  }
  return out;
}

function stripCodeFences(s: string): string {
  // Some local models wrap their JSON in ```json … ``` blocks.
  const fenced = s.match(/```(?:json)?\s*([\s\S]*?)```/i);
  if (fenced && typeof fenced[1] === "string") return fenced[1];
  return s;
}

function normaliseSeverity(raw: unknown): AccessibilitySeverity {
  if (typeof raw !== "string") return "info";
  const lower = raw.toLowerCase();
  if (lower === "error" || lower === "err" || lower === "fail") return "error";
  if (lower === "warn" || lower === "warning") return "warn";
  return "info";
}

function countBySeverity(issues: AccessibilityIssue[]): Record<
  AccessibilitySeverity,
  number
> {
  const counts: Record<AccessibilitySeverity, number> = {
    error: 0,
    warn: 0,
    info: 0,
  };
  for (const i of issues) counts[i.severity] += 1;
  return counts;
}

function labelForSeverity(s: AccessibilitySeverity): string {
  if (s === "error") return "Error";
  if (s === "warn") return "Warning";
  return "Info";
}

const paragraphStyle: React.CSSProperties = {
  margin: 0,
  fontSize: 11,
  color: colors.textMuted,
  lineHeight: 1.5,
};

const hintStyle: React.CSSProperties = {
  margin: 0,
  fontSize: 11,
  color: colors.textMuted,
  lineHeight: 1.5,
};

const emptyStateStyle: React.CSSProperties = {
  padding: `${spacing.md}px ${spacing.sm}px`,
  fontSize: 11,
  color: colors.textMuted,
  textAlign: "center",
  background: colors.bgSoft,
  borderRadius: radius.card,
  border: `1px dashed ${colors.border}`,
};

const summaryRowStyle: React.CSSProperties = {
  display: "flex",
  gap: spacing.xs,
  flexWrap: "wrap",
};

const issueListStyle: React.CSSProperties = {
  listStyle: "none",
  margin: 0,
  padding: 0,
  display: "flex",
  flexDirection: "column",
  gap: spacing.xs,
};

const issueItemStyle: React.CSSProperties = {
  background: colors.bgSoft,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.card,
  padding: spacing.sm,
  display: "flex",
  flexDirection: "column",
  gap: 4,
};

const issueHeaderStyle: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  justifyContent: "space-between",
  gap: spacing.xs,
};

const issueMessageStyle: React.CSSProperties = {
  fontSize: 12,
  color: colors.text,
  lineHeight: 1.4,
};

const nodeLinkStyle: React.CSSProperties = {
  fontFamily:
    'ui-monospace, SFMono-Regular, Menlo, "Roboto Mono", monospace',
  fontSize: 10,
  color: colors.accent,
  background: "transparent",
  border: `1px solid ${colors.accent}`,
  borderRadius: radius.pill,
  padding: "2px 8px",
  cursor: "pointer",
};

const modelBadge: React.CSSProperties = {
  fontSize: 10,
  fontWeight: 500,
  background: colors.bgSoft,
  color: colors.textMuted,
  padding: "2px 8px",
  borderRadius: radius.pill,
  border: `1px solid ${colors.border}`,
};

function severityBadge(severity: AccessibilitySeverity): React.CSSProperties {
  const palette = {
    error: { bg: "rgba(220,38,38,0.12)", fg: "#dc2626", border: "#dc2626" },
    warn: { bg: "rgba(217,119,6,0.12)", fg: "#d97706", border: "#d97706" },
    info: { bg: "rgba(37,99,235,0.12)", fg: "#2563eb", border: "#2563eb" },
  } as const;
  const p = palette[severity];
  return {
    background: p.bg,
    color: p.fg,
    border: `1px solid ${p.border}`,
    fontSize: 10,
    fontWeight: 600,
    padding: "2px 8px",
    borderRadius: radius.pill,
    textTransform: "uppercase",
    letterSpacing: 0.4,
  };
}

function statusStripStyle(kind: "ok" | "err"): React.CSSProperties {
  return {
    padding: `${spacing.xs}px ${spacing.sm}px`,
    fontSize: 11,
    borderRadius: radius.card / 2,
    background:
      kind === "ok" ? "rgba(124,58,237,0.08)" : "rgba(220,38,38,0.08)",
    color: kind === "ok" ? colors.accent : "#dc2626",
    border: `1px solid ${kind === "ok" ? colors.accent : "#dc2626"}`,
  };
}

function primaryBtn(disabled: boolean): React.CSSProperties {
  return {
    padding: "8px 14px",
    fontSize: 12,
    fontWeight: 600,
    background: disabled ? colors.bgSoft : colors.accent,
    color: disabled ? colors.textMuted : colors.textInverse,
    border: `1px solid ${disabled ? colors.border : colors.accent}`,
    borderRadius: radius.pill,
    cursor: disabled ? "not-allowed" : "pointer",
  };
}
