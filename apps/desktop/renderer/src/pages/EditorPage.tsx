import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { CanvasHost, type ViewportState } from "../components/CanvasHost";
import { LeftPanel } from "../components/LeftPanel";
import { PageNavigator } from "../components/PageNavigator";
import { RightPanel } from "../components/RightPanel";
import { SoftProofOverlay } from "../components/SoftProofOverlay";
import { TemplatePicker } from "../components/TemplatePicker";
import { KeyboardShortcutsPanel } from "../components/KeyboardShortcutsPanel";
import {
  TopBar,
  type EditorMode,
  toolsForMode,
  defaultPanelForMode,
} from "../components/TopBar";
import { AIAssistPanel } from "../components/AIAssistPanel";
import { ExportPanel } from "../components/ExportPanel";
import { ArtboardDialog } from "../components/ArtboardDialog";
import { ResponsivePreview } from "../components/ResponsivePreview";
import { PrototypePlayer } from "../components/PrototypePlayer";
import type {
  ArtboardInfo,
  ArtboardPreset,
  ComponentInfo,
  DocumentStatus,
  FlexLayout,
  GridLayout,
  NodeInfo,
  ProjectInfo,
  ResourceLimits,
  Scene,
  SnapGuide,
} from "../../../shared/scene";
import { LowResourceBanner } from "../components/LowResourceBanner";
import { useShortcuts } from "../shortcuts/useShortcuts";
import type { ShortcutHandlers } from "../shortcuts/useShortcuts";
import { colors, font, spacing } from "../styles/tokens";

export interface EditorPageProps {
  project: ProjectInfo;
  onBackHome: () => void;
}

/// Active drawing/selection tool. The selected tool drives both the
/// canvas cursor and the click→action wiring.
export type ToolId = "select" | "rect" | "ellipse" | "line" | "text";

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

const DEFAULT_VIEWPORT: ViewportState = { panX: 0, panY: 0, zoom: 1 };

/// Snap threshold in world units. 6 px @ zoom=1 keeps snaps tight
/// enough to feel deliberate but forgiving on a 4K display where the
/// cursor is travelling at high pixel velocity.
const SNAP_THRESHOLD_WORLD = 6;

/// Empty scene used while we haven't yet pulled one from the bridge.
const EMPTY_SCENE: Scene = {
  clear_color: [0.12, 0.12, 0.14, 1.0],
  objects: [],
};

const TOOL_CURSORS: Record<ToolId, string> = {
  select: "default",
  rect: "crosshair",
  ellipse: "crosshair",
  line: "crosshair",
  text: "text",
};

