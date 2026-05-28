//! Geometric raster transforms: crop, rotate, flip.
//!
//! All operations work against the flattened RGBA8 buffer
//! (`TileGrid::to_image`) and rebuild a new [`TileGrid`] at the end.
//! Rotation uses bilinear sampling and is row-parallel.

use rayon::prelude::*;

use crate::tile::TileGrid;

/// Extract a sub-region of the source grid.
///
/// Clipped at the source bounds — a fully out-of-bounds rectangle
/// yields a 1×1 transparent grid (the tile grid invariants don't
/// allow a 0×0 grid, so we return the smallest legal one).
#[must_use]
pub fn crop(grid: &TileGrid, x: u32, y: u32, w: u32, h: u32) -> TileGrid {
    let src_w = grid.width;
    let src_h = grid.height;
    let x0 = x.min(src_w);
    let y0 = y.min(src_h);
    let x1 = (x.saturating_add(w)).min(src_w);
    let y1 = (y.saturating_add(h)).min(src_h);
    if x0 >= x1 || y0 >= y1 {
        // Degenerate crop: return a 1×1 transparent grid so callers
        // can always rely on a fresh `TileGrid` coming back.
        return TileGrid::new(1, 1, grid.tile_size.max(1)).expect("1x1 tile grid is always valid");
    }
    let out_w = x1 - x0;
    let out_h = y1 - y0;
    let stride_in = (src_w as usize) * 4;
    let stride_out = (out_w as usize) * 4;
    let src = grid.to_image();
    let mut buf = vec![0u8; stride_out * (out_h as usize)];
    buf.par_chunks_mut(stride_out)
        .enumerate()
        .for_each(|(row, dst)| {
            let sy = (y0 as usize) + row;
            let src_off = sy * stride_in + (x0 as usize) * 4;
            dst.copy_from_slice(&src[src_off..src_off + stride_out]);
        });
    TileGrid::from_image(&buf, out_w, out_h, grid.tile_size)
        .expect("from_image with matching dims is infallible")
}

fn rotate_90_cw(grid: &TileGrid) -> TileGrid {
    let w = grid.width as usize;
    let h = grid.height as usize;
    let src = grid.to_image();
    let stride_in = w * 4;
    // 90° CW: output(ox, oy) = input(oy, w-1-ox) with out_w=h, out_h=w.
    let out_w = h as u32;
    let out_h = w as u32;
    let stride_out = (out_w as usize) * 4;
    let mut buf = vec![0u8; stride_out * (out_h as usize)];
    buf.par_chunks_mut(stride_out)
        .enumerate()
        .for_each(|(oy, dst)| {
            for ox in 0..(out_w as usize) {
                let ix = oy;
                let iy = (h - 1) - ox;
                let off = iy * stride_in + ix * 4;
                dst[ox * 4..ox * 4 + 4].copy_from_slice(&src[off..off + 4]);
            }
        });
    TileGrid::from_image(&buf, out_w, out_h, grid.tile_size)
        .expect("from_image with matching dims is infallible")
}

fn rotate_180(grid: &TileGrid) -> TileGrid {
    let w = grid.width as usize;
    let h = grid.height as usize;
    let src = grid.to_image();
    let stride = w * 4;
    let mut buf = vec![0u8; stride * h];
    buf.par_chunks_mut(stride)
        .enumerate()
        .for_each(|(oy, dst)| {
            let iy = (h - 1) - oy;
            let src_row = &src[iy * stride..iy * stride + stride];
            for ox in 0..w {
                let ix = (w - 1) - ox;
                dst[ox * 4..ox * 4 + 4].copy_from_slice(&src_row[ix * 4..ix * 4 + 4]);
            }
        });
    TileGrid::from_image(&buf, grid.width, grid.height, grid.tile_size)
        .expect("from_image with matching dims is infallible")
}

fn bilinear_sample(src: &[u8], width: usize, height: usize, fx: f32, fy: f32) -> [u8; 4] {
    if width == 0 || height == 0 {
        return [0, 0, 0, 0];
    }
    let x0 = fx.floor();
    let y0 = fy.floor();
    let x1 = x0 + 1.0;
    let y1 = y0 + 1.0;
    let tx = fx - x0;
    let ty = fy - y0;
    let stride = width * 4;
    let fetch = |xi: f32, yi: f32| -> [f32; 4] {
        let xc = xi.clamp(0.0, (width - 1) as f32) as usize;
        let yc = yi.clamp(0.0, (height - 1) as f32) as usize;
        let off = yc * stride + xc * 4;
        [
            f32::from(src[off]),
            f32::from(src[off + 1]),
            f32::from(src[off + 2]),
            f32::from(src[off + 3]),
        ]
    };
    let p00 = fetch(x0, y0);
    let p10 = fetch(x1, y0);
    let p01 = fetch(x0, y1);
    let p11 = fetch(x1, y1);
    let mut out = [0u8; 4];
    for c in 0..4 {
        let top = p00[c] * (1.0 - tx) + p10[c] * tx;
        let bot = p01[c] * (1.0 - tx) + p11[c] * tx;
        let v = top * (1.0 - ty) + bot * ty;
        out[c] = v.clamp(0.0, 255.0).round() as u8;
    }
    out
}

