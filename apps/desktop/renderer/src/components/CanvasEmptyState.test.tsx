// H1 — empty-canvas call-to-action component tests.
//
// Shown when a project has no artboards. Pins that each action button
// routes into its real handler and that the palette link opens the
// palette — the empty canvas must never be a dead end.

import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";

import { CanvasEmptyState } from "./CanvasEmptyState";
import type { DiscoveryAction } from "./DiscoveryWelcome";
import { LocaleProvider } from "../i18n";

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

  it("renders the English copy and keeps the hint as a real <kbd>", () => {
    render(
      <CanvasEmptyState
        paletteHint="Ctrl K"
        onOpenPalette={() => {}}
        actions={makeActions({
          templates: () => {},
          ai: () => {},
          elements: () => {},
        })}
      />,
    );
    expect(screen.getByText("Start your first design")).toBeInTheDocument();
    expect(screen.getByText("Open command palette")).toBeInTheDocument();
    // The `{hint}` marker is split out and rendered as a styled key,
    // never leaked into the page as literal text.
    expect(screen.getByText("Ctrl K").tagName).toBe("KBD");
    expect(document.body.textContent ?? "").not.toContain("{hint}");
  });

  it("localizes its copy under a non-English provider", () => {
    render(
      <LocaleProvider initialLocale="es">
        <CanvasEmptyState
          paletteHint="Ctrl K"
          onOpenPalette={() => {}}
          actions={makeActions({
            templates: () => {},
            ai: () => {},
            elements: () => {},
          })}
        />
      </LocaleProvider>,
    );
    expect(screen.getByText("Empieza tu primer diseño")).toBeInTheDocument();
    expect(screen.getByText("Abrir la paleta de comandos")).toBeInTheDocument();
    // Even translated, the keystroke stays a real <kbd> element.
    expect(screen.getByText("Ctrl K").tagName).toBe("KBD");
  });
});
