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
  isCanonicalAltBinding,
  matchesBinding,
  resetShortcutStoreForTests,
  shortcutStore,
  type ActionId,
  type ShortcutBinding,
} from "./registry";

const STORAGE_KEY = "kcreate.shortcuts.v1";

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
    // Tests that exercise the load path persist entries to
    // localStorage; clear it before each test so they don't leak.
    if (typeof window !== "undefined" && window.localStorage) {
      window.localStorage.removeItem(STORAGE_KEY);
    }
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

  // Regression for Devin Review ANALYSIS_0005 on PR #41: the storage
  // invariant documented on `bindingsEqual` requires every binding in
  // the runtime store to be in canonical (physical-key) form so the
  // `bindingsEqual(a,b) ⇔ matchesBinding(event,a) = matchesBinding(event,b)`
  // equivalence holds without `bindingsEqual` needing a
  // `KeyboardEvent` to consult. `isCanonicalAltBinding` is the
  // structural gate.
  it("isCanonicalAltBinding accepts ASCII alt-bindings and rejects glyph alt-bindings", () => {
    // Defaults must all pass.
    for (const id of Object.keys(DEFAULT_BINDINGS) as ActionId[]) {
      expect(isCanonicalAltBinding(DEFAULT_BINDINGS[id])).toBe(true);
    }
    // Common macOS Option-key glyphs that the pre-fix recorder would
    // have stored if the user re-bound an alt action: '√' for
    // Option+V, 'å' for Option+A, '∂' for Option+D, '∑' for Option+W,
    // 'ß' for Option+S, '˙' for Option+H. All MUST be rejected.
    const glyphs = ["\u221a", "\u00e5", "\u2202", "\u2211", "\u00df", "\u02d9"];
    for (const g of glyphs) {
      expect(
        isCanonicalAltBinding({ key: g, mod: false, shift: false, alt: true }),
      ).toBe(false);
    }
    // Non-alt bindings always pass even if the key happens to be a
    // glyph (matchesBinding never routes them through eventKeyForMatching).
    expect(
      isCanonicalAltBinding({
        key: "\u221a",
        mod: false,
        shift: false,
        alt: false,
      }),
    ).toBe(true);
    // Multi-character keys (DOM `KeyboardEvent.key` names) always pass.
    for (const k of ["Escape", "ArrowLeft", "F1", "Delete"]) {
      expect(
        isCanonicalAltBinding({ key: k, mod: false, shift: false, alt: true }),
      ).toBe(true);
    }
  });

  // Regression for Devin Review ANALYSIS_0005 on PR #41: the store
  // constructor's load-path validator must drop any stored
  // alt-binding whose key is a non-ASCII glyph so the storage
  // invariant is upheld structurally. Without this guard, a stale
  // localStorage entry from a pre-eventKeyForMatching app version
  // would survive into the runtime store and silently violate the
  // `bindingsEqual ⇔ matchesBinding` equivalence — a glyph entry
  // would compare as "no conflict" against an ASCII entry but both
  // would match the same physical keystroke.
  it("ShortcutStore drops stale macOS Option-key glyph bindings at load time", () => {
    // Simulate a pre-fix app version having persisted Alt+V with the
    // glyph key (Option+V on US QWERTY produces '√').
    const stale = {
      alignCenterY: {
        key: "\u221a",
        mod: false,
        shift: false,
        alt: true,
      } satisfies ShortcutBinding,
    };
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(stale));
    resetShortcutStoreForTests();
    const snap = shortcutStore().snapshot();
    // Must fall back to the shipped default ("v"), not preserve "√".
    expect(snap.alignCenterY).toEqual(DEFAULT_BINDINGS.alignCenterY);
    expect(snap.alignCenterY.key).toBe("v");
  });

  // Companion: a CANONICAL stored binding (e.g. the user rebound
  // alignCenterY to Alt+B via the post-fix recorder) MUST survive the
  // load path unchanged.
  it("ShortcutStore preserves canonical (physical-key) stored bindings across reload", () => {
    const rebound = {
      alignCenterY: {
        key: "b",
        mod: false,
        shift: false,
        alt: true,
      } satisfies ShortcutBinding,
    };
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(rebound));
    resetShortcutStoreForTests();
    expect(shortcutStore().snapshot().alignCenterY.key).toBe("b");
  });
});
