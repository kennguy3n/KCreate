// LayoutThumbnail — faithful, polished mini-preview of a layout
// template page.
//
// The TemplatePicker previously showed a flat "N pages" placeholder box
// for each card and, in the detail pane, blocked out section rectangles
// positioned with `left: ${bounds.x * 100}%`. That math treats the
// section bounds as normalised [0,1] fractions, but
// `kcreate_core::project` authors them in ABSOLUTE document pixels
// (e.g. a pitch-deck title at `Bounds::new(60, 60, 840, 80)` on a
// 960×540 slide). Multiplying an 840px width by 100% pushes every
// section far off-canvas, which is why the preview rendered as a blank
// grey rectangle — the "no thumbnail" complaint.
//
// This component renders an SVG whose `viewBox` is the page's real
// pixel size (derived from `page_size` + `orientation`, exactly as the
// Rust core does via `PageLayout::dimensions_mm`), then draws each
// section at its real bounds with kind-aware, professional styling:
// headlines as real type, body as a lead line + paragraph rhythm,
// images as an accent gradient with a picture glyph, charts as a mini
// bar chart, footers/page-numbers as muted markers. The accent comes
// from the template's category tint (or its design tokens when present).
//
// A layout template is a *scaffold* — it has no real rendered content
// yet — so a high-quality vector preview is the faithful representation
// of what the user is choosing. This is UI chrome (the editing pipeline
// is still owned entirely by Rust); it runs no vector math against the
// document.

import type {
  DesignTokens,
  PageOrientation,
  PageSize,
  RgbaColor,
  SectionKind,
  TemplatePageDef,
  TemplateSectionDef,
} from "../../../shared/scene";

const PX_PER_MM = 96 / 25.4;

// Portrait `(width_mm, height_mm)` per `PageSizeId`, mirroring
// `kcreate_core::node::PageSize::dimensions_mm`. North-American sizes
// are converted from inches at exactly 25.4 mm/in to match the core.
const DIMENSIONS_MM: Record<string, readonly [number, number]> = {
  a3: [297, 420],
  a4: [210, 297],
  a5: [148, 210],
  letter: [8.5 * 25.4, 11 * 25.4],
  legal: [8.5 * 25.4, 14 * 25.4],
  tabloid: [11 * 25.4, 17 * 25.4],
  presentation_16x9: [5.625 * 25.4, 10 * 25.4],
  presentation_4x3: [7.5 * 25.4, 10 * 25.4],
};

/**
 * Pixel size of a template page after orientation is applied. Mirrors
 * `PageLayout::dimensions_mm` (which swaps width/height for landscape)
 * scaled by `96 / 25.4` px-per-mm — the same factor
 * `Project::apply_layout_template` uses to size the page node.
 */
export function pagePixelSize(page: {
  page_size: PageSize;
  orientation: PageOrientation;
}): { width: number; height: number } {
  const size = page.page_size;
  let widthMm: number;
  let heightMm: number;
  if (size.kind === "custom") {
    widthMm = size.width_mm;
    heightMm = size.height_mm;
  } else {
    const dim = DIMENSIONS_MM[size.kind] ?? DIMENSIONS_MM.a4;
    widthMm = dim?.[0] ?? 210;
    heightMm = dim?.[1] ?? 297;
  }
  let width = widthMm * PX_PER_MM;
  let height = heightMm * PX_PER_MM;
  if (page.orientation === "landscape") {
    const swap = width;
    width = height;
    height = swap;
  }
  return {
    width: Math.max(1, Math.round(width)),
    height: Math.max(1, Math.round(height)),
  };
}

// Neutral professional surface palette (mirrors the editor's UI theme).
// The accent is supplied per template so each category reads distinctly.
const SURFACE = "#FFFFFF";
const INK = "#0F172A";
const INK_SOFT = "#475569";
const MUTED = "#94A3B8";
const HAIRLINE = "#E2E8F0";

function clamp(value: number, lo: number, hi: number): number {
  return Math.max(lo, Math.min(hi, value));
}

