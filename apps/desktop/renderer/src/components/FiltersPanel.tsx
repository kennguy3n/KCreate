// FiltersPanel — Phase 5 Block B Task 12.
//
// Shown in the right-hand inspector when a `RasterLayer` node is
// selected. Each filter section follows the Ask → Preview → Apply →
// Undo loop:
//
// 1. Sliders mutate local state.
// 2. A 100 ms debounce calls `rasterOps.previewFilter()` to compute
//    the post-filter RGBA buffer.
// 3. The buffer is blitted onto an off-canvas `ImageData` so the user
//    sees the result *without* mutating the document.
// 4. "Apply" commits the filter through the matching `applyXxx`
//    bridge function, which records an undoable `Operation` on the
//    project log. Undo through the canvas undo button reverts the
//    edit identically to every other Phase-1 raster adjustment.
//
// Crop / Rotate / Flip / Heal are direct commits (no live preview —
// they re-tile the grid, so the renderer's main scene sync is the
// preview surface).

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import type {
  NodeInfo,
  RasterBlurKind,
  RasterFlipDirection,
  RasterPreviewFilter,
} from "../../../shared/scene";
import { colors, radius, spacing } from "../styles/tokens";

const PREVIEW_DEBOUNCE_MS = 100;
const PREVIEW_MAX_DIM = 256;

type FilterTab = "levels" | "curves" | "blur" | "sharpen" | "transform";

export interface FiltersPanelProps {
  node: NodeInfo;
  onStatus?: (msg: string | null) => void;
}

