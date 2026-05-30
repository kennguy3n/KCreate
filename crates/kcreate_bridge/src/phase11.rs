//! Phase 11 — async N-API wrappers for hot-path bridge calls.
//!
//! These tasks move the long-running portion of raster filters,
//! large exports, and the project save off the main libuv thread
//! and onto the napi-rs worker pool, so the renderer process can
//! keep painting + handling input during multi-second operations.
//!
//! The pattern (matching the Phase 4 `VisionDescribeImageTask` and
//! friends) is:
//!
//! 1. The N-API entry point parses arguments + builds a task struct
//!    on the main thread; this is cheap.
//! 2. The runtime calls `compute()` on a worker thread, which
//!    delegates to the existing sync helper in `raster_ops` /
//!    `document` / `phase9`. Those helpers already release the
//!    workspace lock for the heavy step, so the worker doesn't
//!    pin the main thread.
//! 3. `resolve()` runs back on the main thread and unwraps the
//!    result onto the JS Promise.
//!
//! Cancellation: the tasks are fire-and-forget; if the caller drops
//! the Promise, napi-rs still drives `compute()` to completion, but
//! the resolved value is discarded. This matches the pre-Phase-11
//! behaviour of the sync calls (which can't be cancelled either)
//! and avoids a race between "abort filter mid-pixel" and the
//! workspace lock.

use std::path::PathBuf;

use napi::{Env, Error as NapiError, Result as NapiResult, Task};
use uuid::Uuid;

use crate::document;
use crate::raster_ops::{self, BlurKind, FlipDirection, PreviewFilter};

/// Shared error mapper for the async tasks. Mirrors the private
/// `map_doc_err` helper in `lib.rs` (the canonical mapper for sync
/// N-API entry points) so the async wrappers surface the same
/// `Error::from_reason(...)` strings to JS.
fn map_doc_err(e: document::DocumentBridgeError) -> NapiError {
    NapiError::from_reason(e.to_string())
}

// -- raster_apply_levels ------------------------------------------------------

#[derive(Debug)]
pub struct RasterLevelsTask {
    pub node_id: Uuid,
    pub black_point: f32,
    pub white_point: f32,
    pub gamma: f32,
}

impl Task for RasterLevelsTask {
    type Output = ();
    type JsValue = ();

    fn compute(&mut self) -> NapiResult<Self::Output> {
        raster_ops::apply_levels(self.node_id, self.black_point, self.white_point, self.gamma)
            .map_err(map_doc_err)
    }

    fn resolve(&mut self, _env: Env, _output: Self::Output) -> NapiResult<Self::JsValue> {
        Ok(())
    }
}

// -- raster_apply_curves ------------------------------------------------------

#[derive(Debug)]
pub struct RasterCurvesTask {
    pub node_id: Uuid,
    pub points: Vec<(f32, f32)>,
}

impl Task for RasterCurvesTask {
    type Output = ();
    type JsValue = ();

    fn compute(&mut self) -> NapiResult<Self::Output> {
        raster_ops::apply_curves(self.node_id, self.points.clone()).map_err(map_doc_err)
    }

    fn resolve(&mut self, _env: Env, _output: Self::Output) -> NapiResult<Self::JsValue> {
        Ok(())
    }
}

// -- raster_apply_blur --------------------------------------------------------

#[derive(Debug)]
pub struct RasterBlurTask {
    pub node_id: Uuid,
    pub radius: f32,
    pub kind: BlurKind,
}

impl Task for RasterBlurTask {
    type Output = ();
    type JsValue = ();

    fn compute(&mut self) -> NapiResult<Self::Output> {
        raster_ops::apply_blur(self.node_id, self.radius, self.kind).map_err(map_doc_err)
    }

    fn resolve(&mut self, _env: Env, _output: Self::Output) -> NapiResult<Self::JsValue> {
        Ok(())
    }
}

// -- raster_apply_sharpen -----------------------------------------------------

#[derive(Debug)]
pub struct RasterSharpenTask {
    pub node_id: Uuid,
    pub radius: f32,
    pub amount: f32,
    pub threshold: u8,
}

impl Task for RasterSharpenTask {
    type Output = ();
    type JsValue = ();

    fn compute(&mut self) -> NapiResult<Self::Output> {
        raster_ops::apply_sharpen(self.node_id, self.radius, self.amount, self.threshold)
            .map_err(map_doc_err)
    }

