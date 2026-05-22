import { useCallback, useState } from "react";

import { EditorPage } from "./pages/EditorPage";
import { CREATE_OPTIONS, HomePage } from "./pages/HomePage";
import type { ProjectInfo } from "../../shared/scene";

type Route =
  | { kind: "home" }
  | { kind: "editor"; project: ProjectInfo }
  | { kind: "error"; message: string };

export function App(): JSX.Element {
  const [route, setRoute] = useState<Route>({ kind: "home" });

  const handleOpenEditor = useCallback(async (jobKind: string) => {
    // Phase 0: we don't yet show a directory picker (electron dialog
    // glue lands with the project manager in Phase 1). Until then we
    // create a scratch project under the OS temp dir so the editor
    // skeleton has something to show. The Rust side already does the
    // right thing with persistence + reopen.
    try {
      const project = await openScratchProject();
      // Seed the project with the artboard preset for the workflow
      // the user picked on the home page. We do this *after* the
      // project is created so the bridge has a workspace to register
      // the artboard against. Errors are surfaced via the editor's
      // status bar (the project still opens cleanly).
      const option = CREATE_OPTIONS.find((o) => o.id === jobKind);
      const preset = option?.defaultArtboard ?? null;
      if (preset) {
        try {
          await window.kcreate.artboard.create(
            null,
            preset.name,
            preset.width,
            preset.height,
          );
        } catch {
          // Non-fatal: the editor's artboard panel can still create
          // one manually. The error is swallowed here because surface
          // routes (App → EditorPage status bar) aren't wired yet at
          // this point in the boot sequence; the user sees an empty
          // editor and can recover by clicking "+ New artboard".
        }
      }
      setRoute({ kind: "editor", project });
    } catch (e) {
      setRoute({
        kind: "error",
        message: e instanceof Error ? e.message : String(e),
      });
    }
  }, []);

  const handleBackHome = useCallback(() => {
    void window.kcreate.document.closeProject().finally(() => {
      setRoute({ kind: "home" });
    });
  }, []);

  if (route.kind === "editor") {
    return (
      <EditorPage project={route.project} onBackHome={handleBackHome} />
    );
  }
  if (route.kind === "error") {
    return (
      <div
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          height: "100%",
          padding: 32,
          color: "#B91C1C",
        }}
      >
        Failed to open project: {route.message}
      </div>
    );
  }
  return <HomePage onOpenEditor={handleOpenEditor} />;
}

async function openScratchProject(): Promise<ProjectInfo> {
  // Phase 0 scaffold: drop the scratch project into the OS temp dir
  // resolved by the Electron main process via Node's `os.tmpdir()`. The
  // renderer never hard-codes paths — `/tmp` doesn't exist on Windows,
  // and the previous fallback (`C:\\\\Temp` in a JS source literal) was
  // double-escaped: the runtime string was `C:\\Temp` (with a literal
  // backslash before `Temp`) instead of the intended `C:\Temp`, so
  // project creation failed on Windows. The host's `os.tmpdir()` is
  // correct on every platform and survives sandboxing.
  //
  // Before creating the new scratch project, ask the host to sweep
  // stale `scratch-*.kstudio` directories from the temp dir. Without
  // this, every Home→Editor transition leaks one directory — harmless
  // on macOS/Linux (their temp reapers eventually clean up) but
  // accumulates indefinitely on Windows.
  //
  // The sweep is `await`-ed (NOT fire-and-forget) on purpose. An
  // earlier version of this code dispatched the cleanup as
  // `void cleanupScratchProjects()` so we wouldn't block on a locked
  // file from another running KCreate instance, but Devin Review
  // (BUG_0001 on PR #2) caught the race: `cleanupScratchProjects`'s
  // `fs.readdir` runs on the libuv thread pool, so the readdir can
  // resolve *after* `createProject` has already mkdir'd the new
  // `scratch-{timestamp}.kstudio` directory. The new directory's name
  // matches the sweep filter, so the cleanup loop would `fs.rm` it
  // out from under the live SQLite handle on macOS/Linux (no
  // mandatory file locking) — corrupting the project on the next
  // save. Awaiting guarantees the sweep observes a temp dir that
  // *doesn't* yet contain the new project. The sweep is fast
  // (single `readdir` + N `rm` calls in parallel internally) and
  // already swallows per-entry errors, so locked files from other
  // instances are skipped without blocking the user.
  await window.kcreate.runtime.cleanupScratchProjects().catch(() => {
    // Errors are already counted inside the host sweep; the renderer
    // doesn't surface them — best-effort housekeeping must never
    // block the user-facing path.
  });
  const name = `scratch-${Date.now()}`;
  const dir = await window.kcreate.runtime.tempDir();
  return window.kcreate.document.createProject(name, dir);
}
