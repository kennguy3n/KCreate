// Phase 9 Block D Task 21 — Ruler + measurement-guide overlay.
//
// Renders horizontal + vertical pixel rulers along the top and left
// edges of the canvas viewport, with a tick every 50 pixels (in
// document space) when zoomed in and every 100 when zoomed out.
//
// Click-drag from a ruler creates a guide line via
// `window.kcreate.phase9.guideCreate`. Existing guides on the
// current page are read from `guideList` and drawn over the canvas.
//
// The overlay is purely visual: snapping is enforced by the Rust
// snap engine (see `crates/kcreate_vector/src/snap.rs`).

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { GuideInfo } from "../../../shared/scene";
import { font } from "../styles/tokens";

interface RulerOverlayProps {
  /** The current page id; ruler queries guides scoped to this page. */
  pageId: string | null;
  /** Viewport pan offset (document → screen) in CSS pixels. */
  panX: number;
  panY: number;
  /** Viewport zoom factor (document → screen). */
  zoom: number;
  /** Width / height of the viewport's CSS area. */
  width: number;
  height: number;
  /** Ruler thickness in CSS pixels. */
  rulerSize?: number;
}

const TICK_SHORT = 4;
const TICK_LONG = 8;
const LABEL_FONT = `10px ${font.family}`;
const RULER_BG = "#1f2329";
const RULER_FG = "#e7e8ea";
const GUIDE_COLOR = "#79b8ff";

