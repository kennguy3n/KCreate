// Loads the kcreate_bridge native N-API addon. The Cargo build produces
// `libkcreate_bridge.{dylib,so}` (or `kcreate_bridge.dll`) under
// `target/<profile>/`. Node only `require()`s files with a `.node`
// extension, so we use `process.dlopen` directly against the raw cdylib
// path. This avoids any copy step at build time.

import { createRequire } from "node:module";
import * as path from "node:path";
import * as process from "node:process";

type FrameInfoSnake = {
  frame_id: number;
  width: number;
  height: number;
  byte_length: number;
};

type AcquiredFrameSnake = {
  frame_id: number;
  width: number;
  height: number;
  bytes: Uint8Array;
};

export interface Bridge {
  rendererInit(
    width: number,
    height: number,
  ): { tier: string; width: number; height: number };
  rendererShutdown(): void;
  rendererResize(width: number, height: number): void;
  rendererSetViewport(panX: number, panY: number, zoom: number): void;
  rendererInvalidate(
    x: number | null,
    y: number | null,
    width: number | null,
    height: number | null,
  ): void;
  rendererRender(sceneJson: string): number;
  rendererGetFrame(): Uint8Array | null;
  rendererFrameInfo(): FrameInfoSnake | null;
  rendererAcquireFrame(): AcquiredFrameSnake | null;
}

function bridgeBinaryPath(): string {
  // Allow override for development. In production, the bridge is copied
  // alongside the packaged app via electron-builder's `extraResources`.
  const override = process.env["KCREATE_BRIDGE_PATH"];
  if (override) return override;

  const profile = process.env["KCREATE_BRIDGE_PROFILE"] ?? "debug";
  const targetRoot = path.resolve(__dirname, "..", "..", "..", "..", "target");
  const libDir = path.join(targetRoot, profile);
  const platform = process.platform;
  const name =
    platform === "win32"
      ? "kcreate_bridge.dll"
      : platform === "darwin"
        ? "libkcreate_bridge.dylib"
        : "libkcreate_bridge.so";
  return path.join(libDir, name);
}

export function loadBridge(): Bridge {
  const binaryPath = bridgeBinaryPath();
  // `process.dlopen` lets us load a raw shared library that does not end
  // in `.node`. The Node.js loader populates `module.exports` with the
  // napi-rs addon's exports.
  const moduleStub: NodeJS.Module = {
    exports: {},
    require: createRequire(__filename),
    id: binaryPath,
    filename: binaryPath,
    loaded: false,
    children: [],
    paths: [],
    isPreloading: false,
    path: path.dirname(binaryPath),
    parent: null,
  };
  process.dlopen(moduleStub, binaryPath);
  return moduleStub.exports as Bridge;
}
