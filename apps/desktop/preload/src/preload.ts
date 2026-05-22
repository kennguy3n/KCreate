// Preload script. Runs in a privileged Node context but is exposed to
// the renderer page via `contextBridge`. The renderer can only call the
// methods we explicitly expose here.

import { contextBridge, ipcRenderer } from "electron";

import type {
  AcquiredFrame,
  AiBridge,
  ArtboardBridge,
  ArtboardInfo,
  ArtboardPreset,
  BrandKit,
  BrandKitBridge,
  CanvasBridge,
  ComponentBridge,
  ComponentInfo,
  CreateNodeProps,
  FlexLayout,
  GridLayout,
  Interaction,
  InteractionAction,
  InteractionBridge,
  InteractionTrigger,
  LayoutBridge,
  LayoutStudioBridge,
  LayoutTemplate,
  MasterPageBridge,
  MasterPageInfo,
  PageLayout,
  PageOrientation,
  PageSizeId,
  DesignTokens,
  DesignTokensBridge,
  DocumentBridge,
  DocumentStatus,
  ExportBridge,
  ExportFormat,
  ExportPreset,
  ExportPresetBridge,
  FrameInfo,
  InspectCode,
  JpegExportOptions,
  LayerNamingResult,
  LlmBridge,
  LlmJsonResult,
  LlmMessage,
  LlmReply,
  LlmStatus,
  McpBridge,
  NodeInfo,
  PdfExportOptions,
  PngExportOptions,
  ProjectInfo,
  RendererBridge,
  RendererInfo,
  ResourceLimits,
  RuntimeBridge,
  RuntimeStatus,
  Scene,
  ScratchCleanupResult,
  SvgExportOptions,
  UpdateNodeProps,
  WebpExportOptions,
} from "../../shared/scene";

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
    )) as FrameInfoSnake | null;
    if (!info) return null;
    return {
      frameId: info.frame_id,
      width: info.width,
      height: info.height,
      byteLength: info.byte_length,
    };
  },
  async acquireFrame(): Promise<AcquiredFrame | null> {
    const frame = (await ipcRenderer.invoke(
      "kcreate/renderer/acquireFrame",
    )) as AcquiredFrameSnake | null;
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
      frameId: frame.frame_id,
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

// Snake-case shapes returned from the native bridge. Documented here in
// the preload so the renderer-facing API is unambiguously camelCase.
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
  /// Axis-aligned bounds in document space. Mirrors
  /// `kcreate_core::Node::bounds`; the napi bridge carries it as
  /// `bounds` directly on every NodeInfo so the renderer can render
  /// hotspots / hit-test overlays without a second IPC round trip.
  bounds: BoundsSnake;
  /// Already camelCased on the Rust side via #[serde(rename)]. We
  /// pass it through verbatim because the inner field names are
  /// also camelCased (definitionId / activeVariantId).
  componentInstance?: {
    definitionId: string;
    activeVariantId: string;
    overrides: Record<string, unknown>;
  };
  /// Free-form metadata mirror of `Node::metadata`. Omitted when
  /// empty (Rust skips serializing empty maps).
  metadata?: Record<string, unknown>;
};

type RuntimeStatusSnake = {
  device_tier: string;
  gpu_available: boolean;
  gpu_name: string | null;
  platform: string;
  total_ram_mb: number;
};

function projectFromSnake(p: ProjectInfoSnake): ProjectInfo {
  return {
    id: p.id,
    name: p.name,
    path: p.path,
    createdAt: p.created_at,
    modifiedAt: p.modified_at,
  };
}

function nodeFromSnake(n: NodeInfoSnake): NodeInfo {
  return {
    id: n.id,
    nodeType: n.node_type,
    parentId: n.parent_id,
    children: n.children,
    name: n.name,
    visible: n.visible,
    locked: n.locked,
    bounds: {
      x: n.bounds.x,
      y: n.bounds.y,
      width: n.bounds.width,
      height: n.bounds.height,
    },
    ...(n.componentInstance ? { componentInstance: n.componentInstance } : {}),
    ...(n.metadata ? { metadata: n.metadata } : {}),
  };
}

