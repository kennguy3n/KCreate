// H1 — first-run discovery welcome component tests.
//
// The overlay is presentational: the parent owns the open gate and
// supplies the real handlers via `actions`. These pin that every close
// path fires `onDismiss` (so the "seen" marker is always written), and
// that picking an action / opening the palette dismisses FIRST and
// then runs the real handler (so the welcome never lingers on top of
// the flow the user chose).

import { describe, it, expect, vi, beforeAll, afterAll } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";

import { LocaleProvider } from "../i18n";
import { resolveMessage } from "../i18n/catalog";
import {
  DiscoveryWelcome,
  type DiscoveryAction,
} from "./DiscoveryWelcome";

function actions(order: string[]): DiscoveryAction[] {
  return [
    {
      id: "templates",
      label: "Start from a template",
      description: "Fork a ready-made design.",
      icon: "layout",
      run: () => order.push("run:templates"),
    },
    {
      id: "ai",
      label: "Generate with AI",
      description: "Describe it and let the model draft it.",
      icon: "wand",
      run: () => order.push("run:ai"),
    },
    {
      id: "elements",
      label: "Browse elements",
      description: "Drop in shapes and icons.",
      icon: "sparkles",
      run: () => order.push("run:elements"),
    },
  ];
}

describe("DiscoveryWelcome", () => {
  it("renders nothing when closed", () => {
    render(
      <DiscoveryWelcome
        open={false}
        paletteHint="Ctrl K"
        onOpenPalette={() => {}}
        actions={actions([])}
        onDismiss={() => {}}
      />,
    );
    expect(screen.queryByTestId("kcreate-discovery-welcome")).toBeNull();
  });

  it("renders the palette hint and one card per action when open", () => {
    render(
      <DiscoveryWelcome
        open
        paletteHint="Ctrl K"
        onOpenPalette={() => {}}
        actions={actions([])}
        onDismiss={() => {}}
      />,
    );
    expect(screen.getByTestId("kcreate-discovery-welcome")).toBeInTheDocument();
    expect(screen.getByText("Ctrl K")).toBeInTheDocument();
    for (const id of ["templates", "ai", "elements"]) {
      expect(
        screen.getByTestId(`kcreate-discovery-action-${id}`),
      ).toBeInTheDocument();
    }
  });

  it("dismisses BEFORE running the chosen action", () => {
    const order: string[] = [];
    const onDismiss = vi.fn(() => order.push("dismiss"));
    render(
      <DiscoveryWelcome
        open
        paletteHint="Ctrl K"
        onOpenPalette={() => {}}
        actions={actions(order)}
        onDismiss={onDismiss}
      />,
    );
    fireEvent.click(screen.getByTestId("kcreate-discovery-action-ai"));
    expect(order).toEqual(["dismiss", "run:ai"]);
  });

  it("dismisses BEFORE opening the palette", () => {
    const order: string[] = [];
    const onDismiss = vi.fn(() => order.push("dismiss"));
    const onOpenPalette = vi.fn(() => order.push("palette"));
    render(
      <DiscoveryWelcome
        open
        paletteHint="Ctrl K"
        onOpenPalette={onOpenPalette}
        actions={actions(order)}
        onDismiss={onDismiss}
      />,
    );
    fireEvent.click(screen.getByTestId("kcreate-discovery-palette"));
    expect(order).toEqual(["dismiss", "palette"]);
  });

  it("dismisses on the skip button, the close button, and Escape", () => {
    const onDismiss = vi.fn();
    render(
      <DiscoveryWelcome
        open
        paletteHint="Ctrl K"
        onOpenPalette={() => {}}
        actions={actions([])}
        onDismiss={onDismiss}
      />,
    );
    fireEvent.click(screen.getByTestId("kcreate-discovery-skip"));
    fireEvent.click(screen.getByTestId("kcreate-discovery-close"));
    fireEvent.keyDown(document.body, { key: "Escape" });
    expect(onDismiss).toHaveBeenCalledTimes(3);
  });

  it("dismisses on a backdrop click but not on a dialog-body click", () => {
    const onDismiss = vi.fn();
    render(
      <DiscoveryWelcome
        open
        paletteHint="Ctrl K"
        onOpenPalette={() => {}}
        actions={actions([])}
        onDismiss={onDismiss}
      />,
    );
    // Clicking the dialog body (a child) must not dismiss…
    fireEvent.click(screen.getByText(/welcome to kcreate/i));
    expect(onDismiss).not.toHaveBeenCalled();
    // …but clicking the backdrop (the dialog root itself) does.
    fireEvent.click(screen.getByTestId("kcreate-discovery-welcome"));
    expect(onDismiss).toHaveBeenCalledTimes(1);
  });
});

