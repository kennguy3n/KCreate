// Phase 9 Block D Task 22 — Configurable grid overlay.
//
// Reads per-artboard grid settings via
// `window.kcreate.phase9.artboardGridSettings` and draws an evenly-
// spaced grid over the canvas in the viewport space. Subdivisions
// are drawn at half opacity. Toggling visibility is handled by the
// parent (a keyboard shortcut and a menu item), so this component
// just renders nothing when the artboard's grid is disabled.

import { useEffect, useState } from "react";
import type { GridSettingsInfo } from "../../../shared/scene";

interface GridOverlayProps {
  artboardId: string | null;
  /** Viewport pan offset (document → screen) in CSS pixels. */
  panX: number;
  panY: number;
  /** Viewport zoom factor (document → screen). */
  zoom: number;
  /** Width / height of the viewport's CSS area. */
  width: number;
  height: number;
  /** Caller-controlled visibility (e.g. bound to Ctrl+'). */
  visible: boolean;
}

export function GridOverlay({
  artboardId,
  panX,
  panY,
  zoom,
  width,
  height,
  visible,
}: GridOverlayProps): JSX.Element | null {
  const [settings, setSettings] = useState<GridSettingsInfo | null>(null);

  useEffect(() => {
    if (artboardId === null) {
      setSettings(null);
      return;
    }
    let cancelled = false;
    void window.kcreate.phase9
      .artboardGridSettings(artboardId)
      .then((s) => {
        if (!cancelled) setSettings(s);
      })
      .catch(() => {
        // Missing grid settings → no overlay. Errors are swallowed
        // because the grid is a pure visual aid; failing should not
        // disturb the editor.
        if (!cancelled) setSettings(null);
      });
    return () => {
      cancelled = true;
    };
  }, [artboardId]);

  if (!visible || settings === null || !settings.enabled || settings.spacing <= 0) {
    return null;
  }

  const majorPx = settings.spacing * zoom;
  if (majorPx < 4) {
    // Spacing collapses to noise at extreme zoom-out. Skip drawing
    // rather than producing a moiré pattern.
    return null;
  }
  const subPx = settings.subdivisions > 1
    ? majorPx / settings.subdivisions
    : null;

  const startX = mod(panX, majorPx);
  const startY = mod(panY, majorPx);
  const majorColor = settings.color || "#444a55";
  const subColor = withAlpha(majorColor, 0.4);

  const lines: JSX.Element[] = [];
  // Vertical major lines.
  for (let x = startX; x < width; x += majorPx) {
    lines.push(
      <line
        key={`vmaj-${x}`}
        x1={x}
        x2={x}
        y1={0}
        y2={height}
        stroke={majorColor}
        strokeWidth={1}
      />,
    );
  }
  // Horizontal major lines.
  for (let y = startY; y < height; y += majorPx) {
    lines.push(
      <line
        key={`hmaj-${y}`}
        x1={0}
        x2={width}
        y1={y}
        y2={y}
        stroke={majorColor}
        strokeWidth={1}
      />,
    );
  }
  if (subPx !== null) {
    for (let x = startX % subPx; x < width; x += subPx) {
      lines.push(
        <line
          key={`vsub-${x}`}
          x1={x}
          x2={x}
          y1={0}
          y2={height}
          stroke={subColor}
          strokeWidth={1}
        />,
      );
    }
    for (let y = startY % subPx; y < height; y += subPx) {
      lines.push(
        <line
          key={`hsub-${y}`}
          x1={0}
          x2={width}
          y1={y}
          y2={y}
          stroke={subColor}
          strokeWidth={1}
        />,
      );
    }
  }

  return (
    <svg
      width={width}
      height={height}
      style={{
        position: "absolute",
        inset: 0,
        pointerEvents: "none",
      }}
      role="img"
      aria-label="Pixel grid overlay"
      data-testid="kcreate-grid-overlay"
    >
      {lines}
    </svg>
  );
}

function mod(a: number, b: number): number {
  const r = a % b;
  return r < 0 ? r + b : r;
}

function withAlpha(hex: string, alpha: number): string {
  // Accepts "#RRGGBB" and returns rgba(). Falls back to rgba(0,0,0,a)
  // when the input is malformed.
  const m = /^#([0-9a-f]{6})$/iu.exec(hex);
  if (!m) return `rgba(0,0,0,${alpha})`;
  const v = m[1] ?? "";
  const r = parseInt(v.slice(0, 2), 16);
  const g = parseInt(v.slice(2, 4), 16);
  const b = parseInt(v.slice(4, 6), 16);
  return `rgba(${r},${g},${b},${alpha})`;
}
