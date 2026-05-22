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
  | { type: "path"; commands: PathCommand[] }
  | {
      /**
       * A raster image. `x`/`y`/`width`/`height` define the destination
       * rect in local coordinates; the image is scaled to fit. Pixel
       * data is RGBA8 base64-encoded — length is
       * `pixels_width × pixels_height × 4`.
       */
      type: "image";
      x: number;
      y: number;
      width: number;
      height: number;
      pixels_width: number;
      pixels_height: number;
      pixels_b64: string;
    }
  | {
      /**
       * A short string painted at `(x, y)`. The renderer resolves the
       * font via `kcreate_text` (`fontdb` + `rustybuzz`). The fill is
       * taken from `style.fill` on the parent object so styling stays
       * uniform across kinds.
       */
      type: "text";
      x: number;
      y: number;
      text: string;
      font_family: string;
      font_size: number;
    };

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
/**
 * Compact snapshot of a `ComponentInstance` payload. Present on a
 * `NodeInfo` only when the node is a `ComponentLayer` carrying a
 * parseable `component_instance` metadata blob. Mirrors
 * `kcreate_bridge::document::ComponentInstanceInfo`.
 */
export interface ComponentInstanceInfo {
  definitionId: string;
  activeVariantId: string;
  overrides: Record<string, unknown>;
}

