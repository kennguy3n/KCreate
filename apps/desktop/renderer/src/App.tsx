import { lazy, Suspense, useCallback, useState } from "react";

import { ErrorBoundary } from "./components/ErrorBoundary";
import { TemplateGallery } from "./components/TemplateGallery";
import { useI18n } from "./i18n";
import { openScratchProject } from "./lib/scratchProject";
import { templateResolverFor } from "./lib/templates";
import { CREATE_OPTIONS, HomePage } from "./pages/HomePage";
import type {
  BriefApplyResult,
  ProjectInfo,
  ThemedDesignApplyResult,
} from "../../shared/scene";

export { openScratchProject };

// Lazily load the editor surface so the first paint stays light. The
// home screen is the entry point and needs none of the editor's weight
// — EditorPage transitively pulls the canvas/present pipeline, every
// overlay and panel, the tool state machine, and the shortcut registry,
// which together dominate the renderer bundle. Splitting it into its own
// chunk (fetched on first navigation into the editor) keeps the initial
// download small on-device. `EditorPage` is a named export, so map it
// onto the `default` export that `React.lazy` expects.
const EditorPage = lazy(() =>
  import("./pages/EditorPage").then((m) => ({ default: m.EditorPage })),
);

type Route =
  | { kind: "home" }
  | { kind: "templates" }
  | { kind: "editor"; project: ProjectInfo }
  | { kind: "error"; message: string };

// Minimal, theme-aware placeholder shown while the editor chunk is being
// fetched (see the lazy `EditorPage` import above). Kept tiny and
// dependency-free so it stays in the initial bundle and paints instantly
// on the brief gap between navigation and the chunk resolving.
function EditorLoading(): JSX.Element {
  const { t } = useI18n();
  return (
    <div
      role="status"
      aria-live="polite"
      style={{
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        height: "100%",
        background: "var(--kc-bg-canvas)",
        color: "var(--kc-text-muted)",
        fontSize: 14,
      }}
    >
      {t("app.editor.loading")}
    </div>
  );
}

// Recovery surface shown if the lazy editor chunk fails to load or the
// editor throws while mounting. Without this, a chunk-load failure
// (corrupted asar, a file missing after a partial update) would throw
// past the root and leave a blank white screen with no way out. Offers
// a reload (which re-fetches the chunk — the reliable recovery, since
// `React.lazy` caches a rejected import for the process lifetime) and a
// way back to the home screen.
function EditorLoadError(props: {
  message: string;
  onReload: () => void;
  onBackHome: () => void;
}): JSX.Element {
  const { t } = useI18n();
  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        alignItems: "center",
        justifyContent: "center",
        gap: 16,
        height: "100%",
        padding: 32,
        textAlign: "center",
        background: "var(--kc-bg-canvas)",
        color: "var(--kc-text)",
      }}
    >
      <div style={{ maxWidth: 420 }}>
        <div style={{ fontSize: 15, fontWeight: 600, marginBottom: 8 }}>
          {t("app.editor.loadFailed.title")}
        </div>
        <div style={{ fontSize: 13, color: "var(--kc-text-muted)" }}>
          {props.message}
        </div>
      </div>
      <div style={{ display: "flex", gap: 8 }}>
        <button type="button" onClick={props.onReload}>
          {t("app.action.reload")}
        </button>
        <button type="button" onClick={props.onBackHome}>
          {t("app.action.backToHome")}
        </button>
      </div>
    </div>
  );
}