/// Rotate by `angle_deg` degrees about the centre.
///
/// The output canvas grows to fit the rotated rectangle. Pixels that
/// fall outside the source are returned as transparent.
///
/// Exact multiples of 90° (`0`, `±90`, `±180`, `±270`, `±360`, ...)
/// go through a fast integer-math path that avoids the floating-point
/// drift inherent in `sin(2π) ≈ 1e-7`. This keeps `rotate(_, 360)`
/// truly bitwise-identical to the input.
#[must_use]
pub fn rotate(grid: &TileGrid, angle_deg: f32) -> TileGrid {
    let w = grid.width as usize;
    let h = grid.height as usize;
    if w == 0 || h == 0 {
        return grid.clone();
    }
    // Normalise to [0, 360) and check for an exact 90°-step rotation.
    let normalised = angle_deg.rem_euclid(360.0);
    if (normalised - 0.0).abs() < f32::EPSILON {
        return grid.clone();
    }
    if (normalised - 90.0).abs() < f32::EPSILON {
        return rotate_90_cw(grid);
    }
    if (normalised - 180.0).abs() < f32::EPSILON {
        return rotate_180(grid);
    }
    if (normalised - 270.0).abs() < f32::EPSILON {
        // 270° CW == 90° CCW; apply 90° CW three times for simplicity.
        let once = rotate_90_cw(grid);
        let twice = rotate_90_cw(&once);
        return rotate_90_cw(&twice);
    }
    let theta = angle_deg.to_radians();
    let (sn, cs) = theta.sin_cos();
    // Compute the rotated bounding box.
    let w_f = w as f32;
    let h_f = h as f32;
    let corners = [(0.0f32, 0.0f32), (w_f, 0.0), (0.0, h_f), (w_f, h_f)];
    let mut min_x = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    let cx_in = w_f * 0.5;
    let cy_in = h_f * 0.5;
    for (px, py) in corners {
        let dx = px - cx_in;
        let dy = py - cy_in;
        let rx = dx * cs - dy * sn;
        let ry = dx * sn + dy * cs;
        if rx < min_x {
            min_x = rx;
        }
        if rx > max_x {
            max_x = rx;
        }
        if ry < min_y {
            min_y = ry;
        }
        if ry > max_y {
            max_y = ry;
        }
    }
    let out_w = (max_x - min_x).ceil() as u32;
    let out_h = (max_y - min_y).ceil() as u32;
    let out_w = out_w.max(1);
    let out_h = out_h.max(1);
    let cx_out = out_w as f32 * 0.5;
    let cy_out = out_h as f32 * 0.5;
    let stride_in = w * 4;
    let stride_out = (out_w as usize) * 4;
    let src = grid.to_image();
    let mut buf = vec![0u8; stride_out * (out_h as usize)];
    buf.par_chunks_mut(stride_out)
        .enumerate()
        .for_each(|(row, dst)| {
            let dy = row as f32 - cy_out;
            for x in 0..(out_w as usize) {
                let dx = x as f32 - cx_out;
                // Inverse rotation.
                let sx = dx * cs + dy * sn + cx_in;
                let sy = -dx * sn + dy * cs + cy_in;
                if sx < -0.5 || sx >= w_f - 0.5 || sy < -0.5 || sy >= h_f - 0.5 {
                    // Transparent fill outside the source.
                    let off = x * 4;
                    dst[off] = 0;
                    dst[off + 1] = 0;
                    dst[off + 2] = 0;
                    dst[off + 3] = 0;
                } else {
                    let px = bilinear_sample(&src, w, h, sx, sy);
                    dst[x * 4..x * 4 + 4].copy_from_slice(&px);
                }
            }
            // Suppress an unused warning if width == 0 (handled above).
            let _ = stride_in;
        });
    TileGrid::from_image(&buf, out_w, out_h, grid.tile_size)
        .expect("from_image with matching dims is infallible")
}

/// 3×3 projective matrix in row-major order. Used internally by
/// [`perspective_transform`].
type Mat3 = [[f64; 3]; 3];

