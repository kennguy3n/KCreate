// Theme / Brand Kit instant restyle panel (G4).
//
// Mirrors Gamma's "switch theme to restyle the whole deck" and Canva's
// Brand Kit (pinned palette + fonts). The panel lets the user:
//
//   * pick one of the built-in themes (loaded from
//     `window.kcreate.theme.listBuiltins()`), previewing its palette
//     swatches + type scale;
//   * derive a brand-new theme from the colors already in the open
//     document (`theme.deriveFromDocument`);
//   * author a custom Brand Kit (palette + fonts) that persists with
//     the project through the canonical `brandKit.*` CRUD surface;
//   * hit **Apply** to restyle the whole document in a single undoable
//     operation (`theme.apply`) — one Ctrl+Z reverts the entire
//     restyle.
//
// The panel never touches the scene graph directly: applying a theme
// runs entirely in the Rust bridge (role-aware recolor + type-scale +
// radii), pushes a fresh frame to the canvas via scene-sync, and the
// host re-fetches the document tree through `onApplied`.
//
// Conventions (colors / radius / spacing tokens, `onStatus` bubbling,
// `errMsg`, useCallback/useEffect load+commit, small field
// sub-components) mirror `ColorSettingsPanel.tsx`.

import { useCallback, useEffect, useMemo, useState } from "react";

import type {
  ApplyThemeReport,
  BrandKit,
  FontRef,
  NamedColor,
  RgbaColor,
  Theme,
  ThemePalette,
} from "../../../shared/scene";
import { colors, radius, spacing } from "../styles/tokens";

export interface ThemePanelProps {
  /** Bubbles a transient status line to the editor footer. */
  onStatus?: (msg: string | null) => void;
  /**
   * Fired after a successful `apply` so the host can re-fetch the
   * document tree / selection / status. The canvas itself updates
   * independently via the bridge's scene-sync push, so this is only
   * about keeping React state (layer tree, properties) in sync.
   */
  onApplied?: () => void;
}

/** Palette roles in a stable, human-meaningful order for the swatch row. */
const PALETTE_ROLES: ReadonlyArray<{ key: keyof ThemePalette; label: string }> =
  [
    { key: "background", label: "Background" },
    { key: "surface", label: "Surface" },
    { key: "primary", label: "Primary" },
    { key: "secondary", label: "Secondary" },
    { key: "accent", label: "Accent" },
    { key: "text", label: "Text" },
    { key: "muted", label: "Muted" },
  ];

const clamp01 = (v: number): number => (v < 0 ? 0 : v > 1 ? 1 : v);
const to255 = (v: number): number => Math.round(clamp01(v) * 255);

/** `RgbaColor` (0..1 floats) → CSS `rgba(...)` for previews. */
function rgbaToCss(c: RgbaColor): string {
  return `rgba(${to255(c.r)}, ${to255(c.g)}, ${to255(c.b)}, ${clamp01(c.a)})`;
}

/** `RgbaColor` → `#rrggbb` for `<input type="color">` value binding. */
function rgbaToHex(c: RgbaColor): string {
  const h = (v: number): string => to255(v).toString(16).padStart(2, "0");
  return `#${h(c.r)}${h(c.g)}${h(c.b)}`;
}

/** `#rrggbb` → `RgbaColor`, preserving the supplied alpha. */
function hexToRgba(hex: string, alpha: number): RgbaColor {
  const m = /^#?([0-9a-fA-F]{6})$/.exec(hex.trim());
  const digits = m?.[1];
  if (digits === undefined) return { r: 0, g: 0, b: 0, a: clamp01(alpha) };
  const int = Number.parseInt(digits, 16);
  return {
    r: ((int >> 16) & 0xff) / 255,
    g: ((int >> 8) & 0xff) / 255,
    b: (int & 0xff) / 255,
    a: clamp01(alpha),
  };
}

function errMsg(e: unknown): string {
  if (e instanceof Error) return e.message;
  if (typeof e === "string") return e;
  return JSON.stringify(e);
}

