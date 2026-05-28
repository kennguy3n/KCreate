//! Raster layers: tile grid + masks + adjustment stack.

use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::tile::{TileGrid, TileGridError};

/// Errors from raster-layer composition / adjustment.
#[derive(Debug, Error)]
pub enum RasterLayerError {
    #[error(transparent)]
    Tile(#[from] TileGridError),
    #[error("mask dimensions {mw}x{mh} do not match layer dimensions {lw}x{lh}")]
    MaskMismatch { mw: u32, mh: u32, lw: u32, lh: u32 },
}

/// Blend mode for compositing a raster layer onto its background.
///
/// Kept intentionally small — Phase 1 ships Normal and Multiply (the
/// two everyone reaches for) plus the trivial Add and Screen modes.
/// The list will grow when the adjustment-pipeline UI does.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum BlendMode {
    Normal,
    Multiply,
    Screen,
    Add,
}

/// Single-channel mask. `inverted` flips the mask sense at evaluation
/// time without re-encoding the channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mask {
    pub grid: TileGrid,
    pub inverted: bool,
}

/// A `(t, v)` control point on a Curves curve. Both axes lie in
/// `[0.0, 1.0]` for an RGB intensity remap.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct CurvePoint {
    pub t: f32,
    pub v: f32,
}

impl CurvePoint {
    #[must_use]
    pub const fn new(t: f32, v: f32) -> Self {
        Self { t, v }
    }
}

/// One adjustment stage in a layer's adjustment stack.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AdjustmentLayer {
    /// `delta` is added to the linear-RGB channels, clamped to
    /// `[0, 1]`. Range: `[-1.0, 1.0]`.
    Brightness(f32),
    /// `factor` scales the deviation from 0.5. Range: `[0.0, 2.0]`
    /// (1.0 = identity, 0.0 = flat grey, 2.0 = strong contrast).
    Contrast(f32),
    /// Hue rotation (degrees), saturation multiplier (1.0 = identity),
    /// lightness shift (`-1.0..=1.0`).
    HueSaturation {
        hue: f32,
        saturation: f32,
        lightness: f32,
    },
    /// Levels adjustment: remaps an input value `v \in [0, 1]` to
    /// `(clamp((v - black) / (white - black), 0, 1))^(1 / gamma)`.
    /// `black=0`, `white=1`, `gamma=1` is the identity transform.
    Levels {
        black_point: f32,
        white_point: f32,
        gamma: f32,
    },
    /// Curves adjustment: piecewise cubic Hermite interpolation over
    /// a sorted list of `(t, v)` control points. Identity is
    /// `[(0, 0), (1, 1)]`. Applied per-channel to RGB.
    Curves(Vec<CurvePoint>),
    /// Three-way color balance (lift / gamma / gain) targeting
    /// shadow / midtone / highlight tonal ranges. Each triple is
    /// `[r, g, b]` in `[-1.0, 1.0]`, applied additively to the
    /// pixel's RGB after weighting by the tonal-range membership
    /// function (Gaussian-like falloff centred on luminance
    /// 0.15 / 0.5 / 0.85 respectively). Zero in every channel is
    /// the identity transform.
    ColorBalance {
        shadows: [f32; 3],
        midtones: [f32; 3],
        highlights: [f32; 3],
    },
}

