// Vitest config for the React renderer.
//
// Phase A4. Hooks the existing Vite + React plugin pipeline (same JSX
// transform, same TS resolution) into a `jsdom`-backed test
// environment so we can mount real components with
// `@testing-library/react`. The pattern mirrors `apps/kchat-extension`
// (which builds its own RTL harness on top of `node --test` +
// esbuild) but uses vitest because the spec calls for it and because
// it gives us first-class watch mode for the renderer surface, which
// is by far the largest TS package in the repo.
//
// `globals: false` is deliberate — every test file imports `test`,
// `expect`, etc. explicitly. That keeps grep'ability and prevents
// accidental shadowing of types like `expect` from
// `@testing-library/jest-dom`.

import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";
import * as path from "node:path";

export default defineConfig({
  root: __dirname,
  plugins: [react()],
  test: {
    environment: "jsdom",
    globals: false,
    include: ["src/**/*.test.{ts,tsx}"],
    setupFiles: [path.resolve(__dirname, "tests/setup.vitest.ts")],
    css: false,
    // jsdom is single-threaded; keep the runner deterministic by
    // disabling parallel test files so window.kcreate / window
    // shims don't bleed across modules.
    fileParallelism: false,
    reporters: ["default"],
    clearMocks: true,
  },
});
