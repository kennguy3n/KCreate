//! Smart-select (magic wand) — BFS flood fill in RGB tolerance space.
//!
//! Returns a single-channel mask the same size as the input where
//! `255` means "selected" and `0` means "not selected".
//!
//! Tolerance is normalised to `[0.0, 1.0]`. A tolerance of `0.0` only
//! matches pixels with the exact same RGB as the seed; `1.0` matches
//! every pixel. Distance is computed in Euclidean RGB space and
//! normalised by `sqrt(3 * 255^2)` so the unit is intuitive.

use std::collections::VecDeque;

/// Maximum permitted RGB distance (sqrt(3 * 255^2)).
const MAX_RGB_DIST: f64 = 441.672_955_930_064_3; // sqrt(3) * 255

/// Run a tolerance-bounded BFS flood-fill from `(seed_x, seed_y)`.
///
/// Returns a `width * height` mask. Out-of-range seeds, empty inputs,
/// fully-transparent seeds, and buffer-size mismatches yield a
/// mask of all-zero bytes (sized `width * height` so the caller never
/// has to special-case the empty result).
#[must_use]
pub fn smart_select(
    pixels: &[u8],
    width: u32,
    height: u32,
    seed_x: u32,
    seed_y: u32,
    tolerance: f64,
) -> Vec<u8> {
    let total = (width as usize) * (height as usize);
    if total == 0 || pixels.len() != total * 4 {
        return vec![0u8; total];
    }
    if seed_x >= width || seed_y >= height {
        return vec![0u8; total];
    }
    let tol = tolerance.clamp(0.0, 1.0);
    let seed_idx = ((seed_y as usize) * (width as usize) + seed_x as usize) * 4;
    if pixels[seed_idx + 3] == 0 {
        return vec![0u8; total];
    }
    let seed = [
        pixels[seed_idx],
        pixels[seed_idx + 1],
        pixels[seed_idx + 2],
    ];
    let max_dist = tol * MAX_RGB_DIST;

    let mut mask = vec![0u8; total];
    let start_idx = (seed_y as usize) * (width as usize) + seed_x as usize;
    mask[start_idx] = 255;
    let mut queue: VecDeque<(u32, u32)> = VecDeque::new();
    queue.push_back((seed_x, seed_y));

    while let Some((x, y)) = queue.pop_front() {
        for (nx, ny) in neighbours(x, y, width, height) {
            let idx = (ny as usize) * (width as usize) + nx as usize;
            if mask[idx] != 0 {
                continue;
            }
            let px = idx * 4;
            if pixels[px + 3] == 0 {
                continue;
            }
            let candidate = [pixels[px], pixels[px + 1], pixels[px + 2]];
            if rgb_distance(seed, candidate) <= max_dist {
                mask[idx] = 255;
                queue.push_back((nx, ny));
            }
        }
    }
    mask
}

fn neighbours(x: u32, y: u32, w: u32, h: u32) -> impl Iterator<Item = (u32, u32)> {
    let mut buf: [Option<(u32, u32)>; 4] = [None; 4];
    if x > 0 {
        buf[0] = Some((x - 1, y));
    }
    if x + 1 < w {
        buf[1] = Some((x + 1, y));
    }
    if y > 0 {
        buf[2] = Some((x, y - 1));
    }
    if y + 1 < h {
        buf[3] = Some((x, y + 1));
    }
    buf.into_iter().flatten()
}

