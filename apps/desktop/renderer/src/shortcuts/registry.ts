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
  | "deleteSelectionAlt"
  | "clearSelection"
  | "toolSelect"
  | "toolRect"
  | "toolEllipse"
  | "toolLine"
  | "toolPen"
  | "toolText"
  | "commitPath"
  | "togglePan"
  | "openExport"
  | "openShortcutsPanel"
  | "copy"
  | "paste"
  | "alignLeft"
  | "alignCenterX"
  | "alignRight"
  | "alignTop"
  | "alignCenterY"
  | "alignBottom"
  | "distributeHorizontal"
  | "distributeVertical";

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

export type ShortcutCategory = "editing" | "tools" | "view" | "panels" | "alignment";

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
  // macOS regression fix: Apple keyboards send `Backspace` from the
  // physical "delete" key, so a registry that bound only `Delete`
  // silently broke deletion on every Mac. Same handler as
  // `deleteSelection`, separate action so the panel can show /
  // rebind it independently and the contract "one action = one
  // binding" stays intact.
  deleteSelectionAlt: {
    key: "Backspace",
    mod: false,
    shift: false,
    alt: false,
  },
  clearSelection: { key: "Escape", mod: false, shift: false, alt: false },
  toolSelect: { key: "v", mod: false, shift: false, alt: false },
  toolRect: { key: "r", mod: false, shift: false, alt: false },
  toolEllipse: { key: "e", mod: false, shift: false, alt: false },
  toolLine: { key: "l", mod: false, shift: false, alt: false },
  toolPen: { key: "p", mod: false, shift: false, alt: false },
  toolText: { key: "t", mod: false, shift: false, alt: false },
  // Phase B1 — Pen tool gesture commit. Bound to Enter so a path-in-
  // flight (>= 2 anchors) can be promoted to a real `VectorLayer`
  // without forcing the user to switch tools or click into empty
  // space. The handler is a no-op when the pen state machine is
  // idle, so this binding cannot interfere with non-pen workflows.
  commitPath: { key: "Enter", mod: false, shift: false, alt: false },
  // Space-bar pan: held to temporarily switch to pan mode (a
  // gesture, not a one-shot action). The handler in `useShortcuts`
  // dispatches both keydown and keyup for this action.
  togglePan: { key: " ", mod: false, shift: false, alt: false },
  openExport: { key: "e", mod: true, shift: false, alt: false },
  openShortcutsPanel: { key: "/", mod: true, shift: false, alt: false },
  copy: { key: "c", mod: true, shift: false, alt: false },
  paste: { key: "v", mod: true, shift: false, alt: false },
  // Phase D — Alignment shortcuts. Figma-style: Alt+letter for align,
  // Ctrl+Alt+letter for distribute. Only active when ≥2 nodes selected
  // (handler checks at dispatch time, same as AlignmentToolbar).
  alignLeft: { key: "a", mod: false, shift: false, alt: true },
  alignCenterX: { key: "h", mod: false, shift: false, alt: true },
  alignRight: { key: "d", mod: false, shift: false, alt: true },
  alignTop: { key: "w", mod: false, shift: false, alt: true },
  alignCenterY: { key: "v", mod: false, shift: false, alt: true },
  alignBottom: { key: "s", mod: false, shift: false, alt: true },
  distributeHorizontal: { key: "h", mod: true, shift: false, alt: true },
  distributeVertical: { key: "v", mod: true, shift: false, alt: true },
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
  deleteSelectionAlt: {
    label: "Delete selection (Backspace)",
    category: "editing",
    description:
      "macOS-friendly alternative to Delete; runs the same handler as Delete selection.",
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
  toolPen: {
    label: "Tool: Pen",
    category: "tools",
    description:
      "Switch to the pen tool (multi-click to add anchors; click+drag for smooth curves; Enter to commit; Esc to cancel).",
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
  commitPath: {
    label: "Commit path",
    category: "editing",
    description:
      "Commit the in-flight pen path as a `VectorLayer`. No-op when the pen tool has no anchors recorded.",
  },
  alignLeft: {
    label: "Align left",
    category: "alignment",
    description: "Align selected nodes to the left edge of the group bounding box.",
  },
  alignCenterX: {
    label: "Align center (X)",
    category: "alignment",
    description: "Align selected nodes to the horizontal center of the group bounding box.",
  },
  alignRight: {
    label: "Align right",
    category: "alignment",
    description: "Align selected nodes to the right edge of the group bounding box.",
  },
  alignTop: {
    label: "Align top",
    category: "alignment",
    description: "Align selected nodes to the top edge of the group bounding box.",
  },
  alignCenterY: {
    label: "Align middle (Y)",
    category: "alignment",
    description: "Align selected nodes to the vertical center of the group bounding box.",
  },
  alignBottom: {
    label: "Align bottom",
    category: "alignment",
    description: "Align selected nodes to the bottom edge of the group bounding box.",
  },
  distributeHorizontal: {
    label: "Distribute horizontal",
    category: "alignment",
    description: "Evenly space selected nodes horizontally. Requires 3+ selected.",
  },
  distributeVertical: {
    label: "Distribute vertical",
    category: "alignment",
    description: "Evenly space selected nodes vertically. Requires 3+ selected.",
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

  /// Find every action whose current binding collides with the
  /// supplied keystroke. Returns the list **excluding** `exceptId`
  /// (so a row can ask "who else has my binding?" without listing
  /// itself). Used by the KeyboardShortcutsPanel to surface a
  /// collision warning inline.
  ///
  /// Why this exists: `useShortcuts` walks the handler map and
  /// fires the first action whose binding matches. If two actions
  /// share a binding, the second is unreachable. We deliberately
  /// don't *reject* the rebind in `set()` — the user may want to
  /// swap two actions through a transient collision, or rebind the
  /// loser later. The collision is therefore informational, not
  /// blocking; the panel renders a warning and lists the
  /// conflicting actions so the user can fix it on their own
  /// schedule.
  findConflicts(
    binding: ShortcutBinding,
    exceptId?: ActionId,
  ): ActionId[] {
    const out: ActionId[] = [];
    for (const id of Object.keys(this.bindings) as ActionId[]) {
      if (id === exceptId) continue;
      if (bindingsEqual(this.bindings[id], binding)) {
        out.push(id);
      }
    }
    return out;
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

/// Stable bound references to the singleton's subscribe + snapshot.
/// `useSyncExternalStore` requires its `subscribe` callback to be
/// reference-stable across renders — otherwise React will detach +
/// re-attach the listener on every render of the calling component,
/// which is wasted work and races with concurrent rebinds in the
/// shortcut panel. We bind once at module load and reuse forever;
/// `resetShortcutStoreForTests()` rebinds because the singleton
/// itself is swapped out.
export function shortcutSubscribe(listener: Listener): () => void {
  return shortcutStore().subscribe(listener);
}

export function shortcutGetSnapshot(): Readonly<
  Record<ActionId, ShortcutBinding>
> {
  return shortcutStore().snapshot();
}

/// Test-only reset hook. Drops the singleton so a unit test can
/// observe `localStorage` fresh.
export function resetShortcutStoreForTests(): void {
  storeSingleton = null;
}

/// Normalise a binding key for equality comparison. Single-character
/// keys are folded to lower-case (so "A" === "a"), multi-character
/// named keys ("Escape", "ArrowLeft") are case-sensitive because the
/// DOM spec itself is. Keep this aligned with the equivalent fold in
/// `matchesBinding` — they must agree, otherwise the panel could
/// claim "no conflict" for a binding the dispatcher *does* match.
function normaliseBindingKey(key: string): string {
  return key.length === 1 ? key.toLowerCase() : key;
}

/// Derive the binding-key string from a `KeyboardEvent`, transparently
/// handling the macOS Option dead-key behaviour.
///
/// On macOS, pressing Option+letter (without Cmd) transforms
/// `event.key` into the typed Unicode character (Option+V → "√",
/// Option+A → "å", etc.). This breaks naive `event.key`-based
/// matching for any Alt-only binding because the bound `key` is the
/// pre-transformation letter ("v", "a", …) but the event delivers
/// the post-transformation glyph. Cmd+Option+letter is unaffected
/// because Chromium prioritises Cmd for character generation, so
/// `event.key` stays as the original letter.
///
/// `event.code` is the *physical* key identifier and is NOT subject
/// to this transformation — "KeyV" stays "KeyV" regardless of
/// modifiers, locale, or input method. We use it as the source of
/// truth whenever `altKey` is true and the code is one of the
/// alphabetic `KeyA`–`KeyZ` codes. We deliberately do NOT use
/// `event.code` unconditionally because (a) for non-alpha keys it
/// returns codes like "Slash" / "Digit1" that differ from the
/// `event.key` strings ("/", "1") existing bindings rely on, and
/// (b) on Dvorak / Colemak layouts `event.code` reflects the
/// QWERTY position, which would break users who remap their layout.
/// For Alt-only letter bindings the QWERTY-position behaviour is
/// actually the desired one — Figma, Sketch, and every other tool
/// in the space binds Option+letter to the physical key position
/// rather than the locale-specific glyph.
export function eventKeyForMatching(event: KeyboardEvent): string {
  if (event.altKey) {
    const code = event.code;
    if (code.length === 4 && code.startsWith("Key")) {
      // "KeyV" → "v". Always lower-case so it aligns with
      // `normaliseBindingKey` without an extra fold.
      return code.charAt(3).toLowerCase();
    }
  }
  return event.key;
}

/// Two bindings are equal iff a single `KeyboardEvent` would match
/// both. This is the contract `findConflicts` uses to surface
/// collisions in the panel; it must agree with `matchesBinding`.
export function bindingsEqual(
  a: ShortcutBinding,
  b: ShortcutBinding,
): boolean {
  return (
    a.mod === b.mod &&
    a.shift === b.shift &&
    a.alt === b.alt &&
    normaliseBindingKey(a.key) === normaliseBindingKey(b.key)
  );
}

/// Match a `KeyboardEvent` against a binding. Letter keys match
/// case-insensitively; named keys ("Escape", "Delete", "ArrowLeft",
/// …) are case-sensitive per the DOM spec.
///
/// Letter resolution goes through `eventKeyForMatching` so Alt-only
/// bindings work on macOS, where the Option key would otherwise
/// transform `event.key` into a glyph that never matches the bound
/// letter (see `eventKeyForMatching` doc-comment for the full
/// rationale).
export function matchesBinding(
  event: KeyboardEvent,
  binding: ShortcutBinding,
): boolean {
  const mod = event.ctrlKey || event.metaKey;
  if (mod !== binding.mod) return false;
  if (event.shiftKey !== binding.shift) return false;
  if (event.altKey !== binding.alt) return false;
  return (
    normaliseBindingKey(eventKeyForMatching(event)) ===
    normaliseBindingKey(binding.key)
  );
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
