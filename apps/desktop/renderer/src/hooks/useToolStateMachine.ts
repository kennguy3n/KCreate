/**
 * Pointer-event state machine for the editor canvas.
 *
 * Phase A3b — extracts the implicit drag-state machine that used to
 * live inside `EditorPage.tsx`'s `onCanvasPointer` callback (≈280
 * lines, three event types × four tool branches × a hold-to-pan
 * override). The pre-refactor implementation stored the active drag
 * in a single `useRef<{ kind: "create" | "move" | "pan"; tool;
 * pointerId; ...nine fields }>` and relied on conditional branches
 * inside each event handler to route to the right side effect.
 *
 * That worked, but it had three structural problems:
 *
 * 1. **Field overload.** The single drag record carried every field
 *    every variant might need (`movingNodeId`, `cumulativeDx`, etc.)
 *    even when the active variant didn't use them. `pan` set
 *    `movingNodeId: null`, `create` set `cumulativeDx: 0`, etc.
 *    Code reading the record had to dance around "is this field
 *    meaningful for the current `kind`?" checks at every callsite,
 *    and TypeScript couldn't help narrow.
 * 2. **No isolation of side effects.** The pointer handler interleaved
 *    bridge calls (`hitTest`, `moveNode`, `createRect`, etc.) with
 *    state-machine transitions, so a new tool (Pen, Node-edit) would
 *    have to graft onto a 280-line function and inevitably duplicate
 *    the routing logic.
 * 3. **No test surface.** The drag-state ref was private to
 *    `EditorPage`; the only way to exercise it was to mount the
 *    entire editor and synthesise pointer events. The state machine
 *    is the hardest-to-test path in the editor, and the existing
 *    architecture made unit-level coverage impossible.
 *
 * This hook gives every variant a discriminated-union state that
 * carries ONLY the fields that variant uses, isolates the bridge
 * side effects behind injected dependency callbacks, and exposes
 * `getState()` for direct test assertions. The pre-refactor
 * behaviour is preserved bit-for-bit (the bridge calls fire in the
 * exact same order with the same arguments); the only observable
 * difference is that the discriminated-union types let TypeScript
 * narrow each branch, removing the impossible field reads.
 *
 * Future tool variants (Pen, Vector-edit per the Phase B plan) will
 * extend the `ToolMachineState` union with their own variants and
 * add transition arms to `onCanvasPointer`. Side effects stay
 * isolated to dependency callbacks — the hook itself remains pure
 * w.r.t. the React tree and does not import `window.kcreate`
 * conditionally.
 */

import type { Dispatch, MutableRefObject, SetStateAction } from "react";
import type React from "react";
import { useCallback, useEffect, useRef } from "react";

import type { ViewportState } from "../components/CanvasHost";
import type { ToolId } from "../contexts/EditorContext";
import type {
  NodeInfo,
  PathSegmentWire,
  SnapGuide,
} from "../../../shared/scene";
import { errorMessage } from "../lib/errorMessage";

/**
 * Snap threshold in world units. 6 px @ zoom=1 keeps snaps tight
 * enough to feel deliberate but forgiving on a 4K display where the
 * cursor is travelling at high pixel velocity. Re-export so future
 * tools can use the same threshold and EditorPage can drop its
 * duplicate constant.
 */
export const SNAP_THRESHOLD_WORLD = 6;

/**
 * Pen-tool: minimum screen-space distance the cursor must travel
 * from the anchor point before we promote a `pointerdown` into a
 * smooth-anchor drag (vs. treating it as a corner-anchor click).
 *
 * 4 px is the same threshold most vector editors use (Illustrator,
 * Figma, Affinity Designer) and matches typical OS-level "drag
 * intent" thresholds (Windows SM_CXDRAG defaults to 4, GNOME's
 * `gtk-dnd-drag-threshold` defaults to 8 but most apps override to
 * 4). Below this, hand jitter on a click would silently insert
 * tiny tangent handles next to every "corner" anchor.
 *
 * Compared against screen pixels — converted into world units at
 * comparison time via `viewport.zoom`.
 */
export const PEN_DRAG_THRESHOLD_SCREEN = 4;

/**
 * Pen-tool: screen-space radius around the first anchor that
 * counts as a "close path" click. Same threshold the Figma /
 * Illustrator pen tools use (8 px). Bigger than
 * `PEN_DRAG_THRESHOLD_SCREEN` because closing is a high-precision
 * action — the user is deliberately aiming at a visible 6 px
 * anchor dot — but we still need a few pixels of slack so a click
 * 2 px away from the centre still closes.
 *
 * Compared against screen pixels — converted into world units at
 * comparison time via `viewport.zoom`.
 */
export const PEN_CLOSE_HIT_RADIUS_SCREEN = 8;

/**
 * Node-editor (Phase B3): screen-space radius around an anchor
 * point that counts as a "hit" for select / drag. Same threshold
 * Illustrator / Affinity Designer use for the Direct Select tool
 * (8 px). Matched against `PEN_CLOSE_HIT_RADIUS_SCREEN` so the
 * Pen tool's close-path and the Node tool's anchor-select feel
 * consistent.
 *
 * Compared against screen pixels — converted into world units at
 * comparison time via `viewport.zoom`.
 */
export const NODE_ANCHOR_HIT_RADIUS_SCREEN = 8;

/**
 * Node-editor (Phase B3): screen-space radius around a control
 * handle that counts as a "hit" for select / drag. Slightly
 * smaller than the anchor radius because (a) handles are visually
 * smaller (3 px dot vs. 6 px anchor square), so the bigger
 * radius would invite mis-grabs onto the wrong handle, and (b)
 * handle priority is HIGHER than anchor priority — when an
 * anchor's handle is rendered on top of its own anchor at high
 * zoom-out, the user almost always wants the handle.
 *
 * Compared against screen pixels.
 */
export const NODE_HANDLE_HIT_RADIUS_SCREEN = 6;

/**
 * One anchor in a pen-tool gesture. Stored in world space (matches
 * `kcreate_vector::PathSegment` coordinates) so the overlay can
 * paint directly without re-applying transforms and the commit
 * path can hand the geometry to the bridge unchanged.
 *
 * - `x`, `y`: the anchor point itself.
 * - `inHandle`: where the curve enters this anchor (the second
 *   control point of the *incoming* cubic). `null` ⇒ corner anchor
 *   (no smoothing on the way in).
 * - `outHandle`: where the curve leaves this anchor (the first
 *   control point of the *outgoing* cubic). `null` ⇒ corner anchor
 *   (no smoothing on the way out).
 *
 * Both handles are stored in absolute world coordinates (NOT
 * relative offsets from the anchor) so the overlay can render
 * them in-place without re-deriving the offset every paint.
 */
export interface PenAnchor {
  x: number;
  y: number;
  inHandle: { x: number; y: number } | null;
  outHandle: { x: number; y: number } | null;
}

/**
 * The in-flight anchor while the pointer is down during a pen
 * gesture. Promoted to a committed `PenAnchor` on `pointerup` —
 * the type of anchor (corner vs. smooth) is determined by whether
 * `drag` is non-null at release.
 */
export interface PenPendingAnchor {
  pointerId: number;
  /// Anchor world coords captured at `pointerdown`.
  x: number;
  y: number;
  /// Live cursor world coords if the cursor has moved past
  /// `PEN_DRAG_THRESHOLD_SCREEN` since `pointerdown`. `null`
  /// until the threshold is exceeded.
  drag: { x: number; y: number } | null;
}

/**
 * Explicit pointer-drag state.
 *
 * Each variant carries ONLY the fields that variant actually uses
 * (compare to the pre-refactor blob that carried `movingNodeId:
 * string | null`, `cumulativeDx: number`, etc. on every variant).
 * Narrowing on `state.kind` gives TypeScript enough information to
 * reject reads of fields that don't apply to the current state.
 *
 * - `idle`: no drag in flight. Pointer is either over the canvas
 *   resting or has left it; either way the next `pointerdown` is
 *   the gateway to a new state.
 * - `pan`: hold-to-pan gesture is dragging the viewport. We track
 *   the LAST screen position (not world) so each `pointermove`
 *   delta can be applied directly to `viewport.panX/panY` without
 *   round-tripping through the inverse transform.
 * - `move`: the user clicked a node with the select tool and is
 *   dragging it. We track world deltas so the bridge `moveNode`
 *   call on `pointerup` can apply a single coarse delta (one drag
 *   = one undo step), and we run the snap engine on each frame so
 *   the cumulative offset incorporates the snap correction.
 * - `create`: the user has a drawing tool active and is dragging out
 *   a new shape. Only the starting world coord is captured here —
 *   the ending coord is the live cursor position at `pointerup`,
 *   read directly from the event.
 */
