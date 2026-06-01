import type { MouseEvent as ReactMouseEvent, ReactNode } from "react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import {
  AnnotationOverlay,
  type AnnotationOverlayHandle,
} from "../components/AnnotationOverlay";
import { CanvasHost, type ViewportState } from "../components/CanvasHost";
import {
  DEFAULT_VIEWPORT,
  EditorProvider,
  useEditor,
  useEditorActions,
  type InlineTextEditState,
  type ToolId,
} from "../contexts/EditorContext";
import {
  DocumentProvider,
  useDocument,
} from "../contexts/DocumentContext";
import { ConflictToast } from "../components/ConflictToast";
import { InlineTextEditor } from "../components/InlineTextEditor";
import { CursorOverlay } from "../components/CursorOverlay";
import { LeftPanel } from "../components/LeftPanel";
import { PageNavigator } from "../components/PageNavigator";
import { PathfinderPanel } from "../components/PathfinderPanel";
import { PenOverlay } from "../components/PenOverlay";
import { NodeEditOverlay } from "../components/NodeEditOverlay";
import { RightPanel } from "../components/RightPanel";
import { SelectionOverlay } from "../components/SelectionOverlay";
import { SoftProofOverlay } from "../components/SoftProofOverlay";
import { TemplatePicker } from "../components/TemplatePicker";
import { KeyboardShortcutsPanel } from "../components/KeyboardShortcutsPanel";
import {
  TopBar,
  toolsForMode,
  defaultPanelForMode,
} from "../components/TopBar";
import { AIAssistPanel } from "../components/AIAssistPanel";
import { ExportPanel } from "../components/ExportPanel";
import { ArtboardDialog } from "../components/ArtboardDialog";
import { ResponsivePreview } from "../components/ResponsivePreview";
import { PrototypePlayer } from "../components/PrototypePlayer";
import type {
  Alignment,
  ArtboardInfo,
  DistributeAxis,
  FlexLayout,
  GridLayout,
  NodeInfo,
  ProjectInfo,
  SnapGuide,
} from "../../../shared/scene";
import { LowResourceBanner } from "../components/LowResourceBanner";
import { useShortcuts } from "../shortcuts/useShortcuts";
import type { ShortcutHandlers } from "../shortcuts/useShortcuts";
import { colors, font, spacing } from "../styles/tokens";
import { errorMessage } from "../lib/errorMessage";
import { useToolStateMachine } from "../hooks/useToolStateMachine";

export interface EditorPageProps {
  project: ProjectInfo;
  onBackHome: () => void;
}

/// Active drawing/selection tool. Re-exported from EditorContext for
/// callers that imported `ToolId` from EditorPage in the past — the
/// canonical definition now lives in `../contexts/EditorContext`.
export type { ToolId } from "../contexts/EditorContext";

const CANVAS_WIDTH = 1024;
const CANVAS_HEIGHT = 640;

// Envelope header prefixed to the OS clipboard payload by `handleCopy`
// and stripped by `handlePaste`. Lets the paste path distinguish a
// KCreate node payload from arbitrary text the user might already
// have on the clipboard (the header is plain ASCII so a stray paste
// into a plain-text editor still produces a readable JSON document).
// Kept at module scope so it isn't recreated on every render — the
// component body only ever reads it.
const CLIPBOARD_ENVELOPE_HEADER = "kcreate:clipboard/v1\n";

const TOOL_CURSORS: Record<ToolId, string> = {
  select: "default",
  rect: "crosshair",
  ellipse: "crosshair",
  line: "crosshair",
  // Pen uses the same crosshair as the other geometry tools rather
  // than a custom "pen tip" cursor; the in-canvas anchor/handle
  // overlay (see PenOverlay) gives the user the precision feedback
  // a bespoke cursor would otherwise add.
  pen: "crosshair",
  text: "text",
};

/**
 * Bridge component that wires `DocumentProvider.onStatusError` to
 * `EditorContext.setStatusMessage`. Lives between the two providers
 * because `DocumentProvider` needs to call the editor's status
 * setter, but `setStatusMessage` is only available inside
 * `EditorProvider`. Mounted by the outer `EditorPage` shell below.
 */
function EditorDocumentBridge({
  children,
}: {
  children: ReactNode;
}): JSX.Element {
  const { setStatusMessage } = useEditorActions();
  // Wrap `setStatusMessage` in a plain `(msg: string) => void` closure
  // instead of passing the `Dispatch<SetStateAction<string | null>>`
  // setter directly. Without the wrapper, if `onStatusError` were ever
  // invoked with a function-shaped argument (e.g. a future refactor in
  // `DocumentContext.reportError`), the setter would silently interpret
  // it as a functional updater. The closure narrows the signature to
  // exactly `(msg: string) => void`, making any such drift a compile
  // error at the call site instead of a runtime surprise. Memoised so
  // the prop identity is stable across editor state changes (matters
  // for `onStatusErrorRef` lockstep inside `DocumentProvider`).
  const onStatusError = useCallback(
    (msg: string): void => {
      setStatusMessage(msg);
    },
    [setStatusMessage],
  );
  return (
    <DocumentProvider onStatusError={onStatusError}>
      {children}
    </DocumentProvider>
  );
}

/**
 * Outer shell. Mounts the two providers, then delegates to
 * `EditorPageInner` which reads the contexts and renders the
 * editor surface. Splitting the shell from the body lets the body
 * use `useEditor()` / `useDocument()` (which require being under
 * the respective providers).
 */
export function EditorPage(props: EditorPageProps): JSX.Element {
  return (
    <EditorProvider>
      <EditorDocumentBridge>
        <EditorPageInner {...props} />
      </EditorDocumentBridge>
    </EditorProvider>
  );
}

