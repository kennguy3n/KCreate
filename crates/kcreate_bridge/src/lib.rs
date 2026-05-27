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

pub mod audit;
#[cfg(feature = "collab")]
pub mod collab;
pub mod document;
pub mod hit_test;
pub mod llm;
#[cfg(feature = "native_canvas")]
pub mod native_canvas;
pub mod phase2;
pub mod phase4;
pub mod raster_ops;
pub mod scene_sync;
pub mod state;
pub mod vector_ops;
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
    BoundsInfo as CoreBoundsInfo, CreateNodeProps, DocumentBridgeError, NodeInfo as CoreNodeInfo,
    PngExportRequest as CorePngRequest, ProjectInfo as CoreProjectInfo,
    RuntimeStatus as CoreRuntimeStatus, UndoRedoOutcome as CoreUndoRedoOutcome, UpdateNodeProps,
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
    let status = match &e {
        DocumentBridgeError::NoProject
        | DocumentBridgeError::InvalidNodeType(_)
        | DocumentBridgeError::InvalidArgument { .. }
        | DocumentBridgeError::NodeNotFound(_)
        | DocumentBridgeError::ProjectDirExists(_)
        | DocumentBridgeError::InvalidUuid(_, _) => Status::InvalidArg,
        // Marketplace errors that come from a user-supplied template
        // path / id are user-correctable (bad path, wrong id, duplicate
        // install) — surface as InvalidArg so the renderer can show
        // them inline next to the offending control. Underlying IO
        // failures (disk full, permission denied) stay GenericFailure.
        DocumentBridgeError::Marketplace(me) => match me {
            kcreate_core::MarketplaceError::DirectoryNotFound(_)
            | kcreate_core::MarketplaceError::ManifestParse { .. }
            | kcreate_core::MarketplaceError::TemplateNotFound(_)
            | kcreate_core::MarketplaceError::AlreadyInstalled(_) => Status::InvalidArg,
            kcreate_core::MarketplaceError::Io(_) => Status::GenericFailure,
        },
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
// Native canvas presentation mode — Phase 1, Block A, Task 4–6.
//
// The N-API surface for `presentation_mode` / `switch_native` /
// `switch_offscreen` is always exported (so the host code does not
// have to branch on which Cargo features the cdylib was built with):
// in default builds `presentation_mode` always returns "offscreen",
// `switch_offscreen` is a no-op, and `switch_native` errors with a
// clear "feature not compiled in" message that the renderer can
// surface as a fallback.
// =============================================================================

/// Returns the current presentation mode (`"offscreen"` or `"native"`).
///
/// `"offscreen"` means the host should drive the `requestAnimationFrame`
/// readback loop via `renderer.acquireFrame()`. `"native"` means the
/// Rust renderer is presenting directly to a platform window surface
/// and the host should hide its canvas element.
#[napi]
#[must_use]
pub fn renderer_presentation_mode() -> String {
    state::presentation_mode().as_str().to_string()
}

/// Attach a native presentation surface created from the raw bytes
/// returned by Electron's `BrowserWindow::getNativeWindowHandle()`.
///
/// `width` / `height` are the surface's physical pixel dimensions
/// (caller is responsible for multiplying CSS pixels by
/// `devicePixelRatio`). Returns the platform variant the bridge
/// interpreted the bytes as (`"appkit"`, `"win32"`, `"x11"`, or
/// `"wayland"`).
///
/// Errors with a `feature not compiled in` message in default builds
/// (the `native_canvas` feature flag gates the platform-specific
/// `raw_window_handle` interpretation). The host should treat that
/// error as a signal to remain in offscreen mode.
#[napi]
#[allow(clippy::needless_pass_by_value, unused_variables)]
pub fn renderer_switch_native(handle_bytes: Buffer, width: u32, height: u32) -> NapiResult<String> {
    #[cfg(feature = "native_canvas")]
    {
        state::switch_native(handle_bytes.as_ref(), width, height).map_err(map_err)
    }
    #[cfg(not(feature = "native_canvas"))]
    {
        Err(napi::Error::from_reason(
            "renderer_switch_native: bridge was compiled without the `native_canvas` feature; \
             cannot interpret the platform window handle. Stay on the offscreen path."
                .to_string(),
        ))
    }
}

/// Detach any attached native surface and revert to the offscreen
/// readback path. No-op when already in offscreen mode (or when the
/// `native_canvas` feature is not compiled in).
#[napi]
pub fn renderer_switch_offscreen() {
    #[cfg(feature = "native_canvas")]
    {
        state::switch_offscreen();
    }
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

/// Wire-format mirror of [`kcreate_core::Bounds`] / [`document::BoundsInfo`].
///
/// Kept as a flat `#[napi(object)]` shape so the host can read `bounds`
/// from any `NodeInfo` without a second IPC hop. The four numbers
/// represent the axis-aligned bounding box in document space (CSS-like
/// `x, y, width, height` in document units / px).
#[napi(object)]
#[derive(Debug, Clone, Copy)]
pub struct Bounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl From<CoreBoundsInfo> for Bounds {
    fn from(b: CoreBoundsInfo) -> Self {
        Self {
            x: b.x,
            y: b.y,
            width: b.width,
            height: b.height,
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
    /// Axis-aligned bounding box in document space. Mirrors
    /// `kcreate_core::Node::bounds`. The host's PrototypePlayer
    /// (Block A, Task 2) uses this to position hotspot rectangles on
    /// top of the rendered artboard; previously the wire shape elided
    /// `bounds`, so hotspots never appeared — see PR #5 fix.
    pub bounds: Bounds,
    /// Monotonically-increasing revision counter. Mirrors
    /// `kcreate_core::node::Node::version`. Used by renderer panels
    /// (`FillSection`, `TextFramePanel`, `OpenTypePanel`) as a
    /// dependency-array signal so their hydrate `useEffect` refires
    /// after undo/redo / collab edits on the same node id. Carried as
    /// `f64` because JS `number` (IEEE-754 double) can faithfully
    /// represent every integer up to 2^53 — `version` increments
    /// once per mutation so even a million edits per second for 100
    /// years stays well within range. Using `BigInt` would force
    /// every renderer panel onto `Number(node.version)` conversions
    /// and break the existing `NodeInfo: { version: number }` shape.
    pub version: f64,
}

impl From<CoreNodeInfo> for NodeInfo {
    #[allow(
        clippy::cast_precision_loss,
        reason = "u64 → f64 is exact for values ≤ 2^53; Node::version bumps once per mutation so any realistic editing session stays well under that bound"
    )]
    fn from(c: CoreNodeInfo) -> Self {
        Self {
            id: c.id.to_string(),
            node_type: c.node_type,
            parent_id: c.parent_id.map(|p| p.to_string()),
            children: c.children.iter().map(ToString::to_string).collect(),
            name: c.name,
            visible: c.visible,
            locked: c.locked,
            bounds: c.bounds.into(),
            version: c.version as f64,
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

/// Returns `true` iff the currently open project is untouched —
/// i.e. its [`OperationLog`](kcreate_core::operation::OperationLog)
/// is empty. The host uses this to drive first-time UX (e.g.
/// auto-opening the TemplatePicker on the first switch to Layout
/// mode) without replicating `project_create`'s exact node shape
/// in TypeScript. Errors with `NoProject` if no project is open.
#[napi]
pub fn project_is_untouched() -> NapiResult<bool> {
    document::project_is_untouched().map_err(map_doc_err)
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
/// optional `name`, `visible`, `locked`, `metadata`, and `fill`
/// fields. The `fill` field, when present, is the `kind`-tagged
/// JSON shape of [`kcreate_core::node::FillStyle`] — see
/// [`crate::document::UpdateNodeProps::fill`] for the contract.
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

/// Read the current `FillStyle` for a node, returned as a serialised
/// JSON string. Returns `null` when the node id is not in the
/// document. The renderer-side `FillSection` panel uses this to
/// populate its form on selection change; edits go back through
/// [`document_update_node`] with the new `fill` field.
///
/// String-typed rather than typed because `FillStyle` is a
/// tagged-enum (`kind` discriminator) that napi-rs can't faithfully
/// mirror without a hand-rolled wire-format struct per variant — and
/// the renderer needs the round-trippable JSON shape anyway.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn document_node_fill(node_id: String) -> NapiResult<Option<String>> {
    let id = parse_uuid(&node_id)?;
    document::document_node_fill(id).map_err(map_doc_err)
}

/// Read the node's `extra_fills` stack as a JSON array. Returns
/// `None` when the node id is unknown.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn document_node_extra_fills(node_id: String) -> NapiResult<Option<String>> {
    let id = parse_uuid(&node_id)?;
    document::document_node_extra_fills(id).map_err(map_doc_err)
}

/// Read the node's `extra_strokes` stack as a JSON array.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn document_node_extra_strokes(node_id: String) -> NapiResult<Option<String>> {
    let id = parse_uuid(&node_id)?;
    document::document_node_extra_strokes(id).map_err(map_doc_err)
}

/// Remove a node and its descendants.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn document_delete_node(node_id: String) -> NapiResult<()> {
    let id = parse_uuid(&node_id)?;
    document::document_delete_node(id).map_err(map_doc_err)
}

/// Wire-format mirror of [`document::UndoRedoOutcome`].
///
/// Both `command` and `affectedNodes` are returned on the same hop so
/// the host can gate per-operation side-effects (e.g. the
/// `kcreate/color/settings/changed` broadcast, which only needs to
/// fire when `command == "color_settings_update"`) without a second
/// IPC round-trip into Rust.
#[napi(object)]
#[derive(Debug, Clone)]
pub struct UndoRedoOutcome {
    /// Stable command string from `Operation::command`, e.g.
    /// `"color_settings_update"` or `"document_update_node"`.
    pub command: String,
    /// `Operation::affected_nodes` serialized to strings. Empty for
    /// non-graph operations like `color_settings_update`.
    pub affected_nodes: Vec<String>,
}

impl From<CoreUndoRedoOutcome> for UndoRedoOutcome {
    fn from(c: CoreUndoRedoOutcome) -> Self {
        Self {
            command: c.command,
            affected_nodes: c
                .affected_nodes
                .into_iter()
                .map(|u| u.to_string())
                .collect(),
        }
    }
}

/// Undo last operation. Returns `null` when nothing to undo, otherwise
/// an `UndoRedoOutcome` carrying the operation's `command` and
/// `affectedNodes`.
#[napi]
pub fn document_undo() -> NapiResult<Option<UndoRedoOutcome>> {
    let outcome = document::document_undo().map_err(map_doc_err)?;
    Ok(outcome.map(Into::into))
}

#[napi]
pub fn document_redo() -> NapiResult<Option<UndoRedoOutcome>> {
    let outcome = document::document_redo().map_err(map_doc_err)?;
    Ok(outcome.map(Into::into))
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

/// Force a re-publish of the current scene without taking a
/// document lock for mutation. Used by the Phase 3 collab IPC
/// tick to refresh remote-peer cursor overlays after presence
/// updates arrive. Safe to call even when no project is loaded
/// (returns `Ok` and does nothing).
#[napi]
pub fn document_request_render() -> NapiResult<()> {
    document::document_request_render().map_err(map_doc_err)
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

/// Query the snap engine for an in-flight drag. `moving_id` is the
/// node currently being dragged (so the engine skips its own edges);
/// `candidate_*` are the candidate world-space bounds; `threshold`
/// is the maximum snap distance in world units.
///
/// Returns a JSON-encoded [`kcreate_vector::snap::SnapResult`] —
/// `{ dx, dy, guides }` — or `null` when no project is open.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn canvas_snap(
    moving_id: Option<String>,
    candidate_x: f64,
    candidate_y: f64,
    candidate_w: f64,
    candidate_h: f64,
    threshold: f64,
) -> NapiResult<Option<String>> {
    let parsed_id = match moving_id {
        Some(s) => Some(parse_uuid(&s)?),
        None => None,
    };
    let result = document::canvas_snap(
        parsed_id,
        candidate_x,
        candidate_y,
        candidate_w,
        candidate_h,
        threshold,
    )
    .map_err(map_doc_err)?;
    match result {
        Some(r) => Ok(Some(serde_json::to_string(&r).map_err(|e| {
            NapiError::from_reason(format!("snap result serialise: {e}"))
        })?)),
        None => Ok(None),
    }
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

/// In-memory variant of [`document_import_image`]: stores the
/// caller-provided encoded image bytes directly without a
/// filesystem round-trip. Used by Phase 4 image generation, which
/// produces PNGs in RAM from the diffusion sidecar.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn document_import_image_bytes(
    parent_id: Option<String>,
    bytes: Vec<u8>,
) -> NapiResult<String> {
    let parent = match parent_id.as_deref() {
        Some(s) => Some(parse_uuid(s)?),
        None => None,
    };
    document::document_import_image_bytes(parent, &bytes)
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

// -----------------------------------------------------------------------------
// Phase 5 — raster filter / transform / heal operations.
// All logic lives in `raster_ops.rs`; these are thin N-API marshalling
// wrappers. Every call records an undoable `Operation` with
// `ai_generated: false` (these are user edits, not AI suggestions).
// -----------------------------------------------------------------------------

/// Apply a Levels adjustment to a raster layer (in place).
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn raster_apply_levels(
    node_id: String,
    black_point: f64,
    white_point: f64,
    gamma: f64,
) -> NapiResult<()> {
    let id = parse_uuid(&node_id)?;
    raster_ops::apply_levels(id, black_point as f32, white_point as f32, gamma as f32)
        .map_err(map_doc_err)
}

/// Apply a Curves adjustment defined by `(input, output)` control
/// points. `points_json` is a JSON array of `[[x, y], ...]` floats in
/// `[0.0, 1.0]`.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn raster_apply_curves(node_id: String, points_json: String) -> NapiResult<()> {
    let id = parse_uuid(&node_id)?;
    let parsed: Vec<(f32, f32)> = serde_json::from_str(&points_json)
        .map_err(|e| NapiError::from_reason(format!("invalid curves points JSON: {e}")))?;
    raster_ops::apply_curves(id, parsed).map_err(map_doc_err)
}

/// Apply a blur filter. `kind` is `"gaussian"` or `"box"`.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn raster_apply_blur(node_id: String, radius: f64, kind: String) -> NapiResult<()> {
    let id = parse_uuid(&node_id)?;
    let kind = match kind.as_str() {
        "gaussian" => raster_ops::BlurKind::Gaussian,
        "box" => raster_ops::BlurKind::Box,
        other => {
            return Err(NapiError::from_reason(format!(
                "unknown blur kind '{other}', expected 'gaussian' or 'box'"
            )));
        }
    };
    raster_ops::apply_blur(id, radius as f32, kind).map_err(map_doc_err)
}

/// Apply an unsharp-mask sharpen.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn raster_apply_sharpen(
    node_id: String,
    radius: f64,
    amount: f64,
    threshold: u32,
) -> NapiResult<()> {
    let id = parse_uuid(&node_id)?;
    let threshold_byte = u8::try_from(threshold.min(u32::from(u8::MAX))).unwrap_or(u8::MAX);
    raster_ops::apply_sharpen(id, radius as f32, amount as f32, threshold_byte).map_err(map_doc_err)
}

/// Crop a raster layer in source-pixel coordinates.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn raster_crop(node_id: String, x: u32, y: u32, w: u32, h: u32) -> NapiResult<()> {
    let id = parse_uuid(&node_id)?;
    raster_ops::crop(id, x, y, w, h).map_err(map_doc_err)
}

