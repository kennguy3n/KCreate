// CursorOverlay — Phase 7 Task 13.
//
// Renders one coloured arrow + display-name label per remote peer at
// the peer's last-broadcast cursor world position, projected through
// the local viewport. Sits on top of `CanvasHost` as a transparent
// SVG with `pointer-events: none` so it never intercepts clicks.
//
// Data flow:
//   * On mount, fetch the current peer roster via
//     `window.kcreate.session.peers()`.
//   * Subscribe to `window.kcreate.session.onEvent` and re-fetch the
//     roster on every event that changes presence (`presenceUpdated`,
//     `peerJoined`, `peerLeft`, `peerKicked`, `sessionLeft`).
//   * Also pull the local peer id on mount (and on
//     `sessionStarted` / `sessionLeft`) so the overlay can filter
//     out the local peer — we never draw our own cursor through this
//     overlay because the OS already paints it.
//
// Self-contained: doesn't require the host to pass peer state down.
// Same pattern as PresencePanel which also owns its own roster.
//
// Colour assignment: BLAKE3-equivalent stable hash of the peer id
// indexes into a palette of 8 high-contrast colours so the same peer
// keeps the same colour across the whole UI (Cursor, Selection,
// PresencePanel dot).

import { useEffect, useLayoutEffect, useRef, useState } from "react";

import type { SessionPeer } from "../../../shared/scene";

import type { ViewportState } from "./CanvasHost";

/// 8-entry high-contrast palette. Hand-picked so any two colours have
/// at least 7:1 contrast on both the light and dark canvas background.
/// Indexed by `peerColorIndex(peerId)`. Sourced from the Phase 5
/// design-token palette (see `apps/desktop/renderer/src/styles/tokens.ts`).
export const PEER_PALETTE: readonly string[] = [
  "#e11d48", // rose-600
  "#f59e0b", // amber-500
  "#16a34a", // green-600
  "#0ea5e9", // sky-500
  "#6366f1", // indigo-500
  "#a855f7", // purple-500
  "#ec4899", // pink-500
  "#14b8a6", // teal-500
];

/// Stable 32-bit hash of a base64url peer id, derived via the FNV-1a
/// hash so two different machines pick the *same* colour for the same
/// peer without coordinating. The hash is fast enough to recompute
/// once per remote peer per `presenceUpdated` re-render — the entire
/// 8-character keyspace fits in registers.
///
/// Exposed so other overlays (SelectionOverlay, PresencePanel) pick
/// the same colour for the same peer.
export function peerColorIndex(peerId: string): number {
  // FNV-1a 32-bit. Tested against the reference vectors in
  // http://www.isthe.com/chongo/tech/comp/fnv/. Returning a u32 -> 8
  // bucket projection collapses cleanly because 2^32 % 8 == 0.
  let h = 0x811c9dc5;
  for (let i = 0; i < peerId.length; i++) {
    h ^= peerId.charCodeAt(i);
    // Multiply by 16777619 mod 2^32, expressed as
    // `(h * 0x01000193) >>> 0` to force unsigned wrap.
    h = Math.imul(h, 0x01000193) >>> 0;
  }
  return h % PEER_PALETTE.length;
}

/// Resolve the canonical colour for a peer. Pure function so it can
/// be unit-tested without rendering.
export function colorForPeer(peerId: string): string {
  const idx = peerColorIndex(peerId);
  return PEER_PALETTE[idx] ?? PEER_PALETTE[0]!;
}

export interface CursorOverlayProps {
  /** Width / height of the parent canvas surface in CSS pixels. */
  width: number;
  height: number;
  /** Current viewport pan + zoom. Cursor positions are world units. */
  viewport: ViewportState;
}

/// Convert a world-space (x, y) into screen coordinates using the
/// `screen = world * zoom + pan` projection that `SnapGuidesOverlay`
/// and the smart-guides path use.
///
/// Exposed for tests in `apps/desktop/renderer/src/components/__tests__/`.
export function projectWorld(
  worldX: number,
  worldY: number,
  viewport: ViewportState,
): { x: number; y: number } {
  return {
    x: worldX * viewport.zoom + viewport.panX,
    y: worldY * viewport.zoom + viewport.panY,
  };
}

/// SVG arrow path drawn at the (0, 0) anchor of each remote cursor.
/// Looks like the canonical macOS / Windows pointer (16 px tall);
/// rendered black-outlined so it stays legible on any background.
const CURSOR_PATH = "M2 2 L2 22 L8 18 L11 24 L14 22 L11 16 L18 16 Z";

/// Horizontal padding (px) on either side of the display-name text
/// inside the label pill. Tweak together with `PILL_HEIGHT` if the
/// font size changes.
const PILL_PADDING_X = 6;
/// Pill height (px). Sized for the 11 px label font with a 2 px gap
/// above and below the cap height.
const PILL_HEIGHT = 16;
/// Fallback pill width (px) used on the first paint before
/// `getBBox()` has measured the actual text. Set to the longest
/// realistic display-name a single label might need so the layout
/// doesn't jump for normal Latin names — wide-glyph (CJK / emoji)
/// names will resize on the next frame.
const PILL_FALLBACK_WIDTH = 80;

