// AssetsPanel — Elements / asset library (G6 + H3).
//
// Canva-style Elements panel: a searchable, offline library of 400+
// bundled vector shapes, lines/arrows, icons, frames, and simple
// illustrations. Category tabs + a multi-keyword search box narrow a
// sectioned thumbnail grid; clicking (or dragging onto the canvas)
// inserts the asset as editable, recolorable vector node(s) via the
// bridge (recoloured toward the document's theme accent on insert).
//
// The panel is self-contained over the `window.kcreate.assets`
// surface for *reading* the catalog (categories / list / search) but
// delegates the actual insert to the host (`EditorPage`) via
// `onInsert`, because positioning (viewport centre vs. drop point),
// selection, and the layer-tree refresh all live there. This mirrors
// `DesignTokenEditor` / `BrandKitEditor`, which take only an
// `onStatus` callback and own their own data fetching.
//
// H3 browse UX for a large catalog:
//   * the grid is **sectioned** — by top-level category when browsing
//     "All", by finer sub-group when a single category is active, and
//     into a single ranked "Results" block while searching — each
//     section carrying a header with its asset count;
//   * the grid is **windowed** — only the rows intersecting the
//     viewport (plus a small overscan) are mounted, so 400+ thumbnails
//     stay smooth;
//   * a **"Recently used"** row leads the grid while browsing, sourced
//     from the shared `recentElements` store (persisted to
//     `localStorage`) so it survives reloads. The panel only *reads*
//     that store — recording happens in the host on a successful
//     insert, so a cancelled drag never leaves a phantom entry.

