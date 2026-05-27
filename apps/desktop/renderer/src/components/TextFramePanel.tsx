// TextFramePanel — surfaces the Phase 2 `TextFrameOptions` for the
// currently-selected `TextLayer` and lets the user commit changes
// through `window.kcreate.textFrame`.
//
// Activating multiple columns / hyphenation here is what flips the
// `kcreate_text::paragraph::layout_paragraph` solver into its
// multi-column + hyphenation-aware branches. Toggles are wired
// straight to a single `update(options)` call so undo / redo behaves
// the same way as for `text_frame_update` in Rust tests.

import { useCallback, useEffect, useState } from "react";

import type {
  NodeInfo,
  TextAutoSize,
  TextFrameOptions,
  TextOverflow,
  VerticalAlign,
} from "../../../shared/scene";
import { colors, radius, spacing } from "../styles/tokens";

const OVERFLOW_OPTIONS: ReadonlyArray<{ value: TextOverflow; label: string }> = [
  { value: "clip", label: "Clip" },
  { value: "ellipsis", label: "Ellipsis" },
  { value: "overflow", label: "Overflow (visible)" },
];

const VALIGN_OPTIONS: ReadonlyArray<{ value: VerticalAlign; label: string }> = [
  { value: "top", label: "Top" },
  { value: "middle", label: "Middle" },
  { value: "bottom", label: "Bottom" },
];

const AUTOSIZE_OPTIONS: ReadonlyArray<{ value: TextAutoSize; label: string }> =
  [
    { value: "fixed", label: "Fixed" },
    { value: "height_auto", label: "Height auto" },
    { value: "width_and_height_auto", label: "W + H auto" },
  ];

// The languages with embedded TeX hyphenation patterns. Today only
// `en-US` ships in `kcreate_text::EN_US_PATTERNS`; the other entries
// are accepted by the bridge but fall through to no-hyphenation
// (matches the Task 8 design — patterns are pluggable per language).
const HYPHEN_LANGS: ReadonlyArray<{ value: string; label: string }> = [
  { value: "en-US", label: "English (US)" },
  { value: "de-DE", label: "German" },
  { value: "fr-FR", label: "French" },
  { value: "es-ES", label: "Spanish" },
  { value: "pt-PT", label: "Portuguese" },
  { value: "it-IT", label: "Italian" },
];

const MIN_COLUMNS = 1;
const MAX_COLUMNS = 6;

export interface TextFramePanelProps {
  node: NodeInfo;
  onStatus?: (msg: string | null) => void;
}

