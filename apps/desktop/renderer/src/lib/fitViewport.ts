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
