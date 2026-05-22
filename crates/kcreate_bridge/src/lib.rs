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
pub mod hit_test;
pub mod llm;
pub mod scene_sync;
pub mod state;
pub mod wire;

use std::path::PathBuf;
use std::str::FromStr;

use kcreate_export::svg::SvgExportOptions;
use kcreate_renderer::Rect;
use napi::bindgen_prelude::{AsyncTask, Buffer, Error as NapiError, Result as NapiResult, Status};
use napi::{Env, Task};
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

/// Compute the three inspect-mode code outputs (CSS, Tailwind, and
/// React inline-style object literal) for `node_id`. Returns the
/// JSON-encoded `InspectCode` struct so the renderer can decode it
/// without bespoke type-mirroring.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn document_inspect_node(node_id: String) -> NapiResult<String> {
    let id = parse_uuid(&node_id)?;
    let code = document::document_inspect_node(id).map_err(map_doc_err)?;
    serde_json::to_string(&code)
        .map_err(|e| NapiError::from_reason(format!("document_inspect_node encode: {e}")))
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

/// True iff low-resource mode is currently active.
#[napi]
pub fn low_resource_mode_get() -> bool {
    document::low_resource_mode_get()
}

/// Toggle low-resource mode. Tier 0 hosts are pinned to `true`.
#[napi]
pub fn low_resource_mode_set(enabled: bool) {
    document::low_resource_mode_set(enabled);
}

/// JSON snapshot of the currently-effective resource limits.
///
/// The JSON shape mirrors [`document::ResourceLimits`] verbatim
/// (snake_case fields). Callers decode it on the TypeScript side.
#[napi]
pub fn resource_limits() -> NapiResult<String> {
    let limits = document::resource_limits();
    serde_json::to_string(&limits)
        .map_err(|e| NapiError::from_reason(format!("resource_limits: {e}")))
}

// =============================================================================
// LLM bridge
// =============================================================================

fn map_llm_err(e: llm::LlmBridgeError) -> NapiError {
    NapiError::new(Status::GenericFailure, e.to_string())
}

/// Start the LLM sidecar pointed at `model_path`. Returns the
/// loopback port on success.
#[napi]
pub fn llm_start(model_path: String) -> NapiResult<u32> {
    let port = llm::llm_start(PathBuf::from(model_path)).map_err(map_llm_err)?;
    Ok(u32::from(port))
}

/// Stop the LLM sidecar. Idempotent.
#[napi]
pub fn llm_stop() {
    llm::llm_stop();
}

/// JSON-encoded sidecar status.
#[napi]
pub fn llm_status() -> NapiResult<String> {
    serde_json::to_string(&llm::llm_status())
        .map_err(|e| NapiError::from_reason(format!("llm_status: {e}")))
}

// LLM chat/completion is a *blocking* HTTP round-trip to the local
// llama-server (up to a 60 s timeout in `chat_completion_impl`).
// Exposing it as a synchronous N-API function would block the
// Electron main process event loop for the duration — window
// dragging, menu clicks, and every other IPC queue would freeze.
// We wrap each completion call in `AsyncTask`, which dispatches the
// blocking work to N-API's libuv thread pool and resolves the
// returned JS Promise once the worker finishes. The renderer
// already awaits these results via `ipcRenderer.invoke`, so the
// wire format doesn't change.
//
// We deliberately do NOT use a Tokio runtime here: `ureq` (the
// llama-server client) is itself blocking, and a single-shot worker
// task per request is simpler than threading an async runtime
// through the LLM crate. If the LLM client ever switches to an
// async HTTP library, these tasks can move to `Env::execute_tokio_future`.

/// `napi::Task` for `llm_chat`. Owns the parsed messages so the
/// blocking HTTP call can run on a worker thread.
#[derive(Debug)]
pub struct LlmChatTask {
    messages: Vec<llm::LlmMessage>,
    max_tokens: usize,
    temperature: f32,
}

