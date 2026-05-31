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

import type {
  CanvasBatchItem,
  FillStyle,
  RgbaColor,
} from "../../../shared/scene";

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

/// Named-argument shape for queuing a text node into a
/// [`BatchBuilder`]. The required fields (`x`, `y`, `body`, `size`)
/// stay distinct from the optional cosmetic fields (`fill`, `family`,
/// `name`) so a call site cannot silently reorder the two
/// height-vs-size or family-vs-name params — the most common
/// footgun in the previous positional API (Devin Review PR #31
/// flagged this on `templates.ts:113`).
///
/// `fill = null` is rejected at the type level (use `undefined`
/// instead) so the bridge wire shape never has to carry a nullable
/// fill; the helper only inserts the field into the batch item
/// when it's actually set.
export interface TextSeed {
  x: number;
  y: number;
  body: string;
  size: number;
  fill?: FillStyle;
  family?: string;
  name?: string;
}

/// Named-argument shape for queuing a rectangle into a
/// [`BatchBuilder`]. Mirrors [`TextSeed`]'s contract — required
/// geometry first, optional cosmetic fields second.
export interface RectSeed {
  x: number;
  y: number;
  w: number;
  h: number;
  fill?: FillStyle;
  name?: string;
}

/// Builds up a list of canvas primitives, then flushes them through
/// the batch bridge API in a single round-trip. The bridge takes
/// the workspace write lock once, inserts every item in submission
/// order, records one operation per item, and runs a single
/// `sync_scene` — collapsing a 12-node template from ~24 IPC
/// round-trips (12 × create + 12 × updateNode for fill/name) down
/// to a single `createNodes` call.
///
/// Helpers are synchronous (no IPC during build) so the resolver
/// body reads as a flat list of geometry decisions. Only `flush()`
/// touches the bridge.
export interface BatchBuilder {
  rect(seed: RectSeed): void;
  text(seed: TextSeed): void;
  flush(): Promise<string[]>;
}

/// Construct a fresh [`BatchBuilder`]. Each resolver builds its own
/// — they don't share state so the order of `apply()` calls is
/// deterministic from the caller's perspective.
///
/// Exported so the templates test can pin the reuse-safety contract
/// (post-flush, the internal queue is empty so a second `flush()` is
/// a no-op rather than a re-submission).
export function makeBatch(): BatchBuilder {
  const items: CanvasBatchItem[] = [];
  return {
    rect({ x, y, w, h, fill, name }) {
      const item: Extract<CanvasBatchItem, { kind: "rect" }> = {
        kind: "rect",
        parent: null,
        x,
        y,
        w,
        h,
      };
      if (fill !== undefined) item.fill = fill;
      if (name !== undefined) item.name = name;
      items.push(item);
    },
    text({ x, y, body, size, fill, family = "sans-serif", name }) {
      const item: Extract<CanvasBatchItem, { kind: "text" }> = {
        kind: "text",
        parent: null,
        x,
        y,
        body,
        family,
        size,
      };
      if (fill !== undefined) item.fill = fill;
      if (name !== undefined) item.name = name;
      items.push(item);
    },
    async flush() {
      if (items.length === 0) return [];
      // Snapshot the queued items and clear the buffer *before* we
      // hit the bridge. This makes the builder safely reusable: a
      // future caller that does multi-phase seeding
      // (`b.rect(...); await b.flush(); b.text(...); await b.flush()`)
      // cannot accidentally double-submit the first phase. We snapshot
      // because we must not feed the bridge an array we're about to
      // mutate. Devin Review PR #32 INFO_0001.
      const batch = items.slice();
      items.length = 0;
      return await window.kcreate.canvas.createNodes(batch);
    },
  };
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
    const b = makeBatch();
    // Heading + tagline anchored to the top of the artboard.
    const { x: ax, y: ay, width: aw, height: ah } = ctx;
    const margin = Math.round(aw * 0.06);
    b.text({
      x: ax + margin,
      y: ay + margin,
      body: "Brand Name",
      size: 64,
      fill: solidFill(BRAND_PALETTE.ink),
      name: "Brand title",
    });
    b.text({
      x: ax + margin,
      y: ay + margin + 96,
      body: "Tagline goes here",
      size: 24,
      fill: solidFill(BRAND_PALETTE.espresso),
      name: "Tagline",
    });

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
      b.rect({
        x: sx,
        y: swatchY,
        w: swatchWidth,
        h: swatchHeight,
        fill: solidFill(s.hex),
        name: `Palette / ${s.label}`,
      });
      b.text({
        x: sx,
        y: swatchY + swatchHeight + 16,
        body: s.label,
        size: 20,
        fill: solidFill(BRAND_PALETTE.ink),
        name: `Swatch label / ${s.label}`,
      });
    }

    // Logo placeholder mark anchored to the bottom-left. Rendered
    // as a filled circle (espresso) with a light ring so it reads
    // as a logomark target rather than a content rectangle. `logoSize`
    // was hoisted above the swatch row to participate in the
    // `availableSwatchHeight` clamp; reuse it here so the geometry
    // stays in lockstep.
    const logoX = ax + margin;
    const logoY = ay + logoTopRelative;
    b.rect({
      x: logoX,
      y: logoY,
      w: logoSize,
      h: logoSize,
      fill: solidFill(BRAND_PALETTE.espresso),
      name: "Logo placeholder",
    });

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
    b.text({
      x: captionX,
      y: captionY,
      body: captionText,
      size: captionFontSize,
      fill: solidFill(BRAND_PALETTE.espresso),
      name: "Logo caption",
    });
    await b.flush();
  },
};

