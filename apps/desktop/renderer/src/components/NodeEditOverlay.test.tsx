// NodeEditOverlay tests (Phase B3).
//
// Covers the overlay surface added in Phase B3:
//   * renders `null` when the state machine is idle (so the
//     overlay doesn't draw a stray SVG over the canvas when no
//     node-edit gesture is in flight);
//   * renders one anchor square per element in
//     `state.anchors`, sized to ANCHOR_SIDE_PX, with the
//     `data-selected` attribute reflecting set membership;
//   * renders one control-handle dot (and its dashed tangent
//     line) per non-null `inHandle` / `outHandle`;
//   * projects world coords through the live viewport
//     (`screen = world * zoom + pan`);
//   * emits an SVG `<path>` outline whose `d` matches the
//     line-vs-cubic decision rule for the anchor sequence;
//   * re-renders when the state machine fires `notify()` (the
//     `useSyncExternalStore` subscription works the same way it
//     does for `PenOverlay`).
//
// Uses a minimal in-memory `ToolStateMachine` fake (not
// `useToolStateMachine`) so the overlay surface is tested in
// isolation from pointer-event dispatch logic. State-machine
// integration is covered by `useToolStateMachine.test.tsx`.

import { describe, it, expect } from "vitest";
import { render, act, cleanup } from "@testing-library/react";
import { afterEach } from "vitest";

import { NodeEditOverlay } from "./NodeEditOverlay";
import type {
  PenAnchor,
  ToolMachineState,
  ToolStateMachine,
} from "../hooks/useToolStateMachine";

afterEach(() => {
  cleanup();
});

// Minimal `ToolStateMachine` fake: only `subscribe` and
// `getState` are touched by `useSyncExternalStore`. The other
// methods exist so the type signature is satisfied — calling
// them throws so a future overlay accidentally invoking them
// from render fails loudly.
function makeMachine(initial: ToolMachineState): {
  machine: ToolStateMachine;
  set: (next: ToolMachineState) => void;
} {
  let state: ToolMachineState = initial;
  const listeners = new Set<() => void>();
  const machine: ToolStateMachine = {
    getState: () => state,
    subscribe: (l: () => void) => {
      listeners.add(l);
      return () => {
        listeners.delete(l);
      };
    },
    onCanvasPointer: () => {
      throw new Error("onCanvasPointer should not be called by overlay");
    },
    commitPen: async () => {
      throw new Error("commitPen should not be called by overlay");
    },
    cancelPen: () => {
      throw new Error("cancelPen should not be called by overlay");
    },
    enterNodeEdit: async () => {
      throw new Error("enterNodeEdit should not be called by overlay");
    },
    commitNodeEdit: async () => {
      throw new Error("commitNodeEdit should not be called by overlay");
    },
    cancelNodeEdit: () => {
      throw new Error("cancelNodeEdit should not be called by overlay");
    },
    getLastCursorWorld: () => null,
  };
  const set = (next: ToolMachineState) => {
    state = next;
    for (const l of listeners) l();
  };
  return { machine, set };
}

function makeAnchor(
  x: number,
  y: number,
  inHandle: { x: number; y: number } | null = null,
  outHandle: { x: number; y: number } | null = null,
): PenAnchor {
  return { x, y, inHandle, outHandle };
}

function makeNodeEditState(opts: {
  anchors: PenAnchor[];
  selected?: ReadonlySet<number>;
  closed?: boolean;
  translationX?: number;
  translationY?: number;
}): ToolMachineState {
  return {
    kind: "nodeEdit",
    tool: "select",
    nodeId: "n-test",
    anchors: opts.anchors,
    closed: opts.closed ?? false,
    translationX: opts.translationX ?? 0,
    translationY: opts.translationY ?? 0,
    selectedAnchorIndices: opts.selected ?? new Set<number>(),
    cursor: null,
    drag: null,
    dragMoved: false,
  };
}

const VIEWPORT_DEFAULT = { panX: 0, panY: 0, zoom: 1 };

