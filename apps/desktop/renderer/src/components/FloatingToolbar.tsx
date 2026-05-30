// FloatingToolbar — Phase 10 Block C Task 16.
//
// A contextual toolbar that floats near the selection bounds instead
// of living on a permanent rail. Pure-renderer: it reads selection
// state from props and renders action callbacks; the actions
// themselves are existing bridge calls / commands wired by the
// caller. Position math is in `computeToolbarPosition` — exported so
// the test suite can verify edge clamping.

import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";

import type { NodeInfo } from "../../../shared/scene";
import { colors, radius, spacing } from "../styles/tokens";

export interface ViewportRect {
  /** Top-left corner of the canvas viewport in screen coordinates. */
  originX: number;
  originY: number;
  /** Viewport size in screen pixels. */
  width: number;
  height: number;
  /** Canvas → screen scale factor. */
  zoom: number;
  /** Canvas-space translation (pan). */
  panX: number;
  panY: number;
}

export interface FloatingToolbarAction {
  /** Stable id; used as `key`. */
  id: string;
  /** Short label shown in the button. */
  label: string;
  /** Optional ARIA label and hover tooltip. */
  hint?: string;
  /** Click handler. Async ok — the button stays clickable. */
  onClick: () => void;
  /** Optional left-of-label icon glyph. */
  icon?: string;
  /** When true, the button is rendered in the "primary" variant. */
  primary?: boolean;
}

export interface FloatingToolbarProps {
  selection: NodeInfo[];
  viewport: ViewportRect;
  /** Caller decides which actions are visible for `selection`. */
  actions: FloatingToolbarAction[];
  /** If true, the toolbar is suppressed. Wired to preferences. */
  disabled?: boolean;
  /** Dismiss on Escape or outside-click. */
  onDismiss: () => void;
}

/** Toolbar size hint used for edge clamping. The real DOM size is
 * measured after layout via `useLayoutEffect`. */
const TOOLBAR_W_HINT = 320;
const TOOLBAR_H_HINT = 36;
const GAP_ABOVE_SELECTION = 12;
const MIN_EDGE_PAD = 8;

/**
 * Decide where the toolbar should render, given the selection bounds
 * (canvas space), the viewport, and the toolbar's own size.
 *
 * Strategy:
 * 1. Convert the selection's top edge midpoint to screen space.
 * 2. Anchor the toolbar above the selection by `GAP_ABOVE_SELECTION`.
 * 3. If that would clip off-screen above, flip below the selection.
 * 4. Clamp horizontally so the toolbar stays inside the viewport
 *    with at least `MIN_EDGE_PAD` of breathing room.
 *
 * Exported so the unit tests can lock down the math.
 */
export function computeToolbarPosition(
  selectionBounds: { x: number; y: number; w: number; h: number },
  viewport: ViewportRect,
  toolbarSize: { w: number; h: number },
): { left: number; top: number } {
  const screenMidX =
    viewport.originX +
    (selectionBounds.x + selectionBounds.w / 2 - viewport.panX) * viewport.zoom;
  const screenTopY =
    viewport.originY + (selectionBounds.y - viewport.panY) * viewport.zoom;
  const screenBottomY =
    viewport.originY +
    (selectionBounds.y + selectionBounds.h - viewport.panY) * viewport.zoom;

  let top = screenTopY - toolbarSize.h - GAP_ABOVE_SELECTION;
  // If we'd clip off the top of the viewport, flip below.
  if (top < viewport.originY + MIN_EDGE_PAD) {
    top = screenBottomY + GAP_ABOVE_SELECTION;
  }
  // Clamp vertically so we never escape the viewport.
  const maxTop = viewport.originY + viewport.height - toolbarSize.h - MIN_EDGE_PAD;
  if (top > maxTop) top = maxTop;
  if (top < viewport.originY + MIN_EDGE_PAD) {
    top = viewport.originY + MIN_EDGE_PAD;
  }

  let left = screenMidX - toolbarSize.w / 2;
  const minLeft = viewport.originX + MIN_EDGE_PAD;
  const maxLeft = viewport.originX + viewport.width - toolbarSize.w - MIN_EDGE_PAD;
  if (left < minLeft) left = minLeft;
  if (left > maxLeft) left = maxLeft;

  return { left, top };
}

/**
 * Read the bounding rectangle of `nodes` in canvas space. Returns
 * `null` for an empty selection.
 */
export function selectionBoundsOf(
  nodes: NodeInfo[],
): { x: number; y: number; w: number; h: number } | null {
  if (nodes.length === 0) return null;
  let minX = Infinity;
  let minY = Infinity;
  let maxX = -Infinity;
  let maxY = -Infinity;
  for (const n of nodes) {
    const { x, y, width: w, height: h } = n.bounds;
    if (x < minX) minX = x;
    if (y < minY) minY = y;
    if (x + w > maxX) maxX = x + w;
    if (y + h > maxY) maxY = y + h;
  }
  if (!Number.isFinite(minX) || !Number.isFinite(minY)) return null;
  return { x: minX, y: minY, w: maxX - minX, h: maxY - minY };
}

export function FloatingToolbar({
  selection,
  viewport,
  actions,
  disabled,
  onDismiss,
}: FloatingToolbarProps): JSX.Element | null {
  const ref = useRef<HTMLDivElement | null>(null);
  const [measured, setMeasured] = useState<{ w: number; h: number } | null>(
    null,
  );

  useLayoutEffect(() => {
    if (!ref.current) return;
    const r = ref.current.getBoundingClientRect();
    if (r.width > 0 && r.height > 0) {
      setMeasured({ w: r.width, h: r.height });
    }
  }, [actions, selection]);

  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") onDismiss();
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onDismiss]);

  const bounds = useMemo(() => selectionBoundsOf(selection), [selection]);

  if (disabled || !bounds || selection.length === 0 || actions.length === 0) {
    return null;
  }

  const size = measured ?? { w: TOOLBAR_W_HINT, h: TOOLBAR_H_HINT };
  const { left, top } = computeToolbarPosition(bounds, viewport, size);

  return (
    <div
      ref={ref}
      role="toolbar"
      aria-label="Selection actions"
      style={{
        position: "fixed",
        left,
        top,
        display: "flex",
        gap: spacing.xs,
        padding: spacing.xs,
        background: colors.bg,
        border: `1px solid ${colors.border}`,
        borderRadius: radius.md,
        boxShadow: "0 8px 24px rgba(0,0,0,0.25)",
        zIndex: 950,
      }}
    >
      {actions.map((a) => (
        <button
          key={a.id}
          type="button"
          title={a.hint ?? a.label}
          aria-label={a.hint ?? a.label}
          onClick={a.onClick}
          style={{
            padding: `${spacing.xs}px ${spacing.sm}px`,
            background: a.primary ? colors.accent : "transparent",
            color: a.primary ? colors.textInverse : colors.text,
            border: a.primary ? "none" : `1px solid ${colors.border}`,
            borderRadius: radius.sm,
            cursor: "pointer",
            fontSize: 12,
            display: "inline-flex",
            alignItems: "center",
            gap: 4,
          }}
        >
          {a.icon ? <span aria-hidden>{a.icon}</span> : null}
          <span>{a.label}</span>
        </button>
      ))}
    </div>
  );
}
