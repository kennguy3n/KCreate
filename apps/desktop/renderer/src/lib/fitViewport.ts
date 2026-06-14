import type { Bounds } from "../../../shared/scene";

/**
 * Viewport transform in the canvas' `screen = world * zoom + pan`
 * convention. Mirrors `ViewportState` from `EditorContext` (kept
 * structural here so this pure helper has no dependency on the React
 * context layer).
 */
export interface FitViewport {
  panX: number;
  panY: number;
  zoom: number;
}

/**
 * Pure geometry backing the editor's "fit to content" behaviour (both
 * the user-facing zoom-to-fit and the one-shot framing on project
 * open).
 *
 * Given a set of world-space boxes and the canvas dimensions, returns
 * the viewport that centers the union of those boxes with
 * `marginFactor` padding (0.9 ⇒ the content fills 90% of the shorter
 * axis). Boxes with non-positive area are ignored so zero-size groups
 * (e.g. containers before layout solving) never poison the bounds.
 *
 * Returns `null` when there is nothing with positive area to frame,
 * letting the caller fall back to a default identity viewport instead
 * of dividing by an empty extent.
 */
export function computeFitViewport(
  boxes: readonly Bounds[],
  canvasWidth: number,
  canvasHeight: number,
  marginFactor = 0.9,
): FitViewport | null {
  let minX = Infinity;
  let minY = Infinity;
  let maxX = -Infinity;
  let maxY = -Infinity;
  let framedAny = false;
  for (const b of boxes) {
    if (b.width <= 0 || b.height <= 0) continue;
    framedAny = true;
    minX = Math.min(minX, b.x);
    minY = Math.min(minY, b.y);
    maxX = Math.max(maxX, b.x + b.width);
    maxY = Math.max(maxY, b.y + b.height);
  }
  if (!framedAny) return null;

  const width = Math.max(maxX - minX, 1);
  const height = Math.max(maxY - minY, 1);
  const zoom = Math.min(
    (canvasWidth * marginFactor) / width,
    (canvasHeight * marginFactor) / height,
  );
  const centerWorldX = minX + width / 2;
  const centerWorldY = minY + height / 2;
  return {
    panX: canvasWidth / 2 - centerWorldX * zoom,
    panY: canvasHeight / 2 - centerWorldY * zoom,
    zoom,
  };
}

/**
 * Minimal shape of a node consumed by {@link computeContentFit}: just
 * its visibility flag and world-space bounds. Structurally satisfied by
 * the renderer's full `NodeInfo`, so callers pass `NodeInfo[]` directly.
 */
export interface FitNode {
  visible: boolean;
  bounds: Bounds;
}

/**
 * Box-selection policy backing both the user-facing zoom-to-fit and the
 * one-shot framing on project open: frame the artboards (the document's
 * top-level frames) when present, otherwise fall back to the union of
 * *visible* node bounds so artboard-less pages whose content lives in
 * loose nodes still get framed. Delegates the geometry to
 * {@link computeFitViewport}, so zero-area boxes are dropped and an
 * empty document yields `null`.
 *
 * Deliberately pure (no refs, no React state): callers must pass the
 * *current* artboards and nodes. The one-shot fit effect in `EditorPage`
 * runs as a child effect and therefore fires before the parent
 * `DocumentProvider` syncs its `nodesRef`, so it must feed this function
 * the freshly-rendered `nodes` state rather than a ref that is still
 * stale for the triggering render.
 */
export function computeContentFit(
  artboards: readonly Bounds[],
  nodes: readonly FitNode[],
  canvasWidth: number,
  canvasHeight: number,
  marginFactor = 0.9,
): FitViewport | null {
  const boxes: Bounds[] =
    artboards.length > 0
      ? artboards.map((a) => ({
          x: a.x,
          y: a.y,
          width: a.width,
          height: a.height,
        }))
      : nodes.filter((n) => n.visible).map((n) => n.bounds);
  return computeFitViewport(boxes, canvasWidth, canvasHeight, marginFactor);
}
