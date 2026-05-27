// SpotColorLibraryPanel — Phase 3 / Phase 5 print support.
//
// Surface for managing the project's `SpotColorLibrary`: list the
// registered swatches, add a one-off named swatch, import a
// Pantone-style JSON catalogue, and remove unused swatches. Every
// mutation goes through `window.kcreate.color` IPC so it lands on
// the project log as an undoable operation.
//
// The catalogue parser is the Rust-side
// `SpotColorLibrary::from_json_catalog`. Two shapes are accepted:
//
//   { "name": "Pantone Solid Coated",
//     "entries": [{ "id": "PANTONE 185 C", "cmyk": [0,1,.84,0] }, ...] }
//
//   { "PANTONE 185 C": { "cmyk": [0, 1, 0.84, 0] }, ... }
//
// Malformed swatches are dropped silently so a single bad entry can't
// poison the whole library. The renderer surfaces an explicit
// "parsed N, added X, overwritten Y" report so the user can sanity-
// check the import.

import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ChangeEvent,
} from "react";

import type {
  SpotCatalogLoadReportWire,
  SpotColorWire,
} from "../../../shared/scene";
import { colors, radius, spacing } from "../styles/tokens";

export interface SpotColorLibraryPanelProps {
  /** Bubble status messages to the editor's global status strip. */
  onStatus?: (msg: string | null) => void;
}

interface DraftSwatch {
  name: string;
  displayName: string;
  c: string;
  m: string;
  y: string;
  k: string;
}

const EMPTY_DRAFT: DraftSwatch = {
  name: "",
  displayName: "",
  c: "0",
  m: "0",
  y: "0",
  k: "0",
};

/// The renderer reads the catalogue file as text (UTF-8) and hands
/// the contents to the bridge. We cap the size at 4 MiB so a
/// pathological file (e.g. an accidentally selected binary) doesn't
/// freeze the main thread during JSON parse. A full Pantone Solid
/// Coated library is ~80 KB, so 4 MiB is two orders of magnitude of
/// headroom.
const MAX_CATALOG_BYTES = 4 * 1024 * 1024;

