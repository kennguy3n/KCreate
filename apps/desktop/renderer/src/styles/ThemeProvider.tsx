// Phase 6 Tasks 23-24: React context provider that exposes the
// active theme id and a toggle function. The chosen theme is
// persisted to `localStorage` under `kcreate.theme` so it survives
// reloads.
//
// The renderer's theming model is CSS-variable driven (see
// `./themes.ts` for the rationale): components style themselves via
// `var(--kc-*)` references from `./tokens.ts`, and switching themes
// is just a matter of writing `data-theme="dark"` onto the document
// element. We therefore expose only `themeId`, `setTheme`, and
// `toggle` from the context — there is no `palette` object because
// duplicating the palette in JS would create a second source of
// truth that could drift from `index.html`.
//
// Usage:
//
//   <ThemeProvider>
//     <App />
//   </ThemeProvider>
//
// In any descendant:
//
//   const { themeId, toggle } = useTheme();
//   <button onClick={toggle}>{themeId === "dark" ? "☀" : "☾"}</button>

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
} from "react";

import type { ThemeId } from "./themes";

const STORAGE_KEY = "kcreate.theme";

interface ThemeContextValue {
  themeId: ThemeId;
  setTheme: (id: ThemeId) => void;
  toggle: () => void;
}

const ThemeContext = createContext<ThemeContextValue | null>(null);

function loadPersistedTheme(): ThemeId {
  if (typeof window === "undefined" || !window.localStorage) return "light";
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (raw === "dark" || raw === "light") return raw;
  } catch {
    // Private-mode / quota errors.
  }
  return "light";
}

function persistTheme(id: ThemeId): void {
  if (typeof window === "undefined" || !window.localStorage) return;
  try {
    window.localStorage.setItem(STORAGE_KEY, id);
  } catch {
    // Non-fatal.
  }
}

export function ThemeProvider({
  children,
}: {
  children: React.ReactNode;
}): JSX.Element {
  const [themeId, setThemeId] = useState<ThemeId>(loadPersistedTheme);

  const setTheme = useCallback((id: ThemeId) => {
    setThemeId(id);
    persistTheme(id);
  }, []);

  const toggle = useCallback(() => {
    setThemeId((prev) => {
      const next = prev === "dark" ? "light" : "dark";
      persistTheme(next);
      return next;
    });
  }, []);

  // Push the theme id onto the root element so the CSS variable
  // overrides in `index.html` (`:root[data-theme="dark"]`) take
  // effect and every `var(--kc-*)` reference flips synchronously.
  useEffect(() => {
    document.documentElement.setAttribute("data-theme", themeId);
    return () => {
      document.documentElement.removeAttribute("data-theme");
    };
  }, [themeId]);

  const value = useMemo<ThemeContextValue>(
    () => ({
      themeId,
      setTheme,
      toggle,
    }),
    [themeId, setTheme, toggle],
  );

  return (
    <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>
  );
}

export function useTheme(): ThemeContextValue {
  const ctx = useContext(ThemeContext);
  if (!ctx) {
    throw new Error("useTheme must be called inside a <ThemeProvider>.");
  }
  return ctx;
}
