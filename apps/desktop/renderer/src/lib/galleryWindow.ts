// Virtualised-grid math for the template gallery (H2).
//
// The expanded library ships 120+ templates; mounting a card (each with
// its own lazily-fetched thumbnail) for every one at once is wasteful
// and janky. These pure helpers turn the scroll container's measured
// width/height + scroll offset into the slice of cards that actually
// needs to be in the DOM, plus the top padding that keeps the scrollbar
// honest. Extracted from the component so the windowing is unit-testable
// without a layout engine (jsdom reports zero-sized elements, so the
// component itself can't exercise the windowed branch).

/** The slice of items to render plus the geometry around it. */
export interface GridWindow {
  /** First item index to render (inclusive). */
  startIndex: number;
  /** One past the last item index to render (exclusive). */
  endIndex: number;
  /** Pixels of empty space above the rendered block (scroll spacer). */
  topPad: number;
  /** Full virtual height of the grid, so the scrollbar spans the set. */
  totalHeight: number;
  /** Column count the window was computed for. */
  columns: number;
  /** Total row count across the whole set. */
  rows: number;
}

/**
 * Column count for an `auto-fill` grid of `minColWidth`-wide cards in a
 * `containerWidth`-wide track with `gap` between columns — the same
 * arithmetic CSS `repeat(auto-fill, minmax(minColWidth, 1fr))` uses, so
 * the windowed item→row mapping matches what the browser actually lays
 * out. Always at least one column; falls back to one when the width
 * hasn't been measured yet (jsdom / first paint).
 */
export function computeColumns(
  containerWidth: number,
  minColWidth: number,
  gap: number,
): number {
  if (containerWidth <= 0 || minColWidth <= 0) return 1;
  const cols = Math.floor((containerWidth + gap) / (minColWidth + gap));
  return Math.max(1, cols);
}

/** Inputs for {@link computeGridWindow}. */
export interface GridWindowParams {
  /** Total number of items in the (already filtered) set. */
  total: number;
  /** Column count (see {@link computeColumns}). */
  columns: number;
  /** Card height in px, excluding the inter-row gap. */
  rowHeight: number;
  /** Gap in px between rows (and columns). */
  gap: number;
  /** Current scroll offset of the container in px. */
  scrollTop: number;
  /** Visible height of the scroll container in px. */
  viewportHeight: number;
  /** Extra rows to render above/below the viewport (default 2). */
  overscanRows?: number;
}

/**
 * Compute which items to render for a vertically-scrolling grid.
 *
 * When the viewport hasn't been measured yet (`viewportHeight <= 0`, as
 * in jsdom or before the first `ResizeObserver` callback) the whole set
 * is returned so nothing is hidden — windowing only narrows the DOM
 * once a real viewport height is known. Otherwise only the rows
 * intersecting the viewport (plus `overscanRows` on each side) are
 * emitted, and `topPad` offsets them so they sit at the right scroll
 * position inside a `totalHeight`-tall spacer.
 */
export function computeGridWindow(p: GridWindowParams): GridWindow {
  const columns = Math.max(1, Math.floor(p.columns));
  const total = Math.max(0, Math.floor(p.total));
  const rows = Math.ceil(total / columns);
  const stride = p.rowHeight + p.gap;
  const totalHeight = rows === 0 ? 0 : rows * p.rowHeight + (rows - 1) * p.gap;

  if (p.viewportHeight <= 0 || total === 0) {
    return {
      startIndex: 0,
      endIndex: total,
      topPad: 0,
      totalHeight,
      columns,
      rows,
    };
  }

  const overscanRows = Math.max(0, Math.floor(p.overscanRows ?? 2));
  // Clamp the anchor row to the last real row so a `scrollTop` that
  // overshoots the content (transient over-scroll, a shrunk filter set
  // before the scroll resets) still mounts the tail rows instead of an
  // empty window past the end.
  const firstVisibleRow = Math.min(
    Math.max(0, rows - 1),
    Math.max(0, Math.floor(p.scrollTop / stride)),
  );
  const startRow = Math.max(0, firstVisibleRow - overscanRows);
  const rowsInView = Math.ceil(p.viewportHeight / stride) + 1;
  const endRow = Math.min(rows, firstVisibleRow + rowsInView + overscanRows);

  return {
    startIndex: startRow * columns,
    endIndex: Math.min(total, endRow * columns),
    topPad: startRow * stride,
    totalHeight,
    columns,
    rows,
  };
}
