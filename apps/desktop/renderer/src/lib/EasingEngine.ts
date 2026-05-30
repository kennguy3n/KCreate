// EasingEngine — Phase 11 Block C Task 14.
//
// Pure-function easing utilities used by `PrototypePlayer.tsx` to
// drive prototype transitions (dissolve / slide / push / move-in
// + Smart Animate property interpolation). Mirrors the runtime
// contract of `kcreate_core::EasingCurve`.
//
// All public functions take a *normalised* time `t ∈ [0, 1]` and
// return a *normalised* progress value in roughly the same range
// (spring + over-shoot bezier curves may briefly exceed `[0, 1]`,
// which is the standard motion-design behaviour). Callers are
// responsible for clamping to `[0, 1]` when interpolating bounded
// quantities like opacity.

import type { EasingCurve } from "../../../shared/scene";

/** Linear (identity) interpolation. `linear(t) === t`. */
export function linear(t: number): number {
  return t;
}

/** Quadratic ease-in: slow start, fast finish. */
export function easeIn(t: number): number {
  return t * t;
}

/** Quadratic ease-out: fast start, slow finish. */
export function easeOut(t: number): number {
  return t * (2 - t);
}

/** Cubic ease-in-out: slow start, fast middle, slow finish. */
export function easeInOut(t: number): number {
  if (t < 0.5) return 2 * t * t;
  const inv = -2 * t + 2;
  return 1 - (inv * inv) / 2;
}

/**
 * CSS-style cubic-bezier sampler with control points
 * `(x1, y1)` / `(x2, y2)`. The curve always passes through
 * `(0, 0)` and `(1, 1)`; the control points pull the tangent in
 * each half. Matches the JS `cubic-bezier()` browser
 * implementation closely enough for animation use (Newton +
 * bisection root-find on `x`, no precomputed lookup table — we
 * only sample at ~60Hz so this is fast enough).
 */
export function cubicBezier(
  t: number,
  x1: number,
  y1: number,
  x2: number,
  y2: number,
): number {
  if (t <= 0) return 0;
  if (t >= 1) return 1;
  // Bezier curve as B(s) where s in [0,1]: solve B_x(s) = t for s,
  // then return B_y(s). Coefficients of the cubic polynomial in s.
  const cx = 3 * x1;
  const bx = 3 * (x2 - x1) - cx;
  const ax = 1 - cx - bx;
  const cy = 3 * y1;
  const by = 3 * (y2 - y1) - cy;
  const ay = 1 - cy - by;
  const bezX = (s: number): number => ((ax * s + bx) * s + cx) * s;
  const bezY = (s: number): number => ((ay * s + by) * s + cy) * s;
  const bezDx = (s: number): number => (3 * ax * s + 2 * bx) * s + cx;

  // Newton's method (≤ 4 iterations is enough for animation
  // precision; bail to bisection if the derivative is degenerate).
  let s = t;
  for (let i = 0; i < 4; i += 1) {
    const xs = bezX(s) - t;
    if (Math.abs(xs) < 1e-5) return bezY(s);
    const d = bezDx(s);
    if (Math.abs(d) < 1e-6) break;
    s = s - xs / d;
  }
  // Bisection fallback for ill-conditioned curves (e.g. control
  // points that flatten the derivative near 0 or 1).
  let lo = 0;
  let hi = 1;
  s = t;
  for (let i = 0; i < 32; i += 1) {
    const xs = bezX(s);
    if (Math.abs(xs - t) < 1e-5) return bezY(s);
    if (xs < t) lo = s;
    else hi = s;
    s = (lo + hi) / 2;
  }
  return bezY(s);
}

/**
 * Damped harmonic oscillator. Returns the displacement at time
 * `t ∈ [0, 1]` (normalised over the transition's `duration_ms`)
 * for a unit step from 0 → 1 with given `stiffness` (ω²) and
 * `damping` (2ζω). Tuned so reasonable defaults
 * (`stiffness=180`, `damping=20`) converge to ~1.0 by `t=1`.
 *
 * The closed-form solution avoids per-frame numerical
 * integration, which keeps the renderer's animation loop
 * deterministic across frame-rate jitter.
 */
export function spring(
  t: number,
  stiffness: number,
  damping: number,
): number {
  if (t <= 0) return 0;
  if (stiffness <= 0) return Math.min(1, Math.max(0, t));
  // Time scaling: the closed-form below is defined on `t' = t * T`
  // where T is a duration-independent unit (~1 second of natural
  // oscillation). For the player's normalised `t ∈ [0, 1]` we
  // pass `t' = t` directly — callers tune `stiffness`/`damping`
  // for the desired feel within the action's `duration_ms`.
  const wn = Math.sqrt(stiffness);
  const zeta = damping / (2 * wn);
  if (zeta < 1) {
    // Under-damped: oscillatory approach with decaying envelope.
    const wd = wn * Math.sqrt(1 - zeta * zeta);
    const envelope = Math.exp(-zeta * wn * t);
    return (
      1 -
      envelope *
        (Math.cos(wd * t) + ((zeta * wn) / wd) * Math.sin(wd * t))
    );
  }
  if (Math.abs(zeta - 1) < 1e-6) {
    // Critically damped: monotonic, fastest approach with no
    // overshoot.
    return 1 - (1 + wn * t) * Math.exp(-wn * t);
  }
  // Over-damped: monotonic, slower than critical.
  const wd = wn * Math.sqrt(zeta * zeta - 1);
  const a = -zeta * wn + wd;
  const b = -zeta * wn - wd;
  // C and D solve y(0)=0, y'(0)=0 starting from y_inf=1.
  const c = -b / (a - b);
  const d = a / (a - b);
  return 1 - (c * Math.exp(a * t) + d * Math.exp(b * t));
}

/**
 * Sample an arbitrary [`EasingCurve`] at `t ∈ [0, 1]`. The renderer
 * uses this directly inside `requestAnimationFrame` callbacks.
 *
 * Unknown curve kinds (forward-compat safety net for projects
 * saved by a newer build of KCreate) fall through to `easeInOut`.
 */
export function sample(curve: EasingCurve, t: number): number {
  switch (curve.kind) {
    case "linear":
      return linear(t);
    case "ease_in":
      return easeIn(t);
    case "ease_out":
      return easeOut(t);
    case "ease_in_out":
      return easeInOut(t);
    case "spring":
      return spring(t, curve.stiffness, curve.damping);
    case "cubic_bezier":
      return cubicBezier(t, curve.x1, curve.y1, curve.x2, curve.y2);
    default: {
      // Exhaustiveness check — TS will flag any new variant added
      // to the wire format that we haven't handled.
      const _exhaustive: never = curve;
      void _exhaustive;
      return easeInOut(t);
    }
  }
}

/**
 * Linear interpolation between two scalars by easing progress
 * `p ∈ [0, 1]` (typically the output of [`sample`]).
 */
export function mix(a: number, b: number, p: number): number {
  return a + (b - a) * p;
}

/**
 * Element-wise mix on a 4-tuple. Used by the Smart Animate
 * property interpolator to ease bounds + color channels.
 */
export function mix4(
  a: readonly [number, number, number, number],
  b: readonly [number, number, number, number],
  p: number,
): [number, number, number, number] {
  return [
    mix(a[0], b[0], p),
    mix(a[1], b[1], p),
    mix(a[2], b[2], p),
    mix(a[3], b[3], p),
  ];
}
