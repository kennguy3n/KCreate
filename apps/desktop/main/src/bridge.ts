// Loads the kcreate_bridge native N-API addon. The Cargo build produces
// `libkcreate_bridge.{dylib,so}` (or `kcreate_bridge.dll`) under
// `target/<profile>/`. Node only `require()`s files with a `.node`
// extension, so we use `process.dlopen` directly against the raw cdylib
// path. This avoids any copy step at build time.

import { createRequire } from "node:module";
import * as path from "node:path";
import * as process from "node:process";

// `renderer_frame_info` / `renderer_acquire_frame` return `#[napi(object)]`
// structs directly (not a JSON string), so napi-rs auto-camelCases the Rust
// field names: `frame_id` → `frameId`, `byte_length` → `byteLength`. These
// types must stay camelCase to match the real wire shape — unlike the
// JSON-stringified `*Snake` types below, which preserve serde's snake_case.
type FrameInfoNapi = {
  frameId: number;
  width: number;
  height: number;
  byteLength: number;
};

type AcquiredFrameNapi = {
  frameId: number;
  width: number;
  height: number;
  bytes: Uint8Array;
};

// `ProjectInfoSnake` / `NodeInfoSnake` / `RuntimeStatusSnake` /
// `DocumentStatusSnake` mirror the `#[napi(object)]` structs of the same
// stem in `crates/kcreate_bridge/src/lib.rs`. As with `UndoRedoOutcome`
// below, napi-rs rewrites the Rust snake_case field identifiers to
// camelCase on the JS side (`node_type` → `nodeType`, `total_ram_mb` →
// `totalRamMb`), so these wire shapes are camelCase even though the
// `*Snake` suffix is the house convention for "raw bridge return shape".
type ProjectInfoSnake = {
  id: string;
  name: string;
  path: string;
  createdAt: string;
  modifiedAt: string;
};

type BoundsSnake = {
  x: number;
  y: number;
  width: number;
  height: number;
};

type NodeInfoSnake = {
  id: string;
  nodeType: string;
  parentId: string | null;
  children: string[];
  name: string;
  visible: boolean;
  locked: boolean;
  /// Axis-aligned bounds in document space, mirroring
  /// `kcreate_core::Node::bounds`. Threaded through the napi wire
  /// shape so the renderer can place hotspot rectangles without a
  /// second IPC hop.
  bounds: BoundsSnake;
  /// Monotonic revision counter mirroring `lib.rs::NodeInfo::version`
  /// (`f64`). Required field on the public `NodeInfo` DTO, so it must
  /// appear here too to keep the wire shape in full lockstep (AGENTS.md
  /// Rule 4) — the napi struct returns it on every `document_get_tree`.
  version: number;
};

type RuntimeStatusSnake = {
  deviceTier: string;
  gpuAvailable: boolean;
  gpuName: string | null;
  platform: string;
  totalRamMb: number;
};

type DocumentStatusSnake = {
  nodeCount: number;
  canUndo: boolean;
  canRedo: boolean;
  undoDepth: number;
  redoDepth: number;
};

// Mirror of `crates/kcreate_bridge/src/lib.rs::UndoRedoOutcome`. The
// N-API surface returns camelCase field names (`#[napi(object)]` emits
// the field identifiers literally, and the Rust side is named in
// snake_case → JS via napi's default camelCase rewrite). The `command`
// field is the `Operation::command` string from
// `crates/kcreate_core/src/operation.rs`; the host uses it to gate
// per-operation broadcasts (e.g. `kcreate/color/settings/changed` only
// fires for `color_settings_update`).
type UndoRedoOutcomeSnake = {
  command: string;
  affectedNodes: string[];
};

// Mirror of `crates/kcreate_bridge/src/lib.rs::DiscardedBranchSummary`.
// `anchor_position` is the timeline index where the branch would
// re-attach if restored — UI surfaces it so users can identify which
// undo state a branch was captured from. `op_count` is the size of
// the discarded redo tail (typically 1–N for a single user action,
// or a full group for grouped operations). `discarded_at_iso` is
// RFC 3339 UTC for sort-most-recent-first display. `first_command`
// is the `Operation::command` of the first op in the branch so the
// UI can show a one-line preview (e.g. "Recover: artboard_create").
type DiscardedBranchSummarySnake = {
  anchorPosition: number;
  opCount: number;
  discardedAtIso: string;
  firstCommand: string;
};

// Thumbnail cache + recent-projects bridge (PR #16, Tasks 17-18).
//
// `ThumbnailBytesSnake` mirrors `kcreate_bridge::lib::ThumbnailBytes`.
// `byteSize` is exposed as `number` rather than `BigInt` because every
// byte count we care about (PNG thumbnail at <=2048px on the long
// edge) fits comfortably in JS's safe integer range, and `BigInt`
// would force every renderer call site onto `Number(b.byteSize)`
// conversions for `<img>` sizing math.
type ThumbnailBytesSnake = {
  width: number;
  height: number;
  mime: string;
  byteSize: number;
  bytesBase64: string;
  contentHash: string;
};

// `TemplateInstantiateResultSnake` mirrors
// `kcreate_bridge::lib::TemplateInstantiateResult`. Returned by
// `templateInstantiate` — the artboard the template was poured into
// plus every node id created, so the renderer can select/frame the
// freshly instantiated design.
type TemplateInstantiateResultSnake = {
  artboardId: string;
  nodeIds: string[];
};

// `RecentProjectCoverInfoSnake` — cover-thumbnail metadata only.
// Paired with `thumbnailForCover` / `recentProjectCoverBytes` to
// fetch the actual pixel bytes.
type RecentProjectCoverInfoSnake = {
  width: number;
  height: number;
  mime: string;
  byteSize: number;
  contentHash: string;
};

// `RecentProjectInfoSnake` — one entry on the recent-projects list.
// `path` is the absolute path to the `.kstudio` directory; `projectId`
// is the manifest UUID as a hex string.
type RecentProjectInfoSnake = {
  path: string;
  name: string;
  projectId: string;
  modifiedAt: string;
  lastOpenedAt: string;
  cover: RecentProjectCoverInfoSnake | null;
};

