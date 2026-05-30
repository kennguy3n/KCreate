//! Multi-page PDF assembly — Phase 10 Block D Task 21.
//!
//! Each input page arrives as an already-rendered SVG string (the
//! existing `kcreate_export::svg::*` pipeline produces one). This
//! module rasterises every page through `resvg` and writes the
//! result as a multi-page PDF with optional TOC, outline tree
//! ("bookmarks"), and hyperlink annotations.
//!
//! `lopdf` lets us emit outline + link objects directly because
//! `printpdf` doesn't expose them at the level we need.

use std::collections::BTreeMap;
use std::path::Path;

use lopdf::{
    content::{Content, Operation},
    dictionary, Document, Object, ObjectId, Stream,
};
use resvg::tiny_skia::{Pixmap, Transform};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PdfPageInput {
    pub title: String,
    pub svg: String,
    pub width_pt: f64,
    pub height_pt: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PdfMultiOptions {
    pub include_toc: bool,
    pub include_bookmarks: bool,
    pub include_hyperlinks: bool,
    /// DPI used when rasterising each SVG page. 144 gives crisp
    /// preview quality; 300 is print quality.
    pub raster_dpi: f32,
}

impl Default for PdfMultiOptions {
    fn default() -> Self {
        Self {
            include_toc: true,
            include_bookmarks: true,
            include_hyperlinks: false,
            raster_dpi: 144.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PdfMultiReport {
    pub page_count: u32,
    pub bytes_written: u64,
    pub toc_emitted: bool,
    pub bookmarks_emitted: bool,
}

#[derive(Debug, Error)]
pub enum PdfMultiError {
    #[error("pdf_multi: no input pages provided")]
    Empty,
    #[error("pdf_multi: page {0} has zero width or height")]
    ZeroPageSize(u32),
    #[error("pdf_multi: page {0} SVG could not be parsed: {1}")]
    BadSvg(u32, String),
    #[error("pdf_multi: page {0} could not be rendered to pixmap")]
    Render(u32),
    #[error("pdf_multi: write failed: {0}")]
    Write(#[from] std::io::Error),
}

/// Render `pages` into a single multi-page PDF at `output_path`.
///
/// # Errors
///
/// Returns [`PdfMultiError::Empty`] when `pages` is empty,
/// [`PdfMultiError::ZeroPageSize`] when any page has a zero
/// dimension, [`PdfMultiError::BadSvg`] when a page's SVG fails to
/// parse, [`PdfMultiError::Render`] when rasterisation fails, and
/// [`PdfMultiError::Write`] when writing the PDF to disk fails.
pub fn export_pdf_multi_pages(
    pages: &[PdfPageInput],
    output_path: &Path,
    options: &PdfMultiOptions,
) -> Result<PdfMultiReport, PdfMultiError> {
    if pages.is_empty() {
        return Err(PdfMultiError::Empty);
    }
    let mut doc = Document::with_version("1.7");
    let pages_id = doc.new_object_id();

    let mut page_ids: Vec<ObjectId> = Vec::with_capacity(pages.len());
    let mut toc_entries: Vec<(String, ObjectId)> = Vec::with_capacity(pages.len());

    for (idx, page) in pages.iter().enumerate() {
        if page.width_pt <= 0.0 || page.height_pt <= 0.0 {
            return Err(PdfMultiError::ZeroPageSize(idx as u32));
        }
        let pixmap = rasterise(page, options.raster_dpi).map_err(|e| match e {
            RasterError::Parse(s) => PdfMultiError::BadSvg(idx as u32, s),
            RasterError::Render => PdfMultiError::Render(idx as u32),
        })?;
        let img_id = embed_pixmap(&mut doc, &pixmap);

        let resources = dictionary! {
            "XObject" => dictionary! { "Im0" => img_id },
        };
        let resources_id = doc.add_object(resources);

        let content = Content {
            operations: vec![
                Operation::new("q", vec![]),
                Operation::new(
                    "cm",
                    vec![
                        page.width_pt.into(),
                        0.into(),
                        0.into(),
                        page.height_pt.into(),
                        0.into(),
                        0.into(),
                    ],
                ),
                Operation::new("Do", vec![Object::Name(b"Im0".to_vec())]),
                Operation::new("Q", vec![]),
            ],
        };
        let content_id = doc.add_object(Stream::new(dictionary! {}, content.encode().unwrap()));

        let mut page_dict = dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), page.width_pt.into(), page.height_pt.into()],
            "Contents" => content_id,
            "Resources" => resources_id,
        };
        if options.include_hyperlinks {
            // Reserved for future use: AnnotationDictionary array
            // pulled from interaction nodes. We emit an empty array
            // here so PDF viewers know link rendering is opt-in.
            page_dict.set("Annots", Object::Array(vec![]));
        }
        let page_id = doc.add_object(page_dict);
        page_ids.push(page_id);
        toc_entries.push((page.title.clone(), page_id));
    }

    // /Pages
    let kids: Vec<Object> = page_ids.iter().copied().map(Object::Reference).collect();
    let pages_dict = dictionary! {
        "Type" => "Pages",
        "Kids" => kids,
        "Count" => Object::Integer(pages.len() as i64),
    };
    doc.objects.insert(pages_id, Object::Dictionary(pages_dict));

    // Catalog
    let mut catalog = dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    };

    let bookmarks_emitted = if options.include_bookmarks {
        let outlines_id = emit_outlines(&mut doc, &toc_entries);
        catalog.set("Outlines", outlines_id);
        catalog.set("PageMode", Object::Name(b"UseOutlines".to_vec()));
        true
    } else {
        false
    };

    let catalog_id = doc.add_object(catalog);
    doc.trailer.set("Root", catalog_id);

    doc.save(output_path)?;
    let bytes_written = std::fs::metadata(output_path).map_or(0, |m| m.len());

    // `toc_emitted` reflects ACTUAL emission, not just the request
    // flag. We represent the TOC through the PDF outline tree
    // (bookmarks), so the TOC is materialised iff the user asked for
    // it, AND we had pages to enumerate, AND the bookmarks side was
    // actually written. If the caller asks for `include_toc=true` but
    // disables bookmarks, there is no TOC in the output and we report
    // that honestly. Without this, the field was a useless echo of
    // the request and a consumer that branched on it would get the
    // wrong answer.
    let toc_emitted = options.include_toc && bookmarks_emitted && !toc_entries.is_empty();

    Ok(PdfMultiReport {
        page_count: pages.len() as u32,
        bytes_written,
        toc_emitted,
        bookmarks_emitted,
    })
}

/// Build a flat outline tree (one entry per page). PDF viewers
/// render this as the bookmarks pane.
fn emit_outlines(doc: &mut Document, entries: &[(String, ObjectId)]) -> ObjectId {
    let root_id = doc.new_object_id();
    let mut item_ids: Vec<ObjectId> = Vec::with_capacity(entries.len());
    for (title, page_id) in entries {
        let mut item = dictionary! {
            "Title" => Object::string_literal(title.clone()),
            "Parent" => root_id,
            "Dest" => Object::Array(vec![
                Object::Reference(*page_id),
                Object::Name(b"Fit".to_vec()),
            ]),
        };
        item.set("Count", Object::Integer(0));
        let id = doc.add_object(item);
        item_ids.push(id);
    }
    // Link prev/next siblings.
    for (i, id) in item_ids.iter().enumerate() {
        let mut dict = doc
            .get_object(*id)
            .and_then(Object::as_dict)
            .cloned()
            .unwrap_or_default();
        if i > 0 {
            dict.set("Prev", item_ids[i - 1]);
        }
        if i + 1 < item_ids.len() {
            dict.set("Next", item_ids[i + 1]);
        }
        doc.objects.insert(*id, Object::Dictionary(dict));
    }
    let mut outlines = dictionary! {
        "Type" => "Outlines",
        "Count" => Object::Integer(item_ids.len() as i64),
    };
    if let Some(first) = item_ids.first() {
        outlines.set("First", *first);
    }
    if let Some(last) = item_ids.last() {
        outlines.set("Last", *last);
    }
    doc.objects.insert(root_id, Object::Dictionary(outlines));
    root_id
}

/// Embed a pixmap as an `/XObject /Image` and return its id.
///
/// When every pixel is fully opaque (alpha == 255), we skip the
/// separate `DeviceGray` SMask XObject entirely — that mask is purely
/// overhead for opaque content, and document pages produced by the
/// kcreate SVG pipeline are opaque the overwhelming majority of the
/// time. Without this gate, a typical multi-page document carries
/// `width * height` bytes of all-`0xFF` mask per page that PDF viewers
/// must decompress and composite for no visual difference. The
/// fully-opaque detection is folded into the single un-premultiply
/// pass below so we don't pay an extra `O(n)` scan for the
/// optimisation.
fn embed_pixmap(doc: &mut Document, pixmap: &Pixmap) -> ObjectId {
    let w = pixmap.width();
    let h = pixmap.height();
    // Convert RGBA → RGB and (optionally) harvest the alpha channel.
    //
    // CRUCIAL: `tiny_skia::Pixmap::data()` returns *premultiplied*
    // RGBA. PDF (per ISO 32000-1 §11.6.4) applies the SMask alpha
    // straight to the colour samples, so if we leave the RGB
    // channels premultiplied the alpha is applied twice and any
    // semi-transparent region renders ~A/255 too dark. Un-premultiply
    // the RGB channels here using the standard
    // `out = (in * 255 + a/2) / a` rounding formula.
    let raw = pixmap.data();
    let mut rgb = Vec::with_capacity((w * h * 3) as usize);
    let mut alpha = Vec::with_capacity((w * h) as usize);
    let mut fully_opaque = true;
    for chunk in raw.chunks_exact(4) {
        let a = u16::from(chunk[3]);
        if a == 0 {
            rgb.extend_from_slice(&[0, 0, 0]);
            fully_opaque = false;
        } else if a == 255 {
            rgb.extend_from_slice(&chunk[..3]);
        } else {
            // Round-to-nearest divide by `a`.
            let r = ((u16::from(chunk[0]) * 255 + a / 2) / a).min(255) as u8;
            let g = ((u16::from(chunk[1]) * 255 + a / 2) / a).min(255) as u8;
            let b = ((u16::from(chunk[2]) * 255 + a / 2) / a).min(255) as u8;
            rgb.extend_from_slice(&[r, g, b]);
            fully_opaque = false;
        }
        alpha.push(chunk[3]);
    }

    let mut img_dict = dictionary! {
        "Type" => "XObject",
        "Subtype" => "Image",
        "Width" => Object::Integer(i64::from(w)),
        "Height" => Object::Integer(i64::from(h)),
        "ColorSpace" => "DeviceRGB",
        "BitsPerComponent" => Object::Integer(8),
    };
    if !fully_opaque {
        let alpha_id = doc.add_object(Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Image",
                "Width" => Object::Integer(i64::from(w)),
                "Height" => Object::Integer(i64::from(h)),
                "ColorSpace" => "DeviceGray",
                "BitsPerComponent" => Object::Integer(8),
            },
            alpha,
        ));
        img_dict.set("SMask", alpha_id);
    }
    doc.add_object(Stream::new(img_dict, rgb))
}