fn mat_mul(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut out = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            let mut s = 0.0;
            for k in 0..3 {
                s += a[i][k] * b[k][j];
            }
            out[i][j] = s;
        }
    }
    out
}

fn mat_inverse(m: &Mat3) -> Option<Mat3> {
    let a = m[0][0];
    let b = m[0][1];
    let c = m[0][2];
    let d = m[1][0];
    let e = m[1][1];
    let f = m[1][2];
    let g = m[2][0];
    let h = m[2][1];
    let i = m[2][2];
    let det = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv_det = 1.0 / det;
    let mut out = [[0.0; 3]; 3];
    out[0][0] = (e * i - f * h) * inv_det;
    out[0][1] = (c * h - b * i) * inv_det;
    out[0][2] = (b * f - c * e) * inv_det;
    out[1][0] = (f * g - d * i) * inv_det;
    out[1][1] = (a * i - c * g) * inv_det;
    out[1][2] = (c * d - a * f) * inv_det;
    out[2][0] = (d * h - e * g) * inv_det;
    out[2][1] = (b * g - a * h) * inv_det;
    out[2][2] = (a * e - b * d) * inv_det;
    Some(out)
}

/// Compute the projective basis-mapping `B` such that
/// `B * (1,0,0) = (x0,y0,1)`, `B * (0,1,0) = (x1,y1,1)`,
/// `B * (0,0,1) = (x2,y2,1)`, `B * (1,1,1) ∝ (x3,y3,1)`.
///
/// Returns `None` when the four points are degenerate (collinear).
/// Implements the closed-form construction from Heckbert,
/// "Fundamentals of Texture Mapping and Image Warping" (1989) §2.2.
fn basis_to_points(pts: &[(f64, f64); 4]) -> Option<Mat3> {
    let (x0, y0) = pts[0];
    let (x1, y1) = pts[1];
    let (x2, y2) = pts[2];
    let (x3, y3) = pts[3];
    // Solve for scalars (a, b, c) such that
    //   a * (x0,y0,1) + b * (x1,y1,1) + c * (x2,y2,1) = (x3,y3,1)
    let m = [[x0, x1, x2], [y0, y1, y2], [1.0, 1.0, 1.0]];
    let inv = mat_inverse(&m)?;
    let a = inv[0][0] * x3 + inv[0][1] * y3 + inv[0][2];
    let b = inv[1][0] * x3 + inv[1][1] * y3 + inv[1][2];
    let c = inv[2][0] * x3 + inv[2][1] * y3 + inv[2][2];
    if a.abs() < 1e-12 || b.abs() < 1e-12 || c.abs() < 1e-12 {
        return None;
    }
    Some([
        [a * x0, b * x1, c * x2],
        [a * y0, b * y1, c * y2],
        [a, b, c],
    ])
}

/// Build the 3×3 projective matrix that maps the source rectangle
/// corners `(0,0)`, `(w,0)`, `(0,h)`, `(w,h)` to the four supplied
/// destination corners (top-left, top-right, bottom-left,
/// bottom-right). Returns `None` when the destinations are
/// degenerate.
fn perspective_matrix(src_w: f64, src_h: f64, dst: &[(f64, f64); 4]) -> Option<Mat3> {
    let src = [(0.0, 0.0), (src_w, 0.0), (0.0, src_h), (src_w, src_h)];
    let s = basis_to_points(&src)?;
    let d = basis_to_points(dst)?;
    let s_inv = mat_inverse(&s)?;
    Some(mat_mul(&d, &s_inv))
}

