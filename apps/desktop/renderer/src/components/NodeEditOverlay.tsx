// Phase B3 — Node-editor overlay.
//
// Renders the in-flight node-edit gesture for a VectorLayer:
// outlined anchor squares (filled when selected, hollow when
// idle), small control-handle dots at each handle position, and
// dashed tangent lines between each anchor and its in/out
// handles. Mirrors the SVG-overlay pattern PenOverlay
// established (absolutely-positioned SVG, pointer-events: none,
// projects world coords via `screen = world * zoom + pan`,
// subscribes to the state machine via `useSyncExternalStore`).
//
// Returns `null` when the state machine is not in the
// `"nodeEdit"` variant — there's nothing to draw.

import { useSyncExternalStore, type JSX } from "react";

import type { ViewportState } from "./CanvasHost";
import type {
  PenAnchor,
  ToolStateMachine,
} from "../hooks/useToolStateMachine";

/// Visual side length (in screen px) of an anchor square. Wider
/// than `PenOverlay`'s 4 px anchor dot because the node editor
/// needs the user to see + grab individual anchors precisely; the
/// hit radius (8 px) is even larger than the visible glyph so
/// imperfect clicks still register.
const ANCHOR_SIDE_PX = 8;

/// Visual radius (in screen px) of a control-handle dot. Matched
/// to `PenOverlay`'s `HANDLE_RADIUS_PX` so handles look identical
/// across the two tools — the user is editing the same kind of
/// geometric primitive in both, just at different lifecycle
/// stages (creating vs. editing).
const HANDLE_RADIUS_PX = 3;

/// Stroke colour for the path outline + handle tangent lines.
/// Same magenta as `PenOverlay`'s `PEN_STROKE` so the in-flight
/// visuals read as "transient editor chrome" across both tools.
const NODE_STROKE = "#ff00ff";

/// Faded version of `NODE_STROKE` for the tangent lines between
/// each anchor and its control handles. Same opacity trick as
/// `PEN_GHOST_STROKE` to keep the tangent lines visually
/// secondary to the path outline.
const NODE_HANDLE_LINE_STROKE = "rgba(255, 0, 255, 0.55)";

/// Fill colour for a SELECTED anchor square. Same magenta as the
/// stroke so selection state reads as "this is the colour the
/// editor uses for active geometry."
const NODE_ANCHOR_SELECTED_FILL = "#ff00ff";

/// Fill colour for an UNSELECTED anchor square. White
/// (background-of-overlay) so the anchor reads as hollow + the
/// magenta outline is what the eye groups on.
const NODE_ANCHOR_IDLE_FILL = "#ffffff";

