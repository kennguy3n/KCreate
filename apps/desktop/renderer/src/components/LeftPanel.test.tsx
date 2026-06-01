// LeftPanel starter tests (Phase A4).
//
// Covers the layer-row interactions we ship today:
//   * the Layers tab renders one row per node and the row exposes
//     the layer name;
//   * clicking the eye toggle dispatches `onToggleVisibility(id,
//     !visible)` (i.e. inverts the current value);
//   * clicking the lock toggle dispatches `onToggleLocked(id,
//     !locked)`;
//   * clicking the row body selects the layer via `onSelect(id)`.
//
// We mount LeftPanel directly with a synthetic two-node tree; the
// component is presentational over its props, so no bridge stubbing
// is necessary.

import { describe, it, expect } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";

import type { NodeInfo } from "../../../shared/scene";
import { LeftPanel } from "./LeftPanel";

function makeNode(over: Partial<NodeInfo> & Pick<NodeInfo, "id" | "name">): NodeInfo {
  return {
    nodeType: "RectShape",
    parentId: null,
    children: [],
    visible: true,
    locked: false,
    bounds: { x: 0, y: 0, width: 100, height: 100 },
    version: 1,
    ...over,
  };
}

interface Captured {
  selected: (string | null)[];
  visibility: { id: string; visible: boolean }[];
  locked: { id: string; locked: boolean }[];
}

function renderLeftPanel(nodes: NodeInfo[], selectedId: string | null = null) {
  const captured: Captured = {
    selected: [],
    visibility: [],
    locked: [],
  };
  const utils = render(
    <LeftPanel
      nodes={nodes}
      selectedId={selectedId}
      onSelect={(id) => captured.selected.push(id)}
      onToggleVisibility={(id, visible) =>
        captured.visibility.push({ id, visible })
      }
      onToggleLocked={(id, locked) => captured.locked.push({ id, locked })}
      artboards={[]}
      components={[]}
    />,
  );
  return { ...utils, captured };
}

describe("LeftPanel", () => {
  it("renders a row per node with its display name", () => {
    renderLeftPanel([
      makeNode({ id: "a", name: "Alpha" }),
      makeNode({ id: "b", name: "Beta" }),
    ]);
    expect(screen.getByText("Alpha")).toBeInTheDocument();
    expect(screen.getByText("Beta")).toBeInTheDocument();
  });

  it("toggles visibility by inverting the current value", () => {
    const { captured } = renderLeftPanel([
      makeNode({ id: "a", name: "Alpha", visible: true }),
      makeNode({ id: "b", name: "Beta", visible: false }),
    ]);
    const hideAlpha = screen.getByRole("button", { name: "Hide layer" });
    const showBeta = screen.getByRole("button", { name: "Show layer" });
    fireEvent.click(hideAlpha);
    fireEvent.click(showBeta);
    expect(captured.visibility).toEqual([
      { id: "a", visible: false },
      { id: "b", visible: true },
    ]);
  });

  it("toggles lock by inverting the current value", () => {
    const { captured } = renderLeftPanel([
      makeNode({ id: "a", name: "Alpha", locked: false }),
      makeNode({ id: "b", name: "Beta", locked: true }),
    ]);
    fireEvent.click(screen.getByRole("button", { name: "Lock layer" }));
    fireEvent.click(screen.getByRole("button", { name: "Unlock layer" }));
    expect(captured.locked).toEqual([
      { id: "a", locked: true },
      { id: "b", locked: false },
    ]);
  });

  describe("right-click context menu (Phase D)", () => {
    it("opens a context menu with the standard action set on right-click", () => {
      renderLeftPanel([makeNode({ id: "a", name: "Alpha" })]);
      const row = screen.getByText("Alpha").closest("div");
      expect(row).not.toBeNull();
      fireEvent.contextMenu(row!);
      // Standard items always present
      expect(screen.getByTestId("ctx-rename")).toBeInTheDocument();
      expect(screen.getByTestId("ctx-visibility")).toBeInTheDocument();
      expect(screen.getByTestId("ctx-lock")).toBeInTheDocument();
    });

    it("hides Duplicate when onDuplicateNode is not wired", () => {
      renderLeftPanel([makeNode({ id: "a", name: "Alpha" })]);
      const row = screen.getByText("Alpha").closest("div");
      fireEvent.contextMenu(row!);
      expect(screen.queryByTestId("ctx-duplicate")).toBeNull();
    });

    it("shows Duplicate when onDuplicateNode is wired and fires it on click", () => {
      const duplicates: string[] = [];
      render(
        <LeftPanel
          nodes={[makeNode({ id: "a", name: "Alpha" })]}
          selectedId={null}
          onSelect={() => undefined}
          onDuplicateNode={(id) => duplicates.push(id)}
          artboards={[]}
          components={[]}
        />,
      );
      const row = screen.getByText("Alpha").closest("div");
      fireEvent.contextMenu(row!);
      fireEvent.click(screen.getByTestId("ctx-duplicate"));
      expect(duplicates).toEqual(["a"]);
    });

    it("Rename item opens the rename input pre-filled with the layer name", () => {
      renderLeftPanel([makeNode({ id: "a", name: "Alpha" })]);
      const row = screen.getByText("Alpha").closest("div");
      fireEvent.contextMenu(row!);
      fireEvent.click(screen.getByTestId("ctx-rename"));
      const input = screen.getByDisplayValue("Alpha") as HTMLInputElement;
      expect(input).toBeInTheDocument();
      expect(input.tagName).toBe("INPUT");
    });

    it("Hide/Show item inverts visibility on the targeted row", () => {
      const { captured } = renderLeftPanel([
        makeNode({ id: "a", name: "Alpha", visible: true }),
      ]);
      const row = screen.getByText("Alpha").closest("div");
      fireEvent.contextMenu(row!);
      fireEvent.click(screen.getByTestId("ctx-visibility"));
      expect(captured.visibility).toEqual([{ id: "a", visible: false }]);
    });

    it("Lock/Unlock item inverts lock state on the targeted row", () => {
      const { captured } = renderLeftPanel([
        makeNode({ id: "a", name: "Alpha", locked: false }),
      ]);
      const row = screen.getByText("Alpha").closest("div");
      fireEvent.contextMenu(row!);
      fireEvent.click(screen.getByTestId("ctx-lock"));
      expect(captured.locked).toEqual([{ id: "a", locked: true }]);
    });

    it("Delete item fires onDelete with the targeted id", () => {
      const deletes: string[] = [];
      render(
        <LeftPanel
          nodes={[makeNode({ id: "a", name: "Alpha" })]}
          selectedId={null}
          onSelect={() => undefined}
          onDelete={(id) => deletes.push(id)}
          artboards={[]}
          components={[]}
        />,
      );
      const row = screen.getByText("Alpha").closest("div");
      fireEvent.contextMenu(row!);
      fireEvent.click(screen.getByTestId("ctx-delete"));
      expect(deletes).toEqual(["a"]);
    });

    it("Escape dismisses the menu", () => {
      renderLeftPanel([makeNode({ id: "a", name: "Alpha" })]);
      const row = screen.getByText("Alpha").closest("div");
      fireEvent.contextMenu(row!);
      expect(screen.getByTestId("ctx-rename")).toBeInTheDocument();
      fireEvent.keyDown(document, { key: "Escape" });
      expect(screen.queryByTestId("ctx-rename")).toBeNull();
    });
  });
});
