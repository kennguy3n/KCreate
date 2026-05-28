//! Job-first export presets.
//!
//! Maps each Home-screen job tile to a curated set of
//! [`ExportPreset`]s. The Home page in the renderer reads this
//! list when the user creates a project from a tile and auto-
//! selects the matching presets in the Export panel.
//!
//! The job-type strings are stable identifiers used in
//! `apps/desktop/renderer/src/components/HomePage` and mirrored
//! into `apps/desktop/shared/scene.ts` via the bridge.

use kcreate_core::project::{ExportFormat, ExportPreset};
use serde::{Deserialize, Serialize};

/// Stable identifiers for the job tiles surfaced on the Home
/// page. New tiles must add a variant here so the export-preset
/// mapping fails to compile if a tile is added without curating
/// its preset set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobType {
    AppOrWebsiteUi,
    LogoIconOrBrandKit,
    SocialMediaPost,
    ProductPhotoCleanup,
    PitchDeckOrProposal,
    FlyerPosterOrBrochure,
    DeveloperAssetExport,
}

impl JobType {
    /// Stable string identifier used by the renderer.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::AppOrWebsiteUi => "app_or_website_ui",
            Self::LogoIconOrBrandKit => "logo_icon_or_brand_kit",
            Self::SocialMediaPost => "social_media_post",
            Self::ProductPhotoCleanup => "product_photo_cleanup",
            Self::PitchDeckOrProposal => "pitch_deck_or_proposal",
            Self::FlyerPosterOrBrochure => "flyer_poster_or_brochure",
            Self::DeveloperAssetExport => "developer_asset_export",
        }
    }

    /// Parse from the stable string identifier. Returns `None` on
    /// an unknown id so the bridge can fall back to "no presets"
    /// gracefully.
    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        Some(match id {
            "app_or_website_ui" => Self::AppOrWebsiteUi,
            "logo_icon_or_brand_kit" => Self::LogoIconOrBrandKit,
            "social_media_post" => Self::SocialMediaPost,
            "product_photo_cleanup" => Self::ProductPhotoCleanup,
            "pitch_deck_or_proposal" => Self::PitchDeckOrProposal,
            "flyer_poster_or_brochure" => Self::FlyerPosterOrBrochure,
            "developer_asset_export" => Self::DeveloperAssetExport,
            _ => return None,
        })
    }
}

/// Container for the bridge response: the requested job plus its
/// curated preset list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobExportPresets {
    pub job_type: JobType,
    pub presets: Vec<ExportPreset>,
}

