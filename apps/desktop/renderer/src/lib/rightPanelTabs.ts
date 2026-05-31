/// Pure helpers for the `RightPanel` tab strip. Extracted from the
/// component body so the mode-change clamping logic can be unit
/// tested directly via `node:test` (mirrors the
/// `apps/desktop/renderer/tests/templates.test.mjs` strategy of
/// keeping component-orthogonal logic in `lib/`).
///
/// The clamping problem this helper solves: `RightPanel` derives its
/// tab strip from the editor `mode` prop (Accessibility appears in
/// `design`/`inspect`, Interaction in `prototype`, Preflight in
/// `layout`/`export`, Color in `design`/`layout`/`export`). The
/// active `tab` is held in component state and *outlives* the
/// transition between modes. So a user who lands on the Accessibility
/// tab in `design` mode, then switches the editor to `vector` mode,
/// has `tab === "accessibility"` but no Accessibility entry in the
/// recomputed tab strip. The pill is gone; the render block
/// (`{tab === "accessibility" && showAccessibility ? … : null}`)
/// renders nothing. Result: the right panel goes blank until the
/// user clicks any other pill — a UX paper-cut Devin Review surfaced
/// on PR #31 round 3 (`RightPanel.tsx:205`).
///
/// `clampTabToAvailable` answers the question "given the user's
/// last-chosen tab and the currently visible tab set, which tab
/// should be active?" The contract:
///
///   1. If `current` is in `available`, keep it. The user's
///      explicit choice survives every render that doesn't change
///      the strip composition (zero-cost in the common case).
///   2. If `current` is *not* in `available` (because a mode
///      transition just removed it), fall back to the first tab in
///      `available`. The strip is rendered in user-facing reading
///      order (Properties first), so the first tab is the most
///      sensible default-on-fallback target.
///   3. If `available` is empty (impossible in production — the
///      `BASE_TABS` + always-on `presence`/`constraints`/`tokens`/
///      `publish`/`encryption` guarantee at least ~11 entries), the
///      helper returns `current` unchanged. The component is
///      responsible for not calling us with an empty list; we
///      prefer this to a runtime throw because returning `current`
///      keeps the panel in a consistent (if degenerate) state.
export function clampTabToAvailable<T extends string>(
  current: T,
  available: ReadonlyArray<{ id: T }>,
): T {
  if (available.length === 0) return current;
  for (const entry of available) {
    if (entry.id === current) return current;
  }
  // Fallback: first tab in the rendered order. `available[0]` is
  // structurally guaranteed by the length check above; the
  // non-null assertion is correct here even though TS narrows the
  // index access to `T | undefined`.
  return available[0]!.id;
}
