//! Automatic colour correction — Phase 10 Block A Task 3.
//!
//! Implements three classical algorithms plus a combined mode:
//!
//! 1. **Auto levels** — clip the 0.5 % darkest and brightest pixels
//!    per channel, then linearly stretch the remaining range to
//!    `[0, 255]`. Improves contrast without changing colour balance
//!    when one channel is concentrated in a narrow band.
//! 2. **White balance** (gray-world assumption) — estimate the
//!    scene illuminant by averaging each channel, then scale each
//!    channel so the means match the green channel's mean. Removes
//!    colour casts that affect all pixels uniformly.
//! 3. **Histogram equalisation** — per-channel CDF mapping that
//!    flattens each channel's histogram. Maximises dynamic range
//!    but can introduce colour shifts if applied naively to RGB.
//! 4. **Combined** — auto-levels, then white-balance, then a soft
//!    luminance-only histogram equalisation. The combined mode is
//!    the user-facing "auto-fix" preset.
//!
//! All algorithms operate on 8-bit RGBA and preserve the alpha
//! channel. Row-parallel via `rayon` on the per-pixel mapping step
//! after the (single-pass) histogram is computed.

use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Which correction algorithm(s) to run.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum AutoColorMode {
    AutoLevels,
    WhiteBalance,
    HistogramEqualization,
    #[default]
    Combined,
}

impl AutoColorMode {
    /// Parse a wire-format name into a mode. Accepts both the
    /// snake_case serde rendering and a couple of friendlier aliases
    /// the renderer panel uses.
    #[must_use]
    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "auto_levels" | "autoLevels" | "levels" => Some(Self::AutoLevels),
            "white_balance" | "whiteBalance" | "wb" => Some(Self::WhiteBalance),
            "histogram_equalization" | "histogramEqualization" | "he" => {
                Some(Self::HistogramEqualization)
            }
            "combined" | "auto" => Some(Self::Combined),
            _ => None,
        }
    }
}

/// Tunables for [`auto_color_correct`].
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct AutoColorOptions {
    pub mode: AutoColorMode,
    /// Per-channel clip percentile for auto-levels. Default 0.005
    /// (= 0.5 %). Clamped to `[0.0, 0.2]`.
    pub clip: f32,
}

impl Default for AutoColorOptions {
    fn default() -> Self {
        Self {
            mode: AutoColorMode::default(),
            clip: 0.005,
        }
    }
}

impl AutoColorOptions {
    #[must_use]
    pub fn clamped(mut self) -> Self {
        if !self.clip.is_finite() || self.clip < 0.0 {
            self.clip = 0.0;
        } else {
            self.clip = self.clip.min(0.2);
        }
        self
    }
}

#[derive(Debug, Error)]
pub enum AutoColorError {
    #[error("auto_color: empty image")]
    Empty,
    #[error("auto_color: pixel buffer length {got} != expected {expected}")]
    BufferSize { got: usize, expected: usize },
}

/// Apply automatic colour correction to an 8-bit RGBA image.
///
/// # Errors
///
/// Returns [`AutoColorError`] for empty inputs or buffer size
/// mismatches.
pub fn auto_color_correct(
    pixels: &[u8],
    width: u32,
    height: u32,
    options: AutoColorOptions,
) -> Result<Vec<u8>, AutoColorError> {
    if width == 0 || height == 0 {
        return Err(AutoColorError::Empty);
    }
    let expected = (width as usize) * (height as usize) * 4;
    if pixels.len() != expected {
        return Err(AutoColorError::BufferSize {
            got: pixels.len(),
            expected,
        });
    }
    let opts = options.clamped();
    match opts.mode {
        AutoColorMode::AutoLevels => Ok(auto_levels(pixels, width, height, opts.clip)),
        AutoColorMode::WhiteBalance => Ok(white_balance(pixels, width, height)),
        AutoColorMode::HistogramEqualization => Ok(histogram_equalization(pixels, width, height)),
        AutoColorMode::Combined => {
            let leveled = auto_levels(pixels, width, height, opts.clip);
            let balanced = white_balance(&leveled, width, height);
            Ok(luminance_equalization(&balanced, width, height))
        }
    }
}