impl Task for LlmChatTask {
    type Output = String;
    type JsValue = String;

    fn compute(&mut self) -> NapiResult<Self::Output> {
        let reply = llm::llm_chat(
            std::mem::take(&mut self.messages),
            self.max_tokens,
            self.temperature,
        )
        .map_err(map_llm_err)?;
        serde_json::to_string(&reply)
            .map_err(|e| NapiError::from_reason(format!("llm_chat encode: {e}")))
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> NapiResult<Self::JsValue> {
        Ok(output)
    }
}

/// JSON-encoded chat completion. Input is a JSON array of
/// `{role, content}` objects. Resolves on a worker thread so the
/// Electron main loop stays responsive while llama-server runs
/// inference.
#[napi(ts_return_type = "Promise<string>")]
pub fn llm_chat(
    messages_json: String,
    max_tokens: u32,
    temperature: f64,
) -> NapiResult<AsyncTask<LlmChatTask>> {
    let messages: Vec<llm::LlmMessage> = serde_json::from_str(&messages_json)
        .map_err(|e| NapiError::new(Status::InvalidArg, format!("llm_chat messages: {e}")))?;
    #[allow(clippy::cast_possible_truncation)]
    Ok(AsyncTask::new(LlmChatTask {
        messages,
        max_tokens: max_tokens as usize,
        temperature: temperature as f32,
    }))
}

/// `napi::Task` for `llm_suggest_for_selection`.
#[derive(Debug)]
pub struct LlmSuggestForSelectionTask;

impl Task for LlmSuggestForSelectionTask {
    type Output = String;
    type JsValue = String;

    fn compute(&mut self) -> NapiResult<Self::Output> {
        let reply = llm::llm_suggest_for_selection().map_err(map_llm_err)?;
        serde_json::to_string(&reply)
            .map_err(|e| NapiError::from_reason(format!("llm_suggest encode: {e}")))
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> NapiResult<Self::JsValue> {
        Ok(output)
    }
}

/// JSON-encoded "suggest improvements" output for the current
/// selection or document. Runs on a worker thread.
#[napi(ts_return_type = "Promise<string>")]
pub fn llm_suggest_for_selection() -> AsyncTask<LlmSuggestForSelectionTask> {
    AsyncTask::new(LlmSuggestForSelectionTask)
}

/// `napi::Task` for `ai_suggest_layer_names`.
#[derive(Debug)]
pub struct AiSuggestLayerNamesTask;

impl Task for AiSuggestLayerNamesTask {
    type Output = String;
    type JsValue = String;

    fn compute(&mut self) -> NapiResult<Self::Output> {
        let res = llm::ai_suggest_layer_names().map_err(map_llm_err)?;
        serde_json::to_string(&res)
            .map_err(|e| NapiError::from_reason(format!("ai_suggest_layer_names encode: {e}")))
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> NapiResult<Self::JsValue> {
        Ok(output)
    }
}

/// Ask the LLM to propose semantic names for every layer. Returns a
/// JSON object: `{ suggestions: [[uuid, name], ...], raw_content,
/// tokens_used, model }`. Runs on a worker thread.
#[napi(ts_return_type = "Promise<string>")]
pub fn ai_suggest_layer_names() -> AsyncTask<AiSuggestLayerNamesTask> {
    AsyncTask::new(AiSuggestLayerNamesTask)
}

/// `napi::Task` for `ai_extract_design_tokens`.
#[derive(Debug)]
pub struct AiExtractDesignTokensTask;

impl Task for AiExtractDesignTokensTask {
    type Output = String;
    type JsValue = String;

