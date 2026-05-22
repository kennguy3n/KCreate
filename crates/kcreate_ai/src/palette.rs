//! Dominant-color palette extraction via k-means clustering in RGB.
//!
//! Real algorithm, not a stub. Used by the design-token panel and the
//! AI tasks bridge.
//!
//! Pipeline:
//! 1. Downsample to <=256x256 by stride sampling to keep wall-clock
//!    bounded on large images.
//! 2. Drop fully-transparent pixels (alpha == 0) — they do not
//!    contribute meaningful colour.
//! 3. Run Lloyd's k-means with `max_colors` centroids, deterministic
//!    initialisation (every Nth quantile of the sample set), and a
//!    fixed iteration cap (20).
//! 4. Return centroids sorted by cluster size (most dominant first),
//!    with a frequency in `[0.0, 1.0]`.

use serde::{Deserialize, Serialize};

const MAX_KMEANS_ITERATIONS: usize = 20;
const SAMPLE_DIM_CAP: u32 = 256;

/// A single extracted dominant color.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[allow(clippy::derive_partial_eq_without_eq)]
pub struct ExtractedColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub hex: String,
    /// Fraction of sampled non-transparent pixels in this cluster
    /// (`0.0` .. `1.0`).
    pub frequency: f32,
}

impl ExtractedColor {
    fn from_rgb(r: u8, g: u8, b: u8, frequency: f32) -> Self {
        Self {
            r,
            g,
            b,
            hex: format!("#{r:02X}{g:02X}{b:02X}"),
            frequency,
        }
    }
}

/// Extract up to `max_colors` dominant colors from an RGBA8 image.
///
/// Empty inputs and inputs with no opaque pixels return an empty
/// vector. `max_colors == 0` is treated as "no colours requested" and
/// also returns empty.
#[must_use]
pub fn extract_palette(
    pixels: &[u8],
    width: u32,
    height: u32,
    max_colors: usize,
) -> Vec<ExtractedColor> {
    if width == 0
        || height == 0
        || max_colors == 0
        || pixels.len() != (width as usize) * (height as usize) * 4
    {
        return Vec::new();
    }

    let samples = downsample(pixels, width, height);
    if samples.is_empty() {
        return Vec::new();
    }

    // If we have fewer unique pixels than requested centroids, just
    // return the distinct colours sorted by frequency.
    let k = max_colors.min(samples.len());
    let centroids_init = init_centroids(&samples, k);
    let (centroids, assignments) = run_kmeans(&samples, &centroids_init);
    let mut clusters: Vec<(usize, [f32; 3])> = centroids
        .iter()
        .enumerate()
        .map(|(i, c)| (i, *c))
        .collect();
    let mut counts = vec![0usize; clusters.len()];
    for &a in &assignments {
        counts[a] += 1;
    }
    // Drop empty clusters — they may happen when init_centroids picks
    // duplicate samples.
    let total = samples.len() as f32;
    clusters.retain(|(i, _)| counts[*i] > 0);
    let mut out: Vec<ExtractedColor> = clusters
        .into_iter()
        .map(|(i, c)| {
            let freq = counts[i] as f32 / total;
            let r = c[0].round().clamp(0.0, 255.0) as u8;
            let g = c[1].round().clamp(0.0, 255.0) as u8;
            let b = c[2].round().clamp(0.0, 255.0) as u8;
            ExtractedColor::from_rgb(r, g, b, freq)
        })
        .collect();
    out.sort_by(|a, b| b.frequency.partial_cmp(&a.frequency).unwrap_or(std::cmp::Ordering::Equal));
    out
}

/// Sample at most `SAMPLE_DIM_CAP * SAMPLE_DIM_CAP` opaque pixels via
/// integer stride. Returns f32 RGB samples.
fn downsample(pixels: &[u8], width: u32, height: u32) -> Vec<[f32; 3]> {
    let stride_x = width.div_ceil(SAMPLE_DIM_CAP).max(1);
    let stride_y = height.div_ceil(SAMPLE_DIM_CAP).max(1);
    let mut out: Vec<[f32; 3]> =
        Vec::with_capacity((width as usize / stride_x as usize + 1) * (height as usize / stride_y as usize + 1));
    let mut y = 0u32;
    while y < height {
        let mut x = 0u32;
        while x < width {
            let idx = ((y as usize) * (width as usize) + (x as usize)) * 4;
            let a = pixels[idx + 3];
            if a > 0 {
                out.push([
                    f32::from(pixels[idx]),
                    f32::from(pixels[idx + 1]),
                    f32::from(pixels[idx + 2]),
                ]);
            }
            x += stride_x;
        }
        y += stride_y;
    }
    out
}

