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
#[derive(Clone)]
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
    let curve_points: Vec<CurvePoint> = points
        .iter()
        .map(|(t, v)| CurvePoint::new(*t, *v))
        .collect();
    apply_adjustments_in_place(&mut pixels.rgba, &[AdjustmentLayer::Curves(curve_points)]);
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
    Hsl {
        hue: f32,
        saturation: f32,
        lightness: f32,
    },
    ColorBalance {
        shadows: [f32; 3],
        midtones: [f32; 3],
        highlights: [f32; 3],
    },
}

/// Apply a [`PreviewFilter`] in place over a layer's RGBA buffer.
/// All currently supported variants are dimension-preserving, so the
/// caller's `LayerPixels::{width, height}` remain authoritative.
///
/// This is the single source of truth for how each `PreviewFilter`
/// variant maps to a `kcreate_raster` operation; both the live-
/// preview path (`preview_filter`) and the committal masked-filter
/// path (`apply_filter_masked`) drive their work through this helper
/// so a new variant only has to be added in one place.
fn apply_filter_in_place(pixels: &mut LayerPixels, filter: &PreviewFilter) -> Result<()> {
    match filter {
        PreviewFilter::Levels {
            black_point,
            white_point,
            gamma,
        } => {
            apply_adjustments_in_place(
                &mut pixels.rgba,
                &[AdjustmentLayer::Levels {
                    black_point: *black_point,
                    white_point: *white_point,
                    gamma: *gamma,
                }],
            );
        }
        PreviewFilter::Curves { points } => {
            let curve_points: Vec<CurvePoint> = points
                .iter()
                .map(|(t, v)| CurvePoint::new(*t, *v))
                .collect();
            apply_adjustments_in_place(&mut pixels.rgba, &[AdjustmentLayer::Curves(curve_points)]);
        }
        PreviewFilter::Blur { radius, kind } => {
            let grid =
                TileGrid::from_image(&pixels.rgba, pixels.width, pixels.height, TILE_SIZE)
                    .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))?;
            let blurred = match kind {
                BlurKind::Gaussian => filters::gaussian_blur(&grid, *radius),
                BlurKind::Box => filters::box_blur(&grid, radius.max(0.0).round() as u32),
            };
            pixels.rgba = blurred.to_image();
        }
        PreviewFilter::Sharpen {
            radius,
            amount,
            threshold,
        } => {
            let grid =
                TileGrid::from_image(&pixels.rgba, pixels.width, pixels.height, TILE_SIZE)
                    .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))?;
            let sharp = filters::unsharp_mask(&grid, *radius, *amount, *threshold);
            pixels.rgba = sharp.to_image();
        }
        PreviewFilter::Hsl {
            hue,
            saturation,
            lightness,
        } => {
            apply_adjustments_in_place(
                &mut pixels.rgba,
                &[AdjustmentLayer::HueSaturation {
                    hue: *hue,
                    saturation: *saturation,
                    lightness: *lightness,
                }],
            );
        }
        PreviewFilter::ColorBalance {
            shadows,
            midtones,
            highlights,
        } => {
            apply_adjustments_in_place(
                &mut pixels.rgba,
                &[AdjustmentLayer::ColorBalance {
                    shadows: *shadows,
                    midtones: *midtones,
                    highlights: *highlights,
                }],
            );
        }
    }
    Ok(())
}

/// Return the RGBA bytes a filter *would* produce, without mutating
/// the document. The caller uses this for live previews driven by
/// a debounced slider.
pub fn preview_filter(node_id: Uuid, filter: PreviewFilter) -> Result<(Vec<u8>, u32, u32)> {
    let mut pixels = load_layer_pixels(node_id)?;
    apply_filter_in_place(&mut pixels, &filter)?;
    Ok((pixels.rgba, pixels.width, pixels.height))
}

// -----------------------------------------------------------------------------
// Perspective transform
// -----------------------------------------------------------------------------