export function RulerOverlay({
  pageId,
  panX,
  panY,
  zoom,
  width,
  height,
  rulerSize = 24,
}: RulerOverlayProps): JSX.Element | null {
  const [guides, setGuides] = useState<GuideInfo[]>([]);
  const [draggingFrom, setDraggingFrom] = useState<
    "top" | "left" | null
  >(null);
  const [dragPos, setDragPos] = useState<number>(0);
  const [error, setError] = useState<string | undefined>(undefined);
  /**
   * Ref to the overlay's outer `<div>` so we can read its
   * bounding rect on each pointer event and translate viewport-
   * relative `clientX`/`clientY` into overlay-local coordinates.
   * Without this, both the drag-line preview (which uses `dragPos`
   * as `top:` / `left:` inside the overlay) AND the document-space
   * guide position computed in `handlePointerUp` are wrong
   * whenever the overlay's top-left does not coincide with the
   * viewport origin (toolbars above, sidebars left, app chrome,
   * etc.). The overlay's own bounding rect is the single source
   * of truth for that offset.
   */
  const containerRef = useRef<HTMLDivElement | null>(null);

  const refresh = useCallback(async () => {
    if (pageId === null) return;
    try {
      const list = await window.kcreate.phase9.guideList(pageId);
      setGuides(list);
      setError(undefined);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }, [pageId]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // Step size in document px between major ticks. We aim for a
  // ~100 CSS-pixel gap between major ticks at the current zoom.
  const majorStep = useMemo(() => {
    const targetCss = 100;
    const docPerCss = 1 / zoom;
    const raw = targetCss * docPerCss;
    // Round to a "nice" step (1, 2, 5, 10, 20, 50, 100, …).
    const pow = Math.pow(10, Math.floor(Math.log10(raw)));
    const lead = raw / pow;
    if (lead < 2) return pow;
    if (lead < 5) return 2 * pow;
    return 5 * pow;
  }, [zoom]);

  /**
   * Translate a viewport-relative pointer event into a position
   * that is local to the overlay element along the given axis.
   * Returns `null` if the container ref hasn't been attached yet
   * (which would only happen during the very first render before
   * React commits the ref, but is handled defensively).
   */
  const localPos = useCallback(
    (e: React.PointerEvent, axis: "x" | "y"): number | null => {
      const rect = containerRef.current?.getBoundingClientRect();
      if (rect === undefined) return null;
      return axis === "y" ? e.clientY - rect.top : e.clientX - rect.left;
    },
    [],
  );

  const handleRulerDown = useCallback(
    (which: "top" | "left", e: React.PointerEvent) => {
      e.preventDefault();
      const pos = localPos(e, which === "top" ? "y" : "x");
      if (pos === null) return;
      setDraggingFrom(which);
      setDragPos(pos);
      (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    },
    [localPos],
  );

  const handlePointerMove = useCallback(
    (e: React.PointerEvent) => {
      if (draggingFrom === null) return;
      const pos = localPos(e, draggingFrom === "top" ? "y" : "x");
      if (pos === null) return;
      setDragPos(pos);
    },
    [draggingFrom, localPos],
  );

  const handlePointerUp = useCallback(
    async (e: React.PointerEvent) => {
      if (draggingFrom === null || pageId === null) return;
      const from = draggingFrom;
      setDraggingFrom(null);
      // Convert overlay-local CSS coordinates → document
      // coordinates, then call the bridge to materialize the
      // guide. Using `localPos` here (instead of raw `e.clientY` /
      // `e.clientX`) is critical: the overlay is mounted inside
      // the editor's layout grid with toolbars / panels above and
      // to the left, so the viewport origin and the overlay
      // origin do not coincide. `panX` / `panY` are defined
      // relative to the overlay's own coordinate system, so the
      // doc-space math `(screenPos - pan) / zoom` is only correct
      // when `screenPos` is also overlay-local.
      const pos = localPos(e, from === "top" ? "y" : "x");
      if (pos === null) return;
      const pan = from === "top" ? panY : panX;
      const docPos = (pos - pan) / zoom;
      try {
        await window.kcreate.phase9.guideCreate(
          pageId,
          from === "top" ? "horizontal" : "vertical",
          docPos,
          GUIDE_COLOR,
          false,
        );
        await refresh();
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
      }
    },
    [draggingFrom, pageId, panX, panY, zoom, refresh, localPos],
  );

  if (pageId === null) {
    return null;
  }

  const horizontalGuides = guides.filter((g) => g.orientation === "horizontal");
  const verticalGuides = guides.filter((g) => g.orientation === "vertical");

  return (
    <div
      ref={containerRef}
      style={overlayStyle(width, height)}
      onPointerMove={handlePointerMove}
      onPointerUp={(e) => void handlePointerUp(e)}
      data-testid="kcreate-ruler-overlay"
    >
      {/* Top ruler */}
      <div
        onPointerDown={(e) => handleRulerDown("top", e)}
        style={topRulerStyle(rulerSize)}
        data-testid="kcreate-ruler-top"
      >
        <RulerCanvas
          orientation="horizontal"
          length={width}
          thickness={rulerSize}
          pan={panX}
          zoom={zoom}
          majorStep={majorStep}
        />
      </div>
      {/* Left ruler */}
      <div
        onPointerDown={(e) => handleRulerDown("left", e)}
        style={leftRulerStyle(rulerSize, height)}
        data-testid="kcreate-ruler-left"
      >
        <RulerCanvas
          orientation="vertical"
          length={height}
          thickness={rulerSize}
          pan={panY}
          zoom={zoom}
          majorStep={majorStep}
        />
      </div>
      {/* Existing guides */}
      {horizontalGuides.map((g) => (
        <div
          key={g.id}
          style={{
            position: "absolute",
            left: 0,
            right: 0,
            top: g.position * zoom + panY,
            height: 1,
            background: g.color || GUIDE_COLOR,
            pointerEvents: "none",
          }}
          data-testid="kcreate-guide-horizontal"
          data-guide-id={g.id}
        />
      ))}
      {verticalGuides.map((g) => (
        <div
          key={g.id}
          style={{
            position: "absolute",
            top: 0,
            bottom: 0,
            left: g.position * zoom + panX,
            width: 1,
            background: g.color || GUIDE_COLOR,
            pointerEvents: "none",
          }}
          data-testid="kcreate-guide-vertical"
          data-guide-id={g.id}
        />
      ))}
      {/* In-flight drag indicator */}
      {draggingFrom !== null && (
        <div
          style={
            draggingFrom === "top"
              ? {
                  position: "absolute",
                  left: 0,
                  right: 0,
                  top: dragPos,
                  height: 1,
                  background: GUIDE_COLOR,
                  pointerEvents: "none",
                }
              : {
                  position: "absolute",
                  top: 0,
                  bottom: 0,
                  left: dragPos,
                  width: 1,
                  background: GUIDE_COLOR,
                  pointerEvents: "none",
                }
          }
        />
      )}
      {error !== undefined && (
        <p role="alert" style={errorOverlayStyle}>
          {error}
        </p>
      )}
    </div>
  );
}

function RulerCanvas({
  orientation,
  length,
  thickness,
  pan,
  zoom,
  majorStep,
}: {
  orientation: "horizontal" | "vertical";
  length: number;
  thickness: number;
  pan: number;
  zoom: number;
  majorStep: number;
}): JSX.Element {
  // Compute tick positions in CSS coordinates.
  const startDoc = Math.floor(-pan / zoom / majorStep) * majorStep;
  const ticks: { cssPos: number; label: number }[] = [];
  for (let doc = startDoc; ; doc += majorStep) {
    const cssPos = doc * zoom + pan;
    if (cssPos > length + 50) break;
    if (cssPos < -50) continue;
    ticks.push({ cssPos, label: doc });
  }

  if (orientation === "horizontal") {
    return (
      <svg
        width={length}
        height={thickness}
        style={{ display: "block" }}
        role="img"
        aria-label="Horizontal ruler"
      >
        <rect width={length} height={thickness} fill={RULER_BG} />
        {ticks.map((t, i) => (
          <g key={i}>
            <line
              x1={t.cssPos}
              x2={t.cssPos}
              y1={thickness - TICK_LONG}
              y2={thickness}
              stroke={RULER_FG}
              strokeWidth={1}
            />
            <text
              x={t.cssPos + 3}
              y={thickness - TICK_LONG - 2}
              fill={RULER_FG}
              style={{ font: LABEL_FONT }}
            >
              {t.label}
            </text>
          </g>
        ))}
      </svg>
    );
  }
  return (
    <svg
      width={thickness}
      height={length}
      style={{ display: "block" }}
      role="img"
      aria-label="Vertical ruler"
    >
      <rect width={thickness} height={length} fill={RULER_BG} />
      {ticks.map((t, i) => (
        <g key={i}>
          <line
            x1={thickness - TICK_LONG}
            x2={thickness}
            y1={t.cssPos}
            y2={t.cssPos}
            stroke={RULER_FG}
            strokeWidth={1}
          />
          <text
            x={2}
            y={t.cssPos + 10}
            fill={RULER_FG}
            style={{ font: LABEL_FONT }}
          >
            {t.label}
          </text>
        </g>
      ))}
    </svg>
  );
}

const overlayStyle = (
  width: number,
  height: number,
): React.CSSProperties => ({
  position: "absolute",
  inset: 0,
  width,
  height,
  pointerEvents: "none",
});

const topRulerStyle = (size: number): React.CSSProperties => ({
  position: "absolute",
  top: 0,
  left: 0,
  right: 0,
  height: size,
  pointerEvents: "auto",
  cursor: "ns-resize",
});

const leftRulerStyle = (
  size: number,
  height: number,
): React.CSSProperties => ({
  position: "absolute",
  top: 0,
  left: 0,
  width: size,
  height,
  pointerEvents: "auto",
  cursor: "ew-resize",
});

const errorOverlayStyle: React.CSSProperties = {
  position: "absolute",
  bottom: 12,
  left: 12,
  margin: 0,
  padding: "4px 8px",
  background: "rgba(185,28,28,0.85)",
  color: "white",
  fontSize: 11,
  borderRadius: 4,
  pointerEvents: "none",
};

// "TICK_SHORT" reserved for future minor-tick rendering (8 subdivisions
// at finer zoom levels). The current draw uses major ticks only.
void TICK_SHORT;
