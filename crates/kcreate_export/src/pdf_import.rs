//! PDF import (Phase 3 foundation, Tasks 26–27).
//!
//! KCreate Phase 2 ships *export* to PDF via `printpdf`. Phase 3
//! extends that with *import*: read a PDF and produce a KCreate
//! project graph the user can keep editing.
//!
//! This module is intentionally a **lossless-on-what-it-imports**
//! first cut, not a full PDF interpreter. We do not run a content
//! stream interpreter that re-renders the page — that would require
//! a CFF / Type1 / TrueType glyph rasteriser, font caching, blend
//! modes, etc. Instead we extract the structured pieces of a PDF
//! that KCreate can map 1:1 onto its own node graph:
//!
//! 1. **Page geometry.** Each PDF page becomes one `ImportedPdfPage`
//!    carrying the MediaBox width / height in PDF points (1/72 inch).
//!    The bridge layer turns that into a [`kcreate_core::PageLayout`]
//!    so the imported page is the correct physical size.
//! 2. **Embedded images.** Every `Image` XObject referenced by every
//!    page's resources is walked and its decoded bytes extracted.
//!    JPEG-encoded images (Filter `DCTDecode`) pass through as a
//!    JPEG blob — no re-encoding. Flate-encoded uncompressed pixel
//!    buffers are converted to PNG via the `image` crate. The
//!    extracted images become `RasterLayer` nodes inside their
//!    page.
//! 3. **Document text.** lopdf's `extract_text` runs the content
//!    stream through font encodings and returns plain UTF-8 text per
//!    page. Geometry information from the stream is *not* preserved
//!    in this pass — the importer produces a single `TextLayer` per
//!    page containing the extracted text, which the user can
//!    reposition / re-style after import. This matches how Affinity
//!    Publisher / Scribus import PDFs: text comes in as a stream,
//!    the user lays it out.
//! 4. **Document metadata.** `/Info` `Title` and `Author` are read
//!    out so the importer can name the new project appropriately.
//!
//! **What is *not* yet imported (Phase 3 future):**
//! - Vector paths drawn directly in the content stream (lines,
//!   curves, fills). Lossless vector recovery would need a content
//!   stream interpreter that builds Bezier path strings from `m`,
//!   `l`, `c`, `re`, `f`, `S` operators.
//! - Form XObjects (reusable vector content blocks).
//! - JBIG2 / JPEG-2000-encoded images (`JBIG2Decode`, `JPXDecode`).
//!   Pure-Rust decoders for these are not in the workspace.
//! - Page transforms, blend modes, soft masks, clipping paths.
//!
//! Anything in the "not yet" list is reported as a
//! [`ImportedPdfPage::skipped_images`] count or a structured
//! [`PdfImportWarning`] in [`ImportedPdf::warnings`] so the renderer
//! can surface "5 images skipped (JPEG-2000)" instead of silently
//! losing pixels.

use std::path::Path;

use image::ColorType;
use lopdf::{Document, Object, ObjectId};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Top-level result of importing one PDF file. Carries one entry
/// in [`pages`](Self::pages) per page in source order plus document
/// metadata and any non-fatal warnings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedPdf {
    /// `/Info /Title` from the PDF, if present.
    pub title: Option<String>,
    /// `/Info /Author` from the PDF, if present.
    pub author: Option<String>,
    /// One entry per PDF page in source order.
    pub pages: Vec<ImportedPdfPage>,
    /// Non-fatal warnings — content the importer chose not to bring
    /// across. Surfaced in the UI so the user knows what they lost.
    pub warnings: Vec<PdfImportWarning>,
}

