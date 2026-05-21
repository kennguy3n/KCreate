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

type ProjectInfoSnake = {
  id: string;
  name: string;
  path: string;
  created_at: string;
  modified_at: string;
};

type NodeInfoSnake = {
  id: string;
  node_type: string;
  parent_id: string | null;
  children: string[];
  name: string;
  visible: boolean;
  locked: boolean;
};

type RuntimeStatusSnake = {
  device_tier: string;
  gpu_available: boolean;
  gpu_name: string | null;
  platform: string;
  total_ram_mb: number;
};

type DocumentStatusSnake = {
  node_count: number;
  can_undo: boolean;
  can_redo: boolean;
  undo_depth: number;
  redo_depth: number;
};

export type {
  ProjectInfoSnake,
  NodeInfoSnake,
  RuntimeStatusSnake,
  DocumentStatusSnake,
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

  // Document / project lifecycle
  projectCreate(name: string, dir: string): ProjectInfoSnake;
  projectOpen(dir: string): ProjectInfoSnake;
  projectSave(): void;
  projectClose(): void;
  projectGetInfo(): ProjectInfoSnake | null;
  documentGetTree(): NodeInfoSnake[];
  documentCreateNode(
    nodeType: string,
    parentId: string | null,
    propsJson: string,
  ): string;
  documentUpdateNode(nodeId: string, changesJson: string): void;
  documentDeleteNode(nodeId: string): void;
  documentUndo(): string[] | null;
  documentRedo(): string[] | null;
  documentStatus(): DocumentStatusSnake | null;
  runtimeStatus(): RuntimeStatusSnake;
  exportSvg(nodeIds: string[], optionsJson: string): string;
  exportPng(
    nodeIds: string[],
    outputPath: string,
    optionsJson: string,
  ): number;
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
