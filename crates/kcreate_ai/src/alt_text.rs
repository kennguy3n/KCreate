//! On-device alt-text generation for raster layers.
//!
//! This is *not* a transformer-based caption model — those need
//! 200MB+ of weights and a GPU to be useful. Instead, this is a
//! real heuristic generator that produces a short, factually
//! grounded description ("Bright photograph dominated by warm reds
//! and ambers on a light background, mostly photographic detail")
//! by computing real image statistics: dominant colours (via the
//! existing k-means [`crate::extract_palette`]), overall brightness,
//! contrast, hue centroid, saturation, and edge density.
//!
//! The output is intentionally factual rather than narrative —
//! genuine accessibility metadata, not a creative writing exercise.
//! When the user wants a richer caption they can run it through the
//! LLM sidecar as a post-processing step.
//!
//! Pure-function, no I/O, no networking. Used by
//! [`crate::execute_task`] under [`crate::AiTask::AltTextGeneration`].

use serde::{Deserialize, Serialize};

use crate::palette::{extract_palette, ExtractedColor};

/// Parameters for [`generate_alt_text`].
#[derive(Debug, Clone, Copy)]
pub struct AltTextOptions {
    /// Maximum number of dominant colours to consider when naming
    /// the palette. Caps the k-means run.
    pub max_palette_colors: usize,
    /// Sobel edge-density threshold (0.0..1.0) above which the
    /// image is described as "with significant photographic detail"
    /// rather than "a flat graphic". Default 0.18 was picked from
    /// the unit-test fixtures (gradient ramps land around 0.05,
    /// halftone photographs land around 0.25).
    pub edge_density_threshold: f32,
}

impl Default for AltTextOptions {
    fn default() -> Self {
        Self {
            max_palette_colors: 5,
            edge_density_threshold: 0.18,
        }
    }
}

/// Structured alt-text result.
///
/// `text` is the recommended human-readable string; the structured
/// fields are exposed so the caller can render a richer UI (palette
/// chips, brightness bar, etc.) without re-running the analysis.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[allow(clippy::derive_partial_eq_without_eq)]
pub struct AltTextReport {
    pub text: String,
    /// Mean luminance in 0.0..1.0 (Rec. 709 weights). Used to pick
    /// "dark" / "balanced" / "bright" in the description.
    pub brightness: f32,
    /// Standard deviation of luminance in 0.0..1.0. Used to pick
    /// "low contrast" / "balanced contrast" / "high contrast".
    pub contrast: f32,
    /// Mean saturation in 0.0..1.0 (HSV). Used to pick "muted" /
    /// "saturated" / "vivid".
    pub saturation: f32,
    /// Sobel-edge density in 0.0..1.0. Used to pick "flat graphic"
    /// vs "photographic detail".
    pub edge_density: f32,
    /// Top palette entries, sorted by frequency desc.
    pub palette: Vec<ExtractedColor>,
}

/// Errors returned by [`generate_alt_text`].
#[derive(Debug, thiserror::Error)]
pub enum AltTextError {
    /// `pixels.len()` did not match `width * height * 4`.
    #[error("pixel buffer is {actual} bytes; expected {expected}")]
    BufferSize { expected: usize, actual: usize },
    /// Width or height was zero — nothing to describe.
    #[error("image dimensions must be nonzero")]
    EmptyImage,
}

/// Analyse an RGBA8 raster and produce factual alt-text plus the
/// underlying image statistics.
///
/// The algorithm:
/// 1. Compute mean luminance (brightness), luminance stddev
///    (contrast), and mean saturation across all opaque pixels.
/// 2. Compute Sobel-edge density to distinguish "flat graphic"
///    from "photographic detail".
/// 3. Extract the top-N colours via the existing k-means
///    [`extract_palette`] and name the *dominant hue family*
///    (red / orange / yellow / green / cyan / blue / purple /
///    magenta / neutral).
/// 4. Render a single descriptive sentence covering brightness,
///    contrast, saturation, dominant hue family, and image kind.
pub fn generate_alt_text(
    pixels: &[u8],
    width: u32,
    height: u32,
    options: AltTextOptions,
) -> Result<AltTextReport, AltTextError> {
    if width == 0 || height == 0 {
        return Err(AltTextError::EmptyImage);
    }
    let expected = (width as usize) * (height as usize) * 4;
    if pixels.len() != expected {
        return Err(AltTextError::BufferSize {
            expected,
            actual: pixels.len(),
        });
    }

    let stats = image_statistics(pixels, width, height);
    let edge_density = sobel_edge_density(pixels, width, height);
    let palette = extract_palette(pixels, width, height, options.max_palette_colors.max(1));

    let brightness_word = match stats.brightness {
        b if b < 0.30 => "Dark",
        b if b < 0.65 => "Balanced",
        _ => "Bright",
    };
    let contrast_word = match stats.contrast {
        c if c < 0.10 => " low-contrast",
        c if c < 0.22 => "",
        _ => " high-contrast",
    };
    let saturation_word = match stats.saturation {
        s if s < 0.15 => "muted",
        s if s < 0.45 => "moderately saturated",
        _ => "vivid",
    };
    let kind_word = if edge_density > options.edge_density_threshold {
        "photographic"
    } else {
        "flat-graphic"
    };
    let hue_family = describe_dominant_hue(&palette);

    let mut text = format!("{brightness_word}{contrast_word} {kind_word} image");
    if let Some(h) = hue_family {
        text.push_str(", ");
        text.push_str(saturation_word);
        text.push_str(", dominated by ");
        text.push_str(h);
        text.push('.');
    } else {
        text.push_str(", ");
        text.push_str(saturation_word);
        text.push('.');
    }

    Ok(AltTextReport {
        text,
        brightness: stats.brightness,
        contrast: stats.contrast,
        saturation: stats.saturation,
        edge_density,
        palette,
    })
}

