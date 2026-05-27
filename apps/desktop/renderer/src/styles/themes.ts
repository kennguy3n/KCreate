// Phase 6 Tasks 23-24: theme identifier.
//
// The renderer's theming model is *CSS-variable driven*: the actual
// colour values live in `index.html` under `:root` (light) and
// `:root[data-theme="dark"]` (dark) and components read them through
// `var(--kc-*)` references exported from `./tokens.ts`. The
// `ThemeProvider` only needs to know the current id so it can write
// the `data-theme` attribute on the document element — the browser
// then re-evaluates every `var(...)` in the cascade automatically.
//
// We deliberately *don't* duplicate the palette here as a JS object.
// Doing so would create two sources of truth that would drift the
// instant someone tweaked a CSS variable without also updating the
// JS palette (or vice versa); keeping the palette in CSS only makes
// the dark-mode contract a single edit site.

export type ThemeId = "light" | "dark";