    fn compute(&mut self) -> NapiResult<Self::Output> {
        let res = llm::ai_extract_design_tokens().map_err(map_llm_err)?;
        serde_json::to_string(&res)
            .map_err(|e| NapiError::from_reason(format!("ai_extract_design_tokens encode: {e}")))
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> NapiResult<Self::JsValue> {
        Ok(output)
    }
}

/// Ask the LLM to extract design tokens. Returns
/// `{ json, tokens_used, model }` where `json` is the model's reply
/// in the schema described by `build_design_token_prompt`. Runs on a
/// worker thread.
#[napi(ts_return_type = "Promise<string>")]
pub fn ai_extract_design_tokens() -> AsyncTask<AiExtractDesignTokensTask> {
    AsyncTask::new(AiExtractDesignTokensTask)
}

/// `napi::Task` for `ai_check_accessibility`.
#[derive(Debug)]
pub struct AiCheckAccessibilityTask;

impl Task for AiCheckAccessibilityTask {
    type Output = String;
    type JsValue = String;

    fn compute(&mut self) -> NapiResult<Self::Output> {
        let res = llm::ai_check_accessibility().map_err(map_llm_err)?;
        serde_json::to_string(&res)
            .map_err(|e| NapiError::from_reason(format!("ai_check_accessibility encode: {e}")))
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> NapiResult<Self::JsValue> {
        Ok(output)
    }
}

/// Ask the LLM to audit the document for accessibility issues.
/// Returns `{ json, tokens_used, model }`. Runs on a worker thread.
#[napi(ts_return_type = "Promise<string>")]
pub fn ai_check_accessibility() -> AsyncTask<AiCheckAccessibilityTask> {
    AsyncTask::new(AiCheckAccessibilityTask)
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
///
/// Malformed JSON is rejected with `Status::InvalidArg`, matching
/// [`export_png`]'s contract. `ANALYSIS_0002` on PR #2 flagged the
/// previous `unwrap_or_default()` path: a typo in `optionsJson` would
/// silently fall back to the SVG default viewport, producing a valid
/// but unexpectedly-sized file rather than surfacing the host-side
/// bug. Both export entry points now propagate JSON errors identically.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn export_svg(node_ids: Vec<String>, options_json: String) -> NapiResult<String> {
    let ids: Vec<Uuid> = node_ids
        .iter()
        .map(|s| parse_uuid(s))
        .collect::<NapiResult<_>>()?;
    let opts: SvgExportOptions = serde_json::from_str(&options_json).map_err(|e| {
        NapiError::new(
            Status::InvalidArg,
            format!("kcreate_bridge: bad svg options json: {e}"),
        )
    })?;
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

/// Export the open document to PDF at `output_path`. Returns the file
/// size in bytes. `options_json` accepts `{ width_mm, height_mm, title? }`.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn export_pdf(output_path: String, options_json: String) -> NapiResult<u32> {
    let opts: document::PdfExportRequest = serde_json::from_str(&options_json).map_err(|e| {
        NapiError::new(
            Status::InvalidArg,
            format!("kcreate_bridge: bad pdf options json: {e}"),
        )
    })?;
    let bytes =
        document::export_pdf_file(&PathBuf::from(output_path), &opts).map_err(map_doc_err)?;
    Ok(u32::try_from(bytes).unwrap_or(u32::MAX))
}

/// Render the current renderer scene to WebP at `output_path`. Returns
/// the file size in bytes.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn export_webp(output_path: String, options_json: String) -> NapiResult<u32> {
    let opts: document::WebpExportRequest = serde_json::from_str(&options_json).map_err(|e| {
        NapiError::new(
            Status::InvalidArg,
            format!("kcreate_bridge: bad webp options json: {e}"),
        )
    })?;
    let bytes =
        document::export_webp_file(&PathBuf::from(output_path), &opts).map_err(map_doc_err)?;
    Ok(u32::try_from(bytes).unwrap_or(u32::MAX))
}

/// Render the current renderer scene to JPEG at `output_path`. Returns
/// the file size in bytes.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn export_jpeg(output_path: String, options_json: String) -> NapiResult<u32> {
    let opts: document::JpegExportRequest = serde_json::from_str(&options_json).map_err(|e| {
        NapiError::new(
            Status::InvalidArg,
            format!("kcreate_bridge: bad jpeg options json: {e}"),
        )
    })?;
    let bytes =
        document::export_jpeg_file(&PathBuf::from(output_path), &opts).map_err(map_doc_err)?;
    Ok(u32::try_from(bytes).unwrap_or(u32::MAX))
}

// =============================================================================
// Canvas / scene synchronisation
// =============================================================================

/// Force a scene re-sync. The host calls this after re-initialising the
/// renderer (resize, tier change) when it wants to immediately repaint
/// the document instead of waiting for the next mutation.
#[napi]
pub fn document_sync_scene() -> NapiResult<()> {
    document::document_sync_scene().map_err(map_doc_err)
}

/// Hit-test viewport-relative screen coordinates against the current
/// scene. Returns the topmost selectable node's uuid as a string, or
/// `null` when the cursor is over empty canvas.
#[napi]
pub fn canvas_hit_test(
    x: f64,
    y: f64,
    pan_x: f64,
    pan_y: f64,
    zoom: f64,
) -> NapiResult<Option<String>> {
    let hit =
        document::canvas_hit_test(x as f32, y as f32, pan_x as f32, pan_y as f32, zoom as f32)
            .map_err(map_doc_err)?;
    Ok(hit.map(|u| u.to_string()))
}

/// Replace the document selection. Unknown node ids are silently dropped.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn document_set_selection(node_ids: Vec<String>) -> NapiResult<()> {
    let ids: Vec<Uuid> = node_ids
        .iter()
        .map(|s| parse_uuid(s))
        .collect::<NapiResult<_>>()?;
    document::document_set_selection(ids).map_err(map_doc_err)
}

/// Snapshot of the current selection.
#[napi]
pub fn document_get_selection() -> NapiResult<Vec<String>> {
    document::document_get_selection()
        .map(|v| v.into_iter().map(|u| u.to_string()).collect())
        .map_err(map_doc_err)
}

/// Clear the selection.
#[napi]
pub fn document_clear_selection() -> NapiResult<()> {
    document::document_clear_selection().map_err(map_doc_err)
}

/// Create a rectangle vector layer covering `(x, y, w, h)` in world
/// coordinates. Returns the new node's uuid.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn canvas_create_rect(
    parent_id: Option<String>,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
) -> NapiResult<String> {
    let parent = match parent_id.as_deref() {
        Some(s) => Some(parse_uuid(s)?),
        None => None,
    };
    document::canvas_create_rect(parent, x, y, w, h)
        .map(|u| u.to_string())
        .map_err(map_doc_err)
}

/// Create an ellipse vector layer.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn canvas_create_ellipse(
    parent_id: Option<String>,
    cx: f64,
    cy: f64,
    rx: f64,
    ry: f64,
) -> NapiResult<String> {
    let parent = match parent_id.as_deref() {
        Some(s) => Some(parse_uuid(s)?),
        None => None,
    };
    document::canvas_create_ellipse(parent, cx, cy, rx, ry)
        .map(|u| u.to_string())
        .map_err(map_doc_err)
}

