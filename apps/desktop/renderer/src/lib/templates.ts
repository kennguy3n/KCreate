// Template scaffolding — Track 2 of the HomePage → editor flow.
//
// When the user clicks a card on `HomePage.tsx` we:
//   1. Materialise a fresh scratch project on disk
//      (`scratchProject.openScratchProject()`).
//   2. Create the artboard preset wired to that card
//      (`window.kcreate.artboard.create`).
//   3. Apply the matching `TemplateResolver` from this module to
//      seed starter content (text nodes, swatches, header bars,
//      logo placeholder, …) so the user lands on a populated
//      canvas instead of a blank artboard.
//
// Resolvers are pure data + a single `apply()` entry point that
// drives the existing `window.kcreate.canvas.*` and
// `window.kcreate.document.*` bridges. They never touch the IPC
// shape directly — that lives in `preload/src/preload.ts`. Errors
// inside a resolver are non-fatal: the caller (`App.tsx`) wraps
// `.apply()` in try/catch so a bridge hiccup still lands the user
// in the editor, just with a blank canvas they can recover from
// manually.

import type { FillStyle, RgbaColor } from "../../../shared/scene";

/// Resolver context — what the bridge gives us *after* the
/// artboard has been created. We pass the artboard's world-space
/// rect so resolvers can position seed content relative to the
/// canvas without re-deriving dimensions from the home-page
/// option (which would silently break if a resolver were applied
/// to a custom artboard size in a future surface).
export interface TemplateContext {
  /// Artboard width (px, world space). Matches the `width`
  /// argument passed to `window.kcreate.artboard.create`.
  width: number;
  /// Artboard height (px, world space).
  height: number;
  /// Artboard top-left X in world space. The bridge places fresh
  /// artboards at `(0, 0)` for the first card and offsets each
  /// subsequent one, so the resolver reads this off `artboard.list`
  /// rather than assuming `(0, 0)`.
  x: number;
  /// Artboard top-left Y in world space.
  y: number;
}

/// Apply a resolver: drive the bridge to seed starter content
/// inside the artboard described by `ctx`. Returns once every
/// bridge call has resolved so the caller can refresh the scene
/// tree afterwards.
export interface TemplateResolver {
  apply(ctx: TemplateContext): Promise<void>;
}

/// Convert a 0xRRGGBB hex literal to an `RgbaColor` (channels in
/// `[0.0, 1.0]`). Mirrors the conversion the FillEditor does for
/// solid swatches. The parameter is named `value` rather than `hex`
/// so it doesn't shadow the function name (`no-shadow`).
function hex(value: string, alpha = 1.0): RgbaColor {
  const raw = value.startsWith("#") ? value.slice(1) : value;
  const r = parseInt(raw.slice(0, 2), 16) / 255;
  const g = parseInt(raw.slice(2, 4), 16) / 255;
  const b = parseInt(raw.slice(4, 6), 16) / 255;
  return { r, g, b, a: alpha };
}

function solidFill(rgb: string, alpha = 1.0): FillStyle {
  return { kind: "solid", ...hex(rgb, alpha) };
}

/// Apply `fill` and/or `name` to a freshly created node in a single
/// `updateNode` IPC round-trip. Both fields live on `UpdateNodeProps`
/// (see `apps/desktop/shared/scene.ts`) and the bridge merges any
/// subset of props in one shot, so issuing two sequential calls — one
/// for `fill`, one for `name` — was pure latency overhead on the
/// editor boot path. Each template resolver creates 5–12 nodes with
/// both fields set, so combining halves the IPC traffic on the
/// HomePage → editor transition.
///
/// Returns immediately (no IPC) when neither field is supplied; the
/// caller may have legitimate reasons to seed an anonymous, unfilled
/// node (e.g. a temporary measurement guide).
///
/// The presence checks use explicit `=== null` / `=== undefined`
/// instead of truthiness so an empty-string `name` (a legitimate way
/// to clear an existing name) still issues the IPC. Truthy checks
/// would silently drop `name: ""` and `name: undefined` alike, which
/// is fine for every current call site but a footgun for any future
/// resolver that genuinely wants to blank out a node's name.
async function paint(
  nodeId: string,
  fill: FillStyle | null,
  name: string | undefined,
): Promise<void> {
  if (fill === null && name === undefined) return;
  const props: { fill?: FillStyle; name?: string } = {};
  if (fill !== null) props.fill = fill;
  if (name !== undefined) props.name = name;
  await window.kcreate.document.updateNode(nodeId, props);
}

