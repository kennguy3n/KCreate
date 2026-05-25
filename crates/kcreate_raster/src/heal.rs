//! Foundation healing brush.
//!
//! Copies a disc of pixels from `(src_x, src_y)` to `(dst_x, dst_y)`
//! with two adjustments:
//!
//! 1. **Luminance offset correction.** The mean luminance of an
//!    annular ring at the source disc boundary and at the destination
//!    disc boundary is computed. The destination pixels are shifted
//!    by the difference, so the patched region matches its new
//!    surroundings instead of looking like a hard paste.
//! 2. **Radial alpha falloff.** Pixels closer to the disc centre are
//!    fully replaced; pixels at the boundary blend with the existing
//!    destination via a cosine-squared falloff. This eliminates the
//!    visible disc edge that a naive copy produces.
//!
//! The algorithm is intentionally CPU-side and tile-aware via
//! [`TileGrid::read_pixel_clamped`] / [`TileGrid::write_pixel`] so it
//! works regardless of whether the source disc straddles tile
//! boundaries.

use crate::tile::TileGrid;

fn luminance(rgba: [u8; 4]) -> f32 {
    // BT.601 luma; for healing-brush smoothing this is more than
    // accurate enough.
    0.299 * f32::from(rgba[0]) + 0.587 * f32::from(rgba[1]) + 0.114 * f32::from(rgba[2])
}

fn ring_mean_luminance(
    grid: &TileGrid,
    cx: i64,
    cy: i64,
    radius_inner: f32,
    radius_outer: f32,
) -> f32 {
    let r2_inner = radius_inner * radius_inner;
    let r2_outer = radius_outer * radius_outer;
    let mut total = 0.0f32;
    let mut count = 0usize;
    let r_i = radius_outer.ceil() as i64;
    for dy in -r_i..=r_i {
        for dx in -r_i..=r_i {
            let d2 = (dx * dx + dy * dy) as f32;
            if d2 >= r2_inner && d2 <= r2_outer {
                total += luminance(grid.read_pixel_clamped(cx + dx, cy + dy));
                count += 1;
            }
        }
    }
    if count == 0 {
        0.0
    } else {
        total / count as f32
    }
}

