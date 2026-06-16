// Preload script. Runs in a privileged Node context but is exposed to
// the renderer page via `contextBridge`. The renderer can only call the
// methods we explicitly expose here.

import { contextBridge, ipcRenderer } from "electron";

import type {
  AcquiredFrame,
  AiBridge,
  ApplyThemeReport,
  ArtboardBridge,
  AuditBridge,
  AuditEvent,
  AuditQuery,
  AuditQueryReport,
  ThumbnailBridge,
  ThumbnailBytes,
  RecentProjectsBridge,
  RecentProjectInfo,
  ArtboardInfo,
  ArtboardPreset,
  ResizeTarget,
  MagicResizeContent,
  MagicResizeExportRequest,
  MagicResizeExportReport,
  BrandKit,
  BrandKitBridge,
  CanvasBatchItem,
  CanvasBridge,
  PathSegmentWire,
  FillRuleWire,
  ComponentBridge,
  ComponentInfo,
  SmartAnimateSnapshot,
  CreateNodeProps,
  FlexLayout,
  GridLayout,
  Interaction,
  InteractionAction,
  InteractionBridge,
  InteractionTrigger,
  AssetsBridge,
  AssetCategoryInfo,
  AssetSummary,
  InsertedAsset,
  LayoutBridge,
  LayoutStudioBridge,
  LayoutTemplate,
  TemplateCategory,
  TemplateListReport,
  Theme,
  ThemeBridge,
  TemplateInstantiateReport,
  TemplateImportRequest,
  ImportPickKind,
  TemplateManifest,
  TemplateMarketplaceBridge,
  MasterPageBridge,
  MasterPageInfo,
  PageLayout,
  PageOrientation,
  PageSizeId,
  DesignTokens,
  DesignTokensBridge,
  DiscardedBranchSummary,
  DocumentBridge,
  DocumentStatus,
  UndoRedoOutcome,
  ExportBridge,
  ExportFormat,
  ExportPreset,
  ExportPresetBridge,
  FillStyle,
  StrokeStyleWire,
  FrameInfo,
  InspectCode,
  JpegExportOptions,
  LayerNamingResult,
  LlmBridge,
  LlmJsonResult,
  LlmMessage,
  LlmReply,
  LlmStatus,
  VisionBridge,
  VisionStatus,
  BrandExtraction,
  CropSuggestion,
  DesignTokenSuggestion,
  StyleDescription,
  ImageGenBridge,
  ImageGenStatus,
  GeneratedImage,
  McpBridge,
  NodeInfo,
  PdfExportOptions,
  PngExportOptions,
  PrintReadyExportRequest,
  PrintReadyExportOutcome,
  ProjectInfo,
  RendererBridge,
  RendererInfo,
  ResourceLimits,
  RuntimeBridge,
  RuntimeStatus,
  Scene,
  ScratchCleanupResult,
  StartupTimelineReport,
  TileCacheStats,
  SvgExportOptions,
  UpdateNodeProps,
  WebpExportOptions,
  AiModelBridge,
  AltTextReport,
  UpscaleBackendWire,
  UpscaleWithBackendReportWire,
  SegmentBackendWire,
  SegmentReportWire,
  BatchBridge,
  BatchExportJob,
  BatchStatus,
  ExtractedColor,
  LayoutSuggestion,
  IconPackBridge,
  IconPackPlatform,
  IconPackRequest,
  McpPermission,
  McpPermissionBridge,
  McpPermissionGrant,
  McpStatus,
  ModelInstallReport,
  ModelPack,
  PdfImportBridge,
  PdfImportReport,
  FigmaImportBridge,
  FigmaImportReport,
  SketchImportBridge,
  SketchImportReport,
  JsPanelInfo,
  JsPanelMessage,
  JsPanelMessageOutcome,
  PluginBridge,
  PluginExecuteResult,
  PluginExecuteWithContextResult,
  PluginListEntry,
  TrustedKeyInfo,
  PreflightBridge,
  PreflightIssue,
  PreflightRequest,
  PreflightAutofixRequest,
  PreflightAutofixOutcome,
  ScreenshotElement,
  ScreenshotRequest,
  TextRegion,
  DetectTextRegionsOptions,
  InsertTextLayerForRegionRequest,
  ColorBridge,
  ColorSettings,
  ColorSpaceName,
  ColorValue,
  SpotColorWire,
  SpotCatalogLoadReportWire,
  CanvasSnapBridge,
  SnapResult,
  RasterOpsBridge,
  RasterBlurKind,
  RasterFlipDirection,
  RasterPreviewFilter,
  TextFrameBridge,
  TextFrameOptions,
  TextWrapMode,
  OpenTypeFeatures,
  TextLayoutWire,
  TextBridge,
  TextStyleWire,
  VectorOpsBridge,
  StrokeWidthProfile,
  PathEffectWire,
  SliceBridge,
  SliceWire,
  SliceResultWire,
  SliceUpdateProps,
  SessionBridge,
  SessionCursor,
  SessionEvent,
  SessionJournalSummary,
  SessionLockEntry,
  SessionPeer,
  ProjectAcl,
  PendingClipboardOffer,
  KChatBridge,
  KChatCommunity,
  KChatCommunityMember,
  KChatConversation,
  KChatBackendBridge,
  KChatBackendSignInRequest,
  KChatBackendStatus,
  KChatDevMintRequest,
  KChatInstallRequest,
  KChatLocalIdentity,
  KChatMembershipStatus,
  KChatPostMessageResult,
  KChatShareInvite,
  KChatAcceptedInvite,
  KChatRosterSyncResult,
  KChatArtifactPublishRequest,
  KChatArtifactPublishResult,
  KChatBrandKitArtifactRequest,
  KChatPublishedArtifact,
  CollabPermission,
  SessionStartReport,
  TrustedIssuer,
  ClipboardBridge,
  DeeplinkBridge,
  Phase8Bridge,
  PageNumberFormat,
  JobType,
  ResizeFrameBounds,
  Constraints,
  ProjectEncryptionBridge,
  EncryptionStatus,
  AnnotationBridge,
  Annotation,
  AnnotationListResponse,
  SystemBridge,
  OnboardingBridge,
  OnboardingInstallReport,
  OnboardingProgress,
  Phase9Bridge,
  GuideInfo,
  GridSettingsInfo,
  AlignmentResult,
  Alignment,
  DistributeAxis,
  PaletteApplyResult,
  AutofitRecomputeResult,
  TraceResult,
  IconifyResultInfo,
  BatchAltTextEntry,
  ImportSummary,
  ExifResult,
  SvgPreviewInfo,
  OperationLogFilter,
  OperationInfo,
  ExportValidationRequest,
  ExportValidationReport,
  BriefPlan,
  BriefApplyResult,
  MemoryPressureEvent,
  AutosaveStatus,
  AutosaveMarker,
  Phase10Bridge,
  DenoiseResult,
  InpaintResult,
  AutoColorResult,
  SegmentAtPointResult,
  SmartSelectAtPointResult,
  StrokeMatchSummary,
  ExtractedGlyphResult,
  ReformatDeckResult,
  BriefToOnePagerResult,
  ThemedDesignApplyResult,
  HarmonyResult,
  TypePairingResult,
  SvgOptimizeReport,
  SmartCompressReport,
  ExportPreviewResponse,
  AiImportSummary,
  BrochurePlanResult,
  PluginListing,
  PdfMultiReport,
  Preferences,
} from "../../shared/scene";

// `kcreate/renderer/frameInfo` and `/acquireFrame` forward the bridge's
// `#[napi(object)]` structs straight through IPC. napi-rs auto-camelCases the
// Rust field names (`frame_id` → `frameId`, `byte_length` → `byteLength`), so
// the object arriving here is camelCase — NOT snake_case like the JSON-string
// IPC paths. Reading `frame_id` here yields `undefined` and silently freezes
// the present loop (CanvasHost gates repaints on `frameId` changing).
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

const renderer: RendererBridge = {
  async init(width, height): Promise<RendererInfo> {
    return (await ipcRenderer.invoke(
      "kcreate/renderer/init",
      width,
      height,
    )) as RendererInfo;
  },
  async shutdown(): Promise<void> {
    await ipcRenderer.invoke("kcreate/renderer/shutdown");
  },
  async resize(width, height): Promise<void> {
    await ipcRenderer.invoke("kcreate/renderer/resize", width, height);
  },
  async setViewport(panX, panY, zoom): Promise<void> {
    await ipcRenderer.invoke(
      "kcreate/renderer/setViewport",
      panX,
      panY,
      zoom,
    );
  },
  async invalidate(region): Promise<void> {
    await ipcRenderer.invoke(
      "kcreate/renderer/invalidate",
      region ?? null,
    );
  },
  async render(scene: Scene): Promise<number> {
    return (await ipcRenderer.invoke(
      "kcreate/renderer/render",
      JSON.stringify(scene),
    )) as number;
  },
  async renderCurrent(): Promise<number | null> {
    return (await ipcRenderer.invoke(
      "kcreate/renderer/renderCurrent",
    )) as number | null;
  },
  async setViewportAndRender(panX, panY, zoom): Promise<number | null> {
    return (await ipcRenderer.invoke(
      "kcreate/renderer/setViewportAndRender",
      panX,
      panY,
      zoom,
    )) as number | null;
  },
  async getFrame(): Promise<Uint8Array | null> {
    const buf = (await ipcRenderer.invoke(
      "kcreate/renderer/getFrame",
    )) as Buffer | null;
    if (!buf) return null;
    // Node Buffer is a subclass of Uint8Array; tighten to plain
    // Uint8Array so the renderer doesn't see Node-specific Buffer
    // methods.
    return new Uint8Array(buf.buffer, buf.byteOffset, buf.byteLength);
  },
  async frameInfo(): Promise<FrameInfo | null> {
    const info = (await ipcRenderer.invoke(
      "kcreate/renderer/frameInfo",
    )) as FrameInfoNapi | null;
    if (!info) return null;
    return {
      frameId: info.frameId,
      width: info.width,
      height: info.height,
      byteLength: info.byteLength,
    };
  },
  async acquireFrame(): Promise<AcquiredFrame | null> {
    const frame = (await ipcRenderer.invoke(
      "kcreate/renderer/acquireFrame",
    )) as AcquiredFrameNapi | null;
    if (!frame) return null;
    // Node Buffer is a subclass of Uint8Array; tighten to plain
    // Uint8Array so the renderer doesn't see Node-specific Buffer
    // methods. Use the underlying ArrayBuffer with explicit byteOffset
    // / byteLength so we don't copy.
    const bytes = new Uint8Array(
      frame.bytes.buffer,
      frame.bytes.byteOffset,
      frame.bytes.byteLength,
    );
    return {
      frameId: frame.frameId,
      width: frame.width,
      height: frame.height,
      bytes,
    };
  },
  async presentationMode(): Promise<"offscreen" | "native"> {
    const mode = (await ipcRenderer.invoke(
      "kcreate/renderer/presentationMode",
    )) as string;
    return mode === "native" ? "native" : "offscreen";
  },
  async switchNative(
    width,
    height,
  ): Promise<"appkit" | "win32" | "x11" | "wayland"> {
    // The handle bytes come from the main process — we don't expose
    // `BrowserWindow::getNativeWindowHandle()` to the sandboxed
    // renderer. Two IPC hops in one user gesture is fine; this is a
    // settings-toggle action, not a per-frame path.
    const handle = (await ipcRenderer.invoke(
      "kcreate/canvas/native-handle",
    )) as Buffer | null;
    if (!handle) {
      throw new Error(
        "switchNative: main process has no active BrowserWindow to extract the native handle from",
      );
    }
    const platform = (await ipcRenderer.invoke(
      "kcreate/renderer/switchNative",
      handle,
      width,
      height,
    )) as string;
    // Narrow the string into the typed union the renderer expects.
    if (
      platform === "appkit" ||
      platform === "win32" ||
      platform === "x11" ||
      platform === "wayland"
    ) {
      return platform;
    }
    throw new Error(
      "switchNative: bridge returned unknown platform variant " + platform,
    );
  },
  async switchOffscreen(): Promise<void> {
    await ipcRenderer.invoke("kcreate/renderer/switchOffscreen");
  },
};

// `project_*`, `document_get_tree`, `document_status` and `runtime_status`
// are exported from the bridge as `#[napi(object)]` structs. napi-rs emits
// those field identifiers in camelCase (e.g. `node_type` → `nodeType`,
// `total_ram_mb` → `totalRamMb`), so the returned values already match the
// camelCase `ProjectInfo` / `NodeInfo` / `DocumentStatus` / `RuntimeStatus`
// interfaces in `shared/scene.ts` verbatim. We cast them through directly,
// exactly like `recentProjects.list()` below. (Earlier converters here
// mis-read these as snake_case, which silently dropped every multi-word
// field — the cause of the blank RAM badge and the empty "Type" field.)