export function SpotColorLibraryPanel({
  onStatus,
}: SpotColorLibraryPanelProps): JSX.Element {
  const [spots, setSpots] = useState<SpotColorWire[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [draft, setDraft] = useState<DraftSwatch>(EMPTY_DRAFT);
  const [filter, setFilter] = useState("");
  const [lastReport, setLastReport] =
    useState<SpotCatalogLoadReportWire | null>(null);
  const fileInputRef = useRef<HTMLInputElement | null>(null);

  const refresh = useCallback(async () => {
    setLoadError(null);
    try {
      const next = await window.kcreate.color.listSpots();
      setSpots(next);
    } catch (e) {
      setLoadError(errMsg(e));
      setSpots([]);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const filtered = useMemo(() => {
    if (!spots) return [];
    const needle = filter.trim().toLowerCase();
    if (!needle) return spots;
    return spots.filter(
      (s) =>
        s.name.toLowerCase().includes(needle) ||
        s.displayName.toLowerCase().includes(needle),
    );
  }, [spots, filter]);

  const onAdd = useCallback(async () => {
    setLoadError(null);
    const name = draft.name.trim();
    if (!name) {
      setLoadError("Swatch name is required.");
      return;
    }
    const c = parseCmykChannel(draft.c);
    const m = parseCmykChannel(draft.m);
    const y = parseCmykChannel(draft.y);
    const k = parseCmykChannel(draft.k);
    if (c === null || m === null || y === null || k === null) {
      setLoadError("CMYK channels must be numbers in [0, 1] or [0, 100].");
      return;
    }
    const display =
      draft.displayName.trim().length > 0 ? draft.displayName.trim() : name;
    setBusy(true);
    try {
      await window.kcreate.color.upsertSpot({
        name,
        displayName: display,
        fallbackCmyk: [c, m, y, k],
      });
      setDraft(EMPTY_DRAFT);
      onStatus?.(`Added spot ‘${name}’.`);
      await refresh();
    } catch (e) {
      setLoadError(errMsg(e));
    } finally {
      setBusy(false);
    }
  }, [draft, onStatus, refresh]);

  const onRemove = useCallback(
    async (name: string) => {
      setLoadError(null);
      setBusy(true);
      try {
        const removed = await window.kcreate.color.removeSpot(name);
        if (removed) {
          onStatus?.(`Removed spot ‘${name}’.`);
        }
        await refresh();
      } catch (e) {
        setLoadError(errMsg(e));
      } finally {
        setBusy(false);
      }
    },
    [onStatus, refresh],
  );

  const onCatalogFile = useCallback(
    async (event: ChangeEvent<HTMLInputElement>) => {
      const file = event.target.files?.[0];
      // Reset the input value so re-selecting the same file fires
      // the change event again.
      event.target.value = "";
      if (!file) return;
      if (file.size > MAX_CATALOG_BYTES) {
        setLoadError(
          `Catalogue is ${formatBytes(file.size)}; maximum is ${formatBytes(
            MAX_CATALOG_BYTES,
          )}.`,
        );
        return;
      }
      setLoadError(null);
      setBusy(true);
      try {
        const raw = await file.text();
        const report = await window.kcreate.color.loadCatalog(raw);
        setLastReport(report);
        // Build a status summary that distinguishes catalogue-level
        // counts (raw/parsed/dropped) from project-level counts
        // (added/overwritten). Mention drops/dedups only when they
        // happened, so the common case stays terse.
        const dropParts: string[] = [];
        if (report.malformed > 0) {
          dropParts.push(`${report.malformed} malformed`);
        }
        if (report.duplicatesInCatalog > 0) {
          dropParts.push(`${report.duplicatesInCatalog} duplicate`);
        }
        const dropSuffix =
          dropParts.length > 0 ? ` (dropped ${dropParts.join(" + ")})` : "";
        onStatus?.(
          `Loaded catalogue: ${report.parsed} of ${report.rawEntries} entries → ${report.added} added, ${report.overwritten} replaced${dropSuffix}.`,
        );
        await refresh();
      } catch (e) {
        setLoadError(errMsg(e));
      } finally {
        setBusy(false);
      }
    },
    [onStatus, refresh],
  );

  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        gap: spacing.sm,
        padding: spacing.md,
        background: colors.bg,
        borderRadius: radius.card,
      }}
    >
      <header
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          gap: spacing.sm,
        }}
      >
        <h2
          style={{
            margin: 0,
            fontSize: 14,
            color: colors.text,
          }}
        >
          Spot color library
        </h2>
        <button
          type="button"
          onClick={() => {
            void refresh();
          }}
          disabled={busy}
          aria-label="Reload library"
          style={iconBtn(busy)}
        >
          ↻
        </button>
      </header>

      <p style={{ margin: 0, fontSize: 11, color: colors.textMuted }}>
        Manage Pantone-style named inks used by the export pipeline.
        Catalogues can be imported as JSON (
        <code>{`{ "entries": [...] }`}</code> or a bare{" "}
        <code>{`{ name: { cmyk: [...] }, ... }`}</code> map).
      </p>

      <div
        style={{
          display: "flex",
          gap: spacing.xs,
          alignItems: "center",
        }}
      >
        <input
          type="text"
          placeholder="Filter swatches"
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          disabled={busy}
          style={{
            flex: 1,
            padding: "4px 8px",
            fontSize: 12,
            border: `1px solid ${colors.border}`,
            borderRadius: radius.sm,
            background: colors.bg,
            color: colors.text,
          }}
        />
        <button
          type="button"
          onClick={() => fileInputRef.current?.click()}
          disabled={busy}
          style={primaryBtn(busy)}
        >
          Load catalogue…
        </button>
        <input
          ref={fileInputRef}
          type="file"
          accept="application/json,.json,.pantone"
          onChange={(e) => {
            void onCatalogFile(e);
          }}
          style={{ display: "none" }}
        />
      </div>

      {lastReport ? (
        <p
          style={{
            margin: 0,
            fontSize: 11,
            color: colors.textMuted,
          }}
          data-testid="catalog-report"
        >
          Catalogue: {lastReport.parsed} of {lastReport.rawEntries}{" "}
          entries kept{lastReport.duplicatesInCatalog > 0
            ? `, ${lastReport.duplicatesInCatalog} dedup'd`
            : ""}
          {lastReport.malformed > 0
            ? `, ${lastReport.malformed} malformed dropped`
            : ""}
          . Merged: {lastReport.added} added, {lastReport.overwritten}{" "}
          overwrote.
        </p>
      ) : null}

      <div
        style={{
          maxHeight: 240,
          overflowY: "auto",
          border: `1px solid ${colors.border}`,
          borderRadius: radius.sm,
        }}
        role="list"
        aria-label="Spot color list"
      >
        {filtered.length === 0 ? (
          <p
            style={{
              margin: 0,
              padding: spacing.sm,
              fontSize: 12,
              color: colors.textMuted,
            }}
          >
            No swatches match.
          </p>
        ) : (
          filtered.map((s) => (
            <SwatchRow
              key={s.name}
              swatch={s}
              disabled={busy}
              onRemove={() => {
                void onRemove(s.name);
              }}
            />
          ))
        )}
      </div>

      <fieldset
        style={{
          border: `1px solid ${colors.border}`,
          borderRadius: radius.sm,
          padding: spacing.sm,
          display: "flex",
          flexDirection: "column",
          gap: spacing.xs,
        }}
      >
        <legend
          style={{ padding: "0 4px", fontSize: 11, color: colors.textMuted }}
        >
          Add a swatch
        </legend>
        <Row>
          <TextField
            label="Name (lookup key)"
            value={draft.name}
            onChange={(v) => setDraft((d) => ({ ...d, name: v }))}
            placeholder="PANTONE 185 C"
            disabled={busy}
          />
          <TextField
            label="Display name"
            value={draft.displayName}
            onChange={(v) => setDraft((d) => ({ ...d, displayName: v }))}
            placeholder="Pantone 185 C"
            disabled={busy}
          />
        </Row>
        <Row>
          <CmykField
            label="C"
            value={draft.c}
            onChange={(v) => setDraft((d) => ({ ...d, c: v }))}
            disabled={busy}
          />
          <CmykField
            label="M"
            value={draft.m}
            onChange={(v) => setDraft((d) => ({ ...d, m: v }))}
            disabled={busy}
          />
          <CmykField
            label="Y"
            value={draft.y}
            onChange={(v) => setDraft((d) => ({ ...d, y: v }))}
            disabled={busy}
          />
          <CmykField
            label="K"
            value={draft.k}
            onChange={(v) => setDraft((d) => ({ ...d, k: v }))}
            disabled={busy}
          />
        </Row>
        <button
          type="button"
          onClick={() => {
            void onAdd();
          }}
          disabled={busy || draft.name.trim().length === 0}
          style={primaryBtn(busy || draft.name.trim().length === 0)}
        >
          {busy ? "Working…" : "Add swatch"}
        </button>
      </fieldset>

      {loadError ? (
        <p
          style={{
            margin: 0,
            fontSize: 12,
            color: colors.danger,
          }}
          role="alert"
        >
          {loadError}
        </p>
      ) : null}
    </div>
  );
}

