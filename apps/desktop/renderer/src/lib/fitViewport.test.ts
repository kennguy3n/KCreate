// Unit tests for `computeFitViewport` — the pure geometry backing the
// editor's "fit to content" behaviour (zoom-to-fit + one-shot framing
// on project open). These pin the invariants the EditorPage relies on:
// the union box is centered in the canvas, the chosen zoom keeps the
// content inside the margin on the constraining axis, zero-area boxes
// are ignored, and an empty set returns `null` so the caller can fall
// back to an identity viewport instead of dividing by an empty extent.

import { describe, it, expect } from "vitest";

import type { Bounds } from "../../../shared/scene";
import { computeFitViewport } from "./fitViewport";

const CANVAS_W = 1024;
const CANVAS_H = 640;

/** Apply `screen = world * zoom + pan` to a world point. */
function project(
  vp: { panX: number; panY: number; zoom: number },
  x: number,
  y: number,
): { x: number; y: number } {
  return { x: x * vp.zoom + vp.panX, y: y * vp.zoom + vp.panY };
}

describe("computeFitViewport", () => {
  it("returns null when there are no boxes", () => {
    expect(computeFitViewport([], CANVAS_W, CANVAS_H)).toBeNull();
  });

  it("returns null when every box has non-positive area", () => {
    const boxes: Bounds[] = [
      { x: 10, y: 10, width: 0, height: 50 },
      { x: 0, y: 0, width: 100, height: 0 },
      { x: 5, y: 5, width: -20, height: -20 },
    ];
    expect(computeFitViewport(boxes, CANVAS_W, CANVAS_H)).toBeNull();
  });

  it("centers a single box in the canvas", () => {
    const box: Bounds = { x: 0, y: 0, width: 200, height: 100 };
    const vp = computeFitViewport([box], CANVAS_W, CANVAS_H);
    expect(vp).not.toBeNull();
    const center = project(vp!, box.x + box.width / 2, box.y + box.height / 2);
    expect(center.x).toBeCloseTo(CANVAS_W / 2, 6);
    expect(center.y).toBeCloseTo(CANVAS_H / 2, 6);
  });

  it("frames a box offset far from the origin (regression: content off-screen at open)", () => {
    // Mirrors the scratch project's "Desktop" artboard at world (2020,0).
    const box: Bounds = { x: 2020, y: 0, width: 1440, height: 900 };
    const vp = computeFitViewport([box], CANVAS_W, CANVAS_H);
    expect(vp).not.toBeNull();
    const center = project(vp!, box.x + box.width / 2, box.y + box.height / 2);
    expect(center.x).toBeCloseTo(CANVAS_W / 2, 6);
    expect(center.y).toBeCloseTo(CANVAS_H / 2, 6);
    // The framed content must land inside the canvas on both axes.
    const topLeft = project(vp!, box.x, box.y);
    const bottomRight = project(vp!, box.x + box.width, box.y + box.height);
    expect(topLeft.x).toBeGreaterThanOrEqual(0);
    expect(topLeft.y).toBeGreaterThanOrEqual(0);
    expect(bottomRight.x).toBeLessThanOrEqual(CANVAS_W);
    expect(bottomRight.y).toBeLessThanOrEqual(CANVAS_H);
  });

  it("computes the union across multiple boxes and ignores zero-area ones", () => {
    const boxes: Bounds[] = [
      { x: 0, y: 0, width: 1920, height: 1080 },
      { x: 2020, y: 0, width: 1440, height: 900 },
      { x: 500, y: 500, width: 0, height: 0 }, // ignored
    ];
    const vp = computeFitViewport(boxes, CANVAS_W, CANVAS_H);
    expect(vp).not.toBeNull();
    // Union spans x:[0,3460], y:[0,1080] → center (1730, 540).
    const center = project(vp!, 1730, 540);
    expect(center.x).toBeCloseTo(CANVAS_W / 2, 6);
    expect(center.y).toBeCloseTo(CANVAS_H / 2, 6);
  });

  it("honors the margin factor on the constraining axis", () => {
    // A wide box is width-constrained: its projected width must equal
    // canvasWidth * marginFactor.
    const box: Bounds = { x: 0, y: 0, width: 2000, height: 100 };
    const margin = 0.9;
    const vp = computeFitViewport([box], CANVAS_W, CANVAS_H, margin)!;
    const left = project(vp, box.x, box.y).x;
    const right = project(vp, box.x + box.width, box.y).x;
    expect(right - left).toBeCloseTo(CANVAS_W * margin, 6);
  });
});
