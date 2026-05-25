//! Phase 5 raster filter / transform / heal operations exposed via
//! the N-API bridge.
//!
//! Every function here:
//! 1. Loads the target [`NodeType::RasterLayer`] blob from the
//!    project's content-addressed [`kcreate_storage::BlobStore`].
//! 2. Decodes the PNG payload into RGBA, runs the requested
//!    `kcreate_raster` operation, re-encodes PNG.
//! 3. Stores the new blob, updates the node's
//!    `RasterImageMeta` to point at the new hash + dimensions, and
//!    records an undoable [`Operation`] capturing the *before* node
//!    snapshot so undo restores the previous blob hash and bounds.
//!
//! This module is deliberately the **only** place in the bridge that
//! re-encodes raster bytes — it keeps the storage / scene-sync
//! invariants in one spot.
//!
//! ## Concurrency
//!
//! Every operation runs the slow path (decode → filter → encode)
//! **outside** the workspace lock to keep the renderer responsive
//! while a long blur or large rotation is in flight. The lock is
//! re-acquired only for the final node + blob mutation.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use kcreate_core::node::{Bounds, NodeType};
use kcreate_core::operation::Operation;
use kcreate_export::scene_metadata::{RasterImageMeta, RASTER_IMAGE_METADATA_KEY};
use kcreate_raster::filters;
use kcreate_raster::heal as heal_mod;
use kcreate_raster::layer::{AdjustmentLayer, CurvePoint};
use kcreate_raster::tile::TileGrid;
use kcreate_raster::transform;
use rayon::prelude::*;

/// Apply a sequence of `AdjustmentLayer` ops to a flat RGBA8 buffer
/// in place, row-parallel via rayon. Mirrors the per-tile loop on
/// `RasterLayer::render_rgba` (see `crates/kcreate_raster/src/layer.rs`),
/// but operates on the raw buffer so the bridge can re-encode PNG
/// without round-tripping through `RasterLayer`.
fn apply_adjustments_in_place(rgba: &mut [u8], adjustments: &[AdjustmentLayer]) {
    if adjustments.is_empty() {
        return;
    }
    rgba.par_chunks_mut(4).for_each(|chunk| {
        if chunk.len() < 4 {
            return;
        }
        let mut px = [chunk[0], chunk[1], chunk[2], chunk[3]];
        for adj in adjustments {
            adj.apply_pixel(&mut px);
        }
        chunk[0] = px[0];
        chunk[1] = px[1];
        chunk[2] = px[2];
        chunk[3] = px[3];
    });
}

use crate::document::{slot, DocumentBridgeError, Result};

/// Tile size used for in-memory grids created by raster ops.
/// Matches the existing renderer convention (256 px tiles).
const TILE_SIZE: u32 = 256;

/// `RGB`A bytes + canonical width/height pulled from the project's
/// blob store. Returned by `load_layer_pixels` so the slow path can
/// run outside the workspace lock.
struct LayerPixels {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
}

fn load_layer_pixels(node_id: Uuid) -> Result<LayerPixels> {
    let guard = slot().lock();
    let ws = guard.as_ref().ok_or(DocumentBridgeError::NoProject)?;
    let node = ws
        .project
        .document
        .get_node(node_id)
        .ok_or(DocumentBridgeError::NodeNotFound(node_id))?;
    if !matches!(node.node_type, NodeType::RasterLayer) {
        return Err(DocumentBridgeError::InvalidNodeType(format!(
            "{:?}",
            node.node_type
        )));
    }
    let meta_value = node
        .metadata
        .get(RASTER_IMAGE_METADATA_KEY)
        .ok_or_else(|| {
            DocumentBridgeError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "raster layer missing image metadata",
            ))
        })?;
    let meta: RasterImageMeta = serde_json::from_value(meta_value.clone())?;
    let bytes = ws
        .store
        .blobs()
        .load(&meta.blob_hash)
        .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))?;
    let img = image::load_from_memory(&bytes).map_err(|e| {
        DocumentBridgeError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    })?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    Ok(LayerPixels {
        rgba: rgba.into_raw(),
        width: w,
        height: h,
    })
}

