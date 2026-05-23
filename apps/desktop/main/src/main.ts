// Electron main process entry. Owns the BrowserWindow and the
// renderer-side IPC handlers that proxy to the Rust kcreate_bridge
// native addon.

import {
  app,
  BrowserWindow,
  dialog,
  ipcMain,
  WebContentsView,
  type IpcMainInvokeEvent,
} from "electron";
import * as fs from "node:fs/promises";
import { realpathSync } from "node:fs";
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

// ---------------------------------------------------------------------
// JS panel plugin lifecycle.
//
// Each enabled `js_panel` plugin gets at most one sandboxed
// `WebContentsView` attached to the main window. The renderer asks
// the host to mount / unmount / resize panels through
// `kcreate/plugin/js/open` / `close` / `setBounds`. Messages from
// the panel itself come in on `kcreate/plugin/js/panel/send`, which
// the plugin preload (`plugin-preload.ts`) wires under
// `window.kcreatePlugin.sendMessage`.
//
// The host is the gate: it knows which WebContents belongs to which
// plugin, so even though the plugin can post anything from inside
// its sandbox, the bridge always sees a trusted `(pluginId,
// messageJson)` pair.
//
// Security stance for every panel view:
//   * `sandbox: true`           — chromium sandbox is on
//   * `contextIsolation: true`  — plugin can't reach the host
//   * `nodeIntegration: false`  — no Node.js APIs in the panel
//   * `preload`                 — `plugin-preload.js` only
//   * `webSecurity: true`       — same-origin policy stays on
//   * CSP                       — set on every loaded page (see below)
// ---------------------------------------------------------------------

/// Pixel bounds passed in by the renderer when (re-)mounting a panel.
interface JsPanelBounds {
  x: number;
  y: number;
  width: number;
  height: number;
}

/// A single mounted JS panel.
interface MountedJsPanel {
  pluginId: string;
  view: WebContentsView;
  /// `WebContents.id` — used by the panel-send IPC handler to match
  /// inbound messages from the panel back to its plugin id without
  /// trusting the panel itself.
  webContentsId: number;
}

const mountedJsPanels = new Map<string, MountedJsPanel>();
const webContentsIdToPluginId = new Map<number, string>();

function jsPanelPreloadPath(): string {
  return path.join(__dirname, "plugin-preload.js");
}

/// Resolve a plugin's directory and entry HTML path. The bridge
/// returns the list with `entry_html` relative to the plugin dir
/// (per `crates/kcreate_plugin/src/js_panel.rs` schema). We honour
/// `KCREATE_PLUGIN_DIR` for parity with the Rust side; otherwise
/// fall back to `~/.kcreate/plugins`.
function pluginRoot(): string {
  const fromEnv = process.env["KCREATE_PLUGIN_DIR"];
  if (fromEnv) return fromEnv;
  const home = process.env["HOME"] ?? process.env["USERPROFILE"] ?? ".";
  return path.join(home, ".kcreate", "plugins");
}

interface JsPanelInfoFromBridge {
  id: string;
  config: { entry_html: string };
}

function resolveJsPanelEntry(pluginId: string): string | null {
  if (!bridge) return null;
  let listJson: string;
  try {
    listJson = bridge.pluginJsList();
  } catch {
    return null;
  }
  let list: JsPanelInfoFromBridge[];
  try {
    list = JSON.parse(listJson) as JsPanelInfoFromBridge[];
  } catch {
    return null;
  }
  const entry = list.find((p) => p.id === pluginId);
  if (!entry) return null;
  // The Rust manifest validator already runs `ensure_path_within` on
  // `entry_html` at registration time (see
  // `crates/kcreate_plugin/src/manifest.rs::validate_against_dir`),
  // so a path-traversal `entry_html` would have been rejected before
  // the plugin was ever listed here. We re-check with a `realpath`
  // containment test for defence-in-depth: if the on-disk plugin
  // directory was tampered with between registration and now (e.g.
  // someone symlinked the panel.html to /etc/passwd while the editor
  // was running), we still refuse to `file://`-load anything outside
  // the plugin root.
  const candidate = path.join(pluginRoot(), pluginId, entry.config.entry_html);
  const pluginDirAbs = path.resolve(pluginRoot(), pluginId);
  let resolvedCandidate: string;
  let resolvedRoot: string;
  try {
    resolvedCandidate = realpathSync(candidate);
    resolvedRoot = realpathSync(pluginDirAbs);
  } catch {
    // Either the file or the plugin directory doesn't exist; refuse
    // to load. Returning `null` propagates as "plugin not found" up
    // to `openJsPanel`, which is the right user-facing error.
    return null;
  }
  // `relative` returns "" or a non-`..`-prefixed string when the
  // candidate is inside the root; anything else means the symlink
  // walk escaped the plugin sandbox.
  const rel = path.relative(resolvedRoot, resolvedCandidate);
  if (rel.startsWith("..") || path.isAbsolute(rel)) {
    return null;
  }
  return resolvedCandidate;
}

