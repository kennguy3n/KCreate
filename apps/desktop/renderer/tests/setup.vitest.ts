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

beforeEach(() => {
  installKcreateStub();
});

afterEach(() => {
  cleanup();
});
