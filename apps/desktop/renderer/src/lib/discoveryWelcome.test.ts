// H1 — first-run discovery-welcome gating unit tests.
//
// The welcome overlay must show exactly once. These pin the
// localStorage gate: show when unseen, never again after dismiss.

import { describe, it, expect, beforeEach } from "vitest";

import {
  DISCOVERY_WELCOME_STORAGE_KEY,
  markDiscoveryWelcomeSeen,
  shouldShowDiscoveryWelcome,
} from "./discoveryWelcome";

describe("discoveryWelcome gating", () => {
  beforeEach(() => {
    window.localStorage.removeItem(DISCOVERY_WELCOME_STORAGE_KEY);
  });

  it("shows on a fresh install (no marker yet)", () => {
    expect(shouldShowDiscoveryWelcome()).toBe(true);
  });

  it("never shows again once marked seen", () => {
    expect(shouldShowDiscoveryWelcome()).toBe(true);
    markDiscoveryWelcomeSeen();
    expect(shouldShowDiscoveryWelcome()).toBe(false);
  });

  it("persists the marker under the documented key", () => {
    markDiscoveryWelcomeSeen();
    expect(
      window.localStorage.getItem(DISCOVERY_WELCOME_STORAGE_KEY),
    ).not.toBeNull();
  });
});
