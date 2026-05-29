// Shared helper for creating a fresh scratch project.
//
// Lives in its own module (rather than inside `App.tsx`) so any
// renderer component can compose scratch-project creation without
// importing the top-level `App` component. Previously, `BriefModal`
// imported this from `App.tsx` while `App.tsx` imported the page
// containing `BriefModal` — a circular dependency that happened to
// work only because the export was a hoisted `async function`
// declaration. Extracting the helper here breaks the cycle and
// makes the dependency direction one-way.

import type { ProjectInfo } from "../../../shared/scene";

/**
 * Materialise a brand-new "scratch" project on disk and return its
 * `ProjectInfo`. The project lives in the OS temp dir resolved by
 * the Electron main process via Node's `os.tmpdir()`, so the
 * renderer never hard-codes a platform-specific path.
 *
 * Before creating the new project, we ask the host to sweep stale
 * `scratch-*.kstudio` directories from the temp dir. The sweep is
 * `await`-ed (not fire-and-forget) on purpose: an earlier
 * implementation dispatched the cleanup as `void
 * cleanupScratchProjects()` so it wouldn't block on a locked file
 * from another running KCreate instance, but Devin Review caught
 * the race — `fs.readdir` runs on the libuv thread pool, so the
 * readdir can resolve *after* `createProject` has already mkdir'd
 * the new `scratch-{timestamp}.kstudio` directory. The new
 * directory's name matches the sweep filter, so the cleanup loop
 * would `fs.rm` it out from under the live SQLite handle on
 * macOS/Linux (no mandatory file locking) — corrupting the project
 * on the next save. Awaiting guarantees the sweep observes a temp
 * dir that *doesn't* yet contain the new project. The sweep is
 * fast (single `readdir` + N `rm` calls in parallel internally)
 * and already swallows per-entry errors, so locked files from
 * other instances are skipped without blocking the user.
 */
export async function openScratchProject(): Promise<ProjectInfo> {
  await window.kcreate.runtime.cleanupScratchProjects().catch(() => {
    // Errors are already counted inside the host sweep; the
    // renderer doesn't surface them — best-effort housekeeping
    // must never block the user-facing path.
  });
  const name = `scratch-${Date.now()}`;
  const dir = await window.kcreate.runtime.tempDir();
  return window.kcreate.document.createProject(name, dir);
}