/// Quantile-based deterministic initialisation: sort samples by
/// luminance and pick `k` evenly-spaced quantiles.
fn init_centroids(samples: &[[f32; 3]], k: usize) -> Vec<[f32; 3]> {
    let mut indices: Vec<usize> = (0..samples.len()).collect();
    indices.sort_by(|a, b| {
        let la = luminance(&samples[*a]);
        let lb = luminance(&samples[*b]);
        la.partial_cmp(&lb).unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut out = Vec::with_capacity(k);
    for i in 0..k {
        let q = if k == 1 {
            indices.len() / 2
        } else {
            // Quantile k(i) = (i + 0.5) / k -> int index.
            let frac = (i as f32 + 0.5) / k as f32;
            ((frac * indices.len() as f32) as usize).min(indices.len() - 1)
        };
        out.push(samples[indices[q]]);
    }
    out
}

fn luminance(rgb: &[f32; 3]) -> f32 {
    // Rec.601 — good enough for sorting; we do not need perceptual
    // accuracy here.
    0.299 * rgb[0] + 0.587 * rgb[1] + 0.114 * rgb[2]
}

fn run_kmeans(samples: &[[f32; 3]], init: &[[f32; 3]]) -> (Vec<[f32; 3]>, Vec<usize>) {
    let mut centroids = init.to_vec();
    let mut assignments = vec![0usize; samples.len()];
    for _ in 0..MAX_KMEANS_ITERATIONS {
        let mut changed = false;
        for (si, s) in samples.iter().enumerate() {
            let mut best = 0usize;
            let mut best_dist = f32::INFINITY;
            for (ci, c) in centroids.iter().enumerate() {
                let d = sq_dist(s, c);
                if d < best_dist {
                    best_dist = d;
                    best = ci;
                }
            }
            if assignments[si] != best {
                assignments[si] = best;
                changed = true;
            }
        }
        // Recompute centroids.
        let mut sums = vec![[0.0f64; 3]; centroids.len()];
        let mut counts = vec![0usize; centroids.len()];
        for (si, s) in samples.iter().enumerate() {
            let c = assignments[si];
            sums[c][0] += f64::from(s[0]);
            sums[c][1] += f64::from(s[1]);
            sums[c][2] += f64::from(s[2]);
            counts[c] += 1;
        }
        for ci in 0..centroids.len() {
            if counts[ci] > 0 {
                let n = counts[ci] as f64;
                centroids[ci][0] = (sums[ci][0] / n) as f32;
                centroids[ci][1] = (sums[ci][1] / n) as f32;
                centroids[ci][2] = (sums[ci][2] / n) as f32;
            }
        }
        if !changed {
            break;
        }
    }
    (centroids, assignments)
}

fn sq_dist(a: &[f32; 3], b: &[f32; 3]) -> f32 {
    let dr = a[0] - b[0];
    let dg = a[1] - b[1];
    let db = a[2] - b[2];
    dr * dr + dg * dg + db * db
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: u32, height: u32, rgba: [u8; 4]) -> Vec<u8> {
        let mut v = Vec::with_capacity((width as usize) * (height as usize) * 4);
        for _ in 0..(width as usize) * (height as usize) {
            v.extend_from_slice(&rgba);
        }
        v
    }

    #[test]
    fn empty_input_returns_empty() {
        assert!(extract_palette(&[], 0, 0, 5).is_empty());
        let bytes = vec![0u8; 16];
        assert!(extract_palette(&bytes, 0, 4, 5).is_empty());
    }

    #[test]
    fn zero_max_colors_returns_empty() {
        let pixels = solid(2, 2, [255, 0, 0, 255]);
        assert!(extract_palette(&pixels, 2, 2, 0).is_empty());
    }

    #[test]
    fn buffer_length_mismatch_returns_empty() {
        let pixels = vec![0u8; 8];
        assert!(extract_palette(&pixels, 4, 4, 3).is_empty());
    }

    #[test]
    fn single_color_image_returns_single_color() {
        let pixels = solid(8, 8, [200, 100, 50, 255]);
        let palette = extract_palette(&pixels, 8, 8, 5);
        assert_eq!(palette.len(), 1, "expected a single dominant cluster");
        assert_eq!(palette[0].r, 200);
        assert_eq!(palette[0].g, 100);
        assert_eq!(palette[0].b, 50);
        assert_eq!(palette[0].hex, "#C86432");
        assert!(
            (palette[0].frequency - 1.0).abs() < 1e-3,
            "frequency for a single-color image must be 1.0"
        );
    }

    #[test]
    fn bicolor_image_returns_two_colors_most_frequent_first() {
        // 8 red pixels, 24 blue pixels — blue should sort first.
        let mut pixels = Vec::with_capacity(32 * 4);
        for _ in 0..8 {
            pixels.extend_from_slice(&[255, 0, 0, 255]);
        }
        for _ in 0..24 {
            pixels.extend_from_slice(&[0, 0, 255, 255]);
        }
        let palette = extract_palette(&pixels, 8, 4, 2);
        assert_eq!(palette.len(), 2, "expected two clusters");
        // Most-frequent first.
        assert!(
            palette[0].frequency >= palette[1].frequency,
            "first cluster should be at least as frequent"
        );
        // The dominant cluster must be the blue one (~75%).
        assert!(palette[0].b > 200, "dominant cluster expected blue");
        assert!(palette[1].r > 200, "second cluster expected red");
        assert!(
            (palette[0].frequency + palette[1].frequency - 1.0).abs() < 1e-2,
            "frequencies must sum to 1.0"
        );
    }

    #[test]
    fn transparent_pixels_are_ignored() {
        // 1 red opaque + 99 transparent. Only the red counts.
        let mut pixels = Vec::with_capacity(100 * 4);
        pixels.extend_from_slice(&[255, 0, 0, 255]);
        for _ in 0..99 {
            pixels.extend_from_slice(&[0, 0, 0, 0]);
        }
        let palette = extract_palette(&pixels, 10, 10, 5);
        assert_eq!(palette.len(), 1);
        assert_eq!(palette[0].r, 255);
    }

    #[test]
    fn fully_transparent_image_returns_empty() {
        let pixels = solid(4, 4, [255, 255, 255, 0]);
        assert!(extract_palette(&pixels, 4, 4, 3).is_empty());
    }

    #[test]
    fn hex_is_uppercase() {
        let pixels = solid(2, 2, [0xAB, 0xCD, 0xEF, 255]);
        let palette = extract_palette(&pixels, 2, 2, 1);
        assert_eq!(palette[0].hex, "#ABCDEF");
    }
}