/// Auto-levels: compute the `clip`-percentile and `(1 - clip)`-percentile
/// per channel, then linearly stretch `[low, high] → [0, 255]`.
pub fn auto_levels(pixels: &[u8], width: u32, height: u32, clip: f32) -> Vec<u8> {
    let total = (width as usize) * (height as usize);
    let mut hist = [[0u32; 256]; 3];
    for chunk in pixels.chunks_exact(4) {
        hist[0][chunk[0] as usize] += 1;
        hist[1][chunk[1] as usize] += 1;
        hist[2][chunk[2] as usize] += 1;
    }
    let clip_count = ((total as f32) * clip).round().max(0.0) as u32;
    let mut lo = [0u8; 3];
    let mut hi = [255u8; 3];
    for c in 0..3 {
        let mut acc = 0u32;
        for v in 0..=255u32 {
            acc += hist[c][v as usize];
            if acc > clip_count {
                lo[c] = v as u8;
                break;
            }
        }
        let mut acc2 = 0u32;
        for v in (0..=255u32).rev() {
            acc2 += hist[c][v as usize];
            if acc2 > clip_count {
                hi[c] = v as u8;
                break;
            }
        }
        if hi[c] <= lo[c] {
            // Channel is concentrated in a single bin — no useful
            // stretch. Leave the LUT as a pass-through.
            lo[c] = 0;
            hi[c] = 255;
        }
    }
    // Pre-compute the LUT per channel.
    let mut lut = [[0u8; 256]; 3];
    for c in 0..3 {
        let lo_f = f32::from(lo[c]);
        let hi_f = f32::from(hi[c]);
        let span = (hi_f - lo_f).max(1.0);
        for (v, slot) in lut[c].iter_mut().enumerate() {
            let scaled = ((v as f32 - lo_f) * 255.0 / span).clamp(0.0, 255.0);
            *slot = scaled.round() as u8;
        }
    }
    let mut out = vec![0u8; pixels.len()];
    out.par_chunks_mut(4)
        .zip(pixels.par_chunks(4))
        .for_each(|(dst, src)| {
            dst[0] = lut[0][src[0] as usize];
            dst[1] = lut[1][src[1] as usize];
            dst[2] = lut[2][src[2] as usize];
            dst[3] = src[3];
        });
    out
}

/// Gray-world white balance: scale each channel so its mean matches
/// the green channel's mean. Robust on photos with neutral
/// reference content (e.g. white walls, gray asphalt).
pub fn white_balance(pixels: &[u8], width: u32, height: u32) -> Vec<u8> {
    let total = (width as usize) * (height as usize);
    if total == 0 {
        return Vec::new();
    }
    let mut sum = [0u64; 3];
    for chunk in pixels.chunks_exact(4) {
        sum[0] += u64::from(chunk[0]);
        sum[1] += u64::from(chunk[1]);
        sum[2] += u64::from(chunk[2]);
    }
    let mean_r = sum[0] as f32 / total as f32;
    let mean_g = sum[1] as f32 / total as f32;
    let mean_b = sum[2] as f32 / total as f32;
    // Avoid divide-by-zero on degenerate all-black channels — leave
    // the channel alone in that case.
    let scale_r = if mean_r > 0.0 { mean_g / mean_r } else { 1.0 };
    let scale_b = if mean_b > 0.0 { mean_g / mean_b } else { 1.0 };
    let mut out = vec![0u8; pixels.len()];
    out.par_chunks_mut(4)
        .zip(pixels.par_chunks(4))
        .for_each(|(dst, src)| {
            dst[0] = (f32::from(src[0]) * scale_r).clamp(0.0, 255.0).round() as u8;
            dst[1] = src[1];
            dst[2] = (f32::from(src[2]) * scale_b).clamp(0.0, 255.0).round() as u8;
            dst[3] = src[3];
        });
    out
}

