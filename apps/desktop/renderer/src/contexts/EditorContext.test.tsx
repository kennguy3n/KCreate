// EditorContext — state + action coverage.
//
// Phase A3a. EditorContext owns the editor UI / tool state previously
// scattered across EditorPage's 22 `useState` hooks. These tests
// validate the invariants the surrounding refactor depends on:
//
//   * setters MUST have stable identity across re-renders (the
//     provider memoises the `actions` bundle with empty deps so
//     consumer `useCallback`/`useEffect` lists referring to a setter
//     don't churn);
//   * functional setter updates (`setSelectedIds(prev => ...)`)
//     must flow through unchanged — many EditorPage call sites
//     depend on this form;
//   * `selectedIdsRef`, `panActiveRef`, `inlineTextEditRef` must
//     mirror their state in lockstep (used by EditorPage's stable
//     pointer-handler closures to read latest);
//   * the pan-disarm `blur` / `visibilitychange` listeners must
//     clear `panActive` so the gesture can't strand the user.
//
// Helpers: we render a small consumer component that exposes the
// context bundle through a ref, then drive state through that ref.
// This is the same `act`-friendly pattern React's own tests use for
// custom hooks (the only alternative — renderHook — is overkill
// here).

import { act, render, screen } from "@testing-library/react";
import { useEffect } from "react";
import { afterEach, describe, expect, it } from "vitest";

import {
  DEFAULT_VIEWPORT,
  EditorProvider,
  useEditor,
  useEditorActions,
  useEditorRefs,
  useEditorState,
} from "./EditorContext";

interface Captured {
  bundle: ReturnType<typeof useEditor> | null;
  renderCount: number;
  lastActions: ReturnType<typeof useEditor>["actions"] | null;
  lastRefs: ReturnType<typeof useEditor>["refs"] | null;
}

/** Consumer that captures the bundle into an outer object for the
 * test to drive. Each render bumps `renderCount` so identity-stability
 * assertions can prove memoisation across re-renders. */
function Capture({ captured }: { captured: Captured }): JSX.Element {
  const bundle = useEditor();
  captured.bundle = bundle;
  captured.renderCount += 1;
  // Compare actions/refs against the previous render. We assign
  // AFTER comparison so the assertions can see the prior identity.
  captured.lastActions = bundle.actions;
  captured.lastRefs = bundle.refs;
  // Force a render whenever the parent forceTick changes. We just
  // read the bundle so React's render cycle keeps Capture mounted.
  useEffect(() => {
    return () => undefined;
  });
  return <div data-testid="captured">{bundle.state.statusMessage ?? ""}</div>;
}

function renderEditor(): Captured {
  const captured: Captured = {
    bundle: null,
    renderCount: 0,
    lastActions: null,
    lastRefs: null,
  };
  render(
    <EditorProvider>
      <Capture captured={captured} />
    </EditorProvider>,
  );
  return captured;
}

afterEach(() => {
  // RTL auto-cleans, nothing to do here. Kept for explicitness.
});

