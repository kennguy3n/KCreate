//! Cross-crate integration coverage for Phase 5 raster filters,
//! transforms, and the healing brush.
//!
//! These tests exercise the real implementations in
//! `kcreate_raster::{filters,transform,heal,layer}` end-to-end:
//! TileGrid in → operation → TileGrid out → bytewise comparison.
//! There are matching crate-local unit tests in `kcreate_raster`;
//! the value here is that we lock the *cross-crate API surface* the
//! bridge wraps, so a refactor that breaks the bridge will fail
//! here too.

use kcreate_raster::filters::{box_blur, gaussian_blur, unsharp_mask};
use kcreate_raster::heal::heal;
use kcreate_raster::layer::{AdjustmentLayer, CurvePoint};
use kcreate_raster::tile::TileGrid;
use kcreate_raster::transform::{crop, flip_h, flip_v, rotate};

/// Build a tile grid filled with the given per-row RGBA gradient.
fn grad_grid(w: u32, h: u32, tile: u32) -> TileGrid {
    let mut buf = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            buf[i] = (x as u8).wrapping_mul(7);
            buf[i + 1] = (y as u8).wrapping_mul(11);
            buf[i + 2] = 128;
            buf[i + 3] = 255;
        }
    }
    TileGrid::from_image(&buf, w, h, tile).expect("from_image")
}

fn solid_grid(w: u32, h: u32, tile: u32, rgba: [u8; 4]) -> TileGrid {
    let buf: Vec<u8> = (0..w * h).flat_map(|_| rgba).collect();
    TileGrid::from_image(&buf, w, h, tile).expect("from_image")
}

#[test]
fn levels_identity_is_bitwise_identical() {
    let grid = grad_grid(64, 64, 32);
    let mut rgba = grid.to_image();
    let before = rgba.clone();
    apply_adjustment_per_pixel(
        &mut rgba,
        &AdjustmentLayer::Levels {
            black_point: 0.0,
            white_point: 1.0,
            gamma: 1.0,
        },
    );
    assert_eq!(rgba, before, "levels identity must not change bytes");
}

#[test]
fn curves_identity_is_bitwise_identical() {
    let grid = grad_grid(64, 64, 32);
    let mut rgba = grid.to_image();
    let before = rgba.clone();
    apply_adjustment_per_pixel(
        &mut rgba,
        &AdjustmentLayer::Curves(vec![CurvePoint::new(0.0, 0.0), CurvePoint::new(1.0, 1.0)]),
    );
    assert_eq!(rgba, before, "curves identity must not change bytes");
}

#[test]
fn gaussian_blur_radius_zero_is_identity() {
    let grid = grad_grid(64, 64, 32);
    let before = grid.to_image();
    let out = gaussian_blur(&grid, 0.0);
    assert_eq!(out.to_image(), before, "gaussian r=0 is identity");
}

#[test]
fn box_blur_radius_zero_is_identity() {
    let grid = grad_grid(64, 64, 32);
    let before = grid.to_image();
    let out = box_blur(&grid, 0);
    assert_eq!(out.to_image(), before, "box r=0 is identity");
}

#[test]
fn unsharp_amount_zero_is_identity() {
    let grid = grad_grid(64, 64, 32);
    let before = grid.to_image();
    let out = unsharp_mask(&grid, 1.0, 0.0, 0);
    assert_eq!(out.to_image(), before, "amount=0 is identity");
}

#[test]
fn unsharp_high_threshold_is_identity() {
    let grid = grad_grid(64, 64, 32);
    let before = grid.to_image();
    let out = unsharp_mask(&grid, 1.0, 1.0, 255);
    assert_eq!(out.to_image(), before, "threshold=255 is identity");
}

#[test]
fn crop_produces_correct_dimensions() {
    let grid = grad_grid(64, 64, 32);
    let cropped = crop(&grid, 8, 16, 24, 32);
    assert_eq!(cropped.width, 24);
    assert_eq!(cropped.height, 32);
}

#[test]
fn rotate_360_is_near_identity() {
    let grid = grad_grid(32, 32, 32);
    let rotated = rotate(&grid, 360.0);
    // Bilinear introduces tiny error at the edges; allow a small
    // per-channel tolerance.
    let before = grid.to_image();
    let after = rotated.to_image();
    assert_eq!(before.len(), after.len());
    let mut max_delta = 0i32;
    for (a, b) in before.iter().zip(after.iter()) {
        let d = (i32::from(*a) - i32::from(*b)).abs();
        if d > max_delta {
            max_delta = d;
        }
    }
    assert!(
        max_delta <= 4,
        "max delta {max_delta} too large for 360 rotation"
    );
}

#[test]
fn flip_twice_is_identity() {
    let mut grid = grad_grid(32, 32, 32);
    let before = grid.to_image();
    flip_h(&mut grid);
    flip_h(&mut grid);
    assert_eq!(grid.to_image(), before, "flip_h twice");

    let mut grid2 = grad_grid(32, 32, 32);
    flip_v(&mut grid2);
    flip_v(&mut grid2);
    assert_eq!(grid2.to_image(), before, "flip_v twice");
}

#[test]
fn heal_at_same_src_and_dst_is_near_identity() {
    let mut grid = grad_grid(32, 32, 16);
    let before = grid.to_image();
    heal(&mut grid, 16, 16, 16, 16, 4);
    // Source == destination: heal is a per-pixel cosine-squared
    // alpha blend of (sp == existing) against (existing), so the
    // result is mathematically identical to the source — bar a
    // 1-unit rounding error from the f32→u8 conversion.
    let after = grid.to_image();
    let mut max = 0i32;
    for (a, b) in before.iter().zip(after.iter()) {
        max = max.max((i32::from(*a) - i32::from(*b)).abs());
    }
    assert!(max <= 2, "heal same src/dst max delta = {max}");
}

#[test]
fn heal_on_uniform_color_is_near_identity() {
    let mut grid = solid_grid(32, 32, 16, [200, 100, 50, 255]);
    let before = grid.to_image();
    heal(&mut grid, 8, 8, 20, 20, 6);
    let after = grid.to_image();
    // Uniform field: every blended source pixel equals every
    // existing pixel, so all writes are within rounding error.
    let mut max = 0i32;
    for (a, b) in before.iter().zip(after.iter()) {
        max = max.max((i32::from(*a) - i32::from(*b)).abs());
    }
    assert!(max <= 1, "heal on uniform max delta = {max}");
}

/// Helper: apply one adjustment to a flat RGBA buffer the same way
/// the bridge's `raster_ops` module does. We walk the buffer
/// pixel-by-pixel so the test exercises the public
/// `AdjustmentLayer::apply_pixel` surface.
fn apply_adjustment_per_pixel(rgba: &mut [u8], adj: &AdjustmentLayer) {
    for chunk in rgba.chunks_exact_mut(4) {
        let mut px: [u8; 4] = [chunk[0], chunk[1], chunk[2], chunk[3]];
        adj.apply_pixel(&mut px);
        chunk.copy_from_slice(&px);
    }
}
