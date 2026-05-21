import { useCallback, useEffect, useMemo, useState } from "react";

import { CanvasHost } from "../components/CanvasHost";
import { LeftPanel } from "../components/LeftPanel";
import { RightPanel } from "../components/RightPanel";
import { TopBar, type EditorMode } from "../components/TopBar";
import type {
  DocumentStatus,
  NodeInfo,
  ProjectInfo,
  Scene,
} from "../../../shared/scene";
import { colors, font, spacing } from "../styles/tokens";

export interface EditorPageProps {
  project: ProjectInfo;
  onBackHome: () => void;
}

const SAMPLE_SCENE: Scene = {
  clear_color: [0.12, 0.12, 0.14, 1.0],
  objects: [
    {
      id: 1,
      z: 0,
      translation: [80, 80],
      style: {
        fill: [0.92, 0.36, 0.36, 1.0],
        stroke: { color: [0, 0, 0, 1.0], width: 2.0 },
      },
      kind: { type: "rect", x: 0, y: 0, width: 360, height: 220 },
    },
    {
      id: 2,
      z: 1,
      translation: [560, 220],
      style: { fill: [0.35, 0.78, 0.95, 1.0], stroke: null },
      kind: { type: "circle", cx: 0, cy: 0, radius: 140 },
    },
  ],
};

export function EditorPage({
  project,
  onBackHome,
}: EditorPageProps): JSX.Element {
  const [mode, setMode] = useState<EditorMode>("design");
  const [nodes, setNodes] = useState<NodeInfo[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [statusMessage, setStatusMessage] = useState<string | null>(null);
  const [fps, setFps] = useState<number>(0);
  const [lastTickAt, setLastTickAt] = useState<number>(performance.now());
  // Editing-state from the bridge. `null` while the first probe is
  // in flight, then either a snapshot of the operation log or `null`
  // again if the workspace is closed. Default to disabled controls
  // until we have confirmation from the bridge — a brief disabled
  // flash is preferable to an Undo button that lies about its state.
  const [docStatus, setDocStatus] = useState<DocumentStatus | null>(null);

  const refreshStatus = useCallback(async () => {
    try {
      const s = await window.kcreate.document.status();
      setDocStatus(s);
    } catch (e) {
      setStatusMessage(`status probe failed: ${errorMessage(e)}`);
    }
  }, []);

  const refreshTree = useCallback(async () => {
    try {
      const tree = await window.kcreate.document.getDocumentTree();
      setNodes(tree);
    } catch (e) {
      setStatusMessage(`tree load failed: ${errorMessage(e)}`);
    }
    // The tree changed — so might the operation log (in particular for
    // undo/redo flows that mutate the cursor without changing nodes,
    // we still want this called from the dedicated handlers below).
    await refreshStatus();
  }, [refreshStatus]);

  useEffect(() => {
    void refreshTree();
  }, [refreshTree]);

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

  const handleExport = useCallback(async () => {
    try {
      const svg = await window.kcreate.export.svg([], {
        width: 1024,
        height: 768,
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
    const elapsed = now - lastTickAt;
    setLastTickAt(now);
    if (elapsed > 0) setFps(Math.round(1000 / elapsed));
  }, [lastTickAt]);

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
          gridTemplateColumns: "auto 1fr auto",
          minHeight: 0,
        }}
      >
        <LeftPanel
          nodes={nodes}
          selectedId={selectedId}
          onSelect={setSelectedId}
        />
        <main
          style={{
            position: "relative",
            background: colors.bgCanvas,
            minWidth: 0,
            overflow: "hidden",
          }}
        >
          <CanvasHost
            width={1024}
            height={640}
            scene={SAMPLE_SCENE}
            onFramePresented={onFrame}
          />
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
            {fps} fps · {mode}
          </div>
        </main>
        <RightPanel
          selected={selected}
          onRequestExport={() => {
            void handleExport();
          }}
        />
      </div>
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
      </footer>
    </div>
  );
}

function errorMessage(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}
