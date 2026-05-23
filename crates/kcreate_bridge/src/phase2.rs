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
use kcreate_export::preflight::{run_preflight, PreflightIssue, PreflightOptions};
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
    with_workspace(|ws| Ok(run_preflight(&ws.project.document, &pages, &req.options)))
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

fn ai_models_dir() -> PathBuf {
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

#[allow(dead_code)]
fn _suppress_unused_atomic() {
    let _ = AtomicBool::new(false);
    let _ = Ordering::SeqCst;
}
