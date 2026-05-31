// PathfinderPanel tests (Phase B2).
//
// Pins down the boolean-op panel surface added in Phase B2:
//   * the panel renders nothing (returns null) when fewer than
//     two VectorLayer nodes are selected, including the mixed-
//     selection case (1 vector + 1 text, etc.) — both at
//     `selectedIds.length < 2` and at `vectorSelection.length < 2`;
//   * when ≥2 vector layers are selected, all four op buttons
//     appear with the expected `data-testid` and labels;
//   * clicking each button invokes
//     `window.kcreate.canvas.pathBoolean` with the op token and
//     the FILTERED vector selection (text-layer ids in the
//     selection must be dropped);
//   * `onApplied` is invoked with the returned result ids when
//     the bridge resolves;
//   * `onStatus` is invoked with a formatted error string when
//     the bridge rejects.
//
// Mounts under the session-wide kcreate stub installed in
// `setup.vitest.ts`. The stub default for `canvas.pathBoolean` is
// `["default-bool-result-id"]`; tests that want a specific result
// override via `stub.override("canvas.pathBoolean", () => […])`.

import { describe, it, expect, vi } from "vitest";
import {
  render,
  screen,
  fireEvent,
  waitFor,
  act,
} from "@testing-library/react";

import { PathfinderPanel } from "./PathfinderPanel";
import type { NodeInfo, PathBooleanOp } from "../../../shared/scene";
import { kcreateStub } from "../../tests/helpers/kcreateStub";

async function flushAsync() {
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
  });
}

function mkNode(
  id: string,
  nodeType: string,
  overrides: Partial<NodeInfo> = {},
): NodeInfo {
  return {
    id,
    nodeType,
    parentId: null,
    children: [],
    name: id,
    visible: true,
    locked: false,
    bounds: { x: 0, y: 0, width: 10, height: 10 },
    version: 1,
    ...overrides,
  };
}

interface MountOptions {
  selectedIds: string[];
  nodes: NodeInfo[];
}

function mount(opts: MountOptions) {
  let lastStatus: string | null | undefined;
  const applied = vi.fn<(ids: string[]) => void>();
  const utils = render(
    <PathfinderPanel
      selectedIds={opts.selectedIds}
      nodes={opts.nodes}
      onStatus={(msg) => {
        lastStatus = msg;
      }}
      onApplied={applied}
    />,
  );
  return {
    ...utils,
    applied,
    captured: {
      get lastStatus() {
        return lastStatus;
      },
    },
  };
}

describe("PathfinderPanel — visibility gating", () => {
  it("renders nothing when selection is empty", () => {
    const { container } = mount({ selectedIds: [], nodes: [] });
    expect(container.firstChild).toBeNull();
  });

  it("renders nothing when only one vector layer is selected", () => {
    const nodes = [mkNode("v1", "VectorLayer")];
    const { container } = mount({ selectedIds: ["v1"], nodes });
    expect(container.firstChild).toBeNull();
  });

  it("renders nothing when two non-vector layers are selected", () => {
    // Two selected nodes, but they're text — the boolean panel is
    // vector-only and must hide rather than misleadingly enable.
    const nodes = [mkNode("t1", "TextLayer"), mkNode("t2", "TextLayer")];
    const { container } = mount({ selectedIds: ["t1", "t2"], nodes });
    expect(container.firstChild).toBeNull();
  });

  it("renders nothing when selection mixes 1 vector + 1 text", () => {
    // After filtering to vector-only, only 1 layer remains — the
    // panel must hide, otherwise the user would see enabled
    // buttons that error on click.
    const nodes = [mkNode("v1", "VectorLayer"), mkNode("t1", "TextLayer")];
    const { container } = mount({ selectedIds: ["v1", "t1"], nodes });
    expect(container.firstChild).toBeNull();
  });

  it("renders all four op buttons when 2+ vector layers are selected", () => {
    const nodes = [mkNode("v1", "VectorLayer"), mkNode("v2", "VectorLayer")];
    mount({ selectedIds: ["v1", "v2"], nodes });
    for (const op of ["union", "subtract", "intersect", "exclude"] as const) {
      expect(
        screen.getByTestId(`pathfinder-${op}`),
        `${op} button should render`,
      ).toBeInTheDocument();
    }
    // The panel shows the count of vector layers it will operate on.
    expect(screen.getByText("2 vector layers")).toBeInTheDocument();
  });
});