/// Create a line vector layer from `(x1, y1)` to `(x2, y2)`.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn canvas_create_line(
    parent_id: Option<String>,
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
) -> NapiResult<String> {
    let parent = match parent_id.as_deref() {
        Some(s) => Some(parse_uuid(s)?),
        None => None,
    };
    document::canvas_create_line(parent, x1, y1, x2, y2)
        .map(|u| u.to_string())
        .map_err(map_doc_err)
}

/// Translate a node by `(dx, dy)` in world coordinates. Records an
/// undoable operation.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn canvas_move_node(node_id: String, dx: f64, dy: f64) -> NapiResult<()> {
    let id = parse_uuid(&node_id)?;
    document::canvas_move_node(id, dx, dy).map_err(map_doc_err)
}

/// Create a text layer at `(x, y)` with the given content + font.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn canvas_create_text(
    parent_id: Option<String>,
    x: f64,
    y: f64,
    text: String,
    font_family: String,
    font_size: f64,
) -> NapiResult<String> {
    let parent = match parent_id.as_deref() {
        Some(s) => Some(parse_uuid(s)?),
        None => None,
    };
    document::canvas_create_text(parent, x, y, text, font_family, font_size as f32)
        .map(|u| u.to_string())
        .map_err(map_doc_err)
}

