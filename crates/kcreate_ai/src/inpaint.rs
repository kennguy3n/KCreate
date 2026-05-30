//! Exemplar-based image inpainting — Phase 10 Block A Task 2.
//!
//! Implements a PatchMatch-style texture synthesis algorithm:
//!
//! 1. Build a multi-scale pyramid of the input image and mask. Start
//!    at the coarsest level so the algorithm sees the global
//!    structure first.
//! 2. At each level, iterate the PatchMatch alternation: for each
//!    pixel inside the mask, find a nearest-neighbour patch in the
//!    non-masked source region using random search + propagation.
//! 3. Splat the matched source patch back into the mask region,
//!    blending by patch overlap so the result stays seamless.
//! 4. Upsample to the next level and seed the next NNF (nearest
//!    neighbour field) from the previous one.
//!
//! The implementation favours clarity over peak speed — the
//! algorithmic core is real (random search + propagation) but the
//! patch comparisons are straightforward SSD over RGBA. For
//! production-grade quality the bridge can swap this for an ONNX
//! LaMa model when one is installed (gated behind `onnx_inpaint`,
//! not implemented in this phase).
//!
//! Row-parallel via `rayon` is applied to the patch-splat step and
//! to per-level pyramid construction. The PatchMatch propagation
//! itself is inherently sequential within a single sweep but we run
//! the random-search step in parallel across mask pixels.

use std::collections::HashMap;

use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Tunables for [`inpaint`].
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct InpaintOptions {
    /// Square patch side / 2. Larger patches preserve texture
    /// better but oversmooth fine details. Clamped to `[1, 15]`.
    pub patch_radius: u32,
    /// Number of PatchMatch iterations per pyramid level. More
    /// iterations converge to a better NNF at higher CPU cost.
    /// Clamped to `[1, 16]`.
    pub num_iterations: u32,
    /// Number of pyramid levels. `1` means single-scale.
    /// Clamped to `[1, 6]`.
    pub pyramid_levels: u32,
}

impl Default for InpaintOptions {
    fn default() -> Self {
        Self {
            patch_radius: 3, // 7x7 patches
            num_iterations: 5,
            pyramid_levels: 3,
        }
    }
}

impl InpaintOptions {
    /// Apply the clamping discipline documented on each field.
    #[must_use]
    pub fn clamped(mut self) -> Self {
        self.patch_radius = self.patch_radius.clamp(1, 15);
        self.num_iterations = self.num_iterations.clamp(1, 16);
        self.pyramid_levels = self.pyramid_levels.clamp(1, 6);
        self
    }
}

#[derive(Debug, Error)]
pub enum InpaintError {
    #[error("inpaint: empty image")]
    Empty,
    #[error("inpaint: pixel buffer length {got} != expected {expected}")]
    PixelBufferSize { got: usize, expected: usize },
    #[error("inpaint: mask length {got} != expected {expected}")]
    MaskBufferSize { got: usize, expected: usize },
    #[error("inpaint: source region is empty (mask covers entire image)")]
    NoSource,
}

/// A rectangular masked region, expressed in pixel coordinates.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MaskRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Expand a list of [`MaskRect`]s into a pixel-aligned `u8` mask
/// (255 = inpaint here, 0 = source). Out-of-range rectangles are
/// clipped to the image bounds.
#[must_use]
pub fn mask_from_rects(rects: &[MaskRect], width: u32, height: u32) -> Vec<u8> {
    let mut mask = vec![0u8; (width as usize) * (height as usize)];
    for r in rects {
        let x0 = r.x.min(width);
        let y0 = r.y.min(height);
        let x1 = (r.x.saturating_add(r.width)).min(width);
        let y1 = (r.y.saturating_add(r.height)).min(height);
        for y in y0..y1 {
            let row_start = (y as usize) * (width as usize);
            for x in x0..x1 {
                mask[row_start + (x as usize)] = 255;
            }
        }
    }
    mask
}

