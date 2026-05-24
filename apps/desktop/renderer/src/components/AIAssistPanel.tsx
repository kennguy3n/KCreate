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

import {
  isContainerNodeType,
  type ExtractedColor,
  type LayoutSuggestion,
  type NodeInfo,
  type TextRegion,
} from "../../../shared/scene";
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
      <PaletteSection selected={selectedNode} onStatus={onStatus} />
      <hr style={separatorStyle} />
      <SmartSelectSection selected={selectedNode} onStatus={onStatus} />
      <hr style={separatorStyle} />
      <OcrSection
        selected={selectedNode}
        onStatus={onStatus}
        onApplied={onApplied}
      />
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

// Container-node-type predicate (`isContainerNodeType`) lives in
// `apps/desktop/shared/scene.ts` so there is exactly one TS-side
// source of truth, kept in lockstep with the Rust constant
// `kcreate_core::node::CONTAINER_NODE_WIRE_NAMES` and the
// `NodeType::is_container()` exhaustive match. The Rust test
// `canonical_container_wire_names_match_expected_list` (in
// `crates/kcreate_core/src/node.rs`) fires if anyone changes the
// container classification without also updating the wire-name
// constant the TS file mirrors.

/**
 * Layout-suggest section. Visible whenever a container node
 * (Artboard, Page, GroupLayer, LayoutFrame, ComponentLayer) is
 * selected; clicking
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
    selected !== null && isContainerNodeType(selected.nodeType);
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
          Select an <b>Artboard</b>, <b>Page</b>, <b>Group</b>,{" "}
          <b>Frame</b>, or <b>Component</b> to suggest layout groupings for its
          children.
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

/**
 * Reads the intrinsic raster dimensions from a `NodeInfo.metadata`
 * payload, returning `null` when the node isn't a RasterLayer or the
 * metadata key is absent / malformed. The shape comes from
 * `kcreate_export::scene_metadata::RasterImageMeta` (mirrored by the
 * `raster_image` metadata key); the renderer just reads it — the
 * authoritative source remains the document.
 *
 * Kept as a single helper so future renderer surfaces that depend on
 * intrinsic raster size (palette, smart-select, OCR, …) all read it
 * the same way and don't drift on the wire-shape contract.
 */
function rasterIntrinsicSize(
  node: NodeInfo | null,
): { width: number; height: number } | null {
  if (node === null || node.nodeType !== "RasterLayer") return null;
  const raw = node.metadata?.["raster_image"];
  if (raw === undefined || raw === null || typeof raw !== "object") return null;
  const meta = raw as { width?: unknown; height?: unknown };
  const w = typeof meta.width === "number" ? meta.width : null;
  const h = typeof meta.height === "number" ? meta.height : null;
  if (w === null || h === null || w <= 0 || h <= 0) return null;
  return { width: w, height: h };
}

/**
 * Palette extraction. Visible whenever a `RasterLayer` is selected;
 * clicking "Extract palette" runs `kcreate_ai::extract_palette` (a
 * pure-Rust k-means pass over the raster's RGBA pixels) and renders
 * the top-N dominant colors with their frequency share of the
 * image. Each swatch is clickable — that copies the hex value to the
 * clipboard so the user can paste it into the fill picker (or
 * anywhere else a hex is accepted).
 *
 * Apply-as-fill is deliberately out of scope for this iteration:
 * `kcreate_bridge` exposes no mutator for `node.style.fill` yet
 * (the existing `updateNode` IPC only accepts name/visible/locked/
 * metadata). Wiring "apply this swatch as the document accent" or
 * "as the selected layer's fill" needs a new bridge surface first —
 * see Block C / Block D for that follow-up. Until then, the
 * clipboard-handoff matches the affordance every existing color
 * inspector in the app uses (ColorSettingsPanel, hex inputs, etc.).
 */
