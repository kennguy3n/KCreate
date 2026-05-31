// TopBar starter tests (Phase A4).
//
// Pins down the surface the editor relies on:
//   * mode tabs render and dispatch `onModeChange` with the mode id
//     when clicked;
//   * the design-mode tool palette renders all 5 expected tools and
//     dispatches `onToolChange` with the matching ToolId;
//   * undo/redo buttons honour the `canUndo`/`canRedo` props (i.e.
//     `disabled` reflects the prop).
//
// Wrapped in ThemeProvider because the topbar reads from useTheme to
// render the theme-toggle button.

import { describe, it, expect } from "vitest";
import { render, screen, fireEvent, within } from "@testing-library/react";

import { ThemeProvider } from "../styles/ThemeProvider";
import { EDITOR_MODES, TopBar, toolsForMode } from "./TopBar";
import type { ToolId } from "../pages/EditorPage";

interface Captured {
  modes: string[];
  tools: ToolId[];
  undos: number;
  redos: number;
  exports: number;
  backs: number;
}

function renderTopBar(initial: { canUndo?: boolean; canRedo?: boolean } = {}) {
  const captured: Captured = {
    modes: [],
    tools: [],
    undos: 0,
    redos: 0,
    exports: 0,
    backs: 0,
  };
  const utils = render(
    <ThemeProvider>
      <TopBar
        projectName="test-project"
        mode="design"
        onModeChange={(m) => captured.modes.push(m)}
        tool="select"
        onToolChange={(t) => captured.tools.push(t)}
        canUndo={initial.canUndo ?? true}
        canRedo={initial.canRedo ?? true}
        onUndo={() => {
          captured.undos += 1;
        }}
        onRedo={() => {
          captured.redos += 1;
        }}
        onExport={() => {
          captured.exports += 1;
        }}
        onBackHome={() => {
          captured.backs += 1;
        }}
      />
    </ThemeProvider>,
  );
  return { ...utils, captured };
}

describe("TopBar", () => {
  it("renders every mode tab from EDITOR_MODES inside the mode nav", () => {
    renderTopBar();
    const modeNav = screen.getByRole("navigation", { name: "Editor mode" });
    for (const { label } of EDITOR_MODES) {
      expect(
        within(modeNav).getByRole("button", { name: label }),
        `mode tab "${label}" should render inside the mode nav`,
      ).toBeInTheDocument();
    }
  });

  it("dispatches the picked mode id on tab click", () => {
    const { captured } = renderTopBar();
    const modeNav = screen.getByRole("navigation", { name: "Editor mode" });
    fireEvent.click(within(modeNav).getByRole("button", { name: "Export" }));
    expect(captured.modes).toEqual(["export"]);
  });

  it("renders the design-mode tool palette and dispatches tool ids", () => {
    const { captured } = renderTopBar();
    const toolbar = screen.getByRole("toolbar", { name: "Drawing tools" });
    const expectedTools = toolsForMode("design");
    expect(expectedTools.length).toBeGreaterThan(0);
    for (const t of expectedTools) {
      expect(
        within(toolbar).getByRole("button", { name: new RegExp(t, "i") }),
        `tool button "${t}" should be reachable inside the drawing-tools toolbar`,
      ).toBeInTheDocument();
    }
    fireEvent.click(within(toolbar).getByRole("button", { name: /rect/i }));
    expect(captured.tools).toEqual(["rect"]);
  });

  it("honours canUndo / canRedo on the history buttons", () => {
    renderTopBar({ canUndo: false, canRedo: true });
    expect(screen.getByRole("button", { name: "Undo" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Redo" })).not.toBeDisabled();
  });
});
