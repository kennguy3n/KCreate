//! N-API bridge: exposes the renderer to the Electron main/renderer
//! processes.
//!
//! The N-API surface in this file is intentionally thin — every function
//! is a one-shot wrapper around [`crate::state`], which holds the actual
//! renderer state machine. This separation lets us:
//!   1. Unit-test the full bridge behavior with plain `cargo test` (no
//!      Node.js process required), because `state` has no `napi`
//!      dependencies.
//!   2. Reuse the same logic from headless tooling and benchmarks.
//!
//! Frames are returned as Node.js `Buffer` values containing a freshly
//! copied snapshot of the latest published frame so JS code can hand
//! them to `ctx.putImageData()` / `createImageBitmap()` without holding
//! the presenter's read lock across the boundary.

#![forbid(unsafe_op_in_unsafe_fn)]

pub mod document;
pub mod state;
pub mod wire;

use std::path::PathBuf;
use std::str::FromStr;

use kcreate_export::svg::SvgExportOptions;
use kcreate_renderer::Rect;
use napi::bindgen_prelude::{Buffer, Error as NapiError, Result as NapiResult, Status};
use napi_derive::napi;
use uuid::Uuid;

use crate::document::{
    CreateNodeProps, DocumentBridgeError, NodeInfo as CoreNodeInfo,
    PngExportRequest as CorePngRequest, ProjectInfo as CoreProjectInfo,
    RuntimeStatus as CoreRuntimeStatus, UpdateNodeProps,
};
use crate::state::{
    AcquiredFrame as CoreAcquiredFrame, BridgeError, RendererFrameInfo as CoreFrameInfo,
    RendererInfo as CoreRendererInfo,
};

// Taken by value so it can be passed directly to `Result::map_err`.
#[allow(clippy::needless_pass_by_value)]
fn map_err(e: BridgeError) -> NapiError {
    let status = match e {
        BridgeError::NotInitialized => Status::InvalidArg,
        _ => Status::GenericFailure,
    };
    NapiError::new(status, format!("kcreate_bridge: {e}"))
}

#[allow(clippy::needless_pass_by_value)]
fn map_doc_err(e: DocumentBridgeError) -> NapiError {
    let status = match e {
        DocumentBridgeError::NoProject
        | DocumentBridgeError::InvalidNodeType(_)
        | DocumentBridgeError::NodeNotFound(_)
        | DocumentBridgeError::ProjectDirExists(_)
        | DocumentBridgeError::InvalidUuid(_, _) => Status::InvalidArg,
        _ => Status::GenericFailure,
    };
    NapiError::new(status, format!("kcreate_bridge: {e}"))
}

fn parse_uuid(s: &str) -> NapiResult<Uuid> {
    Uuid::from_str(s).map_err(|e| {
        NapiError::new(
            Status::InvalidArg,
            format!("kcreate_bridge: invalid uuid {s:?}: {e}"),
        )
    })
}

/// Renderer info returned from [`renderer_init`].
#[napi(object)]
#[derive(Debug, Clone)]
pub struct RendererInfo {
    pub tier: String,
    pub width: u32,
    pub height: u32,
}

impl From<CoreRendererInfo> for RendererInfo {
    fn from(c: CoreRendererInfo) -> Self {
        Self {
            tier: c.tier,
            width: c.width,
            height: c.height,
        }
    }
}

/// Initialize the offscreen renderer.
#[napi]
pub fn renderer_init(width: u32, height: u32) -> NapiResult<RendererInfo> {
    state::init(width, height).map(Into::into).map_err(map_err)
}

/// Tear down the renderer. Idempotent.
#[napi]
pub fn renderer_shutdown() {
    state::shutdown();
}

/// Resize the offscreen target.
#[napi]
pub fn renderer_resize(width: u32, height: u32) -> NapiResult<()> {
    state::resize(width, height).map_err(map_err)
}

/// Update viewport pan + zoom.
#[napi]
pub fn renderer_set_viewport(pan_x: f64, pan_y: f64, zoom: f64) -> NapiResult<()> {
    state::set_viewport(pan_x as f32, pan_y as f32, zoom as f32).map_err(map_err)
}

