// BrandKitEditor — CRUD panel for project brand kits (Block G Task 24).
//
// Wraps `window.kcreate.brandKit.{create,update,list,delete}`. A brand
// kit is a top-level palette + typography + spacing rules; users
// apply one to the open project by clicking "Apply kit" which copies
// the kit's colors / spacing into the project's design tokens.

import { useCallback, useEffect, useState } from "react";

import type {
  BrandKit,
  DesignTokens,
  NamedColor,
  RgbaColor,
} from "../../../shared/scene";
import { colors, radius, spacing } from "../styles/tokens";

export interface BrandKitEditorProps {
  onStatus: (msg: string | null) => void;
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

export function BrandKitEditor({
  onStatus,
}: BrandKitEditorProps): JSX.Element {
  const [kits, setKits] = useState<BrandKit[]>([]);
  const [activeId, setActiveId] = useState<string | null>(null);

  // Reload does not depend on `activeId`: we resolve the initial
  // selection inside the functional `setActiveId` updater so the
  // closure stays stable. Depending on `activeId` would cause the
  // first mount to refetch twice (initial list → setActiveId →
  // reload identity changes → useEffect re-fires).
  const reload = useCallback(() => {
    void (async () => {
      try {
        const next = await window.kcreate.brandKit.list();
        setKits(next);
        if (next.length > 0 && next[0]) {
          const first = next[0];
          setActiveId((prev) => prev ?? first.id);
        }
      } catch (e) {
        onStatus(`Brand kit: load failed (${errMsg(e)})`);
      }
    })();
  }, [onStatus]);

  useEffect(() => {
    reload();
  }, [reload]);

  const active = kits.find((k) => k.id === activeId) ?? null;

  const persist = useCallback(
    async (next: BrandKit) => {
      try {
        await window.kcreate.brandKit.update(next);
        setKits((prev) => prev.map((k) => (k.id === next.id ? next : k)));
      } catch (e) {
        onStatus(`Brand kit: save failed (${errMsg(e)})`);
      }
    },
    [onStatus],
  );

  const handleCreate = async (name: string): Promise<void> => {
    try {
      const id = await window.kcreate.brandKit.create(name);
      onStatus(`Brand kit "${name}" created.`);
      setActiveId(id);
      reload();
    } catch (e) {
      onStatus(`Create failed: ${errMsg(e)}`);
    }
  };

  const handleDelete = async (kitId: string): Promise<void> => {
    try {
      await window.kcreate.brandKit.delete(kitId);
      onStatus("Brand kit deleted.");
      if (activeId === kitId) setActiveId(null);
      reload();
    } catch (e) {
      onStatus(`Delete failed: ${errMsg(e)}`);
    }
  };

  const handleApply = useCallback(
    async (kit: BrandKit): Promise<void> => {
      // Apply maps the kit's named colors + spacing onto the project's
      // design tokens. Other token kinds (typography, shadows, radii)
      // are left untouched because the brand-kit shape doesn't carry
      // them in Phase 1. The setter is overwrite-by-key so re-running
      // "Apply" is idempotent.
      try {
        const current = await window.kcreate.designTokens.get();
        const next: DesignTokens = {
          ...current,
          colors: { ...current.colors },
          spacing: { ...current.spacing },
        };
        for (const nc of kit.colors) next.colors[nc.name] = nc.color;
        kit.spacing_scale.forEach((v, idx) => {
          next.spacing[`scale-${idx}`] = v;
        });
        await window.kcreate.designTokens.set(next);
        onStatus(`Brand kit "${kit.name}" applied to project tokens.`);
      } catch (e) {
        onStatus(`Apply failed: ${errMsg(e)}`);
      }
    },
    [onStatus],
  );

  return (
    <div style={panelStyle}>
      <Header title="Brand kits" />
      <div style={listStyle}>
        {kits.length === 0 ? (
          <p style={emptyHintStyle}>
            No brand kits yet. Create one to share palettes across projects.
          </p>
        ) : (
          kits.map((k) => (
            <button
              type="button"
              key={k.id}
              onClick={() => setActiveId(k.id)}
              style={kitTabStyle(k.id === activeId)}
            >
              {k.name}
            </button>
          ))
        )}
      </div>

      <NewKitForm onCreate={handleCreate} />

      {active ? (
        <ActiveKitEditor
          kit={active}
          onRename={(name) => void persist({ ...active, name })}
          onColorsChange={(colorsNext) =>
            void persist({ ...active, colors: colorsNext })
          }
          onSpacingChange={(spacingNext) =>
            void persist({ ...active, spacing_scale: spacingNext })
          }
          onApply={() => void handleApply(active)}
          onDelete={() => void handleDelete(active.id)}
        />
      ) : null}
    </div>
  );
}

function errMsg(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

function NewKitForm({
  onCreate,
}: {
  onCreate: (name: string) => void;
}): JSX.Element {
  const [name, setName] = useState("");
  return (
    <form
      onSubmit={(e) => {
        e.preventDefault();
        const v = name.trim();
        if (!v) return;
        onCreate(v);
        setName("");
      }}
      style={{
        display: "flex",
        gap: spacing.xs,
        marginBottom: spacing.sm,
      }}
    >
      <input
        type="text"
        placeholder="new brand kit name"
        value={name}
        onChange={(e) => setName(e.target.value)}
        style={{ ...inputStyle, flex: 1 }}
      />
      <button type="submit" style={primaryBtn(!name.trim())}>
        Create
      </button>
    </form>
  );
}

function ActiveKitEditor({
  kit,
  onRename,
  onColorsChange,
  onSpacingChange,
  onApply,
  onDelete,
}: {
  kit: BrandKit;
  onRename: (name: string) => void;
  onColorsChange: (next: NamedColor[]) => void;
  onSpacingChange: (next: number[]) => void;
  onApply: () => void;
  onDelete: () => void;
}): JSX.Element {
  return (
    <section style={{ display: "flex", flexDirection: "column", gap: spacing.sm }}>
      <Field label="Name">
        <input
          type="text"
          defaultValue={kit.name}
          onBlur={(e) => onRename(e.target.value)}
          style={inputStyle}
        />
      </Field>

      <Field label="Palette">
        <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
          {kit.colors.map((c, idx) => (
            <ColorRow
              key={`${c.name}-${idx}`}
              value={c}
              onChange={(next) => {
                const out = [...kit.colors];
                out[idx] = next;
                onColorsChange(out);
              }}
              onDelete={() => {
                onColorsChange(kit.colors.filter((_, i) => i !== idx));
              }}
            />
          ))}
          <button
            type="button"
            style={chipBtn(false)}
            onClick={() => {
              onColorsChange([
                ...kit.colors,
                { name: `color-${kit.colors.length + 1}`, color: hexToRgba("#888888") },
              ]);
            }}
          >
            + Color
          </button>
        </div>
      </Field>

      <Field label="Spacing scale">
        <div style={{ display: "flex", flexWrap: "wrap", gap: 4 }}>
          {kit.spacing_scale.map((v, idx) => (
            <input
              key={idx}
              type="number"
              value={v}
              onChange={(e) => {
                const out = [...kit.spacing_scale];
                out[idx] = Number(e.target.value) || 0;
                onSpacingChange(out);
              }}
              style={{ ...inputStyle, width: 56 }}
            />
          ))}
          <button
            type="button"
            style={chipBtn(false)}
            onClick={() => onSpacingChange([...kit.spacing_scale, 8])}
          >
            +
          </button>
          <button
            type="button"
            style={dangerBtn}
            onClick={() => {
              if (kit.spacing_scale.length === 0) return;
              onSpacingChange(kit.spacing_scale.slice(0, -1));
            }}
            title="remove last"
          >
            −
          </button>
        </div>
      </Field>

      <div style={{ display: "flex", gap: spacing.sm }}>
        <button type="button" style={primaryBtn(false)} onClick={onApply}>
          Apply kit to project
        </button>
        <button type="button" style={dangerBtn} onClick={onDelete}>
          Delete kit
        </button>
      </div>
    </section>
  );
}

function ColorRow({
  value,
  onChange,
  onDelete,
}: {
  value: NamedColor;
  onChange: (next: NamedColor) => void;
  onDelete: () => void;
}): JSX.Element {
  return (
    <div style={{ display: "flex", alignItems: "center", gap: spacing.xs }}>
      <input
        type="text"
        value={value.name}
        onChange={(e) => onChange({ ...value, name: e.target.value })}
        style={{ ...inputStyle, width: 110 }}
      />
      <input
        type="color"
        value={rgbaToHex(value.color)}
        onChange={(e) =>
          onChange({ ...value, color: hexToRgba(e.target.value, value.color.a) })
        }
        style={{
          width: 28,
          height: 24,
          border: `1px solid ${colors.border}`,
          background: "none",
          padding: 0,
        }}
      />
      <button type="button" style={dangerBtn} onClick={onDelete} title="remove">
        ×
      </button>
    </div>
  );
}

function Header({ title }: { title: string }): JSX.Element {
  return (
    <h2
      style={{
        margin: 0,
        marginBottom: spacing.sm,
        fontSize: 14,
        fontWeight: 600,
        color: colors.text,
      }}
    >
      {title}
    </h2>
  );
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

const panelStyle: React.CSSProperties = {
  padding: spacing.sm,
  display: "flex",
  flexDirection: "column",
  overflowY: "auto",
};

const listStyle: React.CSSProperties = {
  display: "flex",
  flexWrap: "wrap",
  gap: 4,
  marginBottom: spacing.sm,
};

const emptyHintStyle: React.CSSProperties = {
  fontSize: 11,
  color: colors.textMuted,
  margin: 0,
};

function kitTabStyle(active: boolean): React.CSSProperties {
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

const inputStyle: React.CSSProperties = {
  background: colors.bgSoft,
  color: colors.text,
  border: `1px solid ${colors.border}`,
  borderRadius: 4,
  padding: "4px 6px",
  fontSize: 11,
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
  padding: "0 8px",
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
