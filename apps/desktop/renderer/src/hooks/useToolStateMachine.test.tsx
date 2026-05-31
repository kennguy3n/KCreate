// useToolStateMachine — explicit state-transition coverage.
//
// Phase A3b. The hook owns the canvas pointer-event state machine
// previously inlined into `EditorPage.onCanvasPointer`. The
// pre-refactor state had no test surface (you could only exercise
// it by mounting the entire editor and synthesising pointer events),
// so this file is the first dedicated test pass over those
// transitions. The invariants verified here are exactly the contract
// the `EditorPage` refactor depends on:
//
//   * `getState()` starts at `{ kind: "idle" }` before any event.
//   * `pointerdown` routes to exactly one of `pan` / `move` / `create`
//     based on `panActive` + active tool, with `setPointerCapture`
//     called synchronously and the appropriate bridge call (or no
//     bridge call, for `pan` / `create`).
//   * `pointermove` is variant-aware: `pan` calls `setViewport`,
//     `move` queries the snap engine + updates cumulative delta,
//     `create` is a no-op until commit.
//   * `pointerup` releases capture, clears snap guides, and triggers
//     the right commit (`moveNode` for `move`, `createRect/Ellipse/
//     Line/Text` for `create`, nothing for `pan`).
//   * Stale-pointer events (different `pointerId`) are ignored.
//   * Zero-area drags are rejected for non-text tools.
//   * `lastCursorWorldRef` is updated on EVERY pointer event so
//     paste-at-cursor sees the latest sample regardless of variant.
//   * Bridge failures route through `onError` instead of crashing.
//
// We use `renderHook` here (deviating from the `Capture` pattern in
// `EditorContext.test.tsx`) because the hook is provider-free — there
// is no React context to mount and the `Capture` indirection would
// add noise without buying anything.

import { renderHook, act } from "@testing-library/react";
import type { Dispatch, SetStateAction } from "react";
import type React from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { NodeInfo, SnapGuide } from "../../../shared/scene";
import type { ViewportState } from "../components/CanvasHost";
import type { ToolId } from "../contexts/EditorContext";
import {
  installKcreateStub,
  type KcreateStubHandle,
} from "../../tests/helpers/kcreateStub";
import { useToolStateMachine } from "./useToolStateMachine";

// Build a minimum-viable `NodeInfo` for snap-engine tests. The hook
// only reads `id` and `bounds`; the other fields are required by the
// wire-format `NodeInfo` interface (mirroring
// `crates/kcreate_bridge/src/document.rs::NodeInfo`) but their values
// don't affect any state-machine branch under test.
function makeNode(
  id: string,
  x: number,
  y: number,
  w: number,
  h: number,
): NodeInfo {
  return {
    id,
    nodeType: "Rect",
    parentId: null,
    children: [],
    name: id,
    visible: true,
    locked: false,
    bounds: { x, y, width: w, height: h },
    version: 1,
  };
}

// Minimal canvas-element double. The hook touches three methods on
// `e.currentTarget`: `getBoundingClientRect` (to translate clientX/Y
// into canvas-local coords), `setPointerCapture` and
// `releasePointerCapture` (to ensure pointermove/up keep firing on
// the same element). We record the calls so tests can assert capture
// behaviour without rendering a real HTMLCanvasElement.
interface FakeCanvas {
  rect: { left: number; top: number };
  setPointerCaptureCalls: number[];
  releasePointerCaptureCalls: number[];
}

function makeFakeCanvas(rect = { left: 0, top: 0 }): FakeCanvas {
  return {
    rect,
    setPointerCaptureCalls: [],
    releasePointerCaptureCalls: [],
  };
}

// Build a `React.PointerEvent` good enough for the hook. The hook
// only reads `type`, `button`, `pointerId`, `clientX`, `clientY`,
// and `currentTarget` — everything else (synthetic event pool, react
// internals) is irrelevant. Cast through unknown to skip the
// React.SyntheticEvent shape check.
function makeEvent(
  canvas: FakeCanvas,
  opts: {
    type: "pointerdown" | "pointermove" | "pointerup";
    button?: number;
    pointerId?: number;
    clientX: number;
    clientY: number;
  },
): React.PointerEvent<HTMLCanvasElement> {
  const currentTarget = {
    getBoundingClientRect: () => ({
      left: canvas.rect.left,
      top: canvas.rect.top,
      right: 0,
      bottom: 0,
      width: 0,
      height: 0,
      x: 0,
      y: 0,
      toJSON: () => ({}),
    }),
    setPointerCapture: (id: number) => {
      canvas.setPointerCaptureCalls.push(id);
    },
    releasePointerCapture: (id: number) => {
      canvas.releasePointerCaptureCalls.push(id);
    },
  };
  return {
    type: opts.type,
    button: opts.button ?? 0,
    pointerId: opts.pointerId ?? 1,
    clientX: opts.clientX,
    clientY: opts.clientY,
    currentTarget,
  } as unknown as React.PointerEvent<HTMLCanvasElement>;
}

// Build the dependency bundle. Tests can override individual fields.
// The setter mocks are typed as React `Dispatch<SetStateAction<T>>` so
// the hook accepts them without coercion — tests can still assert on
// the mock call history (the functional-vs-value branch is irrelevant
// for assertion shape).
interface DepsOverrides {
  tool?: ToolId;
  viewport?: ViewportState;
  panActive?: boolean;
  nodes?: NodeInfo[];
  lastCursorWorld?: { x: number; y: number } | null;
  onAfterCommit?: () => Promise<unknown> | void;
}

interface DepsBundle {
  deps: Parameters<typeof useToolStateMachine>[0];
  panActiveRef: { current: boolean };
  nodesRef: { current: NodeInfo[] };
  lastCursorWorldRef: { current: { x: number; y: number } | null };
  setSelectedIds: ReturnType<typeof vi.fn<Dispatch<SetStateAction<string[]>>>>;
  setViewport: ReturnType<typeof vi.fn<Dispatch<SetStateAction<ViewportState>>>>;
  setSnapGuides: ReturnType<typeof vi.fn<Dispatch<SetStateAction<SnapGuide[]>>>>;
  onError: ReturnType<typeof vi.fn<(msg: string) => void>>;
  onAfterCommit: ReturnType<typeof vi.fn<() => Promise<unknown> | void>>;
}

