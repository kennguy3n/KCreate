// Electron main process entry. Owns the BrowserWindow and the
// renderer-side IPC handlers that proxy to the Rust kcreate_bridge
// native addon.

import { app, BrowserWindow, ipcMain } from "electron";
import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";

import { loadBridge, type Bridge } from "./bridge";

/// Naming convention for scratch projects opened from the Home tile.
/// Centralised here so the cleanup pass (`cleanupScratchProjects`) and
/// the renderer's create-scratch path (`App.tsx`) agree on what is
/// considered "scratch" and therefore safe to delete.
const SCRATCH_PREFIX = "scratch-";
const SCRATCH_SUFFIX = ".kstudio";

/// Best-effort cleanup of stale scratch projects in the OS temp dir.
///
/// A "scratch" project is a `.kstudio` directory created from the Home
/// tile click path; the user never named or saved it, so we own its
/// lifecycle. Phase 0 leaks one of these per Home→Editor transition
/// (issue ANALYSIS_0005), which is harmless on macOS/Linux (their temp
/// reapers eventually sweep them) but accumulates indefinitely on
/// Windows. Sweeping here, in the host process where `fs.rm` and the
/// authoritative `os.tmpdir()` live, keeps the cleanup safely scoped:
/// the renderer never gets a path-walk capability.
///
/// Safety constraints, deliberately strict:
///   * Only direct children of `os.tmpdir()` are inspected — we never
///     recurse.
///   * Each candidate name must start with `SCRATCH_PREFIX` AND end
///     with `SCRATCH_SUFFIX`.
///   * Each candidate must resolve (via `path.join`) to a path whose
///     parent is `os.tmpdir()` after `path.resolve` — defence against
///     a future bug introducing `..` segments.
///   * Each candidate must be a directory.
///   * Per-entry errors are swallowed and counted, never thrown, so
///     one locked file on Windows doesn't poison the whole sweep.
async function cleanupScratchProjects(): Promise<{
  scanned: number;
  removed: number;
  errors: number;
}> {
  const base = os.tmpdir();
  let scanned = 0;
  let removed = 0;
  let errors = 0;
  let entries: import("node:fs").Dirent[];
  try {
    entries = await fs.readdir(base, { withFileTypes: true });
  } catch {
    return { scanned, removed, errors: 1 };
  }
  for (const entry of entries) {
    if (!entry.isDirectory()) continue;
    const name = entry.name;
    if (!name.startsWith(SCRATCH_PREFIX) || !name.endsWith(SCRATCH_SUFFIX)) {
      continue;
    }
    scanned += 1;
    const candidate = path.resolve(base, name);
    // Refuse to delete anything whose resolved parent isn't the temp
    // dir itself. This is a paranoia check; readdir won't return
    // entries outside `base`, but symlink-into-temp + traversal would
    // be a credible future regression.
    if (path.dirname(candidate) !== path.resolve(base)) {
      errors += 1;
      continue;
    }
    try {
      await fs.rm(candidate, { recursive: true, force: true });
      removed += 1;
    } catch {
      errors += 1;
    }
  }
  return { scanned, removed, errors };
}

// The native bridge is loaded eagerly in `app.whenReady`, BEFORE any IPC
// handlers are registered. This is the architecturally correct moment:
// `process.dlopen` is a synchronous, one-shot operation, and loading it
// inside the IPC handlers (the old `getBridge()` lazy pattern) opened a
// race where two concurrent IPC events could both observe `bridge ===
// null` and call `loadBridge()` twice. Eager loading at startup
// eliminates the race entirely and also surfaces native-load failures
// at app startup rather than on the first user interaction.
let bridge: Bridge | null = null;

function requireBridge(): Bridge {
  if (!bridge) {
    throw new Error(
      "kcreate native bridge accessed before app initialization completed",
    );
  }
  return bridge;
}

function createWindow(): BrowserWindow {
  const win = new BrowserWindow({
    width: 1280,
    height: 800,
    backgroundColor: "#1e1e1e",
    show: false,
    webPreferences: {
      preload: path.join(__dirname, "..", "..", "preload", "dist", "preload.js"),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: false,
    },
  });

  const devUrl = process.env["KCREATE_DEV_RENDERER_URL"];
  if (devUrl) {
    void win.loadURL(devUrl);
  } else {
    void win.loadFile(
      path.join(__dirname, "..", "..", "renderer", "dist", "index.html"),
    );
  }

  win.once("ready-to-show", () => win.show());
  return win;
}

