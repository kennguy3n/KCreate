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

import type { LayoutTemplate } from "../../../shared/scene";
import { colors, font, radius, spacing } from "../styles/tokens";
import { errorMessage } from "../lib/errorMessage";
import { CATEGORY_LABELS, CATEGORY_TINT } from "../lib/templateCategories";
import { LayoutThumbnail } from "./LayoutThumbnail";

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
  const accent = CATEGORY_TINT[template.category];
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
      {previewPage ? (
        <div style={{ marginBottom: spacing.sm }}>
          <LayoutThumbnail
            page={previewPage}
            accent={accent}
            tokens={template.design_tokens}
            label={template.name}
          >
            <span style={pageCountPillStyle}>
              {template.pages.length} page
              {template.pages.length === 1 ? "" : "s"}
            </span>
          </LayoutThumbnail>
        </div>
      ) : (
        // A pageless template (only possible for a malformed plugin- or
        // AI-contributed template) still gets a framed thumbnail slot so
        // every card keeps the same shape and the grid stays aligned.
        <div style={cardThumbPlaceholderStyle}>
          <span style={pageCountPillStyle}>No pages</span>
          No preview
        </div>
      )}
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
  return (
    <div>
      <div style={{ marginBottom: spacing.sm }}>
        <LayoutThumbnail
          page={page}
          accent={CATEGORY_TINT[template.category]}
          tokens={template.design_tokens}
          label={`${template.name} \u2014 ${page.name}`}
        />
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

const cardThumbPlaceholderStyle: React.CSSProperties = {
  position: "relative",
  width: "100%",
  aspectRatio: "4 / 3",
  marginBottom: spacing.sm,
  background: "#FAFAFA",
  border: `1px solid ${colors.border}`,
  borderRadius: 8,
  display: "flex",
  alignItems: "center",
  justifyContent: "center",
  color: colors.textMuted,
  fontSize: 11,
};

const pageCountPillStyle: React.CSSProperties = {
  position: "absolute",
  top: 6,
  right: 6,
  padding: "2px 8px",
  borderRadius: radius.pill,
  background: "rgba(15, 23, 42, 0.72)",
  color: "#FFFFFF",
  fontSize: 10,
  fontWeight: 600,
  letterSpacing: 0.2,
  pointerEvents: "none",
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