const document: DocumentBridge = {
  async createProject(name, dir): Promise<ProjectInfo> {
    return (await ipcRenderer.invoke(
      "kcreate/project/create",
      name,
      dir,
    )) as ProjectInfo;
  },
  async openProject(dir): Promise<ProjectInfo> {
    return (await ipcRenderer.invoke(
      "kcreate/project/open",
      dir,
    )) as ProjectInfo;
  },
  async saveProject(): Promise<void> {
    await ipcRenderer.invoke("kcreate/project/save");
  },
  async closeProject(): Promise<void> {
    await ipcRenderer.invoke("kcreate/project/close");
  },
  async getProjectInfo(): Promise<ProjectInfo | null> {
    return (await ipcRenderer.invoke(
      "kcreate/project/getInfo",
    )) as ProjectInfo | null;
  },
  async isUntouched(): Promise<boolean> {
    return (await ipcRenderer.invoke(
      "kcreate/project/isUntouched",
    )) as boolean;
  },
  async getDocumentTree(): Promise<NodeInfo[]> {
    return (await ipcRenderer.invoke(
      "kcreate/document/getTree",
    )) as NodeInfo[];
  },
  /**
   * Phase 11 Block D Task 21 — read the workspace version counter
   * without acquiring the workspace lock. Pollers use this to skip
   * `getDocumentTree` IPC when nothing has changed.
   */
  async getDocumentVersion(): Promise<number> {
    return (await ipcRenderer.invoke(
      "kcreate/document/version",
    )) as number;
  },
  async inspectNode(nodeId: string): Promise<InspectCode> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/document/inspectNode",
      nodeId,
    )) as string;
    return JSON.parse(raw) as InspectCode;
  },
  async createNode(
    nodeType: string,
    parentId: string | null,
    props: CreateNodeProps,
  ): Promise<string> {
    return (await ipcRenderer.invoke(
      "kcreate/document/createNode",
      nodeType,
      parentId,
      JSON.stringify(props),
    )) as string;
  },
  async updateNode(
    nodeId: string,
    changes: UpdateNodeProps,
  ): Promise<void> {
    await ipcRenderer.invoke(
      "kcreate/document/updateNode",
      nodeId,
      JSON.stringify(changes),
    );
  },
  async nodeFill(nodeId: string): Promise<FillStyle | null> {
    // The Rust bridge returns a JSON-string-encoded `FillStyle`
    // (or `null` for unknown ids) so its tagged-enum shape survives
    // the napi-rs boundary intact. Parse here so renderer callers
    // see the typed shape, not a raw string. A parse failure
    // bubbles up; we don't try to recover because a malformed
    // payload is a wire-format bug, not a recoverable user error.
    const raw = (await ipcRenderer.invoke(
      "kcreate/document/nodeFill",
      nodeId,
    )) as string | null;
    if (raw === null) {
      return null;
    }
    return JSON.parse(raw) as FillStyle;
  },
  async nodeExtraFills(nodeId: string): Promise<FillStyle[] | null> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/document/nodeExtraFills",
      nodeId,
    )) as string | null;
    if (raw === null) {
      return null;
    }
    return JSON.parse(raw) as FillStyle[];
  },
  async nodeExtraStrokes(nodeId: string): Promise<StrokeStyleWire[] | null> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/document/nodeExtraStrokes",
      nodeId,
    )) as string | null;
    if (raw === null) {
      return null;
    }
    return JSON.parse(raw) as StrokeStyleWire[];
  },
  async deleteNode(nodeId: string): Promise<void> {
    await ipcRenderer.invoke("kcreate/document/deleteNode", nodeId);
  },
  async setLayerColor(
    nodeId: string,
    color: string | null,
  ): Promise<number> {
    return (await ipcRenderer.invoke(
      "kcreate/document/setLayerColor",
      nodeId,
      color,
    )) as number;
  },
  async undo(): Promise<UndoRedoOutcome | null> {
    return (await ipcRenderer.invoke(
      "kcreate/document/undo",
    )) as UndoRedoOutcome | null;
  },
  async redo(): Promise<UndoRedoOutcome | null> {
    return (await ipcRenderer.invoke(
      "kcreate/document/redo",
    )) as UndoRedoOutcome | null;
  },
  async undoGroup(): Promise<UndoRedoOutcome | null> {
    return (await ipcRenderer.invoke(
      "kcreate/document/undoGroup",
    )) as UndoRedoOutcome | null;
  },
  async redoGroup(): Promise<UndoRedoOutcome | null> {
    return (await ipcRenderer.invoke(
      "kcreate/document/redoGroup",
    )) as UndoRedoOutcome | null;
  },
  async listDiscardedBranches(): Promise<DiscardedBranchSummary[]> {
    return (await ipcRenderer.invoke(
      "kcreate/document/listDiscardedBranches",
    )) as DiscardedBranchSummary[];
  },
  async restoreDiscardedBranch(indexFromBack: number): Promise<boolean> {
    return (await ipcRenderer.invoke(
      "kcreate/document/restoreDiscardedBranch",
      indexFromBack,
    )) as boolean;
  },
  async status(): Promise<DocumentStatus | null> {
    return (await ipcRenderer.invoke(
      "kcreate/document/status",
    )) as DocumentStatus | null;
  },
};

const runtime: RuntimeBridge = {
  async status(): Promise<RuntimeStatus> {
    return (await ipcRenderer.invoke(
      "kcreate/runtime/status",
    )) as RuntimeStatus;
  },
  async tempDir(): Promise<string> {
    return (await ipcRenderer.invoke("kcreate/runtime/tempDir")) as string;
  },
  async cleanupScratchProjects(): Promise<ScratchCleanupResult> {
    return (await ipcRenderer.invoke(
      "kcreate/runtime/cleanupScratchProjects",
    )) as ScratchCleanupResult;
  },
  async lowResourceModeGet(): Promise<boolean> {
    return (await ipcRenderer.invoke(
      "kcreate/runtime/lowResourceMode/get",
    )) as boolean;
  },
  async lowResourceModeSet(enabled: boolean): Promise<void> {
    await ipcRenderer.invoke(
      "kcreate/runtime/lowResourceMode/set",
      enabled,
    );
  },
  async resourceLimits(): Promise<ResourceLimits> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/runtime/resourceLimits",
    )) as string;
    return resourceLimitsFromSnake(
      JSON.parse(raw) as ResourceLimitsSnake,
    );
  },
  async startupTimeline(): Promise<StartupTimelineReport | null> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/runtime/startupTimeline",
    )) as string;
    // The bridge returns the literal `"{}"` when the timeline
    // has never been initialised. Detect that empty object case
    // and surface it as `null` so the renderer's diagnostics
    // overlay can hide its row cleanly.
    const parsed = JSON.parse(raw) as Partial<StartupTimelineReportSnake>;
    if (
      typeof parsed.name !== "string" ||
      !Array.isArray(parsed.marks) ||
      !Array.isArray(parsed.phases)
    ) {
      return null;
    }
    return startupTimelineFromSnake(parsed as StartupTimelineReportSnake);
  },
  async startupMark(label: string): Promise<void> {
    await ipcRenderer.invoke("kcreate/runtime/startupMark", label);
  },
  async tileCacheStats(): Promise<TileCacheStats> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/runtime/tileCacheStats",
    )) as string;
    return tileCacheStatsFromSnake(JSON.parse(raw) as TileCacheStatsSnake);
  },
  async tileCacheClear(): Promise<number> {
    return (await ipcRenderer.invoke(
      "kcreate/runtime/tileCacheClear",
    )) as number;
  },
  async writeTextFile(target: string, content: string): Promise<number> {
    return (await ipcRenderer.invoke(
      "kcreate/runtime/writeTextFile",
      target,
      content,
    )) as number;
  },
  async chooseExportTarget(
    format: string,
    defaultName: string,
    defaultDir: string | null,
  ): Promise<string | null> {
    return (await ipcRenderer.invoke(
      "kcreate/runtime/chooseExportTarget",
      format,
      defaultName,
      defaultDir,
    )) as string | null;
  },
  async chooseExportDirectory(
    defaultDir: string | null,
  ): Promise<string | null> {
    return (await ipcRenderer.invoke(
      "kcreate/runtime/chooseExportDirectory",
      defaultDir,
    )) as string | null;
  },
};

type ResourceLimitsSnake = {
  device_tier: string;
  platform: string;
  low_resource_mode: boolean;
  effective_undo_depth: number;
  effective_raster_cache_mb: number;
  effective_max_model_mb: number;
  gpu_rendering_allowed: boolean;
  image_generation_allowed: boolean;
  vision_model_max_mb: number;
};

function resourceLimitsFromSnake(s: ResourceLimitsSnake): ResourceLimits {
  return {
    deviceTier: s.device_tier,
    lowResourceMode: s.low_resource_mode,
    effectiveUndoDepth: s.effective_undo_depth,
    effectiveRasterCacheMb: s.effective_raster_cache_mb,
    effectiveMaxModelMb: s.effective_max_model_mb,
    gpuRenderingAllowed: s.gpu_rendering_allowed,
    imageGenerationAllowed: s.image_generation_allowed,
    visionModelMaxMb: s.vision_model_max_mb,
    platform: s.platform,
  };
}

// Phase 8 Block E Task 27 — wire shape produced by
// `kcreate_perf::Report` (snake_case via serde).
type StartupMarkSnake = {
  label: string;
  monotonic_ns: number;
};
type StartupPhaseSnake = {
  label: string;
  from_ns: number;
  to_ns: number;
  duration_ns: number;
};
type StartupTimelineReportSnake = {
  name: string;
  started_at_unix_ms: number;
  total_ns: number;
  marks: StartupMarkSnake[];
  phases: StartupPhaseSnake[];
};

function startupTimelineFromSnake(
  s: StartupTimelineReportSnake,
): StartupTimelineReport {
  return {
    name: s.name,
    startedAtUnixMs: s.started_at_unix_ms,
    totalNs: s.total_ns,
    marks: s.marks.map((m) => ({
      label: m.label,
      monotonicNs: m.monotonic_ns,
    })),
    phases: s.phases.map((p) => ({
      label: p.label,
      fromNs: p.from_ns,
      toNs: p.to_ns,
      durationNs: p.duration_ns,
    })),
  };
}

// Phase 8 Block E Task 28 — wire shape produced by
// `kcreate_bridge::perf::TileCacheStats`.
type TileCacheStatsSnake = {
  bytes: number;
  entries: number;
  budget_bytes: number;
};

function tileCacheStatsFromSnake(s: TileCacheStatsSnake): TileCacheStats {
  return {
    bytes: s.bytes,
    entries: s.entries,
    budgetBytes: s.budget_bytes,
  };
}

const exportApi: ExportBridge = {
  async svg(nodeIds: string[], options: SvgExportOptions): Promise<string> {
    return (await ipcRenderer.invoke(
      "kcreate/export/svg",
      nodeIds,
      JSON.stringify(options),
    )) as string;
  },
  async png(outputPath: string, options: PngExportOptions): Promise<number> {
    return (await ipcRenderer.invoke(
      "kcreate/export/png",
      outputPath,
      JSON.stringify(options),
    )) as number;
  },
  async pdf(outputPath: string, options: PdfExportOptions): Promise<number> {
    return (await ipcRenderer.invoke(
      "kcreate/export/pdf",
      outputPath,
      JSON.stringify(options),
    )) as number;
  },
  async printReady(
    outputPath: string,
    request: PrintReadyExportRequest,
  ): Promise<PrintReadyExportOutcome> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/export/printReady",
      outputPath,
      JSON.stringify(request),
    )) as string;
    return JSON.parse(raw) as PrintReadyExportOutcome;
  },
  async webp(outputPath: string, options: WebpExportOptions): Promise<number> {
    return (await ipcRenderer.invoke(
      "kcreate/export/webp",
      outputPath,
      JSON.stringify(options),
    )) as number;
  },
  async jpeg(outputPath: string, options: JpegExportOptions): Promise<number> {
    return (await ipcRenderer.invoke(
      "kcreate/export/jpeg",
      outputPath,
      JSON.stringify(options),
    )) as number;
  },
};

const canvas: CanvasBridge = {
  async syncScene(): Promise<void> {
    await ipcRenderer.invoke("kcreate/document/syncScene");
  },
  async hitTest(
    screenX: number,
    screenY: number,
    panX: number,
    panY: number,
    zoom: number,
  ): Promise<string | null> {
    return (await ipcRenderer.invoke(
      "kcreate/canvas/hitTest",
      screenX,
      screenY,
      panX,
      panY,
      zoom,
    )) as string | null;
  },
  async setSelection(nodeIds: string[]): Promise<void> {
    await ipcRenderer.invoke("kcreate/document/setSelection", nodeIds);
  },
  async getSelection(): Promise<string[]> {
    return (await ipcRenderer.invoke(
      "kcreate/document/getSelection",
    )) as string[];
  },
  async clearSelection(): Promise<void> {
    await ipcRenderer.invoke("kcreate/document/clearSelection");
  },
  async importImage(
    parentId: string | null,
    filePath: string,
  ): Promise<string> {
    return (await ipcRenderer.invoke(
      "kcreate/document/importImage",
      parentId,
      filePath,
    )) as string;
  },
  async importImageBytes(
    parentId: string | null,
    bytes: Uint8Array,
  ): Promise<string> {
    return (await ipcRenderer.invoke(
      "kcreate/document/importImageBytes",
      parentId,
      Buffer.from(bytes),
    )) as string;
  },
  async createRect(parentId, x, y, w, h): Promise<string> {
    return (await ipcRenderer.invoke(
      "kcreate/canvas/createRect",
      parentId,
      x,
      y,
      w,
      h,
    )) as string;
  },
  async createEllipse(parentId, cx, cy, rx, ry): Promise<string> {
    return (await ipcRenderer.invoke(
      "kcreate/canvas/createEllipse",
      parentId,
      cx,
      cy,
      rx,
      ry,
    )) as string;
  },
  async createLine(parentId, x1, y1, x2, y2): Promise<string> {
    return (await ipcRenderer.invoke(
      "kcreate/canvas/createLine",
      parentId,
      x1,
      y1,
      x2,
      y2,
    )) as string;
  },
  async createPath(parentId, segments, closed, name): Promise<string> {
    // Serialize on the renderer side: keeps the wire payload's
    // JSON shape (the serde-tagged `PathSegment` representation)
    // as the single source of truth — both the IPC channel and
    // the Rust bridge agree it's a `string`, and never have to
    // negotiate the discriminator layout.
    const segmentsJson = JSON.stringify(segments);
    return (await ipcRenderer.invoke(
      "kcreate/canvas/createPath",
      parentId,
      segmentsJson,
      closed,
      name ?? null,
    )) as string;
  },
  async pathBoolean(op, sourceIds): Promise<string[]> {
    // Pathfinder gesture: the renderer hands us the lowercase op
    // token + the source-id selection in iteration order
    // (z-bottom-first). Bridge re-validates length (>=2) and
    // node-type (VectorLayer) so a future caller bypassing the UI
    // gate still fails cleanly. Returns the new result node ids
    // in shape-emission order so the panel can re-select them.
    return (await ipcRenderer.invoke(
      "kcreate/canvas/pathBoolean",
      op,
      sourceIds,
    )) as string[];
  },
  // Phase B3 — Node editor read entry. The bridge returns a
  // JSON-encoded `PathSnapshot` (Rust serde uses snake_case keys
  // `translation_x` / `translation_y` / `fill_rule`); we re-shape
  // to the camelCase TS wire (`translationX` / `translationY` /
  // `fillRule`) here so renderer consumers only deal with the
  // shape declared in `apps/desktop/shared/scene.ts`. Doing the
  // re-shape in preload (rather than in every renderer caller)
  // keeps the wire boundary single-sourced.
  async pathGetSegments(nodeId: string) {
    const json = (await ipcRenderer.invoke(
      "kcreate/canvas/pathGetSegments",
      nodeId,
    )) as string;
    const raw = JSON.parse(json) as {
      segments: PathSegmentWire[];
      closed: boolean;
      fill_rule: FillRuleWire;
      translation_x: number;
      translation_y: number;
    };
    return {
      segments: raw.segments,
      closed: raw.closed,
      fillRule: raw.fill_rule,
      translationX: raw.translation_x,
      translationY: raw.translation_y,
    };
  },
  // Phase B3 — Node editor write entry. Segments are
  // path-local (the bridge keeps the node's transform
  // translation independent of geometry — see the doc-comment
  // on `PathSnapshot`). We serialize on the renderer side for
  // the same reason `createPath` does: keep the wire payload's
  // JSON shape (the serde-tagged `PathSegment` representation)
  // as the single source of truth.
  async pathSetSegments(
    nodeId: string,
    segments: PathSegmentWire[],
    closed: boolean,
  ): Promise<void> {
    const segmentsJson = JSON.stringify(segments);
    await ipcRenderer.invoke(
      "kcreate/canvas/pathSetSegments",
      nodeId,
      segmentsJson,
      closed,
    );
  },
  async createText(
    parentId,
    x,
    y,
    text,
    fontFamily,
    fontSize,
  ): Promise<string> {
    return (await ipcRenderer.invoke(
      "kcreate/canvas/createText",
      parentId,
      x,
      y,
      text,
      fontFamily,
      fontSize,
    )) as string;
  },
  async moveNode(nodeId: string, dx: number, dy: number): Promise<void> {
    await ipcRenderer.invoke("kcreate/canvas/moveNode", nodeId, dx, dy);
  },
  async createNodes(items: CanvasBatchItem[]): Promise<string[]> {
    // Wire format: JSON-encoded array of CanvasBatchItem in,
    // JSON-encoded array of ids out. The bridge owns the locking +
    // op-log + scene-sync; the preload layer is the marshal step.
    // Empty input is short-circuited renderer-side so we don't pay
    // the IPC round-trip for a no-op.
    if (items.length === 0) return [];
    const raw = (await ipcRenderer.invoke(
      "kcreate/canvas/createNodes",
      JSON.stringify(items),
    )) as string;
    return JSON.parse(raw) as string[];
  },
};

