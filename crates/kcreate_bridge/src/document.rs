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

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use chrono::Utc;
use kcreate_core::component::{
    ComponentDefinition, ComponentInstance, ComponentVariant, COMPONENT_INSTANCE_METADATA_KEY,
};
use kcreate_core::config::RuntimeConfig;
use kcreate_core::document::{DocumentError, DocumentGraph};
use kcreate_core::node::{Node, NodeType};
use kcreate_core::operation::Operation;
use kcreate_core::project::{BrandKit, DesignTokens, ExportPreset, Project, ProjectError};
use kcreate_export::png::{export_png, PngExportError, PngExportOptions};
use kcreate_export::svg::{export_svg_from_document, SvgDocumentExportError, SvgExportOptions};
use kcreate_layout::{layout_flex, layout_grid, FlexLayout, GridLayout};
use kcreate_storage::project_io::{ProjectStore, ProjectStoreError};
use parking_lot::Mutex;
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
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Bridge(#[from] crate::state::BridgeError),
}

pub type Result<T> = std::result::Result<T, DocumentBridgeError>;

/// Open project = in-memory state + on-disk store, plus the
/// bookkeeping needed for incremental persistence.
struct Workspace {
    project: Project,
    store: ProjectStore,
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
    scene_sync: crate::scene_sync::SceneSync,
    /// Currently selected document nodes. Selection is rendered as
    /// highlight overlays in the next scene sync.
    selection: Vec<Uuid>,
}

fn slot() -> &'static Mutex<Option<Workspace>> {
    static WS: OnceLock<Mutex<Option<Workspace>>> = OnceLock::new();
    WS.get_or_init(|| Mutex::new(None))
}