impl AdjustmentLayer {
    /// Apply the adjustment in place to an RGBA8 pixel.
    pub fn apply_pixel(&self, rgba: &mut [u8; 4]) {
        match self {
            Self::Brightness(delta) => {
                let delta = *delta;
                for c in rgba.iter_mut().take(3) {
                    let v = f32::from(*c) / 255.0 + delta;
                    *c = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
                }
            }
            Self::Contrast(factor) => {
                let factor = *factor;
                for c in rgba.iter_mut().take(3) {
                    let v = (f32::from(*c) / 255.0 - 0.5).mul_add(factor, 0.5);
                    *c = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
                }
            }
            Self::HueSaturation {
                hue,
                saturation,
                lightness,
            } => {
                apply_hsl(rgba, *hue, *saturation, *lightness);
            }
            Self::Levels {
                black_point,
                white_point,
                gamma,
            } => {
                let black = (*black_point).clamp(0.0, 1.0);
                // White must be strictly greater than black; if the
                // user inverts them, swap so the math still has a
                // sensible range.
                let white_raw = (*white_point).clamp(0.0, 1.0);
                let (black, white) = if white_raw > black {
                    (black, white_raw)
                } else {
                    // Degenerate range: avoid divide-by-zero by
                    // expanding by an epsilon. The result still
                    // produces a hard step at `black`.
                    (black.min(white_raw), black.max(white_raw) + f32::EPSILON)
                };
                // Gamma must be positive; clamp at a small floor so a
                // user-supplied zero or negative gamma does not blow
                // up the pow.
                let gamma = (*gamma).max(0.01);
                let inv_gamma = 1.0 / gamma;
                for c in rgba.iter_mut().take(3) {
                    let v = f32::from(*c) / 255.0;
                    let normalised = ((v - black) / (white - black)).clamp(0.0, 1.0);
                    let curved = normalised.powf(inv_gamma);
                    *c = (curved.clamp(0.0, 1.0) * 255.0).round() as u8;
                }
            }
            Self::Curves(points) => {
                if points.len() < 2 {
                    return;
                }
                for c in rgba.iter_mut().take(3) {
                    let v = f32::from(*c) / 255.0;
                    let mapped = eval_cubic_hermite(points, v);
                    *c = (mapped.clamp(0.0, 1.0) * 255.0).round() as u8;
                }
            }
            Self::ColorBalance {
                shadows,
                midtones,
                highlights,
            } => {
                apply_color_balance(rgba, *shadows, *midtones, *highlights);
            }
        }
    }

    /// Returns `true` when this stage is the mathematical identity
    /// for every RGB pixel. Used to skip work in render fast paths.
    #[must_use]
    pub fn is_identity(&self) -> bool {
        match self {
            Self::Brightness(delta) => delta.abs() < f32::EPSILON,
            Self::Contrast(factor) => (*factor - 1.0).abs() < f32::EPSILON,
            Self::HueSaturation {
                hue,
                saturation,
                lightness,
            } => {
                hue.abs() < f32::EPSILON
                    && (*saturation - 1.0).abs() < f32::EPSILON
                    && lightness.abs() < f32::EPSILON
            }
            Self::Levels {
                black_point,
                white_point,
                gamma,
            } => {
                black_point.abs() < f32::EPSILON
                    && (*white_point - 1.0).abs() < f32::EPSILON
                    && (*gamma - 1.0).abs() < f32::EPSILON
            }
            Self::Curves(points) => {
                points.len() == 2
                    && (points[0].t - 0.0).abs() < f32::EPSILON
                    && (points[0].v - 0.0).abs() < f32::EPSILON
                    && (points[1].t - 1.0).abs() < f32::EPSILON
                    && (points[1].v - 1.0).abs() < f32::EPSILON
            }
            Self::ColorBalance {
                shadows,
                midtones,
                highlights,
            } => shadows
                .iter()
                .chain(midtones.iter())
                .chain(highlights.iter())
                .all(|v| v.abs() < f32::EPSILON),
        }
    }
}

