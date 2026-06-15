// TemplateGallery — Workstream G2 (ready-made template library).
//
// The "jump-start" surface: a polished, browsable gallery of the
// curated starter templates that ship with KCreate (seeded into the
// marketplace directory on first run by
// `kcreate_core::template_library::seed_bundled_templates`). Lets the
// user filter by `TemplateCategory`, search by name / tag /
// description, preview a larger render, and then either
// **Start from template** (open a new project poured from the
// template's design) or **Duplicate & remix** (same, plus a duplicated
// artboard so the original stays pristine while they iterate).
//
// Real implementation; no placeholder data. Templates and their
// thumbnails come straight from the Rust bridge:
//   * `window.kcreate.templateMarketplace.list(category?, query?)`
//     -> the seeded `TemplateManifest`s.
//   * `window.kcreate.templateMarketplace.thumbnail(id)`
//     -> a real PNG rendered through the shared export pipeline
//        (lazily rendered + cached as `thumbnail.png` on the bridge
//        side), pinned as an `<img src>` so cards show the actual
//        applied design rather than a coloured rectangle.
// The "start" / "remix" actions are delegated to the host (App.tsx),
// which owns project creation + routing into the editor — this
// component is a presentation + selection layer only, mirroring how
// HomePage delegates `onOpenEditor`.

import { useEffect, useMemo, useRef, useState } from "react";

import type {
  TemplateCategory,
  TemplateManifest,
  ThumbnailBytes,
} from "../../../shared/scene";
import { colors, font, radius, shadow, spacing } from "../styles/tokens";
import { errorMessage } from "../lib/errorMessage";
import {
  ALL_CATEGORIES,
  CATEGORY_LABELS,
  CATEGORY_TINT,
} from "../lib/templateCategories";
import { Icon } from "./Icon";

export interface TemplateGalleryProps {
  /** Return to the HomePage without starting a project. */
  onBack: () => void;
  /**
   * Open a new project populated from `templateId`. `opts.remix`
   * additionally duplicates the instantiated artboard so the user
   * edits a copy. Returns a promise so the gallery can disable the
   * action + surface failures inline; on success the host navigates
   * away (this component unmounts), so the resolved value is unused.
   */
  onStartFromTemplate: (
    templateId: string,
    opts: { remix: boolean },
  ) => Promise<void>;
}

/**
 * `data:image/png;base64,…` is the cheapest way to ship the bridge's
 * cached PNG bytes into a React `<img>` without a
 * `URL.createObjectURL` round-trip (which leaks across HMR reloads).
 * The bridge always emits standard (non-URL-safe) base64. Identical
 * to HomePage's helper — kept local so the gallery has no cross-page
 * import coupling.
 */
function dataUrlFor(bytes: ThumbnailBytes): string {
  return `data:${bytes.mime};base64,${bytes.bytesBase64}`;
}

/**
 * `idle` — nothing requested yet (pre-mount). `loading` — a
 * `list()` call is in flight. `ready` — we have a (possibly empty)
 * result set. `error` — the bridge call threw; surfaced inline so the
 * user can retry via the search/filter controls.
 */
type LoadState =
  | { kind: "idle" }
  | { kind: "loading" }
  | { kind: "ready"; templates: TemplateManifest[] }
  | { kind: "error"; message: string };

/**
 * Stable empty-array identity for the non-`ready` load states, so the
 * memoised `templates` value below doesn't change reference every
 * render (which would retrigger the selection + thumbnail effects).
 */
const EMPTY_TEMPLATES: ReadonlyArray<TemplateManifest> = [];

