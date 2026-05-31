# Brewline v2 demo — CHANGELOG

What changed between the original Brewline Coffee walkthrough at
[`docs/demo/brewline`](../brewline) and this v2 re-run. Sections
follow the three PR tracks landed in
[#31](https://github.com/kennguy3n/KCreate/pull/31).

## Track 1 — Lucide SVG icons across the UI

### HomePage

- Every `Create new` card now carries a 40×40 tinted square with a
  20px Lucide icon (`monitor`, `palette`, `image`, `camera`,
  `presentation`, `printer`, `code`, `upload`) instead of starting
  blank.
- The brief card carries an `AI` mark in the existing `BriefTile`
  style.
- Header now leads with a `pen-tool` logo mark.

### TopBar

- `Select` / `Rect` / `Ellipse` / `Line` / `Text` tool buttons
  swapped from text-only labels to `mouse-pointer` / `square` /
  `circle` / `minus` / `type` Lucide glyphs (label still shown for
  the active tool).
- Home / Undo / Redo / Export buttons gained `arrow-left`, `undo`,
  `redo`, `download` icons.
- Theme toggle ☼/☽ replaced by `sun` / `moon` icons.

### LeftPanel

- Layer-row chips: `●` / `○` (visible / hidden) → `eye` / `eye-off`;
  `⌧` / `⌬` (locked / unlocked) → `lock` / `unlock`; `×` (delete) →
  `trash-2`; `×` (filter clear) → `x`.
- Node-type abbreviations gained adjacent type icons so power-users
  can still scan by glyph.

### RightPanel

- Tab strip: `Properties` / `Effects` / `AI Assist` / `Export` /
  `Inspect` / `History` / `Accessibility` / `Color` / `Presence` /
  `Constraints` / `Tokens` / `Publish` / `Encryption` /
  `Preflight` / `Interaction` all gained a 14px icon next to the
  text label.

### ExportPanel

- The five batch-preset rows (`Web Assets`, `Social Pack`,
  `Icon Pack`, `Print Ready`, `Developer Handoff`) gained
  `globe` / `share` / `grid-2x2` / `printer` / `code` icons.

### Implementation notes

- One reusable `<Icon name="…" size={…} />` component
  (`apps/desktop/renderer/src/components/Icon.tsx`) inlines paths
  from a bundled registry (`components/iconRegistry.ts`).
- All icons fill / stroke with `currentColor` so they inherit the
  active theme's foreground colour automatically (no per-icon
  light/dark forks).

## Track 2 — Template scaffolding

The HomePage cards no longer drop the user on an empty canvas. After
`artboard.create()` succeeds in `App.tsx::handleOpenEditor`, the
template resolver for the picked card runs and seeds named, palette-
coloured, deterministically-positioned starter nodes.

### Resolvers shipped

- `BRAND_RESOLVER` — brand kit (12 nodes: title, tagline, four
  palette swatches with four matching labels, logo placeholder +
  caption).
- `SOCIAL_RESOLVER` — Instagram post (5 nodes: cream background,
  burnt-orange headline band, headline, body copy, sage accent).
- `PRINT_RESOLVER` — A4 print layout (6 nodes: espresso header bar,
  document title, cream body block, two body paragraphs, footer
  accent).
- `APP_UI_RESOLVER` — app shell (11 nodes: background, left rail,
  header bar + title, three content tiles + tile headings).
- `PHOTO_RESOLVER` — single drop-zone rect clamped to the short
  artboard side (so it stays square on non-square presets).
- `DECK_RESOLVER` — pitch deck slide (6 nodes: background, title,
  subtitle, two columns sharing a clamped colHeight).
- `DEV_EXPORT_RESOLVER` — single artboard label with the requested
  grid hint.

### Shared layout primitives

- `TemplateContext` carries `{ x, y, width, height }` so each
  resolver works against the bridge's reported artboard offset
  rather than assuming `(0, 0)`. Critical for non-first artboards
  (the bridge stacks fresh artboards on the X axis).
- Resolvers all go through `rect()` / `text()` helpers that merge
  the `fill` + `name` updates into a single `updateNode` IPC call.

### Math invariants (round-3)

