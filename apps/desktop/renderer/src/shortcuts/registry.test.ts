// Phase D — alignment shortcut registry regression tests.
//
// Pins the 8 new `align*` / `distribute*` action ids, their default
// bindings, their `alignment` category metadata, and the
// no-collision-with-existing-shortcuts invariant. The collision check
// is the load-bearing one: bindings differ only in their `alt` /
// `mod` modifier states from existing single-letter tool shortcuts
// (e.g. `toolSelect = "v"`, `alignCenterY = Alt+"v"`, `paste = Cmd+"v"`,
// `distributeVertical = Cmd+Alt+"v"`), so a copy-paste regression that
// dropped the `alt: true` flag would silently make Alt+V switch tools
// instead of aligning. `matchesBinding` is the dispatch-side source of
// truth so we exercise it directly.

import { describe, it, expect, beforeEach } from "vitest";

import {
  ACTION_META,
  DEFAULT_BINDINGS,
  bindingsEqual,
  matchesBinding,
  resetShortcutStoreForTests,
  shortcutStore,
  type ActionId,
  type ShortcutBinding,
} from "./registry";

const ALIGNMENT_IDS = [
  "alignLeft",
  "alignCenterX",
  "alignRight",
  "alignTop",
  "alignCenterY",
  "alignBottom",
  "distributeHorizontal",
  "distributeVertical",
] as const satisfies ReadonlyArray<ActionId>;

function makeKeyboardEvent(init: {
  key: string;
  ctrlKey?: boolean;
  metaKey?: boolean;
  altKey?: boolean;
  shiftKey?: boolean;
}): KeyboardEvent {
  return new KeyboardEvent("keydown", {
    key: init.key,
    ctrlKey: init.ctrlKey ?? false,
    metaKey: init.metaKey ?? false,
    altKey: init.altKey ?? false,
    shiftKey: init.shiftKey ?? false,
  });
}

describe("shortcuts registry: Phase D alignment bindings", () => {
  beforeEach(() => {
    resetShortcutStoreForTests();
  });

  it("ships every alignment action with a default binding", () => {
    for (const id of ALIGNMENT_IDS) {
      expect(DEFAULT_BINDINGS[id]).toBeDefined();
    }
  });

  it("classifies every alignment action under the `alignment` category", () => {
    for (const id of ALIGNMENT_IDS) {
      expect(ACTION_META[id].category).toBe("alignment");
    }
  });

  it("uses Figma-style modifier patterns: Alt for align, Ctrl+Alt for distribute", () => {
    const expectations: Record<(typeof ALIGNMENT_IDS)[number], ShortcutBinding> = {
      alignLeft: { key: "a", mod: false, shift: false, alt: true },
      alignCenterX: { key: "h", mod: false, shift: false, alt: true },
      alignRight: { key: "d", mod: false, shift: false, alt: true },
      alignTop: { key: "w", mod: false, shift: false, alt: true },
      alignCenterY: { key: "v", mod: false, shift: false, alt: true },
      alignBottom: { key: "s", mod: false, shift: false, alt: true },
      distributeHorizontal: {
        key: "h",
        mod: true,
        shift: false,
        alt: true,
      },
      distributeVertical: { key: "v", mod: true, shift: false, alt: true },
    };
    for (const id of ALIGNMENT_IDS) {
      expect(DEFAULT_BINDINGS[id]).toEqual(expectations[id]);
    }
  });

  it("does not collide with any non-alignment default binding", () => {
    const store = shortcutStore();
    for (const id of ALIGNMENT_IDS) {
      const conflicts = store.findConflicts(DEFAULT_BINDINGS[id], id);
      // A conflict WITHIN the alignment set itself would be a bug
      // (every alignment slot is distinct); a conflict against an
      // existing non-alignment action would shadow that action.
      expect(conflicts).toEqual([]);
    }
  });

  it("matchesBinding dispatches alignment actions on the documented keystrokes", () => {
    // Sanity: Alt+V matches alignCenterY but NOT toolSelect / paste /
    // distributeVertical, even though all four use the letter "v".
    const altV = makeKeyboardEvent({ key: "v", altKey: true });
    expect(matchesBinding(altV, DEFAULT_BINDINGS.alignCenterY)).toBe(true);
    expect(matchesBinding(altV, DEFAULT_BINDINGS.toolSelect)).toBe(false);
    expect(matchesBinding(altV, DEFAULT_BINDINGS.paste)).toBe(false);
    expect(matchesBinding(altV, DEFAULT_BINDINGS.distributeVertical)).toBe(
      false,
    );

    // Ctrl+Alt+V matches distributeVertical and NOT alignCenterY /
    // paste / toolSelect.
    const ctrlAltV = makeKeyboardEvent({
      key: "v",
      ctrlKey: true,
      altKey: true,
    });
    expect(matchesBinding(ctrlAltV, DEFAULT_BINDINGS.distributeVertical)).toBe(
      true,
    );
    expect(matchesBinding(ctrlAltV, DEFAULT_BINDINGS.alignCenterY)).toBe(false);
    expect(matchesBinding(ctrlAltV, DEFAULT_BINDINGS.paste)).toBe(false);

    // Alt+A → alignLeft (not selectAll which is Cmd+A).
    const altA = makeKeyboardEvent({ key: "a", altKey: true });
    expect(matchesBinding(altA, DEFAULT_BINDINGS.alignLeft)).toBe(true);
    expect(matchesBinding(altA, DEFAULT_BINDINGS.selectAll)).toBe(false);
  });

  it("alignment bindings are pairwise distinct", () => {
    for (let i = 0; i < ALIGNMENT_IDS.length; i++) {
      for (let j = i + 1; j < ALIGNMENT_IDS.length; j++) {
        const idA = ALIGNMENT_IDS[i]!;
        const idB = ALIGNMENT_IDS[j]!;
        const a = DEFAULT_BINDINGS[idA];
        const b = DEFAULT_BINDINGS[idB];
        expect(bindingsEqual(a, b)).toBe(false);
      }
    }
  });
});
