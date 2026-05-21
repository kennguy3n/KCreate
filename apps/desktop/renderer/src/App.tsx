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
  // Phase 0 scaffold: pick a deterministic scratch directory inside the
  // app's home folder. The host owns the path; the Rust bridge persists
  // documents under `<dir>/<name>.kstudio`.
  const name = `scratch-${Date.now()}`;
  const dir = await window.kcreate.runtime
    .status()
    .then((s) => `${s.platform === "WindowsX64" ? "C:\\\\Temp" : "/tmp"}`);
  return window.kcreate.document.createProject(name, dir);
}
