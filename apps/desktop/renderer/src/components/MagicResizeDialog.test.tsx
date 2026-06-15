// MagicResizeDialog tests (G5 Magic Resize + H6 content-aware depth).
//
// Pins the dialog's contract:
//   * presets render grouped and a toggle flips its aria-checked state;
//   * the primary action is disabled until at least one size is picked;
//   * selecting TWO sizes and confirming hands the parent BOTH targets
//     in one `onResize` call — and routing that through the bridge
//     `artboard.magicResize` returns two new artboard ids (the
//     "2 sizes -> 2 new artboards" flow the spec asks for);
//   * the content-aware toggles (text re-fit + image smart-crop) default
//     on and are threaded through to both `onResize` and `onExport`;
//   * "Resize & export all" routes through the real bridge
//     `artboard.magicResizeExportPng` path and reports the written PNGs;
//   * Cancel closes without resizing.

import { describe, it, expect } from "vitest";
import { render, screen, fireEvent, act } from "@testing-library/react";

import { MagicResizeDialog } from "./MagicResizeDialog";
import { kcreateStub } from "../../tests/helpers/kcreateStub";
import type {
  ArtboardInfo,
  ArtboardPreset,
  MagicResizeContent,
  MagicResizeExportReport,
  ResizeTarget,
} from "../../../shared/scene";

async function flushAsync() {
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
  });
}

const SOURCE: ArtboardInfo = {
  id: "src-artboard",
  name: "Promo",
  x: 0,
  y: 0,
  width: 1080,
  height: 1080,
  pageId: "page-1",
};

const PRESETS: ArtboardPreset[] = [
  { name: "Instagram Post", width: 1080, height: 1080, category: "social_media" },
  { name: "Instagram Story", width: 1080, height: 1920, category: "social_media" },
  { name: "A4", width: 2480, height: 3508, category: "print" },
];

const noop = (): void => {};