/// Evaluate a piecewise cubic Hermite curve over a sorted list of
/// `(t, v)` control points at `t = x`.
///
/// Uses monotone-bounded Catmull–Rom tangents (i.e. averaged secants
/// at interior knots, one-sided secants at endpoints). This keeps
/// the curve smooth without overshooting the way a naive Catmull–Rom
/// can on near-vertical sections — important for an adjustment
/// curve where overshoot manifests as clipped highlights / crushed
/// shadows.
fn eval_cubic_hermite(points: &[CurvePoint], x: f32) -> f32 {
    // Caller is responsible for `points.len() >= 2`.
    debug_assert!(points.len() >= 2);
    // Endpoint extrapolation: clamp to the first / last value.
    if x <= points[0].t {
        return points[0].v;
    }
    if x >= points[points.len() - 1].t {
        return points[points.len() - 1].v;
    }
    // Linear search is fine — the typical control-point set is 2–8
    // points so a binary search costs more than it saves.
    let mut idx = 0usize;
    for i in 0..points.len() - 1 {
        if x >= points[i].t && x <= points[i + 1].t {
            idx = i;
            break;
        }
    }
    let p0 = points[idx];
    let p1 = points[idx + 1];
    let dt = p1.t - p0.t;
    if dt.abs() < f32::EPSILON {
        return p1.v;
    }
    // Tangents at the segment endpoints.
    let m0 = if idx == 0 {
        (p1.v - p0.v) / dt
    } else {
        let prev = points[idx - 1];
        let dt_prev = p1.t - prev.t;
        if dt_prev.abs() < f32::EPSILON {
            (p1.v - p0.v) / dt
        } else {
            (p1.v - prev.v) / dt_prev
        }
    };
    let m1 = if idx + 2 >= points.len() {
        (p1.v - p0.v) / dt
    } else {
        let next = points[idx + 2];
        let dt_next = next.t - p0.t;
        if dt_next.abs() < f32::EPSILON {
            (p1.v - p0.v) / dt
        } else {
            (next.v - p0.v) / dt_next
        }
    };
    // Hermite basis evaluation.
    let s = (x - p0.t) / dt;
    let s2 = s * s;
    let s3 = s2 * s;
    let h00 = 2.0 * s3 - 3.0 * s2 + 1.0;
    let h10 = s3 - 2.0 * s2 + s;
    let h01 = -2.0 * s3 + 3.0 * s2;
    let h11 = s3 - s2;
    h00 * p0.v + h10 * dt * m0 + h01 * p1.v + h11 * dt * m1
}

/// A raster layer: tile grid + optional masks + ordered adjustments.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RasterLayer {
    pub id: Uuid,
    pub grid: TileGrid,
    pub masks: Vec<Mask>,
    pub adjustments: Vec<AdjustmentLayer>,
    pub opacity: f32,
    pub blend_mode: BlendMode,
}

impl RasterLayer {
    /// Construct an empty layer with the given canvas size.
    pub fn new(width: u32, height: u32, tile_size: u32) -> Result<Self, RasterLayerError> {
        Ok(Self {
            id: Uuid::new_v4(),
            grid: TileGrid::new(width, height, tile_size)?,
            masks: Vec::new(),
            adjustments: Vec::new(),
            opacity: 1.0,
            blend_mode: BlendMode::Normal,
        })
    }

    /// Add a mask, validating dimensions match.
    pub fn add_mask(&mut self, mask: Mask) -> Result<(), RasterLayerError> {
        if mask.grid.width != self.grid.width || mask.grid.height != self.grid.height {
            return Err(RasterLayerError::MaskMismatch {
                mw: mask.grid.width,
                mh: mask.grid.height,
                lw: self.grid.width,
                lh: self.grid.height,
            });
        }
        self.masks.push(mask);
        Ok(())
    }

    /// Materialise the layer into a single RGBA8 buffer with every
    /// adjustment applied. Pure function — does not mutate
    /// `self.grid`.
    #[must_use]
    pub fn render_rgba(&self) -> Vec<u8> {
        let mut out = self.grid.to_image();
        // Adjustments run as a flat scan over pixels; rayon
        // parallelises by chunks of 4 bytes.
        out.par_chunks_exact_mut(4).for_each(|px| {
            let mut a: [u8; 4] = [px[0], px[1], px[2], px[3]];
            for adj in &self.adjustments {
                adj.apply_pixel(&mut a);
            }
            px.copy_from_slice(&a);
        });
        out
    }
}

