// H1 — command-palette usage-history unit tests.
//
// The palette boosts recently / frequently run commands. These pin the
// persistence shape (privacy: only id + counters, nothing the user
// typed), the recency + frequency ranking maths, and the LRU cap that
// keeps a long-lived install bounded.

import { describe, it, expect, beforeEach } from "vitest";

import {
  COMMAND_HISTORY_STORAGE_KEY,
  loadCommandHistory,
  recordCommandUse,
} from "./commandPaletteHistory";

function clearHistory(): void {
  window.localStorage.removeItem(COMMAND_HISTORY_STORAGE_KEY);
}

describe("commandPaletteHistory", () => {
  beforeEach(clearHistory);

  it("starts with no recents and a zero boost for unknown ids", () => {
    const history = loadCommandHistory();
    expect(history.recentIds()).toEqual([]);
    expect(history.boost("never-used")).toBe(0);
  });

  it("records a run and persists only the id + counters (privacy)", () => {
    recordCommandUse("openTemplates", 1_000);
    const raw = window.localStorage.getItem(COMMAND_HISTORY_STORAGE_KEY);
    expect(raw).not.toBeNull();
    const parsed = JSON.parse(raw!) as Record<string, unknown>;
    expect(Object.keys(parsed)).toEqual(["openTemplates"]);
    expect(parsed.openTemplates).toEqual({ count: 1, lastUsed: 1_000 });
  });

  it("orders recentIds most-recently-used first", () => {
    recordCommandUse("a", 1_000);
    recordCommandUse("b", 2_000);
    recordCommandUse("c", 3_000);
    expect(loadCommandHistory().recentIds()).toEqual(["c", "b", "a"]);
    // Re-running an older command floats it back to the front.
    recordCommandUse("a", 4_000);
    expect(loadCommandHistory().recentIds()).toEqual(["a", "c", "b"]);
  });

  it("gives a positive boost to used commands and decays with age", () => {
    const now = 10_000_000;
    recordCommandUse("fresh", now);
    const history = loadCommandHistory();
    const freshBoost = history.boost("fresh", now);
    expect(freshBoost).toBeGreaterThan(0);
    // The same command evaluated 24h later (two half-lives) must score
    // strictly lower because the recency term has decayed.
    const laterBoost = history.boost("fresh", now + 24 * 60 * 60 * 1000);
    expect(laterBoost).toBeLessThan(freshBoost);
  });

  it("ranks a more-frequently-used command above a one-off at equal recency", () => {
    const now = 5_000_000;
    recordCommandUse("frequent", now);
    recordCommandUse("frequent", now);
    recordCommandUse("frequent", now);
    recordCommandUse("rare", now);
    const history = loadCommandHistory();
    expect(history.boost("frequent", now)).toBeGreaterThan(
      history.boost("rare", now),
    );
  });

  it("caps stored records and evicts the least-recently-used", () => {
    // Write 205 distinct commands with increasing timestamps; the cap
    // is 200, so the 5 oldest must be dropped on persist.
    for (let i = 0; i < 205; i += 1) {
      recordCommandUse(`cmd-${i}`, 1_000 + i);
    }
    const parsed = JSON.parse(
      window.localStorage.getItem(COMMAND_HISTORY_STORAGE_KEY)!,
    ) as Record<string, unknown>;
    expect(Object.keys(parsed).length).toBe(200);
    // The 5 oldest (cmd-0 … cmd-4) are gone; the newest survive.
    expect(parsed["cmd-0"]).toBeUndefined();
    expect(parsed["cmd-4"]).toBeUndefined();
    expect(parsed["cmd-204"]).toBeDefined();
  });

  it("ignores malformed persisted data and degrades to empty", () => {
    window.localStorage.setItem(COMMAND_HISTORY_STORAGE_KEY, "not json{");
    expect(loadCommandHistory().recentIds()).toEqual([]);
    window.localStorage.setItem(
      COMMAND_HISTORY_STORAGE_KEY,
      JSON.stringify({ bad: { count: "nope" } }),
    );
    expect(loadCommandHistory().recentIds()).toEqual([]);
  });
});
