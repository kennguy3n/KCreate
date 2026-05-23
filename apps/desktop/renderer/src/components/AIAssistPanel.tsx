// AIAssistPanel — Phase 0 local AI workflow.
//
// Implements the Ask → Preview → Apply → Edit → Undo pattern from
// PROPOSAL.md §6.4. The Phase 0 model is `threshold-v0`, a fully
// local, threshold-based background removal that produces a real RGBA
// mask. Phase 1 swaps the backing model for an ONNX u2net behind the
// same panel/UX.
//
// All work runs on the CPU in-process via `kcreate_ai`; no network
// calls are made. The panel is conservative about provenance — we
// always surface compute device, model name, and "Network: None" so
// the user can reason about the action before applying it.

import { useEffect, useRef, useState } from "react";

import type { LayoutSuggestion, NodeInfo } from "../../../shared/scene";
import { colors, radius, spacing } from "../styles/tokens";
import { LlmChatPanel } from "./LlmChatPanel";
import { McpSettingsPanel } from "./McpSettingsPanel";
import { ModelManager } from "./ModelManager";
import { PluginManager } from "./PluginManager";

export interface AIAssistPanelProps {
  selectedNode: NodeInfo | null;
  onApplied: () => void;
  onStatus: (msg: string | null) => void;
}

type Phase = "ready" | "running" | "done" | "error";

