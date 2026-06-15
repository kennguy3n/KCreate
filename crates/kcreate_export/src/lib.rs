//! `kcreate_export` — PNG, SVG, PDF, WebP, and JPEG export pipelines.
//!
//! - [`png`] renders the renderer's scene to a PNG buffer or file.
//! - [`svg`] walks the document graph and emits a clean SVG document
//!   from vector layers.
//! - [`pdf`] walks the document graph and emits a one-page PDF using
//!   `printpdf` (vector paths + embedded raster `XObjects`).
//! - [`webp`] renders the renderer's scene to a WebP buffer or file
//!   via the `image` crate's bundled lossless encoder.
//! - [`jpeg`] renders the renderer's scene to a JPEG buffer or file,
//!   compositing against a chosen background since JPEG is opaque-only.
//! - [`batch`] runs a list of [`ExportItem`]s in sequence, using rayon
//!   only for in-item parallelism (e.g. PNG slab encoding).

#![forbid(unsafe_op_in_unsafe_fn)]

pub mod ai_import;
pub mod batch;
pub mod cmyk_dither;
pub mod code_gen;
pub mod exif;
pub mod figma_import;
pub mod icon_pack;
pub mod job_presets;
pub mod jpeg;
pub mod kbrand;
pub mod page_svg;
pub mod pdf;
pub mod pdf_import;
pub mod pdf_multi;
pub mod pdf_shading;
pub mod penpot_import;
pub mod png;
pub mod preflight;
pub mod psd_import;
pub mod scene_metadata;
pub mod sketch_import;
pub mod slice;
pub mod smart_compress;
pub mod svg;
pub mod svg_optimize;
pub mod svg_preview;
pub mod validate;
pub mod webp;

pub use batch::{
    run_batch, run_batch_parallel, BatchCancel, BatchExportError, BatchExportJob, BatchProgress,
    BatchResult, BatchStatus, ExportItem,
};
pub use cmyk_dither::{quantize_cmyk_image, CmykDither, CmykPixel};
pub use code_gen::{inspect_node, node_to_css, node_to_react_style, node_to_tailwind, InspectCode};
pub use figma_import::{
    import_figma, parse_figma_value, FigmaImportError, FigmaImportWarning, ImportedBounds,
    ImportedFigma, ImportedFigmaArtboard, ImportedFigmaNode, ImportedFigmaPage,
};
pub use jpeg::{export_jpeg, export_jpeg_to_bytes, JpegExportError, JpegExportOptions};
pub use page_svg::{compose_page_svg, compose_page_svg_in_frame};
pub use pdf::{
    export_pdf_from_document, export_pdf_from_document_to_bytes, PdfExportError, PdfExportOptions,
    RasterPixelCache, RasterPixels,
};
pub use pdf_import::{
    import_pdf, ExtractedImage, ExtractedImageData, ImportedPdf, ImportedPdfPage, PdfImportError,
};
pub use png::{export_png, export_png_to_bytes, PngExportError, PngExportOptions};
pub use sketch_import::{
    import_sketch, parse_sketch_zip, ImportedSketch, ImportedSketchArtboard, ImportedSketchNode,
    ImportedSketchPage, SketchImportError, SketchImportWarning,
};
pub use svg::{export_svg_from_document, SvgDocumentExportError, SvgExportOptions};
pub use webp::{export_webp, export_webp_to_bytes, WebpExportError, WebpExportOptions};

pub use icon_pack::{
    built_in_platforms, generate_icon_pack, IconFormat, IconPackError, IconPackPlatform,
    IconPackResult, IconSize, BUILT_IN_PLATFORMS,
};
pub use preflight::{
    clear_cached_font_manager, run_preflight, ColorSpaceTarget, PreflightCheck, PreflightIssue,
    PreflightOptions, PreflightSeverity,
};
pub use scene_metadata::{
    raster_image_meta, text_layer_meta, RasterImageMeta, TextLayerMeta, RASTER_IMAGE_METADATA_KEY,
    TEXT_LAYER_METADATA_KEY, VECTOR_PATH_METADATA_KEY,
};

pub use exif::{read_exif_from_bytes, ExifError, ExifMetadata, ExifValue};
pub use penpot_import::{
    import_penpot, import_penpot_bytes, ImportedPenpot, ImportedPenpotAsset, ImportedPenpotFrame,
    ImportedPenpotPage, ImportedPenpotShape, ImportedPenpotShapeKind, PenpotImportError,
    PenpotImportWarning,
};
pub use psd_import::{
    group_children as psd_group_children, import_psd, import_psd_bytes, ImportedPsd,
    ImportedPsdGroup, ImportedPsdLayer, PsdImportError,
};
pub use svg_preview::{svg_to_raster_preview, SvgPreview, SvgPreviewError, SvgPreviewOptions};
pub use validate::{
    validate_export_request, ExportSeverity, ExportValidationError, ExportValidationIssue,
    ExportValidationReport, ExportValidationRequest, DEFAULT_MAX_DIMENSION,
};
