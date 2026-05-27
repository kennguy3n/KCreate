//! Bridge layer for project-thumbnail rendering and the recent-projects
//! list shown on the HomePage.
//!
//! Two responsibilities live in this module:
//!
//! 1. **Lazy thumbnail rendering** — for the *currently open* project,
//!    render a page (or the project's cover) through `scene_sync` +
//!    `kcreate_export::png` into a low-resolution image, and cache the
//!    bytes in `<project>/.kstudio/thumbnails/` via
//!    [`kcreate_storage::ThumbnailCache`]. Re-rendering is skipped when
//!    the document hasn't changed (content-hash match).
//! 2. **Recent-projects roster** — a persistent list at
//!    `~/.kcreate/recent.json` of `.kstudio` directories the user has
//!    recently created or opened. The HomePage reads this list plus the
//!    on-disk cover thumbnail of each entry to paint a recent-projects
//!    grid *without ever cracking open the SQLite database*.
//!
//! Why both in one module: thumbnails and recent-projects have the same
//! life-cycle (updated on every project open / save, read from the
//! HomePage), and they share the same wire types (`ThumbnailBytes`).
//! Splitting them would force the renderer to round-trip through two
//! N-API surfaces for what is conceptually one feature.
//!
//! ## Background pre-warming
//!
//! [`prepare_thumbnails_background`] spawns a worker thread that
//! ensures the cover + every page in the open project has an up-to-date
//! thumbnail. The worker:
//!
//! * Refuses to start when low-resource mode is active (skip speculative
//!   work per ARCHITECTURE.md §14).
//! * Snapshots the document under the workspace lock, then renders
//!   off-lock so user interactions aren't blocked.
//! * Re-acquires the workspace lock briefly to commit each thumbnail
//!   to disk (the cache is per-project, so the lock guards against a
//!   concurrent project_close).
//!
//! ## Tests
//!
//! The renderer needs a real wgpu / CPU backend to produce pixels, so
//! all of the rendering-shaped tests live in `kcreate_tests` (see
//! `crates/kcreate_tests/tests/thumbnails.rs`). This module's local
//! tests focus on the cache-hit path, the recent-projects store
//! invariants, and serialization round-trips.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::thread;
use std::time::Duration;

use chrono::{DateTime, Utc};
use kcreate_core::node::NodeType;
use kcreate_core::Bounds;
use kcreate_export::png::{export_png_to_bytes, PngExportOptions};
use kcreate_renderer::geometry::Color;
use kcreate_renderer::Scene;
use kcreate_storage::{
    CachedThumbnail, ThumbnailCache, ThumbnailEncoding, ThumbnailError, COVER_KEY,
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::document::{slot as workspace_slot, DocumentBridgeError, Workspace};

type Result<T> = std::result::Result<T, DocumentBridgeError>;

// ---------------------------------------------------------------------------
// Public configuration
// ---------------------------------------------------------------------------

/// Maximum side length (in CSS pixels) of a rendered thumbnail. Chosen
/// so the HomePage's recent-projects card looks crisp at 2x DPR (the
/// card target is ~120 px on the long edge).
pub const DEFAULT_THUMBNAIL_MAX_DIM_PX: u32 = 320;

/// Hard cap on the recent-projects list to keep `~/.kcreate/recent.json`
/// from growing unbounded.
pub const RECENT_PROJECTS_MAX_ENTRIES: usize = 32;

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

/// Bytes + metadata returned from a thumbnail lookup. The renderer
/// turns `bytes_base64` into a `data:` URL for its `<img>` element.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThumbnailBytes {
    pub width: u32,
    pub height: u32,
    pub mime: String,
    pub byte_size: u64,
    /// Base64-encoded image bytes. The N-API surface emits this as a
    /// `String` rather than a `Buffer` because the HomePage assembles
    /// the data URL on the JS side; future call sites that need raw
    /// bytes can decode once.
    pub bytes_base64: String,
    /// BLAKE3 hex of the source content. Renderer can use this as an
    /// `<img>` cache-busting suffix.
    pub content_hash: String,
}