export type ToolMachineState =
  | { kind: "idle" }
  | {
      kind: "pan";
      pointerId: number;
      /// Last sampled screen X. Drives the per-frame `panX` delta.
      lastScreenX: number;
      /// Last sampled screen Y. Drives the per-frame `panY` delta.
      lastScreenY: number;
    }
  | {
      kind: "move";
      pointerId: number;
      /// Captured at entry so a tool switch mid-drag doesn't
      /// retroactively change the drag's semantics. Matches the
      /// pre-refactor record's `tool` field exactly.
      tool: ToolId;
      movingNodeId: string;
      /// Last sampled world X. Drives the per-frame world-space delta.
      lastWorldX: number;
      /// Last sampled world Y.
      lastWorldY: number;
      /// Cumulative world delta since drag start. Folded with snap
      /// corrections on each frame; committed to the bridge as a
      /// single `moveNode(id, dx, dy)` call on `pointerup`.
      cumulativeDx: number;
      cumulativeDy: number;
    }
  | {
      kind: "create";
      pointerId: number;
      /// Tool captured at entry — same rationale as `move.tool`.
      tool: ToolId;
      startWorldX: number;
      startWorldY: number;
    }
  | {
      kind: "pen";
      /// Tool captured at gesture entry. Always `"pen"` today; kept
      /// for parity with `create.tool` / `move.tool` so a future
      /// tool that delegates into pen mode (e.g. a "pencil" tool
      /// that builds a path via the same machinery but commits as
      /// a different node type) has a stable place to record
      /// itself.
      tool: ToolId;
      /// Committed anchors, in draw order. Cleared on commit /
      /// cancel. Persists across `pointerup` because a pen gesture
      /// spans many click cycles — unlike `create`/`move`/`pan`
      /// which begin on `pointerdown` and end on `pointerup`.
      anchors: PenAnchor[];
      /// In-flight anchor while the pointer is down inside the
      /// gesture. `null` between clicks. Promoted to a committed
      /// anchor (corner if `drag` is null, smooth if `drag` is set)
      /// on `pointerup`.
      pending: PenPendingAnchor | null;
      /// Last cursor world position sampled while the pen state is
      /// active, regardless of whether the pointer is down. Drives
      /// the rubber-band preview line from the last committed
      /// anchor to the cursor in `PenOverlay`. `null` until the
      /// first cursor sample after the gesture starts.
      cursor: { x: number; y: number } | null;
    }
  | {
      kind: "nodeEdit";
      /// Tool captured at entry — kept for parity with `pen.tool`,
      /// `create.tool`, etc.
      tool: ToolId;
      /// The VectorLayer node id whose anchors / handles this
      /// gesture is editing. Captured on entry; a tool switch
      /// mid-gesture doesn't change which node receives the
      /// edits (matches `move.tool` capture-at-entry rationale).
      nodeId: string;
      /// Anchors in WORLD space (already projected through
      /// `translationX` / `translationY` from the bridge's
      /// `PathSnapshot`). The overlay renders directly from this
      /// array; the commit path re-projects back to path-local
      /// before handing to `canvas.pathSetSegments`.
      anchors: PenAnchor[];
      /// Mirrors `VectorPath.closed`. Preserved across the
      /// gesture so commits round-trip the open/closed flag.
      closed: boolean;
      /// Translation captured on entry. We do NOT re-read this
      /// per pointermove: any concurrent `canvas.moveNode` would
      /// shift the on-screen anchors out from under the user
      /// mid-drag, which is worse than the rare stale-translation
      /// risk (the node editor takes the selection's lock per
      /// pre-existing editor convention — see EditorPage.handlePointerDown).
      translationX: number;
      translationY: number;
      /// Set of indices into `anchors` that are currently
      /// selected. Single-anchor click clears+adds; shift-click
      /// toggles. Mirrors `EditorContext.selectedIds`'s set
      /// semantics. Empty when no anchor is selected (e.g. just
      /// after entering the tool).
      selectedAnchorIndices: ReadonlySet<number>;
      /// Live cursor world position. Used by the overlay's hover
      /// indicator and by `pointerdown` to test which anchor /
      /// handle (if any) is being grabbed. `null` until the
      /// pointer has moved over the canvas since entry.
      cursor: { x: number; y: number } | null;
      /// What the user has grabbed and is dragging right now,
      /// or `null` between drags. Hoisted into the variant
      /// (rather than a sibling state variant) so the overlay
      /// keeps its anchor list available while a drag is in
      /// flight — the user has to see the anchors they're
      /// pulling on.
      drag: NodeEditDrag | null;
      /// True when the cumulative drag delta has exceeded the
      /// minimum threshold to be considered a "real" drag (vs.
      /// a click+release with sub-pixel pointer jitter).
      /// Anchors / handles only commit their post-drag position
      /// to the bridge when this flag is set on `pointerup`;
      /// otherwise the gesture is treated as a select-only
      /// click. Resets to `false` on every new `pointerdown`.
      dragMoved: boolean;
    };

/**
 * Convert a sequence of pen anchors into the wire-format segment
 * list consumed by `canvas.createPath`. Adjacent corner anchors
 * connect via `line_to`; anchors with at least one non-null handle
 * connect via `cubic_to` (missing handles fall back to the anchor
 * coords, matching the convention `kcreate_vector::VectorPath` uses
 * for "no smoothing on this side"). When `closed` is true and the
 * gesture has ≥ 2 anchors, a closing segment (line or cubic) is
 * appended along with an explicit `close` so `VectorPath::bounds`
 * and the renderer-side fill both treat the path as closed.
 *
 * Exposed for the test suite — the rest of the module accesses it
 * through `commitPen` directly.
 */
export function anchorsToSegments(
  anchors: ReadonlyArray<PenAnchor>,
  closed: boolean,
): PathSegmentWire[] {
  if (anchors.length === 0) return [];
  const segs: PathSegmentWire[] = [];
  const first = anchors[0]!;
  segs.push({ op: "move_to", x: first.x, y: first.y });
  for (let i = 1; i < anchors.length; i++) {
    const prev = anchors[i - 1]!;
    const curr = anchors[i]!;
    segs.push(buildBetween(prev, curr));
  }
  if (closed && anchors.length >= 2) {
    const prev = anchors[anchors.length - 1]!;
    const curr = anchors[0]!;
    segs.push(buildBetween(prev, curr));
    segs.push({ op: "close" });
  }
  return segs;
}

/// Internal helper: pick `line_to` vs. `cubic_to` for the segment
/// from `prev` to `curr`. A pair of pure-corner anchors becomes a
/// straight line; any handle on either side promotes the segment
/// to a cubic with the missing-handle slots collapsed onto the
/// anchor coords (matching `VectorPath` convention).
function buildBetween(prev: PenAnchor, curr: PenAnchor): PathSegmentWire {
  if (prev.outHandle === null && curr.inHandle === null) {
    return { op: "line_to", x: curr.x, y: curr.y };
  }
  return {
    op: "cubic_to",
    ctrl1: prev.outHandle ?? { x: prev.x, y: prev.y },
    ctrl2: curr.inHandle ?? { x: curr.x, y: curr.y },
    end: { x: curr.x, y: curr.y },
  };
}

/**
 * Phase B3 — convert the bridge wire-format `PathSegmentWire[]`
 * stream back into the anchor representation the node editor
 * works with. Round-trips with `anchorsToSegments` for any
 * sequence the pen tool emits:
 *
 *   - `move_to` at index 0 seeds the first anchor.
 *   - `line_to` appends a corner anchor; the inbound/outbound
 *     handles for the segment-between are both null.
 *   - `cubic_to` appends an anchor at `end`; `ctrl1` becomes the
 *     PREVIOUS anchor's `outHandle` (if it differs from the
 *     anchor coords — equal means "no handle"); `ctrl2` becomes
 *     the NEW anchor's `inHandle` under the same rule.
 *   - `quad_to` is elevated to a cubic-equivalent via the
 *     standard 2/3rds Bezier conversion (Q[p0, c, p1] → C[p0,
 *     p0+2/3*(c-p0), p1+2/3*(c-p1), p1]). The node editor's
 *     internal model is always cubic — pen / SVG import paths
 *     may produce quad segments but the user-facing handle UI
 *     doesn't need to distinguish.
 *   - `close` flips `closed: true`. If present after a
 *     `line_to`/`cubic_to` that lands on the start anchor, the
 *     redundant duplicate-of-first anchor is collapsed (we keep
 *     the closing-segment's handles on the first anchor's inbound
 *     side, which is exactly how `anchorsToSegments` would emit
 *     them on the next round trip).
 *
 * Returns `{ anchors, closed }`. Empty input returns `{ anchors:
 * [], closed: false }`. A degenerate path missing `move_to` at
 * index 0 returns `{ anchors: [], closed: false }` (matches the
 * bridge's `MissingMoveTo` rejection — defense in depth so the
 * overlay doesn't render garbage if the contract slips).
 */
