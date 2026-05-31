// Unit tests for the HomePage template resolvers.
//
// The resolvers drive `window.kcreate.canvas.createRect/createText` +
// `window.kcreate.document.updateNode` to seed starter content the
// moment the user clicks a card on the HomePage. This file pins
// down each resolver's bridge call sequence so a regression (wrong
// palette colour, missing logo placeholder, dropped subtitle, …)
// surfaces as a unit-test failure instead of being noticed only on
// visual inspection of the running editor.
//
// Strategy: compile `src/lib/templates.ts` to an ESM module via
// `esbuild` (mirrors the pattern in `apps/kchat-extension/tests/`,
// which is the only other TypeScript test suite in the repo), then
// mount a recording stub on `globalThis.window.kcreate` that
// captures every bridge call. Each test invokes one resolver with a
// synthetic `TemplateContext` and asserts:
//
//   * the recorded `createRect` / `createText` arg shape is sane
//     (positive width/height, ints, inside the artboard rect);
//   * specific palette colours land on the right nodes (via the
//     follow-up `document.updateNode({ fill })` calls);
//   * the named nodes the demo / README references (e.g.
//     "Brand title", "Palette / Espresso", "Drop zone") are
//     actually created.
//
// The resolvers are fully isolated from Electron / Node IPC by the
// `window.kcreate.*` indirection, so no Electron bridge needs to
// boot for these tests — the recording stub is enough.
import { test } from "node:test";
import assert from "node:assert/strict";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";

const TESTS_DIR = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(TESTS_DIR, "..");