/// Apply a single healing-brush stamp.
///
/// `radius` is the disc radius in pixels. `src_x`/`src_y` and
/// `dst_x`/`dst_y` are the disc centres. Out-of-bounds writes are
/// silently dropped; the source disc is read with clamping at the
/// grid edges (so healing near the border still produces a sensible
/// result without a panic).
pub fn heal(grid: &mut TileGrid, src_x: u32, src_y: u32, dst_x: u32, dst_y: u32, radius: u32) {
    if radius == 0 {
        return;
    }
    let r = radius as f32;
    let r2 = r * r;
    // Luminance offset = mean(dst ring) - mean(src ring).
    let src_lum = ring_mean_luminance(grid, src_x as i64, src_y as i64, r, r + 2.0);
    let dst_lum = ring_mean_luminance(grid, dst_x as i64, dst_y as i64, r, r + 2.0);
    let lum_offset = dst_lum - src_lum;
    let r_i = radius as i64;
    // Read all source pixels up front so the destination writes can't
    // disturb the source patch when source and destination overlap.
    let mut samples: Vec<(i64, i64, [u8; 4])> = Vec::new();
    for dy in -r_i..=r_i {
        for dx in -r_i..=r_i {
            let d2 = (dx * dx + dy * dy) as f32;
            if d2 > r2 {
                continue;
            }
            let sp = grid.read_pixel_clamped(src_x as i64 + dx, src_y as i64 + dy);
            samples.push((dx, dy, sp));
        }
    }
    for (dx, dy, sp) in samples {
        let d2 = (dx * dx + dy * dy) as f32;
        let d = d2.sqrt();
        // Cosine-squared falloff: 1.0 at centre, 0.0 at the disc edge.
        let t = (d / r).clamp(0.0, 1.0);
        let alpha = (1.0 - t).powi(2);
        let nx = dst_x as i64 + dx;
        let ny = dst_y as i64 + dy;
        if nx < 0 || ny < 0 || nx >= grid.width as i64 || ny >= grid.height as i64 {
            continue;
        }
        let existing = grid.read_pixel_clamped(nx, ny);
        // Apply the luminance offset to the source sample before
        // blending. Channels are shifted uniformly so the chrominance
        // stays intact and only brightness moves toward the
        // destination's surroundings.
        let mut adj = [0u8; 4];
        for c in 0..3 {
            let v = f32::from(sp[c]) + lum_offset;
            adj[c] = v.clamp(0.0, 255.0) as u8;
        }
        adj[3] = sp[3];
        let mut out = [0u8; 4];
        for c in 0..4 {
            let v = f32::from(adj[c]) * alpha + f32::from(existing[c]) * (1.0 - alpha);
            out[c] = v.clamp(0.0, 255.0) as u8;
        }
        grid.write_pixel(nx as u32, ny as u32, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, rgba: [u8; 4]) -> TileGrid {
        let mut buf = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..w * h {
            buf.extend_from_slice(&rgba);
        }
        TileGrid::from_image(&buf, w, h, 16).expect("grid")
    }

    #[test]
    fn heal_uniform_is_identity() {
        let mut g = solid(32, 32, [100, 120, 140, 255]);
        let before = g.to_image();
        heal(&mut g, 10, 10, 20, 20, 4);
        let after = g.to_image();
        // On a perfectly uniform field every blend should reproduce
        // the original colour (allow a 1-unit rounding tolerance).
        let mut max_err: i32 = 0;
        for (a, b) in before.iter().zip(after.iter()) {
            max_err = max_err.max((i32::from(*a) - i32::from(*b)).abs());
        }
        assert!(max_err <= 1, "max heal-uniform delta = {max_err}");
    }

    #[test]
    fn heal_same_source_and_dest_is_near_identity() {
        // A non-uniform image healed onto itself with src == dst should
        // be approximately the identity within the falloff blend.
        let mut buf = Vec::with_capacity(32 * 32 * 4);
        for y in 0..32u32 {
            for x in 0..32u32 {
                buf.extend_from_slice(&[
                    (x * 8) as u8,
                    (y * 8) as u8,
                    50,
                    255,
                ]);
            }
        }
        let mut g = TileGrid::from_image(&buf, 32, 32, 16).expect("grid");
        let before = g.to_image();
        heal(&mut g, 16, 16, 16, 16, 4);
        let after = g.to_image();
        let mut max_err: i32 = 0;
        for (a, b) in before.iter().zip(after.iter()) {
            max_err = max_err.max((i32::from(*a) - i32::from(*b)).abs());
        }
        // Self-heal should be near-identity; allow a small tolerance
        // for the falloff blend.
        assert!(max_err <= 2, "max self-heal delta = {max_err}");
    }

    #[test]
    fn heal_replaces_center_with_source() {
        // Destination is a black disc target, source is bright red.
        // After healing, the disc centre should be predominantly red
        // (luminance-adjusted to match the surrounding black, which
        // will pull the red toward black).
        let mut buf = vec![0u8; 32 * 32 * 4];
        for px in buf.chunks_exact_mut(4) {
            px[3] = 255;
        }
        // Splash a red square in the corner that will serve as the source.
        for y in 0..8 {
            for x in 0..8 {
                let off = (y * 32 + x) * 4;
                buf[off] = 200;
                buf[off + 1] = 0;
                buf[off + 2] = 0;
                buf[off + 3] = 255;
            }
        }
        let mut g = TileGrid::from_image(&buf, 32, 32, 16).expect("grid");
        heal(&mut g, 3, 3, 20, 20, 2);
        let after = g.to_image();
        let off = (20 * 32 + 20) * 4;
        // The luminance offset corrects toward black, but the red
        // channel of the source is still the dominant colour at the
        // centre.
        assert!(after[off] >= after[off + 1]);
        assert!(after[off] >= after[off + 2]);
    }
}
