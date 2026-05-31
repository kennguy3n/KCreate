// TextStylePanel — Phase A1 inline text editor + font controls.
//
// Surfaces the three style fields the Rust shaper actually consumes
// (`TextStyleWire` in `crates/kcreate_bridge/src/phase2.rs`):
// `fontFamily`, `fontSize`, `lineHeight`. Plus a content textarea
// that mirrors the canvas double-click editor — the user can edit
// the whole layer's text without entering the inline mode if they
// want a free-form multi-line buffer.
//
// Weight / italic / alignment / fill are intentionally NOT here:
// `TextStyleWire` only carries family/size/line-height today, and
// adding renderer-side controls for fields the engine does not yet
// honour would be lying to the user. When the shaping pipeline
// grows those fields (see `kcreate_text::paragraph::TextStyle`),
// extend the wire format first then the panel.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import type { NodeInfo, TextStyleWire } from "../../../shared/scene";
import { colors, radius, spacing } from "../styles/tokens";

const MIN_FONT_SIZE = 1;
const MAX_FONT_SIZE = 512;
const MIN_LINE_HEIGHT = 0.5;
const MAX_LINE_HEIGHT = 8;

export interface TextStylePanelProps {
  node: NodeInfo;
  onStatus?: (msg: string | null) => void;
}

