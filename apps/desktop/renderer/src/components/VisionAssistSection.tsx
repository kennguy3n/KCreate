// VisionAssistSection — Phase 4 vision (VLM) Ask → Preview → Apply
// flow. Surfaces three actions for any selected raster layer:
//
//   1. Describe selection — free-form caption.
//   2. Generate alt text  — accessibility-optimised short caption,
//      applied to the node's `kcreate.altText` metadata.
//   3. Design critique    — terse list of layout / contrast /
//      alignment notes for the artboard the selection belongs to.
//
// The "Apply" step is meaningful for alt-text (writes to node
// metadata, recorded in the operation log) and a no-op preview
// for describe / critique (the user reads the suggestion and acts
// on it manually). This mirrors the Ask → Preview → Apply pattern
// the rest of the AI Assist panel uses.

import { useEffect, useState } from "react";

import type { NodeInfo, VisionStatus } from "../../../shared/scene";
import { colors, radius, spacing } from "../styles/tokens";

interface VisionAssistSectionProps {
  selected: NodeInfo | null;
  onStatus: (msg: string | null) => void;
  onApplied: () => void;
}

type Phase = "idle" | "running" | "preview" | "error";

interface VisionResult {
  kind: "describe" | "alt" | "critique";
  text: string;
}

export function VisionAssistSection({
  selected,
  onStatus,
  onApplied,
}: VisionAssistSectionProps): JSX.Element {
  const [status, setStatus] = useState<VisionStatus | null>(null);
  const [phase, setPhase] = useState<Phase>("idle");
  const [result, setResult] = useState<VisionResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [recommended, setRecommended] = useState<string>("");

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const [s, r] = await Promise.all([
          window.kcreate.vision.status(),
          window.kcreate.vision.recommendedPack(),
        ]);
        if (!cancelled) {
          setStatus(s);
          setRecommended(r);
        }
      } catch {
        // Vision is opt-in; not having it is the common case on
        // first launch. Don't treat the lookup as fatal.
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  const refreshStatus = async (): Promise<void> => {
    const s = await window.kcreate.vision.status();
    setStatus(s);
  };

  const isRaster = selected?.nodeType === "RasterLayer";
  const ready = status?.state === "ready";
  const canRun = isRaster && ready && phase !== "running";

  const runDescribe = async (): Promise<void> => {
    if (!selected) return;
    setPhase("running");
    setError(null);
    onStatus("AI: describing selection (local VLM)…");
    try {
      const text = await window.kcreate.vision.describeNode(
        selected.id,
        "Describe this image in one or two factual sentences.",
      );
      setResult({ kind: "describe", text });
      setPhase("preview");
      onStatus("AI: description ready.");
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      setError(msg);
      setPhase("error");
      onStatus(`AI vision failed: ${msg}`);
    }
  };

  const runAltText = async (): Promise<void> => {
    if (!selected) return;
    setPhase("running");
    setError(null);
    onStatus("AI: generating alt-text (local VLM)…");
    try {
      const text = await window.kcreate.vision.generateAltTextForNode(
        selected.id,
      );
      setResult({ kind: "alt", text });
      setPhase("preview");
      onStatus("AI: alt-text ready.");
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      setError(msg);
      setPhase("error");
      onStatus(`AI vision failed: ${msg}`);
    }
  };

  const runCritique = async (): Promise<void> => {
    if (!selected) return;
    setPhase("running");
    setError(null);
    onStatus("AI: critiquing design (local VLM)…");
    try {
      // We don't have the raw RGBA in the renderer cheaply, so we
      // describe the selected raster layer's content for now. The
      // full "capture artboard" path is wired through batch export
      // in a follow-up; this gets the critique button working for
      // raster reference images right away.
      const text = await window.kcreate.vision.describeNode(
        selected.id,
        "Critique this design. List concrete issues under: Hierarchy, Contrast, Alignment, Spacing, Typography, Accessibility. Be terse and action-oriented.",
      );
      setResult({ kind: "critique", text });
      setPhase("preview");
      onStatus("AI: critique ready.");
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      setError(msg);
      setPhase("error");
      onStatus(`AI vision failed: ${msg}`);
    }
  };

  const applyAlt = async (): Promise<void> => {
    if (!selected || !result || result.kind !== "alt") return;
    try {
      // Persist via the existing apply path so the change goes
      // through the operation log and undo/redo.
      await window.kcreate.aiModel.applyAltText(selected.id, result.text);
      setPhase("idle");
      setResult(null);
      onStatus("AI: alt-text applied.");
      onApplied();
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      setError(msg);
      setPhase("error");
      onStatus(`Apply failed: ${msg}`);
    }
  };

  const startSidecar = async (): Promise<void> => {
    if (!recommended) return;
    onStatus("Starting vision sidecar…");
    try {
      await window.kcreate.vision.start(recommended);
      await refreshStatus();
      onStatus("Vision sidecar ready.");
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      onStatus(`Vision start failed: ${msg}`);
    }
  };

  return (
    <section style={cardStyle}>
      <div style={cardHeaderStyle}>
        <strong>Vision</strong>
        <span style={badgeStyle(ready ? "ok" : "warn")}>
          {status?.state === "ready"
            ? `${status.runtime ?? "vlm"} :${status.port ?? "?"}`
            : (status?.state ?? "stopped")}
        </span>
      </div>
      <p style={hintStyle}>
        Local multimodal model. Describes images, writes alt-text, and
        runs short design critiques without leaving your machine.
      </p>
      {!ready ? (
        <button
          type="button"
          onClick={() => {
            void startSidecar();
          }}
          disabled={!recommended}
          style={primaryBtn(!recommended)}
        >
          {recommended ? `Start (${recommended})` : "No vision pack installed"}
        </button>
      ) : null}
      <div style={{ display: "flex", gap: spacing.xs, flexWrap: "wrap" }}>
        <button
          type="button"
          disabled={!canRun}
          onClick={() => {
            void runDescribe();
          }}
          style={secondaryBtn(!canRun)}
        >
          Describe selection
        </button>
        <button
          type="button"
          disabled={!canRun}
          onClick={() => {
            void runAltText();
          }}
          style={secondaryBtn(!canRun)}
        >
          Generate alt-text
        </button>
        <button
          type="button"
          disabled={!canRun}
          onClick={() => {
            void runCritique();
          }}
          style={secondaryBtn(!canRun)}
        >
          Design critique
        </button>
      </div>
      {phase === "running" ? (
        <div style={statusStripStyle("ok")}>Running locally…</div>
      ) : null}
      {phase === "preview" && result ? (
        <div style={resultBoxStyle}>
          <div style={resultHeaderStyle}>
            <strong>{labelFor(result.kind)}</strong>
            {result.kind === "alt" ? (
              <button
                type="button"
                onClick={() => {
                  void applyAlt();
                }}
                style={primaryBtnInline}
              >
                Apply
              </button>
            ) : null}
          </div>
          <pre style={preStyle}>{result.text}</pre>
        </div>
      ) : null}
      {phase === "error" && error ? (
        <div style={statusStripStyle("err")}>{error}</div>
      ) : null}
    </section>
  );
}

