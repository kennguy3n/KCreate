// BrandVersionPanel — Phase 8 Block C Task 16.
//
// Surfaces brand-kit versioning: save a description-tagged snapshot
// of a kit, list past snapshots, view structured diffs between two
// snapshots (added / removed / changed colours, font changes,
// spacing changes, name changes), and restore a kit to a prior
// snapshot.
//
// Wires `window.kcreate.phase8.brandKitSaveVersion / listVersions /
// restoreVersion / diff`. The storage layer (versioning, JSON
// payload format, salt-free diff algorithm) is implemented in
// `crates/kcreate_storage/src/brand_versions.rs`. This panel is a
// pure UI layer that displays the data and forwards user gestures
// back to the bridge.
//
// UX model:
//   * Top: dropdown to choose which brand kit to manage (the
//     project may have multiple).
//   * Middle: list of snapshots newest-first. Each row shows the
//     description, the snapshot UUID (abbreviated), and the
//     timestamp; clicking selects it as either "before" or "after"
//     for the diff viewer.
//   * Right of list: diff viewer panel — once two snapshots are
//     selected, renders the structured diff with colour swatch
//     before/after pairs.
//   * Bottom: text input + "Save snapshot" button to capture a new
//     version of the currently selected kit.
//   * Per-snapshot row: "Restore" button (with confirm) reverts the
//     kit to that snapshot and triggers a `kits.reload()` in the
//     parent panel via the `onAfterRestore` callback (the
//     brand-kit editor needs to re-fetch since the underlying kit
//     row was overwritten).

import {
  useCallback,
  useEffect,
  useMemo,
  useState,
} from "react";

import type {
  BrandKit,
  BrandKitDiff,
  BrandKitVersionInfo,
  NamedColor,
  RgbaColor,
} from "../../../shared/scene";
import { colors, radius, spacing } from "../styles/tokens";

export interface BrandVersionPanelProps {
  /** Optional callback fired after a successful restore so the
   * parent (`BrandKitEditor`) can re-pull the kit list. */
  onAfterRestore?: (kit: BrandKit) => void;
  /** Optional status sink — same convention as PreflightPanel. */
  onStatus?: (msg: string | null) => void;
}

interface DiffSelection {
  before: string | null;
  after: string | null;
}