function hexToRgb(hex: string): { r: number; g: number; b: number } {
  const clean = hex.replace("#", "");
  const full =
    clean.length === 3
      ? clean
          .split("")
          .map((c) => c + c)
          .join("")
      : clean;
  const r = parseInt(full.slice(0, 2), 16);
  const g = parseInt(full.slice(2, 4), 16);
  const b = parseInt(full.slice(4, 6), 16);
  return {
    r: Number.isNaN(r) ? 0 : r,
    g: Number.isNaN(g) ? 0 : g,
    b: Number.isNaN(b) ? 0 : b,
  };
}

/** Mix `hex` toward `target` by `t` (0 = hex, 1 = target). */
function mixHex(hex: string, target: string, t: number): string {
  const a = hexToRgb(hex);
  const b = hexToRgb(target);
  const k = clamp(t, 0, 1);
  const r = Math.round(a.r + (b.r - a.r) * k);
  const g = Math.round(a.g + (b.g - a.g) * k);
  const bl = Math.round(a.b + (b.b - a.b) * k);
  return `rgb(${r}, ${g}, ${bl})`;
}

function rgbaToCss(c: RgbaColor): string {
  // Channels are floats in [0,1] (wire-format contract). Be defensive
  // about a 0–255 producer by detecting any channel > 1.
  const scale = c.r > 1 || c.g > 1 || c.b > 1 ? 1 : 255;
  const r = Math.round(clamp(c.r * scale, 0, 255));
  const g = Math.round(clamp(c.g * scale, 0, 255));
  const b = Math.round(clamp(c.b * scale, 0, 255));
  const a = clamp(c.a, 0, 1);
  return `rgba(${r}, ${g}, ${b}, ${a})`;
}

/**
 * Resolve the accent colour for a thumbnail. Prefers a template's own
 * design tokens (looking for a conventional primary key) and otherwise
 * falls back to the category tint. The built-in layout templates carry
 * no design tokens today, so this is forward-looking for plugin- or
 * AI-contributed templates that ship a palette.
 */
export function resolveAccent(
  fallback: string,
  tokens?: DesignTokens | null,
): string {
  if (!tokens) return fallback;
  const colors = tokens.colors;
  if (!colors) return fallback;
  for (const key of ["primary", "accent", "brand", "brand-primary"]) {
    const hit = colors[key];
    if (hit) return rgbaToCss(hit);
  }
  // Fall back to the first declared token colour, if any.
  const first = Object.values(colors)[0];
  return first ? rgbaToCss(first) : fallback;
}

/** Estimate how many characters of `text` fit in `widthPx` at `fontPx`. */
function fitText(text: string, widthPx: number, fontPx: number): string {
  const avgCharPx = fontPx * 0.55;
  const maxChars = Math.max(1, Math.floor(widthPx / Math.max(1, avgCharPx)));
  if (text.length <= maxChars) return text;
  if (maxChars <= 1) return "…";
  return `${text.slice(0, maxChars - 1).trimEnd()}…`;
}

interface SectionVisualProps {
  section: TemplateSectionDef;
  accent: string;
  idPrefix: string;
}

function TitleSection({ section, accent }: SectionVisualProps): JSX.Element {
  const { x, y, width, height } = section.bounds;
  const fontPx = clamp(height * 0.46, 10, height * 0.8);
  const text = section.placeholder_text ?? "Title";
  const ruleWidth = clamp(width * 0.32, fontPx, width);
  return (
    <g>
      <text
        x={x}
        y={y + height * 0.5}
        fontSize={fontPx}
        fontWeight={700}
        fill={INK}
        dominantBaseline="middle"
      >
        {fitText(text, width, fontPx)}
      </text>
      <rect
        x={x}
        y={y + height * 0.78}
        width={ruleWidth}
        height={Math.max(2, height * 0.07)}
        rx={Math.max(1, height * 0.035)}
        fill={accent}
      />
    </g>
  );
}

function SubtitleSection({ section }: SectionVisualProps): JSX.Element {
  const { x, y, width, height } = section.bounds;
  const fontPx = clamp(height * 0.62, 9, height);
  const text = section.placeholder_text ?? "Subtitle";
  return (
    <text
      x={x}
      y={y + height * 0.6}
      fontSize={fontPx}
      fontWeight={500}
      fill={INK_SOFT}
      dominantBaseline="middle"
    >
      {fitText(text, width, fontPx)}
    </text>
  );
}