/// Mark a region (or the entire canvas, if no rect is supplied) as dirty.
#[napi]
pub fn renderer_invalidate(
    x: Option<f64>,
    y: Option<f64>,
    width: Option<f64>,
    height: Option<f64>,
) -> NapiResult<()> {
    let region = match (x, y, width, height) {
        (Some(x), Some(y), Some(w), Some(h)) => {
            Some(Rect::new(x as f32, y as f32, w as f32, h as f32))
        }
        _ => None,
    };
    state::invalidate(region).map_err(map_err)
}

/// Render a JS-supplied scene description.
///
/// The scene is encoded as JSON because:
///   1. The Electron renderer process produces scenes in TS/JS, so JSON
///      is the lowest-friction wire format.
///   2. A stable JSON schema keeps the bridge surface narrow and
///      versionable.
///   3. We can switch the encoding to `MessagePack` or a binary `FlatBuffer`
///      later without changing the renderer.
///
/// Returns the published `frameId`.
//
// Takes an owned `String` because that's the type `#[napi]` accepts for
// JS string arguments; we immediately borrow it when handing off to the
// JSON parser.
#[allow(clippy::needless_pass_by_value)]
#[napi]
pub fn renderer_render(scene_json: String) -> NapiResult<u32> {
    let id = state::render(&scene_json).map_err(map_err)?;
    Ok(id.0 as u32)
}

/// Returns the latest published frame as an RGBA8 `Buffer`, or `null`
/// if no frame has been rendered yet.
#[napi]
pub fn renderer_get_frame() -> NapiResult<Option<Buffer>> {
    let bytes = state::get_frame_bytes().map_err(map_err)?;
    Ok(bytes.map(Buffer::from))
}

/// Metadata about the latest published frame.
#[napi(object)]
#[derive(Debug, Clone)]
pub struct FrameInfo {
    pub frame_id: u32,
    pub width: u32,
    pub height: u32,
    pub byte_length: u32,
}

impl From<CoreFrameInfo> for FrameInfo {
    fn from(c: CoreFrameInfo) -> Self {
        Self {
            frame_id: c.frame_id as u32,
            width: c.width,
            height: c.height,
            byte_length: c.byte_length,
        }
    }
}

#[napi]
pub fn renderer_frame_info() -> NapiResult<Option<FrameInfo>> {
    state::get_frame_info()
        .map(|opt| opt.map(Into::into))
        .map_err(map_err)
}

/// Bytes + metadata for the latest published frame, atomically captured
/// under the renderer lock.
#[napi(object)]
#[derive(Clone)]
#[allow(missing_debug_implementations)] // `Buffer` from napi has no Debug impl.
pub struct AcquiredFrame {
    pub frame_id: u32,
    pub width: u32,
    pub height: u32,
    pub bytes: Buffer,
}

impl From<CoreAcquiredFrame> for AcquiredFrame {
    fn from(c: CoreAcquiredFrame) -> Self {
        Self {
            frame_id: c.frame_id as u32,
            width: c.width,
            height: c.height,
            bytes: Buffer::from(c.bytes),
        }
    }
}

/// One-call replacement for `renderer_get_frame` + `renderer_frame_info`.
///
/// Returns `null` if no frame has been published yet, otherwise an
/// `AcquiredFrame` whose `bytes`, `width`, `height`, and `frame_id`
/// all describe the exact same frame (i.e. cannot tear across a resize).
#[napi]
pub fn renderer_acquire_frame() -> NapiResult<Option<AcquiredFrame>> {
    state::acquire_frame()
        .map(|opt| opt.map(Into::into))
        .map_err(map_err)
}

// =============================================================================
// Document / project bridge
// =============================================================================

#[napi(object)]
#[derive(Debug, Clone)]
pub struct ProjectInfo {
    pub id: String,
    pub name: String,
    pub path: String,
    pub created_at: String,
    pub modified_at: String,
}

impl From<CoreProjectInfo> for ProjectInfo {
    fn from(c: CoreProjectInfo) -> Self {
        Self {
            id: c.id.to_string(),
            name: c.name,
            path: c.path.display().to_string(),
            created_at: c.created_at,
            modified_at: c.modified_at,
        }
    }
}