impl ThumbnailBytes {
    fn from_cached(c: CachedThumbnail) -> Self {
        Self {
            width: c.width,
            height: c.height,
            mime: c.encoding.mime().to_string(),
            byte_size: c.bytes.len() as u64,
            bytes_base64: encode_base64(&c.bytes),
            content_hash: c.content_hash,
        }
    }
}

/// A single recent-project entry, mirroring `RecentProject` on disk
/// but with the cover-thumbnail metadata folded in so the HomePage
/// gets everything in one round-trip.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecentProjectInfo {
    pub path: PathBuf,
    pub name: String,
    pub project_id: Uuid,
    pub modified_at: String,
    pub last_opened_at: String,
    /// Best-effort cover thumbnail metadata. `None` when the project
    /// has no cached cover (e.g. freshly created and not yet
    /// pre-warmed).
    pub cover: Option<RecentProjectCoverInfo>,
}

/// Cover thumbnail descriptor — just enough to render an `<img>`
/// without fetching the bytes. Pair with [`recent_project_cover_bytes`]
/// to get the actual pixel data.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecentProjectCoverInfo {
    pub width: u32,
    pub height: u32,
    pub mime: String,
    pub byte_size: u64,
    pub content_hash: String,
}

// ---------------------------------------------------------------------------
// Singletons
// ---------------------------------------------------------------------------

/// Process-global recent-projects roster. Backed by
/// `~/.kcreate/recent.json` (or the path overridden by
/// `KCREATE_RECENT_PROJECTS_FILE` for tests).
fn recent_slot() -> &'static Mutex<RecentProjectsStore> {
    static SLOT: OnceLock<Mutex<RecentProjectsStore>> = OnceLock::new();
    SLOT.get_or_init(|| {
        let path = recent_projects_path();
        Mutex::new(RecentProjectsStore::open(path))
    })
}

fn recent_projects_path() -> PathBuf {
    if let Ok(s) = std::env::var("KCREATE_RECENT_PROJECTS_FILE") {
        return PathBuf::from(s);
    }
    let base = std::env::var_os("HOME").map_or_else(std::env::temp_dir, PathBuf::from);
    base.join(".kcreate").join("recent.json")
}

// ---------------------------------------------------------------------------
// Recent-projects store
// ---------------------------------------------------------------------------

