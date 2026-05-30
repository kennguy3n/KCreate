// MagicWandTool — Phase 10 Block A Task 5.
//
// Mounts when the user activates the Magic Wand tool from the
// Image Studio tool palette. Listens for canvas clicks, calls
// `window.kcreate.phase10.aiSmartSelectAtPoint(...)` with the
// current tolerance, and surfaces the returned boolean mask as a
// semi-transparent overlay above the canvas.
//
// Modifier conventions match the rest of the editor:
//   - plain click        → replace selection
//   - Shift+click        → add to selection
//   - Alt/Option+click   → subtract from selection
//
// This component does not own the selection — it lifts the mask up
// to the host (`onMaskChanged`) so other Image Studio actions
// (filter, crop, mask creation) can consume it.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import type {
  SmartSelectAtPointResult,
  SmartSelectMode,
} from "../../../shared/scene";
import { colors, radius, spacing } from "../styles/tokens";

export interface MagicWandToolProps {
  /** Raster layer the wand is selecting within. `null` disables. */
  nodeId: string | null;
  /**
   * Element whose `click` events represent canvas-space clicks.
   * Coordinates are translated via `viewportToCanvas` before being
   * sent to the bridge.
   */
  canvasEl: HTMLElement | null;
  /** Canvas → canvas-space converter. */
  viewportToCanvas: (clientX: number, clientY: number) =>
    | { x: number; y: number }
    | null;
  /** Called after every successful selection. `null` clears it. */
  onMaskChanged: (result: SmartSelectAtPointResult | null) => void;
  /** Optional status sink. */
  onStatus?: (msg: string | null) => void;
}

function errMsg(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

const TOLERANCE_MIN = 0;
const TOLERANCE_MAX = 1;
const TOLERANCE_DEFAULT = 0.1;

export function MagicWandTool({
  nodeId,
  canvasEl,
  viewportToCanvas,
  onMaskChanged,
  onStatus,
}: MagicWandToolProps): JSX.Element | null {
  const [tolerance, setTolerance] = useState<number>(TOLERANCE_DEFAULT);
  const [last, setLast] = useState<SmartSelectAtPointResult | null>(null);
  const [busy, setBusy] = useState(false);
  // Two refs make the click handler concurrency-safe:
  //
  //  - `lastRef` mirrors the latest selection synchronously so the
  //    NEXT `add`/`subtract` click sees the correct base mask even
  //    if React hasn't flushed `setLast` yet (e.g. clicks fired
  //    inside the same task as the awaited bridge call).
  //  - `busyRef` lets the synchronous DOM click handler reject any
  //    click that arrives while a previous bridge call is still in
  //    flight. Without it, a rapid double-click would dispatch two
  //    concurrent `aiSmartSelectAtPoint` calls; both would read the
  //    same base mask, and whichever resolved second would overwrite
  //    the first — silently dropping the intermediate add/subtract.
  const lastRef = useRef<SmartSelectAtPointResult | null>(null);
  const busyRef = useRef(false);

  const runSelect = useCallback(
    async (x: number, y: number, mode: SmartSelectMode) => {
      if (!nodeId) return;
      busyRef.current = true;
      setBusy(true);
      try {
        // For Add / Subtract, hand the bridge the previous mask so
        // it can run the set-op server-side without round-tripping
        // mask state through the renderer. `replace` ignores it. We
        // read from `lastRef` instead of the `last` state value so a
        // chain of add/subtract clicks composes correctly even when
        // React batches the intermediate state updates.
        const previousMask =
          mode === "replace" ? null : lastRef.current?.maskBase64 ?? null;
        const result = await window.kcreate.phase10.aiSmartSelectAtPoint(
          nodeId,
          Math.round(x),
          Math.round(y),
          tolerance,
          mode,
          previousMask,
        );
        lastRef.current = result;
        setLast(result);
        onMaskChanged(result);
        onStatus?.(
          `magic-wand: ${result.selectedPixelCount.toLocaleString()} px selected`,
        );
      } catch (e) {
        onStatus?.(`magic-wand failed: ${errMsg(e)}`);
      } finally {
        busyRef.current = false;
        setBusy(false);
      }
    },
    [nodeId, tolerance, onMaskChanged, onStatus],
  );

  useEffect(() => {
    if (!canvasEl || !nodeId) return;
    const onClick = (ev: MouseEvent) => {
      // Drop clicks while a previous bridge call is still in flight.
      // The wand state machine is intentionally non-queued: rapid
      // double-clicks would otherwise race on `lastRef` and produce
      // a non-deterministic mask.
      if (busyRef.current) return;
      const canvasPt = viewportToCanvas(ev.clientX, ev.clientY);
      if (!canvasPt) return;
      const mode: SmartSelectMode = ev.shiftKey
        ? "add"
        : ev.altKey
        ? "subtract"
        : "replace";
      void runSelect(canvasPt.x, canvasPt.y, mode);
    };
    canvasEl.addEventListener("click", onClick);
    return () => canvasEl.removeEventListener("click", onClick);
  }, [canvasEl, nodeId, viewportToCanvas, runSelect]);

  const clear = useCallback(() => {
    lastRef.current = null;
    setLast(null);
    onMaskChanged(null);
  }, [onMaskChanged]);

  const summary = useMemo(() => {
    if (!last) return "No selection";
    return `${last.selectedPixelCount.toLocaleString()} px @ ${last.width}×${last.height}`;
  }, [last]);

  if (!nodeId) return null;

  return (
    <div
      role="toolbar"
      aria-label="Magic wand options"
      style={{
        position: "absolute",
        top: spacing.md,
        left: "50%",
        transform: "translateX(-50%)",
        display: "flex",
        alignItems: "center",
        gap: spacing.md,
        padding: spacing.sm,
        background: colors.bg,
        border: `1px solid ${colors.border}`,
        borderRadius: radius.md,
        boxShadow: "0 8px 24px rgba(0,0,0,0.25)",
        zIndex: 940,
      }}
    >
      <span style={{ fontSize: 12, fontWeight: 600 }}>Magic wand</span>
      <label
        style={{
          display: "flex",
          alignItems: "center",
          gap: 6,
          fontSize: 11,
          color: colors.textMuted,
        }}
      >
        Tolerance
        <input
          type="range"
          min={TOLERANCE_MIN}
          max={TOLERANCE_MAX}
          step={0.01}
          value={tolerance}
          onChange={(e) => setTolerance(Number.parseFloat(e.target.value))}
          style={{ width: 120 }}
        />
        <span style={{ minWidth: 36, fontFamily: "monospace" }}>
          {tolerance.toFixed(2)}
        </span>
      </label>
      <span
        style={{
          fontSize: 11,
          color: busy ? colors.accent : colors.textMuted,
          minWidth: 160,
          textAlign: "center",
        }}
      >
        {busy ? "Selecting…" : summary}
      </span>
      <button
        type="button"
        onClick={clear}
        disabled={!last || busy}
        style={{
          padding: `${spacing.xs}px ${spacing.sm}px`,
          background: "transparent",
          color: colors.text,
          border: `1px solid ${colors.border}`,
          borderRadius: radius.sm,
          cursor: last ? "pointer" : "not-allowed",
          fontSize: 12,
        }}
      >
        Clear
      </button>
    </div>
  );
}
