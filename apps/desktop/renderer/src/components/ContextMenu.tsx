// Phase D — shared context-menu primitive.
//
// Floating menu positioned at a fixed (x, y) viewport coordinate,
// auto-clamped to the viewport so it never escapes the screen edges,
// with Escape + outside-click dismissal. Replaces the bespoke quartet
// (`ContextMenu` / `MenuItem` / `MenuDivider` / `MenuSubheading`) that
// previously lived inside `PageNavigator.tsx` so the layer tree, the
// page navigator, and any future right-click surface share one
// implementation.
//
// Contract:
//   * The host opens the menu by setting `{ x, y }` state on a
//     right-click handler (e.preventDefault() + e.clientX/clientY).
//   * The host renders <ContextMenu x y onDismiss>{items}</ContextMenu>.
//   * The menu calls `onDismiss()` on Escape, outside-click, or
//     after a menu item runs (the item itself decides whether to
//     dismiss — most do via the convenience wrapper).
//   * Edge clamping runs in `useLayoutEffect` after the first layout
//     so we measure the actual rendered size; menus with many items
//     don't escape the bottom of the screen.

import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";

import { colors, radius, spacing } from "../styles/tokens";

export interface ContextMenuProps {
  /** Viewport-relative x coordinate (e.clientX from the right-click). */
  x: number;
  /** Viewport-relative y coordinate (e.clientY from the right-click). */
  y: number;
  /** Fires on Escape, outside-click, and item activation (when the
   * item opts in via `dismissOnSelect`). */
  onDismiss: () => void;
  /** Menu contents — typically a mix of `<MenuItem>`, `<MenuDivider>`,
   * `<MenuSubheading>`. Any ReactNode is allowed so callers can
   * embed custom controls (e.g. a color-tag swatch row). */
  children: ReactNode;
  /** Minimum width hint in px; the menu may grow beyond this if the
   * widest item is wider. Defaults to 200, matching the PageNavigator
   * footprint the primitive replaced. */
  minWidth?: number;
  /** Optional accessible label, surfaced via `aria-label` on the
   * `role="menu"` container. Helps screen readers distinguish two
   * concurrently-mounted menus (rare, but the cost is zero). */
  ariaLabel?: string;
}

const EDGE_PAD = 8;

/**
 * Floating menu with edge clamping + Escape + outside-click dismissal.
 *
 * Renders as a fixed-position div at `(x, y)` with the (right, bottom)
 * edges clamped to `viewport - EDGE_PAD` after a one-frame
 * `useLayoutEffect` measurement. If the menu doesn't fit below the
 * cursor we flip it above; if it doesn't fit to the right we flip
 * it to the left. Both flip decisions are made independently so a
 * corner-of-the-screen right-click still produces a fully-visible
 * menu.
 */
export function ContextMenu({
  x,
  y,
  onDismiss,
  children,
  minWidth = 200,
  ariaLabel,
}: ContextMenuProps): JSX.Element {
  const ref = useRef<HTMLDivElement | null>(null);
  // Initial position is the cursor; useLayoutEffect overrides it
  // after measuring the rendered size.
  const [pos, setPos] = useState({ left: x, top: y });

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const rect = el.getBoundingClientRect();
    const vw =
      typeof window !== "undefined" ? window.innerWidth : rect.right + EDGE_PAD;
    const vh =
      typeof window !== "undefined"
        ? window.innerHeight
        : rect.bottom + EDGE_PAD;

    let left = x;
    let top = y;
    // Flip horizontally if right edge would overflow. Don't simply
    // clamp — a clamped menu would cover the cursor, hiding the
    // affordance the user just right-clicked. Flipping mirrors the
    // OS context-menu convention near the screen's right edge.
    if (left + rect.width + EDGE_PAD > vw) {
      left = Math.max(EDGE_PAD, x - rect.width);
    }
    // Same logic vertically.
    if (top + rect.height + EDGE_PAD > vh) {
      top = Math.max(EDGE_PAD, y - rect.height);
    }
    // Final hard clamps for the (rare) case where the menu is larger
    // than the viewport in either axis. Without these the menu would
    // be partially scrolled off, which is worse than a flush-edge
    // overflow that at least keeps the top items visible.
    left = Math.max(EDGE_PAD, Math.min(left, vw - rect.width - EDGE_PAD));
    top = Math.max(EDGE_PAD, Math.min(top, vh - rect.height - EDGE_PAD));

    // Avoid a setState when the cursor position already happens to
    // be a fully-inside placement; otherwise we'd cause a redundant
    // render pass + a `useLayoutEffect` loop in React's strict mode.
    if (left !== pos.left || top !== pos.top) {
      setPos({ left, top });
    }
    // We deliberately re-run only when (x, y) change. `pos` is read
    // for the equality short-circuit above; including it in deps
    // would trigger an infinite loop because the setState mutates it.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [x, y]);

  // Escape closes the menu. Captured at document level so a focused
  // input inside the menu (e.g. a future rename field) doesn't
  // swallow the key.
  useEffect(() => {
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === "Escape") {
        e.stopPropagation();
        onDismiss();
      }
    };
    document.addEventListener("keydown", onKey, true);
    return () => document.removeEventListener("keydown", onKey, true);
  }, [onDismiss]);

  // Outside-click also dismisses. We listen on `mousedown` (not
  // `click`) because a click that originates *inside* the menu and
  // releases *outside* of it (a drag, or a fast double-click) would
  // otherwise dismiss. Capturing on mousedown matches the OS
  // contextmenu convention and avoids that race.
  useEffect(() => {
    const onDown = (e: MouseEvent): void => {
      const el = ref.current;
      if (!el) return;
      if (e.target instanceof Node && el.contains(e.target)) return;
      onDismiss();
    };
    document.addEventListener("mousedown", onDown, true);
    return () => document.removeEventListener("mousedown", onDown, true);
  }, [onDismiss]);

  return (
    <div
      ref={ref}
      role="menu"
      aria-label={ariaLabel}
      // Stop click + contextmenu propagation so a right-click *on*
      // the menu doesn't bubble up to whatever surface opened it
      // (which would close + reopen the menu at the new cursor).
      onClick={(e) => e.stopPropagation()}
      onContextMenu={(e) => {
        e.preventDefault();
        e.stopPropagation();
      }}
      style={{
        position: "fixed",
        left: pos.left,
        top: pos.top,
        background: colors.bg,
        border: `1px solid ${colors.border}`,
        borderRadius: radius.card,
        boxShadow: "0 4px 12px rgba(0,0,0,0.08)",
        padding: spacing.xs,
        minWidth,
        zIndex: 1000,
        fontSize: 12,
        // Defensive: a menu that overflowed even after clamping
        // (e.g. taller than the viewport) should scroll, not cover
        // the whole screen.
        maxHeight: "calc(100vh - 16px)",
        overflowY: "auto",
      }}
    >
      {children}
    </div>
  );
}