export function segmentsToAnchors(
  segments: ReadonlyArray<PathSegmentWire>,
): { anchors: PenAnchor[]; closed: boolean } {
  if (segments.length === 0 || segments[0]?.op !== "move_to") {
    return { anchors: [], closed: false };
  }
  const anchors: PenAnchor[] = [];
  const first = segments[0]!;
  anchors.push({
    x: first.x,
    y: first.y,
    inHandle: null,
    outHandle: null,
  });
  let closed = false;
  for (let i = 1; i < segments.length; i++) {
    const seg = segments[i]!;
    if (seg.op === "move_to") {
      // Multi-subpath imports — current node editor handles only
      // a single subpath, so trailing `move_to`s are dropped
      // rather than silently merged with the preceding anchor.
      // Matches the bridge's single-VectorPath model. Future
      // multi-subpath support extends `nodeEdit` with a subpath
      // index; this branch becomes the seam.
      break;
    }
    if (seg.op === "close") {
      closed = true;
      continue;
    }
    if (seg.op === "line_to") {
      anchors.push({
        x: seg.x,
        y: seg.y,
        inHandle: null,
        outHandle: null,
      });
      continue;
    }
    // Promote both cubic and quad to anchor + handle pair. The
    // previous anchor receives `outHandle = ctrl1` (or
    // 2/3-derived for quads); the new anchor receives `inHandle
    // = ctrl2`. Coincident-with-anchor handles collapse to
    // `null` so the next `anchorsToSegments` emits `line_to`
    // (preserves round-trip fidelity for already-corner anchors
    // that happened to be encoded as zero-bend cubics).
    let ctrl1: { x: number; y: number };
    let ctrl2: { x: number; y: number };
    let end: { x: number; y: number };
    if (seg.op === "cubic_to") {
      ctrl1 = seg.ctrl1;
      ctrl2 = seg.ctrl2;
      end = seg.end;
    } else {
      // quad_to — 2/3 Bezier elevation.
      const prev = anchors[anchors.length - 1]!;
      ctrl1 = {
        x: prev.x + (2 / 3) * (seg.ctrl.x - prev.x),
        y: prev.y + (2 / 3) * (seg.ctrl.y - prev.y),
      };
      ctrl2 = {
        x: seg.end.x + (2 / 3) * (seg.ctrl.x - seg.end.x),
        y: seg.end.y + (2 / 3) * (seg.ctrl.y - seg.end.y),
      };
      end = seg.end;
    }
    const prev = anchors[anchors.length - 1]!;
    const prevHandle =
      ctrl1.x === prev.x && ctrl1.y === prev.y ? null : ctrl1;
    if (prevHandle !== null) {
      prev.outHandle = prevHandle;
    }
    const inHandle =
      ctrl2.x === end.x && ctrl2.y === end.y ? null : ctrl2;
    anchors.push({
      x: end.x,
      y: end.y,
      inHandle,
      outHandle: null,
    });
  }
  // If the last appended anchor coincides with the first (the
  // standard close-with-explicit-segment-back-to-start pattern
  // emitted by `anchorsToSegments`), fold its `inHandle` onto
  // the first anchor's inbound slot and drop it. Without this
  // collapse, round-tripping a closed path would gain a phantom
  // duplicate anchor every cycle.
  if (closed && anchors.length >= 2) {
    const last = anchors[anchors.length - 1]!;
    const head = anchors[0]!;
    if (last.x === head.x && last.y === head.y) {
      if (last.inHandle !== null) {
        head.inHandle = last.inHandle;
      }
      anchors.pop();
    }
  }
  return { anchors, closed };
}

/**
 * Phase B3 — discriminated union of "what's being grabbed" inside
 * an active node-edit drag. Anchors and handles are addressed by
 * their index in the parent path's anchor array; handles also
 * carry which side (`in` or `out`) is being dragged.
 *
 * Drag state is kept FLAT inside the `nodeEdit` variant rather
 * than nested under `drag: NodeEditDrag | null` so the
 * discriminated union narrows cleanly: `state.kind ===
 * "nodeEdit"` only tells you the user is in the node-edit tool,
 * `state.drag?.kind === "anchor"` then tells you what's in
 * flight. Mirrors `pen.pending` shape.
 */
export type NodeEditDrag =
  | {
      kind: "anchor";
      pointerId: number;
      anchorIndex: number;
      /// Cumulative world-space delta since drag start; applied
      /// to anchor coords (and to the anchor's own handles, so
      /// in/out handles travel with the anchor). Folded into the
      /// committed anchor set on `pointerup`.
      cumulativeDx: number;
      cumulativeDy: number;
      /// Last world-space cursor sample; per-frame delta is
      /// `(world - last)`. Re-sampled on every pointermove.
      lastWorldX: number;
      lastWorldY: number;
    }
  | {
      kind: "handle";
      pointerId: number;
      anchorIndex: number;
      side: "in" | "out";
      /// World-space coords the handle should be moved to on the
      /// next frame. Unlike anchor drags we replace the handle
      /// position outright instead of accumulating a delta — a
      /// dragged handle "follows the cursor" rather than "tracks
      /// its grab offset". Matches Illustrator / Figma behaviour
      /// and avoids handle drift on pointer-rate noise.
      cursorWorldX: number;
      cursorWorldY: number;
    };

/**
 * Dependency injection surface. Everything the hook needs to do its
 * job is passed in explicitly so callers (including tests) can
 * substitute alternative implementations. No `window.kcreate.*`
 * call sites are direct here — wait, that's not quite true: the
 * bridge IS still called directly inside the hook, because moving
 * every bridge entry through a callback would balloon the dep
 * surface to 9+ functions and obscure the state machine. The
 * tradeoff: tests install a `window.kcreate` stub (via the existing
 * `installKcreateStub` helper) and pass real refs and setters.
 */
export interface ToolStateMachineDeps {
  /// Currently selected tool. Read on `pointerdown` to decide which
  /// state to transition into. Subsequent transitions use the
  /// `tool` captured in the active state, not this prop — so a tool
  /// switch mid-drag doesn't corrupt the in-flight drag.
  tool: ToolId;
  /// Live viewport (pan + zoom). Used for screen→world inversion.
  /// Because this is a state value, every viewport change re-creates
  /// `onCanvasPointer` via `useCallback` — that's intentional, since
  /// the screen→world transform is closure-captured.
  viewport: ViewportState;
  /// Hold-to-pan armed status. Read inside the handler (not deps)
  /// because the pan gesture must beat the active tool's hit-test
  /// at the instant of `pointerdown`. Ref is the only way to read
  /// "the live value at handler-invocation time" without depending
  /// on the value (which would re-create the handler on every
  /// keystroke).
  panActiveRef: MutableRefObject<boolean>;
  /// Read-latest node mirror. Used by the snap-engine query during
  /// `move` drags to look up the moving node's current bounds.
  nodesRef: MutableRefObject<NodeInfo[]>;
  /// World-space cursor sample. Written on every pointer event so
  /// non-state-machine consumers (paste-at-cursor, double-click
  /// hit-test for inline text editing) can read the last known
  /// cursor position. Owned by the hook because the hook is the
  /// only place that has the screen→world transform handy.
  lastCursorWorldRef: MutableRefObject<{ x: number; y: number } | null>;
  /// `EditorContext.setSelectedIds`. Stable identity.
  setSelectedIds: Dispatch<SetStateAction<string[]>>;
  /// `EditorContext.setViewport`. Stable identity.
  setViewport: Dispatch<SetStateAction<ViewportState>>;
  /// `EditorContext.setSnapGuides`. Stable identity. Cleared to `[]`
  /// on `pointerup` so stale guides don't linger.
  setSnapGuides: Dispatch<SetStateAction<SnapGuide[]>>;
  /// Status sink — invoked on bridge failures so the status bar can
  /// surface the message. Wraps `EditorContext.setStatusMessage`
  /// inside EditorPage, but kept as a generic `(msg: string) => void`
  /// here so the hook doesn't depend on the editor context type.
  onError: (msg: string) => void;
  /// Composed full-resync, invoked after `moveNode` /
  /// `createRect|Ellipse|Line|Text` commits. Returns whatever the
  /// caller's `refreshTree` returns; the hook ignores the value.
  onAfterCommit: () => Promise<unknown> | void;
}

