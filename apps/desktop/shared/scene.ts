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

  /**
   * Current presentation mode. `"offscreen"` (the default) means the
   * host drives the rAF readback loop via `acquireFrame()`;
   * `"native"` means the Rust renderer is presenting directly to a
   * platform window surface and the host should hide the canvas
   * element. Mirrors `kcreate_bridge::state::PresentationMode`.
   */
  presentationMode(): Promise<"offscreen" | "native">;

  /**
   * Attach a native presentation surface backed by the current
   * BrowserWindow's platform handle. `width` and `height` are
   * physical pixels (multiply CSS pixels by `devicePixelRatio`).
   *
   * Resolves to the platform variant the bridge interpreted the
   * handle as (`"appkit"` / `"win32"` / `"x11"` / `"wayland"`).
   * Rejects when the bridge was compiled without the `native_canvas`
   * feature, when the handle bytes are malformed, or when GPU
   * surface creation fails \u2014 in any of those cases the host should
   * stay on the offscreen path.
   *
   * The host does NOT pass the handle bytes itself; the main process
   * fetches them from `BrowserWindow::getNativeWindowHandle()` and
   * forwards them to the bridge in the same IPC.
   */
  switchNative(
    width: number,
    height: number,
  ): Promise<"appkit" | "win32" | "x11" | "wayland">;

  /**
   * Detach any attached native surface and revert to the offscreen
   * readback path. No-op when already in offscreen mode.
   */
  switchOffscreen(): Promise<void>;
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

/**
 * Axis-aligned bounding box in document space. Mirror of
 * `kcreate_core::Bounds` / `kcreate_bridge::Bounds`. Carried on every
 * `NodeInfo` so panels that need hit-target geometry (PrototypePlayer
 * hotspots, layout indicators, overlay alignment) can read it without
 * a second IPC hop.
 */
export interface Bounds {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface NodeInfo {
  id: string;
  nodeType: string;
  parentId: string | null;
  children: string[];
  name: string;
  visible: boolean;
  locked: boolean;
  /**
   * Bounds of the node in document space. Always present on every
   * node (defaults to a zero-size box if the underlying layer has
   * no explicit geometry — e.g. groups before layout solving).
   */
  bounds: Bounds;
  componentInstance?: ComponentInstanceInfo;
  /**
   * Free-form metadata bag mirroring `Node::metadata` on the Rust side.
   * Elided when empty. Used by panels that need structured payloads
   * (e.g. the auto-layout config under the `"layout"` key).
   */
  metadata?: Record<string, unknown>;
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
 * Mirror of `kcreate_export::code_gen::InspectCode`. Each field is
 * a copy-paste-ready snippet describing one rendering target's
 * style for the selected node.
 *
 * - `css`: rule body without selector or braces. Wrap in
 *   `.my-button { ... }` at the call site.
 * - `tailwind`: space-separated utility classes (uses arbitrary
 *   values for non-standard sizes, e.g. `w-[123px]`).
 * - `react_style` *(snake_case in transit, see `InspectCode`)*: a
 *   JSX inline-style object literal — `{ width: 100, ... }`.
 */
export interface InspectCode {
  css: string;
  tailwind: string;
  // Serde renames camelCase to snake_case for the wire — we expose
  // the snake_case field directly to match the Rust struct.
  // Callers should access `code.react_style`, not `reactStyle`.
  react_style: string;
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
  /**
   * Returns `true` iff the currently open project is in its
   * untouched, just-created state — no host-recorded operation
   * has been applied since `createProject` / `openProject`. The
   * host UI uses this for first-time prompts (e.g. auto-opening
   * the TemplatePicker on the first switch to Layout mode); see
   * `apps/desktop/renderer/src/pages/EditorPage.tsx`. Rejects
   * with `NoProject` if no project is open. Mirrors the bridge
   * call `project_is_untouched` in
   * `crates/kcreate_bridge/src/document.rs`.
   */
  isUntouched(): Promise<boolean>;