fn encode_png(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>> {
    let mut png: Vec<u8> = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut png);
    image::write_buffer_with_format(
        &mut cursor,
        rgba,
        width,
        height,
        image::ColorType::Rgba8,
        image::ImageFormat::Png,
    )
    .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))?;
    Ok(png)
}

/// Replace a raster layer's pixel data with a new RGBA buffer of
/// `(out_w, out_h)`, recording an undoable [`Operation`] whose
/// `before_patch` is the node's pre-op JSON snapshot and whose
/// `after_patch` is the post-op snapshot. The node bounds resize to
/// the new image dimensions only when the operation actually changed
/// the canvas size (crop / rotate / non-square flip).
fn replace_layer_pixels(
    node_id: Uuid,
    new_rgba: Vec<u8>,
    out_w: u32,
    out_h: u32,
    op_kind: &'static str,
    op_payload: serde_json::Value,
    resize_bounds: bool,
) -> Result<()> {
    let png = encode_png(&new_rgba, out_w, out_h)?;
    let mut guard = slot().lock();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;

    let before_snapshot = ws
        .project
        .document
        .get_node(node_id)
        .map_or(serde_json::Value::Null, |n| {
            serde_json::to_value(n).unwrap_or(serde_json::Value::Null)
        });

    let blob = ws
        .store
        .blobs()
        .store(&png, "image/png")
        .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))?;

    let new_meta = RasterImageMeta {
        blob_hash: blob.hash,
        width: out_w,
        height: out_h,
    };

    {
        let node = ws
            .project
            .document
            .get_node_mut(node_id)
            .ok_or(DocumentBridgeError::NodeNotFound(node_id))?;
        node.metadata.insert(
            RASTER_IMAGE_METADATA_KEY.to_string(),
            serde_json::to_value(&new_meta)?,
        );
        if resize_bounds {
            node.bounds = Bounds {
                x: node.bounds.x,
                y: node.bounds.y,
                width: f64::from(out_w),
                height: f64::from(out_h),
            };
        }
    }

    let after_snapshot = ws
        .project
        .document
        .get_node(node_id)
        .map_or(serde_json::Value::Null, |n| {
            serde_json::to_value(n).unwrap_or(serde_json::Value::Null)
        });

    let op = Operation::new(
        "raster",
        op_kind,
        serde_json::json!({
            "before": before_snapshot,
            "params": op_payload,
        }),
        after_snapshot,
        vec![node_id],
    );
    ws.project.execute_operation(op);
    ws.project.modified_at = Utc::now();
    let _ = crate::document::sync_scene_locked(&mut guard);
    Ok(())
}

// -----------------------------------------------------------------------------
// Levels + Curves
// -----------------------------------------------------------------------------

/// Apply a Levels adjustment to a raster layer.
pub fn apply_levels(node_id: Uuid, black_point: f32, white_point: f32, gamma: f32) -> Result<()> {
    let mut pixels = load_layer_pixels(node_id)?;
    apply_adjustments_in_place(
        &mut pixels.rgba,
        &[AdjustmentLayer::Levels {
            black_point,
            white_point,
            gamma,
        }],
    );
    replace_layer_pixels(
        node_id,
        pixels.rgba,
        pixels.width,
        pixels.height,
        "raster_apply_levels",
        serde_json::json!({"black": black_point, "white": white_point, "gamma": gamma}),
        false,
    )
}

/// Apply a Curves adjustment defined by `(input, output)` control points.
pub fn apply_curves(node_id: Uuid, points: Vec<(f32, f32)>) -> Result<()> {
    let mut pixels = load_layer_pixels(node_id)?;
    let curve_points: Vec<CurvePoint> =
        points.iter().map(|(t, v)| CurvePoint::new(*t, *v)).collect();
    apply_adjustments_in_place(
        &mut pixels.rgba,
        &[AdjustmentLayer::Curves(curve_points)],
    );
    replace_layer_pixels(
        node_id,
        pixels.rgba,
        pixels.width,
        pixels.height,
        "raster_apply_curves",
        serde_json::json!({ "points": points }),
        false,
    )
}