export function TemplateGallery({
  onBack,
  onStartFromTemplate,
}: TemplateGalleryProps): JSX.Element {
  const [state, setState] = useState<LoadState>({ kind: "idle" });
  // Raw vs. debounced query: the input drives `rawQuery` on every
  // keystroke; `query` trails it by a short debounce so we don't fire
  // a bridge `list()` per character.
  const [rawQuery, setRawQuery] = useState("");
  const [query, setQuery] = useState("");
  const [category, setCategory] = useState<TemplateCategory | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  // Thumbnail bytes keyed by template id. `null` marks a failed /
  // unavailable render so we show the fallback once instead of
  // retrying every render. Held across filter changes so re-filtering
  // never re-decodes an already-fetched preview.
  const [thumbs, setThumbs] = useState<Record<string, ThumbnailBytes | null>>(
    {},
  );
  // The template id currently being started (+ whether as a remix), so
  // we can disable both actions and show progress. Cleared on failure;
  // on success the host unmounts us.
  const [starting, setStarting] = useState<{
    id: string;
    remix: boolean;
  } | null>(null);
  const [startError, setStartError] = useState<string | null>(null);
  // Template ids whose thumbnail fetch has already been dispatched.
  // A `Set` in a ref (not state) so recording a dispatch never
  // retriggers the fetch effect — that feedback loop is exactly what
  // caused the O(N²) duplicate-IPC storm (Devin Review PR #61
  // BUG_0001): keying the effect off `thumbs` re-ran it on every
  // resolved preview and re-dispatched every still-in-flight id. Each
  // id is recorded here exactly once for the gallery's lifetime, so a
  // template is never fetched twice even across filter/search changes.
  const requestedRef = useRef<Set<string>>(new Set());
  // True while this component is mounted. Thumbnail promises check it
  // (instead of a per-effect-run `cancelled` flag) before committing
  // bytes, so a result dispatched under one filter still lands after a
  // quick filter change re-runs the effect — only a real unmount drops
  // it.
  const mountedRef = useRef(true);
  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  // Debounce the search box. 180ms is long enough to coalesce a burst
  // of keystrokes but short enough to feel instant.
  useEffect(() => {
    const handle = setTimeout(() => setQuery(rawQuery.trim()), 180);
    return () => clearTimeout(handle);
  }, [rawQuery]);

  // Fetch the template list whenever the category filter or the
  // debounced query changes. Convention (mirrors the shared bridge
  // docstring): a non-empty query dominates, so we drop the category
  // filter while searching — the search bar is the primary lens.
  useEffect(() => {
    let cancelled = false;
    setState((prev) =>
      prev.kind === "ready" ? prev : { kind: "loading" },
    );
    const effectiveCategory = query ? undefined : (category ?? undefined);
    const effectiveQuery = query || undefined;
    void window.kcreate.templateMarketplace
      .list(effectiveCategory, effectiveQuery)
      .then((report) => {
        if (cancelled) return;
        setState({ kind: "ready", templates: report.templates });
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        setState({ kind: "error", message: errorMessage(e) });
      });
    return () => {
      cancelled = true;
    };
  }, [category, query]);

  // Memoised so its identity is stable across renders where the load
  // state is unchanged — otherwise the `[]` fallback would be a fresh
  // array every render and retrigger the selection / thumbnail effects.
  const templates = useMemo(
    () => (state.kind === "ready" ? state.templates : EMPTY_TEMPLATES),
    [state],
  );

  // Keep a valid selection: default to the first template, and if the
  // current selection falls out of the filtered set, snap back to the
  // first. Done in an effect (not render) so we don't setState during
  // render.
  useEffect(() => {
    const first = templates[0];
    if (!first) {
      if (selectedId !== null) setSelectedId(null);
      return;
    }
    const stillVisible = templates.some((t) => t.id === selectedId);
    if (!stillVisible) setSelectedId(first.id);
  }, [templates, selectedId]);

  // Lazily fetch a real thumbnail for every template we don't have one
  // for yet. Each call hits the bridge's on-disk cache after the first
  // render, so re-filtering is cheap. Failures degrade to the tinted
  // fallback tile (never block the grid).
  useEffect(() => {
    for (const template of templates) {
      if (requestedRef.current.has(template.id)) continue;
      // Record the dispatch *before* awaiting so a re-render mid-flight
      // can't double-fire it. `thumbs` is intentionally NOT a dependency
      // (and no longer read here): the persistent ref is the single
      // source of truth for "already requested".
      requestedRef.current.add(template.id);
      void window.kcreate.templateMarketplace
        .thumbnail(template.id)
        .then((bytes) => {
          if (!mountedRef.current) return;
          setThumbs((prev) =>
            template.id in prev ? prev : { ...prev, [template.id]: bytes },
          );
        })
        .catch(() => {
          if (!mountedRef.current) return;
          setThumbs((prev) =>
            template.id in prev ? prev : { ...prev, [template.id]: null },
          );
        });
    }
  }, [templates]);

  const selected = useMemo(
    () => templates.find((t) => t.id === selectedId) ?? null,
    [templates, selectedId],
  );

  async function start(remix: boolean): Promise<void> {
    if (!selected || starting) return;
    setStarting({ id: selected.id, remix });
    setStartError(null);
    try {
      await onStartFromTemplate(selected.id, { remix });
      // On success the host routes into the editor and unmounts this
      // component; nothing more to do here.
    } catch (e) {
      setStartError(errorMessage(e));
      setStarting(null);
    }
  }

  return (
    <div
      data-testid="kcreate-template-gallery"
      style={{
        display: "flex",
        flexDirection: "column",
        height: "100%",
        background: colors.bgSoft,
        fontFamily: font.family,
        color: colors.text,
      }}
    >
      <header
        style={{
          padding: `${spacing.md}px ${spacing.xl}px`,
          display: "flex",
          alignItems: "center",
          gap: spacing.md,
          background: colors.bg,
          borderBottom: `1px solid ${colors.border}`,
          flexShrink: 0,
        }}
      >
        <button
          type="button"
          onClick={onBack}
          // Lock navigation while a "Start from template" round-trip is
          // in flight: leaving mid-instantiate would orphan the scratch
          // project the host is populating (Devin Review PR #61).
          disabled={starting !== null}
          data-testid="kcreate-template-back"
          aria-label="Back to home"
          style={{
            display: "inline-flex",
            alignItems: "center",
            gap: spacing.xs,
            background: "transparent",
            border: `1px solid ${colors.border}`,
            borderRadius: radius.md,
            padding: `${spacing.xs}px ${spacing.sm}px`,
            color: colors.text,
            cursor: starting ? "default" : "pointer",
            opacity: starting ? 0.7 : 1,
            fontSize: 13,
          }}
        >
          <Icon name="arrow-left" size={16} />
          Home
        </button>
        <div
          style={{
            display: "flex",
            flexDirection: "column",
            gap: 1,
            flex: 1,
            minWidth: 0,
          }}
        >
          <span style={{ fontSize: 18, fontWeight: 600 }}>
            Template gallery
          </span>
          <span style={{ fontSize: 12, color: colors.textMuted }}>
            {state.kind === "ready"
              ? `${templates.length} ready-made design${
                  templates.length === 1 ? "" : "s"
                } — start in one click`
              : "Browse professionally-designed starter templates"}
          </span>
        </div>
        <input
          type="search"
          value={rawQuery}
          onChange={(e) => setRawQuery(e.target.value)}
          placeholder="Search templates…"
          data-testid="kcreate-template-search"
          aria-label="Search templates"
          style={{
            width: 240,
            maxWidth: "40%",
            padding: `${spacing.xs}px ${spacing.sm}px`,
            borderRadius: radius.sm,
            border: `1px solid ${colors.border}`,
            background: colors.bgSoft,
            color: colors.text,
            fontSize: 13,
          }}
        />
      </header>

      <div
        data-testid="kcreate-template-filters"
        style={{
          display: "flex",
          flexWrap: "wrap",
          gap: spacing.xs,
          padding: `${spacing.sm}px ${spacing.xl}px`,
          background: colors.bg,
          borderBottom: `1px solid ${colors.border}`,
          flexShrink: 0,
          opacity: query ? 0.5 : 1,
          pointerEvents: query ? "none" : "auto",
        }}
      >
        <CategoryChip
          label="All"
          tint={colors.accent}
          active={category === null}
          testId="kcreate-template-cat-all"
          onClick={() => setCategory(null)}
        />
        {ALL_CATEGORIES.map((cat) => (
          <CategoryChip
            key={cat}
            label={CATEGORY_LABELS[cat]}
            tint={CATEGORY_TINT[cat]}
            active={category === cat}
            testId={`kcreate-template-cat-${cat}`}
            onClick={() => setCategory(cat)}
          />
        ))}
      </div>

      <div
        style={{
          display: "flex",
          flex: 1,
          minHeight: 0,
        }}
      >
        <main
          style={{
            flex: 1,
            minWidth: 0,
            overflowY: "auto",
            padding: spacing.lg,
          }}
        >
          {state.kind === "error" ? (
            <div
              data-testid="kcreate-template-error"
              style={{
                padding: spacing.lg,
                borderRadius: radius.card,
                background: colors.dangerBg,
                color: colors.danger,
                fontSize: 13,
              }}
            >
              Couldn’t load templates: {state.message}
            </div>
          ) : state.kind !== "ready" ? (
            <div style={{ color: colors.textMuted, fontSize: 13 }}>
              Loading templates…
            </div>
          ) : templates.length === 0 ? (
            <div
              data-testid="kcreate-template-empty"
              style={{ color: colors.textMuted, fontSize: 13 }}
            >
              No templates match {query ? `“${query}”` : "this filter"}.
            </div>
          ) : (
            <div
              style={{
                display: "grid",
                gridTemplateColumns:
                  "repeat(auto-fill, minmax(200px, 1fr))",
                gap: spacing.md,
              }}
            >
              {templates.map((template) => (
                <TemplateCard
                  key={template.id}
                  template={template}
                  thumb={thumbs[template.id]}
                  selected={template.id === selectedId}
                  onSelect={() => setSelectedId(template.id)}
                />
              ))}
            </div>
          )}
        </main>

        {selected ? (
          <aside
            data-testid="kcreate-template-preview"
            style={{
              width: 340,
              flexShrink: 0,
              borderLeft: `1px solid ${colors.border}`,
              background: colors.bg,
              display: "flex",
              flexDirection: "column",
              overflowY: "auto",
            }}
          >
            <div style={{ padding: spacing.lg }}>
              <PreviewImage
                template={selected}
                thumb={thumbs[selected.id]}
              />
              <div
                style={{
                  marginTop: spacing.md,
                  display: "flex",
                  alignItems: "center",
                  gap: spacing.sm,
                }}
              >
                <CategoryBadge category={selected.category} />
                <span style={{ fontSize: 12, color: colors.textMuted }}>
                  {selected.page_count} page
                  {selected.page_count === 1 ? "" : "s"}
                </span>
              </div>
              <h3
                style={{
                  margin: `${spacing.sm}px 0 0`,
                  fontSize: 16,
                  fontWeight: 600,
                }}
              >
                {selected.name}
              </h3>
              {selected.description ? (
                <p
                  style={{
                    margin: `${spacing.xs}px 0 0`,
                    fontSize: 13,
                    lineHeight: 1.5,
                    color: colors.textMuted,
                  }}
                >
                  {selected.description}
                </p>
              ) : null}
              {selected.tags.length > 0 ? (
                <div
                  style={{
                    marginTop: spacing.sm,
                    display: "flex",
                    flexWrap: "wrap",
                    gap: spacing.xs,
                  }}
                >
                  {selected.tags.map((tag) => (
                    <span
                      key={tag}
                      style={{
                        fontSize: 11,
                        padding: `2px ${spacing.xs}px`,
                        borderRadius: radius.sm,
                        background: colors.bgSoft,
                        border: `1px solid ${colors.border}`,
                        color: colors.textMuted,
                      }}
                    >
                      {tag}
                    </span>
                  ))}
                </div>
              ) : null}

              {startError ? (
                <div
                  data-testid="kcreate-template-start-error"
                  style={{
                    marginTop: spacing.md,
                    padding: spacing.sm,
                    borderRadius: radius.sm,
                    background: colors.dangerBg,
                    color: colors.danger,
                    fontSize: 12,
                  }}
                >
                  {startError}
                </div>
              ) : null}

              <div
                style={{
                  marginTop: spacing.md,
                  display: "flex",
                  flexDirection: "column",
                  gap: spacing.sm,
                }}
              >
                <button
                  type="button"
                  data-testid="kcreate-template-start"
                  disabled={starting !== null}
                  onClick={() => void start(false)}
                  style={{
                    display: "inline-flex",
                    alignItems: "center",
                    justifyContent: "center",
                    gap: spacing.xs,
                    padding: `${spacing.sm}px ${spacing.md}px`,
                    borderRadius: radius.md,
                    border: "none",
                    background: colors.accent,
                    color: colors.textInverse,
                    fontSize: 14,
                    fontWeight: 600,
                    cursor: starting ? "default" : "pointer",
                    opacity: starting ? 0.7 : 1,
                  }}
                >
                  <Icon name="sparkles" size={16} />
                  {starting && !starting.remix
                    ? "Starting…"
                    : "Start from template"}
                </button>
                <button
                  type="button"
                  data-testid="kcreate-template-remix"
                  disabled={starting !== null}
                  onClick={() => void start(true)}
                  style={{
                    display: "inline-flex",
                    alignItems: "center",
                    justifyContent: "center",
                    gap: spacing.xs,
                    padding: `${spacing.sm}px ${spacing.md}px`,
                    borderRadius: radius.md,
                    border: `1px solid ${colors.border}`,
                    background: colors.bg,
                    color: colors.text,
                    fontSize: 14,
                    fontWeight: 600,
                    cursor: starting ? "default" : "pointer",
                    opacity: starting ? 0.7 : 1,
                  }}
                >
                  <Icon name="layers" size={16} />
                  {starting && starting.remix
                    ? "Duplicating…"
                    : "Duplicate & remix"}
                </button>
              </div>
            </div>
          </aside>
        ) : null}
      </div>
    </div>
  );
}

