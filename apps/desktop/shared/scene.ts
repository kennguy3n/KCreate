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

/**
 * Phase B1 — Pen tool wire mirror of `kcreate_vector::PathSegment`.
 *
 * This is the document-side path command shape used by
 * {@link CanvasBridge.createPath} and (eventually) the Phase B3
 * node-editor read/write APIs. It is NOT the same as
 * {@link PathCommand} above — `PathCommand` is the renderer-side
 * scene wire (consumed by `parse_scene` to feed the GPU pipeline)
 * and uses a `type` discriminator with `cx`/`cy`/`c1x`/`c1y` flat
 * shorthand. `PathSegmentWire` is the document-side metadata wire
 * (round-tripped through serde on `Vec<PathSegment>`), uses an
 * `op` discriminator with nested `{x, y}` points, and stays in
 * lockstep with the Rust enum so adding a new variant in
 * `kcreate_vector::PathSegment` is a single point of change.
 *
 * The JSON shape is verified against serde by a Rust unit test in
 * `kcreate_bridge::document::tests::canvas_create_path_*`.
 */
export interface PathPointWire {
  x: number;
  y: number;
}

export type PathSegmentWire =
  | { op: "move_to"; x: number; y: number }
  | { op: "line_to"; x: number; y: number }
  | { op: "quad_to"; ctrl: PathPointWire; end: PathPointWire }
  | {
      op: "cubic_to";
      ctrl1: PathPointWire;
      ctrl2: PathPointWire;
      end: PathPointWire;
    }
  | { op: "close" };

/**
 * SVG-style fill rule, mirrors `kcreate_vector::FillRule` with the
 * `snake_case` serde rename.
 */
export type FillRuleWire = "non_zero" | "even_odd";

/**
 * Phase B3 — snapshot returned by `CanvasBridge.pathGetSegments` for
 * the node editor's anchor/handle overlay. Mirrors
 * `crates/kcreate_bridge/src/document.rs::PathSnapshot`.
 *
 * `segments` carry the path's intrinsic (path-local) geometry —
 * the same wire shape `createPath` accepts. `closed` /
 * `fillRule` mirror `VectorPath.closed` / `VectorPath.fill_rule`.
 *
 * `translationX` / `translationY` carry the node's current
 * `transform.tx` / `transform.ty` so the renderer can project
 * path-local anchors into world space without a second IPC round
 * trip. **World position = path-local + translation.** The node
 * editor preserves this contract by NOT folding the translation
 * into the path coordinates on edit; `canvas.moveNode` keeps
 * owning the translation so undo of a node move stays a clean
 * transform patch instead of a path-replace patch.
 *
 * Field names use camelCase here (TS convention) but the Rust
 * struct serializes them as `translation_x` / `translation_y`;
 * the bridge re-shapes between the two when crossing N-API.
 */
export interface PathSnapshot {
  segments: PathSegmentWire[];
  closed: boolean;
  fillRule: FillRuleWire;
  translationX: number;
  translationY: number;
}

/**
 * Phase B2 — Pathfinder boolean op wire token.
 *
 * Mirrors `kcreate_vector::BooleanOp`'s `serde(rename_all =
 * "snake_case")` discriminator. The bridge parses these as plain
 * strings (`canvas_path_boolean(op: &str, ...)`) — adding a new
 * variant means updating this union AND the bridge's match arm
 * AND `kcreate_vector::BooleanOp`, kept in lockstep by
 * `apply_patch_commands_match_dispatcher_arms` and the bridge's
 * own `PathBooleanError::InvalidOp` test.
 */
export type PathBooleanOp = "union" | "subtract" | "intersect" | "exclude";

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
   * Re-render the renderer's most recently published scene at the
   * current viewport and size, returning the new frame id — or `null`
   * when no scene has been published yet.
   *
   * The present surface calls this after a viewport (pan/zoom) or
   * resize change: those operations mark the renderer dirty but do not
   * by themselves rebuild a frame, and the document graph is owned by
   * the bridge, so the host repaints without shipping the scene back
   * across IPC. Mirrors `kcreate_bridge::state::render_current`.
   */
  renderCurrent(): Promise<number | null>;
  /**
   * Set the viewport pan/zoom **and** repaint the cached scene in one
   * IPC round trip, returning the new frame id — or `null` when no scene
   * has been published yet (or no renderer is attached).
   *
   * This is the present surface's pan/zoom hot path: it folds what were
   * two crossings (`setViewport` then `renderCurrent`) into one, doing
   * the viewport write and the repaint under a single renderer lock on
   * the Rust side. The viewport write only dirties the renderer when the
   * pan/zoom actually changes, so the returned id is a fresh frame for a
   * real interaction and the cached id for a no-op. Mirrors
   * `kcreate_bridge::state::set_viewport_and_render`.
   */
  setViewportAndRender(
    panX: number,
    panY: number,
    zoom: number,
  ): Promise<number | null>;
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
  /**
   * Monotonically-increasing revision counter sourced from
   * `kcreate_core::node::Node::version`. Bumped on every mutation
   * (including undo/redo and future collab events). Panels that
   * hydrate node-scoped data the `NodeInfo` payload deliberately
   * doesn't carry (`FillSection`'s `style.fill`,
   * `TextFramePanel`'s `text_frame_options`, `OpenTypePanel`'s
   * OpenType features) must key their fetch effect on
   * `[node.id, node.version]` so the effect refires after
   * undo/redo / remote-peer edits even when `node.id` is stable.
   * Typed as `number` (not `bigint`) because `Node::version` bumps
   * once per mutation and stays well below 2^53 in practice — see
   * the matching `version: f64` comment on the napi `NodeInfo`.
   */
  version: number;
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

/**
 * RGBA colour with channels in `[0.0, 1.0]`. Mirrors
 * `kcreate_core::node::RgbaColor`.
 */
export interface RgbaColor {
  r: number;
  g: number;
  b: number;
  a: number;
}

/**
 * A single stop in a gradient. Mirrors
 * `kcreate_core::node::GradientStop`.
 */
export interface GradientStop {
  offset: number;
  color: RgbaColor;
}

/**
 * 2D point. Mirrors `kcreate_core::node::Point2D`.
 */
export interface Point2D {
  x: number;
  y: number;
}

/**
 * Fill style for a node. Discriminated union tagged on `kind`
 * (snake-cased), mirroring `kcreate_core::node::FillStyle`. The
 * serde encoding is *internally-tagged*: newtype variants flatten
 * the inner type's fields into the same JSON object rather than
 * nesting them under a `"value"` key, so the wire shape is e.g.:
 *
 *   - `{"kind": "none"}`
 *   - `{"kind": "solid", "r": 0.5, "g": 0.25, "b": 0.75, "a": 1.0}`
 *   - `{"kind": "gradient", "shape": "linear", "from": {...},
 *      "to": {...}, "stops": [...]}`
 *   - `{"kind": "gradient", "shape": "radial", "center": {...},
 *      "radius": 0.5, "stops": [...]}`
 *
 * The `shape` discriminator on `"gradient"` is from the inner
 * `GradientKind` enum, which uses its own `#[serde(tag = "shape")]`.
 * The two tags don't collide because they live on independent
 * Rust enums; serde flattens them into one object at the JSON
 * level.
 *
 * The `FillSection` (right panel, properties tab) reads/writes
 * this shape via `kcreate.document.nodeFill` and the new `fill`
 * field on `UpdateNodeProps`.
 */
export type FillStyle =
  | { kind: "none" }
  | ({ kind: "solid" } & RgbaColor)
  | ({ kind: "gradient" } & GradientKind);

/**
 * Variant of `FillStyle::Gradient`. Tagged on `shape`,
 * snake-cased. Mirrors `kcreate_core::node::GradientKind`. The
 * payload fields are flattened alongside the parent `FillStyle`'s
 * `kind` field when this is the inner variant of `FillStyle`.
 */
export type GradientKind =
  | {
      shape: "linear";
      from: Point2D;
      to: Point2D;
      stops: GradientStop[];
    }
  | {
      shape: "radial";
      center: Point2D;
      radius: number;
      stops: GradientStop[];
    };

/**
 * Optional changes accepted by `updateNode`. Only present fields
 * are applied.
 *
 * `fill` is decoupled from `metadata` so the FillEditor doesn't
 * have to know that the fill lives on `node.style.fill` rather
 * than `node.metadata`. The bridge owns that layering detail.
 */
export interface UpdateNodeProps extends CreateNodeProps {
  fill?: FillStyle;
  /// Phase 5 Block C Task 17 — additional fills layered on top of
  /// the primary `fill`. Sending `[]` clears the list; omitting
  /// the field leaves it untouched.
  extra_fills?: FillStyle[];
  /// Phase 5 Block C Task 17 — additional strokes layered above
  /// the primary stroke.
  extra_strokes?: StrokeStyleWire[];
  /// Phase 5 Block D Task 23 — toggle the overprint flag.
  overprint?: boolean;
}

