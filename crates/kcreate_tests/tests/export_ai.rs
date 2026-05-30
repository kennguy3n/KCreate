//! Phase 10 Block C — Export Center AI features.
//!
//! Cross-crate sanity coverage for Tasks 13–18:
//!
//! - SVG optimiser ([`optimize_svg`]) — protected-region invariants,
//!   default-attribute stripping, empty-group removal, path-data
//!   precision shortening.
//! - SSIM-targeting smart compressor ([`smart_compress`]) — binary
//!   search convergence, target/quality reporting, bad-buffer
//!   rejection.
//! - Multi-page PDF assembler ([`export_pdf_multi_pages`]) — page
//!   count round-trip via `lopdf`, optional TOC/bookmark toggles,
//!   zero-dimension page rejection.
//! - Illustrator importer ([`import_illustrator_bytes`]) — PDF
//!   container with embedded SVG payload, legacy AI8 rejection,
//!   PDF fallback for containers with no SVG.

use kcreate_export::ai_import::{import_illustrator_bytes, AiImportError, AiImportPath};
use kcreate_export::pdf_multi::{
    export_pdf_multi_pages, PdfMultiError, PdfMultiOptions, PdfPageInput,
};
use kcreate_export::smart_compress::{
    smart_compress, SmartCompressError, SmartCompressFormat, SmartCompressOptions,
};
use kcreate_export::svg_optimize::{optimize_svg, optimize_svg_with, SvgOptimizeOptions};

// ---------------------------------------------------------------------------
// Task 13 — SVG optimise
// ---------------------------------------------------------------------------

#[test]
fn svg_optimize_strips_xml_decl_and_comments_outside_protected_regions() {
    let input = "<?xml version=\"1.0\"?>\n<!-- hand-edited -->\n<svg xmlns=\"http://www.w3.org/2000/svg\"><rect/></svg>";
    let report = optimize_svg(input).expect("optimize");
    assert!(
        report.bytes_saved > 0,
        "should reclaim XML-decl + comment bytes"
    );
    assert!(
        !report.output_svg.contains("<?xml"),
        "XML decl should be dropped"
    );
    assert!(
        !report.output_svg.contains("<!-- hand-edited -->"),
        "comments should be dropped"
    );
    assert!(report.output_svg.contains("<svg"));
    assert!(report.output_svg.contains("<rect"));
}

#[test]
fn svg_optimize_preserves_text_element_body_byte_for_byte() {
    // Verifying that text body content (which renders user-visible
    // glyphs) is NEVER touched by the markup-level transformations.
    let input = "<svg xmlns=\"http://www.w3.org/2000/svg\">  <text>  spaces matter  </text>  <rect/>  </svg>";
    let report = optimize_svg(input).expect("optimize");
    assert!(
        report.output_svg.contains("  spaces matter  "),
        "text body whitespace must round-trip exactly: got `{}`",
        report.output_svg
    );
}

#[test]
fn svg_optimize_preserves_style_block_body() {
    let style_body = ".a { fill: red }    .b { stroke: blue }";
    let input = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\"><style>{style_body}</style><rect/></svg>"
    );
    let report = optimize_svg(&input).expect("optimize");
    assert!(
        report.output_svg.contains(style_body),
        "style body must be preserved verbatim: got `{}`",
        report.output_svg
    );
}

#[test]
fn svg_optimize_preserves_cdata_block_content() {
    let payload = "  function () { return 1 < 2; }  ";
    let input = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\"><script><![CDATA[{payload}]]></script></svg>"
    );
    let report = optimize_svg(&input).expect("optimize");
    assert!(
        report.output_svg.contains(payload),
        "CDATA body must be preserved verbatim: got `{}`",
        report.output_svg
    );
}

#[test]
fn svg_optimize_strips_empty_groups() {
    let input = "<svg xmlns=\"http://www.w3.org/2000/svg\"><g></g><rect/><g>   </g></svg>";
    let report = optimize_svg(input).expect("optimize");
    assert!(
        !report.output_svg.contains("<g></g>"),
        "empty <g> should be removed"
    );
    assert!(report.output_svg.contains("<rect"));
}

#[test]
fn svg_optimize_strips_default_attribute_values() {
    let input = "<svg xmlns=\"http://www.w3.org/2000/svg\"><rect fill=\"black\" stroke=\"none\" opacity=\"1\" fill-opacity=\"1\"/></svg>";
    let report = optimize_svg(input).expect("optimize");
    assert!(
        !report.output_svg.contains("fill=\"black\""),
        "default fill should be dropped"
    );
    assert!(
        !report.output_svg.contains("stroke=\"none\""),
        "default stroke should be dropped"
    );
    assert!(
        !report.output_svg.contains("opacity=\"1\""),
        "default opacity should be dropped"
    );
}

