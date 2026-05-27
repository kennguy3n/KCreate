//! Phase 2 bridge entry points: preflight, icon pack, parallel batch
//! export, AI model packs, screenshot-to-layout, plugin sandbox, and
//! MCP permission persistence.
//!
//! Logic here is invoked from the thin N-API wrappers in `lib.rs`. All
//! functions return `crate::document::Result<T>` so the N-API layer
//! can use the existing [`DocumentBridgeError`] → `NapiError` mapping.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread::{self, JoinHandle};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use chrono::Utc;
use kcreate_core::node::{Bounds, Node, NodeType};
use kcreate_core::operation::Operation;
use kcreate_export::batch::{
    run_batch_parallel, BatchCancel, BatchExportJob, BatchProgress, BatchResult,
};
use kcreate_export::icon_pack::{generate_icon_pack, IconPackPlatform};
use kcreate_export::pdf::RasterPixelCache;
use kcreate_export::pdf_import::{
    import_pdf as pdf_import_run, ExtractedImageData, ImportedPdf, PdfImportError,
};
use kcreate_export::preflight::{run_preflight_with_spots, PreflightIssue, PreflightOptions};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::document::{
    blob_load, current_scene_safe, sync_scene_after_change, with_workspace, with_workspace_mut,
    DocumentBridgeError, Result,
};

// -----------------------------------------------------------------------------
// Preflight
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PreflightRequest {
    #[serde(default)]
    pub page_ids: Vec<String>,
    #[serde(default)]
    pub options: PreflightOptions,
}

pub fn preflight_run(req: &PreflightRequest) -> Result<Vec<PreflightIssue>> {
    let pages: Vec<Uuid> = req
        .page_ids
        .iter()
        .map(|s| Uuid::parse_str(s).map_err(|e| DocumentBridgeError::InvalidUuid(s.clone(), e)))
        .collect::<Result<Vec<Uuid>>>()?;
    with_workspace(|ws| {
        Ok(run_preflight_with_spots(
            &ws.project.document,
            &pages,
            &req.options,
            &ws.project.spot_color_library,
        ))
    })
}

// -----------------------------------------------------------------------------
// Icon pack
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct IconPackRequest {
    #[serde(default)]
    pub node_ids: Vec<String>,
    pub platforms: Vec<IconPackPlatform>,
    pub output_dir: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct IconPackOutcome {
    pub files: Vec<PathBuf>,
}

pub fn icon_pack_export(req: &IconPackRequest) -> Result<IconPackOutcome> {
    let ids: Vec<Uuid> = req
        .node_ids
        .iter()
        .map(|s| Uuid::parse_str(s).map_err(|e| DocumentBridgeError::InvalidUuid(s.clone(), e)))
        .collect::<Result<Vec<Uuid>>>()?;
    let output_dir = PathBuf::from(&req.output_dir);
    // Snapshot the Scene and a cheap Clone of the DocumentGraph under
    // the workspace lock, then release it before invoking the
    // (potentially slow) renderer. A web + iOS + Android + favicon
    // pack rasterises 30+ sizes, and holding the workspace mutex for
    // the duration would block every other N-API call — including the
    // renderer's per-frame snapshot — during the icon export. Per
    // Devin Review 3289537901.
    let scene = current_scene_safe()?;
    let document = with_workspace(|ws| Ok(ws.project.document.clone()))?;
    let result = generate_icon_pack(&scene, &document, &ids, &req.platforms, &output_dir)
        .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))?;
    std::fs::create_dir_all(&output_dir)?;
    let mut written: Vec<PathBuf> = Vec::with_capacity(result.files.len());
    for (path, bytes) in result.files {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, bytes)?;
        written.push(path);
    }
    Ok(IconPackOutcome { files: written })
}

// -----------------------------------------------------------------------------
// Parallel batch export (async-job model)
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchJobStatus {
    pub job_id: String,
    pub completed: usize,
    pub total: usize,
    pub current_item: String,
    pub finished: bool,
    pub cancelled: bool,
    pub succeeded: Vec<String>,
    pub failed: Vec<(String, String)>,
    pub duration_ms: u64,
}

struct BatchHandle {
    cancel: BatchCancel,
    progress: Arc<Mutex<BatchProgress>>,
    result: Arc<Mutex<Option<BatchResult>>>,
    join: Mutex<Option<JoinHandle<()>>>,
}

fn batch_table() -> &'static Mutex<HashMap<String, Arc<BatchHandle>>> {
    static T: OnceLock<Mutex<HashMap<String, Arc<BatchHandle>>>> = OnceLock::new();
    T.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn batch_start(job: BatchExportJob) -> Result<String> {
    // Snapshot what the worker thread needs: the document graph and the
    // raster pixel cache. We can't hand it the live `Workspace` lock —
    // the worker thread is not allowed to call back into the editing
    // path while a job is running.
    let (doc, rasters) = with_workspace(|ws| {
        let mut rasters = RasterPixelCache::new();
        for (_uuid, node) in ws.project.document.iter() {
            if !matches!(node.node_type, NodeType::RasterLayer) {
                continue;
            }
            let Some(meta_value) = node
                .metadata
                .get(crate::scene_sync::RASTER_IMAGE_METADATA_KEY)
            else {
                continue;
            };
            let Ok(meta) =
                serde_json::from_value::<crate::scene_sync::RasterImageMeta>(meta_value.clone())
            else {
                continue;
            };
            if rasters.contains_key(&meta.blob_hash) {
                continue;
            }
            if let Ok(bytes) = ws.store.blobs().load(&meta.blob_hash) {
                if let Ok(pixels) = kcreate_export::pdf::RasterPixels::decode(&bytes) {
                    rasters.insert(meta.blob_hash, pixels);
                }
            }
        }
        Ok((ws.project.document.clone(), rasters))
    })?;

    let job_id = job.id.to_string();
    let cancel = BatchCancel::new();
    let progress = Arc::new(Mutex::new(BatchProgress::default()));
    let result_slot: Arc<Mutex<Option<BatchResult>>> = Arc::new(Mutex::new(None));

    let cancel_clone = cancel.clone();
    let progress_clone = progress.clone();
    let result_clone = result_slot.clone();
    let join = thread::spawn(move || {
        let p = progress_clone.clone();
        let outcome =
            run_batch_parallel(&job, &doc, &rasters, cancel_clone.as_inner(), move |snap| {
                // Rayon workers complete in arbitrary order, so a later-
                // started thread can race ahead and write a higher
                // `completed` snapshot before an earlier thread's callback
                // runs. Without this guard the UI would briefly see the
                // counter go backwards on every poll. We compare against
                // the published snapshot and only adopt monotonically
                // newer values; the final terminal status is published by
                // the outer `result_slot` so we never need to "catch up"
                // to total here.
                let mut guard = p.lock();
                if snap.completed > guard.completed {
                    *guard = snap;
                }
            });
        match outcome {
            Ok(r) => *result_clone.lock() = Some(r),
            Err(e) => {
                *result_clone.lock() = Some(BatchResult {
                    succeeded: Vec::new(),
                    failed: vec![("batch".to_string(), e.to_string())],
                    duration_ms: 0,
                    cancelled: false,
                });
            }
        }
    });

    let handle = Arc::new(BatchHandle {
        cancel,
        progress,
        result: result_slot,
        join: Mutex::new(Some(join)),
    });
    batch_table().lock().insert(job_id.clone(), handle);
    Ok(job_id)
}

pub fn batch_status(job_id: &str) -> Result<BatchJobStatus> {
    let handle = batch_table().lock().get(job_id).cloned().ok_or_else(|| {
        DocumentBridgeError::Io(std::io::Error::other(format!("unknown job {job_id}")))
    })?;
    let progress = handle.progress.lock().clone();
    let mut status = BatchJobStatus {
        job_id: job_id.to_string(),
        completed: progress.completed,
        total: progress.total,
        current_item: progress.current_item,
        finished: false,
        cancelled: false,
        succeeded: Vec::new(),
        failed: Vec::new(),
        duration_ms: 0,
    };
    // Read `finished` and the result payload under a *single* lock
    // acquisition. The original code took the lock twice — once for
    // `result.lock().is_some()` and again for the `if let Some(r) =
    // result.lock().as_ref()` extraction — which let the worker
    // thread complete between the two reads and produced a status
    // where `finished == false` but `succeeded` / `failed` /
    // `duration_ms` were already populated (Devin Review 3289450816).
    // Holding the lock for the whole snapshot makes that race
    // impossible.
    let finished_now = {
        let guard = handle.result.lock();
        if let Some(r) = guard.as_ref() {
            status.succeeded = r
                .succeeded
                .iter()
                .map(|p| p.display().to_string())
                .collect();
            status.failed.clone_from(&r.failed);
            status.duration_ms = r.duration_ms;
            status.cancelled = r.cancelled;
            status.finished = true;
            true
        } else {
            false
        }
    };
    if finished_now {
        // Reap the worker thread now that we know it's done. We do
        // *not* remove the handle from the global table — see
        // [`batch_dismiss`] for why. Repeated polls after a terminal
        // status are explicitly allowed and return the same terminal
        // status idempotently. Per Devin Review
        // BUG_pr-review-job-e31d5461e1ff4359ad80d927af5d0b54_0002.
        let join_handle = handle.join.lock().take();
        if let Some(j) = join_handle {
            let _ = j.join();
        }
    }
    Ok(status)
}

pub fn batch_cancel(job_id: &str) -> Result<()> {
    let handle = batch_table().lock().get(job_id).cloned().ok_or_else(|| {
        DocumentBridgeError::Io(std::io::Error::other(format!("unknown job {job_id}")))
    })?;
    handle.cancel.cancel();
    Ok(())
}

/// Explicitly release the bookkeeping state for `job_id`.
///
/// The previous design removed the handle the *first* time a caller
/// observed `finished: true` in [`batch_status`]. That made the
/// status API one-shot: any second poll — including one already
/// in-flight across the Electron IPC bridge when the terminal
/// response was sent — failed with `unknown job <id>`. Naive UI
/// polling loops (`setInterval` that only clears on receiving
/// `finished` *and* observing it on the main thread) tripped over
/// this.
///
/// Now the handle stays alive after completion; repeated
/// [`batch_status`] calls return the same terminal payload
/// idempotently. The UI is expected to call [`batch_dismiss`] once
/// it has rendered the terminal status (or no longer cares). Memory
/// growth is bounded by the number of batches the user actually
/// starts in a session, not by polling frequency.
///
/// Dismissing an unknown job id is a no-op (so duplicate dismiss
/// calls don't surface as errors). Returns `true` if a handle was
/// actually dropped, `false` if the id was already gone.
pub fn batch_dismiss(job_id: &str) -> Result<bool> {
    let handle = batch_table().lock().remove(job_id);
    if let Some(h) = handle {
        // Join the worker thread if [`batch_status`] never observed
        // the terminal status. This is a belt-and-braces measure
        // for the case where the UI dismisses a job that's still
        // mid-flight — the cancel flag is flipped so the worker
        // will exit at its next checkpoint.
        h.cancel.cancel();
        let join_handle = h.join.lock().take();
        if let Some(j) = join_handle {
            let _ = j.join();
        }
        Ok(true)
    } else {
        Ok(false)
    }
}

// -----------------------------------------------------------------------------
// AI model packs
// -----------------------------------------------------------------------------

pub fn ai_models_list() -> Result<String> {
    let dir = ai_models_dir();
    let packs = kcreate_ai::list_model_packs(&dir);
    Ok(serde_json::to_string(&packs)?)
}

/// Install an optional model pack from a user-provided source path.
/// The bridge passes through to [`kcreate_ai::install_model_pack`];
/// the JSON return value is the full [`kcreate_ai::InstallReport`]
/// so the UI can show the resulting hash + verified flag.
pub fn ai_model_install(pack_id: String, source_path: String) -> Result<String> {
    let dir = ai_models_dir();
    let source = std::path::PathBuf::from(source_path);
    let report = kcreate_ai::install_model_pack(&pack_id, &source, &dir)
        .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))?;
    Ok(serde_json::to_string(&report)?)
}

/// Uninstall an optional model pack by deleting its file from the
/// models directory. Built-in packs are rejected — see
/// [`kcreate_ai::uninstall_model_pack`].
pub fn ai_model_uninstall(pack_id: String) -> Result<()> {
    let dir = ai_models_dir();
    kcreate_ai::uninstall_model_pack(&pack_id, &dir)
        .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))?;
    Ok(())
}

pub(crate) fn ai_models_dir() -> PathBuf {
    if let Ok(env) = std::env::var("KCREATE_MODELS_DIR") {
        return PathBuf::from(env);
    }
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".kcreate").join("models")
}

pub fn ai_upscale(node_id: Uuid, scale: f64) -> Result<Uuid> {
    // Load encoded image and decode.
    let (encoded, parent) = with_workspace(|ws| {
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
            .get(crate::scene_sync::RASTER_IMAGE_METADATA_KEY)
            .ok_or_else(|| {
                DocumentBridgeError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "raster layer missing image metadata",
                ))
            })?;
        let meta: crate::scene_sync::RasterImageMeta = serde_json::from_value(meta_value.clone())?;
        let bytes = blob_load(ws, &meta.blob_hash)?;
        Ok((bytes, node.parent_id))
    })?;

    let img = image::load_from_memory(&encoded).map_err(|e| {
        DocumentBridgeError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    })?;
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    let (out_pixels, ow, oh) = kcreate_ai::upscale_lanczos(rgba.as_raw(), width, height, scale)
        .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))?;
    let mut png: Vec<u8> = Vec::new();
    {
        let mut cursor = std::io::Cursor::new(&mut png);
        image::write_buffer_with_format(
            &mut cursor,
            &out_pixels,
            ow,
            oh,
            image::ColorType::Rgba8,
            image::ImageFormat::Png,
        )
        .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))?;
    }

    // Insert resulting node + op + ai action.
    let new_id = with_workspace_mut(|ws| {
        let blob = ws
            .store
            .blobs()
            .store(&png, "image/png")
            .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))?;
        let new_meta = crate::scene_sync::RasterImageMeta {
            blob_hash: blob.hash,
            width: ow,
            height: oh,
        };
        let mut new_node = Node::new(NodeType::RasterLayer, "Upscaled");
        new_node.parent_id = parent;
        new_node.bounds = Bounds {
            x: 0.0,
            y: 0.0,
            width: f64::from(ow),
            height: f64::from(oh),
        };
        new_node.metadata.insert(
            crate::scene_sync::RASTER_IMAGE_METADATA_KEY.to_string(),
            serde_json::to_value(&new_meta)?,
        );
        let new_id = ws.project.document.insert_node(new_node)?;
        let snapshot = ws
            .project
            .document
            .get_node(new_id)
            .map_or(serde_json::Value::Null, |n| {
                serde_json::to_value(n).unwrap_or(serde_json::Value::Null)
            });
        let op = Operation::new(
            "ai",
            "ai_upscale",
            serde_json::json!({ "scale": scale }),
            snapshot,
            vec![new_id, node_id],
        )
        .as_ai_generated();
        ws.project.execute_operation(op);
        kcreate_ai::ActionLog::global()
            .lock()
            .append(kcreate_ai::AiAction {
                id: Uuid::new_v4(),
                timestamp: Utc::now(),
                task_type: "upscale".into(),
                model: "lanczos3".into(),
                compute_device: "cpu".into(),
                affected_nodes: vec![new_id, node_id],
                confidence: None,
            });
        ws.project.modified_at = Utc::now();
        Ok(new_id)
    })?;
    sync_scene_after_change();
    Ok(new_id)
}