function destroyJsPanel(pluginId: string): void {
  const mounted = mountedJsPanels.get(pluginId);
  if (!mounted) return;
  mountedJsPanels.delete(pluginId);
  webContentsIdToPluginId.delete(mounted.webContentsId);
  if (mainWindow) {
    try {
      mainWindow.contentView.removeChildView(mounted.view);
    } catch {
      // The window may already have torn down; ignore.
    }
  }
  // `WebContents.destroy()` is the only way to truly free a view; the
  // alternative (`view.webContents.close()`) leaves the renderer
  // process alive until the parent window closes. Use `close()` if
  // `destroy()` isn't available on the current Electron version.
  const wc = mounted.view.webContents as unknown as {
    destroy?: () => void;
    close?: () => void;
  };
  if (typeof wc.destroy === "function") {
    wc.destroy();
  } else if (typeof wc.close === "function") {
    wc.close();
  }
}

/// Tear down every mounted JS panel. Called on `will-quit` so the
/// child processes don't outlive the editor.
function destroyAllJsPanels(): void {
  for (const id of Array.from(mountedJsPanels.keys())) {
    destroyJsPanel(id);
  }
}

function openJsPanel(pluginId: string, bounds: JsPanelBounds): void {
  if (!mainWindow) {
    throw new Error(
      "kcreate/plugin/js/open: main window has not been created yet",
    );
  }
  const entry = resolveJsPanelEntry(pluginId);
  if (!entry) {
    throw new Error(
      `kcreate/plugin/js/open: plugin ${pluginId} is not a known js_panel plugin`,
    );
  }
  // If already mounted, just reposition.
  const existing = mountedJsPanels.get(pluginId);
  if (existing) {
    existing.view.setBounds(bounds);
    return;
  }

  // Each panel gets its own ephemeral (non-`persist:`) session partition.
  //
  // Without this, every `WebContentsView` shares Electron's default
  // session — the same one the main renderer uses to load its own
  // `file://` HTML. Registering `webRequest.onHeadersReceived` on the
  // default session would inject the panel's strict CSP
  // (`connect-src 'none'`, `form-action 'none'`, …) into *every*
  // `file://` response in the process, breaking the main app shell.
  // It would also leave only the last-mounted panel's handler in
  // place, because `onHeadersReceived` is single-listener-per-session.
  //
  // The partition string is non-`persist:` on purpose: panel sessions
  // are wiped on app exit so a plugin can never persist cookies /
  // localStorage / IndexedDB across launches. The `pluginId` segment
  // gives each plugin its own isolated session so plugins can't read
  // each other's web storage either.
  const partition = `plugin-panel:${pluginId}`;
  const view = new WebContentsView({
    webPreferences: {
      partition,
      preload: jsPanelPreloadPath(),
      sandbox: true,
      contextIsolation: true,
      nodeIntegration: false,
      webSecurity: true,
      // Disable everything we don't need. The panel speaks to the host
      // through the preload only.
      allowRunningInsecureContent: false,
      experimentalFeatures: false,
      // Plugin pages are local files; we don't need devtools open by
      // default in production.
    },
  });
  view.setBounds(bounds);
  // Inject a strict CSP via the protocol header before load. Phase 2
  // bans the panel from making any network requests: `default-src
  // 'self' file:; connect-src 'none'`. The plugin's HTML can still
  // pull in sibling JS/CSS via `file://` because both are on the same
  // local origin.
  //
  // The handler is registered on the panel's *own* session (set by
  // `partition` above), not on `session.defaultSession`, so it
  // applies only to this panel's `file://` loads.
  view.webContents.session.webRequest.onHeadersReceived(
    { urls: ["file://*/*"] },
    (details, callback) => {
      const headers = { ...details.responseHeaders };
      headers["Content-Security-Policy"] = [
        "default-src 'self' file:; " +
          "script-src 'self' file:; " +
          "style-src 'self' file: 'unsafe-inline'; " +
          "connect-src 'none'; " +
          "object-src 'none'; " +
          "base-uri 'self'; " +
          "form-action 'none'",
      ];
      callback({ responseHeaders: headers });
    },
  );

  const webContentsId = view.webContents.id;
  webContentsIdToPluginId.set(webContentsId, pluginId);
  mountedJsPanels.set(pluginId, { pluginId, view, webContentsId });

  mainWindow.contentView.addChildView(view);
  void view.webContents.loadFile(entry);
}

