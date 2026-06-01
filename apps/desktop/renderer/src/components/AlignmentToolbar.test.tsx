// Phase D — AlignmentToolbar regression coverage.
//
// The toolbar was already implemented (Phase 9 Block D) but until
// this PR was not mounted anywhere. These tests pin the dispatch
// contract that EditorPage/RightPanel now relies on:
//   * `documentAlign` is called with (selectedIds, alignment) when
//     an align button is clicked.
//   * `documentDistribute` is called with (selectedIds, axis) when
//     a distribute button is clicked.
//   * The 2+ / 3+ disabled gating matches the keyboard-shortcut
//     handler's silent-no-op cardinality guard.
//   * Errors thrown by the bridge surface as an inline alert
//     instead of crashing the panel.

import { describe, it, expect, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";

import { AlignmentToolbar } from "./AlignmentToolbar";

type Phase9Bridge = {
  documentAlign: (ids: string[], alignment: string) => Promise<void>;
  documentDistribute: (ids: string[], axis: string) => Promise<void>;
};

let originalKcreate: typeof window.kcreate | undefined;

interface Captured {
  align: { ids: string[]; alignment: string }[];
  distribute: { ids: string[]; axis: string }[];
}

function installStub(opts?: { rejectAlign?: Error }): Captured {
  const captured: Captured = { align: [], distribute: [] };
  const phase9: Phase9Bridge = {
    documentAlign: async (ids, alignment) => {
      captured.align.push({ ids: [...ids], alignment });
      if (opts?.rejectAlign) throw opts.rejectAlign;
    },
    documentDistribute: async (ids, axis) => {
      captured.distribute.push({ ids: [...ids], axis });
    },
  };
  (window as unknown as { kcreate: { phase9: Phase9Bridge } }).kcreate = {
    phase9,
  };
  return captured;
}

describe("AlignmentToolbar", () => {
  beforeEach(() => {
    originalKcreate = (window as unknown as { kcreate?: typeof window.kcreate })
      .kcreate;
  });

  afterEach(() => {
    if (originalKcreate === undefined) {
      delete (window as unknown as { kcreate?: typeof window.kcreate }).kcreate;
    } else {
      (window as unknown as { kcreate: typeof window.kcreate }).kcreate =
        originalKcreate;
    }
  });

  it("disables every align/distribute button when fewer than 2 nodes are selected", () => {
    installStub();
    render(<AlignmentToolbar selectedNodeIds={["a"]} />);
    expect(
      (screen.getByTestId("kcreate-align-left") as HTMLButtonElement).disabled,
    ).toBe(true);
    expect(
      (screen.getByTestId("kcreate-distribute-horizontal") as HTMLButtonElement)
        .disabled,
    ).toBe(true);
    expect(
      screen.getByText("Select 2+ nodes to align, 3+ to distribute."),
    ).toBeInTheDocument();
  });

  it("enables align buttons with 2 selected, but keeps distribute disabled", () => {
    installStub();
    render(<AlignmentToolbar selectedNodeIds={["a", "b"]} />);
    expect(
      (screen.getByTestId("kcreate-align-left") as HTMLButtonElement).disabled,
    ).toBe(false);
    expect(
      (screen.getByTestId("kcreate-distribute-horizontal") as HTMLButtonElement)
        .disabled,
    ).toBe(true);
  });

  it("enables every button with 3+ selected", () => {
    installStub();
    render(<AlignmentToolbar selectedNodeIds={["a", "b", "c"]} />);
    for (const a of [
      "left",
      "center",
      "right",
      "top",
      "middle",
      "bottom",
    ] as const) {
      expect(
        (screen.getByTestId(`kcreate-align-${a}`) as HTMLButtonElement).disabled,
      ).toBe(false);
    }
    for (const d of ["horizontal", "vertical"] as const) {
      expect(
        (screen.getByTestId(`kcreate-distribute-${d}`) as HTMLButtonElement)
          .disabled,
      ).toBe(false);
    }
  });

  it("dispatches documentAlign with the alignment value and the selection ids", async () => {
    const captured = installStub();
    render(<AlignmentToolbar selectedNodeIds={["a", "b"]} />);
    fireEvent.click(screen.getByTestId("kcreate-align-center"));
    await waitFor(() =>
      expect(captured.align).toEqual([
        { ids: ["a", "b"], alignment: "center" },
      ]),
    );
  });

  it("dispatches documentDistribute with the axis and the selection ids", async () => {
    const captured = installStub();
    render(<AlignmentToolbar selectedNodeIds={["a", "b", "c"]} />);
    fireEvent.click(screen.getByTestId("kcreate-distribute-vertical"));
    await waitFor(() =>
      expect(captured.distribute).toEqual([
        { ids: ["a", "b", "c"], axis: "vertical" },
      ]),
    );
  });

  it("fires onApplied after a successful align", async () => {
    installStub();
    let appliedCount = 0;
    render(
      <AlignmentToolbar
        selectedNodeIds={["a", "b"]}
        onApplied={() => {
          appliedCount += 1;
        }}
      />,
    );
    fireEvent.click(screen.getByTestId("kcreate-align-left"));
    await waitFor(() => expect(appliedCount).toBe(1));
  });

  it("surfaces a bridge error inline without crashing", async () => {
    installStub({ rejectAlign: new Error("align failed") });
    render(<AlignmentToolbar selectedNodeIds={["a", "b"]} />);
    fireEvent.click(screen.getByTestId("kcreate-align-left"));
    await waitFor(() =>
      expect(screen.getByRole("alert")).toHaveTextContent("align failed"),
    );
  });
});
