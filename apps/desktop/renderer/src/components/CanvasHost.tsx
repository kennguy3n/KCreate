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
  /**
   * Show a small performance HUD (fps / frame-time / present payload /
   * node-count) overlaid on the canvas, fed by numbers measured live in
   * the present loop. Off by default; when `false` the measurement code
   * is skipped entirely so it adds zero per-frame overhead.
   */
  perfHud?: boolean;
}

/** Live present-loop metrics surfaced by the optional perf HUD. */
interface PerfHudStats {
  /** Exponentially-smoothed frames presented per second. */
  fps: number;
  /** Exponentially-smoothed host present-work time per frame, in ms. */
  frameMs: number;
  /** Bytes the last present shipped across IPC (dirty sub-rect or full). */
  bytes: number;
  /** Whether the last present was a full frame, a partial, or idle. */
  kind: "full" | "partial" | "idle";
  /** Node count of the current document (throttled document-status poll). */
  nodeCount: number;
}

/** Smoothing factor for the HUD's fps / frame-time EWMAs. */
const PERF_HUD_EWMA = 0.1;
/** Minimum interval between HUD React state publishes, in ms (~6 Hz). */
const PERF_HUD_PUBLISH_MS = 160;
/** Minimum interval between document-status node-count polls, in ms. */
const PERF_HUD_NODECOUNT_MS = 1000;

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
    perfHud = false,
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

  // Perf HUD. `perfHudRef` lets the rAF loop (set up once on mount)
  // read the live toggle without re-running its effect, and
  // `perfStatsRef` accumulates the measured numbers imperatively so a
  // 60 Hz present loop never triggers a React re-render — we publish to
  // `hudStats` state at most every `PERF_HUD_PUBLISH_MS`.
  // `perfHud` is the host's requested default; the user can also flip
  // the overlay locally with Ctrl/Cmd+Shift+P (matching the app's
  // panel-toggle shortcut convention). `null` means "follow the prop".
  const [hudOverride, setHudOverride] = useState<boolean | null>(null);
  const hudEnabled = hudOverride ?? perfHud;
  const perfHudRef = useRef(false);
  perfHudRef.current = hudEnabled;
  const perfStatsRef = useRef<{
    fps: number;
    frameMs: number;
    lastTickTs: number;
    lastPublishTs: number;
    bytes: number;
    kind: "full" | "partial" | "idle";
    nodeCount: number;
    lastNodeCountTs: number;
  }>({
    fps: 0,
    frameMs: 0,
    lastTickTs: 0,
    lastPublishTs: 0,
    bytes: 0,
    kind: "idle",
    nodeCount: 0,
    lastNodeCountTs: 0,
  });
  const [hudStats, setHudStats] = useState<PerfHudStats | null>(null);

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

    // Record what the last present shipped, for the perf HUD. Cheap and
    // only called from the offscreen present path; the HUD-enabled
    // branch reads it on the throttled publish below.
    const recordPresentMetrics = (
      bytes: number,
      kind: "full" | "partial" | "idle",
    ): void => {
      if (!perfHudRef.current) return;
      const s = perfStatsRef.current;
      s.bytes = bytes;
      s.kind = kind;
    };

    // Fold this tick's measured cadence + present-work time into the HUD
    // EWMAs, refresh the throttled node-count poll, and publish to React
    // state at most ~6 Hz so the overlay never re-renders the component
    // at the present loop's rate. `tickStart` is a `performance.now()`
    // sample taken at the top of the tick.
    const now = (): number =>
      typeof performance !== "undefined" ? performance.now() : Date.now();
    const updatePerfHud = (tickStart: number): void => {
      const t = now();
      const s = perfStatsRef.current;
      if (s.lastTickTs > 0) {
        const dt = t - s.lastTickTs;
        if (dt > 0) {
          const inst = 1000 / dt;
          s.fps = s.fps > 0 ? s.fps + PERF_HUD_EWMA * (inst - s.fps) : inst;
        }
      }
      s.lastTickTs = t;
      const work = t - tickStart;
      if (work >= 0) {
        s.frameMs =
          s.frameMs > 0 ? s.frameMs + PERF_HUD_EWMA * (work - s.frameMs) : work;
      }
      if (t - s.lastNodeCountTs >= PERF_HUD_NODECOUNT_MS) {
        s.lastNodeCountTs = t;
        const doc = window.kcreate?.document;
        if (doc) {
          void doc
            .status()
            .then((status) => {
              if (status) perfStatsRef.current.nodeCount = status.nodeCount;
            })
            .catch(() => {
              /* node count is best-effort decoration for the HUD */
            });
        }
      }
      if (t - s.lastPublishTs >= PERF_HUD_PUBLISH_MS) {
        s.lastPublishTs = t;
        setHudStats({
          fps: s.fps,
          frameMs: s.frameMs,
          bytes: s.bytes,
          kind: s.kind,
          nodeCount: s.nodeCount,
        });
      }
    };

    // Dirty-rect present: pull only the pixels that changed since the
    // last present and blit just that sub-rect. The persistent
    // `imageDataRef` backbuffer accumulates the full frame across
    // presents, so a partial update patches only the changed rows into
    // it and repaints a single sub-rectangle. Returns the frame id we
    // actually consumed (it may be newer than `fallbackId` if the
    // renderer advanced between the id poll and this call).
    const presentDirtyRect = async (
      ensure: (w: number, h: number) => void,
      fallbackId: number,
    ): Promise<number> => {
      const present = await bridge.acquirePresent();
      if (!present) return fallbackId;

      const {
        width,
        height,
        dirtyX,
        dirtyY,
        dirtyWidth,
        dirtyHeight,
        full,
        bytes,
      } = present;

      if (full) {
        // Whole-frame present: first frame, post-resize, or a change
        // large enough that a partial blit no longer pays for itself.
        if (bytes.byteLength === width * height * 4) {
          ensure(width, height);
          imageDataRef.current!.data.set(bytes);
          ctx.putImageData(imageDataRef.current!, 0, 0);
          recordPresentMetrics(bytes.byteLength, "full");
        }
      } else if (dirtyWidth > 0 && dirtyHeight > 0) {
        // Partial present: patch the changed rows into the persistent
        // backbuffer, then repaint only the dirty sub-rect via the
        // dirty-region form of `putImageData`.
        const img = imageDataRef.current;
        const dims = imageDataDimsRef.current;
        const expected = dirtyWidth * dirtyHeight * 4;
        if (
          img &&
          dims.w === width &&
          dims.h === height &&
          bytes.byteLength === expected
        ) {
          const data = img.data;
          const fullStride = width * 4;
          const rowBytes = dirtyWidth * 4;
          for (let row = 0; row < dirtyHeight; row += 1) {
            const srcStart = row * rowBytes;
            const destStart = (dirtyY + row) * fullStride + dirtyX * 4;
            data.set(bytes.subarray(srcStart, srcStart + rowBytes), destStart);
          }
          ctx.putImageData(img, 0, 0, dirtyX, dirtyY, dirtyWidth, dirtyHeight);
          recordPresentMetrics(expected, "partial");
        }
        // If the backbuffer isn't yet sized to this frame the partial is
        // skipped; the renderer forces a full present after any resize,
        // so the next tick resynchronises.
      } else {
        // dirtyWidth/Height === 0: the frame id advanced but the pixels
        // are identical to what we already show — nothing to blit.
        recordPresentMetrics(0, "idle");
      }

      return present.frameId;
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
        const hudOn = perfHudRef.current;
        const tickStart = hudOn ? now() : 0;
        try {
          // Resolve the latest published frame id for this tick. There
          // are two ways it advances:
          //
          //   1. The host changed the viewport (pan/zoom). We fold the
          //      viewport write and the repaint into a SINGLE IPC round
          //      trip via `setViewportAndRender`, which returns the new
          //      frame id directly — so on the pan/zoom hot path we skip
          //      the separate `frameInfo()` poll below. This halves the
          //      bridge crossings versus the old
          //      `setViewport` + `renderCurrent` + `frameInfo` sequence.
          //
          //   2. The document mutated (the bridge invalidates +
          //      re-renders on each scene sync). On a static-viewport
          //      tick we learn about that with the cheap `frameInfo()`
          //      metadata poll, so a still canvas costs exactly one
          //      metadata call per rAF tick and zero pixel copies.
          let latestFrameId: number | null = null;
          const vp = viewportRef.current;
          const last = lastSentViewportRef.current;
          if (!last || !viewportEquals(last, vp)) {
            latestFrameId = await bridge.setViewportAndRender(
              vp.panX,
              vp.panY,
              vp.zoom,
            );
            lastSentViewportRef.current = { ...vp };
          }
          // Static-viewport tick (or the combined call reported no scene
          // yet): poll the latest published id so document mutations
          // still present.
          if (latestFrameId === null) {
            const info = await bridge.frameInfo();
            latestFrameId = info ? info.frameId : null;
          }

          // Only pay for a pixel readback when the frame actually
          // advanced. On a typical edit the dirty-rect present path ships
          // just the changed sub-region (`dirtyWidth × dirtyHeight × 4`
          // bytes) instead of the whole framebuffer.
          if (latestFrameId !== null && latestFrameId !== lastFrameIdRef.current) {
            // Default to the id we resolved above. In offscreen mode we
            // replace this with the id of the frame we actually acquired
            // below, which may be newer.
            let presentedId = latestFrameId;
            // In native mode the Rust side has already presented the
            // frame directly to the platform window surface — there
            // is nothing for the host to do beyond bookkeeping. Skip
            // the readback + `putImageData` step entirely so the
            // native path stays zero-copy and we do not waste a CPU
            // download on every frame.
            if (activeModeRef.current === "offscreen") {
              presentedId = await presentDirtyRect(ensureImageData, presentedId);
            }
            lastFrameIdRef.current = presentedId;
            props.onFramePresented?.(presentedId);
          }
          if (hudOn) updatePerfHud(tickStart);
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

  // Drop the overlay's last snapshot when the HUD is switched off so a
  // stale frame doesn't linger on the next enable.
  useEffect(() => {
    if (!hudEnabled) {
      perfStatsRef.current.lastTickTs = 0;
      setHudStats(null);
    }
  }, [hudEnabled]);

  // Ctrl/Cmd+Shift+P toggles the perf HUD locally. Registered at the
  // window level (the canvas rarely holds focus) and scoped tightly to
  // that exact chord so it never shadows a typing or tool shortcut.
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent): void => {
      if (
        (e.ctrlKey || e.metaKey) &&
        e.shiftKey &&
        !e.altKey &&
        (e.code === "KeyP" || e.key === "P" || e.key === "p")
      ) {
        e.preventDefault();
        setHudOverride((prev) => !(prev ?? perfHud));
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
    };
  }, [perfHud]);

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
    <>
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
        data-testid="kcreate-canvas-surface"
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerLeave={onPointerUp}
        onWheel={onWheel}
        onDoubleClick={onDoubleClick}
        onContextMenu={onContextMenu}
      />
      {hudEnabled && hudStats ? <PerfHud stats={hudStats} /> : null}
    </>
  );
}

