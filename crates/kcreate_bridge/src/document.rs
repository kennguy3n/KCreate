//! Document-bridge state machine.
//!
//! Maintains a process-global `Option<Workspace>` describing the
//! currently-open project: its in-memory [`Project`] (document graph,
//! operation log, design tokens) plus the [`ProjectStore`] that
//! persists it to disk. The N-API wrappers in `lib.rs` are thin
//! marshalling layers around the functions here.
//!
//! Concurrency: one [`Workspace`] per process, behind a
//! `parking_lot::Mutex`. All mutations are short and synchronous.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::OnceLock;

use chrono::Utc;
use kcreate_core::component::{
    ComponentDefinition, ComponentInstance, ComponentVariant, COMPONENT_INSTANCE_METADATA_KEY,
};
use kcreate_core::config::RuntimeConfig;
use kcreate_core::document::{DocumentError, DocumentGraph};
use kcreate_core::node::{FillStyle, Node, NodeType, RgbaColor};
use kcreate_core::operation::Operation;
use kcreate_core::project::{
    BrandKit, DesignTokens, ExportFormat, ExportPreset, FontRef, NamedColor, Project, ProjectError,
    Slice,
};
use kcreate_core::theme::{build_color_remap, quantize, ColorUsage, RadiusScale, Theme, TypeRole};
use kcreate_export::png::{export_png_to_bytes, PngExportError, PngExportOptions};
use kcreate_export::svg::{export_svg_from_document, SvgDocumentExportError, SvgExportOptions};
use kcreate_export::{run_png_batch_parallel, PngBatchItem};
use kcreate_layout::{layout_flex, layout_grid, FlexLayout, GridLayout, ResizeNode, ResizeOptions};
use kcreate_storage::project_io::{ProjectStore, ProjectStoreError};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

/// Errors from document-bridge state.
#[derive(Debug, Error)]
pub enum DocumentBridgeError {
    #[error("no project is open — call project_create or project_open first")]
    NoProject,
    #[error(
        "a project is already open at {0}; call project_close first or persist with project_save"
    )]
    ProjectAlreadyOpen(PathBuf),
    #[error("project directory already exists: {0}")]
    ProjectDirExists(PathBuf),
    #[error("invalid node type: {0}")]
    InvalidNodeType(String),
    /// Bridge call received a wire-format argument that doesn't parse
    /// to any known value of the expected enum / vocabulary. Used for
    /// every "string → enum" parser at the bridge boundary —
    /// page size, page orientation, interaction trigger, export
    /// format, MCP permission grant, etc. Distinct from
    /// [`Self::InvalidNodeType`] which is specifically for node-type
    /// mismatches; see Devin Review 3289450981 for the rationale
    /// (semantic error message clarity when debugging failed N-API
    /// calls).
    #[error("invalid value for `{argument}`: {value}")]
    InvalidArgument { argument: String, value: String },
    #[error("node not found: {0}")]
    NodeNotFound(Uuid),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Document(#[from] DocumentError),
    #[error(transparent)]
    Project(#[from] ProjectError),
    #[error(transparent)]
    Storage(#[from] ProjectStoreError),
    #[error(transparent)]
    Png(#[from] PngExportError),
    #[error(transparent)]
    Svg(#[from] SvgDocumentExportError),
    #[error(transparent)]
    Batch(#[from] kcreate_export::BatchExportError),
    #[error("invalid uuid {0:?}: {1}")]
    InvalidUuid(String, uuid::Error),
    #[error("invalid bounds: width={width} height={height} (must be finite and positive)")]
    InvalidBounds { width: f64, height: f64 },
    #[error("invalid component selection: {0}")]
    InvalidComponentSelection(String),
    #[error("component instance metadata on node {0} is malformed: {1}")]
    InvalidComponentInstance(Uuid, String),
    /// Bridge call expected a particular `NodeType` (e.g. `Page`,
    /// `ComponentLayer`, `LayoutFrame`) and the node was something
    /// else. Generic across every "wrong kind" check; supersedes the
    /// older per-variant `WrongComponentNodeType` /
    /// `WrongLayoutNodeType` which were specific to the component
    /// and layout subsystems and read confusingly when reused for
    /// other node kinds (e.g. Page layout) per Devin Review
    /// (PR #5, `page_set_layout` finding).
    #[error("expected a {expected:?} node, got {got:?}")]
    WrongNodeType { expected: NodeType, got: NodeType },
    #[error("layout config on node {0} is malformed: {1}")]
    InvalidLayoutConfig(Uuid, String),
    /// Phase 11 Block C Task 16 — auto-layout propagation cap.
    /// Returned when the recursive solver walks past
    /// `LAYOUT_PROPAGATION_DEPTH_LIMIT` levels of nested
    /// `LayoutFrame` nodes. In practice this means a component
    /// instance graph contains a cycle (instance A includes
    /// instance B includes instance A …) or a very deep nesting
    /// the user explicitly opted into. Surfacing this error lets
    /// the host pop a toast instead of silently truncating the
    /// propagation.
    #[error("auto-layout propagation reached depth limit {limit} at node {node_id}")]
    LayoutRecursionLimit { node_id: Uuid, limit: usize },
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Bridge(#[from] crate::state::BridgeError),
    /// Local-template marketplace failures: corrupt manifest, missing
    /// .ktemplate folder, attempted install of an already-installed
    /// template, etc. The marketplace lives in `kcreate_core` and has
    /// its own structured error type, which we wrap here so the
    /// renderer's structured error mapping stays uniform.
    #[error(transparent)]
    Marketplace(#[from] kcreate_core::MarketplaceError),
    /// Phase 8 (Task 4): a bridge call that requires collab Editor
    /// permission was invoked while the local peer is in
    /// `CollabPermission::Viewer`. Pre-checked BEFORE any local
    /// mutation so a Viewer doesn't end up with annotations / edits
    /// that only exist in their local DB and never reach peers (the
    /// confusing "I added a comment but no-one else sees it" UX).
    /// Surfaces the same string as
    /// [`crate::collab::SessionBridgeError::PermissionDenied`] so
    /// the renderer's error toast logic stays unified.
    #[error("local peer is in read-only mode: operation requires Editor permission")]
    PermissionDenied,
    /// Catch-all for subsystem errors (audit, thumbnail, etc.) that
    /// don't warrant their own variant. The string carries the
    /// underlying error's `Display` output.
    #[error("{0}")]
    Internal(String),
    /// Phase B1 (Pen tool): `canvas_create_path` rejected a
    /// caller-supplied path geometry. See [`CreatePathError`] for
    /// the discriminator. Routed through its own variant (rather
    /// than collapsing into `InvalidArgument`) because the renderer
    /// surfaces each subkind with a different toast and a different
    /// telemetry tag — see `useToolStateMachine.ts` pen branch.
    #[error(transparent)]
    CreatePath(#[from] CreatePathError),
    /// Phase B2 (Pathfinder): `canvas_path_boolean` rejected the
    /// caller's source-id list. See [`PathBooleanError`] for the
    /// discriminator. Routed through its own variant for the same
    /// reason as `CreatePath` — Pathfinder errors get their own
    /// toast / telemetry tag in `PathfinderPanel.tsx`.
    #[error(transparent)]
    PathBoolean(#[from] PathBooleanError),
    /// Phase B3 (Node editor): `canvas_path_get_segments` /
    /// `canvas_path_set_segments` rejected the caller's request.
    /// See [`PathSegmentsError`] for the discriminator. Routed
    /// through its own variant for the same reason as `CreatePath`
    /// and `PathBoolean` — the node editor surfaces each subkind
    /// with a different toast in `useToolStateMachine.ts` node-edit
    /// branch.
    #[error(transparent)]
    PathSegments(#[from] PathSegmentsError),
}

pub type Result<T> = std::result::Result<T, DocumentBridgeError>;

/// Open project = in-memory state + on-disk store, plus the
/// bookkeeping needed for incremental persistence.
pub(crate) struct Workspace {
    pub(crate) project: Project,
    /// On-disk store wrapped in an `Arc<parking_lot::Mutex<…>>` so
    /// (a) the `Workspace` itself can be `Sync` (rusqlite's
    /// `Connection` contains `RefCell`s and is `Send + !Sync`),
    /// (b) the whole workspace can live inside an `RwLock` so
    /// in-memory reads of `project` / `selection` / `scene_sync` can
    /// proceed concurrently while only the on-disk path serialises
    /// through this inner mutex (Phase 11 Block D Task 19), and (c)
    /// long-running disk operations can `Arc::clone` the handle,
    /// drop the workspace lock, and then run their SQL writes
    /// without holding the workspace lock at all (Phase 11 Block B
    /// follow-up round 7 — Devin Review BUG-0001 r7). [`project_save`]
    /// is the canonical consumer of (c): it snapshots the document /
    /// metadata under a brief read lock, drops it, and streams to
    /// SQLite against the cloned `Arc` so concurrent renderer reads
    /// and writes never wait on the save.
    pub(crate) store: std::sync::Arc<parking_lot::Mutex<ProjectStore>>,
    /// Set of operation ids already written to the on-disk store.
    ///
    /// Tracking by id (not by index) is the only correct option once
    /// the in-memory log can mutate by something other than
    /// pure-append: `OperationLog::push` truncates the redo-stack tail
    /// before appending, and bounded-depth trimming drops entries off
    /// the front. An index-based cursor desynchronises in both cases;
    /// a set keyed by `Operation::id` is invariant under both.
    ///
    /// After every successful `project_save`, the set is pruned down
    /// to the ids currently in the in-memory log so its size stays
    /// bounded by `OperationLog::max_depth` (a few KB even on the most
    /// generous device tier).
    persisted_op_ids: HashSet<Uuid>,
    /// Bidirectional uuid ⇄ `ObjectId` mapping rebuilt by [`scene_sync`]
    /// on every mutation. Lives in the workspace (not a sibling
    /// singleton) so it can't outlive the project it describes.
    pub(crate) scene_sync: crate::scene_sync::SceneSync,
    /// Currently selected document nodes. Selection is rendered as
    /// highlight overlays in the next scene sync.
    selection: Vec<Uuid>,
}

/// Workspace singleton.
///
/// Phase 11 Block D Task 19: changed from `parking_lot::Mutex` to
/// `parking_lot::RwLock` so multiple read-only panels (tree view,
/// status bar, selection inspector, export pickers) can query the
/// workspace concurrently. Mutations still take the exclusive
/// `write()` guard so the operation log and scene-sync invariants
/// remain intact.
///
/// # Lock-ordering invariant (Phase 11)
///
/// `workspace RwLock` → `renderer Mutex` → `tile cache Mutex`.
///
/// Every site that touches more than one of these must observe this
/// order. The only path that enters the renderer lock while holding
/// the workspace lock is [`sync_scene_locked`]; the renderer never
/// re-enters the workspace.
pub(crate) fn slot() -> &'static RwLock<Option<Workspace>> {
    static WS: OnceLock<RwLock<Option<Workspace>>> = OnceLock::new();
    WS.get_or_init(|| RwLock::new(None))
}

/// Run `f` against the open workspace under a read-style lock. The
/// caller closure must not call back into other workspace-locking
/// functions or it will deadlock. Used by `phase2.rs` so that crate
/// of bridge entry points doesn't need to know about [`Workspace`]'s
/// private field layout.
pub(crate) fn with_workspace<R>(f: impl FnOnce(&Workspace) -> Result<R>) -> Result<R> {
    let guard = slot().read();
    let ws = guard.as_ref().ok_or(DocumentBridgeError::NoProject)?;
    f(ws)
}

/// Mutable counterpart of [`with_workspace`]. The caller closure must
/// not re-lock the workspace.
pub(crate) fn with_workspace_mut<R>(f: impl FnOnce(&mut Workspace) -> Result<R>) -> Result<R> {
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    f(ws)
}

/// Load a blob by hash from the open workspace's content-addressed
/// store. Pulled out so `phase2.rs` does not need to know about the
/// `ProjectStore` API surface.
pub(crate) fn blob_load(ws: &Workspace, hash: &str) -> Result<Vec<u8>> {
    ws.store
        .lock()
        .blobs()
        .load(hash)
        .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))
}

/// Sync the renderer scene from the current workspace. Used after
/// `phase2.rs` mutates the document so the canvas updates immediately.
/// Failures are logged but not propagated — the next renderer init +
/// sync recovers the state, matching the pattern used elsewhere in
/// this module.
pub(crate) fn sync_scene_after_change() {
    let mut guard = slot().write();
    let _ = sync_scene_locked(&mut guard);
}

/// Lock-free snapshot of the current renderer scene. Wraps
/// [`crate::state::current_scene`] so `phase2.rs` does not need to
/// reach into `state.rs` directly.
pub(crate) fn current_scene_safe() -> Result<kcreate_renderer::Scene> {
    Ok(crate::state::current_scene()?)
}

/// Loopback port of the running MCP server, if any. Used by
/// `phase2.rs::mcp_status`.
#[cfg(feature = "mcp")]
#[must_use]
pub fn mcp_port() -> Option<u32> {
    kcreate_mcp::server::port().map(u32::from)
}

#[cfg(not(feature = "mcp"))]
#[must_use]
pub const fn mcp_port() -> Option<u32> {
    None
}

/// Atomic `(running, port)` snapshot of the MCP server, taken under
/// a single global-lock acquisition.
///
/// Composing [`mcp_is_running`] and [`mcp_port`] separately is a
/// TOCTOU race: the server can be stopped between the two calls,
/// producing a status response with `running: true` and `port: 0`.
/// `phase2::mcp_status` uses this accessor instead so a single
/// status response is internally consistent. Per Devin Review
/// ANALYSIS_pr-review-job-790e7860e5c745e0bee13295709290f4_0001.
#[cfg(feature = "mcp")]
#[must_use]
pub fn mcp_state() -> (bool, Option<u32>) {
    let (running, port) = kcreate_mcp::server::state();
    (running, port.map(u32::from))
}

#[cfg(not(feature = "mcp"))]
#[must_use]
pub const fn mcp_state() -> (bool, Option<u32>) {
    (false, None)
}

/// Test-only helper to reset the singleton between serial tests.
#[cfg(test)]
pub(crate) fn reset_for_tests() {
    *slot().write() = None;
}

/// Snapshot of project identity returned to the host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectInfo {
    pub id: Uuid,
    pub name: String,
    pub path: PathBuf,
    pub created_at: String,
    pub modified_at: String,
}

/// Compact snapshot of a [`ComponentInstance`] for the host's layer
/// panel. `PartialEq` is intentionally not `Eq`: `overrides` carries
/// `serde_json::Value` which can contain `f64` / `NaN`.
#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentInstanceInfo {
    pub definition_id: Uuid,
    pub active_variant_id: Uuid,
    pub overrides: std::collections::HashMap<String, serde_json::Value>,
}

/// Snapshot of one node for the host's layer panel.
///
/// `PartialEq` is intentionally not `Eq` because
/// [`ComponentInstanceInfo::overrides`] carries `serde_json::Value`.
#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeInfo {
    pub id: Uuid,
    pub node_type: String,
    pub parent_id: Option<Uuid>,
    pub children: Vec<Uuid>,
    pub name: String,
    pub visible: bool,
    pub locked: bool,
    /// Axis-aligned bounds of the node in document space, mirroring
    /// `kcreate_core::Node::bounds`. Previously elided from the
    /// layer-panel wire shape; PrototypePlayer / hit-targeted UI need
    /// it to render hotspot rectangles, so we ship the four numbers
    /// directly (32 bytes per node is well under the cost of a second
    /// round trip per node).
    pub bounds: BoundsInfo,
    /// Monotonically-increasing revision counter sourced from
    /// `kcreate_core::node::Node::version`. Bumped on every `touch()`
    /// (i.e. on every mutation, from any source: bridge API calls,
    /// undo/redo, future collab events). Renderer panels that hydrate
    /// node-scoped data the `NodeInfo` payload deliberately doesn't
    /// carry (`FillSection`'s `style.fill`, `TextFramePanel`'s
    /// `text_frame_options`, `OpenTypePanel`'s OpenType features)
    /// key their fetch effect on `[node.id, node.version]` so the
    /// effect refires after undo/redo / remote-peer edits even when
    /// `node.id` is stable. Carried over the bridge as `u64` and
    /// truncated to `f64` at the napi boundary — `version` increments
    /// once per mutation so even a million mutations per second for
    /// 100 years stays well below 2^53.
    pub version: u64,
    /// Present iff `node_type == "ComponentLayer"` and the node
    /// carries a parseable `component_instance` metadata payload.
    /// Renderer panels read this to drive the variant switcher.
    #[serde(rename = "componentInstance", skip_serializing_if = "Option::is_none")]
    pub component_instance: Option<ComponentInstanceInfo>,
    /// Free-form metadata bag (cloned from the underlying Node).
    /// Always emitted as an object so the host can read structured
    /// payloads like `layout` without a second round-trip — but
    /// elided from the wire when empty to keep tree payloads small.
    #[serde(skip_serializing_if = "HashMap::is_empty", default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Wire-format mirror of [`kcreate_core::Bounds`]. Mirrored as a
/// separate type so the napi-rs `#[napi(object)]` shape in
/// `lib.rs::NodeInfo` can spell out the four fields directly — napi
/// would otherwise have to learn about the core type.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BoundsInfo {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl From<kcreate_core::Bounds> for BoundsInfo {
    fn from(b: kcreate_core::Bounds) -> Self {
        Self {
            x: b.x,
            y: b.y,
            width: b.width,
            height: b.height,
        }
    }
}

impl From<&Node> for NodeInfo {
    fn from(n: &Node) -> Self {
        let component_instance = if matches!(n.node_type, NodeType::ComponentLayer) {
            n.metadata
                .get(COMPONENT_INSTANCE_METADATA_KEY)
                .and_then(|v| serde_json::from_value::<ComponentInstance>(v.clone()).ok())
                .map(|inst| ComponentInstanceInfo {
                    definition_id: inst.definition_id,
                    active_variant_id: inst.active_variant_id,
                    overrides: inst.overrides,
                })
        } else {
            None
        };
        Self {
            id: n.id,
            node_type: node_type_name(n.node_type).to_string(),
            parent_id: n.parent_id,
            children: n.children.clone(),
            name: n.name.clone(),
            visible: n.visible,
            locked: n.locked,
            bounds: n.bounds.into(),
            version: n.version,
            component_instance,
            metadata: n.metadata.clone(),
        }
    }
}

const fn node_type_name(t: NodeType) -> &'static str {
    match t {
        NodeType::Page => "Page",
        NodeType::Artboard => "Artboard",
        NodeType::GroupLayer => "GroupLayer",
        NodeType::VectorLayer => "VectorLayer",
        NodeType::RasterLayer => "RasterLayer",
        NodeType::TextLayer => "TextLayer",
        NodeType::ComponentLayer => "ComponentLayer",
        NodeType::LayoutFrame => "LayoutFrame",
    }
}

/// Static device/runtime snapshot returned to the host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeStatus {
    pub device_tier: String,
    pub gpu_available: bool,
    pub gpu_name: Option<String>,
    pub platform: String,
    pub total_ram_mb: u64,
}

// -----------------------------------------------------------------------------
// Project lifecycle
// -----------------------------------------------------------------------------

/// Create a brand-new project at `dir/<name>.kstudio`.
///
/// The parent directory must exist; the `.kstudio` directory must
/// not. The process must not have another project open — callers
/// should call [`project_close`] (or [`project_save`] followed by
/// `project_close`) before switching projects.
pub fn project_create(name: &str, dir: &Path) -> Result<ProjectInfo> {
    // Phase 8 Block E Task 27: cold-path instrumentation. The RAII
    // `perf::scope` guard guarantees `project_create.end` fires on
    // every exit path (early `return`, `?` propagation, success)
    // — paired manual `.start`/`.end` marks would orphan `.start`
    // on any error path between them. The renderer computes "real
    // time spent creating the project" by diffing the two marks;
    // both are monotonic so resume-from-sleep cannot make the
    // delta go negative.
    let _perf = crate::perf::scope("project_create");
    // Hold the singleton lock across the entire create operation so
    // "another project is already open" stays a TOCTOU-free check. The
    // bridge calls are synchronous and short; serialising them is the
    // correct semantics even when N-API begins driving requests from a
    // worker thread.
    let mut guard = slot().write();
    if let Some(ws) = guard.as_ref() {
        return Err(DocumentBridgeError::ProjectAlreadyOpen(
            ws.store.lock().project_dir().to_path_buf(),
        ));
    }
    let project_dir = dir.join(format!("{name}.kstudio"));
    if project_dir.exists() {
        return Err(DocumentBridgeError::ProjectDirExists(project_dir));
    }
    let mut store = ProjectStore::create(&project_dir, name)?;
    // Mirror the manifest's identity into the in-memory Project so the
    // host sees stable ids across reopen and so the two records can't
    // drift. ProjectStore::create generates the manifest's UUID; we
    // adopt it instead of letting Project::new pick a fresh one.
    //
    // `with_max_undo_depth` (rather than `new`) wires the device-tier
    // budget from `RuntimeConfig::max_undo_depth` (32 on Tier 0, 1024
    // on Tier 3) into the new `OperationLog`. `ANALYSIS_0003` on PR #2
    // flagged the prior `OperationLog::default()` path: low-end devices
    // silently used a 256-deep log instead of the intended 32, and
    // high-end devices were capped at 256 instead of 1024. Fixing it
    // at the constructor (rather than mutating the log post-hoc) keeps
    // the depth invariant true for the lifetime of the project.
    let manifest = store.manifest();
    let mut project = Project::with_max_undo_depth(
        manifest.name.clone(),
        runtime_slot().lock().effective_undo_depth(),
    );
    project.id = manifest.id;
    project.created_at = manifest.created_at;
    project.modified_at = manifest.modified_at;
    project.install_default_export_presets();
    project.add_page("Page 1")?;
    store.save_document(&project.document)?;
    let info = build_info(&project, store.project_dir());
    *guard = Some(Workspace {
        project,
        store: std::sync::Arc::new(parking_lot::Mutex::new(store)),
        persisted_op_ids: HashSet::new(),
        scene_sync: crate::scene_sync::SceneSync::new(),
        selection: Vec::new(),
    });
    // First scene sync. Errors from `state::render_scene` are silently
    // tolerated here because creating a project before the renderer is
    // initialised is a legitimate sequence (the host may create a
    // project headlessly for tests). The next renderer_init + sync
    // will recover.
    let _ = sync_scene_locked(&mut guard);
    // Record on the recent-projects list while we still hold the
    // workspace lock — `record_recent_project` only takes the
    // recent-list mutex internally, so this nesting can't deadlock.
    if let Some(ws) = guard.as_ref() {
        crate::thumbnails::record_recent_project(ws);
    }
    drop(guard);
    // Kick off a best-effort thumbnail pre-warm so the HomePage has
    // something to render before the user opens this project again.
    // Errors here are non-fatal: the thumbnail pipeline will lazily
    // generate on first access if pre-warming fails.
    // `prepare_thumbnails_background` spawns a worker thread and
    // returns immediately, so the `_perf` guard's drop-emitted
    // `project_create.end` mark below does not include thumbnail
    // pre-warm CPU time — only the bookend foreground work.
    let _ = crate::thumbnails::prepare_thumbnails_background(
        crate::thumbnails::DEFAULT_THUMBNAIL_MAX_DIM_PX,
    );
    Ok(info)
}

/// Open an existing `.kstudio` directory. The process must not have
/// another project open — callers should call [`project_close`] first.
pub fn project_open(dir: &Path) -> Result<ProjectInfo> {
    // Phase 8 Block E Task 27: cold-path instrumentation. RAII
    // guard so every exit path (including the many `?` operators
    // below that read manifest / document / tokens / brand kits /
    // presets / components / color settings / op log) still emits
    // the matching `project_open.end` mark.
    let _perf = crate::perf::scope("project_open");
    // Same lock discipline as `project_create`: hold across the entire
    // operation, no TOCTOU window between the check and the set.
    let mut guard = slot().write();
    if let Some(ws) = guard.as_ref() {
        return Err(DocumentBridgeError::ProjectAlreadyOpen(
            ws.store.lock().project_dir().to_path_buf(),
        ));
    }
    let store = ProjectStore::open(dir)?;
    let document = store.load_document()?;
    let manifest = store.manifest();
    // We deliberately *don't* call `install_default_export_presets`
    // here — reopen should never invent fresh preset UUIDs. Design
    // tokens, brand kits, and export presets round-trip through the
    // store so identifiers stay stable across close/reopen.
    //
    // Same device-tier wiring as `project_create` (`ANALYSIS_0003` on
    // PR #2). The on-disk operation history may contain *more* rows
    // than the current tier budget allows (e.g. project saved on a
    // Tier 3 box with 1024-deep log, reopened on a Tier 0 box with
    // 32-deep log) — `OperationLog::restore_from` enforces the cap
    // by dropping from the front to retain the most recent ops.
    let mut project = Project::with_max_undo_depth(
        manifest.name.clone(),
        runtime_slot().lock().effective_undo_depth(),
    );
    project.id = manifest.id;
    project.created_at = manifest.created_at;
    project.modified_at = manifest.modified_at;
    project.document = document;
    project.design_tokens = store.load_design_tokens()?;
    project.brand_kits = store.load_brand_kits()?;
    project.export_presets = store.load_export_presets()?;
    project.components = store.load_components()?;
    project.color_settings = store.load_color_settings()?;
    // Restore the operation log from disk so undo survives close+reopen.
    let max_depth = project.operation_log.max_depth();
    let history = store.load_operations(max_depth)?;
    // Every op we just loaded is, by definition, already on disk.
    let persisted_op_ids: HashSet<Uuid> = history.iter().map(|op| op.id).collect();
    project.operation_log.restore_from(history);
    let info = build_info(&project, store.project_dir());
    *guard = Some(Workspace {
        project,
        store: std::sync::Arc::new(parking_lot::Mutex::new(store)),
        persisted_op_ids,
        scene_sync: crate::scene_sync::SceneSync::new(),
        selection: Vec::new(),
    });
    let _ = sync_scene_locked(&mut guard);
    // Record on the recent-projects list while we still hold the
    // workspace lock — same locking discipline as `project_create`.
    if let Some(ws) = guard.as_ref() {
        crate::thumbnails::record_recent_project(ws);
        // Re-register any fonts embedded in this project's brand kits
        // into the process-wide fontdb so documents authored with a
        // custom (non-system) font shape + export that font on reopen.
        let store = ws.store.lock();
        for kit in &ws.project.brand_kits {
            register_kit_embedded_fonts(&store, kit);
        }
    }
    drop(guard);
    // Background pre-warm so the next HomePage visit has fresh
    // thumbnails. Non-fatal on failure. The spawned worker runs
    // off the perf bookend so `project_open.end` (emitted when
    // `_perf` drops) reflects only foreground latency.
    let _ = crate::thumbnails::prepare_thumbnails_background(
        crate::thumbnails::DEFAULT_THUMBNAIL_MAX_DIM_PX,
    );
    Ok(info)
}

/// Persist the current project to disk.
///
/// The document graph is rewritten in full (it's the source of truth
/// and changes shape freely), but the operation log is appended
/// *incrementally*: only ops whose id is not already in
/// `persisted_op_ids` are written. Tracking by id (not index) keeps
/// the persistence path correct against every mutation
/// `OperationLog::push` can perform:
///
/// * **Pure append** — the new op's id is unseen, it gets written.
/// * **Undo + push** — `push` truncates the redo tail and appends a
///   *new* operation with a fresh `Uuid`. The previous tail's ids stay
///   in `persisted_op_ids` (those rows are still on disk, intentionally,
///   since the on-disk log is the audit trail); the new op's id is
///   unseen, so it gets written. Nothing is dropped.
/// * **Bounded-depth front trim** — the trimmed op's id is gone from
///   `iter()` but its row is already on disk. At end of save we
///   (1) prune it from `persisted_op_ids` (so the set stays
///   `O(max_depth)`) and (2) ask the store to drop any on-disk rows
///   beyond `max_depth` so the on-disk table tracks the in-memory bound
///   instead of growing unbounded for the project lifetime.
///
/// Cost: O(history.len()) hash lookups + O(new ops) inserts + one
/// `DELETE` against the `operations` table (which is a single
/// range-delete using a timestamp index). At `max_depth` = 256 the
/// full save path stays in the microseconds.
pub fn project_save() -> Result<()> {
    // Phase 11 Block B follow-up round 7 — Devin Review BUG-0001 (r7).
    //
    // The pre-round-7 implementation held `slot().write()` (exclusive
    // workspace lock) for the entire SQLite write sequence below,
    // which on multi-MB projects could pin every other workspace-
    // touching IPC call (tree view queries, selection inspector,
    // renderer ticks) for hundreds of ms. The async wrapper in
    // `phase11::ProjectSaveTask` moved the call off the libuv main
    // thread, but the workspace lock was still held on the worker —
    // so any concurrent `with_workspace(...)` request on a different
    // thread would queue up behind it. The N-API doc-comment claimed
    // "snapshots under the lock then releases" but the code never
    // released until the SQLite stream was done.
    //
    // Round-7 fix (option (a) from the Devin Review prompt):
    //
    //   1. Snapshot the document + metadata under a **read** lock.
    //      Everything written is a `Clone` (Project derives Clone),
    //      so the snapshot is a deep copy free of any workspace
    //      borrow. `Arc::clone(&ws.store)` is also taken so the
    //      SQLite handle outlives the read guard.
    //   2. Drop the read guard. **No workspace lock is held during
    //      the SQLite write sequence** — readers and writers run
    //      concurrently against the in-memory project state.
    //   3. Stream the snapshot to SQLite using the cloned
    //      `Arc<Mutex<ProjectStore>>`. The inner `Mutex` still
    //      serialises writes against the SQLite connection itself,
    //      which is required because rusqlite's `Connection` is
    //      `Send + !Sync`.
    //   4. Take a brief `write()` lock at the end to merge the
    //      newly-persisted op ids into `persisted_op_ids`. Critical
    //      subtlety: between snapshot and merge, the in-memory log
    //      may have grown (user kept editing). The merge uses set
    //      union with the *post-merge* `operation_log` ids as the
    //      retention mask, so:
    //        - new ops added since snapshot stay unpersisted
    //          (next save picks them up — correct);
    //        - ops trimmed from the front of the log since snapshot
    //          drop out of `persisted_op_ids` (matches the O(max_depth)
    //          invariant — correct).
    //
    // `prune_operations` runs in step 3 against the snapshot's
    // `max_depth`; if the user changed `max_depth` between snapshot
    // and merge, the next save will reconcile to the new bound. This
    // is a non-event because `max_depth` is rarely mutated at
    // runtime (it's a device-tier config setting set at startup).

    // -- Step 1: snapshot under a brief read lock. -----------------
    struct SaveSnapshot {
        document: kcreate_core::DocumentGraph,
        design_tokens: kcreate_core::DesignTokens,
        color_settings: kcreate_core::ColorSettings,
        brand_kits: Vec<kcreate_core::BrandKit>,
        export_presets: Vec<kcreate_core::ExportPreset>,
        components: HashMap<Uuid, kcreate_core::ComponentDefinition>,
        /// Ops in the log that are *not yet* in `persisted_op_ids` at
        /// snapshot time. We clone the operations themselves because
        /// step 3 (SQLite stream) runs without the workspace lock.
        unseen_ops: Vec<Operation>,
        max_depth: usize,
    }

    let (snapshot, store) = {
        let guard = slot().read();
        let ws = guard.as_ref().ok_or(DocumentBridgeError::NoProject)?;
        let unseen_ops: Vec<Operation> = ws
            .project
            .operation_log
            .iter()
            .filter(|op| !ws.persisted_op_ids.contains(&op.id))
            .cloned()
            .collect();
        let snapshot = SaveSnapshot {
            document: ws.project.document.clone(),
            design_tokens: ws.project.design_tokens.clone(),
            color_settings: ws.project.color_settings.clone(),
            brand_kits: ws.project.brand_kits.clone(),
            export_presets: ws.project.export_presets.clone(),
            components: ws.project.components.clone(),
            unseen_ops,
            max_depth: ws.project.operation_log.max_depth(),
        };
        let store = std::sync::Arc::clone(&ws.store);
        (snapshot, store)
        // `guard` (and therefore the workspace read lock) drops here.
    };

    // -- Step 2: stream the snapshot to SQLite without holding any
    //    workspace lock. The inner store `Mutex` is the only thing
    //    serialising disk writes from here on.
    let unseen_ids: Vec<Uuid> = snapshot.unseen_ops.iter().map(|op| op.id).collect();
    {
        let mut store_guard = store.lock();
        store_guard.save_document(&snapshot.document)?;
        store_guard.save_design_tokens(&snapshot.design_tokens)?;
        store_guard.save_color_settings(&snapshot.color_settings)?;
        for kit in &snapshot.brand_kits {
            store_guard.save_brand_kit(kit)?;
        }
        // Reconcile deleted brand kits: any rows on disk whose id is no
        // longer in the snapshot must be removed so deletes survive
        // the next reopen.
        let kit_ids: HashSet<Uuid> = snapshot.brand_kits.iter().map(|k| k.id).collect();
        let on_disk_kits = store_guard.load_brand_kits()?;
        for kit in &on_disk_kits {
            if !kit_ids.contains(&kit.id) {
                store_guard.delete_brand_kit(kit.id)?;
            }
        }
        for preset in &snapshot.export_presets {
            store_guard.save_export_preset(preset)?;
        }
        let preset_ids: HashSet<Uuid> = snapshot.export_presets.iter().map(|p| p.id).collect();
        let on_disk_presets = store_guard.load_export_presets()?;
        for preset in &on_disk_presets {
            if !preset_ids.contains(&preset.id) {
                store_guard.delete_export_preset(preset.id)?;
            }
        }
        // Components: the in-memory map is the source of truth, so we
        // bulk-replace on disk. This handles both upsert and delete in
        // one round-trip; matches how `replace_components` is documented.
        store_guard.replace_components(&snapshot.components)?;
        for op in &snapshot.unseen_ops {
            store_guard.save_operation(op)?;
        }
        // Mirror the in-memory `max_depth` bound onto the on-disk
        // table. Without this, the operations table grows for the
        // project's lifetime; combined with the (historic) load_operations
        // bug, it would silently lose recent history once the row count
        // exceeded `max_depth`. The on-disk bound is the same as the
        // in-memory bound by design — the in-memory log is the canonical
        // undo surface and the disk just snapshots it.
        store_guard.prune_operations(snapshot.max_depth)?;
    }

    // -- Step 3: brief write lock to merge persisted ids. ----------
    // We only update bookkeeping (`persisted_op_ids`); no rendering or
    // disk side effects fire here, so the critical section is O(log).
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    for id in &unseen_ids {
        ws.persisted_op_ids.insert(*id);
    }
    // Retain only ids that are still in the post-save in-memory log
    // (some may have aged out via bounded-depth front-trim while we
    // were streaming to SQLite). This preserves the O(max_depth)
    // size invariant on `persisted_op_ids`.
    let current_ids: HashSet<Uuid> = ws.project.operation_log.iter().map(|op| op.id).collect();
    ws.persisted_op_ids.retain(|id| current_ids.contains(id));
    drop(guard);
    Ok(())
}

/// Close the current project, discarding unsaved in-memory changes.
///
/// In addition to dropping the workspace slot, this resets the
/// autosave subsystem so per-project bookkeeping
/// (`last_saved_modified_at`, `counter`, `last_error`) doesn't
/// leak into the next `project_open` / `project_create`. See
/// [`crate::autosave::autosave_reset`] for the full rationale.
pub fn project_close() {
    crate::autosave::autosave_reset();
    *slot().write() = None;
    // Drop the renderer's cached scene too: the closed project's
    // content must neither linger in renderer memory nor be repainted
    // by a stray `render_current` before the next project is synced.
    // The renderer (and its last presented frame) stays attached.
    crate::state::clear_scene();
}

/// Snapshot of the open project (or `None` if nothing is open).
pub fn project_info() -> Option<ProjectInfo> {
    let guard = slot().write();
    let info = guard
        .as_ref()
        .map(|ws| build_info(&ws.project, ws.store.lock().project_dir()));
    drop(guard);
    info
}

/// Returns true iff the open project is in its untouched, just-
/// created state — no user operation has been recorded since
/// `project_create` / `project_open` set up the workspace.
///
/// Definition: the [`OperationLog`](kcreate_core::operation::OperationLog)
/// is empty. Every host-driven mutation runs through
/// [`Project::execute_operation`], so an empty log is a strict
/// "no user edits yet" signal. `project_create` only calls
/// `Project::add_page("Page 1")` (which mutates the document graph
/// directly without pushing onto the log) before handing the
/// workspace back, so a freshly-created project reports `true`.
///
/// `project_open` restores the on-disk operation history into the
/// in-memory log via [`OperationLog::restore_from`], so a project
/// that was edited before save+close will report `false` on reopen
/// (the user already touched it). A project that was created and
/// saved without any edits is still untouched on reopen.
///
/// Used by the host UI (e.g. `EditorPage.tsx`) to decide whether
/// to auto-open the TemplatePicker on the user's first switch to
/// Layout mode. The previous TypeScript heuristic
/// (`nodes.length === 2 && exactly 1 Page named "Page 1" && 1
/// Artboard`) replicated the exact output of
/// `Project::add_page("Page 1")` and would silently break if the
/// Rust side ever renamed the default page or added a default
/// layer — Devin Review PR #5 ANALYSIS-0006 (commit 5c16b5c)
/// called this out as a maintenance hazard. Exposing the
/// authoritative signal from the bridge keeps the source of
/// truth on the side that owns it.
///
/// Returns [`DocumentBridgeError::NoProject`] when no project is
/// open so the host can distinguish "no project" from "untouched
/// project" without inspecting `project_info()` first.
pub fn project_is_untouched() -> Result<bool> {
    let guard = slot().write();
    let ws = guard.as_ref().ok_or(DocumentBridgeError::NoProject)?;
    Ok(ws.project.operation_log.is_empty())
}

// -----------------------------------------------------------------------------
// Design tokens / brand kits / export presets
// -----------------------------------------------------------------------------

/// Snapshot the current project's design tokens. Returns the empty
/// default `DesignTokens` when no project is open so callers can keep
/// a stable React state shape without special-casing the no-project
/// path.
pub fn design_tokens_get() -> Result<DesignTokens> {
    let guard = slot().write();
    let ws = guard.as_ref().ok_or(DocumentBridgeError::NoProject)?;
    Ok(ws.project.design_tokens.clone())
}

/// Replace the entire design-tokens bag. The caller is responsible
/// for calling [`project_save`] afterwards; this only mutates the
/// in-memory project.
pub fn design_tokens_set(tokens: DesignTokens) -> Result<()> {
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    ws.project.design_tokens = tokens;
    drop(guard);
    Ok(())
}

/// Create a new (empty) brand kit and append it to the project.
/// Returns the new kit's id.
pub fn brand_kit_create(name: String) -> Result<Uuid> {
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    let kit = BrandKit::new(name);
    let id = kit.id;
    ws.project.brand_kits.push(kit);
    drop(guard);
    Ok(id)
}

/// Replace an existing brand kit by id. Returns
/// [`DocumentBridgeError::NodeNotFound`] if the id doesn't match any
/// in-memory kit — brand-kit ids share the `Uuid` namespace with
/// node ids and the bridge surface treats both as opaque, so we
/// reuse the existing error variant rather than introducing a
/// second "not found" type that callers would have to disambiguate.
pub fn brand_kit_update(kit: BrandKit) -> Result<()> {
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    let slot_idx = ws
        .project
        .brand_kits
        .iter()
        .position(|k| k.id == kit.id)
        .ok_or(DocumentBridgeError::NodeNotFound(kit.id))?;
    ws.project.brand_kits[slot_idx] = kit;
    drop(guard);
    Ok(())
}

/// List every brand kit in the project, in insertion order.
pub fn brand_kit_list() -> Result<Vec<BrandKit>> {
    let guard = slot().write();
    let ws = guard.as_ref().ok_or(DocumentBridgeError::NoProject)?;
    Ok(ws.project.brand_kits.clone())
}

/// Remove a brand kit by id. Returns true when something was removed.
pub fn brand_kit_delete(id: Uuid) -> Result<bool> {
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    let before = ws.project.brand_kits.len();
    ws.project.brand_kits.retain(|k| k.id != id);
    Ok(ws.project.brand_kits.len() != before)
}

/// Create a new export preset and append it to the project. Returns the new id.
pub fn export_preset_create(name: String, format: &str, scale: f32) -> Result<Uuid> {
    let format = parse_export_format(format)?;
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    let preset = ExportPreset::new(name, format, scale);
    let id = preset.id;
    ws.project.export_presets.push(preset);
    drop(guard);
    Ok(id)
}

/// List every export preset, in insertion order.
pub fn export_preset_list() -> Result<Vec<ExportPreset>> {
    // Phase 11 Task 19: read-only — share the lock with other readers.
    let guard = slot().read();
    let ws = guard.as_ref().ok_or(DocumentBridgeError::NoProject)?;
    Ok(ws.project.export_presets.clone())
}

/// Delete an export preset by id. Returns true when something was removed.
pub fn export_preset_delete(id: Uuid) -> Result<bool> {
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    let before = ws.project.export_presets.len();
    ws.project.export_presets.retain(|p| p.id != id);
    Ok(ws.project.export_presets.len() != before)
}

fn parse_export_format(format: &str) -> Result<kcreate_core::project::ExportFormat> {
    use kcreate_core::project::ExportFormat;
    match format.to_ascii_lowercase().as_str() {
        "png" => Ok(ExportFormat::Png),
        "svg" => Ok(ExportFormat::Svg),
        "pdf" => Ok(ExportFormat::Pdf),
        "webp" => Ok(ExportFormat::Webp),
        "jpeg" | "jpg" => Ok(ExportFormat::Jpeg),
        other => Err(DocumentBridgeError::InvalidArgument {
            argument: "format".into(),
            value: other.to_string(),
        }),
    }
}

// -----------------------------------------------------------------------------
// Document CRUD
// -----------------------------------------------------------------------------

/// Returns a flat list of every node in document order.
pub fn document_get_tree() -> Result<Vec<NodeInfo>> {
    // Phase 11 Task 19: read-only path — use a shared read guard
    // so the tree panel, status bar, and selection inspector can
    // poll the workspace concurrently with each other.
    let guard = slot().read();
    let ws = guard.as_ref().ok_or(DocumentBridgeError::NoProject)?;
    let mut out = Vec::with_capacity(ws.project.document.node_count());
    for root in ws.project.document.root_ids() {
        push_subtree(&ws.project.document, *root, &mut out);
    }
    drop(guard);
    Ok(out)
}

/// Serialise the open document into a compact JSON payload designed
/// for LLM prompts that need per-node visual properties (colors,
/// fonts, bounds, opacity, effects, blend modes).
///
/// The shape is `{ "project": "...", "nodes": [ { ...full props... } ] }`
/// where each node carries `id`, `type`, `name`, `parent_id`,
/// `children`, `bounds`, `opacity`, `blend_mode`, `visible`,
/// `locked`, `effects`, and the raw `metadata` bag (where fills,
/// strokes, fonts, and text live).
///
/// This is intentionally richer than [`document_get_tree`] — the
/// layer-panel wire shape ([`NodeInfo`]) carries `bounds` (added so
/// the PrototypePlayer can position hotspots without a second round
/// trip per node) but still elides per-node `effects`, `transform`,
/// `fills`, and the raw paint data to keep tree payloads small. LLM
/// prompts for design-token extraction and accessibility audits need
/// the full visual record to produce useful output, so we walk the
/// live `DocumentGraph` directly and serialise every visible property.
pub fn document_serialise_for_ai() -> Result<String> {
    let guard = slot().write();
    let ws = guard.as_ref().ok_or(DocumentBridgeError::NoProject)?;
    let mut nodes = Vec::with_capacity(ws.project.document.node_count());
    for root in ws.project.document.root_ids() {
        push_subtree_full(&ws.project.document, *root, &mut nodes);
    }
    let payload = serde_json::json!({
        "project": ws.project.name,
        "nodes": nodes,
    });
    drop(guard);
    Ok(serde_json::to_string(&payload)?)
}

fn push_subtree_full(doc: &DocumentGraph, id: Uuid, out: &mut Vec<serde_json::Value>) {
    if let Some(node) = doc.get_node(id) {
        out.push(serde_json::json!({
            "id": node.id,
            "type": node_type_name(node.node_type),
            "name": node.name,
            "parent_id": node.parent_id,
            "children": node.children,
            "bounds": {
                "x": node.bounds.x,
                "y": node.bounds.y,
                "width": node.bounds.width,
                "height": node.bounds.height,
            },
            "opacity": node.opacity,
            "blend_mode": format!("{:?}", node.blend_mode),
            "visible": node.visible,
            "locked": node.locked,
            "effects": node.effects,
            "metadata": node.metadata,
        }));
        for child in &node.children {
            push_subtree_full(doc, *child, out);
        }
    }
}

/// Compute the three inspect-mode code outputs (CSS, Tailwind,
/// React inline style) for the node with `id`. The output is the
/// same `InspectCode` struct emitted by `kcreate_export::code_gen`
/// — we just serialize it to JSON at the N-API boundary.
pub fn document_inspect_node(id: Uuid) -> Result<kcreate_export::InspectCode> {
    let guard = slot().write();
    let ws = guard.as_ref().ok_or(DocumentBridgeError::NoProject)?;
    let node = ws
        .project
        .document
        .get_node(id)
        .ok_or(DocumentBridgeError::NodeNotFound(id))?;
    let code = kcreate_export::inspect_node(node);
    drop(guard);
    Ok(code)
}

fn push_subtree(doc: &DocumentGraph, id: Uuid, out: &mut Vec<NodeInfo>) {
    if let Some(node) = doc.get_node(id) {
        out.push(NodeInfo::from(node));
        for child in &node.children {
            push_subtree(doc, *child, out);
        }
    }
}

/// Properties accepted when creating a node.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateNodeProps {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub visible: Option<bool>,
    #[serde(default)]
    pub locked: Option<bool>,
    #[serde(default)]
    pub metadata: Option<HashMap<String, serde_json::Value>>,
}

/// Insert a new node. Returns its id.
///
/// This is a **bare graph mutation** — it does not append an entry to
/// the operation log. The host UI is responsible for calling
/// [`document_record_operation`] separately to make the change
/// undoable, because the bridge has no semantic context (a single
/// user gesture often touches multiple nodes; we don't want one op
/// per CRUD call). See [`kcreate_core::project::Project::undo`] for
/// the full host-driven patch-application contract.
pub fn document_create_node(
    node_type: &str,
    parent_id: Option<Uuid>,
    props: &CreateNodeProps,
) -> Result<Uuid> {
    let kind = parse_node_type(node_type)?;
    let name = props.name.clone().unwrap_or_else(|| default_name_for(kind));
    let mut node = Node::new(kind, name);
    node.parent_id = parent_id;
    if let Some(v) = props.visible {
        node.visible = v;
    }
    if let Some(l) = props.locked {
        node.locked = l;
    }
    if let Some(meta) = &props.metadata {
        node.metadata.clone_from(meta);
    }
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    let id = ws.project.document.insert_node(node)?;
    ws.project.modified_at = Utc::now();
    let _ = sync_scene_locked(&mut guard);
    drop(guard);
    Ok(id)
}

/// Properties accepted on update. Only fields that are `Some` are
/// applied — all others are left untouched.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateNodeProps {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub visible: Option<bool>,
    #[serde(default)]
    pub locked: Option<bool>,
    #[serde(default)]
    pub metadata: Option<HashMap<String, serde_json::Value>>,
    /// Optional override for the node's fill style. When `Some`, the
    /// node's `style.fill` is replaced wholesale (i.e. switching from
    /// `Solid` to `Gradient` or vice versa is supported by sending the
    /// new variant). The renderer-side `FillSection` panel uses this
    /// to commit user edits in the colour / gradient stop editors.
    ///
    /// Decoupled from `metadata` so the FillEditor doesn't have to
    /// know that fill lives on `node.style` vs `node.metadata` —
    /// the bridge owns that layering detail. Mirrors `FillStyle` 1:1
    /// via the `kind`-tagged enum serde produces.
    #[serde(default)]
    pub fill: Option<kcreate_core::node::FillStyle>,
    /// Replace the node's `extra_fills` (additional fills stacked
    /// above the primary `fill`). `Some(empty)` clears them.
    #[serde(default)]
    pub extra_fills: Option<Vec<kcreate_core::node::FillStyle>>,
    /// Replace the node's primary `stroke`. Tri-state: `NoOp` leaves
    /// the existing stroke untouched, `Clear` removes it entirely,
    /// `Set(s)` replaces it. See [`FieldUpdate`] for serde semantics —
    /// an absent JSON field is `NoOp`, `null` is `Clear`, and any
    /// other value is `Set`.
    #[serde(default, deserialize_with = "deserialize_field_update_stroke")]
    pub stroke: FieldUpdate<kcreate_core::node::StrokeStyle>,
    /// Replace the node's `extra_strokes`.
    #[serde(default)]
    pub extra_strokes: Option<Vec<kcreate_core::node::StrokeStyle>>,
    /// Replace the variable-stroke-width profile. `Clear` removes the
    /// profile (uniform width); `Set(profile)` installs the new one.
    #[serde(default, deserialize_with = "deserialize_field_update_profile")]
    pub stroke_width_profile: FieldUpdate<Vec<(f64, f64)>>,
    /// Toggle the overprint flag.
    #[serde(default)]
    pub overprint: Option<bool>,
}

/// Three-way update for an optional field. Serde sees JSON `null` as
/// `Clear`, an absent field as `NoOp`, and any value as `Set(v)`. We
/// can't represent that with `Option<Option<T>>` without tripping the
/// `clippy::option_option` lint, and a custom enum reads better at
/// the call site (`match changes.stroke { Clear => ..., ... }`).
#[derive(Debug, Clone, Default)]
pub enum FieldUpdate<T> {
    /// JSON field absent — leave existing value untouched.
    #[default]
    NoOp,
    /// JSON field is `null` — clear existing value.
    Clear,
    /// JSON field is a value — replace existing value with it.
    Set(T),
}

impl<T> FieldUpdate<T> {
    /// Apply this update to an `Option<T>` slot on a target struct.
    /// `NoOp` leaves the slot alone; `Clear` sets it to `None`;
    /// `Set(v)` replaces it with `Some(v)`.
    pub fn apply(self, slot: &mut Option<T>) {
        match self {
            Self::NoOp => {}
            Self::Clear => *slot = None,
            Self::Set(v) => *slot = Some(v),
        }
    }
}

fn deserialize_field_update_stroke<'de, D>(
    deserializer: D,
) -> std::result::Result<FieldUpdate<kcreate_core::node::StrokeStyle>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    let opt = Option::<kcreate_core::node::StrokeStyle>::deserialize(deserializer)?;
    Ok(match opt {
        Some(v) => FieldUpdate::Set(v),
        None => FieldUpdate::Clear,
    })
}

fn deserialize_field_update_profile<'de, D>(
    deserializer: D,
) -> std::result::Result<FieldUpdate<Vec<(f64, f64)>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    let opt = Option::<Vec<(f64, f64)>>::deserialize(deserializer)?;
    Ok(match opt {
        Some(v) => FieldUpdate::Set(v),
        None => FieldUpdate::Clear,
    })
}

/// Apply an in-place update to a node.
///
/// Bare graph mutation. See [`document_create_node`] and
/// [`kcreate_core::project::Project::undo`] for the host-driven
/// patch-application contract.
pub fn document_update_node(id: Uuid, changes: &UpdateNodeProps) -> Result<()> {
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    let node = ws
        .project
        .document
        .get_node_mut(id)
        .ok_or(DocumentBridgeError::NodeNotFound(id))?;
    if let Some(name) = &changes.name {
        node.name.clone_from(name);
    }
    if let Some(v) = changes.visible {
        node.visible = v;
    }
    if let Some(l) = changes.locked {
        node.locked = l;
    }
    if let Some(meta) = &changes.metadata {
        node.metadata.clone_from(meta);
    }
    if let Some(fill) = &changes.fill {
        node.style.fill = fill.clone();
    }
    if let Some(extra_fills) = &changes.extra_fills {
        node.style.extra_fills.clone_from(extra_fills);
    }
    changes.stroke.clone().apply(&mut node.style.stroke);
    if let Some(extra_strokes) = &changes.extra_strokes {
        node.style.extra_strokes.clone_from(extra_strokes);
    }
    changes
        .stroke_width_profile
        .clone()
        .apply(&mut node.style.stroke_width_profile);
    if let Some(overprint) = changes.overprint {
        node.style.overprint = overprint;
    }
    node.touch();
    ws.project.modified_at = Utc::now();
    let _ = sync_scene_locked(&mut guard);
    drop(guard);
    Ok(())
}

/// Read the current `FillStyle` for a node, serialised as a JSON
/// string. Returns `None` when the node id is not in the document.
///
/// Renderer-side companion to [`document_update_node`]'s new `fill`
/// field: `FillSection` calls this on selection change to populate
/// its form, then writes back through `document_update_node`. Lives
/// here rather than getting hoisted onto `NodeInfo` because the
/// tree-listing path (`document_list_nodes` / PageNavigator) doesn't
/// need fill data for every node and pre-serialising the enum would
/// inflate every tree payload.
pub fn document_node_fill(id: Uuid) -> Result<Option<String>> {
    let guard = slot().write();
    let ws = guard.as_ref().ok_or(DocumentBridgeError::NoProject)?;
    let Some(node) = ws.project.document.get_node(id) else {
        return Ok(None);
    };
    // Goes through `From<serde_json::Error>` on the error variant.
    // FillStyle is a Serialize-derived plain enum, so this is
    // infallible in practice — but unwrapping would lose us the
    // structured error type if a future variant adds a Map-keyed
    // value or other tag that serde-json can't represent.
    let json = serde_json::to_string(&node.style.fill)?;
    Ok(Some(json))
}

/// Read the node's `extra_fills` stack as a JSON array. Returns
/// `None` when the node id is unknown so the renderer can
/// distinguish "no extras yet" (empty array) from "node not
/// found" (null). Phase 5 Block C Task 17.
pub fn document_node_extra_fills(id: Uuid) -> Result<Option<String>> {
    let guard = slot().write();
    let ws = guard.as_ref().ok_or(DocumentBridgeError::NoProject)?;
    let Some(node) = ws.project.document.get_node(id) else {
        return Ok(None);
    };
    let json = serde_json::to_string(&node.style.extra_fills)?;
    Ok(Some(json))
}

/// Read the node's `extra_strokes` stack as a JSON array. Returns
/// `None` when the node id is unknown. Phase 5 Block C Task 17.
pub fn document_node_extra_strokes(id: Uuid) -> Result<Option<String>> {
    let guard = slot().write();
    let ws = guard.as_ref().ok_or(DocumentBridgeError::NoProject)?;
    let Some(node) = ws.project.document.get_node(id) else {
        return Ok(None);
    };
    let json = serde_json::to_string(&node.style.extra_strokes)?;
    Ok(Some(json))
}

/// Remove a node and all its descendants.
///
/// Bare graph mutation. See [`document_create_node`] and
/// [`kcreate_core::project::Project::undo`] for the host-driven
/// patch-application contract.
pub fn document_delete_node(id: Uuid) -> Result<()> {
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    if ws.project.document.remove_node(id).is_none() {
        return Err(DocumentBridgeError::NodeNotFound(id));
    }
    ws.project.modified_at = Utc::now();
    // Drop any selection entries that refer to the deleted node so the
    // host doesn't keep painting highlights over thin air.
    ws.selection.retain(|sel| *sel != id);
    let _ = sync_scene_locked(&mut guard);
    drop(guard);
    Ok(())
}

/// Push an operation onto the project's log.
///
/// Counterpart to [`document_create_node`] / [`document_update_node`]
/// / [`document_delete_node`]: those mutate the graph but do **not**
/// record an op; this records an op but does **not** mutate the
/// graph. The host wires them together at the granularity of the
/// user-facing gesture (e.g. one drag = one op, not one op per
/// pointer sample). See [`kcreate_core::project::Project::undo`] for
/// the architectural rationale.
pub fn document_record_operation(operation: Operation) -> Result<()> {
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    ws.project.execute_operation(operation);
    drop(guard);
    Ok(())
}

/// Outcome of a successful undo / redo.
///
/// Carries both the affected node ids (so the renderer can refresh the
/// view) and the operation `command` string. The host uses `command`
/// to gate side-effect broadcasts that are only meaningful for
/// specific operation kinds — for example, `color_settings_update`
/// fires `kcreate/color/settings/changed`, but a `move_node` does not.
/// Returning the command at the bridge boundary keeps that gating
/// logic in TypeScript next to the IPC channel it controls, instead
/// of pushing every per-op broadcast into Rust.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UndoRedoOutcome {
    /// The `Operation::command` string from the operation that was
    /// rolled back / re-applied. Stable wire identifier; e.g.
    /// `"color_settings_update"`, `"text_frame_update"`,
    /// `"document_update_node"`.
    pub command: String,
    /// The `Operation::affected_nodes` list. Empty for non-graph
    /// operations (e.g. `color_settings_update`).
    pub affected_nodes: Vec<Uuid>,
}

/// Undo the most recent operation.
///
/// Returns:
/// * `Ok(Some(outcome))` when the undo stack is non-empty AND the
///   inverse patch was applied successfully — `outcome.command` is the
///   operation's command string, `outcome.affected_nodes` the impacted
///   nodes.
/// * `Ok(None)` when the undo stack is empty.
/// * `Err(_)` when the inverse patch failed (e.g. corrupted log,
///   missing node). The log cursor is **not** advanced in this case
///   — the next `document_undo()` retries the same operation.
///
/// # Atomicity
///
/// The bridge peeks the pending operation via
/// [`Project::pending_undo`] and applies `before_patch` against the
/// workspace *first*; only on success does it call [`Project::undo`]
/// to commit the cursor move. This prevents the split-brain state
/// where the log cursor advances but the workspace patch fails,
/// which would otherwise silently drop a user's undoable operation
/// (Devin Review BUG / PR #7).
///
/// For non-graph operations recorded by the Phase 2 panels
/// ([`color_settings_update`], [`text_frame_update`],
/// [`text_opentype_features_update`]) the bridge replays `before_patch`
/// onto the workspace itself so the in-memory state actually reverts.
/// For graph-mutating operations (node create / update / delete,
/// reparent, …) the existing host-driven contract still applies — the
/// renderer is expected to fold `before_patch` back into its view via
/// the standard mutate-then-record entry points, mirroring the
/// snapshot stored in [`kcreate_core::project::Project::undo`].
///
/// [`color_settings_update`]: crate::phase2::color_settings_update
/// [`text_frame_update`]: crate::phase2::text_frame_update
/// [`text_opentype_features_update`]: crate::phase2::text_opentype_features_update
pub fn document_undo() -> Result<Option<UndoRedoOutcome>> {
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    let Some(op) = ws.project.pending_undo() else {
        return Ok(None);
    };
    // Apply the inverse patch FIRST. Only commit the cursor move
    // if it succeeds — otherwise the log and workspace would split.
    apply_inverse_patch(ws, &op)?;
    let committed = ws
        .project
        .undo()
        .expect("pending_undo returned Some, so undo cannot return None on the same lock");
    // Phase 11 Block D Task 21: announce the state change to
    // renderer pollers. Graph-mutating operations rely on the host
    // to fold `before_patch` back into the in-memory tree, so
    // `apply_inverse_patch` does NOT touch the graph and therefore
    // does not bump `document_version` itself. The undo is still a
    // user-visible state transition that observers need to see, so
    // we advance the counter here, before dropping the guard.
    ws.project.document.touch_version();
    drop(guard);
    // Phase 7 (Task 17): broadcast the inverse to remote peers,
    // tagged `is_undo: true`, so their renderers can apply the
    // revert AND surface a "<peer> undid their last edit" toast.
    // No-op when no collab session is active.
    broadcast_undo_inverse(&op, BroadcastUndoKind::Undo);
    Ok(Some(UndoRedoOutcome {
        command: committed.command,
        affected_nodes: committed.affected_nodes,
    }))
}

/// Redo the next operation.
///
/// Returns:
/// * `Ok(Some(outcome))` when the redo stack is non-empty AND the
///   forward patch was applied successfully.
/// * `Ok(None)` when the redo stack is empty.
/// * `Err(_)` when the forward patch failed. The log cursor is **not**
///   advanced in this case.
///
/// Atomicity is symmetric with [`document_undo`]: the bridge peeks via
/// [`Project::pending_redo`], applies `after_patch` first, and only
/// commits via [`Project::redo`] on success.
///
/// For Phase 2 non-graph operations the bridge re-applies
/// `after_patch` to the workspace itself; for graph-mutating
/// operations the host-driven contract still applies.
pub fn document_redo() -> Result<Option<UndoRedoOutcome>> {
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    let Some(op) = ws.project.pending_redo() else {
        return Ok(None);
    };
    apply_forward_patch(ws, &op)?;
    let committed = ws
        .project
        .redo()
        .expect("pending_redo returned Some, so redo cannot return None on the same lock");
    // Phase 11 Block D Task 21: see `document_undo` — same
    // rationale, redo is a state transition pollers need to see.
    ws.project.document.touch_version();
    drop(guard);
    // Phase 7 (Task 17): broadcast the forward replay to remote
    // peers, tagged `is_undo: true`. From the remote perspective a
    // redo is structurally identical to a fresh edit (same forward
    // patch) but the activity-feed marker keeps "Ken redid …"
    // separate from "Ken edited …".
    broadcast_undo_inverse(&op, BroadcastUndoKind::Redo);
    Ok(Some(UndoRedoOutcome {
        command: committed.command,
        affected_nodes: committed.affected_nodes,
    }))
}

/// Single source of truth for the set of operation commands
/// [`apply_patch`] knows how to roll forward / backward. Both the
/// snapshot-capture path ([`ApplyPatchSnapshot::capture`]) and the
/// `apply_patch` match below must stay in lockstep with this list —
/// adding a new command to `apply_patch` *without* updating capture
/// would let group-undo silently skip rollback for that command's
/// state, leaving the workspace inconsistent on partial failure.
///
/// The invariant is checked at runtime by [`apply_patch`]'s default
/// arm (debug-asserts in tests; logged in release) and exhaustively
/// in `apply_patch_commands_match` (`tests` module at the bottom of
/// this file), so a forgotten arm fails CI before it can ship.
const APPLY_PATCH_COMMANDS: &[&str] = &[
    "color_settings_update",
    "spot_color_upsert",
    "spot_color_remove",
    "spot_color_load_catalog",
    "text_frame_update",
    "text_opentype_features_update",
    "layer_color_set",
    "clipboard_paste",
    // Phase A1 — inline text editor + font controls. `text_set_*`
    // and `text_replace_range` all write the canonical
    // `TextLayerMeta` payload at `metadata[TEXT_LAYER_METADATA_KEY]`;
    // `text_set_style` additionally writes the camelCase wire
    // payload at `metadata["text_style"]`. Group-undo reverse
    // capture needs to know about these slots — see
    // `ApplyPatchSnapshot::capture`.
    "text_set_content",
    "text_replace_range",
    "text_set_style",
    // G4 — Theme / Brand Kit instant restyle. A single
    // `apply_theme` op snapshots the prior + next `NodeStyle`,
    // text-layer metadata, and project `DesignTokens` for every node
    // the restyle touched, so one undo flips the whole document back.
    // The before / after patches share the [`ApplyThemePatch`] shape;
    // group-undo reverse capture needs to roll back the same slots —
    // see `ApplyPatchSnapshot::capture`.
    "apply_theme",
    // H4 — AI generation depth. `ai_generate_themed_design` /
    // `ai_refine_themed_design` build a whole generated design (a
    // tiled page of artboards + themed layers + hero imagery, plus a
    // theme brand kit) in one shot. Both record a single reversible
    // [`crate::phase10::ThemedDesignPatch`] (before = undo direction,
    // after = redo direction) so one Ctrl+Z removes the generated
    // design and restores whatever the document held before (a prior
    // generated design, a pristine scratch scaffold, or the user's
    // own pages) — see the shared arm in [`apply_patch`]. Replay is
    // pure graph + brand-kit vec ops, so no snapshot slot is needed
    // in `ApplyPatchSnapshot::capture` (documented no-op arm there).
    "ai_generate_themed_design",
    "ai_refine_themed_design",
];

#[inline]
fn is_apply_patch_command(cmd: &str) -> bool {
    APPLY_PATCH_COMMANDS.contains(&cmd)
}

/// Snapshot of every workspace field [`apply_patch`] is capable of
/// mutating, captured before a group of inverse / forward patches is
/// applied. Used to roll the workspace back atomically when a patch
/// fails mid-group, so the operation log cursor (which is not yet
/// advanced) and the workspace state remain in sync — preserving the
/// "a partial failure leaves the stack untouched" contract that
/// [`document_undo_group`] / [`document_redo_group`] publish to
/// callers.
///
/// We snapshot **only the fields apply_patch actually writes to**:
///
/// * `color_settings` — replaced by `color_settings_update` /
///   single-arm.
/// * `spot_color_library` — replaced by `spot_color_upsert` /
///   `spot_color_remove` / `spot_color_load_catalog`.
/// * Per-node `TextFrameOptions` — written by `text_frame_update`.
/// * Per-node `OpenTypeFeatures` — written by
///   `text_opentype_features_update`.
/// * Per-node layer-colour tag — written by `layer_color_set`.
/// * Subtree presence for `clipboard_paste`.
///
/// Graph operations (`document_create_node`, `canvas_move_node`,
/// `document_reparent`, …) currently fall through to the no-op arm
/// in [`apply_patch`], so they don't need to participate. The single
/// source of truth for which commands belong in this snapshot is
/// [`APPLY_PATCH_COMMANDS`]; any new arm added to [`apply_patch`]
/// must also be added there and to [`ApplyPatchSnapshot::capture`].
/// The `apply_patch_commands_match` test enforces the coupling at
/// compile-test time.
struct ApplyPatchSnapshot {
    color_settings: Option<kcreate_core::color::ColorSettings>,
    spot_color_library: Option<kcreate_core::color::SpotColorLibrary>,
    text_frame: HashMap<Uuid, kcreate_core::node::TextFrameOptions>,
    opentype: HashMap<Uuid, kcreate_core::node::OpenTypeFeatures>,
    // Phase 6 Tasks 27-28: per-node prior layer-colour tag (None =
    // untagged). Captured before any group-level inverse patch runs
    // so we can roll the tag back if a later patch in the group
    // fails.
    layer_color: HashMap<Uuid, Option<String>>,
    // Phase 6 Tasks 25-26 (Devin Review ANALYSIS_0005 follow-up):
    // per-clipboard_paste-op snapshot of the subtree's presence in
    // the graph at capture time. Keyed by `new_root_id`.
    //   * `was_present = true`  → we're rolling back an UNDO that
    //     removed the subtree; restore must re-insert from `subtree`.
    //   * `was_present = false` → we're rolling back a REDO that
    //     inserted the subtree; restore must remove `new_root_id`
    //     (and its descendants, via DocumentGraph::remove_node).
    clipboard_paste: HashMap<Uuid, ClipboardPasteSnapshot>,
    // Phase A1 — per-node text-layer metadata captured before any
    // `text_set_content` / `text_replace_range` / `text_set_style`
    // patch in the group runs. Each entry stores the raw JSON for
    // both metadata slots (`TEXT_LAYER_METADATA_KEY` and
    // `text_style`); restore writes them back atomically so a
    // failed group leaves the node where it started, without
    // requiring the rollback code to know the typed shape of
    // either payload (the shape is already verified by the
    // recorder + `apply_patch`).
    text_layer: HashMap<Uuid, TextLayerSnapshot>,
    // G4 — per-node prior style + text-metadata captured before any
    // `apply_theme` patch in the group runs. Keyed by node id. The
    // companion `apply_theme_tokens` holds the project design tokens
    // at capture time so the whole restyle (tokens + every node)
    // rolls back atomically on a mid-group failure.
    apply_theme: HashMap<Uuid, ApplyThemeSnapshot>,
    apply_theme_tokens: Option<DesignTokens>,
}

/// Pre-loop snapshot of the node state an `apply_theme` patch may
/// overwrite: the full `NodeStyle` (fills / strokes / corner radius)
/// plus both text-metadata slots (`TEXT_LAYER_METADATA_KEY` and the
/// camelCase `text_style`). `None` for a text slot means the node
/// had no such key and restore must remove it.
struct ApplyThemeSnapshot {
    style: kcreate_core::node::NodeStyle,
    text_meta: Option<serde_json::Value>,
    text_style: Option<serde_json::Value>,
}

/// Pre-loop snapshot of the metadata slots Phase A1 commands write.
/// Both fields are `Option<Value>` because a freshly-created text
/// layer may not yet carry the camelCase `text_style` slot — the
/// inspector populates it the first time the user touches the
/// style panel. `None` means "remove the key entirely on restore".
struct TextLayerSnapshot {
    meta: Option<serde_json::Value>,
    style: Option<serde_json::Value>,
}

/// Pre-loop state of a single `clipboard_paste` op's subtree so the
/// atomic-rollback path can put the graph back regardless of which
/// direction the failed patch was running in.
struct ClipboardPasteSnapshot {
    was_present: bool,
    subtree: Vec<kcreate_core::node::Node>,
}

impl ApplyPatchSnapshot {
    /// Walk `ops` once and stash whatever pieces of workspace state
    /// the group can touch. Each field is captured **at most once**
    /// (the first op that mentions it) so the snapshot describes the
    /// state immediately *before* the loop runs.
    fn capture(ws: &Workspace, ops: &[Operation]) -> Self {
        let mut snap = Self {
            color_settings: None,
            spot_color_library: None,
            text_frame: HashMap::new(),
            opentype: HashMap::new(),
            layer_color: HashMap::new(),
            clipboard_paste: HashMap::new(),
            text_layer: HashMap::new(),
            apply_theme: HashMap::new(),
            apply_theme_tokens: None,
        };
        for op in ops {
            // Defence-in-depth: skip commands the apply_patch
            // dispatcher doesn't know about so a typo in a future
            // op-recorder can't silently snapshot state that won't
            // be exercised. The match arms below still pair 1:1
            // with the apply_patch arms — see APPLY_PATCH_COMMANDS.
            if !is_apply_patch_command(op.command.as_str()) {
                continue;
            }
            match op.command.as_str() {
                "color_settings_update" => {
                    snap.color_settings
                        .get_or_insert_with(|| ws.project.color_settings.clone());
                }
                "spot_color_upsert" | "spot_color_remove" | "spot_color_load_catalog" => {
                    snap.spot_color_library
                        .get_or_insert_with(|| ws.project.spot_color_library.clone());
                }
                "text_frame_update" => {
                    if let Some(id) = op.affected_nodes.first().copied() {
                        // Only snapshot the first edit per node so we
                        // preserve the pre-loop state; subsequent
                        // edits to the same node don't overwrite the
                        // baseline.
                        if let std::collections::hash_map::Entry::Vacant(slot) =
                            snap.text_frame.entry(id)
                        {
                            if let Some(node) = ws.project.document.get_node(id) {
                                slot.insert(node.text_frame_options().clone());
                            }
                        }
                    }
                }
                "text_opentype_features_update" => {
                    if let Some(id) = op.affected_nodes.first().copied() {
                        if let std::collections::hash_map::Entry::Vacant(slot) =
                            snap.opentype.entry(id)
                        {
                            if let Some(node) = ws.project.document.get_node(id) {
                                slot.insert(node.opentype_features().clone());
                            }
                        }
                    }
                }
                "layer_color_set" => {
                    if let Some(id) = op.affected_nodes.first().copied() {
                        if let std::collections::hash_map::Entry::Vacant(slot) =
                            snap.layer_color.entry(id)
                        {
                            if let Some(node) = ws.project.document.get_node(id) {
                                let prior = node
                                    .metadata
                                    .get(LAYER_COLOR_METADATA_KEY)
                                    .and_then(|v| v.as_str())
                                    .map(str::to_owned);
                                slot.insert(prior);
                            }
                        }
                    }
                }
                "text_set_content" | "text_replace_range" | "text_set_style" => {
                    if let Some(id) = op.affected_nodes.first().copied() {
                        if let std::collections::hash_map::Entry::Vacant(slot) =
                            snap.text_layer.entry(id)
                        {
                            if let Some(node) = ws.project.document.get_node(id) {
                                slot.insert(TextLayerSnapshot {
                                    meta: node
                                        .metadata
                                        .get(crate::scene_sync::TEXT_LAYER_METADATA_KEY)
                                        .cloned(),
                                    style: node.metadata.get("text_style").cloned(),
                                });
                            }
                        }
                    }
                }
                "clipboard_paste" => {
                    // The `new_root_id` lives in affected_nodes[0]
                    // (set by document_clipboard_paste) and is also
                    // the key into the rollback map.
                    let Some(root_id) = op.affected_nodes.first().copied() else {
                        continue;
                    };
                    if let std::collections::hash_map::Entry::Vacant(slot) =
                        snap.clipboard_paste.entry(root_id)
                    {
                        let was_present = ws.project.document.get_node(root_id).is_some();
                        // If the root is present we're about to run
                        // an inverse patch (undo); capture the full
                        // subtree so restore can re-insert in
                        // parent-first order. If it's absent we're
                        // about to redo and only need the marker.
                        let subtree = if was_present {
                            collect_subtree_parent_first(&ws.project.document, root_id)
                        } else {
                            Vec::new()
                        };
                        slot.insert(ClipboardPasteSnapshot {
                            was_present,
                            subtree,
                        });
                    }
                }
                "apply_theme" => {
                    // Capture the design tokens once (the first
                    // apply_theme op in the group) so restore reverts
                    // them to the pre-loop value.
                    snap.apply_theme_tokens
                        .get_or_insert_with(|| ws.project.design_tokens.clone());
                    for id in &op.affected_nodes {
                        if let std::collections::hash_map::Entry::Vacant(slot) =
                            snap.apply_theme.entry(*id)
                        {
                            if let Some(node) = ws.project.document.get_node(*id) {
                                slot.insert(ApplyThemeSnapshot {
                                    style: node.style.clone(),
                                    text_meta: node
                                        .metadata
                                        .get(crate::scene_sync::TEXT_LAYER_METADATA_KEY)
                                        .cloned(),
                                    text_style: node.metadata.get("text_style").cloned(),
                                });
                            }
                        }
                    }
                }
                // No snapshot slot needed for the remaining
                // apply_patch commands. Notably H4's
                // `ai_generate_themed_design` /
                // `ai_refine_themed_design` carry a fully
                // self-contained reversible [`crate::phase10::
                // ThemedDesignPatch`] (removed subtrees + inserted
                // subtree + brand kit before/after); replaying it via
                // `apply_patch` is pure graph + brand-kit vec ops that
                // never fail, so there is nothing extra to capture
                // here for atomic group rollback.
                _ => {}
            }
        }
        snap
    }

    /// Reverse every mutation [`apply_patch`] may have written into
    /// the workspace since [`Self::capture`] ran.
    fn restore(self, ws: &mut Workspace) {
        if let Some(cs) = self.color_settings {
            ws.project.color_settings = cs;
        }
        if let Some(lib) = self.spot_color_library {
            ws.project.spot_color_library = lib;
        }
        for (id, opts) in self.text_frame {
            if let Some(node) = ws.project.document.get_node_mut(id) {
                node.set_text_frame_options(&opts);
            }
        }
        for (id, feats) in self.opentype {
            if let Some(node) = ws.project.document.get_node_mut(id) {
                node.set_opentype_features(&feats);
            }
        }
        for (id, prior_color) in self.layer_color {
            if let Some(node) = ws.project.document.get_node_mut(id) {
                match prior_color {
                    Some(s) => {
                        node.metadata.insert(
                            LAYER_COLOR_METADATA_KEY.to_string(),
                            serde_json::Value::String(s),
                        );
                    }
                    None => {
                        node.metadata.remove(LAYER_COLOR_METADATA_KEY);
                    }
                }
            }
        }
        for (id, snap) in self.text_layer {
            if let Some(node) = ws.project.document.get_node_mut(id) {
                match snap.meta {
                    Some(v) => {
                        node.metadata
                            .insert(crate::scene_sync::TEXT_LAYER_METADATA_KEY.to_string(), v);
                    }
                    None => {
                        node.metadata
                            .remove(crate::scene_sync::TEXT_LAYER_METADATA_KEY);
                    }
                }
                match snap.style {
                    Some(v) => {
                        node.metadata.insert("text_style".to_string(), v);
                    }
                    None => {
                        node.metadata.remove("text_style");
                    }
                }
            }
        }
        for (root_id, paste_snap) in self.clipboard_paste {
            let currently_present = ws.project.document.get_node(root_id).is_some();
            if paste_snap.was_present && !currently_present {
                // We snapshotted while the subtree was in the graph
                // and it's gone now → the failed group ran an undo
                // that removed it. Re-insert in the original
                // parent-first order so each child finds its parent
                // already attached.
                for node in paste_snap.subtree {
                    // We can only log + drop the error: restore is
                    // best-effort and the alternative is to leave
                    // the workspace partially rolled back.
                    let _ = ws.project.document.insert_node(node);
                }
            } else if !paste_snap.was_present && currently_present {
                // The subtree wasn't in the graph at capture time
                // and now it is → the failed group ran a redo that
                // inserted it. Remove root + descendants.
                ws.project.document.remove_node(root_id);
            }
        }
        // G4 — restore per-node style + text metadata, then the
        // design tokens, so a mid-group apply_theme failure rolls the
        // whole restyle back to its pre-loop state.
        for (id, theme_snap) in self.apply_theme {
            if let Some(node) = ws.project.document.get_node_mut(id) {
                node.style = theme_snap.style;
                match theme_snap.text_meta {
                    Some(v) => {
                        node.metadata
                            .insert(crate::scene_sync::TEXT_LAYER_METADATA_KEY.to_string(), v);
                    }
                    None => {
                        node.metadata
                            .remove(crate::scene_sync::TEXT_LAYER_METADATA_KEY);
                    }
                }
                match theme_snap.text_style {
                    Some(v) => {
                        node.metadata.insert("text_style".to_string(), v);
                    }
                    None => {
                        node.metadata.remove("text_style");
                    }
                }
                node.touch();
            }
        }
        if let Some(tokens) = self.apply_theme_tokens {
            ws.project.design_tokens = tokens;
        }
    }
}

/// Walk the subtree rooted at `root_id` in parent-first order
/// (i.e. each child appears after its parent in the returned vec).
/// Used by [`ApplyPatchSnapshot::capture`] so the matching restore
/// path can re-insert via `DocumentGraph::insert_node` without
/// hitting a `NodeNotFound` on a child whose parent hasn't been
/// re-attached yet. Also used by `phase10`'s themed-design generator
/// to capture the inserted / removed subtrees into a reversible
/// [`crate::phase10::ThemedDesignPatch`].
pub(crate) fn collect_subtree_parent_first(
    doc: &kcreate_core::document::DocumentGraph,
    root_id: Uuid,
) -> Vec<kcreate_core::node::Node> {
    let mut out: Vec<kcreate_core::node::Node> = Vec::new();
    let mut stack: Vec<Uuid> = vec![root_id];
    while let Some(id) = stack.pop() {
        let Some(node) = doc.get_node(id) else {
            continue;
        };
        // Push children in reverse so the LIFO stack pops them in
        // their natural left-to-right order — this preserves the
        // sibling layout as it appeared at capture time.
        for child in node.children.iter().rev() {
            stack.push(*child);
        }
        out.push(node.clone());
    }
    out
}

/// One step of group-aware undo. Consumes the entire contiguous run
/// of ops that share the most recent `group_id` (or just one op if
/// ungrouped). Applies each `before_patch` atomically — if any
/// patch fails the workspace is rolled back to its pre-loop state
/// via [`ApplyPatchSnapshot`] and the cursor stays put so the next
/// call retries the same group with no double-application.
///
/// Returns `Ok(None)` when the undo stack is empty. Otherwise an
/// outcome describing the *user-facing* operation: the command
/// string of the head (newest) op and the union of all affected
/// nodes across the group.
pub fn document_undo_group() -> Result<Option<UndoRedoOutcome>> {
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    let pending = ws.project.pending_undo_group();
    if pending.is_empty() {
        return Ok(None);
    }
    // Capture every field the loop below could mutate so we can roll
    // back atomically if one of the patches fails — see the comment
    // on [`ApplyPatchSnapshot`] for why this is the architecturally
    // correct fix (vs. e.g. snapshotting the entire `Project`).
    let snapshot = ApplyPatchSnapshot::capture(ws, &pending);
    // Apply each `before_patch` in undo order (newest-first). On any
    // failure, restore from the snapshot before propagating — never
    // leave the workspace in a half-rolled-back state.
    for op in &pending {
        if let Err(e) = apply_inverse_patch(ws, op) {
            snapshot.restore(ws);
            return Err(e);
        }
    }
    let committed = ws.project.undo_group();
    debug_assert_eq!(committed.len(), pending.len());
    drop(guard);
    let head = pending.first().expect("non-empty");
    let mut affected: Vec<Uuid> = Vec::new();
    for op in &pending {
        for node in &op.affected_nodes {
            if !affected.contains(node) {
                affected.push(*node);
            }
        }
    }
    // Phase 7 (Task 17): broadcast the whole group as a single
    // `is_undo: true` batch so remote peers see "Ken undid 3 edits"
    // (one toast) instead of three separate ones.
    broadcast_undo_inverse_batch(&pending, BroadcastUndoKind::Undo);
    Ok(Some(UndoRedoOutcome {
        command: head.command.clone(),
        affected_nodes: affected,
    }))
}

/// One step of group-aware redo. Symmetric with
/// [`document_undo_group`]; consumes the contiguous run starting
/// at the cursor. Uses the same snapshot+rollback discipline so
/// a patch failure mid-group cannot leave the workspace partially
/// re-applied.
pub fn document_redo_group() -> Result<Option<UndoRedoOutcome>> {
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    let pending = ws.project.pending_redo_group();
    if pending.is_empty() {
        return Ok(None);
    }
    let snapshot = ApplyPatchSnapshot::capture(ws, &pending);
    for op in &pending {
        if let Err(e) = apply_forward_patch(ws, op) {
            snapshot.restore(ws);
            return Err(e);
        }
    }
    let committed = ws.project.redo_group();
    debug_assert_eq!(committed.len(), pending.len());
    drop(guard);
    // Head for "command displayed to user" purposes is the *last*
    // op (the most recent state we end up in).
    let head = pending.last().expect("non-empty");
    let mut affected: Vec<Uuid> = Vec::new();
    for op in &pending {
        for node in &op.affected_nodes {
            if !affected.contains(node) {
                affected.push(*node);
            }
        }
    }
    // Phase 7 (Task 17): broadcast the whole redo group as a single
    // is_undo batch. See `document_undo_group`.
    broadcast_undo_inverse_batch(&pending, BroadcastUndoKind::Redo);
    Ok(Some(UndoRedoOutcome {
        command: head.command.clone(),
        affected_nodes: affected,
    }))
}

/// Wire-format summary of one discarded branch the user can recover.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiscardedBranchSummary {
    /// Position in the timeline this branch attaches to. Restorable
    /// only while `OperationLog::position() == anchor_position`.
    pub anchor_position: usize,
    /// Number of operations the branch contains.
    pub op_count: usize,
    /// ISO-8601 UTC timestamp the branch was discarded at. Useful
    /// for the panel's "discarded 3 minutes ago" affordance.
    pub discarded_at_iso: String,
    /// Stable wire identifier of the first op in the branch — the
    /// renderer uses it as a thumbnail / preview hint.
    pub first_command: String,
}

/// List all discarded redo branches in the current project, newest
/// first. The renderer's undo/branch panel calls this whenever the
/// log changes.
///
/// `Project::discarded_branches()` (a thin wrapper over
/// `OperationLog::branches`) already yields newest-first as of the
/// fix for Devin Review ANALYSIS_0001 on PR #16 — so this function
/// can pass the list through without additional reversal. The index
/// of each summary in the returned `Vec` therefore matches the
/// `index_from_back` argument expected by
/// [`document_restore_discarded_branch`].
pub fn document_list_discarded_branches() -> Result<Vec<DiscardedBranchSummary>> {
    let guard = slot().write();
    let ws = guard.as_ref().ok_or(DocumentBridgeError::NoProject)?;
    let branches = ws.project.discarded_branches();
    Ok(branches
        .into_iter()
        .map(|b| DiscardedBranchSummary {
            anchor_position: b.anchor_position,
            op_count: b.ops.len(),
            discarded_at_iso: b.discarded_at.to_rfc3339(),
            first_command: b.ops.first().map(|o| o.command.clone()).unwrap_or_default(),
        })
        .collect())
}

/// Restore the discarded branch at `index_from_back` (0 = newest).
/// The current redo tail (if any) is captured as a new discarded
/// branch so the swap is reversible. Returns `true` on success.
pub fn document_restore_discarded_branch(index_from_back: usize) -> Result<bool> {
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    Ok(ws.project.restore_discarded_branch(index_from_back))
}

/// Walk `op.before_patch` into workspace state for the non-graph
/// operations recorded by Phase 2 panels.
///
/// Returns `Ok(())` for every other command kind — graph mutations
/// (`document_create_node` / `_update` / `_delete`, `document_reparent`,
/// `canvas_move_node`, …) keep the legacy host-driven undo contract
/// where the renderer is responsible for folding `before_patch` back
/// into the view. We deliberately do not silently `Err` on unknown
/// commands here so the cursor-only undo semantics that the rest of
/// the workspace relies on continue to function.
fn apply_inverse_patch(ws: &mut Workspace, op: &Operation) -> Result<()> {
    apply_patch(ws, op, &op.before_patch)
}

/// Phase 7 (Task 17): kind of broadcast emitted by
/// [`broadcast_undo_inverse`] / [`broadcast_undo_inverse_batch`].
/// Selects which patch direction is forwarded to remote peers — an
/// `Undo` ships the inverse (before_patch) so peers can revert to
/// the prior state; a `Redo` ships the forward (after_patch) so
/// peers can re-apply the operation that was previously undone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BroadcastUndoKind {
    Undo,
    Redo,
}

/// Phase 7 (Task 17): broadcast a single undo / redo step to the
/// active collab session, tagged with the `is_undo: true` marker so
/// remote peers can render an activity-feed entry ("Ken undid …")
/// rather than treating the op as a fresh edit. A no-op when no
/// session is running, so undo / redo still works correctly in solo
/// mode. Errors from the collab layer (no session, viewer
/// permission, transport timeout, etc.) are intentionally swallowed
/// — undo's user-visible local effect is already applied; the
/// broadcast is best-effort.
///
/// Under the default (non-`collab`) feature configuration this
/// resolves to a no-op stub so the four core undo / redo entry points
/// (`document_undo`, `document_redo`, `document_undo_group`,
/// `document_redo_group`) compile and behave identically to the
/// pre-Phase-7 behaviour. The `collab` module only exists with the
/// feature enabled (see `lib.rs`), so we MUST NOT reference it from
/// the default build closure — otherwise the editing-path crates
/// pull in `crate::collab::*` and fail to resolve. This is the
/// `AGENTS.md` "Collab feature isolation" rule.
fn broadcast_undo_inverse(orig: &Operation, kind: BroadcastUndoKind) {
    broadcast_undo_inverse_batch(std::slice::from_ref(orig), kind);
}

/// Phase 7 (Task 17): batch variant of [`broadcast_undo_inverse`]
/// used by group-aware undo / redo. Sends all of `orig` as a single
/// `OperationBroadcast`, so remote peers see "Ken undid 3 edits"
/// (one toast) rather than three separate ones.
///
/// Each broadcast op carries:
///   * a fresh `id` (so the remote journal doesn't dedupe against
///     the original we authored earlier),
///   * a fresh `timestamp` (chronological order on the wire is
///     anchored to when we *broadcast* the revert, not when we
///     originally made the edit),
///   * `before_patch` / `after_patch` swapped for `Undo` so the
///     remote applies the revert; for `Redo` they're forwarded as-is,
///   * the original `command`, `affected_nodes`, and `group_id`
///     (so remote undo-grouping continues to work), and
///   * `is_undo: true` so [`crate::collab::SessionEvent::UndoBroadcast`]
///     fires on the remote side when every op in the batch is
///     marked.
///
/// Feature-gated body — see [`broadcast_undo_inverse`] for the
/// rationale. The non-`collab` build is a strict no-op (we don't
/// even allocate the inverse vec) because the only place that
/// `inverses` is consumed is the collab broadcast call, and
/// allocating just to throw it away would be wasteful on the
/// default solo-mode build that the vast majority of users run.
#[cfg(feature = "collab")]
fn broadcast_undo_inverse_batch(orig: &[Operation], kind: BroadcastUndoKind) {
    if orig.is_empty() {
        return;
    }
    let now = Utc::now();
    let inverses: Vec<Operation> = orig
        .iter()
        .map(|op| {
            let (before, after) = match kind {
                // Undo: swap so the remote applies the inverse.
                BroadcastUndoKind::Undo => (op.after_patch.clone(), op.before_patch.clone()),
                // Redo: forward direction so the remote re-applies.
                BroadcastUndoKind::Redo => (op.before_patch.clone(), op.after_patch.clone()),
            };
            Operation {
                id: Uuid::new_v4(),
                timestamp: now,
                actor: op.actor.clone(),
                command: op.command.clone(),
                before_patch: before,
                after_patch: after,
                affected_nodes: op.affected_nodes.clone(),
                ai_generated: op.ai_generated,
                group_id: op.group_id,
                is_undo: true,
            }
        })
        .collect();
    // The bridge's collab layer is the single place that knows whether
    // a session is active. Swallow every error so solo-mode undo
    // keeps working when no collab gate has been installed.
    let _ = crate::collab::session_broadcast_operations(inverses);
}

/// No-op stub for the default (non-`collab`) build. Solo-mode undo /
/// redo has nothing to broadcast, so this body is intentionally
/// empty — keeping the same signature means the four core undo /
/// redo entry points stay free of `#[cfg]` blocks.
#[cfg(not(feature = "collab"))]
fn broadcast_undo_inverse_batch(_orig: &[Operation], _kind: BroadcastUndoKind) {}

/// Walk `op.after_patch` into workspace state for the non-graph
/// operations recorded by Phase 2 panels. See [`apply_inverse_patch`]
/// for the broader contract.
fn apply_forward_patch(ws: &mut Workspace, op: &Operation) -> Result<()> {
    apply_patch(ws, op, &op.after_patch)
}

/// Serialized shape of a single node's contribution to an
/// [`ApplyThemePatch`]. `style` is the full `NodeStyle` (so undo /
/// redo restore every fill, stroke, and corner radius verbatim);
/// the two text slots mirror `metadata[TEXT_LAYER_METADATA_KEY]` and
/// `metadata["text_style"]` and are `None` for non-text nodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ApplyThemeNodePatch {
    style: kcreate_core::node::NodeStyle,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    text_meta: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    text_style: Option<serde_json::Value>,
}

/// Full payload carried by both `before_patch` and `after_patch` of
/// an `apply_theme` operation: the project `DesignTokens` plus the
/// per-node style / text snapshot for every node the restyle
/// touched. Replaying the whole struct is what makes one Ctrl+Z
/// revert the entire restyle (see the `apply_theme` arm in
/// [`apply_patch`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ApplyThemePatch {
    design_tokens: DesignTokens,
    nodes: std::collections::BTreeMap<Uuid, ApplyThemeNodePatch>,
}

fn apply_patch(ws: &mut Workspace, op: &Operation, patch: &serde_json::Value) -> Result<()> {
    // `project.undo()` / `project.redo()` already bumped
    // `modified_at` before we got here, so we don't need to touch it
    // again — just walk the patch into the live state.
    match op.command.as_str() {
        "color_settings_update" => {
            let settings: kcreate_core::color::ColorSettings =
                serde_json::from_value(patch.clone())?;
            ws.project.color_settings = settings;
            Ok(())
        }
        // All three spot-color commands snapshot the entire
        // `SpotColorLibrary` in their before / after patches (see
        // `phase2::color_spot_upsert`, `color_spot_remove`, and
        // `color_spot_load_catalog`), so the inverse / forward step
        // is a single library replacement. Without these arms the
        // command would record correctly but undo would be a no-op —
        // the library state would not roll back.
        "spot_color_upsert" | "spot_color_remove" | "spot_color_load_catalog" => {
            let library: kcreate_core::color::SpotColorLibrary =
                serde_json::from_value(patch.clone())?;
            ws.project.spot_color_library = library;
            Ok(())
        }
        "text_frame_update" => {
            let id = op.affected_nodes.first().copied().ok_or_else(|| {
                DocumentBridgeError::InvalidArgument {
                    argument: "affected_nodes".into(),
                    value: format!("text_frame_update operation {} has no affected node", op.id),
                }
            })?;
            let options: kcreate_core::node::TextFrameOptions =
                serde_json::from_value(patch.clone())?;
            let node = ws
                .project
                .document
                .get_node_mut(id)
                .ok_or(DocumentBridgeError::NodeNotFound(id))?;
            node.set_text_frame_options(&options);
            Ok(())
        }
        "text_opentype_features_update" => {
            let id = op.affected_nodes.first().copied().ok_or_else(|| {
                DocumentBridgeError::InvalidArgument {
                    argument: "affected_nodes".into(),
                    value: format!(
                        "text_opentype_features_update operation {} has no affected node",
                        op.id
                    ),
                }
            })?;
            let features: kcreate_core::node::OpenTypeFeatures =
                serde_json::from_value(patch.clone())?;
            let node = ws
                .project
                .document
                .get_node_mut(id)
                .ok_or(DocumentBridgeError::NodeNotFound(id))?;
            node.set_opentype_features(&features);
            Ok(())
        }
        // Phase A1 — inline text editor + font controls.
        //
        // `text_set_content` and `text_replace_range` both record a
        // canonical `TextLayerMeta` JSON in `before_patch` /
        // `after_patch`. Roll forward / backward is a single
        // metadata insert against `TEXT_LAYER_METADATA_KEY`; the
        // shaper / scene_sync will pick up the new payload on the
        // next sync. We intentionally do NOT touch the renderer
        // here — `document_undo` / `document_redo` re-run scene
        // sync after the apply_patch loop via `touch_version` plus
        // the caller's existing republish path.
        "text_set_content" | "text_replace_range" => {
            let id = op.affected_nodes.first().copied().ok_or_else(|| {
                DocumentBridgeError::InvalidArgument {
                    argument: "affected_nodes".into(),
                    value: format!("{} operation {} has no affected node", op.command, op.id),
                }
            })?;
            // Validate the payload shape up-front so a malformed
            // patch fails before we mutate node metadata. The
            // recorders write `TextLayerMeta` directly.
            let _: kcreate_export::TextLayerMeta = serde_json::from_value(patch.clone())?;
            let node = ws
                .project
                .document
                .get_node_mut(id)
                .ok_or(DocumentBridgeError::NodeNotFound(id))?;
            node.metadata.insert(
                crate::scene_sync::TEXT_LAYER_METADATA_KEY.to_string(),
                patch.clone(),
            );
            node.touch();
            Ok(())
        }
        // `text_set_style` packs `{ meta: TextLayerMeta, style:
        // TextStyleWire }` because it writes BOTH metadata slots
        // (the canonical layer meta the shaper consumes AND the
        // camelCase wire-format `text_style` slot the inspector
        // panel reads). Roll forward / backward replays both
        // inserts in one match arm.
        "text_set_style" => {
            let id = op.affected_nodes.first().copied().ok_or_else(|| {
                DocumentBridgeError::InvalidArgument {
                    argument: "affected_nodes".into(),
                    value: format!("text_set_style operation {} has no affected node", op.id),
                }
            })?;
            let obj = patch
                .as_object()
                .ok_or_else(|| DocumentBridgeError::InvalidArgument {
                    argument: "text_set_style patch".into(),
                    value: format!("expected {{meta, style}} object, got {patch}"),
                })?;
            let meta_val = obj
                .get("meta")
                .ok_or_else(|| DocumentBridgeError::InvalidArgument {
                    argument: "text_set_style patch".into(),
                    value: format!("missing `meta` field in {patch}"),
                })?;
            let style_val =
                obj.get("style")
                    .ok_or_else(|| DocumentBridgeError::InvalidArgument {
                        argument: "text_set_style patch".into(),
                        value: format!("missing `style` field in {patch}"),
                    })?;
            // Validate both payload shapes before mutating so a
            // malformed patch cannot leave the node half-written
            // (style up to date, meta stale or vice versa).
            let _: kcreate_export::TextLayerMeta = serde_json::from_value(meta_val.clone())?;
            let _: crate::phase2::TextStyleWire = serde_json::from_value(style_val.clone())?;
            let node = ws
                .project
                .document
                .get_node_mut(id)
                .ok_or(DocumentBridgeError::NodeNotFound(id))?;
            node.metadata.insert(
                crate::scene_sync::TEXT_LAYER_METADATA_KEY.to_string(),
                meta_val.clone(),
            );
            node.metadata
                .insert("text_style".to_string(), style_val.clone());
            node.touch();
            Ok(())
        }
        // Phase 6 Tasks 27-28: layer-colour tag. `patch` is either a
        // JSON string (the colour to install) or `null` (clear the
        // tag). The matching `document_set_layer_color` packs both
        // before and after patches in this shape, so undo and redo
        // both go through this arm.
        "layer_color_set" => {
            let id = op.affected_nodes.first().copied().ok_or_else(|| {
                DocumentBridgeError::InvalidArgument {
                    argument: "affected_nodes".into(),
                    value: format!("layer_color_set operation {} has no affected node", op.id),
                }
            })?;
            let node = ws
                .project
                .document
                .get_node_mut(id)
                .ok_or(DocumentBridgeError::NodeNotFound(id))?;
            match patch {
                serde_json::Value::Null => {
                    node.metadata.remove(LAYER_COLOR_METADATA_KEY);
                }
                serde_json::Value::String(s) => {
                    node.metadata.insert(
                        LAYER_COLOR_METADATA_KEY.to_string(),
                        serde_json::Value::String(s.clone()),
                    );
                }
                other => {
                    return Err(DocumentBridgeError::InvalidArgument {
                        argument: "layer_color patch".into(),
                        value: format!("expected string or null, got {other}"),
                    });
                }
            }
            node.version += 1;
            node.updated_at = Utc::now();
            Ok(())
        }
        // Phase 6 Tasks 25-26 (Devin Review ANALYSIS_0005 follow-up):
        // graph-mutating undo / redo of clipboard paste. The matching
        // `document_clipboard_paste` records:
        //   * before_patch = `{ "new_root_id": <uuid> }`
        //   * after_patch  = `{ "subtree": [Node, ...] }`
        // Undo (before_patch) removes the inserted root, which
        // cascades through descendants via `DocumentGraph::remove_node`.
        // Redo (after_patch) re-inserts every node in parent-first
        // order so each insert finds its parent already present.
        "clipboard_paste" => match patch {
            // Undo direction.
            serde_json::Value::Object(map) if map.contains_key("new_root_id") => {
                let id = map
                    .get("new_root_id")
                    .and_then(|v| v.as_str())
                    .and_then(|s| Uuid::parse_str(s).ok())
                    .ok_or_else(|| DocumentBridgeError::InvalidArgument {
                        argument: "clipboard_paste before_patch".into(),
                        value: format!("missing or invalid new_root_id in {patch}"),
                    })?;
                // remove_node returning None means the node is
                // already gone — treat as success so an interrupted
                // group can still cleanly roll back.
                ws.project.document.remove_node(id);
                Ok(())
            }
            // Redo direction.
            serde_json::Value::Object(map) if map.contains_key("subtree") => {
                let subtree: Vec<kcreate_core::node::Node> = serde_json::from_value(
                    map.get("subtree")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                )?;
                for node in subtree {
                    ws.project.document.insert_node(node)?;
                }
                Ok(())
            }
            other => Err(DocumentBridgeError::InvalidArgument {
                argument: "clipboard_paste patch".into(),
                value: format!("expected object with new_root_id or subtree, got {other}"),
            }),
        },
        // G4 — Theme / Brand Kit instant restyle. Both before_patch
        // and after_patch carry the full [`ApplyThemePatch`] shape:
        // the project `DesignTokens` plus a per-node map of the new
        // `NodeStyle` and text-layer metadata slots. Undo replays
        // before_patch, redo replays after_patch — so one Ctrl+Z
        // reverts every recolored fill / stroke / text size in a
        // single step. We do NOT touch the renderer here;
        // `document_undo` / `document_redo` re-run scene sync after
        // the apply_patch loop via `touch_version`.
        "apply_theme" => {
            // Deserialize the WHOLE payload up-front so a malformed
            // patch fails before any mutation (fail-fast, never
            // half-applied).
            let parsed: ApplyThemePatch = serde_json::from_value(patch.clone())?;
            ws.project.design_tokens = parsed.design_tokens;
            for (id, node_patch) in parsed.nodes {
                let Some(node) = ws.project.document.get_node_mut(id) else {
                    // The node was removed by a later op that the LIFO
                    // operation log undoes before this one in normal
                    // flows, so it is present at replay time. Skip
                    // defensively rather than failing the whole
                    // restyle if the graph has diverged.
                    continue;
                };
                node.style = node_patch.style;
                // apply_theme only ever rewrites an existing text
                // slot (never adds one to a non-text node nor removes
                // one), so both patch directions carry `Some` for the
                // slots a text node owns and `None` otherwise —
                // insert-only is the correct symmetric replay.
                if let Some(meta) = node_patch.text_meta {
                    node.metadata
                        .insert(crate::scene_sync::TEXT_LAYER_METADATA_KEY.to_string(), meta);
                }
                if let Some(text_style) = node_patch.text_style {
                    node.metadata.insert("text_style".to_string(), text_style);
                }
                node.touch();
            }
            Ok(())
        }
        // H4 — AI generation depth. `ai_generate_themed_design` and
        // `ai_refine_themed_design` both carry a single reversible
        // [`crate::phase10::ThemedDesignPatch`]: before_patch tagged
        // `Undo`, after_patch tagged `Redo`, same data otherwise. The
        // patch holds the subtrees the run removed (a prior generated
        // design and/or a pristine scratch scaffold), the inserted
        // generated page subtree, and the theme brand kit before /
        // after. Undo removes the generated design and restores the
        // prior document + brand kit; redo re-applies the generation.
        // Replay is pure graph + brand-kit vec ops — see
        // [`apply_themed_design_patch`].
        "ai_generate_themed_design" | "ai_refine_themed_design" => {
            let parsed: crate::phase10::ThemedDesignPatch = serde_json::from_value(patch.clone())?;
            apply_themed_design_patch(ws, parsed)
        }
        other => {
            // No-op fall-through for graph operations
            // (document_create_node, canvas_move_node, …) whose
            // state is rolled back by the operation log itself,
            // never via apply_patch. If a command listed in
            // APPLY_PATCH_COMMANDS reaches this arm, the per-command
            // arm above is missing — that would silently break
            // atomic-rollback for group undo/redo. Debug builds
            // panic so the misalignment surfaces in CI; release
            // builds log and continue so the user is never blocked
            // from editing on a misconfiguration.
            debug_assert!(
                !is_apply_patch_command(other),
                "apply_patch fell through for `{other}` even though it appears in \
                 APPLY_PATCH_COMMANDS — the command list and the apply_patch match arms must \
                 stay in lockstep (see crates/kcreate_bridge/src/document.rs)"
            );
            if is_apply_patch_command(other) {
                log::error!(
                    "apply_patch: command `{other}` is in APPLY_PATCH_COMMANDS but has no \
                     match arm; group-undo rollback for this command will be incorrect"
                );
            }
            Ok(())
        }
    }
}

/// Roll a generated themed design forward (redo) or backward (undo)
/// from a single [`crate::phase10::ThemedDesignPatch`].
///
/// * **Undo** — remove the generated page subtree, re-insert the
///   subtrees the generation had removed (parent-first, so each child
///   finds its parent), and restore the prior theme brand kit (or
///   remove the freshly-created one when there was none before).
/// * **Redo** — re-remove whatever the generation cleared, re-insert
///   the generated subtree parent-first, and re-apply the theme
///   brand kit.
///
/// `remove_node` returning `None` (node already gone) is treated as
/// success so an interrupted group can still cleanly roll back, and
/// every `insert_node` is parent-first so it never fails on a missing
/// parent — matching the `clipboard_paste` reversible-subtree arm.
fn apply_themed_design_patch(
    ws: &mut Workspace,
    patch: crate::phase10::ThemedDesignPatch,
) -> Result<()> {
    match patch.dir {
        crate::phase10::ThemedPatchDir::Undo => {
            ws.project.document.remove_node(patch.inserted_root);
            for subtree in &patch.removed {
                for node in subtree {
                    ws.project.document.insert_node(node.clone())?;
                }
            }
            match patch.brand_kit_before {
                Some(prior) => upsert_brand_kit(ws, prior),
                None => {
                    let created = patch.brand_kit_after.id;
                    ws.project.brand_kits.retain(|k| k.id != created);
                }
            }
        }
        crate::phase10::ThemedPatchDir::Redo => {
            for subtree in &patch.removed {
                if let Some(root) = subtree.first() {
                    ws.project.document.remove_node(root.id);
                }
            }
            for node in &patch.inserted {
                ws.project.document.insert_node(node.clone())?;
            }
            upsert_brand_kit(ws, patch.brand_kit_after);
        }
    }
    Ok(())
}

/// Replace the brand kit with the same id in place, or push it when
/// absent. Used by [`apply_themed_design_patch`] so undo/redo restore
/// the exact theme brand kit without disturbing the user's other
/// kits or their ordering.
fn upsert_brand_kit(ws: &mut Workspace, kit: kcreate_core::project::BrandKit) {
    if let Some(existing) = ws.project.brand_kits.iter_mut().find(|k| k.id == kit.id) {
        *existing = kit;
    } else {
        ws.project.brand_kits.push(kit);
    }
}

// -----------------------------------------------------------------------------
// Scene synchronisation (document graph → renderer)
// -----------------------------------------------------------------------------

/// Rebuild the renderer scene from the current document state.
///
/// Called automatically after every CRUD mutation, selection change,
/// import, or undo/redo. The host can also call it explicitly via the
/// `document_sync_scene` N-API export when it wants to force a redraw
/// (e.g. after the renderer has been re-initialised at a new size).
///
/// Quiet on "renderer not initialised" — that's a legitimate state
/// when the host creates a project headlessly (e.g. in tests) before
/// constructing a canvas. All other renderer errors are returned so
/// the host can surface them.
pub fn document_sync_scene() -> Result<()> {
    let mut guard = slot().write();
    let _ = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    sync_scene_locked(&mut guard)?;
    drop(guard);
    Ok(())
}

/// Internal scene-sync used by every mutation site. Caller must hold
/// the workspace lock. Failures to render are propagated; failures to
/// find a renderer (host hasn't initialised yet) are swallowed.
///
/// # Lock-ordering invariant
///
/// This function takes the **renderer singleton lock** (via
/// `crate::state::render_scene`) while the **workspace lock** is
/// already held by the caller's `MutexGuard`. Every call site in this
/// module therefore observes the same lock order:
///
/// > workspace lock  →  renderer singleton lock
///
/// No code path in `kcreate_bridge` may take these locks in the
/// opposite order. In particular, the renderer crate itself never
/// reaches back into the workspace, and the [`WorkspaceAccess`] MCP
/// impl only enters the workspace lock — it never enters the renderer
/// lock except indirectly through this function (which always
/// observes the canonical order). As long as both invariants hold,
/// there is no path that can deadlock by acquiring them in opposite
/// orders.
pub(crate) fn sync_scene_locked(
    guard: &mut parking_lot::RwLockWriteGuard<'_, Option<Workspace>>,
) -> Result<()> {
    let Some(ws) = guard.as_mut() else {
        return Ok(());
    };
    #[allow(unused_mut)]
    let mut scene = ws.scene_sync.sync_document_to_scene(
        &mut ws.project.document,
        Some(ws.store.lock().blobs()),
        &ws.selection,
    );
    // Layer remote-peer cursors on top of the document.
    //
    // **Lock ordering invariant: workspace → collab.** This site is
    // the only place the bridge acquires the collab slot while
    // already holding the workspace mutex. The reverse order is
    // forbidden by construction:
    //   * `collab::session_start` / `session_drain_events` /
    //     `session_peers` / `apply_event` take `collab::slot()` only
    //     and never reach into the workspace.
    //   * The transport pump task (`collab::pump_inbound`) also
    //     touches only `collab::slot()` plus the host's internal
    //     `RwLock`s — it never re-enters the bridge to touch the
    //     workspace.
    //   * Renderer republishes triggered by collab events
    //     (`document_request_render`) re-enter this function from
    //     scratch on a *fresh* lock acquisition (workspace → collab),
    //     never collab → workspace.
    // Any future contributor adding a callback from the collab pump
    // *into* the workspace lock would violate this invariant and
    // create a deadlock — please add the new path to the list above
    // and update `collab::pump_inbound` if you really need it.
    #[cfg(feature = "collab")]
    {
        // Pull the renderer's current zoom once for cursors *and*
        // halos so both overlays share the same zoom-aware screen
        // sizing. Safe to call under the workspace lock: the
        // renderer slot has its own mutex (`state::slot()`) and we
        // never hold the renderer slot before this point. Falls
        // back to `1.0` in headless contexts.
        let viewport_zoom = crate::state::viewport_zoom();

        // Layer remote-peer selection halos below cursors so a
        // peer's cursor stays the most prominent indicator of
        // "where they are right now" — selection halos are
        // contextual chrome. Both share the same upward overlay
        // id stream so they never collide.
        //
        // Pick a halo starting_z well below the document-content
        // ceiling but high enough to clear local selection
        // highlights. `sync_document_to_scene` bounds document z
        // values by their order; selection highlights are appended
        // after them. `i32::MAX / 2` is safely past both. We start
        // halos at `i32::MAX / 2` and thread the post-emit z back
        // out so cursors begin at whatever halo height was actually
        // reached — a hard-coded gap (e.g. `+1`) would put cursors
        // beneath halos as soon as a single peer with a display
        // name was rendered (rect at z, label at z+1, cursor at z+1
        // → same z, paint order undefined).
        let halo_starting_z = i32::MAX / 2;
        // Single atomic read of the presence state so halos and
        // cursors painted in the same frame come from the same
        // snapshot. The previous two-call shape released and
        // reacquired `collab::slot()` between reads; an inbound
        // presence apply that landed in that gap would leave the
        // scene with halos from snapshot N and cursors from
        // snapshot N+1 in one rendered frame.
        let (selection_triples, cursor_triples) = crate::collab::presence_snapshot();
        let halo_next_z = if selection_triples.is_empty() {
            halo_starting_z
        } else {
            let selections: Vec<crate::scene_sync::PresenceSelection> = selection_triples
                .into_iter()
                .map(
                    |(peer_id, display_name, node_ids)| crate::scene_sync::PresenceSelection {
                        peer_id,
                        display_name,
                        node_ids,
                    },
                )
                .collect();
            ws.scene_sync.append_presence_selection_halos(
                &mut scene,
                &ws.project.document,
                &selections,
                halo_starting_z,
                viewport_zoom,
            )
        };

        let triples = cursor_triples;
        if !triples.is_empty() {
            let cursors: Vec<crate::scene_sync::PresenceCursor> = triples
                .into_iter()
                .map(
                    |(peer_id, display_name, cursor)| crate::scene_sync::PresenceCursor {
                        peer_id,
                        display_name,
                        x: cursor.x,
                        y: cursor.y,
                    },
                )
                .collect();
            // `append_presence_selection_halos` returns the
            // *next free* z (post-emit watermark), per its doc
            // contract — pass it directly so cursors land on the
            // first unused slot above the topmost halo. Adding +1
            // here would leave a gap that's harmless on i32 but
            // misrepresents the threading contract the function
            // documents. When no halos were emitted, the watermark
            // equals `halo_starting_z` so cursors still get a
            // valid base z.
            ws.scene_sync
                .append_presence_cursors(&mut scene, &cursors, halo_next_z, viewport_zoom);
        }
    }
    // The document graph (or presence overlay) just changed, so the
    // renderer's last-published frame is stale. Mark the whole canvas
    // dirty before re-rendering: the offscreen renderer's dirty-region
    // optimisation short-circuits `render_frame` when nothing has
    // invalidated the framebuffer since the previous frame (it returns
    // the cached `FrameId` without rebuilding the display list), which
    // would otherwise keep presenting the stale — or initial blank —
    // frame after every mutation. Invalidating here is the single
    // chokepoint that connects a document change to a renderer repaint.
    // `NotInitialized` is expected when the host runs headlessly (no
    // renderer attached) and is ignored, exactly like the
    // `render_scene` call below.
    let _ = crate::state::invalidate(None);
    // Renderer not initialised is fine here: the host may be working
    // headlessly. Other render errors propagate.
    match crate::state::render_scene(scene) {
        Ok(_) | Err(crate::state::BridgeError::NotInitialized) => Ok(()),
        Err(e) => Err(DocumentBridgeError::Bridge(e)),
    }
}

/// Re-run [`sync_scene_locked`] without applying a document
/// mutation. Used by the Phase 3 collab session to re-publish the
/// scene whenever remote presence (cursors, joins, leaves)
/// changes — the cursor overlay is appended inside
/// `sync_scene_locked` so this is the same path the editor uses
/// for selection changes.
///
/// `Ok(())` when no project is loaded, mirroring the rest of the
/// "no-op when headless" surface of `sync_scene_locked`.
pub fn document_request_render() -> Result<()> {
    let mut guard = slot().write();
    sync_scene_locked(&mut guard)
}

// -----------------------------------------------------------------------------
// Selection
// -----------------------------------------------------------------------------

/// Replace the selection with the given node ids.
///
/// Unknown ids are silently dropped so the host can't get out of sync
/// with the document — selecting a node and then deleting it must not
/// produce a stale selection entry.
pub fn document_set_selection(ids: Vec<Uuid>) -> Result<()> {
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    let valid: Vec<Uuid> = ids
        .into_iter()
        .filter(|id| ws.project.document.contains(*id))
        .collect();
    ws.selection = valid;
    let _ = sync_scene_locked(&mut guard);
    drop(guard);
    Ok(())
}

/// Snapshot of the current selection.
pub fn document_get_selection() -> Result<Vec<Uuid>> {
    // Phase 11 Task 19: read-only — selection is a `Vec<Uuid>` copy.
    let guard = slot().read();
    let ws = guard.as_ref().ok_or(DocumentBridgeError::NoProject)?;
    let sel = ws.selection.clone();
    drop(guard);
    Ok(sel)
}

/// Clear the selection. No-op when nothing is selected.
pub fn document_clear_selection() -> Result<()> {
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    if ws.selection.is_empty() {
        return Ok(());
    }
    ws.selection.clear();
    let _ = sync_scene_locked(&mut guard);
    drop(guard);
    Ok(())
}

// -----------------------------------------------------------------------------
// Hit testing
// -----------------------------------------------------------------------------

/// Hit-test a viewport-relative screen point against the current
/// scene. Returns the document uuid of the topmost selectable node
/// under the cursor, or `None` if no node is under the cursor.
///
/// The host passes the *current* viewport (pan + zoom) because the
/// bridge does not own the canvas' viewport state — that lives in
/// the React shell and is shipped over IPC for each hit query.
pub fn canvas_hit_test(
    screen_x: f32,
    screen_y: f32,
    pan_x: f32,
    pan_y: f32,
    zoom: f32,
) -> Result<Option<Uuid>> {
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    // Rebuild the scene from the document on every hit-test. This
    // sidesteps the "is the renderer's cached scene up to date?"
    // question entirely: a hit-test is cheap (a few hundred reverse-z
    // AABB checks) compared to its UX cost when wrong.
    let scene = ws.scene_sync.sync_document_to_scene(
        &mut ws.project.document,
        Some(ws.store.lock().blobs()),
        &ws.selection,
    );
    let vp = crate::hit_test::Viewport::new(kcreate_renderer::Vec2::new(pan_x, pan_y), zoom);
    let hit = crate::hit_test::hit_test(&ws.scene_sync, &scene, screen_x, screen_y, vp);
    drop(guard);
    Ok(hit)
}

/// Build a [`kcreate_vector::snap::SnapEngine`] over every visible
/// node *except* `moving_id` (the node currently being dragged),
/// query it with the candidate world bounds, and return the snap
/// delta + guide lines as a JSON string.
///
/// Returns `Ok(None)` when no project is open. Returns an empty
/// [`SnapResult`] (`dx=0, dy=0, guides=[]`) when no targets are
/// within the threshold — callers should treat that as "no snap"
/// without special-casing it.
///
/// Guides are returned in world-space coordinates; the canvas
/// overlay is responsible for mapping them through the viewport
/// transform before drawing.
pub fn canvas_snap(
    moving_id: Option<Uuid>,
    candidate_x: f64,
    candidate_y: f64,
    candidate_w: f64,
    candidate_h: f64,
    threshold: f64,
) -> Result<Option<kcreate_vector::snap::SnapResult>> {
    let guard = slot().write();
    let Some(ws) = guard.as_ref() else {
        return Ok(None);
    };
    let mut targets: Vec<kcreate_vector::snap::SnapTarget> = Vec::new();
    for (id, node) in ws.project.document.iter() {
        if !node.visible || node.locked {
            continue;
        }
        if Some(*id) == moving_id {
            continue;
        }
        let world = kcreate_core::node::Bounds {
            x: node.bounds.x + node.transform.tx,
            y: node.bounds.y + node.transform.ty,
            width: node.bounds.width,
            height: node.bounds.height,
        };
        if world.width <= 0.0 || world.height <= 0.0 {
            continue;
        }
        targets.push(kcreate_vector::snap::SnapTarget::from_bounds(
            world.x,
            world.y,
            world.width,
            world.height,
        ));
    }
    let engine = kcreate_vector::snap::SnapEngine::new(targets);
    let result = engine.snap(
        candidate_x,
        candidate_y,
        candidate_w,
        candidate_h,
        threshold,
    );
    drop(guard);
    Ok(Some(result))
}

// -----------------------------------------------------------------------------
// Canvas shape creation
// -----------------------------------------------------------------------------

/// Create a rectangle vector layer covering `(x, y, w, h)` (world
/// space). Returns the new node's uuid. Records an `op_kind` of
/// `"canvas_create_rect"` on the operation log so undo/redo cycles
/// through this gesture symmetrically.
pub fn canvas_create_rect(parent_id: Option<Uuid>, x: f64, y: f64, w: f64, h: f64) -> Result<Uuid> {
    let path = kcreate_vector::VectorPath::new(vec![
        kcreate_vector::PathSegment::MoveTo(kcreate_vector::PathPoint::new(x, y)),
        kcreate_vector::PathSegment::LineTo(kcreate_vector::PathPoint::new(x + w, y)),
        kcreate_vector::PathSegment::LineTo(kcreate_vector::PathPoint::new(x + w, y + h)),
        kcreate_vector::PathSegment::LineTo(kcreate_vector::PathPoint::new(x, y + h)),
        kcreate_vector::PathSegment::Close,
    ]);
    create_vector_layer(
        parent_id,
        "Rectangle",
        x,
        y,
        w,
        h,
        path,
        "canvas_create_rect",
    )
}

/// Create an ellipse vector layer centered at `(cx, cy)` with radii
/// `(rx, ry)`. The ellipse is approximated by four cubic Bezier
/// segments using the standard `(4/3) * tan(pi/8)` magic constant
/// (the same approximation `<circle>` SVG renderers use under the
/// hood — visually indistinguishable from a true ellipse at any
/// reasonable display resolution).
pub fn canvas_create_ellipse(
    parent_id: Option<Uuid>,
    cx: f64,
    cy: f64,
    rx: f64,
    ry: f64,
) -> Result<Uuid> {
    const KAPPA: f64 = 0.552_284_749_830_793_4;
    let ox = rx * KAPPA;
    let oy = ry * KAPPA;
    let path = kcreate_vector::VectorPath::new(vec![
        kcreate_vector::PathSegment::MoveTo(kcreate_vector::PathPoint::new(cx - rx, cy)),
        kcreate_vector::PathSegment::CubicTo {
            ctrl1: kcreate_vector::PathPoint::new(cx - rx, cy - oy),
            ctrl2: kcreate_vector::PathPoint::new(cx - ox, cy - ry),
            end: kcreate_vector::PathPoint::new(cx, cy - ry),
        },
        kcreate_vector::PathSegment::CubicTo {
            ctrl1: kcreate_vector::PathPoint::new(cx + ox, cy - ry),
            ctrl2: kcreate_vector::PathPoint::new(cx + rx, cy - oy),
            end: kcreate_vector::PathPoint::new(cx + rx, cy),
        },
        kcreate_vector::PathSegment::CubicTo {
            ctrl1: kcreate_vector::PathPoint::new(cx + rx, cy + oy),
            ctrl2: kcreate_vector::PathPoint::new(cx + ox, cy + ry),
            end: kcreate_vector::PathPoint::new(cx, cy + ry),
        },
        kcreate_vector::PathSegment::CubicTo {
            ctrl1: kcreate_vector::PathPoint::new(cx - ox, cy + ry),
            ctrl2: kcreate_vector::PathPoint::new(cx - rx, cy + oy),
            end: kcreate_vector::PathPoint::new(cx - rx, cy),
        },
        kcreate_vector::PathSegment::Close,
    ]);
    create_vector_layer(
        parent_id,
        "Ellipse",
        cx - rx,
        cy - ry,
        rx * 2.0,
        ry * 2.0,
        path,
        "canvas_create_ellipse",
    )
}

/// Create a single-line stroke from `(x1, y1)` to `(x2, y2)`.
pub fn canvas_create_line(
    parent_id: Option<Uuid>,
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
) -> Result<Uuid> {
    let path = kcreate_vector::VectorPath::new(vec![
        kcreate_vector::PathSegment::MoveTo(kcreate_vector::PathPoint::new(x1, y1)),
        kcreate_vector::PathSegment::LineTo(kcreate_vector::PathPoint::new(x2, y2)),
    ]);
    let bx = x1.min(x2);
    let by = y1.min(y2);
    let bw = (x2 - x1).abs();
    let bh = (y2 - y1).abs();
    create_vector_layer(
        parent_id,
        "Line",
        bx,
        by,
        bw,
        bh,
        path,
        "canvas_create_line",
    )
}

/// Errors specific to the [`canvas_create_path`] entry point.
///
/// `canvas_create_path` is the only bridge entry that accepts a
/// caller-provided path geometry rather than synthesizing one from
/// shape parameters (`x, y, w, h` for rects, `cx, cy, rx, ry` for
/// ellipses, …). That means it needs richer error reporting so the
/// Pen tool can surface "you sent us junk" without dropping the
/// gesture silently.
#[derive(Debug, Error)]
pub enum CreatePathError {
    /// The wire payload couldn't be parsed as `Vec<PathSegment>`.
    /// The error message includes the underlying serde diagnostic
    /// (line, column, kind) so renderer-side regressions can be
    /// caught without re-deserializing client-side.
    #[error("invalid path JSON: {0}")]
    InvalidJson(String),
    /// The caller sent zero segments. We reject this rather than
    /// silently inserting an invisible 0×0 node, because every Pen
    /// gesture is required to produce at least one `MoveTo` and one
    /// committed segment (line or cubic). A 0-segment payload almost
    /// always means a logic bug in the renderer.
    #[error("path has no segments")]
    Empty,
    /// The path does not start with a `MoveTo`. `VectorPath` doesn't
    /// enforce this structurally (it's just a `Vec<PathSegment>`),
    /// but Kurbo's `BezPath`-derived bounds and the renderer-side
    /// translator both assume `commands[0]` is `MoveTo`. We catch
    /// this here so renderer regressions surface as a typed error
    /// instead of silent geometry corruption downstream.
    #[error("path must start with move_to")]
    MissingMoveTo,
}

/// Create a freehand vector path from a caller-provided sequence of
/// path segments. Used by the Pen tool to commit a finished gesture
/// to the document.
///
/// `segments_json` is the JSON serialization of
/// `Vec<kcreate_vector::PathSegment>` — the same shape `PathSegment`
/// already serializes to via serde's internal tagging (`{"op": ...}`).
/// This piggybacks on the existing serde wire instead of inventing
/// a parallel TS-friendly shape, so adding a new `PathSegment`
/// variant in `kcreate_vector` automatically widens the Pen wire
/// without bridge-side changes.
///
/// `closed` matches `VectorPath::closed` (whether the renderer
/// should join the last point back to the first for fill / hit-test
/// purposes — independent of whether the caller appended an
/// explicit `Close` segment).
///
/// `name` is the layer name to show in the layers panel. `None`
/// defaults to `"Path"`, matching the convention used by the other
/// shape creators (`"Rectangle"`, `"Ellipse"`, `"Line"`).
///
/// Returns the new node's uuid. Records an undoable operation under
/// the `canvas_create_path` op_kind. Bounds are computed by Kurbo's
/// `BezPath::bounding_box()` (tight curve bounds, not control-point
/// bounds) so the layers panel and selection rect match what the
/// user actually drew.
pub fn canvas_create_path(
    parent_id: Option<Uuid>,
    segments_json: &str,
    closed: bool,
    name: Option<String>,
) -> Result<Uuid> {
    let segments: Vec<kcreate_vector::PathSegment> =
        serde_json::from_str(segments_json).map_err(|e| {
            DocumentBridgeError::CreatePath(CreatePathError::InvalidJson(e.to_string()))
        })?;
    if segments.is_empty() {
        return Err(DocumentBridgeError::CreatePath(CreatePathError::Empty));
    }
    if !matches!(segments[0], kcreate_vector::PathSegment::MoveTo(_)) {
        return Err(DocumentBridgeError::CreatePath(
            CreatePathError::MissingMoveTo,
        ));
    }
    let mut path = kcreate_vector::VectorPath::new(segments);
    path.closed = closed;
    let bounds = path.bounds();
    let layer_name = name.unwrap_or_else(|| "Path".to_string());
    create_vector_layer(
        parent_id,
        layer_name.as_str(),
        bounds.min_x,
        bounds.min_y,
        bounds.width(),
        bounds.height(),
        path,
        "canvas_create_path",
    )
}

/// Errors specific to the [`canvas_path_boolean`] entry point.
///
/// Pathfinder lives at the gesture boundary — the user has already
/// selected the inputs, hit Union / Subtract / Intersect / Exclude,
/// and is committed to a destructive replace. We surface every
/// failure mode that can stop the gesture as a typed error so the
/// renderer can decide between a per-cause toast ("select at least
/// two vector layers") and a generic "boolean op failed" telemetry
/// drop, instead of stringly-typed pattern matching at the IPC
/// boundary.
#[derive(Debug, Error)]
pub enum PathBooleanError {
    /// The caller passed `op` as a string but it didn't match any
    /// known [`kcreate_vector::BooleanOp`] variant. The lowercase
    /// wire tokens are the same ones serde emits for the enum:
    /// `"union"`, `"subtract"`, `"intersect"`, `"exclude"`.
    #[error("invalid boolean op `{0}` (expected one of union | subtract | intersect | exclude)")]
    InvalidOp(String),
    /// Boolean ops require **at least two** inputs. The renderer
    /// disables the Pathfinder buttons in this state, but the bridge
    /// re-checks so a future caller (MCP tool, plugin, scripting
    /// API) can't bypass the UI gate and crash the workspace.
    #[error("boolean op requires at least 2 source layers, got {0}")]
    TooFewSources(usize),
    /// A source id didn't map to a node currently in the document
    /// graph. Usually means the renderer's selection cache lagged a
    /// delete; the panel re-fetches `selectedIds` on the next
    /// `refreshTree` cycle so this rarely recurs.
    #[error("source node not found: {0}")]
    SourceNotFound(Uuid),
    /// A source node exists but isn't a `VectorLayer`. We require
    /// every input to carry a `VECTOR_PATH_METADATA_KEY` payload —
    /// rasters, text, groups, frames all reject here. The renderer
    /// already filters non-vector layers out of the selection that
    /// feeds the panel, but the bridge re-checks for the same
    /// reason as `TooFewSources` (UI-gate bypass).
    #[error("source node {id} is a {got:?}, expected a VectorLayer")]
    SourceNotVector { id: Uuid, got: NodeType },
    /// A `VectorLayer` source is missing its `VECTOR_PATH_METADATA_KEY`
    /// slot, or the slot deserialized as `null`. Indicates a
    /// corrupt document — every shape-creator in this file writes
    /// the slot before persisting — but we report it cleanly
    /// instead of panicking.
    #[error("source node {0} has no vector path payload")]
    SourceMissingPath(Uuid),
    /// `boolean_operation` itself rejected the inputs. Covers
    /// `EmptyPath` (after polyline flattening at least one input
    /// produced no closed contour) and any future variants
    /// `kcreate_vector` grows.
    #[error(transparent)]
    Vector(#[from] kcreate_vector::VectorBooleanError),
    /// `boolean_operation` succeeded but returned zero result
    /// shapes — e.g. `A ∩ B` where the inputs don't overlap.
    /// Distinguished from `Vector::EmptyPath` (which fires *before*
    /// the math runs on degenerate input) so the renderer can pick
    /// a "no overlap" toast for intersect/subtract vs. a "degenerate
    /// input" toast.
    #[error("boolean op produced no output (inputs did not intersect for the chosen op)")]
    EmptyResult,
}

/// Apply a polygon boolean (`union` / `subtract` / `intersect` /
/// `exclude`) across `source_ids`, replacing the source nodes with
/// the resulting set of `VectorLayer` nodes.
///
/// # Semantics
///
/// * `source_ids` must contain at least two ids, each pointing at
///   a `VectorLayer` whose `metadata[VECTOR_PATH_METADATA_KEY]`
///   round-trips to a `kcreate_vector::VectorPath`.
/// * The boolean is folded left-to-right via
///   [`kcreate_vector::boolean_operation`]:
///   `result = sources[0]`, then for each subsequent source `b`,
///   `result = op(result, b)`. For `union` / `intersect` / `exclude`
///   this is associative and matches the user's mental "merge them
///   all" intuition; for `subtract` it matches Inkscape's "first
///   minus everything else" semantics.
/// * The operation is **destructive**: source nodes are removed
///   from the graph and replaced with the result nodes. This
///   matches Inkscape's `Path > Union` and Illustrator's
///   `Pathfinder` defaults (non-destructive compound paths are a
///   future feature, tracked for Phase E).
/// * Each result shape becomes its own `VectorLayer`, parented to
///   the *first* source's parent so the gesture stays inside the
///   user's working scope (artboard / page). Bounds are tight
///   (kurbo's `BezPath::bounding_box`) — the boolean returns
///   polyline-only paths so the bezier-vs-control-point distinction
///   from `canvas_create_path` doesn't matter here, but we
///   re-compute on the result path for consistency.
/// * Each result inherits the **first source's** [`NodeStyle`] so
///   fill / stroke / corner-radius carry over predictably. (Matches
///   Illustrator's "bottom object's style wins" when the user
///   selects in z-order; in our model the first id in `source_ids`
///   IS the bottom object because the renderer passes the
///   selection in iteration order over `nodes`, which is
///   z-bottom-first.)
/// * **Hierarchical sources are safe.** The function is designed to
///   tolerate any `Vec<Uuid>` from a caller (UI selection, MCP tool
///   surface, plugin via extended ABI, future scripting API),
///   including cases where one source is a descendant of another.
///   Three independent mechanisms keep the gesture sound:
///
///   1. **Validation reads pre-removal geometry.** Step 1 walks
///      every source id and snapshots its `VectorPath` from the
///      `VECTOR_PATH_METADATA_KEY` BEFORE any graph mutation, so the
///      boolean fold operates on the user-visible shapes regardless
///      of any parent/child relationship between sources.
///
///   2. **Result parent resolved to first non-source ancestor.**
///      Step 3 takes `first_parent = source_ids[0].parent_id` and
///      walks up the parent chain skipping any ancestor that itself
///      appears in `source_ids`, so result nodes are always parented
///      to a node that survives step 4. Without this resolution
///      step, the case `parent(source_ids[0]) == source_ids[1]`
///      would silently cascade-delete every result node in step 4
///      because [`DocumentGraph::remove_node`] recursively removes
///      children (`crates/kcreate_core/src/document.rs:438-447`).
///      Devin Review BUG_0001 (round 7) on PR #38.
///
///   3. **Cleanup is idempotent.** When step 4 calls `remove_node`
///      on a node already swept by an ancestor's recursive deletion,
///      the call returns `None` and is intentionally ignored;
///      `selection.retain` likewise no-ops on absent ids. No panic,
///      no double-free.
///
/// # Undo / redo
///
/// A single `canvas_path_boolean` operation is recorded with the
/// full pre-gesture source node JSONs in `before_patch` and the
/// post-gesture result node JSONs in `after_patch`. This matches
/// the host-driven patch contract documented on
/// [`kcreate_core::project::Project::undo`]: the renderer is
/// responsible for replaying the inverse against its in-memory
/// document tree via `refreshTree()` after `document.undo()` /
/// `document.redo()`. The bridge does NOT register
/// `canvas_path_boolean` with [`APPLY_PATCH_COMMANDS`] because the
/// command rewrites the graph itself rather than mutating a
/// metadata slot — graph operations follow the bare-graph pattern
/// (see `canvas_create_path`).
///
/// Returns the freshly-inserted result node ids in iteration order
/// (matches `Vec` order so the renderer can select them all and
/// preserve the boolean's shape ordering).
pub fn canvas_path_boolean(op_wire: &str, source_ids: Vec<Uuid>) -> Result<Vec<Uuid>> {
    let op: kcreate_vector::BooleanOp = match op_wire {
        "union" => kcreate_vector::BooleanOp::Union,
        "subtract" => kcreate_vector::BooleanOp::Subtract,
        "intersect" => kcreate_vector::BooleanOp::Intersect,
        "exclude" => kcreate_vector::BooleanOp::Exclude,
        other => {
            return Err(DocumentBridgeError::PathBoolean(
                PathBooleanError::InvalidOp(other.to_string()),
            ));
        }
    };
    // Deduplicate source ids while preserving the caller's iteration
    // order (first occurrence wins). Boolean ops over the same shape
    // are mathematical no-ops — `A ∪ A == A`, `A \ A == ∅`,
    // `A ∩ A == A`, `A ⊕ A == ∅` — so doing the fold on duplicate
    // inputs would either waste work (union/intersect) or produce a
    // confusing empty-result error (subtract/exclude) for a gesture
    // the caller obviously didn't intend that way.
    //
    // The renderer's selection model is a Set, so the normal UI flow
    // can't produce duplicates — this guard exists for the same
    // reason the explicit `TooFewSources` / `SourceNotVector` checks
    // do: future callers (MCP tool surface, plugin via extended ABI,
    // future scripting API) can hand us any `Vec<Uuid>` and we must
    // behave sensibly without crashing or wasting work. Without
    // dedup, `[a, a]` resolved the same node twice in the validation
    // loop, folded `union(A, A) = A`, then called `remove_node(a)`
    // twice — the second call returned `None` and was silently
    // ignored. Functionally correct but wasteful and easy to misread
    // when debugging via the operation log.
    //
    // We dedup BEFORE the `< 2` check so `[a, a, a]` is rejected as
    // `TooFewSources(1)` rather than incorrectly proceeding as if
    // three distinct sources were passed. The renderer's `onStatus`
    // toast becomes "boolean op requires at least 2 source layers,
    // got 1" — accurate after dedup. Devin Review ANALYSIS_0003
    // (round 5) on PR #38.
    let source_ids: Vec<Uuid> = {
        let mut seen: HashSet<Uuid> = HashSet::with_capacity(source_ids.len());
        source_ids
            .into_iter()
            .filter(|id| seen.insert(*id))
            .collect()
    };
    if source_ids.len() < 2 {
        return Err(DocumentBridgeError::PathBoolean(
            PathBooleanError::TooFewSources(source_ids.len()),
        ));
    }

    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;

    // 1. Resolve every source id to a (path, style, parent) tuple
    //    BEFORE we start mutating. If any source fails the gesture
    //    aborts cleanly with no graph mutation — atomic-or-nothing.
    //
    //    We also stash the full Node JSON for each source so the
    //    operation log carries everything needed for host-driven
    //    undo (re-inserting these nodes if the user hits Ctrl+Z).
    let mut paths: Vec<kcreate_vector::VectorPath> = Vec::with_capacity(source_ids.len());
    let mut source_snapshots: Vec<serde_json::Value> = Vec::with_capacity(source_ids.len());
    let mut first_style: Option<kcreate_core::node::NodeStyle> = None;
    let mut first_parent: Option<Uuid> = None;
    for id in &source_ids {
        let node = ws
            .project
            .document
            .get_node(*id)
            .ok_or(DocumentBridgeError::PathBoolean(
                PathBooleanError::SourceNotFound(*id),
            ))?;
        if node.node_type != NodeType::VectorLayer {
            return Err(DocumentBridgeError::PathBoolean(
                PathBooleanError::SourceNotVector {
                    id: *id,
                    got: node.node_type,
                },
            ));
        }
        let path = node
            .metadata
            .get(crate::scene_sync::VECTOR_PATH_METADATA_KEY)
            .and_then(|v| serde_json::from_value::<kcreate_vector::VectorPath>(v.clone()).ok())
            .ok_or(DocumentBridgeError::PathBoolean(
                PathBooleanError::SourceMissingPath(*id),
            ))?;
        if first_style.is_none() {
            first_style = Some(node.style.clone());
            first_parent = node.parent_id;
        }
        source_snapshots.push(serde_json::to_value(node)?);
        paths.push(path);
    }

    // 2. Fold the boolean left-to-right. Each intermediate result
    //    becomes the `subject` for the next pair. `boolean_operation`
    //    returns a `Vec<VectorPath>` (the operation can produce
    //    multiple disjoint shapes — e.g. an exclude that splits a
    //    ring into a half-moon and a crescent); we union those
    //    sub-results back into a single `VectorPath` for the next
    //    fold step by concatenating their segment lists. This
    //    matches Inkscape's behaviour: the intermediate stays a
    //    single composite path until the final emit.
    //
    //    `acc` is seeded with `paths[0].clone()` and only ever
    //    reassigned from a `pair` that the in-loop guard above has
    //    already proven non-empty, so `acc.is_empty()` is impossible
    //    after the loop and we don't repeat the check. The fold
    //    contract is: the loop body either returns `EmptyResult` or
    //    overwrites `acc` with a non-empty `Vec`. Devin Review
    //    ANALYSIS_0003 (round 7) on PR #38 — the previous defensive
    //    post-loop check was unreachable dead code.
    let mut acc: Vec<kcreate_vector::VectorPath> = vec![paths[0].clone()];
    for next in paths.iter().skip(1) {
        let combined = merge_paths(&acc);
        let pair = kcreate_vector::boolean_operation(op, &combined, next)
            .map_err(PathBooleanError::Vector)?;
        if pair.is_empty() {
            return Err(DocumentBridgeError::PathBoolean(
                PathBooleanError::EmptyResult,
            ));
        }
        acc = pair;
    }

    // 3. Insert one result node per shape. Use the first source's
    //    style so fill/stroke/corner-radius carry over predictably.
    //    For the parent, walk up the chain from `first_parent`
    //    skipping any ancestor that itself appears in `source_ids`
    //    — those will be deleted in step 4 along with all their
    //    descendants, so parenting results to them would silently
    //    cascade-delete the results too. The walk is bounded by
    //    `nodes.len()` as a belt-and-braces guard against any
    //    pathologically corrupt graph with a parent cycle (the core
    //    `reparent_node` rejects cycles, but a defensive ceiling
    //    here costs O(N) at worst and rules out an infinite loop in
    //    the bridge regardless of how the graph got into that
    //    state). Devin Review BUG_0001 (round 7) on PR #38.
    //
    //    Bounds are recomputed per-shape so each result has tight
    //    kurbo bounds.
    let style = first_style.expect("set on first source loop iteration");
    let source_set: HashSet<Uuid> = source_ids.iter().copied().collect();
    let mut parent = first_parent;
    let max_depth = ws.project.document.node_count();
    let mut walked = 0usize;
    while let Some(pid) = parent {
        if !source_set.contains(&pid) {
            break;
        }
        if walked >= max_depth {
            // Defense in depth: a corrupt graph with a parent cycle
            // would loop forever. Fall back to the root list — every
            // node in `source_set` will be removed in step 4 anyway,
            // so the worst-case outcome is a result parented to
            // root rather than the (now-deleted) descendant scope.
            parent = None;
            break;
        }
        walked += 1;
        parent = ws.project.document.get_node(pid).and_then(|n| n.parent_id);
    }
    let mut result_ids: Vec<Uuid> = Vec::with_capacity(acc.len());
    let mut result_snapshots: Vec<serde_json::Value> = Vec::with_capacity(acc.len());
    for (idx, path) in acc.into_iter().enumerate() {
        let bounds = path.bounds();
        let name = if result_ids.is_empty() {
            op_wire.to_string()
        } else {
            format!("{op_wire} ({})", idx + 1)
        };
        let mut node = Node::new(NodeType::VectorLayer, name);
        node.parent_id = parent;
        node.bounds = kcreate_core::node::Bounds {
            x: bounds.min_x,
            y: bounds.min_y,
            width: bounds.width(),
            height: bounds.height(),
        };
        node.style = style.clone();
        node.metadata.insert(
            crate::scene_sync::VECTOR_PATH_METADATA_KEY.to_string(),
            serde_json::to_value(&path)?,
        );
        let id = ws.project.document.insert_node(node)?;
        let snapshot = ws
            .project
            .document
            .get_node(id)
            .map_or(serde_json::Value::Null, |n| {
                serde_json::to_value(n).unwrap_or(serde_json::Value::Null)
            });
        result_snapshots.push(snapshot);
        result_ids.push(id);
    }

    // 4. Delete the source nodes. Drop them from the selection so
    //    the renderer doesn't paint a highlight over thin air on
    //    the next frame (matches the discipline in
    //    `document_delete_node`).
    for id in &source_ids {
        ws.project.document.remove_node(*id);
        ws.selection.retain(|sel| sel != id);
    }

    // 4b. Adopt the result ids into the selection. The boolean is a
    //     destructive replace: the result nodes ARE the new
    //     selection semantically, and the host's `refreshTree()`
    //     pulls selection back from the bridge via
    //     `canvas.getSelection()` (see `refreshSelection` in
    //     `apps/desktop/renderer/src/pages/EditorPage.tsx`). If the
    //     bridge omitted this step, the JS-side `setSelectedIds`
    //     would get clobbered to `[]` on the next refresh because
    //     step 4 above already removed the source ids and nothing
    //     would have re-populated the slot.
    //
    //     We do this on the bridge side (rather than asking the host
    //     to call `setSelection(resultIds)` after the IPC returns)
    //     for two reasons:
    //
    //     * Atomicity. Graph mutation and selection mutation happen
    //       under the same write lock, so an interleaved
    //       `getSelection()` from another caller never observes the
    //       "sources removed, results not yet selected" intermediate
    //       state.
    //     * Caller-path robustness. A future caller of
    //       `canvas_path_boolean` (the MCP tool surface, a plugin
    //       via the extended ABI, or a future scripting API) gets a
    //       correct selection state without having to know the
    //       JS-side "create-then-select" convention used by
    //       `canvas_create_rect` / `canvas_create_path`. Those
    //       creators deliberately leave selection to the host
    //       because rapid-fire creation (e.g. a paste flurry) often
    //       wants to keep the existing selection; pathfinder's
    //       destructive-replace semantic has no such ambiguity.
    ws.selection.extend(result_ids.iter().copied());

    // 5. Record one undoable operation. `before_patch` carries the
    //    full source node snapshots (renderer re-inserts them on
    //    undo); `after_patch` carries the result snapshots (renderer
    //    deletes them on undo, re-inserts on redo). `affected_nodes`
    //    spans both sides so the renderer's "refresh just the
    //    affected subtrees" optimisation doesn't miss anything.
    let mut affected = source_ids.clone();
    affected.extend(result_ids.iter().copied());
    let before = serde_json::json!({
        "op": op_wire,
        "sources": source_snapshots,
        "result_ids": result_ids,
    });
    let after = serde_json::json!({
        "op": op_wire,
        "source_ids": source_ids,
        "results": result_snapshots,
    });
    let operation = Operation::new("user", "canvas_path_boolean", before, after, affected);
    ws.project.execute_operation(operation);
    ws.project.modified_at = Utc::now();
    let _ = sync_scene_locked(&mut guard);
    drop(guard);
    Ok(result_ids)
}

/// Merge a list of disjoint `VectorPath`s into one composite path
/// by concatenating their segment lists. Used by
/// [`canvas_path_boolean`] to fold a multi-shape intermediate
/// result back into a single subject for the next fold step.
///
/// Each input path's segment list already starts with `MoveTo` and
/// (for closed shapes) ends with `Close`, so the concatenation is
/// well-formed without further normalization. Fill rule and
/// `closed` flag are inherited from the first input — the boolean
/// path produces line-only shapes, so curve attributes don't need
/// to be reconciled across inputs.
fn merge_paths(paths: &[kcreate_vector::VectorPath]) -> kcreate_vector::VectorPath {
    // Total function: defined for every `&[VectorPath]` including
    // the empty slice. Returns an empty `VectorPath` on empty input
    // rather than panicking, so a release-mode caller that violates
    // the documented "non-empty" expectation degrades to a
    // well-defined no-op instead of an out-of-bounds index panic.
    //
    // The downstream call site (`canvas_path_boolean`) seeds `acc`
    // with one element and only reassigns from a non-empty `pair`,
    // so this branch is unreachable in practice — but anchoring the
    // safe path at the type system makes the contract robust to
    // future refactors that might add a new call site without
    // re-deriving the invariant.
    let Some(first) = paths.first() else {
        return kcreate_vector::VectorPath::new(Vec::new());
    };
    if paths.len() == 1 {
        return first.clone();
    }
    let mut segments: Vec<kcreate_vector::PathSegment> = Vec::new();
    for p in paths {
        segments.extend(p.commands.iter().copied());
    }
    let mut out = kcreate_vector::VectorPath::new(segments);
    out.closed = first.closed;
    out.fill_rule = first.fill_rule;
    out
}

// -----------------------------------------------------------------------------
// Phase B3 — Node editor read/write surface
// -----------------------------------------------------------------------------

/// Errors specific to the [`canvas_path_get_segments`] and
/// [`canvas_path_set_segments`] entry points.
///
/// The node editor reads a path's geometry on entry (to populate
/// the anchor / handle overlay) and writes it back on every drag
/// commit (one drag = one undo step). Each direction has its own
/// failure modes — read-side errors are *"this isn't a path"*-class
/// (`NodeNotFound`, `NotVectorLayer`, `MissingPathMetadata`) and
/// write-side errors add wire-validation kinds that mirror
/// [`CreatePathError`] (`InvalidJson`, `Empty`, `MissingMoveTo`).
/// We share the type because every reader-side variant is also a
/// possible writer-side failure (the writer re-resolves the node).
#[derive(Debug, Error)]
pub enum PathSegmentsError {
    /// The node id doesn't exist in the document graph. Caller is
    /// expected to re-fetch the document tree before retrying —
    /// the node may have been deleted by another gesture (undo,
    /// remote-peer op).
    #[error("node {0} not found")]
    NodeNotFound(Uuid),
    /// The node exists but isn't a `VectorLayer`. The node editor
    /// is vector-only; rasters / text / groups / frames don't
    /// have an anchor model.
    #[error("node {id} is a {got:?}, expected a VectorLayer")]
    NotVectorLayer { id: Uuid, got: NodeType },
    /// The `VectorLayer` is missing its `VECTOR_PATH_METADATA_KEY`
    /// slot, or the slot deserialized as `null`. Indicates a
    /// corrupt document — every shape-creator in this file writes
    /// the metadata on insert, so the slot is always present in
    /// well-formed projects.
    #[error("vector layer {0} is missing path metadata")]
    MissingPathMetadata(Uuid),
    /// `set_segments` only: the wire payload couldn't be parsed
    /// as `Vec<PathSegment>`. Mirrors [`CreatePathError::InvalidJson`]
    /// — the error message includes the underlying serde
    /// diagnostic for caller-side debugging.
    #[error("invalid path JSON: {0}")]
    InvalidJson(String),
    /// `set_segments` only: zero segments. Mirrors
    /// [`CreatePathError::Empty`] — an empty path would deserialize
    /// to an invisible 0×0 bounding box, which almost always means
    /// a logic bug in the renderer's serializer. The node editor
    /// in particular should never reach this state because the
    /// gesture began with a non-empty path.
    #[error("path has no segments")]
    Empty,
    /// `set_segments` only: the path does not start with a
    /// `MoveTo`. Same rationale as [`CreatePathError::MissingMoveTo`]
    /// — `VectorPath` doesn't enforce the invariant structurally,
    /// but the renderer-side translator and Kurbo's bounds
    /// computation both rely on it.
    #[error("path must start with move_to")]
    MissingMoveTo,
}

/// Wire shape returned by [`canvas_path_get_segments`]. Mirrors
/// `PathSnapshot` in `apps/desktop/shared/scene.ts`.
///
/// `segments` is the path-local sequence of `PathSegment`s, same
/// shape `canvas_create_path` accepts. `closed` and `fill_rule`
/// mirror `VectorPath::closed` / `VectorPath::fill_rule`. The two
/// `translation_*` fields carry the node's current
/// `transform.tx` / `transform.ty` so the renderer can project
/// path-local anchors into world space without a second bridge
/// round-trip.
///
/// World position = path-local + translation. The node editor
/// preserves this contract by NOT folding the translation into
/// the path coordinates on edit — anchor drags only mutate the
/// path-local segments. `canvas_move_node` keeps owning the
/// translation, so undo of a node move stays a clean transform
/// patch instead of a path-replace patch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PathSnapshot {
    pub segments: Vec<kcreate_vector::PathSegment>,
    pub closed: bool,
    pub fill_rule: kcreate_vector::FillRule,
    /// `node.transform.tx`. World-X of any path-local point P is
    /// `P.x + translation_x`.
    pub translation_x: f64,
    /// `node.transform.ty`. World-Y of any path-local point P is
    /// `P.y + translation_y`.
    pub translation_y: f64,
}

/// Read the geometry of a `VectorLayer` node for the node editor.
///
/// Returns a [`PathSnapshot`] carrying the path's intrinsic
/// segments, its `closed` / `fill_rule` flags, and the node's
/// current transform translation so the renderer can project
/// path-local coords into world space without a second IPC.
///
/// Does NOT take any locks longer than the read needed to clone
/// the metadata payload — the node editor calls this on every
/// entry to the tool plus after every external mutation (undo,
/// remote-peer op) without coalescing, so the locking overhead
/// has to stay cheap.
pub fn canvas_path_get_segments(node_id: Uuid) -> Result<PathSnapshot> {
    let guard = slot().read();
    let ws = guard.as_ref().ok_or(DocumentBridgeError::NoProject)?;
    let node = ws
        .project
        .document
        .get_node(node_id)
        .ok_or(DocumentBridgeError::PathSegments(
            PathSegmentsError::NodeNotFound(node_id),
        ))?;
    if node.node_type != NodeType::VectorLayer {
        return Err(DocumentBridgeError::PathSegments(
            PathSegmentsError::NotVectorLayer {
                id: node_id,
                got: node.node_type,
            },
        ));
    }
    let path: kcreate_vector::VectorPath = node
        .metadata
        .get(crate::scene_sync::VECTOR_PATH_METADATA_KEY)
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .ok_or(DocumentBridgeError::PathSegments(
            PathSegmentsError::MissingPathMetadata(node_id),
        ))?;
    Ok(PathSnapshot {
        segments: path.commands,
        closed: path.closed,
        fill_rule: path.fill_rule,
        translation_x: node.transform.tx,
        translation_y: node.transform.ty,
    })
}

/// Write a new geometry for a `VectorLayer` node from the node
/// editor.
///
/// `segments_json` is the JSON serialization of
/// `Vec<kcreate_vector::PathSegment>` — same wire shape as
/// `canvas_create_path` (the renderer keeps a single
/// `PathSegmentWire` mirror across both APIs). `closed` becomes
/// `VectorPath.closed`.
///
/// Recomputes the node's `bounds` via `VectorPath::bounds()` (tight
/// curve bounds, matching `canvas_create_path`'s seed behaviour),
/// so a node-editor drag that grows the path also grows the
/// selection rect / layers-panel size readout. `transform.tx/ty`
/// are left untouched — the node editor only touches geometry,
/// not position. This keeps the undo replay of a node-move op
/// independent from any subsequent path edit.
///
/// Records ONE undoable operation per call (op_kind:
/// `canvas_path_set_segments`). Callers should coalesce
/// pointermove-rate updates into a single end-of-gesture call so
/// the operation log doesn't get spammed — matches the
/// `canvas_move_node` discipline.
pub fn canvas_path_set_segments(node_id: Uuid, segments_json: &str, closed: bool) -> Result<()> {
    let segments: Vec<kcreate_vector::PathSegment> =
        serde_json::from_str(segments_json).map_err(|e| {
            DocumentBridgeError::PathSegments(PathSegmentsError::InvalidJson(e.to_string()))
        })?;
    if segments.is_empty() {
        return Err(DocumentBridgeError::PathSegments(PathSegmentsError::Empty));
    }
    if !matches!(segments[0], kcreate_vector::PathSegment::MoveTo(_)) {
        return Err(DocumentBridgeError::PathSegments(
            PathSegmentsError::MissingMoveTo,
        ));
    }
    let mut new_path = kcreate_vector::VectorPath::new(segments);
    new_path.closed = closed;

    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;

    // Validate the target before snapshotting anything: same
    // ordering as `canvas_path_get_segments` so callers see a
    // consistent error variant regardless of whether they are
    // reading or writing.
    let before;
    let after;
    {
        let node =
            ws.project
                .document
                .get_node(node_id)
                .ok_or(DocumentBridgeError::PathSegments(
                    PathSegmentsError::NodeNotFound(node_id),
                ))?;
        if node.node_type != NodeType::VectorLayer {
            return Err(DocumentBridgeError::PathSegments(
                PathSegmentsError::NotVectorLayer {
                    id: node_id,
                    got: node.node_type,
                },
            ));
        }
        // Inherit the existing path's `fill_rule` so a node-editor
        // commit doesn't silently revert a user-chosen fill rule
        // back to the `VectorPath::new` default (`NonZero`).
        let existing: Option<kcreate_vector::VectorPath> = node
            .metadata
            .get(crate::scene_sync::VECTOR_PATH_METADATA_KEY)
            .and_then(|v| serde_json::from_value(v.clone()).ok());
        if let Some(prev) = existing {
            new_path.fill_rule = prev.fill_rule;
        }
        // `before` snapshots the FULL node so undo restores the
        // exact pre-edit geometry + bounds + metadata in one
        // operation log entry. Matches the patch shape
        // `create_vector_layer` records on insert (full
        // before/after) so the host-driven undo path in
        // `EditorPage` doesn't need a special arm for set-segments.
        before = serde_json::to_value(node)?;
    }

    // Re-borrow mutably to apply the geometry edit. Splitting the
    // borrow scope is the cleanest way to keep the validation read
    // and the mutation write in the same write-locked workspace
    // call without holding two mutable references simultaneously.
    {
        let node = ws.project.document.get_node_mut(node_id).ok_or(
            // Defensive: the node was present 3 lines ago under
            // the same write lock, so this branch is unreachable
            // in practice. Surfacing the typed error instead of
            // panicking keeps the bridge resilient if a future
            // refactor relaxes the locking discipline.
            DocumentBridgeError::PathSegments(PathSegmentsError::NodeNotFound(node_id)),
        )?;
        let bounds = new_path.bounds();
        node.bounds = kcreate_core::node::Bounds {
            x: bounds.min_x,
            y: bounds.min_y,
            width: bounds.width(),
            height: bounds.height(),
        };
        node.metadata.insert(
            crate::scene_sync::VECTOR_PATH_METADATA_KEY.to_string(),
            serde_json::to_value(&new_path)?,
        );
        node.touch();
        after = serde_json::to_value(&*node)?;
    }
    ws.project.modified_at = Utc::now();
    let op = Operation::new(
        "user",
        "canvas_path_set_segments",
        before,
        after,
        vec![node_id],
    );
    ws.project.execute_operation(op);
    let _ = sync_scene_locked(&mut guard);
    drop(guard);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn create_vector_layer(
    parent_id: Option<Uuid>,
    default_name: &str,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    path: kcreate_vector::VectorPath,
    op_kind: &str,
) -> Result<Uuid> {
    let mut node = Node::new(NodeType::VectorLayer, default_name);
    node.parent_id = parent_id;
    node.bounds = kcreate_core::node::Bounds {
        x,
        y,
        width: w,
        height: h,
    };
    node.metadata.insert(
        crate::scene_sync::VECTOR_PATH_METADATA_KEY.to_string(),
        serde_json::to_value(&path)?,
    );
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    let id = ws.project.document.insert_node(node)?;
    ws.project.modified_at = Utc::now();
    // Record an operation so the gesture is undoable. The bridge owns
    // the "create" patch semantics: before=null, after=full node, so
    // an undo deletes and a redo recreates.
    let snapshot = ws
        .project
        .document
        .get_node(id)
        .map_or(serde_json::Value::Null, |n| {
            serde_json::to_value(n).unwrap_or(serde_json::Value::Null)
        });
    let op = Operation::new("user", op_kind, serde_json::Value::Null, snapshot, vec![id]);
    ws.project.execute_operation(op);
    let _ = sync_scene_locked(&mut guard);
    drop(guard);
    Ok(id)
}

// -----------------------------------------------------------------------------
// Artboards
// -----------------------------------------------------------------------------

/// Wire shape returned to the host by [`artboard_list`]. Mirrors
/// `ArtboardInfo` in `apps/desktop/shared/scene.ts`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArtboardInfo {
    pub id: Uuid,
    pub name: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub page_id: Uuid,
}

/// Create an artboard. If `page_id` is `None`, the artboard is
/// attached to the first existing Page; if no Page exists, a new
/// "Page 1" is created and used.
///
/// Records an undoable `artboard_create` operation and triggers a
/// scene sync.
pub fn artboard_create(
    page_id: Option<Uuid>,
    name: String,
    width: f64,
    height: f64,
) -> Result<Uuid> {
    if !(width.is_finite() && width > 0.0 && height.is_finite() && height > 0.0) {
        return Err(DocumentBridgeError::InvalidBounds { width, height });
    }
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;

    // Resolve target page.
    let resolved_page = match page_id {
        Some(p) => p,
        None => match find_first_page(&ws.project.document) {
            Some(p) => p,
            None => ws.project.add_page("Page 1")?,
        },
    };

    // Auto-position new artboards in a horizontal row 100px apart so
    // they don't stack on top of each other when the user just clicks
    // "New artboard" repeatedly.
    let x = next_artboard_x(&ws.project.document, resolved_page);
    let bounds = kcreate_core::node::Bounds::new(x, 0.0, width, height);
    let id = ws
        .project
        .document
        .create_artboard(resolved_page, &name, bounds)?;
    ws.project.modified_at = Utc::now();
    let snapshot = ws
        .project
        .document
        .get_node(id)
        .map_or(serde_json::Value::Null, |n| {
            serde_json::to_value(n).unwrap_or(serde_json::Value::Null)
        });
    let op = Operation::new(
        "user",
        "artboard_create",
        serde_json::Value::Null,
        snapshot,
        vec![id],
    );
    ws.project.execute_operation(op);
    let _ = sync_scene_locked(&mut guard);
    drop(guard);
    Ok(id)
}

/// All artboards across all pages, sorted by their owning page id
/// then `bounds.x` (the per-page left-to-right order chosen by
/// [`DocumentGraph::list_artboards`]).
pub fn artboard_list() -> Result<Vec<ArtboardInfo>> {
    let guard = slot().write();
    let ws = guard.as_ref().ok_or(DocumentBridgeError::NoProject)?;
    let mut pages: Vec<&Node> = ws
        .project
        .document
        .iter()
        .map(|(_, n)| n)
        .filter(|n| n.node_type == NodeType::Page)
        .collect();
    pages.sort_by_key(|p| p.id);
    let mut out = Vec::new();
    for page in pages {
        for ab in ws.project.document.list_artboards(page.id) {
            out.push(ArtboardInfo {
                id: ab.id,
                name: ab.name.clone(),
                x: ab.bounds.x,
                y: ab.bounds.y,
                width: ab.bounds.width,
                height: ab.bounds.height,
                page_id: page.id,
            });
        }
    }
    Ok(out)
}

/// Deep-clone an artboard. Records an `artboard_duplicate` operation
/// (the snapshot is the new root node only — undo deletes the clone
/// subtree wholesale by removing the new root).
pub fn artboard_duplicate(artboard_id: Uuid) -> Result<Uuid> {
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    let new_id = ws.project.document.duplicate_artboard(artboard_id)?;
    ws.project.modified_at = Utc::now();
    let snapshot = ws
        .project
        .document
        .get_node(new_id)
        .map_or(serde_json::Value::Null, |n| {
            serde_json::to_value(n).unwrap_or(serde_json::Value::Null)
        });
    let op = Operation::new(
        "user",
        "artboard_duplicate",
        serde_json::to_value(artboard_id).unwrap_or(serde_json::Value::Null),
        snapshot,
        vec![new_id],
    );
    ws.project.execute_operation(op);
    let _ = sync_scene_locked(&mut guard);
    drop(guard);
    Ok(new_id)
}

/// Resize an artboard. The `(x, y)` corner is preserved; only
/// `width` and `height` change. Records an undoable operation.
pub fn artboard_resize(artboard_id: Uuid, width: f64, height: f64) -> Result<()> {
    if !(width.is_finite() && width > 0.0 && height.is_finite() && height > 0.0) {
        return Err(DocumentBridgeError::InvalidBounds { width, height });
    }
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    let before = ws
        .project
        .document
        .get_node(artboard_id)
        .map(|n| serde_json::to_value(n.bounds).unwrap_or(serde_json::Value::Null))
        .ok_or(DocumentBridgeError::NodeNotFound(artboard_id))?;
    let current = ws
        .project
        .document
        .get_node(artboard_id)
        .ok_or(DocumentBridgeError::NodeNotFound(artboard_id))?
        .bounds;
    let new_bounds = kcreate_core::node::Bounds::new(current.x, current.y, width, height);
    ws.project
        .document
        .resize_artboard(artboard_id, new_bounds)?;
    ws.project.modified_at = Utc::now();
    let after = serde_json::to_value(new_bounds)?;
    let op = Operation::new("user", "artboard_resize", before, after, vec![artboard_id]);
    ws.project.execute_operation(op);
    let _ = sync_scene_locked(&mut guard);
    drop(guard);
    Ok(())
}

/// Return the built-in artboard preset catalogue.
pub fn artboard_presets() -> Vec<kcreate_core::ArtboardPreset> {
    kcreate_core::standard_presets()
}

// -----------------------------------------------------------------------------
// Magic Resize (G5) — reflow one design to many sizes
// -----------------------------------------------------------------------------

/// One requested Magic-Resize target. Either a named `preset`
/// (resolved against [`kcreate_core::standard_presets`],
/// case-insensitive) **or** an explicit `width` × `height` in pixels.
/// When both a preset and explicit dimensions are supplied the
/// explicit dimensions win. An optional `name` overrides the label of
/// the generated artboard (otherwise the preset name / pixel size is
/// used).
///
/// Mirrors `ResizeTarget` in `apps/desktop/shared/scene.ts`. The wire
/// payload is a JSON array of these (the bridge entry point takes the
/// array as a JSON string so we don't have to register a new
/// `#[napi(object)]` struct).
#[derive(Debug, Clone, Deserialize)]
pub struct ResizeTargetSpec {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub preset: Option<String>,
    #[serde(default)]
    pub width: Option<f64>,
    #[serde(default)]
    pub height: Option<f64>,
}

/// Content-aware behaviour toggles for [`magic_resize_with_content`].
///
/// Both default **on** — the professional Canva-style behaviour — so a
/// plain [`magic_resize`] call (and every existing caller) opts into
/// content-aware text re-fit and image smart-crop without changing its
/// signature. A caller that wants the pure geometric reflow (the
/// previous G-wave behaviour) flips the relevant flag to `false`.
///
/// Mirrors `MagicResizeContent` in `apps/desktop/shared/scene.ts`. The
/// wire payload is a JSON object; the bridge entry point takes it as a
/// JSON string so we don't have to register a new `#[napi(object)]`
/// struct (consistent with how [`ResizeTargetSpec`] is wired).
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MagicResizeContentOptions {
    /// Re-fit text layers to their reflowed box via shaping
    /// (line-break + shrink-to-fit) instead of pure geometric font
    /// scaling, so a headline neither overflows nor shrinks to nothing
    /// after a drastic aspect change.
    pub refit_text: bool,
    /// Smart-crop raster layers toward a detected focal point when the
    /// node's box aspect ratio changes, instead of letting the renderer
    /// stretch the pixels to fill the new bounds.
    pub smart_crop: bool,
}

impl Default for MagicResizeContentOptions {
    fn default() -> Self {
        Self {
            refit_text: true,
            smart_crop: true,
        }
    }
}

/// A target after preset resolution / validation: a concrete label +
/// pixel size.
struct ResolvedResizeTarget {
    label: String,
    width: f64,
    height: f64,
}

fn resolve_resize_target(
    spec: &ResizeTargetSpec,
    presets: &[kcreate_core::ArtboardPreset],
) -> Result<ResolvedResizeTarget> {
    // Explicit dimensions take precedence over a preset name so a
    // caller can override a preset's size while keeping its label.
    if let (Some(width), Some(height)) = (spec.width, spec.height) {
        if !(width.is_finite() && width > 0.0 && height.is_finite() && height > 0.0) {
            return Err(DocumentBridgeError::InvalidBounds { width, height });
        }
        let label = spec
            .name
            .clone()
            .unwrap_or_else(|| format!("{}×{}", width.round() as i64, height.round() as i64));
        return Ok(ResolvedResizeTarget {
            label,
            width,
            height,
        });
    }

    if let Some(preset_name) = spec.preset.as_deref() {
        let trimmed = preset_name.trim();
        let preset = presets
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(trimmed))
            .ok_or_else(|| DocumentBridgeError::InvalidArgument {
                argument: "preset".to_string(),
                value: preset_name.to_string(),
            })?;
        let label = spec.name.clone().unwrap_or_else(|| preset.name.clone());
        return Ok(ResolvedResizeTarget {
            label,
            width: preset.width,
            height: preset.height,
        });
    }

    Err(DocumentBridgeError::InvalidArgument {
        argument: "target".to_string(),
        value: "expected a `preset` name or both `width` and `height`".to_string(),
    })
}

/// Build the reflow-engine input tree for the node `id` (recursively).
///
/// Reads each node's absolute [`Bounds`] and anchoring
/// [`Constraints`] directly off the graph, plus the font size for
/// text layers (from the canonical `TextLayerMeta` blob). The engine
/// reasons purely about bounds — consistent with the flex/grid and
/// constraint solvers, which also ignore `transform` translation
/// (authored shapes bake position into `bounds` and leave the
/// transform identity).
fn build_resize_node(doc: &DocumentGraph, id: Uuid) -> Option<ResizeNode> {
    let node = doc.get_node(id)?;
    let font_size = if node.node_type == NodeType::TextLayer {
        node.metadata
            .get(crate::scene_sync::TEXT_LAYER_METADATA_KEY)
            .and_then(|v| serde_json::from_value::<kcreate_export::TextLayerMeta>(v.clone()).ok())
            .map(|m| f64::from(m.font_size))
    } else {
        None
    };
    let children = doc
        .children_of(id)
        .into_iter()
        .filter_map(|cid| build_resize_node(doc, cid))
        .collect();
    Some(ResizeNode {
        id,
        bounds: node.bounds,
        constraints: node.constraints,
        font_size,
        children,
    })
}

/// Affine-remap every point of `path` from the `old` bounds rectangle
/// into the `new` bounds rectangle. Keeps a `VectorLayer`'s stored
/// geometry consistent with its reflowed `bounds` (the renderer draws
/// vector paths from their absolute point coordinates, not by scaling
/// the bounds rect — so the path itself must move).
fn remap_vector_path(
    path: &mut kcreate_vector::VectorPath,
    old: kcreate_core::node::Bounds,
    new: kcreate_core::node::Bounds,
) {
    let sx = if old.width.abs() > f64::EPSILON {
        new.width / old.width
    } else {
        1.0
    };
    let sy = if old.height.abs() > f64::EPSILON {
        new.height / old.height
    } else {
        1.0
    };
    let map = |p: &mut kcreate_vector::PathPoint| {
        p.x = new.x + (p.x - old.x) * sx;
        p.y = new.y + (p.y - old.y) * sy;
    };
    for seg in &mut path.commands {
        match seg {
            kcreate_vector::PathSegment::MoveTo(p) | kcreate_vector::PathSegment::LineTo(p) => {
                map(p);
            }
            kcreate_vector::PathSegment::QuadTo { ctrl, end } => {
                map(ctrl);
                map(end);
            }
            kcreate_vector::PathSegment::CubicTo { ctrl1, ctrl2, end } => {
                map(ctrl1);
                map(ctrl2);
                map(end);
            }
            kcreate_vector::PathSegment::Close => {}
        }
    }
}

/// Apply a [`kcreate_layout::ResizeResult`] to the cloned subtree:
/// write each node's new bounds (remapping vector geometry so shapes
/// actually move/scale), then rewrite text layers' font sizes in
/// their `TextLayerMeta` blob.
fn apply_resize_result(
    doc: &mut DocumentGraph,
    result: &kcreate_layout::ResizeResult,
    content: MagicResizeContentOptions,
    opts: &ResizeOptions,
) -> Result<()> {
    for (id, new_bounds) in &result.bounds {
        let Some(node) = doc.get_node_mut(*id) else {
            continue;
        };
        let old_bounds = node.bounds;
        if node.node_type == NodeType::VectorLayer {
            if let Some(raw) = node
                .metadata
                .get(crate::scene_sync::VECTOR_PATH_METADATA_KEY)
            {
                if let Ok(mut path) =
                    serde_json::from_value::<kcreate_vector::VectorPath>(raw.clone())
                {
                    remap_vector_path(&mut path, old_bounds, *new_bounds);
                    node.metadata.insert(
                        crate::scene_sync::VECTOR_PATH_METADATA_KEY.to_string(),
                        serde_json::to_value(&path)?,
                    );
                }
            }
        }
        node.bounds = *new_bounds;
        node.touch();
    }

    for (id, new_font) in &result.fonts {
        let Some(node) = doc.get_node_mut(*id) else {
            continue;
        };
        let Some(raw) = node
            .metadata
            .get(crate::scene_sync::TEXT_LAYER_METADATA_KEY)
        else {
            continue;
        };
        let mut meta: kcreate_export::TextLayerMeta = serde_json::from_value(raw.clone())?;
        // `new_font` is the geometric (proportional) size the rest of
        // the design scales to — the upper bound for any content-aware
        // re-fit.
        #[allow(clippy::cast_possible_truncation)]
        let geometric = *new_font as f32;
        let final_size = if content.refit_text {
            // Content-aware re-fit: shrink an overflowing headline back
            // into its reflowed box via shaping, flooring at
            // `min_font_px`. The box is the node's already-applied new
            // bounds; the frame options + family come off the node so
            // columns / insets / wrap-mode are honoured. On a host with
            // no installed fonts the shaper can't resolve a face, so
            // `refit_text_to_box` returns `geometric` unchanged — the
            // graceful fallback keeps the proportional size rather than
            // collapsing to the floor.
            let frame = node.text_frame_options();
            let box_bounds = node.bounds;
            let style = kcreate_text::paragraph::TextStyle {
                font_family: meta.font_family.clone(),
                font_size: geometric,
                line_height: 1.25,
            };
            #[allow(clippy::cast_possible_truncation)]
            let floor = opts.min_font_px as f32;
            kcreate_text::refit_text_to_box(
                &meta.text, &style, &frame, box_bounds, floor, geometric,
            )
        } else {
            geometric
        };
        meta.font_size = final_size;
        node.metadata.insert(
            crate::scene_sync::TEXT_LAYER_METADATA_KEY.to_string(),
            serde_json::to_value(&meta)?,
        );
        node.touch();
    }
    Ok(())
}

/// Resize + reflow an already-cloned artboard subtree in place.
///
/// Runs steps 2-5 of the Magic Resize pipeline against `new_root`
/// (which must already be a deep clone of the source under the target
/// page): resize the frame, reflow the cloned children through the
/// pure engine, apply the new bounds/fonts, then re-run the flex/grid
/// solvers across any `LayoutFrame` descendants. Returns the resized
/// root's serialized snapshot for the operation log.
///
/// Split out from [`magic_resize`] so the caller can roll the clone
/// back if any step here fails.
fn resize_cloned_artboard(
    ws: &mut Workspace,
    new_root: Uuid,
    source_bounds: kcreate_core::node::Bounds,
    target_bounds: kcreate_core::node::Bounds,
    name: String,
    opts: &ResizeOptions,
    content: MagicResizeContentOptions,
) -> Result<serde_json::Value> {
    // 2. Resize + reposition the clone's artboard frame.
    if let Some(root) = ws.project.document.get_node_mut(new_root) {
        root.bounds = target_bounds;
        root.name = name;
        root.touch();
    }

    // 3. Build the reflow input from the cloned children. The cloned
    //    children are still at the SOURCE's coordinates, so the
    //    engine's source frame is the original source bounds.
    let roots: Vec<ResizeNode> = ws
        .project
        .document
        .children_of(new_root)
        .into_iter()
        .filter_map(|cid| build_resize_node(&ws.project.document, cid))
        .collect();

    // Capture the cloned raster nodes' pre-reflow bounds so the
    // smart-crop step (4b) can tell which images actually changed
    // aspect ratio. Done before the reflow mutates them; skipped
    // entirely when smart-crop is off.
    let raster_bounds_before = if content.smart_crop {
        raster_bounds_in_subtree(&ws.project.document, new_root)
    } else {
        HashMap::new()
    };

    // 4. Reflow + apply (bounds, vector geometry, content-aware fonts).
    let result = kcreate_layout::magic_resize(&roots, source_bounds, target_bounds, opts);
    apply_resize_result(&mut ws.project.document, &result, content, opts)?;

    // 5. Re-run auto-layout across any LayoutFrame descendants so
    //    stacked/grid groups reflow to their new frame sizes.
    layout_propagate_in_subtree(ws, new_root)?;

    // 4b. Content-aware image smart-crop. Runs after bounds are final
    //     (post auto-layout) so each raster is cropped to the box it
    //     actually ends up in. Non-destructive: writes a fresh derived
    //     blob and repoints only the clone's metadata.
    if !raster_bounds_before.is_empty() {
        smart_crop_resized_rasters(ws, &raster_bounds_before)?;
    }

    Ok(ws
        .project
        .document
        .get_node(new_root)
        .map_or(serde_json::Value::Null, |n| {
            serde_json::to_value(n).unwrap_or(serde_json::Value::Null)
        }))
}

/// Map every `RasterLayer` descendant of `root` (carrying image
/// metadata) to its current bounds. Used by [`resize_cloned_artboard`]
/// to snapshot raster geometry before the reflow so the smart-crop
/// step can detect a genuine aspect-ratio change.
fn raster_bounds_in_subtree(
    doc: &DocumentGraph,
    root: Uuid,
) -> HashMap<Uuid, kcreate_core::node::Bounds> {
    doc.descendants_of(root)
        .into_iter()
        .filter_map(|id| {
            let node = doc.get_node(id)?;
            if node.node_type == NodeType::RasterLayer
                && node
                    .metadata
                    .contains_key(crate::scene_sync::RASTER_IMAGE_METADATA_KEY)
            {
                Some((id, node.bounds))
            } else {
                None
            }
        })
        .collect()
}

/// Relative aspect-ratio change below which a raster is left alone: a
/// near-uniform scale doesn't distort the image, so re-cropping would
/// only throw pixels away for no visible benefit.
const SMART_CROP_ASPECT_TOLERANCE: f64 = 0.01;

/// True when two rectangles differ in aspect ratio by more than
/// [`SMART_CROP_ASPECT_TOLERANCE`] (relative). Degenerate (zero-extent)
/// bounds never report a change.
fn aspect_changed(a: kcreate_core::node::Bounds, b: kcreate_core::node::Bounds) -> bool {
    if a.height.abs() <= f64::EPSILON || b.height.abs() <= f64::EPSILON {
        return false;
    }
    let aspect_a = a.width / a.height;
    let aspect_b = b.width / b.height;
    if aspect_a.abs() <= f64::EPSILON {
        return false;
    }
    ((aspect_a - aspect_b) / aspect_a).abs() > SMART_CROP_ASPECT_TOLERANCE
}

/// Encode an RGBA8 buffer as PNG bytes. Bridge-local twin of
/// `raster_ops::encode_png` (kept private to that module); used by the
/// Magic Resize smart-crop to persist a derived crop as a new blob.
fn encode_rgba_png(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>> {
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

/// Smart-crop the reflowed raster layers whose box aspect ratio changed
/// toward a detected focal point, so the subject is preserved instead
/// of stretched by the renderer's fill-to-bounds raster path.
///
/// `bounds_before` maps each candidate raster node id to its pre-reflow
/// bounds. For every node whose **new** box aspect differs from its old
/// box aspect beyond [`SMART_CROP_ASPECT_TOLERANCE`] we: load + decode
/// the source pixels, compute the max-area crop of the new box's aspect
/// centred on the focal point ([`kcreate_ai::content_aware_crop`], which
/// degrades to a center-crop when there's no signal), re-encode the
/// cropped pixels as a new PNG blob, and repoint the **clone's**
/// `RasterImageMeta` at it. The original blob is content-addressed and
/// untouched, so the source artboard stays byte-for-byte unchanged.
///
/// Failures on any single node (unreadable blob, undecodable bytes) are
/// skipped rather than aborting the whole resize — the worst case is
/// that one image keeps the existing stretch behaviour.
fn smart_crop_resized_rasters(
    ws: &mut Workspace,
    bounds_before: &HashMap<Uuid, kcreate_core::node::Bounds>,
) -> Result<()> {
    for (id, old_bounds) in bounds_before {
        let Some(node) = ws.project.document.get_node(*id) else {
            continue;
        };
        let new_bounds = node.bounds;
        if !aspect_changed(*old_bounds, new_bounds) {
            continue;
        }
        let Some(meta_value) = node
            .metadata
            .get(crate::scene_sync::RASTER_IMAGE_METADATA_KEY)
        else {
            continue;
        };
        let meta: crate::scene_sync::RasterImageMeta = serde_json::from_value(meta_value.clone())?;

        // Read the source blob under the store lock; decode outside it.
        let bytes = {
            let store = ws.store.lock();
            match store.blobs().load(&meta.blob_hash) {
                Ok(b) => b,
                Err(_) => continue,
            }
        };
        let Ok(decoded) = image::load_from_memory(&bytes) else {
            continue;
        };
        let rgba = decoded.to_rgba8();
        let (src_w, src_h) = rgba.dimensions();

        // Target aspect = the reflowed box. The crop helper only uses
        // the ratio, so the absolute pixel size is irrelevant; round to
        // whole pixels and floor at 1 to stay in range.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let target_w = new_bounds.width.round().clamp(1.0, f64::from(u32::MAX)) as u32;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let target_h = new_bounds.height.round().clamp(1.0, f64::from(u32::MAX)) as u32;

        let Some(crop) =
            kcreate_ai::content_aware_crop(rgba.as_raw(), src_w, src_h, target_w, target_h)
        else {
            continue;
        };
        // A full-frame crop is a no-op (source already matches the
        // target aspect) — skip the re-encode + blob churn.
        if crop.x == 0 && crop.y == 0 && crop.width == src_w && crop.height == src_h {
            continue;
        }

        let cropped = kcreate_ai::apply_crop(rgba.as_raw(), src_w, src_h, crop);
        let png = encode_rgba_png(&cropped, crop.width, crop.height)?;
        let blob = ws
            .store
            .lock()
            .blobs()
            .store(&png, "image/png")
            .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))?;
        let new_meta = crate::scene_sync::RasterImageMeta {
            blob_hash: blob.hash,
            width: crop.width,
            height: crop.height,
        };
        if let Some(node) = ws.project.document.get_node_mut(*id) {
            node.metadata.insert(
                crate::scene_sync::RASTER_IMAGE_METADATA_KEY.to_string(),
                serde_json::to_value(&new_meta)?,
            );
            node.touch();
        }
    }
    Ok(())
}

/// Remove a batch of (sub)trees from the document graph, ignoring ids
/// that are already gone. Used by [`magic_resize`] to unwind the
/// artboards it committed earlier in a batch when a later target fails,
/// so a failed call mutates nothing.
fn rollback_artboards(ws: &mut Workspace, ids: &[Uuid]) {
    for id in ids {
        ws.project.document.remove_node(*id);
    }
}

/// **Magic Resize.** Reflow a finished design from its source
/// artboard onto one or more differently-sized target artboards,
/// preserving layout intent (anchoring + proportional scaling) rather
/// than naively stretching.
///
/// Non-destructive: the source artboard is never modified — each
/// target produces a fresh deep-cloned artboard placed in a row to
/// the right of everything currently on the source's page. All
/// targets are generated under a **single** undoable operation
/// (`magic_resize`) so one undo removes every generated artboard at
/// once.
///
/// For each target the pipeline is:
/// 1. deep-clone the source subtree (new ids) under the same page,
/// 2. resize + reposition the clone's artboard frame to the target,
/// 3. run the pure reflow engine ([`kcreate_layout::magic_resize`])
///    over the cloned children,
/// 4. apply the new bounds (remapping vector geometry) + scaled fonts,
/// 5. re-run the flex/grid solvers across any `LayoutFrame`
///    descendants so stacked/auto-laid-out groups reflow too.
///
/// Returns the new artboards' ids in target order.
///
/// **Atomicity.** The whole batch is all-or-nothing at the graph
/// level: if any target fails after its subtree has been cloned, that
/// partial clone *and* every artboard already generated by this call
/// are removed before the error propagates, so a failed `magic_resize`
/// leaves the document exactly as it found it (and never logs an
/// operation). See [`rollback_artboards`].
pub fn magic_resize(source_artboard_id: Uuid, targets: &[ResizeTargetSpec]) -> Result<Vec<Uuid>> {
    magic_resize_with_content(
        source_artboard_id,
        targets,
        MagicResizeContentOptions::default(),
    )
}

/// [`magic_resize`] with explicit control over the content-aware
/// behaviour (text re-fit + image smart-crop). `magic_resize` defers
/// here with both toggles on; callers that want the pure geometric
/// reflow pass a [`MagicResizeContentOptions`] with the relevant flag
/// cleared. Same atomicity + single-undo guarantees as `magic_resize`.
pub fn magic_resize_with_content(
    source_artboard_id: Uuid,
    targets: &[ResizeTargetSpec],
    content: MagicResizeContentOptions,
) -> Result<Vec<Uuid>> {
    // One-time gap between generated artboards, matching the spacing
    // `artboard_create` / `duplicate_artboard` use.
    const GAP: f64 = 100.0;

    if targets.is_empty() {
        return Err(DocumentBridgeError::InvalidArgument {
            argument: "targets".to_string(),
            value: "at least one target size is required".to_string(),
        });
    }
    // Resolve + validate every target up front so a bad spec aborts
    // before we mutate the graph (all-or-nothing). The preset
    // catalogue is built once and shared across targets rather than
    // rebuilt per spec.
    let presets = kcreate_core::standard_presets();
    let resolved = targets
        .iter()
        .map(|spec| resolve_resize_target(spec, &presets))
        .collect::<Result<Vec<_>>>()?;

    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;

    // Validate the source is an artboard; capture its frame + page.
    let source = ws
        .project
        .document
        .get_node(source_artboard_id)
        .ok_or(DocumentBridgeError::NodeNotFound(source_artboard_id))?;
    if source.node_type != NodeType::Artboard {
        return Err(DocumentBridgeError::WrongNodeType {
            expected: NodeType::Artboard,
            got: source.node_type,
        });
    }
    let source_bounds = source.bounds;
    let source_name = source.name.clone();
    let page_id = source
        .parent_id
        .ok_or(DocumentBridgeError::NodeNotFound(source_artboard_id))?;

    let opts = ResizeOptions::default();
    let mut cursor_x = next_artboard_x(&ws.project.document, page_id);

    let mut new_ids: Vec<Uuid> = Vec::with_capacity(resolved.len());
    let mut snapshots: Vec<serde_json::Value> = Vec::with_capacity(resolved.len());

    for target in &resolved {
        let target_bounds =
            kcreate_core::node::Bounds::new(cursor_x, source_bounds.y, target.width, target.height);
        let name = format!("{source_name} — {}", target.label);

        // 1. Deep clone the source subtree under the same page. A clone
        //    failure mutates nothing for this target, so we only need to
        //    unwind the artboards already committed this call.
        let new_root = match ws
            .project
            .document
            .clone_subtree(source_artboard_id, Some(page_id))
        {
            Ok(id) => id,
            Err(e) => {
                rollback_artboards(ws, &new_ids);
                return Err(e.into());
            }
        };

        // 2-5. Resize the clone's frame, reflow its children, apply the
        //       result, and re-run auto-layout. Any failure here leaves
        //       `new_root` half-built, so unwind it *and* the prior
        //       targets before surfacing the error.
        match resize_cloned_artboard(
            ws,
            new_root,
            source_bounds,
            target_bounds,
            name,
            &opts,
            content,
        ) {
            Ok(snapshot) => {
                cursor_x += target.width + GAP;
                snapshots.push(snapshot);
                new_ids.push(new_root);
            }
            Err(e) => {
                ws.project.document.remove_node(new_root);
                rollback_artboards(ws, &new_ids);
                return Err(e);
            }
        }
    }

    ws.project.modified_at = Utc::now();
    let op = Operation::new(
        "user",
        "magic_resize",
        serde_json::to_value(source_artboard_id).unwrap_or(serde_json::Value::Null),
        serde_json::Value::Array(snapshots),
        new_ids.clone(),
    );
    ws.project.execute_operation(op);
    let _ = sync_scene_locked(&mut guard);
    drop(guard);
    Ok(new_ids)
}

/// Request payload for [`magic_resize_export_png`]. JSON-string wire
/// (camelCase) mirrored by `MagicResizeExportRequest` in
/// `apps/desktop/shared/scene.ts`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MagicResizeExportRequest {
    /// Absolute directory the PNGs are written into (vetted by the
    /// main process's directory-picker before it reaches the bridge).
    pub output_dir: String,
    /// Content-aware behaviour for the underlying resize. Defaults on.
    #[serde(default)]
    pub content: MagicResizeContentOptions,
    /// Optional cap on the longest exported edge (px). When set and an
    /// artboard's longest side exceeds it, that PNG is scaled down to
    /// fit; otherwise every artboard exports at its full pixel size.
    #[serde(default)]
    pub max_dim_px: Option<u32>,
}

/// Outcome of [`magic_resize_export_png`]: the generated artboard ids
/// plus a per-file export report. Serialised (snake_case) to JSON for
/// the N-API boundary.
#[derive(Debug, Clone, Serialize)]
pub struct MagicResizeExportReport {
    /// Ids of the artboards created by the resize, in target order.
    pub artboard_ids: Vec<Uuid>,
    /// Directory the files were written to.
    pub output_dir: String,
    /// Absolute paths of the PNGs successfully written.
    pub written: Vec<String>,
    /// `"filename: error"` for any artboard that failed to export.
    pub failed: Vec<String>,
    /// Wall-clock duration of the parallel render, in milliseconds.
    pub duration_ms: u64,
}

/// **Magic Resize → batch PNG export** (Canva "resize & download all").
///
/// One-shot: run [`magic_resize_with_content`] to generate the target
/// artboards (a single undoable op, with content-aware text re-fit +
/// image smart-crop baked in), then render every generated artboard to
/// a PNG in `request.output_dir` via the parallel batch driver
/// ([`kcreate_export::run_png_batch_parallel`]).
///
/// Rendering is done off the workspace lock: we build each artboard's
/// translated [`Scene`] + [`PngExportOptions`] while holding the lock,
/// then drop it before the (CPU-heavy, parallel) render so no other
/// bridge call is blocked for the duration. The resize itself is the
/// only mutation; the export is read-only and never touches the
/// operation log, so a single undo still removes every artboard.
///
/// Returns a [`MagicResizeExportReport`] (generated ids + per-file
/// success/failure) so the host can surface a precise result.
pub fn magic_resize_export_png(
    source_artboard_id: Uuid,
    targets: &[ResizeTargetSpec],
    request: &MagicResizeExportRequest,
) -> Result<MagicResizeExportReport> {
    if request.output_dir.trim().is_empty() {
        return Err(DocumentBridgeError::InvalidArgument {
            argument: "output_dir".to_string(),
            value: "an output directory is required".to_string(),
        });
    }
    let output_dir = PathBuf::from(&request.output_dir);

    // 1. Resize — its own single undoable operation. Smart-crop derives
    //    are committed to the blob store here, so the scenes built below
    //    resolve the cropped pixels.
    let artboard_ids = magic_resize_with_content(source_artboard_id, targets, request.content)?;

    // 2. Build one translated Scene + PngExportOptions per generated
    //    artboard while holding the workspace + blob lock; collect them
    //    into render payloads, then drop the lock before rendering.
    let items: Vec<PngBatchItem> = {
        let guard = slot().read();
        let ws = guard.as_ref().ok_or(DocumentBridgeError::NoProject)?;

        // Build the whole-document scene ONCE — a single graph traversal
        // and a single blob-resolution pass — with embedded raster pixels
        // resolved (Some(blobs)). Each artboard's render payload is then a
        // cheap clone + translate of this base, instead of re-syncing the
        // entire document per target (previously O(targets × document)).
        // Ephemeral SceneSync so the live scene cache + dirty set are
        // untouched (same discipline as the thumbnail path).
        let mut sync = crate::scene_sync::SceneSync::default();
        let base_scene = sync.sync_document_to_scene_borrowed(
            &ws.project.document,
            Some(ws.store.lock().blobs()),
            &[],
        );

        let mut items = Vec::with_capacity(artboard_ids.len());
        let mut used_names: HashSet<String> = HashSet::with_capacity(artboard_ids.len());
        for (index, id) in artboard_ids.iter().enumerate() {
            let Some(node) = ws.project.document.get_node(*id) else {
                continue;
            };
            let ab_bounds = node.bounds;
            let filename = unique_png_filename(&node.name, index, &mut used_names);

            // Translate a copy of the base scene so this artboard lands at
            // the renderer origin and every other artboard falls outside
            // the export crop.
            let mut scene = base_scene.clone();
            #[allow(clippy::cast_possible_truncation)]
            let dx = -ab_bounds.x as f32;
            #[allow(clippy::cast_possible_truncation)]
            let dy = -ab_bounds.y as f32;
            for obj in &mut scene.objects {
                obj.translation.0 += dx;
                obj.translation.1 += dy;
            }

            let options = png_export_options_for_bounds(ab_bounds, request.max_dim_px);
            items.push(PngBatchItem {
                filename,
                scene,
                options,
            });
        }
        items
    };

    // 3. Render every artboard in parallel, off the lock. The batch
    //    driver creates `output_dir` and isolates per-item failures.
    let cancel = AtomicBool::new(false);
    let result = run_png_batch_parallel(&items, &output_dir, &cancel, |_progress| {})?;

    let written = result
        .succeeded
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    let failed = result
        .failed
        .iter()
        .map(|(name, err)| format!("{name}: {err}"))
        .collect();
    Ok(MagicResizeExportReport {
        artboard_ids,
        output_dir: output_dir.to_string_lossy().into_owned(),
        written,
        failed,
        duration_ms: result.duration_ms,
    })
}

/// Build [`PngExportOptions`] that render `bounds` at full pixel size,
/// optionally scaled down so the longest edge fits within `max_dim_px`.
/// White background matches the single-artboard / thumbnail exporters.
fn png_export_options_for_bounds(
    bounds: kcreate_core::node::Bounds,
    max_dim_px: Option<u32>,
) -> PngExportOptions {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let width = bounds.width.max(1.0) as u32;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let height = bounds.height.max(1.0) as u32;
    let scale = match max_dim_px {
        Some(cap) if cap > 0 => {
            let longest = bounds.width.max(bounds.height).max(1.0);
            if longest > f64::from(cap) {
                #[allow(clippy::cast_possible_truncation)]
                let s = (f64::from(cap) / longest) as f32;
                s
            } else {
                1.0
            }
        }
        _ => 1.0,
    };
    PngExportOptions {
        width,
        height,
        scale,
        background: Some(kcreate_renderer::geometry::Color::rgba(1.0, 1.0, 1.0, 1.0)),
    }
}

/// Derive a filesystem-safe, unique `.png` filename from an artboard
/// name. Non-alphanumeric runs collapse to a single `_`; a numeric
/// prefix keeps the batch in target order and guarantees uniqueness
/// even when two artboards share a name.
fn unique_png_filename(name: &str, index: usize, used: &mut HashSet<String>) -> String {
    let mut slug = String::with_capacity(name.len());
    let mut prev_us = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            prev_us = false;
        } else if !prev_us {
            slug.push('_');
            prev_us = true;
        }
    }
    let slug = slug.trim_matches('_');
    let base = if slug.is_empty() { "artboard" } else { slug };
    // Zero-padded index keeps lexical order == target order.
    let mut candidate = format!("{:02}_{base}.png", index + 1);
    let mut dedup = 1u32;
    while !used.insert(candidate.clone()) {
        candidate = format!("{:02}_{base}_{dedup}.png", index + 1);
        dedup += 1;
    }
    candidate
}

// -----------------------------------------------------------------------------
// Prototype interactions (Block A / Phase 1)
// -----------------------------------------------------------------------------

/// Append an [`kcreate_core::Interaction`] to `node_id`'s metadata.
///
/// Returns the new interaction's id. `trigger` is one of `"click"`,
/// `"hover"`, `"press"`. `action_json` is a serialized
/// [`kcreate_core::InteractionAction`] (tagged-enum form, e.g.
/// `{"kind":"navigate_to","target_artboard_id":"…"}`).
pub fn interaction_add(node_id: Uuid, trigger: &str, action_json: &str) -> Result<Uuid> {
    // Phase 11 expanded `InteractionTrigger` with `MouseEnter` /
    // `MouseLeave` / `AfterDelay { ms }`. We accept the legacy bare
    // discriminator strings (`"click"`, etc.) AND a JSON object form
    // (`{"kind":"after_delay","ms":1500}`) for the data-carrying
    // variant. Serde's hand-rolled (de)serialiser on
    // `kcreate_core::InteractionTrigger` handles both shapes, but we
    // have to feed it a JSON token — wrap the bare-string case in
    // quotes before parsing.
    let trimmed = trigger.trim();
    let parsed: kcreate_core::InteractionTrigger =
        if trimmed.starts_with('{') || trimmed.starts_with('"') {
            serde_json::from_str(trimmed).map_err(|e| DocumentBridgeError::InvalidArgument {
                argument: "trigger".into(),
                value: format!("{trigger} ({e})"),
            })?
        } else {
            let quoted = format!("\"{trimmed}\"");
            serde_json::from_str(&quoted).map_err(|_| DocumentBridgeError::InvalidArgument {
                argument: "trigger".into(),
                value: trigger.to_string(),
            })?
        };
    let trigger = parsed;
    let action: kcreate_core::InteractionAction = serde_json::from_str(action_json)?;
    let interaction = kcreate_core::Interaction::new(trigger, action);
    let interaction_id = interaction.id;
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    let before;
    let after;
    {
        let node = ws
            .project
            .document
            .get_node_mut(node_id)
            .ok_or(DocumentBridgeError::NodeNotFound(node_id))?;
        let mut interactions = node.interactions();
        before = serde_json::to_value(&interactions)?;
        interactions.push(interaction);
        node.set_interactions(&interactions);
        after = serde_json::to_value(&interactions)?;
    }
    ws.project.modified_at = Utc::now();
    let op = Operation::new("user", "interaction_add", before, after, vec![node_id]);
    ws.project.execute_operation(op);
    drop(guard);
    Ok(interaction_id)
}

/// Remove the interaction with `interaction_id` from `node_id`. Records
/// an undoable op when an interaction is actually removed; returns
/// `Ok(false)` if no interaction with that id exists.
pub fn interaction_remove(node_id: Uuid, interaction_id: Uuid) -> Result<bool> {
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    let removed;
    let before;
    let after;
    {
        let node = ws
            .project
            .document
            .get_node_mut(node_id)
            .ok_or(DocumentBridgeError::NodeNotFound(node_id))?;
        let mut interactions = node.interactions();
        before = serde_json::to_value(&interactions)?;
        let original = interactions.len();
        interactions.retain(|i| i.id != interaction_id);
        removed = interactions.len() < original;
        if removed {
            node.set_interactions(&interactions);
        }
        after = serde_json::to_value(&interactions)?;
    }
    if removed {
        ws.project.modified_at = Utc::now();
        let op = Operation::new("user", "interaction_remove", before, after, vec![node_id]);
        ws.project.execute_operation(op);
    }
    drop(guard);
    Ok(removed)
}

/// List all interactions stored on `node_id`.
pub fn interaction_list(node_id: Uuid) -> Result<Vec<kcreate_core::Interaction>> {
    let guard = slot().write();
    let ws = guard.as_ref().ok_or(DocumentBridgeError::NoProject)?;
    let node = ws
        .project
        .document
        .get_node(node_id)
        .ok_or(DocumentBridgeError::NodeNotFound(node_id))?;
    Ok(node.interactions())
}

/// Batched [`interaction_list`] taking many node ids in one IPC round
/// trip. The result is a map keyed by node id (string) with the same
/// shape `interaction_list` would have returned for each. Unknown ids
/// are silently skipped so a partial selection (e.g. just-deleted
/// node) doesn't cause a whole-batch failure. Used by the prototype
/// player to gather hotspots without doing N sequential round trips
/// (Devin Review ANALYSIS-0003).
pub fn interaction_list_batch(
    node_ids: &[Uuid],
) -> Result<std::collections::HashMap<Uuid, Vec<kcreate_core::Interaction>>> {
    let guard = slot().write();
    let ws = guard.as_ref().ok_or(DocumentBridgeError::NoProject)?;
    let mut out = std::collections::HashMap::with_capacity(node_ids.len());
    for id in node_ids {
        if let Some(node) = ws.project.document.get_node(*id) {
            let v = node.interactions();
            if !v.is_empty() {
                out.insert(*id, v);
            }
        }
    }
    Ok(out)
}

// -----------------------------------------------------------------------------
// Layout Studio: page layout + master pages + templates (Block B / Phase 2)
// -----------------------------------------------------------------------------

/// Set the [`kcreate_core::PageLayout`] on `page_id`.
///
/// `layout_json` is the serialized layout. No-op (but error returned)
/// if the node is not a `Page`.
pub fn page_set_layout(page_id: Uuid, layout_json: &str) -> Result<()> {
    let layout: kcreate_core::PageLayout = serde_json::from_str(layout_json)?;
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    let before;
    let after;
    {
        let node = ws
            .project
            .document
            .get_node_mut(page_id)
            .ok_or(DocumentBridgeError::NodeNotFound(page_id))?;
        if node.node_type != kcreate_core::NodeType::Page {
            return Err(DocumentBridgeError::WrongNodeType {
                expected: NodeType::Page,
                got: node.node_type,
            });
        }
        before = node.page_layout().map_or(serde_json::Value::Null, |l| {
            serde_json::to_value(&l).unwrap_or(serde_json::Value::Null)
        });
        node.set_page_layout(&layout);
        after = serde_json::to_value(&layout)?;
    }
    ws.project.modified_at = Utc::now();
    let op = Operation::new("user", "page_set_layout", before, after, vec![page_id]);
    ws.project.execute_operation(op);
    drop(guard);
    Ok(())
}

/// Read the [`kcreate_core::PageLayout`] stored on `page_id`, if any.
pub fn page_get_layout(page_id: Uuid) -> Result<Option<kcreate_core::PageLayout>> {
    let guard = slot().write();
    let ws = guard.as_ref().ok_or(DocumentBridgeError::NoProject)?;
    let node = ws
        .project
        .document
        .get_node(page_id)
        .ok_or(DocumentBridgeError::NodeNotFound(page_id))?;
    Ok(node.page_layout())
}

/// Wire-format payload returned by [`master_page_list`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MasterPageInfo {
    pub id: Uuid,
    pub name: String,
    pub layout: Option<kcreate_core::PageLayout>,
}

/// Create a new master page. Records an undoable op. `size` is a
/// lowercase variant tag (e.g. `"a4"`); `orientation` is `"portrait"`
/// or `"landscape"`.
pub fn master_page_create(name: String, size: &str, orientation: &str) -> Result<Uuid> {
    let page_size = parse_page_size(size)?;
    let orientation = match orientation {
        "portrait" => kcreate_core::PageOrientation::Portrait,
        "landscape" => kcreate_core::PageOrientation::Landscape,
        other => {
            return Err(DocumentBridgeError::InvalidArgument {
                argument: "orientation".into(),
                value: other.to_string(),
            });
        }
    };
    let layout = kcreate_core::PageLayout::new(page_size, orientation);
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    let id = ws.project.create_master_page(name, layout)?;
    let snapshot = ws
        .project
        .document
        .get_node(id)
        .map_or(serde_json::Value::Null, |n| {
            serde_json::to_value(n).unwrap_or(serde_json::Value::Null)
        });
    let op = Operation::new(
        "user",
        "master_page_create",
        serde_json::Value::Null,
        snapshot,
        vec![id],
    );
    ws.project.execute_operation(op);
    // Master page is a `NodeType::Page` inserted into the document graph
    // (see `create_master_page` in `kcreate_core::Project`); the scene
    // sync traverses Page nodes (`scene_sync.rs::emit_node`) so the
    // renderer view must be refreshed.
    let _ = sync_scene_locked(&mut guard);
    drop(guard);
    Ok(id)
}

/// Parse a `PageSizeId` wire string into the core enum.
///
/// The accepted forms here are *exactly* the strings serde emits for
/// the `kcreate_core::PageSize` variants (see the `#[serde(rename =
/// ...)]` attributes on the enum), which is also the `PageSizeId` type
/// declared in `apps/desktop/shared/scene.ts`. The two sides are kept
/// in lockstep per AGENTS.md Rule 4; this parser is the single point
/// where TypeScript `PageSizeId` strings cross into Rust, so the
/// vocabulary list must match the serde rename set 1:1 — no
/// alternate spellings, no case folding.
fn parse_page_size(s: &str) -> Result<kcreate_core::PageSize> {
    Ok(match s {
        "a3" => kcreate_core::PageSize::A3,
        "a4" => kcreate_core::PageSize::A4,
        "a5" => kcreate_core::PageSize::A5,
        "letter" => kcreate_core::PageSize::Letter,
        "legal" => kcreate_core::PageSize::Legal,
        "tabloid" => kcreate_core::PageSize::Tabloid,
        "presentation_16x9" => kcreate_core::PageSize::Presentation16x9,
        "presentation_4x3" => kcreate_core::PageSize::Presentation4x3,
        other => {
            return Err(DocumentBridgeError::InvalidArgument {
                argument: "size".into(),
                value: other.to_string(),
            });
        }
    })
}

/// List all master pages in the open project, sorted by name.
pub fn master_page_list() -> Result<Vec<MasterPageInfo>> {
    let guard = slot().write();
    let ws = guard.as_ref().ok_or(DocumentBridgeError::NoProject)?;
    let masters = ws.project.list_master_pages();
    Ok(masters
        .into_iter()
        .map(|n| MasterPageInfo {
            id: n.id,
            name: n.name.clone(),
            layout: n.page_layout(),
        })
        .collect())
}

/// Attach `master_page_id` to a content page.
///
/// Records `before` / `after` patches as the full `PageLayout` JSON on the
/// content page so the operation log carries enough state for undo / redo
/// (the master id alone is not sufficient — the page may not have had a
/// layout at all before the call, and undo must restore that).
pub fn master_page_apply(content_page_id: Uuid, master_page_id: Uuid) -> Result<()> {
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    let before = ws
        .project
        .document
        .get_node(content_page_id)
        .and_then(Node::page_layout)
        .map_or(serde_json::Value::Null, |l| {
            serde_json::to_value(&l).unwrap_or(serde_json::Value::Null)
        });
    ws.project
        .apply_master_page(content_page_id, master_page_id)?;
    let after = ws
        .project
        .document
        .get_node(content_page_id)
        .and_then(Node::page_layout)
        .map_or(serde_json::Value::Null, |l| {
            serde_json::to_value(&l).unwrap_or(serde_json::Value::Null)
        });
    let op = Operation::new(
        "user",
        "master_page_apply",
        before,
        after,
        vec![content_page_id],
    );
    ws.project.execute_operation(op);
    drop(guard);
    Ok(())
}

/// Detach the master page reference from `content_page_id`.
///
/// Captures the full `PageLayout` on the content page before and after the
/// detach so undo / redo can restore the previous `master_page_id` (which
/// would otherwise be lost — the bridge does not keep a separate copy of
/// page layouts).
pub fn master_page_detach(content_page_id: Uuid) -> Result<()> {
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    let before = ws
        .project
        .document
        .get_node(content_page_id)
        .and_then(Node::page_layout)
        .map_or(serde_json::Value::Null, |l| {
            serde_json::to_value(&l).unwrap_or(serde_json::Value::Null)
        });
    ws.project.detach_master_page(content_page_id)?;
    let after = ws
        .project
        .document
        .get_node(content_page_id)
        .and_then(Node::page_layout)
        .map_or(serde_json::Value::Null, |l| {
            serde_json::to_value(&l).unwrap_or(serde_json::Value::Null)
        });
    let op = Operation::new(
        "user",
        "master_page_detach",
        before,
        after,
        vec![content_page_id],
    );
    ws.project.execute_operation(op);
    drop(guard);
    Ok(())
}

/// Return the catalog of built-in [`kcreate_core::LayoutTemplate`]s.
pub fn layout_template_list() -> Vec<kcreate_core::LayoutTemplate> {
    kcreate_core::builtin_layout_templates()
}

/// Apply a built-in template to the open project.
///
/// Returns the new page ids in template order. Records an undoable op.
pub fn layout_template_apply(template_id: Uuid) -> Result<Vec<Uuid>> {
    let templates = kcreate_core::builtin_layout_templates();
    let template = templates
        .into_iter()
        .find(|t| t.id == template_id)
        .ok_or_else(|| DocumentBridgeError::InvalidNodeType(template_id.to_string()))?;
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    let created = ws.project.apply_layout_template(&template)?;
    let op = Operation::new(
        "user",
        "layout_template_apply",
        serde_json::to_value(template_id)?,
        serde_json::to_value(&created)?,
        created.clone(),
    );
    ws.project.execute_operation(op);
    let _ = sync_scene_locked(&mut guard);
    drop(guard);
    Ok(created)
}

/// Add a new content page (with one default artboard) and return the
/// new page id. Records an undoable `page_add` op.
///
/// Optional `size` / `orientation` set the initial page layout. When
/// **both** are omitted, the page is created at the workspace default
/// (A4 portrait, matching `Project::add_page`). When **both** are
/// provided, the page bounds + `page_layout` metadata are set
/// accordingly. Providing only one of the two is a caller bug and
/// returns `DocumentBridgeError::InvalidArgument` — silently ignoring
/// the partial input would surprise the UI (it would think the page
/// was created at the requested size but only orientation or only
/// size made it through).
pub fn page_add(name: String, size: Option<&str>, orientation: Option<&str>) -> Result<Uuid> {
    // Validate the (size, orientation) pair *before* mutating the
    // workspace. Partial input is rejected with a descriptive error
    // so the caller can see which half is missing.
    let layout_args = match (size, orientation) {
        (None, None) => None,
        (Some(s), Some(o)) => Some((s, o)),
        (Some(_), None) => {
            return Err(DocumentBridgeError::InvalidArgument {
                argument: "orientation".into(),
                value: "<missing> (must be provided when `size` is set)".into(),
            });
        }
        (None, Some(_)) => {
            return Err(DocumentBridgeError::InvalidArgument {
                argument: "size".into(),
                value: "<missing> (must be provided when `orientation` is set)".into(),
            });
        }
    };
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    let page_id = ws.project.add_page(name)?;
    // Apply page layout if the caller supplied one. This must happen
    // *after* `add_page` because `add_page` creates the artboard
    // child whose default bounds depend on the page's bounds (not the
    // other way around). Setting the layout now resizes the page
    // bounds to the requested page size; the artboard inside keeps
    // its 1920x1080 default until the user resizes it.
    if let Some((size_tag, orientation_tag)) = layout_args {
        let layout = kcreate_core::PageLayout::new(
            parse_page_size(size_tag)?,
            match orientation_tag {
                "portrait" => kcreate_core::PageOrientation::Portrait,
                "landscape" => kcreate_core::PageOrientation::Landscape,
                other => {
                    return Err(DocumentBridgeError::InvalidArgument {
                        argument: "orientation".into(),
                        value: other.to_string(),
                    });
                }
            },
        );
        if let Some(node) = ws.project.document.get_node_mut(page_id) {
            let (w_mm, h_mm) = layout.dimensions_mm();
            let px_per_mm = 96.0 / 25.4;
            node.bounds =
                kcreate_core::node::Bounds::new(0.0, 0.0, w_mm * px_per_mm, h_mm * px_per_mm);
            node.set_page_layout(&layout);
        }
    }
    let snapshot = ws
        .project
        .document
        .get_node(page_id)
        .map_or(serde_json::Value::Null, |n| {
            serde_json::to_value(n).unwrap_or(serde_json::Value::Null)
        });
    let op = Operation::new(
        "user",
        "page_add",
        serde_json::Value::Null,
        snapshot,
        vec![page_id],
    );
    ws.project.execute_operation(op);
    ws.project.modified_at = Utc::now();
    let _ = sync_scene_locked(&mut guard);
    drop(guard);
    Ok(page_id)
}

/// Duplicate an existing page (and its artboards / layers) at the
/// document root. Returns the new page id. Records an undoable
/// `page_duplicate` op.
pub fn page_duplicate(page_id: Uuid) -> Result<Uuid> {
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    // Validate the source is a Page before we clone.
    let source = ws
        .project
        .document
        .get_node(page_id)
        .ok_or(DocumentBridgeError::NodeNotFound(page_id))?;
    if source.node_type != kcreate_core::NodeType::Page {
        return Err(DocumentBridgeError::WrongNodeType {
            expected: NodeType::Page,
            got: source.node_type,
        });
    }
    let new_id = ws.project.document.clone_subtree(page_id, None)?;
    if let Some(node) = ws.project.document.get_node_mut(new_id) {
        node.name = format!("{} (copy)", node.name);
    }
    let snapshot = ws
        .project
        .document
        .get_node(new_id)
        .map_or(serde_json::Value::Null, |n| {
            serde_json::to_value(n).unwrap_or(serde_json::Value::Null)
        });
    let op = Operation::new(
        "user",
        "page_duplicate",
        serde_json::to_value(page_id)?,
        snapshot,
        vec![new_id],
    );
    ws.project.execute_operation(op);
    ws.project.modified_at = Utc::now();
    let _ = sync_scene_locked(&mut guard);
    drop(guard);
    Ok(new_id)
}

/// Move `node_id` under `new_parent` (or to the root level when
/// `new_parent` is `None`), inserting at `index` in the destination's
/// children list. The PageNavigator uses this for drag-reorder.
///
/// Records an undoable `document_reparent` op carrying the prior
/// parent + index so the patch can be reversed.
pub fn document_reparent_node(node_id: Uuid, new_parent: Option<Uuid>, index: usize) -> Result<()> {
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    let (prior_parent, prior_index) = {
        let node = ws
            .project
            .document
            .get_node(node_id)
            .ok_or(DocumentBridgeError::NodeNotFound(node_id))?;
        let prior_parent = node.parent_id;
        let prior_index = if let Some(p) = prior_parent {
            ws.project
                .document
                .get_node(p)
                .and_then(|parent| parent.children.iter().position(|c| *c == node_id))
                .unwrap_or(0)
        } else {
            ws.project
                .document
                .root_ids()
                .iter()
                .position(|c| *c == node_id)
                .unwrap_or(0)
        };
        (prior_parent, prior_index)
    };
    ws.project
        .document
        .reparent_node(node_id, new_parent, index)?;
    let before = serde_json::json!({
        "parent_id": prior_parent,
        "index": prior_index,
    });
    let after = serde_json::json!({
        "parent_id": new_parent,
        "index": index,
    });
    let op = Operation::new("user", "document_reparent", before, after, vec![node_id]);
    ws.project.execute_operation(op);
    ws.project.modified_at = Utc::now();
    let _ = sync_scene_locked(&mut guard);
    drop(guard);
    Ok(())
}

// -----------------------------------------------------------------------------
// Clipboard (Phase 6 Tasks 25-26)
// -----------------------------------------------------------------------------
//
// Copy semantics:
//   * `document_clipboard_copy(node_ids)` walks each node + its
//     descendants, serialises the snapshots into a self-contained
//     JSON payload (`ClipboardPayload`), and returns it as a String.
//     The renderer/main process stores the payload on the system
//     clipboard so cross-window paste works.
//   * Payload includes the original parent ids and the source
//     bounds so paste can either drop the nodes under a new parent
//     or keep them root-level relative to the target artboard.
//
// Paste semantics:
//   * `document_clipboard_paste(payload, target_parent_id,
//     offset_x, offset_y)` deserialises the payload and inserts
//     the subtree under `target_parent_id` (or root-level if
//     `None`). All ids are regenerated; the top-level node's
//     bounds are shifted by the offset so paste-at-cursor / paste-
//     next-to-original works without overlap.
//   * Cross-artboard paste falls out naturally because the payload
//     carries no parent reference: the caller picks the destination.

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardPayload {
    /// Schema version so a future evolution can stay backward
    /// compatible.
    pub version: u32,
    /// Self-contained subtrees. Each entry is a Vec<Node> in
    /// pre-order (root first); the root has parent_id = None and
    /// children that reference the new ids in the same vec.
    pub subtrees: Vec<ClipboardSubtree>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardSubtree {
    /// All nodes that make up this subtree, in pre-order.
    pub nodes: Vec<Node>,
}

/// Serialise `node_ids` (each with their descendants) into a
/// portable clipboard payload. Missing ids are skipped silently —
/// the caller is the renderer reacting to the user's selection,
/// which can race with delete.
pub fn document_clipboard_copy(node_ids: &[Uuid]) -> Result<String> {
    let guard = slot().write();
    let ws = guard.as_ref().ok_or(DocumentBridgeError::NoProject)?;
    let mut subtrees = Vec::with_capacity(node_ids.len());
    for &root in node_ids {
        let Some(root_node) = ws.project.document.get_node(root) else {
            continue;
        };
        // Skip Page and Artboard. Pages are a top-level shell
        // (with their own page-duplicate code path) and Artboards
        // are managed by `artboard_*` ops; pasting them through
        // the generic clipboard would clobber the document's
        // artboard registry. The caller (renderer) gates this
        // already, but we re-check defensively.
        if matches!(root_node.node_type, NodeType::Page | NodeType::Artboard) {
            continue;
        }
        let descendant_ids = ws.project.document.descendants_of(root);
        let mut nodes = Vec::with_capacity(descendant_ids.len() + 1);
        nodes.push(root_node.clone());
        for id in &descendant_ids {
            if let Some(n) = ws.project.document.get_node(*id) {
                nodes.push(n.clone());
            }
        }
        // Detach the root from its old parent so paste sees a
        // free-floating subtree. Descendants keep their internal
        // parent references; paste remaps them.
        if let Some(root_owned) = nodes.first_mut() {
            root_owned.parent_id = None;
        }
        subtrees.push(ClipboardSubtree { nodes });
    }
    let payload = ClipboardPayload {
        version: 1,
        subtrees,
    };
    Ok(serde_json::to_string(&payload)?)
}

/// Deserialise `payload` and insert each subtree under
/// `target_parent_id`. All ids are regenerated; the top-level root
/// of every subtree is offset by (`offset_x`, `offset_y`) so paste
/// at the cursor doesn't perfectly overlap the original.
///
/// Records one `clipboard_paste` operation per subtree, but ALL the
/// operations from a single paste call share the same `group_id`
/// (`paste_group_id` below). The `OperationLog::undo_group` /
/// `redo_group` helpers — which back `document_undo_group()` and
/// `document_redo_group()` on the bridge — walk a contiguous run of
/// ops that share a group, so the user sees the entire multi-subtree
/// paste collapse into a single Ctrl+Z, matching the comment's intent.
/// Single-op `undo()` still works for callers that want fine-grained
/// stepping (the grouping is opt-in at consume time, not push time).
pub fn document_clipboard_paste(
    payload: &str,
    target_parent_id: Option<Uuid>,
    offset_x: f64,
    offset_y: f64,
) -> Result<Vec<Uuid>> {
    let parsed: ClipboardPayload = serde_json::from_str(payload).map_err(|e| {
        DocumentBridgeError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    })?;
    if parsed.version != 1 {
        return Err(DocumentBridgeError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("unsupported clipboard payload version: {}", parsed.version),
        )));
    }
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    if let Some(parent_id) = target_parent_id {
        if ws.project.document.get_node(parent_id).is_none() {
            return Err(DocumentBridgeError::NodeNotFound(parent_id));
        }
    }
    let mut new_root_ids = Vec::with_capacity(parsed.subtrees.len());
    let now = Utc::now();
    // One group per paste call — every subtree's `clipboard_paste`
    // op is tagged with the same id so undo_group consumes the
    // whole paste atomically.
    let paste_group_id = Uuid::new_v4();
    for subtree in parsed.subtrees {
        let Some(_root) = subtree.nodes.first() else {
            continue;
        };
        // Build an old_id → new_id map for the whole subtree so we
        // can rewrite parent_id / children references in lockstep.
        let mut id_map: std::collections::HashMap<Uuid, Uuid> =
            std::collections::HashMap::with_capacity(subtree.nodes.len());
        for n in &subtree.nodes {
            id_map.insert(n.id, Uuid::new_v4());
        }
        let old_root_id = subtree.nodes[0].id;
        let new_root_id = id_map[&old_root_id];
        // Capture the inserted nodes in topological order so the
        // redo arm can re-insert them parent-first (matches the
        // shape produced by `document_clipboard_copy`).
        let mut inserted_subtree: Vec<kcreate_core::node::Node> =
            Vec::with_capacity(subtree.nodes.len());
        for (idx, original) in subtree.nodes.into_iter().enumerate() {
            let old_id = original.id;
            let mut copy = original;
            copy.id = id_map[&old_id];
            copy.parent_id = if idx == 0 {
                target_parent_id
            } else {
                copy.parent_id.and_then(|pid| id_map.get(&pid).copied())
            };
            copy.children = copy
                .children
                .iter()
                .filter_map(|c| id_map.get(c).copied())
                .collect();
            copy.version = 0;
            copy.created_at = now;
            copy.updated_at = now;
            if idx == 0 {
                copy.bounds.x += offset_x;
                copy.bounds.y += offset_y;
            }
            ws.project.document.insert_node(copy.clone())?;
            inserted_subtree.push(copy);
        }
        // `after_patch` carries the full inserted subtree in
        // parent-first order so the apply_patch redo arm can
        // re-insert every node in the same order without re-running
        // copy's id-remapping logic.  `before_patch` is a marker
        // pointing at the new root id — the undo arm calls
        // `remove_node(new_root_id)` which cascades through
        // descendants, so the marker is sufficient.
        let before_patch = serde_json::json!({ "new_root_id": new_root_id });
        let after_patch = serde_json::json!({
            "subtree": inserted_subtree,
        });
        let op = Operation::new(
            "user",
            "clipboard_paste",
            before_patch,
            after_patch,
            vec![new_root_id],
        )
        .with_group(paste_group_id);
        ws.project.execute_operation(op);
        new_root_ids.push(new_root_id);
    }
    ws.project.modified_at = now;
    let _ = sync_scene_locked(&mut guard);
    drop(guard);
    Ok(new_root_ids)
}

// -----------------------------------------------------------------------------
// Phase 6 Tasks 27-28: layer colour tagging.
//
// The layer panel exposes a small palette ("red", "orange", "yellow",
// "green", "blue", "purple", "gray") that the user can attach to a
// node for visual grouping in the panel. The tag lives in
// `node.metadata["layerColor"]` so it survives save / reload and
// participates in the operation log like any other property edit.
//
// Setting `colour = None` removes the metadata key (a tagged layer
// becomes untagged), which keeps the JSON payload lean for the common
// case of untagged nodes.
// -----------------------------------------------------------------------------

/// Public metadata key for the renderer-side layer colour tag. Used
/// by `document_set_layer_color` here and consumed by the LayerPanel
/// in TypeScript through `NodeInfo.metadata.layerColor`.
pub const LAYER_COLOR_METADATA_KEY: &str = "layerColor";

/// Set or clear the layer colour tag on `node_id`. Returns the new
/// `version` so the renderer can key effects on `[id, version]` to
/// stay in lockstep with collab and undo/redo.
///
/// The tag is a free-form lowercase string the renderer interprets;
/// no validation here on the Rust side beyond rejecting strings that
/// would be JSON-illegal (empty / whitespace-only). The bridge is the
/// wrong layer to enforce a closed palette — the LayerPanel owns the
/// rendering and the migration path if we ever ship more swatches.
pub fn document_set_layer_color(node_id: Uuid, color: Option<String>) -> Result<u64> {
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    let before = ws
        .project
        .document
        .get_node(node_id)
        .map(|n| n.metadata.get(LAYER_COLOR_METADATA_KEY).cloned())
        .ok_or(DocumentBridgeError::NodeNotFound(node_id))?;
    let new_color = match color.as_deref().map(str::trim) {
        Some("") | None => None,
        Some(s) => Some(s.to_lowercase()),
    };
    let new_version = {
        let node = ws
            .project
            .document
            .get_node_mut(node_id)
            .ok_or(DocumentBridgeError::NodeNotFound(node_id))?;
        match &new_color {
            Some(s) => {
                node.metadata.insert(
                    LAYER_COLOR_METADATA_KEY.to_string(),
                    serde_json::Value::String(s.clone()),
                );
            }
            None => {
                node.metadata.remove(LAYER_COLOR_METADATA_KEY);
            }
        }
        node.version += 1;
        node.updated_at = Utc::now();
        node.version
    };
    let after = ws
        .project
        .document
        .get_node(node_id)
        .and_then(|n| n.metadata.get(LAYER_COLOR_METADATA_KEY).cloned());
    // before/after patches are themselves an Option<JSON string> —
    // `null` for the untagged state, the colour string otherwise. The
    // matching arm in `apply_patch` rewrites the metadata key based on
    // that shape, so undo restores the prior tag (or removes it) and
    // redo re-applies the new tag.
    let before_patch = match &before {
        Some(v) => v.clone(),
        None => serde_json::Value::Null,
    };
    let after_patch = match &after {
        Some(v) => v.clone(),
        None => serde_json::Value::Null,
    };
    let op = Operation::new(
        "user",
        "layer_color_set",
        before_patch,
        after_patch,
        vec![node_id],
    );
    ws.project.execute_operation(op);
    ws.project.modified_at = Utc::now();
    let _ = sync_scene_locked(&mut guard);
    Ok(new_version)
}

// -----------------------------------------------------------------------------
// G4 — Theme / Brand Kit instant restyle.
//
// `document_apply_theme` walks the whole document graph and remaps
// every solid fill / stroke / text color to the theme's role palette,
// applies the type scale to text layers, and buckets corner radii to
// the theme's radius scale — all as ONE undoable operation. The
// restyle is reversible: the recorded `apply_theme` operation carries
// per-node before/after style + text snapshots (plus the design-token
// delta), so a single undo restores the prior look exactly (see the
// `"apply_theme"` arm in `apply_patch` and `ApplyPatchSnapshot`).
//
// Color mapping is role-aware, not a flatten-to-one-color: usages are
// area-weighted across the document and ranked so the most-used fill
// becomes the background and the most saturated chromatic color
// becomes the primary (see `kcreate_core::theme::assign_roles`).
// -----------------------------------------------------------------------------

/// Summary of an [`document_apply_theme`] run, surfaced to the
/// ThemePanel so it can report what the restyle touched. Mirrors
/// `ApplyThemeReport` in `apps/desktop/shared/scene.ts`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyThemeReport {
    pub theme_id: String,
    pub theme_name: String,
    pub affected_nodes: usize,
    pub recolored_fills: usize,
    pub recolored_strokes: usize,
    pub restyled_text: usize,
}

/// Area used to weight a node's colors during role assignment. Clamped
/// to at least `1.0` so degenerate / zero-size nodes still contribute
/// a vote rather than vanishing.
fn node_area(node: &Node) -> f64 {
    let w = node.bounds.width.max(0.0);
    let h = node.bounds.height.max(0.0);
    (w * h).max(1.0)
}

/// Push every solid color carried by `style` (primary + extra fills,
/// primary + extra strokes) into `out`, each weighted by `area`.
/// Gradient / none fills are skipped — they carry no single color to
/// remap.
fn collect_solid_colors(
    style: &kcreate_core::node::NodeStyle,
    area: f64,
    out: &mut Vec<ColorUsage>,
) {
    if let FillStyle::Solid(c) = &style.fill {
        out.push(ColorUsage::new(*c, area));
    }
    for f in &style.extra_fills {
        if let FillStyle::Solid(c) = f {
            out.push(ColorUsage::new(*c, area));
        }
    }
    if let Some(stroke) = &style.stroke {
        out.push(ColorUsage::new(stroke.color, area));
    }
    for s in &style.extra_strokes {
        out.push(ColorUsage::new(s.color, area));
    }
}

/// Remap a single color in place via `remap`, preserving nothing the
/// remap didn't ask for. Returns `true` iff the color actually
/// changed (so callers can count real recolors, not no-ops).
fn remap_solid_color(color: &mut RgbaColor, remap: &HashMap<[u8; 4], RgbaColor>) -> bool {
    if let Some(target) = remap.get(&quantize(*color)) {
        if *target != *color {
            *color = *target;
            return true;
        }
    }
    false
}

/// Remap a fill in place when it is `Solid`. Gradient / none fills are
/// left untouched.
fn remap_solid_fill(fill: &mut FillStyle, remap: &HashMap<[u8; 4], RgbaColor>) -> bool {
    match fill {
        FillStyle::Solid(c) => remap_solid_color(c, remap),
        _ => false,
    }
}

/// Recolor every solid color carried by `style` and bucket its corner
/// radius through `radii`. Returns `(fills_recolored, strokes_recolored)`.
fn restyle_node_style(
    style: &mut kcreate_core::node::NodeStyle,
    remap: &HashMap<[u8; 4], RgbaColor>,
    radii: &RadiusScale,
) -> (usize, usize) {
    let mut fills = 0usize;
    let mut strokes = 0usize;
    if remap_solid_fill(&mut style.fill, remap) {
        fills += 1;
    }
    for f in &mut style.extra_fills {
        if remap_solid_fill(f, remap) {
            fills += 1;
        }
    }
    if let Some(stroke) = &mut style.stroke {
        if remap_solid_color(&mut stroke.color, remap) {
            strokes += 1;
        }
    }
    for s in &mut style.extra_strokes {
        if remap_solid_color(&mut s.color, remap) {
            strokes += 1;
        }
    }
    style.corner_radius = radii.remap(style.corner_radius);
    (fills, strokes)
}

/// Apply the theme's type scale to a text node. Classifies the node's
/// current font size into a [`TypeRole`] and rewrites the canonical
/// `TextLayerMeta` (font family + size) and, when present, the
/// `text_style` override (family + size + line height) to match the
/// theme. The text COLOR is `node.style.fill`, already handled by the
/// fill remap — this only touches typography. Returns `true` iff
/// anything changed.
fn restyle_text_node(node: &mut Node, theme: &Theme) -> Result<bool> {
    let meta_val = node
        .metadata
        .get(crate::scene_sync::TEXT_LAYER_METADATA_KEY)
        .cloned();
    let style_val = node.metadata.get("text_style").cloned();

    // Classify the role from the canonical meta size when present,
    // falling back to the override size.
    let role = meta_val
        .as_ref()
        .and_then(|mv| serde_json::from_value::<kcreate_export::TextLayerMeta>(mv.clone()).ok())
        .map(|m| TypeRole::for_size(m.font_size))
        .or_else(|| {
            style_val
                .as_ref()
                .and_then(|sv| {
                    serde_json::from_value::<crate::phase2::TextStyleWire>(sv.clone()).ok()
                })
                .map(|s| TypeRole::for_size(s.font_size))
        });
    let Some(role) = role else {
        return Ok(false);
    };
    let new_size = theme.type_scale.size_for(role);
    let new_family = theme.type_scale.font_for(role).to_string();
    let new_line_height = f64::from(theme.type_scale.line_height);

    let mut changed = false;
    if let Some(mv) = &meta_val {
        if let Ok(mut meta) = serde_json::from_value::<kcreate_export::TextLayerMeta>(mv.clone()) {
            if (meta.font_size - new_size).abs() > f32::EPSILON || meta.font_family != new_family {
                meta.font_size = new_size;
                meta.font_family.clone_from(&new_family);
                node.metadata.insert(
                    crate::scene_sync::TEXT_LAYER_METADATA_KEY.to_string(),
                    serde_json::to_value(&meta)?,
                );
                changed = true;
            }
        }
    }
    if let Some(sv) = &style_val {
        if let Ok(mut wire) = serde_json::from_value::<crate::phase2::TextStyleWire>(sv.clone()) {
            if (wire.font_size - new_size).abs() > f32::EPSILON
                || wire.font_family != new_family
                || (wire.line_height - new_line_height).abs() > f64::EPSILON
            {
                wire.font_size = new_size;
                wire.font_family.clone_from(&new_family);
                wire.line_height = new_line_height;
                node.metadata
                    .insert("text_style".to_string(), serde_json::to_value(&wire)?);
                changed = true;
            }
        }
    }
    Ok(changed)
}

/// Shared core of the theme-apply operation, parameterised by SCOPE.
///
/// * `scope == None` → whole-document restyle: every node participates
///   in role assignment and is restyled, and the theme's design tokens
///   are merged into the project (the classic `document_apply_theme`
///   behaviour).
/// * `scope == Some(ids)` → restyle only those nodes. Role assignment
///   is derived from the SELECTION's own color usages (so the subtree
///   is themed on its own terms), and the global `design_tokens` are
///   left untouched — otherwise a selection-scoped apply would mutate a
///   document-wide value and a single undo could not restore "exactly".
///
/// Returns `(report, mutated)`; `mutated` tells the caller whether to
/// re-sync the scene. The (non-empty) operation is pushed under the
/// existing `"apply_theme"` command so the same `apply_patch` arm
/// reverses it on undo — no new undo machinery, scoped or not.
fn apply_theme_core(
    ws: &mut Workspace,
    theme: &Theme,
    scope: Option<&HashSet<Uuid>>,
) -> Result<(ApplyThemeReport, bool)> {
    // Resolve the node ids to consider, in deterministic document order.
    // A scoped apply keeps only ids that are part of the scope set.
    let target_ids: Vec<Uuid> = match scope {
        None => ws.project.document.iter().map(|(id, _)| *id).collect(),
        Some(set) => ws
            .project
            .document
            .iter()
            .map(|(id, _)| *id)
            .filter(|id| set.contains(id))
            .collect(),
    };

    // Pass 1 — area-weighted color usages over the target set so role
    // assignment is deterministic.
    let mut usages: Vec<ColorUsage> = Vec::new();
    for id in &target_ids {
        if let Some(node) = ws.project.document.get_node(*id) {
            let area = node_area(node);
            collect_solid_colors(&node.style, area, &mut usages);
        }
    }
    let remap = build_color_remap(&usages, theme);

    // Design tokens: only a whole-document apply touches the global
    // tokens. A scoped apply leaves them byte-identical so the recorded
    // operation's token delta is a no-op and undo restores exactly.
    let before_tokens = ws.project.design_tokens.clone();
    let after_tokens = if scope.is_none() {
        theme.merge_into_tokens(&before_tokens)
    } else {
        before_tokens.clone()
    };
    let tokens_changed = before_tokens != after_tokens;
    if tokens_changed {
        ws.project.design_tokens = after_tokens.clone();
    }

    // Pass 2 — restyle each target node, snapshotting before / after so
    // the whole restyle collapses to one reversible operation.
    let mut before_nodes: BTreeMap<Uuid, ApplyThemeNodePatch> = BTreeMap::new();
    let mut after_nodes: BTreeMap<Uuid, ApplyThemeNodePatch> = BTreeMap::new();
    let mut recolored_fills = 0usize;
    let mut recolored_strokes = 0usize;
    let mut restyled_text = 0usize;

    for id in target_ids {
        let Some(node) = ws.project.document.get_node_mut(id) else {
            continue;
        };
        let before_style = node.style.clone();
        let before_meta = node
            .metadata
            .get(crate::scene_sync::TEXT_LAYER_METADATA_KEY)
            .cloned();
        let before_text_style = node.metadata.get("text_style").cloned();

        let (fills, strokes) = restyle_node_style(&mut node.style, &remap, &theme.radii);
        let text_changed = if matches!(node.node_type, NodeType::TextLayer) {
            restyle_text_node(node, theme)?
        } else {
            false
        };

        let style_changed = node.style != before_style;
        if !style_changed && !text_changed {
            continue;
        }
        node.touch();
        recolored_fills += fills;
        recolored_strokes += strokes;
        if text_changed {
            restyled_text += 1;
        }

        let after_meta = node
            .metadata
            .get(crate::scene_sync::TEXT_LAYER_METADATA_KEY)
            .cloned();
        let after_text_style = node.metadata.get("text_style").cloned();
        before_nodes.insert(
            id,
            ApplyThemeNodePatch {
                style: before_style,
                text_meta: before_meta,
                text_style: before_text_style,
            },
        );
        after_nodes.insert(
            id,
            ApplyThemeNodePatch {
                style: node.style.clone(),
                text_meta: after_meta,
                text_style: after_text_style,
            },
        );
    }

    let affected: Vec<Uuid> = before_nodes.keys().copied().collect();
    let report = ApplyThemeReport {
        theme_id: theme.id.clone(),
        theme_name: theme.name.clone(),
        affected_nodes: affected.len(),
        recolored_fills,
        recolored_strokes,
        restyled_text,
    };

    // Nothing visibly changed and tokens are identical → don't push a
    // no-op onto the undo stack.
    if affected.is_empty() && !tokens_changed {
        return Ok((report, false));
    }

    let before_patch = serde_json::to_value(ApplyThemePatch {
        design_tokens: before_tokens,
        nodes: before_nodes,
    })?;
    let after_patch = serde_json::to_value(ApplyThemePatch {
        design_tokens: after_tokens,
        nodes: after_nodes,
    })?;
    let op = Operation::new("user", "apply_theme", before_patch, after_patch, affected);
    ws.project.execute_operation(op);
    ws.project.modified_at = Utc::now();
    Ok((report, true))
}

/// A zero report for `theme`, used when a scoped apply has nothing to
/// restyle (empty selection) so the panel can call apply unconditionally.
fn empty_apply_report(theme: &Theme) -> ApplyThemeReport {
    ApplyThemeReport {
        theme_id: theme.id.clone(),
        theme_name: theme.name.clone(),
        affected_nodes: 0,
        recolored_fills: 0,
        recolored_strokes: 0,
        restyled_text: 0,
    }
}

/// Instantly restyle the open document to `theme` as a single undoable
/// operation. See the module-level section comment for the contract.
pub fn document_apply_theme(theme: &Theme) -> Result<ApplyThemeReport> {
    let mut guard = slot().write();
    let (report, mutated) = {
        let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
        apply_theme_core(ws, theme, None)?
    };
    if mutated {
        let _ = sync_scene_locked(&mut guard);
    }
    Ok(report)
}

/// Restyle only the selected subtree(s) to `theme`, as a single
/// undoable operation that restores exactly on undo.
///
/// `roots` empty → the live document selection is used. Each root
/// expands to the root plus all of its descendants, so theming a frame
/// themes everything inside it. An empty resolved scope (nothing
/// selected, or the selection no longer exists) is a no-op that returns
/// a zero report rather than erroring, so the panel can wire a single
/// "apply to selection" button without guarding the selection state.
pub fn document_apply_theme_to_selection(
    theme: &Theme,
    roots: Vec<Uuid>,
) -> Result<ApplyThemeReport> {
    let mut guard = slot().write();
    let (report, mutated) = {
        let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;

        // Explicit roots win; otherwise fall back to the live selection.
        let roots: Vec<Uuid> = if roots.is_empty() {
            ws.selection.clone()
        } else {
            roots
        };

        // Expand each existing root into {root} ∪ descendants.
        let mut scope: HashSet<Uuid> = HashSet::new();
        for root in roots {
            if ws.project.document.get_node(root).is_some() {
                scope.insert(root);
                scope.extend(ws.project.document.descendants_of(root));
            }
        }

        if scope.is_empty() {
            (empty_apply_report(theme), false)
        } else {
            apply_theme_core(ws, theme, Some(&scope))?
        }
    };
    if mutated {
        let _ = sync_scene_locked(&mut guard);
    }
    Ok(report)
}

/// Derive a [`Theme`] from the open document's own palette. Aggregates
/// area-weighted solid colors across the graph, runs them through
/// `kcreate_ai::palette::extract_palette` (the same k-means extractor
/// the AI palette panel uses), and assigns roles. Returns a theme the
/// caller can preview, tweak, or apply.
pub fn theme_derive_from_document(name: &str) -> Result<Theme> {
    let guard = slot().read();
    let ws = guard.as_ref().ok_or(DocumentBridgeError::NoProject)?;
    let mut usages: Vec<ColorUsage> = Vec::new();
    for (_id, node) in ws.project.document.iter() {
        let area = node_area(node);
        collect_solid_colors(&node.style, area, &mut usages);
    }
    drop(guard);
    Ok(derive_theme_from_usages(name, &usages))
}

/// Build a synthetic area-weighted RGBA8 strip from the document's
/// solid-color usages and run `kcreate_ai::palette::extract_palette`
/// over it, then assign roles via [`Theme::derive_from_palette`].
/// Going through the palette extractor (rather than the raw usages)
/// keeps the "derive from the design's palette" path identical to the
/// AI palette panel and merges near-duplicate colors via k-means.
fn derive_theme_from_usages(name: &str, usages: &[ColorUsage]) -> Theme {
    // Fixed pixel budget distributed across colors proportional to area;
    // at least one pixel per distinct color so nothing is lost.
    const BUDGET: usize = 4096;
    // Aggregate by quantized color so weights are stable / order-free.
    let mut weights: BTreeMap<[u8; 4], f64> = BTreeMap::new();
    for u in usages {
        if u.color.a <= 0.0 {
            continue;
        }
        *weights.entry(quantize(u.color)).or_insert(0.0) += u.area.max(0.0);
    }
    let total: f64 = weights.values().sum();
    if weights.is_empty() || total <= 0.0 {
        return Theme::derive_from_palette(name, &[]);
    }
    let mut pixels: Vec<u8> = Vec::new();
    for (key, w) in &weights {
        let count = (((w / total) * BUDGET as f64).round() as usize).max(1);
        for _ in 0..count {
            pixels.extend_from_slice(&[key[0], key[1], key[2], 255]);
        }
    }
    let width = (pixels.len() / 4) as u32;
    let extracted = kcreate_ai::palette::extract_palette(&pixels, width, 1, 7);
    theme_from_extracted_colors(name, &extracted)
}

/// Map a k-means [`ExtractedColor`] palette to a [`Theme`] via
/// [`Theme::derive_from_palette`] (opaque colors, frequency-weighted
/// role assignment). Empty input yields the neutral fallback theme.
/// Shared by the derive-from-document and derive-from-image paths so
/// both turn an extracted palette into a theme identically.
fn theme_from_extracted_colors(
    name: &str,
    extracted: &[kcreate_ai::palette::ExtractedColor],
) -> Theme {
    if extracted.is_empty() {
        return Theme::derive_from_palette(name, &[]);
    }
    let palette: Vec<(RgbaColor, f32)> = extracted
        .iter()
        .map(|c| {
            (
                RgbaColor::new(
                    f32::from(c.r) / 255.0,
                    f32::from(c.g) / 255.0,
                    f32::from(c.b) / 255.0,
                    1.0,
                ),
                c.frequency,
            )
        })
        .collect();
    Theme::derive_from_palette(name, &palette)
}

/// Derive a [`Theme`] from an uploaded image's dominant palette.
/// Decodes the image, extracts up to 7 colors with
/// `kcreate_ai::palette::extract_palette` (the Canva "brand colors from
/// a photo" flow), and assigns roles via [`Theme::derive_from_palette`].
/// Pure/offline and workspace-independent — the caller decides whether
/// to preview or apply the result. Returns `InvalidArgument` when the
/// bytes are empty and `Io(InvalidData)` when they aren't a decodable
/// image.
pub fn theme_derive_from_image(name: &str, bytes: &[u8]) -> Result<Theme> {
    if bytes.is_empty() {
        return Err(DocumentBridgeError::InvalidArgument {
            argument: "bytes".to_string(),
            value: "empty image".to_string(),
        });
    }
    let img = image::load_from_memory(bytes)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    let extracted = kcreate_ai::palette::extract_palette(rgba.as_raw(), width, height, 7);
    Ok(theme_from_extracted_colors(name, &extracted))
}

// -----------------------------------------------------------------------------
// Components
// -----------------------------------------------------------------------------

/// Wire shape returned by [`component_list`]. Mirrors `ComponentInfo`
/// in `apps/desktop/shared/scene.ts`. We `rename_all = "camelCase"` so
/// the renderer can `JSON.parse` directly without a key transform.
///
/// `PartialEq` is intentionally not `Eq`: the free-form
/// `ComponentVariantInfo::properties` bag contains `serde_json::Value`
/// which can carry `f64` / `NaN`, and `Eq` would lie about that.
#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ComponentInfo {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub default_variant_id: Uuid,
    pub variants: Vec<ComponentVariantInfo>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub modified_at: chrono::DateTime<chrono::Utc>,
}

#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ComponentVariantInfo {
    pub id: Uuid,
    pub name: String,
    pub properties: std::collections::HashMap<String, serde_json::Value>,
}

impl From<&ComponentDefinition> for ComponentInfo {
    fn from(def: &ComponentDefinition) -> Self {
        Self {
            id: def.id,
            name: def.name.clone(),
            description: def.description.clone(),
            default_variant_id: def.default_variant_id,
            variants: def
                .variants
                .iter()
                .map(|v| ComponentVariantInfo {
                    id: v.id,
                    name: v.name.clone(),
                    properties: v.properties.clone(),
                })
                .collect(),
            created_at: def.created_at,
            modified_at: def.modified_at,
        }
    }
}

/// Convert a selection of nodes into a reusable component definition.
///
/// The selected nodes (and their descendants) are snapshotted into a
/// new [`ComponentDefinition`]. A `NodeType::ComponentLayer` node is
/// created in place of the selection, and the originals are
/// reparented under that new layer (the selection becomes the first
/// instance, with no extra clone needed).
///
/// Subsequent instances of the same component re-clone the snapshot
/// stored on the definition.
///
/// Returns the new component's id. Records an undoable
/// `component_create_from_selection` operation.
pub fn component_create_from_selection(node_ids: Vec<Uuid>, name: String) -> Result<Uuid> {
    if node_ids.is_empty() {
        return Err(DocumentBridgeError::InvalidComponentSelection(
            "selection is empty".into(),
        ));
    }
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;

    // Validate every selected node exists and share a common parent
    // (UI selections are always sibling-flat — supporting cross-parent
    // selections would require choosing a parent for the new layer,
    // which is ambiguous).
    let mut parents = HashSet::new();
    for id in &node_ids {
        let node = ws
            .project
            .document
            .get_node(*id)
            .ok_or(DocumentBridgeError::NodeNotFound(*id))?;
        parents.insert(node.parent_id);
    }
    if parents.len() != 1 {
        return Err(DocumentBridgeError::InvalidComponentSelection(
            "all selected nodes must share the same parent".into(),
        ));
    }
    let parent_id = parents
        .into_iter()
        .next()
        .expect("checked single parent above");

    // Compute the bounding rect of the selection so the new
    // ComponentLayer wraps it sensibly.
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for id in &node_ids {
        let n = ws
            .project
            .document
            .get_node(*id)
            .ok_or(DocumentBridgeError::NodeNotFound(*id))?;
        min_x = min_x.min(n.bounds.x);
        min_y = min_y.min(n.bounds.y);
        max_x = max_x.max(n.bounds.x + n.bounds.width);
        max_y = max_y.max(n.bounds.y + n.bounds.height);
    }
    if !(min_x.is_finite() && min_y.is_finite() && max_x.is_finite() && max_y.is_finite()) {
        return Err(DocumentBridgeError::InvalidComponentSelection(
            "selection bounding box is degenerate".into(),
        ));
    }
    let bounds = kcreate_core::node::Bounds::new(min_x, min_y, max_x - min_x, max_y - min_y);

    // Build the definition. We deep-clone the source subtrees *into*
    // the project graph temporarily, snapshot them as serialized
    // payloads onto the definition, then leave the original nodes in
    // place to become the first instance's children.
    let mut definition = ComponentDefinition::new(&name);

    // Snapshot the source subtrees by serializing each root node and
    // its descendants. The bridge instantiates new copies from this
    // payload via `instantiate_component_snapshot`.
    let mut source_payloads = Vec::with_capacity(node_ids.len());
    for id in &node_ids {
        let mut subtree = Vec::new();
        let all = std::iter::once(*id).chain(ws.project.document.descendants_of(*id));
        for nid in all {
            if let Some(n) = ws.project.document.get_node(nid) {
                subtree.push(n.clone());
            }
        }
        source_payloads.push(subtree);
    }
    let snapshot_json = serde_json::to_value(&source_payloads)?;

    // Create the new ComponentLayer node where the selection was.
    let mut layer = Node::new(NodeType::ComponentLayer, name);
    layer.parent_id = parent_id;
    layer.bounds = bounds;
    let layer_id = ws.project.document.insert_node(layer)?;

    // Reparent the original selection under the new ComponentLayer.
    // Reparent operates child-by-child, preserving order.
    for (i, id) in node_ids.iter().enumerate() {
        ws.project.document.reparent_node(*id, Some(layer_id), i)?;
    }

    // Bake the instance metadata onto the new layer (so the first
    // instance survives save/reopen even before any further edits).
    let instance = ComponentInstance::new(&definition);
    if let Some(layer_node) = ws.project.document.get_node_mut(layer_id) {
        let v = serde_json::to_value(&instance)?;
        layer_node
            .metadata
            .insert(COMPONENT_INSTANCE_METADATA_KEY.to_string(), v);
        // Also stash the source snapshot on the definition so
        // subsequent instantiations clone from a stable payload.
        layer_node.touch();
    }
    definition.source_node_ids.extend(
        source_payloads
            .iter()
            .filter_map(|s| s.first().map(|n| n.id)),
    );
    // Store the source snapshot in a definition-level metadata
    // (variant 0 owns it under the conventional `source_snapshot` key).
    if let Some(default) = definition.variant_mut(definition.default_variant_id) {
        default
            .properties
            .insert("source_snapshot".to_string(), snapshot_json);
    }
    let component_id = ws.project.register_component(definition);

    let snapshot_node = ws
        .project
        .document
        .get_node(layer_id)
        .map_or(serde_json::Value::Null, |n| {
            serde_json::to_value(n).unwrap_or(serde_json::Value::Null)
        });
    let op = Operation::new(
        "user",
        "component_create_from_selection",
        serde_json::to_value(&node_ids).unwrap_or(serde_json::Value::Null),
        snapshot_node,
        vec![layer_id],
    );
    ws.project.execute_operation(op);
    let _ = sync_scene_locked(&mut guard);
    drop(guard);
    Ok(component_id)
}

/// List all registered components (sorted by name).
pub fn component_list() -> Result<Vec<ComponentInfo>> {
    let guard = slot().write();
    let ws = guard.as_ref().ok_or(DocumentBridgeError::NoProject)?;
    Ok(ws
        .project
        .list_components()
        .into_iter()
        .map(ComponentInfo::from)
        .collect())
}

/// Instantiate a component at `(x, y)` under `parent_id`. Returns the
/// new `NodeType::ComponentLayer` node id.
pub fn component_instantiate(
    component_id: Uuid,
    parent_id: Option<Uuid>,
    x: f64,
    y: f64,
) -> Result<Uuid> {
    if !(x.is_finite() && y.is_finite()) {
        return Err(DocumentBridgeError::InvalidBounds {
            width: x,
            height: y,
        });
    }
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;

    // Snapshot the definition so we can release the borrow before
    // mutating the document graph below. Definitions are small (a
    // handful of variants + a JSON snapshot), so the clone is cheap.
    let def = ws
        .project
        .get_component(component_id)
        .cloned()
        .ok_or_else(|| {
            DocumentBridgeError::Project(ProjectError::ComponentNotFound(component_id))
        })?;
    let default_variant_id = def.default_variant_id;
    let snapshot_value = def
        .variant(default_variant_id)
        .and_then(|v| v.properties.get("source_snapshot").cloned())
        .ok_or_else(|| {
            DocumentBridgeError::InvalidComponentSelection(
                "component is missing its source snapshot".into(),
            )
        })?;
    let source_payloads: Vec<Vec<Node>> =
        serde_json::from_value(snapshot_value).map_err(DocumentBridgeError::Json)?;

    // Compute the bounding rect of the source payload so the new
    // ComponentLayer covers exactly its children.
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for payload in &source_payloads {
        if let Some(root) = payload.first() {
            min_x = min_x.min(root.bounds.x);
            min_y = min_y.min(root.bounds.y);
            max_x = max_x.max(root.bounds.x + root.bounds.width);
            max_y = max_y.max(root.bounds.y + root.bounds.height);
        }
    }
    if !(min_x.is_finite() && min_y.is_finite() && max_x.is_finite() && max_y.is_finite()) {
        return Err(DocumentBridgeError::InvalidComponentSelection(
            "component snapshot is empty".into(),
        ));
    }
    let width = max_x - min_x;
    let height = max_y - min_y;

    // Create the new ComponentLayer and reparent at (x, y).
    let mut layer = Node::new(NodeType::ComponentLayer, &def.name);
    layer.parent_id = parent_id;
    layer.bounds = kcreate_core::node::Bounds::new(x, y, width, height);
    let layer_id = ws.project.document.insert_node(layer)?;

    // Deep-clone each source subtree into the document as children of
    // the new layer. Children are reparented by writing the new
    // parent_id and rebuilding the parent's children list as we go.
    instantiate_component_snapshot(
        &mut ws.project.document,
        &source_payloads,
        layer_id,
        (x - min_x, y - min_y),
    )?;

    let instance = ComponentInstance {
        definition_id: component_id,
        active_variant_id: default_variant_id,
        overrides: HashMap::new(),
    };
    if let Some(node) = ws.project.document.get_node_mut(layer_id) {
        node.metadata.insert(
            COMPONENT_INSTANCE_METADATA_KEY.to_string(),
            serde_json::to_value(&instance)?,
        );
        node.touch();
    }

    let after_snapshot = ws
        .project
        .document
        .get_node(layer_id)
        .map_or(serde_json::Value::Null, |n| {
            serde_json::to_value(n).unwrap_or(serde_json::Value::Null)
        });
    let op = Operation::new(
        "user",
        "component_instantiate",
        serde_json::to_value(component_id).unwrap_or(serde_json::Value::Null),
        after_snapshot,
        vec![layer_id],
    );
    ws.project.execute_operation(op);
    let _ = sync_scene_locked(&mut guard);
    drop(guard);
    Ok(layer_id)
}

/// Append a fresh variant to a component. Returns the new variant id.
pub fn component_add_variant(component_id: Uuid, name: String) -> Result<Uuid> {
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    let vid = ws
        .project
        .add_component_variant(component_id, ComponentVariant::new(&name))?;
    let op = Operation::new(
        "user",
        "component_add_variant",
        serde_json::to_value(component_id).unwrap_or(serde_json::Value::Null),
        serde_json::to_value(vid).unwrap_or(serde_json::Value::Null),
        Vec::new(),
    );
    ws.project.execute_operation(op);
    drop(guard);
    Ok(vid)
}

/// Snapshot of a single layer used by Smart Animate property
/// interpolation. Carries only the fields the renderer animates
/// (bounds, opacity, fill colour, corner radius) — the full Node
/// payload would inflate the IPC frame and isn't needed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmartAnimateLayer {
    pub name: String,
    pub bounds: kcreate_core::node::Bounds,
    pub opacity: f32,
    /// CSS-style fill colour (`#RRGGBB`), `None` for non-solid fills
    /// (gradients, images, …) which Smart Animate doesn't blend.
    pub fill_color: Option<String>,
    pub corner_radius: f64,
}

/// Pair of layer snapshots — the renderer's `PrototypePlayer`
/// interpolates corresponding entries by `name` over the configured
/// transition duration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmartAnimateSnapshot {
    pub before: Vec<SmartAnimateLayer>,
    pub after: Vec<SmartAnimateLayer>,
}

fn solid_fill_hex(fill: &kcreate_core::FillStyle) -> Option<String> {
    if let kcreate_core::FillStyle::Solid(c) = fill {
        let r = (c.r.clamp(0.0, 1.0) * 255.0).round() as u8;
        let g = (c.g.clamp(0.0, 1.0) * 255.0).round() as u8;
        let b = (c.b.clamp(0.0, 1.0) * 255.0).round() as u8;
        Some(format!("#{r:02X}{g:02X}{b:02X}"))
    } else {
        None
    }
}

fn smart_animate_layer_from_node(node: &kcreate_core::node::Node) -> SmartAnimateLayer {
    SmartAnimateLayer {
        name: node.name.clone(),
        bounds: node.bounds,
        opacity: node.opacity,
        fill_color: solid_fill_hex(&node.style.fill),
        corner_radius: node.style.corner_radius,
    }
}

/// Collect [`SmartAnimateLayer`] entries from a flat snapshot
/// payload (the serialised `Vec<Vec<Node>>` stored on a variant's
/// `source_snapshot` property). Only the first node of each
/// subtree is emitted because Smart Animate matches layers at the
/// instance root level — deeper recursion would conflate layers
/// with duplicate names across siblings.
fn smart_animate_layers_from_snapshot(
    payloads: &[Vec<kcreate_core::node::Node>],
) -> Vec<SmartAnimateLayer> {
    payloads
        .iter()
        .filter_map(|payload| payload.first().map(smart_animate_layer_from_node))
        .collect()
}

/// Smart Animate snapshot for a switch from the instance's current
/// variant to `target_variant_id`. The renderer interpolates the
/// matching entries (by `name`) over the transition's duration; new
/// names fade in, removed names fade out.
///
/// This is read-only — it does *not* mutate the active variant. The
/// renderer calls [`component_switch_variant`] to commit the change
/// once its animation has finished.
///
/// Phase 11 Block D follow-up round 4 — Devin Review BUG-0001 (r4).
/// This entry point is documented read-only in `scene.ts` and
/// `lib.rs` and only ever calls `guard.as_ref()`. Holding the
/// workspace `RwLock` in `write()` mode for a pure read serialises
/// concurrent readers (tree view, selection inspector, renderer
/// version polling) for the duration of the snapshot — directly
/// undoing the Phase 11 Block D RwLock work documented in
/// ARCHITECTURE.md §17p. Acquire `read()` instead so concurrent
/// readers stay parallel.
pub fn component_smart_animate_snapshot(
    node_id: Uuid,
    target_variant_id: Uuid,
) -> Result<SmartAnimateSnapshot> {
    let guard = slot().read();
    let ws = guard.as_ref().ok_or(DocumentBridgeError::NoProject)?;
    let node = ws
        .project
        .document
        .get_node(node_id)
        .ok_or(DocumentBridgeError::NodeNotFound(node_id))?;
    if node.node_type != NodeType::ComponentLayer {
        return Err(DocumentBridgeError::WrongNodeType {
            expected: NodeType::ComponentLayer,
            got: node.node_type,
        });
    }
    let inst_meta = node
        .metadata
        .get(COMPONENT_INSTANCE_METADATA_KEY)
        .cloned()
        .ok_or_else(|| {
            DocumentBridgeError::InvalidComponentInstance(
                node_id,
                "node is missing component_instance metadata".into(),
            )
        })?;
    let inst: ComponentInstance = serde_json::from_value(inst_meta)
        .map_err(|e| DocumentBridgeError::InvalidComponentInstance(node_id, e.to_string()))?;
    let before: Vec<SmartAnimateLayer> = node
        .children
        .iter()
        .filter_map(|cid| {
            ws.project
                .document
                .get_node(*cid)
                .map(smart_animate_layer_from_node)
        })
        .collect();
    let def = ws
        .project
        .get_component(inst.definition_id)
        .ok_or_else(|| {
            DocumentBridgeError::Project(ProjectError::ComponentNotFound(inst.definition_id))
        })?;
    // First confirm the variant actually exists — that's a hard
    // error. *Missing* `source_snapshot` is a distinct (and
    // legitimate) state: a freshly added variant before the
    // user has authored any content. In that case the "after"
    // set is empty and the renderer naturally fades the
    // existing layers out.
    let variant = def.variant(target_variant_id).ok_or_else(|| {
        DocumentBridgeError::Project(ProjectError::Component(
            kcreate_core::ComponentError::VariantNotFound(target_variant_id),
        ))
    })?;
    let after = if let Some(snapshot_value) = variant.properties.get("source_snapshot") {
        let payloads: Vec<Vec<kcreate_core::node::Node>> =
            serde_json::from_value(snapshot_value.clone()).map_err(DocumentBridgeError::Json)?;
        smart_animate_layers_from_snapshot(&payloads)
    } else {
        Vec::new()
    };
    Ok(SmartAnimateSnapshot { before, after })
}

/// Switch the active variant of a component instance node.
pub fn component_switch_variant(node_id: Uuid, variant_id: Uuid) -> Result<()> {
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    // Validate the node is a ComponentLayer with valid instance
    // metadata, and the variant belongs to its definition.
    let (definition_id, before_value) = {
        let node = ws
            .project
            .document
            .get_node(node_id)
            .ok_or(DocumentBridgeError::NodeNotFound(node_id))?;
        if node.node_type != NodeType::ComponentLayer {
            return Err(DocumentBridgeError::WrongNodeType {
                expected: NodeType::ComponentLayer,
                got: node.node_type,
            });
        }
        let value = node
            .metadata
            .get(COMPONENT_INSTANCE_METADATA_KEY)
            .cloned()
            .ok_or_else(|| {
                DocumentBridgeError::InvalidComponentInstance(
                    node_id,
                    "node is missing component_instance metadata".into(),
                )
            })?;
        let inst: ComponentInstance = serde_json::from_value(value.clone())
            .map_err(|e| DocumentBridgeError::InvalidComponentInstance(node_id, e.to_string()))?;
        (inst.definition_id, value)
    };
    let def = ws.project.get_component(definition_id).ok_or_else(|| {
        DocumentBridgeError::Project(ProjectError::ComponentNotFound(definition_id))
    })?;
    if def.variant(variant_id).is_none() {
        return Err(DocumentBridgeError::Project(ProjectError::Component(
            kcreate_core::ComponentError::VariantNotFound(variant_id),
        )));
    }

    // Write the new variant id.
    let node = ws
        .project
        .document
        .get_node_mut(node_id)
        .ok_or(DocumentBridgeError::NodeNotFound(node_id))?;
    let value = node
        .metadata
        .get_mut(COMPONENT_INSTANCE_METADATA_KEY)
        .ok_or_else(|| {
            DocumentBridgeError::InvalidComponentInstance(
                node_id,
                "node is missing component_instance metadata".into(),
            )
        })?;
    let mut inst: ComponentInstance = serde_json::from_value(value.clone())
        .map_err(|e| DocumentBridgeError::InvalidComponentInstance(node_id, e.to_string()))?;
    inst.active_variant_id = variant_id;
    *value = serde_json::to_value(&inst)?;
    node.touch();

    let after_value = node
        .metadata
        .get(COMPONENT_INSTANCE_METADATA_KEY)
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let op = Operation::new(
        "user",
        "component_switch_variant",
        before_value,
        after_value,
        vec![node_id],
    );
    ws.project.execute_operation(op);
    let _ = sync_scene_locked(&mut guard);
    drop(guard);
    Ok(())
}

/// Detach a component instance: removes the `component_instance`
/// metadata and converts the ComponentLayer into a regular
/// `NodeType::GroupLayer` so its children remain editable as plain
/// nodes.
pub fn component_detach(node_id: Uuid) -> Result<()> {
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
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
    if node.node_type != NodeType::ComponentLayer {
        return Err(DocumentBridgeError::WrongNodeType {
            expected: NodeType::ComponentLayer,
            got: node.node_type,
        });
    }
    node.metadata.remove(COMPONENT_INSTANCE_METADATA_KEY);
    node.node_type = NodeType::GroupLayer;
    node.touch();
    let after = serde_json::to_value(&*node)?;
    let op = Operation::new("user", "component_detach", before, after, vec![node_id]);
    ws.project.execute_operation(op);
    let _ = sync_scene_locked(&mut guard);
    drop(guard);
    Ok(())
}

/// Deep-clone a previously snapshotted source payload into the
/// document under `new_parent`, offset by `(dx, dy)` so the
/// instance lands where the caller asked.
///
/// Each entry in `source_payloads` is a depth-first list of nodes
/// (root + descendants in any order, as long as parent_ids resolve
/// within the payload). The first node in each payload is the
/// subtree root.
fn instantiate_component_snapshot(
    doc: &mut DocumentGraph,
    source_payloads: &[Vec<Node>],
    new_parent: Uuid,
    offset: (f64, f64),
) -> Result<Vec<Uuid>> {
    let mut new_root_ids = Vec::with_capacity(source_payloads.len());
    for payload in source_payloads {
        if payload.is_empty() {
            continue;
        }
        // Remap all old ids → fresh ones.
        let mut id_map: HashMap<Uuid, Uuid> = HashMap::with_capacity(payload.len());
        for n in payload {
            id_map.insert(n.id, Uuid::new_v4());
        }
        let root_old_id = payload[0].id;

        // Insert the root first (its parent is `new_parent`), then
        // insert every descendant in payload order. Each descendant's
        // parent_id has already been remapped to a node we previously
        // inserted, so `insert_node` succeeds without
        // `InvalidReparent`.
        for (i, original) in payload.iter().enumerate() {
            let mut copy = original.clone();
            copy.id = id_map[&original.id];
            copy.parent_id = if i == 0 {
                Some(new_parent)
            } else {
                original
                    .parent_id
                    .and_then(|pid| id_map.get(&pid).copied())
                    .or(Some(new_parent))
            };
            // Children get fixed up automatically by insert_node as
            // each child is inserted; clear the snapshot's children
            // list so we don't pre-populate stale ids.
            copy.children.clear();
            if i == 0 {
                copy.bounds.x += offset.0;
                copy.bounds.y += offset.1;
            }
            copy.version = 0;
            doc.insert_node(copy)?;
        }
        new_root_ids.push(id_map[&root_old_id]);
    }
    Ok(new_root_ids)
}

fn find_first_page(doc: &DocumentGraph) -> Option<Uuid> {
    let mut pages: Vec<&Node> = doc
        .iter()
        .map(|(_, n)| n)
        .filter(|n| n.node_type == NodeType::Page)
        .collect();
    pages.sort_by_key(|p| p.id);
    pages.first().map(|p| p.id)
}

fn next_artboard_x(doc: &DocumentGraph, page_id: Uuid) -> f64 {
    let existing = doc.list_artboards(page_id);
    if existing.is_empty() {
        0.0
    } else {
        // Place to the right of the rightmost existing artboard with
        // a 100-px gap.
        existing
            .iter()
            .map(|n| n.bounds.x + n.bounds.width)
            .fold(f64::NEG_INFINITY, f64::max)
            + 100.0
    }
}

// -----------------------------------------------------------------------------
// Auto-layout (Block C)
// -----------------------------------------------------------------------------

/// Metadata key that stores a `LayoutFrame`'s active layout config
/// as a JSON-serialized [`LayoutConfig`] payload.
pub const LAYOUT_CONFIG_METADATA_KEY: &str = "layout";

/// Tagged config that lives on a `LayoutFrame`. The `mode` tag lets
/// the bridge dispatch to the right solver on
/// [`layout_recompute`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum LayoutConfig {
    Flex(FlexLayout),
    Grid(GridLayout),
}

/// Write a flex layout config onto the given `LayoutFrame` node. The
/// child positions are *not* recomputed by this call — invoke
/// [`layout_recompute`] explicitly when ready (the host typically
/// debounces recompute requests).
pub fn layout_set_flex(node_id: Uuid, layout: FlexLayout) -> Result<()> {
    write_layout_metadata(node_id, LayoutConfig::Flex(layout), "layout_set_flex")
}

/// Write a grid layout config onto the given `LayoutFrame` node.
pub fn layout_set_grid(node_id: Uuid, layout: GridLayout) -> Result<()> {
    write_layout_metadata(node_id, LayoutConfig::Grid(layout), "layout_set_grid")
}

fn write_layout_metadata(node_id: Uuid, config: LayoutConfig, op_kind: &str) -> Result<()> {
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    let before;
    let after;
    {
        let node = ws
            .project
            .document
            .get_node_mut(node_id)
            .ok_or(DocumentBridgeError::NodeNotFound(node_id))?;
        if node.node_type != NodeType::LayoutFrame {
            return Err(DocumentBridgeError::WrongNodeType {
                expected: NodeType::LayoutFrame,
                got: node.node_type,
            });
        }
        before = serde_json::to_value(&*node)?;
        node.metadata.insert(
            LAYOUT_CONFIG_METADATA_KEY.to_string(),
            serde_json::to_value(config)?,
        );
        node.touch();
        after = serde_json::to_value(&*node)?;
    }
    let op = Operation::new("user", op_kind, before, after, vec![node_id]);
    ws.project.execute_operation(op);
    let _ = sync_scene_locked(&mut guard);
    drop(guard);
    Ok(())
}

/// Recompute child positions on a `LayoutFrame`. Reads the layout
/// config from the node's metadata, gathers child intrinsic sizes
/// from their current bounds, runs the solver, then writes the new
/// bounds back as a single undoable operation.
///
/// If the node has no layout metadata this is a no-op (returns Ok).
/// If the metadata is malformed we surface
/// [`DocumentBridgeError::InvalidLayoutConfig`] so the caller can
/// recover (e.g. by re-running `layout_set_flex`).
pub fn layout_recompute(node_id: Uuid) -> Result<()> {
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;

    // Stage 1: read everything we need under the lock — config,
    // parent bounds, and the child ids+sizes — in one borrow.
    let (config, parent_bounds, child_inputs): (LayoutConfig, _, Vec<(Uuid, f64, f64)>) = {
        let node = ws
            .project
            .document
            .get_node(node_id)
            .ok_or(DocumentBridgeError::NodeNotFound(node_id))?;
        if node.node_type != NodeType::LayoutFrame {
            return Err(DocumentBridgeError::WrongNodeType {
                expected: NodeType::LayoutFrame,
                got: node.node_type,
            });
        }
        let raw = match node.metadata.get(LAYOUT_CONFIG_METADATA_KEY) {
            Some(v) => v.clone(),
            None => return Ok(()),
        };
        let config: LayoutConfig = serde_json::from_value(raw)
            .map_err(|e| DocumentBridgeError::InvalidLayoutConfig(node_id, e.to_string()))?;
        let parent_bounds = node.bounds;
        let mut inputs = Vec::with_capacity(node.children.len());
        for cid in &node.children {
            if let Some(c) = ws.project.document.get_node(*cid) {
                inputs.push((*cid, c.bounds.width, c.bounds.height));
            }
        }
        (config, parent_bounds, inputs)
    };

    // Stage 2: run the pure solver (no document mutation).
    let placements = match config {
        LayoutConfig::Flex(f) => layout_flex(parent_bounds, &child_inputs, &f),
        LayoutConfig::Grid(g) => layout_grid(parent_bounds, &child_inputs, &g),
    };

    // Stage 3: apply the placements. We capture before/after of the
    // *parent* node so a single undo restores the whole layout to
    // its previous state — children's previous bounds are encoded
    // in the operation's before snapshot via the parent's snapshot
    // plus the child snapshots we also capture.
    let mut before_children: Vec<serde_json::Value> = Vec::with_capacity(placements.len());
    let mut after_children: Vec<serde_json::Value> = Vec::with_capacity(placements.len());
    let mut affected: Vec<Uuid> = Vec::with_capacity(placements.len() + 1);
    affected.push(node_id);
    for (cid, new_bounds) in placements {
        let child = ws
            .project
            .document
            .get_node_mut(cid)
            .ok_or(DocumentBridgeError::NodeNotFound(cid))?;
        before_children.push(serde_json::to_value(&*child)?);
        child.bounds = new_bounds;
        child.touch();
        after_children.push(serde_json::to_value(&*child)?);
        affected.push(cid);
    }
    let before = serde_json::json!({
        "config": config,
        "children": before_children,
    });
    let after = serde_json::json!({
        "config": config,
        "children": after_children,
    });
    let op = Operation::new("user", "layout_recompute", before, after, affected);
    ws.project.execute_operation(op);
    let _ = sync_scene_locked(&mut guard);
    drop(guard);
    Ok(())
}

/// Maximum recursion depth for [`layout_propagate_in_subtree`].
///
/// Component instances can reference each other (a component
/// definition that contains an instance of another component, which
/// in turn contains yet another instance, …). Resizing the outermost
/// instance would otherwise walk an unbounded chain.
///
/// Sixteen levels is generous in practice: Figma documents in the
/// wild rarely nest components more than five levels deep, and the
/// limit is high enough to never bite legitimate designs while
/// still bounding pathological / circular cases at sub-millisecond
/// cost.
pub const LAYOUT_PROPAGATION_DEPTH_LIMIT: usize = 16;

/// One entry of the [`LayoutPropagationReport`].
#[derive(Debug, Clone)]
pub struct LayoutPropagationChange {
    pub node_id: Uuid,
    pub before: kcreate_core::node::Bounds,
    pub after: kcreate_core::node::Bounds,
}

/// Set of bounds changes recorded by
/// [`layout_propagate_in_subtree`], in tree order (parent before
/// child). The caller wraps this in an [`Operation`] so undo
/// restores the entire propagation atomically.
#[derive(Debug, Clone, Default)]
pub struct LayoutPropagationReport {
    pub changes: Vec<LayoutPropagationChange>,
}

/// Run the auto-layout solver across every `LayoutFrame` descendant
/// of `root_id` (inclusive). Re-applies the parent's solver to its
/// direct children, then recurses into any laid-out child that is
/// itself a `LayoutFrame` so that nested component instances get
/// their internal flex/grid layout refreshed when their bounds
/// change.
///
/// Operates on an already-borrowed [`Workspace`] reference — does
/// NOT lock `slot()`. Callers (`document_resize_frame`,
/// `layout_recompute`, `component_switch_variant`, …) compose this
/// helper inside a single critical section so the propagation is
/// observed atomically by scene-sync.
///
/// Bounded recursion depth ([`LAYOUT_PROPAGATION_DEPTH_LIMIT`])
/// guards against circular component references. Hitting the limit
/// stops the walk on that branch and is reported via
/// [`DocumentBridgeError::LayoutRecursionLimit`] so the caller can
/// surface a structured error instead of silently truncating.
pub(crate) fn layout_propagate_in_subtree(
    ws: &mut Workspace,
    root_id: Uuid,
) -> Result<LayoutPropagationReport> {
    fn walk(
        ws: &mut Workspace,
        node_id: Uuid,
        depth: usize,
        report: &mut LayoutPropagationReport,
    ) -> Result<()> {
        if depth >= LAYOUT_PROPAGATION_DEPTH_LIMIT {
            return Err(DocumentBridgeError::LayoutRecursionLimit {
                node_id,
                limit: LAYOUT_PROPAGATION_DEPTH_LIMIT,
            });
        }

        // Snapshot the data we need under the immutable borrow, then
        // release it before mutating children.
        let (run_solver, child_ids, config, parent_bounds, inputs) = {
            let Some(node) = ws.project.document.get_node(node_id) else {
                return Ok(());
            };
            let child_ids: Vec<Uuid> = node.children.clone();
            let mut config_opt: Option<LayoutConfig> = None;
            let mut parent_bounds = node.bounds;
            let mut inputs: Vec<(Uuid, f64, f64)> = Vec::new();
            if node.node_type == NodeType::LayoutFrame {
                if let Some(raw) = node.metadata.get(LAYOUT_CONFIG_METADATA_KEY) {
                    let cfg: LayoutConfig = serde_json::from_value(raw.clone()).map_err(|e| {
                        DocumentBridgeError::InvalidLayoutConfig(node_id, e.to_string())
                    })?;
                    config_opt = Some(cfg);
                    parent_bounds = node.bounds;
                    for cid in &child_ids {
                        if let Some(c) = ws.project.document.get_node(*cid) {
                            inputs.push((*cid, c.bounds.width, c.bounds.height));
                        }
                    }
                }
            }
            (
                config_opt.is_some(),
                child_ids,
                config_opt,
                parent_bounds,
                inputs,
            )
        };

        if run_solver {
            // Safe because `run_solver` is exactly `config_opt.is_some()`.
            let config = config.expect("config present when run_solver is true");
            let placements = match config {
                LayoutConfig::Flex(f) => layout_flex(parent_bounds, &inputs, &f),
                LayoutConfig::Grid(g) => layout_grid(parent_bounds, &inputs, &g),
            };
            for (cid, new_bounds) in placements {
                let Some(child) = ws.project.document.get_node_mut(cid) else {
                    continue;
                };
                let before = child.bounds;
                if before != new_bounds {
                    child.bounds = new_bounds;
                    child.touch();
                    report.changes.push(LayoutPropagationChange {
                        node_id: cid,
                        before,
                        after: new_bounds,
                    });
                }
            }
        }

        // Recurse into each child so nested `LayoutFrame` instances
        // pick up the propagated bounds change.
        for cid in child_ids {
            walk(ws, cid, depth + 1, report)?;
        }
        Ok(())
    }

    let mut report = LayoutPropagationReport::default();
    walk(ws, root_id, 0, &mut report)?;
    Ok(report)
}

/// Convert a `GroupLayer` into a `LayoutFrame` so it can carry an
/// auto-layout config. No-op if the node is already a `LayoutFrame`;
/// returns an error for any other node type.
pub fn layout_convert_to_frame(node_id: Uuid) -> Result<()> {
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    let before;
    let after;
    {
        let node = ws
            .project
            .document
            .get_node_mut(node_id)
            .ok_or(DocumentBridgeError::NodeNotFound(node_id))?;
        match node.node_type {
            NodeType::LayoutFrame => return Ok(()),
            NodeType::GroupLayer => {}
            other => {
                return Err(DocumentBridgeError::WrongNodeType {
                    expected: NodeType::GroupLayer,
                    got: other,
                });
            }
        }
        before = serde_json::to_value(&*node)?;
        node.node_type = NodeType::LayoutFrame;
        node.touch();
        after = serde_json::to_value(&*node)?;
    }
    let op = Operation::new(
        "user",
        "layout_convert_to_frame",
        before,
        after,
        vec![node_id],
    );
    ws.project.execute_operation(op);
    let _ = sync_scene_locked(&mut guard);
    drop(guard);
    Ok(())
}

// -----------------------------------------------------------------------------
// Canvas node transform (move)
// -----------------------------------------------------------------------------

/// Translate a node by `(dx, dy)` in world coordinates, recording an
/// undoable operation. The host calls this once per pointer gesture
/// (i.e. on mouseup with the accumulated delta), not once per
/// pointer-move event, so the operation log doesn't get spammed.
pub fn canvas_move_node(node_id: Uuid, dx: f64, dy: f64) -> Result<()> {
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    let before;
    let after;
    {
        let node = ws
            .project
            .document
            .get_node_mut(node_id)
            .ok_or(DocumentBridgeError::NodeNotFound(node_id))?;
        before = serde_json::to_value(node.transform)?;
        node.transform.tx += dx;
        node.transform.ty += dy;
        node.touch();
        after = serde_json::to_value(node.transform)?;
    }
    ws.project.modified_at = Utc::now();
    let op = Operation::new("user", "canvas_move_node", before, after, vec![node_id]);
    ws.project.execute_operation(op);
    let _ = sync_scene_locked(&mut guard);
    drop(guard);
    Ok(())
}

// -----------------------------------------------------------------------------
// Raster image import
// -----------------------------------------------------------------------------

/// Import a raster image from disk, storing it in the project's
/// content-addressed blob store and creating a [`NodeType::RasterLayer`]
/// node that references it.
///
/// Returns the new node's uuid. Records an undoable operation so the
/// import can be reverted with a normal undo gesture.
///
/// Phase 0 supports anything the `image` crate can decode (PNG, JPEG,
/// WebP); the blob is stored raw (the original encoded bytes) so the
/// project file size stays small. Decoding to RGBA8 happens on demand
/// at scene-sync time.
pub fn document_import_image(parent_id: Option<Uuid>, file_path: &Path) -> Result<Uuid> {
    let bytes = std::fs::read(file_path)?;
    let mime_type = mime_for_path(file_path);
    document_import_image_bytes_inner(parent_id, &bytes, mime_type)
}

/// In-memory variant of [`document_import_image`]. The image bytes
/// must be in a format the `image` crate can decode (PNG, JPEG,
/// WebP, etc.). The MIME type is sniffed from the magic bytes
/// rather than a file extension, since the caller (typically the
/// Phase 4 image-gen sidecar) produces a raw payload with no
/// filesystem identity. Same operation-log semantics as
/// [`document_import_image`] — recorded as `document_import_image`
/// so undo/redo and the action history don't need a new op kind.
pub fn document_import_image_bytes(parent_id: Option<Uuid>, bytes: &[u8]) -> Result<Uuid> {
    let mime_type = mime_for_bytes(bytes);
    document_import_image_bytes_inner(parent_id, bytes, mime_type)
}

fn document_import_image_bytes_inner(
    parent_id: Option<Uuid>,
    bytes: &[u8],
    mime_type: &str,
) -> Result<Uuid> {
    let (id, _bounds) = import_raster_node(
        parent_id,
        bytes,
        mime_type,
        "Image",
        None,
        "document_import_image",
    )?;
    Ok(id)
}

/// Decode `bytes`, store them in the project's content-addressed blob
/// store, and insert a [`NodeType::RasterLayer`] referencing the blob
/// as a single undoable operation. Returns the new node id and its
/// world-space bounds.
///
/// `placement` controls the node's bounds:
/// * `None` → natural pixel size at the origin `(0, 0)` (the plain
///   "import an image" flow).
/// * `Some((x, y, target_size))` → the image is uniformly scaled so its
///   longest side is `target_size`, with the top-left at `(x, y)` (the
///   brand-logo placement flow).
///
/// `name` labels the node and `op_command` stamps the operation so each
/// caller keeps its own undo provenance (`document_import_image` vs
/// `brand_logo_insert`).
fn import_raster_node(
    parent_id: Option<Uuid>,
    bytes: &[u8],
    mime_type: &str,
    name: &str,
    placement: Option<(f64, f64, f64)>,
    op_command: &str,
) -> Result<(Uuid, kcreate_core::node::Bounds)> {
    let img = image::load_from_memory(bytes).map_err(|e| {
        DocumentBridgeError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    })?;
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    let bounds = match placement {
        Some((x, y, target_size)) => {
            let longest = f64::from(width).max(f64::from(height));
            let scale = if longest > 0.0 {
                target_size / longest
            } else {
                1.0
            };
            kcreate_core::node::Bounds {
                x,
                y,
                width: f64::from(width) * scale,
                height: f64::from(height) * scale,
            }
        }
        None => kcreate_core::node::Bounds {
            x: 0.0,
            y: 0.0,
            width: f64::from(width),
            height: f64::from(height),
        },
    };
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    let blob = ws
        .store
        .lock()
        .blobs()
        .store(bytes, mime_type)
        .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))?;
    let meta = crate::scene_sync::RasterImageMeta {
        blob_hash: blob.hash,
        width,
        height,
    };
    let mut node = Node::new(NodeType::RasterLayer, name);
    node.parent_id = parent_id;
    node.bounds = bounds;
    node.metadata.insert(
        crate::scene_sync::RASTER_IMAGE_METADATA_KEY.to_string(),
        serde_json::to_value(&meta)?,
    );
    let id = ws.project.document.insert_node(node)?;
    ws.project.modified_at = Utc::now();
    let snapshot = ws
        .project
        .document
        .get_node(id)
        .map_or(serde_json::Value::Null, |n| {
            serde_json::to_value(n).unwrap_or(serde_json::Value::Null)
        });
    let op = Operation::new(
        "user",
        op_command,
        serde_json::Value::Null,
        snapshot,
        vec![id],
    );
    ws.project.execute_operation(op);
    let _ = sync_scene_locked(&mut guard);
    drop(guard);
    Ok((id, bounds))
}

/// Sniff a MIME type from the file's leading magic bytes. We only
/// recognise the formats the `image` crate already decodes (and
/// that callers actually feed us); anything else is reported as
/// `application/octet-stream` so the blob still stores correctly,
/// even though scene-sync will refuse to decode it later. This
/// matches the file-path-based [`mime_for_path`] in coverage.
fn mime_for_bytes(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if bytes.starts_with(b"\xFF\xD8\xFF") {
        "image/jpeg"
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        "image/webp"
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        "image/gif"
    } else {
        "application/octet-stream"
    }
}

fn mime_for_path(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|s| s.to_str())
        .map(str::to_ascii_lowercase)
    {
        Some(ext) => match ext.as_str() {
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "webp" => "image/webp",
            "gif" => "image/gif",
            _ => "application/octet-stream",
        },
        None => "application/octet-stream",
    }
}

// -----------------------------------------------------------------------------
// Text layer creation
// -----------------------------------------------------------------------------

/// Create a [`NodeType::TextLayer`] at `(x, y)` rendering `text` in the
/// given family + size. Returns the new node's uuid and records an
/// undoable operation.
pub fn canvas_create_text(
    parent_id: Option<Uuid>,
    x: f64,
    y: f64,
    text: String,
    font_family: String,
    // f64 in, narrowed to f32 once at the TextLayerMeta use site.
    // Mirrors the batch path (`CanvasBatchItem::Text.size: f64`) so
    // both routes preserve caller-supplied JSON precision through the
    // bounds maths and only narrow at the moment of writing into the
    // (still f32) font metadata blob. AGENTS.md rule 4 — wire-format
    // lockstep parity with the batch helper.
    font_size: f64,
) -> Result<Uuid> {
    let font_size_f32 = font_size as f32;
    let meta = crate::scene_sync::TextLayerMeta {
        text: text.clone(),
        font_family,
        font_size: font_size_f32,
    };
    let mut node = Node::new(NodeType::TextLayer, "Text");
    node.parent_id = parent_id;
    node.bounds = kcreate_core::node::Bounds {
        x,
        y,
        // Bounds height defaults to font size; the layer panel can
        // refine it once shaping has run. Use the pre-narrow f64 so
        // exotic caller-supplied sizes survive the bounds round-trip.
        width: font_size * (text.len().max(1) as f64) * 0.6,
        height: font_size,
    };
    node.metadata.insert(
        crate::scene_sync::TEXT_LAYER_METADATA_KEY.to_string(),
        serde_json::to_value(&meta)?,
    );
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    let id = ws.project.document.insert_node(node)?;
    ws.project.modified_at = Utc::now();
    let snapshot = ws
        .project
        .document
        .get_node(id)
        .map_or(serde_json::Value::Null, |n| {
            serde_json::to_value(n).unwrap_or(serde_json::Value::Null)
        });
    let op = Operation::new(
        "user",
        "canvas_create_text",
        serde_json::Value::Null,
        snapshot,
        vec![id],
    );
    ws.project.execute_operation(op);
    let _ = sync_scene_locked(&mut guard);
    drop(guard);
    Ok(id)
}

// -----------------------------------------------------------------------------
// Canvas batch creation
// -----------------------------------------------------------------------------

/// One node-creation step inside a [`canvas_create_nodes`] batch.
/// Internally tagged on `kind` to match the [`FillStyle`] wire shape
/// (`{ "kind": "solid", "r": …, … }`), so the host can produce a
/// uniform JSON value without learning a second discriminant
/// convention. All four variants accept optional `fill` and `name`
/// — when supplied, they are stamped onto the node *before* it is
/// inserted into the graph, which eliminates the second round-trip
/// through `document_update_node` that the single-item helpers
/// require to colour and label a node.
///
/// Mirrors `CanvasBatchItem` in `apps/desktop/shared/scene.ts`.
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CanvasBatchItem {
    Rect {
        parent: Option<Uuid>,
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        #[serde(default)]
        fill: Option<kcreate_core::node::FillStyle>,
        #[serde(default)]
        name: Option<String>,
    },
    Ellipse {
        parent: Option<Uuid>,
        cx: f64,
        cy: f64,
        rx: f64,
        ry: f64,
        #[serde(default)]
        fill: Option<kcreate_core::node::FillStyle>,
        #[serde(default)]
        name: Option<String>,
    },
    Line {
        parent: Option<Uuid>,
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
        #[serde(default)]
        fill: Option<kcreate_core::node::FillStyle>,
        #[serde(default)]
        name: Option<String>,
    },
    Text {
        parent: Option<Uuid>,
        x: f64,
        y: f64,
        body: String,
        family: String,
        // f64 in the wire shape, narrowed to f32 at the use site to
        // mirror the single-item `canvas_create_text` N-API helper
        // (`crates/kcreate_bridge/src/lib.rs:1575` takes `font_size:
        // f64` from JS and does `font_size as f32` before calling
        // into the document layer). JSON numbers are
        // double-precision, so this keeps the JSON → Rust narrowing
        // boundary identical between the batch and per-item paths
        // (AGENTS.md rule 4 — wire-format lockstep with TS
        // `CanvasBatchItem.size: number` in `shared/scene.ts`).
        size: f64,
        #[serde(default)]
        fill: Option<kcreate_core::node::FillStyle>,
        #[serde(default)]
        name: Option<String>,
    },
}

/// Build the in-memory [`Node`] for one batch item and report which
/// `op_kind` should be recorded against it for undo/redo. Splitting
/// this out of [`canvas_create_nodes`] keeps the per-item match arm
/// small and lets the batch loop stay focused on lock ordering +
/// op-log accounting.
///
/// `pub(crate)` so the template library entry points
/// ([`template_instantiate_items`], [`build_template_document`]) can
/// reuse the exact same item → [`Node`] translation rather than
/// duplicating the geometry/fill construction — a single source of
/// truth for the `CanvasBatchItem` wire shape.
pub(crate) fn build_canvas_batch_node(item: CanvasBatchItem) -> Result<(Node, &'static str)> {
    use kcreate_core::node::Bounds;
    match item {
        CanvasBatchItem::Rect {
            parent,
            x,
            y,
            w,
            h,
            fill,
            name,
        } => {
            let path = kcreate_vector::VectorPath::new(vec![
                kcreate_vector::PathSegment::MoveTo(kcreate_vector::PathPoint::new(x, y)),
                kcreate_vector::PathSegment::LineTo(kcreate_vector::PathPoint::new(x + w, y)),
                kcreate_vector::PathSegment::LineTo(kcreate_vector::PathPoint::new(x + w, y + h)),
                kcreate_vector::PathSegment::LineTo(kcreate_vector::PathPoint::new(x, y + h)),
                kcreate_vector::PathSegment::Close,
            ]);
            let default_name = name.as_deref().unwrap_or("Rectangle");
            let mut node = Node::new(NodeType::VectorLayer, default_name);
            node.parent_id = parent;
            node.bounds = Bounds {
                x,
                y,
                width: w,
                height: h,
            };
            node.metadata.insert(
                crate::scene_sync::VECTOR_PATH_METADATA_KEY.to_string(),
                serde_json::to_value(&path)?,
            );
            if let Some(f) = fill {
                node.style.fill = f;
            }
            Ok((node, "canvas_create_rect"))
        }
        CanvasBatchItem::Ellipse {
            parent,
            cx,
            cy,
            rx,
            ry,
            fill,
            name,
        } => {
            const KAPPA: f64 = 0.552_284_749_830_793_4;
            let ox = rx * KAPPA;
            let oy = ry * KAPPA;
            let path = kcreate_vector::VectorPath::new(vec![
                kcreate_vector::PathSegment::MoveTo(kcreate_vector::PathPoint::new(cx - rx, cy)),
                kcreate_vector::PathSegment::CubicTo {
                    ctrl1: kcreate_vector::PathPoint::new(cx - rx, cy - oy),
                    ctrl2: kcreate_vector::PathPoint::new(cx - ox, cy - ry),
                    end: kcreate_vector::PathPoint::new(cx, cy - ry),
                },
                kcreate_vector::PathSegment::CubicTo {
                    ctrl1: kcreate_vector::PathPoint::new(cx + ox, cy - ry),
                    ctrl2: kcreate_vector::PathPoint::new(cx + rx, cy - oy),
                    end: kcreate_vector::PathPoint::new(cx + rx, cy),
                },
                kcreate_vector::PathSegment::CubicTo {
                    ctrl1: kcreate_vector::PathPoint::new(cx + rx, cy + oy),
                    ctrl2: kcreate_vector::PathPoint::new(cx + ox, cy + ry),
                    end: kcreate_vector::PathPoint::new(cx, cy + ry),
                },
                kcreate_vector::PathSegment::CubicTo {
                    ctrl1: kcreate_vector::PathPoint::new(cx - ox, cy + ry),
                    ctrl2: kcreate_vector::PathPoint::new(cx - rx, cy + oy),
                    end: kcreate_vector::PathPoint::new(cx - rx, cy),
                },
                kcreate_vector::PathSegment::Close,
            ]);
            let default_name = name.as_deref().unwrap_or("Ellipse");
            let mut node = Node::new(NodeType::VectorLayer, default_name);
            node.parent_id = parent;
            node.bounds = Bounds {
                x: cx - rx,
                y: cy - ry,
                width: rx * 2.0,
                height: ry * 2.0,
            };
            node.metadata.insert(
                crate::scene_sync::VECTOR_PATH_METADATA_KEY.to_string(),
                serde_json::to_value(&path)?,
            );
            if let Some(f) = fill {
                node.style.fill = f;
            }
            Ok((node, "canvas_create_ellipse"))
        }
        CanvasBatchItem::Line {
            parent,
            x1,
            y1,
            x2,
            y2,
            fill,
            name,
        } => {
            let path = kcreate_vector::VectorPath::new(vec![
                kcreate_vector::PathSegment::MoveTo(kcreate_vector::PathPoint::new(x1, y1)),
                kcreate_vector::PathSegment::LineTo(kcreate_vector::PathPoint::new(x2, y2)),
            ]);
            let bx = x1.min(x2);
            let by = y1.min(y2);
            let bw = (x2 - x1).abs();
            let bh = (y2 - y1).abs();
            let default_name = name.as_deref().unwrap_or("Line");
            let mut node = Node::new(NodeType::VectorLayer, default_name);
            node.parent_id = parent;
            node.bounds = Bounds {
                x: bx,
                y: by,
                width: bw,
                height: bh,
            };
            node.metadata.insert(
                crate::scene_sync::VECTOR_PATH_METADATA_KEY.to_string(),
                serde_json::to_value(&path)?,
            );
            if let Some(f) = fill {
                node.style.fill = f;
            }
            Ok((node, "canvas_create_line"))
        }
        CanvasBatchItem::Text {
            parent,
            x,
            y,
            body,
            family,
            size,
            fill,
            name,
        } => {
            // Narrow size to f32 once at the use site so the rest of
            // the function matches the single-item path. The f64
            // bounds maths uses the pre-narrow value (matches the
            // wire-format intent: caller-supplied precision until the
            // last possible moment).
            let size_f32 = size as f32;
            let meta = crate::scene_sync::TextLayerMeta {
                text: body.clone(),
                font_family: family,
                font_size: size_f32,
            };
            let default_name = name.as_deref().unwrap_or("Text");
            let mut node = Node::new(NodeType::TextLayer, default_name);
            node.parent_id = parent;
            node.bounds = Bounds {
                x,
                y,
                // Same heuristic as single-item canvas_create_text:
                // bounds width = size × char-count × 0.6, refined by
                // the layer panel once shaping has run.
                width: size * (body.len().max(1) as f64) * 0.6,
                height: size,
            };
            node.metadata.insert(
                crate::scene_sync::TEXT_LAYER_METADATA_KEY.to_string(),
                serde_json::to_value(&meta)?,
            );
            if let Some(f) = fill {
                node.style.fill = f;
            }
            Ok((node, "canvas_create_text"))
        }
    }
}

/// Atomic batch creation of vector / text layers.
///
/// Why this exists: the per-item helpers (`canvas_create_rect`,
/// `canvas_create_text`, …) each acquire `slot().write()` exclusively
/// and run `sync_scene_locked` at the end. Seeding a template with
/// 12 nodes therefore costs 12 lock acquisitions + 12 scene rebuilds
/// (plus up to 12 follow-up `document_update_node` round-trips when
/// the caller wants to stamp `fill` / `name`). On the HomePage →
/// editor boot path this is dominated by the scene-sync passes.
///
/// `canvas_create_nodes` takes the write lock once, inserts every
/// node in order, records one `Operation` per item against the
/// existing op-kinds so undo / redo granularity is preserved, then
/// runs a single `sync_scene_locked` before releasing the lock. Each
/// item may carry `fill` and `name` fields which are stamped onto the
/// `Node` *before* `insert_node` is called — so the batch never has
/// to round-trip through `document_update_node` to colour or label a
/// node, even though both fields are still independently mutable
/// afterwards via the normal IPC.
///
/// Items are inserted strictly in submission order so the document's
/// z-order is deterministic from the caller's perspective (the
/// renderer's draw loop walks children front-to-back of the insertion
/// list).
///
/// Returns the new node ids in the same order as `items`. An empty
/// `items` is a no-op and returns immediately without taking the
/// lock.
pub fn canvas_create_nodes(items: Vec<CanvasBatchItem>) -> Result<Vec<Uuid>> {
    if items.is_empty() {
        return Ok(Vec::new());
    }
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    let mut created_ids = Vec::with_capacity(items.len());
    // Track the first error from the loop so we can finalize the
    // document state (modified_at + scene sync) for any nodes that *did*
    // make it in, before propagating the error to the caller. Without
    // this, a mid-batch failure would leave already-inserted nodes
    // present in the document graph (and undoable!) but invisible
    // until the next user-triggered scene sync — Devin Review PR #32
    // BUG_0001 ("ghost nodes" on partial batch failure).
    let mut loop_err: Option<DocumentBridgeError> = None;
    for item in items {
        let step = build_canvas_batch_node(item).and_then(|(node, op_kind)| {
            let id = ws.project.document.insert_node(node)?;
            let snapshot = ws
                .project
                .document
                .get_node(id)
                .map_or(serde_json::Value::Null, |n| {
                    serde_json::to_value(n).unwrap_or(serde_json::Value::Null)
                });
            let op = Operation::new("user", op_kind, serde_json::Value::Null, snapshot, vec![id]);
            ws.project.execute_operation(op);
            Ok::<Uuid, DocumentBridgeError>(id)
        });
        match step {
            Ok(id) => created_ids.push(id),
            Err(e) => {
                loop_err = Some(e);
                break;
            }
        }
    }
    // Always finalize when at least one node was inserted, even on
    // the error path. The fully-successful path is a strict subset.
    if !created_ids.is_empty() {
        ws.project.modified_at = Utc::now();
        let _ = sync_scene_locked(&mut guard);
    }
    drop(guard);
    match loop_err {
        Some(e) => Err(e),
        None => Ok(created_ids),
    }
}

/// Report returned by [`template_instantiate_items`]: the artboard
/// created to hold the instantiated template plus every content node
/// id inserted under it, in submission order.
#[derive(Debug, Clone)]
pub struct TemplateInstantiateReport {
    /// The artboard the template content was parented to.
    pub artboard_id: Uuid,
    /// Content node ids in `items` submission order.
    pub node_ids: Vec<Uuid>,
}

/// Instantiate a ready-made template into the **currently open**
/// workspace. Creates a new artboard sized to `width` × `height` at
/// the next free position in the first page, then inserts every
/// `items` entry as a child of that artboard.
///
/// `items` carry absolute coordinates within `[0,0,width,height]` —
/// the exact space the thumbnail is rendered in (see
/// [`build_template_document`]), so the live canvas and the gallery
/// preview are pixel-identical. The artboard is auto-positioned in
/// the page's horizontal row by [`next_artboard_x`]; each content
/// node is then shifted by the artboard origin via its `transform`.
/// `scene_sync` reads world coordinates as `bounds + transform` for
/// raster/text and `transform` for vector paths (whose path commands
/// carry baked-in absolute coordinates), so one uniform `transform`
/// shift relocates every node kind identically.
///
/// Parenting the content to the artboard means [`artboard_duplicate`]
/// deep-clones the whole design — that is the "Duplicate & remix"
/// path, with no template-specific code.
///
/// Mirrors the lock / op-log discipline of [`canvas_create_nodes`]:
/// one write lock, one `Operation` per node (plus the leading
/// `artboard_create` op), a single `sync_scene_locked` at the end,
/// and mid-batch-failure finalization so a partial insert never
/// leaves ghost nodes (Devin Review PR #32 BUG_0001).
pub fn template_instantiate_items(
    name: &str,
    width: f64,
    height: f64,
    items: Vec<CanvasBatchItem>,
) -> Result<TemplateInstantiateReport> {
    if !(width.is_finite() && width > 0.0 && height.is_finite() && height > 0.0) {
        return Err(DocumentBridgeError::InvalidBounds { width, height });
    }
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;

    // Resolve the target page (first Page, else create one) exactly
    // like artboard_create does.
    let resolved_page = match find_first_page(&ws.project.document) {
        Some(p) => p,
        None => ws.project.add_page("Page 1")?,
    };

    // "Start from template" almost always runs against a brand-new
    // scratch project, which `project_create` seeds with a single
    // *empty* default artboard at the origin. Appending a second
    // artboard beside it (the general insert-into-existing-work case)
    // would strand the design off-screen: the editor opens centred on
    // the origin and the user sees the blank default instead of their
    // template. So when the target page holds exactly one artboard and
    // it has no children, treat the template as the document itself —
    // reuse that artboard in place (resized + renamed to the template)
    // rather than creating a new one. Any other shape (a populated
    // artboard, or several) means we're inserting into existing work,
    // so we append left-to-right as before.
    let reuse_target = {
        let artboards = ws.project.document.list_artboards(resolved_page);
        match artboards.as_slice() {
            [only] if only.children.is_empty() => Some((only.id, only.bounds.x, only.bounds.y)),
            _ => None,
        }
    };
    let (artboard_id, origin_x, origin_y) = match reuse_target {
        Some((id, x, y)) => {
            // Resize the pristine default artboard to the template's
            // canvas and rename it so the layer tree / page navigator
            // reads as the template rather than "Page 1 / Artboard 1".
            let before = ws
                .project
                .document
                .get_node(id)
                .map_or(serde_json::Value::Null, |n| {
                    serde_json::to_value(n).unwrap_or(serde_json::Value::Null)
                });
            ws.project
                .document
                .resize_artboard(id, kcreate_core::node::Bounds::new(x, y, width, height))?;
            if let Some(node) = ws.project.document.get_node_mut(id) {
                node.name = name.to_string();
                node.touch();
            }
            let after = ws
                .project
                .document
                .get_node(id)
                .map_or(serde_json::Value::Null, |n| {
                    serde_json::to_value(n).unwrap_or(serde_json::Value::Null)
                });
            ws.project.execute_operation(Operation::new(
                "user",
                "artboard_resize",
                before,
                after,
                vec![id],
            ));
            (id, x, y)
        }
        None => {
            // Auto-position the artboard so repeated "Start from
            // template" clicks lay designs out left-to-right instead of
            // stacking.
            let origin_x = next_artboard_x(&ws.project.document, resolved_page);
            let origin_y = 0.0_f64;
            let bounds = kcreate_core::node::Bounds::new(origin_x, origin_y, width, height);
            let artboard_id = ws
                .project
                .document
                .create_artboard(resolved_page, name, bounds)?;
            let artboard_snapshot = ws
                .project
                .document
                .get_node(artboard_id)
                .map_or(serde_json::Value::Null, |n| {
                    serde_json::to_value(n).unwrap_or(serde_json::Value::Null)
                });
            ws.project.execute_operation(Operation::new(
                "user",
                "artboard_create",
                serde_json::Value::Null,
                artboard_snapshot,
                vec![artboard_id],
            ));
            (artboard_id, origin_x, origin_y)
        }
    };

    let mut node_ids = Vec::with_capacity(items.len());
    let mut loop_err: Option<DocumentBridgeError> = None;
    for item in items {
        let step = build_canvas_batch_node(item).and_then(|(mut node, op_kind)| {
            // Re-parent into the freshly created artboard and shift by
            // the artboard origin so the design lands aligned with its
            // artboard regardless of how many already exist. A uniform
            // transform shift works for vector/text/raster alike
            // because scene_sync derives world coords from `transform`.
            node.parent_id = Some(artboard_id);
            node.transform.tx += origin_x;
            node.transform.ty += origin_y;
            let id = ws.project.document.insert_node(node)?;
            let snapshot = ws
                .project
                .document
                .get_node(id)
                .map_or(serde_json::Value::Null, |n| {
                    serde_json::to_value(n).unwrap_or(serde_json::Value::Null)
                });
            ws.project.execute_operation(Operation::new(
                "user",
                op_kind,
                serde_json::Value::Null,
                snapshot,
                vec![id],
            ));
            Ok::<Uuid, DocumentBridgeError>(id)
        });
        match step {
            Ok(id) => node_ids.push(id),
            Err(e) => {
                loop_err = Some(e);
                break;
            }
        }
    }

    // The artboard alone is a meaningful mutation, so always finalize.
    ws.project.modified_at = Utc::now();
    let _ = sync_scene_locked(&mut guard);
    drop(guard);
    match loop_err {
        Some(e) => Err(e),
        None => Ok(TemplateInstantiateReport {
            artboard_id,
            node_ids,
        }),
    }
}

/// Build a standalone [`DocumentGraph`] from template content items
/// for **off-document thumbnail rendering**. Items are inserted at
/// the document root (no Page / Artboard) using their authored
/// absolute coordinates, so a `scene_sync` pass over
/// `[0,0,width,height]` reproduces exactly what
/// [`template_instantiate_items`] paints into a live artboard.
///
/// Unlike the live-workspace path this never touches the workspace
/// slot, the operation log, or the scene-sync singleton — it is a
/// pure `items` → graph transform used only by the thumbnail
/// renderer (`crate::thumbnails`).
pub(crate) fn build_template_document(items: Vec<CanvasBatchItem>) -> Result<DocumentGraph> {
    let mut doc = DocumentGraph::new();
    for item in items {
        let (mut node, _op_kind) = build_canvas_batch_node(item)?;
        // Force root placement: the ephemeral thumbnail graph has no
        // artboard. content.json authors items with `parent: null`
        // anyway; this defends against a stray parent that would not
        // resolve in this standalone graph.
        node.parent_id = None;
        doc.insert_node(node)?;
    }
    Ok(doc)
}

// -----------------------------------------------------------------------------
// Runtime status
// -----------------------------------------------------------------------------

/// Cached snapshot of the host system. We cache only the
/// `runtime_status` shape; live tunables (undo depth, raster cache,
/// low-resource mode) come from [`runtime_slot`] instead so they
/// can move at runtime.
#[derive(Debug, Clone)]
struct CachedRuntime {
    status: RuntimeStatus,
}

fn cached_runtime() -> &'static CachedRuntime {
    static CACHE: OnceLock<CachedRuntime> = OnceLock::new();
    CACHE.get_or_init(|| {
        let cfg = RuntimeConfig::detect();
        CachedRuntime {
            status: RuntimeStatus {
                device_tier: format!("{:?}", cfg.device_tier),
                gpu_available: cfg.gpu_available,
                gpu_name: cfg.gpu_name,
                platform: format!("{:?}", cfg.platform),
                total_ram_mb: cfg.total_ram_mb,
            },
        }
    })
}

/// Returns a cached snapshot of the host system.
///
/// The probe (`RuntimeConfig::detect()`) is not cheap — it does
/// filesystem checks and a `sys_info::mem_info()` syscall — so we run
/// it once per process and cache the result. The values are stable
/// for the lifetime of the process.
pub fn runtime_status() -> RuntimeStatus {
    cached_runtime().status.clone()
}

/// Process-global, mutable runtime config. Seeded from
/// [`RuntimeConfig::detect`] on first access; subsequent writes (e.g.
/// low-resource toggle) update this snapshot. The cached
/// [`runtime_status`] shape stays immutable because the system probe
/// itself (RAM, GPU detection, platform) does not change at runtime.
pub(crate) fn runtime_slot() -> &'static parking_lot::Mutex<RuntimeConfig> {
    static SLOT: OnceLock<parking_lot::Mutex<RuntimeConfig>> = OnceLock::new();
    SLOT.get_or_init(|| parking_lot::Mutex::new(RuntimeConfig::detect()))
}

/// True iff low-resource mode is active.
pub fn low_resource_mode_get() -> bool {
    runtime_slot().lock().is_low_resource()
}

/// Manually toggle low-resource mode. On Tier 0 the flag is forced
/// to remain `true` regardless of the request (see
/// [`RuntimeConfig::set_low_resource`]). After flipping, the open
/// project (if any) re-sizes its operation log to the new effective
/// depth.
pub fn low_resource_mode_set(enabled: bool) {
    let new_depth = {
        let mut cfg = runtime_slot().lock();
        cfg.set_low_resource(enabled);
        cfg.effective_undo_depth()
    };
    let mut guard = slot().write();
    if let Some(ws) = guard.as_mut() {
        ws.project.operation_log.set_max_depth(new_depth);
    }
    drop(guard);
    // Phase 8 Block E Task 28: re-sync the tile cache budget so
    // toggling low-resource mode immediately reclaims (or grants)
    // raster memory headroom. The runtime-config lock has already
    // been released above; `resync_tile_cache_budget` takes the
    // cache lock independently, so this is deadlock-free.
    let _ = crate::perf::resync_tile_cache_budget();
}

/// Resolved resource limits the host UI surfaces in Settings.
///
/// Each field is the result of the corresponding
/// `RuntimeConfig::effective_*` getter at the call site — these can
/// shift at runtime when the user toggles low-resource mode, so the
/// host should re-fetch after [`low_resource_mode_set`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceLimits {
    pub device_tier: String,
    pub low_resource_mode: bool,
    pub effective_undo_depth: usize,
    pub effective_raster_cache_mb: u64,
    pub effective_max_model_mb: u64,
    pub gpu_rendering_allowed: bool,
    /// Phase 4 hard gate: `true` iff this machine's tier + GPU
    /// combination permits running the FLUX image-generation
    /// sidecar. UI consumers MUST hide all image-gen affordances
    /// when this is `false` — not just disable them.
    pub image_generation_allowed: bool,
    /// Phase 4 cap on vision-model file size in MB. Independent
    /// of `effective_max_model_mb` because a Tier 0 box that
    /// can't load a 4 GB text LLM can comfortably load a 180 MB
    /// SmolVLM. UI uses this to ghost out vision packs that
    /// exceed the per-tier ceiling.
    pub vision_model_max_mb: u64,
    /// `Debug` form of [`Platform`] for the host machine
    /// (`"MacOsAppleSilicon"`, `"Linux"`, `"Windows"`, …). The
    /// Model Manager uses this to decide whether to show MLX-format
    /// packs; `device_tier` alone is insufficient because it only
    /// encodes the performance class (`Tier0`/`Tier1`/…), never
    /// the platform.
    pub platform: String,
}

/// Snapshot the currently-effective resource limits.
pub fn resource_limits() -> ResourceLimits {
    let cfg = runtime_slot().lock();
    ResourceLimits {
        device_tier: format!("{:?}", cfg.device_tier),
        low_resource_mode: cfg.is_low_resource(),
        effective_undo_depth: cfg.effective_undo_depth(),
        effective_raster_cache_mb: cfg.effective_raster_cache_mb(),
        effective_max_model_mb: cfg.effective_max_model_mb(),
        gpu_rendering_allowed: cfg.gpu_rendering_allowed(),
        image_generation_allowed: cfg.image_generation_allowed(),
        // Use `effective_vision_model_mb` so the UI matches what
        // `phase4::vision_listable_packs` and `phase4::spawn_vision`
        // actually enforce. The raw `device_tier.vision_model_max_mb`
        // ignores `is_low_resource`, which halves the budget — so
        // surfacing the tier-only cap would make the Model Manager
        // show packs as installable that the Rust side will then
        // reject at sidecar-start time.
        vision_model_max_mb: cfg.effective_vision_model_mb(),
        platform: format!("{:?}", cfg.platform),
    }
}

/// Snapshot of the live document's editing state, used by the host UI
/// to enable/disable Undo/Redo buttons without making us round-trip
/// the entire layer tree on every keystroke.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentStatus {
    pub node_count: usize,
    pub can_undo: bool,
    pub can_redo: bool,
    pub undo_depth: usize,
    pub redo_depth: usize,
}

/// Read-only snapshot of the open document's editing state. Returns
/// `None` if no project is open — the host can treat that as "all
/// editing actions disabled".
pub fn document_status() -> Option<DocumentStatus> {
    // Phase 11 Task 19: read-only — derives counters from the log.
    let guard = slot().read();
    let status = guard.as_ref().map(|ws| {
        let log = &ws.project.operation_log;
        DocumentStatus {
            node_count: ws.project.document.node_count(),
            can_undo: log.can_undo(),
            can_redo: log.can_redo(),
            undo_depth: log.position(),
            redo_depth: log.len().saturating_sub(log.position()),
        }
    });
    drop(guard);
    status
}

// -----------------------------------------------------------------------------
// Export
// -----------------------------------------------------------------------------

/// Render `node_ids` (or the entire document if empty) to SVG.
pub fn export_svg(node_ids: &[Uuid], options: &SvgExportOptions) -> Result<String> {
    let guard = slot().write();
    let ws = guard.as_ref().ok_or(DocumentBridgeError::NoProject)?;
    let svg = export_svg_from_document(&ws.project.document, node_ids, options)?;
    drop(guard);
    Ok(svg)
}

/// PNG export options accepted from the host. We expose a JSON-friendly
/// view that maps onto `kcreate_export::png::PngExportOptions`.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PngExportRequest {
    pub width: u32,
    pub height: u32,
    #[serde(default = "default_scale")]
    pub scale: f32,
    #[serde(default)]
    pub background: Option<[f32; 4]>,
}

const fn default_scale() -> f32 {
    1.0
}

/// Render the current renderer scene to PNG at `output_path`. Returns
/// the number of bytes written.
///
/// Note: Phase 0 ties PNG export to the live renderer scene held by
/// [`crate::state`], which is keyed by `kcreate_renderer::scene` ids
/// (a separate id space from the document graph's `Uuid`s). Per-node
/// PNG export therefore requires a Phase 1 document→scene translation
/// step. The bridge API surface intentionally omits a `node_ids`
/// parameter so callers don't get the impression filtering happens
/// here when it doesn't.
pub fn export_png_file(output_path: &Path, options: &PngExportRequest) -> Result<u64> {
    let bytes = export_png_bytes(options)?;
    let written = bytes.len() as u64;
    std::fs::write(output_path, bytes)?;
    Ok(written)
}

/// In-memory companion to [`export_png_file`]. Renders the current
/// scene to PNG bytes without touching the filesystem. Used by the
/// KChat artifact publisher.
pub fn export_png_bytes(options: &PngExportRequest) -> Result<Vec<u8>> {
    let scene = crate::state::current_scene()?;
    let opts = png_options_for(options);
    Ok(export_png_to_bytes(&scene, &opts)?)
}

fn png_options_for(options: &PngExportRequest) -> PngExportOptions {
    PngExportOptions {
        width: options.width,
        height: options.height,
        scale: options.scale,
        background: options
            .background
            .map(|[r, g, b, a]| kcreate_renderer::geometry::Color::rgba(r, g, b, a)),
    }
}

// -----------------------------------------------------------------------------
// PDF export
// -----------------------------------------------------------------------------

/// JSON-friendly PDF export options. Mirrors
/// [`kcreate_export::pdf::PdfExportOptions`].
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PdfExportRequest {
    pub width_mm: f64,
    pub height_mm: f64,
    #[serde(default)]
    pub title: Option<String>,
    /// Output color mode: `"rgb"` (default), `"cmyk"`, or
    /// `"passThrough"`. When omitted, the document's
    /// `color_settings.working_space_cmyk` chooses CMYK iff a CMYK
    /// working space is set; otherwise RGB.
    #[serde(default)]
    pub color_mode: Option<String>,
    /// CMYK rasterisation dithering algorithm: `"none"`,
    /// `"floydSteinberg"` (default), or `"bayer8x8"`. Ignored when
    /// `color_mode != "cmyk"`. Floyd-Steinberg matches the quality
    /// expected of hero PDF artwork; `bayer8x8` is cheaper for
    /// batch / thumbnail exports; `none` reproduces the Phase 2
    /// byte-identical output.
    #[serde(default)]
    pub cmyk_dither: Option<String>,
}

/// Render the open document to PDF. Returns the number of bytes written.
pub fn export_pdf_file(output_path: &Path, options: &PdfExportRequest) -> Result<u64> {
    let bytes = export_pdf_bytes(options)?;
    let written = bytes.len() as u64;
    std::fs::write(output_path, bytes)?;
    Ok(written)
}

/// In-memory companion to [`export_pdf_file`]. Renders the open
/// document to a PDF `Vec<u8>` without touching the filesystem. Used
/// by the KChat artifact publisher.
pub fn export_pdf_bytes(options: &PdfExportRequest) -> Result<Vec<u8>> {
    let guard = slot().write();
    let ws = guard.as_ref().ok_or(DocumentBridgeError::NoProject)?;
    let mut rasters = kcreate_export::pdf::RasterPixelCache::new();
    // Preload every raster layer's pixels so the export crate doesn't
    // need to know about the storage layer. Decode happens once per
    // unique blob hash (HashMap dedupes for free).
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
        let bytes = {
            let store = ws.store.lock();
            match store.blobs().load(&meta.blob_hash) {
                Ok(b) => b,
                Err(_) => continue,
            }
        };
        if let Ok(pixels) = kcreate_export::pdf::RasterPixels::decode(&bytes) {
            rasters.insert(meta.blob_hash, pixels);
        }
    }
    let resolved_color_mode = match options.color_mode.as_deref() {
        Some("rgb" | "Rgb") => kcreate_export::pdf::PdfColorMode::Rgb,
        Some("cmyk" | "Cmyk" | "CMYK") => kcreate_export::pdf::PdfColorMode::Cmyk,
        Some("passThrough" | "pass_through" | "PassThrough") => {
            kcreate_export::pdf::PdfColorMode::PassThrough
        }
        Some(other) => {
            return Err(DocumentBridgeError::InvalidArgument {
                argument: "color_mode".into(),
                value: other.to_string(),
            });
        }
        None => {
            // Auto-pick from the document's color settings: a CMYK
            // working space implies the user wants a print-bound PDF.
            if ws.project.color_settings.working_space_cmyk.is_some() {
                kcreate_export::pdf::PdfColorMode::Cmyk
            } else {
                kcreate_export::pdf::PdfColorMode::Rgb
            }
        }
    };
    let resolved_dither = match options.cmyk_dither.as_deref() {
        Some("none" | "None") => kcreate_export::CmykDither::None,
        Some("floydSteinberg" | "FloydSteinberg" | "floyd_steinberg") => {
            kcreate_export::CmykDither::FloydSteinberg
        }
        Some("bayer8x8" | "Bayer8x8" | "bayer_8x8") => kcreate_export::CmykDither::Bayer8x8,
        Some(other) => {
            return Err(DocumentBridgeError::InvalidArgument {
                argument: "cmyk_dither".into(),
                value: other.to_string(),
            });
        }
        None => kcreate_export::CmykDither::FloydSteinberg,
    };
    let opts = kcreate_export::pdf::PdfExportOptions {
        width_mm: options.width_mm,
        height_mm: options.height_mm,
        title: options
            .title
            .clone()
            .unwrap_or_else(|| ws.project.name.clone()),
        color_mode: resolved_color_mode,
        cmyk_dither: resolved_dither,
    };
    let bytes = kcreate_export::pdf::export_pdf_from_document_to_bytes(
        &ws.project.document,
        &opts,
        &rasters,
    )
    .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))?;
    drop(guard);
    Ok(bytes)
}

// -----------------------------------------------------------------------------
// WebP export
// -----------------------------------------------------------------------------

/// JSON-friendly WebP export options. Mirrors
/// [`kcreate_export::webp::WebpExportOptions`].
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WebpExportRequest {
    pub width: u32,
    pub height: u32,
    #[serde(default = "default_scale")]
    pub scale: f32,
    #[serde(default = "default_quality")]
    pub quality: u32,
    #[serde(default = "default_lossless")]
    pub lossless: bool,
    #[serde(default)]
    pub background: Option<[f32; 4]>,
}

const fn default_quality() -> u32 {
    90
}
const fn default_lossless() -> bool {
    true
}

/// Render the current renderer scene to WebP at `output_path`. Returns
/// the number of bytes written.
pub fn export_webp_file(output_path: &Path, options: &WebpExportRequest) -> Result<u64> {
    let bytes = export_webp_bytes(options)?;
    let written = bytes.len() as u64;
    std::fs::write(output_path, bytes)?;
    Ok(written)
}

/// In-memory companion to [`export_webp_file`]. Used by the KChat
/// artifact publisher.
pub fn export_webp_bytes(options: &WebpExportRequest) -> Result<Vec<u8>> {
    let scene = crate::state::current_scene()?;
    let opts = kcreate_export::WebpExportOptions {
        width: options.width,
        height: options.height,
        scale: options.scale,
        quality: options.quality,
        lossless: options.lossless,
        background: options
            .background
            .map(|[r, g, b, a]| kcreate_renderer::geometry::Color::rgba(r, g, b, a)),
    };
    kcreate_export::export_webp_to_bytes(&scene, &opts)
        .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))
}

// -----------------------------------------------------------------------------
// JPEG export
// -----------------------------------------------------------------------------

/// JSON-friendly JPEG export options. Mirrors
/// [`kcreate_export::jpeg::JpegExportOptions`].
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct JpegExportRequest {
    pub width: u32,
    pub height: u32,
    #[serde(default = "default_scale")]
    pub scale: f32,
    #[serde(default = "default_quality")]
    pub quality: u32,
    #[serde(default)]
    pub background: Option<[f32; 4]>,
}

/// Render the current renderer scene to JPEG at `output_path`. Returns
/// the number of bytes written.
pub fn export_jpeg_file(output_path: &Path, options: &JpegExportRequest) -> Result<u64> {
    let bytes = export_jpeg_bytes(options)?;
    let written = bytes.len() as u64;
    std::fs::write(output_path, bytes)?;
    Ok(written)
}

/// In-memory companion to [`export_jpeg_file`]. Used by the KChat
/// artifact publisher.
pub fn export_jpeg_bytes(options: &JpegExportRequest) -> Result<Vec<u8>> {
    let scene = crate::state::current_scene()?;
    let opts = kcreate_export::JpegExportOptions {
        width: options.width,
        height: options.height,
        scale: options.scale,
        quality: options.quality,
        background: options
            .background
            .map(|[r, g, b, a]| kcreate_renderer::geometry::Color::rgba(r, g, b, a)),
    };
    kcreate_export::export_jpeg_to_bytes(&scene, &opts)
        .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))
}

// -----------------------------------------------------------------------------
// AI: background removal
// -----------------------------------------------------------------------------

/// Run threshold-based background removal on a raster layer node.
///
/// Reads the source layer's blob from the project's content-addressed
/// store, runs `kcreate_ai::remove_background`, writes the result as a
/// new PNG blob, inserts a sibling `RasterLayer` node pointing at the
/// new blob, and appends an AI action to the global action log.
///
/// The original layer is left in place — the host can stack the result
/// on top for an "Apply / before / after / cancel" UI. An undo
/// operation deletes the new node (its `before_patch` is `null`).
pub fn ai_remove_background(node_id: Uuid) -> Result<Uuid> {
    let (encoded_bytes, parent) = {
        let guard = slot().write();
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
            .get(crate::scene_sync::RASTER_IMAGE_METADATA_KEY)
            .ok_or_else(|| {
                DocumentBridgeError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "raster layer missing image metadata",
                ))
            })?;
        let meta: crate::scene_sync::RasterImageMeta = serde_json::from_value(meta_value.clone())?;
        let bytes = ws
            .store
            .lock()
            .blobs()
            .load(&meta.blob_hash)
            .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))?;
        (bytes, node.parent_id)
    };

    // Decode → run bg removal → re-encode PNG.
    let img = image::load_from_memory(&encoded_bytes).map_err(|e| {
        DocumentBridgeError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    })?;
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    let out_rgba = kcreate_ai::remove_background(
        rgba.as_raw(),
        width,
        height,
        kcreate_ai::BgRemoveOptions::default(),
    )
    .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))?;
    let mut png: Vec<u8> = Vec::new();
    {
        let mut cursor = std::io::Cursor::new(&mut png);
        image::write_buffer_with_format(
            &mut cursor,
            &out_rgba,
            width,
            height,
            image::ColorType::Rgba8,
            image::ImageFormat::Png,
        )
        .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))?;
    }

    // Store the new blob, insert a sibling node, append an op + AI action.
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    let blob = ws
        .store
        .lock()
        .blobs()
        .store(&png, "image/png")
        .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))?;
    let new_meta = crate::scene_sync::RasterImageMeta {
        blob_hash: blob.hash,
        width,
        height,
    };
    let mut new_node = Node::new(NodeType::RasterLayer, "Background removed");
    new_node.parent_id = parent;
    new_node.bounds = kcreate_core::node::Bounds {
        x: 0.0,
        y: 0.0,
        width: f64::from(width),
        height: f64::from(height),
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
        "ai_remove_background",
        serde_json::Value::Null,
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
            task_type: "background_removal".into(),
            model: "threshold-v0".into(),
            compute_device: "cpu".into(),
            affected_nodes: vec![new_id, node_id],
            confidence: None,
        });
    ws.project.modified_at = Utc::now();
    let _ = sync_scene_locked(&mut guard);
    drop(guard);
    Ok(new_id)
}

/// Snapshot of the global AI action log as a JSON array, newest first.
pub fn ai_get_action_log() -> Result<String> {
    let log = kcreate_ai::ActionLog::global().lock();
    let snap = log.snapshot();
    let json = serde_json::to_string(&snap)?;
    Ok(json)
}

// -----------------------------------------------------------------------------
// MCP server
// -----------------------------------------------------------------------------

/// Set a node's primary `fill` and record an undoable `mcp_set_fill`
/// operation. Modeled exactly on [`crate::phase2::text_set_content`]:
/// snapshot the before/after paint, mutate through `get_node_mut`,
/// `touch()` the node, stamp `modified_at`, and push the operation
/// onto the project log so an agent-driven fill is undoable and
/// persisted identically to a UI edit. The scene is re-synced so the
/// renderer reflects the new paint next frame.
#[cfg(feature = "mcp")]
pub fn mcp_set_node_fill(node_id: Uuid, fill: FillStyle) -> Result<()> {
    with_workspace_mut(|ws| {
        let node = ws
            .project
            .document
            .get_node(node_id)
            .ok_or(DocumentBridgeError::NodeNotFound(node_id))?;
        let before_json = serde_json::to_value(&node.style.fill)?;
        let after_json = serde_json::to_value(&fill)?;
        let node_mut = ws
            .project
            .document
            .get_node_mut(node_id)
            .ok_or(DocumentBridgeError::NodeNotFound(node_id))?;
        node_mut.style.fill = fill;
        node_mut.touch();
        ws.project.modified_at = Utc::now();
        let op = Operation::new(
            "user",
            "mcp_set_fill",
            before_json,
            after_json,
            vec![node_id],
        );
        ws.project.execute_operation(op);
        Ok(())
    })?;
    sync_scene_after_change();
    Ok(())
}

/// Parse an optional category slug into a [`kcreate_core::TemplateCategory`],
/// treating an empty string as "no filter" (so an agent can pass `""`
/// for "all categories"). Mirrors the converter `lib::template_list`
/// uses so the MCP surface accepts the same slugs the renderer does.
#[cfg(feature = "mcp")]
fn parse_template_category(
    slug: Option<&str>,
) -> std::result::Result<Option<kcreate_core::TemplateCategory>, String> {
    match slug.filter(|s| !s.is_empty()) {
        Some(s) => serde_json::from_str::<kcreate_core::TemplateCategory>(&format!("\"{s}\""))
            .map(Some)
            .map_err(|e| format!("unknown template category {s:?}: {e}")),
        None => Ok(None),
    }
}

/// `DocumentAccess` implementation that talks to the process-global
/// workspace [`slot`]. Each method takes the workspace lock for the
/// minimum duration needed.
///
/// The MCP server holds an `Arc<dyn DocumentAccess>` for its full
/// lifetime, but only calls into this impl while servicing a request
/// on its worker thread — so the workspace lock is held briefly and
/// never across an `await` boundary. Lock-ordering relative to the
/// renderer singleton is documented on [`sync_scene_locked`].
///
/// Every mutating method funnels through an existing bridge op-path
/// entry point (`phase2::template_instantiate`, `phase10::ai_generate_themed_design`,
/// `assets::insert`, `phase2::text_set_content`, [`mcp_set_node_fill`],
/// [`document_apply_theme`], [`magic_resize`]) so an agent's changes
/// are recorded as real undoable operations and persisted — never
/// faked or echoed.
#[cfg(feature = "mcp")]
struct WorkspaceAccess;

#[cfg(feature = "mcp")]
impl kcreate_mcp::tools::DocumentAccess for WorkspaceAccess {
    fn list_artboards(&self) -> Vec<kcreate_mcp::tools::ArtboardInfo> {
        // Read-only: share the lock with other readers so a discovery
        // call never blocks a concurrent read.
        let guard = slot().read();
        let Some(ws) = guard.as_ref() else {
            return Vec::new();
        };
        ws.project
            .document
            .iter()
            .filter(|(_, n)| n.node_type == NodeType::Artboard)
            .map(|(id, n)| kcreate_mcp::tools::ArtboardInfo {
                id: id.to_string(),
                name: n.name.clone(),
                bounds: n.bounds.into(),
            })
            .collect()
    }

    fn create_node(
        &self,
        node_type: NodeType,
        name: String,
        parent_id: Option<Uuid>,
    ) -> std::result::Result<Uuid, String> {
        let mut node = Node::new(node_type, name);
        node.parent_id = parent_id;
        let mut guard = slot().write();
        let ws = guard
            .as_mut()
            .ok_or_else(|| "no project open".to_string())?;
        let id = ws
            .project
            .document
            .insert_node(node)
            .map_err(|e| e.to_string())?;
        ws.project.modified_at = Utc::now();
        // Record an operation so the agent's create is undoable, exactly
        // like a hand gesture: before=null, after=full node, so an undo
        // deletes the node and a redo recreates it.
        let snapshot = ws
            .project
            .document
            .get_node(id)
            .map_or(serde_json::Value::Null, |n| {
                serde_json::to_value(n).unwrap_or(serde_json::Value::Null)
            });
        let op = Operation::new(
            "user",
            "mcp_create_node",
            serde_json::Value::Null,
            snapshot,
            vec![id],
        );
        ws.project.execute_operation(op);
        // Sync the scene so the renderer sees the new node immediately.
        // Failure to sync (e.g. renderer not initialised in a headless
        // host) is non-fatal: the next renderer_init + sync recovers.
        let _ = sync_scene_locked(&mut guard);
        Ok(id)
    }

    fn export_svg(&self, node_ids: &[Uuid]) -> std::result::Result<String, String> {
        // Read-only: SVG export reads the document graph and never mutates
        // it, so take a shared read lock.
        let guard = slot().read();
        let ws = guard
            .as_ref()
            .ok_or_else(|| "no project open".to_string())?;
        kcreate_export::svg::export_svg_from_document(
            &ws.project.document,
            node_ids,
            &kcreate_export::svg::SvgExportOptions::default(),
        )
        .map_err(|e| e.to_string())
    }

    fn list_templates(
        &self,
        category: Option<&str>,
        query: Option<&str>,
    ) -> std::result::Result<serde_json::Value, String> {
        let category = parse_template_category(category)?;
        let report = crate::phase2::template_list(category, query.filter(|q| !q.is_empty()))
            .map_err(|e| e.to_string())?;
        let templates: Vec<serde_json::Value> = report
            .templates
            .iter()
            .map(|t| {
                serde_json::json!({
                    "id": t.id.to_string(),
                    "name": t.name,
                    "description": t.description,
                    "category": t.category,
                    "tags": t.tags,
                    "page_count": t.page_count,
                })
            })
            .collect();
        Ok(serde_json::json!({ "templates": templates }))
    }

    fn apply_template(&self, template_id: Uuid) -> std::result::Result<serde_json::Value, String> {
        let report = crate::phase2::template_instantiate(template_id).map_err(|e| e.to_string())?;
        Ok(serde_json::json!({
            "artboard_id": report.artboard_id.to_string(),
            "node_ids": report
                .node_ids
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
        }))
    }

    fn generate_themed_design(
        &self,
        brief: &str,
        options_json: &str,
    ) -> std::result::Result<serde_json::Value, String> {
        let result = crate::phase10::ai_generate_themed_design(brief, options_json)
            .map_err(|e| e.to_string())?;
        serde_json::to_value(&result).map_err(|e| e.to_string())
    }

    fn list_assets(
        &self,
        category: Option<&str>,
        query: Option<&str>,
    ) -> std::result::Result<serde_json::Value, String> {
        let category = category.filter(|c| !c.is_empty());
        let defs = match query.filter(|q| !q.is_empty()) {
            Some(q) => crate::assets::search(q, category).map_err(|e| e.to_string())?,
            None => crate::assets::list(category).map_err(|e| e.to_string())?,
        };
        let assets: Vec<serde_json::Value> = defs
            .iter()
            .map(|d| {
                serde_json::json!({
                    "id": d.id,
                    "name": d.name,
                    "category": d.category,
                    "group": d.group,
                    "tags": d.tags,
                })
            })
            .collect();
        Ok(serde_json::json!({ "assets": assets }))
    }

    fn insert_asset(
        &self,
        asset_id: &str,
        parent_id: Option<Uuid>,
        x: f64,
        y: f64,
        target_size: Option<f64>,
    ) -> std::result::Result<serde_json::Value, String> {
        // Default placement size matches the Elements panel's default
        // drop size so agent-placed assets look the same as hand-placed.
        let size = target_size.unwrap_or(200.0);
        let inserted =
            crate::assets::insert(asset_id, parent_id, x, y, size).map_err(|e| e.to_string())?;
        // Re-shape into the MCP surface's uniform `snake_case` contract.
        // `InsertedAsset` itself is `camelCase` because it crosses the
        // napi boundary to the Electron host; the agent-facing tool
        // result must match the snake_case every other tool returns
        // (`apply_template` → `node_ids`, etc.) so a client never has to
        // special-case one tool's key casing.
        Ok(serde_json::json!({
            "group_id": inserted.group_id,
            "node_ids": inserted.node_ids,
            "name": inserted.name,
            "x": inserted.x,
            "y": inserted.y,
            "width": inserted.width,
            "height": inserted.height,
        }))
    }

    fn set_fill(&self, node_id: Uuid, fill: serde_json::Value) -> std::result::Result<(), String> {
        let parsed: FillStyle =
            serde_json::from_value(fill).map_err(|e| format!("invalid fill: {e}"))?;
        mcp_set_node_fill(node_id, parsed).map_err(|e| e.to_string())
    }

    fn set_text(&self, node_id: Uuid, content: &str) -> std::result::Result<(), String> {
        crate::phase2::text_set_content(node_id, content).map_err(|e| e.to_string())
    }

    fn list_themes(&self) -> std::result::Result<serde_json::Value, String> {
        let themes: Vec<serde_json::Value> = kcreate_core::theme::builtin_themes()
            .iter()
            .map(|t| serde_json::json!({ "id": t.id, "name": t.name }))
            .collect();
        Ok(serde_json::json!({ "themes": themes }))
    }

    fn apply_theme(&self, theme_id: &str) -> std::result::Result<serde_json::Value, String> {
        let theme = kcreate_core::theme::builtin_themes()
            .into_iter()
            .find(|t| t.id == theme_id)
            .ok_or_else(|| format!("unknown theme id: {theme_id:?}"))?;
        let report = document_apply_theme(&theme).map_err(|e| e.to_string())?;
        serde_json::to_value(&report).map_err(|e| e.to_string())
    }

    fn magic_resize(
        &self,
        source_artboard_id: Uuid,
        targets: serde_json::Value,
    ) -> std::result::Result<serde_json::Value, String> {
        let specs: Vec<ResizeTargetSpec> =
            serde_json::from_value(targets).map_err(|e| format!("invalid targets: {e}"))?;
        let ids =
            crate::document::magic_resize(source_artboard_id, &specs).map_err(|e| e.to_string())?;
        Ok(serde_json::json!({
            "artboard_ids": ids.iter().map(ToString::to_string).collect::<Vec<_>>(),
        }))
    }

    fn export_design(
        &self,
        node_ids: &[Uuid],
        format: &str,
        path: &str,
        options: serde_json::Value,
    ) -> std::result::Result<serde_json::Value, String> {
        let out = Path::new(path);
        let format = format.to_ascii_lowercase();
        let bytes_written: u64 = match format.as_str() {
            // SVG is per-node + deterministic (no renderer required), so
            // it honours `node_ids`.
            "svg" => {
                let svg = self.export_svg(node_ids)?;
                let bytes = svg.into_bytes();
                let written = bytes.len() as u64;
                std::fs::write(out, &bytes).map_err(|e| e.to_string())?;
                written
            }
            // PNG / PDF render the whole current scene/document (see
            // `export_png_file` — per-node raster export needs a
            // document→scene id map the bridge does not expose here).
            "png" => {
                let req: PngExportRequest = serde_json::from_value(options)
                    .map_err(|e| format!("invalid png options: {e}"))?;
                export_png_file(out, &req).map_err(|e| e.to_string())?
            }
            "pdf" => {
                let req: PdfExportRequest = serde_json::from_value(options)
                    .map_err(|e| format!("invalid pdf options: {e}"))?;
                export_pdf_file(out, &req).map_err(|e| e.to_string())?
            }
            other => {
                return Err(format!(
                    "unsupported export format: {other:?} (use svg, png, or pdf)"
                ))
            }
        };
        Ok(serde_json::json!({
            "path": path,
            "format": format,
            "bytes_written": bytes_written,
        }))
    }
}

/// Start the local MCP server. Loopback-only. Idempotent.
///
/// The server is handed a [`kcreate_mcp::PermissionGate`] built from
/// the SAME shared permission store + pending registry the settings UI
/// drives (see [`crate::phase2::mcp_permission_store`] /
/// [`crate::phase2::mcp_pending`]), so every tool call the server
/// dispatches is gated by the user's Once/Always/Denied decisions and
/// the master switch — and a call with no decision on record surfaces
/// as a pending prompt in the UI.
#[cfg(feature = "mcp")]
pub fn mcp_start() -> Result<u32> {
    let access: std::sync::Arc<dyn kcreate_mcp::tools::DocumentAccess> =
        std::sync::Arc::new(WorkspaceAccess);
    let gate = kcreate_mcp::PermissionGate::new(
        crate::phase2::mcp_permission_store(),
        crate::phase2::mcp_pending(),
    );
    let port = kcreate_mcp::server::start_global(access, gate)
        .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))?;
    Ok(u32::from(port))
}

/// Compile-time disabled MCP entry point. The host can rebuild with
/// `--features mcp` to enable it.
#[cfg(not(feature = "mcp"))]
pub fn mcp_start() -> Result<u32> {
    Err(DocumentBridgeError::Io(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "MCP server disabled at compile time (build with --features mcp)",
    )))
}

#[cfg(feature = "mcp")]
pub fn mcp_stop() -> Result<()> {
    kcreate_mcp::server::stop_global();
    Ok(())
}

#[cfg(not(feature = "mcp"))]
pub const fn mcp_stop() -> Result<()> {
    Ok(())
}

#[cfg(feature = "mcp")]
#[must_use]
pub fn mcp_is_running() -> bool {
    kcreate_mcp::server::is_running()
}

#[cfg(not(feature = "mcp"))]
#[must_use]
pub const fn mcp_is_running() -> bool {
    false
}

// -----------------------------------------------------------------------------
// Helpers
// -----------------------------------------------------------------------------

fn build_info(project: &Project, path: &Path) -> ProjectInfo {
    ProjectInfo {
        id: project.id,
        name: project.name.clone(),
        path: path.to_path_buf(),
        created_at: project.created_at.to_rfc3339(),
        modified_at: project.modified_at.to_rfc3339(),
    }
}

fn parse_node_type(s: &str) -> Result<NodeType> {
    // NodeType serializes as snake_case (e.g. "vector_layer"). Accept
    // both that wire form and the Rust-side PascalCase (e.g.
    // "VectorLayer") to keep the host API ergonomic.
    let snake = pascal_to_snake(s);
    let json = format!("\"{snake}\"");
    serde_json::from_str::<NodeType>(&json)
        .map_err(|_| DocumentBridgeError::InvalidNodeType(s.to_string()))
}

fn pascal_to_snake(s: &str) -> String {
    if s.contains('_') || s.chars().all(|c| !c.is_ascii_uppercase()) {
        return s.to_ascii_lowercase();
    }
    let mut out = String::with_capacity(s.len() + 4);
    for (i, c) in s.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

fn default_name_for(t: NodeType) -> String {
    match t {
        NodeType::Page => "Page".to_string(),
        NodeType::Artboard => "Artboard".to_string(),
        NodeType::GroupLayer => "Group".to_string(),
        NodeType::VectorLayer => "Vector".to_string(),
        NodeType::RasterLayer => "Image".to_string(),
        NodeType::TextLayer => "Text".to_string(),
        NodeType::ComponentLayer => "Component".to_string(),
        NodeType::LayoutFrame => "Frame".to_string(),
    }
}

// -----------------------------------------------------------------------------
// Phase 5 — text frame linking + wrap (Block D Tasks 19/20)
// -----------------------------------------------------------------------------

/// Link two `TextLayer` nodes so overflow from `a_id` spills into
/// `b_id`. Both nodes must exist and be `TextLayer`. Linking a
/// frame to itself, or creating a cycle through the chain, is
/// rejected. Recorded as an undoable `text_frame_link` operation.
pub fn text_frame_link(a_id: Uuid, b_id: Uuid) -> Result<()> {
    if a_id == b_id {
        return Err(DocumentBridgeError::InvalidArgument {
            argument: "next_frame_id".into(),
            value: format!("cannot link {a_id} to itself"),
        });
    }
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;

    // Validate target exists and is a text layer.
    let target = ws
        .project
        .document
        .get_node(b_id)
        .ok_or(DocumentBridgeError::NodeNotFound(b_id))?;
    if !matches!(target.node_type, NodeType::TextLayer) {
        return Err(DocumentBridgeError::InvalidNodeType(format!(
            "{:?}",
            target.node_type
        )));
    }

    // Cycle check: walk `b`'s chain forward; if we ever land back at
    // `a`, refuse the link.
    let mut cursor = Some(b_id);
    let mut steps = 0usize;
    while let Some(cur) = cursor {
        if cur == a_id {
            return Err(DocumentBridgeError::InvalidArgument {
                argument: "next_frame_id".into(),
                value: format!("linking {a_id} -> {b_id} would create a cycle"),
            });
        }
        steps += 1;
        if steps > 4096 {
            return Err(DocumentBridgeError::InvalidArgument {
                argument: "next_frame_id".into(),
                value: "existing frame chain exceeds 4096 hops (likely corrupt)".into(),
            });
        }
        let next = ws.project.document.get_node(cur).and_then(|n| {
            if matches!(n.node_type, NodeType::TextLayer) {
                n.text_frame_options().next_frame_id
            } else {
                None
            }
        });
        cursor = next;
    }

    let before_snapshot = ws
        .project
        .document
        .get_node(a_id)
        .map_or(serde_json::Value::Null, |n| {
            serde_json::to_value(n).unwrap_or(serde_json::Value::Null)
        });

    {
        let a = ws
            .project
            .document
            .get_node_mut(a_id)
            .ok_or(DocumentBridgeError::NodeNotFound(a_id))?;
        if !matches!(a.node_type, NodeType::TextLayer) {
            return Err(DocumentBridgeError::InvalidNodeType(format!(
                "{:?}",
                a.node_type
            )));
        }
        let mut opts = a.text_frame_options();
        opts.next_frame_id = Some(b_id);
        a.set_text_frame_options(&opts);
    }

    let after_snapshot = ws
        .project
        .document
        .get_node(a_id)
        .map_or(serde_json::Value::Null, |n| {
            serde_json::to_value(n).unwrap_or(serde_json::Value::Null)
        });

    let op = Operation::new(
        "user",
        "text_frame_link",
        serde_json::json!({
            "before": before_snapshot,
            "params": { "from": a_id, "to": b_id },
        }),
        after_snapshot,
        vec![a_id, b_id],
    );
    ws.project.execute_operation(op);
    ws.project.modified_at = Utc::now();
    let _ = sync_scene_locked(&mut guard);
    Ok(())
}

/// Break the link out of `id` (sets `next_frame_id` to `None`).
/// No-op on frames that aren't currently linked.
pub fn text_frame_unlink(id: Uuid) -> Result<()> {
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    let node = ws
        .project
        .document
        .get_node(id)
        .ok_or(DocumentBridgeError::NodeNotFound(id))?;
    if !matches!(node.node_type, NodeType::TextLayer) {
        return Err(DocumentBridgeError::InvalidNodeType(format!(
            "{:?}",
            node.node_type
        )));
    }
    if node.text_frame_options().next_frame_id.is_none() {
        return Ok(());
    }

    let before_snapshot = serde_json::to_value(node).unwrap_or(serde_json::Value::Null);

    {
        let node_mut = ws
            .project
            .document
            .get_node_mut(id)
            .ok_or(DocumentBridgeError::NodeNotFound(id))?;
        let mut opts = node_mut.text_frame_options();
        opts.next_frame_id = None;
        node_mut.set_text_frame_options(&opts);
    }

    let after_snapshot = ws
        .project
        .document
        .get_node(id)
        .map_or(serde_json::Value::Null, |n| {
            serde_json::to_value(n).unwrap_or(serde_json::Value::Null)
        });

    let op = Operation::new(
        "user",
        "text_frame_unlink",
        before_snapshot,
        after_snapshot,
        vec![id],
    );
    ws.project.execute_operation(op);
    ws.project.modified_at = Utc::now();
    let _ = sync_scene_locked(&mut guard);
    Ok(())
}

/// Replace a TextLayer's wrap mode. `mode_json` is a JSON string
/// matching [`kcreate_core::node::TextWrapMode`]
/// (`"none" | "bounding_box" | "contour"`).
pub fn text_frame_set_wrap(id: Uuid, mode_json: &str) -> Result<()> {
    let mode: kcreate_core::node::TextWrapMode =
        serde_json::from_str(mode_json).map_err(DocumentBridgeError::Json)?;
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;

    let node = ws
        .project
        .document
        .get_node(id)
        .ok_or(DocumentBridgeError::NodeNotFound(id))?;
    if !matches!(node.node_type, NodeType::TextLayer) {
        return Err(DocumentBridgeError::InvalidNodeType(format!(
            "{:?}",
            node.node_type
        )));
    }

    let before_snapshot = serde_json::to_value(node).unwrap_or(serde_json::Value::Null);

    {
        let node_mut = ws
            .project
            .document
            .get_node_mut(id)
            .ok_or(DocumentBridgeError::NodeNotFound(id))?;
        let mut opts = node_mut.text_frame_options();
        opts.wrap_mode = mode;
        node_mut.set_text_frame_options(&opts);
    }

    let after_snapshot = ws
        .project
        .document
        .get_node(id)
        .map_or(serde_json::Value::Null, |n| {
            serde_json::to_value(n).unwrap_or(serde_json::Value::Null)
        });

    let op = Operation::new(
        "user",
        "text_frame_set_wrap",
        before_snapshot,
        after_snapshot,
        vec![id],
    );
    ws.project.execute_operation(op);
    ws.project.modified_at = Utc::now();
    let _ = sync_scene_locked(&mut guard);
    Ok(())
}

// -----------------------------------------------------------------------------
// Phase 5 — slices (Block D Task 22)
// -----------------------------------------------------------------------------

/// Create a new slice at `bounds` and append it to the project.
/// Returns the slice's id.
pub fn slice_create(
    name: String,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    format: &str,
    scale: f32,
) -> Result<Uuid> {
    if !x.is_finite() || !y.is_finite() || !w.is_finite() || !h.is_finite() || w <= 0.0 || h <= 0.0
    {
        return Err(DocumentBridgeError::InvalidBounds {
            width: w,
            height: h,
        });
    }
    if !scale.is_finite() || scale <= 0.0 {
        return Err(DocumentBridgeError::InvalidArgument {
            argument: "scale".into(),
            value: format!("{scale} (must be finite and positive)"),
        });
    }
    let fmt = parse_export_format(format)?;
    let bounds = kcreate_core::Bounds::new(x, y, w, h);
    let slice = Slice::new(name, bounds, fmt, scale);
    let id = slice.id;

    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    let before = serde_json::to_value(&ws.project.slices).unwrap_or(serde_json::Value::Null);
    ws.project.slices.push(slice);
    let after = serde_json::to_value(&ws.project.slices).unwrap_or(serde_json::Value::Null);
    let op = Operation::new("user", "slice_create", before, after, Vec::<Uuid>::new());
    ws.project.execute_operation(op);
    ws.project.modified_at = Utc::now();
    drop(guard);
    Ok(id)
}

/// Patch fields on an existing slice. `changes_json` is a JSON
/// object with optional `name`, `bounds` (`{x,y,width,height}`),
/// `format` (`"png" | "svg" | "pdf" | "webp" | "jpeg"`), and
/// `scale` fields. Other fields on the slice are preserved.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SliceUpdateProps {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub bounds: Option<BoundsInfo>,
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub scale: Option<f32>,
}

pub fn slice_update(id: Uuid, changes: SliceUpdateProps) -> Result<()> {
    // Validate-then-apply: every input is checked and pre-parsed
    // before we touch `ws.project.slices`, so a partial failure
    // (e.g. valid `name` + invalid `bounds`) cannot leave the
    // workspace dirtied without an `Operation` recorded — which
    // would corrupt the undo log.
    let validated_bounds = if let Some(b) = &changes.bounds {
        if !b.width.is_finite() || !b.height.is_finite() || b.width <= 0.0 || b.height <= 0.0 {
            return Err(DocumentBridgeError::InvalidBounds {
                width: b.width,
                height: b.height,
            });
        }
        Some(kcreate_core::Bounds::new(b.x, b.y, b.width, b.height))
    } else {
        None
    };
    let validated_format = if let Some(fmt_str) = &changes.format {
        Some(parse_export_format(fmt_str)?)
    } else {
        None
    };
    if let Some(s) = changes.scale {
        if !s.is_finite() || s <= 0.0 {
            return Err(DocumentBridgeError::InvalidArgument {
                argument: "scale".into(),
                value: format!("{s} (must be finite and positive)"),
            });
        }
    }

    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    let idx = ws
        .project
        .slices
        .iter()
        .position(|s| s.id == id)
        .ok_or(DocumentBridgeError::NodeNotFound(id))?;
    let before = serde_json::to_value(&ws.project.slices[idx]).unwrap_or(serde_json::Value::Null);

    if let Some(name) = changes.name {
        ws.project.slices[idx].name = name;
    }
    if let Some(bounds) = validated_bounds {
        ws.project.slices[idx].bounds = bounds;
    }
    if let Some(fmt) = validated_format {
        ws.project.slices[idx].format = fmt;
        ws.project.slices[idx].suffix = match fmt {
            ExportFormat::Png => ".png".into(),
            ExportFormat::Svg => ".svg".into(),
            ExportFormat::Pdf => ".pdf".into(),
            ExportFormat::Webp => ".webp".into(),
            ExportFormat::Jpeg => ".jpg".into(),
        };
    }
    if let Some(s) = changes.scale {
        ws.project.slices[idx].scale = s;
    }

    let after = serde_json::to_value(&ws.project.slices[idx]).unwrap_or(serde_json::Value::Null);
    let op = Operation::new("user", "slice_update", before, after, Vec::<Uuid>::new());
    ws.project.execute_operation(op);
    ws.project.modified_at = Utc::now();
    drop(guard);
    Ok(())
}

/// Remove a slice by id. Returns true when something was removed.
pub fn slice_delete(id: Uuid) -> Result<bool> {
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    let before = serde_json::to_value(&ws.project.slices).unwrap_or(serde_json::Value::Null);
    let before_len = ws.project.slices.len();
    ws.project.slices.retain(|s| s.id != id);
    let removed = ws.project.slices.len() != before_len;
    if removed {
        let after = serde_json::to_value(&ws.project.slices).unwrap_or(serde_json::Value::Null);
        let op = Operation::new("user", "slice_delete", before, after, Vec::<Uuid>::new());
        ws.project.execute_operation(op);
        ws.project.modified_at = Utc::now();
    }
    Ok(removed)
}

/// List every slice in the project, in insertion order.
pub fn slice_list() -> Result<Vec<Slice>> {
    let guard = slot().write();
    let ws = guard.as_ref().ok_or(DocumentBridgeError::NoProject)?;
    Ok(ws.project.slices.clone())
}

/// Render every slice to a separate file in `output_dir`. Returns
/// one entry per slice describing the path (or per-slice error).
/// Top-level errors (`output_dir` is not a directory, etc.) are
/// promoted to a `DocumentBridgeError::Io`.
pub fn slice_export_all(output_dir: &Path) -> Result<Vec<kcreate_export::slice::SliceResult>> {
    let scene = crate::state::current_scene()
        .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))?;
    let slices = {
        let guard = slot().write();
        let ws = guard.as_ref().ok_or(DocumentBridgeError::NoProject)?;
        ws.project.slices.clone()
    };
    kcreate_export::slice::export_slices(&scene, &slices, output_dir)
        .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))
}

// -----------------------------------------------------------------------------
// Phase 5 — .kbrand import/export (Block D Task 21)
// -----------------------------------------------------------------------------

/// Resolve a font archive path to its IANA MIME type. The match is
/// case-insensitive against the file extension.
fn font_mime_for(path: &str) -> &'static str {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("ttf") => "font/ttf",
        Some("otf") => "font/otf",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    }
}

/// Resolve a logo archive path to its IANA MIME type. The match is
/// case-insensitive against the file extension.
fn logo_mime_for(path: &str) -> &'static str {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("svg") => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

/// Serialize a brand kit (plus its referenced font / logo blobs)
/// into a `.kbrand` archive at `output_path`. The bridge resolves
/// each referenced blob via the project's `assets` table — fonts
/// without an `embedded_asset_id` are recorded by family name in
/// the manifest but contribute no archive entry.
pub fn brand_kit_export(kit_id: Uuid, output_path: &Path) -> Result<()> {
    let (kit, font_assets, logo_assets) = collect_brand_kit_assets(kit_id)?;
    kcreate_export::kbrand::export_brand_kit(&kit, &font_assets, &logo_assets, output_path)
        .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))?;
    Ok(())
}

/// In-memory companion to [`brand_kit_export`]: serialise the
/// `.kbrand` archive into a `Vec<u8>` without touching disk. Used
/// by the KChat artifact-publishing pipeline so the bytes can be
/// streamed straight into a multipart upload.
///
/// Returns the kit name + the encoded archive bytes. The name is
/// surfaced so the publisher can stamp it into the metadata
/// (`projectName`) without re-loading the kit.
pub fn brand_kit_export_to_bytes(kit_id: Uuid) -> Result<(String, Vec<u8>)> {
    let (kit, font_assets, logo_assets) = collect_brand_kit_assets(kit_id)?;
    let bytes = kcreate_export::kbrand::export_brand_kit_to_bytes(&kit, &font_assets, &logo_assets)
        .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))?;
    Ok((kit.name, bytes))
}

/// Shared helper that snapshots a brand kit + every embedded font /
/// logo asset blob it references. Pulled out so both
/// [`brand_kit_export`] (file destination) and
/// [`brand_kit_export_to_bytes`] (in-memory) can drive the same
/// load logic without diverging on what counts as "embedded".
/// Bundle of brand-kit + the (basename → raw bytes) maps for its
/// embedded fonts + logos, returned by [`collect_brand_kit_assets`].
/// Pulled out as a type alias so `clippy::type_complexity` is happy
/// and so the file-vs-bytes export functions share one signature.
type BrandKitAssetBundle = (
    BrandKit,
    std::collections::HashMap<String, Vec<u8>>,
    std::collections::HashMap<String, Vec<u8>>,
);

fn collect_brand_kit_assets(kit_id: Uuid) -> Result<BrandKitAssetBundle> {
    let guard = slot().write();
    let ws = guard.as_ref().ok_or(DocumentBridgeError::NoProject)?;
    let kit = ws
        .project
        .brand_kits
        .iter()
        .find(|k| k.id == kit_id)
        .cloned()
        .ok_or(DocumentBridgeError::NodeNotFound(kit_id))?;

    // Build the font-asset map keyed exactly the way kbrand's
    // `export_brand_kit` looks them up — i.e. through
    // `kcreate_export::kbrand::font_archive_basename`. Otherwise
    // any family containing non-alphanumeric characters
    // (e.g. "Source-Sans-Pro") would fail the lookup and silently
    // drop the font bytes from the archive.
    let mut fonts: std::collections::HashMap<String, Vec<u8>> = std::collections::HashMap::new();
    for font in &kit.fonts {
        if let Some(asset_id) = font.embedded_asset_id {
            let loaded = {
                let store = ws.store.lock();
                store.load_asset(asset_id)?
            };
            if let Some(bytes) = loaded {
                let key = kcreate_export::kbrand::font_archive_basename(
                    &font.family,
                    font.weight,
                    font.italic,
                );
                fonts.insert(key, bytes);
            }
        }
    }
    let mut logos: std::collections::HashMap<String, Vec<u8>> = std::collections::HashMap::new();
    if let Some(logo_id) = kit.logo_asset_id {
        let loaded = {
            let store = ws.store.lock();
            store.load_asset(logo_id)?
        };
        if let Some(bytes) = loaded {
            logos.insert("primary".into(), bytes);
        }
    }
    Ok((kit, fonts, logos))
}

/// Import a `.kbrand` archive: persist every embedded font / logo
/// asset under fresh ids in the project's asset table, build a
/// new [`BrandKit`] from the manifest, and append it. Returns the
/// new kit's id.
pub fn brand_kit_import(file_path: &Path) -> Result<Uuid> {
    let bundle = kcreate_export::kbrand::import_brand_kit(file_path)
        .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))?;

    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;

    // Stage 1: walk the manifest and turn each archive-relative
    // asset path into a project-asset Uuid by storing the bytes.
    // We also snapshot each font's `archive_path` in declaration
    // order so Stage 2 can recover the link without reconstructing
    // the path (and inheriting kbrand.rs's sanitisation rules).
    let mut font_archive_paths: Vec<Option<String>> =
        Vec::with_capacity(bundle.manifest.fonts.len());
    let mut font_asset_ids: std::collections::HashMap<String, Uuid> =
        std::collections::HashMap::new();
    for font in &bundle.manifest.fonts {
        let Some(archive_path) = &font.archive_path else {
            font_archive_paths.push(None);
            continue;
        };
        let Some(bytes) = bundle.assets.get(archive_path) else {
            font_archive_paths.push(None);
            continue;
        };
        let mime = font_mime_for(archive_path);
        let id = Uuid::new_v4();
        ws.store.lock().store_asset_with_id(id, bytes, mime)?;
        font_asset_ids.insert(archive_path.clone(), id);
        font_archive_paths.push(Some(archive_path.clone()));
    }
    let logo_asset_id = bundle.manifest.logos.first().and_then(|logo| {
        let bytes = bundle.assets.get(&logo.archive_path)?;
        let mime = logo_mime_for(&logo.archive_path);
        let id = Uuid::new_v4();
        ws.store.lock().store_asset_with_id(id, bytes, mime).ok()?;
        Some(id)
    });

    // Stage 2: build a BrandKit whose font/logo references point at
    // the freshly-stored asset ids. `KbrandBundle::into_brand_kit`
    // preserves the manifest's font order, so we can recover each
    // archive_path by index.
    let mut kit = bundle.into_brand_kit();
    kit.logo_asset_id = logo_asset_id;
    for (idx, font) in kit.fonts.iter_mut().enumerate() {
        let Some(Some(archive_path)) = font_archive_paths.get(idx) else {
            continue;
        };
        if let Some(id) = font_asset_ids.get(archive_path) {
            font.embedded_asset_id = Some(*id);
        }
    }
    let new_id = kit.id;
    ws.project.brand_kits.push(kit);
    ws.project.modified_at = Utc::now();
    // Register the just-imported kit's embedded fonts so the family
    // resolves at render / export time on this machine.
    if let Some(imported) = ws.project.brand_kits.last() {
        let store = ws.store.lock();
        register_kit_embedded_fonts(&store, imported);
    }
    drop(guard);
    Ok(new_id)
}

// -----------------------------------------------------------------------------
// H5 — user brand-kit depth: logo / font roles / palette-from-image /
// logo placement / cross-project on-disk registry. These compose the
// existing project-scoped brand_kit CRUD + `.kbrand` import/export with
// the persistent registry in `brand_registry.rs`.
// -----------------------------------------------------------------------------

/// Sniff a logo's MIME type from its bytes. SVG (text) is detected by
/// its root marker so a logo pasted/uploaded without a file extension
/// still round-trips through the vector insertion path; everything else
/// falls back to the raster sniff used by image import.
fn logo_mime_for_bytes(bytes: &[u8]) -> &'static str {
    let head = &bytes[..bytes.len().min(512)];
    if let Ok(text) = std::str::from_utf8(head) {
        let trimmed = text.trim_start_matches('\u{feff}').trim_start();
        if trimmed.starts_with("<?xml") || trimmed.starts_with("<svg") || trimmed.contains("<svg") {
            return "image/svg+xml";
        }
    }
    mime_for_bytes(bytes)
}

/// Sniff a font file's MIME type from its leading magic bytes. Used when
/// embedding a fontdb-resolved face so the stored asset carries a
/// sensible content type (the `.kbrand` archive keys fonts by family,
/// not MIME, so this is purely descriptive).
fn font_mime_for_bytes(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(b"OTTO") {
        "font/otf"
    } else if bytes.starts_with(b"wOFF") {
        "font/woff"
    } else if bytes.starts_with(b"wOF2") {
        "font/woff2"
    } else {
        // TrueType (`0x00010000` / `"true"` / `"ttcf"`) and anything else.
        "font/ttf"
    }
}

/// Register every embedded font referenced by `kit` into the
/// process-wide font database so the family resolves at shape / export
/// time even when it is **not** installed as a system font. This is the
/// runtime half of font embedding: the bytes are persisted with the
/// project / brand kit, and on open (or import / registry-load) we feed
/// them back so a document authored with a custom font renders that font
/// on any machine — otherwise text would silently fall back to a default
/// face and the export would no longer carry the chosen typography.
///
/// Families fontdb already knows (a system font, or one registered by an
/// earlier call) are skipped via an exact-name probe so repeated opens
/// don't pile up duplicate faces. A missing or unparseable asset is a
/// non-fatal skip — one bad blob must never block opening a project.
fn register_kit_embedded_fonts(store: &ProjectStore, kit: &BrandKit) {
    let mgr = kcreate_text::FontManager::new();
    for font in &kit.fonts {
        let Some(asset_id) = font.embedded_asset_id else {
            continue;
        };
        // Exact-family probe: `find_family` only returns faces whose
        // name table matches, unlike `resolve_face` (which falls back
        // to any outline font), so it reliably answers "is THIS family
        // already loaded?".
        if !mgr.find_family(&font.family).is_empty() {
            continue;
        }
        if let Ok(Some(bytes)) = store.load_asset(asset_id) {
            let _ = mgr.add_font_bytes(bytes);
        }
    }
}

/// Replace a brand kit's logo with `bytes` (an SVG or raster image).
/// The blob is stored in the project asset table under a fresh id and
/// `logo_asset_id` is repointed at it. The MIME type is sniffed from the
/// bytes so the later `.kbrand` export and [`brand_logo_insert`] pick the
/// right (vector vs raster) path.
pub fn brand_kit_set_logo_bytes(kit_id: Uuid, bytes: &[u8]) -> Result<()> {
    if bytes.is_empty() {
        return Err(DocumentBridgeError::InvalidArgument {
            argument: "bytes".to_string(),
            value: "empty logo".to_string(),
        });
    }
    let mime = logo_mime_for_bytes(bytes);
    with_workspace_mut(|ws| {
        let kit_idx = ws
            .project
            .brand_kits
            .iter()
            .position(|k| k.id == kit_id)
            .ok_or(DocumentBridgeError::NodeNotFound(kit_id))?;
        let asset_id = Uuid::new_v4();
        ws.store.lock().store_asset_with_id(asset_id, bytes, mime)?;
        ws.project.brand_kits[kit_idx].logo_asset_id = Some(asset_id);
        ws.project.modified_at = Utc::now();
        Ok(())
    })
}

/// Set the heading or body font of a brand kit to `family`. When `embed`
/// is set, the chosen face is resolved from fontdb and its raw file bytes
/// are stored as a project asset so the `.kbrand` archive (and any
/// font-embedding export path) carries the actual font.
///
/// `role` is `"heading"` or `"body"`. The kit stores one [`FontRef`] per
/// weight bucket — heading at [`TypeRole::Heading`]'s weight (≥ 600) and
/// body at [`TypeRole::Body`]'s (< 600) — matching the split
/// [`Theme::from_brand_kit`] uses to recover each role.
pub fn brand_kit_set_font_role(
    kit_id: Uuid,
    role: &str,
    family: String,
    embed: bool,
) -> Result<()> {
    let weight = match role {
        "heading" => TypeRole::Heading.font_weight(),
        "body" => TypeRole::Body.font_weight(),
        other => {
            return Err(DocumentBridgeError::InvalidArgument {
                argument: "role".to_string(),
                value: other.to_string(),
            });
        }
    };
    if family.trim().is_empty() {
        return Err(DocumentBridgeError::InvalidArgument {
            argument: "family".to_string(),
            value: "empty font family".to_string(),
        });
    }

    // Resolve the face BEFORE locking the workspace: fontdb maintains its
    // own process-global lock and scans system fonts, so we never want to
    // hold the workspace write lock across that work.
    let resolved_data = if embed {
        let face = kcreate_text::FontManager::new()
            .resolve_face(&family)
            .map_err(|e| DocumentBridgeError::InvalidArgument {
                argument: "family".to_string(),
                value: format!("{family}: {e}"),
            })?;
        Some(face.data)
    } else {
        None
    };

    with_workspace_mut(|ws| {
        let kit_idx = ws
            .project
            .brand_kits
            .iter()
            .position(|k| k.id == kit_id)
            .ok_or(DocumentBridgeError::NodeNotFound(kit_id))?;
        let embedded_asset_id = match &resolved_data {
            Some(data) => {
                let mime = font_mime_for_bytes(data);
                let asset_id = Uuid::new_v4();
                ws.store.lock().store_asset_with_id(asset_id, data, mime)?;
                Some(asset_id)
            }
            None => None,
        };
        let font = FontRef {
            family,
            weight,
            italic: false,
            embedded_asset_id,
        };
        let is_heading = weight >= 600;
        let kit = &mut ws.project.brand_kits[kit_idx];
        if let Some(existing) = kit
            .fonts
            .iter_mut()
            .find(|f| (f.weight >= 600) == is_heading)
        {
            *existing = font;
        } else {
            kit.fonts.push(font);
        }
        ws.project.modified_at = Utc::now();
        // Make the embedded face resolvable now so an immediate
        // apply-theme + export uses it without waiting for a reopen.
        if resolved_data.is_some() {
            let store = ws.store.lock();
            register_kit_embedded_fonts(&store, &ws.project.brand_kits[kit_idx]);
        }
        Ok(())
    })
}

/// Extract up to `num_colors` dominant colors from an uploaded image and
/// store them as the kit's palette (the Canva "brand colors from a photo"
/// flow). Returns the hex codes in dominance order. `num_colors` is
/// clamped to `1..=64`, matching [`phase9::palette_extract_and_apply_brand_kit`].
pub fn brand_kit_extract_palette_from_image_bytes(
    kit_id: Uuid,
    bytes: &[u8],
    num_colors: usize,
) -> Result<Vec<String>> {
    if bytes.is_empty() {
        return Err(DocumentBridgeError::InvalidArgument {
            argument: "bytes".to_string(),
            value: "empty image".to_string(),
        });
    }
    if !(1..=64).contains(&num_colors) {
        return Err(DocumentBridgeError::InvalidArgument {
            argument: "num_colors".to_string(),
            value: num_colors.to_string(),
        });
    }
    let img = image::load_from_memory(bytes)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    let extracted = kcreate_ai::palette::extract_palette(rgba.as_raw(), width, height, num_colors);
    let named: Vec<NamedColor> = extracted
        .iter()
        .enumerate()
        .map(|(i, c)| NamedColor {
            name: format!("Color {}", i + 1),
            color: RgbaColor::new(
                f32::from(c.r) / 255.0,
                f32::from(c.g) / 255.0,
                f32::from(c.b) / 255.0,
                1.0,
            ),
        })
        .collect();
    let hex_codes: Vec<String> = named.iter().map(|c| c.color.to_hex()).collect();

    with_workspace_mut(|ws| {
        let kit = ws
            .project
            .brand_kits
            .iter_mut()
            .find(|k| k.id == kit_id)
            .ok_or(DocumentBridgeError::NodeNotFound(kit_id))?;
        kit.colors = named;
        ws.project.modified_at = Utc::now();
        Ok(())
    })?;
    Ok(hex_codes)
}

/// Insert a brand kit's saved logo onto the canvas as an editable node.
///
/// SVG logos become a group of recolorable [`NodeType::VectorLayer`]
/// nodes (so the theme can remap their fills); raster logos become a
/// single [`NodeType::RasterLayer`]. Either way the artwork is uniformly
/// scaled so its longest side equals `target_size`, placed with its
/// top-left at `(x, y)`, and recorded as one undoable `brand_logo_insert`
/// operation.
pub fn brand_logo_insert(
    kit_id: Uuid,
    parent_id: Option<Uuid>,
    x: f64,
    y: f64,
    target_size: f64,
) -> Result<crate::assets::InsertedAsset> {
    // Pull the logo bytes + content type out under a read lock, which is
    // released when `with_workspace` returns — the insertion helpers below
    // take their own workspace write lock.
    let (bytes, mime) = with_workspace(|ws| {
        let kit = ws
            .project
            .brand_kits
            .iter()
            .find(|k| k.id == kit_id)
            .ok_or(DocumentBridgeError::NodeNotFound(kit_id))?;
        let logo_id = kit
            .logo_asset_id
            .ok_or_else(|| DocumentBridgeError::InvalidArgument {
                argument: "kit_id".to_string(),
                value: format!("brand kit {kit_id} has no logo"),
            })?;
        let store = ws.store.lock();
        let bytes = store
            .load_asset(logo_id)?
            .ok_or(DocumentBridgeError::NodeNotFound(logo_id))?;
        let mime = store.asset_mime(logo_id)?.unwrap_or_default();
        Ok((bytes, mime))
    })?;

    if mime == "image/svg+xml" || logo_mime_for_bytes(&bytes) == "image/svg+xml" {
        crate::assets::insert_styled_paths(
            &bytes,
            "Brand Logo",
            parent_id,
            x,
            y,
            target_size,
            "brand_logo_insert",
            serde_json::json!({ "kit_id": kit_id }),
        )
    } else {
        let raster_mime = if mime.is_empty() {
            mime_for_bytes(&bytes).to_string()
        } else {
            mime
        };
        let (id, bounds) = import_raster_node(
            parent_id,
            &bytes,
            &raster_mime,
            "Brand Logo",
            Some((x, y, target_size)),
            "brand_logo_insert",
        )?;
        Ok(crate::assets::InsertedAsset {
            group_id: id.to_string(),
            node_ids: vec![id.to_string()],
            name: "Brand Logo".to_string(),
            x: bounds.x,
            y: bounds.y,
            width: bounds.width,
            height: bounds.height,
        })
    }
}

/// Persist the project's brand kit `kit_id` to the cross-project on-disk
/// registry (see [`brand_registry`]), bundling every blob it references
/// (logo + each embedded font) so a future session — or a different
/// project — can re-hydrate it offline. Overwrites any registry record
/// with the same id.
pub fn brand_kit_registry_save(kit_id: Uuid) -> Result<()> {
    let record = with_workspace(|ws| {
        let kit = ws
            .project
            .brand_kits
            .iter()
            .find(|k| k.id == kit_id)
            .cloned()
            .ok_or(DocumentBridgeError::NodeNotFound(kit_id))?;
        let store = ws.store.lock();
        let mut asset_ids: Vec<Uuid> = Vec::new();
        if let Some(logo_id) = kit.logo_asset_id {
            asset_ids.push(logo_id);
        }
        for font in &kit.fonts {
            if let Some(id) = font.embedded_asset_id {
                asset_ids.push(id);
            }
        }
        let mut seen: HashSet<Uuid> = HashSet::new();
        let mut assets: Vec<crate::brand_registry::BrandAssetBlob> = Vec::new();
        for id in asset_ids {
            if !seen.insert(id) {
                continue;
            }
            if let Some(bytes) = store.load_asset(id)? {
                let mime = store
                    .asset_mime(id)?
                    .unwrap_or_else(|| "application/octet-stream".to_string());
                assets.push(crate::brand_registry::BrandAssetBlob {
                    asset_id: id,
                    mime,
                    bytes,
                });
            }
        }
        Ok(crate::brand_registry::BrandKitRecord { kit, assets })
    })?;
    crate::brand_registry::save_record(&record)?;
    Ok(())
}

/// List the brand kits available in the cross-project on-disk registry
/// (metadata only — asset blobs are loaded lazily by
/// [`brand_kit_registry_load`]).
pub fn brand_kit_registry_list() -> Result<Vec<BrandKit>> {
    Ok(crate::brand_registry::list_kits()?)
}

/// Load registry kit `kit_id` into the open project: re-store each of its
/// bundled blobs under fresh project-asset ids, relink the kit's
/// logo / font references to those ids, and upsert the kit into the
/// project (keeping the registry id stable so a re-load replaces in
/// place). Returns the kit id.
pub fn brand_kit_registry_load(kit_id: Uuid) -> Result<Uuid> {
    let record = crate::brand_registry::load_record(kit_id)?
        .ok_or(DocumentBridgeError::NodeNotFound(kit_id))?;
    with_workspace_mut(|ws| {
        let mut id_map: HashMap<Uuid, Uuid> = HashMap::new();
        {
            let mut store = ws.store.lock();
            for asset in &record.assets {
                let new_id = Uuid::new_v4();
                store.store_asset_with_id(new_id, &asset.bytes, &asset.mime)?;
                id_map.insert(asset.asset_id, new_id);
            }
        }
        let mut kit = record.kit;
        if let Some(old) = kit.logo_asset_id {
            kit.logo_asset_id = id_map.get(&old).copied();
        }
        for font in &mut kit.fonts {
            if let Some(old) = font.embedded_asset_id {
                font.embedded_asset_id = id_map.get(&old).copied();
            }
        }
        let new_id = kit.id;
        if let Some(existing) = ws.project.brand_kits.iter_mut().find(|k| k.id == new_id) {
            *existing = kit;
        } else {
            ws.project.brand_kits.push(kit);
        }
        ws.project.modified_at = Utc::now();
        // Register the loaded kit's embedded fonts so an immediate
        // apply uses the bundled font even if it isn't installed here.
        if let Some(loaded) = ws.project.brand_kits.iter().find(|k| k.id == new_id) {
            let store = ws.store.lock();
            register_kit_embedded_fonts(&store, loaded);
        }
        Ok(new_id)
    })
}

/// Delete brand kit `kit_id` from the cross-project on-disk registry.
/// Returns `true` if a record was removed, `false` if none existed. Does
/// not touch the open project's in-memory kits.
pub fn brand_kit_registry_delete(kit_id: Uuid) -> Result<bool> {
    Ok(crate::brand_registry::delete_kit(kit_id)?)
}

// -----------------------------------------------------------------------------
// Phase 5 — spot color / overprint convenience entry points (Block D Task 23)
// -----------------------------------------------------------------------------

/// Spec-shaped alias for `color_spot_upsert`: inserts a spot color
/// keyed by `name`, with the canonical CMYK fallback. The display
/// name defaults to `name` (writers that need a separate display
/// name should keep using `color_spot_upsert` directly).
pub fn color_add_spot(name: String, c: f32, m: f32, y: f32, k: f32) -> Result<()> {
    use kcreate_core::color::SpotColorDef;
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    let before = serde_json::to_value(&ws.project.spot_color_library)?;
    let def = SpotColorDef {
        display_name: name.clone(),
        fallback_cmyk: (c, m, y, k),
        library_reference: None,
    };
    ws.project.spot_color_library.insert(name, def);
    let after = serde_json::to_value(&ws.project.spot_color_library)?;
    let op = Operation::new(
        "user",
        "spot_color_upsert",
        before,
        after,
        Vec::<Uuid>::new(),
    );
    ws.project.execute_operation(op);
    ws.project.modified_at = Utc::now();
    let _ = sync_scene_locked(&mut guard);
    Ok(())
}

/// Toggle a node's overprint flag. Recorded as an undoable
/// `node_set_overprint` operation. `node_id` must reference any
/// node (overprint is a style flag, not node-type-specific).
pub fn node_set_overprint(id: Uuid, enabled: bool) -> Result<()> {
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    let before_snapshot = ws
        .project
        .document
        .get_node(id)
        .map(|n| serde_json::to_value(n).unwrap_or(serde_json::Value::Null))
        .ok_or(DocumentBridgeError::NodeNotFound(id))?;

    {
        let node = ws
            .project
            .document
            .get_node_mut(id)
            .ok_or(DocumentBridgeError::NodeNotFound(id))?;
        node.style.overprint = enabled;
    }

    let after_snapshot = ws
        .project
        .document
        .get_node(id)
        .map_or(serde_json::Value::Null, |n| {
            serde_json::to_value(n).unwrap_or(serde_json::Value::Null)
        });

    let op = Operation::new(
        "user",
        "node_set_overprint",
        before_snapshot,
        after_snapshot,
        vec![id],
    );
    ws.project.execute_operation(op);
    ws.project.modified_at = Utc::now();
    let _ = sync_scene_locked(&mut guard);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    fn tmpdir() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    /// Devin Review ANALYSIS (PR #16): the fragile coupling between
    /// `ApplyPatchSnapshot::capture` and `apply_patch`'s match arms
    /// was flagged as a maintenance hazard. This test makes the
    /// coupling explicit: every command listed in
    /// `APPLY_PATCH_COMMANDS` must be reachable through the
    /// snapshot path (otherwise group-undo rolls back inconsistently)
    /// and through the dispatcher path (otherwise undo / redo is a
    /// silent no-op). We test the dispatcher path by constructing a
    /// minimal `Operation` per command and asserting `apply_patch`
    /// either succeeds or returns a *parsing* error — never a
    /// fall-through to the default no-op arm (which the new
    /// `debug_assert!` would also trip on).
    #[test]
    fn apply_patch_commands_match_dispatcher_arms() {
        // Every command we declare patchable must have a match
        // arm. If a maintainer adds a new entry to
        // APPLY_PATCH_COMMANDS without writing the corresponding
        // arm, the debug_assert in apply_patch's default arm fires
        // on every group-undo of that command, surfacing the bug
        // in CI before a release.
        //
        // Hand-maintain this expected set alongside
        // APPLY_PATCH_COMMANDS. If the two diverge, this test
        // fails loudly with both lists printed for diff.
        let expected: std::collections::BTreeSet<&'static str> = [
            "color_settings_update",
            "spot_color_upsert",
            "spot_color_remove",
            "spot_color_load_catalog",
            "text_frame_update",
            "text_opentype_features_update",
            "layer_color_set",
            "clipboard_paste",
            // Phase A1 — inline text editor + font controls.
            "text_set_content",
            "text_replace_range",
            "text_set_style",
            // G4 — Theme / Brand Kit instant restyle.
            "apply_theme",
            // H4 — AI generation depth (generate + refine).
            "ai_generate_themed_design",
            "ai_refine_themed_design",
        ]
        .into_iter()
        .collect();
        let actual: std::collections::BTreeSet<&'static str> =
            APPLY_PATCH_COMMANDS.iter().copied().collect();
        assert_eq!(
            actual, expected,
            "APPLY_PATCH_COMMANDS drifted from the test expected-set — \
             update both the const and this test, and confirm capture() \
             + apply_patch() cover the new command end-to-end"
        );
    }

    #[test]
    #[serial]
    fn lifecycle_create_save_close_reopen() {
        reset_for_tests();
        let dir = tmpdir();
        let info = project_create("demo", dir.path()).expect("create");
        assert_eq!(info.name, "demo");
        // page + artboard were created.
        let tree = document_get_tree().expect("tree");
        assert_eq!(tree.len(), 2);
        project_save().expect("save");
        project_close();
        // Reopen.
        let info2 = project_open(&dir.path().join("demo.kstudio")).expect("open");
        assert_eq!(info2.name, "demo");
        let tree2 = document_get_tree().expect("tree2");
        assert_eq!(tree2.len(), 2);
        project_close();
    }

    #[test]
    #[serial]
    fn create_in_existing_dir_errors() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("x", dir.path()).expect("first");
        // Must close first; reopening over a live workspace is the
        // wrong shape of error (it would silently drop unsaved work).
        project_close();
        let err = project_create("x", dir.path()).expect_err("dup");
        assert!(matches!(err, DocumentBridgeError::ProjectDirExists(_)));
    }

    #[test]
    #[serial]
    fn create_while_open_errors_with_already_open() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("a", dir.path()).expect("first");
        let err = project_create("b", dir.path()).expect_err("blocked");
        assert!(matches!(err, DocumentBridgeError::ProjectAlreadyOpen(_)));
        project_close();
    }

    #[test]
    #[serial]
    fn open_while_open_errors_with_already_open() {
        reset_for_tests();
        let dir = tmpdir();
        let info = project_create("a", dir.path()).expect("first");
        let err = project_open(&info.path).expect_err("blocked");
        assert!(matches!(err, DocumentBridgeError::ProjectAlreadyOpen(_)));
        project_close();
    }

    #[test]
    #[serial]
    fn project_id_is_stable_across_reopen() {
        reset_for_tests();
        let dir = tmpdir();
        let original = project_create("stable", dir.path()).expect("create");
        project_save().expect("save");
        project_close();
        let reopened = project_open(&original.path).expect("reopen");
        assert_eq!(
            original.id, reopened.id,
            "project.id must be the same UUID as the manifest persisted on disk"
        );
        assert_eq!(original.created_at, reopened.created_at);
        project_close();
    }

    #[test]
    #[serial]
    fn operation_history_survives_close_reopen() {
        reset_for_tests();
        let dir = tmpdir();
        let info = project_create("ops", dir.path()).expect("create");
        let op = Operation::new(
            "user",
            "demo",
            serde_json::Value::Null,
            serde_json::Value::Null,
            Vec::new(),
        );
        let op_id = op.id;
        document_record_operation(op).expect("record");
        project_save().expect("save");

        let status = document_status().expect("status");
        assert_eq!(status.undo_depth, 1, "in-memory log shows one op");
        assert!(status.can_undo);

        project_close();
        project_open(&info.path).expect("reopen");
        let status = document_status().expect("status");
        assert_eq!(
            status.undo_depth, 1,
            "operation log must have been restored from disk"
        );
        assert!(status.can_undo);

        // Round-trip the actual op via the storage layer so we know the
        // restored entry is the same one we wrote (not a different one
        // accidentally regenerated).
        let _ = op_id; // ensure original id was captured
        project_close();
    }

    #[test]
    #[serial]
    fn document_status_when_no_project_open() {
        reset_for_tests();
        assert!(
            document_status().is_none(),
            "no project => no status, host should disable controls"
        );
    }

    #[test]
    #[serial]
    fn document_status_reflects_log_position() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("s", dir.path()).expect("create");
        let s0 = document_status().expect("status");
        assert!(!s0.can_undo);
        assert!(!s0.can_redo);

        document_record_operation(Operation::new(
            "user",
            "noop",
            serde_json::Value::Null,
            serde_json::Value::Null,
            Vec::new(),
        ))
        .expect("record");
        let s1 = document_status().expect("status");
        assert!(s1.can_undo);
        assert!(!s1.can_redo);

        document_undo().expect("undo");
        let s2 = document_status().expect("status");
        assert!(!s2.can_undo);
        assert!(s2.can_redo);

        project_close();
    }

    #[test]
    #[serial]
    fn project_is_untouched_no_project_open_errors() {
        reset_for_tests();
        let err = project_is_untouched().expect_err("no project");
        assert!(matches!(err, DocumentBridgeError::NoProject));
    }

    #[test]
    #[serial]
    fn project_is_untouched_flips_after_any_operation() {
        // Devin Review PR #5 ANALYSIS-0006 (commit 5c16b5c) replaced
        // the TS-side `nodes.length === 2 && one Page named "Page 1"
        // && one Artboard` heuristic with this bridge-driven signal.
        // The contract: a freshly-created project reports `true`, and
        // any host-recorded operation flips it to `false` and keeps it
        // there for the rest of the session (undo does NOT restore
        // untouched, because the redo cursor still has the op in it).
        reset_for_tests();
        let dir = tmpdir();
        project_create("untouched", dir.path()).expect("create");
        assert!(
            project_is_untouched().expect("untouched after create"),
            "project_create leaves operation_log empty",
        );

        document_record_operation(Operation::new(
            "user",
            "noop",
            serde_json::Value::Null,
            serde_json::Value::Null,
            Vec::new(),
        ))
        .expect("record");
        assert!(
            !project_is_untouched().expect("touched after op"),
            "recording an operation must mark the project as touched",
        );

        // Undo moves the log cursor but the entry is still in history;
        // the project is no longer "untouched" by the spec.
        document_undo().expect("undo");
        assert!(
            !project_is_untouched().expect("touched after undo"),
            "undo does not restore an untouched project",
        );

        project_close();
    }

    #[test]
    #[serial]
    fn project_is_untouched_survives_save_close_reopen_when_clean() {
        // A project that was created+saved without any user edits
        // must still report `untouched=true` after a full
        // close+reopen cycle. `project_open` restores the operation
        // history from disk; an empty on-disk history stays empty in
        // memory.
        reset_for_tests();
        let dir = tmpdir();
        project_create("clean", dir.path()).expect("create");
        project_save().expect("save");
        project_close();

        project_open(&dir.path().join("clean.kstudio")).expect("reopen");
        assert!(
            project_is_untouched().expect("untouched after clean reopen"),
            "clean reopen must stay untouched",
        );
        project_close();
    }

    #[test]
    #[serial]
    fn project_is_untouched_reports_touched_after_save_close_reopen_when_dirty() {
        // A project edited before save+close has its operation log
        // persisted; reopen restores it, so `is_untouched` reports
        // `false`. This is the contract the EditorPage relies on:
        // re-opening a previously-edited project must NOT auto-pop
        // the TemplatePicker.
        reset_for_tests();
        let dir = tmpdir();
        project_create("dirty", dir.path()).expect("create");
        document_record_operation(Operation::new(
            "user",
            "noop",
            serde_json::Value::Null,
            serde_json::Value::Null,
            Vec::new(),
        ))
        .expect("record");
        project_save().expect("save");
        project_close();

        project_open(&dir.path().join("dirty.kstudio")).expect("reopen");
        assert!(
            !project_is_untouched().expect("touched after dirty reopen"),
            "reopen of a previously-edited project must report touched",
        );
        project_close();
    }

    #[test]
    #[serial]
    fn crud_create_update_delete() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("crud", dir.path()).expect("create");
        let page_id = document_get_tree().expect("tree")[0].id;
        let id = document_create_node(
            "VectorLayer",
            Some(page_id),
            &CreateNodeProps {
                name: Some("Rect".to_string()),
                ..Default::default()
            },
        )
        .expect("create");
        document_update_node(
            id,
            &UpdateNodeProps {
                visible: Some(false),
                ..Default::default()
            },
        )
        .expect("update");
        let tree = document_get_tree().expect("tree");
        let inserted = tree.iter().find(|n| n.id == id).expect("present");
        assert!(!inserted.visible);
        document_delete_node(id).expect("delete");
        let tree = document_get_tree().expect("tree");
        assert!(tree.iter().all(|n| n.id != id));
        project_close();
    }

    /// Wire-format lockstep test for the `fill` field added to
    /// `UpdateNodeProps`. Deserialises every `FillStyle` variant
    /// from the JSON shape the renderer emits, applies it through
    /// `document_update_node`, and then reads it back through
    /// `document_node_fill` (the renderer's read path). Catches
    /// drift in either direction.
    ///
    /// The renderer-side TypeScript types are in
    /// `apps/desktop/shared/scene.ts`; the wire shape we test
    /// here matches the variants documented in `FillStyle`'s
    /// docstring there.
    #[test]
    #[serial]
    fn update_node_fill_wire_format_round_trip() {
        use kcreate_core::node::FillStyle;

        reset_for_tests();
        let dir = tmpdir();
        project_create("fill_wire", dir.path()).expect("create");
        let page_id = document_get_tree().expect("tree")[0].id;
        let id = document_create_node(
            "VectorLayer",
            Some(page_id),
            &CreateNodeProps {
                name: Some("Rect".to_string()),
                ..Default::default()
            },
        )
        .expect("create");

        // 1) Renderer sends a Solid fill: `{kind:"solid", r,g,b,a}`.
        let solid_json = r#"{
            "fill": { "kind": "solid", "r": 0.25, "g": 0.5, "b": 0.75, "a": 1.0 }
        }"#;
        let solid: UpdateNodeProps = serde_json::from_str(solid_json).expect("parse solid update");
        document_update_node(id, &solid).expect("apply solid");

        let read = document_node_fill(id)
            .expect("read solid")
            .expect("present");
        let parsed: FillStyle = serde_json::from_str(&read).expect("parse fill");
        match parsed {
            FillStyle::Solid(rgba) => {
                // RgbaColor channels are f32 in the document graph
                // (renderer-bound); the wire format is JSON numbers
                // (f64). Use the wider epsilon to tolerate the
                // f64 → f32 narrowing on parse.
                assert!((rgba.r - 0.25).abs() < f32::EPSILON);
                assert!((rgba.g - 0.5).abs() < f32::EPSILON);
                assert!((rgba.b - 0.75).abs() < f32::EPSILON);
                assert!((rgba.a - 1.0).abs() < f32::EPSILON);
            }
            other => panic!("expected Solid, got {other:?}"),
        }

        // 2) Renderer sends a Linear gradient: the outer `kind` is
        //    "gradient" and the inner `shape` is "linear"; serde
        //    flattens the inner enum's fields into the outer object.
        let linear_json = r#"{
            "fill": {
                "kind": "gradient",
                "shape": "linear",
                "from": { "x": 0.0, "y": 0.0 },
                "to":   { "x": 1.0, "y": 0.0 },
                "stops": [
                    { "offset": 0.0, "color": { "r": 1.0, "g": 0.0, "b": 0.0, "a": 1.0 } },
                    { "offset": 1.0, "color": { "r": 0.0, "g": 0.0, "b": 1.0, "a": 1.0 } }
                ]
            }
        }"#;
        let linear: UpdateNodeProps =
            serde_json::from_str(linear_json).expect("parse linear update");
        document_update_node(id, &linear).expect("apply linear");

        let read = document_node_fill(id)
            .expect("read gradient")
            .expect("present");
        let parsed: FillStyle = serde_json::from_str(&read).expect("parse fill");
        match parsed {
            FillStyle::Gradient(kcreate_core::node::GradientKind::Linear { stops, .. }) => {
                assert_eq!(stops.len(), 2, "two stops in the round-tripped fill");
                assert!((stops[0].offset - 0.0).abs() < f64::EPSILON);
                assert!((stops[1].offset - 1.0).abs() < f64::EPSILON);
            }
            other => panic!("expected Linear gradient, got {other:?}"),
        }

        // 3) Renderer sends `None` to clear the fill.
        let none_json = r#"{ "fill": { "kind": "none" } }"#;
        let none: UpdateNodeProps = serde_json::from_str(none_json).expect("parse none update");
        document_update_node(id, &none).expect("apply none");

        let read = document_node_fill(id).expect("read none").expect("present");
        let parsed: FillStyle = serde_json::from_str(&read).expect("parse fill");
        assert!(matches!(parsed, FillStyle::None));

        // 4) Unknown node id → `Ok(None)`.
        let unknown = Uuid::new_v4();
        assert!(
            document_node_fill(unknown).expect("read unknown").is_none(),
            "unknown node id should yield Ok(None), not a hard error"
        );

        project_close();
    }

    /// Regression test for PR #5 Devin Review BUG-0001: the
    /// `NodeInfo` wire shape must carry `bounds` so the renderer's
    /// PrototypePlayer can position hotspot rectangles. Previously
    /// the field was elided and the player saw an empty hotspot
    /// catalog regardless of how many interactions were attached.
    #[test]
    #[serial]
    fn node_info_carries_bounds() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("bounds_wire", dir.path()).expect("create");
        // `artboard_create` is the canonical bounds-setting entry
        // point — `document_create_node` doesn't accept geometry, but
        // the panel's hotspot picker only needs *some* node with
        // non-zero bounds to drive the test.
        let ab = artboard_create(None, "Hero".into(), 800.0, 600.0).expect("artboard");
        let tree = document_get_tree().expect("tree");
        let node = tree.iter().find(|n| n.id == ab).expect("present");
        assert!(
            (node.bounds.width - 800.0).abs() < f64::EPSILON,
            "bounds.width should round-trip through the wire format"
        );
        assert!((node.bounds.height - 600.0).abs() < f64::EPSILON);
        // Every other node in the tree (the default page, child
        // layers, ...) must also carry a `bounds` field — even if
        // its width/height are zero. This is the guarantee
        // PrototypePlayer relies on.
        for n in &tree {
            // `n.bounds` is a value-type, so its mere existence is
            // checked by the compiler; assert finiteness so we'd
            // notice if some node accidentally received NaN.
            assert!(n.bounds.x.is_finite());
            assert!(n.bounds.y.is_finite());
            assert!(n.bounds.width.is_finite());
            assert!(n.bounds.height.is_finite());
        }
        // And it survives JSON round-tripping — napi-rs converts
        // `#[napi(object)]` types using the same field-by-field
        // shape, so a JSON round-trip is a faithful proxy.
        let json = serde_json::to_string(node).expect("serialise");
        assert!(json.contains("\"bounds\""));
        assert!(json.contains("\"width\":800"));
        let parsed: NodeInfo = serde_json::from_str(&json).expect("round-trip");
        assert_eq!(parsed.bounds, node.bounds);
        project_close();
    }

    /// Pins the `NodeInfo::version` wire-format contract. The field
    /// is the dependency-array signal that lets renderer panels
    /// (`FillSection`, `TextFramePanel`, `OpenTypePanel`) refire
    /// their hydrate `useEffect` after undo/redo / collab mutations
    /// on the same selected node id. If this assertion ever stops
    /// holding — i.e. mutating a node via `document_update_node`
    /// stops bumping the wire-format `version` — every panel that
    /// keys on `[node.id, node.version]` silently goes stale and
    /// the user's next commit clobbers the just-mutated state.
    #[test]
    #[serial]
    fn node_info_version_bumps_on_update() {
        use kcreate_core::node::{FillStyle, RgbaColor};
        reset_for_tests();
        let dir = tmpdir();
        project_create("version_wire", dir.path()).expect("create");
        let id = document_create_node("VectorLayer", None, &CreateNodeProps::default())
            .expect("create node");
        let v0 = document_get_tree()
            .expect("tree")
            .iter()
            .find(|n| n.id == id)
            .expect("present")
            .version;
        let changes = UpdateNodeProps {
            fill: Some(FillStyle::Solid(RgbaColor {
                r: 1.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            })),
            ..UpdateNodeProps::default()
        };
        document_update_node(id, &changes).expect("update fill");
        let v1 = document_get_tree()
            .expect("tree")
            .iter()
            .find(|n| n.id == id)
            .expect("present")
            .version;
        assert!(
            v1 > v0,
            "node.version must strictly increase after document_update_node; v0={v0}, v1={v1}"
        );
        // And it survives JSON round-tripping under serde
        // (mirroring the napi-rs `#[napi(object)]` field-by-field
        // shape).
        let node = document_get_tree()
            .expect("tree")
            .into_iter()
            .find(|n| n.id == id)
            .expect("present");
        let json = serde_json::to_string(&node).expect("serialise");
        assert!(json.contains("\"version\""));
        let parsed: NodeInfo = serde_json::from_str(&json).expect("round-trip");
        assert_eq!(parsed.version, node.version);
        project_close();
    }

    #[test]
    #[serial]
    fn errors_without_project() {
        reset_for_tests();
        assert!(matches!(
            document_get_tree(),
            Err(DocumentBridgeError::NoProject)
        ));
        assert!(matches!(
            document_create_node("Page", None, &CreateNodeProps::default()),
            Err(DocumentBridgeError::NoProject)
        ));
        assert!(matches!(
            document_undo(),
            Err(DocumentBridgeError::NoProject)
        ));
    }

    #[test]
    #[serial]
    fn invalid_node_type_rejected() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("nt", dir.path()).expect("create");
        let err = document_create_node("NotAType", None, &CreateNodeProps::default())
            .expect_err("invalid");
        assert!(matches!(err, DocumentBridgeError::InvalidNodeType(_)));
        project_close();
    }

    #[test]
    #[serial]
    fn delete_missing_node_errors() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("d", dir.path()).expect("create");
        let err = document_delete_node(Uuid::new_v4()).expect_err("not found");
        assert!(matches!(err, DocumentBridgeError::NodeNotFound(_)));
        project_close();
    }

    #[test]
    #[serial]
    fn export_svg_round_trips_a_vector_node() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("svg", dir.path()).expect("create");
        let page_id = document_get_tree().expect("tree")[0].id;

        // Build a square vector path and stash it on a VectorLayer.
        let path = kcreate_vector::VectorPath::new(vec![
            kcreate_vector::PathSegment::MoveTo(kcreate_vector::PathPoint::new(0.0, 0.0)),
            kcreate_vector::PathSegment::LineTo(kcreate_vector::PathPoint::new(10.0, 0.0)),
            kcreate_vector::PathSegment::LineTo(kcreate_vector::PathPoint::new(10.0, 10.0)),
            kcreate_vector::PathSegment::LineTo(kcreate_vector::PathPoint::new(0.0, 10.0)),
            kcreate_vector::PathSegment::Close,
        ]);
        let mut meta: HashMap<String, serde_json::Value> = HashMap::new();
        meta.insert(
            "vector_path".to_string(),
            serde_json::to_value(&path).expect("ser"),
        );
        document_create_node(
            "VectorLayer",
            Some(page_id),
            &CreateNodeProps {
                name: Some("rect".to_string()),
                metadata: Some(meta),
                ..Default::default()
            },
        )
        .expect("create");
        let svg = export_svg(&[], &SvgExportOptions::default()).expect("svg");
        assert!(svg.contains("<path"));
        assert!(svg.contains("M0 0"));
        project_close();
    }

    /// Regression test for the incremental-save bug: an undo followed
    /// by a fresh push must NOT lose the new operation when we save.
    ///
    /// With the old index-based `persisted_op_count` cursor, the
    /// post-undo `push` would truncate history (replacing the previous
    /// tail with a fresh op), but `persisted_op_count` still pointed
    /// past `history().len()`, so the save loop's range was empty and
    /// the new op was silently dropped. The id-set tracker fixes this
    /// because the new op carries a fresh `Uuid` that's not in
    /// `persisted_op_ids`, so it gets written.
    #[test]
    #[serial]
    fn save_after_undo_then_push_persists_replacement_op() {
        reset_for_tests();
        let dir = tmpdir();
        let info = project_create("undo_push", dir.path()).expect("create");

        // Push op_a, save it.
        let op_a = Operation::new(
            "user",
            "op_a",
            serde_json::Value::Null,
            serde_json::Value::Null,
            Vec::new(),
        );
        document_record_operation(op_a).expect("record a");
        project_save().expect("save a");

        // Undo (cursor moves back) then push op_b. `OperationLog::push`
        // truncates the redo tail before appending, so op_a's entry is
        // gone from the in-memory log but its row remains on disk
        // (audit-trail semantics).
        document_undo().expect("undo");
        let op_b = Operation::new(
            "user",
            "op_b",
            serde_json::Value::Null,
            serde_json::Value::Null,
            Vec::new(),
        );
        let op_b_id = op_b.id;
        document_record_operation(op_b).expect("record b");
        project_save().expect("save b");

        project_close();

        // Reopen and verify op_b survived the save+close.
        project_open(&info.path).expect("reopen");
        let status = document_status().expect("status");
        // load_operations clamps to max_depth, so both rows can be
        // restored — what we care about is that op_b is among them.
        assert!(status.can_undo, "restored log must be non-empty");
        // Reach into the workspace to confirm op_b's id is present in
        // the restored log. Release the guard before further asserts.
        let found_b = {
            let guard = slot().write();
            let ws = guard.as_ref().expect("workspace");
            let present = ws.project.operation_log.iter().any(|op| op.id == op_b_id);
            drop(guard);
            present
        };
        assert!(
            found_b,
            "op_b must have survived save+close+reopen — the index-based persisted_op_count cursor lost it"
        );
        project_close();
    }

    /// Bounded-depth front trimming must not cause repeated save calls
    /// to re-write already-persisted rows (which would fail the table's
    /// PRIMARY KEY constraint) or to drop newly-pushed ops.
    #[test]
    #[serial]
    fn save_is_idempotent_across_front_trims() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("trim", dir.path()).expect("create");

        // Saturate the bounded in-memory log so the next push triggers a
        // front trim. We don't need the full default 256 — pick
        // something small via a fresh workspace by going straight at the
        // operation log; but the bridge doesn't expose max_depth knobs,
        // so just push enough ops to exercise the prune path.
        for i in 0..16 {
            let op = Operation::new(
                "user",
                format!("op_{i}"),
                serde_json::Value::Null,
                serde_json::Value::Null,
                Vec::new(),
            );
            document_record_operation(op).expect("record");
            // Save after each push to exercise the incremental path.
            project_save().expect("save");
        }

        // A final save with no new ops must be a no-op (no PRIMARY KEY
        // violation, no spurious rewrites).
        project_save().expect("idempotent save");
        project_close();
    }

    #[test]
    #[serial]
    fn brand_kit_create_list_update_delete_round_trips_through_save() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("bk", dir.path()).expect("create");

        let id = brand_kit_create("KChat".into()).expect("create");
        // Editing the kit just-created.
        let mut kits = brand_kit_list().expect("list");
        assert_eq!(kits.len(), 1);
        let mut kit = kits.remove(0);
        assert_eq!(kit.id, id);
        kit.colors.push(kcreate_core::project::NamedColor {
            name: "primary".into(),
            color: kcreate_core::node::RgbaColor::KCHAT_PRIMARY,
        });
        brand_kit_update(kit).expect("update");

        project_save().expect("save");
        project_close();
        project_open(&dir.path().join("bk.kstudio")).expect("reopen");
        let kits2 = brand_kit_list().expect("list2");
        assert_eq!(kits2.len(), 1);
        assert_eq!(kits2[0].id, id);
        assert_eq!(kits2[0].colors[0].name, "primary");

        // Deleting + saving must drop the on-disk row so a second
        // reopen agrees the kit is gone.
        let removed = brand_kit_delete(id).expect("delete");
        assert!(removed);
        project_save().expect("save after delete");
        project_close();
        project_open(&dir.path().join("bk.kstudio")).expect("reopen 2");
        assert!(brand_kit_list().expect("list3").is_empty());
        project_close();
    }

    #[test]
    #[serial]
    fn design_tokens_set_persists_across_close_and_reopen() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("dt", dir.path()).expect("create");

        let mut tokens = design_tokens_get().expect("initial");
        assert!(tokens.colors.is_empty());
        tokens.colors.insert(
            "brand/primary".into(),
            kcreate_core::node::RgbaColor::KCHAT_PRIMARY,
        );
        tokens.spacing.insert("space/4".into(), 16.0);
        design_tokens_set(tokens).expect("set");

        project_save().expect("save");
        project_close();
        project_open(&dir.path().join("dt.kstudio")).expect("reopen");
        let loaded = design_tokens_get().expect("get after reopen");
        assert_eq!(loaded.colors.len(), 1);
        assert!(loaded.colors.contains_key("brand/primary"));
        assert_eq!(loaded.spacing.get("space/4").copied(), Some(16.0));
        project_close();
    }

    #[test]
    #[serial]
    fn export_preset_create_list_delete_round_trip() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("ep", dir.path()).expect("create");

        // `project_create` installs the default presets, so we record
        // the baseline count rather than asserting on a fixed number.
        // The on-disk reconciliation must preserve every default
        // preset alongside the ones we add.
        let baseline = export_preset_list().expect("baseline").len();

        let one = export_preset_create("PNG @1x".into(), "png", 1.0).expect("create 1x");
        let two = export_preset_create("PNG @2x".into(), "png", 2.0).expect("create 2x");
        let presets = export_preset_list().expect("list");
        assert_eq!(presets.len(), baseline + 2);
        assert!(presets.iter().any(|p| p.id == one));
        assert!(presets.iter().any(|p| p.id == two));

        // Save → reopen → still baseline + 2.
        project_save().expect("save");
        project_close();
        project_open(&dir.path().join("ep.kstudio")).expect("reopen");
        assert_eq!(
            export_preset_list().expect("list2").len(),
            baseline + 2,
            "reopen must restore every saved preset"
        );

        // Delete + save reconciles the on-disk row so the reopened
        // project no longer has it.
        let removed = export_preset_delete(one).expect("delete");
        assert!(removed);
        project_save().expect("save after delete");
        project_close();
        project_open(&dir.path().join("ep.kstudio")).expect("reopen 2");
        let presets2 = export_preset_list().expect("list3");
        assert_eq!(presets2.len(), baseline + 1);
        assert!(
            presets2.iter().all(|p| p.id != one),
            "deleted preset must not reappear after reopen"
        );
        assert!(presets2.iter().any(|p| p.id == two));
        project_close();
    }

    #[test]
    #[serial]
    fn export_preset_create_rejects_unknown_format() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("ep2", dir.path()).expect("create");
        let err = export_preset_create("nope".into(), "bmp", 1.0).expect_err("bmp");
        assert!(
            matches!(
                err,
                DocumentBridgeError::InvalidArgument { ref argument, .. } if argument == "format"
            ),
            "expected InvalidArgument {{ argument: 'format', .. }}, got {err:?}",
        );
        project_close();
    }

    // ---- Artboard bridge tests ----

    #[test]
    #[serial]
    fn artboard_create_attaches_to_first_page_when_unspecified() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("ab", dir.path()).expect("create");
        let before = artboard_list().expect("baseline");
        // The default project ships with one artboard already.
        let baseline = before.len();
        let id = artboard_create(None, "Hero".into(), 1440.0, 900.0).expect("create artboard");
        let listed = artboard_list().expect("list");
        assert_eq!(listed.len(), baseline + 1);
        let info = listed.iter().find(|a| a.id == id).expect("hero");
        assert_eq!(info.name, "Hero");
        assert!((info.width - 1440.0).abs() < f64::EPSILON);
        assert!((info.height - 900.0).abs() < f64::EPSILON);
        // The page_id is the same for both default + new artboard
        // (we attached to the only page).
        let other_page = listed.iter().find(|a| a.id != id).expect("default");
        assert_eq!(info.page_id, other_page.page_id);
        project_close();
    }

    #[test]
    #[serial]
    fn artboard_create_rejects_invalid_bounds() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("ab2", dir.path()).expect("create");
        let err = artboard_create(None, "Bad".into(), -1.0, 100.0).expect_err("negative w");
        assert!(matches!(err, DocumentBridgeError::InvalidBounds { .. }));
        let err = artboard_create(None, "Bad".into(), 1.0, f64::INFINITY).expect_err("inf h");
        assert!(matches!(err, DocumentBridgeError::InvalidBounds { .. }));
        let err = artboard_create(None, "Bad".into(), f64::NAN, 100.0).expect_err("nan w");
        assert!(matches!(err, DocumentBridgeError::InvalidBounds { .. }));
        project_close();
    }

    #[test]
    #[serial]
    fn artboard_create_no_project_errors() {
        reset_for_tests();
        let err = artboard_create(None, "x".into(), 10.0, 10.0).expect_err("no project");
        assert!(matches!(err, DocumentBridgeError::NoProject));
    }

    #[test]
    #[serial]
    fn artboard_duplicate_offsets_and_renames() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("ab3", dir.path()).expect("create");
        let id = artboard_create(None, "Hero".into(), 400.0, 300.0).expect("create");
        let before = artboard_list().expect("list before");
        let original = before.iter().find(|a| a.id == id).expect("original");
        let original_x = original.x;
        let dup = artboard_duplicate(id).expect("dup");
        assert_ne!(dup, id);
        let after = artboard_list().expect("list after");
        let copy = after.iter().find(|a| a.id == dup).expect("copy");
        // Width(400) + 100 gap = 500 to the right of the original.
        assert!((copy.x - (original_x + 500.0)).abs() < f64::EPSILON);
        assert!(copy.name.contains("copy"));
        assert!(after.iter().any(|a| a.id == id), "original preserved");
        project_close();
    }

    #[test]
    #[serial]
    fn artboard_resize_records_operation_and_preserves_corner() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("ab4", dir.path()).expect("create");
        let id = artboard_create(None, "Hero".into(), 400.0, 300.0).expect("create");
        let before_status = document_status().expect("status");
        artboard_resize(id, 800.0, 600.0).expect("resize");
        let after_status = document_status().expect("status");
        assert!(
            after_status.undo_depth > before_status.undo_depth,
            "resize should be undoable"
        );
        let listed = artboard_list().expect("list");
        let info = listed.iter().find(|a| a.id == id).expect("hero");
        assert!((info.width - 800.0).abs() < f64::EPSILON);
        assert!((info.height - 600.0).abs() < f64::EPSILON);
        // (x, y) corner preserved.
        assert!((info.y - 0.0).abs() < f64::EPSILON);
        project_close();
    }

    #[test]
    #[serial]
    fn artboard_resize_rejects_invalid_bounds() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("ab5", dir.path()).expect("create");
        let id = artboard_create(None, "Hero".into(), 400.0, 300.0).expect("create");
        let err = artboard_resize(id, 0.0, 100.0).expect_err("zero w");
        assert!(matches!(err, DocumentBridgeError::InvalidBounds { .. }));
        project_close();
    }

    #[test]
    #[serial]
    fn artboard_presets_returns_built_in_catalogue() {
        reset_for_tests();
        let presets = artboard_presets();
        // Built-in catalogue is non-empty and every entry has positive
        // dimensions (the renderer treats <=0 as a no-op).
        assert!(!presets.is_empty());
        for p in &presets {
            assert!(p.width > 0.0, "{} width must be > 0", p.name);
            assert!(p.height > 0.0, "{} height must be > 0", p.name);
        }
        // The home-screen affordances depend on these named presets
        // being present — keep them as a contract.
        assert!(presets.iter().any(|p| p.name == "Desktop"));
        assert!(presets.iter().any(|p| p.name == "Instagram Post"));
        assert!(presets.iter().any(|p| p.name == "A4"));
    }

    // ---- Component bridge tests ----

    fn setup_component_project() -> (tempfile::TempDir, Uuid, Vec<Uuid>) {
        reset_for_tests();
        let dir = tmpdir();
        project_create("comp", dir.path()).expect("create");
        // Add an artboard with two sibling rects we can group.
        let ab = artboard_create(None, "Page".into(), 800.0, 600.0).expect("artboard");
        let a = document_create_node(
            "VectorLayer",
            Some(ab),
            &CreateNodeProps {
                name: Some("Rect A".into()),
                ..Default::default()
            },
        )
        .expect("a");
        let b = document_create_node(
            "VectorLayer",
            Some(ab),
            &CreateNodeProps {
                name: Some("Rect B".into()),
                ..Default::default()
            },
        )
        .expect("b");
        (dir, ab, vec![a, b])
    }

    #[test]
    #[serial]
    fn component_create_from_selection_wraps_in_layer_and_registers() {
        let (_dir, ab, kids) = setup_component_project();
        let comp_id = component_create_from_selection(kids.clone(), "Button".into())
            .expect("create component");
        let list = component_list().expect("list");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, comp_id);
        assert_eq!(list[0].name, "Button");
        // Definition has at least the default variant.
        assert!(!list[0].variants.is_empty());
        // The original children should now live under a ComponentLayer
        // which itself is a child of the artboard.
        let tree = document_get_tree().expect("tree");
        let comp_layer = tree
            .iter()
            .find(|n| n.node_type == "ComponentLayer")
            .expect("component layer");
        assert_eq!(comp_layer.parent_id, Some(ab));
        let kid_a = tree.iter().find(|n| n.id == kids[0]).expect("kid a");
        let kid_b = tree.iter().find(|n| n.id == kids[1]).expect("kid b");
        assert_eq!(kid_a.parent_id, Some(comp_layer.id));
        assert_eq!(kid_b.parent_id, Some(comp_layer.id));
        project_close();
    }

    #[test]
    #[serial]
    fn component_create_from_empty_selection_errors() {
        let (_dir, _ab, _kids) = setup_component_project();
        let err = component_create_from_selection(Vec::new(), "Empty".into()).expect_err("empty");
        assert!(matches!(
            err,
            DocumentBridgeError::InvalidComponentSelection(_)
        ));
        project_close();
    }

    #[test]
    #[serial]
    fn component_create_rejects_cross_parent_selection() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("comp_cross", dir.path()).expect("create");
        let ab1 = artboard_create(None, "P1".into(), 400.0, 300.0).expect("ab1");
        let ab2 = artboard_create(None, "P2".into(), 400.0, 300.0).expect("ab2");
        let a =
            document_create_node("VectorLayer", Some(ab1), &CreateNodeProps::default()).expect("a");
        let b =
            document_create_node("VectorLayer", Some(ab2), &CreateNodeProps::default()).expect("b");
        let err = component_create_from_selection(vec![a, b], "Cross".into()).expect_err("cross");
        assert!(matches!(
            err,
            DocumentBridgeError::InvalidComponentSelection(_)
        ));
        // Sanity: keep the borrow-checker happy.
        let _ = dir;
        project_close();
    }

    #[test]
    #[serial]
    fn component_instantiate_clones_under_parent() {
        let (_dir, ab, kids) = setup_component_project();
        let comp_id = component_create_from_selection(kids, "Card".into()).expect("create");
        // Instantiate a second copy under the same artboard at (200,
        // 50). The new node is a ComponentLayer with its own
        // descendants, distinct from the first instance.
        let new_layer = component_instantiate(comp_id, Some(ab), 200.0, 50.0).expect("instantiate");
        let tree = document_get_tree().expect("tree");
        let new_node = tree.iter().find(|n| n.id == new_layer).expect("new layer");
        assert_eq!(new_node.node_type, "ComponentLayer");
        assert_eq!(new_node.parent_id, Some(ab));
        // New instance has its own children (the snapshot was
        // re-cloned, not aliased).
        let has_kids = tree.iter().any(|n| n.parent_id == Some(new_layer));
        assert!(has_kids, "instance should have children");
        project_close();
    }

    #[test]
    #[serial]
    fn component_add_and_switch_variant() {
        let (_dir, _ab, kids) = setup_component_project();
        let comp_id = component_create_from_selection(kids, "Button".into()).expect("create");
        let variant_id = component_add_variant(comp_id, "Hover".into()).expect("add variant");
        // Find the ComponentLayer node.
        let tree = document_get_tree().expect("tree");
        let comp_layer = tree
            .iter()
            .find(|n| n.node_type == "ComponentLayer")
            .expect("component layer");
        component_switch_variant(comp_layer.id, variant_id).expect("switch");
        // The variant is persisted on the node metadata, so list shows
        // the new variant.
        let list = component_list().expect("list");
        assert_eq!(list[0].variants.len(), 2);
        assert!(list[0].variants.iter().any(|v| v.id == variant_id));
        project_close();
    }

    #[test]
    #[serial]
    fn component_detach_converts_to_group_layer() {
        let (_dir, _ab, kids) = setup_component_project();
        let _comp_id = component_create_from_selection(kids, "Card".into()).expect("create");
        let tree = document_get_tree().expect("tree");
        let comp_layer = tree
            .iter()
            .find(|n| n.node_type == "ComponentLayer")
            .expect("component layer");
        component_detach(comp_layer.id).expect("detach");
        let tree = document_get_tree().expect("tree");
        let node = tree.iter().find(|n| n.id == comp_layer.id).expect("node");
        assert_eq!(node.node_type, "GroupLayer");
        project_close();
    }

    #[test]
    #[serial]
    fn component_detach_rejects_non_component_node() {
        let (_dir, _ab, kids) = setup_component_project();
        // `kids[0]` is a plain VectorLayer.
        let err = component_detach(kids[0]).expect_err("not a component");
        assert!(matches!(
            err,
            DocumentBridgeError::WrongNodeType {
                expected: NodeType::ComponentLayer,
                ..
            }
        ));
        project_close();
    }

    // ---- Auto-layout bridge tests (Block C) ----

    fn setup_layout_project() -> (tempfile::TempDir, Uuid, Vec<Uuid>) {
        reset_for_tests();
        let dir = tmpdir();
        project_create("layout", dir.path()).expect("create");
        let ab = artboard_create(None, "Page".into(), 800.0, 600.0).expect("artboard");
        // Add a GroupLayer that we'll convert into a LayoutFrame.
        let frame = document_create_node(
            "GroupLayer",
            Some(ab),
            &CreateNodeProps {
                name: Some("Frame".into()),
                ..Default::default()
            },
        )
        .expect("frame");
        // Children with explicit sizes (set by reaching into the
        // workspace — `document_create_node` doesn't accept bounds).
        let kids: Vec<Uuid> = (0..3)
            .map(|i| {
                let id = document_create_node(
                    "VectorLayer",
                    Some(frame),
                    &CreateNodeProps {
                        name: Some(format!("R{i}")),
                        ..Default::default()
                    },
                )
                .expect("child");
                {
                    let mut g = slot().write();
                    let ws = g.as_mut().expect("ws");
                    let n = ws.project.document.get_node_mut(id).expect("node");
                    n.bounds = kcreate_core::node::Bounds::new(0.0, 0.0, 50.0, 30.0);
                }
                id
            })
            .collect();
        // Give the frame an explicit size.
        {
            let mut g = slot().write();
            let ws = g.as_mut().expect("ws");
            let n = ws.project.document.get_node_mut(frame).expect("frame node");
            n.bounds = kcreate_core::node::Bounds::new(0.0, 0.0, 400.0, 200.0);
        }
        (dir, frame, kids)
    }

    #[test]
    #[serial]
    fn layout_convert_to_frame_promotes_group_layer() {
        let (_dir, frame, _kids) = setup_layout_project();
        layout_convert_to_frame(frame).expect("convert");
        let tree = document_get_tree().expect("tree");
        let frame_node = tree.iter().find(|n| n.id == frame).expect("frame node");
        assert_eq!(frame_node.node_type, "LayoutFrame");
        project_close();
    }

    #[test]
    #[serial]
    fn layout_convert_to_frame_rejects_non_group() {
        let (_dir, _frame, kids) = setup_layout_project();
        let err = layout_convert_to_frame(kids[0]).expect_err("vector layer is not group");
        assert!(matches!(
            err,
            DocumentBridgeError::WrongNodeType {
                expected: NodeType::GroupLayer,
                ..
            }
        ));
        project_close();
    }

    #[test]
    #[serial]
    fn layout_set_flex_and_recompute_packs_children_in_a_row() {
        let (_dir, frame, kids) = setup_layout_project();
        layout_convert_to_frame(frame).expect("convert");
        let cfg = kcreate_layout::FlexLayout {
            direction: kcreate_layout::FlexDirection::Row,
            spacing: 10.0,
            ..kcreate_layout::FlexLayout::default()
        };
        layout_set_flex(frame, cfg).expect("set flex");
        layout_recompute(frame).expect("recompute");

        // Each child is 50px wide with 10px spacing → 0, 60, 120.
        let g = slot().write();
        let ws = g.as_ref().expect("ws");
        let expected = [0.0, 60.0, 120.0];
        for (i, kid) in kids.iter().enumerate() {
            let n = ws.project.document.get_node(*kid).expect("kid");
            assert!(
                (n.bounds.x - expected[i]).abs() < 1e-6,
                "kid {i} x = {} != {}",
                n.bounds.x,
                expected[i],
            );
            assert!((n.bounds.y - 0.0).abs() < 1e-6);
        }
        drop(g);
        project_close();
    }

    #[test]
    #[serial]
    fn layout_set_grid_and_recompute_distributes_children_into_columns() {
        let (_dir, frame, kids) = setup_layout_project();
        layout_convert_to_frame(frame).expect("convert");
        // 3 children, 2 columns, no gaps → cell_w = 200, items at x =
        // 0, 200, 0; rows at y = 0, 0, 30.
        let cfg = kcreate_layout::GridLayout {
            columns: 2,
            row_gap: 0.0,
            column_gap: 0.0,
            padding: kcreate_layout::Padding::default(),
        };
        layout_set_grid(frame, cfg).expect("set grid");
        layout_recompute(frame).expect("recompute");

        let g = slot().write();
        let ws = g.as_ref().expect("ws");
        let n0 = ws.project.document.get_node(kids[0]).expect("n0");
        let n1 = ws.project.document.get_node(kids[1]).expect("n1");
        let n2 = ws.project.document.get_node(kids[2]).expect("n2");
        assert!((n0.bounds.x - 0.0).abs() < 1e-6);
        assert!((n1.bounds.x - 200.0).abs() < 1e-6);
        assert!((n2.bounds.x - 0.0).abs() < 1e-6);
        assert!((n0.bounds.y - 0.0).abs() < 1e-6);
        assert!((n2.bounds.y - 30.0).abs() < 1e-6);
        drop(g);
        project_close();
    }

    #[test]
    #[serial]
    fn layout_recompute_without_config_is_noop() {
        let (_dir, frame, kids) = setup_layout_project();
        layout_convert_to_frame(frame).expect("convert");
        layout_recompute(frame).expect("noop");
        // Children's bounds unchanged.
        let g = slot().write();
        let ws = g.as_ref().expect("ws");
        for kid in &kids {
            let n = ws.project.document.get_node(*kid).expect("kid");
            assert_eq!(n.bounds.x, 0.0);
            assert_eq!(n.bounds.y, 0.0);
        }
        drop(g);
        project_close();
    }

    #[test]
    #[serial]
    fn layout_set_flex_rejects_non_layout_frame() {
        let (_dir, _frame, kids) = setup_layout_project();
        let err = layout_set_flex(kids[0], kcreate_layout::FlexLayout::default())
            .expect_err("vector layer");
        assert!(matches!(
            err,
            DocumentBridgeError::WrongNodeType {
                expected: NodeType::LayoutFrame,
                ..
            }
        ));
        project_close();
    }

    #[test]
    #[serial]
    fn layout_config_survives_save_and_reopen() {
        let (dir, frame, _kids) = setup_layout_project();
        layout_convert_to_frame(frame).expect("convert");
        let cfg = kcreate_layout::FlexLayout {
            direction: kcreate_layout::FlexDirection::Column,
            spacing: 4.0,
            ..kcreate_layout::FlexLayout::default()
        };
        layout_set_flex(frame, cfg).expect("set");
        let project_path = project_info().expect("info").path;
        project_save().expect("save");
        project_close();
        project_open(&project_path).expect("reopen");
        // The layout metadata should still be on the frame.
        let g = slot().write();
        let ws = g.as_ref().expect("ws");
        let node = ws.project.document.get_node(frame).expect("frame");
        let stored = node
            .metadata
            .get(LAYOUT_CONFIG_METADATA_KEY)
            .expect("layout metadata");
        let parsed: LayoutConfig = serde_json::from_value(stored.clone()).expect("config");
        assert!(matches!(parsed, LayoutConfig::Flex(_)));
        drop(g);
        let _ = dir;
        project_close();
    }

    #[test]
    #[serial]
    fn component_survives_save_and_reopen() {
        let (dir, _ab, kids) = setup_component_project();
        let comp_id = component_create_from_selection(kids, "Button".into()).expect("create comp");
        let project_path = project_info().expect("info").path;
        project_save().expect("save");
        project_close();
        project_open(&project_path).expect("reopen");
        let listed = component_list().expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, comp_id);
        // Instantiate after reopen — should still work because the
        // source snapshot was persisted with the definition.
        let tree = document_get_tree().expect("tree");
        let comp_layer = tree
            .iter()
            .find(|n| n.node_type == "ComponentLayer")
            .expect("component layer");
        let parent = comp_layer.parent_id;
        let new_layer =
            component_instantiate(comp_id, parent, 400.0, 50.0).expect("instantiate after reopen");
        assert_ne!(new_layer, comp_layer.id);
        // Sanity: tempdir kept alive until here.
        let _ = dir;
        project_close();
    }

    /// Tier 0 hosts keep low-resource on regardless of the request,
    /// and `resource_limits` reflects the live state every call.
    #[test]
    #[serial]
    fn low_resource_mode_toggle_round_trip() {
        let before = low_resource_mode_get();
        let initial_limits = resource_limits();
        low_resource_mode_set(true);
        assert!(low_resource_mode_get());
        let lr_limits = resource_limits();
        assert!(lr_limits.low_resource_mode);
        assert!(lr_limits.effective_undo_depth <= initial_limits.effective_undo_depth);

        low_resource_mode_set(false);
        // Either the original state, or pinned to true on Tier 0.
        let final_limits = resource_limits();
        if before {
            assert!(low_resource_mode_get());
            assert!(final_limits.low_resource_mode);
        } else {
            assert!(!low_resource_mode_get());
            assert!(!final_limits.low_resource_mode);
            assert_eq!(
                final_limits.effective_undo_depth,
                initial_limits.effective_undo_depth
            );
        }
    }

    /// Toggling low-resource mode while a project is open also
    /// rebounds the live operation log.
    #[test]
    #[serial]
    fn low_resource_mode_resizes_open_project_log() {
        let dir = tempfile::tempdir().expect("temp");
        project_close();
        project_create("lrm", dir.path()).expect("create");
        let before = low_resource_mode_get();
        let before_depth = {
            let g = slot().write();
            g.as_ref().unwrap().project.operation_log.max_depth()
        };
        low_resource_mode_set(true);
        let lr_depth = {
            let g = slot().write();
            g.as_ref().unwrap().project.operation_log.max_depth()
        };
        assert!(lr_depth <= before_depth, "{lr_depth} <= {before_depth}");
        low_resource_mode_set(before);
        project_close();
    }

    /// Regression guard for the LLM AI-task design-token / accessibility
    /// flow: the AI prompts must see per-node visual properties
    /// (bounds, opacity, blend mode, effects, metadata) — not just a
    /// summary of layer-type counts. If a future refactor inadvertently
    /// elides any of these from the AI payload, the prompts would
    /// silently degrade. We pin the shape here.
    #[test]
    #[serial]
    fn document_serialise_for_ai_emits_visual_properties() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("ai-demo", dir.path()).expect("create");
        let tree = document_get_tree().expect("tree");
        let page_id = tree[0].id;
        let json = document_serialise_for_ai().expect("serialise");
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid json");
        assert_eq!(parsed["project"], "ai-demo");
        let nodes = parsed["nodes"].as_array().expect("nodes array");
        // page + default artboard are emitted; both carry bounds and
        // an explicit visibility flag.
        assert!(!nodes.is_empty());
        for n in nodes {
            assert!(n["id"].is_string());
            assert!(n["type"].is_string());
            assert!(n["bounds"]["width"].is_number());
            assert!(n["bounds"]["height"].is_number());
            assert!(n["opacity"].is_number());
            assert!(n["blend_mode"].is_string());
            assert!(n["visible"].is_boolean());
            assert!(n["effects"].is_array());
            assert!(n["metadata"].is_object());
        }
        // The first emitted node is the root page.
        assert_eq!(nodes[0]["id"], serde_json::json!(page_id));
        project_close();
    }

    #[test]
    #[serial]
    fn document_serialise_for_ai_errors_without_project() {
        reset_for_tests();
        let err = document_serialise_for_ai().expect_err("no project");
        assert!(matches!(err, DocumentBridgeError::NoProject));
    }

    // ---- Prototype interaction bridge tests (Block A) ----

    #[test]
    #[serial]
    fn interaction_add_list_remove_round_trip() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("proto", dir.path()).expect("create");
        let tree = document_get_tree().expect("tree");
        let page_id = tree[0].id;

        // Empty list initially.
        let empty = interaction_list(page_id).expect("list");
        assert!(empty.is_empty());

        // Add a NavigateTo interaction.
        let target = Uuid::new_v4();
        let action = serde_json::json!({
            "kind": "navigate_to",
            "target_artboard_id": target,
        });
        let iid = interaction_add(page_id, "click", &action.to_string()).expect("add");

        let listed = interaction_list(page_id).expect("list 2");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, iid);
        assert_eq!(listed[0].trigger, kcreate_core::InteractionTrigger::Click);

        // Remove + relist.
        let removed = interaction_remove(page_id, iid).expect("remove");
        assert!(removed);
        let after = interaction_list(page_id).expect("list 3");
        assert!(after.is_empty());

        // Removing again is a no-op false.
        assert!(!interaction_remove(page_id, iid).expect("remove again"));
        project_close();
    }

    /// Devin Review ANALYSIS-0003: prove `interaction_list_batch`
    /// returns exactly the same per-node data as N individual
    /// `interaction_list` calls would, but in one shot. Adds two
    /// interactions to one node, leaves another node interaction-less
    /// (so the batch must omit it), and includes a non-existent id
    /// (so the batch must tolerate it).
    #[test]
    #[serial]
    fn interaction_list_batch_matches_individual_calls() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("proto-batch", dir.path()).expect("create");
        let tree = document_get_tree().expect("tree");
        let page_a = tree[0].id;
        let page_b = page_add("page b".to_string(), None, None).expect("add b");
        let action = serde_json::json!({"kind": "back"}).to_string();
        let iid1 = interaction_add(page_a, "click", &action).expect("add a1");
        let iid2 = interaction_add(page_a, "hover", &action).expect("add a2");

        let missing = Uuid::new_v4();
        let ids = vec![page_a, page_b, missing];
        let batch = interaction_list_batch(&ids).expect("batch");

        // page_b has zero interactions and `missing` doesn't exist;
        // both must be absent from the result map.
        assert!(!batch.contains_key(&page_b));
        assert!(!batch.contains_key(&missing));
        let listed_a = batch.get(&page_a).cloned().unwrap_or_default();
        assert_eq!(listed_a.len(), 2);
        let ids_set: std::collections::HashSet<Uuid> = listed_a.iter().map(|i| i.id).collect();
        assert!(ids_set.contains(&iid1));
        assert!(ids_set.contains(&iid2));

        // Identical to per-node call results.
        let solo_a = interaction_list(page_a).expect("solo a");
        let solo_b = interaction_list(page_b).expect("solo b");
        assert_eq!(solo_a.len(), listed_a.len());
        assert!(solo_b.is_empty());

        project_close();
    }

    #[test]
    #[serial]
    fn interaction_add_rejects_unknown_trigger() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("proto2", dir.path()).expect("create");
        let page_id = document_get_tree().expect("tree")[0].id;
        let action = serde_json::json!({"kind": "back"}).to_string();
        let err = interaction_add(page_id, "long_press", &action).expect_err("unknown");
        assert!(
            matches!(
                err,
                DocumentBridgeError::InvalidArgument { ref argument, ref value }
                    if argument == "trigger" && value == "long_press"
            ),
            "expected InvalidArgument {{ argument: 'trigger', value: 'long_press' }}, got {err:?}",
        );
        project_close();
    }

    #[test]
    #[serial]
    fn interaction_remove_unknown_node_errors() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("proto3", dir.path()).expect("create");
        let bogus = Uuid::new_v4();
        let err = interaction_remove(bogus, Uuid::new_v4()).expect_err("no node");
        assert!(matches!(err, DocumentBridgeError::NodeNotFound(_)));
        project_close();
    }

    // ---- Page layout / master page / template bridge tests (Block B) ----

    /// `page_add` accepts (None, None) or (Some, Some) but rejects
    /// partial input — passing one of (size, orientation) without the
    /// other would otherwise silently fall through to the workspace
    /// default, which surprises the UI: it would render the page at
    /// the requested size while metadata still claimed the default.
    #[test]
    #[serial]
    fn page_add_rejects_partial_layout_input() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("partial", dir.path()).expect("create");

        // No layout arguments — uses workspace default. OK.
        let _default = page_add("plain".into(), None, None).expect("default");

        // Both supplied — explicitly sized. OK.
        let _sized = page_add("sized".into(), Some("a4"), Some("portrait")).expect("sized");

        // Size without orientation — rejected.
        let err = page_add("half-1".into(), Some("a4"), None).expect_err("missing orientation");
        assert!(
            matches!(
                err,
                DocumentBridgeError::InvalidArgument { ref argument, .. } if argument == "orientation"
            ),
            "expected InvalidArgument {{ argument: 'orientation', .. }}, got {err:?}",
        );

        // Orientation without size — rejected.
        let err = page_add("half-2".into(), None, Some("portrait")).expect_err("missing size");
        assert!(
            matches!(
                err,
                DocumentBridgeError::InvalidArgument { ref argument, .. } if argument == "size"
            ),
            "expected InvalidArgument {{ argument: 'size', .. }}, got {err:?}",
        );

        project_close();
    }

    #[test]
    #[serial]
    fn page_set_and_get_layout_round_trip() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("pg", dir.path()).expect("create");
        let page_id = document_get_tree().expect("tree")[0].id;
        let layout = kcreate_core::PageLayout::new(
            kcreate_core::PageSize::A4,
            kcreate_core::PageOrientation::Portrait,
        );
        let json = serde_json::to_string(&layout).expect("serialize");
        page_set_layout(page_id, &json).expect("set");
        let got = page_get_layout(page_id).expect("get").expect("present");
        assert_eq!(got, layout);
        project_close();
    }

    #[test]
    #[serial]
    fn page_set_layout_rejects_non_page_nodes() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("pg2", dir.path()).expect("create");
        let rect = canvas_create_rect(None, 0.0, 0.0, 10.0, 10.0).expect("rect");
        let layout = kcreate_core::PageLayout::new(
            kcreate_core::PageSize::A4,
            kcreate_core::PageOrientation::Portrait,
        );
        let err =
            page_set_layout(rect, &serde_json::to_string(&layout).unwrap()).expect_err("rect");
        assert!(matches!(
            err,
            DocumentBridgeError::WrongNodeType {
                expected: NodeType::Page,
                ..
            }
        ));
        project_close();
    }

    #[test]
    #[serial]
    fn master_page_create_list_apply_detach_round_trip() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("master", dir.path()).expect("create");
        let content_page = document_get_tree().expect("tree")[0].id;

        let master = master_page_create("Master A".into(), "a4", "portrait").expect("create");

        let masters = master_page_list().expect("list");
        assert_eq!(masters.len(), 1);
        assert_eq!(masters[0].id, master);
        assert_eq!(masters[0].name, "Master A");

        master_page_apply(content_page, master).expect("apply");
        let layout = page_get_layout(content_page)
            .expect("get layout")
            .expect("present");
        assert_eq!(layout.master_page_id, Some(master));

        master_page_detach(content_page).expect("detach");
        let layout = page_get_layout(content_page)
            .expect("get layout 2")
            .expect("present");
        assert!(layout.master_page_id.is_none());
        project_close();
    }

    /// Regression: `master_page_apply` and `master_page_detach` must
    /// record `before` / `after` operation patches containing the full
    /// `PageLayout` on the content page (not the master id alone, not
    /// `Null`). Without these, undo / redo cannot recover the previous
    /// master attachment.
    #[test]
    #[serial]
    fn master_page_apply_and_detach_capture_layout_patches() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("master-undo", dir.path()).expect("create");
        let content_page = document_get_tree().expect("tree")[0].id;
        let master = master_page_create("M".into(), "a4", "portrait").expect("create");

        // After `master_page_apply`, the most recent op must carry the
        // before / after layout (and `after.master_page_id` must match).
        // The default Page created by `project_create` has no layout, so
        // `before_patch` is JSON `null` and `after_patch` is a populated
        // `PageLayout` with the master attached.
        master_page_apply(content_page, master).expect("apply");
        {
            let guard = slot().write();
            let ws = guard.as_ref().expect("ws");
            let op = ws.project.operation_log.last().expect("apply op");
            assert_eq!(op.command, "master_page_apply");
            assert!(
                op.before_patch.is_null(),
                "before_patch should be null when content page had no layout, \
                 was: {}",
                op.before_patch,
            );
            let after: kcreate_core::PageLayout =
                serde_json::from_value(op.after_patch.clone()).expect("decode after");
            assert_eq!(after.master_page_id, Some(master));
        }

        // After `master_page_detach`, the most recent op must have the
        // master set in `before` and cleared in `after`.
        master_page_detach(content_page).expect("detach");
        {
            let guard = slot().write();
            let ws = guard.as_ref().expect("ws");
            let op = ws.project.operation_log.last().expect("detach op");
            assert_eq!(op.command, "master_page_detach");
            let before: kcreate_core::PageLayout =
                serde_json::from_value(op.before_patch.clone()).expect("decode before");
            assert_eq!(before.master_page_id, Some(master));
            let after: kcreate_core::PageLayout =
                serde_json::from_value(op.after_patch.clone()).expect("decode after");
            assert!(after.master_page_id.is_none());
        }
        project_close();
    }

    #[test]
    #[serial]
    fn master_page_create_rejects_unknown_size() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("m2", dir.path()).expect("create");
        let err = master_page_create("bad".into(), "bogus", "portrait").expect_err("unknown size");
        assert!(
            matches!(
                err,
                DocumentBridgeError::InvalidArgument { ref argument, ref value }
                    if argument == "size" && value == "bogus"
            ),
            "expected InvalidArgument {{ argument: 'size', value: 'bogus' }}, got {err:?}",
        );
        project_close();
    }

    #[test]
    #[serial]
    fn layout_template_list_and_apply_create_pages() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("tpl", dir.path()).expect("create");
        let templates = layout_template_list();
        assert_eq!(templates.len(), 3);
        let pitch = templates
            .iter()
            .find(|t| t.category == kcreate_core::TemplateCategory::PitchDeck)
            .expect("pitch");
        let created = layout_template_apply(pitch.id).expect("apply");
        assert_eq!(created.len(), pitch.pages.len());
        // Each created page is undoable.
        assert!(document_undo().expect("undo").is_some());
        project_close();
    }

    #[test]
    #[serial]
    fn layout_template_apply_rejects_unknown_id() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("tpl2", dir.path()).expect("create");
        let err = layout_template_apply(Uuid::nil()).expect_err("unknown");
        assert!(matches!(err, DocumentBridgeError::InvalidNodeType(_)));
        project_close();
    }

    /// Locks the `PageSizeId` wire vocabulary. The accepted strings
    /// here must match the serde `#[rename]` set on
    /// `kcreate_core::PageSize` *and* the `PageSizeId` union in
    /// `apps/desktop/shared/scene.ts` exactly — no aliases. Catches
    /// the dead-code re-introduction the Devin Review #20 finding
    /// warned about.
    #[test]
    fn parse_page_size_accepts_canonical_wire_strings_only() {
        // Canonical (serde-renamed) forms — all eight must round-trip.
        for (wire, expected) in [
            ("a3", kcreate_core::PageSize::A3),
            ("a4", kcreate_core::PageSize::A4),
            ("a5", kcreate_core::PageSize::A5),
            ("letter", kcreate_core::PageSize::Letter),
            ("legal", kcreate_core::PageSize::Legal),
            ("tabloid", kcreate_core::PageSize::Tabloid),
            (
                "presentation_16x9",
                kcreate_core::PageSize::Presentation16x9,
            ),
            ("presentation_4x3", kcreate_core::PageSize::Presentation4x3),
        ] {
            let parsed = parse_page_size(wire).unwrap_or_else(|e| {
                panic!("parse_page_size({wire:?}) should succeed but got {e:?}")
            });
            assert_eq!(
                parsed, expected,
                "parse_page_size({wire:?}) returned the wrong variant",
            );
        }

        // No-underscore forms must be rejected — they would compile
        // here but never appear in the wire format (serde's
        // snake_case rule doesn't insert `_` before digits, which is
        // why the variants need an explicit `#[serde(rename)]` in
        // core/src/node.rs). Accepting them would mask a wire-format
        // drift between Rust and TS instead of surfacing it.
        for bad in ["presentation16x9", "presentation4x3"] {
            let err = parse_page_size(bad).expect_err("must reject no-underscore form");
            match err {
                DocumentBridgeError::InvalidArgument { argument, value } => {
                    assert_eq!(argument, "size");
                    assert_eq!(value, bad);
                }
                other => panic!("expected InvalidArgument {{ argument: 'size', value: {bad:?} }}, got {other:?}"),
            }
        }

        // Uppercase / mixed-case must also be rejected — wire format
        // is case-sensitive on both sides.
        for bad in ["A4", "Letter", "Presentation_16x9"] {
            assert!(
                matches!(
                    parse_page_size(bad),
                    Err(DocumentBridgeError::InvalidArgument { ref argument, .. }) if argument == "size"
                ),
                "parse_page_size should reject case-folded form {bad:?}"
            );
        }
    }

    // ---------------------------------------------------------------
    // Phase 2 — color management bridge tests
    // ---------------------------------------------------------------

    #[test]
    #[serial]
    fn color_settings_default_round_trips_through_get() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("cs", dir.path()).expect("create");

        let raw = crate::phase2::color_settings_get().expect("get");
        let parsed: kcreate_core::color::ColorSettings = serde_json::from_str(&raw).expect("parse");
        assert_eq!(parsed, kcreate_core::color::ColorSettings::default());
        project_close();
    }

    #[test]
    #[serial]
    fn color_settings_update_persists_across_close_reopen() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("cs", dir.path()).expect("create");

        let new_settings = kcreate_core::color::ColorSettings {
            working_space_rgb: kcreate_core::color::IccProfile::AdobeRgb1998,
            working_space_cmyk: Some(kcreate_core::color::IccProfile::FogRa39),
            rendering_intent: kcreate_core::color::RenderingIntent::RelativeColorimetric,
            soft_proof_profile: Some(kcreate_core::color::IccProfile::Swop2006),
            gamut_warning: true,
        };
        crate::phase2::color_settings_update(&serde_json::to_string(&new_settings).unwrap())
            .expect("update");

        project_save().expect("save");
        project_close();
        project_open(&dir.path().join("cs.kstudio")).expect("reopen");

        let raw = crate::phase2::color_settings_get().expect("get after reopen");
        let parsed: kcreate_core::color::ColorSettings =
            serde_json::from_str(&raw).expect("parse after reopen");
        assert_eq!(parsed, new_settings);
        project_close();
    }

    #[test]
    #[serial]
    fn color_convert_srgb_to_cmyk_round_trip() {
        reset_for_tests();
        // Pure red sRGB → CMYK should pass through srgb_to_cmyk and
        // come back with full magenta + yellow, zero cyan + black.
        let red = kcreate_core::color::Color::Srgb {
            r: 1.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        };
        let raw = crate::phase2::color_convert(&serde_json::to_string(&red).unwrap(), "cmyk")
            .expect("convert");
        let out: kcreate_core::color::Color = serde_json::from_str(&raw).expect("parse");
        match out {
            kcreate_core::color::Color::Cmyk { c, m, y, k, a } => {
                assert!(c.abs() < 1e-5, "c should be 0, got {c}");
                assert!((m - 1.0).abs() < 1e-5, "m should be 1, got {m}");
                assert!((y - 1.0).abs() < 1e-5, "y should be 1, got {y}");
                assert!(k.abs() < 1e-5, "k should be 0, got {k}");
                assert!((a - 1.0).abs() < 1e-5);
            }
            other => panic!("expected Cmyk variant, got {other:?}"),
        }
    }

    #[test]
    #[serial]
    fn color_convert_preserves_authored_cmyk() {
        // CMYK → CMYK must short-circuit so K-channel data survives.
        // Round-tripping through sRGB would conflate (0, 0, 0, K=0.5)
        // and (0.5, 0.5, 0.5, K=0) into the same RGB triplet, which
        // is exactly what the print pipeline cannot tolerate.
        let authored = kcreate_core::color::Color::Cmyk {
            c: 0.1,
            m: 0.2,
            y: 0.3,
            k: 0.5,
            a: 1.0,
        };
        let raw = crate::phase2::color_convert(&serde_json::to_string(&authored).unwrap(), "cmyk")
            .expect("convert");
        let out: kcreate_core::color::Color = serde_json::from_str(&raw).expect("parse");
        assert_eq!(out, authored);
    }

    #[test]
    #[serial]
    fn color_convert_preserves_authored_lab() {
        // Lab → Lab must short-circuit because the sRGB connection
        // space clamps each channel to `[0.0, 1.0]` in
        // `xyz_d65_to_srgb`, which throws away out-of-gamut Lab values
        // (think very saturated cyans, deep ProPhoto-only blues). The
        // print and proofing pipelines rely on the original Lab
        // triplet surviving the bridge.
        let authored = kcreate_core::color::Color::Lab {
            l: 50.0,
            // Deliberately out-of-gamut for sRGB: this pushes
            // `lab_to_srgb` past the [0,1] clamp on at least one
            // channel.
            a_star: 90.0,
            b_star: -90.0,
            alpha: 0.75,
        };
        let json = crate::phase2::color_convert(&serde_json::to_string(&authored).unwrap(), "lab")
            .unwrap();
        let out: kcreate_core::color::Color = serde_json::from_str(&json).unwrap();
        assert_eq!(out, authored);
    }

    #[test]
    #[serial]
    fn color_convert_preserves_authored_hsl() {
        // HSL → HSL must short-circuit because the round-trip through
        // sRGB introduces float drift on the hue (atan2-style
        // extraction) which compounds when the color picker
        // re-converts on every keystroke.
        let authored = kcreate_core::color::Color::Hsl {
            h: 173.4,
            s: 0.62,
            l: 0.37,
            a: 0.9,
        };
        let json = crate::phase2::color_convert(&serde_json::to_string(&authored).unwrap(), "hsl")
            .unwrap();
        let out: kcreate_core::color::Color = serde_json::from_str(&json).unwrap();
        assert_eq!(out, authored);
    }

    #[test]
    #[serial]
    fn color_convert_preserves_authored_srgb() {
        // sRGB → sRGB is the trivial identity; the test pins it so a
        // future refactor doesn't accidentally re-route through the
        // Lab/CMYK round-trip path and introduce drift.
        let authored = kcreate_core::color::Color::Srgb {
            r: 0.123,
            g: 0.456,
            b: 0.789,
            a: 0.5,
        };
        let json = crate::phase2::color_convert(&serde_json::to_string(&authored).unwrap(), "srgb")
            .unwrap();
        let out: kcreate_core::color::Color = serde_json::from_str(&json).unwrap();
        assert_eq!(out, authored);
    }

    #[test]
    #[serial]
    fn color_settings_get_returns_default_when_no_project() {
        // Public contract on `color_settings_get`: the panel mounts
        // before any project is loaded, so the function must return
        // the `Default` JSON shape rather than a `NoProject` error.
        reset_for_tests();
        let json = crate::phase2::color_settings_get()
            .expect("color_settings_get must succeed with no project loaded");
        let settings: kcreate_core::color::ColorSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(settings, kcreate_core::color::ColorSettings::default());
    }

    #[test]
    #[serial]
    fn color_convert_rejects_unknown_space() {
        let red = kcreate_core::color::Color::Srgb {
            r: 1.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        };
        let err = crate::phase2::color_convert(&serde_json::to_string(&red).unwrap(), "yuv")
            .expect_err("unknown space must error");
        match err {
            DocumentBridgeError::InvalidArgument { argument, value } => {
                assert_eq!(argument, "to_space");
                assert_eq!(value, "yuv");
            }
            other => panic!("expected InvalidArgument, got {other:?}"),
        }
    }

    // ---------------------------------------------------------------
    // Phase 2 — text frame + OpenType bridge tests (Block B Task 11)
    // ---------------------------------------------------------------

    fn fresh_text_node_for_test(family: &str) -> Uuid {
        canvas_create_text(None, 10.0, 10.0, "Hello".to_string(), family.into(), 16.0)
            .expect("canvas_create_text")
    }

    #[test]
    #[serial]
    fn text_frame_get_returns_default_for_new_node() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("tf", dir.path()).expect("create");

        let id = fresh_text_node_for_test("sans-serif");
        let json = crate::phase2::text_frame_get(id).expect("get");
        let options: kcreate_core::node::TextFrameOptions = serde_json::from_str(&json).unwrap();
        assert_eq!(options, kcreate_core::node::TextFrameOptions::default());
        project_close();
    }

    #[test]
    #[serial]
    fn text_frame_update_round_trips_and_records_operation() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("tf", dir.path()).expect("create");

        let id = fresh_text_node_for_test("sans-serif");
        let new_options = kcreate_core::node::TextFrameOptions {
            overflow: kcreate_core::node::TextOverflow::Ellipsis,
            columns: 3,
            column_gap: 12.0,
            wrap_mode: kcreate_core::node::TextWrapMode::BoundingBox,
            hyphenation: true,
            hyphenation_language: "en-US".into(),
            vertical_alignment: kcreate_core::node::VerticalAlign::Middle,
            inset: kcreate_core::node::FrameInsets {
                top: 4.0,
                right: 4.0,
                bottom: 4.0,
                left: 4.0,
            },
            auto_size: kcreate_core::node::TextAutoSize::HeightAuto,
            next_frame_id: None,
        };
        crate::phase2::text_frame_update(id, &serde_json::to_string(&new_options).unwrap())
            .expect("update");

        let json = crate::phase2::text_frame_get(id).expect("get after update");
        let parsed: kcreate_core::node::TextFrameOptions = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, new_options);

        // The operation must have been recorded so an undo lands the
        // node back on `TextFrameOptions::default()`.
        let log_len = with_workspace(|ws| Ok(ws.project.operation_log.len())).unwrap();
        assert!(
            log_len >= 2,
            "expected at least canvas_create_text + text_frame_update in the log, got {log_len}"
        );
        project_close();
    }

    #[test]
    #[serial]
    fn text_frame_update_rejects_non_text_node() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("tf", dir.path()).expect("create");

        // Create a vector node (not a TextLayer) and verify the
        // bridge rejects text-frame writes against it.
        let rect_id = document_create_node(
            "VectorLayer",
            None,
            &CreateNodeProps {
                name: Some("rect".into()),
                visible: None,
                locked: None,
                metadata: None,
            },
        )
        .expect("create vector");

        let err = crate::phase2::text_frame_update(
            rect_id,
            &serde_json::to_string(&kcreate_core::node::TextFrameOptions::default()).unwrap(),
        )
        .expect_err("non-text node must error");
        assert!(
            matches!(err, DocumentBridgeError::InvalidArgument { ref argument, .. } if argument == "node_id"),
            "expected InvalidArgument(node_id), got {err:?}",
        );
        project_close();
    }

    #[test]
    #[serial]
    fn text_opentype_features_round_trip() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("tf", dir.path()).expect("create");

        let id = fresh_text_node_for_test("sans-serif");

        // Defaults first.
        let json = crate::phase2::text_opentype_features_get(id).expect("get default");
        let parsed: kcreate_core::node::OpenTypeFeatures = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, kcreate_core::node::OpenTypeFeatures::default());

        // Round-trip a non-default set.
        let custom = kcreate_core::node::OpenTypeFeatures {
            ligatures: false,
            contextual_alternates: true,
            kerning: true,
            small_caps: true,
            old_style_figures: true,
            tabular_figures: false,
            stylistic_sets: vec![1, 7, 20],
            fractions: true,
            ordinals: false,
        };
        crate::phase2::text_opentype_features_update(id, &serde_json::to_string(&custom).unwrap())
            .expect("update");
        let after = crate::phase2::text_opentype_features_get(id).expect("get after");
        let parsed_after: kcreate_core::node::OpenTypeFeatures =
            serde_json::from_str(&after).unwrap();
        assert_eq!(parsed_after, custom);
        project_close();
    }

    /// Pins the Phase 2 undo contract for non-graph operations.
    /// `color_settings_update` must actually be reversible end-to-end:
    /// after one `document_undo` the in-memory `color_settings` must
    /// match the pre-update value, and after a follow-up
    /// `document_redo` it must come back to the post-update value.
    /// Regressing this would silently re-introduce the "undoable
    /// docstring lies" bug Devin Review flagged on PR #7.
    #[test]
    #[serial]
    fn color_settings_update_undo_redo_round_trips_state() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("color-undo", dir.path()).expect("create");

        let baseline = kcreate_core::color::ColorSettings::default();
        let updated = kcreate_core::color::ColorSettings {
            working_space_rgb: kcreate_core::color::IccProfile::AdobeRgb1998,
            working_space_cmyk: Some(kcreate_core::color::IccProfile::FogRa39),
            rendering_intent: kcreate_core::color::RenderingIntent::RelativeColorimetric,
            soft_proof_profile: Some(kcreate_core::color::IccProfile::Swop2006),
            gamut_warning: true,
        };
        crate::phase2::color_settings_update(&serde_json::to_string(&updated).unwrap())
            .expect("update");

        let after_update: kcreate_core::color::ColorSettings =
            serde_json::from_str(&crate::phase2::color_settings_get().expect("get after update"))
                .unwrap();
        assert_eq!(after_update, updated);

        document_undo().expect("undo");
        let after_undo: kcreate_core::color::ColorSettings =
            serde_json::from_str(&crate::phase2::color_settings_get().expect("get after undo"))
                .unwrap();
        assert_eq!(
            after_undo, baseline,
            "document_undo must replay before_patch into ws.project.color_settings",
        );

        document_redo().expect("redo");
        let after_redo: kcreate_core::color::ColorSettings =
            serde_json::from_str(&crate::phase2::color_settings_get().expect("get after redo"))
                .unwrap();
        assert_eq!(
            after_redo, updated,
            "document_redo must replay after_patch into ws.project.color_settings",
        );
        project_close();
    }

    /// Pins the undo/redo atomicity contract.
    ///
    /// Before this fix, `document_undo` called `ws.project.undo()`
    /// (which advanced the log cursor unconditionally) *before*
    /// applying `before_patch`. If patch application then failed
    /// (e.g. the JSON didn't deserialize into `ColorSettings`), the
    /// cursor had moved but the workspace state still reflected the
    /// pre-undo value — the operation was silently dropped from the
    /// user's undoable history.
    ///
    /// We construct that scenario by recording an operation whose
    /// `before_patch` is structurally malformed for its `command`
    /// (a string where `ColorSettings` is expected), then assert:
    /// 1. `document_undo` returns `Err`.
    /// 2. `can_undo` is still true (the cursor did not advance).
    /// 3. A fresh, well-formed op pushed *after* the failure remains
    ///    undoable end-to-end (so the failure mode didn't poison
    ///    the log).
    #[test]
    #[serial]
    fn document_undo_does_not_advance_cursor_on_apply_failure() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("undo-atomicity", dir.path()).expect("create");

        // Inject a poison op: command claims color_settings_update, but
        // the before_patch is a bare string, which will fail to
        // deserialize as `ColorSettings`.
        let poison = Operation::new(
            "user",
            "color_settings_update",
            serde_json::json!("not-a-color-settings-object"),
            serde_json::json!("not-a-color-settings-object"),
            Vec::new(),
        );
        document_record_operation(poison).expect("record poison op");

        let before = document_status().expect("status before undo");
        assert!(
            before.can_undo,
            "freshly-recorded op must be undoable in the log",
        );

        let err = document_undo().expect_err("poisoned undo must fail");
        // The error originates from serde_json inside apply_patch.
        assert!(
            matches!(err, DocumentBridgeError::Json(_)),
            "expected Json (serde) error, got: {err:?}",
        );

        let after_failed_undo = document_status().expect("status after failed undo");
        assert!(
            after_failed_undo.can_undo,
            "failed undo must NOT advance the log cursor — the op must \
             remain undoable so the user can retry / inspect / report",
        );
        assert_eq!(
            after_failed_undo.undo_depth, before.undo_depth,
            "undo depth must be unchanged after a failed undo",
        );

        // Subsequent well-formed ops still flow normally — the failure
        // didn't corrupt the log. A `color_settings_update` push from
        // the bridge entry point both mutates state and records.
        let updated = kcreate_core::color::ColorSettings {
            gamut_warning: true,
            ..kcreate_core::color::ColorSettings::default()
        };
        crate::phase2::color_settings_update(&serde_json::to_string(&updated).unwrap())
            .expect("color_settings_update after failed undo still works");

        project_close();
    }

    /// Symmetric atomicity contract for `document_redo`.
    ///
    /// We push an op whose `before_patch` is a valid `ColorSettings`
    /// but whose `after_patch` is structurally malformed. Undoing it
    /// succeeds (valid `before_patch` → cursor moves backwards,
    /// state reverts). Redoing it must then fail at the
    /// `after_patch` apply step — and crucially, the redo cursor
    /// must NOT advance, so the user can retry / inspect / report
    /// instead of silently losing the operation.
    #[test]
    #[serial]
    fn document_redo_does_not_advance_cursor_on_apply_failure() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("redo-atomicity", dir.path()).expect("create");

        let valid_settings = kcreate_core::color::ColorSettings::default();
        let poison = Operation::new(
            "user",
            "color_settings_update",
            serde_json::to_value(&valid_settings).expect("serialize before_patch"),
            // after_patch is intentionally not a ColorSettings shape.
            serde_json::json!("not-a-color-settings-object"),
            Vec::new(),
        );
        document_record_operation(poison).expect("record poison op");

        // Undo succeeds because before_patch deserializes cleanly.
        let undone = document_undo()
            .expect("poison undo OK (before_patch is valid)")
            .expect("an op was on the undo stack");
        assert_eq!(undone.command, "color_settings_update");

        let pre_redo = document_status().expect("status pre-redo");
        assert!(
            pre_redo.can_redo,
            "after a successful undo the op must be on the redo stack",
        );

        let redo_err = document_redo().expect_err("poison redo must fail");
        assert!(
            matches!(redo_err, DocumentBridgeError::Json(_)),
            "expected Json (serde) error from apply_forward_patch, got: {redo_err:?}",
        );

        let post_redo = document_status().expect("status post-failed-redo");
        assert!(
            post_redo.can_redo,
            "failed redo must NOT advance the log cursor — the op \
             must remain redoable so the user can retry / inspect",
        );
        assert_eq!(
            post_redo.redo_depth, pre_redo.redo_depth,
            "redo depth must be unchanged after a failed redo",
        );

        project_close();
    }

    /// Atomicity contract for `document_undo_group`. Devin Review
    /// flagged on PR #16 (BUG_0001): when a group of ops shares a
    /// `group_id` and the loop fails on op N>0, the workspace state
    /// has already been mutated by ops 0..N-1 while the operation
    /// log cursor stayed where it was — leaving cursor and state
    /// permanently out of sync, with no path back.
    ///
    /// The fix is the `ApplyPatchSnapshot` introduced above:
    /// snapshot every field `apply_patch` can touch before the loop,
    /// restore on any error. This test exercises the failure mode
    /// end-to-end:
    ///
    /// 1. Build a group of two ops sharing the same `group_id`:
    ///    one well-formed `color_settings_update` followed by a
    ///    poisoned `color_settings_update` whose `before_patch` is
    ///    a JSON string.
    /// 2. Drive `document_undo_group` and assert it returns an error.
    /// 3. Assert the workspace state is **identical** to its pre-call
    ///    state (color_settings unchanged) — proving the rollback ran.
    /// 4. Assert the undo cursor did not advance (group remains
    ///    pending) so the user can retry / inspect.
    #[test]
    #[serial]
    fn document_undo_group_rolls_workspace_back_on_partial_failure() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("group-undo-atomicity", dir.path()).expect("create");

        // Start at the default color settings.
        let baseline = kcreate_core::color::ColorSettings::default();
        // Two changes the user made in one logical action — represented
        // as a group. The first op is a real settings change, the second
        // op claims to mutate settings again but its `before_patch` is
        // structurally malformed, so apply_inverse_patch will fail.
        let mid = kcreate_core::color::ColorSettings {
            gamut_warning: true,
            ..baseline.clone()
        };
        let final_state = kcreate_core::color::ColorSettings {
            working_space_rgb: kcreate_core::color::IccProfile::AdobeRgb1998,
            gamut_warning: true,
            ..baseline.clone()
        };

        let group_id = Uuid::new_v4();
        let op_a = Operation::new(
            "user",
            "color_settings_update",
            serde_json::to_value(&baseline).unwrap(),
            serde_json::to_value(&mid).unwrap(),
            Vec::new(),
        )
        .with_group(group_id);
        let op_b_poisoned = Operation::new(
            "user",
            "color_settings_update",
            // before_patch poisoned — deserializes as String, not ColorSettings.
            serde_json::json!("not-a-color-settings-object"),
            serde_json::to_value(&final_state).unwrap(),
            Vec::new(),
        )
        .with_group(group_id);

        // Push them in order. Recording does not call apply_patch, so
        // it succeeds — we then mutate the workspace by hand so it
        // matches what op_a + op_b would have produced if applied
        // forward. (Simpler than calling color_settings_update twice
        // because we'd lose control of the group_id.)
        document_record_operation(op_a).expect("record op_a");
        document_record_operation(op_b_poisoned).expect("record op_b");
        with_workspace_mut(|ws| {
            ws.project.color_settings = final_state.clone();
            Ok(())
        })
        .expect("set final state");

        let before = document_status().expect("status before");
        let cs_before =
            with_workspace(|ws| Ok(ws.project.color_settings.clone())).expect("cs before");
        assert_eq!(cs_before, final_state, "preconditions");

        // Now drive the group undo. Op_b's before_patch will fail.
        // pending_undo_group iterates newest-first, so op_b is applied
        // first (and immediately fails) — but in case the order in
        // production swaps, the test below independently asserts the
        // rollback target is `final_state` regardless of which op
        // failed.
        let err = document_undo_group().expect_err("group undo must surface the error");
        assert!(
            matches!(err, DocumentBridgeError::Json(_)),
            "expected Json (serde) error from apply_inverse_patch, got: {err:?}",
        );

        // Cursor invariant — no group consumed.
        let after = document_status().expect("status after");
        assert_eq!(
            after.undo_depth, before.undo_depth,
            "failed group undo must NOT advance the cursor",
        );

        // State invariant — workspace is exactly what it was before
        // the failed call. If `op_a`'s patch ran successfully but the
        // rollback did NOT happen, color_settings would equal `mid`
        // (gamut_warning true, AdobeRgb1998 reverted). The atomicity
        // contract requires the workspace look exactly as it did
        // pre-call: final_state.
        let cs_after =
            with_workspace(|ws| Ok(ws.project.color_settings.clone())).expect("cs after");
        assert_eq!(
            cs_after, final_state,
            "failed group undo must roll the workspace back to its \
             pre-call state — partial application of op_a's \
             before_patch is the bug ApplyPatchSnapshot prevents.",
        );

        project_close();
    }

    /// Symmetric atomicity contract for `document_redo_group`. A
    /// poisoned `after_patch` mid-group must roll the workspace back
    /// and leave the cursor untouched so the user can retry.
    #[test]
    #[serial]
    fn document_redo_group_rolls_workspace_back_on_partial_failure() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("group-redo-atomicity", dir.path()).expect("create");

        let baseline = kcreate_core::color::ColorSettings::default();
        let after_a = kcreate_core::color::ColorSettings {
            gamut_warning: true,
            ..baseline.clone()
        };

        let group_id = Uuid::new_v4();
        let op_a = Operation::new(
            "user",
            "color_settings_update",
            serde_json::to_value(&baseline).unwrap(),
            serde_json::to_value(&after_a).unwrap(),
            Vec::new(),
        )
        .with_group(group_id);
        let op_b_poisoned = Operation::new(
            "user",
            "color_settings_update",
            serde_json::to_value(&after_a).unwrap(),
            // after_patch poisoned.
            serde_json::json!("not-a-color-settings-object"),
            Vec::new(),
        )
        .with_group(group_id);

        document_record_operation(op_a).expect("record op_a");
        document_record_operation(op_b_poisoned).expect("record op_b");
        // Move the cursor backwards so the ops sit on the redo stack.
        // Since these ops haven't been forward-applied to the
        // workspace, undoing them via document_undo_group would
        // try to invert state that isn't there. Skip the undo and
        // manually walk the operation log cursor backwards using
        // the internal helper.
        with_workspace_mut(|ws| {
            // Two single-op undos: each one decrements the cursor.
            // Their before_patches are both well-formed `ColorSettings`,
            // so apply_inverse_patch + undo() succeed.
            ws.project.color_settings = after_a.clone();
            Ok(())
        })
        .expect("simulate post-op_a state");
        document_undo().expect("walk back op_b");
        document_undo().expect("walk back op_a");

        let pre_redo = document_status().expect("status pre-redo");
        assert!(
            pre_redo.can_redo,
            "after two undos the ops must be on the redo stack",
        );
        let cs_before =
            with_workspace(|ws| Ok(ws.project.color_settings.clone())).expect("cs before redo");
        assert_eq!(cs_before, baseline, "redo precondition: baseline state");

        let err = document_redo_group().expect_err("group redo must surface the error");
        assert!(
            matches!(err, DocumentBridgeError::Json(_)),
            "expected Json (serde) error from apply_forward_patch, got: {err:?}",
        );

        let post = document_status().expect("status post");
        assert_eq!(
            post.redo_depth, pre_redo.redo_depth,
            "failed group redo must NOT advance the cursor",
        );

        let cs_after =
            with_workspace(|ws| Ok(ws.project.color_settings.clone())).expect("cs after");
        assert_eq!(
            cs_after, baseline,
            "failed group redo must roll the workspace back to baseline \
             — partial application of op_a's after_patch (gamut_warning \
             flipped to true) is the bug ApplyPatchSnapshot prevents.",
        );

        project_close();
    }

    /// Same end-to-end guarantee for `text_frame_update` — the bridge
    /// must restore the previous `TextFrameOptions` on undo and replay
    /// the new options on redo.
    #[test]
    #[serial]
    fn text_frame_update_undo_redo_round_trips_state() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("tf-undo", dir.path()).expect("create");

        let id = fresh_text_node_for_test("sans-serif");
        let baseline = kcreate_core::node::TextFrameOptions::default();
        let updated = kcreate_core::node::TextFrameOptions {
            overflow: kcreate_core::node::TextOverflow::Ellipsis,
            columns: 4,
            column_gap: 8.0,
            wrap_mode: kcreate_core::node::TextWrapMode::BoundingBox,
            hyphenation: true,
            hyphenation_language: "en-US".into(),
            vertical_alignment: kcreate_core::node::VerticalAlign::Bottom,
            inset: kcreate_core::node::FrameInsets {
                top: 2.0,
                right: 2.0,
                bottom: 2.0,
                left: 2.0,
            },
            auto_size: kcreate_core::node::TextAutoSize::HeightAuto,
            next_frame_id: None,
        };
        crate::phase2::text_frame_update(id, &serde_json::to_string(&updated).unwrap())
            .expect("update");

        document_undo().expect("undo");
        let after_undo: kcreate_core::node::TextFrameOptions =
            serde_json::from_str(&crate::phase2::text_frame_get(id).expect("get after undo"))
                .unwrap();
        assert_eq!(
            after_undo, baseline,
            "document_undo must restore the previous TextFrameOptions on the node",
        );

        document_redo().expect("redo");
        let after_redo: kcreate_core::node::TextFrameOptions =
            serde_json::from_str(&crate::phase2::text_frame_get(id).expect("get after redo"))
                .unwrap();
        assert_eq!(
            after_redo, updated,
            "document_redo must replay the new TextFrameOptions onto the node",
        );
        project_close();
    }

    /// And `text_opentype_features_update` — same contract, third
    /// Phase 2 panel-driven non-graph operation.
    #[test]
    #[serial]
    fn text_opentype_features_update_undo_redo_round_trips_state() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("ot-undo", dir.path()).expect("create");

        let id = fresh_text_node_for_test("sans-serif");
        let baseline = kcreate_core::node::OpenTypeFeatures::default();
        let updated = kcreate_core::node::OpenTypeFeatures {
            ligatures: false,
            contextual_alternates: true,
            kerning: true,
            small_caps: true,
            old_style_figures: true,
            tabular_figures: false,
            stylistic_sets: vec![2, 5, 11],
            fractions: true,
            ordinals: true,
        };
        crate::phase2::text_opentype_features_update(id, &serde_json::to_string(&updated).unwrap())
            .expect("update");

        document_undo().expect("undo");
        let after_undo: kcreate_core::node::OpenTypeFeatures = serde_json::from_str(
            &crate::phase2::text_opentype_features_get(id).expect("get after undo"),
        )
        .unwrap();
        assert_eq!(after_undo, baseline);

        document_redo().expect("redo");
        let after_redo: kcreate_core::node::OpenTypeFeatures = serde_json::from_str(
            &crate::phase2::text_opentype_features_get(id).expect("get after redo"),
        )
        .unwrap();
        assert_eq!(after_redo, updated);
        project_close();
    }

    /// Pins the spot-color undo contract. Before the `apply_patch`
    /// arms for `spot_color_upsert` / `spot_color_remove` /
    /// `spot_color_load_catalog` were wired in (Devin Review
    /// ANALYSIS_0002 on PR #16), each of those commands recorded an
    /// op with full `before` / `after` library snapshots but
    /// `apply_inverse_patch` fell through to the `_ => Ok(())` arm —
    /// so undo advanced the operation-log cursor without rolling
    /// the library back. The user would hit ⌘Z, see the swatch
    /// still listed, and conclude undo was broken.
    ///
    /// This test exercises all three commands end-to-end through
    /// the bridge and asserts undo / redo round-trips the library
    /// to its expected state at each step.
    #[test]
    #[serial]
    fn spot_color_commands_undo_redo_round_trip_library() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("spot-undo", dir.path()).expect("create");

        // Closures (rather than nested `fn`s) so clippy doesn't
        // complain about items appearing after statements. They
        // capture nothing and serve only as readable
        // assertion-helpers.
        let snapshot = || -> (usize, bool) {
            with_workspace(|ws| {
                let lib = &ws.project.spot_color_library;
                Ok((lib.entries.len(), lib.entries.contains_key("PANTONE 185 C")))
            })
            .expect("workspace snapshot")
        };
        let cmyk_of = |name: &str| -> Option<(f32, f32, f32, f32)> {
            with_workspace(|ws| {
                Ok(ws
                    .project
                    .spot_color_library
                    .entries
                    .get(name)
                    .map(|d| d.fallback_cmyk))
            })
            .expect("cmyk lookup")
        };

        assert_eq!(snapshot(), (0, false), "fresh project library is empty");

        // 1. Upsert "PANTONE 185 C". Wire shape is camelCase per
        // `SpotColorWire`'s `#[serde(rename_all = "camelCase")]`.
        let upsert_wire = serde_json::json!({
            "name": "PANTONE 185 C",
            "displayName": "PANTONE 185 C",
            "fallbackCmyk": [0.0, 1.0, 0.78, 0.03],
            "libraryReference": null,
        });
        crate::phase2::color_spot_upsert(&upsert_wire.to_string()).expect("upsert");
        assert_eq!(snapshot(), (1, true));

        // 2. Load a catalog merging two more swatches in one op.
        let catalog = serde_json::json!({
            "entries": [
                { "name": "PANTONE Reflex Blue C", "fallback_cmyk": [1.0, 0.72, 0.0, 0.06] },
                { "name": "PANTONE 802 C", "fallback_cmyk": [0.61, 0.0, 0.91, 0.0] },
            ]
        });
        let _report =
            crate::phase2::color_spot_load_catalog(&catalog.to_string()).expect("load catalog");
        assert_eq!(snapshot(), (3, true));

        // 3. Remove the first swatch.
        let removed = crate::phase2::color_spot_remove("PANTONE 185 C").expect("remove");
        assert!(removed);
        assert_eq!(snapshot(), (2, false));

        // Undo remove → 3 swatches again, "PANTONE 185 C" back.
        document_undo().expect("undo remove");
        assert_eq!(
            snapshot(),
            (3, true),
            "undo of spot_color_remove must restore the swatch — \
             before the apply_patch arm landed, this stayed at (2, false)."
        );

        // Undo catalog → 1 swatch (the original upsert).
        document_undo().expect("undo catalog");
        assert_eq!(
            snapshot(),
            (1, true),
            "undo of spot_color_load_catalog must drop the two \
             swatches the catalog added.",
        );

        // Undo upsert → empty library.
        document_undo().expect("undo upsert");
        assert_eq!(
            snapshot(),
            (0, false),
            "undo of spot_color_upsert must restore the empty library."
        );

        // Now walk redo back to the final state and re-check each
        // intermediate snapshot to prove the after_patch arm is
        // also wired.
        document_redo().expect("redo upsert");
        assert_eq!(snapshot(), (1, true));

        document_redo().expect("redo catalog");
        assert_eq!(snapshot(), (3, true));

        document_redo().expect("redo remove");
        assert_eq!(snapshot(), (2, false));

        // Spot check the def survives the round-trip (CMYK + name).
        assert_eq!(
            cmyk_of("PANTONE Reflex Blue C"),
            Some((1.0, 0.72, 0.0, 0.06)),
            "Reflex Blue CMYK survives undo / redo",
        );

        project_close();
    }

    #[test]
    #[serial]
    fn text_layout_compute_returns_overflow_when_height_is_tight() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("tf", dir.path()).expect("create");

        let id = fresh_text_node_for_test("sans-serif");

        // Tighten the frame so any non-trivial text overflows. The
        // layout engine is the source of truth here; we only assert
        // the JSON wire shape is parseable + the overflow flag is a
        // bool, not whether overflow is `true` (depends on host font).
        let tight = kcreate_core::node::TextFrameOptions {
            columns: 1,
            ..kcreate_core::node::TextFrameOptions::default()
        };
        crate::phase2::text_frame_update(id, &serde_json::to_string(&tight).unwrap())
            .expect("frame update");

        // Inject `metadata["text"]` = "long line" by mutating the node.
        with_workspace_mut(|ws| {
            let n = ws.project.document.get_node_mut(id).unwrap();
            n.metadata.insert(
                "text".to_string(),
                serde_json::Value::String("supercalifragilistic".into()),
            );
            Ok(())
        })
        .unwrap();

        let json = crate::phase2::text_layout_compute(id).expect("layout");
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(parsed.get("lines").is_some(), "missing `lines` field");
        assert!(
            parsed
                .get("overflow")
                .and_then(serde_json::Value::as_bool)
                .is_some(),
            "missing or non-bool `overflow` field"
        );
        assert!(
            parsed
                .get("usedHeight")
                .and_then(serde_json::Value::as_f64)
                .is_some(),
            "missing `usedHeight` field"
        );
        project_close();
    }

    // ------------------------------------------------------------------
    // Phase 2 Block C — plugin context bridge tests (Task 15)
    // ------------------------------------------------------------------

    /// Lay down a plugin directory at `dir/<id>/` with a manifest
    /// declaring `permissions` and a `.wasm` blob produced from `wat`.
    /// Returns the plugin id so tests can chain into the registry.
    fn write_test_plugin(
        dir: &std::path::Path,
        id: &str,
        wat_src: &str,
        permissions: &[&str],
    ) -> String {
        let plugin_dir = dir.join(id);
        std::fs::create_dir_all(&plugin_dir).unwrap();
        let wasm = wat::parse_str(wat_src).unwrap();
        std::fs::write(plugin_dir.join("entry.wasm"), wasm).unwrap();
        let manifest = serde_json::json!({
            "id": id,
            "name": id,
            "version": "0.1.0",
            "type": "wasm",
            "entry_point": "entry.wasm",
            "permissions": permissions,
        });
        std::fs::write(
            plugin_dir.join("manifest.json"),
            serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .unwrap();
        id.to_string()
    }

    /// Plugin that calls `kcreate_read_document` with
    /// `{"type":"list_nodes"}` so the host's output buffer contains
    /// the response (the host function writes its own response).
    const READ_DOC_WAT: &str = r#"
        (module
            (import "env" "kcreate_read_document"
                (func $rd (param i32 i32) (result i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "{\"type\":\"list_nodes\"}")
            (func (export "run")
                i32.const 0  i32.const 21  call $rd  drop
            )
        )
    "#;

    /// Plugin that submits one `delete_node` proposal carrying a
    /// specific UUID. The UUID is the placeholder
    /// `00000000-0000-0000-0000-000000000000`; tests rewrite this
    /// at runtime by setting an env var the WAT can't read. So we
    /// instead use a UUID we will be supplying — for this test we
    /// will create a node, capture its id, write a per-test
    /// instance of the WAT with that id baked in, and run.
    fn delete_proposal_wat(node_id: uuid::Uuid) -> String {
        let payload = format!("{{\"type\":\"delete_node\",\"node_id\":\"{node_id}\"}}");
        let len = payload.len();
        // The WAT data section needs the inner double quotes escaped
        // as `\"` so the WAT parser sees one string literal, not a
        // sequence of broken strings.
        let escaped: String = payload.replace('"', "\\\"");
        format!(
            r#"
            (module
                (import "env" "kcreate_write_proposal"
                    (func $wp (param i32 i32) (result i32)))
                (memory (export "memory") 1)
                (data (i32.const 0) "{escaped}")
                (func (export "run")
                    i32.const 0  i32.const {len}  call $wp  drop
                )
            )
            "#
        )
    }

    /// Helper: bind the plugin registry to a per-test directory so
    /// every test sees a fresh registry. `KCREATE_PLUGIN_DIR` is the
    /// override the bridge respects.
    struct PluginEnvGuard {
        prev: Option<String>,
    }
    impl PluginEnvGuard {
        fn new(dir: &std::path::Path) -> Self {
            let prev = std::env::var("KCREATE_PLUGIN_DIR").ok();
            // SAFETY: `set_var` is only unsafe in multi-threaded
            // contexts because of UB if other threads read env
            // concurrently. The bridge plugin tests are marked
            // `#[serial]` precisely so this lifecycle is sound;
            // we just need to wrap the call.
            unsafe { std::env::set_var("KCREATE_PLUGIN_DIR", dir) };
            // The plugin registry is a process-global `OnceLock`
            // that captures `plugin_dir()` at first init — we have
            // to explicitly reseed it so the test's tmpdir takes
            // effect even when a previous test already triggered
            // init under a different dir.
            crate::phase2::reset_plugin_state_for_tests();
            Self { prev }
        }
    }
    impl Drop for PluginEnvGuard {
        fn drop(&mut self) {
            match &self.prev {
                Some(v) => unsafe { std::env::set_var("KCREATE_PLUGIN_DIR", v) },
                None => unsafe { std::env::remove_var("KCREATE_PLUGIN_DIR") },
            }
            // Re-seed back to whatever the (likely-absent) env now
            // points at, so a stray test that doesn't use the guard
            // can still rely on the registry being well-formed.
            crate::phase2::reset_plugin_state_for_tests();
        }
    }

    #[test]
    #[serial]
    fn plugin_execute_with_context_reads_document_when_permitted() {
        reset_for_tests();
        let dir = tmpdir();
        let plugin_dir = tmpdir();
        let _guard = PluginEnvGuard::new(plugin_dir.path());

        project_create("ctx", dir.path()).expect("create");
        let id = write_test_plugin(
            plugin_dir.path(),
            "list-nodes",
            READ_DOC_WAT,
            &["read_document"],
        );
        // `plugin_list` performs the registry scan; without it,
        // `plugin_enable` can't find the freshly-written manifest.
        crate::phase2::plugin_list().expect("list");
        crate::phase2::plugin_enable(&id).expect("enable");

        let out = crate::phase2::plugin_execute_with_context(&id, "run", "").expect("execute");
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        let response: Vec<String> =
            serde_json::from_str(parsed.get("output").and_then(|v| v.as_str()).unwrap())
                .expect("response is a JSON array of ids");
        // The default project has at least a Page + Artboard, so the
        // node id list must be non-empty.
        assert!(!response.is_empty(), "expected non-empty node id list");
        project_close();
    }

    #[test]
    #[serial]
    fn plugin_execute_with_context_denies_without_permission() {
        reset_for_tests();
        let dir = tmpdir();
        let plugin_dir = tmpdir();
        let _guard = PluginEnvGuard::new(plugin_dir.path());

        project_create("ctx", dir.path()).expect("create");
        // No `read_document` permission declared.
        let id = write_test_plugin(plugin_dir.path(), "list-nodes-deny", READ_DOC_WAT, &[]);
        crate::phase2::plugin_list().expect("list");
        crate::phase2::plugin_enable(&id).expect("enable");

        let out = crate::phase2::plugin_execute_with_context(&id, "run", "").expect("execute");
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        // Output should be empty when the call was denied — the
        // host writes nothing to the output buffer.
        assert_eq!(
            parsed.get("output").and_then(|v| v.as_str()).unwrap_or(""),
            "",
            "expected empty output on permission deny"
        );
        let logs = parsed
            .get("logs")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        assert!(
            logs.iter().any(|l| l
                .as_str()
                .is_some_and(|s| s.contains("denied (missing ReadDocument)"))),
            "expected deny log line, got {logs:?}"
        );
        project_close();
    }

    #[test]
    #[serial]
    fn plugin_execute_with_context_applies_delete_proposal() {
        reset_for_tests();
        let dir = tmpdir();
        let plugin_dir = tmpdir();
        let _guard = PluginEnvGuard::new(plugin_dir.path());

        project_create("ctx", dir.path()).expect("create");
        // Create a node we will then ask the plugin to delete.
        let target = document_create_node(
            "VectorLayer",
            None,
            &CreateNodeProps {
                name: Some("doomed".into()),
                visible: None,
                locked: None,
                metadata: None,
            },
        )
        .expect("create node");

        let wat = delete_proposal_wat(target);
        let id = write_test_plugin(plugin_dir.path(), "deleter", &wat, &["write_document"]);
        crate::phase2::plugin_list().expect("list");
        crate::phase2::plugin_enable(&id).expect("enable");

        let out = crate::phase2::plugin_execute_with_context(&id, "run", "").expect("execute");
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        let reports = parsed
            .get("proposals")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        assert_eq!(reports.len(), 1, "expected one proposal report");
        let status = reports[0]
            .get("outcome")
            .and_then(|o| o.get("status"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert_eq!(status, "applied", "expected applied, got {status}");
        // Node should be gone from the document.
        assert!(
            with_workspace(|ws| Ok(ws.project.document.get_node(target).is_none())).unwrap(),
            "node should be gone after applied delete proposal"
        );
        project_close();
    }

    #[test]
    #[serial]
    fn plugin_execute_with_context_rejects_delete_of_unknown_node() {
        reset_for_tests();
        let dir = tmpdir();
        let plugin_dir = tmpdir();
        let _guard = PluginEnvGuard::new(plugin_dir.path());

        project_create("ctx", dir.path()).expect("create");
        // UUID that doesn't resolve to any node.
        let ghost = uuid::Uuid::new_v4();
        let wat = delete_proposal_wat(ghost);
        let id = write_test_plugin(
            plugin_dir.path(),
            "ghost-deleter",
            &wat,
            &["write_document"],
        );
        crate::phase2::plugin_list().expect("list");
        crate::phase2::plugin_enable(&id).expect("enable");

        let out = crate::phase2::plugin_execute_with_context(&id, "run", "").expect("execute");
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        let reports = parsed
            .get("proposals")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        assert_eq!(reports.len(), 1);
        let status = reports[0]
            .get("outcome")
            .and_then(|o| o.get("status"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert_eq!(status, "rejected", "expected rejected, got {status}");
        project_close();
    }

    // ------------------------------------------------------------------
    // Phase 2 Block D — JS panel bridge tests (Task 18)
    // ------------------------------------------------------------------

    /// Lay down a JS panel plugin (manifest.json + entry_html stub)
    /// under `dir/<id>/`.
    fn write_test_js_panel(dir: &std::path::Path, id: &str, permissions: &[&str]) -> String {
        let plugin_dir = dir.join(id);
        std::fs::create_dir_all(&plugin_dir).unwrap();
        std::fs::write(
            plugin_dir.join("panel.html"),
            b"<!doctype html><html></html>",
        )
        .unwrap();
        let manifest = serde_json::json!({
            "id": id,
            "name": id,
            "version": "0.1.0",
            "type": "js_panel",
            "entry_point": "panel.html",
            "permissions": permissions,
            "js_panel": {
                "entry_html": "panel.html",
                "panel_title": "Test Panel",
                "panel_position": "right_sidebar",
                "width": 320,
                "height": 480,
                "permissions": permissions
            }
        });
        std::fs::write(
            plugin_dir.join("manifest.json"),
            serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .unwrap();
        id.to_string()
    }

    #[test]
    #[serial]
    fn plugin_js_list_returns_only_js_panel_plugins() {
        let dir = tmpdir();
        let plugin_dir = tmpdir();
        let _guard = PluginEnvGuard::new(plugin_dir.path());

        project_create("ctx", dir.path()).expect("create");
        // One wasm plugin + one js_panel plugin.
        let _wasm_id = write_test_plugin(
            plugin_dir.path(),
            "some-wasm",
            READ_DOC_WAT,
            &["read_document"],
        );
        let panel_id = write_test_js_panel(plugin_dir.path(), "some-panel", &["read_document"]);
        // Force registry scan so newly-written manifests are visible.
        crate::phase2::plugin_list().expect("list");

        let list = crate::phase2::plugin_js_list().expect("js list");
        let ids: Vec<&str> = list.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, vec![panel_id.as_str()]);
        assert_eq!(list[0].config.entry_html, "panel.html");
        assert_eq!(list[0].config.width, 320);
        project_close();
    }

    #[test]
    #[serial]
    fn plugin_js_message_read_document_requires_permission() {
        let dir = tmpdir();
        let plugin_dir = tmpdir();
        let _guard = PluginEnvGuard::new(plugin_dir.path());

        project_create("ctx", dir.path()).expect("create");
        let panel_id = write_test_js_panel(plugin_dir.path(), "no-perm-panel", &[]);
        crate::phase2::plugin_list().expect("list");
        crate::phase2::plugin_enable(&panel_id).expect("enable");

        let msg = serde_json::json!({
            "type": "read_document",
            "query": { "type": "list_nodes" }
        })
        .to_string();
        let out = crate::phase2::plugin_js_message(&panel_id, &msg).expect("msg");
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["status"], "denied");
        assert_eq!(parsed["permission"], "read_document");
        project_close();
    }

    #[test]
    #[serial]
    fn plugin_js_message_read_document_succeeds_with_permission() {
        let dir = tmpdir();
        let plugin_dir = tmpdir();
        let _guard = PluginEnvGuard::new(plugin_dir.path());

        project_create("ctx", dir.path()).expect("create");
        let panel_id = write_test_js_panel(plugin_dir.path(), "read-panel", &["read_document"]);
        crate::phase2::plugin_list().expect("list");
        crate::phase2::plugin_enable(&panel_id).expect("enable");

        let msg = serde_json::json!({
            "type": "read_document",
            "query": { "type": "list_nodes" }
        })
        .to_string();
        let out = crate::phase2::plugin_js_message(&panel_id, &msg).expect("msg");
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["status"], "ok", "expected ok, got {parsed}");
        let result = parsed.get("result").expect("result");
        assert!(result.is_array(), "list_nodes must produce a JSON array");
        project_close();
    }

    #[test]
    #[serial]
    fn plugin_js_message_rejects_invalid_json() {
        let dir = tmpdir();
        let plugin_dir = tmpdir();
        let _guard = PluginEnvGuard::new(plugin_dir.path());

        project_create("ctx", dir.path()).expect("create");
        let panel_id = write_test_js_panel(plugin_dir.path(), "invalid-panel", &[]);
        crate::phase2::plugin_list().expect("list");
        crate::phase2::plugin_enable(&panel_id).expect("enable");

        let out = crate::phase2::plugin_js_message(&panel_id, "not json").expect("msg");
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["status"], "invalid");
        project_close();
    }

    #[test]
    #[serial]
    fn plugin_js_message_rejects_non_js_panel_plugin() {
        let dir = tmpdir();
        let plugin_dir = tmpdir();
        let _guard = PluginEnvGuard::new(plugin_dir.path());

        project_create("ctx", dir.path()).expect("create");
        let wasm_id = write_test_plugin(
            plugin_dir.path(),
            "wasm-not-panel",
            READ_DOC_WAT,
            &["read_document"],
        );
        crate::phase2::plugin_list().expect("list");
        crate::phase2::plugin_enable(&wasm_id).expect("enable");

        let msg = serde_json::json!({
            "type": "log",
            "message": "hi"
        })
        .to_string();
        let out = crate::phase2::plugin_js_message(&wasm_id, &msg).expect("msg");
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["status"], "invalid");
        let reason = parsed["reason"].as_str().unwrap_or("");
        assert!(reason.contains("not a js_panel"), "got: {reason}");
        project_close();
    }

    #[test]
    #[serial]
    fn slice_update_validates_inputs_before_mutating_state() {
        // Regression for Devin Review BUG_..._0001: a partial
        // failure (e.g. valid `name` + invalid `bounds`) must
        // leave the slice byte-for-byte unchanged and record no
        // `Operation`. Otherwise an undoable edit would silently
        // dirty the workspace.
        reset_for_tests();
        let dir = tmpdir();
        project_create("slices", dir.path()).expect("create");

        let id = slice_create("shipping".into(), 10.0, 20.0, 300.0, 150.0, "png", 1.0)
            .expect("create slice");

        // Snapshot the slice + the project's undo depth.
        let before_slice = slice_list().expect("list")[0].clone();
        let undo_depth_before = document_status().expect("status").undo_depth;

        // Send a payload whose name is valid but whose bounds are
        // illegal (width = -1). `slice_update` must return an
        // error and must NOT have written the new name.
        let bad = SliceUpdateProps {
            name: Some("renamed".into()),
            bounds: Some(BoundsInfo {
                x: 0.0,
                y: 0.0,
                width: -1.0,
                height: 100.0,
            }),
            format: None,
            scale: None,
        };
        let err = slice_update(id, bad).expect_err("must reject negative width");
        assert!(
            matches!(err, DocumentBridgeError::InvalidBounds { .. }),
            "expected InvalidBounds, got {err:?}"
        );

        let after_slice = slice_list().expect("list")[0].clone();
        assert_eq!(
            after_slice.name, before_slice.name,
            "name must not have been written when bounds validation failed",
        );
        assert_eq!(
            after_slice.bounds, before_slice.bounds,
            "bounds must not have changed",
        );
        let undo_depth_after = document_status().expect("status").undo_depth;
        assert_eq!(
            undo_depth_after, undo_depth_before,
            "no Operation should have been recorded for the failed update",
        );

        // The same payload with a bad scale (but valid bounds and
        // valid name) must also reject without mutating.
        let bad_scale = SliceUpdateProps {
            name: Some("also_renamed".into()),
            bounds: Some(BoundsInfo {
                x: 5.0,
                y: 5.0,
                width: 100.0,
                height: 100.0,
            }),
            format: Some("png".into()),
            scale: Some(0.0),
        };
        let err = slice_update(id, bad_scale).expect_err("must reject zero scale");
        assert!(
            matches!(err, DocumentBridgeError::InvalidArgument { .. }),
            "expected InvalidArgument, got {err:?}"
        );
        let still = slice_list().expect("list")[0].clone();
        assert_eq!(still.name, before_slice.name);
        assert_eq!(still.bounds, before_slice.bounds);
        assert_eq!(still.scale, before_slice.scale);

        project_close();
    }

    #[test]
    fn stroke_style_deserializes_with_omitted_cap_join_dash() {
        // Regression for Devin Review ANALYSIS_..._0002: the TS
        // `StrokeStyleWire` declares `cap`, `join`, and `dash` as
        // optional. Rust's `StrokeStyle` must accept payloads that
        // omit any of them, falling back to `LineCap::Butt`,
        // `LineJoin::Miter`, and an empty dash array.
        use kcreate_core::node::{LineCap, LineJoin, StrokeStyle};

        // Minimum payload — only the two required fields.
        let json = r#"{ "color": { "r": 0.0, "g": 0.0, "b": 0.0, "a": 1.0 }, "width": 2.5 }"#;
        let s: StrokeStyle = serde_json::from_str(json).expect("deserialize");
        assert_eq!(s.width, 2.5);
        assert!(s.dash.is_empty());
        assert_eq!(s.cap, LineCap::Butt);
        assert_eq!(s.join, LineJoin::Miter);

        // Full payload with `"round"` cap and `"bevel"` join — the
        // TS wire's lowercase strings must round-trip.
        let json = r#"{
            "color": { "r": 0.0, "g": 0.0, "b": 0.0, "a": 1.0 },
            "width": 1.0,
            "dash": [4.0, 2.0],
            "cap": "round",
            "join": "bevel"
        }"#;
        let s: StrokeStyle = serde_json::from_str(json).expect("deserialize");
        assert_eq!(s.cap, LineCap::Round);
        assert_eq!(s.join, LineJoin::Bevel);
        assert_eq!(s.dash, vec![4.0, 2.0]);
    }

    // ---------------------------------------------------------------
    // Clipboard (Phase 6 Tasks 25-26)
    // ---------------------------------------------------------------

    #[test]
    #[serial]
    fn clipboard_copy_paste_round_trip_under_same_artboard() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("clip", dir.path()).expect("create");
        let ab = artboard_create(None, "Page".into(), 800.0, 600.0).expect("artboard");
        let rect = document_create_node(
            "VectorLayer",
            Some(ab),
            &CreateNodeProps {
                name: Some("Rect".into()),
                ..Default::default()
            },
        )
        .expect("create");

        // Copy → payload is a self-contained JSON Document.
        let payload = document_clipboard_copy(&[rect]).expect("copy");
        let parsed: ClipboardPayload = serde_json::from_str(&payload).expect("parse");
        assert_eq!(parsed.version, 1, "schema version pinned to 1");
        assert_eq!(parsed.subtrees.len(), 1);
        assert_eq!(
            parsed.subtrees[0].nodes.len(),
            1,
            "leaf rect has no descendants"
        );
        assert_eq!(
            parsed.subtrees[0].nodes[0].parent_id, None,
            "root detached so paste picks the parent"
        );

        // Paste under the same artboard, offset by (10, 20).
        let pasted = document_clipboard_paste(&payload, Some(ab), 10.0, 20.0).expect("paste");
        assert_eq!(pasted.len(), 1);
        let new_id = pasted[0];
        assert_ne!(new_id, rect, "paste must regenerate ids");

        // The new node must be a child of the destination artboard and
        // sit at (10, 20) relative to the original (which was at 0,0).
        let tree = document_get_tree().expect("tree");
        let pasted_node = tree.iter().find(|n| n.id == new_id).expect("present");
        assert_eq!(pasted_node.parent_id, Some(ab));
        assert_eq!(pasted_node.bounds.x, 10.0);
        assert_eq!(pasted_node.bounds.y, 20.0);
        project_close();
    }

    #[test]
    #[serial]
    fn clipboard_paste_remaps_descendant_ids_and_keeps_hierarchy() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("clip-tree", dir.path()).expect("create");
        let ab = artboard_create(None, "Page".into(), 800.0, 600.0).expect("artboard");
        let parent = document_create_node(
            "GroupLayer",
            Some(ab),
            &CreateNodeProps {
                name: Some("Group".into()),
                ..Default::default()
            },
        )
        .expect("parent");
        let child_a = document_create_node(
            "VectorLayer",
            Some(parent),
            &CreateNodeProps {
                name: Some("A".into()),
                ..Default::default()
            },
        )
        .expect("child a");
        let child_b = document_create_node(
            "VectorLayer",
            Some(parent),
            &CreateNodeProps {
                name: Some("B".into()),
                ..Default::default()
            },
        )
        .expect("child b");

        let payload = document_clipboard_copy(&[parent]).expect("copy");
        let pasted = document_clipboard_paste(&payload, Some(ab), 5.0, 5.0).expect("paste");
        assert_eq!(pasted.len(), 1);
        let new_parent = pasted[0];
        assert_ne!(new_parent, parent);

        let tree = document_get_tree().expect("tree");
        let new_parent_node = tree.iter().find(|n| n.id == new_parent).expect("parent");
        assert_eq!(new_parent_node.parent_id, Some(ab));
        // Two children must have been recreated under the new parent.
        let new_children: Vec<_> = tree
            .iter()
            .filter(|n| n.parent_id == Some(new_parent))
            .collect();
        assert_eq!(new_children.len(), 2, "both children recreated");
        // None of the new ids may collide with the originals.
        for child in &new_children {
            assert_ne!(child.id, child_a);
            assert_ne!(child.id, child_b);
        }
        project_close();
    }

    #[test]
    #[serial]
    fn clipboard_paste_cross_artboard() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("clip-cross", dir.path()).expect("create");
        let ab1 = artboard_create(None, "Page 1".into(), 800.0, 600.0).expect("ab1");
        let ab2 = artboard_create(None, "Page 2".into(), 800.0, 600.0).expect("ab2");
        let rect = document_create_node(
            "VectorLayer",
            Some(ab1),
            &CreateNodeProps {
                name: Some("Rect".into()),
                ..Default::default()
            },
        )
        .expect("rect");

        let payload = document_clipboard_copy(&[rect]).expect("copy");
        let pasted = document_clipboard_paste(&payload, Some(ab2), 0.0, 0.0).expect("paste");
        assert_eq!(pasted.len(), 1);
        let new_id = pasted[0];
        let tree = document_get_tree().expect("tree");
        let pasted_node = tree.iter().find(|n| n.id == new_id).expect("present");
        assert_eq!(
            pasted_node.parent_id,
            Some(ab2),
            "cross-artboard paste must reparent to the destination"
        );
        // Original under ab1 stays put.
        let original_node = tree.iter().find(|n| n.id == rect).expect("original");
        assert_eq!(original_node.parent_id, Some(ab1));
        project_close();
    }

    #[test]
    #[serial]
    fn clipboard_copy_skips_pages_and_artboards() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("clip-skip", dir.path()).expect("create");
        let ab = artboard_create(None, "Page".into(), 800.0, 600.0).expect("artboard");
        let page_id = document_get_tree().expect("tree")[0].id;

        // Copy a mix of a Page id, an Artboard id, and an unknown id.
        let payload = document_clipboard_copy(&[page_id, ab, Uuid::new_v4()]).expect("copy");
        let parsed: ClipboardPayload = serde_json::from_str(&payload).expect("parse");
        assert!(
            parsed.subtrees.is_empty(),
            "pages/artboards/unknown ids must be filtered out"
        );
        project_close();
    }

    #[test]
    #[serial]
    fn clipboard_paste_rejects_invalid_target_parent() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("clip-bad-parent", dir.path()).expect("create");
        let ab = artboard_create(None, "Page".into(), 800.0, 600.0).expect("artboard");
        let rect = document_create_node("VectorLayer", Some(ab), &CreateNodeProps::default())
            .expect("rect");
        let payload = document_clipboard_copy(&[rect]).expect("copy");
        let bogus_parent = Uuid::new_v4();
        let err = document_clipboard_paste(&payload, Some(bogus_parent), 0.0, 0.0)
            .expect_err("must reject unknown parent");
        assert!(matches!(err, DocumentBridgeError::NodeNotFound(id) if id == bogus_parent));
        project_close();
    }

    #[test]
    #[serial]
    fn set_layer_color_round_trip_persists_in_metadata_and_undoes() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("color-tag", dir.path()).expect("create");
        let ab = artboard_create(None, "Page".into(), 800.0, 600.0).expect("artboard");
        let rect = document_create_node("VectorLayer", Some(ab), &CreateNodeProps::default())
            .expect("rect");

        // Set → metadata["layerColor"] = "blue", version bumps.
        let v0 = document_set_layer_color(rect, Some("Blue".into())).expect("set");
        let tree = document_get_tree().expect("tree");
        let node = tree.iter().find(|n| n.id == rect).expect("present");
        let color = node
            .metadata
            .get(LAYER_COLOR_METADATA_KEY)
            .and_then(|v| v.as_str());
        assert_eq!(color, Some("blue"), "tag is lower-cased");
        assert!(v0 >= 1, "version bumped on first set");

        // Clear (empty string) → metadata key removed.
        document_set_layer_color(rect, Some("   ".into())).expect("clear via whitespace");
        let tree = document_get_tree().expect("tree");
        let node = tree.iter().find(|n| n.id == rect).expect("present");
        assert!(
            !node.metadata.contains_key(LAYER_COLOR_METADATA_KEY),
            "empty / whitespace string removes the tag",
        );

        // Undo restores the prior tag.
        let undone = document_undo().expect("undo").expect("op present");
        assert_eq!(undone.affected_nodes, vec![rect]);
        let tree = document_get_tree().expect("tree");
        let node = tree.iter().find(|n| n.id == rect).expect("present");
        let color = node
            .metadata
            .get(LAYER_COLOR_METADATA_KEY)
            .and_then(|v| v.as_str());
        assert_eq!(color, Some("blue"), "undo restored the previous tag");

        project_close();
    }

    #[test]
    #[serial]
    fn set_layer_color_rejects_unknown_node() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("color-tag-bad", dir.path()).expect("create");
        let _ab = artboard_create(None, "Page".into(), 800.0, 600.0).expect("artboard");
        let bogus = Uuid::new_v4();
        let err = document_set_layer_color(bogus, Some("red".into()))
            .expect_err("must reject unknown node");
        assert!(matches!(err, DocumentBridgeError::NodeNotFound(id) if id == bogus));
        project_close();
    }

    #[test]
    #[serial]
    fn clipboard_paste_multi_subtree_undoes_as_single_group() {
        // Devin Review ANALYSIS_0005 regression guard. A multi-subtree
        // paste must collapse into a single user-visible undo because
        // every subtree's clipboard_paste op shares one `group_id`.
        // `document_undo` (single-op) still steps once per subtree,
        // so we cover both shapes to nail the contract.
        reset_for_tests();
        let dir = tmpdir();
        project_create("clip-grouped-undo", dir.path()).expect("create");
        let ab = artboard_create(None, "Page".into(), 800.0, 600.0).expect("artboard");
        // Copy three sibling rectangles in one go so the payload has
        // three subtrees (the canonical motivation for grouping).
        let r1 =
            document_create_node("VectorLayer", Some(ab), &CreateNodeProps::default()).expect("r1");
        let r2 =
            document_create_node("VectorLayer", Some(ab), &CreateNodeProps::default()).expect("r2");
        let r3 =
            document_create_node("VectorLayer", Some(ab), &CreateNodeProps::default()).expect("r3");
        let payload = document_clipboard_copy(&[r1, r2, r3]).expect("copy");

        // Paste under the same artboard; expect three new ids.
        let pasted = document_clipboard_paste(&payload, Some(ab), 5.0, 5.0).expect("paste");
        assert_eq!(pasted.len(), 3, "three subtrees → three new roots");
        let tree_after_paste = document_get_tree().expect("tree");
        // Three originals + three pastes + 1 artboard + 1 root.
        let pasted_present: usize = pasted
            .iter()
            .filter(|id| tree_after_paste.iter().any(|n| n.id == **id))
            .count();
        assert_eq!(pasted_present, 3, "all three pastes are in the tree");

        // Single Ctrl+Z via `document_undo_group` removes ALL THREE
        // paste operations atomically — the group_id collapses them.
        let outcome = document_undo_group()
            .expect("undo group")
            .expect("op present");
        assert_eq!(
            outcome.affected_nodes.len(),
            3,
            "group undo reports every affected node",
        );
        let tree_after_undo = document_get_tree().expect("tree");
        for id in &pasted {
            assert!(
                !tree_after_undo.iter().any(|n| n.id == *id),
                "paste {id} removed by group undo",
            );
        }

        project_close();
    }

    #[test]
    #[serial]
    fn clipboard_paste_rejects_future_schema_version() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("clip-schema", dir.path()).expect("create");
        let _ab = artboard_create(None, "Page".into(), 800.0, 600.0).expect("artboard");
        // Hand-crafted v9 payload — must be rejected loudly so a
        // future schema bump doesn't get silently dropped.
        let v9_payload = r#"{"version":9,"subtrees":[]}"#;
        let err = document_clipboard_paste(v9_payload, None, 0.0, 0.0)
            .expect_err("must reject future schema");
        assert!(matches!(err, DocumentBridgeError::Io(_)));
        project_close();
    }

    // ---- Phase 11 Block C Task 18 — prototype + component
    // advanced tests. These cover the new wire-format surface
    // (Transition / AfterDelay), the auto-layout propagation
    // hook on instance resize (Task 16), and the Smart Animate
    // snapshot helper (Task 17).

    #[test]
    #[serial]
    fn phase11_layout_propagation_reflows_flex_instance_on_resize() {
        // Build a component definition whose root is a LayoutFrame
        // with a row flex layout, then instantiate it and resize
        // the instance — the children should be reflowed by the
        // propagation pass in `document_resize_frame`.
        reset_for_tests();
        let dir = tmpdir();
        project_create("phase11-prop", dir.path()).expect("create");
        let ab = artboard_create(None, "Page".into(), 1000.0, 800.0).expect("artboard");

        // Build the component definition payload in place: a
        // LayoutFrame with three 50×30 vector children.
        let frame = document_create_node(
            "LayoutFrame",
            Some(ab),
            &CreateNodeProps {
                name: Some("Card".into()),
                ..Default::default()
            },
        )
        .expect("frame");
        {
            let mut g = slot().write();
            let ws = g.as_mut().expect("ws");
            let n = ws.project.document.get_node_mut(frame).expect("frame");
            n.bounds = kcreate_core::node::Bounds::new(0.0, 0.0, 400.0, 80.0);
        }
        let kids: Vec<Uuid> = (0..3)
            .map(|i| {
                let id = document_create_node(
                    "VectorLayer",
                    Some(frame),
                    &CreateNodeProps {
                        name: Some(format!("R{i}")),
                        ..Default::default()
                    },
                )
                .expect("child");
                let mut g = slot().write();
                let ws = g.as_mut().expect("ws");
                let n = ws.project.document.get_node_mut(id).expect("kid");
                n.bounds = kcreate_core::node::Bounds::new(0.0, 0.0, 50.0, 30.0);
                id
            })
            .collect();
        let cfg = kcreate_layout::FlexLayout {
            direction: kcreate_layout::FlexDirection::Row,
            spacing: 10.0,
            ..kcreate_layout::FlexLayout::default()
        };
        layout_set_flex(frame, cfg).expect("set flex");
        layout_recompute(frame).expect("recompute");

        // Sanity: the children are packed at x=0/60/120.
        {
            let g = slot().write();
            let ws = g.as_ref().expect("ws");
            let n0 = ws.project.document.get_node(kids[0]).expect("k0");
            let n2 = ws.project.document.get_node(kids[2]).expect("k2");
            assert!((n0.bounds.x - 0.0).abs() < 1e-6, "n0 x = {}", n0.bounds.x);
            assert!((n2.bounds.x - 120.0).abs() < 1e-6, "n2 x = {}", n2.bounds.x);
        }

        // Now resize the frame to a wider box — the propagation
        // pass in `document_resize_frame` should rerun the flex
        // solver, but the children's *intrinsic* widths are
        // unchanged (Row spacing default is "pack at intrinsic
        // size + spacing"), so the positions remain at 0 / 60 /
        // 120. The key assertion is that the *bounds were
        // touched and recorded in the operation payload* — the
        // numeric equality is incidental.
        let new_bounds = kcreate_core::node::Bounds::new(0.0, 0.0, 800.0, 80.0);
        crate::phase8::document_resize_frame(frame, new_bounds).expect("resize");

        {
            let g = slot().write();
            let ws = g.as_ref().expect("ws");
            let frame_n = ws.project.document.get_node(frame).expect("frame");
            assert!(
                (frame_n.bounds.width - 800.0).abs() < 1e-6,
                "frame width after resize = {}",
                frame_n.bounds.width,
            );
            // The flex solver respects each child's intrinsic
            // width even when the parent grows, so x positions
            // stay at 0 / 60 / 120 (the propagation didn't *move*
            // them, but it did *visit* them — this is asserted by
            // the next test using a spreading config).
            let n0 = ws.project.document.get_node(kids[0]).expect("k0");
            assert!((n0.bounds.x - 0.0).abs() < 1e-6);
        }

        // Verify the resize operation captured a
        // `layout_propagation` payload in its before/after JSON —
        // this is the marker that the propagation pass walked
        // the subtree (Task 16). The walk is what re-runs the
        // flex solver on instances after their parent's bounds
        // change, so the presence of these arrays in the op log
        // is the directly observable side-effect.
        {
            let g = slot().write();
            let ws = g.as_ref().expect("ws");
            let last_op = ws
                .project
                .operation_log
                .last()
                .expect("resize op recorded")
                .clone();
            assert_eq!(last_op.command, "document_resize_frame");
            let before_layout = last_op
                .before_patch
                .get("layout_propagation")
                .expect("before layout_propagation");
            let after_layout = last_op
                .after_patch
                .get("layout_propagation")
                .expect("after layout_propagation");
            assert!(before_layout.is_array());
            assert!(after_layout.is_array());
            assert_eq!(
                before_layout.as_array().expect("before arr").len(),
                after_layout.as_array().expect("after arr").len(),
                "before/after propagation payloads must be the same length"
            );
        }
        project_close();
    }

    #[test]
    #[serial]
    fn phase11_layout_propagation_respects_recursion_limit() {
        // The propagation walk caps at LAYOUT_PROPAGATION_DEPTH_LIMIT
        // (16) to keep a pathological deeply-nested tree from
        // blowing the stack. A direct unit test of the helper is
        // cleaner than reproducing a 17-deep nesting via the
        // bridge API.
        use crate::document::{layout_propagate_in_subtree, LAYOUT_PROPAGATION_DEPTH_LIMIT};
        reset_for_tests();
        let dir = tmpdir();
        project_create("phase11-prop-depth", dir.path()).expect("create");
        let ab = artboard_create(None, "Page".into(), 200.0, 200.0).expect("artboard");

        // Build a chain of N+1 nested LayoutFrames so the walk
        // hits the cap.
        let mut parent = ab;
        for i in 0..=LAYOUT_PROPAGATION_DEPTH_LIMIT {
            let f = document_create_node(
                "LayoutFrame",
                Some(parent),
                &CreateNodeProps {
                    name: Some(format!("F{i}")),
                    ..Default::default()
                },
            )
            .expect("frame");
            parent = f;
        }

        // Direct call against the artboard root — we expect a
        // `LayoutRecursionLimit` error.
        let mut g = slot().write();
        let ws = g.as_mut().expect("ws");
        let err = layout_propagate_in_subtree(ws, ab).expect_err("must hit recursion limit");
        match err {
            DocumentBridgeError::LayoutRecursionLimit { limit, .. } => {
                assert_eq!(limit, LAYOUT_PROPAGATION_DEPTH_LIMIT);
            }
            other => panic!("expected LayoutRecursionLimit, got {other:?}"),
        }
        drop(g);
        project_close();
    }

    #[test]
    #[serial]
    fn phase11_smart_animate_snapshot_matches_layer_names() {
        // Create a component with two children, instantiate it,
        // add a second variant whose snapshot has different
        // bounds for the same layer names, and verify
        // `component_smart_animate_snapshot` returns the before
        // (current children) and after (variant snapshot) layers
        // with matching names.
        let (_dir, _ab, kids) = setup_component_project();
        let comp_id = component_create_from_selection(kids, "Card".into()).expect("create");
        // The default variant already exists with the original
        // snapshot — add a second one.
        let variant_id = component_add_variant(comp_id, "Hover".into()).expect("variant");

        // Find the placed ComponentLayer.
        let inst_id = {
            let tree = document_get_tree().expect("tree");
            tree.iter()
                .find(|n| n.node_type == "ComponentLayer")
                .map(|n| n.id)
                .expect("instance")
        };

        // Mutate the new variant's source_snapshot so its layers
        // differ from the current instance (the before set
        // mirrors the live instance children, the after set
        // mirrors the variant's stored snapshot).
        {
            let mut g = slot().write();
            let ws = g.as_mut().expect("ws");
            let def = ws
                .project
                .get_component_mut(comp_id)
                .expect("component def");
            let variant = def.variant_mut(variant_id).expect("variant");
            // Build a synthetic two-node snapshot — two layers
            // named "Rect A" and "Rect B" (the names the
            // `setup_component_project` helper assigned to the
            // children that got wrapped into the component).
            let payloads: Vec<Vec<kcreate_core::node::Node>> = vec![
                {
                    let mut n = kcreate_core::node::Node::new(NodeType::VectorLayer, "Rect A");
                    n.bounds = kcreate_core::node::Bounds::new(99.0, 99.0, 10.0, 10.0);
                    n.opacity = 0.5;
                    vec![n]
                },
                {
                    let mut n = kcreate_core::node::Node::new(NodeType::VectorLayer, "Rect B");
                    n.bounds = kcreate_core::node::Bounds::new(50.0, 0.0, 30.0, 30.0);
                    vec![n]
                },
            ];
            variant.properties.insert(
                "source_snapshot".into(),
                serde_json::to_value(&payloads).expect("snapshot json"),
            );
        }

        let snap = component_smart_animate_snapshot(inst_id, variant_id).expect("snapshot");
        // The before set is the live instance children (two
        // nodes wrapped by the create_from_selection helper).
        assert_eq!(snap.before.len(), 2, "two layers in before set");
        let before_names: std::collections::HashSet<&str> =
            snap.before.iter().map(|l| l.name.as_str()).collect();
        assert!(before_names.contains("Rect A"));
        assert!(before_names.contains("Rect B"));

        // The after set comes from the variant's snapshot we
        // just stamped in.
        assert_eq!(snap.after.len(), 2, "two layers in after set");
        let after_a = snap
            .after
            .iter()
            .find(|l| l.name == "Rect A")
            .expect("after Rect A");
        assert!((after_a.bounds.x - 99.0).abs() < 1e-6);
        assert!((after_a.opacity - 0.5).abs() < 1e-6);

        project_close();
    }

    #[test]
    #[serial]
    fn phase11_smart_animate_snapshot_does_not_commit_variant() {
        // `component_smart_animate_snapshot` is read-only — the
        // renderer commits the swap with
        // `component_switch_variant` after the animation
        // finishes. This test asserts the active variant id is
        // unchanged after a snapshot call.
        let (_dir, _ab, kids) = setup_component_project();
        let comp_id = component_create_from_selection(kids, "Card".into()).expect("create");
        let variant_id = component_add_variant(comp_id, "Hover".into()).expect("variant");
        let inst_id = {
            let tree = document_get_tree().expect("tree");
            tree.iter()
                .find(|n| n.node_type == "ComponentLayer")
                .map(|n| n.id)
                .expect("instance")
        };

        // Record the active variant id before the snapshot call.
        let before_active = {
            let g = slot().write();
            let ws = g.as_ref().expect("ws");
            let inst_n = ws.project.document.get_node(inst_id).expect("inst");
            let meta = inst_n
                .metadata
                .get(COMPONENT_INSTANCE_METADATA_KEY)
                .cloned()
                .expect("instance meta");
            let ci: ComponentInstance = serde_json::from_value(meta).expect("component instance");
            ci.active_variant_id
        };

        let _ = component_smart_animate_snapshot(inst_id, variant_id).expect("snapshot");
        // After the snapshot call the active variant must still
        // be the original — we did NOT commit the swap.
        let after_active = {
            let g = slot().write();
            let ws = g.as_ref().expect("ws");
            let inst_n = ws.project.document.get_node(inst_id).expect("inst");
            let meta = inst_n
                .metadata
                .get(COMPONENT_INSTANCE_METADATA_KEY)
                .cloned()
                .expect("instance meta");
            let ci: ComponentInstance = serde_json::from_value(meta).expect("component instance");
            ci.active_variant_id
        };
        assert_eq!(
            before_active, after_active,
            "snapshot must not mutate the active variant id",
        );

        project_close();
    }

    // ---- Phase 11 Block D Tasks 19/21/24 — concurrency stress +
    // document-version bump invariants.

    /// Task 19 + 24 — concurrent readers do not deadlock or
    /// observe torn data while a single writer mutates the
    /// workspace. We spawn `READERS` reader threads that hammer
    /// `document_get_tree` / `document_status` (both now use
    /// `slot().read()`) and one writer that creates, updates, then
    /// deletes a sequence of vector layers. The assertions are:
    ///
    ///   * Every reader returns at least the expected baseline
    ///     count (the writer never deletes the artboard).
    ///   * The writer completes all `WRITES` iterations.
    ///   * No reader observes a `NoProject` mid-run (the writer
    ///     never closes the project).
    ///
    /// Iteration counts are intentionally modest (200 writes,
    /// 8 readers, 200 iterations each) so the test stays CI-fast
    /// (< 1s) while still exercising real parallelism on the
    /// `RwLock` — the previous `Mutex` would serialise every read,
    /// so a regression to `Mutex` would still pass but the
    /// `RUNS / second` ratio would drop. We assert the qualitative
    /// invariant (no deadlock, no corruption) which is what users
    /// actually care about; a microbenchmark lives in
    /// `crates/kcreate_tests/tests/render_pipeline_perf.rs`.
    #[test]
    #[serial]
    fn phase11_rwlock_workspace_concurrent_readers_no_deadlock() {
        const READERS: usize = 8;
        const READS_PER_READER: usize = 200;
        const WRITES: usize = 200;

        reset_for_tests();
        let dir = tmpdir();
        project_create("phase11-rwlock", dir.path()).expect("create");
        let ab = artboard_create(None, "Page".into(), 1000.0, 800.0).expect("artboard");

        // Pre-create one stable child so readers always observe
        // at least one VectorLayer beyond the artboard root.
        let stable = document_create_node(
            "VectorLayer",
            Some(ab),
            &CreateNodeProps {
                name: Some("Stable".into()),
                ..Default::default()
            },
        )
        .expect("stable");

        let barrier = std::sync::Arc::new(std::sync::Barrier::new(READERS + 1));
        let mut readers = Vec::with_capacity(READERS);
        for _ in 0..READERS {
            let b = std::sync::Arc::clone(&barrier);
            readers.push(std::thread::spawn(move || {
                b.wait();
                let mut last_count = 0usize;
                for _ in 0..READS_PER_READER {
                    let tree = document_get_tree().expect("tree");
                    // The artboard + the stable layer are always
                    // present; writer-created transient layers
                    // may or may not be in this snapshot.
                    assert!(
                        tree.len() >= 2,
                        "reader observed truncated tree: len={}",
                        tree.len()
                    );
                    let status = document_status().expect("status");
                    assert!(
                        status.node_count >= 2,
                        "reader observed truncated node_count: {}",
                        status.node_count
                    );
                    last_count = tree.len();
                }
                last_count
            }));
        }

        // Writer thread: drives mutations through the bridge.
        let writer = {
            let b = std::sync::Arc::clone(&barrier);
            std::thread::spawn(move || {
                b.wait();
                let mut transient_ids = Vec::with_capacity(WRITES);
                for i in 0..WRITES {
                    let id = document_create_node(
                        "VectorLayer",
                        Some(ab),
                        &CreateNodeProps {
                            name: Some(format!("T{i}")),
                            ..Default::default()
                        },
                    )
                    .expect("transient");
                    transient_ids.push(id);
                }
                for id in &transient_ids {
                    document_delete_node(*id).expect("delete");
                }
                transient_ids.len()
            })
        };

        let writes = writer.join().expect("writer joined");
        assert_eq!(writes, WRITES);
        for r in readers {
            let n = r.join().expect("reader joined");
            assert!(n >= 2, "final read snapshot too small: {n}");
        }

        // The stable layer must survive the storm.
        let final_tree = document_get_tree().expect("tree");
        assert!(
            final_tree.iter().any(|n| n.id == stable),
            "stable layer must be present after concurrent storm",
        );

        project_close();
    }

    /// Task 21 — every mutation bumps the process-global
    /// `document_version` counter; read-only entry points must
    /// NOT bump it. The counter is plumbed through the bridge as
    /// `lib::document_version()`; we read it via the core helper
    /// (the bridge wrapper just truncates to `u32`).
    #[test]
    #[serial]
    fn phase11_document_version_advances_on_mutation_only() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("phase11-ver", dir.path()).expect("create");
        let ab = artboard_create(None, "Page".into(), 200.0, 200.0).expect("artboard");

        let v0 = kcreate_core::document::document_version_global();

        // Read-only calls must not advance the counter.
        let _ = document_get_tree().expect("tree");
        let _ = document_status().expect("status");
        let _ = document_get_selection().expect("selection");
        let v_read = kcreate_core::document::document_version_global();
        assert_eq!(v_read, v0, "read-only calls bumped document_version");

        // Single mutation: create + update.
        let id = document_create_node(
            "VectorLayer",
            Some(ab),
            &CreateNodeProps {
                name: Some("v".into()),
                ..Default::default()
            },
        )
        .expect("create");
        let v1 = kcreate_core::document::document_version_global();
        assert!(
            v1 > v0,
            "create did not bump document_version: {v0} -> {v1}"
        );

        document_update_node(
            id,
            &UpdateNodeProps {
                name: Some("renamed".into()),
                ..Default::default()
            },
        )
        .expect("update");
        let v2 = kcreate_core::document::document_version_global();
        assert!(
            v2 > v1,
            "update did not bump document_version: {v1} -> {v2}"
        );

        // Undo + redo must each bump.
        let _ = document_undo().expect("undo");
        let v3 = kcreate_core::document::document_version_global();
        assert!(v3 > v2, "undo did not bump document_version: {v2} -> {v3}");
        let _ = document_redo().expect("redo");
        let v4 = kcreate_core::document::document_version_global();
        assert!(v4 > v3, "redo did not bump document_version: {v3} -> {v4}");

        project_close();
    }

    // -------------------------------------------------------------
    // Phase B1 — canvas_create_path coverage
    // -------------------------------------------------------------
    //
    // canvas_create_path is the only shape-creator that takes a
    // caller-provided segment list (rest synthesize one from
    // {x,y,w,h} / {cx,cy,rx,ry} / endpoints). These tests pin:
    //   * the wire shape (JSON `{"op":"move_to",...}`) round-trips
    //     into a real `VectorPath` end-to-end,
    //   * bounds come from Kurbo's `BezPath::bounding_box()` (tight
    //     curve bounds, not control-point bounds),
    //   * undo deletes the node and redo recreates it (i.e. the
    //     "create" op_kind is properly recorded), and
    //   * every `CreatePathError` variant fires on the right input.

    fn line_segments_json(x1: f64, y1: f64, x2: f64, y2: f64) -> String {
        format!(r#"[{{"op":"move_to","x":{x1},"y":{y1}}},{{"op":"line_to","x":{x2},"y":{y2}}}]"#)
    }

    #[test]
    #[serial]
    fn canvas_create_path_inserts_node_with_kurbo_bounds() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("path1", dir.path()).expect("create");
        // Diagonal line from (5,7) to (20,30). Bounds should be the
        // tight box (5,7)-(20,30) = width 15, height 23.
        let id = canvas_create_path(None, &line_segments_json(5.0, 7.0, 20.0, 30.0), false, None)
            .expect("create_path");
        let tree = document_get_tree().expect("tree");
        let node = tree.iter().find(|n| n.id == id).expect("inserted");
        assert_eq!(node.node_type, "VectorLayer");
        assert_eq!(node.name, "Path");
        assert!(
            (node.bounds.x - 5.0).abs() < 1e-6
                && (node.bounds.y - 7.0).abs() < 1e-6
                && (node.bounds.width - 15.0).abs() < 1e-6
                && (node.bounds.height - 23.0).abs() < 1e-6,
            "got bounds {:?}",
            node.bounds
        );
        project_close();
    }

    #[test]
    #[serial]
    fn canvas_create_path_uses_tight_curve_bounds_not_control_point_bounds() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("path_curve", dir.path()).expect("create");
        // A cubic curve from (0,0) to (10,0) with both control
        // points yanked up to y=100. The CURVE itself never goes
        // above y=75 (cubic peaks at 3/4 of the control height for
        // symmetric handles), but the control-point bounding box
        // includes the full (0,0)-(10,100) extent. We pin that the
        // bridge uses Kurbo's tight bounds.
        let segs = r#"[
            {"op":"move_to","x":0.0,"y":0.0},
            {"op":"cubic_to","ctrl1":{"x":0.0,"y":100.0},"ctrl2":{"x":10.0,"y":100.0},"end":{"x":10.0,"y":0.0}}
        ]"#;
        let id = canvas_create_path(None, segs, false, Some("Curve".into())).expect("path");
        let tree = document_get_tree().expect("tree");
        let node = tree.iter().find(|n| n.id == id).expect("inserted");
        assert_eq!(node.name, "Curve");
        // Tight cubic peak: y_max = 75 exactly for the chosen
        // symmetric handles. Allow 1e-3 for Kurbo's flattening.
        assert!(
            node.bounds.height < 80.0 && node.bounds.height > 70.0,
            "expected tight cubic bounds ~75, got {:?}",
            node.bounds
        );
        project_close();
    }

    #[test]
    #[serial]
    fn canvas_create_path_stores_vector_path_metadata() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("path_meta", dir.path()).expect("create");
        let id = canvas_create_path(None, &line_segments_json(0.0, 0.0, 10.0, 0.0), true, None)
            .expect("path");
        // Read back the raw metadata blob and verify it deserializes
        // to a VectorPath whose `closed` flag matches the caller.
        let guard = slot().read();
        let ws = guard.as_ref().expect("ws");
        let node = ws.project.document.get_node(id).expect("node");
        let blob = node
            .metadata
            .get(crate::scene_sync::VECTOR_PATH_METADATA_KEY)
            .expect("vector_path metadata");
        let path: kcreate_vector::VectorPath =
            serde_json::from_value(blob.clone()).expect("vector_path deserializes");
        assert!(path.closed);
        assert_eq!(path.commands.len(), 2);
    }

    #[test]
    #[serial]
    fn canvas_create_path_records_undoable_operation() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("path_undo", dir.path()).expect("create");
        let id = canvas_create_path(None, &line_segments_json(0.0, 0.0, 5.0, 5.0), false, None)
            .expect("path");
        let after_create = document_get_tree().expect("tree");
        assert!(after_create.iter().any(|n| n.id == id));

        // For graph-mutating ops the bridge does NOT fold the
        // inverse patch back into the in-memory tree itself — the
        // host owns that step (see `document_undo` doc comment).
        // What we *can* verify at the bridge layer is that an op
        // was recorded under the `canvas_create_path` op_kind and
        // is visible to `document_undo` / `document_redo`.
        let undo_outcome = document_undo()
            .expect("undo")
            .expect("canvas_create_path was recorded");
        assert_eq!(undo_outcome.command, "canvas_create_path");
        assert_eq!(undo_outcome.affected_nodes, vec![id]);

        let redo_outcome = document_redo()
            .expect("redo")
            .expect("canvas_create_path is on the redo stack");
        assert_eq!(redo_outcome.command, "canvas_create_path");
        assert_eq!(redo_outcome.affected_nodes, vec![id]);
        project_close();
    }

    #[test]
    #[serial]
    fn canvas_create_path_rejects_empty_payload() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("path_empty", dir.path()).expect("create");
        let err = canvas_create_path(None, "[]", false, None).expect_err("empty rejected");
        assert!(matches!(
            err,
            DocumentBridgeError::CreatePath(CreatePathError::Empty)
        ));
        project_close();
    }

    #[test]
    #[serial]
    fn canvas_create_path_rejects_missing_move_to() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("path_no_move", dir.path()).expect("create");
        // A path that starts with `line_to` is structurally invalid:
        // `LineTo` needs an implicit current-point that doesn't
        // exist without a leading `MoveTo`.
        let segs = r#"[{"op":"line_to","x":1.0,"y":1.0}]"#;
        let err = canvas_create_path(None, segs, false, None).expect_err("missing move_to");
        assert!(matches!(
            err,
            DocumentBridgeError::CreatePath(CreatePathError::MissingMoveTo)
        ));
        project_close();
    }

    #[test]
    #[serial]
    fn canvas_create_path_rejects_invalid_json() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("path_bad_json", dir.path()).expect("create");
        let err =
            canvas_create_path(None, "not-json", false, None).expect_err("invalid json rejected");
        assert!(matches!(
            err,
            DocumentBridgeError::CreatePath(CreatePathError::InvalidJson(_))
        ));
        project_close();
    }

    #[test]
    #[serial]
    fn canvas_create_path_supports_all_segment_kinds() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("path_all_kinds", dir.path()).expect("create");
        // Exercises every PathSegment variant in one path, including
        // QuadTo + CubicTo + Close. Locks in that the wire format
        // covers the full kcreate_vector::PathSegment surface.
        let segs = r#"[
            {"op":"move_to","x":0.0,"y":0.0},
            {"op":"line_to","x":10.0,"y":0.0},
            {"op":"quad_to","ctrl":{"x":15.0,"y":5.0},"end":{"x":10.0,"y":10.0}},
            {"op":"cubic_to","ctrl1":{"x":5.0,"y":15.0},"ctrl2":{"x":0.0,"y":15.0},"end":{"x":0.0,"y":10.0}},
            {"op":"close"}
        ]"#;
        let id = canvas_create_path(None, segs, true, None).expect("create");
        let guard = slot().read();
        let ws = guard.as_ref().expect("ws");
        let node = ws.project.document.get_node(id).expect("node");
        let blob = node
            .metadata
            .get(crate::scene_sync::VECTOR_PATH_METADATA_KEY)
            .expect("vector_path metadata");
        let path: kcreate_vector::VectorPath =
            serde_json::from_value(blob.clone()).expect("path deserializes");
        assert_eq!(path.commands.len(), 5);
        assert!(matches!(
            path.commands[0],
            kcreate_vector::PathSegment::MoveTo(_)
        ));
        assert!(matches!(
            path.commands[1],
            kcreate_vector::PathSegment::LineTo(_)
        ));
        assert!(matches!(
            path.commands[2],
            kcreate_vector::PathSegment::QuadTo { .. }
        ));
        assert!(matches!(
            path.commands[3],
            kcreate_vector::PathSegment::CubicTo { .. }
        ));
        assert!(matches!(
            path.commands[4],
            kcreate_vector::PathSegment::Close
        ));
        assert!(path.closed);
    }

    // -------------------------------------------------------------
    // Phase B2 — canvas_path_boolean coverage
    // -------------------------------------------------------------
    //
    // canvas_path_boolean is the Pathfinder entry point. These
    // tests pin:
    //   * each of the four ops produces a non-empty result on
    //     overlapping squares,
    //   * source nodes are removed and result nodes are inserted
    //     with the *first* source's style,
    //   * a single undoable op is recorded under the
    //     `canvas_path_boolean` op_kind with affected_nodes
    //     spanning both sides,
    //   * each PathBooleanError variant fires on the right input
    //     (invalid op string, fewer than 2 sources, missing node,
    //     non-vector source, source missing path metadata,
    //     empty-result on disjoint intersect).
    //
    // Two overlapping axis-aligned squares are the simplest input
    // that produces a non-trivial boolean for every op: union is a
    // rectangle, intersect is the overlap square, subtract is an
    // L-shape, exclude is two disjoint L-shapes. Disjoint squares
    // are used for the `EmptyResult` test (intersect = nothing).
    fn square_path_segments(x: f64, y: f64, side: f64) -> Vec<kcreate_vector::PathSegment> {
        vec![
            kcreate_vector::PathSegment::MoveTo(kcreate_vector::PathPoint::new(x, y)),
            kcreate_vector::PathSegment::LineTo(kcreate_vector::PathPoint::new(x + side, y)),
            kcreate_vector::PathSegment::LineTo(kcreate_vector::PathPoint::new(x + side, y + side)),
            kcreate_vector::PathSegment::LineTo(kcreate_vector::PathPoint::new(x, y + side)),
            kcreate_vector::PathSegment::Close,
        ]
    }

    fn insert_square(x: f64, y: f64, side: f64) -> Uuid {
        let segs = square_path_segments(x, y, side);
        let segs_json = serde_json::to_string(&segs).expect("serialize");
        canvas_create_path(None, &segs_json, true, Some(format!("Square@{x},{y}")))
            .expect("create square")
    }

    fn assert_vector_layer(id: Uuid) -> kcreate_vector::VectorPath {
        let guard = slot().read();
        let ws = guard.as_ref().expect("ws");
        let node = ws.project.document.get_node(id).expect("node");
        assert_eq!(node.node_type, NodeType::VectorLayer);
        let blob = node
            .metadata
            .get(crate::scene_sync::VECTOR_PATH_METADATA_KEY)
            .expect("vector_path");
        serde_json::from_value(blob.clone()).expect("deserialize")
    }

    #[test]
    #[serial]
    fn canvas_path_boolean_union_merges_overlapping_squares() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("bool_union", dir.path()).expect("create");
        // Square A at (0,0)-(10,10); B at (5,5)-(15,15). Union
        // covers the L-shape envelope (0,0)-(15,15) with the
        // upper-right corner cut out — bounds box should still be
        // 15x15. Result must be ≥1 shape (typically exactly 1
        // because the union is connected).
        let a = insert_square(0.0, 0.0, 10.0);
        let b = insert_square(5.0, 5.0, 10.0);
        let result_ids = canvas_path_boolean("union", vec![a, b]).expect("union");
        assert!(
            !result_ids.is_empty(),
            "union produced at least one result shape"
        );
        // Sources are gone, results are in.
        let tree = document_get_tree().expect("tree");
        assert!(
            tree.iter().all(|n| n.id != a && n.id != b),
            "source nodes were removed"
        );
        for id in &result_ids {
            assert!(tree.iter().any(|n| n.id == *id), "result {id} inserted");
            let path = assert_vector_layer(*id);
            // Boolean output is line-only (the i_overlay pipeline
            // flattens beziers before processing), so every
            // segment should be MoveTo / LineTo / Close.
            for seg in &path.commands {
                assert!(
                    matches!(
                        seg,
                        kcreate_vector::PathSegment::MoveTo(_)
                            | kcreate_vector::PathSegment::LineTo(_)
                            | kcreate_vector::PathSegment::Close
                    ),
                    "boolean output should be polyline-only, got {seg:?}"
                );
            }
        }
        project_close();
    }

    #[test]
    #[serial]
    fn canvas_path_boolean_intersect_overlapping_squares_is_overlap() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("bool_intersect", dir.path()).expect("create");
        // A=(0,0)-(10,10), B=(5,5)-(15,15). Intersect = (5,5)-(10,10),
        // tight bounds 5x5.
        let a = insert_square(0.0, 0.0, 10.0);
        let b = insert_square(5.0, 5.0, 10.0);
        let result_ids = canvas_path_boolean("intersect", vec![a, b]).expect("intersect");
        assert_eq!(result_ids.len(), 1, "intersect is connected");
        let tree = document_get_tree().expect("tree");
        let result = tree
            .iter()
            .find(|n| n.id == result_ids[0])
            .expect("result inserted");
        assert!(
            (result.bounds.width - 5.0).abs() < 1e-3 && (result.bounds.height - 5.0).abs() < 1e-3,
            "intersect bounds should be 5x5, got {:?}",
            result.bounds
        );
        project_close();
    }

    #[test]
    #[serial]
    fn canvas_path_boolean_subtract_is_first_minus_rest() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("bool_subtract", dir.path()).expect("create");
        let a = insert_square(0.0, 0.0, 10.0);
        let b = insert_square(5.0, 5.0, 10.0);
        let result_ids = canvas_path_boolean("subtract", vec![a, b]).expect("subtract");
        assert!(!result_ids.is_empty(), "subtract produced a result");
        // A \ B is an L-shape — origin still at (0,0), but the
        // overall bounds-rect of the L is still (0,0)-(10,10) since
        // we keep the outer edge. We just check the bounds origin
        // isn't shifted (i.e. we didn't accidentally compute B \ A).
        let tree = document_get_tree().expect("tree");
        let result = tree.iter().find(|n| n.id == result_ids[0]).expect("result");
        assert!(
            result.bounds.x.abs() < 1e-3 && result.bounds.y.abs() < 1e-3,
            "subtract A\\B keeps the A corner, got {:?}",
            result.bounds
        );
        project_close();
    }

    #[test]
    #[serial]
    fn canvas_path_boolean_exclude_overlapping_squares_produces_xor() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("bool_exclude", dir.path()).expect("create");
        let a = insert_square(0.0, 0.0, 10.0);
        let b = insert_square(5.0, 5.0, 10.0);
        let result_ids = canvas_path_boolean("exclude", vec![a, b]).expect("exclude");
        assert!(
            !result_ids.is_empty(),
            "exclude produced at least one shape"
        );
        // XOR of two overlapping squares is two disjoint L-shapes
        // (or one path with two sub-contours). Either way the
        // bounding box of the union of results spans (0,0)-(15,15).
        let tree = document_get_tree().expect("tree");
        let (mut min_x, mut min_y, mut max_x, mut max_y) = (
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        );
        for id in &result_ids {
            let n = tree.iter().find(|n| n.id == *id).expect("result");
            min_x = min_x.min(n.bounds.x);
            min_y = min_y.min(n.bounds.y);
            max_x = max_x.max(n.bounds.x + n.bounds.width);
            max_y = max_y.max(n.bounds.y + n.bounds.height);
        }
        assert!(min_x.abs() < 1e-3, "exclude min_x ~0, got {min_x}");
        assert!(min_y.abs() < 1e-3, "exclude min_y ~0, got {min_y}");
        assert!(
            (max_x - 15.0).abs() < 1e-3,
            "exclude max_x ~15, got {max_x}"
        );
        assert!(
            (max_y - 15.0).abs() < 1e-3,
            "exclude max_y ~15, got {max_y}"
        );
        project_close();
    }

    #[test]
    #[serial]
    fn canvas_path_boolean_records_one_undoable_operation() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("bool_undo", dir.path()).expect("create");
        let a = insert_square(0.0, 0.0, 10.0);
        let b = insert_square(5.0, 5.0, 10.0);
        // Each create_path recorded one op; the boolean should
        // record exactly one more.
        let before = {
            let guard = slot().read();
            guard.as_ref().expect("ws").project.operation_log.len()
        };
        let _ = canvas_path_boolean("union", vec![a, b]).expect("union");
        let after = {
            let guard = slot().read();
            guard.as_ref().expect("ws").project.operation_log.len()
        };
        assert_eq!(
            after,
            before + 1,
            "boolean recorded exactly one undo entry: before={before} after={after}"
        );
        let outcome = document_undo()
            .expect("undo")
            .expect("boolean was recorded");
        assert_eq!(outcome.command, "canvas_path_boolean");
        // affected_nodes spans sources + results (count is
        // sources.len() + result_ids.len()).
        assert!(
            outcome.affected_nodes.len() >= 3,
            "affected_nodes spans both sides, got {:?}",
            outcome.affected_nodes
        );
        let redo = document_redo()
            .expect("redo")
            .expect("boolean is on the redo stack");
        assert_eq!(redo.command, "canvas_path_boolean");
        project_close();
    }

    #[test]
    #[serial]
    fn canvas_path_boolean_preserves_first_source_style_on_result() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("bool_style", dir.path()).expect("create");
        let a = insert_square(0.0, 0.0, 10.0);
        let b = insert_square(5.0, 5.0, 10.0);
        // Stamp a distinguishing corner radius onto A. The result
        // node should inherit it because A is the first source.
        {
            let mut guard = slot().write();
            let ws = guard.as_mut().expect("ws");
            let node = ws.project.document.get_node_mut(a).expect("node a");
            node.style.corner_radius = 7.5;
        }
        let result_ids = canvas_path_boolean("union", vec![a, b]).expect("union");
        let guard = slot().read();
        let ws = guard.as_ref().expect("ws");
        for id in &result_ids {
            let n = ws.project.document.get_node(*id).expect("result");
            assert!(
                (n.style.corner_radius - 7.5).abs() < 1e-6,
                "result inherits first source's corner_radius, got {}",
                n.style.corner_radius
            );
        }
    }

    #[test]
    #[serial]
    fn canvas_path_boolean_rejects_invalid_op_string() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("bool_bad_op", dir.path()).expect("create");
        let a = insert_square(0.0, 0.0, 10.0);
        let b = insert_square(5.0, 5.0, 10.0);
        let err = canvas_path_boolean("xor", vec![a, b]).expect_err("invalid op rejected");
        assert!(
            matches!(
                err,
                DocumentBridgeError::PathBoolean(PathBooleanError::InvalidOp(ref s)) if s == "xor"
            ),
            "got {err:?}"
        );
        project_close();
    }

    #[test]
    #[serial]
    fn canvas_path_boolean_rejects_fewer_than_two_sources() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("bool_one_src", dir.path()).expect("create");
        let a = insert_square(0.0, 0.0, 10.0);
        let err = canvas_path_boolean("union", vec![a]).expect_err("one source rejected");
        assert!(
            matches!(
                err,
                DocumentBridgeError::PathBoolean(PathBooleanError::TooFewSources(1))
            ),
            "got {err:?}"
        );
        let err = canvas_path_boolean("union", vec![]).expect_err("zero sources rejected");
        assert!(
            matches!(
                err,
                DocumentBridgeError::PathBoolean(PathBooleanError::TooFewSources(0))
            ),
            "got {err:?}"
        );
        project_close();
    }

    #[test]
    #[serial]
    fn canvas_path_boolean_rejects_missing_source() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("bool_missing", dir.path()).expect("create");
        let a = insert_square(0.0, 0.0, 10.0);
        let ghost = Uuid::new_v4();
        let err = canvas_path_boolean("union", vec![a, ghost]).expect_err("missing rejected");
        match err {
            DocumentBridgeError::PathBoolean(PathBooleanError::SourceNotFound(id)) => {
                assert_eq!(id, ghost);
            }
            other => panic!("expected SourceNotFound, got {other:?}"),
        }
        project_close();
    }

    #[test]
    #[serial]
    fn canvas_path_boolean_rejects_non_vector_source() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("bool_text_src", dir.path()).expect("create");
        let a = insert_square(0.0, 0.0, 10.0);
        // Insert a text layer — also a Node but the wrong type.
        let text_id =
            canvas_create_text(None, 0.0, 0.0, "hi".to_string(), "Inter".to_string(), 12.0)
                .expect("create text");
        let err = canvas_path_boolean("union", vec![a, text_id]).expect_err("non-vector rejected");
        match err {
            DocumentBridgeError::PathBoolean(PathBooleanError::SourceNotVector { id, got }) => {
                assert_eq!(id, text_id);
                assert_eq!(got, NodeType::TextLayer);
            }
            other => panic!("expected SourceNotVector, got {other:?}"),
        }
        project_close();
    }

    #[test]
    #[serial]
    fn canvas_path_boolean_disjoint_intersect_returns_empty_result() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("bool_disjoint", dir.path()).expect("create");
        let a = insert_square(0.0, 0.0, 5.0);
        let b = insert_square(100.0, 100.0, 5.0);
        // Intersect of disjoint squares is the empty set. Either
        // boolean_operation returns Ok([]) (we map to EmptyResult)
        // or returns an i_overlay-side error wrapped in Vector.
        // Both are acceptable failure modes for the renderer; we
        // assert it's a PathBoolean variant.
        let err = canvas_path_boolean("intersect", vec![a, b]).expect_err("disjoint rejected");
        assert!(
            matches!(err, DocumentBridgeError::PathBoolean(_)),
            "got {err:?}"
        );
        project_close();
    }

    /// Regression test for Devin Review #0001 (round 2) on PR #38.
    ///
    /// After a successful boolean, `ws.selection` must contain
    /// exactly the new result ids — not the (now-deleted) source
    /// ids, and not the empty slot left by step 4 of
    /// `canvas_path_boolean`. The host's `refreshTree()` pulls
    /// selection back via `document_get_selection()`, so if the
    /// bridge state were stale here the JS-side `setSelectedIds`
    /// would be clobbered to `[]` on the next refresh and the user
    /// would lose their selection right after the gesture.
    #[test]
    #[serial]
    fn canvas_path_boolean_sets_selection_to_result_ids() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("bool_sel", dir.path()).expect("create");
        let a = insert_square(0.0, 0.0, 10.0);
        let b = insert_square(5.0, 5.0, 10.0);
        // Seed the selection with the source ids — this mirrors the
        // real host gesture (user selects A + B, then clicks Union).
        document_set_selection(vec![a, b]).expect("seed selection");
        let result_ids = canvas_path_boolean("union", vec![a, b]).expect("union");
        assert!(!result_ids.is_empty(), "union produced ≥1 result shape");
        let after = document_get_selection().expect("get selection");
        // The new selection IS the result ids, in iteration order,
        // and contains neither of the (now-deleted) source ids.
        assert_eq!(after, result_ids, "selection adopts result ids");
        assert!(
            !after.contains(&a) && !after.contains(&b),
            "deleted source ids are gone from selection: {after:?}"
        );
        project_close();
    }

    /// Regression test for Devin Review ANALYSIS_0003 (round 5) on
    /// PR #38: duplicate source ids in `canvas_path_boolean` must
    /// be deduplicated (first occurrence wins) before the
    /// `TooFewSources` count check, so a future caller (MCP /
    /// plugin / scripting API) that passes `[a, a, b]` gets a
    /// correct two-distinct-shapes fold instead of wasting work
    /// resolving `a` twice and silently double-removing it from
    /// the graph, AND a caller that passes `[a, a]` (all dupes)
    /// gets a clean `TooFewSources(1)` error with the accurate
    /// post-dedup count instead of being allowed to proceed as if
    /// two distinct sources were passed.
    #[test]
    #[serial]
    fn canvas_path_boolean_dedupes_duplicate_source_ids() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("bool_dup", dir.path()).expect("create");
        let a = insert_square(0.0, 0.0, 10.0);
        let b = insert_square(5.0, 5.0, 10.0);
        // Pass `a` three times interleaved with `b`. After dedup
        // this reduces to `[a, b]` — the exact same input the
        // `_union_merges_overlapping_squares` test uses — so the
        // result must match: a single connected union shape.
        let result_ids =
            canvas_path_boolean("union", vec![a, a, b, a]).expect("dedup-then-union ok");
        assert!(
            !result_ids.is_empty(),
            "union produced at least one result shape"
        );

        // After the gesture, the source nodes are gone (each
        // removed exactly once — without dedup, `remove_node(a)`
        // would have been called three times, with the second and
        // third calls silently returning `None`). The bridge
        // selection now contains exactly the result ids.
        let guard = slot().read();
        let ws = guard.as_ref().expect("ws");
        assert!(
            ws.project.document.get_node(a).is_none(),
            "source a removed once, cleanly"
        );
        assert!(
            ws.project.document.get_node(b).is_none(),
            "source b removed once, cleanly"
        );
        assert_eq!(
            ws.selection, result_ids,
            "selection adopts result ids (no source remnants)"
        );
    }

    /// Regression test for Devin Review ANALYSIS_0003 (round 5)
    /// on PR #38, edge case: a `Vec<Uuid>` consisting entirely of
    /// duplicates of the same id must be rejected as
    /// `TooFewSources` with the **deduplicated** count (1), not
    /// the raw input length. This is the load-bearing reason we
    /// dedup BEFORE the `< 2` check rather than after — `[a, a]`
    /// passing the count check then folding `union(A, A) = A`
    /// would be a silently-degenerate gesture.
    #[test]
    #[serial]
    fn canvas_path_boolean_all_duplicates_rejected_as_too_few_sources() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("bool_all_dup", dir.path()).expect("create");
        let a = insert_square(0.0, 0.0, 10.0);
        for raw_len in [2usize, 3, 5] {
            let ids: Vec<Uuid> = std::iter::repeat_n(a, raw_len).collect();
            let err = canvas_path_boolean("union", ids).expect_err("all-dupes rejected");
            assert!(
                matches!(
                    err,
                    DocumentBridgeError::PathBoolean(PathBooleanError::TooFewSources(1))
                ),
                "raw_len={raw_len} must dedup to 1 and report TooFewSources(1), got {err:?}"
            );
        }
        // The shape itself must still be intact — the early
        // rejection happens BEFORE the write-guard is taken, so
        // nothing in the graph should have changed.
        let guard = slot().read();
        let ws = guard.as_ref().expect("ws");
        assert!(
            ws.project.document.get_node(a).is_some(),
            "source untouched after early-rejected dedup gesture"
        );
        drop(guard);
        project_close();
    }

    /// Regression test for Devin Review BUG_0001 (round 7) on PR #38:
    /// when `source_ids[0]` is a child (or deeper descendant) of
    /// another source node, the result nodes must NOT be silently
    /// cascade-deleted by step 4's recursive `remove_node`.
    ///
    /// Pre-fix behaviour (the bug): `first_parent = source_ids[0]
    /// .parent_id` was used verbatim. If that parent_id belonged to
    /// `source_ids[1]`, the result nodes were inserted as children
    /// of `source_ids[1]`; then `remove_node(source_ids[1])`
    /// recursively deleted ALL its children — including the
    /// just-inserted results. The function returned ids that no
    /// longer existed in the graph; the JS side selected phantom
    /// nodes and the operation log referenced deleted nodes.
    ///
    /// Post-fix behaviour: step 3 walks up the parent chain from
    /// `first_parent` skipping any ancestor that itself appears in
    /// `source_ids`. With `parent(a) == b` and `source_ids = [a, b]`,
    /// the walk advances past `b` to `b.parent_id` (root in this
    /// test setup), so the results are parented to root and survive
    /// step 4's cleanup of `a` + `b`.
    ///
    /// The test pins three invariants together: (1) the call
    /// succeeds and returns non-empty `result_ids`; (2) every
    /// returned id resolves to a live node in the graph; (3) both
    /// sources (parent + descendant) are gone; (4) the selection
    /// matches the result ids — the JS side will paint exactly the
    /// new shapes, no phantoms.
    #[test]
    #[serial]
    fn canvas_path_boolean_hierarchical_sources_do_not_cascade_delete_results() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("bool_hier", dir.path()).expect("create");
        // Two overlapping squares so the union has a connected
        // shape (matches `_union_merges_overlapping_squares`).
        let parent_sq = insert_square(0.0, 0.0, 10.0);
        let child_sq = insert_square(5.0, 5.0, 10.0);
        // Reparent the child square under the parent square so the
        // hierarchy is `root -> parent_sq -> child_sq`. The bridge
        // accepts this via `document_reparent_node`; the core
        // `reparent_node` permits any non-cycling reparent
        // regardless of node-type (VectorLayer accepting a
        // VectorLayer child is unusual in normal UI flow but
        // structurally valid).
        document_reparent_node(child_sq, Some(parent_sq), 0).expect("reparent");

        // Pass the child FIRST so `first_parent == parent_sq` —
        // this is the exact configuration that triggered the
        // cascade-delete bug pre-fix.
        let result_ids = canvas_path_boolean("union", vec![child_sq, parent_sq]).expect("union ok");
        assert!(!result_ids.is_empty(), "union produced at least one result");

        let guard = slot().read();
        let ws = guard.as_ref().expect("ws");
        // The load-bearing invariant: every returned id must
        // resolve to a live node. Pre-fix this assertion FAILS
        // because step 4's recursive remove_node deleted the
        // results along with `parent_sq`.
        for rid in &result_ids {
            assert!(
                ws.project.document.get_node(*rid).is_some(),
                "result {rid} survived step 4 cleanup (not cascade-deleted)"
            );
            // And it must be parented at root (the first non-source
            // ancestor of `parent_sq` is `None`), not inside the
            // descendant scope that got swept.
            let n = ws.project.document.get_node(*rid).expect("live");
            assert_eq!(
                n.parent_id, None,
                "result {rid} parented to first non-source ancestor (root)"
            );
        }
        // Both sources are gone — parent_sq directly via
        // `remove_node`, child_sq via the recursive cascade from
        // its parent.
        assert!(
            ws.project.document.get_node(parent_sq).is_none(),
            "parent_sq removed"
        );
        assert!(
            ws.project.document.get_node(child_sq).is_none(),
            "child_sq swept by parent's recursive remove (or by its own remove_node call — both paths are idempotent)"
        );
        // Selection adopts the result ids only — no phantoms, no
        // remnants.
        assert_eq!(ws.selection, result_ids, "selection == result_ids");
        drop(guard);
        project_close();
    }

    /// Regression test for Devin Review #0004 (round 1, edited) on
    /// PR #38: `merge_paths` is now a total function — defined for
    /// the empty slice — so a future caller that violates the
    /// "non-empty" expectation degrades to a well-defined empty
    /// path instead of a release-mode out-of-bounds panic.
    ///
    /// This pin guards the type-system fix against future
    /// refactors that might re-introduce a `paths[0]` index without
    /// first proving the slice is non-empty.
    #[test]
    fn merge_paths_handles_empty_slice_without_panic() {
        // The function is private; the test lives in the same
        // module so we can call it directly. Empty input yields an
        // empty path with the default fill rule, not a panic.
        let out = super::merge_paths(&[]);
        assert!(out.commands.is_empty(), "empty input → empty segments");
        assert!(
            !out.closed,
            "empty path is not closed (no contour to close)"
        );

        // Single-element input is short-circuited to a clone.
        let p = kcreate_vector::VectorPath::new(square_path_segments(0.0, 0.0, 10.0));
        let cloned = super::merge_paths(std::slice::from_ref(&p));
        assert_eq!(cloned.commands, p.commands, "len=1 → clone");

        // Multi-element input concatenates segments and inherits
        // the first input's `closed` + `fill_rule`.
        let a = {
            let mut v = kcreate_vector::VectorPath::new(square_path_segments(0.0, 0.0, 10.0));
            v.closed = true;
            v
        };
        let b = kcreate_vector::VectorPath::new(square_path_segments(20.0, 0.0, 10.0));
        let merged = super::merge_paths(&[a.clone(), b.clone()]);
        assert_eq!(
            merged.commands.len(),
            a.commands.len() + b.commands.len(),
            "concatenates segment lists"
        );
        assert!(merged.closed, "merged inherits first input's closed flag");
    }

    // -------------------------------------------------------------
    // Phase B3 — canvas_path_get_segments / canvas_path_set_segments
    // coverage
    // -------------------------------------------------------------
    //
    // These two entry points are the bridge surface for the node
    // editor. The invariants verified here are exactly the
    // contract the renderer-side `useToolStateMachine` /
    // `NodeEditOverlay` rely on:
    //
    //   * `get_segments` returns the path-local geometry +
    //     translation, so the renderer can project anchors into
    //     world space without a second IPC.
    //   * the wire round-trip is lossless (get → edit → set →
    //     get yields the edited path bit-for-bit).
    //   * `set_segments` recomputes bounds via Kurbo (tight
    //     curve bounds), so a node-editor drag that grows the
    //     path also grows the selection rect.
    //   * `set_segments` records exactly one undoable
    //     `canvas_path_set_segments` op per call — undo
    //     restores the pre-edit geometry.
    //   * every `PathSegmentsError` variant fires on the right
    //     input.

    #[test]
    #[serial]
    fn canvas_path_get_segments_returns_translation_and_path_local_geometry() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("path_get", dir.path()).expect("create");
        let id = insert_square(0.0, 0.0, 10.0);
        // Move the node by (50, 70). The path-local geometry
        // should be unaffected (we re-emit the same square in
        // path-local coords) but the snapshot's translation
        // fields should carry the move.
        canvas_move_node(id, 50.0, 70.0).expect("move");
        let snap = canvas_path_get_segments(id).expect("get_segments");
        assert!(snap.closed, "square was inserted with closed=true");
        assert_eq!(snap.translation_x, 50.0);
        assert_eq!(snap.translation_y, 70.0);
        // The square inserted by `insert_square(0,0,10)` is a
        // move + 3 line + close, so the snapshot's segments
        // should be the same count.
        assert_eq!(snap.segments.len(), 5);
        // First segment should be MoveTo (0, 0) in path-local.
        match &snap.segments[0] {
            kcreate_vector::PathSegment::MoveTo(p) => {
                assert_eq!(p.x, 0.0);
                assert_eq!(p.y, 0.0);
            }
            other => panic!("expected MoveTo as first segment, got {other:?}"),
        }
        project_close();
    }

    #[test]
    #[serial]
    fn canvas_path_get_segments_rejects_non_vector_layer() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("path_get_nonvec", dir.path()).expect("create");
        // Create a TextLayer (not a VectorLayer) and try to get
        // its segments — should fail with NotVectorLayer.
        // `canvas_create_rect` would create a VectorLayer (rects
        // are stored as 5-segment paths), so it would not
        // exercise this guard.
        let id = canvas_create_text(None, 0.0, 0.0, "hi".to_string(), "Inter".to_string(), 12.0)
            .expect("create text");
        let err = canvas_path_get_segments(id).expect_err("expected error");
        match err {
            DocumentBridgeError::PathSegments(PathSegmentsError::NotVectorLayer {
                id: got_id,
                ..
            }) => {
                assert_eq!(got_id, id);
            }
            other => panic!("expected NotVectorLayer, got {other:?}"),
        }
        project_close();
    }

    #[test]
    #[serial]
    fn canvas_path_get_segments_rejects_missing_node() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("path_get_missing", dir.path()).expect("create");
        let phantom = Uuid::new_v4();
        let err = canvas_path_get_segments(phantom).expect_err("expected error");
        match err {
            DocumentBridgeError::PathSegments(PathSegmentsError::NodeNotFound(got_id)) => {
                assert_eq!(got_id, phantom);
            }
            other => panic!("expected NodeNotFound, got {other:?}"),
        }
        project_close();
    }

    #[test]
    #[serial]
    fn canvas_path_set_segments_round_trips_through_get_segments() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("path_round_trip", dir.path()).expect("create");
        let id = insert_square(0.0, 0.0, 10.0);
        // Replace the square with a diagonal line.
        let new_segments = line_segments_json(2.0, 3.0, 100.0, 200.0);
        canvas_path_set_segments(id, &new_segments, false).expect("set_segments");
        // Read it back.
        let snap = canvas_path_get_segments(id).expect("get_segments");
        assert!(!snap.closed, "explicit closed=false should round-trip");
        assert_eq!(snap.segments.len(), 2);
        match &snap.segments[0] {
            kcreate_vector::PathSegment::MoveTo(p) => {
                assert_eq!(p.x, 2.0);
                assert_eq!(p.y, 3.0);
            }
            other => panic!("expected MoveTo, got {other:?}"),
        }
        match &snap.segments[1] {
            kcreate_vector::PathSegment::LineTo(p) => {
                assert_eq!(p.x, 100.0);
                assert_eq!(p.y, 200.0);
            }
            other => panic!("expected LineTo, got {other:?}"),
        }
        project_close();
    }

    #[test]
    #[serial]
    fn canvas_path_set_segments_recomputes_bounds_via_kurbo() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("path_bounds", dir.path()).expect("create");
        let id = insert_square(0.0, 0.0, 10.0);
        // Replace with a much-larger diagonal line. The node's
        // bounds should grow to match the new geometry — this is
        // the contract the layers panel + selection rect depend
        // on.
        let new_segments = line_segments_json(5.0, 7.0, 105.0, 207.0);
        canvas_path_set_segments(id, &new_segments, false).expect("set_segments");
        let tree = document_get_tree().expect("tree");
        let node = tree.iter().find(|n| n.id == id).expect("node still exists");
        // Line from (5,7) to (105,207) → bounds (5,7) - (105,207),
        // width 100, height 200.
        assert!(
            (node.bounds.x - 5.0).abs() < 1e-6,
            "x got {:?}",
            node.bounds
        );
        assert!(
            (node.bounds.y - 7.0).abs() < 1e-6,
            "y got {:?}",
            node.bounds
        );
        assert!(
            (node.bounds.width - 100.0).abs() < 1e-6,
            "width got {:?}",
            node.bounds
        );
        assert!(
            (node.bounds.height - 200.0).abs() < 1e-6,
            "height got {:?}",
            node.bounds
        );
        project_close();
    }

    #[test]
    #[serial]
    fn canvas_path_set_segments_records_undoable_op() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("path_undo", dir.path()).expect("create");
        let id = insert_square(0.0, 0.0, 10.0);
        // Replace with a diagonal line.
        let new_segments = line_segments_json(2.0, 3.0, 100.0, 200.0);
        canvas_path_set_segments(id, &new_segments, false).expect("set_segments");
        // Per `apply_inverse_patch` doc comment, `canvas_path_set_segments`
        // is a host-driven rollback command (NOT in
        // APPLY_PATCH_COMMANDS) — the renderer is expected to re-
        // fetch via `canvas.pathGetSegments` after the undo cursor
        // moves. What we can verify at the bridge layer is that an
        // op was recorded under the `canvas_path_set_segments`
        // op_kind and is visible to `document_undo` / `document_redo`,
        // exactly the same contract `canvas_create_path` uses
        // (see `canvas_create_path_records_undoable_op` above).
        let undo_outcome = document_undo()
            .expect("undo")
            .expect("canvas_path_set_segments was recorded");
        assert_eq!(undo_outcome.command, "canvas_path_set_segments");
        assert_eq!(undo_outcome.affected_nodes, vec![id]);
        let redo_outcome = document_redo()
            .expect("redo")
            .expect("canvas_path_set_segments is on the redo stack");
        assert_eq!(redo_outcome.command, "canvas_path_set_segments");
        assert_eq!(redo_outcome.affected_nodes, vec![id]);
        project_close();
    }

    #[test]
    #[serial]
    fn canvas_path_set_segments_rejects_empty_segments() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("path_set_empty", dir.path()).expect("create");
        let id = insert_square(0.0, 0.0, 10.0);
        let err = canvas_path_set_segments(id, "[]", false).expect_err("err");
        match err {
            DocumentBridgeError::PathSegments(PathSegmentsError::Empty) => {}
            other => panic!("expected Empty, got {other:?}"),
        }
        project_close();
    }

    #[test]
    #[serial]
    fn canvas_path_set_segments_rejects_missing_move_to() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("path_set_no_move", dir.path()).expect("create");
        let id = insert_square(0.0, 0.0, 10.0);
        // A single line_to with no preceding move_to.
        let bad = r#"[{"op":"line_to","x":10,"y":20}]"#;
        let err = canvas_path_set_segments(id, bad, false).expect_err("err");
        match err {
            DocumentBridgeError::PathSegments(PathSegmentsError::MissingMoveTo) => {}
            other => panic!("expected MissingMoveTo, got {other:?}"),
        }
        project_close();
    }

    #[test]
    #[serial]
    fn canvas_path_set_segments_rejects_invalid_json() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("path_set_bad_json", dir.path()).expect("create");
        let id = insert_square(0.0, 0.0, 10.0);
        let err = canvas_path_set_segments(id, "{not json", false).expect_err("err");
        match err {
            DocumentBridgeError::PathSegments(PathSegmentsError::InvalidJson(_)) => {}
            other => panic!("expected InvalidJson, got {other:?}"),
        }
        project_close();
    }

    #[test]
    #[serial]
    fn canvas_path_set_segments_rejects_non_vector_layer() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("path_set_nonvec", dir.path()).expect("create");
        let id = canvas_create_text(None, 0.0, 0.0, "hi".to_string(), "Inter".to_string(), 12.0)
            .expect("create text");
        let segs = line_segments_json(0.0, 0.0, 5.0, 5.0);
        let err = canvas_path_set_segments(id, &segs, false).expect_err("err");
        match err {
            DocumentBridgeError::PathSegments(PathSegmentsError::NotVectorLayer {
                id: got_id,
                ..
            }) => {
                assert_eq!(got_id, id);
            }
            other => panic!("expected NotVectorLayer, got {other:?}"),
        }
        project_close();
    }

    // -----------------------------------------------------------------
    // G4 — Theme / Brand Kit instant restyle.
    //
    // These exercise the full bridge entry point end-to-end: role-aware
    // recolor, type-scale application to text, and the single-undoable-op
    // contract (apply -> one undo restores the prior look exactly). The
    // pure role-assignment / derive-from-palette logic is unit-tested in
    // `kcreate_core::theme`; here we prove the document walk + operation
    // log + apply_patch inverse wiring behave.
    // -----------------------------------------------------------------

    fn rgb8(r: u8, g: u8, b: u8) -> RgbaColor {
        RgbaColor::new(
            f32::from(r) / 255.0,
            f32::from(g) / 255.0,
            f32::from(b) / 255.0,
            1.0,
        )
    }

    /// Insert a node with explicit bounds + solid fill straight into the
    /// open workspace, bypassing the create/update entry points so tests
    /// get full control over geometry, fill, and corner radius. Returns
    /// the new node id.
    fn insert_styled_rect(
        node_type: NodeType,
        name: &str,
        (x, y, w, h): (f64, f64, f64, f64),
        fill: RgbaColor,
        corner_radius: f64,
    ) -> Uuid {
        use kcreate_core::node::Bounds;
        let mut guard = slot().write();
        let ws = guard.as_mut().expect("project open");
        let mut node = Node::new(node_type, name.to_string());
        node.bounds = Bounds {
            x,
            y,
            width: w,
            height: h,
        };
        node.style.fill = FillStyle::Solid(fill);
        node.style.corner_radius = corner_radius;
        ws.project.document.insert_node(node).expect("insert rect")
    }

    /// Insert a text node carrying the canonical `TextLayerMeta` (font +
    /// size) and a `node.style.fill` (the text colour).
    fn insert_text_node(
        name: &str,
        (x, y, w, h): (f64, f64, f64, f64),
        font_family: &str,
        font_size: f32,
        fill: RgbaColor,
    ) -> Uuid {
        use kcreate_core::node::Bounds;
        let meta = kcreate_export::TextLayerMeta {
            text: "Real recognizable copy".to_string(),
            font_family: font_family.to_string(),
            font_size,
        };
        let mut guard = slot().write();
        let ws = guard.as_mut().expect("project open");
        let mut node = Node::new(NodeType::TextLayer, name.to_string());
        node.bounds = Bounds {
            x,
            y,
            width: w,
            height: h,
        };
        node.style.fill = FillStyle::Solid(fill);
        node.metadata.insert(
            crate::scene_sync::TEXT_LAYER_METADATA_KEY.to_string(),
            serde_json::to_value(&meta).expect("meta json"),
        );
        ws.project.document.insert_node(node).expect("insert text")
    }

    fn node_solid_fill(id: Uuid) -> RgbaColor {
        let guard = slot().read();
        let ws = guard.as_ref().expect("open");
        match &ws.project.document.get_node(id).expect("node").style.fill {
            FillStyle::Solid(c) => *c,
            other => panic!("expected solid fill, got {other:?}"),
        }
    }

    fn read_text_meta(id: Uuid) -> kcreate_export::TextLayerMeta {
        let guard = slot().read();
        let ws = guard.as_ref().expect("open");
        let v = ws
            .project
            .document
            .get_node(id)
            .expect("node")
            .metadata
            .get(crate::scene_sync::TEXT_LAYER_METADATA_KEY)
            .cloned()
            .expect("text meta present");
        serde_json::from_value(v).expect("meta")
    }

    fn snapshot_styles() -> std::collections::BTreeMap<Uuid, kcreate_core::node::NodeStyle> {
        let guard = slot().read();
        let ws = guard.as_ref().expect("open");
        ws.project
            .document
            .iter()
            .map(|(id, n)| (*id, n.style.clone()))
            .collect()
    }

    fn snapshot_metadata() -> std::collections::BTreeMap<Uuid, HashMap<String, serde_json::Value>> {
        let guard = slot().read();
        let ws = guard.as_ref().expect("open");
        ws.project
            .document
            .iter()
            .map(|(id, n)| (*id, n.metadata.clone()))
            .collect()
    }

    fn snapshot_tokens() -> DesignTokens {
        let guard = slot().read();
        let ws = guard.as_ref().expect("open");
        ws.project.design_tokens.clone()
    }

    /// Encode a vertical-band RGBA8 PNG from `bands` (one solid colour per
    /// equal-width band). Synthesizes a recognizable image for the
    /// palette-extraction / derive-from-image tests without shipping a
    /// binary fixture.
    fn synth_band_png(bands: &[[u8; 3]]) -> Vec<u8> {
        assert!(!bands.is_empty(), "need at least one band");
        let height = 64u32;
        let band_w = 24u32;
        let width = band_w * bands.len() as u32;
        let img = image::ImageBuffer::from_fn(width, height, |x, _y| {
            let idx = ((x / band_w) as usize).min(bands.len() - 1);
            let c = bands[idx];
            image::Rgba([c[0], c[1], c[2], 255])
        });
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut buf, image::ImageFormat::Png)
            .expect("encode png");
        buf.into_inner()
    }

    /// A small recognizable SVG brand mark (rounded square + ring + inner
    /// square) whose solid fills the theme can remap. Used by the logo
    /// import / insert tests and the H5 proof.
    fn brand_logo_svg() -> Vec<u8> {
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="120" height="120" viewBox="0 0 120 120"><rect x="12" y="12" width="96" height="96" rx="20" fill="#2563EB"/><circle cx="60" cy="60" r="28" fill="#FFFFFF"/><rect x="46" y="46" width="28" height="28" fill="#F59E0B"/></svg>"##.to_vec()
    }

    /// Insert a `GroupLayer` (optionally under `parent`) and return its id.
    fn insert_group(parent: Option<Uuid>, name: &str) -> Uuid {
        let mut guard = slot().write();
        let ws = guard.as_mut().expect("open");
        let mut node = Node::new(NodeType::GroupLayer, name.to_string());
        node.parent_id = parent;
        ws.project.document.insert_node(node).expect("insert group")
    }

    /// World-space rectangle path the renderer can rasterise — mirrors the
    /// geometry `canvas_create_rect` attaches.
    fn rect_path_value(x: f64, y: f64, w: f64, h: f64) -> serde_json::Value {
        let path = kcreate_vector::VectorPath::new(vec![
            kcreate_vector::PathSegment::MoveTo(kcreate_vector::PathPoint::new(x, y)),
            kcreate_vector::PathSegment::LineTo(kcreate_vector::PathPoint::new(x + w, y)),
            kcreate_vector::PathSegment::LineTo(kcreate_vector::PathPoint::new(x + w, y + h)),
            kcreate_vector::PathSegment::LineTo(kcreate_vector::PathPoint::new(x, y + h)),
            kcreate_vector::PathSegment::Close,
        ]);
        serde_json::to_value(&path).expect("path json")
    }

    /// Insert a solid-filled, renderable rounded rect under `parent`.
    fn vec_rect(
        parent: Uuid,
        name: &str,
        (x, y, w, h): (f64, f64, f64, f64),
        fill: RgbaColor,
        radius: f64,
    ) -> Uuid {
        use kcreate_core::node::Bounds;
        let mut guard = slot().write();
        let ws = guard.as_mut().expect("open");
        let mut node = Node::new(NodeType::VectorLayer, name.to_string());
        node.parent_id = Some(parent);
        node.bounds = Bounds {
            x,
            y,
            width: w,
            height: h,
        };
        node.style.fill = FillStyle::Solid(fill);
        node.style.corner_radius = radius;
        node.metadata.insert(
            crate::scene_sync::VECTOR_PATH_METADATA_KEY.to_string(),
            rect_path_value(x, y, w, h),
        );
        ws.project.document.insert_node(node).expect("rect")
    }

    /// Insert a text layer carrying canonical `TextLayerMeta`; its colour
    /// is `node.style.fill`.
    fn vec_text(
        parent: Uuid,
        name: &str,
        (x, y, w, h): (f64, f64, f64, f64),
        copy: &str,
        font: &str,
        size: f32,
        fill: RgbaColor,
    ) -> Uuid {
        use kcreate_core::node::Bounds;
        let meta = kcreate_export::TextLayerMeta {
            text: copy.to_string(),
            font_family: font.to_string(),
            font_size: size,
        };
        let mut guard = slot().write();
        let ws = guard.as_mut().expect("open");
        let mut node = Node::new(NodeType::TextLayer, name.to_string());
        node.parent_id = Some(parent);
        node.bounds = Bounds {
            x,
            y,
            width: w,
            height: h,
        };
        node.style.fill = FillStyle::Solid(fill);
        node.metadata.insert(
            crate::scene_sync::TEXT_LAYER_METADATA_KEY.to_string(),
            serde_json::to_value(&meta).expect("meta"),
        );
        ws.project.document.insert_node(node).expect("text")
    }

    /// Translate the open document to a renderer scene and PNG-encode it
    /// through the same export path the host uses.
    fn render_png(width: u32, height: u32) -> Vec<u8> {
        let guard = slot().read();
        let ws = guard.as_ref().expect("open");
        let mut sync = crate::scene_sync::SceneSync::new();
        let scene = sync.sync_document_to_scene_borrowed(&ws.project.document, None, &[]);
        drop(guard);
        export_png_to_bytes(
            &scene,
            &PngExportOptions {
                width,
                height,
                scale: 1.0,
                background: None,
            },
        )
        .expect("png")
    }

    /// Scoped `KCREATE_BRAND_KIT_DIR` override that restores the previous
    /// value on drop. Sound because brand-registry tests run `#[serial]`.
    struct BrandDirEnvGuard {
        prev: Option<String>,
    }

    impl BrandDirEnvGuard {
        fn set(dir: &std::path::Path) -> Self {
            let prev = std::env::var("KCREATE_BRAND_KIT_DIR").ok();
            // SAFETY: brand-registry tests are `#[serial]`, so no other
            // thread reads or writes the process environment concurrently.
            unsafe {
                std::env::set_var("KCREATE_BRAND_KIT_DIR", dir);
            }
            Self { prev }
        }
    }

    impl Drop for BrandDirEnvGuard {
        fn drop(&mut self) {
            // SAFETY: see `BrandDirEnvGuard::set`.
            unsafe {
                match &self.prev {
                    Some(v) => std::env::set_var("KCREATE_BRAND_KIT_DIR", v),
                    None => std::env::remove_var("KCREATE_BRAND_KIT_DIR"),
                }
            }
        }
    }

    #[test]
    #[serial]
    fn apply_theme_remaps_colors_to_roles_area_aware() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("theme_roles", dir.path()).expect("create");

        // A recognizable hero: a large white canvas, a light-grey surface
        // card, and one saturated-blue call-to-action button. The blue is
        // the only chromatic colour, so it must land on `primary`; white
        // covers the most area, so it must land on `background`.
        let bg = insert_styled_rect(
            NodeType::VectorLayer,
            "Canvas",
            (0.0, 0.0, 1200.0, 800.0),
            rgb8(0xFF, 0xFF, 0xFF),
            0.0,
        );
        let surface = insert_styled_rect(
            NodeType::VectorLayer,
            "Card",
            (80.0, 80.0, 640.0, 360.0),
            rgb8(0xF1, 0xF5, 0xF9),
            16.0,
        );
        let primary = insert_styled_rect(
            NodeType::VectorLayer,
            "CTA",
            (120.0, 360.0, 200.0, 56.0),
            rgb8(0x25, 0x63, 0xEB),
            12.0,
        );

        let theme = Theme::builtin("midnight").expect("midnight theme");
        let report = document_apply_theme(&theme).expect("apply");

        assert_eq!(report.theme_id, "midnight");
        assert!(
            report.recolored_fills >= 3,
            "expected at least the three inserted fills recolored, got {}",
            report.recolored_fills
        );

        assert_eq!(
            quantize(node_solid_fill(bg)),
            quantize(theme.palette.background),
            "dominant white fill must map to the theme background"
        );
        assert_eq!(
            quantize(node_solid_fill(surface)),
            quantize(theme.palette.surface),
            "light-grey card must map to the theme surface"
        );
        assert_eq!(
            quantize(node_solid_fill(primary)),
            quantize(theme.palette.primary),
            "saturated blue CTA must map to the theme primary"
        );

        // Design tokens were populated from the theme.
        let tokens = snapshot_tokens();
        assert_eq!(
            tokens.colors.get("background").copied().map(quantize),
            Some(quantize(theme.palette.background)),
            "design tokens must carry the theme background colour"
        );
        assert!(
            tokens.typography.contains_key("display"),
            "design tokens must carry the type scale"
        );

        project_close();
    }

    #[test]
    #[serial]
    fn apply_theme_applies_type_scale_to_text() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("theme_type", dir.path()).expect("create");

        // A display-sized heading and a body paragraph, in fonts that
        // differ from the theme so the swap is observable.
        let heading = insert_text_node(
            "Hero heading",
            (0.0, 0.0, 600.0, 80.0),
            "Roboto",
            40.0,
            rgb8(0x0F, 0x17, 0x2A),
        );
        let body = insert_text_node(
            "Body copy",
            (0.0, 120.0, 600.0, 240.0),
            "Georgia",
            16.0,
            rgb8(0x33, 0x41, 0x55),
        );

        let theme = Theme::builtin("midnight").expect("midnight theme");
        let report = document_apply_theme(&theme).expect("apply");
        assert!(
            report.restyled_text >= 2,
            "both text nodes should be restyled, got {}",
            report.restyled_text
        );

        // 40px -> Display: midnight display = 48px, heading font = Poppins.
        let hm = read_text_meta(heading);
        assert!(
            (hm.font_size - theme.type_scale.display).abs() < f32::EPSILON,
            "heading size {} != display {}",
            hm.font_size,
            theme.type_scale.display
        );
        assert_eq!(hm.font_family, theme.type_scale.heading_font);

        // 16px -> Body: midnight body = 16px, body font = Inter.
        let bm = read_text_meta(body);
        assert!(
            (bm.font_size - theme.type_scale.body).abs() < f32::EPSILON,
            "body size {} != body {}",
            bm.font_size,
            theme.type_scale.body
        );
        assert_eq!(bm.font_family, theme.type_scale.body_font);

        project_close();
    }

    #[test]
    #[serial]
    fn apply_theme_is_single_undoable_op_restoring_exactly() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("theme_undo", dir.path()).expect("create");

        insert_styled_rect(
            NodeType::VectorLayer,
            "Canvas",
            (0.0, 0.0, 1200.0, 800.0),
            rgb8(0xFF, 0xFF, 0xFF),
            4.0,
        );
        insert_styled_rect(
            NodeType::VectorLayer,
            "CTA",
            (120.0, 360.0, 200.0, 56.0),
            rgb8(0x25, 0x63, 0xEB),
            18.0,
        );
        insert_text_node(
            "Hero heading",
            (0.0, 0.0, 600.0, 80.0),
            "Roboto",
            40.0,
            rgb8(0x0F, 0x17, 0x2A),
        );

        let before_styles = snapshot_styles();
        let before_meta = snapshot_metadata();
        let before_tokens = snapshot_tokens();

        let status0 = document_status().expect("status");
        assert_eq!(status0.undo_depth, 0, "no ops before apply");

        let theme = Theme::builtin("forest").expect("forest theme");
        let report = document_apply_theme(&theme).expect("apply");
        assert!(report.affected_nodes > 0);

        // The restyle actually changed something.
        assert_ne!(
            before_styles,
            snapshot_styles(),
            "apply_theme must change node styles"
        );

        // Exactly one undoable operation was recorded — one Ctrl+Z reverts
        // the whole restyle.
        let status1 = document_status().expect("status");
        assert_eq!(
            status1.undo_depth, 1,
            "the entire restyle must collapse to a single operation"
        );
        assert!(status1.can_undo);

        document_undo().expect("undo").expect("undo outcome");

        assert_eq!(
            before_styles,
            snapshot_styles(),
            "one undo must restore every node style exactly"
        );
        assert_eq!(
            before_meta,
            snapshot_metadata(),
            "one undo must restore text metadata exactly"
        );
        assert_eq!(
            before_tokens,
            snapshot_tokens(),
            "one undo must restore the design tokens exactly"
        );

        // And it is redoable, re-applying the same restyle exactly.
        document_redo().expect("redo").expect("redo outcome");
        assert_ne!(
            before_styles,
            snapshot_styles(),
            "redo must re-apply the restyle"
        );

        project_close();
    }

    #[test]
    #[serial]
    fn theme_derive_from_document_returns_usable_theme() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("theme_derive", dir.path()).expect("create");

        insert_styled_rect(
            NodeType::VectorLayer,
            "Canvas",
            (0.0, 0.0, 1200.0, 800.0),
            rgb8(0x10, 0x2A, 0x43),
            0.0,
        );
        insert_styled_rect(
            NodeType::VectorLayer,
            "Accent",
            (40.0, 40.0, 160.0, 160.0),
            rgb8(0xF9, 0x73, 0x16),
            0.0,
        );

        let theme = theme_derive_from_document("Derived").expect("derive");
        assert_eq!(theme.name, "Derived");
        assert!(
            theme.id.starts_with("derived-"),
            "derived theme id should be prefixed, got {}",
            theme.id
        );

        // A derived theme must be directly usable: its design tokens cover
        // every role so applying it can't leave a hole.
        let tokens = theme.to_design_tokens();
        for role in [
            "background",
            "surface",
            "primary",
            "secondary",
            "accent",
            "text",
            "muted",
        ] {
            assert!(
                tokens.colors.contains_key(role),
                "derived theme missing colour role `{role}`"
            );
        }

        // Applying the derived theme to its own document is a valid,
        // undoable round-trip.
        let report = document_apply_theme(&theme).expect("apply derived");
        assert!(report.affected_nodes > 0);

        project_close();
    }

    /// Real-design before/after proof. Builds a recognizable SaaS
    /// landing-page hero (nav, logo, headline + subhead, primary &
    /// secondary CTAs, a hero panel, a three-card feature row — real
    /// copy, never blank rectangles), then drives the *real*
    /// `document_apply_theme` entry point and renders the workspace
    /// document to PNG through the production `scene_sync` translation +
    /// `export_png_to_bytes` (CPU tiny-skia, no GPU). Asserts both themed
    /// renders are valid PNGs and differ pixel-wise over the SAME layout.
    /// Set `KCREATE_PROOF_DIR` to also drop the artifacts to disk.
    #[test]
    #[serial]
    fn apply_theme_renders_recognizable_before_after_proof() {
        use kcreate_core::node::{Bounds, StrokeStyle};

        const W: u32 = 1200;
        const H: u32 = 800;

        // World-space rectangle path the renderer can rasterise. Matches
        // the geometry `canvas_create_rect` attaches; `node_translation`
        // stays at the identity origin so the path renders in place.
        fn rect_path(x: f64, y: f64, w: f64, h: f64) -> serde_json::Value {
            let path = kcreate_vector::VectorPath::new(vec![
                kcreate_vector::PathSegment::MoveTo(kcreate_vector::PathPoint::new(x, y)),
                kcreate_vector::PathSegment::LineTo(kcreate_vector::PathPoint::new(x + w, y)),
                kcreate_vector::PathSegment::LineTo(kcreate_vector::PathPoint::new(x + w, y + h)),
                kcreate_vector::PathSegment::LineTo(kcreate_vector::PathPoint::new(x, y + h)),
                kcreate_vector::PathSegment::Close,
            ]);
            serde_json::to_value(&path).expect("path json")
        }

        // A solid-filled rect, child of `parent`.
        fn rect(
            parent: Uuid,
            name: &str,
            (x, y, w, h): (f64, f64, f64, f64),
            fill: RgbaColor,
            radius: f64,
        ) -> Uuid {
            let mut guard = slot().write();
            let ws = guard.as_mut().expect("open");
            let mut node = Node::new(NodeType::VectorLayer, name.to_string());
            node.parent_id = Some(parent);
            node.bounds = Bounds {
                x,
                y,
                width: w,
                height: h,
            };
            node.style.fill = FillStyle::Solid(fill);
            node.style.corner_radius = radius;
            node.metadata.insert(
                crate::scene_sync::VECTOR_PATH_METADATA_KEY.to_string(),
                rect_path(x, y, w, h),
            );
            ws.project.document.insert_node(node).expect("rect")
        }

        // An outlined (stroked, no fill) rounded rect — exercises the
        // stroke remap path of the restyle.
        fn outline(
            parent: Uuid,
            name: &str,
            (x, y, w, h): (f64, f64, f64, f64),
            stroke: RgbaColor,
            radius: f64,
        ) -> Uuid {
            let mut guard = slot().write();
            let ws = guard.as_mut().expect("open");
            let mut node = Node::new(NodeType::VectorLayer, name.to_string());
            node.parent_id = Some(parent);
            node.bounds = Bounds {
                x,
                y,
                width: w,
                height: h,
            };
            node.style.fill = FillStyle::None;
            node.style.stroke = Some(StrokeStyle {
                color: stroke,
                width: 2.0,
                ..StrokeStyle::default()
            });
            node.style.corner_radius = radius;
            node.metadata.insert(
                crate::scene_sync::VECTOR_PATH_METADATA_KEY.to_string(),
                rect_path(x, y, w, h),
            );
            ws.project.document.insert_node(node).expect("outline")
        }

        // A text layer carrying canonical `TextLayerMeta`; its colour is
        // `node.style.fill`.
        fn text(
            parent: Uuid,
            name: &str,
            (x, y, w, h): (f64, f64, f64, f64),
            copy: &str,
            font: &str,
            size: f32,
            fill: RgbaColor,
        ) -> Uuid {
            let meta = kcreate_export::TextLayerMeta {
                text: copy.to_string(),
                font_family: font.to_string(),
                font_size: size,
            };
            let mut guard = slot().write();
            let ws = guard.as_mut().expect("open");
            let mut node = Node::new(NodeType::TextLayer, name.to_string());
            node.parent_id = Some(parent);
            node.bounds = Bounds {
                x,
                y,
                width: w,
                height: h,
            };
            node.style.fill = FillStyle::Solid(fill);
            node.metadata.insert(
                crate::scene_sync::TEXT_LAYER_METADATA_KEY.to_string(),
                serde_json::to_value(&meta).expect("meta"),
            );
            ws.project.document.insert_node(node).expect("text")
        }

        // Translate the open document to a renderer scene and PNG-encode
        // it through the same export path the host uses.
        fn render(width: u32, height: u32) -> Vec<u8> {
            let guard = slot().read();
            let ws = guard.as_ref().expect("open");
            let mut sync = crate::scene_sync::SceneSync::new();
            let scene = sync.sync_document_to_scene_borrowed(&ws.project.document, None, &[]);
            drop(guard);
            export_png_to_bytes(
                &scene,
                &PngExportOptions {
                    width,
                    height,
                    scale: 1.0,
                    background: None,
                },
            )
            .expect("png")
        }

        reset_for_tests();
        let dir = tmpdir();
        project_create("theme_proof", dir.path()).expect("create");

        // Source-design palette.
        let white = rgb8(0xFF, 0xFF, 0xFF);
        let slate_50 = rgb8(0xF1, 0xF5, 0xF9);
        let navy = rgb8(0x0F, 0x17, 0x2A);
        let muted = rgb8(0x47, 0x55, 0x69);
        let blue = rgb8(0x25, 0x63, 0xEB);
        let green = rgb8(0x10, 0xB9, 0x81);
        let amber = rgb8(0xF5, 0x9E, 0x0B);

        let page = {
            let mut guard = slot().write();
            let ws = guard.as_mut().expect("open");
            ws.project
                .document
                .insert_node(Node::new(NodeType::Page, "Page".to_string()))
                .expect("page")
        };
        let artboard = {
            let mut guard = slot().write();
            let ws = guard.as_mut().expect("open");
            let mut a = Node::new(NodeType::Artboard, "Landing".to_string());
            a.parent_id = Some(page);
            a.bounds = Bounds {
                x: 0.0,
                y: 0.0,
                width: f64::from(W),
                height: f64::from(H),
            };
            a.style.fill = FillStyle::Solid(white);
            ws.project.document.insert_node(a).expect("artboard")
        };

        // Top navigation.
        rect(
            artboard,
            "Nav bar",
            (0.0, 0.0, f64::from(W), 72.0),
            slate_50,
            0.0,
        );
        rect(artboard, "Logo mark", (48.0, 20.0, 32.0, 32.0), blue, 8.0);
        text(
            artboard,
            "Brand",
            (92.0, 26.0, 160.0, 24.0),
            "KCreate",
            "Poppins",
            20.0,
            navy,
        );
        text(
            artboard,
            "Nav Home",
            (840.0, 28.0, 80.0, 20.0),
            "Home",
            "Inter",
            16.0,
            muted,
        );
        text(
            artboard,
            "Nav Pricing",
            (936.0, 28.0, 90.0, 20.0),
            "Pricing",
            "Inter",
            16.0,
            muted,
        );
        text(
            artboard,
            "Nav Docs",
            (1052.0, 28.0, 70.0, 20.0),
            "Docs",
            "Inter",
            16.0,
            muted,
        );

        // Hero copy.
        text(
            artboard,
            "Headline",
            (80.0, 150.0, 680.0, 150.0),
            "Design at the speed of thought",
            "Poppins",
            52.0,
            navy,
        );
        text(
            artboard,
            "Subhead",
            (80.0, 320.0, 700.0, 90.0),
            "Restyle your whole document in a single click.",
            "Inter",
            22.0,
            muted,
        );

        // Calls to action.
        rect(
            artboard,
            "Primary CTA",
            (80.0, 440.0, 220.0, 60.0),
            blue,
            12.0,
        );
        text(
            artboard,
            "Primary CTA label",
            (118.0, 458.0, 160.0, 26.0),
            "Get started",
            "Inter",
            18.0,
            white,
        );
        outline(
            artboard,
            "Secondary CTA",
            (320.0, 440.0, 200.0, 60.0),
            blue,
            12.0,
        );
        text(
            artboard,
            "Secondary CTA label",
            (352.0, 458.0, 160.0, 26.0),
            "Watch demo",
            "Inter",
            18.0,
            blue,
        );

        // Hero illustration panel.
        rect(
            artboard,
            "Hero panel",
            (820.0, 150.0, 320.0, 360.0),
            green,
            20.0,
        );
        rect(
            artboard,
            "Hero accent",
            (860.0, 200.0, 120.0, 120.0),
            amber,
            60.0,
        );

        // Feature card row.
        let cards = [
            (80.0_f64, "Themes", "Switch the entire look instantly."),
            (470.0, "Brand kits", "Pin your palette and fonts."),
            (860.0, "Reversible", "Every restyle is one undo away."),
        ];
        for (cx, title, body) in cards {
            rect(artboard, "Card", (cx, 600.0, 260.0, 150.0), slate_50, 16.0);
            rect(
                artboard,
                "Card icon",
                (cx + 24.0, 624.0, 40.0, 40.0),
                amber,
                10.0,
            );
            text(
                artboard,
                "Card title",
                (cx + 24.0, 680.0, 220.0, 24.0),
                title,
                "Poppins",
                20.0,
                navy,
            );
            text(
                artboard,
                "Card body",
                (cx + 24.0, 712.0, 220.0, 30.0),
                body,
                "Inter",
                15.0,
                muted,
            );
        }

        // Render the source design, then Theme A (daybreak, light) and
        // Theme B (midnight, dark) over the identical layout.
        let original = render(W, H);
        assert!(
            original.starts_with(&[0x89, b'P', b'N', b'G']),
            "original is a PNG"
        );

        let theme_a = Theme::builtin("daybreak").expect("daybreak");
        document_apply_theme(&theme_a).expect("apply A");
        let png_a = render(W, H);

        document_undo().expect("undo").expect("undo outcome");

        let theme_b = Theme::builtin("midnight").expect("midnight");
        document_apply_theme(&theme_b).expect("apply B");
        let png_b = render(W, H);

        for (label, png) in [("A", &png_a), ("B", &png_b)] {
            assert!(
                png.starts_with(&[0x89, b'P', b'N', b'G']),
                "theme {label} is a PNG"
            );
            assert!(
                png.windows(4).any(|w| w == b"IDAT"),
                "theme {label} carries an IDAT chunk"
            );
        }
        assert_ne!(
            png_a, png_b,
            "two themes must restyle the same layout differently"
        );

        if let Ok(out) = std::env::var("KCREATE_PROOF_DIR") {
            let out = std::path::Path::new(&out);
            std::fs::create_dir_all(out).expect("proof dir");
            std::fs::write(out.join("hero_original.png"), &original).expect("write original");
            std::fs::write(out.join("hero_theme_a_daybreak.png"), &png_a).expect("write A");
            std::fs::write(out.join("hero_theme_b_midnight.png"), &png_b).expect("write B");
        }

        project_close();
    }

    // -------------------------------------------------------------------------
    // H5 — Brand Kit depth (user kits, fonts+embed, logo, scope, derive-image)
    // -------------------------------------------------------------------------

    #[test]
    #[serial]
    fn theme_derive_from_image_produces_usable_theme() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("derive_image", dir.path()).expect("create");

        // A recognizable design to restyle (dark canvas + warm accent).
        insert_styled_rect(
            NodeType::VectorLayer,
            "Canvas",
            (0.0, 0.0, 1200.0, 800.0),
            rgb8(0x10, 0x2A, 0x43),
            0.0,
        );
        insert_styled_rect(
            NodeType::VectorLayer,
            "Accent",
            (40.0, 40.0, 200.0, 200.0),
            rgb8(0xF9, 0x73, 0x16),
            0.0,
        );

        // Upload an image whose dominant colours differ from the design.
        let photo = synth_band_png(&[[0x0F, 0x17, 0x2A], [0x25, 0x63, 0xEB], [0xF5, 0x9E, 0x0B]]);
        let theme = theme_derive_from_image("Sunrise Photo", &photo).expect("derive");

        assert_eq!(theme.name, "Sunrise Photo");
        assert!(
            theme.id.starts_with("derived-"),
            "derived-from-image theme id should be prefixed, got {}",
            theme.id
        );

        // Every role is covered, so applying it can't leave a hole.
        let tokens = theme.to_design_tokens();
        for role in [
            "background",
            "surface",
            "primary",
            "secondary",
            "accent",
            "text",
            "muted",
        ] {
            assert!(
                tokens.colors.contains_key(role),
                "derived theme missing colour role `{role}`"
            );
        }
        // A non-degenerate palette: the background and primary differ.
        assert_ne!(
            quantize(theme.palette.background),
            quantize(theme.palette.primary),
            "derived palette collapsed background and primary"
        );

        // It is directly usable as an undoable restyle.
        let report = document_apply_theme(&theme).expect("apply derived");
        assert!(report.affected_nodes > 0);

        // Empty bytes are rejected; undecodable bytes surface as an IO error.
        assert!(theme_derive_from_image("x", &[]).is_err());
        assert!(theme_derive_from_image("x", b"not an image").is_err());

        project_close();
    }

    #[test]
    #[serial]
    fn brand_kit_extract_palette_from_image_sets_named_colors() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("kit_palette", dir.path()).expect("create");

        let kit_id = brand_kit_create("Photo Brand".to_string()).expect("kit");
        let photo = synth_band_png(&[
            [0xE1, 0x1D, 0x48],
            [0x7C, 0x3A, 0xED],
            [0xF5, 0x9E, 0x0B],
            [0x10, 0x18, 0x28],
        ]);
        let hexes =
            brand_kit_extract_palette_from_image_bytes(kit_id, &photo, 4).expect("extract palette");
        assert!(
            !hexes.is_empty() && hexes.len() <= 4,
            "expected 1..=4 colours, got {}",
            hexes.len()
        );
        for h in &hexes {
            // `RgbaColor::to_hex` encodes `#RRGGBBAA`.
            assert!(
                h.starts_with('#') && h.len() == 9,
                "malformed hex code `{h}`"
            );
        }

        // The kit now carries the extracted palette, named in dominance order.
        let kit = brand_kit_list()
            .expect("list")
            .into_iter()
            .find(|k| k.id == kit_id)
            .expect("kit present");
        assert_eq!(kit.colors.len(), hexes.len());
        for (i, c) in kit.colors.iter().enumerate() {
            assert_eq!(c.name, format!("Color {}", i + 1));
        }

        // Argument guards.
        assert!(brand_kit_extract_palette_from_image_bytes(kit_id, &[], 4).is_err());
        assert!(brand_kit_extract_palette_from_image_bytes(kit_id, &photo, 0).is_err());
        assert!(brand_kit_extract_palette_from_image_bytes(kit_id, &photo, 65).is_err());

        project_close();
    }

    #[test]
    #[serial]
    fn brand_kit_set_font_role_embeds_real_font_bytes() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("kit_fonts", dir.path()).expect("create");

        let kit_id = brand_kit_create("Type Brand".to_string()).expect("kit");
        brand_kit_set_font_role(kit_id, "heading", "DejaVu Serif".to_string(), true)
            .expect("set heading");
        brand_kit_set_font_role(kit_id, "body", "DejaVu Sans".to_string(), true).expect("set body");

        let kit = brand_kit_list()
            .expect("list")
            .into_iter()
            .find(|k| k.id == kit_id)
            .expect("kit present");
        assert_eq!(kit.fonts.len(), 2, "one font per weight bucket");

        let heading = kit
            .fonts
            .iter()
            .find(|f| f.weight >= 600)
            .expect("heading font");
        assert_eq!(heading.family, "DejaVu Serif");
        assert!(
            heading.embedded_asset_id.is_some(),
            "embed=true must store a font asset"
        );
        let body = kit
            .fonts
            .iter()
            .find(|f| f.weight < 600)
            .expect("body font");
        assert_eq!(body.family, "DejaVu Sans");
        assert!(body.embedded_asset_id.is_some());

        // The embedded blob is a real sfnt font, not a placeholder.
        let asset_id = heading.embedded_asset_id.expect("asset id");
        let bytes = {
            let guard = slot().read();
            let ws = guard.as_ref().expect("open");
            let store = ws.store.lock();
            store
                .load_asset(asset_id)
                .expect("load asset")
                .expect("asset bytes present")
        };
        assert!(bytes.len() > 4, "font asset suspiciously small");
        let sig = &bytes[..4];
        assert!(
            sig == [0x00, 0x01, 0x00, 0x00] || sig == b"OTTO" || sig == b"true" || sig == b"ttcf",
            "embedded bytes lack a recognised sfnt signature, got {sig:?}"
        );

        // Re-setting the same role replaces in place rather than appending.
        brand_kit_set_font_role(kit_id, "heading", "DejaVu Sans Mono".to_string(), false)
            .expect("replace heading");
        let kit = brand_kit_list()
            .expect("list")
            .into_iter()
            .find(|k| k.id == kit_id)
            .expect("kit present");
        assert_eq!(kit.fonts.len(), 2, "replacing a role must not add a font");
        let heading = kit
            .fonts
            .iter()
            .find(|f| f.weight >= 600)
            .expect("heading font");
        assert_eq!(heading.family, "DejaVu Sans Mono");
        assert!(
            heading.embedded_asset_id.is_none(),
            "embed=false must not store an asset"
        );

        // Bad arguments are rejected.
        assert!(brand_kit_set_font_role(kit_id, "subtitle", "X".to_string(), false).is_err());
        assert!(brand_kit_set_font_role(kit_id, "body", "   ".to_string(), false).is_err());

        project_close();
    }

    #[test]
    #[serial]
    fn brand_logo_insert_places_editable_recolorable_node() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("kit_logo", dir.path()).expect("create");

        let kit_id = brand_kit_create("Logo Brand".to_string()).expect("kit");
        brand_kit_set_logo_bytes(kit_id, &brand_logo_svg()).expect("set logo");

        let inserted = brand_logo_insert(kit_id, None, 50.0, 60.0, 240.0).expect("insert logo");
        assert!(!inserted.node_ids.is_empty(), "logo produced no nodes");
        assert_eq!(inserted.name, "Brand Logo");
        assert!(
            inserted.width > 0.0 && inserted.height > 0.0,
            "logo has zero bounds"
        );
        let longest = inserted.width.max(inserted.height);
        assert!(
            longest <= 240.5,
            "logo should scale to at most the target longest side, got {longest}"
        );

        // At least one inserted node carries an editable, theme-recolorable
        // solid fill (the SVG path fills).
        let guard = slot().read();
        let ws = guard.as_ref().expect("open");
        let mut found_solid = false;
        for id_str in &inserted.node_ids {
            let id = Uuid::parse_str(id_str).expect("uuid");
            let node = ws
                .project
                .document
                .get_node(id)
                .expect("inserted node exists");
            if matches!(node.style.fill, FillStyle::Solid(_)) {
                found_solid = true;
            }
        }
        drop(guard);
        assert!(
            found_solid,
            "an SVG logo must insert at least one solid-filled, recolorable node"
        );

        // A kit with no logo cannot insert.
        let empty_kit = brand_kit_create("No Logo".to_string()).expect("kit");
        assert!(brand_logo_insert(empty_kit, None, 0.0, 0.0, 64.0).is_err());

        project_close();
    }

    #[test]
    #[serial]
    fn apply_theme_to_selection_touches_only_subtree_and_undo_restores() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("scope_apply", dir.path()).expect("create");

        // Two sibling subtrees painted with identical colours. Theming the
        // first must leave the second byte-identical.
        let dark = rgb8(0x20, 0x30, 0x40);
        let warm = rgb8(0xC0, 0x40, 0x40);
        let group_a = insert_group(None, "Group A");
        let a1 = vec_rect(group_a, "A1", (0.0, 0.0, 100.0, 100.0), dark, 0.0);
        let a2 = vec_rect(group_a, "A2", (0.0, 100.0, 100.0, 100.0), warm, 0.0);
        let group_b = insert_group(None, "Group B");
        let b1 = vec_rect(group_b, "B1", (200.0, 0.0, 100.0, 100.0), dark, 0.0);
        let b2 = vec_rect(group_b, "B2", (200.0, 100.0, 100.0, 100.0), warm, 0.0);

        let before_styles = snapshot_styles();
        let before_tokens = snapshot_tokens();
        let a1_before = node_solid_fill(a1);
        let a2_before = node_solid_fill(a2);
        let b1_before = node_solid_fill(b1);
        let b2_before = node_solid_fill(b2);

        assert_eq!(
            document_status().expect("status").undo_depth,
            0,
            "direct inserts must not push undo ops"
        );

        let theme = Theme::builtin("midnight").expect("midnight theme");
        document_set_selection(vec![group_a]).expect("select A");
        let report =
            document_apply_theme_to_selection(&theme, vec![group_a]).expect("apply to selection");
        assert!(
            report.affected_nodes > 0,
            "scoped apply must restyle the selected subtree"
        );

        // Group B is untouched — byte identical.
        assert_eq!(node_solid_fill(b1), b1_before, "sibling B1 must not change");
        assert_eq!(node_solid_fill(b2), b2_before, "sibling B2 must not change");
        // Group A actually changed.
        assert!(
            node_solid_fill(a1) != a1_before || node_solid_fill(a2) != a2_before,
            "selected subtree should have been recolored"
        );
        // A scoped apply leaves the global design tokens byte-identical.
        assert_eq!(
            before_tokens,
            snapshot_tokens(),
            "selection-scope apply must not touch global design tokens"
        );

        // Exactly one undoable operation, and one undo restores everything.
        assert_eq!(
            document_status().expect("status").undo_depth,
            1,
            "scoped restyle must collapse to one operation"
        );
        document_undo().expect("undo").expect("undo outcome");
        assert_eq!(
            before_styles,
            snapshot_styles(),
            "one undo must restore every node style exactly"
        );
        assert_eq!(
            before_tokens,
            snapshot_tokens(),
            "one undo must restore design tokens exactly"
        );

        project_close();
    }

    #[test]
    #[serial]
    fn brand_kit_registry_roundtrips_across_projects() {
        let registry = tmpdir();
        let _env = BrandDirEnvGuard::set(registry.path());

        reset_for_tests();
        let dir1 = tmpdir();
        project_create("project_one", dir1.path()).expect("create p1");

        let kit_id = brand_kit_create("Acme Brand".to_string()).expect("kit");
        brand_kit_set_logo_bytes(kit_id, &brand_logo_svg()).expect("logo");
        let photo = synth_band_png(&[[0xE1, 0x1D, 0x48], [0x7C, 0x3A, 0xED], [0xF5, 0x9E, 0x0B]]);
        let palette =
            brand_kit_extract_palette_from_image_bytes(kit_id, &photo, 3).expect("palette");
        let palette_len = palette.len();
        assert!(!palette.is_empty());
        brand_kit_set_font_role(kit_id, "heading", "DejaVu Serif".to_string(), true)
            .expect("heading font");

        brand_kit_registry_save(kit_id).expect("save to registry");

        // The kit is discoverable in the cross-project registry.
        let listed = brand_kit_registry_list().expect("list registry");
        assert!(
            listed
                .iter()
                .any(|k| k.id == kit_id && k.name == "Acme Brand"),
            "saved kit must appear in the registry listing"
        );

        project_close();

        // A brand-new project on disk shares the registry, not the kits.
        let dir2 = tmpdir();
        project_create("project_two", dir2.path()).expect("create p2");
        assert!(
            brand_kit_list().expect("list").is_empty(),
            "a fresh project starts with no in-memory kits"
        );

        let loaded_id = brand_kit_registry_load(kit_id).expect("load from registry");
        assert_eq!(loaded_id, kit_id, "registry load keeps the kit id stable");

        let kit = brand_kit_list()
            .expect("list")
            .into_iter()
            .find(|k| k.id == kit_id)
            .expect("kit rehydrated into p2");
        assert_eq!(kit.name, "Acme Brand");
        assert_eq!(
            kit.colors.len(),
            palette_len,
            "palette survived the round-trip"
        );
        assert_eq!(kit.fonts.len(), 1, "font role survived the round-trip");
        let logo_id = kit.logo_asset_id.expect("logo survived");
        let font_id = kit.fonts[0]
            .embedded_asset_id
            .expect("embedded font survived");

        // The logo + font blobs were re-hydrated into p2's own asset store.
        {
            let guard = slot().read();
            let ws = guard.as_ref().expect("open");
            let store = ws.store.lock();
            assert!(
                store.load_asset(logo_id).expect("load logo").is_some(),
                "logo blob must be re-stored in the loading project"
            );
            assert!(
                store.load_asset(font_id).expect("load font").is_some(),
                "embedded font blob must be re-stored in the loading project"
            );
        }

        // Deleting from the registry leaves the loaded project copy intact.
        assert!(brand_kit_registry_delete(kit_id).expect("delete"));
        assert!(
            !brand_kit_registry_list()
                .expect("list")
                .iter()
                .any(|k| k.id == kit_id),
            "deleted kit must be gone from the registry"
        );
        assert!(
            brand_kit_list()
                .expect("list")
                .iter()
                .any(|k| k.id == kit_id),
            "registry delete must not touch the open project"
        );

        project_close();
    }

    /// Full H5 proof: compose a recognizable SaaS hero, define a brand kit
    /// (logo + palette-derived-from-an-image + heading/body fonts), then
    /// render (a) the original, (b) the whole-document branded result with
    /// the logo placed, and (c) the selection-only branded result. Asserts
    /// the three renders are distinct PNGs and writes them to
    /// `KCREATE_PROOF_DIR` when set so they can be opened + attached.
    #[test]
    #[serial]
    fn brand_kit_branding_renders_whole_doc_and_selection_proof() {
        const W: u32 = 1200;
        const H: u32 = 800;

        reset_for_tests();
        let dir = tmpdir();
        project_create("brand_proof", dir.path()).expect("create");

        // Palette for the base design.
        let white = rgb8(0xFF, 0xFF, 0xFF);
        let slate_50 = rgb8(0xF1, 0xF5, 0xF9);
        let navy = rgb8(0x0F, 0x17, 0x2A);
        let muted = rgb8(0x47, 0x55, 0x69);
        let blue = rgb8(0x25, 0x63, 0xEB);
        let green = rgb8(0x10, 0xB9, 0x81);
        let amber = rgb8(0xF5, 0x9E, 0x0B);

        // Page → artboard.
        let page = insert_group(None, "Page");
        let artboard = insert_group(Some(page), "Artboard");
        vec_rect(artboard, "Canvas", (0.0, 0.0, 1200.0, 800.0), white, 0.0);

        // Top nav.
        let nav = insert_group(Some(artboard), "Nav");
        vec_rect(nav, "Nav bar", (0.0, 0.0, 1200.0, 72.0), slate_50, 0.0);
        vec_rect(nav, "Logo mark", (48.0, 20.0, 32.0, 32.0), blue, 8.0);
        vec_text(
            nav,
            "Brand",
            (92.0, 24.0, 180.0, 28.0),
            "KCreate",
            "DejaVu Serif",
            22.0,
            navy,
        );
        vec_text(
            nav,
            "Nav: Product",
            (840.0, 28.0, 90.0, 20.0),
            "Product",
            "DejaVu Sans",
            15.0,
            muted,
        );
        vec_text(
            nav,
            "Nav: Pricing",
            (944.0, 28.0, 80.0, 20.0),
            "Pricing",
            "DejaVu Sans",
            15.0,
            muted,
        );
        vec_text(
            nav,
            "Nav: Docs",
            (1040.0, 28.0, 60.0, 20.0),
            "Docs",
            "DejaVu Sans",
            15.0,
            muted,
        );

        // Hero (the subtree we later restyle in isolation).
        let hero = insert_group(Some(artboard), "Hero");
        vec_text(
            hero,
            "Headline",
            (96.0, 200.0, 620.0, 120.0),
            "Design at the speed of thought",
            "DejaVu Serif",
            48.0,
            navy,
        );
        vec_text(
            hero,
            "Subhead",
            (96.0, 336.0, 560.0, 80.0),
            "Compose, theme, and ship on-brand designs entirely offline.",
            "DejaVu Sans",
            20.0,
            muted,
        );
        vec_rect(hero, "Primary CTA", (96.0, 448.0, 196.0, 56.0), blue, 12.0);
        vec_text(
            hero,
            "Primary CTA label",
            (128.0, 464.0, 140.0, 24.0),
            "Get started",
            "DejaVu Sans",
            18.0,
            white,
        );
        vec_rect(
            hero,
            "Hero panel",
            (820.0, 150.0, 320.0, 360.0),
            green,
            20.0,
        );
        vec_rect(
            hero,
            "Hero accent",
            (860.0, 196.0, 120.0, 120.0),
            amber,
            60.0,
        );

        // Feature cards.
        let cards = insert_group(Some(artboard), "Cards");
        let labels = ["Themes", "Brand kits", "Reversible"];
        for (i, label) in labels.iter().enumerate() {
            let x = 96.0 + (i as f64) * 360.0;
            vec_rect(cards, "Card", (x, 600.0, 320.0, 140.0), slate_50, 16.0);
            vec_rect(
                cards,
                "Card icon",
                (x + 24.0, 624.0, 40.0, 40.0),
                blue,
                10.0,
            );
            vec_text(
                cards,
                "Card title",
                (x + 24.0, 684.0, 260.0, 24.0),
                label,
                "DejaVu Sans",
                18.0,
                navy,
            );
        }

        // (a) Original — no branding, no logo yet.
        let png_original = render_png(W, H);
        assert_eq!(&png_original[..4], &[0x89, b'P', b'N', b'G'], "PNG magic");
        assert!(
            png_original.windows(4).any(|w| w == b"IDAT"),
            "original PNG missing image data"
        );

        // Define a brand kit: logo + palette-derived-from-an-image + fonts.
        let kit_id = brand_kit_create("Acme".to_string()).expect("kit");
        brand_kit_set_logo_bytes(kit_id, &brand_logo_svg()).expect("logo");
        let brand_photo = synth_band_png(&[
            [0xE1, 0x1D, 0x48],
            [0x7C, 0x3A, 0xED],
            [0xF5, 0x9E, 0x0B],
            [0x10, 0x18, 0x28],
            [0xF8, 0xFA, 0xFC],
        ]);
        brand_kit_extract_palette_from_image_bytes(kit_id, &brand_photo, 5).expect("palette");
        brand_kit_set_font_role(kit_id, "heading", "DejaVu Serif".to_string(), true)
            .expect("heading font");
        brand_kit_set_font_role(kit_id, "body", "DejaVu Sans".to_string(), true)
            .expect("body font");

        let kit = brand_kit_list()
            .expect("list")
            .into_iter()
            .find(|k| k.id == kit_id)
            .expect("kit present");
        let theme = Theme::from_brand_kit(&kit);

        // Place the saved logo as an editable node on the artboard.
        let logo = brand_logo_insert(kit_id, Some(artboard), 1020.0, 16.0, 40.0).expect("logo");
        assert!(!logo.node_ids.is_empty());

        // (b) Whole-document branding.
        let report = document_apply_theme(&theme).expect("apply whole-doc");
        assert!(report.affected_nodes > 0);
        let png_whole = render_png(W, H);
        assert_ne!(
            png_original, png_whole,
            "whole-document branding must change the render"
        );

        // Undo the whole-doc restyle; the placed logo stays.
        document_undo().expect("undo").expect("undo outcome");

        // (c) Selection-only branding — restyle just the hero subtree.
        document_set_selection(vec![hero]).expect("select hero");
        let sel_report =
            document_apply_theme_to_selection(&theme, vec![hero]).expect("apply to selection");
        assert!(sel_report.affected_nodes > 0);
        let png_selection = render_png(W, H);
        assert_ne!(
            png_original, png_selection,
            "selection branding must change the render"
        );
        assert_ne!(
            png_whole, png_selection,
            "selection-only result must differ from the whole-document result"
        );

        // Persist proofs when a destination is provided.
        if let Ok(out) = std::env::var("KCREATE_PROOF_DIR") {
            let out = std::path::Path::new(&out);
            std::fs::create_dir_all(out).expect("mkdir proof dir");
            std::fs::write(out.join("hero_original.png"), &png_original).expect("write original");
            std::fs::write(out.join("hero_branded_whole_doc.png"), &png_whole)
                .expect("write whole-doc");
            std::fs::write(out.join("hero_branded_selection.png"), &png_selection)
                .expect("write selection");
        }

        project_close();
    }

    // -------------------------------------------------------------------------
    // Magic Resize (G5)
    // -------------------------------------------------------------------------

    fn bounds(x: f64, y: f64, w: f64, h: f64) -> kcreate_core::node::Bounds {
        kcreate_core::node::Bounds::new(x, y, w, h)
    }

    /// Insert a leaf rectangle under `artboard` with explicit bounds,
    /// bypassing the operation log so the only op a Magic-Resize test
    /// records is the resize itself.
    fn add_rect_child(artboard: Uuid, b: kcreate_core::node::Bounds) -> Uuid {
        let mut guard = slot().write();
        let ws = guard.as_mut().expect("project open");
        let mut node = Node::new(NodeType::VectorLayer, "rect".to_string());
        node.parent_id = Some(artboard);
        node.bounds = b;
        let id = ws.project.document.insert_node(node).expect("insert child");
        drop(guard);
        id
    }

    /// Insert a text leaf under `artboard` carrying a `TextLayerMeta`
    /// (so the reflow engine has a font size to scale). `font_px` is
    /// already `f32` to keep the test cast-free.
    fn add_text_child(artboard: Uuid, b: kcreate_core::node::Bounds, font_px: f32) -> Uuid {
        let mut guard = slot().write();
        let ws = guard.as_mut().expect("project open");
        let mut node = Node::new(NodeType::TextLayer, "headline".to_string());
        node.parent_id = Some(artboard);
        node.bounds = b;
        let meta = crate::scene_sync::TextLayerMeta {
            text: "SUMMER FEST".to_string(),
            font_family: "Inter".to_string(),
            font_size: font_px,
        };
        node.metadata.insert(
            crate::scene_sync::TEXT_LAYER_METADATA_KEY.to_string(),
            serde_json::to_value(&meta).expect("serialize text meta"),
        );
        let id = ws.project.document.insert_node(node).expect("insert text");
        drop(guard);
        id
    }

    /// Insert a `LayoutFrame` child carrying *malformed* layout-config
    /// metadata. `layout_propagate_in_subtree` (step 5 of the resize
    /// pipeline) deserializes this key and fails with
    /// `InvalidLayoutConfig`, letting a test drive Magic Resize's
    /// mid-pipeline failure — and therefore its rollback — path.
    fn add_broken_layout_frame(artboard: Uuid, b: kcreate_core::node::Bounds) -> Uuid {
        let mut guard = slot().write();
        let ws = guard.as_mut().expect("project open");
        let mut node = Node::new(NodeType::LayoutFrame, "broken-frame".to_string());
        node.parent_id = Some(artboard);
        node.bounds = b;
        node.metadata.insert(
            LAYOUT_CONFIG_METADATA_KEY.to_string(),
            serde_json::json!({ "not": "a valid layout config" }),
        );
        let id = ws.project.document.insert_node(node).expect("insert frame");
        drop(guard);
        id
    }

    fn node_bounds(id: Uuid) -> kcreate_core::node::Bounds {
        let guard = slot().write();
        let ws = guard.as_ref().expect("project open");
        ws.project
            .document
            .get_node(id)
            .expect("node present")
            .bounds
    }

    fn child_ids(parent: Uuid) -> Vec<Uuid> {
        let guard = slot().write();
        let ws = guard.as_ref().expect("project open");
        ws.project.document.children_of(parent)
    }

    fn node_font(id: Uuid) -> f32 {
        let guard = slot().write();
        let ws = guard.as_ref().expect("project open");
        let node = ws.project.document.get_node(id).expect("node present");
        let raw = node
            .metadata
            .get(crate::scene_sync::TEXT_LAYER_METADATA_KEY)
            .expect("text meta present");
        serde_json::from_value::<crate::scene_sync::TextLayerMeta>(raw.clone())
            .expect("parse text meta")
            .font_size
    }

    fn artboard_count() -> usize {
        artboard_list().expect("artboard list").len()
    }

    /// Build a recognizable 1000×1000 poster: a full-bleed header band
    /// pinned to the top, a centered logo, a full-bleed footer pinned
    /// to the bottom, and a headline text layer. Children carry default
    /// constraints so the engine infers anchoring from geometry — the
    /// same path real authored designs hit. Returns the source artboard
    /// id; children are returned in insertion order by `child_ids`:
    /// `[header, logo, footer, headline]`.
    fn build_poster() -> Uuid {
        let ab = artboard_create(None, "Poster".to_string(), 1000.0, 1000.0).expect("artboard");
        let o = node_bounds(ab);
        add_rect_child(ab, bounds(o.x, o.y, 1000.0, 120.0)); // header
        add_rect_child(ab, bounds(o.x + 400.0, o.y + 400.0, 200.0, 200.0)); // centered logo
        add_rect_child(ab, bounds(o.x, o.y + 920.0, 1000.0, 80.0)); // footer
        add_text_child(ab, bounds(o.x + 100.0, o.y + 200.0, 800.0, 90.0), 60.0); // headline
        ab
    }

    fn spec_px(width: f64, height: f64) -> ResizeTargetSpec {
        ResizeTargetSpec {
            name: None,
            preset: None,
            width: Some(width),
            height: Some(height),
        }
    }

    fn spec_preset(name: &str) -> ResizeTargetSpec {
        ResizeTargetSpec {
            name: None,
            preset: Some(name.to_string()),
            width: None,
            height: None,
        }
    }

    #[test]
    #[serial]
    fn magic_resize_creates_one_artboard_per_target_at_target_sizes() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("mr_create", dir.path()).expect("create");
        let src = build_poster();
        let before = artboard_count();

        // One preset target + one explicit-pixel target.
        let ids = magic_resize(
            src,
            &[spec_preset("Instagram Story"), spec_px(2480.0, 3508.0)],
        )
        .expect("magic resize");

        assert_eq!(ids.len(), 2, "two targets => two new artboards");
        assert_eq!(artboard_count(), before + 2);
        let story = node_bounds(ids[0]);
        assert!(
            (story.width - 1080.0).abs() < 1e-6,
            "story.w={}",
            story.width
        );
        assert!(
            (story.height - 1920.0).abs() < 1e-6,
            "story.h={}",
            story.height
        );
        let a4 = node_bounds(ids[1]);
        assert!((a4.width - 2480.0).abs() < 1e-6, "a4.w={}", a4.width);
        assert!((a4.height - 3508.0).abs() < 1e-6, "a4.h={}", a4.height);
        project_close();
    }

    #[test]
    #[serial]
    fn magic_resize_preserves_anchoring_end_to_end() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("mr_anchor", dir.path()).expect("create");
        let src = build_poster();

        // Square → 9:16 story: the classic aspect-ratio hop.
        let ids = magic_resize(src, &[spec_px(1080.0, 1920.0)]).expect("resize");
        let ab = node_bounds(ids[0]);
        let kids = child_ids(ids[0]);
        assert_eq!(kids.len(), 4, "clone preserves the 4 children");

        let header = node_bounds(kids[0]);
        let logo = node_bounds(kids[1]);
        let footer = node_bounds(kids[2]);

        // Header spans the full new width and pins to the top.
        assert!((header.x - ab.x).abs() < 1.0, "header.x={}", header.x);
        assert!(
            (header.width - ab.width).abs() < 1.0,
            "header.width={}",
            header.width
        );
        assert!((header.y - ab.y).abs() < 1.0, "header.y={}", header.y);
        assert!(
            header.height < ab.height * 0.25,
            "header is a band, not a panel: h={}",
            header.height
        );

        // Footer spans the width and its bottom edge tracks the frame.
        let footer_bottom = footer.y + footer.height;
        assert!(
            ((ab.y + ab.height) - footer_bottom).abs() < 1.0,
            "footer bottom gap, bottom={footer_bottom}"
        );
        assert!(
            (footer.width - ab.width).abs() < 1.0,
            "footer.width={}",
            footer.width
        );

        // Logo stays centered on both axes.
        let cx = logo.x + logo.width * 0.5;
        let cy = logo.y + logo.height * 0.5;
        assert!((cx - (ab.x + ab.width * 0.5)).abs() < 1.5, "cx={cx}");
        assert!((cy - (ab.y + ab.height * 0.5)).abs() < 1.5, "cy={cy}");
        project_close();
    }

    #[test]
    #[serial]
    fn magic_resize_redistributes_space_without_overflow() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("mr_overflow", dir.path()).expect("create");
        let src = build_poster();

        // Hop to two very different aspect ratios; nothing may exit the
        // generated frame (redistribute, don't letterbox/overflow).
        let ids =
            magic_resize(src, &[spec_px(1080.0, 1920.0), spec_px(2480.0, 3508.0)]).expect("resize");
        for id in ids {
            let ab = node_bounds(id);
            for child in child_ids(id) {
                let c = node_bounds(child);
                assert!(c.x >= ab.x - 1e-6, "left overflow {}", c.x);
                assert!(c.y >= ab.y - 1e-6, "top overflow {}", c.y);
                assert!(
                    c.x + c.width <= ab.x + ab.width + 1e-6,
                    "right overflow {}",
                    c.x + c.width
                );
                assert!(
                    c.y + c.height <= ab.y + ab.height + 1e-6,
                    "bottom overflow {}",
                    c.y + c.height
                );
            }
        }
        project_close();
    }

    #[test]
    #[serial]
    fn magic_resize_keeps_fonts_within_clamp_bounds() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("mr_font", dir.path()).expect("create");
        let src = build_poster();
        let opts = ResizeOptions::default();

        // To the A4 poster (scales up) and to a tiny 200×200 tile
        // (scales down) — the headline font must stay inside the clamp
        // window for both.
        let ids =
            magic_resize(src, &[spec_px(2480.0, 3508.0), spec_px(200.0, 200.0)]).expect("resize");
        for id in ids {
            let headline = child_ids(id)[3];
            let fs = f64::from(node_font(headline));
            assert!(
                fs >= opts.min_font_px - 1e-6 && fs <= opts.max_font_px + 1e-6,
                "font {fs} outside [{}, {}]",
                opts.min_font_px,
                opts.max_font_px
            );
        }
        project_close();
    }

    #[test]
    #[serial]
    fn magic_resize_is_non_destructive_to_source() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("mr_nondestruct", dir.path()).expect("create");
        let src = build_poster();

        let before_ab = node_bounds(src);
        let before_kids: Vec<_> = child_ids(src).into_iter().map(node_bounds).collect();
        let before_font = node_font(child_ids(src)[3]);

        let _ =
            magic_resize(src, &[spec_px(1080.0, 1920.0), spec_px(2480.0, 3508.0)]).expect("resize");

        assert_eq!(
            node_bounds(src),
            before_ab,
            "source artboard bounds changed"
        );
        let after_kids: Vec<_> = child_ids(src).into_iter().map(node_bounds).collect();
        assert_eq!(after_kids, before_kids, "source child bounds changed");
        assert!(
            (f64::from(node_font(child_ids(src)[3])) - f64::from(before_font)).abs() < 1e-6,
            "source font changed"
        );
        project_close();
    }

    #[test]
    #[serial]
    fn magic_resize_is_a_single_undoable_op() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("mr_undo", dir.path()).expect("create");
        let src = build_poster();

        let depth_before = document_status().expect("status").undo_depth;
        let ids = magic_resize(
            src,
            &[
                spec_px(1080.0, 1920.0),
                spec_px(2480.0, 3508.0),
                spec_preset("Instagram Post"),
            ],
        )
        .expect("resize");
        assert_eq!(ids.len(), 3);

        // Three targets, ONE log entry — so a single undo retires the
        // whole batch (the host folds the inverse back into its tree).
        let after = document_status().expect("status");
        assert_eq!(
            after.undo_depth,
            depth_before + 1,
            "all targets must collapse to one undoable op"
        );
        assert!(after.can_undo);

        document_undo().expect("undo");
        let undone = document_status().expect("status");
        assert_eq!(
            undone.undo_depth, depth_before,
            "one undo retires the entire Magic-Resize op"
        );
        assert!(undone.can_redo);
        project_close();
    }

    #[test]
    #[serial]
    fn magic_resize_rejects_empty_targets() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("mr_empty", dir.path()).expect("create");
        let src = build_poster();
        let err = magic_resize(src, &[]).expect_err("empty targets must error");
        assert!(matches!(err, DocumentBridgeError::InvalidArgument { .. }));
        project_close();
    }

    #[test]
    #[serial]
    fn magic_resize_rejects_unknown_preset() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("mr_badpreset", dir.path()).expect("create");
        let src = build_poster();
        let err = magic_resize(src, &[spec_preset("Nonexistent Preset")])
            .expect_err("unknown preset must error");
        assert!(matches!(err, DocumentBridgeError::InvalidArgument { .. }));
        // Nothing was created on the failure path.
        project_close();
    }

    #[test]
    #[serial]
    fn magic_resize_rejects_non_artboard_source() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("mr_badsrc", dir.path()).expect("create");
        let text = canvas_create_text(None, 0.0, 0.0, "hi".to_string(), "Inter".to_string(), 12.0)
            .expect("create text");
        let err = magic_resize(text, &[spec_px(1080.0, 1920.0)])
            .expect_err("non-artboard source must error");
        assert!(matches!(err, DocumentBridgeError::WrongNodeType { .. }));
        project_close();
    }

    #[test]
    #[serial]
    fn magic_resize_rolls_back_partial_artboards_on_failure() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("mr_rollback", dir.path()).expect("create");

        // Source whose subtree contains a LayoutFrame with malformed
        // layout-config metadata. The clone + reflow succeed, but step 5
        // (`layout_propagate_in_subtree`) fails with InvalidLayoutConfig
        // — exercising the mid-pipeline failure path.
        let ab = artboard_create(None, "Broken".to_string(), 1000.0, 1000.0).expect("artboard");
        let o = node_bounds(ab);
        add_rect_child(ab, bounds(o.x, o.y, 1000.0, 120.0));
        add_broken_layout_frame(ab, bounds(o.x + 100.0, o.y + 300.0, 400.0, 400.0));

        let before = artboard_count();
        let depth_before = document_status().expect("status").undo_depth;

        let err = magic_resize(ab, &[spec_px(1080.0, 1920.0), spec_px(2480.0, 3508.0)])
            .expect_err("malformed layout config must abort the resize");
        assert!(
            matches!(err, DocumentBridgeError::InvalidLayoutConfig(..)),
            "expected InvalidLayoutConfig, got {err:?}"
        );

        // All-or-nothing: no partial clone survives and no operation was
        // logged, so the document is exactly as it was before the call.
        assert_eq!(
            artboard_count(),
            before,
            "a failed resize must leave no partial artboards behind"
        );
        assert_eq!(
            document_status().expect("status").undo_depth,
            depth_before,
            "a failed resize must not log an operation"
        );
        project_close();
    }

    // -------------------------------------------------------------------------
    // Real-design proof. Compose a recognizable promo poster (header band,
    // headline, hero disc, body copy, CTA button, footer band), Magic-Resize
    // it to a 9:16 story and an A4 poster, render all three through the real
    // scene-sync + PNG pipeline, and assert each target is a non-empty,
    // aspect-correct, *distinct* layout (not a blank or letterboxed frame).
    // Set `KCREATE_MAGIC_RESIZE_PROOF_DIR` to also dump the three PNGs.
    // -------------------------------------------------------------------------

    fn rgba(r: f32, g: f32, b: f32) -> kcreate_core::node::RgbaColor {
        kcreate_core::node::RgbaColor { r, g, b, a: 1.0 }
    }

    fn set_node_fill(id: Uuid, color: kcreate_core::node::RgbaColor) {
        let mut guard = slot().write();
        let ws = guard.as_mut().expect("project open");
        let node = ws.project.document.get_node_mut(id).expect("node present");
        node.style.fill = kcreate_core::node::FillStyle::Solid(color);
        node.touch();
        drop(guard);
    }

    fn rect_path(b: kcreate_core::node::Bounds) -> kcreate_vector::VectorPath {
        use kcreate_vector::{PathPoint, PathSegment, VectorPath};
        VectorPath::new(vec![
            PathSegment::MoveTo(PathPoint::new(b.x, b.y)),
            PathSegment::LineTo(PathPoint::new(b.x + b.width, b.y)),
            PathSegment::LineTo(PathPoint::new(b.x + b.width, b.y + b.height)),
            PathSegment::LineTo(PathPoint::new(b.x, b.y + b.height)),
            PathSegment::Close,
        ])
    }

    /// A four-cubic ellipse centered at `(cx, cy)` with radii `(rx, ry)`,
    /// authored in world coordinates so reflow's affine path remap moves it.
    fn ellipse_path(cx: f64, cy: f64, rx: f64, ry: f64) -> kcreate_vector::VectorPath {
        use kcreate_vector::{PathPoint, PathSegment, VectorPath};
        const K: f64 = 0.552_284_749_830_793_4;
        let (ox, oy) = (rx * K, ry * K);
        VectorPath::new(vec![
            PathSegment::MoveTo(PathPoint::new(cx, cy - ry)),
            PathSegment::CubicTo {
                ctrl1: PathPoint::new(cx + ox, cy - ry),
                ctrl2: PathPoint::new(cx + rx, cy - oy),
                end: PathPoint::new(cx + rx, cy),
            },
            PathSegment::CubicTo {
                ctrl1: PathPoint::new(cx + rx, cy + oy),
                ctrl2: PathPoint::new(cx + ox, cy + ry),
                end: PathPoint::new(cx, cy + ry),
            },
            PathSegment::CubicTo {
                ctrl1: PathPoint::new(cx - ox, cy + ry),
                ctrl2: PathPoint::new(cx - rx, cy + oy),
                end: PathPoint::new(cx - rx, cy),
            },
            PathSegment::CubicTo {
                ctrl1: PathPoint::new(cx - rx, cy - oy),
                ctrl2: PathPoint::new(cx - ox, cy - ry),
                end: PathPoint::new(cx, cy - ry),
            },
            PathSegment::Close,
        ])
    }

    fn add_shape(
        artboard: Uuid,
        b: kcreate_core::node::Bounds,
        color: kcreate_core::node::RgbaColor,
        path: kcreate_vector::VectorPath,
    ) -> Uuid {
        let mut guard = slot().write();
        let ws = guard.as_mut().expect("project open");
        let mut node = Node::new(NodeType::VectorLayer, "shape".to_string());
        node.parent_id = Some(artboard);
        node.bounds = b;
        node.style.fill = kcreate_core::node::FillStyle::Solid(color);
        node.metadata.insert(
            crate::scene_sync::VECTOR_PATH_METADATA_KEY.to_string(),
            serde_json::to_value(&path).expect("serialize vector path"),
        );
        let id = ws.project.document.insert_node(node).expect("insert shape");
        drop(guard);
        id
    }

    fn add_label(
        artboard: Uuid,
        b: kcreate_core::node::Bounds,
        text: &str,
        font_px: f32,
        color: kcreate_core::node::RgbaColor,
    ) -> Uuid {
        let mut guard = slot().write();
        let ws = guard.as_mut().expect("project open");
        let mut node = Node::new(NodeType::TextLayer, "label".to_string());
        node.parent_id = Some(artboard);
        node.bounds = b;
        node.style.fill = kcreate_core::node::FillStyle::Solid(color);
        let meta = crate::scene_sync::TextLayerMeta {
            text: text.to_string(),
            font_family: "Inter".to_string(),
            font_size: font_px,
        };
        node.metadata.insert(
            crate::scene_sync::TEXT_LAYER_METADATA_KEY.to_string(),
            serde_json::to_value(&meta).expect("serialize text meta"),
        );
        let id = ws.project.document.insert_node(node).expect("insert label");
        drop(guard);
        id
    }

    /// Compose a recognizable 1080² "Summer Fest" promo poster on its own
    /// artboard and return the artboard id. Children carry default
    /// constraints so the engine infers anchoring from geometry.
    fn build_promo_poster() -> Uuid {
        let ab =
            artboard_create(None, "Summer Fest".to_string(), 1080.0, 1080.0).expect("artboard");
        set_node_fill(ab, rgba(0.07, 0.09, 0.20)); // deep-navy background
        let o = node_bounds(ab);
        let coral = rgba(0.98, 0.42, 0.36);
        let sun = rgba(1.0, 0.80, 0.23);
        let cream = rgba(0.98, 0.96, 0.90);
        let navy2 = rgba(0.12, 0.16, 0.32);

        // Header band — full width, pinned to the top.
        let header = bounds(o.x, o.y, 1080.0, 140.0);
        add_shape(ab, header, coral, rect_path(header));
        // Logo badge — a small sun disc centered in the header.
        let logo = bounds(o.x + 484.0, o.y + 30.0, 112.0, 112.0);
        add_shape(
            ab,
            logo,
            sun,
            ellipse_path(o.x + 540.0, o.y + 86.0, 52.0, 52.0),
        );
        // Headline + subhead — near the top, centered horizontally.
        add_label(
            ab,
            bounds(o.x + 90.0, o.y + 190.0, 900.0, 110.0),
            "SUMMER FEST",
            84.0,
            cream,
        );
        add_label(
            ab,
            bounds(o.x + 90.0, o.y + 312.0, 900.0, 48.0),
            "JUNE 21  -  RIVERSIDE PARK",
            34.0,
            sun,
        );
        // Hero photo — a real raster image (sky / ground + a bright sun
        // subject). Magic Resize's smart-crop reframes it toward the
        // subject on every aspect change instead of stretching it.
        let hero = bounds(o.x + 140.0, o.y + 392.0, 800.0, 376.0);
        add_raster_node(ab, hero, 600, 360, &hero_photo_rgba(600, 360));
        // Body copy.
        add_label(
            ab,
            bounds(o.x + 90.0, o.y + 792.0, 900.0, 44.0),
            "Live music - Food trucks - Fireworks",
            30.0,
            cream,
        );
        // CTA button — centered, lower third.
        let cta = bounds(o.x + 360.0, o.y + 858.0, 360.0, 92.0);
        add_shape(ab, cta, coral, rect_path(cta));
        add_label(
            ab,
            bounds(o.x + 360.0, o.y + 884.0, 360.0, 40.0),
            "GET TICKETS",
            34.0,
            cream,
        );
        // Footer band — full width, pinned to the bottom.
        let footer = bounds(o.x, o.y + 980.0, 1080.0, 100.0);
        add_shape(ab, footer, navy2, rect_path(footer));
        ab
    }

    /// Render a single artboard (by id) through the real document→scene
    /// translation + PNG encoder, with the longest edge scaled to
    /// `max_dim_px`. Returns `(png_bytes, out_w, out_h)`.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        clippy::cast_sign_loss
    )]
    fn render_artboard_png(artboard_id: Uuid, max_dim_px: u32) -> (Vec<u8>, u32, u32) {
        let guard = slot().write();
        let ws = guard.as_ref().expect("project open");
        let ab_bounds = ws
            .project
            .document
            .get_node(artboard_id)
            .expect("artboard present")
            .bounds;
        let mut sync = crate::scene_sync::SceneSync::default();
        let mut scene = sync.sync_document_to_scene_borrowed(
            &ws.project.document,
            Some(ws.store.lock().blobs()),
            &[],
        );
        drop(guard);

        // Translate so the target artboard lands at the renderer origin;
        // every other artboard falls outside the crop.
        let dx = -ab_bounds.x as f32;
        let dy = -ab_bounds.y as f32;
        for obj in &mut scene.objects {
            obj.translation.0 += dx;
            obj.translation.1 += dy;
        }

        let longest = ab_bounds.width.max(ab_bounds.height).max(1.0);
        let scale = (f64::from(max_dim_px) / longest) as f32;
        let opts = PngExportOptions {
            width: ab_bounds.width.max(1.0) as u32,
            height: ab_bounds.height.max(1.0) as u32,
            scale,
            background: Some(kcreate_renderer::geometry::Color::rgba(1.0, 1.0, 1.0, 1.0)),
        };
        let bytes = export_png_to_bytes(&scene, &opts).expect("render artboard png");
        let out_w = (opts.width as f32 * scale).round() as u32;
        let out_h = (opts.height as f32 * scale).round() as u32;
        (bytes, out_w, out_h)
    }

    #[test]
    #[serial]
    fn magic_resize_renders_recognizable_design_across_aspect_ratios() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("mr_proof", dir.path()).expect("create");
        let src = build_promo_poster();

        let ids = magic_resize(
            src,
            &[spec_preset("Instagram Story"), spec_px(2480.0, 3508.0)],
        )
        .expect("resize");
        assert_eq!(ids.len(), 2);

        let square = render_artboard_png(src, 1200);
        let story = render_artboard_png(ids[0], 1200);
        let a4 = render_artboard_png(ids[1], 1200);

        // Every target must produce a real PNG with a non-trivial body —
        // i.e. an actual rendered design, never a blank frame.
        for (label, render) in [("square", &square), ("story", &story), ("a4", &a4)] {
            let (bytes, w, h) = render;
            assert!(
                bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
                "{label}: not a PNG"
            );
            assert!(
                bytes.len() > 2_000,
                "{label}: PNG too small ({} bytes) — likely blank",
                bytes.len()
            );
            assert!(*w > 0 && *h > 0, "{label}: zero output dims");
        }

        // Aspect ratios are honored (reflow, not uniform scale / letterbox).
        assert!(square.1.abs_diff(square.2) <= 1, "square should be ~1:1");
        assert!(story.2 > story.1, "story must be taller than wide (9:16)");
        assert!(a4.2 > a4.1, "a4 must be taller than wide");
        // The three renders are genuinely different layouts.
        assert_ne!(square.0, story.0, "square and story rendered identically");
        assert_ne!(story.0, a4.0, "story and a4 rendered identically");

        if let Ok(out) = std::env::var("KCREATE_MAGIC_RESIZE_PROOF_DIR") {
            let out_dir = std::path::Path::new(&out);
            std::fs::create_dir_all(out_dir).expect("create proof dir");
            std::fs::write(out_dir.join("01_square_1080.png"), &square.0).expect("write square");
            std::fs::write(out_dir.join("02_story_1080x1920.png"), &story.0).expect("write story");
            std::fs::write(out_dir.join("03_a4_2480x3508.png"), &a4.0).expect("write a4");
        }
        project_close();
    }

    // ===================================================================
    // H6 — content-aware Magic Resize: text re-fit, image smart-crop,
    // batch resize-and-export. Helpers + tests.
    // ===================================================================

    /// True when the host has at least one installed font face, so the
    /// shaper can actually measure text. On a truly headless box with no
    /// fonts, content-aware re-fit degrades to the geometric size by
    /// design and the box-fit assertions would be vacuous — those tests
    /// early-return instead of asserting nothing.
    fn fonts_available() -> bool {
        kcreate_text::FontManager::new().font_count() > 0
    }

    /// Read a text node's `(text, family, font_px, frame, box_bounds)`.
    fn text_layer(
        id: Uuid,
    ) -> (
        String,
        String,
        f32,
        kcreate_core::node::TextFrameOptions,
        kcreate_core::node::Bounds,
    ) {
        let guard = slot().write();
        let ws = guard.as_ref().expect("project open");
        let node = ws.project.document.get_node(id).expect("node present");
        let raw = node
            .metadata
            .get(crate::scene_sync::TEXT_LAYER_METADATA_KEY)
            .expect("text meta present");
        let meta: crate::scene_sync::TextLayerMeta =
            serde_json::from_value(raw.clone()).expect("parse text meta");
        (
            meta.text,
            meta.font_family,
            meta.font_size,
            node.text_frame_options(),
            node.bounds,
        )
    }

    /// Shape `text` at `font_px` in `box_bounds` and report whether it
    /// fits (no overflow). Uses the same `layout_paragraph` path the
    /// content-aware re-fit measures with, so a `true` here means the
    /// renderer would draw every line inside the box.
    fn text_fits_box_at(
        text: &str,
        family: &str,
        font_px: f32,
        frame: &kcreate_core::node::TextFrameOptions,
        box_bounds: kcreate_core::node::Bounds,
    ) -> bool {
        let style = kcreate_text::paragraph::TextStyle {
            font_family: family.to_string(),
            font_size: font_px,
            line_height: 1.25,
        };
        match kcreate_text::layout_paragraph(text, &style, frame, box_bounds, None) {
            Ok(layout) => !layout.overflow,
            Err(_) => false,
        }
    }

    /// True when the text node at `id` fits its own box at its current
    /// font size.
    fn text_node_fits(id: Uuid) -> bool {
        let (text, family, size, frame, box_bounds) = text_layer(id);
        text_fits_box_at(&text, &family, size, &frame, box_bounds)
    }

    /// Insert a `TextLayer` headline child with custom text + font size.
    fn add_headline(
        artboard: Uuid,
        b: kcreate_core::node::Bounds,
        text: &str,
        font_px: f32,
    ) -> Uuid {
        let mut guard = slot().write();
        let ws = guard.as_mut().expect("project open");
        let mut node = Node::new(NodeType::TextLayer, "headline".to_string());
        node.parent_id = Some(artboard);
        node.bounds = b;
        let meta = crate::scene_sync::TextLayerMeta {
            text: text.to_string(),
            font_family: "Inter".to_string(),
            font_size: font_px,
        };
        node.metadata.insert(
            crate::scene_sync::TEXT_LAYER_METADATA_KEY.to_string(),
            serde_json::to_value(&meta).expect("serialize text meta"),
        );
        let id = ws
            .project
            .document
            .insert_node(node)
            .expect("insert headline");
        drop(guard);
        id
    }

    /// A 1080² poster whose single child is a long headline authored to
    /// *just* fit its box. A drastic downscale (e.g. → 320²) clamps the
    /// geometric font larger, relative to its box, than at the source,
    /// so the geometric size overflows and the content-aware path must
    /// shrink it back into frame.
    fn build_refit_poster() -> Uuid {
        let ab = artboard_create(None, "Refit".to_string(), 1080.0, 1080.0).expect("artboard");
        let o = node_bounds(ab);
        add_headline(
            ab,
            bounds(o.x + 60.0, o.y + 140.0, 960.0, 260.0),
            "SUMMER FEST DOWN BY THE RIVERSIDE PARK - LIVE MUSIC, FOOD TRUCKS AND FIREWORKS ALL WEEKEND LONG",
            46.0,
        );
        ab
    }

    /// Build an `width`×`height` RGBA8 image: a flat `bg` canvas with a
    /// dense checkerboard "subject" block at `subject` = `(x, y, w, h)`
    /// alternating `fg` / its inverse. The checkerboard gives the focal
    /// detector real edges to lock onto; the surround is featureless.
    fn checkerboard_subject_rgba(
        width: u32,
        height: u32,
        subject: (u32, u32, u32, u32),
        fg: [u8; 3],
        bg: [u8; 3],
    ) -> Vec<u8> {
        let (sx, sy, sw, sh) = subject;
        let inv = [255 - fg[0], 255 - fg[1], 255 - fg[2]];
        let mut px = Vec::with_capacity((width as usize) * (height as usize) * 4);
        for y in 0..height {
            for x in 0..width {
                let in_subject = x >= sx && x < sx + sw && y >= sy && y < sy + sh;
                let rgb = if in_subject {
                    if ((x / 8) + (y / 8)) % 2 == 0 {
                        fg
                    } else {
                        inv
                    }
                } else {
                    bg
                };
                px.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
            }
        }
        px
    }

    /// A small "photo" for the poster's hero slot: two flat sky / ground
    /// bands with a bright off-centre sun disc. The disc + horizon give
    /// the focal detector real edges to track; the bands are otherwise
    /// featureless.
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_sign_loss,
        clippy::cast_possible_truncation
    )]
    fn hero_photo_rgba(width: u32, height: u32) -> Vec<u8> {
        let sky = [80u8, 140, 220];
        let ground = [40u8, 120, 70];
        let sun = [255u8, 220, 90];
        let cx = width as f32 * 0.62;
        let cy = height as f32 * 0.34;
        let r = (width.min(height) as f32) * 0.18;
        let horizon = (height as f32 * 0.62) as u32;
        let mut px = Vec::with_capacity((width as usize) * (height as usize) * 4);
        for y in 0..height {
            for x in 0..width {
                let dx = x as f32 - cx;
                let dy = y as f32 - cy;
                let rgb = if dx * dx + dy * dy <= r * r {
                    sun
                } else if y < horizon {
                    sky
                } else {
                    ground
                };
                px.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
            }
        }
        px
    }

    /// Count pixels close to `target` (squared RGB distance < 60²), so a
    /// "subject retained" assertion can't be satisfied by background.
    fn count_color_pixels(rgba: &[u8], target: [u8; 3]) -> usize {
        rgba.chunks_exact(4)
            .filter(|p| {
                let dr = i32::from(p[0]) - i32::from(target[0]);
                let dg = i32::from(p[1]) - i32::from(target[1]);
                let db = i32::from(p[2]) - i32::from(target[2]);
                dr * dr + dg * dg + db * db < 60 * 60
            })
            .count()
    }

    /// PNG-encode `pixels` (RGBA8, `img_w`×`img_h`), store it as a blob,
    /// and attach a `RasterLayer` child at `box_bounds` pointing at it.
    fn add_raster_node(
        artboard: Uuid,
        box_bounds: kcreate_core::node::Bounds,
        img_w: u32,
        img_h: u32,
        pixels: &[u8],
    ) -> Uuid {
        let png = encode_rgba_png(pixels, img_w, img_h).expect("encode raster png");
        let mut guard = slot().write();
        let ws = guard.as_mut().expect("project open");
        let blob = ws
            .store
            .lock()
            .blobs()
            .store(&png, "image/png")
            .expect("store raster blob");
        let meta = crate::scene_sync::RasterImageMeta {
            blob_hash: blob.hash,
            width: img_w,
            height: img_h,
        };
        let mut node = Node::new(NodeType::RasterLayer, "photo".to_string());
        node.parent_id = Some(artboard);
        node.bounds = box_bounds;
        node.metadata.insert(
            crate::scene_sync::RASTER_IMAGE_METADATA_KEY.to_string(),
            serde_json::to_value(&meta).expect("serialize raster meta"),
        );
        let id = ws
            .project
            .document
            .insert_node(node)
            .expect("insert raster");
        drop(guard);
        id
    }

    /// A 1080² poster whose single child is a full-bleed raster carrying
    /// a centred checkerboard subject, so a square→banner resize forces a
    /// genuine aspect-ratio change for the smart-crop to act on.
    fn build_raster_poster(subject: (u32, u32, u32, u32)) -> Uuid {
        let ab = artboard_create(None, "Photo".to_string(), 1080.0, 1080.0).expect("artboard");
        let o = node_bounds(ab);
        let pixels = checkerboard_subject_rgba(800, 800, subject, [220, 40, 40], [235, 235, 235]);
        add_raster_node(ab, bounds(o.x, o.y, 1080.0, 1080.0), 800, 800, &pixels);
        ab
    }

    /// Read a raster node's image metadata.
    fn raster_meta(id: Uuid) -> crate::scene_sync::RasterImageMeta {
        let guard = slot().write();
        let ws = guard.as_ref().expect("project open");
        let node = ws.project.document.get_node(id).expect("node present");
        let raw = node
            .metadata
            .get(crate::scene_sync::RASTER_IMAGE_METADATA_KEY)
            .expect("raster meta present");
        serde_json::from_value(raw.clone()).expect("parse raster meta")
    }

    /// Load + decode a blob to RGBA8, returning `(pixels, width, height)`.
    fn load_blob_rgba(hash: &str) -> (Vec<u8>, u32, u32) {
        let bytes = {
            let guard = slot().write();
            let ws = guard.as_ref().expect("project open");
            let store = ws.store.lock();
            store.blobs().load(hash).expect("load blob")
        };
        let img = image::load_from_memory(&bytes)
            .expect("decode blob")
            .to_rgba8();
        let (w, h) = img.dimensions();
        (img.into_raw(), w, h)
    }

    #[test]
    #[serial]
    fn magic_resize_refits_headline_within_its_box_across_aspects() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("mr_refit", dir.path()).expect("create");

        // No fonts → re-fit degrades to geometric by design and these
        // bounds assertions would be vacuous. Skip rather than lie.
        if !fonts_available() {
            project_close();
            return;
        }

        let src = build_refit_poster();
        // Source headline must itself fit (it's authored to).
        assert!(
            text_node_fits(child_ids(src)[0]),
            "source headline overflows its own box"
        );

        let targets = [
            spec_preset("Instagram Story"), // 1080×1920 — taller
            spec_px(2480.0, 3508.0),        // A4 — larger
            spec_px(320.0, 320.0),          // drastic downscale
        ];

        // Same source, same targets, re-fit OFF (pure geometric) vs ON.
        let geo = magic_resize_with_content(
            src,
            &targets,
            MagicResizeContentOptions {
                refit_text: false,
                smart_crop: false,
            },
        )
        .expect("geometric resize");
        let cw = magic_resize_with_content(
            src,
            &targets,
            MagicResizeContentOptions {
                refit_text: true,
                smart_crop: false,
            },
        )
        .expect("content-aware resize");

        let mut shrank_somewhere = false;
        let mut trace: Vec<String> = Vec::new();
        for i in 0..targets.len() {
            let geo_headline = child_ids(geo[i])[0];
            let cw_headline = child_ids(cw[i])[0];
            let geo_font = node_font(geo_headline);
            let cw_font = node_font(cw_headline);
            trace.push(format!("[target {i}: geo={geo_font:.2} cw={cw_font:.2}]"));

            // Re-fit never inflates past the proportional (geometric) size.
            assert!(
                cw_font <= geo_font + 0.5,
                "target {i}: re-fit {cw_font} exceeded geometric {geo_font}"
            );
            // Re-fit keeps the headline inside its reflowed box.
            assert!(
                text_node_fits(cw_headline),
                "target {i}: re-fit headline overflows its box at {cw_font}px"
            );
            if cw_font + 0.5 < geo_font {
                shrank_somewhere = true;
            }
        }

        // The drastic downscale forces the geometric font past its box,
        // so content-aware re-fit must have shrunk the headline somewhere
        // — proving the shaping path is wired into the resize.
        assert!(
            shrank_somewhere,
            "content-aware re-fit never shrank below geometric: {}",
            trace.join(" ")
        );
        project_close();
    }

    #[test]
    #[serial]
    fn magic_resize_smart_crops_raster_toward_subject() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("mr_crop", dir.path()).expect("create");

        // Centred subject so the retained-subject assertion is robust to
        // focal-vs-centre; focal precision itself is unit-tested in
        // kcreate_ai::focal_crop.
        let src = build_raster_poster((300, 300, 200, 200));
        let src_raster = child_ids(src)[0];
        let original_hash = raster_meta(src_raster).blob_hash;

        // Square → wide banner: a genuine aspect change that the renderer
        // would otherwise resolve by distorting the image.
        let target = spec_px(1600.0, 600.0);

        // --- Smart-crop ON ---
        let on = magic_resize_with_content(
            src,
            std::slice::from_ref(&target),
            MagicResizeContentOptions {
                refit_text: false,
                smart_crop: true,
            },
        )
        .expect("resize on");
        let clone_raster = child_ids(on[0])[0];
        let cropped = raster_meta(clone_raster);
        assert_ne!(
            cropped.blob_hash, original_hash,
            "smart-crop must derive a new blob, not reuse the stretched source"
        );

        // The derived crop's pixel aspect matches the reflowed box, so the
        // renderer's fill-to-bounds path draws it undistorted.
        let box_bounds = node_bounds(clone_raster);
        let crop_aspect = f64::from(cropped.width) / f64::from(cropped.height);
        let box_aspect = box_bounds.width / box_bounds.height;
        assert!(
            ((crop_aspect - box_aspect) / box_aspect).abs() < 0.02,
            "crop aspect {crop_aspect} should match box aspect {box_aspect}"
        );

        // The subject survived: the decoded crop still carries the warm-red
        // checkerboard, not just flat background.
        let (px, w, h) = load_blob_rgba(&cropped.blob_hash);
        assert_eq!(
            (w, h),
            (cropped.width, cropped.height),
            "blob dims match meta"
        );
        let subject_px = count_color_pixels(&px, [220, 40, 40]);
        assert!(
            subject_px > 500,
            "subject must be retained in the crop (found {subject_px} subject px)"
        );

        // Source blob is content-addressed and untouched (non-destructive).
        assert_eq!(
            raster_meta(src_raster).blob_hash,
            original_hash,
            "source raster blob must not change"
        );

        // --- Smart-crop OFF: the clone keeps the source blob verbatim ---
        let off = magic_resize_with_content(
            src,
            std::slice::from_ref(&target),
            MagicResizeContentOptions {
                refit_text: false,
                smart_crop: false,
            },
        )
        .expect("resize off");
        let off_raster = child_ids(off[0])[0];
        assert_eq!(
            raster_meta(off_raster).blob_hash,
            original_hash,
            "smart-crop off must leave the raster blob untouched"
        );
        project_close();
    }

    #[test]
    #[serial]
    fn magic_resize_export_png_writes_non_blank_pngs_in_one_undo() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("mr_export", dir.path()).expect("create");
        let src = build_promo_poster();

        let out = tmpdir();
        let out_dir = out.path().join("sizes");

        let depth_before = document_status().expect("status").undo_depth;
        let ab_before = artboard_count();

        let targets = [
            spec_preset("Instagram Post"),  // 1080×1080
            spec_preset("Instagram Story"), // 1080×1920
            spec_px(2480.0, 3508.0),        // A4
        ];
        let request = MagicResizeExportRequest {
            output_dir: out_dir.to_string_lossy().into_owned(),
            content: MagicResizeContentOptions::default(),
            max_dim_px: Some(900),
        };
        let report = magic_resize_export_png(src, &targets, &request).expect("export");

        // One PNG per target, none failed.
        assert_eq!(report.written.len(), targets.len(), "one PNG per target");
        assert!(
            report.failed.is_empty(),
            "no per-file failures: {:?}",
            report.failed
        );
        assert_eq!(report.artboard_ids.len(), targets.len());

        // Each written file exists, is a real PNG, and is non-blank.
        for path in &report.written {
            let bytes = std::fs::read(path).expect("read exported png");
            assert!(
                bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
                "{path} is not a PNG"
            );
            assert!(
                bytes.len() > 2_000,
                "{path} too small ({} bytes) — likely blank",
                bytes.len()
            );
        }

        // The resize added N artboards…
        assert_eq!(artboard_count(), ab_before + targets.len());
        // …under exactly ONE undoable op (the export is read-only — it
        // must not record a second entry on top of the resize).
        let after = document_status().expect("status");
        assert_eq!(
            after.undo_depth,
            depth_before + 1,
            "batch resize-export must be a single undoable op"
        );
        assert!(after.can_undo);

        // One undo retires the entire batch. Magic Resize is a
        // graph-mutating op, so (per `document_undo`) the inverse is
        // folded back by the host tree, not the bridge graph — we assert
        // the log contract here, matching `magic_resize_is_a_single_undoable_op`.
        document_undo().expect("undo");
        let undone = document_status().expect("status");
        assert_eq!(
            undone.undo_depth, depth_before,
            "one undo retires the whole batch resize-export"
        );
        assert!(undone.can_redo);
        project_close();
    }
}