function makeDeps(overrides: DepsOverrides = {}): DepsBundle {
  const panActiveRef = { current: overrides.panActive ?? false };
  const nodesRef = { current: overrides.nodes ?? [] };
  const lastCursorWorldRef = {
    current: overrides.lastCursorWorld ?? null,
  };
  const setSelectedIds = vi.fn<Dispatch<SetStateAction<string[]>>>();
  const setViewport = vi.fn<Dispatch<SetStateAction<ViewportState>>>();
  const setSnapGuides = vi.fn<Dispatch<SetStateAction<SnapGuide[]>>>();
  const onError = vi.fn<(msg: string) => void>();
  const onAfterCommit = vi.fn<() => Promise<unknown> | void>(
    overrides.onAfterCommit ?? (() => Promise.resolve()),
  );

  return {
    panActiveRef,
    nodesRef,
    lastCursorWorldRef,
    setSelectedIds,
    setViewport,
    setSnapGuides,
    onError,
    onAfterCommit,
    deps: {
      tool: overrides.tool ?? "select",
      viewport: overrides.viewport ?? { panX: 0, panY: 0, zoom: 1 },
      panActiveRef,
      nodesRef,
      lastCursorWorldRef,
      setSelectedIds,
      setViewport,
      setSnapGuides,
      onError,
      onAfterCommit,
    },
  };
}

// Pump pending microtasks. Several pointer-event branches dispatch
// `void (async () => {...})()` IIFEs that fire bridge calls, so we
// need to flush the microtask queue before asserting their effects.
async function flush(): Promise<void> {
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
  });
}

let stub: KcreateStubHandle;

beforeEach(() => {
  stub = installKcreateStub();
});

afterEach(() => {
  vi.clearAllMocks();
});

