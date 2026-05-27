// KChat design tokens. Mirrored here as TypeScript constants so
// components can compose inline styles without dragging in a CSS-in-JS
// library. The colour values are CSS-variable references; the
// concrete palette is defined in `index.html`'s `:root` and
// `:root[data-theme="dark"]` blocks and switched at runtime by
// `ThemeProvider` setting the `data-theme` attribute.
//
// This indirection means *every* `style={{ color: colors.text }}`
// pattern in the renderer automatically participates in light/dark
// theming without any per-component refactor — the browser
// re-evaluates `var(...)` whenever the cascade changes.

export const colors = {
  accent: "var(--kc-accent)",
  accentHover: "var(--kc-accent-hover)",
  bg: "var(--kc-bg)",
  bgSoft: "var(--kc-bg-soft)",
  bgCanvas: "var(--kc-bg-canvas)",
  border: "var(--kc-border)",
  text: "var(--kc-text)",
  textMuted: "var(--kc-text-muted)",
  textInverse: "var(--kc-text-inverse)",
  /// Used by the Phase 3 presence indicator for "peer is online
  /// and has broadcast presence in the past few seconds".
  success: "var(--kc-success)",
  /// Used by error banners and the "leave session" destructive
  /// CTA in the PresencePanel.
  danger: "var(--kc-danger)",
} as const;

export const radius = {
  card: 12,
  pill: 9999,
  /// Small radius for inline form controls (inputs, tags).
  sm: 4,
  /// Medium radius for card-shaped panels nested inside a larger
  /// container — e.g. the section blocks in `PresencePanel`.
  md: 6,
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