export function ThemePanel({ onStatus, onApplied }: ThemePanelProps): JSX.Element {
  const [builtins, setBuiltins] = useState<Theme[]>([]);
  // Themes derived from the open document this session (shown first).
  const [derived, setDerived] = useState<Theme[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [deriveName, setDeriveName] = useState("Derived theme");

  const [kits, setKits] = useState<BrandKit[]>([]);
  // The brand kit currently open in the editor (a working draft copy).
  const [draftKit, setDraftKit] = useState<BrandKit | null>(null);

  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [report, setReport] = useState<ApplyThemeReport | null>(null);

  const allThemes = useMemo<Theme[]>(
    () => [...derived, ...builtins],
    [derived, builtins],
  );
  const selected = useMemo<Theme | null>(
    () => allThemes.find((t) => t.id === selectedId) ?? null,
    [allThemes, selectedId],
  );

  const loadThemes = useCallback(async () => {
    const list = (await window.kcreate.theme.listBuiltins()) ?? [];
    setBuiltins(list);
    setSelectedId((prev) =>
      prev !== null ? prev : (list[0]?.id ?? null),
    );
  }, []);

  const loadKits = useCallback(async () => {
    const list = (await window.kcreate.brandKit.list()) ?? [];
    setKits(list);
  }, []);

  useEffect(() => {
    void (async () => {
      try {
        await Promise.all([loadThemes(), loadKits()]);
      } catch (e) {
        setError(errMsg(e));
      }
    })();
  }, [loadThemes, loadKits]);

  const applyTheme = useCallback(
    async (theme: Theme, statusLabel?: string) => {
      setBusy(true);
      setError(null);
      onStatus?.(statusLabel ?? `Theme: applying “${theme.name}”…`);
      try {
        const r = await window.kcreate.theme.apply(theme);
        setReport(r);
        onStatus?.(
          `Applied “${r.themeName}”: ${r.affectedNodes} nodes — ` +
            `${r.recoloredFills} fills, ${r.recoloredStrokes} strokes, ` +
            `${r.restyledText} text.`,
        );
        onApplied?.();
      } catch (e) {
        const msg = errMsg(e);
        setError(msg);
        onStatus?.(`Theme apply failed: ${msg}`);
      } finally {
        setBusy(false);
      }
    },
    [onStatus, onApplied],
  );

  const handleApplySelected = useCallback(() => {
    if (selected === null) return;
    void applyTheme(selected);
  }, [selected, applyTheme]);

  const handleDerive = useCallback(() => {
    void (async () => {
      setBusy(true);
      setError(null);
      const name = deriveName.trim() === "" ? "Derived theme" : deriveName.trim();
      onStatus?.(`Theme: deriving “${name}” from this design…`);
      try {
        const t = await window.kcreate.theme.deriveFromDocument(name);
        setDerived((prev) => [t, ...prev.filter((p) => p.id !== t.id)]);
        setSelectedId(t.id);
        onStatus?.(`Derived theme “${t.name}” — review and Apply.`);
      } catch (e) {
        const msg = errMsg(e);
        setError(msg);
        onStatus?.(`Derive failed: ${msg}`);
      } finally {
        setBusy(false);
      }
    })();
  }, [deriveName, onStatus]);

  // --- Brand-kit authoring -------------------------------------------------

  const openKit = useCallback((kit: BrandKit) => {
    // Deep-copy so edits stay local until Save.
    setDraftKit({
      ...kit,
      colors: kit.colors.map((c) => ({ ...c, color: { ...c.color } })),
      fonts: kit.fonts.map((f) => ({ ...f })),
      spacing_scale: [...kit.spacing_scale],
      export_rules: kit.export_rules.map((r) => ({ ...r })),
    });
  }, []);

  const handleNewKit = useCallback(() => {
    void (async () => {
      setBusy(true);
      setError(null);
      const name = `Brand kit ${kits.length + 1}`;
      try {
        const id = await window.kcreate.brandKit.create(name);
        await window.kcreate.document.saveProject();
        const list = (await window.kcreate.brandKit.list()) ?? [];
        setKits(list);
        const created = list.find((k) => k.id === id) ?? null;
        if (created !== null) openKit(created);
        onStatus?.(`Created brand kit “${name}”.`);
      } catch (e) {
        const msg = errMsg(e);
        setError(msg);
        onStatus?.(`Create kit failed: ${msg}`);
      } finally {
        setBusy(false);
      }
    })();
  }, [kits.length, openKit, onStatus]);

  const handleSaveKit = useCallback(() => {
    if (draftKit === null) return;
    void (async () => {
      setBusy(true);
      setError(null);
      try {
        await window.kcreate.brandKit.update(draftKit);
        await window.kcreate.document.saveProject();
        await loadKits();
        onStatus?.(`Saved brand kit “${draftKit.name}”.`);
      } catch (e) {
        const msg = errMsg(e);
        setError(msg);
        onStatus?.(`Save kit failed: ${msg}`);
      } finally {
        setBusy(false);
      }
    })();
  }, [draftKit, loadKits, onStatus]);

  const handleDeleteKit = useCallback(
    (kitId: string) => {
      void (async () => {
        setBusy(true);
        setError(null);
        try {
          await window.kcreate.brandKit.delete(kitId);
          await window.kcreate.document.saveProject();
          await loadKits();
          setDraftKit((prev) => (prev?.id === kitId ? null : prev));
          onStatus?.("Deleted brand kit.");
        } catch (e) {
          const msg = errMsg(e);
          setError(msg);
          onStatus?.(`Delete kit failed: ${msg}`);
        } finally {
          setBusy(false);
        }
      })();
    },
    [loadKits, onStatus],
  );

  const handleApplyKit = useCallback(
    (kit: BrandKit) => {
      void (async () => {
        const label = `Theme: applying brand kit “${kit.name}”…`;
        setBusy(true);
        setError(null);
        onStatus?.(label);
        try {
          const theme = await window.kcreate.theme.fromBrandKit(kit);
          // Pass the brand-kit label through so applyTheme's own
          // "applying…" status doesn't clobber the kit-specific one.
          await applyTheme(theme, label);
        } catch (e) {
          const msg = errMsg(e);
          setError(msg);
          onStatus?.(`Apply kit failed: ${msg}`);
        } finally {
          setBusy(false);
        }
      })();
    },
    [applyTheme, onStatus],
  );

  // Draft mutators (operate on the working copy; persisted on Save).
  const patchDraft = useCallback((next: Partial<BrandKit>) => {
    setDraftKit((prev) => (prev === null ? prev : { ...prev, ...next }));
  }, []);

  const addColor = useCallback(() => {
    setDraftKit((prev) =>
      prev === null
        ? prev
        : {
            ...prev,
            colors: [
              ...prev.colors,
              {
                name: `Color ${prev.colors.length + 1}`,
                color: { r: 0.15, g: 0.39, b: 0.92, a: 1 },
              },
            ],
          },
    );
  }, []);

  const updateColor = useCallback((index: number, next: NamedColor) => {
    setDraftKit((prev) =>
      prev === null
        ? prev
        : {
            ...prev,
            colors: prev.colors.map((c, i) => (i === index ? next : c)),
          },
    );
  }, []);

  const removeColor = useCallback((index: number) => {
    setDraftKit((prev) =>
      prev === null
        ? prev
        : { ...prev, colors: prev.colors.filter((_, i) => i !== index) },
    );
  }, []);

  const addFont = useCallback(() => {
    setDraftKit((prev) =>
      prev === null
        ? prev
        : {
            ...prev,
            fonts: [
              ...prev.fonts,
              { family: "Inter", weight: 400, italic: false, embedded_asset_id: null },
            ],
          },
    );
  }, []);

  const updateFont = useCallback((index: number, next: FontRef) => {
    setDraftKit((prev) =>
      prev === null
        ? prev
        : { ...prev, fonts: prev.fonts.map((f, i) => (i === index ? next : f)) },
    );
  }, []);

  const removeFont = useCallback((index: number) => {
    setDraftKit((prev) =>
      prev === null
        ? prev
        : { ...prev, fonts: prev.fonts.filter((_, i) => i !== index) },
    );
  }, []);

  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        gap: spacing.md,
        fontSize: 12,
        color: colors.text,
      }}
    >
      <section style={{ display: "flex", flexDirection: "column", gap: spacing.sm }}>
        <SectionTitle>Themes</SectionTitle>
        <p style={{ margin: 0, color: colors.textMuted, fontSize: 11 }}>
          Applying a theme restyles the whole document in one undoable step.
        </p>
        <div style={{ display: "flex", flexDirection: "column", gap: spacing.sm }}>
          {allThemes.map((theme) => (
            <ThemeCard
              key={theme.id}
              theme={theme}
              selected={theme.id === selectedId}
              onSelect={() => setSelectedId(theme.id)}
            />
          ))}
          {allThemes.length === 0 ? (
            <span style={{ color: colors.textMuted, fontSize: 11 }}>
              No themes available.
            </span>
          ) : null}
        </div>
        <button
          type="button"
          disabled={busy || selected === null}
          onClick={handleApplySelected}
          aria-label="Apply theme"
          style={primaryButtonStyle(busy || selected === null)}
        >
          {busy ? "Applying…" : `Apply${selected ? ` “${selected.name}”` : ""}`}
        </button>
      </section>

      <section style={{ display: "flex", flexDirection: "column", gap: spacing.sm }}>
        <SectionTitle>Derive from this design</SectionTitle>
        <p style={{ margin: 0, color: colors.textMuted, fontSize: 11 }}>
          Build a theme from the colors already used in the open document.
        </p>
        <div style={{ display: "flex", gap: spacing.sm }}>
          <input
            type="text"
            value={deriveName}
            onChange={(e) => setDeriveName(e.target.value)}
            aria-label="Derived theme name"
            style={inputStyle}
          />
          <button
            type="button"
            disabled={busy}
            onClick={handleDerive}
            aria-label="Derive theme from document"
            style={secondaryButtonStyle(busy)}
          >
            Derive
          </button>
        </div>
      </section>

      <section style={{ display: "flex", flexDirection: "column", gap: spacing.sm }}>
        <SectionTitle>Brand kits</SectionTitle>
        <p style={{ margin: 0, color: colors.textMuted, fontSize: 11 }}>
          Pin a custom palette + fonts. Saved with the project.
        </p>
        <div style={{ display: "flex", flexDirection: "column", gap: spacing.xs }}>
          {kits.map((kit) => (
            <div
              key={kit.id}
              style={{
                display: "flex",
                alignItems: "center",
                gap: spacing.sm,
                padding: spacing.xs,
                border: `1px solid ${colors.border}`,
                borderRadius: radius.md,
                background:
                  draftKit?.id === kit.id ? colors.accentBgSoft : colors.bg,
              }}
            >
              <SwatchRow colorsList={kit.colors.map((c) => c.color)} />
              <span style={{ flex: 1, fontSize: 11 }}>{kit.name}</span>
              <button
                type="button"
                disabled={busy}
                onClick={() => openKit(kit)}
                aria-label={`Edit ${kit.name}`}
                style={miniButtonStyle(busy)}
              >
                Edit
              </button>
              <button
                type="button"
                disabled={busy}
                onClick={() => handleApplyKit(kit)}
                aria-label={`Apply ${kit.name}`}
                style={miniButtonStyle(busy)}
              >
                Apply
              </button>
              <button
                type="button"
                disabled={busy}
                onClick={() => handleDeleteKit(kit.id)}
                aria-label={`Delete ${kit.name}`}
                style={miniButtonStyle(busy)}
              >
                Delete
              </button>
            </div>
          ))}
        </div>
        <button
          type="button"
          disabled={busy}
          onClick={handleNewKit}
          aria-label="New brand kit"
          style={secondaryButtonStyle(busy)}
        >
          + New brand kit
        </button>
      </section>

      {draftKit !== null ? (
        <section
          style={{
            display: "flex",
            flexDirection: "column",
            gap: spacing.sm,
            padding: spacing.sm,
            border: `1px solid ${colors.border}`,
            borderRadius: radius.card,
            background: colors.bgSoft,
          }}
        >
          <SectionTitle>Edit “{draftKit.name}”</SectionTitle>
          <LabeledField label="Name">
            <input
              type="text"
              value={draftKit.name}
              onChange={(e) => patchDraft({ name: e.target.value })}
              aria-label="Brand kit name"
              style={inputStyle}
            />
          </LabeledField>

          <div style={{ fontSize: 11, color: colors.textMuted }}>Colors</div>
          {draftKit.colors.map((c, i) => (
            <div
              key={i}
              style={{ display: "flex", gap: spacing.xs, alignItems: "center" }}
            >
              <input
                type="color"
                value={rgbaToHex(c.color)}
                onChange={(e) =>
                  updateColor(i, {
                    ...c,
                    color: hexToRgba(e.target.value, c.color.a),
                  })
                }
                aria-label={`${c.name} color`}
                style={{
                  width: 28,
                  height: 24,
                  padding: 0,
                  border: `1px solid ${colors.border}`,
                  borderRadius: radius.sm,
                  background: "transparent",
                }}
              />
              <input
                type="text"
                value={c.name}
                onChange={(e) => updateColor(i, { ...c, name: e.target.value })}
                aria-label={`Color ${i + 1} name`}
                style={{ ...inputStyle, flex: 1 }}
              />
              <button
                type="button"
                onClick={() => removeColor(i)}
                aria-label={`Remove color ${i + 1}`}
                style={miniButtonStyle(false)}
              >
                ✕
              </button>
            </div>
          ))}
          <button
            type="button"
            onClick={addColor}
            aria-label="Add color"
            style={miniButtonStyle(false)}
          >
            + Color
          </button>

          <div style={{ fontSize: 11, color: colors.textMuted }}>Fonts</div>
          {draftKit.fonts.map((f, i) => (
            <div
              key={i}
              style={{ display: "flex", gap: spacing.xs, alignItems: "center" }}
            >
              <input
                type="text"
                value={f.family}
                onChange={(e) => updateFont(i, { ...f, family: e.target.value })}
                aria-label={`Font ${i + 1} family`}
                style={{ ...inputStyle, flex: 1 }}
              />
              <input
                type="number"
                value={f.weight}
                min={100}
                max={900}
                step={100}
                onChange={(e) =>
                  updateFont(i, {
                    ...f,
                    weight: Number.parseInt(e.target.value, 10) || 400,
                  })
                }
                aria-label={`Font ${i + 1} weight`}
                style={{ ...inputStyle, width: 64 }}
              />
              <button
                type="button"
                onClick={() => removeFont(i)}
                aria-label={`Remove font ${i + 1}`}
                style={miniButtonStyle(false)}
              >
                ✕
              </button>
            </div>
          ))}
          <button
            type="button"
            onClick={addFont}
            aria-label="Add font"
            style={miniButtonStyle(false)}
          >
            + Font
          </button>

          <div style={{ display: "flex", gap: spacing.sm }}>
            <button
              type="button"
              disabled={busy}
              onClick={handleSaveKit}
              aria-label="Save brand kit"
              style={primaryButtonStyle(busy)}
            >
              Save
            </button>
            <button
              type="button"
              disabled={busy}
              onClick={() => handleApplyKit(draftKit)}
              aria-label="Apply brand kit as theme"
              style={secondaryButtonStyle(busy)}
            >
              Apply as theme
            </button>
            <button
              type="button"
              onClick={() => setDraftKit(null)}
              aria-label="Close brand kit editor"
              style={secondaryButtonStyle(false)}
            >
              Close
            </button>
          </div>
        </section>
      ) : null}

      {report !== null ? (
        <div style={{ fontSize: 11, color: colors.textMuted }}>
          Last apply: {report.themeName} — {report.affectedNodes} nodes,{" "}
          {report.recoloredFills} fills, {report.recoloredStrokes} strokes,{" "}
          {report.restyledText} text.
        </div>
      ) : null}

      {error !== null ? (
        <div style={{ fontSize: 11, color: colors.danger }}>{error}</div>
      ) : null}
    </div>
  );
}

