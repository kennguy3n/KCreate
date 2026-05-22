import { useEffect, useMemo, useRef, useState } from "react";

import type {
  ArtboardPreset,
  ArtboardPresetCategory,
} from "../../../shared/scene";
import { colors, font, radius, shadow, spacing } from "../styles/tokens";

export interface ArtboardDialogProps {
  /** When `false` the dialog renders nothing. Owned by the parent. */
  open: boolean;
  /** Presets loaded from the bridge. */
  presets: ArtboardPreset[];
  /**
   * Called when the user confirms. The parent is responsible for the
   * actual `artboard.create` IPC round-trip + refreshing the tree.
   */
  onCreate: (args: { name: string; width: number; height: number }) => void;
  /** Cancel — close without creating. */
  onClose: () => void;
}

/// Human-readable category labels. Ordered as they should appear in
/// the grouped preset grid. The order matters because the dialog
/// lays the categories out vertically and users scan top→bottom.
const CATEGORY_LABELS: ReadonlyArray<{
  id: ArtboardPresetCategory;
  label: string;
}> = [
  { id: "web_desktop", label: "Web — Desktop" },
  { id: "web_tablet", label: "Web — Tablet" },
  { id: "web_mobile", label: "Web — Mobile" },
  { id: "social_media", label: "Social Media" },
  { id: "print", label: "Print" },
  { id: "custom", label: "Custom" },
];

/// Minimum + maximum dimensions enforced by the dialog so the user
/// can't accidentally create a degenerate or absurd artboard. The
/// upper bound mirrors `kcreate_core::Bounds` — bigger than this
/// stresses the GPU readback path on weak machines.
const MIN_DIM = 1;
const MAX_DIM = 20000;

export function ArtboardDialog({
  open,
  presets,
  onCreate,
  onClose,
}: ArtboardDialogProps): JSX.Element | null {
  // Default to a sensible web-desktop preset when the dialog opens.
  // We intentionally re-derive on every open so that a user who
  // edited the inputs and cancelled doesn't see the stale values on
  // their next attempt.
  const initial = useMemo(() => {
    const desktop = presets.find((p) => p.category === "web_desktop");
    return desktop ?? presets[0] ?? null;
  }, [presets]);
  const [name, setName] = useState(initial?.name ?? "Artboard");
  const [width, setWidth] = useState(initial?.width ?? 1440);
  const [height, setHeight] = useState(initial?.height ?? 900);
  const [error, setError] = useState<string | null>(null);
  const nameInputRef = useRef<HTMLInputElement | null>(null);

  // Reset the form when the dialog (re)opens.
  useEffect(() => {
    if (!open) return;
    if (initial) {
      setName(initial.name);
      setWidth(initial.width);
      setHeight(initial.height);
    }
    setError(null);
    // Defer focus to the next tick so the input is mounted.
    const handle = setTimeout(() => {
      nameInputRef.current?.focus();
      nameInputRef.current?.select();
    }, 0);
    return () => clearTimeout(handle);
  }, [open, initial]);

  // Close-on-ESC. Mount-time only — capture phase so the dialog
  // beats any document-level shortcuts the editor wires up.
  useEffect(() => {
    if (!open) return;
    const handler = (e: KeyboardEvent): void => {
      if (e.key === "Escape") {
        e.preventDefault();
        onClose();
      }
    };
    document.addEventListener("keydown", handler);
    return () => document.removeEventListener("keydown", handler);
  }, [open, onClose]);

  if (!open) return null;

  const validate = (): { ok: true } | { ok: false; reason: string } => {
    const trimmed = name.trim();
    if (trimmed.length === 0) return { ok: false, reason: "Name is required." };
    if (!Number.isFinite(width) || width < MIN_DIM || width > MAX_DIM) {
      return { ok: false, reason: `Width must be between ${MIN_DIM} and ${MAX_DIM}.` };
    }
    if (!Number.isFinite(height) || height < MIN_DIM || height > MAX_DIM) {
      return { ok: false, reason: `Height must be between ${MIN_DIM} and ${MAX_DIM}.` };
    }
    return { ok: true };
  };

  const submit = (): void => {
    const result = validate();
    if (!result.ok) {
      setError(result.reason);
      return;
    }
    setError(null);
    onCreate({ name: name.trim(), width, height });
  };

  const grouped = groupPresets(presets);

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-labelledby="artboard-dialog-title"
      onClick={onClose}
      style={{
        position: "fixed",
        inset: 0,
        background: "rgba(15, 18, 25, 0.55)",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        zIndex: 1000,
        fontFamily: font.family,
      }}
    >
      <div
        onClick={(e) => e.stopPropagation()}
        style={{
          width: 720,
          maxWidth: "90vw",
          maxHeight: "85vh",
          background: colors.bg,
          color: colors.text,
          borderRadius: radius.card,
          boxShadow: shadow.cardHover,
          display: "flex",
          flexDirection: "column",
          overflow: "hidden",
        }}
      >
        <header
          style={{
            padding: `${spacing.md}px ${spacing.lg}px`,
            borderBottom: `1px solid ${colors.border}`,
          }}
        >
          <h2
            id="artboard-dialog-title"
            style={{ margin: 0, fontSize: 16, fontWeight: 600 }}
          >
            New artboard
          </h2>
        </header>
        <div
          style={{
            flex: 1,
            overflowY: "auto",
            padding: spacing.lg,
            display: "grid",
            gridTemplateColumns: "1fr 240px",
            gap: spacing.lg,
            minHeight: 0,
          }}
        >
          <section
            style={{
              display: "flex",
              flexDirection: "column",
              gap: spacing.md,
            }}
          >
            {CATEGORY_LABELS.map((cat) => {
              const items = grouped.get(cat.id);
              if (!items || items.length === 0) return null;
              return (
                <div key={cat.id}>
                  <h3
                    style={{
                      margin: `0 0 ${spacing.xs}px`,
                      fontSize: 11,
                      fontWeight: 600,
                      color: colors.textMuted,
                      textTransform: "uppercase",
                      letterSpacing: 0.6,
                    }}
                  >
                    {cat.label}
                  </h3>
                  <div
                    style={{
                      display: "grid",
                      gridTemplateColumns:
                        "repeat(auto-fill, minmax(120px, 1fr))",
                      gap: spacing.xs,
                    }}
                  >
                    {items.map((p) => (
                      <PresetButton
                        key={`${p.name}-${p.width}x${p.height}`}
                        preset={p}
                        selected={
                          width === p.width &&
                          height === p.height &&
                          name === p.name
                        }
                        onSelect={() => {
                          setName(p.name);
                          setWidth(p.width);
                          setHeight(p.height);
                        }}
                      />
                    ))}
                  </div>
                </div>
              );
            })}
          </section>
          <aside
            style={{
              display: "flex",
              flexDirection: "column",
              gap: spacing.md,
              borderLeft: `1px solid ${colors.border}`,
              paddingLeft: spacing.md,
            }}
          >
            <Field label="Name">
              <input
                ref={nameInputRef}
                value={name}
                onChange={(e) => setName(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") submit();
                }}
                style={inputStyle}
              />
            </Field>
            <Field label="Width (px)">
              <input
                type="number"
                value={width}
                min={MIN_DIM}
                max={MAX_DIM}
                onChange={(e) => setWidth(Number(e.target.value))}
                onKeyDown={(e) => {
                  if (e.key === "Enter") submit();
                }}
                style={inputStyle}
              />
            </Field>
            <Field label="Height (px)">
              <input
                type="number"
                value={height}
                min={MIN_DIM}
                max={MAX_DIM}
                onChange={(e) => setHeight(Number(e.target.value))}
                onKeyDown={(e) => {
                  if (e.key === "Enter") submit();
                }}
                style={inputStyle}
              />
            </Field>
            {error ? (
              <div
                role="alert"
                style={{ color: "#B91C1C", fontSize: 12, lineHeight: 1.4 }}
              >
                {error}
              </div>
            ) : null}
          </aside>
        </div>
        <footer
          style={{
            display: "flex",
            gap: spacing.sm,
            justifyContent: "flex-end",
            padding: `${spacing.md}px ${spacing.lg}px`,
            borderTop: `1px solid ${colors.border}`,
            background: colors.bgSoft,
          }}
        >
          <button type="button" onClick={onClose} style={secondaryButton}>
            Cancel
          </button>
          <button type="button" onClick={submit} style={primaryButton}>
            Create artboard
          </button>
        </footer>
      </div>
    </div>
  );
}