/// Per-channel histogram equalisation. Flattens each channel's
/// histogram so the cumulative distribution is uniform.
pub fn histogram_equalization(pixels: &[u8], width: u32, height: u32) -> Vec<u8> {
    let total = (width as usize) * (height as usize);
    if total == 0 {
        return Vec::new();
    }
    let mut hist = [[0u32; 256]; 3];
    for chunk in pixels.chunks_exact(4) {
        hist[0][chunk[0] as usize] += 1;
        hist[1][chunk[1] as usize] += 1;
        hist[2][chunk[2] as usize] += 1;
    }
    let mut lut = [[0u8; 256]; 3];
    for c in 0..3 {
        let mut cdf = 0u32;
        let mut cdf_min = 0u32;
        // First non-zero CDF anchor.
        for &h in &hist[c] {
            if h > 0 {
                cdf_min = h;
                break;
            }
        }
        let denom = (total as u32).saturating_sub(cdf_min).max(1) as f32;
        for (v, &h) in hist[c].iter().enumerate() {
            cdf += h;
            let value = ((cdf.saturating_sub(cdf_min) as f32) * 255.0 / denom)
                .clamp(0.0, 255.0)
                .round();
            lut[c][v] = value as u8;
        }
    }
    let mut out = vec![0u8; pixels.len()];
    out.par_chunks_mut(4)
        .zip(pixels.par_chunks(4))
        .for_each(|(dst, src)| {
            dst[0] = lut[0][src[0] as usize];
            dst[1] = lut[1][src[1] as usize];
            dst[2] = lut[2][src[2] as usize];
            dst[3] = src[3];
        });
    out
}

