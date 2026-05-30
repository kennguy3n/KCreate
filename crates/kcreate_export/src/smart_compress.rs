//! SSIM-targeting smart compressor — Phase 10 Block C Task 14.
//!
//! Given a raw RGBA buffer and a target SSIM (structural similarity)
//! score, repeatedly encode at falling quality settings until the
//! produced bytes round-trip-decode to a buffer whose 8×8-block SSIM
//! against the original drops below the target. The *last* setting
//! whose SSIM still satisfied the target wins.
//!
//! Quality search uses a binary search, not a linear scan, so a 100-
//! step range collapses in ~7 iterations.
//!
//! SSIM implementation: the canonical luminance × contrast ×
//! structure product on 8×8 luminance tiles, row-parallel via
//! rayon. Constants `C1` / `C2` come from Wang & Bovik (2004).

use image::{ImageEncoder, RgbaImage};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SmartCompressFormat {
    Jpeg,
    Webp,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SmartCompressOptions {
    pub format: SmartCompressFormat,
    pub target_ssim: f64,
    /// Minimum quality the search is allowed to consider. Below this
    /// we return the lowest quality even if SSIM is missed. JPEG: 1.
    pub min_quality: u8,
    /// Maximum quality the search starts at. JPEG: 95 (above which
    /// returns diminish faster than file size grows).
    pub max_quality: u8,
}

impl Default for SmartCompressOptions {
    fn default() -> Self {
        Self {
            format: SmartCompressFormat::Jpeg,
            target_ssim: 0.98,
            min_quality: 30,
            max_quality: 95,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SmartCompressReport {
    pub quality: u8,
    pub format: SmartCompressFormat,
    pub original_bytes: u64,
    pub compressed_bytes: u64,
    pub ratio: f64,
    pub ssim: f64,
    /// Number of binary-search iterations actually executed.
    pub iterations: u32,
    /// The chosen compressed payload, ready to write to disk or
    /// return to the renderer as a base64 blob.
    #[serde(with = "base64_blob")]
    pub bytes: Vec<u8>,
}

#[derive(Debug, Error)]
pub enum SmartCompressError {
    #[error(
        "smart_compress: input pixel buffer is the wrong length: expected {expected}, got {got}"
    )]
    BadBuffer { expected: usize, got: usize },
    #[error("smart_compress: dimensions zero (width={0}, height={1})")]
    ZeroDim(u32, u32),
    #[error("smart_compress: encoding failed: {0}")]
    Encode(String),
    #[error("smart_compress: target_ssim out of [0,1]: {0}")]
    BadTarget(f64),
}

/// Run the binary-search compressor.
///
/// # Errors
///
/// Returns [`SmartCompressError::BadBuffer`] when the RGBA buffer
/// length doesn't match `width * height * 4`, [`SmartCompressError::ZeroDim`]
/// when either dimension is zero, [`SmartCompressError::BadTarget`] when
/// the target SSIM falls outside `[0, 1]`, and
/// [`SmartCompressError::Encode`] when the underlying encoder fails.
pub fn smart_compress(
    pixels: &[u8],
    width: u32,
    height: u32,
    opts: SmartCompressOptions,
) -> Result<SmartCompressReport, SmartCompressError> {
    if width == 0 || height == 0 {
        return Err(SmartCompressError::ZeroDim(width, height));
    }
    let expected = (width as usize) * (height as usize) * 4;
    if pixels.len() != expected {
        return Err(SmartCompressError::BadBuffer {
            expected,
            got: pixels.len(),
        });
    }
    if !(0.0..=1.0).contains(&opts.target_ssim) {
        return Err(SmartCompressError::BadTarget(opts.target_ssim));
    }
    let original_bytes = expected as u64;

    let min = opts.min_quality.clamp(1, 100);
    let max = opts.max_quality.clamp(min, 100);

    let mut low = min;
    let mut high = max;
    let mut best: Option<(u8, Vec<u8>, f64)> = None;
    let mut iterations = 0u32;

    // Binary search: the lowest quality whose SSIM still satisfies
    // the target. Quality is monotone (higher quality → higher SSIM)
    // for these encoders, which makes this safe.
    while low <= high {
        let mid = low + (high - low) / 2;
        let bytes = encode_at_quality(pixels, width, height, opts.format, mid)?;
        let decoded = decode_to_rgba(&bytes, opts.format, width, height)?;
        let score = ssim_rgba(pixels, &decoded, width, height);
        iterations += 1;
        if score >= opts.target_ssim {
            best = Some((mid, bytes, score));
            if mid == 0 {
                break;
            }
            high = mid.saturating_sub(1);
        } else {
            low = mid + 1;
        }
        if iterations > 12 {
            break;
        }
    }

    let (quality, bytes, ssim) = match best {
        Some(b) => b,
        None => {
            // Nothing satisfied the target — return the highest
            // quality the search considered so the user still gets
            // something usable.
            let bytes = encode_at_quality(pixels, width, height, opts.format, max)?;
            let decoded = decode_to_rgba(&bytes, opts.format, width, height)?;
            let score = ssim_rgba(pixels, &decoded, width, height);
            (max, bytes, score)
        }
    };

    let compressed_bytes = bytes.len() as u64;
    let ratio = compressed_bytes as f64 / original_bytes as f64;
    Ok(SmartCompressReport {
        quality,
        format: opts.format,
        original_bytes,
        compressed_bytes,
        ratio,
        ssim,
        iterations,
        bytes,
    })
}

fn encode_at_quality(
    pixels: &[u8],
    width: u32,
    height: u32,
    format: SmartCompressFormat,
    quality: u8,
) -> Result<Vec<u8>, SmartCompressError> {
    let q = quality.clamp(1, 100);
    let mut out = Vec::with_capacity((pixels.len() / 8).max(1024));
    match format {
        SmartCompressFormat::Jpeg => {
            // JPEG is opaque; composite against white so we have
            // something to encode for transparent regions.
            let rgb = rgba_to_rgb_over_white(pixels);
            let mut cursor = std::io::Cursor::new(&mut out);
            let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut cursor, q);
            enc.write_image(&rgb, width, height, image::ExtendedColorType::Rgb8)
                .map_err(|e| SmartCompressError::Encode(format!("JPEG: {e}")))?;
        }
        SmartCompressFormat::Webp => {
            // The bundled WebP encoder only supports lossless RGBA,
            // so we vary "quality" by downsampling + reupsampling
            // — lower quality means more aggressive downsample.
            let img = RgbaImage::from_raw(width, height, pixels.to_vec())
                .ok_or_else(|| SmartCompressError::Encode("WebP wrap".into()))?;
            let scale = (f32::from(q) / 100.0).clamp(0.30, 1.0);
            let dw = ((width as f32 * scale).round() as u32).max(1);
            let dh = ((height as f32 * scale).round() as u32).max(1);
            let small = if (dw, dh) == (width, height) {
                img
            } else {
                let s =
                    image::imageops::resize(&img, dw, dh, image::imageops::FilterType::Triangle);
                image::imageops::resize(&s, width, height, image::imageops::FilterType::Triangle)
            };
            let mut cursor = std::io::Cursor::new(&mut out);
            let enc = image::codecs::webp::WebPEncoder::new_lossless(&mut cursor);
            enc.write_image(
                small.as_raw(),
                width,
                height,
                image::ExtendedColorType::Rgba8,
            )
            .map_err(|e| SmartCompressError::Encode(format!("WebP: {e}")))?;
        }
    }
    Ok(out)
}

fn decode_to_rgba(
    bytes: &[u8],
    format: SmartCompressFormat,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, SmartCompressError> {
    let img = image::load_from_memory(bytes)
        .map_err(|e| SmartCompressError::Encode(format!("decode: {e}")))?
        .to_rgba8();
    if img.width() != width || img.height() != height {
        // For lossy formats the decoder should still produce
        // identical dims; if it doesn't (e.g. WebP rescaled) resize
        // back so SSIM compares apples-to-apples.
        let resized =
            image::imageops::resize(&img, width, height, image::imageops::FilterType::Triangle);
        Ok(resized.into_raw())
    } else {
        let _ = format; // currently unused beyond decode dispatch
        Ok(img.into_raw())
    }
}

fn rgba_to_rgb_over_white(rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.len() / 4 * 3);
    for chunk in rgba.chunks_exact(4) {
        let r = u16::from(chunk[0]);
        let g = u16::from(chunk[1]);
        let b = u16::from(chunk[2]);
        let a = u16::from(chunk[3]);
        let composite = |c: u16| ((c * a + 255 * (255 - a) + 127) / 255).min(255) as u8;
        out.push(composite(r));
        out.push(composite(g));
        out.push(composite(b));
    }
    out
}

// ---------------------------------------------------------------------------
// SSIM
// ---------------------------------------------------------------------------

const SSIM_K1: f64 = 0.01;
const SSIM_K2: f64 = 0.03;
const SSIM_L: f64 = 255.0;
const BLOCK: u32 = 8;

/// Compute the mean 8×8-block SSIM between two RGBA buffers using
/// the luminance channel.
#[must_use]
pub fn ssim_rgba(a: &[u8], b: &[u8], width: u32, height: u32) -> f64 {
    let la = luminance(a);
    let lb = luminance(b);
    let w = width as usize;
    let h = height as usize;
    let rows: Vec<(u32, u32)> = (0..(h as u32 / BLOCK))
        .flat_map(|by| (0..(w as u32 / BLOCK)).map(move |bx| (bx, by)))
        .collect();
    if rows.is_empty() {
        return 1.0;
    }
    let c1 = (SSIM_K1 * SSIM_L).powi(2);
    let c2 = (SSIM_K2 * SSIM_L).powi(2);
    let total: f64 = rows
        .par_iter()
        .map(|&(bx, by)| ssim_block(&la, &lb, w, h, bx, by, c1, c2))
        .sum();
    total / rows.len() as f64
}

// Per-block SSIM helper — its arg shape (two luma buffers, image
// dimensions, block coordinates, and the two SSIM stabilisation
// constants) matches the SSIM paper's notation 1:1 so we keep it
// flat rather than bundling into a struct.
#[allow(clippy::too_many_arguments)]
fn ssim_block(a: &[u8], b: &[u8], w: usize, _h: usize, bx: u32, by: u32, c1: f64, c2: f64) -> f64 {
    let x0 = (bx * BLOCK) as usize;
    let y0 = (by * BLOCK) as usize;
    let n = f64::from(BLOCK * BLOCK);
    let mut sum_a = 0.0;
    let mut sum_b = 0.0;
    for dy in 0..BLOCK as usize {
        for dx in 0..BLOCK as usize {
            sum_a += f64::from(a[(y0 + dy) * w + (x0 + dx)]);
            sum_b += f64::from(b[(y0 + dy) * w + (x0 + dx)]);
        }
    }
    let mean_a = sum_a / n;
    let mean_b = sum_b / n;
    let mut var_a = 0.0;
    let mut var_b = 0.0;
    let mut cov = 0.0;
    for dy in 0..BLOCK as usize {
        for dx in 0..BLOCK as usize {
            let va = f64::from(a[(y0 + dy) * w + (x0 + dx)]) - mean_a;
            let vb = f64::from(b[(y0 + dy) * w + (x0 + dx)]) - mean_b;
            var_a += va * va;
            var_b += vb * vb;
            cov += va * vb;
        }
    }
    var_a /= n;
    var_b /= n;
    cov /= n;
    let numerator = (2.0 * mean_a * mean_b + c1) * (2.0 * cov + c2);
    let denominator = (mean_a.powi(2) + mean_b.powi(2) + c1) * (var_a + var_b + c2);
    if denominator == 0.0 {
        1.0
    } else {
        numerator / denominator
    }
}

fn luminance(rgba: &[u8]) -> Vec<u8> {
    rgba.par_chunks_exact(4)
        .map(|c| {
            // Rec. 709 luma.
            let r = f64::from(c[0]);
            let g = f64::from(c[1]);
            let b = f64::from(c[2]);
            (0.2126 * r + 0.7152 * g + 0.0722 * b)
                .clamp(0.0, 255.0)
                .round() as u8
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Base64 serde helper — keeps the payload JSON-safe but binary-fast
// ---------------------------------------------------------------------------

mod base64_blob {
    use base64::Engine as _;
    use serde::{Deserialize, Deserializer, Serializer};

    pub(super) fn serialize<S: Serializer>(v: &Vec<u8>, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(v))
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        base64::engine::general_purpose::STANDARD
            .decode(s)
            .map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checker(w: u32, h: u32) -> Vec<u8> {
        let mut v = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let on = ((x / 4) + (y / 4)) % 2 == 0;
                let c = if on { 220 } else { 32 };
                v.extend_from_slice(&[c, c, c, 255]);
            }
        }
        v
    }

    #[test]
    fn ssim_identity_is_one() {
        let buf = checker(32, 32);
        let s = ssim_rgba(&buf, &buf, 32, 32);
        assert!((s - 1.0).abs() < 1e-6, "got {s}");
    }

    #[test]
    fn ssim_falls_with_noise() {
        let buf = checker(32, 32);
        let mut noisy = buf.clone();
        // SSIM is computed on luminance, so a uniform RGB bias would
        // largely cancel out (e.g. +40 R and -40 G produces a small
        // luminance shift). To actually move the luminance variance
        // we alternate the bias sign every pixel — this introduces
        // high-frequency noise that disrupts the structure term of
        // SSIM the way real compression artefacts do.
        for (i, chunk) in noisy.chunks_exact_mut(4).enumerate() {
            let sign: i16 = if i.is_multiple_of(2) { 1 } else { -1 };
            let delta = 60 * sign;
            for slot in chunk.iter_mut().take(3) {
                *slot = (i16::from(*slot) + delta).clamp(0, 255) as u8;
            }
        }
        let s = ssim_rgba(&buf, &noisy, 32, 32);
        assert!(s < 0.98, "expected <0.98, got {s}");
    }

    #[test]
    fn jpeg_round_trip_meets_target() {
        let pixels = checker(64, 64);
        let report = smart_compress(
            &pixels,
            64,
            64,
            SmartCompressOptions {
                format: SmartCompressFormat::Jpeg,
                target_ssim: 0.85,
                min_quality: 5,
                max_quality: 95,
            },
        )
        .unwrap();
        assert!(report.compressed_bytes < report.original_bytes);
        assert!(report.ssim >= 0.85 || report.quality == 95);
    }

    #[test]
    fn bad_buffer_length_errors() {
        let err = smart_compress(&[0u8; 10], 4, 4, SmartCompressOptions::default()).unwrap_err();
        assert!(matches!(err, SmartCompressError::BadBuffer { .. }));
    }

    #[test]
    fn bad_target_errors() {
        let pixels = checker(8, 8);
        let err = smart_compress(
            &pixels,
            8,
            8,
            SmartCompressOptions {
                target_ssim: 1.5,
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(matches!(err, SmartCompressError::BadTarget(_)));
    }
}
