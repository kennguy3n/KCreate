// Tests for the LanguageSwitcher control.
//
// The switcher is a native <select> (keyboard- and screen-reader-
// complete out of the box). These tests assert it:
//   * exposes an accessible name (aria-label) and a labelled combobox;
//   * lists every shipped locale by its endonym;
//   * drives `setLocale`, which flips `document.documentElement.dir`
//     to RTL when Arabic is chosen.

import { describe, it, expect, beforeEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";

import { LocaleProvider } from "../i18n";
import { LanguageSwitcher } from "./LanguageSwitcher";

function renderSwitcher(initialLocale?: "en" | "es" | "ar") {
  return render(
    <LocaleProvider initialLocale={initialLocale}>
      <LanguageSwitcher />
    </LocaleProvider>,
  );
}

describe("LanguageSwitcher", () => {
  beforeEach(() => {
    window.localStorage.clear();
    document.documentElement.removeAttribute("dir");
  });

  it("renders a labelled combobox listing every locale endonym", () => {
    renderSwitcher("en");
    const select = screen.getByRole("combobox", { name: "Change language" });
    expect(select).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "English" })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "Español" })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "العربية" })).toBeInTheDocument();
  });

  it("reflects the active locale as the selected value", () => {
    renderSwitcher("es");
    const select = screen.getByTestId("kcreate-language-switcher");
    expect(select).toHaveValue("es");
  });

  it("switches to Arabic and flips the document to RTL", () => {
    renderSwitcher("en");
    const select = screen.getByTestId("kcreate-language-switcher");
    fireEvent.change(select, { target: { value: "ar" } });
    expect(select).toHaveValue("ar");
    expect(document.documentElement.dir).toBe("rtl");
    expect(window.localStorage.getItem("kcreate.locale")).toBe("ar");
  });
});