pub fn ai_extract_palette(node_id: Uuid, max_colors: usize) -> Result<String> {
    let encoded = with_workspace(|ws| {
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
            .get(crate::scene_sync::RASTER_IMAGE_METADATA_KEY)
            .ok_or_else(|| {
                DocumentBridgeError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "raster layer missing image metadata",
                ))
            })?;
        let meta: crate::scene_sync::RasterImageMeta = serde_json::from_value(meta_value.clone())?;
        blob_load(ws, &meta.blob_hash)
    })?;
    let img = image::load_from_memory(&encoded).map_err(|e| {
        DocumentBridgeError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    })?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    let palette = kcreate_ai::extract_palette(rgba.as_raw(), w, h, max_colors);
    Ok(serde_json::to_string(&palette)?)
}

pub fn ai_smart_select(node_id: Uuid, x: u32, y: u32, tolerance: f64) -> Result<String> {
    let encoded = with_workspace(|ws| {
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
            .get(crate::scene_sync::RASTER_IMAGE_METADATA_KEY)
            .ok_or_else(|| {
                DocumentBridgeError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "raster layer missing image metadata",
                ))
            })?;
        let meta: crate::scene_sync::RasterImageMeta = serde_json::from_value(meta_value.clone())?;
        blob_load(ws, &meta.blob_hash)
    })?;
    let img = image::load_from_memory(&encoded).map_err(|e| {
        DocumentBridgeError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    })?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    let mask = kcreate_ai::smart_select(rgba.as_raw(), w, h, x, y, tolerance);
    Ok(B64.encode(&mask))
}

// -----------------------------------------------------------------------------
// Phase 3 Tasks 9-10 — backend-selectable upscale + point-prompt
// segmentation. Both bridge entries accept the backend as a string
// ("lanczos3" / "esrgan" / "edge_aware" / "sam") so the renderer can
// flip backends without a wire-format change. The ONNX backends are
// gated behind Cargo features on `kcreate_ai`; when those features
// are off the underlying enum returns `BackendUnavailable` and the
// renderer can fall back to the built-in path.
// -----------------------------------------------------------------------------

/// Result wire shape for `ai_upscale_with_backend`. Mirrors the
/// existing single-node-return contract for `ai_upscale`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpscaleWithBackendReport {
    pub new_node_id: Uuid,
    pub backend: String,
    pub output_width: u32,
    pub output_height: u32,
}

/// Upscale a raster layer with the caller-selected backend.
/// `backend` accepts the serde representation of
/// [`kcreate_ai::UpscaleBackend`] (`"lanczos3"` / `"esrgan"`).
pub fn ai_upscale_with_backend(
    node_id: Uuid,
    scale: f64,
    backend: &str,
    model_path: Option<&str>,
) -> Result<String> {
    let parsed_backend: kcreate_ai::UpscaleBackend =
        serde_json::from_value(serde_json::Value::String(backend.into())).map_err(|_| {
            DocumentBridgeError::InvalidArgument {
                argument: "backend".into(),
                value: backend.into(),
            }
        })?;

    let (encoded, parent) = with_workspace(|ws| {
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
            .get(crate::scene_sync::RASTER_IMAGE_METADATA_KEY)
            .ok_or_else(|| {
                DocumentBridgeError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "raster layer missing image metadata",
                ))
            })?;
        let meta: crate::scene_sync::RasterImageMeta = serde_json::from_value(meta_value.clone())?;
        let bytes = blob_load(ws, &meta.blob_hash)?;
        Ok((bytes, node.parent_id))
    })?;

    let img = image::load_from_memory(&encoded).map_err(|e| {
        DocumentBridgeError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    })?;
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    let model_path_buf = model_path.map(std::path::PathBuf::from);
    let (out_pixels, ow, oh) = kcreate_ai::upscale_with_backend(
        rgba.as_raw(),
        width,
        height,
        scale,
        parsed_backend,
        model_path_buf.as_deref(),
    )
    .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))?;

    let mut png: Vec<u8> = Vec::new();
    {
        let mut cursor = std::io::Cursor::new(&mut png);
        image::write_buffer_with_format(
            &mut cursor,
            &out_pixels,
            ow,
            oh,
            image::ColorType::Rgba8,
            image::ImageFormat::Png,
        )
        .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))?;
    }

    let model_name = match parsed_backend {
        kcreate_ai::UpscaleBackend::Lanczos3 => "lanczos3",
        kcreate_ai::UpscaleBackend::Esrgan => "esrgan",
    };

    let new_id = with_workspace_mut(|ws| {
        let blob = ws
            .store
            .blobs()
            .store(&png, "image/png")
            .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))?;
        let new_meta = crate::scene_sync::RasterImageMeta {
            blob_hash: blob.hash,
            width: ow,
            height: oh,
        };
        let mut new_node = Node::new(NodeType::RasterLayer, "Upscaled");
        new_node.parent_id = parent;
        new_node.bounds = Bounds {
            x: 0.0,
            y: 0.0,
            width: f64::from(ow),
            height: f64::from(oh),
        };
        new_node.metadata.insert(
            crate::scene_sync::RASTER_IMAGE_METADATA_KEY.to_string(),
            serde_json::to_value(&new_meta)?,
        );
        let new_id = ws.project.document.insert_node(new_node)?;
        let snapshot = ws
            .project
            .document
            .get_node(new_id)
            .map_or(serde_json::Value::Null, |n| {
                serde_json::to_value(n).unwrap_or(serde_json::Value::Null)
            });
        let op = Operation::new(
            "ai",
            "ai_upscale",
            serde_json::json!({ "scale": scale, "backend": model_name }),
            snapshot,
            vec![new_id, node_id],
        )
        .as_ai_generated();
        ws.project.execute_operation(op);
        kcreate_ai::ActionLog::global()
            .lock()
            .append(kcreate_ai::AiAction {
                id: Uuid::new_v4(),
                timestamp: Utc::now(),
                task_type: "upscale".into(),
                model: model_name.into(),
                compute_device: "cpu".into(),
                affected_nodes: vec![new_id, node_id],
                confidence: None,
            });
        ws.project.modified_at = Utc::now();
        Ok(new_id)
    })?;
    sync_scene_after_change();
    let report = UpscaleWithBackendReport {
        new_node_id: new_id,
        backend: model_name.into(),
        output_width: ow,
        output_height: oh,
    };
    Ok(serde_json::to_string(&report)?)
}

/// Result wire shape for `ai_segment`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SegmentReport {
    pub backend: String,
    pub width: u32,
    pub height: u32,
    pub mask_base64: String,
    pub area: u64,
    pub confidence: f32,
}

/// Point-prompt segmentation. Returns a single-channel mask
/// (`width * height` bytes, `0` = background, `255` = foreground)
/// base64-encoded so it crosses the N-API boundary as a string.
pub fn ai_segment(
    node_id: Uuid,
    point_x: u32,
    point_y: u32,
    tolerance: f64,
    edge_threshold: f64,
    backend: &str,
    model_path: Option<&str>,
) -> Result<String> {
    let parsed: kcreate_ai::SegmentBackend =
        serde_json::from_value(serde_json::Value::String(backend.into())).map_err(|_| {
            DocumentBridgeError::InvalidArgument {
                argument: "backend".into(),
                value: backend.into(),
            }
        })?;
    let encoded = with_workspace(|ws| {
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
            .get(crate::scene_sync::RASTER_IMAGE_METADATA_KEY)
            .ok_or_else(|| {
                DocumentBridgeError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "raster layer missing image metadata",
                ))
            })?;
        let meta: crate::scene_sync::RasterImageMeta = serde_json::from_value(meta_value.clone())?;
        blob_load(ws, &meta.blob_hash)
    })?;
    let img = image::load_from_memory(&encoded).map_err(|e| {
        DocumentBridgeError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    })?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    let opts = kcreate_ai::SegmentOptions {
        point_x,
        point_y,
        tolerance,
        edge_threshold,
    };
    let model_path_buf = model_path.map(std::path::PathBuf::from);
    let result = kcreate_ai::segment_with_backend(
        rgba.as_raw(),
        w,
        h,
        &opts,
        parsed,
        model_path_buf.as_deref(),
    )
    .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))?;
    let mask = result.masks.into_iter().next().ok_or_else(|| {
        DocumentBridgeError::Io(std::io::Error::other("segmentation produced no masks"))
    })?;
    let backend_name = match parsed {
        kcreate_ai::SegmentBackend::EdgeAware => "edge_aware",
        kcreate_ai::SegmentBackend::Sam => "sam",
    };
    kcreate_ai::ActionLog::global()
        .lock()
        .append(kcreate_ai::AiAction {
            id: Uuid::new_v4(),
            timestamp: Utc::now(),
            task_type: "segment".into(),
            model: backend_name.into(),
            compute_device: "cpu".into(),
            affected_nodes: vec![node_id],
            confidence: Some(mask.confidence),
        });
    let report = SegmentReport {
        backend: backend_name.into(),
        width: mask.width,
        height: mask.height,
        mask_base64: B64.encode(&mask.mask),
        area: mask.area,
        confidence: mask.confidence,
    };
    Ok(serde_json::to_string(&report)?)
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ScreenshotRequest {
    pub image_base64: String,
    pub width: u32,
    pub height: u32,
}

pub fn ai_screenshot_to_layout(req: &ScreenshotRequest) -> Result<String> {
    let pixels = B64.decode(req.image_base64.as_bytes()).map_err(|e| {
        DocumentBridgeError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    })?;
    let elements = kcreate_ai::analyze_screenshot_for_layout(&pixels, req.width, req.height);
    Ok(serde_json::to_string(&elements)?)
}

// -----------------------------------------------------------------------------
// Text-region detection + insert-as-text-layer (Phase 4 Block D)
// -----------------------------------------------------------------------------

/// Run the local text-region detector against the raster layer
/// identified by `node_id` and return the resulting
/// `Vec<TextRegion>` as JSON. Read-only — no graph mutation.
///
/// The detector lives in `kcreate_ai::ocr::detect_text_regions`;
/// see the module-level doc there for the algorithm. Coordinates
/// in the returned regions are in raster-pixel space; the
/// renderer maps them into document space using the raster
/// layer's `bounds` + intrinsic dimensions before previewing /
/// inserting.
///
/// `options_json` is the JSON form of [`OcrDetectOptions`] (a
/// `camelCase` mirror of `kcreate_ai::DetectTextRegionsOptions`);
/// pass `null` from the renderer to use defaults. We accept JSON
/// rather than threading every field through the N-API signature
/// so the wire surface stays compact as the option set grows.
pub fn ai_detect_text_regions(node_id: Uuid, options_json: &str) -> Result<String> {
    let options: kcreate_ai::DetectTextRegionsOptions =
        if options_json.trim().is_empty() || options_json.trim() == "null" {
            kcreate_ai::DetectTextRegionsOptions::default()
        } else {
            serde_json::from_str(options_json)?
        };
    let encoded = with_workspace(|ws| {
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
            .get(crate::scene_sync::RASTER_IMAGE_METADATA_KEY)
            .ok_or_else(|| {
                DocumentBridgeError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "raster layer missing image metadata",
                ))
            })?;
        let meta: crate::scene_sync::RasterImageMeta = serde_json::from_value(meta_value.clone())?;
        blob_load(ws, &meta.blob_hash)
    })?;
    let img = image::load_from_memory(&encoded).map_err(|e| {
        DocumentBridgeError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    })?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    let regions = kcreate_ai::detect_text_regions(rgba.as_raw(), w, h, options).map_err(|e| {
        DocumentBridgeError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    })?;
    Ok(serde_json::to_string(&regions)?)
}

/// Renderer → bridge request to materialise a detected text region
/// as a new `TextLayer`. The region is supplied in raster-pixel
/// space (matching the wire shape returned by
/// [`ai_detect_text_regions`]); the bridge maps it into document
/// space using the source raster's bounds.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct InsertTextLayerForRegionRequest {
    /// Raster layer the region was detected on. Determines the
    /// parent + the source bounds for coordinate mapping.
    pub raster_node_id: Uuid,
    /// Region in raster-pixel space. Identical wire shape to
    /// [`kcreate_ai::TextRegion`] for round-trip ergonomics.
    pub region: TextRegionInsert,
    /// Initial text content. May be empty — the user typically
    /// types the recognised text after insertion since the
    /// detector reports bboxes, not characters.
    #[serde(default)]
    pub text: String,
    /// Override the renderer-side font family. Defaults to the
    /// project's default sans-serif when empty / omitted.
    #[serde(default)]
    pub font_family: Option<String>,
    /// Override the heuristic font size. When `None`, the bridge
    /// uses the region's `height * font_size_height_ratio` (see
    /// the constant below).
    #[serde(default)]
    pub font_size: Option<f32>,
}

/// Wire shape for a single region in a
/// [`InsertTextLayerForRegionRequest`]. Mirrors
/// [`kcreate_ai::TextRegion`] field-by-field so the renderer can
/// pass the detector's output through unchanged. We don't reuse
/// `TextRegion` directly because the detector's struct doesn't
/// derive `Deserialize` for camelCase by default (its serde
/// rename rule applies to serialise + deserialise symmetrically,
/// so this is actually redundant — but the explicit mirror also
/// gives us `#[serde(deny_unknown_fields)]` which we want on the
/// bridge surface and don't on the detector struct).
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TextRegionInsert {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub glyph_count: u32,
    #[serde(default)]
    pub estimated_char_count: u32,
}

/// Heuristic ratio: an upper-case glyph fills roughly 70% of the
/// detected line's bbox vertically (the rest is ascender + descender
/// padding). Empirical sweet-spot — too low and the inserted text
/// shrinks below the raster glyphs; too high and ascenders clip the
/// next line when the user types multi-line text.
const FONT_SIZE_HEIGHT_RATIO: f32 = 0.75;

