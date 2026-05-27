// TemplatePicker — Phase 2, Block B, Task 11/12.
//
// Modal shown the first time the user enters Layout mode in a project
// (and on demand via the "New from Template" button). Lets the user
// pick one of the built-in `kcreate_core::LayoutTemplate`s and apply
// it to the current project. The Rust bridge owns the catalogue and
// the page-creation step — this component only does selection.
//
// Real implementation; no placeholder data, no stub. Templates come
// from `window.kcreate.layoutStudio.listTemplates()`, which serialises
// the static catalogue defined in
// `kcreate_core::project::layout_template::builtin_templates`.

import { useEffect, useMemo, useState } from "react";

import type {
  LayoutTemplate,
  PageSize,
  TemplateCategory,
} from "../../../shared/scene";
import { colors, font, radius, spacing } from "../styles/tokens";

export interface TemplatePickerProps {
  /** Controls visibility. Parent owns the open/closed state. */
  open: boolean;
  /** Dismiss without applying. */
  onClose: () => void;
  /**
   * Called after a template was successfully applied. Receives the
   * list of new page ids returned by
   * `layoutStudio.applyTemplate(templateId)`. The host typically
   * refreshes the document tree and selects the first created page.
   */
  onApplied: (createdPageIds: string[]) => void;
  /** Status bar tap for transient error / progress messages. */
  onStatus?: (msg: string | null) => void;
}

const CATEGORY_LABELS: Record<TemplateCategory, string> = {
  pitch_deck: "Pitch Deck",
  proposal: "Proposal",
  brochure: "Brochure",
  flyer: "Flyer",
  report: "Report",
  custom: "Custom",
};

const CATEGORY_TINT: Record<TemplateCategory, string> = {
  pitch_deck: "#7E22CE",
  proposal: "#1D4ED8",
  brochure: "#0D9488",
  flyer: "#EA580C",
  report: "#374151",
  custom: "#4B5563",
};