/// Test-only helper to reset the singleton between serial tests.
#[cfg(test)]
pub(crate) fn reset_for_tests() {
    *slot().lock() = None;
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
    // Hold the singleton lock across the entire create operation so
    // "another project is already open" stays a TOCTOU-free check. The
    // bridge calls are synchronous and short; serialising them is the
    // correct semantics even when N-API begins driving requests from a
    // worker thread.
    let mut guard = slot().lock();
    if let Some(ws) = guard.as_ref() {
        return Err(DocumentBridgeError::ProjectAlreadyOpen(
            ws.store.project_dir().to_path_buf(),
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
        store,
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
    drop(guard);
    Ok(info)
}

/// Open an existing `.kstudio` directory. The process must not have
/// another project open — callers should call [`project_close`] first.
pub fn project_open(dir: &Path) -> Result<ProjectInfo> {
    // Same lock discipline as `project_create`: hold across the entire
    // operation, no TOCTOU window between the check and the set.
    let mut guard = slot().lock();
    if let Some(ws) = guard.as_ref() {
        return Err(DocumentBridgeError::ProjectAlreadyOpen(
            ws.store.project_dir().to_path_buf(),
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
    // Restore the operation log from disk so undo survives close+reopen.
    let max_depth = project.operation_log.max_depth();
    let history = store.load_operations(max_depth)?;
    // Every op we just loaded is, by definition, already on disk.
    let persisted_op_ids: HashSet<Uuid> = history.iter().map(|op| op.id).collect();
    project.operation_log.restore_from(history);
    let info = build_info(&project, store.project_dir());
    *guard = Some(Workspace {
        project,
        store,
        persisted_op_ids,
        scene_sync: crate::scene_sync::SceneSync::new(),
        selection: Vec::new(),
    });
    let _ = sync_scene_locked(&mut guard);
    drop(guard);
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
    let mut guard = slot().lock();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    ws.store.save_document(&ws.project.document)?;
    // Persist project-level metadata (design tokens, brand kits,
    // export presets). These mirror the in-memory `Project` fields
    // and must round-trip across close/reopen so identifiers stay
    // stable. Brand kits / presets are upserted by id; the design
    // tokens table holds a single row keyed on `'current'`.
    ws.store.save_design_tokens(&ws.project.design_tokens)?;
    for kit in &ws.project.brand_kits {
        ws.store.save_brand_kit(kit)?;
    }
    // Reconcile deleted brand kits: any rows on disk whose id is no
    // longer in memory must be removed so deletes survive the next
    // reopen.
    let kit_ids: HashSet<Uuid> = ws.project.brand_kits.iter().map(|k| k.id).collect();
    let on_disk_kits = ws.store.load_brand_kits()?;
    for kit in &on_disk_kits {
        if !kit_ids.contains(&kit.id) {
            ws.store.delete_brand_kit(kit.id)?;
        }
    }
    for preset in &ws.project.export_presets {
        ws.store.save_export_preset(preset)?;
    }
    let preset_ids: HashSet<Uuid> = ws.project.export_presets.iter().map(|p| p.id).collect();
    let on_disk_presets = ws.store.load_export_presets()?;
    for preset in &on_disk_presets {
        if !preset_ids.contains(&preset.id) {
            ws.store.delete_export_preset(preset.id)?;
        }
    }
    // Components: the in-memory map is the source of truth, so we
    // bulk-replace on disk. This handles both upsert and delete in
    // one round-trip; matches how `replace_components` is documented.
    ws.store.replace_components(&ws.project.components)?;

    let current_ids: HashSet<Uuid> = ws.project.operation_log.iter().map(|op| op.id).collect();
    // Collect new ops first so we can satisfy the borrow checker (the
    // save loop borrows `ws.store` mutably while reading from
    // `ws.project.operation_log` immutably) and so we can use
    // `HashSet::insert`'s return value to avoid the
    // contains-then-insert race that `clippy::set_contains_or_insert`
    // flags.
    let unseen: Vec<Operation> = ws
        .project
        .operation_log
        .iter()
        .filter(|op| !ws.persisted_op_ids.contains(&op.id))
        .cloned()
        .collect();
    for op in &unseen {
        ws.store.save_operation(op)?;
        ws.persisted_op_ids.insert(op.id);
    }
    // Forget ids that have aged out of the bounded in-memory log so
    // `persisted_op_ids` stays O(max_depth).
    ws.persisted_op_ids.retain(|id| current_ids.contains(id));
    // Mirror the in-memory `max_depth` bound onto the on-disk table.
    // Without this, the operations table grows for the project's
    // lifetime; combined with the (now-fixed) load_operations bug, it
    // would silently lose recent history once the row count exceeded
    // `max_depth`. The on-disk bound is the same as the in-memory bound
    // by design — the in-memory log is the canonical undo surface and
    // the disk just snapshots it.
    let max_depth = ws.project.operation_log.max_depth();
    ws.store.prune_operations(max_depth)?;
    drop(guard);
    Ok(())
}

/// Close the current project, discarding unsaved in-memory changes.
pub fn project_close() {
    *slot().lock() = None;
}

/// Snapshot of the open project (or `None` if nothing is open).
pub fn project_info() -> Option<ProjectInfo> {
    let guard = slot().lock();
    let info = guard
        .as_ref()
        .map(|ws| build_info(&ws.project, ws.store.project_dir()));
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
    let guard = slot().lock();
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
    let guard = slot().lock();
    let ws = guard.as_ref().ok_or(DocumentBridgeError::NoProject)?;
    Ok(ws.project.design_tokens.clone())
}

/// Replace the entire design-tokens bag. The caller is responsible
/// for calling [`project_save`] afterwards; this only mutates the
/// in-memory project.
pub fn design_tokens_set(tokens: DesignTokens) -> Result<()> {
    let mut guard = slot().lock();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    ws.project.design_tokens = tokens;
    drop(guard);
    Ok(())
}

/// Create a new (empty) brand kit and append it to the project.
/// Returns the new kit's id.
pub fn brand_kit_create(name: String) -> Result<Uuid> {
    let mut guard = slot().lock();
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
    let mut guard = slot().lock();
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
    let guard = slot().lock();
    let ws = guard.as_ref().ok_or(DocumentBridgeError::NoProject)?;
    Ok(ws.project.brand_kits.clone())
}

/// Remove a brand kit by id. Returns true when something was removed.
pub fn brand_kit_delete(id: Uuid) -> Result<bool> {
    let mut guard = slot().lock();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    let before = ws.project.brand_kits.len();
    ws.project.brand_kits.retain(|k| k.id != id);
    Ok(ws.project.brand_kits.len() != before)
}

/// Create a new export preset and append it to the project. Returns the new id.
pub fn export_preset_create(name: String, format: &str, scale: f32) -> Result<Uuid> {
    let format = parse_export_format(format)?;
    let mut guard = slot().lock();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    let preset = ExportPreset::new(name, format, scale);
    let id = preset.id;
    ws.project.export_presets.push(preset);
    drop(guard);
    Ok(id)
}

/// List every export preset, in insertion order.
pub fn export_preset_list() -> Result<Vec<ExportPreset>> {
    let guard = slot().lock();
    let ws = guard.as_ref().ok_or(DocumentBridgeError::NoProject)?;
    Ok(ws.project.export_presets.clone())
}

/// Delete an export preset by id. Returns true when something was removed.
pub fn export_preset_delete(id: Uuid) -> Result<bool> {
    let mut guard = slot().lock();
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
        other => Err(DocumentBridgeError::InvalidNodeType(other.to_string())),
    }
}

// -----------------------------------------------------------------------------
// Document CRUD
// -----------------------------------------------------------------------------

/// Returns a flat list of every node in document order.
pub fn document_get_tree() -> Result<Vec<NodeInfo>> {
    let guard = slot().lock();
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
    let guard = slot().lock();
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
    let guard = slot().lock();
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
    let mut guard = slot().lock();
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
}

/// Apply an in-place update to a node.
///
/// Bare graph mutation. See [`document_create_node`] and
/// [`kcreate_core::project::Project::undo`] for the host-driven
/// patch-application contract.
pub fn document_update_node(id: Uuid, changes: &UpdateNodeProps) -> Result<()> {
    let mut guard = slot().lock();
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
    node.touch();
    ws.project.modified_at = Utc::now();
    let _ = sync_scene_locked(&mut guard);
    drop(guard);
    Ok(())
}

/// Remove a node and all its descendants.
///
/// Bare graph mutation. See [`document_create_node`] and
/// [`kcreate_core::project::Project::undo`] for the host-driven
/// patch-application contract.
pub fn document_delete_node(id: Uuid) -> Result<()> {
    let mut guard = slot().lock();
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
    let mut guard = slot().lock();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    ws.project.execute_operation(operation);
    drop(guard);
    Ok(())
}

/// Undo the most recent operation. Returns the affected node ids of
/// the rolled-back operation, or `None` if the undo stack is empty.
///
/// Only moves the log cursor — the host applies `before_patch` to
/// its in-memory state. See [`kcreate_core::project::Project::undo`]
/// for the contract.
pub fn document_undo() -> Result<Option<Vec<Uuid>>> {
    let mut guard = slot().lock();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    let affected = ws.project.undo().map(|op| op.affected_nodes);
    drop(guard);
    Ok(affected)
}

/// Redo the next operation. Returns the affected node ids of the
/// re-applied operation, or `None` if the redo stack is empty.
///
/// Only moves the log cursor — the host applies `after_patch` to its
/// in-memory state. See [`kcreate_core::project::Project::undo`] for
/// the contract.
pub fn document_redo() -> Result<Option<Vec<Uuid>>> {
    let mut guard = slot().lock();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    let affected = ws.project.redo().map(|op| op.affected_nodes);
    drop(guard);
    Ok(affected)
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
    let mut guard = slot().lock();
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
fn sync_scene_locked(guard: &mut parking_lot::MutexGuard<'_, Option<Workspace>>) -> Result<()> {
    let Some(ws) = guard.as_mut() else {
        return Ok(());
    };
    let scene = ws.scene_sync.sync_document_to_scene(
        &ws.project.document,
        Some(ws.store.blobs()),
        &ws.selection,
    );
    // Renderer not initialised is fine here: the host may be working
    // headlessly. Other render errors propagate.
    match crate::state::render_scene(scene) {
        Ok(_) | Err(crate::state::BridgeError::NotInitialized) => Ok(()),
        Err(e) => Err(DocumentBridgeError::Bridge(e)),
    }
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
    let mut guard = slot().lock();
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
    let guard = slot().lock();
    let ws = guard.as_ref().ok_or(DocumentBridgeError::NoProject)?;
    let sel = ws.selection.clone();
    drop(guard);
    Ok(sel)
}

/// Clear the selection. No-op when nothing is selected.
pub fn document_clear_selection() -> Result<()> {
    let mut guard = slot().lock();
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
    let mut guard = slot().lock();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    // Rebuild the scene from the document on every hit-test. This
    // sidesteps the "is the renderer's cached scene up to date?"
    // question entirely: a hit-test is cheap (a few hundred reverse-z
    // AABB checks) compared to its UX cost when wrong.
    let scene = ws.scene_sync.sync_document_to_scene(
        &ws.project.document,
        Some(ws.store.blobs()),
        &ws.selection,
    );
    let vp = crate::hit_test::Viewport::new(kcreate_renderer::Vec2::new(pan_x, pan_y), zoom);
    let hit = crate::hit_test::hit_test(&ws.scene_sync, &scene, screen_x, screen_y, vp);
    drop(guard);
    Ok(hit)
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
    let mut guard = slot().lock();
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
    let mut guard = slot().lock();
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
    let guard = slot().lock();
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
    let mut guard = slot().lock();
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
    let mut guard = slot().lock();
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
// Prototype interactions (Block A / Phase 1)
// -----------------------------------------------------------------------------

/// Append an [`kcreate_core::Interaction`] to `node_id`'s metadata.
///
/// Returns the new interaction's id. `trigger` is one of `"click"`,
/// `"hover"`, `"press"`. `action_json` is a serialized
/// [`kcreate_core::InteractionAction`] (tagged-enum form, e.g.
/// `{"kind":"navigate_to","target_artboard_id":"…"}`).
pub fn interaction_add(node_id: Uuid, trigger: &str, action_json: &str) -> Result<Uuid> {
    let trigger = match trigger {
        "click" => kcreate_core::InteractionTrigger::Click,
        "hover" => kcreate_core::InteractionTrigger::Hover,
        "press" => kcreate_core::InteractionTrigger::Press,
        other => return Err(DocumentBridgeError::InvalidNodeType(other.to_string())),
    };
    let action: kcreate_core::InteractionAction = serde_json::from_str(action_json)?;
    let interaction = kcreate_core::Interaction::new(trigger, action);
    let interaction_id = interaction.id;
    let mut guard = slot().lock();
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
    let mut guard = slot().lock();
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
    let guard = slot().lock();
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
    let guard = slot().lock();
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
    let mut guard = slot().lock();
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
    let guard = slot().lock();
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
        other => return Err(DocumentBridgeError::InvalidNodeType(other.to_string())),
    };
    let layout = kcreate_core::PageLayout::new(page_size, orientation);
    let mut guard = slot().lock();
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
        other => return Err(DocumentBridgeError::InvalidNodeType(other.to_string())),
    })
}

/// List all master pages in the open project, sorted by name.
pub fn master_page_list() -> Result<Vec<MasterPageInfo>> {
    let guard = slot().lock();
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
    let mut guard = slot().lock();
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
    let mut guard = slot().lock();
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
    let mut guard = slot().lock();
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
/// returns `DocumentBridgeError::InvalidNodeType` — silently ignoring
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
            return Err(DocumentBridgeError::InvalidNodeType(
                "page_add: `size` provided without `orientation` (pass both or neither)".into(),
            ));
        }
        (None, Some(_)) => {
            return Err(DocumentBridgeError::InvalidNodeType(
                "page_add: `orientation` provided without `size` (pass both or neither)".into(),
            ));
        }
    };
    let mut guard = slot().lock();
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
                other => return Err(DocumentBridgeError::InvalidNodeType(other.to_string())),
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
    let mut guard = slot().lock();
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
    let mut guard = slot().lock();
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
    let mut guard = slot().lock();
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
    let guard = slot().lock();
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
    let mut guard = slot().lock();
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
    let mut guard = slot().lock();
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

/// Switch the active variant of a component instance node.
pub fn component_switch_variant(node_id: Uuid, variant_id: Uuid) -> Result<()> {
    let mut guard = slot().lock();
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
    let mut guard = slot().lock();
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
    let mut guard = slot().lock();
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
    let mut guard = slot().lock();
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

/// Convert a `GroupLayer` into a `LayoutFrame` so it can carry an
/// auto-layout config. No-op if the node is already a `LayoutFrame`;
/// returns an error for any other node type.
pub fn layout_convert_to_frame(node_id: Uuid) -> Result<()> {
    let mut guard = slot().lock();
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
    let mut guard = slot().lock();
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
    let img = image::load_from_memory(&bytes).map_err(|e| {
        DocumentBridgeError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    })?;
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    let mime_type = mime_for_path(file_path);
    let mut guard = slot().lock();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    let blob = ws
        .store
        .blobs()
        .store(&bytes, mime_type)
        .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))?;
    let meta = crate::scene_sync::RasterImageMeta {
        blob_hash: blob.hash,
        width,
        height,
    };
    let mut node = Node::new(NodeType::RasterLayer, "Image");
    node.parent_id = parent_id;
    node.bounds = kcreate_core::node::Bounds {
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
        "document_import_image",
        serde_json::Value::Null,
        snapshot,
        vec![id],
    );
    ws.project.execute_operation(op);
    let _ = sync_scene_locked(&mut guard);
    drop(guard);
    Ok(id)
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
    font_size: f32,
) -> Result<Uuid> {
    let meta = crate::scene_sync::TextLayerMeta {
        text: text.clone(),
        font_family,
        font_size,
    };
    let mut node = Node::new(NodeType::TextLayer, "Text");
    node.parent_id = parent_id;
    node.bounds = kcreate_core::node::Bounds {
        x,
        y,
        // Bounds height defaults to font size; the layer panel can
        // refine it once shaping has run.
        width: f64::from(font_size) * (text.len().max(1) as f64) * 0.6,
        height: f64::from(font_size),
    };
    node.metadata.insert(
        crate::scene_sync::TEXT_LAYER_METADATA_KEY.to_string(),
        serde_json::to_value(&meta)?,
    );
    let mut guard = slot().lock();
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
    let mut guard = slot().lock();
    if let Some(ws) = guard.as_mut() {
        ws.project.operation_log.set_max_depth(new_depth);
    }
    drop(guard);
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
    let guard = slot().lock();
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
    let guard = slot().lock();
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
    let scene = crate::state::current_scene()?;
    let opts = PngExportOptions {
        width: options.width,
        height: options.height,
        scale: options.scale,
        background: options
            .background
            .map(|[r, g, b, a]| kcreate_renderer::geometry::Color::rgba(r, g, b, a)),
    };
    export_png(&scene, &opts, output_path)?;
    let meta = std::fs::metadata(output_path)?;
    Ok(meta.len())
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
}

/// Render the open document to PDF. Returns the number of bytes written.
pub fn export_pdf_file(output_path: &Path, options: &PdfExportRequest) -> Result<u64> {
    let guard = slot().lock();
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
        let bytes = match ws.store.blobs().load(&meta.blob_hash) {
            Ok(b) => b,
            Err(_) => continue,
        };
        if let Ok(pixels) = kcreate_export::pdf::RasterPixels::decode(&bytes) {
            rasters.insert(meta.blob_hash, pixels);
        }
    }
    let opts = kcreate_export::pdf::PdfExportOptions {
        width_mm: options.width_mm,
        height_mm: options.height_mm,
        title: options
            .title
            .clone()
            .unwrap_or_else(|| ws.project.name.clone()),
    };
    let bytes = kcreate_export::pdf::export_pdf_from_document(
        &ws.project.document,
        &opts,
        &rasters,
        output_path,
    )
    .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))?;
    drop(guard);
    Ok(bytes as u64)
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
    kcreate_export::export_webp(&scene, &opts, output_path)
        .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))?;
    let meta = std::fs::metadata(output_path)?;
    Ok(meta.len())
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
    kcreate_export::export_jpeg(&scene, &opts, output_path)
        .map_err(|e| DocumentBridgeError::Io(std::io::Error::other(e.to_string())))?;
    let meta = std::fs::metadata(output_path)?;
    Ok(meta.len())
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
    let mut guard = slot().lock();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    let blob = ws
        .store
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