/// Wire shape for a stroke. Mirrors `kcreate_core::node::StrokeStyle`.
export interface StrokeStyleWire {
  color: RgbaColor;
  width: number;
  cap?: "butt" | "round" | "square";
  join?: "miter" | "round" | "bevel";
  dash?: number[];
}

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
 * Summary of a discarded redo tail, surfaced to the renderer so the
 * branch panel can offer "recover branch" affordances. Mirrors
 * `crates/kcreate_bridge/src/document.rs::DiscardedBranchSummary`.
 *
 * - `anchorPosition`: the timeline cursor index this branch would
 *   re-attach to if restored. Stale anchors (the user did more work
 *   after the branch was captured) cause
 *   {@link DocumentBridge.restoreDiscardedBranch} to return `false`.
 * - `opCount`: number of ops in the discarded tail. For grouped
 *   compound operations this is the size of the group.
 * - `discardedAtIso`: RFC 3339 UTC timestamp; the panel sorts
 *   most-recent-first using this field.
 * - `firstCommand`: stable `Operation::command` of the first op in
 *   the discarded tail. Used as a one-line preview ("Recover:
 *   artboard_create").
 */
export interface DiscardedBranchSummary {
  anchorPosition: number;
  opCount: number;
  discardedAtIso: string;
  firstCommand: string;
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
   * Phase 11 Block D Task 21 — monotonic workspace version counter.
   * Bumps on every mutation (create / update / delete / undo / redo /
   * reparent / paste). Renderer pollers compare two snapshots of
   * this value to skip the full `getDocumentTree` IPC round-trip
   * when the document hasn't changed since the last paint. The Rust
   * implementation is a single `AtomicU64::load`, so calling this
   * at 60Hz is free.
   */
  getDocumentVersion(): Promise<number>;
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
  /**
   * Read the current `FillStyle` for a node, or `null` when the
   * node id is not in the open document. Used by the `FillSection`
   * panel to populate its editor on selection change. Writes go
   * back through {@link updateNode} with the new `fill` field on
   * `UpdateNodeProps` — there's no separate setter because the
   * existing channel already takes care of the operation log,
   * scene re-sync, and persistence.
   */
  nodeFill(nodeId: string): Promise<FillStyle | null>;
  /// Phase 5 Block C Task 17. Read the node's `extra_fills`
  /// stack. Returns `null` only when the id is unknown; an empty
  /// array means "no extras yet".
  nodeExtraFills(nodeId: string): Promise<FillStyle[] | null>;
  /// Phase 5 Block C Task 17. Read the node's `extra_strokes`
  /// stack.
  nodeExtraStrokes(nodeId: string): Promise<StrokeStyleWire[] | null>;
  deleteNode(nodeId: string): Promise<void>;
  /**
   * Phase 6 Tasks 27-28 — install or clear a layer-colour tag on
   * `nodeId`. Pass a non-empty string to install (the bridge
   * canonicalises whitespace + case before storing under
   * `Node::metadata["layerColor"]`); pass `null` to clear. Returns
   * the node's new `version` so renderer-side effects keyed on
   * `[id, version]` re-fire without a full `getTree()` round-trip.
   */
  setLayerColor(nodeId: string, color: string | null): Promise<number>;
  undo(): Promise<UndoRedoOutcome | null>;
  redo(): Promise<UndoRedoOutcome | null>;
  /**
   * Group-aware undo (Phase 6 Task 15). Consumes the entire
   * contiguous run of ops at the head of the undo stack that share
   * the same `group_id` — a `drag-to-move` sequence recorded as 50
   * `canvas_move_node` ops with the same group id undoes as a single
   * user action. Falls back to single-op undo when the head op
   * carries no group id. Resolves to `null` when no project is
   * loaded or the stack is empty. Mirrors `document_undo_group` on
   * the Rust bridge — see `crates/kcreate_bridge/src/document.rs`.
   */
  undoGroup(): Promise<UndoRedoOutcome | null>;
  /** Symmetric with {@link undoGroup}. */
  redoGroup(): Promise<UndoRedoOutcome | null>;
  /**
   * Newest-first list of redo tails that were dropped because the
   * user pushed a new op after undoing some history (Phase 6
   * Task 16). The branch panel uses this list to offer "recover
   * branch" UX. Bounded by `OperationLog::max_branches` (16 by
   * default).
   */
  listDiscardedBranches(): Promise<DiscardedBranchSummary[]>;
  /**
   * Restore the discarded branch at `indexFromBack` (0 = newest, as
   * listed by {@link listDiscardedBranches}). Returns `true` on
   * success, `false` if the index is out of range OR the branch's
   * `anchorPosition` no longer matches the current undo cursor.
   * Restored ops appear at the head of the redo stack — the user
   * presses Redo / Ctrl+Y to replay them.
   */
  restoreDiscardedBranch(indexFromBack: number): Promise<boolean>;

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
  /// Phase 4 hard gate: when `false`, the renderer MUST NOT show
  /// any image-generation UI at all (no Generate button, no
  /// generation packs in the model manager, no menu entries).
  /// Combines tier ≥ 2 AND GPU availability.
  imageGenerationAllowed: boolean;
  /// Phase 4 per-tier ceiling on vision-model size in MB. Separate
  /// from `effectiveMaxModelMb` because the vision sidecar runs in
  /// a separate process from the text LLM and tier 0/1 can afford
  /// a 180 MB SmolVLM even though they can't afford a 4 GB LLM.
  ///
  /// Unit: **binary MB** (mebibytes, 1024 × 1024 B). Matches the
  /// Rust side (`kcreate_bridge::phase4::vision_listable_packs` /
  /// `model_registry`) so a pack's `sizeBytes / (1024*1024)` can
  /// be compared directly to this cap. Decimal-MB callers will
  /// disagree by ~2.4% near tier boundaries.
  ///
  /// This is the **effective** cap (i.e. halved when
  /// `lowResourceMode` is true), so it always agrees with what
  /// `vision_listable_packs` and `spawn_vision` will actually
  /// enforce on the Rust side. Do NOT apply your own
  /// low-resource halving on top — that would double-discount.
  visionModelMaxMb: number;
  /// `Debug` form of the host `Platform` enum
  /// (`"MacOsAppleSilicon"`, `"Linux"`, `"Windows"`, …). Phase 12
  /// removed the MLX sidecar so this is no longer used to gate
  /// `_mlx`-suffixed packs (the registry no longer ships any),
  /// but the field stays in the wire shape because a future
  /// platform-aware optimisation (e.g. a Vulkan-only image gen
  /// pack) will need it again.
  platform: string;
}

/**
 * Phase 8 Block E Task 27 — one entry in `StartupTimelineReport.marks`.
 * `monotonicNs` is the nanosecond offset from the monotonic clock
 * anchor captured when the timeline was created (Rust-side
 * `Instant::now()` at `Timeline::start`), NOT from `startedAtUnixMs`.
 * The wall-clock `startedAtUnixMs` is only kept for human correlation
 * with system logs; never mix the two by adding them (the units don't
 * line up — one is monotonic ns since timeline construction, the other
 * is wall-clock ms since epoch). All per-mark and per-phase durations
 * are computed monotonically so resume-from-sleep cannot make them go
 * backwards.
 */
export interface StartupMark {
  label: string;
  monotonicNs: number;
}

/** Phase 8 Block E Task 27 — one derived `phase` interval. */
export interface StartupPhase {
  label: string;
  fromNs: number;
  toNs: number;
  durationNs: number;
}

/** Phase 8 Block E Task 27 — full startup-timeline snapshot. */
export interface StartupTimelineReport {
  name: string;
  startedAtUnixMs: number;
  totalNs: number;
  marks: StartupMark[];
  phases: StartupPhase[];
}

/** Phase 8 Block E Task 28 — tile-cache snapshot. */
export interface TileCacheStats {
  bytes: number;
  entries: number;
  budgetBytes: number;
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
   * Phase 8 Block E Task 27 — startup-perf profiling.
   *
   * Snapshot of the process-wide startup timeline owned by the
   * bridge (`kcreate_perf::startup`). Returns `null` when the
   * timeline has never been initialised — the renderer can then
   * skip showing the diagnostics overlay. Each entry in
   * `marks` is `{ label, monotonicNs }`; each entry in `phases`
   * is `{ label, fromNs, toNs, durationNs }`. All durations
   * are monotonic — never wall-clock — so resume from sleep
   * cannot make them go backwards.
   */
  startupTimeline(): Promise<StartupTimelineReport | null>;
  /**
   * Drop a named mark on the global startup timeline from the
   * renderer (e.g. `first_paint`, `first_interactive`). The
   * mark joins the same monotonic timeline as the bridge's own
   * cold-path marks so a single report tells the full story.
   */
  startupMark(label: string): Promise<void>;
  /**
   * Phase 8 Block E Task 28 — tile-cache observability.
   *
   * Snapshot of the bridge-owned LRU tile cache. `bytes` and
   * `budgetBytes` are byte counts (not MB); `entries` is the
   * number of cached tile slots.
   */
  tileCacheStats(): Promise<TileCacheStats>;
  /**
   * Drop every entry from the process-wide tile cache. Returns
   * the count of entries that were evicted so the UI can show
   * `Freed N tiles`.
   */
  tileCacheClear(): Promise<number>;
  /**
   * Write a UTF-8 text file at `path`. The host requires the path to
   * be inside either the OS temp directory returned by `tempDir()`
   * OR a directory the user has explicitly approved this session via
   * `chooseExportTarget` / `chooseExportDirectory` — any other
   * location is rejected. This lets the renderer land sidecar files
   * (e.g. design-token JSON for a dev handoff) next to a user-
   * picked primary export without granting it write access to
   * arbitrary paths. Returns the number of bytes written.
   */
  writeTextFile(path: string, content: string): Promise<number>;
  /**
   * Phase A2 — native save-as dialog. Wraps Electron's
   * `dialog.showSaveDialog` with per-format extension filters and
   * an optional initial directory.
   *
   * - `format` is the wire-format export-format name (`"png"`,
   *   `"svg"`, `"pdf"`, `"webp"`, `"jpeg"`); the filter list and
   *   the dialog title are derived from it.
   * - `defaultName` is the basename (with extension) the dialog
   *   pre-populates — e.g. `kcreate-export-1735574000.png`.
   * - `defaultDir` (optional) is the directory the dialog opens in.
   *   The renderer typically passes
   *   `Preferences.export.lastDirByFormat[format]` so the user
   *   doesn't have to re-navigate each time.
   *
   * Returns the absolute chosen path, or `null` if the user
   * cancelled (the renderer must short-circuit the export in
   * that case — no temp-dir fallback).
   */
  chooseExportTarget(
    format: string,
    defaultName: string,
    defaultDir: string | null,
  ): Promise<string | null>;
  /**
   * Sibling to `chooseExportTarget` for batch presets that emit
   * multiple files into a shared directory. Wraps
   * `dialog.showOpenDialog` with `properties: ['openDirectory',
   * 'createDirectory']`. Returns the absolute chosen directory,
   * or `null` if the user cancelled.
   */
  chooseExportDirectory(defaultDir: string | null): Promise<string | null>;
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
  /// In-memory variant of [`importImage`] that takes raw encoded
  /// bytes (PNG / JPEG / WebP). Used by the Phase 4 image-gen flow
  /// to insert generated PNGs without round-tripping through a
  /// temp file.
  importImageBytes(
    parentId: string | null,
    bytes: Uint8Array,
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
  /**
   * Phase B1 — Pen tool commit path.
   *
   * Insert a freehand vector path from a caller-built segment
   * list. Mirrors `kcreate_bridge::document::canvas_create_path`.
   *
   * `segments` is serialized to JSON and passed across the IPC
   * boundary unchanged — the bridge re-deserializes via serde so
   * the TS wire shape MUST match
   * `kcreate_vector::PathSegment`'s serde-internally-tagged
   * representation. See {@link PathSegmentWire} for the
   * authoritative type.
   *
   * `closed` becomes `VectorPath.closed` (whether the renderer
   * joins the last point back to the first for fill / hit-test
   * purposes — independent of whether the caller appended an
   * explicit `{op:"close"}` segment).
   *
   * `name` is the layer name shown in the layers panel; default
   * `"Path"`.
   *
   * Throws:
   * - `InvalidArg` when `segments` parses but is empty.
   * - `InvalidArg` when the first segment is not `move_to`.
   * - `InvalidArg` when `segments` is not valid JSON for
   *   `Vec<PathSegment>`.
   */
  createPath(
    parentId: string | null,
    segments: PathSegmentWire[],
    closed: boolean,
    name?: string | null,
  ): Promise<string>;
  /**
   * Phase B2 — Pathfinder gesture.
   *
   * Apply a polygon boolean (`union` / `subtract` / `intersect` /
   * `exclude`) across the given source vector layers, replacing
   * them with the resulting shape(s). Destructive: the source
   * nodes are removed and replaced by one or more new
   * `VectorLayer` nodes that inherit the *first* source's style.
   *
   * `sourceIds` must contain at least two ids, each pointing at a
   * `VectorLayer` with a vector path payload. The bridge folds
   * left-to-right via `kcreate_vector::boolean_operation` (matches
   * Inkscape's `Path > Union` semantics — see
   * `canvas_path_boolean` for the full contract).
   *
   * Returns the freshly-inserted result node ids in iteration
   * order so the renderer can re-select them all and preserve the
   * boolean's shape ordering.
   *
   * Throws (each maps to a distinct `PathfinderPanel` toast):
   * - `PathBoolean::InvalidOp` — `op` is not one of the four
   *   wire tokens.
   * - `PathBoolean::TooFewSources` — fewer than 2 ids.
   * - `PathBoolean::SourceNotFound` — id does not match any node.
   * - `PathBoolean::SourceNotVector` — source is not a VectorLayer.
   * - `PathBoolean::SourceMissingPath` — source has no path metadata.
   * - `PathBoolean::Vector` — `boolean_operation` rejected the
   *   inputs (e.g. polyline flattening produced no contour).
   * - `PathBoolean::EmptyResult` — op produced zero shapes (e.g.
   *   `intersect` on non-overlapping inputs).
   */
  pathBoolean(op: PathBooleanOp, sourceIds: string[]): Promise<string[]>;
  /**
   * Phase B3 — read a `VectorLayer`'s geometry into a
   * {@link PathSnapshot} for the node editor's anchor/handle
   * overlay. The returned `segments` are path-local; the
   * `translationX` / `translationY` carry the node's
   * `transform.tx` / `transform.ty` so the renderer can project
   * each anchor into world space without a second IPC.
   *
   * Read-only — records NO operation in the undo log. Caller is
   * the node-edit state machine's tool-entry handler.
   *
   * Rejects with `Status::InvalidArg` for every
   * `PathSegmentsError` variant (`NodeNotFound`, `NotVectorLayer`,
   * `MissingPathMetadata`) so the renderer's typed-toast path can
   * tell the user "the layer disappeared, please re-select" vs.
   * the generic "internal error" toast.
   */
  pathGetSegments(nodeId: string): Promise<PathSnapshot>;
  /**
   * Phase B3 — write new geometry to a `VectorLayer` from the
   * node editor. `segments` are path-local (intrinsic to the
   * path, NOT world-space); the bridge recomputes `node.bounds`
   * from them but leaves `transform.tx` / `transform.ty`
   * untouched so `canvas.moveNode` keeps owning node position.
   *
   * Records ONE undoable `canvas_path_set_segments` operation
   * per call. Callers MUST coalesce pointermove-rate updates
   * into a single end-of-gesture call so the operation log stays
   * coarse-grained (one drag = one undo step) — matches the
   * `canvas.moveNode` discipline.
   *
   * Rejects with `Status::InvalidArg` for `PathSegmentsError`
   * variants `NodeNotFound`, `NotVectorLayer`, `InvalidJson`,
   * `Empty`, and `MissingMoveTo` — same routing as `createPath`
   * and `pathBoolean`.
   */
  pathSetSegments(
    nodeId: string,
    segments: PathSegmentWire[],
    closed: boolean,
  ): Promise<void>;
  createText(
    parentId: string | null,
    x: number,
    y: number,
    text: string,
    fontFamily: string,
    fontSize: number,
  ): Promise<string>;
  /**
   * Atomic batch creation of canvas primitives. The bridge takes
   * the workspace write lock once, inserts every item in
   * submission order, records one operation per item against the
   * single-item `op_kind` (so undo/redo granularity matches the
   * non-batch path), and runs a single `sync_scene` before
   * releasing the lock.
   *
   * Each item may carry optional `fill` and `name` fields which
   * are stamped onto the node *before* it is inserted into the
   * graph — so the batch never has to round-trip through
   * `document.updateNode` to colour or label a node. Returns the
   * new node ids in the same order as `items`. Empty input is a
   * no-op that returns `[]` without taking the lock.
   *
   * Mirrors `canvas_create_nodes` /
   * `kcreate_bridge::document::CanvasBatchItem`.
   */
  createNodes(items: CanvasBatchItem[]): Promise<string[]>;
  moveNode(nodeId: string, dx: number, dy: number): Promise<void>;
}

/**
 * One item in a {@link CanvasBridge.createNodes} batch. Internally
 * tagged on `kind` (matching the {@link FillStyle} wire shape) so a
 * single tagged-union array can hold heterogeneous primitives. All
 * variants accept optional `fill` and `name`; the bridge stamps
 * both onto the node before insert so no follow-up `updateNode`
 * round-trip is required to colour or label a freshly-created node.
 *
 * Mirrors `kcreate_bridge::document::CanvasBatchItem`.
 */
export type CanvasBatchItem =
  | {
      kind: "rect";
      parent: string | null;
      x: number;
      y: number;
      w: number;
      h: number;
      fill?: FillStyle;
      name?: string;
    }
  | {
      kind: "ellipse";
      parent: string | null;
      cx: number;
      cy: number;
      rx: number;
      ry: number;
      fill?: FillStyle;
      name?: string;
    }
  | {
      kind: "line";
      parent: string | null;
      x1: number;
      y1: number;
      x2: number;
      y2: number;
      fill?: FillStyle;
      name?: string;
    }
  | {
      kind: "text";
      parent: string | null;
      x: number;
      y: number;
      body: string;
      family: string;
      size: number;
      fill?: FillStyle;
      name?: string;
    };

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
  /**
   * Phase C — recommended LLM pack id for the current device tier
   * (one of `llm_bonsai_1_7b` / `llm_bonsai_4b` / `llm_bonsai_8b`).
   * Empty string when the registry has no recommendation, which is
   * expected never on a supported device.
   */
  recommendedPack(): Promise<string>;
}

// -----------------------------------------------------------------------------
// Phase C — onboarding / first-run welcome bridge
// -----------------------------------------------------------------------------

/**
 * Progress event emitted on every ~256 KiB of downloaded bytes
 * while the welcome modal's one-click install is running. The
 * shape mirrors `apps/desktop/main/src/onboardingDownloader.ts`
 * (the channel is in the main process, not the Rust bridge).
 */
export interface OnboardingProgress {
  packId: string;
  phase:
    | "resolving"
    | "connecting"
    | "downloading"
    | "verifying"
    | "installing"
    | "done"
    | "error"
    | "cancelled";
  receivedBytes: number;
  totalBytes: number | null;
  message: string;
}

/**
 * Result returned by `onboarding.installRecommendedPack()` on
 * success. Mirrors the Rust `InstallReport`: `verified=true` means
 * the registry pinned a SHA-256 and the downloaded bytes match;
 * `verified=false` means the registry has no pinned hash yet so
 * the actual hash is reported for the user's records.
 *
 * Field naming is `camelCase` to match
 * `kcreate_ai::InstallReport`'s `#[serde(rename_all =
 * "camelCase")]` serialisation. The Rust JSON is shipped through
 * the bridge → main → preload → renderer pipeline unchanged, so
 * the field names here MUST match the on-wire keys exactly.
 * See `install_report_serialises_to_camelcase_wire_format` in
 * `crates/kcreate_ai/src/model_registry.rs` for the lockstep
 * pin.
 */
export interface OnboardingInstallReport {
  packId: string;
  verified: boolean;
  actualSha256: string;
  sizeBytes: number;
}

/**
 * Phase C — one-click recommended-pack download + install. The
 * renderer NEVER sees a URL; the main process resolves the pack
 * id via `llm.recommendedPack()`, validates the registry URL
 * against the host allow-list, streams to a per-process temp
 * file, then hands it to `aiModel.installModelPack`'s same
 * SHA-256 verify + atomic rename path.
 */
export interface OnboardingBridge {
  installRecommendedPack(): Promise<OnboardingInstallReport>;
  cancelInstall(): Promise<void>;
  /**
   * Subscribe to progress events. Returns an unsubscribe handle —
   * the renderer must call it on cleanup to avoid leaking IPC
   * listeners across re-renders.
   */
  onInstallProgress(fn: (progress: OnboardingProgress) => void): () => void;
}

/**
 * Phase C — narrow system surface for the welcome modal's "Open
 * download page" fallback. Only HTTPS URLs whose hostname is in
 * `apps/desktop/main/src/onboardingDownloader.ALLOWED_HOSTS` are
 * accepted; everything else throws in the main process.
 */
export interface SystemBridge {
  openExternal(url: string): Promise<void>;
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

// `RgbaColor` is declared once above near the `FillStyle` mirror; the
// design-tokens section reuses that single declaration so a future
// schema change (e.g. wide-gamut floats, alpha-premultiplied semantics)
// only needs to be threaded through one place.

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
  /// Phase 5 Block D Task 21. Serialize the brand kit (plus its
  /// referenced font / logo blobs) into a `.kbrand` ZIP archive at
  /// `outputPath`.
  export(kitId: string, outputPath: string): Promise<void>;
  /// Import a `.kbrand` archive. Embedded font / logo blobs are
  /// stored in the project's asset table under fresh ids; a new
  /// `BrandKit` referencing those assets is appended. Returns the
  /// new kit's id.
  import(filePath: string): Promise<string>;
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
  /**
   * Phase 11 Block C Task 17 — Smart Animate snapshot. Read-only;
   * returns the before/after property snapshots the
   * `PrototypePlayer` interpolates over a variant switch. The
   * caller commits the swap with `switchVariant` after the
   * animation finishes so the document graph is only mutated
   * once per user gesture.
   */
  smartAnimateSnapshot(
    nodeId: string,
    targetVariantId: string,
  ): Promise<SmartAnimateSnapshot>;
  /** Detach an instance — turns the `ComponentLayer` into a plain group. */
  detach(nodeId: string): Promise<void>;
}

/**
 * Phase 11 Block C Task 17 — one entry of [`SmartAnimateSnapshot`].
 * Mirrors `kcreate_bridge::document::SmartAnimateLayer`.
 */
export interface SmartAnimateLayer {
  name: string;
  bounds: Bounds;
  opacity: number;
  /** `#RRGGBB` for solid fills, `null` for gradients / images. */
  fill_color: string | null;
  corner_radius: number;
}

/**
 * Phase 11 Block C Task 17 — paired property snapshots for Smart
 * Animate. Mirrors `kcreate_bridge::document::SmartAnimateSnapshot`.
 */
export interface SmartAnimateSnapshot {
  before: SmartAnimateLayer[];
  after: SmartAnimateLayer[];
}

// ============================================================================
// Prototype interactions (mirrors `kcreate_core::node::Interaction`)
// ============================================================================

/**
 * Mirrors `kcreate_core::InteractionTrigger`.
 *
 * Phase 11 added `mouse_enter` / `mouse_leave` and the data-carrying
 * `{ kind: "after_delay", ms }` variant. Pre-Phase-11 projects emit
 * the simple variants as bare snake_case strings, so the union keeps
 * those forms to preserve backward compatibility. The renderer-side
 * `InteractionPanel.tsx` and `PrototypePlayer.tsx` accept either
 * shape; the bridge's `interaction.add` accepts the bare string for
 * simple variants and a `JSON.stringify`'d object for `after_delay`.
 */
export type InteractionTrigger =
  | "click"
  | "hover"
  | "press"
  | "mouse_enter"
  | "mouse_leave"
  | { kind: "after_delay"; ms: number };

export type AnimationType =
  | "instant"
  | "dissolve"
  | "slide_in"
  | "slide_out"
  | "push"
  | "move_in";

export type SlideDirection = "left" | "right" | "up" | "down";

export type EasingCurve =
  | { kind: "linear" }
  | { kind: "ease_in" }
  | { kind: "ease_out" }
  | { kind: "ease_in_out" }
  | { kind: "spring"; stiffness: number; damping: number }
  | { kind: "cubic_bezier"; x1: number; y1: number; x2: number; y2: number };

/**
 * Mirrors `kcreate_core::Transition`. The bridge persists this on
 * every navigation-style `InteractionAction`. All fields are
 * optional in the wire format (Rust uses `#[serde(default)]`),
 * which lets pre-Phase-11 projects round-trip unchanged.
 */
export interface Transition {
  animation: AnimationType;
  duration_ms: number;
  easing: EasingCurve;
  direction?: SlideDirection | null;
}

export type InteractionAction =
  | {
      kind: "navigate_to";
      target_artboard_id: string;
      transition?: Transition;
    }
  | { kind: "scroll_to"; target_node_id: string }
  | {
      kind: "open_overlay";
      overlay_artboard_id: string;
      transition?: Transition;
    }
  | { kind: "close_overlay" }
  | { kind: "back" }
  | {
      kind: "switch_variant";
      variant_id: string;
      transition?: Transition;
    };

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
// Phase 3 — Local template marketplace (Tasks 11-12).
//
// Mirrors `kcreate_core::marketplace::{TemplateSource, TemplateManifest,
// MarketplaceError}`. The renderer's TemplateMarketplace panel calls
// `window.kcreate.templateMarketplace.{list,installLocal,remove}` to
// surface the contents of `~/.kcreate/templates/` (or whichever
// directory is pointed to by the `KCREATE_TEMPLATE_DIR` env var the
// bridge respects).
// ---------------------------------------------------------------------------

/**
 * Mirror of `kcreate_core::marketplace::TemplateSource` —
 * tagged-union with a single Phase 3 variant for local-on-disk
 * templates. A future remote-marketplace variant would live here
 * alongside `Local`.
 */
export type TemplateSource = {
  type: "local";
  /** Absolute path of the `.ktemplate/` folder on the user's disk. */
  path: string;
};

/**
 * Manifest of an installed template, mirrors
 * `kcreate_core::marketplace::TemplateManifest`. Wire-format lockstep
 * (AGENTS.md rule 4).
 */
export interface TemplateManifest {
  id: string;
  name: string;
  description: string;
  category: TemplateCategory;
  tags: string[];
  /** Relative path to a thumbnail image inside the template folder. */
  thumbnail: string | null;
  page_count: number;
  author: string | null;
  version: string;
  source: TemplateSource | null;
}

/** Mirror of `kcreate_bridge::phase2::TemplateListReport`. */
export interface TemplateListReport {
  templates: TemplateManifest[];
}

export interface TemplateMarketplaceBridge {
  /**
   * List installed templates from the marketplace directory.
   * `category` filters by `TemplateCategory` discriminant;
   * `query` filters by case-insensitive substring against name,
   * tag, or description. When both are supplied, the renderer's
   * convention is that a non-empty query overrides the category
   * (search bar dominates).
   */
  list(
    category?: TemplateCategory,
    query?: string,
  ): Promise<TemplateListReport>;
  /**
   * Install a `.ktemplate/` folder from `sourcePath` into the
   * marketplace root (copies the directory). Returns the installed
   * manifest. Rejects with `Status::InvalidArg` if the source has no
   * valid manifest or the same id is already installed.
   */
  installLocal(sourcePath: string): Promise<TemplateManifest>;
  /**
   * Remove an installed template by id — deletes the `.ktemplate/`
   * folder on disk. Rejects with `Status::InvalidArg` for an
   * unknown id.
   */
  remove(templateId: string): Promise<void>;
}

// ---------------------------------------------------------------------------
// Phase 6 — Audit log (Tasks 13–14)
// ---------------------------------------------------------------------------

/** Discriminator for `AuditEventKind`. */
export type AuditEventKindTag =
  | "operation"
  | "ai_action"
  | "project"
  | "collab"
  | "other";

/** Condensed operation record inside an audit event. */
export interface AuditOperationRecord {
  op_id: string;
  command: string;
  ai_generated: boolean;
}

/** Project lifecycle action payload. */
export type AuditProjectAction =
  | { action: "open"; path: string }
  | { action: "close" }
  | { action: "save" }
  | { action: "export"; format: string; destination: string };

/**
 * Phase 7 (Task 20): collaboration session action payload mirroring
 * `kcreate_audit::event::CollabAction`. Each variant maps to a
 * single bridge-level transition (session start, peer kick, …) so
 * the renderer can render specific human-readable strings.
 *
 * Field names are snake_case to match the Rust struct's serde
 * output (the enum carries `#[serde(rename_all = "snake_case")]`
 * which renames both variant names and field names).
 */
export type AuditCollabAction =
  | { action: "session_started"; community_id: string | null }
  | { action: "session_left" }
  | { action: "peer_joined"; peer_id: string; display_name: string }
  | { action: "peer_left"; peer_id: string }
  | { action: "peer_kicked"; peer_id: string; reason: string }
  | { action: "operation_received"; peer_id: string; op_count: number }
  | { action: "conflict_resolved"; node_id: string }
  | { action: "kchat_backend_status"; status: string };

/** Discriminated union matching `kcreate_audit::AuditEventKind`. */
export type AuditEventKind =
  | { type: "operation" } & AuditOperationRecord
  | {
      type: "ai_action";
      action_type: string;
      model: string;
      compute_device: string;
      prompt: string | null;
    }
  | { type: "project" } & AuditProjectAction
  | { type: "collab" } & AuditCollabAction
  | { type: "other"; label: string; payload: unknown };

/** One row from the audit log. */
export interface AuditEvent {
  id: string;
  timestamp: string;
  actor: string;
  project_id: string | null;
  affected_nodes: string[];
  kind: AuditEventKind;
}

/** Filter for `audit.query()`. All fields optional — empty = match all. */
export interface AuditQuery {
  since?: string;
  until?: string;
  kind?: AuditEventKindTag;
  project_id?: string;
  affected_node?: string;
  limit?: number;
}

/** Result of `audit.query()`. */
export interface AuditQueryReport {
  events: AuditEvent[];
  total: number;
}

export interface AuditBridge {
  /** Record an audit event. Returns the event's UUID. */
  record(event: AuditEvent): Promise<string>;
  /** Query the audit log. */
  query(filter: AuditQuery): Promise<AuditQueryReport>;
  /** Total row count in the audit log. */
  count(): Promise<number>;
  /** Delete rows older than `cutoffIso` (RFC 3339). Returns rows removed. */
  purge(cutoffIso: string): Promise<number>;
  /** Filesystem path of the current audit database. */
  path(): Promise<string>;
}

// ---------------------------------------------------------------------------
// Phase 6 — Tasks 17-18: Lazy thumbnail cache + recent-projects.
//
// The N-API surface lives in `kcreate_bridge::lib::{thumbnail_for_cover,
// thumbnail_for_page, thumbnail_prepare_background, recent_projects_list,
// recent_project_cover_bytes}`; the wire types below mirror the
// corresponding `#[napi(object)]` structs (`ThumbnailBytes`,
// `RecentProjectInfo`, `RecentProjectCoverInfo`).
//
// `bytesBase64` is a standard (non-URL-safe) base64-encoded PNG. The
// HomePage assembles `data:${mime};base64,${bytesBase64}` and pins it
// as the `src` on an `<img>` so React can rely on browser-native
// decoding + caching without a `Blob`/`createObjectURL` round-trip
// (which would leak across HMR reloads).
// ---------------------------------------------------------------------------

export interface ThumbnailBytes {
  width: number;
  height: number;
  mime: string;
  byteSize: number;
  bytesBase64: string;
  /** BLAKE3 hex content hash of the encoded bytes. */
  contentHash: string;
}

/** Cover-thumbnail metadata (no pixel bytes — see `recentProjectCoverBytes`). */
export interface RecentProjectCoverInfo {
  width: number;
  height: number;
  mime: string;
  byteSize: number;
  contentHash: string;
}

/** One entry on the persistent recent-projects roster. */
export interface RecentProjectInfo {
  /** Absolute path to the `.kstudio` directory. */
  path: string;
  /** Display name from the project manifest. */
  name: string;
  /** Manifest UUID as a hex string. Matches `ProjectInfo.id`. */
  projectId: string;
  /** RFC 3339 UTC of the last project mutation. */
  modifiedAt: string;
  /** RFC 3339 UTC of the most recent open / create through the bridge. */
  lastOpenedAt: string;
  /** Best-effort cover-thumbnail metadata. `null` when none is cached. */
  cover: RecentProjectCoverInfo | null;
}

export interface ThumbnailBridge {
  /**
   * Ensure the current project has a cover thumbnail on disk and
   * return its bytes. On a cache hit no rendering is performed.
   * `maxDimPx === 0` means "use the default" (320 px on the long
   * edge — see `kcreate_bridge::thumbnails::DEFAULT_THUMBNAIL_MAX_DIM_PX`).
   * Errors with `NoProject` when no project is open.
   */
  forCover(maxDimPx: number): Promise<ThumbnailBytes>;
  /**
   * Same shape as `forCover`, but for a specific page node id.
   * Errors with `NodeNotFound` for unknown ids or `InvalidArgument`
   * when the id refers to a non-Page node.
   */
  forPage(pageId: string, maxDimPx: number): Promise<ThumbnailBytes>;
  /**
   * Spawn a background worker that pre-warms every page's thumbnail.
   * Returns immediately. Becomes a no-op when low-resource mode is
   * active (per ARCHITECTURE.md §14: "skip speculative thumbnails").
   */
  prepareBackground(maxDimPx: number): Promise<void>;
}

export interface RecentProjectsBridge {
  /** Snapshot the persistent recent-projects list (most-recent-first). */
  list(): Promise<RecentProjectInfo[]>;
  /**
   * Read the cached cover bytes for a project on the recent list
   * *without* opening the project. Returns `null` when no cover is
   * cached for that path (e.g. the user has never opened the project
   * since the cache was introduced).
   */
  coverBytes(projectDir: string): Promise<ThumbnailBytes | null>;
}

// ---------------------------------------------------------------------------
// Phase 2 — Preflight, Icon Pack, Batch Async, AI extras, Plugin sandbox,
// MCP permission persistence, Screenshot-to-Layout.
// ---------------------------------------------------------------------------

export type PreflightSeverity = "error" | "warning" | "info";

// AGENTS.md rule 4: must mirror `kcreate_export::preflight::PreflightCheck`
// 1:1. Every variant in the Rust enum's `as_str()` (preflight.rs:124-137)
// must appear here, otherwise the renderer's switch statements lose
// exhaustiveness.
export type PreflightCheckId =
  | "bleed_margin"
  | "font_embed"
  | "image_resolution"
  | "color_space"
  | "overprint_table"
  | "trapping"
  | "transparency"
  | "page_size"
  | "shading"
  | "font_glyph_coverage"
  | "total_ink_coverage"
  | "bleed_area_empty"
  | "spot_color_missing";

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

export type ModelPackCategory =
  | "core"
  | "image_pro"
  | "design_pro"
  | "vision"
  | "generation";

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

/// Serde-rendered name for `kcreate_ai::UpscaleBackend`. The string
/// is passed through to the bridge verbatim and parsed there; keep
/// in sync with the Rust enum if a new variant is added.
export type UpscaleBackendWire = "lanczos3" | "esrgan";

/// Serde-rendered name for `kcreate_ai::SegmentBackend`.
export type SegmentBackendWire = "edge_aware" | "sam";

/// Wire mirror of `kcreate_bridge::phase2::UpscaleWithBackendReport`.
export interface UpscaleWithBackendReportWire {
  /// The id of the newly inserted RasterLayer node.
  newNodeId: string;
  /// The backend that produced the result. Matches the request.
  backend: UpscaleBackendWire;
  outputWidth: number;
  outputHeight: number;
}

/// Wire mirror of `kcreate_bridge::phase2::SegmentReport`. The mask
/// is base64-encoded so the IPC channel stays a string-only wire.
export interface SegmentReportWire {
  backend: SegmentBackendWire;
  width: number;
  height: number;
  /// `width * height` bytes, `255` = foreground, `0` = background.
  maskBase64: string;
  area: number;
  confidence: number;
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
  /// Backend-selectable upscale. `backend = "lanczos3"` is built-in
  /// and always available; `backend = "esrgan"` requires the
  /// `onnx_upscale` Cargo feature on the kcreate_ai build and a
  /// valid model file path (typically resolved via
  /// `listModelPacks()` then the installed pack's `file_path`).
  upscaleWithBackend(
    nodeId: string,
    scale: number,
    backend: UpscaleBackendWire,
    modelPath: string,
  ): Promise<UpscaleWithBackendReportWire>;
  /// Point-prompt segmentation. The built-in `edge_aware` backend
  /// runs a real CIE-Lab + Sobel-edge-aware flood fill — no model
  /// required. `backend = "sam"` selects the SAM ONNX path, which
  /// requires the `onnx_segment` feature and a model file.
  segment(
    nodeId: string,
    pointX: number,
    pointY: number,
    tolerance: number,
    edgeThreshold: number,
    backend: SegmentBackendWire,
    modelPath: string,
  ): Promise<SegmentReportWire>;
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
  /// Run the text-region detector against a raster layer. Returns
  /// the detected regions in raster-pixel space (top-left origin),
  /// in reading order. Pass `null` for `options` to use the
  /// detector defaults from
  /// `kcreate_ai::DetectTextRegionsOptions::default()`.
  ///
  /// Read-only: does NOT modify the document. To insert a region
  /// as a TextLayer, call
  /// [`AiModelBridge.insertTextLayerForRegion`].
  detectTextRegions(
    nodeId: string,
    options?: DetectTextRegionsOptions | null,
  ): Promise<TextRegion[]>;
  /// Create a new `TextLayer` sibling of the source raster
  /// positioned over the detected region. The region's pixel-space
  /// coordinates are mapped into document space using the raster's
  /// `bounds` + intrinsic dimensions. Returns the new node id.
  ///
  /// The created TextLayer carries an `ai_insert_text_layer`
  /// operation in the undo log + an entry in the AI action log
  /// (`task_type: "ocr_insert_text_layer"`).
  insertTextLayerForRegion(
    request: InsertTextLayerForRegionRequest,
  ): Promise<string>;
}

/// A detected text-like region in a raster image, mirroring
/// `kcreate_ai::ocr::TextRegion`. Coordinates are in raster-pixel
/// space (top-left origin) matching the raster's intrinsic
/// dimensions, not document space.
///
/// `glyphCount` is the count of connected components merged into
/// the line; `estimatedCharCount` is `width / avg_glyph_advance`
/// (rounded up, floored at `glyphCount`). Both are heuristic hints
/// for the renderer — the detector does NOT perform character
/// recognition.
export interface TextRegion {
  x: number;
  y: number;
  width: number;
  height: number;
  glyphCount: number;
  estimatedCharCount: number;
}

/// Detector parameters, mirroring
/// `kcreate_ai::ocr::DetectTextRegionsOptions`. All fields optional
/// on the wire — omitted fields fall back to defaults on the Rust
/// side via `#[serde(default)]`.
export interface DetectTextRegionsOptions {
  /// Luminance threshold (0–255). Pixels at or below are "ink".
  luminanceThreshold?: number;
  /// Minimum component size in pixels.
  minComponentPixels?: number;
  /// Maximum component size as a fraction of the image area.
  maxComponentFraction?: number;
  /// Vertical overlap fraction required to merge two components
  /// into one line.
  lineOverlapRatio?: number;
  /// Horizontal gap allowance (× cap-height) within one line.
  lineGapRatio?: number;
  /// Hard cap on input image area, in pixels (`width * height`).
  /// Larger inputs are rejected on the Rust side with a typed
  /// error rather than fed into the O(W*H)-memory flood-fill.
  /// Omit to use the Rust default (16 million pixels, enough for
  /// any reasonable 4K screenshot).
  maxImagePixels?: number;
}

/// Renderer → bridge request to materialise a detected text region
/// as a new TextLayer. Mirrors
/// `kcreate_bridge::phase2::InsertTextLayerForRegionRequest`.
export interface InsertTextLayerForRegionRequest {
  /// Source raster the region was detected on. The new TextLayer
  /// is inserted as a sibling under the same parent.
  rasterNodeId: string;
  /// Region in raster-pixel space (top-left origin). Carries the
  /// glyph / char counts from the detector unchanged.
  region: TextRegion;
  /// Initial text content. Empty by default — the user typically
  /// types the recognised text after insertion since the detector
  /// reports bboxes, not characters.
  text?: string;
  /// Override the font family. Empty / omitted = bridge default.
  fontFamily?: string;
  /// Override the font size. Omitted = bridge estimates from the
  /// region's height in document space.
  fontSize?: number;
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

// AGENTS.md rule 4: must mirror `kcreate_bridge::phase2::FigmaImportReport`
// 1:1. Camel-case keys are produced by serde
// `#[serde(rename_all = "camelCase")]` on the Rust struct.
export interface FigmaImportReport {
  /// `document.name` from the Figma export, if present.
  documentName: string | null;
  /// New KCreate page ids in import order.
  pageIds: string[];
  /// Total child nodes (artboards, vectors, text, raster) created
  /// across all imported pages.
  nodesImported: number;
  /// Nodes the importer dropped (unsupported types, no geometry,
  /// shapeless image refs).
  nodesSkipped: number;
  /// Human-readable, non-fatal warnings.
  warnings: string[];
}

// AGENTS.md rule 4: must mirror `kcreate_bridge::phase2::SketchImportReport`
// 1:1.
export interface SketchImportReport {
  /// `metadata.name` from `document.json` when present.
  documentName: string | null;
  /// New KCreate page ids in import order.
  pageIds: string[];
  /// Total child nodes successfully created.
  nodesImported: number;
  /// Dropped nodes (unsupported classes, missing image refs).
  nodesSkipped: number;
  /// Human-readable, non-fatal warnings.
  warnings: string[];
}

/// Renderer-facing Figma JSON import API. Symmetric with
/// [`PdfImportBridge`] — pickFile keeps the OS picker in the main
/// process, importFigma runs the Rust importer + ingest pass.
export interface FigmaImportBridge {
  /// Show an Electron file picker scoped to `.json` /
  /// `.fig.json` and return the chosen absolute path.
  pickFile(): Promise<string | null>;
  /// Import the Figma JSON at `filePath` into the current project.
  importFigma(filePath: string): Promise<FigmaImportReport>;
}

/// Renderer-facing `.sketch` import API.
export interface SketchImportBridge {
  /// Show an Electron file picker scoped to `.sketch`.
  pickFile(): Promise<string | null>;
  /// Import the Sketch archive at `filePath` into the current project.
  importSketch(filePath: string): Promise<SketchImportReport>;
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
  /// Insert or replace a spot color in the project's
  /// `SpotColorLibrary` (Phase 5 Block D Task 23). Records an
  /// undoable `spot_color_upsert` operation.
  upsertSpot(spot: SpotColorWire): Promise<void>;
  /// Remove a spot color by name. Resolves to `false` when the
  /// name was not in the library.
  removeSpot(name: string): Promise<boolean>;
  /// List every spot color in the document.
  listSpots(): Promise<SpotColorWire[]>;
  /// Parse a Pantone-style JSON catalogue and merge every swatch into
  /// the project's `SpotColorLibrary`. `rawJson` is the full UTF-8
  /// catalogue contents (the renderer reads the file from disk via
  /// the native open-file dialog and passes the string here). The
  /// catalogue can be either:
  ///
  /// * `{ "name": "...", "entries": [{ "id": "PANTONE 185 C", "cmyk": [..4 floats..] }, ...] }`
  /// * a bare map `{ "PANTONE 185 C": { "cmyk": [..] }, "PANTONE 354 C": [..] }`
  ///
  /// CMYK channels outside `[0, 1]` are clamped; malformed entries
  /// (wrong-length arrays, non-finite numbers) are dropped without
  /// failing the rest of the load. Returns a structured report.
  /// Recorded as a single undoable `spot_color_load_catalog` op.
  loadCatalog(rawJson: string): Promise<SpotCatalogLoadReportWire>;
  /// Spec-shaped convenience wrapper for `upsertSpot` (Phase 5
  /// Block D Task 23). Equivalent to upsertSpot with
  /// `displayName = name`, no `libraryReference`.
  addSpot(
    name: string,
    c: number,
    m: number,
    y: number,
    k: number,
  ): Promise<void>;
  /// Toggle `NodeStyle::overprint` on any node. Records an
  /// undoable `node_set_overprint` operation.
  setNodeOverprint(nodeId: string, enabled: boolean): Promise<void>;
}

/// Wire shape for the spot color CRUD endpoints. Mirrors
/// `kcreate_bridge::phase2::SpotColorWire` 1:1.
export interface SpotColorWire {
  name: string;
  displayName: string;
  /// Tuple `(c, m, y, k)` in `[0, 1]`.
  fallbackCmyk: [number, number, number, number];
  libraryReference?: string;
}

/// Result of [`ColorBridge.loadCatalog`]. Mirrors
/// `kcreate_bridge::phase2::SpotCatalogLoadReport` 1:1.
///
/// The four catalogue-level counters satisfy
/// `rawEntries == parsed + duplicatesInCatalog + malformed`, so the
/// renderer can show users exactly why a load dropped or dedup'd
/// entries instead of presenting only the surviving `parsed` count
/// (Devin Review ANALYSIS_0005 on PR #16).
export interface SpotCatalogLoadReportWire {
  /// Total entries in the catalogue file before any validation /
  /// dedup. Mirrors `CatalogParseStats::raw_entries`.
  rawEntries: number;
  /// Entries that survived parsing and were merged into the project
  /// library.
  parsed: number;
  /// Entries dropped because they collided on `name`/`id` with an
  /// earlier well-formed entry in the same catalogue (last-write-
  /// wins). Always `0` for the bare-map shape (JSON object keys are
  /// unique at the parser level).
  duplicatesInCatalog: number;
  /// Entries dropped as malformed (wrong-length CMYK, non-finite
  /// values, missing `id` in the wrapped form, etc.).
  malformed: number;
  /// Swatches newly inserted into the project library.
  added: number;
  /// Swatches that overwrote an existing entry of the same `name`.
  overwritten: number;
}

// ---------------------------------------------------------------------------
// Phase 5 — smart-guides snap engine (Block C Task 13/14).
//
// Mirrors `kcreate_vector::snap::{SnapResult, SnapGuide, Axis}`.
// ---------------------------------------------------------------------------

export type SnapAxis = "Horizontal" | "Vertical";

export interface SnapGuide {
  axis: SnapAxis;
  position: number;
  from: number;
  to: number;
}

export interface SnapResult {
  dx: number;
  dy: number;
  guides: SnapGuide[];
}

export interface CanvasSnapBridge {
  /// Query the snap engine for an in-flight drag. Returns `null` when
  /// no project is open (e.g. the user opened the editor before any
  /// project loaded). Returns `{ dx: 0, dy: 0, guides: [] }` when no
  /// targets are within `threshold` — callers should treat that as
  /// "no snap" without special-casing it.
  query(
    movingId: string | null,
    candidateX: number,
    candidateY: number,
    candidateW: number,
    candidateH: number,
    threshold: number,
  ): Promise<SnapResult | null>;
}

// ---------------------------------------------------------------------------
// Phase 5 — raster filters (Block B Task 11).
//
// Mirrors the discriminated union accepted by
// `kcreate_bridge::raster_ops::PreviewFilter`. The renderer-side
// `FiltersPanel` builds these objects directly from slider state and
// passes them to `rasterOps.previewFilter`.
// ---------------------------------------------------------------------------

export type RasterBlurKind = "gaussian" | "box";
export type RasterFlipDirection = "horizontal" | "vertical";

/// Mirrors `kcreate_bridge::raster_ops::PreviewFilter` 1:1. The `type`
/// tag discriminates the variant; field names are snake_case to match
/// the Rust serde shape — DO NOT rename to camelCase or the bridge
/// will fail to deserialise.
export type RasterPreviewFilter =
  | {
      type: "levels";
      black_point: number;
      white_point: number;
      gamma: number;
    }
  | { type: "curves"; points: [number, number][] }
  | { type: "blur"; radius: number; kind: RasterBlurKind }
  | {
      type: "sharpen";
      radius: number;
      amount: number;
      threshold: number;
    }
  | {
      // Phase 8 Block B Task 9 — hue (deg), saturation (multiplier),
      // lightness (additive shift in `[-1, 1]`).
      type: "hsl";
      hue: number;
      saturation: number;
      lightness: number;
    }
  | {
      // Phase 8 Block B Task 10 — three-way shadows / midtones /
      // highlights balance, each `[r, g, b]` in `[-1, 1]`.
      type: "color_balance";
      shadows: [number, number, number];
      midtones: [number, number, number];
      highlights: [number, number, number];
    };

export interface RasterOpsBridge {
  applyLevels(
    nodeId: string,
    black: number,
    white: number,
    gamma: number,
  ): Promise<void>;
  applyCurves(nodeId: string, points: [number, number][]): Promise<void>;
  applyBlur(
    nodeId: string,
    radius: number,
    kind: RasterBlurKind,
  ): Promise<void>;
  applySharpen(
    nodeId: string,
    radius: number,
    amount: number,
    threshold: number,
  ): Promise<void>;
  crop(
    nodeId: string,
    x: number,
    y: number,
    w: number,
    h: number,
  ): Promise<void>;
  rotate(nodeId: string, angleDeg: number): Promise<void>;
  flip(nodeId: string, direction: RasterFlipDirection): Promise<void>;
  heal(
    nodeId: string,
    srcX: number,
    srcY: number,
    dstX: number,
    dstY: number,
    radius: number,
  ): Promise<void>;
  /// Non-destructive preview. Returns the post-filter RGBA buffer as
  /// a `Uint8Array` (already detached from the underlying `Buffer`).
  previewFilter(
    nodeId: string,
    filter: RasterPreviewFilter,
  ): Promise<Uint8Array>;
  // Phase 8 Block B — perspective transform, HSL adjustment, color
  // balance, and mask-aware filter application. Each commits an
  // undoable `Operation`. `perspective` accepts the destination
  // corners in **TL, TR, BL, BR** order in source-pixel space.
  // `applyFilterMasked` accepts a flat row-major `Uint8Array` whose
  // length must equal `width * height` of the layer; each byte is a
  // selection predicate (`0` = not selected, any non-zero = selected).
  // Byte-array transport keeps large masks cheap to send across the
  // IPC boundary versus a JS `boolean[]`. The bridge composes the
  // filter through a 1-pixel feather kernel at the mask boundary so
  // the seam does not alias.
  perspective(
    nodeId: string,
    corners: [
      [number, number],
      [number, number],
      [number, number],
      [number, number],
    ],
  ): Promise<void>;
  applyHsl(
    nodeId: string,
    hue: number,
    saturation: number,
    lightness: number,
  ): Promise<void>;
  applyColorBalance(
    nodeId: string,
    shadows: [number, number, number],
    midtones: [number, number, number],
    highlights: [number, number, number],
  ): Promise<void>;
  applyFilterMasked(
    nodeId: string,
    filter: RasterPreviewFilter,
    mask: Uint8Array,
  ): Promise<void>;
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
  /// Phase 5 Block D Task 19. When set, overflow text from this
  /// frame spills into the linked TextLayer at `next_frame_id`.
  /// `null` (or absent on legacy projects) terminates the chain.
  next_frame_id?: string | null;
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

/// Wire-format mirror of `kcreate_bridge::phase2::TextStyleWire`,
/// which itself mirrors `kcreate_text::paragraph::TextStyle`. Phase
/// A1 wire surface — used by `window.kcreate.text.{getStyle,setStyle}`.
///
/// `lineHeight` is the multiplier the shaper applies to `fontSize`
/// when laying out lines; `1.25` is the Rust-side default.
///
/// Adding a field here requires the matching change in
/// `crates/kcreate_bridge/src/phase2.rs::TextStyleWire` and
/// `crates/kcreate_text/src/paragraph.rs::TextStyle` (rule 4 of
/// `AGENTS.md`).
export interface TextStyleWire {
  fontFamily: string;
  fontSize: number;
  lineHeight: number;
}

/// Phase A1 — inline text editor + font controls. Mutators record
/// an undoable operation in the project's log; reads are pure.
///
/// `replaceRange` indices are **UTF-16 code-unit offsets** (matching
/// JavaScript's `String.length` / `Selection.anchorOffset`) — the
/// bridge converts to/from UTF-8 internally and rejects ranges that
/// would split a surrogate pair.
export interface TextBridge {
  /// Replace the text content of a `TextLayer` node. Preserves the
  /// current style; use `setStyle` to mutate font family / size /
  /// line height.
  setContent(nodeId: string, content: string): Promise<void>;
  /// Replace the text style for a `TextLayer` node.
  setStyle(nodeId: string, style: TextStyleWire): Promise<void>;
  /// Splice the UTF-16 range `[start..end]` of the node's text with
  /// `replacement`. Used by the inline canvas editor's blur
  /// commit path: `replaceRange(0, content.length, newContent)`.
  replaceRange(
    nodeId: string,
    start: number,
    end: number,
    replacement: string,
  ): Promise<void>;
  /// Read the current text content for a `TextLayer` node. Used to
  /// hydrate the `TextStylePanel` content textarea + the inline
  /// canvas editor's initial buffer.
  getContent(nodeId: string): Promise<string>;
  /// Read the current text style for a `TextLayer` node. Used to
  /// hydrate the `TextStylePanel` controls on selection change.
  getStyle(nodeId: string): Promise<TextStyleWire>;
  /// Sorted, deduplicated list of font family names known to the
  /// process-wide font database. First call lazily loads system
  /// fonts; subsequent calls are cached.
  listFonts(): Promise<string[]>;
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
  /// Phase 5 Block D Task 19. Link `aId`'s overflow to spill into
  /// `bId`. Both must be TextLayer nodes. The bridge rejects
  /// self-links and cycle creation; the call resolves once the
  /// undoable `text_frame_link` operation lands.
  link(aId: string, bId: string): Promise<void>;
  /// Break the link out of `nodeId`. No-op if `nodeId` is not
  /// currently linked.
  unlink(nodeId: string): Promise<void>;
  /// Replace the text frame's wrap mode (Phase 5 Block D Task 20).
  setWrap(nodeId: string, mode: TextWrapMode): Promise<void>;
}

// ---------------------------------------------------------------------------
// Phase 5 — vector path operations (Block C Tasks 15, 16, 18).
//
// Every call mutates the document and records an undoable Operation.
// The simplify / smooth / offset variants rewrite the stored
// `VectorPath`; `setStrokeProfile`, `applyPathEffect`, and
// `clearPathEffects` mutate the node's `NodeStyle` so the renderer
// applies the effect chain at draw time without losing the original
// geometry.
// ---------------------------------------------------------------------------

/// JSON-friendly path-effect discriminator. Mirrors
/// `kcreate_core::node::PathEffect` (`#[serde(tag = "kind",
/// rename_all = "snake_case")]`).
export type PathEffectWire =
  | { kind: "dash"; pattern: number[]; offset?: number }
  | { kind: "round_corners"; radius: number };

/// `[t, width]` control-point pairs. `t ∈ [0, 1]` along the path
/// parameter; `width` is in world units.
export type StrokeWidthProfile = Array<[number, number]>;

export interface VectorOpsBridge {
  /// Ramer-Douglas-Peucker simplification.
  simplify(nodeId: string, tolerance: number): Promise<void>;
  /// Chaikin corner-cutting smoothing.
  smooth(nodeId: string, iterations: number): Promise<void>;
  /// Parallel offset; positive = outward for closed paths.
  offset(nodeId: string, distance: number): Promise<void>;
  /// Install a variable stroke-width profile (`null` clears it).
  setStrokeProfile(
    nodeId: string,
    profile: StrokeWidthProfile | null,
  ): Promise<void>;
  /// Push a non-destructive path effect onto the node's chain.
  applyPathEffect(nodeId: string, effect: PathEffectWire): Promise<void>;
  /// Remove every path effect from the node.
  clearPathEffects(nodeId: string): Promise<void>;
}

// ---------------------------------------------------------------------------
// Phase 5 — slices (Block D Task 22). Mirrors
// `kcreate_core::project::Slice` + `kcreate_export::slice::SliceResult`.
// ---------------------------------------------------------------------------

export interface SliceWire {
  id: string;
  name: string;
  bounds: { x: number; y: number; width: number; height: number };
  format: ExportFormat;
  scale: number;
  suffix: string;
}

export interface SliceResultWire {
  sliceId: string;
  name: string;
  path?: string | null;
  bytesWritten: number;
  error?: string | null;
}

export interface SliceUpdateProps {
  name?: string;
  bounds?: { x: number; y: number; width: number; height: number };
  format?: ExportFormat;
  scale?: number;
}

export interface SliceBridge {
  create(
    name: string,
    x: number,
    y: number,
    w: number,
    h: number,
    format: ExportFormat,
    scale: number,
  ): Promise<string>;
  update(sliceId: string, changes: SliceUpdateProps): Promise<void>;
  delete(sliceId: string): Promise<boolean>;
  list(): Promise<SliceWire[]>;
  exportAll(outputDir: string): Promise<SliceResultWire[]>;
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
    }
  | {
      /// Phase 7 (Task 8): a peer was evicted from the session
      /// because their KChat community membership was revoked.
      kind: "peerKicked";
      peerId: string;
      reason: string;
    }
  | {
      /// Phase 7 (Task 11): a peer's collaboration permission
      /// changed (e.g. the host downgraded them to viewer).
      kind: "permissionChanged";
      peerId: string;
      permission: CollabPermission;
    }
  | {
      /// Phase 7 (Task 15): the local late-join replay finished —
      /// a remote host sent us a `ResumeBundle` and the journal
      /// has been brought up to date. Consumers (EditorPage,
      /// PresencePanel) use this to dismiss the "syncing…"
      /// indicator and surface the post-resume document state.
      kind: "resumeApplied";
      /// Peer id of the host that supplied the bundle.
      fromPeerId: string;
      /// Number of operations from the bundle that were appended
      /// to the local journal. May be smaller than the bundle's
      /// operation count because the journal silently dedupes
      /// entries we have already seen.
      appliedCount: number;
    }
  | {
      /// Phase 7 (Task 16): the CRDT conflict resolver tiebroke a
      /// concurrent edit. Renderer surfaces this as a brief
      /// `ConflictToast` when `loserPeerId` matches the local
      /// session peer id — "Your edit to X was overridden by Y".
      kind: "conflictResolved";
      /// Node whose value the resolver had to tiebreak (UUID).
      nodeId: string;
      /// Peer whose write the resolver kept.
      winnerPeerId: string;
      /// Peer whose write the resolver discarded. The renderer
      /// only toasts when this equals the local peer id.
      loserPeerId: string;
      /// Free-form field path of the resolved value (e.g.
      /// `"transform.x"`, `"fill.color"`).
      field: string;
    }
  | {
      /// Phase 7 (Task 17): a remote peer broadcast an undo (or
      /// redo) inverse-operation batch. The operations were
      /// already journaled via `operationsJournaled`; this event
      /// only carries the "show 'Ken undid …' in the activity
      /// feed" intent so the renderer can distinguish a fresh
      /// edit from an undo.
      kind: "undoBroadcast";
      /// Peer id that produced the undo.
      peerId: string;
      /// Number of inverse operations in the batch (>= 1). Used
      /// by the activity feed so the toast pluralizes correctly.
      opCount: number;
    }
  | {
      /// Phase 7 (Task 19): a fresh session key has been scheduled.
      /// The renderer can surface a "rotating session keys…" toast
      /// while peers acknowledge.
      kind: "keyRotationScheduled";
      /// Incrementing epoch number; matches the value passed into
      /// later `keyRotationCompleted` events so the renderer can
      /// correlate the two.
      epoch: number;
      /// New cert fingerprint (base64url, BLAKE3 of the SPKI bytes).
      newCertFingerprint: string;
      /// Wall-clock millisecond timestamp by which peers must ack
      /// the rotation or be disconnected.
      deadlineUnixMs: number;
    }
  | {
      /// Phase 7 (Task 19): the grace window for a key rotation
      /// elapsed. Acknowledged peers stay connected; missing peers
      /// have already been kicked with reason `key-rotation-timeout`.
      kind: "keyRotationCompleted";
      epoch: number;
      ackedPeerIds: string[];
      droppedPeerIds: string[];
    }
  | {
      /// Phase 7 (Task 22): a peer exceeded its per-second
      /// operations/presence budget. The renderer surfaces this
      /// as a non-blocking toast — repeated overflows escalate
      /// to a `peerKicked` event with reason `rate-limit-exceeded`.
      kind: "rateLimitWarning";
      peerId: string;
      /// Which counter was breached: `operations` or `presence`.
      metric: string;
      /// Number of consecutive rolling 1-second windows the peer
      /// has been continuously over budget. Named `_windows`
      /// (not `_seconds`) because the counter measures **windows**,
      /// not wall-clock seconds — keeps the unit explicit if the
      /// window width is ever retuned. Drives the warn → kick
      /// escalation threshold
      /// (`SessionConfig::rate_limit_disconnect_after`).
      consecutiveOverflowWindows: number;
    }
  | {
      /// Phase 7 (Task 21): a peer was rejected (or kicked) because
      /// the project ACL doesn't authorise them and the active
      /// session isn't relying on community gating to admit them.
      /// The renderer uses this to log the denial in the audit feed
      /// and to refresh the connected-peers list.
      kind: "aclRejected";
      peerId: string;
      reason: string;
    }
  | {
      /// Phase 7 (Task 23): a remote peer offered to share an
      /// encrypted clipboard payload with us. The renderer shows
      /// an accept/reject prompt; choosing accept resolves to the
      /// plaintext via `session.acceptClipboardOffer(offerId)`,
      /// reject simply discards the offer via
      /// `session.rejectClipboardOffer(offerId)`.
      kind: "clipboardShareOffered";
      fromPeerId: string;
      /// Short, human-readable preview the sender attached. Never
      /// rendered as HTML — display it as plain text.
      previewLabel: string;
      /// Opaque identifier used by `acceptClipboardOffer` /
      /// `rejectClipboardOffer`.
      offerId: string;
    }
  | {
      /// Phase 8 (Task 4): a remote peer broadcast an annotation
      /// mutation (create / edit / resolve / delete) and the bridge
      /// has already applied it to the local project DB. The
      /// renderer's `AnnotationOverlay` listens for this so its
      /// per-page list re-renders without polling the DB on every
      /// frame.
      ///
      /// `verb` is named `verb` (not `kind`) because the parent
      /// `SessionEvent` is serde-tagged with `tag = "kind"`. Mirrors
      /// `kcreate_bridge::collab::SessionEvent::AnnotationsApplied`.
      kind: "annotationsApplied";
      /// Peer id that emitted the broadcast.
      peerId: string;
      /// `"upsert"` or `"delete"` — snake_case mirror of
      /// `kcreate_collab::AnnotationBroadcastKind`.
      verb: string;
      /// Number of annotations affected. Drives toast text
      /// pluralization in the UI.
      count: number;
      /// Page ids touched by the broadcast. Used by the renderer to
      /// know which `AnnotationOverlay` instances need to re-fetch.
      pageIds: string[];
    };

/// Phase 7 (Task 21): permission level for a single entry in the
/// project ACL. Mirrors `kcreate_collab::AclPermission`.
export type AclPermission = "editor" | "viewer";

/// Phase 7 (Task 21): ACL enforcement mode. Mirrors
/// `kcreate_collab::AclMode`.
///
/// - `open`: ACL is advisory; peers not listed are still admitted
///   (used when community gating is the primary authorisation).
/// - `enforce`: ACL is the gate; peers must be either in the ACL
///   or in the active community (when community gating is on).
export type AclMode = "open" | "enforce";

/// Phase 7 (Task 21): one row in the project ACL. Mirrors
/// `kcreate_collab::AclEntry`.
export interface AclEntry {
  /// Base64url Ed25519 public key.
  publicKey: string;
  /// Free-form name shown in the ACL panel.
  displayName: string;
  permission: AclPermission;
}

/// Phase 7 (Task 21): persisted project ACL. Stored as
/// `<project_dir>/acl.json`. Mirrors `kcreate_collab::ProjectAcl`.
export interface ProjectAcl {
  mode: AclMode;
  entries: AclEntry[];
}

/// Phase 7 (Task 23): inbound pending clipboard offer surfaced by
/// `session.pendingClipboardOffers()`. The renderer renders one row
/// per entry and calls `acceptClipboardOffer` / `rejectClipboardOffer`
/// when the user picks an action.
export interface PendingClipboardOffer {
  offerId: string;
  fromPeerId: string;
  previewLabel: string;
}

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
    /// Phase 7 (Task 7): optional KChat community id used to scope
    /// mDNS auto-discovery. When set, two KCreate peers on the
    /// same LAN only auto-connect when they're members of the
    /// same KChat community. Must match the currently-installed
    /// `KChatMembership.groupId`; mismatches reject with an
    /// `invalid argument "communityId"` error from the bridge.
    /// `null` (or omitted) preserves pre-Phase-7 behaviour.
    communityId?: string | null,
    /// Phase 7 (Task 21): absolute path to the open project's
    /// `.kstudio/` directory. When supplied the bridge loads
    /// `<dir>/acl.json` at session start and persists ACL changes
    /// back to that file via `session.acl.set` so peer-allowlist
    /// edits survive process restart. Pass `ProjectInfo.path`.
    /// `null` (or omitted) keeps the ACL in-memory only — useful
    /// for ad-hoc sessions or test harnesses that don't have a
    /// project on disk.
    projectDir?: string | null,
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
  /// Phase 7 (Task 8): forcibly disconnect a connected peer.
  kickPeer(peerId: string, reason: string): Promise<void>;
  /// Phase 7 (Task 11): set a peer's collaboration permission.
  setPeerPermission(
    peerId: string,
    permission: CollabPermission,
  ): Promise<void>;
  /// Phase 7 (Task 11): snapshot of the local peer's permission.
  localPermission(): Promise<CollabPermission>;
  /// Phase 7 (Task 15): ask the supplied peer for a `ResumeBundle`
  /// covering everything we're missing relative to our local
  /// resume vector. Used by late joiners to backfill journal
  /// history that predates their `join()`. The result arrives
  /// asynchronously as a `ResumeApplied` session event; this call
  /// only fires the request. KChat-gated.
  requestResume(peerId: string): Promise<void>;
  /// Phase 7 (Task 21): snapshot of the active session's ACL. `null`
  /// when no session is running.
  acl(): Promise<ProjectAcl | null>;
  /// Phase 7 (Task 21): replace the active session's ACL. The new
  /// policy is persisted to `<project_dir>/acl.json` and applied
  /// immediately — connected peers that no longer meet the policy
  /// are kicked with reason `acl-rejected`.
  setAcl(acl: ProjectAcl): Promise<void>;
  /// Phase 7 (Task 19): force an immediate session-key rotation.
  /// Returns the new epoch number. `graceMs` is the wall-clock
  /// window peers have to ack the rotation; non-acking peers are
  /// disconnected with `key-rotation-timeout`. The rotation result
  /// arrives asynchronously as a `keyRotationCompleted` event.
  rotateKeys(graceMs: number): Promise<number>;
  /// Phase 7 (Task 19): current rotation epoch (0 at session start;
  /// bumped on every successful rotation). `null` when no session
  /// is running.
  keyEpoch(): Promise<number | null>;
  /// Phase 7 (Task 23): encrypt `plaintext` for `peerId` and send
  /// it as an inbound `ClipboardShare` offer. Returns the generated
  /// offer id; the recipient eventually responds by accepting or
  /// rejecting through their own bridge.
  shareClipboard(
    peerId: string,
    plaintext: Uint8Array,
    previewLabel: string,
  ): Promise<string>;
  /// Phase 7 (Task 23): decrypt and dequeue an inbound clipboard
  /// offer matching `offerId`. Returns the plaintext bytes.
  acceptClipboardOffer(offerId: string): Promise<Uint8Array>;
  /// Phase 7 (Task 23): discard an inbound clipboard offer without
  /// decrypting it. Idempotent — unknown ids are a no-op.
  rejectClipboardOffer(offerId: string): Promise<void>;
  /// Phase 7 (Task 23): snapshot of inbound clipboard offers that
  /// haven't yet been accepted or rejected.
  pendingClipboardOffers(): Promise<PendingClipboardOffer[]>;
  /// Phase 7 (Task 25): queue one local-authored operation into the
  /// outbound throttle buffer. The bridge accumulates ops until
  /// the configured flush interval elapses or the max-ops cap is
  /// hit, then broadcasts the whole batch in a single envelope.
  /// `operation` is a JSON-serializable `Operation` value — the
  /// preload stringifies it and the Rust bridge parses it. Typed
  /// `unknown` rather than the Rust `Operation` shape because the
  /// renderer does not currently model operations; future work
  /// will tighten the type once a renderer-side mutation builder
  /// exists.
  queueOperation(operation: unknown): Promise<void>;
  /// Phase 7 (Task 25): drain the pending op batch and broadcast
  /// it immediately. Returns the number of ops that were flushed
  /// (0 if the queue was empty). Call this at the end of a drag
  /// interaction so the final state lands on the wire without
  /// waiting for the throttle deadline.
  flushPendingOperations(): Promise<number>;
  /// Phase 7 (Task 25): check the pending batch against the
  /// configured flush interval and broadcast it if the deadline
  /// has expired. Call once per event tick. Returns the number of
  /// ops flushed on this tick (0 when no flush was due). Cheap
  /// when the queue is empty.
  tickOutboundBatch(): Promise<number>;
  /// Phase 7 (Task 27): set the list of pages the local peer is
  /// currently viewing. Remote presence updates and conflict
  /// toasts for pages outside this set are suppressed from the
  /// renderer event stream to reduce overlay churn in multi-page
  /// documents. Operations still journal across the whole project
  /// so document consistency is preserved. Pass `[]` to revert to
  /// "interested in everything" (useful for the export preview
  /// pane or a presentation mode that needs every event).
  setActivePages(pageIds: string[]): Promise<void>;
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
  /// 32-byte Ed25519 verifying key of the issuer that minted the
  /// active membership (URL-safe base64, no padding). `null` when
  /// `locked` is true. Renderer surfaces this on a "Issued by …"
  /// line below the group / expiry summary.
  issuerPublicKey?: string | null;
  /// Human-readable label of the matching trusted-issuer entry
  /// (if any). `null` when locked, when the issuer is not on the
  /// allowlist (`issuerTrusted` is `false`), or when the allowlist
  /// is empty (no labels to attach). The renderer falls back to a
  /// truncated `issuerPublicKey` for display in those cases.
  issuerLabel?: string | null;
  /// `true` iff the issuer is listed in the configured allowlist
  /// OR the allowlist is empty (backward-compat: empty list means
  /// "accept any issuer"). The renderer renders a distinct
  /// "Untrusted issuer — test only" badge when this is `false`.
  issuerTrusted?: boolean;
}

/// One entry in the KChat trusted-issuer allowlist. Mirrors
/// `kcreate_bridge::collab::TrustedIssuer`. The allowlist is
/// loaded from disk at app start (via
/// `KChatBridge.setTrustStorePath`) and mutated via
/// `addTrustedIssuer` / `removeTrustedIssuer`. An empty list
/// preserves the pre-Block-E behaviour of "accept any issuer" so
/// the dev-mint flow keeps working out of the box.
export interface TrustedIssuer {
  /// 32-byte Ed25519 verifying key of the issuer, URL-safe base64
  /// (no padding). Padding is stripped on the way in so a user
  /// pasting a padded key from a KChat admin dashboard still
  /// matches install requests, which always arrive unpadded.
  issuerPublicKey: string;
  /// Human-readable label shown in the sign-in panel and on the
  /// "Issued by" line of the membership-status summary. Max 128
  /// characters; non-empty.
  label: string;
  /// ISO-8601 timestamp at which the entry was added or last
  /// updated. Reset to "now" by `addTrustedIssuer` even when the
  /// caller supplies an older timestamp — this prevents a buggy
  /// renderer from back-dating the addition.
  addedAt: string;
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
  /// Point the trust-store at a JSON file on disk. The Electron
  /// main process calls this once at app start with
  /// `<userData>/kchat_trust.json`. The file is created lazily on
  /// first add; missing-file is treated as "empty allowlist". The
  /// current allowlist (after reading the file, if any) is
  /// returned. Idempotent — safe to call multiple times.
  setTrustStorePath(path: string): Promise<TrustedIssuer[]>;
  /// Snapshot the current trusted-issuer allowlist.
  trustedIssuers(): Promise<TrustedIssuer[]>;
  /// Add (or update) a trusted issuer. If an entry with the same
  /// `issuerPublicKey` already exists, its label and timestamp
  /// are replaced — same call is the "edit label" path. The
  /// returned list is the post-add snapshot. Persisted to the
  /// configured trust-store file (if any) via an atomic
  /// temp-file-then-rename.
  addTrustedIssuer(issuer: TrustedIssuer): Promise<TrustedIssuer[]>;
  /// Remove the entry with the given `issuerPublicKey`. No-op if
  /// no matching entry exists (returns the unchanged list). When
  /// the last entry is removed, the bridge collapses back to
  /// "accept any issuer" (`issuerTrusted` becomes `true` for any
  /// active membership).
  removeTrustedIssuer(issuerPublicKey: string): Promise<TrustedIssuer[]>;
}

// ---------------------------------------------------------------------------
// Phase 7 — KChat backend (HTTPS REST).
//
// Mirrors `kcreate_kchat_client` wire-format types and the bridge
// surface in `kcreate_bridge::kchat_backend`. The REST client
// signs in to the same KChat / Mattermost backend
// `uney-chat-desktop` uses. A separate `.kcz` companion
// extension ships inside KChat Desktop (`apps/kchat-extension/`)
// and surfaces recent KCreate projects + share invites via the
// host's procedures registry — it does NOT proxy this bridge;
// both apps independently talk to the same backend.
// ---------------------------------------------------------------------------

/// Local-user identity reported by `kchat.identity.get`. Mirrors
/// `kcreate_kchat_client::KChatIdentity`.
export interface KChatIdentity {
  /// Human-readable display name.
  displayName: string;
  /// XMPP bare JID (e.g. `alice@kchat.com`) used by
  /// `uney-chat-desktop` as the primary user identifier.
  jid: string;
  /// Ed25519 public key, URL-safe base64 (no padding). KCreate
  /// uses this to bind multiplayer attestations to the local
  /// peer.
  publicKey: string;
  /// BLAKE3 hash of the Ed25519 public key, URL-safe base64
  /// (no padding) — same `PeerId` shape KCreate uses across the
  /// collab stack.
  peerId: string;
}

/// Status snapshot returned by `KChatBackendBridge.{connect,
/// disconnect, status}`. Mirrors
/// `kcreate_bridge::kchat_backend::KChatBackendStatus`.
export interface KChatBackendStatus {
  connected: boolean;
  /// HTTPS base URL the client is signed in to. `null` when not
  /// signed in.
  baseUrl: string | null;
  /// Identity returned by the login response. `null` until the
  /// renderer signs in.
  identity: KChatIdentity | null;
}

/// Sign-in request body the renderer hands to
/// `KChatBackendBridge.connect`. Mirrors
/// `kcreate_bridge::kchat_backend::KChatBackendSignInRequest`.
export interface KChatBackendSignInRequest {
  /// HTTPS base URL of the KChat / Mattermost backend. The Rust
  /// client refuses anything but `https://` in production builds.
  baseUrl: string;
  /// Login id (XMPP bare JID for KChat; username or email for
  /// Mattermost — the backend disambiguates).
  loginId: string;
  /// Password / OAuth bearer token (treated opaquely).
  password: string;
  /// Optional TOTP code when the user has 2FA enabled.
  totp?: string;
}

/// A community reported by `kchat.communities.list`. Mirrors
/// `kcreate_kchat_client::KChatCommunity`.
export interface KChatCommunity {
  id: string;
  name: string;
  description: string | null;
  memberCount: number;
  /// Local user's role in the community. Drives the collab
  /// permission model in Block B (owner/admin → Editor with
  /// kick rights; member → Editor or Viewer depending on host
  /// downgrades).
  role: "owner" | "admin" | "member";
}

/// A community member reported by `kchat.communities.getMembers`.
/// Mirrors `kcreate_kchat_client::KChatCommunityMember`.
export interface KChatCommunityMember {
  jid: string;
  displayName: string;
  publicKey: string;
  peerId: string;
  role: "owner" | "admin" | "member";
}

/// A conversation/channel within a community. Mirrors
/// `kcreate_kchat_client::KChatConversation`.
export interface KChatConversation {
  id: string;
  name: string;
  communityId: string;
  conversationType: "channel" | "direct";
}

/// Invite-card payload posted to a KChat conversation by
/// `KChatBackendBridge.shareToConversation`. Mirrors
/// `kcreate_bridge::kchat_backend::KChatShareInvite`.
export interface KChatShareInvite {
  /// KCreate project id the invite points to (UUID).
  projectId: string;
  /// Human-readable project name.
  projectName: string;
  /// Owning peer id (BLAKE3 hash of owner Ed25519 key).
  ownerPeerId: string;
  /// Owner Ed25519 public key, URL-safe base64 (no padding).
  ownerPublicKey: string;
  /// Owner display name as shown by KCreate.
  ownerDisplayName: string;
  /// SHA-256 of the owner QUIC TLS leaf cert, URL-safe base64.
  certFingerprint: string;
  /// Owner QUIC socket address (`<ip>:<port>`).
  ownerSocketAddr: string;
  /// Community the invite is gated on. Joiner must be a member.
  communityId: string;
  /// Conversation the invite is posted to.
  conversationId: string;
}

/// Result of `KChatBackendBridge.shareToConversation`. Mirrors
/// `kcreate_kchat_client::PostMessageResponse`.
export interface KChatPostMessageResult {
  messageId: string;
  /// RFC3339 UTC timestamp the server stamped on the message.
  postedAt: string;
}

/// Phase 7 (Task 10): result of accepting a share-document invite.
/// Mirrors `kcreate_bridge::kchat_backend::KChatAcceptedInvite`.
export interface KChatAcceptedInvite {
  projectId: string;
  projectName: string;
  ownerPeerId: string;
  ownerDisplayName: string;
  communityId: string;
  conversationId: string;
}

/// Phase 7 (Task 8): result of a roster-sync tick.
/// Mirrors `kcreate_bridge::kchat_backend::KChatRosterSyncResult`.
export interface KChatRosterSyncResult {
  /// How many members the KChat backend reported on this tick.
  polledMembers: number;
  /// Peer ids that were evicted because they were no longer in the
  /// community roster.
  kicked: string[];
}

/// Phase 7 (Task 11): collaboration permission for a session peer.
/// Mirrors `kcreate_bridge::collab::CollabPermission`.
export type CollabPermission = "editor" | "viewer";

// -----------------------------------------------------------------------------
// Phase 8 — KChat artifact publishing (Block A, Tasks 1–3)
// -----------------------------------------------------------------------------

/// Wire format of an artifact published to a KChat conversation.
/// Mirrors `kcreate_kchat_client::ArtifactKind` — keep the lowercase
/// strings in lockstep with the Rust `#[serde(rename_all = "lowercase")]`
/// emission.
export type KChatArtifactKind =
  | "png"
  | "svg"
  | "pdf"
  | "webp"
  | "jpeg"
  | "brandKit";

/// Structured metadata stamped onto a published artifact. Mirrors
/// `kcreate_kchat_client::ArtifactMetadata`. The renderer paints
/// the rich-card preview directly from this without re-fetching
/// the artifact bytes.
export interface KChatArtifactMetadata {
  /// Originating project name (verbatim).
  projectName: string;
  /// Originating page / artboard name, or brand-kit name for
  /// `brandKit` uploads. `undefined` for whole-project uploads.
  artboardName?: string;
  /// Free-form preset label echoed onto the rich-card chip
  /// (e.g. `"PNG @1x"`, `"PDF A4 300dpi"`).
  exportPreset?: string;
  /// Rasterised pixel dimensions. `undefined` for vector-only
  /// formats (SVG, PDF) and `.kbrand`.
  widthPx?: number;
  heightPx?: number;
  /// Source project id (UUID string).
  projectId: string;
  /// Wire format of the artifact bytes.
  kind: KChatArtifactKind;
}

/// Response from `KChatBackendBridge.publishArtifact` /
/// `publishBrandKit`. Mirrors
/// `kcreate_kchat_client::ArtifactPublishResult`.
export interface KChatArtifactPublishResult {
  artifactId: string;
  conversationId: string;
  /// Backend-issued URL the renderer can hit to preview / download
  /// the artifact (typically a signed short-lived link).
  previewUrl: string;
  /// Backend-issued URL of the rendered thumbnail. May equal
  /// `previewUrl` when no separate thumbnail was generated.
  thumbnailUrl: string;
  kind: KChatArtifactKind;
  /// RFC3339 UTC timestamp the backend stamped on the artifact.
  publishedAt: string;
}

/// One artifact returned by `KChatBackendBridge.listArtifacts`.
/// Mirrors `kcreate_kchat_client::PublishedArtifact`.
export interface KChatPublishedArtifact {
  artifactId: string;
  conversationId: string;
  previewUrl: string;
  thumbnailUrl: string;
  kind: KChatArtifactKind;
  metadata: KChatArtifactMetadata;
  /// Size of the artifact bytes the backend has on file.
  byteSize: number;
  /// RFC3339 UTC timestamp the backend stamped on the artifact.
  publishedAt: string;
}

/// SVG-specific artifact-publish payload. Mirrors
/// `kcreate_bridge::kchat_artifact::KChatSvgArtifactRequest`.
/// `nodeIds` is optional / empty = "the whole document",
/// matching the bridge's `export.svg` semantics.
export interface KChatSvgArtifactRequest {
  options: SvgExportOptions;
  nodeIds?: string[];
}

/// Discriminated union over the artifact format the renderer
/// wants to publish. Mirrors
/// `kcreate_bridge::kchat_artifact::KChatArtifactRequestKind`
/// (Rust `#[serde(tag = "format")]`).
export type KChatArtifactRequestKind =
  | ({ format: "png" } & PngExportOptions)
  | { format: "svg"; options: SvgExportOptions; nodeIds?: string[] }
  | ({ format: "pdf" } & PdfExportOptions)
  | ({ format: "webp" } & WebpExportOptions)
  | ({ format: "jpeg" } & JpegExportOptions);

/// Bridge-facing request for
/// `KChatBackendBridge.publishArtifact`. Mirrors
/// `kcreate_bridge::kchat_artifact::KChatArtifactRequest`.
export interface KChatArtifactPublishRequest {
  /// Wire format of the artifact to produce.
  kind: KChatArtifactRequestKind;
  /// Free-form preset label echoed on the rich-card chip.
  exportPreset?: string;
  /// Page / artboard name surfaced on the rich-card preview.
  artboardName?: string;
}

/// Bridge-facing request for
/// `KChatBackendBridge.publishBrandKit`. Mirrors
/// `kcreate_bridge::kchat_artifact::KChatBrandKitArtifactRequest`.
export interface KChatBrandKitArtifactRequest {
  /// Brand-kit to serialise. Must exist in the open project.
  brandKitId: string;
  /// Free-form preset label (e.g. `"Brand Kit v3"`). Optional.
  exportPreset?: string;
}

/// Phase 7 KChat **backend** bridge surface. Every method other
/// than `available` is optional because non-`kchat-backend` builds
/// do not link the underlying N-API exports; the renderer probes
/// via `available()` and falls back to the paste-attestation flow
/// when the answer is `false`.
export interface KChatBackendBridge {
  /// Capability probe — `true` when the bridge was compiled with
  /// the `kchat-backend` feature flag.
  available(): Promise<boolean>;
  /// Sign in to a KChat / Mattermost backend. Tears down any prior
  /// session + installed authority before installing the new
  /// client so a stale membership doesn't outlive the sign-in it
  /// was minted from.
  connect(request: KChatBackendSignInRequest): Promise<KChatBackendStatus>;
  /// Sign out (idempotent). Also clears any KChat authority
  /// installed by `selectCommunity` so a stale membership doesn't
  /// outlive the sign-in it was minted from.
  disconnect(): Promise<KChatBackendStatus>;
  /// Snapshot the current sign-in state + cached identity.
  status(): Promise<KChatBackendStatus>;
  /// List the communities the local user belongs to.
  listCommunities(): Promise<KChatCommunity[]>;
  /// Pick a community and install its attestation as the active
  /// KChat authority. Returns the same `KChatMembershipStatus`
  /// shape as `KChatBridge.install`.
  selectCommunity(communityId: string): Promise<KChatMembershipStatus>;
  /// Return the member roster (with roles) for the given
  /// community. Used by Block B's roster-sync tick + the
  /// community-role-based permissions model.
  getCommunityMembers(communityId: string): Promise<KChatCommunityMember[]>;
  /// Return the conversations/channels in the given community.
  listConversations(communityId: string): Promise<KChatConversation[]>;
  /// Post a document-share invite into a KChat conversation. The
  /// payload is tagged `kcreate.invite.v1` so KChat Desktop
  /// renders it as a rich card.
  shareToConversation(
    conversationId: string,
    invite: KChatShareInvite,
  ): Promise<KChatPostMessageResult>;
  /// Phase 7 (Task 10): accept a share-document invite received
  /// through a KChat conversation. Validates community match +
  /// sender membership and dials the owner via `session.join()`.
  acceptInvite(inviteJson: string): Promise<KChatAcceptedInvite>;
  /// Phase 7 (Task 8): roster-sync tick — polls the KChat backend
  /// for the latest community members, reconciles against the
  /// active session (kicks revoked peers, refreshes role ->
  /// permission).
  syncCommunityRoster(
    communityId: string,
  ): Promise<KChatRosterSyncResult>;
  /// Phase 8 (Block A, Task 2): render the current scene to the
  /// requested artifact format **in memory** + generate a
  /// thumbnail and publish the package to a KChat conversation
  /// as a rich preview card. No temp file is touched.
  publishArtifact(
    conversationId: string,
    request: KChatArtifactPublishRequest,
  ): Promise<KChatArtifactPublishResult>;
  /// Phase 8 (Block A, Task 2): serialise the named brand-kit
  /// (and its embedded font / logo asset blobs) into an in-memory
  /// `.kbrand` archive and publish it as a `brandKit` artifact.
  publishBrandKit(
    conversationId: string,
    request: KChatBrandKitArtifactRequest,
  ): Promise<KChatArtifactPublishResult>;
  /// Phase 8 (Block A, Task 2): list previously-published
  /// artifacts for the given conversation.
  listArtifacts(
    conversationId: string,
  ): Promise<KChatPublishedArtifact[]>;
}

// -----------------------------------------------------------------------------
// Phase 4 — Vision & Image Generation
// -----------------------------------------------------------------------------

/** Mirror of `kcreate_bridge::phase4::VisionStatusInfo`. */
export interface VisionStatus {
  state: "stopped" | "starting" | "ready" | "error";
  // Phase 12 collapsed the vision dispatcher down to a single
  // runtime (`llama_server`). The field is retained — typed as a
  // string union to preserve forward-compat — so a future
  // Rust-native runtime can extend the variant set without
  // requiring every renderer caller to be re-typed.
  runtime: "llama_server" | null;
  port: number | null;
  modelName: string | null;
  error: string | null;
}

/** Mirror of `kcreate_ai::brand_extract::BrandExtraction`. */
export interface BrandExtraction {
  colors: string[];
  fonts: string[];
  spacing: number[];
}

/** Mirror of `kcreate_ai::smart_crop::CropSuggestion`. */
export interface CropSuggestion {
  x: number;
  y: number;
  w: number;
  h: number;
  confidence: number;
}

/** Mirror of `kcreate_ai::design_tokens_vlm::DesignTokenSuggestion`. */
export interface DesignTokenSuggestion {
  spacing: number[];
  colors: string[];
  typography: string[];
}

/** Mirror of `kcreate_ai::style_describe::StyleDescription`. */
export interface StyleDescription {
  summary: string;
  colorMood: string[];
  typography: string[];
  layout: string[];
}

/** Mirror of `kcreate_bridge::phase4::ImageGenStatusInfo`. */
export interface ImageGenStatus {
  state: "stopped" | "starting" | "ready" | "error";
  port: number | null;
  error: string | null;
  /** Hard gate: when false, the renderer must not show the panel. */
  allowed: boolean;
}

/** Mirror of `kcreate_bridge::phase4::GeneratedImagePayload`. */
export interface GeneratedImage {
  width: number;
  height: number;
  pngB64: string;
}

/**
 * Vision (VLM) bridge. Runs a multimodal sidecar (llama-server
 * with an mmproj projector) on loopback and exposes describe /
 * alt-text / critique operations. Soft-gated: available on every
 * tier, but the dispatcher picks model size by `RuntimeConfig`.
 */
export interface VisionBridge {
  start(packId: string): Promise<number>;
  stop(): Promise<void>;
  status(): Promise<VisionStatus>;
  describeImage(
    rgba: Uint8Array,
    width: number,
    height: number,
    userPrompt: string,
  ): Promise<string>;
  describeNode(nodeId: string, userPrompt: string): Promise<string>;
  generateAltText(
    rgba: Uint8Array,
    width: number,
    height: number,
  ): Promise<string>;
  generateAltTextForNode(nodeId: string): Promise<string>;
  analyzeDesign(
    rgba: Uint8Array,
    width: number,
    height: number,
  ): Promise<string>;
  extractBrand(
    rgba: Uint8Array,
    width: number,
    height: number,
  ): Promise<BrandExtraction>;
  suggestCrop(
    rgba: Uint8Array,
    width: number,
    height: number,
    aspectRatio: number,
  ): Promise<CropSuggestion>;
  suggestDesignTokens(
    rgba: Uint8Array,
    width: number,
    height: number,
  ): Promise<DesignTokenSuggestion>;
  describeStyle(
    rgba: Uint8Array,
    width: number,
    height: number,
  ): Promise<StyleDescription>;
  /** Recommended pack id for the current device, or `""`. */
  recommendedPack(): Promise<string>;
  /** mmproj companion pack id for a vision pack, or `""`. */
  mmprojFor(packId: string): Promise<string>;
  /** Pack ids the UI is allowed to show in the vision section. */
  listablePacks(): Promise<string[]>;
}

/**
 * Image-generation bridge. Hard-gated on Tier 2+ with GPU; the
 * `allowed` flag in [`ImageGenStatus`] mirrors
 * `RuntimeConfig::image_generation_allowed`. The renderer must drop
 * the entire panel — not just disable it — when `allowed === false`.
 */
export interface ImageGenBridge {
  start(packId: string): Promise<number>;
  stop(): Promise<void>;
  status(): Promise<ImageGenStatus>;
  generate(
    prompt: string,
    width: number,
    height: number,
    steps: number,
    seed: number | null,
  ): Promise<GeneratedImage>;
  allowed(): Promise<boolean>;
  recommendedPack(): Promise<string>;
}

/**
 * Phase 6 Tasks 25-26 — node clipboard.
 *
 * Mirrors `kcreate_bridge::document::{document_clipboard_copy,
 * document_clipboard_paste}`. The renderer drives Ctrl+C / Ctrl+V
 * through this surface: copy returns a self-contained JSON payload
 * the main process stashes on the OS clipboard, paste accepts that
 * payload back and instantiates fresh nodes (new ids, optional
 * cursor-offset, recorded as an undoable `clipboard_paste`
 * operation).
 *
 * The payload format is opaque to the renderer — Rust pins
 * `version: 1` and rejects future versions explicitly so a future
 * schema bump can't silently drop data.
 */
/**
 * Phase 7 (Block E) — `kcreate://` deeplink listener exposed by the
 * preload bridge. The main process registers the protocol via
 * `app.setAsDefaultProtocolClient("kcreate")` and forwards every
 * accepted URL to the renderer through the
 * `kcreate/deeplink/received` IPC channel; `InvitePanel.tsx`
 * subscribes here so a share-card click in KChat Desktop auto-fills
 * + accepts the invite when KCreate is already running.
 */
export interface DeeplinkBridge {
  /**
   * Register a callback that fires whenever a `kcreate://...` URL is
   * dispatched by the OS shell. Returns an `unsubscribe` function;
   * call it in a `useEffect` cleanup to detach the listener when
   * the panel unmounts. Idempotent on the host side — repeated
   * subscriptions each get their own slot on the IPC channel.
   */
  onUrl(callback: (url: string) => void): () => void;
}

export interface ClipboardBridge {
  /**
   * Serialise `nodeIds` (each with their descendants) into a portable
   * JSON payload. Page and Artboard ids are filtered out defensively —
   * those have dedicated `page_duplicate` / `artboard_*` ops and must
   * not flow through the generic clipboard.
   */
  copy(nodeIds: string[]): Promise<string>;
  /**
   * Deserialise `payload` and insert each subtree under
   * `targetParentId` (or document root when `null`). Every id is
   * regenerated so the paste is independent of the source nodes;
   * each subtree's top-level root is offset by (`offsetX`, `offsetY`)
   * so paste-at-cursor doesn't perfectly overlap the original.
   * Returns the new root ids in source order.
   */
  paste(
    payload: string,
    targetParentId: string | null,
    offsetX: number,
    offsetY: number,
  ): Promise<string[]>;
}

declare global {
  interface Window {
    kcreate: {
      renderer: RendererBridge;
      document: DocumentBridge;
      canvas: CanvasBridge;
      ai: AiBridge;
      llm: LlmBridge;
      vision: VisionBridge;
      imageGen: ImageGenBridge;
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
      templateMarketplace: TemplateMarketplaceBridge;
      audit: AuditBridge;
      thumbnail: ThumbnailBridge;
      recentProjects: RecentProjectsBridge;
      preflight: PreflightBridge;
      iconPack: IconPackBridge;
      batch: BatchBridge;
      aiModel: AiModelBridge;
      pdfImport: PdfImportBridge;
      figmaImport: FigmaImportBridge;
      sketchImport: SketchImportBridge;
      plugin: PluginBridge;
      mcpPermission: McpPermissionBridge;
      color: ColorBridge;
      canvasSnap: CanvasSnapBridge;
      rasterOps: RasterOpsBridge;
      textFrame: TextFrameBridge;
      text: TextBridge;
      vectorOps: VectorOpsBridge;
      slice: SliceBridge;
      session: SessionBridge;
      kchat: KChatBridge;
      kchatBackend: KChatBackendBridge;
      deeplink: DeeplinkBridge;
      clipboard: ClipboardBridge;
      phase8: Phase8Bridge;
      phase9: Phase9Bridge;
      phase10: Phase10Bridge;
      projectEncryption: ProjectEncryptionBridge;
      annotation: AnnotationBridge;
      system: SystemBridge;
      onboarding: OnboardingBridge;
    };
  }
}

/**
 * Wire-format mirror of `kcreate_text::tokens::PageNumberFormat`.
 * Matches the Rust serde `rename_all = "snake_case"` form.
 */
export type PageNumberFormat =
  | "arabic"
  | "roman_lower"
  | "roman_upper"
  | "alpha_lower"
  | "alpha_upper";

/**
 * Wire-format mirror of `kcreate_text::tokens::PageContext`. One
 * entry per page in the document, in document order. The shaper
 * uses these to substitute page-number tokens at render time.
 *
 * `section_prefix` is `null` when the page (and its enclosing
 * section) has no prefix configured — mirrors the Rust
 * `Option<String>` shape exactly.
 */
export interface PageContext {
  display_number: number;
  section_total: number;
  section_prefix: string | null;
}

/**
 * Wire-format mirror of `kcreate_export::job_presets::JobType`. The
 * `Phase8Bridge.exportJobPresets` API accepts the short snake_case
 * aliases (`app_ui`, `logo`, etc.) for ergonomics.
 */
export type JobType =
  | "app_or_website_ui"
  | "app_ui"
  | "logo_icon_or_brand_kit"
  | "logo"
  | "social_media_post"
  | "social_post"
  | "product_photo_cleanup"
  | "product_photo"
  | "pitch_deck_or_proposal"
  | "pitch_deck"
  | "flyer_poster_or_brochure"
  | "flyer_poster"
  | "developer_asset_export"
  | "developer_asset";

/**
 * Wire-format mirror of `kcreate_export::job_presets::JobExportPreset`.
 */
export interface JobExportPreset {
  name: string;
  format: string;
  scale: number;
  width?: number | null;
  height?: number | null;
  bleed_mm?: number | null;
  background?: string | null;
}

/**
 * Wire-format mirror of `kcreate_export::job_presets::JobExportPresets`.
 */
export interface JobExportPresets {
  job_type: string;
  presets: JobExportPreset[];
}

/**
 * Wire-format mirror of `kcreate_bridge::phase8::BrandKitVersionInfo`.
 */
export interface BrandKitVersionInfo {
  versionId: string;
  brandKitId: string;
  timestamp: string;
  description: string;
}

/**
 * Wire-format mirror of `kcreate_core::node::RgbaColor`. Channels
 * are floats in `[0.0, 1.0]`.
 */
export interface RgbaColor {
  r: number;
  g: number;
  b: number;
  a: number;
}

/**
 * Wire-format mirror of `kcreate_core::project::NamedColor`. A
 * brand-kit colour swatch with a user-facing name and the resolved
 * `RgbaColor`.
 */
export interface NamedColor {
  name: string;
  color: RgbaColor;
}

/**
 * Wire-format mirror of `kcreate_storage::brand_versions::ColorChange`.
 * Captures a swatch whose colour value changed between two snapshots,
 * keyed by `name` so the UI can render `before` / `after` swatches
 * side-by-side.
 */
export interface ColorChange {
  name: string;
  before: NamedColor;
  after: NamedColor;
}

/**
 * Wire-format mirror of `kcreate_storage::brand_versions::BrandKitDiff`.
 *
 * All fields are present on every diff — empty arrays / `false` /
 * `null` are valid "no change" values. `name_changed` is a JSON
 * tuple `[before, after]` because Rust's `Option<(String, String)>`
 * serialises that way; the UI should index `[0]` / `[1]` rather
 * than `.before` / `.after`.
 */
export interface BrandKitDiff {
  added_colors: NamedColor[];
  removed_colors: NamedColor[];
  changed_colors: ColorChange[];
  added_fonts: string[];
  removed_fonts: string[];
  spacing_changed: boolean;
  export_rules_changed: boolean;
  name_changed: [string, string] | null;
}

/**
 * Wire-format mirror of `kcreate_core::node::Bounds` used by
 * `Phase8Bridge.documentResizeFrame`.
 */
export interface ResizeFrameBounds {
  x: number;
  y: number;
  width: number;
  height: number;
}

/**
 * Wire-format mirror of `kcreate_core::node::Constraint`. Describes
 * how one axis (horizontal or vertical) of a node responds when its
 * parent frame is resized. Tags are the Rust `snake_case` rename
 * (`fixed`, `min`, `max`, `center`, `scale`, `stretch`).
 */
export type Constraint =
  | "fixed"
  | "min"
  | "max"
  | "center"
  | "scale"
  | "stretch";

/**
 * Wire-format mirror of `kcreate_core::node::Constraints`. A
 * horizontal + vertical [`Constraint`] pair stored on every node;
 * applied by `Phase8Bridge.resizeFrame` to recompute child bounds
 * after a parent resize.
 */
export interface Constraints {
  horizontal: Constraint;
  vertical: Constraint;
}

/**
 * Phase 8 production-hardening bridge. Wraps the N-API entry points
 * for design-token binding, constraint-aware frame resize, text
 * auto-fit, page-numbering tokens, section numbering, job-first
 * export presets, and brand-kit versioning.
 */
export interface Phase8Bridge {
  bindToken(nodeId: string, property: string, tokenName: string): Promise<void>;
  unbindToken(nodeId: string, property: string): Promise<void>;
  propagateToken(tokenName: string): Promise<number>;
  resizeFrame(frameId: string, bounds: ResizeFrameBounds): Promise<void>;
  /// Snapshot the token bindings on the supplied node (snake_case
  /// property → token name). Empty map for unbound nodes.
  nodeTokenBindings(nodeId: string): Promise<Record<string, string>>;
  /// Snapshot the resize constraints on the supplied node.
  nodeConstraints(nodeId: string): Promise<Constraints>;
  /// Update the resize constraints on the supplied node. Records an
  /// undoable operation; bounds are not recomputed until the next
  /// parent resize.
  setNodeConstraints(
    nodeId: string,
    constraints: Constraints,
  ): Promise<void>;
  setAutoFit(nodeId: string, enabled: boolean): Promise<boolean>;
  pageNumberToken(format: PageNumberFormat): Promise<string>;
  setPageSection(
    pageId: string,
    startNumber: number | null,
    prefix: string | null,
  ): Promise<void>;
  resolvePageContexts(): Promise<PageContext[]>;
  exportJobPresets(job: JobType): Promise<JobExportPresets>;
  brandKitSaveVersion(
    brandKitId: string,
    description: string,
  ): Promise<BrandKitVersionInfo>;
  brandKitListVersions(brandKitId: string): Promise<BrandKitVersionInfo[]>;
  brandKitRestoreVersion(versionId: string): Promise<BrandKit>;
  brandKitDiff(beforeId: string, afterId: string): Promise<BrandKitDiff>;
}

// ---------------------------------------------------------------------
// Phase 8 (Task 26) — project encryption.
//
// Wraps the SQLCipher passphrase / re-key / recovery flows. The
// renderer never sees raw key material — only passphrases cross the
// IPC boundary; the bridge derives the key on the Rust side using
// PBKDF2-HMAC-SHA256 against a per-project salt persisted in
// `manifest.json`.
// ---------------------------------------------------------------------

/**
 * Wire-format mirror of `kcreate_bridge::encryption::EncryptionStatus`.
 * Reported by `ProjectEncryptionBridge.status()`; when `enabled` is
 * `false`, the rest of the fields carry stub values (empty salt,
 * default iteration count) and the renderer should hide the
 * change-passphrase / export-recovery controls.
 */
export interface EncryptionStatus {
  /** Whether the active project's database is SQLCipher-encrypted. */
  enabled: boolean;
  /**
   * PBKDF2 iteration count in use. Reported even when disabled so
   * the UI can pre-populate cost-factor sliders for the enable
   * flow.
   */
  iterations: number;
  /**
   * Base64 url-safe (no padding) per-project salt. Empty when
   * `enabled` is `false`.
   */
  salt: string;
}

/**
 * Phase 8 Task 26 — project encryption bridge. All operations are
 * scoped to the currently-open project; they fail with a
 * `DocumentBridgeError::ProjectNotOpen` (surfaced as a JS Error) if
 * no project is open.
 */
export interface ProjectEncryptionBridge {
  /** Snapshot the current encryption state. */
  status(): Promise<EncryptionStatus>;
  /**
   * Pure passphrase-strength score in `[0, 4]` matching the
   * standard weak/fair/good/strong/very-strong scale. Does not
   * touch the workspace.
   */
  passphraseStrength(passphrase: string): Promise<number>;
  /**
   * Encrypt the active project's database with `passphrase`.
   * Fails if encryption is already enabled.
   */
  enable(passphrase: string): Promise<EncryptionStatus>;
  /** Rotate the project passphrase. */
  changePassphrase(
    oldPassphrase: string,
    newPassphrase: string,
  ): Promise<void>;
  /**
   * Export a plaintext copy of the project's database to
   * `outputPath`. The encrypted source is left untouched.
   */
  exportPlaintextRecovery(
    passphrase: string,
    outputPath: string,
  ): Promise<string>;
  /**
   * Open the OS-native save-file dialog scoped to the plaintext
   * recovery export (filters to `.sqlite` / `.db`,
   * `showOverwriteConfirmation` enabled). Returns the absolute
   * chosen path, or `null` if the user cancelled. Implemented in
   * the main process so the renderer never sees the user's
   * filesystem (mirrors the existing `kcreate/pdf/pickFile` /
   * `kcreate/sketch/pickFile` pattern).
   */
  pickRecoveryPath(): Promise<string | null>;
}

// ---------------------------------------------------------------------
// Phase 8 (Task 4) — design-review annotations.
//
// Wire-format mirrors for the bridge surface in
// `crates/kcreate_bridge/src/annotation_bridge.rs`. Every shape uses
// `camelCase` because the Rust structs all carry
// `#[serde(rename_all = "camelCase")]`. Position is `f64` on the Rust
// side; JS numbers are IEEE-754 doubles so the precision matches.
// ---------------------------------------------------------------------

/**
 * Wire-format mirror of `kcreate_core::annotation::AnnotationPosition`.
 * World-coordinate location of the annotation pin on the artboard.
 * Stored as floats (`f64` on the Rust side) so a pin placed on a
 * large artboard does not lose precision after a round-trip.
 */
export interface AnnotationPosition {
  x: number;
  y: number;
}

/**
 * Wire-format mirror of `kcreate_core::annotation::Annotation`.
 *
 * - `timestamp` is an ISO-8601 RFC3339 string (chrono `DateTime<Utc>`).
 * - `threadId` is `null` for the head of a new thread; non-null for
 *   replies — it points at the thread's root annotation id so the
 *   sidebar UI can group every entry under one pin.
 */
export interface Annotation {
  id: string;
  pageId: string;
  authorPeerId: string;
  authorName: string;
  position: AnnotationPosition;
  text: string;
  timestamp: string;
  resolved: boolean;
  threadId: string | null;
}

/**
 * Request payload for `annotationCreate`. Posted by the
 * `AnnotationOverlay` when the user places a new pin on the canvas.
 */
export interface AnnotationCreateRequest {
  pageId: string;
  authorPeerId: string;
  authorName: string;
  position: AnnotationPosition;
  text: string;
}

/**
 * Request payload for `annotationReply`. Posts a threaded reply
 * onto an existing annotation; the bridge walks `parentId` to the
 * thread root so the reply is attached correctly regardless of
 * whether the UI passed the head id or a sibling reply's id.
 */
export interface AnnotationReplyRequest {
  parentId: string;
  authorPeerId: string;
  authorName: string;
  text: string;
}

/**
 * Request payload for `annotationList`. The two `include*` flags
 * cover the three useful filter combinations: open-only (default),
 * resolved-only (archive view), and all (show everything).
 */
export interface AnnotationListRequest {
  pageId: string;
  includeResolved: boolean;
  includeUnresolved: boolean;
}

/**
 * Response payload for `annotationList`. Wraps the array so the
 * N-API marshal layer returns a single JSON object instead of a
 * bare array — matches the rest of the Phase 8 bridge convention.
 */
export interface AnnotationListResponse {
  annotations: Annotation[];
}

/**
 * Request payload for `annotationResolve`. Used for both resolve
 * (`resolved: true`) and unresolve (`resolved: false`).
 */
export interface AnnotationResolveRequest {
  id: string;
  resolved: boolean;
}

/**
 * Annotation bridge — design-review CRUD over the project's local
 * SQLite store. When a collab session is active each verb also
 * broadcasts the mutation to connected peers via
 * `Message::AnnotationBroadcast` so every project DB converges
 * through the same upsert / delete helpers.
 */
export interface AnnotationBridge {
  create(request: AnnotationCreateRequest): Promise<Annotation>;
  reply(request: AnnotationReplyRequest): Promise<Annotation>;
  list(request: AnnotationListRequest): Promise<AnnotationListResponse>;
  resolve(request: AnnotationResolveRequest): Promise<boolean>;
  delete(id: string): Promise<boolean>;
}

// ---------------------------------------------------------------------------
// Phase 9 — wire-format types for the new bridge surface.
//
// Mirrors `crates/kcreate_bridge/src/phase9.rs`, `perf.rs`, and
// `autosave.rs`. All JSON-string boundaries are decoded in the
// preload layer (`window.kcreate.phase9.*`) so the renderer only
// sees typed objects.
// ---------------------------------------------------------------------------

/** Mirror of `kcreate_bridge::phase9::GuideInfo`. */
export interface GuideInfo {
  id: string;
  pageId: string;
  orientation: "horizontal" | "vertical";
  position: number;
  color: string;
  locked: boolean;
  createdAt: string;
}

/** Mirror of `kcreate_bridge::phase9::GridSettingsInfo`. */
export interface GridSettingsInfo {
  artboardId: string;
  enabled: boolean;
  spacing: number;
  subdivisions: number;
  color: string;
}

/** Mirror of `kcreate_bridge::phase9::AlignmentResult`. One per node
 * the align/distribute pass actually moved. */
export interface AlignmentResult {
  nodeId: string;
  oldBounds: ResizeFrameBounds;
  newBounds: ResizeFrameBounds;
}

/** Alignment keyword accepted by `documentAlign`. */
export type Alignment =
  | "left"
  | "center"
  | "right"
  | "top"
  | "middle"
  | "bottom";

/** Distribution axis accepted by `documentDistribute`. */
export type DistributeAxis = "horizontal" | "vertical";

/** Mirror of `kcreate_bridge::phase9::PaletteApplyResult`. */
export interface PaletteApplyResult {
  brandKitId: string;
  colors: NamedColor[];
}

/** Mirror of `kcreate_bridge::phase9::AutofitRecomputeResult`. */
export interface AutofitRecomputeResult {
  nodeId: string;
  previousSize: number;
  newSize: number;
}

/** Mirror of `kcreate_bridge::phase9::TraceResult`. */
export interface TraceResult {
  groupNodeId: string;
  pathCount: number;
  closedPathCount: number;
  pathNodeIds: string[];
}

/** Mirror of `kcreate_bridge::phase9::IconifyResultInfo`. */
export interface IconifyResultInfo {
  sourceNodeId: string;
  groupNodeId: string;
  pathCount: number;
  strokeWidth: number;
  gridSize: number;
}

/** Mirror of `kcreate_bridge::phase9::BatchAltTextEntry`. */
export interface BatchAltTextEntry {
  nodeId: string;
  altText: string;
  fallback: boolean;
}

/** Mirror of `kcreate_bridge::phase9::ImportSummary`. */
export interface ImportSummary {
  rootNodeId: string;
  pageNodeId: string | null;
  layerCount: number;
  warnings: string[];
}

/** Mirror of `kcreate_bridge::phase9::ExifResult`. */
export interface ExifResult {
  width: number | null;
  height: number | null;
  makeModel: string | null;
  dateTime: string | null;
  orientation: number | null;
  gps: [number, number] | null;
  raw: Record<string, string>;
}

/** Mirror of `kcreate_bridge::phase9::SvgPreviewInfo`. */
export interface SvgPreviewInfo {
  pngBytes: number[];
  width: number;
  height: number;
}

/** Mirror of `kcreate_bridge::phase9::OperationLogFilter`. */
export interface OperationLogFilter {
  aiOnly: boolean;
  manualOnly: boolean;
  since: string | null;
  until: string | null;
  limit: number;
}

/** Mirror of `kcreate_bridge::phase9::OperationInfo`. */
export interface OperationInfo {
  id: string;
  timestamp: string;
  actor: string;
  command: string;
  affectedNodes: string[];
  aiGenerated: boolean;
  groupId: string | null;
  isUndo: boolean;
}

/** Mirror of `kcreate_export::validate::ExportSeverity`. */
export type ExportSeverity = "error" | "warning";

/** Mirror of `kcreate_export::validate::ExportValidationIssue`. */
export interface ExportValidationIssue {
  severity: ExportSeverity;
  code: string;
  message: string;
}

/** Mirror of `kcreate_export::validate::ExportValidationRequest`.
 *
 * Field names and optionality mirror the Rust struct exactly so that
 * `JSON.stringify` of this object deserialises cleanly into the Rust
 * value via `serde(rename_all = "camelCase")`. Keep the four boolean
 * attributes flat — see the Rust doc comment for the rationale. */
export interface ExportValidationRequest {
  /** One or more node IDs to export. Empty is invalid. */
  nodeIds: string[];
  /** Target format. Currently one of `png`, `jpeg`, `webp`, `svg`, `pdf`. */
  format: string;
  /** Optional explicit output width in pixels. `0` is rejected. */
  width: number | null;
  /** Optional explicit output height in pixels. */
  height: number | null;
  /** JPEG quality slider in `[1, 100]`, if format = `jpeg`. */
  jpegQuality: number | null;
  /** Whether the request wants a transparent background. */
  transparent: boolean;
  /** If true, suppress non-fatal warnings about oversized dimensions. */
  forceOversized: boolean;
  /** True if any of the selected nodes has text content. */
  hasText: boolean;
  /** True if the bridge could not find a system font that covers every
   * glyph in the selection. */
  missingFonts: boolean;
}

/** Mirror of `kcreate_export::validate::ExportValidationReport`. */
export interface ExportValidationReport {
  ok: boolean;
  issues: ExportValidationIssue[];
}

/** Mirror of `kcreate_bridge::phase9::BriefStarterLayer`. */
export interface BriefStarterLayer {
  name: string;
  kind: "text" | "shape" | "image" | "group";
  suggestedContent: string | null;
}

/** Mirror of `kcreate_bridge::phase9::BriefPlan`. */
export interface BriefPlan {
  artboardPreset: string;
  palette: string[];
  starterLayers: BriefStarterLayer[];
}

/** Mirror of `kcreate_bridge::phase9::BriefApplyResult`. */
export interface BriefApplyResult {
  artboardId: string;
  brandKitId: string;
  layerIds: string[];
}

/** Mirror of `kcreate_bridge::perf::MemoryPressureEvent`. */
export type MemoryPressureEvent =
  | { kind: "entered"; available_mb: number; threshold_mb: number }
  | { kind: "released"; available_mb: number; threshold_mb: number };

/** Mirror of `kcreate_bridge::autosave::AutosaveStatus`. */
export interface AutosaveStatus {
  running: boolean;
  intervalSecs: number;
  lastSavedAt: string | null;
  pendingChanges: boolean;
}

/** Mirror of `kcreate_bridge::autosave::AutosaveMarker`. */
export interface AutosaveMarker {
  projectPath: string;
  autosavePath: string;
  capturedAt: string;
  cleanRevision: number;
}

/**
 * Phase 9 bridge surface. Each method round-trips through the
 * `kcreate/phase9/*` IPC channels; preload decodes the JSON strings
 * returned by the napi entry points so callers see typed objects.
 */
export interface Phase9Bridge {
  // -------- Block D Task 21 — Guides ----------------------------------
  guideCreate(
    pageId: string,
    orientation: "horizontal" | "vertical",
    position: number,
    color: string | null,
    locked: boolean,
  ): Promise<GuideInfo>;
  guideDelete(id: string): Promise<boolean>;
  guideClearPage(pageId: string): Promise<number>;
  guideList(pageId: string): Promise<GuideInfo[]>;
  guideListAll(): Promise<GuideInfo[]>;

  // -------- Block D Task 22 — Grid settings ---------------------------
  artboardGridSettings(artboardId: string): Promise<GridSettingsInfo>;
  artboardSetGrid(
    artboardId: string,
    enabled: boolean,
    spacing: number,
    subdivisions: number,
    color: string | null,
  ): Promise<GridSettingsInfo>;

  // -------- Block D Task 23 — Alignment / distribution ----------------
  documentAlign(
    nodeIds: string[],
    alignment: Alignment,
  ): Promise<AlignmentResult[]>;
  documentDistribute(
    nodeIds: string[],
    axis: DistributeAxis,
  ): Promise<AlignmentResult[]>;

  // -------- Block B Task 10 — Palette → brand kit ---------------------
  paletteExtractAndApplyBrandKit(
    nodeId: string,
    numColors: number,
    brandKitName: string,
  ): Promise<PaletteApplyResult>;

  // -------- Block B Task 11 — Text autofit ----------------------------
  textAutofitRecompute(nodeId: string): Promise<AutofitRecomputeResult>;

  // -------- Block B Task 12 — Raster → vector trace -------------------
  aiTraceRaster(
    nodeId: string,
    threshold: number,
    simplifyTolerance: number,
  ): Promise<TraceResult>;

  // -------- Block D Task 19 — Icon-ify --------------------------------
  aiIconify(sourceNodeId: string, gridSize: number): Promise<IconifyResultInfo>;

  // -------- Block D Task 20 — Batch alt-text --------------------------
  aiBatchAltText(pageId: string): Promise<BatchAltTextEntry[]>;

  // -------- Block C Tasks 13–15 — PSD / Penpot / EXIF -----------------
  importPsd(path: string): Promise<ImportSummary>;
  importPenpot(path: string): Promise<ImportSummary>;
  imageReadExif(bytes: Uint8Array): Promise<ExifResult>;

  // -------- Block C Task 16 — SVG preview -----------------------------
  exportSvgPreview(
    svgBytes: Uint8Array,
    maxWidth: number,
    maxHeight: number,
    transparent: boolean,
  ): Promise<SvgPreviewInfo>;

  // -------- Block C Task 17 — History panel ---------------------------
  operationLogFilter(filter: OperationLogFilter): Promise<OperationInfo[]>;

  // -------- Block E Task 27 — Export validation -----------------------
  exportValidate(
    request: ExportValidationRequest,
  ): Promise<ExportValidationReport>;

  // -------- Block B Task 7 — Brief → project --------------------------
  briefToProject(plan: BriefPlan): Promise<BriefApplyResult>;

  // -------- Block E Task 25 — Memory watchdog -------------------------
  memoryWatchdogStart(pollIntervalMs: number): Promise<boolean>;
  memoryWatchdogStop(): Promise<boolean>;
  drainMemoryEvents(): Promise<MemoryPressureEvent[]>;
  runtimeGpuBackendName(): Promise<string>;

  // -------- Block E Task 26 — Autosave --------------------------------
  autosaveStart(): Promise<boolean>;
  autosaveStop(): Promise<boolean>;
  autosaveForceNow(): Promise<boolean>;
  autosaveStatus(): Promise<AutosaveStatus>;
  autosaveRecoveryAvailable(): Promise<AutosaveMarker | null>;
  autosaveRecover(): Promise<void>;
  autosaveDismissRecovery(): Promise<void>;
}

// =============================================================
// Phase 10 — Image Studio AI, Vector/Layout AI, Export AI +
// Live Preview, Brand Hub + Plugin Marketplace, Preferences.
// Wire-format mirrors of `crates/kcreate_bridge/src/phase10.rs`.
// =============================================================

/** Result of `aiDenoise`. New raster node id + dimensions. */
export interface DenoiseResult {
  newNodeId: string;
  width: number;
  height: number;
}

/** Result of `aiInpaint`. */
export interface InpaintResult {
  newNodeId: string;
  width: number;
  height: number;
}

/** Auto-color mode (matches Rust `AutoColorMode` serde). */
export type AutoColorMode =
  | "auto_levels"
  | "white_balance"
  | "histogram_equalization"
  | "combined";

/** Result of `aiAutoColor`. */
export interface AutoColorResult {
  newNodeId: string;
  mode: string;
  width: number;
  height: number;
}

/** Result of `aiSegmentAtPoint`. */
export interface SegmentAtPointResult {
  /** Base64-encoded `width*height` byte mask (255 fg, 0 bg). */
  maskBase64: string;
  width: number;
  height: number;
  /** Backend that produced the mask (`edge_aware` | `sam`). */
  backend: string;
}

/** Set-op mode for `aiSmartSelectAtPoint`. */
export type SmartSelectMode = "replace" | "add" | "subtract";

/** Result of `aiSmartSelectAtPoint`. */
export interface SmartSelectAtPointResult {
  maskBase64: string;
  width: number;
  height: number;
  mode: string;
  selectedPixelCount: number;
}

/** Rectangle for inpainting mask input. */
export interface InpaintMaskRect {
  x: number;
  y: number;
  w: number;
  h: number;
}

/**
 * Flattened stroke properties — wire-format mirror of
 * `kcreate_ai::stroke_match::StrokeProperties` (`#[serde(rename_all = "camelCase")]`).
 */
export interface StrokeProperties {
  /** `#RRGGBBAA` lowercase hex, matching the renderer convention. */
  colorHex: string;
  width: number;
  /** Dash array; empty when the stroke is solid. */
  dash: number[];
  cap: string;
  join: string;
  /** `(t, width)` pairs describing a variable-width profile, or `null` for uniform. */
  widthProfile: Array<[number, number]> | null;
}

/** Per-target record returned for each node that received the source's stroke. */
export interface StrokeDeltaApplied {
  targetNodeId: string;
  /** `true` when the target had a previous stroke that got overwritten. */
  hadPreviousStroke: boolean;
}

/** Result of `aiMatchStroke`. Mirror of Rust `StrokeMatchSummary`. */
export interface StrokeMatchSummary {
  sourceNodeId: string;
  applied: StrokeDeltaApplied[];
  sourceProperties: StrokeProperties;
}

/** Result of `aiExtractGlyph`. */
export interface ExtractedGlyphResult {
  /** Serialized vector paths as JSON. */
  pathsJson: string;
  emSize: number;
  /** `(minX, minY, maxX, maxY)`. */
  boundingBox: [number, number, number, number];
}

/**
 * One placement on a reformatted deck page. Mirror of Rust
 * `kcreate_ai::reformat::ReformatPagePlacement`
 * (`#[serde(rename_all = "camelCase")]`). Coordinates are flat
 * (`newX`/`newY`/`newWidth`/`newHeight`) — there is no nested
 * `bounds` object.
 */
export interface ReformatPagePlacement {
  sourceNodeId: string;
  newX: number;
  newY: number;
  newWidth: number;
  newHeight: number;
  scale: number;
}

/**
 * One page in a reformatted deck. Mirror of Rust
 * `kcreate_ai::reformat::ReformatPage`. The Rust field is
 * `placements`, not `nodes`, and the index is exposed at the page
 * level.
 */
export interface ReformatPage {
  index: number;
  title: string;
  placements: ReformatPagePlacement[];
}

/** Result of `aiReformatToDeck`. Mirror of Rust `ReformatDeckResult`. */
export interface ReformatDeckResult {
  pages: ReformatPage[];
  pageWidth: number;
  pageHeight: number;
}

/** Section type returned by `aiBriefToOnePager`. */
export type OnePagerSectionType =
  | "header"
  | "body"
  | "image_placeholder"
  | "callout";

/**
 * Mirror of Rust `kcreate_ai::one_pager::OnePagerSection`
 * (`#[serde(rename_all = "camelCase")]`). Coordinates are flat —
 * the Rust struct has `x`, `y`, `width`, `height` at the top level
 * rather than nested under `bounds`.
 */
export interface OnePagerSection {
  sectionType: OnePagerSectionType;
  text: string;
  x: number;
  y: number;
  width: number;
  height: number;
}

/** Result of `aiBriefToOnePager`. Mirror of Rust `BriefToOnePagerResult`. */
export interface BriefToOnePagerResult {
  sections: OnePagerSection[];
  pageWidth: number;
  pageHeight: number;
}

/**
 * Built-in professional theme identifiers for the Gamma-style themed
 * design generator. Mirror of `kcreate_ai::themed_deck::ThemeId`
 * (`#[serde(rename_all = "camelCase")]`).
 */
export type ThemeId = "midnight" | "sunrise" | "forest" | "ember" | "slate";

/** Output format for the themed design generator. */
export type ThemedDesignFormat = "deck" | "onePager";

/** Page size for one-pager output (ignored for decks). */
export type ThemedOnePagerSize = "letter" | "a4" | "square";

/**
 * Options for `aiGenerateThemedDesign`. Serialized to the
 * `options_json` argument the bridge parses into
 * `kcreate_bridge::phase10::ThemedDesignRequest`
 * (`#[serde(rename_all = "camelCase")]`, every field optional). An
 * empty object yields a Midnight A4 six-slide deck.
 */
export interface ThemedDesignOptions {
  format?: ThemedDesignFormat;
  themeId?: ThemeId;
  onePagerSize?: ThemedOnePagerSize;
  /** Number of content sections; clamped per format by the bridge. */
  sectionCount?: number;
  /**
   * Opt-in LLM enrichment. When `true` *and* the local sidecar is
   * `ready`, the brief is expanded into a structured outline by the
   * model; on any failure the deterministic planner is used instead.
   */
  useLlm?: boolean;
}

/**
 * Mirror of `kcreate_bridge::phase10::ThemedDesignApplyResult`
 * (`#[serde(rename_all = "camelCase")]`). Returned after the generated
 * themed design has been applied to the open document.
 */
export interface ThemedDesignApplyResult {
  pageId: string;
  artboardIds: string[];
  brandKitId: string;
  slideCount: number;
  themeId: ThemeId;
  themeName: string;
  format: ThemedDesignFormat;
  usedLlm: boolean;
}

/** Harmony type for `aiHarmonizePalette`. */
export type HarmonyType =
  | "auto"
  | "complementary"
  | "triadic"
  | "analogous"
  | "split_complementary"
  | "tetradic";

/**
 * Mirror of Rust `kcreate_ai::palette_harmonize::HarmonySuggestion`
 * (`#[serde(rename_all = "camelCase")]`).
 */
export interface HarmonySuggestion {
  inputHex: string;
  suggestedHex: string;
  hueShiftDegrees: number;
}

/** Mirror of Rust `HarmonyResult`. */
export interface HarmonyResult {
  /** Serialized as snake_case (`HarmonyRule` uses `rename_all = "snake_case"`). */
  rule: HarmonyType;
  suggestions: HarmonySuggestion[];
}

/** Result of `aiSuggestTypePairing`. */
export interface TypePairingSuggestion {
  fontName: string;
  reason: string;
  confidence: number;
}

/** Mirror of Rust `TypePairingResult` (`#[serde(rename_all = "camelCase")]`). */
export interface TypePairingResult {
  headingFont: string;
  /** Heading-font classification — `serif` | `sans` | `mono` | `display` | `script`. */
  headingCategory: string;
  suggestions: TypePairingSuggestion[];
}

/** Result of `exportOptimizeSvg`. */
export interface SvgOptimizeReport {
  originalBytes: number;
  optimisedBytes: number;
  bytesSaved: number;
  ratio: number;
  outputSvg: string;
}

/** Result of `exportSmartCompress`. */
export interface SmartCompressReport {
  quality: number;
  format: "jpeg" | "webp";
  originalBytes: number;
  compressedBytes: number;
  ratio: number;
  ssim: number;
  iterations: number;
  /** Base64-encoded compressed bytes. */
  bytes: string;
}

/** Request for `exportPreview`. */
export interface ExportPreviewRequest {
  nodeId: string;
  /** `png` | `jpeg` | `jpg` | `webp`. */
  format: string;
  maxDimensionPx?: number | null;
}

/** Result of `exportPreview`. */
export interface ExportPreviewResponse {
  bytesBase64: string;
  mimeType: string;
  width: number;
  height: number;
  byteSize: number;
}

/** Result of `importAi`. */
export interface AiImportSummary {
  path: "svg" | "pdf";
  widthPt: number | null;
  heightPt: number | null;
  objectCount: number;
  message: string | null;
  svgPayloadBase64: string;
}

/**
 * Mirror of Rust `BrochureSection` in
 * `kcreate_bridge::phase10` (`#[serde(rename_all = "camelCase")]`).
 * Coordinates are flat (no nested `bounds`).
 */
export interface BrochureSection {
  sectionKind: string;
  x: number;
  y: number;
  width: number;
  height: number;
  /** Hex string `#RRGGBB` of the brand-token applied to this section. */
  styleColorHex: string | null;
}

/** Mirror of Rust `BrochurePage`. */
export interface BrochurePage {
  index: number;
  /** `cover` | `content` | `back`. */
  pageType: string;
  sections: BrochureSection[];
}

/** Result of `aiBrandToBrochure`. Mirror of Rust `BrochurePlanResult`. */
export interface BrochurePlanResult {
  pages: BrochurePage[];
  brandKitId: string;
}

/** Listing returned by `pluginMarketplaceList`. */
export interface PluginListing {
  id: string;
  name: string;
  version: string;
  author: string;
  description: string;
  permissions: string[];
  trustStatus: string;
  installed: boolean;
}

/** Result of `exportPdfMulti`. Mirror of Rust `PdfMultiReport`. */
export interface PdfMultiReport {
  pageCount: number;
  bytesWritten: number;
  tocEmitted: boolean;
  /** `true` when the bookmarks/outline tree was emitted. */
  bookmarksEmitted: boolean;
}

/** Mirror of `kcreate_bridge::phase10::Preferences`. */
export interface Preferences {
  general: {
    theme: "dark" | "light" | "system";
    language: string;
    autosaveIntervalSec: number;
    /**
     * Days of `.kstudio` scratch-project retention before the
     * autosaver garbage-collects them. `0` disables the sweep.
     * Mirrors `GeneralPrefs::scratch_project_cleanup_days` (u32).
     */
    scratchProjectCleanupDays: number;
  };
  canvas: {
    defaultGridSpacing: number;
    defaultGridSubdivisions: number;
    snapThresholdPx: number;
    rulerUnits: "px" | "mm" | "in" | "pt";
  };
  ai: {
    defaultLlmModel: string;
    autoStartSidecar: boolean;
    gbnfGrammarDebugging: boolean;
  };
  performance: {
    rasterCacheBudgetMb: number;
    undoDepthOverride: number | null;
    lowResourceMode: boolean;
  };
  privacy: {
    telemetryOptIn: boolean;
    auditLogRetentionDays: number;
  };
  /**
   * Phase A2 — sticky directory state for the native save-as
   * dialog. `lastDirByFormat` keys are the wire-format export
   * names (`"png"`, `"svg"`, `"pdf"`, `"webp"`, `"jpeg"`); values
   * are absolute directory paths the user last picked. The
   * renderer passes the entry as `defaultDir` to
   * `chooseExportTarget` so consecutive exports for the same
   * format open in the same place.
   *
   * `lastBatchDir` is the absolute directory last picked via
   * `chooseExportDirectory`; `null` until the first batch run.
   */
  export: {
    lastDirByFormat: Record<string, string>;
    lastBatchDir: string | null;
  };
  /**
   * Phase C — first-run welcome modal state. `completed` flips to
   * `true` the first time any close path of the modal fires
   * (install succeeded, manual file install succeeded, user
   * clicked "Skip"). `lastSeenPackId` records the recommendation
   * id the user was shown so a future tier-change pass can
   * detect when the recommended pack rolled over.
   *
   * Mirrors `OnboardingPrefs` in `crates/kcreate_bridge/src/phase10.rs`;
   * the field is `#[serde(default)]`-loaded on the Rust side so
   * a preferences file written before Phase C continues to
   * deserialise into a default-`completed=false` value — which is
   * the correct first-run behaviour for an existing user who has
   * never seen the welcome modal.
   */
  onboarding: {
    completed: boolean;
    lastSeenPackId: string | null;
  };
}

/**
 * Phase 10 bridge surface. Each method round-trips through the
 * `kcreate/phase10/*` IPC channels; preload decodes JSON strings
 * returned by the napi entry points so callers see typed objects.
 */
export interface Phase10Bridge {
  // -------- Block A — Image Studio AI -----------------------
  aiDenoise(
    nodeId: string,
    strength: number,
    searchRadius: number,
    patchRadius: number,
  ): Promise<DenoiseResult>;
  aiInpaint(
    nodeId: string,
    maskRects: InpaintMaskRect[],
    patchRadius: number | null,
    numIterations: number | null,
    pyramidLevels: number | null,
  ): Promise<InpaintResult>;
  aiAutoColor(nodeId: string, mode: AutoColorMode): Promise<AutoColorResult>;
  aiSegmentAtPoint(
    nodeId: string,
    pointX: number,
    pointY: number,
    isPositive: boolean,
  ): Promise<SegmentAtPointResult>;
  aiSmartSelectAtPoint(
    nodeId: string,
    x: number,
    y: number,
    tolerance: number,
    mode: SmartSelectMode,
    previousMaskBase64: string | null,
  ): Promise<SmartSelectAtPointResult>;

  // -------- Block B — Vector/Layout AI ----------------------
  aiMatchStroke(
    sourceNodeId: string,
    targetNodeIds: string[],
  ): Promise<StrokeMatchSummary>;
  aiExtractGlyph(
    nodeId: string,
    cropX: number,
    cropY: number,
    cropWidth: number,
    cropHeight: number,
    emSize: number,
  ): Promise<ExtractedGlyphResult>;
  aiReformatToDeck(pageId: string): Promise<ReformatDeckResult>;
  aiBriefToOnePager(
    brief: string,
    pageSize: "letter" | "a4" | "square" | null,
  ): Promise<BriefToOnePagerResult>;
  aiGenerateThemedDesign(
    brief: string,
    options: ThemedDesignOptions,
  ): Promise<ThemedDesignApplyResult>;
  aiHarmonizePalette(
    brandKitId: string,
    harmonyType: HarmonyType,
  ): Promise<HarmonyResult>;
  aiSuggestTypePairing(headingFontName: string): Promise<TypePairingResult>;

  // -------- Block C — Export AI + Live Preview --------------
  exportOptimizeSvg(svg: string): Promise<SvgOptimizeReport>;
  exportSmartCompress(
    nodeId: string,
    format: "jpeg" | "webp",
    targetSsim: number | null,
  ): Promise<SmartCompressReport>;
  exportPreview(request: ExportPreviewRequest): Promise<ExportPreviewResponse>;
  importAi(path: string): Promise<AiImportSummary>;

  // -------- Block D — Brand Hub + Plugin Marketplace --------
  aiBrandToBrochure(
    brandKitId: string,
    numPages: number,
  ): Promise<BrochurePlanResult>;
  pluginMarketplaceList(): Promise<PluginListing[]>;
  pluginMarketplaceInstallLocal(path: string): Promise<PluginListing>;
  pluginMarketplaceRemove(id: string): Promise<boolean>;
  exportPdfMulti(
    options: Record<string, unknown>,
    outputPath: string,
  ): Promise<PdfMultiReport>;

  // -------- Block D Task 23 — Preferences -------------------
  preferencesLoad(): Promise<Preferences>;
  preferencesSave(prefs: Preferences): Promise<void>;
}