/// Materialise a detected text region as a new `TextLayer` sibling
/// of the source raster.
///
/// Returns the new node id. Records an `ai_insert_text_layer` op in
/// the project log + an AI action so the operation is undoable and
/// attributable in the action log.
pub fn ai_insert_text_layer_for_region(req: &InsertTextLayerForRegionRequest) -> Result<Uuid> {
    let raster_node_id = req.raster_node_id;
    let region = req.region;
    let new_id = with_workspace_mut(|ws| {
        let raster = ws
            .project
            .document
            .get_node(raster_node_id)
            .ok_or(DocumentBridgeError::NodeNotFound(raster_node_id))?;
        if !matches!(raster.node_type, NodeType::RasterLayer) {
            return Err(DocumentBridgeError::InvalidNodeType(format!(
                "{:?}",
                raster.node_type
            )));
        }
        let parent = raster.parent_id;
        let raster_bounds = raster.bounds;
        let meta_value = raster
            .metadata
            .get(crate::scene_sync::RASTER_IMAGE_METADATA_KEY)
            .ok_or_else(|| {
                DocumentBridgeError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "raster layer missing image metadata",
                ))
            })?;
        let raster_meta: crate::scene_sync::RasterImageMeta =
            serde_json::from_value(meta_value.clone())?;
        if raster_meta.width == 0 || raster_meta.height == 0 {
            return Err(DocumentBridgeError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "raster layer reports zero intrinsic dimensions",
            )));
        }
        // Map raster-pixel space → document space. Linear scale by
        // the raster bounds / intrinsic pixel size; raster pixel
        // (0,0) maps to (bounds.x, bounds.y), pixel (w, h) maps to
        // (bounds.x + bounds.width, bounds.y + bounds.height).
        let sx = raster_bounds.width / f64::from(raster_meta.width);
        let sy = raster_bounds.height / f64::from(raster_meta.height);
        let doc_x = raster_bounds.x + f64::from(region.x) * sx;
        let doc_y = raster_bounds.y + f64::from(region.y) * sy;
        let doc_w = f64::from(region.width) * sx;
        let doc_h = f64::from(region.height) * sy;

        // Pick the font size — caller-supplied if present,
        // otherwise estimated from the region's height. The
        // estimate uses the average of sx/sy because line height
        // in document space is `region.height * sy`; falling back
        // to a fixed 16pt when the resulting estimate isn't finite.
        let font_size = req.font_size.unwrap_or_else(|| {
            let h_doc = (f64::from(region.height) * sy) as f32;
            let candidate = h_doc * FONT_SIZE_HEIGHT_RATIO;
            if candidate.is_finite() && candidate > 0.0 {
                candidate
            } else {
                16.0
            }
        });
        let font_family = req
            .font_family
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "Sans".to_string());

        let mut new_node = Node::new(NodeType::TextLayer, "Detected text");
        new_node.parent_id = parent;
        new_node.bounds = kcreate_core::node::Bounds {
            x: doc_x,
            y: doc_y,
            width: doc_w,
            height: doc_h,
        };
        let text_meta = kcreate_export::scene_metadata::TextLayerMeta {
            text: req.text.clone(),
            font_family,
            font_size,
        };
        new_node.metadata.insert(
            kcreate_export::scene_metadata::TEXT_LAYER_METADATA_KEY.to_string(),
            serde_json::to_value(&text_meta)?,
        );
        let new_id = ws.project.document.insert_node(new_node)?;

        let snapshot = ws
            .project
            .document
            .get_node(new_id)
            .map_or(serde_json::Value::Null, |n| {
                serde_json::to_value(n).unwrap_or(serde_json::Value::Null)
            });
        let op = Operation::new(
            "ai",
            "ai_insert_text_layer",
            serde_json::Value::Null,
            snapshot,
            vec![new_id, raster_node_id],
        )
        .as_ai_generated();
        ws.project.execute_operation(op);
        kcreate_ai::ActionLog::global()
            .lock()
            .append(kcreate_ai::AiAction {
                id: Uuid::new_v4(),
                timestamp: Utc::now(),
                task_type: "ocr_insert_text_layer".into(),
                model: "ocr-heuristic-v0".into(),
                compute_device: "cpu".into(),
                affected_nodes: vec![new_id, raster_node_id],
                confidence: None,
            });
        ws.project.modified_at = Utc::now();
        Ok(new_id)
    })?;
    sync_scene_after_change();
    Ok(new_id)
}

// -----------------------------------------------------------------------------
// AI inference: alt-text + layout-suggest (Phase 4 Block B)
// -----------------------------------------------------------------------------

/// Run the local alt-text heuristic against the raster layer
/// identified by `node_id` and return the resulting
/// [`kcreate_ai::AltTextReport`] as JSON.
///
/// This call is read-only — it does NOT write the generated label
/// onto the node. The renderer surfaces the result inline with an
/// "Apply" button that calls [`ai_apply_alt_text`] to persist it.
/// Splitting analysis from persistence lets the user reject a
/// generated description without polluting the document history
/// with an apply-then-undo operation.
pub fn ai_alt_text_for_node(node_id: Uuid) -> Result<String> {
    let encoded = with_workspace(|ws| {
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
            .get(crate::scene_sync::RASTER_IMAGE_METADATA_KEY)
            .ok_or_else(|| {
                DocumentBridgeError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "raster layer missing image metadata",
                ))
            })?;
        let meta: crate::scene_sync::RasterImageMeta = serde_json::from_value(meta_value.clone())?;
        blob_load(ws, &meta.blob_hash)
    })?;
    let img = image::load_from_memory(&encoded).map_err(|e| {
        DocumentBridgeError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    })?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    let mut report =
        kcreate_ai::generate_alt_text(rgba.as_raw(), w, h, kcreate_ai::AltTextOptions::default())
            .map_err(|e| {
            DocumentBridgeError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
        })?;
    // Task 14: if a VLM sidecar is ready, prefer its caption. The
    // heuristic statistics (brightness / contrast / palette / edge
    // density) stay accurate, so we keep them for the UI's chips —
    // we only swap out the `text` string for one written by the
    // VLM, which is far more semantically grounded ("portrait of a
    // woman wearing a red coat against a brick wall") than what
    // statistics alone can describe ("Bright photographic image
    // dominated by warm reds…"). On any VLM failure, the heuristic
    // text is kept verbatim — degraded gracefully, never errored.
    if let Ok(text) = crate::phase4::vision_generate_alt_text(rgba.as_raw(), w, h) {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            report.text = trimmed.to_string();
        }
    }
    Ok(serde_json::to_string(&report)?)
}

/// Persist an alt-text label onto a node. Records an operation in
/// the project's operation log so the change participates in undo
/// / redo and shows up in the action history.
///
/// An empty `text` clears the alt-text metadata key entirely
/// (matching [`kcreate_core::node::Node::set_alt_text`]'s
/// "empty == missing" semantic), so the user can revert to
/// "no alt text" without leaving a tombstone behind.
pub fn ai_apply_alt_text(node_id: Uuid, text: String) -> Result<()> {
    with_workspace_mut(|ws| {
        let before = ws
            .project
            .document
            .get_node(node_id)
            .map(|n| serde_json::to_value(n).unwrap_or(serde_json::Value::Null))
            .ok_or(DocumentBridgeError::NodeNotFound(node_id))?;
        let node = ws
            .project
            .document
            .get_node_mut(node_id)
            .ok_or(DocumentBridgeError::NodeNotFound(node_id))?;
        node.set_alt_text(&text);
        let after = ws
            .project
            .document
            .get_node(node_id)
            .map_or(serde_json::Value::Null, |n| {
                serde_json::to_value(n).unwrap_or(serde_json::Value::Null)
            });
        let op = Operation::new("ai", "ai_apply_alt_text", before, after, vec![node_id])
            .as_ai_generated();
        ws.project.execute_operation(op);
        kcreate_ai::ActionLog::global()
            .lock()
            .append(kcreate_ai::AiAction {
                id: Uuid::new_v4(),
                timestamp: Utc::now(),
                task_type: "alt_text_apply".into(),
                model: "alt-text-heuristic-v0".into(),
                compute_device: "cpu".into(),
                affected_nodes: vec![node_id],
                confidence: None,
            });
        ws.project.modified_at = Utc::now();
        Ok(())
    })?;
    sync_scene_after_change();
    Ok(())
}

/// Run the layout-suggest heuristic over the direct children of
/// the artboard / frame / page identified by `artboard_id` and
/// return the suggestions as JSON.
///
/// Only direct children with non-zero bounds are considered. The
/// caller's responsibility is to choose a parent node — passing a
/// leaf raster layer for example returns an empty suggestion list
/// rather than an error so the UI can render a "nothing to
/// suggest" state without special-casing the call.
///
/// Like [`ai_alt_text_for_node`], this is read-only — the renderer
/// previews suggestions before any apply step. A future Phase 4
/// follow-up will add `ai_apply_layout_suggestion` to actually
/// promote a suggestion into a real LayoutFrame.
pub fn ai_layout_suggest_for_artboard(artboard_id: Uuid) -> Result<String> {
    let nodes = with_workspace(|ws| {
        let parent = ws
            .project
            .document
            .get_node(artboard_id)
            .ok_or(DocumentBridgeError::NodeNotFound(artboard_id))?;
        let child_ids: Vec<Uuid> = parent.children.clone();
        let mut out: Vec<kcreate_ai::LayoutNode> = Vec::with_capacity(child_ids.len());
        for child_id in child_ids {
            // A node that vanished between the parent-fetch and
            // here would be a graph-integrity bug; surface it
            // rather than silently dropping the child.
            let child = ws
                .project
                .document
                .get_node(child_id)
                .ok_or(DocumentBridgeError::NodeNotFound(child_id))?;
            if !child.visible {
                continue;
            }
            // Compose world-space bounds the same way every other
            // layout-consuming surface in the bridge does (see
            // `scene_sync::node_world_bounds`): local `bounds` plus
            // the node's `transform.{tx,ty}`. `canvas_move_node` and
            // friends only mutate the transform — leaving local
            // `bounds` untouched — so reading `bounds` alone gives
            // the clustering algorithm pre-move positions and the
            // returned `LayoutSuggestion` bounds end up
            // visually-correct-looking but pointing at where the
            // user *used* to drag the layer, not where it is now.
            let b = child.bounds;
            if b.width <= 0.0 || b.height <= 0.0 {
                continue;
            }
            out.push(kcreate_ai::LayoutNode {
                id: child_id,
                bounds: kcreate_ai::LayoutBounds {
                    x: (b.x + child.transform.tx) as f32,
                    y: (b.y + child.transform.ty) as f32,
                    width: b.width as f32,
                    height: b.height as f32,
                },
            });
        }
        Ok::<_, DocumentBridgeError>(out)
    })?;
    // `suggest_layout_grouping` requires at least 2 nodes — return
    // an empty list rather than propagating the error so the UI
    // can render a clean "nothing to suggest" state.
    if nodes.len() < 2 {
        return Ok("[]".to_string());
    }
    let suggestions =
        kcreate_ai::suggest_layout_grouping(&nodes, kcreate_ai::LayoutSuggestOptions::default())
            .map_err(|e| {
                DocumentBridgeError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
            })?;
    kcreate_ai::ActionLog::global()
        .lock()
        .append(kcreate_ai::AiAction {
            id: Uuid::new_v4(),
            timestamp: Utc::now(),
            task_type: "layout_suggest".into(),
            model: "layout-heuristic-v0".into(),
            compute_device: "cpu".into(),
            affected_nodes: vec![artboard_id],
            confidence: None,
        });
    Ok(serde_json::to_string(&suggestions)?)
}

// -----------------------------------------------------------------------------
// Plugin sandbox
// -----------------------------------------------------------------------------

fn plugin_registry() -> &'static Mutex<kcreate_plugin::PluginRegistry> {
    static R: OnceLock<Mutex<kcreate_plugin::PluginRegistry>> = OnceLock::new();
    R.get_or_init(|| {
        Mutex::new(kcreate_plugin::PluginRegistry::with_trust(
            plugin_dir(),
            load_trust_store(),
        ))
    })
}

fn plugin_dir() -> PathBuf {
    if let Ok(env) = std::env::var("KCREATE_PLUGIN_DIR") {
        return PathBuf::from(env);
    }
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".kcreate").join("plugins")
}

/// Where the host stores the trusted plugin-signing public keys.
/// Sits alongside the plugin directory so a user can move both with
/// one `KCREATE_PLUGIN_DIR` override.
fn trust_store_path() -> PathBuf {
    plugin_dir().join("trusted_keys.json")
}

/// Load the trust store from `trusted_keys.json` if it exists, or
/// return an empty store otherwise. A malformed file is logged and
/// treated as empty — it must never prevent the bridge from starting,
/// since `plugin_list` (and the rest of the host) need to keep
/// working even for users who never set up the file.
fn load_trust_store() -> kcreate_plugin::TrustStore {
    let path = trust_store_path();
    if !path.exists() {
        return kcreate_plugin::TrustStore::default();
    }
    match kcreate_plugin::TrustStore::load_from_path(&path) {
        Ok(store) => store,
        Err(e) => {
            log::warn!(
                "kcreate_bridge: trust store at {} could not be loaded ({e}); proceeding with no trusted keys",
                path.display(),
            );
            kcreate_plugin::TrustStore::default()
        }
    }
}

fn plugin_runtime() -> &'static kcreate_plugin::WasmPluginRuntime {
    static RT: OnceLock<kcreate_plugin::WasmPluginRuntime> = OnceLock::new();
    RT.get_or_init(kcreate_plugin::WasmPluginRuntime::new)
}

/// Re-seed the plugin registry from the current `plugin_dir()` so
/// per-test directories take effect. The static `OnceLock` only
/// initializes once per process; without this helper, the second
/// `#[serial]` test that sets `KCREATE_PLUGIN_DIR` to a fresh path
/// would silently keep scanning the first test's (now-dropped) temp
/// directory. Only compiled in test builds.
#[cfg(test)]
pub(crate) fn reset_plugin_state_for_tests() {
    let mut reg = plugin_registry().lock();
    *reg = kcreate_plugin::PluginRegistry::with_trust(plugin_dir(), load_trust_store());
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginListEntry {
    #[serde(flatten)]
    pub manifest: kcreate_plugin::PluginManifest,
    pub enabled: bool,
    /// Outcome of the last signature-sidecar verification for this
    /// plugin. Always present; `unsigned` for plugins that ship
    /// without `manifest.json.sig`.
    pub signature: kcreate_plugin::SignatureStatus,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustedKeyInfo {
    pub key_id: String,
    pub comment: String,
}

pub fn plugin_list() -> Result<Vec<PluginListEntry>> {
    let mut reg = plugin_registry().lock();
    reg.scan()
        .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))?;
    Ok(reg
        .list()
        .into_iter()
        .map(|m| PluginListEntry {
            enabled: reg.is_enabled(&m.id),
            signature: reg
                .signature_status_for(&m.id)
                .cloned()
                .unwrap_or(kcreate_plugin::SignatureStatus::Unsigned),
            manifest: m.clone(),
        })
        .collect())
}

/// Snapshot of every trusted Ed25519 public key the host knows about.
/// Surfaced to the UI's "Trusted Authorities" list so users can see
/// who is allowed to sign plugins they install. Order is unspecified
/// (the trust store is a `HashMap` under the hood); the UI sorts
/// alphabetically by `keyId`.
pub fn plugin_trust_list() -> Result<Vec<TrustedKeyInfo>> {
    let reg = plugin_registry().lock();
    Ok(reg
        .trust_store()
        .entries()
        .map(|(id, comment)| TrustedKeyInfo {
            key_id: id.to_string(),
            comment: comment.to_string(),
        })
        .collect())
}

/// Re-read `trusted_keys.json` and rescan plugins so previously-
/// rejected native plugins (or `Invalid`-status sandboxed plugins)
/// get a second chance once the user adds the missing key. Exposed
/// as a bridge call so the UI can offer a "Reload trusted keys"
/// button without restarting the host.
pub fn plugin_trust_reload() -> Result<()> {
    let new_store = load_trust_store();
    let mut reg = plugin_registry().lock();
    reg.set_trust_store(new_store);
    reg.scan()
        .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))?;
    Ok(())
}

pub fn plugin_enable(id: &str) -> Result<()> {
    let mut reg = plugin_registry().lock();
    reg.enable(id)
        .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))
}

pub fn plugin_disable(id: &str) -> Result<()> {
    let mut reg = plugin_registry().lock();
    reg.disable(id)
        .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))
}

