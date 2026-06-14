// Unit tests for `computeFitViewport` — the pure geometry backing the
// editor's "fit to content" behaviour (zoom-to-fit + one-shot framing
// on project open). These pin the invariants the EditorPage relies on:
// the union box is centered in the canvas, the chosen zoom keeps the
// content inside the margin on the constraining axis, zero-area boxes
// are ignored, and an empty set returns `null` so the caller can fall
// back to an identity viewport instead of dividing by an empty extent.

import { describe, it, expect } from "vitest";

import type { Bounds } from "../../../shared/scene";
import type { FitNode } from "./fitViewport";
import { computeContentFit, computeFitViewport } from "./fitViewport";

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

describe("computeContentFit", () => {
  const node = (bounds: Bounds, visible = true): FitNode => ({
    visible,
    bounds,
  });

  it("frames the artboards when present, ignoring nodes", () => {
    const artboards: Bounds[] = [{ x: 0, y: 0, width: 1920, height: 1080 }];
    // A loose node far away must NOT widen the framed union when
    // artboards exist (artboards are the document's top-level frames).
    const nodes = [node({ x: 9000, y: 9000, width: 100, height: 100 })];
    const vp = computeContentFit(artboards, nodes, CANVAS_W, CANVAS_H);
    expect(vp).not.toBeNull();
    const center = project(vp!, 960, 540);
    expect(center.x).toBeCloseTo(CANVAS_W / 2, 6);
    expect(center.y).toBeCloseTo(CANVAS_H / 2, 6);
  });

  it("frames the union of visible node bounds when there are no artboards (regression: artboard-less docs)", () => {
    // The bug: the one-shot fit effect read a stale (empty) nodes ref, so
    // artboard-less docs fell back to DEFAULT_VIEWPORT and never framed
    // their content. With the current nodes passed in, the loose content
    // at a large world offset is framed and centered.
    const nodes = [
      node({ x: 2020, y: 300, width: 400, height: 200 }),
      node({ x: 2620, y: 300, width: 200, height: 200 }),
    ];
    const vp = computeContentFit([], nodes, CANVAS_W, CANVAS_H);
    expect(vp).not.toBeNull();
    // Union spans x:[2020,2820], y:[300,500] → center (2420, 400).
    const center = project(vp!, 2420, 400);
    expect(center.x).toBeCloseTo(CANVAS_W / 2, 6);
    expect(center.y).toBeCloseTo(CANVAS_H / 2, 6);
  });

  it("skips hidden nodes when falling back to node bounds", () => {
    const nodes = [
      node({ x: 0, y: 0, width: 100, height: 100 }, false), // hidden → ignored
      node({ x: 1000, y: 1000, width: 200, height: 200 }),
    ];
    const vp = computeContentFit([], nodes, CANVAS_W, CANVAS_H);
    expect(vp).not.toBeNull();
    // Only the visible node frames → center (1100, 1100).
    const center = project(vp!, 1100, 1100);
    expect(center.x).toBeCloseTo(CANVAS_W / 2, 6);
    expect(center.y).toBeCloseTo(CANVAS_H / 2, 6);
  });

  it("returns null when there is nothing with positive area to frame", () => {
    expect(computeContentFit([], [], CANVAS_W, CANVAS_H)).toBeNull();
    // Only hidden / zero-area content → still null.
    const nodes = [
      node({ x: 0, y: 0, width: 100, height: 100 }, false),
      node({ x: 10, y: 10, width: 0, height: 0 }),
    ];
    expect(computeContentFit([], nodes, CANVAS_W, CANVAS_H)).toBeNull();
  });
});
