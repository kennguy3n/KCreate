// Unit tests for the RightPanel tab-strip clamping helper.
//
// The helper (`apps/desktop/renderer/src/lib/rightPanelTabs.ts`) is a
// pure function that decides which tab the right panel should
// display after a mode transition removes the previously-active
// entry. Devin Review surfaced the underlying bug on PR #31 round
// 3 (`RightPanel.tsx:205`) — see the doc comment on
// `clampTabToAvailable` for the full story. The component side is
// the standard React pattern (a `useEffect` watching the memoized
// tab strip), but the decision logic is more interesting to test
// than the effect plumbing, so it lives here as a pure function we
// can exercise without booting a renderer.
//
// Compilation strategy mirrors `templates.test.mjs`: compile the TS
// source to in-memory ESM via `esbuild`, import via data URL, and
// memoize so every test in the file shares one compile.
import { test } from "node:test";
import assert from "node:assert/strict";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";

const TESTS_DIR = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(TESTS_DIR, "..");

let _modulePromise = null;
async function loadHelper() {
  if (_modulePromise) return _modulePromise;
  _modulePromise = (async () => {
    const result = await build({
      entryPoints: [resolve(ROOT, "src/lib/rightPanelTabs.ts")],
      bundle: true,
      format: "esm",
      target: ["es2022"],
      platform: "neutral",
      write: false,
      legalComments: "none",
      mainFields: ["module", "main"],
      conditions: ["import", "default"],
    });
    const code = result.outputFiles[0]?.text;
    if (!code) {
      throw new Error("failed to compile src/lib/rightPanelTabs.ts");
    }
    const dataUrl = `data:text/javascript;base64,${Buffer.from(code).toString(
      "base64",
    )}`;
    return import(dataUrl);
  })();
  return _modulePromise;
}

/// Build a minimal RightPanel-shaped tab strip for the synthetic
/// mode transitions tested below. We don't pull the real `IconName`
/// union — the helper is generic over `T extends string` and only
/// cares about the `id` field, so simple string ids keep the test
/// fixture light and self-contained.
function tabs(...ids) {
  return ids.map((id) => ({ id }));
}

test("clampTabToAvailable: keeps current tab when present in strip", async () => {
  const { clampTabToAvailable } = await loadHelper();
  // Common case — design mode strip with Accessibility entry, user is
  // on Accessibility. No state write should be triggered.
  const strip = tabs(
    "properties",
    "effects",
    "ai",
    "export",
    "inspect",
    "history",
    "accessibility",
    "color",
    "presence",
  );
  const result = clampTabToAvailable("accessibility", strip);
  assert.equal(result, "accessibility");
  // Identity check: returning the literal we passed in (not a new
  // value) is the contract the `useEffect` relies on to skip the
  // `setTab` write when nothing changed.
  assert.equal(result === "accessibility", true);
});

test("clampTabToAvailable: falls back to first tab when current removed by mode change", async () => {
  const { clampTabToAvailable } = await loadHelper();
  // Simulated transition: user was on Accessibility in design mode,
  // switched to vector mode (no Accessibility / Color / Preflight /
  // Interaction). The clamp should send them to the first visible
  // tab (Properties) — the most sensible default since the strip is
  // rendered in user-facing reading order.
  const vectorModeStrip = tabs(
    "properties",
    "effects",
    "ai",
    "export",
    "inspect",
    "history",
    "presence",
    "constraints",
    "tokens",
    "publish",
    "encryption",
  );
  const result = clampTabToAvailable("accessibility", vectorModeStrip);
  assert.equal(result, "properties");
});

test("clampTabToAvailable: handles every mode-conditional removal", async () => {
  const { clampTabToAvailable } = await loadHelper();
  // Pin each of the five mode-gated tabs (accessibility,
  // interaction, preflight, color, theme) — these are the only
  // entries that can disappear from the strip under a mode
  // transition, so they're the entire failure surface this helper
  // exists to handle. Strip without ANY mode-conditional tabs (e.g.
  // image mode).
  const minimalStrip = tabs(
    "properties",
    "effects",
    "ai",
    "export",
    "inspect",
    "history",
    "presence",
    "constraints",
    "tokens",
    "publish",
    "encryption",
  );
  for (const ghostTab of [
    "accessibility",
    "interaction",
    "preflight",
    "color",
    "theme",
  ]) {
    const result = clampTabToAvailable(ghostTab, minimalStrip);
    assert.equal(
      result,
      "properties",
      `clamp from ${ghostTab} should fall back to properties`,
    );
  }
});

test("clampTabToAvailable: keeps the always-on tabs across every mode", async () => {
  const { clampTabToAvailable } = await loadHelper();
  // The Properties / Effects / AI / Export / Inspect / History tabs
  // plus Presence / Constraints / Tokens / Publish / Encryption are
  // always present (Phase 8 Block C surfaces). A user sitting on any
  // of those should never be clamped away.
  const designStrip = tabs(
    "properties",
    "effects",
    "ai",
    "export",
    "inspect",
    "history",
    "accessibility",
    "color",
    "presence",
    "constraints",
    "tokens",
    "publish",
    "encryption",
  );
  for (const stableTab of [
    "properties",
    "effects",
    "ai",
    "export",
    "inspect",
    "history",
    "presence",
    "constraints",
    "tokens",
    "publish",
    "encryption",
  ]) {
    assert.equal(clampTabToAvailable(stableTab, designStrip), stableTab);
  }
});

test("clampTabToAvailable: empty strip returns current unchanged (defensive)", async () => {
  const { clampTabToAvailable } = await loadHelper();
  // Production never hits this — RightPanel always has at least the
  // BASE_TABS entries. But the helper must not throw on an empty
  // strip (e.g. a future refactor that builds the tab list
  // asynchronously). Returning `current` keeps the component
  // consistent rather than crashing into a fallback that doesn't
  // exist either.
  assert.equal(clampTabToAvailable("properties", []), "properties");
  assert.equal(clampTabToAvailable("accessibility", []), "accessibility");
});

test("clampTabToAvailable: clamps when current is a typo-like string outside the union", async () => {
  const { clampTabToAvailable } = await loadHelper();
  // The helper is generic so it doesn't type-narrow the input — a
  // future caller passing a string that isn't in the union should
  // still be clamped to a real tab. (In practice the union narrows
  // this away, but the helper's behavior is defined.)
  const strip = tabs("properties", "effects", "history");
  const result = clampTabToAvailable("definitely-not-a-tab", strip);
  assert.equal(result, "properties");
});

test("clampTabToAvailable: fallback respects strip order — first entry wins", async () => {
  const { clampTabToAvailable } = await loadHelper();
  // A future contributor might reorder BASE_TABS (e.g. promote
  // Inspect ahead of Properties). The helper picks the first entry
  // in the strip, NOT a hard-coded "properties" literal, so the
  // contract follows whatever ordering the component decides on.
  const reorderedStrip = tabs("inspect", "properties", "effects");
  assert.equal(
    clampTabToAvailable("accessibility", reorderedStrip),
    "inspect",
  );
});