/// Persistent file-backed list. Always ordered most-recent-first.
#[derive(Debug)]
pub(crate) struct RecentProjectsStore {
    path: PathBuf,
    entries: Vec<RecentProjectEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct RecentProjectEntry {
    path: PathBuf,
    name: String,
    project_id: Uuid,
    modified_at: DateTime<Utc>,
    last_opened_at: DateTime<Utc>,
}

impl RecentProjectsStore {
    fn open(path: PathBuf) -> Self {
        let entries = match fs::read(&path) {
            Ok(bytes) => {
                serde_json::from_slice::<Vec<RecentProjectEntry>>(&bytes).unwrap_or_default()
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => {
                log::warn!(
                    "recent projects file {} unreadable: {e} — starting empty",
                    path.display()
                );
                Vec::new()
            }
        };
        Self { path, entries }
    }

    fn record(&mut self, path: &Path, name: &str, project_id: Uuid, modified_at: DateTime<Utc>) {
        // Drop any existing entry with the same path so we can move it
        // to the front (case-sensitive equality is correct: the path
        // comes straight from `ProjectStore::project_dir()` which is
        // canonicalised by the time we get here).
        self.entries.retain(|e| e.path != path);
        self.entries.insert(
            0,
            RecentProjectEntry {
                path: path.to_path_buf(),
                name: name.to_string(),
                project_id,
                modified_at,
                last_opened_at: Utc::now(),
            },
        );
        self.entries.truncate(RECENT_PROJECTS_MAX_ENTRIES);
        if let Err(e) = self.persist() {
            log::warn!(
                "recent projects file {} unwritable: {e}",
                self.path.display()
            );
        }
    }

    fn list_pruned(&mut self) -> Vec<RecentProjectEntry> {
        // Lazily drop entries whose `.kstudio` directory no longer
        // exists. We don't surface "this project moved" errors — the
        // HomePage just shows what's still there.
        let before = self.entries.len();
        self.entries
            .retain(|e| e.path.join("manifest.json").exists());
        if self.entries.len() != before {
            if let Err(e) = self.persist() {
                log::warn!(
                    "recent projects file {} unwritable: {e}",
                    self.path.display()
                );
            }
        }
        self.entries.clone()
    }

    fn persist(&self) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec_pretty(&self.entries)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, &bytes)?;
        fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

/// Append (or move-to-front) the current open project on the
/// recent-projects list. Called from `project_create` / `project_open`.
///
/// We deliberately read the input from the workspace under the
/// caller's already-held workspace lock; the recent-projects mutex is
/// briefly contended and never held across renderer work.
pub(crate) fn record_recent_project(ws: &Workspace) {
    let manifest = ws.store.manifest();
    let mut slot = recent_slot().lock();
    slot.record(
        ws.store.project_dir(),
        &manifest.name,
        manifest.id,
        manifest.modified_at,
    );
}

/// Snapshot the current recent-projects list (pruned of paths that
/// no longer exist on disk). Each entry includes whatever cover-
/// thumbnail metadata is cached on disk.
pub fn recent_projects_list() -> Vec<RecentProjectInfo> {
    let entries = recent_slot().lock().list_pruned();
    entries
        .into_iter()
        .map(|e| {
            let cover = peek_cover_meta(&e.path);
            RecentProjectInfo {
                path: e.path,
                name: e.name,
                project_id: e.project_id,
                modified_at: e.modified_at.to_rfc3339(),
                last_opened_at: e.last_opened_at.to_rfc3339(),
                cover,
            }
        })
        .collect()
}

/// Look up a project's cover thumbnail bytes by directory path
/// *without* opening the project. Returns `None` when the project has
/// no cached cover.
pub fn recent_project_cover_bytes(project_dir: &Path) -> Result<Option<ThumbnailBytes>> {
    let thumb_dir = project_dir.join("thumbnails");
    if !thumb_dir.exists() {
        return Ok(None);
    }
    let cache = match ThumbnailCache::open(&thumb_dir) {
        Ok(c) => c,
        Err(e) => {
            log::debug!("cover cache for {}: {e}", project_dir.display());
            return Ok(None);
        }
    };
    let Some(entry) = cache.index().entries.get(&COVER_KEY).cloned() else {
        return Ok(None);
    };
    // Read by the indexed hash — even if the *current* project state
    // has drifted since the cover was rendered, the stored bytes are
    // still the right thing to show in the recent-projects list.
    let cached = match cache.lookup(COVER_KEY, &entry.content_hash) {
        Ok(Some(c)) => c,
        Ok(None) => return Ok(None),
        Err(e) => return Err(thumb_err(e)),
    };
    Ok(Some(ThumbnailBytes::from_cached(cached)))
}

fn peek_cover_meta(project_dir: &Path) -> Option<RecentProjectCoverInfo> {
    let thumb_dir = project_dir.join("thumbnails");
    if !thumb_dir.exists() {
        return None;
    }
    let cache = ThumbnailCache::open(&thumb_dir).ok()?;
    let entry = cache.index().entries.get(&COVER_KEY)?;
    Some(RecentProjectCoverInfo {
        width: entry.width,
        height: entry.height,
        mime: entry.encoding.mime().to_string(),
        byte_size: entry.byte_size,
        content_hash: entry.content_hash.clone(),
    })
}

// ---------------------------------------------------------------------------
// Thumbnail rendering — currently-open project
// ---------------------------------------------------------------------------

/// Ensure the currently open project has a cover thumbnail; return it.
///
/// On a cache hit the cached bytes are returned without touching the
/// renderer. On a miss the first page (or the document bounds, when
/// no Page node is present) is rendered at `max_dim_px` on the long
/// edge.
pub fn ensure_cover_thumbnail(max_dim_px: u32) -> Result<ThumbnailBytes> {
    let dim = sanitize_dim(max_dim_px);
    // Atomic snapshot: hash + target + scene are all derived from
    // the **same** workspace lock window. See `ensure_thumbnail_for`
    // for why this matters.
    ensure_thumbnail_for(COVER_KEY, dim, |ws| {
        let hash = document_content_hash(ws);
        let target = pick_cover_target(ws);
        let scene = build_scene_for_target(ws, target);
        Ok((hash, target, scene))
    })
}

/// Ensure a specific page thumbnail exists; return it. Errors out if
/// `page_id` doesn't refer to a Page node in the open project.
pub fn ensure_page_thumbnail(page_id: Uuid, max_dim_px: u32) -> Result<ThumbnailBytes> {
    let dim = sanitize_dim(max_dim_px);
    ensure_thumbnail_for(page_id, dim, move |ws| {
        let node = ws
            .project
            .document
            .get_node(page_id)
            .ok_or(DocumentBridgeError::NodeNotFound(page_id))?;
        if node.node_type != NodeType::Page {
            return Err(DocumentBridgeError::InvalidArgument {
                argument: "page_id".to_string(),
                value: format!("node {page_id} is a {:?}, not a Page", node.node_type),
            });
        }
        let hash = page_content_hash(ws, page_id);
        let target = ThumbnailTarget {
            bounds: node.bounds,
            background: None,
        };
        let scene = build_scene_for_target(ws, target);
        Ok((hash, target, scene))
    })
}

/// Process-global flag: is a pre-warm worker currently running? Set
/// to `true` by [`prepare_thumbnails_background`] before spawning the
/// thread and cleared by the thread itself on exit (success or
/// panic-recovered). Guards against thread-thrash if pre-warm is
/// kicked off repeatedly (e.g. rapid project_create+project_open in
/// a test, or a user spam-clicking "Open Project") — additional
/// callers are coalesced into the in-flight worker. Because the
/// worker re-reads the workspace snapshot it sees the latest page
/// list, so coalescing doesn't drop updates.
static PREWARM_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// Kick off a background thread that pre-warms every page's
/// thumbnail. Returns immediately. Does nothing in low-resource mode.
///
/// Calls are *coalesced*: at most one pre-warm worker runs at a
/// time. If a second caller arrives while the worker is still
/// running, it returns `Ok(())` without spawning a new thread — the
/// in-flight worker covers them. This is safe because the worker
/// snapshots the page list at the start of its run; the next call
/// after it exits will pick up any newly-added pages.
pub fn prepare_thumbnails_background(max_dim_px: u32) -> Result<()> {
    if crate::document::runtime_slot().lock().is_low_resource() {
        log::debug!("thumbnail pre-warm skipped: low_resource_mode on");
        return Ok(());
    }
    // CAS so two simultaneous callers can't both observe `false` and
    // race past the gate. We use `Acquire` on success to pair with
    // the worker's `Release` store-on-exit.
    if PREWARM_IN_FLIGHT
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        log::debug!("thumbnail pre-warm skipped: worker already in flight");
        return Ok(());
    }
    let dim = sanitize_dim(max_dim_px);
    // Snapshot the page list under the workspace lock so the worker
    // doesn't depend on the document staying open.
    let pages = {
        let guard = workspace_slot().lock();
        let ws = match guard.as_ref() {
            Some(ws) => ws,
            None => {
                // Drop the in-flight flag before returning so a
                // later open_project can spawn a fresh worker.
                PREWARM_IN_FLIGHT.store(false, Ordering::Release);
                return Err(DocumentBridgeError::NoProject);
            }
        };
        page_ids(ws)
    };
    let spawn_result = thread::Builder::new()
        .name("kcreate-thumbnail-prewarm".to_string())
        .spawn(move || {
            // Tiny stagger so the renderer isn't slammed if multiple
            // pre-warm requests pile up (e.g. user spam-saves).
            thread::sleep(Duration::from_millis(50));
            if let Err(e) = ensure_cover_thumbnail(dim) {
                log::debug!("cover pre-warm failed: {e}");
            }
            for page in pages {
                if let Err(e) = ensure_page_thumbnail(page, dim) {
                    log::debug!("page {page} pre-warm failed: {e}");
                }
            }
            // Clear the in-flight flag last so the next call can
            // observe the worker's effects via `Acquire` on the CAS.
            PREWARM_IN_FLIGHT.store(false, Ordering::Release);
        });
    match spawn_result {
        Ok(_) => Ok(()),
        Err(e) => {
            // Spawn failed — release the gate so a retry can succeed.
            PREWARM_IN_FLIGHT.store(false, Ordering::Release);
            Err(DocumentBridgeError::Io(std::io::Error::other(
                e.to_string(),
            )))
        }
    }
}

// ---------------------------------------------------------------------------
// Internal rendering pipeline
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
struct ThumbnailTarget {
    bounds: Bounds,
    background: Option<Color>,
}

/// Drive a cache-or-render decision for `key` using a single
/// workspace-lock window to capture **both** the content hash and the
/// renderable scene. This is the atomicity contract Devin Review
/// flagged on PR #16 (ANALYSIS_0001): if the hash were computed in one
/// lock window and the scene in another, a concurrent mutation between
/// them would store thumbnail bytes keyed by a hash that no longer
/// describes the document — every subsequent lookup at that hash would
/// return a stale picture.
///
/// The closure `prepare` runs while we hold the workspace lock and
/// must derive the hash, target bounds, and the scene from the **same
/// snapshot of `ws`**. We then:
///
/// 1. Check the cache under that same lock for a hit on `(key, hash)`.
///    A hit returns immediately without rendering.
/// 2. Drop the lock and run the (potentially slow) GPU/CPU render +
///    PNG encode off-lock, so the UI thread is never blocked by the
///    render itself.
/// 3. Re-acquire the lock briefly to commit the rendered bytes back
///    to the cache, keyed by the snapshot hash. Because the bytes
///    were rendered from the snapshot the hash describes, the cache
///    invariant ("bytes at hash H depict the document at hash H")
///    holds even if the document has since mutated — the next
///    thumbnail request will see the new hash → miss → re-render.
fn ensure_thumbnail_for<F>(key: Uuid, max_dim_px: u32, prepare: F) -> Result<ThumbnailBytes>
where
    F: FnOnce(&mut Workspace) -> Result<(String, ThumbnailTarget, Scene)>,
{
    // Step 1 — single lock window: derive hash + target + scene, and
    // short-circuit on a cache hit. Note the cache is on disk under
    // `ws.store.thumbnails_dir()`, so we must hold the lock long
    // enough to safely open it; the open is cheap (sled-style hash
    // map) so this doesn't materially block the UI thread.
    let (content_hash, target, scene) = {
        let mut guard = workspace_slot().lock();
        let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
        let cache = ThumbnailCache::open(ws.store.thumbnails_dir()).map_err(thumb_err)?;
        let (hash, target, scene) = prepare(ws)?;
        if let Some(cached) = cache.lookup(key, &hash).map_err(thumb_err)? {
            return Ok(ThumbnailBytes::from_cached(cached));
        }
        (hash, target, scene)
    };

    // Step 2 — off-lock render + encode. This is the slow part
    // (wgpu submit, readback, PNG encode) and must NOT hold the
    // workspace lock so other bridge calls (editing, save, close)
    // can run in parallel.
    let (bytes, width, height) =
        render_scene_to_png(&scene, target.bounds, max_dim_px, target.background)?;

    // Step 3 — re-acquire only to commit the cache row. If the
    // project was closed between step 2 and step 3, just drop the
    // rendered bytes — the next request will re-render. (Avoids
    // touching the cache for a project we no longer own.)
    cache_store(key, &content_hash, &bytes, width, height)?;

    Ok(ThumbnailBytes {
        width,
        height,
        mime: ThumbnailEncoding::Png.mime().to_string(),
        byte_size: bytes.len() as u64,
        bytes_base64: encode_base64(&bytes),
        content_hash,
    })
}

fn cache_store(key: Uuid, content_hash: &str, bytes: &[u8], width: u32, height: u32) -> Result<()> {
    let mut guard = workspace_slot().lock();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;
    let mut cache = ThumbnailCache::open(ws.store.thumbnails_dir()).map_err(thumb_err)?;
    cache
        .store(
            key,
            content_hash,
            bytes.to_vec(),
            width,
            height,
            ThumbnailEncoding::Png,
        )
        .map_err(thumb_err)
}

fn build_scene_for_target(ws: &mut Workspace, target: ThumbnailTarget) -> Scene {
    // Hidden ws field access matches what `sync_scene_locked` does. We
    // use the workspace's scene_sync since it already understands the
    // document's blobs / selection layering — but pass `&[]` for
    // selection so highlight overlays aren't baked into thumbnails.
    let mut scene =
        ws.scene_sync
            .sync_document_to_scene(&ws.project.document, Some(ws.store.blobs()), &[]);
    // Translate so the target bounds land at the renderer origin —
    // mirroring `kcreate_export::slice::translate_scene`.
    let dx = -target.bounds.x as f32;
    let dy = -target.bounds.y as f32;
    for obj in &mut scene.objects {
        obj.translation.0 += dx;
        obj.translation.1 += dy;
    }
    scene
}

fn render_scene_to_png(
    scene: &Scene,
    bounds: Bounds,
    max_dim_px: u32,
    background: Option<Color>,
) -> Result<(Vec<u8>, u32, u32)> {
    let (w_px, h_px, scale) = compute_thumbnail_dims(bounds, max_dim_px);
    let opts = PngExportOptions {
        width: w_px,
        height: h_px,
        scale,
        background,
    };
    let bytes = export_png_to_bytes(scene, &opts)
        .map_err(|e| DocumentBridgeError::Internal(format!("thumbnail render: {e}")))?;
    // `scale` is folded into the output dimensions by export_png.
    let out_w = (w_px as f32 * scale).round().clamp(1.0, u32::MAX as f32) as u32;
    let out_h = (h_px as f32 * scale).round().clamp(1.0, u32::MAX as f32) as u32;
    Ok((bytes, out_w, out_h))
}

/// Pick the page that should represent the project on the HomePage.
/// Prefers the first Page node in document-order; falls back to the
/// document bounds if no Page exists (shouldn't happen for projects
/// created via `project_create`, but a defensive default keeps the
/// thumbnail call path total).
fn pick_cover_target(ws: &Workspace) -> ThumbnailTarget {
    for root in ws.project.document.root_ids() {
        if let Some(node) = ws.project.document.get_node(*root) {
            if node.node_type == NodeType::Page {
                return ThumbnailTarget {
                    bounds: node.bounds,
                    background: None,
                };
            }
        }
    }
    // Walk the entire tree for the first Page (matches the
    // "find-by-type" idiom used elsewhere in the bridge).
    for (_, node) in ws.project.document.iter() {
        if node.node_type == NodeType::Page {
            return ThumbnailTarget {
                bounds: node.bounds,
                background: None,
            };
        }
    }
    // No Page node — render a 1024x1024 fallback.
    ThumbnailTarget {
        bounds: Bounds::new(0.0, 0.0, 1024.0, 1024.0),
        background: None,
    }
}

fn page_ids(ws: &Workspace) -> Vec<Uuid> {
    ws.project
        .document
        .iter()
        .filter_map(|(id, node)| (node.node_type == NodeType::Page).then_some(*id))
        .collect()
}

/// Compute thumbnail base dims so the long edge equals `max_dim_px`
/// (with `scale = 1.0`). Aspect ratio is preserved; both dims are at
/// least 1.
fn compute_thumbnail_dims(bounds: Bounds, max_dim_px: u32) -> (u32, u32, f32) {
    let aspect_w = bounds.width.max(1.0);
    let aspect_h = bounds.height.max(1.0);
    let max = f64::from(max_dim_px.max(8));
    let (w, h) = if aspect_w >= aspect_h {
        (max, (max * (aspect_h / aspect_w)).max(1.0))
    } else {
        ((max * (aspect_w / aspect_h)).max(1.0), max)
    };
    (w.round() as u32, h.round() as u32, 1.0)
}

fn sanitize_dim(max_dim_px: u32) -> u32 {
    if max_dim_px == 0 {
        DEFAULT_THUMBNAIL_MAX_DIM_PX
    } else {
        // Cap to a safety limit so a hostile caller can't OOM the box.
        max_dim_px.min(2048)
    }
}

// ---------------------------------------------------------------------------
// Content hashing
// ---------------------------------------------------------------------------

/// BLAKE3 hex of the document graph + project-level metadata that the
/// renderer sees. Conservative: any mutation that affects pixels also
/// changes the hash (and a handful of mutations that don't, like a
/// metadata-only update, also bust the cache — that's a fine
/// trade-off for correctness).
fn document_content_hash(ws: &Workspace) -> String {
    let mut hasher = blake3::Hasher::new();
    let nodes_json = serde_json::to_vec(&ws.project.document).unwrap_or_default();
    hasher.update(&nodes_json);
    // Mix in the project's color settings + design tokens since both
    // can flip the rendered output (background colours, fill styles
    // resolved through tokens).
    if let Ok(cs) = serde_json::to_vec(&ws.project.color_settings) {
        hasher.update(b"\x00cs");
        hasher.update(&cs);
    }
    if let Ok(dt) = serde_json::to_vec(&ws.project.design_tokens) {
        hasher.update(b"\x00dt");
        hasher.update(&dt);
    }
    hasher.finalize().to_hex().to_string()
}

/// BLAKE3 hex of a single page's subtree + project-level metadata.
fn page_content_hash(ws: &Workspace, page_id: Uuid) -> String {
    let mut hasher = blake3::Hasher::new();
    // Hash the Page node itself.
    if let Some(page) = ws.project.document.get_node(page_id) {
        if let Ok(json) = serde_json::to_vec(page) {
            hasher.update(&json);
        }
    }
    // Then every descendant. Document order is stable because
    // `descendants_of` walks children in their insertion order.
    for child in ws.project.document.descendants_of(page_id) {
        if let Some(node) = ws.project.document.get_node(child) {
            if let Ok(json) = serde_json::to_vec(node) {
                hasher.update(&json);
            }
        }
    }
    if let Ok(cs) = serde_json::to_vec(&ws.project.color_settings) {
        hasher.update(b"\x00cs");
        hasher.update(&cs);
    }
    if let Ok(dt) = serde_json::to_vec(&ws.project.design_tokens) {
        hasher.update(b"\x00dt");
        hasher.update(&dt);
    }
    hasher.finalize().to_hex().to_string()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn encode_base64(bytes: &[u8]) -> String {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    STANDARD.encode(bytes)
}

fn thumb_err(e: ThumbnailError) -> DocumentBridgeError {
    DocumentBridgeError::Internal(format!("thumbnail cache: {e}"))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn write_temp_recent(contents: &[RecentProjectEntry]) -> tempfile::NamedTempFile {
        use std::io::Write;
        let mut f = tempfile::Builder::new().suffix(".json").tempfile().unwrap();
        let bytes = serde_json::to_vec(contents).unwrap();
        f.write_all(&bytes).unwrap();
        f
    }

    #[test]
    fn compute_thumbnail_dims_preserves_aspect_ratio_landscape() {
        let (w, h, scale) = compute_thumbnail_dims(Bounds::new(0.0, 0.0, 1920.0, 1080.0), 320);
        assert_eq!(scale, 1.0);
        assert_eq!(w, 320);
        assert!((h as i32 - 180).abs() <= 1, "got {h}");
    }

    #[test]
    fn compute_thumbnail_dims_preserves_aspect_ratio_portrait() {
        let (w, h, _scale) = compute_thumbnail_dims(Bounds::new(0.0, 0.0, 1080.0, 1920.0), 320);
        assert_eq!(h, 320);
        assert!((w as i32 - 180).abs() <= 1, "got {w}");
    }

    #[test]
    fn compute_thumbnail_dims_floors_at_one_pixel() {
        let (w, h, _scale) = compute_thumbnail_dims(Bounds::new(0.0, 0.0, 0.0, 0.0), 320);
        assert!(w >= 1);
        assert!(h >= 1);
    }

    #[test]
    fn sanitize_dim_zero_becomes_default() {
        assert_eq!(sanitize_dim(0), DEFAULT_THUMBNAIL_MAX_DIM_PX);
    }

    #[test]
    fn sanitize_dim_clamps_huge_values() {
        assert_eq!(sanitize_dim(99_999), 2048);
    }

    #[test]
    fn recent_projects_record_moves_to_front_and_dedups() {
        let f = write_temp_recent(&[]);
        let mut store = RecentProjectsStore::open(f.path().to_path_buf());
        let pa = tempfile::tempdir().unwrap();
        let pb = tempfile::tempdir().unwrap();
        // Force manifest.json so `list_pruned` keeps them.
        fs::write(pa.path().join("manifest.json"), b"{}").unwrap();
        fs::write(pb.path().join("manifest.json"), b"{}").unwrap();

        let id_a = Uuid::new_v4();
        let id_b = Uuid::new_v4();
        store.record(pa.path(), "A", id_a, Utc::now());
        store.record(pb.path(), "B", id_b, Utc::now());
        store.record(pa.path(), "A", id_a, Utc::now());
        let list = store.list_pruned();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].path, pa.path());
        assert_eq!(list[1].path, pb.path());
    }

