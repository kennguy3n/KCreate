//! Pixel-domain image filters.
//!
//! Every filter takes an immutable [`TileGrid`] in and returns a fresh
//! [`TileGrid`] out. Filters operate on the **flattened** RGBA8
//! representation (`TileGrid::to_image`) and rebuild the tile layout
//! at the end. Working in the dense buffer keeps the inner loops
//! cache-friendly and lets us parallelise rows / columns with rayon
//! without touching the sparse tile bookkeeping.
//!
//! The convolution loops use `read_pixel_clamped` only for sourcing
//! at tile boundaries — the dense buffer makes the common-case fetch
//! a simple indexed load.

use rayon::prelude::*;

use crate::tile::TileGrid;

/// 1-D Gaussian kernel sized from a real-valued radius.
///
/// Returns a normalised kernel of `ceil(radius * 3) * 2 + 1`
/// coefficients. A radius of `0.0` collapses to the single-tap
/// `[1.0]` (identity).
#[must_use]
pub fn gaussian_kernel_1d(radius: f32) -> Vec<f32> {
    if radius <= 0.0 {
        return vec![1.0];
    }
    let r = radius.ceil() as i32 * 3;
    let sigma = radius;
    let two_sigma_sq = 2.0 * sigma * sigma;
    let mut k = Vec::with_capacity((r * 2 + 1) as usize);
    let mut sum = 0.0f32;
    for i in -r..=r {
        let v = (-(i as f32 * i as f32) / two_sigma_sq).exp();
        sum += v;
        k.push(v);
    }
    if sum > 0.0 {
        for v in &mut k {
            *v /= sum;
        }
    }
    k
}

fn convolve_horizontal(src: &[u8], width: usize, height: usize, kernel: &[f32]) -> Vec<u8> {
    let r = (kernel.len() / 2) as i64;
    let stride = width * 4;
    let mut out = vec![0u8; stride * height];
    out.par_chunks_mut(stride)
        .enumerate()
        .for_each(|(y, dst_row)| {
            let src_row = &src[y * stride..y * stride + stride];
            for x in 0..width {
                let mut acc = [0.0f32; 4];
                for (kidx, kv) in kernel.iter().enumerate() {
                    let sx = (x as i64 + kidx as i64 - r).clamp(0, width as i64 - 1) as usize;
                    let p = &src_row[sx * 4..sx * 4 + 4];
                    acc[0] += f32::from(p[0]) * *kv;
                    acc[1] += f32::from(p[1]) * *kv;
                    acc[2] += f32::from(p[2]) * *kv;
                    acc[3] += f32::from(p[3]) * *kv;
                }
                let dst = &mut dst_row[x * 4..x * 4 + 4];
                dst[0] = acc[0].clamp(0.0, 255.0).round() as u8;
                dst[1] = acc[1].clamp(0.0, 255.0).round() as u8;
                dst[2] = acc[2].clamp(0.0, 255.0).round() as u8;
                dst[3] = acc[3].clamp(0.0, 255.0).round() as u8;
            }
        });
    out
}

fn convolve_vertical(src: &[u8], width: usize, height: usize, kernel: &[f32]) -> Vec<u8> {
    let r = (kernel.len() / 2) as i64;
    let stride = width * 4;
    let mut out = vec![0u8; stride * height];
    // Process one output column per parallel task by chunking
    // contiguous x-ranges. A per-column par_iter would require
    // gathering rows on the fly; chunking by x-band keeps the inner
    // loop tight while still parallel.
    let bands: usize = rayon::current_num_threads().max(1);
    let band_w = width.div_ceil(bands);
    out.par_chunks_mut(stride)
        .enumerate()
        .for_each(|(y, dst_row)| {
            for band in 0..bands {
                let x0 = band * band_w;
                let x1 = ((band + 1) * band_w).min(width);
                if x0 >= x1 {
                    break;
                }
                for x in x0..x1 {
                    let mut acc = [0.0f32; 4];
                    for (kidx, kv) in kernel.iter().enumerate() {
                        let sy = (y as i64 + kidx as i64 - r).clamp(0, height as i64 - 1) as usize;
                        let off = sy * stride + x * 4;
                        let p = &src[off..off + 4];
                        acc[0] += f32::from(p[0]) * *kv;
                        acc[1] += f32::from(p[1]) * *kv;
                        acc[2] += f32::from(p[2]) * *kv;
                        acc[3] += f32::from(p[3]) * *kv;
                    }
                    let dst = &mut dst_row[x * 4..x * 4 + 4];
                    dst[0] = acc[0].clamp(0.0, 255.0).round() as u8;
                    dst[1] = acc[1].clamp(0.0, 255.0).round() as u8;
                    dst[2] = acc[2].clamp(0.0, 255.0).round() as u8;
                    dst[3] = acc[3].clamp(0.0, 255.0).round() as u8;
                }
            }
        });
    out
}