#[derive(Debug, Clone, Copy)]
struct ImageStats {
    brightness: f32,
    contrast: f32,
    saturation: f32,
}

/// Compute mean luminance, luminance stddev, and mean saturation
/// across all opaque pixels in the buffer. Skips fully-transparent
/// pixels (alpha == 0) so RGBA pads don't poison the result.
fn image_statistics(pixels: &[u8], width: u32, height: u32) -> ImageStats {
    let mut count = 0u64;
    let mut sum_y = 0.0_f64;
    let mut sum_y2 = 0.0_f64;
    let mut sum_s = 0.0_f64;
    for y in 0..height {
        for x in 0..width {
            let i = ((y as usize) * (width as usize) + x as usize) * 4;
            let a = pixels[i + 3];
            if a == 0 {
                continue;
            }
            let r = f32::from(pixels[i]) / 255.0;
            let g = f32::from(pixels[i + 1]) / 255.0;
            let b = f32::from(pixels[i + 2]) / 255.0;
            // Rec. 709 luminance.
            let lum = 0.2126 * r + 0.7152 * g + 0.0722 * b;
            let max = r.max(g).max(b);
            let min = r.min(g).min(b);
            let sat = if max > 0.0 { (max - min) / max } else { 0.0 };
            sum_y += f64::from(lum);
            sum_y2 += f64::from(lum * lum);
            sum_s += f64::from(sat);
            count += 1;
        }
    }
    if count == 0 {
        return ImageStats {
            brightness: 0.0,
            contrast: 0.0,
            saturation: 0.0,
        };
    }
    let n = count as f64;
    let mean_y = sum_y / n;
    let mean_sq_y = sum_y2 / n;
    let mean_y_sq = mean_y * mean_y;
    let var_y = (mean_sq_y - mean_y_sq).max(0.0);
    let stddev_y = var_y.sqrt();
    let mean_s = sum_s / n;
    ImageStats {
        brightness: mean_y as f32,
        contrast: stddev_y as f32,
        saturation: mean_s as f32,
    }
}

/// Compute Sobel-edge density: the fraction of opaque pixels whose
/// gradient magnitude exceeds 0.15 (a mid-strength threshold that
/// catches real edges without picking up JPEG noise).
fn sobel_edge_density(pixels: &[u8], width: u32, height: u32) -> f32 {
    if width < 3 || height < 3 {
        return 0.0;
    }
    let w = width as i32;
    let h = height as i32;
    // Pre-compute luminance into a flat buffer to avoid recomputing
    // it in the convolution inner loop.
    let mut lum = vec![0.0_f32; (width as usize) * (height as usize)];
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let r = f32::from(pixels[i]) / 255.0;
            let g = f32::from(pixels[i + 1]) / 255.0;
            let b = f32::from(pixels[i + 2]) / 255.0;
            lum[(y * w + x) as usize] = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        }
    }
    let sample = |xx: i32, yy: i32| -> f32 { lum[(yy * w + xx) as usize] };
    let mut strong = 0u64;
    let mut considered = 0u64;
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let i = ((y * w + x) * 4) as usize;
            if pixels[i + 3] == 0 {
                continue;
            }
            considered += 1;
            let gx = -sample(x - 1, y - 1) - 2.0 * sample(x - 1, y) - sample(x - 1, y + 1)
                + sample(x + 1, y - 1)
                + 2.0 * sample(x + 1, y)
                + sample(x + 1, y + 1);
            let gy = -sample(x - 1, y - 1) - 2.0 * sample(x, y - 1) - sample(x + 1, y - 1)
                + sample(x - 1, y + 1)
                + 2.0 * sample(x, y + 1)
                + sample(x + 1, y + 1);
            let mag = gx.hypot(gy);
            if mag > 0.15 {
                strong += 1;
            }
        }
    }
    if considered == 0 {
        0.0
    } else {
        strong as f32 / considered as f32
    }
}