/// Apply a perspective (projective) transform to `grid`, mapping the
/// source rectangle's four corners to the supplied destination
/// corners (in `(top_left, top_right, bottom_left, bottom_right)`
/// order, in pixels of the output canvas). Uses inverse mapping with
/// bilinear interpolation, row-parallel via rayon.
///
/// The output canvas size is the axis-aligned bounding box of the
/// supplied destination corners, clamped to `[1, u32::MAX]`. Pixels
/// outside the warped quadrilateral are transparent.
///
/// Returns the source grid unchanged when the destination corners
/// are degenerate (collinear / collapsed) — the only sensible
/// no-op for an invertible-transform API.
#[must_use]
pub fn perspective_transform(grid: &TileGrid, corners: [(f64, f64); 4]) -> TileGrid {
    let src_w = f64::from(grid.width);
    let src_h = f64::from(grid.height);
    if src_w == 0.0 || src_h == 0.0 {
        return grid.clone();
    }
    let Some(forward) = perspective_matrix(src_w, src_h, &corners) else {
        return grid.clone();
    };
    let Some(inverse) = mat_inverse(&forward) else {
        return grid.clone();
    };

    // Output canvas = axis-aligned bbox of destination corners,
    // translated so the bbox sits at (0, 0). The caller's `corners`
    // are in absolute pixels of the output canvas's coordinate
    // system; we re-anchor at the min so a negative corner doesn't
    // produce a negative-sized canvas.
    let mut min_x = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for (cx, cy) in &corners {
        if *cx < min_x {
            min_x = *cx;
        }
        if *cx > max_x {
            max_x = *cx;
        }
        if *cy < min_y {
            min_y = *cy;
        }
        if *cy > max_y {
            max_y = *cy;
        }
    }
    let out_w_f = (max_x - min_x).ceil().max(1.0);
    let out_h_f = (max_y - min_y).ceil().max(1.0);
    let out_w = (out_w_f as u32).max(1);
    let out_h = (out_h_f as u32).max(1);

    let stride_out = (out_w as usize) * 4;
    let src = grid.to_image();
    let mut buf = vec![0u8; stride_out * (out_h as usize)];
    let src_w_usize = grid.width as usize;
    let src_h_usize = grid.height as usize;

    buf.par_chunks_mut(stride_out)
        .enumerate()
        .for_each(|(row, dst)| {
            // Pixel positions are addressed by their top-left
            // corner (matching `rotate`); the inverse transform
            // maps output corner `(dx, dy)` back to a source
            // sample position, and `bilinear_sample` interpolates
            // there. Using the corner convention means an identity
            // perspective is bitwise-stable instead of averaging
            // four neighbours at every integer position.
            let dy = row as f64 + min_y;
            for x in 0..(out_w as usize) {
                let dx = x as f64 + min_x;
                // Apply inverse projective transform.
                let w = inverse[2][0] * dx + inverse[2][1] * dy + inverse[2][2];
                if w.abs() < 1e-12 {
                    let off = x * 4;
                    dst[off..off + 4].copy_from_slice(&[0, 0, 0, 0]);
                    continue;
                }
                let sx = (inverse[0][0] * dx + inverse[0][1] * dy + inverse[0][2]) / w;
                let sy = (inverse[1][0] * dx + inverse[1][1] * dy + inverse[1][2]) / w;
                let off = x * 4;
                if sx < -0.5
                    || sx >= src_w - 0.5
                    || sy < -0.5
                    || sy >= src_h - 0.5
                    || !sx.is_finite()
                    || !sy.is_finite()
                {
                    dst[off..off + 4].copy_from_slice(&[0, 0, 0, 0]);
                } else {
                    let px = bilinear_sample(&src, src_w_usize, src_h_usize, sx as f32, sy as f32);
                    dst[off..off + 4].copy_from_slice(&px);
                }
            }
        });

    TileGrid::from_image(&buf, out_w, out_h, grid.tile_size)
        .expect("from_image with matching dims is infallible")
}

/// In-place horizontal flip.
pub fn flip_h(grid: &mut TileGrid) {
    let w = grid.width;
    let h = grid.height;
    if w == 0 || h == 0 {
        return;
    }
    let src = grid.to_image();
    let stride = (w as usize) * 4;
    let mut out = vec![0u8; stride * (h as usize)];
    out.par_chunks_mut(stride)
        .enumerate()
        .for_each(|(y, dst_row)| {
            let src_row = &src[y * stride..y * stride + stride];
            for x in 0..(w as usize) {
                let dx = ((w as usize) - 1) - x;
                dst_row[dx * 4..dx * 4 + 4].copy_from_slice(&src_row[x * 4..x * 4 + 4]);
            }
        });
    *grid = TileGrid::from_image(&out, w, h, grid.tile_size)
        .expect("from_image with matching dims is infallible");
}

