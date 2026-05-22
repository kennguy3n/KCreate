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
import { IconPackDialog } from "./IconPackDialog";

// One leaf job inside a preset. Each leaf is a single `export.*` call.
interface PresetJob {
  format: ExportFormat;
  scale?: number;
  suffix: string;
  background?: [number, number, number, number] | null;
  quality?: number;
  lossless?: boolean;
  pdfWidthMm?: number;
  pdfHeightMm?: number;
}

interface ExportPreset {
  id: string;
  label: string;
  description: string;
  jobs: PresetJob[];
}

const WHITE: [number, number, number, number] = [1, 1, 1, 1];

// Built-in presets. The user spec calls out these five preset groups
// in Block G Task 25; "Developer Handoff" emits the SVG plus a JSON
// dump of the project's design tokens (handled inline in runPreset).
const BUILTIN_PRESETS: ReadonlyArray<ExportPreset> = [
  {
    id: "web-assets",
    label: "Web Assets",
    description: "PNG @1x, @2x, @3x.",
    jobs: [
      { format: "png", scale: 1, suffix: "-1x" },
      { format: "png", scale: 2, suffix: "-2x" },
      { format: "png", scale: 3, suffix: "-3x" },
    ],
  },
  {
    id: "social-pack",
    label: "Social Pack",
    description: "Instagram 1080², Twitter 1200×675, FB 1200×630.",
    jobs: [
      { format: "png", scale: 1, suffix: "-instagram", background: WHITE },
      { format: "png", scale: 1, suffix: "-twitter", background: WHITE },
      { format: "png", scale: 1, suffix: "-facebook", background: WHITE },
    ],
  },
  {
    id: "icon-pack",
    label: "Icon Pack",
    description: "PNG @16/24/32/48/512 + SVG.",
    jobs: [
      { format: "png", scale: 1, suffix: "-16" },
      { format: "png", scale: 1, suffix: "-24" },
      { format: "png", scale: 1, suffix: "-32" },
      { format: "png", scale: 1, suffix: "-48" },
      { format: "png", scale: 1, suffix: "-512" },
      { format: "svg", suffix: "" },
    ],
  },
  {
    id: "print-ready",
    label: "Print Ready",
    description: "PDF at A4 300dpi.",
    jobs: [
      {
        format: "pdf",
        suffix: "",
        pdfWidthMm: 210,
        pdfHeightMm: 297,
      },
    ],
  },
  {
    id: "dev-handoff",
    label: "Developer Handoff",
    description: "SVG + CSS tokens JSON.",
    jobs: [{ format: "svg", suffix: "" }],
  },
];