export function FiltersPanel({
  node,
  onStatus,
}: FiltersPanelProps): JSX.Element | null {
  const [tab, setTab] = useState<FilterTab>("levels");

  // Levels.
  const [levelsBlack, setLevelsBlack] = useState(0);
  const [levelsWhite, setLevelsWhite] = useState(1);
  const [levelsGamma, setLevelsGamma] = useState(1);

  // Curves. Default identity: two anchors at (0,0) and (1,1).
  const [curvePoints, setCurvePoints] = useState<[number, number][]>([
    [0, 0],
    [1, 1],
  ]);

  // Blur.
  const [blurRadius, setBlurRadius] = useState(2);
  const [blurKind, setBlurKind] = useState<RasterBlurKind>("gaussian");

  // Sharpen.
  const [sharpRadius, setSharpRadius] = useState(2);
  const [sharpAmount, setSharpAmount] = useState(0.5);
  const [sharpThreshold, setSharpThreshold] = useState(0);

  // Transform.
  const [cropX, setCropX] = useState(0);
  const [cropY, setCropY] = useState(0);
  const [cropW, setCropW] = useState(Math.round(node.bounds.width));
  const [cropH, setCropH] = useState(Math.round(node.bounds.height));
  const [rotateDeg, setRotateDeg] = useState(0);

  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const previewCanvasRef = useRef<HTMLCanvasElement | null>(null);
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Reset crop bounds when selection changes.
  useEffect(() => {
    setCropX(0);
    setCropY(0);
    setCropW(Math.round(node.bounds.width));
    setCropH(Math.round(node.bounds.height));
  }, [node.id, node.bounds.width, node.bounds.height]);

  const reportStatus = useCallback(
    (msg: string | null) => {
      onStatus?.(msg);
    },
    [onStatus],
  );

  // Build the current filter wire object for the active tab. `null`
  // means "no preview for this tab" (transform is direct-commit).
  const previewFilter = useMemo<RasterPreviewFilter | null>(() => {
    switch (tab) {
      case "levels":
        return {
          type: "levels",
          black_point: levelsBlack,
          white_point: levelsWhite,
          gamma: levelsGamma,
        };
      case "curves":
        return { type: "curves", points: curvePoints };
      case "blur":
        return {
          type: "blur",
          radius: blurRadius,
          kind: blurKind,
        };
      case "sharpen":
        return {
          type: "sharpen",
          radius: sharpRadius,
          amount: sharpAmount,
          threshold: sharpThreshold,
        };
      case "transform":
        return null;
    }
  }, [
    tab,
    levelsBlack,
    levelsWhite,
    levelsGamma,
    curvePoints,
    blurRadius,
    blurKind,
    sharpRadius,
    sharpAmount,
    sharpThreshold,
  ]);

  // Debounced live preview.
  useEffect(() => {
    if (!previewFilter) {
      return;
    }
    if (debounceRef.current !== null) {
      clearTimeout(debounceRef.current);
    }
    debounceRef.current = setTimeout(() => {
      void (async () => {
        try {
          const rgba = await window.kcreate.rasterOps.previewFilter(
            node.id,
            previewFilter,
          );
          const canvas = previewCanvasRef.current;
          if (!canvas) {
            return;
          }
          // The preview buffer is `node.bounds.width * height * 4` RGBA
          // bytes (matching the raster layer's tile grid resolution).
          // We don't down-sample here — the canvas itself is constrained
          // to PREVIEW_MAX_DIM and the browser scales the image data on
          // paint, which keeps the preview fast even for large layers.
          const w = Math.max(1, Math.round(node.bounds.width));
          const h = Math.max(1, Math.round(node.bounds.height));
          if (rgba.length !== w * h * 4) {
            // Defensive: bridge returned a different resolution than
            // the bounds (e.g. after a crop happened mid-debounce).
            // Skip — the next slider change will refire.
            return;
          }
          canvas.width = w;
          canvas.height = h;
          const ctx = canvas.getContext("2d");
          if (!ctx) {
            return;
          }
          const clamped = new Uint8ClampedArray(rgba.buffer.slice(0));
          ctx.putImageData(new ImageData(clamped, w, h), 0, 0);
        } catch (e) {
          setError(errMsg(e));
        }
      })();
    }, PREVIEW_DEBOUNCE_MS);
    return () => {
      if (debounceRef.current !== null) {
        clearTimeout(debounceRef.current);
        debounceRef.current = null;
      }
    };
  }, [previewFilter, node.id, node.bounds.width, node.bounds.height]);

  const guarded = useCallback(
    async (label: string, fn: () => Promise<void>) => {
      setBusy(true);
      setError(null);
      reportStatus(`${label}…`);
      try {
        await fn();
        reportStatus(`${label} ✓`);
      } catch (e) {
        setError(errMsg(e));
        reportStatus(null);
      } finally {
        setBusy(false);
      }
    },
    [reportStatus],
  );

  const applyLevels = useCallback(() => {
    void guarded("Apply Levels", () =>
      window.kcreate.rasterOps.applyLevels(
        node.id,
        levelsBlack,
        levelsWhite,
        levelsGamma,
      ),
    );
  }, [guarded, node.id, levelsBlack, levelsWhite, levelsGamma]);

  const applyCurves = useCallback(() => {
    void guarded("Apply Curves", () =>
      window.kcreate.rasterOps.applyCurves(node.id, curvePoints),
    );
  }, [guarded, node.id, curvePoints]);

  const applyBlur = useCallback(() => {
    void guarded("Apply Blur", () =>
      window.kcreate.rasterOps.applyBlur(node.id, blurRadius, blurKind),
    );
  }, [guarded, node.id, blurRadius, blurKind]);

  const applySharpen = useCallback(() => {
    void guarded("Apply Sharpen", () =>
      window.kcreate.rasterOps.applySharpen(
        node.id,
        sharpRadius,
        sharpAmount,
        sharpThreshold,
      ),
    );
  }, [guarded, node.id, sharpRadius, sharpAmount, sharpThreshold]);

  const applyCrop = useCallback(() => {
    void guarded("Crop", () =>
      window.kcreate.rasterOps.crop(node.id, cropX, cropY, cropW, cropH),
    );
  }, [guarded, node.id, cropX, cropY, cropW, cropH]);

  const applyRotate = useCallback(() => {
    void guarded("Rotate", () =>
      window.kcreate.rasterOps.rotate(node.id, rotateDeg),
    );
  }, [guarded, node.id, rotateDeg]);

  const applyFlip = useCallback(
    (direction: RasterFlipDirection) => {
      void guarded(`Flip ${direction}`, () =>
        window.kcreate.rasterOps.flip(node.id, direction),
      );
    },
    [guarded, node.id],
  );

  if (node.nodeType !== "RasterLayer") {
    return null;
  }

  return (
    <section
      style={{
        background: colors.bg,
        border: `1px solid ${colors.border}`,
        borderRadius: radius.card,
        padding: spacing.md,
        display: "grid",
        gap: spacing.sm,
      }}
    >
      <header
        style={{
          display: "flex",
          justifyContent: "space-between",
          alignItems: "center",
        }}
      >
        <h3 style={{ margin: 0, fontSize: 14, color: colors.text }}>Filters</h3>
        <span style={{ fontSize: 11, color: colors.textMuted }}>
          Ask → Preview → Apply → Undo
        </span>
      </header>

      <nav
        style={{
          display: "flex",
          gap: spacing.xs,
          flexWrap: "wrap",
        }}
        role="tablist"
      >
        {(
          ["levels", "curves", "blur", "sharpen", "transform"] as FilterTab[]
        ).map((t) => (
          <button
            key={t}
            role="tab"
            aria-selected={tab === t}
            onClick={() => setTab(t)}
            style={tabStyle(tab === t)}
            type="button"
          >
            {tabLabel(t)}
          </button>
        ))}
      </nav>

      {tab === "levels" && (
        <LevelsSection
          black={levelsBlack}
          white={levelsWhite}
          gamma={levelsGamma}
          onBlack={setLevelsBlack}
          onWhite={setLevelsWhite}
          onGamma={setLevelsGamma}
          onApply={applyLevels}
          busy={busy}
        />
      )}
      {tab === "curves" && (
        <CurvesSection
          points={curvePoints}
          onPoints={setCurvePoints}
          onApply={applyCurves}
          busy={busy}
        />
      )}
      {tab === "blur" && (
        <BlurSection
          radius={blurRadius}
          kind={blurKind}
          onRadius={setBlurRadius}
          onKind={setBlurKind}
          onApply={applyBlur}
          busy={busy}
        />
      )}
      {tab === "sharpen" && (
        <SharpenSection
          radius={sharpRadius}
          amount={sharpAmount}
          threshold={sharpThreshold}
          onRadius={setSharpRadius}
          onAmount={setSharpAmount}
          onThreshold={setSharpThreshold}
          onApply={applySharpen}
          busy={busy}
        />
      )}
      {tab === "transform" && (
        <TransformSection
          cropX={cropX}
          cropY={cropY}
          cropW={cropW}
          cropH={cropH}
          rotateDeg={rotateDeg}
          onCrop={(x, y, w, h) => {
            setCropX(x);
            setCropY(y);
            setCropW(w);
            setCropH(h);
          }}
          onRotateDeg={setRotateDeg}
          onApplyCrop={applyCrop}
          onApplyRotate={applyRotate}
          onApplyFlip={applyFlip}
          busy={busy}
        />
      )}

      {tab !== "transform" && (
        <div
          style={{
            display: "grid",
            gap: spacing.xs,
            placeItems: "center",
            background: colors.bgSoft,
            borderRadius: radius.md,
            padding: spacing.sm,
          }}
        >
          <span style={{ fontSize: 11, color: colors.textMuted }}>Preview</span>
          <canvas
            ref={previewCanvasRef}
            style={{
              maxWidth: PREVIEW_MAX_DIM,
              maxHeight: PREVIEW_MAX_DIM,
              imageRendering: "pixelated",
              border: `1px solid ${colors.border}`,
              borderRadius: radius.sm,
            }}
          />
        </div>
      )}

      {error && (
        <p
          role="alert"
          style={{
            margin: 0,
            color: colors.danger,
            fontSize: 12,
          }}
        >
          {error}
        </p>
      )}
    </section>
  );
}