import {
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";

import type { AssetCategoryInfo, AssetSummary } from "../../../shared/scene";
import { colors, radius, spacing } from "../styles/tokens";
import { errorMessage } from "../lib/errorMessage";
import {
  getRecentElementIds,
  subscribeRecentElements,
} from "../lib/recentElements";

/// Drag payload key carrying an asset id from a thumbnail to the
/// canvas drop zone (`EditorPage.handleCanvasDrop`). A custom
/// `application/x-…` type so it never collides with the "Files"
/// payload the canvas already handles for image / PDF import.
export const ASSET_DRAG_MIME = "application/x-kcreate-asset";

// All-categories pseudo-tab. Kept distinct from any real category
// slug (lowercase identifiers like "shapes") so the active-tab
// comparison never aliases a real category.
const ALL_TAB = "__all__";

// Search debounce. The catalog lives in-process (synchronous Rust
// over IPC) so this is cheap, but debouncing avoids a burst of
// list/search round-trips while the user is mid-keystroke.
const SEARCH_DEBOUNCE_MS = 100;

// Grid geometry. The grid is virtualised over fixed-height rows: thumb
// rows pack `GRID_COLS` assets, section headers get their own row.
// Heights are the stable per-row strides the windowing math relies on
// (so they must match the rendered row box).
const GRID_COLS = 3;
const HEADER_ROW_H = 30;
const THUMB_ROW_H = 80;
// Extra pixels rendered above/below the viewport so a flick-scroll
// doesn't flash blank rows before the next paint.
const OVERSCAN_PX = 280;
// Viewport height assumed before the scroll container has been
// measured (and under jsdom, which doesn't lay out) — large enough
// that small catalogs render in full without a scroll event.
const DEFAULT_VIEWPORT_H = 560;

const noopStatus = (_: string | null): void => undefined;

/// One rendered section: a header plus the assets filed under it.
interface AssetSection {
  /// Stable key, unique across sections in a single render.
  key: string;
  /// Header label (category label, sub-group name, or "Results").
  label: string;
  /// Asset count shown beside the label.
  count: number;
  assets: AssetSummary[];
}

/// A single virtualised row: either a section header or a packed row
/// of up to {@link GRID_COLS} thumbnails.
type GridRow =
  | { kind: "header"; key: string; label: string; count: number }
  | { kind: "thumbs"; key: string; sectionKey: string; assets: AssetSummary[] };

export interface AssetsPanelProps {
  /// Insert the asset onto the canvas at a host-chosen position
  /// (viewport centre for clicks). Wired by `EditorPage` to
  /// `window.kcreate.assets.insert` + selection + tree refresh.
  onInsert: (assetId: string) => void;
  /// Surface a status / error string to the host status bar.
  onStatus?: (msg: string | null) => void;
}

/// Render a bundled SVG string as an `<img>` data URL. Using `<img>`
/// (rather than `dangerouslySetInnerHTML`) means the markup is
/// inert — it cannot run script or pull network sub-resources — so
/// even though the catalog ships in-repo and is trusted, the
/// thumbnail path stays XSS-proof by construction.
function svgDataUrl(svg: string): string {
  return `data:image/svg+xml;utf8,${encodeURIComponent(svg)}`;
}

export function AssetsPanel({
  onInsert,
  onStatus = noopStatus,
}: AssetsPanelProps): JSX.Element {
  const [categories, setCategories] = useState<AssetCategoryInfo[]>([]);
  const [active, setActive] = useState<string>(ALL_TAB);
  const [query, setQuery] = useState("");
  const [assets, setAssets] = useState<AssetSummary[]>([]);
  const [loading, setLoading] = useState(true);
  // Full id→asset map, fetched once, so the recently-used row can
  // resolve ids that belong to a category the user hasn't opened yet.
  const [catalogById, setCatalogById] = useState<Map<string, AssetSummary>>(
    () => new Map(),
  );
  // Recently-used ids come from the shared store (written by the host
  // on a successful insert). Subscribing here keeps the row live as
  // the user inserts, including drops handled outside this component.
  const recentIds = useSyncExternalStore(
    subscribeRecentElements,
    getRecentElementIds,
  );

  // Stable ref to the status callback so the data-loading effects
  // don't re-fire when the host passes a fresh closure each render.
  const onStatusRef = useRef(onStatus);
  useEffect(() => {
    onStatusRef.current = onStatus;
  }, [onStatus]);

  // Load the category list once on mount. The catalog is static, so
  // there is nothing to refresh — the tabs never change at runtime.
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const cats = await window.kcreate.assets.categories();
        if (!cancelled) setCategories(cats);
      } catch (e) {
        if (!cancelled) {
          onStatusRef.current(`elements: load categories failed: ${errorMessage(e)}`);
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  // Load the whole catalog once for recently-used resolution. The
  // scoped grid below is loaded separately (and may be narrowed to a
  // category or query); this map is the unfiltered source of truth.
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const all = await window.kcreate.assets.list(null);
        if (!cancelled) setCatalogById(new Map(all.map((a) => [a.id, a])));
      } catch (e) {
        if (!cancelled) {
          onStatusRef.current(`elements: load catalog failed: ${errorMessage(e)}`);
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  // Load the asset grid whenever the active category or query
  // changes. An empty query lists the (optionally category-scoped)
  // catalog; a non-empty query runs ranked name/tag search. A
  // per-effect `cancelled` flag plus the debounce timer drops stale
  // responses so fast typing can never paint an out-of-order result
  // set.
  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    const category = active === ALL_TAB ? null : active;
    const trimmed = query.trim();
    const timer = setTimeout(() => {
      void (async () => {
        try {
          const next =
            trimmed.length === 0
              ? await window.kcreate.assets.list(category)
              : await window.kcreate.assets.search(trimmed, category);
          if (!cancelled) {
            setAssets(next);
            setLoading(false);
          }
        } catch (e) {
          if (!cancelled) {
            setAssets([]);
            setLoading(false);
            onStatusRef.current(`elements: load failed: ${errorMessage(e)}`);
          }
        }
      })();
    }, SEARCH_DEBOUNCE_MS);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [active, query]);

  const totalCount = useMemo(
    () => categories.reduce((sum, c) => sum + c.count, 0),
    [categories],
  );

  const recentAssets = useMemo(
    () =>
      recentIds
        .map((id) => catalogById.get(id))
        .filter((a): a is AssetSummary => a !== undefined),
    [recentIds, catalogById],
  );

  // Group the loaded assets into headed sections. Searching keeps the
  // ranked order in one "Results" block; browsing "All" sections by
  // top-level category; a single category sections by finer sub-group.
  const sections = useMemo<AssetSection[]>(() => {
    const out: AssetSection[] = [];
    const trimmed = query.trim();

    if (trimmed.length === 0 && recentAssets.length > 0) {
      out.push({
        key: "recent",
        label: "Recently used",
        count: recentAssets.length,
        assets: recentAssets,
      });
    }

    if (trimmed.length > 0) {
      out.push({
        key: "results",
        label: "Results",
        count: assets.length,
        assets,
      });
      return out;
    }

    if (active === ALL_TAB) {
      const byCategory = new Map<string, AssetSummary[]>();
      for (const a of assets) {
        const bucket = byCategory.get(a.category);
        if (bucket) bucket.push(a);
        else byCategory.set(a.category, [a]);
      }
      for (const c of categories) {
        const bucket = byCategory.get(c.slug);
        if (bucket && bucket.length > 0) {
          out.push({
            key: `cat:${c.slug}`,
            label: c.label,
            count: bucket.length,
            assets: bucket,
          });
        }
      }
      return out;
    }

    // Single category → section by sub-group in first-seen order.
    const order: string[] = [];
    const byGroup = new Map<string, AssetSummary[]>();
    for (const a of assets) {
      const g = a.group.length > 0 ? a.group : "Other";
      const bucket = byGroup.get(g);
      if (bucket) {
        bucket.push(a);
      } else {
        order.push(g);
        byGroup.set(g, [a]);
      }
    }
    for (const g of order) {
      const bucket = byGroup.get(g);
      if (bucket) {
        out.push({ key: `grp:${g}`, label: g, count: bucket.length, assets: bucket });
      }
    }
    return out;
  }, [assets, categories, active, query, recentAssets]);

  // Flatten sections into the fixed-height row model the windowing
  // math consumes: one header row, then ⌈n / GRID_COLS⌉ thumb rows.
  const rows = useMemo<GridRow[]>(() => {
    const out: GridRow[] = [];
    for (const s of sections) {
      out.push({ kind: "header", key: `h:${s.key}`, label: s.label, count: s.count });
      for (let i = 0; i < s.assets.length; i += GRID_COLS) {
        out.push({
          kind: "thumbs",
          key: `t:${s.key}:${i}`,
          sectionKey: s.key,
          assets: s.assets.slice(i, i + GRID_COLS),
        });
      }
    }
    return out;
  }, [sections]);

  // Prefix-sum of row heights → each row's top offset + the total
  // scroll height. Recomputed only when the row set changes.
  const { offsets, totalHeight } = useMemo(() => {
    const offs = new Array<number>(rows.length + 1);
    let acc = 0;
    offs[0] = 0;
    for (let i = 0; i < rows.length; i += 1) {
      const row = rows[i];
      acc += row !== undefined && row.kind === "header" ? HEADER_ROW_H : THUMB_ROW_H;
      offs[i + 1] = acc;
    }
    return { offsets: offs, totalHeight: acc };
  }, [rows]);

  const scrollRef = useRef<HTMLDivElement | null>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewportH, setViewportH] = useState(0);

  // Measure the scroll viewport (re-measured when the grid (re)mounts
  // and on window resize). jsdom reports 0 here; the windowing falls
  // back to DEFAULT_VIEWPORT_H so component tests still mount rows.
  useLayoutEffect(() => {
    const measure = (): void => setViewportH(scrollRef.current?.clientHeight ?? 0);
    measure();
    window.addEventListener("resize", measure);
    return () => window.removeEventListener("resize", measure);
  }, [loading]);

  // Reset to the top of the grid when the result set changes, so a new
  // category / query never lands mid-scroll on a stale offset.
  useEffect(() => {
    setScrollTop(0);
    if (scrollRef.current) scrollRef.current.scrollTop = 0;
  }, [active, query]);

  // Window: the contiguous run of rows intersecting the viewport plus
  // overscan. Offsets are monotonic so a linear scan is both correct
  // and trivially cheap at this row count.
  const viewport = viewportH || DEFAULT_VIEWPORT_H;
  const upper = scrollTop - OVERSCAN_PX;
  const lower = scrollTop + viewport + OVERSCAN_PX;
  let start = 0;
  while (start < rows.length) {
    const next = offsets[start + 1];
    if (next === undefined || next > upper) break;
    start += 1;
  }
  let end = start;
  while (end < rows.length) {
    const o = offsets[end];
    if (o === undefined || o >= lower) break;
    end += 1;
  }

  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        gap: spacing.sm,
        height: "100%",
        minHeight: 0,
      }}
      data-testid="assets-panel"
    >
      <input
        type="search"
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        placeholder="Search elements — arrow, check, phone, bar chart…"
        aria-label="Search elements"
        style={{
          width: "100%",
          boxSizing: "border-box",
          padding: "6px 8px",
          fontSize: 12,
          background: colors.bgSoft,
          color: colors.text,
          border: `1px solid ${colors.border}`,
          borderRadius: radius.sm,
        }}
      />

      <div
        role="tablist"
        aria-label="Element categories"
        style={{
          display: "flex",
          flexWrap: "wrap",
          gap: spacing.xs,
        }}
      >
        <CategoryChip
          label="All"
          count={totalCount}
          active={active === ALL_TAB}
          onClick={() => setActive(ALL_TAB)}
        />
        {categories.map((c) => (
          <CategoryChip
            key={c.slug}
            label={c.label}
            count={c.count}
            active={active === c.slug}
            onClick={() => setActive(c.slug)}
          />
        ))}
      </div>

      {loading ? (
        <p style={{ fontSize: 12, color: colors.textMuted, padding: spacing.xs }}>
          Loading elements…
        </p>
      ) : assets.length === 0 ? (
        <p style={{ fontSize: 12, color: colors.textMuted, padding: spacing.xs }}>
          {query.trim().length > 0
            ? `No elements match “${query.trim()}”.`
            : "No elements in this category."}
        </p>
      ) : (
        <div
          ref={scrollRef}
          onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}
          aria-label="Elements"
          style={{
            position: "relative",
            flex: 1,
            minHeight: 0,
            maxHeight: "72vh",
            overflowY: "auto",
          }}
        >
          <div style={{ position: "relative", height: totalHeight }}>
            {rows.slice(start, end).map((row, i) => {
              const idx = start + i;
              const top = offsets[idx] ?? 0;
              if (row.kind === "header") {
                return (
                  <SectionHeader
                    key={row.key}
                    label={row.label}
                    count={row.count}
                    top={top}
                  />
                );
              }
              return (
                <div
                  key={row.key}
                  style={{
                    position: "absolute",
                    top,
                    left: 0,
                    right: 0,
                    height: THUMB_ROW_H,
                    display: "grid",
                    gridTemplateColumns: `repeat(${GRID_COLS}, 1fr)`,
                    gap: spacing.xs,
                  }}
                >
                  {row.assets.map((asset) => (
                    <AssetThumb
                      key={`${row.sectionKey}:${asset.id}`}
                      asset={asset}
                      onActivate={onInsert}
                    />
                  ))}
                </div>
              );
            })}
          </div>
        </div>
      )}
    </div>
  );
}

