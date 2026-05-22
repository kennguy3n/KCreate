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

/// Lockfile written inside each scratch project directory by the
/// instance that owns it. Presence-plus-live-PID is the signal
/// `cleanupScratchProjects` uses to refuse deletion of a directory
/// that another running KCreate is actively editing.
///
/// The dot prefix keeps it out of glob results that show user-visible
/// project content; the extension `.kclock` (KCreate lock) is unique
/// enough to avoid collisions with anything the editor or its tools
/// might legitimately store inside `.kstudio`.
const SCRATCH_LOCKFILE_NAME = ".kclock";

/// Grace period during which a freshly-created scratch directory is
/// considered "in use" even if its lockfile hasn't landed yet.
///
/// Closes the narrow window between
///   1. The bridge `project_create` finishing (`.kstudio` exists), and
///   2. The IPC handler writing the lockfile (`.kclock` lands).
/// A concurrent instance running `cleanupScratchProjects` in that
/// window would otherwise delete the just-created scratch project of
/// another running KCreate. 10 s is far longer than the create
/// pathway needs and far shorter than "genuinely stale".
const SCRATCH_LOCKFILE_GRACE_MS = 10_000;

/// Shape of `.kclock`. The PID lets a sweeping instance check
/// liveness via `process.kill(pid, 0)`. `startedAt` is informational
/// (helps humans tailing the temp dir) and lets us future-proof
/// against PID reuse if we ever start matching against
/// `app.startTime()` too.
interface ScratchLockfile {
  pid: number;
  startedAt: string;
}

/// Track the path of the open scratch project (if any) so the
/// `kcreate/project/close` and `will-quit` paths can remove its
/// lockfile before the sweep runs. A non-null value means "this
/// instance currently owns a scratch dir at this path".
let ownedScratchPath: string | null = null;

/// Returns true when `dir` looks like a scratch directory that this
/// host process would own — i.e. lives directly under `os.tmpdir()`
/// and matches the `SCRATCH_PREFIX`/`SCRATCH_SUFFIX` naming. Used to
/// scope lockfile writes to the projects whose lifecycle we actually
/// manage; user-named projects in `~/Documents/` never get a `.kclock`
/// because they're never swept.
function isOwnedScratchPath(dir: string): boolean {
  const resolved = path.resolve(dir);
  if (path.dirname(resolved) !== path.resolve(os.tmpdir())) return false;
  const name = path.basename(resolved);
  return name.startsWith(SCRATCH_PREFIX) && name.endsWith(SCRATCH_SUFFIX);
}

/// Write the lockfile that marks `projectPath` as owned by this
/// process. Best-effort — if the write fails, the dir falls back to
/// the mtime-grace heuristic in `isScratchDirOwnedByLiveInstance`,
/// so the failure mode is "deletable a bit earlier than ideal" not
/// "data loss".
async function writeScratchLockfile(projectPath: string): Promise<void> {
  const lockPath = path.join(projectPath, SCRATCH_LOCKFILE_NAME);
  const payload: ScratchLockfile = {
    pid: process.pid,
    startedAt: new Date().toISOString(),
  };
  try {
    await fs.writeFile(lockPath, JSON.stringify(payload), { encoding: "utf8" });
  } catch {
    // best-effort; grace window covers the lockfile-missing case.
  }
}

/// Remove our lockfile so the directory becomes eligible for
/// cleanup. Idempotent and silent on ENOENT.
async function removeScratchLockfile(projectPath: string): Promise<void> {
  const lockPath = path.join(projectPath, SCRATCH_LOCKFILE_NAME);
  try {
    await fs.rm(lockPath, { force: true });
  } catch {
    // best-effort
  }
}

/// Cross-platform "is this PID alive right now?" check.
///
/// `process.kill(pid, 0)` is the POSIX idiom: signal 0 performs the
/// permission-and-existence check without actually delivering a
/// signal. Node's libuv wraps this on Windows too (it routes to
/// `OpenProcess`), so the same code works across all three target
/// platforms.
///
/// Two error codes we deliberately interpret as "alive":
///   * `ESRCH` — no such process → dead.
///   * `EPERM` — process exists but we lack permission to signal it
///              (this happens on Windows when the other instance
///              runs as a different user). Definitely alive; we
///              must NOT clean its dir.
function isProcessAlive(pid: number): boolean {
  if (!Number.isFinite(pid) || pid <= 0 || pid === process.pid) return false;
  try {
    process.kill(pid, 0);
    return true;
  } catch (err) {
    const code = (err as NodeJS.ErrnoException).code;
    return code === "EPERM";
  }
}