/// Compile the resolver module + its (type-only) deps to an
/// in-memory ESM bundle and import it via a data: URL so the tests
/// run without a pre-built `dist/`. Matches the strategy used by
/// `apps/kchat-extension/tests/manifest.test.mjs`.
///
/// The compiled module is memoised at file scope: every test sees the
/// same imported module reference, which is sound because the
/// resolver code is purely functional (no module-level state) and is
/// the same input on every call. Recompiling on each test wastes
/// ~50–100ms × N tests = up to a second of CI time for a result that
/// is guaranteed to be byte-identical.
let _resolverModulePromise = null;
async function loadResolvers() {
  if (_resolverModulePromise) return _resolverModulePromise;
  _resolverModulePromise = (async () => {
    const result = await build({
      entryPoints: [resolve(ROOT, "src/lib/templates.ts")],
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
      throw new Error("failed to compile src/lib/templates.ts");
    }
    const dataUrl = `data:text/javascript;base64,${Buffer.from(code).toString(
      "base64",
    )}`;
    return import(dataUrl);
  })();
  return _resolverModulePromise;
}

/// Build a recording stub for `window.kcreate` and install it as
/// the global `window` so the resolver can call into it without an
/// Electron bridge. The stub assigns a deterministic node id of the
/// form `n-N` (N = 0, 1, 2, …) to every `createRect` / `createText`
/// call so the follow-up `updateNode` calls can be correlated with
/// the node they paint.
///
/// `globalThis.window` is overwritten for the lifetime of the test;
/// the previous value is saved and restored via `t.after()` so a
/// future contributor enabling `--test-concurrency` (or interleaving
/// tests with a jsdom-installing helper) does not stomp on a sibling
/// suite's window. Pass the test's `t` from the `node:test` callback
/// to opt into the auto-restore; tests that forget to thread `t`
/// through will still see the recorder, but won't get cleanup — the
/// in-tree tests all pass `t` so this stays uniform.
function installRecorder(t) {
  const calls = [];
  let nextId = 0;
  // Track every `createNodes` invocation separately so a test can
  // assert "the resolver actually used the batch surface" without
  // having to inspect the per-item synthesized calls below. Each
  // entry has `items` (the raw batch) and `ids` (the assigned ids
  // in submission order) so tests can verify both the wire shape
  // and the id mapping the resolver received.
  const batchCalls = [];
  const recorder = {
    canvas: {
      async createRect(_parentId, x, y, w, h) {
        const id = `n-${nextId++}`;
        calls.push({ kind: "createRect", id, x, y, w, h });
        return id;
      },
      async createText(_parentId, x, y, body, family, size) {
        const id = `n-${nextId++}`;
        calls.push({ kind: "createText", id, x, y, body, family, size });
        return id;
      },
      async createNodes(items) {
        // Synthesize the same per-item `createRect`/`createText` +
        // `updateNode` calls the old non-batch path would have
        // produced. This preserves the existing `nodesFromCalls`
        // aggregator + every per-node assertion verbatim, while
        // also exposing the batch shape via `batchCalls` for tests
        // that need to assert "the batch surface is what was used".
        const ids = [];
        for (const item of items) {
          const id = `n-${nextId++}`;
          ids.push(id);
          switch (item.kind) {
            case "rect":
              calls.push({
                kind: "createRect",
                id,
                x: item.x,
                y: item.y,
                w: item.w,
                h: item.h,
              });
              break;
            case "text":
              calls.push({
                kind: "createText",
                id,
                x: item.x,
                y: item.y,
                body: item.body,
                family: item.family,
                size: item.size,
              });
              break;
            case "ellipse":
              calls.push({
                kind: "createEllipse",
                id,
                cx: item.cx,
                cy: item.cy,
                rx: item.rx,
                ry: item.ry,
              });
              break;
            case "line":
              calls.push({
                kind: "createLine",
                id,
                x1: item.x1,
                y1: item.y1,
                x2: item.x2,
                y2: item.y2,
              });
              break;
            default:
              throw new Error(`unknown batch item kind: ${item.kind}`);
          }
          if (item.fill !== undefined || item.name !== undefined) {
            const props = {};
            if (item.fill !== undefined) props.fill = item.fill;
            if (item.name !== undefined) props.name = item.name;
            calls.push({ kind: "updateNode", id, props });
          }
        }
        batchCalls.push({ items, ids });
        return ids;
      },
    },
    document: {
      async updateNode(id, props) {
        calls.push({ kind: "updateNode", id, props });
      },
    },
  };
  // jsdom isn't pulled in for these tests; assign a minimal stand-in
  // for `window` so `window.kcreate.*` resolves. The resolver never
  // touches anything else on `window`. We save the previous value
  // (`undefined` in the default Node runtime, but a real `Window`
  // object if something like jsdom wired one up) so the teardown can
  // restore it without unconditionally `delete`ing the global.
  const hadPrevious = "window" in globalThis;
  const previous = globalThis.window;
  globalThis.window = { kcreate: recorder };
  if (t && typeof t.after === "function") {
    t.after(() => {
      if (hadPrevious) {
        globalThis.window = previous;
      } else {
        delete globalThis.window;
      }
    });
  }
  // Attach the per-batch view as a property on the returned array
  // so existing callers (`const calls = installRecorder(t)`) keep
  // working unchanged, and the few callers that want to assert on
  // batching can reach for `calls.batchCalls` without a recorder
  // API change. Arrays in JS allow arbitrary properties, and the
  // standard array methods (`for-of`, `.filter`, `.map`, …) don't
  // care.
  calls.batchCalls = batchCalls;
  return calls;
}

/// Convenience: bucket the recorded calls by node id so each test
/// can assert "node N is a rect with fill X named Y" in one step.
function nodesFromCalls(calls) {
  const nodes = new Map();
  for (const call of calls) {
    if (call.kind === "createRect" || call.kind === "createText") {
      nodes.set(call.id, { create: call, updates: [] });
    } else if (call.kind === "updateNode") {
      const entry = nodes.get(call.id);
      if (!entry) {
        throw new Error(`updateNode for unknown id ${call.id}`);
      }
      entry.updates.push(call.props);
    }
  }
  return nodes;
}

function nameOf(entry) {
  const named = entry.updates.find((u) => "name" in u);
  return named ? named.name : null;
}

function fillOf(entry) {
  const filled = entry.updates.find((u) => "fill" in u);
  return filled ? filled.fill : null;
}

/// All node creates should land inside the artboard rect. (Slight
/// overdraw on the right edge is permitted because the resolvers
/// round margins.)
function assertInsideArtboard(calls, ctx) {
  const right = ctx.x + ctx.width;
  const bottom = ctx.y + ctx.height;
  for (const call of calls) {
    if (call.kind !== "createRect" && call.kind !== "createText") continue;
    assert.ok(
      call.x >= ctx.x - 1,
      `${call.kind}(${call.id}) x=${call.x} < artboard.x=${ctx.x}`,
    );
    assert.ok(
      call.y >= ctx.y - 1,
      `${call.kind}(${call.id}) y=${call.y} < artboard.y=${ctx.y}`,
    );
    if (call.kind === "createRect") {
      assert.ok(call.w >= 0, `rect w must be >= 0, got ${call.w}`);
      assert.ok(call.h >= 0, `rect h must be >= 0, got ${call.h}`);
      assert.ok(
        call.x + call.w <= right + 1,
        `rect right edge ${call.x + call.w} > artboard right ${right}`,
      );
      assert.ok(
        call.y + call.h <= bottom + 1,
        `rect bottom edge ${call.y + call.h} > artboard bottom ${bottom}`,
      );
    }
  }
}

/// Convert a 0xRRGGBB literal to the same RgbaColor shape the
/// resolver's `hex(...)` helper produces, for direct equality
/// against the recorded `updateNode({ fill })` payload.
function paletteFill(hex, alpha = 1.0) {
  const raw = hex.startsWith("#") ? hex.slice(1) : hex;
  return {
    kind: "solid",
    r: parseInt(raw.slice(0, 2), 16) / 255,
    g: parseInt(raw.slice(2, 4), 16) / 255,
    b: parseInt(raw.slice(4, 6), 16) / 255,
    a: alpha,
  };
}

// Shared 1920×1080 artboard at world (0, 0) — matches the default
// the bridge hands the first card. A second test set below uses a
// non-default Y offset so the `ay` plumbing is exercised too.
const A1080 = { x: 0, y: 0, width: 1920, height: 1080 };

test("BRAND_PALETTE exports the six Brewline colours used by the demo", async () => {
  const { BRAND_PALETTE } = await loadResolvers();
  // These exact strings are referenced in docs/demo/brewline-v2 +
  // the resolver visual assertions; pin them here so a rename of
  // any swatch surfaces as a failed test.
  assert.equal(BRAND_PALETTE.espresso, "#3E2723");
  assert.equal(BRAND_PALETTE.cream, "#FFF8E1");
  assert.equal(BRAND_PALETTE.burntOrange, "#E65100");
  assert.equal(BRAND_PALETTE.sage, "#689F63");
  assert.equal(BRAND_PALETTE.ink, "#111827");
  assert.equal(BRAND_PALETTE.paper, "#F8FAFC");
});

test("templateResolverFor returns a resolver for every CREATE_OPTIONS id with a template", async () => {
  const { templateResolverFor, TEMPLATE_RESOLVERS } = await loadResolvers();
  // Sanity: the 7 ids the HomePage seeds. `import` is intentionally
  // absent (file-picker flow, no seed).
  const seededIds = [
    "brand",
    "social",
    "print",
    "app-ui",
    "photo",
    "deck",
    "dev-export",
  ];
  for (const id of seededIds) {
    assert.ok(
      typeof templateResolverFor(id)?.apply === "function",
      `expected a resolver for ${id}`,
    );
    assert.ok(TEMPLATE_RESOLVERS[id], `expected TEMPLATE_RESOLVERS[${id}]`);
  }
  assert.equal(
    templateResolverFor("import"),
    undefined,
    "import card must NOT have a resolver (file picker drives it)",
  );
  assert.equal(
    templateResolverFor("totally-fake"),
    undefined,
    "unknown jobKind must yield undefined",
  );
});

test("BRAND resolver seeds title, tagline, four palette swatches with labels, and a logo placeholder + caption", async (t) => {
  const { BRAND_PALETTE, templateResolverFor } = await loadResolvers();
  const calls = installRecorder(t);
  await templateResolverFor("brand").apply(A1080);
  assertInsideArtboard(calls, A1080);
  const nodes = nodesFromCalls(calls);

  const named = new Map(
    [...nodes.values()]
      .map((n) => [nameOf(n), n])
      .filter(([n]) => n !== null),
  );
  // Heading + tagline.
  assert.ok(named.has("Brand title"), "missing 'Brand title' text node");
  assert.ok(named.has("Tagline"), "missing 'Tagline' text node");
  assert.equal(named.get("Brand title").create.kind, "createText");
  assert.equal(named.get("Brand title").create.size, 64);
  assert.equal(named.get("Tagline").create.size, 24);

  // 4 named palette swatches, each with the right fill. Labels
  // come from the BRAND_RESOLVER `swatches` table (mixed-case
  // "Burnt orange", not "Burnt Orange" — the resolver wrote it
  // sentence-case, so the test pins the actual string).
  for (const [label, hex] of [
    ["Palette / Espresso", BRAND_PALETTE.espresso],
    ["Palette / Cream", BRAND_PALETTE.cream],
    ["Palette / Burnt orange", BRAND_PALETTE.burntOrange],
    ["Palette / Sage", BRAND_PALETTE.sage],
  ]) {
    assert.ok(named.has(label), `missing swatch '${label}'`);
    const entry = named.get(label);
    assert.equal(entry.create.kind, "createRect");
    assert.deepEqual(fillOf(entry), paletteFill(hex));
  }

  // Logo placeholder + caption.
  assert.ok(
    named.has("Logo placeholder"),
    "missing 'Logo placeholder' rect node",
  );
  assert.equal(named.get("Logo placeholder").create.kind, "createRect");
  assert.ok(named.has("Logo caption"), "missing 'Logo caption' text node");
  assert.equal(named.get("Logo caption").create.kind, "createText");
});

// The shipped `brand` card on HomePage.tsx wires the editor to a
// 1024×1024 artboard. The general BRAND test above runs against
// 1920×1080 (a stress test for the non-square case), so this
// supplementary case pins the *production* geometry: every seeded
// node must land inside the actual preset and the resolver must
// still emit the full set of named nodes (title, tagline, four
// swatches, logo placeholder + caption) at the smaller square
// dimension. Mirrors the bot's review suggestion on commit 9746c8e.
test("BRAND resolver lays out cleanly on the actual 1024\u00d71024 brand preset", async (t) => {
  const { BRAND_PALETTE, templateResolverFor } = await loadResolvers();
  const ctx = { x: 0, y: 0, width: 1024, height: 1024 };
  const calls = installRecorder(t);
  await templateResolverFor("brand").apply(ctx);
  assertInsideArtboard(calls, ctx);
  const nodes = nodesFromCalls(calls);
  const named = new Map(
    [...nodes.values()]
      .map((n) => [nameOf(n), n])
      .filter(([n]) => n !== null),
  );
  // All headline + tagline + swatches + logo nodes must still be
  // present at the smaller square dimension.
  for (const required of [
    "Brand title",
    "Tagline",
    "Palette / Espresso",
    "Palette / Cream",
    "Palette / Burnt orange",
    "Palette / Sage",
    "Logo placeholder",
    "Logo caption",
  ]) {
    assert.ok(named.has(required), `missing '${required}' on 1024\u00d71024`);
  }
  // Swatches keep their palette colours.
  assert.deepEqual(
    fillOf(named.get("Palette / Espresso")),
    paletteFill(BRAND_PALETTE.espresso),
  );
  assert.deepEqual(
    fillOf(named.get("Palette / Sage")),
    paletteFill(BRAND_PALETTE.sage),
  );
  // Swatch row sits inside the artboard horizontally (no overflow on
  // the right edge). Worst-case offender on a tight square preset is
  // the last swatch's right edge.
  const lastSwatch = named.get("Palette / Sage");
  const lastRight = lastSwatch.create.x + lastSwatch.create.w;
  assert.ok(
    lastRight <= ctx.width,
    `last swatch right edge ${lastRight} > artboard width ${ctx.width}`,
  );
  // Floor invariant on the four-swatch budget. The resolver computes
  // `swatchWidth = Math.floor((swatchRowWidth - swatchGap * 3) / 4)`
  // so `4*swatchWidth + 3*swatchGap <= swatchRowWidth` (with at most
  // 3px of unused slack — one per dropped fractional part). With
  // `Math.round` the sum could exceed the row width by ~2px on an
  // unfavorable numerator. This is exercised here against the actual
  // 1024\u00d71024 shipped preset so a future regression to `Math.round`
  // (or any other rounding mode that can round up) would be caught
  // by the inequality regardless of whether the production geometry
  // happens to have slack. Mirrors the DECK_RESOLVER floor invariant.
  const espresso = named.get("Palette / Espresso");
  const sage = named.get("Palette / Sage");
  assert.ok(espresso, "missing 'Palette / Espresso' rect for floor check");
  assert.ok(sage, "missing 'Palette / Sage' rect for floor check");
  const swatchWidth = espresso.create.w;
  // Adjacent swatches are spaced by `swatchWidth + swatchGap` so we
  // can recover `swatchGap` from any two consecutive swatches.
  const cream = named.get("Palette / Cream");
  assert.ok(cream, "missing 'Palette / Cream' rect for gap check");
  const swatchGap = cream.create.x - (espresso.create.x + swatchWidth);
  const margin = espresso.create.x;
  const swatchRowWidth = ctx.width - margin * 2;
  assert.ok(
    4 * swatchWidth + 3 * swatchGap <= swatchRowWidth,
    `floor invariant violated: 4*${swatchWidth} + 3*${swatchGap} = ` +
      `${4 * swatchWidth + 3 * swatchGap} > swatchRowWidth ${swatchRowWidth}`,
  );
});

// BRAND_RESOLVER originally set `swatchHeight = swatchWidth` (square
// tile), which fits cleanly on the shipped 1024\u00d71024 brand preset
// but overlaps the logo placeholder on wide artboards. On 1920\u00d71080:
// swatchWidth = 400, swatchY = 486, label baseline \u2248 922, logo top =
// 658 \u2192 264px of label/logo overlap. Devin Review surfaced this on
// commit 55afb7b. The fix clamps `swatchHeight` to the available
// vertical budget between `swatchY` and the logo top (minus a label
// + safety reserve). This test exercises the 1920\u00d71080 stress case
// and asserts the swatch row + its labels never collide with the
// logo placeholder, regardless of artboard aspect ratio.
test("BRAND resolver clamps swatchHeight so labels never overlap the logo on a wide artboard", async (t) => {
  const { templateResolverFor } = await loadResolvers();
  const calls = installRecorder(t);
  const ctx = { x: 0, y: 0, width: 1920, height: 1080 };
  await templateResolverFor("brand").apply(ctx);
  const nodes = nodesFromCalls(calls);
  const named = new Map(
    [...nodes.values()]
      .map((n) => [nameOf(n), n])
      .filter(([n]) => n !== null),
  );
  const swatch = named.get("Palette / Espresso");
  const swatchLabel = named.get("Swatch label / Espresso");
  const logo = named.get("Logo placeholder");
  assert.ok(swatch, "missing 'Palette / Espresso' rect for overlap check");
  assert.ok(swatchLabel, "missing 'Swatch label / Espresso' text for overlap check");
  assert.ok(logo, "missing 'Logo placeholder' rect for overlap check");
  // Swatch row bottom must clear the logo top.
  const swatchBottom = swatch.create.y + swatch.create.h;
  const logoTop = logo.create.y;
  assert.ok(
    swatchBottom <= logoTop,
    `swatch row bottom ${swatchBottom} > logo top ${logoTop} (overlap)`,
  );
  // Label baseline (text y is its top; reserve 20px for the glyph
  // height to match the resolver's `labelHeight` budget) must also
  // clear the logo top.
  const labelBottom = swatchLabel.create.y + 20;
  assert.ok(
    labelBottom <= logoTop,
    `label bottom ${labelBottom} > logo top ${logoTop} (overlap)`,
  );
  // And the swatch must still have positive height (the clamp does
  // not collapse the row to zero on this stress case).
  assert.ok(
    swatch.create.h > 0,
    `swatch height ${swatch.create.h} must be > 0 on 1920\u00d71080`,
  );
});

// BRAND_RESOLVER originally placed the logo caption at
// `logoX + logoSize + 24` (right-of-logo), which fits cleanly on the
// shipped 1024\u00d71024 brand preset but overflows the right edge on a
// narrow artboard (Devin Review surfaced on commit cb2c097: at
// 300\u00d7300 the caption estimated at 247px wide starts at x=90 and
// ends at x=337, 55px past the 282px right margin). The fix detects
// the overflow case and flips the caption to *above* the logomark,
// left-aligned with it. This test exercises the 300\u00d7300 narrow
// case and asserts the caption origin stays within the artboard
// horizontally and the caption sits above (not on/below) the logo.
test("BRAND resolver flips logo caption above the logomark when right-of-logo would overflow on a narrow artboard", async (t) => {
  const { templateResolverFor } = await loadResolvers();
  const calls = installRecorder(t);
  const ctx = { x: 0, y: 0, width: 300, height: 300 };
  await templateResolverFor("brand").apply(ctx);
  const nodes = nodesFromCalls(calls);
  const named = new Map(
    [...nodes.values()]
      .map((n) => [nameOf(n), n])
      .filter(([n]) => n !== null),
  );
  const logo = named.get("Logo placeholder");
  const caption = named.get("Logo caption");
  assert.ok(logo, "missing 'Logo placeholder' rect for caption-flip check");
  assert.ok(caption, "missing 'Logo caption' text for caption-flip check");
  // Caption origin must be \u2264 logo top (caption is above, not beside).
  assert.ok(
    caption.create.y <= logo.create.y,
    `caption y ${caption.create.y} should be <= logo y ${logo.create.y} (above-logo flip)`,
  );
  // Caption origin must be at logo's left edge (left-aligned with logo).
  assert.equal(
    caption.create.x,
    logo.create.x,
    `caption x ${caption.create.x} should match logo x ${logo.create.x} when flipped above`,
  );
  // Caption origin must be inside the artboard horizontally.
  assert.ok(
    caption.create.x >= 0,
    `caption x ${caption.create.x} should be >= 0`,
  );
  assert.ok(
    caption.create.x < ctx.width,
    `caption x ${caption.create.x} should be < artboard width ${ctx.width}`,
  );
  // Caption origin must be inside the artboard vertically (positive y).
  assert.ok(
    caption.create.y >= 0,
    `caption y ${caption.create.y} should be >= 0 (above-logo flip must fit)`,
  );
});

// Sanity: the shipped 1024\u00d71024 brand preset must keep the original
// right-of-logo caption placement (no behavioural regression from the
// narrow-artboard defensive flip above).
test("BRAND resolver keeps logo caption to the right of the logomark on the shipped 1024\u00d71024 preset", async (t) => {
  const { templateResolverFor } = await loadResolvers();
  const calls = installRecorder(t);
  const ctx = { x: 0, y: 0, width: 1024, height: 1024 };
  await templateResolverFor("brand").apply(ctx);
  const nodes = nodesFromCalls(calls);
  const named = new Map(
    [...nodes.values()]
      .map((n) => [nameOf(n), n])
      .filter(([n]) => n !== null),
  );
  const logo = named.get("Logo placeholder");
  const caption = named.get("Logo caption");
  assert.ok(logo, "missing 'Logo placeholder' rect for caption-position check");
  assert.ok(caption, "missing 'Logo caption' text for caption-position check");
  // Caption must be to the right of the logo (not flipped above).
  assert.ok(
    caption.create.x > logo.create.x + logo.create.w,
    `caption x ${caption.create.x} should be right of logo right edge ${logo.create.x + logo.create.w} on shipped preset`,
  );
  // Caption must overlap the logo vertically (it is centred against
  // the logomark, not above/below it).
  assert.ok(
    caption.create.y >= logo.create.y,
    `caption y ${caption.create.y} should be >= logo top ${logo.create.y} on shipped preset`,
  );
  assert.ok(
    caption.create.y < logo.create.y + logo.create.h,
    `caption y ${caption.create.y} should be < logo bottom ${logo.create.y + logo.create.h} on shipped preset`,
  );
});

// APP_UI_RESOLVER computes `tileHeight = Math.max(0, tileBottom -
// tileTop)` so a future surface applying it to an extremely short
// artboard doesn't pass a negative height into `createRect`. The
// shipped 1440\u00d7900 preset has plenty of slack, so this test exercises
// a degenerate \u201cnegative budget\u201d artboard size and asserts the clamp
// kicks in: tile rects must still have `h >= 0`. Mirrors the
// documented clamp on `DECK_RESOLVER`'s `colHeight` (line 508).
test("APP_UI resolver clamps tileHeight to >= 0 on a degenerate short artboard", async (t) => {
  const { templateResolverFor } = await loadResolvers();
  const calls = installRecorder(t);
  // `headerH = round(50 * 0.08) = 4`, `tileMargin = 32`, so
  // `tileTop = 0 + 4 + 32 = 36`, `tileBottom = 0 + 50 - 32 = 18`,
  // and the un-clamped value would be `18 - 36 = -18`.
  const ctx = { x: 0, y: 0, width: 1440, height: 50 };
  await templateResolverFor("app-ui").apply(ctx);
  const nodes = nodesFromCalls(calls);
  const named = new Map(
    [...nodes.values()]
      .map((n) => [nameOf(n), n])
      .filter(([n]) => n !== null),
  );
  for (const i of [1, 2, 3]) {
    const tile = named.get(`Content tile ${i}`);
    assert.ok(tile, `missing 'Content tile ${i}'`);
    assert.ok(
      tile.create.h >= 0,
      `tile ${i} height ${tile.create.h} must be >= 0 after clamp`,
    );
  }
});

test("SOCIAL resolver seeds a cream background, burnt-orange headline band, headline + body copy, and sage accent", async (t) => {
  const { BRAND_PALETTE, templateResolverFor } = await loadResolvers();
  const calls = installRecorder(t);
  // The shipped social preset is 1080×1080.
  const ctx = { x: 0, y: 0, width: 1080, height: 1080 };
  await templateResolverFor("social").apply(ctx);
  assertInsideArtboard(calls, ctx);
  const nodes = nodesFromCalls(calls);
  const named = new Map(
    [...nodes.values()]
      .map((n) => [nameOf(n), n])
      .filter(([n]) => n !== null),
  );
  // First rect should be the full-bleed background painted cream.
  const bg = named.get("Background");
  assert.ok(bg, "missing 'Background' rect node");
  assert.equal(bg.create.kind, "createRect");
  assert.equal(bg.create.w, ctx.width);
  assert.equal(bg.create.h, ctx.height);
  assert.deepEqual(fillOf(bg), paletteFill(BRAND_PALETTE.cream));
  // Burnt-orange headline band.
  const band = named.get("Headline band");
  assert.ok(band, "missing 'Headline band' rect node");
  assert.deepEqual(fillOf(band), paletteFill(BRAND_PALETTE.burntOrange));
  // Headline + body.
  assert.ok(named.has("Headline"), "missing 'Headline' text node");
  assert.equal(named.get("Headline").create.size, 48);
  assert.ok(named.has("Body copy"), "missing 'Body copy' text node");
  assert.equal(named.get("Body copy").create.size, 20);
  // Sage accent dot.
  const accent = named.get("Accent");
  assert.ok(accent, "missing 'Accent' rect node");
  assert.deepEqual(fillOf(accent), paletteFill(BRAND_PALETTE.sage));
});

test("PRINT resolver seeds an espresso header bar, cream body block, two body paragraphs, and a burnt-orange footer accent on A4", async (t) => {
  const { BRAND_PALETTE, templateResolverFor } = await loadResolvers();
  const calls = installRecorder(t);
  // A4 @ 300dpi — matches the shipped print preset on
  // `HomePage.tsx::CREATE_OPTIONS` (`{ name: "A4", width: 2480, height:
  // 3508 }`). Mirrors `kcreate_core::node::standard_presets()`.
  // Running on the real shipped dimensions ensures the resolver's
  // proportional math (margin = aw * 0.06, headerHeight = ah * 0.16,
  // body block = ah * 0.5, footer = ah * 0.04) covers the production
  // geometry — not a half-resolution stand-in — so any rounding /
  // overflow regression at the actual size is caught here.
  const ctx = { x: 0, y: 0, width: 2480, height: 3508 };
  await templateResolverFor("print").apply(ctx);
  assertInsideArtboard(calls, ctx);
  const nodes = nodesFromCalls(calls);
  const named = new Map(
    [...nodes.values()]
      .map((n) => [nameOf(n), n])
      .filter(([n]) => n !== null),
  );
  const header = named.get("Header bar");
  assert.ok(header, "missing 'Header bar' rect node");
  assert.equal(header.create.kind, "createRect");
  // Header is espresso (cream text sits on top); the burnt-orange
  // strip lives at the FOOTER, not the header.
  assert.deepEqual(fillOf(header), paletteFill(BRAND_PALETTE.espresso));
  assert.ok(named.has("Document title"), "missing 'Document title'");
  assert.equal(named.get("Document title").create.size, 72);
  // Body block + at least the first body paragraph.
  const bodyBlock = named.get("Body block");
  assert.ok(bodyBlock, "missing 'Body block' rect node");
  assert.deepEqual(fillOf(bodyBlock), paletteFill(BRAND_PALETTE.cream));
  assert.ok(
    named.has("Body paragraph"),
    "missing 'Body paragraph' text node",
  );
  // Footer accent strip in burnt orange.
  const footer = named.get("Footer accent");
  assert.ok(footer, "missing 'Footer accent' rect node");
  assert.deepEqual(fillOf(footer), paletteFill(BRAND_PALETTE.burntOrange));
});

test("APP_UI resolver seeds an app-shell layout: background, left rail, header, header title, and three content tiles with headings", async (t) => {
  const { templateResolverFor } = await loadResolvers();
  const calls = installRecorder(t);
  await templateResolverFor("app-ui").apply(A1080);
  assertInsideArtboard(calls, A1080);
  const nodes = nodesFromCalls(calls);
  const names = new Set(
    [...nodes.values()].map((n) => nameOf(n)).filter(Boolean),
  );
  assert.ok(names.has("App background"), "missing 'App background'");
  assert.ok(names.has("Left rail"), "missing 'Left rail'");
  assert.ok(names.has("Header"), "missing 'Header' bar");
  assert.ok(names.has("Header title"), "missing 'Header title' text");
  // Content tiles 1–3 + each tile's heading.
  for (const i of [1, 2, 3]) {
    assert.ok(
      names.has(`Content tile ${i}`),
      `missing 'Content tile ${i}'`,
    );
    assert.ok(
      names.has(`Tile heading ${i}`),
      `missing 'Tile heading ${i}'`,
    );
  }
});

test("PHOTO resolver clamps the drop zone to the SHORT side so it fits non-square artboards", async (t) => {
  const { BRAND_PALETTE, templateResolverFor } = await loadResolvers();
  // Landscape 3000×2000 — the post-fix behaviour should keep the
  // drop zone fully inside the artboard. The pre-fix code would
  // have placed a 3000-wide drop zone on a 2000-tall canvas (1000px
  // of overflow vertically).
  const ctx = { x: 0, y: 0, width: 3000, height: 2000 };
  const calls = installRecorder(t);
  await templateResolverFor("photo").apply(ctx);
  assertInsideArtboard(calls, ctx);
  const nodes = nodesFromCalls(calls);
  const named = new Map(
    [...nodes.values()]
      .map((n) => [nameOf(n), n])
      .filter(([n]) => n !== null),
  );
  // Backdrop fills the whole canvas.
  const backdrop = named.get("Photo backdrop");
  assert.ok(backdrop, "missing 'Photo backdrop'");
  assert.equal(backdrop.create.w, ctx.width);
  assert.equal(backdrop.create.h, ctx.height);
  // Drop zone is a square <= min(w, h).
  const drop = named.get("Drop zone");
  assert.ok(drop, "missing 'Drop zone'");
  assert.equal(drop.create.w, drop.create.h, "drop zone must be square");
  assert.ok(
    drop.create.w <= Math.min(ctx.width, ctx.height),
    `drop zone side ${drop.create.w} exceeds shortSide ${Math.min(ctx.width, ctx.height)}`,
  );
  assert.deepEqual(fillOf(drop), paletteFill(BRAND_PALETTE.paper));
});

test("DECK resolver keeps two columns visible even on a tight 800×450 artboard", async (t) => {
  const { templateResolverFor } = await loadResolvers();
  // Half-height of the shipped 1920×1080 deck preset — would have
  // collapsed `colHeight` to zero pre-clamp.
  const ctx = { x: 0, y: 0, width: 800, height: 450 };
  const calls = installRecorder(t);
  await templateResolverFor("deck").apply(ctx);
  assertInsideArtboard(calls, ctx);
  const nodes = nodesFromCalls(calls);
  const named = new Map(
    [...nodes.values()]
      .map((n) => [nameOf(n), n])
      .filter(([n]) => n !== null),
  );
  const a = named.get("Column A");
  const b = named.get("Column B");
  assert.ok(a, "missing 'Column A'");
  assert.ok(b, "missing 'Column B'");
  assert.ok(a.create.h > 0, "Column A height must be positive");
  assert.ok(b.create.h > 0, "Column B height must be positive");
});

test("DEV_EXPORT resolver labels the artboard with its grid hint", async (t) => {
  const { BRAND_PALETTE, templateResolverFor } = await loadResolvers();
  // The shipped dev-export preset is 512×512.
  const ctx = { x: 0, y: 0, width: 512, height: 512 };
  const calls = installRecorder(t);
  await templateResolverFor("dev-export").apply(ctx);
  assertInsideArtboard(calls, ctx);
  const nodes = nodesFromCalls(calls);
  const named = new Map(
    [...nodes.values()]
      .map((n) => [nameOf(n), n])
      .filter(([n]) => n !== null),
  );
  // Backdrop + icon body + notch + caption.
  assert.ok(named.has("Icon backdrop"), "missing 'Icon backdrop'");
  const body = named.get("Icon body");
  assert.ok(body, "missing 'Icon body'");
  assert.deepEqual(fillOf(body), paletteFill(BRAND_PALETTE.burntOrange));
  assert.ok(named.has("Icon notch"), "missing 'Icon notch'");
  const caption = named.get("Spec caption");
  assert.ok(caption, "missing 'Spec caption'");
  // The caption must mention the grid size + the artboard dimensions.
  assert.ok(
    caption.create.body.includes("512"),
    `caption '${caption.create.body}' missing artboard width`,
  );
  assert.ok(
    caption.create.body.includes("64"),
    `caption '${caption.create.body}' missing 64px grid hint`,
  );
});

test("Resolvers honour a non-zero artboard Y offset (second + later artboards)", async (t) => {
  // The bridge offsets every artboard after the first; the resolver
  // reads `ay` off `artboard.list()` and forwards it through. This
  // test feeds a Y-offset ctx and verifies the seeded nodes land at
  // the offset, not at world (0, 0). Mirrors the regression
  // protected by `DECK_RESOLVER`'s "Two-column body" comment.
  const { templateResolverFor } = await loadResolvers();
  const ctx = { x: 0, y: 2200, width: 1920, height: 1080 };
  const calls = installRecorder(t);
  await templateResolverFor("deck").apply(ctx);
  assertInsideArtboard(calls, ctx);
  // No node may land above `ay` (would be visible on the wrong slide).
  for (const c of calls) {
    if (c.kind !== "createRect" && c.kind !== "createText") continue;
    assert.ok(
      c.y >= ctx.y - 1,
      `node ${c.id} at y=${c.y} below artboard top ${ctx.y}`,
    );
  }
});

// Item 2 of the PR #31 follow-up work: every resolver must use the
// `canvas.createNodes` batch surface — not the per-item
// `createRect`/`createText` helpers. Asserting this directly
// guarantees that the IPC-traffic improvement (12+ round-trips → 1)
// can't silently regress in a future refactor that accidentally
// re-introduces sequential awaits.
test("every resolver issues exactly one canvas.createNodes batch (no per-item createRect/createText calls)", async (t) => {
  const { templateResolverFor } = await loadResolvers();
  // Use a context per resolver so each test exercises its actual
  // shipped artboard preset (matches the size the bridge hands to
  // the resolver from `HomePage.tsx` after the user clicks a card).
  const cases = [
    { id: "brand", ctx: { x: 0, y: 0, width: 1024, height: 1024 } },
    { id: "social", ctx: { x: 0, y: 0, width: 1080, height: 1080 } },
    { id: "print", ctx: { x: 0, y: 0, width: 2480, height: 3508 } },
    { id: "app-ui", ctx: { x: 0, y: 0, width: 1440, height: 900 } },
    { id: "photo", ctx: { x: 0, y: 0, width: 2048, height: 2048 } },
    { id: "deck", ctx: { x: 0, y: 0, width: 1920, height: 1080 } },
    { id: "dev-export", ctx: { x: 0, y: 0, width: 512, height: 512 } },
  ];
  for (const { id, ctx } of cases) {
    const calls = installRecorder(t);
    await templateResolverFor(id).apply(ctx);
    // The batch fixture exposes `batchCalls` for this assertion;
    // every resolver must call `canvas.createNodes` exactly once and
    // never reach for the per-item `createRect`/`createText` helpers
    // (those still exist as a backward-compat surface on the
    // bridge, but the resolvers themselves only go through the
    // batch).
    assert.equal(
      calls.batchCalls.length,
      1,
      `${id} resolver should issue exactly one batch (got ${calls.batchCalls.length})`,
    );
    assert.ok(
      calls.batchCalls[0].items.length > 0,
      `${id} resolver batch must contain at least one item`,
    );
    // No top-level updateNode calls (fills + names go onto the
    // batch items directly, not through a follow-up updateNode).
    // The batch fixture re-emits an updateNode entry per item with
    // fill/name set, so we have to scope the count to entries the
    // batch did NOT synthesize. Easiest: count direct
    // updateNode calls before the batch ran by inspecting recorder
    // sequence — every updateNode in this fixture comes from the
    // batch synth path (the resolvers never invoke
    // document.updateNode directly anymore), so the count of
    // updateNode entries must equal the count of batch items that
    // carried fill or name. Verify that here.
    const updateCount = calls.filter((c) => c.kind === "updateNode").length;
    const batchItems = calls.batchCalls[0].items;
    const expectedUpdateCount = batchItems.filter(
      (it) => it.fill !== undefined || it.name !== undefined,
    ).length;
    assert.equal(
      updateCount,
      expectedUpdateCount,
      `${id}: expected ${expectedUpdateCount} synthesized updateNode entries (one per batch item carrying fill/name), got ${updateCount}`,
    );
  }
});
