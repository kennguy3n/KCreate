/**
 * ResponsivePreview — three side-by-side mini-canvases showing the
 * current renderer output at desktop, tablet, and mobile breakpoints.
 *
 * Phase 1 scope: this is a **scaled** preview, not a reflowed one.
 * The renderer is single-pass and does not yet redo layout for a
 * smaller viewport, so each panel shows the *same* pixel buffer
 * downscaled to the breakpoint width. Phase 2 will wire each panel
 * to its own renderer pipeline so children can actually reflow.
 *
 * The component pulls the latest frame via `renderer.acquireFrame()`
 * on mount + on a 1Hz timer, then paints each breakpoint to a
 * dedicated `<canvas>` via `putImageData` + transform-scale. We
 * deliberately read the frame from the bridge instead of
 * `document.querySelector` on the live canvas because the live
 * canvas lives in a sibling subtree that may not be mounted when
 * the user switches to "Prototype" mode.
 */
import { useEffect, useRef, useState } from "react";

import { colors, radius, spacing } from "../styles/tokens";

interface Breakpoint {
  id: "desktop" | "tablet" | "mobile";
  label: string;
  width: number; // device-independent pixels, matches PROPOSAL.md §4.2 spec
}

const BREAKPOINTS: ReadonlyArray<Breakpoint> = [
  { id: "desktop", label: "Desktop", width: 1440 },
  { id: "tablet", label: "Tablet", width: 768 },
  { id: "mobile", label: "Mobile", width: 375 },
];

/**
 * Pixel cap for the preview thumbnails. The breakpoint widths above
 * are *spec* widths; we render each thumbnail at min(spec_width,
 * THUMBNAIL_MAX_CSS) CSS pixels so three frames always fit
 * side-by-side inside the prototype canvas pane without horizontal
 * scrolling at typical editor window sizes.
 */
const THUMBNAIL_MAX_CSS = 360;

interface ResponsivePreviewProps {
  /** Optional callback for surfacing errors in the status bar. */
  onStatus?: (msg: string) => void;
}

export function ResponsivePreview({
  onStatus,
}: ResponsivePreviewProps): JSX.Element {
  const desktopRef = useRef<HTMLCanvasElement | null>(null);
  const tabletRef = useRef<HTMLCanvasElement | null>(null);
  const mobileRef = useRef<HTMLCanvasElement | null>(null);
  const [frameMeta, setFrameMeta] = useState<{
    width: number;
    height: number;
    frameId: number;
  } | null>(null);

  useEffect(() => {
    let cancelled = false;
    let lastFrameId = -1;

    const paint = async (): Promise<void> => {
      try {
        const frame = await window.kcreate.renderer.acquireFrame();
        if (cancelled || !frame) return;
        if (frame.frameId === lastFrameId) return;
        lastFrameId = frame.frameId;
        setFrameMeta({
          width: frame.width,
          height: frame.height,
          frameId: frame.frameId,
        });
        // Paint the same RGBA buffer to all three thumbnails. The
        // canvas's intrinsic width/height stays at the source
        // resolution; CSS scales the element down to the breakpoint
        // width. This keeps the thumbnail crisp at HiDPI without
        // shipping three GPU readbacks per frame.
        const buffer = new Uint8ClampedArray(
          frame.bytes.buffer,
          frame.bytes.byteOffset,
          frame.bytes.byteLength,
        );
        const image = new ImageData(buffer, frame.width, frame.height);
        for (const ref of [desktopRef, tabletRef, mobileRef]) {
          const canvas = ref.current;
          if (!canvas) continue;
          canvas.width = frame.width;
          canvas.height = frame.height;
          const ctx = canvas.getContext("2d");
          if (!ctx) continue;
          ctx.putImageData(image, 0, 0);
        }
      } catch (e) {
        if (cancelled) return;
        const msg = e instanceof Error ? e.message : String(e);
        onStatus?.(`Responsive preview: ${msg}`);
      }
    };

    void paint();
    const handle = window.setInterval(() => {
      void paint();
    }, 1000);
    return (): void => {
      cancelled = true;
      window.clearInterval(handle);
    };
  }, [onStatus]);

  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        gap: spacing.md,
        padding: spacing.md,
        background: colors.bgSoft,
        borderRadius: radius.card,
        height: "100%",
        overflow: "auto",
      }}
    >
      <div
        style={{
          display: "flex",
          gap: spacing.sm,
          alignItems: "baseline",
        }}
      >
        <h2 style={{ margin: 0, fontSize: 14, color: colors.text }}>
          Responsive preview
        </h2>
        <span style={{ fontSize: 11, color: colors.textMuted }}>
          Phase 1: scaled snapshot at three breakpoints. Phase 2 will
          add per-breakpoint reflow.
        </span>
      </div>
      <div
        style={{
          display: "flex",
          gap: spacing.md,
          alignItems: "flex-start",
          flexWrap: "wrap",
        }}
      >
        {BREAKPOINTS.map((bp) => (
          <BreakpointFrame
            key={bp.id}
            breakpoint={bp}
            canvasRef={
              bp.id === "desktop"
                ? desktopRef
                : bp.id === "tablet"
                  ? tabletRef
                  : mobileRef
            }
            sourceWidth={frameMeta?.width ?? null}
            sourceHeight={frameMeta?.height ?? null}
          />
        ))}
      </div>
    </div>
  );
}

function BreakpointFrame({
  breakpoint,
  canvasRef,
  sourceWidth,
  sourceHeight,
}: {
  breakpoint: Breakpoint;
  canvasRef: React.MutableRefObject<HTMLCanvasElement | null>;
  sourceWidth: number | null;
  sourceHeight: number | null;
}): JSX.Element {
  const cssWidth = Math.min(breakpoint.width, THUMBNAIL_MAX_CSS);
  // Pick a thumbnail height that preserves the source aspect ratio.
  // If we have no frame yet, fall back to a 16:9 placeholder so the
  // panel doesn't collapse to 0px tall.
  const aspect =
    sourceWidth && sourceHeight && sourceHeight > 0
      ? sourceWidth / sourceHeight
      : 16 / 9;
  const cssHeight = Math.round(cssWidth / aspect);
  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        gap: spacing.xs,
        alignItems: "flex-start",
      }}
    >
      <div
        style={{
          display: "flex",
          gap: spacing.sm,
          alignItems: "baseline",
        }}
      >
        <span style={{ fontSize: 12, color: colors.text, fontWeight: 600 }}>
          {breakpoint.label}
        </span>
        <span style={{ fontSize: 11, color: colors.textMuted }}>
          {breakpoint.width}px
        </span>
      </div>
      <div
        style={{
          width: cssWidth,
          height: cssHeight,
          background: colors.bgCanvas,
          borderRadius: radius.card / 2,
          border: `1px solid ${colors.border}`,
          overflow: "hidden",
          position: "relative",
        }}
      >
        <canvas
          ref={canvasRef}
          style={{
            width: "100%",
            height: "100%",
            display: "block",
            // CSS image scaling sometimes uses bicubic on Linux,
            // which makes pixel-art thumbnails fuzzy. `crisp-edges`
            // / `pixelated` keeps the downsample honest.
            imageRendering: "auto",
          }}
        />
        {sourceWidth == null ? (
          <div
            style={{
              position: "absolute",
              inset: 0,
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              fontSize: 11,
              color: colors.textMuted,
              background: "rgba(0,0,0,0.35)",
            }}
          >
            no frame
          </div>
        ) : null}
      </div>
    </div>
  );
}