async function rect(
  x: number,
  y: number,
  w: number,
  h: number,
  fill: FillStyle | null = null,
  name?: string,
): Promise<string> {
  const id = await window.kcreate.canvas.createRect(null, x, y, w, h);
  await paint(id, fill, name);
  return id;
}

async function text(
  x: number,
  y: number,
  body: string,
  size: number,
  fill: FillStyle | null = null,
  family = "sans-serif",
  name?: string,
): Promise<string> {
  const id = await window.kcreate.canvas.createText(
    null,
    x,
    y,
    body,
    family,
    size,
  );
  await paint(id, fill, name);
  return id;
}

/// Brewline-flavoured brand palette. Reused by the demo +
/// integration tests so the "no-longer-blank-page" assertion has
/// a stable colour set to look for.
export const BRAND_PALETTE = {
  espresso: "#3E2723",
  cream: "#FFF8E1",
  burntOrange: "#E65100",
  sage: "#689F63",
  ink: "#111827",
  paper: "#F8FAFC",
} as const;

const BRAND_RESOLVER: TemplateResolver = {
  async apply(ctx) {
    // Heading + tagline anchored to the top of the artboard.
    const { x: ax, y: ay, width: aw, height: ah } = ctx;
    const margin = Math.round(aw * 0.06);
    await text(
      ax + margin,
      ay + margin,
      "Brand Name",
      64,
      solidFill(BRAND_PALETTE.ink),
      "sans-serif",
      "Brand title",
    );
    await text(
      ax + margin,
      ay + margin + 96,
      "Tagline goes here",
      24,
      solidFill(BRAND_PALETTE.espresso),
      "sans-serif",
      "Tagline",
    );

    // Palette swatch row across the middle of the canvas. Four
    // even-width swatches with a small horizontal gap; total row
    // width = artboard - 2*margin. Each swatch is square-ish
    // (height = swatch width).
    //
    // `Math.floor` (not `Math.round`) so the four-swatch budget is
    // never over-allocated: `4 * swatchWidth + 3 * swatchGap <=
    // swatchRowWidth`. With `Math.round` an odd numerator could round
    // the quotient up by 0.5 on each of the four swatches, costing up
    // to ~2px on the row's right edge. Today the shipped 1024×1024
    // brand preset has slack — `4*214 + 3*15 = 901 <= 902` even with
    // rounding — so this is defensive, not load-bearing. But the
    // floor invariant `4*swatchWidth + 3*swatchGap <= swatchRowWidth`
    // then holds for any artboard size a future surface might apply
    // this resolver to, mirroring the pattern already in use on
    // `APP_UI_RESOLVER` (line 389) and `DECK_RESOLVER` (line 519).
    const swatchGap = Math.round(aw * 0.015);
    const swatchRowWidth = aw - margin * 2;
    const swatchWidth = Math.floor(
      (swatchRowWidth - swatchGap * 3) / 4,
    );
    const swatchY = ay + Math.round(ah * 0.45);

    // `swatchHeight` is the *vertical* budget for one swatch tile. The
    // resolver was originally designed for the shipped 1024×1024 brand
    // preset where `ah ≈ aw` and a square tile (`swatchHeight =
    // swatchWidth`) fits between `swatchY` and the logo placeholder
    // anchored at `ay + ah - margin - logoSize`. On a wider artboard
    // (e.g. 1920×1080 with `swatchWidth = 400`, `swatchY = 486`,
    // `logoY = 658`) the square would extend down to y=886 and collide
    // with the logo block at y=658..965 — a real visual regression
    // surfaced by Devin Review on commit 55afb7b.
    //
    // Clamp to the available vertical budget so the swatch row + its
    // labels never collide with the logo placeholder, regardless of
    // artboard aspect ratio. The reserved space below the swatch row
    // accounts for the 16px gap before the label (`swatchY +
    // swatchHeight + 16`), the label's ~20px text height, and an 8px
    // safety margin between the label baseline and the logo top.
    // `Math.max(0, …)` mirrors the `APP_UI_RESOLVER` tileHeight clamp
    // (line 396) so a degenerate short artboard never passes a
    // negative height into `createRect`.
    const logoSize = Math.round(aw * 0.16);
    const logoTopRelative = ah - margin - logoSize;
    const swatchTopRelative = Math.round(ah * 0.45);
    const labelGap = 16;
    const labelHeight = 20;
    const labelToLogoSafety = 8;
    const availableSwatchHeight =
      logoTopRelative - swatchTopRelative - labelGap - labelHeight - labelToLogoSafety;
    const swatchHeight = Math.max(
      0,
      Math.min(swatchWidth, availableSwatchHeight),
    );
    const swatches: ReadonlyArray<{ label: string; hex: string }> = [
      { label: "Espresso", hex: BRAND_PALETTE.espresso },
      { label: "Cream", hex: BRAND_PALETTE.cream },
      { label: "Burnt orange", hex: BRAND_PALETTE.burntOrange },
      { label: "Sage", hex: BRAND_PALETTE.sage },
    ];
    for (let i = 0; i < swatches.length; i += 1) {
      // Bounds-checked above by the loop guard, but TypeScript's
      // `noUncheckedIndexedAccess` widens the array element to
      // `T | undefined`. Narrow back via the runtime guard so the
      // body below can address `s` without an `!` non-null assert.
      const s = swatches[i];
      if (!s) continue;
      const sx = ax + margin + i * (swatchWidth + swatchGap);
      await rect(
        sx,
        swatchY,
        swatchWidth,
        swatchHeight,
        solidFill(s.hex),
        `Palette / ${s.label}`,
      );
      await text(
        sx,
        swatchY + swatchHeight + 16,
        s.label,
        20,
        solidFill(BRAND_PALETTE.ink),
        "sans-serif",
        `Swatch label / ${s.label}`,
      );
    }

    // Logo placeholder mark anchored to the bottom-left. Rendered
    // as a filled circle (espresso) with a light ring so it reads
    // as a logomark target rather than a content rectangle. `logoSize`
    // was hoisted above the swatch row to participate in the
    // `availableSwatchHeight` clamp; reuse it here so the geometry
    // stays in lockstep.
    const logoX = ax + margin;
    const logoY = ay + logoTopRelative;
    await rect(
      logoX,
      logoY,
      logoSize,
      logoSize,
      solidFill(BRAND_PALETTE.espresso),
      "Logo placeholder",
    );

    // Caption placement: by default the caption sits to the *right* of
    // the logomark, vertically centred — the visual the shipped
    // 1024×1024 brand preset was designed around. On a narrow brand
    // artboard the right-of-logo placement would push the caption past
    // the right edge of the canvas (Devin Review surfaced this on
    // commit cb2c097: at 300×300 the caption starts at x=90 and the
    // estimated 247px-wide text ends at x=337 — 55px past the
    // artboard's 282px right margin). Detect that case and fall back
    // to placing the caption *above* the logomark, left-aligned with
    // it, which preserves the caption's role as a labelling tag
    // without overflowing either axis (the logo is anchored to the
    // bottom-left margin, so there's always vertical room above it).
    //
    // Width estimate (`charCount × fontSize × 0.55`) is a conservative
    // approximation for proportional sans-serif text — exact glyph
    // widths aren't available at this layer (the renderer's shaped
    // text metrics live behind the bridge in `kcreate_text`). The
    // estimate intentionally rounds up: a slight over-estimate flips
    // borderline cases to the safer above-logo placement.
    const captionFontSize = 24;
    const captionText = "Drop your mark here";
    const captionEstimatedWidth = Math.ceil(
      captionText.length * captionFontSize * 0.55,
    );
    const captionRightOfLogoX = logoX + logoSize + 24;
    const artboardRightEdge = ax + aw - margin;
    const captionFitsRightOfLogo =
      captionRightOfLogoX + captionEstimatedWidth <= artboardRightEdge;
    const captionX = captionFitsRightOfLogo
      ? captionRightOfLogoX
      : logoX;
    const captionY = captionFitsRightOfLogo
      ? logoY + Math.round(logoSize / 2) - 14
      : logoY - captionFontSize - 8;
    await text(
      captionX,
      captionY,
      captionText,
      captionFontSize,
      solidFill(BRAND_PALETTE.espresso),
      "sans-serif",
      "Logo caption",
    );
  },
};