pub fn plugin_execute(id: &str, function: &str, input_json: &str) -> Result<String> {
    // Resolve existence and enabled state under a single registry
    // lock acquisition so the two are observed atomically, then drop
    // the lock before any potentially slow WASM compile / execute.
    let (entry, enabled) = {
        let reg = plugin_registry().lock();
        (reg.entry_point_for(id), reg.is_enabled(id))
    };
    // Existence check *before* enabled check. `PluginRegistry::is_enabled`
    // returns `false` for unknown ids (`HashMap::get(...).unwrap_or(false)`),
    // so checking enabled first would surface "not enabled" for plugins
    // that don't exist at all — leading callers to look for a disabled
    // plugin instead of a typo / missing install. Per Devin Review
    // BUG_pr-review-job-790e7860e5c745e0bee13295709290f4_0001.
    let path = entry.ok_or_else(|| {
        DocumentBridgeError::Io(std::io::Error::other(format!("plugin {id} not found")))
    })?;
    if !enabled {
        return Err(DocumentBridgeError::Io(std::io::Error::other(format!(
            "plugin {id} is not enabled"
        ))));
    }
    // `execute_path` keeps a `(path, mtime)`-keyed compiled-`Module`
    // cache inside the runtime, so the steady-state hot path skips both
    // the disk read and the wasmi compile. A rebuilt `.wasm` file is
    // picked up automatically the next call because mtime moves.
    let rt = plugin_runtime();
    let out = rt
        .execute_path(&path, function, input_json, 64)
        .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))?;
    Ok(serde_json::to_string(&serde_json::json!({
        "output": out.output,
        "logs": out.logs,
    }))?)
}

/// Outcome of validating + applying a single proposal. Returned to
/// the renderer so the user can see which proposals took effect.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum ProposalOutcome {
    /// Proposal validated and was applied as an operation. `node_id`
    /// is the affected (or newly-created) node.
    Applied { node_id: Uuid },
    /// Proposal failed validation. `reason` explains why; the
    /// document is unchanged.
    Rejected { reason: String },
}

/// Plugin proposal pre-application, paired with its outcome. The
/// renderer uses this to render a per-proposal status line.
#[derive(Debug, Clone, Serialize)]
pub struct ProposalReport {
    #[serde(flatten)]
    pub proposal: kcreate_plugin::ProposedMutation,
    pub outcome: ProposalOutcome,
}

/// Execute a plugin under the Phase 2 extended host ABI (Block C
/// Task 15).
///
/// Pipeline:
///
/// 1. Resolve the plugin entry point + manifest permissions under
///    one registry lock.
/// 2. Snapshot the document as JSON for `kcreate_read_document`
///    queries.
/// 3. Build an asset loader closure that re-enters
///    [`with_workspace`] per blob read so concurrent edits can
///    still proceed while the plugin runs.
/// 4. Execute the plugin under [`kcreate_plugin::PluginContext`].
/// 5. After the plugin returns, validate each proposal — checking
///    node-id resolution and parent-id resolution — and apply
///    accepted ones via [`crate::document::document_create_node`] /
///    [`crate::document::document_update_node`] /
///    [`crate::document::document_delete_node`] so they record
///    operations and become undoable.
/// 6. Return a JSON envelope:
///    `{ "output": "...", "logs": [...], "proposals": [ ProposalReport, ... ] }`.
///
/// The basic ABI used by [`plugin_execute`] still works because the
/// runtime simply skips the extended host functions when no
/// `PluginContext` is supplied. Plugins that don't request any of
/// `read_document` / `read_assets` / `write_document` in their
/// manifest get an empty permission set here and behave identically
/// to the legacy path.
pub fn plugin_execute_with_context(id: &str, function: &str, input_json: &str) -> Result<String> {
    let (entry, manifest, enabled) = {
        let reg = plugin_registry().lock();
        (
            reg.entry_point_for(id),
            reg.list().iter().find(|m| m.id == id).map(|m| (*m).clone()),
            reg.is_enabled(id),
        )
    };
    let path = entry.ok_or_else(|| {
        DocumentBridgeError::Io(std::io::Error::other(format!("plugin {id} not found")))
    })?;
    let manifest = manifest.ok_or_else(|| {
        // Defensive: `entry_point_for` returning `Some` while
        // `list()` returns no matching manifest would be a registry
        // bug, not a user-facing one — but we surface it cleanly
        // rather than panicking inside the bridge.
        DocumentBridgeError::Io(std::io::Error::other(format!(
            "plugin {id} manifest missing"
        )))
    })?;
    if !enabled {
        return Err(DocumentBridgeError::Io(std::io::Error::other(format!(
            "plugin {id} is not enabled"
        ))));
    }

    // Snapshot the document for `kcreate_read_document`. We use the
    // same shape as `document_serialise_for_ai` — a `{"project",
    // "nodes":[...]}` envelope — so plugins see node properties
    // exactly as the LLM-prompt path does.
    let snapshot_json = crate::document::document_serialise_for_ai()?;
    let snapshot: serde_json::Value = serde_json::from_str(&snapshot_json)?;

    // Permissions from the manifest. The runtime denies any
    // intrinsic that doesn't have a matching grant, so plugins that
    // didn't declare a permission can't reach the gated functions.
    let permissions: std::collections::HashSet<kcreate_plugin::PluginPermission> =
        manifest.permissions.iter().copied().collect();

    // Asset loader: each call re-acquires the workspace under
    // `with_workspace` so we don't hold the lock during plugin
    // execution. Errors become `None` from the plugin's perspective
    // (matches `kcreate_read_asset`'s "asset not found" semantics).
    let asset_loader: kcreate_plugin::AssetLoader = std::sync::Arc::new(|hash: &str| {
        with_workspace(|ws| Ok(crate::document::blob_load(ws, hash)))
            .ok()
            .and_then(std::result::Result::ok)
    });

    let context = kcreate_plugin::PluginContext {
        plugin_id: id.to_string(),
        document_snapshot: snapshot,
        asset_loader,
        permissions,
        proposals: Vec::new(),
    };

    let rt = plugin_runtime();
    let out = rt
        .execute_path_with_context(&path, function, input_json, 64, context)
        .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))?;

    let reports = apply_plugin_proposals(out.proposals);

    Ok(serde_json::to_string(&serde_json::json!({
        "output": out.output,
        "logs": out.logs,
        "proposals": reports,
    }))?)
}

/// List installed JS panel plugins along with their panel configs.
///
/// Used by the Electron main process: on startup (and after every
/// `plugin_list` rescan) it queries this to decide which sandboxed
/// `BrowserView` instances to mount and where. WASM and native
/// plugins are filtered out — the host needs them for execution but
/// not for panel allocation.
pub fn plugin_js_list() -> Result<Vec<kcreate_plugin::JsPanelInfo>> {
    let reg = plugin_registry().lock();
    Ok(reg
        .list()
        .into_iter()
        .filter_map(|m| {
            if m.plugin_type != kcreate_plugin::PluginType::JsPanel {
                return None;
            }
            let cfg = m.js_panel.clone()?;
            Some(kcreate_plugin::JsPanelInfo {
                id: m.id.clone(),
                name: m.name.clone(),
                version: m.version.clone(),
                config: cfg,
                enabled: reg.is_enabled(&m.id),
            })
        })
        .collect())
}

/// Validate a single message ferried from a JS panel and (when the
/// message is a mutating one) apply its side effects.
///
/// The Electron host (`apps/desktop/main/src/main.ts`) is the gate:
/// when a panel calls `window.kcreatePlugin.sendMessage(type, payload)`,
/// the main process forwards `(plugin_id, message)` into this
/// function before the panel sees any response. The bridge:
///
/// 1. Looks the plugin up; refuses if missing, not a JS panel, or
///    disabled.
/// 2. Checks the panel's declared permissions against the message
///    type. A panel that didn't declare `read_document` cannot send
///    a `read_document` message and will get a `Denied { permission }`
///    outcome back.
/// 3. For `read_document`, resolves the query against a fresh
///    snapshot.
/// 4. For `write_proposal`, validates and applies the proposal as a
///    recorded operation (same code path as the WASM plugin proposal
///    apply).
/// 5. For `log`, attaches the message to the host log buffer (the
///    Electron host can forward it to the dev console).
pub fn plugin_js_message(plugin_id: &str, message_json: &str) -> Result<String> {
    let outcome = plugin_js_message_inner(plugin_id, message_json);
    Ok(serde_json::to_string(&outcome)?)
}

fn plugin_js_message_inner(
    plugin_id: &str,
    message_json: &str,
) -> kcreate_plugin::JsPanelMessageOutcome {
    // 1. Resolve manifest + enabled status under a single lock.
    let (manifest, enabled) = {
        let reg = plugin_registry().lock();
        (
            reg.list()
                .iter()
                .find(|m| m.id == plugin_id)
                .map(|m| (*m).clone()),
            reg.is_enabled(plugin_id),
        )
    };
    let manifest = match manifest {
        Some(m) => m,
        None => {
            return kcreate_plugin::JsPanelMessageOutcome::Invalid {
                reason: format!("plugin {plugin_id} not found"),
            }
        }
    };
    if manifest.plugin_type != kcreate_plugin::PluginType::JsPanel {
        return kcreate_plugin::JsPanelMessageOutcome::Invalid {
            reason: format!("plugin {plugin_id} is not a js_panel plugin"),
        };
    }
    if !enabled {
        return kcreate_plugin::JsPanelMessageOutcome::Invalid {
            reason: format!("plugin {plugin_id} is not enabled"),
        };
    }
    let cfg = match manifest.js_panel.as_ref() {
        Some(c) => c.clone(),
        None => {
            return kcreate_plugin::JsPanelMessageOutcome::Invalid {
                reason: format!("plugin {plugin_id} missing js_panel config"),
            }
        }
    };

    // 2. Parse the message.
    let msg: kcreate_plugin::JsPanelMessage = match serde_json::from_str(message_json) {
        Ok(m) => m,
        Err(e) => {
            return kcreate_plugin::JsPanelMessageOutcome::Invalid {
                reason: format!("malformed message: {e}"),
            }
        }
    };

    // 3. Dispatch with permission gating.
    match msg {
        kcreate_plugin::JsPanelMessage::ReadDocument { query } => {
            if !cfg.has(kcreate_plugin::PluginPermission::ReadDocument) {
                log::warn!(
                    "kcreate.plugin.js[{plugin_id}]: read_document denied (missing ReadDocument)"
                );
                return kcreate_plugin::JsPanelMessageOutcome::Denied {
                    permission: kcreate_plugin::PluginPermission::ReadDocument,
                };
            }
            // Parse the query against the same DocumentQuery enum the
            // WASM ABI uses so JS panels and WASM plugins speak the
            // same dialect.
            let parsed: kcreate_plugin::DocumentQuery = match serde_json::from_value(query) {
                Ok(q) => q,
                Err(e) => {
                    return kcreate_plugin::JsPanelMessageOutcome::Invalid {
                        reason: format!("invalid query: {e}"),
                    }
                }
            };
            let snapshot_json = match crate::document::document_serialise_for_ai() {
                Ok(s) => s,
                Err(e) => {
                    return kcreate_plugin::JsPanelMessageOutcome::Invalid {
                        reason: format!("snapshot failed: {e}"),
                    }
                }
            };
            let snapshot: serde_json::Value = match serde_json::from_str(&snapshot_json) {
                Ok(v) => v,
                Err(e) => {
                    return kcreate_plugin::JsPanelMessageOutcome::Invalid {
                        reason: format!("snapshot parse failed: {e}"),
                    }
                }
            };
            let result = kcreate_plugin::resolve_document_query(&snapshot, &parsed);
            kcreate_plugin::JsPanelMessageOutcome::Ok { result }
        }
        kcreate_plugin::JsPanelMessage::WriteProposal { proposal } => {
            if !cfg.has(kcreate_plugin::PluginPermission::WriteDocument) {
                log::warn!(
                    "kcreate.plugin.js[{plugin_id}]: write_proposal denied (missing WriteDocument)"
                );
                return kcreate_plugin::JsPanelMessageOutcome::Denied {
                    permission: kcreate_plugin::PluginPermission::WriteDocument,
                };
            }
            let mutation: kcreate_plugin::ProposedMutation = match serde_json::from_value(proposal)
            {
                Ok(m) => m,
                Err(e) => {
                    return kcreate_plugin::JsPanelMessageOutcome::Invalid {
                        reason: format!("invalid proposal: {e}"),
                    }
                }
            };
            // Single-proposal apply, same code path WASM plugins go
            // through. Report flows back as the outcome's result.
            let reports = apply_plugin_proposals(vec![mutation]);
            let report = reports
                .into_iter()
                .next()
                .expect("one proposal in, one report out");
            let value = serde_json::to_value(&report).unwrap_or(serde_json::Value::Null);
            kcreate_plugin::JsPanelMessageOutcome::Ok { result: value }
        }
        kcreate_plugin::JsPanelMessage::Log { message } => {
            log::info!("kcreate.plugin.js[{plugin_id}]: {message}");
            kcreate_plugin::JsPanelMessageOutcome::Ok {
                result: serde_json::Value::Null,
            }
        }
    }
}

/// Validate and apply a batch of plugin proposals.
///
/// Per-proposal contract:
/// * `CreateNode` — parent must resolve to an existing node (or be
///   the root by leaving `parent_id` resolved to `None` after the
///   lookup); `node_type` must be one of the strings accepted by
///   [`crate::document::document_create_node`].
/// * `UpdateNode` — `node_id` must resolve.
/// * `DeleteNode` — `node_id` must resolve.
///
/// Each accepted proposal is applied through the existing
/// `document_*` helpers so it records an operation and is undoable.
/// Rejected proposals leave the document unchanged.
fn apply_plugin_proposals(proposals: Vec<kcreate_plugin::ProposedMutation>) -> Vec<ProposalReport> {
    proposals
        .into_iter()
        .map(|p| {
            let outcome = apply_one_proposal(&p);
            ProposalReport {
                proposal: p,
                outcome,
            }
        })
        .collect()
}

fn apply_one_proposal(proposal: &kcreate_plugin::ProposedMutation) -> ProposalOutcome {
    match proposal {
        kcreate_plugin::ProposedMutation::CreateNode {
            parent_id,
            node_type,
            props,
        } => {
            // Validate parent existence up front so we return a
            // clean rejection rather than letting `document_create_node`
            // fail later with a less specific error.
            let parent_check =
                with_workspace(|ws| Ok(ws.project.document.get_node(*parent_id).is_some()));
            match parent_check {
                Ok(true) => {}
                Ok(false) => {
                    return ProposalOutcome::Rejected {
                        reason: format!("parent node {parent_id} not found"),
                    }
                }
                Err(e) => {
                    return ProposalOutcome::Rejected {
                        reason: format!("workspace unavailable: {e}"),
                    }
                }
            }
            let create_props: crate::document::CreateNodeProps =
                match serde_json::from_value(props.clone()) {
                    Ok(p) => p,
                    Err(e) => {
                        return ProposalOutcome::Rejected {
                            reason: format!("invalid create props: {e}"),
                        }
                    }
                };
            match crate::document::document_create_node(node_type, Some(*parent_id), &create_props)
            {
                Ok(new_id) => ProposalOutcome::Applied { node_id: new_id },
                Err(e) => ProposalOutcome::Rejected {
                    reason: format!("create failed: {e}"),
                },
            }
        }
        kcreate_plugin::ProposedMutation::UpdateNode { node_id, changes } => {
            let exists = with_workspace(|ws| Ok(ws.project.document.get_node(*node_id).is_some()));
            match exists {
                Ok(true) => {}
                Ok(false) => {
                    return ProposalOutcome::Rejected {
                        reason: format!("node {node_id} not found"),
                    }
                }
                Err(e) => {
                    return ProposalOutcome::Rejected {
                        reason: format!("workspace unavailable: {e}"),
                    }
                }
            }
            let update_props: crate::document::UpdateNodeProps =
                match serde_json::from_value(changes.clone()) {
                    Ok(p) => p,
                    Err(e) => {
                        return ProposalOutcome::Rejected {
                            reason: format!("invalid update changes: {e}"),
                        }
                    }
                };
            match crate::document::document_update_node(*node_id, &update_props) {
                Ok(()) => ProposalOutcome::Applied { node_id: *node_id },
                Err(e) => ProposalOutcome::Rejected {
                    reason: format!("update failed: {e}"),
                },
            }
        }
        kcreate_plugin::ProposedMutation::DeleteNode { node_id } => {
            let exists = with_workspace(|ws| Ok(ws.project.document.get_node(*node_id).is_some()));
            match exists {
                Ok(true) => {}
                Ok(false) => {
                    return ProposalOutcome::Rejected {
                        reason: format!("node {node_id} not found"),
                    }
                }
                Err(e) => {
                    return ProposalOutcome::Rejected {
                        reason: format!("workspace unavailable: {e}"),
                    }
                }
            }
            match crate::document::document_delete_node(*node_id) {
                Ok(()) => ProposalOutcome::Applied { node_id: *node_id },
                Err(e) => ProposalOutcome::Rejected {
                    reason: format!("delete failed: {e}"),
                },
            }
        }
    }
}