/// One imported PDF page. The bridge layer maps this onto a KCreate
/// `Page` node with optional `TextLayer` + `RasterLayer` children.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedPdfPage {
    /// Zero-based page index.
    pub index: usize,
    /// MediaBox width in PDF points (1/72 inch).
    pub width_pt: f64,
    /// MediaBox height in PDF points.
    pub height_pt: f64,
    /// Plain UTF-8 text extracted from the page's content stream.
    /// May be empty if the page is image-only.
    pub text: String,
    /// Image XObjects referenced by this page's resources.
    pub images: Vec<ExtractedImage>,
    /// Number of image XObjects on this page that the importer
    /// could not decode (e.g. JPEG-2000 / JBIG2 / unsupported color
    /// space). The renderer surfaces this so the user knows pixels
    /// are missing.
    pub skipped_images: usize,
}

/// A single image extracted from a PDF page.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtractedImage {
    /// Pixel width as declared in the PDF Image XObject dict.
    pub width: u32,
    /// Pixel height as declared in the PDF Image XObject dict.
    pub height: u32,
    /// The decoded image data, ready to be persisted to the blob
    /// store.
    pub data: ExtractedImageData,
}

/// Decoded image payload — either JPEG passthrough or a freshly
/// encoded PNG depending on the source PDF filter chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum ExtractedImageData {
    /// The image XObject was stored as JPEG (Filter `DCTDecode`).
    /// `bytes` is a complete JPEG file the renderer can hand
    /// straight to its raster blob store without re-encoding.
    Jpeg { bytes: Vec<u8> },
    /// The image XObject was a raw pixel buffer (Filter
    /// `FlateDecode` over uncompressed pixels) that the importer
    /// re-encoded as a standalone PNG.
    Png { bytes: Vec<u8> },
}

impl ExtractedImageData {
    /// Pre-encoded blob bytes ready to hand to the blob store.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        match self {
            Self::Jpeg { bytes } | Self::Png { bytes } => bytes,
        }
    }

    /// The MIME type matching [`Self::bytes`].
    #[must_use]
    pub fn mime_type(&self) -> &'static str {
        match self {
            Self::Jpeg { .. } => "image/jpeg",
            Self::Png { .. } => "image/png",
        }
    }
}

/// Non-fatal observations emitted while importing — e.g. images the
/// importer skipped because their filter chain isn't supported.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum PdfImportWarning {
    /// An image XObject with a filter chain we don't decode
    /// (`JPXDecode`, `JBIG2Decode`, `CCITTFaxDecode`, …) was
    /// skipped.
    UnsupportedImageFilter {
        page_index: usize,
        filter_chain: String,
    },
    /// An image XObject whose declared color space we don't support
    /// (e.g. `DeviceN`, indexed-into-Separation) was skipped.
    UnsupportedImageColorSpace {
        page_index: usize,
        color_space: String,
    },
    /// We couldn't extract a MediaBox for the page (malformed PDF).
    /// The page is imported at a fallback US Letter size.
    MissingMediaBox { page_index: usize },
}

/// Hard failures from the PDF import pipeline. Errors here mean the
/// renderer should refuse the import — see [`PdfImportWarning`] for
/// non-fatal cases.
#[derive(Debug, Error)]
pub enum PdfImportError {
    /// The PDF file couldn't be opened.
    #[error("failed to open PDF {path}: {source}")]
    Open {
        path: String,
        #[source]
        source: lopdf::Error,
    },
    /// The PDF is encrypted and we have no password.
    #[error("PDF {path} is encrypted; password-protected PDFs are not yet supported")]
    Encrypted { path: String },
    /// The PDF has zero pages — nothing to import.
    #[error("PDF {path} contains no pages")]
    NoPages { path: String },
    /// A specific page failed to read.
    #[error("failed to read PDF page {index}: {source}")]
    Page {
        index: usize,
        #[source]
        source: lopdf::Error,
    },
}

/// Default page size used when a PDF page is missing its MediaBox.
/// US Letter (612 × 792 pt) was picked over A4 because PDFs without
/// a MediaBox are almost universally legacy US documents.
const FALLBACK_MEDIABOX_PT: (f64, f64) = (612.0, 792.0);

