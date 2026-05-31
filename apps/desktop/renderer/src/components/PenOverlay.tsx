// Phase B1 — pen-tool overlay.
//
// Renders the in-flight pen gesture (committed anchors, in-flight
// pending anchor + drag handles, and a rubber-band preview from the
// last committed anchor to the cursor) as an absolutely-positioned
// SVG layered on top of the canvas. Subscribes to the
// `ToolStateMachine` so anchor advances re-paint without forcing a
// full editor re-render — the state machine mutates its ref-backed
// state on every pointer event and notifies subscribers, which is
// the only way the overlay can stay in sync at pointer-event rate.
//
// Coordinate convention: every anchor / handle / cursor coord is in
// world space, projected to screen via `screen = world * zoom + pan`
// — the same convention `SnapGuidesOverlay` / `SelectionOverlay` /
// `CursorOverlay` use, so all four overlays agree on the projection
// and a viewport pan/zoom shifts them in lockstep.
//
// The overlay is `pointer-events: none` so it never intercepts
// clicks; the pen tool's pointer handler is wired on the canvas
// itself via `onCanvasPointer`.

import { useSyncExternalStore, type JSX } from "react";

import type { ViewportState } from "./CanvasHost";
import type {
  PenAnchor,
  ToolStateMachine,
} from "../hooks/useToolStateMachine";

/// Visual radius (in screen px) of a committed anchor dot. Sized to
/// match `PEN_CLOSE_HIT_RADIUS_SCREEN` so the user can see exactly
/// what they need to click to close the path.
const ANCHOR_RADIUS_PX = 4;

/// Visual radius (in screen px) of a control-point handle dot.
/// Smaller than the anchor so the eye groups the anchor as primary
/// and the handles as secondary.
const HANDLE_RADIUS_PX = 3;

/// Stroke colour for the committed path + handle tangent lines.
/// Same magenta as `SnapGuidesOverlay` so the in-flight visuals
/// read as "transient editor chrome" not "real layer content".
const PEN_STROKE = "#ff00ff";

/// Faded version of `PEN_STROKE` for the rubber-band preview (the
/// segment from the last committed anchor to the cursor). Solid
/// stroke would visually compete with the committed path — the
/// dashed + lower-opacity treatment matches Figma's "ghost segment".
const PEN_GHOST_STROKE = "rgba(255, 0, 255, 0.55)";

export interface PenOverlayProps {
  /// The pen tool's state machine handle, returned by
  /// `useToolStateMachine`. Used to subscribe to in-flight gesture
  /// updates and to read the current state synchronously inside the
  /// `getSnapshot` callback. The overlay itself doesn't mutate
  /// state — it's read-only.
  machine: ToolStateMachine;
  /// Live viewport (pan + zoom). Re-projects all world-space coords
  /// to screen on every render; the overlay re-renders both on
  /// state-machine notify AND on viewport changes (the latter via
  /// React's normal prop-change dataflow).
  viewport: ViewportState;
  /// Canvas dimensions in CSS px — drives the SVG root size so the
  /// overlay exactly covers the canvas surface.
  width: number;
  height: number;
}

/**
 * Pen-tool overlay. Reads the state machine via
 * `useSyncExternalStore` (so React re-renders on `notify()` from
 * the machine) and projects every anchor / handle / cursor into
 * screen space using the live viewport.
 *
 * Returns `null` when the state machine is not in the `"pen"`
 * variant — there's nothing to draw. The overlay always renders
 * the root `<svg>` element when active, even with zero anchors,
 * because returning `null` mid-gesture would flicker if the
 * gesture had a brief idle state between events (it doesn't
 * today, but future tools that delegate to the pen machinery
 * might).
 */
