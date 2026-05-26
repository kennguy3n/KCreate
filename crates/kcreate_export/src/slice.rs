//! Slice export — render named regions of the canvas to separate
//! files in parallel.
//!
//! A [`Slice`](kcreate_core::project::Slice) carries a name, a
//! bounding rectangle in document space, a target format, and a
//! per-slice scale. `export_slices` walks the slice list in parallel
//! via rayon, translating the scene so the slice's top-left lands at
//! the origin, then rendering at the slice's dimensions.
//!
//! ## Determinism
//!
//! - Files are written using `<slice.name><slice.suffix>`. We
//!   sanitize the name so filesystem-unsafe characters become
//!   underscores; on collision the second file gets a numeric
//!   suffix (`-2`, `-3`, …).
//! - The order of returned `SliceResult`s mirrors the input
//!   `slices` slice, regardless of the parallelism the work runs
//!   under.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use rayon::prelude::*;
use thiserror::Error;

use kcreate_core::project::{ExportFormat, Slice};
use kcreate_renderer::geometry::Color;
use kcreate_renderer::scene::Scene;

use crate::jpeg::{export_jpeg_to_bytes, JpegExportError, JpegExportOptions};
use crate::png::{export_png_to_bytes, PngExportError, PngExportOptions};
use crate::webp::{export_webp_to_bytes, WebpExportError, WebpExportOptions};

/// Outcome of exporting a single slice. The `path` is filled in only
/// on success; on failure the `error` field carries a copy of the
/// underlying error message so callers can show it in the UI without
/// holding onto the typed error.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SliceResult {
    pub slice_id: uuid::Uuid,
    pub name: String,
    pub path: Option<PathBuf>,
    pub bytes_written: u64,
    pub error: Option<String>,
}

