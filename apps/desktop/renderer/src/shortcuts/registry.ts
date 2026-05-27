// Phase 6 Task 21-22: a configurable, user-overridable keyboard
// shortcut registry. The registry is the single source of truth for
// the editor's keystroke → action mapping; both the global listener
// in EditorPage and the KeyboardShortcutsPanel read and mutate the
// same store.
//
// Design contract:
//
//   * `ActionId` is a fixed, exhaustive enum of every binding-able
//     action the editor exposes. Adding a new shortcut means adding
//     an entry here AND in DEFAULT_BINDINGS so the panel can render
//     it.
//   * A `ShortcutBinding` is a single keystroke (key + modifiers).
//     Each action maps to *at most one* binding to keep the UI
//     unambiguous; users who need a second binding for the same
//     action can pick one of the OS-standard alternatives we list
//     in defaults (e.g. Ctrl+Y for redo is exposed as a separate
//     `redoAlt` action that delegates to the same handler).
//   * Bindings persist to `localStorage` under
//     `kcreate.shortcuts.v1` as a JSON map of `ActionId -> Binding`.
//     The version suffix lets us migrate the schema later without
//     breaking older clients.
//   * The store is a tiny pub/sub — the panel subscribes to render
//     edits live, and the editor's `useShortcuts` hook subscribes
//     so a rebind takes effect immediately without a page reload.

/// Every action the editor can bind to a keystroke. Add a new
/// entry here AND in `DEFAULT_BINDINGS` to expose a new shortcut.
export type ActionId =
  | "undo"
  | "redo"
  | "redoAlt"
  | "selectAll"
  | "deleteSelection"
  | "clearSelection"
  | "toolSelect"
  | "toolRect"
  | "toolEllipse"
  | "toolLine"
  | "toolText"
  | "togglePan"
  | "openExport"
  | "openShortcutsPanel"
  | "copy"
  | "paste";

/// A keystroke. `key` follows the `KeyboardEvent.key` convention
/// (case-insensitive a–z, named keys like "Escape", "Delete",
/// "Backspace", " " for Space). Modifiers are tri-state: `true`
/// requires the modifier, `false` requires it to be released,
/// `undefined` doesn't care.
export interface ShortcutBinding {
  /// `KeyboardEvent.key`, normalised to lower-case for letters.
  readonly key: string;
  /// Cmd on macOS, Ctrl elsewhere. Most editing shortcuts want
  /// this set so they don't collide with raw letter keys.
  readonly mod: boolean;
  readonly shift: boolean;
  readonly alt: boolean;
}

/// Metadata for the panel.
export interface ActionMeta {
  /// Short label rendered in the panel ("Undo", "Tool: Rectangle").
  readonly label: string;
  /// Group header in the panel ("Editing", "Tools", "View").
  readonly category: ShortcutCategory;
  /// One-line description shown next to the binding.
  readonly description: string;
}

export type ShortcutCategory = "editing" | "tools" | "view" | "panels";

/// The shipped defaults. Mirrors the previous hard-coded handler in
/// `EditorPage.tsx`; users can override any of these via the panel.
/// Note "Cmd+Z / Ctrl+Z" is encoded as `mod: true` because the
/// runtime collapses metaKey | ctrlKey to a single bit.
export const DEFAULT_BINDINGS: Record<ActionId, ShortcutBinding> = {
  undo: { key: "z", mod: true, shift: false, alt: false },
  redo: { key: "z", mod: true, shift: true, alt: false },
  redoAlt: { key: "y", mod: true, shift: false, alt: false },
  selectAll: { key: "a", mod: true, shift: false, alt: false },
  deleteSelection: { key: "Delete", mod: false, shift: false, alt: false },
  clearSelection: { key: "Escape", mod: false, shift: false, alt: false },
  toolSelect: { key: "v", mod: false, shift: false, alt: false },
  toolRect: { key: "r", mod: false, shift: false, alt: false },
  toolEllipse: { key: "e", mod: false, shift: false, alt: false },
  toolLine: { key: "l", mod: false, shift: false, alt: false },
  toolText: { key: "t", mod: false, shift: false, alt: false },
  // Space-bar pan: held to temporarily switch to pan mode (a
  // gesture, not a one-shot action). The handler in `useShortcuts`
  // dispatches both keydown and keyup for this action.
  togglePan: { key: " ", mod: false, shift: false, alt: false },
  openExport: { key: "e", mod: true, shift: false, alt: false },
  openShortcutsPanel: { key: "/", mod: true, shift: false, alt: false },
  copy: { key: "c", mod: true, shift: false, alt: false },
  paste: { key: "v", mod: true, shift: false, alt: false },
};