function CategoryChip({
  label,
  tint,
  active,
  testId,
  onClick,
}: {
  label: string;
  tint: string;
  active: boolean;
  testId: string;
  onClick: () => void;
}): JSX.Element {
  return (
    <button
      type="button"
      data-testid={testId}
      aria-pressed={active}
      onClick={onClick}
      style={{
        padding: `${spacing.xs}px ${spacing.sm}px`,
        borderRadius: radius.pill,
        border: `1px solid ${active ? tint : colors.border}`,
        background: active ? tint : colors.bg,
        color: active ? "#FFFFFF" : colors.textMuted,
        fontSize: 12,
        fontWeight: active ? 600 : 500,
        cursor: "pointer",
        whiteSpace: "nowrap",
      }}
    >
      {label}
    </button>
  );
}

function CategoryBadge({
  category,
}: {
  category: TemplateCategory;
}): JSX.Element {
  const tint = CATEGORY_TINT[category];
  return (
    <span
      style={{
        fontSize: 11,
        fontWeight: 600,
        padding: `2px ${spacing.xs}px`,
        borderRadius: radius.sm,
        // Low-alpha tint background with the full-strength tint as
        // text; the hex+"1A" suffix is safe here because the tints
        // are concrete hex literals (unlike the `var(--kc-*)` accent).
        background: `${tint}1A`,
        color: tint,
      }}
    >
      {CATEGORY_LABELS[category]}
    </span>
  );
}

