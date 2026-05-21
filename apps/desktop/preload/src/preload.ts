// Preload script. Runs in a privileged Node context but is exposed to
// the renderer page via `contextBridge`. The renderer can only call the
// methods we explicitly expose here.

import { contextBridge, ipcRenderer } from "electron";

import type {
  AcquiredFrame,
  AiBridge,
  CanvasBridge,
  CreateNodeProps,
  DocumentBridge,
  DocumentStatus,
  ExportBridge,
  FrameInfo,
  JpegExportOptions,
  McpBridge,
  NodeInfo,
  PdfExportOptions,
  PngExportOptions,
  ProjectInfo,
  RendererBridge,
  RendererInfo,
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
};

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
  async hitTest(x: number, y: number): Promise<string | null> {
    return (await ipcRenderer.invoke(
      "kcreate/canvas/hitTest",
      x,
      y,
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
  async createText(parentId, x, y, text, fontSize): Promise<string> {
    return (await ipcRenderer.invoke(
      "kcreate/canvas/createText",
      parentId,
      x,
      y,
      text,
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

contextBridge.exposeInMainWorld("kcreate", {
  renderer,
  document,
  canvas,
  ai,
  mcp,
  runtime,
  export: exportApi,
});