/// Import a PDF file. Streams the PDF off disk via lopdf, walks every
/// page and every Image XObject, and returns a structured
/// [`ImportedPdf`] the bridge layer can convert to project nodes.
///
/// # Errors
/// Returns [`PdfImportError::Open`] if the file cannot be opened or
/// parsed; [`PdfImportError::Encrypted`] for password-protected
/// PDFs (Phase 3 future); [`PdfImportError::NoPages`] for empty
/// PDFs; [`PdfImportError::Page`] if a specific page cannot be
/// decoded.
pub fn import_pdf<P: AsRef<Path>>(path: P) -> Result<ImportedPdf, PdfImportError> {
    let path = path.as_ref();
    let path_str = path.display().to_string();
    let doc = Document::load(path).map_err(|source| PdfImportError::Open {
        path: path_str.clone(),
        source,
    })?;

    if doc.is_encrypted() {
        return Err(PdfImportError::Encrypted { path: path_str });
    }

    let pages_map = doc.get_pages();
    if pages_map.is_empty() {
        return Err(PdfImportError::NoPages { path: path_str });
    }

    let (title, author) = read_info(&doc);

    let mut warnings = Vec::<PdfImportWarning>::new();
    let mut pages = Vec::with_capacity(pages_map.len());

    // lopdf's BTreeMap iterates in page-number order (key = 1-based
    // page number). We re-index to zero-based for downstream UI.
    for (page_number, page_id) in &pages_map {
        let index = (*page_number as usize).saturating_sub(1);

        let (width_pt, height_pt) = match read_media_box(&doc, *page_id) {
            Some(b) => b,
            None => {
                warnings.push(PdfImportWarning::MissingMediaBox { page_index: index });
                FALLBACK_MEDIABOX_PT
            }
        };

        let text = doc
            .extract_text(&[*page_number])
            .map_err(|source| PdfImportError::Page { index, source })?;

        let (images, skipped, image_warnings) = extract_page_images(&doc, *page_id, index);
        warnings.extend(image_warnings);

        pages.push(ImportedPdfPage {
            index,
            width_pt,
            height_pt,
            text,
            images,
            skipped_images: skipped,
        });
    }

    Ok(ImportedPdf {
        title,
        author,
        pages,
        warnings,
    })
}

/// Read `/Info /Title` and `/Info /Author` from the document trailer
/// if either is set. Both are optional in PDF, so absence is normal.
fn read_info(doc: &Document) -> (Option<String>, Option<String>) {
    let info_id = match doc.trailer.get(b"Info").and_then(Object::as_reference) {
        Ok(id) => id,
        Err(_) => return (None, None),
    };
    let dict = match doc.get_dictionary(info_id) {
        Ok(d) => d,
        Err(_) => return (None, None),
    };
    let read = |key: &[u8]| {
        dict.get(key)
            .ok()
            .and_then(|o| match o {
                Object::String(bytes, _) => Some(Document::decode_text(None, bytes)),
                _ => None,
            })
            .filter(|s| !s.is_empty())
    };
    (read(b"Title"), read(b"Author"))
}

/// Read a page's MediaBox in PDF points, **following the inheritance
/// chain** per PDF 1.7 §7.7.3.4. MediaBox (along with CropBox,
/// Resources, and Rotate) is one of the four page attributes a page
/// can inherit from any ancestor Pages node — many multi-page PDFs
/// declare the MediaBox once on the root Pages object and rely on
/// every page to inherit it, so reading only the page dict misses
/// the box on the majority of real-world documents.
///
/// Returns `None` if the box is missing or malformed all the way up
/// to the root — the caller falls back to US Letter and emits a
/// warning. MediaBox is `[llx lly urx ury]`; the importer takes
/// `width = urx - llx`, `height = ury - lly` so non-zero-origin
/// MediaBoxes still produce correct page sizes.
fn read_media_box(doc: &Document, page_id: ObjectId) -> Option<(f64, f64)> {
    // Cap the parent walk so a deliberately-cyclic PDF can't cause an
    // infinite loop. Real Page trees are shallow (≤ 10 levels even
    // for huge documents), so 32 is comfortably above the worst case
    // and still bounded.
    let mut current_id = page_id;
    for _ in 0..32 {
        let Ok(dict) = doc.get_dictionary(current_id) else {
            return None;
        };
        if let Some(box_) = media_box_from_dict(dict) {
            return Some(box_);
        }
        match dict.get(b"Parent").and_then(Object::as_reference) {
            Ok(parent_id) if parent_id != current_id => current_id = parent_id,
            _ => return None,
        }
    }
    None
}