function BodyTextSection({ section }: SectionVisualProps): JSX.Element {
  const { x, y, width, height } = section.bounds;
  const leadFont = clamp(height * 0.1, 9, 22);
  const text = section.placeholder_text ?? "Body content";
  // A real lead line, then a paragraph rhythm of muted bars so the
  // block reads as set copy rather than an empty box.
  const lineH = Math.max(6, leadFont * 1.5);
  const barH = Math.max(3, leadFont * 0.5);
  const widths = [1, 0.97, 0.92, 0.85, 0.74, 0.95, 0.9, 0.6];
  const top = y + leadFont * 1.4;
  const available = Math.max(0, height - (top - y));
  const lineCount = Math.max(0, Math.floor(available / lineH));
  const bars: JSX.Element[] = [];
  for (let i = 0; i < lineCount; i += 1) {
    const w = widths[i % widths.length] ?? 0.8;
    bars.push(
      <rect
        key={i}
        x={x}
        y={top + i * lineH}
        width={width * w}
        height={barH}
        rx={barH / 2}
        fill={HAIRLINE}
      />,
    );
  }
  return (
    <g>
      <text
        x={x}
        y={y + leadFont}
        fontSize={leadFont}
        fontWeight={600}
        fill={INK_SOFT}
        dominantBaseline="middle"
      >
        {fitText(text, width, leadFont)}
      </text>
      {bars}
    </g>
  );
}

function ImageSection({
  section,
  accent,
  idPrefix,
}: SectionVisualProps): JSX.Element {
  const { x, y, width, height } = section.bounds;
  const gradientId = `${idPrefix}-img`;
  const light = mixHex(accent, "#FFFFFF", 0.55);
  // Picture glyph: a sun disc + two mountains, scaled to the block.
  const cx = x + width * 0.32;
  const cy = y + height * 0.34;
  const r = Math.min(width, height) * 0.1;
  const baseY = y + height * 0.78;
  const m1 = `${x + width * 0.18},${baseY} ${x + width * 0.42},${y + height * 0.46} ${x + width * 0.6},${baseY}`;
  const m2 = `${x + width * 0.46},${baseY} ${x + width * 0.68},${y + height * 0.54} ${x + width * 0.86},${baseY}`;
  return (
    <g>
      <defs>
        <linearGradient id={gradientId} x1="0" y1="0" x2="1" y2="1">
          <stop offset="0" stopColor={accent} />
          <stop offset="1" stopColor={light} />
        </linearGradient>
      </defs>
      <rect
        x={x}
        y={y}
        width={width}
        height={height}
        rx={Math.min(width, height) * 0.05}
        fill={`url(#${gradientId})`}
      />
      <circle cx={cx} cy={cy} r={r} fill="#FFFFFF" fillOpacity={0.85} />
      <polygon points={m1} fill="#FFFFFF" fillOpacity={0.7} />
      <polygon points={m2} fill="#FFFFFF" fillOpacity={0.55} />
    </g>
  );
}

function ChartSection({ section, accent }: SectionVisualProps): JSX.Element {
  const { x, y, width, height } = section.bounds;
  const baseline = y + height * 0.94;
  const axisY = baseline;
  const barCount = 6;
  const gap = width * 0.04;
  const barW = (width - gap * (barCount - 1)) / barCount;
  const heights = [0.45, 0.7, 0.55, 0.85, 0.65, 1];
  const usableH = height * 0.82;
  const light = mixHex(accent, "#FFFFFF", 0.4);
  const bars: JSX.Element[] = [];
  for (let i = 0; i < barCount; i += 1) {
    const frac = heights[i] ?? 0.6;
    const bh = usableH * frac;
    bars.push(
      <rect
        key={i}
        x={x + i * (barW + gap)}
        y={baseline - bh}
        width={barW}
        height={bh}
        rx={Math.min(barW * 0.3, 3)}
        fill={i === barCount - 1 ? accent : light}
      />,
    );
  }
  return (
    <g>
      {bars}
      <line
        x1={x}
        y1={axisY}
        x2={x + width}
        y2={axisY}
        stroke={HAIRLINE}
        strokeWidth={Math.max(1, height * 0.015)}
      />
    </g>
  );
}

