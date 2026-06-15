import { useEffect, useMemo, useState } from "react";

import type {
  ArtboardInfo,
  ArtboardPreset,
  ArtboardPresetCategory,
  MagicResizeContent,
  ResizeTarget,
} from "../../../shared/scene";
import { colors, font, radius, shadow, spacing } from "../styles/tokens";

export interface MagicResizeDialogProps {
  /** When `false` the dialog renders nothing. Owned by the parent. */
  open: boolean;
  /**
   * The artboard whose design is being reflowed. `null` while the
   * dialog is closed; the parent supplies it when opening.
   */
  source: ArtboardInfo | null;
  /** Presets loaded from the bridge (the selectable target sizes). */
  presets: ArtboardPreset[];
  /**
   * Called when the user confirms with one or more selected targets.
   * The parent owns the `artboard.magicResize` IPC round-trip +
   * refreshing the tree. `content` carries the content-aware toggles
   * (text re-fit + image smart-crop).
   */
  onResize: (
    targets: ResizeTarget[],
    content: MagicResizeContent,
  ) => void | Promise<void>;
  /**
   * Called when the user picks "Resize & export all": reflow onto every
   * selected target AND render each to a PNG in one action. The parent
   * owns the directory picker + `artboard.magicResizeExportPng` IPC.
   *
   * When the handler returns a `Promise`, the dialog drops its busy
   * latch once that promise settles. That matters for paths that leave
   * the dialog open instead of closing it — e.g. the user cancels the
   * export directory picker — so the action buttons re-enable rather
   * than staying stuck on "Exporting…".
   */
  onExport: (
    targets: ResizeTarget[],
    content: MagicResizeContent,
  ) => void | Promise<void>;
  /** Cancel — close without resizing. */
  onClose: () => void;
}

/// Human-readable category labels, ordered as they appear in the
/// grouped grid. Mirrors `ArtboardDialog` so the two surfaces read as
/// the same control.
const CATEGORY_LABELS: ReadonlyArray<{
  id: ArtboardPresetCategory;
  label: string;
}> = [
  { id: "social_media", label: "Social Media" },
  { id: "print", label: "Print" },
  { id: "web_desktop", label: "Web — Desktop" },
  { id: "web_tablet", label: "Web — Tablet" },
  { id: "web_mobile", label: "Web — Mobile" },
  { id: "custom", label: "Custom" },
];

/// Stable key for a preset. Preset names are unique within the
/// catalogue, but we fold in the dimensions too so a future duplicate
/// label can't collapse two distinct sizes into one selection.
function presetKey(p: ArtboardPreset): string {
  return `${p.name}|${p.width}×${p.height}`;
}

