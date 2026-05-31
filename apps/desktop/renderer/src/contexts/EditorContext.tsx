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

const EditorContext = createContext<EditorContextValue | null>(null);

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

  const value = useMemo<EditorContextValue>(
    () => ({ state, actions, refs }),
    [state, actions, refs],
  );

  return (
    <EditorContext.Provider value={value}>{children}</EditorContext.Provider>
  );
}

/**
 * Internal helper — throws if called outside an `<EditorProvider>`.
 * All public hooks below use this so missing-provider errors fail
 * loudly at the call site instead of silently returning `null` and
 * crashing later when a property is destructured.
 */
function useEditorContextOrThrow(): EditorContextValue {
  const ctx = useContext(EditorContext);
  if (ctx === null) {
    throw new Error(
      "EditorContext consumer used outside <EditorProvider>. Wrap the " +
        "editor surface in <EditorProvider> before rendering components " +
        "that call useEditor / useEditorState / useEditorActions / useEditorRefs.",
    );
  }
  return ctx;
}

/** Full bundle — state + actions + refs. Convenient for EditorPage. */
export function useEditor(): EditorContextValue {
  return useEditorContextOrThrow();
}

/** State only — re-renders on any state change. */
export function useEditorState(): EditorState {
  return useEditorContextOrThrow().state;
}

/** Actions only — stable identity, never causes re-renders by itself. */
export function useEditorActions(): EditorActions {
  return useEditorContextOrThrow().actions;
}

/** Refs only — stable identity, never causes re-renders by itself. */
export function useEditorRefs(): EditorRefs {
  return useEditorContextOrThrow().refs;
}
