//! `.kbrand` — portable brand kit bundle.
//!
//! Format: a ZIP archive containing
//! - `manifest.json` — serialized [`KbrandManifest`] (canonical version + brand kit metadata)
//! - `fonts/<name>.ttf|.otf` — optional embedded font assets
//! - `logos/<name>.png|.svg|.jpg|.jpeg|.webp` — optional embedded logo assets
//!
//! All entries inside the archive use forward-slash paths; the
//! manifest's `fonts` and `logos` lists reference them by archive
//! path so importers can resolve them back to bytes.
//!
//! The exporter and importer validate magic bytes on every embedded
//! asset so malicious or corrupted archives cannot crash the
//! consumer; invalid assets cause an [`KbrandError`] rather than
//! silently being skipped.

use std::collections::HashMap;
use std::io::{Read, Seek, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use kcreate_core::project::{BrandKit, FontRef, NamedColor};

/// Bumped any time the wire format inside a `.kbrand` changes in a
/// non-backwards-compatible way. The importer rejects archives with
/// a higher major version than it knows about.
pub const KBRAND_FORMAT_VERSION_MAJOR: u32 = 1;
pub const KBRAND_FORMAT_VERSION_MINOR: u32 = 0;

const FONTS_DIR: &str = "fonts/";
const LOGOS_DIR: &str = "logos/";
const MANIFEST_FILE: &str = "manifest.json";

/// Versioned manifest stored as `manifest.json` in the archive.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KbrandManifest {
    pub format_major: u32,
    pub format_minor: u32,
    pub name: String,
    pub colors: Vec<NamedColor>,
    pub fonts: Vec<KbrandFontEntry>,
    pub spacing_scale: Vec<f32>,
    pub logos: Vec<KbrandLogoEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KbrandFontEntry {
    pub family: String,
    pub weight: u16,
    pub italic: bool,
    /// Archive-relative path of the font file, e.g.
    /// `fonts/Inter-Regular.ttf`. `None` when the font is referenced
    /// by family alone (host system fallback).
    pub archive_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KbrandLogoEntry {
    pub name: String,
    /// Archive-relative path, e.g. `logos/wordmark.svg`.
    pub archive_path: String,
}

/// Errors emitted by the `.kbrand` importer / exporter.
#[derive(Debug, Error)]
pub enum KbrandError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Zip(#[from] zip::result::ZipError),
    #[error("manifest.json missing from .kbrand archive")]
    MissingManifest,
    #[error("manifest is malformed: {0}")]
    ManifestParse(#[from] serde_json::Error),
    #[error(
        "archive format version {got}.x is newer than the highest version this build knows about ({}.{})",
        KBRAND_FORMAT_VERSION_MAJOR,
        KBRAND_FORMAT_VERSION_MINOR
    )]
    UnsupportedFormatVersion { got: u32 },
    #[error("font asset `{path}` is not a valid TTF / OTF file")]
    InvalidFontAsset { path: String },
    #[error("logo asset `{path}` is not a recognised image format (PNG / JPEG / SVG / WebP)")]
    InvalidLogoAsset { path: String },
    #[error("declared {kind} `{path}` is missing from the archive")]
    MissingAsset { kind: &'static str, path: String },
}

/// In-memory view of a `.kbrand` archive after import. Callers can
/// convert this into a [`BrandKit`] + register the embedded assets
/// into their content-addressed blob store as a separate step.
#[derive(Debug, Clone)]
pub struct KbrandBundle {
    pub manifest: KbrandManifest,
    /// Map of archive-relative path → raw bytes for every embedded
    /// font and logo asset.
    pub assets: HashMap<String, Vec<u8>>,
}

impl KbrandBundle {
    /// Convert the bundle into a [`BrandKit`]. Asset IDs on fonts
    /// and logos are left unset (`embedded_asset_id: None`) — the
    /// caller is expected to insert the bytes into their blob store
    /// and patch the kit afterwards.
    #[must_use]
    pub fn into_brand_kit(self) -> BrandKit {
        let mut kit = BrandKit::new(&self.manifest.name);
        kit.colors = self.manifest.colors;
        kit.fonts = self
            .manifest
            .fonts
            .into_iter()
            .map(|f| FontRef {
                family: f.family,
                weight: f.weight,
                italic: f.italic,
                embedded_asset_id: None,
            })
            .collect();
        kit.spacing_scale = self.manifest.spacing_scale;
        kit
    }
}

/// Serialize `kit` and the supplied font / logo asset blobs into a
/// `.kbrand` archive at `output`. `font_assets` and `logo_assets` map
/// from "natural" name (e.g. `Inter-Regular`, `wordmark`) to bytes;
/// the function picks the right archive sub-path and validates magic
/// bytes before writing.
pub fn export_brand_kit<S: std::hash::BuildHasher>(
    kit: &BrandKit,
    font_assets: &HashMap<String, Vec<u8>, S>,
    logo_assets: &HashMap<String, Vec<u8>, S>,
    output: &Path,
) -> Result<(), KbrandError> {
    let file = std::fs::File::create(output)?;
    write_brand_kit(kit, font_assets, logo_assets, file)
}

/// Serialize `kit` and the supplied font / logo asset blobs into a
/// `.kbrand` archive returned as an in-memory `Vec<u8>`. Used by
/// the KChat artifact-publishing pipeline which streams the bytes
/// straight into a multipart upload without touching the filesystem.
pub fn export_brand_kit_to_bytes<S: std::hash::BuildHasher>(
    kit: &BrandKit,
    font_assets: &HashMap<String, Vec<u8>, S>,
    logo_assets: &HashMap<String, Vec<u8>, S>,
) -> Result<Vec<u8>, KbrandError> {
    let cursor = std::io::Cursor::new(Vec::new());
    let final_cursor = write_brand_kit_inner(kit, font_assets, logo_assets, cursor)?;
    Ok(final_cursor.into_inner())
}

/// Generic brand-kit serializer. Accepts any `Write + Seek` sink so
/// the file-path and in-memory entry points can share one
/// implementation.
fn write_brand_kit<S, W>(
    kit: &BrandKit,
    font_assets: &HashMap<String, Vec<u8>, S>,
    logo_assets: &HashMap<String, Vec<u8>, S>,
    writer: W,
) -> Result<(), KbrandError>
where
    S: std::hash::BuildHasher,
    W: std::io::Write + std::io::Seek,
{
    write_brand_kit_inner(kit, font_assets, logo_assets, writer)?;
    Ok(())
}

fn write_brand_kit_inner<S, W>(
    kit: &BrandKit,
    font_assets: &HashMap<String, Vec<u8>, S>,
    logo_assets: &HashMap<String, Vec<u8>, S>,
    writer: W,
) -> Result<W, KbrandError>
where
    S: std::hash::BuildHasher,
    W: std::io::Write + std::io::Seek,
{
    let mut zip_writer = zip::ZipWriter::new(writer);
    let options: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    // Validate and emit fonts.
    let mut font_entries = Vec::with_capacity(kit.fonts.len());
    for font in &kit.fonts {
        let name_key = font_archive_basename(&font.family, font.weight, font.italic);
        let archive_path = if let Some(bytes) = font_assets.get(&name_key) {
            let ext = guess_font_extension(bytes).ok_or_else(|| KbrandError::InvalidFontAsset {
                path: name_key.clone(),
            })?;
            let path = format!("{FONTS_DIR}{name_key}.{ext}");
            zip_writer.start_file(&path, options)?;
            zip_writer.write_all(bytes)?;
            Some(path)
        } else {
            None
        };
        font_entries.push(KbrandFontEntry {
            family: font.family.clone(),
            weight: font.weight,
            italic: font.italic,
            archive_path,
        });
    }

    // Validate and emit logos.
    let mut logo_entries = Vec::with_capacity(logo_assets.len());
    let mut sorted_logos: Vec<(&String, &Vec<u8>)> = logo_assets.iter().collect();
    sorted_logos.sort_by(|a, b| a.0.cmp(b.0));
    for (name, bytes) in sorted_logos {
        let ext = guess_image_extension(bytes)
            .ok_or_else(|| KbrandError::InvalidLogoAsset { path: name.clone() })?;
        let path = format!("{LOGOS_DIR}{}.{ext}", sanitize_name(name));
        zip_writer.start_file(&path, options)?;
        zip_writer.write_all(bytes)?;
        logo_entries.push(KbrandLogoEntry {
            name: name.clone(),
            archive_path: path,
        });
    }

    // Emit manifest last (so importers can detect truncation).
    let manifest = KbrandManifest {
        format_major: KBRAND_FORMAT_VERSION_MAJOR,
        format_minor: KBRAND_FORMAT_VERSION_MINOR,
        name: kit.name.clone(),
        colors: kit.colors.clone(),
        fonts: font_entries,
        spacing_scale: kit.spacing_scale.clone(),
        logos: logo_entries,
    };
    let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
    zip_writer.start_file(MANIFEST_FILE, options)?;
    zip_writer.write_all(&manifest_bytes)?;

    let inner = zip_writer.finish()?;
    Ok(inner)
}

/// Read a `.kbrand` archive, validate every embedded asset, and
/// return an in-memory [`KbrandBundle`].
pub fn import_brand_kit(input: &Path) -> Result<KbrandBundle, KbrandError> {
    let file = std::fs::File::open(input)?;
    read_brand_kit_from_reader(file)
}

/// Same as [`import_brand_kit`] but reads from any `Read + Seek`
/// source. Used by tests that want to round-trip through an
/// in-memory buffer without touching the filesystem.
pub fn read_brand_kit_from_reader<R: Read + Seek>(reader: R) -> Result<KbrandBundle, KbrandError> {
    let mut archive = zip::ZipArchive::new(reader)?;

    // First pass: pull out the manifest.
    let manifest: KbrandManifest = {
        let mut entry = archive
            .by_name(MANIFEST_FILE)
            .map_err(|_| KbrandError::MissingManifest)?;
        let mut buf = String::new();
        entry.read_to_string(&mut buf)?;
        serde_json::from_str(&buf)?
    };
    if manifest.format_major > KBRAND_FORMAT_VERSION_MAJOR {
        return Err(KbrandError::UnsupportedFormatVersion {
            got: manifest.format_major,
        });
    }

    // Second pass: pull out every asset declared in the manifest.
    let mut assets: HashMap<String, Vec<u8>> = HashMap::new();
    for font in &manifest.fonts {
        if let Some(path) = &font.archive_path {
            let bytes = read_archive_file(&mut archive, path, "font")?;
            if guess_font_extension(&bytes).is_none() {
                return Err(KbrandError::InvalidFontAsset { path: path.clone() });
            }
            assets.insert(path.clone(), bytes);
        }
    }
    for logo in &manifest.logos {
        let bytes = read_archive_file(&mut archive, &logo.archive_path, "logo")?;
        if guess_image_extension(&bytes).is_none() {
            return Err(KbrandError::InvalidLogoAsset {
                path: logo.archive_path.clone(),
            });
        }
        assets.insert(logo.archive_path.clone(), bytes);
    }

    Ok(KbrandBundle { manifest, assets })
}

fn read_archive_file<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    path: &str,
    kind: &'static str,
) -> Result<Vec<u8>, KbrandError> {
    let mut entry = match archive.by_name(path) {
        Ok(e) => e,
        Err(_) => {
            return Err(KbrandError::MissingAsset {
                kind,
                path: path.to_string(),
            })
        }
    };
    let mut buf = Vec::with_capacity(entry.size() as usize);
    entry.read_to_end(&mut buf)?;
    Ok(buf)
}

/// Canonical archive-relative basename for a font's bytes in a
/// `.kbrand` archive (without the file extension). Exposed so the
/// bridge can populate the `font_assets` map passed to
/// [`export_brand_kit`] with the exact same keys this writer
/// expects — keeping the two sides in lockstep regardless of how
/// `sanitize_name` evolves.
#[must_use]
pub fn font_archive_basename(family: &str, weight: u16, italic: bool) -> String {
    format!(
        "{}-{}{}",
        sanitize_name(family),
        weight,
        if italic { "-italic" } else { "" }
    )
}

/// Replace characters that are awkward inside ZIP archive paths with
/// underscores. Keeps the archive deterministic and portable across
/// filesystems.
fn sanitize_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Detect whether `bytes` is a TTF or OTF font by inspecting the sfnt
/// version magic bytes. Returns the canonical file extension when
/// valid; `None` when neither TTF nor OTF.
#[must_use]
pub fn guess_font_extension(bytes: &[u8]) -> Option<&'static str> {
    if bytes.len() < 4 {
        return None;
    }
    let magic = &bytes[..4];
    // TTF: 0x00010000 (legacy TrueType) or `true` / `typ1`
    if magic == [0x00, 0x01, 0x00, 0x00] || magic == b"true" || magic == b"typ1" {
        return Some("ttf");
    }
    // OTF: `OTTO`
    if magic == b"OTTO" {
        return Some("otf");
    }
    // TrueType Collection — store as .ttc but expose as .ttf for
    // simplicity in v1; consumers that care can re-parse.
    if magic == b"ttcf" {
        return Some("ttc");
    }
    None
}

