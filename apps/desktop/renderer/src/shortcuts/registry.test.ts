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
  code?: string;
  ctrlKey?: boolean;
  metaKey?: boolean;
  altKey?: boolean;
  shiftKey?: boolean;
}): KeyboardEvent {
  return new KeyboardEvent("keydown", {
    key: init.key,
    code: init.code ?? "",
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

  // Regression for the macOS Option dead-key bug: pressing Option+V
  // (without Cmd) on macOS delivers `event.key === "√"` while the
  // bound `key` is the pre-transformation letter "v". Without the
  // `event.code` fallback in `matchesBinding`, alignment shortcuts
  // would silently fail to fire on every Mac in the user base.
  //
  // Each letter row maps Option+<letter> → the Unicode glyph macOS
  // produces. Values cribbed directly from a macOS Sequoia keyboard
  // viewer (US-English layout) at the time this test was written.
  it("matchesBinding honours event.code for Alt-only letter chords (macOS Option key)", () => {
    const macOptionGlyphs: ReadonlyArray<{
      action: (typeof ALIGNMENT_IDS)[number];
      key: string;
      glyph: string;
      code: string;
    }> = [
      { action: "alignLeft", key: "a", glyph: "\u00e5", code: "KeyA" },
      { action: "alignCenterX", key: "h", glyph: "\u02d9", code: "KeyH" },
      { action: "alignRight", key: "d", glyph: "\u2202", code: "KeyD" },
      { action: "alignTop", key: "w", glyph: "\u2211", code: "KeyW" },
      { action: "alignCenterY", key: "v", glyph: "\u221a", code: "KeyV" },
      { action: "alignBottom", key: "s", glyph: "\u00df", code: "KeyS" },
    ];
    for (const { action, key, glyph, code } of macOptionGlyphs) {
      // Simulate the macOS event: `event.key` is the Option-transformed
      // glyph (would have been `key` on Windows / Linux), `event.code`
      // remains the physical key identifier.
      const ev = makeKeyboardEvent({ key: glyph, code, altKey: true });
      expect(
        matchesBinding(ev, DEFAULT_BINDINGS[action]),
        `${action} should fire when the user presses Option+${key} on macOS (event.key="${glyph}", event.code="${code}")`,
      ).toBe(true);
    }
  });

  // Cmd+Option+letter is unaffected by the dead-key bug because
  // Chromium prioritises Cmd for character generation, so
  // `event.key` stays as the original letter ("h" / "v"). The
  // event.code path still has to be a no-op here — if it kicked in
  // unconditionally we could end up matching a Cmd-only binding
  // against a Cmd+Option event, or vice versa.
  it("matchesBinding still distinguishes Alt vs Cmd+Alt on the macOS path", () => {
    const cmdOptH = makeKeyboardEvent({
      key: "h",
      code: "KeyH",
      metaKey: true,
      altKey: true,
    });
    expect(matchesBinding(cmdOptH, DEFAULT_BINDINGS.distributeHorizontal)).toBe(
      true,
    );
    expect(matchesBinding(cmdOptH, DEFAULT_BINDINGS.alignCenterX)).toBe(false);

    // And an Option-only event with the dead-key glyph should NOT
    // match the Cmd+Option distribute binding even though its
    // event.code matches.
    const optHGlyph = makeKeyboardEvent({
      key: "\u02d9",
      code: "KeyH",
      altKey: true,
    });
    expect(
      matchesBinding(optHGlyph, DEFAULT_BINDINGS.distributeHorizontal),
    ).toBe(false);
    expect(matchesBinding(optHGlyph, DEFAULT_BINDINGS.alignCenterX)).toBe(true);
  });

  // Defence-in-depth: the `event.code` fallback must only kick in
  // for `KeyA`–`KeyZ` codes when `altKey` is set. Otherwise it
  // would shadow non-alpha bindings (e.g. an Alt+Slash chord whose
  // `event.code` is "Slash" but whose bound `key` is "/"). Both
  // these patterns are currently absent from the registry, but the
  // helper has to stay safe for future bindings.
  it("matchesBinding falls back to event.key for non-letter codes even when altKey is set", () => {
    // An Alt+/ chord on macOS: `event.key === "÷"` and `event.code
    // === "Slash"`. Neither side of the chord matches "v", so
    // alignCenterY must NOT fire.
    const altSlash = makeKeyboardEvent({
      key: "\u00f7",
      code: "Slash",
      altKey: true,
    });
    expect(matchesBinding(altSlash, DEFAULT_BINDINGS.alignCenterY)).toBe(false);
    // And conversely, a hypothetical alt-only binding on `/` would
    // still match via event.key because the code path doesn't
    // engage for "Slash".
    const altSlashBinding: ShortcutBinding = {
      key: "/",
      mod: false,
      shift: false,
      alt: true,
    };
    const altSlashEvent = makeKeyboardEvent({
      key: "/",
      code: "Slash",
      altKey: true,
    });
    expect(matchesBinding(altSlashEvent, altSlashBinding)).toBe(true);
  });
});
