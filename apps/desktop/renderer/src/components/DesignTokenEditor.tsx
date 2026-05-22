// DesignTokenEditor — project-wide token CRUD panel (Block G Task 23).
//
// Wraps `window.kcreate.designTokens.{get,set}` with a four-section
// editor: colors, typography, spacing, shadows. Add / rename / delete
// per section; per-row "Apply to Selection" calls the canvas bridge to
// stamp the token onto the currently selected node's metadata. The
// setter does NOT persist on its own — the user has to save the
// project after editing to land tokens on disk; the bridge is
// idempotent so re-saving is safe.

import { useCallback, useEffect, useState } from "react";

import type {
  DesignTokens,
  RgbaColor,
  ShadowToken,
  TypographyToken,
} from "../../../shared/scene";
import { colors, radius, spacing } from "../styles/tokens";

export interface DesignTokenEditorProps {
  selectedNodeId: string | null;
  onStatus: (msg: string | null) => void;
}

function emptyTokens(): DesignTokens {
  return { colors: {}, typography: {}, spacing: {}, radii: {}, shadows: {} };
}

function rgbaToHex({ r, g, b }: RgbaColor): string {
  const c = (v: number): string =>
    Math.max(0, Math.min(255, Math.round(v * 255)))
      .toString(16)
      .padStart(2, "0");
  return `#${c(r)}${c(g)}${c(b)}`;
}

function hexToRgba(hex: string, a = 1): RgbaColor {
  const m = /^#([0-9a-f]{6})$/i.exec(hex.trim());
  if (!m || !m[1]) return { r: 0, g: 0, b: 0, a };
  const v = parseInt(m[1], 16);
  return {
    r: ((v >> 16) & 0xff) / 255,
    g: ((v >> 8) & 0xff) / 255,
    b: (v & 0xff) / 255,
    a,
  };
}

const defaultTypography: TypographyToken = {
  font_family: "Inter",
  font_weight: 400,
  font_size: 16,
  line_height: 1.4,
  letter_spacing: 0,
};

const defaultShadow: ShadowToken = {
  offset_x: 0,
  offset_y: 2,
  blur: 6,
  spread: 0,
  color: { r: 0, g: 0, b: 0, a: 0.25 },
};