/// Resolve the plugin id for an IPC sender. Returns `null` for
/// senders that aren't an enrolled JS panel — those are spoofed
/// `panel/send` messages from the main renderer or some other
/// WebContents and should be rejected.
function pluginIdForSender(event: IpcMainInvokeEvent): string | null {
  return webContentsIdToPluginId.get(event.sender.id) ?? null;
}

/// Returns `true` iff the IPC sender is the main editor renderer
/// (i.e. our top-level BrowserWindow). Used as a defence-in-depth
/// guard on the *trusted* JS-panel IPC channels: even though the
/// main renderer is the only one with `window.kcreate.*` exposed,
/// a sandbox-bypass in some future Electron release should not be
/// able to call into `pluginJsMessage` on behalf of another plugin
/// from inside a panel's WebContents.
function isMainRendererSender(event: IpcMainInvokeEvent): boolean {
  const win = mainWindow;
  if (!win || win.isDestroyed()) return false;
  return event.sender.id === win.webContents.id;
}

/// Push-broadcast a "color settings changed" event to the main
/// renderer. Phase 2 ships a single mainWindow but we still gate on
/// destruction so a late-firing handler doesn't crash during quit.
///
/// We deliberately do NOT include the new settings payload — keeping
/// the event content-free lets the renderer issue a single fresh
/// `colorSettingsGet()` IPC, which is the same shape it would have
/// fetched on mount, and avoids leaking the wire format into the IPC
/// event channel (which has no schema enforcement). The fetch is
/// effectively free: `color_settings_get` returns a cloned struct
/// behind the workspace mutex.
function broadcastColorSettingsChanged(): void {
  const win = mainWindow;
  if (!win || win.isDestroyed()) return;
  win.webContents.send("kcreate/color/settings/changed");
}

/// Period (ms) for the Phase 3 collab event-drain timer. Balances
/// "remote cursors feel responsive" (smaller is better) against
/// "we don't waste a workspace lock + IPC round-trip when nothing
/// is happening" (larger is better). 50ms gives 20Hz cursor
/// updates, which matches WebSocket-based collab tools (Figma /
/// Miro use 30–60Hz).
const SESSION_EVENT_TICK_MS = 50;

/// Process-wide handle for the session event-drain timer. `null`
/// when no session is running; rest of the code reads `null` as
/// "tick is stopped" and avoids the bridge call entirely.
let sessionEventTickHandle: NodeJS.Timeout | null = null;

function startSessionEventTick(): void {
  if (sessionEventTickHandle !== null) return;
  sessionEventTickHandle = setInterval(() => {
    drainSessionEvents();
  }, SESSION_EVENT_TICK_MS);
  // Unref so the timer never holds Electron in the event loop on
  // its own — quit() should not have to remember to stop it.
  sessionEventTickHandle.unref();
}