export function PenOverlay({
  machine,
  viewport,
  width,
  height,
}: PenOverlayProps): JSX.Element | null {
  const state = useSyncExternalStore(
    machine.subscribe,
    machine.getState,
    // Server snapshot — never called in Electron, but required by
    // the `useSyncExternalStore` signature. Returning the same
    // "idle" sentinel as the client default keeps the SSR-safety
    // contract trivially satisfied.
    machine.getState,
  );
  if (state.kind !== "pen") return null;

  const { panX, panY, zoom } = viewport;
  const project = (wx: number, wy: number): { x: number; y: number } => ({
    x: wx * zoom + panX,
    y: wy * zoom + panY,
  });

  // Build the committed-anchor path (`<path d="M ..." />`) by
  // walking anchors[] and emitting M / L / C ops that mirror the
  // `anchorsToSegments` Rust-wire conversion. Keeps the visual
  // preview lockstep with what `commitPenGesture` will eventually
  // send to the bridge — if a future segment kind is added (e.g.
  // arcs), both paths need to grow together.
  let d = "";
  if (state.anchors.length > 0) {
    const first = state.anchors[0]!;
    const fp = project(first.x, first.y);
    d = `M ${fp.x} ${fp.y}`;
    for (let i = 1; i < state.anchors.length; i++) {
      const prev = state.anchors[i - 1]!;
      const curr = state.anchors[i]!;
      const ep = project(curr.x, curr.y);
      if (prev.outHandle === null && curr.inHandle === null) {
        d += ` L ${ep.x} ${ep.y}`;
      } else {
        const c1p = project(
          prev.outHandle?.x ?? prev.x,
          prev.outHandle?.y ?? prev.y,
        );
        const c2p = project(
          curr.inHandle?.x ?? curr.x,
          curr.inHandle?.y ?? curr.y,
        );
        d += ` C ${c1p.x} ${c1p.y}, ${c2p.x} ${c2p.y}, ${ep.x} ${ep.y}`;
      }
    }
  }

  // Rubber-band ghost segment from the last committed anchor to
  // the current cursor (or the pending anchor's drag handle, if a
  // drag is in flight). Mirrors how Illustrator / Figma preview
  // the segment "you're about to commit when you next click".
  let ghostD = "";
  if (state.anchors.length > 0 && state.cursor) {
    const last = state.anchors[state.anchors.length - 1]!;
    const lp = project(last.x, last.y);
    const cp = project(state.cursor.x, state.cursor.y);
    // If the last anchor is a smooth anchor (has an outHandle),
    // preview the segment as a cubic with that handle and a
    // mirror at the cursor for symmetry — this is what the
    // commit will actually produce if the next click is a
    // corner. If the last anchor is a corner, preview as a
    // straight line.
    if (last.outHandle) {
      const out = project(last.outHandle.x, last.outHandle.y);
      ghostD = `M ${lp.x} ${lp.y} C ${out.x} ${out.y}, ${cp.x} ${cp.y}, ${cp.x} ${cp.y}`;
    } else {
      ghostD = `M ${lp.x} ${lp.y} L ${cp.x} ${cp.y}`;
    }
  }

  return (
    <svg
      width={width}
      height={height}
      style={{
        position: "absolute",
        top: 0,
        left: 0,
        pointerEvents: "none",
      }}
    >
      {d ? (
        <path d={d} stroke={PEN_STROKE} strokeWidth={1} fill="none" />
      ) : null}
      {ghostD ? (
        <path
          d={ghostD}
          stroke={PEN_GHOST_STROKE}
          strokeWidth={1}
          strokeDasharray="4 2"
          fill="none"
        />
      ) : null}
      {state.anchors.map((a, i) => (
        <AnchorMarker key={`a-${i}`} anchor={a} project={project} />
      ))}
      {state.pending ? (
        <PendingAnchorMarker
          pending={state.pending}
          project={project}
        />
      ) : null}
    </svg>
  );
}