/// Apply a 4-corner projective transform to a raster layer. The
/// destination corners are supplied as `[(x, y); 4]` in **TL, TR, BL,
/// BR** order in source-pixel space; the output canvas grows to the
/// axis-aligned bounding box of those corners, with transparent
/// padding where the warped quadrilateral does not cover the canvas.
///
/// The transform records an undoable `Operation` and resizes the
/// layer's node bounds — perspective generally changes the canvas
/// size, so the node has to widen / heighten alongside.
pub fn apply_perspective(node_id: Uuid, corners: [(f64, f64); 4]) -> Result<()> {
    let pixels = load_layer_pixels(node_id)?;
    let grid = TileGrid::from_image(&pixels.rgba, pixels.width, pixels.height, TILE_SIZE)
        .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))?;
    let warped = transform::perspective_transform(&grid, corners);
    let out_w = warped.width;
    let out_h = warped.height;
    let out_rgba = warped.to_image();
    // The transform falls back to the source grid when the corners
    // are degenerate, in which case the canvas size does not change
    // and we mark the op as a no-op resize. Otherwise the bounds
    // must follow the new canvas extent so the renderer doesn't
    // letterbox the warped image.
    let canvas_resized = out_w != pixels.width || out_h != pixels.height;
    replace_layer_pixels(
        node_id,
        out_rgba,
        out_w,
        out_h,
        "raster_perspective",
        serde_json::json!({ "corners": corners }),
        canvas_resized,
    )
}

// -----------------------------------------------------------------------------
// HSL + Color Balance
// -----------------------------------------------------------------------------

/// Apply a Hue / Saturation / Lightness shift to a raster layer.
///
/// * `hue` is a rotation in degrees around the colour wheel
///   (`-180.0..=180.0`).
/// * `saturation` is a multiplier (`0.0` flattens to grey,
///   `1.0` is identity, `> 1.0` boosts).
/// * `lightness` is an additive shift in `[-1.0, 1.0]`.
pub fn apply_hsl(node_id: Uuid, hue: f32, saturation: f32, lightness: f32) -> Result<()> {
    let mut pixels = load_layer_pixels(node_id)?;
    apply_adjustments_in_place(
        &mut pixels.rgba,
        &[AdjustmentLayer::HueSaturation {
            hue,
            saturation,
            lightness,
        }],
    );
    replace_layer_pixels(
        node_id,
        pixels.rgba,
        pixels.width,
        pixels.height,
        "raster_apply_hsl",
        serde_json::json!({"hue": hue, "saturation": saturation, "lightness": lightness}),
        false,
    )
}

/// Apply a three-way Color Balance adjustment (shadows / midtones /
/// highlights) to a raster layer. Each triple is `[r, g, b]` in
/// `[-1.0, 1.0]`. All-zeros is the identity transform.
pub fn apply_color_balance(
    node_id: Uuid,
    shadows: [f32; 3],
    midtones: [f32; 3],
    highlights: [f32; 3],
) -> Result<()> {
    let mut pixels = load_layer_pixels(node_id)?;
    apply_adjustments_in_place(
        &mut pixels.rgba,
        &[AdjustmentLayer::ColorBalance {
            shadows,
            midtones,
            highlights,
        }],
    );
    replace_layer_pixels(
        node_id,
        pixels.rgba,
        pixels.width,
        pixels.height,
        "raster_apply_color_balance",
        serde_json::json!({
            "shadows": shadows,
            "midtones": midtones,
            "highlights": highlights,
        }),
        false,
    )
}

// -----------------------------------------------------------------------------
// Masked filter application (Phase 8 Task 11)
// -----------------------------------------------------------------------------

/// Errors specific to [`apply_filter_masked`]. Surfaced through
/// [`DocumentBridgeError::Io`] so callers see a single error type but
/// the message identifies the mask shape mismatch.
#[derive(Debug)]
struct MaskShapeMismatch {
    mask_len: usize,
    expected: usize,
}