describe("PathfinderPanel — bridge interaction", () => {
  it.each<PathBooleanOp>(["union", "subtract", "intersect", "exclude"])(
    "calls pathBoolean(%s, vectorIds) on click",
    async (op) => {
      const stub = kcreateStub();
      const nodes = [mkNode("v1", "VectorLayer"), mkNode("v2", "VectorLayer")];
      mount({ selectedIds: ["v1", "v2"], nodes });

      fireEvent.click(screen.getByTestId(`pathfinder-${op}`));
      await flushAsync();

      const call = stub.calls.find((c) => c.method === "canvas.pathBoolean");
      expect(call, `${op} click should invoke pathBoolean`).toBeDefined();
      expect(call?.args[0]).toBe(op);
      expect(call?.args[1]).toEqual(["v1", "v2"]);
    },
  );

  it("filters non-vector layers out of the bridge call", async () => {
    // Selection contains 2 vector + 1 text + 1 raster. Only the
    // two vector ids should be forwarded to the bridge — the
    // panel does the type filter so the bridge's
    // `SourceNotVector` error never has to fire under normal UI
    // flow.
    const stub = kcreateStub();
    const nodes = [
      mkNode("v1", "VectorLayer"),
      mkNode("t1", "TextLayer"),
      mkNode("v2", "VectorLayer"),
      mkNode("r1", "RasterLayer"),
    ];
    mount({ selectedIds: ["v1", "t1", "v2", "r1"], nodes });

    fireEvent.click(screen.getByTestId("pathfinder-union"));
    await flushAsync();

    const call = stub.calls.find((c) => c.method === "canvas.pathBoolean");
    expect(call?.args[1]).toEqual(["v1", "v2"]);
  });

  it("preserves selection iteration order (z-bottom-first) in the bridge call", async () => {
    // The bridge folds left-to-right and inherits the FIRST
    // source's style, so the panel must not re-sort the
    // selection. We pin this with a non-natural order — if a
    // future refactor accidentally sorts by id alphabetically,
    // this test catches it.
    const stub = kcreateStub();
    const nodes = [
      mkNode("z-bottom", "VectorLayer"),
      mkNode("a-top", "VectorLayer"),
      mkNode("m-middle", "VectorLayer"),
    ];
    mount({
      selectedIds: ["z-bottom", "m-middle", "a-top"],
      nodes,
    });

    fireEvent.click(screen.getByTestId("pathfinder-subtract"));
    await flushAsync();

    const call = stub.calls.find((c) => c.method === "canvas.pathBoolean");
    expect(call?.args[1]).toEqual(["z-bottom", "m-middle", "a-top"]);
  });

  it("invokes onApplied with the bridge-returned result ids on success", async () => {
    const stub = kcreateStub();
    stub.override("canvas.pathBoolean", () => ["r1", "r2", "r3"]);
    const nodes = [mkNode("v1", "VectorLayer"), mkNode("v2", "VectorLayer")];
    const { applied, captured } = mount({
      selectedIds: ["v1", "v2"],
      nodes,
    });

    fireEvent.click(screen.getByTestId("pathfinder-exclude"));
    await flushAsync();

    expect(applied).toHaveBeenCalledTimes(1);
    expect(applied).toHaveBeenCalledWith(["r1", "r2", "r3"]);
    // Success path clears any prior status message.
    expect(captured.lastStatus).toBeNull();
  });

  it("surfaces a typed error message via onStatus on bridge rejection", async () => {
    const stub = kcreateStub();
    stub.override("canvas.pathBoolean", () => {
      throw new Error("source node x is a TextLayer, expected a VectorLayer");
    });
    const nodes = [mkNode("v1", "VectorLayer"), mkNode("v2", "VectorLayer")];
    const { applied, captured } = mount({
      selectedIds: ["v1", "v2"],
      nodes,
    });

    fireEvent.click(screen.getByTestId("pathfinder-intersect"));
    await flushAsync();
    await waitFor(() => {
      expect(captured.lastStatus).toMatch(/^intersect failed:/);
    });
    expect(captured.lastStatus).toContain("expected a VectorLayer");
    // Error path must not advance the selection — onApplied must
    // NOT be called so the user keeps the originals selected and
    // can fix the inputs.
    expect(applied).not.toHaveBeenCalled();
  });
});