    fn resolve(&mut self, _env: Env, _output: Self::Output) -> NapiResult<Self::JsValue> {
        Ok(())
    }
}

// -- raster_apply_hsl ---------------------------------------------------------

#[derive(Debug)]
pub struct RasterHslTask {
    pub node_id: Uuid,
    pub hue: f32,
    pub saturation: f32,
    pub lightness: f32,
}

impl Task for RasterHslTask {
    type Output = ();
    type JsValue = ();

    fn compute(&mut self) -> NapiResult<Self::Output> {
        raster_ops::apply_hsl(self.node_id, self.hue, self.saturation, self.lightness)
            .map_err(map_doc_err)
    }

    fn resolve(&mut self, _env: Env, _output: Self::Output) -> NapiResult<Self::JsValue> {
        Ok(())
    }
}

// -- raster_apply_color_balance ----------------------------------------------

#[derive(Debug)]
pub struct RasterColorBalanceTask {
    pub node_id: Uuid,
    pub shadows: [f32; 3],
    pub midtones: [f32; 3],
    pub highlights: [f32; 3],
}

impl Task for RasterColorBalanceTask {
    type Output = ();
    type JsValue = ();

    fn compute(&mut self) -> NapiResult<Self::Output> {
        raster_ops::apply_color_balance(self.node_id, self.shadows, self.midtones, self.highlights)
            .map_err(map_doc_err)
    }

    fn resolve(&mut self, _env: Env, _output: Self::Output) -> NapiResult<Self::JsValue> {
        Ok(())
    }
}

// -- raster_perspective -------------------------------------------------------

#[derive(Debug)]
pub struct RasterPerspectiveTask {
    pub node_id: Uuid,
    pub corners: [(f64, f64); 4],
}

impl Task for RasterPerspectiveTask {
    type Output = ();
    type JsValue = ();

    fn compute(&mut self) -> NapiResult<Self::Output> {
        raster_ops::apply_perspective(self.node_id, self.corners).map_err(map_doc_err)
    }

    fn resolve(&mut self, _env: Env, _output: Self::Output) -> NapiResult<Self::JsValue> {
        Ok(())
    }
}

// -- raster_apply_filter_masked ----------------------------------------------

#[derive(Debug)]
pub struct RasterFilterMaskedTask {
    pub node_id: Uuid,
    pub filter: PreviewFilter,
    pub mask: Vec<u8>,
}

impl Task for RasterFilterMaskedTask {
    type Output = ();
    type JsValue = ();

    fn compute(&mut self) -> NapiResult<Self::Output> {
        raster_ops::apply_filter_masked(self.node_id, self.filter.clone(), self.mask.clone())
            .map_err(map_doc_err)
    }

    fn resolve(&mut self, _env: Env, _output: Self::Output) -> NapiResult<Self::JsValue> {
        Ok(())
    }
}

// -- raster_crop -------------------------------------------------------------

#[derive(Debug)]
pub struct RasterCropTask {
    pub node_id: Uuid,
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl Task for RasterCropTask {
    type Output = ();
    type JsValue = ();

    fn compute(&mut self) -> NapiResult<Self::Output> {
        raster_ops::crop(self.node_id, self.x, self.y, self.w, self.h).map_err(map_doc_err)
    }

    fn resolve(&mut self, _env: Env, _output: Self::Output) -> NapiResult<Self::JsValue> {
        Ok(())
    }
}

// -- raster_rotate / raster_flip / raster_heal --------------------------------
//
// The audit identified rotate/flip/heal as filter-class ops that
// also benefit from running on the worker pool; they take the same
// "load → transform → write" path inside `raster_ops` and are
// equally subject to the main-thread freeze on large layers.

#[derive(Debug)]
pub struct RasterRotateTask {
    pub node_id: Uuid,
    pub angle_deg: f32,
}

impl Task for RasterRotateTask {
    type Output = ();
    type JsValue = ();

    fn compute(&mut self) -> NapiResult<Self::Output> {
        raster_ops::rotate(self.node_id, self.angle_deg).map_err(map_doc_err)
    }

    fn resolve(&mut self, _env: Env, _output: Self::Output) -> NapiResult<Self::JsValue> {
        Ok(())
    }
}

#[derive(Debug)]
pub struct RasterFlipTask {
    pub node_id: Uuid,
    pub direction: FlipDirection,
}

impl Task for RasterFlipTask {
    type Output = ();
    type JsValue = ();

