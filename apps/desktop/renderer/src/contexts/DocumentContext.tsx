/**
 * Document / project mirror state context.
 *
 * Phase A3a — extracts the server-mirror state that EditorPage used
 * to own as `useState` hooks tied to bridge probes. These are read
 * by many panels (LeftPanel for the layer tree, RightPanel for the
 * selected node, the artboard tab, the component asset list, the
 * low-resource banner). Lifting them into a context lets future
 * components consume them without prop drilling.
 *
 * This context owns the **DATA**; refresh callbacks are exposed
 * via `actions` so a consumer (or EditorPage) can re-pull from the
 * bridge. Error reporting flows out through an `onStatusError`
 * prop on the provider so the refresh helpers can surface failures
 * without depending on `EditorContext` directly (avoids a circular
 * import — `EditorContext.setStatusMessage` is reached via the
 * provider callback instead).
 *
 * `scene` lives here because conceptually it's a server-derived
 * sample of the document graph. Today it's the empty sentinel; in
 * Phase 1 we'll swap it for a push-based subscription. Having it
 * already routed through the context means that swap doesn't
 * require touching every consumer.
 */

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import type { Dispatch, MutableRefObject, ReactNode, SetStateAction } from "react";

import type {
  ArtboardInfo,
  ArtboardPreset,
  ComponentInfo,
  DocumentStatus,
  NodeInfo,
  ResourceLimits,
  Scene,
} from "../../../shared/scene";

/**
 * Empty scene used while we haven't yet pulled one from the
 * bridge. Module-scoped so the reference is stable across re-renders.
 *
 * Deep-frozen at runtime so accidental mutation by a future consumer
 * (e.g. Phase 1's push-subscription path doing `scene.objects.push(...)`
 * on the sentinel before swapping in a real Scene) becomes a strict-mode
 * `TypeError` instead of silently corrupting every other provider
 * instance that shares this module-scoped reference. The `Scene` wire
 * type still declares mutable arrays — the freeze is defense-in-depth
 * at the sentinel only; real bridge-pulled scenes remain mutable as
 * before.
 */
const EMPTY_SCENE: Scene = (() => {
  const sentinel: Scene = {
    clear_color: [0.12, 0.12, 0.14, 1.0],
    objects: [],
  };
  Object.freeze(sentinel.clear_color);
  Object.freeze(sentinel.objects);
  Object.freeze(sentinel);
  return sentinel;
})();

/** Re-export so EditorPage can keep using the same identity. */
export { EMPTY_SCENE };

/** Local helper — mirrors the duplicated `errorMessage` used by
 * EditorPage and other renderer modules. Inlined here to avoid an
 * unrelated cross-file extraction inside the A3a refactor; a
 * dedicated cleanup is a sensible follow-up. */