export function TemplatePicker({
  open,
  onClose,
  onApplied,
  onStatus,
}: TemplatePickerProps): JSX.Element | null {
  const [templates, setTemplates] = useState<LayoutTemplate[] | null>(null);
  const [loading, setLoading] = useState<boolean>(false);
  const [applying, setApplying] = useState<boolean>(false);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [previewIndex, setPreviewIndex] = useState<number>(0);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  // Pull the template catalogue from the bridge whenever the modal
  // opens. We don't cache across opens — the catalogue is static today
  // but plugin-contributed templates (Phase 2+) will make it dynamic.
  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    setLoading(true);
    setErrorMsg(null);
    (async () => {
      try {
        const list = await window.kcreate.layoutStudio.listTemplates();
        if (cancelled) return;
        setTemplates(list);
        // Reset preview state for the new open.
        setSelectedId(list[0]?.id ?? null);
        setPreviewIndex(0);
      } catch (e) {
        if (cancelled) return;
        setErrorMsg(errorMessage(e));
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [open]);

  // Close on Escape.
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === "Escape" && !applying) {
        e.preventDefault();
        onClose();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose, applying]);

  const selectedTemplate = useMemo<LayoutTemplate | null>(() => {
    if (!templates || !selectedId) return null;
    return templates.find((t) => t.id === selectedId) ?? null;
  }, [templates, selectedId]);

  const handleApply = async (): Promise<void> => {
    if (!selectedTemplate) return;
    setApplying(true);
    setErrorMsg(null);
    try {
      const ids = await window.kcreate.layoutStudio.applyTemplate(
        selectedTemplate.id,
      );
      onStatus?.(
        `Applied "${selectedTemplate.name}" (${ids.length} page${ids.length === 1 ? "" : "s"} created)`,
      );
      onApplied(ids);
      onClose();
    } catch (e) {
      setErrorMsg(`Apply failed: ${errorMessage(e)}`);
    } finally {
      setApplying(false);
    }
  };

  const handleBlank = (): void => {
    // "Blank" is a no-op: dismiss the modal and let the user use the
    // existing empty project. We still notify the host with an empty
    // id list so it can clear the "first-time" sentinel.
    onApplied([]);
    onClose();
  };

  if (!open) return null;

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-labelledby="template-picker-title"
      style={overlayStyle}
      onClick={() => {
        if (!applying) onClose();
      }}
    >
      <div
        style={dialogStyle}
        onClick={(e) => e.stopPropagation()}
      >
        <header style={headerStyle}>
          <h2 id="template-picker-title" style={titleStyle}>
            Pick a layout template
          </h2>
          <button
            type="button"
            onClick={onClose}
            disabled={applying}
            aria-label="Close"
            style={iconButtonStyle}
          >
            ×
          </button>
        </header>
        {loading ? (
          <div style={messageStyle}>Loading templates…</div>
        ) : errorMsg ? (
          <div style={{ ...messageStyle, color: "#B91C1C" }}>{errorMsg}</div>
        ) : !templates || templates.length === 0 ? (
          <div style={messageStyle}>No templates available.</div>
        ) : (
          <div style={bodyStyle}>
            <div style={gridStyle} role="listbox" aria-label="Templates">
              {templates.map((t) => (
                <TemplateCard
                  key={t.id}
                  template={t}
                  selected={t.id === selectedId}
                  onSelect={() => {
                    setSelectedId(t.id);
                    setPreviewIndex(0);
                  }}
                />
              ))}
              <BlankCard onSelect={handleBlank} />
            </div>
            {selectedTemplate ? (
              <aside style={previewStyle}>
                <PreviewHeader template={selectedTemplate} />
                <PreviewCarousel
                  template={selectedTemplate}
                  index={previewIndex}
                  onPrev={() =>
                    setPreviewIndex((i) =>
                      i === 0
                        ? Math.max(selectedTemplate.pages.length - 1, 0)
                        : i - 1,
                    )
                  }
                  onNext={() =>
                    setPreviewIndex((i) =>
                      i + 1 >= selectedTemplate.pages.length ? 0 : i + 1,
                    )
                  }
                />
                <button
                  type="button"
                  onClick={() => {
                    void handleApply();
                  }}
                  disabled={applying}
                  style={applyButtonStyle}
                >
                  {applying
                    ? "Applying…"
                    : `Use "${selectedTemplate.name}"`}
                </button>
              </aside>
            ) : null}
          </div>
        )}
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------
// Sub-components
// ---------------------------------------------------------------------

interface TemplateCardProps {
  template: LayoutTemplate;
  selected: boolean;
  onSelect: () => void;
}

function TemplateCard({
  template,
  selected,
  onSelect,
}: TemplateCardProps): JSX.Element {
  const previewPage = template.pages[0];
  const aspect = previewPage
    ? aspectFromPageSize(previewPage.page_size, previewPage.orientation === "portrait")
    : 0.71;
  return (
    <button
      type="button"
      role="option"
      aria-selected={selected}
      onClick={onSelect}
      style={{
        ...cardStyle,
        borderColor: selected ? colors.accent : colors.border,
        boxShadow: selected
          ? `0 0 0 2px ${colors.accentRing}`
          : "none",
      }}
    >
      <div
        style={{
          width: "100%",
          aspectRatio: aspect.toString(),
          background: "#FAFAFA",
          border: `1px solid ${colors.border}`,
          borderRadius: 6,
          marginBottom: spacing.sm,
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          color: colors.textMuted,
          fontSize: 11,
        }}
      >
        {template.pages.length} page{template.pages.length === 1 ? "" : "s"}
      </div>
      <div style={cardTitleStyle}>{template.name}</div>
      <div style={cardCategoryStyle}>
        <span
          style={{
            ...categoryBadgeStyle,
            background: `${CATEGORY_TINT[template.category]}1A`,
            color: CATEGORY_TINT[template.category],
          }}
        >
          {CATEGORY_LABELS[template.category]}
        </span>
      </div>
      <div style={cardDescriptionStyle}>{template.description}</div>
    </button>
  );
}

function BlankCard({ onSelect }: { onSelect: () => void }): JSX.Element {
  return (
    <button type="button" onClick={onSelect} style={blankCardStyle}>
      <div style={blankIconStyle}>+</div>
      <div style={cardTitleStyle}>Blank</div>
      <div style={cardDescriptionStyle}>Start from an empty project.</div>
    </button>
  );
}

interface PreviewHeaderProps {
  template: LayoutTemplate;
}

function PreviewHeader({ template }: PreviewHeaderProps): JSX.Element {
  return (
    <header style={{ marginBottom: spacing.sm }}>
      <div style={previewNameStyle}>{template.name}</div>
      <div style={previewDescriptionStyle}>{template.description}</div>
      <div style={previewMetaRowStyle}>
        <span
          style={{
            ...categoryBadgeStyle,
            background: `${CATEGORY_TINT[template.category]}1A`,
            color: CATEGORY_TINT[template.category],
          }}
        >
          {CATEGORY_LABELS[template.category]}
        </span>
        <span style={previewMetaStyle}>
          {template.pages.length} page{template.pages.length === 1 ? "" : "s"}
        </span>
        {template.design_tokens ? (
          <span style={previewMetaStyle}>Design tokens included</span>
        ) : null}
      </div>
    </header>
  );
}

interface PreviewCarouselProps {
  template: LayoutTemplate;
  index: number;
  onPrev: () => void;
  onNext: () => void;
}

function PreviewCarousel({
  template,
  index,
  onPrev,
  onNext,
}: PreviewCarouselProps): JSX.Element {
  const page = template.pages[index];
  if (!page) {
    return <div style={messageStyle}>No pages.</div>;
  }
  const portrait = page.orientation === "portrait";
  const aspect = aspectFromPageSize(page.page_size, portrait);
  return (
    <div>
      <div
        style={{
          position: "relative",
          width: "100%",
          aspectRatio: aspect.toString(),
          background: "#FAFAFA",
          border: `1px solid ${colors.border}`,
          borderRadius: 6,
          marginBottom: spacing.sm,
          overflow: "hidden",
        }}
      >
        {/* Block out the section boxes so the preview reads as a real layout. */}
        {page.sections.map((s, i) => (
          <div
            key={i}
            title={s.kind}
            style={{
              position: "absolute",
              left: `${s.bounds.x * 100}%`,
              top: `${s.bounds.y * 100}%`,
              width: `${s.bounds.width * 100}%`,
              height: `${s.bounds.height * 100}%`,
              background: sectionTint(s.kind),
              border: `1px solid ${colors.border}`,
              fontSize: 9,
              color: colors.textMuted,
              padding: 2,
              boxSizing: "border-box",
              overflow: "hidden",
            }}
          >
            {s.placeholder_text ?? s.kind}
          </div>
        ))}
      </div>
      <div
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          marginBottom: spacing.sm,
        }}
      >
        <button
          type="button"
          onClick={onPrev}
          disabled={template.pages.length <= 1}
          style={navButtonStyle}
          aria-label="Previous page"
        >
          ‹
        </button>
        <span style={previewMetaStyle}>
          {page.name} ({index + 1} / {template.pages.length})
        </span>
        <button
          type="button"
          onClick={onNext}
          disabled={template.pages.length <= 1}
          style={navButtonStyle}
          aria-label="Next page"
        >
          ›
        </button>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------

function aspectFromPageSize(size: PageSize, portrait: boolean): number {
  // Ratios are width/height. Returning portrait or landscape on demand
  // keeps the preview thumbnail proportional to the real page.
  let portraitRatio: number;
  if (size.kind === "custom") {
    portraitRatio = size.height_mm > 0 ? size.width_mm / size.height_mm : 1;
  } else {
    portraitRatio = PORTRAIT_ASPECTS[size.kind] ?? 210 / 297;
  }
  return portrait ? portraitRatio : 1 / portraitRatio;
}

const PORTRAIT_ASPECTS: Record<string, number> = {
  a4: 210 / 297,
  a3: 297 / 420,
  a5: 148 / 210,
  letter: 8.5 / 11,
  legal: 8.5 / 14,
  tabloid: 11 / 17,
  presentation_16x9: 9 / 16,
  presentation_4x3: 3 / 4,
};

function sectionTint(kind: string): string {
  switch (kind) {
    case "title":
      return "#DBEAFE";
    case "subtitle":
      return "#E0E7FF";
    case "body_text":
      return "#F1F5F9";
    case "image":
      return "#FEF3C7";
    case "chart":
      return "#DCFCE7";
    case "footer":
      return "#F3F4F6";
    case "page_number":
      return "#F3F4F6";
    default:
      return "#F8FAFC";
  }
}

function errorMessage(e: unknown): string {
  if (e instanceof Error) return e.message;
  return String(e);
}

// ---------------------------------------------------------------------
// Styles
// ---------------------------------------------------------------------

const overlayStyle: React.CSSProperties = {
  position: "fixed",
  inset: 0,
  background: "rgba(17, 24, 39, 0.55)",
  display: "flex",
  alignItems: "center",
  justifyContent: "center",
  zIndex: 2000,
  fontFamily: font.family,
};

const dialogStyle: React.CSSProperties = {
  width: "min(960px, 95vw)",
  maxHeight: "90vh",
  background: colors.bg,
  borderRadius: radius.card,
  boxShadow: "0 24px 64px rgba(0,0,0,0.24)",
  display: "flex",
  flexDirection: "column",
  overflow: "hidden",
};

const headerStyle: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  justifyContent: "space-between",
  padding: `${spacing.md}px ${spacing.lg}px`,
  borderBottom: `1px solid ${colors.border}`,
};