export const ACTION_META: Record<ActionId, ActionMeta> = {
  undo: {
    label: "Undo",
    category: "editing",
    description: "Step backward through the operation log.",
  },
  redo: {
    label: "Redo",
    category: "editing",
    description: "Re-apply the last undone operation.",
  },
  redoAlt: {
    label: "Redo (alt)",
    category: "editing",
    description: "Windows-style redo binding; runs the same handler as Redo.",
  },
  selectAll: {
    label: "Select all",
    category: "editing",
    description: "Select every node on the active artboard.",
  },
  deleteSelection: {
    label: "Delete selection",
    category: "editing",
    description: "Remove the selected nodes.",
  },
  clearSelection: {
    label: "Clear selection",
    category: "editing",
    description: "Drop the current selection without modifying it.",
  },
  toolSelect: {
    label: "Tool: Select",
    category: "tools",
    description: "Switch to the selection tool.",
  },
  toolRect: {
    label: "Tool: Rectangle",
    category: "tools",
    description: "Switch to the rectangle tool.",
  },
  toolEllipse: {
    label: "Tool: Ellipse",
    category: "tools",
    description: "Switch to the ellipse tool.",
  },
  toolLine: {
    label: "Tool: Line",
    category: "tools",
    description: "Switch to the line tool.",
  },
  toolText: {
    label: "Tool: Text",
    category: "tools",
    description: "Switch to the text tool.",
  },
  togglePan: {
    label: "Pan (hold)",
    category: "view",
    description: "Hold the bound key to temporarily switch to pan mode.",
  },
  openExport: {
    label: "Open Export",
    category: "panels",
    description: "Open the Export panel and dialog.",
  },
  openShortcutsPanel: {
    label: "Open Shortcuts Panel",
    category: "panels",
    description: "Open the keyboard shortcuts panel.",
  },
  copy: {
    label: "Copy",
    category: "editing",
    description:
      "Serialise the current selection to the OS clipboard as a KCreate payload.",
  },
  paste: {
    label: "Paste",
    category: "editing",
    description:
      "Insert the clipboard payload under the active artboard, offset to avoid overlap.",
  },
};

const STORAGE_KEY = "kcreate.shortcuts.v1";

type Listener = (
  bindings: Readonly<Record<ActionId, ShortcutBinding>>,
) => void;

/// Singleton in-process binding store. Loaded from localStorage on
/// first access; mutations write through synchronously so a reload
/// preserves the user's bindings.
///
/// React-store contract: `snapshot()` is consumed by
/// `useSyncExternalStore`, which calls it on every render and
/// compares the returned value against the previous snapshot via
/// `Object.is`. Any new reference makes React think the store
/// changed, which schedules another render, which calls
/// `snapshot()` again, which… loops. The store therefore holds a
/// single frozen `bindings` object and only swaps the reference
/// when an actual mutation happens. Read paths return that frozen
/// reference directly — never a spread copy.
class ShortcutStore {
  private bindings: Readonly<Record<ActionId, ShortcutBinding>>;
  private readonly listeners = new Set<Listener>();

  constructor() {
    // Assemble the merged binding map as a mutable local, then
    // freeze it once and assign — the field itself is immutable
    // from this point onwards, which guarantees that any caller
    // who holds onto a snapshot can't mutate the store from
    // under us.
    const merged: Record<ActionId, ShortcutBinding> = { ...DEFAULT_BINDINGS };
    if (typeof window !== "undefined" && window.localStorage) {
      try {
        const raw = window.localStorage.getItem(STORAGE_KEY);
        if (raw) {
          const parsed = JSON.parse(raw) as Partial<
            Record<ActionId, ShortcutBinding>
          >;
          // Validate each entry — drop any whose key isn't a
          // non-empty string (a corrupted localStorage shouldn't
          // brick the editor).
          for (const id of Object.keys(parsed) as ActionId[]) {
            const b = parsed[id];
            if (
              b &&
              typeof b.key === "string" &&
              b.key.length > 0 &&
              typeof b.mod === "boolean" &&
              typeof b.shift === "boolean" &&
              typeof b.alt === "boolean" &&
              id in DEFAULT_BINDINGS
            ) {
              merged[id] = b;
            }
          }
        }
      } catch {
        // Ignore: a malformed entry falls back to defaults.
      }
    }
    this.bindings = Object.freeze(merged);
  }