describe("MagicResizeDialog", () => {
  it("renders nothing when closed", () => {
    const { container } = render(
      <MagicResizeDialog
        open={false}
        source={SOURCE}
        presets={PRESETS}
        onResize={noop}
        onExport={noop}
        onClose={noop}
      />,
    );
    expect(container).toBeEmptyDOMElement();
  });

  it("disables the generate action until a size is selected", () => {
    render(
      <MagicResizeDialog
        open
        source={SOURCE}
        presets={PRESETS}
        onResize={noop}
        onExport={noop}
        onClose={noop}
      />,
    );
    const generate = screen.getByRole("button", { name: /Generate/ });
    expect(generate).toBeDisabled();

    fireEvent.click(screen.getByRole("checkbox", { name: /Instagram Post/ }));
    expect(generate).toBeEnabled();
  });

  it("toggling a preset flips its checked state", () => {
    render(
      <MagicResizeDialog
        open
        source={SOURCE}
        presets={PRESETS}
        onResize={noop}
        onExport={noop}
        onClose={noop}
      />,
    );
    const toggle = screen.getByRole("checkbox", { name: /A4/ });
    expect(toggle).toHaveAttribute("aria-checked", "false");
    fireEvent.click(toggle);
    expect(toggle).toHaveAttribute("aria-checked", "true");
    fireEvent.click(toggle);
    expect(toggle).toHaveAttribute("aria-checked", "false");
  });

  it("selecting two sizes resizes to two new artboards", async () => {
    const handle = kcreateStub();
    // The parent (EditorPage) owns the bridge round-trip; emulate it so
    // the test exercises the full "2 sizes -> magicResize -> 2 ids" path.
    handle.override("artboard.magicResize", () => ["story-id", "a4-id"]);

    let received: ResizeTarget[] | null = null;
    let receivedContent: MagicResizeContent | null = null;
    let newIds: string[] = [];
    render(
      <MagicResizeDialog
        open
        source={SOURCE}
        presets={PRESETS}
        onResize={(targets, content) => {
          received = targets;
          receivedContent = content;
          void (async () => {
            newIds = await window.kcreate.artboard.magicResize(
              SOURCE.id,
              targets,
              content,
            );
          })();
        }}
        onExport={noop}
        onClose={noop}
      />,
    );

    fireEvent.click(screen.getByRole("checkbox", { name: /Instagram Story/ }));
    fireEvent.click(screen.getByRole("checkbox", { name: /A4/ }));
    fireEvent.click(screen.getByRole("button", { name: /Generate 2 resizes/ }));
    await flushAsync();

    // The dialog handed the parent both targets in a single call.
    expect(received).not.toBeNull();
    expect(received).toHaveLength(2);
    const names = (received as unknown as ResizeTarget[])
      .map((t) => t.preset)
      .sort();
    expect(names).toEqual(["A4", "Instagram Story"]);

    // …with the content-aware toggles defaulting on.
    expect(receivedContent).toEqual({ refitText: true, smartCrop: true });

    // It routed through the bridge once with both targets…
    const calls = handle.calls.filter(
      (c) => c.method === "artboard.magicResize",
    );
    expect(calls).toHaveLength(1);
    const call = calls[0];
    expect(call?.args[0]).toBe(SOURCE.id);
    expect(call?.args[1]).toHaveLength(2);

    // …and got two new artboard ids back.
    expect(newIds).toEqual(["story-id", "a4-id"]);
  });

  it("defaults content-aware toggles on and threads them through onResize", () => {
    let received: MagicResizeContent | null = null;
    render(
      <MagicResizeDialog
        open
        source={SOURCE}
        presets={PRESETS}
        onResize={(_targets, content) => {
          received = content;
        }}
        onExport={noop}
        onClose={noop}
      />,
    );

    const refit = screen.getByRole("checkbox", { name: /Re-fit text to box/ });
    const crop = screen.getByRole("checkbox", { name: /Smart-crop images/ });
    expect(refit).toBeChecked();
    expect(crop).toBeChecked();

    // Turn OFF text re-fit, leave smart-crop on, then resize.
    fireEvent.click(refit);
    expect(refit).not.toBeChecked();
    fireEvent.click(screen.getByRole("checkbox", { name: /Instagram Post/ }));
    fireEvent.click(screen.getByRole("button", { name: /Generate/ }));

    expect(received).toEqual({ refitText: false, smartCrop: true });
  });

  it("Resize & export all routes through the bridge export path", async () => {
    const handle = kcreateStub();
    handle.override("artboard.magicResizeExportPng", () => ({
      artboard_ids: ["story-id", "a4-id"],
      output_dir: "/out",
      written: ["/out/01_story.png", "/out/02_a4.png"],
      failed: [],
      duration_ms: 12,
    }));

    let exportTargets: ResizeTarget[] | null = null;
    let exportContent: MagicResizeContent | null = null;
    let report: MagicResizeExportReport | null = null;
    render(
      <MagicResizeDialog
        open
        source={SOURCE}
        presets={PRESETS}
        onResize={noop}
        onExport={(targets, content) => {
          exportTargets = targets;
          exportContent = content;
          void (async () => {
            report = await window.kcreate.artboard.magicResizeExportPng(
              SOURCE.id,
              targets,
              { outputDir: "/out", content },
            );
          })();
        }}
        onClose={noop}
      />,
    );

    fireEvent.click(screen.getByRole("checkbox", { name: /Instagram Story/ }));
    fireEvent.click(screen.getByRole("checkbox", { name: /A4/ }));
    fireEvent.click(
      screen.getByRole("button", { name: /Resize & export all/ }),
    );
    await flushAsync();

    expect(exportTargets).not.toBeNull();
    expect(exportTargets).toHaveLength(2);
    expect(exportContent).toEqual({ refitText: true, smartCrop: true });

    // Routed once through the real bridge export channel.
    const calls = handle.calls.filter(
      (c) => c.method === "artboard.magicResizeExportPng",
    );
    expect(calls).toHaveLength(1);
    expect(calls[0]?.args[0]).toBe(SOURCE.id);

    // …and reported the written PNGs back.
    expect(report).not.toBeNull();
    expect((report as unknown as MagicResizeExportReport).written).toHaveLength(
      2,
    );
  });

  it("emits targets in on-screen display order, not click order", () => {
    let received: ResizeTarget[] | null = null;
    render(
      <MagicResizeDialog
        open
        source={SOURCE}
        presets={PRESETS}
        onResize={(targets) => {
          received = targets;
        }}
        onExport={noop}
        onClose={noop}
      />,
    );

    // Click in REVERSE display order: A4 (print, last) before Instagram
    // Story (social_media, earlier). The generated artboards should still
    // come out in display order so their left-to-right layout matches the
    // grid the user sees.
    fireEvent.click(screen.getByRole("checkbox", { name: /A4/ }));
    fireEvent.click(screen.getByRole("checkbox", { name: /Instagram Story/ }));
    fireEvent.click(screen.getByRole("button", { name: /Generate 2 resizes/ }));

    expect(received).not.toBeNull();
    const names = (received as unknown as ResizeTarget[]).map((t) => t.preset);
    expect(names).toEqual(["Instagram Story", "A4"]);
  });

  it("does not fire a second resize while the first is in flight", () => {
    let resizeCalls = 0;
    render(
      <MagicResizeDialog
        open
        source={SOURCE}
        presets={PRESETS}
        // The parent stays "busy" (does not close the dialog yet),
        // mirroring the async bridge round-trip window during which a
        // second click must not spawn duplicate artboards.
        onResize={() => {
          resizeCalls += 1;
        }}
        onExport={noop}
        onClose={noop}
      />,
    );
    fireEvent.click(screen.getByRole("checkbox", { name: /Instagram Story/ }));
    const generate = screen.getByRole("button", { name: /Generate/ });
    fireEvent.click(generate);
    expect(resizeCalls).toBe(1);

    // The action latches: disabled + shows progress.
    expect(generate).toBeDisabled();
    expect(generate).toHaveTextContent(/Generating/);

    fireEvent.click(generate);
    expect(resizeCalls).toBe(1);
  });

  it("re-enables the actions when the export settles without closing the dialog", async () => {
    // Mirrors EditorPage: onExport returns the async handler's Promise.
    // When that handler bails early — e.g. the user cancels the export
    // directory picker — it resolves WITHOUT the parent closing the
    // dialog. The busy latch must clear so the buttons don't stay stuck
    // on "Exporting…" forever (Devin Review bug).
    let resolveExport: (() => void) | null = null;
    render(
      <MagicResizeDialog
        open
        source={SOURCE}
        presets={PRESETS}
        onResize={noop}
        onExport={() =>
          new Promise<void>((resolve) => {
            resolveExport = resolve;
          })
        }
        onClose={noop}
      />,
    );

    fireEvent.click(screen.getByRole("checkbox", { name: /Instagram Story/ }));
    const exportButton = screen.getByRole("button", {
      name: /Resize & export all/,
    });
    fireEvent.click(exportButton);

    // Latched while the action is in flight.
    expect(exportButton).toBeDisabled();
    expect(exportButton).toHaveTextContent(/Exporting/);

    // The handler resolves but does NOT close the dialog (cancel path).
    await act(async () => {
      resolveExport?.();
      await Promise.resolve();
    });

    // Busy cleared → the action is usable again with the dialog open.
    expect(exportButton).toBeEnabled();
    expect(exportButton).toHaveTextContent(/Resize & export all/);
  });

  it("Cancel closes without resizing", () => {
    let closed = false;
    let resized = false;
    render(
      <MagicResizeDialog
        open
        source={SOURCE}
        presets={PRESETS}
        onResize={() => {
          resized = true;
        }}
        onExport={noop}
        onClose={() => {
          closed = true;
        }}
      />,
    );
    fireEvent.click(screen.getByRole("checkbox", { name: /Instagram Post/ }));
    fireEvent.click(screen.getByRole("button", { name: /Cancel/ }));
    expect(closed).toBe(true);
    expect(resized).toBe(false);
  });
});
