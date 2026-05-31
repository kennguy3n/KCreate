import { useCallback, useState } from "react";

import { openScratchProject } from "./lib/scratchProject";
import { templateResolverFor } from "./lib/templates";
import { EditorPage } from "./pages/EditorPage";
import { CREATE_OPTIONS, HomePage } from "./pages/HomePage";
import type { BriefApplyResult, ProjectInfo } from "../../shared/scene";

export { openScratchProject };

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
        // Two stages: (1) create the artboard, (2) if that succeeded
        // run the Track-2 template resolver to seed starter content
        // inside it. Step (2) is gated on step (1) so a failed
        // artboard.create can never produce orphan rect/text nodes
        // floating at the world origin. Both stages are non-fatal —
        // a failure leaves the user on a recoverable editor surface
        // (blank artboard / "+ New artboard" affordance still works).
        let artboardOk = false;
        try {
          await window.kcreate.artboard.create(
            null,
            preset.name,
            preset.width,
            preset.height,
          );
          artboardOk = true;
        } catch {
          // Non-fatal: the editor's artboard panel can still create
          // one manually. The error is swallowed here because surface
          // routes (App → EditorPage status bar) aren't wired yet at
          // this point in the boot sequence; the user sees an empty
          // editor and can recover by clicking "+ New artboard".
        }

        if (artboardOk) {
          // Track 2 — seed the artboard with starter content via the
          // template resolver registered for this card. The resolver
          // reads the artboard's world rect off `artboard.list()` so
          // we don't have to assume `(0, 0)` (the bridge offsets
          // subsequent artboards). Wrapped in try/catch — a resolver
          // failure should not block the editor from opening; the
          // user lands on a blank artboard they can still edit.
          try {
            const resolver = templateResolverFor(jobKind);
            if (resolver) {
              const artboards = await window.kcreate.artboard.list();
              // The artboard we just created is the most recent one
              // for the active project. Falling back to the preset
              // dimensions (origin 0,0) keeps the resolver running
              // even if listing fails or returns an empty array.
              const target =
                artboards.find((a) => a.name === preset.name) ??
                artboards[artboards.length - 1];
              const ctx = target
                ? {
                    x: target.x,
                    y: target.y,
                    width: target.width,
                    height: target.height,
                  }
                : { x: 0, y: 0, width: preset.width, height: preset.height };
              await resolver.apply(ctx);
              // Push the new nodes into the renderer's scene so the
              // editor opens on a populated canvas instead of waiting
              // for the next event-driven sync.
              await window.kcreate.canvas.syncScene();
            }
          } catch {
            // Non-fatal: same rationale as the artboard.create catch
            // above. A failed template seed should never block the
            // editor from opening.
          }
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

  // Phase 9 Block B Task 7 — Brief-to-project applied. The bridge
  // has already added the artboard, brand kit, and starter layers
  // to the currently open project. `BriefModal.applyPlan`
  // guarantees there is an open project (it materialises a scratch
  // one when invoked from HomePage), so `getProjectInfo` is
  // expected to be non-null on this path.
  const handleBriefApplied = useCallback(async (result: BriefApplyResult) => {
    try {
      const project = await window.kcreate.document.getProjectInfo();
      if (!project) {
        // Defensive: brief_to_project would have failed if no
        // project was open, so reaching here means the workspace
        // was closed between the apply call returning and this
        // callback firing. Surface a clear error.
        setRoute({
          kind: "error",
          message:
            "Brief applied but the project was closed before the editor could open it.",
        });
        return;
      }
      void result;
      setRoute({ kind: "editor", project });
    } catch (e) {
      setRoute({
        kind: "error",
        message: e instanceof Error ? e.message : String(e),
      });
    }
  }, []);

  // Open a project from the HomePage's Recent grid. The bridge's
  // `projectOpen` does the heavy lifting (workspace mount, scene
  // sync, audit + recent-list update). Failures route to the error
  // surface identically to the scratch-project path.
  const handleOpenProject = useCallback(async (projectDir: string) => {
    try {
      const project = await window.kcreate.document.openProject(projectDir);
      setRoute({ kind: "editor", project });
    } catch (e) {
      setRoute({
        kind: "error",
        message: e instanceof Error ? e.message : String(e),
      });
    }
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
  return (
    <HomePage
      onOpenEditor={handleOpenEditor}
      onOpenProject={(p) => void handleOpenProject(p)}
      onBriefApplied={(r) => void handleBriefApplied(r)}
    />
  );
}