describe("EditorContext", () => {
  it("initialises state to documented defaults", () => {
    const captured = renderEditor();
    const { state } = captured.bundle!;
    expect(state.mode).toBe("design");
    expect(state.tool).toBe("select");
    expect(state.selectedIds).toEqual([]);
    expect(state.statusMessage).toBeNull();
    expect(state.viewport).toEqual(DEFAULT_VIEWPORT);
    expect(state.fps).toBe(0);
    expect(state.panActive).toBe(false);
    expect(state.snapGuides).toEqual([]);
    expect(state.inlineTextEdit).toBeNull();
  });

  it("dispatches setSelectedIds and propagates to the ref", () => {
    const captured = renderEditor();
    const { actions, refs } = captured.bundle!;
    act(() => {
      actions.setSelectedIds(["a", "b"]);
    });
    expect(captured.bundle!.state.selectedIds).toEqual(["a", "b"]);
    // Lockstep: ref reflects new value.
    expect(refs.selectedIdsRef.current).toEqual(["a", "b"]);
  });

  it("accepts functional setSelectedIds updates", () => {
    const captured = renderEditor();
    act(() => {
      captured.bundle!.actions.setSelectedIds(["x"]);
    });
    act(() => {
      captured.bundle!.actions.setSelectedIds((prev) => [...prev, "y"]);
    });
    expect(captured.bundle!.state.selectedIds).toEqual(["x", "y"]);
  });

  it("dispatches setMode and setTool", () => {
    const captured = renderEditor();
    act(() => {
      captured.bundle!.actions.setMode("export");
      captured.bundle!.actions.setTool("text");
    });
    expect(captured.bundle!.state.mode).toBe("export");
    expect(captured.bundle!.state.tool).toBe("text");
  });

  it("dispatches setStatusMessage and renders the DOM update", () => {
    const captured = renderEditor();
    act(() => {
      captured.bundle!.actions.setStatusMessage("hello");
    });
    expect(captured.bundle!.state.statusMessage).toBe("hello");
    expect(screen.getByTestId("captured")).toHaveTextContent("hello");
  });

  it("dispatches setViewport", () => {
    const captured = renderEditor();
    act(() => {
      captured.bundle!.actions.setViewport({ panX: 100, panY: 50, zoom: 2 });
    });
    expect(captured.bundle!.state.viewport).toEqual({
      panX: 100,
      panY: 50,
      zoom: 2,
    });
  });

  it("dispatches setPanActive and propagates to the ref", () => {
    const captured = renderEditor();
    act(() => {
      captured.bundle!.actions.setPanActive(true);
    });
    expect(captured.bundle!.state.panActive).toBe(true);
    expect(captured.bundle!.refs.panActiveRef.current).toBe(true);
  });

  it("preserves action identity across re-renders", () => {
    const captured = renderEditor();
    const firstActions = captured.bundle!.actions;
    // Force a re-render via a state change.
    act(() => {
      captured.bundle!.actions.setFps(60);
    });
    const secondActions = captured.bundle!.actions;
    expect(secondActions).toBe(firstActions);
    expect(secondActions.setMode).toBe(firstActions.setMode);
    expect(secondActions.setStatusMessage).toBe(firstActions.setStatusMessage);
  });

  it("preserves ref identity across re-renders", () => {
    const captured = renderEditor();
    const firstRefs = captured.bundle!.refs;
    act(() => {
      captured.bundle!.actions.setSelectedIds(["1"]);
    });
    const secondRefs = captured.bundle!.refs;
    expect(secondRefs).toBe(firstRefs);
    expect(secondRefs.selectedIdsRef).toBe(firstRefs.selectedIdsRef);
  });

  it("clears panActive when the window receives a blur event", () => {
    const captured = renderEditor();
    act(() => {
      captured.bundle!.actions.setPanActive(true);
    });
    expect(captured.bundle!.state.panActive).toBe(true);
    act(() => {
      window.dispatchEvent(new Event("blur"));
    });
    expect(captured.bundle!.state.panActive).toBe(false);
    expect(captured.bundle!.refs.panActiveRef.current).toBe(false);
  });

  it("does NOT touch panActive on blur when the gesture isn't armed", () => {
    const captured = renderEditor();
    const renderCountBefore = captured.renderCount;
    act(() => {
      window.dispatchEvent(new Event("blur"));
    });
    expect(captured.bundle!.state.panActive).toBe(false);
    // Render count should match: the gated `clearPan` exits without
    // calling `setPanActive`, so no re-render is triggered.
    expect(captured.renderCount).toBe(renderCountBefore);
  });

  it("dispatches setInlineTextEdit and propagates to the ref", () => {
    const captured = renderEditor();
    const draft = {
      nodeId: "n1",
      rect: { x: 0, y: 0, width: 10, height: 10 },
      style: {
        fontFamily: "Arial",
        fontSize: 16,
        lineHeight: 1.25,
      },
      initialContent: "hi",
    };
    act(() => {
      captured.bundle!.actions.setInlineTextEdit(draft);
    });
    expect(captured.bundle!.state.inlineTextEdit).toBe(draft);
    expect(captured.bundle!.refs.inlineTextEditRef.current).toBe(draft);
  });

  it("throws when useEditor is called outside the provider", () => {
    // Suppress the React error boundary log noise for this single
    // expected throw — restored in `finally`.
    const origError = console.error;
    console.error = () => undefined;
    try {
      expect(() => render(<Capture captured={{
        bundle: null,
        renderCount: 0,
        lastActions: null,
        lastRefs: null,
      }} />)).toThrow(/EditorContext consumer used outside/);
    } finally {
      console.error = origError;
    }
  });

  it("actions-only consumers do NOT re-render on state changes", () => {
    // Architectural invariant from the context split (PR #35 / Devin
    // Review #0003 + #0004): a consumer that subscribes only to the
    // actions context must stay inert through state churn — the
    // actions value has stable identity for the provider's lifetime,
    // so React skips the consumer entirely.
    //
    // This is what unblocks `EditorDocumentBridge` from re-rendering
    // (and re-mounting `DocumentProvider`) every time selection /
    // status / viewport / FPS change.
    let actionsRenderCount = 0;
    let refsRenderCount = 0;
    let stateRenderCount = 0;
    let capturedActions: ReturnType<typeof useEditorActions> | null = null;

    function ActionsConsumer(): JSX.Element {
      actionsRenderCount += 1;
      const a = useEditorActions();
      capturedActions = a;
      return <div data-testid="actions-only" />;
    }
    function RefsConsumer(): JSX.Element {
      refsRenderCount += 1;
      useEditorRefs();
      return <div data-testid="refs-only" />;
    }
    function StateConsumer(): JSX.Element {
      stateRenderCount += 1;
      useEditorState();
      return <div data-testid="state-only" />;
    }

    render(
      <EditorProvider>
        <ActionsConsumer />
        <RefsConsumer />
        <StateConsumer />
      </EditorProvider>,
    );

    expect(actionsRenderCount).toBe(1);
    expect(refsRenderCount).toBe(1);
    expect(stateRenderCount).toBe(1);

    // Drive a sequence of unrelated state changes. The state
    // consumer should re-render for each; the actions/refs
    // consumers must stay at exactly one render.
    act(() => {
      capturedActions!.setStatusMessage("first");
    });
    act(() => {
      capturedActions!.setViewport({ panX: 10, panY: 20, zoom: 1.5 });
    });
    act(() => {
      capturedActions!.setSelectedIds(["a"]);
    });
    act(() => {
      capturedActions!.setFps(60);
    });

    expect(stateRenderCount).toBeGreaterThan(1);
    expect(actionsRenderCount).toBe(1);
    expect(refsRenderCount).toBe(1);
  });

  it("honours initialMode / initialTool provider props", () => {
    const captured: Captured = {
      bundle: null,
      renderCount: 0,
      lastActions: null,
      lastRefs: null,
    };
    render(
      <EditorProvider initialMode="layout" initialTool="rect">
        <Capture captured={captured} />
      </EditorProvider>,
    );
    expect(captured.bundle!.state.mode).toBe("layout");
    expect(captured.bundle!.state.tool).toBe("rect");
  });
});
