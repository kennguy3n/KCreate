/**
 * Phase B2 — Pathfinder boolean-op panel.
 *
 * A small floating panel that surfaces the four
 * `kcreate_vector::boolean_operation` ops (Union / Subtract /
 * Intersect / Exclude) at the gesture boundary. Visible only when
 * the selection holds at least two `VectorLayer` nodes; otherwise
 * renders `null` so it doesn't clutter the canvas overlay.
 *
 * The panel is intentionally render-pure: it takes `selectedIds`
 * + a `nodes` lookup and the bridge entry-point, derives the
 * "can apply" predicate locally, and calls
 * `window.kcreate.canvas.pathBoolean` directly on click. Lifting
 * the bridge call into the panel keeps the surface small (no new
 * EditorContext action) and matches the pattern used by
 * `ExportPanel` for the export-format ops.
 *
 * After a successful op the panel asks the host to (1) refresh
 * the document tree (so the new result nodes appear) and (2)
 * re-select the result ids (so the user can immediately chain
 * another boolean or move the result). Errors are forwarded to
 * the host's status sink — same channel the rest of the editor
 * uses for transient toasts.
 *
 * The button labels mirror Inkscape's `Path > {Union, Difference,
 * Intersection, Exclusion}` menu: short, unambiguous, and the
 * standard naming the user already knows. We don't use icons
 * because the icon registry doesn't ship boolean-op glyphs and
 * text is more self-documenting at this size anyway.
 */

import { useCallback, useMemo, useRef, useState, type JSX } from "react";
import type { NodeInfo, PathBooleanOp } from "../../../shared/scene";
import { errorMessage } from "../lib/errorMessage";
import { colors, radius, spacing } from "../styles/tokens";

export interface PathfinderPanelProps {
  /** Selected node ids in iteration order (z-bottom-first). */
  selectedIds: string[];
  /**
   * Flattened document tree from `DocumentContext.nodes`. The panel
   * only needs `id` + `nodeType` from each entry, but we accept
   * the full `NodeInfo` so callers don't have to project the list
   * first.
   */
  nodes: NodeInfo[];
  /**
   * Host status sink (toast channel). Called with a non-null
   * string on error, and `null` to clear. Mirrors the same
   * `onStatus` prop signature as `RightPanel` / `ExportPanel`.
   */
  onStatus: (msg: string | null) => void;
  /**
   * Called after a successful boolean. Receives the new result
   * node ids in shape-emission order so the host can re-select
   * them and refresh the tree.
   */
  onApplied: (resultIds: string[]) => void;
}

const BOOLEAN_OPS: ReadonlyArray<{
  op: PathBooleanOp;
  label: string;
  hint: string;
}> = [
  { op: "union", label: "Union", hint: "Merge selected shapes (A ∪ B)" },
  {
    op: "subtract",
    label: "Subtract",
    hint: "First shape minus the rest (A \\ B)",
  },
  {
    op: "intersect",
    label: "Intersect",
    hint: "Keep overlap of all shapes (A ∩ B)",
  },
  {
    op: "exclude",
    label: "Exclude",
    hint: "Symmetric difference (A ⊕ B)",
  },
];