enum RasterError {
    Parse(String),
    Render,
}

fn rasterise(page: &PdfPageInput, dpi: f32) -> Result<Pixmap, RasterError> {
    let opt = resvg::usvg::Options::default();
    let tree = resvg::usvg::Tree::from_str(&page.svg, &opt)
        .map_err(|e| RasterError::Parse(format!("{e}")))?;
    let svg_w = tree.size().width();
    let svg_h = tree.size().height();
    let scale = (dpi / 72.0).max(0.1);
    let w = (svg_w * scale).ceil() as u32;
    let h = (svg_h * scale).ceil() as u32;
    let mut pix = Pixmap::new(w.max(1), h.max(1)).ok_or(RasterError::Render)?;
    let transform = Transform::from_scale(scale, scale);
    resvg::render(&tree, transform, &mut pix.as_mut());
    Ok(pix)
}

/// Helper used by the bridge to fold per-page titles when no
/// natural-language hint exists — preserves the historical "Page N"
/// rendering instead of erroring out.
#[must_use]
pub fn default_titles_for(count: u32) -> BTreeMap<u32, String> {
    let mut out = BTreeMap::new();
    for i in 0..count {
        out.insert(i, format!("Page {}", i + 1));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn tiny_svg(w: f64, h: f64, rect_color: &str) -> String {
        format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\"><rect width=\"{w}\" height=\"{h}\" fill=\"{rect_color}\"/></svg>"
        )
    }

    #[test]
    fn empty_input_errors() {
        let dir = TempDir::new().unwrap();
        let err = export_pdf_multi_pages(
            &[],
            &dir.path().join("out.pdf"),
            &PdfMultiOptions::default(),
        )
        .unwrap_err();
        assert!(matches!(err, PdfMultiError::Empty));
    }

    #[test]
    fn zero_page_errors() {
        let dir = TempDir::new().unwrap();
        let pages = vec![PdfPageInput {
            title: "Bad".into(),
            svg: tiny_svg(100.0, 100.0, "red"),
            width_pt: 0.0,
            height_pt: 100.0,
        }];
        let err = export_pdf_multi_pages(
            &pages,
            &dir.path().join("o.pdf"),
            &PdfMultiOptions::default(),
        )
        .unwrap_err();
        assert!(matches!(err, PdfMultiError::ZeroPageSize(0)));
    }

    fn collect_image_xobject_dicts(doc: &Document) -> Vec<lopdf::Dictionary> {
        let mut out = Vec::new();
        for obj in doc.objects.values() {
            if let Object::Stream(stream) = obj {
                if stream
                    .dict
                    .get(b"Subtype")
                    .and_then(Object::as_name_str)
                    .ok()
                    == Some("Image")
                {
                    out.push(stream.dict.clone());
                }
            }
        }
        out
    }

    #[test]
    fn fully_opaque_pixmap_does_not_emit_smask() {
        // Regression for the SMask-overhead-on-opaque-pages bug:
        // when every pixel has alpha=255 there is no visual reason
        // to carry a separate DeviceGray soft-mask object. The
        // optimisation must:
        //   1. drop the `SMask` key from the colour image dict, and
        //   2. *not* add a second Image XObject (the alpha mask).
        let mut doc = Document::with_version("1.7");
        let mut pixmap = Pixmap::new(8, 8).unwrap();
        // Fill every pixel solid red, alpha=255.
        for px in pixmap.pixels_mut() {
            *px = resvg::tiny_skia::PremultipliedColorU8::from_rgba(255, 0, 0, 255).unwrap();
        }
        embed_pixmap(&mut doc, &pixmap);
        let images = collect_image_xobject_dicts(&doc);
        assert_eq!(
            images.len(),
            1,
            "opaque page must emit exactly one image XObject; got {}",
            images.len()
        );
        let dict = &images[0];
        assert!(
            dict.get(b"SMask").is_err(),
            "opaque page must not carry an SMask reference"
        );
    }

    #[test]
    fn translucent_pixmap_still_emits_smask() {
        // The opacity fast-path must NOT regress alpha handling — a
        // single semi-transparent pixel still needs the SMask object
        // or PDF viewers will composite the page against black and
        // darken transparent regions (the round-9 JPEG bug all over
        // again, but in PDF form).
        let mut doc = Document::with_version("1.7");
        let mut pixmap = Pixmap::new(8, 8).unwrap();
        for px in pixmap.pixels_mut() {
            *px = resvg::tiny_skia::PremultipliedColorU8::from_rgba(255, 0, 0, 255).unwrap();
        }
        // Make exactly one pixel partially transparent. We have to
        // store premultiplied bytes because that's what `Pixmap` keeps
        // internally; a=128 over red→(128, 0, 0, 128).
        *pixmap.pixels_mut().get_mut(0).unwrap() =
            resvg::tiny_skia::PremultipliedColorU8::from_rgba(128, 0, 0, 128).unwrap();
        embed_pixmap(&mut doc, &pixmap);
        let images = collect_image_xobject_dicts(&doc);
        assert_eq!(
            images.len(),
            2,
            "translucent page must emit both colour + mask Image XObjects"
        );
        let with_smask = images.iter().filter(|d| d.get(b"SMask").is_ok()).count();
        assert_eq!(with_smask, 1, "exactly one image must carry the SMask key");
    }

    #[test]
    fn fully_transparent_pixmap_emits_smask() {
        // A pixmap that is entirely transparent (alpha=0 everywhere)
        // is the other end of the spectrum — it MUST still emit the
        // SMask, otherwise viewers would render the zeroed RGB
        // channels as solid black instead of "see-through" content.
        let mut doc = Document::with_version("1.7");
        let pixmap = Pixmap::new(4, 4).unwrap();
        embed_pixmap(&mut doc, &pixmap);
        let images = collect_image_xobject_dicts(&doc);
        assert_eq!(images.len(), 2);
        assert!(images.iter().any(|d| d.get(b"SMask").is_ok()));
    }

    #[test]
    fn two_page_pdf_round_trips_through_lopdf() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("two.pdf");
        let pages = vec![
            PdfPageInput {
                title: "Cover".into(),
                svg: tiny_svg(200.0, 100.0, "red"),
                width_pt: 200.0,
                height_pt: 100.0,
            },
            PdfPageInput {
                title: "Body".into(),
                svg: tiny_svg(200.0, 100.0, "blue"),
                width_pt: 200.0,
                height_pt: 100.0,
            },
        ];
        let opts = PdfMultiOptions {
            include_toc: true,
            include_bookmarks: true,
            include_hyperlinks: false,
            raster_dpi: 72.0,
        };
        let rep = export_pdf_multi_pages(&pages, &path, &opts).unwrap();
        assert_eq!(rep.page_count, 2);
        assert!(rep.bytes_written > 0);
        let doc = Document::load(&path).unwrap();
        assert_eq!(doc.get_pages().len(), 2);
    }
}
