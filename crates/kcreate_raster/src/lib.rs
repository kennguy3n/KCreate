//! `kcreate_raster` — tile engine, raster layers, masks, adjustments.
//!
//! The crate is structured around two ideas:
//!
//! 1. A [`TileGrid`] partitions an image into fixed-size square tiles
//!    so the renderer / editor can mark *dirty regions* and only
//!    re-encode the rectangles that actually changed. This makes
//!    interactive raster edits scale to large canvases without
//!    rewriting the whole framebuffer every frame.
//! 2. A [`RasterLayer`] wraps a tile grid with optional **masks**
//!    (single-channel grids whose alpha multiplies the layer alpha)
//!    and **adjustment** stacks (brightness / contrast / HSL) that
//!    can be evaluated CPU-side using `rayon` for parallelism.
//!
//! Everything is CPU-only — Phase 1 ships the editing primitives;
//! the GPU upload path is a Phase 2 concern.

#![forbid(unsafe_op_in_unsafe_fn)]

pub mod filters;
pub mod heal;
pub mod layer;
pub mod tile;
pub mod transform;

pub use filters::{box_blur, gaussian_blur, unsharp_mask};
pub use heal::heal;
pub use layer::{AdjustmentLayer, BlendMode, CurvePoint, Mask, RasterLayer, RasterLayerError};
pub use tile::{Tile, TileGrid, TileGridError, DEFAULT_TILE_SIZE};
pub use transform::{crop, flip_h, flip_v, perspective_transform, rotate};