const SOCIAL_RESOLVER: TemplateResolver = {
  async apply(ctx) {
    const { x: ax, y: ay, width: aw, height: ah } = ctx;
    // Solid cream background covering the full artboard so the
    // template reads as "designed" instead of "blank canvas".
    await rect(ax, ay, aw, ah, solidFill(BRAND_PALETTE.cream), "Background");
    // Burnt-orange band behind the headline to anchor the type.
    const bandHeight = Math.round(ah * 0.28);
    const bandY = ay + Math.round(ah * 0.18);
    await rect(
      ax,
      bandY,
      aw,
      bandHeight,
      solidFill(BRAND_PALETTE.burntOrange),
      "Headline band",
    );
    await text(
      ax + Math.round(aw * 0.08),
      bandY + Math.round(bandHeight * 0.25),
      "Your Headline",
      48,
      solidFill(BRAND_PALETTE.cream),
      "sans-serif",
      "Headline",
    );
    await text(
      ax + Math.round(aw * 0.08),
      bandY + bandHeight + 48,
      "Add your message here",
      20,
      solidFill(BRAND_PALETTE.espresso),
      "sans-serif",
      "Body copy",
    );
    // Sage accent dot bottom-right (placeholder for a brand mark).
    const dotSize = Math.round(aw * 0.08);
    await rect(
      ax + aw - dotSize - Math.round(aw * 0.06),
      ay + ah - dotSize - Math.round(ah * 0.06),
      dotSize,
      dotSize,
      solidFill(BRAND_PALETTE.sage),
      "Accent",
    );
  },
};

