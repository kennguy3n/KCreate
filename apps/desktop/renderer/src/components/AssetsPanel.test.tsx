// AssetsPanel tests (G6 — Elements / asset library).
//
// AssetsPanel is self-contained over `window.kcreate.assets.*` for
// reading the catalog and delegates the insert to an `onInsert`
// callback. These tests drive the installed `kcreateStub` with a
// small fixture catalog and assert:
//   * categories + assets render on mount (list path);
//   * typing in the search box routes to `assets.search` and narrows
//     the grid;
//   * selecting a category chip routes to `assets.list` with the
//     category slug;
//   * clicking a thumbnail fires `onInsert(assetId)`;
//   * dragging a thumbnail writes the asset id onto the dataTransfer
//     under `ASSET_DRAG_MIME` (the canvas drop contract).
//
// Data loading is async + debounced (100ms), so we use the async
// `findBy*` queries which poll until the effect settles.

import { describe, it, expect, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";

import type { AssetCategoryInfo, AssetSummary } from "../../../shared/scene";
import { kcreateStub } from "../../tests/helpers/kcreateStub";
import { AssetsPanel, ASSET_DRAG_MIME } from "./AssetsPanel";

const CATS: AssetCategoryInfo[] = [
  { slug: "shapes", label: "Shapes", count: 2 },
  { slug: "icons", label: "Icons", count: 1 },
];

const SHAPES: AssetSummary[] = [
  { id: "circle", name: "Circle", category: "shapes", tags: ["round"], svg: "<svg/>" },
  { id: "square", name: "Square", category: "shapes", tags: [], svg: "<svg/>" },
];
const ICONS: AssetSummary[] = [
  {
    id: "chart-bar",
    name: "Bar chart",
    category: "icons",
    tags: ["chart", "graph"],
    svg: "<svg/>",
  },
];
const ALL: AssetSummary[] = [...SHAPES, ...ICONS];

function noop(): void {
  /* intentionally empty */
}

describe("AssetsPanel", () => {
  beforeEach(() => {
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

  it("writes the asset id onto the dataTransfer on drag start", async () => {
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
  });
});