/**
 * Hook return surface.
 *
 * - `onCanvasPointer`: the React pointer handler to attach to the
 *   canvas element. Memoised via `useCallback` with `viewport` as
 *   its only volatile dep — every other dep is a ref or a stable
 *   setter.
 * - `getState`: read the current state machine state. Backed by a
 *   ref, so reads do not trigger re-renders. Use this in the render
 *   path ONLY to read state already reactively driven by some other
 *   value (e.g. `EditorPage`'s cursor logic reads `getState()` only
 *   after a re-render driven by `viewport` / `panActive` / `tool`).
 *   Tests can call it synchronously after `onCanvasPointer(...)` to
 *   assert transitions.
 * - `getLastCursorWorld`: read the last world-space cursor sample.
 *   Returns `null` until the user has moved the pointer over the
 *   canvas at least once. `EditorPage`'s `handlePaste` reads this
 *   to position the new subtree near the cursor.
 * - `commitPen`: promote the in-flight pen gesture to a real
 *   `VectorLayer` via `canvas.createPath`. No-op when the state
 *   machine is not in the `"pen"` variant or has fewer than 2
 *   committed anchors. Returns the new node id on success, or
 *   `null` if there was nothing to commit. Resolves AFTER
 *   `onAfterCommit` has run so callers can chain a refresh.
 * - `cancelPen`: discard the in-flight pen gesture without
 *   committing. No-op when the state machine is not in the
 *   `"pen"` variant. Returns `true` if a gesture was actually
 *   cancelled (so `EditorPage`'s Escape handler can decide whether
 *   to fall through to "clear selection").
 * - `subscribe`: register a listener that fires on every state
 *   transition. Used by the `PenOverlay` component (and future
 *   tool overlays) to re-paint when the in-flight gesture
 *   advances. Pointer events otherwise mutate the state machine
 *   without re-rendering React, so without an explicit subscribe
 *   surface the overlay would be invisible. Returns an unsubscribe
 *   function.
 * - `enterNodeEdit`: Phase B3 — fetch the given VectorLayer's
 *   geometry via `canvas.pathGetSegments`, project anchors into
 *   world space, and transition into the `nodeEdit` variant. If
 *   another tool's gesture is in flight (`pen`, `move`, etc.)
 *   the call is rejected with a status toast — same defensive
 *   posture as `commitPen`'s `state.kind === "pen"` guard.
 *   Resolves with `true` on success, `false` if the bridge call
 *   failed or the state machine wasn't idle.
 * - `commitNodeEdit`: Phase B3 — push the current `nodeEdit`
 *   variant's anchors back through `canvas.pathSetSegments`,
 *   then return to `idle`. No-op (returns `false`) when not in
 *   `nodeEdit`. Resolves AFTER `onAfterCommit` so the caller can
 *   chain a refresh.
 * - `cancelNodeEdit`: Phase B3 — drop the in-flight `nodeEdit`
 *   variant without committing, returning to `idle`. Used by
 *   the Escape shortcut. Returns `true` if a gesture was
 *   actually cancelled.
 */
export interface ToolStateMachine {
  onCanvasPointer: (e: React.PointerEvent<HTMLCanvasElement>) => void;
  getState: () => ToolMachineState;
  getLastCursorWorld: () => { x: number; y: number } | null;
  commitPen: () => Promise<string | null>;
  cancelPen: () => boolean;
  enterNodeEdit: (nodeId: string) => Promise<boolean>;
  commitNodeEdit: () => Promise<boolean>;
  cancelNodeEdit: () => boolean;
  subscribe: (listener: () => void) => () => void;
}

const IDLE: ToolMachineState = { kind: "idle" };

