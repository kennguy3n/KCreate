/**
 * Editor UI / tool state context.
 *
 * Phase A3a — extracts the tool-level state that EditorPage used to
 * own as a sea of `useState` hooks. The goals:
 *
 * 1. Centralize ownership so future tools (Pen tool, Vector node
 *    editor, etc. — Phase B) can read/dispatch editor state without
 *    prop-drilling through `EditorPage`.
 * 2. Preserve the existing callback-stability invariants. Many of
 *    EditorPage's callbacks deliberately do NOT depend on `nodes`,
 *    `selectedIds`, etc. to avoid re-attaching window listeners (e.g.
 *    `useShortcuts`) on every state change. Those callbacks read
 *    current values through refs. We expose the same refs from this
 *    context so the pattern continues to work.
 * 3. Keep the same setter signatures (`setMode`, `setTool`, etc.) so
 *    EditorPage's body changes are minimal — just replace
 *    `useState` with `useEditor()` destructuring.
 *
 * The provider is mounted by `EditorPage` (one provider per editor
 * surface). All editor-internal components — TopBar, LeftPanel,
 * RightPanel, CanvasHost, etc. — continue to receive props from
 * `EditorPage`; the context is the host's own state, not a
 * replacement for the component props.
 */

import { createContext, useContext, useEffect, useMemo, useRef, useState } from "react";
import type { Dispatch, MutableRefObject, ReactNode, SetStateAction } from "react";

import type { ViewportState } from "../components/CanvasHost";
import type { EditorMode } from "../components/TopBar";
import type { SnapGuide, TextStyleWire } from "../../../shared/scene";

/**
 * Active drawing/selection tool. Re-exported from EditorPage's old
 * inline definition. The selected tool drives the canvas cursor and
 * the pointer-handler dispatch.
 */
export type ToolId = "select" | "rect" | "ellipse" | "line" | "text";

/**
 * State for the in-canvas text editor that mounts as an absolutely
 * positioned `<contenteditable>` over a hit-tested `TextLayer` when
 * the user double-clicks. Bounds are captured in screen space at
 * double-click time so the editor lines up with the canvas glyphs
 * even if pan/zoom change while the user is typing.
 *
 * The identity of an `InlineTextEditState` object is meaningful: a
 * commit's `finally` block compares the captured draft against
 * `inlineTextEditRef.current` and only nulls state when the
 * identities match (race-safe dismissal — see EditorPage's
 * `commitInlineTextEdit`).
 */
export interface InlineTextEditState {
  nodeId: string;
  rect: { x: number; y: number; width: number; height: number };
  style: TextStyleWire;
  initialContent: string;
}

/**
 * Default viewport — module-scoped so the reference is stable
 * across re-renders. Used as `useState` initial value, never
 * mutated. Matches EditorPage's prior local constant.
 */
export const DEFAULT_VIEWPORT: ViewportState = { panX: 0, panY: 0, zoom: 1 };

/**
 * Public state surface of the editor context. These are the values
 * a consumer reads through `useEditor()`. Field semantics match the
 * `useState` hooks they replace in EditorPage.
 */
export interface EditorState {
  mode: EditorMode;
  tool: ToolId;
  selectedIds: string[];
  statusMessage: string | null;
  viewport: ViewportState;
  fps: number;
  panActive: boolean;
  snapGuides: SnapGuide[];
  inlineTextEdit: InlineTextEditState | null;
}

/**
 * Public setter surface. We keep the standard React setter
 * signatures (`Dispatch<SetStateAction<T>>`) so EditorPage's
 * existing call sites — including functional updates like
 * `setSelectedIds((prev) => prev.filter(...))` — keep working
 * unchanged.
 */
export interface EditorActions {
  setMode: Dispatch<SetStateAction<EditorMode>>;
  setTool: Dispatch<SetStateAction<ToolId>>;
  setSelectedIds: Dispatch<SetStateAction<string[]>>;
  setStatusMessage: Dispatch<SetStateAction<string | null>>;
  setViewport: Dispatch<SetStateAction<ViewportState>>;
  setFps: Dispatch<SetStateAction<number>>;
  setPanActive: Dispatch<SetStateAction<boolean>>;
  setSnapGuides: Dispatch<SetStateAction<SnapGuide[]>>;
  setInlineTextEdit: Dispatch<SetStateAction<InlineTextEditState | null>>;
}

/**
 * Read-latest refs for callbacks that must stay reference-stable.
 *
 * `selectedIdsRef` exists because `handleCopy` / `handlePaste` /
 * pointer handlers in EditorPage read the current selection but
 * MUST NOT re-create their closures when selection changes (doing
 * so detaches+reattaches window listeners and cancels in-flight
 * drags). The ref provides a "read latest" escape hatch without
 * depending on the state directly.
 *
 * `panActiveRef` mirrors `panActive` so the stable pointer-handler
 * closure can observe the latest hold-to-pan state without
 * depending on it.
 *
 * `inlineTextEditRef` is the identity ref documented above on
 * `InlineTextEditState`. A new editor mounted before a prior
 * commit's `finally` runs would otherwise flash-close.
 */