export function AIAssistPanel({
  selectedNode,
  onApplied,
  onStatus,
}: AIAssistPanelProps): JSX.Element {
  const [phase, setPhase] = useState<Phase>("ready");
  const [newNodeId, setNewNodeId] = useState<string | null>(null);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  const canApply =
    selectedNode !== null &&
    selectedNode.nodeType === "RasterLayer" &&
    phase !== "running";

  const handleApply = async (): Promise<void> => {
    if (!selectedNode) return;
    setPhase("running");
    setErrorMsg(null);
    setNewNodeId(null);
    onStatus("AI: removing background (local CPU, threshold-v0)…");
    try {
      const id = await window.kcreate.ai.removeBackground(selectedNode.id);
      setNewNodeId(id);
      setPhase("done");
      onStatus("AI: background removed.");
      onApplied();
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      setErrorMsg(msg);
      setPhase("error");
      onStatus(`AI failed: ${msg}`);
    }
  };

  return (
    <aside
      style={{
        width: 320,
        background: colors.bg,
        borderLeft: `1px solid ${colors.border}`,
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
        AI Assist
      </h2>
      <p style={paragraphStyle}>
        All AI runs locally on this machine. No data leaves your computer.
      </p>

      <section style={cardStyle}>
        <div style={cardHeaderStyle}>
          <strong>Action</strong>
          <span style={badgeStyle("ok")}>Local CPU</span>
        </div>
        <dl style={kvListStyle}>
          <KV label="Action">Remove background</KV>
          <KV label="Compute">Local CPU</KV>
          <KV label="Model">threshold-v0</KV>
          <KV label="Network">None</KV>
          <KV label="Will modify">
            {selectedNode
              ? `${selectedNode.name} (${selectedNode.nodeType})`
              : "—"}
          </KV>
          <KV label="Will create">
            New RasterLayer with transparent background
          </KV>
        </dl>
      </section>

      {phase === "running" ? (
        <div style={statusStripStyle("ok")}>Running locally…</div>
      ) : null}
      {phase === "done" && newNodeId ? (
        <div style={statusStripStyle("ok")}>
          Applied. New layer:{" "}
          <code style={monoStyle}>{newNodeId.slice(0, 8)}…</code>
        </div>
      ) : null}
      {phase === "error" && errorMsg ? (
        <div style={statusStripStyle("err")}>{errorMsg}</div>
      ) : null}

      <div style={{ display: "flex", gap: spacing.sm }}>
        <button
          type="button"
          onClick={() => {
            void handleApply();
          }}
          disabled={!canApply}
          style={primaryBtn(!canApply)}
        >
          {phase === "running" ? "Running…" : "Apply"}
        </button>
        <button
          type="button"
          onClick={() => {
            setPhase("ready");
            setNewNodeId(null);
            setErrorMsg(null);
          }}
          disabled={phase === "running"}
          style={secondaryBtn(phase === "running")}
        >
          Reset
        </button>
      </div>

      <p style={hintStyle}>
        Select a <b>RasterLayer</b> node to enable Apply. Undo any time
        with <kbd>Ctrl/Cmd+Z</kbd> — the AI action is recorded in the
        operation log alongside vector edits.
      </p>

      <hr style={separatorStyle} />
      <LayoutAssistSection selected={selectedNode} onStatus={onStatus} />
      <hr style={separatorStyle} />

      <ModelManager onStatus={onStatus} />
      <LlmChatPanel onStatus={onStatus} />
      <hr style={separatorStyle} />
      <PluginManager onStatus={onStatus} />
      <hr style={separatorStyle} />
      <McpSettingsPanel onStatus={onStatus} />
    </aside>
  );
}

const separatorStyle: React.CSSProperties = {
  border: "none",
  borderTop: "1px solid #E5E7EB",
  margin: "16px 0 8px",
};

/**
 * Container node types eligible for layout-suggest. Hoisted to
 * module scope so the `Set` allocation happens once per process
 * rather than once per render — same pattern as `BASE_TABS` in
 * `RightPanel.tsx`. The values mirror the `NodeType` discriminants
 * the bridge serialises (`crates/kcreate_core/src/node.rs`).
 */
const LAYOUT_ASSIST_CONTAINER_TYPES: ReadonlySet<string> = new Set([
  "Artboard",
  "Page",
  "GroupLayer",
  "LayoutFrame",
]);

/**
 * Layout-suggest section. Visible whenever a container node
 * (Artboard, Page, GroupLayer, LayoutFrame) is selected; clicking
 * "Suggest layout" runs the local DBSCAN-with-alignment clustering
 * heuristic in `kcreate_ai::layout_suggest` over the container's
 * direct visible children and renders a preview of each proposed
 * group. The apply step is intentionally not wired yet — Phase 4
 * follow-up Block B exposes the analysis surface and the
 * preview-only UX so the user can iterate on the algorithm
 * before any LayoutFrame mutation lands.
 */
function LayoutAssistSection({
  selected,
  onStatus,
}: {
  selected: NodeInfo | null;
  onStatus: (msg: string | null) => void;
}): JSX.Element {
  const isContainer =
    selected !== null && LAYOUT_ASSIST_CONTAINER_TYPES.has(selected.nodeType);
  const nodeId = isContainer ? selected.id : null;

  type LayoutPhase = "idle" | "running" | "done" | "error";
  const [phase, setPhase] = useState<LayoutPhase>("idle");
  const [suggestions, setSuggestions] = useState<LayoutSuggestion[]>([]);
  const [error, setError] = useState<string | null>(null);
  // Monotonic per-section request token. Each `run()` invocation
  // bumps the counter and captures the new value; the async result
  // is only applied if the captured token still matches at completion
  // time. This pattern matches the `cancelled` flag used by
  // `useSessionLocks` and the EditorPage presence broadcast, but is
  // adapted to button-triggered async (where we don't have a
  // useEffect-style cleanup hook). A bare `cancelled` flag is
  // insufficient because a *second* in-flight `run()` would set its
  // own flag and never have it flipped — the request-token approach
  // generalises cleanly to N concurrent calls.
  const requestTokenRef = useRef(0);

  // Reset state and invalidate any in-flight `run()` when the
  // selection changes — the previous result, if it still arrives,
  // would be attributed to the wrong artboard.
  useEffect(() => {
    setPhase("idle");
    setSuggestions([]);
    setError(null);
    requestTokenRef.current += 1;
  }, [nodeId]);

  if (!isContainer || nodeId === null) {
    return (
      <section style={cardStyle}>
        <div style={cardHeaderStyle}>
          <strong>Layout assist</strong>
          <span style={badgeStyle("ok")}>Local CPU</span>
        </div>
        <p style={paragraphStyle}>
          Select an <b>Artboard</b>, <b>Page</b>, <b>Group</b>, or
          <b> Frame</b> to suggest layout groupings for its children.
        </p>
      </section>
    );
  }

  const run = async (): Promise<void> => {
    requestTokenRef.current += 1;
    const token = requestTokenRef.current;
    setPhase("running");
    setError(null);
    onStatus("Suggesting layout groupings locally…");
    try {
      const r = await window.kcreate.aiModel.layoutSuggestForArtboard(nodeId);
      if (requestTokenRef.current !== token) {
        // Selection changed, or a newer `run()` started, while this
        // call was in flight — drop the result silently rather than
        // overwrite the freshly-reset state with stale clustering
        // output from a previous artboard.
        return;
      }
      setSuggestions(r);
      setPhase("done");
      onStatus(
        r.length === 0
          ? "Layout assist: no groupings found."
          : `Layout assist: ${r.length} suggestion${r.length === 1 ? "" : "s"}.`,
      );
    } catch (e) {
      if (requestTokenRef.current !== token) {
        // Same rationale as the success path — a stale error from a
        // superseded request would surface as a misleading red
        // banner on the now-correct selection.
        return;
      }
      const msg = e instanceof Error ? e.message : String(e);
      setError(msg);
      setPhase("error");
      onStatus(`Layout assist failed: ${msg}`);
    }
  };

  return (
    <section style={cardStyle}>
      <div style={cardHeaderStyle}>
        <strong>Layout assist</strong>
        <span style={badgeStyle("ok")}>Local CPU</span>
      </div>
      <p style={paragraphStyle}>
        Clusters the direct visible children of{" "}
        <b>{selected?.name}</b> by proximity and edge alignment.
        Preview-only — no nodes are moved.
      </p>
      <button
        type="button"
        onClick={() => {
          void run();
        }}
        disabled={phase === "running"}
        style={primaryBtn(phase === "running")}
        aria-label="Suggest layout groupings"
      >
        {phase === "running" ? "Analyzing…" : "Suggest layout"}
      </button>
      {phase === "done" && suggestions.length === 0 ? (
        <div style={statusStripStyle("ok")}>
          No clusters detected. (Need at least two aligned children.)
        </div>
      ) : null}
      {phase === "error" && error !== null ? (
        <div style={statusStripStyle("err")}>{error}</div>
      ) : null}
      {suggestions.length > 0 ? (
        <ul
          style={{
            listStyle: "none",
            margin: 0,
            padding: 0,
            display: "flex",
            flexDirection: "column",
            gap: spacing.xs,
          }}
          aria-label="Layout suggestions"
        >
          {suggestions.map((s, idx) => (
            <li
              key={`layout-${idx}`}
              style={{
                background: colors.bg,
                border: `1px solid ${colors.border}`,
                borderRadius: radius.card / 2,
                padding: spacing.xs,
                display: "flex",
                flexDirection: "column",
                gap: 2,
                fontSize: 11,
              }}
            >
              <div
                style={{
                  display: "flex",
                  justifyContent: "space-between",
                  gap: spacing.xs,
                }}
              >
                <strong style={{ color: colors.text }}>{s.name}</strong>
                <span
                  style={{
                    color: colors.textMuted,
                    fontVariantNumeric: "tabular-nums",
                  }}
                >
                  {s.member_ids.length}{" "}
                  {s.member_ids.length === 1 ? "node" : "nodes"}
                </span>
              </div>
              <div style={{ color: colors.textMuted }}>
                {s.orientation}
                {s.alignment ? ` · ${s.alignment.replace("_", " ")}` : ""}
                {" · "}
                {Math.round(s.bounds.width)}×{Math.round(s.bounds.height)} at{" "}
                ({Math.round(s.bounds.x)}, {Math.round(s.bounds.y)})
              </div>
            </li>
          ))}
        </ul>
      ) : null}
    </section>
  );
}

function KV({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}): JSX.Element {
  return (
    <>
      <dt style={kvLabelStyle}>{label}</dt>
      <dd style={kvValueStyle}>{children}</dd>
    </>
  );
}

const cardStyle: React.CSSProperties = {
  background: colors.bgSoft,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.card,
  padding: spacing.sm,
  display: "flex",
  flexDirection: "column",
  gap: spacing.xs,
};

const cardHeaderStyle: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  justifyContent: "space-between",
  fontSize: 12,
  color: colors.text,
};

const kvListStyle: React.CSSProperties = {
  margin: 0,
  display: "grid",
  gridTemplateColumns: "auto 1fr",
  gap: "2px 8px",
  fontSize: 11,
};

const kvLabelStyle: React.CSSProperties = {
  color: colors.textMuted,
  fontWeight: 500,
  margin: 0,
};

const kvValueStyle: React.CSSProperties = {
  color: colors.text,
  margin: 0,
};

function badgeStyle(kind: "ok" | "err"): React.CSSProperties {
  return {
    background: kind === "ok" ? "rgba(124,58,237,0.15)" : "rgba(220,38,38,0.15)",
    color: kind === "ok" ? colors.accent : "#dc2626",
    fontSize: 10,
    fontWeight: 600,
    padding: "2px 6px",
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
    flex: 1,
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

function secondaryBtn(disabled: boolean): React.CSSProperties {
  return {
    padding: "8px 14px",
    fontSize: 12,
    fontWeight: 500,
    background: colors.bg,
    color: colors.text,
    border: `1px solid ${colors.border}`,
    borderRadius: radius.pill,
    cursor: disabled ? "not-allowed" : "pointer",
    opacity: disabled ? 0.5 : 1,
  };
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

const monoStyle: React.CSSProperties = {
  fontFamily:
    'ui-monospace, SFMono-Regular, Menlo, "Roboto Mono", monospace',
  fontSize: 11,
};
