// CanvasHost — renderer-side presentation surface.
//
// This component does NOT render the scene itself. The Rust
// `kcreate_renderer` crate owns the entire rendering pipeline; this
// component is purely a presentation surface:
//   1. Owns an HTML `<canvas>` of the editor viewport dimensions.
//   2. On every `requestAnimationFrame`, asks the Rust renderer for the
//      latest frame via the preload-exposed IPC bridge.
//   3. Writes the returned RGBA8 pixel buffer to the canvas via
//      `putImageData`.
//   4. Forwards pointer / keyboard events to the bridge so Rust can do
//      hit testing and tool state.
//
// Phase 1 will replace the readback/transfer step with a native child
// view that Rust composits into directly. The component interface is
// designed to survive that swap unchanged.

import { useEffect, useRef, useState } from "react";

import type { Scene } from "../../../shared/scene";

export interface CanvasHostProps {
  /**
   * Width and height of the canvas in CSS pixels. Multiplied by
   * devicePixelRatio for the actual render target size.
   */
  width: number;
  height: number;
  /**
   * The current scene to render. The component sends this to the Rust
   * renderer when it changes. Sending the same scene object twice is
   * fine — the renderer's display-list cache short-circuits the work.
   */
  scene: Scene;
  /**
   * Viewport pan + zoom. Sent to the renderer when it changes.
   */
  viewport?: { panX: number; panY: number; zoom: number };
  /**
   * Called whenever a new frame is presented to the canvas. Useful for
   * FPS readouts in the host UI.
   */
  onFramePresented?: (frameId: number) => void;
  /**
   * Forwarded pointer events. The host typically uses these to drive
   * tool state (selection rectangle, drag, etc.).
   */
  onPointer?: (event: React.PointerEvent<HTMLCanvasElement>) => void;
}

interface Viewport {
  panX: number;
  panY: number;
  zoom: number;
}

const ZERO_VIEWPORT: Viewport = { panX: 0, panY: 0, zoom: 1 };

export function CanvasHost(props: CanvasHostProps): JSX.Element {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const ctxRef = useRef<CanvasRenderingContext2D | null>(null);
  const imageDataRef = useRef<ImageData | null>(null);
  const lastFrameIdRef = useRef<number>(0);
  const sceneRef = useRef<Scene>(props.scene);
  const viewportRef = useRef<Viewport>(props.viewport ?? ZERO_VIEWPORT);
  const dprRef = useRef<number>(
    typeof window === "undefined" ? 1 : window.devicePixelRatio || 1,
  );
  const [initError, setInitError] = useState<string | null>(null);

  // Keep refs up to date on every render.
  sceneRef.current = props.scene;
  viewportRef.current = props.viewport ?? ZERO_VIEWPORT;

  useEffect(() => {
    const bridge = window.kcreate?.renderer;
    if (!bridge) {
      setInitError(
        "Renderer bridge is unavailable. The preload script may not be loaded.",
      );
      return undefined;
    }

    const canvas = canvasRef.current;
    if (!canvas) return undefined;

    const ctx = canvas.getContext("2d", { alpha: false });
    if (!ctx) {
      setInitError("2D canvas context not available");
      return undefined;
    }
    ctxRef.current = ctx;

    let cancelled = false;
    let rafHandle: number | null = null;

    (async () => {
      const dpr = dprRef.current;
      const wPx = Math.max(1, Math.round(props.width * dpr));
      const hPx = Math.max(1, Math.round(props.height * dpr));
      canvas.width = wPx;
      canvas.height = hPx;
      imageDataRef.current = ctx.createImageData(wPx, hPx);

      try {
        await bridge.init(wPx, hPx);
        await bridge.setViewport(
          viewportRef.current.panX,
          viewportRef.current.panY,
          viewportRef.current.zoom,
        );
      } catch (err) {
        if (!cancelled) {
          setInitError(
            err instanceof Error ? err.message : "init failed: " + String(err),
          );
        }
        return;
      }

      const tick = async (): Promise<void> => {
        if (cancelled) return;
        try {
          // Sync viewport into the renderer every frame. The renderer
          // ignores no-op changes (it compares old vs new values
          // internally before invalidating).
          const vp = viewportRef.current;
          await bridge.setViewport(vp.panX, vp.panY, vp.zoom);

          const frameId = await bridge.render(sceneRef.current);
          if (frameId !== lastFrameIdRef.current) {
            const buf = await bridge.getFrame();
            const info = await bridge.frameInfo();
            if (buf && info && imageDataRef.current) {
              const expected = info.width * info.height * 4;
              if (buf.byteLength === expected) {
                imageDataRef.current.data.set(buf);
                ctx.putImageData(imageDataRef.current, 0, 0);
              }
            }
            lastFrameIdRef.current = frameId;
            props.onFramePresented?.(frameId);
          }
        } catch (err) {
          // Surfacing the error to React state would unmount the
          // component; that's heavy-handed for a transient render
          // failure. Log it for the devtools and try again next frame.
          console.warn("kcreate render tick failed", err);
        } finally {
          if (!cancelled) {
            rafHandle = requestAnimationFrame(() => {
              void tick();
            });
          }
        }
      };

      rafHandle = requestAnimationFrame(() => {
        void tick();
      });
    })().catch((err: unknown) => {
      if (!cancelled) {
        setInitError(
          err instanceof Error ? err.message : "init failed: " + String(err),
        );
      }
    });

    return () => {
      cancelled = true;
      if (rafHandle !== null) cancelAnimationFrame(rafHandle);
      void bridge.shutdown().catch(() => {
        /* best-effort */
      });
    };
    // We intentionally do NOT depend on props.scene/viewport: those are
    // tracked via refs so the rAF loop sees current values without
    // re-subscribing. The effect lifecycle owns init/shutdown.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [props.width, props.height]);

  // Handle resize separately from init/shutdown to avoid tearing down
  // the renderer on every viewport change.
  useEffect(() => {
    const bridge = window.kcreate?.renderer;
    if (!bridge) return;
    const dpr = dprRef.current;
    const wPx = Math.max(1, Math.round(props.width * dpr));
    const hPx = Math.max(1, Math.round(props.height * dpr));
    void bridge.resize(wPx, hPx).catch(() => {
      /* swallow during teardown */
    });
    const canvas = canvasRef.current;
    if (canvas && ctxRef.current) {
      canvas.width = wPx;
      canvas.height = hPx;
      imageDataRef.current = ctxRef.current.createImageData(wPx, hPx);
    }
  }, [props.width, props.height]);

  if (initError) {
    return (
      <div role="alert" style={{ padding: 8, color: "#f55" }}>
        Renderer error: {initError}
      </div>
    );
  }

  return (
    <canvas
      ref={canvasRef}
      style={{
        width: props.width,
        height: props.height,
        display: "block",
        cursor: "default",
      }}
      onPointerDown={props.onPointer}
      onPointerMove={props.onPointer}
      onPointerUp={props.onPointer}
    />
  );
}
