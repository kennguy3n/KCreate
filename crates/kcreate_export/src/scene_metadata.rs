//! Shared node-metadata schema for layer payloads.
//!
//! These metadata keys and DTOs are the on-disk contract between
//! `kcreate_bridge::scene_sync` (which writes them when a layer is
//! imported / shaped) and every export pipeline (SVG, PDF, raster,
//! preflight, icon pack). Defining the schema in `kcreate_export`
//! keeps every consumer that does *not* link the bridge — preflight
//! checks, icon-pack rendering, PDF flatten — pointed at a single
//! source of truth.
//!
//! `kcreate_bridge::scene_sync` re-exports identical key/value
//! constants and verifies the strings via test (see
//! `crates/kcreate_bridge/src/scene_sync.rs`).

use kcreate_core::node::Node;
use serde::{Deserialize, Serialize};

/// Metadata key on a [`kcreate_core::node::NodeType::VectorLayer`]
/// holding the serialised [`kcreate_vector::VectorPath`].
pub const VECTOR_PATH_METADATA_KEY: &str = "vector_path";

/// Metadata key on a [`kcreate_core::node::NodeType::RasterLayer`]
/// holding a [`RasterImageMeta`] payload.
pub const RASTER_IMAGE_METADATA_KEY: &str = "raster_image";

/// Metadata key on a [`kcreate_core::node::NodeType::TextLayer`]
/// holding a [`TextLayerMeta`] payload.
pub const TEXT_LAYER_METADATA_KEY: &str = "text";

/// On-disk representation of a raster layer's pixel data. The hash
/// points at a blob in the project's content-addressed
/// `kcreate_storage::BlobStore`. The dimensions are the *source*
/// pixel dimensions (independent of how big the layer is rendered).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RasterImageMeta {
    pub blob_hash: String,
    pub width: u32,
    pub height: u32,
}

/// On-disk representation of a text layer's glyph payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(clippy::derive_partial_eq_without_eq)]
pub struct TextLayerMeta {
    pub text: String,
    pub font_family: String,
    pub font_size: f32,
}

/// Read [`RasterImageMeta`] from a node, returning `None` when the
/// key is absent or the payload fails to parse.
#[must_use]
pub fn raster_image_meta(node: &Node) -> Option<RasterImageMeta> {
    node.metadata
        .get(RASTER_IMAGE_METADATA_KEY)
        .and_then(|v| serde_json::from_value::<RasterImageMeta>(v.clone()).ok())
}

/// Read [`TextLayerMeta`] from a node, returning `None` when the key
/// is absent or the payload fails to parse.
#[must_use]
pub fn text_layer_meta(node: &Node) -> Option<TextLayerMeta> {
    node.metadata
        .get(TEXT_LAYER_METADATA_KEY)
        .and_then(|v| serde_json::from_value::<TextLayerMeta>(v.clone()).ok())
}
