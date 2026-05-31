// Inline SVG icon component backed by `iconRegistry.ts`. The
// registry was generated from `kennguy3n/svg-lucide` source SVGs
// (24×24 viewBox, 2px stroke, currentColor) and stores only the
// primitive children (`<path>`, `<circle>`, `<rect>`, `<line>`,
// `<polyline>`, `<polygon>`). This component supplies the wrapping
// `<svg>` element with the shared defaults so all icons keep the
// same visual weight and respect the parent CSS `color` (i.e. they
// theme automatically via `var(--kc-*)` tokens).
//
// Sizes used across the renderer:
//   * 14 — inline glyphs next to a text label (layer list, leaf
//          rows in left panel).
//   * 16 — toolbar / button glyphs (TopBar, RightPanel tabs,
//          ExportPanel preset buttons).
//   * 20 — card / pill glyphs (homepage `CreateCard` badge).
//   * 24 — large hero glyphs (homepage logo mark, splash dialogs).

import type { CSSProperties } from "react";

import {
  ICON_REGISTRY,
  type IconName,
  type IconNode,
} from "./iconRegistry";

export type { IconName } from "./iconRegistry";

export interface IconProps {
  /** Registered lucide icon name. See `iconRegistry.ts`. */
  name: IconName;
  /** Width and height in px. Defaults to 16 (toolbar size). */
  size?: number;
  /** Override the default 2 px stroke. */
  strokeWidth?: number;
  /**
   * Accessible label. When provided the icon renders with
   * `role="img"` and an inline `<title>`; otherwise it renders as
   * `aria-hidden="true"` so screen readers skip it. Decorative
   * icons next to a visible text label should leave this unset and
   * rely on the sibling label for accessibility.
   */
  title?: string;
  className?: string;
  style?: CSSProperties;
}

/// Convert kebab-cased SVG attribute names (e.g. `stroke-width`,
/// `fill-rule`) to React's camelCase equivalents. The lucide
/// primitives we register only carry positional attrs (`d`, `cx`,
/// `cy`, `r`, `rx`, `ry`, `x`, `y`, `width`, `height`, `points`,
/// `x1`, `x2`, `y1`, `y2`) plus the occasional `fill` (filled-dot
/// icons like `palette`). We still funnel everything through this
/// helper so any future icon registry additions Just Work without
/// silently dropping a kebab-cased prop.
function toReactAttrs(a: Readonly<Record<string, string>>): Record<string, string> {
  const out: Record<string, string> = {};
  for (const [k, v] of Object.entries(a)) {
    const camel = k.replace(/-([a-z])/g, (_m, c: string) => c.toUpperCase());
    out[camel] = v;
  }
  return out;
}

/// Exhaustiveness helper. Adding a new tag to `IconNodeTag` without
/// extending the `renderNode` switch causes `_exhaustive` to be
/// `never` at compile time and the `default` branch to throw at
/// runtime — belt-and-braces guard against the registry growing a
/// tag the renderer can't draw.
function assertNever(_exhaustive: never, tag: string): never {
  throw new Error(`Icon registry has unsupported tag: ${tag}`);
}

function renderNode(node: IconNode, key: number): JSX.Element {
  const props = toReactAttrs(node.a);
  switch (node.t) {
    case "path":
      return <path key={key} {...props} />;
    case "circle":
      return <circle key={key} {...props} />;
    case "rect":
      return <rect key={key} {...props} />;
    case "line":
      return <line key={key} {...props} />;
    case "polyline":
      return <polyline key={key} {...props} />;
    case "polygon":
      return <polygon key={key} {...props} />;
    default:
      return assertNever(node.t, (node as { t: string }).t);
  }
}

/// Default inline style applied to every rendered `<Icon>`. Hoisted to
/// module scope so the common case (no caller-supplied `style`) shares
/// a single object reference across every render of every icon in the
/// app. The previous per-render `{ display: 'inline-block', ... }`
/// literal allocated a fresh object on every render even when the
/// callsite never passed a `style` prop, which defeated React's
/// reference equality and forced the DOM to rewrite inline styles
/// each tick.
const DEFAULT_ICON_STYLE: CSSProperties = {
  display: "inline-block",
  flexShrink: 0,
  verticalAlign: "middle",
};

export function Icon({
  name,
  size = 16,
  strokeWidth = 2,
  title,
  className,
  style,
}: IconProps): JSX.Element {
  const nodes = ICON_REGISTRY[name];
  // `display: inline-block` + `flexShrink: 0` keeps the icon from
  // collapsing inside a flex row when the surrounding label is
  // wider than the available space. `verticalAlign: middle` lines
  // it up with adjacent text glyphs in inline contexts.
  //
  // When no caller-supplied `style` is passed (the majority of icon
  // usages across the app), reuse the shared `DEFAULT_ICON_STYLE`
  // module constant so React sees a stable `style` reference across
  // renders and skips the DOM inline-style rewrite. Only allocate a
  // fresh merged object when the caller is actually layering style
  // on top of the defaults. This mirrors the `ICON_ROW_STYLE`
  // hoist in `TopBar.tsx`.
  const mergedStyle: CSSProperties =
    style === undefined
      ? DEFAULT_ICON_STYLE
      : { ...DEFAULT_ICON_STYLE, ...style };
  return (
    <svg
      xmlns="http://www.w3.org/2000/svg"
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={strokeWidth}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden={title ? undefined : true}
      role={title ? "img" : undefined}
      focusable={false}
      className={className}
      style={mergedStyle}
    >
      {title ? <title>{title}</title> : null}
      {nodes.map((n, i) => renderNode(n, i))}
    </svg>
  );
}