/// Build the curated preset list for `job_type`. Returns owned
/// `ExportPreset`s with freshly-generated IDs so callers can
/// mutate them (rename, retarget directory) before persisting.
#[must_use]
pub fn presets_for_job(job_type: JobType) -> JobExportPresets {
    let presets = match job_type {
        JobType::AppOrWebsiteUi => vec![
            ExportPreset::new("PNG @1x", ExportFormat::Png, 1.0),
            ExportPreset::new("PNG @2x", ExportFormat::Png, 2.0),
            ExportPreset::new("PNG @3x", ExportFormat::Png, 3.0),
            ExportPreset::new("SVG sprite", ExportFormat::Svg, 1.0),
            // CSS export piggybacks on the inspect-mode code-gen
            // pipeline; the bridge wires it through `code_gen.rs`
            // when the preset's format is SVG and the suffix is
            // ".css".
            with_suffix(
                ExportPreset::new("CSS variables", ExportFormat::Svg, 1.0),
                ".css",
            ),
        ],
        JobType::LogoIconOrBrandKit => vec![
            ExportPreset::new("SVG clean", ExportFormat::Svg, 1.0),
            with_suffix(
                ExportPreset::new("PNG favicon 16", ExportFormat::Png, 16.0 / 256.0),
                "-16.png",
            ),
            with_suffix(
                ExportPreset::new("PNG favicon 32", ExportFormat::Png, 32.0 / 256.0),
                "-32.png",
            ),
            with_suffix(
                ExportPreset::new("PNG favicon 48", ExportFormat::Png, 48.0 / 256.0),
                "-48.png",
            ),
            ExportPreset::new("iOS icon PDF", ExportFormat::Pdf, 1.0),
        ],
        JobType::SocialMediaPost => vec![
            ExportPreset::new("Instagram square 1080", ExportFormat::Png, 1.0),
            ExportPreset::new("Instagram story 1080×1920", ExportFormat::Png, 1.0),
            ExportPreset::new("Twitter / X 1200×630", ExportFormat::Png, 1.0),
            ExportPreset::new("Web preview JPEG", ExportFormat::Jpeg, 1.0),
        ],
        JobType::ProductPhotoCleanup => vec![
            ExportPreset::new("PNG transparent", ExportFormat::Png, 1.0),
            ExportPreset::new("JPEG white bg", ExportFormat::Jpeg, 1.0),
            ExportPreset::new("WebP", ExportFormat::Webp, 1.0),
        ],
        JobType::PitchDeckOrProposal => vec![
            ExportPreset::new("PDF 16:9", ExportFormat::Pdf, 1.0),
            ExportPreset::new("PDF A4", ExportFormat::Pdf, 1.0),
        ],
        JobType::FlyerPosterOrBrochure => vec![
            ExportPreset::new("PDF print 300dpi", ExportFormat::Pdf, 1.0),
            ExportPreset::new("PNG web preview", ExportFormat::Png, 1.0),
        ],
        JobType::DeveloperAssetExport => vec![
            ExportPreset::new("SVG sprite", ExportFormat::Svg, 1.0),
            ExportPreset::new("PNG @1x", ExportFormat::Png, 1.0),
            ExportPreset::new("PNG @2x", ExportFormat::Png, 2.0),
            ExportPreset::new("PNG @3x", ExportFormat::Png, 3.0),
            with_suffix(
                ExportPreset::new("CSS variables", ExportFormat::Svg, 1.0),
                ".css",
            ),
        ],
    };
    JobExportPresets { job_type, presets }
}

/// Every job type, in display order. The Home page iterates this
/// list to render the tile grid.
#[must_use]
pub fn every_job() -> Vec<JobType> {
    vec![
        JobType::AppOrWebsiteUi,
        JobType::LogoIconOrBrandKit,
        JobType::SocialMediaPost,
        JobType::ProductPhotoCleanup,
        JobType::PitchDeckOrProposal,
        JobType::FlyerPosterOrBrochure,
        JobType::DeveloperAssetExport,
    ]
}

fn with_suffix(mut preset: ExportPreset, suffix: &str) -> ExportPreset {
    preset.suffix = suffix.to_string();
    preset
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_job_returns_non_empty_presets() {
        for job in every_job() {
            let result = presets_for_job(job);
            assert!(
                !result.presets.is_empty(),
                "job {} returned no presets",
                job.id()
            );
        }
    }

    #[test]
    fn job_ids_round_trip() {
        for job in every_job() {
            let id = job.id();
            let parsed = JobType::from_id(id).expect("known id");
            assert_eq!(parsed, job);
        }
    }

    #[test]
    fn unknown_job_id_returns_none() {
        assert!(JobType::from_id("definitely-not-a-tile").is_none());
    }

    #[test]
    fn app_ui_includes_three_density_buckets() {
        let result = presets_for_job(JobType::AppOrWebsiteUi);
        let png_count = result
            .presets
            .iter()
            .filter(|p| p.format == ExportFormat::Png)
            .count();
        assert!(
            png_count >= 3,
            "expected at least 3 PNG density buckets, got {png_count}"
        );
    }

    #[test]
    fn social_media_presets_use_distinct_names() {
        let result = presets_for_job(JobType::SocialMediaPost);
        let mut names: Vec<&str> = result.presets.iter().map(|p| p.name.as_str()).collect();
        names.sort_unstable();
        let count = names.len();
        names.dedup();
        assert_eq!(names.len(), count, "duplicate preset names");
    }
}
