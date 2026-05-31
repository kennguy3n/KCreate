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
 */
export interface ToolStateMachine {
  onCanvasPointer: (e: React.PointerEvent<HTMLCanvasElement>) => void;
  getState: () => ToolMachineState;
  getLastCursorWorld: () => { x: number; y: number } | null;
  commitPen: () => Promise<string | null>;
  cancelPen: () => boolean;
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
        // switch tools" action.
        stateRef.current = IDLE;
        notify();
        return null;
      }
      const segments = anchorsToSegments(state.anchors, closed);
      // Reset to idle BEFORE the async bridge call so subsequent
      // events (tool switch, Escape) cannot mutate the in-flight
      // gesture. The bridge call is fire-and-forget from the
      // state machine's perspective.
      stateRef.current = IDLE;
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
    if (state.kind !== "pen") return false;
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
          stateRef.current = {
            kind: "pan",
            pointerId,
            lastScreenX: sx,
            lastScreenY: sy,
          };
          return;
        }

        const activeTool = toolRef.current;

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
          drag.cursor = { x: wx, y: wy };
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
              drag.pending.drag = { x: wx, y: wy };
            }
          }
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
          drag.anchors = [...drag.anchors, newAnchor];
          drag.pending = null;
          drag.cursor = { x: wx, y: wy };
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
        stateRef.current = IDLE;

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
    if (state.kind !== "pen") return;
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
    subscribe,
  };
}