// -----------------------------------------------------------------------------
// Blur + Sharpen
// -----------------------------------------------------------------------------

/// Kind of blur the caller wants. Mirrors the TypeScript wire enum.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlurKind {
    Gaussian,
    Box,
}

/// Apply Gaussian or Box blur with the given radius (in pixels).
pub fn apply_blur(node_id: Uuid, radius: f32, kind: BlurKind) -> Result<()> {
    let pixels = load_layer_pixels(node_id)?;
    let grid = TileGrid::from_image(&pixels.rgba, pixels.width, pixels.height, TILE_SIZE)
        .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))?;
    let blurred = match kind {
        BlurKind::Gaussian => filters::gaussian_blur(&grid, radius),
        // Box blur takes an integer radius; clamp negative / NaN to 0.
        BlurKind::Box => filters::box_blur(&grid, radius.max(0.0).round() as u32),
    };
    let out_rgba = blurred.to_image();
    replace_layer_pixels(
        node_id,
        out_rgba,
        pixels.width,
        pixels.height,
        "raster_apply_blur",
        serde_json::json!({"radius": radius, "kind": kind}),
        false,
    )
}

/// Apply an unsharp-mask sharpen (`radius` + `amount` + `threshold`).
pub fn apply_sharpen(node_id: Uuid, radius: f32, amount: f32, threshold: u8) -> Result<()> {
    let pixels = load_layer_pixels(node_id)?;
    let grid = TileGrid::from_image(&pixels.rgba, pixels.width, pixels.height, TILE_SIZE)
        .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))?;
    let sharp = filters::unsharp_mask(&grid, radius, amount, threshold);
    let out_rgba = sharp.to_image();
    replace_layer_pixels(
        node_id,
        out_rgba,
        pixels.width,
        pixels.height,
        "raster_apply_sharpen",
        serde_json::json!({"radius": radius, "amount": amount, "threshold": threshold}),
        false,
    )
}

// -----------------------------------------------------------------------------
// Crop / Rotate / Flip
// -----------------------------------------------------------------------------

/// Crop a raster layer to `(x, y, w, h)` in source-pixel coordinates.
pub fn crop(node_id: Uuid, x: u32, y: u32, w: u32, h: u32) -> Result<()> {
    let pixels = load_layer_pixels(node_id)?;
    let grid = TileGrid::from_image(&pixels.rgba, pixels.width, pixels.height, TILE_SIZE)
        .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))?;
    let cropped = transform::crop(&grid, x, y, w, h);
    let out_w = cropped.width;
    let out_h = cropped.height;
    let out_rgba = cropped.to_image();
    replace_layer_pixels(
        node_id,
        out_rgba,
        out_w,
        out_h,
        "raster_crop",
        serde_json::json!({"x": x, "y": y, "w": w, "h": h}),
        true,
    )
}

/// Rotate a raster layer by `angle_deg` degrees (positive = clockwise).
pub fn rotate(node_id: Uuid, angle_deg: f32) -> Result<()> {
    let pixels = load_layer_pixels(node_id)?;
    let grid = TileGrid::from_image(&pixels.rgba, pixels.width, pixels.height, TILE_SIZE)
        .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))?;
    let rotated = transform::rotate(&grid, angle_deg);
    let out_w = rotated.width;
    let out_h = rotated.height;
    let out_rgba = rotated.to_image();
    replace_layer_pixels(
        node_id,
        out_rgba,
        out_w,
        out_h,
        "raster_rotate",
        serde_json::json!({ "angle_deg": angle_deg }),
        true,
    )
}

/// Direction for [`flip`].
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlipDirection {
    Horizontal,
    Vertical,
}

/// Flip a raster layer about its centre.
pub fn flip(node_id: Uuid, direction: FlipDirection) -> Result<()> {
    let pixels = load_layer_pixels(node_id)?;
    let mut grid = TileGrid::from_image(&pixels.rgba, pixels.width, pixels.height, TILE_SIZE)
        .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))?;
    match direction {
        FlipDirection::Horizontal => transform::flip_h(&mut grid),
        FlipDirection::Vertical => transform::flip_v(&mut grid),
    }
    let out_rgba = grid.to_image();
    replace_layer_pixels(
        node_id,
        out_rgba,
        pixels.width,
        pixels.height,
        "raster_flip",
        serde_json::json!({ "direction": direction }),
        false,
    )
}