  getDocumentTree(): Promise<NodeInfo[]>;
  /**
   * Inspect-mode code generation for a single node. Returns three
   * handoff snippets (raw CSS rule body, Tailwind utility classes,
   * React inline-style object literal) computed by
   * `kcreate_export::code_gen`. The mapping is intentionally lossy
   * — emitted code is a copy-paste starting point, not a
   * pixel-perfect reproduction. See the rustdoc on
   * `node_to_css` for details.
   */
  inspectNode(nodeId: string): Promise<InspectCode>;
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

/**
 * Resolved resource limits surfaced by `window.kcreate.runtime.resourceLimits()`.
 * Mirrors `kcreate_bridge::document::ResourceLimits`.
 *
 * Values change when the user toggles low-resource mode, so callers
 * should re-fetch after each toggle.
 */
export interface ResourceLimits {
  deviceTier: string;
  lowResourceMode: boolean;
  effectiveUndoDepth: number;
  effectiveRasterCacheMb: number;
  effectiveMaxModelMb: number;
  gpuRenderingAllowed: boolean;
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
  /** Current low-resource mode flag. */
  lowResourceModeGet(): Promise<boolean>;
  /**
   * Set the low-resource mode flag. Tier 0 hosts ignore `false` and
   * keep the flag set — see the Rust `RuntimeConfig::set_low_resource`.
   */
  lowResourceModeSet(enabled: boolean): Promise<void>;
  /** Snapshot the resolved effective limits. */
  resourceLimits(): Promise<ResourceLimits>;
  /**
   * Write a UTF-8 text file at `path`. The host requires the path to
   * be inside the OS temp directory returned by `tempDir()` — any
   * other location is rejected — so the renderer can only land
   * sidecar files (e.g. design-token JSON for a dev handoff) next to
   * its other exports. Returns the number of bytes written.
   */
  writeTextFile(path: string, content: string): Promise<number>;
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
  /**
   * Ask the LLM to propose semantic names for every layer. Requires
   * the sidecar to be `ready`; rejects otherwise.
   */
  suggestLayerNames(): Promise<LayerNamingResult>;
  /**
   * Ask the LLM to extract design tokens (colors / fonts / spacing)
   * from the open document. Requires the sidecar to be `ready`.
   */
  extractDesignTokens(): Promise<LlmJsonResult>;
  /**
   * Ask the LLM to audit the document for accessibility issues.
   * Requires the sidecar to be `ready`.
   */
  checkAccessibility(): Promise<LlmJsonResult>;
}

/** Suggestions returned by `ai.suggestLayerNames`. */
export interface LayerNamingResult {
  /** Parsed `(id, new-name)` pairs. May be empty if the model's
   * reply could not be parsed; in that case the UI should fall back
   * to displaying `raw_content`. */
  suggestions: Array<[string, string]>;
  raw_content: string;
  tokens_used: number;
  model: string;
}

/** Free-form JSON reply returned by `ai.extractDesignTokens` and
 * `ai.checkAccessibility`. The `json` field is the model's raw
 * output in the schema described by the prompt builder. */
export interface LlmJsonResult {
  json: string;
  tokens_used: number;
  model: string;
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

/** Role of a chat message in the LLM conversation. */
export type LlmRole = "system" | "user" | "assistant";

/** One message in an LLM conversation. */
export interface LlmMessage {
  role: LlmRole;
  content: string;
}

/** Reply payload returned by an LLM chat call. */
export interface LlmReply {
  content: string;
  tokens_used: number;
  model: string;
}

/**
 * LLM sidecar status. The `state` field is a tagged union; only
 * `ready` carries `model_name` / `context_size` / `port`, and only
 * `error` carries `error`.
 */
export interface LlmStatus {
  state: "stopped" | "starting" | "ready" | "error";
  model_name: string | null;
  context_size: number | null;
  port: number | null;
  error: string | null;
}

/**
 * Bridge to the local LLM sidecar (`llama-server` from the
 * `kennguy3n/llama.cpp` fork). All traffic stays on loopback
 * (`127.0.0.1`) and the sidecar is opt-in — `start()` must be
 * called before any chat operation succeeds.
 */
export interface LlmBridge {
  /** Spawn the sidecar with the GGUF model at `modelPath`. */
  start(modelPath: string): Promise<number>;
  /** Stop the sidecar. Idempotent. */
  stop(): Promise<void>;
  /** Current sidecar status. */
  status(): Promise<LlmStatus>;
  /**
   * Run a synchronous chat completion. Requires the sidecar to be
   * `ready`; otherwise the promise rejects.
   */
  chat(
    messages: LlmMessage[],
    maxTokens: number,
    temperature: number,
  ): Promise<LlmReply>;
  /**
   * Run a context-aware "suggest improvements" prompt over the
   * current selection (or whole document if nothing is selected).
   */
  suggestForSelection(): Promise<LlmReply>;
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

/**
 * Padding (one f64 per edge). Mirrors `kcreate_layout::Padding`.
 */
export interface LayoutPadding {
  top: number;
  right: number;
  bottom: number;
  left: number;
}

/**
 * Flex layout config. Field names mirror `kcreate_layout::FlexLayout`
 * (snake_case in the wire JSON the bridge expects).
 */
export interface FlexLayout {
  direction: "row" | "column";
  spacing: number;
  padding: LayoutPadding;
  alignment:
    | "start"
    | "center"
    | "end"
    | "space_between"
    | "space_evenly";
  cross_alignment: "start" | "center" | "end" | "stretch";
  wrap: boolean;
}

/**
 * Grid layout config. Mirrors `kcreate_layout::GridLayout`.
 */
export interface GridLayout {
  columns: number;
  row_gap: number;
  column_gap: number;
  padding: LayoutPadding;
}

export interface LayoutBridge {
  /** Write a flex config onto a `LayoutFrame` node. */
  setFlex(nodeId: string, config: FlexLayout): Promise<void>;
  /** Write a grid config onto a `LayoutFrame` node. */
  setGrid(nodeId: string, config: GridLayout): Promise<void>;
  /**
   * Recompute child positions for the LayoutFrame from its stored
   * config. No-op if the node has no layout metadata.
   */
  recompute(nodeId: string): Promise<void>;
  /** Promote a `GroupLayer` to a `LayoutFrame` so it can carry layout config. */
  convertToFrame(nodeId: string): Promise<void>;
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

// ============================================================================
// Prototype interactions (mirrors `kcreate_core::node::Interaction`)
// ============================================================================

export type InteractionTrigger = "click" | "hover" | "press";

export type InteractionAction =
  | { kind: "navigate_to"; target_artboard_id: string }
  | { kind: "scroll_to"; target_node_id: string }
  | { kind: "open_overlay"; overlay_artboard_id: string }
  | { kind: "close_overlay" }
  | { kind: "back" };

export interface Interaction {
  id: string;
  trigger: InteractionTrigger;
  action: InteractionAction;
}

export interface InteractionBridge {
  add(
    nodeId: string,
    trigger: InteractionTrigger,
    action: InteractionAction,
  ): Promise<string>;
  remove(nodeId: string, interactionId: string): Promise<boolean>;
  list(nodeId: string): Promise<Interaction[]>;
  /**
   * Batched [`list`]. Resolves to a map keyed by node id; nodes that
   * have no interactions are omitted from the result, so callers
   * should treat a missing key as an empty list. Used by the
   * prototype player to gather all hotspots on an artboard with a
   * single IPC round trip (Devin Review ANALYSIS-0003).
   */
  listBatch(nodeIds: string[]): Promise<Record<string, Interaction[]>>;
}

// ============================================================================
// Layout Studio: pages, master pages, templates
// (mirrors `kcreate_core::node::PageLayout` and `kcreate_core::project::*`)
// ============================================================================

/**
 * Plain identifier for one of the well-known page sizes. Used by APIs
 * that take a single size argument (e.g. `masterPage.create`). For the
 * full serialised representation of a page size, see [`PageSize`].
 */
export type PageSizeId =
  | "a3"
  | "a4"
  | "a5"
  | "letter"
  | "legal"
  | "tabloid"
  | "presentation_16x9"
  | "presentation_4x3";

/**
 * Serialised form of `kcreate_core::node::PageSize`. Mirrors the
 * internally-tagged serde representation: every variant is an object
 * `{ kind: "<variant>", ...fields }`. Unit variants have just `kind`.
 */
export type PageSize =
  | { kind: PageSizeId }
  | { kind: "custom"; width_mm: number; height_mm: number };

export type PageOrientation = "portrait" | "landscape";

export interface Margins {
  top_mm: number;
  right_mm: number;
  bottom_mm: number;
  left_mm: number;
}

export interface PageLayout {
  page_size: PageSize;
  orientation: PageOrientation;
  margins: Margins;
  master_page_id: string | null;
  page_number: number | null;
}

export interface MasterPageInfo {
  id: string;
  name: string;
  layout: PageLayout | null;
}

export type TemplateCategory =
  | "pitch_deck"
  | "proposal"
  | "brochure"
  | "flyer"
  | "report"
  | "custom";

export type SectionKind =
  | "title"
  | "subtitle"
  | "body_text"
  | "image"
  | "chart"
  | "footer"
  | "page_number";

export interface TemplateSectionDef {
  kind: SectionKind;
  bounds: { x: number; y: number; width: number; height: number };
  placeholder_text: string | null;
}

export interface TemplatePageDef {
  name: string;
  page_size: PageSize;
  orientation: PageOrientation;
  sections: TemplateSectionDef[];
}

export interface LayoutTemplate {
  id: string;
  name: string;
  description: string;
  category: TemplateCategory;
  pages: TemplatePageDef[];
  /**
   * Optional design-token bundle the template wants to apply to the
   * project when instantiated (palette, typography ramp, etc.).
   *
   * Mirrors the `design_tokens: Option<DesignTokens>` field on
   * `kcreate_core::project::LayoutTemplate`. Serialised as JSON `null`
   * by serde when absent — kept in the TS shape for wire-format
   * lockstep (AGENTS.md rule 4).
   */
  design_tokens: DesignTokens | null;
}

export interface MasterPageBridge {
  create(
    name: string,
    size: PageSizeId,
    orientation: PageOrientation,
  ): Promise<string>;
  list(): Promise<MasterPageInfo[]>;
  apply(contentPageId: string, masterPageId: string): Promise<void>;
  detach(contentPageId: string): Promise<void>;
}

export interface LayoutStudioBridge {
  setPageLayout(pageId: string, layout: PageLayout): Promise<void>;
  getPageLayout(pageId: string): Promise<PageLayout | null>;
  listTemplates(): Promise<LayoutTemplate[]>;
  applyTemplate(templateId: string): Promise<string[]>;
  /**
   * Add a new content page. When `size` and `orientation` are omitted
   * the page is created at the workspace default (1920x1080, no page
   * layout metadata). Returns the new page id.
   */
  addPage(
    name: string,
    size?: PageSizeId,
    orientation?: PageOrientation,
  ): Promise<string>;
  /**
   * Duplicate a page (with its artboards / layers). The new page lives
   * at the document root and is named "<original> (copy)". Returns the
   * new page id.
   */
  duplicatePage(pageId: string): Promise<string>;
  /**
   * Move `nodeId` to position `index` under `newParent` — or to the
   * root list when `newParent` is `null`. Drives the PageNavigator's
   * drag-reorder gesture and the layer panel's future move gesture.
   */
  reparentNode(
    nodeId: string,
    newParent: string | null,
    index: number,
  ): Promise<void>;
}

// ---------------------------------------------------------------------------
// Phase 2 — Preflight, Icon Pack, Batch Async, AI extras, Plugin sandbox,
// MCP permission persistence, Screenshot-to-Layout.
// ---------------------------------------------------------------------------

export type PreflightSeverity = "error" | "warning" | "info";

export type PreflightCheckId =
  | "bleed_margin"
  | "font_embed"
  | "image_resolution"
  | "color_space"
  | "transparency"
  | "page_size";

export interface PreflightIssue {
  check: PreflightCheckId;
  severity: PreflightSeverity;
  message: string;
  affected_node_id: string | null;
  page_id: string | null;
}

export type PreflightColorSpaceTarget = "cmyk" | "rgb";

export interface PreflightOptions {
  targetDpi: number;
  requireBleedMm: number;
  allowTransparency: boolean;
  targetColorSpace: PreflightColorSpaceTarget;
}

export interface PreflightRequest {
  pageIds: string[];
  /** Options use snake_case wire keys (Rust struct uses camelCase rename — but its inner fields are camelCase too via `rename_all = "camelCase"`). */
  options: PreflightOptions;
}

export interface PreflightBridge {
  run(request: PreflightRequest): Promise<PreflightIssue[]>;
}

export type IconPlatformName = "web" | "ios" | "android" | "favicon";

export type IconFormat = "png" | "svg" | "ico";

export interface IconSize {
  width: number;
  height: number;
  scale: number;
  suffix: string;
  format: IconFormat;
}

export interface IconPackPlatform {
  name: IconPlatformName | string;
  sizes: IconSize[];
}

export interface IconPackRequest {
  nodeIds: string[];
  platforms: IconPackPlatform[];
  outputDir: string;
}

export interface IconPackBridge {
  builtInPlatforms(): Promise<IconPackPlatform[]>;
  generate(request: IconPackRequest): Promise<string[]>;
}

/**
 * A single item in a batch export. Tagged by `format`: either an SVG
 * render of selected nodes or a PDF render of the whole document.
 */
export type BatchExportItem =
  | {
      format: "svg";
      filename: string;
      node_ids: string[];
      options: SvgExportOptions;
    }
  | { format: "pdf"; filename: string; options: PdfExportOptions };

export type BatchLifecycleStatus =
  | { status: "pending" }
  | { status: "running"; completed: number; total: number }
  | {
      status: "done";
      succeeded: number;
      failed: number;
      errors: string[];
    }
  | { status: "cancelled"; completed: number; total: number };

export interface BatchExportJob {
  id: string;
  items: BatchExportItem[];
  output_dir: string;
  status: BatchLifecycleStatus;
}

export interface BatchStatus {
  jobId: string;
  completed: number;
  total: number;
  currentItem: string;
  finished: boolean;
  cancelled: boolean;
  succeeded: string[];
  failed: Array<[string, string]>;
  durationMs: number;
}

export interface BatchBridge {
  start(job: BatchExportJob): Promise<string>;
  status(jobId: string): Promise<BatchStatus>;
  cancel(jobId: string): Promise<void>;
  /**
   * Release the bookkeeping state for `jobId`.
   *
   * `status()` is idempotent across terminal states — once a job
   * reaches `finished: true`, every subsequent `status()` call
   * returns the same terminal payload. The UI is expected to call
   * `dismiss()` once it has rendered that payload to free the
   * cached result. Dismissing an unknown id is a no-op; the return
   * value is `true` when a handle was actually dropped.
   */
  dismiss(jobId: string): Promise<boolean>;
}

export interface ExtractedColor {
  r: number;
  g: number;
  b: number;
  hex: string;
  frequency: number;
}

export type ModelPackCategory = "core" | "image_pro" | "design_pro" | "generation";

export type ModelKind = "built_in" | "onnx" | "sidecar";

export interface ModelPack {
  id: string;
  name: string;
  category: ModelPackCategory;
  kind: ModelKind;
  capabilities: string[];
  sizeBytes: number;
  filePath: string;
  installed: boolean;
}

export type ScreenshotElementType =
  | "header"
  | "navigation"
  | "hero"
  | "text_block"
  | "image"
  | "button"
  | "card"
  | "footer"
  | "sidebar"
  | "form"
  | "list";

export interface ScreenshotElementBounds {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface ScreenshotElement {
  element_type: ScreenshotElementType;
  bounds: ScreenshotElementBounds;
  confidence: number;
  suggested_name: string;
}

export interface ScreenshotRequest {
  imageBase64: string;
  width: number;
  height: number;
}

export interface AiModelBridge {
  upscale(nodeId: string, scale: number): Promise<string>;
  extractPalette(nodeId: string, maxColors: number): Promise<ExtractedColor[]>;
  smartSelect(
    nodeId: string,
    x: number,
    y: number,
    tolerance: number,
  ): Promise<string>;
  listModelPacks(): Promise<ModelPack[]>;
  screenshotToLayout(request: ScreenshotRequest): Promise<ScreenshotElement[]>;
}

export type PluginType = "wasm" | "js_panel" | "native";

export type PluginPermission =
  | "read_document"
  | "write_document"
  | "read_assets"
  | "export_files"
  | "network_access";

export interface PluginManifest {
  id: string;
  name: string;
  version: string;
  author: string;
  description: string;
  /** Renamed from `plugin_type` on the Rust side via `#[serde(rename = "type")]`. */
  type: PluginType;
  entry_point: string;
  permissions: PluginPermission[];
}

/**
 * Plugin list entry — the manifest fields are flattened to the
 * top-level object on the wire (via `#[serde(flatten)]`), so the JSON
 * has the manifest fields *and* `enabled` side-by-side.
 */
export type PluginListEntry = PluginManifest & { enabled: boolean };

export interface PluginExecuteResult {
  output: string;
  logs: string[];
}

export interface PluginBridge {
  list(): Promise<PluginListEntry[]>;
  enable(id: string): Promise<void>;
  disable(id: string): Promise<void>;
  execute(id: string, fn: string, input: string): Promise<PluginExecuteResult>;
}

export type McpPermissionGrant = "once" | "always" | "denied";

export interface McpPermission {
  client_id: string;
  tool_name: string;
  granted: McpPermissionGrant;
  granted_at: string;
}

export interface McpStatus {
  running: boolean;
  port: number;
}

export interface McpPermissionBridge {
  list(): Promise<McpPermission[]>;
  grant(
    clientId: string,
    toolName: string,
    grant: McpPermissionGrant,
  ): Promise<void>;
  revoke(clientId: string, toolName: string): Promise<void>;
  status(): Promise<McpStatus>;
}

// ---------------------------------------------------------------------------
// Phase 2 — Color management (CMYK / ICC foundation).
// ---------------------------------------------------------------------------

/// Well-known ICC profile identifiers + opt-in custom profile slot.
/// Mirrors `kcreate_core::color::IccProfile`. Custom profiles store a
/// human label and the BLAKE3 hash of the profile blob in the
/// content-addressed asset store.
export type IccProfile =
  | "SrgbIec61966"
  | "AdobeRgb1998"
  | "DisplayP3"
  | "FogRa39"
  | "Swop2006"
  | { Custom: { name: string; blob_hash: string } };

/// Rendering intent for gamut mapping. Mirrors
/// `kcreate_core::color::RenderingIntent`.
export type RenderingIntent =
  | "Perceptual"
  | "RelativeColorimetric"
  | "Saturation"
  | "AbsoluteColorimetric";

/// A color value in one of the supported color spaces. Mirrors
/// `kcreate_core::color::Color`. The tagged-enum wire format is
/// generated by `serde` so JSON looks like
/// `{ "Srgb": { "r": 1, "g": 0, "b": 0, "a": 1 } }`.
///
/// Distinct from the legacy `Color` RGBA tuple used by the renderer's
/// scene wire format (line 7); this richer enum is only for the Phase 2
/// color-management bridge so it can preserve CMYK / Lab / HSL on the
/// way to / from print export.
export type ColorValue =
  | { Srgb: { r: number; g: number; b: number; a: number } }
  | { Cmyk: { c: number; m: number; y: number; k: number; a: number } }
  | { Lab: { l: number; a_star: number; b_star: number; alpha: number } }
  | { Hsl: { h: number; s: number; l: number; a: number } };

/// Document-level color management settings. Mirrors
/// `kcreate_core::color::ColorSettings`. `working_space_cmyk` of
/// `null` means "no CMYK conversion until the user opts in"; setting
/// it to `FogRa39` / `Swop2006` / a custom profile activates the
/// CMYK PDF export pipeline.
export interface ColorSettings {
  working_space_rgb: IccProfile;
  working_space_cmyk: IccProfile | null;
  rendering_intent: RenderingIntent;
  soft_proof_profile: IccProfile | null;
  gamut_warning: boolean;
}

export type ColorSpaceName = "srgb" | "cmyk" | "lab" | "hsl";

export interface ColorBridge {
  /// Read the document's current color settings.
  getSettings(): Promise<ColorSettings>;
  /// Replace the document's color settings. Records an undoable
  /// `color_settings_update` operation.
  updateSettings(settings: ColorSettings): Promise<void>;
  /// Convert a color value into the given color space. Cmyk → Cmyk
  /// short-circuits so authored K-channel data survives round trips.
  convert(color: ColorValue, toSpace: ColorSpaceName): Promise<ColorValue>;
}

// ---------------------------------------------------------------------------
// Phase 2 — Text frame + OpenType (Block B Tasks 7, 10, 11).
//
// The wire format mirrors `kcreate_core::node::TextFrameOptions`,
// `kcreate_core::node::OpenTypeFeatures` and the `TextLayoutWire` JSON
// returned by `phase2::text_layout_compute`. Adding a field on either
// side requires adding it here too — rule 4 of AGENTS.md.
// ---------------------------------------------------------------------------

// All four enums use `#[serde(rename_all = "snake_case")]` on the Rust
// side, so the wire values are lowercase / underscore-separated.
export type TextOverflow = "clip" | "ellipsis" | "overflow";
export type TextWrapMode = "none" | "bounding_box" | "contour";
export type VerticalAlign = "top" | "middle" | "bottom";
export type TextAutoSize = "fixed" | "height_auto" | "width_and_height_auto";

/// Per-side text-frame inset, in document units (points by default).
export interface FrameInsets {
  top: number;
  right: number;
  bottom: number;
  left: number;
}

export interface TextFrameOptions {
  overflow: TextOverflow;
  columns: number;
  column_gap: number;
  wrap_mode: TextWrapMode;
  hyphenation: boolean;
  /// BCP-47 language tag, e.g. `"en-US"`, `"de-DE"`. Hyphenation
  /// patterns ship for `en-US` only; other languages fall through
  /// to no-hyphenation until additional `.pat` files are bundled.
  hyphenation_language: string;
  vertical_alignment: VerticalAlign;
  inset: FrameInsets;
  auto_size: TextAutoSize;
}

/// OpenType feature toggles. `stylistic_sets` is a sparse list of
/// 1..=20 indices (ss01..=ss20); other indices are silently dropped
/// by the Rust encoder. See `kcreate_text::opentype_features_to_buzz`.
export interface OpenTypeFeatures {
  ligatures: boolean;
  contextual_alternates: boolean;
  kerning: boolean;
  small_caps: boolean;
  old_style_figures: boolean;
  tabular_figures: boolean;
  stylistic_sets: number[];
  fractions: boolean;
  ordinals: boolean;
}

/// One line of a paragraph layout. Wire payload returned by
/// `text_layout_compute`. The renderer doesn't redraw glyphs from
/// this — it's strictly for the inspector / debug overlay (line
/// boundaries, column boundaries, overflow indication).
export interface TextLayoutLineWire {
  originX: number;
  baselineY: number;
  width: number;
  column: number;
  glyphCount: number;
}

export interface TextLayoutWire {
  lines: TextLayoutLineWire[];
  overflow: boolean;
  usedHeight: number;
}

export interface TextFrameBridge {
  /// Read the text-frame options for a `TextLayer` node.
  get(nodeId: string): Promise<TextFrameOptions>;
  /// Replace the text-frame options. Records an undoable
  /// `text_frame_update` operation.
  update(nodeId: string, options: TextFrameOptions): Promise<void>;
  /// Compute the paragraph layout (line origins, baselines, columns,
  /// overflow flag) without recording an operation.
  computeLayout(nodeId: string): Promise<TextLayoutWire>;
  /// Read the OpenType feature toggles for a `TextLayer` node.
  getOpenTypeFeatures(nodeId: string): Promise<OpenTypeFeatures>;
  /// Replace the OpenType feature toggles. Records an undoable
  /// `text_opentype_features_update` operation.
  updateOpenTypeFeatures(
    nodeId: string,
    features: OpenTypeFeatures,
  ): Promise<void>;
}

declare global {
  interface Window {
    kcreate: {
      renderer: RendererBridge;
      document: DocumentBridge;
      canvas: CanvasBridge;
      ai: AiBridge;
      llm: LlmBridge;
      mcp: McpBridge;
      runtime: RuntimeBridge;
      export: ExportBridge;
      designTokens: DesignTokensBridge;
      brandKit: BrandKitBridge;
      exportPreset: ExportPresetBridge;
      artboard: ArtboardBridge;
      component: ComponentBridge;
      layout: LayoutBridge;
      interaction: InteractionBridge;
      masterPage: MasterPageBridge;
      layoutStudio: LayoutStudioBridge;
      preflight: PreflightBridge;
      iconPack: IconPackBridge;
      batch: BatchBridge;
      aiModel: AiModelBridge;
      plugin: PluginBridge;
      mcpPermission: McpPermissionBridge;
      color: ColorBridge;
      textFrame: TextFrameBridge;
    };
  }
}