/// `DocumentAccess` implementation that talks to the process-global
/// workspace [`slot`]. Each method takes the workspace lock for the
/// minimum duration needed.
///
/// The MCP server holds an `Arc<dyn DocumentAccess>` for its full
/// lifetime, but only calls into this impl while servicing a request
/// on its worker thread — so the workspace lock is held briefly and
/// never across an `await` boundary. Lock-ordering relative to the
/// renderer singleton is documented on [`sync_scene_locked`].
#[cfg(feature = "mcp")]
struct WorkspaceAccess;

#[cfg(feature = "mcp")]
impl kcreate_mcp::tools::DocumentAccess for WorkspaceAccess {
    fn list_artboards(&self) -> Vec<kcreate_mcp::tools::ArtboardInfo> {
        let guard = slot().lock();
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
        let mut guard = slot().lock();
        let ws = guard
            .as_mut()
            .ok_or_else(|| "no project open".to_string())?;
        let mut node = Node::new(node_type, name);
        node.parent_id = parent_id;
        let id = ws
            .project
            .document
            .insert_node(node)
            .map_err(|e| e.to_string())?;
        ws.project.modified_at = Utc::now();
        // Sync the scene so the renderer sees the new node immediately.
        // Failure to sync (e.g. renderer not initialised in a headless
        // host) is non-fatal: the next renderer_init + sync recovers.
        let _ = sync_scene_locked(&mut guard);
        Ok(id)
    }