/// Decide whether `dirPath` is currently in use by a live KCreate
/// instance other than this one. Used by `cleanupScratchProjects` to
/// skip deletion of directories another process is editing.
///
/// Order of evidence:
///   1. Parse `<dir>/.kclock`. If it names a live foreign PID, the
///      directory is owned → skip.
///   2. If no lockfile but the directory mtime is within the grace
///      window (`SCRATCH_LOCKFILE_GRACE_MS`), assume it's mid-create
///      by an instance whose lockfile hasn't landed yet → skip.
///   3. Otherwise → stale, eligible for deletion.
///
/// A malformed or unreadable lockfile is treated as "absent" —
/// pessimistically *not* a reason to skip cleanup, because the
/// alternative is leaking forever on corruption. The grace window
/// still catches the legitimate "just created" case.
async function isScratchDirOwnedByLiveInstance(dirPath: string): Promise<boolean> {
  const lockPath = path.join(dirPath, SCRATCH_LOCKFILE_NAME);
  try {
    const raw = await fs.readFile(lockPath, { encoding: "utf8" });
    const parsed: unknown = JSON.parse(raw);
    if (parsed && typeof parsed === "object" && "pid" in parsed) {
      const pid = (parsed as { pid: unknown }).pid;
      if (typeof pid === "number" && pid !== process.pid && isProcessAlive(pid)) {
        return true;
      }
    }
  } catch {
    // No lockfile or unreadable — fall through to the grace check.
  }
  try {
    const st = await fs.stat(dirPath);
    const ageMs = Date.now() - st.mtimeMs;
    if (ageMs >= 0 && ageMs < SCRATCH_LOCKFILE_GRACE_MS) {
      return true;
    }
  } catch {
    // Stat failed — fall through. Returning false (not owned) is
    // the safer choice: the subsequent `fs.rm` will fail too and be
    // counted as an error, surfacing the underlying problem.
  }
  return false;
}

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
  skippedOwned: number;
}> {
  const base = os.tmpdir();
  let scanned = 0;
  let removed = 0;
  let errors = 0;
  let skippedOwned = 0;
  let entries: import("node:fs").Dirent[];
  try {
    entries = await fs.readdir(base, { withFileTypes: true });
  } catch {
    return { scanned, removed, errors: 1, skippedOwned };
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
    // ANALYSIS_c42c3f1_0001: cross-instance safety. On macOS/Linux,
    // advisory file locking doesn't prevent `fs.rm` from succeeding
    // even while another KCreate has the SQLite file open, so the
    // unlucky-second instance would corrupt the first's open
    // workspace. The lockfile + liveness check below makes the host
    // process the sole authority on "is this scratch dir in use".
    if (await isScratchDirOwnedByLiveInstance(candidate)) {
      skippedOwned += 1;
      continue;
    }
    try {
      await fs.rm(candidate, { recursive: true, force: true });
      removed += 1;
    } catch {
      errors += 1;
    }
  }
  return { scanned, removed, errors, skippedOwned };
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

// The currently active main BrowserWindow. Tracked at module scope
// so the `kcreate/canvas/native-handle` IPC can extract the
// platform-specific window handle for the native presentation path
// (Phase 1, Block A, Task 4). Set in `createWindow`, cleared on the
// window's `closed` event so a stale reference can never outlive the
// native handle it would hand out.
let mainWindow: BrowserWindow | null = null;

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
  mainWindow = win;
  // Lifecycle for the native-canvas presentation path
  // (`crates/kcreate_bridge/src/native_canvas.rs`). The Rust side
  // holds an `Arc<PlatformHandle>` that wraps the BrowserWindow's
  // OS-level handle (NSView*/HWND/XID/wl_surface*). If we let the
  // OS destroy the window while that Arc is still alive, the
  // wrapped pointer becomes dangling — `unsafe impl Send + Sync
  // for PlatformHandle` is only sound while the underlying OS
  // resource outlives the surface.
  //
  // Electron's `'close'` event fires *before* the OS resource is
  // destroyed (unlike `'closed'`, which fires after). We hook it
  // to ask the bridge to switch back to offscreen presentation,
  // which detaches the native surface and drops the
  // `Arc<PlatformHandle>` in the renderer state. `rendererSwitchOffscreen`
  // is synchronous, returns immediately if no native surface is
  // attached, and is safe to call even when the bridge was built
  // without the `native_canvas` Cargo feature (it then no-ops).
  win.on("close", () => {
    if (!bridge) return;
    try {
      bridge.rendererSwitchOffscreen();
    } catch {
      // best-effort: a shutdown failure here cannot block window
      // close, and the OS will still tear down the handle. Logging
      // is intentionally elided to avoid noise during normal exit.
    }
  });
  win.on("closed", () => {
    if (mainWindow === win) mainWindow = null;
  });
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

  // Native canvas presentation mode (Phase 1, Block A, Tasks 4–6).
  //
  // The renderer can ask the main process to extract the platform
  // window handle (`NSView*` / `HWND` / `XID` / `wl_surface*`) from
  // the BrowserWindow and pass it back as a Buffer; the renderer then
  // calls `switchNative` with those bytes to swap the presentation
  // path away from CPU readback. The exact byte interpretation lives
  // on the Rust side under `crates/kcreate_bridge/src/native_canvas.rs`,
  // gated behind the `native_canvas` Cargo feature so the default
  // build remains `unsafe`-free.
  ipcMain.handle("kcreate/canvas/native-handle", () => {
    const win = mainWindow;
    if (!win || win.isDestroyed()) return null;
    // Electron exposes `getNativeWindowHandle()` on every platform; it
    // returns a Node `Buffer` containing the platform-specific handle
    // (size + endianness vary). We forward the bytes opaquely — the
    // bridge interprets them based on the host OS.
    const handle = win.getNativeWindowHandle();
    return handle;
  });
  ipcMain.handle(
    "kcreate/renderer/switchNative",
    (_e, handleBytes: Buffer, width: number, height: number) =>
      requireBridge().rendererSwitchNative(handleBytes, width, height),
  );
  ipcMain.handle("kcreate/renderer/switchOffscreen", () => {
    requireBridge().rendererSwitchOffscreen();
  });
  ipcMain.handle("kcreate/renderer/presentationMode", () =>
    requireBridge().rendererPresentationMode(),
  );

  // Document / project lifecycle.
  //
  // The `project/create` and `project/open` handlers also manage the
  // scratch-dir lockfile (ANALYSIS_c42c3f1_0001): after the bridge
  // succeeds, we write `.kclock` into the project dir so a sibling
  // KCreate's `cleanupScratchProjects` sweep refuses to delete it
  // while we still hold the SQLite file open. `project/close`
  // removes our own lockfile so the next sweep — either in this
  // process or any other — will reap the directory cleanly.
  ipcMain.handle(
    "kcreate/project/create",
    async (_e, name: string, dir: string) => {
      const info = requireBridge().projectCreate(name, dir);
      if (isOwnedScratchPath(info.path)) {
        ownedScratchPath = info.path;
        await writeScratchLockfile(info.path);
      }
      return info;
    },
  );
  ipcMain.handle("kcreate/project/open", async (_e, dir: string) => {
    const info = requireBridge().projectOpen(dir);
    if (isOwnedScratchPath(info.path)) {
      ownedScratchPath = info.path;
      await writeScratchLockfile(info.path);
    }
    return info;
  });
  ipcMain.handle("kcreate/project/save", () => {
    requireBridge().projectSave();
  });
  ipcMain.handle("kcreate/project/close", async () => {
    // Release the lockfile *before* the bridge call so a concurrent
    // sweep observing the empty `.kclock`-less dir would only see a
    // workspace that's about to close; with the bridge call first,
    // there is a TOCTOU window where the dir has been closed but the
    // lockfile still names this PID as owner (and we'd briefly block
    // our own sweep).
    const path = ownedScratchPath;
    if (path !== null) {
      await removeScratchLockfile(path);
      ownedScratchPath = null;
    }
    requireBridge().projectClose();
  });
  ipcMain.handle("kcreate/project/getInfo", () =>
    requireBridge().projectGetInfo(),
  );
  ipcMain.handle("kcreate/project/isUntouched", () =>
    requireBridge().projectIsUntouched(),
  );

  ipcMain.handle("kcreate/document/getTree", () =>
    requireBridge().documentGetTree(),
  );
  ipcMain.handle(
    "kcreate/document/inspectNode",
    (_e, nodeId: string): string =>
      requireBridge().documentInspectNode(nodeId),
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
  ipcMain.handle("kcreate/runtime/lowResourceMode/get", (): boolean =>
    requireBridge().lowResourceModeGet(),
  );
  ipcMain.handle(
    "kcreate/runtime/lowResourceMode/set",
    (_e, enabled: boolean): void => {
      requireBridge().lowResourceModeSet(enabled);
    },
  );
  ipcMain.handle("kcreate/runtime/resourceLimits", (): string =>
    requireBridge().resourceLimits(),
  );

  ipcMain.handle(
    "kcreate/llm/start",
    (_e, modelPath: string): number =>
      requireBridge().llmStart(modelPath),
  );
  ipcMain.handle("kcreate/llm/stop", (): void => {
    requireBridge().llmStop();
  });
  ipcMain.handle("kcreate/llm/status", (): string =>
    requireBridge().llmStatus(),
  );
  // Each LLM completion handler simply forwards the bridge's
  // Promise. The bridge runs the blocking HTTP work on N-API's
  // libuv thread pool (see `LlmChatTask` etc. in
  // `crates/kcreate_bridge/src/lib.rs`), so the main loop stays
  // responsive across the up-to-60-second llama-server timeout.
  ipcMain.handle(
    "kcreate/llm/chat",
    (
      _e,
      messagesJson: string,
      maxTokens: number,
      temperature: number,
    ): Promise<string> =>
      requireBridge().llmChat(messagesJson, maxTokens, temperature),
  );
  ipcMain.handle("kcreate/llm/suggest", (): Promise<string> =>
    requireBridge().llmSuggestForSelection(),
  );
  ipcMain.handle("kcreate/ai/suggestLayerNames", (): Promise<string> =>
    requireBridge().aiSuggestLayerNames(),
  );
  ipcMain.handle("kcreate/ai/extractDesignTokens", (): Promise<string> =>
    requireBridge().aiExtractDesignTokens(),
  );
  ipcMain.handle("kcreate/ai/checkAccessibility", (): Promise<string> =>
    requireBridge().aiCheckAccessibility(),
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
  // Sandboxed text-file sink used by the dev-handoff export preset
  // and other renderer-side sidecars. The path is canonicalised
  // against `os.tmpdir()` so the renderer can only write inside the
  // OS temp directory (the same root `tempDir()` hands out). Any
  // attempt to escape that root via `..` or an absolute prefix is
  // rejected.
  ipcMain.handle(
    "kcreate/runtime/writeTextFile",
    async (_e, target: string, content: string): Promise<number> => {
      const tmp = path.resolve(os.tmpdir());
      const resolved = path.resolve(target);
      const rel = path.relative(tmp, resolved);
      if (rel.startsWith("..") || path.isAbsolute(rel)) {
        throw new Error(
          `writeTextFile rejected: ${target} is outside ${tmp}`,
        );
      }
      await fs.mkdir(path.dirname(resolved), { recursive: true });
      const bytes = Buffer.byteLength(content, "utf8");
      await fs.writeFile(resolved, content, "utf8");
      return bytes;
    },
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
  ipcMain.handle(
    "kcreate/export/pdf",
    (_e, outputPath: string, optionsJson: string) =>
      requireBridge().exportPdf(outputPath, optionsJson),
  );
  ipcMain.handle(
    "kcreate/export/webp",
    (_e, outputPath: string, optionsJson: string) =>
      requireBridge().exportWebp(outputPath, optionsJson),
  );
  ipcMain.handle(
    "kcreate/export/jpeg",
    (_e, outputPath: string, optionsJson: string) =>
      requireBridge().exportJpeg(outputPath, optionsJson),
  );

  // Canvas: scene sync, hit testing, selection, shape creation, move,
  // raster import. All N-API entry points already validate inputs and
  // raise typed errors on misuse; this layer just marshals.
  ipcMain.handle("kcreate/document/syncScene", () => {
    requireBridge().documentSyncScene();
  });
  ipcMain.handle(
    "kcreate/canvas/hitTest",
    (
      _e,
      x: number,
      y: number,
      panX: number,
      panY: number,
      zoom: number,
    ) => requireBridge().canvasHitTest(x, y, panX, panY, zoom),
  );
  ipcMain.handle(
    "kcreate/document/setSelection",
    (_e, nodeIds: string[]) => {
      requireBridge().documentSetSelection(nodeIds);
    },
  );
  ipcMain.handle("kcreate/document/getSelection", () =>
    requireBridge().documentGetSelection(),
  );
  ipcMain.handle("kcreate/document/clearSelection", () => {
    requireBridge().documentClearSelection();
  });
  ipcMain.handle(
    "kcreate/document/importImage",
    (_e, parentId: string | null, filePath: string) =>
      requireBridge().documentImportImage(parentId, filePath),
  );
  ipcMain.handle(
    "kcreate/canvas/createRect",
    (
      _e,
      parentId: string | null,
      x: number,
      y: number,
      w: number,
      h: number,
    ) => requireBridge().canvasCreateRect(parentId, x, y, w, h),
  );
  ipcMain.handle(
    "kcreate/canvas/createEllipse",
    (
      _e,
      parentId: string | null,
      cx: number,
      cy: number,
      rx: number,
      ry: number,
    ) => requireBridge().canvasCreateEllipse(parentId, cx, cy, rx, ry),
  );
  ipcMain.handle(
    "kcreate/canvas/createLine",
    (
      _e,
      parentId: string | null,
      x1: number,
      y1: number,
      x2: number,
      y2: number,
    ) => requireBridge().canvasCreateLine(parentId, x1, y1, x2, y2),
  );
  ipcMain.handle(
    "kcreate/canvas/createText",
    (
      _e,
      parentId: string | null,
      x: number,
      y: number,
      text: string,
      fontFamily: string,
      fontSize: number,
    ) =>
      requireBridge().canvasCreateText(
        parentId,
        x,
        y,
        text,
        fontFamily,
        fontSize,
      ),
  );
  ipcMain.handle(
    "kcreate/canvas/moveNode",
    (_e, nodeId: string, dx: number, dy: number) => {
      requireBridge().canvasMoveNode(nodeId, dx, dy);
    },
  );

  // AI Assist
  ipcMain.handle(
    "kcreate/ai/removeBackground",
    (_e, nodeId: string) => requireBridge().aiRemoveBackground(nodeId),
  );
  ipcMain.handle("kcreate/ai/getActionLog", () =>
    requireBridge().aiGetActionLog(),
  );

  // Local MCP server. Loopback-only, opt-in. The renderer cannot bind
  // to anything other than 127.0.0.1 — the server addr is hard-coded in
  // kcreate_mcp::server.
  ipcMain.handle("kcreate/mcp/start", () => requireBridge().mcpStart());
  ipcMain.handle("kcreate/mcp/stop", () => {
    requireBridge().mcpStop();
  });
  ipcMain.handle("kcreate/mcp/isRunning", () =>
    requireBridge().mcpIsRunning(),
  );

  // Design tokens / brand kits / export presets (Task 19). Mutations
  // do not auto-save — the host should call project/save when it
  // wants the changes to land on disk.
  ipcMain.handle("kcreate/designTokens/get", () =>
    requireBridge().designTokensGet(),
  );
  ipcMain.handle(
    "kcreate/designTokens/set",
    (_e, tokensJson: string) => {
      requireBridge().designTokensSet(tokensJson);
    },
  );
  ipcMain.handle("kcreate/brandKit/create", (_e, name: string) =>
    requireBridge().brandKitCreate(name),
  );
  ipcMain.handle("kcreate/brandKit/update", (_e, kitJson: string) => {
    requireBridge().brandKitUpdate(kitJson);
  });
  ipcMain.handle("kcreate/brandKit/list", () =>
    requireBridge().brandKitList(),
  );
  ipcMain.handle("kcreate/brandKit/delete", (_e, kitId: string) =>
    requireBridge().brandKitDelete(kitId),
  );
  ipcMain.handle(
    "kcreate/exportPreset/create",
    (_e, name: string, format: string, scale: number) =>
      requireBridge().exportPresetCreate(name, format, scale),
  );
  ipcMain.handle("kcreate/exportPreset/list", () =>
    requireBridge().exportPresetList(),
  );
  ipcMain.handle("kcreate/exportPreset/delete", (_e, presetId: string) =>
    requireBridge().exportPresetDelete(presetId),
  );

  ipcMain.handle(
    "kcreate/artboard/create",
    (
      _e,
      pageId: string,
      name: string,
      width: number,
      height: number,
    ): string =>
      requireBridge().artboardCreate(
        pageId.length > 0 ? pageId : null,
        name,
        width,
        height,
      ),
  );
  ipcMain.handle("kcreate/artboard/list", () =>
    requireBridge().artboardList(),
  );
  ipcMain.handle(
    "kcreate/artboard/duplicate",
    (_e, artboardId: string): string =>
      requireBridge().artboardDuplicate(artboardId),
  );
  ipcMain.handle(
    "kcreate/artboard/resize",
    (_e, artboardId: string, width: number, height: number): void => {
      requireBridge().artboardResize(artboardId, width, height);
    },
  );
  ipcMain.handle("kcreate/artboard/presets", () =>
    requireBridge().artboardPresets(),
  );

  // Components (Block B).
  ipcMain.handle(
    "kcreate/component/createFromSelection",
    (_e, nodeIds: string[], name: string): string =>
      requireBridge().componentCreateFromSelection(nodeIds, name),
  );
  ipcMain.handle("kcreate/component/list", () =>
    requireBridge().componentList(),
  );
  ipcMain.handle(
    "kcreate/component/instantiate",
    (
      _e,
      componentId: string,
      parentId: string,
      x: number,
      y: number,
    ): string =>
      requireBridge().componentInstantiate(
        componentId,
        parentId.length > 0 ? parentId : null,
        x,
        y,
      ),
  );
  ipcMain.handle(
    "kcreate/component/addVariant",
    (_e, componentId: string, name: string): string =>
      requireBridge().componentAddVariant(componentId, name),
  );
  ipcMain.handle(
    "kcreate/component/switchVariant",
    (_e, nodeId: string, variantId: string): void => {
      requireBridge().componentSwitchVariant(nodeId, variantId);
    },
  );
  ipcMain.handle(
    "kcreate/component/detach",
    (_e, nodeId: string): void => {
      requireBridge().componentDetach(nodeId);
    },
  );
  ipcMain.handle(
    "kcreate/layout/setFlex",
    (_e, nodeId: string, layoutJson: string): void => {
      requireBridge().layoutSetFlex(nodeId, layoutJson);
    },
  );
  ipcMain.handle(
    "kcreate/layout/setGrid",
    (_e, nodeId: string, layoutJson: string): void => {
      requireBridge().layoutSetGrid(nodeId, layoutJson);
    },
  );
  ipcMain.handle("kcreate/layout/recompute", (_e, nodeId: string): void => {
    requireBridge().layoutRecompute(nodeId);
  });
  ipcMain.handle(
    "kcreate/layout/convertToFrame",
    (_e, nodeId: string): void => {
      requireBridge().layoutConvertToFrame(nodeId);
    },
  );

  // Prototype interactions (Phase 1, Block A)
  ipcMain.handle(
    "kcreate/interaction/add",
    (_e, nodeId: string, trigger: string, actionJson: string): string =>
      requireBridge().interactionAdd(nodeId, trigger, actionJson),
  );
  ipcMain.handle(
    "kcreate/interaction/remove",
    (_e, nodeId: string, interactionId: string): boolean =>
      requireBridge().interactionRemove(nodeId, interactionId),
  );
  ipcMain.handle(
    "kcreate/interaction/list",
    (_e, nodeId: string): string => requireBridge().interactionList(nodeId),
  );
  ipcMain.handle(
    "kcreate/interaction/list-batch",
    (_e, nodeIds: string[]): string =>
      requireBridge().interactionListBatch(JSON.stringify(nodeIds)),
  );

  // Layout Studio (Phase 2, Block B): page layout, master pages, templates
  ipcMain.handle(
    "kcreate/page/setLayout",
    (_e, pageId: string, layoutJson: string): void => {
      requireBridge().pageSetLayout(pageId, layoutJson);
    },
  );
  ipcMain.handle(
    "kcreate/page/getLayout",
    (_e, pageId: string): string => requireBridge().pageGetLayout(pageId),
  );
  ipcMain.handle(
    "kcreate/masterPage/create",
    (_e, name: string, size: string, orientation: string): string =>
      requireBridge().masterPageCreate(name, size, orientation),
  );
  ipcMain.handle("kcreate/masterPage/list", (): string =>
    requireBridge().masterPageList(),
  );
  ipcMain.handle(
    "kcreate/masterPage/apply",
    (_e, contentPageId: string, masterPageId: string): void => {
      requireBridge().masterPageApply(contentPageId, masterPageId);
    },
  );
  ipcMain.handle(
    "kcreate/masterPage/detach",
    (_e, contentPageId: string): void => {
      requireBridge().masterPageDetach(contentPageId);
    },
  );
  ipcMain.handle("kcreate/layoutTemplate/list", (): string =>
    requireBridge().layoutTemplateList(),
  );
  ipcMain.handle(
    "kcreate/layoutTemplate/apply",
    (_e, templateId: string): string =>
      requireBridge().layoutTemplateApply(templateId),
  );
  ipcMain.handle(
    "kcreate/page/add",
    (
      _e,
      name: string,
      size: string | undefined,
      orientation: string | undefined,
    ): string => requireBridge().pageAdd(name, size, orientation),
  );
  ipcMain.handle("kcreate/page/duplicate", (_e, pageId: string): string =>
    requireBridge().pageDuplicate(pageId),
  );
  ipcMain.handle(
    "kcreate/document/reparent",
    (
      _e,
      nodeId: string,
      newParent: string | null | undefined,
      index: number,
    ): void => {
      requireBridge().documentReparentNode(
        nodeId,
        newParent ?? undefined,
        index,
      );
    },
  );

  // ---------------------------------------------------------------------
  // Phase 2 — preflight, icon pack, batch async, AI extras, plugins, MCP perms.
  // ---------------------------------------------------------------------
  ipcMain.handle("kcreate/preflight/run", (_e, requestJson: string) =>
    requireBridge().preflightRun(requestJson),
  );
  ipcMain.handle(
    "kcreate/export/iconPack",
    (_e, requestJson: string) => requireBridge().exportIconPack(requestJson),
  );
  ipcMain.handle("kcreate/export/iconPack/builtInPlatforms", () =>
    requireBridge().exportIconPackBuiltInPlatforms(),
  );
  ipcMain.handle("kcreate/export/batch/start", (_e, jobJson: string) =>
    requireBridge().exportBatchStart(jobJson),
  );
  ipcMain.handle("kcreate/export/batch/status", (_e, jobId: string) =>
    requireBridge().exportBatchStatus(jobId),
  );
  ipcMain.handle("kcreate/export/batch/cancel", (_e, jobId: string) => {
    requireBridge().exportBatchCancel(jobId);
  });
  ipcMain.handle("kcreate/export/batch/dismiss", (_e, jobId: string) =>
    requireBridge().exportBatchDismiss(jobId),
  );
  ipcMain.handle(
    "kcreate/ai/upscale",
    (_e, nodeId: string, scale: number) =>
      requireBridge().aiUpscale(nodeId, scale),
  );
  ipcMain.handle(
    "kcreate/ai/extractPalette",
    (_e, nodeId: string, maxColors: number) =>
      requireBridge().aiExtractPalette(nodeId, maxColors),
  );
  ipcMain.handle(
    "kcreate/ai/smartSelect",
    (_e, nodeId: string, x: number, y: number, tolerance: number) =>
      requireBridge().aiSmartSelect(nodeId, x, y, tolerance),
  );
  ipcMain.handle("kcreate/ai/listModelPacks", () =>
    requireBridge().aiListModelPacks(),
  );
  ipcMain.handle(
    "kcreate/ai/screenshotToLayout",
    (_e, requestJson: string) =>
      requireBridge().aiScreenshotToLayout(requestJson),
  );
  ipcMain.handle("kcreate/plugin/list", () =>
    requireBridge().pluginList(),
  );
  ipcMain.handle("kcreate/plugin/enable", (_e, id: string) => {
    requireBridge().pluginEnable(id);
  });
  ipcMain.handle("kcreate/plugin/disable", (_e, id: string) => {
    requireBridge().pluginDisable(id);
  });
  ipcMain.handle(
    "kcreate/plugin/execute",
    (_e, id: string, fn: string, input: string) =>
      requireBridge().pluginExecute(id, fn, input),
  );
  ipcMain.handle("kcreate/mcp/permission/list", () =>
    requireBridge().mcpPermissionList(),
  );
  ipcMain.handle(
    "kcreate/mcp/permission/grant",
    (_e, clientId: string, toolName: string, grant: string) => {
      requireBridge().mcpPermissionGrant(clientId, toolName, grant);
    },
  );
  ipcMain.handle(
    "kcreate/mcp/permission/revoke",
    (_e, clientId: string, toolName: string) => {
      requireBridge().mcpPermissionRevoke(clientId, toolName);
    },
  );
  ipcMain.handle("kcreate/mcp/status", () =>
    requireBridge().mcpStatus(),
  );

  // ---------------------------------------------------------------------
  // Phase 2 — color management (ICC / CMYK foundation)
  // ---------------------------------------------------------------------
  ipcMain.handle("kcreate/color/settings/get", () =>
    requireBridge().colorSettingsGet(),
  );
  ipcMain.handle(
    "kcreate/color/settings/update",
    (_e, settingsJson: string) => {
      requireBridge().colorSettingsUpdate(settingsJson);
    },
  );
  ipcMain.handle(
    "kcreate/color/convert",
    (_e, fromJson: string, toSpace: string) =>
      requireBridge().colorConvert(fromJson, toSpace),
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

// Final cleanup before the process exits. Three concerns, in order:
//   1. **Close any open project.** Releases the SQLite file handle so
//      `cleanupScratchProjects` (next step) can actually delete the
//      `.kstudio` directory on Windows, where mandatory file locking
//      would otherwise leave the SQLite file in a "delete pending"
//      state and the parent directory non-empty.
//   2. **Sweep scratch projects.** The renderer creates one
//      `scratch-*.kstudio` per Home→Editor click. The next sweep would
//      catch them, but only if the app is opened again — if the user
//      quits, the directory leaks until the next session. macOS/Linux
//      temp reapers eventually clean up; Windows `%TEMP%` accumulates
//      indefinitely. Sweeping here closes that gap (issue ANALYSIS_0005).
//   3. **Shutdown the renderer.** Drops the wgpu adapter, the readback
//      buffers, and the GPU device. Last step because nothing else
//      depends on it.
//
// Electron's `will-quit` fires synchronously. To run async cleanup
// safely we use the standard `event.preventDefault()` + re-quit
// pattern: prevent the first quit, run cleanup, then re-issue
// `app.quit()` which re-fires `will-quit` with `didFinalCleanup`
// guarding us against infinite recursion.
let didFinalCleanup = false;
app.on("will-quit", (event) => {
  if (didFinalCleanup) return;
  event.preventDefault();
  void (async () => {
    try {
      // Drop the lockfile *first* so our own scratch dir becomes
      // eligible for the sweep two lines below. Doing this before
      // `projectClose` is intentional: the bridge releases the
      // SQLite file on close, but the lockfile lives on disk and is
      // managed entirely by the host — there is no dependency
      // ordering between the two cleanups.
      if (ownedScratchPath !== null) {
        await removeScratchLockfile(ownedScratchPath);
        ownedScratchPath = null;
      }
      if (bridge) {
        // `projectClose` is sync and infallible (it just drops the
        // workspace slot). Wrap in try so a bug here never blocks
        // shutdown.
        try {
          bridge.projectClose();
        } catch {
          // best-effort
        }
      }
      // Sweep AFTER projectClose so the just-closed scratch directory
      // is itself eligible for deletion. The lockfile check inside
      // `cleanupScratchProjects` still protects directories belonging
      // to *other* running KCreate instances.
      await cleanupScratchProjects();
      if (bridge) {
        try {
          bridge.rendererShutdown();
        } catch {
          // best-effort
        }
        bridge = null;
      }
    } finally {
      didFinalCleanup = true;
      app.quit();
    }
  })();
});
