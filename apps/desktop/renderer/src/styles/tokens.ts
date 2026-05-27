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
  /// Low-alpha accent tint for soft chip / pill backgrounds
  /// (PluginManager WASM tag, ScreenshotToLayout drop zone). Use
  /// this instead of template-string concat of `accent` + a hex
  /// alpha suffix — `accent` is a `var(...)` reference, not a hex
  /// literal, so `${colors.accent}22` produces invalid CSS at
  /// runtime. The token carries the alpha baked into the channel
  /// space so dark mode can re-balance the tint without touching
  /// every call site.
  accentBgSoft: "var(--kc-accent-bg-soft)",
  /// Mid-alpha accent halo for focus rings / selection outlines
  /// (TemplatePicker selected card). Same indirection rationale as
  /// `accentBgSoft`.
  accentRing: "var(--kc-accent-ring)",
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
  /// Solid background tint for destructive states (e.g. failed-load
  /// banners in AuditPanel / TemplateMarketplace). Auto-flips for
  /// dark mode via the override in index.html.
  dangerBg: "var(--kc-danger-bg)",
  /// Lower-alpha background tint for destructive accents inside
  /// dense panels (chip/badge backgrounds in PluginManager,
  /// ModelManager, AIAssistPanel).
  dangerBgSoft: "var(--kc-danger-bg-soft)",
  /// Mid-alpha danger border for destructive callout cards
  /// (McpSettingsPanel "MCP off" notice). Same indirection
  /// rationale as `accentBgSoft`.
  dangerBorder: "var(--kc-danger-border)",
  /// Warning accent (preflight warnings, plugin permission badges).
  warn: "var(--kc-warn)",
  warnBg: "var(--kc-warn-bg)",
  warnBgSoft: "var(--kc-warn-bg-soft)",
  /// Info accent (preflight info findings).
  info: "var(--kc-info)",
  infoBg: "var(--kc-info-bg)",
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

// Shadow tokens are CSS variable references so they automatically pick
// up the dark-mode override defined in index.html (`--kc-shadow` and
// `--kc-shadow-hover` are redefined under `:root[data-theme="dark"]`
// with stronger alphas to match the darker substrate).
export const shadow = {
  card: "var(--kc-shadow)",
  cardHover: "var(--kc-shadow-hover)",
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