/// Render a single committed anchor: the anchor dot plus, if it's
/// a smooth anchor, its incoming and outgoing handles (small dots)
/// joined to the anchor by 1 px tangent lines.
function AnchorMarker({
  anchor,
  project,
}: {
  anchor: PenAnchor;
  project: (wx: number, wy: number) => { x: number; y: number };
}): JSX.Element {
  const a = project(anchor.x, anchor.y);
  return (
    <g>
      {anchor.inHandle ? (
        <>
          <line
            x1={a.x}
            y1={a.y}
            x2={project(anchor.inHandle.x, anchor.inHandle.y).x}
            y2={project(anchor.inHandle.x, anchor.inHandle.y).y}
            stroke={PEN_STROKE}
            strokeWidth={1}
          />
          <circle
            cx={project(anchor.inHandle.x, anchor.inHandle.y).x}
            cy={project(anchor.inHandle.x, anchor.inHandle.y).y}
            r={HANDLE_RADIUS_PX}
            fill="#ffffff"
            stroke={PEN_STROKE}
            strokeWidth={1}
          />
        </>
      ) : null}
      {anchor.outHandle ? (
        <>
          <line
            x1={a.x}
            y1={a.y}
            x2={project(anchor.outHandle.x, anchor.outHandle.y).x}
            y2={project(anchor.outHandle.x, anchor.outHandle.y).y}
            stroke={PEN_STROKE}
            strokeWidth={1}
          />
          <circle
            cx={project(anchor.outHandle.x, anchor.outHandle.y).x}
            cy={project(anchor.outHandle.x, anchor.outHandle.y).y}
            r={HANDLE_RADIUS_PX}
            fill="#ffffff"
            stroke={PEN_STROKE}
            strokeWidth={1}
          />
        </>
      ) : null}
      <rect
        x={a.x - ANCHOR_RADIUS_PX}
        y={a.y - ANCHOR_RADIUS_PX}
        width={ANCHOR_RADIUS_PX * 2}
        height={ANCHOR_RADIUS_PX * 2}
        fill="#ffffff"
        stroke={PEN_STROKE}
        strokeWidth={1}
      />
    </g>
  );
}

/// Render the pending (in-flight) anchor: the anchor dot itself
/// plus, if the user is mid-drag, the tangent line from the anchor
/// to the cursor (which becomes the smooth anchor's outHandle on
/// release). Highlighted slightly differently from committed
/// anchors so the user can see which anchor they're currently
/// laying.
function PendingAnchorMarker({
  pending,
  project,
}: {
  pending: { x: number; y: number; drag: { x: number; y: number } | null };
  project: (wx: number, wy: number) => { x: number; y: number };
}): JSX.Element {
  const a = project(pending.x, pending.y);
  return (
    <g>
      {pending.drag ? (
        <>
          {(() => {
            const d = project(pending.drag.x, pending.drag.y);
            // Symmetric reflection through the anchor — the inHandle
            // preview, which mirrors the commit-time computation
            // in `pointerup` (smooth anchor branch).
            const m = {
              x: 2 * a.x - d.x,
              y: 2 * a.y - d.y,
            };
            return (
              <>
                <line
                  x1={m.x}
                  y1={m.y}
                  x2={d.x}
                  y2={d.y}
                  stroke={PEN_STROKE}
                  strokeWidth={1}
                />
                <circle
                  cx={d.x}
                  cy={d.y}
                  r={HANDLE_RADIUS_PX}
                  fill="#ffffff"
                  stroke={PEN_STROKE}
                  strokeWidth={1}
                />
                <circle
                  cx={m.x}
                  cy={m.y}
                  r={HANDLE_RADIUS_PX}
                  fill="#ffffff"
                  stroke={PEN_STROKE}
                  strokeWidth={1}
                />
              </>
            );
          })()}
        </>
      ) : null}
      <rect
        x={a.x - ANCHOR_RADIUS_PX}
        y={a.y - ANCHOR_RADIUS_PX}
        width={ANCHOR_RADIUS_PX * 2}
        height={ANCHOR_RADIUS_PX * 2}
        fill={PEN_STROKE}
        stroke={PEN_STROKE}
        strokeWidth={1}
      />
    </g>
  );
}