function FooterSection({ section }: SectionVisualProps): JSX.Element {
  const { x, y, width, height } = section.bounds;
  const text = section.placeholder_text;
  if (text) {
    const fontPx = clamp(height * 0.5, 8, height);
    return (
      <text
        x={x}
        y={y + height * 0.6}
        fontSize={fontPx}
        fontWeight={500}
        fill={MUTED}
        dominantBaseline="middle"
      >
        {fitText(text, width, fontPx)}
      </text>
    );
  }
  return (
    <rect
      x={x}
      y={y + height * 0.35}
      width={width * 0.5}
      height={Math.max(2, height * 0.3)}
      rx={Math.max(1, height * 0.15)}
      fill={HAIRLINE}
    />
  );
}

function PageNumberSection({
  section,
  accent,
}: SectionVisualProps): JSX.Element {
  const { x, y, width, height } = section.bounds;
  const text = section.placeholder_text ?? "01";
  const fontPx = clamp(height * 0.6, 7, height);
  return (
    <g>
      <rect
        x={x}
        y={y + height * 0.15}
        width={width}
        height={Math.max(2, height * 0.7)}
        rx={Math.max(2, height * 0.35)}
        fill={mixHex(accent, "#FFFFFF", 0.82)}
      />
      <text
        x={x + width * 0.5}
        y={y + height * 0.55}
        fontSize={fontPx}
        fontWeight={600}
        fill={accent}
        textAnchor="middle"
        dominantBaseline="middle"
      >
        {fitText(text, width, fontPx)}
      </text>
    </g>
  );
}

function renderSection(
  section: TemplateSectionDef,
  index: number,
  accent: string,
  idPrefix: string,
): JSX.Element {
  const props: SectionVisualProps = {
    section,
    accent,
    idPrefix: `${idPrefix}-${index}`,
  };
  const kind: SectionKind = section.kind;
  switch (kind) {
    case "title":
      return <TitleSection key={index} {...props} />;
    case "subtitle":
      return <SubtitleSection key={index} {...props} />;
    case "body_text":
      return <BodyTextSection key={index} {...props} />;
    case "image":
      return <ImageSection key={index} {...props} />;
    case "chart":
      return <ChartSection key={index} {...props} />;
    case "footer":
      return <FooterSection key={index} {...props} />;
    case "page_number":
      return <PageNumberSection key={index} {...props} />;
    default:
      return <g key={index} />;
  }
}

export interface LayoutThumbnailProps {
  /** The template page to preview. */
  page: TemplatePageDef;
  /** Accent hex (category tint); overridden by `tokens` when present. */
  accent: string;
  /** Optional design tokens contributed by the template. */
  tokens?: DesignTokens | null;
  /** Accessible label for the preview (e.g. the template name). */
  label?: string;
  /** Overlay content (e.g. a page-count pill) drawn above the surface. */
  children?: React.ReactNode;
  /** Style overrides for the framed container. */
  style?: React.CSSProperties;
}

/**
 * Renders one template page as a polished, proportional SVG preview.
 */
export function LayoutThumbnail({
  page,
  accent,
  tokens,
  label,
  children,
  style,
}: LayoutThumbnailProps): JSX.Element {
  const { width, height } = pagePixelSize(page);
  const resolved = resolveAccent(accent, tokens);
  const sections = page.sections ?? [];
  return (
    <div
      style={{
        position: "relative",
        width: "100%",
        aspectRatio: `${width} / ${height}`,
        background: SURFACE,
        borderRadius: 8,
        overflow: "hidden",
        boxShadow: "inset 0 0 0 1px rgba(15, 23, 42, 0.08)",
        ...style,
      }}
    >
      <svg
        viewBox={`0 0 ${width} ${height}`}
        width="100%"
        height="100%"
        preserveAspectRatio="xMidYMid meet"
        role="img"
        aria-label={label ? `${label} preview` : "Layout preview"}
        style={{ display: "block" }}
      >
        <rect x={0} y={0} width={width} height={height} fill={SURFACE} />
        {sections.map((section, i) =>
          renderSection(section, i, resolved, "lt"),
        )}
      </svg>
      {children}
    </div>
  );
}