/// Pull a MediaBox tuple out of a single page-tree dict, without
/// walking the parent chain. Returns `None` if the entry is missing,
/// not a 4-element array of numbers, or has zero / negative size.
fn media_box_from_dict(dict: &lopdf::Dictionary) -> Option<(f64, f64)> {
    let arr = dict.get(b"MediaBox").ok()?.as_array().ok()?;
    if arr.len() != 4 {
        return None;
    }
    let nums: Vec<f64> = arr.iter().filter_map(as_f64).collect();
    if nums.len() != 4 {
        return None;
    }
    let width = (nums[2] - nums[0]).abs();
    let height = (nums[3] - nums[1]).abs();
    if width > 0.0 && height > 0.0 {
        Some((width, height))
    } else {
        None
    }
}

/// Walk every `XObject` entry referenced by a page's resources and
/// extract each `Subtype /Image` as an [`ExtractedImage`].
///
/// Resource dicts may be inherited up the Pages tree, so we use
/// `Document::get_page_resources` which already handles inheritance.
/// Returns `(images, skipped_count, warnings)`.
fn extract_page_images(
    doc: &Document,
    page_id: ObjectId,
    page_index: usize,
) -> (Vec<ExtractedImage>, usize, Vec<PdfImportWarning>) {
    let (resources_inline, resource_ids) = doc.get_page_resources(page_id);

    // Walk every Resources dict that applies to this page — both
    // the inline one (if present) and the inherited ones via
    // ObjectId references. The two never overlap by construction:
    // `get_page_resources` returns the inline dict separately from
    // any inherited dicts up the Pages tree.
    let mut xobject_refs = Vec::<ObjectId>::new();
    if let Some(res) = resources_inline {
        collect_xobject_refs(res, &mut xobject_refs);
    }
    for res_id in resource_ids {
        if let Ok(res) = doc.get_dictionary(res_id) {
            collect_xobject_refs(res, &mut xobject_refs);
        }
    }

    let mut images = Vec::new();
    let mut skipped = 0usize;
    let mut warnings = Vec::new();

    for xobj_id in xobject_refs {
        match decode_image_xobject(doc, xobj_id, page_index) {
            Ok(Some(img)) => images.push(img),
            Ok(None) => {} // not an image XObject (e.g. a Form)
            Err(w) => {
                skipped += 1;
                warnings.push(w);
            }
        }
    }
    (images, skipped, warnings)
}

/// Walk a `Resources` dictionary's `XObject` sub-dictionary and push
/// every reference into `out`. Direct (inline) XObjects in the
/// Resources dict are extremely rare in real PDFs — virtually all
/// real-world PDFs store XObjects as indirect objects — so we
/// intentionally only collect indirect references and treat any
/// inline XObject as a warning-less skip. lopdf will surface a
/// downstream decode error if a real document violates that.
fn collect_xobject_refs(resources: &lopdf::Dictionary, out: &mut Vec<ObjectId>) {
    let Ok(xobject) = resources.get(b"XObject") else {
        return;
    };
    let Ok(dict) = xobject.as_dict() else {
        return;
    };
    for (_, val) in dict {
        if let Ok(id) = val.as_reference() {
            out.push(id);
        }
    }
}