const SOCIAL_RESOLVER: TemplateResolver = {
  async apply(ctx) {
    const b = makeBatch();
    const { x: ax, y: ay, width: aw, height: ah } = ctx;
    // Solid cream background covering the full artboard so the
    // template reads as "designed" instead of "blank canvas". The
    // background's width/height already come from the bridge as
    // positive integers (artboard preset), but we run them through
    // `Math.floor` / `Math.max(0, …)` defensively so a future
    // resolver caller that derives `aw`/`ah` from a user-resized
    // artboard can't accidentally hand the bridge a negative or
    // fractional dimension. Mirrors the BRAND/APP_UI clamp pattern.
    b.rect({
      x: ax,
      y: ay,
      w: Math.max(0, Math.floor(aw)),
      h: Math.max(0, Math.floor(ah)),
      fill: solidFill(BRAND_PALETTE.cream),
      name: "Background",
    });
    // Burnt-orange band behind the headline to anchor the type.
    // `Math.floor` on the band height + width so the band never
    // over-allocates against the artboard's right/bottom edges; the
    // band Y is `Math.round`ed because its anchor is the top of
    // the headline, which the eye naturally aligns rather than the
    // exact pixel boundary the rect occupies. `Math.max(0, …)` so a
    // very short artboard can't yield a negative band height.
    const bandHeight = Math.max(0, Math.floor(ah * 0.28));
    const bandY = ay + Math.round(ah * 0.18);
    b.rect({
      x: ax,
      y: bandY,
      w: Math.max(0, Math.floor(aw)),
      h: bandHeight,
      fill: solidFill(BRAND_PALETTE.burntOrange),
      name: "Headline band",
    });
    b.text({
      x: ax + Math.round(aw * 0.08),
      y: bandY + Math.round(bandHeight * 0.25),
      body: "Your Headline",
      size: 48,
      fill: solidFill(BRAND_PALETTE.cream),
      name: "Headline",
    });
    b.text({
      x: ax + Math.round(aw * 0.08),
      y: bandY + bandHeight + 48,
      body: "Add your message here",
      size: 20,
      fill: solidFill(BRAND_PALETTE.espresso),
      name: "Body copy",
    });
    // Sage accent dot bottom-right (placeholder for a brand mark).
    // `Math.floor` on the dot size + clamp so a very narrow
    // artboard yields a non-negative, non-overflowing dot.
    const dotSize = Math.max(0, Math.floor(aw * 0.08));
    b.rect({
      x: ax + Math.max(0, aw - dotSize - Math.round(aw * 0.06)),
      y: ay + Math.max(0, ah - dotSize - Math.round(ah * 0.06)),
      w: dotSize,
      h: dotSize,
      fill: solidFill(BRAND_PALETTE.sage),
      name: "Accent",
    });
    await b.flush();
  },
};

