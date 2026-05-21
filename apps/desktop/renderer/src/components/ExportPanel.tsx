// ExportPanel — full export workflow surface (Phase 0 + 1).
//
// Renders the per-format export controls (PNG / SVG / PDF / WebP /
// JPEG) and routes the export through the appropriate `window.kcreate
// .export.*` IPC bridge. The bridge does the file I/O — the renderer
// never touches disk directly.

import { useEffect, useState } from "react";

import type {
  JpegExportOptions,
  PdfExportOptions,
  PngExportOptions,
  SvgExportOptions,
  WebpExportOptions,
} from "../../../shared/scene";
import { colors, radius, spacing } from "../styles/tokens";

export interface ExportPanelProps {
  onStatus: (msg: string | null) => void;
  width: number;
  height: number;
}

type ExportFormat = "png" | "svg" | "pdf" | "webp" | "jpeg";

interface FormatChoice {
  id: ExportFormat;
  label: string;
  ext: string;
}

const FORMATS: ReadonlyArray<FormatChoice> = [
  { id: "png", label: "PNG", ext: "png" },
  { id: "svg", label: "SVG", ext: "svg" },
  { id: "pdf", label: "PDF", ext: "pdf" },
  { id: "webp", label: "WebP", ext: "webp" },
  { id: "jpeg", label: "JPEG", ext: "jpg" },
];

export function ExportPanel({
  onStatus,
  width,
  height,
}: ExportPanelProps): JSX.Element {
  const [format, setFormat] = useState<ExportFormat>("png");
  const [scale, setScale] = useState(1);
  const [transparent, setTransparent] = useState(true);
  const [quality, setQuality] = useState(90);
  const [lossless, setLossless] = useState(true);
  const [tempDir, setTempDir] = useState<string>("");
  const [running, setRunning] = useState(false);

  // Resolve a writable directory for export targets. The renderer
  // doesn't have filesystem access — the main process exposes a
  // platform-appropriate temp dir via `runtime.tempDir()`.
  useEffect(() => {
    let cancelled = false;
    void window.kcreate.runtime.tempDir().then((d) => {
      if (!cancelled) setTempDir(d);
    });
    return () => {
      cancelled = true;
    };
  }, []);

  const handleExport = async (): Promise<void> => {
    if (!tempDir) {
      onStatus("Export: temp dir not resolved yet.");
      return;
    }
    setRunning(true);
    onStatus(`Export: ${format.toUpperCase()} →`);
    const ts = Date.now();
    const out = `${tempDir}/kcreate-export-${ts}.${formatExt(format)}`;
    try {
      const bg = transparent ? null : whiteColor();
      if (format === "png") {
        const opts: PngExportOptions = {
          width,
          height,
          scale,
          background: bg,
        };
        const bytes = await window.kcreate.export.png(out, opts);
        onStatus(`PNG · ${bytes} bytes → ${out}`);
      } else if (format === "svg") {
        const opts: SvgExportOptions = {
          width,
          height,
          includeMetadata: false,
          optimize: true,
        };
        const svg = await window.kcreate.export.svg([], opts);
        onStatus(`SVG · ${svg.length} bytes (inline)`);
      } else if (format === "pdf") {
        const opts: PdfExportOptions = {
          widthMm: 210,
          heightMm: 297,
          title: "KCreate document",
        };
        const bytes = await window.kcreate.export.pdf(out, opts);
        onStatus(`PDF · ${bytes} bytes → ${out}`);
      } else if (format === "webp") {
        const opts: WebpExportOptions = {
          width,
          height,
          scale,
          quality,
          lossless,
          background: bg,
        };
        const bytes = await window.kcreate.export.webp(out, opts);
        onStatus(`WebP · ${bytes} bytes → ${out}`);
      } else if (format === "jpeg") {
        // JPEG has no alpha — force a non-null background.
        const opts: JpegExportOptions = {
          width,
          height,
          scale,
          quality,
          background: bg ?? whiteColor(),
        };
        const bytes = await window.kcreate.export.jpeg(out, opts);
        onStatus(`JPEG · ${bytes} bytes → ${out}`);
      }
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      onStatus(`Export failed: ${msg}`);
    } finally {
      setRunning(false);
    }
  };

  const handleBatch = async (): Promise<void> => {
    // Phase 0 "batch": fire PNG @1x and @2x sequentially so the user
    // can see batch flow end-to-end. Real preset library lands with
    // the brand-kit/export-preset persistence in Task 17/18.
    if (!tempDir) return;
    setRunning(true);
    onStatus("Batch: PNG @1x, @2x…");
    try {
      const ts = Date.now();
      const opts1: PngExportOptions = {
        width,
        height,
        scale: 1,
        background: transparent ? null : whiteColor(),
      };
      const opts2: PngExportOptions = { ...opts1, scale: 2 };
      const out1 = `${tempDir}/kcreate-batch-${ts}-1x.png`;
      const out2 = `${tempDir}/kcreate-batch-${ts}-2x.png`;
      const [b1, b2] = await Promise.all([
        window.kcreate.export.png(out1, opts1),
        window.kcreate.export.png(out2, opts2),
      ]);
      onStatus(`Batch · ${b1 + b2} bytes total → ${tempDir}`);
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      onStatus(`Batch failed: ${msg}`);
    } finally {
      setRunning(false);
    }
  };

  const supportsQuality = format === "webp" || format === "jpeg";
  const supportsLossless = format === "webp";
  const supportsTransparency = format !== "jpeg" && format !== "pdf";

  return (
    <aside
      style={{
        width: 320,
        background: colors.bg,
        borderLeft: `1px solid ${colors.border}`,
        display: "flex",
        flexDirection: "column",
        padding: spacing.md,
        gap: spacing.sm,
      }}
    >
      <h2
        style={{
          margin: 0,
          fontSize: 14,
          fontWeight: 600,
          color: colors.text,
        }}
      >
        Export
      </h2>
      <p style={paragraphStyle}>
        Exports run locally through the Rust export crate. No network
        round trip.
      </p>

      <Field label="Format">
        <div style={{ display: "flex", gap: 4, flexWrap: "wrap" }}>
          {FORMATS.map((f) => (
            <button
              key={f.id}
              type="button"
              onClick={() => setFormat(f.id)}
              aria-pressed={format === f.id}
              style={chipBtn(format === f.id)}
            >
              {f.label}
            </button>
          ))}
        </div>
      </Field>

      <Field label={`Scale (${width}×${height} → ${width * scale}×${height * scale})`}>
        <input
          type="number"
          min={1}
          max={8}
          step={1}
          value={scale}
          onChange={(e) =>
            setScale(Math.max(1, Math.min(8, Number(e.target.value) || 1)))
          }
          style={inputStyle}
        />
      </Field>

      {supportsQuality ? (
        <Field label={`Quality (${quality})`}>
          <input
            type="range"
            min={1}
            max={100}
            value={quality}
            onChange={(e) => setQuality(Number(e.target.value))}
          />
        </Field>
      ) : null}

      {supportsLossless ? (
        <ToggleField
          label="Lossless (WebP)"
          value={lossless}
          onChange={setLossless}
        />
      ) : null}

      {supportsTransparency ? (
        <ToggleField
          label="Transparent background"
          value={transparent}
          onChange={setTransparent}
        />
      ) : (
        <p style={hintStyle}>
          {format === "jpeg"
            ? "JPEG has no alpha — exports composite over white."
            : "PDF uses opaque page background."}
        </p>
      )}

      <div style={{ display: "flex", gap: spacing.sm, marginTop: spacing.sm }}>
        <button
          type="button"
          onClick={() => {
            void handleExport();
          }}
          disabled={running || !tempDir}
          style={primaryBtn(running || !tempDir)}
        >
          {running ? "Exporting…" : "Export"}
        </button>
        <button
          type="button"
          onClick={() => {
            void handleBatch();
          }}
          disabled={running || !tempDir}
          style={secondaryBtn(running || !tempDir)}
        >
          Batch (PNG @1x, @2x)
        </button>
      </div>

      <p style={hintStyle}>
        Files write to <code style={monoStyle}>{tempDir || "…"}</code>.
        Phase 1 will add a native save-as dialog.
      </p>
    </aside>
  );
}