const ai: AiBridge = {
  async removeBackground(nodeId: string): Promise<string> {
    return (await ipcRenderer.invoke(
      "kcreate/ai/removeBackground",
      nodeId,
    )) as string;
  },
  async getActionLog(): Promise<string> {
    return (await ipcRenderer.invoke("kcreate/ai/getActionLog")) as string;
  },
  async suggestLayerNames(): Promise<LayerNamingResult> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/ai/suggestLayerNames",
    )) as string;
    return JSON.parse(raw) as LayerNamingResult;
  },
  async extractDesignTokens(): Promise<LlmJsonResult> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/ai/extractDesignTokens",
    )) as string;
    return JSON.parse(raw) as LlmJsonResult;
  },
  async checkAccessibility(): Promise<LlmJsonResult> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/ai/checkAccessibility",
    )) as string;
    return JSON.parse(raw) as LlmJsonResult;
  },
};

const vision: VisionBridge = {
  async start(packId: string): Promise<number> {
    return (await ipcRenderer.invoke(
      "kcreate/vision/start",
      packId,
    )) as number;
  },
  async stop(): Promise<void> {
    await ipcRenderer.invoke("kcreate/vision/stop");
  },
  async status(): Promise<VisionStatus> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/vision/status",
    )) as string;
    return JSON.parse(raw) as VisionStatus;
  },
  async describeImage(
    rgba: Uint8Array,
    width: number,
    height: number,
    userPrompt: string,
  ): Promise<string> {
    return (await ipcRenderer.invoke(
      "kcreate/vision/describeImage",
      Buffer.from(rgba),
      width,
      height,
      userPrompt,
    )) as string;
  },
  async describeNode(nodeId: string, userPrompt: string): Promise<string> {
    return (await ipcRenderer.invoke(
      "kcreate/vision/describeNode",
      nodeId,
      userPrompt,
    )) as string;
  },
  async generateAltText(
    rgba: Uint8Array,
    width: number,
    height: number,
  ): Promise<string> {
    return (await ipcRenderer.invoke(
      "kcreate/vision/generateAltText",
      Buffer.from(rgba),
      width,
      height,
    )) as string;
  },
  async generateAltTextForNode(nodeId: string): Promise<string> {
    return (await ipcRenderer.invoke(
      "kcreate/vision/generateAltTextForNode",
      nodeId,
    )) as string;
  },
  async analyzeDesign(
    rgba: Uint8Array,
    width: number,
    height: number,
  ): Promise<string> {
    return (await ipcRenderer.invoke(
      "kcreate/vision/analyzeDesign",
      Buffer.from(rgba),
      width,
      height,
    )) as string;
  },
  async extractBrand(
    rgba: Uint8Array,
    width: number,
    height: number,
  ): Promise<BrandExtraction> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/ai/extractBrandFromImage",
      Buffer.from(rgba),
      width,
      height,
    )) as string;
    return JSON.parse(raw) as BrandExtraction;
  },
  async suggestCrop(
    rgba: Uint8Array,
    width: number,
    height: number,
    aspectRatio: number,
  ): Promise<CropSuggestion> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/ai/suggestCrop",
      Buffer.from(rgba),
      width,
      height,
      aspectRatio,
    )) as string;
    return JSON.parse(raw) as CropSuggestion;
  },
  async suggestDesignTokens(
    rgba: Uint8Array,
    width: number,
    height: number,
  ): Promise<DesignTokenSuggestion> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/ai/suggestDesignTokens",
      Buffer.from(rgba),
      width,
      height,
    )) as string;
    return JSON.parse(raw) as DesignTokenSuggestion;
  },
  async describeStyle(
    rgba: Uint8Array,
    width: number,
    height: number,
  ): Promise<StyleDescription> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/ai/describeStyle",
      Buffer.from(rgba),
      width,
      height,
    )) as string;
    return JSON.parse(raw) as StyleDescription;
  },
  async recommendedPack(): Promise<string> {
    return (await ipcRenderer.invoke(
      "kcreate/vision/recommendedPack",
    )) as string;
  },
  async mmprojFor(packId: string): Promise<string> {
    return (await ipcRenderer.invoke(
      "kcreate/vision/mmprojFor",
      packId,
    )) as string;
  },
  async listablePacks(): Promise<string[]> {
    return (await ipcRenderer.invoke(
      "kcreate/vision/listablePacks",
    )) as string[];
  },
};

const imageGen: ImageGenBridge = {
  async start(packId: string): Promise<number> {
    return (await ipcRenderer.invoke(
      "kcreate/imageGen/start",
      packId,
    )) as number;
  },
  async stop(): Promise<void> {
    await ipcRenderer.invoke("kcreate/imageGen/stop");
  },
  async status(): Promise<ImageGenStatus> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/imageGen/status",
    )) as string;
    return JSON.parse(raw) as ImageGenStatus;
  },
  async generate(
    prompt: string,
    width: number,
    height: number,
    steps: number,
    seed: number | null,
  ): Promise<GeneratedImage> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/imageGen/generate",
      prompt,
      width,
      height,
      steps,
      seed,
    )) as string;
    return JSON.parse(raw) as GeneratedImage;
  },
  async allowed(): Promise<boolean> {
    return (await ipcRenderer.invoke("kcreate/imageGen/allowed")) as boolean;
  },
  async recommendedPack(): Promise<string> {
    return (await ipcRenderer.invoke(
      "kcreate/imageGen/recommendedPack",
    )) as string;
  },
};

const llm: LlmBridge = {
  async start(modelPath: string): Promise<number> {
    return (await ipcRenderer.invoke(
      "kcreate/llm/start",
      modelPath,
    )) as number;
  },
  async stop(): Promise<void> {
    await ipcRenderer.invoke("kcreate/llm/stop");
  },
  async status(): Promise<LlmStatus> {
    const raw = (await ipcRenderer.invoke("kcreate/llm/status")) as string;
    return JSON.parse(raw) as LlmStatus;
  },
  async chat(
    messages: LlmMessage[],
    maxTokens: number,
    temperature: number,
  ): Promise<LlmReply> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/llm/chat",
      JSON.stringify(messages),
      maxTokens,
      temperature,
    )) as string;
    return JSON.parse(raw) as LlmReply;
  },
  async suggestForSelection(): Promise<LlmReply> {
    const raw = (await ipcRenderer.invoke("kcreate/llm/suggest")) as string;
    return JSON.parse(raw) as LlmReply;
  },
  async recommendedPack(): Promise<string> {
    return (await ipcRenderer.invoke(
      "kcreate/llm/recommendedPack",
    )) as string;
  },
};

const mcp: McpBridge = {
  async start(): Promise<number> {
    return (await ipcRenderer.invoke("kcreate/mcp/start")) as number;
  },
  async stop(): Promise<void> {
    await ipcRenderer.invoke("kcreate/mcp/stop");
  },
  async isRunning(): Promise<boolean> {
    return (await ipcRenderer.invoke("kcreate/mcp/isRunning")) as boolean;
  },
};

const designTokens: DesignTokensBridge = {
  async get(): Promise<DesignTokens> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/designTokens/get",
    )) as string;
    return JSON.parse(raw) as DesignTokens;
  },
  async set(tokens: DesignTokens): Promise<void> {
    await ipcRenderer.invoke(
      "kcreate/designTokens/set",
      JSON.stringify(tokens),
    );
  },
};

const brandKit: BrandKitBridge = {
  async create(name: string): Promise<string> {
    return (await ipcRenderer.invoke(
      "kcreate/brandKit/create",
      name,
    )) as string;
  },
  async update(kit: BrandKit): Promise<void> {
    await ipcRenderer.invoke("kcreate/brandKit/update", JSON.stringify(kit));
  },
  async list(): Promise<BrandKit[]> {
    const raw = (await ipcRenderer.invoke("kcreate/brandKit/list")) as string;
    return JSON.parse(raw) as BrandKit[];
  },
  async delete(kitId: string): Promise<boolean> {
    return (await ipcRenderer.invoke(
      "kcreate/brandKit/delete",
      kitId,
    )) as boolean;
  },
  async export(kitId: string, outputPath: string): Promise<void> {
    await ipcRenderer.invoke("kcreate/brandKit/export", kitId, outputPath);
  },
  async import(filePath: string): Promise<string> {
    return (await ipcRenderer.invoke(
      "kcreate/brandKit/import",
      filePath,
    )) as string;
  },
  async setLogoBytes(kitId: string, bytes: Uint8Array): Promise<void> {
    await ipcRenderer.invoke("kcreate/brandKit/setLogoBytes", kitId, bytes);
  },
  async setFontRole(
    kitId: string,
    role: "heading" | "body",
    family: string,
    embed: boolean,
  ): Promise<void> {
    await ipcRenderer.invoke(
      "kcreate/brandKit/setFontRole",
      kitId,
      role,
      family,
      embed,
    );
  },
  async extractPaletteFromImage(
    kitId: string,
    bytes: Uint8Array,
    numColors: number,
  ): Promise<string[]> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/brandKit/extractPaletteFromImage",
      kitId,
      bytes,
      numColors,
    )) as string;
    return JSON.parse(raw) as string[];
  },
  async insertLogo(
    kitId: string,
    parentId: string | null,
    x: number,
    y: number,
    targetSize: number,
  ): Promise<InsertedAsset> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/brandKit/insertLogo",
      kitId,
      parentId,
      x,
      y,
      targetSize,
    )) as string;
    return JSON.parse(raw) as InsertedAsset;
  },
  async registrySave(kitId: string): Promise<void> {
    await ipcRenderer.invoke("kcreate/brandKit/registrySave", kitId);
  },
  async registryList(): Promise<BrandKit[]> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/brandKit/registryList",
    )) as string;
    return JSON.parse(raw) as BrandKit[];
  },
  async registryLoad(kitId: string): Promise<string> {
    return (await ipcRenderer.invoke(
      "kcreate/brandKit/registryLoad",
      kitId,
    )) as string;
  },
  async registryDelete(kitId: string): Promise<boolean> {
    return (await ipcRenderer.invoke(
      "kcreate/brandKit/registryDelete",
      kitId,
    )) as boolean;
  },
};

const theme: ThemeBridge = {
  async listBuiltins(): Promise<Theme[]> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/theme/listBuiltins",
    )) as string;
    return JSON.parse(raw) as Theme[];
  },
  async apply(themeValue: Theme): Promise<ApplyThemeReport> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/theme/apply",
      JSON.stringify(themeValue),
    )) as string;
    return JSON.parse(raw) as ApplyThemeReport;
  },
  async deriveFromDocument(name: string): Promise<Theme> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/theme/deriveFromDocument",
      name,
    )) as string;
    return JSON.parse(raw) as Theme;
  },
  async fromBrandKit(kit: BrandKit): Promise<Theme> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/theme/fromBrandKit",
      JSON.stringify(kit),
    )) as string;
    return JSON.parse(raw) as Theme;
  },
  async applyToSelection(
    themeValue: Theme,
    roots: string[],
  ): Promise<ApplyThemeReport> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/theme/applyToSelection",
      JSON.stringify(themeValue),
      roots,
    )) as string;
    return JSON.parse(raw) as ApplyThemeReport;
  },
  async deriveFromImage(name: string, bytes: Uint8Array): Promise<Theme> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/theme/deriveFromImage",
      name,
      bytes,
    )) as string;
    return JSON.parse(raw) as Theme;
  },
};

const exportPreset: ExportPresetBridge = {
  async create(
    name: string,
    format: ExportFormat,
    scale: number,
  ): Promise<string> {
    return (await ipcRenderer.invoke(
      "kcreate/exportPreset/create",
      name,
      format,
      scale,
    )) as string;
  },
  async list(): Promise<ExportPreset[]> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/exportPreset/list",
    )) as string;
    return JSON.parse(raw) as ExportPreset[];
  },
  async delete(presetId: string): Promise<boolean> {
    return (await ipcRenderer.invoke(
      "kcreate/exportPreset/delete",
      presetId,
    )) as boolean;
  },
};

type ArtboardInfoSnake = {
  id: string;
  name: string;
  x: number;
  y: number;
  width: number;
  height: number;
  page_id: string;
};

function artboardFromSnake(a: ArtboardInfoSnake): ArtboardInfo {
  return {
    id: a.id,
    name: a.name,
    x: a.x,
    y: a.y,
    width: a.width,
    height: a.height,
    pageId: a.page_id,
  };
}

const artboard: ArtboardBridge = {
  async create(
    pageId: string | null,
    name: string,
    width: number,
    height: number,
  ): Promise<string> {
    return (await ipcRenderer.invoke(
      "kcreate/artboard/create",
      pageId ?? "",
      name,
      width,
      height,
    )) as string;
  },
  async list(): Promise<ArtboardInfo[]> {
    const raw = (await ipcRenderer.invoke("kcreate/artboard/list")) as string;
    const parsed = JSON.parse(raw) as ArtboardInfoSnake[];
    return parsed.map(artboardFromSnake);
  },
  async duplicate(artboardId: string): Promise<string> {
    return (await ipcRenderer.invoke(
      "kcreate/artboard/duplicate",
      artboardId,
    )) as string;
  },
  async resize(
    artboardId: string,
    width: number,
    height: number,
  ): Promise<void> {
    await ipcRenderer.invoke(
      "kcreate/artboard/resize",
      artboardId,
      width,
      height,
    );
  },
  async presets(): Promise<ArtboardPreset[]> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/artboard/presets",
    )) as string;
    return JSON.parse(raw) as ArtboardPreset[];
  },
  async magicResize(
    sourceArtboardId: string,
    targets: ResizeTarget[],
    content?: MagicResizeContent,
  ): Promise<string[]> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/artboard/magic-resize",
      sourceArtboardId,
      JSON.stringify(targets),
      content === undefined ? "" : JSON.stringify(content),
    )) as string;
    return JSON.parse(raw) as string[];
  },
  async magicResizeExportPng(
    sourceArtboardId: string,
    targets: ResizeTarget[],
    request: MagicResizeExportRequest,
  ): Promise<MagicResizeExportReport> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/artboard/magic-resize-export-png",
      sourceArtboardId,
      JSON.stringify(targets),
      JSON.stringify(request),
    )) as string;
    return JSON.parse(raw) as MagicResizeExportReport;
  },
};