export function EditorPage({
  project,
  onBackHome,
}: EditorPageProps): JSX.Element {
  const [mode, setMode] = useState<EditorMode>("design");
  const [tool, setTool] = useState<ToolId>("select");
  const [nodes, setNodes] = useState<NodeInfo[]>([]);
  const [selectedIds, setSelectedIds] = useState<string[]>([]);
  // Active smart-guides for the in-flight drag, in world space.
  // Cleared on pointerup. Rendered as an SVG overlay positioned
  // above the canvas (see the `<svg>` in the canvas pane below).
  const [snapGuides, setSnapGuides] = useState<SnapGuide[]>([]);
  const [statusMessage, setStatusMessage] = useState<string | null>(null);
  const [fps, setFps] = useState<number>(0);
  const [viewport, setViewport] = useState<ViewportState>(DEFAULT_VIEWPORT);
  // The document graph lives in Rust; we only keep a sampled `Scene`
  // snapshot here for the renderer. Phase 1 will swap this for a
  // push-based subscription rather than periodic resync. We don't
  // currently rebuild the scene client-side (the Rust scene_sync
  // pushes into the renderer directly), so this is a stable empty
  // sentinel today.
  const [scene] = useState<Scene>(EMPTY_SCENE);
  const [docStatus, setDocStatus] = useState<DocumentStatus | null>(null);
  const [artboards, setArtboards] = useState<ArtboardInfo[]>([]);
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
  const [artboardPresets, setArtboardPresets] = useState<ArtboardPreset[]>(
    [],
  );
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
  const [components, setComponents] = useState<ComponentInfo[]>([]);
  const [resourceLimits, setResourceLimits] = useState<ResourceLimits | null>(
    null,
  );
  const lastTickAtRef = useRef<number>(performance.now());
  // Drag-to-create / drag-to-move state. Storing in a ref keeps the
  // pointer handler stable while still tracking the current drag.
  const dragStateRef = useRef<{
    kind: "create" | "move";
    tool: ToolId;
    pointerId: number;
    startWorldX: number;
    startWorldY: number;
    lastWorldX: number;
    lastWorldY: number;
    movingNodeId: string | null;
    cumulativeDx: number;
    cumulativeDy: number;
  } | null>(null);

  // Phase 6 Tasks 25-26: latest world-space cursor sample. Paste uses
  // it to position the new subtree near the cursor; we update on every
  // pointer event (not just down/up) so a stationary cursor over the
  // canvas still drives a sensible paste origin.
  const lastCursorWorldRef = useRef<{ x: number; y: number } | null>(null);

  /// `nodes` mirror used inside the canvas pointer handler. The
  /// handler is wrapped in `useCallback` and we deliberately do NOT
  /// add `nodes` to its deps array (re-creating the callback on every
  /// node mutation would cancel any in-flight drag). The ref is the
  /// idiomatic React workaround for "read latest value inside a
  /// stable callback".
  const nodesRef = useRef<NodeInfo[]>(nodes);
  useEffect(() => {
    nodesRef.current = nodes;
  }, [nodes]);

  const selectedId: string | null =
    selectedIds.length === 1 ? (selectedIds[0] ?? null) : null;

  const refreshStatus = useCallback(async () => {
    try {
      const s = await window.kcreate.document.status();
      setDocStatus(s);
    } catch (e) {
      setStatusMessage(`status probe failed: ${errorMessage(e)}`);
    }
  }, []);

  const refreshSelection = useCallback(async () => {
    try {
      const sel = await window.kcreate.canvas.getSelection();
      setSelectedIds(sel);
    } catch (e) {
      setStatusMessage(`selection probe failed: ${errorMessage(e)}`);
    }
  }, []);

  const refreshArtboards = useCallback(async () => {
    try {
      const list = await window.kcreate.artboard.list();
      setArtboards(list);
    } catch (e) {
      setStatusMessage(`artboard list failed: ${errorMessage(e)}`);
    }
  }, []);

  const refreshComponents = useCallback(async () => {
    try {
      const list = await window.kcreate.component.list();
      setComponents(list);
    } catch (e) {
      setStatusMessage(`component list failed: ${errorMessage(e)}`);
    }
  }, []);

  const refreshTree = useCallback(async () => {
    try {
      const tree = await window.kcreate.document.getDocumentTree();
      setNodes(tree);
    } catch (e) {
      setStatusMessage(`tree load failed: ${errorMessage(e)}`);
    }
    await refreshStatus();
    await refreshSelection();
    await refreshArtboards();
    await refreshComponents();
  }, [refreshStatus, refreshSelection, refreshArtboards, refreshComponents]);

  // Initial load + on-mode-change resync.
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
  }, [project.id]);

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
    [],
  );

  const refreshResourceLimits = useCallback(async () => {
    try {
      const limits = await window.kcreate.runtime.resourceLimits();
      setResourceLimits(limits);
    } catch (e) {
      setStatusMessage(`resource limits failed: ${errorMessage(e)}`);
    }
  }, []);

  const handleToggleLowResource = useCallback(
    async (enabled: boolean) => {
      try {
        await window.kcreate.runtime.lowResourceModeSet(enabled);
        await refreshResourceLimits();
      } catch (e) {
        setStatusMessage(`toggle low-resource failed: ${errorMessage(e)}`);
      }
    },
    [refreshResourceLimits],
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
  }, []);

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
  }, [refreshSelection]);

  const handleCreateArtboard = useCallback(
    async (args: { name: string; width: number; height: number }) => {
      try {
        const id = await window.kcreate.artboard.create(
          null,
          args.name,
          args.width,
          args.height,
        );
        await refreshTree();
        const list = await window.kcreate.artboard.list();
        setArtboards(list);
        const created = list.find((a) => a.id === id);
        if (created) focusArtboard(created);
      } catch (e) {
        setStatusMessage(`create artboard failed: ${errorMessage(e)}`);
      } finally {
        setArtboardDialogOpen(false);
      }
    },
    [refreshTree, focusArtboard],
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
    [refreshTree],
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
    [refreshTree],
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
    [refreshTree],
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
    [refreshTree],
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
    [selectedIds, refreshTree],
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
    [artboards, refreshTree],
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
    [refreshComponents],
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
    [refreshTree],
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
    [refreshTree],
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
    [refreshTree],
  );

  // When the mode changes, snap to its default tool so the canvas
  // cursor and toolbar stay aligned.
  useEffect(() => {
    const tools = toolsForMode(mode);
    if (!tools.includes(tool)) {
      setTool(tools[0] ?? "select");
    }
  }, [mode, tool]);

  const selected = useMemo(
    () => nodes.find((n) => n.id === selectedId) ?? null,
    [nodes, selectedId],
  );

  const canUndo = docStatus?.canUndo ?? false;
  const canRedo = docStatus?.canRedo ?? false;

  const handleUndo = useCallback(async () => {
    try {
      await window.kcreate.document.undo();
      await refreshTree();
    } catch (e) {
      setStatusMessage(`undo failed: ${errorMessage(e)}`);
    }
  }, [refreshTree]);

  const handleRedo = useCallback(async () => {
    try {
      await window.kcreate.document.redo();
      await refreshTree();
    } catch (e) {
      setStatusMessage(`redo failed: ${errorMessage(e)}`);
    }
  }, [refreshTree]);

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
  }, [selectedIds, refreshTree]);

  const handleSelectAll = useCallback(async () => {
    try {
      const ids = nodes.map((n) => n.id);
      await window.kcreate.canvas.setSelection(ids);
      await refreshSelection();
    } catch (e) {
      setStatusMessage(`select all failed: ${errorMessage(e)}`);
    }
  }, [nodes, refreshSelection]);

  const handleClearSelection = useCallback(async () => {
    try {
      await window.kcreate.canvas.clearSelection();
      setSelectedIds([]);
    } catch (e) {
      setStatusMessage(`clear selection failed: ${errorMessage(e)}`);
    }
  }, []);

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

  const handleCopy = useCallback(async () => {
    if (selectedIds.length === 0) return;
    try {
      // Filter selected ids that are themselves Pages / Artboards.
      // The bridge filters defensively too, but doing it here avoids a
      // surprising "you copied something but the clipboard is empty"
      // when the user has only top-level container ids selected.
      const eligible = selectedIds.filter((id) => {
        const n = nodes.find((node) => node.id === id);
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
  }, [selectedIds, nodes]);

  const handlePaste = useCallback(async () => {
    try {
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
      if (selectedIds.length > 0) {
        const first = nodes.find((n) => n.id === selectedIds[0]);
        // Walk up to find an Artboard ancestor.
        let cursor: NodeInfo | undefined = first;
        while (cursor && cursor.nodeType !== "Artboard") {
          cursor = cursor.parentId
            ? nodes.find((n) => n.id === cursor!.parentId)
            : undefined;
        }
        if (cursor && cursor.nodeType === "Artboard") {
          targetArtboard = cursor.id;
        }
      }
      if (!targetArtboard) {
        targetArtboard = artboards[0]?.id ?? null;
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
  }, [selectedIds, nodes, artboards, refreshTree]);

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
              lower.endsWith(".webp")
            ) {
              const buf = await f.arrayBuffer();
              await window.kcreate.canvas.importImageBytes(
                target,
                new Uint8Array(buf),
              );
              imported += 1;
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
    [artboards, refreshTree],
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
    [],
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
  }, []);

  const onFrame = useCallback(() => {
    const now = performance.now();
    const elapsed = now - lastTickAtRef.current;
    lastTickAtRef.current = now;
    if (elapsed > 0) setFps(Math.round(1000 / elapsed));
  }, []);

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
    [mode],
  );
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
      clearSelection: (e) => {
        e.preventDefault();
        void handleClearSelection();
      },
      toolSelect: (e) => tryTool("select", e),
      toolRect: (e) => tryTool("rect", e),
      toolEllipse: (e) => tryTool("ellipse", e),
      toolLine: (e) => tryTool("line", e),
      toolText: (e) => tryTool("text", e),
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
    }),
    [
      handleUndo,
      handleRedo,
      handleSelectAll,
      handleDeleteSelected,
      handleClearSelection,
      handleCopy,
      handlePaste,
      tryTool,
    ],
  );
  useShortcuts(shortcutHandlers);

  // Map screen→world. The renderer reads pan/zoom directly so the same
  // formula is used both for the wheel-zoom anchor (inside CanvasHost)
  // and for click-to-hit-test below.
  const screenToWorld = useCallback(
    (sx: number, sy: number): { x: number; y: number } => {
      return {
        x: (sx - viewport.panX) / viewport.zoom,
        y: (sy - viewport.panY) / viewport.zoom,
      };
    },
    [viewport],
  );

  const onCanvasPointer = useCallback(
    (e: React.PointerEvent<HTMLCanvasElement>) => {
      if (e.button !== 0 && e.type === "pointerdown") return;
      // React nullifies `SyntheticEvent.currentTarget` once the
      // synchronous handler returns, so the async IIFE below cannot
      // read it after an `await`. Capture the canvas element + pointer
      // id synchronously so `setPointerCapture` /
      // `releasePointerCapture` keep working across awaits.
      const canvasEl = e.currentTarget;
      const pointerId = e.pointerId;
      const rect = canvasEl.getBoundingClientRect();
      const sx = e.clientX - rect.left;
      const sy = e.clientY - rect.top;
      const { x: wx, y: wy } = screenToWorld(sx, sy);
      // Phase 6 Tasks 25-26: latest world-space cursor sample drives
      // paste-at-cursor.
      lastCursorWorldRef.current = { x: wx, y: wy };
      // Capture the viewport snapshot at pointer-down time. The Rust
      // hit-test wants screen coordinates plus the viewport so it can
      // run the screen→world transform once — if we pre-transformed
      // here too, the renderer would double-apply pan + zoom and miss
      // every click.
      const vp = viewport;

      if (e.type === "pointerdown") {
        if (tool === "select") {
          // Click-to-select: hit-test, then either start a move drag or
          // clear selection. The bridge does the screen→world transform
          // internally; we send raw screen coordinates plus the current
          // viewport (single source of truth, no double-transform).
          void (async () => {
            try {
              const hit = await window.kcreate.canvas.hitTest(
                sx,
                sy,
                vp.panX,
                vp.panY,
                vp.zoom,
              );
              if (hit) {
                await window.kcreate.canvas.setSelection([hit]);
                setSelectedIds([hit]);
                canvasEl.setPointerCapture(pointerId);
                dragStateRef.current = {
                  kind: "move",
                  tool,
                  pointerId,
                  startWorldX: wx,
                  startWorldY: wy,
                  lastWorldX: wx,
                  lastWorldY: wy,
                  movingNodeId: hit,
                  cumulativeDx: 0,
                  cumulativeDy: 0,
                };
              } else {
                await window.kcreate.canvas.clearSelection();
                setSelectedIds([]);
              }
            } catch (err) {
              setStatusMessage(`hit-test failed: ${errorMessage(err)}`);
            }
          })();
          return;
        }
        // Drawing tools — record drag start in world coords; commit on
        // pointerup.
        canvasEl.setPointerCapture(pointerId);
        dragStateRef.current = {
          kind: "create",
          tool,
          pointerId,
          startWorldX: wx,
          startWorldY: wy,
          lastWorldX: wx,
          lastWorldY: wy,
          movingNodeId: null,
          cumulativeDx: 0,
          cumulativeDy: 0,
        };
        return;
      }

      if (e.type === "pointermove") {
        const drag = dragStateRef.current;
        if (!drag || drag.pointerId !== e.pointerId) return;
        if (drag.kind === "move" && drag.movingNodeId) {
          const dx = wx - drag.lastWorldX;
          const dy = wy - drag.lastWorldY;
          drag.lastWorldX = wx;
          drag.lastWorldY = wy;
          drag.cumulativeDx += dx;
          drag.cumulativeDy += dy;
          // Smart-guides: query the snap engine for the candidate
          // world-space bounds and apply the returned delta to the
          // *cumulative* offset (so the next pointermove keeps
          // working off the snapped position). The bridge call is
          // cheap (O(log n) per axis after the sorted-edge build);
          // we still fire it on every move because the engine is
          // built from-scratch each time — the dragged node's bounds
          // are dirty otherwise.
          const movingNode = nodesRef.current.find(
            (n) => n.id === drag.movingNodeId,
          );
          if (movingNode) {
            const candX =
              movingNode.bounds.x + drag.cumulativeDx;
            const candY =
              movingNode.bounds.y + drag.cumulativeDy;
            void (async () => {
              try {
                const snap = await window.kcreate.canvasSnap.query(
                  movingNode.id,
                  candX,
                  candY,
                  movingNode.bounds.width,
                  movingNode.bounds.height,
                  SNAP_THRESHOLD_WORLD,
                );
                if (!snap) return;
                if (snap.dx !== 0 || snap.dy !== 0) {
                  drag.cumulativeDx += snap.dx;
                  drag.cumulativeDy += snap.dy;
                }
                setSnapGuides(snap.guides);
              } catch {
                // Snap is purely advisory — failures shouldn't
                // abort the drag. Silently swallow.
              }
            })();
          }
          // Don't fire a bridge call for every micro-pixel of cursor
          // motion — only push the accumulated delta on pointerup. This
          // keeps undo entries coarse (one drag = one op) and avoids
          // op-log spam.
          return;
        }
        // Drawing — no commit until pointerup; the canvas does not yet
        // show an in-flight ghost. Phase 1 will add a transient overlay
        // by passing the in-progress rect/ellipse to the renderer
        // alongside the persisted scene.
        return;
      }

      if (e.type === "pointerup") {
        const drag = dragStateRef.current;
        if (!drag || drag.pointerId !== pointerId) return;
        try {
          canvasEl.releasePointerCapture(pointerId);
        } catch {
          // capture might already be released
        }
        dragStateRef.current = null;
        // Clear smart-guides — the drag is done, so any displayed
        // guide lines belong to a stale candidate position.
        setSnapGuides([]);
        if (drag.kind === "move" && drag.movingNodeId) {
          if (drag.cumulativeDx !== 0 || drag.cumulativeDy !== 0) {
            void (async () => {
              try {
                await window.kcreate.canvas.moveNode(
                  drag.movingNodeId!,
                  drag.cumulativeDx,
                  drag.cumulativeDy,
                );
                await refreshTree();
              } catch (err) {
                setStatusMessage(`move failed: ${errorMessage(err)}`);
              }
            })();
          }
          return;
        }
        // Creation: convert the drag to the actual shape parameters.
        const x0 = drag.startWorldX;
        const y0 = drag.startWorldY;
        const x1 = wx;
        const y1 = wy;
        const minX = Math.min(x0, x1);
        const minY = Math.min(y0, y1);
        const w = Math.abs(x1 - x0);
        const h = Math.abs(y1 - y0);
        // Reject zero-area drags — that's a stray click, not a drawing.
        if (w < 1 && h < 1 && drag.tool !== "text") return;

        void (async () => {
          try {
            let newId: string | null = null;
            if (drag.tool === "rect") {
              newId = await window.kcreate.canvas.createRect(
                null,
                minX,
                minY,
                w,
                h,
              );
            } else if (drag.tool === "ellipse") {
              newId = await window.kcreate.canvas.createEllipse(
                null,
                minX + w / 2,
                minY + h / 2,
                w / 2,
                h / 2,
              );
            } else if (drag.tool === "line") {
              newId = await window.kcreate.canvas.createLine(
                null,
                x0,
                y0,
                x1,
                y1,
              );
            } else if (drag.tool === "text") {
              newId = await window.kcreate.canvas.createText(
                null,
                x0,
                y0,
                "Text",
                "sans-serif",
                24,
              );
            }
            if (newId) {
              await window.kcreate.canvas.setSelection([newId]);
            }
            await refreshTree();
          } catch (err) {
            setStatusMessage(`create failed: ${errorMessage(err)}`);
          }
        })();
      }
    },
    [tool, viewport, screenToWorld, refreshTree],
  );

  const onZoomToFit = useCallback(() => {
    // No documentBounds API yet; reset to identity. Phase 1 will compute
    // a bounding box across visible nodes.
    setViewport(DEFAULT_VIEWPORT);
  }, []);

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
    [refreshTree],
  );

  // Right-panel content depends on the active mode (see PROPOSAL § 6).
  // Image mode focuses on AI Assist; Export mode focuses on Export
  // presets; everything else lands on the properties inspector.
  const rightPanelFocus = defaultPanelForMode(mode);
  const cursor = TOOL_CURSORS[tool];

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
          onDragOver={handleCanvasDragOver}
          onDrop={handleCanvasDrop}
          style={{
            position: "relative",
            background: colors.bgCanvas,
            minWidth: 0,
            overflow: "hidden",
          }}
        >
          <CanvasHost
            width={CANVAS_WIDTH}
            height={CANVAS_HEIGHT}
            scene={scene}
            viewport={viewport}
            onViewportChange={setViewport}
            onFramePresented={onFrame}
            onPointer={onCanvasPointer}
            onZoomToFit={onZoomToFit}
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
            Phase 2 soft-proof / gamut-warning overlay. Reads the
            project's color settings via `window.kcreate.color` and
            renders a CSS-filter wash on top of the canvas. Renders
            nothing when both features are disabled, so the offscreen
            wgpu path remains the source of truth for exports.
          */}
          <SoftProofOverlay />
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

function errorMessage(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
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