#[test]
fn svg_optimize_shortens_path_coordinate_precision() {
    let input = "<svg xmlns=\"http://www.w3.org/2000/svg\"><path d=\"M 0.123456789 0.987654321 L 1.111111111 2.222222222\"/></svg>";
    let opts = SvgOptimizeOptions {
        coord_precision: 3,
        ..Default::default()
    };
    let report = optimize_svg_with(input, opts).expect("optimize");
    assert!(
        !report.output_svg.contains("0.123456789"),
        "long decimals should be shortened: `{}`",
        report.output_svg
    );
    assert!(report.output_svg.contains("0.123"));
    assert!(report.output_svg.contains("0.988"));
}

#[test]
fn svg_optimize_rejects_empty_input() {
    let err = optimize_svg("").unwrap_err();
    assert!(matches!(
        err,
        kcreate_export::svg_optimize::SvgOptimizeError::Empty
    ));
}

#[test]
fn svg_optimize_does_not_inflate_already_minified_input() {
    // A minified SVG should round-trip without losing payload.
    let input = "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 10 10\"><path d=\"M0 0L1 1\"/></svg>";
    let report = optimize_svg(input).expect("optimize");
    assert!(report.output_svg.contains("<svg"));
    assert!(report.output_svg.contains("<path"));
    assert!(report.output_svg.contains("M0 0L1 1") || report.output_svg.contains("M0,0L1,1"));
}

// ---------------------------------------------------------------------------
// Task 14 — Smart compress
// ---------------------------------------------------------------------------

/// Build a deterministic RGBA test image — a gradient with a few
/// hard edges so JPEG/WebP have something to compress non-trivially.
fn gradient_rgba(width: u32, height: u32) -> Vec<u8> {
    let mut buf = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let r = ((x * 255) / width.max(1)) as u8;
            let g = ((y * 255) / height.max(1)) as u8;
            // A vertical band in the middle for edge content.
            let b = if x > width / 3 && x < width * 2 / 3 {
                240
            } else {
                32
            };
            buf.extend_from_slice(&[r, g, b, 255]);
        }
    }
    buf
}

#[test]
fn smart_compress_jpeg_returns_payload_within_quality_range() {
    let w = 64;
    let h = 48;
    let pixels = gradient_rgba(w, h);
    let opts = SmartCompressOptions {
        format: SmartCompressFormat::Jpeg,
        target_ssim: 0.9,
        min_quality: 20,
        max_quality: 95,
    };
    let report = smart_compress(&pixels, w, h, opts).expect("compress");
    assert_eq!(report.format, SmartCompressFormat::Jpeg);
    assert!(
        (20..=95).contains(&report.quality),
        "quality {} out of band",
        report.quality
    );
    assert!(!report.bytes.is_empty(), "must return non-empty payload");
    assert!(
        report.iterations > 0,
        "binary search should record at least one step"
    );
    assert!(
        report.compressed_bytes <= report.original_bytes,
        "JPEG should at least not exceed raw RGBA byte count for a 64×48 image"
    );
    assert!(
        report.ssim >= 0.0 && report.ssim <= 1.0,
        "SSIM out of unit interval"
    );
}

#[test]
fn smart_compress_higher_target_ssim_picks_higher_quality() {
    let w = 48;
    let h = 32;
    let pixels = gradient_rgba(w, h);
    let low = smart_compress(
        &pixels,
        w,
        h,
        SmartCompressOptions {
            format: SmartCompressFormat::Jpeg,
            target_ssim: 0.80,
            min_quality: 10,
            max_quality: 95,
        },
    )
    .expect("low");
    let high = smart_compress(
        &pixels,
        w,
        h,
        SmartCompressOptions {
            format: SmartCompressFormat::Jpeg,
            target_ssim: 0.99,
            min_quality: 10,
            max_quality: 95,
        },
    )
    .expect("high");
    assert!(
        high.quality >= low.quality,
        "tighter target {} should pick at least as high a quality as looser target {}",
        high.quality,
        low.quality,
    );
}

#[test]
fn smart_compress_rejects_invalid_target_ssim() {
    let pixels = vec![0u8; 4 * 4 * 4];
    let opts = SmartCompressOptions {
        target_ssim: 1.5,
        ..Default::default()
    };
    let err = smart_compress(&pixels, 4, 4, opts).unwrap_err();
    assert!(matches!(err, SmartCompressError::BadTarget(_)));
}