/// Import a raster image from disk into the project. The image bytes
/// are stored as a content-addressed blob and a `RasterLayer` node is
/// inserted referencing it. Returns the new node's uuid.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn document_import_image(parent_id: Option<String>, file_path: String) -> NapiResult<String> {
    let parent = match parent_id.as_deref() {
        Some(s) => Some(parse_uuid(s)?),
        None => None,
    };
    document::document_import_image(parent, &PathBuf::from(file_path))
        .map(|u| u.to_string())
        .map_err(map_doc_err)
}

// =============================================================================
// AI / MCP
// =============================================================================

/// Run local-CPU background removal on a `RasterLayer` node.
///
/// Creates a new `RasterLayer` with the resulting transparent image,
/// records an AI operation in the project log (so undo restores the
/// original), and returns the new node's uuid.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn ai_remove_background(node_id: String) -> NapiResult<String> {
    let id = parse_uuid(&node_id)?;
    document::ai_remove_background(id)
        .map(|u| u.to_string())
        .map_err(map_doc_err)
}

/// Returns the AI action log as a JSON array.
#[napi]
pub fn ai_get_action_log() -> NapiResult<String> {
    document::ai_get_action_log().map_err(map_doc_err)
}

/// Start the local MCP server on loopback. Returns the bound port.
/// The server is opt-in (the `mcp` cargo feature must be enabled at
/// bridge build time) and bound to 127.0.0.1 — never the public
/// network.
#[napi]
pub fn mcp_start() -> NapiResult<u32> {
    document::mcp_start().map_err(map_doc_err)
}

/// Stop the local MCP server. Idempotent.
#[napi]
pub fn mcp_stop() -> NapiResult<()> {
    document::mcp_stop().map_err(map_doc_err)
}

/// Returns true when the MCP server is bound and accepting requests.
#[napi]
pub fn mcp_is_running() -> bool {
    document::mcp_is_running()
}

// -----------------------------------------------------------------------------
// Design tokens / brand kits / export presets (Task 19)
// -----------------------------------------------------------------------------

/// Return the current project's design tokens as a JSON object.
#[napi]
pub fn design_tokens_get() -> NapiResult<String> {
    let tokens = document::design_tokens_get().map_err(map_doc_err)?;
    serde_json::to_string(&tokens).map_err(|e| NapiError::from_reason(e.to_string()))
}

/// Replace the design-tokens bag from JSON. Persisted on the next
/// `project_save`.
#[napi]
pub fn design_tokens_set(tokens_json: String) -> NapiResult<()> {
    let tokens: kcreate_core::project::DesignTokens =
        serde_json::from_str(&tokens_json).map_err(|e| NapiError::from_reason(e.to_string()))?;
    document::design_tokens_set(tokens).map_err(map_doc_err)
}

/// Create a new brand kit with the given display name. Returns the
/// new kit's UUID as a string.
#[napi]
pub fn brand_kit_create(name: String) -> NapiResult<String> {
    document::brand_kit_create(name)
        .map(|id| id.to_string())
        .map_err(map_doc_err)
}

/// Replace an existing brand kit. The kit's id field is the key.
#[napi]
pub fn brand_kit_update(kit_json: String) -> NapiResult<()> {
    let kit: kcreate_core::project::BrandKit =
        serde_json::from_str(&kit_json).map_err(|e| NapiError::from_reason(e.to_string()))?;
    document::brand_kit_update(kit).map_err(map_doc_err)
}

/// List every brand kit in the project as a JSON array.
#[napi]
pub fn brand_kit_list() -> NapiResult<String> {
    let kits = document::brand_kit_list().map_err(map_doc_err)?;
    serde_json::to_string(&kits).map_err(|e| NapiError::from_reason(e.to_string()))
}

