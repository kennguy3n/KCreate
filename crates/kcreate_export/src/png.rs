//! PNG export — render a scene through the offscreen wgpu pipeline,
//! then encode the resulting RGBA8 frame to PNG via the `image` crate.
//!
//! `export_png_to_bytes` keeps everything in memory; `export_png` writes
//! directly to disk via [`std::fs`].

use std::path::Path;

use image::{codecs::png::PngEncoder, ColorType, ImageEncoder};
use kcreate_renderer::geometry::Color;
use kcreate_renderer::{initialize, Scene};
use thiserror::Error;

/// Errors from PNG export.
#[derive(Debug, Error)]
pub enum PngExportError {
    #[error("invalid dimensions: width and height must both be positive")]
    InvalidDimensions,
    #[error("invalid scale: {0}; must be > 0 and finite")]
    InvalidScale(f32),
    #[error(transparent)]
    Renderer(#[from] kcreate_renderer::RendererError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("encoding failed: {0}")]
    Encode(String),
    #[error("no frame published")]
    NoFrame,
}

/// PNG export options. `width` × `height` × `scale` defines the final
/// raster size.
#[derive(Debug, Clone, Copy)]
pub struct PngExportOptions {
    pub width: u32,
    pub height: u32,
    pub scale: f32,
    /// Background color. `None` means transparent.
    pub background: Option<Color>,
}

impl Default for PngExportOptions {
    fn default() -> Self {
        Self {
            width: 512,
            height: 512,
            scale: 1.0,
            background: None,
        }
    }
}

/// Render `scene` into PNG bytes.
pub fn export_png_to_bytes(
    scene: &Scene,
    options: &PngExportOptions,
) -> Result<Vec<u8>, PngExportError> {
    if options.width == 0 || options.height == 0 {
        return Err(PngExportError::InvalidDimensions);
    }
    if !options.scale.is_finite() || options.scale <= 0.0 {
        return Err(PngExportError::InvalidScale(options.scale));
    }

    let final_w = scaled_dim(options.width, options.scale);
    let final_h = scaled_dim(options.height, options.scale);

    let scene = apply_background(scene.clone(), options.background);

    // The renderer is the source of truth: build a fresh offscreen
    // context at the export size, render once, then read the latest
    // frame back. We don't reuse the singleton bridge renderer because
    // exports happen at arbitrary scales independent of the UI.
    let ctx = initialize(final_w, final_h)?;
    let _frame_id = ctx.render_frame(&scene)?;
    let rgba: Vec<u8> = {
        let frame = ctx.latest_frame().ok_or(PngExportError::NoFrame)?;
        frame.pixels().to_vec()
    };

    let mut out = Vec::with_capacity((final_w as usize) * (final_h as usize) * 4);
    let encoder = PngEncoder::new(&mut out);
    encoder
        .write_image(&rgba, final_w, final_h, ColorType::Rgba8.into())
        .map_err(|e| PngExportError::Encode(e.to_string()))?;
    Ok(out)
}

/// Render `scene` into a PNG file at `output_path`.
pub fn export_png(
    scene: &Scene,
    options: &PngExportOptions,
    output_path: &Path,
) -> Result<(), PngExportError> {
    let bytes = export_png_to_bytes(scene, options)?;
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
    use kcreate_renderer::geometry::{Color, Rect, Style};
    use kcreate_renderer::scene::{Object, ObjectKind};

    fn empty_scene() -> Scene {
        Scene::new(Color::rgba(1.0, 1.0, 1.0, 1.0))
    }

    fn rect_scene() -> Scene {
        let mut s = Scene::new(Color::rgba(0.0, 0.0, 0.0, 1.0));
        s.add_object(Object::new(
            ObjectKind::Rect(Rect::new(4.0, 4.0, 8.0, 8.0)),
            Style {
                fill: Some(Color::rgba(1.0, 0.0, 0.0, 1.0)),
                stroke: None,
            },
        ));
        s
    }

    #[test]
    fn rejects_zero_dimensions() {
        let opts = PngExportOptions {
            width: 0,
            ..PngExportOptions::default()
        };
        let err = export_png_to_bytes(&empty_scene(), &opts).expect_err("must err");
        assert!(matches!(err, PngExportError::InvalidDimensions));
    }

    #[test]
    fn rejects_non_positive_scale() {
        let opts = PngExportOptions {
            scale: 0.0,
            ..PngExportOptions::default()
        };
        let err = export_png_to_bytes(&empty_scene(), &opts).expect_err("must err");
        assert!(matches!(err, PngExportError::InvalidScale(_)));
    }

    #[test]
    fn empty_scene_emits_valid_png_header() {
        let opts = PngExportOptions {
            width: 32,
            height: 16,
            scale: 1.0,
            background: Some(Color::rgba(0.0, 0.0, 0.0, 1.0)),
        };
        let bytes = export_png_to_bytes(&empty_scene(), &opts).expect("png");
        assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"), "PNG signature");
    }

    #[test]
    fn rect_scene_emits_valid_png_with_dimensions() {
        let opts = PngExportOptions {
            width: 32,
            height: 16,
            scale: 1.0,
            background: Some(Color::rgba(0.0, 0.0, 0.0, 1.0)),
        };
        let bytes = export_png_to_bytes(&rect_scene(), &opts).expect("png");
        let decoded = image::load_from_memory(&bytes).expect("decode png");
        assert_eq!(decoded.width(), 32);
        assert_eq!(decoded.height(), 16);
    }

    #[test]
    fn scale_multiplies_output_dimensions() {
        let opts = PngExportOptions {
            width: 16,
            height: 16,
            scale: 2.0,
            background: Some(Color::rgba(0.0, 0.0, 0.0, 1.0)),
        };
        let bytes = export_png_to_bytes(&empty_scene(), &opts).expect("png");
        let decoded = image::load_from_memory(&bytes).expect("decode png");
        assert_eq!(decoded.width(), 32);
        assert_eq!(decoded.height(), 32);
    }

    #[test]
    fn export_png_writes_file() {
        let opts = PngExportOptions {
            width: 16,
            height: 16,
            scale: 1.0,
            background: Some(Color::rgba(0.0, 0.0, 0.0, 1.0)),
        };
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("out.png");
        export_png(&empty_scene(), &opts, &path).expect("write");
        let on_disk = std::fs::read(&path).expect("read");
        assert!(on_disk.starts_with(b"\x89PNG\r\n\x1a\n"));
    }
}
