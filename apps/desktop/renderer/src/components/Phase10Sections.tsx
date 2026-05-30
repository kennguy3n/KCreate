// Phase10Sections — AIAssistPanel sub-components for Phase 10
// Block A (Image Studio AI) and Block B (Vector/Layout AI) actions.
//
// Each section follows the existing AIAssistPanel pattern:
//   - guard on selection type (returns a hint when the wrong node is
//     selected),
//   - phase state machine (`idle` → `running` → `done` | `error`),
//   - calls the Phase 10 bridge surface via `window.kcreate.phase10.*`,
//   - reports progress through the shared `onStatus` sink and
//     `onApplied` callback so the host panel can refresh.
//
// These sections are mounted from AIAssistPanel.tsx.

import { useState } from "react";

import type { AutoColorMode, NodeInfo } from "../../../shared/scene";
import { colors, radius, spacing } from "../styles/tokens";

type Phase = "idle" | "running" | "done" | "error";

function errMsg(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
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

const paragraphStyle: React.CSSProperties = {
  margin: 0,
  fontSize: 11,
  color: colors.textMuted,
  lineHeight: 1.5,
};

function badgeStyle(kind: "ok" | "err"): React.CSSProperties {
  return {
    background: kind === "ok" ? "rgba(124,58,237,0.15)" : colors.dangerBg,
    color: kind === "ok" ? colors.accent : colors.danger,
    fontSize: 10,
    fontWeight: 600,
    padding: "2px 6px",
    borderRadius: radius.pill,
    textTransform: "uppercase",
    letterSpacing: 0.4,
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

function statusStripStyle(kind: "ok" | "err"): React.CSSProperties {
  return {
    padding: `${spacing.xs}px ${spacing.sm}px`,
    fontSize: 11,
    borderRadius: radius.card / 2,
    background:
      kind === "ok" ? "rgba(124,58,237,0.08)" : colors.dangerBgSoft,
    color: kind === "ok" ? colors.accent : colors.danger,
    border: `1px solid ${kind === "ok" ? colors.accent : colors.danger}`,
  };
}

// =============================================================
// Block A — Image Studio AI
// =============================================================

/** Task 1 — AI denoise via NLM. */
export function DenoiseSection({
  selected,
  onStatus,
  onApplied,
}: {
  selected: NodeInfo | null;
  onStatus: (msg: string | null) => void;
  onApplied: () => void;
}): JSX.Element {
  const isRaster = selected !== null && selected.nodeType === "RasterLayer";
  const nodeId = isRaster ? selected.id : null;
  const [phase, setPhase] = useState<Phase>("idle");
  const [strength, setStrength] = useState(10);
  const [searchRadius, setSearchRadius] = useState(10);
  const [patchRadius, setPatchRadius] = useState(3);
  const [error, setError] = useState<string | null>(null);

  if (!nodeId) {
    return (
      <section style={cardStyle}>
        <div style={cardHeaderStyle}>
          <strong>Denoise</strong>
          <span style={badgeStyle("ok")}>Local CPU</span>
        </div>
        <p style={paragraphStyle}>
          Select a <b>RasterLayer</b> to remove noise via non-local means.
        </p>
      </section>
    );
  }

  const run = async (): Promise<void> => {
    setPhase("running");
    setError(null);
    onStatus(`Denoising (strength=${strength.toFixed(1)})…`);
    try {
      await window.kcreate.phase10.aiDenoise(
        nodeId,
        strength,
        searchRadius,
        patchRadius,
      );
      setPhase("done");
      onStatus("Denoise applied.");
      onApplied();
    } catch (e) {
      const msg = errMsg(e);
      setError(msg);
      setPhase("error");
      onStatus(`Denoise failed: ${msg}`);
    }
  };

  const busy = phase === "running";
  return (
    <section style={cardStyle}>
      <div style={cardHeaderStyle}>
        <strong>Denoise</strong>
        <span style={badgeStyle("ok")}>Local CPU</span>
      </div>
      <SliderRow
        label="Strength"
        min={0.1}
        max={50}
        step={0.1}
        value={strength}
        onChange={setStrength}
      />
      <SliderRow
        label="Search radius"
        min={3}
        max={20}
        step={1}
        value={searchRadius}
        onChange={(v) => setSearchRadius(Math.round(v))}
      />
      <SliderRow
        label="Patch radius"
        min={1}
        max={7}
        step={1}
        value={patchRadius}
        onChange={(v) => setPatchRadius(Math.round(v))}
      />
      <div style={{ display: "flex", gap: spacing.xs }}>
        <button
          type="button"
          onClick={() => void run()}
          disabled={busy}
          style={primaryBtn(busy)}
        >
          {busy ? "Denoising…" : "Apply"}
        </button>
      </div>
      {phase === "error" && error ? (
        <div style={statusStripStyle("err")}>{error}</div>
      ) : null}
    </section>
  );
}

/** Task 3 — Auto color correction. */
export function AutoColorSection({
  selected,
  onStatus,
  onApplied,
}: {
  selected: NodeInfo | null;
  onStatus: (msg: string | null) => void;
  onApplied: () => void;
}): JSX.Element {
  const isRaster = selected !== null && selected.nodeType === "RasterLayer";
  const nodeId = isRaster ? selected.id : null;
  const [phase, setPhase] = useState<Phase>("idle");
  const [mode, setMode] = useState<AutoColorMode>("combined");
  const [error, setError] = useState<string | null>(null);

  if (!nodeId) {
    return (
      <section style={cardStyle}>
        <div style={cardHeaderStyle}>
          <strong>Auto color</strong>
          <span style={badgeStyle("ok")}>Local CPU</span>
        </div>
        <p style={paragraphStyle}>
          Select a <b>RasterLayer</b> to auto-correct exposure, white
          balance, and contrast.
        </p>
      </section>
    );
  }

  const run = async (): Promise<void> => {
    setPhase("running");
    setError(null);
    onStatus(`Auto color (${mode})…`);
    try {
      await window.kcreate.phase10.aiAutoColor(nodeId, mode);
      setPhase("done");
      onStatus("Auto color applied.");
      onApplied();
    } catch (e) {
      const msg = errMsg(e);
      setError(msg);
      setPhase("error");
      onStatus(`Auto color failed: ${msg}`);
    }
  };

  const busy = phase === "running";
  return (
    <section style={cardStyle}>
      <div style={cardHeaderStyle}>
        <strong>Auto color</strong>
        <span style={badgeStyle("ok")}>Local CPU</span>
      </div>
      <label
        style={{ display: "flex", alignItems: "center", gap: spacing.xs }}
      >
        <span style={{ fontSize: 11, color: colors.textMuted }}>Mode</span>
        <select
          value={mode}
          onChange={(e) =>
            setMode(e.target.value as AutoColorMode)
          }
          style={selectStyle}
        >
          <option value="auto_levels">Auto levels</option>
          <option value="white_balance">White balance</option>
          <option value="histogram_equalization">Histogram EQ</option>
          <option value="combined">Combined</option>
        </select>
      </label>
      <div style={{ display: "flex", gap: spacing.xs }}>
        <button
          type="button"
          onClick={() => void run()}
          disabled={busy}
          style={primaryBtn(busy)}
        >
          {busy ? "Working…" : "Apply"}
        </button>
      </div>
      {phase === "error" && error ? (
        <div style={statusStripStyle("err")}>{error}</div>
      ) : null}
    </section>
  );
}

// =============================================================
// Block B — Vector / Layout AI
// =============================================================

/** Task 9 — Reformat current page into a 16:9 deck. */
export function ReformatDeckSection({
  selected,
  onStatus,
  onApplied,
}: {
  selected: NodeInfo | null;
  onStatus: (msg: string | null) => void;
  onApplied: () => void;
}): JSX.Element {
  const isPage = selected !== null && selected.nodeType === "Page";
  const pageId = isPage ? selected.id : null;
  const [phase, setPhase] = useState<Phase>("idle");
  const [pageCount, setPageCount] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);

  if (!pageId) {
    return (
      <section style={cardStyle}>
        <div style={cardHeaderStyle}>
          <strong>Reformat as deck</strong>
          <span style={badgeStyle("ok")}>Local LLM</span>
        </div>
        <p style={paragraphStyle}>
          Select a <b>Page</b> to split its content into a multi-page
          16:9 presentation.
        </p>
      </section>
    );
  }

  const run = async (): Promise<void> => {
    setPhase("running");
    setError(null);
    setPageCount(null);
    onStatus("Reformatting into 16:9 deck…");
    try {
      const result = await window.kcreate.phase10.aiReformatToDeck(pageId);
      setPageCount(result.pages.length);
      setPhase("done");
      onStatus(`Created ${result.pages.length} deck page(s).`);
      onApplied();
    } catch (e) {
      const msg = errMsg(e);
      setError(msg);
      setPhase("error");
      onStatus(`Reformat failed: ${msg}`);
    }
  };

  const busy = phase === "running";
  return (
    <section style={cardStyle}>
      <div style={cardHeaderStyle}>
        <strong>Reformat as deck</strong>
        <span style={badgeStyle("ok")}>Local LLM</span>
      </div>
      <p style={paragraphStyle}>
        Uses the local LLM to analyse the page contents and emit a 16:9
        multi-page layout.
      </p>
      <div style={{ display: "flex", gap: spacing.xs }}>
        <button
          type="button"
          onClick={() => void run()}
          disabled={busy}
          style={primaryBtn(busy)}
        >
          {busy ? "Working…" : "Generate deck"}
        </button>
      </div>
      {pageCount !== null && phase === "done" ? (
        <div style={statusStripStyle("ok")}>
          Produced {pageCount} new page(s).
        </div>
      ) : null}
      {phase === "error" && error ? (
        <div style={statusStripStyle("err")}>{error}</div>
      ) : null}
    </section>
  );
}

/** Task 10 — Brief → one-pager. */
export function BriefToOnePagerSection({
  onStatus,
  onApplied,
}: {
  onStatus: (msg: string | null) => void;
  onApplied: () => void;
}): JSX.Element {
  const [phase, setPhase] = useState<Phase>("idle");
  const [brief, setBrief] = useState("");
  const [pageSize, setPageSize] = useState<"letter" | "a4" | "square">(
    "letter",
  );
  const [error, setError] = useState<string | null>(null);

  const run = async (): Promise<void> => {
    if (brief.trim().length === 0) {
      setError("Brief cannot be empty");
      setPhase("error");
      return;
    }
    setPhase("running");
    setError(null);
    onStatus("Generating one-pager from brief…");
    try {
      await window.kcreate.phase10.aiBriefToOnePager(brief, pageSize);
      setPhase("done");
      onStatus("One-pager created.");
      onApplied();
    } catch (e) {
      const msg = errMsg(e);
      setError(msg);
      setPhase("error");
      onStatus(`One-pager failed: ${msg}`);
    }
  };

  const busy = phase === "running";
  return (
    <section style={cardStyle}>
      <div style={cardHeaderStyle}>
        <strong>Brief → one-pager</strong>
        <span style={badgeStyle("ok")}>Local LLM</span>
      </div>
      <textarea
        value={brief}
        onChange={(e) => setBrief(e.target.value)}
        placeholder="Paste a free-form or markdown brief…"
        style={{
          minHeight: 100,
          resize: "vertical",
          fontFamily: "inherit",
          fontSize: 12,
          padding: spacing.xs,
          background: colors.bg,
          color: colors.text,
          border: `1px solid ${colors.border}`,
          borderRadius: radius.sm,
        }}
      />
      <label
        style={{ display: "flex", alignItems: "center", gap: spacing.xs }}
      >
        <span style={{ fontSize: 11, color: colors.textMuted }}>
          Page size
        </span>
        <select
          value={pageSize}
          onChange={(e) => setPageSize(e.target.value as typeof pageSize)}
          style={selectStyle}
        >
          <option value="letter">Letter (8.5 × 11 in)</option>
          <option value="a4">A4 (210 × 297 mm)</option>
          <option value="square">Square (1080 × 1080 px)</option>
        </select>
      </label>
      <div style={{ display: "flex", gap: spacing.xs }}>
        <button
          type="button"
          onClick={() => void run()}
          disabled={busy || brief.trim().length === 0}
          style={primaryBtn(busy || brief.trim().length === 0)}
        >
          {busy ? "Working…" : "Generate"}
        </button>
      </div>
      {phase === "error" && error ? (
        <div style={statusStripStyle("err")}>{error}</div>
      ) : null}
    </section>
  );
}

// =============================================================
// Shared bits
// =============================================================

const selectStyle: React.CSSProperties = {
  flex: 1,
  fontSize: 12,
  padding: "4px 6px",
  background: colors.bg,
  color: colors.text,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.sm,
};

function SliderRow({
  label,
  min,
  max,
  step,
  value,
  onChange,
}: {
  label: string;
  min: number;
  max: number;
  step: number;
  value: number;
  onChange: (v: number) => void;
}): JSX.Element {
  return (
    <label
      style={{
        display: "grid",
        gridTemplateColumns: "100px 1fr 48px",
        alignItems: "center",
        gap: spacing.xs,
        fontSize: 11,
        color: colors.textMuted,
      }}
    >
      <span>{label}</span>
      <input
        type="range"
        min={min}
        max={max}
        step={step}
        value={value}
        onChange={(e) => onChange(Number.parseFloat(e.target.value))}
      />
      <span style={{ fontFamily: "monospace", textAlign: "right" }}>
        {value.toFixed(step < 1 ? 1 : 0)}
      </span>
    </label>
  );
}