export function App(): JSX.Element {
  const { t } = useI18n();
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
        //
        // `artboard.create()` returns the created artboard's id
        // directly from the bridge; we capture it so the template
        // resolver can look up the world rect by id (unambiguous)
        // rather than by name (which would race a hypothetical
        // pre-existing artboard with the same name — Devin Review
        // surfaced this on PR #31 as a latent footgun if
        // `handleOpenEditor` were ever extended to apply to a
        // non-fresh project).
        let createdArtboardId: string | null = null;
        try {
          createdArtboardId = await window.kcreate.artboard.create(
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

        if (createdArtboardId !== null) {
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
              // Look up the artboard by the id we just received from
              // the bridge — exact match, no name collisions. The
              // empty-list / id-not-found fallback uses the preset's
              // nominal dimensions at the origin; the resolver still
              // runs and lands nodes at world (0,0), which is at
              // worst visually misaligned (the user can fix it from
              // the editor). If `list()` itself *throws*, the outer
              // try/catch below skips the resolver entirely — a
              // thrown `list()` likely means the bridge state is too
              // inconsistent to seed safely, so landing on a blank
              // artboard is the right recovery surface.
              const target = artboards.find(
                (a) => a.id === createdArtboardId,
              );
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
  const handleBriefApplied = useCallback(
    async (result: BriefApplyResult | ThemedDesignApplyResult) => {
      try {
        const project = await window.kcreate.document.getProjectInfo();
        if (!project) {
          // Defensive: brief_to_project would have failed if no
          // project was open, so reaching here means the workspace
          // was closed between the apply call returning and this
          // callback firing. Surface a clear error.
          setRoute({
            kind: "error",
            message: t("app.error.briefProjectClosed"),
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
    },
    [t],
  );

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

  // "Start from template" / "Duplicate & remix" — the G2 jump-start
  // entry point. Mirrors `handleOpenEditor`'s shape: materialise a
  // scratch project, pour the template's design into it via the
  // bridge (`templateMarketplace.instantiate`, which creates an
  // artboard + all of the template's nodes from its `content.json`),
  // optionally duplicate that artboard so a "remix" edits a copy,
  // push the new nodes into the renderer scene, then route into the
  // editor on the populated canvas.
  //
  // Unlike `handleOpenEditor`, failures here are NOT swallowed: this
  // is a deliberate, explicit user action on a specific template, so
  // a failure should surface in the gallery (inline, no navigation)
  // rather than dumping the user onto a blank editor. We close the
  // half-open scratch project on failure to avoid leaking an empty
  // workspace, then rethrow so the gallery can show the error and
  // stay put.
  const handleStartFromTemplate = useCallback(
    async (templateId: string, opts: { remix: boolean }) => {
      const project = await openScratchProject();
      try {
        const report =
          await window.kcreate.templateMarketplace.instantiate(templateId);
        if (opts.remix) {
          // Remix = jump-start from a *copy* so the pristine
          // instantiation stays untouched as a reference. The bridge
          // offsets the duplicate to its own artboard slot.
          await window.kcreate.artboard.duplicate(report.artboardId);
        }
        // Push the freshly created nodes into the renderer's scene so
        // the editor opens on the populated canvas instead of waiting
        // for the next event-driven sync.
        await window.kcreate.canvas.syncScene();
      } catch (e) {
        // Roll back the scratch project we opened above so a failed
        // instantiation never strands an empty workspace. Swallow any
        // close error — the original instantiation error is the one
        // worth surfacing.
        await window.kcreate.document.closeProject().catch(() => {});
        throw e;
      }
      setRoute({ kind: "editor", project });
    },
    [],
  );

  if (route.kind === "editor") {
    return (
      <ErrorBoundary
        fallback={(error, reset) => (
          <EditorLoadError
            message={error.message}
            onReload={() => window.location.reload()}
            onBackHome={() => {
              reset();
              handleBackHome();
            }}
          />
        )}
      >
        <Suspense fallback={<EditorLoading />}>
          <EditorPage project={route.project} onBackHome={handleBackHome} />
        </Suspense>
      </ErrorBoundary>
    );
  }
  if (route.kind === "templates") {
    return (
      <TemplateGallery
        onBack={() => setRoute({ kind: "home" })}
        onStartFromTemplate={handleStartFromTemplate}
      />
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
        role="alert"
      >
        {t("app.error.openProject", { message: route.message })}
      </div>
    );
  }
  return (
    <HomePage
      onOpenEditor={handleOpenEditor}
      onOpenProject={(p) => void handleOpenProject(p)}
      onBriefApplied={(r) => void handleBriefApplied(r)}
      onBrowseTemplates={() => setRoute({ kind: "templates" })}
    />
  );
}

