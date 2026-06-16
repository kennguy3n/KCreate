// @vitest-environment node
//
// I1 — release-notes / progress projection. These pure helpers live in a
// separate module precisely so they can be tested without `electron`
// (which is unavailable in the vitest node environment). They convert
// electron-updater's loosely-typed payloads into the narrow `UpdateState`
// wire shapes the renderer consumes, so the in-app changelog stays
// readable and the progress bar gets exactly the four fields it needs.

import type {
  ProgressInfo,
  UpdateInfo as BuilderUpdateInfo,
} from "electron-updater";

import { describe, expect, it } from "vitest";

import { coalesceReleaseNotes, toWireInfo, toWireProgress } from "./updaterFormat";

describe("coalesceReleaseNotes", () => {
  it("returns null for null / empty / whitespace notes", () => {
    expect(coalesceReleaseNotes(null)).toBeNull();
    expect(coalesceReleaseNotes(undefined)).toBeNull();
    expect(coalesceReleaseNotes("")).toBeNull();
    expect(coalesceReleaseNotes("   \n  ")).toBeNull();
  });

  it("trims a plain string note", () => {
    expect(coalesceReleaseNotes("  Fixes a crash.  ")).toBe("Fixes a crash.");
  });

  it("joins a list newest-first, one release per paragraph", () => {
    const notes = [
      { version: "0.0.3", note: "Latest fix." },
      { version: "0.0.2", note: "Earlier fix." },
    ];
    expect(coalesceReleaseNotes(notes)).toBe(
      "v0.0.3\nLatest fix.\n\nv0.0.2\nEarlier fix.",
    );
  });

  it("drops empty entries and omits the version prefix when absent", () => {
    const notes = [
      { version: "", note: "Unversioned note." },
      { version: "0.0.2", note: "   " },
    ];
    expect(coalesceReleaseNotes(notes)).toBe("Unversioned note.");
  });
});

describe("toWireInfo", () => {
  it("projects version / releaseDate / coalesced notes", () => {
    const info = {
      version: "0.0.2",
      releaseDate: "2026-01-01T00:00:00.000Z",
      releaseNotes: "Shiny new build.",
      files: [],
      path: "",
      sha512: "",
    } as unknown as BuilderUpdateInfo;

    expect(toWireInfo(info)).toEqual({
      version: "0.0.2",
      releaseDate: "2026-01-01T00:00:00.000Z",
      releaseNotes: "Shiny new build.",
    });
  });

  it("normalises a missing releaseDate to null", () => {
    const info = {
      version: "0.0.2",
      releaseNotes: null,
      files: [],
      path: "",
      sha512: "",
    } as unknown as BuilderUpdateInfo;

    expect(toWireInfo(info)).toEqual({
      version: "0.0.2",
      releaseDate: null,
      releaseNotes: null,
    });
  });
});

describe("toWireProgress", () => {
  it("copies exactly the four fields the renderer needs", () => {
    const progress = {
      percent: 42.5,
      bytesPerSecond: 1024,
      transferred: 512,
      total: 2048,
      delta: 256,
    } as unknown as ProgressInfo;

    expect(toWireProgress(progress)).toEqual({
      percent: 42.5,
      bytesPerSecond: 1024,
      transferred: 512,
      total: 2048,
    });
  });
});