const component: ComponentBridge = {
  async createFromSelection(nodeIds: string[], name: string): Promise<string> {
    return (await ipcRenderer.invoke(
      "kcreate/component/createFromSelection",
      nodeIds,
      name,
    )) as string;
  },
  async list(): Promise<ComponentInfo[]> {
    const raw = (await ipcRenderer.invoke("kcreate/component/list")) as string;
    return JSON.parse(raw) as ComponentInfo[];
  },
  async instantiate(
    componentId: string,
    parentId: string | null,
    x: number,
    y: number,
  ): Promise<string> {
    return (await ipcRenderer.invoke(
      "kcreate/component/instantiate",
      componentId,
      parentId ?? "",
      x,
      y,
    )) as string;
  },
  async addVariant(componentId: string, name: string): Promise<string> {
    return (await ipcRenderer.invoke(
      "kcreate/component/addVariant",
      componentId,
      name,
    )) as string;
  },
  async switchVariant(nodeId: string, variantId: string): Promise<void> {
    await ipcRenderer.invoke(
      "kcreate/component/switchVariant",
      nodeId,
      variantId,
    );
  },
  async smartAnimateSnapshot(
    nodeId: string,
    targetVariantId: string,
  ): Promise<SmartAnimateSnapshot> {
    const json = (await ipcRenderer.invoke(
      "kcreate/component/smartAnimateSnapshot",
      nodeId,
      targetVariantId,
    )) as string;
    return JSON.parse(json) as SmartAnimateSnapshot;
  },
  async detach(nodeId: string): Promise<void> {
    await ipcRenderer.invoke("kcreate/component/detach", nodeId);
  },
};

const layout: LayoutBridge = {
  async setFlex(nodeId: string, config: FlexLayout): Promise<void> {
    await ipcRenderer.invoke(
      "kcreate/layout/setFlex",
      nodeId,
      JSON.stringify(config),
    );
  },
  async setGrid(nodeId: string, config: GridLayout): Promise<void> {
    await ipcRenderer.invoke(
      "kcreate/layout/setGrid",
      nodeId,
      JSON.stringify(config),
    );
  },
  async recompute(nodeId: string): Promise<void> {
    await ipcRenderer.invoke("kcreate/layout/recompute", nodeId);
  },
  async convertToFrame(nodeId: string): Promise<void> {
    await ipcRenderer.invoke("kcreate/layout/convertToFrame", nodeId);
  },
};

const interaction: InteractionBridge = {
  async add(
    nodeId: string,
    trigger: InteractionTrigger,
    action: InteractionAction,
  ): Promise<string> {
    // Phase 11: `InteractionTrigger` is now a union of simple
    // discriminator strings (`"click"`, …) AND a data-carrying
    // `{ kind: "after_delay", ms }` object. The bridge accepts both
    // forms, but the IPC channel passes a string — JSON-encode the
    // object form before crossing the boundary.
    const triggerWire =
      typeof trigger === "string" ? trigger : JSON.stringify(trigger);
    return (await ipcRenderer.invoke(
      "kcreate/interaction/add",
      nodeId,
      triggerWire,
      JSON.stringify(action),
    )) as string;
  },
  async remove(nodeId: string, interactionId: string): Promise<boolean> {
    return (await ipcRenderer.invoke(
      "kcreate/interaction/remove",
      nodeId,
      interactionId,
    )) as boolean;
  },
  async list(nodeId: string): Promise<Interaction[]> {
    const json = (await ipcRenderer.invoke(
      "kcreate/interaction/list",
      nodeId,
    )) as string;
    return JSON.parse(json) as Interaction[];
  },
  async listBatch(
    nodeIds: string[],
  ): Promise<Record<string, Interaction[]>> {
    const json = (await ipcRenderer.invoke(
      "kcreate/interaction/list-batch",
      nodeIds,
    )) as string;
    return JSON.parse(json) as Record<string, Interaction[]>;
  },
};

const masterPage: MasterPageBridge = {
  async create(
    name: string,
    size: PageSizeId,
    orientation: PageOrientation,
  ): Promise<string> {
    return (await ipcRenderer.invoke(
      "kcreate/masterPage/create",
      name,
      size,
      orientation,
    )) as string;
  },
  async list(): Promise<MasterPageInfo[]> {
    const json = (await ipcRenderer.invoke(
      "kcreate/masterPage/list",
    )) as string;
    return JSON.parse(json) as MasterPageInfo[];
  },
  async apply(
    contentPageId: string,
    masterPageId: string,
  ): Promise<void> {
    await ipcRenderer.invoke(
      "kcreate/masterPage/apply",
      contentPageId,
      masterPageId,
    );
  },
  async detach(contentPageId: string): Promise<void> {
    await ipcRenderer.invoke("kcreate/masterPage/detach", contentPageId);
  },
};

const layoutStudio: LayoutStudioBridge = {
  async setPageLayout(pageId: string, layoutValue: PageLayout): Promise<void> {
    await ipcRenderer.invoke(
      "kcreate/page/setLayout",
      pageId,
      JSON.stringify(layoutValue),
    );
  },
  async getPageLayout(pageId: string): Promise<PageLayout | null> {
    const json = (await ipcRenderer.invoke(
      "kcreate/page/getLayout",
      pageId,
    )) as string;
    return json === "" ? null : (JSON.parse(json) as PageLayout);
  },
  async listTemplates(): Promise<LayoutTemplate[]> {
    const json = (await ipcRenderer.invoke(
      "kcreate/layoutTemplate/list",
    )) as string;
    return JSON.parse(json) as LayoutTemplate[];
  },
  async applyTemplate(templateId: string): Promise<string[]> {
    const json = (await ipcRenderer.invoke(
      "kcreate/layoutTemplate/apply",
      templateId,
    )) as string;
    return JSON.parse(json) as string[];
  },
  async addPage(
    name: string,
    size?: PageSizeId,
    orientation?: PageOrientation,
  ): Promise<string> {
    return (await ipcRenderer.invoke(
      "kcreate/page/add",
      name,
      size,
      orientation,
    )) as string;
  },
  async duplicatePage(pageId: string): Promise<string> {
    return (await ipcRenderer.invoke(
      "kcreate/page/duplicate",
      pageId,
    )) as string;
  },
  async reparentNode(
    nodeId: string,
    newParent: string | null,
    index: number,
  ): Promise<void> {
    await ipcRenderer.invoke(
      "kcreate/document/reparent",
      nodeId,
      newParent,
      index,
    );
  },
};

// ---------------------------------------------------------------------------
// G6 — Elements / asset library. The bridge returns JSON strings which
// we parse into the shared wire types.
// ---------------------------------------------------------------------------

const assets: AssetsBridge = {
  async categories(): Promise<AssetCategoryInfo[]> {
    const json = (await ipcRenderer.invoke(
      "kcreate/assets/categories",
    )) as string;
    return JSON.parse(json) as AssetCategoryInfo[];
  },
  async list(category?: string | null): Promise<AssetSummary[]> {
    const json = (await ipcRenderer.invoke(
      "kcreate/assets/list",
      category ?? undefined,
    )) as string;
    return JSON.parse(json) as AssetSummary[];
  },
  async search(
    query: string,
    category?: string | null,
  ): Promise<AssetSummary[]> {
    const json = (await ipcRenderer.invoke(
      "kcreate/assets/search",
      query,
      category ?? undefined,
    )) as string;
    return JSON.parse(json) as AssetSummary[];
  },
  async insert(
    assetId: string,
    parentId: string | null,
    x: number,
    y: number,
    targetSize: number,
  ): Promise<InsertedAsset> {
    const json = (await ipcRenderer.invoke(
      "kcreate/assets/insert",
      assetId,
      parentId,
      x,
      y,
      targetSize,
    )) as string;
    return JSON.parse(json) as InsertedAsset;
  },
};

// ---------------------------------------------------------------------------
// Phase 3 — local template marketplace (Tasks 11-12).
// ---------------------------------------------------------------------------

const templateMarketplace: TemplateMarketplaceBridge = {
  async list(
    category?: TemplateCategory,
    query?: string,
  ): Promise<TemplateListReport> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/template/list",
      category ?? null,
      query ?? null,
    )) as string;
    return JSON.parse(raw) as TemplateListReport;
  },
  async installLocal(sourcePath: string): Promise<TemplateManifest> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/template/installLocal",
      sourcePath,
    )) as string;
    return JSON.parse(raw) as TemplateManifest;
  },
  async remove(templateId: string): Promise<void> {
    await ipcRenderer.invoke("kcreate/template/remove", templateId);
  },
  async instantiate(
    templateId: string,
  ): Promise<TemplateInstantiateReport> {
    return (await ipcRenderer.invoke(
      "kcreate/template/instantiate",
      templateId,
    )) as TemplateInstantiateReport;
  },
  async thumbnail(templateId: string): Promise<ThumbnailBytes> {
    return (await ipcRenderer.invoke(
      "kcreate/template/thumbnail",
      templateId,
    )) as ThumbnailBytes;
  },
  async pickImport(kind: ImportPickKind): Promise<string | null> {
    return (await ipcRenderer.invoke(
      "kcreate/template/pickImport",
      kind,
    )) as string | null;
  },
  async import(request: TemplateImportRequest): Promise<TemplateManifest> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/template/import",
      JSON.stringify(request),
    )) as string;
    return JSON.parse(raw) as TemplateManifest;
  },
};

// ---------------------------------------------------------------------------
// Phase 6 — Audit log (Tasks 13–14)
// ---------------------------------------------------------------------------

const audit: AuditBridge = {
  async record(event: AuditEvent): Promise<string> {
    return (await ipcRenderer.invoke(
      "kcreate/audit/record",
      JSON.stringify(event),
    )) as string;
  },
  async query(filter: AuditQuery): Promise<AuditQueryReport> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/audit/query",
      JSON.stringify(filter),
    )) as string;
    return JSON.parse(raw) as AuditQueryReport;
  },
  async count(): Promise<number> {
    return (await ipcRenderer.invoke("kcreate/audit/count")) as number;
  },
  async purge(cutoffIso: string): Promise<number> {
    return (await ipcRenderer.invoke(
      "kcreate/audit/purge",
      cutoffIso,
    )) as number;
  },
  async path(): Promise<string> {
    return (await ipcRenderer.invoke("kcreate/audit/path")) as string;
  },
};

// ---------------------------------------------------------------------------
// Phase 6 — Tasks 17-18: lazy thumbnail cache + recent-projects.
// ---------------------------------------------------------------------------

const thumbnail: ThumbnailBridge = {
  async forCover(maxDimPx: number): Promise<ThumbnailBytes> {
    return (await ipcRenderer.invoke(
      "kcreate/thumbnail/forCover",
      maxDimPx,
    )) as ThumbnailBytes;
  },
  async forPage(pageId: string, maxDimPx: number): Promise<ThumbnailBytes> {
    return (await ipcRenderer.invoke(
      "kcreate/thumbnail/forPage",
      pageId,
      maxDimPx,
    )) as ThumbnailBytes;
  },
  async prepareBackground(maxDimPx: number): Promise<void> {
    await ipcRenderer.invoke("kcreate/thumbnail/prepareBackground", maxDimPx);
  },
};

const recentProjects: RecentProjectsBridge = {
  async list(): Promise<RecentProjectInfo[]> {
    return (await ipcRenderer.invoke(
      "kcreate/recent/list",
    )) as RecentProjectInfo[];
  },
  async coverBytes(projectDir: string): Promise<ThumbnailBytes | null> {
    return (await ipcRenderer.invoke(
      "kcreate/recent/coverBytes",
      projectDir,
    )) as ThumbnailBytes | null;
  },
};

// ---------------------------------------------------------------------------
// Phase 2 — preflight, icon pack, batch async, AI extras, plugins, MCP perms.
// ---------------------------------------------------------------------------

const preflight: PreflightBridge = {
  async run(request: PreflightRequest): Promise<PreflightIssue[]> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/preflight/run",
      JSON.stringify(request),
    )) as string;
    return JSON.parse(raw) as PreflightIssue[];
  },
  async autofix(
    request: PreflightAutofixRequest,
  ): Promise<PreflightAutofixOutcome> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/preflight/autofix",
      JSON.stringify(request),
    )) as string;
    return JSON.parse(raw) as PreflightAutofixOutcome;
  },
};

const iconPack: IconPackBridge = {
  async builtInPlatforms(): Promise<IconPackPlatform[]> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/export/iconPack/builtInPlatforms",
    )) as string;
    return JSON.parse(raw) as IconPackPlatform[];
  },
  async generate(request: IconPackRequest): Promise<string[]> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/export/iconPack",
      JSON.stringify(request),
    )) as string;
    return JSON.parse(raw) as string[];
  },
};

const batch: BatchBridge = {
  async start(job: BatchExportJob): Promise<string> {
    return (await ipcRenderer.invoke(
      "kcreate/export/batch/start",
      JSON.stringify(job),
    )) as string;
  },
  async status(jobId: string): Promise<BatchStatus> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/export/batch/status",
      jobId,
    )) as string;
    return JSON.parse(raw) as BatchStatus;
  },
  async cancel(jobId: string): Promise<void> {
    await ipcRenderer.invoke("kcreate/export/batch/cancel", jobId);
  },
  async dismiss(jobId: string): Promise<boolean> {
    return (await ipcRenderer.invoke(
      "kcreate/export/batch/dismiss",
      jobId,
    )) as boolean;
  },
};

const aiModel: AiModelBridge = {
  async upscale(nodeId: string, scale: number): Promise<string> {
    return (await ipcRenderer.invoke(
      "kcreate/ai/upscale",
      nodeId,
      scale,
    )) as string;
  },
  async extractPalette(
    nodeId: string,
    maxColors: number,
  ): Promise<ExtractedColor[]> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/ai/extractPalette",
      nodeId,
      maxColors,
    )) as string;
    return JSON.parse(raw) as ExtractedColor[];
  },
  async smartSelect(
    nodeId: string,
    x: number,
    y: number,
    tolerance: number,
  ): Promise<string> {
    return (await ipcRenderer.invoke(
      "kcreate/ai/smartSelect",
      nodeId,
      x,
      y,
      tolerance,
    )) as string;
  },
  async upscaleWithBackend(
    nodeId: string,
    scale: number,
    backend: UpscaleBackendWire,
    modelPath: string,
  ): Promise<UpscaleWithBackendReportWire> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/ai/upscaleWithBackend",
      nodeId,
      scale,
      backend,
      modelPath,
    )) as string;
    return JSON.parse(raw) as UpscaleWithBackendReportWire;
  },
  async segment(
    nodeId: string,
    pointX: number,
    pointY: number,
    tolerance: number,
    edgeThreshold: number,
    backend: SegmentBackendWire,
    modelPath: string,
  ): Promise<SegmentReportWire> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/ai/segment",
      nodeId,
      pointX,
      pointY,
      tolerance,
      edgeThreshold,
      backend,
      modelPath,
    )) as string;
    return JSON.parse(raw) as SegmentReportWire;
  },
  async detectTextRegions(
    nodeId: string,
    options?: DetectTextRegionsOptions | null,
  ): Promise<TextRegion[]> {
    const optsJson = options === undefined || options === null
      ? "null"
      : JSON.stringify(options);
    const raw = (await ipcRenderer.invoke(
      "kcreate/ai/detectTextRegions",
      nodeId,
      optsJson,
    )) as string;
    return JSON.parse(raw) as TextRegion[];
  },
  async insertTextLayerForRegion(
    request: InsertTextLayerForRegionRequest,
  ): Promise<string> {
    return (await ipcRenderer.invoke(
      "kcreate/ai/insertTextLayerForRegion",
      JSON.stringify(request),
    )) as string;
  },
  async listModelPacks(): Promise<ModelPack[]> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/ai/listModelPacks",
    )) as string;
    return JSON.parse(raw) as ModelPack[];
  },
  async pickModelFile(): Promise<string | null> {
    return (await ipcRenderer.invoke(
      "kcreate/ai/pickModelFile",
    )) as string | null;
  },
  async installModelPack(
    packId: string,
    sourcePath: string,
  ): Promise<ModelInstallReport> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/ai/installModelPack",
      packId,
      sourcePath,
    )) as string;
    return JSON.parse(raw) as ModelInstallReport;
  },
  async uninstallModelPack(packId: string): Promise<void> {
    await ipcRenderer.invoke("kcreate/ai/uninstallModelPack", packId);
  },
  async screenshotToLayout(
    request: ScreenshotRequest,
  ): Promise<ScreenshotElement[]> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/ai/screenshotToLayout",
      JSON.stringify(request),
    )) as string;
    return JSON.parse(raw) as ScreenshotElement[];
  },
  async altTextForNode(nodeId: string): Promise<AltTextReport> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/ai/altTextForNode",
      nodeId,
    )) as string;
    return JSON.parse(raw) as AltTextReport;
  },
  async applyAltText(nodeId: string, text: string): Promise<void> {
    await ipcRenderer.invoke("kcreate/ai/applyAltText", nodeId, text);
  },
  async layoutSuggestForArtboard(
    artboardId: string,
  ): Promise<LayoutSuggestion[]> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/ai/layoutSuggestForArtboard",
      artboardId,
    )) as string;
    return JSON.parse(raw) as LayoutSuggestion[];
  },
};

