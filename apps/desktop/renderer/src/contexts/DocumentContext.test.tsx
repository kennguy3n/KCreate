// DocumentContext — state + refresh-action coverage.
//
// Phase A3a. DocumentContext owns the bridge-derived mirror state
// (nodes, artboards, components, docStatus, resourceLimits) that
// EditorPage used to own. These tests validate:
//
//   * setters MUST have stable identity across re-renders
//     (memoised `actions` bundle);
//   * `refreshTree` pulls ONLY the document tree (single-purpose by
//     design — status / artboards / components / selection refresh
//     live on their own actions so EditorPage can compose them in
//     the original pre-refactor sequencing);
//   * `refreshStatus` / `refreshArtboards` / `refreshComponents`
//     each pull their slice from the bridge in isolation;
//   * `nodesRef` / `artboardsRef` mirror their state in lockstep;
//   * a refresh failure routes through `onStatusError` (the
//     provider's error sink wired by EditorPage to
//     `EditorContext.setStatusMessage`);
//   * functional updates work on `setNodes` (used by EditorPage's
//     optimistic local updates).

import { act, render } from "@testing-library/react";
import { useEffect, useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type {
  ArtboardInfo,
  ComponentInfo,
  DocumentStatus,
  NodeInfo,
} from "../../../shared/scene";
import { installKcreateStub, kcreateStub } from "../../tests/helpers/kcreateStub";

import {
  DocumentProvider,
  useDocument,
  useDocumentActions,
  useDocumentRefs,
  useDocumentState,
} from "./DocumentContext";

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
  bounds: { x: 0, y: 0, width: 0, height: 0 },
  version: 1,
};

const SAMPLE_ARTBOARD: ArtboardInfo = {
  id: "ab1",
  name: "AB",
  x: 0,
  y: 0,
  width: 100,
  height: 100,
  pageId: "page-1",
};

const SAMPLE_COMPONENT: ComponentInfo = {
  id: "c1",
  name: "Comp",
  description: "",
  defaultVariantId: "v1",
  variants: [
    { id: "v1", name: "Default", properties: {} },
  ],
  createdAt: "2025-01-01T00:00:00Z",
  modifiedAt: "2025-01-01T00:00:00Z",
};

