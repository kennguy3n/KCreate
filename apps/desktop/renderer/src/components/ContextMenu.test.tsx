// ContextMenu primitive tests — Phase D.
//
// Covers the contract of the shared `ContextMenu` / `MenuItem`
// surface: renders items, dismisses on Escape, dismisses on
// outside mousedown, items fire their onClick, and disabled items
// suppress activation.
//
// Edge-clamping is checked via a smoke test (open near the viewport
// right edge and confirm the rendered `left` is < cursor.x), but the
// exact placement math is not pinned because jsdom returns a 0×0
// bounding rect by default and we don't want a brittle dependency
// on `Element.prototype.getBoundingClientRect` overrides.

import { describe, it, expect, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";

import {
  ContextMenu,
  MenuDivider,
  MenuItem,
  MenuSubheading,
} from "./ContextMenu";

describe("ContextMenu", () => {
  it("renders MenuItem labels and dispatches their onClick", () => {
    const renameClicks = vi.fn();
    const deleteClicks = vi.fn();
    render(
      <ContextMenu x={120} y={140} onDismiss={() => {}}>
        <MenuItem label="Rename" onClick={renameClicks} />
        <MenuDivider />
        <MenuItem label="Delete" danger onClick={deleteClicks} />
      </ContextMenu>,
    );
    fireEvent.click(screen.getByRole("menuitem", { name: "Rename" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Delete" }));
    expect(renameClicks).toHaveBeenCalledTimes(1);
    expect(deleteClicks).toHaveBeenCalledTimes(1);
  });

  it("dismisses on Escape", () => {
    const onDismiss = vi.fn();
    render(
      <ContextMenu x={50} y={50} onDismiss={onDismiss}>
        <MenuItem label="A" onClick={() => {}} />
      </ContextMenu>,
    );
    fireEvent.keyDown(document, { key: "Escape" });
    expect(onDismiss).toHaveBeenCalledTimes(1);
  });

  it("dismisses on mousedown outside the menu", () => {
    const onDismiss = vi.fn();
    render(
      <>
        <div data-testid="outside" style={{ width: 200, height: 200 }} />
        <ContextMenu x={50} y={50} onDismiss={onDismiss}>
          <MenuItem label="A" onClick={() => {}} />
        </ContextMenu>
      </>,
    );
    fireEvent.mouseDown(screen.getByTestId("outside"));
    expect(onDismiss).toHaveBeenCalledTimes(1);
  });

  it("does NOT dismiss when mousedown originates inside the menu", () => {
    const onDismiss = vi.fn();
    render(
      <ContextMenu x={50} y={50} onDismiss={onDismiss}>
        <MenuItem label="Inner" onClick={() => {}} />
      </ContextMenu>,
    );
    fireEvent.mouseDown(screen.getByRole("menuitem", { name: "Inner" }));
    expect(onDismiss).not.toHaveBeenCalled();
  });

  it("renders MenuSubheading and MenuDivider without crashing", () => {
    render(
      <ContextMenu x={10} y={10} onDismiss={() => {}}>
        <MenuSubheading label="Layer color" />
        <MenuItem label="Red" onClick={() => {}} />
        <MenuDivider />
        <MenuItem label="Clear" onClick={() => {}} />
      </ContextMenu>,
    );
    expect(screen.getByText("Layer color")).toBeInTheDocument();
    expect(screen.getByRole("separator")).toBeInTheDocument();
    expect(screen.getAllByRole("menuitem")).toHaveLength(2);
  });

  it("disabled MenuItem ignores clicks and is marked disabled", () => {
    const handler = vi.fn();
    render(
      <ContextMenu x={10} y={10} onDismiss={() => {}}>
        <MenuItem label="Distribute" onClick={handler} disabled />
      </ContextMenu>,
    );
    const btn = screen.getByRole("menuitem", { name: "Distribute" });
    expect(btn).toBeDisabled();
    fireEvent.click(btn);
    expect(handler).not.toHaveBeenCalled();
  });

  it("clamps inside the viewport when opened near the right edge", () => {
    // Force a known viewport width so the clamp math is deterministic.
    const originalInnerWidth = window.innerWidth;
    const originalInnerHeight = window.innerHeight;
    Object.defineProperty(window, "innerWidth", {
      value: 800,
      configurable: true,
    });
    Object.defineProperty(window, "innerHeight", {
      value: 600,
      configurable: true,
    });
    // jsdom's default `getBoundingClientRect` returns a 0×0 rect, so
    // simulate a 220×100 menu. The clamp logic must flip horizontally
    // because 790 + 220 > 800 - 8.
    const originalGBCR = Element.prototype.getBoundingClientRect;
    Element.prototype.getBoundingClientRect = function () {
      return {
        x: 0,
        y: 0,
        width: 220,
        height: 100,
        top: 0,
        left: 0,
        right: 220,
        bottom: 100,
        toJSON: () => ({}),
      } as DOMRect;
    };
    try {
      render(
        <ContextMenu x={790} y={50} onDismiss={() => {}}>
          <MenuItem label="A" onClick={() => {}} />
        </ContextMenu>,
      );
      const menu = screen.getByRole("menu");
      const left = parseFloat((menu as HTMLElement).style.left);
      // The menu's right edge was at 790 + 220 = 1010 → would overflow
      // (viewport 800 - EDGE_PAD 8 = 792). It must flip to the left
      // of the cursor: 790 - 220 = 570. The exact value is checked
      // because the clamp math is the load-bearing detail being
      // verified.
      expect(left).toBe(570);
    } finally {
      Element.prototype.getBoundingClientRect = originalGBCR;
      Object.defineProperty(window, "innerWidth", {
        value: originalInnerWidth,
        configurable: true,
      });
      Object.defineProperty(window, "innerHeight", {
        value: originalInnerHeight,
        configurable: true,
      });
    }
  });
});
