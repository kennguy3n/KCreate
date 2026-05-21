//! `kcreate_vector` — vector math, boolean ops, SVG, spatial index.
//!
//! This crate owns the path representation and everything that
//! operates purely on vector geometry: boolean ops, SVG import/export,
//! and the R-tree spatial index used for fast hit-testing on the
//! editor canvas.

#![forbid(unsafe_op_in_unsafe_fn)]

pub mod boolean;
pub mod path;
pub mod spatial_index;
pub mod svg_export;
pub mod svg_import;

pub use boolean::{boolean_operation, BooleanOp, VectorBooleanError};
pub use path::{BoundingBox, FillRule, PathError, PathPoint, PathSegment, VectorPath};
pub use spatial_index::VectorSpatialIndex;
pub use svg_export::{export_svg, export_svg_to_file, SvgExportError};
pub use svg_import::{import_svg, import_svg_file, SvgImportError};