function registerIpcHandlers(): void {
  ipcMain.handle("kcreate/renderer/init", (_e, width: number, height: number) =>
    requireBridge().rendererInit(width, height),
  );
  ipcMain.handle("kcreate/renderer/shutdown", () => {
    requireBridge().rendererShutdown();
  });
  ipcMain.handle(
    "kcreate/renderer/resize",
    (_e, width: number, height: number) =>
      requireBridge().rendererResize(width, height),
  );
  ipcMain.handle(
    "kcreate/renderer/setViewport",
    (_e, panX: number, panY: number, zoom: number) =>
      requireBridge().rendererSetViewport(panX, panY, zoom),
  );
  ipcMain.handle(
    "kcreate/renderer/invalidate",
    (
      _e,
      region: { x: number; y: number; width: number; height: number } | null,
    ) =>
      requireBridge().rendererInvalidate(
        region?.x ?? null,
        region?.y ?? null,
        region?.width ?? null,
        region?.height ?? null,
      ),
  );
  ipcMain.handle("kcreate/renderer/render", (_e, sceneJson: string) =>
    requireBridge().rendererRender(sceneJson),
  );
  ipcMain.handle("kcreate/renderer/getFrame", () =>
    requireBridge().rendererGetFrame(),
  );
  ipcMain.handle("kcreate/renderer/frameInfo", () =>
    requireBridge().rendererFrameInfo(),
  );
  ipcMain.handle("kcreate/renderer/acquireFrame", () =>
    requireBridge().rendererAcquireFrame(),
  );

  // Document / project lifecycle.
  ipcMain.handle(
    "kcreate/project/create",
    (_e, name: string, dir: string) =>
      requireBridge().projectCreate(name, dir),
  );
  ipcMain.handle("kcreate/project/open", (_e, dir: string) =>
    requireBridge().projectOpen(dir),
  );
  ipcMain.handle("kcreate/project/save", () => {
    requireBridge().projectSave();
  });
  ipcMain.handle("kcreate/project/close", () => {
    requireBridge().projectClose();
  });
  ipcMain.handle("kcreate/project/getInfo", () =>
    requireBridge().projectGetInfo(),
  );

  ipcMain.handle("kcreate/document/getTree", () =>
    requireBridge().documentGetTree(),
  );
  ipcMain.handle(
    "kcreate/document/createNode",
    (
      _e,
      nodeType: string,
      parentId: string | null,
      propsJson: string,
    ) =>
      requireBridge().documentCreateNode(nodeType, parentId, propsJson),
  );
  ipcMain.handle(
    "kcreate/document/updateNode",
    (_e, nodeId: string, changesJson: string) => {
      requireBridge().documentUpdateNode(nodeId, changesJson);
    },
  );
  ipcMain.handle(
    "kcreate/document/deleteNode",
    (_e, nodeId: string) => {
      requireBridge().documentDeleteNode(nodeId);
    },
  );
  ipcMain.handle("kcreate/document/undo", () =>
    requireBridge().documentUndo(),
  );
  ipcMain.handle("kcreate/document/redo", () =>
    requireBridge().documentRedo(),
  );
  ipcMain.handle("kcreate/document/status", () =>
    requireBridge().documentStatus(),
  );

  ipcMain.handle("kcreate/runtime/status", () =>
    requireBridge().runtimeStatus(),
  );
  // The OS temp dir is owned by the host (Node `os.tmpdir()`), not by
  // the Rust bridge — it's a process-environment concern, not a
  // rendering one. Surfacing it through the runtime bridge lets the
  // renderer stay agnostic of POSIX vs Windows path conventions.
  ipcMain.handle("kcreate/runtime/tempDir", () => os.tmpdir());
  // Sweep stale scratch projects (`scratch-*.kstudio` under
  // `os.tmpdir()`). See `cleanupScratchProjects` for safety
  // constraints — this is intentionally a zero-argument IPC so the
  // renderer never picks the path/prefix.
  ipcMain.handle("kcreate/runtime/cleanupScratchProjects", () =>
    cleanupScratchProjects(),
  );

  ipcMain.handle(
    "kcreate/export/svg",
    (_e, nodeIds: string[], optionsJson: string) =>
      requireBridge().exportSvg(nodeIds, optionsJson),
  );
  ipcMain.handle(
    "kcreate/export/png",
    (_e, outputPath: string, optionsJson: string) =>
      requireBridge().exportPng(outputPath, optionsJson),
  );
}

void app.whenReady().then(() => {
  // Load the native bridge synchronously, before any window/IPC traffic
  // can hit `requireBridge()`. See the comment above `let bridge`.
  bridge = loadBridge();
  registerIpcHandlers();
  createWindow();

  app.on("activate", () => {
    if (BrowserWindow.getAllWindows().length === 0) {
      createWindow();
    }
  });
});

app.on("window-all-closed", () => {
  if (process.platform !== "darwin") {
    app.quit();
  }
});

app.on("will-quit", () => {
  if (bridge) {
    bridge.rendererShutdown();
    bridge = null;
  }
});