export interface EditorRefs {
  selectedIdsRef: MutableRefObject<string[]>;
  panActiveRef: MutableRefObject<boolean>;
  inlineTextEditRef: MutableRefObject<InlineTextEditState | null>;
}

interface EditorContextValue {
  state: EditorState;
  actions: EditorActions;
  refs: EditorRefs;
}

/**
 * Why three distinct contexts instead of one bundle?
 *
 * React's context API only supports whole-value subscription:
 * `useContext(ctx)` re-renders the consumer whenever the provider's
 * `value` prop is referentially different. A naive `{ state, actions,
 * refs }` bundle means EVERY consumer re-renders on EVERY state
 * change — even consumers that only read `actions` (which are
 * intentionally stable for the provider's whole lifetime).
 *
 * `EditorDocumentBridge` is the canonical victim: it only needs
 * `setStatusMessage` (an action) but would re-render on every
 * status / viewport / FPS / selection change otherwise. The bridge
 * sits between `EditorProvider` and `DocumentProvider`, so each
 * spurious re-render also re-mounts `DocumentProvider`'s subtree.
 *
 * Splitting into `EditorStateContext` / `EditorActionsContext` /
 * `EditorRefsContext` lets each consumer subscribe only to what it
 * actually reads. The Provider builds all three values once;
 * `actions` and `refs` get stable identity (empty-deps `useMemo`),
 * so consumers of those contexts NEVER re-render from this
 * provider. Consumers that mix concerns can still call
 * `useEditor()` to get the merged bundle — that hook subscribes to
 * all three, which is the original behaviour.
 */
const EditorStateContext = createContext<EditorState | null>(null);
const EditorActionsContext = createContext<EditorActions | null>(null);
const EditorRefsContext = createContext<EditorRefs | null>(null);

export interface EditorProviderProps {
  /**
   * Initial mode. Defaults to `"design"`. Tests can override to
   * skip the prior `useState` initializer.
   */
  initialMode?: EditorMode;
  /**
   * Initial tool. Defaults to `"select"`.
   */
  initialTool?: ToolId;
  children: ReactNode;
}

/**
 * Provider that owns the editor UI / tool state. Mount once inside
 * the editor surface (today: `EditorPage`). Consumers read via
 * `useEditor()` (full bundle) or the targeted hooks below.
 */
