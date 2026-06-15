// ThemePanel tests (G4 — Theme / Brand Kit instant restyle).
//
// Pins the two contract-level behaviours from the workstream spec:
//   * select a built-in theme → "Apply" calls `theme.apply` with the
//     selected theme (the whole-document restyle entry point), and the
//     host `onApplied` refresh callback fires;
//   * author a custom brand kit → "New brand kit" persists through the
//     canonical `brandKit.create` + `document.saveProject` surface, and
//     "Save" round-trips the edited draft through `brandKit.update`.
//
// Bridge calls are recorded by the session-wide stub installed in
// `setup.vitest.ts`; `theme.*` / `brandKit.*` namespaces were added to
// the stub for this panel.

import { describe, it, expect } from "vitest";
import { render, screen, fireEvent, act } from "@testing-library/react";

import { ThemePanel } from "./ThemePanel";
import { kcreateStub } from "../../tests/helpers/kcreateStub";
import type {
  ApplyThemeReport,
  BrandKit,
  RgbaColor,
  Theme,
} from "../../../shared/scene";

async function flushAsync(): Promise<void> {
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
  });
}

// Drains both the macrotask queue (a `setTimeout(0)`) and microtasks.
// The file-pick flows chain `File.arrayBuffer()` (a promise) and a
// bridge call, so a deeper settle than `flushAsync` keeps them
// deterministic under jsdom.
async function settle(): Promise<void> {
  await act(async () => {
    await new Promise((r) => setTimeout(r, 0));
    await Promise.resolve();
    await Promise.resolve();
  });
}

// Intercept the next dynamically-created `<input>` (the transient file
// picker `ThemePanel.pickFileBytes` builds) and synthesize a selection
// of `bytes` named `name`, without opening a real OS dialog. One-shot:
// it restores `document.createElement` as soon as it has patched the
// first input, so React's own inputs are untouched. Call AFTER the
// initial render so the panel's own inputs don't consume the shot.
function mockFilePick(name: string, bytes: Uint8Array): () => void {
  const realCreate = document.createElement.bind(document);
  let restored = false;
  const restore = (): void => {
    if (restored) return;
    restored = true;
    document.createElement = realCreate;
  };
  document.createElement = ((tag: string): HTMLElement => {
    const el = realCreate(tag);
    if (tag === "input") {
      const input = el as HTMLInputElement;
      input.click = (): void => {
        const file = new File([bytes], name, { type: "image/png" });
        // jsdom's `Blob.arrayBuffer` is unreliable under the test env, so
        // pin a deterministic promise that yields exactly `bytes`.
        Object.defineProperty(file, "arrayBuffer", {
          value: (): Promise<ArrayBuffer> =>
            Promise.resolve(
              bytes.buffer.slice(
                bytes.byteOffset,
                bytes.byteOffset + bytes.byteLength,
              ),
            ),
          configurable: true,
        });
        Object.defineProperty(input, "files", {
          value: [file],
          configurable: true,
        });
        input.dispatchEvent(new Event("change"));
      };
      restore();
    }
    return el;
  }) as typeof document.createElement;
  return restore;
}

function kitWithLogo(id: string, name: string, logoAssetId: string): BrandKit {
  return { ...emptyKit(id, name), logo_asset_id: logoAssetId };
}

function rgba(r: number, g: number, b: number): RgbaColor {
  return { r, g, b, a: 1 };
}

function makeTheme(id: string, name: string): Theme {
  return {
    id,
    name,
    palette: {
      background: rgba(1, 1, 1),
      surface: rgba(0.95, 0.95, 0.95),
      primary: rgba(0.15, 0.39, 0.92),
      secondary: rgba(0.45, 0.2, 0.8),
      accent: rgba(0.95, 0.6, 0.1),
      text: rgba(0.05, 0.05, 0.05),
      muted: rgba(0.5, 0.5, 0.5),
    },
    type_scale: {
      body_font: "Inter",
      heading_font: "Inter",
      display: 40,
      heading: 24,
      body: 16,
      caption: 12,
      line_height: 1.4,
    },
    spacing: { xs: 4, sm: 8, md: 16, lg: 24, xl: 32 },
    radii: { none: 0, small: 4, medium: 8, large: 16, full: 9999 },
  };
}

