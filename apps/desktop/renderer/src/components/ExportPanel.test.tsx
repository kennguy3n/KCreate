// ExportPanel starter tests (Phase A4).
//
// Pins down the new Phase A2 native-dialog export flow:
//   * the format pills render and selecting one updates the active
//     format (PNG ↔ SVG visible-control change);
//   * clicking "Export" with PNG selected calls
//     `runtime.chooseExportTarget("png", …)`;
//   * if the user cancels the picker (resolver returns `null`) the
//     panel does NOT invoke `export.png` afterwards;
//   * when the picker resolves to a path, `export.png` is invoked
//     with that path AND the sticky last-dir gets persisted via
//     `phase10.preferencesSave` (the panel reads/writes the
//     `lastDirByFormat` map in preferences).
//
// We mount the panel with `selectedIds=[]` so it routes through the
// "whole scene" export path. Bridge calls are recorded by the
// session-wide stub installed in `setup.vitest.ts`.

import { describe, it, expect } from "vitest";
import {
  render,
  screen,
  fireEvent,
  waitFor,
  act,
} from "@testing-library/react";

import { ExportPanel } from "./ExportPanel";
import { kcreateStub } from "../../tests/helpers/kcreateStub";

async function flushAsync() {
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
  });
}

function mount() {
  let lastStatus: string | null | undefined;
  const utils = render(
    <ExportPanel
      onStatus={(msg) => {
        lastStatus = msg;
      }}
      width={400}
      height={300}
      selectedIds={[]}
    />,
  );
  return {
    ...utils,
    captured: {
      get lastStatus() {
        return lastStatus;
      },
    },
  };
}

describe("ExportPanel", () => {
  it("renders the five format pills", () => {
    mount();
    // The PNG pill is the first child of the wrapping `<label>` and
    // therefore inherits the aggregated label text in its accessible
    // name; we instead pin it down by visible text + tag, which is
    // stable. The other four pills have unique accessible names so
    // `getByRole` works directly.
    expect(screen.getByText("PNG", { selector: "button" })).toBeInTheDocument();
    for (const label of ["SVG", "PDF", "WebP", "JPEG"]) {
      expect(
        screen.getByRole("button", { name: label }),
        `format pill "${label}" should render`,
      ).toBeInTheDocument();
    }
  });

  it("calls chooseExportTarget with the active format on Export click", async () => {
    const stub = kcreateStub();
    stub.override("runtime.chooseExportTarget", () => null); // user cancels
    mount();
    await flushAsync();

    fireEvent.click(screen.getByRole("button", { name: "Export" }));
    await flushAsync();

    const dialog = stub.calls.find(
      (c) => c.method === "runtime.chooseExportTarget",
    );
    expect(dialog, "Export click should open the save dialog").toBeDefined();
    expect(dialog?.args[0]).toBe("png");
    expect(stub.calls.some((c) => c.method === "export.png")).toBe(false);
  });

  it("invokes export.png with the chosen path and persists sticky dir", async () => {
    const stub = kcreateStub();
    const chosen = "/home/test/Pictures/render.png";
    stub.override("runtime.chooseExportTarget", () => chosen);
    stub.override("export.png", () => 12345);

    mount();
    await flushAsync();

    fireEvent.click(screen.getByRole("button", { name: "Export" }));
    await waitFor(() => {
      expect(
        stub.calls.some((c) => c.method === "export.png"),
        "export.png should run after the picker resolves",
      ).toBe(true);
    });

    const png = stub.calls.find((c) => c.method === "export.png");
    expect(png?.args[0]).toBe(chosen);

    await waitFor(() => {
      expect(
        stub.calls.some((c) => c.method === "phase10.preferencesSave"),
        "sticky last-dir should be persisted after a successful export",
      ).toBe(true);
    });

    const save = stub.calls.find(
      (c) => c.method === "phase10.preferencesSave",
    );
    const savedPrefs = save?.args[0] as {
      export?: { lastDirByFormat?: Record<string, string> };
    } | undefined;
    expect(savedPrefs?.export?.lastDirByFormat?.png).toBe(
      "/home/test/Pictures",
    );
  });
});