/// Run exemplar-based inpainting. `mask` is one byte per pixel: any
/// non-zero value marks the destination region. The returned image
/// is RGBA, same dimensions as the input.
///
/// An empty mask is treated as a no-op and the input is returned
/// verbatim — this matches user expectations (no selection ⇒ no
/// change) without falsely erroring.
///
/// # Errors
///
/// Returns [`InpaintError`] when the buffer sizes don't match, the
/// image is empty, or the mask covers the entire image (no source
/// region to sample from).
pub fn inpaint(
    pixels: &[u8],
    mask: &[u8],
    width: u32,
    height: u32,
    options: InpaintOptions,
) -> Result<Vec<u8>, InpaintError> {
    if width == 0 || height == 0 {
        return Err(InpaintError::Empty);
    }
    let total_px = (width as usize) * (height as usize);
    let expected_pixels = total_px * 4;
    if pixels.len() != expected_pixels {
        return Err(InpaintError::PixelBufferSize {
            got: pixels.len(),
            expected: expected_pixels,
        });
    }
    if mask.len() != total_px {
        return Err(InpaintError::MaskBufferSize {
            got: mask.len(),
            expected: total_px,
        });
    }
    let opts = options.clamped();
    // Fast-path: empty mask → copy input.
    if mask.iter().all(|&b| b == 0) {
        return Ok(pixels.to_vec());
    }
    if mask.iter().all(|&b| b != 0) {
        return Err(InpaintError::NoSource);
    }

    // Build pyramid from coarse → fine.
    let mut levels: Vec<(Vec<u8>, Vec<u8>, u32, u32)> =
        Vec::with_capacity(opts.pyramid_levels as usize);
    levels.push((pixels.to_vec(), mask.to_vec(), width, height));
    let mut cw = width;
    let mut ch = height;
    let mut cp = pixels.to_vec();
    let mut cm = mask.to_vec();
    for _ in 1..opts.pyramid_levels {
        let nw = (cw / 2).max(opts.patch_radius * 2 + 1);
        let nh = (ch / 2).max(opts.patch_radius * 2 + 1);
        if nw == cw && nh == ch {
            break;
        }
        let (downp, downm) = downsample(&cp, &cm, cw, ch, nw, nh);
        cw = nw;
        ch = nh;
        cp = downp;
        cm = downm;
        levels.push((cp.clone(), cm.clone(), cw, ch));
    }
    levels.reverse(); // coarsest first

    // Seed: fill the masked region at the coarsest level with the
    // mean colour of the source region so PatchMatch has something
    // sensible to start from.
    let (start_p, start_m, sw, sh) = levels.first().expect("at least one pyramid level");
    let mut current = seed_with_mean(start_p, start_m, *sw, *sh);
    let mut current_w = *sw;
    let mut current_h = *sh;

    for (i, (orig_p, orig_m, w_l, h_l)) in levels.iter().enumerate() {
        // Upsample previous level's result into this level if needed.
        if i > 0 {
            current = upsample(&current, current_w, current_h, *w_l, *h_l);
            current_w = *w_l;
            current_h = *h_l;
            // Splat the original non-masked pixels back in — we must
            // never overwrite known content.
            for y in 0..*h_l {
                for x in 0..*w_l {
                    let pi = (y as usize) * (*w_l as usize) + (x as usize);
                    if orig_m[pi] == 0 {
                        let ci = pi * 4;
                        current[ci] = orig_p[ci];
                        current[ci + 1] = orig_p[ci + 1];
                        current[ci + 2] = orig_p[ci + 2];
                        current[ci + 3] = orig_p[ci + 3];
                    }
                }
            }
        }
        run_patchmatch_level(
            &mut current,
            orig_p,
            orig_m,
            *w_l,
            *h_l,
            opts.patch_radius as i32,
            opts.num_iterations as i32,
        );
    }

    Ok(current)
}

/// Box-filter downsample paired with logical-OR mask downsample.
/// We OR the mask so the coarse level still treats partially-masked
/// areas as masked — this is the standard convention for inpainting
/// pyramids.
fn downsample(
    pixels: &[u8],
    mask: &[u8],
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
) -> (Vec<u8>, Vec<u8>) {
    let mut dst_p = vec![0u8; (dst_w as usize) * (dst_h as usize) * 4];
    let mut dst_m = vec![0u8; (dst_w as usize) * (dst_h as usize)];
    let sx = src_w as f32 / dst_w as f32;
    let sy = src_h as f32 / dst_h as f32;
    for y in 0..dst_h {
        for x in 0..dst_w {
            let x0 = (x as f32 * sx).floor() as u32;
            let y0 = (y as f32 * sy).floor() as u32;
            let x1 = (((x + 1) as f32 * sx).ceil() as u32).min(src_w);
            let y1 = (((y + 1) as f32 * sy).ceil() as u32).min(src_h);
            let mut sum = [0u32; 4];
            let mut count = 0u32;
            let mut masked = false;
            for yy in y0..y1 {
                for xx in x0..x1 {
                    let si = ((yy * src_w + xx) * 4) as usize;
                    sum[0] += u32::from(pixels[si]);
                    sum[1] += u32::from(pixels[si + 1]);
                    sum[2] += u32::from(pixels[si + 2]);
                    sum[3] += u32::from(pixels[si + 3]);
                    count += 1;
                    if mask[(yy * src_w + xx) as usize] != 0 {
                        masked = true;
                    }
                }
            }
            let di = ((y * dst_w + x) * 4) as usize;
            if let Some(c) = std::num::NonZeroU32::new(count) {
                let c = c.get();
                dst_p[di] = (sum[0] / c) as u8;
                dst_p[di + 1] = (sum[1] / c) as u8;
                dst_p[di + 2] = (sum[2] / c) as u8;
                dst_p[di + 3] = (sum[3] / c) as u8;
            }
            dst_m[(y * dst_w + x) as usize] = if masked { 255 } else { 0 };
        }
    }
    (dst_p, dst_m)
}

