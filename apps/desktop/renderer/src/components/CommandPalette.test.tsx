// H1 — command palette component + ranking tests.
//
// Covers the behaviours the workstream promises: it opens (renders)
// only when `open`, a fuzzy query narrows the list, ↑/↓ moves the
// active row, Enter runs the active command's REAL handler, Esc and
// backdrop close, and every headline capability is reachable by typing
// its name. `buildRows` is exercised directly for the pure ranking
// (recent group, fuzzy ordering) so the maths is pinned without DOM.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";

import {
  CommandPalette,
  buildRows,
  type PaletteCommand,
} from "./CommandPalette";
import {
  COMMAND_HISTORY_STORAGE_KEY,
  loadCommandHistory,
  recordCommandUse,
} from "../lib/commandPaletteHistory";

function cmd(
  id: string,
  label: string,
  group: string,
  run: () => void = () => {},
  extra: Partial<PaletteCommand> = {},
): PaletteCommand {
  return { id, label, group, run, ...extra };
}

/** The five headline G-wave capabilities the palette must surface. */
function capabilityCommands(spies: Record<string, () => void>): PaletteCommand[] {
  return [
    cmd("openTemplates", "Start from a template", "Create", spies.templates, {
      keywords: ["template", "gallery"],
    }),
    cmd("openAiGenerate", "Generate with AI", "Create", spies.ai, {
      keywords: ["ai", "brief"],
    }),
    cmd("openTheme", "Open Theme & Brand kit", "Panels", spies.theme, {
      keywords: ["brand", "restyle"],
    }),
    cmd("openElements", "Browse elements", "Panels", spies.elements, {
      keywords: ["assets", "shapes"],
    }),
    cmd("openMagicResize", "Magic resize", "Panels", spies.resize, {
      keywords: ["resize", "adapt"],
    }),
  ];
}

function input(): HTMLInputElement {
  return screen.getByTestId(
    "kcreate-command-palette-input",
  ) as HTMLInputElement;
}