/// Separable two-pass Gaussian blur.
///
/// Identity for `radius <= 0.0`. The horizontal pass is row-parallel
/// and the vertical pass is column-parallel (chunked by x-bands).
#[must_use]
pub fn gaussian_blur(grid: &TileGrid, radius: f32) -> TileGrid {
    if radius <= 0.0 {
        return grid.clone();
    }
    let kernel = gaussian_kernel_1d(radius);
    let width = grid.width as usize;
    let height = grid.height as usize;
    if width == 0 || height == 0 {
        return grid.clone();
    }
    let src = grid.to_image();
    let h = convolve_horizontal(&src, width, height, &kernel);
    let v = convolve_vertical(&h, width, height, &kernel);
    TileGrid::from_image(&v, grid.width, grid.height, grid.tile_size).unwrap_or_else(|_| {
        // `from_image` only fails when dimensions/buffer mismatch — we
        // built the buffer ourselves so this is unreachable in practice.
        // The fallback returns the source to avoid panicking on a
        // resource-pressured machine.
        grid.clone()
    })
}

fn box_blur_pass_horizontal(src: &[u8], width: usize, height: usize, radius: usize) -> Vec<u8> {
    let stride = width * 4;
    let mut out = vec![0u8; stride * height];
    let window = (radius * 2 + 1) as i32;
    out.par_chunks_mut(stride)
        .enumerate()
        .for_each(|(y, dst_row)| {
            let src_row = &src[y * stride..y * stride + stride];
            // Initialise the sliding sum with the left edge.
            let mut sum = [0i32; 4];
            for k in -(radius as i32)..=radius as i32 {
                let sx = k.clamp(0, width as i32 - 1) as usize;
                let p = &src_row[sx * 4..sx * 4 + 4];
                sum[0] += i32::from(p[0]);
                sum[1] += i32::from(p[1]);
                sum[2] += i32::from(p[2]);
                sum[3] += i32::from(p[3]);
            }
            for x in 0..width {
                let dst = &mut dst_row[x * 4..x * 4 + 4];
                dst[0] = (sum[0] / window) as u8;
                dst[1] = (sum[1] / window) as u8;
                dst[2] = (sum[2] / window) as u8;
                dst[3] = (sum[3] / window) as u8;
                // Slide: subtract left, add right.
                let left_x = (x as i32 - radius as i32).clamp(0, width as i32 - 1) as usize;
                let right_x = (x as i32 + radius as i32 + 1).clamp(0, width as i32 - 1) as usize;
                let lp = &src_row[left_x * 4..left_x * 4 + 4];
                let rp = &src_row[right_x * 4..right_x * 4 + 4];
                for c in 0..4 {
                    sum[c] += i32::from(rp[c]);
                    sum[c] -= i32::from(lp[c]);
                }
            }
        });
    out
}