/// Rotate a raster layer by `angle_deg` degrees (positive = clockwise).
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn raster_rotate(node_id: String, angle_deg: f64) -> NapiResult<()> {
    let id = parse_uuid(&node_id)?;
    raster_ops::rotate(id, angle_deg as f32).map_err(map_doc_err)
}

/// Flip a raster layer. `direction` is `"horizontal"` or `"vertical"`.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn raster_flip(node_id: String, direction: String) -> NapiResult<()> {
    let id = parse_uuid(&node_id)?;
    let dir = match direction.as_str() {
        "horizontal" => raster_ops::FlipDirection::Horizontal,
        "vertical" => raster_ops::FlipDirection::Vertical,
        other => {
            return Err(NapiError::from_reason(format!(
                "unknown flip direction '{other}', expected 'horizontal' or 'vertical'"
            )));
        }
    };
    raster_ops::flip(id, dir).map_err(map_doc_err)
}

/// Heal a disc from `(src_x, src_y)` over `(dst_x, dst_y)` with the
/// given radius. All coordinates are source-pixel.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn raster_heal(
    node_id: String,
    src_x: u32,
    src_y: u32,
    dst_x: u32,
    dst_y: u32,
    radius: u32,
) -> NapiResult<()> {
    let id = parse_uuid(&node_id)?;
    raster_ops::heal(id, src_x, src_y, dst_x, dst_y, radius).map_err(map_doc_err)
}

/// Non-destructive filter preview. `filter_json` is a JSON object
/// matching the `PreviewFilter` discriminated union; returns a
/// Buffer of RGBA bytes in row-major order, packed against
/// `(width, height)` returned via a separate IPC frame info call.
/// The caller is expected to know the dimensions match the source
/// layer (current behaviour: every Phase-5 preview preserves
/// dimensions — crop / rotate / flip are commit-only).
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn raster_preview_filter(node_id: String, filter_json: String) -> NapiResult<Buffer> {
    let id = parse_uuid(&node_id)?;
    let filter: raster_ops::PreviewFilter = serde_json::from_str(&filter_json)
        .map_err(|e| NapiError::from_reason(format!("invalid filter JSON: {e}")))?;
    let (bytes, _w, _h) = raster_ops::preview_filter(id, filter).map_err(map_doc_err)?;
    Ok(bytes.into())
}

// -----------------------------------------------------------------------------
// Phase 5 — vector path operations + non-destructive effects.
// All logic lives in `vector_ops.rs`; these are thin N-API marshallers.
// -----------------------------------------------------------------------------

/// Apply Ramer-Douglas-Peucker simplification to the vector node's path.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn vector_simplify(node_id: String, tolerance: f64) -> NapiResult<()> {
    let id = parse_uuid(&node_id)?;
    vector_ops::simplify(id, tolerance).map_err(map_doc_err)
}

/// Apply Chaikin corner-cutting smoothing `iterations` times.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn vector_smooth(node_id: String, iterations: u32) -> NapiResult<()> {
    let id = parse_uuid(&node_id)?;
    vector_ops::smooth(id, iterations).map_err(map_doc_err)
}

/// Apply a parallel offset (`distance` in world units).
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn vector_offset(node_id: String, distance: f64) -> NapiResult<()> {
    let id = parse_uuid(&node_id)?;
    vector_ops::offset(id, distance).map_err(map_doc_err)
}

/// Install a variable stroke-width profile on the node's primary
/// stroke. `profile_json` is a JSON array of `[t, width]` pairs
/// with `t` in `[0,1]`; `null` clears the profile.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn vector_set_stroke_profile(node_id: String, profile_json: String) -> NapiResult<()> {
    let id = parse_uuid(&node_id)?;
    let parsed: Option<Vec<(f64, f64)>> = serde_json::from_str(&profile_json)
        .map_err(|e| NapiError::from_reason(format!("invalid stroke-profile JSON: {e}")))?;
    vector_ops::set_stroke_profile(id, parsed).map_err(map_doc_err)
}

/// Push a `PathEffect` (Dash | RoundCorners) onto the node's
/// non-destructive effect chain. `effect_json` is a JSON object
/// using the `kind` discriminator.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn vector_apply_path_effect(node_id: String, effect_json: String) -> NapiResult<()> {
    let id = parse_uuid(&node_id)?;
    let effect: kcreate_core::node::PathEffect = serde_json::from_str(&effect_json)
        .map_err(|e| NapiError::from_reason(format!("invalid path-effect JSON: {e}")))?;
    vector_ops::apply_path_effect(id, effect).map_err(map_doc_err)
}

/// Remove every path effect from the node.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn vector_clear_path_effects(node_id: String) -> NapiResult<()> {
    let id = parse_uuid(&node_id)?;
    vector_ops::clear_path_effects(id).map_err(map_doc_err)
}

// -----------------------------------------------------------------------------
// Phase 5 — text frame linking + wrap (Block D Tasks 19/20).
// -----------------------------------------------------------------------------

/// Link frame `a_id` so its overflow spills into `b_id`. Both
/// must reference text-layer nodes; self-link and cycle creation
/// are rejected.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn text_frame_link(a_id: String, b_id: String) -> NapiResult<()> {
    let a = parse_uuid(&a_id)?;
    let b = parse_uuid(&b_id)?;
    document::text_frame_link(a, b).map_err(map_doc_err)
}

