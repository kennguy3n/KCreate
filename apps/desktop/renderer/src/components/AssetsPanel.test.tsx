// AssetsPanel tests (G6 + H3 — Elements / asset library).
//
// AssetsPanel is self-contained over `window.kcreate.assets.*` for
// reading the catalog and delegates the insert to an `onInsert`
// callback. These tests drive the installed `kcreateStub` with a
// small fixture catalog and assert:
//   * categories + assets render on mount (list path), grouped into
//     headed sections with per-section counts;
//   * typing in the search box routes to `assets.search` and narrows
//     the grid to a single ranked "Results" section;
//   * selecting a category chip routes to `assets.list` with the
//     category slug and sections by finer sub-group;
//   * clicking a thumbnail fires `onInsert(assetId)`;
//   * dragging a thumbnail writes the asset id onto the dataTransfer
//     under `ASSET_DRAG_MIME` (the canvas drop contract) but does NOT
//     record — a cancelled drag must leave no phantom recently-used
//     entry; recording is the host's job on a successful insert;
//   * the "Recently used" row reflects the shared `recentElements`
//     store (persisted to `localStorage`), which the panel only reads;
//   * the grid is windowed — a large catalog only mounts the rows in
//     view, not all of them.
//
// Data loading is async + debounced (100ms), so we use the async
// `findBy*` queries which poll until the effect settles.

