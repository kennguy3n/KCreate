// Bundles the Electron main process and preload scripts into the flat layout
// the runtime expects:
//
//   main/dist/main.js            <- package.json "main"
//   main/dist/plugin-preload.js  <- jsPanelPreloadPath() (main.ts)
//   preload/dist/preload.js      <- BrowserWindow webPreferences.preload (main.ts)
//
// `main.ts` and `bridge.ts` compute sibling/parent paths from `__dirname`
// (preload, renderer/dist/index.html, the Rust `target/` dir), so the emitted
// files MUST sit directly in `main/dist` / `preload/dist`. A plain `tsc` build
// with `rootDir: "."` + `include: shared/**` nests the output under
// `main/dist/main/src/…`, which breaks every one of those `__dirname` joins and
// stops the app from booting. Bundling pins the layout and inlines the shared
// wire types. Type checking stays in `pnpm typecheck` (tsc --noEmit).
import { build } from "esbuild";
import { fileURLToPath } from "node:url";
import { rmSync } from "node:fs";
import path from "node:path";

const here = path.dirname(fileURLToPath(import.meta.url));

const shared = {
  bundle: true,
  platform: "node",
  format: "cjs",
  target: "node20",
  // Electron is provided by the runtime; node builtins are external by default
  // on `platform: "node"`.
  external: ["electron"],
  sourcemap: true,
  logLevel: "info",
};

async function run() {
  // Start from a clean slate so no stale nested `tsc` output lingers.
  rmSync(path.join(here, "main", "dist"), { recursive: true, force: true });
  rmSync(path.join(here, "preload", "dist"), { recursive: true, force: true });

  await build({
    ...shared,
    absWorkingDir: here,
    outdir: "main/dist",
    entryPoints: {
      main: "main/src/main.ts",
      "plugin-preload": "main/src/plugin-preload.ts",
    },
  });

  await build({
    ...shared,
    absWorkingDir: here,
    outdir: "preload/dist",
    entryPoints: {
      preload: "preload/src/preload.ts",
    },
  });
}

run().catch((err) => {
  console.error(err);
  process.exit(1);
});