describe("CommandPalette", () => {
  beforeEach(() => {
    window.localStorage.removeItem(COMMAND_HISTORY_STORAGE_KEY);
  });

  it("renders nothing when closed", () => {
    render(
      <CommandPalette open={false} commands={[]} onClose={() => {}} />,
    );
    expect(screen.queryByTestId("kcreate-command-palette")).toBeNull();
  });

  it("renders the overlay + search input when open", () => {
    render(
      <CommandPalette
        open
        commands={[cmd("a", "Alpha", "Group")]}
        onClose={() => {}}
      />,
    );
    expect(screen.getByTestId("kcreate-command-palette")).toBeInTheDocument();
    expect(input()).toBeInTheDocument();
  });

  it("narrows the list as the user types a fuzzy query", () => {
    render(
      <CommandPalette
        open
        commands={[
          cmd("openTemplates", "Start from a template", "Create"),
          cmd("toolEllipse", "Ellipse tool", "Tools"),
        ]}
        onClose={() => {}}
      />,
    );
    // Both visible with an empty query.
    expect(
      screen.getByTestId("kcreate-command-row-openTemplates"),
    ).toBeInTheDocument();
    expect(
      screen.getByTestId("kcreate-command-row-toolEllipse"),
    ).toBeInTheDocument();

    fireEvent.change(input(), { target: { value: "templ" } });
    expect(
      screen.getByTestId("kcreate-command-row-openTemplates"),
    ).toBeInTheDocument();
    expect(
      screen.queryByTestId("kcreate-command-row-toolEllipse"),
    ).toBeNull();
  });

  it("shows an empty message when nothing matches", () => {
    render(
      <CommandPalette
        open
        commands={[cmd("a", "Alpha", "Group")]}
        onClose={() => {}}
      />,
    );
    fireEvent.change(input(), { target: { value: "zzzzz" } });
    expect(screen.getByText(/no matching commands/i)).toBeInTheDocument();
  });

  it("runs the active command's real handler on Enter and then closes", () => {
    const run = vi.fn();
    const onClose = vi.fn();
    render(
      <CommandPalette
        open
        commands={[cmd("openTemplates", "Start from a template", "Create", run)]}
        onClose={onClose}
      />,
    );
    fireEvent.change(input(), { target: { value: "template" } });
    fireEvent.keyDown(input(), { key: "Enter" });
    expect(run).toHaveBeenCalledTimes(1);
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("runs a command on click", () => {
    const run = vi.fn();
    render(
      <CommandPalette
        open
        commands={[cmd("a", "Alpha", "Group", run)]}
        onClose={() => {}}
      />,
    );
    fireEvent.click(screen.getByText("Alpha"));
    expect(run).toHaveBeenCalledTimes(1);
  });

  it("moves the active row with ArrowDown and runs the newly-active command", () => {
    const first = vi.fn();
    const second = vi.fn();
    render(
      <CommandPalette
        open
        commands={[
          cmd("first", "Quartz", "Group", first),
          cmd("second", "Quasar", "Group", second),
        ]}
        onClose={() => {}}
      />,
    );
    // "qua" matches both; "Quartz" sorts first (alpha tiebreak).
    fireEvent.change(input(), { target: { value: "qua" } });
    fireEvent.keyDown(input(), { key: "ArrowDown" });
    fireEvent.keyDown(input(), { key: "Enter" });
    expect(second).toHaveBeenCalledTimes(1);
    expect(first).not.toHaveBeenCalled();
  });

  it("closes on Escape and on backdrop click", () => {
    const onClose = vi.fn();
    const { rerender } = render(
      <CommandPalette open commands={[]} onClose={onClose} />,
    );
    fireEvent.keyDown(input(), { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);

    rerender(<CommandPalette open commands={[]} onClose={onClose} />);
    fireEvent.click(screen.getByTestId("kcreate-command-palette"));
    expect(onClose).toHaveBeenCalledTimes(2);
  });

  it("does not run a disabled command", () => {
    const run = vi.fn();
    render(
      <CommandPalette
        open
        commands={[
          cmd("x", "Disabled thing", "Group", run, {
            disabled: true,
            disabledReason: "Needs a selection",
          }),
        ]}
        onClose={() => {}}
      />,
    );
    fireEvent.click(screen.getByText("Disabled thing"));
    expect(run).not.toHaveBeenCalled();
  });

  it("reaches every headline capability by typing its name", () => {
    const spies = {
      templates: vi.fn(),
      ai: vi.fn(),
      theme: vi.fn(),
      elements: vi.fn(),
      resize: vi.fn(),
    };
    const commands = capabilityCommands(spies);
    const probes: ReadonlyArray<{ query: string; spy: () => void }> = [
      { query: "template", spy: spies.templates },
      { query: "generate", spy: spies.ai },
      { query: "theme", spy: spies.theme },
      { query: "elements", spy: spies.elements },
      { query: "magic", spy: spies.resize },
    ];
    for (const probe of probes) {
      const onClose = vi.fn();
      const { unmount } = render(
        <CommandPalette open commands={commands} onClose={onClose} />,
      );
      fireEvent.change(input(), { target: { value: probe.query } });
      fireEvent.keyDown(input(), { key: "Enter" });
      expect(
        probe.spy,
        `typing "${probe.query}" then Enter should run its capability`,
      ).toHaveBeenCalledTimes(1);
      unmount();
    }
  });

  it("reaches a capability by keyword synonym (e.g. 'brand' → Theme)", () => {
    const spies = {
      templates: vi.fn(),
      ai: vi.fn(),
      theme: vi.fn(),
      elements: vi.fn(),
      resize: vi.fn(),
    };
    render(
      <CommandPalette
        open
        commands={capabilityCommands(spies)}
        onClose={() => {}}
      />,
    );
    fireEvent.change(input(), { target: { value: "brand" } });
    fireEvent.keyDown(input(), { key: "Enter" });
    expect(spies.theme).toHaveBeenCalledTimes(1);
  });
});

describe("buildRows", () => {
  beforeEach(() => {
    window.localStorage.removeItem(COMMAND_HISTORY_STORAGE_KEY);
  });

  const commands: PaletteCommand[] = [
    cmd("openTemplates", "Start from a template", "Create"),
    cmd("openAiGenerate", "Generate with AI", "Create"),
    cmd("toolRect", "Rectangle tool", "Tools"),
  ];

  it("groups by `group` in first-seen order for an empty query", () => {
    const rows = buildRows(commands, "", loadCommandHistory());
    const headers = rows
      .filter((r) => r.kind === "header")
      .map((r) => (r.kind === "header" ? r.label : ""));
    expect(headers).toEqual(["Create", "Tools"]);
  });

  it("surfaces a Recent group from persisted history, newest first", () => {
    recordCommandUse("toolRect", 2_000);
    recordCommandUse("openTemplates", 1_000);
    const rows = buildRows(commands, "", loadCommandHistory());
    expect(rows[0]).toEqual({ kind: "header", label: "Recent" });
    // First two command rows after the Recent header are the two we ran,
    // newest (toolRect) first.
    const commandRows = rows.filter((r) => r.kind === "command");
    expect(commandRows[0]!.kind === "command" && commandRows[0]!.command.id).toBe(
      "toolRect",
    );
  });

  it("returns a flat fuzzy-ranked list for a non-empty query (no headers)", () => {
    const rows = buildRows(commands, "tool", loadCommandHistory());
    expect(rows.every((r) => r.kind === "command")).toBe(true);
    // "tool" matches "Rectangle tool" but not the two Create commands.
    expect(rows).toHaveLength(1);
    expect(rows[0]!.kind === "command" && rows[0]!.command.id).toBe("toolRect");
  });
});
