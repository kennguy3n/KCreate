// i18n + a11y coverage for the TopBar.
//
// The plain TopBar.test.tsx renders the bar bare (no LocaleProvider) and
// pins the English defaults. This file wraps it in a LocaleProvider so
// it exercises the actual localization path: every label, aria-name,
// and tooltip on the chrome must come from the active catalog. Expected
// strings are pulled from `resolveMessage` rather than hard-coded so the
// test asserts "TopBar renders whatever the catalog says for this
// locale" (the contract) instead of duplicating the translations.

import { describe, it, expect, beforeEach } from "vitest";
import { render, screen, within } from "@testing-library/react";

import { ThemeProvider } from "../styles/ThemeProvider";
import { LocaleProvider } from "../i18n";
import { resolveMessage } from "../i18n/catalog";
import { EDITOR_MODES, TopBar, TOOL_LABELS } from "./TopBar";

function renderLocalizedTopBar(locale: "es" | "ar") {
  return render(
    <LocaleProvider initialLocale={locale}>
      <ThemeProvider>
        <TopBar
          projectName="demo"
          mode="design"
          onModeChange={() => {}}
          tool="select"
          onToolChange={() => {}}
          canUndo={true}
          canRedo={true}
          onUndo={() => {}}
          onRedo={() => {}}
          onExport={() => {}}
          onBackHome={() => {}}
        />
      </ThemeProvider>
    </LocaleProvider>,
  );
}

describe("TopBar (localized)", () => {
  beforeEach(() => {
    window.localStorage.clear();
    document.documentElement.removeAttribute("dir");
  });

  it("labels the mode nav and tabs from the Spanish catalog", () => {
    renderLocalizedTopBar("es");
    const modeNav = screen.getByRole("navigation", {
      name: resolveMessage("es", "topbar.aria.editorMode"),
    });
    for (const { mode } of EDITOR_MODES) {
      const label = resolveMessage("es", `topbar.mode.${mode}`);
      expect(
        within(modeNav).getByRole("button", { name: label }),
      ).toBeInTheDocument();
    }
  });

  it("localizes the back-to-home control's accessible name", () => {
    renderLocalizedTopBar("es");
    expect(
      screen.getByRole("button", {
        name: resolveMessage("es", "topbar.aria.backToHome"),
      }),
    ).toBeInTheDocument();
  });

  it("localizes tool aria-labels and the {label} ({key}) tooltip", () => {
    renderLocalizedTopBar("es");
    const toolbar = screen.getByRole("toolbar", {
      name: resolveMessage("es", "topbar.aria.drawingTools"),
    });
    const selectLabel = resolveMessage("es", "topbar.tool.select");
    const selectBtn = within(toolbar).getByRole("button", {
      name: selectLabel,
    });
    expect(selectBtn).toHaveAttribute(
      "title",
      `${selectLabel} (${TOOL_LABELS.select.key})`,
    );
  });

  it("renders the language switcher reflecting the active locale", () => {
    renderLocalizedTopBar("es");
    expect(screen.getByTestId("kcreate-language-switcher")).toHaveValue("es");
  });
});
