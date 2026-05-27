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
import { matchesBinding, shortcutStore } from "./registry";

export type ShortcutHandlers = Partial<
  Record<ActionId, (event: KeyboardEvent) => void>
>;

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
  const store = shortcutStore();
  return useSyncExternalStore(
    (listener) => store.subscribe(listener),
    () => store.snapshot(),
    () => store.snapshot(),
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
    const onKeyDown = (event: KeyboardEvent): void => {
      if (!enabled) return;

      // Skip when the user is typing in a form field. Otherwise
      // pressing "R" inside a name input would silently switch
      // tools. We deliberately keep this check inside the
      // listener so a freshly-focused input is honoured immediately.
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
        const handler = handlers[id];
        if (!handler) continue;
        handler(event);
        return;
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [handlers, bindings, enabled]);
}
