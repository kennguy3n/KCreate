// @vitest-environment node
//
// I1 — packaging path resolution for the Rust napi cdylib.
//
// `bridgeBinaryPath` is the single seam that lets ONE main-process build
// find the bridge in three layouts: a packaged app (under
// `process.resourcesPath`), a dev checkout (under `target/<profile>`),
// and an explicit override (`KCREATE_BRIDGE_PATH`, used by the e2e
// harness and the packaged-launch proof). A regression here means the
// shipped app can't load its renderer at all, so the precedence and the
// platform-specific filenames are pinned below without a real dlopen.

import path from "node:path";

import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { bridgeBinaryName, bridgeBinaryPath } from "./bridge";

const ENV_KEYS = [
  "KCREATE_BRIDGE_PATH",
  "KCREATE_BRIDGE_PROFILE",
] as const;

describe("bridgeBinaryName", () => {
  it("maps each platform to its cdylib filename", () => {
    expect(bridgeBinaryName("linux")).toBe("libkcreate_bridge.so");
    expect(bridgeBinaryName("darwin")).toBe("libkcreate_bridge.dylib");
    expect(bridgeBinaryName("win32")).toBe("kcreate_bridge.dll");
  });
});

describe("bridgeBinaryPath", () => {
  const saved: Record<string, string | undefined> = {};

  beforeEach(() => {
    for (const key of ENV_KEYS) {
      saved[key] = process.env[key];
      delete process.env[key];
    }
  });

  afterEach(() => {
    for (const key of ENV_KEYS) {
      if (saved[key] === undefined) delete process.env[key];
      else process.env[key] = saved[key];
    }
  });

  it("honours the KCREATE_BRIDGE_PATH override above everything else", () => {
    process.env["KCREATE_BRIDGE_PATH"] = "/tmp/custom/libkcreate_bridge.so";
    // Override wins even for a packaged context.
    expect(
      bridgeBinaryPath({ isPackaged: true, resourcesPath: "/opt/app/resources" }),
    ).toBe("/tmp/custom/libkcreate_bridge.so");
    expect(bridgeBinaryPath()).toBe("/tmp/custom/libkcreate_bridge.so");
  });

  it("resolves under resourcesPath/bridge when packaged", () => {
    const resourcesPath = "/opt/KCreate/resources";
    expect(bridgeBinaryPath({ isPackaged: true, resourcesPath })).toBe(
      path.join(resourcesPath, "bridge", bridgeBinaryName()),
    );
  });

  it("resolves under target/<profile> in a dev checkout", () => {
    const dev = bridgeBinaryPath();
    expect(dev.endsWith(path.join("target", "debug", bridgeBinaryName()))).toBe(
      true,
    );
  });

  it("respects KCREATE_BRIDGE_PROFILE for the dev target directory", () => {
    process.env["KCREATE_BRIDGE_PROFILE"] = "release";
    const dev = bridgeBinaryPath({ isPackaged: false, resourcesPath: "" });
    expect(
      dev.endsWith(path.join("target", "release", bridgeBinaryName())),
    ).toBe(true);
  });
});