// --- presentational sub-components -----------------------------------------

function SectionTitle({ children }: { children: React.ReactNode }): JSX.Element {
  return (
    <div style={{ fontSize: 12, fontWeight: 600, color: colors.text }}>
      {children}
    </div>
  );
}

function LabeledField({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}): JSX.Element {
  return (
    <label style={{ display: "flex", flexDirection: "column", gap: 2 }}>
      <span style={{ fontSize: 11, color: colors.textMuted }}>{label}</span>
      {children}
    </label>
  );
}

function SwatchRow({ colorsList }: { colorsList: RgbaColor[] }): JSX.Element {
  return (
    <div style={{ display: "flex", gap: 2 }}>
      {colorsList.slice(0, 7).map((c, i) => (
        <span
          key={i}
          style={{
            width: 14,
            height: 14,
            borderRadius: radius.sm,
            background: rgbaToCss(c),
            border: `1px solid ${colors.border}`,
          }}
        />
      ))}
    </div>
  );
}

function ThemeCard({
  theme,
  selected,
  onSelect,
}: {
  theme: Theme;
  selected: boolean;
  onSelect: () => void;
}): JSX.Element {
  const p = theme.palette;
  return (
    <button
      type="button"
      onClick={onSelect}
      aria-label={`Select theme ${theme.name}`}
      aria-pressed={selected}
      style={{
        display: "flex",
        flexDirection: "column",
        gap: spacing.xs,
        padding: spacing.sm,
        textAlign: "left",
        cursor: "pointer",
        border: `1px solid ${selected ? colors.accent : colors.border}`,
        borderRadius: radius.card,
        background: rgbaToCss(p.background),
        outline: selected ? `2px solid ${colors.accentRing}` : "none",
      }}
    >
      <div
        style={{
          display: "flex",
          alignItems: "baseline",
          justifyContent: "space-between",
        }}
      >
        <span
          style={{
            fontFamily: theme.type_scale.heading_font,
            fontSize: 15,
            fontWeight: 700,
            color: rgbaToCss(p.text),
          }}
        >
          {theme.name}
        </span>
        <span
          style={{
            fontFamily: theme.type_scale.body_font,
            fontSize: 11,
            color: rgbaToCss(p.muted),
          }}
        >
          Aa
        </span>
      </div>
      <SwatchRow
        colorsList={PALETTE_ROLES.map(({ key }) => p[key])}
      />
    </button>
  );
}