function PaletteSection({
  selected,
  onStatus,
}: {
  selected: NodeInfo | null;
  onStatus: (msg: string | null) => void;
}): JSX.Element {
  const isRaster = selected !== null && selected.nodeType === "RasterLayer";
  const nodeId = isRaster ? selected.id : null;
  const intrinsic = rasterIntrinsicSize(selected);

  type PalettePhase = "idle" | "running" | "done" | "error";
  const [phase, setPhase] = useState<PalettePhase>("idle");
  const [maxColors, setMaxColors] = useState(6);
  const [palette, setPalette] = useState<ExtractedColor[]>([]);
  const [error, setError] = useState<string | null>(null);
  // Per-section request token — same rationale as LayoutAssistSection
  // above (re-running while a previous call is in flight must drop
  // the stale result rather than overwrite the fresh one).
  const requestTokenRef = useRef(0);

  useEffect(() => {
    setPhase("idle");
    setPalette([]);
    setError(null);
    requestTokenRef.current += 1;
  }, [nodeId]);

  if (!isRaster || nodeId === null) {
    return (
      <section style={cardStyle}>
        <div style={cardHeaderStyle}>
          <strong>Palette extraction</strong>
          <span style={badgeStyle("ok")}>Local CPU</span>
        </div>
        <p style={paragraphStyle}>
          Select a <b>RasterLayer</b> to extract its dominant colors
          via local k-means clustering.
        </p>
      </section>
    );
  }

  const run = async (): Promise<void> => {
    requestTokenRef.current += 1;
    const token = requestTokenRef.current;
    setPhase("running");
    setError(null);
    onStatus(
      `Extracting up to ${maxColors} colors from ${selected?.name} locally…`,
    );
    try {
      const colorsOut = await window.kcreate.aiModel.extractPalette(
        nodeId,
        maxColors,
      );
      if (requestTokenRef.current !== token) return;
      setPalette(colorsOut);
      setPhase("done");
      onStatus(
        colorsOut.length === 0
          ? "Palette: no colors found (empty raster)."
          : `Palette: ${colorsOut.length} dominant color${colorsOut.length === 1 ? "" : "s"}.`,
      );
    } catch (e) {
      if (requestTokenRef.current !== token) return;
      const msg = e instanceof Error ? e.message : String(e);
      setError(msg);
      setPhase("error");
      onStatus(`Palette failed: ${msg}`);
    }
  };

  const copyHex = async (hex: string): Promise<void> => {
    try {
      await navigator.clipboard.writeText(hex);
      onStatus(`Copied ${hex} to clipboard.`);
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      onStatus(`Copy failed: ${msg}`);
    }
  };

  return (
    <section style={cardStyle}>
      <div style={cardHeaderStyle}>
        <strong>Palette extraction</strong>
        <span style={badgeStyle("ok")}>Local CPU</span>
      </div>
      <p style={paragraphStyle}>
        K-means over <b>{selected?.name}</b>
        {intrinsic ? ` (${intrinsic.width}×${intrinsic.height} px)` : ""}.
        Click any swatch to copy its hex to the clipboard.
      </p>
      <div
        style={{
          display: "flex",
          gap: spacing.xs,
          alignItems: "center",
          fontSize: 11,
          color: colors.textMuted,
        }}
      >
        <label htmlFor="palette-max-colors">Max colors</label>
        <input
          id="palette-max-colors"
          type="number"
          min={2}
          max={12}
          value={maxColors}
          onChange={(e) => {
            const next = Number.parseInt(e.target.value, 10);
            // Clamp to the k-means bounds the Rust side will accept
            // (Phase 2 cap = 12, floor = 2). Out-of-range values
            // would still trip the Rust validator but pinging the
            // sidecar for an arithmetic error is wasteful when we
            // can short-circuit here.
            if (Number.isFinite(next)) {
              setMaxColors(Math.min(12, Math.max(2, next)));
            }
          }}
          disabled={phase === "running"}
          style={{
            width: 48,
            padding: "2px 6px",
            border: `1px solid ${colors.border}`,
            borderRadius: radius.card / 2,
            fontSize: 11,
            background: colors.bg,
            color: colors.text,
          }}
        />
        <button
          type="button"
          onClick={() => {
            void run();
          }}
          disabled={phase === "running"}
          style={primaryBtn(phase === "running")}
          aria-label="Extract palette"
        >
          {phase === "running" ? "Extracting…" : "Extract palette"}
        </button>
      </div>
      {phase === "error" && error !== null ? (
        <div style={statusStripStyle("err")}>{error}</div>
      ) : null}
      {phase === "done" && palette.length === 0 ? (
        <div style={statusStripStyle("ok")}>
          No colors found. (Raster is empty or fully transparent.)
        </div>
      ) : null}
      {palette.length > 0 ? (
        <ul
          style={{
            listStyle: "none",
            margin: 0,
            padding: 0,
            display: "flex",
            flexDirection: "column",
            gap: spacing.xs,
          }}
          aria-label="Extracted palette"
        >
          {palette.map((c, idx) => (
            <li
              key={`palette-${idx}-${c.hex}`}
              style={{
                display: "flex",
                alignItems: "center",
                gap: spacing.xs,
              }}
            >
              <button
                type="button"
                onClick={() => {
                  void copyHex(c.hex);
                }}
                aria-label={`Copy ${c.hex} to clipboard`}
                style={{
                  flex: 1,
                  display: "flex",
                  alignItems: "center",
                  gap: spacing.sm,
                  padding: spacing.xs,
                  background: colors.bg,
                  border: `1px solid ${colors.border}`,
                  borderRadius: radius.card / 2,
                  cursor: "pointer",
                  textAlign: "left",
                  color: colors.text,
                  fontSize: 11,
                }}
              >
                <span
                  style={{
                    width: 24,
                    height: 24,
                    background: c.hex,
                    border: `1px solid ${colors.border}`,
                    borderRadius: radius.card / 2,
                    flexShrink: 0,
                  }}
                  aria-hidden
                />
                <span style={monoStyle}>{c.hex}</span>
                <span
                  style={{
                    marginLeft: "auto",
                    color: colors.textMuted,
                    fontVariantNumeric: "tabular-nums",
                  }}
                >
                  {(c.frequency * 100).toFixed(1)}%
                </span>
              </button>
            </li>
          ))}
        </ul>
      ) : null}
    </section>
  );
}

