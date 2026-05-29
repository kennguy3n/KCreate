//! SVG-to-raster preview rendering (Phase 9 Task 16).
//!
//! Uses `resvg` to rasterise an SVG byte buffer into a PNG. The
//! crate is pure Rust (no native deps), and chains
//! `usvg` (already in the workspace) for parsing + `tiny-skia`
//! for the actual rasteriser. The result is suitable for both
//! the thumbnail pipeline and the Export panel preview.

use image::ImageEncoder;
use resvg::tiny_skia::Pixmap;
use resvg::usvg::{self, Transform};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SvgPreviewError {
    #[error("invalid SVG document: {0}")]
    InvalidSvg(String),
    #[error("svg dimensions resolve to zero")]
    ZeroDimensions,
    #[error("pixmap allocation failed at {width}x{height}")]
    PixmapAlloc { width: u32, height: u32 },
    #[error("png encode failed: {0}")]
    Png(#[from] image::ImageError),
    #[error("requested preview size is invalid ({width}x{height})")]
    InvalidRequest { width: u32, height: u32 },
}

/// Options controlling SVG → raster conversion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SvgPreviewOptions {
    /// Maximum width of the output PNG in pixels.
    pub max_width: u32,
    /// Maximum height of the output PNG in pixels.
    pub max_height: u32,
    /// If true, render against a transparent background; otherwise
    /// composite against opaque white (matches Export panel
    /// thumbnails).
    pub transparent: bool,
}

impl Default for SvgPreviewOptions {
    fn default() -> Self {
        Self {
            max_width: 512,
            max_height: 512,
            transparent: false,
        }
    }
}

/// Rasterised preview.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SvgPreview {
    pub width: u32,
    pub height: u32,
    /// PNG-encoded bytes.
    pub png_bytes: Vec<u8>,
}

/// Rasterise `svg_bytes` to a PNG byte buffer that fits within
/// `(max_width, max_height)` while preserving aspect ratio.
pub fn svg_to_raster_preview(
    svg_bytes: &[u8],
    opts: &SvgPreviewOptions,
) -> Result<SvgPreview, SvgPreviewError> {
    if opts.max_width == 0 || opts.max_height == 0 {
        return Err(SvgPreviewError::InvalidRequest {
            width: opts.max_width,
            height: opts.max_height,
        });
    }
    let tree = usvg::Tree::from_data(svg_bytes, &usvg::Options::default())
        .map_err(|e| SvgPreviewError::InvalidSvg(e.to_string()))?;
    let size = tree.size();
    let src_w = size.width();
    let src_h = size.height();
    if src_w <= 0.0 || src_h <= 0.0 {
        return Err(SvgPreviewError::ZeroDimensions);
    }
    let aspect = src_w / src_h;
    let (out_w, out_h) = if src_w / (opts.max_width as f32) > src_h / (opts.max_height as f32) {
        let w = opts.max_width;
        let h = ((w as f32) / aspect).round().max(1.0) as u32;
        (w, h.min(opts.max_height))
    } else {
        let h = opts.max_height;
        let w = ((h as f32) * aspect).round().max(1.0) as u32;
        (w.min(opts.max_width), h)
    };
    let mut pixmap = Pixmap::new(out_w, out_h).ok_or(SvgPreviewError::PixmapAlloc {
        width: out_w,
        height: out_h,
    })?;
    if !opts.transparent {
        // Tiny-skia uses straight RGBA; fill with opaque white.
        pixmap.fill(resvg::tiny_skia::Color::from_rgba(1.0, 1.0, 1.0, 1.0).unwrap());
    }
    let transform = Transform::from_scale((out_w as f32) / src_w, (out_h as f32) / src_h);
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    let mut out = Vec::with_capacity((out_w as usize) * (out_h as usize) * 4);
    let encoder = image::codecs::png::PngEncoder::new(&mut out);
    encoder.write_image(pixmap.data(), out_w, out_h, image::ExtendedColorType::Rgba8)?;
    Ok(SvgPreview {
        width: out_w,
        height: out_h,
        png_bytes: out,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED_SQUARE_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="50" viewBox="0 0 100 50">
        <rect x="0" y="0" width="100" height="50" fill="#ff0000"/>
    </svg>"##;

    #[test]
    fn renders_simple_svg() {
        let preview = svg_to_raster_preview(RED_SQUARE_SVG, &SvgPreviewOptions::default()).unwrap();
        // Aspect 100:50 = 2:1 → 512x256 (height-bound) or 512xN.
        assert!(preview.width > 0);
        assert!(preview.height > 0);
        assert!(preview.width >= preview.height * 2 - 2);
        // Real PNG signature.
        assert_eq!(&preview.png_bytes[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
    }

    #[test]
    fn rejects_zero_max() {
        let err = svg_to_raster_preview(
            RED_SQUARE_SVG,
            &SvgPreviewOptions {
                max_width: 0,
                max_height: 0,
                transparent: false,
            },
        )
        .unwrap_err();
        assert!(matches!(err, SvgPreviewError::InvalidRequest { .. }));
    }

    #[test]
    fn rejects_invalid_svg() {
        let err = svg_to_raster_preview(b"<not-svg/>", &SvgPreviewOptions::default()).unwrap_err();
        assert!(matches!(err, SvgPreviewError::InvalidSvg(_)));
    }

    #[test]
    fn respects_max_dims_for_tall_svg() {
        let tall_svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="50" height="200" viewBox="0 0 50 200">
            <rect fill="#000" width="50" height="200"/>
        </svg>"##;
        let preview = svg_to_raster_preview(
            tall_svg,
            &SvgPreviewOptions {
                max_width: 64,
                max_height: 64,
                transparent: false,
            },
        )
        .unwrap();
        assert!(preview.width <= 64);
        assert!(preview.height <= 64);
        // 50:200 = 1:4 → height-bound at 64, width = 16.
        assert!(preview.height >= preview.width * 4 - 4);
    }
}