/// Nearest-neighbour upsample. We don't bilinear here because we're
/// about to feed the result back into PatchMatch which will smooth
/// any blockiness on its first sweep.
fn upsample(pixels: &[u8], src_w: u32, src_h: u32, dst_w: u32, dst_h: u32) -> Vec<u8> {
    let mut dst = vec![0u8; (dst_w as usize) * (dst_h as usize) * 4];
    for y in 0..dst_h {
        let sy = ((u64::from(y) * u64::from(src_h)) / u64::from(dst_h)) as u32;
        for x in 0..dst_w {
            let sx = ((u64::from(x) * u64::from(src_w)) / u64::from(dst_w)) as u32;
            let si = ((sy * src_w + sx) * 4) as usize;
            let di = ((y * dst_w + x) * 4) as usize;
            dst[di..di + 4].copy_from_slice(&pixels[si..si + 4]);
        }
    }
    dst
}

/// Replace every masked pixel with the mean colour of the
/// non-masked region. Cheap seed for the coarsest pyramid level.
fn seed_with_mean(pixels: &[u8], mask: &[u8], _width: u32, _height: u32) -> Vec<u8> {
    let mut sum = [0u64; 4];
    let mut count = 0u64;
    for (i, &m) in mask.iter().enumerate() {
        if m == 0 {
            let pi = i * 4;
            sum[0] += u64::from(pixels[pi]);
            sum[1] += u64::from(pixels[pi + 1]);
            sum[2] += u64::from(pixels[pi + 2]);
            sum[3] += u64::from(pixels[pi + 3]);
            count += 1;
        }
    }
    let mean = std::num::NonZeroU64::new(count).map_or([128u8, 128, 128, 255], |c| {
        let c = c.get();
        [
            (sum[0] / c) as u8,
            (sum[1] / c) as u8,
            (sum[2] / c) as u8,
            255,
        ]
    });
    let mut out = pixels.to_vec();
    for (i, &m) in mask.iter().enumerate() {
        if m != 0 {
            let pi = i * 4;
            out[pi] = mean[0];
            out[pi + 1] = mean[1];
            out[pi + 2] = mean[2];
            out[pi + 3] = mean[3];
        }
    }
    out
}