const pdfImport: PdfImportBridge = {
  async pickFile(): Promise<string | null> {
    return (await ipcRenderer.invoke(
      "kcreate/pdf/pickFile",
    )) as string | null;
  },
  async importPdf(filePath: string): Promise<PdfImportReport> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/pdf/import",
      filePath,
    )) as string;
    return JSON.parse(raw) as PdfImportReport;
  },
};

const figmaImport: FigmaImportBridge = {
  async pickFile(): Promise<string | null> {
    return (await ipcRenderer.invoke(
      "kcreate/figma/pickFile",
    )) as string | null;
  },
  async importFigma(filePath: string): Promise<FigmaImportReport> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/figma/import",
      filePath,
    )) as string;
    return JSON.parse(raw) as FigmaImportReport;
  },
};

const sketchImport: SketchImportBridge = {
  async pickFile(): Promise<string | null> {
    return (await ipcRenderer.invoke(
      "kcreate/sketch/pickFile",
    )) as string | null;
  },
  async importSketch(filePath: string): Promise<SketchImportReport> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/sketch/import",
      filePath,
    )) as string;
    return JSON.parse(raw) as SketchImportReport;
  },
};

const plugin: PluginBridge = {
  async list(): Promise<PluginListEntry[]> {
    const raw = (await ipcRenderer.invoke("kcreate/plugin/list")) as string;
    return JSON.parse(raw) as PluginListEntry[];
  },
  async enable(id: string): Promise<void> {
    await ipcRenderer.invoke("kcreate/plugin/enable", id);
  },
  async disable(id: string): Promise<void> {
    await ipcRenderer.invoke("kcreate/plugin/disable", id);
  },
  async execute(
    id: string,
    fn: string,
    input: string,
  ): Promise<PluginExecuteResult> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/plugin/execute",
      id,
      fn,
      input,
    )) as string;
    return JSON.parse(raw) as PluginExecuteResult;
  },
  async executeWithContext(
    id: string,
    fn: string,
    input: string,
  ): Promise<PluginExecuteWithContextResult> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/plugin/executeWithContext",
      id,
      fn,
      input,
    )) as string;
    return JSON.parse(raw) as PluginExecuteWithContextResult;
  },
  async jsList(): Promise<JsPanelInfo[]> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/plugin/js/list",
    )) as string;
    return JSON.parse(raw) as JsPanelInfo[];
  },
  async jsMessage(
    pluginId: string,
    message: JsPanelMessage,
  ): Promise<JsPanelMessageOutcome> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/plugin/js/message",
      pluginId,
      JSON.stringify(message),
    )) as string;
    return JSON.parse(raw) as JsPanelMessageOutcome;
  },
  async jsOpen(
    pluginId: string,
    bounds: { x: number; y: number; width: number; height: number },
  ): Promise<void> {
    await ipcRenderer.invoke("kcreate/plugin/js/open", pluginId, bounds);
  },
  async jsSetBounds(
    pluginId: string,
    bounds: { x: number; y: number; width: number; height: number },
  ): Promise<void> {
    await ipcRenderer.invoke("kcreate/plugin/js/setBounds", pluginId, bounds);
  },
  async jsClose(pluginId: string): Promise<void> {
    await ipcRenderer.invoke("kcreate/plugin/js/close", pluginId);
  },
  async trustList(): Promise<TrustedKeyInfo[]> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/plugin/trust/list",
    )) as string;
    return JSON.parse(raw) as TrustedKeyInfo[];
  },
  async trustReload(): Promise<void> {
    await ipcRenderer.invoke("kcreate/plugin/trust/reload");
  },
};

const mcpPermission: McpPermissionBridge = {
  async list(): Promise<McpPermission[]> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/mcp/permission/list",
    )) as string;
    return JSON.parse(raw) as McpPermission[];
  },
  async grant(
    clientId: string,
    toolName: string,
    grant: McpPermissionGrant,
  ): Promise<void> {
    await ipcRenderer.invoke(
      "kcreate/mcp/permission/grant",
      clientId,
      toolName,
      grant,
    );
  },
  async revoke(clientId: string, toolName: string): Promise<void> {
    await ipcRenderer.invoke(
      "kcreate/mcp/permission/revoke",
      clientId,
      toolName,
    );
  },
  async status(): Promise<McpStatus> {
    const raw = (await ipcRenderer.invoke("kcreate/mcp/status")) as string;
    return JSON.parse(raw) as McpStatus;
  },
};

// ---------------------------------------------------------------------------
// Phase 2 — Color management (CMYK / ICC foundation).
// ---------------------------------------------------------------------------

const color: ColorBridge = {
  async getSettings(): Promise<ColorSettings> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/color/settings/get",
    )) as string;
    return JSON.parse(raw) as ColorSettings;
  },
  async updateSettings(settings: ColorSettings): Promise<void> {
    await ipcRenderer.invoke(
      "kcreate/color/settings/update",
      JSON.stringify(settings),
    );
  },
  async convert(
    value: ColorValue,
    toSpace: ColorSpaceName,
  ): Promise<ColorValue> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/color/convert",
      JSON.stringify(value),
      toSpace,
    )) as string;
    return JSON.parse(raw) as ColorValue;
  },
  /// Subscribe to the push channel that fires whenever
  /// `color_settings_update`, `documentUndo`, or `documentRedo`
  /// mutates `ws.project.color_settings`. The event is intentionally
  /// payload-free — subscribers should call `getSettings()` to read
  /// the new shape. Returns an unsubscribe function so callers can
  /// detach in their effect cleanup.
  onSettingsChanged(callback: () => void): () => void {
    const channel = "kcreate/color/settings/changed";
    const listener = (): void => {
      callback();
    };
    ipcRenderer.on(channel, listener);
    return () => {
      ipcRenderer.removeListener(channel, listener);
    };
  },
  async upsertSpot(spot: SpotColorWire): Promise<void> {
    await ipcRenderer.invoke(
      "kcreate/color/spot/upsert",
      JSON.stringify(spot),
    );
  },
  async removeSpot(name: string): Promise<boolean> {
    return (await ipcRenderer.invoke(
      "kcreate/color/spot/remove",
      name,
    )) as boolean;
  },
  async listSpots(): Promise<SpotColorWire[]> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/color/spot/list",
    )) as string;
    return JSON.parse(raw) as SpotColorWire[];
  },
  async loadCatalog(rawJson: string): Promise<SpotCatalogLoadReportWire> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/color/spot/load-catalog",
      rawJson,
    )) as string;
    return JSON.parse(raw) as SpotCatalogLoadReportWire;
  },
  async addSpot(
    name: string,
    c: number,
    m: number,
    y: number,
    k: number,
  ): Promise<void> {
    await ipcRenderer.invoke("kcreate/color/spot/add", name, c, m, y, k);
  },
  async setNodeOverprint(nodeId: string, enabled: boolean): Promise<void> {
    await ipcRenderer.invoke(
      "kcreate/node/overprint/set",
      nodeId,
      enabled,
    );
  },
};

// ---------------------------------------------------------------------------
// Phase 5 — smart-guides snap engine (Block C Task 13/14). The
// `CanvasHost` calls this on every drag-move event; the implementation
// is intentionally a one-shot RPC (no long-lived stream) so that pan /
// zoom changes between drags don't accumulate stale state on the
// renderer side.
// ---------------------------------------------------------------------------

const canvasSnap: CanvasSnapBridge = {
  async query(
    movingId: string | null,
    candidateX: number,
    candidateY: number,
    candidateW: number,
    candidateH: number,
    threshold: number,
  ): Promise<SnapResult | null> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/canvas/snap",
      movingId,
      candidateX,
      candidateY,
      candidateW,
      candidateH,
      threshold,
    )) as string | null;
    if (raw === null) {
      return null;
    }
    return JSON.parse(raw) as SnapResult;
  },
};

// ---------------------------------------------------------------------------
// Phase 5 — raster filters (Block B Task 11). `applyXxx` / `crop` /
// `rotate` / `flip` / `heal` commit through the bridge (each records
// an undoable Operation); `previewFilter` is non-destructive and
// returns the post-filter RGBA bytes for live preview.
// ---------------------------------------------------------------------------

const rasterOps: RasterOpsBridge = {
  async applyLevels(
    nodeId: string,
    black: number,
    white: number,
    gamma: number,
  ): Promise<void> {
    await ipcRenderer.invoke(
      "kcreate/raster/apply/levels",
      nodeId,
      black,
      white,
      gamma,
    );
  },
  async applyCurves(
    nodeId: string,
    points: [number, number][],
  ): Promise<void> {
    await ipcRenderer.invoke(
      "kcreate/raster/apply/curves",
      nodeId,
      JSON.stringify(points),
    );
  },
  async applyBlur(
    nodeId: string,
    radius: number,
    kind: RasterBlurKind,
  ): Promise<void> {
    await ipcRenderer.invoke(
      "kcreate/raster/apply/blur",
      nodeId,
      radius,
      kind,
    );
  },
  async applySharpen(
    nodeId: string,
    radius: number,
    amount: number,
    threshold: number,
  ): Promise<void> {
    await ipcRenderer.invoke(
      "kcreate/raster/apply/sharpen",
      nodeId,
      radius,
      amount,
      threshold,
    );
  },
  async crop(
    nodeId: string,
    x: number,
    y: number,
    w: number,
    h: number,
  ): Promise<void> {
    await ipcRenderer.invoke("kcreate/raster/crop", nodeId, x, y, w, h);
  },
  async rotate(nodeId: string, angleDeg: number): Promise<void> {
    await ipcRenderer.invoke("kcreate/raster/rotate", nodeId, angleDeg);
  },
  async flip(
    nodeId: string,
    direction: RasterFlipDirection,
  ): Promise<void> {
    await ipcRenderer.invoke("kcreate/raster/flip", nodeId, direction);
  },
  async heal(
    nodeId: string,
    srcX: number,
    srcY: number,
    dstX: number,
    dstY: number,
    radius: number,
  ): Promise<void> {
    await ipcRenderer.invoke(
      "kcreate/raster/heal",
      nodeId,
      srcX,
      srcY,
      dstX,
      dstY,
      radius,
    );
  },
  async previewFilter(
    nodeId: string,
    filter: RasterPreviewFilter,
  ): Promise<Uint8Array> {
    const buf = (await ipcRenderer.invoke(
      "kcreate/raster/preview",
      nodeId,
      JSON.stringify(filter),
    )) as Buffer | Uint8Array;
    // Electron transfers Buffer over IPC. Normalise to Uint8Array
    // so renderer-side `canvas.getContext('2d').putImageData` can
    // wrap it in `ImageData` without a copy.
    return buf instanceof Uint8Array ? buf : new Uint8Array(buf);
  },
  async perspective(
    nodeId: string,
    corners: [
      [number, number],
      [number, number],
      [number, number],
      [number, number],
    ],
  ): Promise<void> {
    await ipcRenderer.invoke(
      "kcreate/raster/perspective",
      nodeId,
      JSON.stringify(corners),
    );
  },
  async applyHsl(
    nodeId: string,
    hue: number,
    saturation: number,
    lightness: number,
  ): Promise<void> {
    await ipcRenderer.invoke(
      "kcreate/raster/apply/hsl",
      nodeId,
      hue,
      saturation,
      lightness,
    );
  },
  async applyColorBalance(
    nodeId: string,
    shadows: [number, number, number],
    midtones: [number, number, number],
    highlights: [number, number, number],
  ): Promise<void> {
    await ipcRenderer.invoke(
      "kcreate/raster/apply/color_balance",
      nodeId,
      JSON.stringify(shadows),
      JSON.stringify(midtones),
      JSON.stringify(highlights),
    );
  },
  async applyFilterMasked(
    nodeId: string,
    filter: RasterPreviewFilter,
    mask: Uint8Array,
  ): Promise<void> {
    // The Rust N-API surface (`raster_apply_filter_masked` in
    // `crates/kcreate_bridge/src/lib.rs`) declares the mask as
    // `napi::bindgen_prelude::Buffer`, which is decoded via
    // `napi_get_buffer_info` — that NAPI primitive only accepts
    // Node.js `Buffer` instances, not plain `Uint8Array`. Wrap with
    // `Buffer.from(mask.buffer, mask.byteOffset, mask.byteLength)` so
    // the resulting Buffer shares the existing ArrayBuffer (no copy)
    // and crosses the IPC boundary as a Buffer the bridge can decode.
    // This mirrors the convention already used by every other binary
    // IPC parameter in this file (e.g. `documentImportImageBytes`,
    // `visionDescribeImage`, `clipboard-share`).
    await ipcRenderer.invoke(
      "kcreate/raster/apply/filter_masked",
      nodeId,
      JSON.stringify(filter),
      Buffer.from(mask.buffer, mask.byteOffset, mask.byteLength),
    );
  },
};

// ---------------------------------------------------------------------------
// Phase 2 — Text frame + OpenType (Block B Task 11).
//
// The bridge always speaks JSON over IPC for these calls — the renderer
// must `JSON.parse` the responses and `JSON.stringify` outgoing options
// before invoking. Doing the (de)serialisation here keeps callers
// uniform with the rest of the `window.kcreate.*` surface.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Phase 3 — LAN collaboration session.
//
// All four IPC channels live behind `kcreate/session/...`. The main
// process handles them in `apps/desktop/main/src/main.ts`. The
// renderer subscribes to a single push channel
// `kcreate/session/event` that fans out discovered / peerJoined /
// peerLeft / presenceUpdated events; the main process polls the
// bridge's bounded queue on a fixed tick and forwards each entry.
// ---------------------------------------------------------------------------