// --- shared inline styles ---------------------------------------------------

const inputStyle: React.CSSProperties = {
  fontSize: 12,
  padding: `${spacing.xs}px ${spacing.sm}px`,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.md,
  background: colors.bg,
  color: colors.text,
};

function primaryButtonStyle(disabled: boolean): React.CSSProperties {
  return {
    fontSize: 12,
    padding: `${spacing.xs}px ${spacing.md}px`,
    borderRadius: radius.md,
    border: `1px solid ${colors.accent}`,
    background: disabled ? colors.bgSoft : colors.accent,
    color: disabled ? colors.textMuted : colors.textInverse,
    cursor: disabled ? "default" : "pointer",
  };
}

function secondaryButtonStyle(disabled: boolean): React.CSSProperties {
  return {
    fontSize: 12,
    padding: `${spacing.xs}px ${spacing.md}px`,
    borderRadius: radius.md,
    border: `1px solid ${colors.border}`,
    background: colors.bg,
    color: disabled ? colors.textMuted : colors.text,
    cursor: disabled ? "default" : "pointer",
  };
}

function miniButtonStyle(disabled: boolean): React.CSSProperties {
  return {
    fontSize: 11,
    padding: `2px ${spacing.sm}px`,
    borderRadius: radius.sm,
    border: `1px solid ${colors.border}`,
    background: colors.bg,
    color: disabled ? colors.textMuted : colors.text,
    cursor: disabled ? "default" : "pointer",
  };
}