function SectionHeader({
  label,
  count,
  top,
}: {
  label: string;
  count: number;
  top: number;
}): JSX.Element {
  return (
    <div
      style={{
        position: "absolute",
        top,
        left: 0,
        right: 0,
        height: HEADER_ROW_H,
        display: "flex",
        alignItems: "center",
        gap: spacing.xs,
        fontSize: 11,
        fontWeight: 600,
        textTransform: "uppercase",
        letterSpacing: 0.4,
        color: colors.textMuted,
      }}
    >
      <span>{label}</span>
      <span style={{ opacity: 0.6, fontWeight: 500 }}>{count}</span>
    </div>
  );
}

function CategoryChip({
  label,
  count,
  active,
  onClick,
}: {
  label: string;
  count: number;
  active: boolean;
  onClick: () => void;
}): JSX.Element {
  return (
    <button
      type="button"
      role="tab"
      aria-selected={active}
      onClick={onClick}
      style={{
        padding: "3px 8px",
        fontSize: 11,
        fontWeight: 500,
        cursor: "pointer",
        borderRadius: radius.pill,
        border: `1px solid ${active ? colors.accent : colors.border}`,
        background: active ? colors.accentBgSoft : "transparent",
        color: active ? colors.accent : colors.textMuted,
      }}
    >
      {label}
      <span style={{ marginLeft: 4, opacity: 0.7 }}>{count}</span>
    </button>
  );
}

