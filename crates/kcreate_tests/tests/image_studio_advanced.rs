//! Phase 8 Block B: Image Studio primitives.
//!
//! Cross-crate tests for the perspective transform,
//! color-range selection, HSL adjustment and color balance
//! adjustment.

use kcreate_ai::color_range::select_by_color_range;
use kcreate_raster::layer::AdjustmentLayer;
use kcreate_raster::tile::TileGrid;
use kcreate_raster::transform::perspective_transform;

fn make_solid_grid(w: u32, h: u32, color: [u8; 4]) -> TileGrid {
    let mut grid = TileGrid::new(w, h, 16).unwrap();
    for y in 0..h {
        for x in 0..w {
            grid.write_pixel(x, y, color);
        }
    }
    grid
}

#[test]
fn perspective_identity_preserves_pixels() {
    let grid = make_solid_grid(32, 32, [255, 100, 50, 255]);
    // Corner order: TL, TR, BL, BR (matches the implementation's convention).
    let corners = [(0.0, 0.0), (32.0, 0.0), (0.0, 32.0), (32.0, 32.0)];
    let result = perspective_transform(&grid, corners);
    assert_eq!(result.width, grid.width);
    assert_eq!(result.height, grid.height);
    // Center pixel should retain the original color.
    let pixel = result.read_pixel_clamped(16, 16);
    assert_eq!(pixel[0], 255);
    assert_eq!(pixel[1], 100);
    assert_eq!(pixel[2], 50);
}

#[test]
fn perspective_with_translated_canvas() {
    // A transform that shifts and shrinks the input — check that the
    // returned canvas has positive dimensions and is non-degenerate.
    let grid = make_solid_grid(32, 32, [255, 100, 50, 255]);
    let corners = [(5.0, 0.0), (27.0, 0.0), (0.0, 32.0), (32.0, 32.0)];
    let result = perspective_transform(&grid, corners);
    assert!(result.width > 0);
    assert!(result.height > 0);
}

#[test]
fn color_range_exact_match_selects_only_target() {
    let mut rgba = Vec::with_capacity(4 * 16);
    // 16 pixels: 8 red, 8 blue
    for _ in 0..8 {
        rgba.extend_from_slice(&[255, 0, 0, 255]);
    }
    for _ in 0..8 {
        rgba.extend_from_slice(&[0, 0, 255, 255]);
    }
    let mask = select_by_color_range(&rgba, 16, 1, [255, 0, 0, 255], 0.0);
    assert_eq!(mask.len(), 16);
    assert_eq!(mask.iter().filter(|m| **m).count(), 8);
    for (i, &m) in mask.iter().enumerate() {
        assert_eq!(m, i < 8);
    }
}

#[test]
fn color_range_fuzziness_widens_selection() {
    let mut rgba = Vec::with_capacity(4 * 4);
    rgba.extend_from_slice(&[255, 0, 0, 255]); // exact red
    rgba.extend_from_slice(&[250, 5, 5, 255]); // near red
    rgba.extend_from_slice(&[100, 100, 100, 255]); // grey
    rgba.extend_from_slice(&[0, 0, 0, 255]); // black
    let mask_strict = select_by_color_range(&rgba, 4, 1, [255, 0, 0, 255], 0.0);
    let mask_loose = select_by_color_range(&rgba, 4, 1, [255, 0, 0, 255], 1.0);
    assert!(
        mask_loose.iter().filter(|m| **m).count() >= mask_strict.iter().filter(|m| **m).count(),
        "loose fuzziness should select at least as many pixels"
    );
}

#[test]
fn hsl_zero_shift_is_identity() {
    // hue 0, saturation 1, lightness 0 means no change.
    let adj = AdjustmentLayer::HueSaturation {
        hue: 0.0,
        saturation: 1.0,
        lightness: 0.0,
    };
    let mut p = [128u8, 64u8, 200u8, 255u8];
    let before = p;
    adj.apply_pixel(&mut p);
    assert_eq!(p, before);
}

#[test]
fn color_balance_zero_is_identity() {
    let adj = AdjustmentLayer::ColorBalance {
        shadows: [0.0, 0.0, 0.0],
        midtones: [0.0, 0.0, 0.0],
        highlights: [0.0, 0.0, 0.0],
    };
    let mut p = [128u8, 64u8, 200u8, 255u8];
    let before = p;
    adj.apply_pixel(&mut p);
    assert_eq!(p, before);
}

#[test]
fn hsl_identity_predicate() {
    let identity = AdjustmentLayer::HueSaturation {
        hue: 0.0,
        saturation: 1.0,
        lightness: 0.0,
    };
    assert!(identity.is_identity());
    let not_identity = AdjustmentLayer::HueSaturation {
        hue: 0.0,
        saturation: 1.0,
        lightness: 0.5,
    };
    assert!(!not_identity.is_identity());
}