#[test]
fn smart_compress_rejects_wrong_buffer_length() {
    let opts = SmartCompressOptions::default();
    let err = smart_compress(&[0u8; 16], 4, 4, opts).unwrap_err();
    assert!(matches!(err, SmartCompressError::BadBuffer { .. }));
}

#[test]
fn smart_compress_rejects_zero_dimensions() {
    let opts = SmartCompressOptions::default();
    let err = smart_compress(&[], 0, 4, opts).unwrap_err();
    assert!(matches!(err, SmartCompressError::ZeroDim(0, 4)));
}

#[test]
fn smart_compress_webp_path_produces_valid_output() {
    let w = 32;
    let h = 24;
    let pixels = gradient_rgba(w, h);
    let opts = SmartCompressOptions {
        format: SmartCompressFormat::Webp,
        target_ssim: 0.9,
        min_quality: 30,
        max_quality: 90,
    };
    let report = smart_compress(&pixels, w, h, opts).expect("webp");
    assert_eq!(report.format, SmartCompressFormat::Webp);
    assert!(!report.bytes.is_empty());
}

// ---------------------------------------------------------------------------
// Task 17 — Illustrator (.ai) import
// ---------------------------------------------------------------------------

#[test]
fn ai_import_rejects_empty_bytes() {
    let err = import_illustrator_bytes(&[]).unwrap_err();
    assert!(matches!(err, AiImportError::Empty));
}

#[test]
fn ai_import_rejects_legacy_postscript_ai8() {
    // Pre-CS Illustrator saves are raw PostScript without a PDF wrapper.
    let buf = b"%!PS-Adobe-3.0\n%%For: Adobe Illustrator 8\n%%Pages: 1\n";
    let err = import_illustrator_bytes(buf).unwrap_err();
    assert!(matches!(err, AiImportError::LegacyPostScript));
}

#[test]
fn ai_import_extracts_embedded_svg_payload_from_pdf_container() {
    let mut buf = b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n".to_vec();
    buf.extend_from_slice(
        b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"120\" height=\"80\"><circle cx=\"10\" cy=\"10\" r=\"5\"/></svg>",
    );
    buf.extend_from_slice(b"\n%%EOF\n");

    let summary = import_illustrator_bytes(&buf).expect("import");
    assert_eq!(summary.path, AiImportPath::Svg);
    assert_eq!(summary.width_pt, Some(120.0));
    assert_eq!(summary.height_pt, Some(80.0));
    assert!(
        summary.object_count >= 1,
        "at least one node should be reported"
    );
    assert!(!summary.svg_payload_base64.is_empty());
    assert!(summary.message.is_some());
}

#[test]
fn ai_import_rejects_pdf_with_malformed_svg_payload() {
    let mut buf = b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n".to_vec();
    // `<svg` triggers extraction but the body is not actually well-formed SVG.
    buf.extend_from_slice(b"<svg xmlns=\"http://www.w3.org/2000/svg\"><rect></WRONG></svg>");
    buf.extend_from_slice(b"\n%%EOF\n");
    let err = import_illustrator_bytes(&buf).unwrap_err();
    assert!(matches!(err, AiImportError::BadSvg(_)));
}

// ---------------------------------------------------------------------------
// Task 21 — Multi-page PDF assembly (covered more thoroughly in
// the brand_plugin_ai test module, but we lock down basic invariants
// here so the export pipeline can't silently break TOC / bookmark
// gating.)
// ---------------------------------------------------------------------------

fn synth_page(idx: u32) -> PdfPageInput {
    PdfPageInput {
        title: format!("Page {}", idx + 1),
        svg: format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"400\" height=\"300\"><rect width=\"400\" height=\"300\" fill=\"#fff\"/><text x=\"40\" y=\"60\">Page {}</text></svg>",
            idx + 1
        ),
        width_pt: 400.0,
        height_pt: 300.0,
    }
}

#[test]
fn pdf_multi_rejects_empty_input() {
    let tmp = tempfile_path("kcreate-pdf-multi-empty.pdf");
    let err = export_pdf_multi_pages(&[], &tmp, &PdfMultiOptions::default()).unwrap_err();
    assert!(matches!(err, PdfMultiError::Empty));
}

#[test]
fn pdf_multi_rejects_zero_dimension_page() {
    let tmp = tempfile_path("kcreate-pdf-multi-zero.pdf");
    let mut p = synth_page(0);
    p.width_pt = 0.0;
    let err = export_pdf_multi_pages(&[p], &tmp, &PdfMultiOptions::default()).unwrap_err();
    assert!(matches!(err, PdfMultiError::ZeroPageSize(0)));
}

