//! `kcreate_export` — PNG and SVG export pipelines.
//!
//! - [`png`] renders the renderer's scene to a PNG buffer or file.
//! - [`svg`] walks the document graph and emits a clean SVG document
//!   from vector layers.

#![forbid(unsafe_op_in_unsafe_fn)]

pub mod png;
pub mod svg;

pub use png::{export_png, export_png_to_bytes, PngExportError, PngExportOptions};
pub use svg::{export_svg_from_document, SvgDocumentExportError, SvgExportOptions};