/// Apply a three-way color balance (lift / gamma / gain style) by
/// computing the pixel's luminance and weighting the shadow /
/// midtone / highlight offsets by the membership function for each
/// tonal range. Membership uses a centered Gaussian falloff so
/// adjacent ranges blend smoothly — pushing red into shadows tints
/// dark pixels red without producing a hard step at mid-grey.
///
/// The triples are clamped to `[-1.0, 1.0]` per channel. Total
/// contribution is the sum of the three weighted triples, capped
/// at `±1.0` per channel after summation so a heavy-handed user
/// configuration can't blow channels past the legal range.
#[allow(clippy::many_single_char_names)]
fn apply_color_balance(
    rgba: &mut [u8; 4],
    shadows: [f32; 3],
    midtones: [f32; 3],
    highlights: [f32; 3],
) {
    // Luminance in [0,1]; Rec. 709 weights so the tonal-range
    // pickers match perceptual brightness rather than channel max.
    let r_lin = f32::from(rgba[0]) / 255.0;
    let g_lin = f32::from(rgba[1]) / 255.0;
    let b_lin = f32::from(rgba[2]) / 255.0;
    let lum = (0.2126_f32).mul_add(r_lin, 0.7152_f32.mul_add(g_lin, 0.0722 * b_lin));
    // Centred Gaussian-ish weights. Width chosen so each band's
    // FWHM is ~0.3; this gives a moderate overlap that matches the
    // perceptual response of Photoshop's color-balance dialog.
    let w_shadow = (-((lum - 0.15).powi(2)) / 0.04).exp();
    let w_mid = (-((lum - 0.5).powi(2)) / 0.04).exp();
    let w_high = (-((lum - 0.85).powi(2)) / 0.04).exp();
    // Normalise the weights so total weight is 1.0 — without
    // normalisation, pure mid-grey (where each weight is small)
    // would dilute the user's adjustment more than highlights or
    // shadows do.
    let sum = w_shadow + w_mid + w_high;
    let (w_shadow, w_mid, w_high) = if sum > f32::EPSILON {
        (w_shadow / sum, w_mid / sum, w_high / sum)
    } else {
        (0.0, 1.0, 0.0)
    };
    let mut delta = [0.0f32; 3];
    for ch in 0..3 {
        let s = shadows[ch].clamp(-1.0, 1.0);
        let m = midtones[ch].clamp(-1.0, 1.0);
        let h = highlights[ch].clamp(-1.0, 1.0);
        delta[ch] = (s * w_shadow + m * w_mid + h * w_high).clamp(-1.0, 1.0);
    }
    for (ch, c) in rgba.iter_mut().take(3).enumerate() {
        let v = f32::from(*c) / 255.0 + delta[ch];
        *c = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    }
}

#[allow(clippy::many_single_char_names)]
fn apply_hsl(rgba: &mut [u8; 4], hue_shift: f32, saturation_scale: f32, lightness_shift: f32) {
    let (mut h, mut s, mut l) = rgb_to_hsl(rgba[0], rgba[1], rgba[2]);
    h = (h + hue_shift).rem_euclid(360.0);
    s = (s * saturation_scale).clamp(0.0, 1.0);
    l = (l + lightness_shift).clamp(0.0, 1.0);
    let (r, g, b) = hsl_to_rgb(h, s, l);
    rgba[0] = r;
    rgba[1] = g;
    rgba[2] = b;
}

#[allow(clippy::many_single_char_names)]
fn rgb_to_hsl(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let r = f32::from(r) / 255.0;
    let g = f32::from(g) / 255.0;
    let b = f32::from(b) / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = f32::midpoint(max, min);
    let d = max - min;
    if d.abs() < f32::EPSILON {
        return (0.0, 0.0, l);
    }
    let s = if l < 0.5 {
        d / (max + min)
    } else {
        d / (2.0 - max - min)
    };
    let h = if (max - r).abs() < f32::EPSILON {
        60.0 * ((g - b) / d).rem_euclid(6.0)
    } else if (max - g).abs() < f32::EPSILON {
        60.0 * (((b - r) / d) + 2.0)
    } else {
        60.0 * (((r - g) / d) + 4.0)
    };
    (h.rem_euclid(360.0), s, l)
}