// -----------------------------------------------------------------------------
// MCP permissions
// -----------------------------------------------------------------------------

#[cfg(feature = "mcp")]
fn mcp_permission_store() -> &'static kcreate_mcp::McpPermissionStore {
    static S: OnceLock<kcreate_mcp::McpPermissionStore> = OnceLock::new();
    S.get_or_init(|| {
        let dir = mcp_permission_dir();
        // `open_recoverable` quarantines a corrupt mcp_permissions.json
        // and starts empty instead of panicking, so a partially-flushed
        // or hand-edited file does not crash the Electron main process
        // on the first MCP permission operation. An `Err` here only
        // surfaces a hard I/O failure (e.g. dir not writable), which we
        // do still treat as fatal — there is no sensible recovery for
        // "cannot create the permissions directory at all".
        kcreate_mcp::McpPermissionStore::open_recoverable(&dir)
            .expect("kcreate_bridge: MCP permission directory not writable")
    })
}

#[cfg(feature = "mcp")]
fn mcp_permission_dir() -> PathBuf {
    if let Ok(env) = std::env::var("KCREATE_MCP_DIR") {
        return PathBuf::from(env);
    }
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".kcreate")
}

#[cfg(feature = "mcp")]
pub fn mcp_permission_list() -> Result<String> {
    let list = mcp_permission_store().list();
    Ok(serde_json::to_string(&list)?)
}

#[cfg(not(feature = "mcp"))]
pub fn mcp_permission_list() -> Result<String> {
    Ok("[]".to_string())
}

#[cfg(feature = "mcp")]
pub fn mcp_permission_grant(client_id: &str, tool_name: &str, grant: &str) -> Result<()> {
    let grant_kind = parse_grant(grant)?;
    mcp_permission_store()
        .grant(client_id, tool_name, grant_kind)
        .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))
}

#[cfg(not(feature = "mcp"))]
pub fn mcp_permission_grant(_client_id: &str, _tool_name: &str, _grant: &str) -> Result<()> {
    Err(DocumentBridgeError::Io(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "MCP feature disabled at compile time",
    )))
}

#[cfg(feature = "mcp")]
pub fn mcp_permission_revoke(client_id: &str, tool_name: &str) -> Result<()> {
    mcp_permission_store()
        .revoke(client_id, tool_name)
        .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))
}

#[cfg(not(feature = "mcp"))]
pub fn mcp_permission_revoke(_client_id: &str, _tool_name: &str) -> Result<()> {
    Ok(())
}

#[cfg(feature = "mcp")]
fn parse_grant(s: &str) -> Result<kcreate_mcp::PermissionGrant> {
    match s {
        "once" => Ok(kcreate_mcp::PermissionGrant::Once),
        "always" => Ok(kcreate_mcp::PermissionGrant::Always),
        "denied" => Ok(kcreate_mcp::PermissionGrant::Denied),
        other => Err(DocumentBridgeError::InvalidArgument {
            argument: "grant".into(),
            value: other.to_string(),
        }),
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpStatus {
    pub running: bool,
    pub port: u32,
}

pub fn mcp_status() -> McpStatus {
    // Single-shot lock acquisition via `mcp_state`. Composing
    // `mcp_is_running` + `mcp_port` separately was a TOCTOU — the
    // server could be stopped between the two calls and produce a
    // status with `running: true` and `port: 0` that no caller
    // expects to be valid. Per Devin Review
    // ANALYSIS_pr-review-job-790e7860e5c745e0bee13295709290f4_0001.
    let (running, port) = crate::document::mcp_state();
    McpStatus {
        running,
        port: port.unwrap_or(0),
    }
}

// -----------------------------------------------------------------------------
// Color management (Phase 2)
// -----------------------------------------------------------------------------

/// Read the project's [`ColorSettings`] as JSON. Returns the
/// `Default` (sRGB, no CMYK profile, perceptual intent) when no
/// project is currently loaded, so the renderer/UI can always render
/// the panel without crashing.
///
/// The fall-back is part of the public contract — the
/// `ColorSettingsPanel` mounts at app start, before any project is
/// opened, and needs a stable JSON shape to populate its controls.
pub fn color_settings_get() -> Result<String> {
    use kcreate_core::color::ColorSettings;
    let settings = match with_workspace(|ws| Ok(ws.project.color_settings.clone())) {
        Ok(settings) => settings,
        // Match the docstring contract: no project ⇒ default settings,
        // not an error. The panel renders identically whether or not a
        // project is loaded, and `color_settings_update` will refuse
        // the write (via `with_workspace_mut`) until one is.
        Err(DocumentBridgeError::NoProject) => ColorSettings::default(),
        Err(err) => return Err(err),
    };
    Ok(serde_json::to_string(&settings)?)
}

/// Replace the project's [`ColorSettings`] with the supplied JSON
/// blob and record an operation in the project's log.
///
/// The operation `command` is `"color_settings_update"`; `before_patch`
/// is the previous settings JSON, `after_patch` is the new one, and
/// `affected_nodes` is empty because color settings are document-wide
/// (no specific node is mutated).
///
/// **Undo contract.** Undo is real: `document_undo` deserialises
/// `before_patch` and writes it back into `ws.project.color_settings`
/// before returning, so after one undo the in-memory settings match
/// the pre-update value (and `color_settings_get` returns the
/// previous shape). Redo is symmetric via `after_patch`. The
/// dispatch lives in `crate::document::apply_inverse_patch` /
/// `apply_forward_patch` so the bridge owns the workspace-state
/// reversal — the renderer just calls `window.kcreate.document.undo()`
/// and refreshes; no command-specific knowledge required on the
/// host side.
pub fn color_settings_update(settings_json: &str) -> Result<()> {
    use kcreate_core::color::ColorSettings;
    let new_settings: ColorSettings = serde_json::from_str(settings_json)?;
    with_workspace_mut(|ws| {
        let before = serde_json::to_value(&ws.project.color_settings)?;
        let after = serde_json::to_value(&new_settings)?;
        ws.project.color_settings = new_settings;
        let op = Operation::new(
            "user",
            "color_settings_update",
            before,
            after,
            Vec::<Uuid>::new(),
        );
        ws.project.execute_operation(op);
        Ok(())
    })?;
    sync_scene_after_change();
    Ok(())
}

/// Convert a single color value between color spaces. `from_json`
/// must deserialize into a [`kcreate_core::color::Color`]; `to_space`
/// is one of `"srgb"`, `"cmyk"`, `"lab"`, `"hsl"`. The result is
/// serialized back to JSON.
///
/// This is a pure utility (no workspace lock needed) so the color
/// picker can preview conversions in real time even when no project
/// is open.
pub fn color_convert(from_json: &str, to_space: &str) -> Result<String> {
    use kcreate_core::color::{srgb_to_cmyk, srgb_to_hsl, srgb_to_lab, Color};
    let from: Color = serde_json::from_str(from_json)?;
    // `Color::to_srgb` is the canonical entry into the sRGB connection
    // space for every variant; the per-space helpers below only need
    // the sRGB triplet plus the alpha that came with the source.
    let (r, g, b, a) = from.to_srgb();
    // Identity short-circuits for every variant. Round-tripping
    // through the sRGB connection space is *lossy*:
    //
    // * CMYK → CMYK loses K-channel information (CSS Color Module
    //   Level 4 §13).
    // * Lab → Lab loses out-of-gamut values because
    //   `xyz_d65_to_srgb` clamps each channel to `[0.0, 1.0]`.
    // * HSL → HSL is technically lossless for in-gamut sRGB, but the
    //   atan2-style hue extraction in `srgb_to_hsl` introduces tiny
    //   rounding drift on every round-trip that accumulates if the
    //   color picker re-converts on every keystroke.
    //
    // Returning the input unchanged when the target matches its native
    // space keeps `color_convert` idempotent and matches the
    // `color_convert_preserves_authored_*` test contract.
    let converted = match to_space {
        "srgb" => match &from {
            Color::Srgb {
                r: rr,
                g: gg,
                b: bb,
                a: aa,
            } => Color::Srgb {
                r: *rr,
                g: *gg,
                b: *bb,
                a: *aa,
            },
            _ => Color::Srgb { r, g, b, a },
        },
        "cmyk" => match &from {
            Color::Cmyk { c, m, y, k, a } => Color::Cmyk {
                c: *c,
                m: *m,
                y: *y,
                k: *k,
                a: *a,
            },
            _ => {
                let (c, m, y, k) = srgb_to_cmyk(r, g, b);
                Color::Cmyk { c, m, y, k, a }
            }
        },
        "lab" => match &from {
            Color::Lab {
                l,
                a_star,
                b_star,
                alpha,
            } => Color::Lab {
                l: *l,
                a_star: *a_star,
                b_star: *b_star,
                alpha: *alpha,
            },
            _ => {
                let (l, a_star, b_star) = srgb_to_lab(r, g, b);
                Color::Lab {
                    l,
                    a_star,
                    b_star,
                    alpha: a,
                }
            }
        },
        "hsl" => match &from {
            Color::Hsl { h, s, l, a: aa } => Color::Hsl {
                h: *h,
                s: *s,
                l: *l,
                a: *aa,
            },
            _ => {
                let (h, s, l) = srgb_to_hsl(r, g, b);
                Color::Hsl { h, s, l, a }
            }
        },
        other => {
            return Err(DocumentBridgeError::InvalidArgument {
                argument: "to_space".into(),
                value: other.to_string(),
            });
        }
    };
    Ok(serde_json::to_string(&converted)?)
}

// -----------------------------------------------------------------------------
// Spot color library (Phase 5, Block D Task 23)
// -----------------------------------------------------------------------------

/// Wire shape for spot CRUD. Mirrors `SpotColorDef` 1:1 plus the
/// `name` lookup key.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpotColorWire {
    pub name: String,
    pub display_name: String,
    pub fallback_cmyk: (f32, f32, f32, f32),
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub library_reference: Option<String>,
}

/// Insert or replace a spot colour in the project's
/// `SpotColorLibrary`. Records a `spot_color_upsert` operation on
/// the project log so undo reverses the change.
pub fn color_spot_upsert(wire_json: &str) -> Result<()> {
    use kcreate_core::color::SpotColorDef;
    let wire: SpotColorWire = serde_json::from_str(wire_json)?;
    with_workspace_mut(|ws| {
        let before = serde_json::to_value(&ws.project.spot_color_library)?;
        let def = SpotColorDef {
            display_name: wire.display_name.clone(),
            fallback_cmyk: wire.fallback_cmyk,
            library_reference: wire.library_reference.clone(),
        };
        ws.project.spot_color_library.insert(wire.name.clone(), def);
        let after = serde_json::to_value(&ws.project.spot_color_library)?;
        let op = Operation::new(
            "user",
            "spot_color_upsert",
            before,
            after,
            Vec::<Uuid>::new(),
        );
        ws.project.execute_operation(op);
        Ok(())
    })?;
    sync_scene_after_change();
    Ok(())
}

/// Remove a spot colour by name. Returns `false` (and records no
/// operation) when the name was not in the library — this matches
/// `BTreeMap::remove` semantics and lets the renderer round-trip the
/// "delete then re-add" affordance without a second IPC.
pub fn color_spot_remove(name: &str) -> Result<bool> {
    let mut removed = false;
    with_workspace_mut(|ws| {
        let before = serde_json::to_value(&ws.project.spot_color_library)?;
        removed = ws.project.spot_color_library.entries.remove(name).is_some();
        if removed {
            let after = serde_json::to_value(&ws.project.spot_color_library)?;
            let op = Operation::new(
                "user",
                "spot_color_remove",
                before,
                after,
                Vec::<Uuid>::new(),
            );
            ws.project.execute_operation(op);
        }
        Ok(())
    })?;
    if removed {
        sync_scene_after_change();
    }
    Ok(removed)
}

/// Report of a `color_spot_load_catalog` call.
///
/// Mirrors `SpotCatalogLoadReportWire` in `apps/desktop/shared/scene.ts`.
///
/// The four numeric counters satisfy
/// `raw_entries == parsed + duplicates_in_catalog + malformed`, so the
/// renderer can present a faithful breakdown of why a load dropped
/// entries (malformed CMYK arrays, same-id collisions within the
/// catalogue, etc.) instead of silently showing only the dedup'd
/// `parsed` count. `added` and `overwritten` are *project-level*
/// counts on top of that: they describe how the parsed library merged
/// into the existing `SpotColorLibrary`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpotCatalogLoadReport {
    /// Total entries in the catalogue file before any
    /// validation/dedup, mirroring
    /// `kcreate_core::color::CatalogParseStats::raw_entries`.
    /// `parsed + duplicates_in_catalog + malformed` always equals
    /// this value (Devin Review ANALYSIS_0005 on PR #16).
    pub raw_entries: usize,
    /// Number of entries that survived parsing and dedup and were
    /// merged into the project library.
    pub parsed: usize,
    /// Number of entries dropped because they collided with another
    /// entry of the same `name`/`id` *within the same catalogue*
    /// (last-write-wins). Always `0` for the bare-map shape because
    /// JSON object keys are unique at the parser level.
    pub duplicates_in_catalog: usize,
    /// Number of entries dropped as malformed (wrong-length CMYK,
    /// non-finite values, missing `id` in the wrapped form, etc.).
    pub malformed: usize,
    /// Number of swatches in the parsed catalogue that were not
    /// previously in the project library (newly inserted).
    pub added: usize,
    /// Number of swatches that overwrote an existing entry with the
    /// same `name` (the merge policy is "last-loaded wins").
    pub overwritten: usize,
}