export function BrandVersionPanel({
  onAfterRestore,
  onStatus,
}: BrandVersionPanelProps): JSX.Element {
  const [kits, setKits] = useState<BrandKit[]>([]);
  const [activeKitId, setActiveKitId] = useState<string | null>(null);
  const [versions, setVersions] = useState<BrandKitVersionInfo[]>([]);
  const [description, setDescription] = useState("");
  const [diffSelection, setDiffSelection] = useState<DiffSelection>({
    before: null,
    after: null,
  });
  const [diff, setDiff] = useState<BrandKitDiff | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const status = useCallback(
    (msg: string | null) => {
      onStatus?.(msg);
    },
    [onStatus],
  );

  const loadKits = useCallback(async (): Promise<void> => {
    try {
      const next = await window.kcreate.brandKit.list();
      setKits(next);
      setActiveKitId((prev) => {
        if (prev != null && next.some((k) => k.id === prev)) return prev;
        return next[0]?.id ?? null;
      });
    } catch (e) {
      setError(`Failed to load brand kits: ${errMsg(e)}`);
    }
  }, []);

  const loadVersions = useCallback(
    async (kitId: string): Promise<void> => {
      try {
        const next = await window.kcreate.phase8.brandKitListVersions(kitId);
        setVersions(next);
        // Drop diff selections that no longer reference live versions
        // — happens after a sibling client deletes a row (future) or
        // a fresh project resets the version table.
        const ids = new Set(next.map((v) => v.versionId));
        setDiffSelection((sel) => ({
          before: sel.before != null && ids.has(sel.before) ? sel.before : null,
          after: sel.after != null && ids.has(sel.after) ? sel.after : null,
        }));
      } catch (e) {
        setError(`Failed to list versions: ${errMsg(e)}`);
      }
    },
    [],
  );

  useEffect(() => {
    void loadKits();
  }, [loadKits]);

  useEffect(() => {
    if (activeKitId == null) {
      setVersions([]);
      return;
    }
    void loadVersions(activeKitId);
  }, [activeKitId, loadVersions]);

  // Recompute the diff whenever both sides of the selection are set.
  useEffect(() => {
    const { before, after } = diffSelection;
    if (before == null || after == null) {
      setDiff(null);
      return;
    }
    if (before === after) {
      // A diff against itself is empty by definition; short-circuit
      // so we don't issue a no-op bridge call.
      setDiff(emptyDiff());
      return;
    }
    let cancelled = false;
    (async () => {
      try {
        const next = await window.kcreate.phase8.brandKitDiff(before, after);
        if (!cancelled) setDiff(next);
      } catch (e) {
        if (!cancelled) setError(`Diff failed: ${errMsg(e)}`);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [diffSelection]);

  const handleSave = useCallback(async (): Promise<void> => {
    if (activeKitId == null) return;
    const trimmed = description.trim();
    if (trimmed.length === 0) {
      setError("Provide a description before saving the snapshot.");
      return;
    }
    setBusy("save");
    setError(null);
    try {
      const v = await window.kcreate.phase8.brandKitSaveVersion(
        activeKitId,
        trimmed,
      );
      status(`Saved snapshot "${v.description}".`);
      setDescription("");
      await loadVersions(activeKitId);
    } catch (e) {
      setError(`Save failed: ${errMsg(e)}`);
    } finally {
      setBusy(null);
    }
  }, [activeKitId, description, loadVersions, status]);

  const handleRestore = useCallback(
    async (versionId: string): Promise<void> => {
      if (activeKitId == null) return;
      const ok = window.confirm(
        "Restore this snapshot? The current colours / fonts / spacing of this brand kit will be overwritten. The current state is itself snapshotted by saving a new version first.",
      );
      if (!ok) return;
      setBusy(`restore-${versionId}`);
      setError(null);
      try {
        const kit = await window.kcreate.phase8.brandKitRestoreVersion(
          versionId,
        );
        status(`Restored brand kit "${kit.name}".`);
        onAfterRestore?.(kit);
        await loadKits();
        await loadVersions(activeKitId);
      } catch (e) {
        setError(`Restore failed: ${errMsg(e)}`);
      } finally {
        setBusy(null);
      }
    },
    [activeKitId, loadKits, loadVersions, onAfterRestore, status],
  );

  const activeKit = useMemo(
    () => kits.find((k) => k.id === activeKitId) ?? null,
    [activeKitId, kits],
  );

  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        gap: spacing.md,
        padding: spacing.md,
        fontSize: 12,
      }}
    >
      <header style={{ display: "flex", flexDirection: "column", gap: 2 }}>
        <h3 style={{ margin: 0, fontSize: 13, fontWeight: 600 }}>
          Brand kit versions
        </h3>
        <small style={{ color: colors.textMuted }}>
          Snapshot the current brand kit, compare two snapshots, or
          revert to any point in its history.
        </small>
      </header>

      <label style={fieldLabelStyle}>
        Brand kit
        <select
          value={activeKitId ?? ""}
          onChange={(e) => setActiveKitId(e.target.value || null)}
          style={selectStyle}
          disabled={kits.length === 0}
        >
          {kits.length === 0 ? (
            <option value="">No brand kits in this project</option>
          ) : null}
          {kits.map((k) => (
            <option key={k.id} value={k.id}>
              {k.name}
            </option>
          ))}
        </select>
      </label>

      {activeKit != null ? (
        <section
          style={{
            display: "flex",
            flexDirection: "column",
            gap: spacing.sm,
            background: colors.bgSoft,
            padding: spacing.sm,
            borderRadius: radius.md,
            border: `1px solid ${colors.border}`,
          }}
        >
          <strong style={{ fontSize: 12 }}>Save snapshot</strong>
          <input
            type="text"
            value={description}
            onChange={(e) => setDescription(e.target.value)}
            placeholder="e.g. Q3 launch palette"
            style={inputStyle}
            maxLength={200}
          />
          <button
            type="button"
            onClick={() => {
              void handleSave();
            }}
            disabled={busy != null || description.trim().length === 0}
            style={primaryButtonStyle(busy === "save")}
          >
            {busy === "save" ? "Saving…" : "Save snapshot"}
          </button>
        </section>
      ) : null}

      <section style={{ display: "flex", flexDirection: "column", gap: 4 }}>
        <strong style={{ fontSize: 12 }}>History</strong>
        {versions.length === 0 ? (
          <small style={{ color: colors.textMuted }}>
            {activeKitId == null
              ? "Select a brand kit to view its history."
              : "No snapshots yet. Save one above."}
          </small>
        ) : (
          <ul
            style={{
              listStyle: "none",
              margin: 0,
              padding: 0,
              display: "flex",
              flexDirection: "column",
              gap: 2,
              maxHeight: 280,
              overflowY: "auto",
            }}
          >
            {versions.map((v) => {
              const isBefore = diffSelection.before === v.versionId;
              const isAfter = diffSelection.after === v.versionId;
              return (
                <li
                  key={v.versionId}
                  style={{
                    display: "flex",
                    flexDirection: "column",
                    gap: 2,
                    padding: spacing.xs,
                    background:
                      isBefore || isAfter ? colors.accentBgSoft : "transparent",
                    border: `1px solid ${
                      isBefore || isAfter ? colors.accent : colors.border
                    }`,
                    borderRadius: radius.sm,
                  }}
                >
                  <div
                    style={{
                      display: "flex",
                      justifyContent: "space-between",
                      gap: 4,
                    }}
                  >
                    <strong style={{ fontSize: 12 }}>{v.description}</strong>
                    <small style={{ color: colors.textMuted }}>
                      {formatTimestamp(v.timestamp)}
                    </small>
                  </div>
                  <div style={{ display: "flex", gap: 4, flexWrap: "wrap" }}>
                    <button
                      type="button"
                      onClick={() =>
                        setDiffSelection((sel) => ({
                          ...sel,
                          before: isBefore ? null : v.versionId,
                        }))
                      }
                      style={chipButtonStyle(isBefore)}
                    >
                      Before
                    </button>
                    <button
                      type="button"
                      onClick={() =>
                        setDiffSelection((sel) => ({
                          ...sel,
                          after: isAfter ? null : v.versionId,
                        }))
                      }
                      style={chipButtonStyle(isAfter)}
                    >
                      After
                    </button>
                    <button
                      type="button"
                      onClick={() => {
                        void handleRestore(v.versionId);
                      }}
                      disabled={busy != null}
                      style={chipDangerStyle(
                        busy === `restore-${v.versionId}`,
                      )}
                    >
                      {busy === `restore-${v.versionId}`
                        ? "Restoring…"
                        : "Restore"}
                    </button>
                    <small
                      style={{
                        marginLeft: "auto",
                        fontFamily: "monospace",
                        color: colors.textMuted,
                        fontSize: 10,
                      }}
                    >
                      {v.versionId.slice(0, 8)}
                    </small>
                  </div>
                </li>
              );
            })}
          </ul>
        )}
      </section>

      {diff != null ? <DiffView diff={diff} /> : null}

      {error != null ? (
        <div
          style={{
            background: colors.dangerBgSoft,
            border: `1px solid ${colors.dangerBorder}`,
            color: colors.danger,
            padding: spacing.xs,
            borderRadius: radius.sm,
            display: "flex",
            justifyContent: "space-between",
            gap: 4,
            fontSize: 11,
          }}
        >
          <span>{error}</span>
          <button
            type="button"
            onClick={() => setError(null)}
            style={{
              background: "transparent",
              border: "none",
              color: colors.danger,
              cursor: "pointer",
              fontSize: 11,
            }}
            aria-label="Dismiss error"
          >
            ✕
          </button>
        </div>
      ) : null}
    </div>
  );
}

function DiffView({ diff }: { diff: BrandKitDiff }): JSX.Element {
  const empty =
    diff.added_colors.length === 0 &&
    diff.removed_colors.length === 0 &&
    diff.changed_colors.length === 0 &&
    diff.added_fonts.length === 0 &&
    diff.removed_fonts.length === 0 &&
    !diff.spacing_changed &&
    !diff.export_rules_changed &&
    diff.name_changed == null;
  return (
    <section
      style={{
        display: "flex",
        flexDirection: "column",
        gap: 4,
        padding: spacing.sm,
        background: colors.bgSoft,
        borderRadius: radius.md,
        border: `1px solid ${colors.border}`,
      }}
    >
      <strong style={{ fontSize: 12 }}>Diff</strong>
      {empty ? (
        <small style={{ color: colors.textMuted }}>
          No differences between the selected snapshots.
        </small>
      ) : (
        <>
          {diff.name_changed != null ? (
            <DiffRow
              label="Name"
              value={`${diff.name_changed[0]} → ${diff.name_changed[1]}`}
            />
          ) : null}
          {diff.added_colors.length > 0 ? (
            <ColorList
              label={`Added colours (${diff.added_colors.length})`}
              swatches={diff.added_colors}
              tone="success"
            />
          ) : null}
          {diff.removed_colors.length > 0 ? (
            <ColorList
              label={`Removed colours (${diff.removed_colors.length})`}
              swatches={diff.removed_colors}
              tone="danger"
            />
          ) : null}
          {diff.changed_colors.length > 0 ? (
            <ColorPairList
              label={`Changed colours (${diff.changed_colors.length})`}
              pairs={diff.changed_colors}
            />
          ) : null}
          {diff.added_fonts.length > 0 ? (
            <DiffRow
              label={`Added fonts (${diff.added_fonts.length})`}
              value={diff.added_fonts.join(", ")}
            />
          ) : null}
          {diff.removed_fonts.length > 0 ? (
            <DiffRow
              label={`Removed fonts (${diff.removed_fonts.length})`}
              value={diff.removed_fonts.join(", ")}
            />
          ) : null}
          {diff.spacing_changed ? (
            <DiffRow label="Spacing scale" value="changed" />
          ) : null}
          {diff.export_rules_changed ? (
            <DiffRow label="Export rules" value="changed" />
          ) : null}
        </>
      )}
    </section>
  );
}

function DiffRow({
  label,
  value,
}: {
  label: string;
  value: string;
}): JSX.Element {
  return (
    <div
      style={{
        display: "flex",
        justifyContent: "space-between",
        gap: spacing.sm,
        fontSize: 11,
      }}
    >
      <span style={{ color: colors.textMuted }}>{label}</span>
      <span style={{ color: colors.text }}>{value}</span>
    </div>
  );
}

function ColorList({
  label,
  swatches,
  tone,
}: {
  label: string;
  swatches: NamedColor[];
  tone: "success" | "danger";
}): JSX.Element {
  const colour = tone === "success" ? colors.success : colors.danger;
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 2 }}>
      <small style={{ color: colour, fontWeight: 600 }}>{label}</small>
      <div style={{ display: "flex", gap: 6, flexWrap: "wrap" }}>
        {swatches.map((s) => (
          <SwatchChip key={`${tone}-${s.name}`} swatch={s} />
        ))}
      </div>
    </div>
  );
}