function errorMessage(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/**
 * Public state surface. Field semantics match the `useState` hooks
 * they replace in EditorPage.
 */
export interface DocumentState {
  nodes: NodeInfo[];
  artboards: ArtboardInfo[];
  artboardPresets: ArtboardPreset[];
  components: ComponentInfo[];
  docStatus: DocumentStatus | null;
  resourceLimits: ResourceLimits | null;
  scene: Scene;
}

/**
 * Public action surface. Setters keep the `Dispatch<SetStateAction<T>>`
 * shape so existing functional-update patterns work unchanged.
 * Refresh callbacks have stable identity (memoised with empty
 * deps) so they can be safely listed in consumer `useEffect` /
 * `useCallback` deps without forcing re-runs.
 */
export interface DocumentActions {
  setNodes: Dispatch<SetStateAction<NodeInfo[]>>;
  setArtboards: Dispatch<SetStateAction<ArtboardInfo[]>>;
  setArtboardPresets: Dispatch<SetStateAction<ArtboardPreset[]>>;
  setComponents: Dispatch<SetStateAction<ComponentInfo[]>>;
  setDocStatus: Dispatch<SetStateAction<DocumentStatus | null>>;
  setResourceLimits: Dispatch<SetStateAction<ResourceLimits | null>>;

  /**
   * Pulls the document status from the bridge, sets it on state, and
   * returns it. Returns `null` if the bridge call failed (the error
   * is also routed through `onStatusError`). Callers that just want
   * the side-effect (state update) can ignore the return value;
   * callers that need to use the freshly-fetched value (e.g. for a
   * follow-up action that references the latest value before the
   * next render commits) read the returned value directly instead
   * of awaiting React's commit and reading from state / a ref.
   */
  refreshStatus: () => Promise<DocumentStatus | null>;
  /**
   * Pulls the artboard list from the bridge, sets it on state, and
   * returns it. Returns `[]` if the bridge call failed. See
   * `refreshStatus` for the rationale on returning the value.
   *
   * `EditorPage.handleCreateArtboard` is the canonical caller that
   * needs the freshly-fetched value: it has to locate the newly-
   * created artboard by id to focus it, and reading from
   * `artboards` state or `artboardsRef.current` immediately after
   * `await refreshArtboards()` is unreliable because React's commit
   * runs after the awaited promise resolves.
   */
  refreshArtboards: () => Promise<ArtboardInfo[]>;
  /**
   * Pulls the component list from the bridge, sets it on state, and
   * returns it. Returns `[]` if the bridge call failed.
   */
  refreshComponents: () => Promise<ComponentInfo[]>;
  /**
   * Re-pulls the document tree ONLY. Single-purpose by design —
   * status / artboards / components / selection refreshes live on
   * their own actions so callers can compose them in whatever
   * order their feature requires. EditorPage's composed full-resync
   * wraps this together with `refreshStatus` / `refreshSelection` /
   * `refreshArtboards` / `refreshComponents` to preserve the
   * pre-refactor sequencing exactly. Returns the fetched tree
   * (`[]` on failure) so callers that compose this with other
   * refreshes can chain the returned data.
   */
  refreshTree: () => Promise<NodeInfo[]>;
}

/**
 * Read-latest refs for callbacks that must stay reference-stable.
 * Matches EditorPage's prior `nodesRef` / `artboardsRef` pattern —
 * see `EditorContext.EditorRefs` for the broader rationale.
 */
export interface DocumentRefs {
  nodesRef: MutableRefObject<NodeInfo[]>;
  artboardsRef: MutableRefObject<ArtboardInfo[]>;
}

interface DocumentContextValue {
  state: DocumentState;
  actions: DocumentActions;
  refs: DocumentRefs;
}

/**
 * Why three distinct contexts instead of a single bundle?
 *
 * React's context API only supports whole-value subscription:
 * `useContext(ctx)` re-renders the consumer whenever the provider's
 * `value` prop changes identity. A `{ state, actions, refs }` bundle
 * forces every consumer to re-render on every state change — even
 * components that only read `actions`, whose identity is stable for
 * the provider's whole lifetime.
 *
 * Splitting into `DocumentStateContext` / `DocumentActionsContext` /
 * `DocumentRefsContext` lets each consumer subscribe only to what
 * it actually reads. `EditorPage` keeps its existing destructure via
 * `useDocument()` (which subscribes to all three — same behaviour as
 * before), but components that only need refreshers or refs can
 * call the targeted hooks and stay inert through state churn. See
 * the parallel rationale block in `EditorContext.tsx`.
 */
const DocumentStateContext = createContext<DocumentState | null>(null);
const DocumentActionsContext = createContext<DocumentActions | null>(null);
const DocumentRefsContext = createContext<DocumentRefs | null>(null);

export interface DocumentProviderProps {
  /**
   * Initial artboard presets. EditorPage prior pre-seeded these
   * synchronously from a hard-coded list while the bridge probe
   * resolved; we accept it as a prop so the provider stays test-
   * friendly.
   */
  initialArtboardPresets?: ArtboardPreset[];
  /**
   * Error sink for refresh failures. EditorPage wires this to
   * `EditorContext.setStatusMessage`. The provider invokes it with
   * a human-readable string like `"status probe failed: ..."`.
   * Optional; when omitted, refresh errors are swallowed.
   */
  onStatusError?: (msg: string) => void;
  children: ReactNode;
}

/**
 * Provider that owns the document mirror state. Mount inside the
 * editor surface (today: `EditorPage`).
 */
export function DocumentProvider({
  initialArtboardPresets = [],
  onStatusError,
  children,
}: DocumentProviderProps): JSX.Element {
  const [nodes, setNodes] = useState<NodeInfo[]>([]);
  const [artboards, setArtboards] = useState<ArtboardInfo[]>([]);
  const [artboardPresets, setArtboardPresets] =
    useState<ArtboardPreset[]>(initialArtboardPresets);
  const [components, setComponents] = useState<ComponentInfo[]>([]);
  const [docStatus, setDocStatus] = useState<DocumentStatus | null>(null);
  const [resourceLimits, setResourceLimits] = useState<ResourceLimits | null>(
    null,
  );

  // Read-latest refs. Match EditorPage's prior pattern.
  const nodesRef = useRef<NodeInfo[]>(nodes);
  useEffect(() => {
    nodesRef.current = nodes;
  }, [nodes]);
  const artboardsRef = useRef<ArtboardInfo[]>(artboards);
  useEffect(() => {
    artboardsRef.current = artboards;
  }, [artboards]);

  // Keep the error sink in a ref so we can call it inside refresh
  // callbacks without re-creating those callbacks every render —
  // EditorPage's prior versions had the same property.
  const onStatusErrorRef = useRef(onStatusError);
  useEffect(() => {
    onStatusErrorRef.current = onStatusError;
  }, [onStatusError]);

  const reportError = useCallback((msg: string): void => {
    onStatusErrorRef.current?.(msg);
  }, []);

  const refreshStatus = useCallback(async (): Promise<DocumentStatus | null> => {
    try {
      const s = await window.kcreate.document.status();
      setDocStatus(s);
      return s;
    } catch (e) {
      reportError(`status probe failed: ${errorMessage(e)}`);
      return null;
    }
  }, [reportError]);

  const refreshArtboards = useCallback(async (): Promise<ArtboardInfo[]> => {
    try {
      const list = await window.kcreate.artboard.list();
      setArtboards(list);
      return list;
    } catch (e) {
      reportError(`artboard list failed: ${errorMessage(e)}`);
      return [];
    }
  }, [reportError]);

  const refreshComponents = useCallback(async (): Promise<ComponentInfo[]> => {
    try {
      const list = await window.kcreate.component.list();
      setComponents(list);
      return list;
    } catch (e) {
      reportError(`component list failed: ${errorMessage(e)}`);
      return [];
    }
  }, [reportError]);

  const refreshTree = useCallback(async (): Promise<NodeInfo[]> => {
    try {
      const tree = await window.kcreate.document.getDocumentTree();
      setNodes(tree);
      return tree;
    } catch (e) {
      reportError(`tree load failed: ${errorMessage(e)}`);
      return [];
    }
  }, [reportError]);

  const actions = useMemo<DocumentActions>(
    () => ({
      setNodes,
      setArtboards,
      setArtboardPresets,
      setComponents,
      setDocStatus,
      setResourceLimits,
      refreshStatus,
      refreshArtboards,
      refreshComponents,
      refreshTree,
    }),
    [refreshStatus, refreshArtboards, refreshComponents, refreshTree],
  );

  const refs = useMemo<DocumentRefs>(
    () => ({
      nodesRef,
      artboardsRef,
    }),
    [],
  );

  const state = useMemo<DocumentState>(
    () => ({
      nodes,
      artboards,
      artboardPresets,
      components,
      docStatus,
      resourceLimits,
      scene: EMPTY_SCENE,
    }),
    [nodes, artboards, artboardPresets, components, docStatus, resourceLimits],
  );

  return (
    <DocumentStateContext.Provider value={state}>
      <DocumentActionsContext.Provider value={actions}>
        <DocumentRefsContext.Provider value={refs}>
          {children}
        </DocumentRefsContext.Provider>
      </DocumentActionsContext.Provider>
    </DocumentStateContext.Provider>
  );
}

function requireDocumentContext<T>(
  ctxValue: T | null,
  consumerName: string,
): T {
  if (ctxValue === null) {
    throw new Error(
      `DocumentContext consumer used outside <DocumentProvider>. Wrap the ` +
        `editor surface in <DocumentProvider> before rendering components ` +
        `that call ${consumerName}.`,
    );
  }
  return ctxValue;
}

/**
 * Full bundle — state + actions + refs. Convenient for EditorPage
 * (which needs everything).
 *
 * NOTE: subscribers to this hook re-render on every state change.
 * Components that only need a subset MUST use `useDocumentState` /
 * `useDocumentActions` / `useDocumentRefs` to opt out of unrelated
 * re-renders.
 *
 * The returned bundle is memoised against `[state, actions, refs]` so
 * its object identity only changes when one of the three underlying
 * context values changes. Without the memo, every call would build a
 * fresh `{ state, actions, refs }` object — safe for the current
 * destructuring callsites in `EditorPageInner`, but a foot-gun for any
 * future consumer that passes the bundle into a `useEffect` /
 * `useMemo` dep array or down to a child as a prop (it would re-fire /
 * re-render on every parent render). Memoising at the hook boundary
 * makes the bundle behave like the underlying contexts themselves.
 */
export function useDocument(): DocumentContextValue {
  const state = requireDocumentContext(
    useContext(DocumentStateContext),
    "useDocument",
  );
  const actions = requireDocumentContext(
    useContext(DocumentActionsContext),
    "useDocument",
  );
  const refs = requireDocumentContext(
    useContext(DocumentRefsContext),
    "useDocument",
  );
  return useMemo<DocumentContextValue>(
    () => ({ state, actions, refs }),
    [state, actions, refs],
  );
}

/** State only — re-renders on any state change. */
export function useDocumentState(): DocumentState {
  return requireDocumentContext(
    useContext(DocumentStateContext),
    "useDocumentState",
  );
}

/**
 * Actions only — stable identity for the provider's lifetime, so
 * consumers of this hook never re-render from state changes here.
 */
export function useDocumentActions(): DocumentActions {
  return requireDocumentContext(
    useContext(DocumentActionsContext),
    "useDocumentActions",
  );
}

/**
 * Refs only — stable identity for the provider's lifetime, so
 * consumers of this hook never re-render from state changes here.
 */
export function useDocumentRefs(): DocumentRefs {
  return requireDocumentContext(
    useContext(DocumentRefsContext),
    "useDocumentRefs",
  );
}