const PRINT_RESOLVER: TemplateResolver = {
  async apply(ctx) {
    const b = makeBatch();
    const { x: ax, y: ay, width: aw, height: ah } = ctx;
    // Header bar across the top in espresso brown.
    // `Math.floor` (not `Math.round`) on heights so the header +
    // body block + footer never collectively exceed `ah`; the
    // BRAND/DECK/APP_UI resolvers already pin this invariant on
    // their respective height budgets. `Math.max(0, …)` mirrors
    // the BRAND `swatchHeight` clamp so a degenerate short
    // artboard can't hand `createRect` a negative dimension.
    const headerHeight = Math.max(0, Math.floor(ah * 0.16));
    b.rect({
      x: ax,
      y: ay,
      w: Math.max(0, Math.floor(aw)),
      h: headerHeight,
      fill: solidFill(BRAND_PALETTE.espresso),
      name: "Header bar",
    });
    const margin = Math.round(aw * 0.06);
    b.text({
      x: ax + margin,
      y: ay + Math.round(headerHeight * 0.3),
      body: "Document Title",
      size: 72,
      fill: solidFill(BRAND_PALETTE.cream),
      name: "Document title",
    });
    // Body placeholder block in cream. The body width subtracts
    // both side margins from the artboard; `Math.max(0, …)` so a
    // very narrow artboard (where `margin*2 > aw`) can't yield a
    // negative width. Same protection on the body height.
    const bodyBlockWidth = Math.max(0, Math.floor(aw - margin * 2));
    const bodyBlockHeight = Math.max(0, Math.floor(ah * 0.5));
    b.rect({
      x: ax + margin,
      y: ay + headerHeight + margin,
      w: bodyBlockWidth,
      h: bodyBlockHeight,
      fill: solidFill(BRAND_PALETTE.cream),
      name: "Body block",
    });
    b.text({
      x: ax + margin * 2,
      y: ay + headerHeight + margin * 2,
      body: "Body copy goes here. Replace this placeholder with",
      size: 28,
      fill: solidFill(BRAND_PALETTE.ink),
      name: "Body paragraph",
    });
    b.text({
      x: ax + margin * 2,
      y: ay + headerHeight + margin * 2 + 40,
      body: "your real content. The header bar above is editable too.",
      size: 28,
      fill: solidFill(BRAND_PALETTE.ink),
      name: "Body paragraph 2",
    });
    // Footer accent strip in burnt orange. `Math.floor` so the
    // footer height never over-allocates against the artboard
    // bottom edge. `Math.max(0, ay + ah - footerHeight)` so a
    // negative-height artboard (impossible today, defensive for the
    // same reason as BRAND `swatchHeight`) can't pin the footer
    // above the artboard top.
    const footerHeight = Math.max(0, Math.floor(ah * 0.04));
    b.rect({
      x: ax,
      y: ay + Math.max(0, ah - footerHeight),
      w: Math.max(0, Math.floor(aw)),
      h: footerHeight,
      fill: solidFill(BRAND_PALETTE.burntOrange),
      name: "Footer accent",
    });
    await b.flush();
  },
};

const APP_UI_RESOLVER: TemplateResolver = {
  async apply(ctx) {
    const b = makeBatch();
    const { x: ax, y: ay, width: aw, height: ah } = ctx;
    // App-shell with a left rail, header, and content area.
    // `Math.floor` so the rail + content split fits the artboard
    // exactly: `rail + (aw - rail) = aw` no matter the rounding
    // mode for `rail`. Mirrors the DECK `colWidth` floor invariant.
    // `Math.max(0, …)` defends against degenerate artboards a
    // future surface might pass in.
    const rail = Math.max(0, Math.floor(aw * 0.12));
    const headerH = Math.max(0, Math.floor(ah * 0.08));
    b.rect({
      x: ax,
      y: ay,
      w: Math.max(0, Math.floor(aw)),
      h: Math.max(0, Math.floor(ah)),
      fill: solidFill(BRAND_PALETTE.paper),
      name: "App background",
    });
    b.rect({
      x: ax,
      y: ay,
      w: rail,
      h: Math.max(0, Math.floor(ah)),
      fill: solidFill(BRAND_PALETTE.espresso),
      name: "Left rail",
    });
    b.rect({
      x: ax + rail,
      y: ay,
      w: Math.max(0, aw - rail),
      h: headerH,
      fill: solidFill(BRAND_PALETTE.cream),
      name: "Header",
    });
    b.text({
      x: ax + rail + 32,
      y: ay + Math.round(headerH * 0.3),
      body: "App / Website UI",
      size: 28,
      fill: solidFill(BRAND_PALETTE.ink),
      name: "Header title",
    });
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
      b.rect({
        x: tx,
        y: tileTop,
        w: tileWidth,
        h: tileHeight,
        fill: solidFill(BRAND_PALETTE.cream),
        name: `Content tile ${i + 1}`,
      });
      b.text({
        x: tx + 24,
        y: tileTop + 24,
        body: `Section ${i + 1}`,
        size: 22,
        fill: solidFill(BRAND_PALETTE.ink),
        name: `Tile heading ${i + 1}`,
      });
    }
    await b.flush();
  },
};