import { describe, it, expect, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";

import type { AssetCategoryInfo, AssetSummary } from "../../../shared/scene";
import { kcreateStub } from "../../tests/helpers/kcreateStub";
import { AssetsPanel, ASSET_DRAG_MIME } from "./AssetsPanel";
import { recordRecentElement } from "../lib/recentElements";

const RECENT_KEY = "kcreate.elements.recent.v1";

const CATS: AssetCategoryInfo[] = [
  { slug: "shapes", label: "Shapes", count: 2 },
  { slug: "icons", label: "Icons", count: 1 },
];

const SHAPES: AssetSummary[] = [
  {
    id: "circle",
    name: "Circle",
    category: "shapes",
    group: "Geometric",
    tags: ["round"],
    svg: "<svg/>",
  },
  {
    id: "square",
    name: "Square",
    category: "shapes",
    group: "Geometric",
    tags: [],
    svg: "<svg/>",
  },
];
const ICONS: AssetSummary[] = [
  {
    id: "chart-bar",
    name: "Bar chart",
    category: "icons",
    group: "Charts",
    tags: ["chart", "graph"],
    svg: "<svg/>",
  },
];
const ALL: AssetSummary[] = [...SHAPES, ...ICONS];

function noop(): void {
  /* intentionally empty */
}

// Build a large single-category catalog to exercise the windowing.
function manyIcons(n: number): AssetSummary[] {
  return Array.from({ length: n }, (_, i) => ({
    id: `icon-${i}`,
    name: `Icon ${i}`,
    category: "icons",
    group: "Bulk",
    tags: [],
    svg: "<svg/>",
  }));
}

describe("AssetsPanel", () => {
  beforeEach(() => {
    // Recently-used persists to localStorage, which is shared across
    // tests in a jsdom file — clear it so each test starts clean.
    window.localStorage.clear();
    const stub = kcreateStub();
    stub.override("assets.categories", () => CATS);
    stub.override("assets.list", (category?: unknown) =>
      category === "icons" ? ICONS : category === "shapes" ? SHAPES : ALL,
    );
    stub.override("assets.search", (query?: unknown) => {
      const q = String(query ?? "").toLowerCase();
      return ALL.filter(
        (a) =>
          a.name.toLowerCase().includes(q) ||
          a.tags.some((t) => t.toLowerCase().includes(q)),
      );
    });
  });

  it("renders category chips and the full catalog on mount", async () => {
    render(<AssetsPanel onInsert={noop} />);
    // All + the two fixture categories.
    expect(
      await screen.findByRole("tab", { name: /^All/ }),
    ).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: /^Shapes/ })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: /^Icons/ })).toBeInTheDocument();
    // Every asset renders as an insert button.
    expect(
      await screen.findByRole("button", { name: "Insert Circle" }),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Insert Square" })).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Insert Bar chart" }),
    ).toBeInTheDocument();
  });

  it("sections the 'All' grid by category", async () => {
    render(<AssetsPanel onInsert={noop} />);
    // Section headers carry the category label; counts come from the
    // assets actually filed under each (2 shapes, 1 icon).
    expect(await screen.findByText("Shapes")).toBeInTheDocument();
    expect(screen.getByText("Icons")).toBeInTheDocument();
  });

  it("routes a search query through assets.search and narrows the grid", async () => {
    const stub = kcreateStub();
    render(<AssetsPanel onInsert={noop} />);
    await screen.findByRole("button", { name: "Insert Circle" });

    fireEvent.change(screen.getByRole("searchbox", { name: "Search elements" }), {
      target: { value: "chart" },
    });

    // The "chart" tag only matches the Bar chart icon.
    expect(
      await screen.findByRole("button", { name: "Insert Bar chart" }),
    ).toBeInTheDocument();
    await waitFor(() => {
      expect(screen.queryByRole("button", { name: "Insert Circle" })).toBeNull();
    });
    const search = stub.calls.find(
      (c) => c.method === "assets.search" && c.args[0] === "chart",
    );
    expect(search).toBeDefined();
    // Scoped to "all" categories → null category argument.
    expect(search?.args[1]).toBeNull();
  });

  it("routes a category chip through assets.list with the slug", async () => {
    const stub = kcreateStub();
    render(<AssetsPanel onInsert={noop} />);
    await screen.findByRole("button", { name: "Insert Circle" });

    fireEvent.click(screen.getByRole("tab", { name: /^Icons/ }));

    expect(
      await screen.findByRole("button", { name: "Insert Bar chart" }),
    ).toBeInTheDocument();
    await waitFor(() => {
      expect(screen.queryByRole("button", { name: "Insert Circle" })).toBeNull();
    });
    // Single category → sections by finer sub-group.
    expect(screen.getByText("Charts")).toBeInTheDocument();
    const listCall = stub.calls.find(
      (c) => c.method === "assets.list" && c.args[0] === "icons",
    );
    expect(listCall).toBeDefined();
  });

  it("fires onInsert with the asset id when a thumbnail is clicked", async () => {
    const inserted: string[] = [];
    render(<AssetsPanel onInsert={(id) => inserted.push(id)} />);
    const circle = await screen.findByRole("button", { name: "Insert Circle" });
    fireEvent.click(circle);
    expect(inserted).toEqual(["circle"]);
  });

  it("writes the asset id onto the dataTransfer on drag start without recording", async () => {
    render(<AssetsPanel onInsert={noop} />);
    const circle = await screen.findByRole("button", { name: "Insert Circle" });
    const data: Record<string, string> = {};
    const dataTransfer = {
      setData: (type: string, value: string) => {
        data[type] = value;
      },
      effectAllowed: "none",
    };
    fireEvent.dragStart(circle, { dataTransfer });
    expect(data[ASSET_DRAG_MIME]).toBe("circle");
    expect(dataTransfer.effectAllowed).toBe("copy");
    // A drag-start is NOT an insert: a cancelled drag (released off the
    // canvas) must leave no recently-used entry. Recording is the
    // host's job, only once a drop actually inserts.
    expect(window.localStorage.getItem(RECENT_KEY)).toBeNull();
    expect(screen.queryByText("Recently used")).toBeNull();
  });

  it("does not record into Recently used when a thumbnail is merely clicked", async () => {
    // The panel delegates the insert to `onInsert`; the host records
    // the recently-used entry only after the insert succeeds. So a
    // click alone (panel-side) must not touch the store.
    render(<AssetsPanel onInsert={noop} />);
    const square = await screen.findByRole("button", { name: "Insert Square" });
    fireEvent.click(square);
    expect(window.localStorage.getItem(RECENT_KEY)).toBeNull();
    expect(screen.queryByText("Recently used")).toBeNull();
  });

  it("reflects the shared recentElements store in a Recently used row", async () => {
    // Simulate the host recording a successful insert.
    recordRecentElement("square");
    const { unmount } = render(<AssetsPanel onInsert={noop} />);

    // The recently-used section header appears…
    expect(await screen.findByText("Recently used")).toBeInTheDocument();
    // …backed by the persisted, most-recent-first id list.
    expect(JSON.parse(window.localStorage.getItem(RECENT_KEY) ?? "[]")).toEqual([
      "square",
    ]);
    // "Square" appears twice: once in Recently used, once in Shapes.
    await waitFor(() => {
      expect(screen.getAllByRole("button", { name: "Insert Square" }).length).toBe(2);
    });

    // It survives a remount (the panel re-reads the store on mount).
    unmount();
    render(<AssetsPanel onInsert={noop} />);
    expect(await screen.findByText("Recently used")).toBeInTheDocument();
  });

  it("windows a large catalog instead of mounting every thumbnail", async () => {
    const BULK = manyIcons(600);
    const stub = kcreateStub();
    stub.override("assets.categories", () => [
      { slug: "icons", label: "Icons", count: BULK.length },
    ]);
    stub.override("assets.list", () => BULK);

    render(<AssetsPanel onInsert={noop} />);
    // Some early thumbnails mount…
    expect(await screen.findByRole("button", { name: "Insert Icon 0" })).toBeInTheDocument();
    // …but the windowing means the far end of a 600-asset catalog is
    // NOT in the DOM (it would be without virtualization).
    expect(screen.queryByRole("button", { name: "Insert Icon 599" })).toBeNull();
    // The number of mounted thumbnails is bounded well below the total.
    const mounted = screen.getAllByRole("button", { name: /^Insert Icon/ });
    expect(mounted.length).toBeGreaterThan(0);
    expect(mounted.length).toBeLessThan(BULK.length);
  });
});