export function MagicResizeDialog({
  open,
  source,
  presets,
  onResize,
  onExport,
  onClose,
}: MagicResizeDialogProps): JSX.Element | null {
  const [selected, setSelected] = useState<ReadonlySet<string>>(new Set());
  // Content-aware toggles, both on by default (Canva-like behaviour).
  const [refitText, setRefitText] = useState(true);
  const [smartCrop, setSmartCrop] = useState(true);
  // Which action is in flight (if any). Latched once the user confirms
  // so a second click can't fire a duplicate `onResize` / `onExport`
  // (and spawn duplicate artboards) during the window before the
  // parent's async handler closes the dialog.
  const [busy, setBusy] = useState<null | "resize" | "export">(null);

  // Reset the selection, toggles, and busy latch whenever the dialog
  // (re)opens or the source artboard changes, so a prior session's
  // picks don't linger.
  useEffect(() => {
    if (open) {
      setSelected(new Set());
      setRefitText(true);
      setSmartCrop(true);
      setBusy(null);
    }
  }, [open, source?.id]);

  // Close-on-ESC. Capture phase so the dialog beats editor shortcuts.
  useEffect(() => {
    if (!open) return;
    const handler = (e: KeyboardEvent): void => {
      if (e.key === "Escape") {
        e.preventDefault();
        onClose();
      }
    };
    document.addEventListener("keydown", handler, { capture: true });
    return () =>
      document.removeEventListener("keydown", handler, { capture: true });
  }, [open, onClose]);

  const grouped = useMemo(() => groupPresets(presets), [presets]);

  const byKey = useMemo(() => {
    const m = new Map<string, ArtboardPreset>();
    for (const p of presets) m.set(presetKey(p), p);
    return m;
  }, [presets]);

  // Preset keys in the order they appear on screen (category order, then
  // within-category order). `submit` walks this — not the selection Set's
  // click order — so the generated artboards are laid out left-to-right in
  // the same order the user sees the sizes, regardless of which they
  // clicked first.
  const orderedKeys = useMemo(() => {
    const keys: string[] = [];
    for (const cat of CATEGORY_LABELS) {
      const items = grouped.get(cat.id);
      if (!items) continue;
      for (const p of items) keys.push(presetKey(p));
    }
    return keys;
  }, [grouped]);

  if (!open || !source) return null;

  const toggle = (key: string): void => {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(key)) {
        next.delete(key);
      } else {
        next.add(key);
      }
      return next;
    });
  };

  const buildTargets = (): ResizeTarget[] => {
    const targets: ResizeTarget[] = [];
    for (const key of orderedKeys) {
      if (!selected.has(key)) continue;
      const preset = byKey.get(key);
      if (preset) targets.push({ preset: preset.name });
    }
    return targets;
  };

  // Drop the busy latch once a parent action settles. The content-aware
  // handlers in the parent are async (`Promise<void>`): on success they
  // close the dialog, so this component unmounts and the reset is a
  // harmless no-op; but on a path that leaves the dialog open — most
  // importantly the user cancelling the export directory picker — the
  // promise still settles, so we re-enable the buttons rather than
  // leaving them disabled forever. A synchronous (`void`) handler keeps
  // the legacy contract where the parent alone owns closing the dialog.
  const runAction = (result: void | Promise<void>): void => {
    if (result instanceof Promise) {
      void result.finally(() => setBusy(null));
    }
  };

  const submit = (): void => {
    if (busy) return;
    const targets = buildTargets();
    if (targets.length === 0) return;
    setBusy("resize");
    runAction(onResize(targets, { refitText, smartCrop }));
  };

  const exportAll = (): void => {
    if (busy) return;
    const targets = buildTargets();
    if (targets.length === 0) return;
    setBusy("export");
    runAction(onExport(targets, { refitText, smartCrop }));
  };

  const count = selected.size;
  const actionsDisabled = count === 0 || busy !== null;

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-labelledby="magic-resize-dialog-title"
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
            display: "flex",
            flexDirection: "column",
            gap: 2,
          }}
        >
          <h2
            id="magic-resize-dialog-title"
            style={{ margin: 0, fontSize: 16, fontWeight: 600 }}
          >
            Magic Resize
          </h2>
          <span style={{ fontSize: 12, color: colors.textMuted }}>
            Reflow <strong>{source.name}</strong> (
            {Math.round(source.width)} × {Math.round(source.height)}) into the
            sizes you pick. The original is kept.
          </span>
        </header>
        <div
          style={{
            flex: 1,
            overflowY: "auto",
            padding: spacing.lg,
            display: "flex",
            flexDirection: "column",
            gap: spacing.md,
            minHeight: 0,
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
                    gridTemplateColumns: "repeat(auto-fill, minmax(140px, 1fr))",
                    gap: spacing.xs,
                  }}
                >
                  {items.map((p) => {
                    const key = presetKey(p);
                    return (
                      <PresetToggle
                        key={key}
                        preset={p}
                        selected={selected.has(key)}
                        onToggle={() => toggle(key)}
                      />
                    );
                  })}
                </div>
              </div>
            );
          })}
        </div>
        <section
          style={{
            display: "flex",
            flexWrap: "wrap",
            gap: spacing.md,
            padding: `${spacing.sm}px ${spacing.lg}px`,
            borderTop: `1px solid ${colors.border}`,
          }}
        >
          <ContentToggle
            id="magic-resize-refit-text"
            label="Re-fit text to box"
            hint="Shrink-to-fit headlines so they don't overflow or vanish"
            checked={refitText}
            onChange={setRefitText}
          />
          <ContentToggle
            id="magic-resize-smart-crop"
            label="Smart-crop images"
            hint="Crop toward the subject instead of stretching on aspect change"
            checked={smartCrop}
            onChange={setSmartCrop}
          />
        </section>
        <footer
          style={{
            display: "flex",
            alignItems: "center",
            gap: spacing.sm,
            justifyContent: "flex-end",
            padding: `${spacing.md}px ${spacing.lg}px`,
            borderTop: `1px solid ${colors.border}`,
            background: colors.bgSoft,
          }}
        >
          <span
            style={{
              marginRight: "auto",
              fontSize: 12,
              color: colors.textMuted,
            }}
          >
            {count === 0
              ? "No sizes selected"
              : `${count} size${count === 1 ? "" : "s"} selected`}
          </span>
          <button type="button" onClick={onClose} style={secondaryButton}>
            Cancel
          </button>
          <button
            type="button"
            onClick={exportAll}
            disabled={actionsDisabled}
            style={{
              ...secondaryButton,
              opacity: actionsDisabled ? 0.5 : 1,
              cursor: actionsDisabled ? "not-allowed" : "pointer",
            }}
          >
            {busy === "export" ? "Exporting…" : "Resize & export all"}
          </button>
          <button
            type="button"
            onClick={submit}
            disabled={actionsDisabled}
            style={{
              ...primaryButton,
              opacity: actionsDisabled ? 0.5 : 1,
              cursor: actionsDisabled ? "not-allowed" : "pointer",
            }}
          >
            {busy === "resize"
              ? "Generating…"
              : count <= 1
                ? "Generate resize"
                : `Generate ${count} resizes`}
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

function PresetToggle({
  preset,
  selected,
  onToggle,
}: {
  preset: ArtboardPreset;
  selected: boolean;
  onToggle: () => void;
}): JSX.Element {
  return (
    <button
      type="button"
      role="checkbox"
      aria-checked={selected}
      onClick={onToggle}
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
      <span
        style={{
          fontSize: 12,
          fontWeight: 500,
          color: selected ? colors.accent : colors.text,
        }}
      >
        {preset.name}
      </span>
      <span style={{ fontSize: 10, color: colors.textMuted }}>
        {preset.width} × {preset.height}
      </span>
    </button>
  );
}

function ContentToggle({
  id,
  label,
  hint,
  checked,
  onChange,
}: {
  id: string;
  label: string;
  hint: string;
  checked: boolean;
  onChange: (next: boolean) => void;
}): JSX.Element {
  return (
    <label
      htmlFor={id}
      style={{
        display: "flex",
        alignItems: "flex-start",
        gap: spacing.xs,
        cursor: "pointer",
        flex: "1 1 240px",
      }}
    >
      <input
        id={id}
        type="checkbox"
        checked={checked}
        onChange={(e) => onChange(e.target.checked)}
        style={{ marginTop: 2, cursor: "pointer" }}
      />
      <span style={{ display: "flex", flexDirection: "column", gap: 1 }}>
        <span style={{ fontSize: 12, fontWeight: 500, color: colors.text }}>
          {label}
        </span>
        <span style={{ fontSize: 10, color: colors.textMuted }}>{hint}</span>
      </span>
    </label>
  );
}

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
