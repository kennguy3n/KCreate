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
  GradientStop,
  Point2D,
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

/// A single gradient colour stop. `offset` is the position along the
/// gradient axis in `[0, 1]`. Mirrors `kcreate_core::node::GradientStop`
/// — the same wire shape the FillEditor builds for gradient swatches.
function stop(offset: number, rgb: string, alpha = 1.0): GradientStop {
  return { offset, color: hex(rgb, alpha) };
}

/// Linear gradient fill. `from`/`to` are *normalised, node-local*
/// coordinates in `[0, 1]` — the renderer maps them onto the node's
/// bounds, so the helper yields a clean ramp regardless of the rect's
/// pixel size. Defaults to a top→bottom sweep.
function linearFill(
  stops: GradientStop[],
  from: Point2D = { x: 0, y: 0 },
  to: Point2D = { x: 0, y: 1 },
): FillStyle {
  return { kind: "gradient", shape: "linear", from, to, stops };
}

/// Radial gradient fill. `center` + `radius` are normalised node-local
/// values (centre defaults to the middle of the node, radius to half
/// its shorter extent). Used for soft glows / spotlight accents.
function radialFill(
  stops: GradientStop[],
  center: Point2D = { x: 0.5, y: 0.5 },
  radius = 0.5,
): FillStyle {
  return { kind: "gradient", shape: "radial", center, radius, stops };
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

/// Named-argument shape for queuing an ellipse into a
/// [`BatchBuilder`]. `cx`/`cy` are the centre, `rx`/`ry` the radii
/// (matching `CanvasBatchItem`'s `ellipse` variant on the wire). Used
/// for logomarks, avatars, icon glyphs, and chart points.
export interface EllipseSeed {
  cx: number;
  cy: number;
  rx: number;
  ry: number;
  fill?: FillStyle;
  name?: string;
}

/// Named-argument shape for queuing a line into a [`BatchBuilder`].
/// Endpoints are absolute world-space coordinates; the `fill` colours
/// the stroke (the renderer treats a line's fill as its stroke paint).
/// Used for axis rules, dividers, and connectors.
export interface LineSeed {
  x1: number;
  y1: number;
  x2: number;
  y2: number;
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
  ellipse(seed: EllipseSeed): void;
  line(seed: LineSeed): void;
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
    ellipse({ cx, cy, rx, ry, fill, name }) {
      const item: Extract<CanvasBatchItem, { kind: "ellipse" }> = {
        kind: "ellipse",
        parent: null,
        cx,
        cy,
        rx,
        ry,
      };
      if (fill !== undefined) item.fill = fill;
      if (name !== undefined) item.name = name;
      items.push(item);
    },
    line({ x1, y1, x2, y2, fill, name }) {
      const item: Extract<CanvasBatchItem, { kind: "line" }> = {
        kind: "line",
        parent: null,
        x1,
        y1,
        x2,
        y2,
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

/// Modern product-UI design system layered on top of the brand
/// neutrals for the app / deck / poster showcases. Kept separate from
/// `BRAND_PALETTE` (which keeps the Brewline coffee identity the brand
/// board + demo are pinned to) so the product surfaces read as a
/// contemporary SaaS system: an indigo brand ramp, slate neutral type
/// scale, hairline dividers, and a small set of status accents for
/// metric deltas / chart series.
export const UI_THEME = {
  /// Primary text — slate-900.
  ink: "#0F172A",
  /// Secondary text — slate-600.
  inkSoft: "#475569",
  /// Tertiary / caption text + inactive icons — slate-400.
  muted: "#94A3B8",
  /// 1px dividers + card outlines — slate-200.
  hairline: "#E2E8F0",
  /// Card / panel surface.
  surface: "#FFFFFF",
  /// App canvas behind cards — slate-50.
  canvas: "#F8FAFC",
  /// Indigo brand ramp (rail / hero gradients).
  brandDark: "#312E81",
  brand: "#4F46E5",
  brandBright: "#6366F1",
  /// Tint behind the brand colour — indigo-50.
  brandSoft: "#EEF2FF",
  /// Status accents for deltas + chart series.
  emerald: "#10B981",
  amber: "#F59E0B",
  rose: "#F43F5E",
  sky: "#0EA5E9",
  violet: "#8B5CF6",
} as const;

/// Clamp a numeric value into `[lo, hi]`. Centralises the
/// `Math.max(lo, Math.min(hi, v))` pattern the resolvers lean on for
/// defensive geometry so a degenerate artboard never yields a
/// negative or overflowing dimension.
function clamp(value: number, lo: number, hi: number): number {
  return Math.max(lo, Math.min(hi, value));
}

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
    // Eyebrow kicker above the headline band — a small sage label
    // that gives the post a magazine-style lead-in instead of a bare
    // headline.
    b.text({
      x: ax + Math.round(aw * 0.08),
      y: ay + Math.round(ah * 0.1),
      body: "NEW THIS WEEK",
      size: 22,
      fill: solidFill(BRAND_PALETTE.sage),
      name: "Eyebrow",
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
    // Sage rule under the headline band to separate headline from body.
    b.line({
      x1: ax + Math.round(aw * 0.08),
      y1: bandY + bandHeight + 28,
      x2: ax + Math.round(aw * 0.08) + Math.round(aw * 0.16),
      y2: bandY + bandHeight + 28,
      fill: solidFill(BRAND_PALETTE.sage),
      name: "Body rule",
    });
    b.text({
      x: ax + Math.round(aw * 0.08),
      y: bandY + bandHeight + 48,
      body: "Add your message here",
      size: 20,
      fill: solidFill(BRAND_PALETTE.espresso),
      name: "Body copy",
    });
    b.text({
      x: ax + Math.round(aw * 0.08),
      y: bandY + bandHeight + 84,
      body: "Tap the link in bio to read the full story.",
      size: 16,
      fill: solidFill(BRAND_PALETTE.espresso, 0.7),
      name: "Body subcopy",
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
    // A concentric ring inside the accent dot so the brand mark reads
    // as an intentional badge rather than a flat square.
    const dotCx = ax + Math.max(0, aw - dotSize - Math.round(aw * 0.06)) +
      Math.round(dotSize / 2);
    const dotCy = ay + Math.max(0, ah - dotSize - Math.round(ah * 0.06)) +
      Math.round(dotSize / 2);
    b.ellipse({
      cx: dotCx,
      cy: dotCy,
      rx: Math.max(2, Math.round(dotSize * 0.28)),
      ry: Math.max(2, Math.round(dotSize * 0.28)),
      fill: solidFill(BRAND_PALETTE.cream),
      name: "Accent ring",
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
    // Burnt-orange rule + subhead under the masthead title so the
    // header reads as a designed masthead rather than a flat bar.
    b.line({
      x1: ax + margin,
      y1: ay + Math.round(headerHeight * 0.3) + 92,
      x2: ax + margin + Math.round(aw * 0.18),
      y2: ay + Math.round(headerHeight * 0.3) + 92,
      fill: solidFill(BRAND_PALETTE.burntOrange),
      name: "Masthead rule",
    });
    b.text({
      x: ax + margin,
      y: ay + Math.round(headerHeight * 0.3) + 108,
      body: "Prepared by Northwind · 2026",
      size: 30,
      fill: solidFill(BRAND_PALETTE.cream, 0.85),
      name: "Masthead subtitle",
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
    b.text({
      x: ax + margin,
      y: ay + Math.max(0, ah - footerHeight) + Math.round(footerHeight * 0.3),
      body: "Page 1 · Confidential",
      size: Math.max(10, Math.round(footerHeight * 0.32)),
      fill: solidFill(BRAND_PALETTE.cream),
      name: "Footer caption",
    });
    await b.flush();
  },
};

const APP_UI_RESOLVER: TemplateResolver = {
  async apply(ctx) {
    const b = makeBatch();
    const { x: ax, y: ay, width: aw, height: ah } = ctx;
    // A real product dashboard: gradient nav rail with brand + nav
    // items + user chip, a top bar with title / search / primary
    // action, a row of KPI metric cards (with deltas + sparkline), and
    // a revenue chart card beside a recent-activity list. Composed
    // entirely from the batch primitives (rect / ellipse / line / text
    // + gradients) — no new vector math, every node editable.
    //
    // `Math.floor` so the rail + content split fits the artboard
    // exactly: `rail + (aw - rail) = aw` regardless of the rounding
    // mode. `Math.max(0, …)` defends against degenerate artboards a
    // future surface might pass in. Mirrors the DECK `colWidth` floor
    // invariant.
    const rail = Math.max(0, Math.floor(aw * 0.165));
    const headerH = Math.max(0, Math.floor(ah * 0.085));
    const pad = 28;
    const contentX = ax + rail;
    const contentW = Math.max(0, aw - rail);
    const fullW = Math.max(0, Math.floor(aw));
    const fullH = Math.max(0, Math.floor(ah));

    // Canvas + gradient rail.
    b.rect({
      x: ax,
      y: ay,
      w: fullW,
      h: fullH,
      fill: solidFill(UI_THEME.canvas),
      name: "App background",
    });
    b.rect({
      x: ax,
      y: ay,
      w: rail,
      h: fullH,
      fill: linearFill(
        [stop(0, UI_THEME.brandDark), stop(1, UI_THEME.brand)],
        { x: 0, y: 0 },
        { x: 0.3, y: 1 },
      ),
      name: "Left rail",
    });

    // Rail brand lockup.
    b.ellipse({
      cx: ax + 30,
      cy: ay + 40,
      rx: 13,
      ry: 13,
      fill: solidFill(UI_THEME.surface),
      name: "Brand mark",
    });
    b.text({
      x: ax + 56,
      y: ay + 30,
      body: "Northwind",
      size: 21,
      fill: solidFill(UI_THEME.surface),
      name: "Brand wordmark",
    });

    // Rail navigation. The first row is the active route (subtle
    // white wash behind it); the rest are muted.
    const navItems: ReadonlyArray<string> = [
      "Dashboard",
      "Projects",
      "Templates",
      "Assets",
      "Reports",
    ];
    const navTop = ay + 96;
    const navStride = 50;
    for (let i = 0; i < navItems.length; i += 1) {
      const label = navItems[i] ?? "";
      const rowY = navTop + i * navStride;
      const active = i === 0;
      if (active) {
        b.rect({
          x: ax + 14,
          y: rowY - 4,
          w: Math.max(0, rail - 28),
          h: 40,
          fill: solidFill(UI_THEME.surface, 0.16),
          name: "Nav highlight",
        });
      }
      b.ellipse({
        cx: ax + 34,
        cy: rowY + 16,
        rx: 8,
        ry: 8,
        fill: solidFill(UI_THEME.surface, active ? 0.95 : 0.5),
        name: `Nav icon ${i + 1}`,
      });
      b.text({
        x: ax + 58,
        y: rowY + 6,
        body: label,
        size: 17,
        fill: solidFill(UI_THEME.surface, active ? 1 : 0.66),
        name: `Nav label ${i + 1}`,
      });
    }

    // Rail user chip pinned to the bottom.
    const chipY = ay + fullH - 70;
    b.ellipse({
      cx: ax + 34,
      cy: chipY + 16,
      rx: 15,
      ry: 15,
      fill: solidFill(UI_THEME.emerald),
      name: "Rail avatar",
    });
    b.text({
      x: ax + 58,
      y: chipY + 4,
      body: "Ada Lovelace",
      size: 15,
      fill: solidFill(UI_THEME.surface),
      name: "Rail user",
    });
    b.text({
      x: ax + 58,
      y: chipY + 24,
      body: "Product designer",
      size: 12,
      fill: solidFill(UI_THEME.surface, 0.6),
      name: "Rail user role",
    });

    // Top bar across the content area.
    b.rect({
      x: contentX,
      y: ay,
      w: contentW,
      h: headerH,
      fill: solidFill(UI_THEME.surface),
      name: "Header",
    });
    b.line({
      x1: contentX,
      y1: ay + headerH,
      x2: ax + aw,
      y2: ay + headerH,
      fill: solidFill(UI_THEME.hairline),
      name: "Header divider",
    });
    b.text({
      x: contentX + pad,
      y: ay + Math.round(headerH * 0.26),
      body: "Dashboard",
      size: 28,
      fill: solidFill(UI_THEME.ink),
      name: "Header title",
    });
    b.text({
      x: contentX + pad,
      y: ay + Math.round(headerH * 0.26) + 34,
      body: "Welcome back, Ada — here's this week at a glance",
      size: 14,
      fill: solidFill(UI_THEME.muted),
      name: "Header subtitle",
    });

    // Primary action + search field on the right of the top bar.
    const btnW = 150;
    const btnH = 42;
    const ctrlY = ay + Math.max(0, Math.round((headerH - btnH) / 2));
    const btnX = ax + aw - pad - btnW;
    b.rect({
      x: btnX,
      y: ctrlY,
      w: btnW,
      h: btnH,
      fill: linearFill(
        [stop(0, UI_THEME.brand), stop(1, UI_THEME.brandBright)],
        { x: 0, y: 0 },
        { x: 1, y: 1 },
      ),
      name: "Primary button",
    });
    b.text({
      x: btnX + 22,
      y: ctrlY + 12,
      body: "+ New project",
      size: 15,
      fill: solidFill(UI_THEME.surface),
      name: "Primary button label",
    });
    const searchW = clamp(contentW - pad * 2 - btnW - 360, 160, 320);
    const searchX = btnX - 20 - searchW;
    b.rect({
      x: searchX,
      y: ctrlY,
      w: searchW,
      h: btnH,
      fill: solidFill(UI_THEME.canvas),
      name: "Search field",
    });
    b.ellipse({
      cx: searchX + 22,
      cy: ctrlY + 18,
      rx: 6,
      ry: 6,
      fill: solidFill(UI_THEME.muted),
      name: "Search glyph",
    });
    b.line({
      x1: searchX + 26,
      y1: ctrlY + 22,
      x2: searchX + 32,
      y2: ctrlY + 28,
      fill: solidFill(UI_THEME.muted),
      name: "Search glyph handle",
    });
    b.text({
      x: searchX + 40,
      y: ctrlY + 13,
      body: "Search projects…",
      size: 14,
      fill: solidFill(UI_THEME.muted),
      name: "Search placeholder",
    });

    // KPI metric cards. The three "Content tile" rects are the
    // pinned named nodes; each gets a top accent strip, a label
    // ("Tile heading"), a big value, a delta chip, and a sparkline.
    // `Math.max(0, …)` on width/height so a degenerate artboard
    // never yields a negative rect (the clamp the APP_UI tile test
    // pins). Mirrors the DECK `colHeight` clamp.
    const metrics: ReadonlyArray<{
      label: string;
      value: string;
      delta: string;
      positive: boolean;
      accent: string;
      spark: ReadonlyArray<number>;
    }> = [
      {
        label: "MONTHLY REVENUE",
        value: "$48,250",
        delta: "+12.4%",
        positive: true,
        accent: UI_THEME.brand,
        spark: [0.35, 0.5, 0.42, 0.62, 0.55, 0.78, 0.9],
      },
      {
        label: "ACTIVE USERS",
        value: "12,840",
        delta: "+4.1%",
        positive: true,
        accent: UI_THEME.sky,
        spark: [0.5, 0.45, 0.6, 0.58, 0.72, 0.68, 0.82],
      },
      {
        label: "CONVERSION",
        value: "3.8%",
        delta: "-0.6%",
        positive: false,
        accent: UI_THEME.violet,
        spark: [0.7, 0.66, 0.72, 0.6, 0.64, 0.55, 0.5],
      },
    ];
    const cardsTop = ay + headerH + 28;
    const cardGap = 24;
    const cardH = clamp(Math.floor(ah * 0.155), 0, 150);
    const cardW = Math.max(
      0,
      Math.floor((contentW - pad * 2 - cardGap * 2) / 3),
    );
    for (let i = 0; i < metrics.length; i += 1) {
      const m = metrics[i];
      if (!m) continue;
      const cardX = contentX + pad + i * (cardW + cardGap);
      b.rect({
        x: cardX,
        y: cardsTop,
        w: cardW,
        h: cardH,
        fill: solidFill(UI_THEME.surface),
        name: `Content tile ${i + 1}`,
      });
      b.rect({
        x: cardX,
        y: cardsTop,
        w: cardW,
        h: 4,
        fill: solidFill(m.accent),
        name: `Card accent ${i + 1}`,
      });
      b.text({
        x: cardX + 22,
        y: cardsTop + 22,
        body: m.label,
        size: 13,
        fill: solidFill(UI_THEME.muted),
        name: `Tile heading ${i + 1}`,
      });
      b.text({
        x: cardX + 22,
        y: cardsTop + 48,
        body: m.value,
        size: 36,
        fill: solidFill(UI_THEME.ink),
        name: `Metric value ${i + 1}`,
      });
      const deltaColor = m.positive ? UI_THEME.emerald : UI_THEME.rose;
      b.rect({
        x: cardX + 22,
        y: cardsTop + 102,
        w: 78,
        h: 26,
        fill: solidFill(deltaColor, 0.14),
        name: `Delta chip ${i + 1}`,
      });
      b.text({
        x: cardX + 32,
        y: cardsTop + 107,
        body: m.delta,
        size: 13,
        fill: solidFill(deltaColor),
        name: `Delta ${i + 1}`,
      });
      // Sparkline: connect the normalised series into an upward
      // trend with line segments coloured by the card accent.
      const sparkW = Math.max(0, Math.min(120, cardW - 150));
      const sparkX0 = cardX + cardW - 22 - sparkW;
      const sparkBottom = cardsTop + cardH - 22;
      const sparkH = Math.max(0, Math.min(48, cardH - 60));
      for (let s = 0; s < m.spark.length - 1; s += 1) {
        const d0 = m.spark[s] ?? 0;
        const d1 = m.spark[s + 1] ?? 0;
        const step = m.spark.length > 1 ? sparkW / (m.spark.length - 1) : 0;
        b.line({
          x1: sparkX0 + step * s,
          y1: sparkBottom - sparkH * d0,
          x2: sparkX0 + step * (s + 1),
          y2: sparkBottom - sparkH * d1,
          fill: solidFill(m.accent),
          name: `Sparkline ${i + 1}-${s + 1}`,
        });
      }
    }

    // Lower row: revenue chart card + recent-activity card.
    const lowerTop = cardsTop + cardH + 28;
    const lowerBottom = ay + fullH - pad;
    const lowerH = Math.max(0, lowerBottom - lowerTop);
    const chartW = Math.max(
      0,
      Math.floor((contentW - pad * 2 - cardGap) * 0.62),
    );
    const listW = Math.max(0, contentW - pad * 2 - cardGap - chartW);
    const chartX = contentX + pad;
    const listX = chartX + chartW + cardGap;

    b.rect({
      x: chartX,
      y: lowerTop,
      w: chartW,
      h: lowerH,
      fill: solidFill(UI_THEME.surface),
      name: "Chart card",
    });
    b.text({
      x: chartX + 28,
      y: lowerTop + 24,
      body: "Revenue over time",
      size: 18,
      fill: solidFill(UI_THEME.ink),
      name: "Chart title",
    });
    b.text({
      x: chartX + 28,
      y: lowerTop + 50,
      body: "Last 12 weeks · USD",
      size: 13,
      fill: solidFill(UI_THEME.muted),
      name: "Chart subtitle",
    });
    const chartSeries: ReadonlyArray<number> = [
      0.32, 0.45, 0.38, 0.52, 0.6, 0.48, 0.66, 0.72, 0.64, 0.8, 0.74, 0.92,
    ];
    const plotX0 = chartX + 28;
    const plotRight = chartX + chartW - 28;
    const plotW = Math.max(0, plotRight - plotX0);
    const plotBottom = lowerTop + lowerH - 44;
    const plotTop = lowerTop + 92;
    const plotH = Math.max(0, plotBottom - plotTop);
    const barGap = 14;
    const barW = Math.max(
      0,
      Math.floor((plotW - barGap * (chartSeries.length - 1)) / chartSeries.length),
    );
    b.line({
      x1: plotX0,
      y1: plotBottom,
      x2: plotRight,
      y2: plotBottom,
      fill: solidFill(UI_THEME.hairline),
      name: "Chart axis",
    });
    for (let i = 0; i < chartSeries.length; i += 1) {
      const d = chartSeries[i] ?? 0;
      const barH = Math.max(2, Math.round(plotH * d));
      b.rect({
        x: plotX0 + i * (barW + barGap),
        y: plotBottom - barH,
        w: barW,
        h: barH,
        fill: linearFill(
          [stop(0, UI_THEME.brandBright), stop(1, UI_THEME.brand)],
          { x: 0, y: 0 },
          { x: 0, y: 1 },
        ),
        name: `Chart bar ${i + 1}`,
      });
    }

    b.rect({
      x: listX,
      y: lowerTop,
      w: listW,
      h: lowerH,
      fill: solidFill(UI_THEME.surface),
      name: "Activity card",
    });
    b.text({
      x: listX + 24,
      y: lowerTop + 24,
      body: "Recent activity",
      size: 18,
      fill: solidFill(UI_THEME.ink),
      name: "Activity title",
    });
    const activity: ReadonlyArray<{ who: string; what: string; tint: string }> =
      [
        { who: "Maya Chen", what: "shipped the onboarding flow", tint: UI_THEME.brand },
        { who: "Devin", what: "generated 6 deck variants", tint: UI_THEME.emerald },
        { who: "Liam Ortiz", what: "commented on Pricing v3", tint: UI_THEME.amber },
        { who: "Priya N.", what: "exported the brand kit", tint: UI_THEME.sky },
        { who: "Sam Park", what: "restyled the report theme", tint: UI_THEME.violet },
      ];
    const rowTop0 = lowerTop + 60;
    const rowStride = clamp(
      Math.floor((lowerH - 76) / activity.length),
      0,
      96,
    );
    for (let j = 0; j < activity.length; j += 1) {
      const row = activity[j];
      if (!row) continue;
      const rowY = rowTop0 + j * rowStride;
      b.ellipse({
        cx: listX + 40,
        cy: rowY + 16,
        rx: 16,
        ry: 16,
        fill: solidFill(row.tint),
        name: `Activity avatar ${j + 1}`,
      });
      b.text({
        x: listX + 68,
        y: rowY + 4,
        body: row.who,
        size: 15,
        fill: solidFill(UI_THEME.ink),
        name: `Activity who ${j + 1}`,
      });
      b.text({
        x: listX + 68,
        y: rowY + 26,
        body: row.what,
        size: 13,
        fill: solidFill(UI_THEME.muted),
        name: `Activity what ${j + 1}`,
      });
      if (j < activity.length - 1) {
        b.line({
          x1: listX + 24,
          y1: rowY + rowStride - 8,
          x2: listX + listW - 24,
          y2: rowY + rowStride - 8,
          fill: solidFill(UI_THEME.hairline),
          name: `Activity divider ${j + 1}`,
        });
      }
    }

    await b.flush();
  },
};

const PHOTO_RESOLVER: TemplateResolver = {
  async apply(ctx) {
    const b = makeBatch();
    const { x: ax, y: ay, width: aw, height: ah } = ctx;
    // Soft radial wash so the surround reads as an intentional studio
    // backdrop rather than a flat fill. The PHOTO test pins only the
    // backdrop's width/height (full-bleed), not its fill, so a radial
    // gradient is free to use here.
    b.rect({
      x: ax,
      y: ay,
      w: Math.max(0, Math.floor(aw)),
      h: Math.max(0, Math.floor(ah)),
      fill: radialFill(
        [stop(0, BRAND_PALETTE.cream), stop(1, "#EFE6CC")],
        { x: 0.5, y: 0.42 },
        0.75,
      ),
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
    // L-shaped crop marks at each corner of the drop zone so the
    // surface reads like a photographer's framing guide. Drawn as
    // burnt-orange line pairs; lines are decorative chrome and are
    // skipped by the inside-artboard assertion.
    const markLen = Math.max(8, Math.round(innerSize * 0.06));
    const corners: ReadonlyArray<{ cx: number; cy: number; sx: number; sy: number }> = [
      { cx: innerX, cy: innerY, sx: 1, sy: 1 },
      { cx: innerX + innerSize, cy: innerY, sx: -1, sy: 1 },
      { cx: innerX, cy: innerY + innerSize, sx: 1, sy: -1 },
      { cx: innerX + innerSize, cy: innerY + innerSize, sx: -1, sy: -1 },
    ];
    for (let i = 0; i < corners.length; i += 1) {
      const c = corners[i];
      if (!c) continue;
      b.line({
        x1: c.cx,
        y1: c.cy,
        x2: c.cx + c.sx * markLen,
        y2: c.cy,
        fill: solidFill(BRAND_PALETTE.burntOrange),
        name: `Crop mark ${i + 1}H`,
      });
      b.line({
        x1: c.cx,
        y1: c.cy,
        x2: c.cx,
        y2: c.cy + c.sy * markLen,
        fill: solidFill(BRAND_PALETTE.burntOrange),
        name: `Crop mark ${i + 1}V`,
      });
    }
    await b.flush();
  },
};

const DECK_RESOLVER: TemplateResolver = {
  async apply(ctx) {
    const b = makeBatch();
    const { x: ax, y: ay, width: aw, height: ah } = ctx;
    const fullW = Math.max(0, Math.floor(aw));
    const fullH = Math.max(0, Math.floor(ah));
    const margin = Math.round(aw * 0.06);

    // Title slide: a soft radial-lit canvas, a bold brand accent edge,
    // an eyebrow → headline → subtitle hierarchy, an accent rule, two
    // feature cards, and a footer with page number + wordmark.
    b.rect({
      x: ax,
      y: ay,
      w: fullW,
      h: fullH,
      fill: radialFill(
        [stop(0, UI_THEME.surface), stop(1, UI_THEME.canvas)],
        { x: 0.28, y: 0.22 },
        0.95,
      ),
      name: "Slide background",
    });
    // Bold brand accent edge down the left.
    b.rect({
      x: ax,
      y: ay,
      w: Math.max(0, Math.round(aw * 0.016)),
      h: fullH,
      fill: linearFill(
        [stop(0, UI_THEME.brandBright), stop(1, UI_THEME.brandDark)],
        { x: 0, y: 0 },
        { x: 0, y: 1 },
      ),
      name: "Accent edge",
    });

    // Title block hierarchy (eyebrow / headline / subtitle).
    const titleSize = clamp(Math.round(ah * 0.085), 24, 96);
    const subSize = clamp(Math.round(ah * 0.032), 12, 34);
    const eyebrowY = ay + Math.round(ah * 0.15);
    const titleY = ay + Math.round(ah * 0.21);
    const subtitleY = titleY + titleSize + Math.round(ah * 0.02);
    b.text({
      x: ax + margin,
      y: eyebrowY,
      body: "PRODUCT STRATEGY · 2026",
      size: clamp(Math.round(ah * 0.026), 10, 22),
      fill: solidFill(UI_THEME.brand),
      name: "Eyebrow",
    });
    b.text({
      x: ax + margin,
      y: titleY,
      body: "The next chapter of Northwind",
      size: titleSize,
      fill: solidFill(UI_THEME.ink),
      name: "Slide title",
    });
    b.text({
      x: ax + margin,
      y: subtitleY,
      body: "A product vision for ambient, on-device design",
      size: subSize,
      fill: solidFill(UI_THEME.inkSoft),
      name: "Subtitle",
    });
    b.line({
      x1: ax + margin,
      y1: subtitleY + subSize + Math.round(ah * 0.02),
      x2: ax + margin + Math.round(aw * 0.16),
      y2: subtitleY + subSize + Math.round(ah * 0.02),
      fill: solidFill(UI_THEME.brand),
      name: "Title rule",
    });

    // Two feature cards. `colTop`/`colHeight` already include the
    // artboard Y-offset (`ay`), so the closing edge also includes
    // `ay` — otherwise a non-zero artboard origin (the bridge offsets
    // every artboard after the first) yields a negative height.
    // `Math.max(0, …)` clamps `colHeight` so a short custom artboard
    // never collapses the cards to negative-size nodes (the DECK
    // 800×450 + Y-offset tests pin this).
    const footerH = Math.round(ah * 0.09);
    const colTop = ay + Math.round(ah * 0.46);
    const colBottom = ay + fullH - footerH;
    const colHeight = Math.max(0, colBottom - colTop);
    // `Math.floor` (not `Math.round`) so the two-column budget is
    // never over-allocated: `2 * colWidth + 3 * margin <= aw`.
    const colWidth = Math.floor((aw - margin * 3) / 2);
    const cards: ReadonlyArray<{
      name: "Column A" | "Column B";
      accent: string;
      heading: string;
      body: string;
    }> = [
      {
        name: "Column A",
        accent: UI_THEME.brand,
        heading: "On-device intelligence",
        body: "Generate, restyle and resize without a round-trip to the cloud.",
      },
      {
        name: "Column B",
        accent: UI_THEME.emerald,
        heading: "Print-ready precision",
        body: "Vector accuracy and CMYK preflight built into every export.",
      },
    ];
    const headingSize = clamp(Math.round(ah * 0.034), 12, 34);
    const bodySize = clamp(Math.round(ah * 0.024), 10, 24);
    const stripH = Math.max(0, Math.round(ah * 0.012));
    for (let i = 0; i < cards.length; i += 1) {
      const card = cards[i];
      if (!card) continue;
      const cardX = ax + margin + i * (colWidth + margin);
      b.rect({
        x: cardX,
        y: colTop,
        w: colWidth,
        h: colHeight,
        fill: solidFill(UI_THEME.surface),
        name: card.name,
      });
      b.rect({
        x: cardX,
        y: colTop,
        w: colWidth,
        h: stripH,
        fill: solidFill(card.accent),
        name: `${card.name} accent`,
      });
      b.ellipse({
        cx: cardX + Math.round(aw * 0.03),
        cy: colTop + Math.round(ah * 0.085),
        rx: Math.max(2, Math.round(ah * 0.022)),
        ry: Math.max(2, Math.round(ah * 0.022)),
        fill: solidFill(card.accent, 0.18),
        name: `${card.name} icon`,
      });
      b.text({
        x: cardX + Math.round(aw * 0.022),
        y: colTop + Math.round(ah * 0.14),
        body: card.heading,
        size: headingSize,
        fill: solidFill(UI_THEME.ink),
        name: `${card.name} heading`,
      });
      b.text({
        x: cardX + Math.round(aw * 0.022),
        y: colTop + Math.round(ah * 0.14) + headingSize + Math.round(ah * 0.02),
        body: card.body,
        size: bodySize,
        fill: solidFill(UI_THEME.inkSoft),
        name: `${card.name} body`,
      });
    }

    // Footer: hairline rule + page marker + wordmark.
    b.line({
      x1: ax + margin,
      y1: ay + fullH - footerH,
      x2: ax + fullW - margin,
      y2: ay + fullH - footerH,
      fill: solidFill(UI_THEME.hairline),
      name: "Footer rule",
    });
    b.text({
      x: ax + margin,
      y: ay + fullH - Math.round(footerH * 0.6),
      body: "01 / 12",
      size: clamp(Math.round(ah * 0.022), 9, 20),
      fill: solidFill(UI_THEME.muted),
      name: "Page number",
    });
    b.text({
      x: ax + fullW - margin - Math.round(aw * 0.12),
      y: ay + fullH - Math.round(footerH * 0.6),
      body: "Northwind",
      size: clamp(Math.round(ah * 0.022), 9, 20),
      fill: solidFill(UI_THEME.inkSoft),
      name: "Footer wordmark",
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
