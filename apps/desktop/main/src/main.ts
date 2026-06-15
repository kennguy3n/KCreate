// Electron main process entry. Owns the BrowserWindow and the
// renderer-side IPC handlers that proxy to the Rust kcreate_bridge
// native addon.

import {
  app,
  BrowserWindow,
  dialog,
  ipcMain,
  shell,
  WebContentsView,
  type IpcMainInvokeEvent,
} from "electron";
import * as fs from "node:fs/promises";
import { realpathSync } from "node:fs";
import * as os from "node:os";
import * as path from "node:path";

import { loadBridge, type Bridge } from "./bridge";
import {
  start as startOnboardingDownload,
  validateOpenExternalUrl,
  type OnboardingHandle,
  type OnboardingInstallReport,
} from "./onboardingDownloader";

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

// Phase C — handle to the in-flight one-click recommended-pack
// download, or `null` when no download is running. The
// `kcreate/onboarding/installRecommendedPack` IPC mutates this
// in lock-step with the download's lifetime; the helper below
// gives callers (a fresh install request, or the explicit
// `kcreate/onboarding/cancelInstall` IPC) a single place to
// cleanly stop an active download without duplicating null
// checks at every call site.
let activeOnboardingHandle: OnboardingHandle | null = null;

function cancelOnboardingDownload(): void {
  if (activeOnboardingHandle) {
    activeOnboardingHandle.cancel();
    activeOnboardingHandle = null;
  }
}

// Phase A2 — set of directories the user has explicitly approved
// for sidecar writes (`writeTextFile`) by going through one of the
// native pickers (`chooseExportTarget` / `chooseExportDirectory`).
// Process-scoped so it survives across panel re-mounts but resets
// on app restart; entries are absolute paths after `path.resolve`.
//
// The renderer never writes into this set — it only grows when the
// main process gets back a non-null result from an Electron
// `dialog.show*Dialog` call, which means the user clicked through
// a native picker for that exact path.
const approvedExportDirectories: Set<string> = new Set();

/// Register `absoluteDir` as a session-approved sidecar-write root.
/// Idempotent; safe to call from both the save-file picker (which
/// passes the *parent* dir of the chosen file) and the open-folder
/// picker (which passes the folder itself).
function approveExportDirectory(absoluteDir: string): void {
  approvedExportDirectories.add(path.resolve(absoluteDir));
}

/// Return `true` when `target` (an already-resolved absolute path)
/// is inside either (a) the OS temp directory or (b) any directory
/// the user approved this session. Mirrors the historical
/// `writeTextFile` sandbox semantics — `..` escapes and `/etc/...`-
/// style absolute paths both come back false.
function isWriteableExportPath(target: string): boolean {
  const tmp = path.resolve(os.tmpdir());
  if (isInside(target, tmp)) return true;
  for (const approved of approvedExportDirectories) {
    if (isInside(target, approved)) return true;
  }
  return false;
}

/// Pure helper: `true` iff `target` lives under `root` (or is the
/// root itself), based on `path.relative`. A relative path that
/// starts with `..` or is itself absolute means `target` escaped
/// `root`, so we refuse it.
function isInside(target: string, root: string): boolean {
  const rel = path.relative(root, target);
  return !rel.startsWith("..") && !path.isAbsolute(rel);
}

