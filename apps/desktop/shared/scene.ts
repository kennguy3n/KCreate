// Wire format for scenes sent across the N-API bridge to the Rust
// renderer. Must stay in lockstep with crates/kcreate_bridge/src/wire.rs.
//
// The renderer accepts this structure as a JSON string via
// `renderer_render(scene_json)`.

export type Color = [number, number, number, number]; // RGBA in [0, 1]

export interface Stroke {
  color: Color;
  width: number;
}

export interface ObjectStyle {
  fill: Color | null;
  stroke: Stroke | null;
}

export type ObjectKind =
  | { type: "rect"; x: number; y: number; width: number; height: number }
  | { type: "circle"; cx: number; cy: number; radius: number }
  | { type: "line"; x1: number; y1: number; x2: number; y2: number }
  | { type: "path"; commands: PathCommand[] };

export type PathCommand =
  | { type: "move"; x: number; y: number }
  | { type: "line"; x: number; y: number }
  | { type: "quad"; cx: number; cy: number; x: number; y: number }
  | {
      type: "cubic";
      c1x: number;
      c1y: number;
      c2x: number;
      c2y: number;
      x: number;
      y: number;
    }
  | { type: "close" };

export interface SceneObject {
  id: number;
  z: number;
  translation: [number, number];
  visible?: boolean;
  style: ObjectStyle;
  kind: ObjectKind;
}

export interface Scene {
  clear_color: Color;
  objects: SceneObject[];
}

export interface FrameInfo {
  frameId: number;
  width: number;
  height: number;
  byteLength: number;
}

/**
 * Latest published frame: bytes + metadata, atomically captured under
 * the renderer lock so the dimensions and pixel buffer cannot tear
 * across a resize.
 */
export interface AcquiredFrame {
  frameId: number;
  width: number;
  height: number;
  bytes: Uint8Array;
}

export interface RendererInfo {
  tier: string;
  width: number;
  height: number;
}

/**
 * Shape of the renderer bridge exposed to the renderer process via the
 * preload script. The preload uses `contextBridge.exposeInMainWorld` to
 * publish this on `window.kcreate.renderer`.
 */
export interface RendererBridge {
  /**
   * Initialize the renderer or, if it already exists, ensure it is sized
   * to `width × height` (resizing in place if needed). Calling `init`
   * again is safe and does not tear down the GPU device.
   */
  init(width: number, height: number): Promise<RendererInfo>;
  shutdown(): Promise<void>;
  resize(width: number, height: number): Promise<void>;
  setViewport(panX: number, panY: number, zoom: number): Promise<void>;
  invalidate(
    region?: { x: number; y: number; width: number; height: number },
  ): Promise<void>;
  render(scene: Scene): Promise<number>;
  /**
   * Returns a copy of the latest RGBA8 pixel buffer, or null if no frame
   * has been published yet.
   *
   * Prefer `acquireFrame()` in the per-frame hot path: it does the
   * equivalent of `frameInfo()` + `getFrame()` in a single IPC round
   * trip and cannot tear across a resize.
   */
  getFrame(): Promise<Uint8Array | null>;
  frameInfo(): Promise<FrameInfo | null>;
  /**
   * Atomic snapshot of the latest frame: bytes + dimensions captured in
   * a single locked read on the Rust side, and a single IPC round trip
   * across the process boundary. Returns null if no frame has been
   * published yet.
   */
  acquireFrame(): Promise<AcquiredFrame | null>;
}

/**
 * Project identity returned by `createProject` / `openProject`.
 */
export interface ProjectInfo {
  id: string;
  name: string;
  path: string;
  createdAt: string;
  modifiedAt: string;
}

/**
 * Flattened document node. The host UI builds the layer tree by
 * walking parent → children — it does not have to mirror the full
 * `kcreate_core::Node` payload.
 */
export interface NodeInfo {
  id: string;
  nodeType: string;
  parentId: string | null;
  children: string[];
  name: string;
  visible: boolean;
  locked: boolean;
}

/**
 * Static runtime + device snapshot for the home screen / model badge.
 */
export interface RuntimeStatus {
  deviceTier: string;
  gpuAvailable: boolean;
  gpuName: string | null;
  platform: string;
  totalRamMb: number;
}

/** Optional props accepted by `createNode`. */
export interface CreateNodeProps {
  name?: string;
  visible?: boolean;
  locked?: boolean;
  metadata?: Record<string, unknown>;
}

/** Optional changes accepted by `updateNode`. Only present fields are applied. */
export type UpdateNodeProps = CreateNodeProps;

/** SVG export options. `0` for width/height means "fit to content". */
export interface SvgExportOptions {
  width: number;
  height: number;
  includeMetadata: boolean;
  optimize: boolean;
}

/** PNG export options. `scale` multiplies width/height. */
export interface PngExportOptions {
  width: number;
  height: number;
  scale: number;
  background: Color | null;
}

/**
 * Editing-state snapshot for the host UI. Polled after every
 * mutation so undo/redo controls can reflect the actual operation
 * log state instead of heuristics like "are there any layers?".
 */
export interface DocumentStatus {
  nodeCount: number;
  canUndo: boolean;
  canRedo: boolean;
  undoDepth: number;
  redoDepth: number;
}

/**
 * Document + project lifecycle bridge. Exposed on
 * `window.kcreate.document`. All methods round-trip through the
 * Electron main process; the renderer never imports the native addon
 * directly.
 */
export interface DocumentBridge {
  createProject(name: string, dir: string): Promise<ProjectInfo>;
  openProject(dir: string): Promise<ProjectInfo>;
  saveProject(): Promise<void>;
  closeProject(): Promise<void>;
  getProjectInfo(): Promise<ProjectInfo | null>;

  getDocumentTree(): Promise<NodeInfo[]>;
  createNode(
    nodeType: string,
    parentId: string | null,
    props: CreateNodeProps,
  ): Promise<string>;
  updateNode(nodeId: string, changes: UpdateNodeProps): Promise<void>;
  deleteNode(nodeId: string): Promise<void>;
  undo(): Promise<string[] | null>;
  redo(): Promise<string[] | null>;

  /**
   * Snapshot of the open document's editing state, or `null` if no
   * project is open. Backed by the operation log so the host can
   * accurately enable/disable Undo / Redo without guessing.
   */
  status(): Promise<DocumentStatus | null>;
}

/** Runtime / device probe. */
export interface RuntimeBridge {
  status(): Promise<RuntimeStatus>;
  /**
   * OS-appropriate temporary directory, resolved by the Electron main
   * process via Node's `os.tmpdir()` so the renderer never has to
   * hard-code paths (`/tmp` on POSIX, `%TEMP%` on Windows, etc.). The
   * value is stable for the lifetime of the process.
   */
  tempDir(): Promise<string>;
}

/**
 * Export pipeline.
 *
 * `svg` walks the document graph directly, so it can render a
 * caller-specified node subset (`nodeIds` empty = whole document).
 * `png` rasterises the live renderer scene; the renderer's id space
 * (`u64`) is disjoint from the document graph's (`Uuid`), so
 * per-node PNG export waits on the Phase 1 document→scene translator
 * — the API surface omits a `nodeIds` parameter rather than accepting
 * one it would silently ignore.
 */
export interface ExportBridge {
  svg(nodeIds: string[], options: SvgExportOptions): Promise<string>;
  png(outputPath: string, options: PngExportOptions): Promise<number>;
}

declare global {
  interface Window {
    kcreate: {
      renderer: RendererBridge;
      document: DocumentBridge;
      runtime: RuntimeBridge;
      export: ExportBridge;
    };
  }
}
