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
  ImportPickKind,
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
import { computeColumns, computeGridWindow } from "../lib/galleryWindow";
import { Icon, type IconName } from "./Icon";

/**
 * Grid geometry shared by the layout and the virtualisation math.
 * `CARD_MIN_WIDTH` is the `minmax()` floor the auto-fill grid packs to;
 * `CARD_HEIGHT` is the fixed per-card height (150px thumbnail + the
 * name/badge footer) so every row is the same height and the windowed
 * `topPad`/`totalHeight` arithmetic is exact. `GRID_GAP` matches the
 * gap applied to both the grid and the column packing.
 */
const CARD_MIN_WIDTH = 200;
const CARD_HEIGHT = 210;
const GRID_GAP = spacing.md;
/** Padding inside the scroll container (its `padding: spacing.lg`). */
const GRID_PADDING = spacing.lg;

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

/**
 * Tally a flat manifest list into a per-`category` count map. Shared by
 * the two paths that populate the filter-chip badges (the unfiltered
 * main-grid result, and the dedicated unfiltered fetch used while a
 * filter narrows the grid) so they can never drift apart.
 */
function tallyByCategory(
  templates: ReadonlyArray<TemplateManifest>,
): Record<string, number> {
  const tally: Record<string, number> = {};
  for (const t of templates) {
    tally[t.category] = (tally[t.category] ?? 0) + 1;
  }
  return tally;
}

/**
 * A single row in the "Remix from file" dropdown. Each maps to one
 * `ImportPickKind` and opens a single-purpose OS dialog (a file
 * picker or a directory picker) — splitting the two avoids the
 * Windows/Linux limitation where a combined `openFile`+`openDirectory`
 * dialog silently degrades to directory-only, making bare `*.json`
 * imports unreachable.
 */
