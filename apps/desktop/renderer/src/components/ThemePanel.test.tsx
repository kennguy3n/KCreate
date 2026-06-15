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
});