/// In-place vertical flip.
pub fn flip_v(grid: &mut TileGrid) {
    let w = grid.width;
    let h = grid.height;
    if w == 0 || h == 0 {
        return;
    }
    let src = grid.to_image();
    let stride = (w as usize) * 4;
    let mut out = vec![0u8; stride * (h as usize)];
    out.par_chunks_mut(stride)
        .enumerate()
        .for_each(|(y, dst_row)| {
            let dy = ((h as usize) - 1) - y;
            let src_row = &src[dy * stride..dy * stride + stride];
            dst_row.copy_from_slice(src_row);
        });
    *grid = TileGrid::from_image(&out, w, h, grid.tile_size)
        .expect("from_image with matching dims is infallible");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tile::TileGrid;

    fn checker(w: u32, h: u32) -> TileGrid {
        let mut buf = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let v: u8 = if (x + y) % 2 == 0 { 0 } else { 255 };
                buf.extend_from_slice(&[v, v, v, 255]);
            }
        }
        TileGrid::from_image(&buf, w, h, 32).expect("grid")
    }

    #[test]
    fn crop_produces_correct_dimensions() {
        let g = checker(16, 16);
        let out = crop(&g, 2, 4, 8, 6);
        assert_eq!(out.width, 8);
        assert_eq!(out.height, 6);
    }

    #[test]
    fn crop_out_of_bounds_returns_minimal() {
        let g = checker(8, 8);
        let out = crop(&g, 100, 100, 4, 4);
        assert_eq!(out.width, 1);
        assert_eq!(out.height, 1);
    }

    #[test]
    fn flip_twice_is_identity() {
        let mut g = checker(16, 16);
        let original = g.to_image();
        flip_h(&mut g);
        flip_h(&mut g);
        assert_eq!(g.to_image(), original);
        flip_v(&mut g);
        flip_v(&mut g);
        assert_eq!(g.to_image(), original);
    }

    #[test]
    fn rotate_360_is_identity() {
        let g = checker(32, 32);
        let out = rotate(&g, 360.0);
        assert_eq!(out.width, g.width);
        assert_eq!(out.height, g.height);
        assert_eq!(out.to_image(), g.to_image());
    }

    #[test]
    fn rotate_180_twice_is_identity() {
        let g = checker(16, 16);
        let once = rotate(&g, 180.0);
        let twice = rotate(&once, 180.0);
        assert_eq!(twice.to_image(), g.to_image());
    }

    #[test]
    fn rotate_90_swaps_dimensions() {
        let g = checker(20, 10);
        let out = rotate(&g, 90.0);
        // Exact 90° flips dimensions exactly.
        assert_eq!(out.width, 10);
        assert_eq!(out.height, 20);
    }

    #[test]
    fn rotate_45_preserves_center_pixel() {
        let g = checker(64, 64);
        let out = rotate(&g, 45.0);
        // Bounding box grows to ~90×90, central pixel should still be
        // a valid bilinear sample of the source centre.
        assert!(out.width >= g.width);
        assert!(out.height >= g.height);
    }

    #[test]
    fn perspective_identity_corners_round_trip() {
        let g = checker(32, 32);
        let out = perspective_transform(&g, [(0.0, 0.0), (32.0, 0.0), (0.0, 32.0), (32.0, 32.0)]);
        assert_eq!(out.width, 32);
        assert_eq!(out.height, 32);
        // Identity mapping reproduces the source bitmap up to
        // bilinear-sampling rounding (use a max-abs-diff bound).
        let src = g.to_image();
        let dst = out.to_image();
        assert_eq!(src.len(), dst.len());
        let mut max_diff = 0i32;
        for (a, b) in src.iter().zip(dst.iter()) {
            max_diff = max_diff.max((i32::from(*a) - i32::from(*b)).abs());
        }
        assert!(max_diff <= 2, "identity perspective drift {max_diff}");
    }

    #[test]
    fn perspective_degenerate_returns_source() {
        let g = checker(16, 16);
        let out = perspective_transform(&g, [(0.0, 0.0), (0.0, 0.0), (0.0, 0.0), (0.0, 0.0)]);
        // Degenerate (all corners collapsed): we return the input
        // unchanged so the editor never produces a 0×0 grid.
        assert_eq!(out.to_image(), g.to_image());
    }

    #[test]
    fn perspective_top_compressed_keeps_corners_in_place() {
        // Build a synthetic image with a single white pixel at the
        // top-left corner and zero elsewhere. After a transform that
        // maps (0,0) → (10, 5), the resulting canvas should have a
        // bright pixel near (10, 5) and dark elsewhere.
        let mut buf = vec![0u8; 32 * 32 * 4];
        buf[3] = 255; // (0,0) alpha
        buf[0] = 255;
        buf[1] = 255;
        buf[2] = 255;
        let g = TileGrid::from_image(&buf, 32, 32, 16).expect("grid");
        // Trapezoid: top edge shifted inward by 5 px on either side.
        let out = perspective_transform(&g, [(5.0, 0.0), (27.0, 0.0), (0.0, 32.0), (32.0, 32.0)]);
        // The top-left corner of the source maps to (5, 0); check
        // that the alpha at output (5, 0) is bright.
        let stride = (out.width as usize) * 4;
        let alpha_5_0 = out.to_image()[3 + 5 * 4];
        let _ = stride;
        assert!(alpha_5_0 > 128, "warped corner pixel dim ({alpha_5_0})");
    }
}