const PRINT_RESOLVER: TemplateResolver = {
  async apply(ctx) {
    const { x: ax, y: ay, width: aw, height: ah } = ctx;
    // Header bar across the top in espresso brown.
    const headerHeight = Math.round(ah * 0.16);
    await rect(
      ax,
      ay,
      aw,
      headerHeight,
      solidFill(BRAND_PALETTE.espresso),
      "Header bar",
    );
    const margin = Math.round(aw * 0.06);
    await text(
      ax + margin,
      ay + Math.round(headerHeight * 0.3),
      "Document Title",
      72,
      solidFill(BRAND_PALETTE.cream),
      "sans-serif",
      "Document title",
    );
    // Body placeholder block in cream.
    await rect(
      ax + margin,
      ay + headerHeight + margin,
      aw - margin * 2,
      Math.round(ah * 0.5),
      solidFill(BRAND_PALETTE.cream),
      "Body block",
    );
    await text(
      ax + margin * 2,
      ay + headerHeight + margin * 2,
      "Body copy goes here. Replace this placeholder with",
      28,
      solidFill(BRAND_PALETTE.ink),
      "sans-serif",
      "Body paragraph",
    );
    await text(
      ax + margin * 2,
      ay + headerHeight + margin * 2 + 40,
      "your real content. The header bar above is editable too.",
      28,
      solidFill(BRAND_PALETTE.ink),
      "sans-serif",
      "Body paragraph 2",
    );
    // Footer accent strip in burnt orange.
    const footerHeight = Math.round(ah * 0.04);
    await rect(
      ax,
      ay + ah - footerHeight,
      aw,
      footerHeight,
      solidFill(BRAND_PALETTE.burntOrange),
      "Footer accent",
    );
  },
};

const APP_UI_RESOLVER: TemplateResolver = {
  async apply(ctx) {
    const { x: ax, y: ay, width: aw, height: ah } = ctx;
    // App-shell shell with a left rail, header, and content area.
    const rail = Math.round(aw * 0.12);
    const headerH = Math.round(ah * 0.08);
    await rect(ax, ay, aw, ah, solidFill(BRAND_PALETTE.paper), "App background");
    await rect(ax, ay, rail, ah, solidFill(BRAND_PALETTE.espresso), "Left rail");
    await rect(
      ax + rail,
      ay,
      aw - rail,
      headerH,
      solidFill(BRAND_PALETTE.cream),
      "Header",
    );
    await text(
      ax + rail + 32,
      ay + Math.round(headerH * 0.3),
      "App / Website UI",
      28,
      solidFill(BRAND_PALETTE.ink),
      "sans-serif",
      "Header title",
    );
    // Three content tiles in the body area. `Math.floor` (not
    // `Math.round`) so the three-tile budget is never over-allocated,
    // mirroring the documented rationale on `DECK_RESOLVER`'s
    // `colWidth` (see `templates.ts:493-503`). Today the APP_UI
    // formula has slack — `rail + 4*tileMargin + 3*tileWidth ≤ aw`
    // leaves ~`tileMargin` of right padding even with `Math.round`'s
    // worst-case +1 — so this is defensive, not load-bearing. But the
    // floor invariant `rail + 4*tileMargin + 3*tileWidth ≤ aw` then
    // holds for any artboard size a future surface might apply this
    // resolver to, without re-deriving the rounding behaviour.
    const tileMargin = 32;
    const tileTop = ay + headerH + tileMargin;
    const tileBottom = ay + ah - tileMargin;
    const tileWidth = Math.floor((aw - rail - tileMargin * 4) / 3);
    // `Math.max(0, ...)` so a future surface applying this resolver
    // to a very short artboard (`ah < headerH + 2*tileMargin`) doesn't
    // pass a negative height into `createRect`. Mirrors the documented
    // clamp on `DECK_RESOLVER`'s `colHeight` (see line 508). Today the
    // shipped 1440×900 app-ui preset always yields a positive value
    // (~764px), so this is defensive, not load-bearing.
    const tileHeight = Math.max(0, tileBottom - tileTop);
    for (let i = 0; i < 3; i += 1) {
      const tx = ax + rail + tileMargin + i * (tileWidth + tileMargin);
      await rect(
        tx,
        tileTop,
        tileWidth,
        tileHeight,
        solidFill(BRAND_PALETTE.cream),
        `Content tile ${i + 1}`,
      );
      await text(
        tx + 24,
        tileTop + 24,
        `Section ${i + 1}`,
        22,
        solidFill(BRAND_PALETTE.ink),
        "sans-serif",
        `Tile heading ${i + 1}`,
      );
    }
  },
};

