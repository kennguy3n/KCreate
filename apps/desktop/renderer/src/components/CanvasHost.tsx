// CanvasHost — renderer-side presentation surface.
//
// This component does NOT render the scene itself. The Rust
// `kcreate_renderer` crate owns the entire rendering pipeline and the
// bridge owns the document graph, so the scene never travels back
// through JS. This component is purely a presentation surface:
//   1. Owns an HTML `<canvas>` of the editor viewport dimensions.
//   2. On every `requestAnimationFrame`, polls the renderer's cheap
//      frame metadata (`frameInfo()`) over the preload-exposed IPC
//      bridge and only pulls the full pixel buffer (`acquireFrame()`)
//      when the published frame id actually advanced.
//   3. Writes the returned RGBA8 pixel buffer to the canvas via
//      `putImageData`.
//   4. On a viewport (pan/zoom) or resize change, asks the renderer to
//      repaint the current document via `renderCurrent()` — those
//      operations mark the renderer dirty but do not by themselves
//      rebuild a frame.
//   5. Forwards pointer / keyboard events to the bridge so Rust can do
//      hit testing and tool state.
//
// Phase 1, Block A, Task 6 added a dual-mode surface: when `mode` is
// `"native"`, the Rust renderer composits directly into a platform
// window surface and the readback / `putImageData` step is skipped
// entirely. The `<canvas>` element is still rendered (sized + hit
// targeted) but kept transparent so the underlying native surface
// shows through. Pointer events are still forwarded so tool state
// works identically in both modes.

import { useCallback, useEffect, useRef, useState } from "react";

export interface ViewportState {
  panX: number;
  panY: number;
  zoom: number;
}

/**
 * Presentation strategy the host wants the canvas surface to use.
 *
 * - `"offscreen"`: default. The renderer publishes frames through
 *   `acquireFrame()` and the host draws them into the `<canvas>` via
 *   `putImageData`. Works on every platform; one CPU readback per
 *   frame.
 * - `"native"`: the bridge has been switched to native presentation
 *   via `renderer.switchNative()` and is presenting directly to the
 *   BrowserWindow's underlying surface. The `<canvas>` element stays
 *   in the layout (to receive pointer events) but is rendered
 *   transparent. The rAF readback loop is suspended.
 *
 * If the host requests `"native"` but the bridge rejects the switch
 * (e.g. the binary was built without the `native_canvas` Cargo
 * feature, or surface creation fails), the component falls back to
 * `"offscreen"` and surfaces the reason via `onNativeFallback`.
 */
export type CanvasPresentationMode = "offscreen" | "native";

export interface CanvasHostProps {
  /**
   * Width and height of the canvas in CSS pixels. Multiplied by
   * devicePixelRatio for the actual render target size.
   */
  width: number;
  height: number;
  /**
   * Requested presentation mode. Defaults to `"offscreen"`. Switching
   * to `"native"` triggers a one-time bridge negotiation; switching
   * back to `"offscreen"` reattaches the rAF readback loop.
   */
  mode?: CanvasPresentationMode;
  /**
   * Notified when a requested `mode = "native"` could not be honoured
   * and the component fell back to the offscreen path. Hosts use
   * this to surface a "native canvas unavailable" warning to the
   * user and to clear their settings toggle.
   */
  onNativeFallback?: (reason: string) => void;
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
    mode: requestedMode,
    onNativeFallback,
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
  const viewportRef = useRef<Viewport>(props.viewport ?? ZERO_VIEWPORT);
  const dprRef = useRef<number>(
    typeof window === "undefined" ? 1 : window.devicePixelRatio || 1,
  );
  const [initError, setInitError] = useState<string | null>(null);
  // Effective presentation mode after negotiation with the bridge.
  // `requestedMode` is the host's preference; this is what the
  // component is actually presenting in. Falls back to `"offscreen"`
  // whenever the native path is requested but unavailable.
  const [activeMode, setActiveMode] = useState<CanvasPresentationMode>(
    "offscreen",
  );
  const activeModeRef = useRef<CanvasPresentationMode>("offscreen");
  activeModeRef.current = activeMode;

  // Keep refs up to date on every render.
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

      // Present whatever the renderer has already published for the
      // current document at this freshly-initialised size + viewport.
      // If a project was opened before this surface mounted, its frame
      // shows immediately; if nothing has been rendered yet,
      // `renderCurrent()` resolves to null and the first document sync
      // drives the first frame. A transient failure here must not abort
      // the present loop, so it is best-effort.
      try {
        await bridge.renderCurrent();
      } catch (err) {
        console.warn("kcreate initial renderCurrent failed", err);
      }