function groupPresets(
  presets: ArtboardPreset[],
): Map<ArtboardPresetCategory, ArtboardPreset[]> {
  const out = new Map<ArtboardPresetCategory, ArtboardPreset[]>();
  for (const p of presets) {
    const bucket = out.get(p.category);
    if (bucket) {
      bucket.push(p);
    } else {
      out.set(p.category, [p]);
    }
  }
  return out;
}

function PresetButton({
  preset,
  selected,
  onSelect,
}: {
  preset: ArtboardPreset;
  selected: boolean;
  onSelect: () => void;
}): JSX.Element {
  return (
    <button
      type="button"
      onClick={onSelect}
      title={`${preset.width} × ${preset.height}`}
      style={{
        textAlign: "left",
        background: selected ? colors.bgSoft : colors.bg,
        border: `1px solid ${selected ? colors.accent : colors.border}`,
        borderRadius: radius.card / 2,
        padding: `${spacing.xs}px ${spacing.sm}px`,
        cursor: "pointer",
        display: "flex",
        flexDirection: "column",
        gap: 2,
      }}
    >
      <span style={{ fontSize: 12, fontWeight: 500, color: colors.text }}>
        {preset.name}
      </span>
      <span style={{ fontSize: 10, color: colors.textMuted }}>
        {preset.width} × {preset.height}
      </span>
    </button>
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
        gap: spacing.xs,
        fontSize: 12,
        color: colors.textMuted,
      }}
    >
      <span style={{ fontWeight: 500 }}>{label}</span>
      {children}
    </label>
  );
}

const inputStyle: React.CSSProperties = {
  fontFamily: font.family,
  background: colors.bg,
  color: colors.text,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.card / 2,
  padding: `${spacing.xs}px ${spacing.sm}px`,
  fontSize: 13,
};

const primaryButton: React.CSSProperties = {
  background: colors.accent,
  color: colors.textInverse,
  border: "none",
  borderRadius: radius.card / 2,
  padding: `${spacing.xs}px ${spacing.md}px`,
  fontSize: 13,
  fontWeight: 500,
  cursor: "pointer",
};

const secondaryButton: React.CSSProperties = {
  background: "transparent",
  color: colors.text,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.card / 2,
  padding: `${spacing.xs}px ${spacing.md}px`,
  fontSize: 13,
  cursor: "pointer",
};
