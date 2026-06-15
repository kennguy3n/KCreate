// H1 — global shortcut dispatch test for the command palette.
//
// The palette is opened by the `openCommandPalette` action wired
// through `useShortcuts` in EditorPage. This mounts a tiny harness
// around the real hook + real registry binding and proves that a
// Cmd/Ctrl+K keydown on the document fires the handler, while a
// keystroke inside a form field is gated (so typing "k" into an input
// never opens the palette).

import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, fireEvent } from "@testing-library/react";

import { useShortcuts, type ShortcutHandlers } from "./useShortcuts";
import { resetShortcutStoreForTests } from "./registry";

function Harness({ handlers }: { handlers: ShortcutHandlers }): JSX.Element {
  useShortcuts(handlers);
  return (
    <div>
      <input data-testid="field" />
    </div>
  );
}

describe("useShortcuts — command palette open chord", () => {
  beforeEach(() => {
    if (typeof window !== "undefined" && window.localStorage) {
      window.localStorage.removeItem("kcreate.shortcuts.v1");
    }
    resetShortcutStoreForTests();
  });

  it("fires openCommandPalette on Ctrl+K from the document", () => {
    const openCommandPalette = vi.fn();
    render(<Harness handlers={{ openCommandPalette }} />);
    fireEvent.keyDown(document.body, { key: "k", ctrlKey: true });
    expect(openCommandPalette).toHaveBeenCalledTimes(1);
  });

  it("fires openCommandPalette on Cmd+K (macOS) too", () => {
    const openCommandPalette = vi.fn();
    render(<Harness handlers={{ openCommandPalette }} />);
    fireEvent.keyDown(document.body, { key: "k", metaKey: true });
    expect(openCommandPalette).toHaveBeenCalledTimes(1);
  });

  it("does not fire on a bare 'k' keypress", () => {
    const openCommandPalette = vi.fn();
    render(<Harness handlers={{ openCommandPalette }} />);
    fireEvent.keyDown(document.body, { key: "k" });
    expect(openCommandPalette).not.toHaveBeenCalled();
  });

  it("is gated while focus is in a form field (typing never opens it)", () => {
    const openCommandPalette = vi.fn();
    const { getByTestId } = render(
      <Harness handlers={{ openCommandPalette }} />,
    );
    const field = getByTestId("field");
    // Keydown originating from the input must be ignored by the global
    // listener so Ctrl+K typed mid-edit doesn't hijack the keystroke.
    fireEvent.keyDown(field, { key: "k", ctrlKey: true });
    expect(openCommandPalette).not.toHaveBeenCalled();
  });
});