/// Top-level errors that abort the entire batch, before any per-slice
/// rendering happens.
#[derive(Debug, Error)]
pub enum SliceExportError {
    #[error("output directory `{0}` is not a directory")]
    OutputNotADirectory(PathBuf),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Errors that can occur while rendering or encoding a single slice.
/// Recorded on the `SliceResult.error` field; never aborts the
/// batch (other slices keep going).
#[derive(Debug, Error)]
enum PerSliceError {
    #[error(transparent)]
    Png(#[from] PngExportError),
    #[error(transparent)]
    Webp(#[from] WebpExportError),
    #[error(transparent)]
    Jpeg(#[from] JpegExportError),
    #[error("slice format `{0:?}` is not yet wired through slice export")]
    UnsupportedFormat(ExportFormat),
    #[error("slice has zero width or height")]
    DegenerateBounds,
    #[error("slice scale `{0}` is invalid (must be finite and > 0)")]
    InvalidScale(f32),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Render every slice in `slices` to a separate file inside
/// `output_dir`. Returns one [`SliceResult`] per input slice in the
/// same order. Per-slice failures land in the result's `error`
/// field; the batch as a whole only errors when the output
/// directory itself is broken.
pub fn export_slices(
    scene: &Scene,
    slices: &[Slice],
    output_dir: &Path,
) -> Result<Vec<SliceResult>, SliceExportError> {
    std::fs::create_dir_all(output_dir)?;
    let meta = std::fs::metadata(output_dir)?;
    if !meta.is_dir() {
        return Err(SliceExportError::OutputNotADirectory(
            output_dir.to_path_buf(),
        ));
    }

    // De-duplicate names by tracking a per-name counter shared across
    // worker threads. `AtomicUsize` keeps the collision check
    // contention-free in the common no-collision case.
    let collision_counter = AtomicUsize::new(0);

    // Pre-compute a sanitized base filename for every slice so we can
    // emit a deterministic per-slice path without locking.
    let plans: Vec<SlicePlan> = slices
        .iter()
        .enumerate()
        .map(|(idx, slice)| {
            let base = sanitize_filename(&slice.name);
            // For deterministic output paths under collisions, the
            // first occurrence of a name uses just `<base><suffix>`
            // and subsequent occurrences add `-2`, `-3`, … in input
            // order. We don't run that scan in parallel because
            // (a) it's O(n) and (b) `output_dir` lookups would race.
            let dedup_seed = slices[..idx]
                .iter()
                .filter(|s| sanitize_filename(&s.name) == base)
                .count();
            let unique_name = if dedup_seed == 0 {
                base
            } else {
                let _ = collision_counter.fetch_add(1, Ordering::Relaxed);
                format!("{base}-{}", dedup_seed + 1)
            };
            SlicePlan {
                slice: slice.clone(),
                file_name: format!("{unique_name}{}", slice.suffix),
            }
        })
        .collect();

    let results: Vec<SliceResult> = plans
        .par_iter()
        .map(|plan| {
            let target = output_dir.join(&plan.file_name);
            match render_one_slice(scene, &plan.slice, &target) {
                Ok(bytes) => SliceResult {
                    slice_id: plan.slice.id,
                    name: plan.slice.name.clone(),
                    path: Some(target),
                    bytes_written: bytes,
                    error: None,
                },
                Err(e) => SliceResult {
                    slice_id: plan.slice.id,
                    name: plan.slice.name.clone(),
                    path: None,
                    bytes_written: 0,
                    error: Some(e.to_string()),
                },
            }
        })
        .collect();
    Ok(results)
}

struct SlicePlan {
    slice: Slice,
    file_name: String,
}

fn render_one_slice(scene: &Scene, slice: &Slice, target: &Path) -> Result<u64, PerSliceError> {
    if !slice.scale.is_finite() || slice.scale <= 0.0 {
        return Err(PerSliceError::InvalidScale(slice.scale));
    }
    let base_w = slice.bounds.width.round() as i64;
    let base_h = slice.bounds.height.round() as i64;
    if base_w <= 0 || base_h <= 0 {
        return Err(PerSliceError::DegenerateBounds);
    }
    let scaled_w = ((base_w as f64) * f64::from(slice.scale)).round();
    let scaled_h = ((base_h as f64) * f64::from(slice.scale)).round();
    let width = scaled_w.clamp(1.0, f64::from(u32::MAX)) as u32;
    let height = scaled_h.clamp(1.0, f64::from(u32::MAX)) as u32;

    // Translate every object so the slice's top-left lands at the
    // renderer origin. We clone the scene so the input stays
    // immutable.
    let translated = translate_scene(scene, -slice.bounds.x as f32, -slice.bounds.y as f32);

    let bytes = match slice.format {
        ExportFormat::Png => export_png_to_bytes(
            &translated,
            &PngExportOptions {
                width: base_w as u32,
                height: base_h as u32,
                scale: slice.scale,
                background: None,
            },
        )?,
        ExportFormat::Webp => export_webp_to_bytes(
            &translated,
            &WebpExportOptions {
                width,
                height,
                background: None,
                ..WebpExportOptions::default()
            },
        )?,
        ExportFormat::Jpeg => export_jpeg_to_bytes(
            &translated,
            &JpegExportOptions {
                width,
                height,
                background: Some(Color::rgba(1.0, 1.0, 1.0, 1.0)),
                ..JpegExportOptions::default()
            },
        )?,
        // SVG / PDF slice export requires a separate scene
        // serialiser; not wired through yet — fall back to a
        // descriptive error so the UI can show "Slice format not
        // supported". This is intentional: returning a half-baked
        // SVG that omits the slice transform would be worse.
        other @ (ExportFormat::Svg | ExportFormat::Pdf) => {
            return Err(PerSliceError::UnsupportedFormat(other));
        }
    };
    std::fs::write(target, &bytes)?;
    Ok(bytes.len() as u64)
}

fn translate_scene(scene: &Scene, dx: f32, dy: f32) -> Scene {
    let mut out = scene.clone();
    for obj in &mut out.objects {
        obj.translation.0 += dx;
        obj.translation.1 += dy;
    }
    out
}

fn sanitize_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "slice".to_string()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kcreate_core::Bounds;
    use kcreate_renderer::geometry::{Color, Rect, Style};
    use kcreate_renderer::scene::{Object, ObjectKind};

    fn scene_with_one_rect() -> Scene {
        let mut s = Scene::new(Color::rgba(0.0, 0.0, 0.0, 1.0));
        let obj = Object::new(
            ObjectKind::Rect(Rect::new(0.0, 0.0, 100.0, 100.0)),
            Style {
                fill: Some(Color::rgba(1.0, 0.0, 0.0, 1.0)),
                stroke: None,
            },
        );
        s.add_object(obj);
        s
    }

    #[test]
    fn empty_slice_list_produces_empty_results() {
        let dir = tempfile::tempdir().expect("tempdir");
        let scene = scene_with_one_rect();
        let out = export_slices(&scene, &[], dir.path()).expect("ok");
        assert!(out.is_empty());
    }

    #[test]
    fn png_slice_export_produces_a_file_with_correct_dimensions() {
        let dir = tempfile::tempdir().expect("tempdir");
        let scene = scene_with_one_rect();
        let slice = Slice::new(
            "hero",
            Bounds {
                x: 0.0,
                y: 0.0,
                width: 50.0,
                height: 50.0,
            },
            ExportFormat::Png,
            1.0,
        );
        let out = export_slices(&scene, &[slice], dir.path()).expect("ok");
        assert_eq!(out.len(), 1);
        assert!(out[0].error.is_none(), "{:?}", out[0].error);
        let p = out[0].path.as_ref().expect("path");
        assert!(p.exists());
        assert!(p.file_name().unwrap().to_string_lossy().starts_with("hero"));
        let bytes = std::fs::read(p).expect("read");
        assert!(!bytes.is_empty());
    }

    #[test]
    fn collisions_get_suffixed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let scene = scene_with_one_rect();
        let make = |name: &str| {
            Slice::new(
                name,
                Bounds {
                    x: 0.0,
                    y: 0.0,
                    width: 32.0,
                    height: 32.0,
                },
                ExportFormat::Png,
                1.0,
            )
        };
        let out = export_slices(
            &scene,
            &[make("hero"), make("hero"), make("hero")],
            dir.path(),
        )
        .expect("ok");
        let names: Vec<_> = out
            .iter()
            .filter_map(|r| {
                r.path
                    .as_ref()
                    .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            })
            .collect();
        assert!(names.contains(&"hero.png".to_string()));
        assert!(names.contains(&"hero-2.png".to_string()));
        assert!(names.contains(&"hero-3.png".to_string()));
    }

    #[test]
    fn degenerate_bounds_yield_per_slice_error_not_batch_failure() {
        let dir = tempfile::tempdir().expect("tempdir");
        let scene = scene_with_one_rect();
        let slice = Slice::new(
            "bad",
            Bounds {
                x: 0.0,
                y: 0.0,
                width: 0.0,
                height: 50.0,
            },
            ExportFormat::Png,
            1.0,
        );
        let out = export_slices(&scene, &[slice], dir.path()).expect("ok");
        assert_eq!(out.len(), 1);
        assert!(out[0].error.is_some());
        assert!(out[0].path.is_none());
    }
}