/// Load a Pantone-style JSON catalogue and merge its entries into
/// the project's `SpotColorLibrary`. Returns a structured report of
/// added vs overwritten swatches.
///
/// The merge is recorded as a single undoable
/// `spot_color_load_catalog` operation so users can undo a bulk
/// import in one keystroke (vs once per swatch when they would have
/// added them with `color_spot_upsert`).
///
/// `raw_json` is the file contents as a UTF-8 string; the renderer
/// is responsible for reading the file off disk because the bridge
/// doesn't have native-file-dialog access on its own.
pub fn color_spot_load_catalog(raw_json: &str) -> Result<SpotCatalogLoadReport> {
    use kcreate_core::color::SpotColorLibrary;
    let (parsed, stats) =
        SpotColorLibrary::from_json_catalog_with_report(raw_json).map_err(|e| {
            DocumentBridgeError::InvalidArgument {
                argument: "spot_catalog".into(),
                value: e.to_string(),
            }
        })?;
    let parsed_count = stats.parsed;
    let mut report = SpotCatalogLoadReport {
        raw_entries: stats.raw_entries,
        parsed: parsed_count,
        duplicates_in_catalog: stats.duplicates_in_catalog,
        malformed: stats.malformed,
        added: 0,
        overwritten: 0,
    };
    if parsed_count == 0 {
        return Ok(report);
    }
    with_workspace_mut(|ws| {
        let before = serde_json::to_value(&ws.project.spot_color_library)?;
        for (name, def) in parsed.iter() {
            if ws.project.spot_color_library.get(name).is_some() {
                report.overwritten += 1;
            } else {
                report.added += 1;
            }
            ws.project
                .spot_color_library
                .insert(name.clone(), def.clone());
        }
        let after = serde_json::to_value(&ws.project.spot_color_library)?;
        let op = Operation::new(
            "user",
            "spot_color_load_catalog",
            before,
            after,
            Vec::<Uuid>::new(),
        );
        ws.project.execute_operation(op);
        Ok(())
    })?;
    sync_scene_after_change();
    Ok(report)
}

/// Enumerate the spot library as a JSON array of [`SpotColorWire`].
pub fn color_spot_list() -> Result<String> {
    let entries = with_workspace(|ws| {
        Ok(ws
            .project
            .spot_color_library
            .iter()
            .map(|(name, def)| SpotColorWire {
                name: name.clone(),
                display_name: def.display_name.clone(),
                fallback_cmyk: def.fallback_cmyk,
                library_reference: def.library_reference.clone(),
            })
            .collect::<Vec<_>>())
    })
    .unwrap_or_default();
    Ok(serde_json::to_string(&entries)?)
}

// -----------------------------------------------------------------------------
// Text frame + OpenType bridge (Phase 2, Block B Task 11)
// -----------------------------------------------------------------------------

/// JSON describing the precomputed paragraph layout for a text node.
/// Returned by [`text_layout_compute`] so the inspector / debug view
/// can render line outlines and column boundaries without re-running
/// the layout engine in TypeScript.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextLayoutLineWire {
    pub origin_x: f64,
    pub baseline_y: f64,
    pub width: f64,
    pub column: u32,
    pub glyph_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextLayoutWire {
    pub lines: Vec<TextLayoutLineWire>,
    pub overflow: bool,
    pub used_height: f64,
}

/// Read the `TextFrameOptions` metadata for a `TextLayer` node and
/// return it as JSON. Returns the `Default` JSON (single column,
/// no hyphenation, clip overflow, top-aligned, no inset, fixed
/// size) when the node has no `text_frame` metadata yet — this is
/// the documented behaviour of [`Node::text_frame_options`] and lets
/// the UI mount the panel without needing to special-case freshly
/// created text nodes.
pub fn text_frame_get(node_id: Uuid) -> Result<String> {
    let options = with_workspace(|ws| {
        let node = ws
            .project
            .document
            .get_node(node_id)
            .ok_or(DocumentBridgeError::NodeNotFound(node_id))?;
        if node.node_type != NodeType::TextLayer {
            return Err(DocumentBridgeError::InvalidArgument {
                argument: "node_id".into(),
                value: format!("node {node_id} is not a TextLayer"),
            });
        }
        Ok(node.text_frame_options())
    })?;
    Ok(serde_json::to_string(&options)?)
}

/// Replace the `TextFrameOptions` metadata for a `TextLayer` node and
/// record an operation in the project's log.
///
/// The operation `command` is `"text_frame_update"`; `before_patch` /
/// `after_patch` are the previous / new options JSON; `affected_nodes`
/// contains the single node id so the renderer dispatcher can
/// invalidate that node's cached layout. Undo is real — see
/// [`color_settings_update`] for the full contract; the bridge replays
/// `before_patch` onto the node itself, so `document_undo` actually
/// restores the previous `TextFrameOptions`.
pub fn text_frame_update(node_id: Uuid, options_json: &str) -> Result<()> {
    use kcreate_core::node::TextFrameOptions;
    let new_options: TextFrameOptions = serde_json::from_str(options_json)?;
    with_workspace_mut(|ws| {
        let node = ws
            .project
            .document
            .get_node(node_id)
            .ok_or(DocumentBridgeError::NodeNotFound(node_id))?;
        if node.node_type != NodeType::TextLayer {
            return Err(DocumentBridgeError::InvalidArgument {
                argument: "node_id".into(),
                value: format!("node {node_id} is not a TextLayer"),
            });
        }
        let before = serde_json::to_value(node.text_frame_options())?;
        let after = serde_json::to_value(&new_options)?;
        let node_mut = ws
            .project
            .document
            .get_node_mut(node_id)
            .ok_or(DocumentBridgeError::NodeNotFound(node_id))?;
        node_mut.set_text_frame_options(&new_options);
        let op = Operation::new("user", "text_frame_update", before, after, vec![node_id]);
        ws.project.execute_operation(op);
        Ok(())
    })?;
    sync_scene_after_change();
    Ok(())
}

/// Compute the paragraph layout for a `TextLayer` node and return a
/// JSON wire-format describing each line. The renderer uses
/// [`kcreate_text::layout_paragraph`] for actual drawing; this entry
/// point exists so the inspector / debug overlay can show line
/// outlines and overflow without owning a font manager in TS.
///
/// Text + font are read from the node's canonical
/// [`kcreate_export::TextLayerMeta`] at the `TEXT_LAYER_METADATA_KEY`
/// metadata slot — that is the same payload `scene_sync` reads to
/// drive the renderer, so the layout inspector now sees exactly what
/// the canvas sees. `line_height` defaults to 1.25 because
/// `TextLayerMeta` does not yet carry it; if a caller wants a
/// different leading they can override via the optional
/// `metadata["text_style"]` slot (a `TextStyleWire` JSON object).
///
/// For backward compatibility with the older "bare string at
/// metadata\[text\]" convention (still used by some bridge tests),
/// the code falls back to reading the metadata value as a JSON string
/// if it cannot be deserialised as a `TextLayerMeta`.
pub fn text_layout_compute(node_id: Uuid) -> Result<String> {
    use kcreate_core::node::TextFrameOptions;
    use kcreate_export::TextLayerMeta;
    use kcreate_text::{layout_paragraph, HyphenationPatterns, TextStyle, EN_US_PATTERNS};

    let (text, style, frame, bounds) = with_workspace(|ws| {
        let node = ws
            .project
            .document
            .get_node(node_id)
            .ok_or(DocumentBridgeError::NodeNotFound(node_id))?;
        if node.node_type != NodeType::TextLayer {
            return Err(DocumentBridgeError::InvalidArgument {
                argument: "node_id".into(),
                value: format!("node {node_id} is not a TextLayer"),
            });
        }
        // Resolve text + base font from the canonical TextLayerMeta
        // slot. Fall back to the legacy bare-string convention if the
        // value at `metadata["text"]` isn't a TextLayerMeta object —
        // older tests still write a raw string there.
        let raw_meta = node
            .metadata
            .get(crate::scene_sync::TEXT_LAYER_METADATA_KEY);
        let (text, font_family, font_size) = match raw_meta {
            Some(v) => match serde_json::from_value::<TextLayerMeta>(v.clone()) {
                Ok(meta) => (meta.text, Some(meta.font_family), Some(meta.font_size)),
                Err(_) => (
                    v.as_str().unwrap_or("").to_string(),
                    None::<String>,
                    None::<f32>,
                ),
            },
            None => (String::new(), None::<String>, None::<f32>),
        };

        // `metadata["text_style"]` is the optional override slot
        // (line_height + any overrides on font_family / font_size).
        let mut style: TextStyle = node
            .metadata
            .get("text_style")
            .and_then(|v| serde_json::from_value::<TextStyleWire>(v.clone()).ok())
            .map(TextStyle::from)
            .unwrap_or_default();
        if let Some(family) = font_family {
            // Only overwrite from TextLayerMeta if the override slot
            // did not provide a non-default family. We treat the
            // TextStyle default's family ("sans-serif") as "no
            // override supplied".
            if style.font_family == TextStyle::default().font_family {
                style.font_family = family;
            }
        }
        if let Some(size) = font_size {
            if (style.font_size - TextStyle::default().font_size).abs() < f32::EPSILON {
                style.font_size = size;
            }
        }

        let frame: TextFrameOptions = node.text_frame_options();
        Ok((text, style, frame, node.bounds))
    })?;

    // Hyphenation patterns: only English ships embedded today.
    // Languages other than English fall through to `None` (no
    // hyphenation) until the project ships additional `.pat` files —
    // matches the Task 8 design.
    let patterns: Option<HyphenationPatterns> =
        if frame.hyphenation && frame.hyphenation_language.to_lowercase().starts_with("en") {
            Some(HyphenationPatterns::from_tex_patterns(EN_US_PATTERNS))
        } else {
            None
        };

    let layout =
        layout_paragraph(&text, &style, &frame, bounds, patterns.as_ref()).map_err(|e| {
            DocumentBridgeError::InvalidArgument {
                argument: "layout".into(),
                value: e.to_string(),
            }
        })?;

    let wire = TextLayoutWire {
        lines: layout
            .lines
            .iter()
            .map(|l| TextLayoutLineWire {
                origin_x: l.origin_x,
                baseline_y: l.baseline_y,
                width: l.width,
                column: l.column,
                glyph_count: l.glyphs.len(),
            })
            .collect(),
        overflow: layout.overflow,
        used_height: layout.used_height,
    };
    Ok(serde_json::to_string(&wire)?)
}

/// Wire format for the renderer-side `TextStyle` carried in the
/// `metadata["text_style"]` field. Mirrors
/// [`kcreate_text::paragraph::TextStyle`] one-for-one but is owned
/// here because the bridge crate is the wire-format boundary
/// (rule 4 of AGENTS.md). Adding a field on either side requires
/// adding it here too plus a test in `document.rs`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TextStyleWire {
    font_family: String,
    font_size: f32,
    line_height: f64,
}

impl From<TextStyleWire> for kcreate_text::TextStyle {
    fn from(w: TextStyleWire) -> Self {
        Self {
            font_family: w.font_family,
            font_size: w.font_size,
            line_height: w.line_height,
        }
    }
}

/// Read the `OpenTypeFeatures` metadata for a `TextLayer` node and
/// return it as JSON. Returns the `Default` JSON (ligatures +
/// contextual_alternates + kerning on, everything else off, no
/// stylistic sets) when the node has no `opentype_features`
/// metadata yet.
pub fn text_opentype_features_get(node_id: Uuid) -> Result<String> {
    let features = with_workspace(|ws| {
        let node = ws
            .project
            .document
            .get_node(node_id)
            .ok_or(DocumentBridgeError::NodeNotFound(node_id))?;
        if node.node_type != NodeType::TextLayer {
            return Err(DocumentBridgeError::InvalidArgument {
                argument: "node_id".into(),
                value: format!("node {node_id} is not a TextLayer"),
            });
        }
        Ok(node.opentype_features())
    })?;
    Ok(serde_json::to_string(&features)?)
}

/// Replace the `OpenTypeFeatures` metadata for a `TextLayer` node and
/// record an operation. `command` is `"text_opentype_features_update"`;
/// undo / scene-sync semantics mirror [`text_frame_update`].
pub fn text_opentype_features_update(node_id: Uuid, features_json: &str) -> Result<()> {
    use kcreate_core::node::OpenTypeFeatures;
    let new_features: OpenTypeFeatures = serde_json::from_str(features_json)?;
    with_workspace_mut(|ws| {
        let node = ws
            .project
            .document
            .get_node(node_id)
            .ok_or(DocumentBridgeError::NodeNotFound(node_id))?;
        if node.node_type != NodeType::TextLayer {
            return Err(DocumentBridgeError::InvalidArgument {
                argument: "node_id".into(),
                value: format!("node {node_id} is not a TextLayer"),
            });
        }
        let before = serde_json::to_value(node.opentype_features())?;
        let after = serde_json::to_value(&new_features)?;
        let node_mut = ws
            .project
            .document
            .get_node_mut(node_id)
            .ok_or(DocumentBridgeError::NodeNotFound(node_id))?;
        node_mut.set_opentype_features(&new_features);
        let op = Operation::new(
            "user",
            "text_opentype_features_update",
            before,
            after,
            vec![node_id],
        );
        ws.project.execute_operation(op);
        Ok(())
    })?;
    sync_scene_after_change();
    Ok(())
}

// -----------------------------------------------------------------------------
// PDF import (Phase 3 foundation — Tasks 26-27)
// -----------------------------------------------------------------------------

/// JSON-serialisable report returned to the renderer after a PDF
/// import. The renderer uses this to show "Imported 4 pages
/// (2 images skipped)" so the user knows what happened.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PdfImportReport {
    /// Title field from the PDF's `/Info` dict, if present.
    pub title: Option<String>,
    /// Author field from the PDF's `/Info` dict, if present.
    pub author: Option<String>,
    /// Page ids of the newly-inserted KCreate pages, in import
    /// order. The renderer can navigate to `pages[0]` immediately
    /// after import.
    pub page_ids: Vec<Uuid>,
    /// Total images successfully extracted across all pages.
    pub images_imported: usize,
    /// Total images skipped (unsupported filter / color space).
    pub images_skipped: usize,
    /// Pages with empty content streams or unreadable MediaBoxes.
    /// Surfaces as a non-blocking warning in the UI.
    pub warnings: Vec<String>,
}

/// PDF point → screen-space pixel scale used by KCreate's page
/// layout system. 1 pt = 1/72 in, KCreate scenes are at 96 dpi, so
/// 1 pt = 96/72 px.
const PT_TO_PX: f64 = 96.0 / 72.0;

/// Import a PDF into the *current* project (one Page per PDF page).
/// Each KCreate page is sized to the PDF page's MediaBox; embedded
/// JPEG / Flate images become RasterLayer children; extracted text
/// becomes a TextLayer per page (or is omitted if the page has no
/// text).
///
/// Records one undoable operation per imported page, so the user can
/// hit Undo to remove specific pages from the import or Cmd-Z several
/// times to undo the whole batch.
pub fn pdf_import(file_path: String) -> Result<String> {
    let imported = pdf_import_run(&file_path).map_err(map_pdf_import_err)?;
    let report = ingest_imported_pdf(imported)?;
    Ok(serde_json::to_string(&report)?)
}

/// Translate a [`PdfImportError`] into the bridge error envelope. We
/// preserve the kind in the message string so the renderer's error
/// surface can show "PDF is encrypted" vs. "PDF has no pages"
/// without re-defining the enum on the TS side.
fn map_pdf_import_err(err: PdfImportError) -> DocumentBridgeError {
    DocumentBridgeError::Io(std::io::Error::other(err.to_string()))
}