/**
 * Smart selection. Visible whenever a `RasterLayer` is selected.
 *
 * Calls `kcreate_ai::smart_select` (a BFS flood-fill over the
 * raster's RGBA pixels, gated on the Euclidean colour distance from
 * the seed pixel). The user supplies a seed (x, y) in the raster's
 * intrinsic pixel coordinate space and a tolerance in [0.0, 1.0]
 * (0 = exact-colour match, 1 = match everything).
 *
 * The mask is returned as base64 of a `width × height` packed
 * grayscale buffer (0 = excluded, 255 = included). We render it
 * directly to a small `<canvas>` for preview. Materialising the
 * mask as a new RasterMaskLayer is a follow-up — it needs a
 * mutator surface on the bridge that doesn't exist yet (parallels
 * the palette → fill plumbing gap described on PaletteSection).
 *
 * The clamp on (x, y) to the raster's intrinsic dimensions is
 * authoritative on the Rust side; we apply the same clamp here so
 * the user gets immediate feedback on out-of-bounds inputs without
 * a bridge round trip.
 */
function SmartSelectSection({
  selected,
  onStatus,
}: {
  selected: NodeInfo | null;
  onStatus: (msg: string | null) => void;
}): JSX.Element {
  const isRaster = selected !== null && selected.nodeType === "RasterLayer";
  const nodeId = isRaster ? selected.id : null;
  const intrinsic = rasterIntrinsicSize(selected);

  type SmartPhase = "idle" | "running" | "done" | "error";
  const [phase, setPhase] = useState<SmartPhase>("idle");
  const [seedX, setSeedX] = useState(0);
  const [seedY, setSeedY] = useState(0);
  const [tolerance, setTolerance] = useState(0.1);
  const [maskInfo, setMaskInfo] = useState<{
    base64: string;
    width: number;
    height: number;
    selectedPixels: number;
  } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  // Per-section request token — same rationale as the other AI
  // sections; a re-run while one is in flight must drop the stale
  // result rather than overwrite the fresh one.
  const requestTokenRef = useRef(0);

  // Reset state and re-centre the seed on the new raster when the
  // selection changes. Centring is a useful default — most raster
  // subjects sit roughly in the middle, and the user can drag the
  // seed from there.
  //
  // We depend on the primitive width/height pair rather than the
  // `intrinsic` object itself because `rasterIntrinsicSize` returns
  // a fresh object on every render — using it in the dep list would
  // re-fire the effect on every parent re-render. The primitive
  // pair carries the only state we actually care about.
  const intrinsicWidth = intrinsic?.width ?? null;
  const intrinsicHeight = intrinsic?.height ?? null;
  useEffect(() => {
    setPhase("idle");
    setMaskInfo(null);
    setError(null);
    requestTokenRef.current += 1;
    if (intrinsicWidth !== null && intrinsicHeight !== null) {
      setSeedX(Math.floor(intrinsicWidth / 2));
      setSeedY(Math.floor(intrinsicHeight / 2));
    } else {
      setSeedX(0);
      setSeedY(0);
    }
  }, [nodeId, intrinsicWidth, intrinsicHeight]);

  // Repaint the mask preview whenever the result lands. Done in an
  // effect (not inline) so React commits the <canvas> before we
  // touch its 2D context — pre-commit canvas access can null-ref on
  // first render.
  useEffect(() => {
    if (maskInfo === null) return;
    const canvas = canvasRef.current;
    if (canvas === null) return;
    const ctx = canvas.getContext("2d");
    if (ctx === null) return;
    // Decode base64 → bytes; one mask byte per pixel.
    let bytes: Uint8Array;
    try {
      const bin = atob(maskInfo.base64);
      bytes = new Uint8Array(bin.length);
      for (let i = 0; i < bin.length; i += 1) bytes[i] = bin.charCodeAt(i);
    } catch {
      return;
    }
    canvas.width = maskInfo.width;
    canvas.height = maskInfo.height;
    // Defensive guard: the Rust mask is one byte per pixel, so
    // `bytes.length` must exactly match `width * height`. Bail out
    // and clear the canvas if it doesn't — `Uint8ClampedArray`
    // silently ignores out-of-bounds writes, so a size mismatch
    // would otherwise render a truncated preview with uninitialized
    // transparent pixels for the missing region. Mismatches imply
    // a bridge / backend bug, so we render nothing rather than a
    // visually-plausible-but-wrong mask.
    const expectedLen = maskInfo.width * maskInfo.height;
    if (bytes.length !== expectedLen) {
      ctx.clearRect(0, 0, maskInfo.width, maskInfo.height);
      console.warn(
        `kcreate: smart-select mask size mismatch — got ${bytes.length} bytes, expected ${expectedLen} (${maskInfo.width}×${maskInfo.height})`,
      );
      return;
    }
    const image = ctx.createImageData(maskInfo.width, maskInfo.height);
    // The Rust mask is one byte per pixel (0 or 255). We expand to
    // RGBA with the mask value as the alpha channel and a vivid
    // accent as the RGB so the selection reads at a glance.
    for (let i = 0; i < bytes.length; i += 1) {
      const v = bytes[i] ?? 0;
      const o = i * 4;
      image.data[o + 0] = 124; // accent R
      image.data[o + 1] = 58; // accent G
      image.data[o + 2] = 237; // accent B
      image.data[o + 3] = v;
    }
    ctx.putImageData(image, 0, 0);
  }, [maskInfo]);

  if (!isRaster || nodeId === null) {
    return (
      <section style={cardStyle}>
        <div style={cardHeaderStyle}>
          <strong>Smart selection</strong>
          <span style={badgeStyle("ok")}>Local CPU</span>
        </div>
        <p style={paragraphStyle}>
          Select a <b>RasterLayer</b> to build a flood-fill selection
          mask from a seed pixel.
        </p>
      </section>
    );
  }

  if (intrinsic === null) {
    // Real edge case — a RasterLayer with no `raster_image` metadata
    // is broken on the document side. We surface it as a useful
    // diagnostic rather than silently no-op, because the AI call
    // would otherwise fail with a less actionable Io error.
    return (
      <section style={cardStyle}>
        <div style={cardHeaderStyle}>
          <strong>Smart selection</strong>
          <span style={badgeStyle("err")}>No raster metadata</span>
        </div>
        <p style={paragraphStyle}>
          <b>{selected?.name}</b> is missing intrinsic-size metadata.
          Re-import or replace the raster to enable smart-select.
        </p>
      </section>
    );
  }

  const run = async (): Promise<void> => {
    requestTokenRef.current += 1;
    const token = requestTokenRef.current;
    setPhase("running");
    setError(null);
    onStatus(
      `Building selection mask from (${seedX}, ${seedY}) with tolerance ${tolerance.toFixed(2)}…`,
    );
    try {
      const base64 = await window.kcreate.aiModel.smartSelect(
        nodeId,
        seedX,
        seedY,
        tolerance,
      );
      if (requestTokenRef.current !== token) return;
      // Count selected pixels for the status strip. Reusing the
      // decode pass that the preview effect runs would couple the
      // two concerns; this is a cheap O(n) pass on a buffer the
      // browser already holds, run once.
      const bin = atob(base64);
      let selectedPixels = 0;
      for (let i = 0; i < bin.length; i += 1) {
        if (bin.charCodeAt(i) > 0) selectedPixels += 1;
      }
      setMaskInfo({
        base64,
        width: intrinsic.width,
        height: intrinsic.height,
        selectedPixels,
      });
      setPhase("done");
      const total = intrinsic.width * intrinsic.height;
      const pct = total === 0 ? 0 : (selectedPixels / total) * 100;
      onStatus(
        `Smart selection: ${selectedPixels.toLocaleString()} / ${total.toLocaleString()} px (${pct.toFixed(1)}%).`,
      );
    } catch (e) {
      if (requestTokenRef.current !== token) return;
      const msg = e instanceof Error ? e.message : String(e);
      setError(msg);
      setPhase("error");
      onStatus(`Smart selection failed: ${msg}`);
    }
  };

  return (
    <section style={cardStyle}>
      <div style={cardHeaderStyle}>
        <strong>Smart selection</strong>
        <span style={badgeStyle("ok")}>Local CPU</span>
      </div>
      <p style={paragraphStyle}>
        BFS flood-fill over <b>{selected?.name}</b> ({intrinsic.width}×
        {intrinsic.height} px). Pick a seed pixel and a colour-distance
        tolerance.
      </p>
      <div
        style={{
          display: "grid",
          gridTemplateColumns: "auto 1fr",
          gap: "4px 6px",
          fontSize: 11,
          color: colors.textMuted,
          alignItems: "center",
        }}
      >
        <label htmlFor="smart-x">Seed X (px)</label>
        <input
          id="smart-x"
          type="number"
          min={0}
          max={intrinsic.width - 1}
          value={seedX}
          onChange={(e) => {
            const next = Number.parseInt(e.target.value, 10);
            if (Number.isFinite(next)) {
              setSeedX(Math.min(intrinsic.width - 1, Math.max(0, next)));
            }
          }}
          disabled={phase === "running"}
          style={numberInputStyle}
        />
        <label htmlFor="smart-y">Seed Y (px)</label>
        <input
          id="smart-y"
          type="number"
          min={0}
          max={intrinsic.height - 1}
          value={seedY}
          onChange={(e) => {
            const next = Number.parseInt(e.target.value, 10);
            if (Number.isFinite(next)) {
              setSeedY(Math.min(intrinsic.height - 1, Math.max(0, next)));
            }
          }}
          disabled={phase === "running"}
          style={numberInputStyle}
        />
        <label htmlFor="smart-tol">Tolerance</label>
        <input
          id="smart-tol"
          type="number"
          min={0}
          max={1}
          step={0.01}
          value={tolerance}
          onChange={(e) => {
            const next = Number.parseFloat(e.target.value);
            if (Number.isFinite(next)) {
              setTolerance(Math.min(1, Math.max(0, next)));
            }
          }}
          disabled={phase === "running"}
          style={numberInputStyle}
        />
      </div>
      <button
        type="button"
        onClick={() => {
          void run();
        }}
        disabled={phase === "running"}
        style={primaryBtn(phase === "running")}
        aria-label="Build selection mask"
      >
        {phase === "running" ? "Selecting…" : "Build selection mask"}
      </button>
      {phase === "error" && error !== null ? (
        <div style={statusStripStyle("err")}>{error}</div>
      ) : null}
      {maskInfo !== null ? (
        <div
          style={{
            display: "flex",
            flexDirection: "column",
            gap: spacing.xs,
            alignItems: "center",
          }}
        >
          <canvas
            ref={canvasRef}
            aria-label="Selection mask preview"
            style={{
              maxWidth: "100%",
              maxHeight: 160,
              imageRendering: "pixelated",
              background:
                "repeating-conic-gradient(#e5e7eb 0% 25%, transparent 0% 50%) 50% / 12px 12px",
              border: `1px solid ${colors.border}`,
              borderRadius: radius.card / 2,
            }}
          />
          <span style={{ fontSize: 10, color: colors.textMuted }}>
            {maskInfo.selectedPixels.toLocaleString()} px selected of{" "}
            {(maskInfo.width * maskInfo.height).toLocaleString()} (
            {maskInfo.width * maskInfo.height === 0
              ? "0.0"
              : (
                  (maskInfo.selectedPixels /
                    (maskInfo.width * maskInfo.height)) *
                  100
                ).toFixed(1)}
            %)
          </span>
        </div>
      ) : null}
    </section>
  );
}