const session: SessionBridge = {
  async start(
    seedB64: string,
    displayName: string,
    projectId: string,
    advertiseMdns: boolean,
    // Phase 7 (Task 7): optional community gate. `null` (or omitted)
    // = no community scoping; matches pre-Phase-7 behaviour.
    communityId: string | null = null,
    // Phase 7 (Task 21): optional `.kstudio/` directory path so the
    // bridge persists ACL mutations to `<dir>/acl.json`. `null` (or
    // omitted) keeps the ACL in memory only.
    projectDir: string | null = null,
  ): Promise<SessionStartReport> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/session/start",
      seedB64,
      displayName,
      projectId,
      advertiseMdns,
      communityId,
      projectDir,
    )) as string;
    return JSON.parse(raw) as SessionStartReport;
  },
  async leave(): Promise<void> {
    await ipcRenderer.invoke("kcreate/session/leave");
  },
  async join(
    peerId: string,
    publicKey: string,
    displayName: string,
    socketAddr: string,
    certFingerprintB64: string,
  ): Promise<void> {
    await ipcRenderer.invoke(
      "kcreate/session/join",
      peerId,
      publicKey,
      displayName,
      socketAddr,
      certFingerprintB64,
    );
  },
  async peers(): Promise<SessionPeer[]> {
    const raw = (await ipcRenderer.invoke("kcreate/session/peers")) as string;
    return JSON.parse(raw) as SessionPeer[];
  },
  async info(): Promise<SessionStartReport | null> {
    const raw = (await ipcRenderer.invoke("kcreate/session/info")) as string;
    return JSON.parse(raw) as SessionStartReport | null;
  },
  async journalSummary(): Promise<SessionJournalSummary> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/session/journalSummary",
    )) as string;
    return JSON.parse(raw) as SessionJournalSummary;
  },
  async locks(): Promise<SessionLockEntry[]> {
    const raw = (await ipcRenderer.invoke("kcreate/session/locks")) as string;
    return JSON.parse(raw) as SessionLockEntry[];
  },
  async claimLocks(nodeIds: string[]): Promise<string> {
    return (await ipcRenderer.invoke(
      "kcreate/session/claimLocks",
      JSON.stringify(nodeIds),
    )) as string;
  },
  async releaseLocks(nodeIds: string[]): Promise<void> {
    await ipcRenderer.invoke(
      "kcreate/session/releaseLocks",
      JSON.stringify(nodeIds),
    );
  },
  async sendPresence(
    activePage: string | null,
    selection: string[],
    cursor: SessionCursor | null,
  ): Promise<void> {
    await ipcRenderer.invoke(
      "kcreate/session/sendPresence",
      activePage,
      JSON.stringify(selection),
      cursor === null ? null : JSON.stringify(cursor),
    );
  },
  onEvent(callback: (event: SessionEvent) => void): () => void {
    const channel = "kcreate/session/event";
    const listener = (_evt: unknown, payload: string): void => {
      try {
        const ev = JSON.parse(payload) as SessionEvent;
        callback(ev);
      } catch {
        // Malformed event payload — swallow rather than crash the
        // renderer. The main process logs the underlying bridge
        // error before emitting.
      }
    };
    ipcRenderer.on(channel, listener);
    return () => {
      ipcRenderer.removeListener(channel, listener);
    };
  },
  async kickPeer(peerId: string, reason: string): Promise<void> {
    await ipcRenderer.invoke(
      "kcreate/session/kick-peer",
      peerId,
      reason,
    );
  },
  async requestResume(peerId: string): Promise<void> {
    await ipcRenderer.invoke("kcreate/session/request-resume", peerId);
  },
  async setPeerPermission(
    peerId: string,
    permission: CollabPermission,
  ): Promise<void> {
    await ipcRenderer.invoke(
      "kcreate/session/set-peer-permission",
      peerId,
      permission,
    );
  },
  async localPermission(): Promise<CollabPermission> {
    return (await ipcRenderer.invoke(
      "kcreate/session/local-permission",
    )) as CollabPermission;
  },
  async acl(): Promise<ProjectAcl | null> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/session/acl-get",
    )) as string | null;
    return raw === null ? null : (JSON.parse(raw) as ProjectAcl);
  },
  async setAcl(acl: ProjectAcl): Promise<void> {
    await ipcRenderer.invoke(
      "kcreate/session/acl-set",
      JSON.stringify(acl),
    );
  },
  async rotateKeys(graceMs: number): Promise<number> {
    return (await ipcRenderer.invoke(
      "kcreate/session/rotate-keys",
      graceMs,
    )) as number;
  },
  async keyEpoch(): Promise<number | null> {
    return (await ipcRenderer.invoke(
      "kcreate/session/key-epoch",
    )) as number | null;
  },
  async shareClipboard(
    peerId: string,
    plaintext: Uint8Array,
    previewLabel: string,
  ): Promise<string> {
    // The bridge keeps the local signing key on its side after
    // `session_start`, so the renderer never has to handle the
    // seed for clipboard sharing.
    return (await ipcRenderer.invoke(
      "kcreate/session/clipboard-share",
      peerId,
      Buffer.from(plaintext),
      previewLabel,
    )) as string;
  },
  async acceptClipboardOffer(offerId: string): Promise<Uint8Array> {
    const buf = (await ipcRenderer.invoke(
      "kcreate/session/clipboard-accept",
      offerId,
    )) as Buffer;
    return new Uint8Array(buf);
  },
  async rejectClipboardOffer(offerId: string): Promise<void> {
    await ipcRenderer.invoke(
      "kcreate/session/clipboard-reject",
      offerId,
    );
  },
  async pendingClipboardOffers(): Promise<PendingClipboardOffer[]> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/session/pending-clipboard-offers",
    )) as string;
    return JSON.parse(raw) as PendingClipboardOffer[];
  },
  async queueOperation(operation: unknown): Promise<void> {
    await ipcRenderer.invoke(
      "kcreate/session/queue-operation",
      JSON.stringify(operation),
    );
  },
  async flushPendingOperations(): Promise<number> {
    return (await ipcRenderer.invoke(
      "kcreate/session/flush-pending-operations",
    )) as number;
  },
  async tickOutboundBatch(): Promise<number> {
    return (await ipcRenderer.invoke(
      "kcreate/session/tick-outbound-batch",
    )) as number;
  },
  async setActivePages(pageIds: string[]): Promise<void> {
    await ipcRenderer.invoke(
      "kcreate/session/set-active-pages",
      JSON.stringify(pageIds),
    );
  },
};

// ---------------------------------------------------------------------------
// KChat group authority. See `KChatBridge` in shared/scene.ts for the
// contract — multiplayer is locked until the future KChat client
// invokes `install()` with a signed membership attestation.
// ---------------------------------------------------------------------------

const kchat: KChatBridge = {
  async install(
    request: KChatInstallRequest,
  ): Promise<KChatMembershipStatus> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/kchat/install",
      JSON.stringify(request),
    )) as string;
    return JSON.parse(raw) as KChatMembershipStatus;
  },
  async clear(): Promise<KChatMembershipStatus> {
    const raw = (await ipcRenderer.invoke("kcreate/kchat/clear")) as string;
    return JSON.parse(raw) as KChatMembershipStatus;
  },
  async status(): Promise<KChatMembershipStatus> {
    const raw = (await ipcRenderer.invoke("kcreate/kchat/status")) as string;
    return JSON.parse(raw) as KChatMembershipStatus;
  },
  async deriveLocalIdentity(seedB64: string): Promise<KChatLocalIdentity> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/kchat/derive-local-identity",
      seedB64,
    )) as string;
    return JSON.parse(raw) as KChatLocalIdentity;
  },
  async devIssuerAvailable(): Promise<boolean> {
    // The handler always returns boolean; in production builds it
    // returns false because the bridge function is absent.
    return (await ipcRenderer.invoke(
      "kcreate/kchat/dev-issuer-available",
    )) as boolean;
  },
  async devMintMembership(
    request: KChatDevMintRequest,
  ): Promise<KChatInstallRequest> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/kchat/dev-mint-membership",
      JSON.stringify(request),
    )) as string;
    return JSON.parse(raw) as KChatInstallRequest;
  },
  async setTrustStorePath(p: string): Promise<TrustedIssuer[]> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/kchat/set-trust-store-path",
      p,
    )) as string;
    return JSON.parse(raw) as TrustedIssuer[];
  },
  async trustedIssuers(): Promise<TrustedIssuer[]> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/kchat/trusted-issuers",
    )) as string;
    return JSON.parse(raw) as TrustedIssuer[];
  },
  async addTrustedIssuer(issuer: TrustedIssuer): Promise<TrustedIssuer[]> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/kchat/add-trusted-issuer",
      JSON.stringify(issuer),
    )) as string;
    return JSON.parse(raw) as TrustedIssuer[];
  },
  async removeTrustedIssuer(
    issuerPublicKey: string,
  ): Promise<TrustedIssuer[]> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/kchat/remove-trusted-issuer",
      issuerPublicKey,
    )) as string;
    return JSON.parse(raw) as TrustedIssuer[];
  },
};

// ---------------------------------------------------------------------------
// Phase 7 — KChat backend (HTTPS REST) bridge. Mirrors the Rust
// surface in `kcreate_bridge::kchat_backend`. Every wire call is a
// thin JSON-string passthrough; the IPC handler in `main.ts`
// returns a typed error if the bridge wasn't built with
// `kchat-backend`.
// ---------------------------------------------------------------------------

const kchatBackend: KChatBackendBridge = {
  async available(): Promise<boolean> {
    return (await ipcRenderer.invoke(
      "kcreate/kchat-backend/available",
    )) as boolean;
  },
  async connect(
    request: KChatBackendSignInRequest,
  ): Promise<KChatBackendStatus> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/kchat-backend/connect",
      JSON.stringify(request),
    )) as string;
    return JSON.parse(raw) as KChatBackendStatus;
  },
  async disconnect(): Promise<KChatBackendStatus> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/kchat-backend/disconnect",
    )) as string;
    return JSON.parse(raw) as KChatBackendStatus;
  },
  async status(): Promise<KChatBackendStatus> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/kchat-backend/status",
    )) as string;
    return JSON.parse(raw) as KChatBackendStatus;
  },
  async listCommunities(): Promise<KChatCommunity[]> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/kchat-backend/list-communities",
    )) as string;
    return JSON.parse(raw) as KChatCommunity[];
  },
  async selectCommunity(communityId: string): Promise<KChatMembershipStatus> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/kchat-backend/select-community",
      communityId,
    )) as string;
    return JSON.parse(raw) as KChatMembershipStatus;
  },
  async getCommunityMembers(
    communityId: string,
  ): Promise<KChatCommunityMember[]> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/kchat-backend/get-community-members",
      communityId,
    )) as string;
    return JSON.parse(raw) as KChatCommunityMember[];
  },
  async listConversations(communityId: string): Promise<KChatConversation[]> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/kchat-backend/list-conversations",
      communityId,
    )) as string;
    return JSON.parse(raw) as KChatConversation[];
  },
  async shareToConversation(
    conversationId: string,
    invite: KChatShareInvite,
  ): Promise<KChatPostMessageResult> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/kchat-backend/share-to-conversation",
      conversationId,
      JSON.stringify(invite),
    )) as string;
    return JSON.parse(raw) as KChatPostMessageResult;
  },
  async acceptInvite(inviteJson: string): Promise<KChatAcceptedInvite> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/kchat-backend/accept-invite",
      inviteJson,
    )) as string;
    return JSON.parse(raw) as KChatAcceptedInvite;
  },
  async syncCommunityRoster(
    communityId: string,
  ): Promise<KChatRosterSyncResult> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/kchat-backend/sync-community-roster",
      communityId,
    )) as string;
    return JSON.parse(raw) as KChatRosterSyncResult;
  },
  async publishArtifact(
    conversationId: string,
    request: KChatArtifactPublishRequest,
  ): Promise<KChatArtifactPublishResult> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/kchat-backend/publish-artifact",
      conversationId,
      JSON.stringify(request),
    )) as string;
    return JSON.parse(raw) as KChatArtifactPublishResult;
  },
  async publishBrandKit(
    conversationId: string,
    request: KChatBrandKitArtifactRequest,
  ): Promise<KChatArtifactPublishResult> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/kchat-backend/publish-brand-kit",
      conversationId,
      JSON.stringify(request),
    )) as string;
    return JSON.parse(raw) as KChatArtifactPublishResult;
  },
  async listArtifacts(
    conversationId: string,
  ): Promise<KChatPublishedArtifact[]> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/kchat-backend/list-artifacts",
      conversationId,
    )) as string;
    return JSON.parse(raw) as KChatPublishedArtifact[];
  },
};

// Phase 7 (Task E): `kcreate://` deeplink listener. The main
// process forwards every accepted deeplink URL on
// `kcreate/deeplink/received`; the renderer subscribes through
// this bridge so `InvitePanel.tsx` (and any future panel that
// wants to react to share-invite deeplinks) can auto-paste an
// invite payload that arrived through a KChat Desktop share card.
//
// Cold-start race: `main.ts` flushes the pending-deeplink queue
// on `did-finish-load`, but that fires before the React tree has
// mounted and registered `onUrl`. To make sure a deeplink fired
// in that millisecond window isn't lost, we register an IPC
// listener here (at preload load time, before the renderer's JS
// bundle runs) and buffer URLs until at least one consumer
// subscribes. The buffer is capped to mirror the main-side cap so
// a renderer stuck before mount can't drive unbounded memory
// growth.
//
// Subscriber model: we keep a Set<callback> rather than a single
// slot so a future second panel (e.g. an audit-log feed that also
// records deeplink arrivals) can subscribe without silently
// stealing URLs from the first subscriber. Each URL is fanned out
// to every registered listener.
const DEEPLINK_CHANNEL = "kcreate/deeplink/received";
const DEEPLINK_BUFFER_CAP = 50;
const pendingDeeplinks: string[] = [];
const deeplinkSubscribers = new Set<(url: string) => void>();

ipcRenderer.on(DEEPLINK_CHANNEL, (_evt: unknown, url: unknown): void => {
  if (typeof url !== "string") {
    return;
  }
  if (deeplinkSubscribers.size > 0) {
    // Snapshot the subscriber set before iterating so a listener
    // that unsubscribes itself during the callback doesn't mutate
    // the live set mid-iteration.
    const snapshot = Array.from(deeplinkSubscribers);
    for (const subscriber of snapshot) {
      subscriber(url);
    }
    return;
  }
  if (pendingDeeplinks.length >= DEEPLINK_BUFFER_CAP) {
    pendingDeeplinks.shift();
  }
  pendingDeeplinks.push(url);
});

const deeplink: DeeplinkBridge = {
  onUrl(callback: (url: string) => void): () => void {
    deeplinkSubscribers.add(callback);
    // Drain any URLs that arrived before any consumer was ready,
    // in arrival order so the first subscriber observes them
    // deterministically. Subsequent subscribers that join later
    // miss the drain — they only see URLs that arrive after they
    // register, which matches the "subscribe once at mount" usage
    // pattern from `InvitePanel.tsx`.
    if (pendingDeeplinks.length > 0) {
      const drained = pendingDeeplinks.splice(0);
      for (const url of drained) {
        callback(url);
      }
    }
    return () => {
      deeplinkSubscribers.delete(callback);
    };
  },
};