export function useToolStateMachine(
  deps: ToolStateMachineDeps,
): ToolStateMachine {
  const {
    tool,
    viewport,
    panActiveRef,
    nodesRef,
    lastCursorWorldRef,
    setSelectedIds,
    setViewport,
    setSnapGuides,
    onError,
    onAfterCommit,
  } = deps;

  // The whole point of the state machine: a single source of truth
  // for the drag state, with discriminated-union typing so each
  // variant exposes only the fields that variant uses. Ref-backed
  // because pointer events fire faster than React commits — a
  // useState would either drop events (stale closure on the
  // setter) or churn renders (one per pointermove). Same tradeoff
  // the pre-refactor code made.
  const stateRef = useRef<ToolMachineState>(IDLE);

  // Subscriber registry for the `subscribe`/`notify` surface. Used
  // by `PenOverlay` (and future tool overlays) to re-paint when an
  // in-flight pen gesture advances — without this, the overlay
  // would render once with stale state because pointer events
  // mutate the ref without triggering React. A plain `Set` is used
  // because (a) the number of listeners is tiny (1, in practice)
  // and (b) the contract is "fire-and-forget" — no listener should
  // throw, and any that does won't corrupt the registry because
  // the call is wrapped in a try/catch.
  const listenersRef = useRef<Set<() => void>>(new Set());
  // Saved pen state across hold-to-pan interruptions. Pen is the
  // first multi-cycle tool gesture in the codebase (every click
  // commits an anchor that survives across pointerdown/up cycles),
  // so the naive "pan replaces state" pattern that worked for the
  // single-cycle `create` / `move` / `select` tools silently loses
  // every laid-down anchor the moment the user holds Space. We
  // stash the pen state here on pan entry and restore it on pan
  // exit, mirroring Figma / Illustrator behaviour. `null` when no
  // gesture is in flight.
  const savedPenStateRef = useRef<ToolMachineState | null>(null);
  const notify = useCallback((): void => {
    for (const listener of listenersRef.current) {
      try {
        listener();
      } catch {
        // Listeners are expected to be react-state setters; the
        // only way they throw is if the consumer unmounted between
        // the pointer event firing and the listener executing.
        // Swallow so the state machine survives a buggy subscriber.
      }
    }
  }, []);
  const subscribe = useCallback((listener: () => void): (() => void) => {
    const set = listenersRef.current;
    set.add(listener);
    return () => {
      set.delete(listener);
    };
  }, []);

  // Keep the callback deps stable across renders. `onError` and
  // `onAfterCommit` come in as fresh closures on every parent
  // render; capturing them in refs lets the bridge-call IIFEs read
  // the latest version without re-creating the handler.
  const onErrorRef = useRef(onError);
  onErrorRef.current = onError;
  const onAfterCommitRef = useRef(onAfterCommit);
  onAfterCommitRef.current = onAfterCommit;

  // Stable references to the volatile prop values that the handler
  // needs but should not depend on. `tool` is the most-frequently-
  // touched of these (a tool switch is a single setState in
  // EditorContext), so re-creating the handler on every tool change
  // is fine — but reading from a ref is what the snap-engine query
  // does for `nodesRef.current`, and consistency reads better.
  const toolRef = useRef(tool);
  toolRef.current = tool;

  // Screen→world inversion lives here, not in EditorPage. The
  // viewport is the only volatile dep of the handler; capturing it
  // via closure means every viewport change re-creates the handler,
  // which is what we want (otherwise pan/zoom changes would not be
  // reflected in subsequent hit tests).
  const screenToWorld = useCallback(
    (sx: number, sy: number): { x: number; y: number } => ({
      x: (sx - viewport.panX) / viewport.zoom,
      y: (sy - viewport.panY) / viewport.zoom,
    }),
    [viewport],
  );

  /**
   * Commit the in-flight pen gesture as a `VectorLayer` via
   * `canvas.createPath`. Resolves with the new node id on success
   * or `null` if there was nothing to commit (idle / fewer than 2
   * anchors). State is reset to `idle` BEFORE the bridge call so
   * a tool-switch or Escape mid-commit cannot trample on the
   * gesture being committed.
   *
   * `closed` is forwarded as both (a) the `VectorPath.closed` flag
   * (controls fill / hit-test) and (b) the trigger for appending a
   * closing segment + explicit `close` op to the segment list.
   *
   * Shared between three call sites:
   *   1. `pointerdown` when the click lands inside the first
   *      anchor's hit radius (closes the path with `closed=true`).
   *   2. `commitPen` public method (Enter shortcut / tool-switch
   *      effect; commits open with `closed=false`).
   *   3. The tool-switch `useEffect` below (commits open).
   */
  const commitPenGesture = useCallback(
    async (closed: boolean): Promise<string | null> => {
      const state = stateRef.current;
      if (state.kind !== "pen") return null;
      // Need at least 2 anchors for a meaningful path — a single
      // anchor would deserialize to one `MoveTo` and reject in
      // `canvas_create_path` (`CreatePathError::Empty`-adjacent —
      // it'd be `MissingMoveTo`-passing but zero-length geometry).
      // We catch it here so the bridge call is never even fired
      // for trivially invalid gestures.
      if (state.anchors.length < 2) {
        // Reset state so the next pen click starts fresh, even
        // though we didn't commit. Otherwise a 1-anchor gesture
        // would silently persist across the user's "give up and
        // switch tools" action. Also clear any saved-across-pan
        // shadow so it can't rehydrate the abandoned gesture.
        stateRef.current = IDLE;
        savedPenStateRef.current = null;
        notify();
        return null;
      }
      const segments = anchorsToSegments(state.anchors, closed);
      // Reset to idle BEFORE the async bridge call so subsequent
      // events (tool switch, Escape) cannot mutate the in-flight
      // gesture. The bridge call is fire-and-forget from the
      // state machine's perspective.
      stateRef.current = IDLE;
      savedPenStateRef.current = null;
      notify();
      try {
        const newId = await window.kcreate.canvas.createPath(
          null,
          segments,
          closed,
          null,
        );
        await window.kcreate.canvas.setSelection([newId]);
        await onAfterCommitRef.current();
        return newId;
      } catch (err) {
        onErrorRef.current(`pen commit failed: ${errorMessage(err)}`);
        return null;
      }
    },
    [notify],
  );

  /**
   * Discard the in-flight pen gesture without committing.
   * Returns `true` if the cancellation actually consumed a pen
   * gesture (so `EditorPage`'s Escape handler can decide whether
   * to fall through to "clear selection").
   *
   * Shared between two call sites:
   *   1. `cancelPen` public method (Escape shortcut).
   *   2. The tool-switch `useEffect` for safety when a future
   *      caller wants to abandon rather than auto-commit.
   *
   * Sync (no bridge call) because cancellation has no
   * persistent-state side effect — the document is unchanged.
   */
  const cancelPenGesture = useCallback((): boolean => {
    const state = stateRef.current;
    // Also treat "pan with a saved pen shadow" as a cancellable
    // gesture so the user can Escape out of a pen path even
    // while still holding Space — otherwise the shadow rehydrates
    // an abandoned path on pan release.
    const hasShadow = savedPenStateRef.current !== null;
    if (state.kind !== "pen" && !hasShadow) return false;
    stateRef.current = IDLE;
    savedPenStateRef.current = null;
    notify();
    return true;
  }, [notify]);

  /**
   * Phase B3 — enter `nodeEdit` for the given VectorLayer node.
   * Round-trips through `canvas.pathGetSegments`, converts
   * `PathSnapshot.segments` (path-local) into the anchor
   * representation, projects every anchor + handle into world
   * space using the snapshot's `translationX` / `translationY`,
   * and atomically transitions into the `nodeEdit` variant.
   *
   * Refuses (returns `false`) if the state machine is not in
   * `idle` — same posture as `commitPen`'s `state.kind === "pen"`
   * guard. The caller is expected to commit / cancel any active
   * gesture first.
   *
   * On bridge failure the error is surfaced via `onError` and the
   * state machine stays in `idle`. Callers should not assume a
   * transition happened on `true`-vs-`false` alone; reading
   * `getState()` after `await` is the source of truth.
   */
  const enterNodeEdit = useCallback(
    async (nodeId: string): Promise<boolean> => {
      const state = stateRef.current;
      if (state.kind !== "idle") {
        onErrorRef.current(
          "node edit refused: another gesture is in flight",
        );
        return false;
      }
      try {
        const snap = await window.kcreate.canvas.pathGetSegments(nodeId);
        const { anchors: pathLocal, closed } = segmentsToAnchors(
          snap.segments,
        );
        // Project path-local → world by adding the node's
        // translation. The node editor renders directly off
        // these world-space anchors; the commit path
        // re-projects back to path-local using the same
        // translation captured here.
        const anchors: PenAnchor[] = pathLocal.map((a) => ({
          x: a.x + snap.translationX,
          y: a.y + snap.translationY,
          inHandle:
            a.inHandle === null
              ? null
              : {
                  x: a.inHandle.x + snap.translationX,
                  y: a.inHandle.y + snap.translationY,
                },
          outHandle:
            a.outHandle === null
              ? null
              : {
                  x: a.outHandle.x + snap.translationX,
                  y: a.outHandle.y + snap.translationY,
                },
        }));
        stateRef.current = {
          kind: "nodeEdit",
          tool: toolRef.current,
          nodeId,
          anchors,
          closed,
          translationX: snap.translationX,
          translationY: snap.translationY,
          selectedAnchorIndices: new Set<number>(),
          cursor: null,
          drag: null,
          dragMoved: false,
        };
        notify();
        return true;
      } catch (err) {
        onErrorRef.current(
          `node edit enter failed: ${errorMessage(err)}`,
        );
        return false;
      }
    },
    [notify],
  );

  /**
   * Phase B3 — commit the current `nodeEdit` gesture's anchors
   * back through `canvas.pathSetSegments`. Projects world-space
   * anchors back to path-local using the `translationX` /
   * `translationY` captured at entry (NOT a fresh read — see
   * the doc on `nodeEdit.translationX`). Records ONE undoable
   * operation per call.
   *
   * No-op (returns `false`) when not in `nodeEdit`. Resolves
   * AFTER `onAfterCommit` so the caller can chain a refresh.
   */
  const commitNodeEdit = useCallback(async (): Promise<boolean> => {
    const state = stateRef.current;
    if (state.kind !== "nodeEdit") return false;
    // Need at least a single anchor for a path that the bridge
    // will accept (`canvas.pathSetSegments` rejects empty input
    // and missing-`MoveTo` paths). An anchor list with one entry
    // serializes to a single `move_to` plus the optional `close`,
    // both of which the bridge accepts.
    if (state.anchors.length === 0) {
      onErrorRef.current(
        "node edit commit refused: path has no anchors",
      );
      return false;
    }
    // Re-project world → path-local using the translation
    // captured on entry. If `translationX/Y` is `(0, 0)` this is
    // an identity transform and `anchors` round-trip unchanged.
    const local: PenAnchor[] = state.anchors.map((a) => ({
      x: a.x - state.translationX,
      y: a.y - state.translationY,
      inHandle:
        a.inHandle === null
          ? null
          : {
              x: a.inHandle.x - state.translationX,
              y: a.inHandle.y - state.translationY,
            },
      outHandle:
        a.outHandle === null
          ? null
          : {
              x: a.outHandle.x - state.translationX,
              y: a.outHandle.y - state.translationY,
            },
    }));
    const segments = anchorsToSegments(local, state.closed);
    const { nodeId, closed } = state;
    // Reset to idle BEFORE the async bridge call so subsequent
    // events (tool switch, Escape) cannot mutate the in-flight
    // gesture. Same discipline as `commitPenGesture`.
    stateRef.current = IDLE;
    notify();
    try {
      await window.kcreate.canvas.pathSetSegments(
        nodeId,
        segments,
        closed,
      );
      await onAfterCommitRef.current();
      return true;
    } catch (err) {
      onErrorRef.current(
        `node edit commit failed: ${errorMessage(err)}`,
      );
      return false;
    }
  }, [notify]);

  /**
   * Phase B3 — discard the in-flight `nodeEdit` gesture without
   * committing. Sync (no bridge call) — the document is
   * unchanged. Returns `true` if the cancellation actually
   * consumed a `nodeEdit` gesture.
   */
  const cancelNodeEdit = useCallback((): boolean => {
    const state = stateRef.current;
    if (state.kind !== "nodeEdit") return false;
    stateRef.current = IDLE;
    notify();
    return true;
  }, [notify]);

  const onCanvasPointer = useCallback(
    (e: React.PointerEvent<HTMLCanvasElement>): void => {
      // `pointerdown` is the only event we filter by mouse button —
      // for `pointermove` / `pointerup` we trust the captured
      // `pointerId` from the active state to gate dispatch.
      if (e.button !== 0 && e.type === "pointerdown") return;

      // React nullifies `SyntheticEvent.currentTarget` once the
      // synchronous handler returns, so the async IIFE inside the
      // `select`-tool hit-test branch below cannot read it after
      // an `await`. Capture the canvas element and pointer id
      // synchronously here so `setPointerCapture` /
      // `releasePointerCapture` keep working across awaits.
      const canvasEl = e.currentTarget;
      const pointerId = e.pointerId;
      const rect = canvasEl.getBoundingClientRect();
      const sx = e.clientX - rect.left;
      const sy = e.clientY - rect.top;
      const { x: wx, y: wy } = screenToWorld(sx, sy);
      // Phase 6 Tasks 25-26: latest world-space cursor sample drives
      // paste-at-cursor (read inside `EditorPage.handlePaste`).
      lastCursorWorldRef.current = { x: wx, y: wy };
      // Capture the viewport snapshot at pointer-down time. The Rust
      // hit-test wants screen coordinates plus the viewport so it
      // can run the screen→world transform once — if we
      // pre-transformed here too, the renderer would double-apply
      // pan + zoom and miss every click.
      const vp = viewport;

      if (e.type === "pointerdown") {
        // Hold-to-pan beats every tool. We commit the viewport
        // delta on every pointermove (no batching) because pan is a
        // transient visual effect; there's no op-log entry, so we
        // don't worry about coarsening like we do for node moves.
        if (panActiveRef.current) {
          canvasEl.setPointerCapture(pointerId);
          // Stash an in-flight pen gesture so the user doesn't
          // lose every committed anchor when they hold Space to
          // pan mid-path. We only save when the user has actually
          // committed at least one anchor — a "fresh pen + Space"
          // shouldn't strand an empty pen state in the ref.
          // Discarding any `pending` anchor is correct: if the
          // user's currently mid-click, the pan steals the gesture
          // and the half-pressed click never reaches pointerup —
          // restoring `pending` would leave a permanent
          // pending-anchor ghost in the overlay.
          const existing = stateRef.current;
          if (
            existing.kind === "pen" &&
            existing.anchors.length > 0
          ) {
            savedPenStateRef.current = {
              ...existing,
              pending: null,
            };
          } else {
            savedPenStateRef.current = null;
          }
          stateRef.current = {
            kind: "pan",
            pointerId,
            lastScreenX: sx,
            lastScreenY: sy,
          };
          notify();
          return;
        }

        const activeTool = toolRef.current;

        // Phase B3 — Node editor pointerdown. Only intercepts
        // when we're already in the `nodeEdit` variant (the
        // editor enters it via `enterNodeEdit` from a
        // double-click on a VectorLayer, not from a tool
        // selection). When in `nodeEdit`, EVERY click in the
        // canvas routes through this branch regardless of
        // `activeTool`, because the user is conceptually inside
        // a modal sub-editor of the path. Tool-bar tool changes
        // STILL work — they just don't take effect until the
        // user commits/cancels and the state returns to `idle`.
        {
          const existing = stateRef.current;
          if (existing.kind === "nodeEdit") {
            const hitRadiusWorld =
              NODE_ANCHOR_HIT_RADIUS_SCREEN / vp.zoom;
            const handleRadiusWorld =
              NODE_HANDLE_HIT_RADIUS_SCREEN / vp.zoom;
            // Handles are tested BEFORE anchors so when a handle
            // sits on top of its own anchor (a zero-bend cubic
            // viewed at high zoom-out) the user still grabs the
            // handle. Matches Illustrator's Direct Select
            // priority.
            let handleHit: {
              anchorIndex: number;
              side: "in" | "out";
            } | null = null;
            for (let i = 0; i < existing.anchors.length; i++) {
              const a = existing.anchors[i]!;
              if (a.inHandle !== null) {
                const dx = wx - a.inHandle.x;
                const dy = wy - a.inHandle.y;
                if (Math.hypot(dx, dy) <= handleRadiusWorld) {
                  handleHit = { anchorIndex: i, side: "in" };
                  break;
                }
              }
              if (a.outHandle !== null) {
                const dx = wx - a.outHandle.x;
                const dy = wy - a.outHandle.y;
                if (Math.hypot(dx, dy) <= handleRadiusWorld) {
                  handleHit = { anchorIndex: i, side: "out" };
                  break;
                }
              }
            }
            if (handleHit !== null) {
              canvasEl.setPointerCapture(pointerId);
              stateRef.current = {
                ...existing,
                cursor: { x: wx, y: wy },
                drag: {
                  kind: "handle",
                  pointerId,
                  anchorIndex: handleHit.anchorIndex,
                  side: handleHit.side,
                  cursorWorldX: wx,
                  cursorWorldY: wy,
                },
                dragMoved: false,
              };
              notify();
              return;
            }
            let anchorHit = -1;
            for (let i = 0; i < existing.anchors.length; i++) {
              const a = existing.anchors[i]!;
              const dx = wx - a.x;
              const dy = wy - a.y;
              if (Math.hypot(dx, dy) <= hitRadiusWorld) {
                anchorHit = i;
                break;
              }
            }
            if (anchorHit >= 0) {
              canvasEl.setPointerCapture(pointerId);
              // Shift-click toggles set membership; plain click
              // replaces. Mirrors `EditorContext`'s top-level
              // node selection semantics so the muscle memory
              // transfers.
              const nextSelection = new Set<number>(
                e.shiftKey ? existing.selectedAnchorIndices : [],
              );
              if (e.shiftKey && nextSelection.has(anchorHit)) {
                nextSelection.delete(anchorHit);
              } else {
                nextSelection.add(anchorHit);
              }
              stateRef.current = {
                ...existing,
                cursor: { x: wx, y: wy },
                selectedAnchorIndices: nextSelection,
                drag: {
                  kind: "anchor",
                  pointerId,
                  anchorIndex: anchorHit,
                  cumulativeDx: 0,
                  cumulativeDy: 0,
                  lastWorldX: wx,
                  lastWorldY: wy,
                },
                dragMoved: false,
              };
              notify();
              return;
            }
            // Click in empty space inside `nodeEdit` clears
            // the anchor selection — same affordance as the
            // top-level select tool's empty-canvas click.
            stateRef.current = {
              ...existing,
              cursor: { x: wx, y: wy },
              selectedAnchorIndices: new Set<number>(),
              drag: null,
              dragMoved: false,
            };
            notify();
            return;
          }
        }

        if (activeTool === "select") {
          // Click-to-select: hit-test, then either start a move drag
          // or clear the selection. The bridge does the screen→world
          // transform internally; we send raw screen coordinates
          // plus the current viewport (single source of truth, no
          // double-transform).
          void (async () => {
            try {
              const hit = await window.kcreate.canvas.hitTest(
                sx,
                sy,
                vp.panX,
                vp.panY,
                vp.zoom,
              );
              if (hit) {
                await window.kcreate.canvas.setSelection([hit]);
                setSelectedIds([hit]);
                canvasEl.setPointerCapture(pointerId);
                stateRef.current = {
                  kind: "move",
                  pointerId,
                  tool: activeTool,
                  movingNodeId: hit,
                  lastWorldX: wx,
                  lastWorldY: wy,
                  cumulativeDx: 0,
                  cumulativeDy: 0,
                };
              } else {
                await window.kcreate.canvas.clearSelection();
                setSelectedIds([]);
              }
            } catch (err) {
              onErrorRef.current(`hit-test failed: ${errorMessage(err)}`);
            }
          })();
          return;
        }

        if (activeTool === "pen") {
          // Pen tool is the only multi-event gesture: each
          // `pointerdown` either (a) closes the path if the click
          // lands inside `PEN_CLOSE_HIT_RADIUS_SCREEN` of the
          // first anchor (with ≥ 2 anchors already laid down), or
          // (b) starts a new in-flight anchor. Pointer capture
          // makes sure `pointermove` / `pointerup` reach us even
          // if the cursor briefly leaves the canvas bounds (e.g.
          // tracking a fast drag along the edge).
          canvasEl.setPointerCapture(pointerId);
          const existing = stateRef.current;
          // Convert close-hit radius to world units at the active
          // zoom so the threshold stays a constant ~8 screen-px
          // regardless of zoom level. At zoom < 1 (zoomed out) the
          // world-space radius grows; at zoom > 1 (zoomed in) it
          // shrinks — same gesture, same precision.
          const closeRadiusWorld = PEN_CLOSE_HIT_RADIUS_SCREEN / vp.zoom;
          if (existing.kind === "pen" && existing.anchors.length >= 2) {
            const first = existing.anchors[0]!;
            const dx = wx - first.x;
            const dy = wy - first.y;
            if (Math.hypot(dx, dy) <= closeRadiusWorld) {
              // Click-on-first-anchor: commit as a closed path.
              // `commitPenGesture(true)` clears the state and
              // releases pointer capture as part of its cleanup,
              // so we can return immediately afterward.
              try {
                canvasEl.releasePointerCapture(pointerId);
              } catch {
                // capture may have been released already; the
                // commit below doesn't depend on it.
              }
              void commitPenGesture(true);
              return;
            }
          }
          // Start (or continue) a pen gesture by laying a new
          // pending anchor. If state was idle (gesture just
          // started), bootstrap a fresh `pen` state with no
          // committed anchors yet. If state is already `pen`
          // (additional click), keep the existing anchor list.
          const baseAnchors =
            existing.kind === "pen" ? existing.anchors : [];
          stateRef.current = {
            kind: "pen",
            tool: activeTool,
            anchors: baseAnchors,
            pending: {
              pointerId,
              x: wx,
              y: wy,
              drag: null,
            },
            cursor: { x: wx, y: wy },
          };
          notify();
          return;
        }

        // Drawing tools — record drag start in world coords; commit
        // on pointerup.
        canvasEl.setPointerCapture(pointerId);
        stateRef.current = {
          kind: "create",
          pointerId,
          tool: activeTool,
          startWorldX: wx,
          startWorldY: wy,
        };
        return;
      }

      if (e.type === "pointermove") {
        const drag = stateRef.current;
        if (drag.kind === "idle") return;

        if (drag.kind === "pen") {
          // Pen state has no top-level `pointerId` because the
          // gesture spans many pointers (each click can in
          // principle be from a different pointer device — e.g.
          // touch + mouse on a hybrid laptop). We always update
          // `cursor` for the rubber-band preview; we update
          // `pending.drag` only when the in-flight anchor is
          // owned by THIS pointer.
          //
          // CRITICAL: assign a NEW state object rather than
          // mutating `stateRef.current` in place. The pen tool's
          // overlay is wired through `useSyncExternalStore`, which
          // compares snapshots with `Object.is`. A same-reference
          // return short-circuits the subscriber re-render even
          // though `notify()` fires the listeners, so in-place
          // mutation here would make the cursor preview / drag
          // handles / rubber-band invisible until the next
          // `pointerdown`. `pan` and `move` states get away with
          // in-place mutation because they don't expose their
          // intermediate state through `useSyncExternalStore` —
          // they drive React state directly via `setViewport` /
          // bridge calls. This same lesson is documented at
          // `apps/desktop/renderer/src/shortcuts/registry.ts:236`
          // for `ShortcutStore.snapshot()`.
          let nextPending: PenPendingAnchor | null = drag.pending;
          if (drag.pending && drag.pending.pointerId === pointerId) {
            const pdx = sx - (drag.pending.x * vp.zoom + vp.panX);
            const pdy = sy - (drag.pending.y * vp.zoom + vp.panY);
            // Promote to a "drag in progress" only after the
            // cursor moves past the screen-space threshold —
            // below it the user is just clicking with mild hand
            // jitter and shouldn't get spurious smooth handles.
            if (
              drag.pending.drag !== null ||
              Math.hypot(pdx, pdy) >= PEN_DRAG_THRESHOLD_SCREEN
            ) {
              nextPending = { ...drag.pending, drag: { x: wx, y: wy } };
            }
          }
          stateRef.current = {
            ...drag,
            cursor: { x: wx, y: wy },
            pending: nextPending,
          };
          notify();
          return;
        }

        if (drag.kind === "nodeEdit") {
          // Phase B3 — node editor pointermove. Like `pen`, the
          // `nodeEdit` variant has no top-level `pointerId`
          // because the gesture spans many pointer presses (each
          // anchor / handle drag is its own captured-pointer
          // session). We always update `cursor` for the hover
          // indicator; we update the active drag only when the
          // event's `pointerId` matches the captured drag.
          //
          // Same useSyncExternalStore caveat as `pen`: new
          // object identity required so subscribers re-paint.
          if (drag.drag !== null && drag.drag.pointerId === pointerId) {
            if (drag.drag.kind === "anchor") {
              const dx = wx - drag.drag.lastWorldX;
              const dy = wy - drag.drag.lastWorldY;
              const cumulativeDx = drag.drag.cumulativeDx + dx;
              const cumulativeDy = drag.drag.cumulativeDy + dy;
              // Apply the delta to ALL selected anchors so a
              // multi-select drags as a group. Singleton drags
              // also fall through this path — `selectedAnchorIndices`
              // is guaranteed to include `anchorIndex` because
              // the pointerdown branch always added it.
              const nextAnchors = drag.anchors.map((a, i) => {
                if (!drag.selectedAnchorIndices.has(i)) return a;
                return {
                  x: a.x + dx,
                  y: a.y + dy,
                  inHandle:
                    a.inHandle === null
                      ? null
                      : {
                          x: a.inHandle.x + dx,
                          y: a.inHandle.y + dy,
                        },
                  outHandle:
                    a.outHandle === null
                      ? null
                      : {
                          x: a.outHandle.x + dx,
                          y: a.outHandle.y + dy,
                        },
                };
              });
              stateRef.current = {
                ...drag,
                anchors: nextAnchors,
                cursor: { x: wx, y: wy },
                drag: {
                  kind: "anchor",
                  pointerId: drag.drag.pointerId,
                  anchorIndex: drag.drag.anchorIndex,
                  cumulativeDx,
                  cumulativeDy,
                  lastWorldX: wx,
                  lastWorldY: wy,
                },
                dragMoved:
                  drag.dragMoved ||
                  Math.hypot(cumulativeDx, cumulativeDy) > 0,
              };
              notify();
              return;
            }
            // handle drag — set the handle's coords to the cursor
            // directly (no cumulative delta — handle follows
            // cursor 1:1).
            const idx = drag.drag.anchorIndex;
            const side = drag.drag.side;
            const nextAnchors = drag.anchors.map((a, i) => {
              if (i !== idx) return a;
              return {
                ...a,
                inHandle:
                  side === "in" ? { x: wx, y: wy } : a.inHandle,
                outHandle:
                  side === "out" ? { x: wx, y: wy } : a.outHandle,
              };
            });
            stateRef.current = {
              ...drag,
              anchors: nextAnchors,
              cursor: { x: wx, y: wy },
              drag: {
                kind: "handle",
                pointerId: drag.drag.pointerId,
                anchorIndex: idx,
                side,
                cursorWorldX: wx,
                cursorWorldY: wy,
              },
              dragMoved: true,
            };
            notify();
            return;
          }
          // Pointer moved with no active drag — just update
          // cursor for the hover indicator.
          stateRef.current = {
            ...drag,
            cursor: { x: wx, y: wy },
          };
          notify();
          return;
        }

        if (drag.pointerId !== pointerId) return;

        if (drag.kind === "pan") {
          // Translate the viewport by the screen-space delta since
          // the last sample. We work in screen pixels (not world
          // units) on purpose: panning *is* the screen→world
          // translation we'd otherwise compute, so re-deriving it
          // would just be `delta_screen / zoom * zoom = delta_screen`.
          const dx = sx - drag.lastScreenX;
          const dy = sy - drag.lastScreenY;
          drag.lastScreenX = sx;
          drag.lastScreenY = sy;
          setViewport((v) => ({
            ...v,
            panX: v.panX + dx,
            panY: v.panY + dy,
          }));
          return;
        }

        if (drag.kind === "move") {
          const dx = wx - drag.lastWorldX;
          const dy = wy - drag.lastWorldY;
          drag.lastWorldX = wx;
          drag.lastWorldY = wy;
          drag.cumulativeDx += dx;
          drag.cumulativeDy += dy;
          // Smart-guides: query the snap engine for the candidate
          // world-space bounds and apply the returned delta to the
          // *cumulative* offset (so the next pointermove keeps
          // working off the snapped position). The bridge call is
          // cheap (O(log n) per axis after the sorted-edge build);
          // we still fire it on every move because the engine is
          // built from-scratch each time — the dragged node's
          // bounds are dirty otherwise.
          const movingNode = nodesRef.current.find(
            (n) => n.id === drag.movingNodeId,
          );
          if (movingNode) {
            const candX = movingNode.bounds.x + drag.cumulativeDx;
            const candY = movingNode.bounds.y + drag.cumulativeDy;
            void (async () => {
              try {
                const snap = await window.kcreate.canvasSnap.query(
                  movingNode.id,
                  candX,
                  candY,
                  movingNode.bounds.width,
                  movingNode.bounds.height,
                  SNAP_THRESHOLD_WORLD,
                );
                if (!snap) return;
                // Guard against the case where the user released the
                // pointer (or the drag transitioned) while the snap
                // query was in flight. Reading `stateRef.current` by
                // identity ensures we don't fold a stale snap delta
                // into the next drag.
                if (stateRef.current !== drag) return;
                if (snap.dx !== 0 || snap.dy !== 0) {
                  drag.cumulativeDx += snap.dx;
                  drag.cumulativeDy += snap.dy;
                }
                setSnapGuides(snap.guides);
              } catch {
                // Snap is purely advisory — failures shouldn't abort
                // the drag. Silently swallow.
              }
            })();
          }
          // Don't fire a bridge call for every micro-pixel of cursor
          // motion — only push the accumulated delta on pointerup.
          // This keeps undo entries coarse (one drag = one op) and
          // avoids op-log spam.
          return;
        }

        // `create`: no commit until pointerup. The canvas does not
        // yet show an in-flight ghost; Phase 1 will add a transient
        // overlay by passing the in-progress rect/ellipse to the
        // renderer alongside the persisted scene.
        return;
      }

      if (e.type === "pointerup") {
        const drag = stateRef.current;
        if (drag.kind === "idle") return;

        if (drag.kind === "pen") {
          // Pen state spans gestures, so we don't reset to idle on
          // pointerup. We only promote the in-flight pending
          // anchor (if any) into the committed `anchors` list.
          // Releasing pointer capture matches the symmetry with
          // `pointerdown`; without it, a subsequent pointermove
          // outside the canvas wouldn't fire and the cursor
          // preview would freeze.
          try {
            canvasEl.releasePointerCapture(pointerId);
          } catch {
            // capture might already be released by the OS — same
            // as the non-pen path below.
          }
          if (!drag.pending || drag.pending.pointerId !== pointerId) {
            // Stale pointerup (different pointer, or pending was
            // already promoted). Nothing to do.
            return;
          }
          const pending = drag.pending;
          let newAnchor: PenAnchor;
          if (pending.drag) {
            // Smooth anchor: outHandle is where the user dragged,
            // inHandle is the symmetric reflection through the
            // anchor (so the curve passes through the anchor with
            // continuous first derivative — the same convention
            // Illustrator / Figma use for "smooth" pen anchors).
            const out = pending.drag;
            newAnchor = {
              x: pending.x,
              y: pending.y,
              inHandle: {
                x: 2 * pending.x - out.x,
                y: 2 * pending.y - out.y,
              },
              outHandle: { x: out.x, y: out.y },
            };
          } else {
            // Corner anchor: no smoothing on either side.
            newAnchor = {
              x: pending.x,
              y: pending.y,
              inHandle: null,
              outHandle: null,
            };
          }
          // Same `useSyncExternalStore` immutability contract as
          // the pointermove branch above — must assign a NEW state
          // object so `Object.is` sees a different snapshot and
          // re-paints the overlay with the newly committed anchor.
          stateRef.current = {
            ...drag,
            anchors: [...drag.anchors, newAnchor],
            pending: null,
            cursor: { x: wx, y: wy },
          };
          notify();
          return;
        }

        if (drag.kind === "nodeEdit") {
          // Phase B3 — node editor pointerup. Like `pen`, the
          // gesture spans pointer-press cycles so we don't
          // collapse to `idle`. We just clear the active drag.
          // The mutation to the anchor positions has already
          // landed via pointermove; the actual bridge commit is
          // deferred until the user presses Enter or switches
          // tools (see `commitNodeEdit`). This matches Figma /
          // Illustrator: every anchor/handle drag updates the
          // overlay live but the document only takes a single
          // operation at end-of-edit-session.
          if (drag.drag === null || drag.drag.pointerId !== pointerId) {
            return;
          }
          try {
            canvasEl.releasePointerCapture(pointerId);
          } catch {
            // capture might already be released — same as
            // the non-nodeEdit path below.
          }
          stateRef.current = {
            ...drag,
            drag: null,
            cursor: { x: wx, y: wy },
          };
          notify();
          return;
        }

        if (drag.pointerId !== pointerId) return;
        try {
          canvasEl.releasePointerCapture(pointerId);
        } catch {
          // capture might already be released
        }
        // Snap-clear-on-release: the drag is done, so any displayed
        // guide lines belong to a stale candidate position.
        setSnapGuides([]);
        // Reset state BEFORE firing the bridge commit so the in-flight
        // snap-query guard above sees the transition immediately.
        // For pan release, prefer restoring a saved pen gesture (set
        // on pan-enter when a pen gesture was in-flight) over the IDLE
        // default — see the `savedPenStateRef` comment above. The ref
        // is cleared after restoration so a subsequent unrelated pan
        // can't accidentally rehydrate stale state.
        if (drag.kind === "pan" && savedPenStateRef.current) {
          stateRef.current = savedPenStateRef.current;
          savedPenStateRef.current = null;
          notify();
        } else {
          stateRef.current = IDLE;
          // Notify only on pan exit so a pen-overlay subscriber
          // observing pen-vs-pan transitions sees the gesture
          // ended. `move`/`create` already drive their own React
          // state via setSelectedIds/onAfterCommit downstream, so
          // an extra notify would just trigger a redundant
          // subscriber callback. The "pen" branch was eliminated
          // by the pen-specific early-return block above.
          if (drag.kind === "pan") {
            notify();
          }
        }

        if (drag.kind === "pan") {
          // Pan drags don't write to the op log or touch the
          // document — there's nothing to commit on release. The
          // viewport has already been mutated incrementally on each
          // pointermove sample.
          return;
        }

        if (drag.kind === "move") {
          if (drag.cumulativeDx !== 0 || drag.cumulativeDy !== 0) {
            void (async () => {
              try {
                await window.kcreate.canvas.moveNode(
                  drag.movingNodeId,
                  drag.cumulativeDx,
                  drag.cumulativeDy,
                );
                await onAfterCommitRef.current();
              } catch (err) {
                onErrorRef.current(`move failed: ${errorMessage(err)}`);
              }
            })();
          }
          return;
        }

        // `create`: convert the drag to the actual shape parameters.
        const x0 = drag.startWorldX;
        const y0 = drag.startWorldY;
        const x1 = wx;
        const y1 = wy;
        const minX = Math.min(x0, x1);
        const minY = Math.min(y0, y1);
        const w = Math.abs(x1 - x0);
        const h = Math.abs(y1 - y0);
        // Reject zero-area drags — that's a stray click, not a
        // drawing. Text is exempt because clicking creates a text
        // layer at the cursor.
        if (w < 1 && h < 1 && drag.tool !== "text") return;

        void (async () => {
          try {
            let newId: string | null = null;
            if (drag.tool === "rect") {
              newId = await window.kcreate.canvas.createRect(
                null,
                minX,
                minY,
                w,
                h,
              );
            } else if (drag.tool === "ellipse") {
              newId = await window.kcreate.canvas.createEllipse(
                null,
                minX + w / 2,
                minY + h / 2,
                w / 2,
                h / 2,
              );
            } else if (drag.tool === "line") {
              newId = await window.kcreate.canvas.createLine(
                null,
                x0,
                y0,
                x1,
                y1,
              );
            } else if (drag.tool === "text") {
              newId = await window.kcreate.canvas.createText(
                null,
                x0,
                y0,
                "Text",
                "sans-serif",
                24,
              );
            }
            if (newId) {
              await window.kcreate.canvas.setSelection([newId]);
            }
            await onAfterCommitRef.current();
          } catch (err) {
            onErrorRef.current(`create failed: ${errorMessage(err)}`);
          }
        })();
      }
    },
    [
      screenToWorld,
      viewport,
      panActiveRef,
      nodesRef,
      lastCursorWorldRef,
      setSelectedIds,
      setViewport,
      setSnapGuides,
      notify,
      commitPenGesture,
    ],
  );

  // Tool-switch effect: if the user switches AWAY from the pen
  // tool while a pen gesture is in flight (≥ 1 anchor laid down),
  // auto-commit the gesture instead of silently dropping the
  // anchors. Matches Illustrator / Figma semantics: "I'm done
  // drawing this path; pick a different tool". A < 2 anchor
  // gesture is dropped (no path to commit) — `commitPenGesture`
  // handles the "nothing to commit" case as a clean reset.
  useEffect(() => {
    if (tool === "pen") return;
    const state = stateRef.current;
    // If we're currently mid-pan with a saved pen shadow, promote
    // the shadow back to the active state first so
    // `commitPenGesture` (which only inspects `stateRef.current`)
    // sees the anchors and commits them. Without this, switching
    // tools while still holding Space would silently drop the
    // entire path.
    if (state.kind !== "pen" && savedPenStateRef.current) {
      stateRef.current = savedPenStateRef.current;
      savedPenStateRef.current = null;
    }
    if (stateRef.current.kind !== "pen") return;
    void commitPenGesture(false);
  }, [tool, commitPenGesture]);

  const getState = useCallback((): ToolMachineState => stateRef.current, []);
  const getLastCursorWorld = useCallback(
    () => lastCursorWorldRef.current,
    [lastCursorWorldRef],
  );
  const commitPen = useCallback(
    () => commitPenGesture(false),
    [commitPenGesture],
  );
  const cancelPen = useCallback(() => cancelPenGesture(), [cancelPenGesture]);

  return {
    onCanvasPointer,
    getState,
    getLastCursorWorld,
    commitPen,
    cancelPen,
    enterNodeEdit,
    commitNodeEdit,
    cancelNodeEdit,
    subscribe,
  };
}