export function DesignTokenEditor({
  selectedNodeId,
  onStatus,
}: DesignTokenEditorProps): JSX.Element {
  const [tokens, setTokens] = useState<DesignTokens>(emptyTokens);
  const [busy, setBusy] = useState(false);

  const reload = useCallback(() => {
    void (async () => {
      try {
        const next = await window.kcreate.designTokens.get();
        setTokens(next);
      } catch (e) {
        onStatus(`Design tokens: load failed (${errMsg(e)})`);
      }
    })();
  }, [onStatus]);

  useEffect(() => {
    reload();
  }, [reload]);

  const persist = useCallback(
    async (next: DesignTokens) => {
      setBusy(true);
      try {
        await window.kcreate.designTokens.set(next);
        setTokens(next);
      } catch (e) {
        onStatus(`Design tokens: save failed (${errMsg(e)})`);
      } finally {
        setBusy(false);
      }
    },
    [onStatus],
  );

  const applyTokenToSelection = useCallback(
    async (key: string, label: string) => {
      if (!selectedNodeId) {
        onStatus("Select a layer to apply a token.");
        return;
      }
      // Tokens persist via the canvas bridge's `updateNode` metadata
      // patch. The Rust side records this as a regular op so it
      // shows up in undo / redo.
      try {
        await window.kcreate.document.updateNode(selectedNodeId, {
          metadata: { [key]: label },
        });
        onStatus(`Applied ${key}="${label}" to ${selectedNodeId.slice(0, 8)}.`);
      } catch (e) {
        onStatus(`Apply failed: ${errMsg(e)}`);
      }
    },
    [selectedNodeId, onStatus],
  );

  return (
    <div style={panelStyle}>
      <Header title="Design tokens" busy={busy} />
      <Section label="Colors">
        {Object.entries(tokens.colors).map(([name, col]) => (
          <ColorRow
            key={name}
            name={name}
            value={col}
            onRename={(next) => {
              if (!next || next === name || tokens.colors[next]) return;
              const value = tokens.colors[name];
              if (!value) return;
              const out = { ...tokens.colors };
              out[next] = value;
              delete out[name];
              void persist({ ...tokens, colors: out });
            }}
            onChange={(next) => {
              void persist({
                ...tokens,
                colors: { ...tokens.colors, [name]: next },
              });
            }}
            onDelete={() => {
              const out = { ...tokens.colors };
              delete out[name];
              void persist({ ...tokens, colors: out });
            }}
            onApply={() => {
              void applyTokenToSelection("designToken.color", name);
            }}
          />
        ))}
        <AddRow
          placeholder="new color name"
          onAdd={(name) => {
            if (tokens.colors[name]) return;
            void persist({
              ...tokens,
              colors: { ...tokens.colors, [name]: hexToRgba("#888888") },
            });
          }}
        />
      </Section>

      <Section label="Typography">
        {Object.entries(tokens.typography).map(([name, t]) => (
          <TypographyRow
            key={name}
            name={name}
            value={t}
            onChange={(next) => {
              void persist({
                ...tokens,
                typography: { ...tokens.typography, [name]: next },
              });
            }}
            onDelete={() => {
              const out = { ...tokens.typography };
              delete out[name];
              void persist({ ...tokens, typography: out });
            }}
            onApply={() => {
              void applyTokenToSelection("designToken.typography", name);
            }}
          />
        ))}
        <AddRow
          placeholder="new typography token"
          onAdd={(name) => {
            if (tokens.typography[name]) return;
            void persist({
              ...tokens,
              typography: { ...tokens.typography, [name]: defaultTypography },
            });
          }}
        />
      </Section>

      <Section label="Spacing">
        {Object.entries(tokens.spacing).map(([name, v]) => (
          <NumberRow
            key={name}
            name={name}
            value={v}
            onChange={(next) => {
              void persist({
                ...tokens,
                spacing: { ...tokens.spacing, [name]: next },
              });
            }}
            onDelete={() => {
              const out = { ...tokens.spacing };
              delete out[name];
              void persist({ ...tokens, spacing: out });
            }}
            onApply={() => {
              void applyTokenToSelection("designToken.spacing", name);
            }}
          />
        ))}
        <AddRow
          placeholder="new spacing token"
          onAdd={(name) => {
            if (tokens.spacing[name] !== undefined) return;
            void persist({
              ...tokens,
              spacing: { ...tokens.spacing, [name]: 8 },
            });
          }}
        />
      </Section>

      <Section label="Shadows">
        {Object.entries(tokens.shadows).map(([name, s]) => (
          <ShadowRow
            key={name}
            name={name}
            value={s}
            onChange={(next) => {
              void persist({
                ...tokens,
                shadows: { ...tokens.shadows, [name]: next },
              });
            }}
            onDelete={() => {
              const out = { ...tokens.shadows };
              delete out[name];
              void persist({ ...tokens, shadows: out });
            }}
            onApply={() => {
              void applyTokenToSelection("designToken.shadow", name);
            }}
          />
        ))}
        <AddRow
          placeholder="new shadow token"
          onAdd={(name) => {
            if (tokens.shadows[name]) return;
            void persist({
              ...tokens,
              shadows: { ...tokens.shadows, [name]: defaultShadow },
            });
          }}
        />
      </Section>
    </div>
  );
}