describe("NodeEditOverlay", () => {
  it("renders null when state is idle", () => {
    const { machine } = makeMachine({ kind: "idle" });
    const { container } = render(
      <NodeEditOverlay
        machine={machine}
        viewport={VIEWPORT_DEFAULT}
        width={800}
        height={600}
      />,
    );
    expect(container.firstChild).toBe(null);
  });

  it("renders one anchor rect per element in state.anchors", () => {
    const { machine } = makeMachine(
      makeNodeEditState({
        anchors: [
          makeAnchor(0, 0),
          makeAnchor(100, 0),
          makeAnchor(100, 100),
        ],
      }),
    );
    const { container } = render(
      <NodeEditOverlay
        machine={machine}
        viewport={VIEWPORT_DEFAULT}
        width={800}
        height={600}
      />,
    );
    const rects = container.querySelectorAll(
      '[data-testid^="node-anchor-"][data-testid$="-rect"]',
    );
    expect(rects).toHaveLength(3);
  });

  it("marks selected anchors with data-selected='true' and idle anchors with 'false'", () => {
    const { machine } = makeMachine(
      makeNodeEditState({
        anchors: [makeAnchor(0, 0), makeAnchor(100, 0)],
        selected: new Set([1]),
      }),
    );
    const { container } = render(
      <NodeEditOverlay
        machine={machine}
        viewport={VIEWPORT_DEFAULT}
        width={800}
        height={600}
      />,
    );
    const r0 = container.querySelector('[data-testid="node-anchor-0-rect"]');
    const r1 = container.querySelector('[data-testid="node-anchor-1-rect"]');
    expect(r0?.getAttribute("data-selected")).toBe("false");
    expect(r1?.getAttribute("data-selected")).toBe("true");
  });

  it("renders one handle dot per non-null handle and skips null handles", () => {
    const { machine } = makeMachine(
      makeNodeEditState({
        anchors: [
          // First anchor: only outHandle.
          makeAnchor(0, 0, null, { x: 20, y: -10 }),
          // Second anchor: both handles.
          makeAnchor(
            100,
            0,
            { x: 80, y: -10 },
            { x: 120, y: 10 },
          ),
          // Third anchor: no handles at all (pure corner).
          makeAnchor(200, 0, null, null),
        ],
      }),
    );
    const { container } = render(
      <NodeEditOverlay
        machine={machine}
        viewport={VIEWPORT_DEFAULT}
        width={800}
        height={600}
      />,
    );
    expect(
      container.querySelector('[data-testid="node-anchor-0-handle-in"]'),
    ).toBe(null);
    expect(
      container.querySelector('[data-testid="node-anchor-0-handle-out"]'),
    ).not.toBe(null);
    expect(
      container.querySelector('[data-testid="node-anchor-1-handle-in"]'),
    ).not.toBe(null);
    expect(
      container.querySelector('[data-testid="node-anchor-1-handle-out"]'),
    ).not.toBe(null);
    expect(
      container.querySelector('[data-testid="node-anchor-2-handle-in"]'),
    ).toBe(null);
    expect(
      container.querySelector('[data-testid="node-anchor-2-handle-out"]'),
    ).toBe(null);
  });

  it("projects world coords through viewport (pan + zoom)", () => {
    // anchor at world (10, 20); viewport pan = (5, 7), zoom = 2.
    // expected screen = (10*2+5, 20*2+7) = (25, 47).
    // anchor square is ANCHOR_SIDE_PX=8 → x = 25-4, y = 47-4.
    const { machine } = makeMachine(
      makeNodeEditState({ anchors: [makeAnchor(10, 20)] }),
    );
    const { container } = render(
      <NodeEditOverlay
        machine={machine}
        viewport={{ panX: 5, panY: 7, zoom: 2 }}
        width={800}
        height={600}
      />,
    );
    const rect = container.querySelector(
      '[data-testid="node-anchor-0-rect"]',
    );
    expect(rect?.getAttribute("x")).toBe("21"); // 25 - 4
    expect(rect?.getAttribute("y")).toBe("43"); // 47 - 4
    expect(rect?.getAttribute("width")).toBe("8");
    expect(rect?.getAttribute("height")).toBe("8");
  });

  it("emits a straight line between two corner anchors in the path outline", () => {
    const { machine } = makeMachine(
      makeNodeEditState({
        anchors: [makeAnchor(0, 0), makeAnchor(50, 0)],
      }),
    );
    const { container } = render(
      <NodeEditOverlay
        machine={machine}
        viewport={VIEWPORT_DEFAULT}
        width={800}
        height={600}
      />,
    );
    const path = container.querySelector("path");
    expect(path?.getAttribute("d")).toBe("M 0 0 L 50 0");
  });

  it("emits a cubic segment when either side has a handle", () => {
    const { machine } = makeMachine(
      makeNodeEditState({
        anchors: [
          makeAnchor(0, 0, null, { x: 10, y: 0 }),
          makeAnchor(100, 0, { x: 90, y: 0 }, null),
        ],
      }),
    );
    const { container } = render(
      <NodeEditOverlay
        machine={machine}
        viewport={VIEWPORT_DEFAULT}
        width={800}
        height={600}
      />,
    );
    const path = container.querySelector("path");
    expect(path?.getAttribute("d")).toBe(
      "M 0 0 C 10 0, 90 0, 100 0",
    );
  });

  it("appends a Z (and the closing line / cubic) when state.closed is true", () => {
    const { machine } = makeMachine(
      makeNodeEditState({
        anchors: [
          makeAnchor(0, 0),
          makeAnchor(100, 0),
          makeAnchor(100, 100),
        ],
        closed: true,
      }),
    );
    const { container } = render(
      <NodeEditOverlay
        machine={machine}
        viewport={VIEWPORT_DEFAULT}
        width={800}
        height={600}
      />,
    );
    const path = container.querySelector("path");
    // The closing segment is a straight line back to (0, 0) +
    // the Z terminator.
    expect(path?.getAttribute("d")).toBe(
      "M 0 0 L 100 0 L 100 100 L 0 0 Z",
    );
  });

  it("re-renders when the state machine fires notify()", () => {
    // Start in idle (overlay returns null) and verify the
    // overlay appears once notify() reports the transition.
    const { machine, set } = makeMachine({ kind: "idle" });
    const { container } = render(
      <NodeEditOverlay
        machine={machine}
        viewport={VIEWPORT_DEFAULT}
        width={800}
        height={600}
      />,
    );
    expect(container.firstChild).toBe(null);
    act(() => {
      set(
        makeNodeEditState({
          anchors: [makeAnchor(0, 0), makeAnchor(50, 0)],
        }),
      );
    });
    const rects = container.querySelectorAll(
      '[data-testid^="node-anchor-"][data-testid$="-rect"]',
    );
    expect(rects).toHaveLength(2);
  });
});