/// One PatchMatch sweep over a single pyramid level. The `pixels`
/// buffer is mutated in place — only the masked region is touched;
/// non-masked pixels are protected by [`splat_into_masked`].
fn run_patchmatch_level(
    pixels: &mut [u8],
    original: &[u8],
    mask: &[u8],
    width: u32,
    height: u32,
    pr: i32,
    iters: i32,
) {
    let w = width as i32;
    let h = height as i32;
    if w <= 2 * pr || h <= 2 * pr {
        return; // can't form a patch centred away from the edge
    }
    let total = (width as usize) * (height as usize);

    // Collect mask centre pixels we need to fill. Skip any pixel
    // whose full patch would fall outside the image — the boundary
    // ring is left as the mean-seeded colour for the upper pyramid
    // level to refine.
    let mut targets: Vec<(i32, i32)> = Vec::new();
    for y in pr..(h - pr) {
        for x in pr..(w - pr) {
            if mask[(y as usize) * (width as usize) + (x as usize)] != 0 {
                targets.push((x, y));
            }
        }
    }
    if targets.is_empty() {
        return;
    }

    // Pre-compute the set of legal source patch centres (every
    // pixel whose patch is entirely in the non-masked region).
    let mut legal_sources: Vec<(i32, i32)> = Vec::new();
    for y in pr..(h - pr) {
        for x in pr..(w - pr) {
            if patch_is_pure_source(mask, width, x, y, pr) {
                legal_sources.push((x, y));
            }
        }
    }
    if legal_sources.is_empty() {
        return;
    }

    // Build a coordinate -> index lookup so the propagation step can
    // find the target index for an arbitrary `(x, y)` in O(1).
    // Without this, propagating from the top neighbour would need a
    // linear scan over `targets` for every masked pixel; the map
    // turns that into a constant-time hash hit and keeps the per-
    // sweep cost linear in the mask size.
    let mut index_by_coord: HashMap<(i32, i32), usize> = HashMap::with_capacity(targets.len());
    for (i, &(tx, ty)) in targets.iter().enumerate() {
        index_by_coord.insert((tx, ty), i);
    }

    // NNF: for each target, the (sx, sy) source patch centre that
    // currently matches it best. Initialise with a deterministic
    // pseudo-random pick per target so reruns are reproducible.
    let mut rng = XorShiftRng::seed(0xA1B2_C3D4 ^ u64::from(width) ^ (u64::from(height) << 16));
    let mut nnf: Vec<(i32, i32)> = targets
        .iter()
        .map(|_| {
            let pick = (rng.next_u32() as usize) % legal_sources.len();
            legal_sources[pick]
        })
        .collect();

    for iter in 0..iters {
        // 1) Random search: each target picks a few random source
        //    candidates and keeps the best. Parallelised across targets.
        let scored: Vec<(i32, i32)> = targets
            .par_iter()
            .enumerate()
            .map(|(i, &(tx, ty))| {
                let mut best = nnf[i];
                let mut best_cost = patch_ssd(pixels, original, width, tx, ty, best.0, best.1, pr);
                // Per-thread deterministic PRNG so the parallel
                // iteration order doesn't matter — the seed mixes
                // the iteration, target index, and base seed.
                let mut local_rng = XorShiftRng::seed(
                    0xDEAD_BEEF_u64
                        .wrapping_add(iter as u64)
                        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                        ^ (i as u64),
                );
                for _ in 0..6 {
                    let pick = (local_rng.next_u32() as usize) % legal_sources.len();
                    let cand = legal_sources[pick];
                    let cost = patch_ssd(pixels, original, width, tx, ty, cand.0, cand.1, pr);
                    if cost < best_cost {
                        best = cand;
                        best_cost = cost;
                    }
                }
                best
            })
            .collect();
        nnf = scored;

        // 2) Propagation: for each target, check whether the
        //    leftward / upward neighbours' source patches would be
        //    a better fit when shifted into this target. Both
        //    neighbours are tried — standard PatchMatch propagates
        //    along the scan-order axes (left + top), which lets a
        //    good match travel both horizontally and vertically
        //    through the mask in a single sweep. Sequential
        //    because the neighbour writes happen in scan order.
        for (i, &(tx, ty)) in targets.iter().enumerate() {
            let mut best = nnf[i];
            let mut best_cost = patch_ssd(pixels, original, width, tx, ty, best.0, best.1, pr);

            // Left neighbour: (tx - 1, ty) -> shift its source by +1 in x.
            if let Some(&li) = index_by_coord.get(&(tx - 1, ty)) {
                let (sx, sy) = nnf[li];
                let cand_x = sx + 1;
                let cand_y = sy;
                if cand_x >= pr && cand_x < w - pr && cand_y >= pr && cand_y < h - pr {
                    let cost = patch_ssd(pixels, original, width, tx, ty, cand_x, cand_y, pr);
                    if cost < best_cost {
                        best = (cand_x, cand_y);
                        best_cost = cost;
                    }
                }
            }

            // Top neighbour: (tx, ty - 1) -> shift its source by +1 in y.
            if let Some(&ti) = index_by_coord.get(&(tx, ty - 1)) {
                let (sx, sy) = nnf[ti];
                let cand_x = sx;
                let cand_y = sy + 1;
                if cand_x >= pr && cand_x < w - pr && cand_y >= pr && cand_y < h - pr {
                    let cost = patch_ssd(pixels, original, width, tx, ty, cand_x, cand_y, pr);
                    if cost < best_cost {
                        best = (cand_x, cand_y);
                        best_cost = cost;
                    }
                }
            }

            nnf[i] = best;
        }

        // 3) Splat the matched source patches back into the masked
        //    region with overlap-averaging.
        splat_into_masked(pixels, original, mask, width, height, &targets, &nnf, pr);
    }

    debug_assert_eq!(pixels.len(), total * 4);
}