#[napi(object)]
#[derive(Debug, Clone)]
pub struct NodeInfo {
    pub id: String,
    pub node_type: String,
    pub parent_id: Option<String>,
    pub children: Vec<String>,
    pub name: String,
    pub visible: bool,
    pub locked: bool,
}

impl From<CoreNodeInfo> for NodeInfo {
    fn from(c: CoreNodeInfo) -> Self {
        Self {
            id: c.id.to_string(),
            node_type: c.node_type,
            parent_id: c.parent_id.map(|p| p.to_string()),
            children: c.children.iter().map(ToString::to_string).collect(),
            name: c.name,
            visible: c.visible,
            locked: c.locked,
        }
    }
}

#[napi(object)]
#[derive(Debug, Clone)]
pub struct RuntimeStatus {
    pub device_tier: String,
    pub gpu_available: bool,
    pub gpu_name: Option<String>,
    pub platform: String,
    /// Total system RAM, in megabytes. Carried over the N-API boundary
    /// as `f64` because JS `number` (an IEEE-754 double) can faithfully
    /// represent every integer up to 2⁵³ — i.e. ~9 PB worth of MB,
    /// which covers any plausible hardware. Using `u32` would silently
    /// cap at ~4 TB; `i64` would force the TS side into `BigInt` and
    /// break `RuntimeStatus: { totalRamMb: number }`.
    pub total_ram_mb: f64,
}

impl From<CoreRuntimeStatus> for RuntimeStatus {
    #[allow(
        clippy::cast_precision_loss,
        reason = "u64 → f64 is exact for values ≤ 2^53, which covers all realistic RAM sizes (2^53 MB ≈ 9 PB)"
    )]
    fn from(c: CoreRuntimeStatus) -> Self {
        Self {
            device_tier: c.device_tier,
            gpu_available: c.gpu_available,
            gpu_name: c.gpu_name,
            platform: c.platform,
            total_ram_mb: c.total_ram_mb as f64,
        }
    }
}

/// Editing-state snapshot the host UI reads to enable/disable
/// undo/redo controls without polling the entire layer tree.
#[napi(object)]
#[derive(Debug, Clone)]
pub struct DocumentStatus {
    pub node_count: u32,
    pub can_undo: bool,
    pub can_redo: bool,
    pub undo_depth: u32,
    pub redo_depth: u32,
}

impl From<document::DocumentStatus> for DocumentStatus {
    #[allow(clippy::cast_possible_truncation)]
    fn from(s: document::DocumentStatus) -> Self {
        // The host doesn't care about exact counts beyond a few
        // hundred (the bounded log max_depth defaults to 256); a
        // saturating cast is fine.
        Self {
            node_count: u32::try_from(s.node_count).unwrap_or(u32::MAX),
            can_undo: s.can_undo,
            can_redo: s.can_redo,
            undo_depth: u32::try_from(s.undo_depth).unwrap_or(u32::MAX),
            redo_depth: u32::try_from(s.redo_depth).unwrap_or(u32::MAX),
        }
    }
}

/// Create a new project under `dir/<name>.kstudio`.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn project_create(name: String, dir: String) -> NapiResult<ProjectInfo> {
    document::project_create(&name, &PathBuf::from(dir))
        .map(Into::into)
        .map_err(map_doc_err)
}

/// Open an existing `.kstudio` directory.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn project_open(dir: String) -> NapiResult<ProjectInfo> {
    document::project_open(&PathBuf::from(dir))
        .map(Into::into)
        .map_err(map_doc_err)
}

/// Persist the current project to disk.
#[napi]
pub fn project_save() -> NapiResult<()> {
    document::project_save().map_err(map_doc_err)
}

/// Close the current project, discarding unsaved in-memory state.
#[napi]
pub fn project_close() {
    document::project_close();
}

/// Identity snapshot of the open project, or `null` if none is open.
#[napi]
pub fn project_get_info() -> Option<ProjectInfo> {
    document::project_info().map(Into::into)
}

/// Flat document tree.
#[napi]
pub fn document_get_tree() -> NapiResult<Vec<NodeInfo>> {
    Ok(document::document_get_tree()
        .map_err(map_doc_err)?
        .into_iter()
        .map(Into::into)
        .collect())
}

