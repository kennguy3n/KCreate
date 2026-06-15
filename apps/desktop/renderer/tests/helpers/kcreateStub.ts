// Minimal `window.kcreate` stub for vitest component tests.
//
// The real preload bridge lives in `apps/desktop/preload/src/preload.ts`
// and exposes hundreds of methods spanning several namespaces. Wiring
// up the entire surface for every renderer test would be impossibly
// brittle, so this helper installs a recording stub that:
//
//   * resolves every method to a sane default (empty array, null,
//     resolved promise, …) so component mounts don't throw;
//   * records every invocation in a per-test `calls[]` log so tests
//     can assert "the Export button called runtime.chooseExportTarget
//     with these args";
//   * lets a test override any individual method with a custom
//     resolver to inject behaviour-specific responses (e.g.
//     `handle.override("runtime.chooseExportTarget", () =>
//     "/home/user/Pictures/foo.png")`).
//
// The stub is reinstalled in `beforeEach` so call logs and overrides
// don't bleed across tests.

import type {
  ApplyThemeReport,
  Preferences,
  Theme,
} from "../../../shared/scene";

export interface KcreateStubCall {
  /// Dot-path of the method, e.g. "runtime.status", "export.png".
  method: string;
  args: ReadonlyArray<unknown>;
}

export type KcreateStubResolver = (...args: unknown[]) => unknown;

export interface KcreateStubHandle {
  calls: KcreateStubCall[];
  /// Install a custom resolver for a single method (dot path, e.g.
  /// "phase10.preferencesLoad"). The resolver's return value is
  /// wrapped in `Promise.resolve` automatically, matching the
  /// production async-bridge contract.
  override(method: string, resolver: KcreateStubResolver): void;
}

let handle: KcreateStubHandle | null = null;

/// Returns the active stub handle for the current test. Throws if
/// called outside a `beforeEach` block that ran `installKcreateStub`.
export function kcreateStub(): KcreateStubHandle {
  if (handle === null) {
    throw new Error(
      "kcreateStub() called before installKcreateStub() — make sure setup.vitest.ts is registered as a setupFile.",
    );
  }
  return handle;
}

// Mirror of `Preferences` from `apps/desktop/shared/scene.ts`
// (which in turn mirrors `kcreate_bridge::phase10::Preferences`).
// Tests reach into nested keys (e.g. `prefs.export.lastDirByFormat`)
// and the production `PreferencesPanel` reads `general.theme`,
// `canvas.snapThresholdPx`, etc. — so the stub default MUST have
// the same shape as production or the first render under the test
// harness throws on `undefined.<nestedKey>`. Drift here has bitten
// us before; keep this in lockstep with the `Preferences` interface
// in `apps/desktop/shared/scene.ts:5677`.
//
// Typed via the imported interface so a future field addition is a
// compile error here, not a silent runtime gap in test land.
const defaultPrefs: Preferences = {
  general: {
    theme: "system",
    language: "en-US",
    autosaveIntervalSec: 60,
    scratchProjectCleanupDays: 7,
  },
  canvas: {
    defaultGridSpacing: 8,
    defaultGridSubdivisions: 4,
    snapThresholdPx: 6,
    rulerUnits: "px",
  },
  ai: {
    defaultLlmModel: "",
    autoStartSidecar: false,
    gbnfGrammarDebugging: false,
  },
  performance: {
    rasterCacheBudgetMb: 256,
    undoDepthOverride: null,
    lowResourceMode: false,
  },
  privacy: {
    telemetryOptIn: false,
    auditLogRetentionDays: 30,
  },
  export: {
    lastDirByFormat: {},
    lastBatchDir: null,
  },
  // Phase C — default to `completed=true` in test land so any
  // component that mounts the renderer under the stub does not
  // accidentally render the welcome modal and steal focus from
  // the assertion target. Tests that exercise the welcome modal
  // override this explicitly via the per-method `phase10.preferencesLoad`
  // override map below.
  onboarding: {
    completed: true,
    lastSeenPackId: null,
  },
};