export function TextFramePanel({
  node,
  onStatus,
}: TextFramePanelProps): JSX.Element {
  const [options, setOptions] = useState<TextFrameOptions | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [linkInsets, setLinkInsets] = useState(true);

  const load = useCallback(
    async (id: string) => {
      setError(null);
      try {
        const next = await window.kcreate.textFrame.get(id);
        setOptions(next);
      } catch (e) {
        setError(errMsg(e));
      }
    },
    [],
  );

  // Dependency on `node.version` (not just `node.id`) so undo/redo
  // and collab edits on the same selected node refire the hydrate
  // path. Without it, the panel would silently show pre-undo state
  // and the user's next commit would clobber the just-undone fields
  // — see PR #12 Devin Review thread on RightPanel.tsx:549 (same
  // architectural gap was filed against TextFramePanel and
  // OpenTypePanel in parallel; this is the uniform fix).
  useEffect(() => {
    void load(node.id);
  }, [load, node.id, node.version]);

  const commit = useCallback(
    async (next: TextFrameOptions) => {
      setBusy(true);
      onStatus?.("Text frame: updating…");
      try {
        await window.kcreate.textFrame.update(node.id, next);
        setOptions(next);
        onStatus?.("Text frame: updated.");
        setError(null);
      } catch (e) {
        const msg = errMsg(e);
        setError(msg);
        onStatus?.(`Text frame: update failed — ${msg}`);
      } finally {
        setBusy(false);
      }
    },
    [node.id, onStatus],
  );

  if (!options) {
    return (
      <div style={{ display: "flex", flexDirection: "column", gap: spacing.sm }}>
        <h4 style={sectionHeading}>Text frame</h4>
        {error ? (
          <p style={errorText}>{error}</p>
        ) : (
          <p style={mutedText}>Loading…</p>
        )}
      </div>
    );
  }

  // Inset helpers — when "link all" is on, editing one field
  // propagates to all four. When off, each side is editable
  // independently.
  const setInset = (side: "top" | "right" | "bottom" | "left", v: number) => {
    if (linkInsets) {
      void commit({
        ...options,
        inset: { top: v, right: v, bottom: v, left: v },
      });
    } else {
      void commit({
        ...options,
        inset: { ...options.inset, [side]: v },
      });
    }
  };

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: spacing.md }}>
      <h4 style={sectionHeading}>Text frame</h4>

      <Row>
        <NumberField
          label="Columns"
          value={options.columns}
          min={MIN_COLUMNS}
          max={MAX_COLUMNS}
          step={1}
          disabled={busy}
          onChange={(v) =>
            void commit({
              ...options,
              columns: clamp(Math.round(v), MIN_COLUMNS, MAX_COLUMNS),
            })
          }
        />
        <NumberField
          label="Column gap"
          value={options.column_gap}
          min={0}
          step={1}
          disabled={busy}
          onChange={(v) => void commit({ ...options, column_gap: v })}
        />
      </Row>

      <SelectField
        label="Overflow"
        value={options.overflow}
        options={OVERFLOW_OPTIONS}
        disabled={busy}
        onChange={(v) =>
          void commit({ ...options, overflow: v as TextOverflow })
        }
      />

      <ButtonGroup<VerticalAlign>
        label="Vertical alignment"
        value={options.vertical_alignment}
        options={VALIGN_OPTIONS}
        disabled={busy}
        onChange={(v) => void commit({ ...options, vertical_alignment: v })}
      />

      <ButtonGroup<TextAutoSize>
        label="Auto-size"
        value={options.auto_size}
        options={AUTOSIZE_OPTIONS}
        disabled={busy}
        onChange={(v) => void commit({ ...options, auto_size: v })}
      />

      <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
        <CheckboxField
          label="Hyphenation"
          checked={options.hyphenation}
          disabled={busy}
          onChange={(v) => void commit({ ...options, hyphenation: v })}
        />
        <SelectField
          label="Hyphenation language"
          value={options.hyphenation_language}
          options={HYPHEN_LANGS}
          disabled={busy || !options.hyphenation}
          onChange={(v) =>
            void commit({ ...options, hyphenation_language: v })
          }
        />
        {options.hyphenation && !options.hyphenation_language.startsWith("en") ? (
          <p style={hintText}>
            Only English hyphenation patterns ship today; other languages
            fall through to no hyphenation until their patterns are
            bundled.
          </p>
        ) : null}
      </div>

      <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
        <div style={insetHeaderRow}>
          <span style={fieldLabel}>Frame inset</span>
          <CheckboxField
            label="Link all"
            checked={linkInsets}
            disabled={busy}
            onChange={setLinkInsets}
          />
        </div>
        <Row>
          <NumberField
            label="Top"
            value={options.inset.top}
            disabled={busy}
            onChange={(v) => setInset("top", v)}
          />
          <NumberField
            label="Right"
            value={options.inset.right}
            disabled={busy || linkInsets}
            onChange={(v) => setInset("right", v)}
          />
        </Row>
        <Row>
          <NumberField
            label="Bottom"
            value={options.inset.bottom}
            disabled={busy || linkInsets}
            onChange={(v) => setInset("bottom", v)}
          />
          <NumberField
            label="Left"
            value={options.inset.left}
            disabled={busy || linkInsets}
            onChange={(v) => setInset("left", v)}
          />
        </Row>
      </div>

      {error ? <p style={errorText}>{error}</p> : null}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Layout primitives — kept local because they're presentationally identical
// to the equivalents in ColorSettingsPanel / RightPanel but typed against
// `TextFrameOptions` rather than a `string` value union.
// ---------------------------------------------------------------------------

function Row({ children }: { children: React.ReactNode }): JSX.Element {
  return (
    <div style={{ display: "flex", gap: spacing.xs, alignItems: "flex-end" }}>
      {children}
    </div>
  );
}