  /// Stable, frozen snapshot of the current bindings. The reference
  /// only changes when a mutation (`set` / `resetOne` / `resetAll`)
  /// actually rewrites the store, so `useSyncExternalStore` can
  /// compare snapshots with `Object.is` without thrashing. The
  /// returned object is frozen; callers who want a mutable copy
  /// must spread on their side.
  snapshot(): Readonly<Record<ActionId, ShortcutBinding>> {
    return this.bindings;
  }

  /// Look up the binding for an action.
  get(id: ActionId): ShortcutBinding {
    return this.bindings[id];
  }

  /// Rebind a single action. Writes through to localStorage and
  /// notifies subscribers synchronously.
  set(id: ActionId, binding: ShortcutBinding): void {
    this.bindings = Object.freeze({ ...this.bindings, [id]: binding });
    this.persist();
    this.fire();
  }

  /// Reset a single action to its shipped default.
  resetOne(id: ActionId): void {
    this.set(id, DEFAULT_BINDINGS[id]);
  }

  /// Reset every action to its shipped default.
  resetAll(): void {
    this.bindings = Object.freeze({ ...DEFAULT_BINDINGS });
    this.persist();
    this.fire();
  }

  /// Subscribe to changes. Returns the unsubscribe function.
  subscribe(listener: Listener): () => void {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  }

  private persist(): void {
    if (typeof window === "undefined" || !window.localStorage) return;
    try {
      window.localStorage.setItem(STORAGE_KEY, JSON.stringify(this.bindings));
    } catch {
      // Quota / private-mode failures are non-fatal; the registry
      // continues to work in-memory for the current session.
    }
  }

  private fire(): void {
    const snap = this.snapshot();
    for (const listener of this.listeners) {
      listener(snap);
    }
  }
}

let storeSingleton: ShortcutStore | null = null;

export function shortcutStore(): ShortcutStore {
  if (!storeSingleton) {
    storeSingleton = new ShortcutStore();
  }
  return storeSingleton;
}

/// Test-only reset hook. Drops the singleton so a unit test can
/// observe `localStorage` fresh.
export function resetShortcutStoreForTests(): void {
  storeSingleton = null;
}

/// Match a `KeyboardEvent` against a binding. Letter keys match
/// case-insensitively; named keys ("Escape", "Delete", "ArrowLeft",
/// …) are case-sensitive per the DOM spec.
export function matchesBinding(
  event: KeyboardEvent,
  binding: ShortcutBinding,
): boolean {
  const mod = event.ctrlKey || event.metaKey;
  if (mod !== binding.mod) return false;
  if (event.shiftKey !== binding.shift) return false;
  if (event.altKey !== binding.alt) return false;
  const evKey =
    event.key.length === 1 ? event.key.toLowerCase() : event.key;
  const bKey = binding.key.length === 1 ? binding.key.toLowerCase() : binding.key;
  return evKey === bKey;
}

/// Render a binding into a human-readable label
/// ("⌘+Shift+Z", "Ctrl+A", "Esc"). Picks the OS-appropriate
/// modifier glyph when run inside a browser; the SSR / test
/// fallback uses ASCII so snapshots are stable.
export function formatBinding(binding: ShortcutBinding): string {
  const parts: string[] = [];
  const isMac =
    typeof navigator !== "undefined" &&
    /Mac|iPhone|iPad/.test(navigator.platform);
  if (binding.mod) parts.push(isMac ? "\u2318" : "Ctrl");
  if (binding.alt) parts.push(isMac ? "\u2325" : "Alt");
  if (binding.shift) parts.push(isMac ? "\u21E7" : "Shift");
  const key = binding.key;
  const friendly: Record<string, string> = {
    " ": "Space",
    Escape: "Esc",
    ArrowLeft: "\u2190",
    ArrowRight: "\u2192",
    ArrowUp: "\u2191",
    ArrowDown: "\u2193",
  };
  parts.push(friendly[key] ?? (key.length === 1 ? key.toUpperCase() : key));
  return parts.join(isMac ? "" : "+");
}
