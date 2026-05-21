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
pub mod jpeg;
pub mod pdf;
pub mod png;
pub mod svg;
pub mod webp;

pub use batch::{run_batch, BatchExportError, BatchExportJob, BatchStatus, ExportItem};
pub use jpeg::{export_jpeg, export_jpeg_to_bytes, JpegExportError, JpegExportOptions};
pub use pdf::{
    export_pdf_from_document, PdfExportError, PdfExportOptions, RasterPixelCache, RasterPixels,
};
pub use png::{export_png, export_png_to_bytes, PngExportError, PngExportOptions};
pub use svg::{export_svg_from_document, SvgDocumentExportError, SvgExportOptions};
pub use webp::{export_webp, export_webp_to_bytes, WebpExportError, WebpExportOptions};
