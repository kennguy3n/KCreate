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
  | { type: "path"; commands: PathCommand[] };

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
   */
  getFrame(): Promise<Uint8Array | null>;
  frameInfo(): Promise<FrameInfo | null>;
}

declare global {
  interface Window {
    kcreate: {
      renderer: RendererBridge;
    };
  }
}