/// Break the link out of `node_id` (sets `next_frame_id` to `None`).
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn text_frame_unlink(node_id: String) -> NapiResult<()> {
    let id = parse_uuid(&node_id)?;
    document::text_frame_unlink(id).map_err(map_doc_err)
}

/// Replace the text frame's wrap mode. `mode_json` is one of
/// `"none" | "bounding_box" | "contour"` (matching
/// [`TextWrapMode`]).
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn text_frame_set_wrap(node_id: String, mode_json: String) -> NapiResult<()> {
    let id = parse_uuid(&node_id)?;
    document::text_frame_set_wrap(id, &mode_json).map_err(map_doc_err)
}

// -----------------------------------------------------------------------------
// Phase 5 — slices (Block D Task 22).
// -----------------------------------------------------------------------------

/// Append a new slice to the project's slice list. Returns the
/// slice's UUID. `format` is `"png" | "svg" | "pdf" | "webp" | "jpeg"`.
#[napi]
#[allow(clippy::too_many_arguments, clippy::needless_pass_by_value)]
pub fn slice_create(
    name: String,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    format: String,
    scale: f64,
) -> NapiResult<String> {
    let id =
        document::slice_create(name, x, y, w, h, &format, scale as f32).map_err(map_doc_err)?;
    Ok(id.to_string())
}

/// Patch fields on a slice. `changes_json` is a JSON object with
/// optional `name`, `bounds`, `format`, `scale` keys.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn slice_update(slice_id: String, changes_json: String) -> NapiResult<()> {
    let id = parse_uuid(&slice_id)?;
    let parsed: document::SliceUpdateProps = serde_json::from_str(&changes_json)
        .map_err(|e| NapiError::from_reason(format!("invalid slice update JSON: {e}")))?;
    document::slice_update(id, parsed).map_err(map_doc_err)
}

/// Remove a slice by id. Returns true when something was removed.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn slice_delete(slice_id: String) -> NapiResult<bool> {
    let id = parse_uuid(&slice_id)?;
    document::slice_delete(id).map_err(map_doc_err)
}

/// Enumerate every slice as a JSON array.
#[napi]
pub fn slice_list() -> NapiResult<String> {
    let slices = document::slice_list().map_err(map_doc_err)?;
    serde_json::to_string(&slices).map_err(|e| NapiError::from_reason(e.to_string()))
}

/// Render every slice into `output_dir` (created if missing).
/// Returns a JSON array of per-slice `SliceResult` records.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn slice_export_all(output_dir: String) -> NapiResult<String> {
    let results =
        document::slice_export_all(std::path::Path::new(&output_dir)).map_err(map_doc_err)?;
    serde_json::to_string(&results).map_err(|e| NapiError::from_reason(e.to_string()))
}

// -----------------------------------------------------------------------------
// Phase 5 — .kbrand import/export (Block D Task 21).
// -----------------------------------------------------------------------------

/// Serialize the brand kit identified by `kit_id` to a `.kbrand`
/// archive at `output_path`. Referenced font / logo blobs are
/// resolved through the project's asset table.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn brand_kit_export(kit_id: String, output_path: String) -> NapiResult<()> {
    let id = parse_uuid(&kit_id)?;
    document::brand_kit_export(id, std::path::Path::new(&output_path)).map_err(map_doc_err)
}

/// Import a `.kbrand` archive. Embedded fonts / logos are
/// inserted into the project's asset table under fresh ids; a new
/// `BrandKit` referencing those assets is appended. Returns the
/// new kit's UUID.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn brand_kit_import(file_path: String) -> NapiResult<String> {
    let id = document::brand_kit_import(std::path::Path::new(&file_path)).map_err(map_doc_err)?;
    Ok(id.to_string())
}

// -----------------------------------------------------------------------------
// Phase 5 — spot colors + overprint convenience (Block D Task 23).
// -----------------------------------------------------------------------------

/// Spec-shaped wrapper for `color_spot_upsert`: inserts a spot
/// color with name + CMYK fallback. Display name defaults to
/// `name`.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn color_add_spot(name: String, c: f64, m: f64, y: f64, k: f64) -> NapiResult<()> {
    document::color_add_spot(name, c as f32, m as f32, y as f32, k as f32).map_err(map_doc_err)
}

/// Toggle a node's overprint flag.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn node_set_overprint(node_id: String, enabled: bool) -> NapiResult<()> {
    let id = parse_uuid(&node_id)?;
    document::node_set_overprint(id, enabled).map_err(map_doc_err)
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

/// Batched [`interaction_list`]. Accepts a JSON array of node id strings;
/// returns a JSON object keyed by node id with the value being a JSON
/// array of [`kcreate_core::Interaction`]. Nodes that don't exist or
/// have no interactions are omitted from the result. One IPC trip for
/// the whole batch, used by the prototype player.
#[napi]
pub fn interaction_list_batch(node_ids_json: String) -> NapiResult<String> {
    let ids: Vec<String> = serde_json::from_str(&node_ids_json)
        .map_err(|e| NapiError::from_reason(format!("invalid node id list: {e}")))?;
    let mut uuids = Vec::with_capacity(ids.len());
    for id in &ids {
        uuids.push(parse_uuid(id)?);
    }
    let map = document::interaction_list_batch(&uuids).map_err(map_doc_err)?;
    // Serialise with string keys so the renderer can index by node id
    // without re-parsing UUIDs on the JS side. `HashMap<Uuid, _>`
    // serialises to a JSON object with Uuid display-formatted as
    // strings by default.
    serde_json::to_string(&map).map_err(|e| NapiError::from_reason(e.to_string()))
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

/// List installed local templates. Returns a JSON array of
/// `TemplateManifest` entries, sorted by name. Optional `category`
/// (snake_case `TemplateCategory` discriminant such as `"pitch_deck"`)
/// or `query` (case-insensitive substring matched against name, tag,
/// or description) narrow the results — providing both with the
/// query non-empty applies the query and ignores the category, which
/// matches the renderer's "search box overrides category filter" UX.
#[napi]
pub fn template_list(category: Option<String>, query: Option<String>) -> NapiResult<String> {
    let cat = match category.as_deref().filter(|s| !s.is_empty()) {
        Some(s) => Some(
            serde_json::from_str::<kcreate_core::TemplateCategory>(&format!("\"{s}\"")).map_err(
                |e| {
                    NapiError::new(
                        Status::InvalidArg,
                        format!("template_list: invalid category {s:?}: {e}"),
                    )
                },
            )?,
        ),
        None => None,
    };
    let q = query.as_deref().filter(|s| !s.is_empty());
    let report = phase2::template_list(cat, q).map_err(map_doc_err)?;
    serde_json::to_string(&report).map_err(|e| NapiError::from_reason(e.to_string()))
}

/// Install a local template from a `.ktemplate/` source folder.
/// Copies the directory into the marketplace root and returns the
/// installed `TemplateManifest` as JSON.
#[napi]
pub fn template_install_local(source_path: String) -> NapiResult<String> {
    let manifest = phase2::template_install_local(&source_path).map_err(map_doc_err)?;
    serde_json::to_string(&manifest).map_err(|e| NapiError::from_reason(e.to_string()))
}

/// Remove an installed local template by id. Deletes the
/// `.ktemplate/` folder on disk.
#[napi]
pub fn template_remove(template_id: String) -> NapiResult<()> {
    let id = parse_uuid(&template_id)?;
    phase2::template_remove(id).map_err(map_doc_err)
}

// ---------------------------------------------------------------------------
// Phase 6 — Audit log (Tasks 13–14)
// ---------------------------------------------------------------------------

/// Record an audit event. `event_json` is a JSON-serialised
/// `AuditEvent` from the renderer (or from the bridge itself for
/// side-effect recording). Returns the event's UUID as a string.
#[napi]
pub fn audit_record(event_json: String) -> NapiResult<String> {
    let event: kcreate_audit::AuditEvent = serde_json::from_str(&event_json)
        .map_err(|e| NapiError::from_reason(format!("audit_record: {e}")))?;
    let id = audit::audit_record(&event).map_err(map_doc_err)?;
    Ok(id.to_string())
}

/// Query the audit log. `query_json` is a JSON-serialised
/// `AuditQuery`. Returns a JSON string containing
/// `{ events: AuditEvent[], total: number }`.
#[napi]
pub fn audit_query(query_json: String) -> NapiResult<String> {
    let filter: kcreate_audit::AuditQuery = serde_json::from_str(&query_json)
        .map_err(|e| NapiError::from_reason(format!("audit_query: {e}")))?;
    let report = audit::audit_query(&filter).map_err(map_doc_err)?;
    serde_json::to_string(&report).map_err(|e| NapiError::from_reason(e.to_string()))
}

/// Return the total number of audit rows.
#[napi]
pub fn audit_count() -> NapiResult<f64> {
    let count = audit::audit_count().map_err(map_doc_err)?;
    Ok(count as f64)
}

/// Delete audit rows strictly older than `cutoff_iso` (RFC 3339).
/// Returns the number of rows removed.
#[napi]
pub fn audit_purge(cutoff_iso: String) -> NapiResult<f64> {
    let cutoff = chrono::DateTime::parse_from_rfc3339(&cutoff_iso)
        .map_err(|e| NapiError::from_reason(format!("audit_purge: invalid timestamp: {e}")))?
        .with_timezone(&chrono::Utc);
    let removed = audit::audit_purge_before(cutoff).map_err(map_doc_err)?;
    Ok(removed as f64)
}

/// Return the filesystem path of the current audit database.
#[napi]
pub fn audit_path() -> NapiResult<String> {
    Ok(audit::audit_path())
}

// ---------------------------------------------------------------------------

/// Add a new content page to the open project. `size` and
/// `orientation` are optional; omit both to use the workspace default.
/// Returns the new page id as a string.
#[napi]
pub fn page_add(
    name: String,
    size: Option<String>,
    orientation: Option<String>,
) -> NapiResult<String> {
    document::page_add(name, size.as_deref(), orientation.as_deref())
        .map(|id| id.to_string())
        .map_err(map_doc_err)
}

/// Duplicate `page_id` (subtree-cloned at the root). Returns the new
/// page id as a string.
#[napi]
pub fn page_duplicate(page_id: String) -> NapiResult<String> {
    let pid = parse_uuid(&page_id)?;
    document::page_duplicate(pid)
        .map(|id| id.to_string())
        .map_err(map_doc_err)
}

/// Reparent `node_id` under `new_parent` (`None` => move to the root
/// list) at the given `index`. Used by the PageNavigator's drag-reorder
/// and by future layer-panel move gestures.
#[napi]
pub fn document_reparent_node(
    node_id: String,
    new_parent: Option<String>,
    index: u32,
) -> NapiResult<()> {
    let nid = parse_uuid(&node_id)?;
    let pid = match new_parent {
        Some(s) if !s.is_empty() => Some(parse_uuid(&s)?),
        _ => None,
    };
    document::document_reparent_node(nid, pid, index as usize).map_err(map_doc_err)
}

// ---------------------------------------------------------------------------
// Phase 2 — preflight, icon pack, parallel batch, AI model packs,
// plugin sandbox, MCP permissions, screenshot-to-layout.
// ---------------------------------------------------------------------------

/// Run the print-readiness checks against the supplied pages. When
/// `request_json` is empty the default options + every page are used.
#[napi]
pub fn preflight_run(request_json: String) -> NapiResult<String> {
    let req: phase2::PreflightRequest = if request_json.trim().is_empty() {
        phase2::PreflightRequest {
            page_ids: Vec::new(),
            options: kcreate_export::preflight::PreflightOptions::default(),
        }
    } else {
        serde_json::from_str(&request_json)
            .map_err(|e| NapiError::from_reason(format!("preflight: invalid JSON: {e}")))?
    };
    let issues = phase2::preflight_run(&req).map_err(map_doc_err)?;
    serde_json::to_string(&issues).map_err(|e| NapiError::from_reason(e.to_string()))
}

/// Generate an icon pack for the given platforms and write the files
/// to `output_dir`. Returns a JSON array of paths actually written.
#[napi]
pub fn export_icon_pack(request_json: String) -> NapiResult<String> {
    let req: phase2::IconPackRequest = serde_json::from_str(&request_json)
        .map_err(|e| NapiError::from_reason(format!("icon pack: invalid JSON: {e}")))?;
    let outcome = phase2::icon_pack_export(&req).map_err(map_doc_err)?;
    serde_json::to_string(&outcome.files).map_err(|e| NapiError::from_reason(e.to_string()))
}

/// Returns the built-in icon-pack platform presets (web / iOS /
/// Android / favicon) as a JSON array — so the renderer's
/// IconPackDialog can show them without hard-coding the size lists.
#[napi]
pub fn export_icon_pack_built_in_platforms() -> NapiResult<String> {
    let platforms = kcreate_export::icon_pack::built_in_platforms();
    serde_json::to_string(&platforms).map_err(|e| NapiError::from_reason(e.to_string()))
}

/// Start a parallel batch-export job and return its id. The job runs
/// on a background thread; the renderer polls `export_batch_status`
/// for progress and may call `export_batch_cancel` at any time.
#[napi]
pub fn export_batch_start(job_json: String) -> NapiResult<String> {
    let job: kcreate_export::batch::BatchExportJob = serde_json::from_str(&job_json)
        .map_err(|e| NapiError::from_reason(format!("batch: invalid JSON: {e}")))?;
    phase2::batch_start(job).map_err(map_doc_err)
}

#[napi]
pub fn export_batch_status(job_id: String) -> NapiResult<String> {
    let status = phase2::batch_status(&job_id).map_err(map_doc_err)?;
    serde_json::to_string(&status).map_err(|e| NapiError::from_reason(e.to_string()))
}

#[napi]
pub fn export_batch_cancel(job_id: String) -> NapiResult<()> {
    phase2::batch_cancel(&job_id).map_err(map_doc_err)
}

/// Release the bookkeeping state for a finished batch-export job.
///
/// Repeated polls of `export_batch_status` after a job reaches a
/// terminal state are explicitly allowed and return the same
/// terminal payload — see the docs on [`phase2::batch_dismiss`] for
/// why. The renderer is expected to call this once it has rendered
/// the terminal status to free the cached `BatchResult`. Dismissing
/// an unknown id is a no-op; the return value is `true` when a
/// handle was actually dropped.
#[napi]
pub fn export_batch_dismiss(job_id: String) -> NapiResult<bool> {
    phase2::batch_dismiss(&job_id).map_err(map_doc_err)
}

/// Lanczos3-upscale the raster layer at `node_id` by `scale`. A new
/// RasterLayer node is inserted as a sibling; its id is returned.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn ai_upscale(node_id: String, scale: f64) -> NapiResult<String> {
    // Pass `scale` through to the algorithm as `f64`. The previous
    // `scale as f32` cast silently rounded values just above 1.0
    // (e.g. `1.0000001_f64`) down to exactly `1.0_f32`, which the
    // algorithm then rejected as out of range. JavaScript numbers
    // are always `f64`, so preserving that precision across the FFI
    // boundary is the right architectural fix. Per Devin Review
    // ANALYSIS_pr-review-job-0594c03f68c24589ba78a32926e3874f_0004.
    let id = parse_uuid(&node_id)?;
    phase2::ai_upscale(id, scale)
        .map(|u| u.to_string())
        .map_err(map_doc_err)
}