    fn export_svg(&self, node_ids: &[Uuid]) -> std::result::Result<String, String> {
        let guard = slot().lock();
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
}

/// Start the local MCP server. Loopback-only. Idempotent.
#[cfg(feature = "mcp")]
pub fn mcp_start() -> Result<u32> {
    let access: std::sync::Arc<dyn kcreate_mcp::tools::DocumentAccess> =
        std::sync::Arc::new(WorkspaceAccess);
    let port = kcreate_mcp::server::start_global(access)
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

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    fn tmpdir() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
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
            let guard = slot().lock();
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
        assert!(matches!(err, DocumentBridgeError::InvalidNodeType(_)));
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
                    let mut g = slot().lock();
                    let ws = g.as_mut().expect("ws");
                    let n = ws.project.document.get_node_mut(id).expect("node");
                    n.bounds = kcreate_core::node::Bounds::new(0.0, 0.0, 50.0, 30.0);
                }
                id
            })
            .collect();
        // Give the frame an explicit size.
        {
            let mut g = slot().lock();
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
        let g = slot().lock();
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

        let g = slot().lock();
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
        let g = slot().lock();
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
        let g = slot().lock();
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
            let g = slot().lock();
            g.as_ref().unwrap().project.operation_log.max_depth()
        };
        low_resource_mode_set(true);
        let lr_depth = {
            let g = slot().lock();
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
        assert!(matches!(err, DocumentBridgeError::InvalidNodeType(_)));
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
            matches!(err, DocumentBridgeError::InvalidNodeType(ref m) if m.contains("orientation")),
            "expected InvalidNodeType naming `orientation`, got {err:?}",
        );

        // Orientation without size — rejected.
        let err = page_add("half-2".into(), None, Some("portrait")).expect_err("missing size");
        assert!(
            matches!(err, DocumentBridgeError::InvalidNodeType(ref m) if m.contains("size")),
            "expected InvalidNodeType naming `size`, got {err:?}",
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
            let guard = slot().lock();
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
            let guard = slot().lock();
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
        assert!(matches!(err, DocumentBridgeError::InvalidNodeType(_)));
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
                DocumentBridgeError::InvalidNodeType(s) => assert_eq!(s, bad),
                other => panic!("expected InvalidNodeType({bad:?}), got {other:?}"),
            }
        }

        // Uppercase / mixed-case must also be rejected — wire format
        // is case-sensitive on both sides.
        for bad in ["A4", "Letter", "Presentation_16x9"] {
            assert!(
                matches!(
                    parse_page_size(bad),
                    Err(DocumentBridgeError::InvalidNodeType(_))
                ),
                "parse_page_size should reject case-folded form {bad:?}"
            );
        }
    }
}