export interface MenuItemProps {
  label: string;
  onClick: () => void;
  /** Renders the label in the destructive red used elsewhere in
   * the app (used for Delete-style items). Defaults to false. */
  danger?: boolean;
  /** When true, the row is rendered greyed-out and the click handler
   * is suppressed. Defaults to false. Useful for items whose
   * action isn't available given the current selection
   * (e.g. "Distribute" when fewer than 3 items are selected). */
  disabled?: boolean;
  /** Optional shortcut hint rendered right-aligned (e.g. "⌘C", "Del").
   * Purely visual — the actual keystroke is dispatched by the
   * shortcut registry, not by the menu. */
  shortcut?: string;
  /** Optional test-id surfaced for vitest selectors. */
  "data-testid"?: string;
}

/** A single clickable row inside a `<ContextMenu>`. */
export function MenuItem({
  label,
  onClick,
  danger,
  disabled,
  shortcut,
  "data-testid": testId,
}: MenuItemProps): JSX.Element {
  return (
    <button
      type="button"
      role="menuitem"
      disabled={disabled}
      onClick={() => {
        if (disabled) return;
        onClick();
      }}
      data-testid={testId}
      style={{
        display: "flex",
        alignItems: "center",
        justifyContent: "space-between",
        gap: spacing.sm,
        width: "100%",
        textAlign: "left",
        padding: "6px 10px",
        background: "transparent",
        border: "none",
        cursor: disabled ? "not-allowed" : "pointer",
        color: disabled
          ? colors.textMuted
          : danger
            ? "#B91C1C"
            : colors.text,
        opacity: disabled ? 0.6 : 1,
        borderRadius: 6,
        fontSize: 12,
      }}
    >
      <span>{label}</span>
      {shortcut !== undefined ? (
        <span
          style={{
            color: colors.textMuted,
            fontSize: 10,
            fontFamily:
              "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
          }}
        >
          {shortcut}
        </span>
      ) : null}
    </button>
  );
}

/** A 1px divider between menu sections. */
export function MenuDivider(): JSX.Element {
  return (
    <div
      role="separator"
      style={{
        height: 1,
        background: colors.border,
        margin: `${spacing.xs}px 0`,
      }}
    />
  );
}

export interface MenuSubheadingProps {
  label: string;
}

/** A non-interactive group heading (e.g. "Apply master", "Layer color"). */
export function MenuSubheading({ label }: MenuSubheadingProps): JSX.Element {
  return (
    <div
      role="presentation"
      style={{
        padding: "2px 10px",
        fontSize: 10,
        fontWeight: 600,
        color: colors.textMuted,
        textTransform: "uppercase",
        letterSpacing: 0.5,
      }}
    >
      {label}
    </div>
  );
}

/**
 * Small convenience hook for the common "open at cursor / close /
 * is-open" state pattern. Callers can use this OR manage their own
 * state — the primitive doesn't require it.
 *
 * Usage:
 *   const menu = useContextMenuState<MyMenuPayload>();
 *   <Row onContextMenu={(e) => { e.preventDefault(); menu.open(e, row); }} />
 *   {menu.state ? (
 *     <ContextMenu x={menu.state.x} y={menu.state.y} onDismiss={menu.close}>
 *       <MenuItem label="…" onClick={() => { …; menu.close(); }} />
 *     </ContextMenu>
 *   ) : null}
 */
export function useContextMenuState<TPayload>(): {
  state: { x: number; y: number; payload: TPayload } | null;
  open: (e: React.MouseEvent, payload: TPayload) => void;
  close: () => void;
} {
  const [state, setState] = useState<
    { x: number; y: number; payload: TPayload } | null
  >(null);
  const open = useCallback((e: React.MouseEvent, payload: TPayload): void => {
    e.preventDefault();
    e.stopPropagation();
    setState({ x: e.clientX, y: e.clientY, payload });
  }, []);
  const close = useCallback((): void => setState(null), []);
  return { state, open, close };
}