function errMsg(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

function Header({
  title,
  busy,
}: {
  title: string;
  busy: boolean;
}): JSX.Element {
  return (
    <div
      style={{
        display: "flex",
        alignItems: "baseline",
        justifyContent: "space-between",
        marginBottom: spacing.sm,
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
        {title}
      </h2>
      <span style={{ fontSize: 10, color: colors.textMuted }}>
        {busy ? "Saving…" : ""}
      </span>
    </div>
  );
}

function Section({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}): JSX.Element {
  return (
    <section
      style={{
        marginBottom: spacing.md,
        display: "flex",
        flexDirection: "column",
        gap: spacing.xs,
      }}
    >
      <h3 style={sectionHeaderStyle}>{label}</h3>
      {children}
    </section>
  );
}

function ColorRow({
  name,
  value,
  onRename,
  onChange,
  onDelete,
  onApply,
}: {
  name: string;
  value: RgbaColor;
  onRename: (next: string) => void;
  onChange: (next: RgbaColor) => void;
  onDelete: () => void;
  onApply: () => void;
}): JSX.Element {
  return (
    <div style={rowStyle}>
      <input
        type="text"
        defaultValue={name}
        onBlur={(e) => onRename(e.target.value)}
        style={{ ...inputStyle, width: 110 }}
      />
      <input
        type="color"
        value={rgbaToHex(value)}
        onChange={(e) => onChange(hexToRgba(e.target.value, value.a))}
        style={{
          width: 28,
          height: 24,
          border: `1px solid ${colors.border}`,
          background: "none",
          padding: 0,
        }}
      />
      <input
        type="number"
        min={0}
        max={1}
        step={0.05}
        value={value.a}
        onChange={(e) => onChange({ ...value, a: Number(e.target.value) || 0 })}
        style={{ ...inputStyle, width: 52 }}
        title="alpha"
      />
      <button type="button" style={chipBtn(false)} onClick={onApply}>
        Apply
      </button>
      <button type="button" style={dangerBtn} onClick={onDelete} title="delete">
        ×
      </button>
    </div>
  );
}

function TypographyRow({
  name,
  value,
  onChange,
  onDelete,
  onApply,
}: {
  name: string;
  value: TypographyToken;
  onChange: (next: TypographyToken) => void;
  onDelete: () => void;
  onApply: () => void;
}): JSX.Element {
  return (
    <div style={{ ...rowStyle, flexWrap: "wrap" }}>
      <input
        type="text"
        value={value.font_family}
        onChange={(e) => onChange({ ...value, font_family: e.target.value })}
        style={{ ...inputStyle, width: 100 }}
        title={name}
      />
      <input
        type="number"
        min={100}
        max={900}
        step={100}
        value={value.font_weight}
        onChange={(e) =>
          onChange({ ...value, font_weight: Number(e.target.value) || 400 })
        }
        style={{ ...inputStyle, width: 56 }}
        title="weight"
      />
      <input
        type="number"
        min={6}
        max={144}
        step={1}
        value={value.font_size}
        onChange={(e) =>
          onChange({ ...value, font_size: Number(e.target.value) || 16 })
        }
        style={{ ...inputStyle, width: 56 }}
        title="size px"
      />
      <button type="button" style={chipBtn(false)} onClick={onApply}>
        Apply
      </button>
      <button type="button" style={dangerBtn} onClick={onDelete}>
        ×
      </button>
    </div>
  );
}

function NumberRow({
  name,
  value,
  onChange,
  onDelete,
  onApply,
}: {
  name: string;
  value: number;
  onChange: (next: number) => void;
  onDelete: () => void;
  onApply: () => void;
}): JSX.Element {
  return (
    <div style={rowStyle}>
      <span style={{ ...labelStyle, width: 110 }}>{name}</span>
      <input
        type="number"
        value={value}
        onChange={(e) => onChange(Number(e.target.value) || 0)}
        style={{ ...inputStyle, width: 72 }}
      />
      <button type="button" style={chipBtn(false)} onClick={onApply}>
        Apply
      </button>
      <button type="button" style={dangerBtn} onClick={onDelete}>
        ×
      </button>
    </div>
  );
}

function ShadowRow({
  name,
  value,
  onChange,
  onDelete,
  onApply,
}: {
  name: string;
  value: ShadowToken;
  onChange: (next: ShadowToken) => void;
  onDelete: () => void;
  onApply: () => void;
}): JSX.Element {
  return (
    <div style={{ ...rowStyle, flexWrap: "wrap" }}>
      <span style={{ ...labelStyle, width: 90 }}>{name}</span>
      <input
        type="number"
        value={value.offset_x}
        onChange={(e) =>
          onChange({ ...value, offset_x: Number(e.target.value) || 0 })
        }
        style={{ ...inputStyle, width: 48 }}
        title="x"
      />
      <input
        type="number"
        value={value.offset_y}
        onChange={(e) =>
          onChange({ ...value, offset_y: Number(e.target.value) || 0 })
        }
        style={{ ...inputStyle, width: 48 }}
        title="y"
      />
      <input
        type="number"
        value={value.blur}
        onChange={(e) =>
          onChange({ ...value, blur: Number(e.target.value) || 0 })
        }
        style={{ ...inputStyle, width: 48 }}
        title="blur"
      />
      <input
        type="color"
        value={rgbaToHex(value.color)}
        onChange={(e) =>
          onChange({
            ...value,
            color: hexToRgba(e.target.value, value.color.a),
          })
        }
        style={{
          width: 28,
          height: 24,
          border: `1px solid ${colors.border}`,
          background: "none",
          padding: 0,
        }}
      />
      <button type="button" style={chipBtn(false)} onClick={onApply}>
        Apply
      </button>
      <button type="button" style={dangerBtn} onClick={onDelete}>
        ×
      </button>
    </div>
  );
}

function AddRow({
  placeholder,
  onAdd,
}: {
  placeholder: string;
  onAdd: (name: string) => void;
}): JSX.Element {
  const [value, setValue] = useState("");
  return (
    <form
      onSubmit={(e) => {
        e.preventDefault();
        const name = value.trim();
        if (!name) return;
        onAdd(name);
        setValue("");
      }}
      style={{
        display: "flex",
        gap: spacing.xs,
        marginTop: spacing.xs,
      }}
    >
      <input
        type="text"
        placeholder={placeholder}
        value={value}
        onChange={(e) => setValue(e.target.value)}
        style={{ ...inputStyle, flex: 1 }}
      />
      <button type="submit" style={primaryBtn(!value.trim())}>
        +
      </button>
    </form>
  );
}

const panelStyle: React.CSSProperties = {
  padding: spacing.sm,
  display: "flex",
  flexDirection: "column",
  overflowY: "auto",
};

const sectionHeaderStyle: React.CSSProperties = {
  margin: 0,
  fontSize: 11,
  fontWeight: 600,
  textTransform: "uppercase",
  letterSpacing: 0.4,
  color: colors.textMuted,
};

const rowStyle: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: spacing.xs,
  padding: "4px 0",
};

const inputStyle: React.CSSProperties = {
  background: colors.bgSoft,
  color: colors.text,
  border: `1px solid ${colors.border}`,
  borderRadius: 4,
  padding: "4px 6px",
  fontSize: 11,
  fontFamily: "inherit",
};

const labelStyle: React.CSSProperties = {
  fontSize: 11,
  color: colors.text,
  fontFamily: "inherit",
};

function chipBtn(active: boolean): React.CSSProperties {
  return {
    padding: "3px 8px",
    fontSize: 10,
    fontWeight: 500,
    background: active ? colors.accent : colors.bgSoft,
    color: active ? colors.textInverse : colors.text,
    border: `1px solid ${active ? colors.accent : colors.border}`,
    borderRadius: radius.pill,
    cursor: "pointer",
  };
}

const dangerBtn: React.CSSProperties = {
  padding: "0 6px",
  fontSize: 14,
  lineHeight: "20px",
  background: "transparent",
  color: colors.textMuted,
  border: `1px solid ${colors.border}`,
  borderRadius: 4,
  cursor: "pointer",
};

function primaryBtn(disabled: boolean): React.CSSProperties {
  return {
    padding: "4px 10px",
    fontSize: 11,
    fontWeight: 600,
    background: disabled ? colors.bgSoft : colors.accent,
    color: disabled ? colors.textMuted : colors.textInverse,
    border: "none",
    borderRadius: 4,
    cursor: disabled ? "not-allowed" : "pointer",
  };
}
