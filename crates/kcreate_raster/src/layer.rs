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
}

impl AdjustmentLayer {
    /// Apply the adjustment in place to an RGBA8 pixel.
    pub fn apply_pixel(&self, rgba: &mut [u8; 4]) {
        match *self {
            Self::Brightness(delta) => {
                for c in rgba.iter_mut().take(3) {
                    let v = f32::from(*c) / 255.0 + delta;
                    *c = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
                }
            }
            Self::Contrast(factor) => {
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
                apply_hsl(rgba, hue, saturation, lightness);
            }
        }
    }
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
