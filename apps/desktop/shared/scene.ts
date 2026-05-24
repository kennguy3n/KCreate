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

/**
 * Wire-format names of `NodeType` variants that are containers (i.e.
 * `NodeType::is_container() == true` on the Rust side, exposed via
 * the bridge as `NodeInfo::nodeType`).
 *
 * **Lockstep contract**: this constant mirrors
 * `kcreate_core::node::CONTAINER_NODE_WIRE_NAMES` in
 * `crates/kcreate_core/src/node.rs`. If you change `is_container()`
 * (or add/remove a `NodeType` variant) you must update three things
 * together: (1) the Rust `is_container` exhaustive match, (2) the
 * Rust `CONTAINER_NODE_WIRE_NAMES` constant, and (3) this TS
 * constant. The Rust test `canonical_container_wire_names_match_expected_list`
 * fires if (1) and (2) ever diverge; consumers of (3) in the renderer
 * (e.g. `AIAssistPanel::LAYOUT_ASSIST_CONTAINER_TYPES`) import from
 * here so there is exactly one TS-side source of truth.
 */
export const CONTAINER_NODE_TYPES: ReadonlyArray<string> = Object.freeze([
  "Page",
  "Artboard",
  "GroupLayer",
  "LayoutFrame",
  "ComponentLayer",
]);

/**
 * Convenience predicate: is the given wire-format node-type name a
 * container? Backs the `AIAssistPanel` layout-suggest button-eligibility
 * gate. Implementation is a tiny constant-time `Set` lookup so callers
 * don't pay an `indexOf` per render.
 */