/// Extract the dominant colors from a raster layer.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn ai_extract_palette(node_id: String, max_colors: u32) -> NapiResult<String> {
    let id = parse_uuid(&node_id)?;
    phase2::ai_extract_palette(id, max_colors as usize).map_err(map_doc_err)
}

/// Smart-select flood fill. Returns a base64-encoded 1-byte mask
/// where 255 means "in the selection".
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn ai_smart_select(node_id: String, x: u32, y: u32, tolerance: f64) -> NapiResult<String> {
    let id = parse_uuid(&node_id)?;
    phase2::ai_smart_select(id, x, y, tolerance).map_err(map_doc_err)
}

/// Backend-selectable upscale. `backend` is the serde representation
/// of [`kcreate_ai::UpscaleBackend`] (`"lanczos3"` / `"esrgan"`).
/// `model_path` is required for ONNX backends; pass an empty string
/// to omit. Returns a JSON [`phase2::UpscaleWithBackendReport`].
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn ai_upscale_with_backend(
    node_id: String,
    scale: f64,
    backend: String,
    model_path: String,
) -> NapiResult<String> {
    let id = parse_uuid(&node_id)?;
    let path = if model_path.is_empty() {
        None
    } else {
        Some(model_path.as_str())
    };
    phase2::ai_upscale_with_backend(id, scale, &backend, path).map_err(map_doc_err)
}

/// Point-prompt segmentation. `backend` is the serde representation
/// of [`kcreate_ai::SegmentBackend`] (`"edge_aware"` / `"sam"`).
/// `model_path` is required for ONNX backends; pass an empty string
/// to omit. Returns a JSON [`phase2::SegmentReport`].
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn ai_segment(
    node_id: String,
    point_x: u32,
    point_y: u32,
    tolerance: f64,
    edge_threshold: f64,
    backend: String,
    model_path: String,
) -> NapiResult<String> {
    let id = parse_uuid(&node_id)?;
    let path = if model_path.is_empty() {
        None
    } else {
        Some(model_path.as_str())
    };
    phase2::ai_segment(
        id,
        point_x,
        point_y,
        tolerance,
        edge_threshold,
        &backend,
        path,
    )
    .map_err(map_doc_err)
}

/// Detect text-like regions in the raster layer identified by
/// `node_id`. Returns the JSON-serialised `Vec<TextRegion>` from
/// `kcreate_ai::ocr::detect_text_regions`. `options_json` accepts
/// `"null"` or `""` to use detector defaults; otherwise must
/// deserialise to `kcreate_ai::DetectTextRegionsOptions`.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn ai_detect_text_regions(node_id: String, options_json: String) -> NapiResult<String> {
    let id = parse_uuid(&node_id)?;
    phase2::ai_detect_text_regions(id, &options_json).map_err(map_doc_err)
}

/// Materialise a detected text region as a new `TextLayer` sibling
/// of the source raster. `request_json` must deserialise to
/// `phase2::InsertTextLayerForRegionRequest`. Returns the new
/// node id (UUID).
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn ai_insert_text_layer_for_region(request_json: String) -> NapiResult<String> {
    let req: phase2::InsertTextLayerForRegionRequest = serde_json::from_str(&request_json)
        .map_err(|e| napi::Error::from_reason(format!("invalid request: {e}")))?;
    let id = phase2::ai_insert_text_layer_for_region(&req).map_err(map_doc_err)?;
    Ok(id.to_string())
}

/// Return the registry of locally available / installable AI model
/// packs.
#[napi]
pub fn ai_list_model_packs() -> NapiResult<String> {
    phase2::ai_models_list().map_err(map_doc_err)
}

/// Install an optional model pack from a user-provided source path.
/// `pack_id` must match a non-built-in entry in
/// [`ai_list_model_packs`]; `source_path` is the absolute path to
/// the weights file the user downloaded out of band (KCreate does
/// not fetch from the network itself). Returns the
/// [`kcreate_ai::InstallReport`] JSON describing the actual hash
/// and the `verified` flag.
#[napi]
pub fn ai_install_model_pack(pack_id: String, source_path: String) -> NapiResult<String> {
    phase2::ai_model_install(pack_id, source_path).map_err(map_doc_err)
}

/// Uninstall an optional model pack by deleting its file from the
/// models directory. Idempotent — uninstalling an already-absent
/// pack returns Ok.
#[napi]
pub fn ai_uninstall_model_pack(pack_id: String) -> NapiResult<()> {
    phase2::ai_model_uninstall(pack_id).map_err(map_doc_err)
}

/// Import a PDF file at `file_path` into the current project: one
/// Page per PDF page, embedded images become RasterLayer children,
/// extracted text becomes a TextLayer per page. Returns JSON
/// matching [`phase2::PdfImportReport`].
#[napi]
pub fn pdf_import(file_path: String) -> NapiResult<String> {
    phase2::pdf_import(file_path).map_err(map_doc_err)
}

/// Run edge-detection + connected-component analysis over the
/// supplied RGBA8 screenshot and return the detected UI regions.
#[napi]
pub fn ai_screenshot_to_layout(request_json: String) -> NapiResult<String> {
    let req: phase2::ScreenshotRequest = serde_json::from_str(&request_json)
        .map_err(|e| NapiError::from_reason(format!("screenshot: invalid JSON: {e}")))?;
    phase2::ai_screenshot_to_layout(&req).map_err(map_doc_err)
}