type DocumentStatusSnake = {
  node_count: number;
  can_undo: boolean;
  can_redo: boolean;
  undo_depth: number;
  redo_depth: number;
};

function documentStatusFromSnake(s: DocumentStatusSnake): DocumentStatus {
  return {
    nodeCount: s.node_count,
    canUndo: s.can_undo,
    canRedo: s.can_redo,
    undoDepth: s.undo_depth,
    redoDepth: s.redo_depth,
  };
}

function runtimeFromSnake(s: RuntimeStatusSnake): RuntimeStatus {
  return {
    deviceTier: s.device_tier,
    gpuAvailable: s.gpu_available,
    gpuName: s.gpu_name,
    platform: s.platform,
    totalRamMb: s.total_ram_mb,
  };
}

const document: DocumentBridge = {
  async createProject(name, dir): Promise<ProjectInfo> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/project/create",
      name,
      dir,
    )) as ProjectInfoSnake;
    return projectFromSnake(raw);
  },
  async openProject(dir): Promise<ProjectInfo> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/project/open",
      dir,
    )) as ProjectInfoSnake;
    return projectFromSnake(raw);
  },
  async saveProject(): Promise<void> {
    await ipcRenderer.invoke("kcreate/project/save");
  },
  async closeProject(): Promise<void> {
    await ipcRenderer.invoke("kcreate/project/close");
  },
  async getProjectInfo(): Promise<ProjectInfo | null> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/project/getInfo",
    )) as ProjectInfoSnake | null;
    return raw ? projectFromSnake(raw) : null;
  },
  async getDocumentTree(): Promise<NodeInfo[]> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/document/getTree",
    )) as NodeInfoSnake[];
    return raw.map(nodeFromSnake);
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
  async deleteNode(nodeId: string): Promise<void> {
    await ipcRenderer.invoke("kcreate/document/deleteNode", nodeId);
  },
  async undo(): Promise<string[] | null> {
    return (await ipcRenderer.invoke(
      "kcreate/document/undo",
    )) as string[] | null;
  },
  async redo(): Promise<string[] | null> {
    return (await ipcRenderer.invoke(
      "kcreate/document/redo",
    )) as string[] | null;
  },
  async status(): Promise<DocumentStatus | null> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/document/status",
    )) as DocumentStatusSnake | null;
    return raw ? documentStatusFromSnake(raw) : null;
  },
};

const runtime: RuntimeBridge = {
  async status(): Promise<RuntimeStatus> {
    const raw = (await ipcRenderer.invoke(
      "kcreate/runtime/status",
    )) as RuntimeStatusSnake;
    return runtimeFromSnake(raw);
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
  async writeTextFile(target: string, content: string): Promise<number> {
    return (await ipcRenderer.invoke(
      "kcreate/runtime/writeTextFile",
      target,
      content,
    )) as number;
  },
};

type ResourceLimitsSnake = {
  device_tier: string;
  low_resource_mode: boolean;
  effective_undo_depth: number;
  effective_raster_cache_mb: number;
  effective_max_model_mb: number;
  gpu_rendering_allowed: boolean;
};

function resourceLimitsFromSnake(s: ResourceLimitsSnake): ResourceLimits {
  return {
    deviceTier: s.device_tier,
    lowResourceMode: s.low_resource_mode,
    effectiveUndoDepth: s.effective_undo_depth,
    effectiveRasterCacheMb: s.effective_raster_cache_mb,
    effectiveMaxModelMb: s.effective_max_model_mb,
    gpuRenderingAllowed: s.gpu_rendering_allowed,
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
    return (await ipcRenderer.invoke(
      "kcreate/interaction/add",
      nodeId,
      trigger,
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

contextBridge.exposeInMainWorld("kcreate", {
  renderer,
  document,
  canvas,
  ai,
  llm,
  mcp,
  runtime,
  export: exportApi,
  designTokens,
  brandKit,
  exportPreset,
  artboard,
  component,
  layout,
  interaction,
  masterPage,
  layoutStudio,
});
