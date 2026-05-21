import { useCallback, useState } from "react";

import { EditorPage } from "./pages/EditorPage";
import { HomePage } from "./pages/HomePage";
import type { ProjectInfo } from "../../shared/scene";

type Route =
  | { kind: "home" }
  | { kind: "editor"; project: ProjectInfo }
  | { kind: "error"; message: string };

export function App(): JSX.Element {
  const [route, setRoute] = useState<Route>({ kind: "home" });

  const handleOpenEditor = useCallback(async (_jobKind: string) => {
    // Phase 0: we don't yet show a directory picker (electron dialog
    // glue lands with the project manager in Phase 1). Until then we
    // create a scratch project under the OS temp dir so the editor
    // skeleton has something to show. The Rust side already does the
    // right thing with persistence + reopen.
    try {
      const project = await openScratchProject();
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
  // accumulates indefinitely on Windows. The cleanup IPC is
  // intentionally fire-and-forget: we don't gate project creation on
  // it because a locked file (e.g. another KCreate instance holding
  // the SQLite file open) shouldn't block the user from opening their
  // new scratch.
  void window.kcreate.runtime
    .cleanupScratchProjects()
    .catch(() => {
      // Errors are already counted inside the host sweep; the renderer
      // doesn't surface them — best-effort housekeeping must never
      // block the user-facing path.
    });
  const name = `scratch-${Date.now()}`;
  const dir = await window.kcreate.runtime.tempDir();
  return window.kcreate.document.createProject(name, dir);
}