export type {
  FrameInfoNapi,
  AcquiredFrameNapi,
  ProjectInfoSnake,
  NodeInfoSnake,
  BoundsSnake,
  RuntimeStatusSnake,
  DocumentStatusSnake,
  UndoRedoOutcomeSnake,
  DiscardedBranchSummarySnake,
  ThumbnailBytesSnake,
  TemplateInstantiateResultSnake,
  RecentProjectCoverInfoSnake,
  RecentProjectInfoSnake,
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
  rendererRenderCurrent(): number | null;
  rendererSetViewportAndRender(
    panX: number,
    panY: number,
    zoom: number,
  ): number | null;
  rendererGetFrame(): Uint8Array | null;
  rendererFrameInfo(): FrameInfoNapi | null;
  rendererAcquireFrame(): AcquiredFrameNapi | null;

  // Native canvas presentation mode (Phase 1, Block A, Tasks 4–6).
  //
  // `rendererSwitchNative` errors with a "feature not compiled in"
  // message when the bridge was built without the `native_canvas`
  // Cargo feature; the host should treat that as a signal to stay on
  // the offscreen readback path.
  rendererPresentationMode(): string;
  rendererSwitchNative(
    handleBytes: Buffer | Uint8Array,
    width: number,
    height: number,
  ): string;
  rendererSwitchOffscreen(): void;

  // Document / project lifecycle
  projectCreate(name: string, dir: string): ProjectInfoSnake;
  projectOpen(dir: string): ProjectInfoSnake;
  projectSave(): Promise<void>;
  projectClose(): void;
  projectGetInfo(): ProjectInfoSnake | null;
  projectIsUntouched(): boolean;
  /**
   * Phase 11 Block D Task 21 — monotonic version counter that
   * advances on every workspace mutation. Renderer pollers compare
   * two snapshots to skip `documentGetTree` IPC when the document
   * hasn't changed. The reader is a single `AtomicU64` load on the
   * Rust side, so it's safe to call at 60Hz.
   */
  documentVersion(): number;
  documentGetTree(): NodeInfoSnake[];
  documentInspectNode(nodeId: string): string;
  documentCreateNode(
    nodeType: string,
    parentId: string | null,
    propsJson: string,
  ): string;
  documentUpdateNode(nodeId: string, changesJson: string): void;
  /**
   * Read the current `FillStyle` for a node, serialised as a JSON
   * string. Returns `null` when the node id is unknown. Renderer
   * `FillSection` uses this on selection change to populate the
   * fill editor. The shape mirrors `kcreate_core::node::FillStyle`
   * 1:1 via its `#[serde(tag = "kind", rename_all = "snake_case")]`
   * tagged enum — see `FillStyle` in `apps/desktop/shared/scene.ts`.
   *
   * String-typed at the N-API boundary because napi-rs can't mirror
   * a tagged enum without a wire struct per variant; the renderer
   * gets the round-trippable JSON shape directly.
   */
  documentNodeFill(nodeId: string): string | null;
  documentNodeExtraFills(nodeId: string): string | null;
  documentNodeExtraStrokes(nodeId: string): string | null;
  documentDeleteNode(nodeId: string): void;
  /**
   * Phase 6 Tasks 27-28 — layer colour tags. `color` is either a
   * non-empty colour key (canonical lowercase, e.g. `"red"`,
   * `"blue"`, `"yellow"`) to install, or `null` / `undefined` to
   * clear the tag. The bridge canonicalises whitespace + case and
   * records an undoable `layer_color_set` op; returns the node's
   * post-mutation `version` so renderer reads can be invalidated
   * without a full `getTree`. Errors on unknown nodes.
   */
  documentSetLayerColor(nodeId: string, color?: string | null): number;
  documentUndo(): UndoRedoOutcomeSnake | null;
  documentRedo(): UndoRedoOutcomeSnake | null;
  /**
   * Group-aware undo. Consumes the entire contiguous run of ops at
   * the head of the undo stack that share the same `group_id` (a
   * `drag-move-50-times` sequence undoes as one user action). Falls
   * back to single-op undo when the head op carries no `group_id`.
   * The atomicity is enforced inside `kcreate_bridge::document`:
   * peek-the-pending-group, apply every `before_patch`, only then
   * commit the cursor move — so a partial failure leaves the stack
   * untouched and the next call retries the same group. Returns
   * `null` when no project is loaded or the stack is empty.
   */
  documentUndoGroup(): UndoRedoOutcomeSnake | null;
  /**
   * Symmetric with [`documentUndoGroup`] — re-applies the entire
   * contiguous run at the head of the redo stack that share a
   * `group_id`.
   */
  documentRedoGroup(): UndoRedoOutcomeSnake | null;
  /**
   * Newest-first list of redo tails that were dropped because the
   * user pushed a new op after undoing some history. Each entry is a
   * `DiscardedBranchSummarySnake`; the renderer's branch panel uses
   * them to offer "recover branch" affordances. Bounded by the
   * project's `OperationLog::max_branches` (16 by default — see
   * `crates/kcreate_core/src/operation.rs::default_max_branches`).
   * Returns `[]` when no project is loaded or no branches exist.
   */
  documentListDiscardedBranches(): DiscardedBranchSummarySnake[];
  /**
   * Restore the discarded branch at `indexFromBack` (0 = newest, as
   * listed by [`documentListDiscardedBranches`]). Returns `true` on
   * success, `false` if the index is out of range OR the branch's
   * `anchor_position` no longer matches the current undo cursor
   * (i.e. the user did more work after the branch was captured and
   * the branch would attach to the wrong place). On success the
   * restored ops appear at the head of the redo stack and the user
   * can press Redo / Ctrl+Y to re-apply them in order.
   */
  documentRestoreDiscardedBranch(indexFromBack: number): boolean;

  // Phase 6 — Tasks 17-18: lazy thumbnail cache + recent-projects.
  //
  // `thumbnailForCover` / `thumbnailForPage` produce cached PNG bytes
  // for the currently open project. On a cache hit they return
  // immediately without invoking the renderer. `maxDimPx === 0` means
  // "use the default" (320 px on the long edge — see
  // `kcreate_bridge::thumbnails::DEFAULT_THUMBNAIL_MAX_DIM_PX`).
  thumbnailForCover(maxDimPx: number): ThumbnailBytesSnake;
  thumbnailForPage(pageId: string, maxDimPx: number): ThumbnailBytesSnake;
  // Kick off a background worker that warms every page's thumbnail.
  // Returns immediately. Becomes a no-op under low-resource mode.
  thumbnailPrepareBackground(maxDimPx: number): void;
  // Snapshot the persistent recent-projects list (most-recent-first).
  // Entries whose `.kstudio` directory no longer exists are pruned
  // lazily. Each entry carries best-effort cover-thumbnail metadata.
  recentProjectsList(): RecentProjectInfoSnake[];
  // Fetch the cached cover bytes for a project on the recent list
  // *without* opening the project. Returns `null` when no cover is
  // cached for that path.
  recentProjectCoverBytes(projectDir: string): ThumbnailBytesSnake | null;

  documentStatus(): DocumentStatusSnake | null;
  runtimeStatus(): RuntimeStatusSnake;
  lowResourceModeGet(): boolean;
  lowResourceModeSet(enabled: boolean): void;
  resourceLimits(): string;
  // Phase 8 Block E Task 27 — startup-perf profiling.
  // `runtimeStartupTimeline` returns the JSON-serialised
  // `kcreate_perf::Report` shape (snake_case fields). Returns the
  // literal `"{}"` if the timeline has never been initialised, so
  // the renderer doesn't have to special-case "no timeline yet".
  runtimeStartupTimeline(): string;
  // Drop a renderer-side phase mark onto the same global timeline
  // (e.g. `"first_paint"`, `"first_interactive"`) so a single
  // report tells the full startup story.
  runtimeStartupMark(label: string): void;
  // Phase 8 Block E Task 28 — tile-cache stats + clear. The
  // bridge deliberately does NOT expose insert / get to the
  // renderer; those are raster-op internals.
  runtimeTileCacheStats(): string;
  runtimeTileCacheClear(): number;
  llmStart(modelPath: string): number;
  llmStop(): void;
  llmStatus(): string;
  /// Phase C — recommended LLM pack id for the current device.
  /// Empty string when the registry has no recommendation
  /// (expected to be never for any supported device tier).
  llmRecommendedPack(): string;
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
  // Phase 4: vision sidecar.
  visionStart(packId: string): number;
  visionStop(): void;
  visionStatus(): string;
  // Vision inference is wrapped in N-API `AsyncTask` on the Rust
  // side so the Electron main process doesn't freeze while the VLM
  // runs (cold-load + inference can take 5–30 s). Each call resolves
  // a JS `Promise<string>` once the worker thread finishes.
  visionDescribeImage(
    rgba: Buffer,
    width: number,
    height: number,
    userPrompt: string,
  ): Promise<string>;
  visionDescribeNode(nodeId: string, userPrompt: string): Promise<string>;
  visionGenerateAltText(
    rgba: Buffer,
    width: number,
    height: number,
  ): Promise<string>;
  visionGenerateAltTextForNode(nodeId: string): Promise<string>;
  visionAnalyzeDesign(
    rgba: Buffer,
    width: number,
    height: number,
  ): Promise<string>;
  aiExtractBrandFromImage(
    rgba: Buffer,
    width: number,
    height: number,
  ): Promise<string>;
  aiSuggestCrop(
    rgba: Buffer,
    width: number,
    height: number,
    aspectRatio: number,
  ): Promise<string>;
  aiSuggestDesignTokens(
    rgba: Buffer,
    width: number,
    height: number,
  ): Promise<string>;
  aiDescribeStyle(
    rgba: Buffer,
    width: number,
    height: number,
  ): Promise<string>;
  visionRecommendedPack(): string;
  visionMmprojFor(packId: string): string;
  visionListablePacks(): string[];
  // Phase 4: image generation sidecar.
  imageGenStart(packId: string): number;
  imageGenStop(): void;
  imageGenStatus(): string;
  imageGenGenerate(
    prompt: string,
    width: number,
    height: number,
    steps: number,
    seed: number | null,
  ): Promise<string>;
  imageGenAllowed(): boolean;
  imageGenRecommendedPack(): string;
  exportSvg(nodeIds: string[], optionsJson: string): string;
  // Async SVG variant the renderer uses when the document has > 100
  // nodes; the sync variant stays for small docs where worker
  // dispatch overhead would dominate.
  exportSvgAsync(nodeIds: string[], optionsJson: string): Promise<string>;
  exportPng(outputPath: string, optionsJson: string): Promise<number>;
  exportPdf(outputPath: string, optionsJson: string): Promise<number>;
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
  documentImportImageBytes(parentId: string | null, bytes: Buffer): string;
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
  /**
   * Phase B1 — Pen tool. Mirrors
   * `kcreate_bridge::canvas_create_path`. `segmentsJson` is the
   * JSON serialization of `Vec<kcreate_vector::PathSegment>`
   * (see `PathSegmentWire` in `apps/desktop/shared/scene.ts`).
   * The bridge re-deserializes server-side, so the only contract
   * the main process needs to honour is "valid JSON in, uuid out
   * or throw on error".
   */
  canvasCreatePath(
    parentId: string | null,
    segmentsJson: string,
    closed: boolean,
    name: string | null,
  ): string;
  /**
   * Phase B2 (Pathfinder): apply a polygon boolean across the
   * given source vector layers, replacing them with the result(s).
   *
   * `op` is the lowercase wire token (`"union"` / `"subtract"` /
   * `"intersect"` / `"exclude"`); `sourceIds` is a 2+ length list
   * of source node UUIDs. Returns the freshly-inserted result
   * node ids in iteration order.
   *
   * Wire mirror: `apps/desktop/shared/scene.ts::PathBooleanOp`.
   */
  canvasPathBoolean(op: string, sourceIds: string[]): string[];
  /**
   * Phase B3 (Node editor): read a `VectorLayer` node's geometry
   * into a JSON-encoded `PathSnapshot` (see
   * `apps/desktop/shared/scene.ts::PathSnapshot`). The bridge
   * round-trips through JSON for the same reason `createPath` does
   * — keeps adding a new `PathSegment` variant in
   * `kcreate_vector` a pure kcreate_vector change instead of
   * cascading into bridge schema updates.
   *
   * Read-only — records NO operation. Throws on missing node /
   * wrong node type / missing path metadata.
   */
  canvasPathGetSegments(nodeId: string): string;
  /**
   * Phase B3 (Node editor): write new geometry to a `VectorLayer`
   * node. `segmentsJson` is the JSON serialization of
   * `Vec<kcreate_vector::PathSegment>` — same wire shape as
   * `canvasCreatePath`. `closed` becomes `VectorPath.closed`.
   *
   * Records ONE undoable `canvas_path_set_segments` operation per
   * call; the renderer is expected to coalesce per-frame
   * pointermove updates into a single end-of-gesture call so the
   * operation log stays coarse-grained — matches the
   * `canvasMoveNode` discipline.
   */
  canvasPathSetSegments(
    nodeId: string,
    segmentsJson: string,
    closed: boolean,
  ): void;
  canvasCreateText(
    parentId: string | null,
    x: number,
    y: number,
    text: string,
    fontFamily: string,
    fontSize: number,
  ): string;
  canvasMoveNode(nodeId: string, dx: number, dy: number): void;
  /**
   * Atomic batch canvas creation. Takes a JSON-encoded array of
   * `CanvasBatchItem` and returns a JSON-encoded array of the
   * new node ids in the same order. Mirrors
   * `kcreate_bridge::canvas_create_nodes`.
   */
  canvasCreateNodes(itemsJson: string): string;

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
  magicResize(sourceArtboardId: string, targetsJson: string): string;

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
  componentSmartAnimateSnapshot(nodeId: string, targetVariantId: string): string;
  componentDetach(nodeId: string): void;

  // Auto-layout (Phase 1, Block C)
  layoutSetFlex(nodeId: string, layoutJson: string): void;
  layoutSetGrid(nodeId: string, layoutJson: string): void;
  layoutRecompute(nodeId: string): void;
  layoutConvertToFrame(nodeId: string): void;

  // Prototype interactions (Phase 1, Block A)
  interactionAdd(
    nodeId: string,
    trigger: string,
    actionJson: string,
  ): string;
  interactionRemove(nodeId: string, interactionId: string): boolean;
  interactionList(nodeId: string): string;
  /**
   * Batched [`interactionList`] taking a JSON array of node ids and
   * returning a JSON object keyed by node id. Used by the prototype
   * player so a single artboard's hotspots cost one IPC round trip
   * (Devin Review ANALYSIS-0003). The JSON input is preferred over
   * `string[]` because napi-rs can't infer that an array parameter
   * should arrive as JSON.
   */
  interactionListBatch(nodeIdsJson: string): string;

  // Layout Studio (Phase 2, Block B)
  pageSetLayout(pageId: string, layoutJson: string): void;
  pageGetLayout(pageId: string): string;
  masterPageCreate(
    name: string,
    size: string,
    orientation: string,
  ): string;
  masterPageList(): string;
  masterPageApply(contentPageId: string, masterPageId: string): void;
  masterPageDetach(contentPageId: string): void;
  layoutTemplateList(): string;
  layoutTemplateApply(templateId: string): string;
  templateList(category: string | undefined, query: string | undefined): string;
  templateInstallLocal(sourcePath: string): string;
  templateRemove(templateId: string): void;
  // Pour a bundled/marketplace template (resolved by id -> content.json)
  // into the open workspace as a fresh artboard. Returns the new
  // artboard id + every node id created so the renderer can select and
  // frame the instantiated design ("Start from template").
  templateInstantiate(templateId: string): TemplateInstantiateResultSnake;
  // Render (or read the cached) thumbnail PNG for a template id. The
  // PNG is produced by the same Rust export pipeline used for project
  // covers, so gallery cards are real previews of the applied design.
  // Async (napi `AsyncTask`): a cold render runs on a worker thread so
  // it never blocks the Electron main process.
  templateThumbnail(templateId: string): Promise<ThumbnailBytesSnake>;

  // Phase 6 — audit log
  auditRecord(eventJson: string): string;
  auditQuery(queryJson: string): string;
  auditCount(): number;
  auditPurge(cutoffIso: string): number;
  auditPath(): string;

  pageAdd(
    name: string,
    size?: string,
    orientation?: string,
  ): string;
  pageDuplicate(pageId: string): string;
  documentReparentNode(
    nodeId: string,
    newParent: string | undefined,
    index: number,
  ): void;

  // Phase 6 Tasks 25-26 — node clipboard.
  documentClipboardCopy(nodeIds: string[]): string;
  documentClipboardPaste(
    payload: string,
    targetParentId: string | undefined,
    offsetX: number,
    offsetY: number,
  ): string[];

  // Phase 2 — print preflight, icon pack, async batch, AI extras,
  // plugin sandbox, MCP permission persistence.
  preflightRun(requestJson: string): string;
  exportIconPack(requestJson: string): string;
  exportIconPackBuiltInPlatforms(): string;
  exportBatchStart(jobJson: string): string;
  exportBatchStatus(jobId: string): string;
  exportBatchCancel(jobId: string): void;
  exportBatchDismiss(jobId: string): boolean;
  aiUpscale(nodeId: string, scale: number): string;
  aiExtractPalette(nodeId: string, maxColors: number): string;
  aiSmartSelect(
    nodeId: string,
    x: number,
    y: number,
    tolerance: number,
  ): string;
  // Phase 3 Tasks 9-10 — backend-selectable upscale + point-prompt
  // segmentation. Both backends accept the serde representation of
  // `kcreate_ai::{UpscaleBackend, SegmentBackend}` as a plain string.
  // `modelPath` may be `""` to omit; ONNX backends require a path.
  aiUpscaleWithBackend(
    nodeId: string,
    scale: number,
    backend: string,
    modelPath: string,
  ): string;
  aiSegment(
    nodeId: string,
    pointX: number,
    pointY: number,
    tolerance: number,
    edgeThreshold: number,
    backend: string,
    modelPath: string,
  ): string;
  aiDetectTextRegions(nodeId: string, optionsJson: string): string;
  aiInsertTextLayerForRegion(requestJson: string): string;
  aiListModelPacks(): string;
  aiInstallModelPack(packId: string, sourcePath: string): string;
  aiUninstallModelPack(packId: string): void;
  pdfImport(filePath: string): string;
  figmaImport(filePath: string): string;
  sketchImport(filePath: string): string;
  aiScreenshotToLayout(requestJson: string): string;
  aiAltTextForNode(nodeId: string): string;
  aiApplyAltText(nodeId: string, text: string): void;
  aiLayoutSuggestForArtboard(artboardId: string): string;
  pluginList(): string;
  pluginEnable(id: string): void;
  pluginDisable(id: string): void;
  pluginExecute(id: string, function_: string, input: string): string;
  pluginExecuteWithContext(
    id: string,
    function_: string,
    input: string,
  ): string;
  pluginJsList(): string;
  pluginJsMessage(pluginId: string, messageJson: string): string;
  pluginTrustList(): string;
  pluginTrustReload(): void;
  mcpPermissionList(): string;
  mcpPermissionGrant(
    clientId: string,
    toolName: string,
    grant: string,
  ): void;
  mcpPermissionRevoke(clientId: string, toolName: string): void;
  mcpStatus(): string;
  // Phase 2 — color management (ICC / CMYK foundation).
  colorSettingsGet(): string;
  colorSettingsUpdate(settingsJson: string): void;
  colorConvert(fromJson: string, toSpace: string): string;
  // Phase 5 — spot color library (Block D Task 23). The wire shape
  // mirrors `phase2::SpotColorWire` 1:1 (`name`, `displayName`,
  // `fallbackCmyk`, optional `libraryReference`). Every mutation
  // records an undoable operation on the project log.
  colorSpotUpsert(wireJson: string): void;
  colorSpotRemove(name: string): boolean;
  colorSpotList(): string;
  // Phase 3 — Pantone-style JSON catalogue loader. Parses `rawJson`
  // via `kcreate_core::color::SpotColorLibrary::from_json_catalog`
  // and merges it into the project's library. Returns a JSON
  // `SpotCatalogLoadReportWire { added, overwritten, parsed }`.
  // Recorded as a single undoable `spot_color_load_catalog` op.
  colorSpotLoadCatalog(rawJson: string): string;
  // Phase 5 — smart-guides snap engine (Block C Task 13/14). Returns
  // a JSON `SnapResult { dx, dy, guides }` or `null` when no project
  // is loaded. `movingId` is the dragged node so its own edges are
  // excluded from the candidate edge set.
  canvasSnap(
    movingId: string | null,
    candidateX: number,
    candidateY: number,
    candidateW: number,
    candidateH: number,
    threshold: number,
  ): string | null;
  // Phase 5 — raster filters (Block B Task 11). All mutate the
  // RasterLayer node's tile grid in place and record an undoable
  // `Operation` with `ai_generated: false`. `rasterPreviewFilter`
  // returns the post-filter RGBA buffer without committing.
  // Phase 11 Block B: these were sync prior to Phase 11. Each now
  // returns a Promise that resolves once the worker-pool task
  // finishes; the renderer already `await`s the calls.
  rasterApplyLevels(
    nodeId: string,
    black: number,
    white: number,
    gamma: number,
  ): Promise<void>;
  rasterApplyCurves(nodeId: string, pointsJson: string): Promise<void>;
  rasterApplyBlur(nodeId: string, radius: number, kind: string): Promise<void>;
  rasterApplySharpen(
    nodeId: string,
    radius: number,
    amount: number,
    threshold: number,
  ): Promise<void>;
  rasterCrop(
    nodeId: string,
    x: number,
    y: number,
    w: number,
    h: number,
  ): Promise<void>;
  // Phase 11 Block B follow-up — Devin Review ANALYSIS-0003.
  // Rotate / flip / heal now dispatch through `AsyncTask` on the
  // Rust side, so the N-API surface returns `Promise<void>` instead
  // of executing on the main thread.
  rasterRotate(nodeId: string, angleDeg: number): Promise<void>;
  rasterFlip(nodeId: string, direction: string): Promise<void>;
  rasterHeal(
    nodeId: string,
    srcX: number,
    srcY: number,
    dstX: number,
    dstY: number,
    radius: number,
  ): Promise<void>;
  rasterPreviewFilter(nodeId: string, filterJson: string): Buffer;
  // Phase 8 Block B — perspective transform, HSL adjustment, color
  // balance adjustment, and mask-aware filter application. Each
  // mutates the RasterLayer node in place and records an undoable
  // `Operation`. The mask-aware variant accepts a flat row-major
  // boolean array whose length must equal `width * height` of the
  // layer; it composes the filter through a 1-pixel feather kernel
  // at the mask boundary so the seam does not alias.
  rasterPerspective(nodeId: string, cornersJson: string): Promise<void>;
  rasterApplyHsl(
    nodeId: string,
    hue: number,
    saturation: number,
    lightness: number,
  ): Promise<void>;
  rasterApplyColorBalance(
    nodeId: string,
    shadowsJson: string,
    midtonesJson: string,
    highlightsJson: string,
  ): Promise<void>;
  // `mask` is a flat row-major `Buffer` of length
  // `layer_width * layer_height`. Byte `0` means "not selected";
  // any non-zero byte means "selected". Crossing the IPC boundary
  // as bytes (rather than `boolean[]`) avoids per-element
  // structured-clone work on large masks. The Rust N-API decodes
  // this via `napi_get_buffer_info`, which only accepts Node
  // `Buffer` — the preload wraps the renderer-facing `Uint8Array`
  // with `Buffer.from(buffer, byteOffset, byteLength)` (zero-copy
  // view over the same ArrayBuffer) before invoking the IPC
  // channel.
  // Phase 11 Block B follow-up round 3 — Devin Review BUG-0001 (r3).
  // `raster_apply_filter_masked` on the Rust side now returns
  // `AsyncTask<phase11::RasterFilterMaskedTask>` so the masked filter
  // pipeline runs on the libuv worker pool instead of the main
  // thread. The N-API export carries
  // `#[napi(ts_return_type = "Promise<void>")]`, the generated
  // `.d.ts` and `main.ts` IPC handler already `await` the call,
  // and `shared/scene.ts` declares `Promise<void>` — so this
  // hand-written `NativeBridge` declaration must match
  // `Promise<void>` too. AGENTS.md Rule 4 (wire-format lockstep).
  rasterApplyFilterMasked(
    nodeId: string,
    filterJson: string,
    mask: Buffer,
  ): Promise<void>;
  // Phase 5 — vector path operations + non-destructive effects
  // (Block C Tasks 15, 16, 18). All mutate the VectorLayer's
  // stored geometry (simplify / smooth / offset) or its NodeStyle
  // (set stroke profile, push / clear path effect) and record an
  // undoable `Operation`. Bridge enforces argument validation
  // (finite numbers, profile.t in [0,1], dash pattern non-empty).
  vectorSimplify(nodeId: string, tolerance: number): void;
  vectorSmooth(nodeId: string, iterations: number): void;
  vectorOffset(nodeId: string, distance: number): void;
  vectorSetStrokeProfile(nodeId: string, profileJson: string): void;
  vectorApplyPathEffect(nodeId: string, effectJson: string): void;
  vectorClearPathEffects(nodeId: string): void;
  // Phase 5 — text frame linking + wrap (Block D Tasks 19/20).
  textFrameLink(aId: string, bId: string): void;
  textFrameUnlink(nodeId: string): void;
  textFrameSetWrap(nodeId: string, modeJson: string): void;
  // Phase 5 — slices (Block D Task 22). `sliceList` and
  // `sliceExportAll` return JSON arrays; on the export path the
  // returned `SliceResult[]` carries one entry per slice with the
  // file path (or per-slice error message).
  sliceCreate(
    name: string,
    x: number,
    y: number,
    w: number,
    h: number,
    format: string,
    scale: number,
  ): string;
  sliceUpdate(sliceId: string, changesJson: string): void;
  sliceDelete(sliceId: string): boolean;
  sliceList(): string;
  sliceExportAll(outputDir: string): string;
  // Phase 5 — `.kbrand` import/export (Block D Task 21). Asset
  // blobs (fonts / logos) are persisted into the project's asset
  // table when importing; exporting walks the brand kit's
  // referenced asset ids and bundles the underlying bytes.
  brandKitExport(kitId: string, outputPath: string): void;
  brandKitImport(filePath: string): string;
  // Phase 5 — spot color / overprint shortcuts (Block D Task 23).
  // `colorAddSpot` is a spec-shaped alias for `colorSpotUpsert`
  // (no `displayName` / `libraryReference`); `nodeSetOverprint`
  // toggles `NodeStyle::overprint` on any node.
  colorAddSpot(
    name: string,
    c: number,
    m: number,
    y: number,
    k: number,
  ): void;
  nodeSetOverprint(nodeId: string, enabled: boolean): void;
  // Phase 2 — text frame + OpenType (Block B Task 11).
  textFrameGet(nodeId: string): string;
  textFrameUpdate(nodeId: string, optionsJson: string): void;
  textLayoutCompute(nodeId: string): string;
  textOpentypeFeaturesGet(nodeId: string): string;
  textOpentypeFeaturesUpdate(nodeId: string, featuresJson: string): void;
  // Phase A1 — inline text editor + font controls. The wrappers
  // mirror the matching `kcreate_bridge::text_*` entry points.
  // `textListFonts` returns a JSON array of strings; the others
  // mutate the document and record an undoable operation.
  textSetContent(nodeId: string, content: string): void;
  textSetStyle(nodeId: string, styleJson: string): void;
  textReplaceRange(
    nodeId: string,
    start: number,
    end: number,
    replacement: string,
  ): void;
  textContentGet(nodeId: string): string;
  textStyleGet(nodeId: string): string;
  textListFonts(): string;
  // Phase 3 — LAN collaboration session. All entry points are
  // gated by the bridge crate's `collab` feature flag at compile
  // time. When the bridge is built WITHOUT the flag, these exports
  // are absent from the cdylib entirely — a direct call would throw
  // `TypeError: ...is not a function` from V8 (there is no napi
  // "method not implemented" resolver involved; the symbol simply
  // does not exist). To keep non-collab developer builds usable,
  // `loadBridge` installs `applyCollabFallbacks` over the raw
  // exports: read accessors return a benign "no session" snapshot,
  // fire-and-forget/idempotent calls become no-ops, and the handful
  // of genuinely user-initiated collab actions (start / join /
  // KChat install / key + ACL mutations / clipboard share) throw a
  // single clear "collaboration unavailable in this build" error
  // instead of a cryptic `is not a function`. Production builds for
  // packaged apps enable the feature, in which case the fallback
  // layer detects the present exports and returns them untouched.
  sessionStart(
    seedB64: string,
    displayName: string,
    projectId: string,
    advertiseMdns: boolean,
    /**
     * Phase 7 (Task 7): optional KChat community id. When set, the
     * session's mDNS advertisement is tagged with the community
     * so two KCreate peers on the same LAN only auto-discover each
     * other when they belong to the same KChat community. Must
     * match the currently-installed KChat membership's group id;
     * mismatches throw a typed `kcreate_bridge: invalid argument
     * "communityId"` error from the Rust side.
     */
    communityId: string | null,
    /**
     * Phase 7 (Task 21): absolute path to the open project's
     * `.kstudio/` directory. When supplied the bridge loads
     * `<dir>/acl.json` at session start and persists every ACL
     * mutation back to that file so peer-allowlist edits survive
     * process restart. `null` keeps the ACL purely in-memory —
     * appropriate for ad-hoc sessions without a project on disk.
     */
    projectDir: string | null,
  ): string;
  /**
   * Returns the leaving peer's base64url-encoded id when a session was
   * actually running, or `null` when the call was a no-op (no active
   * session). `main.ts` forwards that id as a synthetic `sessionLeft`
   * event on the renderer's session-event channel, so consumers like
   * `useSessionLocks` and `EditorPage`'s presence-broadcast effect can
   * react to local-side lifecycle transitions through the same channel
   * they use for remote peer events.
   */
  sessionLeave(): string | null;
  sessionJoin(
    peerId: string,
    publicKey: string,
    displayName: string,
    socketAddr: string,
    certFingerprintB64: string,
  ): void;
  sessionPeers(): string;
  sessionDrainEvents(): string;
  sessionSendPresence(
    activePage: string | null,
    selectionJson: string,
    cursorJson: string | null,
  ): void;
  sessionInfo(): string;
  // Block 7: operation journal summary. KChat-gated; returns a
  // JSON `SessionJournalSummary` for the running session.
  sessionJournalSummary(): string;
  // Block 8: advisory edit-lock roster + claim/release. All three
  // are KChat-gated; `sessionLocks` returns a JSON
  // `Vec<SessionLockEntry>`. The claim variant returns the
  // wall-clock RFC3339 timestamp of acquisition so the renderer
  // can show "locked N seconds ago" without a second IPC.
  sessionLocks(): string;
  sessionClaimLocks(nodeIdsJson: string): string;
  sessionReleaseLocks(nodeIdsJson: string): void;
  // KChat group authority — multiplayer gate. Until
  // `kchatInstallAuthority` is called with a valid JSON
  // `KChatInstallRequest`, every `session*` call rejects with
  // `NotInKChatGroup`. The KChat client (out of tree) is the
  // only thing authorised to install an authority.
  kchatInstallAuthority(requestJson: string): string;
  kchatClearAuthority(): string;
  kchatMembershipStatus(): string;
  // Pure-crypto helper: derive the local peer's (peerId,
  // peerPublicKey) from the persistent Ed25519 seed. The sign-in
  // panel needs this to pre-fill the membership-binding fields
  // without pulling an Ed25519 implementation into the renderer.
  kchatDeriveLocalIdentity(seedB64: string): string;
  // Dev-only: probe + mint endpoints exposed only when the bridge
  // was built with the `kchat-dev-issuer` feature. The probe
  // (`kchatDevIssuerAvailable`) is always present; the mint
  // function may be undefined on production bridges. The renderer
  // uses the probe to decide whether to surface the "Mint dev
  // membership" affordance in the KChat sign-in panel.
  kchatDevIssuerAvailable?(): boolean;
  kchatDevMintMembership?(requestJson: string): string;
  // Trusted-issuer allowlist for distinguishing real KChat
  // installs (server-minted) from dev-mint installs (in-process
  // issuer). Empty list = "accept any issuer" (backward-compat
  // with the dev flow). Non-empty = installs must match a listed
  // pubkey or the gate stays locked. `kchatSetTrustStorePath`
  // points the bridge at a JSON file on disk; subsequent
  // add/remove calls are persisted via atomic temp-file-rename so
  // changes survive an app restart.
  kchatSetTrustStorePath(path: string): string;
  kchatTrustedIssuers(): string;
  kchatAddTrustedIssuer(issuerJson: string): string;
  kchatRemoveTrustedIssuer(issuerPublicKey: string): string;
  // Phase 7 — KChat backend (HTTPS REST). All entry points are
  // gated on the `kchat-backend` feature flag (which implies
  // `collab`); `kchatBackendAvailable` is always present as a
  // capability probe. `kchatBackendConnect/Disconnect/Status`
  // return JSON `KChatBackendStatus`; `kchatBackendListCommunities`
  // returns JSON `KChatCommunity[]`;
  // `kchatBackendSelectCommunity` returns a JSON
  // `KChatMembershipStatus` (same shape as `kchatInstallAuthority`
  // — replaces the dev-mint flow);
  // `kchatBackendShareToConversation` returns a JSON
  // `KChatPostMessageResult`.
  kchatBackendAvailable(): boolean;
  // `kchatBackendConnect` accepts a JSON-encoded
  // `KChatBackendSignInRequest` (`{ baseUrl, loginId, password, totp? }`).
  kchatBackendConnect?(requestJson: string): string;
  kchatBackendDisconnect?(): string;
  kchatBackendStatus?(): string;
  kchatBackendListCommunities?(): string;
  kchatBackendSelectCommunity?(communityId: string): string;
  kchatBackendGetCommunityMembers?(communityId: string): string;
  kchatBackendListConversations?(communityId: string): string;
  kchatBackendShareToConversation?(conversationId: string, inviteJson: string): string;
  // Phase 7 (Task 10): accept a document-share invite.
  kchatBackendAcceptInvite?(inviteJson: string): string;
  // Phase 7 (Task 8): roster-sync tick.
  kchatBackendSyncCommunityRoster?(communityId: string): string;
  // Phase 8 (Block A, Task 2): publish an exported artifact
  // (PNG / SVG / PDF / WebP / JPEG) to a KChat conversation.
  // `requestJson` is a JSON-encoded
  // `KChatArtifactPublishRequest`; the return is JSON-encoded
  // `KChatArtifactPublishResult`.
  kchatBackendPublishArtifact?(
    conversationId: string,
    requestJson: string,
  ): string;
  // Phase 8 (Block A, Task 2): publish a `.kbrand` brand-kit
  // archive. `requestJson` is a JSON-encoded
  // `KChatBrandKitArtifactRequest`.
  kchatBackendPublishBrandKit?(
    conversationId: string,
    requestJson: string,
  ): string;
  // Phase 8 (Block A, Task 2): list previously-published
  // artifacts for the given conversation. Returns
  // JSON-encoded `KChatPublishedArtifact[]`.
  kchatBackendListArtifacts?(conversationId: string): string;
  // Phase 7 (Task 8): kick a connected peer.
  sessionKickPeer(peerId: string, reason: string): void;
  // Phase 7 (Task 15): ask a connected host to backfill journal
  // entries we are missing relative to our local ResumeVector.
  sessionRequestResume(peerId: string): void;
  // Phase 7 (Task 11): set a peer's permission.
  sessionSetPeerPermission(peerId: string, permission: string): void;
  // Phase 7 (Task 11): local permission snapshot.
  sessionLocalPermission(): string;
  // Phase 7 (Task 21): ACL snapshot / replace. `sessionAclGet`
  // returns the JSON-serialised `ProjectAcl` or `null` when no
  // session is running; `sessionAclSet` takes the same shape.
  sessionAclGet(): string | null;
  sessionAclSet(aclJson: string): void;
  // Phase 7 (Task 19): force a session-key rotation; returns the
  // new epoch. `sessionKeyEpoch` reports the current epoch (or
  // `null` when idle).
  sessionRotateKeys(graceMs: number): number;
  sessionKeyEpoch(): number | null;
  // Phase 7 (Task 23): encrypted clipboard sharing primitives.
  // The local signing key lives on the bridge — there's no seed
  // round-trip in either direction.
  sessionClipboardShare(
    peerId: string,
    plaintext: Buffer,
    previewLabel: string,
  ): string;
  sessionClipboardAccept(offerId: string): Buffer;
  sessionClipboardReject(offerId: string): void;
  sessionPendingClipboardOffers(): string;
  /// Phase 7 (Task 25): queue one local-authored operation into the
  /// outbound throttle buffer. The bridge flushes the buffer in a
  /// single broadcast when the configured interval elapses or the
  /// max-ops cap is hit.
  sessionQueueOperation(opJson: string): void;
  /// Phase 7 (Task 25): drain the pending op batch right now,
  /// returning the number of ops flushed (0 if the queue was
  /// empty). Used at the end of a drag interaction so the final
  /// state lands on the wire without waiting for the timer.
  sessionFlushPendingOperations(): number;
  /// Phase 7 (Task 25): check whether the pending batch's timer has
  /// expired and broadcast it if so. Called every event tick.
  /// Returns the number of ops flushed on this tick (0 when no
  /// flush was due). Cheap when the queue is empty.
  sessionTickOutboundBatch(): number;
  /// Phase 7 (Task 27): set the list of pages the local peer is
  /// currently viewing. Remote presence updates for other pages
  /// are suppressed from the renderer event stream to reduce
  /// overlay churn. Operations still journal across the whole
  /// document. Pass `"[]"` to revert to "interested in everything".
  sessionSetActivePages(pageIdsJson: string): void;
  /// Re-publish the cached scene. Used by the session event tick
  /// to refresh remote-peer cursor overlays.
  documentRequestRender(): void;

  // -------------------------------------------------------------------
  // Phase 8 — design-token binding, constraint-aware resize, text
  // auto-fit, page-numbering tokens, section pages, job presets,
  // brand-kit versioning. See `crates/kcreate_bridge/src/phase8.rs`.
  // -------------------------------------------------------------------
  documentBindToken(
    nodeIdStr: string,
    property: string,
    tokenName: string,
  ): void;
  documentUnbindToken(nodeIdStr: string, property: string): void;
  documentPropagateToken(tokenName: string): number;
  documentNodeTokenBindings(nodeIdStr: string): string;
  documentNodeConstraints(nodeIdStr: string): string;
  documentSetNodeConstraints(nodeIdStr: string, constraintsJson: string): void;
  documentResizeFrame(frameIdStr: string, boundsJson: string): void;
  // -------------------------------------------------------------------
  // Phase 8 Task 26 — project encryption surface.
  // -------------------------------------------------------------------
  projectEncryptionStatus(): string;
  projectPassphraseStrength(passphrase: string): number;
  projectEnableEncryption(passphrase: string): string;
  projectChangePassphrase(
    oldPassphrase: string,
    newPassphrase: string,
  ): void;
  projectExportPlaintextRecovery(
    passphrase: string,
    outputPath: string,
  ): string;
  textSetAutoFit(nodeIdStr: string, enabled: boolean): boolean;
  pageNumberToken(format: string): string;
  pageSetSection(
    pageIdStr: string,
    startNumber: number | null | undefined,
    prefix: string | null | undefined,
  ): void;
  pageResolveContexts(): string;
  exportJobPresets(job: string): string;
  brandKitSaveVersion(
    brandKitIdStr: string,
    description: string,
  ): string;
  brandKitListVersions(brandKitIdStr: string): string;
  brandKitRestoreVersion(versionIdStr: string): string;
  brandKitDiff(beforeIdStr: string, afterIdStr: string): string;

  // -------------------------------------------------------------------
  // Phase 8 (Task 4) — design-review annotations bridge. Each verb is
  // a thin JSON marshal — the actual logic lives in
  // `crates/kcreate_bridge/src/annotation_bridge.rs`. When a collab
  // session is active, mutations also broadcast to peers via
  // `Message::AnnotationBroadcast`.
  // -------------------------------------------------------------------
  annotationCreate(requestJson: string): string;
  annotationReply(requestJson: string): string;
  annotationList(requestJson: string): string;
  annotationResolve(requestJson: string): boolean;
  annotationDelete(idStr: string): boolean;

  // -------------------------------------------------------------------
  // Phase 9 — guides, grid, alignment, AI palette/autofit/trace/iconify/
  // batch-alt-text, PSD/Penpot/EXIF import, SVG preview, history panel,
  // export validation, brief→project, memory watchdog, autosave. See
  // `crates/kcreate_bridge/src/{phase9,perf,autosave}.rs`.
  // -------------------------------------------------------------------
  guideCreate(
    pageIdStr: string,
    orientation: string,
    position: number,
    color: string | null | undefined,
    locked: boolean,
  ): string;
  guideDelete(idStr: string): boolean;
  guideClearPage(pageIdStr: string): number;
  guideList(pageIdStr: string): string;
  guideListAll(): string;

  artboardGridSettings(artboardIdStr: string): string;
  artboardSetGrid(
    artboardIdStr: string,
    enabled: boolean,
    spacing: number,
    subdivisions: number,
    color: string | null | undefined,
  ): string;

  documentAlign(nodeIdsJson: string, alignment: string): string;
  documentDistribute(nodeIdsJson: string, axis: string): string;

  paletteExtractAndApplyBrandKit(
    nodeIdStr: string,
    numColors: number,
    brandKitName: string,
  ): string;

  textAutofitRecompute(nodeIdStr: string): string;

  aiTraceRaster(
    nodeIdStr: string,
    threshold: number,
    simplifyTolerance: number,
  ): string;
  aiIconify(sourceNodeIdStr: string, gridSize: number): string;
  aiBatchAltText(pageIdStr: string): string;

  importPsd(path: string): string;
  importPenpot(path: string): string;
  imageReadExif(bytes: Uint8Array): string;

  exportSvgPreview(
    svgBytes: Uint8Array,
    maxWidth: number,
    maxHeight: number,
    transparent: boolean,
  ): string;

  operationLogFilter(filterJson: string): string;
  exportValidate(requestJson: string): string;
  briefToProject(planJson: string): string;

  memoryWatchdogStart(pollIntervalMs: number): boolean;
  memoryWatchdogStop(): boolean;
  drainMemoryEvents(): string;
  runtimeGpuBackendName(): string;

  autosaveStart(): boolean;
  autosaveStop(): boolean;
  autosaveForceNow(): boolean;
  autosaveStatus(): string;
  autosaveRecoveryAvailable(): string;
  autosaveRecover(): void;
  autosaveDismissRecovery(): void;

  // -------------------------------------------------------------------
  // Phase 10 — Image Studio AI, Vector/Layout AI, Export AI + Live
  // Preview, Brand Hub + Plugin Marketplace, Preferences. See
  // `crates/kcreate_bridge/src/phase10.rs`.
  // -------------------------------------------------------------------

  // Image Studio AI (Block A)
  aiDenoise(
    nodeIdStr: string,
    strength: number,
    searchRadius: number,
    patchRadius: number,
  ): string;
  aiInpaint(
    nodeIdStr: string,
    maskJson: string,
    patchRadius: number | null | undefined,
    numIterations: number | null | undefined,
    pyramidLevels: number | null | undefined,
  ): string;
  aiAutoColor(nodeIdStr: string, mode: string): string;
  aiSegmentAtPoint(
    nodeIdStr: string,
    pointX: number,
    pointY: number,
    isPositive: boolean,
  ): string;
  aiSmartSelectAtPoint(
    nodeIdStr: string,
    x: number,
    y: number,
    tolerance: number,
    mode: string,
    previousMaskBase64: string | null | undefined,
  ): string;

  // Vector/Layout AI (Block B)
  aiMatchStroke(sourceIdStr: string, targetIdsJson: string): string;
  aiExtractGlyph(
    nodeIdStr: string,
    cropX: number,
    cropY: number,
    cropWidth: number,
    cropHeight: number,
    emSize: number,
  ): string;
  aiReformatToDeck(pageIdStr: string): string;
  aiBriefToOnePager(brief: string, pageSize: string | null | undefined): string;
  aiGenerateThemedDesign(brief: string, optionsJson: string): string;
  aiHarmonizePalette(brandKitIdStr: string, harmonyType: string): string;
  aiSuggestTypePairing(headingFontName: string): string;

  // Export AI + Live Preview (Block C)
  exportOptimizeSvg(svg: string): string;
  exportSmartCompress(
    nodeIdStr: string,
    format: string,
    targetSsim: number | null | undefined,
  ): string;
  exportPreview(requestJson: string): string;
  importAi(path: string): string;

  // Brand Hub + Plugin Marketplace (Block D)
  aiBrandToBrochure(brandKitIdStr: string, numPages: number): string;
  pluginMarketplaceList(): string;
  pluginMarketplaceInstallLocal(path: string): string;
  pluginMarketplaceRemove(id: string): boolean;
  exportPdfMulti(optionsJson: string, outputPath: string): string;

  // Preferences (Block D Task 23)
  preferencesLoad(): string;
  preferencesSave(prefsJson: string): void;
}