const textFrame: TextFrameBridge = {
  async get(nodeId: string): Promise<TextFrameOptions> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/text/frame/get",
      nodeId,
    )) as string;
    return JSON.parse(raw) as TextFrameOptions;
  },
  async update(nodeId: string, options: TextFrameOptions): Promise<void> {
    await ipcRenderer.invoke(
      "kcreate/text/frame/update",
      nodeId,
      JSON.stringify(options),
    );
  },
  async computeLayout(nodeId: string): Promise<TextLayoutWire> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/text/layout/compute",
      nodeId,
    )) as string;
    return JSON.parse(raw) as TextLayoutWire;
  },
  async getOpenTypeFeatures(nodeId: string): Promise<OpenTypeFeatures> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/text/opentype/get",
      nodeId,
    )) as string;
    return JSON.parse(raw) as OpenTypeFeatures;
  },
  async updateOpenTypeFeatures(
    nodeId: string,
    features: OpenTypeFeatures,
  ): Promise<void> {
    await ipcRenderer.invoke(
      "kcreate/text/opentype/update",
      nodeId,
      JSON.stringify(features),
    );
  },
  async link(aId: string, bId: string): Promise<void> {
    await ipcRenderer.invoke("kcreate/text/frame/link", aId, bId);
  },
  async unlink(nodeId: string): Promise<void> {
    await ipcRenderer.invoke("kcreate/text/frame/unlink", nodeId);
  },
  async setWrap(nodeId: string, mode: TextWrapMode): Promise<void> {
    // The Rust side expects the JSON-encoded enum value, e.g.
    // `"none"`, `"bounding_box"`, `"contour"`. JSON.stringify
    // wraps the string literal in double quotes for us.
    await ipcRenderer.invoke(
      "kcreate/text/frame/wrap/set",
      nodeId,
      JSON.stringify(mode),
    );
  },
};

// ---------------------------------------------------------------------------
// Phase A1 — inline text editor + font controls.
//
// Mirrors the `kcreate_bridge::text_*` entry points; each mutator
// records an undoable operation on the Rust side. The wire format
// for `setStyle` is `TextStyleWire` (camelCase JSON, fields
// `fontFamily`, `fontSize`, `lineHeight`).
// ---------------------------------------------------------------------------
const text: TextBridge = {
  async setContent(nodeId: string, content: string): Promise<void> {
    await ipcRenderer.invoke(
      "kcreate/text/content/set",
      nodeId,
      content,
    );
  },
  async setStyle(nodeId: string, style: TextStyleWire): Promise<void> {
    await ipcRenderer.invoke(
      "kcreate/text/style/set",
      nodeId,
      JSON.stringify(style),
    );
  },
  async replaceRange(
    nodeId: string,
    start: number,
    end: number,
    replacement: string,
  ): Promise<void> {
    await ipcRenderer.invoke(
      "kcreate/text/range/replace",
      nodeId,
      start,
      end,
      replacement,
    );
  },
  async getContent(nodeId: string): Promise<string> {
    return (await ipcRenderer.invoke(
      "kcreate/text/content/get",
      nodeId,
    )) as string;
  },
  async getStyle(nodeId: string): Promise<TextStyleWire> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/text/style/get",
      nodeId,
    )) as string;
    return JSON.parse(raw) as TextStyleWire;
  },
  async listFonts(): Promise<string[]> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/text/fonts/list",
    )) as string;
    return JSON.parse(raw) as string[];
  },
};

// ---------------------------------------------------------------------------
// Phase 5 — vector path operations (Block C Tasks 15, 16, 18).
// ---------------------------------------------------------------------------

const vectorOps: VectorOpsBridge = {
  async simplify(nodeId: string, tolerance: number): Promise<void> {
    await ipcRenderer.invoke("kcreate/vector/simplify", nodeId, tolerance);
  },
  async smooth(nodeId: string, iterations: number): Promise<void> {
    await ipcRenderer.invoke("kcreate/vector/smooth", nodeId, iterations);
  },
  async offset(nodeId: string, distance: number): Promise<void> {
    await ipcRenderer.invoke("kcreate/vector/offset", nodeId, distance);
  },
  async setStrokeProfile(
    nodeId: string,
    profile: StrokeWidthProfile | null,
  ): Promise<void> {
    await ipcRenderer.invoke(
      "kcreate/vector/strokeProfile/set",
      nodeId,
      JSON.stringify(profile),
    );
  },
  async applyPathEffect(
    nodeId: string,
    effect: PathEffectWire,
  ): Promise<void> {
    await ipcRenderer.invoke(
      "kcreate/vector/pathEffect/apply",
      nodeId,
      JSON.stringify(effect),
    );
  },
  async clearPathEffects(nodeId: string): Promise<void> {
    await ipcRenderer.invoke("kcreate/vector/pathEffect/clear", nodeId);
  },
};

// ---------------------------------------------------------------------------
// Phase 5 — slices (Block D Task 22).
// ---------------------------------------------------------------------------

const slice: SliceBridge = {
  async create(
    name: string,
    x: number,
    y: number,
    w: number,
    h: number,
    format: ExportFormat,
    scale: number,
  ): Promise<string> {
    return (await ipcRenderer.invoke(
      "kcreate/slice/create",
      name,
      x,
      y,
      w,
      h,
      format,
      scale,
    )) as string;
  },
  async update(sliceId: string, changes: SliceUpdateProps): Promise<void> {
    await ipcRenderer.invoke(
      "kcreate/slice/update",
      sliceId,
      JSON.stringify(changes),
    );
  },
  async delete(sliceId: string): Promise<boolean> {
    return (await ipcRenderer.invoke(
      "kcreate/slice/delete",
      sliceId,
    )) as boolean;
  },
  async list(): Promise<SliceWire[]> {
    const raw = (await ipcRenderer.invoke("kcreate/slice/list")) as string;
    return JSON.parse(raw) as SliceWire[];
  },
  async exportAll(outputDir: string): Promise<SliceResultWire[]> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/slice/exportAll",
      outputDir,
    )) as string;
    return JSON.parse(raw) as SliceResultWire[];
  },
};

// ---------------------------------------------------------------------------
// Phase 6 Tasks 25-26 — node clipboard bridge. See `ClipboardBridge` in
// shared/scene.ts for the contract.
// ---------------------------------------------------------------------------

const clipboard: ClipboardBridge = {
  async copy(nodeIds: string[]): Promise<string> {
    return (await ipcRenderer.invoke(
      "kcreate/clipboard/copy",
      nodeIds,
    )) as string;
  },
  async paste(
    payload: string,
    targetParentId: string | null,
    offsetX: number,
    offsetY: number,
  ): Promise<string[]> {
    return (await ipcRenderer.invoke(
      "kcreate/clipboard/paste",
      payload,
      targetParentId,
      offsetX,
      offsetY,
    )) as string[];
  },
};

// ---------------------------------------------------------------------------
// Phase 8 — design-token binding, constraints, autofit, page numbering,
// section pages, job presets, brand-kit versioning. See `Phase8Bridge` in
// shared/scene.ts for the contract.
// ---------------------------------------------------------------------------

const phase8: Phase8Bridge = {
  async bindToken(
    nodeId: string,
    property: string,
    tokenName: string,
  ): Promise<void> {
    await ipcRenderer.invoke(
      "kcreate/phase8/bind-token",
      nodeId,
      property,
      tokenName,
    );
  },
  async unbindToken(nodeId: string, property: string): Promise<void> {
    await ipcRenderer.invoke(
      "kcreate/phase8/unbind-token",
      nodeId,
      property,
    );
  },
  async propagateToken(tokenName: string): Promise<number> {
    return (await ipcRenderer.invoke(
      "kcreate/phase8/propagate-token",
      tokenName,
    )) as number;
  },
  async nodeTokenBindings(
    nodeId: string,
  ): Promise<Record<string, string>> {
    const json = (await ipcRenderer.invoke(
      "kcreate/phase8/node-token-bindings",
      nodeId,
    )) as string;
    return JSON.parse(json) as Record<string, string>;
  },
  async nodeConstraints(nodeId: string): Promise<Constraints> {
    const json = (await ipcRenderer.invoke(
      "kcreate/phase8/node-constraints",
      nodeId,
    )) as string;
    return JSON.parse(json) as Constraints;
  },
  async setNodeConstraints(
    nodeId: string,
    constraints: Constraints,
  ): Promise<void> {
    await ipcRenderer.invoke(
      "kcreate/phase8/set-node-constraints",
      nodeId,
      constraints,
    );
  },
  async resizeFrame(frameId: string, bounds: ResizeFrameBounds): Promise<void> {
    await ipcRenderer.invoke("kcreate/phase8/resize-frame", frameId, bounds);
  },
  async setAutoFit(nodeId: string, enabled: boolean): Promise<boolean> {
    return (await ipcRenderer.invoke(
      "kcreate/phase8/set-auto-fit",
      nodeId,
      enabled,
    )) as boolean;
  },
  async pageNumberToken(format: PageNumberFormat): Promise<string> {
    return (await ipcRenderer.invoke(
      "kcreate/phase8/page-number-token",
      format,
    )) as string;
  },
  async setPageSection(
    pageId: string,
    startNumber: number | null,
    prefix: string | null,
  ): Promise<void> {
    await ipcRenderer.invoke(
      "kcreate/phase8/set-page-section",
      pageId,
      startNumber,
      prefix,
    );
  },
  async resolvePageContexts() {
    const raw = (await ipcRenderer.invoke(
      "kcreate/phase8/resolve-page-contexts",
    )) as string;
    return JSON.parse(raw);
  },
  async exportJobPresets(job: JobType) {
    const raw = (await ipcRenderer.invoke(
      "kcreate/phase8/export-job-presets",
      job,
    )) as string;
    return JSON.parse(raw);
  },
  async brandKitSaveVersion(brandKitId: string, description: string) {
    const raw = (await ipcRenderer.invoke(
      "kcreate/phase8/brand-kit/save-version",
      brandKitId,
      description,
    )) as string;
    return JSON.parse(raw);
  },
  async brandKitListVersions(brandKitId: string) {
    const raw = (await ipcRenderer.invoke(
      "kcreate/phase8/brand-kit/list-versions",
      brandKitId,
    )) as string;
    return JSON.parse(raw);
  },
  async brandKitRestoreVersion(versionId: string) {
    const raw = (await ipcRenderer.invoke(
      "kcreate/phase8/brand-kit/restore-version",
      versionId,
    )) as string;
    return JSON.parse(raw);
  },
  async brandKitDiff(beforeId: string, afterId: string) {
    const raw = (await ipcRenderer.invoke(
      "kcreate/phase8/brand-kit/diff",
      beforeId,
      afterId,
    )) as string;
    return JSON.parse(raw);
  },
};

// ---------------------------------------------------------------------------
// Phase 8 (Task 26) — project encryption bridge.
// See `ProjectEncryptionBridge` in shared/scene.ts for the contract.
// ---------------------------------------------------------------------------

const projectEncryption: ProjectEncryptionBridge = {
  async status(): Promise<EncryptionStatus> {
    const json = (await ipcRenderer.invoke(
      "kcreate/project/encryption/status",
    )) as string;
    return JSON.parse(json) as EncryptionStatus;
  },
  async passphraseStrength(passphrase: string): Promise<number> {
    return (await ipcRenderer.invoke(
      "kcreate/project/encryption/passphrase-strength",
      passphrase,
    )) as number;
  },
  async enable(passphrase: string): Promise<EncryptionStatus> {
    const json = (await ipcRenderer.invoke(
      "kcreate/project/encryption/enable",
      passphrase,
    )) as string;
    return JSON.parse(json) as EncryptionStatus;
  },
  async changePassphrase(
    oldPassphrase: string,
    newPassphrase: string,
  ): Promise<void> {
    await ipcRenderer.invoke(
      "kcreate/project/encryption/change-passphrase",
      oldPassphrase,
      newPassphrase,
    );
  },
  async exportPlaintextRecovery(
    passphrase: string,
    outputPath: string,
  ): Promise<string> {
    return (await ipcRenderer.invoke(
      "kcreate/project/encryption/export-plaintext-recovery",
      passphrase,
      outputPath,
    )) as string;
  },
  async pickRecoveryPath(): Promise<string | null> {
    return (await ipcRenderer.invoke(
      "kcreate/project/encryption/pick-recovery-path",
    )) as string | null;
  },
};

// ---------------------------------------------------------------------------
// Phase 8 (Task 4) — design-review annotations bridge.
// See `AnnotationBridge` in shared/scene.ts for the contract.
// ---------------------------------------------------------------------------

const annotation: AnnotationBridge = {
  async create(request) {
    const raw = (await ipcRenderer.invoke(
      "kcreate/annotation/create",
      JSON.stringify(request),
    )) as string;
    return JSON.parse(raw) as Annotation;
  },
  async reply(request) {
    const raw = (await ipcRenderer.invoke(
      "kcreate/annotation/reply",
      JSON.stringify(request),
    )) as string;
    return JSON.parse(raw) as Annotation;
  },
  async list(request) {
    const raw = (await ipcRenderer.invoke(
      "kcreate/annotation/list",
      JSON.stringify(request),
    )) as string;
    return JSON.parse(raw) as AnnotationListResponse;
  },
  async resolve(request) {
    return (await ipcRenderer.invoke(
      "kcreate/annotation/resolve",
      JSON.stringify(request),
    )) as boolean;
  },
  async delete(id) {
    return (await ipcRenderer.invoke("kcreate/annotation/delete", id)) as boolean;
  },
};

// ---------------------------------------------------------------------------
// Phase 9 — guides, grid, alignment, AI palette/autofit/trace/iconify/batch-
// alt-text, PSD/Penpot/EXIF import, SVG preview, history panel, export
// validation, brief → project, memory watchdog, autosave. See `Phase9Bridge`
// in shared/scene.ts for the contract.
// ---------------------------------------------------------------------------