function SwatchRow({
  swatch,
  disabled,
  onRemove,
}: {
  swatch: SpotColorWire;
  disabled: boolean;
  onRemove: () => void;
}): JSX.Element {
  const [c, m, y, k] = swatch.fallbackCmyk;
  const swatchPreview = cmykToCssHex(c, m, y, k);
  return (
    <div
      role="listitem"
      style={{
        display: "grid",
        gridTemplateColumns: "20px 1fr auto auto",
        gap: spacing.xs,
        alignItems: "center",
        padding: "6px 8px",
        borderBottom: `1px solid ${colors.border}`,
      }}
    >
      <span
        aria-hidden
        style={{
          display: "inline-block",
          width: 16,
          height: 16,
          background: swatchPreview,
          border: `1px solid ${colors.border}`,
          borderRadius: 2,
        }}
      />
      <div
        style={{
          display: "flex",
          flexDirection: "column",
          gap: 1,
          minWidth: 0,
        }}
      >
        <span
          style={{
            fontSize: 12,
            color: colors.text,
            overflow: "hidden",
            textOverflow: "ellipsis",
            whiteSpace: "nowrap",
          }}
          title={swatch.name}
        >
          {swatch.displayName || swatch.name}
        </span>
        <span
          style={{
            fontSize: 10,
            color: colors.textMuted,
          }}
        >
          C{pct(c)} M{pct(m)} Y{pct(y)} K{pct(k)}
          {swatch.libraryReference ? ` · ${swatch.libraryReference}` : ""}
        </span>
      </div>
      <code
        style={{
          fontSize: 10,
          color: colors.textMuted,
          background: colors.bgSoft,
          padding: "1px 4px",
          borderRadius: 3,
        }}
      >
        {swatch.name}
      </code>
      <button
        type="button"
        onClick={onRemove}
        disabled={disabled}
        aria-label={`Remove ${swatch.name}`}
        style={iconBtn(disabled)}
      >
        ×
      </button>
    </div>
  );
}

