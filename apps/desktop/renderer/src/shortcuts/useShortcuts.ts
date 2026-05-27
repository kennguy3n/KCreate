// Phase 6 Task 21-22: a React hook that wires the keyboard-shortcut
// registry into a window-level keydown listener. The caller supplies
// a handler for each `ActionId`; the hook resolves the current
// binding from the registry and dispatches the matching handler
// when the keystroke fires.
//
// The hook intentionally re-subscribes whenever the store's
// bindings change, so a rebind in the panel takes effect
// immediately without a remount.

import { useEffect, useSyncExternalStore } from "react";

import type { ActionId, ShortcutBinding } from "./registry";
import {
  matchesBinding,
  shortcutGetSnapshot,
  shortcutSubscribe,
} from "./registry";

/// One bound handler. Most actions are one-shots — a single
/// function fired on keydown. Hold-style actions (currently just
/// `togglePan`) need to observe key release as well, so they pass
/// an object whose `onKeyDown` / `onKeyUp` are dispatched on the
/// matching DOM event. Either field may be omitted; an object
/// with only `onKeyUp` is valid (rare, but useful for actions that
/// arm on press elsewhere and disarm on release).
export type ShortcutHandler =
  | ((event: KeyboardEvent) => void)
  | {
      readonly onKeyDown?: (event: KeyboardEvent) => void;
      readonly onKeyUp?: (event: KeyboardEvent) => void;
    };

export type ShortcutHandlers = Partial<Record<ActionId, ShortcutHandler>>;

function resolveHandler(
  handler: ShortcutHandler | undefined,
  phase: "down" | "up",
): ((event: KeyboardEvent) => void) | null {
  if (!handler) return null;
  if (typeof handler === "function") {
    // Bare functions are keydown-only — they exist to preserve the
    // pre-hold-style handler shape every action used before
    // togglePan was wired. Returning `null` on keyup keeps the
    // listener short-circuit cheap.
    return phase === "down" ? handler : null;
  }
  return phase === "down" ? handler.onKeyDown ?? null : handler.onKeyUp ?? null;
}

/// Subscribe to the live snapshot of bindings. Use this in the
/// shortcut panel and anywhere you need to render bindings live.
///
/// The returned object is the store's frozen snapshot — same
/// reference across renders unless a real mutation happened. This
/// is critical for `useSyncExternalStore`'s reference-equality
/// contract (`Object.is` between renders), and it also lets the
/// hook be safely passed to memoised children without invalidating
/// their props on every parent render.
export function useShortcutBindings(): Readonly<
  Record<ActionId, ShortcutBinding>
> {
  // Subscribe + snapshot are module-level stable bindings into the
  // singleton store (see `shortcutSubscribe` / `shortcutGetSnapshot`
  // in registry.ts). Passing them by reference — instead of inline
  // arrow wrappers — keeps `useSyncExternalStore` from detaching
  // and re-attaching the listener on every render. The third
  // argument (server snapshot) is the same getter because the
  // renderer is always client-side.
  return useSyncExternalStore(
    shortcutSubscribe,
    shortcutGetSnapshot,
    shortcutGetSnapshot,
  );
}

/// Bind a handler-map to the window. Only actions present in
/// `handlers` are wired; unhandled actions are no-ops. Returns
/// nothing; the hook owns its listener and detaches on unmount.
///
/// `enabled` is honoured at dispatch time, not at listener-attach
/// time, so toggling it doesn't flap the listener: the listener
/// stays mounted and simply early-returns while disabled. This
/// matters because attach/detach cycles can race with the user
/// pressing a key during a re-render.
export function useShortcuts(
  handlers: ShortcutHandlers,
  enabled: boolean = true,
): void {
  const bindings = useShortcutBindings();

  useEffect(() => {
    const dispatch = (event: KeyboardEvent, phase: "down" | "up"): void => {
      if (!enabled) return;

      // Skip when the user is typing in a form field. Otherwise
      // pressing "R" inside a name input would silently switch
      // tools. We deliberately keep this check inside the
      // listener so a freshly-focused input is honoured immediately.
      //
      // The form-field gate must apply to both phases — if it
      // didn't, releasing Space inside a text input would still
      // fire `togglePan.onKeyUp` and disarm the gesture even though
      // the matching keydown was skipped. Keeping it symmetric
      // preserves the "shortcuts never interfere with typing"
      // contract on both sides of the gesture.
      const target = event.target as HTMLElement | null;
      const tag = target?.tagName?.toLowerCase();
      const isEditable =
        tag === "input" ||
        tag === "textarea" ||
        tag === "select" ||
        (target?.isContentEditable ?? false);
      if (isEditable) return;

      // Walk the action list once. The set is small (~15 entries)
      // so a linear scan is fine; we don't index by keystroke
      // because modifier-mask collisions would force a more
      // complex map structure for negligible gain.
      const ids = Object.keys(handlers) as ActionId[];
      for (const id of ids) {
        const binding = bindings[id];
        if (!binding) continue;
        if (!matchesBinding(event, binding)) continue;
        const fn = resolveHandler(handlers[id], phase);
        if (!fn) continue;
        fn(event);
        return;
      }
    };
    const onKeyDown = (event: KeyboardEvent): void => dispatch(event, "down");
    const onKeyUp = (event: KeyboardEvent): void => dispatch(event, "up");
    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("keyup", onKeyUp);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("keyup", onKeyUp);
    };
  }, [handlers, bindings, enabled]);
}