/// Run the local alt-text heuristic against a raster layer.
/// Returns the [`kcreate_ai::AltTextReport`] as JSON (text +
/// brightness + contrast + saturation + edge density + palette).
/// Read-only: does NOT persist anything to the document; call
/// [`ai_apply_alt_text`] to commit the chosen text.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn ai_alt_text_for_node(node_id: String) -> NapiResult<String> {
    let id = parse_uuid(&node_id)?;
    phase2::ai_alt_text_for_node(id).map_err(map_doc_err)
}

/// Persist an alt-text label onto `node_id`. Records an
/// undo/redo-able operation in the document log. An empty `text`
/// clears the metadata key entirely.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn ai_apply_alt_text(node_id: String, text: String) -> NapiResult<()> {
    let id = parse_uuid(&node_id)?;
    phase2::ai_apply_alt_text(id, text).map_err(map_doc_err)
}

/// Run the layout-suggest heuristic over the direct (visible,
/// non-degenerate) children of `artboard_id`. Returns a JSON
/// array of [`kcreate_ai::LayoutSuggestion`]. Returns an empty
/// `[]` rather than an error when fewer than two candidates
/// remain, so the UI can render a "nothing to suggest" state
/// without special-casing the call.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn ai_layout_suggest_for_artboard(artboard_id: String) -> NapiResult<String> {
    let id = parse_uuid(&artboard_id)?;
    phase2::ai_layout_suggest_for_artboard(id).map_err(map_doc_err)
}

/// List installed plugins with their enabled flag.
#[napi]
pub fn plugin_list() -> NapiResult<String> {
    let list = phase2::plugin_list().map_err(map_doc_err)?;
    serde_json::to_string(&list).map_err(|e| NapiError::from_reason(e.to_string()))
}

#[napi]
pub fn plugin_enable(id: String) -> NapiResult<()> {
    phase2::plugin_enable(&id).map_err(map_doc_err)
}

#[napi]
pub fn plugin_disable(id: String) -> NapiResult<()> {
    phase2::plugin_disable(&id).map_err(map_doc_err)
}

/// Snapshot of every Ed25519 public key in `trusted_keys.json`. The
/// UI's "Trusted Authorities" list calls this on PluginManager mount
/// so users can see which signing identities are currently allowed
/// to register native plugins. Returns a JSON array of
/// `{ keyId, comment }`.
#[napi]
pub fn plugin_trust_list() -> NapiResult<String> {
    let list = phase2::plugin_trust_list().map_err(map_doc_err)?;
    serde_json::to_string(&list).map_err(|e| NapiError::from_reason(e.to_string()))
}

/// Reload `trusted_keys.json` from disk and rescan plugins. Use this
/// after the user adds a new trusted key out-of-band so previously-
/// rejected native plugins get a second chance without restarting
/// the host.
#[napi]
pub fn plugin_trust_reload() -> NapiResult<()> {
    phase2::plugin_trust_reload().map_err(map_doc_err)
}

#[napi]
pub fn plugin_execute(id: String, function: String, input: String) -> NapiResult<String> {
    phase2::plugin_execute(&id, &function, &input).map_err(map_doc_err)
}

/// Phase 2 extended-ABI execution: builds a `PluginContext` with the
/// current document snapshot, the project's blob store as asset loader,
/// and the manifest's declared permissions, then validates and applies
/// any proposals the plugin emits. Returns a JSON envelope `{output,
/// logs, proposals}` (see `phase2::ProposalReport`).
#[napi]
pub fn plugin_execute_with_context(
    id: String,
    function: String,
    input: String,
) -> NapiResult<String> {
    phase2::plugin_execute_with_context(&id, &function, &input).map_err(map_doc_err)
}

/// List installed JS panel plugins. Returns a JSON array of
/// `JsPanelInfo` so the Electron main process can decide which
/// sandboxed `BrowserView` instances to mount.
#[napi]
pub fn plugin_js_list() -> NapiResult<String> {
    let list = phase2::plugin_js_list().map_err(map_doc_err)?;
    serde_json::to_string(&list).map_err(|e| NapiError::from_reason(e.to_string()))
}

/// Validate and dispatch a single message from a sandboxed JS panel.
/// The Electron host calls this for every inbound `postMessage` from
/// the panel's `<webview>` / `BrowserView`; the bridge enforces the
/// permission gates and returns a `JsPanelMessageOutcome` JSON.
#[napi]
pub fn plugin_js_message(plugin_id: String, message_json: String) -> NapiResult<String> {
    phase2::plugin_js_message(&plugin_id, &message_json).map_err(map_doc_err)
}

#[napi]
pub fn mcp_permission_list() -> NapiResult<String> {
    phase2::mcp_permission_list().map_err(map_doc_err)
}

#[napi]
pub fn mcp_permission_grant(client_id: String, tool_name: String, grant: String) -> NapiResult<()> {
    phase2::mcp_permission_grant(&client_id, &tool_name, &grant).map_err(map_doc_err)
}

#[napi]
pub fn mcp_permission_revoke(client_id: String, tool_name: String) -> NapiResult<()> {
    phase2::mcp_permission_revoke(&client_id, &tool_name).map_err(map_doc_err)
}

#[napi]
pub fn mcp_status() -> NapiResult<String> {
    let status = phase2::mcp_status();
    serde_json::to_string(&status).map_err(|e| NapiError::from_reason(e.to_string()))
}

// ---------------------------------------------------------------------------
// Phase 2 — color management
// ---------------------------------------------------------------------------

/// Read the project's color management settings as JSON.
#[napi]
pub fn color_settings_get() -> NapiResult<String> {
    phase2::color_settings_get().map_err(map_doc_err)
}

/// Replace the project's color management settings. `settings_json`
/// must deserialize into `kcreate_core::color::ColorSettings`.
/// Records an undoable `color_settings_update` operation.
#[napi]
pub fn color_settings_update(settings_json: String) -> NapiResult<()> {
    phase2::color_settings_update(&settings_json).map_err(map_doc_err)
}

/// Convert a color value between color spaces. `to_space` is one of
/// `"srgb"`, `"cmyk"`, `"lab"`, `"hsl"`. Returns the converted color
/// as a JSON `kcreate_core::color::Color`.
#[napi]
pub fn color_convert(from_json: String, to_space: String) -> NapiResult<String> {
    phase2::color_convert(&from_json, &to_space).map_err(map_doc_err)
}

/// Insert or replace a spot colour in the document's
/// `SpotColorLibrary`. `wire_json` is a [`phase2::SpotColorWire`].
/// Records an undoable `spot_color_upsert` operation.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn color_spot_upsert(wire_json: String) -> NapiResult<()> {
    phase2::color_spot_upsert(&wire_json).map_err(map_doc_err)
}

/// Remove a spot colour by name. Returns `false` when the name was
/// not present in the library (no operation recorded), `true` after a
/// successful undoable `spot_color_remove`.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn color_spot_remove(name: String) -> NapiResult<bool> {
    phase2::color_spot_remove(&name).map_err(map_doc_err)
}

/// List every spot colour in the document as a JSON array of
/// [`phase2::SpotColorWire`].
#[napi]
pub fn color_spot_list() -> NapiResult<String> {
    phase2::color_spot_list().map_err(map_doc_err)
}

/// Parse a Pantone-style JSON catalogue and merge its entries into
/// the project's `SpotColorLibrary`. Returns a JSON
/// [`phase2::SpotCatalogLoadReport`] with `{added, overwritten, parsed}`.
///
/// The catalog supports two shapes (see
/// [`kcreate_core::color::SpotColorLibrary::from_json_catalog`]):
/// * Wrapped: `{ "entries": [{ "id": "...", "cmyk": [...] }, ...] }`
/// * Bare map: `{ "swatch-id": { "cmyk": [...] }, ... }`
///
/// Recorded as a single undoable `spot_color_load_catalog` operation.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn color_spot_load_catalog(raw_json: String) -> NapiResult<String> {
    let report = phase2::color_spot_load_catalog(&raw_json).map_err(map_doc_err)?;
    serde_json::to_string(&report).map_err(|e| napi::Error::from_reason(e.to_string()))
}

// ---------------------------------------------------------------------------
// Phase 2 — text frame + OpenType (Block B Task 11)
// ---------------------------------------------------------------------------

/// Read the `TextFrameOptions` for a `TextLayer` node as JSON.
///
/// Returns the default options (single column, no hyphenation,
/// clip overflow, top-aligned, no inset, fixed size) when the node
/// has no `text_frame` metadata. Errors if the node id doesn't
/// resolve or the node is not a `TextLayer`.
#[napi]
pub fn text_frame_get(node_id: String) -> NapiResult<String> {
    let id = parse_uuid(&node_id)?;
    phase2::text_frame_get(id).map_err(map_doc_err)
}

/// Replace the `TextFrameOptions` for a `TextLayer` node and record
/// an undoable `text_frame_update` operation.
#[napi]
pub fn text_frame_update(node_id: String, options_json: String) -> NapiResult<()> {
    let id = parse_uuid(&node_id)?;
    phase2::text_frame_update(id, &options_json).map_err(map_doc_err)
}

/// Compute and return the paragraph layout for a `TextLayer` node
/// as JSON (one entry per laid-out line with origin / baseline /
/// width / column / glyph count plus an `overflow` flag and
/// `usedHeight`). Pure read; does not record an operation.
#[napi]
pub fn text_layout_compute(node_id: String) -> NapiResult<String> {
    let id = parse_uuid(&node_id)?;
    phase2::text_layout_compute(id).map_err(map_doc_err)
}

/// Read the `OpenTypeFeatures` for a `TextLayer` node as JSON.
#[napi]
pub fn text_opentype_features_get(node_id: String) -> NapiResult<String> {
    let id = parse_uuid(&node_id)?;
    phase2::text_opentype_features_get(id).map_err(map_doc_err)
}

/// Replace the `OpenTypeFeatures` for a `TextLayer` node and record
/// an undoable `text_opentype_features_update` operation.
#[napi]
pub fn text_opentype_features_update(node_id: String, features_json: String) -> NapiResult<()> {
    let id = parse_uuid(&node_id)?;
    phase2::text_opentype_features_update(id, &features_json).map_err(map_doc_err)
}

// ---------------------------------------------------------------------------
// Phase 3 — LAN collaboration session
//
// Gated by the `collab` feature so default builds keep the
// editing path free of QUIC / mDNS / tokio dependencies. The
// Electron host enables `collab` for release packaging; debug
// builds default-on through the workspace `default-members` list.
// ---------------------------------------------------------------------------