const PHOTO_RESOLVER: TemplateResolver = {
  async apply(ctx) {
    const { x: ax, y: ay, width: aw, height: ah } = ctx;
    // Checkerboard-style background hint so the user can tell
    // we're inside the artboard before they drop a photo in.
    await rect(ax, ay, aw, ah, solidFill(BRAND_PALETTE.cream), "Photo backdrop");
    // Margin is taken off the SHORT side so the inner drop-zone
    // square fits even on portrait/landscape artboards. The shipped
    // Photo preset is 2048×2048 (square), so on the happy path this
    // collapses to `aw * 0.1` (the previous behaviour). The reason
    // we centre the inner square inside the artboard is so a future
    // 3000×2000 "photo cleanup" preset — or any custom artboard the
    // user resizes to a non-square aspect — doesn't get a drop zone
    // that overflows the shorter axis. Mirrors the defensive
    // `Math.min` clamp `DECK_RESOLVER` applies to its title reserve
    // (see `titleReserve` below).
    const shortSide = Math.min(aw, ah);
    const margin = Math.round(shortSide * 0.1);
    const innerSize = Math.max(0, shortSide - margin * 2);
    // Centre the square inside the artboard so unused space lands on
    // both sides of the dominant axis instead of below/right of the
    // drop zone.
    const innerX = ax + Math.round((aw - innerSize) / 2);
    const innerY = ay + Math.round((ah - innerSize) / 2);
    await rect(
      innerX,
      innerY,
      innerSize,
      innerSize,
      solidFill(BRAND_PALETTE.paper),
      "Drop zone",
    );
    await text(
      innerX + 48,
      innerY + 48,
      "Import a photo",
      48,
      solidFill(BRAND_PALETTE.espresso),
      "sans-serif",
      "Drop zone heading",
    );
    await text(
      innerX + 48,
      innerY + 120,
      "Use AI Assist \u2192 Background removal once imported.",
      22,
      solidFill(BRAND_PALETTE.ink),
      "sans-serif",
      "Drop zone hint",
    );
  },
};

