#!/usr/bin/env node
// Build script for the KCreate companion .kcz bundle.
//
//   1. Bundle src/entry.tsx + dependencies into a single ESM file
//      using esbuild (no platform globals, no Node built-ins).
//   2. Validate src/manifest.json against the Zod schema in
//      src/manifest.ts (transpiled on the fly so the build script
//      is self-contained — no compiled `dist/` needed).
//   3. Stage the bundle + manifest into `dist/staging/`. The sign
//      step (scripts/sign.mjs) ZIPs that into a real .kcz.
import { build } from "esbuild";
import { mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const ROOT = resolve(__dirname, "..");
const SRC = resolve(ROOT, "src");
const DIST = resolve(ROOT, "dist");
const STAGING = resolve(DIST, "staging");

async function clean() {
  await rm(DIST, { recursive: true, force: true });
  await mkdir(STAGING, { recursive: true });
}

async function bundleEntry() {
  await build({
    entryPoints: [resolve(SRC, "entry.tsx")],
    outfile: resolve(STAGING, "panel.js"),
    bundle: true,
    format: "esm",
    target: ["es2022"],
    platform: "browser",
    minify: false,
    sourcemap: false,
    legalComments: "none",
    // The host injects `globalThis.__kchatHost`; do not let the
    // bundler think it can resolve it at build time.
    external: [],
    define: {
      "process.env.NODE_ENV": '"production"',
    },
    jsx: "automatic",
    loader: { ".tsx": "tsx", ".ts": "ts" },
  });
}

async function validateManifest() {
  const manifestPath = resolve(SRC, "manifest.json");
  const raw = await readFile(manifestPath, "utf8");
  const parsed = JSON.parse(raw);
  // The schema lives in `src/manifest.ts`. We compile that file to
  // a temporary JS module via esbuild so this script never depends
  // on a pre-built `dist/`.
  const schemaBuild = await build({
    entryPoints: [resolve(SRC, "manifest.ts")],
    bundle: true,
    format: "esm",
    target: ["es2022"],
    platform: "neutral",
    write: false,
    legalComments: "none",
    external: [],
    mainFields: ["module", "main"],
    conditions: ["import", "default"],
  });
  const code = schemaBuild.outputFiles[0]?.text;
  if (!code) {
    throw new Error("failed to compile src/manifest.ts");
  }
  // Use a data URL ESM import so we can call `parseManifest`
  // without writing a temp file.
  const dataUrl = `data:text/javascript;base64,${Buffer.from(code).toString(
    "base64",
  )}`;
  const mod = await import(dataUrl);
  const validated = mod.parseManifest(parsed);
  await writeFile(
    resolve(STAGING, "manifest.json"),
    `${JSON.stringify(validated, null, 2)}\n`,
    "utf8",
  );
}

async function main() {
  await clean();
  await bundleEntry();
  await validateManifest();
  console.log(`[kcreate-companion] staged bundle at ${STAGING}`);
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