function ImportMenuItem({
  testId,
  icon,
  title,
  subtitle,
  onClick,
}: {
  testId: string;
  icon: IconName;
  title: string;
  subtitle: string;
  onClick: () => void;
}): JSX.Element {
  return (
    <button
      type="button"
      role="menuitem"
      data-testid={testId}
      onClick={onClick}
      style={{
        display: "flex",
        alignItems: "center",
        gap: spacing.sm,
        background: "transparent",
        border: "none",
        borderRadius: 0,
        padding: `${spacing.sm}px ${spacing.md}px`,
        color: colors.text,
        cursor: "pointer",
        textAlign: "left",
        font: "inherit",
        fontSize: 13,
        width: "100%",
      }}
    >
      <Icon name={icon} size={18} />
      <span style={{ display: "flex", flexDirection: "column" }}>
        <span style={{ fontWeight: 600 }}>{title}</span>
        <span style={{ fontSize: 11, color: colors.textMuted }}>
          {subtitle}
        </span>
      </span>
    </button>
  );
}

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
  // Per-category catalog counts for the filter chips, plus the grand
  // total behind the "All" chip. Fetched once (and after an import)
  // from an unfiltered `list()` so the counts reflect the whole
  // library, not the currently-filtered view. Empty until that resolves
  // (and stays empty if it fails — counts are decorative, never block
  // the gallery).
  const [counts, setCounts] = useState<Record<string, number>>({});
  const [totalCount, setTotalCount] = useState<number | null>(null);
  // "Remix from file" import state: `importing` disables the button +
  // shows progress; `importError` surfaces a failed pick/import inline.
  // `importMenuOpen` toggles the small two-option menu (file vs.
  // package) — the OS dialog can't pick files AND directories at once
  // on Windows/Linux, so the user chooses which to open.
  const [importing, setImporting] = useState(false);
  const [importError, setImportError] = useState<string | null>(null);
  const [importMenuOpen, setImportMenuOpen] = useState(false);
  // Bumped to force the catalog (and counts) to reload — e.g. after a
  // successful import registers a new template.
  const [reloadToken, setReloadToken] = useState(0);
  // Measured scroll-container geometry driving the virtualised grid:
  // `viewport` (content width/height) feeds the column + row-window
  // math; `scrollTop` selects which rows are mounted. Both come from
  // the live `<main>` element via a ResizeObserver + scroll handler, so
  // only the cards near the viewport are in the DOM at 120+ templates.
  const scrollRef = useRef<HTMLElement | null>(null);
  const [viewport, setViewport] = useState<{ width: number; height: number }>({
    width: 0,
    height: 0,
  });
  const [scrollTop, setScrollTop] = useState(0);
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
  // A template id we want selected once a *pending* catalog reload lands
  // (set by `runImport` for the just-imported entry). The async reload
  // resolves after `runImport` returns, so the selection-guard effect
  // would otherwise run against the stale `templates` list, fail to find
  // the imported id, and snap the selection to the first card — leaving
  // the freshly-imported template in the grid but unselected. The list-
  // load effect honours this the moment the imported id appears in the
  // reloaded set; the guard leaves the selection untouched until then.
  const pendingSelectRef = useRef<string | null>(null);

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
        // Resolve a pending post-import selection now that we hold the
        // freshly-loaded list: select the imported template if it is
        // present, otherwise drop the request and let the selection guard
        // fall back to a default. Cleared unconditionally so a stale
        // request can never linger past the reload it was queued for.
        const pending = pendingSelectRef.current;
        if (pending !== null) {
          pendingSelectRef.current = null;
          if (report.templates.some((t) => t.id === pending)) {
            setSelectedId(pending);
          }
        }
        // When neither a category nor a search term is active, this
        // query already returned the WHOLE library — reuse it for the
        // chip counts instead of firing a second identical unfiltered
        // `list()` on every mount/reload (the dedicated counts effect
        // below only runs while a filter is narrowing the grid).
        if (effectiveCategory === undefined && effectiveQuery === undefined) {
          setCounts(tallyByCategory(report.templates));
          setTotalCount(report.templates.length);
        }
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        setState({ kind: "error", message: errorMessage(e) });
      });
    return () => {
      cancelled = true;
    };
  }, [category, query, reloadToken]);

  // Catalog counts for the filter chips always reflect the WHOLE
  // library, independent of the active filter. While unfiltered, the
  // main-grid query above already returned the whole library and
  // populated the counts — so a dedicated unfiltered `list()` is only
  // needed when a category/search is narrowing the grid (otherwise the
  // chip totals would shrink to the filtered slice). Failures are
  // swallowed: counts are a nicety, and the main grid already surfaces
  // load errors — we must not let this crash the gallery (the
  // `list()`-rejects test rejects every call).
  useEffect(() => {
    const filtering = (category ?? undefined) !== undefined || query !== "";
    if (!filtering) return; // counts already derived from the main grid
    let cancelled = false;
    void window.kcreate.templateMarketplace
      .list(undefined, undefined)
      .then((report) => {
        if (cancelled) return;
        setCounts(tallyByCategory(report.templates));
        setTotalCount(report.templates.length);
      })
      .catch(() => {
        /* counts are decorative; ignore load failures */
      });
    return () => {
      cancelled = true;
    };
  }, [category, query, reloadToken]);

  // Memoised so its identity is stable across renders where the load
  // state is unchanged — otherwise the `[]` fallback would be a fresh
  // array every render and retrigger the selection / thumbnail effects.
  const templates = useMemo(
    () => (state.kind === "ready" ? state.templates : EMPTY_TEMPLATES),
    [state],
  );

  // Column count from the measured track width (auto-fill, matching the
  // CSS grid below). Subtract the container padding so the count tracks
  // the real content width, not the padded box.
  const columns = useMemo(
    () =>
      computeColumns(
        Math.max(0, viewport.width - GRID_PADDING * 2),
        CARD_MIN_WIDTH,
        GRID_GAP,
      ),
    [viewport.width],
  );
  // The slice of cards to mount + the spacer geometry around them.
  // Before the viewport is measured (jsdom / first paint) this returns
  // the whole set, so tests and SSR see every card.
  const gridWindow = useMemo(
    () =>
      computeGridWindow({
        total: templates.length,
        columns,
        rowHeight: CARD_HEIGHT,
        gap: GRID_GAP,
        scrollTop,
        viewportHeight: viewport.height,
        overscanRows: 2,
      }),
    [templates.length, columns, scrollTop, viewport.height],
  );
  const visibleTemplates = useMemo(
    () => templates.slice(gridWindow.startIndex, gridWindow.endIndex),
    [templates, gridWindow.startIndex, gridWindow.endIndex],
  );

  // Measure the scroll container so the window math has a real viewport.
  // A ResizeObserver tracks width/height through layout + theme changes;
  // an explicit read seeds it before the first observer callback. (When
  // ResizeObserver is absent — jsdom — we stay at the unmeasured 0×0,
  // which makes the window return the whole set.)
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const measure = (): void =>
      setViewport({ width: el.clientWidth, height: el.clientHeight });
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // Snap back to the top of the list whenever the visible set changes
  // (category / search / import) so we never open a shorter result set
  // already scrolled past its end.
  useEffect(() => {
    if (scrollRef.current) scrollRef.current.scrollTop = 0;
    setScrollTop(0);
  }, [category, query, reloadToken]);

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
    // A post-import selection is still pending until the reload that
    // contains the imported template lands (resolved in the list-load
    // effect above). While it is outstanding, leave the selection alone:
    // snapping to the first card here would race the async reload and
    // leave the imported template unselected once it arrives.
    if (
      pendingSelectRef.current !== null &&
      !templates.some((t) => t.id === pendingSelectRef.current)
    ) {
      return;
    }
    const stillVisible = templates.some((t) => t.id === selectedId);
    if (!stillVisible) setSelectedId(first.id);
  }, [templates, selectedId]);

  // Lazily fetch a real thumbnail for every *visible* template we don't
  // have one for yet. Scoping the fetch to the windowed slice (not the
  // whole filtered set) means a 120+ catalog only renders the thumbnails
  // actually on screen; scrolling pulls in the rest, and `requestedRef`
  // dedupes so nothing is fetched twice. Each call hits the bridge's
  // on-disk cache after the first render, so re-filtering is cheap.
  // Failures degrade to the tinted fallback tile (never block the grid).
  useEffect(() => {
    for (const template of visibleTemplates) {
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
  }, [visibleTemplates]);

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

  // "Remix from file": pick an external design via the OS dialog,
  // import it as a new library template through the real bridge path,
  // then reload the catalog (clearing filters) and select the
  // freshly-imported entry. `kind` selects whether to open a file
  // picker (a bare template-content `*.json`) or a directory picker
  // (a `.kstudio` project / `.ktemplate` package) — see
  // `ImportPickKind`. A cancelled dialog (`null` path) is a no-op;
  // failures surface inline without disturbing the existing grid.
  async function runImport(kind: ImportPickKind): Promise<void> {
    if (importing) return;
    setImportMenuOpen(false);
    setImporting(true);
    setImportError(null);
    try {
      const path = await window.kcreate.templateMarketplace.pickImport(kind);
      if (!path) return;
      const imported = await window.kcreate.templateMarketplace.import({
        sourcePath: path,
      });
      setCategory(null);
      setRawQuery("");
      setQuery("");
      // Defer selection to the reload: the catalog `list()` triggered by
      // `reloadToken` below resolves asynchronously, so we record the
      // desired selection and let the list-load effect apply it once the
      // imported template is actually in `templates`. Calling
      // `setSelectedId(imported.id)` here would be clobbered by the
      // selection guard running against the pre-reload list.
      pendingSelectRef.current = imported.id;
      setReloadToken((n) => n + 1);
    } catch (e) {
      setImportError(errorMessage(e));
    } finally {
      setImporting(false);
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
          // in flight (leaving mid-instantiate would orphan the scratch
          // project the host is populating — Devin Review PR #61) or
          // while a "Remix from file" import is in flight (unmounting
          // mid-import discards the post-import selection + filter reset;
          // the import itself still completes in the background).
          disabled={starting !== null || importing}
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
            cursor: starting || importing ? "default" : "pointer",
            opacity: starting || importing ? 0.7 : 1,
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
        <div style={{ position: "relative", flexShrink: 0 }}>
          <button
            type="button"
            onClick={() => setImportMenuOpen((open) => !open)}
            // Disabled while a previous import is in flight or a
            // "Start from template" round-trip is mid-instantiate (same
            // navigation-lock rationale as the Back button).
            disabled={importing || starting !== null}
            data-testid="kcreate-template-import"
            aria-haspopup="menu"
            aria-expanded={importMenuOpen}
            aria-label="Import a design as a new template"
            style={{
              display: "inline-flex",
              alignItems: "center",
              gap: spacing.xs,
              background: "transparent",
              border: `1px solid ${colors.border}`,
              borderRadius: radius.md,
              padding: `${spacing.xs}px ${spacing.sm}px`,
              color: colors.text,
              cursor: importing || starting ? "default" : "pointer",
              opacity: importing || starting ? 0.7 : 1,
              fontSize: 13,
              whiteSpace: "nowrap",
            }}
          >
            <Icon name="file-plus" size={16} />
            {importing ? "Importing…" : "Remix from file"}
            <span aria-hidden style={{ fontSize: 10, marginLeft: 2 }}>
              ▾
            </span>
          </button>
          {importMenuOpen ? (
            <>
              {/* Full-viewport click-catcher so a click anywhere else
                  dismisses the menu (cheaper + more robust than a
                  document listener that has to be added/removed). */}
              <div
                data-testid="kcreate-template-import-overlay"
                onClick={() => setImportMenuOpen(false)}
                style={{
                  position: "fixed",
                  inset: 0,
                  zIndex: 10,
                }}
              />
              <div
                role="menu"
                data-testid="kcreate-template-import-menu"
                onKeyDown={(e) => {
                  if (e.key === "Escape") setImportMenuOpen(false);
                }}
                style={{
                  position: "absolute",
                  top: "calc(100% + 4px)",
                  right: 0,
                  zIndex: 11,
                  minWidth: 248,
                  display: "flex",
                  flexDirection: "column",
                  background: colors.bg,
                  border: `1px solid ${colors.border}`,
                  borderRadius: radius.md,
                  boxShadow: shadow.card,
                  overflow: "hidden",
                }}
              >
                <ImportMenuItem
                  testId="kcreate-template-import-file"
                  icon="file-text"
                  title="From a design file"
                  subtitle=".json template content"
                  onClick={() => void runImport("file")}
                />
                <ImportMenuItem
                  testId="kcreate-template-import-package"
                  icon="package"
                  title="From a project or package"
                  subtitle=".kstudio project · .ktemplate folder"
                  onClick={() => void runImport("directory")}
                />
              </div>
            </>
          ) : null}
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
          count={totalCount ?? undefined}
          testId="kcreate-template-cat-all"
          onClick={() => setCategory(null)}
        />
        {ALL_CATEGORIES.map((cat) => (
          <CategoryChip
            key={cat}
            label={CATEGORY_LABELS[cat]}
            tint={CATEGORY_TINT[cat]}
            active={category === cat}
            count={totalCount === null ? undefined : (counts[cat] ?? 0)}
            testId={`kcreate-template-cat-${cat}`}
            onClick={() => setCategory(cat)}
          />
        ))}
      </div>

      {importError ? (
        <div
          data-testid="kcreate-template-import-error"
          role="alert"
          style={{
            padding: `${spacing.sm}px ${spacing.xl}px`,
            background: colors.dangerBg,
            color: colors.danger,
            fontSize: 13,
            borderBottom: `1px solid ${colors.border}`,
            flexShrink: 0,
          }}
        >
          Couldn’t import that file: {importError}
        </div>
      ) : null}

      <div
        style={{
          display: "flex",
          flex: 1,
          minHeight: 0,
        }}
      >
        <main
          ref={scrollRef}
          onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}
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
            // Virtualised grid: a full-height spacer carries the
            // scrollbar for the whole set, while only the windowed slice
            // of cards is mounted, absolutely positioned at `topPad`.
            // The window collapses to "render everything" until the
            // viewport is measured, so jsdom (zero-size) shows all cards.
            <div
              data-testid="kcreate-template-grid"
              style={{ position: "relative", height: gridWindow.totalHeight }}
            >
              <div
                style={{
                  position: "absolute",
                  top: gridWindow.topPad,
                  left: 0,
                  right: 0,
                  display: "grid",
                  gridTemplateColumns: `repeat(${gridWindow.columns}, minmax(${CARD_MIN_WIDTH}px, 1fr))`,
                  gap: GRID_GAP,
                }}
              >
                {visibleTemplates.map((template) => (
                  <TemplateCard
                    key={template.id}
                    template={template}
                    thumb={thumbs[template.id]}
                    selected={template.id === selectedId}
                    onSelect={() => setSelectedId(template.id)}
                  />
                ))}
              </div>
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
  count,
  testId,
  onClick,
}: {
  label: string;
  tint: string;
  active: boolean;
  /** Catalog count shown as a trailing badge; omitted until loaded. */
  count?: number;
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
        display: "inline-flex",
        alignItems: "center",
        gap: spacing.xs,
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
      <span>{label}</span>
      {count === undefined ? null : (
        <span
          data-testid={`${testId}-count`}
          style={{
            fontSize: 11,
            fontWeight: 600,
            lineHeight: 1,
            padding: "2px 6px",
            borderRadius: radius.pill,
            background: active ? "rgba(255,255,255,0.25)" : colors.bgSoft,
            color: active ? "#FFFFFF" : colors.textMuted,
          }}
        >
          {count}
        </span>
      )}
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
        // Fixed height keeps every grid row uniform so the windowed
        // `topPad` / `totalHeight` math lines up exactly with the DOM.
        height: CARD_HEIGHT,
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