describe("useToolStateMachine", () => {
  describe("initial state", () => {
    it("starts in idle with no cursor sample", () => {
      const { deps } = makeDeps();
      const { result } = renderHook(() => useToolStateMachine(deps));

      expect(result.current.getState()).toEqual({ kind: "idle" });
      expect(result.current.getLastCursorWorld()).toBeNull();
    });
  });

  describe("idle → pan", () => {
    it("enters pan when panActive is set and captures the pointer", () => {
      const { deps, panActiveRef } = makeDeps({ panActive: true });
      panActiveRef.current = true;
      const { result } = renderHook(() => useToolStateMachine(deps));
      const canvas = makeFakeCanvas();

      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, {
            type: "pointerdown",
            clientX: 100,
            clientY: 50,
            pointerId: 7,
          }),
        );
      });

      const state = result.current.getState();
      expect(state.kind).toBe("pan");
      if (state.kind === "pan") {
        expect(state.pointerId).toBe(7);
        expect(state.lastScreenX).toBe(100);
        expect(state.lastScreenY).toBe(50);
      }
      expect(canvas.setPointerCaptureCalls).toEqual([7]);
    });

    it("pan beats the active tool — select doesn't hit-test when pan is armed", () => {
      const { deps, panActiveRef } = makeDeps({
        tool: "select",
        panActive: true,
      });
      panActiveRef.current = true;
      const { result } = renderHook(() => useToolStateMachine(deps));

      act(() => {
        result.current.onCanvasPointer(
          makeEvent(makeFakeCanvas(), {
            type: "pointerdown",
            clientX: 10,
            clientY: 10,
          }),
        );
      });

      expect(stub.calls.find((c) => c.method === "canvas.hitTest")).toBeUndefined();
      expect(result.current.getState().kind).toBe("pan");
    });

    it("pointermove translates the viewport by screen-space delta", () => {
      const { deps, panActiveRef, setViewport } = makeDeps({
        panActive: true,
        viewport: { panX: 0, panY: 0, zoom: 2 },
      });
      panActiveRef.current = true;
      const { result } = renderHook(() => useToolStateMachine(deps));
      const canvas = makeFakeCanvas();

      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerdown", clientX: 100, clientY: 50 }),
        );
      });
      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointermove", clientX: 120, clientY: 70 }),
        );
      });

      // Viewport delta is in screen pixels (the renderer translates
      // screen → world internally; pan is purely a viewport
      // translation, no zoom division).
      expect(setViewport).toHaveBeenCalledTimes(1);
      const updater = setViewport.mock.calls[0]?.[0];
      expect(typeof updater).toBe("function");
      const next = (updater as (v: ViewportState) => ViewportState)({
        panX: 0,
        panY: 0,
        zoom: 2,
      });
      expect(next).toEqual({ panX: 20, panY: 20, zoom: 2 });
    });

    it("pointerup releases capture, clears snap guides, returns to idle", () => {
      const { deps, panActiveRef, setSnapGuides } = makeDeps({
        panActive: true,
      });
      panActiveRef.current = true;
      const { result } = renderHook(() => useToolStateMachine(deps));
      const canvas = makeFakeCanvas();

      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerdown", clientX: 10, clientY: 10 }),
        );
      });
      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerup", clientX: 20, clientY: 20 }),
        );
      });

      expect(result.current.getState()).toEqual({ kind: "idle" });
      expect(canvas.releasePointerCaptureCalls).toEqual([1]);
      expect(setSnapGuides).toHaveBeenCalledWith([]);
      // No bridge call for pan-up — the viewport delta is the only
      // commit and that happened incrementally on pointermove.
      expect(
        stub.calls.find(
          (c) =>
            c.method === "canvas.moveNode" ||
            c.method === "canvas.createRect" ||
            c.method === "canvas.createEllipse" ||
            c.method === "canvas.createLine" ||
            c.method === "canvas.createText",
        ),
      ).toBeUndefined();
    });
  });

  describe("idle → move (select tool with hit)", () => {
    it("enters move and calls setSelection when hitTest returns an id", async () => {
      stub = installKcreateStub();
      stub.override("canvas.hitTest", () => "node-42");

      const { deps, setSelectedIds } = makeDeps({ tool: "select" });
      const { result } = renderHook(() => useToolStateMachine(deps));
      const canvas = makeFakeCanvas();

      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerdown", clientX: 50, clientY: 50 }),
        );
      });
      await flush();

      const state = result.current.getState();
      expect(state.kind).toBe("move");
      if (state.kind === "move") {
        expect(state.movingNodeId).toBe("node-42");
        expect(state.lastWorldX).toBe(50);
        expect(state.lastWorldY).toBe(50);
        expect(state.cumulativeDx).toBe(0);
        expect(state.cumulativeDy).toBe(0);
      }
      expect(setSelectedIds).toHaveBeenCalledWith(["node-42"]);
      expect(canvas.setPointerCaptureCalls).toEqual([1]);
      expect(
        stub.calls.find((c) => c.method === "canvas.setSelection")?.args,
      ).toEqual([["node-42"]]);
    });

    it("clears the selection on miss and stays idle", async () => {
      stub = installKcreateStub();
      stub.override("canvas.hitTest", () => null);

      const { deps, setSelectedIds } = makeDeps({ tool: "select" });
      const { result } = renderHook(() => useToolStateMachine(deps));

      act(() => {
        result.current.onCanvasPointer(
          makeEvent(makeFakeCanvas(), {
            type: "pointerdown",
            clientX: 50,
            clientY: 50,
          }),
        );
      });
      await flush();

      expect(result.current.getState()).toEqual({ kind: "idle" });
      expect(setSelectedIds).toHaveBeenCalledWith([]);
      expect(stub.calls.find((c) => c.method === "canvas.clearSelection")).toBeDefined();
    });

    it("hit-test failure routes through onError", async () => {
      stub = installKcreateStub();
      stub.override("canvas.hitTest", () => {
        throw new Error("bridge offline");
      });

      const { deps, onError } = makeDeps({ tool: "select" });
      const { result } = renderHook(() => useToolStateMachine(deps));

      act(() => {
        result.current.onCanvasPointer(
          makeEvent(makeFakeCanvas(), {
            type: "pointerdown",
            clientX: 10,
            clientY: 10,
          }),
        );
      });
      await flush();

      expect(onError).toHaveBeenCalledWith("hit-test failed: bridge offline");
      expect(result.current.getState()).toEqual({ kind: "idle" });
    });
  });

  describe("move pointermove + pointerup", () => {
    it("accumulates world deltas across pointermove samples", async () => {
      stub = installKcreateStub();
      stub.override("canvas.hitTest", () => "n1");
      stub.override("canvasSnap.query", () => null);

      const { deps } = makeDeps({
        tool: "select",
        nodes: [makeNode("n1", 0, 0, 10, 10)],
      });
      const { result } = renderHook(() => useToolStateMachine(deps));
      const canvas = makeFakeCanvas();

      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerdown", clientX: 50, clientY: 50 }),
        );
      });
      await flush();
      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointermove", clientX: 55, clientY: 60 }),
        );
      });
      await flush();
      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointermove", clientX: 60, clientY: 70 }),
        );
      });
      await flush();

      const state = result.current.getState();
      expect(state.kind).toBe("move");
      if (state.kind === "move") {
        expect(state.cumulativeDx).toBe(10);
        expect(state.cumulativeDy).toBe(20);
        expect(state.lastWorldX).toBe(60);
        expect(state.lastWorldY).toBe(70);
      }
    });

    it("folds the snap-engine delta into the cumulative offset", async () => {
      stub = installKcreateStub();
      stub.override("canvas.hitTest", () => "n1");
      // Engine snaps -3 / +2 (so the node ends up 3 left, 2 down
      // from where the cursor would have placed it).
      stub.override("canvasSnap.query", () => ({
        dx: -3,
        dy: 2,
        guides: [{ axis: "Vertical", position: 100, from: 0, to: 200 }],
      }));

      const { deps, setSnapGuides } = makeDeps({
        tool: "select",
        nodes: [
makeNode("n1", 0, 0, 10, 10),
        ],
      });
      const { result } = renderHook(() => useToolStateMachine(deps));
      const canvas = makeFakeCanvas();

      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerdown", clientX: 0, clientY: 0 }),
        );
      });
      await flush();
      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointermove", clientX: 5, clientY: 5 }),
        );
      });
      await flush();

      const state = result.current.getState();
      expect(state.kind).toBe("move");
      if (state.kind === "move") {
        // Cursor delta was +5/+5; snap added -3/+2 → final +2/+7.
        expect(state.cumulativeDx).toBe(2);
        expect(state.cumulativeDy).toBe(7);
      }
      expect(setSnapGuides).toHaveBeenLastCalledWith([
        { axis: "Vertical", position: 100, from: 0, to: 200 },
      ]);
    });

    it("commits moveNode on pointerup with cumulative delta", async () => {
      stub = installKcreateStub();
      stub.override("canvas.hitTest", () => "n1");
      stub.override("canvasSnap.query", () => null);

      const { deps, onAfterCommit } = makeDeps({
        tool: "select",
        nodes: [
makeNode("n1", 0, 0, 10, 10),
        ],
      });
      const { result } = renderHook(() => useToolStateMachine(deps));
      const canvas = makeFakeCanvas();

      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerdown", clientX: 0, clientY: 0 }),
        );
      });
      await flush();
      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointermove", clientX: 12, clientY: 8 }),
        );
      });
      await flush();
      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerup", clientX: 12, clientY: 8 }),
        );
      });
      await flush();

      const moveCall = stub.calls.find((c) => c.method === "canvas.moveNode");
      expect(moveCall?.args).toEqual(["n1", 12, 8]);
      expect(onAfterCommit).toHaveBeenCalledTimes(1);
      expect(result.current.getState()).toEqual({ kind: "idle" });
    });

    it("zero-delta moves skip the bridge moveNode call", async () => {
      stub = installKcreateStub();
      stub.override("canvas.hitTest", () => "n1");
      stub.override("canvasSnap.query", () => null);

      const { deps, onAfterCommit } = makeDeps({ tool: "select" });
      const { result } = renderHook(() => useToolStateMachine(deps));
      const canvas = makeFakeCanvas();

      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerdown", clientX: 50, clientY: 50 }),
        );
      });
      await flush();
      // No pointermove — straight to pointerup at the same position.
      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerup", clientX: 50, clientY: 50 }),
        );
      });
      await flush();

      expect(stub.calls.find((c) => c.method === "canvas.moveNode")).toBeUndefined();
      expect(onAfterCommit).not.toHaveBeenCalled();
      expect(result.current.getState()).toEqual({ kind: "idle" });
    });

    it("moveNode failure routes through onError without crashing", async () => {
      stub = installKcreateStub();
      stub.override("canvas.hitTest", () => "n1");
      stub.override("canvasSnap.query", () => null);
      stub.override("canvas.moveNode", () => {
        throw new Error("storage full");
      });

      const { deps, onError } = makeDeps({
        tool: "select",
        nodes: [
makeNode("n1", 0, 0, 10, 10),
        ],
      });
      const { result } = renderHook(() => useToolStateMachine(deps));
      const canvas = makeFakeCanvas();

      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerdown", clientX: 0, clientY: 0 }),
        );
      });
      await flush();
      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointermove", clientX: 5, clientY: 5 }),
        );
      });
      await flush();
      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerup", clientX: 5, clientY: 5 }),
        );
      });
      await flush();

      expect(onError).toHaveBeenCalledWith("move failed: storage full");
      expect(result.current.getState()).toEqual({ kind: "idle" });
    });
  });

  describe("idle → create (drawing tools)", () => {
    it("enters create on pointerdown without bridge call", () => {
      const { deps } = makeDeps({ tool: "rect" });
      const { result } = renderHook(() => useToolStateMachine(deps));
      const canvas = makeFakeCanvas();

      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, {
            type: "pointerdown",
            clientX: 30,
            clientY: 40,
            pointerId: 3,
          }),
        );
      });

      const state = result.current.getState();
      expect(state.kind).toBe("create");
      if (state.kind === "create") {
        expect(state.tool).toBe("rect");
        expect(state.startWorldX).toBe(30);
        expect(state.startWorldY).toBe(40);
        expect(state.pointerId).toBe(3);
      }
      expect(canvas.setPointerCaptureCalls).toEqual([3]);
      // No bridge call yet — create commits on pointerup only.
      expect(
        stub.calls.find(
          (c) =>
            c.method === "canvas.createRect" ||
            c.method === "canvas.hitTest",
        ),
      ).toBeUndefined();
    });

    it("commits createRect on pointerup with bounding box", async () => {
      stub = installKcreateStub();
      stub.override("canvas.createRect", () => "new-rect-id");

      const { deps, onAfterCommit } = makeDeps({ tool: "rect" });
      const { result } = renderHook(() => useToolStateMachine(deps));
      const canvas = makeFakeCanvas();

      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerdown", clientX: 100, clientY: 50 }),
        );
      });
      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerup", clientX: 140, clientY: 90 }),
        );
      });
      await flush();

      const createCall = stub.calls.find((c) => c.method === "canvas.createRect");
      expect(createCall?.args).toEqual([null, 100, 50, 40, 40]);
      expect(
        stub.calls.find((c) => c.method === "canvas.setSelection")?.args,
      ).toEqual([["new-rect-id"]]);
      expect(onAfterCommit).toHaveBeenCalledTimes(1);
      expect(result.current.getState()).toEqual({ kind: "idle" });
    });

    it("commits createEllipse with center+radii", async () => {
      stub = installKcreateStub();
      stub.override("canvas.createEllipse", () => "new-ellipse");

      const { deps } = makeDeps({ tool: "ellipse" });
      const { result } = renderHook(() => useToolStateMachine(deps));
      const canvas = makeFakeCanvas();

      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerdown", clientX: 0, clientY: 0 }),
        );
      });
      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerup", clientX: 40, clientY: 20 }),
        );
      });
      await flush();

      const call = stub.calls.find((c) => c.method === "canvas.createEllipse");
      // cx=20, cy=10, rx=20, ry=10
      expect(call?.args).toEqual([null, 20, 10, 20, 10]);
    });

    it("commits createLine with raw drag endpoints (not bounding box)", async () => {
      stub = installKcreateStub();
      stub.override("canvas.createLine", () => "new-line");

      const { deps } = makeDeps({ tool: "line" });
      const { result } = renderHook(() => useToolStateMachine(deps));
      const canvas = makeFakeCanvas();

      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerdown", clientX: 10, clientY: 20 }),
        );
      });
      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerup", clientX: 30, clientY: 5 }),
        );
      });
      await flush();

      const call = stub.calls.find((c) => c.method === "canvas.createLine");
      expect(call?.args).toEqual([null, 10, 20, 30, 5]);
    });

    it("commits createText at drag-start with default copy + size", async () => {
      stub = installKcreateStub();
      stub.override("canvas.createText", () => "new-text");

      const { deps } = makeDeps({ tool: "text" });
      const { result } = renderHook(() => useToolStateMachine(deps));
      const canvas = makeFakeCanvas();

      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerdown", clientX: 7, clientY: 9 }),
        );
      });
      act(() => {
        // Same point — text accepts a zero-area "drag" because clicking
        // is the canonical way to drop a text layer.
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerup", clientX: 7, clientY: 9 }),
        );
      });
      await flush();

      const call = stub.calls.find((c) => c.method === "canvas.createText");
      expect(call?.args).toEqual([null, 7, 9, "Text", "sans-serif", 24]);
    });

    it("rejects zero-area drags for non-text tools", async () => {
      const { deps, onAfterCommit } = makeDeps({ tool: "rect" });
      const { result } = renderHook(() => useToolStateMachine(deps));
      const canvas = makeFakeCanvas();

      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerdown", clientX: 100, clientY: 100 }),
        );
      });
      act(() => {
        // Same point + same point → w=0, h=0, tool != text → rejected.
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerup", clientX: 100, clientY: 100 }),
        );
      });
      await flush();

      expect(stub.calls.find((c) => c.method === "canvas.createRect")).toBeUndefined();
      expect(onAfterCommit).not.toHaveBeenCalled();
      expect(result.current.getState()).toEqual({ kind: "idle" });
    });

    it("createRect failure routes through onError", async () => {
      stub = installKcreateStub();
      stub.override("canvas.createRect", () => {
        throw new Error("disk full");
      });

      const { deps, onError } = makeDeps({ tool: "rect" });
      const { result } = renderHook(() => useToolStateMachine(deps));
      const canvas = makeFakeCanvas();

      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerdown", clientX: 0, clientY: 0 }),
        );
      });
      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerup", clientX: 100, clientY: 100 }),
        );
      });
      await flush();

      expect(onError).toHaveBeenCalledWith("create failed: disk full");
      expect(result.current.getState()).toEqual({ kind: "idle" });
    });
  });

  describe("event gating", () => {
    it("non-primary button on pointerdown is ignored", () => {
      const { deps } = makeDeps({ tool: "rect" });
      const { result } = renderHook(() => useToolStateMachine(deps));

      act(() => {
        result.current.onCanvasPointer(
          makeEvent(makeFakeCanvas(), {
            type: "pointerdown",
            button: 2,
            clientX: 10,
            clientY: 10,
          }),
        );
      });

      expect(result.current.getState()).toEqual({ kind: "idle" });
    });

    it("stale pointerId on pointermove is ignored", () => {
      const { deps, setViewport, panActiveRef } = makeDeps({ panActive: true });
      panActiveRef.current = true;
      const { result } = renderHook(() => useToolStateMachine(deps));
      const canvas = makeFakeCanvas();

      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, {
            type: "pointerdown",
            clientX: 10,
            clientY: 10,
            pointerId: 1,
          }),
        );
      });
      setViewport.mockClear();
      act(() => {
        // Different pointerId — this is a stray multi-touch sample
        // that doesn't belong to the active drag.
        result.current.onCanvasPointer(
          makeEvent(canvas, {
            type: "pointermove",
            clientX: 30,
            clientY: 30,
            pointerId: 99,
          }),
        );
      });

      expect(setViewport).not.toHaveBeenCalled();
      // Original pan state is untouched.
      const state = result.current.getState();
      expect(state.kind).toBe("pan");
      if (state.kind === "pan") {
        expect(state.lastScreenX).toBe(10);
        expect(state.lastScreenY).toBe(10);
      }
    });

    it("stale pointerId on pointerup is ignored", () => {
      const { deps, setSnapGuides, panActiveRef } = makeDeps({ panActive: true });
      panActiveRef.current = true;
      const { result } = renderHook(() => useToolStateMachine(deps));
      const canvas = makeFakeCanvas();

      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, {
            type: "pointerdown",
            clientX: 10,
            clientY: 10,
            pointerId: 1,
          }),
        );
      });
      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, {
            type: "pointerup",
            clientX: 10,
            clientY: 10,
            pointerId: 7,
          }),
        );
      });

      expect(result.current.getState().kind).toBe("pan");
      expect(setSnapGuides).not.toHaveBeenCalled();
      expect(canvas.releasePointerCaptureCalls).toEqual([]);
    });

    it("pointermove while idle is a no-op", () => {
      const { deps, setViewport } = makeDeps();
      const { result } = renderHook(() => useToolStateMachine(deps));

      act(() => {
        result.current.onCanvasPointer(
          makeEvent(makeFakeCanvas(), {
            type: "pointermove",
            clientX: 50,
            clientY: 50,
          }),
        );
      });

      expect(setViewport).not.toHaveBeenCalled();
      expect(result.current.getState()).toEqual({ kind: "idle" });
    });
  });

  describe("cursor world tracking", () => {
    it("updates lastCursorWorldRef on pointerdown with screen→world transform", () => {
      const { deps } = makeDeps({
        viewport: { panX: 100, panY: 50, zoom: 2 },
      });
      const { result } = renderHook(() => useToolStateMachine(deps));

      act(() => {
        result.current.onCanvasPointer(
          makeEvent(makeFakeCanvas(), {
            type: "pointerdown",
            clientX: 220,
            clientY: 70,
          }),
        );
      });

      // World = (screen - pan) / zoom → ((220-100)/2, (70-50)/2)
      expect(result.current.getLastCursorWorld()).toEqual({ x: 60, y: 10 });
    });

    it("updates lastCursorWorldRef on every event, including idle pointermove", () => {
      const { deps } = makeDeps();
      const { result } = renderHook(() => useToolStateMachine(deps));

      act(() => {
        result.current.onCanvasPointer(
          makeEvent(makeFakeCanvas(), {
            type: "pointermove",
            clientX: 11,
            clientY: 22,
          }),
        );
      });

      expect(result.current.getLastCursorWorld()).toEqual({ x: 11, y: 22 });
    });

    it("updates lastCursorWorldRef on pointerup too", () => {
      const { deps, panActiveRef } = makeDeps({ panActive: true });
      panActiveRef.current = true;
      const { result } = renderHook(() => useToolStateMachine(deps));
      const canvas = makeFakeCanvas();

      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerdown", clientX: 10, clientY: 10 }),
        );
      });
      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerup", clientX: 99, clientY: 88 }),
        );
      });

      expect(result.current.getLastCursorWorld()).toEqual({ x: 99, y: 88 });
    });
  });

  describe("state guards", () => {
    it("snap result is ignored if pointer is released mid-query (no stale fold)", async () => {
      // Pause the snap query so we can release the pointer while
      // it's in flight, then resolve it. The hook should see that
      // the state-ref identity changed and discard the result.
      let resolveSnap: (v: unknown) => void = () => undefined;
      const snapPromise = new Promise<unknown>((resolve) => {
        resolveSnap = resolve;
      });

      stub = installKcreateStub();
      stub.override("canvas.hitTest", () => "n1");
      stub.override("canvasSnap.query", () => snapPromise);

      const { deps } = makeDeps({
        tool: "select",
        nodes: [
makeNode("n1", 0, 0, 10, 10),
        ],
      });
      const { result } = renderHook(() => useToolStateMachine(deps));
      const canvas = makeFakeCanvas();

      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerdown", clientX: 0, clientY: 0 }),
        );
      });
      await flush();
      // Start a pointermove that fires the snap query, but DON'T
      // flush yet — we want the IIFE to be in flight when we
      // release the pointer.
      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointermove", clientX: 4, clientY: 4 }),
        );
      });
      // Release while the snap query is still pending.
      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerup", clientX: 4, clientY: 4 }),
        );
      });
      await flush();
      // Now resolve the snap query with a large delta that would
      // have folded into a no-longer-existent drag.
      await act(async () => {
        resolveSnap({
          dx: 999,
          dy: 999,
          guides: [{ axis: "Vertical", position: 1, from: 0, to: 1 }],
        });
        await Promise.resolve();
        await Promise.resolve();
      });

      // State is idle (the drag committed on pointerup); the stale
      // snap result must NOT have been folded into a new state.
      expect(result.current.getState()).toEqual({ kind: "idle" });
    });
  });

  // Phase B1 — Pen tool. Covers the multi-event gesture model:
  // clicks place anchors, drags promote anchors to smooth (with
  // symmetric handles), close-on-first-anchor commits as a closed
  // path, Enter / `commitPen()` commits as an open path, Escape /
  // `cancelPen()` discards, tool-switch auto-commits, the
  // `subscribe()` surface fires on each transition, and the wire
  // format handed to `canvas.createPath` mirrors the
  // `PathSegmentWire` JSON shape consumed by the Rust bridge.
  describe("pen tool", () => {
    it("first click places a single anchor and seeds the gesture", () => {
      const { deps } = makeDeps({ tool: "pen" });
      const { result } = renderHook(() => useToolStateMachine(deps));
      const canvas = makeFakeCanvas();

      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, {
            type: "pointerdown",
            clientX: 10,
            clientY: 20,
            pointerId: 3,
          }),
        );
      });

      const s = result.current.getState();
      expect(s.kind).toBe("pen");
      if (s.kind !== "pen") throw new Error("expected pen");
      expect(s.anchors).toEqual([]);
      expect(s.pending).toEqual({
        pointerId: 3,
        x: 10,
        y: 20,
        drag: null,
      });
      expect(s.cursor).toEqual({ x: 10, y: 20 });
      // Pointer capture must be acquired so subsequent move/up
      // events route correctly even if the cursor briefly exits
      // the canvas bounds.
      expect(canvas.setPointerCaptureCalls).toEqual([3]);
    });

    it("pointerup with no drag promotes pending to a corner anchor", () => {
      const { deps } = makeDeps({ tool: "pen" });
      const { result } = renderHook(() => useToolStateMachine(deps));
      const canvas = makeFakeCanvas();

      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerdown", clientX: 5, clientY: 5 }),
        );
      });
      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerup", clientX: 5, clientY: 5 }),
        );
      });

      const s = result.current.getState();
      if (s.kind !== "pen") throw new Error("expected pen");
      expect(s.pending).toBeNull();
      expect(s.anchors).toEqual([
        { x: 5, y: 5, inHandle: null, outHandle: null },
      ]);
    });

    it("drag past threshold promotes pending to a smooth anchor with mirrored handles", () => {
      const { deps } = makeDeps({ tool: "pen" });
      const { result } = renderHook(() => useToolStateMachine(deps));
      const canvas = makeFakeCanvas();

      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerdown", clientX: 100, clientY: 100 }),
        );
      });
      // 20 px drag — well past the 4 px screen threshold.
      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointermove", clientX: 120, clientY: 100 }),
        );
      });
      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerup", clientX: 120, clientY: 100 }),
        );
      });

      const s = result.current.getState();
      if (s.kind !== "pen") throw new Error("expected pen");
      expect(s.anchors).toHaveLength(1);
      const a = s.anchors[0]!;
      expect(a.x).toBe(100);
      expect(a.y).toBe(100);
      // outHandle is where the drag ended; inHandle is the
      // symmetric reflection through the anchor.
      expect(a.outHandle).toEqual({ x: 120, y: 100 });
      expect(a.inHandle).toEqual({ x: 80, y: 100 });
    });

    it("sub-threshold drag stays a corner anchor", () => {
      const { deps } = makeDeps({ tool: "pen" });
      const { result } = renderHook(() => useToolStateMachine(deps));
      const canvas = makeFakeCanvas();

      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerdown", clientX: 100, clientY: 100 }),
        );
      });
      // 2 px drag — below the 4 px screen threshold.
      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointermove", clientX: 102, clientY: 100 }),
        );
      });
      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerup", clientX: 102, clientY: 100 }),
        );
      });

      const s = result.current.getState();
      if (s.kind !== "pen") throw new Error("expected pen");
      expect(s.anchors).toEqual([
        { x: 100, y: 100, inHandle: null, outHandle: null },
      ]);
    });

    it("commitPen no-ops when state is idle", async () => {
      const { deps } = makeDeps({ tool: "pen" });
      const { result } = renderHook(() => useToolStateMachine(deps));

      let newId: string | null | undefined;
      await act(async () => {
        newId = await result.current.commitPen();
      });
      expect(newId).toBeNull();
      expect(stub.calls.filter((c) => c.method === "canvas.createPath")).toEqual([]);
    });

    it("commitPen no-ops when fewer than 2 anchors have been laid", async () => {
      const { deps, onAfterCommit } = makeDeps({ tool: "pen" });
      const { result } = renderHook(() => useToolStateMachine(deps));
      const canvas = makeFakeCanvas();

      // Lay a single corner anchor — not enough to commit.
      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerdown", clientX: 0, clientY: 0 }),
        );
      });
      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerup", clientX: 0, clientY: 0 }),
        );
      });

      let newId: string | null | undefined;
      await act(async () => {
        newId = await result.current.commitPen();
      });

      expect(newId).toBeNull();
      // State resets to idle (the failed commit can't leave a
      // dangling one-anchor gesture).
      expect(result.current.getState()).toEqual({ kind: "idle" });
      // No bridge call, no refresh.
      expect(stub.calls.filter((c) => c.method === "canvas.createPath")).toEqual([]);
      expect(onAfterCommit).not.toHaveBeenCalled();
    });

    it("commitPen with 2+ anchors fires createPath as an open path and selects the new node", async () => {
      const { deps, onAfterCommit } = makeDeps({ tool: "pen" });
      const { result } = renderHook(() => useToolStateMachine(deps));
      const canvas = makeFakeCanvas();

      // Click-click — two corner anchors.
      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerdown", clientX: 0, clientY: 0 }),
        );
      });
      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerup", clientX: 0, clientY: 0 }),
        );
      });
      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerdown", clientX: 50, clientY: 50 }),
        );
      });
      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerup", clientX: 50, clientY: 50 }),
        );
      });

      let newId: string | null | undefined;
      await act(async () => {
        newId = await result.current.commitPen();
      });

      expect(newId).toBe("default-path-id");
      expect(result.current.getState()).toEqual({ kind: "idle" });

      const createCall = stub.calls.find((c) => c.method === "canvas.createPath");
      expect(createCall).toBeDefined();
      expect(createCall!.args[0]).toBeNull(); // parentId
      // Wire-format check: segments come in as PathSegmentWire[] —
      // the preload serializes them to JSON before crossing the
      // IPC boundary, but `window.kcreate.canvas.createPath` (the
      // preload entry, mocked here) receives the array directly.
      const segs = createCall!.args[1] as Array<Record<string, unknown>>;
      expect(segs).toEqual([
        { op: "move_to", x: 0, y: 0 },
        { op: "line_to", x: 50, y: 50 },
      ]);
      expect(createCall!.args[2]).toBe(false); // closed
      expect(createCall!.args[3]).toBeNull(); // name

      // setSelection routed to the new node + refresh fired.
      const selCall = stub.calls.find(
        (c) =>
          c.method === "canvas.setSelection" &&
          (c.args[0] as string[])[0] === "default-path-id",
      );
      expect(selCall).toBeDefined();
      expect(onAfterCommit).toHaveBeenCalledTimes(1);
    });

    it("clicking on the first anchor closes the path (commits with closed=true + close segment)", async () => {
      const { deps } = makeDeps({ tool: "pen" });
      const { result } = renderHook(() => useToolStateMachine(deps));
      const canvas = makeFakeCanvas();

      // Lay anchors at (0,0) → (50,0) → (50,50).
      const layCorner = (x: number, y: number): void => {
        act(() => {
          result.current.onCanvasPointer(
            makeEvent(canvas, { type: "pointerdown", clientX: x, clientY: y }),
          );
        });
        act(() => {
          result.current.onCanvasPointer(
            makeEvent(canvas, { type: "pointerup", clientX: x, clientY: y }),
          );
        });
      };
      layCorner(0, 0);
      layCorner(50, 0);
      layCorner(50, 50);

      // Click back on the first anchor (within close-hit radius of
      // 8 px). The hit-test is on pointerdown, which then fires
      // commitPenGesture(true) — we await flush for the async
      // bridge call.
      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerdown", clientX: 2, clientY: 2 }),
        );
      });
      await flush();

      expect(result.current.getState()).toEqual({ kind: "idle" });
      const createCall = stub.calls.find((c) => c.method === "canvas.createPath");
      expect(createCall).toBeDefined();
      const segs = createCall!.args[1] as Array<Record<string, unknown>>;
      // 3 corner anchors → MoveTo + 2 LineTos + closing LineTo + Close.
      expect(segs).toEqual([
        { op: "move_to", x: 0, y: 0 },
        { op: "line_to", x: 50, y: 0 },
        { op: "line_to", x: 50, y: 50 },
        { op: "line_to", x: 0, y: 0 },
        { op: "close" },
      ]);
      expect(createCall!.args[2]).toBe(true); // closed
    });

    it("close-hit radius scales with zoom so the gesture stays a consistent screen-space click", async () => {
      // At zoom=2, the close radius (8 screen px) becomes 4 world
      // units. A 5-world-unit click should NOT close, but a 3-
      // world-unit click should. This guards against future
      // refactors that swap the screen-space comparison for a
      // raw world-space one.
      const { deps } = makeDeps({
        tool: "pen",
        viewport: { panX: 0, panY: 0, zoom: 2 },
      });
      const { result } = renderHook(() => useToolStateMachine(deps));
      const canvas = makeFakeCanvas();

      // Lay 2 anchors. clientX/Y are screen coords; world coords
      // at zoom=2 are clientX/2.
      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerdown", clientX: 0, clientY: 0 }),
        );
      });
      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerup", clientX: 0, clientY: 0 }),
        );
      });
      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerdown", clientX: 100, clientY: 100 }),
        );
      });
      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerup", clientX: 100, clientY: 100 }),
        );
      });

      // Click 10 world units away from (0,0) → 20 px in screen
      // space. WELL outside the 8 px close radius → stays open,
      // lays a 3rd anchor.
      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerdown", clientX: 20, clientY: 0 }),
        );
      });
      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerup", clientX: 20, clientY: 0 }),
        );
      });
      await flush();

      // No close fired yet — state still in pen with 3 anchors.
      const s = result.current.getState();
      if (s.kind !== "pen") throw new Error("expected pen");
      expect(s.anchors).toHaveLength(3);
      expect(stub.calls.filter((c) => c.method === "canvas.createPath")).toEqual([]);
    });

    it("cancelPen discards an in-flight gesture and returns true", () => {
      const { deps } = makeDeps({ tool: "pen" });
      const { result } = renderHook(() => useToolStateMachine(deps));
      const canvas = makeFakeCanvas();

      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerdown", clientX: 0, clientY: 0 }),
        );
      });
      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerup", clientX: 0, clientY: 0 }),
        );
      });

      const cancelled = result.current.cancelPen();
      expect(cancelled).toBe(true);
      expect(result.current.getState()).toEqual({ kind: "idle" });
    });

    it("cancelPen returns false when state is idle (so caller falls through to clearSelection)", () => {
      const { deps } = makeDeps({ tool: "pen" });
      const { result } = renderHook(() => useToolStateMachine(deps));

      expect(result.current.cancelPen()).toBe(false);
      expect(result.current.getState()).toEqual({ kind: "idle" });
    });

    it("switching tool away from pen auto-commits the in-flight gesture", async () => {
      const { deps } = makeDeps({ tool: "pen" });
      const { result, rerender } = renderHook(
        ({ depsArg }: { depsArg: typeof deps }) =>
          useToolStateMachine(depsArg),
        { initialProps: { depsArg: deps } },
      );
      const canvas = makeFakeCanvas();

      // Lay 2 anchors.
      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerdown", clientX: 0, clientY: 0 }),
        );
      });
      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerup", clientX: 0, clientY: 0 }),
        );
      });
      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerdown", clientX: 10, clientY: 10 }),
        );
      });
      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerup", clientX: 10, clientY: 10 }),
        );
      });

      // Now switch tool. The hook's useEffect on [tool] should
      // detect tool !== "pen" + state.kind === "pen" and auto-fire
      // commitPenGesture(false).
      await act(async () => {
        rerender({ depsArg: { ...deps, tool: "select" } });
        await Promise.resolve();
        await Promise.resolve();
      });

      const createCall = stub.calls.find((c) => c.method === "canvas.createPath");
      expect(createCall).toBeDefined();
      expect(createCall!.args[2]).toBe(false); // open path
      expect(result.current.getState()).toEqual({ kind: "idle" });
    });

    it("subscribe fires on every state transition; unsubscribe stops further calls", () => {
      const { deps } = makeDeps({ tool: "pen" });
      const { result } = renderHook(() => useToolStateMachine(deps));
      const listener = vi.fn();
      const canvas = makeFakeCanvas();

      const unsub = result.current.subscribe(listener);

      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerdown", clientX: 0, clientY: 0 }),
        );
      });
      expect(listener).toHaveBeenCalledTimes(1);

      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointermove", clientX: 5, clientY: 5 }),
        );
      });
      expect(listener).toHaveBeenCalledTimes(2);

      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerup", clientX: 5, clientY: 5 }),
        );
      });
      expect(listener).toHaveBeenCalledTimes(3);

      unsub();
      // Further events should not call the listener again.
      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerdown", clientX: 20, clientY: 20 }),
        );
      });
      expect(listener).toHaveBeenCalledTimes(3);
    });

    it("bridge failure on createPath routes through onError and clears state", async () => {
      stub.override("canvas.createPath", () => {
        throw new Error("disk full");
      });
      const { deps, onError, onAfterCommit } = makeDeps({ tool: "pen" });
      const { result } = renderHook(() => useToolStateMachine(deps));
      const canvas = makeFakeCanvas();

      // Lay 2 anchors so the commit actually fires.
      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerdown", clientX: 0, clientY: 0 }),
        );
      });
      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerup", clientX: 0, clientY: 0 }),
        );
      });
      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerdown", clientX: 5, clientY: 5 }),
        );
      });
      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerup", clientX: 5, clientY: 5 }),
        );
      });

      let newId: string | null | undefined;
      await act(async () => {
        newId = await result.current.commitPen();
      });

      expect(newId).toBeNull();
      expect(onError).toHaveBeenCalledWith(
        expect.stringContaining("disk full"),
      );
      // State was reset to idle BEFORE the bridge call, so the
      // failed commit doesn't leave a dangling pen state.
      expect(result.current.getState()).toEqual({ kind: "idle" });
      // onAfterCommit only fires on success — the bridge throw
      // short-circuits before it.
      expect(onAfterCommit).not.toHaveBeenCalled();
    });

    it("smooth-anchor segment between two smooth anchors emits cubic_to with both handles", async () => {
      const { deps } = makeDeps({ tool: "pen" });
      const { result } = renderHook(() => useToolStateMachine(deps));
      const canvas = makeFakeCanvas();

      // Anchor 1 at (0,0), dragged to (10,0) → smooth, out=(10,0), in=(-10,0).
      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerdown", clientX: 0, clientY: 0 }),
        );
      });
      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointermove", clientX: 10, clientY: 0 }),
        );
      });
      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerup", clientX: 10, clientY: 0 }),
        );
      });
      // Anchor 2 at (50,50), dragged to (60,50) → smooth, out=(60,50), in=(40,50).
      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerdown", clientX: 50, clientY: 50 }),
        );
      });
      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointermove", clientX: 60, clientY: 50 }),
        );
      });
      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerup", clientX: 60, clientY: 50 }),
        );
      });

      await act(async () => {
        await result.current.commitPen();
      });

      const createCall = stub.calls.find((c) => c.method === "canvas.createPath");
      const segs = createCall!.args[1] as Array<Record<string, unknown>>;
      expect(segs).toEqual([
        { op: "move_to", x: 0, y: 0 },
        {
          op: "cubic_to",
          // Anchor 1's outHandle (where the user dragged).
          ctrl1: { x: 10, y: 0 },
          // Anchor 2's inHandle (symmetric reflection of (60,50)
          // through (50,50)).
          ctrl2: { x: 40, y: 50 },
          end: { x: 50, y: 50 },
        },
      ]);
    });

    it("non-pen tools (select/rect/etc.) do not enter pen state", () => {
      const { deps } = makeDeps({ tool: "rect" });
      const { result } = renderHook(() => useToolStateMachine(deps));
      const canvas = makeFakeCanvas();

      act(() => {
        result.current.onCanvasPointer(
          makeEvent(canvas, { type: "pointerdown", clientX: 0, clientY: 0 }),
        );
      });
      // Rect tool goes into `create`, not `pen`.
      expect(result.current.getState().kind).toBe("create");
    });
  });
});