/// Electron `dialog.showSaveDialog` filter set keyed by the wire-
/// format export-format names (`"png"`, `"svg"`, `"pdf"`, `"webp"`,
/// `"jpeg"`). Matches the `formatExt` table in `ExportPanel.tsx`
/// and the bridge's `kcreate/export/{format}` IPC channels — the
/// filter `extensions` array drives the OS-native picker's file-
/// type dropdown so the user always sees a sensible default.
function exportSaveDialogFilters(
  format: string,
): Array<{ name: string; extensions: string[] }> {
  switch (format) {
    case "png":
      return [
        { name: "PNG image", extensions: ["png"] },
        { name: "All files", extensions: ["*"] },
      ];
    case "svg":
      return [
        { name: "SVG vector", extensions: ["svg"] },
        { name: "All files", extensions: ["*"] },
      ];
    case "pdf":
      return [
        { name: "PDF document", extensions: ["pdf"] },
        { name: "All files", extensions: ["*"] },
      ];
    case "webp":
      return [
        { name: "WebP image", extensions: ["webp"] },
        { name: "All files", extensions: ["*"] },
      ];
    case "jpeg":
      return [
        { name: "JPEG image", extensions: ["jpg", "jpeg"] },
        { name: "All files", extensions: ["*"] },
      ];
    default:
      // Unknown formats still get a non-empty filter list so the
      // dialog opens; the user can pick "All files" to override.
      return [{ name: "All files", extensions: ["*"] }];
  }
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
  ipcMain.handle("kcreate/renderer/renderCurrent", () =>
    requireBridge().rendererRenderCurrent(),
  );
  ipcMain.handle(
    "kcreate/renderer/setViewportAndRender",
    (_e, panX: number, panY: number, zoom: number) =>
      requireBridge().rendererSetViewportAndRender(panX, panY, zoom),
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
  ipcMain.handle("kcreate/project/save", async () => {
    // Phase 11 Block B: projectSave is now async; the worker pool
    // serialises the project off the main thread. ipcMain.handle
    // forwards the Promise to the renderer transparently.
    await requireBridge().projectSave();
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
  // Phase 11 Block D Task 21 — lock-free document-version snapshot
  // used by renderer pollers to skip `getTree` round-trips when the
  // workspace hasn't changed.
  ipcMain.handle("kcreate/document/version", () =>
    requireBridge().documentVersion(),
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
  // FillSection (PropertiesPanel, right panel) calls this on
  // selection change to populate its form with the node's current
  // FillStyle. Writes go back through the existing
  // `kcreate/document/updateNode` channel with the new `fill` field
  // — no separate setter, the existing channel already touches the
  // operation log and triggers a scene re-sync. The JSON string is
  // round-trip-stable with `FillStyle` in `apps/desktop/shared/scene.ts`.
  ipcMain.handle(
    "kcreate/document/nodeFill",
    (_e, nodeId: string) => requireBridge().documentNodeFill(nodeId),
  );
  ipcMain.handle(
    "kcreate/document/nodeExtraFills",
    (_e, nodeId: string) => requireBridge().documentNodeExtraFills(nodeId),
  );
  ipcMain.handle(
    "kcreate/document/nodeExtraStrokes",
    (_e, nodeId: string) => requireBridge().documentNodeExtraStrokes(nodeId),
  );
  ipcMain.handle(
    "kcreate/document/deleteNode",
    (_e, nodeId: string) => {
      requireBridge().documentDeleteNode(nodeId);
    },
  );
  // Phase 6 Tasks 27-28 — layer-colour tag. `color` is either a
  // colour key string (canonicalised by the Rust side) or `null` to
  // clear. Returns the node's post-mutation `version` so renderer
  // listeners can re-key effects without a full tree refresh.
  ipcMain.handle(
    "kcreate/document/setLayerColor",
    (_e, nodeId: string, color: string | null) =>
      requireBridge().documentSetLayerColor(nodeId, color),
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
  ipcMain.handle("kcreate/document/undoGroup", () => {
    const result = requireBridge().documentUndoGroup();
    if (result) broadcastForCommand(result.command);
    return result;
  });
  ipcMain.handle("kcreate/document/redoGroup", () => {
    const result = requireBridge().documentRedoGroup();
    if (result) broadcastForCommand(result.command);
    return result;
  });
  ipcMain.handle("kcreate/document/listDiscardedBranches", () =>
    requireBridge().documentListDiscardedBranches(),
  );
  ipcMain.handle(
    "kcreate/document/restoreDiscardedBranch",
    (_e, indexFromBack: number): boolean =>
      requireBridge().documentRestoreDiscardedBranch(indexFromBack),
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

  // Phase 8 Block E Task 27 — startup-perf profiling. The bridge
  // hands back a pre-serialised JSON string (snake_case fields,
  // mirrors `kcreate_perf::Report`). Renderer parses on the
  // preload boundary so the IPC payload stays a primitive string
  // — same pattern as `resourceLimits` above.
  ipcMain.handle("kcreate/runtime/startupTimeline", (): string =>
    requireBridge().runtimeStartupTimeline(),
  );
  ipcMain.handle(
    "kcreate/runtime/startupMark",
    (_e, label: string): void => {
      requireBridge().runtimeStartupMark(label);
    },
  );
  // Phase 8 Block E Task 28 — tile-cache observability.
  ipcMain.handle("kcreate/runtime/tileCacheStats", (): string =>
    requireBridge().runtimeTileCacheStats(),
  );
  ipcMain.handle("kcreate/runtime/tileCacheClear", (): number =>
    requireBridge().runtimeTileCacheClear(),
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
  // Phase C — the welcome modal calls this on mount to resolve the
  // tier-appropriate Bonsai pack id before it shows install CTAs.
  // Empty string when the registry has no recommendation (expected
  // never on a supported device).
  ipcMain.handle("kcreate/llm/recommendedPack", (): string =>
    requireBridge().llmRecommendedPack(),
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
  // ----- Phase 4: vision sidecar -----
  ipcMain.handle("kcreate/vision/start", (_e, packId: string): number =>
    requireBridge().visionStart(packId),
  );
  ipcMain.handle("kcreate/vision/stop", (): void => {
    requireBridge().visionStop();
  });
  ipcMain.handle("kcreate/vision/status", (): string =>
    requireBridge().visionStatus(),
  );
  // Phase 4 vision inference handlers — Rust returns a JS Promise via
  // N-API `AsyncTask`, so each handler just returns the promise and
  // `ipcMain.handle` awaits it before serializing the reply to the
  // renderer. The main process event loop stays responsive during
  // the underlying VLM round-trip.
  ipcMain.handle(
    "kcreate/vision/describeImage",
    (
      _e,
      rgba: Buffer,
      width: number,
      height: number,
      userPrompt: string,
    ): Promise<string> =>
      requireBridge().visionDescribeImage(
        rgba,
        width,
        height,
        userPrompt,
      ),
  );
  ipcMain.handle(
    "kcreate/vision/describeNode",
    (_e, nodeId: string, userPrompt: string): Promise<string> =>
      requireBridge().visionDescribeNode(nodeId, userPrompt),
  );
  ipcMain.handle(
    "kcreate/vision/generateAltText",
    (_e, rgba: Buffer, width: number, height: number): Promise<string> =>
      requireBridge().visionGenerateAltText(rgba, width, height),
  );
  ipcMain.handle(
    "kcreate/vision/generateAltTextForNode",
    (_e, nodeId: string): Promise<string> =>
      requireBridge().visionGenerateAltTextForNode(nodeId),
  );
  ipcMain.handle(
    "kcreate/vision/analyzeDesign",
    (_e, rgba: Buffer, width: number, height: number): Promise<string> =>
      requireBridge().visionAnalyzeDesign(rgba, width, height),
  );
  ipcMain.handle(
    "kcreate/ai/extractBrandFromImage",
    (_e, rgba: Buffer, width: number, height: number): Promise<string> =>
      requireBridge().aiExtractBrandFromImage(rgba, width, height),
  );
  ipcMain.handle(
    "kcreate/ai/suggestCrop",
    (
      _e,
      rgba: Buffer,
      width: number,
      height: number,
      aspectRatio: number,
    ): Promise<string> =>
      requireBridge().aiSuggestCrop(
        rgba,
        width,
        height,
        aspectRatio,
      ),
  );
  ipcMain.handle(
    "kcreate/ai/suggestDesignTokens",
    (_e, rgba: Buffer, width: number, height: number): Promise<string> =>
      requireBridge().aiSuggestDesignTokens(rgba, width, height),
  );
  ipcMain.handle(
    "kcreate/ai/describeStyle",
    (_e, rgba: Buffer, width: number, height: number): Promise<string> =>
      requireBridge().aiDescribeStyle(rgba, width, height),
  );
  ipcMain.handle("kcreate/vision/recommendedPack", (): string =>
    requireBridge().visionRecommendedPack(),
  );
  ipcMain.handle("kcreate/vision/mmprojFor", (_e, packId: string): string =>
    requireBridge().visionMmprojFor(packId),
  );
  ipcMain.handle("kcreate/vision/listablePacks", (): string[] =>
    requireBridge().visionListablePacks(),
  );
  // ----- Phase 4: image generation sidecar -----
  ipcMain.handle("kcreate/imageGen/start", (_e, packId: string): number =>
    requireBridge().imageGenStart(packId),
  );
  ipcMain.handle("kcreate/imageGen/stop", (): void => {
    requireBridge().imageGenStop();
  });
  ipcMain.handle("kcreate/imageGen/status", (): string =>
    requireBridge().imageGenStatus(),
  );
  ipcMain.handle(
    "kcreate/imageGen/generate",
    (
      _e,
      prompt: string,
      width: number,
      height: number,
      steps: number,
      seed: number | null,
    ): Promise<string> =>
      requireBridge().imageGenGenerate(prompt, width, height, steps, seed),
  );
  ipcMain.handle("kcreate/imageGen/allowed", (): boolean =>
    requireBridge().imageGenAllowed(),
  );
  ipcMain.handle("kcreate/imageGen/recommendedPack", (): string =>
    requireBridge().imageGenRecommendedPack(),
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
  // and other renderer-side sidecars. The path must land inside
  // either (a) the OS temp directory (`os.tmpdir()`) — the
  // historical sandbox root — or (b) a directory the user has
  // explicitly approved this session via `chooseExportTarget` /
  // `chooseExportDirectory`. Anything else (a stray absolute path
  // or a `..` escape) is rejected. The session allowlist starts
  // empty and only grows when a user OS dialog returns a directory,
  // so the renderer can never grant itself write access — every
  // approved path is one the user typed into a native picker.
  ipcMain.handle(
    "kcreate/runtime/writeTextFile",
    async (_e, target: string, content: string): Promise<number> => {
      const resolved = path.resolve(target);
      if (!isWriteableExportPath(resolved)) {
        throw new Error(
          `writeTextFile rejected: ${target} is outside ${os.tmpdir()} and any user-approved export directory`,
        );
      }
      await fs.mkdir(path.dirname(resolved), { recursive: true });
      const bytes = Buffer.byteLength(content, "utf8");
      await fs.writeFile(resolved, content, "utf8");
      return bytes;
    },
  );

  // Phase A2 — native save-as dialog.
  //
  // `chooseExportTarget` wraps `dialog.showSaveDialog` with
  // per-format extension filters so the renderer can land exports
  // at a user-chosen absolute path instead of the OS temp dir.
  // Returns the absolute chosen path on success, `null` on cancel.
  // The dialog opens in `defaultDir` (when provided) so consecutive
  // exports for the same format stay rooted at the user's last
  // location — the renderer persists that hint in
  // `Preferences.export.lastDirByFormat`.
  ipcMain.handle(
    "kcreate/runtime/chooseExportTarget",
    async (
      _e,
      format: string,
      defaultName: string,
      defaultDir: string | null,
    ): Promise<string | null> => {
      const win = mainWindow;
      if (!win) return null;
      const filters = exportSaveDialogFilters(format);
      const initial = defaultDir
        ? path.join(defaultDir, defaultName)
        : defaultName;
      const result = await dialog.showSaveDialog(win, {
        title: `Export ${format.toUpperCase()}`,
        defaultPath: initial,
        filters,
        properties: ["showOverwriteConfirmation", "createDirectory"],
      });
      if (result.canceled || !result.filePath) return null;
      const chosen = path.resolve(result.filePath);
      // Approve the parent directory so later sidecar writes
      // (e.g. dev-handoff `tokens.json`) can land next to the
      // primary export through `writeTextFile`.
      approveExportDirectory(path.dirname(chosen));
      return chosen;
    },
  );

  // Sibling to `chooseExportTarget` for batch presets that emit
  // multiple files into a shared directory. Wraps `showOpenDialog`
  // with `openDirectory` + `createDirectory` so the user can drop
  // the run into a new folder. Returns the absolute chosen
  // directory or `null` on cancel.
  ipcMain.handle(
    "kcreate/runtime/chooseExportDirectory",
    async (_e, defaultDir: string | null): Promise<string | null> => {
      const win = mainWindow;
      if (!win) return null;
      const result = await dialog.showOpenDialog(win, {
        title: "Choose export directory",
        defaultPath: defaultDir ?? undefined,
        properties: ["openDirectory", "createDirectory"],
      });
      if (result.canceled || result.filePaths.length === 0) return null;
      const first = result.filePaths[0];
      if (!first) return null;
      const chosen = path.resolve(first);
      approveExportDirectory(chosen);
      return chosen;
    },
  );

  ipcMain.handle(
    "kcreate/export/svg",
    (_e, nodeIds: string[], optionsJson: string) => {
      // Pick async vs sync based on the **export selection** size,
      // not the total document size. Worker dispatch is ~200 µs;
      // serialising the SVG for a 5-node selection is faster than
      // that, so the sync path stays for small selections and we
      // switch to the async worker past 100 selected nodes (where
      // the round-trip pays for itself many times over). This is a
      // heuristic, not a guarantee — a single-artboard export from
      // a huge document still hits the sync path because the
      // subtree we're serialising is bounded by `nodeIds`, not the
      // whole document. The bridge SVG serialiser walks only the
      // requested nodes' subtrees, so the heuristic tracks the
      // actual cost.
      //
      // Phase 11 Block B follow-up — Devin Review ANALYSIS-0006.
      if (nodeIds.length > 100) {
        return requireBridge().exportSvgAsync(nodeIds, optionsJson);
      }
      return requireBridge().exportSvg(nodeIds, optionsJson);
    },
  );
  ipcMain.handle(
    "kcreate/export/png",
    (_e, outputPath: string, optionsJson: string) =>
      // Phase 11 Block B: async; returns Promise<number>.
      requireBridge().exportPng(outputPath, optionsJson),
  );
  ipcMain.handle(
    "kcreate/export/pdf",
    (_e, outputPath: string, optionsJson: string) =>
      // Phase 11 Block B: async; returns Promise<number>.
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
    "kcreate/document/importImageBytes",
    (_e, parentId: string | null, bytes: Buffer) =>
      requireBridge().documentImportImageBytes(parentId, bytes),
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
    "kcreate/canvas/createPath",
    (
      _e,
      parentId: string | null,
      segmentsJson: string,
      closed: boolean,
      name: string | null,
    ) =>
      requireBridge().canvasCreatePath(parentId, segmentsJson, closed, name),
  );
  ipcMain.handle(
    "kcreate/canvas/pathBoolean",
    (_e, op: string, sourceIds: string[]) =>
      requireBridge().canvasPathBoolean(op, sourceIds),
  );
  // Phase B3 — Node editor read/write surface. `pathGetSegments`
  // returns a JSON-encoded `PathSnapshot` (see
  // `apps/desktop/shared/scene.ts::PathSnapshot`); preload
  // re-parses it before handing to the renderer so the channel
  // payload stays a single string and matches the
  // `createPath` / `pathBoolean` discipline of passing path
  // geometry across the boundary as JSON.
  ipcMain.handle(
    "kcreate/canvas/pathGetSegments",
    (_e, nodeId: string) => requireBridge().canvasPathGetSegments(nodeId),
  );
  ipcMain.handle(
    "kcreate/canvas/pathSetSegments",
    (
      _e,
      nodeId: string,
      segmentsJson: string,
      closed: boolean,
    ) =>
      requireBridge().canvasPathSetSegments(nodeId, segmentsJson, closed),
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
  ipcMain.handle(
    "kcreate/canvas/createNodes",
    (_e, itemsJson: string): string =>
      requireBridge().canvasCreateNodes(itemsJson),
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
  ipcMain.handle(
    "kcreate/artboard/magic-resize",
    (_e, sourceArtboardId: string, targetsJson: string): string =>
      requireBridge().magicResize(sourceArtboardId, targetsJson),
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
    "kcreate/component/smartAnimateSnapshot",
    (_e, nodeId: string, targetVariantId: string): string =>
      requireBridge().componentSmartAnimateSnapshot(nodeId, targetVariantId),
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

  // Local template marketplace (Phase 3 — Tasks 11-12). The
  // renderer's TemplateMarketplace panel calls list/install/remove
  // here; the bridge persists installs by copying the .ktemplate
  // folder into ~/.kcreate/templates/ (configurable via the
  // KCREATE_TEMPLATE_DIR env var the bridge reads). `category` and
  // `query` are nullable on the IPC side so the renderer can omit
  // them; we normalise null → undefined for the napi signature.
  ipcMain.handle(
    "kcreate/template/list",
    (_e, category: string | null, query: string | null): string =>
      requireBridge().templateList(
        category ?? undefined,
        query ?? undefined,
      ),
  );
  ipcMain.handle(
    "kcreate/template/installLocal",
    (_e, sourcePath: string): string =>
      requireBridge().templateInstallLocal(sourcePath),
  );
  ipcMain.handle(
    "kcreate/template/remove",
    (_e, templateId: string): void => {
      requireBridge().templateRemove(templateId);
    },
  );
  // G2 — "Start from template": pour the template's content.json into
  // the open workspace as a fresh artboard. Returns the new artboard id
  // + node ids so the renderer can select/frame the design.
  ipcMain.handle(
    "kcreate/template/instantiate",
    (_e, templateId: string) =>
      requireBridge().templateInstantiate(templateId),
  );
  // G2 — gallery card preview: render (or read cached) thumbnail PNG
  // for a template id via the shared export pipeline.
  ipcMain.handle(
    "kcreate/template/thumbnail",
    (_e, templateId: string) =>
      requireBridge().templateThumbnail(templateId),
  );
  // Phase 6 — Audit log (Tasks 13–14)
  ipcMain.handle(
    "kcreate/audit/record",
    (_e, eventJson: string): string =>
      requireBridge().auditRecord(eventJson),
  );
  ipcMain.handle(
    "kcreate/audit/query",
    (_e, queryJson: string): string =>
      requireBridge().auditQuery(queryJson),
  );
  ipcMain.handle(
    "kcreate/audit/count",
    (): number => requireBridge().auditCount(),
  );
  ipcMain.handle(
    "kcreate/audit/purge",
    (_e, cutoffIso: string): number =>
      requireBridge().auditPurge(cutoffIso),
  );
  ipcMain.handle(
    "kcreate/audit/path",
    (): string => requireBridge().auditPath(),
  );

  // Phase 6 — Tasks 17-18: lazy thumbnail cache + recent-projects.
  // All five handlers go through `requireBridge()` (which throws on
  // the no-bridge path) so the renderer always sees a meaningful
  // error rather than a `TypeError: undefined.thumbnailForCover`.
  ipcMain.handle(
    "kcreate/thumbnail/forCover",
    (_e, maxDimPx: number) => requireBridge().thumbnailForCover(maxDimPx),
  );
  ipcMain.handle(
    "kcreate/thumbnail/forPage",
    (_e, pageId: string, maxDimPx: number) =>
      requireBridge().thumbnailForPage(pageId, maxDimPx),
  );
  ipcMain.handle(
    "kcreate/thumbnail/prepareBackground",
    (_e, maxDimPx: number): void => {
      requireBridge().thumbnailPrepareBackground(maxDimPx);
    },
  );
  ipcMain.handle(
    "kcreate/recent/list",
    () => requireBridge().recentProjectsList(),
  );
  ipcMain.handle(
    "kcreate/recent/coverBytes",
    (_e, projectDir: string) =>
      requireBridge().recentProjectCoverBytes(projectDir),
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
  // -------------------------------------------------------------------
  // Phase 6 Tasks 25-26 — node clipboard. Renderer marshals the OS
  // clipboard text; this IPC just serialises selected node ids into a
  // portable JSON payload (copy) and instantiates fresh nodes from a
  // payload (paste).
  // -------------------------------------------------------------------
  ipcMain.handle(
    "kcreate/clipboard/copy",
    (_e, nodeIds: string[]): string =>
      requireBridge().documentClipboardCopy(nodeIds),
  );
  ipcMain.handle(
    "kcreate/clipboard/paste",
    (
      _e,
      payload: string,
      targetParentId: string | null | undefined,
      offsetX: number,
      offsetY: number,
    ): string[] =>
      requireBridge().documentClipboardPaste(
        payload,
        targetParentId ?? undefined,
        offsetX,
        offsetY,
      ),
  );

  // -------------------------------------------------------------------
  // Phase 8 — design-token binding, constraint-aware frame resize,
  // text auto-fit, page-numbering tokens, section pages, job presets,
  // brand-kit versioning. The renderer parses JSON-returning calls
  // (presets, version info, diffs) itself; the main process just
  // forwards strings.
  // -------------------------------------------------------------------
  ipcMain.handle(
    "kcreate/phase8/bind-token",
    (_e, nodeId: string, property: string, tokenName: string): void =>
      requireBridge().documentBindToken(nodeId, property, tokenName),
  );
  ipcMain.handle(
    "kcreate/phase8/unbind-token",
    (_e, nodeId: string, property: string): void =>
      requireBridge().documentUnbindToken(nodeId, property),
  );
  ipcMain.handle(
    "kcreate/phase8/propagate-token",
    (_e, tokenName: string): number =>
      requireBridge().documentPropagateToken(tokenName),
  );
  ipcMain.handle(
    "kcreate/phase8/node-token-bindings",
    (_e, nodeId: string): string =>
      requireBridge().documentNodeTokenBindings(nodeId),
  );
  ipcMain.handle(
    "kcreate/phase8/node-constraints",
    (_e, nodeId: string): string =>
      requireBridge().documentNodeConstraints(nodeId),
  );
  ipcMain.handle(
    "kcreate/phase8/set-node-constraints",
    (
      _e,
      nodeId: string,
      constraints: unknown,
    ): void =>
      requireBridge().documentSetNodeConstraints(
        nodeId,
        JSON.stringify(constraints),
      ),
  );
  // -------------------------------------------------------------------
  // Phase 8 Task 26 — project encryption.
  // -------------------------------------------------------------------
  ipcMain.handle(
    "kcreate/project/encryption/status",
    (): string => requireBridge().projectEncryptionStatus(),
  );
  ipcMain.handle(
    "kcreate/project/encryption/passphrase-strength",
    (_e, passphrase: string): number =>
      requireBridge().projectPassphraseStrength(passphrase),
  );
  ipcMain.handle(
    "kcreate/project/encryption/enable",
    (_e, passphrase: string): string =>
      requireBridge().projectEnableEncryption(passphrase),
  );
  ipcMain.handle(
    "kcreate/project/encryption/change-passphrase",
    (_e, oldPassphrase: string, newPassphrase: string): void =>
      requireBridge().projectChangePassphrase(oldPassphrase, newPassphrase),
  );
  ipcMain.handle(
    "kcreate/project/encryption/export-plaintext-recovery",
    (_e, passphrase: string, outputPath: string): string =>
      requireBridge().projectExportPlaintextRecovery(passphrase, outputPath),
  );
  // Native save dialog scoped to the plaintext recovery export. We
  // keep the picker in the main process (instead of an inline
  // `<input type="text">` in the renderer) so (a) the renderer
  // never sees the user's filesystem, (b) the OS handles overwrite-
  // protection / sandbox prompts / iCloud routing, and (c) the
  // default filename + extension are pinned in a single place
  // (mirrors the pattern used by `kcreate/pdf/pickFile` /
  // `kcreate/sketch/pickFile`). Returns the absolute chosen path,
  // or `null` if the user cancelled — the panel uses `null` to
  // short-circuit without showing an error.
  ipcMain.handle("kcreate/project/encryption/pick-recovery-path", async () => {
    const win = mainWindow;
    if (!win) return null;
    const result = await dialog.showSaveDialog(win, {
      title: "Export plaintext recovery copy",
      defaultPath: "recovery.sqlite",
      filters: [
        { name: "SQLite database", extensions: ["sqlite", "db"] },
        { name: "All files", extensions: ["*"] },
      ],
      properties: ["showOverwriteConfirmation", "createDirectory"],
    });
    if (result.canceled || !result.filePath) return null;
    return result.filePath;
  });
  ipcMain.handle(
    "kcreate/phase8/resize-frame",
    (
      _e,
      frameId: string,
      bounds: { x: number; y: number; width: number; height: number },
    ): void =>
      requireBridge().documentResizeFrame(frameId, JSON.stringify(bounds)),
  );
  ipcMain.handle(
    "kcreate/phase8/set-auto-fit",
    (_e, nodeId: string, enabled: boolean): boolean =>
      requireBridge().textSetAutoFit(nodeId, enabled),
  );
  ipcMain.handle(
    "kcreate/phase8/page-number-token",
    (_e, format: string): string => requireBridge().pageNumberToken(format),
  );
  ipcMain.handle(
    "kcreate/phase8/set-page-section",
    (
      _e,
      pageId: string,
      startNumber: number | null,
      prefix: string | null,
    ): void =>
      requireBridge().pageSetSection(
        pageId,
        startNumber ?? null,
        prefix ?? null,
      ),
  );
  ipcMain.handle(
    "kcreate/phase8/resolve-page-contexts",
    (): string => requireBridge().pageResolveContexts(),
  );
  ipcMain.handle(
    "kcreate/phase8/export-job-presets",
    (_e, job: string): string => requireBridge().exportJobPresets(job),
  );
  ipcMain.handle(
    "kcreate/phase8/brand-kit/save-version",
    (_e, brandKitId: string, description: string): string =>
      requireBridge().brandKitSaveVersion(brandKitId, description),
  );
  ipcMain.handle(
    "kcreate/phase8/brand-kit/list-versions",
    (_e, brandKitId: string): string =>
      requireBridge().brandKitListVersions(brandKitId),
  );
  ipcMain.handle(
    "kcreate/phase8/brand-kit/restore-version",
    (_e, versionId: string): string =>
      requireBridge().brandKitRestoreVersion(versionId),
  );
  ipcMain.handle(
    "kcreate/phase8/brand-kit/diff",
    (_e, beforeId: string, afterId: string): string =>
      requireBridge().brandKitDiff(beforeId, afterId),
  );

  // ---------------------------------------------------------------------
  // Phase 8 (Task 4) — design-review annotation CRUD. Each handler is a
  // thin pass-through; the actual storage write + collab broadcast is
  // performed in `crates/kcreate_bridge/src/annotation_bridge.rs`.
  // ---------------------------------------------------------------------
  ipcMain.handle(
    "kcreate/annotation/create",
    (_e, requestJson: string): string =>
      requireBridge().annotationCreate(requestJson),
  );
  ipcMain.handle(
    "kcreate/annotation/reply",
    (_e, requestJson: string): string =>
      requireBridge().annotationReply(requestJson),
  );
  ipcMain.handle(
    "kcreate/annotation/list",
    (_e, requestJson: string): string =>
      requireBridge().annotationList(requestJson),
  );
  ipcMain.handle(
    "kcreate/annotation/resolve",
    (_e, requestJson: string): boolean =>
      requireBridge().annotationResolve(requestJson),
  );
  ipcMain.handle(
    "kcreate/annotation/delete",
    (_e, id: string): boolean => requireBridge().annotationDelete(id),
  );

  // ---------------------------------------------------------------------
  // Phase 9 — guides, grid, alignment, AI palette/autofit/trace/iconify/
  // batch-alt-text, PSD/Penpot/EXIF import, SVG preview, history panel,
  // export validation, brief→project, memory watchdog, autosave.
  // ---------------------------------------------------------------------
  ipcMain.handle(
    "kcreate/phase9/guide/create",
    (
      _e,
      pageId: string,
      orientation: string,
      position: number,
      color: string | null,
      locked: boolean,
    ): string =>
      requireBridge().guideCreate(pageId, orientation, position, color, locked),
  );
  ipcMain.handle(
    "kcreate/phase9/guide/delete",
    (_e, id: string): boolean => requireBridge().guideDelete(id),
  );
  ipcMain.handle(
    "kcreate/phase9/guide/clear-page",
    (_e, pageId: string): number => requireBridge().guideClearPage(pageId),
  );
  ipcMain.handle(
    "kcreate/phase9/guide/list",
    (_e, pageId: string): string => requireBridge().guideList(pageId),
  );
  ipcMain.handle(
    "kcreate/phase9/guide/list-all",
    (): string => requireBridge().guideListAll(),
  );

  ipcMain.handle(
    "kcreate/phase9/grid/get",
    (_e, artboardId: string): string =>
      requireBridge().artboardGridSettings(artboardId),
  );
  ipcMain.handle(
    "kcreate/phase9/grid/set",
    (
      _e,
      artboardId: string,
      enabled: boolean,
      spacing: number,
      subdivisions: number,
      color: string | null,
    ): string =>
      requireBridge().artboardSetGrid(
        artboardId,
        enabled,
        spacing,
        subdivisions,
        color,
      ),
  );

  ipcMain.handle(
    "kcreate/phase9/document/align",
    (_e, nodeIds: string[], alignment: string): string =>
      requireBridge().documentAlign(JSON.stringify(nodeIds), alignment),
  );
  ipcMain.handle(
    "kcreate/phase9/document/distribute",
    (_e, nodeIds: string[], axis: string): string =>
      requireBridge().documentDistribute(JSON.stringify(nodeIds), axis),
  );

  ipcMain.handle(
    "kcreate/phase9/palette/apply-brand-kit",
    (_e, nodeId: string, numColors: number, brandKitName: string): string =>
      requireBridge().paletteExtractAndApplyBrandKit(
        nodeId,
        numColors,
        brandKitName,
      ),
  );

  ipcMain.handle(
    "kcreate/phase9/text/autofit-recompute",
    (_e, nodeId: string): string =>
      requireBridge().textAutofitRecompute(nodeId),
  );

  ipcMain.handle(
    "kcreate/phase9/ai/trace-raster",
    (
      _e,
      nodeId: string,
      threshold: number,
      simplifyTolerance: number,
    ): string =>
      requireBridge().aiTraceRaster(nodeId, threshold, simplifyTolerance),
  );
  ipcMain.handle(
    "kcreate/phase9/ai/iconify",
    (_e, nodeId: string, gridSize: number): string =>
      requireBridge().aiIconify(nodeId, gridSize),
  );
  ipcMain.handle(
    "kcreate/phase9/ai/batch-alt-text",
    (_e, pageId: string): string => requireBridge().aiBatchAltText(pageId),
  );

  ipcMain.handle(
    "kcreate/phase9/import/psd",
    (_e, path: string): string => requireBridge().importPsd(path),
  );
  ipcMain.handle(
    "kcreate/phase9/import/penpot",
    (_e, path: string): string => requireBridge().importPenpot(path),
  );
  ipcMain.handle(
    "kcreate/phase9/image/read-exif",
    (_e, bytes: Uint8Array): string => requireBridge().imageReadExif(bytes),
  );

  ipcMain.handle(
    "kcreate/phase9/export/svg-preview",
    (
      _e,
      svgBytes: Uint8Array,
      maxWidth: number,
      maxHeight: number,
      transparent: boolean,
    ): string =>
      requireBridge().exportSvgPreview(
        svgBytes,
        maxWidth,
        maxHeight,
        transparent,
      ),
  );

  ipcMain.handle(
    "kcreate/phase9/operation-log/filter",
    (_e, filterJson: string): string =>
      requireBridge().operationLogFilter(filterJson),
  );
  ipcMain.handle(
    "kcreate/phase9/export/validate",
    (_e, requestJson: string): string =>
      requireBridge().exportValidate(requestJson),
  );
  ipcMain.handle(
    "kcreate/phase9/brief/to-project",
    (_e, planJson: string): string => requireBridge().briefToProject(planJson),
  );

  ipcMain.handle(
    "kcreate/phase9/memory/watchdog-start",
    (_e, pollIntervalMs: number): boolean =>
      requireBridge().memoryWatchdogStart(pollIntervalMs),
  );
  ipcMain.handle(
    "kcreate/phase9/memory/watchdog-stop",
    (): boolean => requireBridge().memoryWatchdogStop(),
  );
  ipcMain.handle(
    "kcreate/phase9/memory/drain-events",
    (): string => requireBridge().drainMemoryEvents(),
  );
  ipcMain.handle(
    "kcreate/phase9/runtime/gpu-backend-name",
    (): string => requireBridge().runtimeGpuBackendName(),
  );

  ipcMain.handle(
    "kcreate/phase9/autosave/start",
    (): boolean => requireBridge().autosaveStart(),
  );
  ipcMain.handle(
    "kcreate/phase9/autosave/stop",
    (): boolean => requireBridge().autosaveStop(),
  );
  ipcMain.handle(
    "kcreate/phase9/autosave/force-now",
    (): boolean => requireBridge().autosaveForceNow(),
  );
  ipcMain.handle(
    "kcreate/phase9/autosave/status",
    (): string => requireBridge().autosaveStatus(),
  );
  ipcMain.handle(
    "kcreate/phase9/autosave/recovery-available",
    (): string => requireBridge().autosaveRecoveryAvailable(),
  );
  ipcMain.handle(
    "kcreate/phase9/autosave/recover",
    (): void => requireBridge().autosaveRecover(),
  );
  ipcMain.handle(
    "kcreate/phase9/autosave/dismiss-recovery",
    (): void => requireBridge().autosaveDismissRecovery(),
  );

  // ---------------------------------------------------------------------
  // Phase 10 — Image Studio AI, Vector/Layout AI, Export AI + Live
  // Preview, Brand Hub + Plugin Marketplace, Preferences. See
  // `crates/kcreate_bridge/src/phase10.rs` and `apps/desktop/preload`.
  // ---------------------------------------------------------------------
  // Block A — Image Studio AI
  ipcMain.handle(
    "kcreate/phase10/ai/denoise",
    (
      _e,
      nodeId: string,
      strength: number,
      searchRadius: number,
      patchRadius: number,
    ): string =>
      requireBridge().aiDenoise(nodeId, strength, searchRadius, patchRadius),
  );
  ipcMain.handle(
    "kcreate/phase10/ai/inpaint",
    (
      _e,
      nodeId: string,
      maskJson: string,
      patchRadius: number | null,
      numIterations: number | null,
      pyramidLevels: number | null,
    ): string =>
      requireBridge().aiInpaint(
        nodeId,
        maskJson,
        patchRadius,
        numIterations,
        pyramidLevels,
      ),
  );
  ipcMain.handle(
    "kcreate/phase10/ai/auto-color",
    (_e, nodeId: string, mode: string): string =>
      requireBridge().aiAutoColor(nodeId, mode),
  );
  ipcMain.handle(
    "kcreate/phase10/ai/segment-at-point",
    (
      _e,
      nodeId: string,
      pointX: number,
      pointY: number,
      isPositive: boolean,
    ): string =>
      requireBridge().aiSegmentAtPoint(nodeId, pointX, pointY, isPositive),
  );
  ipcMain.handle(
    "kcreate/phase10/ai/smart-select-at-point",
    (
      _e,
      nodeId: string,
      x: number,
      y: number,
      tolerance: number,
      mode: string,
      previousMaskBase64: string | null,
    ): string =>
      requireBridge().aiSmartSelectAtPoint(
        nodeId,
        x,
        y,
        tolerance,
        mode,
        previousMaskBase64,
      ),
  );

  // Block B — Vector/Layout AI
  ipcMain.handle(
    "kcreate/phase10/ai/match-stroke",
    (_e, sourceId: string, targetIds: string[]): string =>
      requireBridge().aiMatchStroke(sourceId, JSON.stringify(targetIds)),
  );
  ipcMain.handle(
    "kcreate/phase10/ai/extract-glyph",
    (
      _e,
      nodeId: string,
      cropX: number,
      cropY: number,
      cropWidth: number,
      cropHeight: number,
      emSize: number,
    ): string =>
      requireBridge().aiExtractGlyph(
        nodeId,
        cropX,
        cropY,
        cropWidth,
        cropHeight,
        emSize,
      ),
  );
  ipcMain.handle(
    "kcreate/phase10/ai/reformat-to-deck",
    (_e, pageId: string): string => requireBridge().aiReformatToDeck(pageId),
  );
  ipcMain.handle(
    "kcreate/phase10/ai/brief-to-one-pager",
    (_e, brief: string, pageSize: string | null): string =>
      requireBridge().aiBriefToOnePager(brief, pageSize),
  );
  ipcMain.handle(
    "kcreate/phase10/ai/generate-themed-design",
    (_e, brief: string, optionsJson: string): string =>
      requireBridge().aiGenerateThemedDesign(brief, optionsJson),
  );
  ipcMain.handle(
    "kcreate/phase10/ai/harmonize-palette",
    (_e, brandKitId: string, harmonyType: string): string =>
      requireBridge().aiHarmonizePalette(brandKitId, harmonyType),
  );
  ipcMain.handle(
    "kcreate/phase10/ai/suggest-type-pairing",
    (_e, headingFontName: string): string =>
      requireBridge().aiSuggestTypePairing(headingFontName),
  );

  // Block C — Export AI + Live Preview
  ipcMain.handle(
    "kcreate/phase10/export/optimize-svg",
    (_e, svg: string): string => requireBridge().exportOptimizeSvg(svg),
  );
  ipcMain.handle(
    "kcreate/phase10/export/smart-compress",
    (
      _e,
      nodeId: string,
      format: string,
      targetSsim: number | null,
    ): string =>
      requireBridge().exportSmartCompress(nodeId, format, targetSsim),
  );
  ipcMain.handle(
    "kcreate/phase10/export/preview",
    (_e, requestJson: string): string =>
      requireBridge().exportPreview(requestJson),
  );
  ipcMain.handle(
    "kcreate/phase10/import/ai",
    (_e, path: string): string => requireBridge().importAi(path),
  );

  // Block D — Brand Hub + Plugin Marketplace
  ipcMain.handle(
    "kcreate/phase10/ai/brand-to-brochure",
    (_e, brandKitId: string, numPages: number): string =>
      requireBridge().aiBrandToBrochure(brandKitId, numPages),
  );
  ipcMain.handle(
    "kcreate/phase10/plugin-marketplace/list",
    (): string => requireBridge().pluginMarketplaceList(),
  );
  ipcMain.handle(
    "kcreate/phase10/plugin-marketplace/install-local",
    (_e, path: string): string =>
      requireBridge().pluginMarketplaceInstallLocal(path),
  );
  ipcMain.handle(
    "kcreate/phase10/plugin-marketplace/remove",
    (_e, id: string): boolean => requireBridge().pluginMarketplaceRemove(id),
  );
  ipcMain.handle(
    "kcreate/phase10/export/pdf-multi",
    (_e, optionsJson: string, outputPath: string): string =>
      requireBridge().exportPdfMulti(optionsJson, outputPath),
  );

  // Block D Task 23 — Preferences
  ipcMain.handle(
    "kcreate/phase10/preferences/load",
    (): string => requireBridge().preferencesLoad(),
  );
  ipcMain.handle(
    "kcreate/phase10/preferences/save",
    (_e, prefsJson: string): void => requireBridge().preferencesSave(prefsJson),
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
  ipcMain.handle(
    "kcreate/ai/upscaleWithBackend",
    (
      _e,
      nodeId: string,
      scale: number,
      backend: string,
      modelPath: string,
    ) =>
      requireBridge().aiUpscaleWithBackend(nodeId, scale, backend, modelPath),
  );
  ipcMain.handle(
    "kcreate/ai/segment",
    (
      _e,
      nodeId: string,
      pointX: number,
      pointY: number,
      tolerance: number,
      edgeThreshold: number,
      backend: string,
      modelPath: string,
    ) =>
      requireBridge().aiSegment(
        nodeId,
        pointX,
        pointY,
        tolerance,
        edgeThreshold,
        backend,
        modelPath,
      ),
  );
  ipcMain.handle(
    "kcreate/ai/detectTextRegions",
    (_e, nodeId: string, optionsJson: string) =>
      requireBridge().aiDetectTextRegions(nodeId, optionsJson),
  );
  ipcMain.handle(
    "kcreate/ai/insertTextLayerForRegion",
    (_e, requestJson: string) =>
      requireBridge().aiInsertTextLayerForRegion(requestJson),
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
  // Phase C — one-click recommended-pack download.
  //
  // The renderer triggers this with no arguments; the main process
  // resolves the recommended pack id via the bridge, validates the
  // download URL from the static registry against an allow-list
  // (`onboardingDownloader.ALLOWED_HOSTS`), streams the bytes
  // into a per-process temp file under `os.tmpdir()`, then hands
  // the temp path to `aiInstallModelPack` (same SHA-256 verify +
  // atomic rename the manual "I have the file" flow uses). The
  // renderer never sees the URL — the registry is the only place
  // a URL flows from, and the allow-list prevents a typo or a
  // maliciously-edited registry entry from fetching arbitrary
  // hosts.
  //
  // Concurrency: a single in-flight handle is tracked at module
  // scope (`activeOnboardingHandle`). A second `start` invocation
  // while one is running cleanly cancels the prior run (matches
  // the renderer's "close modal mid-download" UX) before kicking
  // off the new one.
  ipcMain.handle("kcreate/onboarding/installRecommendedPack", async () => {
    cancelOnboardingDownload();
    const win = mainWindow;
    const handle = startOnboardingDownload(requireBridge(), win);
    activeOnboardingHandle = handle;
    try {
      const report: OnboardingInstallReport = await handle.done;
      return JSON.stringify(report);
    } finally {
      if (activeOnboardingHandle === handle) {
        activeOnboardingHandle = null;
      }
    }
  });
  ipcMain.handle("kcreate/onboarding/cancelInstall", () => {
    cancelOnboardingDownload();
  });
  // `kcreate/system/openExternal` exposes Electron's `shell.openExternal`
  // through a narrow channel that mirrors `onboardingDownloader.validateUrl`'s
  // allow-list. The welcome modal's "Open download page" fallback uses
  // this to launch the user's default browser at the Hugging Face
  // model card when they prefer the manual install flow. Validating
  // the URL here (not just in the renderer) prevents a
  // compromised renderer process from coaxing the main process into
  // opening file://, mailto:, or arbitrary http: URLs.
  ipcMain.handle("kcreate/system/openExternal", async (_e, url: string) => {
    const validated = validateOpenExternalUrl(url);
    await shell.openExternal(validated);
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
  // `kcreate/figma/pickFile` mirrors the PDF picker exactly: keep
  // the OS dialog in the main process so the renderer never sees
  // the filesystem. The Rust side (`crates/kcreate_export/src/
  // figma_import.rs`) only understands UTF-8 JSON, so the filter is
  // scoped to `.json`. Figma exports come in two shapes (the REST
  // API dump `.json` and the official `figma export-json` plugin's
  // `.fig.json`) — both are matched by the single `.json`
  // extension entry. The legacy `.fig` binary container is
  // *not* a JSON export and would deterministically fail
  // `serde_json::from_slice` in the importer, so including it in
  // the filter only misleads users about what's supported.
  ipcMain.handle("kcreate/figma/pickFile", async () => {
    const win = mainWindow;
    if (!win) return null;
    const result = await dialog.showOpenDialog(win, {
      title: "Import Figma JSON",
      properties: ["openFile"],
      filters: [
        { name: "Figma JSON (*.json, *.fig.json)", extensions: ["json"] },
        { name: "All files", extensions: ["*"] },
      ],
    });
    if (result.canceled || result.filePaths.length === 0) return null;
    return result.filePaths[0];
  });
  ipcMain.handle("kcreate/figma/import", (_e, filePath: string) =>
    requireBridge().figmaImport(filePath),
  );
  // `kcreate/sketch/pickFile` — scoped to `.sketch` (Sketch's ZIP
  // archive container).
  ipcMain.handle("kcreate/sketch/pickFile", async () => {
    const win = mainWindow;
    if (!win) return null;
    const result = await dialog.showOpenDialog(win, {
      title: "Import Sketch file",
      properties: ["openFile"],
      filters: [
        { name: "Sketch", extensions: ["sketch"] },
        { name: "All files", extensions: ["*"] },
      ],
    });
    if (result.canceled || result.filePaths.length === 0) return null;
    return result.filePaths[0];
  });
  ipcMain.handle("kcreate/sketch/import", (_e, filePath: string) =>
    requireBridge().sketchImport(filePath),
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
  // Phase 5 — spot color library (Block D Task 23). Each call mutates
  // the project's `SpotColorLibrary` through an undoable operation;
  // the renderer hydrates its swatch panel via `kcreate/color/spot/list`.
  // ---------------------------------------------------------------------
  ipcMain.handle("kcreate/color/spot/upsert", (_e, wireJson: string) => {
    requireBridge().colorSpotUpsert(wireJson);
  });
  ipcMain.handle("kcreate/color/spot/remove", (_e, name: string) =>
    requireBridge().colorSpotRemove(name),
  );
  ipcMain.handle("kcreate/color/spot/list", () =>
    requireBridge().colorSpotList(),
  );
  ipcMain.handle(
    "kcreate/color/spot/load-catalog",
    (_e, rawJson: string) => requireBridge().colorSpotLoadCatalog(rawJson),
  );

  // ---------------------------------------------------------------------
  // Phase 5 — smart-guides snap engine (Block C Task 13). The
  // `CanvasHost` calls this on every drag-move event and applies the
  // returned delta + renders the guide list as a dashed overlay.
  // ---------------------------------------------------------------------
  ipcMain.handle(
    "kcreate/canvas/snap",
    (
      _e,
      movingId: string | null,
      candidateX: number,
      candidateY: number,
      candidateW: number,
      candidateH: number,
      threshold: number,
    ) =>
      requireBridge().canvasSnap(
        movingId,
        candidateX,
        candidateY,
        candidateW,
        candidateH,
        threshold,
      ),
  );

  // ---------------------------------------------------------------------
  // Phase 5 — raster filters (Block B Task 11). Live preview goes
  // through `kcreate/raster/preview` (non-destructive); the rest are
  // undoable commits.
  // ---------------------------------------------------------------------
  // Phase 11 Block B: raster filters became Promise-returning. The
  // handlers must `await` so ipcMain.handle forwards rejection back
  // to the renderer's `invoke`.
  ipcMain.handle(
    "kcreate/raster/apply/levels",
    async (_e, nodeId: string, black: number, white: number, gamma: number) => {
      await requireBridge().rasterApplyLevels(nodeId, black, white, gamma);
    },
  );
  ipcMain.handle(
    "kcreate/raster/apply/curves",
    async (_e, nodeId: string, pointsJson: string) => {
      await requireBridge().rasterApplyCurves(nodeId, pointsJson);
    },
  );
  ipcMain.handle(
    "kcreate/raster/apply/blur",
    async (_e, nodeId: string, radius: number, kind: string) => {
      await requireBridge().rasterApplyBlur(nodeId, radius, kind);
    },
  );
  ipcMain.handle(
    "kcreate/raster/apply/sharpen",
    async (
      _e,
      nodeId: string,
      radius: number,
      amount: number,
      threshold: number,
    ) => {
      await requireBridge().rasterApplySharpen(nodeId, radius, amount, threshold);
    },
  );
  ipcMain.handle(
    "kcreate/raster/crop",
    async (_e, nodeId: string, x: number, y: number, w: number, h: number) => {
      await requireBridge().rasterCrop(nodeId, x, y, w, h);
    },
  );
  // Phase 11 Block B follow-up — Devin Review ANALYSIS-0003.
  // Rotate / flip / heal are now async (AsyncTask) on the Rust
  // side; `await` so the IPC reply is sent only after the worker
  // finishes mutating the layer (renderer relies on the resolved
  // promise to invalidate caches and refresh the tree).
  ipcMain.handle(
    "kcreate/raster/rotate",
    async (_e, nodeId: string, angleDeg: number) => {
      await requireBridge().rasterRotate(nodeId, angleDeg);
    },
  );
  ipcMain.handle(
    "kcreate/raster/flip",
    async (_e, nodeId: string, direction: string) => {
      await requireBridge().rasterFlip(nodeId, direction);
    },
  );
  ipcMain.handle(
    "kcreate/raster/heal",
    async (
      _e,
      nodeId: string,
      srcX: number,
      srcY: number,
      dstX: number,
      dstY: number,
      radius: number,
    ) => {
      await requireBridge().rasterHeal(nodeId, srcX, srcY, dstX, dstY, radius);
    },
  );
  ipcMain.handle(
    "kcreate/raster/preview",
    (_e, nodeId: string, filterJson: string) =>
      requireBridge().rasterPreviewFilter(nodeId, filterJson),
  );

  // -------------------------------------------------------------------
  // Phase 8 Block B — perspective transform, HSL, color balance, and
  // mask-aware filter application. All commit-only (no preview path)
  // because the live-preview surface for these ops re-uses
  // `kcreate/raster/preview` with the extended `PreviewFilter` enum.
  // -------------------------------------------------------------------
  ipcMain.handle(
    "kcreate/raster/perspective",
    async (_e, nodeId: string, cornersJson: string) => {
      await requireBridge().rasterPerspective(nodeId, cornersJson);
    },
  );
  ipcMain.handle(
    "kcreate/raster/apply/hsl",
    async (
      _e,
      nodeId: string,
      hue: number,
      saturation: number,
      lightness: number,
    ) => {
      await requireBridge().rasterApplyHsl(nodeId, hue, saturation, lightness);
    },
  );
  ipcMain.handle(
    "kcreate/raster/apply/color_balance",
    async (
      _e,
      nodeId: string,
      shadowsJson: string,
      midtonesJson: string,
      highlightsJson: string,
    ) => {
      await requireBridge().rasterApplyColorBalance(
        nodeId,
        shadowsJson,
        midtonesJson,
        highlightsJson,
      );
    },
  );
  ipcMain.handle(
    "kcreate/raster/apply/filter_masked",
    async (_e, nodeId: string, filterJson: string, mask: Buffer) => {
      // `mask` arrives as a Node `Buffer` because the preload wraps
      // the renderer-supplied `Uint8Array` with
      // `Buffer.from(buffer, byteOffset, byteLength)` before invoke;
      // typing it as `Buffer` here keeps the contract obvious and
      // matches the napi-rs `Buffer` decoder in `raster_apply_filter_masked`.
      await requireBridge().rasterApplyFilterMasked(nodeId, filterJson, mask);
    },
  );

  // ---------------------------------------------------------------------
  // Phase 5 — vector path operations + non-destructive effects.
  // (Block C Tasks 15, 16, 18.) All mutating; see Rust-side
  // `vector_ops.rs` for argument validation rules.
  // ---------------------------------------------------------------------
  ipcMain.handle(
    "kcreate/vector/simplify",
    (_e, nodeId: string, tolerance: number) => {
      requireBridge().vectorSimplify(nodeId, tolerance);
    },
  );
  ipcMain.handle(
    "kcreate/vector/smooth",
    (_e, nodeId: string, iterations: number) => {
      requireBridge().vectorSmooth(nodeId, iterations);
    },
  );
  ipcMain.handle(
    "kcreate/vector/offset",
    (_e, nodeId: string, distance: number) => {
      requireBridge().vectorOffset(nodeId, distance);
    },
  );
  ipcMain.handle(
    "kcreate/vector/strokeProfile/set",
    (_e, nodeId: string, profileJson: string) => {
      requireBridge().vectorSetStrokeProfile(nodeId, profileJson);
    },
  );
  ipcMain.handle(
    "kcreate/vector/pathEffect/apply",
    (_e, nodeId: string, effectJson: string) => {
      requireBridge().vectorApplyPathEffect(nodeId, effectJson);
    },
  );
  ipcMain.handle(
    "kcreate/vector/pathEffect/clear",
    (_e, nodeId: string) => {
      requireBridge().vectorClearPathEffects(nodeId);
    },
  );

  // ---------------------------------------------------------------------
  // Phase 5 — text frame linking + wrap (Block D Tasks 19/20).
  // ---------------------------------------------------------------------
  ipcMain.handle(
    "kcreate/text/frame/link",
    (_e, aId: string, bId: string) => {
      requireBridge().textFrameLink(aId, bId);
    },
  );
  ipcMain.handle(
    "kcreate/text/frame/unlink",
    (_e, nodeId: string) => {
      requireBridge().textFrameUnlink(nodeId);
    },
  );
  ipcMain.handle(
    "kcreate/text/frame/wrap/set",
    (_e, nodeId: string, modeJson: string) => {
      requireBridge().textFrameSetWrap(nodeId, modeJson);
    },
  );

  // ---------------------------------------------------------------------
  // Phase 5 — slices (Block D Task 22).
  // ---------------------------------------------------------------------
  ipcMain.handle(
    "kcreate/slice/create",
    (
      _e,
      name: string,
      x: number,
      y: number,
      w: number,
      h: number,
      format: string,
      scale: number,
    ) => requireBridge().sliceCreate(name, x, y, w, h, format, scale),
  );
  ipcMain.handle(
    "kcreate/slice/update",
    (_e, sliceId: string, changesJson: string) => {
      requireBridge().sliceUpdate(sliceId, changesJson);
    },
  );
  ipcMain.handle("kcreate/slice/delete", (_e, sliceId: string) =>
    requireBridge().sliceDelete(sliceId),
  );
  ipcMain.handle("kcreate/slice/list", () => requireBridge().sliceList());
  ipcMain.handle("kcreate/slice/exportAll", (_e, outputDir: string) =>
    requireBridge().sliceExportAll(outputDir),
  );

  // ---------------------------------------------------------------------
  // Phase 5 — `.kbrand` import/export (Block D Task 21).
  // ---------------------------------------------------------------------
  ipcMain.handle(
    "kcreate/brandKit/export",
    (_e, kitId: string, outputPath: string) => {
      requireBridge().brandKitExport(kitId, outputPath);
    },
  );
  ipcMain.handle("kcreate/brandKit/import", (_e, filePath: string) =>
    requireBridge().brandKitImport(filePath),
  );

  // ---------------------------------------------------------------------
  // Phase 5 — spot color / overprint shortcuts (Block D Task 23).
  // ---------------------------------------------------------------------
  ipcMain.handle(
    "kcreate/color/spot/add",
    (_e, name: string, c: number, m: number, y: number, k: number) => {
      requireBridge().colorAddSpot(name, c, m, y, k);
    },
  );
  ipcMain.handle(
    "kcreate/node/overprint/set",
    (_e, nodeId: string, enabled: boolean) => {
      requireBridge().nodeSetOverprint(nodeId, enabled);
    },
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
  // Phase A1 — inline text editor + font controls.
  //
  // The five mutators record undoable operations in the project's
  // log (the bridge handles operation construction + scene-sync
  // republish); `listFonts` is a pure read of the process-wide
  // FontManager. All channels live under `kcreate/text/*` next to
  // the existing text-frame + OpenType handlers.
  // ---------------------------------------------------------------------
  ipcMain.handle(
    "kcreate/text/content/set",
    (_e, nodeId: string, content: string) => {
      requireBridge().textSetContent(nodeId, content);
    },
  );
  ipcMain.handle(
    "kcreate/text/style/set",
    (_e, nodeId: string, styleJson: string) => {
      requireBridge().textSetStyle(nodeId, styleJson);
    },
  );
  ipcMain.handle(
    "kcreate/text/range/replace",
    (
      _e,
      nodeId: string,
      start: number,
      end: number,
      replacement: string,
    ) => {
      requireBridge().textReplaceRange(nodeId, start, end, replacement);
    },
  );
  ipcMain.handle("kcreate/text/content/get", (_e, nodeId: string) =>
    requireBridge().textContentGet(nodeId),
  );
  ipcMain.handle("kcreate/text/style/get", (_e, nodeId: string) =>
    requireBridge().textStyleGet(nodeId),
  );
  ipcMain.handle("kcreate/text/fonts/list", () =>
    requireBridge().textListFonts(),
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
      // Phase 7 (Task 7): optional community gate. Defaults to
      // `null` so legacy renderer callers that don't pass the
      // argument still work unchanged.
      communityId: string | null = null,
      // Phase 7 (Task 21): absolute path to the open project's
      // `.kstudio/` directory. When supplied the bridge loads
      // `<dir>/acl.json` at session start and persists every ACL
      // mutation back to that file so peer-allowlist edits
      // survive process restart. Defaults to `null` so legacy
      // renderer callers that don't pass the argument still work
      // unchanged (ACL stays in-memory only).
      projectDir: string | null = null,
    ) => {
      const report = requireBridge().sessionStart(
        seedB64,
        displayName,
        projectId,
        advertiseMdns,
        communityId,
        projectDir,
      );
      startSessionEventTick();
      return report;
    },
  );
  ipcMain.handle("kcreate/session/leave", () => {
    // Drain whatever events the bridge still has buffered before we
    // tear down the session — anything queued between the last
    // tick and now would otherwise be lost when the slot drops in
    // sessionLeave(). After the leave succeeds we synthesise the
    // `sessionLeft` event from the returned peer id, then stop the
    // tick. Doing the tick-stop *after* the synthetic emit means
    // we don't race a subsequent in-flight tick fetching events
    // from a now-empty slot (the bridge's `session_drain_events`
    // returns NotRunning post-leave, which the drain function
    // swallows quietly, so this is defence-in-depth rather than
    // strictly required).
    drainSessionEvents();
    const leftPeerId = requireBridge().sessionLeave();
    if (leftPeerId !== null) {
      const win = mainWindow;
      if (win && !win.isDestroyed()) {
        // Emit on the same `kcreate/session/event` channel every
        // other session signal flows through so renderer consumers
        // (useSessionLocks, EditorPage presence-broadcast effect,
        // PresencePanel) see local-side teardown through their
        // existing subscription without a separate code path. The
        // wire shape matches `SessionEvent::SessionLeft` in
        // `crates/kcreate_bridge/src/collab.rs`, which mirrors
        // `shared/scene.ts::SessionEvent`.
        win.webContents.send(
          "kcreate/session/event",
          JSON.stringify({ kind: "sessionLeft", peerId: leftPeerId }),
        );
      }
    }
    stopSessionEventTick();
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
  ipcMain.handle(
    "kcreate/kchat/derive-local-identity",
    (_e, seedB64: string) => requireBridge().kchatDeriveLocalIdentity(seedB64),
  );
  // Dev-only mint endpoints. Both probes return false / throw a
  // typed error on production bridges (the function is either
  // absent from the bridge binary entirely, or the feature-gated
  // shim returns `KChatDevIssuerDisabled`). The renderer treats
  // `available === false` as "hide the affordance".
  ipcMain.handle("kcreate/kchat/dev-issuer-available", () => {
    const fn = requireBridge().kchatDevIssuerAvailable;
    if (typeof fn !== "function") return false;
    try {
      return fn();
    } catch {
      // A throwing probe means the function exists but the bridge
      // is in some unexpected state. Treat the same as "off" so
      // the UI doesn't offer an affordance that won't work.
      return false;
    }
  });
  ipcMain.handle(
    "kcreate/kchat/dev-mint-membership",
    (_e, requestJson: string) => {
      const fn = requireBridge().kchatDevMintMembership;
      if (typeof fn !== "function") {
        throw new Error(
          "kchat: dev issuer not enabled in this build " +
            "(rebuild kcreate_bridge with --features kchat-dev-issuer)",
        );
      }
      return fn(requestJson);
    },
  );
  // Trusted-issuer allowlist for distinguishing real KChat
  // installs from dev-mint installs. The bridge starts with an
  // empty list (= "accept any issuer", preserving the dev flow),
  // gets pointed at the persistent JSON file at startup (see
  // `whenReady` below), and exposes add/remove/list to the
  // renderer's KChatSignInPanel.
  ipcMain.handle("kcreate/kchat/set-trust-store-path", (_e, p: string) => {
    // Non-collab developer builds omit the trust-store ABI from the
    // cdylib (the JS bridge does NOT synthesise it — see
    // `applyCollabFallbacks` in bridge.ts — so `initializeKChatTrustStore`
    // can detect its absence). Probe before calling so an unused IPC
    // invocation degrades to an idempotent no-op instead of throwing a
    // `TypeError` back across the channel. Mirrors the guard in
    // `initializeKChatTrustStore` below.
    const fn = requireBridge().kchatSetTrustStorePath;
    if (typeof fn !== "function") {
      return undefined;
    }
    return fn.call(requireBridge(), p);
  });
  ipcMain.handle("kcreate/kchat/trusted-issuers", () =>
    requireBridge().kchatTrustedIssuers(),
  );
  ipcMain.handle(
    "kcreate/kchat/add-trusted-issuer",
    (_e, issuerJson: string) =>
      requireBridge().kchatAddTrustedIssuer(issuerJson),
  );
  ipcMain.handle(
    "kcreate/kchat/remove-trusted-issuer",
    (_e, issuerPublicKey: string) =>
      requireBridge().kchatRemoveTrustedIssuer(issuerPublicKey),
  );

  // -------------------------------------------------------------------
  // Phase 7 — KChat backend (HTTPS REST). All entry points except
  // the `available` probe are optional on the bridge: a non-collab
  // or non-`kchat-backend` build simply doesn't link them, and the
  // handlers below return a typed error so the renderer can fall
  // back to the paste-attestation flow. Channels follow the
  // `kcreate/kchat-backend/*` namespace per Option C spec.
  // -------------------------------------------------------------------
  ipcMain.handle("kcreate/kchat-backend/available", () => {
    const fn = requireBridge().kchatBackendAvailable;
    if (typeof fn !== "function") return false;
    try {
      return fn();
    } catch {
      return false;
    }
  });
  // Helper: resolve an optional `kchat-backend` bridge function or
  // throw a typed error. Keeps each handler one line below.
  const requireKChatBackend = <K extends keyof Bridge>(method: K): Bridge[K] => {
    const fn = requireBridge()[method];
    if (typeof fn !== "function") {
      throw new Error(
        `kchat-backend: ${String(method)} not available in this build ` +
          "(rebuild kcreate_bridge with --features kchat-backend)",
      );
    }
    return fn;
  };
  ipcMain.handle(
    "kcreate/kchat-backend/connect",
    (_e, requestJson: string) =>
      (
        requireKChatBackend("kchatBackendConnect") as (
          requestJson: string,
        ) => string
      )(requestJson),
  );
  ipcMain.handle("kcreate/kchat-backend/disconnect", () =>
    (requireKChatBackend("kchatBackendDisconnect") as () => string)(),
  );
  ipcMain.handle("kcreate/kchat-backend/status", () =>
    (requireKChatBackend("kchatBackendStatus") as () => string)(),
  );
  ipcMain.handle("kcreate/kchat-backend/list-communities", () =>
    (requireKChatBackend("kchatBackendListCommunities") as () => string)(),
  );
  ipcMain.handle(
    "kcreate/kchat-backend/select-community",
    (_e, communityId: string) =>
      (
        requireKChatBackend("kchatBackendSelectCommunity") as (
          id: string,
        ) => string
      )(communityId),
  );
  ipcMain.handle(
    "kcreate/kchat-backend/get-community-members",
    (_e, communityId: string) =>
      (
        requireKChatBackend("kchatBackendGetCommunityMembers") as (
          id: string,
        ) => string
      )(communityId),
  );
  ipcMain.handle(
    "kcreate/kchat-backend/list-conversations",
    (_e, communityId: string) =>
      (
        requireKChatBackend("kchatBackendListConversations") as (
          id: string,
        ) => string
      )(communityId),
  );
  ipcMain.handle(
    "kcreate/kchat-backend/share-to-conversation",
    (_e, conversationId: string, inviteJson: string) =>
      (
        requireKChatBackend("kchatBackendShareToConversation") as (
          c: string,
          i: string,
        ) => string
      )(conversationId, inviteJson),
  );

  // Phase 7 (Task 10): accept invite.
  ipcMain.handle(
    "kcreate/kchat-backend/accept-invite",
    (_e, inviteJson: string) =>
      (
        requireKChatBackend("kchatBackendAcceptInvite") as (
          j: string,
        ) => string
      )(inviteJson),
  );

  // Phase 7 (Task 8): roster-sync tick.
  ipcMain.handle(
    "kcreate/kchat-backend/sync-community-roster",
    (_e, communityId: string) =>
      (
        requireKChatBackend("kchatBackendSyncCommunityRoster") as (
          c: string,
        ) => string
      )(communityId),
  );

  // Phase 8 (Block A, Task 2): publish an exported artifact to a
  // KChat conversation. `requestJson` is a JSON-encoded
  // `KChatArtifactPublishRequest`; returns the JSON-encoded
  // `KChatArtifactPublishResult`.
  ipcMain.handle(
    "kcreate/kchat-backend/publish-artifact",
    (_e, conversationId: string, requestJson: string) =>
      (
        requireKChatBackend("kchatBackendPublishArtifact") as (
          c: string,
          r: string,
        ) => string
      )(conversationId, requestJson),
  );

  // Phase 8 (Block A, Task 2): publish a `.kbrand` brand-kit
  // archive to a KChat conversation.
  ipcMain.handle(
    "kcreate/kchat-backend/publish-brand-kit",
    (_e, conversationId: string, requestJson: string) =>
      (
        requireKChatBackend("kchatBackendPublishBrandKit") as (
          c: string,
          r: string,
        ) => string
      )(conversationId, requestJson),
  );

  // Phase 8 (Block A, Task 2): list previously-published
  // artifacts for the given conversation.
  ipcMain.handle(
    "kcreate/kchat-backend/list-artifacts",
    (_e, conversationId: string) =>
      (
        requireKChatBackend("kchatBackendListArtifacts") as (
          c: string,
        ) => string
      )(conversationId),
  );

  // Phase 7 (Task 8): kick a connected peer.
  ipcMain.handle(
    "kcreate/session/kick-peer",
    (_e, peerId: string, reason: string) =>
      requireBridge().sessionKickPeer(peerId, reason),
  );

  // Phase 7 (Task 15): ask a connected host to backfill journal
  // entries we are missing relative to our local ResumeVector.
  ipcMain.handle(
    "kcreate/session/request-resume",
    (_e, peerId: string) => requireBridge().sessionRequestResume(peerId),
  );

  // Phase 7 (Task 11): set peer permission.
  ipcMain.handle(
    "kcreate/session/set-peer-permission",
    (_e, peerId: string, permission: string) =>
      requireBridge().sessionSetPeerPermission(peerId, permission),
  );

  // Phase 7 (Task 11): local permission snapshot.
  ipcMain.handle("kcreate/session/local-permission", () =>
    requireBridge().sessionLocalPermission(),
  );

  // Phase 7 (Task 21): ACL snapshot / replace.
  ipcMain.handle("kcreate/session/acl-get", () =>
    requireBridge().sessionAclGet(),
  );
  ipcMain.handle("kcreate/session/acl-set", (_e, aclJson: string) =>
    requireBridge().sessionAclSet(aclJson),
  );

  // Phase 7 (Task 19): force a key rotation / read the current epoch.
  ipcMain.handle("kcreate/session/rotate-keys", (_e, graceMs: number) =>
    requireBridge().sessionRotateKeys(graceMs),
  );
  ipcMain.handle("kcreate/session/key-epoch", () =>
    requireBridge().sessionKeyEpoch(),
  );

  // Phase 7 (Task 23): encrypted clipboard sharing. The bridge
  // holds the local signing key from session_start — no seed
  // travels back through IPC.
  ipcMain.handle(
    "kcreate/session/clipboard-share",
    (_e, peerId: string, plaintext: Buffer, previewLabel: string) =>
      requireBridge().sessionClipboardShare(peerId, plaintext, previewLabel),
  );
  ipcMain.handle(
    "kcreate/session/clipboard-accept",
    (_e, offerId: string) => requireBridge().sessionClipboardAccept(offerId),
  );
  ipcMain.handle("kcreate/session/clipboard-reject", (_e, offerId: string) =>
    requireBridge().sessionClipboardReject(offerId),
  );
  ipcMain.handle("kcreate/session/pending-clipboard-offers", () =>
    requireBridge().sessionPendingClipboardOffers(),
  );
  // Phase 7 (Task 25): outbound op throttle wiring. The renderer
  // calls `queue-operation` on every local mutation,
  // `tick-outbound-batch` on the same cadence as the event drain
  // (so the bridge can flush when the 50 ms timer expires without
  // renderer bookkeeping), and `flush-pending-operations` at the
  // end of a drag interaction. Channel names use kebab-case to
  // match the rest of the `kcreate/session/*` IPC surface
  // (`request-resume`, `cert-fingerprint`, `pending-clipboard-offers`,
  // etc.).
  ipcMain.handle("kcreate/session/queue-operation", (_e, opJson: string) =>
    requireBridge().sessionQueueOperation(opJson),
  );
  ipcMain.handle("kcreate/session/flush-pending-operations", () =>
    requireBridge().sessionFlushPendingOperations(),
  );
  ipcMain.handle("kcreate/session/tick-outbound-batch", () =>
    requireBridge().sessionTickOutboundBatch(),
  );
  // Phase 7 (Task 27): selective sync — tell the bridge which
  // pages the local peer is currently viewing. Presence updates
  // and conflict toasts for off-screen pages are suppressed from
  // the renderer event stream; operations are still journaled.
  ipcMain.handle("kcreate/session/set-active-pages", (_e, pageIdsJson: string) =>
    requireBridge().sessionSetActivePages(pageIdsJson),
  );
}

// ---------------------------------------------------------------------------
// Phase 7 — `kcreate://` deeplink scheme.
//
// The companion `.kcz` extension in `apps/kchat-extension/` builds
// `kcreate://join?payload=<base64url(invite_json)>` URLs that KChat
// Desktop fires through the OS shell. We:
//
//   1. Register `kcreate` as a custom protocol so the OS routes it
//      to KCreate even when KCreate is closed.
//   2. Hold a single-instance lock so a second OS-spawned KCreate
//      process forwards the deeplink to the running instance via
//      the `second-instance` event (Windows + Linux). macOS hands
//      the URL through the dedicated `open-url` event instead.
//   3. Scan `process.argv` on first launch (Windows + Linux only —
//      on macOS the argv path is empty for deeplinks).
//   4. Buffer URLs that arrive before the renderer is ready and
//      flush them once `did-finish-load` fires, so a cold-start
//      deeplink isn't lost between the OS hand-off and the React
//      tree mounting.
//   5. Forward every accepted URL to the renderer through the
//      `kcreate/deeplink/received` IPC channel; the renderer side
//      lives in `InvitePanel.tsx` (Phase 7 Task 10).
//
// Only `kcreate://` URLs are accepted. Any other scheme that lands
// here is dropped to keep the deeplink surface tight.
// ---------------------------------------------------------------------------

const DEEPLINK_SCHEME = "kcreate";
const DEEPLINK_CHANNEL = "kcreate/deeplink/received";

// Hard cap the cold-start deeplink buffer so a renderer that
// crashes mid-load (or a `did-finish-load` event that never fires)
// can't drive unbounded memory growth if the OS keeps handing us
// new `kcreate://` URLs through the protocol activation path. Once
// the cap is hit we drop the *oldest* pending URL on each push so
// the most recent invite wins — a stale invite is less useful than
// the one the user clicked five seconds ago.
const MAX_PENDING_DEEPLINKS = 50;
const pendingDeeplinks: string[] = [];

function pushPendingDeeplink(url: string): void {
  if (pendingDeeplinks.length >= MAX_PENDING_DEEPLINKS) {
    pendingDeeplinks.shift();
  }
  pendingDeeplinks.push(url);
}

function isKcreateUrl(value: string): boolean {
  // We accept both `kcreate://...` and (rarely-seen on Windows
  // shells) `kcreate:...` so the renderer doesn't have to guess.
  return value.startsWith(`${DEEPLINK_SCHEME}://`) || value.startsWith(`${DEEPLINK_SCHEME}:`);
}

function extractDeeplinksFromArgv(argv: readonly string[]): string[] {
  return argv.filter(isKcreateUrl);
}

function dispatchDeeplink(url: string): void {
  if (!isKcreateUrl(url)) {
    return;
  }
  const win = mainWindow;
  if (!win || win.webContents.isLoading() || win.webContents.isDestroyed()) {
    // Buffer until the renderer is ready. A cold-start deeplink
    // path lands here when the OS launches us straight from the
    // protocol hand-off. The buffer is capped so a stuck renderer
    // doesn't trigger unbounded memory growth.
    pushPendingDeeplink(url);
    return;
  }
  try {
    win.webContents.send(DEEPLINK_CHANNEL, url);
    // Bring the window forward so a one-click deeplink lands the
    // user on the join UI without an extra Alt-Tab.
    if (win.isMinimized()) {
      win.restore();
    }
    win.focus();
  } catch (err) {
    console.error("kcreate: failed to dispatch deeplink", url, err);
  }
}

function flushPendingDeeplinks(): void {
  const win = mainWindow;
  if (!win || win.webContents.isDestroyed()) {
    return;
  }
  while (pendingDeeplinks.length > 0) {
    const url = pendingDeeplinks.shift()!;
    try {
      win.webContents.send(DEEPLINK_CHANNEL, url);
    } catch (err) {
      console.error("kcreate: failed to flush deeplink", url, err);
    }
  }
}

function registerProtocolHandler(): void {
  // `setAsDefaultProtocolClient` returns false if the OS refuses
  // (e.g. another app is registered and the user hasn't approved
  // the switch). That's not fatal — KCreate still works without
  // the deeplink path — so we just log and move on.
  let ok: boolean;
  if (process.platform === "win32" && process.defaultApp) {
    // During `electron .` dev runs the entry-point script is
    // argv[1]; pass it through so the spawned secondary instance
    // can find our app code.
    const script = process.argv[1];
    ok =
      typeof script === "string"
        ? app.setAsDefaultProtocolClient(DEEPLINK_SCHEME, process.execPath, [
            path.resolve(script),
          ])
        : app.setAsDefaultProtocolClient(DEEPLINK_SCHEME);
  } else {
    ok = app.setAsDefaultProtocolClient(DEEPLINK_SCHEME);
  }
  if (!ok) {
    console.warn(
      `kcreate: failed to register ${DEEPLINK_SCHEME}:// protocol (already registered by another app?)`,
    );
  }
}

/// Point the KChat trust-store at the per-user JSON file under
/// the Electron `userData` directory and surface any I/O failure
/// to the main-process log. We deliberately swallow errors here
/// rather than crashing the app — a missing file is treated as
/// "empty allowlist" on the Rust side, and any deeper I/O issue
/// (permissions, corrupt JSON) is non-fatal to the editor. The
/// renderer surfaces a banner if subsequent add/remove calls
/// fail to persist. Idempotent: safe to call multiple times.
///
/// `kchatSetTrustStorePath` is gated behind the `collab` Cargo
/// feature in `kcreate_bridge`. Every shipped desktop artifact
/// builds with `collab` enabled, so in practice the function is
/// always present — but we probe with `typeof fn !== "function"`
/// before calling so non-collab developer builds don't generate
/// a spurious "function is not a function" stack trace at every
/// startup. Mirrors the pattern used for `kchatDevIssuerAvailable`
/// (`registerIpcHandlers` above).
function initializeKChatTrustStore(): void {
  const fn = requireBridge().kchatSetTrustStorePath;
  if (typeof fn !== "function") {
    // Non-collab build: the trust-store ABI isn't compiled in.
    // The bridge's add/remove/list endpoints will also be absent,
    // and the renderer's KChatSignInPanel won't be able to call
    // them — but that's the expected non-collab state. Silent
    // return rather than logging so we don't spam the console
    // every startup.
    return;
  }
  try {
    const trustFile = path.join(app.getPath("userData"), "kchat_trust.json");
    fn.call(requireBridge(), trustFile);
  } catch (err) {
    console.error("kchat: failed to initialise trust store on disk", err);
  }
}

// Acquire the single-instance lock BEFORE `app.whenReady`. When a
// user clicks a `kcreate://` deeplink while KCreate is already
// running the OS spawns a fresh KCreate process; that second
// process bails out here and the running primary instance picks
// up the deeplink via the `second-instance` event below.
if (!app.requestSingleInstanceLock()) {
  app.quit();
} else {
  app.on("second-instance", (_event, argv) => {
    // Windows / Linux: the deeplink URL is the last argv entry of
    // the secondary instance. Forward whatever we find to the
    // renderer (the helper drops non-`kcreate:` strings).
    for (const url of extractDeeplinksFromArgv(argv)) {
      dispatchDeeplink(url);
    }
    // Focus + restore even when argv carried no URL, so a second
    // launch (e.g. user double-clicking the dock icon) still
    // brings the existing window forward.
    const win = mainWindow;
    if (win) {
      if (win.isMinimized()) win.restore();
      win.focus();
    }
  });

  // macOS hands deeplinks through the dedicated `open-url` event
  // rather than argv. The event can fire before `whenReady` — we
  // still buffer the URL through `dispatchDeeplink` (which routes
  // to `pendingDeeplinks` when the window doesn't exist yet) so a
  // cold-start deeplink isn't lost.
  app.on("open-url", (event, url) => {
    event.preventDefault();
    dispatchDeeplink(url);
  });

  void app.whenReady().then(() => {
    // Load the native bridge synchronously, before any window/IPC traffic
    // can hit `requireBridge()`. See the comment above `let bridge`.
    bridge = loadBridge();
    registerIpcHandlers();
    // Wire the KChat trust-store at `<userData>/kchat_trust.json`.
    // Must run AFTER the bridge is loaded (it dispatches an N-API
    // call) but BEFORE any renderer window opens (so the first
    // `kchat.status()` poll already sees the loaded allowlist).
    initializeKChatTrustStore();
    // Register the `kcreate://` protocol AFTER the lock is held
    // and BEFORE the window is created so any URL that happens to
    // be sitting in argv from a cold-start path is picked up by
    // the buffer below.
    registerProtocolHandler();
    const win = createWindow();
    // Drain any deeplinks that arrived before the window mounted
    // (cold-start path: OS spawns us straight from the protocol
    // hand-off, the URL is in argv, and the renderer needs the
    // payload as soon as the React tree mounts).
    win.webContents.once("did-finish-load", () => {
      flushPendingDeeplinks();
    });
    // Cold-start argv scan (Windows / Linux only — macOS routes
    // through `open-url` and the argv list is empty for deeplink
    // launches). The first hit goes into `pendingDeeplinks` and is
    // flushed by the `did-finish-load` hook above.
    for (const url of extractDeeplinksFromArgv(process.argv)) {
      dispatchDeeplink(url);
    }

    app.on("activate", () => {
      if (BrowserWindow.getAllWindows().length === 0) {
        // Mirror the cold-start flush wiring above so any deeplinks
        // that arrived while every window was closed (macOS: user
        // closed the last window, then clicked a `kcreate://` link
        // in KChat Desktop; the `open-url` listener buffered the
        // URL into `pendingDeeplinks`) are drained as soon as the
        // re-created renderer finishes loading. Without this hook
        // the buffered URLs sit in memory forever and the share
        // invite is silently lost.
        const reopened = createWindow();
        reopened.webContents.once("did-finish-load", () => {
          flushPendingDeeplinks();
        });
      }
    });
  });
}

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
      // Drain whatever events the bridge has buffered before we
      // pull the slot from under it; the bridge can't push events
      // through a queue it's about to drop. We don't bother
      // synthesising a `sessionLeft` on this path — the renderer
      // process is about to be destroyed alongside the main
      // process, so any consumer state it would reset is going to
      // disappear with the window. (The IPC handler path above
      // *does* emit the synthetic event because the renderer
      // continues running across a session leave.)
      try {
        drainSessionEvents();
      } catch {
        // best-effort
      }
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