function ColorPairList({
  label,
  pairs,
}: {
  label: string;
  pairs: Array<{ name: string; before: NamedColor; after: NamedColor }>;
}): JSX.Element {
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 2 }}>
      <small style={{ color: colors.warn, fontWeight: 600 }}>{label}</small>
      <ul
        style={{
          listStyle: "none",
          padding: 0,
          margin: 0,
          display: "flex",
          flexDirection: "column",
          gap: 2,
        }}
      >
        {pairs.map((p) => (
          <li
            key={`pair-${p.name}`}
            style={{
              display: "flex",
              alignItems: "center",
              gap: 6,
              fontSize: 11,
            }}
          >
            <span style={{ minWidth: 80, color: colors.text }}>{p.name}</span>
            <Swatch color={p.before.color} />
            <span style={{ color: colors.textMuted }}>→</span>
            <Swatch color={p.after.color} />
            <small style={{ color: colors.textMuted, fontFamily: "monospace" }}>
              {rgbaToHex(p.before.color)} → {rgbaToHex(p.after.color)}
            </small>
          </li>
        ))}
      </ul>
    </div>
  );
}

function SwatchChip({ swatch }: { swatch: NamedColor }): JSX.Element {
  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: 4,
        padding: 2,
        border: `1px solid ${colors.border}`,
        borderRadius: radius.sm,
      }}
    >
      <Swatch color={swatch.color} />
      <small style={{ fontSize: 10, color: colors.text }}>{swatch.name}</small>
      <small
        style={{
          fontSize: 10,
          color: colors.textMuted,
          fontFamily: "monospace",
        }}
      >
        {rgbaToHex(swatch.color)}
      </small>
    </div>
  );
}

