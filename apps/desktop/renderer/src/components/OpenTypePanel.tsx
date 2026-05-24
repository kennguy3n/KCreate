// OpenTypePanel — surfaces the Phase 2 `OpenTypeFeatures` for the
// selected `TextLayer` and lets the user toggle individual features
// or stylistic sets.
//
// Each toggle flips a single bool on `OpenTypeFeatures` and commits
// the whole struct through `window.kcreate.textFrame
// .updateOpenTypeFeatures`. The Rust encoder
// (`opentype_features_to_buzz`) always emits all 9 boolean features
// — disabling a toggle here writes a `value=0` feature entry that
// suppresses the font's default, which matches typographer
// expectations (e.g. turning off `kern` actually disables kerning).
//
// Stylistic sets are stored as a sparse `Vec<u8>` of 1..=20 indices
// — the Rust side silently drops any out-of-range entries.

import { useCallback, useEffect, useState } from "react";

import type { NodeInfo, OpenTypeFeatures } from "../../../shared/scene";
import { colors, radius, spacing } from "../styles/tokens";

const FEATURE_TOGGLES: ReadonlyArray<{
  key: keyof Omit<OpenTypeFeatures, "stylistic_sets">;
  label: string;
  hint: string;
}> = [
  { key: "ligatures", label: "Ligatures", hint: "liga, clig" },
  { key: "contextual_alternates", label: "Contextual alternates", hint: "calt" },
  { key: "kerning", label: "Kerning", hint: "kern" },
  { key: "small_caps", label: "Small caps", hint: "smcp" },
  { key: "old_style_figures", label: "Old-style figures", hint: "onum" },
  { key: "tabular_figures", label: "Tabular figures", hint: "tnum" },
  { key: "fractions", label: "Fractions", hint: "frac" },
  { key: "ordinals", label: "Ordinals", hint: "ordn" },
];

const STYLISTIC_SET_INDICES: ReadonlyArray<number> = Array.from(
  { length: 20 },
  (_, i) => i + 1,
);

export interface OpenTypePanelProps {
  node: NodeInfo;
  onStatus?: (msg: string | null) => void;
}

export function OpenTypePanel({
  node,
  onStatus,
}: OpenTypePanelProps): JSX.Element {
  const [features, setFeatures] = useState<OpenTypeFeatures | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async (id: string) => {
    setError(null);
    try {
      const next = await window.kcreate.textFrame.getOpenTypeFeatures(id);
      setFeatures(next);
    } catch (e) {
      setError(errMsg(e));
    }
  }, []);

  // Dependency on `node.version` (not just `node.id`) so undo/redo
  // and collab edits on the same selected node refire the hydrate
  // path. See `TextFramePanel` and `FillSection` for the matching
  // pattern — PR #12 Devin Review filed this gap once against
  // `FillSection`, but the same shape exists across every bridge-
  // hydrating panel, so we fix it uniformly.
  useEffect(() => {
    void load(node.id);
  }, [load, node.id, node.version]);

  const commit = useCallback(
    async (next: OpenTypeFeatures) => {
      setBusy(true);
      onStatus?.("OpenType: updating…");
      try {
        await window.kcreate.textFrame.updateOpenTypeFeatures(node.id, next);
        setFeatures(next);
        onStatus?.("OpenType: updated.");
        setError(null);
      } catch (e) {
        const msg = errMsg(e);
        setError(msg);
        onStatus?.(`OpenType: update failed — ${msg}`);
      } finally {
        setBusy(false);
      }
    },
    [node.id, onStatus],
  );

  if (!features) {
    return (
      <div style={{ display: "flex", flexDirection: "column", gap: spacing.sm }}>
        <h4 style={sectionHeading}>OpenType features</h4>
        {error ? (
          <p style={errorText}>{error}</p>
        ) : (
          <p style={mutedText}>Loading…</p>
        )}
      </div>
    );
  }

  const toggleSs = (idx: number, on: boolean) => {
    const next = new Set(features.stylistic_sets);
    if (on) next.add(idx);
    else next.delete(idx);
    const sorted = Array.from(next).sort((a, b) => a - b);
    void commit({ ...features, stylistic_sets: sorted });
  };

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: spacing.md }}>
      <h4 style={sectionHeading}>OpenType features</h4>

      <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
        {FEATURE_TOGGLES.map((f) => (
          <ToggleRow
            key={f.key}
            label={f.label}
            hint={f.hint}
            checked={features[f.key]}
            disabled={busy}
            onChange={(v) => void commit({ ...features, [f.key]: v })}
          />
        ))}
      </div>

      <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
        <span style={fieldLabel}>Stylistic sets (ss01–ss20)</span>
        <div style={ssGrid}>
          {STYLISTIC_SET_INDICES.map((idx) => {
            const active = features.stylistic_sets.includes(idx);
            return (
              <button
                key={idx}
                type="button"
                aria-pressed={active}
                disabled={busy}
                onClick={() => toggleSs(idx, !active)}
                style={{
                  ...ssButtonStyle,
                  background: active ? colors.bgSoft : "transparent",
                  color: active ? colors.accent : colors.text,
                  cursor: busy ? "default" : "pointer",
                  opacity: busy ? 0.6 : 1,
                }}
                title={`ss${idx.toString().padStart(2, "0")}`}
              >
                {idx.toString().padStart(2, "0")}
              </button>
            );
          })}
        </div>
      </div>

      {error ? <p style={errorText}>{error}</p> : null}
    </div>
  );
}

function ToggleRow({
  label,
  hint,
  checked,
  disabled,
  onChange,
}: {
  label: string;
  hint: string;
  checked: boolean;
  disabled?: boolean;
  onChange: (v: boolean) => void;
}): JSX.Element {
  return (
    <label style={toggleRow}>
      <span style={toggleLabel}>
        {label}
        <span style={hintText}> · {hint}</span>
      </span>
      <input
        type="checkbox"
        checked={checked}
        disabled={disabled}
        onChange={(e) => onChange(e.target.checked)}
      />
    </label>
  );
}

const fieldLabel: React.CSSProperties = {
  fontSize: 11,
  color: colors.textMuted,
};

const toggleRow: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  justifyContent: "space-between",
  gap: spacing.xs,
};

const toggleLabel: React.CSSProperties = {
  fontSize: 12,
  color: colors.text,
};

const hintText: React.CSSProperties = {
  fontSize: 11,
  color: colors.textMuted,
  fontStyle: "italic",
};

const sectionHeading: React.CSSProperties = {
  margin: 0,
  fontSize: 13,
  fontWeight: 600,
};

const ssGrid: React.CSSProperties = {
  display: "grid",
  gridTemplateColumns: "repeat(5, 1fr)",
  gap: 4,
};

const ssButtonStyle: React.CSSProperties = {
  padding: "4px 0",
  fontSize: 11,
  fontFamily: "ui-monospace, monospace",
  border: `1px solid ${colors.border}`,
  borderRadius: radius.card,
  textAlign: "center",
};

const mutedText: React.CSSProperties = {
  color: colors.textMuted,
  fontSize: 12,
  margin: 0,
};

const errorText: React.CSSProperties = {
  color: "#DC2626",
  fontSize: 12,
  margin: 0,
};

function errMsg(e: unknown): string {
  if (e instanceof Error) return e.message;
  if (typeof e === "string") return e;
  return JSON.stringify(e);
}
