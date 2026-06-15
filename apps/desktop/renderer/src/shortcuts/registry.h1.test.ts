// H1 — discoverability shortcut registry regression tests.
//
// Pins the 6 new `open*` action ids the command palette + entry points
// depend on, their default bindings, their `panels` category metadata,
// and the no-collision invariant against every existing shortcut. The
// collision check is load-bearing: the five capability bindings all
// share the `mod+shift+<letter>` shape, and `openCommandPalette` is a
// bare `mod+k`, so a copy-paste regression that dropped a modifier
// flag would silently shadow an existing action (e.g. mod+k → mod+t).

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

const STORAGE_KEY = "kcreate.shortcuts.v1";

const H1_IDS = [
  "openCommandPalette",
  "openTemplates",
  "openTheme",
  "openElements",
  "openMagicResize",
  "openAiGenerate",
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

describe("shortcuts registry: H1 discoverability bindings", () => {
  beforeEach(() => {
    if (typeof window !== "undefined" && window.localStorage) {
      window.localStorage.removeItem(STORAGE_KEY);
    }
    resetShortcutStoreForTests();
  });

  it("ships every H1 action with a default binding", () => {
    for (const id of H1_IDS) {
      expect(DEFAULT_BINDINGS[id]).toBeDefined();
    }
  });

  it("classifies every H1 action under the `panels` category", () => {
    for (const id of H1_IDS) {
      expect(ACTION_META[id].category).toBe("panels");
    }
  });

  it("binds the command palette to Cmd/Ctrl+K and the 5 flows to Mod+Shift+<letter>", () => {
    const expectations: Record<(typeof H1_IDS)[number], ShortcutBinding> = {
      openCommandPalette: { key: "k", mod: true, shift: false, alt: false },
      openTemplates: { key: "t", mod: true, shift: true, alt: false },
      openTheme: { key: "y", mod: true, shift: true, alt: false },
      openElements: { key: "e", mod: true, shift: true, alt: false },
      openMagicResize: { key: "r", mod: true, shift: true, alt: false },
      openAiGenerate: { key: "g", mod: true, shift: true, alt: false },
    };
    for (const id of H1_IDS) {
      expect(DEFAULT_BINDINGS[id]).toEqual(expectations[id]);
    }
  });

  it("does not collide with any existing default binding", () => {
    const store = shortcutStore();
    for (const id of H1_IDS) {
      expect(store.findConflicts(DEFAULT_BINDINGS[id], id)).toEqual([]);
    }
  });

  it("keeps the H1 bindings pairwise distinct", () => {
    for (let i = 0; i < H1_IDS.length; i += 1) {
      for (let j = i + 1; j < H1_IDS.length; j += 1) {
        const a = DEFAULT_BINDINGS[H1_IDS[i]!];
        const b = DEFAULT_BINDINGS[H1_IDS[j]!];
        expect(bindingsEqual(a, b)).toBe(false);
      }
    }
  });

  it("dispatches on the documented keystrokes via matchesBinding", () => {
    // Ctrl+K opens the palette and nothing else letter-shares with it.
    const ctrlK = makeKeyboardEvent({ key: "k", ctrlKey: true });
    expect(matchesBinding(ctrlK, DEFAULT_BINDINGS.openCommandPalette)).toBe(
      true,
    );
    // Cmd+K (macOS) also matches — `mod` is Ctrl OR Meta.
    const cmdK = makeKeyboardEvent({ key: "k", metaKey: true });
    expect(matchesBinding(cmdK, DEFAULT_BINDINGS.openCommandPalette)).toBe(
      true,
    );
    // Bare "k" (a future text-tool style chord) must NOT open the palette.
    const bareK = makeKeyboardEvent({ key: "k" });
    expect(matchesBinding(bareK, DEFAULT_BINDINGS.openCommandPalette)).toBe(
      false,
    );

    // Ctrl+Shift+E opens elements; Ctrl+E (a different binding) does not.
    const ctrlShiftE = makeKeyboardEvent({
      key: "e",
      ctrlKey: true,
      shiftKey: true,
    });
    expect(matchesBinding(ctrlShiftE, DEFAULT_BINDINGS.openElements)).toBe(
      true,
    );
    const ctrlE = makeKeyboardEvent({ key: "e", ctrlKey: true });
    expect(matchesBinding(ctrlE, DEFAULT_BINDINGS.openElements)).toBe(false);
  });
});