function Swatch({ color }: { color: RgbaColor }): JSX.Element {
  return (
    <span
      style={{
        display: "inline-block",
        width: 14,
        height: 14,
        borderRadius: 3,
        border: `1px solid ${colors.border}`,
        background: rgbaToCss(color),
      }}
      aria-hidden
    />
  );
}

function rgbaToHex({ r, g, b }: RgbaColor): string {
  const c = (v: number): string =>
    Math.max(0, Math.min(255, Math.round(v * 255)))
      .toString(16)
      .padStart(2, "0");
  return `#${c(r)}${c(g)}${c(b)}`;
}

function rgbaToCss({ r, g, b, a }: RgbaColor): string {
  const clamp = (v: number): number =>
    Math.max(0, Math.min(255, Math.round(v * 255)));
  return `rgba(${clamp(r)}, ${clamp(g)}, ${clamp(b)}, ${a.toFixed(3)})`;
}

function formatTimestamp(iso: string): string {
  try {
    const dt = new Date(iso);
    if (Number.isNaN(dt.getTime())) return iso;
    return dt.toLocaleString();
  } catch {
    return iso;
  }
}

function emptyDiff(): BrandKitDiff {
  return {
    added_colors: [],
    removed_colors: [],
    changed_colors: [],
    added_fonts: [],
    removed_fonts: [],
    spacing_changed: false,
    export_rules_changed: false,
    name_changed: null,
  };
}

