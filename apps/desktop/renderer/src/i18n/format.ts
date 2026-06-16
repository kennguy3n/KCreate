// Tiny ICU-lite message formatter.
//
// KCreate deliberately avoids a heavy i18n runtime (react-intl,
// i18next, FormatJS) — those ship hundreds of KB of parser +
// polyfills that work against the "super lightweight on device"
// goal. The renderer already targets `es2022`, so the platform
// `Intl` APIs (NumberFormat, PluralRules) are available for free in
// every Electron build. This module layers a ~1 KB formatter on top
// of them that understands the subset of the ICU MessageFormat
// grammar the UI actually needs:
//
//   * interpolation        "Open {name}"
//   * number formatting    "{count, number}"   (locale-aware grouping)
//   * pluralization        "{count, plural, one {# layer} other {# layers}}"
//
// Plural messages support exact selectors (`=0`) and the CLDR plural
// categories (zero/one/two/few/many/other) resolved via
// `Intl.PluralRules`, and the `#` token expands to the locale-formatted
// count. Nested interpolation inside a plural branch works because the
// chosen branch is fed back through the same formatter.

export type MessageVars = Record<string, string | number>;

/**
 * Return the index of the `}` that closes the `{` at `open`, honouring
 * nested braces. Returns -1 when the brace is unbalanced (the caller
 * then emits the remainder verbatim so a malformed catalog string can
 * never throw at render time).
 */
function matchBrace(input: string, open: number): number {
  let depth = 0;
  for (let i = open; i < input.length; i += 1) {
    const ch = input[i];
    if (ch === "{") depth += 1;
    else if (ch === "}") {
      depth -= 1;
      if (depth === 0) return i;
    }
  }
  return -1;
}

interface PluralBranch {
  /** `=N` exact selector, or a CLDR category (one/other/…). */
  readonly selector: string;
  readonly message: string;
}

/** Parse `one {…} other {…}` branch list out of a plural argument body. */
function parsePluralBranches(body: string): PluralBranch[] {
  const branches: PluralBranch[] = [];
  let i = 0;
  while (i < body.length) {
    // Skip whitespace / commas between branches.
    while (i < body.length && /[\s,]/.test(body.charAt(i))) i += 1;
    if (i >= body.length) break;
    // Read the selector token up to the opening brace.
    let selector = "";
    while (i < body.length && body[i] !== "{") {
      selector += body[i];
      i += 1;
    }
    if (i >= body.length || body[i] !== "{") break;
    const close = matchBrace(body, i);
    if (close === -1) break;
    const message = body.slice(i + 1, close);
    branches.push({ selector: selector.trim(), message });
    i = close + 1;
  }
  return branches;
}

/**
 * Render a single `{…}` placeholder body. Handles the three argument
 * shapes (`name`, `name, number`, `name, plural, …`) and falls back to
 * the literal `{body}` when the referenced variable is missing so a
 * translation bug surfaces visibly rather than silently dropping text.
 */
function renderArg(
  body: string,
  vars: MessageVars | undefined,
  localeId: string,
): string {
  const firstComma = body.indexOf(",");
  if (firstComma === -1) {
    const name = body.trim();
    const value = vars?.[name];
    return value === undefined ? `{${body}}` : String(value);
  }

  const name = body.slice(0, firstComma).trim();
  const rest = body.slice(firstComma + 1).trim();
  const value = vars?.[name];

  if (rest === "number") {
    const n = typeof value === "number" ? value : Number(value);
    return Number.isFinite(n)
      ? new Intl.NumberFormat(localeId).format(n)
      : `{${body}}`;
  }

  if (rest.startsWith("plural")) {
    const argBody = rest.slice("plural".length).replace(/^[\s,]+/, "");
    const branches = parsePluralBranches(argBody);
    const n = typeof value === "number" ? value : Number(value);
    const count = Number.isFinite(n) ? n : 0;

    const exact = branches.find((b) => b.selector === `=${count}`);
    const category = new Intl.PluralRules(localeId).select(count);
    const chosen =
      exact ??
      branches.find((b) => b.selector === category) ??
      branches.find((b) => b.selector === "other") ??
      branches[0];
    if (!chosen) return "";

    const formattedCount = new Intl.NumberFormat(localeId).format(count);
    const withHash = chosen.message.replace(/#/g, formattedCount);
    // Recurse so `{name}` inside a plural branch still interpolates.
    return formatMessage(withHash, vars, localeId);
  }

  // Unknown argument type — emit the variable value if we have one.
  return value === undefined ? `{${body}}` : String(value);
}

/**
 * Format an ICU-lite `template` with `vars`, resolving locale-sensitive
 * number/plural rules against `localeId` (a BCP-47 tag). Pure and
 * allocation-light: strings with no `{` are returned untouched.
 */
export function formatMessage(
  template: string,
  vars: MessageVars | undefined,
  localeId: string,
): string {
  if (template.indexOf("{") === -1) return template;
  let out = "";
  let i = 0;
  while (i < template.length) {
    const ch = template[i];
    if (ch !== "{") {
      out += ch;
      i += 1;
      continue;
    }
    const close = matchBrace(template, i);
    if (close === -1) {
      // Unbalanced brace — emit the rest verbatim.
      out += template.slice(i);
      break;
    }
    out += renderArg(template.slice(i + 1, close), vars, localeId);
    i = close + 1;
  }
  return out;
}