const DECK_RESOLVER: TemplateResolver = {
  async apply(ctx) {
    const { x: ax, y: ay, width: aw, height: ah } = ctx;
    await rect(ax, ay, aw, ah, solidFill(BRAND_PALETTE.paper), "Slide background");
    const margin = Math.round(aw * 0.06);
    // Title block.
    await text(
      ax + margin,
      ay + margin,
      "Pitch Deck Title",
      80,
      solidFill(BRAND_PALETTE.ink),
      "sans-serif",
      "Slide title",
    );
    await text(
      ax + margin,
      ay + margin + 110,
      "Subtitle or short positioning line",
      32,
      solidFill(BRAND_PALETTE.espresso),
      "sans-serif",
      "Subtitle",
    );
    // Two-column body for talking points. `colTop` already includes
    // the artboard Y-offset (`ay`), so the closing edge of the
    // column also has to include `ay` — otherwise a non-zero
    // artboard origin (the bridge offsets every artboard after the
    // first) yields a negative height. Mirrors the shape used by
    // `APP_UI_RESOLVER` (`tileBottom = ay + ah - tileMargin`,
    // `tileHeight = tileBottom - tileTop`).
    //
    // The 220px title-block reserve fits a 1920×1080 slide (the
    // shipped deck preset, ~20% of `ah`). Clamping it to at most
    // 30% of `ah` keeps the columns visible if this resolver is
    // ever applied to a smaller custom artboard surface — without
    // that bound a 300px-tall artboard would have a negative
    // `colHeight` and the two background rects would silently
    // collapse to zero-size nodes.
    const titleReserve = Math.min(220, Math.round(ah * 0.3));
    const colTop = ay + margin + titleReserve;
    const colHeight = Math.max(0, ay + ah - colTop - margin);
    // `Math.floor` (not `Math.round`) so the two-column budget is
    // never over-allocated: `2 * colWidth + 3 * margin <= aw`. With
    // `Math.round` an odd `(aw - 3*margin)` rounds the half up, e.g.
    // on 1920×1080: `margin=115`, `(aw - 3*margin)=1575`, `1575/2 ⇒
    // 788` (round) vs `787` (floor). 2*788 + 3*115 = 1921 > 1920;
    // 2*787 + 3*115 = 1919 < 1920. Visually invisible because the
    // overflow is one sub-pixel on the right margin, but the floor
    // gives a clean invariant any future caller (e.g. someone adding
    // a third column with `n * colWidth + (n+1) * margin <= aw`) can
    // rely on without re-deriving the rounding behaviour.
    const colWidth = Math.floor((aw - margin * 3) / 2);
    await rect(
      ax + margin,
      colTop,
      colWidth,
      colHeight,
      solidFill(BRAND_PALETTE.cream),
      "Column A",
    );
    await rect(
      ax + margin * 2 + colWidth,
      colTop,
      colWidth,
      colHeight,
      solidFill(BRAND_PALETTE.cream),
      "Column B",
    );
  },
};

const DEV_EXPORT_RESOLVER: TemplateResolver = {
  async apply(ctx) {
    const { x: ax, y: ay, width: aw, height: ah } = ctx;
    // The dev-export preset is the icon-pack starting point, so we
    // frame the canvas like an icon preview surface (filled body
    // with a centred notch) and label the artboard with the export
    // grid size. `iconGridSizeHint` is purely a label — nothing
    // below snaps to it. Naming it `gridHint` (rather than `grid`)
    // keeps that boundary explicit so a future contributor doesn't
    // assume the rectangles below are grid-aligned.
    const iconGridSizeHint = 64;
    await rect(ax, ay, aw, ah, solidFill(BRAND_PALETTE.paper), "Icon backdrop");
    const inset = Math.round(aw * 0.12);
    await rect(
      ax + inset,
      ay + inset,
      aw - inset * 2,
      ah - inset * 2,
      solidFill(BRAND_PALETTE.burntOrange),
      "Icon body",
    );
    // Inner notch so the icon body has visual content out of the box.
    const notch = Math.round((aw - inset * 2) * 0.35);
    await rect(
      ax + Math.round(aw / 2) - Math.round(notch / 2),
      ay + Math.round(ah / 2) - Math.round(notch / 2),
      notch,
      notch,
      solidFill(BRAND_PALETTE.cream),
      "Icon notch",
    );
    await text(
      ax + 16,
      ay + 16,
      `${aw}×${ah}\u00a0\u00b7\u00a0${iconGridSizeHint}px grid`,
      18,
      solidFill(BRAND_PALETTE.espresso),
      "sans-serif",
      "Spec caption",
    );
  },
};

/// Resolver registry keyed by `CREATE_OPTIONS.id`. The "import"
/// card is intentionally absent because it has no
/// `defaultArtboard` and the user supplies the file
/// (`null` here means "do nothing"; `App.tsx` checks for presence).
export const TEMPLATE_RESOLVERS: Readonly<
  Record<string, TemplateResolver | undefined>
> = {
  brand: BRAND_RESOLVER,
  social: SOCIAL_RESOLVER,
  print: PRINT_RESOLVER,
  "app-ui": APP_UI_RESOLVER,
  photo: PHOTO_RESOLVER,
  deck: DECK_RESOLVER,
  "dev-export": DEV_EXPORT_RESOLVER,
};

/// Look up the resolver for a `CREATE_OPTIONS.id`. Returns
/// `undefined` for ids without a seed (the "import" card). The
/// caller treats `undefined` and a resolver that throws
/// identically — neither path blocks the editor from opening.
export function templateResolverFor(
  jobKind: string,
): TemplateResolver | undefined {
  return TEMPLATE_RESOLVERS[jobKind];
}