const phase9: Phase9Bridge = {
  async guideCreate(pageId, orientation, position, color, locked) {
    const raw = (await ipcRenderer.invoke(
      "kcreate/phase9/guide/create",
      pageId,
      orientation,
      position,
      color,
      locked,
    )) as string;
    return JSON.parse(raw) as GuideInfo;
  },
  async guideDelete(id) {
    return (await ipcRenderer.invoke("kcreate/phase9/guide/delete", id)) as boolean;
  },
  async guideClearPage(pageId) {
    return (await ipcRenderer.invoke("kcreate/phase9/guide/clear-page", pageId)) as number;
  },
  async guideList(pageId) {
    const raw = (await ipcRenderer.invoke("kcreate/phase9/guide/list", pageId)) as string;
    return JSON.parse(raw) as GuideInfo[];
  },
  async guideListAll() {
    const raw = (await ipcRenderer.invoke("kcreate/phase9/guide/list-all")) as string;
    return JSON.parse(raw) as GuideInfo[];
  },

  async artboardGridSettings(artboardId) {
    const raw = (await ipcRenderer.invoke("kcreate/phase9/grid/get", artboardId)) as string;
    return JSON.parse(raw) as GridSettingsInfo;
  },
  async artboardSetGrid(artboardId, enabled, spacing, subdivisions, color) {
    const raw = (await ipcRenderer.invoke(
      "kcreate/phase9/grid/set",
      artboardId,
      enabled,
      spacing,
      subdivisions,
      color,
    )) as string;
    return JSON.parse(raw) as GridSettingsInfo;
  },

  async documentAlign(nodeIds, alignment: Alignment) {
    const raw = (await ipcRenderer.invoke(
      "kcreate/phase9/document/align",
      nodeIds,
      alignment,
    )) as string;
    return JSON.parse(raw) as AlignmentResult[];
  },
  async documentDistribute(nodeIds, axis: DistributeAxis) {
    const raw = (await ipcRenderer.invoke(
      "kcreate/phase9/document/distribute",
      nodeIds,
      axis,
    )) as string;
    return JSON.parse(raw) as AlignmentResult[];
  },

  async paletteExtractAndApplyBrandKit(nodeId, numColors, brandKitName) {
    const raw = (await ipcRenderer.invoke(
      "kcreate/phase9/palette/apply-brand-kit",
      nodeId,
      numColors,
      brandKitName,
    )) as string;
    return JSON.parse(raw) as PaletteApplyResult;
  },

  async textAutofitRecompute(nodeId) {
    const raw = (await ipcRenderer.invoke(
      "kcreate/phase9/text/autofit-recompute",
      nodeId,
    )) as string;
    return JSON.parse(raw) as AutofitRecomputeResult;
  },

  async aiTraceRaster(nodeId, threshold, simplifyTolerance) {
    const raw = (await ipcRenderer.invoke(
      "kcreate/phase9/ai/trace-raster",
      nodeId,
      threshold,
      simplifyTolerance,
    )) as string;
    return JSON.parse(raw) as TraceResult;
  },
  async aiIconify(sourceNodeId, gridSize) {
    const raw = (await ipcRenderer.invoke(
      "kcreate/phase9/ai/iconify",
      sourceNodeId,
      gridSize,
    )) as string;
    return JSON.parse(raw) as IconifyResultInfo;
  },
  async aiBatchAltText(pageId) {
    const raw = (await ipcRenderer.invoke(
      "kcreate/phase9/ai/batch-alt-text",
      pageId,
    )) as string;
    return JSON.parse(raw) as BatchAltTextEntry[];
  },

  async importPsd(path) {
    const raw = (await ipcRenderer.invoke("kcreate/phase9/import/psd", path)) as string;
    return JSON.parse(raw) as ImportSummary;
  },
  async importPenpot(path) {
    const raw = (await ipcRenderer.invoke("kcreate/phase9/import/penpot", path)) as string;
    return JSON.parse(raw) as ImportSummary;
  },
  async imageReadExif(bytes) {
    const raw = (await ipcRenderer.invoke(
      "kcreate/phase9/image/read-exif",
      bytes,
    )) as string;
    return JSON.parse(raw) as ExifResult;
  },

  async exportSvgPreview(svgBytes, maxWidth, maxHeight, transparent) {
    const raw = (await ipcRenderer.invoke(
      "kcreate/phase9/export/svg-preview",
      svgBytes,
      maxWidth,
      maxHeight,
      transparent,
    )) as string;
    return JSON.parse(raw) as SvgPreviewInfo;
  },

  async operationLogFilter(filter: OperationLogFilter) {
    const raw = (await ipcRenderer.invoke(
      "kcreate/phase9/operation-log/filter",
      JSON.stringify(filter),
    )) as string;
    return JSON.parse(raw) as OperationInfo[];
  },
  async exportValidate(request: ExportValidationRequest) {
    const raw = (await ipcRenderer.invoke(
      "kcreate/phase9/export/validate",
      JSON.stringify(request),
    )) as string;
    return JSON.parse(raw) as ExportValidationReport;
  },
  async briefToProject(plan: BriefPlan) {
    const raw = (await ipcRenderer.invoke(
      "kcreate/phase9/brief/to-project",
      JSON.stringify(plan),
    )) as string;
    return JSON.parse(raw) as BriefApplyResult;
  },

  async memoryWatchdogStart(pollIntervalMs) {
    return (await ipcRenderer.invoke(
      "kcreate/phase9/memory/watchdog-start",
      pollIntervalMs,
    )) as boolean;
  },
  async memoryWatchdogStop() {
    return (await ipcRenderer.invoke("kcreate/phase9/memory/watchdog-stop")) as boolean;
  },
  async drainMemoryEvents() {
    const raw = (await ipcRenderer.invoke(
      "kcreate/phase9/memory/drain-events",
    )) as string;
    return JSON.parse(raw) as MemoryPressureEvent[];
  },
  async runtimeGpuBackendName() {
    return (await ipcRenderer.invoke(
      "kcreate/phase9/runtime/gpu-backend-name",
    )) as string;
  },

  async autosaveStart() {
    return (await ipcRenderer.invoke("kcreate/phase9/autosave/start")) as boolean;
  },
  async autosaveStop() {
    return (await ipcRenderer.invoke("kcreate/phase9/autosave/stop")) as boolean;
  },
  async autosaveForceNow() {
    return (await ipcRenderer.invoke("kcreate/phase9/autosave/force-now")) as boolean;
  },
  async autosaveStatus() {
    const raw = (await ipcRenderer.invoke("kcreate/phase9/autosave/status")) as string;
    return JSON.parse(raw) as AutosaveStatus;
  },
  async autosaveRecoveryAvailable() {
    const raw = (await ipcRenderer.invoke(
      "kcreate/phase9/autosave/recovery-available",
    )) as string;
    return JSON.parse(raw) as AutosaveMarker | null;
  },
  async autosaveRecover() {
    await ipcRenderer.invoke("kcreate/phase9/autosave/recover");
  },
  async autosaveDismissRecovery() {
    await ipcRenderer.invoke("kcreate/phase9/autosave/dismiss-recovery");
  },
};

// ---------------------------------------------------------------------------
// Phase 10 — Image Studio AI, Vector/Layout AI, Export AI + Live Preview,
// Brand Hub + Plugin Marketplace, Preferences. See `Phase10Bridge`
// in shared/scene.ts for the contract and `crates/kcreate_bridge/src/
// phase10.rs` for the Rust side.
// ---------------------------------------------------------------------------

const phase10: Phase10Bridge = {
  // ---- Block A — Image Studio AI ------------------------------------------
  async aiDenoise(nodeId, strength, searchRadius, patchRadius) {
    const raw = (await ipcRenderer.invoke(
      "kcreate/phase10/ai/denoise",
      nodeId,
      strength,
      searchRadius,
      patchRadius,
    )) as string;
    return JSON.parse(raw) as DenoiseResult;
  },
  async aiInpaint(nodeId, maskRects, patchRadius, numIterations, pyramidLevels) {
    const raw = (await ipcRenderer.invoke(
      "kcreate/phase10/ai/inpaint",
      nodeId,
      JSON.stringify(maskRects),
      patchRadius,
      numIterations,
      pyramidLevels,
    )) as string;
    return JSON.parse(raw) as InpaintResult;
  },
  async aiAutoColor(nodeId, mode) {
    const raw = (await ipcRenderer.invoke(
      "kcreate/phase10/ai/auto-color",
      nodeId,
      mode,
    )) as string;
    return JSON.parse(raw) as AutoColorResult;
  },
  async aiSegmentAtPoint(nodeId, pointX, pointY, isPositive) {
    const raw = (await ipcRenderer.invoke(
      "kcreate/phase10/ai/segment-at-point",
      nodeId,
      pointX,
      pointY,
      isPositive,
    )) as string;
    return JSON.parse(raw) as SegmentAtPointResult;
  },
  async aiSmartSelectAtPoint(
    nodeId,
    x,
    y,
    tolerance,
    mode,
    previousMaskBase64,
  ) {
    const raw = (await ipcRenderer.invoke(
      "kcreate/phase10/ai/smart-select-at-point",
      nodeId,
      x,
      y,
      tolerance,
      mode,
      previousMaskBase64,
    )) as string;
    return JSON.parse(raw) as SmartSelectAtPointResult;
  },

  // ---- Block B — Vector/Layout AI -----------------------------------------
  async aiMatchStroke(sourceId, targetIds) {
    const raw = (await ipcRenderer.invoke(
      "kcreate/phase10/ai/match-stroke",
      sourceId,
      targetIds,
    )) as string;
    return JSON.parse(raw) as StrokeMatchSummary;
  },
  async aiExtractGlyph(
    nodeId,
    cropX,
    cropY,
    cropWidth,
    cropHeight,
    emSize,
  ) {
    const raw = (await ipcRenderer.invoke(
      "kcreate/phase10/ai/extract-glyph",
      nodeId,
      cropX,
      cropY,
      cropWidth,
      cropHeight,
      emSize,
    )) as string;
    return JSON.parse(raw) as ExtractedGlyphResult;
  },
  async aiReformatToDeck(pageId) {
    const raw = (await ipcRenderer.invoke(
      "kcreate/phase10/ai/reformat-to-deck",
      pageId,
    )) as string;
    return JSON.parse(raw) as ReformatDeckResult;
  },
  async aiBriefToOnePager(brief, pageSize) {
    const raw = (await ipcRenderer.invoke(
      "kcreate/phase10/ai/brief-to-one-pager",
      brief,
      pageSize,
    )) as string;
    return JSON.parse(raw) as BriefToOnePagerResult;
  },
  async aiGenerateThemedDesign(brief, options) {
    const raw = (await ipcRenderer.invoke(
      "kcreate/phase10/ai/generate-themed-design",
      brief,
      JSON.stringify(options),
    )) as string;
    return JSON.parse(raw) as ThemedDesignApplyResult;
  },
  async aiRefineThemedDesign(instruction) {
    const raw = (await ipcRenderer.invoke(
      "kcreate/phase10/ai/refine-themed-design",
      instruction,
    )) as string;
    return JSON.parse(raw) as ThemedDesignApplyResult;
  },
  async aiHarmonizePalette(brandKitId, harmonyType) {
    const raw = (await ipcRenderer.invoke(
      "kcreate/phase10/ai/harmonize-palette",
      brandKitId,
      harmonyType,
    )) as string;
    return JSON.parse(raw) as HarmonyResult;
  },
  async aiSuggestTypePairing(headingFontName) {
    const raw = (await ipcRenderer.invoke(
      "kcreate/phase10/ai/suggest-type-pairing",
      headingFontName,
    )) as string;
    return JSON.parse(raw) as TypePairingResult;
  },

  // ---- Block C — Export AI + Live Preview ---------------------------------
  async exportOptimizeSvg(svg) {
    const raw = (await ipcRenderer.invoke(
      "kcreate/phase10/export/optimize-svg",
      svg,
    )) as string;
    return JSON.parse(raw) as SvgOptimizeReport;
  },
  async exportSmartCompress(nodeId, format, targetSsim) {
    const raw = (await ipcRenderer.invoke(
      "kcreate/phase10/export/smart-compress",
      nodeId,
      format,
      targetSsim,
    )) as string;
    return JSON.parse(raw) as SmartCompressReport;
  },
  async exportPreview(request) {
    const raw = (await ipcRenderer.invoke(
      "kcreate/phase10/export/preview",
      JSON.stringify(request),
    )) as string;
    return JSON.parse(raw) as ExportPreviewResponse;
  },
  async importAi(path) {
    const raw = (await ipcRenderer.invoke(
      "kcreate/phase10/import/ai",
      path,
    )) as string;
    return JSON.parse(raw) as AiImportSummary;
  },

  // ---- Block D — Brand Hub + Plugin Marketplace ---------------------------
  async aiBrandToBrochure(brandKitId, numPages) {
    const raw = (await ipcRenderer.invoke(
      "kcreate/phase10/ai/brand-to-brochure",
      brandKitId,
      numPages,
    )) as string;
    return JSON.parse(raw) as BrochurePlanResult;
  },
  async pluginMarketplaceList() {
    const raw = (await ipcRenderer.invoke(
      "kcreate/phase10/plugin-marketplace/list",
    )) as string;
    return JSON.parse(raw) as PluginListing[];
  },
  async pluginMarketplaceInstallLocal(path) {
    const raw = (await ipcRenderer.invoke(
      "kcreate/phase10/plugin-marketplace/install-local",
      path,
    )) as string;
    return JSON.parse(raw) as PluginListing;
  },
  async pluginMarketplaceRemove(id) {
    return (await ipcRenderer.invoke(
      "kcreate/phase10/plugin-marketplace/remove",
      id,
    )) as boolean;
  },
  async exportPdfMulti(options, outputPath) {
    const raw = (await ipcRenderer.invoke(
      "kcreate/phase10/export/pdf-multi",
      JSON.stringify(options),
      outputPath,
    )) as string;
    return JSON.parse(raw) as PdfMultiReport;
  },

  // ---- Block D Task 23 — Preferences --------------------------------------
  async preferencesLoad() {
    const raw = (await ipcRenderer.invoke(
      "kcreate/phase10/preferences/load",
    )) as string;
    return JSON.parse(raw) as Preferences;
  },
  async preferencesSave(prefs) {
    await ipcRenderer.invoke(
      "kcreate/phase10/preferences/save",
      JSON.stringify(prefs),
    );
  },
};

// Phase C — system surface for the welcome modal's "Open download
// page" fallback. The main process validates the URL against the
// `onboardingDownloader.ALLOWED_HOSTS` allow-list before passing
// it to `shell.openExternal`, so a compromised renderer cannot
// coax the main process into opening arbitrary URLs.
const system: SystemBridge = {
  async openExternal(url: string): Promise<void> {
    await ipcRenderer.invoke("kcreate/system/openExternal", url);
  },
};

// Phase C — one-click recommended-pack download surface. The
// renderer triggers the download via `installRecommendedPack()`,
// subscribes to progress via `onInstallProgress(fn)` (returns an
// unsubscribe handle), and aborts via `cancelInstall()`. See
// `main/src/onboardingDownloader.ts` for the wire-shape of
// `OnboardingProgress` / `OnboardingInstallReport`.
const onboarding: OnboardingBridge = {
  async installRecommendedPack(): Promise<OnboardingInstallReport> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/onboarding/installRecommendedPack",
    )) as string;
    return JSON.parse(raw) as OnboardingInstallReport;
  },
  async cancelInstall(): Promise<void> {
    await ipcRenderer.invoke("kcreate/onboarding/cancelInstall");
  },
  onInstallProgress(
    fn: (progress: OnboardingProgress) => void,
  ): () => void {
    const handler = (_e: unknown, progress: OnboardingProgress): void => {
      fn(progress);
    };
    ipcRenderer.on("kcreate/onboarding/installProgress", handler);
    return (): void => {
      ipcRenderer.removeListener(
        "kcreate/onboarding/installProgress",
        handler,
      );
    };
  },
};

contextBridge.exposeInMainWorld("kcreate", {
  renderer,
  document,
  canvas,
  ai,
  llm,
  vision,
  imageGen,
  mcp,
  runtime,
  export: exportApi,
  designTokens,
  brandKit,
  theme,
  exportPreset,
  artboard,
  component,
  layout,
  interaction,
  masterPage,
  layoutStudio,
  assets,
  templateMarketplace,
  audit,
  thumbnail,
  recentProjects,
  preflight,
  iconPack,
  batch,
  aiModel,
  pdfImport,
  figmaImport,
  sketchImport,
  plugin,
  mcpPermission,
  color,
  canvasSnap,
  rasterOps,
  textFrame,
  text,
  vectorOps,
  slice,
  session,
  kchat,
  kchatBackend,
  deeplink,
  clipboard,
  phase8,
  phase9,
  phase10,
  projectEncryption,
  annotation,
  system,
  onboarding,
});
