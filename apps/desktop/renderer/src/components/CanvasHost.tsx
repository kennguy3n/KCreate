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

import { useCallback, useEffect, useRef, useState } from "react";

import type { Scene } from "../../../shared/scene";

export interface ViewportState {
  panX: number;
  panY: number;
  zoom: number;
}

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
  viewport?: ViewportState;
  /**
   * Called when the user pans/zooms the canvas. The host should
   * propagate the value back through `viewport` to apply it.
   */
  onViewportChange?: (next: ViewportState) => void;
  /**
   * Called when the user double-clicks the canvas. Hosts typically use
   * this to recompute a zoom-to-fit viewport from the document bounds.
   * The point is in CSS pixels relative to the canvas.
   */
  onZoomToFit?: () => void;
  /**
   * Called whenever a new frame is presented to the canvas. Useful for
   * FPS readouts in the host UI.
   */
  onFramePresented?: (frameId: number) => void;
  /**
   * Forwarded pointer events. The host typically uses these to drive
   * tool state (selection rectangle, drag, etc.). The CanvasHost
   * still intercepts middle-button + Space+drag for panning; it
   * forwards the events afterwards so the host can layer its own
   * tools on top.
   */
  onPointer?: (event: React.PointerEvent<HTMLCanvasElement>) => void;
  /**
   * Optional canvas cursor style override (e.g. "crosshair" while the
   * rect tool is active).
   */
  cursor?: string;
}

const MIN_ZOOM = 0.1;
const MAX_ZOOM = 32;
/// Mouse-wheel sensitivity. Each line-event multiplies the zoom by
/// `exp(-deltaY × this)` so vertical scroll up zooms in. Empirically
/// chosen to feel close to Figma/Sketch at standard wheel granularity.
const WHEEL_ZOOM_STEP = 0.0025;

type Viewport = ViewportState;

const ZERO_VIEWPORT: Viewport = { panX: 0, panY: 0, zoom: 1 };

function viewportEquals(a: Viewport, b: Viewport): boolean {
  return a.panX === b.panX && a.panY === b.panY && a.zoom === b.zoom;
}

function clampZoom(z: number): number {
  if (!Number.isFinite(z)) return 1;
  return Math.max(MIN_ZOOM, Math.min(MAX_ZOOM, z));
}