function errMsg(e: unknown): string {
  if (e instanceof Error) return e.message;
  return String(e);
}

const inputStyle: React.CSSProperties = {
  width: "100%",
  padding: 6,
  fontSize: 12,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.sm,
  background: colors.bg,
  color: colors.text,
  boxSizing: "border-box",
};

const selectStyle: React.CSSProperties = {
  ...inputStyle,
  padding: 4,
};

const fieldLabelStyle: React.CSSProperties = {
  display: "flex",
  flexDirection: "column",
  gap: 2,
  fontSize: 11,
  fontWeight: 600,
  color: colors.textMuted,
};

function primaryButtonStyle(active: boolean): React.CSSProperties {
  return {
    padding: "6px 10px",
    fontSize: 12,
    fontWeight: 600,
    background: colors.accent,
    color: colors.textInverse,
    border: `1px solid ${colors.accent}`,
    borderRadius: radius.pill,
    cursor: active ? "wait" : "pointer",
  };
}

function chipButtonStyle(active: boolean): React.CSSProperties {
  return {
    padding: "1px 8px",
    fontSize: 10,
    fontWeight: 600,
    background: active ? colors.accent : "transparent",
    color: active ? colors.textInverse : colors.accent,
    border: `1px solid ${colors.accent}`,
    borderRadius: radius.pill,
    cursor: "pointer",
  };
}

function chipDangerStyle(active: boolean): React.CSSProperties {
  return {
    padding: "1px 8px",
    fontSize: 10,
    fontWeight: 600,
    background: active ? colors.danger : "transparent",
    color: active ? colors.textInverse : colors.danger,
    border: `1px solid ${colors.danger}`,
    borderRadius: radius.pill,
    cursor: active ? "wait" : "pointer",
  };
}
