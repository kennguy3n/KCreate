// Tests for the i18n LocaleProvider + useI18n hook.
//
// Covers the contract the whole renderer leans on:
//   * `useI18n` works WITHOUT a provider, defaulting to the English /
//     LTR catalog — this is what keeps the ~400 provider-less component
//     tests green;
//   * a mounted provider translates through the active catalog and
//     interpolates vars;
//   * `setLocale` flips `document.documentElement.{lang,dir}` (the one
//     place RTL is toggled), persists to localStorage, and announces
//     the change in a polite live region in the NEW locale.

import { describe, it, expect, beforeEach } from "vitest";
import { render, screen, fireEvent, act } from "@testing-library/react";

import { LocaleProvider, useI18n } from "./LocaleProvider";

function Consumer(): JSX.Element {
  const { t, locale, dir, setLocale } = useI18n();
  return (
    <div>
      <span data-testid="home">{t("topbar.home")}</span>
      <span data-testid="locale">{locale}</span>
      <span data-testid="dir">{dir}</span>
      <span data-testid="msg">
        {t("app.error.openProject", { message: "boom" })}
      </span>
      <button onClick={() => setLocale("es")}>es</button>
      <button onClick={() => setLocale("ar")}>ar</button>
    </div>
  );
}

describe("useI18n without a provider", () => {
  it("defaults to the English LTR catalog instead of throwing", () => {
    render(<Consumer />);
    expect(screen.getByTestId("home")).toHaveTextContent("Home");
    expect(screen.getByTestId("locale")).toHaveTextContent("en");
    expect(screen.getByTestId("dir")).toHaveTextContent("ltr");
  });

  it("interpolates ICU-lite vars through the default t()", () => {
    render(<Consumer />);
    expect(screen.getByTestId("msg")).toHaveTextContent(
      "Failed to open project: boom",
    );
  });
});

describe("LocaleProvider", () => {
  beforeEach(() => {
    window.localStorage.clear();
    document.documentElement.removeAttribute("dir");
    document.documentElement.removeAttribute("lang");
  });

  it("translates through the active locale and reflects it on <html>", () => {
    render(
      <LocaleProvider initialLocale="es">
        <Consumer />
      </LocaleProvider>,
    );
    expect(screen.getByTestId("home")).toHaveTextContent("Inicio");
    expect(document.documentElement.lang).toBe("es");
    expect(document.documentElement.dir).toBe("ltr");
  });

  it("switches to Arabic, flips the document to RTL, and persists", () => {
    render(
      <LocaleProvider>
        <Consumer />
      </LocaleProvider>,
    );
    expect(screen.getByTestId("home")).toHaveTextContent("Home");

    fireEvent.click(screen.getByRole("button", { name: "ar" }));

    expect(screen.getByTestId("home")).toHaveTextContent("الرئيسية");
    expect(screen.getByTestId("dir")).toHaveTextContent("rtl");
    expect(document.documentElement.dir).toBe("rtl");
    expect(document.documentElement.lang).toBe("ar");
    expect(window.localStorage.getItem("kcreate.locale")).toBe("ar");
  });

  it("announces the change in a polite live region in the new locale", () => {
    render(
      <LocaleProvider>
        <Consumer />
      </LocaleProvider>,
    );
    const announcer = screen.getByTestId("kcreate-locale-announcer");
    expect(announcer).toHaveAttribute("aria-live", "polite");
    expect(announcer).toHaveTextContent("");

    fireEvent.click(screen.getByRole("button", { name: "es" }));
    expect(announcer).toHaveTextContent("Idioma cambiado a Español");
  });

  it("reads the persisted locale on mount", () => {
    window.localStorage.setItem("kcreate.locale", "es");
    render(
      <LocaleProvider>
        <Consumer />
      </LocaleProvider>,
    );
    expect(screen.getByTestId("locale")).toHaveTextContent("es");
    expect(screen.getByTestId("home")).toHaveTextContent("Inicio");
  });

  it("ignores a re-selection of the already-active locale", () => {
    render(
      <LocaleProvider initialLocale="es">
        <Consumer />
      </LocaleProvider>,
    );
    const announcer = screen.getByTestId("kcreate-locale-announcer");
    act(() => {
      fireEvent.click(screen.getByRole("button", { name: "es" }));
    });
    // No announcement when the locale does not actually change.
    expect(announcer).toHaveTextContent("");
  });
});