/// Take a freshly-parsed [`ImportedPdf`] and project it onto the
/// current workspace's document. Returns a [`PdfImportReport`]
/// summarising what landed.
fn ingest_imported_pdf(imported: ImportedPdf) -> Result<PdfImportReport> {
    let mut page_ids = Vec::with_capacity(imported.pages.len());
    let mut images_imported = 0usize;
    let mut images_skipped = 0usize;
    let mut warnings: Vec<String> = imported.warnings.iter().map(format_pdf_warning).collect();

    for imported_page in imported.pages {
        images_skipped += imported_page.skipped_images;
        let page_id = with_workspace_mut(|ws| {
            let label = if imported_page.text.trim().is_empty() {
                format!("PDF page {}", imported_page.index + 1)
            } else {
                let snippet: String = imported_page
                    .text
                    .lines()
                    .next()
                    .unwrap_or("")
                    .chars()
                    .take(32)
                    .collect();
                if snippet.is_empty() {
                    format!("PDF page {}", imported_page.index + 1)
                } else {
                    format!("{} — page {}", snippet, imported_page.index + 1)
                }
            };

            // Add the empty Page first so child nodes can hang off
            // it. `Project::add_page` records its own op; we record
            // the population step (images + text) as a separate op
            // below so the user can undo "add page" and "fill page"
            // independently.
            let new_page_id = ws.project.add_page(label)?;

            if let Some(page_node) = ws.project.document.get_node_mut(new_page_id) {
                let width_px = imported_page.width_pt * PT_TO_PX;
                let height_px = imported_page.height_pt * PT_TO_PX;
                page_node.bounds = Bounds::new(0.0, 0.0, width_px, height_px);
            }

            // Embed images.
            let mut created_node_ids = Vec::<Uuid>::new();
            for img in &imported_page.images {
                let blob = ws
                    .store
                    .blobs()
                    .store(img.data.bytes(), img.data.mime_type())
                    .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))?;
                let meta = crate::scene_sync::RasterImageMeta {
                    blob_hash: blob.hash,
                    width: img.width,
                    height: img.height,
                };
                let mut node = Node::new(NodeType::RasterLayer, "Imported image");
                node.parent_id = Some(new_page_id);
                node.bounds = Bounds::new(0.0, 0.0, f64::from(img.width), f64::from(img.height));
                node.metadata.insert(
                    crate::scene_sync::RASTER_IMAGE_METADATA_KEY.to_string(),
                    serde_json::to_value(&meta)?,
                );
                let id = ws.project.document.insert_node(node)?;
                created_node_ids.push(id);
            }

            // Embed extracted text as a single TextLayer if the
            // page had any.
            let trimmed = imported_page.text.trim();
            if !trimmed.is_empty() {
                let meta = kcreate_export::TextLayerMeta {
                    text: imported_page.text.clone(),
                    font_family: "Helvetica".to_string(),
                    font_size: 12.0,
                };
                let mut node = Node::new(NodeType::TextLayer, "Imported text");
                node.parent_id = Some(new_page_id);
                let (page_w, page_h) = (
                    imported_page.width_pt * PT_TO_PX,
                    imported_page.height_pt * PT_TO_PX,
                );
                // Default text block: full page width minus 1in
                // margins on each side, top half of the page. The
                // user is expected to re-flow this; we just give
                // them something visible.
                let margin = 96.0; // 1in @ 96dpi
                node.bounds = Bounds::new(
                    margin,
                    margin,
                    (page_w - margin * 2.0).max(96.0),
                    (page_h / 2.0).max(96.0),
                );
                node.metadata.insert(
                    crate::scene_sync::TEXT_LAYER_METADATA_KEY.to_string(),
                    serde_json::to_value(&meta)?,
                );
                let id = ws.project.document.insert_node(node)?;
                created_node_ids.push(id);
            }

            // Record one undoable op for the page-population step
            // (images + text). The page itself was recorded by
            // `add_page`.
            if !created_node_ids.is_empty() {
                let snapshot = serde_json::json!({
                    "page_id": new_page_id,
                    "node_count": created_node_ids.len(),
                });
                let op = Operation::new(
                    "user",
                    "pdf_import_populate_page",
                    serde_json::Value::Null,
                    snapshot,
                    created_node_ids.clone(),
                );
                ws.project.execute_operation(op);
            }
            ws.project.modified_at = Utc::now();

            images_imported += imported_page.images.len();
            Ok(new_page_id)
        })?;
        page_ids.push(page_id);
    }

    sync_scene_after_change();

    if page_ids.is_empty() {
        warnings.push("PDF contained no importable pages".to_string());
    }

    Ok(PdfImportReport {
        title: imported.title,
        author: imported.author,
        page_ids,
        images_imported,
        images_skipped,
        warnings,
    })
}

fn format_pdf_warning(w: &kcreate_export::pdf_import::PdfImportWarning) -> String {
    use kcreate_export::pdf_import::PdfImportWarning as W;
    match w {
        W::UnsupportedImageFilter {
            page_index,
            filter_chain,
        } => format!(
            "Page {}: unsupported image filter ({})",
            page_index + 1,
            filter_chain
        ),
        W::UnsupportedImageColorSpace {
            page_index,
            color_space,
        } => format!(
            "Page {}: unsupported image color space ({})",
            page_index + 1,
            color_space
        ),
        W::MissingMediaBox { page_index } => format!(
            "Page {}: missing MediaBox — defaulted to US Letter",
            page_index + 1
        ),
    }
}

// Suppress unused-import warnings if the JPEG/PNG enum tag ever
// becomes the only reference. `ExtractedImageData` is currently
// only matched via its `bytes()`/`mime_type()` helpers above so
// the explicit re-export from `lib.rs` still flows through.
#[allow(dead_code)]
fn _force_extracted_image_data_link(d: &ExtractedImageData) -> usize {
    d.bytes().len()
}

// -----------------------------------------------------------------------------
// Avoid unused warnings on disabled features
// -----------------------------------------------------------------------------
// Template marketplace (Phase 3 — local only)
// -----------------------------------------------------------------------------

fn template_marketplace() -> &'static Mutex<kcreate_core::LocalMarketplace> {
    static MP: OnceLock<Mutex<kcreate_core::LocalMarketplace>> = OnceLock::new();
    MP.get_or_init(|| {
        let root = template_dir();
        let mut mp = kcreate_core::LocalMarketplace::new(root);
        let _ = mp.scan();
        Mutex::new(mp)
    })
}

fn template_dir() -> PathBuf {
    std::env::var("KCREATE_TEMPLATE_DIR").map_or_else(
        |_| kcreate_core::LocalMarketplace::default_dir(),
        PathBuf::from,
    )
}

/// Re-seed the template marketplace from the current `template_dir()`
/// so per-test directories take effect. Tasks 11-12 (the bridge tests
/// for `template_install_local` / `template_remove`) consume this; it
/// stays compiled in `cfg(test)` builds so those tests have a clean
/// per-test marketplace state without needing to mutate the
/// singleton's internals.
#[cfg(test)]
#[allow(dead_code)]
pub(crate) fn reset_marketplace_for_tests() {
    let dir = template_dir();
    let mut mp = template_marketplace().lock();
    *mp = kcreate_core::LocalMarketplace::new(dir);
    let _ = mp.scan();
}

#[derive(Debug, Clone, Serialize)]
pub struct TemplateListReport {
    pub templates: Vec<kcreate_core::TemplateManifest>,
}

/// List all installed local templates. Optionally filter by
/// category or search query.
pub fn template_list(
    category: Option<kcreate_core::TemplateCategory>,
    query: Option<&str>,
) -> Result<TemplateListReport> {
    let mp = template_marketplace().lock();
    let templates: Vec<kcreate_core::TemplateManifest> = if let Some(q) = query {
        mp.search(q).into_iter().cloned().collect()
    } else if let Some(cat) = category {
        mp.filter_by_category(cat).into_iter().cloned().collect()
    } else {
        mp.list().into_iter().cloned().collect()
    };
    Ok(TemplateListReport { templates })
}

/// Install a `.ktemplate/` folder from a local path. Wraps
/// [`kcreate_core::LocalMarketplace::install_local`] and lets the
/// underlying [`kcreate_core::MarketplaceError`] propagate through
/// the `#[from]` conversion on [`crate::document::DocumentBridgeError`]
/// so the renderer receives a structured error rather than a
/// stringified one.
pub fn template_install_local(source_path: &str) -> Result<kcreate_core::TemplateManifest> {
    let path = PathBuf::from(source_path);
    let mut mp = template_marketplace().lock();
    Ok(mp.install_local(&path)?)
}

/// Remove an installed template by id.
pub fn template_remove(template_id: uuid::Uuid) -> Result<()> {
    let mut mp = template_marketplace().lock();
    mp.remove(template_id)?;
    Ok(())
}

// -----------------------------------------------------------------------------