export function TextStylePanel({
  node,
  onStatus,
}: TextStylePanelProps): JSX.Element {
  const [style, setStyle] = useState<TextStyleWire | null>(null);
  const [content, setContent] = useState<string | null>(null);
  const [fonts, setFonts] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // The textarea is mounted as uncontrolled w.r.t. our `content`
  // state during typing — we drive it from `defaultValue` and let
  // React rehydrate on `node.version` change. This avoids the
  // remount-on-every-keystroke flicker we'd get if `value` were
  // bound to local state.
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);

  const load = useCallback(async (id: string) => {
    setError(null);
    try {
      const [nextStyle, nextContent] = await Promise.all([
        window.kcreate.text.getStyle(id),
        window.kcreate.text.getContent(id),
      ]);
      setStyle(nextStyle);
      setContent(nextContent);
      if (textareaRef.current && textareaRef.current.value !== nextContent) {
        textareaRef.current.value = nextContent;
      }
    } catch (e) {
      setError(errMsg(e));
    }
  }, []);

  // Hydrate on selection change or version bump (undo/redo, collab).
  useEffect(() => {
    void load(node.id);
  }, [load, node.id, node.version]);

  // Font list — process-wide cache lives in Rust, so calling on
  // every mount is cheap. Done once on mount; if the user
  // installs a system font mid-session they can re-select the
  // node to refire this effect.
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const list = await window.kcreate.text.listFonts();
        if (!cancelled) setFonts(list);
      } catch (e) {
        if (!cancelled) {
          // Font enumeration failure is non-fatal — the user can
          // still type a family name into the combobox; the shaper
          // falls back to its registered default if the family is
          // not resolvable. Surface a status hint for diagnosis.
          onStatus?.(`Text style: font enumeration failed — ${errMsg(e)}`);
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [onStatus]);

  const commitStyle = useCallback(
    async (next: TextStyleWire) => {
      setBusy(true);
      onStatus?.("Text style: updating…");
      try {
        await window.kcreate.text.setStyle(node.id, next);
        setStyle(next);
        setError(null);
        onStatus?.("Text style: updated.");
      } catch (e) {
        const msg = errMsg(e);
        setError(msg);
        onStatus?.(`Text style: update failed — ${msg}`);
      } finally {
        setBusy(false);
      }
    },
    [node.id, onStatus],
  );

  const commitContent = useCallback(
    async (next: string) => {
      setBusy(true);
      onStatus?.("Text content: updating…");
      try {
        await window.kcreate.text.setContent(node.id, next);
        setContent(next);
        setError(null);
        onStatus?.("Text content: updated.");
      } catch (e) {
        const msg = errMsg(e);
        setError(msg);
        onStatus?.(`Text content: update failed — ${msg}`);
      } finally {
        setBusy(false);
      }
    },
    [node.id, onStatus],
  );

  // Sorted family list with a dedicated "(current)" entry when the
  // node's font family is not present in the OS font DB. This
  // covers the case where a project bundles a font that hasn't been
  // installed system-wide — we still want the user to see and
  // preserve the assigned family rather than silently snapping it
  // to a sibling.
  const familyOptions = useMemo<string[]>(() => {
    if (!style) return fonts;
    if (fonts.includes(style.fontFamily)) return fonts;
    return [style.fontFamily, ...fonts];
  }, [fonts, style]);

  if (!style || content === null) {
    return (
      <div style={{ display: "flex", flexDirection: "column", gap: spacing.sm }}>
        <h4 style={sectionHeading}>Text style</h4>
        {error ? (
          <p style={errorText}>{error}</p>
        ) : (
          <p style={mutedText}>Loading…</p>
        )}
      </div>
    );
  }

  return (
    <div
      style={{ display: "flex", flexDirection: "column", gap: spacing.md }}
      data-testid="text-style-panel"
    >
      <h4 style={sectionHeading}>Text style</h4>

      <label style={fieldStack}>
        <span style={fieldLabel}>Content</span>
        <textarea
          ref={textareaRef}
          defaultValue={content}
          disabled={busy}
          rows={4}
          spellCheck={false}
          // Commit on blur so we don't emit an operation per
          // keystroke; the inline canvas editor takes the same
          // approach. Typing-rate undo entries would balloon the
          // operation log and make collaboration replay slow.
          onBlur={(e) => {
            const next = e.target.value;
            if (next !== content) void commitContent(next);
          }}
          style={{
            ...inputStyle,
            resize: "vertical",
            fontFamily: "inherit",
            minHeight: 64,
          }}
        />
      </label>

      <label style={fieldStack}>
        <span style={fieldLabel}>Font family</span>
        <input
          list={`text-style-fonts-${node.id}`}
          type="text"
          value={style.fontFamily}
          disabled={busy}
          onChange={(e) =>
            // Typing into the combobox is treated as a draft until
            // the user commits via blur or Enter — same UX as the
            // textarea above. Local state update is cheap, the
            // bridge call only fires once they're done.
            setStyle({ ...style, fontFamily: e.target.value })
          }
          onBlur={(e) => {
            const next = e.target.value;
            if (next && next !== style.fontFamily) {
              void commitStyle({ ...style, fontFamily: next });
            }
          }}
          style={inputStyle}
        />
        <datalist id={`text-style-fonts-${node.id}`}>
          {familyOptions.map((family) => (
            <option key={family} value={family} />
          ))}
        </datalist>
      </label>

      <Row>
        <NumberField
          label="Size"
          value={style.fontSize}
          min={MIN_FONT_SIZE}
          max={MAX_FONT_SIZE}
          step={1}
          disabled={busy}
          onChange={(v) =>
            void commitStyle({
              ...style,
              fontSize: clamp(v, MIN_FONT_SIZE, MAX_FONT_SIZE),
            })
          }
        />
        <NumberField
          label="Line height"
          value={style.lineHeight}
          min={MIN_LINE_HEIGHT}
          max={MAX_LINE_HEIGHT}
          step={0.05}
          disabled={busy}
          onChange={(v) =>
            void commitStyle({
              ...style,
              lineHeight: clamp(v, MIN_LINE_HEIGHT, MAX_LINE_HEIGHT),
            })
          }
        />
      </Row>

      {error ? <p style={errorText}>{error}</p> : null}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Layout primitives — kept local for the same reason as in
// TextFramePanel: shared shape, locally-typed props.
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

const sectionHeading: React.CSSProperties = {
  margin: 0,
  fontSize: 13,
  fontWeight: 600,
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

function clamp(v: number, lo: number, hi: number): number {
  if (v < lo) return lo;
  if (v > hi) return hi;
  return v;
}

function errMsg(e: unknown): string {
  if (e instanceof Error) return e.message;
  if (typeof e === "string") return e;
  try {
    return JSON.stringify(e);
  } catch {
    return String(e);
  }
}