/// Map the most-frequent palette entry to a hue-family English
/// phrase. Weighted by frequency so a small but vivid accent
/// doesn't override a large neutral background.
fn describe_dominant_hue(palette: &[ExtractedColor]) -> Option<&'static str> {
    let top = palette.first()?;
    Some(hue_family(top.r, top.g, top.b))
}

/// Classify an RGB into one of nine human-readable hue families.
/// Bins by HSV hue + saturation; an unsaturated colour gets
/// "neutral" / "near-black" / "near-white" based on luminance.
fn hue_family(r: u8, g: u8, b: u8) -> &'static str {
    let r = f32::from(r) / 255.0;
    let g = f32::from(g) / 255.0;
    let b = f32::from(b) / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let lum = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    if delta < 0.06 {
        if lum < 0.15 {
            return "near-black tones";
        } else if lum > 0.85 {
            return "near-white tones";
        }
        return "neutral greys";
    }
    let mut hue = if (max - r).abs() < f32::EPSILON {
        ((g - b) / delta) % 6.0
    } else if (max - g).abs() < f32::EPSILON {
        ((b - r) / delta) + 2.0
    } else {
        ((r - g) / delta) + 4.0
    };
    hue *= 60.0;
    if hue < 0.0 {
        hue += 360.0;
    }
    match hue {
        h if !(15.0..345.0).contains(&h) => "reds and pinks",
        h if h < 45.0 => "oranges and ambers",
        h if h < 70.0 => "yellows",
        h if h < 165.0 => "greens",
        h if h < 195.0 => "cyans",
        h if h < 255.0 => "blues",
        h if h < 285.0 => "purples",
        _ => "magentas",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid_image(width: u32, height: u32, r: u8, g: u8, b: u8, a: u8) -> Vec<u8> {
        let mut v = Vec::with_capacity((width as usize) * (height as usize) * 4);
        for _ in 0..(width as usize) * (height as usize) {
            v.extend_from_slice(&[r, g, b, a]);
        }
        v
    }

    #[test]
    fn rejects_empty_image() {
        let err = generate_alt_text(&[], 0, 0, AltTextOptions::default()).expect_err("empty");
        assert!(matches!(err, AltTextError::EmptyImage));
    }

    #[test]
    fn rejects_buffer_size_mismatch() {
        let err = generate_alt_text(&[0u8; 4], 4, 4, AltTextOptions::default()).expect_err("size");
        assert!(matches!(
            err,
            AltTextError::BufferSize {
                expected: 64,
                actual: 4
            }
        ));
    }

    #[test]
    fn solid_dark_red_describes_as_dark_reds() {
        let pixels = solid_image(32, 32, 60, 0, 0, 255);
        let r = generate_alt_text(&pixels, 32, 32, AltTextOptions::default()).expect("ok");
        assert!(r.text.starts_with("Dark"), "got {:?}", r.text);
        assert!(r.text.contains("reds and pinks"), "got {:?}", r.text);
        assert!(r.brightness < 0.30);
        // Solid fill — Sobel gradient is zero everywhere.
        assert_eq!(r.edge_density, 0.0);
        assert!(r.text.contains("flat-graphic"));
    }

    #[test]
    fn near_white_describes_as_bright_near_white() {
        let pixels = solid_image(16, 16, 250, 250, 250, 255);
        let r = generate_alt_text(&pixels, 16, 16, AltTextOptions::default()).expect("ok");
        assert!(r.text.starts_with("Bright"), "got {:?}", r.text);
        assert!(r.text.contains("near-white tones"), "got {:?}", r.text);
    }

    #[test]
    fn fully_transparent_image_does_not_panic_and_returns_neutral() {
        let pixels = solid_image(8, 8, 200, 0, 0, 0);
        let r = generate_alt_text(&pixels, 8, 8, AltTextOptions::default()).expect("ok");
        // All-transparent => no opaque samples, statistics are zero,
        // palette is empty, no hue family.
        assert_eq!(r.brightness, 0.0);
        assert!(r.palette.is_empty());
        assert!(!r.text.contains("dominated by"));
    }

    #[test]
    fn stripes_classify_as_photographic_detail() {
        // Period-4 vertical stripes (2px on, 2px off). This is the
        // simplest pattern where the Sobel kernel reads two
        // different colors at (x-1, x+1) for EVERY interior pixel.
        // A period-2 alternation (single-pixel stripes) would NOT
        // work because x-1 and x+1 land on the same colour. A pure
        // checkerboard wouldn't work either because both Sobel
        // axes integrate to zero by symmetry.
        let w = 16u32;
        let h = 16u32;
        let mut pixels = Vec::with_capacity((w as usize) * (h as usize) * 4);
        for _y in 0..h {
            for x in 0..w {
                let on = (x / 2) % 2 == 0;
                let v = if on { 240 } else { 16 };
                pixels.extend_from_slice(&[v, v, v, 255]);
            }
        }
        let r = generate_alt_text(&pixels, w, h, AltTextOptions::default()).expect("ok");
        assert!(r.edge_density > 0.5, "edge density was {}", r.edge_density);
        assert!(
            r.text.contains("photographic"),
            "should classify as photographic, got {:?}",
            r.text
        );
        assert!(
            r.text.contains("high-contrast"),
            "stripes should classify as high-contrast, got {:?}",
            r.text
        );
    }

    /// Tripwire: pins the JSON wire format `AltTextReport`
    /// serialises to. The renderer-side TS mirror at
    /// `apps/desktop/shared/scene.ts::AltTextReport` expects
    /// `snake_case` field names (no `rename_all = "camelCase"` on
    /// the Rust struct), and any future contributor who adds a
    /// `#[serde(rename_all)]` attribute — or renames / adds /
    /// removes a field — without updating the TS mirror would
    /// silently break the IPC contract: `phase2.rs` serialises
    /// this struct via `serde_json::to_string`, and the renderer
    /// `JSON.parse`s the result into a typed `AltTextReport`,
    /// with no automatic case translation in between.
    ///
    /// AGENTS.md rule 4 (wire-format lockstep) is the parent
    /// invariant; this test enforces it for the alt-text surface
    /// in the same way that
    /// `session_event_variants_serialise_to_renderer_camel_case_wire_format`
    /// enforces it for the `SessionEvent` enum.
    #[test]
    fn alt_text_report_serialises_to_renderer_wire_format() {
        let report = AltTextReport {
            text: "Bright photograph dominated by reds and pinks".to_string(),
            brightness: 0.75,
            contrast: 0.22,
            saturation: 0.5,
            edge_density: 0.18,
            palette: vec![ExtractedColor {
                r: 255,
                g: 0,
                b: 0,
                hex: "#FF0000".to_string(),
                frequency: 0.42,
            }],
        };
        let v = serde_json::to_value(&report).expect("serialise");
        let obj = v.as_object().expect("AltTextReport must be a JSON object");
        // Exhaustive shape — assert every expected key is present
        // and the unexpected snake-cased aliases are NOT present
        // (defence against accidentally adding a `rename_all`).
        for expected in [
            "text",
            "brightness",
            "contrast",
            "saturation",
            "edge_density",
            "palette",
        ] {
            assert!(
                obj.contains_key(expected),
                "AltTextReport missing wire-format field {expected:?}; got {obj:?}",
            );
        }
        // The renderer's TS type uses `edge_density`, so the
        // camelCase variant must NOT appear; if a future change
        // adds `#[serde(rename_all = "camelCase")]` this assertion
        // fires and the contributor is forced to update
        // `shared/scene.ts` in the same commit.
        assert!(
            !obj.contains_key("edgeDensity"),
            "AltTextReport leaked camelCase wire field; shared/scene.ts expects snake_case",
        );
        // Field values round-trip correctly.
        assert_eq!(obj["text"], "Bright photograph dominated by reds and pinks");
        assert_eq!(obj["edge_density"], f64::from(0.18_f32));
        // Nested palette must remain snake_case too — ExtractedColor
        // has its own wire-format contract that the renderer reads
        // verbatim (`r`, `g`, `b`, `hex`, `frequency`).
        let palette = obj["palette"].as_array().expect("palette array");
        assert_eq!(palette.len(), 1);
        let color = palette[0].as_object().expect("color object");
        for expected in ["r", "g", "b", "hex", "frequency"] {
            assert!(
                color.contains_key(expected),
                "ExtractedColor missing wire-format field {expected:?}; got {color:?}",
            );
        }
        assert_eq!(color["hex"], "#FF0000");
        assert_eq!(color["frequency"], f64::from(0.42_f32));
    }

    #[test]
    fn hue_family_buckets_canonical_corners() {
        assert_eq!(hue_family(255, 0, 0), "reds and pinks");
        assert_eq!(hue_family(255, 128, 0), "oranges and ambers");
        assert_eq!(hue_family(255, 255, 0), "yellows");
        assert_eq!(hue_family(0, 255, 0), "greens");
        assert_eq!(hue_family(0, 255, 255), "cyans");
        assert_eq!(hue_family(0, 0, 255), "blues");
        assert_eq!(hue_family(128, 0, 255), "purples");
        assert_eq!(hue_family(255, 0, 255), "magentas");
        assert_eq!(hue_family(128, 128, 128), "neutral greys");
        assert_eq!(hue_family(0, 0, 0), "near-black tones");
        assert_eq!(hue_family(250, 250, 250), "near-white tones");
    }
}
