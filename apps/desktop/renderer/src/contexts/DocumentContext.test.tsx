// DocumentContext — state + refresh-action coverage.
//
// Phase A3a. DocumentContext owns the bridge-derived mirror state
// (nodes, artboards, components, docStatus, resourceLimits) that
// EditorPage used to own. These tests validate:
//
//   * setters MUST have stable identity across re-renders
//     (memoised `actions` bundle);
//   * `refreshTree` pulls `getDocumentTree` then sequences the
//     three sub-refreshers; setters surface the bridge return
//     values into state;
//   * `nodesRef` / `artboardsRef` mirror their state in lockstep;
//   * a refresh failure routes through `onStatusError` (the
//     provider's error sink wired by EditorPage to
//     `EditorContext.setStatusMessage`);
//   * functional updates work on `setNodes` (used by EditorPage's
//     optimistic local updates).

import { act, render } from "@testing-library/react";
import { useEffect } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type {
  ArtboardInfo,
  ComponentInfo,
  DocumentStatus,
  NodeInfo,
} from "../../../../shared/scene";
import { installKcreateStub, kcreateStub } from "../../tests/helpers/kcreateStub";

import { DocumentProvider, useDocument } from "./DocumentContext";

interface Captured {
  bundle: ReturnType<typeof useDocument> | null;
  renderCount: number;
}

function Capture({ captured }: { captured: Captured }): JSX.Element {
  const bundle = useDocument();
  captured.bundle = bundle;
  captured.renderCount += 1;
  useEffect(() => undefined, []);
  return <div />;
}

function renderDocument(opts: {
  initialArtboardPresets?: never;
  onStatusError?: (msg: string) => void;
} = {}): Captured {
  const captured: Captured = { bundle: null, renderCount: 0 };
  render(
    <DocumentProvider onStatusError={opts.onStatusError}>
      <Capture captured={captured} />
    </DocumentProvider>,
  );
  return captured;
}

const SAMPLE_NODE: NodeInfo = {
  id: "n1",
  parentId: null,
  nodeType: "Group",
  name: "Group",
  visible: true,
  locked: false,
  children: [],
};

const SAMPLE_ARTBOARD: ArtboardInfo = {
  id: "ab1",
  name: "AB",
  x: 0,
  y: 0,
  width: 100,
  height: 100,
};

const SAMPLE_COMPONENT: ComponentInfo = {
  id: "c1",
  name: "Comp",
  thumbnailPath: null,
  artboardId: "ab1",
};

const SAMPLE_STATUS: DocumentStatus = {
  projectId: "p1",
  unsaved: false,
  undoDepth: 0,
  redoDepth: 0,
};