function tabLabel(t: FilterTab): string {
  switch (t) {
    case "levels":
      return "Levels";
    case "curves":
      return "Curves";
    case "blur":
      return "Blur";
    case "sharpen":
      return "Sharpen";
    case "transform":
      return "Transform";
  }
}

function tabStyle(active: boolean): React.CSSProperties {
  return {
    padding: `${spacing.xs}px ${spacing.sm}px`,
    border: `1px solid ${active ? colors.accent : colors.border}`,
    background: active ? colors.accent : colors.bg,
    color: active ? colors.textInverse : colors.text,
    borderRadius: radius.sm,
    fontSize: 12,
    cursor: "pointer",
  };
}

interface LevelsSectionProps {
  black: number;
  white: number;
  gamma: number;
  onBlack: (v: number) => void;
  onWhite: (v: number) => void;
  onGamma: (v: number) => void;
  onApply: () => void;
  busy: boolean;
}

function LevelsSection({
  black,
  white,
  gamma,
  onBlack,
  onWhite,
  onGamma,
  onApply,
  busy,
}: LevelsSectionProps): JSX.Element {
  return (
    <div style={{ display: "grid", gap: spacing.xs }}>
      <Slider
        label="Black point"
        min={0}
        max={1}
        step={0.01}
        value={black}
        onChange={onBlack}
      />
      <Slider
        label="White point"
        min={0}
        max={1}
        step={0.01}
        value={white}
        onChange={onWhite}
      />
      <Slider
        label="Gamma"
        min={0.1}
        max={3}
        step={0.01}
        value={gamma}
        onChange={onGamma}
      />
      <button
        type="button"
        onClick={onApply}
        disabled={busy}
        style={applyButtonStyle(busy)}
      >
        Apply Levels
      </button>
    </div>
  );
}