/// Detect whether `bytes` is one of the supported logo image formats.
#[must_use]
pub fn guess_image_extension(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("png");
    }
    if bytes.starts_with(b"\xff\xd8\xff") {
        return Some("jpg");
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some("webp");
    }
    // SVG: lazy sniff — must start with optional XML decl + an `<svg`
    // open tag within the first 512 bytes.
    let head_len = bytes.len().min(512);
    let head = &bytes[..head_len];
    if std::str::from_utf8(head).is_ok_and(|s| s.contains("<svg")) {
        return Some("svg");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn dummy_otf_bytes() -> Vec<u8> {
        // OTF magic + enough padding to satisfy any reader that
        // checks length. We do *not* construct a valid font; we
        // only check magic-byte sniffing.
        let mut v = b"OTTO".to_vec();
        v.extend_from_slice(&[0u8; 256]);
        v
    }

    fn dummy_png_bytes() -> Vec<u8> {
        let mut v = b"\x89PNG\r\n\x1a\n".to_vec();
        v.extend_from_slice(&[0u8; 64]);
        v
    }

    #[test]
    fn magic_byte_sniffing_recognises_known_types() {
        assert_eq!(guess_font_extension(&dummy_otf_bytes()), Some("otf"));
        assert_eq!(guess_image_extension(&dummy_png_bytes()), Some("png"));
        assert_eq!(
            guess_image_extension(b"<?xml version=\"1.0\"?><svg></svg>"),
            Some("svg")
        );
        assert_eq!(guess_font_extension(b"not a font"), None);
        assert_eq!(guess_image_extension(b"not an image"), None);
    }

    #[test]
    fn roundtrip_through_memory_preserves_tokens() {
        let mut kit = BrandKit::new("Test");
        kit.spacing_scale = vec![4.0, 8.0, 12.0];
        kit.fonts.push(FontRef {
            family: "Inter".into(),
            weight: 400,
            italic: false,
            embedded_asset_id: None,
        });

        let mut fonts = HashMap::new();
        fonts.insert("Inter-400".into(), dummy_otf_bytes());
        let mut logos = HashMap::new();
        logos.insert("wordmark".into(), dummy_png_bytes());

        let tmp = tempfile::NamedTempFile::new().expect("temp");
        export_brand_kit(&kit, &fonts, &logos, tmp.path()).expect("export");

        let bundle = import_brand_kit(tmp.path()).expect("import");
        assert_eq!(bundle.manifest.name, "Test");
        assert_eq!(bundle.manifest.spacing_scale, vec![4.0, 8.0, 12.0]);
        assert_eq!(bundle.manifest.fonts.len(), 1);
        assert_eq!(bundle.manifest.logos.len(), 1);
        // Asset bytes round-tripped.
        assert!(bundle.assets.values().any(|b| b.starts_with(b"OTTO")));
        assert!(bundle
            .assets
            .values()
            .any(|b| b.starts_with(b"\x89PNG\r\n\x1a\n")));
    }

    #[test]
    fn invalid_font_asset_is_rejected_on_export() {
        let mut kit = BrandKit::new("Test");
        kit.fonts.push(FontRef {
            family: "Bogus".into(),
            weight: 400,
            italic: false,
            embedded_asset_id: None,
        });
        let mut fonts = HashMap::new();
        fonts.insert("Bogus-400".into(), b"not a font".to_vec());
        let logos = HashMap::new();

        let tmp = tempfile::NamedTempFile::new().expect("temp");
        let err = export_brand_kit(&kit, &fonts, &logos, tmp.path()).err();
        assert!(matches!(err, Some(KbrandError::InvalidFontAsset { .. })));
    }

    #[test]
    fn future_format_version_is_rejected() {
        // Hand-craft an archive with a too-high format_major.
        let manifest = serde_json::json!({
            "format_major": KBRAND_FORMAT_VERSION_MAJOR + 1,
            "format_minor": 0,
            "name": "future",
            "colors": [],
            "fonts": [],
            "spacing_scale": [],
            "logos": [],
        });
        let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
        let mut zip_buf: Vec<u8> = Vec::new();
        {
            let cur = Cursor::new(&mut zip_buf);
            let mut writer = zip::ZipWriter::new(cur);
            writer
                .start_file::<_, ()>(MANIFEST_FILE, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(&manifest_bytes).unwrap();
            writer.finish().unwrap();
        }
        let cur = Cursor::new(zip_buf);
        let err = read_brand_kit_from_reader(cur).err();
        assert!(matches!(
            err,
            Some(KbrandError::UnsupportedFormatVersion { .. })
        ));
    }
}