beforeEach(() => {
  installKcreateStub();
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("DocumentContext", () => {
  it("initialises state to empty mirror values", () => {
    const captured = renderDocument();
    const { state } = captured.bundle!;
    expect(state.nodes).toEqual([]);
    expect(state.artboards).toEqual([]);
    expect(state.artboardPresets).toEqual([]);
    expect(state.components).toEqual([]);
    expect(state.docStatus).toBeNull();
    expect(state.resourceLimits).toBeNull();
    expect(state.scene.clear_color).toEqual([0.12, 0.12, 0.14, 1.0]);
    expect(state.scene.objects).toEqual([]);
  });

  it("preserves action identity across re-renders", async () => {
    const captured = renderDocument();
    const firstActions = captured.bundle!.actions;
    act(() => {
      captured.bundle!.actions.setNodes([SAMPLE_NODE]);
    });
    const secondActions = captured.bundle!.actions;
    // useMemo deps include the refresh callbacks; the refresh
    // callbacks themselves are memoised on `reportError`, which is
    // memoised with empty deps. So actions identity should be
    // preserved across pure state changes.
    expect(secondActions).toBe(firstActions);
  });

  it("preserves ref identity across re-renders", () => {
    const captured = renderDocument();
    const firstRefs = captured.bundle!.refs;
    act(() => {
      captured.bundle!.actions.setArtboards([SAMPLE_ARTBOARD]);
    });
    const secondRefs = captured.bundle!.refs;
    expect(secondRefs).toBe(firstRefs);
    expect(secondRefs.nodesRef).toBe(firstRefs.nodesRef);
    expect(secondRefs.artboardsRef).toBe(firstRefs.artboardsRef);
  });

  it("dispatches setNodes (direct and functional) and syncs nodesRef", () => {
    const captured = renderDocument();
    act(() => {
      captured.bundle!.actions.setNodes([SAMPLE_NODE]);
    });
    expect(captured.bundle!.state.nodes).toEqual([SAMPLE_NODE]);
    expect(captured.bundle!.refs.nodesRef.current).toEqual([SAMPLE_NODE]);

    const next: NodeInfo = { ...SAMPLE_NODE, id: "n2" };
    act(() => {
      captured.bundle!.actions.setNodes((prev) => [...prev, next]);
    });
    expect(captured.bundle!.state.nodes).toEqual([SAMPLE_NODE, next]);
    expect(captured.bundle!.refs.nodesRef.current).toEqual([
      SAMPLE_NODE,
      next,
    ]);
  });

  it("dispatches setArtboards and syncs artboardsRef", () => {
    const captured = renderDocument();
    act(() => {
      captured.bundle!.actions.setArtboards([SAMPLE_ARTBOARD]);
    });
    expect(captured.bundle!.state.artboards).toEqual([SAMPLE_ARTBOARD]);
    expect(captured.bundle!.refs.artboardsRef.current).toEqual([
      SAMPLE_ARTBOARD,
    ]);
  });

  it("refreshStatus pulls from the bridge and sets state", async () => {
    kcreateStub().override("document.status", () => SAMPLE_STATUS);
    const captured = renderDocument();
    await act(async () => {
      await captured.bundle!.actions.refreshStatus();
    });
    expect(captured.bundle!.state.docStatus).toEqual(SAMPLE_STATUS);
  });

  it("refreshArtboards pulls from the bridge and sets state", async () => {
    kcreateStub().override("artboard.list", () => [SAMPLE_ARTBOARD]);
    const captured = renderDocument();
    await act(async () => {
      await captured.bundle!.actions.refreshArtboards();
    });
    expect(captured.bundle!.state.artboards).toEqual([SAMPLE_ARTBOARD]);
  });

  it("refreshComponents pulls from the bridge and sets state", async () => {
    kcreateStub().override("component.list", () => [SAMPLE_COMPONENT]);
    const captured = renderDocument();
    await act(async () => {
      await captured.bundle!.actions.refreshComponents();
    });
    expect(captured.bundle!.state.components).toEqual([SAMPLE_COMPONENT]);
  });

  it("refreshTree sequences tree + status + artboards + components", async () => {
    kcreateStub().override("document.getDocumentTree", () => [SAMPLE_NODE]);
    kcreateStub().override("document.status", () => SAMPLE_STATUS);
    kcreateStub().override("artboard.list", () => [SAMPLE_ARTBOARD]);
    kcreateStub().override("component.list", () => [SAMPLE_COMPONENT]);

    const captured = renderDocument();
    await act(async () => {
      await captured.bundle!.actions.refreshTree();
    });

    expect(captured.bundle!.state.nodes).toEqual([SAMPLE_NODE]);
    expect(captured.bundle!.state.docStatus).toEqual(SAMPLE_STATUS);
    expect(captured.bundle!.state.artboards).toEqual([SAMPLE_ARTBOARD]);
    expect(captured.bundle!.state.components).toEqual([SAMPLE_COMPONENT]);

    // Confirm the right methods were invoked.
    const calls = kcreateStub().calls.map((c) => c.method);
    expect(calls).toContain("document.getDocumentTree");
    expect(calls).toContain("document.status");
    expect(calls).toContain("artboard.list");
    expect(calls).toContain("component.list");
  });

  it("routes refresh failures through onStatusError", async () => {
    const onStatusError = vi.fn();
    kcreateStub().override("document.status", () => {
      throw new Error("boom");
    });

    const captured = renderDocument({ onStatusError });
    await act(async () => {
      await captured.bundle!.actions.refreshStatus();
    });

    expect(onStatusError).toHaveBeenCalledTimes(1);
    expect(onStatusError.mock.calls[0][0]).toMatch(/status probe failed: boom/);
    expect(captured.bundle!.state.docStatus).toBeNull();
  });

  it("falls through silently when onStatusError is absent", async () => {
    kcreateStub().override("artboard.list", () => {
      throw new Error("nope");
    });
    const captured = renderDocument();
    await act(async () => {
      await captured.bundle!.actions.refreshArtboards();
    });
    expect(captured.bundle!.state.artboards).toEqual([]);
  });

  it("throws when useDocument is called outside the provider", () => {
    const origError = console.error;
    console.error = () => undefined;
    try {
      const captured: Captured = { bundle: null, renderCount: 0 };
      expect(() => render(<Capture captured={captured} />)).toThrow(
        /DocumentContext consumer used outside/,
      );
    } finally {
      console.error = origError;
    }
  });
});