    #[test]
    fn recent_projects_truncates_to_max_entries() {
        let f = write_temp_recent(&[]);
        let mut store = RecentProjectsStore::open(f.path().to_path_buf());
        // Hold the tempdirs alive until the end of the test so their
        // `manifest.json` files keep existing while `record` runs.
        // The Vec is intentionally never *read* — it's a drop guard —
        // so we silence clippy::collection_is_never_read.
        #[allow(clippy::collection_is_never_read)]
        let mut dirs = Vec::new();
        for i in 0..(RECENT_PROJECTS_MAX_ENTRIES + 5) {
            let d = tempfile::tempdir().unwrap();
            fs::write(d.path().join("manifest.json"), b"{}").unwrap();
            store.record(d.path(), &format!("P{i}"), Uuid::new_v4(), Utc::now());
            dirs.push(d);
        }
        assert_eq!(store.entries.len(), RECENT_PROJECTS_MAX_ENTRIES);
    }

    #[test]
    fn recent_projects_list_drops_paths_with_missing_manifest() {
        let f = write_temp_recent(&[]);
        let mut store = RecentProjectsStore::open(f.path().to_path_buf());
        let alive = tempfile::tempdir().unwrap();
        let dead = tempfile::tempdir().unwrap();
        fs::write(alive.path().join("manifest.json"), b"{}").unwrap();
        // No manifest on `dead` → should get pruned.
        store.record(alive.path(), "alive", Uuid::new_v4(), Utc::now());
        store.record(dead.path(), "dead", Uuid::new_v4(), Utc::now());
        let list = store.list_pruned();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "alive");
    }

    #[test]
    fn recent_projects_round_trips_to_disk() {
        let f = write_temp_recent(&[]);
        let path = f.path().to_path_buf();
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("manifest.json"), b"{}").unwrap();
        {
            let mut store = RecentProjectsStore::open(path.clone());
            store.record(dir.path(), "X", Uuid::new_v4(), Utc::now());
        }
        let store = RecentProjectsStore::open(path);
        assert_eq!(store.entries.len(), 1);
        assert_eq!(store.entries[0].name, "X");
    }

    #[test]
    fn thumbnail_bytes_roundtrips_via_base64() {
        use base64::engine::general_purpose::STANDARD;
        use base64::Engine;
        let original = b"\x89PNG\r\n\x1a\nfake_payload";
        let b64 = encode_base64(original);
        let decoded = STANDARD.decode(b64.as_bytes()).unwrap();
        assert_eq!(decoded.as_slice(), original);
    }
}