const titleStyle: React.CSSProperties = {
  margin: 0,
  fontSize: 16,
  fontWeight: 600,
  color: colors.text,
};

const iconButtonStyle: React.CSSProperties = {
  width: 28,
  height: 28,
  fontSize: 18,
  background: "transparent",
  border: "none",
  cursor: "pointer",
  color: colors.textMuted,
};

const bodyStyle: React.CSSProperties = {
  display: "grid",
  gridTemplateColumns: "1fr 320px",
  gap: spacing.lg,
  padding: spacing.lg,
  overflow: "auto",
};

const gridStyle: React.CSSProperties = {
  display: "grid",
  gridTemplateColumns: "repeat(auto-fill, minmax(200px, 1fr))",
  gap: spacing.md,
  alignContent: "flex-start",
};

const cardStyle: React.CSSProperties = {
  textAlign: "left",
  padding: spacing.md,
  background: colors.bg,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.card,
  cursor: "pointer",
  display: "flex",
  flexDirection: "column",
  gap: 2,
  color: colors.text,
};

const blankCardStyle: React.CSSProperties = {
  ...cardStyle,
  borderStyle: "dashed",
  alignItems: "center",
  justifyContent: "center",
  textAlign: "center",
  minHeight: 220,
};

const blankIconStyle: React.CSSProperties = {
  fontSize: 32,
  fontWeight: 200,
  color: colors.textMuted,
  marginBottom: spacing.sm,
};

