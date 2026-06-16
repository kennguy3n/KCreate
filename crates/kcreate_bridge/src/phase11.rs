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
    // Phase 11 Block B follow-up round 4 — Devin Review ANALYSIS-0005
    // (r4). The pre-Phase-11 sync `export_png` returned
    // `u32::try_from(bytes).unwrap_or(u32::MAX)` — i.e. **silently**
    // capped the byte count for files >4 GB, so a caller asking
    // "how big is my export?" got a wrong-but-finite number. The
    // first Phase 11 port replaced the silent cap with a hard
    // `Promise` rejection, which is more correct but also a
    // user-facing behaviour change for the same overflow regime.
    // The right architectural fix is to remove the overflow regime
    // altogether: return the byte count as `f64` (which is what JS
    // `number` already is on the wire), giving us a precise integer
    // up to 2^53 bytes (≈9 PB) before any rounding. The N-API
    // export already advertises `Promise<number>` to TypeScript,
    // so this changes nothing for callers — it just preserves the
    // upper bits and eliminates the regression vs the sync API.
    type Output = f64;
    type JsValue = f64;

    fn compute(&mut self) -> NapiResult<Self::Output> {
        let opts = serde_json::from_str(&self.options_json)
            .map_err(|e| NapiError::from_reason(format!("invalid export_png options JSON: {e}")))?;
        let bytes = document::export_png_file(&self.output_path, &opts).map_err(map_doc_err)?;
        Ok(bytes as f64)
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
    // Phase 11 Block B follow-up round 4 — Devin Review ANALYSIS-0005
    // (r4). Same rationale as `ExportPngTask` above: return the byte
    // count as `f64` so the precision matches the wire-level
    // `Promise<number>` contract and we don't silently cap or hard
    // error on >4 GB exports.
    type Output = f64;
    type JsValue = f64;

    fn compute(&mut self) -> NapiResult<Self::Output> {
        let opts = serde_json::from_str(&self.options_json)
            .map_err(|e| NapiError::from_reason(format!("invalid export_pdf options JSON: {e}")))?;
        let bytes = document::export_pdf_file(&self.output_path, &opts).map_err(map_doc_err)?;
        Ok(bytes as f64)
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> NapiResult<Self::JsValue> {
        Ok(output)
    }
}

/// Async print-ready PDF export. Streams a press-ready PDF (bleed +
/// trim/registration marks + CMYK + spot separations) to disk and
/// resolves with the export outcome as JSON (media/trim box, bleed,
/// spot plates, color mode) so the PreflightPanel can summarise the
/// result without a second round-trip. Heavy (clips, marks, lopdf
/// post-process), so it runs on the napi worker pool.
#[derive(Debug)]
pub struct PrintReadyExportTask {
    pub output_path: PathBuf,
    pub request_json: String,
}

impl Task for PrintReadyExportTask {
    type Output = String;
    type JsValue = String;

    fn compute(&mut self) -> NapiResult<Self::Output> {
        let request: document::PrintReadyExportRequest = serde_json::from_str(&self.request_json)
            .map_err(|e| {
            NapiError::from_reason(format!("invalid export_print_ready options JSON: {e}"))
        })?;
        let outcome = document::export_print_ready_pdf_file(&self.output_path, &request)
            .map_err(map_doc_err)?;
        serde_json::to_string(&outcome).map_err(|e| NapiError::from_reason(e.to_string()))
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
        // Phase 11 Block B follow-up round 7 — Devin Review BUG-0001
        // (r7). The pre-r7 version of this comment claimed
        // `document::project_save` "snapshots under the workspace
        // lock, then drops the lock, then streams to SQLite", but
        // the implementation actually held `slot().write()` for the
        // entire SQLite stream. Round 7 fixed `project_save` to
        // match the contract: it now (1) snapshots Project fields
        // under a brief read lock, (2) clones the `Arc` to the
        // SQLite store, (3) drops the workspace lock, (4) streams
        // the snapshot to SQLite holding *only* the inner store
        // `Mutex`, and (5) takes a brief workspace write lock to
        // merge the newly-persisted op ids back into
        // `persisted_op_ids`. Dispatching this task to the napi
        // worker pool keeps the long step (4) off the libuv main
        // thread, and the workspace-lock-free design of step (4)
        // also means concurrent renderer reads / writes don't
        // block waiting on the save.
        document::project_save().map_err(map_doc_err)
    }

    fn resolve(&mut self, _env: Env, _output: Self::Output) -> NapiResult<Self::JsValue> {
        Ok(())
    }
}

// -- magic_resize_export_png --------------------------------------------------

/// Async Magic-Resize → batch PNG export. The resize is a single
/// undoable mutation, but the follow-on render of every generated
/// artboard to a PNG is CPU-heavy and runs across a rayon pool inside
/// [`document::magic_resize_export_png`]; on a large/multi-target design
/// that is easily multiple seconds. Running it on the libuv main thread
/// would freeze the Electron main process (window drag, menus, every
/// other IPC) for the duration, so we dispatch the whole operation to
/// the napi worker pool. The workspace lock is only held for the resize
/// plus scene build; the parallel render happens off the lock, so
/// concurrent renderer reads aren't blocked either.
#[derive(Debug)]
pub struct MagicResizeExportPngTask {
    pub source_artboard_id: Uuid,
    pub targets: Vec<document::ResizeTargetSpec>,
    pub request: document::MagicResizeExportRequest,
}

impl Task for MagicResizeExportPngTask {
    type Output = String;
    type JsValue = String;

    fn compute(&mut self) -> NapiResult<Self::Output> {
        let report = document::magic_resize_export_png(
            self.source_artboard_id,
            &self.targets,
            &self.request,
        )
        .map_err(map_doc_err)?;
        serde_json::to_string(&report).map_err(|e| NapiError::from_reason(e.to_string()))
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> NapiResult<Self::JsValue> {
        Ok(output)
    }
}