All three rectangular-budget resolvers now share the same defensive
shape, matching `DECK_RESOLVER`'s documented invariant:

- `Math.floor(...)` on the inner width so the per-row budget can
  never be over-allocated by ±1 px even at unfavourable artboard
  sizes (`BRAND` swatchWidth, `APP_UI` tileWidth, `DECK` colWidth).
- `Math.max(0, ...)` on the inner height so a degenerate short
  artboard never passes a negative `h` into `createRect`
  (`APP_UI` tileHeight, `DECK` colHeight).

The Brewline-v2 demo screenshots prove these invariants on the
actual shipped 1024×1024 brand preset and the 1440×900 desktop
preset; the test suite covers a degenerate 1440×50 surface for the
`APP_UI` tileHeight clamp.

## Track 3 — Demo re-run

- 25 fresh screenshots replacing the v1 set, all captured against
  the same headless Linux / CPU-only / no-network constraints
  documented in the original demo.
- Coverage now spans **every** shipped template card (v1 only
  exercised one) and shows the seeded geometry via the Inspect-mode
  code-gen output, so the demo doubles as a regression artefact for
  the layout invariants.
- Status-bar copy is captured for each Export batch preset
  (`N files · M bytes → /tmp`) and on-disk file listings are
  enumerated in the README so reviewers don't need to re-run the
  demo to confirm the export pipeline still produces all 15
  expected artifacts.
- `Preflight: 0 errors, 0 warnings, 1 info` runs cleanly on the
  empty seed project at the 300 DPI / 3 mm bleed / 300% max-ink
  default profile.

## Pre-existing capabilities re-verified in v2

These were already present in the v1 demo; v2 re-confirms they
still work with the Track-1/Track-2 changes layered on top.

- **Inspect mode** — `CSS` / `Tailwind` / `React style` outputs
  (from `kcreate_export::code_gen`) all produce live values for
  selected nodes.
- **Export batch presets** — all five chain runs succeed; total of
  15 files (PNG / SVG / PDF / JSON) written to `/tmp`.
- **Prototype mode** — responsive preview (Desktop 1440 / Tablet
  768 / Mobile 375) plus `InteractionPanel` add-and-Play flow.
- **Color management** — Working RGB / CMYK / rendering intent /
  soft-proof / gamut warning controls.
- **Preflight** — Print Preflight panel runs `kcreate_export::preflight`
  with the documented info note about empty per-page metadata.

## Repository-relative paths

- New code:
  - `apps/desktop/renderer/src/components/Icon.tsx` (+ `iconRegistry.ts`)
  - `apps/desktop/renderer/src/lib/templates.ts`
  - `apps/desktop/renderer/tests/templates.test.mjs`
- Touched UI:
  - `apps/desktop/renderer/src/components/TopBar.tsx`
  - `apps/desktop/renderer/src/components/LeftPanel.tsx`
  - `apps/desktop/renderer/src/components/RightPanel.tsx`
  - `apps/desktop/renderer/src/components/ExportPanel.tsx`
  - `apps/desktop/renderer/src/pages/HomePage.tsx`
  - `apps/desktop/renderer/src/App.tsx`
- This demo:
  - `docs/demo/brewline-v2/README.md`
  - `docs/demo/brewline-v2/CHANGELOG.md` (this file)
  - `docs/demo/brewline-v2/screenshots/` (25 PNGs)

## Test coverage delta

The test suite for `templates.ts` runs as part of
`pnpm --filter @kcreate/desktop test`. v2 adds two cases on top of
the v1 demo's coverage:

| Test                                                                                                          | Purpose                                                                                                   |
| ------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------- |
| `BRAND resolver lays out cleanly on the actual 1024×1024 brand preset` (extended)                              | Now also asserts the round-3 floor invariant: `4 * swatchWidth + 3 * swatchGap ≤ swatchRowWidth`         |
| `APP_UI resolver clamps tileHeight to >= 0 on a degenerate short artboard`                                     | Exercises a 1440×50 surface where the unclamped value would be `–18`; asserts every content tile `h ≥ 0`. |

Final result: `12 / 12 tests pass`, no clippy / lint / typecheck
warnings.