/// Create a new node. `props_json` is a JSON object with optional
/// `name`, `visible`, `locked`, `metadata` fields. Returns the new id.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn document_create_node(
    node_type: String,
    parent_id: Option<String>,
    props_json: String,
) -> NapiResult<String> {
    let props: CreateNodeProps = serde_json::from_str(&props_json).map_err(|e| {
        NapiError::new(
            Status::InvalidArg,
            format!("kcreate_bridge: bad node props json: {e}"),
        )
    })?;
    let parent = match parent_id.as_deref() {
        Some(s) => Some(parse_uuid(s)?),
        None => None,
    };
    let id = document::document_create_node(&node_type, parent, &props).map_err(map_doc_err)?;
    Ok(id.to_string())
}

/// Update a node in place. `changes_json` is a JSON object with
/// optional `name`, `visible`, `locked`, `metadata` fields.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn document_update_node(node_id: String, changes_json: String) -> NapiResult<()> {
    let id = parse_uuid(&node_id)?;
    let changes: UpdateNodeProps = serde_json::from_str(&changes_json).map_err(|e| {
        NapiError::new(
            Status::InvalidArg,
            format!("kcreate_bridge: bad changes json: {e}"),
        )
    })?;
    document::document_update_node(id, &changes).map_err(map_doc_err)
}

/// Remove a node and its descendants.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn document_delete_node(node_id: String) -> NapiResult<()> {
    let id = parse_uuid(&node_id)?;
    document::document_delete_node(id).map_err(map_doc_err)
}

/// Undo last operation. Returns `null` when nothing to undo, otherwise
/// the list of affected node ids.
#[napi]
pub fn document_undo() -> NapiResult<Option<Vec<String>>> {
    let ids = document::document_undo().map_err(map_doc_err)?;
    Ok(ids.map(|v| v.into_iter().map(|u| u.to_string()).collect()))
}

#[napi]
pub fn document_redo() -> NapiResult<Option<Vec<String>>> {
    let ids = document::document_redo().map_err(map_doc_err)?;
    Ok(ids.map(|v| v.into_iter().map(|u| u.to_string()).collect()))
}

/// Static runtime / device snapshot.
#[napi]
pub fn runtime_status() -> RuntimeStatus {
    document::runtime_status().into()
}

/// Snapshot of the open document's editing state.
///
/// Returns `None` when no project is open. Hosts call this after any
/// mutation that may have changed undo/redo availability (project
/// open/close, create/update/delete node, record/undo/redo operation,
/// save).
#[napi]
pub fn document_status() -> Option<DocumentStatus> {
    document::document_status().map(Into::into)
}

// =============================================================================
// Export bridge
// =============================================================================

/// Export SVG for the given node ids (empty = whole document).
/// `options_json` is a JSON object with `width`, `height`,
/// `include_metadata`, `optimize` fields.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn export_svg(node_ids: Vec<String>, options_json: String) -> NapiResult<String> {
    let ids: Vec<Uuid> = node_ids
        .iter()
        .map(|s| parse_uuid(s))
        .collect::<NapiResult<_>>()?;
    let opts: SvgExportOptions = serde_json::from_str(&options_json).unwrap_or_default();
    document::export_svg(&ids, &opts).map_err(map_doc_err)
}

/// Export the current renderer scene to PNG at `output_path`. Returns
/// the file size in bytes.
///
/// Phase 0 deliberately omits a `node_ids` parameter: PNG export
/// rasterises the live renderer scene (held by `crate::state`), whose
/// id space is `u64`-keyed and disjoint from the document graph's
/// `Uuid`s. Per-node PNG export will land in Phase 1 alongside the
/// document→scene translator. SVG export, which walks the document
/// graph directly, *does* accept node ids today.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn export_png(output_path: String, options_json: String) -> NapiResult<u32> {
    let opts: CorePngRequest = serde_json::from_str(&options_json).map_err(|e| {
        NapiError::new(
            Status::InvalidArg,
            format!("kcreate_bridge: bad png options json: {e}"),
        )
    })?;
    let bytes =
        document::export_png_file(&PathBuf::from(output_path), &opts).map_err(map_doc_err)?;
    Ok(u32::try_from(bytes).unwrap_or(u32::MAX))
}