export interface NodeEditOverlayProps {
  /// The node editor's state machine handle, returned by
  /// `useToolStateMachine`. Used to subscribe to in-flight gesture
  /// updates and to read the current state synchronously inside
  /// `getSnapshot`. Mirrors `PenOverlayProps.machine`.
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
 * Node-editor overlay. Reads the state machine via
 * `useSyncExternalStore` (so React re-renders on `notify()` from
 * the machine) and projects every anchor / handle into screen
 * space using the live viewport.
 *
 * Visual layering (back to front, matching SVG painter's
 * algorithm):
 *   1. Path outline (`<path d="...">`) — solid magenta line
 *      walking the anchor sequence, segments built with the same
 *      line-vs-cubic decision rule as `anchorsToSegments` so the
 *      visual matches the bridge-side geometry exactly.
 *   2. Per-anchor: tangent lines (anchor ↔ in/out handles) — dashed
 *      magenta-at-55% so they read as transient.
 *   3. Per-anchor: control-handle dots — solid magenta.
 *   4. Per-anchor: anchor square — filled magenta if selected,
 *      hollow white if idle. Always on top so the user can
 *      always see + grab the anchor itself.
 */
export function NodeEditOverlay({
  machine,
  viewport,
  width,
  height,
}: NodeEditOverlayProps): JSX.Element | null {
  const state = useSyncExternalStore(
    machine.subscribe,
    machine.getState,
    // Server snapshot — never called in Electron, but required by
    // the `useSyncExternalStore` signature. Returning the same
    // "idle" sentinel as the client default keeps the SSR-safety
    // contract trivially satisfied. Same approach as PenOverlay.
    machine.getState,
  );
  if (state.kind !== "nodeEdit") return null;

  const { panX, panY, zoom } = viewport;
  const project = (wx: number, wy: number): { x: number; y: number } => ({
    x: wx * zoom + panX,
    y: wy * zoom + panY,
  });

  // Build the path-outline `d` attribute by walking the anchor
  // sequence and emitting M / L / C ops that mirror the
  // `anchorsToSegments` Rust-wire conversion. Same rule as
  // `PenOverlay`: a pair of pure-corner anchors becomes a
  // straight line; any handle on either side promotes the
  // segment to a cubic.
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
    if (state.closed && state.anchors.length >= 2) {
      // Closing segment back to the first anchor uses the same
      // line-vs-cubic rule. Bundles into `Z` after the explicit
      // C/L so the user can see the closing segment's curvature.
      const prev = state.anchors[state.anchors.length - 1]!;
      const curr = state.anchors[0]!;
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
      d += " Z";
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
      data-testid="node-edit-overlay"
    >
      {d ? (
        <path
          d={d}
          stroke={NODE_STROKE}
          strokeWidth={1}
          fill="none"
        />
      ) : null}
      {state.anchors.map((a, i) => (
        <NodeAnchorGroup
          key={`a-${i}`}
          anchor={a}
          index={i}
          selected={state.selectedAnchorIndices.has(i)}
          project={project}
        />
      ))}
    </svg>
  );
}

/// Render one anchor + its tangent lines + handle dots. Drawn
/// in a single `<g>` so a future tweak (e.g. hover halo, lock
/// glyph) can decorate the entire anchor cluster without
/// touching the projection logic.
function NodeAnchorGroup({
  anchor,
  index,
  selected,
  project,
}: {
  anchor: PenAnchor;
  index: number;
  selected: boolean;
  project: (wx: number, wy: number) => { x: number; y: number };
}): JSX.Element {
  const ap = project(anchor.x, anchor.y);
  const inP = anchor.inHandle
    ? project(anchor.inHandle.x, anchor.inHandle.y)
    : null;
  const outP = anchor.outHandle
    ? project(anchor.outHandle.x, anchor.outHandle.y)
    : null;
  const half = ANCHOR_SIDE_PX / 2;
  return (
    <g data-testid={`node-anchor-${index}`}>
      {inP ? (
        <line
          x1={ap.x}
          y1={ap.y}
          x2={inP.x}
          y2={inP.y}
          stroke={NODE_HANDLE_LINE_STROKE}
          strokeWidth={1}
          strokeDasharray="3 2"
        />
      ) : null}
      {outP ? (
        <line
          x1={ap.x}
          y1={ap.y}
          x2={outP.x}
          y2={outP.y}
          stroke={NODE_HANDLE_LINE_STROKE}
          strokeWidth={1}
          strokeDasharray="3 2"
        />
      ) : null}
      {inP ? (
        <circle
          cx={inP.x}
          cy={inP.y}
          r={HANDLE_RADIUS_PX}
          fill={NODE_STROKE}
          data-testid={`node-anchor-${index}-handle-in`}
        />
      ) : null}
      {outP ? (
        <circle
          cx={outP.x}
          cy={outP.y}
          r={HANDLE_RADIUS_PX}
          fill={NODE_STROKE}
          data-testid={`node-anchor-${index}-handle-out`}
        />
      ) : null}
      <rect
        x={ap.x - half}
        y={ap.y - half}
        width={ANCHOR_SIDE_PX}
        height={ANCHOR_SIDE_PX}
        fill={
          selected
            ? NODE_ANCHOR_SELECTED_FILL
            : NODE_ANCHOR_IDLE_FILL
        }
        stroke={NODE_STROKE}
        strokeWidth={1}
        data-testid={`node-anchor-${index}-rect`}
        data-selected={selected ? "true" : "false"}
      />
    </g>
  );
}