interface CurvesSectionProps {
  points: [number, number][];
  onPoints: (next: [number, number][]) => void;
  onApply: () => void;
  busy: boolean;
}

function CurvesSection({
  points,
  onPoints,
  onApply,
  busy,
}: CurvesSectionProps): JSX.Element {
  const SIZE = 200;
  const handleClick = (e: React.MouseEvent<SVGSVGElement>) => {
    const rect = (e.target as Element)
      .closest("svg")!
      .getBoundingClientRect();
    const t = clamp01((e.clientX - rect.left) / rect.width);
    const v = clamp01(1 - (e.clientY - rect.top) / rect.height);
    const next: [number, number][] = [...points, [t, v] as [number, number]];
    next.sort((a, b) => a[0] - b[0]);
    onPoints(next);
  };
  const removePoint = (i: number) => {
    if (points.length <= 2) {
      return; // keep at least two anchors
    }
    onPoints(points.filter((_, idx) => idx !== i));
  };
  const reset = () =>
    onPoints([
      [0, 0],
      [1, 1],
    ]);
  return (
    <div style={{ display: "grid", gap: spacing.xs }}>
      <span style={{ fontSize: 12, color: colors.textMuted }}>
        Click to add a control point. Right-click a point to remove it.
        Identity curve = two anchors at (0, 0) and (1, 1).
      </span>
      <svg
        width={SIZE}
        height={SIZE}
        viewBox={`0 0 ${SIZE} ${SIZE}`}
        onClick={handleClick}
        style={{
          background: colors.bgSoft,
          border: `1px solid ${colors.border}`,
          borderRadius: radius.sm,
          cursor: "crosshair",
        }}
      >
        <line
          x1={0}
          y1={SIZE}
          x2={SIZE}
          y2={0}
          stroke={colors.border}
          strokeDasharray="4 2"
        />
        <polyline
          points={points
            .map(([t, v]) => `${t * SIZE},${(1 - v) * SIZE}`)
            .join(" ")}
          fill="none"
          stroke={colors.accent}
          strokeWidth={2}
        />
        {points.map(([t, v], i) => (
          <circle
            key={`${t}-${v}-${i}`}
            cx={t * SIZE}
            cy={(1 - v) * SIZE}
            r={5}
            fill={colors.accent}
            onContextMenu={(e) => {
              e.preventDefault();
              e.stopPropagation();
              removePoint(i);
            }}
          />
        ))}
      </svg>
      <div style={{ display: "flex", gap: spacing.xs }}>
        <button
          type="button"
          onClick={reset}
          style={secondaryButtonStyle()}
        >
          Reset
        </button>
        <button
          type="button"
          onClick={onApply}
          disabled={busy}
          style={applyButtonStyle(busy)}
        >
          Apply Curves
        </button>
      </div>
    </div>
  );
}