fn rgb_distance(a: [u8; 3], b: [u8; 3]) -> f64 {
    let dr = f64::from(a[0]) - f64::from(b[0]);
    let dg = f64::from(a[1]) - f64::from(b[1]);
    let db = f64::from(a[2]) - f64::from(b[2]);
    (dr * dr + dg * dg + db * db).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, c: [u8; 4]) -> Vec<u8> {
        let mut v = Vec::with_capacity((w as usize) * (h as usize) * 4);
        for _ in 0..(w as usize) * (h as usize) {
            v.extend_from_slice(&c);
        }
        v
    }

    #[test]
    fn empty_input_returns_empty_mask() {
        let m = smart_select(&[], 0, 0, 0, 0, 0.5);
        assert!(m.is_empty());
    }

    #[test]
    fn out_of_range_seed_returns_all_zero_mask() {
        let pixels = solid(4, 4, [255, 0, 0, 255]);
        let m = smart_select(&pixels, 4, 4, 10, 10, 0.5);
        assert_eq!(m.len(), 16);
        assert!(m.iter().all(|&b| b == 0));
    }

    #[test]
    fn solid_region_selects_everything() {
        let pixels = solid(4, 4, [200, 100, 50, 255]);
        let m = smart_select(&pixels, 4, 4, 0, 0, 0.1);
        assert_eq!(count_selected(&m), 16);
    }

    #[allow(clippy::naive_bytecount)] // test-only helper, no perf concern
    fn count_selected(mask: &[u8]) -> usize {
        mask.iter().filter(|&&b| b == 255).count()
    }

    #[test]
    fn zero_tolerance_only_selects_seed_when_neighbours_differ() {
        // Build a 4x1 image: black, red, red, red. Seed at the black
        // pixel with tolerance 0 -> only the black pixel selected.
        let mut pixels = Vec::with_capacity(16);
        pixels.extend_from_slice(&[0, 0, 0, 255]);
        pixels.extend_from_slice(&[255, 0, 0, 255]);
        pixels.extend_from_slice(&[255, 0, 0, 255]);
        pixels.extend_from_slice(&[255, 0, 0, 255]);
        let m = smart_select(&pixels, 4, 1, 0, 0, 0.0);
        assert_eq!(m[0], 255);
        assert_eq!(m[1], 0);
        assert_eq!(m[2], 0);
        assert_eq!(m[3], 0);
    }

    #[test]
    fn tolerance_grows_selection_through_gradient() {
        // 8x1 image with gradient red 0..224 step 32.
        let mut pixels = Vec::with_capacity(32);
        for i in 0..8u8 {
            pixels.extend_from_slice(&[i * 32, 0, 0, 255]);
        }
        // Low tolerance should pick up only seed + immediate.
        let m_low = smart_select(&pixels, 8, 1, 0, 0, 0.08);
        let selected_low = count_selected(&m_low);
        // High tolerance picks up the whole gradient.
        let m_high = smart_select(&pixels, 8, 1, 0, 0, 1.0);
        let selected_high = count_selected(&m_high);
        assert!(selected_high > selected_low);
        assert_eq!(selected_high, 8);
    }

    #[test]
    fn does_not_bleed_across_color_boundary() {
        // 4x1: red red blue blue. Seed on a red pixel, tolerance low
        // enough to reject blue -> mask is 1,1,0,0.
        let mut pixels = Vec::with_capacity(16);
        pixels.extend_from_slice(&[255, 0, 0, 255]);
        pixels.extend_from_slice(&[255, 0, 0, 255]);
        pixels.extend_from_slice(&[0, 0, 255, 255]);
        pixels.extend_from_slice(&[0, 0, 255, 255]);
        let m = smart_select(&pixels, 4, 1, 0, 0, 0.1);
        assert_eq!(m, vec![255, 255, 0, 0]);
    }

    #[test]
    fn transparent_seed_returns_zero_mask() {
        let pixels = solid(2, 2, [255, 0, 0, 0]);
        let m = smart_select(&pixels, 2, 2, 0, 0, 1.0);
        assert!(m.iter().all(|&b| b == 0));
    }

    #[test]
    fn transparent_neighbour_blocks_propagation() {
        // 3x1: red transparent red. Seed at left red, tolerance any
        // value — middle pixel is transparent so the right red is
        // unreachable.
        let mut pixels = Vec::with_capacity(12);
        pixels.extend_from_slice(&[255, 0, 0, 255]);
        pixels.extend_from_slice(&[255, 0, 0, 0]);
        pixels.extend_from_slice(&[255, 0, 0, 255]);
        let m = smart_select(&pixels, 3, 1, 0, 0, 0.5);
        assert_eq!(m, vec![255, 0, 0]);
    }

    #[test]
    fn seed_at_image_boundary_works() {
        let pixels = solid(4, 4, [255, 255, 255, 255]);
        let m = smart_select(&pixels, 4, 4, 3, 3, 0.1);
        assert_eq!(count_selected(&m), 16);
    }
}