function labelFor(kind: VisionResult["kind"]): string {
  switch (kind) {
    case "describe":
      return "Description";
    case "alt":
      return "Alt-text suggestion";
    case "critique":
      return "Design critique";
  }
}

const cardStyle: React.CSSProperties = {
  background: colors.bgSoft,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.card,
  padding: spacing.md,
  display: "flex",
  flexDirection: "column",
  gap: spacing.sm,
};
const cardHeaderStyle: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  justifyContent: "space-between",
};
const hintStyle: React.CSSProperties = {
  margin: 0,
  fontSize: 11,
  color: colors.textMuted,
  lineHeight: 1.5,
};
function badgeStyle(kind: "ok" | "warn"): React.CSSProperties {
  return {
    fontSize: 10,
    fontWeight: 600,
    padding: "2px 6px",
    borderRadius: radius.pill,
    background:
      kind === "ok" ? "rgba(124,58,237,0.15)" : colors.dangerBgSoft,
    color: kind === "ok" ? colors.accent : colors.danger,
    textTransform: "uppercase",
    letterSpacing: 0.4,
  };
}
function primaryBtn(disabled: boolean): React.CSSProperties {
  return {
    padding: "6px 12px",
    fontSize: 12,
    fontWeight: 600,
    background: disabled ? colors.bgSoft : colors.accent,
    color: disabled ? colors.textMuted : colors.textInverse,
    border: `1px solid ${disabled ? colors.border : colors.accent}`,
    borderRadius: radius.pill,
    cursor: disabled ? "not-allowed" : "pointer",
  };
}
const primaryBtnInline: React.CSSProperties = {
  ...primaryBtn(false),
  fontSize: 11,
  padding: "2px 8px",
};
function secondaryBtn(disabled: boolean): React.CSSProperties {
  return {
    padding: "6px 12px",
    fontSize: 12,
    fontWeight: 500,
    background: colors.bg,
    color: disabled ? colors.textMuted : colors.text,
    border: `1px solid ${colors.border}`,
    borderRadius: radius.pill,
    cursor: disabled ? "not-allowed" : "pointer",
  };
}
function statusStripStyle(kind: "ok" | "err"): React.CSSProperties {
  return {
    padding: `${spacing.xs}px ${spacing.sm}px`,
    fontSize: 11,
    borderRadius: radius.md,
    background:
      kind === "ok" ? "rgba(124,58,237,0.08)" : colors.dangerBgSoft,
    color: kind === "ok" ? colors.accent : colors.danger,
    border: `1px solid ${kind === "ok" ? colors.accent : colors.danger}`,
  };
}
const resultBoxStyle: React.CSSProperties = {
  background: colors.bg,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.md,
  padding: spacing.sm,
  display: "flex",
  flexDirection: "column",
  gap: spacing.xs,
};
const resultHeaderStyle: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  justifyContent: "space-between",
};
const preStyle: React.CSSProperties = {
  margin: 0,
  fontSize: 11,
  fontFamily: "ui-monospace, monospace",
  whiteSpace: "pre-wrap",
  color: colors.text,
};
