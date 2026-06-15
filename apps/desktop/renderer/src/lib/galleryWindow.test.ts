// Unit tests for the template-gallery virtualisation math (H2).
//
// jsdom reports every element as zero-sized, so the component can never
// exercise its windowed branch under vitest — these tests pin the pure
// geometry directly: column packing, the "render everything until
// measured" fallback, and the windowed slice + spacer for a large set.

import { describe, it, expect } from "vitest";

import { computeColumns, computeGridWindow } from "./galleryWindow";

describe("computeColumns", () => {
  it("packs auto-fill columns the way CSS minmax() does", () => {
    // (920 + 16) / (200 + 16) = 4.33 -> 4 columns.
    expect(computeColumns(920, 200, 16)).toBe(4);
    // Exactly enough room for 5 columns: 5*200 + 4*16 = 1064.
    expect(computeColumns(1064, 200, 16)).toBe(5);
  });

  it("never returns less than one column", () => {
    expect(computeColumns(120, 200, 16)).toBe(1);
    expect(computeColumns(0, 200, 16)).toBe(1);
    expect(computeColumns(-50, 200, 16)).toBe(1);
  });
});

describe("computeGridWindow", () => {
  it("renders the whole set before the viewport is measured", () => {
    const w = computeGridWindow({
      total: 122,
      columns: 4,
      rowHeight: 210,
      gap: 16,
      scrollTop: 0,
      viewportHeight: 0,
    });
    expect(w.startIndex).toBe(0);
    expect(w.endIndex).toBe(122);
    expect(w.topPad).toBe(0);
  });

  it("windows a large set to the viewport plus overscan", () => {
    // 122 items / 4 cols = 31 rows; stride = 210 + 16 = 226.
    // Scrolled to row 10 (scrollTop 2260), 720px tall viewport.
    const w = computeGridWindow({
      total: 122,
      columns: 4,
      rowHeight: 210,
      gap: 16,
      scrollTop: 2260,
      viewportHeight: 720,
      overscanRows: 2,
    });
    // firstVisibleRow = floor(2260/226) = 10; startRow = 10 - 2 = 8.
    expect(w.startIndex).toBe(8 * 4);
    // rowsInView = ceil(720/226)+1 = 5; endRow = 10 + 5 + 2 = 17.
    expect(w.endIndex).toBe(17 * 4);
    expect(w.topPad).toBe(8 * 226);
    // Far fewer than the whole set is mounted.
    expect(w.endIndex - w.startIndex).toBeLessThan(122);
    // The spacer spans the full virtual height (31 rows).
    expect(w.totalHeight).toBe(31 * 210 + 30 * 16);
    expect(w.rows).toBe(31);
  });

  it("clamps the window to the end of the set", () => {
    const w = computeGridWindow({
      total: 10,
      columns: 4,
      rowHeight: 210,
      gap: 16,
      scrollTop: 100000,
      viewportHeight: 720,
    });
    expect(w.endIndex).toBe(10);
    expect(w.startIndex).toBeLessThanOrEqual(10);
  });

  it("handles an empty set", () => {
    const w = computeGridWindow({
      total: 0,
      columns: 4,
      rowHeight: 210,
      gap: 16,
      scrollTop: 0,
      viewportHeight: 720,
    });
    expect(w).toMatchObject({
      startIndex: 0,
      endIndex: 0,
      topPad: 0,
      totalHeight: 0,
    });
  });

  it("starts at the top when not scrolled", () => {
    const w = computeGridWindow({
      total: 122,
      columns: 4,
      rowHeight: 210,
      gap: 16,
      scrollTop: 0,
      viewportHeight: 720,
    });
    expect(w.startIndex).toBe(0);
    expect(w.topPad).toBe(0);
    expect(w.endIndex).toBeLessThan(122);
  });
});