export interface NodeInfo {
  id: string;
  nodeType: string;
  parentId: string | null;
  children: string[];
  name: string;
  visible: boolean;
  locked: boolean;
  componentInstance?: ComponentInstanceInfo;
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

/** Result of a scratch-project cleanup sweep. */
export interface ScratchCleanupResult {
  /** Number of `scratch-*.kstudio` entries inspected. */
  scanned: number;
  /** Number of entries successfully removed. */
  removed: number;
  /** Number of entries that errored during inspection/removal. */
  errors: number;
  /**
   * Number of entries skipped because they were claimed by another
   * live KCreate instance (via `.kclock` PID liveness check or the
   * "just-created" mtime grace window). Surfaced so Phase 1
   * observability (and the `runtime/devTools` panel) can distinguish
   * "successfully skipped a sibling" from "failed to delete".
   */
  skippedOwned: number;
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
  /**
   * Best-effort sweep of stale `scratch-*.kstudio` projects in the OS
   * temp directory. Owned by the host because (a) it needs filesystem
   * access the renderer doesn't have, and (b) the prefix/suffix and
   * base directory are fixed in host code to keep the API surface
   * narrow (the renderer cannot ask the host to delete arbitrary
   * paths). Never throws; reports per-entry errors via the result.
   */
  cleanupScratchProjects(): Promise<ScratchCleanupResult>;
}

/** PDF export options. `width_mm`/`height_mm` are the page size in mm. */
export interface PdfExportOptions {
  widthMm: number;
  heightMm: number;
  title: string;
}

/** WebP export options. `quality` is 0..100; `lossless` overrides quality. */
export interface WebpExportOptions {
  width: number;
  height: number;
  scale: number;
  quality: number;
  lossless: boolean;
  background: Color | null;
}

/** JPEG export options. `quality` is 0..100. */
export interface JpegExportOptions {
  width: number;
  height: number;
  scale: number;
  quality: number;
  background: Color | null;
}

/**
 * Export pipeline.
 *
 * `svg` walks the document graph directly, so it can render a
 * caller-specified node subset (`nodeIds` empty = whole document).
 * `png` / `webp` / `jpeg` rasterise the live renderer scene at the
 * requested dimensions. `pdf` walks the document graph and embeds
 * vector paths + raster images directly into the PDF page.
 */
export interface ExportBridge {
  svg(nodeIds: string[], options: SvgExportOptions): Promise<string>;
  png(outputPath: string, options: PngExportOptions): Promise<number>;
  pdf(outputPath: string, options: PdfExportOptions): Promise<number>;
  webp(outputPath: string, options: WebpExportOptions): Promise<number>;
  jpeg(outputPath: string, options: JpegExportOptions): Promise<number>;
}

/**
 * Canvas-side interactions. `hitTest` returns the document Uuid (as a
 * string) of the topmost node at the given screen-space point, or
 * `null` if the point misses every object.
 *
 * `hitTest` takes **screen-space** coordinates plus the current
 * viewport (pan + zoom) so the Rust side can do the screen→world
 * transform once, against the same `Viewport` math used by the
 * renderer presenter. Callers must NOT pre-transform `(x, y)` into
 * world space — the bridge owns that conversion (see
 * `crates/kcreate_bridge/src/document.rs::canvas_hit_test`).
 *
 * `setSelection` / `getSelection` / `clearSelection` manage the
 * selection set that the scene-sync layer paints as a highlight
 * overlay. The `createRect/Ellipse/Line/Text` and `moveNode` calls go
 * through the operation log and re-sync the scene on success.
 */
export interface CanvasBridge {
  syncScene(): Promise<void>;
  hitTest(
    screenX: number,
    screenY: number,
    panX: number,
    panY: number,
    zoom: number,
  ): Promise<string | null>;
  setSelection(nodeIds: string[]): Promise<void>;
  getSelection(): Promise<string[]>;
  clearSelection(): Promise<void>;
  importImage(
    parentId: string | null,
    filePath: string,
  ): Promise<string>;
  createRect(
    parentId: string | null,
    x: number,
    y: number,
    width: number,
    height: number,
  ): Promise<string>;
  createEllipse(
    parentId: string | null,
    cx: number,
    cy: number,
    rx: number,
    ry: number,
  ): Promise<string>;
  createLine(
    parentId: string | null,
    x1: number,
    y1: number,
    x2: number,
    y2: number,
  ): Promise<string>;
  createText(
    parentId: string | null,
    x: number,
    y: number,
    text: string,
    fontFamily: string,
    fontSize: number,
  ): Promise<string>;
  moveNode(nodeId: string, dx: number, dy: number): Promise<void>;
}

/**
 * AI Assist bridge. Phase 0 ships the threshold-based background
 * removal; `getActionLog()` returns the JSON-serialised newest-first
 * log so the AI Assist panel can show provenance ("model
 * `threshold-v0`, ran locally on CPU, affected N nodes").
 */
export interface AiBridge {
  removeBackground(nodeId: string): Promise<string>;
  getActionLog(): Promise<string>;
}

/**
 * Local MCP server bridge. The server is bound to `127.0.0.1` only
 * (no remote access ever) and is opt-in — disabled until
 * `start()` is called, and dropped on `stop()` or process exit.
 */
export interface McpBridge {
  start(): Promise<number>;
  stop(): Promise<void>;
  isRunning(): Promise<boolean>;
}

// -----------------------------------------------------------------------------
// Design tokens / brand kits / export presets (Task 19)
//
// These mirror the Rust types in `kcreate_core::project`. They are
// declared here so the renderer can typecheck the JSON crossing the
// bridge — every field name and casing matches the snake_case
// serde representation used on the Rust side, because the bridge
// hands the renderer a JSON string verbatim.
// -----------------------------------------------------------------------------

/** RGBA in [0, 1] — same as `kcreate_core::node::RgbaColor`. */
export interface RgbaColor {
  r: number;
  g: number;
  b: number;
  a: number;
}

/** Typography token mirroring `kcreate_core::project::TypographyToken`. */
export interface TypographyToken {
  font_family: string;
  font_weight: number;
  font_size: number;
  line_height: number;
  letter_spacing: number;
}

/** Drop-shadow token. */
export interface ShadowToken {
  offset_x: number;
  offset_y: number;
  blur: number;
  spread: number;
  color: RgbaColor;
}

/** Project-wide reusable tokens. Maps mirror `HashMap<String, T>`. */
export interface DesignTokens {
  colors: Record<string, RgbaColor>;
  typography: Record<string, TypographyToken>;
  spacing: Record<string, number>;
  radii: Record<string, number>;
  shadows: Record<string, ShadowToken>;
}

/** Named color inside a brand kit. */
export interface NamedColor {
  name: string;
  color: RgbaColor;
}

/** Font reference inside a brand kit. */
export interface FontRef {
  family: string;
  weight: number;
  italic: boolean;
  embedded_asset_id: string | null;
}

export type ExportFormat = "png" | "svg" | "pdf" | "webp" | "jpeg";

/** A pre-configured export target. */
export interface ExportPreset {
  id: string;
  name: string;
  format: ExportFormat;
  scale: number;
  suffix: string;
}

/** A brand kit: top-level palette / typography / logos / spacing /
 * per-format export rules. */
export interface BrandKit {
  id: string;
  name: string;
  logo_asset_id: string | null;
  colors: NamedColor[];
  fonts: FontRef[];
  spacing_scale: number[];
  export_rules: ExportPreset[];
}

/**
 * Design-token CRUD bridge. The setter does NOT persist; call
 * `window.kcreate.document.save()` after a setter to land the change
 * on disk.
 */
export interface DesignTokensBridge {
  get(): Promise<DesignTokens>;
  set(tokens: DesignTokens): Promise<void>;
}

/**
 * Brand-kit CRUD bridge. `create` returns the new kit's UUID;
 * `update` replaces the existing row keyed on `kit.id`.
 */
export interface BrandKitBridge {
  create(name: string): Promise<string>;
  update(kit: BrandKit): Promise<void>;
  list(): Promise<BrandKit[]>;
  delete(kitId: string): Promise<boolean>;
}

/**
 * Export-preset CRUD bridge. Used by both the Export panel and the
 * project home page (to seed default presets).
 */
export interface ExportPresetBridge {
  create(name: string, format: ExportFormat, scale: number): Promise<string>;
  list(): Promise<ExportPreset[]>;
  delete(presetId: string): Promise<boolean>;
}

/**
 * Per-artboard summary returned by `window.kcreate.artboard.list()`.
 * Matches `kcreate_bridge::document::ArtboardInfo`.
 */
export interface ArtboardInfo {
  id: string;
  name: string;
  x: number;
  y: number;
  width: number;
  height: number;
  pageId: string;
}

export type ArtboardPresetCategory =
  | "web_desktop"
  | "web_tablet"
  | "web_mobile"
  | "social_media"
  | "print"
  | "custom";

/**
 * One of the built-in artboard preset sizes surfaced in the New
 * Artboard dialog and home-screen affordances.
 */
export interface ArtboardPreset {
  name: string;
  width: number;
  height: number;
  category: ArtboardPresetCategory;
}

export interface ArtboardBridge {
  create(
    pageId: string | null,
    name: string,
    width: number,
    height: number,
  ): Promise<string>;
  list(): Promise<ArtboardInfo[]>;
  duplicate(artboardId: string): Promise<string>;
  resize(artboardId: string, width: number, height: number): Promise<void>;
  presets(): Promise<ArtboardPreset[]>;
}

/**
 * One named variant of a component. The `properties` bag is intentionally
 * free-form JSON so callers can carry whatever metadata they need without
 * us baking schema into the Rust types. Mirrors
 * `kcreate_core::component::ComponentVariant`.
 */
export interface ComponentVariantInfo {
  id: string;
  name: string;
  properties: Record<string, unknown>;
}

/**
 * Per-component summary returned by `window.kcreate.component.list()`.
 * Mirrors `kcreate_bridge::document::ComponentInfo`.
 */
export interface ComponentInfo {
  id: string;
  name: string;
  description: string;
  defaultVariantId: string;
  variants: ComponentVariantInfo[];
  createdAt: string;
  modifiedAt: string;
}

export interface ComponentBridge {
  /**
   * Convert the given (sibling-flat) selection into a new component
   * definition, swap the originals out for a single `ComponentLayer`
   * instance, and return the new definition's id.
   */
  createFromSelection(nodeIds: string[], name: string): Promise<string>;
  list(): Promise<ComponentInfo[]>;
  /**
   * Instantiate a stored definition. If `parentId` is null the new
   * layer is added under the first artboard (or the project root if
   * no artboards exist).
   */
  instantiate(
    componentId: string,
    parentId: string | null,
    x: number,
    y: number,
  ): Promise<string>;
  addVariant(componentId: string, name: string): Promise<string>;
  /** Switch which variant a placed `ComponentLayer` displays. */
  switchVariant(nodeId: string, variantId: string): Promise<void>;
  /** Detach an instance — turns the `ComponentLayer` into a plain group. */
  detach(nodeId: string): Promise<void>;
}

declare global {
  interface Window {
    kcreate: {
      renderer: RendererBridge;
      document: DocumentBridge;
      canvas: CanvasBridge;
      ai: AiBridge;
      mcp: McpBridge;
      runtime: RuntimeBridge;
      export: ExportBridge;
      designTokens: DesignTokensBridge;
      brandKit: BrandKitBridge;
      exportPreset: ExportPresetBridge;
      artboard: ArtboardBridge;
      component: ComponentBridge;
    };
  }
}
