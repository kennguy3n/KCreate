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

describe("PathfinderPanel — pending-state gating", () => {
  // Devin Review #0002 (round 3) on PR #38: without an in-flight
  // gate, a user can double-click a boolean-op button during the
  // async `pathBoolean` IPC. The first call deletes the source
  // nodes and inserts the results; the second call sees the
  // now-deleted source ids and surfaces a confusing
  // `SourceNotFound` toast for a gesture that visually succeeded.
  //
  // These tests pin the gate at the panel level so the issue is
  // caught at the same boundary that introduced it (the panel),
  // independent of any host-side debouncing.

  it("disables all four buttons while pathBoolean is in flight", async () => {
    const stub = kcreateStub();
    // Park the IPC indefinitely so the panel stays in its pending
    // state and we can observe the disabled markup before
    // resolution. `Promise.resolve(override(...))` unwraps thenables
    // so returning a Promise from the override forwards it intact
    // to the awaiter inside `apply`. We use a holder object instead
    // of a `let` binding so TS doesn't narrow the closure-assigned
    // value to `never` at the call site below.
    const bridge: { release: ((ids: string[]) => void) | null } = {
      release: null,
    };
    stub.override(
      "canvas.pathBoolean",
      () =>
        new Promise<string[]>((resolve) => {
          bridge.release = resolve;
        }),
    );
    const nodes = [mkNode("v1", "VectorLayer"), mkNode("v2", "VectorLayer")];
    mount({ selectedIds: ["v1", "v2"], nodes });

    // Pre-condition: every button is enabled before the click.
    for (const op of [
      "union",
      "subtract",
      "intersect",
      "exclude",
    ] as const) {
      expect(
        (screen.getByTestId(`pathfinder-${op}`) as HTMLButtonElement).disabled,
        `${op} should be enabled before any click`,
      ).toBe(false);
    }

    fireEvent.click(screen.getByTestId("pathfinder-union"));
    // Give React a microtask to flush the `setIsPending(true)`
    // state update — without this the click handler hasn't
    // committed and the disabled flag hasn't propagated yet.
    await flushAsync();

    // While the bridge call is parked, every button (including
    // the other three ops the user didn't click) must be
    // disabled. Disabling the whole row, not just the clicked
    // button, prevents a "click Union, immediately click
    // Intersect" double-fire that would race two boolean ops on
    // the same source set.
    for (const op of [
      "union",
      "subtract",
      "intersect",
      "exclude",
    ] as const) {
      expect(
        (screen.getByTestId(`pathfinder-${op}`) as HTMLButtonElement).disabled,
        `${op} should be disabled while pathBoolean is in flight`,
      ).toBe(true);
    }

    // Release the IPC so the test doesn't leak a pending
    // promise; assert re-enable in the next test.
    bridge.release?.(["r1"]);
    await flushAsync();
  });

  it("re-enables all buttons after the bridge resolves", async () => {
    const stub = kcreateStub();
    const bridge: { release: ((ids: string[]) => void) | null } = {
      release: null,
    };
    stub.override(
      "canvas.pathBoolean",
      () =>
        new Promise<string[]>((resolve) => {
          bridge.release = resolve;
        }),
    );
    const nodes = [mkNode("v1", "VectorLayer"), mkNode("v2", "VectorLayer")];
    const { applied } = mount({ selectedIds: ["v1", "v2"], nodes });

    fireEvent.click(screen.getByTestId("pathfinder-subtract"));
    await flushAsync();
    expect(
      (screen.getByTestId("pathfinder-subtract") as HTMLButtonElement).disabled,
    ).toBe(true);

    // Release the bridge; the `finally` block must clear the
    // pending state regardless of whether the promise resolved or
    // rejected.
    await act(async () => {
      bridge.release?.(["r1", "r2"]);
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(applied).toHaveBeenCalledWith(["r1", "r2"]);
    for (const op of [
      "union",
      "subtract",
      "intersect",
      "exclude",
    ] as const) {
      expect(
        (screen.getByTestId(`pathfinder-${op}`) as HTMLButtonElement).disabled,
        `${op} should be re-enabled after the bridge resolves`,
      ).toBe(false);
    }
  });

  it("re-enables all buttons after the bridge rejects", async () => {
    const stub = kcreateStub();
    const bridge: { reject: ((err: Error) => void) | null } = {
      reject: null,
    };
    stub.override(
      "canvas.pathBoolean",
      () =>
        new Promise<string[]>((_, reject) => {
          bridge.reject = reject;
        }),
    );
    const nodes = [mkNode("v1", "VectorLayer"), mkNode("v2", "VectorLayer")];
    const { captured } = mount({ selectedIds: ["v1", "v2"], nodes });

    fireEvent.click(screen.getByTestId("pathfinder-exclude"));
    await flushAsync();
    expect(
      (screen.getByTestId("pathfinder-exclude") as HTMLButtonElement).disabled,
    ).toBe(true);

    await act(async () => {
      bridge.reject?.(new Error("boolean op produced no output"));
      await Promise.resolve();
      await Promise.resolve();
    });

    await waitFor(() => {
      expect(captured.lastStatus).toMatch(/^exclude failed:/);
    });
    for (const op of [
      "union",
      "subtract",
      "intersect",
      "exclude",
    ] as const) {
      expect(
        (screen.getByTestId(`pathfinder-${op}`) as HTMLButtonElement).disabled,
        `${op} should be re-enabled after the bridge rejects`,
      ).toBe(false);
    }
  });

  it("only invokes pathBoolean once when a button is double-clicked rapidly", async () => {
    // The canonical race the gate protects against: rapid
    // double-click on Union while the first IPC is still in
    // flight. The second click must be a no-op (button disabled,
    // and `apply`'s own `if (isPending) return` belt-and-braces).
    const stub = kcreateStub();
    const bridge: { release: ((ids: string[]) => void) | null } = {
      release: null,
    };
    stub.override(
      "canvas.pathBoolean",
      () =>
        new Promise<string[]>((resolve) => {
          bridge.release = resolve;
        }),
    );
    const nodes = [mkNode("v1", "VectorLayer"), mkNode("v2", "VectorLayer")];
    mount({ selectedIds: ["v1", "v2"], nodes });

    const btn = screen.getByTestId("pathfinder-union");
    fireEvent.click(btn);
    await flushAsync();
    // Second click while disabled — should be ignored by the
    // browser (disabled buttons don't fire click handlers) AND
    // by the in-handler guard if a synthetic test bypass slipped
    // through.
    fireEvent.click(btn);
    await flushAsync();

    const calls = stub.calls.filter((c) => c.method === "canvas.pathBoolean");
    expect(
      calls,
      "double-click while pending must fire pathBoolean exactly once",
    ).toHaveLength(1);

    await act(async () => {
      bridge.release?.(["r1"]);
      await Promise.resolve();
      await Promise.resolve();
    });
  });
});
