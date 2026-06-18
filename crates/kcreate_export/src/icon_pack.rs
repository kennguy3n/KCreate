//! Icon pack generator.
//!
//! Builds Web / iOS / Android / Favicon icon sets from a rendered
//! scene plus the originating document graph. Each [`IconSize`]
//! either rasterises through the offscreen wgpu pipeline
//! ([`crate::png::export_png_to_bytes`]) or emits scalable SVG
//! ([`crate::svg::export_svg_from_document`]) — no other pipelines
//! are involved, so the output always matches the in-app canvas.
//!
//! ICO bundling (single `.ico` file containing multiple sizes) is
//! deferred to Phase 3; the current `IconFormat::Ico` variant writes
//! individual PNGs alongside a sidecar manifest listing them.

use std::path::{Path, PathBuf};

use kcreate_core::document::DocumentGraph;
use kcreate_renderer::Scene;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::png::{export_png_to_bytes, PngExportError, PngExportOptions};
use crate::svg::{export_svg_from_document, SvgDocumentExportError, SvgExportOptions};

/// Output container for an icon pack run. Files are returned in
/// memory; the caller (typically the export bridge) writes them to
/// `output_dir`.
#[derive(Debug, Clone)]
pub struct IconPackResult {
    pub files: Vec<(PathBuf, Vec<u8>)>,
}

/// Raster output format for an [`IconSize`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IconFormat {
    Png,
    Svg,
    /// Individual PNG (true ICO bundling deferred to Phase 3).
    Ico,
}

/// A single size in an icon pack.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(clippy::derive_partial_eq_without_eq)]
pub struct IconSize {
    pub width: u32,
    pub height: u32,
    pub scale: f32,
    /// Filename suffix (e.g. `"@2x"`, `"-48x48"`). Empty for the
    /// primary size.
    pub suffix: String,
    pub format: IconFormat,
}

/// Named group of icon sizes targeting one platform.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(clippy::derive_partial_eq_without_eq)]
pub struct IconPackPlatform {
    pub name: String,
    pub sizes: Vec<IconSize>,
}

