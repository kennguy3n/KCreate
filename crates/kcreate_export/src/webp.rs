//! WebP export — render a scene through the offscreen wgpu pipeline,
//! then encode the resulting RGBA8 frame to WebP via the `image` crate.
//!
//! WebP is rendered through the same offscreen renderer as PNG so
//! background, scale, and dimension handling stay identical. The
//! difference is the encoder choice — `image::codecs::webp::WebPEncoder`.
//! The `image` crate's bundled encoder produces **lossless** WebP only
//! (libwebp's lossy path is not wired up); we surface a `lossless: bool`
//! flag for forward-compatibility but it always encodes losslessly today,
//! and the `quality` parameter is therefore ignored. The flag is
//! retained on the wire so that swapping the encoder for `webp` (the
//! libwebp wrapper crate) in Phase 1 doesn't require a breaking API
//! change.

use std::path::Path;

use image::{codecs::webp::WebPEncoder, ColorType, ImageEncoder};
use kcreate_renderer::geometry::Color;
use kcreate_renderer::{initialize, Scene, Vec2};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::png::PngExportError;

/// Errors from WebP export.
#[derive(Debug, Error)]
pub enum WebpExportError {
    #[error("invalid dimensions: width and height must both be positive")]
    InvalidDimensions,
    #[error("invalid scale: {0}; must be > 0 and finite")]
    InvalidScale(f32),
    #[error("invalid quality: {0}; must be in 0..=100")]
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

/// Map our shared renderer / IO errors over from the PNG pipeline so
/// the bridge can lift a `WebpExportError` straight from a generic
/// scene-render result without two separate match arms.
impl From<PngExportError> for WebpExportError {
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

/// WebP export options.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WebpExportOptions {
    pub width: u32,
    pub height: u32,
    pub scale: f32,
    /// 0..=100. Currently ignored — the bundled encoder is lossless.
    pub quality: u32,
    /// Reserved for the Phase 1 `webp`-crate-backed encoder. Always
    /// treated as `true` today.
    pub lossless: bool,
    pub background: Option<Color>,
}

impl Default for WebpExportOptions {
    fn default() -> Self {
        Self {
            width: 512,
            height: 512,
            scale: 1.0,
            quality: 90,
            lossless: true,
            background: None,
        }
    }
}

/// Render `scene` into WebP bytes.
pub fn export_webp_to_bytes(
    scene: &Scene,
    options: &WebpExportOptions,
) -> Result<Vec<u8>, WebpExportError> {
    if options.width == 0 || options.height == 0 {
        return Err(WebpExportError::InvalidDimensions);
    }
    if !options.scale.is_finite() || options.scale <= 0.0 {
        return Err(WebpExportError::InvalidScale(options.scale));
    }
    if options.quality > 100 {
        return Err(WebpExportError::InvalidQuality(options.quality));
    }

    let final_w = scaled_dim(options.width, options.scale);
    let final_h = scaled_dim(options.height, options.scale);

    let scene = apply_background(scene.clone(), options.background);

    let ctx = initialize(final_w, final_h)?;
    // Supersample at `scale`: zoom the viewport so scene units map to
    // `scale` pixels each, filling the enlarged buffer (see png.rs).
    ctx.set_viewport(Vec2::ZERO, options.scale);
    let _frame_id = ctx.render_frame(&scene)?;
    let rgba: Vec<u8> = {
        let frame = ctx.latest_frame().ok_or(WebpExportError::NoFrame)?;
        frame.pixels().to_vec()
    };

    let mut out = Vec::with_capacity((final_w as usize) * (final_h as usize) * 2);
    let encoder = WebPEncoder::new_lossless(&mut out);
    encoder
        .write_image(&rgba, final_w, final_h, ColorType::Rgba8.into())
        .map_err(|e| WebpExportError::Encode(e.to_string()))?;
    Ok(out)
}

/// Render `scene` into a WebP file at `output_path`.
pub fn export_webp(
    scene: &Scene,
    options: &WebpExportOptions,
    output_path: &Path,
) -> Result<(), WebpExportError> {
    let bytes = export_webp_to_bytes(scene, options)?;
    std::fs::write(output_path, bytes)?;
    Ok(())
}

fn scaled_dim(base: u32, scale: f32) -> u32 {
    let f = f64::from(base) * f64::from(scale);
    f.round().clamp(1.0, f64::from(u32::MAX)) as u32
}

const fn apply_background(mut scene: Scene, bg: Option<Color>) -> Scene {
    if let Some(c) = bg {
        scene.clear_color = c;
    }
    scene
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
        let opts = WebpExportOptions {
            width: 0,
            ..WebpExportOptions::default()
        };
        let err = export_webp_to_bytes(&empty_scene(), &opts).expect_err("must err");
        assert!(matches!(err, WebpExportError::InvalidDimensions));
    }

    #[test]
    fn rejects_invalid_quality() {
        let opts = WebpExportOptions {
            quality: 101,
            ..WebpExportOptions::default()
        };
        let err = export_webp_to_bytes(&empty_scene(), &opts).expect_err("must err");
        assert!(matches!(err, WebpExportError::InvalidQuality(101)));
    }

    #[test]
    fn empty_scene_emits_valid_webp_header() {
        let opts = WebpExportOptions {
            width: 32,
            height: 16,
            scale: 1.0,
            quality: 90,
            lossless: true,
            background: Some(Color::rgba(0.0, 0.0, 0.0, 1.0)),
        };
        let bytes = export_webp_to_bytes(&empty_scene(), &opts).expect("webp");
        // RIFF container header.
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WEBP");
    }

    #[test]
    fn scale_supersamples_content_instead_of_padding() {
        use kcreate_renderer::geometry::{Rect, Style};
        use kcreate_renderer::scene::{Object, ObjectKind};

        let mut scene = Scene::new(Color::rgba(0.0, 0.0, 0.0, 1.0));
        scene.add_object(Object::new(
            ObjectKind::Rect(Rect::new(0.0, 0.0, 16.0, 16.0)),
            Style {
                fill: Some(Color::rgba(1.0, 0.0, 0.0, 1.0)),
                stroke: None,
            },
        ));
        let opts = WebpExportOptions {
            width: 16,
            height: 16,
            scale: 2.0,
            quality: 90,
            lossless: true,
            background: Some(Color::rgba(0.0, 0.0, 0.0, 1.0)),
        };
        let bytes = export_webp_to_bytes(&scene, &opts).expect("webp");
        let decoded = image::load_from_memory(&bytes)
            .expect("decode webp")
            .to_rgba8();
        assert_eq!(decoded.dimensions(), (32, 32));
        // The far corner would be empty padding without the viewport-zoom fix.
        let corner = decoded.get_pixel(31, 31);
        assert!(
            corner[0] > 200 && corner[1] < 60 && corner[2] < 60,
            "far corner must be the supersampled fill, got {corner:?}"
        );
    }
}
