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
use kcreate_export::preflight::{run_preflight, PreflightIssue, PreflightOptions};
use kcreate_export::pdf::RasterPixelCache;
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
        .map(|s| {
            Uuid::parse_str(s).map_err(|e| DocumentBridgeError::InvalidUuid(s.clone(), e))
        })
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
        .map(|s| {
            Uuid::parse_str(s).map_err(|e| DocumentBridgeError::InvalidUuid(s.clone(), e))
        })
        .collect::<Result<Vec<Uuid>>>()?;
    let output_dir = PathBuf::from(&req.output_dir);
    let scene = current_scene_safe()?;
    let result = with_workspace(|ws| {
        generate_icon_pack(&scene, &ws.project.document, &ids, &req.platforms, &output_dir)
            .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))
    })?;
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
        let outcome = run_batch_parallel(&job, &doc, &rasters, cancel_clone.as_inner(), move |snap| {
            *p.lock() = snap;
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
    let handle = batch_table()
        .lock()
        .get(job_id)
        .cloned()
        .ok_or_else(|| DocumentBridgeError::Io(std::io::Error::other(format!("unknown job {job_id}"))))?;
    let progress = handle.progress.lock().clone();
    let finished_now = handle.result.lock().is_some();
    let mut status = BatchJobStatus {
        job_id: job_id.to_string(),
        completed: progress.completed,
        total: progress.total,
        current_item: progress.current_item,
        finished: finished_now,
        cancelled: false,
        succeeded: Vec::new(),
        failed: Vec::new(),
        duration_ms: 0,
    };
    if let Some(r) = handle.result.lock().as_ref() {
        status.succeeded = r.succeeded.iter().map(|p| p.display().to_string()).collect();
        status.failed.clone_from(&r.failed);
        status.duration_ms = r.duration_ms;
        status.cancelled = r.cancelled;
    }
    if finished_now {
        let join_handle = handle.join.lock().take();
        if let Some(j) = join_handle {
            let _ = j.join();
        }
    }
    Ok(status)
}

pub fn batch_cancel(job_id: &str) -> Result<()> {
    let handle = batch_table()
        .lock()
        .get(job_id)
        .cloned()
        .ok_or_else(|| DocumentBridgeError::Io(std::io::Error::other(format!("unknown job {job_id}"))))?;
    handle.cancel.cancel();
    Ok(())
}

// -----------------------------------------------------------------------------
// AI model packs
// -----------------------------------------------------------------------------

pub fn ai_models_list() -> Result<String> {
    let dir = ai_models_dir();
    let packs = kcreate_ai::list_model_packs(&dir);
    Ok(serde_json::to_string(&packs)?)
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

pub fn ai_upscale(node_id: Uuid, scale: f32) -> Result<Uuid> {
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

    let img = image::load_from_memory(&encoded)
        .map_err(|e| DocumentBridgeError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e)))?;
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
        kcreate_ai::ActionLog::global().lock().append(kcreate_ai::AiAction {
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
    let img = image::load_from_memory(&encoded)
        .map_err(|e| DocumentBridgeError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e)))?;
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
    let img = image::load_from_memory(&encoded)
        .map_err(|e| DocumentBridgeError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e)))?;
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
    let pixels = B64
        .decode(req.image_base64.as_bytes())
        .map_err(|e| DocumentBridgeError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e)))?;
    let elements = kcreate_ai::analyze_screenshot_for_layout(&pixels, req.width, req.height);
    Ok(serde_json::to_string(&elements)?)
}

// -----------------------------------------------------------------------------
// Plugin sandbox
// -----------------------------------------------------------------------------

fn plugin_registry() -> &'static Mutex<kcreate_plugin::PluginRegistry> {
    static R: OnceLock<Mutex<kcreate_plugin::PluginRegistry>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(kcreate_plugin::PluginRegistry::new(plugin_dir())))
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

fn plugin_runtime() -> &'static kcreate_plugin::WasmPluginRuntime {
    static RT: OnceLock<kcreate_plugin::WasmPluginRuntime> = OnceLock::new();
    RT.get_or_init(kcreate_plugin::WasmPluginRuntime::new)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginListEntry {
    #[serde(flatten)]
    pub manifest: kcreate_plugin::PluginManifest,
    pub enabled: bool,
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
            manifest: m.clone(),
        })
        .collect())
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
    let (entry, enabled) = {
        let reg = plugin_registry().lock();
        (reg.entry_point_for(id), reg.is_enabled(id))
    };
    if !enabled {
        return Err(DocumentBridgeError::Io(std::io::Error::other(format!(
            "plugin {id} is not enabled"
        ))));
    }
    let path = entry.ok_or_else(|| {
        DocumentBridgeError::Io(std::io::Error::other(format!("plugin {id} not found")))
    })?;
    let bytes = std::fs::read(&path)?;
    let rt = plugin_runtime();
    let out = rt
        .execute(&bytes, function, input_json, 64)
        .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))?;
    Ok(serde_json::to_string(&serde_json::json!({
        "output": out.output,
        "logs": out.logs,
    }))?)
}

// -----------------------------------------------------------------------------
// MCP permissions
// -----------------------------------------------------------------------------

#[cfg(feature = "mcp")]
fn mcp_permission_store() -> &'static kcreate_mcp::McpPermissionStore {
    static S: OnceLock<kcreate_mcp::McpPermissionStore> = OnceLock::new();
    S.get_or_init(|| {
        let dir = mcp_permission_dir();
        kcreate_mcp::McpPermissionStore::open(&dir)
            .expect("kcreate_bridge: failed to open MCP permission store")
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
        other => Err(DocumentBridgeError::InvalidNodeType(other.to_string())),
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpStatus {
    pub running: bool,
    pub port: u32,
}

pub fn mcp_status() -> McpStatus {
    let running = crate::document::mcp_is_running();
    let port = if running {
        crate::document::mcp_port().unwrap_or(0)
    } else {
        0
    };
    McpStatus { running, port }
}

// -----------------------------------------------------------------------------
// Avoid unused warnings on disabled features
// -----------------------------------------------------------------------------

#[allow(dead_code)]
fn _suppress_unused_atomic() {
    let _ = AtomicBool::new(false);
    let _ = Ordering::SeqCst;
}
