//! JPEG export — render a scene through the offscreen wgpu pipeline,
//! then encode the resulting frame to JPEG via the `image` crate.
//!
//! JPEG is opaque-only; the renderer's RGBA8 frame is flattened against
//! the configured background (default white) before encoding. The
//! `quality` knob is forwarded directly to `JpegEncoder::new_with_quality`.

use std::path::Path;

use image::{codecs::jpeg::JpegEncoder, ColorType, ImageEncoder};
use kcreate_renderer::geometry::Color;
use kcreate_renderer::{initialize, Scene};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::png::PngExportError;

/// Errors from JPEG export.
#[derive(Debug, Error)]
pub enum JpegExportError {
    #[error("invalid dimensions: width and height must both be positive")]
    InvalidDimensions,
    #[error("invalid scale: {0}; must be > 0 and finite")]
    InvalidScale(f32),
    #[error("invalid quality: {0}; must be in 1..=100")]
    InvalidQuality(u32),
    #[error(transparent)]
    Renderer(#[from] kcreate_renderer::RendererError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("encoding failed: {0}")]
    Encode(String),
    #[error("no frame published")]
    NoFrame,
}

impl From<PngExportError> for JpegExportError {
    fn from(value: PngExportError) -> Self {
        match value {
            PngExportError::InvalidDimensions => Self::InvalidDimensions,
            PngExportError::InvalidScale(s) => Self::InvalidScale(s),
            PngExportError::Renderer(e) => Self::Renderer(e),
            PngExportError::Io(e) => Self::Io(e),
            PngExportError::Encode(e) => Self::Encode(e),
            PngExportError::NoFrame => Self::NoFrame,
        }
    }
}

/// JPEG export options.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct JpegExportOptions {
    pub width: u32,
    pub height: u32,
    pub scale: f32,
    /// 1..=100. Higher values trade file size for fidelity.
    pub quality: u32,
    /// Background color. JPEG is opaque, so this is always composited
    /// in. `None` means white (255, 255, 255).
    pub background: Option<Color>,
}

impl Default for JpegExportOptions {
    fn default() -> Self {
        Self {
            width: 512,
            height: 512,
            scale: 1.0,
            quality: 90,
            background: None,
        }
    }
}

/// Render `scene` into JPEG bytes.
pub fn export_jpeg_to_bytes(
    scene: &Scene,
    options: &JpegExportOptions,
) -> Result<Vec<u8>, JpegExportError> {
    if options.width == 0 || options.height == 0 {
        return Err(JpegExportError::InvalidDimensions);
    }
    if !options.scale.is_finite() || options.scale <= 0.0 {
        return Err(JpegExportError::InvalidScale(options.scale));
    }
    if !(1..=100).contains(&options.quality) {
        return Err(JpegExportError::InvalidQuality(options.quality));
    }

    let final_w = scaled_dim(options.width, options.scale);
    let final_h = scaled_dim(options.height, options.scale);

    let bg = options
        .background
        .unwrap_or_else(|| Color::rgba(1.0, 1.0, 1.0, 1.0));

    let mut scene = scene.clone();
    scene.clear_color = bg;

    let ctx = initialize(final_w, final_h)?;
    let _frame_id = ctx.render_frame(&scene)?;
    let rgba: Vec<u8> = {
        let frame = ctx.latest_frame().ok_or(JpegExportError::NoFrame)?;
        frame.pixels().to_vec()
    };

    // Composite RGBA8 → RGB8 against the explicit background. The
    // renderer's alpha can legitimately be < 1 on transparent regions
    // even when clear_color.a == 1, e.g. when an object's fill is
    // partially transparent. Pre-multiplying by alpha here keeps the
    // result identical to what the user sees on the canvas.
    let mut rgb = Vec::with_capacity((final_w as usize) * (final_h as usize) * 3);
    let br = (bg.r * 255.0).round().clamp(0.0, 255.0) as u8;
    let bg_g = (bg.g * 255.0).round().clamp(0.0, 255.0) as u8;
    let bb = (bg.b * 255.0).round().clamp(0.0, 255.0) as u8;
    for px in rgba.chunks_exact(4) {
        let alpha = f32::from(px[3]) / 255.0;
        let inv = 1.0 - alpha;
        let composite = |c: u8, b: u8| -> u8 {
            (f32::from(c).mul_add(alpha, f32::from(b) * inv))
                .round()
                .clamp(0.0, 255.0) as u8
        };
        rgb.push(composite(px[0], br));
        rgb.push(composite(px[1], bg_g));
        rgb.push(composite(px[2], bb));
    }

    let mut out = Vec::with_capacity((final_w as usize) * (final_h as usize));
    let encoder = JpegEncoder::new_with_quality(&mut out, options.quality as u8);
    encoder
        .write_image(&rgb, final_w, final_h, ColorType::Rgb8.into())
        .map_err(|e| JpegExportError::Encode(e.to_string()))?;
    Ok(out)
}

/// Render `scene` into a JPEG file at `output_path`.
pub fn export_jpeg(
    scene: &Scene,
    options: &JpegExportOptions,
    output_path: &Path,
) -> Result<(), JpegExportError> {
    let bytes = export_jpeg_to_bytes(scene, options)?;
    std::fs::write(output_path, bytes)?;
    Ok(())
}

fn scaled_dim(base: u32, scale: f32) -> u32 {
    let f = f64::from(base) * f64::from(scale);
    f.round().clamp(1.0, f64::from(u32::MAX)) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use kcreate_renderer::geometry::Color;

    fn empty_scene() -> Scene {
        Scene::new(Color::rgba(1.0, 1.0, 1.0, 1.0))
    }

    #[test]
    fn rejects_zero_dimensions() {
        let opts = JpegExportOptions {
            width: 0,
            ..JpegExportOptions::default()
        };
        let err = export_jpeg_to_bytes(&empty_scene(), &opts).expect_err("must err");
        assert!(matches!(err, JpegExportError::InvalidDimensions));
    }

    #[test]
    fn rejects_zero_quality() {
        let opts = JpegExportOptions {
            quality: 0,
            ..JpegExportOptions::default()
        };
        let err = export_jpeg_to_bytes(&empty_scene(), &opts).expect_err("must err");
        assert!(matches!(err, JpegExportError::InvalidQuality(0)));
    }

    #[test]
    fn empty_scene_emits_valid_jpeg_header() {
        let opts = JpegExportOptions {
            width: 32,
            height: 16,
            scale: 1.0,
            quality: 80,
            background: Some(Color::rgba(0.0, 0.0, 0.0, 1.0)),
        };
        let bytes = export_jpeg_to_bytes(&empty_scene(), &opts).expect("jpeg");
        // JPEG SOI marker.
        assert_eq!(&bytes[0..2], &[0xFF, 0xD8]);
    }
}