function stopSessionEventTick(): void {
  if (sessionEventTickHandle === null) return;
  clearInterval(sessionEventTickHandle);
  sessionEventTickHandle = null;
}

/// Drain pending session events from the bridge and forward each
/// entry to the renderer. If any event is a presence/peer update,
/// re-publish the scene so the cursor overlay refreshes
/// (`sync_scene_locked` in the bridge appends remote cursors to
/// every scene it builds).
///
/// Quiet on error: the most likely failures are "no session is
/// running" (race with `session_leave`) and "bridge not yet
/// loaded" — both safe to ignore at tick rate.
function drainSessionEvents(): void {
  const win = mainWindow;
  if (!win || win.isDestroyed()) return;
  let payload: string;
  try {
    payload = requireBridge().sessionDrainEvents();
  } catch {
    // Session was torn down between ticks; nothing to forward.
    return;
  }
  let parsed: Array<{ kind: string }>;
  try {
    parsed = JSON.parse(payload) as Array<{ kind: string }>;
  } catch {
    return;
  }
  if (parsed.length === 0) return;
  let needsRender = false;
  for (const ev of parsed) {
    win.webContents.send("kcreate/session/event", JSON.stringify(ev));
    if (
      ev.kind === "presenceUpdated" ||
      ev.kind === "peerLeft" ||
      ev.kind === "peerJoined"
    ) {
      needsRender = true;
    }
  }
  if (needsRender) {
    try {
      requireBridge().documentRequestRender();
    } catch {
      // No project loaded yet, or renderer not initialised. Both
      // are no-ops as far as the cursor overlay is concerned.
    }
  }
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
  // `kcreate/document/undo` and `.../redo` may roll back / replay a
  // `color_settings_update` operation as of Phase 2 (the bridge owns
  // the dispatch — see `crates/kcreate_bridge/src/document.rs`'s
  // `apply_inverse_patch`). The bridge now returns the rolled-back
  // operation's `command` string alongside the affected node ids, so
  // we gate the `color/settings/changed` broadcast on the command
  // actually being one of the color-settings ops. This avoids an
  // unnecessary React re-render on every unrelated undo / redo
  // (e.g. a `move_node`) — flagged in Devin Review on commit 7b5d49a.
  //
  // Other operations the bridge may need to push-notify on in the
  // future should slot into this dispatch table; keep the table
  // explicit (not a string-prefix match) so we don't accidentally
  // broadcast on a future op whose name happens to contain
  // `color_settings`.
  const broadcastForCommand = (command: string): void => {
    switch (command) {
      case "color_settings_update":
        broadcastColorSettingsChanged();
        break;
      default:
        // No-op: most ops affect node graph state only and the
        // renderer's existing tree-refresh path handles them.
        break;
    }
  };
  ipcMain.handle("kcreate/document/undo", () => {
    const result = requireBridge().documentUndo();
    if (result) broadcastForCommand(result.command);
    return result;
  });
  ipcMain.handle("kcreate/document/redo", () => {
    const result = requireBridge().documentRedo();
    if (result) broadcastForCommand(result.command);
    return result;
  });
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
  // `kcreate/ai/installModelPack` is a *trusted* main-process IPC:
  // the renderer hands us the user-picked file path returned by the
  // file-picker (`kcreate/ai/pickModelFile` below), and we pass it
  // through to the Rust installer. The Rust side verifies SHA-256
  // (if the registry has a pinned hash) and atomically renames the
  // file into `models_dir` — we never have to deal with partial
  // writes or hash drift in the IPC layer.
  ipcMain.handle(
    "kcreate/ai/installModelPack",
    (_e, packId: string, sourcePath: string) =>
      requireBridge().aiInstallModelPack(packId, sourcePath),
  );
  ipcMain.handle("kcreate/ai/uninstallModelPack", (_e, packId: string) => {
    requireBridge().aiUninstallModelPack(packId);
  });
  // Native file picker scoped to weights files (ONNX / GGUF / a
  // wildcard fallback so users can still install future formats
  // without a code change). Returns the absolute path or `null` if
  // the user cancelled. Done in the main process because Electron's
  // `dialog` module is main-only — exposing it through the preload
  // would be an unnecessary surface expansion.
  ipcMain.handle("kcreate/ai/pickModelFile", async () => {
    const win = mainWindow;
    if (!win) return null;
    const result = await dialog.showOpenDialog(win, {
      title: "Select downloaded model weights",
      properties: ["openFile"],
      filters: [
        { name: "Model weights", extensions: ["onnx", "gguf", "safetensors"] },
        { name: "All files", extensions: ["*"] },
      ],
    });
    if (result.canceled || result.filePaths.length === 0) return null;
    return result.filePaths[0];
  });
  // `kcreate/pdf/pickFile` opens an Electron-native file picker
  // scoped to .pdf so the renderer can hand the resolved path to
  // `kcreate/pdf/import`. We keep the picker in the main process
  // (instead of the renderer) so the renderer never sees the
  // underlying filesystem — the renderer only ever gets a single
  // user-chosen path back.
  ipcMain.handle("kcreate/pdf/pickFile", async () => {
    const win = mainWindow;
    if (!win) return null;
    const result = await dialog.showOpenDialog(win, {
      title: "Import PDF",
      properties: ["openFile"],
      filters: [
        { name: "PDF", extensions: ["pdf"] },
        { name: "All files", extensions: ["*"] },
      ],
    });
    if (result.canceled || result.filePaths.length === 0) return null;
    return result.filePaths[0];
  });
  ipcMain.handle("kcreate/pdf/import", (_e, filePath: string) =>
    requireBridge().pdfImport(filePath),
  );
  ipcMain.handle(
    "kcreate/ai/screenshotToLayout",
    (_e, requestJson: string) =>
      requireBridge().aiScreenshotToLayout(requestJson),
  );
  ipcMain.handle(
    "kcreate/ai/altTextForNode",
    (_e, nodeId: string) =>
      requireBridge().aiAltTextForNode(nodeId),
  );
  ipcMain.handle(
    "kcreate/ai/applyAltText",
    (_e, nodeId: string, text: string) => {
      requireBridge().aiApplyAltText(nodeId, text);
    },
  );
  ipcMain.handle(
    "kcreate/ai/layoutSuggestForArtboard",
    (_e, artboardId: string) =>
      requireBridge().aiLayoutSuggestForArtboard(artboardId),
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
  ipcMain.handle(
    "kcreate/plugin/executeWithContext",
    (_e, id: string, fn: string, input: string) =>
      requireBridge().pluginExecuteWithContext(id, fn, input),
  );
  ipcMain.handle("kcreate/plugin/js/list", () =>
    requireBridge().pluginJsList(),
  );
  ipcMain.handle("kcreate/plugin/trust/list", () =>
    requireBridge().pluginTrustList(),
  );
  ipcMain.handle("kcreate/plugin/trust/reload", () => {
    requireBridge().pluginTrustReload();
  });
  // `kcreate/plugin/js/message` is the *trusted* JS-panel IPC: the
  // caller passes `(pluginId, messageJson)` directly and we forward
  // both to the bridge. Only the main editor renderer is allowed to
  // call it — defence-in-depth against a sandbox-bypass in a JS panel
  // WebContents that might otherwise claim to be any pluginId. The
  // main renderer already has full bridge access (it can call
  // `pluginExecuteWithContext`, mutate the document, etc.), so the
  // check here is purely a sender-attestation gate.
  //
  // The *untrusted* path — messages originating from inside a sandboxed
  // panel — goes through `kcreate/plugin/js/panel/send` below, which
  // looks up `pluginIdForSender(event)` so the panel can't impersonate
  // another plugin even if it tampers with its own postMessage payload.
  ipcMain.handle(
    "kcreate/plugin/js/message",
    (event: IpcMainInvokeEvent, pluginId: string, messageJson: string) => {
      if (!isMainRendererSender(event)) {
        // A JS panel's preload should never reach this channel; if it
        // does, the panel is either misconfigured or attempting to
        // impersonate the main renderer. Return a `status: invalid`
        // envelope rather than throw — that matches the panel/send
        // contract on the renderer side.
        return JSON.stringify({
          status: "invalid",
          reason: "channel is only callable from the main editor renderer",
        });
      }
      return requireBridge().pluginJsMessage(pluginId, messageJson);
    },
  );
  ipcMain.handle(
    "kcreate/plugin/js/open",
    (_e, pluginId: string, bounds: JsPanelBounds) => {
      openJsPanel(pluginId, bounds);
    },
  );
  ipcMain.handle("kcreate/plugin/js/close", (_e, pluginId: string) => {
    destroyJsPanel(pluginId);
  });
  ipcMain.handle(
    "kcreate/plugin/js/setBounds",
    (_e, pluginId: string, bounds: JsPanelBounds) => {
      const mounted = mountedJsPanels.get(pluginId);
      if (!mounted) return;
      mounted.view.setBounds(bounds);
    },
  );
  // The panel's preload calls this. We trust the sender's WebContents
  // id (because the host stamped it when the panel was mounted), but
  // NOT any pluginId the panel might claim. A message from a
  // non-panel sender is silently dropped — it's a spoofing attempt.
  ipcMain.handle(
    "kcreate/plugin/js/panel/send",
    (event: IpcMainInvokeEvent, messageJson: string) => {
      const pluginId = pluginIdForSender(event);
      if (!pluginId) {
        return JSON.stringify({
          status: "invalid",
          reason: "sender is not a registered JS panel",
        });
      }
      return requireBridge().pluginJsMessage(pluginId, messageJson);
    },
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
      // Push-notify so `SoftProofOverlay` (and any future
      // color-aware UI) can react synchronously instead of waiting
      // on a 2-second polling tick. See
      // `apps/desktop/renderer/src/components/SoftProofOverlay.tsx`
      // for the subscriber side.
      broadcastColorSettingsChanged();
    },
  );
  ipcMain.handle(
    "kcreate/color/convert",
    (_e, fromJson: string, toSpace: string) =>
      requireBridge().colorConvert(fromJson, toSpace),
  );

  // ---------------------------------------------------------------------
  // Phase 2 — text frame + OpenType (Block B Task 11)
  // ---------------------------------------------------------------------
  ipcMain.handle("kcreate/text/frame/get", (_e, nodeId: string) =>
    requireBridge().textFrameGet(nodeId),
  );
  ipcMain.handle(
    "kcreate/text/frame/update",
    (_e, nodeId: string, optionsJson: string) => {
      requireBridge().textFrameUpdate(nodeId, optionsJson);
    },
  );
  ipcMain.handle("kcreate/text/layout/compute", (_e, nodeId: string) =>
    requireBridge().textLayoutCompute(nodeId),
  );
  ipcMain.handle("kcreate/text/opentype/get", (_e, nodeId: string) =>
    requireBridge().textOpentypeFeaturesGet(nodeId),
  );
  ipcMain.handle(
    "kcreate/text/opentype/update",
    (_e, nodeId: string, featuresJson: string) => {
      requireBridge().textOpentypeFeaturesUpdate(nodeId, featuresJson);
    },
  );

  // ---------------------------------------------------------------------
  // Phase 3 — LAN collaboration session
  //
  // The bridge exposes seven entry points
  // (`session_{start,leave,join,peers,drain_events,send_presence,info}`)
  // plus `document_request_render` for the cursor-overlay refresh
  // path. The main process plumbs them all through `kcreate/session/*`
  // channels; the renderer subscribes to a single push channel
  // (`kcreate/session/event`) for live updates.
  //
  // We start a 50ms timer when the first `start` succeeds and stop
  // it on `leave`. The timer polls `sessionDrainEvents`, fans the
  // resulting JSON event array out to the renderer, and — if any
  // entry is a presence update — re-publishes the scene so remote
  // cursors animate without waiting for a local document mutation.
  // ---------------------------------------------------------------------
  ipcMain.handle(
    "kcreate/session/start",
    (
      _e,
      seedB64: string,
      displayName: string,
      projectId: string,
      advertiseMdns: boolean,
    ) => {
      const report = requireBridge().sessionStart(
        seedB64,
        displayName,
        projectId,
        advertiseMdns,
      );
      startSessionEventTick();
      return report;
    },
  );
  ipcMain.handle("kcreate/session/leave", () => {
    stopSessionEventTick();
    requireBridge().sessionLeave();
  });
  ipcMain.handle(
    "kcreate/session/join",
    (
      _e,
      peerId: string,
      publicKey: string,
      displayName: string,
      socketAddr: string,
      certFingerprintB64: string,
    ) => {
      requireBridge().sessionJoin(
        peerId,
        publicKey,
        displayName,
        socketAddr,
        certFingerprintB64,
      );
    },
  );
  ipcMain.handle("kcreate/session/peers", () =>
    requireBridge().sessionPeers(),
  );
  ipcMain.handle("kcreate/session/info", () => requireBridge().sessionInfo());
  // Block 7: Operation journal summary. Returns the running session's
  // per-peer Lamport high-water marks so the renderer can show the
  // PresencePanel "Activity" tab without keeping a parallel JS copy.
  ipcMain.handle("kcreate/session/journalSummary", () =>
    requireBridge().sessionJournalSummary(),
  );
  // Block 8: advisory edit-lock roster.
  ipcMain.handle("kcreate/session/locks", () =>
    requireBridge().sessionLocks(),
  );
  ipcMain.handle(
    "kcreate/session/claimLocks",
    (_e, nodeIdsJson: string) =>
      requireBridge().sessionClaimLocks(nodeIdsJson),
  );
  ipcMain.handle(
    "kcreate/session/releaseLocks",
    (_e, nodeIdsJson: string) => {
      requireBridge().sessionReleaseLocks(nodeIdsJson);
    },
  );
  ipcMain.handle(
    "kcreate/session/sendPresence",
    (
      _e,
      activePage: string | null,
      selectionJson: string,
      cursorJson: string | null,
    ) => {
      requireBridge().sessionSendPresence(
        activePage,
        selectionJson,
        cursorJson,
      );
    },
  );

  // ---------------------------------------------------------------------
  // KChat group authority. The renderer surfaces a "locked"
  // PresencePanel until a future KChat client invokes
  // `kchat.install()` with a valid signed membership attestation.
  // Until then the bridge gate refuses every `session.*` call at the
  // protocol layer — see
  // `kcreate_bridge::collab::session_start/join/sendPresence`.
  // ---------------------------------------------------------------------
  ipcMain.handle("kcreate/kchat/install", (_e, requestJson: string) =>
    requireBridge().kchatInstallAuthority(requestJson),
  );
  ipcMain.handle("kcreate/kchat/clear", () =>
    requireBridge().kchatClearAuthority(),
  );
  ipcMain.handle("kcreate/kchat/status", () =>
    requireBridge().kchatMembershipStatus(),
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
      // Tear down sandboxed JS panel views first so their child
      // processes don't outlive the bridge. They don't own any
      // shared resources, so destruction is safe to do before the
      // workspace cleanup.
      destroyAllJsPanels();
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
      // Tear down any live collab session BEFORE projectClose /
      // rendererShutdown. The order matters:
      //   1. Stop the event-tick timer so we don't race the bridge
      //      drain against the leave path.
      //   2. Call sessionLeave so peers receive a Goodbye on the
      //      live QUIC connection (otherwise they wait ~30 s for the
      //      QUIC idle timeout — see kcreate_collab_transport/host.rs)
      //      and the runtime's mDNS responder is dropped cleanly.
      //   3. Then close the project + shut the renderer down as
      //      before.
      // All three wrapped in try so a bug in the leave path can't
      // block app quit. sessionLeave throws when no session is
      // running, which is the common case (most users quit without
      // a live session), so swallowing the throw is correct.
      stopSessionEventTick();
      if (bridge) {
        try {
          bridge.sessionLeave();
        } catch {
          // No session was running — nothing to leave.
        }
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
