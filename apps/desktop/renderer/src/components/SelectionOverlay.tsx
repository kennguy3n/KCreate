// SelectionOverlay — Phase 7 Task 14.
//
// Renders a coloured outline around every node a remote peer has in
// its `presence.selection`. Same peer-colour assignment as
// `CursorOverlay` (FNV-1a hash → 8-entry palette) so it's visually
// obvious that "the green cursor and the green outline belong to
// the same person".
//
// Sits on top of `CanvasHost` as a transparent SVG with
// `pointer-events: none`. Same projection model as
// `SnapGuidesOverlay` and `CursorOverlay`: `screen = world * zoom + pan`.
//
// Data flow:
//   * Peer roster is owned here (same pull-on-event pattern as
//     CursorOverlay / PresencePanel) — the host doesn't have to thread
//     a peer list down.
//   * Node bounds are passed in as a `NodeInfo[]` prop because
//     EditorPage already owns that state (the layer panel needs it
//     too) and re-fetching here would duplicate the IPC traffic on
//     every node mutation.

import { useEffect, useMemo, useRef, useState } from "react";

import type { NodeInfo, SessionPeer } from "../../../shared/scene";

import type { ViewportState } from "./CanvasHost";
import { colorForPeer, projectWorld } from "./CursorOverlay";

export interface SelectionOverlayProps {
  /** Width / height of the parent canvas surface in CSS pixels. */
  width: number;
  height: number;
  /** Current viewport pan + zoom. Bounds are world units. */
  viewport: ViewportState;
  /**
   * Snapshot of the document's nodes with their world-space bounds.
   * Sourced from `window.kcreate.document.getTree()` and refreshed by
   * the host on every doc mutation. Allowed to be stale by one frame
   * — outlines just lag the selection by one render in that case.
   */
  nodes: NodeInfo[];
}

/// Stroke width used for the selection rectangle, in CSS pixels. Stays
/// visually consistent across zoom levels because we don't scale it
/// through the viewport (a 4-px outline at zoom=8 would be 32 px,
/// which would dominate the canvas).
const SELECTION_STROKE_PX = 2;

export function SelectionOverlay({
  width,
  height,
  viewport,
  nodes,
}: SelectionOverlayProps): JSX.Element | null {
  const [peers, setPeers] = useState<SessionPeer[]>([]);
  const [localPeerId, setLocalPeerId] = useState<string | null>(null);
  const peersRef = useRef<SessionPeer[]>(peers);
  peersRef.current = peers;

  useEffect(() => {
    let cancelled = false;
    const refreshPeers = async (): Promise<void> => {
      try {
        const list = await window.kcreate.session.peers();
        if (!cancelled) setPeers(list);
      } catch {
        // Bridge transient — keep stale roster.
      }
    };
    const refreshLocal = async (): Promise<void> => {
      try {
        const info = await window.kcreate.session.info();
        if (!cancelled) setLocalPeerId(info?.peerId ?? null);
      } catch {
        // Bridge transient — leave the previous local id alone so
        // we don't accidentally start drawing our own selection.
      }
    };
    void refreshPeers();
    void refreshLocal();
    const unsubscribe = window.kcreate.session.onEvent((ev) => {
      switch (ev.kind) {
        case "presenceUpdated":
        case "peerJoined":
        case "peerLeft":
        case "peerKicked":
          void refreshPeers();
          break;
        case "sessionStarted":
        case "sessionLeft":
          void refreshLocal();
          void refreshPeers();
          break;
        default:
          break;
      }
    });
    return () => {
      cancelled = true;
      unsubscribe();
    };
  }, []);

  // Index nodes by id for O(1) lookup. The list re-renders on every
  // doc mutation, so memoising on `nodes` is the right granularity.
  const nodeById = useMemo(() => {
    const m = new Map<string, NodeInfo>();
    for (const n of nodes) m.set(n.id, n);
    return m;
  }, [nodes]);

  // Flatten remote peers + their selection ids into a single list of
  // (peer, node) pairs to render. Skipping the local peer (we show
  // our own selection through the renderer's normal selection
  // outline, not through this overlay).
  const outlines = peers
    .filter((p) => p.peerId !== localPeerId && p.presence != null)
    .flatMap((p) => {
      const selection = p.presence?.selection ?? [];
      return selection
        .map((nodeId) => {
          const node = nodeById.get(nodeId);
          if (node == null) return null;
          return { peer: p, node };
        })
        .filter((x): x is { peer: SessionPeer; node: NodeInfo } => x != null);
    });

  if (outlines.length === 0) {
    return null;
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
      aria-hidden="true"
    >
      {outlines.map(({ peer, node }) => {
        const color = colorForPeer(peer.peerId);
        const topLeft = projectWorld(node.bounds.x, node.bounds.y, viewport);
        const bottomRight = projectWorld(
          node.bounds.x + node.bounds.width,
          node.bounds.y + node.bounds.height,
          viewport,
        );
        const x = Math.min(topLeft.x, bottomRight.x);
        const y = Math.min(topLeft.y, bottomRight.y);
        const w = Math.abs(bottomRight.x - topLeft.x);
        const h = Math.abs(bottomRight.y - topLeft.y);
        // Clip selections that are entirely off-canvas — the SVG
        // engine would handle this anyway, but we drop the node up
        // front so the DOM stays cheap when a peer selects a node
        // far outside the visible viewport.
        if (x + w < 0 || x > width || y + h < 0 || y > height) {
          return null;
        }
        return (
          <g key={`${peer.peerId}:${node.id}`}>
            <rect
              x={x}
              y={y}
              width={w}
              height={h}
              fill="none"
              stroke={color}
              strokeWidth={SELECTION_STROKE_PX}
              strokeDasharray="6 4"
            />
            {/*
              Small name pill in the top-left of the outline so the
              user can tell at a glance who's editing this node. Only
              rendered when the outline is large enough that the pill
              won't overflow the box (>= 48 × 18). Smaller outlines
              get only the dashed rectangle.
            */}
            {w >= 48 && h >= 18 && (
              <>
                <rect
                  x={x}
                  y={Math.max(y - 16, 0)}
                  width={Math.max(40, peer.displayName.length * 7 + 8)}
                  height={14}
                  rx={2}
                  ry={2}
                  fill={color}
                  opacity={0.95}
                />
                <text
                  x={x + 4}
                  y={Math.max(y - 16, 0) + 10}
                  fill="#ffffff"
                  fontSize={10}
                  fontFamily="-apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif"
                  fontWeight={600}
                >
                  {peer.displayName}
                </text>
              </>
            )}
          </g>
        );
      })}
    </svg>
  );
}
