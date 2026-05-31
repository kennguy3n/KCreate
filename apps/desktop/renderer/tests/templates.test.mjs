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
async function loadResolvers() {
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
}

/// Build a recording stub for `window.kcreate` and install it as
/// the global `window` so the resolver can call into it without an
/// Electron bridge. The stub assigns a deterministic node id of the
/// form `n-N` (N = 0, 1, 2, …) to every `createRect` / `createText`
/// call so the follow-up `updateNode` calls can be correlated with
/// the node they paint.
function installRecorder() {
  const calls = [];
  let nextId = 0;
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
    },
    document: {
      async updateNode(id, props) {
        calls.push({ kind: "updateNode", id, props });
      },
    },
  };
  // jsdom isn't pulled in for these tests; assign a minimal stand-in
  // for `window` so `window.kcreate.*` resolves. The resolver never
  // touches anything else on `window`.
  globalThis.window = { kcreate: recorder };
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

test("BRAND resolver seeds title, tagline, four palette swatches with labels, and a logo placeholder + caption", async () => {
  const { BRAND_PALETTE, templateResolverFor } = await loadResolvers();
  const calls = installRecorder();
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

test("SOCIAL resolver seeds a cream background, burnt-orange headline band, headline + body copy, and sage accent", async () => {
  const { BRAND_PALETTE, templateResolverFor } = await loadResolvers();
  const calls = installRecorder();
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

test("PRINT resolver seeds an espresso header bar, cream body block, two body paragraphs, and a burnt-orange footer accent on A4", async () => {
  const { BRAND_PALETTE, templateResolverFor } = await loadResolvers();
  const calls = installRecorder();
  // A4 @ 150dpi-ish — the shipped print preset.
  const ctx = { x: 0, y: 0, width: 1240, height: 1754 };
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

test("APP_UI resolver seeds an app-shell layout: background, left rail, header, header title, and three content tiles with headings", async () => {
  const { templateResolverFor } = await loadResolvers();
  const calls = installRecorder();
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

test("PHOTO resolver clamps the drop zone to the SHORT side so it fits non-square artboards", async () => {
  const { BRAND_PALETTE, templateResolverFor } = await loadResolvers();
  // Landscape 3000×2000 — the post-fix behaviour should keep the
  // drop zone fully inside the artboard. The pre-fix code would
  // have placed a 3000-wide drop zone on a 2000-tall canvas (1000px
  // of overflow vertically).
  const ctx = { x: 0, y: 0, width: 3000, height: 2000 };
  const calls = installRecorder();
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

test("DECK resolver keeps two columns visible even on a tight 800×450 artboard", async () => {
  const { templateResolverFor } = await loadResolvers();
  // Half-height of the shipped 1920×1080 deck preset — would have
  // collapsed `colHeight` to zero pre-clamp.
  const ctx = { x: 0, y: 0, width: 800, height: 450 };
  const calls = installRecorder();
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

test("DEV_EXPORT resolver labels the artboard with its grid hint", async () => {
  const { BRAND_PALETTE, templateResolverFor } = await loadResolvers();
  // The shipped dev-export preset is 512×512.
  const ctx = { x: 0, y: 0, width: 512, height: 512 };
  const calls = installRecorder();
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

test("Resolvers honour a non-zero artboard Y offset (second + later artboards)", async () => {
  // The bridge offsets every artboard after the first; the resolver
  // reads `ay` off `artboard.list()` and forwards it through. This
  // test feeds a Y-offset ctx and verifies the seeded nodes land at
  // the offset, not at world (0, 0). Mirrors the regression
  // protected by `DECK_RESOLVER`'s "Two-column body" comment.
  const { templateResolverFor } = await loadResolvers();
  const ctx = { x: 0, y: 2200, width: 1920, height: 1080 };
  const calls = installRecorder();
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
