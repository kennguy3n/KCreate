// Preload script. Runs in a privileged Node context but is exposed to
// the renderer page via `contextBridge`. The renderer can only call the
// methods we explicitly expose here.

import { contextBridge, ipcRenderer } from "electron";

import type {
  AcquiredFrame,
  FrameInfo,
  RendererBridge,
  RendererInfo,
  Scene,
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

contextBridge.exposeInMainWorld("kcreate", { renderer });