const cardTitleStyle: React.CSSProperties = {
  fontSize: 13,
  fontWeight: 600,
  color: colors.text,
};

const cardCategoryStyle: React.CSSProperties = {
  display: "flex",
  gap: spacing.xs,
  marginBottom: 2,
};

const categoryBadgeStyle: React.CSSProperties = {
  display: "inline-flex",
  alignItems: "center",
  padding: "1px 8px",
  borderRadius: radius.pill,
  fontSize: 10,
  fontWeight: 600,
  letterSpacing: 0.3,
  textTransform: "uppercase",
};

const cardDescriptionStyle: React.CSSProperties = {
  fontSize: 11,
  color: colors.textMuted,
  lineHeight: 1.4,
};

const previewStyle: React.CSSProperties = {
  background: colors.bgSoft,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.card,
  padding: spacing.md,
  display: "flex",
  flexDirection: "column",
  gap: spacing.sm,
  alignSelf: "flex-start",
};

const previewNameStyle: React.CSSProperties = {
  fontSize: 14,
  fontWeight: 600,
  color: colors.text,
};

const previewDescriptionStyle: React.CSSProperties = {
  fontSize: 11,
  color: colors.textMuted,
  marginTop: 2,
  marginBottom: spacing.sm,
};

const previewMetaRowStyle: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: spacing.xs,
  flexWrap: "wrap",
};

const previewMetaStyle: React.CSSProperties = {
  fontSize: 11,
  color: colors.textMuted,
};

const navButtonStyle: React.CSSProperties = {
  width: 28,
  height: 28,
  background: colors.bg,
  border: `1px solid ${colors.border}`,
  borderRadius: 6,
  cursor: "pointer",
  fontSize: 14,
  color: colors.text,
};

const applyButtonStyle: React.CSSProperties = {
  padding: "8px 14px",
  background: colors.accent,
  color: colors.textInverse,
  border: "none",
  borderRadius: radius.pill,
  fontSize: 12,
  fontWeight: 600,
  cursor: "pointer",
};

const messageStyle: React.CSSProperties = {
  padding: spacing.lg,
  color: colors.textMuted,
  fontSize: 13,
  textAlign: "center",
};
