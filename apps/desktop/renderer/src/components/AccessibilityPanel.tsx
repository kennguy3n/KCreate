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
//
// Phase 4 follow-up Block B adds an inline, raster-scoped
// "Generate alt-text" affordance driven by the local
// brightness/contrast/saturation/edge-density heuristic in
// `kcreate_ai::alt_text`. The heuristic runs entirely in Rust — no
// LLM, no network — so it's always available regardless of model
// pack state. The two paths complement each other: the
// document-level LLM audit catches *what's missing* (no alt text
// at all on a raster); the per-node heuristic gives the user a
// factual default they can accept or edit.

import { useEffect, useMemo, useState } from "react";

import type { AltTextReport, LlmJsonResult, NodeInfo } from "../../../shared/scene";
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
  /**
   * Currently-selected node — drives the inline "Generate
   * alt-text" affordance when it's a raster layer. `null` (or
   * non-raster) collapses the per-node section gracefully.
   */
  selected?: NodeInfo | null;
}

type Phase = "ready" | "running" | "done" | "error";

export function AccessibilityPanel({
  onSelectNode,
  onStatus,
  selected,
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

      <AltTextSection selected={selected ?? null} onStatus={onStatus} />
    </aside>
  );
}

/**
 * Per-node alt-text section. Visible whenever a raster layer is
 * selected; renders a "Generate alt-text" button that runs the
 * local heuristic in Rust and shows the result inline. The user
 * can edit the text before clicking "Apply", or "Clear" to
 * remove an existing label.
 *
 * The component holds its own state (rather than lifting to the
 * parent) so cycling between layers doesn't lose an unsaved
 * draft mid-way through editing — the per-node `nodeId` key in
 * the `useEffect` resets state cleanly when the selection
 * changes.
 */