export function EditorProvider({
  initialMode = "design",
  initialTool = "select",
  children,
}: EditorProviderProps): JSX.Element {
  const [mode, setMode] = useState<EditorMode>(initialMode);
  const [tool, setTool] = useState<ToolId>(initialTool);
  const [selectedIds, setSelectedIds] = useState<string[]>([]);
  const [statusMessage, setStatusMessage] = useState<string | null>(null);
  const [viewport, setViewport] = useState<ViewportState>(DEFAULT_VIEWPORT);
  const [fps, setFps] = useState<number>(0);
  const [panActive, setPanActive] = useState<boolean>(false);
  const [snapGuides, setSnapGuides] = useState<SnapGuide[]>([]);
  const [inlineTextEdit, setInlineTextEdit] =
    useState<InlineTextEditState | null>(null);

  // Refs mirror state so stable callbacks can read the latest
  // value without depending on it. See `EditorRefs` for rationale.
  const selectedIdsRef = useRef<string[]>(selectedIds);
  useEffect(() => {
    selectedIdsRef.current = selectedIds;
  }, [selectedIds]);

  const panActiveRef = useRef<boolean>(panActive);
  useEffect(() => {
    panActiveRef.current = panActive;
  }, [panActive]);

  // Defense-in-depth disarm for the hold-to-pan gesture. The gesture
  // is normally cleared by the bound keyup, but a user can lose
  // Space-as-keyup entirely if focus leaves the document mid-hold
  // (alt-tab to another app, click into a system dialog, drag a
  // file from outside the window, etc.). The keyup never reaches
  // us in those cases, so we listen for window `blur` and the
  // document's `visibilitychange` (fires on tab-switch and OS
  // lock-screen) and clear the gesture proactively. Gated on
  // `panActiveRef.current` so we don't fire a no-op state update on
  // every focus change.
  useEffect(() => {
    const clearPan = (): void => {
      if (panActiveRef.current) {
        setPanActive(false);
      }
    };
    const onVisibilityChange = (): void => {
      if (typeof document !== "undefined" && document.hidden) {
        clearPan();
      }
    };
    window.addEventListener("blur", clearPan);
    if (typeof document !== "undefined") {
      document.addEventListener("visibilitychange", onVisibilityChange);
    }
    return () => {
      window.removeEventListener("blur", clearPan);
      if (typeof document !== "undefined") {
        document.removeEventListener("visibilitychange", onVisibilityChange);
      }
    };
  }, []);

  const inlineTextEditRef = useRef<InlineTextEditState | null>(inlineTextEdit);
  useEffect(() => {
    inlineTextEditRef.current = inlineTextEdit;
  }, [inlineTextEdit]);

  // `actions` is memoised against an empty dep set — React
  // guarantees `useState` setters are stable, so we can build the
  // object once and re-use it for the provider's lifetime. This
  // matters because consumers may pass action functions into
  // `useCallback`/`useEffect` deps, and a fresh object every render
  // would defeat memoisation downstream.
  const actions = useMemo<EditorActions>(
    () => ({
      setMode,
      setTool,
      setSelectedIds,
      setStatusMessage,
      setViewport,
      setFps,
      setPanActive,
      setSnapGuides,
      setInlineTextEdit,
    }),
    [],
  );

  // `refs` is similarly stable: ref objects don't change identity
  // across renders, so memoising against an empty dep set is safe.
  const refs = useMemo<EditorRefs>(
    () => ({
      selectedIdsRef,
      panActiveRef,
      inlineTextEditRef,
    }),
    [],
  );

  // `state` changes on every state mutation (intentional — that's
  // what triggers consumer re-renders). We still wrap it in
  // `useMemo` keyed on its constituent fields so the object
  // identity is preserved across re-renders where no state changed
  // (e.g. when a parent above the provider re-renders).
  const state = useMemo<EditorState>(
    () => ({
      mode,
      tool,
      selectedIds,
      statusMessage,
      viewport,
      fps,
      panActive,
      snapGuides,
      inlineTextEdit,
    }),
    [
      mode,
      tool,
      selectedIds,
      statusMessage,
      viewport,
      fps,
      panActive,
      snapGuides,
      inlineTextEdit,
    ],
  );

  // Provider nests are equivalent semantically to a single
  // multi-value `<Context.Provider>` chain; React's reconciler
  // optimises away no-op subtrees for stable values, so the actions
  // and refs providers never invalidate their consumers.
  return (
    <EditorStateContext.Provider value={state}>
      <EditorActionsContext.Provider value={actions}>
        <EditorRefsContext.Provider value={refs}>
          {children}
        </EditorRefsContext.Provider>
      </EditorActionsContext.Provider>
    </EditorStateContext.Provider>
  );
}

/**
 * Internal helper — throws if the requested context is missing.
 * Used by every public hook so missing-provider errors fail loudly
 * at the call site instead of silently returning `null` and
 * crashing later when a property is destructured.
 */
function requireEditorContext<T>(
  ctxValue: T | null,
  consumerName: string,
): T {
  if (ctxValue === null) {
    throw new Error(
      `EditorContext consumer used outside <EditorProvider>. Wrap the ` +
        `editor surface in <EditorProvider> before rendering components ` +
        `that call ${consumerName}.`,
    );
  }
  return ctxValue;
}

/**
 * Full bundle — state + actions + refs. Convenient for EditorPage
 * (which needs everything) and tests that want to assert on the
 * whole value at once.
 *
 * NOTE: subscribers to this hook re-render on every state change.
 * Components that only need a subset MUST use `useEditorState` /
 * `useEditorActions` / `useEditorRefs` to opt out of unrelated
 * re-renders. The `EditorDocumentBridge` host wrapper is the
 * canonical example.
 */
export function useEditor(): EditorContextValue {
  const state = requireEditorContext(
    useContext(EditorStateContext),
    "useEditor",
  );
  const actions = requireEditorContext(
    useContext(EditorActionsContext),
    "useEditor",
  );
  const refs = requireEditorContext(
    useContext(EditorRefsContext),
    "useEditor",
  );
  return { state, actions, refs };
}

/** State only — re-renders on any state change. */
export function useEditorState(): EditorState {
  return requireEditorContext(
    useContext(EditorStateContext),
    "useEditorState",
  );
}

/**
 * Actions only — stable identity for the provider's lifetime, so
 * consumers of this hook never re-render from state changes here.
 */
export function useEditorActions(): EditorActions {
  return requireEditorContext(
    useContext(EditorActionsContext),
    "useEditorActions",
  );
}

/**
 * Refs only — stable identity for the provider's lifetime, so
 * consumers of this hook never re-render from state changes here.
 */
export function useEditorRefs(): EditorRefs {
  return requireEditorContext(
    useContext(EditorRefsContext),
    "useEditorRefs",
  );
}