function Row({ children }: { children: React.ReactNode }): JSX.Element {
  return (
    <div
      style={{
        display: "flex",
        gap: spacing.xs,
        alignItems: "flex-end",
      }}
    >
      {children}
    </div>
  );
}

function TextField({
  label,
  value,
  onChange,
  placeholder,
  disabled,
}: {
  label: string;
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
  disabled?: boolean;
}): JSX.Element {
  return (
    <label style={{ display: "flex", flexDirection: "column", gap: 2, flex: 1 }}>
      <span style={{ fontSize: 10, color: colors.textMuted }}>{label}</span>
      <input
        type="text"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder}
        disabled={disabled}
        style={{
          padding: "4px 6px",
          fontSize: 12,
          border: `1px solid ${colors.border}`,
          borderRadius: radius.sm,
          background: colors.bg,
          color: colors.text,
        }}
      />
    </label>
  );
}

function CmykField({
  label,
  value,
  onChange,
  disabled,
}: {
  label: string;
  value: string;
  onChange: (v: string) => void;
  disabled?: boolean;
}): JSX.Element {
  return (
    <label
      style={{
        display: "flex",
        flexDirection: "column",
        gap: 2,
        width: 56,
      }}
    >
      <span style={{ fontSize: 10, color: colors.textMuted }}>{label}</span>
      <input
        type="text"
        inputMode="decimal"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        disabled={disabled}
        style={{
          padding: "4px 6px",
          fontSize: 12,
          border: `1px solid ${colors.border}`,
          borderRadius: radius.sm,
          background: colors.bg,
          color: colors.text,
          textAlign: "right",
        }}
      />
    </label>
  );
}

/// Accept either a 0..1 fraction (`"0.85"`) or a 0..100 percent
/// (`"85"`) and normalise to a fraction. Returns `null` for any
/// unparseable input or NaN; the caller surfaces a validation
/// message.
function parseCmykChannel(raw: string): number | null {
  const trimmed = raw.trim();
  if (trimmed === "") return 0;
  const n = Number(trimmed);
  if (!Number.isFinite(n)) return null;
  if (n < 0) return null;
  if (n <= 1) return n;
  if (n <= 100) return n / 100;
  return null;
}

function cmykToCssHex(c: number, m: number, y: number, k: number): string {
  // Simple naïve CMYK → sRGB conversion for the swatch preview chip.
  // Production print colour comes from the PDF pipeline; this is
  // strictly for the UI.
  const r = Math.round(255 * (1 - c) * (1 - k));
  const g = Math.round(255 * (1 - m) * (1 - k));
  const b = Math.round(255 * (1 - y) * (1 - k));
  return `#${toHex(r)}${toHex(g)}${toHex(b)}`;
}

function toHex(v: number): string {
  const clamped = Math.max(0, Math.min(255, v));
  const s = clamped.toString(16);
  return s.length === 1 ? `0${s}` : s;
}

function pct(v: number): string {
  return `${Math.round(v * 100)}`;
}

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KiB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MiB`;
}

function errMsg(e: unknown): string {
  if (e instanceof Error) return e.message;
  if (typeof e === "string") return e;
  return JSON.stringify(e);
}

function iconBtn(disabled: boolean): React.CSSProperties {
  return {
    width: 22,
    height: 22,
    padding: 0,
    border: `1px solid ${colors.border}`,
    borderRadius: radius.sm,
    background: colors.bg,
    color: disabled ? colors.textMuted : colors.text,
    cursor: disabled ? "not-allowed" : "pointer",
    fontSize: 14,
    lineHeight: "20px",
  };
}

function primaryBtn(disabled: boolean): React.CSSProperties {
  return {
    padding: "4px 10px",
    fontSize: 12,
    fontWeight: 500,
    border: "none",
    borderRadius: radius.sm,
    background: disabled ? colors.bgSoft : colors.accent,
    color: disabled ? colors.textMuted : colors.textInverse,
    cursor: disabled ? "not-allowed" : "pointer",
  };
}