interface BlurSectionProps {
  radius: number;
  kind: RasterBlurKind;
  onRadius: (v: number) => void;
  onKind: (v: RasterBlurKind) => void;
  onApply: () => void;
  busy: boolean;
}

function BlurSection({
  radius: blurR,
  kind,
  onRadius,
  onKind,
  onApply,
  busy,
}: BlurSectionProps): JSX.Element {
  return (
    <div style={{ display: "grid", gap: spacing.xs }}>
      <Slider
        label="Radius"
        min={0.5}
        max={50}
        step={0.5}
        value={blurR}
        onChange={onRadius}
      />
      <div style={{ display: "flex", gap: spacing.xs }}>
        {(["gaussian", "box"] as RasterBlurKind[]).map((k) => (
          <button
            key={k}
            type="button"
            onClick={() => onKind(k)}
            style={tabStyle(kind === k)}
          >
            {k}
          </button>
        ))}
      </div>
      <button
        type="button"
        onClick={onApply}
        disabled={busy}
        style={applyButtonStyle(busy)}
      >
        Apply Blur
      </button>
    </div>
  );
}

interface SharpenSectionProps {
  radius: number;
  amount: number;
  threshold: number;
  onRadius: (v: number) => void;
  onAmount: (v: number) => void;
  onThreshold: (v: number) => void;
  onApply: () => void;
  busy: boolean;
}

function SharpenSection({
  radius: sharpR,
  amount,
  threshold,
  onRadius,
  onAmount,
  onThreshold,
  onApply,
  busy,
}: SharpenSectionProps): JSX.Element {
  return (
    <div style={{ display: "grid", gap: spacing.xs }}>
      <Slider
        label="Radius"
        min={0.5}
        max={20}
        step={0.5}
        value={sharpR}
        onChange={onRadius}
      />
      <Slider
        label="Amount"
        min={0}
        max={3}
        step={0.05}
        value={amount}
        onChange={onAmount}
      />
      <Slider
        label="Threshold (0–255)"
        min={0}
        max={255}
        step={1}
        value={threshold}
        onChange={onThreshold}
      />
      <button
        type="button"
        onClick={onApply}
        disabled={busy}
        style={applyButtonStyle(busy)}
      >
        Apply Sharpen
      </button>
    </div>
  );
}

interface TransformSectionProps {
  cropX: number;
  cropY: number;
  cropW: number;
  cropH: number;
  rotateDeg: number;
  onCrop: (x: number, y: number, w: number, h: number) => void;
  onRotateDeg: (v: number) => void;
  onApplyCrop: () => void;
  onApplyRotate: () => void;
  onApplyFlip: (direction: RasterFlipDirection) => void;
  busy: boolean;
}