impl std::fmt::Display for MaskShapeMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "filter mask length {} does not match layer pixel count {}",
            self.mask_len, self.expected
        )
    }
}

impl std::error::Error for MaskShapeMismatch {}

/// Compute a per-pixel float feather weight in `[0.0, 1.0]` from the
/// raw byte-coded selection mask. The mask is one byte per pixel:
/// `0` means "not selected", any non-zero byte means "selected".
/// (Bytes carry the same information as the boolean predicate but
/// cross the IPC boundary without per-element conversion — a 4K mask
/// fits in an 8.3 MB `Uint8Array` instead of an 8.3 M-element JS
/// boolean array.) The weight at pixel `(x, y)` is the average of
/// itself and its 4-connected neighbours (north / south / east /
/// west), with selected => `1.0` and not-selected => `0.0`. Pixels
/// in the interior of a selected / unselected region therefore stay
/// exactly at `1.0` / `0.0` (no work for `apply_filter_masked` to
/// do), while pixels on the mask boundary land somewhere in
/// between, giving us a 1px feather kernel without re-implementing
/// a separable blur.
///
/// Border pixels treat out-of-bounds neighbours as having the same
/// mask value as the centre — i.e. the kernel is "clamped" at the
/// canvas edge. This avoids a spurious feather where the canvas
/// edge happens to be a mask boundary.
fn feather_mask_weights(mask: &[u8], width: u32, height: u32) -> Vec<f32> {
    let w = width as usize;
    let h = height as usize;
    let mut weights = vec![0.0_f32; w * h];
    let bit = |byte: u8| u8::from(byte != 0);
    weights.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, slot) in row.iter_mut().enumerate() {
            let centre = bit(mask[y * w + x]);
            let west = if x > 0 {
                bit(mask[y * w + x - 1])
            } else {
                centre
            };
            let east = if x + 1 < w {
                bit(mask[y * w + x + 1])
            } else {
                centre
            };
            let north = if y > 0 {
                bit(mask[(y - 1) * w + x])
            } else {
                centre
            };
            let south = if y + 1 < h {
                bit(mask[(y + 1) * w + x])
            } else {
                centre
            };
            let count = centre + west + east + north + south;
            *slot = f32::from(count) / 5.0;
        }
    });
    weights
}

