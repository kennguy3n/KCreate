// ExportPreviewPanel — Phase 10 Block C Task 15.
//
// Live preview of the bytes the Export panel will emit. The preview
// reuses the same export pipeline as the real export, capped at 1024
// px on the longest side so the IPC payload stays small. Renders the
// resulting bytes back into a data URL for browser-native display
// without writing temp files.
//
// Debounced so rapid edits to format / node selection don't flood
// the bridge.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import type { ExportPreviewResponse } from "../../../shared/scene";
import { colors, radius, spacing } from "../styles/tokens";

export interface ExportPreviewPanelProps {
  /** Node id to preview. `null` clears the panel. */
  nodeId: string | null;
  /** Format the Export panel is currently configured for. */
  format: "png" | "jpeg" | "webp";
  /** Max longest-side in px (default 1024, clamped by the bridge). */
  maxDimensionPx?: number;
  /** Optional status sink for surfacing errors. */
  onStatus?: (msg: string | null) => void;
}

const DEBOUNCE_MS = 300;

function errMsg(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

function humanBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(2)} MB`;
}

export function ExportPreviewPanel({
  nodeId,
  format,
  maxDimensionPx = 1024,
  onStatus,
}: ExportPreviewPanelProps): JSX.Element {
  const [preview, setPreview] = useState<ExportPreviewResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [zoom, setZoom] = useState(1);
  const requestSeq = useRef(0);

  const refresh = useCallback(async () => {
    if (!nodeId) {
      setPreview(null);
      setError(null);
      return;
    }
    const seq = ++requestSeq.current;
    setBusy(true);
    try {
      const resp = await window.kcreate.phase10.exportPreview({
        nodeId,
        format,
        maxDimensionPx,
      });
      // Stale-response guard: a faster request fired after us wins.
      if (seq !== requestSeq.current) return;
      setPreview(resp);
      setError(null);
    } catch (e) {
      if (seq !== requestSeq.current) return;
      const msg = errMsg(e);
      setError(msg);
      setPreview(null);
      onStatus?.(`export preview: ${msg}`);
    } finally {
      if (seq === requestSeq.current) setBusy(false);
    }
  }, [nodeId, format, maxDimensionPx, onStatus]);

  // Debounced effect: collapse rapid changes into one bridge call.
  useEffect(() => {
    const t = window.setTimeout(() => {
      void refresh();
    }, DEBOUNCE_MS);
    return () => window.clearTimeout(t);
  }, [refresh]);

  const dataUrl = useMemo(() => {
    if (!preview) return null;
    return `data:${preview.mimeType};base64,${preview.bytesBase64}`;
  }, [preview]);

  return (
    <section
      aria-label="Export preview"
      style={{
        display: "flex",
        flexDirection: "column",
        gap: spacing.sm,
        padding: spacing.md,
        background: colors.bgSoft,
        border: `1px solid ${colors.border}`,
        borderRadius: radius.md,
      }}
    >
      <header
        style={{
          display: "flex",
          justifyContent: "space-between",
          alignItems: "center",
        }}
      >
        <span style={{ fontWeight: 600, fontSize: 13 }}>Live preview</span>
        <div style={{ display: "flex", gap: spacing.xs, alignItems: "center" }}>
          {busy ? (
            <span
              style={{
                fontSize: 11,
                color: colors.textMuted,
              }}
            >
              rendering…
            </span>
          ) : null}
          <button
            type="button"
            onClick={() => setZoom((z) => Math.max(0.25, z / 1.5))}
            aria-label="Zoom out"
            style={miniBtnStyle}
          >
            −
          </button>
          <span style={{ fontSize: 11, color: colors.textMuted, width: 40, textAlign: "center" }}>
            {(zoom * 100).toFixed(0)}%
          </span>
          <button
            type="button"
            onClick={() => setZoom((z) => Math.min(4, z * 1.5))}
            aria-label="Zoom in"
            style={miniBtnStyle}
          >
            +
          </button>
        </div>
      </header>
      <div
        style={{
          minHeight: 200,
          maxHeight: 420,
          overflow: "auto",
          background:
            "repeating-conic-gradient(#0001 0% 25%, transparent 0% 50%) 50% / 16px 16px",
          borderRadius: radius.sm,
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
        }}
      >
        {error ? (
          <span
            style={{
              color: colors.danger,
              fontSize: 12,
              padding: spacing.md,
              textAlign: "center",
            }}
          >
            {error}
          </span>
        ) : dataUrl && preview ? (
          <img
            src={dataUrl}
            alt={`Preview of node at ${preview.width}×${preview.height}`}
            style={{
              width: preview.width * zoom,
              height: preview.height * zoom,
              imageRendering: zoom > 1.5 ? "pixelated" : "auto",
              maxWidth: "none",
            }}
          />
        ) : (
          <span style={{ color: colors.textMuted, fontSize: 12 }}>
            {nodeId ? "Preview loading…" : "Select a node to preview"}
          </span>
        )}
      </div>
      {preview ? (
        <footer
          style={{
            display: "flex",
            justifyContent: "space-between",
            fontSize: 11,
            color: colors.textMuted,
          }}
        >
          <span>
            {preview.width} × {preview.height} {format.toUpperCase()}
          </span>
          <span>{humanBytes(preview.byteSize)}</span>
        </footer>
      ) : null}
    </section>
  );
}

const miniBtnStyle: React.CSSProperties = {
  padding: "2px 8px",
  background: "transparent",
  color: colors.text,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.sm,
  cursor: "pointer",
  fontSize: 12,
};