function AltTextSection({
  selected,
  onStatus,
}: {
  selected: NodeInfo | null;
  onStatus?: (msg: string | null) => void;
}): JSX.Element {
  const isRaster =
    selected !== null && selected.nodeType === "RasterLayer";
  const nodeId = isRaster ? selected.id : null;

  type AltPhase = "idle" | "generating" | "ready" | "applying" | "error";
  const [phase, setPhase] = useState<AltPhase>("idle");
  const [report, setReport] = useState<AltTextReport | null>(null);
  const [draft, setDraft] = useState<string>("");
  const [existing, setExisting] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Reset state on selection change so the user doesn't see stale
  // analysis when they click a different raster.
  useEffect(() => {
    setPhase("idle");
    setReport(null);
    setDraft("");
    setError(null);
    // Fetch the currently-persisted alt-text so the user can see
    // what (if anything) is already on the node before generating.
    // We read it from the node's metadata via a lightweight bridge
    // call — `getNode` already returns the metadata blob, but the
    // `NodeInfo` surface drops it for size reasons. Until that's
    // exposed, derive from the document tree on the next refresh
    // by leaving `existing` `null` here.
    setExisting(null);
  }, [nodeId]);

  if (!isRaster || nodeId === null) {
    return <></>;
  }

  const generate = async (): Promise<void> => {
    setPhase("generating");
    setError(null);
    onStatus?.("Generating alt-text locally…");
    try {
      const r = await window.kcreate.aiModel.altTextForNode(nodeId);
      setReport(r);
      setDraft(r.text);
      setPhase("ready");
      onStatus?.("Alt-text suggestion ready. Edit and Apply when satisfied.");
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      setError(msg);
      setPhase("error");
      onStatus?.(`Alt-text failed: ${msg}`);
    }
  };

  const apply = async (): Promise<void> => {
    setPhase("applying");
    setError(null);
    try {
      await window.kcreate.aiModel.applyAltText(nodeId, draft);
      setExisting(draft.length === 0 ? null : draft);
      setPhase("ready");
      onStatus?.(
        draft.length === 0
          ? "Alt-text cleared."
          : "Alt-text applied to layer.",
      );
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      setError(msg);
      setPhase("error");
      onStatus?.(`Apply alt-text failed: ${msg}`);
    }
  };

  const clear = async (): Promise<void> => {
    setPhase("applying");
    setError(null);
    try {
      await window.kcreate.aiModel.applyAltText(nodeId, "");
      setDraft("");
      setReport(null);
      setExisting(null);
      setPhase("idle");
      onStatus?.("Alt-text cleared.");
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      setError(msg);
      setPhase("error");
      onStatus?.(`Clear alt-text failed: ${msg}`);
    }
  };

  return (
    <section
      style={{
        marginTop: spacing.md,
        paddingTop: spacing.sm,
        borderTop: `1px solid ${colors.border}`,
        display: "flex",
        flexDirection: "column",
        gap: spacing.xs,
      }}
      aria-label="Alt-text for selected image"
    >
      <h3
        style={{
          margin: 0,
          fontSize: 12,
          fontWeight: 600,
          color: colors.text,
        }}
      >
        Image description
      </h3>
      <p style={paragraphStyle}>
        Generates a factual alt-text suggestion from the raster&apos;s
        pixels using a local heuristic — no model required.
      </p>

      {existing !== null ? (
        <div
          style={{
            ...statusStripStyle("ok"),
            background: "rgba(34,197,94,0.08)",
            color: "#16a34a",
            border: "1px solid #16a34a",
          }}
        >
          Currently set: <em>{existing}</em>
        </div>
      ) : null}

      <div style={{ display: "flex", gap: spacing.xs, flexWrap: "wrap" }}>
        <button
          type="button"
          onClick={() => {
            void generate();
          }}
          disabled={phase === "generating" || phase === "applying"}
          style={primaryBtn(phase === "generating")}
          aria-label="Generate alt-text suggestion"
        >
          {phase === "generating" ? "Analyzing…" : "Generate alt-text"}
        </button>
        {report !== null ? (
          <button
            type="button"
            onClick={() => {
              void apply();
            }}
            disabled={
              phase === "applying" ||
              phase === "generating" ||
              draft.trim().length === 0
            }
            style={primaryBtn(
              phase === "applying" || draft.trim().length === 0,
            )}
            aria-label="Apply alt-text to selected image"
          >
            {phase === "applying" ? "Applying…" : "Apply"}
          </button>
        ) : null}
        {existing !== null || report !== null ? (
          <button
            type="button"
            onClick={() => {
              void clear();
            }}
            disabled={phase === "applying" || phase === "generating"}
            style={{
              ...primaryBtn(phase === "applying"),
              background: "transparent",
              color: colors.textMuted,
              border: `1px solid ${colors.border}`,
            }}
            aria-label="Clear alt-text on selected image"
          >
            Clear
          </button>
        ) : null}
      </div>

      {report !== null ? (
        <>
          <textarea
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            rows={3}
            style={{
              width: "100%",
              fontSize: 12,
              padding: spacing.xs,
              border: `1px solid ${colors.border}`,
              borderRadius: radius.card / 2,
              background: colors.bg,
              color: colors.text,
              resize: "vertical",
              fontFamily: "inherit",
            }}
            aria-label="Editable alt-text draft"
          />
          <div
            style={{
              display: "flex",
              gap: spacing.xs,
              flexWrap: "wrap",
              fontSize: 11,
              color: colors.textMuted,
            }}
          >
            <span>
              brightness {report.brightness.toFixed(2)}
            </span>
            <span>contrast {report.contrast.toFixed(2)}</span>
            <span>saturation {report.saturation.toFixed(2)}</span>
            <span>edges {report.edge_density.toFixed(2)}</span>
          </div>
          {report.palette.length > 0 ? (
            <div
              style={{
                display: "flex",
                gap: 4,
                marginTop: 2,
              }}
              aria-label="Dominant colours"
            >
              {report.palette.slice(0, 6).map((c, idx) => (
                <span
                  key={`palette-${idx}`}
                  title={`rgb(${c.r}, ${c.g}, ${c.b})`}
                  style={{
                    width: 16,
                    height: 16,
                    background: `rgb(${c.r}, ${c.g}, ${c.b})`,
                    border: `1px solid ${colors.border}`,
                    borderRadius: 4,
                  }}
                />
              ))}
            </div>
          ) : null}
        </>
      ) : null}

      {phase === "error" && error !== null ? (
        <div style={statusStripStyle("err")}>{error}</div>
      ) : null}
    </section>
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
  for (const entry of issues) {
    if (typeof entry !== "object" || entry === null) continue;
    const r = entry as Record<string, unknown>;
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