const SAMPLE_STATUS: DocumentStatus = {
  nodeCount: 4,
  canUndo: true,
  canRedo: false,
  undoDepth: 2,
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

  it("refreshTree pulls ONLY the document tree (no cascade)", async () => {
    kcreateStub().override("document.getDocumentTree", () => [SAMPLE_NODE]);
    kcreateStub().override("document.status", () => SAMPLE_STATUS);
    kcreateStub().override("artboard.list", () => [SAMPLE_ARTBOARD]);
    kcreateStub().override("component.list", () => [SAMPLE_COMPONENT]);

    const captured = renderDocument();
    await act(async () => {
      await captured.bundle!.actions.refreshTree();
    });

    // Only the tree slice should be touched — status / artboards /
    // components remain at their defaults. Composing those into a
    // full resync is the caller's job (see EditorPage.refreshTree).
    expect(captured.bundle!.state.nodes).toEqual([SAMPLE_NODE]);
    expect(captured.bundle!.state.docStatus).toBeNull();
    expect(captured.bundle!.state.artboards).toEqual([]);
    expect(captured.bundle!.state.components).toEqual([]);

    // Confirm only the tree method was invoked.
    const calls = kcreateStub().calls.map((c) => c.method);
    expect(calls).toContain("document.getDocumentTree");
    expect(calls).not.toContain("document.status");
    expect(calls).not.toContain("artboard.list");
    expect(calls).not.toContain("component.list");
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
    const firstCallArg = onStatusError.mock.calls[0]?.[0];
    expect(firstCallArg).toMatch(/status probe failed: boom/);
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

  it("actions-only consumers do NOT re-render on state changes", () => {
    // Architectural invariant from the context split (PR #35 / Devin
    // Review #0003 + #0004): a consumer that subscribes only to the
    // actions context stays inert through state churn — the actions
    // value has stable identity for the provider's lifetime, so
    // React skips the consumer entirely.
    //
    // Same proof point as the parallel test in EditorContext, but
    // for DocumentContext (the contexts are independent but share
    // the split-by-shape pattern).
    let actionsRenderCount = 0;
    let refsRenderCount = 0;
    let stateRenderCount = 0;
    let capturedActions: ReturnType<typeof useDocumentActions> | null = null;

    function ActionsConsumer(): JSX.Element {
      actionsRenderCount += 1;
      const a = useDocumentActions();
      capturedActions = a;
      return <div data-testid="doc-actions-only" />;
    }
    function RefsConsumer(): JSX.Element {
      refsRenderCount += 1;
      useDocumentRefs();
      return <div data-testid="doc-refs-only" />;
    }
    function StateConsumer(): JSX.Element {
      stateRenderCount += 1;
      useDocumentState();
      return <div data-testid="doc-state-only" />;
    }

    render(
      <DocumentProvider>
        <ActionsConsumer />
        <RefsConsumer />
        <StateConsumer />
      </DocumentProvider>,
    );

    expect(actionsRenderCount).toBe(1);
    expect(refsRenderCount).toBe(1);
    expect(stateRenderCount).toBe(1);

    act(() => {
      capturedActions!.setNodes([SAMPLE_NODE]);
    });
    act(() => {
      capturedActions!.setArtboards([SAMPLE_ARTBOARD]);
    });
    act(() => {
      capturedActions!.setComponents([SAMPLE_COMPONENT]);
    });
    act(() => {
      capturedActions!.setDocStatus(SAMPLE_STATUS);
    });

    expect(stateRenderCount).toBeGreaterThan(1);
    expect(actionsRenderCount).toBe(1);
    expect(refsRenderCount).toBe(1);
  });

  it("EMPTY_SCENE sentinel is deep-frozen", async () => {
    // Architectural invariant from PR #35 / Devin Review #0005:
    // the module-scoped `EMPTY_SCENE` is shared across ALL
    // DocumentProvider instances, so accidental mutation would
    // silently corrupt every consumer. The freeze converts the
    // latent foot-gun into a strict-mode `TypeError` at the
    // offending call site.
    const { EMPTY_SCENE } = await import("./DocumentContext");
    expect(Object.isFrozen(EMPTY_SCENE)).toBe(true);
    expect(Object.isFrozen(EMPTY_SCENE.clear_color)).toBe(true);
    expect(Object.isFrozen(EMPTY_SCENE.objects)).toBe(true);

    // Mutation attempts throw in strict mode (which vitest +
    // ES-modules run under). Use `as any` casts to bypass the
    // TypeScript surface, since the wire type still declares
    // mutable arrays — the freeze is runtime defense-in-depth at
    // the sentinel only.
    expect(() => {
      (EMPTY_SCENE.objects as unknown as unknown[]).push({});
    }).toThrow();
    expect(() => {
      (EMPTY_SCENE.clear_color as unknown as number[])[0] = 999;
    }).toThrow();
    expect(() => {
      (EMPTY_SCENE as { clear_color: unknown }).clear_color = [1, 1, 1, 1];
    }).toThrow();
  });

  it("useDocument() returns a memoised bundle (stable across pure re-renders)", () => {
    // Architectural invariant from PR #35 / Devin Review #0004:
    // the full-bundle hook must memoise against
    // `[state, actions, refs]` so its identity is preserved when
    // none of the underlying context values change. Without the
    // memo, any future consumer passing the bundle into a
    // `useEffect` dep array would re-fire on every parent render.
    let bumpParent: (() => void) | null = null;
    const seenBundles: Array<ReturnType<typeof useDocument>> = [];

    function Inner(): JSX.Element {
      const bundle = useDocument();
      seenBundles.push(bundle);
      return <div />;
    }

    function Parent(): JSX.Element {
      // Parent-local state — changes here force a re-render of
      // Parent (and therefore Inner) WITHOUT touching the provider's
      // context values. The bundle identity must survive this.
      const [, setTick] = useState(0);
      bumpParent = () => setTick((n) => n + 1);
      return <Inner />;
    }

    render(
      <DocumentProvider>
        <Parent />
      </DocumentProvider>,
    );

    expect(seenBundles.length).toBeGreaterThan(0);
    const first = seenBundles[seenBundles.length - 1];

    // Drive a parent-only re-render. The provider's context
    // values are untouched.
    act(() => {
      bumpParent!();
    });

    const second = seenBundles[seenBundles.length - 1];

    // Bundle identity is preserved because state / actions /
    // refs identities are all preserved by the provider.
    expect(second).toBe(first);
  });
});