#[cfg(feature = "collab")]
#[allow(clippy::needless_pass_by_value)]
fn map_session_err(e: crate::collab::SessionBridgeError) -> NapiError {
    let status = match e {
        crate::collab::SessionBridgeError::InvalidArgument { .. }
        | crate::collab::SessionBridgeError::NotRunning
        | crate::collab::SessionBridgeError::AlreadyRunning
        | crate::collab::SessionBridgeError::NotInKChatGroup
        | crate::collab::SessionBridgeError::KChatDevIssuerDisabled => Status::InvalidArg,
        _ => Status::GenericFailure,
    };
    NapiError::new(status, format!("kcreate_bridge: {e}"))
}

/// Start a collab session. Returns a JSON [`SessionStartReport`].
/// `seed_b64` is the 32-byte Ed25519 signing-key seed, base64url
/// encoded (padded or unpadded both accepted). The renderer
/// persists the seed across launches so the same machine
/// presents a stable peer identity.
#[cfg(feature = "collab")]
#[napi]
pub fn session_start(
    seed_b64: String,
    display_name: String,
    project_id: String,
    advertise_mdns: bool,
) -> NapiResult<String> {
    let pid = parse_uuid(&project_id)?;
    let report = crate::collab::session_start(&seed_b64, &display_name, pid, advertise_mdns)
        .map_err(map_session_err)?;
    serde_json::to_string(&report).map_err(|e| {
        NapiError::new(
            Status::GenericFailure,
            format!("kcreate_bridge: session_start serialize: {e}"),
        )
    })
}

/// Stop the running session (graceful Goodbye + endpoint close).
/// Idempotent. Returns the leaving peer's base64url-encoded id if
/// a session was actually running (so the orchestrator in
/// `main.ts` can emit a synthetic `sessionLeft` event on the
/// renderer's session-event channel), or `null` if the call was a
/// no-op because no session was active.
#[cfg(feature = "collab")]
#[napi]
pub fn session_leave() -> NapiResult<Option<String>> {
    crate::collab::session_leave().map_err(map_session_err)
}

/// Dial a known peer. All five fields come from the discovered
/// peer roster or a pasted peer link.
#[cfg(feature = "collab")]
#[napi]
pub fn session_join(
    peer_id: String,
    public_key: String,
    display_name: String,
    socket_addr: String,
    cert_fingerprint_b64: String,
) -> NapiResult<()> {
    crate::collab::session_join(
        &peer_id,
        &public_key,
        &display_name,
        &socket_addr,
        &cert_fingerprint_b64,
    )
    .map_err(map_session_err)
}

/// Return the current peer roster as JSON `Vec<SessionPeer>`.
#[cfg(feature = "collab")]
#[napi]
pub fn session_peers() -> NapiResult<String> {
    let peers = crate::collab::session_peers().map_err(map_session_err)?;
    serde_json::to_string(&peers).map_err(|e| {
        NapiError::new(
            Status::GenericFailure,
            format!("kcreate_bridge: session_peers serialize: {e}"),
        )
    })
}

/// Drain the buffered session events as JSON `Vec<SessionEvent>`.
/// The Electron main process calls this on a tick.
#[cfg(feature = "collab")]
#[napi]
pub fn session_drain_events() -> NapiResult<String> {
    let events = crate::collab::session_drain_events().map_err(map_session_err)?;
    serde_json::to_string(&events).map_err(|e| {
        NapiError::new(
            Status::GenericFailure,
            format!("kcreate_bridge: session_drain_events serialize: {e}"),
        )
    })
}

/// Broadcast the local user's presence. `cursor_json` may be null
/// or a JSON `{ "x": number, "y": number }`.
#[cfg(feature = "collab")]
#[napi]
pub fn session_send_presence(
    active_page: Option<String>,
    selection_json: String,
    cursor_json: Option<String>,
) -> NapiResult<()> {
    let active = match active_page {
        Some(s) if !s.is_empty() => Some(parse_uuid(&s)?),
        _ => None,
    };
    let selection: Vec<Uuid> = serde_json::from_str(&selection_json).map_err(|e| {
        NapiError::new(
            Status::InvalidArg,
            format!("kcreate_bridge: session_send_presence selection: {e}"),
        )
    })?;
    let cursor: Option<crate::collab::SessionCursor> = match cursor_json {
        Some(s) => Some(serde_json::from_str(&s).map_err(|e| {
            NapiError::new(
                Status::InvalidArg,
                format!("kcreate_bridge: session_send_presence cursor: {e}"),
            )
        })?),
        None => None,
    };
    crate::collab::session_send_presence(active, selection, cursor).map_err(map_session_err)
}

/// Block 7: read the running session's operation journal summary
/// as a JSON `SessionJournalSummary`. KChat-gated; returns an
/// error envelope if multiplayer is locked or no session is
/// running.
#[cfg(feature = "collab")]
#[napi]
pub fn session_journal_summary() -> NapiResult<String> {
    let summary = crate::collab::session_journal_summary().map_err(map_session_err)?;
    serde_json::to_string(&summary).map_err(|e| {
        NapiError::new(
            Status::GenericFailure,
            format!("kcreate_bridge: session_journal_summary serialize: {e}"),
        )
    })
}

/// Block 8: snapshot of the advisory edit-lock roster. KChat-gated.
/// Returns a JSON `Vec<SessionLockEntry>` so the renderer can
/// deserialize directly into its TS type.
#[cfg(feature = "collab")]
#[napi]
pub fn session_locks() -> NapiResult<String> {
    let rows = crate::collab::session_locks().map_err(map_session_err)?;
    serde_json::to_string(&rows).map_err(|e| {
        NapiError::new(
            Status::GenericFailure,
            format!("kcreate_bridge: session_locks serialize: {e}"),
        )
    })
}

/// Block 8: claim advisory edit locks on the supplied node ids
/// (parsed as a JSON `string[]` of UUIDs). KChat-gated; the local
/// roster updates immediately and the change is broadcast to
/// every connected peer. Returns the wall-clock `acquired_at`
/// as an RFC3339 string so the renderer can show "locked X
/// seconds ago" without a second IPC.
#[cfg(feature = "collab")]
#[napi]
pub fn session_claim_locks(node_ids_json: String) -> NapiResult<String> {
    let node_ids: Vec<Uuid> = serde_json::from_str(&node_ids_json).map_err(|e| {
        NapiError::new(
            Status::InvalidArg,
            format!("kcreate_bridge: session_claim_locks parse node_ids: {e}"),
        )
    })?;
    let acquired_at = crate::collab::session_claim_locks(node_ids).map_err(map_session_err)?;
    Ok(acquired_at.to_rfc3339())
}

/// Block 8: release advisory edit locks. Empty node-id list means
/// "release every lock the local peer currently holds".
#[cfg(feature = "collab")]
#[napi]
pub fn session_release_locks(node_ids_json: String) -> NapiResult<()> {
    let node_ids: Vec<Uuid> = serde_json::from_str(&node_ids_json).map_err(|e| {
        NapiError::new(
            Status::InvalidArg,
            format!("kcreate_bridge: session_release_locks parse node_ids: {e}"),
        )
    })?;
    crate::collab::session_release_locks(node_ids).map_err(map_session_err)?;
    Ok(())
}

/// Read the cached `SessionStartReport` for the running session,
/// or `null` if no session is running. Returns a JSON string so
/// the renderer can deserialize directly into its TS type.
#[cfg(feature = "collab")]
#[napi]
pub fn session_info() -> NapiResult<String> {
    let info = crate::collab::session_info();
    serde_json::to_string(&info).map_err(|e| {
        NapiError::new(
            Status::GenericFailure,
            format!("kcreate_bridge: session_info serialize: {e}"),
        )
    })
}

/// Install (or refresh) the KChat group authority from a JSON
/// `KChatInstallRequest`. Until this is called with a valid
/// payload, every multiplayer entry point (`session_start`,
/// `session_join`, `session_send_presence`) fails with
/// `InvalidArg("multiplayer is locked: not signed into a KChat
/// group")`.
///
/// Returns a JSON [`KChatMembershipStatus`] describing the new
/// state on success.
#[cfg(feature = "collab")]
#[napi]
pub fn kchat_install_authority(request_json: String) -> NapiResult<String> {
    let req: crate::collab::KChatInstallRequest =
        serde_json::from_str(&request_json).map_err(|e| {
            NapiError::new(
                Status::InvalidArg,
                format!("kcreate_bridge: kchat_install_authority request: {e}"),
            )
        })?;
    let status = crate::collab::kchat_install_authority(req).map_err(map_session_err)?;
    serde_json::to_string(&status).map_err(|e| {
        NapiError::new(
            Status::GenericFailure,
            format!("kcreate_bridge: kchat_install_authority serialize: {e}"),
        )
    })
}

/// Clear the installed KChat authority and re-lock multiplayer.
/// Returns the new (locked) status as JSON.
#[cfg(feature = "collab")]
#[napi]
pub fn kchat_clear_authority() -> NapiResult<String> {
    let status = crate::collab::kchat_clear_authority();
    serde_json::to_string(&status).map_err(|e| {
        NapiError::new(
            Status::GenericFailure,
            format!("kcreate_bridge: kchat_clear_authority serialize: {e}"),
        )
    })
}

/// Snapshot the current KChat membership status. Returns a JSON
/// [`KChatMembershipStatus`]. Renderer polls this on mount to
/// decide whether to show the "sign into a KChat group" CTA or
/// the live PresencePanel.
#[cfg(feature = "collab")]
#[napi]
pub fn kchat_membership_status() -> NapiResult<String> {
    let status = crate::collab::kchat_membership_status();
    serde_json::to_string(&status).map_err(|e| {
        NapiError::new(
            Status::GenericFailure,
            format!("kcreate_bridge: kchat_membership_status serialize: {e}"),
        )
    })
}

/// Probe whether the bridge was built with the `kchat-dev-issuer`
/// feature. Always callable; returns `false` when the feature is
/// off so the renderer can decide whether to surface the dev-only
/// "Mint dev membership" affordance.
#[napi]
pub fn kchat_dev_issuer_available() -> bool {
    cfg!(feature = "kchat-dev-issuer")
}

/// Derive the local KChat peer identity from a persistent seed.
/// Returns a JSON `KChatLocalIdentity` (`peerId`, `peerPublicKey`).
/// Used by the sign-in panel to pre-fill the public key field
/// (which the user otherwise can't compute without a crypto
/// library in the renderer).
#[cfg(feature = "collab")]
#[napi]
pub fn kchat_derive_local_identity(seed_b64: String) -> NapiResult<String> {
    let identity =
        crate::collab::kchat_derive_local_identity(&seed_b64).map_err(map_session_err)?;
    serde_json::to_string(&identity).map_err(|e| {
        NapiError::new(
            Status::GenericFailure,
            format!("kcreate_bridge: kchat_derive_local_identity serialize: {e}"),
        )
    })
}