/// Errors from [`generate_icon_pack`].
#[derive(Debug, Error)]
pub enum IconPackError {
    #[error("png: {0}")]
    Png(#[from] PngExportError),
    #[error("svg: {0}")]
    Svg(#[from] SvgDocumentExportError),
    #[error("no platforms specified")]
    NoPlatforms,
    #[error("platform '{name}' has no sizes")]
    EmptyPlatform { name: String },
    #[error("invalid icon size for platform '{platform}': {reason}")]
    InvalidSize { platform: String, reason: String },
}

/// Generate every icon in `platforms` for `scene` / `document`.
///
/// PNG sizes go through the renderer at the requested width/height.
/// SVG sizes emit the full document as scalable vector. The output
/// files are returned in memory; the caller writes them to
/// `output_dir`.
pub fn generate_icon_pack(
    scene: &Scene,
    document: &DocumentGraph,
    node_ids: &[Uuid],
    platforms: &[IconPackPlatform],
    output_dir: &Path,
) -> Result<IconPackResult, IconPackError> {
    if platforms.is_empty() {
        return Err(IconPackError::NoPlatforms);
    }
    let mut files: Vec<(PathBuf, Vec<u8>)> = Vec::new();
    for platform in platforms {
        if platform.sizes.is_empty() {
            return Err(IconPackError::EmptyPlatform {
                name: platform.name.clone(),
            });
        }
        let platform_dir = output_dir.join(&platform.name);
        for size in &platform.sizes {
            validate_size(&platform.name, size)?;
            let (filename, bytes) = render_one(scene, document, node_ids, &platform.name, size)?;
            files.push((platform_dir.join(filename), bytes));
        }
    }
    Ok(IconPackResult { files })
}

fn validate_size(platform_name: &str, size: &IconSize) -> Result<(), IconPackError> {
    if size.width == 0 || size.height == 0 {
        return Err(IconPackError::InvalidSize {
            platform: platform_name.to_string(),
            reason: "width and height must be > 0".to_string(),
        });
    }
    if !size.scale.is_finite() || size.scale <= 0.0 {
        return Err(IconPackError::InvalidSize {
            platform: platform_name.to_string(),
            reason: format!("scale must be > 0 and finite (got {})", size.scale),
        });
    }
    Ok(())
}

fn render_one(
    scene: &Scene,
    document: &DocumentGraph,
    node_ids: &[Uuid],
    platform_name: &str,
    size: &IconSize,
) -> Result<(String, Vec<u8>), IconPackError> {
    let final_w = scaled_dim(size.width, size.scale);
    let final_h = scaled_dim(size.height, size.scale);
    let base_name = format!(
        "{platform}-{w}x{h}{suffix}",
        platform = platform_name,
        w = final_w,
        h = final_h,
        suffix = size.suffix,
    );
    match size.format {
        IconFormat::Png | IconFormat::Ico => {
            let opts = PngExportOptions {
                width: size.width,
                height: size.height,
                scale: size.scale,
                background: None,
            };
            let bytes = export_png_to_bytes(scene, &opts)?;
            Ok((format!("{base_name}.png"), bytes))
        }
        IconFormat::Svg => {
            let opts = SvgExportOptions {
                width: f64::from(final_w),
                height: f64::from(final_h),
                ..SvgExportOptions::default()
            };
            let svg = export_svg_from_document(document, node_ids, &opts)?;
            Ok((format!("{base_name}.svg"), svg.into_bytes()))
        }
    }
}

fn scaled_dim(base: u32, scale: f32) -> u32 {
    let f = f64::from(base) * f64::from(scale);
    f.round().clamp(1.0, f64::from(u32::MAX)) as u32
}

/// Built-in platform presets covering the four targets specified in
/// OVERVIEW.md §4.7 (Export Center).
#[must_use]
pub fn built_in_platforms() -> Vec<IconPackPlatform> {
    vec![
        web_platform(),
        ios_platform(),
        android_platform(),
        favicon_platform(),
    ]
}

/// Stable list of built-in platform presets. Useful for the UI's
/// "select all" affordance.
pub const BUILT_IN_PLATFORMS: &[&str] = &["web", "ios", "android", "favicon"];

fn web_platform() -> IconPackPlatform {
    let png_sizes = [16, 32, 48, 64, 128, 256, 512];
    let mut sizes: Vec<IconSize> = png_sizes
        .iter()
        .map(|&px| IconSize {
            width: px,
            height: px,
            scale: 1.0,
            suffix: String::new(),
            format: IconFormat::Png,
        })
        .collect();
    sizes.push(IconSize {
        width: 512,
        height: 512,
        scale: 1.0,
        suffix: "-vector".to_string(),
        format: IconFormat::Svg,
    });
    IconPackPlatform {
        name: "web".to_string(),
        sizes,
    }
}

fn ios_platform() -> IconPackPlatform {
    // Sizes from Apple's "App Icon" matrix (Phone + iPad + App Store).
    let sizes_px: [u32; 13] = [20, 29, 40, 58, 60, 76, 80, 87, 120, 152, 167, 180, 1024];
    IconPackPlatform {
        name: "ios".to_string(),
        sizes: sizes_px
            .iter()
            .map(|&px| IconSize {
                width: px,
                height: px,
                scale: 1.0,
                suffix: String::new(),
                format: IconFormat::Png,
            })
            .collect(),
    }
}

fn android_platform() -> IconPackPlatform {
    let densities: [(&str, u32); 5] = [
        ("mdpi", 48),
        ("hdpi", 72),
        ("xhdpi", 96),
        ("xxhdpi", 144),
        ("xxxhdpi", 192),
    ];
    IconPackPlatform {
        name: "android".to_string(),
        sizes: densities
            .iter()
            .map(|(name, px)| IconSize {
                width: *px,
                height: *px,
                scale: 1.0,
                suffix: format!("-{name}"),
                format: IconFormat::Png,
            })
            .collect(),
    }
}

fn favicon_platform() -> IconPackPlatform {
    let png_sizes = [16, 32, 48];
    let mut sizes: Vec<IconSize> = png_sizes
        .iter()
        .map(|&px| IconSize {
            width: px,
            height: px,
            scale: 1.0,
            suffix: String::new(),
            format: IconFormat::Png,
        })
        .collect();
    // Bundle marker — caller emits per-size PNGs; true ICO is Phase 3.
    sizes.push(IconSize {
        width: 16,
        height: 16,
        scale: 1.0,
        suffix: "-ico".to_string(),
        format: IconFormat::Ico,
    });
    IconPackPlatform {
        name: "favicon".to_string(),
        sizes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kcreate_renderer::geometry::{Color, Rect, Style};
    use kcreate_renderer::scene::{Object, ObjectKind};
    use kcreate_renderer::Scene;

    fn small_scene() -> Scene {
        let mut s = Scene::new(Color::rgba(0.0, 0.0, 0.0, 1.0));
        s.add_object(Object::new(
            ObjectKind::Rect(Rect::new(0.0, 0.0, 16.0, 16.0)),
            Style::filled(Color::rgba(1.0, 0.0, 0.0, 1.0)),
        ));
        s
    }

    #[test]
    fn rejects_no_platforms() {
        let doc = DocumentGraph::new();
        let err = generate_icon_pack(&small_scene(), &doc, &[], &[], Path::new("."))
            .expect_err("must err");
        assert!(matches!(err, IconPackError::NoPlatforms));
    }

    #[test]
    fn rejects_empty_platform() {
        let doc = DocumentGraph::new();
        let platforms = vec![IconPackPlatform {
            name: "empty".to_string(),
            sizes: Vec::new(),
        }];
        let err = generate_icon_pack(&small_scene(), &doc, &[], &platforms, Path::new("."))
            .expect_err("must err");
        assert!(matches!(err, IconPackError::EmptyPlatform { .. }));
    }

    #[test]
    fn rejects_zero_dimension() {
        let doc = DocumentGraph::new();
        let platforms = vec![IconPackPlatform {
            name: "bad".to_string(),
            sizes: vec![IconSize {
                width: 0,
                height: 16,
                scale: 1.0,
                suffix: String::new(),
                format: IconFormat::Png,
            }],
        }];
        let err = generate_icon_pack(&small_scene(), &doc, &[], &platforms, Path::new("."))
            .expect_err("must err");
        assert!(matches!(err, IconPackError::InvalidSize { .. }));
    }

    #[test]
    fn generates_web_pack_with_expected_files() {
        let doc = DocumentGraph::new();
        let platforms = vec![web_platform()];
        let result = generate_icon_pack(&small_scene(), &doc, &[], &platforms, Path::new("/tmp/x"))
            .expect("pack");
        // Web platform: 7 PNGs + 1 SVG = 8 files
        assert_eq!(result.files.len(), 8);
        let png_count = result
            .files
            .iter()
            .filter(|(p, _)| p.extension().is_some_and(|e| e == "png"))
            .count();
        let svg_count = result
            .files
            .iter()
            .filter(|(p, _)| p.extension().is_some_and(|e| e == "svg"))
            .count();
        assert_eq!(png_count, 7);
        assert_eq!(svg_count, 1);
        // Every file lives under output_dir/web/.
        for (p, _) in &result.files {
            assert!(p.starts_with("/tmp/x/web"));
        }
    }

    #[test]
    fn android_pack_has_density_suffixes() {
        let doc = DocumentGraph::new();
        let platforms = vec![android_platform()];
        let result = generate_icon_pack(&small_scene(), &doc, &[], &platforms, Path::new("/tmp/y"))
            .expect("pack");
        // mdpi 48, hdpi 72, xhdpi 96, xxhdpi 144, xxxhdpi 192
        assert_eq!(result.files.len(), 5);
        assert!(result
            .files
            .iter()
            .any(|(p, _)| p.to_string_lossy().contains("xxxhdpi")));
    }

    #[test]
    fn ios_pack_has_thirteen_sizes() {
        let doc = DocumentGraph::new();
        let platforms = vec![ios_platform()];
        let result = generate_icon_pack(&small_scene(), &doc, &[], &platforms, Path::new("/tmp/z"))
            .expect("pack");
        assert_eq!(result.files.len(), 13);
    }

    #[test]
    fn favicon_pack_includes_ico_placeholder() {
        let doc = DocumentGraph::new();
        let platforms = vec![favicon_platform()];
        let result = generate_icon_pack(&small_scene(), &doc, &[], &platforms, Path::new("/tmp/f"))
            .expect("pack");
        // 16, 32, 48 PNG + 1 ICO-placeholder PNG = 4 files
        assert_eq!(result.files.len(), 4);
        assert!(result
            .files
            .iter()
            .any(|(p, _)| p.to_string_lossy().contains("-ico")));
    }

    #[test]
    fn png_contents_have_png_signature() {
        let doc = DocumentGraph::new();
        let platforms = vec![IconPackPlatform {
            name: "tiny".to_string(),
            sizes: vec![IconSize {
                width: 8,
                height: 8,
                scale: 1.0,
                suffix: String::new(),
                format: IconFormat::Png,
            }],
        }];
        let result = generate_icon_pack(&small_scene(), &doc, &[], &platforms, Path::new("/tmp/t"))
            .expect("pack");
        let (_, bytes) = &result.files[0];
        // PNG signature: 89 50 4E 47 0D 0A 1A 0A
        assert_eq!(
            &bytes[0..8],
            &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]
        );
    }

    #[test]
    fn built_in_platforms_match_named_list() {
        let v = built_in_platforms();
        let names: Vec<&str> = v.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, BUILT_IN_PLATFORMS);
    }
}