/// Delete a brand kit by id. Returns true when something was
/// removed; false if no matching kit existed.
#[napi]
pub fn brand_kit_delete(kit_id: String) -> NapiResult<bool> {
    let id = parse_uuid(&kit_id)?;
    document::brand_kit_delete(id).map_err(map_doc_err)
}

/// Create a new export preset. `format` is one of `png` / `svg` /
/// `pdf` / `webp` / `jpeg`. Returns the new preset's UUID.
#[napi]
pub fn export_preset_create(name: String, format: String, scale: f64) -> NapiResult<String> {
    document::export_preset_create(name, &format, scale as f32)
        .map(|id| id.to_string())
        .map_err(map_doc_err)
}

/// List every export preset in the project as a JSON array.
#[napi]
pub fn export_preset_list() -> NapiResult<String> {
    let presets = document::export_preset_list().map_err(map_doc_err)?;
    serde_json::to_string(&presets).map_err(|e| NapiError::from_reason(e.to_string()))
}

/// Delete an export preset by id. Returns true when something was removed.
#[napi]
pub fn export_preset_delete(preset_id: String) -> NapiResult<bool> {
    let id = parse_uuid(&preset_id)?;
    document::export_preset_delete(id).map_err(map_doc_err)
}

// ---------------------------------------------------------------------------
// Artboards
// ---------------------------------------------------------------------------

/// Create a new artboard. `page_id` may be `None`/empty to attach to
/// (or create) the first Page in the project. Returns the new
/// artboard's UUID.
#[napi]
pub fn artboard_create(
    page_id: Option<String>,
    name: String,
    width: f64,
    height: f64,
) -> NapiResult<String> {
    let parent = match page_id.as_deref() {
        Some(s) if !s.is_empty() => Some(parse_uuid(s)?),
        _ => None,
    };
    document::artboard_create(parent, name, width, height)
        .map(|id| id.to_string())
        .map_err(map_doc_err)
}

/// List every artboard in the project as a JSON array.
#[napi]
pub fn artboard_list() -> NapiResult<String> {
    let infos = document::artboard_list().map_err(map_doc_err)?;
    serde_json::to_string(&infos).map_err(|e| NapiError::from_reason(e.to_string()))
}

/// Deep-clone an artboard and all its descendants. Returns the new
/// artboard's UUID.
#[napi]
pub fn artboard_duplicate(artboard_id: String) -> NapiResult<String> {
    let id = parse_uuid(&artboard_id)?;
    document::artboard_duplicate(id)
        .map(|new_id| new_id.to_string())
        .map_err(map_doc_err)
}

/// Resize the artboard's bounds (width/height). The (x, y) corner is
/// preserved.
#[napi]
pub fn artboard_resize(artboard_id: String, width: f64, height: f64) -> NapiResult<()> {
    let id = parse_uuid(&artboard_id)?;
    document::artboard_resize(id, width, height).map_err(map_doc_err)
}

/// Return the built-in artboard preset catalogue as a JSON array.
#[napi]
pub fn artboard_presets() -> NapiResult<String> {
    let presets = document::artboard_presets();
    serde_json::to_string(&presets).map_err(|e| NapiError::from_reason(e.to_string()))
}

// -----------------------------------------------------------------------------
// Components (Block B)
// -----------------------------------------------------------------------------

/// Convert a selection of nodes into a reusable component. Returns
/// the new component's UUID.
#[napi]
pub fn component_create_from_selection(node_ids: Vec<String>, name: String) -> NapiResult<String> {
    let mut parsed = Vec::with_capacity(node_ids.len());
    for s in node_ids {
        parsed.push(parse_uuid(&s)?);
    }
    document::component_create_from_selection(parsed, name)
        .map(|id| id.to_string())
        .map_err(map_doc_err)
}

