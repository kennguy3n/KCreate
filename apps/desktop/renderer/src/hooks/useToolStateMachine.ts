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
import { useCallback, useRef } from "react";

import type { ViewportState } from "../components/CanvasHost";
import type { ToolId } from "../contexts/EditorContext";
import type { NodeInfo, SnapGuide } from "../../../shared/scene";
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
 */
export interface ToolStateMachine {
  onCanvasPointer: (e: React.PointerEvent<HTMLCanvasElement>) => void;
  getState: () => ToolMachineState;
  getLastCursorWorld: () => { x: number; y: number } | null;
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
        if (drag.kind === "idle" || drag.pointerId !== pointerId) return;

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
        if (drag.kind === "idle" || drag.pointerId !== pointerId) return;
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
    ],
  );

  const getState = useCallback((): ToolMachineState => stateRef.current, []);
  const getLastCursorWorld = useCallback(
    () => lastCursorWorldRef.current,
    [lastCursorWorldRef],
  );

  return { onCanvasPointer, getState, getLastCursorWorld };
}