// Single, actionable message thrown when the renderer triggers a
// genuinely user-initiated collaboration action against a bridge that
// was compiled without the `collab` feature. Anything that fires
// automatically on editor mount degrades silently instead (see
// `collabFallbacks`); only deliberate "start a session / join / mint
// membership / mutate keys+ACL / share clipboard" gestures surface
// this so the user gets a clear reason rather than a cryptic
// `is not a function`.
const COLLAB_UNAVAILABLE_MESSAGE =
  "KCreate collaboration is unavailable in this build: the native bridge " +
  "was compiled without the `collab` Cargo feature. Rebuild kcreate_bridge " +
  "with `--features collab` to enable LAN sessions.";

function collabUnavailable(): never {
  throw new Error(COLLAB_UNAVAILABLE_MESSAGE);
}

// Fallback implementations for the collab-gated exports, used only when
// the bridge was built without `--features collab`. Return shapes mirror
// the RAW native return types (the snake/camel-correct JSON strings the
// real exports emit), so preload's existing `JSON.parse` paths and the
// renderer's typed wrappers behave exactly as they do against an idle
// session. Deliberately omits `kchatSetTrustStorePath`,
// `kchatDevMintMembership`, and the `kchatBackend*` methods: main.ts
// already gates those behind `typeof fn === "function"` checks, and
// synthesising them here would flip that detection and change startup
// behaviour (trust-store init, dev-mint availability, backend sign-in).
function collabFallbacks(): Partial<Bridge> {
  const lockedMembership = JSON.stringify({
    locked: true,
    groupId: null,
    peerId: null,
    expiresAt: null,
  });
  const emptyJournal = JSON.stringify({
    entryCount: 0,
    peerCount: 0,
    byPeer: {},
  });
  return {
    // --- session read accessors → benign "no session" snapshot ---
    sessionInfo: () => "null",
    sessionPeers: () => "[]",
    sessionDrainEvents: () => "[]",
    sessionLocks: () => "[]",
    sessionPendingClipboardOffers: () => "[]",
    sessionJournalSummary: () => emptyJournal,
    sessionLocalPermission: () => "editor",
    sessionAclGet: () => null,
    sessionKeyEpoch: () => null,
    sessionClaimLocks: () => "[]",
    sessionFlushPendingOperations: () => 0,
    sessionTickOutboundBatch: () => 0,
    // --- session fire-and-forget / idempotent → no-op ---
    sessionLeave: () => null,
    sessionSendPresence: () => undefined,
    sessionReleaseLocks: () => undefined,
    sessionQueueOperation: () => undefined,
    sessionSetActivePages: () => undefined,
    sessionClipboardReject: () => undefined,
    // --- KChat read / idempotent → benign locked / empty defaults ---
    kchatMembershipStatus: () => lockedMembership,
    kchatClearAuthority: () => lockedMembership,
    kchatTrustedIssuers: () => "[]",
    kchatDeriveLocalIdentity: () => "null",
    // --- user-initiated collab actions → single clear error ---
    sessionStart: collabUnavailable,
    sessionJoin: collabUnavailable,
    sessionKickPeer: collabUnavailable,
    sessionRequestResume: collabUnavailable,
    sessionSetPeerPermission: collabUnavailable,
    sessionAclSet: collabUnavailable,
    sessionRotateKeys: collabUnavailable,
    sessionClipboardShare: collabUnavailable,
    sessionClipboardAccept: collabUnavailable,
    kchatInstallAuthority: collabUnavailable,
    kchatAddTrustedIssuer: collabUnavailable,
    kchatRemoveTrustedIssuer: collabUnavailable,
  };
}

