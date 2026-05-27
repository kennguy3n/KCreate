// Validates that:
//   1. The shipped `src/manifest.json` parses against the Zod
//      schema in `src/manifest.ts`.
//   2. Required slot ids + procedure ids are declared.
//   3. Schema rejects malformed manifests (extra-key / wrong-shape).
//
// We compile `src/manifest.ts` to ESM via esbuild and import the
// resulting code through a data: URL so the tests run without a
// pre-built `dist/`. This matches the build script's strategy and
// keeps the tests honest against the same parser the build uses.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";

const ROOT = dirname(dirname(fileURLToPath(import.meta.url)));

async function loadParser() {
  const result = await build({
    entryPoints: [resolve(ROOT, "src/manifest.ts")],
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
    throw new Error("failed to compile src/manifest.ts");
  }
  const dataUrl = `data:text/javascript;base64,${Buffer.from(code).toString(
    "base64",
  )}`;
  return import(dataUrl);
}

test("ships a valid manifest that the schema accepts", async () => {
  const { parseManifest } = await loadParser();
  const raw = await readFile(resolve(ROOT, "src/manifest.json"), "utf8");
  const parsed = parseManifest(JSON.parse(raw));
  assert.equal(parsed.manifestVersion, 1);
  assert.equal(parsed.identity.id, "kcreate.companion");
  assert.ok(
    parsed.procedures.some((p) => p.id === "kchat.post_message"),
    "post_message procedure must be declared so the panel can share invites",
  );
  assert.ok(
    parsed.contributes.views.some(
      (v) => v.slot === "outer-rightbar.community-context",
    ),
    "must contribute a view to the community-context slot",
  );
  assert.ok(
    parsed.contributes.deeplinks.some((d) => d.scheme === "kcreate"),
    "must claim the kcreate:// deeplink scheme",
  );
});

test("rejects a manifest with a malformed publisherPublicKey", async () => {
  const { parseManifest } = await loadParser();
  const raw = await readFile(resolve(ROOT, "src/manifest.json"), "utf8");
  const doc = JSON.parse(raw);
  doc.identity.publisherPublicKey = "too-short";
  assert.throws(() => parseManifest(doc), /publisherPublicKey/);
});

test("rejects an unknown slot id", async () => {
  const { parseManifest } = await loadParser();
  const raw = await readFile(resolve(ROOT, "src/manifest.json"), "utf8");
  const doc = JSON.parse(raw);
  doc.contributes.views[0].slot = "outer-rightbar.unknown-slot";
  assert.throws(() => parseManifest(doc));
});

test("rejects an unknown procedure category", async () => {
  const { parseManifest } = await loadParser();
  const raw = await readFile(resolve(ROOT, "src/manifest.json"), "utf8");
  const doc = JSON.parse(raw);
  doc.procedures[0].category = "delete";
  assert.throws(() => parseManifest(doc));
});

test("requires at least one view contribution", async () => {
  const { parseManifest } = await loadParser();
  const raw = await readFile(resolve(ROOT, "src/manifest.json"), "utf8");
  const doc = JSON.parse(raw);
  doc.contributes.views = [];
  assert.throws(() => parseManifest(doc));
});