/// List every registered component as a JSON array.
#[napi]
pub fn component_list() -> NapiResult<String> {
    let list = document::component_list().map_err(map_doc_err)?;
    serde_json::to_string(&list).map_err(|e| NapiError::from_reason(e.to_string()))
}

/// Instantiate a component at `(x, y)` under `parent_id`. Returns
/// the new ComponentLayer node's UUID.
#[napi]
pub fn component_instantiate(
    component_id: String,
    parent_id: Option<String>,
    x: f64,
    y: f64,
) -> NapiResult<String> {
    let cid = parse_uuid(&component_id)?;
    let parent = match parent_id {
        Some(s) if !s.is_empty() => Some(parse_uuid(&s)?),
        _ => None,
    };
    document::component_instantiate(cid, parent, x, y)
        .map(|id| id.to_string())
        .map_err(map_doc_err)
}

/// Append a fresh variant to a component. Returns the new variant's
/// UUID.
#[napi]
pub fn component_add_variant(component_id: String, name: String) -> NapiResult<String> {
    let cid = parse_uuid(&component_id)?;
    document::component_add_variant(cid, name)
        .map(|id| id.to_string())
        .map_err(map_doc_err)
}

/// Switch the active variant of a component instance node.
#[napi]
pub fn component_switch_variant(node_id: String, variant_id: String) -> NapiResult<()> {
    let nid = parse_uuid(&node_id)?;
    let vid = parse_uuid(&variant_id)?;
    document::component_switch_variant(nid, vid).map_err(map_doc_err)
}

/// Detach a component instance — converts the ComponentLayer into a
/// regular GroupLayer.
#[napi]
pub fn component_detach(node_id: String) -> NapiResult<()> {
    let nid = parse_uuid(&node_id)?;
    document::component_detach(nid).map_err(map_doc_err)
}

// -----------------------------------------------------------------------------
// Auto-layout (Block C)
// -----------------------------------------------------------------------------

/// Write a `FlexLayout` config (JSON) onto the given LayoutFrame.
#[napi]
pub fn layout_set_flex(node_id: String, layout_json: String) -> NapiResult<()> {
    let nid = parse_uuid(&node_id)?;
    let cfg: kcreate_layout::FlexLayout = serde_json::from_str(&layout_json)
        .map_err(|e| NapiError::from_reason(format!("layout json: {e}")))?;
    document::layout_set_flex(nid, cfg).map_err(map_doc_err)
}

/// Write a `GridLayout` config (JSON) onto the given LayoutFrame.
#[napi]
pub fn layout_set_grid(node_id: String, layout_json: String) -> NapiResult<()> {
    let nid = parse_uuid(&node_id)?;
    let cfg: kcreate_layout::GridLayout = serde_json::from_str(&layout_json)
        .map_err(|e| NapiError::from_reason(format!("layout json: {e}")))?;
    document::layout_set_grid(nid, cfg).map_err(map_doc_err)
}

/// Recompute child positions for a LayoutFrame from its layout config.
#[napi]
pub fn layout_recompute(node_id: String) -> NapiResult<()> {
    let nid = parse_uuid(&node_id)?;
    document::layout_recompute(nid).map_err(map_doc_err)
}

/// Convert a GroupLayer node into a LayoutFrame so it can carry an
/// auto-layout config. No-op for already-LayoutFrame nodes.
#[napi]
pub fn layout_convert_to_frame(node_id: String) -> NapiResult<()> {
    let nid = parse_uuid(&node_id)?;
    document::layout_convert_to_frame(nid).map_err(map_doc_err)
}

// -----------------------------------------------------------------------------
// Prototype interactions (Block A)
// -----------------------------------------------------------------------------

/// Add an interaction to a node. Returns the new interaction's id.
/// `trigger` is `"click"` / `"hover"` / `"press"`; `action_json` is a
/// serialised [`kcreate_core::InteractionAction`].
#[napi]
pub fn interaction_add(
    node_id: String,
    trigger: String,
    action_json: String,
) -> NapiResult<String> {
    let nid = parse_uuid(&node_id)?;
    document::interaction_add(nid, &trigger, &action_json)
        .map(|id| id.to_string())
        .map_err(map_doc_err)
}

