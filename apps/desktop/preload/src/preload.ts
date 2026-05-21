// Preload script. Runs in a privileged Node context but is exposed to
// the renderer page via `contextBridge`. The renderer can only call the
// methods we explicitly expose here.

import { contextBridge, ipcRenderer } from "electron";

import type {
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
};

contextBridge.exposeInMainWorld("kcreate", { renderer });