const PHOTO_RESOLVER: TemplateResolver = {
  async apply(ctx) {
    const b = makeBatch();
    const { x: ax, y: ay, width: aw, height: ah } = ctx;
    // Checkerboard-style background hint so the user can tell
    // we're inside the artboard before they drop a photo in.
    b.rect({
      x: ax,
      y: ay,
      w: Math.max(0, Math.floor(aw)),
      h: Math.max(0, Math.floor(ah)),
      fill: solidFill(BRAND_PALETTE.cream),
      name: "Photo backdrop",
    });
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
    b.rect({
      x: innerX,
      y: innerY,
      w: innerSize,
      h: innerSize,
      fill: solidFill(BRAND_PALETTE.paper),
      name: "Drop zone",
    });
    b.text({
      x: innerX + 48,
      y: innerY + 48,
      body: "Import a photo",
      size: 48,
      fill: solidFill(BRAND_PALETTE.espresso),
      name: "Drop zone heading",
    });
    b.text({
      x: innerX + 48,
      y: innerY + 120,
      body: "Use AI Assist \u2192 Background removal once imported.",
      size: 22,
      fill: solidFill(BRAND_PALETTE.ink),
      name: "Drop zone hint",
    });
    await b.flush();
  },
};

const DECK_RESOLVER: TemplateResolver = {
  async apply(ctx) {
    const b = makeBatch();
    const { x: ax, y: ay, width: aw, height: ah } = ctx;
    b.rect({
      x: ax,
      y: ay,
      w: Math.max(0, Math.floor(aw)),
      h: Math.max(0, Math.floor(ah)),
      fill: solidFill(BRAND_PALETTE.paper),
      name: "Slide background",
    });
    const margin = Math.round(aw * 0.06);
    // Title block.
    b.text({
      x: ax + margin,
      y: ay + margin,
      body: "Pitch Deck Title",
      size: 80,
      fill: solidFill(BRAND_PALETTE.ink),
      name: "Slide title",
    });
    b.text({
      x: ax + margin,
      y: ay + margin + 110,
      body: "Subtitle or short positioning line",
      size: 32,
      fill: solidFill(BRAND_PALETTE.espresso),
      name: "Subtitle",
    });
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
    b.rect({
      x: ax + margin,
      y: colTop,
      w: colWidth,
      h: colHeight,
      fill: solidFill(BRAND_PALETTE.cream),
      name: "Column A",
    });
    b.rect({
      x: ax + margin * 2 + colWidth,
      y: colTop,
      w: colWidth,
      h: colHeight,
      fill: solidFill(BRAND_PALETTE.cream),
      name: "Column B",
    });
    await b.flush();
  },
};

const DEV_EXPORT_RESOLVER: TemplateResolver = {
  async apply(ctx) {
    const b = makeBatch();
    const { x: ax, y: ay, width: aw, height: ah } = ctx;
    // The dev-export preset is the icon-pack starting point, so we
    // frame the canvas like an icon preview surface (filled body
    // with a centred notch) and label the artboard with the export
    // grid size. `iconGridSizeHint` is purely a label — nothing
    // below snaps to it. Naming it `gridHint` (rather than `grid`)
    // keeps that boundary explicit so a future contributor doesn't
    // assume the rectangles below are grid-aligned.
    const iconGridSizeHint = 64;
    b.rect({
      x: ax,
      y: ay,
      w: Math.max(0, Math.floor(aw)),
      h: Math.max(0, Math.floor(ah)),
      fill: solidFill(BRAND_PALETTE.paper),
      name: "Icon backdrop",
    });
    // `Math.floor` on the inset so the icon body's right/bottom
    // edges never bleed past the artboard. `Math.max(0, …)` on the
    // resulting body dimensions defends against a degenerate
    // artboard (where `inset * 2 > aw` would yield a negative
    // body), mirroring the BRAND/APP_UI clamp pattern.
    const inset = Math.max(0, Math.floor(aw * 0.12));
    const bodyW = Math.max(0, aw - inset * 2);
    const bodyH = Math.max(0, ah - inset * 2);
    b.rect({
      x: ax + inset,
      y: ay + inset,
      w: bodyW,
      h: bodyH,
      fill: solidFill(BRAND_PALETTE.burntOrange),
      name: "Icon body",
    });
    // Inner notch so the icon body has visual content out of the box.
    // `Math.floor` so the notch never over-allocates inside the body;
    // `Math.max(0, …)` defends against the same degenerate-artboard
    // case as the body above.
    const notch = Math.max(0, Math.floor(bodyW * 0.35));
    b.rect({
      x: ax + Math.round(aw / 2) - Math.round(notch / 2),
      y: ay + Math.round(ah / 2) - Math.round(notch / 2),
      w: notch,
      h: notch,
      fill: solidFill(BRAND_PALETTE.cream),
      name: "Icon notch",
    });
    b.text({
      x: ax + 16,
      y: ay + 16,
      body: `${aw}×${ah}\u00a0\u00b7\u00a0${iconGridSizeHint}px grid`,
      size: 18,
      fill: solidFill(BRAND_PALETTE.espresso),
      name: "Spec caption",
    });
    await b.flush();
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
