// H1 — empty-canvas call-to-action component tests.
//
// Shown when a project has no artboards. Pins that each action button
// routes into its real handler and that the palette link opens the
// palette — the empty canvas must never be a dead end.

import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";

import { CanvasEmptyState } from "./CanvasEmptyState";
import type { DiscoveryAction } from "./DiscoveryWelcome";

function makeActions(spies: {
  templates: () => void;
  ai: () => void;
  elements: () => void;
}): DiscoveryAction[] {
  return [
    {
      id: "templates",
      label: "Start from a template",
      description: "Fork a ready-made design.",
      icon: "layout",
      run: spies.templates,
    },
    {
      id: "ai",
      label: "Generate with AI",
      description: "Let the model draft it.",
      icon: "wand",
      run: spies.ai,
    },
    {
      id: "elements",
      label: "Browse elements",
      description: "Drop in shapes and icons.",
      icon: "sparkles",
      run: spies.elements,
    },
  ];
}

describe("CanvasEmptyState", () => {
  it("renders a button per action plus the palette link", () => {
    const spies = { templates: vi.fn(), ai: vi.fn(), elements: vi.fn() };
    render(
      <CanvasEmptyState
        paletteHint="Ctrl K"
        onOpenPalette={() => {}}
        actions={makeActions(spies)}
      />,
    );
    expect(
      screen.getByTestId("kcreate-canvas-empty-state"),
    ).toBeInTheDocument();
    for (const id of ["templates", "ai", "elements"]) {
      expect(
        screen.getByTestId(`kcreate-canvas-empty-action-${id}`),
      ).toBeInTheDocument();
    }
    expect(
      screen.getByTestId("kcreate-canvas-empty-palette"),
    ).toBeInTheDocument();
    expect(screen.getByText("Ctrl K")).toBeInTheDocument();
  });

  it("runs the matching real handler when an action is clicked", () => {
    const spies = { templates: vi.fn(), ai: vi.fn(), elements: vi.fn() };
    render(
      <CanvasEmptyState
        paletteHint="Ctrl K"
        onOpenPalette={() => {}}
        actions={makeActions(spies)}
      />,
    );
    fireEvent.click(screen.getByTestId("kcreate-canvas-empty-action-templates"));
    expect(spies.templates).toHaveBeenCalledTimes(1);
    expect(spies.ai).not.toHaveBeenCalled();
    expect(spies.elements).not.toHaveBeenCalled();
  });

  it("opens the command palette from the palette link", () => {
    const onOpenPalette = vi.fn();
    render(
      <CanvasEmptyState
        paletteHint="Ctrl K"
        onOpenPalette={onOpenPalette}
        actions={makeActions({
          templates: () => {},
          ai: () => {},
          elements: () => {},
        })}
      />,
    );
    fireEvent.click(screen.getByTestId("kcreate-canvas-empty-palette"));
    expect(onOpenPalette).toHaveBeenCalledTimes(1);
  });
});