describe("DiscoveryWelcome (focus management)", () => {
  // jsdom has no layout engine, so every element reports
  // `offsetHeight === 0` and `useFocusTrap`'s visibility filter would
  // treat the dialog's controls as hidden. Shim a positive height for
  // this suite so the focusable query behaves like a laid-out document
  // (the same workaround the useFocusTrap suite uses).
  let offsetSpy: { mockRestore: () => void };

  beforeAll(() => {
    offsetSpy = vi
      .spyOn(HTMLElement.prototype, "offsetHeight", "get")
      .mockReturnValue(1);
  });

  afterAll(() => {
    offsetSpy.mockRestore();
  });

  it("moves focus into the dialog (the close control) when opened", () => {
    render(
      <DiscoveryWelcome
        open
        paletteHint="Ctrl K"
        onOpenPalette={() => {}}
        actions={actions([])}
        onDismiss={() => {}}
      />,
    );
    expect(document.activeElement).toBe(
      screen.getByTestId("kcreate-discovery-close"),
    );
  });

  it("wraps Tab from the last focusable back to the first", () => {
    render(
      <DiscoveryWelcome
        open
        paletteHint="Ctrl K"
        onOpenPalette={() => {}}
        actions={actions([])}
        onDismiss={() => {}}
      />,
    );
    screen.getByTestId("kcreate-discovery-skip").focus();
    document.dispatchEvent(
      new KeyboardEvent("keydown", { key: "Tab", bubbles: true }),
    );
    expect(document.activeElement).toBe(
      screen.getByTestId("kcreate-discovery-close"),
    );
  });

  it("wraps Shift+Tab from the first focusable back to the last", () => {
    render(
      <DiscoveryWelcome
        open
        paletteHint="Ctrl K"
        onOpenPalette={() => {}}
        actions={actions([])}
        onDismiss={() => {}}
      />,
    );
    screen.getByTestId("kcreate-discovery-close").focus();
    document.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "Tab",
        shiftKey: true,
        bubbles: true,
      }),
    );
    expect(document.activeElement).toBe(
      screen.getByTestId("kcreate-discovery-skip"),
    );
  });

  it("returns focus to the opener when closed", () => {
    const { getByTestId, rerender } = render(
      <div>
        <button data-testid="opener">opener</button>
        <DiscoveryWelcome
          open={false}
          paletteHint="Ctrl K"
          onOpenPalette={() => {}}
          actions={actions([])}
          onDismiss={() => {}}
        />
      </div>,
    );
    const opener = getByTestId("opener");
    opener.focus();
    expect(document.activeElement).toBe(opener);

    rerender(
      <div>
        <button data-testid="opener">opener</button>
        <DiscoveryWelcome
          open
          paletteHint="Ctrl K"
          onOpenPalette={() => {}}
          actions={actions([])}
          onDismiss={() => {}}
        />
      </div>,
    );
    expect(document.activeElement).toBe(
      getByTestId("kcreate-discovery-close"),
    );

    rerender(
      <div>
        <button data-testid="opener">opener</button>
        <DiscoveryWelcome
          open={false}
          paletteHint="Ctrl K"
          onOpenPalette={() => {}}
          actions={actions([])}
          onDismiss={() => {}}
        />
      </div>,
    );
    expect(document.activeElement).toBe(opener);
  });
});

describe("DiscoveryWelcome (localized)", () => {
  it("renders its chrome from the active catalog (es + ar)", () => {
    for (const locale of ["es", "ar"] as const) {
      const { unmount } = render(
        <LocaleProvider initialLocale={locale}>
          <DiscoveryWelcome
            open
            paletteHint="Ctrl K"
            onOpenPalette={() => {}}
            actions={actions([])}
            onDismiss={() => {}}
          />
        </LocaleProvider>,
      );
      // Title, lead, palette CTA, and skip button all come from the
      // catalog rather than hard-coded English.
      expect(
        screen.getByText(resolveMessage(locale, "discovery.title")),
      ).toBeInTheDocument();
      expect(
        screen.getByText(resolveMessage(locale, "discovery.lead")),
      ).toBeInTheDocument();
      expect(
        screen.getByText(resolveMessage(locale, "discovery.openPalette")),
      ).toBeInTheDocument();
      expect(
        screen.getByTestId("kcreate-discovery-skip"),
      ).toHaveTextContent(resolveMessage(locale, "discovery.skip"));
      // The close control's accessible name is localized too.
      expect(
        screen.getByRole("button", {
          name: resolveMessage(locale, "discovery.aria.close"),
        }),
      ).toBeInTheDocument();
      unmount();
    }
  });
});