const numberInputStyle: React.CSSProperties = {
  padding: "2px 6px",
  border: `1px solid ${colors.border}`,
  borderRadius: radius.card / 2,
  fontSize: 11,
  background: colors.bg,
  color: colors.text,
};

/**
 * Text-region detection section. Visible whenever a `RasterLayer`
 * is selected — runs the local CV detector
 * (`kcreate_ai::ocr::detect_text_regions`) and renders each
 * detected region as a row with an "Insert as text layer" button.
 *
 * The detector is honest about its scope: it reports text-shaped
 * bboxes, not characters. Clicking insert materialises a new
 * `TextLayer` sibling of the source raster positioned over the
 * region; the user types the actual recognised text into the
 * layer after insertion.
 *
 * State machine mirrors the other AI sections:
 *
 *   idle → running → (done | error) → idle (on new selection)
 *
 * Each detect run gets a fresh request token (same pattern as
 * `LayoutAssistSection` and `SmartSelectSection`) so a re-detect
 * during an in-flight call drops the stale result rather than
 * overwriting the fresh one.
 */
function OcrSection({
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

  type OcrPhase = "idle" | "running" | "done" | "error";
  const [phase, setPhase] = useState<OcrPhase>("idle");
  const [regions, setRegions] = useState<TextRegion[]>([]);
  const [error, setError] = useState<string | null>(null);
  // Per-row insert phase: tracks which region is currently being
  // materialised so we can disable its button (and only its
  // button) while the bridge call is in flight. Keyed by region
  // index because the bbox tuple isn't a stable identity across
  // re-detect runs.
  const [insertingIndex, setInsertingIndex] = useState<number | null>(null);
  // Request-token guard — see LayoutAssistSection for the full
  // rationale; the same pattern applies here.
  const requestTokenRef = useRef(0);

  // Reset on selection change.
  useEffect(() => {
    setPhase("idle");
    setRegions([]);
    setError(null);
    setInsertingIndex(null);
    requestTokenRef.current += 1;
  }, [nodeId]);

  if (!isRaster || nodeId === null) {
    return (
      <section style={cardStyle}>
        <div style={cardHeaderStyle}>
          <strong>Text region detection</strong>
          <span style={badgeStyle("ok")}>Local CPU</span>
        </div>
        <p style={paragraphStyle}>
          Select a <b>RasterLayer</b> to detect text-shaped regions
          and insert each one as a new text layer at the detected
          bbox.
        </p>
      </section>
    );
  }

  const run = async (): Promise<void> => {
    requestTokenRef.current += 1;
    const token = requestTokenRef.current;
    setPhase("running");
    setError(null);
    setRegions([]);
    onStatus("Scanning raster for text-shaped regions…");
    try {
      const detected = await window.kcreate.aiModel.detectTextRegions(nodeId);
      if (requestTokenRef.current !== token) return;
      setRegions(detected);
      setPhase("done");
      onStatus(
        detected.length === 0
          ? "No text-shaped regions detected."
          : `Detected ${detected.length} text region${detected.length === 1 ? "" : "s"}.`,
      );
    } catch (e) {
      if (requestTokenRef.current !== token) return;
      const msg = e instanceof Error ? e.message : String(e);
      setError(msg);
      setPhase("error");
      onStatus(`Text-region detection failed: ${msg}`);
    }
  };

  const insertRegion = async (region: TextRegion, idx: number): Promise<void> => {
    setInsertingIndex(idx);
    onStatus(
      `Inserting text layer over region ${idx + 1} of ${regions.length}…`,
    );
    try {
      await window.kcreate.aiModel.insertTextLayerForRegion({
        rasterNodeId: nodeId,
        region,
      });
      onStatus(
        `Text layer inserted for region ${idx + 1}. Type the recognised text into the new layer.`,
      );
      onApplied();
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      onStatus(`Text-layer insert failed: ${msg}`);
    } finally {
      setInsertingIndex(null);
    }
  };

  return (
    <section style={cardStyle}>
      <div style={cardHeaderStyle}>
        <strong>Text region detection</strong>
        <span style={badgeStyle("ok")}>Local CPU</span>
      </div>
      <p style={paragraphStyle}>
        Detector reports text-shaped bboxes — character recognition
        is not part of this pass. Each row is one detected line; the
        inserted <b>TextLayer</b> sits over the region with the
        estimated font size and an empty text body for you to type
        into.
      </p>
      <button
        type="button"
        onClick={() => {
          void run();
        }}
        disabled={phase === "running"}
        style={primaryBtn(phase === "running")}
        aria-label="Detect text regions"
      >
        {phase === "running" ? "Detecting…" : "Detect text regions"}
      </button>
      {phase === "error" && error !== null ? (
        <div style={statusStripStyle("err")}>{error}</div>
      ) : null}
      {phase === "done" && regions.length === 0 ? (
        <div style={statusStripStyle("ok")}>
          No text-shaped regions found. Try adjusting the source
          raster&apos;s contrast — the default detector threshold
          targets dark-on-light screenshots.
        </div>
      ) : null}
      {regions.length > 0 ? (
        <ul
          style={{
            margin: 0,
            padding: 0,
            listStyle: "none",
            display: "flex",
            flexDirection: "column",
            gap: 4,
          }}
        >
          {regions.map((r, idx) => {
            const inserting = insertingIndex === idx;
            // The detector emits at most a few dozen lines per
            // image and we drive the row list off the detector
            // output, which is a snapshot we hold for the lifetime
            // of this `done` phase — `regions` doesn't reorder, so
            // the array index is a stable key. Same reasoning as
            // the gradient stop list in RightPanel's FillSection.
            return (
              <li
                key={idx}
                style={{
                  display: "flex",
                  alignItems: "center",
                  gap: 6,
                  fontSize: 11,
                  background: colors.bg,
                  border: `1px solid ${colors.border}`,
                  borderRadius: radius.card / 2,
                  padding: "4px 6px",
                }}
              >
                <span
                  style={{
                    flex: 1,
                    fontFamily: monoStyle.fontFamily,
                    color: colors.text,
                  }}
                  title={`region ${idx + 1}: ${r.width}×${r.height} px @ (${r.x}, ${r.y}); glyphs=${r.glyphCount}, est chars=${r.estimatedCharCount}`}
                >
                  {`${r.width}×${r.height} @ (${r.x}, ${r.y}) · ${r.glyphCount} glyphs · ~${r.estimatedCharCount} chars`}
                </span>
                <button
                  type="button"
                  onClick={() => {
                    void insertRegion(r, idx);
                  }}
                  disabled={insertingIndex !== null}
                  style={secondaryBtn(insertingIndex !== null)}
                  aria-label={`Insert text layer for region ${idx + 1}`}
                >
                  {inserting ? "Inserting…" : "Insert text layer"}
                </button>
              </li>
            );
          })}
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
