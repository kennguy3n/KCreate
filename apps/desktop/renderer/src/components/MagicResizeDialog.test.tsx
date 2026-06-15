// MagicResizeDialog tests (G5 — Magic Resize).
//
// Pins the dialog's contract:
//   * presets render grouped and a toggle flips its aria-checked state;
//   * the primary action is disabled until at least one size is picked;
//   * selecting TWO sizes and confirming hands the parent BOTH targets
//     in one `onResize` call — and routing that through the bridge
//     `artboard.magicResize` returns two new artboard ids (the
//     "2 sizes -> 2 new artboards" flow the spec asks for);
//   * Cancel closes without resizing.

import { describe, it, expect } from "vitest";
import { render, screen, fireEvent, act } from "@testing-library/react";

import { MagicResizeDialog } from "./MagicResizeDialog";
import { kcreateStub } from "../../tests/helpers/kcreateStub";
import type {
  ArtboardInfo,
  ArtboardPreset,
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

describe("MagicResizeDialog", () => {
  it("renders nothing when closed", () => {
    const { container } = render(
      <MagicResizeDialog
        open={false}
        source={SOURCE}
        presets={PRESETS}
        onResize={() => {}}
        onClose={() => {}}
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
        onResize={() => {}}
        onClose={() => {}}
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
        onResize={() => {}}
        onClose={() => {}}
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
    let newIds: string[] = [];
    render(
      <MagicResizeDialog
        open
        source={SOURCE}
        presets={PRESETS}
        onResize={(targets) => {
          received = targets;
          void (async () => {
            newIds = await window.kcreate.artboard.magicResize(
              SOURCE.id,
              targets,
            );
          })();
        }}
        onClose={() => {}}
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