const REPORT: ApplyThemeReport = {
  themeId: "midnight",
  themeName: "Midnight",
  affectedNodes: 5,
  recoloredFills: 4,
  recoloredStrokes: 1,
  restyledText: 2,
};

function emptyKit(id: string, name: string): BrandKit {
  return {
    id,
    name,
    logo_asset_id: null,
    colors: [],
    fonts: [],
    spacing_scale: [],
    export_rules: [],
  };
}

describe("ThemePanel", () => {
  it("applies the selected built-in theme and notifies the host", async () => {
    const stub = kcreateStub();
    const themeA = makeTheme("daybreak", "Daybreak");
    const themeB = makeTheme("midnight", "Midnight");
    stub.override("theme.listBuiltins", () => [themeA, themeB]);
    stub.override("theme.apply", () => REPORT);

    let applied = 0;
    render(<ThemePanel onApplied={() => (applied += 1)} />);
    await flushAsync();

    // Pick the second theme, then Apply.
    fireEvent.click(screen.getByLabelText("Select theme Midnight"));
    fireEvent.click(screen.getByLabelText("Apply theme"));
    await flushAsync();

    const applyCall = stub.calls.find((c) => c.method === "theme.apply");
    expect(applyCall, "Apply should call theme.apply").toBeDefined();
    expect((applyCall?.args[0] as Theme).id).toBe("midnight");
    expect(applied, "onApplied should fire after a successful apply").toBe(1);
  });

  it("derives a theme from the open document", async () => {
    const stub = kcreateStub();
    stub.override("theme.listBuiltins", () => [makeTheme("daybreak", "Daybreak")]);
    stub.override("theme.deriveFromDocument", () =>
      makeTheme("derived-foo", "From design"),
    );

    render(<ThemePanel />);
    await flushAsync();

    fireEvent.click(screen.getByLabelText("Derive theme from document"));
    await flushAsync();

    const deriveCall = stub.calls.find(
      (c) => c.method === "theme.deriveFromDocument",
    );
    expect(deriveCall, "Derive should call theme.deriveFromDocument").toBeDefined();
    // The derived theme is added to the list and auto-selected.
    expect(screen.getByLabelText("Select theme From design")).toBeInTheDocument();
  });

  it("creates and persists a custom brand kit", async () => {
    const stub = kcreateStub();
    let kits: BrandKit[] = [];
    stub.override("brandKit.list", () => kits);
    stub.override("brandKit.create", () => {
      kits = [emptyKit("kit-1", "Brand kit 1")];
      return "kit-1";
    });

    render(<ThemePanel />);
    await flushAsync();

    fireEvent.click(screen.getByLabelText("New brand kit"));
    await flushAsync();

    expect(
      stub.calls.some((c) => c.method === "brandKit.create"),
      "New kit should call brandKit.create",
    ).toBe(true);
    expect(
      stub.calls.some((c) => c.method === "document.saveProject"),
      "Create should persist via document.saveProject",
    ).toBe(true);

    // The editor opens on the new kit; saving round-trips the draft.
    expect(screen.getByLabelText("Brand kit name")).toBeInTheDocument();
    fireEvent.click(screen.getByLabelText("Save brand kit"));
    await flushAsync();

    const updateCall = stub.calls.find((c) => c.method === "brandKit.update");
    expect(updateCall, "Save should call brandKit.update").toBeDefined();
    expect((updateCall?.args[0] as BrandKit).id).toBe("kit-1");
  });

  it("applies a brand kit as a theme with the kit-specific status", async () => {
    const stub = kcreateStub();
    const kit = emptyKit("kit-7", "Acme Brand");
    const kitTheme = makeTheme("kit-theme", "Kit Theme");
    stub.override("brandKit.list", () => [kit]);
    stub.override("theme.fromBrandKit", () => kitTheme);
    stub.override("theme.apply", () => REPORT);

    const statuses: (string | null)[] = [];
    let applied = 0;
    render(
      <ThemePanel
        onStatus={(m) => statuses.push(m)}
        onApplied={() => (applied += 1)}
      />,
    );
    await flushAsync();

    fireEvent.click(screen.getByLabelText("Apply Acme Brand"));
    await flushAsync();

    // Composed path: derive a theme from the kit, then apply it.
    const fromKit = stub.calls.find((c) => c.method === "theme.fromBrandKit");
    expect(fromKit, "should derive a theme from the kit").toBeDefined();
    expect((fromKit?.args[0] as BrandKit).id).toBe("kit-7");
    const applyCall = stub.calls.find((c) => c.method === "theme.apply");
    expect(applyCall, "should apply the kit-derived theme").toBeDefined();
    expect((applyCall?.args[0] as Theme).id).toBe("kit-theme");
    expect(applied, "onApplied fires after a successful kit apply").toBe(1);

    // The kit-specific status is surfaced and is NOT clobbered by the
    // generic per-theme "applying" label (the statusLabel pass-through).
    expect(statuses).toContain("Theme: applying brand kit “Acme Brand”…");
    expect(statuses).not.toContain("Theme: applying “Kit Theme”…");

    // `finally { setBusy(false) }` re-enables the control after success.
    expect(screen.getByLabelText("Apply Acme Brand")).not.toBeDisabled();
  });

  it("resets busy when applying a brand kit fails", async () => {
    const stub = kcreateStub();
    const kit = emptyKit("kit-9", "Broken Kit");
    stub.override("brandKit.list", () => [kit]);
    stub.override("theme.fromBrandKit", () => {
      throw new Error("derive boom");
    });

    const statuses: (string | null)[] = [];
    render(<ThemePanel onStatus={(m) => statuses.push(m)} />);
    await flushAsync();

    fireEvent.click(screen.getByLabelText("Apply Broken Kit"));
    await flushAsync();

    expect(
      statuses.some((s) => s?.startsWith("Apply kit failed:")),
      "failure surfaces an apply-kit error status",
    ).toBe(true);
    // The `finally` re-enables the control even on the failure path.
    expect(screen.getByLabelText("Apply Broken Kit")).not.toBeDisabled();
  });

  it("surfaces the kit-specific label when the apply step (not derive) fails", async () => {
    // Regression for the composed apply: the derive succeeds but the
    // bridge `theme.apply` throws. Because `handleApplyKit` composes the
    // non-lifecycle-managing core directly, the failure propagates to
    // its own catch and is reported as an "Apply kit failed" message —
    // not the generic "Theme apply failed" that the standalone path
    // uses. This pins the fix for the swallowed-error review note.
    const stub = kcreateStub();
    const kit = emptyKit("kit-11", "Flaky Kit");
    stub.override("brandKit.list", () => [kit]);
    stub.override("theme.fromBrandKit", () => makeTheme("kit-theme", "Kit Theme"));
    stub.override("theme.apply", () => {
      throw new Error("apply boom");
    });

    const statuses: (string | null)[] = [];
    render(<ThemePanel onStatus={(m) => statuses.push(m)} />);
    await flushAsync();

    fireEvent.click(screen.getByLabelText("Apply Flaky Kit"));
    await flushAsync();

    // The apply failure is attributed to the kit operation, and the
    // generic per-theme failure label is NOT used.
    expect(
      statuses.some((s) => s?.startsWith("Apply kit failed:")),
      "apply-step failure surfaces the kit-specific label",
    ).toBe(true);
    expect(
      statuses.some((s) => s?.startsWith("Theme apply failed:")),
      "the generic per-theme failure label is not used for a kit apply",
    ).toBe(false);
    expect(screen.getByLabelText("Apply Flaky Kit")).not.toBeDisabled();
  });

  // --- H5: apply-to-selection scope --------------------------------------

  it("routes Apply through applyToSelection when the scope is the selection", async () => {
    const stub = kcreateStub();
    const themeA = makeTheme("daybreak", "Daybreak");
    stub.override("theme.listBuiltins", () => [themeA]);
    stub.override("theme.apply", () => REPORT);
    stub.override("theme.applyToSelection", () => REPORT);

    render(<ThemePanel selectedIds={["node-1", "node-2"]} />);
    await flushAsync();

    // Switch the scope toggle to "Selection (2)", then Apply.
    fireEvent.click(screen.getByRole("radio", { name: "Selection (2)" }));
    fireEvent.click(screen.getByLabelText("Apply theme"));
    await flushAsync();

    const sel = stub.calls.find((c) => c.method === "theme.applyToSelection");
    expect(sel, "selection scope must call theme.applyToSelection").toBeDefined();
    expect((sel?.args[0] as Theme).id).toBe("daybreak");
    expect(sel?.args[1], "the current selection ids are forwarded").toEqual([
      "node-1",
      "node-2",
    ]);
    // The whole-document path must NOT be taken in selection scope.
    expect(stub.calls.some((c) => c.method === "theme.apply")).toBe(false);
  });

  it("uses the whole-document path in document scope (the default)", async () => {
    const stub = kcreateStub();
    stub.override("theme.listBuiltins", () => [makeTheme("daybreak", "Daybreak")]);
    stub.override("theme.apply", () => REPORT);
    stub.override("theme.applyToSelection", () => REPORT);

    render(<ThemePanel selectedIds={["node-1"]} />);
    await flushAsync();

    // No scope change — document is the default.
    fireEvent.click(screen.getByLabelText("Apply theme"));
    await flushAsync();

    expect(stub.calls.some((c) => c.method === "theme.apply")).toBe(true);
    expect(stub.calls.some((c) => c.method === "theme.applyToSelection")).toBe(
      false,
    );
  });

  it("blocks selection-scope apply when nothing is selected", async () => {
    const stub = kcreateStub();
    stub.override("theme.listBuiltins", () => [makeTheme("daybreak", "Daybreak")]);

    render(<ThemePanel selectedIds={[]} />);
    await flushAsync();

    fireEvent.click(screen.getByRole("radio", { name: "Selection (0)" }));

    // Apply is disabled with an empty selection, and clicking is a no-op.
    expect(screen.getByLabelText("Apply theme")).toBeDisabled();
    fireEvent.click(screen.getByLabelText("Apply theme"));
    await flushAsync();
    expect(stub.calls.some((c) => c.method === "theme.applyToSelection")).toBe(
      false,
    );
    expect(stub.calls.some((c) => c.method === "theme.apply")).toBe(false);
  });

  it("blocks brand-kit apply when selection scope has nothing selected", async () => {
    // Regression for the review note: the per-kit "Apply" button routes
    // through the same scope-aware path as the main "Apply theme" button,
    // so it must be disabled (and a click a no-op) when selection scope is
    // active with an empty selection — otherwise the kit apply silently
    // produces a zero-node no-op ("Applied … 0 nodes") with no feedback.
    const stub = kcreateStub();
    const kit = emptyKit("kit-13", "Scoped Kit");
    stub.override("brandKit.list", () => [kit]);
    stub.override("theme.fromBrandKit", () =>
      makeTheme("kit-theme", "Kit Theme"),
    );
    stub.override("theme.applyToSelection", () => REPORT);

    const statuses: (string | null)[] = [];
    render(<ThemePanel selectedIds={[]} onStatus={(m) => statuses.push(m)} />);
    await flushAsync();

    fireEvent.click(screen.getByRole("radio", { name: "Selection (0)" }));

    const applyKit = screen.getByLabelText("Apply Scoped Kit");
    expect(
      applyKit,
      "the per-kit Apply button is disabled in blocked selection scope",
    ).toBeDisabled();
    fireEvent.click(applyKit);
    await flushAsync();

    // Neither the derive nor the selection apply should have run, and no
    // zero-node "Applied …" status should have been surfaced.
    expect(stub.calls.some((c) => c.method === "theme.fromBrandKit")).toBe(
      false,
    );
    expect(stub.calls.some((c) => c.method === "theme.applyToSelection")).toBe(
      false,
    );
    expect(statuses.some((s) => s?.startsWith("Applied "))).toBe(false);
  });

  // --- H5: derive a theme from an uploaded image -------------------------

  it("derives a theme from an uploaded image and selects it", async () => {
    const stub = kcreateStub();
    stub.override("theme.listBuiltins", () => [makeTheme("daybreak", "Daybreak")]);
    stub.override("theme.deriveFromImage", () =>
      makeTheme("derived-sunset-photo", "Sunset photo"),
    );

    render(<ThemePanel />);
    await flushAsync();

    const restore = mockFilePick("sunset.png", new Uint8Array([1, 2, 3, 4]));
    try {
      fireEvent.click(screen.getByLabelText("Derive theme from image"));
      await settle();
    } finally {
      restore();
    }

    const call = stub.calls.find((c) => c.method === "theme.deriveFromImage");
    expect(call, "should call theme.deriveFromImage").toBeDefined();
    expect(call?.args[0], "default image-theme name is forwarded").toBe(
      "Image theme",
    );
    expect(
      call?.args[1] as Uint8Array,
      "the picked image bytes are forwarded",
    ).toBeInstanceOf(Uint8Array);
    expect((call?.args[1] as Uint8Array).length).toBe(4);
    // The derived theme is appended to the list and auto-selected.
    expect(
      screen.getByLabelText("Select theme Sunset photo"),
    ).toBeInTheDocument();
  });

  // --- H5: cross-project brand library (on-disk registry) ----------------

  it("saves an open kit to the cross-project brand library", async () => {
    const stub = kcreateStub();
    stub.override("brandKit.list", () => [emptyKit("kit-3", "Acme")]);

    render(<ThemePanel />);
    await flushAsync();

    fireEvent.click(screen.getByLabelText("Edit Acme"));
    await flushAsync();
    fireEvent.click(screen.getByLabelText("Save brand kit to library"));
    await flushAsync();

    const save = stub.calls.find((c) => c.method === "brandKit.registrySave");
    expect(save, "should persist the kit to the registry").toBeDefined();
    expect(save?.args[0]).toBe("kit-3");
    // The draft + project are flushed before the registry write.
    expect(stub.calls.some((c) => c.method === "brandKit.update")).toBe(true);
    expect(stub.calls.some((c) => c.method === "document.saveProject")).toBe(
      true,
    );
  });

  it("loads a kit from the brand library into the project", async () => {
    const stub = kcreateStub();
    stub.override("brandKit.registryList", () => [
      emptyKit("lib-9", "Studio Brand"),
    ]);
    stub.override("brandKit.registryLoad", () => "new-kit-id");
    stub.override("brandKit.list", () => [emptyKit("new-kit-id", "Studio Brand")]);

    render(<ThemePanel />);
    await flushAsync();

    fireEvent.click(
      screen.getByLabelText("Load Studio Brand into this project"),
    );
    await flushAsync();

    const load = stub.calls.find((c) => c.method === "brandKit.registryLoad");
    expect(load, "should hydrate the kit from the registry").toBeDefined();
    expect(load?.args[0]).toBe("lib-9");
    // The freshly-loaded project kit opens in the editor.
    expect(screen.getByLabelText("Brand kit name")).toBeInTheDocument();
  });

  // --- H5: per-role custom fonts + embedding -----------------------------

  it("sets a heading font through the kit and embeds it by default", async () => {
    const stub = kcreateStub();
    stub.override("brandKit.list", () => [emptyKit("kit-5", "Acme")]);
    stub.override("text.listFonts", () => ["DejaVu Sans", "DejaVu Serif"]);

    render(<ThemePanel />);
    await flushAsync();

    fireEvent.click(screen.getByLabelText("Edit Acme"));
    await flushAsync();
    fireEvent.change(screen.getByLabelText("Heading font"), {
      target: { value: "DejaVu Serif" },
    });
    await flushAsync();

    const call = stub.calls.find((c) => c.method === "brandKit.setFontRole");
    expect(call, "should apply the heading font through the kit").toBeDefined();
    // (kitId, role, family, embed) — embed defaults to true in the panel.
    expect(call?.args).toEqual(["kit-5", "heading", "DejaVu Serif", true]);
  });

  // --- H5: insert the saved logo as an editable node ---------------------

  it("inserts the saved brand logo as an editable node", async () => {
    const stub = kcreateStub();
    stub.override("brandKit.list", () => [
      kitWithLogo("kit-6", "Acme", "asset-logo-1"),
    ]);

    let applied = 0;
    render(<ThemePanel onApplied={() => (applied += 1)} />);
    await flushAsync();

    fireEvent.click(screen.getByLabelText("Edit Acme"));
    await flushAsync();
    fireEvent.click(screen.getByLabelText("Insert brand logo"));
    await flushAsync();

    const call = stub.calls.find((c) => c.method === "brandKit.insertLogo");
    expect(call, "should insert the logo via brandKit.insertLogo").toBeDefined();
    expect(call?.args[0]).toBe("kit-6");
    // A successful insert refreshes the canvas through onApplied.
    expect(applied).toBe(1);
  });
});