/// Decode one XObject by id. Returns:
///
/// * `Ok(Some(img))` if it's an Image XObject we successfully decoded.
/// * `Ok(None)` if it's a Form XObject (we ignore those today).
/// * `Err(warning)` if it's an Image XObject we couldn't decode
///   (unsupported filter / color space) — the caller records the
///   warning and increments the page's `skipped_images` counter.
fn decode_image_xobject(
    doc: &Document,
    xobj_id: ObjectId,
    page_index: usize,
) -> Result<Option<ExtractedImage>, PdfImportWarning> {
    let obj = match doc.get_object(xobj_id) {
        Ok(o) => o,
        Err(_) => return Ok(None),
    };
    let stream = match obj.as_stream() {
        Ok(s) => s,
        Err(_) => return Ok(None),
    };
    let dict = &stream.dict;
    if dict.get(b"Subtype").and_then(Object::as_name).ok() != Some(b"Image") {
        // Form XObject or something else — skip silently.
        return Ok(None);
    }
    let width = dict
        .get(b"Width")
        .and_then(Object::as_i64)
        .ok()
        .and_then(|i| u32::try_from(i).ok())
        .ok_or_else(|| PdfImportWarning::UnsupportedImageFilter {
            page_index,
            filter_chain: "missing /Width".to_string(),
        })?;
    let height = dict
        .get(b"Height")
        .and_then(Object::as_i64)
        .ok()
        .and_then(|i| u32::try_from(i).ok())
        .ok_or_else(|| PdfImportWarning::UnsupportedImageFilter {
            page_index,
            filter_chain: "missing /Height".to_string(),
        })?;

    let filter_chain = stream.filters().unwrap_or_default();
    let filter_str = filter_chain.join(",");

    // DCTDecode == JPEG. The PDF stores the complete JPEG bitstream
    // in the stream content; we copy it out verbatim and the
    // renderer's blob store treats it as a normal JPEG.
    if filter_chain.iter().any(|f| f == "DCTDecode") {
        return Ok(Some(ExtractedImage {
            width,
            height,
            data: ExtractedImageData::Jpeg {
                bytes: stream.content.clone(),
            },
        }));
    }

    // FlateDecode (or no filter at all) over an uncompressed pixel
    // buffer. We decode the flate filter to get raw pixels, then
    // re-encode as PNG so the renderer can ingest it. ColorSpace
    // determines bytes-per-pixel; we currently support DeviceRGB
    // (3 bpp), DeviceGray (1 bpp), and DeviceCMYK (we convert
    // CMYK -> RGB on the fly).
    if filter_chain.is_empty()
        || filter_chain.iter().all(|f| f == "FlateDecode")
    {
        // Decompress the Flate filter. If decompression fails (e.g.
        // the stream is corrupt), surface it as an
        // `UnsupportedImageFilter` so the user sees "Flate
        // decompression failed" rather than the misleading
        // `UnsupportedImageColorSpace` that the later byte-length
        // check would otherwise produce. We do NOT fall back to the
        // raw compressed bytes: that just feeds compressed data into
        // the pixel-buffer path where every downstream check fails
        // in a confusing way (see Devin Review finding "FlateDecode
        // fallback silently passes compressed bytes as raw pixels"
        // on PR #7).
        let raw = stream.decompressed_content().map_err(|e| {
            PdfImportWarning::UnsupportedImageFilter {
                page_index,
                filter_chain: format!("{filter_str} (decompression failed: {e})"),
            }
        })?;
        let color_space = dict
            .get(b"ColorSpace")
            .and_then(|o| match o {
                Object::Name(n) => Ok(n.clone()),
                Object::Reference(_) => Object::as_name(o).map(<[u8]>::to_vec),
                _ => Ok::<Vec<u8>, lopdf::Error>(Vec::new()),
            })
            .unwrap_or_default();
        let bpc = dict.get(b"BitsPerComponent").and_then(Object::as_i64).ok();
        return encode_raw_to_png(&raw, width, height, &color_space, bpc, page_index)
            .map(Some);
    }

    // Unsupported filter chain (JPXDecode, JBIG2Decode, CCITTFaxDecode,
    // LZW chains, etc.). Record a warning so the user sees "n images
    // skipped (JPXDecode)" rather than a silent loss.
    Err(PdfImportWarning::UnsupportedImageFilter {
        page_index,
        filter_chain: filter_str,
    })
}