function EditorPageInner({
  project,
  onBackHome,
}: EditorPageProps): JSX.Element {
  // Editor UI / tool state lives in `EditorContext`. We destructure
  // here so the existing local variable names in the function body
  // continue to compile unchanged.
  const editor = useEditor();
  const {
    mode,
    tool,
    selectedIds,
    statusMessage,
    viewport,
    fps,
    panActive,
    snapGuides,
    inlineTextEdit,
  } = editor.state;
  const {
    setMode,
    setTool,
    setSelectedIds,
    setStatusMessage,
    setViewport,
    setFps,
    setPanActive,
    setSnapGuides,
    setInlineTextEdit,
  } = editor.actions;
  const { selectedIdsRef, panActiveRef, inlineTextEditRef } = editor.refs;

  // Document / project mirror state lives in `DocumentContext`.
  const documentCtx = useDocument();
  const {
    nodes,
    artboards,
    artboardPresets,
    components,
    docStatus,
    resourceLimits,
    scene,
  } = documentCtx.state;
  const {
    setArtboardPresets,
    setResourceLimits,
    refreshStatus,
    refreshArtboards,
    refreshComponents,
    refreshTree: refreshDocumentTree,
  } = documentCtx.actions;
  const { nodesRef, artboardsRef } = documentCtx.refs;

  // Phase 6 Tasks 25-26: latest world-space cursor sample. Paste reads
  // it (`handlePaste` below) to position the new subtree near the
  // cursor. The state machine in `useToolStateMachine` (mounted later
  // in this component) updates it on every pointer event (down / move
  // / up) so a stationary cursor still drives a sensible paste origin.
  // Declared up here, BEFORE `handlePaste`, so the closure capture is
  // safe — moving it next to the state machine call site would put it
  // in the Temporal Dead Zone for the paste closure created earlier.
  const lastCursorWorldRef = useRef<{ x: number; y: number } | null>(null);

  // Host-specific UI / modal state. These don't need to be shared
  // with future tools so they stay as plain `useState` in the
  // EditorPage body.
  const [prototypePlaying, setPrototypePlaying] = useState<boolean>(false);
  // Stable identity for `PrototypePlayer`'s `onClose` prop. Devin
  // Review PR #5 ANALYSIS-0004 (commit 4ee9970): the player's
  // keyboard handler lists `onClose` in its `useEffect` deps so the
  // Escape-to-close binding stays in sync if the host swaps out the
  // callback. An inline `() => setPrototypePlaying(false)` creates a
  // new function identity on every parent render → re-adds the
  // `window.addEventListener("keydown", ...)` listener every time
  // anything else in `EditorPage` re-renders. `useCallback` with an
  // empty deps array gives us a single stable identity (React
  // guarantees `setState` setters are stable), eliminating the
  // listener churn without changing behaviour.
  const handlePrototypeClose = useCallback((): void => {
    setPrototypePlaying(false);
  }, []);
  // `panActive` lifecycle (state + ref mirror + defense-in-depth
  // disarm on window blur / visibilitychange) is owned by
  // `EditorContext.EditorProvider`. The host only reads + dispatches.
  const [artboardDialogOpen, setArtboardDialogOpen] = useState(false);
  // TemplatePicker is shown automatically the first time the user
  // enters Layout mode for a given project. The sentinel below is per
  // project id so re-opening a project skips the picker, but switching
  // projects within one session re-prompts.
  const [templatePickerOpen, setTemplatePickerOpen] = useState<boolean>(false);
  const [shortcutsPanelOpen, setShortcutsPanelOpen] = useState<boolean>(false);
  const [layoutPickerShownFor, setLayoutPickerShownFor] = useState<string | null>(
    null,
  );
  const lastTickAtRef = useRef<number>(performance.now());
  // Drag state lives in `useToolStateMachine` (`apps/desktop/renderer/
  // src/hooks/useToolStateMachine.ts`). The hook owns the
  // discriminated-union state (Idle | Pan | Move | Create), the
  // pointer event router, and the bridge side effects (hit-test,
  // snap query, moveNode, createRect/Ellipse/Line/Text). The hook
  // also owns the last-world-cursor sample (used by paste-at-cursor)
  // because it's the only place with the live screen→world
  // transform handy. See the `dragKind` reader below for how the
  // cursor logic peeks at the state machine, and `lastCursorWorld`
  // for the paste reader.

  // `nodes`, `selectedIds`, and `artboards` read-latest refs live in
  // their respective contexts (`DocumentContext` / `EditorContext`).
  // Consumers in this function body read them via the destructured
  // names above (`nodesRef`, `selectedIdsRef`, `artboardsRef`) so
  // the call sites are unchanged. The rationale for refs-instead-of-
  // deps is documented on `EditorRefs` and `DocumentRefs`.

  const selectedId: string | null =
    selectedIds.length === 1 ? (selectedIds[0] ?? null) : null;

  // `refreshStatus`, `refreshArtboards`, `refreshComponents`, and
  // `refreshDocumentTree` come from `DocumentContext.actions` and
  // each pulls exactly one slice from the bridge. `refreshSelection`
  // lives here because selection state belongs to `EditorContext`
  // (cross-context orchestration), not `DocumentContext`. The
  // composed `refreshTree` below preserves the pre-refactor
  // sequencing (tree → status → selection → artboards → components)
  // verbatim so undo/redo / artboard-creation / component-instance
  // flows that depend on visibility ordering keep their semantics.

  const refreshSelection = useCallback(async () => {
    try {
      const sel = await window.kcreate.canvas.getSelection();
      setSelectedIds(sel);
    } catch (e) {
      setStatusMessage(`selection probe failed: ${errorMessage(e)}`);
    }
  }, [setSelectedIds, setStatusMessage]);

  /**
   * Composed full-resync. Pulls the document tree, status, selection,
   * artboards, and components in the exact sequence the pre-refactor
   * `EditorPage` used. Returns the fetched artboards / components /
   * tree so callers that need a freshly-fetched value for a follow-up
   * action (e.g. `handleCreateArtboard` locating the newly-created
   * artboard by id) can read it directly instead of double-fetching
   * via a second IPC round-trip or racing against React's commit.
   *
   * Devin Review #0003 on PR #35: the prior `handleCreateArtboard`
   * called `refreshTree()` and then `await window.kcreate.artboard.list()`
   * a second time because the composed refresher swallowed the data
   * `refreshArtboards()` had already pulled. Threading the data
   * through the return value eliminates the double-fetch while
   * preserving the side-effect semantics for callers that ignore it.
   */
  const refreshTree = useCallback(async () => {
    const tree = await refreshDocumentTree();
    const status = await refreshStatus();
    await refreshSelection();
    const artboardsList = await refreshArtboards();
    const componentsList = await refreshComponents();
    return {
      tree,
      status,
      artboards: artboardsList,
      components: componentsList,
    };
  }, [
    refreshDocumentTree,
    refreshStatus,
    refreshSelection,
    refreshArtboards,
    refreshComponents,
  ]);

  // Initial-load resync. Fires once on mount because `refreshTree`'s
  // identity is stable for the provider's lifetime (all of its
  // transitive deps — `refreshDocumentTree`, `refreshStatus`,
  // `refreshSelection`, `refreshArtboards`, `refreshComponents` —
  // come from DocumentContext / EditorContext actions, both of which
  // are memoised with empty deps so their callable identities never
  // change).
  //
  // Pre-refactor this comment said "Initial load + on-mode-change
  // resync", which was stale — mode-change reset for the active
  // tool is handled by a separate `useEffect` further down (the one
  // keyed on `[mode, tool, setTool]`). Devin Review #0004 on
  // commit `5b09939` flagged the drift.
  useEffect(() => {
    void refreshTree();
  }, [refreshTree]);

  // First-time-into-Layout prompt: when the user switches to Layout
  // mode for the first time on an untouched project, pop the
  // TemplatePicker.
  //
  // We ask the bridge (`document.isUntouched`) instead of inferring
  // it from the document tree shape. The bridge's signal is
  // `Project::operation_log.is_empty()` — every host-recorded
  // mutation runs through `Project::execute_operation`, so an empty
  // log is the strict, authoritative "no user edits yet" check. See
  // `crates/kcreate_bridge/src/document.rs::project_is_untouched`.
  //
  // The previous implementation replicated `Project::add_page("Page
  // 1")`'s exact output in TypeScript (`nodes.length === 2 && one
  // Page named "Page 1" && one Artboard`). That was fragile: if the
  // Rust side ever renamed the default page, added a default layer,
  // or restructured the initial node graph, the heuristic would
  // silently break and the picker would never auto-open (Devin
  // Review PR #5 ANALYSIS-0006, commit 5c16b5c). Moving the
  // detection to the bridge keeps the source of truth on the side
  // that actually owns the project state.
  //
  // The user can re-open the picker any time via the "Templates"
  // button in the PageNavigator footer; this just controls the
  // automatic first-pop.
  //
  // **One probe per project session.** Devin Review PR #5
  // ANALYSIS-0006 on commit 4ee9970 flagged that the previous
  // implementation re-probed the bridge on every `nodes.length`
  // change — wasteful since the untouched→touched transition is
  // monotonic per project session (`operation_log.is_empty()` only
  // ever flips false→true once, because we never clear the log
  // mid-session). One probe at project-open time is sufficient; the
  // probe `useEffect` only fires when `project.id` changes (open /
  // create / reopen) and the result lives in `untouchedProbe` for
  // the remainder of the session.
  const [untouchedProbe, setUntouchedProbe] = useState<{
    projectId: string;
    isUntouched: boolean;
  } | null>(null);
  useEffect(() => {
    let cancelled = false;
    void (async (): Promise<void> => {
      try {
        const isUntouched = await window.kcreate.document.isUntouched();
        if (!cancelled) {
          setUntouchedProbe({ projectId: project.id, isUntouched });
        }
      } catch (e) {
        // No project open, or bridge call rejected — fall back to
        // "not untouched" so we don't surprise the user with a
        // modal pop during a transient error state.
        if (!cancelled) {
          setUntouchedProbe({ projectId: project.id, isUntouched: false });
          setStatusMessage(`isUntouched probe failed: ${errorMessage(e)}`);
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [project.id, setStatusMessage]);

  useEffect(() => {
    if (mode !== "layout") return;
    if (layoutPickerShownFor === project.id) return;
    if (untouchedProbe === null) return;
    if (untouchedProbe.projectId !== project.id) return;
    if (!untouchedProbe.isUntouched) {
      // User already designed something — don't surprise them with a
      // modal they didn't ask for. Mark the sentinel anyway so the
      // logic doesn't re-evaluate.
      setLayoutPickerShownFor(project.id);
      return;
    }
    setTemplatePickerOpen(true);
    setLayoutPickerShownFor(project.id);
  }, [mode, project.id, untouchedProbe, layoutPickerShownFor]);

  // Layout mode page selection helper — selects the page node so the
  // canvas pans/zooms to its bounds and the right panel shows its
  // properties.
  const handleSelectPage = useCallback(
    async (pageId: string): Promise<void> => {
      try {
        await window.kcreate.canvas.setSelection([pageId]);
        setSelectedIds([pageId]);
      } catch (e) {
        setStatusMessage(`select page failed: ${errorMessage(e)}`);
      }
    },
    [setSelectedIds, setStatusMessage],
  );

  const refreshResourceLimits = useCallback(async () => {
    try {
      const limits = await window.kcreate.runtime.resourceLimits();
      setResourceLimits(limits);
    } catch (e) {
      setStatusMessage(`resource limits failed: ${errorMessage(e)}`);
    }
  }, [setStatusMessage, setResourceLimits]);

  const handleToggleLowResource = useCallback(
    async (enabled: boolean) => {
      try {
        await window.kcreate.runtime.lowResourceModeSet(enabled);
        await refreshResourceLimits();
      } catch (e) {
        setStatusMessage(`toggle low-resource failed: ${errorMessage(e)}`);
      }
    },
    [refreshResourceLimits, setStatusMessage],
  );

  useEffect(() => {
    void refreshResourceLimits();
  }, [refreshResourceLimits]);

  // Load preset catalogue once. It's deterministic and the bridge
  // recomputes it on each call so caching once on mount is fine.
  useEffect(() => {
    let cancelled = false;
    void window.kcreate.artboard
      .presets()
      .then((p) => {
        if (!cancelled) setArtboardPresets(p);
      })
      .catch((err: unknown) => {
        if (!cancelled) {
          setStatusMessage(`artboard presets failed: ${errorMessage(err)}`);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [setArtboardPresets, setStatusMessage]);

  // Focus the canvas viewport on an artboard with ~10% margin.
  // World→screen transform is `screen = world * zoom + pan`, so to
  // center an artboard of bounds (x, y, w, h) we solve for pan such
  // that the artboard center sits at the canvas center.
  const focusArtboard = useCallback((a: ArtboardInfo) => {
    const marginFactor = 0.9;
    const zoom = Math.min(
      (CANVAS_WIDTH * marginFactor) / Math.max(a.width, 1),
      (CANVAS_HEIGHT * marginFactor) / Math.max(a.height, 1),
    );
    const centerWorldX = a.x + a.width / 2;
    const centerWorldY = a.y + a.height / 2;
    const panX = CANVAS_WIDTH / 2 - centerWorldX * zoom;
    const panY = CANVAS_HEIGHT / 2 - centerWorldY * zoom;
    setViewport({ panX, panY, zoom });
    void window.kcreate.canvas.setSelection([a.id]).then(refreshSelection);
  }, [refreshSelection, setViewport]);

  const handleCreateArtboard = useCallback(
    async (args: { name: string; width: number; height: number }) => {
      try {
        const id = await window.kcreate.artboard.create(
          null,
          args.name,
          args.width,
          args.height,
        );
        // Use the artboards list `refreshTree` already pulled (via
        // `refreshArtboards` inside its cascade). Reading from the
        // returned value instead of state / `artboardsRef.current`
        // avoids two pitfalls: (a) a redundant second
        // `window.kcreate.artboard.list()` IPC round-trip, and (b)
        // a race against React's commit since the ref / state are
        // only updated when the next render flushes — not when the
        // `await refreshTree()` promise resolves.
        // Devin Review #0003 on PR #35.
        const { artboards: list } = await refreshTree();
        const created = list.find((a) => a.id === id);
        if (created) focusArtboard(created);
      } catch (e) {
        setStatusMessage(`create artboard failed: ${errorMessage(e)}`);
      } finally {
        setArtboardDialogOpen(false);
      }
    },
    [refreshTree, focusArtboard, setStatusMessage],
  );

  /**
   * Phase B2 — re-select the new boolean result(s) and refresh
   * the document tree after a successful Pathfinder gesture.
   * Memoised so `PathfinderPanel`'s internal `useCallback` for the
   * click dispatcher has a stable identity across `EditorPage`
   * re-renders — matches the stability discipline `onStatus`
   * already has via the `setStatusMessage` setter (Devin Review
   * #0003 on PR #38).
   */
  const handlePathfinderApplied = useCallback(
    (resultIds: string[]) => {
      setSelectedIds(resultIds);
      void refreshTree();
    },
    [setSelectedIds, refreshTree],
  );

  const handleDuplicateArtboard = useCallback(
    async (id: string) => {
      try {
        await window.kcreate.artboard.duplicate(id);
        await refreshTree();
      } catch (e) {
        setStatusMessage(`duplicate artboard failed: ${errorMessage(e)}`);
      }
    },
    [refreshTree, setStatusMessage],
  );

  const handleResizeArtboard = useCallback(
    async (id: string, width: number, height: number) => {
      try {
        await window.kcreate.artboard.resize(id, width, height);
        await refreshTree();
      } catch (e) {
        setStatusMessage(`resize artboard failed: ${errorMessage(e)}`);
      }
    },
    [refreshTree, setStatusMessage],
  );

  const handleDeleteArtboard = useCallback(
    async (id: string) => {
      try {
        await window.kcreate.document.deleteNode(id);
        await refreshTree();
      } catch (e) {
        setStatusMessage(`delete artboard failed: ${errorMessage(e)}`);
      }
    },
    [refreshTree, setStatusMessage],
  );

  const handleRenameArtboard = useCallback(
    async (id: string, name: string) => {
      try {
        await window.kcreate.document.updateNode(id, { name });
        await refreshTree();
      } catch (e) {
        setStatusMessage(`rename artboard failed: ${errorMessage(e)}`);
      }
    },
    [refreshTree, setStatusMessage],
  );

  // Component lifecycle handlers. Each one mirrors a single bridge
  // call and then refreshes both the tree and the component list so
  // the panel stays in sync.
  const handleComponentCreateFromSelection = useCallback(
    async (name: string) => {
      if (selectedIds.length === 0) {
        setStatusMessage("select one or more sibling nodes first");
        return;
      }
      try {
        await window.kcreate.component.createFromSelection(selectedIds, name);
        await refreshTree();
      } catch (e) {
        setStatusMessage(`create component failed: ${errorMessage(e)}`);
      }
    },
    [selectedIds, refreshTree, setStatusMessage],
  );

  const handleComponentInstantiate = useCallback(
    async (componentId: string) => {
      // Default to the first artboard (or null = project root) so
      // newly-placed instances land somewhere visible. (x, y) is the
      // top-left of the new layer relative to that parent.
      const parentId = artboards[0]?.id ?? null;
      try {
        await window.kcreate.component.instantiate(
          componentId,
          parentId,
          80,
          80,
        );
        await refreshTree();
      } catch (e) {
        setStatusMessage(`instantiate component failed: ${errorMessage(e)}`);
      }
    },
    [artboards, refreshTree, setStatusMessage],
  );

  const handleComponentAddVariant = useCallback(
    async (componentId: string, name: string) => {
      try {
        await window.kcreate.component.addVariant(componentId, name);
        await refreshComponents();
      } catch (e) {
        setStatusMessage(`add variant failed: ${errorMessage(e)}`);
      }
    },
    [refreshComponents, setStatusMessage],
  );

  const handleComponentSwitchVariant = useCallback(
    async (nodeId: string, variantId: string) => {
      try {
        await window.kcreate.component.switchVariant(nodeId, variantId);
        await refreshTree();
      } catch (e) {
        setStatusMessage(`switch variant failed: ${errorMessage(e)}`);
      }
    },
    [refreshTree, setStatusMessage],
  );

  const handleComponentDetach = useCallback(
    async (nodeId: string) => {
      try {
        await window.kcreate.component.detach(nodeId);
        await refreshTree();
      } catch (e) {
        setStatusMessage(`detach component failed: ${errorMessage(e)}`);
      }
    },
    [refreshTree, setStatusMessage],
  );

  const layoutHandlers = useMemo(
    () => ({
      // `setFlex` / `setGrid` only persist the layout config on the
      // node; the visible child bounds change when `recompute` runs
      // *next*. Skip the intermediate `refreshTree` here so a single
      // user edit fires just two IPC round-trips (set → recompute)
      // and one final tree fetch instead of four. RightPanel's
      // FlexControls / GridControls always call `recompute` right
      // after, which carries the refresh.
      setFlex: async (nodeId: string, config: FlexLayout) => {
        try {
          await window.kcreate.layout.setFlex(nodeId, config);
        } catch (e) {
          setStatusMessage(`set flex layout failed: ${errorMessage(e)}`);
        }
      },
      setGrid: async (nodeId: string, config: GridLayout) => {
        try {
          await window.kcreate.layout.setGrid(nodeId, config);
        } catch (e) {
          setStatusMessage(`set grid layout failed: ${errorMessage(e)}`);
        }
      },
      recompute: async (nodeId: string) => {
        try {
          await window.kcreate.layout.recompute(nodeId);
          await refreshTree();
        } catch (e) {
          setStatusMessage(`layout recompute failed: ${errorMessage(e)}`);
        }
      },
      convertToFrame: async (nodeId: string) => {
        try {
          await window.kcreate.layout.convertToFrame(nodeId);
          await refreshTree();
        } catch (e) {
          setStatusMessage(
            `layout convert to frame failed: ${errorMessage(e)}`,
          );
        }
      },
    }),
    [refreshTree, setStatusMessage],
  );

  // When the mode changes, snap to its default tool so the canvas
  // cursor and toolbar stay aligned.
  useEffect(() => {
    const tools = toolsForMode(mode);
    if (!tools.includes(tool)) {
      setTool(tools[0] ?? "select");
    }
  }, [mode, tool, setTool]);

  const selected = useMemo(
    () => nodes.find((n) => n.id === selectedId) ?? null,
    [nodes, selectedId],
  );

  /// Active page id used by the annotation overlay. Derived
  /// either from the current selection (if it's a Page) or from
  /// the first Page in the document tree. Returns `null` if the
  /// project has no pages yet (during boot before refresh).
  const activePageId = useMemo<string | null>(() => {
    if (selected && selected.nodeType === "Page") return selected.id;
    const firstPage = nodes.find((n) => n.nodeType === "Page");
    return firstPage ? firstPage.id : null;
  }, [selected, nodes]);

  const canUndo = docStatus?.canUndo ?? false;
  const canRedo = docStatus?.canRedo ?? false;

  const handleUndo = useCallback(async () => {
    try {
      await window.kcreate.document.undo();
      await refreshTree();
    } catch (e) {
      setStatusMessage(`undo failed: ${errorMessage(e)}`);
    }
  }, [refreshTree, setStatusMessage]);

  const handleRedo = useCallback(async () => {
    try {
      await window.kcreate.document.redo();
      await refreshTree();
    } catch (e) {
      setStatusMessage(`redo failed: ${errorMessage(e)}`);
    }
  }, [refreshTree, setStatusMessage]);

  const handleDeleteSelected = useCallback(async () => {
    if (selectedIds.length === 0) return;
    try {
      for (const id of selectedIds) {
        await window.kcreate.document.deleteNode(id);
      }
      await window.kcreate.canvas.clearSelection();
      await refreshTree();
    } catch (e) {
      setStatusMessage(`delete failed: ${errorMessage(e)}`);
    }
  }, [selectedIds, refreshTree, setStatusMessage]);

  const handleSelectAll = useCallback(async () => {
    try {
      const ids = nodes.map((n) => n.id);
      await window.kcreate.canvas.setSelection(ids);
      await refreshSelection();
    } catch (e) {
      setStatusMessage(`select all failed: ${errorMessage(e)}`);
    }
  }, [nodes, refreshSelection, setStatusMessage]);

  // Phase D — Align/Distribute keyboard shortcut handlers.
  // Mirrors the AlignmentToolbar's bridge calls 1:1 so the toolbar
  // buttons and the keyboard map share a single dispatch point.
  // Requires the same selection cardinality the toolbar enforces
  // (≥2 to align, ≥3 to distribute); under-selection is a silent
  // no-op rather than a thrown error so the shortcut feels like a
  // dead key when it isn't applicable instead of surfacing a toast.
  //
  // Reads `selectedIds` through a ref so the `shortcutHandlers`
  // useMemo doesn't have to re-create every time the selection
  // changes — matches the `handleCopy` / `handlePaste` pattern above.
  const handleAlign = useCallback(
    async (a: Alignment) => {
      const ids = selectedIdsRef.current;
      if (ids.length < 2) return;
      try {
        await window.kcreate.phase9.documentAlign(ids, a);
        await refreshTree();
      } catch (e) {
        setStatusMessage(`align failed: ${errorMessage(e)}`);
      }
    },
    [selectedIdsRef, refreshTree, setStatusMessage],
  );

  const handleDistribute = useCallback(
    async (axis: DistributeAxis) => {
      const ids = selectedIdsRef.current;
      if (ids.length < 3) return;
      try {
        await window.kcreate.phase9.documentDistribute(ids, axis);
        await refreshTree();
      } catch (e) {
        setStatusMessage(`distribute failed: ${errorMessage(e)}`);
      }
    },
    [selectedIdsRef, refreshTree, setStatusMessage],
  );

  const handleClearSelection = useCallback(async () => {
    try {
      await window.kcreate.canvas.clearSelection();
      setSelectedIds([]);
    } catch (e) {
      setStatusMessage(`clear selection failed: ${errorMessage(e)}`);
    }
  }, [setSelectedIds, setStatusMessage]);

  // ------------------------------------------------------------------
  // Phase 6 Tasks 25-26 — node clipboard.
  //
  // Copy:   serialise the current selection through the Rust bridge
  //         (Page/Artboard ids are filtered out defensively), wrap the
  //         result in the `CLIPBOARD_ENVELOPE_HEADER` envelope so
  //         paste can distinguish a KCreate payload from arbitrary
  //         text the user may have on the OS clipboard, then push to
  //         `navigator.clipboard`.
  //
  // Paste:  read the OS clipboard, validate the envelope, infer the
  //         destination artboard (artboard owning the first selected
  //         node, falling back to the first artboard on the active
  //         page), compute an offset so the top-left of the first
  //         pasted root lands at the cursor (or at +20,+20 when the
  //         cursor hasn't been over the canvas yet), and refresh the
  //         tree / selection so the user sees + can immediately drag
  //         the pasted nodes.
  // ------------------------------------------------------------------

  // `handleCopy` reads `selectedIds` and `nodes` via refs so its
  // identity stays stable across node / selection mutations. This
  // keeps the `shortcutHandlers` memo (and therefore the
  // `useShortcuts` window listeners) from churning on every drag
  // frame — see the `nodesRef` doc comment above.
  const handleCopy = useCallback(async () => {
    const selectionSnapshot = selectedIdsRef.current;
    const nodesSnapshot = nodesRef.current;
    if (selectionSnapshot.length === 0) return;
    try {
      // Filter selected ids that are themselves Pages / Artboards.
      // The bridge filters defensively too, but doing it here avoids a
      // surprising "you copied something but the clipboard is empty"
      // when the user has only top-level container ids selected.
      const eligible = selectionSnapshot.filter((id) => {
        const n = nodesSnapshot.find((node) => node.id === id);
        if (!n) return false;
        return n.nodeType !== "Page" && n.nodeType !== "Artboard";
      });
      if (eligible.length === 0) {
        setStatusMessage(
          "nothing copyable in selection (pages and artboards are copied via duplicate)",
        );
        return;
      }
      const payload = await window.kcreate.clipboard.copy(eligible);
      const enveloped = CLIPBOARD_ENVELOPE_HEADER + payload;
      if (typeof navigator !== "undefined" && navigator.clipboard?.writeText) {
        await navigator.clipboard.writeText(enveloped);
      } else {
        // Headless / pre-Electron-ready boot: stash on window so a
        // subsequent paste in the same session still works. We never
        // surface this fallback to the user — it's a defensive belt.
        (window as unknown as { __kcreateClipboard?: string }).__kcreateClipboard =
          enveloped;
      }
      setStatusMessage(`copied ${eligible.length} node(s)`);
    } catch (e) {
      setStatusMessage(`copy failed: ${errorMessage(e)}`);
    }
  }, [nodesRef, selectedIdsRef, setStatusMessage]);

  // `handlePaste` reads `selectedIds`, `nodes`, and `artboards`
  // via refs for the same listener-churn reason as `handleCopy`.
  // `refreshTree` is itself reference-stable (its deps closure over
  // only `setNodes`-class setters which React guarantees stable),
  // so it can stay in the deps array.
  const handlePaste = useCallback(async () => {
    try {
      const selectionSnapshot = selectedIdsRef.current;
      const nodesSnapshot = nodesRef.current;
      const artboardsSnapshot = artboardsRef.current;
      let enveloped: string | undefined;
      if (typeof navigator !== "undefined" && navigator.clipboard?.readText) {
        try {
          enveloped = await navigator.clipboard.readText();
        } catch {
          // Read can throw if the document isn't focused; fall back to
          // the in-session stash below.
        }
      }
      if (!enveloped) {
        enveloped = (window as unknown as { __kcreateClipboard?: string })
          .__kcreateClipboard;
      }
      if (!enveloped || !enveloped.startsWith(CLIPBOARD_ENVELOPE_HEADER)) {
        setStatusMessage("clipboard is empty or not a KCreate payload");
        return;
      }
      const payload = enveloped.slice(CLIPBOARD_ENVELOPE_HEADER.length);

      // Resolve target artboard: the artboard owning the first
      // selection if any, otherwise the first artboard on the page.
      let targetArtboard: string | null = null;
      if (selectionSnapshot.length > 0) {
        const first = nodesSnapshot.find((n) => n.id === selectionSnapshot[0]);
        // Walk up to find an Artboard ancestor.
        let cursor: NodeInfo | undefined = first;
        while (cursor && cursor.nodeType !== "Artboard") {
          cursor = cursor.parentId
            ? nodesSnapshot.find((n) => n.id === cursor!.parentId)
            : undefined;
        }
        if (cursor && cursor.nodeType === "Artboard") {
          targetArtboard = cursor.id;
        }
      }
      if (!targetArtboard) {
        targetArtboard = artboardsSnapshot[0]?.id ?? null;
      }
      if (!targetArtboard) {
        setStatusMessage("no artboard to paste into");
        return;
      }

      // Compute (offset_x, offset_y) so the first pasted root lands
      // at the cursor (or at +20,+20 when we have no cursor sample).
      let offsetX = 20;
      let offsetY = 20;
      try {
        const parsed = JSON.parse(payload) as {
          subtrees?: { nodes?: { bounds?: { x?: number; y?: number } }[] }[];
        };
        const firstRoot = parsed.subtrees?.[0]?.nodes?.[0]?.bounds;
        const cursorWorld = lastCursorWorldRef.current;
        if (
          firstRoot &&
          typeof firstRoot.x === "number" &&
          typeof firstRoot.y === "number" &&
          cursorWorld
        ) {
          offsetX = cursorWorld.x - firstRoot.x;
          offsetY = cursorWorld.y - firstRoot.y;
        }
      } catch {
        // Malformed payload — let the bridge surface the real error
        // when paste runs. We keep the (+20,+20) fallback so the user
        // still gets visible feedback if Rust accepts it.
      }

      const newRoots = await window.kcreate.clipboard.paste(
        payload,
        targetArtboard,
        offsetX,
        offsetY,
      );
      await refreshTree();
      if (newRoots.length > 0) {
        await window.kcreate.canvas.setSelection(newRoots);
        setSelectedIds(newRoots);
      }
      setStatusMessage(`pasted ${newRoots.length} node(s)`);
    } catch (e) {
      setStatusMessage(`paste failed: ${errorMessage(e)}`);
    }
  }, [refreshTree, artboardsRef, nodesRef, selectedIdsRef, setSelectedIds, setStatusMessage]);

  // Phase D — Duplicate a single layer-tree node via the layer-row
  // context menu. Reuses the clipboard copy+paste primitive so undo/
  // redo, parent-artboard resolution, and the +20/+20 offset behave
  // identically to the keyboard `Cmd+C / Cmd+V` flow. We don't touch
  // the OS clipboard because (1) it would surprise the user if their
  // text-clipboard contents got overwritten by a UI affordance they
  // didn't explicitly initiate, and (2) it would race the async OS
  // clipboard write against the paste read. Instead we serialise the
  // single node via the bridge and feed the payload straight into the
  // paste codepath in-process.
  const handleDuplicateLayer = useCallback(
    async (id: string) => {
      const node = nodesRef.current.find((n) => n.id === id);
      if (!node) return;
      if (node.nodeType === "Page" || node.nodeType === "Artboard") {
        setStatusMessage(
          "pages and artboards duplicate via the page navigator",
        );
        return;
      }
      try {
        const payload = await window.kcreate.clipboard.copy([id]);
        // Resolve target artboard: walk up the parent chain from the
        // node we just copied; fall back to the first artboard on the
        // active page if the node is detached.
        let targetArtboard: string | null = null;
        let cursor: NodeInfo | undefined = node;
        while (cursor && cursor.nodeType !== "Artboard") {
          cursor = cursor.parentId
            ? nodesRef.current.find((n) => n.id === cursor!.parentId)
            : undefined;
        }
        if (cursor && cursor.nodeType === "Artboard") {
          targetArtboard = cursor.id;
        }
        if (!targetArtboard) {
          targetArtboard = artboardsRef.current[0]?.id ?? null;
        }
        if (!targetArtboard) {
          setStatusMessage("no artboard to duplicate into");
          return;
        }
        const newRoots = await window.kcreate.clipboard.paste(
          payload,
          targetArtboard,
          20,
          20,
        );
        await refreshTree();
        if (newRoots.length > 0) {
          await window.kcreate.canvas.setSelection(newRoots);
          setSelectedIds(newRoots);
        }
        setStatusMessage(`duplicated ${newRoots.length} node(s)`);
      } catch (e) {
        setStatusMessage(`duplicate failed: ${errorMessage(e)}`);
      }
    },
    [
      nodesRef,
      artboardsRef,
      refreshTree,
      setSelectedIds,
      setStatusMessage,
    ],
  );

  // ------------------------------------------------------------------
  // Phase 6 Task 25 — drag-and-drop from the OS file manager.
  //
  // We accept the three formats that have a clean bridge entry point:
  //   * raster images (PNG / JPEG / WebP / GIF) → canvas.importImage
  //     (path) or canvas.importImageBytes (sandboxed File only)
  //   * SVG                                     → canvas.importImage
  //     (delegates to the SVG path; the bridge's `document_import_image`
  //     sniffs the magic and routes to the SVG ingest path)
  //   * PDF                                     → pdfImport.importPdf
  // Anything else falls through with a status message rather than a
  // silent no-op.
  //
  // Electron's File objects carry an absolute `path`; the bridge takes
  // file paths so we route through `importImage` directly when path
  // is present. As a defensive fallback (e.g., when a non-Electron
  // dev build is hosting the renderer), we read the bytes and call
  // `importImageBytes` instead — that one accepts raster only.
  // ------------------------------------------------------------------

  // Phase D — drag-hover visual feedback.
  //
  // `dragHover` flips on the first `dragenter` whose payload contains
  // Files and off again when the matching `dragleave` (or `drop`)
  // fires. The DOM emits dragenter / dragleave for EVERY descendant
  // crossing in addition to the root, so a naive boolean toggles on
  // and off as the cursor sweeps across nested elements (CanvasHost,
  // overlays, etc.). We count enters minus leaves on a `useRef`
  // counter — `dragHover === true` iff `counter > 0`. The counter is
  // reset to zero in `handleCanvasDrop` regardless of error because a
  // missed `dragleave` after `drop` would otherwise wedge the overlay
  // on permanently.
  const [dragHover, setDragHover] = useState(false);
  const dragHoverCountRef = useRef(0);

  const handleCanvasDragEnter = useCallback(
    (e: React.DragEvent<HTMLElement>): void => {
      if (!e.dataTransfer.types.includes("Files")) return;
      e.preventDefault();
      dragHoverCountRef.current += 1;
      if (dragHoverCountRef.current === 1) setDragHover(true);
    },
    [],
  );

  const handleCanvasDragLeave = useCallback(
    (e: React.DragEvent<HTMLElement>): void => {
      if (!e.dataTransfer.types.includes("Files")) return;
      e.preventDefault();
      dragHoverCountRef.current = Math.max(0, dragHoverCountRef.current - 1);
      if (dragHoverCountRef.current === 0) setDragHover(false);
    },
    [],
  );

  const handleCanvasDragOver = useCallback(
    (e: React.DragEvent<HTMLElement>): void => {
      if (e.dataTransfer.types.includes("Files")) {
        e.preventDefault();
        // Set the drop effect so the OS shows the "copy" cursor.
        e.dataTransfer.dropEffect = "copy";
      }
    },
    [],
  );

  const handleCanvasDrop = useCallback(
    (e: React.DragEvent<HTMLElement>): void => {
      // Always clear the hover state on drop, even if the payload is
      // empty — the OS won't emit a matching dragleave after drop.
      dragHoverCountRef.current = 0;
      setDragHover(false);
      if (!e.dataTransfer.files || e.dataTransfer.files.length === 0) return;
      e.preventDefault();
      const target = artboards[0]?.id ?? null;
      if (!target) {
        setStatusMessage("no artboard available — drop ignored");
        return;
      }
      const files = Array.from(e.dataTransfer.files);
      void (async () => {
        let imported = 0;
        const errors: string[] = [];
        for (const f of files) {
          // Electron's File exposes an absolute path. Bare-browser File
          // does not — fall back to `arrayBuffer` + importImageBytes.
          const path = (f as unknown as { path?: string }).path;
          try {
            const lower = f.name.toLowerCase();
            if (lower.endsWith(".pdf")) {
              if (!path) {
                errors.push(`${f.name}: PDF import requires a file path`);
                continue;
              }
              await window.kcreate.pdfImport.importPdf(path);
              imported += 1;
              continue;
            }
            if (
              path &&
              (lower.endsWith(".png") ||
                lower.endsWith(".jpg") ||
                lower.endsWith(".jpeg") ||
                lower.endsWith(".webp") ||
                lower.endsWith(".gif") ||
                lower.endsWith(".svg"))
            ) {
              await window.kcreate.canvas.importImage(target, path);
              imported += 1;
              continue;
            }
            if (
              lower.endsWith(".png") ||
              lower.endsWith(".jpg") ||
              lower.endsWith(".jpeg") ||
              lower.endsWith(".webp") ||
              lower.endsWith(".gif")
            ) {
              const buf = await f.arrayBuffer();
              await window.kcreate.canvas.importImageBytes(
                target,
                new Uint8Array(buf),
              );
              imported += 1;
              continue;
            }
            // SVG cannot go through importImageBytes because the
            // vector importer (usvg in kcreate_vector) requires a
            // filesystem path for relative-href resolution. Drop a
            // helpful message so the user knows to save and reopen
            // (or run in Electron) instead of silently failing.
            if (lower.endsWith(".svg")) {
              errors.push(
                `${f.name}: SVG drag-drop requires a file path (save the file and use File \u2192 Import, or use the Electron build)`,
              );
              continue;
            }
            errors.push(`${f.name}: unsupported file type`);
          } catch (err) {
            errors.push(`${f.name}: ${errorMessage(err)}`);
          }
        }
        await refreshTree();
        if (errors.length === 0) {
          setStatusMessage(`imported ${imported} file(s)`);
        } else {
          setStatusMessage(
            `imported ${imported} file(s); ${errors.length} failed: ${errors.join("; ")}`,
          );
        }
      })();
    },
    [artboards, refreshTree, setStatusMessage],
  );

  const handleSelect = useCallback(
    async (id: string | null) => {
      try {
        if (id === null) {
          await window.kcreate.canvas.clearSelection();
          setSelectedIds([]);
        } else {
          await window.kcreate.canvas.setSelection([id]);
          setSelectedIds([id]);
        }
      } catch (e) {
        setStatusMessage(`select failed: ${errorMessage(e)}`);
      }
    },
    [setStatusMessage, setSelectedIds],
  );

  const handleExport = useCallback(async () => {
    try {
      const svg = await window.kcreate.export.svg([], {
        width: CANVAS_WIDTH,
        height: CANVAS_HEIGHT,
        includeMetadata: false,
        optimize: true,
      });
      setStatusMessage(`Exported SVG · ${svg.length} bytes`);
    } catch (e) {
      setStatusMessage(`export failed: ${errorMessage(e)}`);
    }
  }, [setStatusMessage]);

  const onFrame = useCallback(() => {
    const now = performance.now();
    const elapsed = now - lastTickAtRef.current;
    lastTickAtRef.current = now;
    if (elapsed > 0) setFps(Math.round(1000 / elapsed));
  }, [setFps]);

  // Periodically resync the rendered scene from the bridge so document
  // mutations (creates, moves, deletes, undo/redo) show up on the
  // canvas. The bridge already maintains the renderer scene via
  // scene_sync — this is just the per-tick pull. Cheap because both
  // sides are in-process and the Scene struct is small.
  useEffect(() => {
    let cancelled = false;
    const tick = async (): Promise<void> => {
      if (cancelled) return;
      // No bridge API to read the scene back yet — the renderer owns
      // it. We rely on the bridge's `document_sync_scene()` having been
      // called after every mutation, so simply ask it to re-emit
      // whenever the tree-shape changes. The display-list cache makes
      // this near-free.
      try {
        await window.kcreate.canvas.syncScene();
      } catch {
        // bridge may be transient-ly closed during teardown; ignore
      }
    };
    void tick();
    return () => {
      cancelled = true;
    };
  }, [nodes]);

  // Track the active collab session's local peer id as state so
  // the presence-broadcast effect below can re-fire when a session
  // starts or stops. Without this signal, a peer-id transition
  // (e.g. user leaves session A, joins session B) wouldn't trigger
  // the broadcast effect because `selectedIds` may not have
  // changed across the transition, and the fingerprint-dedup ref
  // would silently swallow what should be the *first* broadcast
  // to session B's peers. We use `peerId` as the lifecycle marker
  // because `SessionStartReport` doesn't expose a distinct session
  // id and the local peer identity is regenerated on every
  // `session.start()` per the bridge contract.
  //
  // The state is driven directly from the bridge's session-event
  // channel — `sessionStarted` (pushed by `session_start` itself)
  // and `sessionLeft` (synthesised by `main.ts` after the bridge
  // returns the leaving peer id) cover both local-side lifecycle
  // transitions without a polling `session.info()` call. The
  // initial mount still does one `session.info()` to handle the
  // case where a session was already running before this component
  // mounted (e.g. EditorPage was unmounted/remounted across a
  // route change while the session kept going).
  const [activeSessionPeerId, setActiveSessionPeerId] = useState<
    string | null
  >(null);
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const info = await window.kcreate.session.info();
        if (!cancelled) setActiveSessionPeerId(info?.peerId ?? null);
      } catch {
        // Bridge transient (e.g. between projectClose and
        // rendererShutdown); we deliberately don't clear
        // `activeSessionPeerId` here — a transient IPC failure
        // shouldn't masquerade as a session end. The lifecycle
        // events below are the authoritative change signal.
      }
    })();
    // Event-driven lifecycle tracking. The bridge's `sessionStarted`
    // / `sessionLeft` events fire exactly when the local peer id
    // transitions, so we can set state directly from the event
    // payload — no need to re-fetch via `session.info()`. This
    // also closes the stale-state gap that the previous
    // `peerJoined`/`peerLeft`-based refresh had: a local
    // `session.leave()` emits no `peerLeft` (only remote peers
    // emit those), and `session_start` returning to the renderer
    // is not itself an event, so the only way to learn about
    // local transitions used to be polling — which created the
    // window where `useSessionLocks` and this effect's
    // dedup-fingerprint disagreed on whether a session was live.
    const unsubscribe = window.kcreate.session.onEvent((ev) => {
      if (ev.kind === "sessionStarted") {
        if (!cancelled) setActiveSessionPeerId(ev.peerId);
      } else if (ev.kind === "sessionLeft") {
        if (!cancelled) setActiveSessionPeerId(null);
      }
    });
    return () => {
      cancelled = true;
      unsubscribe();
    };
  }, []);

  // Broadcast the local user's selection (and currently `null` for
  // cursor / active page until a future change wires those up) to
  // every connected peer whenever the selection set actually
  // changes. The bridge gate-checks KChat membership before
  // serialising the beacon — outside a KChat group the
  // `sendPresence` call rejects with a typed `not in KChat group`
  // error which we silently swallow here (the bridge is the source
  // of truth for "is multiplayer enabled right now"; the renderer
  // doesn't need to duplicate that state machine).
  //
  // We only attempt the broadcast when a session is running
  // (`activeSessionPeerId !== null`). This is purely a
  // micro-optimisation — without it the renderer would call
  // `sendPresence` on every selection change and get the
  // KChat-gate rejection back, which is harmless but pollutes the
  // bridge's structured log.
  //
  // The `useEffect` dep array fires on every referentially-new
  // `selectedIds` array, including no-op renders that reuse the
  // same content (React's setState always produces a new array
  // reference). Without a value-level guard, every selection-related
  // re-render — including the implicit one on initial mount when
  // `selectedIds === []` — triggers an IPC round trip. The
  // ref-based dedup below collapses that to one IPC *per actual
  // content change*. We store the canonicalised string form of the
  // sorted id list because (a) it ignores selection order (which
  // the bridge already normalises) and (b) string equality is O(n)
  // on a short list, cheap relative to an IPC hop.
  //
  // Including `activeSessionPeerId` in the deps + resetting the
  // fingerprint ref whenever the peer id transitions ensures that
  // the *first* selection broadcast to a freshly-started session
  // always fires, even if the user's selection set is identical to
  // what they had in the previous session — without this, session B
  // would never learn the user's current selection until they
  // clicked something different. The session-ref tracks the value
  // we last keyed the fingerprint against so we only reset on
  // genuine peer-id transitions, not on every effect re-run.
  const lastBroadcastSelectionRef = useRef<string | null>(null);
  const lastBroadcastSessionRef = useRef<string | null>(null);
  useEffect(() => {
    if (lastBroadcastSessionRef.current !== activeSessionPeerId) {
      lastBroadcastSelectionRef.current = null;
      lastBroadcastSessionRef.current = activeSessionPeerId;
    }
    if (activeSessionPeerId === null) {
      return undefined;
    }
    const fingerprint = [...selectedIds].sort().join("\u001f");
    if (lastBroadcastSelectionRef.current === fingerprint) {
      return undefined;
    }
    let cancelled = false;
    const broadcast = async (): Promise<void> => {
      try {
        await window.kcreate.session.sendPresence(null, selectedIds, null);
        if (!cancelled) {
          // Only record the fingerprint after a successful broadcast,
          // so if the IPC fails (or the session is offline) we'll
          // retry the same selection on the next render rather than
          // pinning a never-delivered value.
          lastBroadcastSelectionRef.current = fingerprint;
        }
      } catch {
        // Either the gate is closed (no KChat group) or the
        // session was just torn down — either way, drop the
        // broadcast silently. Selection changes are local-first;
        // the absence of a beacon doesn't break the editor.
      }
    };
    void broadcast();
    return () => {
      cancelled = true;
    };
  }, [selectedIds, activeSessionPeerId]);

  // Keyboard shortcuts. Routed through the user-overridable
  // registry (`src/shortcuts/registry.ts`). Tool-switch actions are
  // gated by the active mode so e.g. "R" in a mode that doesn't
  // expose the rectangle tool stays a no-op instead of switching
  // to a tool that isn't in the palette.
  const tryTool = useCallback(
    (next: ToolId, e: KeyboardEvent) => {
      const tools = toolsForMode(mode);
      if (!tools.includes(next)) return;
      e.preventDefault();
      setTool(next);
    },
    [mode, setTool],
  );

  // Wrap `setStatusMessage` in a plain `(msg: string) => void` closure
  // before handing it to the state machine. `setStatusMessage` is
  // `Dispatch<SetStateAction<string | null>>`, which accepts both a
  // plain string AND a functional updater (`prev => string | null`).
  // The hook only ever calls `onError(string)`, so the assignment is
  // currently type-safe via function parameter contravariance — but
  // any future drift inside the hook (e.g. accidentally passing an
  // `Error` object whose value happens to be callable, or a closure
  // captured for some other purpose) would be silently interpreted
  // as a functional updater by React rather than a string. Narrowing
  // the prop boundary to `(msg: string) => void` here makes that
  // drift a compile error instead of a runtime surprise. Mirrors the
  // same defensive wrapper used by `EditorDocumentBridge` above.
  const onToolStateMachineError = useCallback(
    (msg: string): void => {
      setStatusMessage(msg);
    },
    [setStatusMessage],
  );
  // Pointer-event state machine. Owns the
  // (Idle | Pan | Move | Create | Pen) discriminated-union state,
  // the canvas pointer handler, and the bridge side effects
  // (hit-test, snap query, moveNode, createRect/Ellipse/Line/
  // Text/Path). See `hooks/useToolStateMachine.ts` for the
  // architectural rationale. The hook's `onAfterCommit` hooks into
  // `refreshTree` so committed drags trigger a full document re-
  // pull, exactly as the pre-refactor handler did.
  // `lastCursorWorldRef` is declared higher up (right after the
  // context destructures) so the paste closure created earlier in
  // this component captures a defined binding.
  //
  // Declared BEFORE `shortcutHandlers` so the Escape (cancel pen) /
  // Enter (commit pen) routes can call `toolStateMachine.cancelPen()`
  // / `commitPen()` directly. Moving it later was the pre-Phase-B1
  // layout — fine when the state machine only exposed
  // `onCanvasPointer`, since `CanvasHost` is rendered way below
  // `shortcutHandlers`, but it broke the moment Pen shortcuts
  // needed to call back into the machine.
  const toolStateMachine = useToolStateMachine({
    tool,
    viewport,
    panActiveRef,
    nodesRef,
    lastCursorWorldRef,
    setSelectedIds,
    setViewport,
    setSnapGuides,
    onError: onToolStateMachineError,
    onAfterCommit: refreshTree,
  });
  const onCanvasPointer = toolStateMachine.onCanvasPointer;
  // Pull the stable-identity pen callbacks out into locals so the
  // `shortcutHandlers` useMemo can list THEM in its dep array
  // instead of the whole `toolStateMachine` bundle. The bundle is
  // a fresh object every render (see hook return), so depending
  // on it would re-create `shortcutHandlers` every render and
  // make `useShortcuts` reattach its window listeners on every
  // render. Each individual callback IS `useCallback`-wrapped
  // inside the hook so their identity is stable.
  const cancelPen = toolStateMachine.cancelPen;
  const commitPen = toolStateMachine.commitPen;
  // Phase B3 — same stable-callback-destructuring discipline as
  // `cancelPen` / `commitPen`. The destructure exists so
  // `shortcutHandlers`'s dep array can list these directly
  // instead of the volatile-identity `toolStateMachine` bundle.
  const enterNodeEdit = toolStateMachine.enterNodeEdit;
  const cancelNodeEdit = toolStateMachine.cancelNodeEdit;
  const commitNodeEdit = toolStateMachine.commitNodeEdit;

  const shortcutHandlers = useMemo<ShortcutHandlers>(
    () => ({
      undo: (e) => {
        e.preventDefault();
        void handleUndo();
      },
      redo: (e) => {
        e.preventDefault();
        void handleRedo();
      },
      redoAlt: (e) => {
        e.preventDefault();
        void handleRedo();
      },
      selectAll: (e) => {
        e.preventDefault();
        void handleSelectAll();
      },
      deleteSelection: (e) => {
        e.preventDefault();
        void handleDeleteSelected();
      },
      // macOS regression fix: Backspace is the physical "delete"
      // key on Apple keyboards. We dispatch through the same
      // handler so a future change to deletion semantics only has
      // one call site to update.
      deleteSelectionAlt: (e) => {
        e.preventDefault();
        void handleDeleteSelected();
      },
      clearSelection: (e) => {
        e.preventDefault();
        // Escape semantics, in priority order:
        //   1. If a node-edit gesture is in flight, cancel it.
        //      Highest priority because nodeEdit is a modal
        //      sub-editor — the user expects Escape to exit
        //      that mode before doing anything else.
        //   2. If a pen gesture is in flight, cancel it. The
        //      user pressed Escape to abandon the path; it would
        //      be surprising if Escape also deselected an
        //      unrelated shape under that gesture.
        //   3. Otherwise clear the current selection (the
        //      pre-pen-tool behaviour).
        if (cancelNodeEdit()) return;
        if (cancelPen()) return;
        void handleClearSelection();
      },
      // Phase B1 — Enter commits the in-flight pen gesture as an
      // OPEN path. No-op when the state machine isn't in the pen
      // variant (handled inside `commitPen`), so this binding
      // can't interfere with other modes.
      // Phase B3 — Enter ALSO commits an in-flight node-edit
      // gesture (preferred ordering: node-edit first because
      // it's modal; pen second because the user has to be
      // actively in the pen tool to have a pen state). Both
      // commit functions are no-ops when their respective
      // variant is not active, so the fall-through is safe.
      commitPath: (e) => {
        e.preventDefault();
        void (async () => {
          if (await commitNodeEdit()) return;
          await commitPen();
        })();
      },
      toolSelect: (e) => tryTool("select", e),
      toolRect: (e) => tryTool("rect", e),
      toolEllipse: (e) => tryTool("ellipse", e),
      toolLine: (e) => tryTool("line", e),
      toolPen: (e) => tryTool("pen", e),
      toolText: (e) => tryTool("text", e),
      // Hold-to-pan: object-form handler so `useShortcuts`
      // dispatches both phases. `event.repeat` guards against OS
      // auto-repeat re-arming the gesture every frame, and we
      // preventDefault on keydown so Space doesn't scroll the page
      // through any host-page handler that survived our event
      // gating. Keyup unconditionally disarms — if a third party
      // (e.g. losing window focus) leaves us armed, the next Space
      // press will re-arm and the next release will clear it.
      togglePan: {
        onKeyDown: (e) => {
          if (e.repeat) return;
          e.preventDefault();
          setPanActive(true);
        },
        onKeyUp: (e) => {
          e.preventDefault();
          setPanActive(false);
        },
      },
      // Switching to Export mode swaps the right panel to the
      // ExportPanel via `defaultPanelForMode` — that's the same
      // surface the TopBar's "Export" mode tab opens, so the
      // shortcut and the UI button stay lockstep.
      openExport: (e) => {
        e.preventDefault();
        setMode("export");
      },
      openShortcutsPanel: (e) => {
        e.preventDefault();
        setShortcutsPanelOpen((open) => !open);
      },
      copy: (e) => {
        e.preventDefault();
        void handleCopy();
      },
      paste: (e) => {
        e.preventDefault();
        void handlePaste();
      },
      // Phase D — Alignment shortcuts. Same selection-cardinality
      // guards as the toolbar buttons; under-selection is a silent
      // no-op (see `handleAlign` / `handleDistribute`).
      alignLeft: (e) => {
        e.preventDefault();
        void handleAlign("left");
      },
      alignCenterX: (e) => {
        e.preventDefault();
        void handleAlign("center");
      },
      alignRight: (e) => {
        e.preventDefault();
        void handleAlign("right");
      },
      alignTop: (e) => {
        e.preventDefault();
        void handleAlign("top");
      },
      alignCenterY: (e) => {
        e.preventDefault();
        void handleAlign("middle");
      },
      alignBottom: (e) => {
        e.preventDefault();
        void handleAlign("bottom");
      },
      distributeHorizontal: (e) => {
        e.preventDefault();
        void handleDistribute("horizontal");
      },
      distributeVertical: (e) => {
        e.preventDefault();
        void handleDistribute("vertical");
      },
    }),
    // Depend on `toolStateMachine.cancelPen` / `commitPen`
    // individually rather than the whole `toolStateMachine` object.
    // The hook intentionally returns a fresh object on every render
    // (it's a bundle of stable `useCallback` refs, not a
    // `useMemo`-wrapped struct), so depending on `toolStateMachine`
    // would recreate `shortcutHandlers` every render and cause
    // `useShortcuts` to detach + reattach its window keydown / keyup
    // listeners on every render. The individual callbacks ARE stable
    // (each is `useCallback`-wrapped inside the hook), so depending
    // on them gives `shortcutHandlers` proper identity.
    [
      handleUndo,
      handleRedo,
      handleSelectAll,
      handleDeleteSelected,
      handleClearSelection,
      handleCopy,
      handlePaste,
      handleAlign,
      handleDistribute,
      tryTool,
      setMode,
      setPanActive,
      cancelPen,
      commitPen,
      cancelNodeEdit,
      commitNodeEdit,
    ],
  );
  useShortcuts(shortcutHandlers);

  const onZoomToFit = useCallback(() => {
    // No documentBounds API yet; reset to identity. Phase 1 will compute
    // a bounding box across visible nodes.
    setViewport(DEFAULT_VIEWPORT);
  }, [setViewport]);

  // Imperative handle into the AnnotationOverlay. The overlay's root
  // SVG is permanently `pointer-events: none` (so it never blocks
  // canvas tools), and the canvas-level double-click gesture is owned
  // here on `<main>` instead — when it fires we forward the
  // overlay-local screen coordinates through this ref. See
  // `AnnotationOverlayHandle` in `components/AnnotationOverlay.tsx`.
  const annotationOverlayRef = useRef<AnnotationOverlayHandle | null>(null);

  // The annotation overlay only accepts drop-pin gestures while the
  // editor is in a creation-friendly mode. Pulled out so the
  // double-click handler, the overlay's `allowCreate` prop, and the
  // CanvasHost `onZoomToFit` suppression all agree on the same
  // predicate.
  const annotationCreateActive = mode === "design" || mode === "layout";

  // Handle the canvas-area double-click. In annotation-creation modes
  // we route the gesture into the AnnotationOverlay to drop a draft
  // pin; in every other mode the default zoom-to-fit fires from
  // CanvasHost (we don't run anything here). We listen on `<main>`
  // rather than CanvasHost because the canvas surface stop-propagates
  // some events for native viewport gestures — `<main>` is the
  // single point that sees every dblclick within the canvas pane.
  const onMainDoubleClick = useCallback(
    (e: ReactMouseEvent<HTMLElement>) => {
      const rect = e.currentTarget.getBoundingClientRect();
      const localX = e.clientX - rect.left;
      const localY = e.clientY - rect.top;
      if (annotationCreateActive) {
        annotationOverlayRef.current?.beginDraftAt(localX, localY);
        return;
      }
      // Phase A1 — when the user double-clicks a `TextLayer`, mount
      // the inline canvas editor over its bounding box. Hit-test
      // through the bridge (same coordinate convention as the
      // pointer-down handler above) so we agree with what the
      // renderer is drawing. World-space bounds come from the
      // existing `nodes` mirror; we project through the live
      // viewport to position the editor.
      // Phase B3 — when the user double-clicks a `VectorLayer`,
      // enter the node editor for that node instead (mounting an
      // overlay full of anchor/handle widgets via the state
      // machine's `nodeEdit` variant). The two are
      // mutually-exclusive (a node is either TextLayer OR
      // VectorLayer) so the branch order doesn't matter for
      // correctness; we test `TextLayer` first to preserve the
      // pre-Phase-B3 hit ordering for that path.
      void (async () => {
        try {
          const hit = await window.kcreate.canvas.hitTest(
            localX,
            localY,
            viewport.panX,
            viewport.panY,
            viewport.zoom,
          );
          if (!hit) return;
          const node = nodes.find((n) => n.id === hit);
          if (!node) return;
          if (node.nodeType === "VectorLayer") {
            await enterNodeEdit(hit);
            return;
          }
          if (node.nodeType !== "TextLayer") return;
          const [style, content] = await Promise.all([
            window.kcreate.text.getStyle(hit),
            window.kcreate.text.getContent(hit),
          ]);
          const screenX = node.bounds.x * viewport.zoom + viewport.panX;
          const screenY = node.bounds.y * viewport.zoom + viewport.panY;
          const screenW = node.bounds.width * viewport.zoom;
          const screenH = node.bounds.height * viewport.zoom;
          const nextDraft: InlineTextEditState = {
            nodeId: hit,
            rect: {
              x: screenX,
              y: screenY,
              width: Math.max(screenW, 32),
              height: Math.max(screenH, style.fontSize * style.lineHeight),
            },
            style,
            initialContent: content,
          };
          // Update the ref BEFORE scheduling the state update so any
          // commit captured between this tick and the next React
          // render flush sees the new draft identity — without this,
          // a commit that resolves in the gap between `setState` and
          // the lockstep effect could still null the new editor.
          inlineTextEditRef.current = nextDraft;
          setInlineTextEdit(nextDraft);
        } catch (err) {
          setStatusMessage(
            `Inline text edit failed: ${errorMessage(err)}`,
          );
        }
      })();
    },
    [annotationCreateActive, nodes, viewport, inlineTextEditRef, setInlineTextEdit, setStatusMessage, enterNodeEdit],
  );

  // Commit / cancel handlers for the inline text editor. Commit
  // uses `replaceRange(0, length, next)` so the operation log
  // records a single splice (matches what the bridge would do for
  // a future remote-peer text-edit; the operation kind is the same
  // as for partial edits, which keeps the undo replay tidy).
  // Indices are UTF-16 code units — same as JavaScript string
  // length — so we can pass `initialContent.length` directly.
  //
  // The captured `draft` is the editor instance this commit owns;
  // `inlineTextEditRef.current` is the editor currently mounted on
  // screen. They diverge when the user double-clicks a different
  // TextLayer while the prior commit's `replaceRange` round-trip is
  // still in flight — in that case the new editor has already
  // taken over the draft slot and the prior commit must NOT null
  // it, or the new editor flashes closed. The identity check in
  // `finally` enforces this.
  const commitInlineTextEdit = useCallback(
    async (next: string) => {
      const draft = inlineTextEditRef.current;
      if (!draft) return;
      try {
        await window.kcreate.text.replaceRange(
          draft.nodeId,
          0,
          draft.initialContent.length,
          next,
        );
        setStatusMessage("Text updated.");
      } catch (err) {
        setStatusMessage(`Text update failed: ${errorMessage(err)}`);
      } finally {
        if (inlineTextEditRef.current === draft) {
          setInlineTextEdit(null);
        }
      }
    },
    [inlineTextEditRef, setInlineTextEdit, setStatusMessage],
  );

  // Cancel always nulls — the user explicitly dismissed the
  // editor (Escape key, blur, etc.), so the latest draft is the
  // one being cancelled regardless of any in-flight commit.
  const cancelInlineTextEdit = useCallback(() => {
    setInlineTextEdit(null);
  }, [setInlineTextEdit]);

  const handleUpdateNode = useCallback(
    async (
      nodeId: string,
      changes: Parameters<
        typeof window.kcreate.document.updateNode
      >[1],
    ) => {
      try {
        await window.kcreate.document.updateNode(nodeId, changes);
        await refreshTree();
      } catch (err) {
        setStatusMessage(`update failed: ${errorMessage(err)}`);
      }
    },
    [refreshTree, setStatusMessage],
  );

  // Right-panel content depends on the active mode (see PROPOSAL § 6).
  // Image mode focuses on AI Assist; Export mode focuses on Export
  // presets; everything else lands on the properties inspector.
  const rightPanelFocus = defaultPanelForMode(mode);
  // Hold-to-pan flips the canvas cursor to `grab` while armed and
  // `grabbing` while the pan drag is in flight. `grabbing` is keyed
  // off the live drag-state ref (not panActive) so a release of the
  // pointer button without releasing the key drops back to `grab`
  // immediately. Falls back to the per-tool cursor when the gesture
  // isn't armed.
  const cursor = panActive
    ? toolStateMachine.getState().kind === "pan"
      ? "grabbing"
      : "grab"
    : TOOL_CURSORS[tool];

  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        height: "100%",
        fontFamily: font.family,
        color: colors.text,
        background: colors.bgSoft,
      }}
    >
      <TopBar
        projectName={project.name}
        mode={mode}
        onModeChange={setMode}
        tool={tool}
        onToolChange={setTool}
        canUndo={canUndo}
        canRedo={canRedo}
        onUndo={() => {
          void handleUndo();
        }}
        onRedo={() => {
          void handleRedo();
        }}
        onExport={() => {
          void handleExport();
        }}
        onBackHome={onBackHome}
      />
      <div
        style={{
          flex: 1,
          display: "grid",
          gridTemplateColumns:
            mode === "layout" ? "auto auto 1fr auto" : "auto 1fr auto",
          minHeight: 0,
        }}
      >
        {mode === "layout" ? (
          <PageNavigator
            nodes={nodes}
            selectedPageId={selectedId}
            onSelectPage={(id) => {
              void handleSelectPage(id);
            }}
            onStatus={setStatusMessage}
            onChanged={() => {
              void refreshTree();
            }}
            onNewFromTemplate={() => setTemplatePickerOpen(true)}
            onImportPdf={() => {
              void (async () => {
                try {
                  const path = await window.kcreate.pdfImport.pickFile();
                  if (!path) return;
                  setStatusMessage(`Importing ${path}…`);
                  const report = await window.kcreate.pdfImport.importPdf(
                    path,
                  );
                  await refreshTree();
                  const pieces: string[] = [];
                  pieces.push(
                    `Imported ${report.pageIds.length} page${
                      report.pageIds.length === 1 ? "" : "s"
                    }`,
                  );
                  if (report.imagesImported > 0) {
                    pieces.push(`${report.imagesImported} image${
                      report.imagesImported === 1 ? "" : "s"
                    }`);
                  }
                  if (report.imagesSkipped > 0) {
                    pieces.push(`${report.imagesSkipped} image${
                      report.imagesSkipped === 1 ? "" : "s"
                    } skipped`);
                  }
                  setStatusMessage(pieces.join(", "));
                  if (report.warnings.length > 0) {
                    console.warn("PDF import warnings:", report.warnings);
                  }
                  const firstPageId = report.pageIds[0];
                  if (firstPageId) {
                    await handleSelectPage(firstPageId);
                  }
                } catch (e) {
                  setStatusMessage(`PDF import failed: ${errorMessage(e)}`);
                }
              })();
            }}
          />
        ) : null}
        <LeftPanel
          nodes={nodes}
          selectedId={selectedId}
          onSelect={(id) => {
            void handleSelect(id);
          }}
          onToggleVisibility={(id, visible) => {
            void handleUpdateNode(id, { visible });
          }}
          onToggleLocked={(id, locked) => {
            void handleUpdateNode(id, { locked });
          }}
          onRename={(id, name) => {
            void handleUpdateNode(id, { name });
          }}
          onDelete={(id) => {
            void (async () => {
              try {
                await window.kcreate.document.deleteNode(id);
                await refreshTree();
              } catch (err) {
                setStatusMessage(`delete failed: ${errorMessage(err)}`);
              }
            })();
          }}
          onDuplicateNode={(id) => {
            void handleDuplicateLayer(id);
          }}
          onSelectMany={(ids) => {
            void (async () => {
              try {
                await window.kcreate.canvas.setSelection(ids);
                await refreshSelection();
              } catch (err) {
                setStatusMessage(
                  `select group failed: ${errorMessage(err)}`,
                );
              }
            })();
          }}
          onSetLayerColor={(id, color) => {
            void (async () => {
              try {
                await window.kcreate.document.setLayerColor(id, color);
                await refreshTree();
              } catch (err) {
                setStatusMessage(
                  `set layer colour failed: ${errorMessage(err)}`,
                );
              }
            })();
          }}
          artboards={artboards}
          onRequestCreateArtboard={() => setArtboardDialogOpen(true)}
          onFocusArtboard={focusArtboard}
          onRenameArtboard={(id, name) => {
            void handleRenameArtboard(id, name);
          }}
          onDuplicateArtboard={(id) => {
            void handleDuplicateArtboard(id);
          }}
          onResizeArtboard={(id, w, h) => {
            void handleResizeArtboard(id, w, h);
          }}
          onDeleteArtboard={(id) => {
            void handleDeleteArtboard(id);
          }}
          selectedIds={selectedIds}
          components={components}
          onComponentCreateFromSelection={(name) => {
            void handleComponentCreateFromSelection(name);
          }}
          onComponentInstantiate={(id) => {
            void handleComponentInstantiate(id);
          }}
          onComponentAddVariant={(id, name) => {
            void handleComponentAddVariant(id, name);
          }}
          onComponentSwitchVariant={(nodeId, variantId) => {
            void handleComponentSwitchVariant(nodeId, variantId);
          }}
          onComponentDetach={(id) => {
            void handleComponentDetach(id);
          }}
          onDesignSystemStatus={setStatusMessage}
        />
        <main
          onDragEnter={handleCanvasDragEnter}
          onDragLeave={handleCanvasDragLeave}
          onDragOver={handleCanvasDragOver}
          onDrop={handleCanvasDrop}
          onDoubleClick={onMainDoubleClick}
          style={{
            position: "relative",
            background: colors.bgCanvas,
            minWidth: 0,
            overflow: "hidden",
          }}
          data-testid="kcreate-canvas-main"
          data-drag-hover={dragHover ? "true" : "false"}
        >
          <CanvasHost
            width={CANVAS_WIDTH}
            height={CANVAS_HEIGHT}
            scene={scene}
            viewport={viewport}
            onViewportChange={setViewport}
            onFramePresented={onFrame}
            onPointer={onCanvasPointer}
            // In annotation-creation modes the double-click gesture
            // is reserved for dropping a new annotation pin (handled
            // via `onMainDoubleClick` on the parent `<main>` and
            // forwarded into `AnnotationOverlay` via the imperative
            // handle). Suppress the default zoom-to-fit so the two
            // behaviours don't fire on the same gesture.
            onZoomToFit={annotationCreateActive ? undefined : onZoomToFit}
            cursor={cursor}
          />
          {/*
            Phase 5 Block C Task 14 — smart-guides overlay. World-space
            guides from the snap engine are projected through the
            current viewport (`screen = world * zoom + pan`) and
            rendered as 1px dashed magenta lines. The overlay sits on
            top of the canvas but is `pointer-events: none` so it
            doesn't intercept clicks. Cleared on pointerup.
          */}
          <SnapGuidesOverlay
            guides={snapGuides}
            viewport={viewport}
            width={CANVAS_WIDTH}
            height={CANVAS_HEIGHT}
          />
          {/*
            Phase B1 — pen tool overlay. Renders in-flight anchors,
            handles, and the rubber-band preview from the last
            committed anchor to the cursor while the user is laying
            down a path. Returns `null` (and renders nothing) when
            the state machine is not in the `"pen"` variant, so this
            adds zero DOM weight when the pen tool isn't active.
            Pointer-events: none — the pen tool's pointer handler
            is wired directly on `CanvasHost` via `onCanvasPointer`.
          */}
          <PenOverlay
            machine={toolStateMachine}
            viewport={viewport}
            width={CANVAS_WIDTH}
            height={CANVAS_HEIGHT}
          />
          {/*
            Phase B3 — Node-editor overlay. Renders the in-flight
            node-edit gesture (path outline + anchor squares +
            handle dots + tangent lines) when the state machine is
            in the `"nodeEdit"` variant; returns `null` otherwise.
            Pointer-events: none — pointer handling for anchor /
            handle drag is wired directly on `CanvasHost` via
            `onCanvasPointer`, same as `PenOverlay`.
          */}
          <NodeEditOverlay
            machine={toolStateMachine}
            viewport={viewport}
            width={CANVAS_WIDTH}
            height={CANVAS_HEIGHT}
          />
          {/*
            Phase B2 — Pathfinder boolean-op panel. Floating
            pill at the bottom-centre of the canvas with the
            four boolean ops (Union / Subtract / Intersect /
            Exclude). The panel renders `null` when fewer than
            two `VectorLayer` nodes are selected, so it costs
            zero DOM weight in the (common) inactive case.
            On success it pushes the new result ids back into
            the selection and refreshes the tree, matching the
            "destructive replace" semantics the bridge
            implements (sources are removed, results take
            their place).
          */}
          <PathfinderPanel
            selectedIds={selectedIds}
            nodes={nodes}
            onStatus={setStatusMessage}
            onApplied={handlePathfinderApplied}
          />
          {/*
            Phase 7 Task 14 — remote-peer selection outlines. Coloured
            dashed rectangles around every node that any remote peer
            has currently selected. Same peer-colour assignment as the
            cursor overlay so it's obvious who's editing what. Renders
            nothing when no remote selection is live.
          */}
          <SelectionOverlay
            width={CANVAS_WIDTH}
            height={CANVAS_HEIGHT}
            viewport={viewport}
            nodes={nodes}
          />
          {/*
            Phase 7 Task 13 — remote-peer cursors. Coloured arrows +
            display-name pills at each remote peer's last-broadcast
            world position, projected through the local viewport.
            Same peer-colour palette as SelectionOverlay so the two
            overlays agree visually. Renders nothing in solo mode.
          */}
          <CursorOverlay
            width={CANVAS_WIDTH}
            height={CANVAS_HEIGHT}
            viewport={viewport}
          />
          {/*
            Phase 7 Task 16 — conflict resolution toast. Listens for
            `conflictResolved` session events and surfaces a brief
            "<peer> overrode your edit" toast in the bottom-right
            when the local peer was the loser. Clicking the toast
            triggers a local undo so the user can quickly revert.
            Self-contained — owns its own subscription + roster.
          */}
          <ConflictToast nodes={nodes} />
          {/*
            Phase 8 Task 5 — design-review annotation pins. Root SVG
            is permanently `pointer-events: none` so it never blocks
            canvas tools; pin children opt back in with
            `pointer-events: auto` so they remain clickable, and the
            host owns the double-click drop-pin gesture via the
            imperative `AnnotationOverlayHandle` ref forwarded
            through `onMainDoubleClick` on the parent `<main>`. The
            overlay returns `null` until a page is mounted (the
            bridge needs a page id to scope annotation reads/writes).
          */}
          <AnnotationOverlay
            ref={annotationOverlayRef}
            width={CANVAS_WIDTH}
            height={CANVAS_HEIGHT}
            viewport={viewport}
            pageId={activePageId}
            project={project}
            allowCreate={annotationCreateActive}
          />
          {/*
            Phase 2 soft-proof / gamut-warning overlay. Reads the
            project's color settings via `window.kcreate.color` and
            renders a CSS-filter wash on top of the canvas. Renders
            nothing when both features are disabled, so the offscreen
            wgpu path remains the source of truth for exports.
          */}
          <SoftProofOverlay />
          {inlineTextEdit ? (
            <InlineTextEditor
              nodeId={inlineTextEdit.nodeId}
              rect={inlineTextEdit.rect}
              style={inlineTextEdit.style}
              initialContent={inlineTextEdit.initialContent}
              onCommit={(next) => {
                void commitInlineTextEdit(next);
              }}
              onCancel={cancelInlineTextEdit}
            />
          ) : null}
          <div
            style={{
              position: "absolute",
              top: spacing.sm,
              right: spacing.sm,
              background: "rgba(17, 24, 39, 0.7)",
              color: colors.textInverse,
              fontSize: 11,
              padding: "2px 8px",
              borderRadius: 4,
            }}
          >
            {fps} fps · {mode} · {tool} · {Math.round(viewport.zoom * 100)}%
          </div>
          {mode === "prototype" ? (
            <div
              style={{
                position: "absolute",
                inset: 0,
                background: "rgba(17, 24, 39, 0.92)",
                overflow: "auto",
                display: "flex",
                flexDirection: "column",
              }}
            >
              <div
                style={{
                  display: "flex",
                  justifyContent: "flex-end",
                  padding: spacing.sm,
                  gap: spacing.sm,
                }}
              >
                <button
                  type="button"
                  onClick={() => setPrototypePlaying(true)}
                  style={{
                    padding: "6px 14px",
                    fontSize: 12,
                    fontWeight: 600,
                    background: colors.accent,
                    color: colors.textInverse,
                    border: `1px solid ${colors.accent}`,
                    borderRadius: 9999,
                    cursor: "pointer",
                  }}
                >
                  ▶ Play
                </button>
              </div>
              <div style={{ flex: 1, overflow: "auto" }}>
                <ResponsivePreview onStatus={setStatusMessage} />
              </div>
            </div>
          ) : null}
          <PrototypePlayer
            open={prototypePlaying}
            tree={nodes}
            artboards={artboards}
            startArtboardId={
              selected && selected.nodeType === "Artboard" ? selected.id : null
            }
            onClose={handlePrototypeClose}
          />
          {/*
            Phase D — drag-hover overlay. Shows a dashed border and a
            hint label while the user holds a file payload over the
            canvas surface. `pointer-events: none` so the underlying
            drop target keeps receiving dragover/drop events. Hidden
            (not unmounted) when not hovering so CSS transitions can
            ease in/out cleanly if a future style step adds them.
          */}
          {dragHover ? (
            <div
              data-testid="kcreate-drag-hover-overlay"
              role="presentation"
              style={{
                position: "absolute",
                inset: 0,
                pointerEvents: "none",
                background: "rgba(59, 130, 246, 0.08)",
                border: "2px dashed rgba(59, 130, 246, 0.6)",
                borderRadius: 4,
                display: "flex",
                alignItems: "center",
                justifyContent: "center",
                zIndex: 50,
              }}
            >
              <div
                style={{
                  background: colors.bg,
                  border: `1px solid ${colors.border}`,
                  borderRadius: 6,
                  padding: `${spacing.sm}px ${spacing.md}px`,
                  fontSize: 13,
                  fontWeight: 600,
                  color: colors.text,
                  boxShadow: "0 4px 12px rgba(0,0,0,0.12)",
                }}
              >
                Drop files to import (PNG, JPG, WebP, GIF, SVG, PDF)
              </div>
            </div>
          ) : null}
        </main>
        {rightPanelFocus === "ai" ? (
          <AIAssistPanel
            selectedNode={selected}
            onApplied={() => {
              void refreshTree();
            }}
            onStatus={setStatusMessage}
          />
        ) : rightPanelFocus === "export" ? (
          <ExportPanel
            onStatus={setStatusMessage}
            width={CANVAS_WIDTH}
            height={CANVAS_HEIGHT}
            selectedIds={selectedIds}
          />
        ) : (
          <RightPanel
            selected={selected}
            selectedIds={selectedIds}
            onAlignmentApplied={() => {
              void refreshTree();
            }}
            onChange={(changes) => {
              if (!selected) return;
              void handleUpdateNode(selected.id, changes);
            }}
            onRequestExport={() => {
              void handleExport();
            }}
            layout={layoutHandlers}
            mode={mode}
            onStatus={setStatusMessage}
            onSelectNode={(id) => {
              void handleSelect(id);
            }}
            artboards={artboards.map((a) => ({ id: a.id, name: a.name }))}
            tree={nodes}
            onInteractionsChanged={() => {
              void refreshTree();
            }}
            project={project}
          />
        )}
      </div>
      {resourceLimits ? (
        <LowResourceBanner
          limits={resourceLimits}
          onToggle={handleToggleLowResource}
        />
      ) : null}
      <footer
        style={{
          padding: `${spacing.xs}px ${spacing.md}px`,
          borderTop: `1px solid ${colors.border}`,
          background: colors.bg,
          fontSize: 11,
          color: colors.textMuted,
          display: "flex",
          gap: spacing.md,
          minHeight: 22,
        }}
      >
        <span>{statusMessage ?? `Project: ${project.path}`}</span>
        <span style={{ marginLeft: "auto" }}>
          {selectedIds.length === 0
            ? "No selection"
            : `${selectedIds.length} selected`}
        </span>
      </footer>
      <ArtboardDialog
        open={artboardDialogOpen}
        presets={artboardPresets}
        onCreate={(args) => {
          void handleCreateArtboard(args);
        }}
        onClose={() => setArtboardDialogOpen(false)}
      />
      <TemplatePicker
        open={templatePickerOpen}
        onClose={() => setTemplatePickerOpen(false)}
        onApplied={(ids) => {
          // Refresh the tree so the new pages appear in the navigator,
          // and focus the first new page if we got any.
          void (async () => {
            await refreshTree();
            const first = ids[0];
            if (first) await handleSelectPage(first);
          })();
        }}
        onStatus={setStatusMessage}
      />
      {shortcutsPanelOpen ? (
        <div
          role="dialog"
          aria-modal="true"
          aria-label="Keyboard shortcuts"
          onClick={(e) => {
            if (e.target === e.currentTarget) {
              setShortcutsPanelOpen(false);
            }
          }}
          style={{
            position: "fixed",
            inset: 0,
            background: "rgba(17, 24, 39, 0.45)",
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            zIndex: 50,
          }}
        >
          <div
            style={{
              width: "min(720px, 90vw)",
              maxHeight: "80vh",
              overflowY: "auto",
              background: colors.bg,
              borderRadius: 12,
              boxShadow: "0 12px 32px rgba(0, 0, 0, 0.24)",
            }}
          >
            <KeyboardShortcutsPanel onStatus={setStatusMessage} />
          </div>
        </div>
      ) : null}
    </div>
  );
}



interface SnapGuidesOverlayProps {
  guides: SnapGuide[];
  viewport: ViewportState;
  width: number;
  height: number;
}

/// Phase 5 Block C Task 14. Projects world-space `SnapGuide` lines
/// through the canvas viewport (`screen = world * zoom + pan`) and
/// renders them as a 1 px dashed magenta line plus a small distance
/// label at the midpoint. Pointer events are disabled so the overlay
/// can sit above the canvas without intercepting drags.
function SnapGuidesOverlay({
  guides,
  viewport,
  width,
  height,
}: SnapGuidesOverlayProps): JSX.Element | null {
  if (guides.length === 0) {
    return null;
  }
  return (
    <svg
      width={width}
      height={height}
      style={{
        position: "absolute",
        top: 0,
        left: 0,
        pointerEvents: "none",
      }}
    >
      {guides.map((g, i) => {
        if (g.axis === "Vertical") {
          const x = g.position * viewport.zoom + viewport.panX;
          const y1 = g.from * viewport.zoom + viewport.panY;
          const y2 = g.to * viewport.zoom + viewport.panY;
          return (
            <line
              key={`v-${i}`}
              x1={x}
              y1={y1}
              x2={x}
              y2={y2}
              stroke="#ff00ff"
              strokeWidth={1}
              strokeDasharray="4 2"
            />
          );
        }
        const y = g.position * viewport.zoom + viewport.panY;
        const x1 = g.from * viewport.zoom + viewport.panX;
        const x2 = g.to * viewport.zoom + viewport.panX;
        return (
          <line
            key={`h-${i}`}
            x1={x1}
            y1={y}
            x2={x2}
            y2={y}
            stroke="#ff00ff"
            strokeWidth={1}
            strokeDasharray="4 2"
          />
        );
      })}
    </svg>
  );
}
