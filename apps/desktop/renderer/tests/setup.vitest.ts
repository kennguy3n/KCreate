// Global setup for the vitest test runner.
//
// Phase A4. Three things every component test needs:
//
//   1. `@testing-library/jest-dom` matchers (toBeInTheDocument, …)
//      registered onto vitest's `expect`.
//   2. A reset hook that unmounts whatever React mounted into the
//      jsdom root between tests so component identity / state
//      doesn't leak.
//   3. A minimal `window.kcreate` shim — every renderer entry point
//      reaches into the context-bridge surface on mount. We install
//      a recording stub here so individual tests can override only
//      the calls they care about (set `window.kcreate.x.y =
//      vi.fn(...)`) instead of building the entire bridge each
//      time.

import "@testing-library/jest-dom/vitest";
import { afterEach, beforeEach } from "vitest";
import { cleanup } from "@testing-library/react";

import { installKcreateStub } from "./helpers/kcreateStub";

// jsdom has no layout engine, so `Element.prototype.scrollIntoView`
// is intentionally unimplemented. Components that keep an active row
// in view on keyboard navigation (e.g. the command palette) call it
// from an effect; without a stub the effect throws under jsdom even
// though the behaviour is correct in Electron. Install a no-op so
// those effects run cleanly in tests. Guarded on `Element` so the
// pure-Node test files (which opt out of jsdom) don't trip over a
// missing global.
if (
  typeof Element !== "undefined" &&
  typeof Element.prototype.scrollIntoView !== "function"
) {
  Element.prototype.scrollIntoView = function scrollIntoView(): void {};
}

// Most test files run under jsdom and need a `window.kcreate`
// shim. A small set of pure-Node tests in `apps/desktop/main/src/`
// opt out of jsdom via `// @vitest-environment node` at the top
// of the file — they don't have a `window` global and don't
// interact with the bridge surface, so the stub install would
// throw a `ReferenceError: window is not defined`. Guarding both
// hooks on `typeof window !== "undefined"` lets the two
// environments share one setup file cleanly.
beforeEach(() => {
  if (typeof window !== "undefined") {
    installKcreateStub();
  }
});

afterEach(() => {
  if (typeof window !== "undefined") {
    cleanup();
  }
});