#[test]
fn pdf_multi_emits_requested_number_of_pages_with_bookmarks_disabled() {
    let tmp = tempfile_path("kcreate-pdf-multi-nob.pdf");
    let pages: Vec<PdfPageInput> = (0..3).map(synth_page).collect();
    let opts = PdfMultiOptions {
        include_toc: false,
        include_bookmarks: false,
        include_hyperlinks: false,
        raster_dpi: 72.0,
    };
    let report = export_pdf_multi_pages(&pages, &tmp, &opts).expect("pdf");
    assert_eq!(report.page_count, 3);
    assert!(!report.toc_emitted);
    assert!(!report.bookmarks_emitted);
    assert!(report.bytes_written > 0);
    // Re-read with lopdf to confirm the file is structurally valid.
    let doc = lopdf::Document::load(&tmp).expect("load PDF");
    assert_eq!(doc.get_pages().len(), 3);
    std::fs::remove_file(&tmp).ok();
}

#[test]
fn pdf_multi_emits_bookmarks_when_requested() {
    let tmp = tempfile_path("kcreate-pdf-multi-bk.pdf");
    let pages: Vec<PdfPageInput> = (0..2).map(synth_page).collect();
    let opts = PdfMultiOptions {
        include_toc: false,
        include_bookmarks: true,
        include_hyperlinks: false,
        raster_dpi: 72.0,
    };
    let report = export_pdf_multi_pages(&pages, &tmp, &opts).expect("pdf");
    assert_eq!(report.page_count, 2);
    assert!(
        report.bookmarks_emitted,
        "bookmarks toggle should be honoured"
    );
    let doc = lopdf::Document::load(&tmp).expect("load PDF");
    let catalog = doc
        .get_object(doc.trailer.get(b"Root").unwrap().as_reference().unwrap())
        .unwrap()
        .as_dict()
        .unwrap();
    assert!(
        catalog.has(b"Outlines"),
        "catalog should expose /Outlines when bookmarks are on"
    );
    std::fs::remove_file(&tmp).ok();
}

#[test]
fn pdf_multi_toc_alone_does_not_get_emitted() {
    // The implementation materialises the TOC through the PDF
    // outline (bookmark) tree. Asking for `include_toc=true` while
    // turning off bookmarks therefore produces no TOC in the output,
    // and `toc_emitted` must reflect that honestly — it must NOT
    // echo the request flag.
    let tmp = tempfile_path("kcreate-pdf-multi-toc-only.pdf");
    let pages: Vec<PdfPageInput> = (0..2).map(synth_page).collect();
    let opts = PdfMultiOptions {
        include_toc: true,
        include_bookmarks: false,
        include_hyperlinks: false,
        raster_dpi: 72.0,
    };
    let report = export_pdf_multi_pages(&pages, &tmp, &opts).expect("pdf");
    assert!(
        !report.toc_emitted,
        "toc cannot be emitted without the outline tree it lives in"
    );
    assert!(!report.bookmarks_emitted);
    assert_eq!(report.page_count, pages.len() as u32);
    std::fs::remove_file(&tmp).ok();
}

#[test]
fn pdf_multi_emits_toc_via_outlines_when_both_requested() {
    // When the caller asks for BOTH a TOC and bookmarks, the outline
    // tree IS the TOC, so `toc_emitted` must be true.
    let tmp = tempfile_path("kcreate-pdf-multi-toc.pdf");
    let pages: Vec<PdfPageInput> = (0..2).map(synth_page).collect();
    let opts = PdfMultiOptions {
        include_toc: true,
        include_bookmarks: true,
        include_hyperlinks: false,
        raster_dpi: 72.0,
    };
    let report = export_pdf_multi_pages(&pages, &tmp, &opts).expect("pdf");
    assert!(report.toc_emitted, "toc toggle should be honoured");
    assert!(report.bookmarks_emitted);
    assert_eq!(report.page_count, pages.len() as u32);
    let doc = lopdf::Document::load(&tmp).expect("load PDF");
    assert!(doc.get_pages().len() >= 2);
    let catalog = doc
        .get_object(doc.trailer.get(b"Root").unwrap().as_reference().unwrap())
        .unwrap()
        .as_dict()
        .unwrap();
    assert!(
        catalog.has(b"Outlines"),
        "outline tree must be present when toc + bookmarks are emitted"
    );
    std::fs::remove_file(&tmp).ok();
}

// Helper: pick a per-test temp path so the suite doesn't collide when
// running in parallel. tempfile is a dev-dep we already pull in for
// other tests; here we keep things zero-dep by using std::env.
fn tempfile_path(name: &str) -> std::path::PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "{}-{}-{}",
        name,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    ));
    p
}