const CONTAINER_NODE_TYPE_SET = new Set<string>(CONTAINER_NODE_TYPES);
export function isContainerNodeType(nodeType: string): boolean {
  return CONTAINER_NODE_TYPE_SET.has(nodeType);
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
 * Outcome of a successful `documentUndo` / `documentRedo` round-trip.
 *
 * Mirrors `crates/kcreate_bridge/src/document.rs::UndoRedoOutcome`.
 * The `command` is the stable `Operation::command` string (e.g.
 * `"color_settings_update"`, `"document_update_node"`) — the host
 * uses it to gate per-operation broadcasts so an undo of an
 * unrelated op (a `move_node`, say) doesn't fire the
 * `kcreate/color/settings/changed` event and trigger a needless
 * `SoftProofOverlay` re-render. `affectedNodes` is the list of node
 * ids the operation touched, empty for non-graph ops like
 * `color_settings_update`.
 */
export interface UndoRedoOutcome {
  command: string;
  affectedNodes: string[];
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
  undo(): Promise<UndoRedoOutcome | null>;
  redo(): Promise<UndoRedoOutcome | null>;

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
  /**
   * Output device color space.
   * - `"rgb"` (default): standard `/DeviceRGB`.
   * - `"cmyk"`: `/DeviceCMYK` for print-bound output.
   * - `"passThrough"`: leave colors in source space.
   * When omitted the bridge picks CMYK iff a CMYK working space is set
   * on the document's color settings, otherwise RGB.
   */
  colorMode?: "rgb" | "cmyk" | "passThrough";
  /**
   * CMYK rasterisation dithering. Ignored unless `colorMode === "cmyk"`.
   * - `"floydSteinberg"` (default): error-diffusion; best quality.
   * - `"bayer8x8"`: 8×8 ordered dither; parallelisable.
   * - `"none"`: nearest-neighbour quantisation (Phase 2 baseline).
   */
  cmykDither?: "none" | "floydSteinberg" | "bayer8x8";
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
  | "page_size"
  | "shading"
  | "font_glyph_coverage"
  | "total_ink_coverage"
  | "bleed_area_empty";

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
  /**
   * Hard minimum DPI for raster images — anything below this is
   * an `image_resolution` Error ("unrecoverable"), anything between
   * the floor and `targetDpi` is a Warning, anything above is
   * silent. When `0` (the default), the floor is inferred from
   * `targetColorSpace`: 150 DPI for `cmyk` (press soft-proof
   * minimum), 72 DPI for `rgb` (screen baseline). Set explicitly
   * to override (e.g. 240 for a high-end commercial run, 96 for
   * low-res draft proofs). Use `0` rather than `null` so the
   * Rust-side `serde_json` round-trip stays simple (the Rust
   * struct uses `0.0` as the deny-by-default sentinel because
   * non-finite floats are not JSON-encodable).
   */
  imageDpiFloor: number;
  requireBleedMm: number;
  /**
   * Whether to raise a `bleed_area_empty` warning per uncovered
   * side on artboards configured with bleed. Useful to turn off
   * for documents containing a mix of press + screen artboards
   * where the screen-only pages would otherwise generate noise.
   * Defaults to `true`.
   */
  checkBleedAreaCoverage: boolean;
  allowTransparency: boolean;
  targetColorSpace: PreflightColorSpaceTarget;
  /**
   * Total ink coverage cap as a fraction (1.0 = 100%, 3.0 = 300%).
   * 300% is the GRACoL / SWOP commercial offset default; web /
   * newsprint targets use lower caps (240% — 280%). When a CMYK
   * fill's component sum exceeds this value, the preflight engine
   * emits a `total_ink_coverage` warning. Defaults to 3.0 (300%)
   * when omitted by older clients — the Rust side has
   * `#[serde(default)]`.
   */
  targetTotalInkCoverage: number;
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
  /// Canonical out-of-band download URL. KCreate never fetches this
  /// itself — the user downloads the weights and points the
  /// installer at the file. Empty for built-in packs.
  downloadUrl: string;
  /// Hex-encoded SHA-256 of the canonical weights. Empty when the
  /// registry hasn't pinned a hash yet — see the comment on the
  /// Rust mirror at `crates/kcreate_ai/src/model_registry.rs`.
  sha256: string;
}

/// Result of a successful `aiModel.install()` call. Mirrors
/// `kcreate_ai::model_registry::InstallReport`.
export interface ModelInstallReport {
  packId: string;
  /// Hex-encoded SHA-256 of the bytes actually written into
  /// `models_dir`.
  actualSha256: string;
  /// `true` iff the registry carried a non-empty canonical hash and
  /// it matched the source file. `false` means the file installed
  /// but couldn't be cross-checked against a canonical hash.
  verified: boolean;
  sizeBytes: number;
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

/// Result of `kcreate_ai::generate_alt_text` — the heuristic
/// alt-text generator. The `text` field is the recommended
/// human-readable description; the structured fields are exposed
/// so the renderer can render a richer preview UI without
/// re-running the analysis.
export interface AltTextReport {
  text: string;
  /// Mean luminance in 0.0..1.0 (Rec. 709 weights). Drives the
  /// "Dark / Balanced / Bright" word in the generated sentence.
  brightness: number;
  /// Stddev of luminance in 0.0..1.0. Drives the
  /// "low-contrast / balanced / high-contrast" word.
  contrast: number;
  /// Mean saturation in 0.0..1.0 (HSV). Drives the
  /// "muted / balanced / vivid" word.
  saturation: number;
  /// Sobel edge density in 0.0..1.0. Above ~0.18 the description
  /// switches from "flat graphic" to "photographic detail".
  edge_density: number;
  /// Top-N dominant colors via k-means.
  palette: ExtractedColor[];
}

/// Axis-aligned bounding box returned by
/// `kcreate_ai::layout_suggest::Bounds`. `x`/`y` are the top-left
/// corner. Distinct from [`ScreenshotElementBounds`] only to
/// signal which Rust type it mirrors.
export interface LayoutBounds {
  x: number;
  y: number;
  width: number;
  height: number;
}

/// Detected dominant orientation of a cluster. Mirrors
/// `kcreate_ai::layout_suggest::LayoutOrientation` (snake_case).
export type LayoutOrientation = "row" | "column" | "grid" | "cloud";

/// Detected alignment edge within a cluster. Mirrors
/// `kcreate_ai::layout_suggest::LayoutAlignment` (snake_case).
export type LayoutAlignment =
  | "left"
  | "right"
  | "top"
  | "bottom"
  | "center_horizontal"
  | "center_vertical";

/// One proposed group from `kcreate_ai::suggest_layout_grouping`.
/// The renderer previews each suggestion before any apply step;
/// a future `applyLayoutSuggestion` call will promote a chosen
/// suggestion into a real `LayoutFrame`.
export interface LayoutSuggestion {
  /// Human-readable label (e.g. "Row of 3", "Vertical stack").
  name: string;
  /// Axis-aligned bounding box covering every member node.
  bounds: LayoutBounds;
  /// Node ids that belong in the proposed group.
  member_ids: string[];
  /// Detected dominant orientation. Drives a row vs. column
  /// preview affordance in the UI.
  orientation: LayoutOrientation;
  /// Detected alignment edge within the group, or `null` when the
  /// cluster doesn't read as aligned on any single edge.
  alignment: LayoutAlignment | null;
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
  /// Open the native file picker scoped to weights files and return
  /// the chosen absolute path (or `null` if the user cancelled).
  pickModelFile(): Promise<string | null>;
  /// Install a model pack from a user-provided source file. The
  /// install is atomic — partial writes never land in the models
  /// directory.
  installModelPack(
    packId: string,
    sourcePath: string,
  ): Promise<ModelInstallReport>;
  /// Uninstall a model pack by deleting its file. Idempotent.
  uninstallModelPack(packId: string): Promise<void>;
  screenshotToLayout(request: ScreenshotRequest): Promise<ScreenshotElement[]>;
  /// Run the local alt-text heuristic against a raster layer.
  /// Read-only: does NOT persist anything to the document — call
  /// [`AiModelBridge.applyAltText`] to commit the chosen string.
  altTextForNode(nodeId: string): Promise<AltTextReport>;
  /// Persist an alt-text label onto `nodeId`. Records an
  /// undo/redo-able operation in the document log. An empty
  /// string clears the metadata entry entirely.
  applyAltText(nodeId: string, text: string): Promise<void>;
  /// Run the layout-suggest heuristic over the direct (visible,
  /// non-degenerate) children of `artboardId`. Returns an empty
  /// list when fewer than two candidates remain, rather than an
  /// error — so the UI can render a "nothing to suggest" state
  /// without special-casing the call.
  layoutSuggestForArtboard(artboardId: string): Promise<LayoutSuggestion[]>;
}

/// Result of [`PdfImportBridge.importPdf`] mirroring
/// `kcreate_bridge::phase2::PdfImportReport`.
export interface PdfImportReport {
  /// `/Info /Title` from the PDF, if present.
  title: string | null;
  /// `/Info /Author` from the PDF, if present.
  author: string | null;
  /// New KCreate page ids in import order. The renderer can jump
  /// to `pageIds[0]` to focus the first imported page.
  pageIds: string[];
  /// Successfully extracted image count across all pages.
  imagesImported: number;
  /// Skipped image count (unsupported filter / color space).
  imagesSkipped: number;
  /// Human-readable, non-fatal warnings the renderer should
  /// surface (e.g. "Page 3: unsupported image filter (JPXDecode)").
  warnings: string[];
}

/// Renderer-facing PDF import API. The picker is a thin wrapper
/// over Electron's `dialog.showOpenDialog`, kept in the main
/// process so renderer code never touches the filesystem directly.
export interface PdfImportBridge {
  /// Show an Electron file picker scoped to `.pdf` and return the
  /// chosen absolute path, or `null` if the user cancelled.
  pickFile(): Promise<string | null>;
  /// Import the PDF at `filePath` into the current project. One
  /// new Page per PDF page, with embedded images as RasterLayer
  /// children and extracted text as a TextLayer per page.
  importPdf(filePath: string): Promise<PdfImportReport>;
}

export type PluginType = "wasm" | "js_panel" | "native";

export type PluginPermission =
  | "read_document"
  | "write_document"
  | "read_assets"
  | "export_files"
  | "network_access";

export type PanelPosition =
  | "right_sidebar"
  | "bottom_panel"
  | "floating_window";

/**
 * JS-panel-specific config carried alongside the standard manifest
 * fields when `type === "js_panel"`. Required for that type; absent
 * for WASM / native plugins.
 */
export interface JsPanelConfig {
  entry_html: string;
  panel_title: string;
  panel_position: PanelPosition;
  width: number;
  height: number;
  permissions: PluginPermission[];
}

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
  /** Present only when `type === "js_panel"`. */
  js_panel?: JsPanelConfig;
}

/**
 * Outcome of the host's last verification of a plugin's optional
 * `manifest.json.sig` sidecar. Mirrors
 * `kcreate_plugin::SignatureStatus` (tagged union, snake_case
 * variants).
 *
 * Invariant: only **non-native** plugins can carry `invalid` — the
 * registry refuses to load native plugins in any state other than
 * `verified`, so they will never appear with `invalid` or
 * `unsigned` in `pluginList()` output.
 */
export type PluginSignatureStatus =
  | { status: "unsigned" }
  | { status: "verified"; key_id: string }
  | { status: "invalid"; key_id: string; reason: string };

/**
 * Plugin list entry — the manifest fields are flattened to the
 * top-level object on the wire (via `#[serde(flatten)]`), so the JSON
 * has the manifest fields *and* `enabled` / `signature` side-by-side.
 */
export type PluginListEntry = PluginManifest & {
  enabled: boolean;
  signature: PluginSignatureStatus;
};

/**
 * A single trusted Ed25519 public key loaded from
 * `~/.kcreate/plugins/trusted_keys.json`. The UI's
 * "Trusted Authorities" list renders one row per entry.
 */
export interface TrustedKeyInfo {
  /** Stable identifier the manifest sidecar references via `key_id`. */
  keyId: string;
  /** Free-form human label. */
  comment: string;
}

export interface PluginExecuteResult {
  output: string;
  logs: string[];
}

/**
 * Outcome of a single proposal validated and (when accepted) applied
 * by `plugin_execute_with_context`. The `status` discriminator
 * matches `phase2::ProposalOutcome`'s serde tag.
 */
export type PluginProposalReport =
  | {
      type: "create_node";
      parent_id: string;
      node_type: string;
      props: unknown;
      outcome: { status: "applied"; node_id: string } | { status: "rejected"; reason: string };
    }
  | {
      type: "update_node";
      node_id: string;
      changes: unknown;
      outcome: { status: "applied"; node_id: string } | { status: "rejected"; reason: string };
    }
  | {
      type: "delete_node";
      node_id: string;
      outcome: { status: "applied"; node_id: string } | { status: "rejected"; reason: string };
    };

/**
 * Extended-ABI execution result: same as the basic
 * `PluginExecuteResult` but with the proposal outcomes the host
 * applied.
 */
export interface PluginExecuteWithContextResult extends PluginExecuteResult {
  proposals: PluginProposalReport[];
}

/**
 * Description of an installed JS panel plugin returned by
 * `plugin_js_list()`. The Electron host uses these to decide which
 * sandboxed `BrowserView` instances to mount and where.
 */
export interface JsPanelInfo {
  id: string;
  name: string;
  version: string;
  config: JsPanelConfig;
  enabled: boolean;
}

/**
 * Single message ferried between a JS panel and the bridge. The
 * `type` discriminator matches `kcreate_plugin::JsPanelMessage`.
 */
export type JsPanelMessage =
  | { type: "read_document"; query: unknown }
  | { type: "write_proposal"; proposal: unknown }
  | { type: "log"; message: string };

/**
 * Outcome of a `JsPanelMessage`. The Electron host returns this to
 * the panel via `postMessage` so the panel can update its UI.
 */
export type JsPanelMessageOutcome =
  | { status: "ok"; result: unknown }
  | { status: "denied"; permission: PluginPermission }
  | { status: "invalid"; reason: string };

export interface PluginBridge {
  list(): Promise<PluginListEntry[]>;
  enable(id: string): Promise<void>;
  disable(id: string): Promise<void>;
  execute(id: string, fn: string, input: string): Promise<PluginExecuteResult>;
  /**
   * Extended-ABI execution: builds a `PluginContext` with the current
   * document snapshot and the plugin's manifest permissions, runs the
   * plugin, then validates and applies any proposals it produced.
   */
  executeWithContext(
    id: string,
    fn: string,
    input: string,
  ): Promise<PluginExecuteWithContextResult>;
  /** List installed JS panel plugins for the Electron host. */
  jsList(): Promise<JsPanelInfo[]>;
  /**
   * Validate and dispatch a single message from a sandboxed JS panel.
   * The Electron host calls this for every inbound `postMessage`.
   */
  jsMessage(pluginId: string, message: JsPanelMessage): Promise<JsPanelMessageOutcome>;
  /**
   * Ask the Electron host to mount a sandboxed `WebContentsView` for
   * the named plugin. Bounds are in CSS pixels relative to the main
   * window content area. If the panel is already mounted, the bounds
   * are updated in place. Throws if the plugin id is unknown or not
   * a `js_panel` plugin.
   */
  jsOpen(pluginId: string, bounds: { x: number; y: number; width: number; height: number }): Promise<void>;
  /** Update the bounds of an already-mounted panel. No-op if not mounted. */
  jsSetBounds(
    pluginId: string,
    bounds: { x: number; y: number; width: number; height: number },
  ): Promise<void>;
  /** Tear down the panel for `pluginId` if mounted; no-op otherwise. */
  jsClose(pluginId: string): Promise<void>;
  /**
   * Snapshot of trusted plugin-signing public keys. The UI's
   * "Trusted Authorities" list calls this on mount.
   */
  trustList(): Promise<TrustedKeyInfo[]>;
  /**
   * Re-read `trusted_keys.json` from disk and rescan plugins so any
   * previously-rejected native plugin gets a second chance once a
   * matching key is added.
   */
  trustReload(): Promise<void>;
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

/// Color-space taxonomy for custom ICC profiles. Mirrors
/// `kcreate_core::color::IccColorSpace`. Determines whether a
/// `Custom` profile activates the CMYK export pipeline, the
/// grayscale path, or stays in the RGB working space. Required so
/// `IccProfile.is_cmyk` returns the right answer for custom press
/// profiles instead of silently falling back to RGB.
export type IccColorSpace = "Rgb" | "Cmyk" | "Gray" | "Lab";

/// Well-known ICC profile identifiers + opt-in custom profile slot.
/// Mirrors `kcreate_core::color::IccProfile`. Custom profiles store a
/// human label, the BLAKE3 hash of the profile blob in the
/// content-addressed asset store, and the device color space the
/// profile targets. The `color_space` field is optional on the wire
/// for forward-compat with projects authored before it existed; the
/// Rust side defaults it to `Rgb`.
export type IccProfile =
  | "SrgbIec61966"
  | "AdobeRgb1998"
  | "DisplayP3"
  | "FogRa39"
  | "Swop2006"
  | {
      Custom: {
        name: string;
        blob_hash: string;
        color_space?: IccColorSpace;
      };
    };

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
  /// `color_settings_update` operation; the bridge owns the
  /// inverse-patch dispatch so `documentUndo()` actually restores
  /// the previous settings (Phase 2 PR #7).
  updateSettings(settings: ColorSettings): Promise<void>;
  /// Convert a color value into the given color space. Cmyk → Cmyk
  /// short-circuits so authored K-channel data survives round trips.
  convert(color: ColorValue, toSpace: ColorSpaceName): Promise<ColorValue>;
  /// Push-channel subscription that fires whenever
  /// `ws.project.color_settings` mutates: direct updates and undo /
  /// redo of a `color_settings_update` operation both notify here.
  /// The callback receives no payload — call `getSettings()` to read
  /// the new shape. Returns an unsubscribe function for effect
  /// cleanup. Replaces the previous 2-second polling fallback that
  /// `SoftProofOverlay` relied on before the bridge gained push
  /// semantics.
  onSettingsChanged(callback: () => void): () => void;
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

// ---------------------------------------------------------------------------
// Phase 3 — LAN collaboration session.
//
// Mirrors the JSON shapes emitted by
// `kcreate_bridge::collab::{SessionStartReport, SessionPeer,
// SessionPresence, SessionCursor, SessionEvent}`. Field names are
// `camelCase` because the Rust DTOs use `#[serde(rename_all =
// "camelCase")]`. The `SessionEvent` discriminator follows
// `#[serde(tag = "kind", rename_all = "camelCase")]` so the
// `kind` field is the discriminator.
// ---------------------------------------------------------------------------

export interface SessionStartReport {
  peerId: string;
  publicKey: string;
  displayName: string;
  projectId: string;
  localAddr: string;
  certFingerprint: string;
  advertiseMdns: boolean;
}

export interface SessionCursor {
  x: number;
  y: number;
}

export interface SessionPresence {
  activePage: string | null;
  selection: string[];
  cursor: SessionCursor | null;
  /// ISO-8601 datetime.
  sentAt: string;
}

export interface SessionPeer {
  peerId: string;
  publicKey: string;
  displayName: string;
  presence: SessionPresence | null;
}

export type SessionEvent =
  | {
      kind: "discovered";
      peerId: string;
      publicKey: string;
      displayName: string;
      projectId: string;
      socketAddr: string;
      certFingerprint: string;
    }
  | { kind: "undiscovered"; peerId: string }
  | {
      kind: "peerJoined";
      peerId: string;
      publicKey: string;
      displayName: string;
    }
  | { kind: "peerLeft"; peerId: string }
  | {
      kind: "presenceUpdated";
      peerId: string;
      presence: SessionPresence;
    }
  | {
      /// Block 7: a remote peer's `OperationBroadcast` was recorded
      /// in the session journal. Emitted once per batch, after
      /// every individual op has been validated for monotonicity
      /// against the journal's per-peer high-water mark.
      kind: "operationsJournaled";
      peerId: string;
      /// Number of operations recorded in this batch.
      opCount: number;
      /// Highest Lamport clock value observed in the batch.
      highestClock: number;
    }
  | {
      /// Block 8: the advisory edit-lock roster changed — a peer
      /// claimed or released one or more node locks. The renderer
      /// uses this event to know when to re-read
      /// `session.locks()` rather than polling every frame.
      kind: "locksChanged";
      /// Which peer caused the change. For PeerLeft-triggered
      /// auto-releases this is the leaving peer.
      peerId: string;
      /// Node ids whose lock state flipped. Cross-reference with
      /// the authoritative `session.locks()` roster to determine
      /// which entries are now claimed vs. released.
      nodeIds: string[];
    }
  | {
      /// Round 11: the local collab session just started. Emitted
      /// synchronously by the bridge in `session_start` (mirrored
      /// in `crates/kcreate_bridge/src/collab.rs::SessionEvent`)
      /// so renderer hooks (`useSessionLocks`, the EditorPage
      /// presence-broadcast effect) can re-key their state on
      /// local-side lifecycle transitions — the existing
      /// `peer*` events only fire for *remote* peers and would
      /// never signal a fresh local session by themselves.
      kind: "sessionStarted";
      /// Base64url-encoded local peer id.
      peerId: string;
      /// Project the new session is bound to (UUID hyphenated).
      projectId: string;
    }
  | {
      /// Round 11: the local collab session just stopped. Synthesised
      /// by `main.ts`'s `kcreate/session/leave` IPC handler after
      /// the bridge returns the leaving peer id; the bridge itself
      /// can't push the event through its regular queue because
      /// that queue is dropped as part of the leave. Consumers
      /// reset session-keyed dedup state (e.g. EditorPage's
      /// presence-broadcast fingerprint, useSessionLocks's lock
      /// roster cache) when they see this.
      kind: "sessionLeft";
      /// Base64url-encoded peer id of the session that just left.
      peerId: string;
    };

/// Block 7: per-peer Lamport high-water marks for the journal
/// scoped to the running session's project. Mirrors
/// `kcreate_bridge::collab::SessionJournalSummary`.
export interface SessionJournalSummary {
  /// Total number of journaled entries across every peer.
  entryCount: number;
  /// Distinct peers the journal has heard from.
  peerCount: number;
  /// Per-peer high-water clock. Keys are base64url peer ids;
  /// values are the highest Lamport clock the journal has
  /// recorded for that peer.
  byPeer: Record<string, number>;
}

/// Block 8: one entry in the advisory edit-lock roster. Mirrors
/// `kcreate_bridge::collab::SessionLockEntry`.
export interface SessionLockEntry {
  /// Node currently locked.
  nodeId: string;
  /// Peer id of the lock holder (base64url).
  holderPeerId: string;
  /// RFC3339 wall-clock time the holder acquired the lock.
  acquiredAt: string;
}

export interface SessionBridge {
  /// Start a local collab session. Returns the local peer's
  /// identity + bind address + cert fingerprint so the renderer
  /// can display a "share this with peers" UI. `seedB64` is the
  /// persistent Ed25519 seed (base64url, 32 bytes); supply the
  /// same value across sessions for stable peer identity.
  ///
  /// Rejects with `multiplayer is locked: not signed into a KChat
  /// group` if no valid `KChatGroupAuthority` is installed via
  /// `kchat.install()`. The KChat client is the only thing
  /// authorised to call install — until it does, multiplayer is
  /// hard-locked at the protocol layer.
  start(
    seedB64: string,
    displayName: string,
    projectId: string,
    advertiseMdns: boolean,
  ): Promise<SessionStartReport>;
  /// Stop the running session. Idempotent.
  leave(): Promise<void>;
  /// Dial a known peer. Use the fields from a discovered-peer
  /// `SessionEvent` (`peerId`, `publicKey`, `displayName`,
  /// `socketAddr`, `certFingerprint`) or from a pasted peer link.
  ///
  /// Rejects with the same KChat-gate error as `start` if the
  /// renderer hasn't installed an authority yet.
  join(
    peerId: string,
    publicKey: string,
    displayName: string,
    socketAddr: string,
    certFingerprintB64: string,
  ): Promise<void>;
  /// Snapshot of currently-connected peers + their latest
  /// presence (cursor / selection / active page).
  peers(): Promise<SessionPeer[]>;
  /// Read the cached local-identity report, or `null` if no
  /// session is running.
  info(): Promise<SessionStartReport | null>;
  /// Block 7: read the running session's per-peer Lamport
  /// high-water marks. KChat-gated: rejects with the standard
  /// `not signed into a KChat group` error if no authority is
  /// installed. Used by the PresencePanel's "Activity" tab to
  /// surface "we've recorded N ops across M peers".
  journalSummary(): Promise<SessionJournalSummary>;
  /// Block 8: read the advisory edit-lock roster. KChat-gated.
  /// Returns an empty list when no session is running so the
  /// renderer can call this unconditionally on every paint.
  locks(): Promise<SessionLockEntry[]>;
  /// Block 8: claim advisory edit locks on the supplied node ids.
  /// KChat-gated. Updates the local roster immediately and
  /// broadcasts the claim to every connected peer. Returns the
  /// wall-clock acquisition timestamp as an RFC3339 string.
  claimLocks(nodeIds: string[]): Promise<string>;
  /// Block 8: release advisory edit locks. Empty `nodeIds` means
  /// "release everything I currently hold" (the "I'm done editing"
  /// signal).
  releaseLocks(nodeIds: string[]): Promise<void>;
  /// Broadcast the local user's presence to every connected peer.
  /// `cursor` is world-space coordinates; pass `null` when the
  /// pointer has left the canvas.
  ///
  /// Rejects with the KChat-gate error if no authority is
  /// installed — presence beacons never leave the box outside a
  /// KChat group.
  sendPresence(
    activePage: string | null,
    selection: string[],
    cursor: SessionCursor | null,
  ): Promise<void>;
  /// Subscribe to the push channel that fires when peers join /
  /// leave / move their cursor. Returns an unsubscribe function.
  /// The renderer is responsible for filtering by kind.
  onEvent(callback: (event: SessionEvent) => void): () => void;
}

// ---------------------------------------------------------------------------
// Phase 4 — KChat group authority (multiplayer gate).
//
// Mirrors the JSON shapes emitted by
// `kcreate_bridge::collab::{KChatInstallRequest, KChatMembershipStatus}`.
// Field names are `camelCase` because the Rust DTOs use
// `#[serde(rename_all = "camelCase")]`.
//
// Architecture: every multiplayer entry point on `SessionBridge`
// fails closed until `KChatBridge.install()` is called with a
// valid Ed25519-signed membership attestation minted by a KChat
// group server. The KChat client (out of tree, future work) is
// the only thing authorised to call `install`. Until then the
// renderer surfaces a "Collaboration is locked — sign into a
// KChat group to enable multiplayer" CTA in place of the
// start/join buttons.
// ---------------------------------------------------------------------------

export interface KChatInstallRequest {
  /// 32-byte Ed25519 verifying key of the KChat group server (the
  /// issuer trust root), URL-safe base64 (no padding).
  issuerPublicKey: string;
  /// Group identifier minted on the issuer side. URL-safe ASCII,
  /// max 128 chars.
  groupId: string;
  /// Peer id (BLAKE3-derived from the peer's public key) of the
  /// local user.
  peerId: string;
  /// 32-byte Ed25519 verifying key of the local user, URL-safe
  /// base64 (no padding). Must match the peer key used for
  /// `session.start`.
  peerPublicKey: string;
  /// Membership issuance time (ISO-8601).
  issuedAt: string;
  /// Membership expiry time (ISO-8601). KChat servers should mint
  /// short-lived attestations (hours, not days).
  expiresAt: string;
  /// 64-byte Ed25519 signature, URL-safe base64 (no padding).
  signature: string;
}

export interface KChatMembershipStatus {
  /// True when no authority is installed, or the installed one is
  /// expired / forged / bound to a different peer key. The
  /// `PresencePanel` shows the locked CTA when this is `true`.
  locked: boolean;
  groupId: string | null;
  peerId: string | null;
  /// ISO-8601 expiry, or `null` when locked. The renderer can use
  /// this to show a "renew soon" CTA when expiry is imminent.
  expiresAt: string | null;
}

/// Dev-only payload accepted by the optional
/// `KChatBridge.devMintMembership` endpoint. The mint runs against
/// a deterministic in-process Ed25519 issuer derived from
/// `issuerSeed`. Same seed produces the same trust root across
/// runs (intended — useful for reproducible local-LAN dev sessions).
///
/// **Never wire this into production builds.** The bridge gates
/// the underlying N-API export behind the `kchat-dev-issuer` cargo
/// feature, off by default; the preload IPC handler resolves to a
/// "not enabled" error in that case.
export interface KChatDevMintRequest {
  /// 32-byte Ed25519 seed used to derive the dev issuer, URL-safe
  /// base64 (no padding).
  issuerSeed: string;
  /// Group identifier. Same shape as `KChatInstallRequest.groupId`.
  groupId: string;
  /// 32-byte Ed25519 verifying key of the local peer, URL-safe
  /// base64 (no padding). Typically the persistent seed-derived
  /// public key the PresencePanel already stores.
  peerPublicKey: string;
  /// Membership validity, in seconds. Capped at 365 days by the
  /// `kcreate_kchat::MAX_DEV_VALIDITY` constant on the Rust side.
  validForSeconds: number;
}

/// Result of [`KChatBridge.deriveLocalIdentity`]. Mirrors
/// `kcreate_bridge::collab::KChatLocalIdentity`.
export interface KChatLocalIdentity {
  /// BLAKE3-derived peer id (URL-safe base64, no padding).
  peerId: string;
  /// 32-byte Ed25519 verifying key (URL-safe base64, no padding).
  peerPublicKey: string;
}

export interface KChatBridge {
  /// Install a verified KChat group authority. The supplied
  /// membership is re-verified on the Rust side before being
  /// stored, so a malformed payload from a buggy KChat client
  /// can't sneak past the gate.
  install(request: KChatInstallRequest): Promise<KChatMembershipStatus>;
  /// Clear the installed authority and re-lock multiplayer. Any
  /// running collab session is left as-is (call `session.leave()`
  /// first if you want to tear it down).
  clear(): Promise<KChatMembershipStatus>;
  /// Snapshot the current gate state. The `PresencePanel` polls
  /// this on mount to decide between the locked CTA and the
  /// live multiplayer UI.
  status(): Promise<KChatMembershipStatus>;
  /// Derive the local peer's (peerId, peerPublicKey) from the
  /// persistent Ed25519 seed. The sign-in panel uses this to
  /// pre-fill the membership-binding fields — the renderer
  /// doesn't have a native Ed25519 implementation, so the
  /// derivation has to happen on the Rust side.
  deriveLocalIdentity(seedB64: string): Promise<KChatLocalIdentity>;
  /// Probe whether the bridge was built with the dev issuer
  /// feature enabled. Returns `false` on production bridges. The
  /// renderer uses this to decide whether to surface the "Mint
  /// dev membership" affordance in the sign-in panel — production
  /// builds should not see any dev-only UI.
  devIssuerAvailable(): Promise<boolean>;
  /// Dev-only: mint a fresh KChat attestation locally and return
  /// a [`KChatInstallRequest`] the caller can pass right back into
  /// [`install`]. Rejects with a typed error when the bridge is
  /// built without `kchat-dev-issuer`.
  devMintMembership(request: KChatDevMintRequest): Promise<KChatInstallRequest>;
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
      pdfImport: PdfImportBridge;
      plugin: PluginBridge;
      mcpPermission: McpPermissionBridge;
      color: ColorBridge;
      textFrame: TextFrameBridge;
      session: SessionBridge;
      kchat: KChatBridge;
    };
  }
}
