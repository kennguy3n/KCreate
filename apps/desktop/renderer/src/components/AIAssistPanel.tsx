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

import { useState } from "react";

import type { NodeInfo } from "../../../shared/scene";
import { colors, radius, spacing } from "../styles/tokens";

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
    </aside>
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