/// Re-encode a raw uncompressed pixel buffer as a PNG using the
/// `image` crate. Supports DeviceGray @ 8bpc, DeviceRGB @ 8bpc, and
/// DeviceCMYK @ 8bpc (CMYK is converted to RGB on the fly using a
/// straight subtractive formula — accurate enough for preview, and
/// the user can re-color-manage on the KCreate side via the CMYK /
/// ICC pipeline from Phase 2 Block A).
fn encode_raw_to_png(
    raw: &[u8],
    width: u32,
    height: u32,
    color_space: &[u8],
    bits_per_component: Option<i64>,
    page_index: usize,
) -> Result<ExtractedImage, PdfImportWarning> {
    if bits_per_component != Some(8) && bits_per_component.is_some() {
        // 1 / 2 / 4 / 16-bpc raw images are valid PDF but would
        // require a bit unpacker; we don't have one yet so the
        // image is recorded as skipped.
        return Err(PdfImportWarning::UnsupportedImageColorSpace {
            page_index,
            color_space: format!(
                "{} @ {}bpc",
                std::str::from_utf8(color_space).unwrap_or("?"),
                bits_per_component.unwrap_or_default()
            ),
        });
    }

    let pixels = usize::try_from(width).unwrap_or(0) * usize::try_from(height).unwrap_or(0);
    let (color, normalised): (ColorType, Vec<u8>) = match color_space {
        b"DeviceGray" if raw.len() >= pixels => (ColorType::L8, raw[..pixels].to_vec()),
        b"DeviceRGB" if raw.len() >= pixels * 3 => (ColorType::Rgb8, raw[..pixels * 3].to_vec()),
        b"DeviceCMYK" if raw.len() >= pixels * 4 => (
            ColorType::Rgb8,
            cmyk_to_rgb(&raw[..pixels * 4]),
        ),
        other => {
            return Err(PdfImportWarning::UnsupportedImageColorSpace {
                page_index,
                color_space: std::str::from_utf8(other).unwrap_or("?").to_string(),
            });
        }
    };

    let mut out = Vec::<u8>::new();
    let mut writer = std::io::Cursor::new(&mut out);
    image::write_buffer_with_format(
        &mut writer,
        &normalised,
        width,
        height,
        color,
        image::ImageFormat::Png,
    )
    .map_err(|_| PdfImportWarning::UnsupportedImageColorSpace {
        page_index,
        color_space: std::str::from_utf8(color_space)
            .unwrap_or("?")
            .to_string(),
    })?;

    Ok(ExtractedImage {
        width,
        height,
        data: ExtractedImageData::Png { bytes: out },
    })
}

/// Convert an 8-bit CMYK pixel buffer to 8-bit RGB using the simple
/// subtractive formula `R = (1-C)·(1-K)·255`. This is the standard
/// "naive" CMYK->RGB used for preview-only conversions and matches
/// the formula `printpdf` itself uses when downgrading CMYK images
/// for screen viewing.
fn cmyk_to_rgb(cmyk: &[u8]) -> Vec<u8> {
    let mut rgb = Vec::with_capacity((cmyk.len() / 4) * 3);
    for chunk in cmyk.chunks_exact(4) {
        let c = f32::from(chunk[0]) / 255.0;
        let m = f32::from(chunk[1]) / 255.0;
        let y = f32::from(chunk[2]) / 255.0;
        let k = f32::from(chunk[3]) / 255.0;
        let r = ((1.0 - c) * (1.0 - k) * 255.0).clamp(0.0, 255.0) as u8;
        let g = ((1.0 - m) * (1.0 - k) * 255.0).clamp(0.0, 255.0) as u8;
        let b = ((1.0 - y) * (1.0 - k) * 255.0).clamp(0.0, 255.0) as u8;
        rgb.push(r);
        rgb.push(g);
        rgb.push(b);
    }
    rgb
}