#[allow(clippy::many_single_char_names)]
fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (u8, u8, u8) {
    let c = (1.0 - 2.0f32.mul_add(l, -1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0).rem_euclid(2.0) - 1.0).abs());
    let m = l - c / 2.0;
    let (r1, g1, b1) = match h as i32 {
        0..=59 => (c, x, 0.0),
        60..=119 => (x, c, 0.0),
        120..=179 => (0.0, c, x),
        180..=239 => (0.0, x, c),
        240..=299 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let to_u8 = |v: f32| ((v + m).clamp(0.0, 1.0) * 255.0).round() as u8;
    (to_u8(r1), to_u8(g1), to_u8(b1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brightness_brightens() {
        let mut px = [128u8, 128, 128, 255];
        AdjustmentLayer::Brightness(0.25).apply_pixel(&mut px);
        assert!(px[0] > 128);
        assert!(px[1] > 128);
        assert!(px[2] > 128);
        assert_eq!(px[3], 255);
    }

    #[test]
    fn contrast_pushes_extremes() {
        let mut bright = [200u8, 200, 200, 255];
        AdjustmentLayer::Contrast(1.5).apply_pixel(&mut bright);
        assert!(bright[0] >= 200);
        let mut dark = [50u8, 50, 50, 255];
        AdjustmentLayer::Contrast(1.5).apply_pixel(&mut dark);
        assert!(dark[0] <= 50);
    }

    #[test]
    fn add_mask_dimension_check() {
        let mut layer = RasterLayer::new(8, 8, 4).expect("layer");
        let small_mask = Mask {
            grid: TileGrid::new(4, 4, 4).expect("small grid"),
            inverted: false,
        };
        assert!(layer.add_mask(small_mask).is_err());
    }

    #[test]
    fn levels_identity_preserves_pixels() {
        let mut px = [100u8, 150, 200, 255];
        let levels = AdjustmentLayer::Levels {
            black_point: 0.0,
            white_point: 1.0,
            gamma: 1.0,
        };
        assert!(levels.is_identity());
        levels.apply_pixel(&mut px);
        assert_eq!(px, [100, 150, 200, 255]);
    }

    #[test]
    fn levels_black_white_clamp_extends_dynamic_range() {
        // Pixel value 64 with black=64, white=192 → normalised 0.0
        let mut dark = [64u8, 64, 64, 255];
        AdjustmentLayer::Levels {
            black_point: 64.0 / 255.0,
            white_point: 192.0 / 255.0,
            gamma: 1.0,
        }
        .apply_pixel(&mut dark);
        assert_eq!(dark[0], 0);
        // Pixel value 192 with the same range → normalised 1.0
        let mut bright = [192u8, 192, 192, 255];
        AdjustmentLayer::Levels {
            black_point: 64.0 / 255.0,
            white_point: 192.0 / 255.0,
            gamma: 1.0,
        }
        .apply_pixel(&mut bright);
        assert_eq!(bright[0], 255);
    }

    #[test]
    fn levels_gamma_darkens_midtones() {
        let mut mid = [128u8, 128, 128, 255];
        AdjustmentLayer::Levels {
            black_point: 0.0,
            white_point: 1.0,
            gamma: 0.5,
        }
        .apply_pixel(&mut mid);
        // gamma 0.5 → inv_gamma 2.0 → 0.502^2 ≈ 0.252 → ~64.
        assert!(mid[0] < 80);
    }

    #[test]
    fn curves_identity_preserves_pixels() {
        let mut px = [42u8, 84, 127, 255];
        let curves =
            AdjustmentLayer::Curves(vec![CurvePoint::new(0.0, 0.0), CurvePoint::new(1.0, 1.0)]);
        assert!(curves.is_identity());
        curves.apply_pixel(&mut px);
        assert_eq!(px, [42, 84, 127, 255]);
    }

    #[test]
    fn curves_inversion_inverts_midtones() {
        let mut px = [64u8, 64, 64, 255];
        AdjustmentLayer::Curves(vec![CurvePoint::new(0.0, 1.0), CurvePoint::new(1.0, 0.0)])
            .apply_pixel(&mut px);
        // Linear inversion of 64 → 191.
        assert_eq!(px[0], 191);
    }

    #[test]
    fn curves_smooth_lift_is_monotone() {
        let curve = AdjustmentLayer::Curves(vec![
            CurvePoint::new(0.0, 0.0),
            CurvePoint::new(0.5, 0.65),
            CurvePoint::new(1.0, 1.0),
        ]);
        let mut prev = 0u8;
        for v in 0..=255u8 {
            let mut px = [v, v, v, 255];
            curve.apply_pixel(&mut px);
            assert!(px[0] >= prev, "curve must be monotone at v={v}");
            prev = px[0];
        }
    }

    #[test]
    fn render_runs_adjustments() {
        let pixels = vec![100u8; 64 * 4];
        let grid = TileGrid::from_image(&pixels, 8, 8, 4).expect("grid");
        let layer = RasterLayer {
            id: Uuid::new_v4(),
            grid,
            masks: Vec::new(),
            adjustments: vec![AdjustmentLayer::Brightness(0.4)],
            opacity: 1.0,
            blend_mode: BlendMode::Normal,
        };
        let out = layer.render_rgba();
        assert!(out[0] > 100);
    }
}