export function PathfinderPanel({
  selectedIds,
  nodes,
  onStatus,
  onApplied,
}: PathfinderPanelProps): JSX.Element | null {
  // Filter the selection to *just* vector layers. We do this in
  // the panel rather than at the EditorContext layer because the
  // selection is a general thing (you can multi-select rasters +
  // text + frames) and only the Pathfinder panel cares about the
  // vector-only subset.
  //
  // useMemo so we don't re-walk `nodes` on every keystroke / mouse
  // move; the `nodes` array identity is stable across non-tree
  // re-renders (DocumentContext caches it under refreshTree).
  const vectorSelection = useMemo(() => {
    if (selectedIds.length < 2) return [] as string[];
    const byId = new Map(nodes.map((n) => [n.id, n.nodeType] as const));
    return selectedIds.filter((id) => byId.get(id) === "VectorLayer");
  }, [selectedIds, nodes]);

  // Tracks whether a boolean is currently in flight. Split into
  // a ref + a state because the two roles want different update
  // semantics:
  //
  //   * `pendingRef` — the **serialization guard**. Mutated
  //     synchronously inside the click handler before any
  //     `await`, so a second click fired during the
  //     sub-microtask window between `setPending(true)` and the
  //     React re-render commit still sees `pendingRef.current ===
  //     true` and is rejected. A `useState` value here would
  //     close over the *render-time* snapshot inside `apply`'s
  //     closure and could let the racing click slip through
  //     before React schedules the next render. Refs are the
  //     correct primitive for "this changed *now*, not after the
  //     next commit" — same pattern we use for the pen tool's
  //     `dragStateRef` and the editor's `inlineTextEditRef`.
  //
  //   * `isPending` (state) — the **visual disabled flag**. Must
  //     be React state so the four `<button disabled={…}>` props
  //     re-render and the browser's built-in click suppression
  //     kicks in. This is the *primary* defense (browsers drop
  //     `pointerdown` on `disabled` buttons natively); the ref
  //     guard is a belt-and-braces secondary line that also
  //     covers synthetic `button.click()` calls from devtools /
  //     tests / MCP scripting where the disabled attribute is
  //     ignored.
  //
  // Without this gate a double-click on (say) Union fires the
  // gesture twice: the first delete-and-replace succeeds, and the
  // second sends the now-deleted source ids back to the bridge
  // which returns `PathBooleanError::SourceNotFound`. The error
  // toasts cleanly so it's not data-corrupting, but the user
  // still sees a confusing red message for a gesture that
  // visually succeeded. Devin Review #0002 (round 3) on PR #38;
  // upgraded from a pure `useState` guard to the ref + state
  // split in response to ANALYSIS_0001 (round 4) which correctly
  // pointed out the sub-microtask window.
  const pendingRef = useRef(false);
  const [isPending, setIsPending] = useState(false);

  // Bridge call. Bound separately so the disabled-state and the
  // four button click handlers all close over the same identity —
  // makes the click handler trivially stable for tests + memoised
  // children. Captures `vectorSelection` by reference so a stale
  // closure can't fire against an outdated selection.
  //
  // NOTE: `pendingRef` is intentionally NOT in the deps array.
  // Refs are stable across renders (the `useRef` return identity
  // never changes) and reading `.current` inside the callback
  // always sees the latest write — that's the whole point of
  // using a ref here. Listing it as a dep would do nothing
  // useful and would mislead future readers into thinking the
  // callback identity tracks the guard value.
  const apply = useCallback(
    async (op: PathBooleanOp) => {
      // Synchronous guard. Reads the ref (always current) and
      // *atomically* claims the in-flight slot before any
      // `await`. A second `apply()` invocation that happens in
      // the same task or the next microtask (e.g. a synthetic
      // double-click via `button.click(); button.click();` in
      // devtools) will short-circuit here.
      if (pendingRef.current) return;
      pendingRef.current = true;
      // Schedule the re-render that flips `disabled` on the four
      // buttons. This is the user-visible primary defense — the
      // ref guard above is the belt-and-braces secondary one.
      setIsPending(true);
      try {
        const result = await window.kcreate.canvas.pathBoolean(
          op,
          vectorSelection,
        );
        onStatus(null);
        onApplied(result);
      } catch (e) {
        // Surface the typed PathBooleanError display string to the
        // user. The bridge's thiserror format is already
        // user-readable ("source node {id} is a TextLayer, expected
        // a VectorLayer", "boolean op produced no output ...").
        onStatus(`${op} failed: ${errorMessage(e)}`);
      } finally {
        // Always release on completion — the panel will
        // re-render against the new (or unchanged) selection on
        // the next refresh and the user can fire the next
        // gesture. Ref release MUST come first so a click landing
        // between `setIsPending(false)` and React's commit can
        // proceed without being blocked by a stale-true ref.
        pendingRef.current = false;
        setIsPending(false);
      }
    },
    [vectorSelection, onStatus, onApplied],
  );

  // Hide the panel entirely when there's nothing to do. This is
  // the most common state (any time the user has 0–1 vector layers
  // selected) so spending zero pixels on it keeps the canvas
  // overlay uncluttered.
  if (vectorSelection.length < 2) return null;

  return (
    <div
      data-testid="pathfinder-panel"
      style={{
        position: "absolute",
        // Bottom-centre overlay — same conceptual slot as
        // Figma's "boolean group" pill. Above the status bar
        // (footer is 22px tall) with some breathing room.
        bottom: spacing.lg,
        left: "50%",
        transform: "translateX(-50%)",
        display: "flex",
        gap: spacing.xs,
        padding: spacing.xs,
        background: colors.bgSoft,
        border: `1px solid ${colors.border}`,
        borderRadius: radius.md,
        boxShadow: "0 4px 16px rgba(0,0,0,0.18)",
        // Sits ABOVE the canvas but BELOW dialogs / popovers
        // (dialogs use z-index > 1000 throughout the app).
        zIndex: 50,
        // Re-enable pointer events — the overlay layer they sit
        // on usually has pointer-events:none for the pen
        // preview etc.
        pointerEvents: "auto",
      }}
    >
      {BOOLEAN_OPS.map(({ op, label, hint }) => (
        <button
          key={op}
          type="button"
          title={hint}
          aria-label={hint}
          data-testid={`pathfinder-${op}`}
          disabled={isPending}
          onClick={() => {
            void apply(op);
          }}
          style={{
            padding: `${spacing.xs}px ${spacing.sm}px`,
            background: colors.bg,
            color: colors.text,
            border: `1px solid ${colors.border}`,
            borderRadius: radius.sm,
            fontSize: 12,
            // Pending buttons get the standard not-allowed cursor;
            // the visual greying comes from `disabled` itself via
            // the browser default. We don't theme it heavier than
            // that because the panel only flickers through the
            // pending state for ~10-100ms in practice.
            cursor: isPending ? "not-allowed" : "pointer",
            opacity: isPending ? 0.6 : 1,
          }}
        >
          {label}
        </button>
      ))}
      <span
        style={{
          alignSelf: "center",
          marginLeft: spacing.xs,
          fontSize: 11,
          color: colors.textMuted,
        }}
      >
        {vectorSelection.length} vector layers
      </span>
    </div>
  );
}
