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

type BoundsSnake = {
  x: number;
  y: number;
  width: number;
  height: number;
};

type NodeInfoSnake = {
  id: string;
  node_type: string;
  parent_id: string | null;
  children: string[];
  name: string;
  visible: boolean;
  locked: boolean;
  /// Axis-aligned bounds in document space, mirroring
  /// `kcreate_core::Node::bounds`. Threaded through the napi wire
  /// shape so the renderer can place hotspot rectangles without a
  /// second IPC hop.
  bounds: BoundsSnake;
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

export type {
  ProjectInfoSnake,
  NodeInfoSnake,
  BoundsSnake,
  RuntimeStatusSnake,
  DocumentStatusSnake,
  UndoRedoOutcomeSnake,
  DiscardedBranchSummarySnake,
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
  projectSave(): void;
  projectClose(): void;
  projectGetInfo(): ProjectInfoSnake | null;
  projectIsUntouched(): boolean;
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
  rasterApplyLevels(
    nodeId: string,
    black: number,
    white: number,
    gamma: number,
  ): void;
  rasterApplyCurves(nodeId: string, pointsJson: string): void;
  rasterApplyBlur(nodeId: string, radius: number, kind: string): void;
  rasterApplySharpen(
    nodeId: string,
    radius: number,
    amount: number,
    threshold: number,
  ): void;
  rasterCrop(
    nodeId: string,
    x: number,
    y: number,
    w: number,
    h: number,
  ): void;
  rasterRotate(nodeId: string, angleDeg: number): void;
  rasterFlip(nodeId: string, direction: string): void;
  rasterHeal(
    nodeId: string,
    srcX: number,
    srcY: number,
    dstX: number,
    dstY: number,
    radius: number,
  ): void;
  rasterPreviewFilter(nodeId: string, filterJson: string): Buffer;
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
  // Phase 3 — LAN collaboration session. All entry points are
  // gated by the bridge crate's `collab` feature flag at compile
  // time; when the bridge is built without the flag, calls into
  // these functions will throw a "Method not implemented" napi
  // error from the runtime resolver. Production builds for
  // packaged apps must enable the feature; debug builds may opt
  // out for faster turn-around on UI-only work.
  sessionStart(
    seedB64: string,
    displayName: string,
    projectId: string,
    advertiseMdns: boolean,
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
  /// Re-publish the cached scene. Used by the session event tick
  /// to refresh remote-peer cursor overlays.
  documentRequestRender(): void;
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