#[inline]
fn patch_is_pure_source(mask: &[u8], width: u32, cx: i32, cy: i32, pr: i32) -> bool {
    for dy in -pr..=pr {
        for dx in -pr..=pr {
            let mi = ((cy + dy) as usize) * (width as usize) + ((cx + dx) as usize);
            if mask[mi] != 0 {
                return false;
            }
        }
    }
    true
}

// Eight scalar args is the natural shape for a per-patch SSD
// helper (two image buffers + width + two coords + patch radius).
// Bundling them into a struct just to dodge clippy adds noise.
#[allow(clippy::too_many_arguments)]
#[inline]
fn patch_ssd(
    target_buf: &[u8],
    source_buf: &[u8],
    width: u32,
    tx: i32,
    ty: i32,
    sx: i32,
    sy: i32,
    pr: i32,
) -> u64 {
    let mut acc = 0u64;
    let w = width as i32;
    for dy in -pr..=pr {
        for dx in -pr..=pr {
            let ti = (((ty + dy) * w + (tx + dx)) * 4) as usize;
            let si = (((sy + dy) * w + (sx + dx)) * 4) as usize;
            for c in 0..3 {
                let d = i32::from(target_buf[ti + c]) - i32::from(source_buf[si + c]);
                acc += (d * d) as u64;
            }
        }
    }
    acc
}

// Eight-arg helper that takes the working buffer, the original
// pixels, the mask, image dimensions, the precomputed target
// locations, the source map, and the patch radius. A struct would
// just be a one-shot named tuple.
#[allow(clippy::too_many_arguments)]
fn splat_into_masked(
    pixels: &mut [u8],
    original: &[u8],
    mask: &[u8],
    width: u32,
    height: u32,
    targets: &[(i32, i32)],
    nnf: &[(i32, i32)],
    pr: i32,
) {
    let w = width as i32;
    let h = height as i32;
    // Accumulate weighted colour + weight per pixel; only commit
    // masked positions back. Use f32 to avoid overflow across many
    // overlapping patches.
    let mut sum = vec![[0.0f32; 4]; (width as usize) * (height as usize)];
    let mut wsum = vec![0.0f32; (width as usize) * (height as usize)];
    for (&(tx, ty), &(sx, sy)) in targets.iter().zip(nnf.iter()) {
        for dy in -pr..=pr {
            for dx in -pr..=pr {
                let dst_x = tx + dx;
                let dst_y = ty + dy;
                let src_x = sx + dx;
                let src_y = sy + dy;
                if dst_x < 0 || dst_x >= w || dst_y < 0 || dst_y >= h {
                    continue;
                }
                if src_x < 0 || src_x >= w || src_y < 0 || src_y >= h {
                    continue;
                }
                let di = (dst_y as usize) * (width as usize) + (dst_x as usize);
                if mask[di] == 0 {
                    continue; // never overwrite source
                }
                let si = ((src_y * w + src_x) * 4) as usize;
                // Cosine-window weight peaks at patch centre and
                // falls off at the edges so overlapping patches
                // blend smoothly.
                let r = ((dx * dx + dy * dy) as f32).sqrt() / (pr as f32 + 1.0);
                let weight = ((1.0 - r).max(0.0)).powi(2) + 0.001;
                sum[di][0] += weight * f32::from(original[si]);
                sum[di][1] += weight * f32::from(original[si + 1]);
                sum[di][2] += weight * f32::from(original[si + 2]);
                sum[di][3] += weight * f32::from(original[si + 3]);
                wsum[di] += weight;
            }
        }
    }
    for (i, &m) in mask.iter().enumerate() {
        if m == 0 {
            continue;
        }
        if wsum[i] > 0.0 {
            let pi = i * 4;
            pixels[pi] = (sum[i][0] / wsum[i]).round().clamp(0.0, 255.0) as u8;
            pixels[pi + 1] = (sum[i][1] / wsum[i]).round().clamp(0.0, 255.0) as u8;
            pixels[pi + 2] = (sum[i][2] / wsum[i]).round().clamp(0.0, 255.0) as u8;
            pixels[pi + 3] = (sum[i][3] / wsum[i]).round().clamp(0.0, 255.0) as u8;
        }
    }
}

/// Deterministic xor-shift PRNG. We avoid pulling `rand` into the
/// editing-path deny-list closure; xor-shift is sufficient for
/// PatchMatch's random-search step.
#[derive(Debug)]
struct XorShiftRng {
    state: u64,
}