function AssetThumb({
  asset,
  onActivate,
}: {
  asset: AssetSummary;
  onActivate: (assetId: string) => void;
}): JSX.Element {
  const src = useMemo(() => svgDataUrl(asset.svg), [asset.svg]);
  return (
    <button
      type="button"
      title={asset.name}
      aria-label={`Insert ${asset.name}`}
      draggable
      onClick={() => onActivate(asset.id)}
      onDragStart={(e) => {
        // Carry the asset id so the canvas drop zone can insert at
        // the exact drop point. `effectAllowed = "copy"` matches the
        // copy semantics of an insert (the catalog entry is never
        // consumed). Recording into "Recently used" happens in the
        // host once the drop actually inserts — a cancelled drag
        // (released off-canvas) must not leave a phantom entry.
        e.dataTransfer.setData(ASSET_DRAG_MIME, asset.id);
        e.dataTransfer.effectAllowed = "copy";
      }}
      style={{
        display: "flex",
        flexDirection: "column",
        alignItems: "center",
        gap: 2,
        padding: spacing.xs,
        height: THUMB_ROW_H - spacing.xs,
        boxSizing: "border-box",
        cursor: "grab",
        background: colors.bgSoft,
        border: `1px solid ${colors.border}`,
        borderRadius: radius.md,
      }}
    >
      <img
        src={src}
        alt=""
        draggable={false}
        width={40}
        height={40}
        style={{ width: 40, height: 40, objectFit: "contain", pointerEvents: "none" }}
      />
      <span
        style={{
          fontSize: 10,
          color: colors.textMuted,
          maxWidth: "100%",
          overflow: "hidden",
          textOverflow: "ellipsis",
          whiteSpace: "nowrap",
        }}
      >
        {asset.name}
      </span>
    </button>
  );
}