function TemplateCard({
  template,
  thumb,
  selected,
  onSelect,
}: {
  template: TemplateManifest;
  thumb: ThumbnailBytes | null | undefined;
  selected: boolean;
  onSelect: () => void;
}): JSX.Element {
  return (
    <button
      type="button"
      data-testid={`kcreate-template-card-${template.id}`}
      aria-pressed={selected}
      onClick={onSelect}
      style={{
        textAlign: "left",
        background: colors.bg,
        border: `1px solid ${selected ? colors.accent : colors.border}`,
        outline: selected ? `2px solid ${colors.accentRing}` : "none",
        borderRadius: radius.card,
        padding: 0,
        overflow: "hidden",
        boxShadow: shadow.card,
        display: "flex",
        flexDirection: "column",
        cursor: "pointer",
        transition: "box-shadow 120ms ease, transform 120ms ease",
      }}
      onMouseEnter={(e) => {
        e.currentTarget.style.boxShadow = shadow.cardHover;
        e.currentTarget.style.transform = "translateY(-1px)";
      }}
      onMouseLeave={(e) => {
        e.currentTarget.style.boxShadow = shadow.card;
        e.currentTarget.style.transform = "translateY(0)";
      }}
    >
      <ThumbBox template={template} thumb={thumb} height={150} />
      <div
        style={{
          padding: spacing.sm,
          display: "flex",
          flexDirection: "column",
          gap: spacing.xs,
        }}
      >
        <span
          style={{
            fontSize: 13,
            fontWeight: 600,
            color: colors.text,
            overflow: "hidden",
            textOverflow: "ellipsis",
            whiteSpace: "nowrap",
          }}
        >
          {template.name}
        </span>
        <CategoryBadge category={template.category} />
      </div>
    </button>
  );
}