function formatExt(f: ExportFormat): string {
  return FORMATS.find((x) => x.id === f)?.ext ?? f;
}

function whiteColor(): [number, number, number, number] {
  return [1.0, 1.0, 1.0, 1.0];
}

function Field({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}): JSX.Element {
  return (
    <label
      style={{
        display: "flex",
        flexDirection: "column",
        gap: 4,
        fontSize: 11,
        color: colors.textMuted,
      }}
    >
      <span>{label}</span>
      {children}
    </label>
  );
}

function ToggleField({
  label,
  value,
  onChange,
}: {
  label: string;
  value: boolean;
  onChange: (v: boolean) => void;
}): JSX.Element {
  return (
    <label
      style={{
        display: "flex",
        alignItems: "center",
        gap: 6,
        fontSize: 12,
        color: colors.text,
        cursor: "pointer",
      }}
    >
      <input
        type="checkbox"
        checked={value}
        onChange={(e) => onChange(e.target.checked)}
      />
      {label}
    </label>
  );
}

const inputStyle: React.CSSProperties = {
  background: colors.bgSoft,
  color: colors.text,
  border: `1px solid ${colors.border}`,
  borderRadius: 4,
  padding: "4px 6px",
  fontSize: 12,
  fontFamily: "inherit",
};

function chipBtn(active: boolean): React.CSSProperties {
  return {
    padding: "4px 10px",
    fontSize: 11,
    fontWeight: active ? 600 : 500,
    background: active ? colors.accent : colors.bgSoft,
    color: active ? colors.textInverse : colors.text,
    border: `1px solid ${active ? colors.accent : colors.border}`,
    borderRadius: radius.pill,
    cursor: "pointer",
  };
}

function primaryBtn(disabled: boolean): React.CSSProperties {
  return {
    flex: 1,
    padding: "8px 14px",
    fontSize: 12,
    fontWeight: 600,
    background: disabled ? colors.bgSoft : colors.accent,
    color: disabled ? colors.textMuted : colors.textInverse,
    border: `1px solid ${disabled ? colors.border : colors.accent}`,
    borderRadius: radius.pill,
    cursor: disabled ? "not-allowed" : "pointer",
  };
}

function secondaryBtn(disabled: boolean): React.CSSProperties {
  return {
    padding: "8px 14px",
    fontSize: 12,
    fontWeight: 500,
    background: colors.bg,
    color: colors.text,
    border: `1px solid ${colors.border}`,
    borderRadius: radius.pill,
    cursor: disabled ? "not-allowed" : "pointer",
    opacity: disabled ? 0.5 : 1,
  };
}

const paragraphStyle: React.CSSProperties = {
  margin: 0,
  fontSize: 11,
  color: colors.textMuted,
  lineHeight: 1.5,
};

const hintStyle: React.CSSProperties = {
  margin: 0,
  fontSize: 11,
  color: colors.textMuted,
  lineHeight: 1.5,
};

const monoStyle: React.CSSProperties = {
  fontFamily:
    'ui-monospace, SFMono-Regular, Menlo, "Roboto Mono", monospace',
  fontSize: 11,
};