function TransformSection({
  cropX,
  cropY,
  cropW,
  cropH,
  rotateDeg,
  onCrop,
  onRotateDeg,
  onApplyCrop,
  onApplyRotate,
  onApplyFlip,
  busy,
}: TransformSectionProps): JSX.Element {
  return (
    <div style={{ display: "grid", gap: spacing.sm }}>
      <fieldset style={fieldsetStyle()}>
        <legend style={legendStyle()}>Crop</legend>
        <div
          style={{
            display: "grid",
            gridTemplateColumns: "1fr 1fr",
            gap: spacing.xs,
          }}
        >
          <NumberInput
            label="X"
            value={cropX}
            onChange={(v) => onCrop(v, cropY, cropW, cropH)}
          />
          <NumberInput
            label="Y"
            value={cropY}
            onChange={(v) => onCrop(cropX, v, cropW, cropH)}
          />
          <NumberInput
            label="W"
            value={cropW}
            onChange={(v) => onCrop(cropX, cropY, v, cropH)}
          />
          <NumberInput
            label="H"
            value={cropH}
            onChange={(v) => onCrop(cropX, cropY, cropW, v)}
          />
        </div>
        <button
          type="button"
          onClick={onApplyCrop}
          disabled={busy}
          style={applyButtonStyle(busy)}
        >
          Apply Crop
        </button>
      </fieldset>

      <fieldset style={fieldsetStyle()}>
        <legend style={legendStyle()}>Rotate</legend>
        <Slider
          label="Angle (deg)"
          min={-180}
          max={180}
          step={1}
          value={rotateDeg}
          onChange={onRotateDeg}
        />
        <button
          type="button"
          onClick={onApplyRotate}
          disabled={busy}
          style={applyButtonStyle(busy)}
        >
          Apply Rotate
        </button>
      </fieldset>

      <fieldset style={fieldsetStyle()}>
        <legend style={legendStyle()}>Flip</legend>
        <div style={{ display: "flex", gap: spacing.xs }}>
          <button
            type="button"
            onClick={() => onApplyFlip("horizontal")}
            disabled={busy}
            style={secondaryButtonStyle()}
          >
            Flip horizontal
          </button>
          <button
            type="button"
            onClick={() => onApplyFlip("vertical")}
            disabled={busy}
            style={secondaryButtonStyle()}
          >
            Flip vertical
          </button>
        </div>
      </fieldset>
    </div>
  );
}

interface SliderProps {
  label: string;
  min: number;
  max: number;
  step: number;
  value: number;
  onChange: (v: number) => void;
}

function Slider({
  label,
  min,
  max,
  step,
  value,
  onChange,
}: SliderProps): JSX.Element {
  return (
    <label style={{ display: "grid", gap: 2, fontSize: 12 }}>
      <span style={{ color: colors.textMuted }}>
        {label} <strong style={{ color: colors.text }}>{value.toFixed(2)}</strong>
      </span>
      <input
        type="range"
        min={min}
        max={max}
        step={step}
        value={value}
        onChange={(e) => onChange(Number(e.target.value))}
      />
    </label>
  );
}

interface NumberInputProps {
  label: string;
  value: number;
  onChange: (v: number) => void;
}

function NumberInput({
  label,
  value,
  onChange,
}: NumberInputProps): JSX.Element {
  return (
    <label style={{ display: "grid", gap: 2, fontSize: 12 }}>
      <span style={{ color: colors.textMuted }}>{label}</span>
      <input
        type="number"
        value={value}
        onChange={(e) => onChange(Number(e.target.value))}
        style={{
          padding: 4,
          border: `1px solid ${colors.border}`,
          borderRadius: radius.sm,
          fontSize: 12,
        }}
      />
    </label>
  );
}

function applyButtonStyle(busy: boolean): React.CSSProperties {
  return {
    padding: `${spacing.xs}px ${spacing.sm}px`,
    background: busy ? colors.bgSoft : colors.accent,
    color: busy ? colors.textMuted : colors.textInverse,
    border: "none",
    borderRadius: radius.sm,
    fontSize: 12,
    cursor: busy ? "not-allowed" : "pointer",
  };
}

function secondaryButtonStyle(): React.CSSProperties {
  return {
    padding: `${spacing.xs}px ${spacing.sm}px`,
    background: colors.bg,
    color: colors.text,
    border: `1px solid ${colors.border}`,
    borderRadius: radius.sm,
    fontSize: 12,
    cursor: "pointer",
  };
}

function fieldsetStyle(): React.CSSProperties {
  return {
    border: `1px solid ${colors.border}`,
    borderRadius: radius.sm,
    padding: spacing.sm,
    margin: 0,
    display: "grid",
    gap: spacing.xs,
  };
}

function legendStyle(): React.CSSProperties {
  return {
    fontSize: 11,
    color: colors.textMuted,
    padding: `0 ${spacing.xs}px`,
  };
}

function errMsg(e: unknown): string {
  if (e instanceof Error) {
    return e.message;
  }
  return String(e);
}

function clamp01(v: number): number {
  if (v < 0) {
    return 0;
  }
  if (v > 1) {
    return 1;
  }
  return v;
}