// G4 — Theme / Brand Kit. A complete, valid default `Theme` so any
// test that renders `ThemePanel` and triggers `theme.deriveFromDocument`
// or `theme.fromBrandKit` without an explicit override receives a
// well-formed theme (with all seven palette roles + type/spacing/radii
// scales) rather than `undefined`. Typed via the imported interface so
// a future field addition is a compile error here, not a silent runtime
// gap. Sub-structs are snake_case to mirror `kcreate_core::theme`'s
// default serde, matching the `Theme` wire shape in `shared/scene.ts`.
const defaultTheme: Theme = {
  id: "stub-theme",
  name: "Stub theme",
  palette: {
    background: { r: 1, g: 1, b: 1, a: 1 },
    surface: { r: 0.95, g: 0.96, b: 0.98, a: 1 },
    primary: { r: 0.15, g: 0.39, b: 0.92, a: 1 },
    secondary: { r: 0.05, g: 0.65, b: 0.91, a: 1 },
    accent: { r: 0.96, g: 0.62, b: 0.04, a: 1 },
    text: { r: 0.06, g: 0.09, b: 0.16, a: 1 },
    muted: { r: 0.39, g: 0.45, b: 0.55, a: 1 },
  },
  type_scale: {
    body_font: "Inter",
    heading_font: "Inter",
    display: 44,
    heading: 28,
    body: 16,
    caption: 12,
    line_height: 1.4,
  },
  spacing: { xs: 4, sm: 8, md: 16, lg: 24, xl: 40 },
  radii: { none: 0, small: 4, medium: 8, large: 16, full: 9999 },
};

// G4 — zeroed `theme.apply` report. `ApplyThemeReport` uses camelCase
// serde, so the keys are camelCase (unlike `Theme`'s sub-structs above).
const defaultApplyThemeReport: ApplyThemeReport = {
  themeId: "stub-theme",
  themeName: "Stub theme",
  affectedNodes: 0,
  recoloredFills: 0,
  recoloredStrokes: 0,
  restyledText: 0,
};