      const tick = async (): Promise<void> => {
        if (cancelled) return;
        try {
          // Sync the viewport only when the host actually changed it,
          // then ask the renderer to repaint the current document at the
          // new viewport. A pan/zoom marks the renderer dirty but does
          // not by itself rebuild a frame, so without this the canvas
          // would not follow the viewport. Skipping the IPC round-trip
          // when nothing changed keeps the steady-state (static
          // viewport) case free.
          const vp = viewportRef.current;
          const last = lastSentViewportRef.current;
          if (!last || !viewportEquals(last, vp)) {
            await bridge.setViewport(vp.panX, vp.panY, vp.zoom);
            lastSentViewportRef.current = { ...vp };
            await bridge.renderCurrent();
          }

          // Present path. Poll the latest published frame id with the
          // cheap `frameInfo()` metadata call and only pay for the full
          // pixel readback (`acquireFrame`, ~W×H×4 bytes across IPC)
          // when the frame actually advanced. The renderer publishes a
          // new frame whenever the document mutates (the bridge
          // invalidates + re-renders on each scene sync) or when
          // `renderCurrent()` runs above for a viewport / resize change,
          // so a static document costs one metadata poll per rAF tick
          // and zero pixel copies.
          const info = await bridge.frameInfo();
          if (info && info.frameId !== lastFrameIdRef.current) {
            // In native mode the Rust side has already presented the
            // frame directly to the platform window surface — there
            // is nothing for the host to do beyond bookkeeping. Skip
            // the readback + `putImageData` step entirely so the
            // native path stays zero-copy and we do not waste a CPU
            // download on every frame.
            if (activeModeRef.current === "offscreen") {
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
            }
            lastFrameIdRef.current = info.frameId;
            props.onFramePresented?.(info.frameId);
          }
        } catch (err) {
          // Surfacing the error to React state would unmount the
          // component; that's heavy-handed for a transient present
          // failure. Log it for the devtools and try again next frame.
          console.warn("kcreate present tick failed", err);
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

  // Negotiate the presentation mode whenever the host's preference
  // changes (Phase 1, Block A, Task 6). The request is asynchronous
  // because attaching a native surface requires the bridge to extract
  // the platform window handle from the main process and ask wgpu to
  // build a swapchain — neither of which is synchronous.
  //
  // Devin Review PR #5 ANALYSIS-0001 (commit 5c16b5c): if `switchNative`
  // ever rejects (e.g. bridge compiled without `native_canvas`, no
  // wgpu adapter for the window's surface, Wayland session not yet
  // wired through), the host stays in `activeMode = "offscreen"` while
  // `requestedMode = "native"`. Without a guard, every subsequent
  // resize would re-fire the effect (`propWidth` / `propHeight` are in
  // the deps array) and retry the same failing call — spamming
  // `onNativeFallback` with the same error and re-running the wgpu
  // surface-creation cost for nothing. `nativeRejectedForRef` records
  // the `requestedMode` value that already failed so we short-circuit
  // until the host itself picks a different preference (toggling back
  // to "offscreen" and then to "native" clears the ref).
  const nativeRejectedForRef = useRef<CanvasPresentationMode | null>(null);
  useEffect(() => {
    const bridge = window.kcreate?.renderer;
    if (!bridge) return undefined;
    const desired: CanvasPresentationMode = requestedMode ?? "offscreen";
    if (desired === activeMode) return undefined;
    if (desired === "native" && nativeRejectedForRef.current === desired) {
      // Already tried this exact request, host hasn't changed its
      // mind. Don't retry; let the resize path drive the offscreen
      // pipeline (which is already running).
      return undefined;
    }

    let cancelled = false;
    void (async (): Promise<void> => {
      if (desired === "native") {
        const dpr = dprRef.current;
        const wPx = Math.max(1, Math.round(propWidth * dpr));
        const hPx = Math.max(1, Math.round(propHeight * dpr));
        try {
          await bridge.switchNative(wPx, hPx);
          if (!cancelled) {
            nativeRejectedForRef.current = null;
            setActiveMode("native");
          }
        } catch (err) {
          // Native path is unavailable — keep the offscreen loop
          // running and let the host clear its toggle.
          if (!cancelled) {
            nativeRejectedForRef.current = desired;
            const reason =
              err instanceof Error
                ? err.message
                : "switchNative failed: " + String(err);
            onNativeFallback?.(reason);
            // Force a re-render on the next paint so the canvas
            // reflects the offscreen state (background, transparency).
            setActiveMode("offscreen");
          }
        }
      } else {
        // Switching away from native clears the rejection record so
        // a subsequent flip back to "native" can try again (the host
        // may have e.g. plugged in a GPU or restarted with the
        // feature flag on).
        nativeRejectedForRef.current = null;
        try {
          await bridge.switchOffscreen();
        } catch {
          // No-op: detaching the native surface is best-effort.
        }
        if (!cancelled) setActiveMode("offscreen");
      }
    })();

    return () => {
      cancelled = true;
    };
    // `activeMode` is intentionally not in the dependency list: it's
    // the result of the negotiation, not an input — including it
    // would re-fire the effect immediately after every successful
    // switch.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [requestedMode, propWidth, propHeight, onNativeFallback]);

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
    void bridge
      .resize(wPx, hPx)
      .then(() => bridge.renderCurrent())
      .catch(() => {
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
        // In native mode the Rust renderer paints straight to the
        // BrowserWindow's surface beneath the React tree. We keep the
        // <canvas> element in the layout so pointer events still route
        // through React (`pointerEvents: "auto"` is the default), but
        // we have to hide its pixel buffer because the 2D context was
        // created with `alpha: false` for the offscreen path — that
        // makes the backing store permanently opaque, and a CSS
        // `background: transparent` only affects the element box, not
        // the canvas bitmap (Devin Review BUG-0003). `opacity: 0` is
        // the cleanest way to keep the element hit-testable while
        // letting the native surface composit underneath it.
        opacity: activeMode === "native" ? 0 : undefined,
        background: activeMode === "native" ? "transparent" : undefined,
      }}
      data-presentation-mode={activeMode}
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