/**
 * Install graceful fallbacks for the collab-gated exports when the
 * native bridge was built without `--features collab`.
 *
 * The renderer unconditionally polls `session.info()` / `session.peers()`
 * / `session.locks()` on every editor mount (six overlays do so) plus
 * `kchat.membershipStatus()` etc. from the presence/sign-in panels. In a
 * non-collab developer build those exports are absent from the cdylib, so
 * each poll threw `TypeError: ...is not a function` back through the IPC
 * handler — flooding the main-process log on every editor open. This
 * layer fills only the genuinely-missing exports with no-session
 * fallbacks; a real `collab` build is detected and returned untouched
 * with zero overhead.
 *
 * Exported for unit testing without a real `process.dlopen`.
 */
export function applyCollabFallbacks(raw: Partial<Bridge>): Bridge {
  // `sessionInfo` is representative: it exists iff the cdylib was built
  // with `--features collab`, in which case every collab export is
  // present and must not be shadowed.
  if (typeof raw.sessionInfo === "function") {
    return raw as Bridge;
  }
  const target = raw as Record<string, unknown>;
  for (const [name, impl] of Object.entries(collabFallbacks())) {
    // Fill genuinely-absent exports only; never shadow a present one.
    if (typeof target[name] !== "function") {
      target[name] = impl;
    }
  }
  return raw as Bridge;
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

// napi-rs synchronous exports built with `napi/dyn-symbols` return an
// `Error` as a *value* on the failure path instead of throwing it. Each
// IPC handler forwards that return verbatim through `ipcMain.handle`, so
// the renderer's `invoke()` *resolves* with an Error object rather than
// rejecting. Callers that `JSON.parse` the result then crash with
// "Unexpected token 'E', \"Error: …\" is not valid JSON", and callers that
// return the string verbatim hand back a bogus Error-shaped value. (The
// async `AsyncTask` exports are unaffected — their promise rejects
// cleanly.) Wrapping every bridge method so a returned `Error` is thrown
// restores normal promise-rejection semantics across the whole IPC surface
// at a single chokepoint.
export function normalizeBridgeErrors(raw: Bridge): Bridge {
  const wrapped = new Map<PropertyKey, unknown>();
  return new Proxy(raw, {
    get(target, prop, receiver): unknown {
      const value = Reflect.get(target, prop, receiver) as unknown;
      if (typeof value !== "function") {
        return value;
      }
      const cached = wrapped.get(prop);
      if (cached !== undefined) {
        return cached;
      }
      const fn = value as (...args: unknown[]) => unknown;
      const guard = (...args: unknown[]): unknown => {
        const result = fn.apply(target, args);
        if (result instanceof Error) {
          throw result;
        }
        return result;
      };
      wrapped.set(prop, guard);
      return guard;
    },
  }) as Bridge;
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
  // Compose the two capability layers: first install graceful collab
  // fallbacks for a non-collab build (so absent `session_*`/`kchat_*`
  // exports degrade instead of being missing), then normalize the
  // synchronous error-as-value return convention into thrown errors so
  // failed calls reject cleanly through the IPC surface.
  return normalizeBridgeErrors(
    applyCollabFallbacks(moduleStub.exports as Partial<Bridge>),
  );
}
