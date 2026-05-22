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
  documentInspectNode(nodeId: string): string;
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
  lowResourceModeGet(): boolean;
  lowResourceModeSet(enabled: boolean): void;
  resourceLimits(): string;
  llmStart(modelPath: string): number;
  llmStop(): void;
  llmStatus(): string;
  // LLM completion calls return Promises because the underlying
  // `AsyncTask` runs the blocking HTTP round-trip on N-API's libuv
  // thread pool instead of the Electron main loop. See the
  // `LlmChatTask` family in `crates/kcreate_bridge/src/lib.rs`.
  llmChat(
    messagesJson: string,
    maxTokens: number,
    temperature: number,
  ): Promise<string>;
  llmSuggestForSelection(): Promise<string>;
  aiSuggestLayerNames(): Promise<string>;
  aiExtractDesignTokens(): Promise<string>;
  aiCheckAccessibility(): Promise<string>;
  exportSvg(nodeIds: string[], optionsJson: string): string;
  exportPng(outputPath: string, optionsJson: string): number;
  exportPdf(outputPath: string, optionsJson: string): number;
  exportWebp(outputPath: string, optionsJson: string): number;
  exportJpeg(outputPath: string, optionsJson: string): number;

  // Canvas: scene sync, hit testing, selection, shape creation, move
  documentSyncScene(): void;
  canvasHitTest(
    x: number,
    y: number,
    panX: number,
    panY: number,
    zoom: number,
  ): string | null;
  documentSetSelection(nodeIds: string[]): void;
  documentGetSelection(): string[];
  documentClearSelection(): void;
  documentImportImage(parentId: string | null, filePath: string): string;
  canvasCreateRect(
    parentId: string | null,
    x: number,
    y: number,
    w: number,
    h: number,
  ): string;
  canvasCreateEllipse(
    parentId: string | null,
    cx: number,
    cy: number,
    rx: number,
    ry: number,
  ): string;
  canvasCreateLine(
    parentId: string | null,
    x1: number,
    y1: number,
    x2: number,
    y2: number,
  ): string;
  canvasCreateText(
    parentId: string | null,
    x: number,
    y: number,
    text: string,
    fontFamily: string,
    fontSize: number,
  ): string;
  canvasMoveNode(nodeId: string, dx: number, dy: number): void;

  // AI Assist
  aiRemoveBackground(nodeId: string): string;
  aiGetActionLog(): string;

  // Local MCP server (loopback only, opt-in)
  mcpStart(): number;
  mcpStop(): void;
  mcpIsRunning(): boolean;

  // Design tokens / brand kits / export presets (Task 19)
  designTokensGet(): string;
  designTokensSet(tokensJson: string): void;
  brandKitCreate(name: string): string;
  brandKitUpdate(kitJson: string): void;
  brandKitList(): string;
  brandKitDelete(kitId: string): boolean;
  exportPresetCreate(name: string, format: string, scale: number): string;
  exportPresetList(): string;
  exportPresetDelete(presetId: string): boolean;

  // Artboards (Phase 1, Block A)
  artboardCreate(
    pageId: string | null,
    name: string,
    width: number,
    height: number,
  ): string;
  artboardList(): string;
  artboardDuplicate(artboardId: string): string;
  artboardResize(artboardId: string, width: number, height: number): void;
  artboardPresets(): string;

  // Components (Phase 1, Block B)
  componentCreateFromSelection(nodeIds: string[], name: string): string;
  componentList(): string;
  componentInstantiate(
    componentId: string,
    parentId: string | null,
    x: number,
    y: number,
  ): string;
  componentAddVariant(componentId: string, name: string): string;
  componentSwitchVariant(nodeId: string, variantId: string): void;
  componentDetach(nodeId: string): void;

  // Auto-layout (Phase 1, Block C)
  layoutSetFlex(nodeId: string, layoutJson: string): void;
  layoutSetGrid(nodeId: string, layoutJson: string): void;
  layoutRecompute(nodeId: string): void;
  layoutConvertToFrame(nodeId: string): void;
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