    fn compute(&mut self) -> NapiResult<Self::Output> {
        raster_ops::flip(self.node_id, self.direction).map_err(map_doc_err)
    }

    fn resolve(&mut self, _env: Env, _output: Self::Output) -> NapiResult<Self::JsValue> {
        Ok(())
    }
}

#[derive(Debug)]
pub struct RasterHealTask {
    pub node_id: Uuid,
    pub src_x: u32,
    pub src_y: u32,
    pub dst_x: u32,
    pub dst_y: u32,
    pub radius: u32,
}

impl Task for RasterHealTask {
    type Output = ();
    type JsValue = ();

    fn compute(&mut self) -> NapiResult<Self::Output> {
        raster_ops::heal(
            self.node_id,
            self.src_x,
            self.src_y,
            self.dst_x,
            self.dst_y,
            self.radius,
        )
        .map_err(map_doc_err)
    }

    fn resolve(&mut self, _env: Env, _output: Self::Output) -> NapiResult<Self::JsValue> {
        Ok(())
    }
}

// -- exports ------------------------------------------------------------------

#[derive(Debug)]
pub struct ExportPngTask {
    pub output_path: PathBuf,
    pub options_json: String,
}

impl Task for ExportPngTask {
    type Output = u32;
    type JsValue = u32;

    fn compute(&mut self) -> NapiResult<Self::Output> {
        let opts = serde_json::from_str(&self.options_json)
            .map_err(|e| NapiError::from_reason(format!("invalid export_png options JSON: {e}")))?;
        let bytes = document::export_png_file(&self.output_path, &opts).map_err(map_doc_err)?;
        u32::try_from(bytes)
            .map_err(|_| NapiError::from_reason("export_png byte count overflows u32"))
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> NapiResult<Self::JsValue> {
        Ok(output)
    }
}

#[derive(Debug)]
pub struct ExportPdfTask {
    pub output_path: PathBuf,
    pub options_json: String,
}

impl Task for ExportPdfTask {
    type Output = u32;
    type JsValue = u32;

    fn compute(&mut self) -> NapiResult<Self::Output> {
        let opts = serde_json::from_str(&self.options_json)
            .map_err(|e| NapiError::from_reason(format!("invalid export_pdf options JSON: {e}")))?;
        let bytes = document::export_pdf_file(&self.output_path, &opts).map_err(map_doc_err)?;
        u32::try_from(bytes)
            .map_err(|_| NapiError::from_reason("export_pdf byte count overflows u32"))
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> NapiResult<Self::JsValue> {
        Ok(output)
    }
}

/// Async SVG export for documents with > 100 nodes. The small-doc
/// path stays sync (`export_svg` in lib.rs) — a 5-node SVG export
/// is microseconds and the worker dispatch overhead would dominate.
#[derive(Debug)]
pub struct ExportSvgAsyncTask {
    pub node_ids: Vec<Uuid>,
    pub options_json: String,
}

impl Task for ExportSvgAsyncTask {
    type Output = String;
    type JsValue = String;

    fn compute(&mut self) -> NapiResult<Self::Output> {
        let opts = serde_json::from_str(&self.options_json)
            .map_err(|e| NapiError::from_reason(format!("invalid export_svg options JSON: {e}")))?;
        document::export_svg(&self.node_ids, &opts).map_err(map_doc_err)
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> NapiResult<Self::JsValue> {
        Ok(output)
    }
}

// -- project_save -------------------------------------------------------------

#[derive(Debug, Default)]
pub struct ProjectSaveTask;

impl Task for ProjectSaveTask {
    type Output = ();
    type JsValue = ();

    fn compute(&mut self) -> NapiResult<Self::Output> {
        // `document::project_save` already snapshots the document
        // graph under the workspace lock, drops the lock, then
        // streams the snapshot to SQLite. Dispatching the whole
        // call to the worker pool means even the snapshot step
        // (which traverses the node tree) doesn't pin the main
        // thread on multi-thousand-node projects.
        document::project_save().map_err(map_doc_err)
    }

    fn resolve(&mut self, _env: Env, _output: Self::Output) -> NapiResult<Self::JsValue> {
        Ok(())
    }
}
