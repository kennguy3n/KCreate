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

pub mod batch;
pub mod cmyk_dither;
pub mod code_gen;
pub mod icon_pack;
pub mod jpeg;
pub mod kbrand;
pub mod pdf;
pub mod pdf_import;
pub mod pdf_shading;
pub mod png;
pub mod preflight;
pub mod scene_metadata;
pub mod slice;
pub mod svg;
pub mod webp;

pub use batch::{run_batch, BatchExportError, BatchExportJob, BatchStatus, ExportItem};
pub use cmyk_dither::{quantize_cmyk_image, CmykDither, CmykPixel};
pub use code_gen::{inspect_node, node_to_css, node_to_react_style, node_to_tailwind, InspectCode};
pub use jpeg::{export_jpeg, export_jpeg_to_bytes, JpegExportError, JpegExportOptions};
pub use pdf::{
    export_pdf_from_document, PdfExportError, PdfExportOptions, RasterPixelCache, RasterPixels,
};
pub use pdf_import::{
    import_pdf, ExtractedImage, ExtractedImageData, ImportedPdf, ImportedPdfPage, PdfImportError,
};
pub use png::{export_png, export_png_to_bytes, PngExportError, PngExportOptions};
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
