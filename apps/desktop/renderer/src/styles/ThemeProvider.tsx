// Phase 6 Tasks 23-24: React context provider that exposes the
// active theme palette and a toggle function. The chosen theme is
// persisted to `localStorage` under `kcreate.theme` so it survives
// reloads.
//
// Usage:
//
//   <ThemeProvider>
//     <App />
//   </ThemeProvider>
//
// In any descendant:
//
//   const { palette, themeId, setTheme } = useTheme();
//   <div style={{ background: palette.bg }}>…</div>

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
} from "react";

import type { ThemeId, ThemePalette } from "./themes";
import { paletteFor } from "./themes";

const STORAGE_KEY = "kcreate.theme";

interface ThemeContextValue {
  themeId: ThemeId;
  palette: ThemePalette;
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

  // Push the theme id onto the root element so global CSS selectors
  // (e.g. `[data-theme="dark"] *`) can react.
  useEffect(() => {
    document.documentElement.setAttribute("data-theme", themeId);
    return () => {
      document.documentElement.removeAttribute("data-theme");
    };
  }, [themeId]);

  const value = useMemo<ThemeContextValue>(
    () => ({
      themeId,
      palette: paletteFor(themeId),
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
