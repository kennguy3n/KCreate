// Phase 6 Tasks 23-24: light and dark theme palettes.
//
// Each palette is a complete set of the colours used throughout the
// renderer. Components call `useTheme()` to get the active palette;
// the provider stores the choice in `localStorage` and applies a
// `data-theme` attribute to the root element so CSS `:root` blocks
// can pick it up if we ever add global CSS.
//
// Dark-mode tokens:
//   bg        #1F2937   (Tailwind `gray-800`)
//   cards     #374151   (Tailwind `gray-700`)
//   text      #F9FAFB   (Tailwind `gray-50`)
//   accent    #7C3AED   (unchanged from light)
//
// The spec above comes from the Phase 6 task description.

export interface ThemePalette {
  readonly accent: string;
  readonly accentHover: string;
  readonly bg: string;
  readonly bgSoft: string;
  readonly bgCanvas: string;
  readonly border: string;
  readonly text: string;
  readonly textMuted: string;
  readonly textInverse: string;
  readonly success: string;
  readonly danger: string;
}

export type ThemeId = "light" | "dark";

export const LIGHT_PALETTE: ThemePalette = {
  accent: "#7C3AED",
  accentHover: "#6D28D9",
  bg: "#FFFFFF",
  bgSoft: "#F5F3FF",
  bgCanvas: "#1e1e1e",
  border: "#E5E7EB",
  text: "#111827",
  textMuted: "#4B5563",
  textInverse: "#FFFFFF",
  success: "#16A34A",
  danger: "#DC2626",
};

export const DARK_PALETTE: ThemePalette = {
  accent: "#7C3AED",
  accentHover: "#6D28D9",
  bg: "#1F2937",
  bgSoft: "#374151",
  bgCanvas: "#111827",
  border: "#4B5563",
  text: "#F9FAFB",
  textMuted: "#9CA3AF",
  textInverse: "#111827",
  success: "#22C55E",
  danger: "#EF4444",
};

export function paletteFor(id: ThemeId): ThemePalette {
  return id === "dark" ? DARK_PALETTE : LIGHT_PALETTE;
}