function NumberField({
  label,
  value,
  min,
  max,
  step,
  disabled,
  onChange,
}: {
  label: string;
  value: number;
  min?: number;
  max?: number;
  step?: number;
  disabled?: boolean;
  onChange: (v: number) => void;
}): JSX.Element {
  return (
    <label style={fieldStack}>
      <span style={fieldLabel}>{label}</span>
      <input
        type="number"
        value={value}
        min={min}
        max={max}
        step={step ?? 1}
        disabled={disabled}
        onChange={(e) => {
          const next = Number(e.target.value);
          if (Number.isFinite(next)) onChange(next);
        }}
        style={inputStyle}
      />
    </label>
  );
}

function SelectField<V extends string>({
  label,
  value,
  options,
  disabled,
  onChange,
}: {
  label: string;
  value: V;
  options: ReadonlyArray<{ value: V; label: string }>;
  disabled?: boolean;
  onChange: (v: V) => void;
}): JSX.Element {
  return (
    <label style={fieldStack}>
      <span style={fieldLabel}>{label}</span>
      <select
        value={value}
        disabled={disabled}
        onChange={(e) => onChange(e.target.value as V)}
        style={inputStyle}
      >
        {options.map((o) => (
          <option key={o.value} value={o.value}>
            {o.label}
          </option>
        ))}
      </select>
    </label>
  );
}

function ButtonGroup<V extends string>({
  label,
  value,
  options,
  disabled,
  onChange,
}: {
  label: string;
  value: V;
  options: ReadonlyArray<{ value: V; label: string }>;
  disabled?: boolean;
  onChange: (v: V) => void;
}): JSX.Element {
  return (
    <div style={fieldStack}>
      <span style={fieldLabel}>{label}</span>
      <div style={{ display: "flex", gap: 2 }} role="group" aria-label={label}>
        {options.map((o) => {
          const selected = o.value === value;
          return (
            <button
              key={o.value}
              type="button"
              disabled={disabled}
              onClick={() => onChange(o.value)}
              style={{
                ...buttonStyle,
                background: selected ? colors.bgSoft : "transparent",
                color: selected ? colors.accent : colors.text,
                cursor: disabled ? "default" : "pointer",
                opacity: disabled ? 0.6 : 1,
              }}
            >
              {o.label}
            </button>
          );
        })}
      </div>
    </div>
  );
}

function CheckboxField({
  label,
  checked,
  disabled,
  onChange,
}: {
  label: string;
  checked: boolean;
  disabled?: boolean;
  onChange: (v: boolean) => void;
}): JSX.Element {
  return (
    <label
      style={{ display: "flex", alignItems: "center", gap: spacing.xs }}
    >
      <input
        type="checkbox"
        checked={checked}
        disabled={disabled}
        onChange={(e) => onChange(e.target.checked)}
      />
      <span style={fieldLabel}>{label}</span>
    </label>
  );
}

const fieldStack: React.CSSProperties = {
  display: "flex",
  flexDirection: "column",
  gap: 4,
  flex: 1,
};

const fieldLabel: React.CSSProperties = {
  fontSize: 11,
  color: colors.textMuted,
};

const inputStyle: React.CSSProperties = {
  padding: "4px 6px",
  fontSize: 12,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.card,
  background: colors.bg,
  color: colors.text,
  width: "100%",
};

const buttonStyle: React.CSSProperties = {
  padding: "4px 8px",
  fontSize: 11,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.card,
};

const sectionHeading: React.CSSProperties = {
  margin: 0,
  fontSize: 13,
  fontWeight: 600,
};

const insetHeaderRow: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  justifyContent: "space-between",
};

const mutedText: React.CSSProperties = {
  color: colors.textMuted,
  fontSize: 12,
  margin: 0,
};

const errorText: React.CSSProperties = {
  color: colors.danger,
  fontSize: 12,
  margin: 0,
};

const hintText: React.CSSProperties = {
  color: colors.textMuted,
  fontSize: 11,
  margin: 0,
  fontStyle: "italic",
};

function clamp(v: number, lo: number, hi: number): number {
  if (v < lo) return lo;
  if (v > hi) return hi;
  return v;
}

function errMsg(e: unknown): string {
  if (e instanceof Error) return e.message;
  if (typeof e === "string") return e;
  return JSON.stringify(e);
}
