// AssetsPanel — Elements / asset library (G6).
//
// Canva-style Elements panel: a searchable, offline library of
// bundled vector shapes, lines/arrows, icons, frames, and simple
// illustrations. Category tabs + a search box narrow a thumbnail
// grid; clicking (or dragging onto the canvas) inserts the asset as
// editable, recolorable vector node(s) via the bridge.
//
// The panel is self-contained over the `window.kcreate.assets`
// surface for *reading* the catalog (categories / list / search) but
// delegates the actual insert to the host (`EditorPage`) via
// `onInsert`, because positioning (viewport centre vs. drop point),
// selection, and the layer-tree refresh all live there. This mirrors
// `DesignTokenEditor` / `BrandKitEditor`, which take only an
// `onStatus` callback and own their own data fetching.

import { useEffect, useMemo, useRef, useState } from "react";

import type { AssetCategoryInfo, AssetSummary } from "../../../shared/scene";
import { colors, radius, spacing } from "../styles/tokens";
import { errorMessage } from "../lib/errorMessage";

/// Drag payload key carrying an asset id from a thumbnail to the
/// canvas drop zone (`EditorPage.handleCanvasDrop`). A custom
/// `application/x-…` type so it never collides with the "Files"
/// payload the canvas already handles for image / PDF import.
export const ASSET_DRAG_MIME = "application/x-kcreate-asset";

export interface AssetsPanelProps {
  /// Insert the asset onto the canvas at a host-chosen position
  /// (viewport centre for clicks). Wired by `EditorPage` to
  /// `window.kcreate.assets.insert` + selection + tree refresh.
  onInsert: (assetId: string) => void;
  /// Surface a status / error string to the host status bar.
  onStatus?: (msg: string | null) => void;
}

// All-categories pseudo-tab. Kept distinct from any real category
// slug (lowercase identifiers like "shapes") so the active-tab
// comparison never aliases a real category.
const ALL_TAB = "__all__";

// Search debounce. The catalog lives in-process (synchronous Rust
// over IPC) so this is cheap, but debouncing avoids a burst of
// list/search round-trips while the user is mid-keystroke.
const SEARCH_DEBOUNCE_MS = 100;

const noopStatus = (_: string | null): void => undefined;

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

  // Load the asset grid whenever the active category or query
  // changes. An empty query lists the (optionally category-scoped)
  // catalog; a non-empty query runs name/tag search. A per-effect
  // `cancelled` flag plus the debounce timer drops stale responses
  // so fast typing can never paint an out-of-order result set.
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

  return (
    <div
      style={{ display: "flex", flexDirection: "column", gap: spacing.sm }}
      data-testid="assets-panel"
    >
      <input
        type="search"
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        placeholder="Search elements — arrow, check, phone, chart…"
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
          aria-label="Elements"
          style={{
            display: "grid",
            gridTemplateColumns: "repeat(3, 1fr)",
            gap: spacing.xs,
          }}
        >
          {assets.map((asset) => (
            <AssetThumb key={asset.id} asset={asset} onInsert={onInsert} />
          ))}
        </div>
      )}
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
  onInsert,
}: {
  asset: AssetSummary;
  onInsert: (assetId: string) => void;
}): JSX.Element {
  const src = useMemo(() => svgDataUrl(asset.svg), [asset.svg]);
  return (
    <button
      type="button"
      title={asset.name}
      aria-label={`Insert ${asset.name}`}
      draggable
      onClick={() => onInsert(asset.id)}
      onDragStart={(e) => {
        // Carry the asset id so the canvas drop zone can insert at
        // the exact drop point. `effectAllowed = "copy"` matches the
        // copy semantics of an insert (the catalog entry is never
        // consumed).
        e.dataTransfer.setData(ASSET_DRAG_MIME, asset.id);
        e.dataTransfer.effectAllowed = "copy";
      }}
      style={{
        display: "flex",
        flexDirection: "column",
        alignItems: "center",
        gap: 2,
        padding: spacing.xs,
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