#[allow(dead_code)]
fn _suppress_unused_atomic() {
    let _ = AtomicBool::new(false);
    let _ = Ordering::SeqCst;
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod ai_inference_tests {
    use super::*;
    use crate::document::{project_close, project_create, reset_for_tests};
    use kcreate_ai::AltTextReport;
    use kcreate_ai::LayoutSuggestion;
    use serial_test::serial;

    /// Encode a small RGBA8 buffer as PNG so `image::load_from_memory`
    /// in `ai_alt_text_for_node` can decode it.
    fn rgba_png(width: u32, height: u32, fill: [u8; 4]) -> Vec<u8> {
        let pixel_count = (width as usize) * (height as usize);
        let mut rgba = Vec::with_capacity(pixel_count * 4);
        for _ in 0..pixel_count {
            rgba.extend_from_slice(&fill);
        }
        let mut png: Vec<u8> = Vec::new();
        let mut cursor = std::io::Cursor::new(&mut png);
        image::write_buffer_with_format(
            &mut cursor,
            &rgba,
            width,
            height,
            image::ColorType::Rgba8,
            image::ImageFormat::Png,
        )
        .expect("png encode");
        png
    }

    /// Insert a raster node with `pixels` bytes (already PNG-encoded)
    /// directly into the open workspace, bypassing the file-system
    /// `document_import_image` path so tests don't have to write a
    /// PNG to disk just to read it back.
    fn insert_test_raster(png: &[u8], width: u32, height: u32, parent: Option<Uuid>) -> Uuid {
        with_workspace_mut(|ws| {
            let blob = ws
                .store
                .blobs()
                .store(png, "image/png")
                .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))?;
            let meta = crate::scene_sync::RasterImageMeta {
                blob_hash: blob.hash,
                width,
                height,
            };
            let mut node = Node::new(NodeType::RasterLayer, "Test raster");
            node.parent_id = parent;
            node.bounds = Bounds {
                x: 0.0,
                y: 0.0,
                width: f64::from(width),
                height: f64::from(height),
            };
            node.metadata.insert(
                crate::scene_sync::RASTER_IMAGE_METADATA_KEY.to_string(),
                serde_json::to_value(&meta)?,
            );
            let id = ws.project.document.insert_node(node)?;
            Ok::<_, DocumentBridgeError>(id)
        })
        .expect("insert raster")
    }

    fn tmpdir() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    /// `ai_alt_text_for_node` decodes the raster, runs the alt-text
    /// heuristic, and returns the full `AltTextReport` as JSON. The
    /// document is read-only — no operation is recorded, no
    /// metadata is mutated.
    #[test]
    #[serial]
    fn alt_text_for_node_returns_report_without_mutating_document() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("alt-text", dir.path()).expect("project");
        // A bright, fully-saturated red raster is enough to drive a
        // deterministic palette + brightness classification through
        // the alt-text heuristic.
        let png = rgba_png(16, 16, [240, 20, 20, 255]);
        let raster_id = insert_test_raster(&png, 16, 16, None);

        // Snapshot the node version before the analysis.
        let version_before = with_workspace(|ws| {
            Ok::<u64, DocumentBridgeError>(
                ws.project
                    .document
                    .get_node(raster_id)
                    .expect("node")
                    .version,
            )
        })
        .unwrap();

        let json = ai_alt_text_for_node(raster_id).expect("alt-text");
        let report: AltTextReport = serde_json::from_str(&json).expect("decode");
        // The heuristic always emits some text; the palette must be
        // non-empty because we asked for at least one color.
        assert!(!report.text.is_empty(), "text must be non-empty");
        assert!(
            !report.palette.is_empty(),
            "palette must contain at least one color"
        );
        // Bright red → high saturation, mid-brightness, low edge
        // density (uniform fill).
        assert!(
            report.saturation > 0.5,
            "saturation must reflect the strong red fill (got {})",
            report.saturation
        );
        assert!(
            report.edge_density < 0.05,
            "uniform fill has near-zero edges (got {})",
            report.edge_density
        );

        let version_after = with_workspace(|ws| {
            Ok::<u64, DocumentBridgeError>(
                ws.project
                    .document
                    .get_node(raster_id)
                    .expect("node")
                    .version,
            )
        })
        .unwrap();
        assert_eq!(
            version_before, version_after,
            "analysis-only call must NOT touch the document"
        );
        assert!(
            with_workspace(|ws| Ok::<bool, DocumentBridgeError>(
                ws.project
                    .document
                    .get_node(raster_id)
                    .expect("node")
                    .alt_text()
                    .is_none()
            ))
            .unwrap(),
            "analysis-only call must NOT write alt_text metadata"
        );
        project_close();
    }

    /// `ai_apply_alt_text` writes the label, records an op, and the
    /// `Node::alt_text()` accessor returns the new string. Passing
    /// an empty string clears the entry per the documented
    /// `empty == missing` semantic.
    #[test]
    #[serial]
    fn apply_alt_text_writes_then_clears_round_trip() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("apply-alt", dir.path()).expect("project");
        let png = rgba_png(4, 4, [128, 128, 128, 255]);
        let raster_id = insert_test_raster(&png, 4, 4, None);

        // Write.
        ai_apply_alt_text(raster_id, "A neutral grey square".to_string()).expect("apply");
        let stored = with_workspace(|ws| {
            Ok::<Option<String>, DocumentBridgeError>(
                ws.project
                    .document
                    .get_node(raster_id)
                    .expect("node")
                    .alt_text()
                    .map(str::to_string),
            )
        })
        .unwrap();
        assert_eq!(stored.as_deref(), Some("A neutral grey square"));

        // Clear with empty string.
        ai_apply_alt_text(raster_id, String::new()).expect("clear");
        let stored = with_workspace(|ws| {
            Ok::<Option<String>, DocumentBridgeError>(
                ws.project
                    .document
                    .get_node(raster_id)
                    .expect("node")
                    .alt_text()
                    .map(str::to_string),
            )
        })
        .unwrap();
        assert_eq!(
            stored, None,
            "passing empty string must clear the metadata entry entirely"
        );
        project_close();
    }

    /// `ai_alt_text_for_node` rejects a non-raster node so the
    /// renderer doesn't have to defend the call surface for
    /// vector / text / group / page nodes that have no pixels.
    #[test]
    #[serial]
    fn alt_text_rejects_non_raster_nodes() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("non-raster", dir.path()).expect("project");
        let group_id = with_workspace_mut(|ws| {
            let mut node = Node::new(NodeType::GroupLayer, "Group");
            node.bounds = Bounds {
                x: 0.0,
                y: 0.0,
                width: 100.0,
                height: 100.0,
            };
            Ok::<Uuid, DocumentBridgeError>(ws.project.document.insert_node(node)?)
        })
        .expect("insert group");
        let err = ai_alt_text_for_node(group_id).expect_err("must reject");
        assert!(
            matches!(err, DocumentBridgeError::InvalidNodeType(_)),
            "non-raster node must produce InvalidNodeType (got {err:?})"
        );
        project_close();
    }

    /// `ai_layout_suggest_for_artboard` clusters children by
    /// proximity and returns at least one suggestion when the input
    /// is a clear visual row. The candidate count drives a real
    /// algorithm call; we don't fabricate suggestions.
    #[test]
    #[serial]
    fn layout_suggest_groups_visually_aligned_row() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("layout", dir.path()).expect("project");

        // Build an artboard with three small squares laid out in a
        // perfectly aligned horizontal row.
        let artboard_id = with_workspace_mut(|ws| {
            let mut art = Node::new(NodeType::Artboard, "Frame");
            art.bounds = Bounds {
                x: 0.0,
                y: 0.0,
                width: 800.0,
                height: 200.0,
            };
            let id = ws.project.document.insert_node(art)?;
            for (i, x) in [10.0, 60.0, 110.0].iter().enumerate() {
                let mut child = Node::new(NodeType::VectorLayer, format!("Box {i}"));
                child.parent_id = Some(id);
                child.bounds = Bounds {
                    x: *x,
                    y: 50.0,
                    width: 40.0,
                    height: 40.0,
                };
                ws.project.document.insert_node(child)?;
            }
            Ok::<Uuid, DocumentBridgeError>(id)
        })
        .expect("artboard");

        let json = ai_layout_suggest_for_artboard(artboard_id).expect("suggest");
        let suggestions: Vec<LayoutSuggestion> = serde_json::from_str(&json).expect("decode");
        assert!(
            !suggestions.is_empty(),
            "an aligned row must produce at least one suggestion"
        );
        // The largest suggestion must cover all three boxes.
        let cover = suggestions
            .iter()
            .map(|s| s.member_ids.len())
            .max()
            .unwrap();
        assert_eq!(cover, 3, "top suggestion must cluster all three boxes");
        project_close();
    }

    /// `ai_layout_suggest_for_artboard` returns `[]` instead of an
    /// error when the artboard has fewer than two eligible
    /// children — the algorithm requires ≥2 candidates, but the UI
    /// must be able to call the bridge unconditionally on
    /// selection without a special case for "0 or 1 child".
    #[test]
    #[serial]
    fn layout_suggest_returns_empty_array_for_too_few_children() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("layout-empty", dir.path()).expect("project");
        let empty_artboard_id = with_workspace_mut(|ws| {
            let mut art = Node::new(NodeType::Artboard, "Empty");
            art.bounds = Bounds {
                x: 0.0,
                y: 0.0,
                width: 100.0,
                height: 100.0,
            };
            Ok::<Uuid, DocumentBridgeError>(ws.project.document.insert_node(art)?)
        })
        .expect("artboard");

        let json = ai_layout_suggest_for_artboard(empty_artboard_id).expect("suggest");
        assert_eq!(
            json, "[]",
            "empty artboard must serialise as the literal empty array, not an error"
        );
        project_close();
    }

    /// `ai_layout_suggest_for_artboard` skips invisible and
    /// degenerate-bounds children so accidentally-hidden helper
    /// layers don't pollute the suggestion set.
    #[test]
    #[serial]
    fn layout_suggest_skips_invisible_and_degenerate_children() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("layout-filter", dir.path()).expect("project");
        let artboard_id = with_workspace_mut(|ws| {
            let mut art = Node::new(NodeType::Artboard, "Frame");
            art.bounds = Bounds {
                x: 0.0,
                y: 0.0,
                width: 400.0,
                height: 200.0,
            };
            let id = ws.project.document.insert_node(art)?;
            // Two visible nodes that should make it through.
            for (i, x) in [10.0, 60.0].iter().enumerate() {
                let mut child = Node::new(NodeType::VectorLayer, format!("Visible {i}"));
                child.parent_id = Some(id);
                child.bounds = Bounds {
                    x: *x,
                    y: 50.0,
                    width: 40.0,
                    height: 40.0,
                };
                ws.project.document.insert_node(child)?;
            }
            // Invisible — must be filtered out before the heuristic.
            let mut hidden = Node::new(NodeType::VectorLayer, "Hidden");
            hidden.parent_id = Some(id);
            hidden.bounds = Bounds {
                x: 200.0,
                y: 50.0,
                width: 40.0,
                height: 40.0,
            };
            hidden.visible = false;
            ws.project.document.insert_node(hidden)?;
            // Degenerate bounds (zero width) — must also be filtered.
            let mut degen = Node::new(NodeType::VectorLayer, "Degenerate");
            degen.parent_id = Some(id);
            degen.bounds = Bounds {
                x: 300.0,
                y: 50.0,
                width: 0.0,
                height: 40.0,
            };
            ws.project.document.insert_node(degen)?;
            Ok::<Uuid, DocumentBridgeError>(id)
        })
        .expect("artboard");

        let json = ai_layout_suggest_for_artboard(artboard_id).expect("suggest");
        let suggestions: Vec<LayoutSuggestion> = serde_json::from_str(&json).expect("decode");
        // With only the two visible non-degenerate boxes, the
        // top suggestion clusters exactly those two.
        let cover = suggestions
            .iter()
            .map(|s| s.member_ids.len())
            .max()
            .unwrap_or(0);
        assert_eq!(
            cover, 2,
            "filtering must leave exactly two clusterable children"
        );
        project_close();
    }

    // -----------------------------------------------------------------
    // Text-region detection + insert-as-text-layer (Phase 4 Block D)
    // -----------------------------------------------------------------

    /// Build a PNG raster of `width × height` with a black bar
    /// drawn in the rect `(x..x+w, y..y+h)`. The detector treats
    /// dark pixels as ink, so the bar materialises as a single
    /// text-shaped region. Useful for exercising both detection
    /// and the raster→document coordinate mapping in the
    /// insert-text-layer path.
    fn rgba_png_with_bar(width: u32, height: u32, bar: (u32, u32, u32, u32)) -> Vec<u8> {
        let pixel_count = (width as usize) * (height as usize);
        let mut rgba = vec![255u8; pixel_count * 4]; // white background
                                                     // Re-set alpha to 255 explicitly for the rgba layout (the
                                                     // initial `255` fills R/G/B too which is what we want for
                                                     // white — alpha follows naturally).
        let (bx, by, bw, bh) = bar;
        for ry in by..(by + bh).min(height) {
            for rx in bx..(bx + bw).min(width) {
                let o = ((ry as usize) * (width as usize) + (rx as usize)) * 4;
                rgba[o] = 0;
                rgba[o + 1] = 0;
                rgba[o + 2] = 0;
                rgba[o + 3] = 255;
            }
        }
        let mut png: Vec<u8> = Vec::new();
        let mut cursor = std::io::Cursor::new(&mut png);
        image::write_buffer_with_format(
            &mut cursor,
            &rgba,
            width,
            height,
            image::ColorType::Rgba8,
            image::ImageFormat::Png,
        )
        .expect("png encode");
        png
    }

    /// `TextRegion` round-trips through serde with the exact wire
    /// shape the TypeScript mirror expects (camelCase keys).
    /// Pins the public wire format — if anyone re-renames or
    /// drops a field, this test breaks at `cargo test` time
    /// rather than failing silently in the renderer.
    #[test]
    fn text_region_serialises_to_camelcase_wire_format() {
        let region = kcreate_ai::TextRegion {
            x: 4,
            y: 5,
            width: 60,
            height: 12,
            glyph_count: 9,
            estimated_char_count: 11,
        };
        let json = serde_json::to_string(&region).expect("encode");
        let v: serde_json::Value = serde_json::from_str(&json).expect("re-decode");
        assert_eq!(v["x"], 4);
        assert_eq!(v["y"], 5);
        assert_eq!(v["width"], 60);
        assert_eq!(v["height"], 12);
        assert_eq!(v["glyphCount"], 9);
        assert_eq!(v["estimatedCharCount"], 11);
        // Round-trip back through the detector struct.
        let back: kcreate_ai::TextRegion = serde_json::from_str(&json).expect("decode");
        assert_eq!(back.x, 4);
        assert_eq!(back.glyph_count, 9);
        assert_eq!(back.estimated_char_count, 11);
    }

    /// The renderer-facing `InsertTextLayerForRegionRequest`
    /// accepts the JSON shape declared in
    /// `apps/desktop/shared/scene.ts::InsertTextLayerForRegionRequest`
    /// — including the optional `text` / `fontFamily` / `fontSize`
    /// fields with snake_case Rust names. `deny_unknown_fields`
    /// prevents accidental rename drift.
    #[test]
    fn insert_text_layer_request_accepts_camelcase_optional_fields() {
        let raster_id = Uuid::new_v4();
        let wire = serde_json::json!({
            "rasterNodeId": raster_id.to_string(),
            "region": {
                "x": 4,
                "y": 5,
                "width": 60,
                "height": 12,
                "glyphCount": 9,
                "estimatedCharCount": 11,
            },
            "text": "Hello",
            "fontFamily": "Inter",
            "fontSize": 14.0,
        });
        let req: InsertTextLayerForRegionRequest = serde_json::from_value(wire).expect("decode");
        assert_eq!(req.raster_node_id, raster_id);
        assert_eq!(req.region.glyph_count, 9);
        assert_eq!(req.region.estimated_char_count, 11);
        assert_eq!(req.text, "Hello");
        assert_eq!(req.font_family.as_deref(), Some("Inter"));
        assert!((req.font_size.expect("size") - 14.0).abs() < 1e-6);

        // Optional fields really are optional — omit text / font*.
        let minimal = serde_json::json!({
            "rasterNodeId": Uuid::new_v4().to_string(),
            "region": {
                "x": 0, "y": 0, "width": 1, "height": 1,
            },
        });
        let minimal_req: InsertTextLayerForRegionRequest =
            serde_json::from_value(minimal).expect("decode minimal");
        assert!(minimal_req.text.is_empty());
        assert!(minimal_req.font_family.is_none());
        assert!(minimal_req.font_size.is_none());

        // Unknown field at top level — `deny_unknown_fields` rejects
        // it so we catch wire-format drift at compile / test time.
        let stray = serde_json::json!({
            "rasterNodeId": Uuid::new_v4().to_string(),
            "region": {"x": 0, "y": 0, "width": 1, "height": 1},
            "stray": true,
        });
        assert!(
            serde_json::from_value::<InsertTextLayerForRegionRequest>(stray).is_err(),
            "unknown top-level field must be rejected"
        );
    }

    /// End-to-end: detect regions on a synthetic raster, then
    /// materialise the first one as a TextLayer. Verifies:
    ///   - the detector finds the single ink rectangle we drew,
    ///   - the new node is a `TextLayer` parented under the
    ///     raster's parent (root in this test),
    ///   - the bounds round-trip the raster→document mapping
    ///     correctly (raster is 100×40, bar at (10, 5, 60, 12)
    ///     with raster bounds (x=200, y=300, w=100, h=40)),
    ///   - the new node carries text-layer metadata with the
    ///     heuristic font size derived from the region height.
    #[test]
    #[serial]
    fn detect_then_insert_text_layer_maps_coordinates_to_doc_space() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("ocr", dir.path()).expect("project");
        let png = rgba_png_with_bar(100, 40, (10, 5, 60, 12));
        // Insert the raster with non-trivial bounds so the
        // mapping isn't accidentally an identity transform.
        let raster_id = with_workspace_mut(|ws| {
            let blob = ws
                .store
                .blobs()
                .store(&png, "image/png")
                .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))?;
            let meta = crate::scene_sync::RasterImageMeta {
                blob_hash: blob.hash,
                width: 100,
                height: 40,
            };
            let mut node = Node::new(NodeType::RasterLayer, "Bar raster");
            node.parent_id = None;
            node.bounds = Bounds {
                x: 200.0,
                y: 300.0,
                width: 100.0,
                height: 40.0,
            };
            node.metadata.insert(
                crate::scene_sync::RASTER_IMAGE_METADATA_KEY.to_string(),
                serde_json::to_value(&meta)?,
            );
            Ok::<Uuid, DocumentBridgeError>(ws.project.document.insert_node(node)?)
        })
        .expect("raster");

        let json = ai_detect_text_regions(raster_id, "null").expect("detect");
        let regions: Vec<kcreate_ai::TextRegion> =
            serde_json::from_str(&json).expect("decode regions");
        assert!(
            !regions.is_empty(),
            "detector must find the synthetic ink bar; got 0 regions"
        );
        // Pick the largest by area — robust against the detector
        // emitting one or two adjacent regions for noisy edges.
        let region = regions
            .iter()
            .max_by_key(|r| r.width * r.height)
            .copied()
            .expect("region");
        // The detector reports the bar bbox (give or take the
        // half-pixel padding inside the heuristic).
        assert!(region.width >= 50 && region.width <= 70);
        assert!(region.height >= 8 && region.height <= 16);

        let req = InsertTextLayerForRegionRequest {
            raster_node_id: raster_id,
            region: TextRegionInsert {
                x: region.x,
                y: region.y,
                width: region.width,
                height: region.height,
                glyph_count: region.glyph_count,
                estimated_char_count: region.estimated_char_count,
            },
            text: String::new(),
            font_family: None,
            font_size: None,
        };
        let new_id = ai_insert_text_layer_for_region(&req).expect("insert");

        let (parent_id, bounds, text_meta_value) = with_workspace(|ws| {
            let n = ws.project.document.get_node(new_id).expect("new node");
            assert!(matches!(n.node_type, NodeType::TextLayer));
            let m = n
                .metadata
                .get(kcreate_export::scene_metadata::TEXT_LAYER_METADATA_KEY)
                .expect("text-layer metadata")
                .clone();
            Ok::<_, DocumentBridgeError>((n.parent_id, n.bounds, m))
        })
        .unwrap();
        assert_eq!(parent_id, None, "new layer is a sibling of the raster");

        // Raster bounds = (200, 300, 100, 40); raster intrinsic =
        // 100 × 40 → identity scale on x, identity scale on y
        // (because the raster's bounds size equals its intrinsic
        // size here). Region (~10, ~5, ~60, ~12) maps to doc
        // (~210, ~305, ~60, ~12).
        assert!((bounds.x - 210.0).abs() < 5.0, "x ≈ 210, got {}", bounds.x);
        assert!((bounds.y - 305.0).abs() < 5.0, "y ≈ 305, got {}", bounds.y);
        assert!(
            bounds.width >= 50.0 && bounds.width <= 70.0,
            "width ≈ 60, got {}",
            bounds.width
        );
        assert!(
            bounds.height >= 8.0 && bounds.height <= 16.0,
            "height ≈ 12, got {}",
            bounds.height
        );

        let text_meta: kcreate_export::scene_metadata::TextLayerMeta =
            serde_json::from_value(text_meta_value).expect("text meta");
        assert!(
            text_meta.text.is_empty(),
            "default text is empty — user types after insertion"
        );
        // Font size comes from region.height * sy * 0.75 where
        // sy = 1.0 here. region.height ~12 → font_size ~9.
        assert!(
            text_meta.font_size > 0.0 && text_meta.font_size < 30.0,
            "font size in plausible range, got {}",
            text_meta.font_size
        );

        // Insert must record an operation with `ai_insert_text_layer`
        // for undo + the AI action log; the latter we don't assert
        // here because ActionLog is process-global and we don't
        // want to introduce inter-test coupling on its contents.
        let op_kinds: Vec<String> = with_workspace(|ws| {
            Ok::<Vec<String>, DocumentBridgeError>(
                ws.project
                    .operation_log
                    .iter()
                    .map(|o| o.command.clone())
                    .collect(),
            )
        })
        .unwrap();
        assert!(
            op_kinds.iter().any(|k| k == "ai_insert_text_layer"),
            "op log must contain ai_insert_text_layer; got {op_kinds:?}",
        );

        project_close();
    }
}