const defaultsByMethod: Record<string, () => unknown> = {
  "runtime.status": () => ({
    version: "test",
    platform: "test",
    arch: "test",
    mode: "cpu",
  }),
  "runtime.tempDir": () => "/tmp",
  "runtime.writeTextFile": () => 0,
  "runtime.chooseExportTarget": () => null,
  "runtime.chooseExportDirectory": () => null,
  "llm.status": () => ({ state: "idle" }),
  "recentProjects.list": () => [],
  "recentProjects.coverBytes": () => null,
  "phase10.preferencesLoad": () => ({ ...defaultPrefs }),
  "phase10.preferencesSave": () => undefined,
  "designTokens.get": () => null,
  // G4 — Theme / Brand Kit. `theme.listBuiltins` and `brandKit.list`
  // default to empty so ThemePanel mounts cleanly; tests that
  // exercise selection / apply override with real fixtures.
  "theme.listBuiltins": () => [],
  // `theme.apply` resolves to a zeroed report; `deriveFromDocument` and
  // `fromBrandKit` resolve to a complete default `Theme`. Without these,
  // a test that renders `ThemePanel` and clicks Apply/Derive without an
  // override would read `undefined.themeName` / `undefined.id`. Spread a
  // fresh copy per call so a test that mutates the result can't bleed
  // into the next.
  "theme.apply": () => ({ ...defaultApplyThemeReport }),
  "theme.deriveFromDocument": () => ({ ...defaultTheme }),
  "theme.fromBrandKit": () => ({ ...defaultTheme }),
  "brandKit.list": () => [],
  "brandKit.create": () => "00000000-0000-0000-0000-000000000000",
  "brandKit.update": () => undefined,
  "brandKit.delete": () => true,
  "document.saveProject": () => undefined,
  "export.png": () => 0,
  "export.svg": () => "",
  "export.pdf": () => 0,
  "export.webp": () => 0,
  "export.jpeg": () => 0,
  "artboard.create": () => "00000000-0000-0000-0000-000000000000",
  "artboard.list": () => [],
  "artboard.presets": () => [],
  "component.list": () => [],
  "document.projectOpen": () => null,
  "document.status": () => null,
  "document.getDocumentTree": () => [],
  // `canvasSnap.query` returns `null` by default — no snap. Tests
  // that exercise the snap path override this. The bridge contract
  // is `(nodeId, x, y, w, h, threshold) => SnapResult | null`, where
  // `SnapResult = { dx, dy, guides }`.
  "canvasSnap.query": () => null,
  // `canvas.hitTest` returns null by default (miss). The state
  // machine's select-tool branch routes through here.
  "canvas.hitTest": () => null,
  "canvas.setSelection": () => undefined,
  "canvas.clearSelection": () => undefined,
  "canvas.moveNode": () => undefined,
  "canvas.createRect": () => "default-rect-id",
  "canvas.createEllipse": () => "default-ellipse-id",
  "canvas.createLine": () => "default-line-id",
  // Phase B1 — pen tool. `createPath(parentId, segmentsJson, closed,
  // name) => string` matches the preload entry shape; stub returns
  // a stable default id so tests exercising the pen state machine
  // can assert on the bridge call's call list + `setSelection`
  // follow-up without wiring a custom override every time.
  "canvas.createPath": () => "default-path-id",
  // Phase B2 — Pathfinder boolean. `pathBoolean(op, sourceIds) =>
  // string[]` mirrors the preload entry shape; stub returns a
  // single deterministic result id so tests for `PathfinderPanel`
  // can pin both the call list (op + sourceIds passed verbatim)
  // and the re-selection follow-up without per-test wiring.
  "canvas.pathBoolean": () => ["default-bool-result-id"],
  // Phase B3 — Node editor read/write surface.
  // `pathGetSegments(nodeId) => PathSnapshot` returns a tiny
  // valid square path so tests can exercise the node-edit entry
  // path (anchor population, overlay render, hit-test) without
  // wiring a custom override every time. Per-test overrides
  // still apply for the edge cases (missing node, non-vector,
  // etc.). Coordinates and `closed: true` match a 100x100 square
  // at the origin; `fillRule: "non_zero"` is the default.
  "canvas.pathGetSegments": () => ({
    segments: [
      { op: "move_to", x: 0, y: 0 },
      { op: "line_to", x: 100, y: 0 },
      { op: "line_to", x: 100, y: 100 },
      { op: "line_to", x: 0, y: 100 },
      { op: "close" },
    ],
    closed: true,
    fillRule: "non_zero",
    translationX: 0,
    translationY: 0,
  }),
  // `pathSetSegments(nodeId, segments, closed) => void` returns
  // undefined on success; tests asserting failure modes override
  // with a throwing function.
  "canvas.pathSetSegments": () => undefined,
  "canvas.createText": () => "default-text-id",
  // Phase C — runtime tier surface for the welcome modal. Returns
  // a neutral mid-tier so existing tests that previously didn't
  // touch this method keep working; welcome-modal tests override
  // this explicitly per case.
  //
  // Field names mirror the on-wire `ResourceLimits` interface in
  // `apps/desktop/shared/scene.ts:745-782` exactly (`visionModelMaxMb`,
  // not `effectiveMaxVisionModelMb`, and the required `platform`
  // string is present). A previous iteration used the wrong vision
  // field name and omitted `platform` — undetected because the
  // stub return type was `unknown` and the only renderer caller in
  // the welcome modal (`tierLabel`) reads `deviceTier` only. Any
  // future test that asserts on `visionModelMaxMb` would have
  // silently read `undefined` against the old shape. Bot-flagged
  // ANALYSIS_0003.
  "runtime.resourceLimits": () => ({
    deviceTier: "1",
    lowResourceMode: false,
    effectiveUndoDepth: 50,
    effectiveRasterCacheMb: 256,
    effectiveMaxModelMb: 4096,
    gpuRenderingAllowed: true,
    imageGenerationAllowed: false,
    visionModelMaxMb: 256,
    platform: "Linux",
  }),
  // Phase C — recommended LLM pack id surfaced via the bridge.
  // Defaults to the 1.7B Bonsai pack (matches the tier-1 default
  // above). Welcome-modal tests override to drive each branch.
  "llm.recommendedPack": () => "llm_bonsai_1_7b",
  // Phase C — full model pack catalog used by the welcome modal
  // to look up display name + size for the recommended pack id.
  "aiModel.listModelPacks": () => [
    {
      id: "llm_bonsai_1_7b",
      name: "Ternary-Bonsai 1.7B (Q2_0 GGUF)",
      kind: "sidecar",
      category: "core",
      capabilities: ["chat"],
      sizeBytes: 750_000_000,
      sha256: "",
      filePath: "",
      installed: false,
      downloadUrl: "https://huggingface.co/example/llm_bonsai_1_7b.gguf",
    },
  ],
  // Phase C — manual-install fallback. Returns null by default so
  // a welcome-modal test that exercises "I have the file" without
  // overriding gets the same cancel-path semantics as
  // ModelManager. Tests that drive the success branch override.
  "aiModel.pickModelFile": () => null,
  // Phase C — manual installer used by both ModelManager and the
  // "I have the file" fallback in the welcome modal. Default
  // mirrors the verified-install happy path with a small payload.
  "aiModel.installModelPack": () => ({
    packId: "llm_bonsai_1_7b",
    verified: true,
    actualSha256: "0".repeat(64),
    sizeBytes: 750_000_000,
  }),
  // Phase C — one-click install. Default mirrors a verified
  // download + install happy path. Tests that drive the error
  // branch override with a throwing resolver.
  // Field naming is camelCase to match the on-wire JSON keys
  // emitted by `kcreate_ai::InstallReport` (see
  // `install_report_serialises_to_camelcase_wire_format` in
  // `crates/kcreate_ai/src/model_registry.rs`). The renderer-side
  // `OnboardingInstallReport` and main-process validation both
  // read the camelCase keys directly — a previous stub iteration
  // used snake_case (`pack_id`, `actual_sha256`, `size_bytes`)
  // which was wrong but went undetected because the same wrong
  // interface was declared on both ends.
  "onboarding.installRecommendedPack": () => ({
    packId: "llm_bonsai_1_7b",
    verified: true,
    actualSha256: "0".repeat(64),
    sizeBytes: 750_000_000,
  }),
  // Phase C — idempotent cancel. Welcome modal calls this on
  // unmount even when no install is in flight, so the default
  // must be a no-op.
  "onboarding.cancelInstall": () => undefined,
  // Phase C — progress subscription. The default returns a
  // no-op unsubscribe so tests that don't drive progress events
  // still mount cleanly. Tests that drive progress override with
  // a resolver that captures the listener and returns a real
  // unsubscribe function.
  "onboarding.onInstallProgress": () => (): void => undefined,
  // Phase C — narrow system surface used by the welcome modal's
  // "Open download page" fallback. Main-process validation against
  // the host allow-list is the real defence; the stub just records.
  "system.openExternal": () => undefined,
  // G6 — Elements / asset library. The catalog is static, so the
  // defaults return an empty set; AssetsPanel tests override these
  // with a fixture catalog to assert search / insert behaviour.
  "assets.categories": () => [],
  "assets.list": () => [],
  "assets.search": () => [],
  "assets.insert": () => ({
    groupId: "group-0",
    nodeIds: ["node-0"],
    name: "asset",
    x: 0,
    y: 0,
    width: 0,
    height: 0,
  }),
  // Workstream G2 — ready-made template library. `TemplateGallery`
  // mounts straight into `list` + `thumbnail`; defaults keep the
  // component mountable (empty gallery, fallback thumbnails) so tests
  // that don't drive the catalog stay green. Catalog-driving tests
  // override `templateMarketplace.list` to return a `TemplateListReport`
  // and `templateMarketplace.thumbnail` to return `ThumbnailBytes`.
  "templateMarketplace.list": () => ({ templates: [] }),
  "templateMarketplace.thumbnail": () => null,
  "templateMarketplace.instantiate": () => ({
    artboardId: "00000000-0000-0000-0000-000000000000",
    nodeIds: [],
  }),
  // `installLocal` resolves to the installed `TemplateManifest` (not a
  // list) — mirror that shape so a test that doesn't override it still
  // type-checks against the real bridge contract. Snake_case fields
  // match the serde JSON wire format (this is a JSON-string type, not a
  // `#[napi(object)]`).
  "templateMarketplace.installLocal": () => ({
    id: "00000000-0000-0000-0000-000000000000",
    name: "",
    description: "",
    category: "custom",
    tags: [],
    thumbnail: null,
    page_count: 0,
    author: null,
    version: "0.0.0",
    source: null,
  }),
  "templateMarketplace.remove": () => undefined,
  setLayerColor: () => undefined,
};