/** Number of bytes in a kibibyte, for the HUD payload readout. */
const KIB = 1024;

/**
 * Small, professional performance overlay. Absolutely positioned in the
 * canvas pane's top-right corner, non-interactive (`pointerEvents:
 * none`), and fed entirely by numbers measured in the present loop.
 */
function PerfHud({ stats }: { stats: PerfHudStats }): JSX.Element {
  const payload =
    stats.kind === "idle"
      ? "idle"
      : `${(stats.bytes / KIB).toFixed(1)} KiB ${stats.kind}`;
  const rows: Array<[string, string]> = [
    ["FPS", stats.fps.toFixed(0)],
    ["Frame", `${stats.frameMs.toFixed(2)} ms`],
    ["Present", payload],
    ["Nodes", stats.nodeCount.toLocaleString()],
  ];
  return (
    <div
      data-testid="kcreate-perf-hud"
      style={{
        position: "absolute",
        top: 10,
        right: 10,
        zIndex: 20,
        pointerEvents: "none",
        userSelect: "none",
        padding: "8px 10px",
        borderRadius: 8,
        background: "rgba(17, 19, 24, 0.82)",
        border: "1px solid rgba(255, 255, 255, 0.08)",
        boxShadow: "0 2px 10px rgba(0, 0, 0, 0.35)",
        backdropFilter: "blur(6px)",
        color: "#e8eaed",
        font: '11px / 1.45 ui-monospace, SFMono-Regular, "SF Mono", Menlo, Consolas, monospace',
        letterSpacing: 0.2,
        minWidth: 132,
      }}
    >
      <div
        style={{
          fontSize: 9,
          letterSpacing: 1.2,
          textTransform: "uppercase",
          color: "#8a8f98",
          marginBottom: 4,
        }}
      >
        Performance
      </div>
      {rows.map(([label, value]) => (
        <div
          key={label}
          style={{
            display: "flex",
            justifyContent: "space-between",
            gap: 16,
          }}
        >
          <span style={{ color: "#9aa0aa" }}>{label}</span>
          <span style={{ color: "#e8eaed", fontVariantNumeric: "tabular-nums" }}>
            {value}
          </span>
        </div>
      ))}
    </div>
  );
}