export function CanvasHost(props: CanvasHostProps): JSX.Element {
  // Destructure callbacks so the useCallback deps below don't have to
  // depend on the whole `props` object (which the eslint
  // react-hooks/exhaustive-deps rule flags as too coarse).
  const {
    width: propWidth,
    height: propHeight,
    viewport: propViewport,
    onViewportChange,
    onZoomToFit,
    onPointer,
    cursor: propCursor,
  } = props;
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

  // Pan / zoom interaction state. Tracked in refs so the handlers
  // close over them without recreating on every render — pointer/wheel
  // events fire at potentially hundreds of Hz and we don't want
  // React to allocate fresh closures for each one.
  const spacePressedRef = useRef(false);
  const panStateRef = useRef<{
    pointerId: number;
    startX: number;
    startY: number;
    originPanX: number;
    originPanY: number;
  } | null>(null);
  const [cursor, setCursor] = useState<string | null>(null);

  // Track Space-key state on the window — the canvas only gets keyboard
  // events when it has focus, which it usually doesn't. The host
  // EditorPage handles tool shortcuts, but Space-as-pan-modifier is
  // local to the canvas surface so we keep the listener here.
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent): void => {
      if (e.code === "Space" && !spacePressedRef.current) {
        spacePressedRef.current = true;
        // Only show the grab cursor when the user is actively hovering
        // the canvas — but cheap to set unconditionally.
        setCursor("grab");
      }
    };
    const onKeyUp = (e: KeyboardEvent): void => {
      if (e.code === "Space") {
        spacePressedRef.current = false;
        if (!panStateRef.current) setCursor(null);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("keyup", onKeyUp);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("keyup", onKeyUp);
    };
  }, []);

  const emitViewport = useCallback(
    (next: Viewport) => {
      const last = propViewport ?? ZERO_VIEWPORT;
      if (viewportEquals(last, next)) return;
      onViewportChange?.(next);
    },
    [onViewportChange, propViewport],
  );

  const onWheel = useCallback(
    (e: React.WheelEvent<HTMLCanvasElement>) => {
      // Ctrl+wheel and pinch-to-zoom both surface here. We treat any
      // wheel event as a zoom toward the cursor; horizontal-only
      // trackpad scroll still pans the cursor anchor naturally because
      // deltaY is zero there.
      e.preventDefault();
      const canvas = canvasRef.current;
      if (!canvas) return;
      const rect = canvas.getBoundingClientRect();
      const px = e.clientX - rect.left;
      const py = e.clientY - rect.top;
      const cur = viewportRef.current;
      // exp(-deltaY × step) is monotonic, exactly 1 at deltaY=0, and
      // composes correctly for repeated wheel ticks (two ticks of half
      // step ≡ one tick of full step).
      const factor = Math.exp(-e.deltaY * WHEEL_ZOOM_STEP);
      const nextZoom = clampZoom(cur.zoom * factor);
      if (nextZoom === cur.zoom) return;
      // Keep the world-space point under the cursor stationary. world
      // = (screen - pan) / zoom; we want world to stay the same after
      // updating zoom, so pan' = screen - world × zoom'.
      const worldX = (px - cur.panX) / cur.zoom;
      const worldY = (py - cur.panY) / cur.zoom;
      const nextPanX = px - worldX * nextZoom;
      const nextPanY = py - worldY * nextZoom;
      emitViewport({ panX: nextPanX, panY: nextPanY, zoom: nextZoom });
    },
    [emitViewport],
  );

  const onPointerDown = useCallback(
    (e: React.PointerEvent<HTMLCanvasElement>) => {
      // Middle-click or Space+left-click begins a pan. We capture the
      // pointer so we keep receiving move events even if the cursor
      // leaves the canvas mid-drag.
      const isMiddle = e.button === 1;
      const isSpaceDrag = e.button === 0 && spacePressedRef.current;
      if (isMiddle || isSpaceDrag) {
        e.preventDefault();
        e.currentTarget.setPointerCapture(e.pointerId);
        const cur = viewportRef.current;
        panStateRef.current = {
          pointerId: e.pointerId,
          startX: e.clientX,
          startY: e.clientY,
          originPanX: cur.panX,
          originPanY: cur.panY,
        };
        setCursor("grabbing");
        return;
      }
      onPointer?.(e);
    },
    [onPointer],
  );

  const onPointerMove = useCallback(
    (e: React.PointerEvent<HTMLCanvasElement>) => {
      const pan = panStateRef.current;
      if (pan && pan.pointerId === e.pointerId) {
        const dx = e.clientX - pan.startX;
        const dy = e.clientY - pan.startY;
        const cur = viewportRef.current;
        emitViewport({
          panX: pan.originPanX + dx,
          panY: pan.originPanY + dy,
          zoom: cur.zoom,
        });
        return;
      }
      onPointer?.(e);
    },
    [emitViewport, onPointer],
  );

  const onPointerUp = useCallback(
    (e: React.PointerEvent<HTMLCanvasElement>) => {
      const pan = panStateRef.current;
      if (pan && pan.pointerId === e.pointerId) {
        try {
          e.currentTarget.releasePointerCapture(e.pointerId);
        } catch {
          // capture may already be released if the pointer left the
          // window; ignore.
        }
        panStateRef.current = null;
        setCursor(spacePressedRef.current ? "grab" : null);
        return;
      }
      onPointer?.(e);
    },
    [onPointer],
  );

  const onDoubleClick = useCallback(
    (e: React.MouseEvent<HTMLCanvasElement>) => {
      // Double-click on empty canvas resets to fit; the host wires the
      // bounds calculation since the document graph lives in Rust.
      e.preventDefault();
      onZoomToFit?.();
    },
    [onZoomToFit],
  );

  // Suppress the browser's context menu on right/middle click so
  // middle-drag-pan and future right-click context menus don't get
  // hijacked by the platform menu.
  const onContextMenu = useCallback((e: React.MouseEvent) => {
    e.preventDefault();
  }, []);

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
        width: propWidth,
        height: propHeight,
        display: "block",
        cursor: cursor ?? propCursor ?? "default",
        touchAction: "none",
      }}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerLeave={onPointerUp}
      onWheel={onWheel}
      onDoubleClick={onDoubleClick}
      onContextMenu={onContextMenu}
    />
  );
}