/// Remove an interaction from a node. Returns `true` when an
/// interaction with the given id was removed.
#[napi]
pub fn interaction_remove(node_id: String, interaction_id: String) -> NapiResult<bool> {
    let nid = parse_uuid(&node_id)?;
    let iid = parse_uuid(&interaction_id)?;
    document::interaction_remove(nid, iid).map_err(map_doc_err)
}

/// List interactions on a node. Returns a JSON array of [`kcreate_core::Interaction`].
#[napi]
pub fn interaction_list(node_id: String) -> NapiResult<String> {
    let nid = parse_uuid(&node_id)?;
    let list = document::interaction_list(nid).map_err(map_doc_err)?;
    serde_json::to_string(&list).map_err(|e| NapiError::from_reason(e.to_string()))
}

// -----------------------------------------------------------------------------
// Layout Studio (Block B): page layout, master pages, templates
// -----------------------------------------------------------------------------

/// Write the `PageLayout` JSON onto a Page node.
#[napi]
pub fn page_set_layout(page_id: String, layout_json: String) -> NapiResult<()> {
    let pid = parse_uuid(&page_id)?;
    document::page_set_layout(pid, &layout_json).map_err(map_doc_err)
}

/// Read the `PageLayout` JSON on a Page node. Returns the empty string
/// when no layout is attached.
#[napi]
pub fn page_get_layout(page_id: String) -> NapiResult<String> {
    let pid = parse_uuid(&page_id)?;
    let layout = document::page_get_layout(pid).map_err(map_doc_err)?;
    match layout {
        Some(l) => serde_json::to_string(&l).map_err(|e| NapiError::from_reason(e.to_string())),
        None => Ok(String::new()),
    }
}

/// Create a new master page. `size` ∈ {a3, a4, a5, letter, legal,
/// tabloid, presentation_16x9, presentation_4x3}. `orientation` ∈
/// {portrait, landscape}. Returns the new page id.
#[napi]
pub fn master_page_create(name: String, size: String, orientation: String) -> NapiResult<String> {
    document::master_page_create(name, &size, &orientation)
        .map(|id| id.to_string())
        .map_err(map_doc_err)
}

/// List every master page as a JSON array.
#[napi]
pub fn master_page_list() -> NapiResult<String> {
    let list = document::master_page_list().map_err(map_doc_err)?;
    serde_json::to_string(&list).map_err(|e| NapiError::from_reason(e.to_string()))
}

/// Attach a master to a content page.
#[napi]
pub fn master_page_apply(content_page_id: String, master_page_id: String) -> NapiResult<()> {
    let cid = parse_uuid(&content_page_id)?;
    let mid = parse_uuid(&master_page_id)?;
    document::master_page_apply(cid, mid).map_err(map_doc_err)
}

/// Clear the master reference on a content page.
#[napi]
pub fn master_page_detach(content_page_id: String) -> NapiResult<()> {
    let cid = parse_uuid(&content_page_id)?;
    document::master_page_detach(cid).map_err(map_doc_err)
}

/// Return the built-in layout-template catalog as a JSON array.
#[napi]
pub fn layout_template_list() -> NapiResult<String> {
    let templates = document::layout_template_list();
    serde_json::to_string(&templates).map_err(|e| NapiError::from_reason(e.to_string()))
}

/// Apply a built-in template by id. Returns a JSON array of created
/// page uuids.
#[napi]
pub fn layout_template_apply(template_id: String) -> NapiResult<String> {
    let tid = parse_uuid(&template_id)?;
    let ids = document::layout_template_apply(tid).map_err(map_doc_err)?;
    let strs: Vec<String> = ids.into_iter().map(|i| i.to_string()).collect();
    serde_json::to_string(&strs).map_err(|e| NapiError::from_reason(e.to_string()))
}