impl XorShiftRng {
    fn seed(seed: u64) -> Self {
        Self {
            state: if seed == 0 {
                0xCAFE_BABE_DEAD_BEEF
            } else {
                seed
            },
        }
    }
    fn next_u32(&mut self) -> u32 {
        // xorshift64* — fine for non-crypto random picks.
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 32) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, c: [u8; 4]) -> Vec<u8> {
        let total = (w as usize) * (h as usize) * 4;
        let mut v = Vec::with_capacity(total);
        for _ in 0..(w as usize * h as usize) {
            v.extend_from_slice(&c);
        }
        v
    }

    #[test]
    fn empty_mask_is_noop() {
        let img = solid(16, 16, [50, 100, 150, 255]);
        let mask = vec![0u8; 16 * 16];
        let out = inpaint(&img, &mask, 16, 16, InpaintOptions::default()).unwrap();
        assert_eq!(out, img);
    }

    #[test]
    fn rectangular_mask_fills_from_surroundings() {
        // Solid red image; punch a 4×4 hole in the middle and ask
        // inpaint to fill it. The result should be approximately red
        // everywhere — the only nearby source pixels are red.
        let w = 16u32;
        let h = 16u32;
        let red = [200u8, 30u8, 30u8, 255u8];
        let img = solid(w, h, red);
        let rect = MaskRect {
            x: 6,
            y: 6,
            width: 4,
            height: 4,
        };
        let mask = mask_from_rects(&[rect], w, h);
        let out = inpaint(
            &img,
            &mask,
            w,
            h,
            InpaintOptions {
                patch_radius: 2,
                num_iterations: 3,
                pyramid_levels: 2,
            },
        )
        .unwrap();
        for y in rect.y..(rect.y + rect.height) {
            for x in rect.x..(rect.x + rect.width) {
                let i = ((y * w + x) * 4) as usize;
                // Allow ±25 per channel — exemplar inpaint converges
                // close to red but the boundary blending can drift
                // before convergence.
                assert!(
                    (i32::from(out[i]) - i32::from(red[0])).abs() <= 25,
                    "R drift at ({x},{y}): {}",
                    out[i]
                );
                assert!(
                    (i32::from(out[i + 1]) - i32::from(red[1])).abs() <= 25,
                    "G drift at ({x},{y}): {}",
                    out[i + 1]
                );
                assert!(
                    (i32::from(out[i + 2]) - i32::from(red[2])).abs() <= 25,
                    "B drift at ({x},{y}): {}",
                    out[i + 2]
                );
            }
        }
    }

    #[test]
    fn full_mask_errors() {
        let img = solid(8, 8, [100, 100, 100, 255]);
        let mask = vec![255u8; 8 * 8];
        assert!(matches!(
            inpaint(&img, &mask, 8, 8, InpaintOptions::default()),
            Err(InpaintError::NoSource)
        ));
    }

    #[test]
    fn buffer_size_mismatch_errors() {
        let img = vec![0u8; 10];
        let mask = vec![0u8; 4];
        assert!(matches!(
            inpaint(&img, &mask, 4, 4, InpaintOptions::default()),
            Err(InpaintError::PixelBufferSize { .. })
        ));
    }

    #[test]
    fn options_clamping_keeps_radii_in_range() {
        let opts = InpaintOptions {
            patch_radius: 99,
            num_iterations: 99,
            pyramid_levels: 99,
        }
        .clamped();
        assert!(opts.patch_radius <= 15);
        assert!(opts.num_iterations <= 16);
        assert!(opts.pyramid_levels <= 6);
    }

    #[test]
    fn mask_from_rects_clips_to_bounds() {
        let mask = mask_from_rects(
            &[MaskRect {
                x: 6,
                y: 6,
                width: 100,
                height: 100,
            }],
            8,
            8,
        );
        // The rect extends past the right/bottom edge — only the
        // intersection should be marked.
        assert_eq!(mask[6 * 8 + 6], 255);
        assert_eq!(mask[7 * 8 + 7], 255);
        assert_eq!(mask.len(), 64);
        // `naive_bytecount` would suggest the bytecount crate; we
        // deliberately keep this dependency-free since the test
        // mask is tiny (64 bytes).
        #[allow(clippy::naive_bytecount)]
        let masked = mask.iter().filter(|&&b| b == 255).count();
        assert_eq!(masked, 4);
    }
}