/**
 * Methods that the production preload exposes as **synchronous**
 * (return the value directly, NOT a Promise). The stub must
 * preserve this so callers can use the returned value (e.g. an
 * unsubscribe function) without unwrapping a Promise. Add new
 * entries here when wiring a new sync bridge method that the
 * tests need to observe.
 */
const SYNC_METHODS = new Set<string>([
  // Phase C — `onboarding.onInstallProgress(fn) => unsubscribe`.
  // The unsubscribe handle MUST be a callable, not a Promise, so
  // React's useEffect cleanup can invoke it on unmount. Wrapping
  // in Promise.resolve makes the cleanup call a Promise and
  // throws `destroy is not a function`.
  "onboarding.onInstallProgress",
]);

export function installKcreateStub(): KcreateStubHandle {
  const calls: KcreateStubCall[] = [];
  const overrides = new Map<string, KcreateStubResolver>();

  const recordCall = (method: string, args: unknown[]): unknown => {
    calls.push({ method, args });
    const override = overrides.get(method);
    const value =
      override !== undefined
        ? override(...args)
        : (() => {
            const def = defaultsByMethod[method];
            return def === undefined ? undefined : def();
          })();
    if (SYNC_METHODS.has(method)) {
      return value;
    }
    return Promise.resolve(value);
  };

  const namespace = (prefix: string): unknown =>
    new Proxy(
      {},
      {
        get: (_target, prop: string | symbol): unknown => {
          if (typeof prop !== "string") return undefined;
          const path = `${prefix}.${prop}`;
          return (...args: unknown[]): unknown => recordCall(path, args);
        },
      },
    );

  const stub = {
    runtime: namespace("runtime"),
    llm: namespace("llm"),
    recentProjects: namespace("recentProjects"),
    phase10: namespace("phase10"),
    designTokens: namespace("designTokens"),
    export: namespace("export"),
    artboard: namespace("artboard"),
    document: namespace("document"),
    canvas: namespace("canvas"),
    canvasSnap: namespace("canvasSnap"),
    text: namespace("text"),
    // G4 — Theme / Brand Kit instant restyle.
    theme: namespace("theme"),
    brandKit: namespace("brandKit"),
    project: namespace("project"),
    audit: namespace("audit"),
    component: namespace("component"),
    // Phase C — model manager + welcome modal surfaces.
    aiModel: namespace("aiModel"),
    onboarding: namespace("onboarding"),
    system: namespace("system"),
    assets: namespace("assets"),
    // Workstream G2 — ready-made template library bridge.
    templateMarketplace: namespace("templateMarketplace"),
    setLayerColor: (...args: unknown[]): unknown =>
      recordCall("setLayerColor", args),
  };

  Object.defineProperty(window, "kcreate", {
    value: stub,
    writable: true,
    configurable: true,
  });

  handle = {
    calls,
    override(method, resolver) {
      overrides.set(method, resolver);
    },
  };
  return handle;
}