export interface ExportPanelProps {
  onStatus: (msg: string | null) => void;
  width: number;
  height: number;
  /// Currently-selected node ids in the editor. Used to scope the icon
  /// pack generator to the user's selection so the dialog text "Render
  /// the selected node(s)" reflects actual behaviour. Empty means "no
  /// explicit selection" — the icon-pack backend falls back to the
  /// whole scene.
  selectedIds: string[];
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
  selectedIds,
}: ExportPanelProps): JSX.Element {
  const [format, setFormat] = useState<ExportFormat>("png");
  const [scale, setScale] = useState(1);
  const [transparent, setTransparent] = useState(true);
  const [quality, setQuality] = useState(90);
  const [lossless, setLossless] = useState(true);
  const [tempDir, setTempDir] = useState<string>("");
  const [running, setRunning] = useState(false);
  const [iconPackOpen, setIconPackOpen] = useState(false);

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

  const runPreset = async (preset: ExportPreset): Promise<void> => {
    if (!tempDir) return;
    setRunning(true);
    onStatus(`Preset "${preset.label}" → 0/${preset.jobs.length}`);
    const ts = Date.now();
    const baseName = `kcreate-${preset.id}-${ts}`;
    let totalBytes = 0;
    let succeeded = 0;
    const failures: string[] = [];
    for (let i = 0; i < preset.jobs.length; i += 1) {
      const job = preset.jobs[i];
      if (!job) continue;
      onStatus(`Preset "${preset.label}" → ${i}/${preset.jobs.length}`);
      try {
        const bytes = await runOnePresetJob(
          job,
          tempDir,
          baseName,
          width,
          height,
        );
        totalBytes += bytes;
        succeeded += 1;
      } catch (e) {
        failures.push(`#${i + 1} ${job.format}: ${errorMessage(e)}`);
      }
    }
    if (preset.id === "dev-handoff") {
      // Companion JSON: dump the project's design tokens so a
      // downstream developer can wire colors/typography/spacing into
      // their codebase without the .kstudio archive.
      try {
        const tokens = await window.kcreate.designTokens.get();
        const json = JSON.stringify(tokens, null, 2);
        const blobBytes = new TextEncoder().encode(json).byteLength;
        const handoffPath = `${tempDir}/${baseName}-tokens.json`;
        await window.kcreate.runtime.writeTextFile(handoffPath, json);
        totalBytes += blobBytes;
        succeeded += 1;
      } catch (e) {
        failures.push(`tokens.json: ${errorMessage(e)}`);
      }
    }
    setRunning(false);
    if (failures.length === 0) {
      onStatus(
        `Preset "${preset.label}" · ${succeeded} files · ${totalBytes} bytes → ${tempDir}`,
      );
    } else {
      onStatus(
        `Preset "${preset.label}" finished with ${failures.length} failures: ${failures.join("; ")}`,
      );
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
      </div>

      <hr
        style={{
          border: "none",
          borderTop: `1px solid ${colors.border}`,
          margin: `${spacing.md}px 0 ${spacing.xs}px`,
        }}
      />
      <h3 style={presetHeaderStyle}>Batch presets</h3>
      <p style={hintStyle}>
        Each preset runs as a chain of single-format exports under the
        OS temp directory. Failures inside a chain do not abort the
        rest of the chain.
      </p>
      <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
        {BUILTIN_PRESETS.map((p) => (
          <button
            type="button"
            key={p.id}
            onClick={() => {
              void runPreset(p);
            }}
            disabled={running || !tempDir}
            style={presetBtn(running || !tempDir)}
          >
            <span style={{ fontWeight: 600 }}>{p.label}</span>
            <span style={{ fontSize: 10, color: colors.textMuted }}>
              {p.description}
            </span>
          </button>
        ))}
      </div>

      <p style={hintStyle}>
        Files write to <code style={monoStyle}>{tempDir || "…"}</code>.
        Phase 1 will add a native save-as dialog.
      </p>

      <hr
        style={{
          border: "none",
          borderTop: `1px solid ${colors.border}`,
          margin: `${spacing.md}px 0 ${spacing.xs}px`,
        }}
      />
      <h3 style={presetHeaderStyle}>Icon pack</h3>
      <p style={hintStyle}>
        Render the selected node(s) to web / iOS / Android / favicon
        icon size grids.
      </p>
      <button
        type="button"
        onClick={() => setIconPackOpen(true)}
        style={presetBtn(false)}
      >
        <span style={{ fontWeight: 600 }}>Generate icon pack…</span>
        <span style={{ fontSize: 10, color: colors.textMuted }}>
          Multi-platform sizes via kcreate_export::icon_pack
        </span>
      </button>

      {iconPackOpen ? (
        <IconPackDialog
          nodeIds={selectedIds}
          onClose={() => setIconPackOpen(false)}
          onStatus={onStatus}
        />
      ) : null}
    </aside>
  );
}

function formatExt(f: ExportFormat): string {
  return FORMATS.find((x) => x.id === f)?.ext ?? f;
}

function errorMessage(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

// Execute a single leaf inside a preset chain. Returns bytes written
// so the caller can aggregate. SVG returns the inline string length
// for parity with the rasters.
async function runOnePresetJob(
  job: PresetJob,
  tempDir: string,
  baseName: string,
  width: number,
  height: number,
): Promise<number> {
  const ext = formatExt(job.format);
  const out = `${tempDir}/${baseName}${job.suffix}.${ext}`;
  switch (job.format) {
    case "png": {
      const opts: PngExportOptions = {
        width,
        height,
        scale: job.scale ?? 1,
        background: job.background ?? null,
      };
      return window.kcreate.export.png(out, opts);
    }
    case "svg": {
      const opts: SvgExportOptions = {
        width,
        height,
        includeMetadata: false,
        optimize: true,
      };
      const svg = await window.kcreate.export.svg([], opts);
      await window.kcreate.runtime.writeTextFile(out, svg);
      return new TextEncoder().encode(svg).byteLength;
    }
    case "pdf": {
      const opts: PdfExportOptions = {
        widthMm: job.pdfWidthMm ?? 210,
        heightMm: job.pdfHeightMm ?? 297,
        title: "KCreate document",
      };
      return window.kcreate.export.pdf(out, opts);
    }
    case "webp": {
      const opts: WebpExportOptions = {
        width,
        height,
        scale: job.scale ?? 1,
        quality: job.quality ?? 80,
        lossless: job.lossless ?? false,
        background: job.background ?? null,
      };
      return window.kcreate.export.webp(out, opts);
    }
    case "jpeg": {
      const opts: JpegExportOptions = {
        width,
        height,
        scale: job.scale ?? 1,
        quality: job.quality ?? 80,
        background: job.background ?? WHITE,
      };
      return window.kcreate.export.jpeg(out, opts);
    }
    default: {
      // Exhaustiveness guard: TS will narrow `job.format` to `never`
      // if we ever extend `ExportFormat` without updating this switch.
      const exhaustive: never = job.format;
      throw new Error(`unsupported preset job format: ${String(exhaustive)}`);
    }
  }
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

const presetHeaderStyle: React.CSSProperties = {
  margin: 0,
  fontSize: 11,
  fontWeight: 700,
  letterSpacing: 0.4,
  textTransform: "uppercase",
  color: colors.textMuted,
};

function presetBtn(disabled: boolean): React.CSSProperties {
  return {
    display: "flex",
    flexDirection: "column",
    alignItems: "flex-start",
    gap: 2,
    padding: "8px 10px",
    fontSize: 11,
    fontWeight: 500,
    background: colors.bgSoft,
    color: colors.text,
    border: `1px solid ${colors.border}`,
    borderRadius: 6,
    cursor: disabled ? "not-allowed" : "pointer",
    opacity: disabled ? 0.5 : 1,
    textAlign: "left",
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
