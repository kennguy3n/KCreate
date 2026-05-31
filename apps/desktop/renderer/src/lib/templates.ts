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
/// solid swatches.
function hex(hex: string, alpha = 1.0): RgbaColor {
  const raw = hex.startsWith("#") ? hex.slice(1) : hex;
  const r = parseInt(raw.slice(0, 2), 16) / 255;
  const g = parseInt(raw.slice(2, 4), 16) / 255;
  const b = parseInt(raw.slice(4, 6), 16) / 255;
  return { r, g, b, a: alpha };
}

function solidFill(rgb: string, alpha = 1.0): FillStyle {
  return { kind: "solid", ...hex(rgb, alpha) };
}

async function paint(nodeId: string, fill: FillStyle): Promise<void> {
  await window.kcreate.document.updateNode(nodeId, { fill });
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
  if (fill) await paint(id, fill);
  if (name) await window.kcreate.document.updateNode(id, { name });
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
  if (fill) await paint(id, fill);
  if (name) await window.kcreate.document.updateNode(id, { name });
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
    const swatchGap = Math.round(aw * 0.015);
    const swatchRowWidth = aw - margin * 2;
    const swatchWidth = Math.round(
      (swatchRowWidth - swatchGap * 3) / 4,
    );
    const swatchHeight = swatchWidth;
    const swatchY = ay + Math.round(ah * 0.45);
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
    // as a logomark target rather than a content rectangle.
    const logoSize = Math.round(aw * 0.16);
    const logoX = ax + margin;
    const logoY = ay + ah - margin - logoSize;
    await rect(
      logoX,
      logoY,
      logoSize,
      logoSize,
      solidFill(BRAND_PALETTE.espresso),
      "Logo placeholder",
    );
    await text(
      logoX + logoSize + 24,
      logoY + Math.round(logoSize / 2) - 14,
      "Drop your mark here",
      24,
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
    // Three content tiles in the body area.
    const tileMargin = 32;
    const tileTop = ay + headerH + tileMargin;
    const tileBottom = ay + ah - tileMargin;
    const tileWidth = Math.round((aw - rail - tileMargin * 4) / 3);
    const tileHeight = tileBottom - tileTop;
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
    const margin = Math.round(aw * 0.1);
    const innerSize = aw - margin * 2;
    await rect(
      ax + margin,
      ay + margin,
      innerSize,
      innerSize,
      solidFill(BRAND_PALETTE.paper),
      "Drop zone",
    );
    await text(
      ax + margin + 48,
      ay + margin + 48,
      "Import a photo",
      48,
      solidFill(BRAND_PALETTE.espresso),
      "sans-serif",
      "Drop zone heading",
    );
    await text(
      ax + margin + 48,
      ay + margin + 120,
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
    // Two-column body for talking points.
    const colTop = ay + margin + 220;
    const colHeight = ah - colTop - margin;
    const colWidth = Math.round((aw - margin * 3) / 2);
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
    // 64-grid alignment hint behind a centred icon glyph
    // placeholder. A 512px artboard reads as a single "tile" so
    // we frame the canvas like an icon preview surface.
    const grid = 64;
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
      `${aw}\u00d7${ah}\u00a0\u00b7\u00a0${grid}px grid`,
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