fn box_blur_pass_vertical(src: &[u8], width: usize, height: usize, radius: usize) -> Vec<u8> {
    let stride = width * 4;
    let mut out = vec![0u8; stride * height];
    let window = (radius * 2 + 1) as i32;
    // Do the work column-by-column and let rayon parallelise the
    // column dimension. We collect per-column results into a Vec<Vec<u8>>
    // then scatter them back into the strided output once, which is
    // simpler than splitting `out` into disjoint column slices.
    let col_results: Vec<Vec<u8>> = (0..width)
        .into_par_iter()
        .map(|x| {
            let mut col_out = vec![0u8; height * 4];
            let mut sum = [0i32; 4];
            for k in -(radius as i32)..=radius as i32 {
                let sy = k.clamp(0, height as i32 - 1) as usize;
                let off = sy * stride + x * 4;
                let p = &src[off..off + 4];
                sum[0] += i32::from(p[0]);
                sum[1] += i32::from(p[1]);
                sum[2] += i32::from(p[2]);
                sum[3] += i32::from(p[3]);
            }
            for y in 0..height {
                let dst = &mut col_out[y * 4..y * 4 + 4];
                dst[0] = (sum[0] / window) as u8;
                dst[1] = (sum[1] / window) as u8;
                dst[2] = (sum[2] / window) as u8;
                dst[3] = (sum[3] / window) as u8;
                let top = (y as i32 - radius as i32).clamp(0, height as i32 - 1) as usize;
                let bot = (y as i32 + radius as i32 + 1).clamp(0, height as i32 - 1) as usize;
                let tp = &src[top * stride + x * 4..top * stride + x * 4 + 4];
                let bp = &src[bot * stride + x * 4..bot * stride + x * 4 + 4];
                for c in 0..4 {
                    sum[c] += i32::from(bp[c]);
                    sum[c] -= i32::from(tp[c]);
                }
            }
            col_out
        })
        .collect();
    for (x, col) in col_results.into_iter().enumerate() {
        for y in 0..height {
            let dst = &mut out[y * stride + x * 4..y * stride + x * 4 + 4];
            let src_slice = &col[y * 4..y * 4 + 4];
            dst.copy_from_slice(src_slice);
        }
    }
    out
}

/// Three-pass sliding-window box blur. Approximates a Gaussian with
/// `radius * sqrt(3)` standard deviation for large radii at O(1)
/// per pixel per pass.
#[must_use]
pub fn box_blur(grid: &TileGrid, radius: u32) -> TileGrid {
    if radius == 0 {
        return grid.clone();
    }
    let width = grid.width as usize;
    let height = grid.height as usize;
    if width == 0 || height == 0 {
        return grid.clone();
    }
    let mut buf = grid.to_image();
    let r = radius as usize;
    for _ in 0..3 {
        buf = box_blur_pass_horizontal(&buf, width, height, r);
        buf = box_blur_pass_vertical(&buf, width, height, r);
    }
    TileGrid::from_image(&buf, grid.width, grid.height, grid.tile_size)
        .unwrap_or_else(|_| grid.clone())
}

