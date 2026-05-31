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

const defaultPrefs = {
  autosaveSeconds: 60,
  confirmDelete: true,
  snapEnabled: true,
  smartGuidesEnabled: true,
  canvasBackground: "checker",
  keyboardLayout: "qwerty",
  accent: "#3B82F6",
  theme: "light",
  locale: "en-US",
  units: "px",
  showRulers: true,
  showGrid: false,
  gridSize: 8,
  nudgeSmall: 1,
  nudgeLarge: 10,
  export: {
    lastDirByFormat: {},
    lastBatchDir: null,
  },
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
  "export.png": () => 0,
  "export.svg": () => "",
  "export.pdf": () => 0,
  "export.webp": () => 0,
  "export.jpeg": () => 0,
  "artboard.create": () => "00000000-0000-0000-0000-000000000000",
  "document.projectOpen": () => null,
  setLayerColor: () => undefined,
};

export function installKcreateStub(): KcreateStubHandle {
  const calls: KcreateStubCall[] = [];
  const overrides = new Map<string, KcreateStubResolver>();

  const recordCall = (method: string, args: unknown[]): unknown => {
    calls.push({ method, args });
    const override = overrides.get(method);
    if (override !== undefined) {
      return Promise.resolve(override(...args));
    }
    const defaultResolver = defaultsByMethod[method];
    const value = defaultResolver === undefined ? undefined : defaultResolver();
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
    text: namespace("text"),
    project: namespace("project"),
    audit: namespace("audit"),
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