/// Luminance-only histogram equalisation in YCbCr space. Avoids the
/// hue shifts of per-channel RGB equalisation by only modifying the
/// Y (luma) channel.
fn luminance_equalization(pixels: &[u8], width: u32, height: u32) -> Vec<u8> {
    let total = (width as usize) * (height as usize);
    if total == 0 {
        return Vec::new();
    }
    let mut hist = [0u32; 256];
    // BT.601 luma weights.
    let lum = |r: u8, g: u8, b: u8| -> u8 {
        let y = 0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b);
        y.clamp(0.0, 255.0).round() as u8
    };
    for chunk in pixels.chunks_exact(4) {
        hist[lum(chunk[0], chunk[1], chunk[2]) as usize] += 1;
    }
    let mut cdf = 0u32;
    let mut cdf_min = 0u32;
    for &h in &hist {
        if h > 0 {
            cdf_min = h;
            break;
        }
    }
    let denom = (total as u32).saturating_sub(cdf_min).max(1) as f32;
    let mut lut = [0u8; 256];
    for (v, &h) in hist.iter().enumerate() {
        cdf += h;
        lut[v] = ((cdf.saturating_sub(cdf_min) as f32) * 255.0 / denom)
            .clamp(0.0, 255.0)
            .round() as u8;
    }
    let mut out = vec![0u8; pixels.len()];
    out.par_chunks_mut(4)
        .zip(pixels.par_chunks(4))
        .for_each(|(dst, src)| {
            let y = lum(src[0], src[1], src[2]);
            let new_y = lut[y as usize];
            if y == 0 {
                // Avoid division by zero — black pixel stays black.
                dst[0] = 0;
                dst[1] = 0;
                dst[2] = 0;
            } else {
                let scale = f32::from(new_y) / f32::from(y);
                dst[0] = (f32::from(src[0]) * scale).clamp(0.0, 255.0).round() as u8;
                dst[1] = (f32::from(src[1]) * scale).clamp(0.0, 255.0).round() as u8;
                dst[2] = (f32::from(src[2]) * scale).clamp(0.0, 255.0).round() as u8;
            }
            dst[3] = src[3];
        });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(w: u32, h: u32) -> Vec<u8> {
        let mut v = Vec::with_capacity((w as usize) * (h as usize) * 4);
        for _y in 0..h {
            for x in 0..w {
                let g = ((x as f32 / (w - 1) as f32) * 255.0) as u8;
                v.extend_from_slice(&[g, g, g, 255]);
            }
        }
        v
    }

    fn solid(w: u32, h: u32, c: [u8; 4]) -> Vec<u8> {
        let mut v = Vec::with_capacity((w as usize) * (h as usize) * 4);
        for _ in 0..(w as usize * h as usize) {
            v.extend_from_slice(&c);
        }
        v
    }

    #[test]
    fn auto_levels_widens_compressed_range() {
        // 32×1 image where green ranges only over [50, 150]. After
        // auto-levels the min/max should be near 0/255.
        let mut img = Vec::with_capacity(32 * 4);
        for x in 0..32 {
            let g = (50 + (x * 100) / 31) as u8;
            img.extend_from_slice(&[0, g, 0, 255]);
        }
        let out = auto_levels(&img, 32, 1, 0.0);
        let min = out.chunks(4).map(|c| c[1]).min().unwrap();
        let max = out.chunks(4).map(|c| c[1]).max().unwrap();
        assert!(min <= 5, "min should approach 0, got {min}");
        assert!(max >= 250, "max should approach 255, got {max}");
    }

    #[test]
    fn white_balance_neutralises_uniform_cast() {
        // Image with a 1.5x red cast on a gray background. White-
        // balance should pull red down toward green.
        let img = solid(8, 8, [180, 120, 120, 255]);
        let out = white_balance(&img, 8, 8);
        let mut sum_r = 0u32;
        let mut sum_g = 0u32;
        for c in out.chunks(4) {
            sum_r += u32::from(c[0]);
            sum_g += u32::from(c[1]);
        }
        let mean_r = sum_r as f32 / 64.0;
        let mean_g = sum_g as f32 / 64.0;
        // After gray-world WB the channel means should match within
        // ±2 quantisation units.
        assert!(
            (mean_r - mean_g).abs() <= 2.0,
            "WB failed to neutralise cast: mean_r={mean_r}, mean_g={mean_g}"
        );
    }

    #[test]
    fn histogram_equalization_flattens_distribution() {
        let img = ramp(64, 1);
        let out = histogram_equalization(&img, 64, 1);
        // The ramp's CDF is already linear, so equalisation should
        // return roughly the same ramp. But edge bins should still
        // anchor at 0 and 255 after rounding.
        let first = out[0];
        let last = out[(63 * 4) as usize];
        assert!(first <= 8, "min after EQ should be near 0, got {first}");
        assert!(last >= 247, "max after EQ should be near 255, got {last}");
    }

    #[test]
    fn alpha_channel_preserved_in_all_modes() {
        let mut img = solid(8, 8, [100, 100, 100, 255]);
        img[5 * 4 + 3] = 0;
        for mode in [
            AutoColorMode::AutoLevels,
            AutoColorMode::WhiteBalance,
            AutoColorMode::HistogramEqualization,
            AutoColorMode::Combined,
        ] {
            let out =
                auto_color_correct(&img, 8, 8, AutoColorOptions { mode, clip: 0.005 }).unwrap();
            assert_eq!(out[5 * 4 + 3], 0, "alpha lost in mode {mode:?}");
        }
    }

    #[test]
    fn empty_image_errors() {
        assert!(matches!(
            auto_color_correct(&[], 0, 0, AutoColorOptions::default()),
            Err(AutoColorError::Empty)
        ));
    }

    #[test]
    fn buffer_size_mismatch_errors() {
        assert!(matches!(
            auto_color_correct(&[1u8], 4, 4, AutoColorOptions::default()),
            Err(AutoColorError::BufferSize { .. })
        ));
    }

    #[test]
    fn mode_parser_accepts_aliases() {
        assert_eq!(
            AutoColorMode::from_wire("auto_levels"),
            Some(AutoColorMode::AutoLevels)
        );
        assert_eq!(
            AutoColorMode::from_wire("autoLevels"),
            Some(AutoColorMode::AutoLevels)
        );
        assert_eq!(
            AutoColorMode::from_wire("wb"),
            Some(AutoColorMode::WhiteBalance)
        );
        assert_eq!(AutoColorMode::from_wire("???"), None);
    }
}
