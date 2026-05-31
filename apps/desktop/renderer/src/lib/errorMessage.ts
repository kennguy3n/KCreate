/**
 * Renderer-wide error-to-string helper.
 *
 * Replaces eight separate per-file copies that were drifting independently
 * (some returned `[object Object]` for non-Error objects via `String(e)`,
 * others used `JSON.stringify` for richer diagnostics, one mishandled
 * `undefined` by returning the literal `undefined` value because
 * `JSON.stringify(undefined)` is `undefined` — not a string — which
 * silently violated the `(unknown) => string` contract).
 *
 * This unified version is a strict **superset** of every prior variant:
 * - `Error` → `.message` (matches every prior variant).
 * - `string` → as-is (matches the rich variant; the standard variant's
 *   `String(s)` was a no-op for strings, so behaviour is preserved).
 * - `null` / `undefined` → the literal `"null"` / `"undefined"` (prior
 *   `String(null)` already returned `"null"`; `JSON.stringify(undefined)`
 *   was the silent contract violation we're closing).
 * - Other objects → `JSON.stringify(e)` when it returns a string, so a
 *   `{ code: 42 }` shows up as `{"code":42}` instead of
 *   `[object Object]`. Circular refs / unstringifiable values fall back
 *   to `String(e)` via try/catch.
 * - Primitives (number / boolean / bigint / symbol) → `String(e)`
 *   (`JSON.stringify` works for the first three but `String(e)` is the
 *   stable choice and matches what every prior variant produced).
 *
 * Devin Review #0005 on PR #35 (commit `2ebf4b5`) flagged the
 * cross-file duplication.
 */
export function errorMessage(e: unknown): string {
  if (e instanceof Error) return e.message;
  if (typeof e === "string") return e;
  if (e === null) return "null";
  if (e === undefined) return "undefined";
  if (typeof e === "object") {
    try {
      const json = JSON.stringify(e);
      if (typeof json === "string") return json;
    } catch {
      // Circular refs or unstringifiable values — fall through to
      // `String(e)` below.
    }
  }
  return String(e);
}
