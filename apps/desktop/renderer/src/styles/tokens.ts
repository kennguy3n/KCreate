// KChat design tokens. Mirrored here as TypeScript constants so
// components can compose inline styles without dragging in a CSS-in-JS
// library. Keep these in lockstep with the `:root` block in
// `index.html`.

export const colors = {
  accent: "#7C3AED",
  accentHover: "#6D28D9",
  bg: "#FFFFFF",
  bgSoft: "#F5F3FF",
  bgCanvas: "#1e1e1e",
  border: "#E5E7EB",
  text: "#111827",
  textMuted: "#4B5563",
  textInverse: "#FFFFFF",
} as const;

export const radius = {
  card: 12,
  pill: 9999,
} as const;

export const shadow = {
  card: "0 1px 3px rgba(0, 0, 0, 0.08)",
  cardHover: "0 4px 12px rgba(124, 58, 237, 0.18)",
} as const;

export const spacing = {
  xs: 4,
  sm: 8,
  md: 16,
  lg: 24,
  xl: 32,
} as const;

export const font = {
  family:
    'Inter, -apple-system, system-ui, "Segoe UI", Roboto, sans-serif',
} as const;