/// Configure the on-disk path for the KChat trusted-issuer
/// allowlist. The Electron main process should call this once at
/// startup with `<userData>/kchat_trust.json`. Reads the file at
/// the supplied path (or starts with an empty list if missing)
/// and replaces the in-memory store. Subsequent
/// `kchat_add_trusted_issuer` / `kchat_remove_trusted_issuer`
/// calls atomically persist back to this path.
///
/// Returns the current list as a JSON `TrustedIssuer[]`.
#[cfg(feature = "collab")]
#[napi]
pub fn kchat_set_trust_store_path(path: String) -> NapiResult<String> {
    let issuers = crate::collab::kchat_set_trust_store_path(std::path::PathBuf::from(path))
        .map_err(map_session_err)?;
    serde_json::to_string(&issuers).map_err(|e| {
        NapiError::new(
            Status::GenericFailure,
            format!("kcreate_bridge: kchat_set_trust_store_path serialize: {e}"),
        )
    })
}

/// Return the current trusted-issuer allowlist as a JSON
/// `TrustedIssuer[]`. Cheap clone of the in-memory list; never
/// reads from disk (the on-disk list is loaded by
/// `kchat_set_trust_store_path`).
#[cfg(feature = "collab")]
#[napi]
pub fn kchat_trusted_issuers() -> NapiResult<String> {
    let issuers = crate::collab::kchat_list_trusted_issuers();
    serde_json::to_string(&issuers).map_err(|e| {
        NapiError::new(
            Status::GenericFailure,
            format!("kcreate_bridge: kchat_trusted_issuers serialize: {e}"),
        )
    })
}

/// Add (or update) a trusted issuer. `issuer_json` deserialises
/// to `TrustedIssuer { issuer_public_key, label, added_at? }`. If
/// an entry with the same `issuer_public_key` exists, its label
/// and timestamp are replaced — so the renderer can re-call this
/// to rename an entry. Persists to the configured path. Returns
/// the updated list.
#[cfg(feature = "collab")]
#[napi]
pub fn kchat_add_trusted_issuer(issuer_json: String) -> NapiResult<String> {
    let issuer: crate::collab::TrustedIssuer = serde_json::from_str(&issuer_json).map_err(|e| {
        NapiError::new(
            Status::InvalidArg,
            format!("kcreate_bridge: kchat_add_trusted_issuer request: {e}"),
        )
    })?;
    let updated = crate::collab::kchat_add_trusted_issuer(issuer).map_err(map_session_err)?;
    serde_json::to_string(&updated).map_err(|e| {
        NapiError::new(
            Status::GenericFailure,
            format!("kcreate_bridge: kchat_add_trusted_issuer serialize: {e}"),
        )
    })
}

/// Remove a trusted issuer by its `issuer_public_key`. No-ops when
/// no matching entry exists. Persists to the configured path.
/// Returns the updated list. Removing the last entry collapses
/// the allowlist back to "accept any issuer" mode.
#[cfg(feature = "collab")]
#[napi]
pub fn kchat_remove_trusted_issuer(issuer_public_key: String) -> NapiResult<String> {
    let updated =
        crate::collab::kchat_remove_trusted_issuer(&issuer_public_key).map_err(map_session_err)?;
    serde_json::to_string(&updated).map_err(|e| {
        NapiError::new(
            Status::GenericFailure,
            format!("kcreate_bridge: kchat_remove_trusted_issuer serialize: {e}"),
        )
    })
}

/// Dev-only: mint a fresh KChat membership attestation against a
/// deterministic in-process issuer. Returns a JSON
/// [`KChatInstallRequest`] the renderer can pass straight back
/// into [`kchat_install_authority`] without going through any
/// out-of-tree KChat server.
///
/// The request payload is a JSON `KChatDevMintRequest`:
///
/// ```json
/// {
///   "issuerSeed":      "base64url 32 bytes (deterministic; same seed → same issuer)",
///   "groupId":         "url-safe ASCII group id",
///   "peerPublicKey":   "base64url 32 bytes (local peer Ed25519 verifying key)",
///   "validForSeconds": 3600
/// }
/// ```
///
/// Compiled into any bridge built with the `collab` feature (the
/// only feature that gates the multiplayer N-API surface as a
/// whole). The inner `crate::collab::kchat_dev_mint_membership_json`
/// dispatches to either the real implementation (when the bridge
/// is *also* built with `kchat-dev-issuer`) or a no-op shim that
/// returns `KChatDevIssuerDisabled` (when it is not). This keeps
/// the IPC surface stable across collab builds so renderers can
/// probe via `kchat_dev_issuer_available()` and decide whether to
/// surface the affordance, without conditionally importing the
/// function. See `crates/kcreate_bridge/Cargo.toml` — the
/// `kchat-dev-issuer` feature already implies `collab`, so
/// double-gating here would be redundant.
#[cfg(feature = "collab")]
#[napi]
pub fn kchat_dev_mint_membership(request_json: String) -> NapiResult<String> {
    crate::collab::kchat_dev_mint_membership_json(&request_json).map_err(map_session_err)
}

// =============================================================================
// Phase 4 — Vision (VLM) + Image Generation
// =============================================================================
//
// Thin N-API wrappers around `phase4.rs`. The logic lives entirely
// in the bridge module; this layer only marshals between Rust types
// and the JSON-strings / primitive arguments the renderer hands us.

fn map_phase4_err(e: phase4::Phase4BridgeError) -> NapiError {
    NapiError::new(Status::GenericFailure, e.to_string())
}

/// Start the vision sidecar for the given pack id. Returns the
/// listening port. The dispatcher decides between llama-server and
/// MLX based on the pack id suffix + platform + MLX availability;
/// the renderer doesn't need to know which runtime is in use.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn vision_start(pack_id: String) -> NapiResult<u32> {
    let port = phase4::vision_start(pack_id).map_err(map_phase4_err)?;
    Ok(u32::from(port))
}

/// Stop the vision sidecar if running. Idempotent.
#[napi]
pub fn vision_stop() {
    phase4::vision_stop();
}

/// JSON-encoded [`phase4::VisionStatusInfo`].
#[napi]
pub fn vision_status() -> NapiResult<String> {
    serde_json::to_string(&phase4::vision_status())
        .map_err(|e| NapiError::from_reason(format!("vision_status: {e}")))
}

// ----- Vision inference (AsyncTask) -----
//
// Every VLM / diffusion HTTP round-trip below can take 5–30+
// seconds (cold model load, prompt processing on CPU). Running them
// on the Electron main thread freezes the window for the duration,
// which is what the LLM chat task wrappers above already avoid. We
// mirror that pattern: each `pub fn` constructs a `Task`, returns
// `AsyncTask<...>`, and N-API resolves the JS `Promise<string>` once
// the libuv worker finishes. The renderer was already `await`-ing
// these calls, so the JS-visible contract doesn't change — we just
// stop freezing the UI while the model thinks.
//
// Wire shape: pixel arguments use `napi::bindgen_prelude::Buffer`
// (zero-copy on the way in from a Node `Buffer`). The previous
// shape was `Vec<u8>`, which forced TypeScript callers to
// `Array.from(buffer)` — a ~4 M-element JS array allocation per
// 1024×1024 frame plus a per-element copy through the JSON-ish
// V8 boundary. A `Buffer` parameter binds straight to the
// underlying `ArrayBuffer`, so the only copy happens once when we
// snapshot the bytes into the `Task` (the `Buffer` can't outlive
// the call — it holds a JS reference that's invalid on the libuv
// worker thread).

/// Describe a raw RGBA image. Returns the model's text answer.
#[derive(Debug)]
pub struct VisionDescribeImageTask {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
    user_prompt: String,
}

impl Task for VisionDescribeImageTask {
    type Output = String;
    type JsValue = String;

    fn compute(&mut self) -> NapiResult<Self::Output> {
        phase4::vision_describe_image(&self.rgba, self.width, self.height, &self.user_prompt)
            .map_err(map_phase4_err)
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> NapiResult<Self::JsValue> {
        Ok(output)
    }
}

#[napi(ts_return_type = "Promise<string>")]
pub fn vision_describe_image(
    rgba: Buffer,
    width: u32,
    height: u32,
    user_prompt: String,
) -> AsyncTask<VisionDescribeImageTask> {
    AsyncTask::new(VisionDescribeImageTask {
        rgba: rgba.to_vec(),
        width,
        height,
        user_prompt,
    })
}

/// Describe the image stored on a raster layer node.
#[derive(Debug)]
pub struct VisionDescribeNodeTask {
    node_id: Uuid,
    user_prompt: String,
}

impl Task for VisionDescribeNodeTask {
    type Output = String;
    type JsValue = String;

    fn compute(&mut self) -> NapiResult<Self::Output> {
        phase4::vision_describe_node(self.node_id, &self.user_prompt).map_err(map_phase4_err)
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> NapiResult<Self::JsValue> {
        Ok(output)
    }
}

#[napi(ts_return_type = "Promise<string>")]
pub fn vision_describe_node(
    node_id: String,
    user_prompt: String,
) -> NapiResult<AsyncTask<VisionDescribeNodeTask>> {
    let id = parse_uuid(&node_id)?;
    Ok(AsyncTask::new(VisionDescribeNodeTask {
        node_id: id,
        user_prompt,
    }))
}

/// Generate alt-text for a raw RGBA image.
#[derive(Debug)]
pub struct VisionGenerateAltTextTask {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
}

impl Task for VisionGenerateAltTextTask {
    type Output = String;
    type JsValue = String;

    fn compute(&mut self) -> NapiResult<Self::Output> {
        phase4::vision_generate_alt_text(&self.rgba, self.width, self.height)
            .map_err(map_phase4_err)
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> NapiResult<Self::JsValue> {
        Ok(output)
    }
}

#[napi(ts_return_type = "Promise<string>")]
pub fn vision_generate_alt_text(
    rgba: Buffer,
    width: u32,
    height: u32,
) -> AsyncTask<VisionGenerateAltTextTask> {
    AsyncTask::new(VisionGenerateAltTextTask {
        rgba: rgba.to_vec(),
        width,
        height,
    })
}

/// Generate alt-text for a document raster node, using the VLM.
#[derive(Debug)]
pub struct VisionGenerateAltTextForNodeTask {
    node_id: Uuid,
}

impl Task for VisionGenerateAltTextForNodeTask {
    type Output = String;
    type JsValue = String;

