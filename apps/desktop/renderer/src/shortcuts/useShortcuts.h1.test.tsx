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

function Harness({
  handlers,
  enabled = true,
}: {
  handlers: ShortcutHandlers;
  enabled?: boolean;
}): JSX.Element {
  useShortcuts(handlers, enabled);
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

  // Regression — Devin Review BUG_0001: while a modal overlay (e.g. the
  // first-run discovery welcome) is open, EditorPage passes
  // `enabled=false`, so global editor shortcuts must NOT fire under the
  // modal. Otherwise Escape would also clear the selection and a bare
  // "r" would switch tools behind the overlay.
  it("suppresses every global shortcut while disabled (modal open)", () => {
    const clearSelection = vi.fn();
    const toolRect = vi.fn();
    const { rerender } = render(
      <Harness handlers={{ clearSelection, toolRect }} enabled={false} />,
    );
    fireEvent.keyDown(document.body, { key: "Escape" });
    fireEvent.keyDown(document.body, { key: "r" });
    expect(clearSelection).not.toHaveBeenCalled();
    expect(toolRect).not.toHaveBeenCalled();

    // Re-enabling (modal closed) restores dispatch on the same window
    // listener — proving the gate is checked at dispatch time, not at
    // attach time.
    rerender(<Harness handlers={{ clearSelection, toolRect }} enabled />);
    fireEvent.keyDown(document.body, { key: "Escape" });
    fireEvent.keyDown(document.body, { key: "r" });
    expect(clearSelection).toHaveBeenCalledTimes(1);
    expect(toolRect).toHaveBeenCalledTimes(1);
  });
});
