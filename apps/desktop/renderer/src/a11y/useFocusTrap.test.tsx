// Tests for the useFocusTrap hook — the focus-management contract every
// modal/overlay owes:
//   * on activate, focus moves into the container;
//   * Tab / Shift+Tab wrap at the focusable boundary (focus can never
//     escape behind the overlay);
//   * Escape invokes onEscape;
//   * on deactivate, focus returns to the opener.
//
// jsdom has no layout engine, so every element reports
// `offsetWidth/offsetHeight === 0`; the hook's visibility filter would
// then treat all controls as hidden. We shim `offsetHeight` to a
// positive value for this suite so the focusable query behaves like a
// laid-out document (the standard jsdom focus-trap workaround).

import { describe, it, expect, beforeAll, afterAll, vi } from "vitest";
import { render } from "@testing-library/react";

import { useFocusTrap } from "./useFocusTrap";

function Harness({
  active,
  onEscape,
}: {
  active: boolean;
  onEscape: () => void;
}): JSX.Element {
  const ref = useFocusTrap<HTMLDivElement>({ active, onEscape });
  return (
    <div>
      <button data-testid="opener">opener</button>
      <div ref={ref} data-testid="trap">
        <button data-testid="first">first</button>
        <button data-testid="middle">middle</button>
        <button data-testid="last">last</button>
      </div>
    </div>
  );
}

let offsetSpy: { mockRestore: () => void };

beforeAll(() => {
  offsetSpy = vi
    .spyOn(HTMLElement.prototype, "offsetHeight", "get")
    .mockReturnValue(1);
});

afterAll(() => {
  offsetSpy.mockRestore();
});

describe("useFocusTrap", () => {
  it("moves focus into the container on activate", () => {
    const { getByTestId } = render(
      <Harness active={true} onEscape={() => {}} />,
    );
    expect(document.activeElement).toBe(getByTestId("first"));
  });

  it("invokes onEscape when Escape is pressed", () => {
    const onEscape = vi.fn();
    render(<Harness active={true} onEscape={onEscape} />);
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    expect(onEscape).toHaveBeenCalledTimes(1);
  });

  it("wraps Tab from the last focusable back to the first", () => {
    const { getByTestId } = render(
      <Harness active={true} onEscape={() => {}} />,
    );
    getByTestId("last").focus();
    document.dispatchEvent(
      new KeyboardEvent("keydown", { key: "Tab", bubbles: true }),
    );
    expect(document.activeElement).toBe(getByTestId("first"));
  });

  it("wraps Shift+Tab from the first focusable back to the last", () => {
    const { getByTestId } = render(
      <Harness active={true} onEscape={() => {}} />,
    );
    getByTestId("first").focus();
    document.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "Tab",
        shiftKey: true,
        bubbles: true,
      }),
    );
    expect(document.activeElement).toBe(getByTestId("last"));
  });

  it("returns focus to the opener when deactivated", () => {
    const { getByTestId, rerender } = render(
      <Harness active={false} onEscape={() => {}} />,
    );
    const opener = getByTestId("opener");
    opener.focus();
    expect(document.activeElement).toBe(opener);

    rerender(<Harness active={true} onEscape={() => {}} />);
    expect(document.activeElement).toBe(getByTestId("first"));

    rerender(<Harness active={false} onEscape={() => {}} />);
    expect(document.activeElement).toBe(opener);
  });
});