    fn compute(&mut self) -> NapiResult<Self::Output> {
        phase4::vision_generate_alt_text_for_node(self.node_id).map_err(map_phase4_err)
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> NapiResult<Self::JsValue> {
        Ok(output)
    }
}

#[napi(ts_return_type = "Promise<string>")]
pub fn vision_generate_alt_text_for_node(
    node_id: String,
) -> NapiResult<AsyncTask<VisionGenerateAltTextForNodeTask>> {
    let id = parse_uuid(&node_id)?;
    Ok(AsyncTask::new(VisionGenerateAltTextForNodeTask {
        node_id: id,
    }))
}

/// Run a design critique on the given RGBA snapshot.
#[derive(Debug)]
pub struct VisionAnalyzeDesignTask {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
}

impl Task for VisionAnalyzeDesignTask {
    type Output = String;
    type JsValue = String;

    fn compute(&mut self) -> NapiResult<Self::Output> {
        phase4::vision_analyze_design(&self.rgba, self.width, self.height).map_err(map_phase4_err)
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> NapiResult<Self::JsValue> {
        Ok(output)
    }
}

#[napi(ts_return_type = "Promise<string>")]
pub fn vision_analyze_design(
    rgba: Buffer,
    width: u32,
    height: u32,
) -> AsyncTask<VisionAnalyzeDesignTask> {
    AsyncTask::new(VisionAnalyzeDesignTask {
        rgba: rgba.to_vec(),
        width,
        height,
    })
}

/// Extract a brand profile from a reference image. Returns JSON-
/// encoded [`kcreate_ai::brand_extract::BrandExtraction`].
#[derive(Debug)]
pub struct AiExtractBrandFromImageTask {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
}

impl Task for AiExtractBrandFromImageTask {
    type Output = String;
    type JsValue = String;

    fn compute(&mut self) -> NapiResult<Self::Output> {
        let res = phase4::vision_extract_brand(&self.rgba, self.width, self.height)
            .map_err(map_phase4_err)?;
        serde_json::to_string(&res)
            .map_err(|e| NapiError::from_reason(format!("ai_extract_brand: {e}")))
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> NapiResult<Self::JsValue> {
        Ok(output)
    }
}

#[napi(ts_return_type = "Promise<string>")]
pub fn ai_extract_brand_from_image(
    rgba: Buffer,
    width: u32,
    height: u32,
) -> AsyncTask<AiExtractBrandFromImageTask> {
    AsyncTask::new(AiExtractBrandFromImageTask {
        rgba: rgba.to_vec(),
        width,
        height,
    })
}

/// Suggest a content-aware crop. `aspect_ratio` is the desired
/// width/height ratio; pass `0` to let the VLM choose.
#[derive(Debug)]
pub struct AiSuggestCropTask {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
    aspect_ratio: Option<f32>,
}

impl Task for AiSuggestCropTask {
    type Output = String;
    type JsValue = String;

    fn compute(&mut self) -> NapiResult<Self::Output> {
        let res =
            phase4::vision_suggest_crop(&self.rgba, self.width, self.height, self.aspect_ratio)
                .map_err(map_phase4_err)?;
        serde_json::to_string(&res)
            .map_err(|e| NapiError::from_reason(format!("ai_suggest_crop: {e}")))
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> NapiResult<Self::JsValue> {
        Ok(output)
    }
}

#[napi(ts_return_type = "Promise<string>")]
pub fn ai_suggest_crop(
    rgba: Buffer,
    width: u32,
    height: u32,
    aspect_ratio: f64,
) -> AsyncTask<AiSuggestCropTask> {
    #[allow(clippy::cast_possible_truncation)]
    let aspect = if aspect_ratio > 0.0 {
        Some(aspect_ratio as f32)
    } else {
        None
    };
    AsyncTask::new(AiSuggestCropTask {
        rgba: rgba.to_vec(),
        width,
        height,
        aspect_ratio: aspect,
    })
}

/// Suggest a starter design-token set (spacing, colors, typography)
/// for the given artboard snapshot.
#[derive(Debug)]
pub struct AiSuggestDesignTokensTask {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
}

impl Task for AiSuggestDesignTokensTask {
    type Output = String;
    type JsValue = String;

    fn compute(&mut self) -> NapiResult<Self::Output> {
        let res = phase4::vision_suggest_design_tokens(&self.rgba, self.width, self.height)
            .map_err(map_phase4_err)?;
        serde_json::to_string(&res)
            .map_err(|e| NapiError::from_reason(format!("ai_suggest_design_tokens: {e}")))
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> NapiResult<Self::JsValue> {
        Ok(output)
    }
}

#[napi(ts_return_type = "Promise<string>")]
pub fn ai_suggest_design_tokens(
    rgba: Buffer,
    width: u32,
    height: u32,
) -> AsyncTask<AiSuggestDesignTokensTask> {
    AsyncTask::new(AiSuggestDesignTokensTask {
        rgba: rgba.to_vec(),
        width,
        height,
    })
}

/// Describe the visual style of an image.
#[derive(Debug)]
pub struct AiDescribeStyleTask {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
}

impl Task for AiDescribeStyleTask {
    type Output = String;
    type JsValue = String;

    fn compute(&mut self) -> NapiResult<Self::Output> {
        let res = phase4::vision_describe_style(&self.rgba, self.width, self.height)
            .map_err(map_phase4_err)?;
        serde_json::to_string(&res)
            .map_err(|e| NapiError::from_reason(format!("ai_describe_style: {e}")))
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> NapiResult<Self::JsValue> {
        Ok(output)
    }
}

#[napi(ts_return_type = "Promise<string>")]
pub fn ai_describe_style(rgba: Buffer, width: u32, height: u32) -> AsyncTask<AiDescribeStyleTask> {
    AsyncTask::new(AiDescribeStyleTask {
        rgba: rgba.to_vec(),
        width,
        height,
    })
}

/// Recommended vision pack for the current device tier + platform.
/// Empty string when the registry has no recommendation.
#[napi]
pub fn vision_recommended_pack() -> String {
    phase4::vision_recommended_pack().unwrap_or_default()
}

/// Inverse lookup: given a vision pack id, return the mmproj
/// companion id, or empty string for MLX packs that don't need one.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn vision_mmproj_for(pack_id: String) -> String {
    phase4::vision_mmproj_for(pack_id).unwrap_or_default()
}

/// Pack ids the renderer is allowed to show in the vision section
/// of the Model Manager (after platform + tier filtering).
#[napi]
pub fn vision_listable_packs() -> Vec<String> {
    phase4::vision_listable_packs()
}

// ----- Image generation -----

/// Start the image-generation sidecar. Hard-gated on
/// `RuntimeConfig::image_generation_allowed()`.
#[napi]
#[allow(clippy::needless_pass_by_value)]
pub fn image_gen_start(pack_id: String) -> NapiResult<u32> {
    let port = phase4::image_gen_start(pack_id).map_err(map_phase4_err)?;
    Ok(u32::from(port))
}

/// Stop the image-generation sidecar.
#[napi]
pub fn image_gen_stop() {
    phase4::image_gen_stop();
}

/// JSON-encoded [`phase4::ImageGenStatusInfo`].
#[napi]
pub fn image_gen_status() -> NapiResult<String> {
    serde_json::to_string(&phase4::image_gen_status())
        .map_err(|e| NapiError::from_reason(format!("image_gen_status: {e}")))
}

/// `napi::Task` for `image_gen_generate`. FLUX diffusion runs for
/// tens of seconds even on a Tier-2 GPU; the main process must stay
/// responsive while it does.
#[derive(Debug)]
pub struct ImageGenGenerateTask {
    prompt: String,
    width: u32,
    height: u32,
    steps: u32,
    seed: Option<u64>,
}

impl Task for ImageGenGenerateTask {
    type Output = String;
    type JsValue = String;

    fn compute(&mut self) -> NapiResult<Self::Output> {
        let out = phase4::image_gen_generate(
            std::mem::take(&mut self.prompt),
            self.width,
            self.height,
            self.steps,
            self.seed,
        )
        .map_err(map_phase4_err)?;
        serde_json::to_string(&out)
            .map_err(|e| NapiError::from_reason(format!("image_gen_generate: {e}")))
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> NapiResult<Self::JsValue> {
        Ok(output)
    }
}

/// Generate an image. Returns JSON-encoded
/// [`phase4::GeneratedImagePayload`] (PNG bytes as base64). Runs on
/// a worker thread; resolves a JS `Promise<string>`.
///
/// Seed handling: N-API surfaces this parameter as TS `number | null`,
/// which arrives in Rust as `Option<i64>`. Diffusion seeds are
/// unsigned (the Python server passes them straight into
/// `torch.Generator().manual_seed`, which accepts any non-negative
/// integer). The renderer's input handler already strips non-digits
/// so it can't *generate* a negative seed, but we reject any negative
/// value explicitly here rather than silently abs-valuing — a direct
/// IPC caller (plugins, scripted tests) passing `seed: -1` got
/// `seed = 1` under the previous `i64::unsigned_abs` mapping, which
/// is a lossy silent transform on a public bridge function. Fail
/// loudly instead so the divergence is surfaced at the call site.
#[napi(ts_return_type = "Promise<string>")]
pub fn image_gen_generate(
    prompt: String,
    width: u32,
    height: u32,
    steps: u32,
    seed: Option<i64>,
) -> NapiResult<AsyncTask<ImageGenGenerateTask>> {
    let seed = match seed {
        None => None,
        Some(s) if s >= 0 => Some(s as u64),
        Some(s) => {
            return Err(NapiError::from_reason(format!(
                "image_gen_generate: seed must be a non-negative integer, got {s}"
            )));
        }
    };
    Ok(AsyncTask::new(ImageGenGenerateTask {
        prompt,
        width,
        height,
        steps,
        seed,
    }))
}

/// Is image generation allowed at all on this device? Mirrors
/// `RuntimeConfig::image_generation_allowed`.
#[napi]
pub fn image_gen_allowed() -> bool {
    phase4::image_gen_allowed()
}

/// Recommended image-generation pack id. Empty string when not
/// allowed on this device.
#[napi]
pub fn image_gen_recommended_pack() -> String {
    phase4::image_gen_recommended_pack().unwrap_or_default()
}
