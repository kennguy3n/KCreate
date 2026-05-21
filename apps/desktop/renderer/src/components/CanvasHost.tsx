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

function viewportEquals(a: Viewport, b: Viewport): boolean {
  return a.panX === b.panX && a.panY === b.panY && a.zoom === b.zoom;
}

export function CanvasHost(props: CanvasHostProps): JSX.Element {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const ctxRef = useRef<CanvasRenderingContext2D | null>(null);
  const imageDataRef = useRef<ImageData | null>(null);
  const imageDataDimsRef = useRef<{ w: number; h: number }>({ w: 0, h: 0 });
  const lastFrameIdRef = useRef<number>(0);
  // Last viewport sent to the bridge. Used to short-circuit setViewport
  // calls — the Rust side also dedupes internally, but skipping the IPC
  // round-trip is strictly better for end-to-end latency.
  const lastSentViewportRef = useRef<Viewport | null>(null);
  const sceneRef = useRef<Scene>(props.scene);
  const viewportRef = useRef<Viewport>(props.viewport ?? ZERO_VIEWPORT);
  const dprRef = useRef<number>(
    typeof window === "undefined" ? 1 : window.devicePixelRatio || 1,
  );
  const [initError, setInitError] = useState<string | null>(null);

  // Keep refs up to date on every render.
  sceneRef.current = props.scene;
  viewportRef.current = props.viewport ?? ZERO_VIEWPORT;

  // Init / rAF loop. This effect runs ONCE per component mount — its
  // dependency array is empty. Width / height changes are funnelled
  // through the `renderer.init(...)` call below (which is idempotent
  // and resizes in place if the renderer already exists at a different
  // size), so we never tear down the GPU device just because the host
  // resized the canvas.
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

    const ensureImageData = (w: number, h: number): void => {
      const dims = imageDataDimsRef.current;
      if (dims.w === w && dims.h === h && imageDataRef.current) return;
      imageDataRef.current = ctx.createImageData(w, h);
      imageDataDimsRef.current = { w, h };
      canvas.width = w;
      canvas.height = h;
    };

    (async () => {
      const dpr = dprRef.current;
      const wPx = Math.max(1, Math.round(props.width * dpr));
      const hPx = Math.max(1, Math.round(props.height * dpr));
      ensureImageData(wPx, hPx);

      try {
        await bridge.init(wPx, hPx);
        await bridge.setViewport(
          viewportRef.current.panX,
          viewportRef.current.panY,
          viewportRef.current.zoom,
        );
        lastSentViewportRef.current = { ...viewportRef.current };
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
          // Only sync the viewport if the host actually changed it.
          // Saves an IPC round trip per frame for the steady-state /
          // static-viewport case. The Rust side also dedupes, but
          // skipping the IPC round-trip avoids serializing / awaiting
          // a no-op promise.
          const vp = viewportRef.current;
          const last = lastSentViewportRef.current;
          if (!last || !viewportEquals(last, vp)) {
            await bridge.setViewport(vp.panX, vp.panY, vp.zoom);
            lastSentViewportRef.current = { ...vp };
          }

          const frameId = await bridge.render(sceneRef.current);
          if (frameId !== lastFrameIdRef.current) {
            // Atomically get bytes + dimensions in a single IPC round
            // trip. `acquireFrame` guarantees the buffer length matches
            // the reported width × height × 4 even if a resize is in
            // flight on the host side, eliminating the tearing window
            // that existed when we called `getFrame()` and
            // `frameInfo()` separately.
            const frame = await bridge.acquireFrame();
            if (frame) {
              const expected = frame.width * frame.height * 4;
              if (frame.bytes.byteLength === expected) {
                ensureImageData(frame.width, frame.height);
                imageDataRef.current!.data.set(frame.bytes);
                ctx.putImageData(imageDataRef.current!, 0, 0);
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

    // IMPORTANT: we deliberately do NOT call `bridge.shutdown()` here.
    // The Rust renderer is a process-wide singleton owned by the main
    // process; the Electron main process tears it down in its
    // `will-quit` handler. Calling shutdown from a component
    // teardown would (a) destroy the GPU device for any other
    // CanvasHost instance that happens to be mounted, and (b) race
    // with React StrictMode's intentional mount → unmount → mount
    // cycle, causing the renderer to be re-initialized on every
    // mount even though `init` is itself idempotent.
    return () => {
      cancelled = true;
      if (rafHandle !== null) cancelAnimationFrame(rafHandle);
    };
    // Empty deps: this effect owns the lifecycle of the rAF loop. Size
    // changes are handled by the separate resize effect below.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Resize: width/height changes drive `bridge.resize(...)`, which is a
  // no-op on the GPU device (only the offscreen surface and presenter
  // buffers are reallocated) and matches the canvas backing store. The
  // rAF loop above picks up the new ImageData on its next frame.
  useEffect(() => {
    const bridge = window.kcreate?.renderer;
    if (!bridge) return;
    const dpr = dprRef.current;
    const wPx = Math.max(1, Math.round(props.width * dpr));
    const hPx = Math.max(1, Math.round(props.height * dpr));
    void bridge.resize(wPx, hPx).catch(() => {
      /* renderer may not be initialized yet; init effect will catch up */
    });
    const canvas = canvasRef.current;
    if (canvas && ctxRef.current) {
      canvas.width = wPx;
      canvas.height = hPx;
      imageDataRef.current = ctxRef.current.createImageData(wPx, hPx);
      imageDataDimsRef.current = { w: wPx, h: hPx };
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