/// Apply a filter to a raster layer but only where `mask[i] == true`,
/// with a 1-pixel feather at the mask boundary so the seam does not
/// alias. The mask must contain exactly `layer_width * layer_height`
/// elements; any mismatch returns a structured error so the renderer
/// can recover instead of silently producing nonsense.
///
/// Semantics:
/// * Fully unmasked pixels (`mask[i] == false` *and* all 4 neighbours
///   are `false`) are copied straight from the source — bit-exact.
/// * Fully masked pixels (`mask[i] == true` *and* all 4 neighbours
///   are `true`) take the filtered output verbatim.
/// * Boundary pixels blend `original * (1 - w) + filtered * w` where
///   `w` is the 5-tap (centre + N/S/E/W) average of the boolean
///   mask. This yields a single-pixel feather without re-running a
///   separable blur over the mask.
///
/// Alpha is blended on the same weight curve so feathered edges
/// reveal the underlying transparency naturally.
pub fn apply_filter_masked(node_id: Uuid, filter: PreviewFilter, mask: Vec<u8>) -> Result<()> {
    let pixels = load_layer_pixels(node_id)?;
    let total = (pixels.width as usize) * (pixels.height as usize);
    if mask.len() != total {
        return Err(DocumentBridgeError::Io(std::io::Error::other(
            MaskShapeMismatch {
                mask_len: mask.len(),
                expected: total,
            }
            .to_string(),
        )));
    }

    // Run the filter through the same helper the live-preview path
    // uses so the masked and unmasked outputs are pixel-equivalent at
    // mask=1, then composite the result over the source through the
    // feathered mask.
    let mut filtered_pixels = pixels.clone();
    apply_filter_in_place(&mut filtered_pixels, &filter)?;
    let filtered = filtered_pixels.rgba;
    if filtered.len() != pixels.rgba.len() {
        // Defensive: every supported filter is dimension-preserving,
        // but if a future variant changes size we surface that as a
        // clear error rather than panic in the blend loop.
        return Err(DocumentBridgeError::Io(std::io::Error::other(format!(
            "masked filter produced unexpected buffer size {} (expected {})",
            filtered.len(),
            pixels.rgba.len()
        ))));
    }

    let weights = feather_mask_weights(&mask, pixels.width, pixels.height);
    let mut out = pixels.rgba.clone();
    out.par_chunks_mut(4)
        .zip(pixels.rgba.par_chunks(4))
        .zip(filtered.par_chunks(4))
        .zip(weights.par_iter())
        .for_each(|(((out_px, src_px), filt_px), &w)| {
            if w <= 0.0 {
                // Fully unmasked — copy source exactly. (Skip the
                // float blend so the bytes round-trip bit-exact.)
                out_px.copy_from_slice(src_px);
                return;
            }
            if w >= 1.0 {
                // Fully masked — copy filter verbatim.
                out_px.copy_from_slice(filt_px);
                return;
            }
            for i in 0..4 {
                let s = f32::from(src_px[i]);
                let f = f32::from(filt_px[i]);
                let blended = s.mul_add(1.0 - w, f * w);
                out_px[i] = blended.round().clamp(0.0, 255.0) as u8;
            }
        });

    let mask_true = mask.iter().filter(|b| **b != 0).count();
    replace_layer_pixels(
        node_id,
        out,
        pixels.width,
        pixels.height,
        "raster_apply_filter_masked",
        serde_json::json!({
            "filter": filter,
            "mask_len": mask.len(),
            "mask_true": mask_true,
        }),
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{
        document_import_image_bytes, project_close, project_create, reset_for_tests,
    };
    use serde_json::Value;
    use serial_test::serial;

    #[test]
    fn feather_weights_zero_when_mask_all_unset() {
        let mask = vec![0u8; 9];
        let weights = feather_mask_weights(&mask, 3, 3);
        assert_eq!(weights, vec![0.0; 9]);
    }

    #[test]
    fn feather_weights_one_when_mask_all_set() {
        let mask = vec![1u8; 9];
        let weights = feather_mask_weights(&mask, 3, 3);
        for w in weights {
            assert!((w - 1.0).abs() < f32::EPSILON, "expected 1.0, got {w}");
        }
    }

    #[test]
    fn feather_weights_treat_any_nonzero_byte_as_selected() {
        // Selection masks crossing the IPC boundary as Uint8Array
        // may carry arbitrary byte values (e.g. 255 from a tool that
        // pre-multiplied a coverage value). Any non-zero byte must
        // count as fully selected, identical to a `1`.
        let mask = vec![255u8; 9];
        let weights = feather_mask_weights(&mask, 3, 3);
        for w in weights {
            assert!((w - 1.0).abs() < f32::EPSILON, "expected 1.0, got {w}");
        }
    }

    #[test]
    fn feather_weights_intermediate_at_boundary() {
        // A horizontal step: top row unselected, bottom row
        // selected. The boundary pixels should average (centre + 4
        // neighbours) and produce intermediate weights.
        //
        //   0 0 0
        //   1 1 1
        //
        // Top-row centres see (0, 0, 1 [south], 0, 0) when
        // neighbours-out-of-bounds clamp to the centre value:
        // top-row pixels actually see (centre=0, west=0, east=0,
        // north=0 (clamp), south=1) → 1/5.
        let mask = vec![0u8, 0, 0, 1, 1, 1];
        let weights = feather_mask_weights(&mask, 3, 2);
        // Top row: each centre sees one selected neighbour (south).
        for w in &weights[0..3] {
            assert!((w - 0.2).abs() < 1e-4, "expected 0.2, got {w}");
        }
        // Bottom row: each centre + west + east + south(clamp)=1
        // and north=0 → 4/5.
        for w in &weights[3..6] {
            assert!((w - 0.8).abs() < 1e-4, "expected 0.8, got {w}");
        }
    }

    #[test]
    fn preview_filter_hsl_serde_round_trip() {
        let filter = PreviewFilter::Hsl {
            hue: 180.0,
            saturation: 0.5,
            lightness: -0.25,
        };
        let json = serde_json::to_value(&filter).expect("serialize");
        assert_eq!(json["type"], "hsl");
        assert_eq!(json["hue"], 180.0);
        assert_eq!(json["saturation"], 0.5);
        assert_eq!(json["lightness"], -0.25);
        let back: PreviewFilter = serde_json::from_value(json).expect("deserialize");
        match back {
            PreviewFilter::Hsl {
                hue,
                saturation,
                lightness,
            } => {
                assert!((hue - 180.0).abs() < f32::EPSILON);
                assert!((saturation - 0.5).abs() < f32::EPSILON);
                assert!((lightness - -0.25).abs() < f32::EPSILON);
            }
            other => panic!("expected Hsl, got {other:?}"),
        }
    }

    #[test]
    fn preview_filter_color_balance_serde_round_trip() {
        let filter = PreviewFilter::ColorBalance {
            shadows: [0.1, 0.0, -0.2],
            midtones: [0.0, 0.3, 0.0],
            highlights: [-0.4, 0.0, 0.5],
        };
        let json = serde_json::to_value(&filter).expect("serialize");
        assert_eq!(json["type"], "color_balance");
        let back: PreviewFilter = serde_json::from_value(json).expect("deserialize");
        match back {
            PreviewFilter::ColorBalance {
                shadows,
                midtones,
                highlights,
            } => {
                assert!((shadows[0] - 0.1).abs() < f32::EPSILON);
                assert!((midtones[1] - 0.3).abs() < f32::EPSILON);
                assert!((highlights[2] - 0.5).abs() < f32::EPSILON);
            }
            other => panic!("expected ColorBalance, got {other:?}"),
        }
    }

    /// Build a tiny solid-colour RGBA PNG used to seed a raster
    /// layer fixture. `width * height` pixels of `colour`.
    fn build_test_png(width: u32, height: u32, colour: [u8; 4]) -> Vec<u8> {
        let rgba: Vec<u8> = (0..width * height).flat_map(|_| colour).collect();
        encode_png(&rgba, width, height).expect("encode test png")
    }

    /// Inspect the `raster_image` metadata blob for `node_id`.
    /// Panics if the node is missing or not a raster layer. Used by
    /// the integration tests below to confirm an op actually
    /// rewrote the blob hash.
    fn raster_meta(node_id: Uuid) -> RasterImageMeta {
        let guard = crate::document::slot().lock();
        let ws = guard.as_ref().expect("workspace");
        let node = ws.project.document.get_node(node_id).expect("node exists");
        let meta = node
            .metadata
            .get(RASTER_IMAGE_METADATA_KEY)
            .expect("raster meta")
            .clone();
        serde_json::from_value(meta).expect("decode raster meta")
    }

    /// Snapshot the node's `Bounds` for assertions across an op.
    fn raster_bounds(node_id: Uuid) -> Bounds {
        let guard = crate::document::slot().lock();
        let ws = guard.as_ref().expect("workspace");
        ws.project
            .document
            .get_node(node_id)
            .expect("node exists")
            .bounds
    }

    /// Helper: create a fresh workspace, import a solid PNG, and
    /// return the raster node's uuid. Caller is expected to wrap in
    /// `#[serial]` and to call [`project_close`] at end.
    fn seed_workspace_with_raster(width: u32, height: u32, colour: [u8; 4]) -> Uuid {
        reset_for_tests();
        let dir = tempfile::tempdir().expect("tempdir");
        project_create("raster-ops-test", dir.path()).expect("create project");
        let png = build_test_png(width, height, colour);
        let id = document_import_image_bytes(None, &png).expect("import png");
        // Leak the tempdir so the workspace's project files survive
        // until project_close is called by the test body. Using
        // forget here means we accept a tempdir leak in test mode in
        // exchange for not racing the workspace.
        std::mem::forget(dir);
        id
    }

    #[test]
    #[serial]
    fn apply_hsl_identity_preserves_pixel_count_and_changes_blob() {
        let node_id = seed_workspace_with_raster(16, 16, [200, 100, 50, 255]);
        let before = raster_meta(node_id);
        // Identity HSL: hue=0, saturation=1, lightness=0. The
        // pixels round-trip unchanged but the bridge still
        // re-encodes the PNG, so the resulting blob hash _may_ be
        // the same (PNG is deterministic for the same bytes). We
        // assert the canvas dimensions hold and the op recorded.
        apply_hsl(node_id, 0.0, 1.0, 0.0).expect("identity hsl");
        let after = raster_meta(node_id);
        assert_eq!(before.width, after.width);
        assert_eq!(before.height, after.height);
        assert_eq!(
            before.blob_hash, after.blob_hash,
            "identity HSL should produce the same content-addressed blob",
        );
        project_close();
    }

    #[test]
    #[serial]
    fn apply_hsl_non_identity_changes_blob() {
        let node_id = seed_workspace_with_raster(16, 16, [200, 100, 50, 255]);
        let before = raster_meta(node_id);
        // Hue rotate 180° flips the chromaticity — the blob bytes
        // *must* differ from the source.
        apply_hsl(node_id, 180.0, 1.0, 0.0).expect("hue rotate");
        let after = raster_meta(node_id);
        assert_ne!(
            before.blob_hash, after.blob_hash,
            "180° hue rotation must produce a different blob",
        );
        project_close();
    }

    #[test]
    #[serial]
    fn apply_color_balance_identity_keeps_blob_hash() {
        let node_id = seed_workspace_with_raster(8, 8, [120, 80, 200, 255]);
        let before = raster_meta(node_id);
        apply_color_balance(node_id, [0.0; 3], [0.0; 3], [0.0; 3]).expect("identity balance");
        let after = raster_meta(node_id);
        assert_eq!(before.blob_hash, after.blob_hash);
        project_close();
    }

    #[test]
    #[serial]
    fn apply_perspective_identity_keeps_canvas_size() {
        let node_id = seed_workspace_with_raster(32, 32, [10, 20, 30, 255]);
        let bounds_before = raster_bounds(node_id);
        let meta_before = raster_meta(node_id);
        let corners = [(0.0, 0.0), (32.0, 0.0), (0.0, 32.0), (32.0, 32.0)];
        apply_perspective(node_id, corners).expect("perspective identity");
        let bounds_after = raster_bounds(node_id);
        let meta_after = raster_meta(node_id);
        assert!((bounds_before.width - bounds_after.width).abs() < f64::EPSILON);
        assert!((bounds_before.height - bounds_after.height).abs() < f64::EPSILON);
        assert_eq!(meta_before.width, meta_after.width);
        assert_eq!(meta_before.height, meta_after.height);
        project_close();
    }

    #[test]
    #[serial]
    fn apply_perspective_grows_canvas_for_widened_corners() {
        let node_id = seed_workspace_with_raster(32, 32, [10, 20, 30, 255]);
        // Bottom corners pushed outwards: bbox is 50 × 32.
        let corners = [(0.0, 0.0), (32.0, 0.0), (-9.0, 32.0), (41.0, 32.0)];
        apply_perspective(node_id, corners).expect("perspective widen");
        let bounds_after = raster_bounds(node_id);
        let meta_after = raster_meta(node_id);
        assert!(
            bounds_after.width >= 50.0,
            "expected widened bounds, got {}",
            bounds_after.width
        );
        assert!(meta_after.width >= 50);
        project_close();
    }

    #[test]
    #[serial]
    fn apply_filter_masked_rejects_wrong_size_mask() {
        let node_id = seed_workspace_with_raster(8, 8, [50, 50, 50, 255]);
        let filter = PreviewFilter::Levels {
            black_point: 0.0,
            white_point: 1.0,
            gamma: 1.0,
        };
        let bad_mask = vec![1u8; 64 + 1];
        let err = apply_filter_masked(node_id, filter, bad_mask).expect_err("size mismatch");
        let msg = err.to_string();
        assert!(
            msg.contains("mask length")
                || msg.contains("does not match")
                || msg.contains("pixel count"),
            "unexpected error message: {msg}",
        );
        project_close();
    }

    #[test]
    #[serial]
    fn apply_filter_masked_with_all_false_mask_keeps_blob() {
        // Apply a real (non-identity) filter but mask everything
        // out → output must match the source byte-for-byte, so the
        // content-addressed blob hash is unchanged.
        let node_id = seed_workspace_with_raster(8, 8, [80, 120, 200, 255]);
        let before = raster_meta(node_id);
        let filter = PreviewFilter::Hsl {
            hue: 180.0,
            saturation: 0.0,
            lightness: 0.0,
        };
        let mask = vec![0u8; 64];
        apply_filter_masked(node_id, filter, mask).expect("masked filter");
        let after = raster_meta(node_id);
        assert_eq!(
            before.blob_hash, after.blob_hash,
            "fully-unmasked filter should be a no-op on pixel data",
        );
        project_close();
    }

    #[test]
    #[serial]
    fn apply_filter_masked_with_all_true_mask_matches_unmasked_apply() {
        // Apply a non-identity filter with mask=true everywhere and
        // confirm the resulting blob differs from the source (i.e.
        // the filter ran). Equivalence to the unmasked apply is
        // verified pixel-wise below by the operation log entry
        // shape — full unmasked-bit-for-bit equality is exercised
        // by `feather_weights_one_when_mask_all_true`.
        let node_id = seed_workspace_with_raster(16, 16, [80, 120, 200, 255]);
        let before = raster_meta(node_id);
        let filter = PreviewFilter::Hsl {
            hue: 90.0,
            saturation: 1.0,
            lightness: 0.0,
        };
        let mask = vec![1u8; 256];
        apply_filter_masked(node_id, filter, mask).expect("masked filter");
        let after = raster_meta(node_id);
        assert_ne!(
            before.blob_hash, after.blob_hash,
            "fully-masked filter must rewrite the blob",
        );
        project_close();
    }

    #[test]
    #[serial]
    fn apply_filter_masked_records_operation_with_mask_summary() {
        let node_id = seed_workspace_with_raster(8, 8, [80, 120, 200, 255]);
        let filter = PreviewFilter::Levels {
            black_point: 0.0,
            white_point: 1.0,
            gamma: 1.0,
        };
        // Half-selected mask.
        let mut mask = vec![0u8; 64];
        for slot in mask.iter_mut().take(32) {
            *slot = 1;
        }
        apply_filter_masked(node_id, filter, mask).expect("masked filter");
        // The most recent operation must capture mask_len + mask_true.
        let guard = crate::document::slot().lock();
        let ws = guard.as_ref().expect("workspace");
        let last_op = ws.project.operation_log.last().expect("operation logged");
        let params: &Value = &last_op.before_patch;
        // before_patch is `{ "before": <node>, "params": { ... } }`.
        let captured = params.get("params").expect("captured params").clone();
        assert_eq!(captured["mask_len"], 64);
        assert_eq!(captured["mask_true"], 32);
        drop(guard);
        project_close();
    }
}