// -----------------------------------------------------------------------------
// Healing brush
// -----------------------------------------------------------------------------

/// Heal a disc from `(src_x, src_y)` over `(dst_x, dst_y)` with the
/// given `radius` (all in source pixels).
pub fn heal(
    node_id: Uuid,
    src_x: u32,
    src_y: u32,
    dst_x: u32,
    dst_y: u32,
    radius: u32,
) -> Result<()> {
    let pixels = load_layer_pixels(node_id)?;
    let mut grid = TileGrid::from_image(&pixels.rgba, pixels.width, pixels.height, TILE_SIZE)
        .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))?;
    heal_mod::heal(&mut grid, src_x, src_y, dst_x, dst_y, radius);
    let out_rgba = grid.to_image();
    replace_layer_pixels(
        node_id,
        out_rgba,
        pixels.width,
        pixels.height,
        "raster_heal",
        serde_json::json!({
            "src_x": src_x, "src_y": src_y,
            "dst_x": dst_x, "dst_y": dst_y,
            "radius": radius,
        }),
        false,
    )
}

// -----------------------------------------------------------------------------
// Preview (non-destructive)
// -----------------------------------------------------------------------------

/// Filter to apply for a non-destructive preview. Mirrors the
/// TypeScript discriminated-union exposed by `preload.ts`.
///
/// The `type` tag is used as the discriminator to avoid clashing with
/// the inner `kind` field on the `Blur` variant.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PreviewFilter {
    Levels {
        black_point: f32,
        white_point: f32,
        gamma: f32,
    },
    Curves {
        points: Vec<(f32, f32)>,
    },
    Blur {
        radius: f32,
        kind: BlurKind,
    },
    Sharpen {
        radius: f32,
        amount: f32,
        threshold: u8,
    },
}

/// Return the RGBA bytes a filter *would* produce, without mutating
/// the document. The caller uses this for live previews driven by
/// a debounced slider.
pub fn preview_filter(node_id: Uuid, filter: PreviewFilter) -> Result<(Vec<u8>, u32, u32)> {
    let mut pixels = load_layer_pixels(node_id)?;
    let (out_rgba, out_w, out_h) = match filter {
        PreviewFilter::Levels {
            black_point,
            white_point,
            gamma,
        } => {
            apply_adjustments_in_place(
                &mut pixels.rgba,
                &[AdjustmentLayer::Levels {
                    black_point,
                    white_point,
                    gamma,
                }],
            );
            (pixels.rgba, pixels.width, pixels.height)
        }
        PreviewFilter::Curves { points } => {
            let curve_points: Vec<CurvePoint> =
                points.iter().map(|(t, v)| CurvePoint::new(*t, *v)).collect();
            apply_adjustments_in_place(
                &mut pixels.rgba,
                &[AdjustmentLayer::Curves(curve_points)],
            );
            (pixels.rgba, pixels.width, pixels.height)
        }
        PreviewFilter::Blur { radius, kind } => {
            let grid =
                TileGrid::from_image(&pixels.rgba, pixels.width, pixels.height, TILE_SIZE)
                    .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))?;
            let blurred = match kind {
                BlurKind::Gaussian => filters::gaussian_blur(&grid, radius),
                BlurKind::Box => filters::box_blur(&grid, radius.max(0.0).round() as u32),
            };
            (blurred.to_image(), blurred.width, blurred.height)
        }
        PreviewFilter::Sharpen {
            radius,
            amount,
            threshold,
        } => {
            let grid =
                TileGrid::from_image(&pixels.rgba, pixels.width, pixels.height, TILE_SIZE)
                    .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))?;
            let sharp = filters::unsharp_mask(&grid, radius, amount, threshold);
            (sharp.to_image(), sharp.width, sharp.height)
        }
    };
    Ok((out_rgba, out_w, out_h))
}