/// Best-effort conversion of an [`Object`] to an `f64`. PDF
/// numbers are either Integer or Real (Float); both flow through
/// here.
fn as_f64(obj: &Object) -> Option<f64> {
    match obj {
        Object::Integer(i) => Some(*i as f64),
        Object::Real(r) => Some(f64::from(*r)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::{dictionary, Dictionary, Stream};

    /// Build a minimal in-memory PDF with one US Letter page and a
    /// JPEG image XObject. Returns the on-disk path of the saved
    /// PDF so [`import_pdf`] can round-trip through real lopdf.
    fn write_jpeg_pdf(width: u32, height: u32) -> tempfile::NamedTempFile {
        // A tiny valid JPEG: 4×4 solid red, encoded by the `image`
        // crate. We embed those bytes as the Image XObject content
        // with Filter = DCTDecode.
        let mut jpeg = Vec::<u8>::new();
        {
            let img =
                image::RgbImage::from_pixel(width, height, image::Rgb([255u8, 0u8, 0u8]));
            img.write_to(
                &mut std::io::Cursor::new(&mut jpeg),
                image::ImageFormat::Jpeg,
            )
            .unwrap();
        }

        let mut doc = Document::with_version("1.7");
        let pages_id = doc.new_object_id();

        let mut img_dict = Dictionary::new();
        img_dict.set("Type", Object::Name(b"XObject".to_vec()));
        img_dict.set("Subtype", Object::Name(b"Image".to_vec()));
        img_dict.set("Width", Object::Integer(i64::from(width)));
        img_dict.set("Height", Object::Integer(i64::from(height)));
        img_dict.set("ColorSpace", Object::Name(b"DeviceRGB".to_vec()));
        img_dict.set("BitsPerComponent", Object::Integer(8));
        img_dict.set("Filter", Object::Name(b"DCTDecode".to_vec()));
        let mut img_stream = Stream::new(img_dict, jpeg);
        img_stream.allows_compression = false;
        let img_id = doc.add_object(Object::Stream(img_stream));

        let resources_id = doc.add_object(dictionary! {
            "XObject" => dictionary! { "Im0" => img_id },
        });
        let content_stream = Stream::new(Dictionary::new(), b"q 612 0 0 792 0 0 cm /Im0 Do Q".to_vec());
        let content_id = doc.add_object(Object::Stream(content_stream));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            "Resources" => resources_id,
            "Contents" => content_id,
        });
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![page_id.into()],
                "Count" => 1,
            }),
        );
        let catalog_id = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        let info_id = doc.add_object(dictionary! {
            "Title" => Object::string_literal("Test PDF"),
            "Author" => Object::string_literal("KCreate Tests"),
        });
        doc.trailer.set("Root", catalog_id);
        doc.trailer.set("Info", info_id);

        let tmp = tempfile::NamedTempFile::new().unwrap();
        doc.save(tmp.path()).unwrap();
        tmp
    }

    #[test]
    fn import_extracts_pages_and_jpeg_image() {
        let tmp = write_jpeg_pdf(4, 4);
        let imported = import_pdf(tmp.path()).unwrap();
        assert_eq!(imported.title.as_deref(), Some("Test PDF"));
        assert_eq!(imported.author.as_deref(), Some("KCreate Tests"));
        assert_eq!(imported.pages.len(), 1);
        let page = &imported.pages[0];
        assert_eq!(page.index, 0);
        assert!((page.width_pt - 612.0).abs() < 0.001);
        assert!((page.height_pt - 792.0).abs() < 0.001);
        assert_eq!(page.images.len(), 1);
        assert_eq!(page.skipped_images, 0);
        let img = &page.images[0];
        assert_eq!(img.width, 4);
        assert_eq!(img.height, 4);
        match &img.data {
            ExtractedImageData::Jpeg { bytes } => {
                assert!(bytes.starts_with(&[0xff, 0xd8, 0xff]), "JPEG SOI marker");
            }
            ExtractedImageData::Png { .. } => panic!("expected JPEG passthrough"),
        }
    }

    #[test]
    fn import_rejects_nonexistent_file() {
        let err = import_pdf("/nonexistent/path/missing.pdf").unwrap_err();
        assert!(matches!(err, PdfImportError::Open { .. }));
    }

    #[test]
    fn cmyk_to_rgb_pure_cyan_is_red_complement() {
        // Pure cyan in CMYK (255, 0, 0, 0) should map to RGB
        // (0, 255, 255). The naive formula:
        //   R = (1 - 1.0) * (1 - 0) * 255 = 0
        //   G = (1 - 0.0) * (1 - 0) * 255 = 255
        //   B = (1 - 0.0) * (1 - 0) * 255 = 255
        let cmyk = vec![255u8, 0, 0, 0];
        let rgb = cmyk_to_rgb(&cmyk);
        assert_eq!(rgb, vec![0u8, 255, 255]);
    }

    #[test]
    fn cmyk_to_rgb_pure_black_is_rgb_zero() {
        // K = 255 forces all output channels to 0 regardless of CMY.
        let cmyk = vec![128u8, 64, 200, 255];
        let rgb = cmyk_to_rgb(&cmyk);
        assert_eq!(rgb, vec![0u8, 0, 0]);
    }

    #[test]
    fn extracted_image_data_mime_type_is_correct() {
        assert_eq!(
            ExtractedImageData::Jpeg { bytes: vec![] }.mime_type(),
            "image/jpeg"
        );
        assert_eq!(
            ExtractedImageData::Png { bytes: vec![] }.mime_type(),
            "image/png"
        );
    }

    /// Build a 2-page PDF where MediaBox lives only on the root
    /// Pages node, not on the individual Page dicts. Real-world PDF
    /// authoring tools commonly do this for uniformly-sized documents
    /// (a single MediaBox on Pages is enough; per-page boxes would be
    /// redundant). The importer must walk the Parent chain to find
    /// it.
    fn write_inherited_media_box_pdf() -> tempfile::NamedTempFile {
        let mut doc = Document::with_version("1.7");
        let pages_id = doc.new_object_id();
        // Page dicts deliberately omit MediaBox so they must inherit
        // it from the parent Pages node.
        let empty_stream_a = doc.add_object(Object::Stream(Stream::new(
            Dictionary::new(),
            b"".to_vec(),
        )));
        let page_a = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Resources" => dictionary! {},
            "Contents" => empty_stream_a,
        });
        let empty_stream_b = doc.add_object(Object::Stream(Stream::new(
            Dictionary::new(),
            b"".to_vec(),
        )));
        let page_b = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Resources" => dictionary! {},
            "Contents" => empty_stream_b,
        });
        // MediaBox declared ONLY on the parent Pages node — A5 (420 × 595 pt).
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![page_a.into(), page_b.into()],
                "Count" => 2,
                "MediaBox" => vec![0.into(), 0.into(), 420.into(), 595.into()],
            }),
        );
        let catalog_id = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        doc.trailer.set("Root", catalog_id);

        let tmp = tempfile::NamedTempFile::new().unwrap();
        doc.save(tmp.path()).unwrap();
        tmp
    }

    #[test]
    fn import_resolves_inherited_media_box() {
        let tmp = write_inherited_media_box_pdf();
        let imported = import_pdf(tmp.path()).unwrap();
        assert_eq!(imported.pages.len(), 2);
        // Both pages must report A5 dimensions, not the US Letter
        // fallback (612 × 792).
        for (i, page) in imported.pages.iter().enumerate() {
            assert!(
                (page.width_pt - 420.0).abs() < 0.001,
                "page {i} width: expected 420 pt (A5), got {}",
                page.width_pt,
            );
            assert!(
                (page.height_pt - 595.0).abs() < 0.001,
                "page {i} height: expected 595 pt (A5), got {}",
                page.height_pt,
            );
        }
        // No MissingMediaBox warnings — we resolved through Parent.
        assert!(
            !imported
                .warnings
                .iter()
                .any(|w| matches!(w, PdfImportWarning::MissingMediaBox { .. })),
            "should not warn about missing MediaBox; inheritance resolves it",
        );
    }
}