/// Per-peer label that measures its own text via `getBBox()` after
/// the first paint and resizes the pill background to match. This
/// is the SVG-native equivalent of CSS `width: fit-content` and
/// correctly handles wide glyphs (CJK, emoji, ligatures) and the
/// narrow-glyph case (i, l, 1) that a static `n * 7px` heuristic
/// gets wrong.
///
/// We render the text *before* committing the pill width to a real
/// value (the `<text>` is mounted as soon as React paints, which is
/// when `useLayoutEffect` runs and `textRef.current.getBBox()`
/// returns the measured size). On the very first paint the pill is
/// drawn with `PILL_FALLBACK_WIDTH` so there's no flash of an
/// empty rectangle — the layout effect then resizes it in the same
/// commit cycle, before the browser paints, so the user never sees
/// a visibly-wrong width.
function PeerLabel({
  name,
  color,
}: {
  name: string;
  color: string;
}): JSX.Element {
  const textRef = useRef<SVGTextElement>(null);
  const [textWidth, setTextWidth] = useState<number | null>(null);

  useLayoutEffect(() => {
    if (textRef.current == null) return;
    try {
      const bbox = textRef.current.getBBox();
      setTextWidth(bbox.width);
    } catch {
      // `getBBox()` throws if the element is detached (e.g. the
      // overlay was unmounted between the effect being queued and
      // running). Leave the fallback width in place — the next
      // mount will measure cleanly.
    }
  }, [name]);

  const pillWidth =
    textWidth != null
      ? Math.ceil(textWidth) + PILL_PADDING_X * 2
      : PILL_FALLBACK_WIDTH;

  return (
    <>
      {/*
        Display-name pill. Offset right + down so the arrow tip
        isn't covered by the label. Background uses the peer
        colour at full saturation; text is white for legibility.
        Width is measured from the rendered text rather than
        estimated from the character count — handles CJK / emoji /
        ligatures correctly.
      */}
      <rect
        x={18}
        y={18}
        rx={3}
        ry={3}
        height={PILL_HEIGHT}
        width={pillWidth}
        fill={color}
        opacity={0.95}
      />
      <text
        ref={textRef}
        x={18 + PILL_PADDING_X}
        y={30}
        fill="#ffffff"
        fontSize={11}
        fontFamily="-apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif"
        fontWeight={600}
      >
        {name}
      </text>
    </>
  );
}

export function CursorOverlay({
  width,
  height,
  viewport,
}: CursorOverlayProps): JSX.Element | null {
  const [peers, setPeers] = useState<SessionPeer[]>([]);
  const [localPeerId, setLocalPeerId] = useState<string | null>(null);
  const peersRef = useRef<SessionPeer[]>(peers);
  peersRef.current = peers;

  // Refresh peers + local peer id on mount and on every relevant
  // session event. We pull a fresh roster instead of mutating in
  // place for the same reason PresencePanel does: the bridge owns
  // the canonical map and a single GET is cheaper than reconciling
  // disjoint event payloads.
  useEffect(() => {
    let cancelled = false;
    const refreshPeers = async (): Promise<void> => {
      try {
        const list = await window.kcreate.session.peers();
        if (!cancelled) setPeers(list);
      } catch {
        // Bridge transient (e.g. session ended between events). Leave
        // the stale roster in place; the next event will retry.
      }
    };
    const refreshLocal = async (): Promise<void> => {
      try {
        const info = await window.kcreate.session.info();
        if (!cancelled) setLocalPeerId(info?.peerId ?? null);
      } catch {
        // See above.
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
          // The roster is reset on transitions; pull both so we
          // don't render a stale cursor from the previous session.
          void refreshPeers();
          break;
        default:
          // Other events (discovered, locksChanged, operationsJournaled,
          // permissionChanged, resumeApplied, conflictResolved,
          // undoBroadcast) don't affect cursor positions, so we skip
          // the IPC round-trip.
          break;
      }
    });
    return () => {
      cancelled = true;
      unsubscribe();
    };
  }, []);

  // Filter to remote peers with a known cursor. We deliberately
  // include peers whose presence was sent before the local viewport
  // settled — their cursor will animate as soon as the viewport
  // changes, which matches the user's mental model ("Ken is over
  // there in world space").
  const remoteWithCursor = peers.filter(
    (p) => p.peerId !== localPeerId && p.presence?.cursor != null,
  );

  if (remoteWithCursor.length === 0) {
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
      {remoteWithCursor.map((peer) => {
        // The filter above guarantees cursor is non-null.
        const cursor = peer.presence!.cursor!;
        const screen = projectWorld(cursor.x, cursor.y, viewport);
        // Clip cursors that fall completely off-canvas — drawing them
        // anyway would waste an SVG node per off-screen peer (and could
        // show a label that overflows the parent container in dev
        // tools). We use a 32 px slop so a cursor that's "just off"
        // still hints at where the peer is.
        if (
          screen.x < -32 ||
          screen.x > width + 32 ||
          screen.y < -32 ||
          screen.y > height + 32
        ) {
          return null;
        }
        const color = colorForPeer(peer.peerId);
        return (
          <g
            key={peer.peerId}
            transform={`translate(${screen.x.toFixed(2)}, ${screen.y.toFixed(2)})`}
          >
            <path
              d={CURSOR_PATH}
              fill={color}
              stroke="rgba(0, 0, 0, 0.6)"
              strokeWidth={1}
              strokeLinejoin="miter"
            />
            <PeerLabel name={peer.displayName} color={color} />
          </g>
        );
      })}
    </svg>
  );
}