/// Unsharp mask: `out = src + amount * (src - blur(src))` clamped to
/// `[0, 255]`, with the delta gated by a `threshold` on luminance.
///
/// `amount == 0.0` is the identity transform. `threshold == 255` is
/// also the identity (no pixel can exceed it).
#[must_use]
pub fn unsharp_mask(grid: &TileGrid, radius: f32, amount: f32, threshold: u8) -> TileGrid {
    if amount.abs() < f32::EPSILON || threshold == u8::MAX {
        return grid.clone();
    }
    let blurred = gaussian_blur(grid, radius);
    let width = grid.width as usize;
    let height = grid.height as usize;
    let stride = width * 4;
    let src = grid.to_image();
    let blr = blurred.to_image();
    let mut out = vec![0u8; stride * height];
    out.par_chunks_mut(stride)
        .enumerate()
        .for_each(|(y, dst_row)| {
            let src_row = &src[y * stride..y * stride + stride];
            let blr_row = &blr[y * stride..y * stride + stride];
            for x in 0..width {
                let s = &src_row[x * 4..x * 4 + 4];
                let b = &blr_row[x * 4..x * 4 + 4];
                let mut rgba = [0u8; 4];
                for c in 0..3 {
                    let delta = i32::from(s[c]) - i32::from(b[c]);
                    let abs_delta = delta.unsigned_abs() as u8;
                    let v = if abs_delta > threshold {
                        let scaled = (delta as f32 * amount).round() as i32;
                        (i32::from(s[c]) + scaled).clamp(0, 255) as u8
                    } else {
                        s[c]
                    };
                    rgba[c] = v;
                }
                rgba[3] = s[3];
                dst_row[x * 4..x * 4 + 4].copy_from_slice(&rgba);
            }
        });
    TileGrid::from_image(&out, grid.width, grid.height, grid.tile_size)
        .unwrap_or_else(|_| grid.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tile::TileGrid;

    fn solid(w: u32, h: u32, rgba: [u8; 4]) -> TileGrid {
        let mut buf = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..w * h {
            buf.extend_from_slice(&rgba);
        }
        TileGrid::from_image(&buf, w, h, 32).expect("tile grid")
    }

    #[test]
    fn gaussian_kernel_normalises() {
        let k = gaussian_kernel_1d(2.0);
        let sum: f32 = k.iter().sum();
        assert!((sum - 1.0).abs() < 1e-5);
    }

    #[test]
    fn gaussian_blur_zero_radius_is_identity() {
        let g = solid(8, 8, [10, 20, 30, 255]);
        let out = gaussian_blur(&g, 0.0);
        assert_eq!(out.to_image(), g.to_image());
    }

    #[test]
    fn gaussian_blur_on_constant_preserves_color() {
        let g = solid(32, 32, [10, 20, 30, 255]);
        let out = gaussian_blur(&g, 4.0);
        let buf = out.to_image();
        // Central pixels should remain at the constant colour.
        for y in 8..24 {
            for x in 8..24 {
                let off = (y * 32 + x) * 4;
                assert!((i32::from(buf[off]) - 10).abs() <= 1);
                assert!((i32::from(buf[off + 1]) - 20).abs() <= 1);
                assert!((i32::from(buf[off + 2]) - 30).abs() <= 1);
                assert_eq!(buf[off + 3], 255);
            }
        }
    }

    #[test]
    fn box_blur_zero_radius_is_identity() {
        let g = solid(8, 8, [10, 20, 30, 255]);
        let out = box_blur(&g, 0);
        assert_eq!(out.to_image(), g.to_image());
    }

    #[test]
    fn box_blur_on_constant_preserves_color() {
        let g = solid(32, 32, [40, 50, 60, 255]);
        let out = box_blur(&g, 3);
        let buf = out.to_image();
        for px in buf.chunks_exact(4) {
            assert!((i32::from(px[0]) - 40).abs() <= 1);
            assert!((i32::from(px[1]) - 50).abs() <= 1);
            assert!((i32::from(px[2]) - 60).abs() <= 1);
            assert_eq!(px[3], 255);
        }
    }

    #[test]
    fn unsharp_mask_zero_amount_is_identity() {
        let g = solid(8, 8, [10, 20, 30, 255]);
        let out = unsharp_mask(&g, 2.0, 0.0, 0);
        assert_eq!(out.to_image(), g.to_image());
    }

    #[test]
    fn unsharp_mask_threshold_max_is_identity() {
        let g = solid(8, 8, [10, 20, 30, 255]);
        let out = unsharp_mask(&g, 2.0, 1.5, u8::MAX);
        assert_eq!(out.to_image(), g.to_image());
    }

    #[test]
    fn unsharp_mask_sharpens_step_edge() {
        // 16x1 image with a step at x=8.
        let w = 16u32;
        let h = 1u32;
        let mut buf = Vec::with_capacity((w * h * 4) as usize);
        for x in 0..w {
            let v: u8 = if x < 8 { 50 } else { 200 };
            buf.extend_from_slice(&[v, v, v, 255]);
        }
        let g = TileGrid::from_image(&buf, w, h, 16).expect("grid");
        let out = unsharp_mask(&g, 2.0, 1.0, 0);
        let after = out.to_image();
        // Pixel just before the edge should darken, just after should
        // brighten — that's the textbook unsharp halo.
        let before_edge = after[7 * 4];
        let after_edge = after[8 * 4];
        assert!(
            before_edge <= 50,
            "expected darkening before edge, got {before_edge}"
        );
        assert!(
            after_edge >= 200,
            "expected brightening after edge, got {after_edge}"
        );
    }
}