function PreviewImage({
  template,
  thumb,
}: {
  template: TemplateManifest;
  thumb: ThumbnailBytes | null | undefined;
}): JSX.Element {
  return <ThumbBox template={template} thumb={thumb} height={260} />;
}

/**
 * Shared thumbnail surface. Renders the real PNG when we have bytes,
 * a neutral "rendering…" shimmer while the fetch is in flight
 * (`undefined`), and a tinted fallback tile with the template's
 * initial when the render failed (`null`). `objectFit: contain`
 * keeps portrait (mobile), square (social), and landscape (deck)
 * aspect ratios all legible inside the same fixed-height box.
 */
function ThumbBox({
  template,
  thumb,
  height,
}: {
  template: TemplateManifest;
  thumb: ThumbnailBytes | null | undefined;
  height: number;
}): JSX.Element {
  const tint = CATEGORY_TINT[template.category];
  return (
    <div
      style={{
        height,
        width: "100%",
        background: colors.bgSoft,
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        borderBottom: `1px solid ${colors.border}`,
        overflow: "hidden",
      }}
    >
      {thumb ? (
        <img
          src={dataUrlFor(thumb)}
          alt={`${template.name} preview`}
          style={{
            maxWidth: "100%",
            maxHeight: "100%",
            objectFit: "contain",
            display: "block",
          }}
        />
      ) : thumb === null ? (
        <div
          aria-hidden="true"
          style={{
            width: 48,
            height: 48,
            borderRadius: radius.md,
            background: `${tint}1A`,
            color: tint,
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
          }}
        >
          <Icon name="image" size={22} />
        </div>
      ) : (
        <span style={{ fontSize: 12, color: colors.textMuted }}>
          Rendering…
        </span>
      )}
    </div>
  );
}
