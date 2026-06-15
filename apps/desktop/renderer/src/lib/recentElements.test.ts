// recentElements store tests (H3).
//
// The store is the single source of truth for the Elements panel's
// "Recently used" row: the host writes to it on a *successful* insert,
// the panel reads it reactively. These tests assert:
//   * an empty / missing list reads back as a STABLE empty reference
//     (so it is safe as a `useSyncExternalStore` snapshot);
//   * recording prepends, dedupes, caps, and persists to localStorage;
//   * the snapshot reference only changes when the value changes;
//   * subscribers are notified on record and stop after unsubscribe;
//   * a blank id is a no-op.

import { describe, it, expect, beforeEach } from "vitest";

import {
  getRecentElementIds,
  recordRecentElement,
  subscribeRecentElements,
} from "./recentElements";

const RECENT_KEY = "kcreate.elements.recent.v1";

describe("recentElements", () => {
  beforeEach(() => {
    window.localStorage.clear();
  });

  it("reads an empty, stable list when nothing is stored", () => {
    const a = getRecentElementIds();
    const b = getRecentElementIds();
    expect(a).toEqual([]);
    // Same reference while unchanged — no useSyncExternalStore churn.
    expect(a).toBe(b);
  });

  it("records newest-first and persists to localStorage", () => {
    recordRecentElement("circle");
    recordRecentElement("square");
    expect(getRecentElementIds()).toEqual(["square", "circle"]);
    expect(JSON.parse(window.localStorage.getItem(RECENT_KEY) ?? "[]")).toEqual([
      "square",
      "circle",
    ]);
  });

  it("dedupes by moving a re-inserted id back to the head", () => {
    recordRecentElement("a");
    recordRecentElement("b");
    recordRecentElement("a");
    expect(getRecentElementIds()).toEqual(["a", "b"]);
  });

  it("caps the list at 12 entries", () => {
    for (let i = 0; i < 20; i += 1) recordRecentElement(`id-${i}`);
    const ids = getRecentElementIds();
    expect(ids).toHaveLength(12);
    // Newest first → the last 12 ids recorded, head = most recent.
    expect(ids[0]).toBe("id-19");
    expect(ids[11]).toBe("id-8");
  });

  it("hands back a new reference only when the value changes", () => {
    const before = getRecentElementIds();
    recordRecentElement("x");
    const after = getRecentElementIds();
    expect(after).not.toBe(before);
    // Reading again without a change keeps the same reference.
    expect(getRecentElementIds()).toBe(after);
  });

  it("notifies subscribers on record and stops after unsubscribe", () => {
    let hits = 0;
    const unsubscribe = subscribeRecentElements(() => {
      hits += 1;
    });
    recordRecentElement("one");
    expect(hits).toBe(1);
    unsubscribe();
    recordRecentElement("two");
    expect(hits).toBe(1);
  });

  it("ignores a blank id", () => {
    recordRecentElement("");
    expect(getRecentElementIds()).toEqual([]);
    expect(window.localStorage.getItem(RECENT_KEY)).toBeNull();
  });
});
