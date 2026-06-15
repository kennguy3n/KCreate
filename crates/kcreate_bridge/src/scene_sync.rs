//! Document → renderer scene translator.
//!
//! The renderer's [`Scene`] is keyed by [`ObjectId`] (a `u64`) and the
//! document graph is keyed by [`Uuid`]. They are deliberately separate
//! id spaces because the renderer is a stateless replay surface that
//! doesn't care about the document's identity, history, or metadata —
//! every render is driven by a fresh snapshot.
//!
//! [`SceneSync`] owns the bidirectional [`Uuid`] ⇄ [`ObjectId`] mapping
//! that bridges those id spaces, plus the *translation* that walks the
//! document tree and emits scene objects. The translation is purely
//! deterministic from the document state (plus a small amount of
//! incidental ordering via `next_id`), so it's safe to call from any
//! mutation site to re-derive the scene.
//!
//! For Phase 0 the translator handles:
//!
//! * [`NodeType::Artboard`] → a soft drop-shadow rect behind the
//!   artboard, a background [`ObjectKind::Rect`] filled with the
//!   artboard colour, and a name label drawn above the top edge.
//!   Descendants whose bounds fall entirely outside the artboard
//!   clip rect are pruned from the scene (Figma/Penpot-style frame
//!   clipping). Dashed spacing guides between adjacent artboards
//!   during drag/resize are not yet emitted — that gesture-state
//!   plumbing is not exposed by the bridge.
//! * [`NodeType::VectorLayer`] with a `vector_path` metadata entry →
//!   an [`ObjectKind::Path`] driven by the layer's [`VectorPath`].
//! * [`NodeType::RasterLayer`] with a `raster_image` metadata entry →
//!   an [`ObjectKind::Image`] whose pixels are loaded from the blob
//!   store; falls back to a coloured placeholder rect when the blob
//!   isn't available (e.g. the project was opened on a fresh box and
//!   the blob hasn't been resolved yet).
//! * [`NodeType::TextLayer`] with `text` metadata → an
//!   [`ObjectKind::Text`] painted at the node origin.
//! * Selection highlights appended on top, in document order.
//!
//! Invisible nodes (`node.visible == false`) are skipped *with their
//! entire subtree*, mirroring the editor's "hide group, hide
//! everything under it" semantics.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use kcreate_core::document::DocumentGraph;
use kcreate_core::node::{Node, NodeType, PathEffect};
use kcreate_renderer::{
    Color, Object, ObjectId, ObjectKind, Paint, PathCommand, Point2, Rect, Scene, Stroke, Style,
};
use kcreate_storage::blobs::BlobStore;
use kcreate_vector::{PathSegment, VectorPath};
use uuid::Uuid;

// The on-disk metadata schema (DTOs + key strings) is the contract
// between this translator and every export pipeline. Owning the schema
// in `kcreate_export::scene_metadata` keeps consumers that do not link
// the bridge (preflight, icon-pack rendering, PDF flatten) pointed at a
// single source of truth. We re-export the names here so existing
// `crate::scene_sync::…` call sites continue to work — but only one
// definition exists.
pub use kcreate_export::scene_metadata::{
    RasterImageMeta, TextLayerMeta, RASTER_IMAGE_METADATA_KEY, TEXT_LAYER_METADATA_KEY,
    VECTOR_PATH_METADATA_KEY,
};

/// Selection-highlight stroke colour (`KChat` primary `#7C3AED` at 50%).
const SELECTION_STROKE: Color = Color {
    r: 0.486_274_5,
    g: 0.227_450_98,
    b: 0.929_411_77,
    a: 0.5,
};
/// Default raster placeholder colour when blob loading fails.
const RASTER_PLACEHOLDER: Color = Color {
    r: 0.85,
    g: 0.85,
    b: 0.85,
    a: 1.0,
};
/// Default background clear colour (paper white).
pub const DEFAULT_CLEAR: Color = Color {
    r: 1.0,
    g: 1.0,
    b: 1.0,
    a: 1.0,
};

/// Soft drop-shadow under each artboard. Matches the Figma/Penpot
/// convention of a small offset + low alpha so multi-artboard pages
/// read at a glance.
const ARTBOARD_SHADOW: Color = Color {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 0.12,
};
/// Horizontal/vertical inflation of the shadow rect relative to the
/// artboard. The shadow is offset down-right by this amount so it
/// peeks out from under the artboard background.
const ARTBOARD_SHADOW_OFFSET: f32 = 6.0;
/// Pixel-grid distance between the artboard's top edge and the
/// baseline of the name label drawn above it.
const ARTBOARD_LABEL_GAP: f32 = 8.0;
/// Default label colour for artboard names (KChat secondary text).
const ARTBOARD_LABEL_COLOR: Color = Color {
    r: 0.45,
    g: 0.42,
    b: 0.52,
    a: 1.0,
};
/// Default label font (matches the renderer's text shaping default).
const ARTBOARD_LABEL_FONT: &str = "Inter";
/// Default label font size in document pixels.
const ARTBOARD_LABEL_FONT_SIZE: f32 = 12.0;

/// Width (in screen pixels) of the remote-peer cursor triangle.
/// `append_presence_cursors` divides this by the current viewport
/// zoom to produce a world-space size, so the on-screen triangle
/// stays a constant size as the user pans / zooms — matching the
/// Figma / Photoshop convention for remote cursors. Sized to read
/// clearly without overlapping fine vector detail.
const CURSOR_WIDTH: f32 = 14.0;
/// Height (in screen pixels) of the remote-peer cursor triangle.
const CURSOR_HEIGHT: f32 = 18.0;
/// Gap (in screen pixels) between the cursor's bounding box and
/// the name label.
const CURSOR_LABEL_GAP: f32 = 2.0;
/// Font size (in screen pixels) used for the cursor's display-name
/// label. Slightly smaller than artboard labels because cursors
/// stack up when multiple peers are in the same area.
const CURSOR_LABEL_FONT_SIZE: f32 = 10.0;
/// Minimum viewport zoom factor used when scaling cursor geometry
/// from screen-space pixels into world-space units. Cursors stop
/// growing once the user has zoomed out beyond this point — at
/// extreme zoom-outs a cursor proportionally inflated to stay
/// 14 px wide would dwarf the document, which is worse UX than
/// a cursor that disappears below the resolution floor.
const CURSOR_MIN_VIEWPORT_ZOOM: f32 = 0.05;

/// Width (in screen pixels) of a remote-peer selection halo
/// stroke. Like cursors, halo strokes are quoted in screen pixels
/// and divided by viewport zoom so the halo reads at a constant
/// thickness regardless of pan/zoom.
const HALO_STROKE_WIDTH: f32 = 2.0;
/// Alpha of remote-peer halos. The local user's selection
/// highlight is drawn at 50% alpha; remote halos use 70% so they
/// pop slightly above (peers don't expect their own selection to
/// be more prominent than remote peers').
const HALO_STROKE_ALPHA: f32 = 0.7;
/// Font size (in screen pixels) of the peer-name label drawn at
/// the top-left of each halo. Smaller than the cursor label
/// because halos are bounded by node geometry and the label
/// shouldn't overpower a small node.
const HALO_LABEL_FONT_SIZE: f32 = 9.0;
/// Inset (in screen pixels) of the halo-label baseline below the
/// top edge of the halo rect.
const HALO_LABEL_OFFSET: f32 = 3.0;
/// Inflate the halo rect outwards by this many screen pixels so
/// the stroke sits *outside* the node rather than overlapping the
/// node's own stroke and creating a moiré.
const HALO_OUTSET: f32 = 1.5;

/// World-space cursor + label payload for a single remote peer.
///
/// Lives at the bridge layer (not in `kcreate_renderer`) so the
/// renderer stays oblivious to collaboration concepts. The bridge
/// translates an authoritative collab presence map into a list of
/// these and hands them to [`SceneSync::append_presence_cursors`].
#[derive(Debug, Clone, PartialEq)]
pub struct PresenceCursor {
    /// Opaque peer identifier; used as the seed for the
    /// deterministic per-peer cursor colour.
    pub peer_id: String,
    /// Human-readable name painted next to the cursor. May be
    /// empty (in which case the label is suppressed).
    pub display_name: String,
    /// Cursor position in document world coordinates.
    pub x: f64,
    pub y: f64,
}

/// Per-peer selection payload for halo rendering. Mirrors
/// [`PresenceCursor`] but for the `selection: Vec<Uuid>` field of
/// the collab presence message.
///
/// The renderer draws one peer-coloured halo rect per `node_id`
/// that is also currently emitted in the document scene — invisible
/// or non-existent nodes are silently filtered (a peer can drag a
/// selection off a node that gets deleted before the next sync
/// without crashing the local renderer).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PresenceSelection {
    /// Opaque peer identifier; reuses the same hash → HSL hue
    /// derivation as [`PresenceCursor`] so a peer's cursor and
    /// their selection halos share one colour.
    pub peer_id: String,
    /// Human-readable name. Drawn as a small pill anchored to the
    /// top-left of every halo so collisions between two peers'
    /// selections still read clearly.
    pub display_name: String,
    /// Node ids the peer is currently selecting on the active
    /// page. Order is preserved so the renderer can paint them in
    /// the same z-order the peer sees locally.
    pub node_ids: Vec<Uuid>,
}

/// First `ObjectId` value used for overlay objects (selection
/// highlights, artboard shadow rects, artboard name labels — any
/// scene object that isn't tied to a document UUID).
///
/// Overlays live in the high `[OVERLAY_ID_THRESHOLD, u64::MAX]`
/// range, while document-backed objects come from a monotonic
/// allocator starting at `1` and counting up (see
/// [`SceneSync::next_id`]). The two ranges never collide in practice:
/// a project would need more than 2^63 live mappings before a real
/// `ObjectId` could land at or above this threshold, which is
/// physically impossible. Hit-testing uses the constant via
/// [`is_overlay_id`] so the exclusion is a guaranteed id-range check
/// rather than a fragile style heuristic.
pub const OVERLAY_ID_THRESHOLD: u64 = u64::MAX / 2;

/// Legacy alias preserved for crates that imported the
/// pre-Phase-1-Block-A name. New code should use
/// [`OVERLAY_ID_THRESHOLD`].
pub const HIGHLIGHT_ID_THRESHOLD: u64 = OVERLAY_ID_THRESHOLD;

/// Returns whether the given `ObjectId` was allocated for an overlay
/// object (selection highlight, artboard shadow, artboard label, …)
/// rather than a document-backed node.
#[must_use]
pub const fn is_overlay_id(id: ObjectId) -> bool {
    id.0 >= OVERLAY_ID_THRESHOLD
}

/// Legacy alias for [`is_overlay_id`] — see [`HIGHLIGHT_ID_THRESHOLD`].
#[must_use]
pub const fn is_selection_highlight_id(id: ObjectId) -> bool {
    is_overlay_id(id)
}

/// Bidirectional `Uuid` ⇄ `ObjectId` map plus a monotonic id allocator.
///
/// The id allocator is intentionally local to each [`SceneSync`] —
/// it's not a process-global counter — so tests can construct
/// independent sync instances and reason about deterministic ids.
///
/// `overlay_watermark` is the next overlay ID that
/// [`append_presence_cursors`] should allocate from. It is reset
/// to [`OVERLAY_ID_THRESHOLD`] at the start of every
/// [`sync_document_to_scene`] call and advanced past every artboard
/// overlay emitted during that call, so a follow-up
/// `append_presence_cursors` continues the same upward stream
/// rather than restarting at the threshold and colliding with the
/// artboard overlays that just got emitted.
/// Phase 10 Block E Task 26 — per-node cache entry. Leaf-node emit
/// (`emit_vector`, `emit_raster`, `emit_text`) is idempotent given an
/// unchanged [`Node::version`], so we record the `Object`s those
/// emitters produced and replay them on a subsequent sync without
/// re-walking the node metadata. The sub-id list lets us re-populate
/// the reverse `object_id_to_uuid` map for dashed vector paths whose
/// sub-paths each carry their own `ObjectId`.
#[derive(Debug, Clone)]
struct NodeCacheEntry {
    /// Snapshot of `Node::version` when these objects were emitted.
    version: u64,
    /// Objects emitted by the leaf-node emitter. `z` values are
    /// rewritten on reuse so they slot into the current sync's z
    /// stream rather than the historical one.
    objects: Vec<Object>,
    /// Every sub-`ObjectId` (sub-paths of a dashed vector) whose
    /// reverse-map entry must be restored. Includes the primary id
    /// for uniformity.
    sub_object_ids: Vec<ObjectId>,
    /// The `z` advance the original emit produced, captured as
    /// `z_after - z_before`. Today every leaf emitter increments `z`
    /// by exactly one per emitted [`Object`], so this is identical to
    /// `objects.len() as i32` — but storing the actual delta lets a
    /// future emitter reserve z slots (e.g. for sub-layers) without
    /// silently corrupting the replay path. Replay rebases the
    /// stream's `z` by exactly this many units, regardless of how
    /// many objects were cached.
    z_advance: i32,
}

/// Phase 11 perf-at-scale — full-scene fast-path cache.
///
/// The per-node [`NodeCacheEntry`] cache only spares the *leaf metadata
/// walk* for unchanged nodes; [`SceneSync::sync_document_to_scene`]
/// still recurses the entire tree, rebuilds the reverse id map, sweeps
/// four `HashMap`s, and runs an `O(N log N)` z-sort on **every** call —
/// even when the document graph has not changed at all. That happens
/// constantly: a [`SceneSync::sync_document_to_scene`] fires on every
/// `canvas_hit_test` (i.e. every hover/move the host queries) and on
/// every selection change, neither of which mutates the graph. For a
/// large document that is `O(N log N)` of pure waste per pointer event.
///
/// This snapshot holds the *content* objects (everything the tree walk
/// emitted, **before** selection highlights) from the last full
/// rebuild, already in the staged order the rebuild produced. When the
/// next sync sees an empty dirty set and no structural change, it
/// clones these objects and re-appends only the (selection-sized,
/// not document-sized) highlight overlay — skipping the walk, the map
/// rebuild, and the four sweeps entirely. The single batched z-sort is
/// retained so the output is byte-for-byte identical to a full rebuild.
#[derive(Debug, Clone)]
struct ContentSceneCache {
    /// Objects emitted by the document tree walk, excluding selection
    /// highlights. Staged exactly as the producing full rebuild left
    /// them so re-appending highlights + one sort reproduces it.
    content_objects: Vec<Object>,
    /// The `z` counter value immediately after the content walk, i.e.
    /// the base `z` the highlight loop starts from.
    content_z: i32,
    /// `overlay_watermark` as the producing rebuild left it, so a
    /// follow-up `append_presence_cursors` resumes the same id stream.
    overlay_watermark: u64,
    /// Whether the full rebuild that produced this cache was given a
    /// `blob_store`. Raster emit is the one content path whose output
    /// depends on `blob_store` presence: with `Some` it emits the real
    /// image, with `None` it emits a coloured placeholder rect. The
    /// fast path replays cached content *verbatim* and never consults
    /// `blob_store`, so serving this cache when the caller's
    /// `blob_store.is_some()` no longer matches would leave a stale
    /// placeholder (or a stale image) on screen. The fast-path guard
    /// compares this against the current call's `blob_store.is_some()`
    /// and falls through to a full rebuild on any transition. Both
    /// production callers currently always pass `Some`, so this is
    /// defensive against a future caller that does not.
    blob_store_present: bool,
}

#[derive(Debug)]
pub struct SceneSync {
    uuid_to_object_id: HashMap<Uuid, ObjectId>,
    object_id_to_uuid: HashMap<ObjectId, Uuid>,
    next_id: AtomicU64,
    overlay_watermark: u64,
    /// Phase 11 perf-at-scale — snapshot of the last full rebuild's
    /// content objects, used to short-circuit a no-op-document sync
    /// (hit-test / selection change) without re-walking the tree.
    /// `None` until the first sync and whenever the cache is
    /// conservatively invalidated. See [`ContentSceneCache`].
    content_cache: Option<ContentSceneCache>,
    /// Per-node cache of previously-emitted display-list objects.
    /// Only populated for leaf node types whose `emit_*` is a pure
    /// function of `(node, blob_store)` — Vector, Raster, Text.
    /// Container nodes (Artboard, Page, Group) are NOT cached
    /// because their emits draw overlay decorations (drop shadow,
    /// label) whose `ObjectId`s come from the per-sync overlay
    /// allocator and must not be re-used across syncs.
    node_cache: HashMap<Uuid, NodeCacheEntry>,
    /// Last-seen `Node::version` per uuid. Lets the next sync
    /// distinguish a stale cache (version bumped → re-emit) from a
    /// fresh cache (version unchanged → reuse). Kept separately
    /// from `node_cache` so a cache miss can still update the
    /// version table without allocating an empty entry.
    last_version: HashMap<Uuid, u64>,
    /// Test-only counter: how many times the no-op-document fast
    /// path ([`Self::sync_from_content_cache`]) has been taken since
    /// this `SceneSync` was constructed. Output-equality with a full
    /// rebuild alone cannot prove the fast path actually ran (that is
    /// precisely the property under test), so the tests read this
    /// per-instance counter to assert the branch was exercised. It is
    /// `#[cfg(test)]` and compiled out of production builds entirely.
    #[cfg(test)]
    fast_path_hits: u64,
}

impl Default for SceneSync {
    /// Delegates to [`SceneSync::new`] so a `default()`-constructed
    /// instance is identical to a `new()`-constructed one. The derived
    /// `Default` would instead zero `next_id` and `overlay_watermark`,
    /// which is wrong on both counts: the document-object allocator
    /// would hand out `ObjectId(0)` — the reserved "no object" sentinel
    /// — for the first node, and `overlay_watermark` would start below
    /// [`OVERLAY_ID_THRESHOLD`] so [`is_overlay_id`] would mis-classify
    /// the first artboard overlay as a document object. `thumbnails.rs`
    /// constructs an ephemeral `SceneSync::default()` per render, so the
    /// two constructors must agree.
    fn default() -> Self {
        Self::new()
    }
}

impl SceneSync {
    #[must_use]
    pub fn new() -> Self {
        Self {
            uuid_to_object_id: HashMap::new(),
            object_id_to_uuid: HashMap::new(),
            // ObjectId(0) is reserved as a sentinel for "no object".
            next_id: AtomicU64::new(1),
            overlay_watermark: OVERLAY_ID_THRESHOLD,
            content_cache: None,
            node_cache: HashMap::new(),
            last_version: HashMap::new(),
            #[cfg(test)]
            fast_path_hits: 0,
        }
    }

    /// Diagnostic accessor for [`tests`] — number of leaf-node
    /// entries currently held in the incremental cache.
    #[must_use]
    pub fn cached_node_count(&self) -> usize {
        self.node_cache.len()
    }

    fn next_overlay_id(state: &mut OverlayIdAllocator) -> ObjectId {
        let id = ObjectId(state.next);
        state.next = state.next.saturating_add(1);
        // We start at OVERLAY_ID_THRESHOLD and count up; selection
        // highlights count down from u64::MAX. The two streams meet at
        // the midpoint of u64::MAX, which is unreachable in practice.
        id
    }

    /// Stable lookup: which renderer object corresponds to this doc uuid?
    #[must_use]
    pub fn object_id_for_uuid(&self, id: Uuid) -> Option<ObjectId> {
        self.uuid_to_object_id.get(&id).copied()
    }

    /// Reverse lookup: which document node produced this renderer object?
    #[must_use]
    pub fn uuid_for_object_id(&self, id: ObjectId) -> Option<Uuid> {
        self.object_id_to_uuid.get(&id).copied()
    }

    /// Total mapped objects (test/inspection only).
    #[must_use]
    pub fn len(&self) -> usize {
        self.uuid_to_object_id.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.uuid_to_object_id.is_empty()
    }

    /// Drop *all* mappings. Call when the document graph is replaced
    /// wholesale (e.g. project reopen) so stale uuids never resurface.
    pub fn clear(&mut self) {
        self.uuid_to_object_id.clear();
        self.object_id_to_uuid.clear();
        self.next_id.store(1, Ordering::Relaxed);
        self.overlay_watermark = OVERLAY_ID_THRESHOLD;
        self.content_cache = None;
        self.node_cache.clear();
        self.last_version.clear();
    }

    fn allocate(&self, doc_id: Uuid) -> ObjectId {
        if let Some(existing) = self.uuid_to_object_id.get(&doc_id) {
            return *existing;
        }
        ObjectId(self.next_id.fetch_add(1, Ordering::Relaxed))
    }

    fn record(&mut self, doc_id: Uuid, obj_id: ObjectId) {
        self.uuid_to_object_id.insert(doc_id, obj_id);
        self.object_id_to_uuid.insert(obj_id, doc_id);
    }

    /// Mint a *fresh* `ObjectId` for an auxiliary scene `Object` that
    /// belongs to `parent_doc_id` but should have a distinct renderer
    /// identity (e.g. each sub-path produced by a dash path effect).
    ///
    /// Unlike [`allocate`], this never returns an existing id —
    /// callers expect a brand-new value so the scene's `Vec<Object>`
    /// can carry several entries for the same node without sharing
    /// an `ObjectId`. The reverse map is populated so hit-tests on
    /// any sub-id resolve back to the parent node uuid; the forward
    /// map is left alone so `object_id_for_uuid(parent_doc_id)`
    /// continues to point at the *primary* id recorded by the
    /// first sub-path's `record` call.
    fn allocate_sub_object_id(&mut self, parent_doc_id: Uuid) -> ObjectId {
        let id = ObjectId(self.next_id.fetch_add(1, Ordering::Relaxed));
        self.object_id_to_uuid.insert(id, parent_doc_id);
        id
    }

    /// Phase 10 Block E Task 26 — try to replay a previously-cached
    /// leaf emit for `node`. Returns `true` when the cache hit and
    /// objects were appended; the caller should NOT then call the
    /// fresh `emit_*` for this node.
    ///
    /// Cache hit conditions:
    /// 1. An entry exists for `node.id` from a prior sync.
    /// 2. The cached `version` matches `node.version` exactly (any
    ///    mutation bumps it via `Node::touch`).
    ///
    /// On hit, the cached `Object`s are cloned with their stored
    /// relative z values (`0, 1, 2, …`) rebased onto the current `z`
    /// stream so they slot into the right place in document order.
    /// The reverse-map entries for every sub-`ObjectId` are restored
    /// so hit-testing on a sub-path still resolves back to the parent
    /// uuid.
    fn try_replay_cached(
        &mut self,
        node: &Node,
        objects: &mut Vec<Object>,
        z: &mut i32,
        emitted: &mut Vec<Uuid>,
    ) -> bool {
        let Some(entry) = self.node_cache.get(&node.id) else {
            return false;
        };
        if entry.version != node.version {
            return false;
        }
        let base_z = *z;
        // Use the recorded `z_advance` rather than the cached object
        // count: replay must move the z stream by the same delta the
        // original emit moved it, even if a future emitter chooses to
        // skip slots (e.g. reserving z values for adjustment sub-
        // layers). Coupling replay to `len()` looks innocent today
        // because every leaf emitter is z+=1 per object, but it would
        // silently produce mismatched z values the day that
        // assumption changes.
        let advance = entry.z_advance;
        // Push cached objects by reference + clone-on-push. We used
        // to materialise the whole `entry.objects` vec into a
        // temporary `cached_objects` clone purely to end the borrow
        // on `node_cache` before the post-loop mutations on
        // `uuid_to_object_id` / `object_id_to_uuid`. Object clones
        // still happen (the cache must outlive each emit) but
        // they're now spread one-per-push instead of eagerly
        // cloning the entire `Vec<Object>`, and we no longer
        // allocate the intermediate `cached_objects: Vec<Object>`
        // at all. For a steady-state sync over a 1000-node doc
        // that eliminates 1000 wasted `Vec<Object>` heap
        // allocations per frame.
        for obj in &entry.objects {
            let mut cloned = obj.clone();
            cloned.z = base_z.saturating_add(obj.z);
            objects.push(cloned);
        }
        // `sub_object_ids` is `Vec<ObjectId>` (i.e. `Vec<u64>`-sized)
        // and typically holds one entry per leaf node. Cloning it
        // here ends the `node_cache` borrow so the map-update
        // mutations below can take `&mut self.{uuid,object}_id_to_*`.
        // Eliminating this clone too would require either a
        // split-borrow trick that Rust's borrow checker cannot see
        // through `HashMap::get` (it doesn't know `node_cache`,
        // `uuid_to_object_id`, `object_id_to_uuid` are disjoint
        // fields of `self`) or moving the maps onto a sub-struct
        // that excludes `node_cache`. Neither is worth the code-
        // shape disruption for an 8-byte-per-entry clone of a
        // typically-1-entry vec.
        let sub_object_ids = entry.sub_object_ids.clone();
        *z = z.saturating_add(advance);
        // Repopulate the maps so hit-testing works the same as a
        // fresh emit. The primary id is the first sub id (every
        // leaf emit calls `record` for the first object); follow-on
        // sub ids only need the reverse map.
        if let Some(&primary) = sub_object_ids.first() {
            self.uuid_to_object_id.insert(node.id, primary);
        }
        for &sub_id in &sub_object_ids {
            self.object_id_to_uuid.insert(sub_id, node.id);
        }
        emitted.push(node.id);
        self.last_version.insert(node.id, node.version);
        true
    }

    /// Phase 10 Block E Task 26 — capture the objects emitted by a
    /// leaf `emit_*` call into [`node_cache`] so the next sync with
    /// the same `node.version` can replay them.
    ///
    /// `obj_start` and `z_start` are the watermarks captured before
    /// the emit; cached `z` values are stored relative to `z_start`
    /// so the replay path can rebase them onto an arbitrary current
    /// `z`.
    fn capture_cache(
        &mut self,
        node: &Node,
        objects: &[Object],
        obj_start: usize,
        z_start: i32,
        z_end: i32,
    ) {
        let mut cached_objects = Vec::with_capacity(objects.len() - obj_start);
        let mut sub_ids = Vec::with_capacity(objects.len() - obj_start);
        for obj in &objects[obj_start..] {
            sub_ids.push(obj.id);
            let mut snapshot = obj.clone();
            snapshot.z = obj.z.saturating_sub(z_start);
            cached_objects.push(snapshot);
        }
        let z_advance = z_end.saturating_sub(z_start);
        self.node_cache.insert(
            node.id,
            NodeCacheEntry {
                version: node.version,
                objects: cached_objects,
                sub_object_ids: sub_ids,
                z_advance,
            },
        );
        self.last_version.insert(node.id, node.version);
    }

    /// Phase 11 Block A Task 2 — read-only sync entry point used by
    /// test fixtures that hand in a `&DocumentGraph`. Always does a
    /// full rebuild (no dirty-set drain, no incremental cache
    /// pruning). Production code MUST use
    /// [`Self::sync_document_to_scene`] so dirty tracking stays in
    /// lockstep with the document.
    pub fn sync_document_to_scene_borrowed(
        &mut self,
        doc: &DocumentGraph,
        blob_store: Option<&BlobStore>,
        selection: &[Uuid],
    ) -> Scene {
        // Conservative: invalidate the entire incremental cache so a
        // borrowed-form caller cannot accidentally serve stale
        // entries from a previous mutable sync.
        self.node_cache.clear();
        self.last_version.clear();
        let scene = self.sync_document_to_scene_inner(doc, blob_store, selection);
        // Conservative: the immutable test-only path bypasses dirty
        // tracking, so it must never leave a content-cache that a
        // later production `sync_document_to_scene` could serve as a
        // no-op fast path. `_inner` populated it above; drop it.
        self.content_cache = None;
        scene
    }

    /// Translate the document graph into a renderer [`Scene`].
    ///
    /// **Phase 11 Block A Task 2 — incremental sync.**
    ///
    /// Takes [`&mut DocumentGraph`] so the sync can drain the
    /// document's dirty set in one atomic step. When
    /// `structure_dirty` is set, the per-node version cache is
    /// flushed (forcing every leaf-node emit_* to re-walk metadata).
    /// When only a subset of node ids is dirty, only those entries
    /// are evicted from the cache; the version-comparison path in
    /// [`Self::try_replay_cached`] then naturally reuses every other
    /// node's previous emit. A document with no edits since the
    /// previous sync (empty dirty set, `structure_dirty == false`)
    /// replays the entire scene from cache without re-walking any
    /// leaf metadata.
    ///
    /// `blob_store` is consulted for raster image pixel data. When
    /// `None` (or when a particular blob can't be loaded), raster
    /// layers fall back to a coloured placeholder rect so the canvas
    /// still shows *something* in their place.
    ///
    /// `selection` is the current set of selected node ids. For each
    /// selected node we append a stroked highlight rect on top of all
    /// content; the highlights are NOT recorded in the `uuid↔object_id`
    /// map (they have ids in a private high range so hit-testing
    /// against them never collides with a real node).
    pub fn sync_document_to_scene(
        &mut self,
        doc: &mut DocumentGraph,
        blob_store: Option<&BlobStore>,
        selection: &[Uuid],
    ) -> Scene {
        let (dirty_ids, structure_dirty) = doc.drain_dirty();
        // Phase 11 perf-at-scale — no-op-document fast path. When the
        // graph has not changed since the last full rebuild (empty
        // dirty set, no structural change) the entire tree walk,
        // reverse-map rebuild, and four `HashMap` sweeps are pure
        // waste: they reproduce exactly the content objects already
        // captured in `content_cache`. This is the common case for
        // `canvas_hit_test` (fires on every hover/move) and selection
        // changes, neither of which mutates the graph. Replay the
        // cached content and re-append only the (selection-sized)
        // highlight overlay. The same no-edits invariant the per-node
        // cache already trusts guarantees the cached content is still
        // current; see [`ContentSceneCache`].
        //
        // The cache is only valid when the `blob_store` presence still
        // matches the rebuild that produced it: raster emit is the one
        // content path whose output depends on whether a `blob_store`
        // was supplied (real image vs. placeholder rect), and the fast
        // path replays content verbatim without consulting it. A
        // `None`→`Some` (or `Some`→`None`) transition must therefore
        // force a full rebuild so rasters are re-emitted. See
        // [`ContentSceneCache::blob_store_present`].
        if !structure_dirty
            && dirty_ids.is_empty()
            && self
                .content_cache
                .as_ref()
                .is_some_and(|cache| cache.blob_store_present == blob_store.is_some())
        {
            return self.sync_from_content_cache(doc, selection);
        }
        if structure_dirty {
            // Tree shape changed (insert/remove/reparent/reorder).
            // Evict the entire incremental cache because the
            // z-order / container chrome must be re-emitted from
            // scratch — z values are positional and would slot in
            // wrong if we replayed.
            self.node_cache.clear();
            self.last_version.clear();
        } else {
            // Only specific node properties changed. Evict just
            // those entries; everything else replays.
            for id in &dirty_ids {
                self.node_cache.remove(id);
                self.last_version.remove(id);
            }
        }
        self.sync_document_to_scene_inner(doc, blob_store, selection)
    }

    /// Internal scene walk shared by [`Self::sync_document_to_scene`]
    /// and the legacy immutable test entry point
    /// [`Self::sync_document_to_scene_borrowed`]. The dirty-set
    /// handling lives in the public mutable form so the internal
    /// walk only depends on read access to the graph.
    fn sync_document_to_scene_inner(
        &mut self,
        doc: &DocumentGraph,
        blob_store: Option<&BlobStore>,
        selection: &[Uuid],
    ) -> Scene {
        // Rebuild the reverse map (`object_id_to_uuid`) from the
        // forward map (`uuid_to_object_id`) so the two stay in
        // lock-step. The forward map persists across syncs so
        // `allocate` can reuse stable `ObjectId`s for uuids that
        // re-appear (a node that goes invisible and back, an undo,
        // etc.); the reverse map is purely derived and only needs
        // to be valid after each sync.
        //
        // Rust's split-borrow rules let us iterate
        // `&self.uuid_to_object_id` while mutating the disjoint
        // `&mut self.object_id_to_uuid` directly, so neither a
        // `mem::take` swap nor a `HashMap::clone()` of the forward
        // map is needed — both used to live here purely as borrow-
        // checker workarounds. For a 1000-node steady-state sync
        // that eliminates one full `HashMap<Uuid, ObjectId>` clone
        // per call.
        self.object_id_to_uuid.clear();
        self.object_id_to_uuid.reserve(self.uuid_to_object_id.len());
        for (&uuid, &obj_id) in &self.uuid_to_object_id {
            self.object_id_to_uuid.insert(obj_id, uuid);
        }

        let mut scene = Scene::new(DEFAULT_CLEAR);
        // Accumulate every object emitted during the recursive walk
        // into a single Vec, then push them into the scene in one
        // `add_objects` call below. Calling `Scene::add_object` per
        // emitted node is O(N²·log N) because each push triggers a
        // re-sort of the growing vec (see `Scene::add_object`); a
        // single batched `add_objects` collapses that to one
        // O(N·log N) stable sort at the end. Pre-allocate to roughly
        // the document's node count so dense scenes don't realloc
        // their way up through the powers-of-two on every push;
        // `node_count()` is cheap (HashMap len).
        let mut staged: Vec<Object> = Vec::with_capacity(doc.node_count());
        let mut z = 0_i32;
        let mut emitted_uuids: Vec<Uuid> = Vec::new();
        // Reset every sync: `sync_document_to_scene` rebuilds the
        // scene from scratch, so the previous watermark is stale.
        // `append_presence_cursors` (called after this returns) will
        // pick up the post-walk value and continue the same upward
        // stream so cursor overlays do not collide with artboards.
        let mut overlay = OverlayIdAllocator::new();
        for root in doc.root_ids() {
            self.visit(
                doc,
                *root,
                blob_store,
                &mut staged,
                &mut z,
                &mut emitted_uuids,
                &mut overlay,
                None,
            );
        }

        // Sweep mappings whose uuid no longer corresponds to an emitted
        // scene object so a node that goes invisible (and later comes
        // back) doesn't bring a fresh `ObjectId` with it.
        let kept: std::collections::HashSet<Uuid> = emitted_uuids.iter().copied().collect();
        self.uuid_to_object_id.retain(|uuid, _| kept.contains(uuid));
        // Mirror the prune into the reverse map.
        self.object_id_to_uuid.retain(|_, uuid| kept.contains(uuid));
        // Phase 10 Block E Task 26 — sweep stale cache entries for
        // nodes that didn't appear in this sync (deleted or hidden).
        // Keeping them would leak memory and risk a wrong replay if
        // a future sync somehow encounters the same uuid in a
        // different document state.
        self.node_cache.retain(|uuid, _| kept.contains(uuid));
        self.last_version.retain(|uuid, _| kept.contains(uuid));

        // Persist the watermark for follow-up overlay emitters
        // (notably `append_presence_cursors`). Selection highlights
        // count downward from `u64::MAX` so they don't participate in
        // this watermark — see [`Self::append_presence_cursors`].
        self.overlay_watermark = overlay.next;

        // Phase 11 perf-at-scale — snapshot the content objects (and
        // the z / watermark state the highlight + presence emitters
        // resume from) BEFORE appending selection highlights, so a
        // following no-op-document sync can replay this exact staged
        // order without re-walking the tree. Highlights are
        // selection-dependent and re-derived on every sync, so they
        // are deliberately excluded from the snapshot. See
        // [`ContentSceneCache`].
        self.content_cache = Some(ContentSceneCache {
            content_objects: staged.clone(),
            content_z: z,
            overlay_watermark: self.overlay_watermark,
            blob_store_present: blob_store.is_some(),
        });

        // Selection highlights go on top, sorted by document order so
        // overlapping selections paint deterministically. Accumulated
        // into the same `staged` vec so the final `add_objects` call
        // sorts everything together — highlights have monotonically
        // increasing `z` values from the post-walk watermark, so the
        // sort places them last regardless.
        Self::append_selection_highlights(&mut staged, &mut z, doc, selection);

        // Single batched insert + sort. See the `Vec::with_capacity`
        // comment above for the perf rationale.
        scene.add_objects(staged);

        scene
    }

    /// Phase 11 perf-at-scale — no-op-document fast path body.
    ///
    /// Replays the content objects captured by the last full rebuild
    /// (held in [`Self::content_cache`]) and re-appends the current
    /// selection highlights, skipping the tree walk, the reverse-map
    /// rebuild, and the four `HashMap` sweeps that
    /// [`Self::sync_document_to_scene_inner`] performs. The output is
    /// byte-for-byte identical to a full rebuild of an unchanged
    /// document (proven by `fast_path_matches_full_rebuild`): the
    /// cached objects are in the same pre-sort staged order, the
    /// highlights are appended with the same ids / z values from the
    /// same `content_z` base, and the single batched `add_objects`
    /// applies the same stable sort.
    ///
    /// Caller MUST guarantee `self.content_cache.is_some()`.
    fn sync_from_content_cache(&mut self, doc: &DocumentGraph, selection: &[Uuid]) -> Scene {
        #[cfg(test)]
        {
            self.fast_path_hits += 1;
        }
        // Copy the scalars and clone the content vec out of the cache
        // in a tight scope so the immutable borrow of
        // `self.content_cache` ends before we mutate `self`.
        let (mut staged, mut z, watermark) = {
            let cache = self
                .content_cache
                .as_ref()
                .expect("sync_from_content_cache requires a populated content_cache");
            (
                cache.content_objects.clone(),
                cache.content_z,
                cache.overlay_watermark,
            )
        };
        // Restore the post-content-walk watermark so a follow-up
        // `append_presence_cursors` resumes the same upward id stream
        // it would have after a full rebuild.
        self.overlay_watermark = watermark;
        Self::append_selection_highlights(&mut staged, &mut z, doc, selection);
        let mut scene = Scene::new(DEFAULT_CLEAR);
        scene.add_objects(staged);
        scene
    }

    /// Append a stroked highlight rect for each visible selected node
    /// onto `staged`, advancing `z` per highlight. Shared by the full
    /// rebuild ([`Self::sync_document_to_scene_inner`]) and the no-op
    /// fast path ([`Self::sync_from_content_cache`]) so both produce
    /// identical overlays. Highlights take ids counting down from
    /// `u64::MAX` so they never collide with real node `ObjectId`s and
    /// `is_selection_highlight_id` keeps them out of hit-testing; they
    /// are intentionally NOT recorded in the `uuid <-> object_id` map.
    fn append_selection_highlights(
        staged: &mut Vec<Object>,
        z: &mut i32,
        doc: &DocumentGraph,
        selection: &[Uuid],
    ) {
        let mut highlight_id = u64::MAX;
        for sel_uuid in selection {
            let Some(node) = doc.get_node(*sel_uuid) else {
                continue;
            };
            if !node.visible {
                continue;
            }
            let world = node_world_bounds(node);
            let style = Style {
                fill: None,
                stroke: Some(Stroke::new(SELECTION_STROKE, 2.0)),
            };
            let highlight = Object::new(
                ObjectKind::Rect(Rect::new(
                    world.x as f32,
                    world.y as f32,
                    world.width as f32,
                    world.height as f32,
                )),
                style,
            )
            .with_id(ObjectId(highlight_id))
            .with_z(*z);
            staged.push(highlight);
            *z += 1;
            highlight_id = highlight_id.saturating_sub(1);
        }
    }

    /// Append remote-peer cursor overlays to an existing scene, in
    /// addition to whatever selection highlights / artboard chrome
    /// `sync_document_to_scene` already laid down.
    ///
    /// Each cursor renders as:
    ///
    /// 1. A small filled triangle (`Path`) pointing up-left at the
    ///    cursor's world-space `(x, y)`, with a deterministic
    ///    peer-specific colour derived from the peer id hash.
    /// 2. A short label (`Text`) carrying the peer's display name,
    ///    painted just below-right of the triangle so the cursor
    ///    glyph itself doesn't occlude it.
    ///
    /// The objects are emitted through the same scene-graph path as
    /// real document layers — they composite naturally with strokes,
    /// fills, and effects. Hit testing is unaffected: the ids come
    /// from the overlay range (high half of `u64`) so
    /// `is_overlay_id` returns true and `hit_test` filters them out
    /// before reverse-z scanning.
    ///
    /// `viewport_zoom` is the renderer's current pixels-per-scene-unit
    /// scale. The cursor triangle, label gap, and label font size
    /// are quoted in **screen pixels** ([`CURSOR_WIDTH`],
    /// [`CURSOR_HEIGHT`], [`CURSOR_LABEL_FONT_SIZE`]) and divided by
    /// the viewport zoom before being baked into the scene, so the
    /// renderer ends up drawing them at a constant on-screen size
    /// regardless of pan/zoom — matching the Figma / Photoshop
    /// remote-cursor convention. `viewport_zoom` is clamped to
    /// [`CURSOR_MIN_VIEWPORT_ZOOM`] to avoid runaway sizes at
    /// extreme zoom-outs and to defend against any caller that
    /// might pass `0.0` or a negative value.
    ///
    /// Cursor overlay IDs continue the same upward stream that
    /// [`sync_document_to_scene`] used for artboard overlays (via
    /// the persisted `overlay_watermark`), so cursor and artboard
    /// overlays never collide in the high `[OVERLAY_ID_THRESHOLD,
    /// u64::MAX]` range — even though both streams count upward
    /// from the same base.
    pub fn append_presence_cursors(
        &mut self,
        scene: &mut Scene,
        cursors: &[PresenceCursor],
        starting_z: i32,
        viewport_zoom: f32,
    ) {
        if cursors.is_empty() {
            return;
        }
        // Continue the overlay-id stream where `sync_document_to_scene`
        // left off (artboard shadows + labels emitted earlier in this
        // scene). Restarting at `OVERLAY_ID_THRESHOLD` would collide
        // with every artboard overlay in the scene; selection
        // highlights are not at risk because they count *downward*
        // from `u64::MAX`.
        let mut overlay = OverlayIdAllocator::resuming(self.overlay_watermark);
        // Convert screen-pixel constants to world units. Clamp to
        // avoid div-by-zero on uninitialised viewports and runaway
        // sizes at very small zooms (cursor that fills the canvas is
        // worse UX than a cursor that vanishes below resolution).
        let zoom = viewport_zoom.max(CURSOR_MIN_VIEWPORT_ZOOM);
        let cursor_width_world = CURSOR_WIDTH / zoom;
        let cursor_height_world = CURSOR_HEIGHT / zoom;
        let cursor_label_gap_world = CURSOR_LABEL_GAP / zoom;
        let cursor_label_font_size_world = CURSOR_LABEL_FONT_SIZE / zoom;
        // Stage every cursor + label object in a local buffer then
        // hand the whole batch to `Scene::add_objects` so the scene
        // sorts ONCE instead of once per push. With 100 peers that
        // means a single O(N·log N) sort over the freshly-emitted
        // 200-ish overlay objects instead of 200 sorts of a growing
        // vec — an order-of-magnitude reduction in scene-sync cost
        // for cursor-heavy collab sessions.
        let mut emitted: Vec<Object> = Vec::with_capacity(cursors.len() * 2);
        let mut z = starting_z;
        for cursor in cursors {
            let color = peer_color(&cursor.peer_id);
            let (cx, cy) = (cursor.x as f32, cursor.y as f32);

            // Triangle outline (filled): an 8-vertex teardrop pointer
            // is overkill for a remote cursor; a 3-point isoceles
            // triangle reads instantly and costs almost nothing to
            // tesselate on the CPU backend.
            let path = vec![
                PathCommand::MoveTo(Point2::new(cx, cy)),
                PathCommand::LineTo(Point2::new(
                    cx + cursor_width_world,
                    cy + cursor_height_world * 0.6,
                )),
                PathCommand::LineTo(Point2::new(
                    cx + cursor_width_world * 0.45,
                    cy + cursor_height_world * 0.6,
                )),
                PathCommand::LineTo(Point2::new(
                    cx + cursor_width_world * 0.3,
                    cy + cursor_height_world,
                )),
                PathCommand::Close,
            ];
            let cursor_style = Style {
                fill: Some(Paint::Solid(color)),
                stroke: Some(Stroke::new(Color::rgba(1.0, 1.0, 1.0, 0.9), 1.0)),
            };
            let cursor_id = Self::next_overlay_id(&mut overlay);
            emitted.push(
                Object::new(ObjectKind::Path(path), cursor_style)
                    .with_id(cursor_id)
                    .with_z(z),
            );
            z += 1;

            // Name label below-right of the cursor tip. We emit the
            // text only when a non-empty display_name is set; otherwise
            // the bridge would render an empty Text node which the
            // renderer happily wastes a glyph cache slot on.
            if !cursor.display_name.is_empty() {
                let label_origin = Point2::new(
                    cx + cursor_width_world + cursor_label_gap_world,
                    cy + cursor_height_world + cursor_label_font_size_world,
                );
                let label_style = Style {
                    fill: Some(Paint::Solid(color)),
                    stroke: None,
                };
                let label_id = Self::next_overlay_id(&mut overlay);
                emitted.push(
                    Object::new(
                        ObjectKind::Text {
                            origin: label_origin,
                            text: cursor.display_name.clone(),
                            font_family: ARTBOARD_LABEL_FONT.to_string(),
                            font_size: cursor_label_font_size_world,
                        },
                        label_style,
                    )
                    .with_id(label_id)
                    .with_z(z),
                );
                z += 1;
            }
        }
        scene.add_objects(emitted);
        // Persist the watermark so a follow-up cursor append (e.g. a
        // second presence push in the same frame, hypothetically)
        // continues the stream too.
        self.overlay_watermark = overlay.next;
    }

    /// Append remote-peer selection halos to the scene.
    ///
    /// For every entry in `selections`, draws a peer-coloured stroke
    /// rectangle around the world bounds of every node id in the
    /// peer's selection set, plus a small name pill anchored at the
    /// halo's top-left. The local user's own selection is rendered
    /// separately by [`Self::sync_document_to_scene`] using the
    /// neutral `SELECTION_STROKE` colour — the two never collide
    /// because halos are emitted in the overlay id range (continuing
    /// the same upward stream `append_presence_cursors` uses) and
    /// the local selection counts downward from `u64::MAX`.
    ///
    /// `viewport_zoom` clamping and screen→world conversion match
    /// [`Self::append_presence_cursors`] exactly: stroke width,
    /// label font size, and outset are quoted in screen pixels and
    /// divided by the clamped zoom so a remote selection halo
    /// reads at constant thickness regardless of pan/zoom — the
    /// same Figma / Photoshop convention.
    ///
    /// Nodes that are absent from `doc` or marked invisible are
    /// silently skipped: a peer's selection lags the document tree
    /// (e.g. a peer drags-and-drops a layer just as a third peer
    /// deletes it), and rendering halos around dangling ids would
    /// either misplace them at the origin or panic.
    ///
    /// Returns the *next* free z value after every halo + label has
    /// been emitted. Callers that paint additional overlays on top
    /// (e.g. presence cursors) MUST start from this returned value
    /// rather than guessing a constant offset — with even one peer
    /// selecting one node and providing a display name, two
    /// objects are emitted at `starting_z` and `starting_z + 1`,
    /// so a hard-coded gap of `1` would put the next overlay
    /// stream *underneath* the halo label. Threading the watermark
    /// out is the only way to guarantee cursors always paint above
    /// halos no matter how many peers / selected nodes / labels
    /// got emitted this frame.
    pub fn append_presence_selection_halos(
        &mut self,
        scene: &mut Scene,
        doc: &DocumentGraph,
        selections: &[PresenceSelection],
        starting_z: i32,
        viewport_zoom: f32,
    ) -> i32 {
        if selections.is_empty() {
            return starting_z;
        }
        // Continue the same upward overlay-id stream that
        // `sync_document_to_scene` / `append_presence_cursors`
        // emitted from. Restarting at `OVERLAY_ID_THRESHOLD` would
        // collide with everything those two already laid down.
        let mut overlay = OverlayIdAllocator::resuming(self.overlay_watermark);
        let zoom = viewport_zoom.max(CURSOR_MIN_VIEWPORT_ZOOM);
        let stroke_width_world = HALO_STROKE_WIDTH / zoom;
        let label_font_size_world = HALO_LABEL_FONT_SIZE / zoom;
        let label_offset_world = HALO_LABEL_OFFSET / zoom;
        let outset_world = HALO_OUTSET / zoom;
        // Stage every halo + label in a local buffer then hand the
        // whole batch to `Scene::add_objects` so the scene sorts
        // ONCE instead of once per push. With 100 peers each
        // halo'ing 20 nodes that's a single sort of ~2000 freshly
        // emitted overlay objects instead of 2000 sorts of a growing
        // vec — the scene-sync hot path for multi-peer collab
        // sessions, so the saving is large. Capacity hint is an
        // upper bound (label is emitted at most once per peer);
        // overshooting wastes a few hundred bytes, undershooting
        // forces a realloc mid-loop.
        let label_budget = selections.len();
        let halo_budget: usize = selections.iter().map(|s| s.node_ids.len()).sum();
        let mut emitted: Vec<Object> = Vec::with_capacity(halo_budget + label_budget);
        let mut z = starting_z;
        for selection in selections {
            let base = peer_color(&selection.peer_id);
            let stroke_color = Color::rgba(base.r, base.g, base.b, HALO_STROKE_ALPHA);
            // Anchor the peer-name label to the *first rendered*
            // halo, not the first node id in the list. If the
            // user's selection starts with an invisible or
            // since-deleted node, the previous `== first()` guard
            // would skip every halo before the visible one and
            // then refuse to emit the label entirely (because the
            // visible id no longer matches `first()`). A simple
            // "emit once per peer" latch fixes that and keeps the
            // one-label-per-peer invariant the test pins.
            let mut label_emitted = false;
            for node_id in &selection.node_ids {
                let Some(node) = doc.get_node(*node_id) else {
                    continue;
                };
                if !node.visible {
                    continue;
                }
                let world = node_world_bounds(node);
                // Inflate the rect outwards so the halo stroke sits
                // outside the node's own paint surface. Without the
                // outset a 2 px stroke laid on top of a 1 px node
                // stroke moirés badly at most zooms.
                let rect = Rect::new(
                    (world.x - f64::from(outset_world)) as f32,
                    (world.y - f64::from(outset_world)) as f32,
                    (world.width + 2.0 * f64::from(outset_world)) as f32,
                    (world.height + 2.0 * f64::from(outset_world)) as f32,
                );
                let style = Style {
                    fill: None,
                    stroke: Some(Stroke::new(stroke_color, stroke_width_world)),
                };
                let halo_id = Self::next_overlay_id(&mut overlay);
                emitted.push(
                    Object::new(ObjectKind::Rect(rect), style)
                        .with_id(halo_id)
                        .with_z(z),
                );
                z += 1;

                // Peer-name pill at top-left of the first rendered
                // halo (see `label_emitted` doc above).
                if !selection.display_name.is_empty() && !label_emitted {
                    let origin = Point2::new(
                        (world.x - f64::from(outset_world)) as f32,
                        (world.y - f64::from(outset_world)) as f32 - label_offset_world,
                    );
                    let label_style = Style {
                        fill: Some(Paint::Solid(stroke_color)),
                        stroke: None,
                    };
                    let label_id = Self::next_overlay_id(&mut overlay);
                    emitted.push(
                        Object::new(
                            ObjectKind::Text {
                                origin,
                                text: selection.display_name.clone(),
                                font_family: ARTBOARD_LABEL_FONT.to_string(),
                                font_size: label_font_size_world,
                            },
                            label_style,
                        )
                        .with_id(label_id)
                        .with_z(z),
                    );
                    z += 1;
                    label_emitted = true;
                }
            }
        }
        scene.add_objects(emitted);
        // Persist watermark so any follow-up overlay emitter in the
        // same sync (currently none, but future-proof) continues
        // the stream.
        self.overlay_watermark = overlay.next;
        z
    }

    #[allow(clippy::too_many_arguments)]
    fn visit(
        &mut self,
        doc: &DocumentGraph,
        id: Uuid,
        blob_store: Option<&BlobStore>,
        objects: &mut Vec<Object>,
        z: &mut i32,
        emitted: &mut Vec<Uuid>,
        overlay: &mut OverlayIdAllocator,
        clip: Option<kcreate_core::node::Bounds>,
    ) {
        let Some(node) = doc.get_node(id) else { return };
        if !node.visible {
            return;
        }
        // Frame clipping: if a clip rect is in effect and this node's
        // world bounds are entirely outside it, prune the node *and*
        // its subtree from the scene. The clip never applies to the
        // artboard itself (it sets its own clip for descendants).
        if !matches!(node.node_type, NodeType::Artboard) {
            if let Some(clip_rect) = clip {
                let world = node_world_bounds(node);
                if !bounds_overlap(&world, &clip_rect) {
                    return;
                }
            }
        }
        let child_clip = match node.node_type {
            NodeType::Artboard => {
                self.emit_artboard(node, objects, z, emitted, overlay);
                Some(node_world_bounds(node))
            }
            NodeType::VectorLayer => {
                if !self.try_replay_cached(node, objects, z, emitted) {
                    let z_start = *z;
                    let obj_start = objects.len();
                    self.emit_vector(node, objects, z, emitted);
                    self.capture_cache(node, objects, obj_start, z_start, *z);
                }
                clip
            }
            NodeType::RasterLayer => {
                // Raster cache depends on blob_store availability —
                // if blob_store is None, we may emit a placeholder
                // rect; once a blob_store appears the next sync
                // should regenerate. Only cache when the blob_store
                // is present so cache→reality drift can't happen.
                if blob_store.is_some() && self.try_replay_cached(node, objects, z, emitted) {
                    // hit
                } else {
                    let z_start = *z;
                    let obj_start = objects.len();
                    self.emit_raster(node, objects, z, blob_store, emitted);
                    if blob_store.is_some() {
                        self.capture_cache(node, objects, obj_start, z_start, *z);
                    } else {
                        // Without a blob store, invalidate any prior
                        // cache so the next blob-bearing sync re-emits.
                        self.node_cache.remove(&node.id);
                    }
                }
                clip
            }
            NodeType::TextLayer => {
                if !self.try_replay_cached(node, objects, z, emitted) {
                    let z_start = *z;
                    let obj_start = objects.len();
                    self.emit_text(node, objects, z, emitted);
                    self.capture_cache(node, objects, obj_start, z_start, *z);
                }
                clip
            }
            NodeType::Page
            | NodeType::GroupLayer
            | NodeType::ComponentLayer
            | NodeType::LayoutFrame => clip,
        };
        for child in &node.children {
            self.visit(
                doc, *child, blob_store, objects, z, emitted, overlay, child_clip,
            );
        }
    }

    fn emit_artboard(
        &mut self,
        node: &Node,
        objects: &mut Vec<Object>,
        z: &mut i32,
        emitted: &mut Vec<Uuid>,
        overlay: &mut OverlayIdAllocator,
    ) {
        let world = node_world_bounds(node);

        // 1. Drop shadow under the artboard. Offset down-right by
        //    `ARTBOARD_SHADOW_OFFSET`. Drawn first so it sits behind
        //    everything that follows. Overlay id => not hit-testable.
        let shadow = Object::new(
            ObjectKind::Rect(Rect::new(
                world.x as f32 + ARTBOARD_SHADOW_OFFSET,
                world.y as f32 + ARTBOARD_SHADOW_OFFSET,
                world.width as f32,
                world.height as f32,
            )),
            Style {
                fill: Some(Paint::Solid(ARTBOARD_SHADOW)),
                stroke: None,
            },
        )
        .with_id(Self::next_overlay_id(overlay))
        .with_z(*z);
        objects.push(shadow);
        *z += 1;

        // 2. Artboard background rect — the hit-testable, document-
        //    backed object the user can select and drag.
        let obj_id = self.allocate(node.id);
        self.record(node.id, obj_id);
        emitted.push(node.id);
        // Gradient fills from `node_fill` are in the artboard's local
        // path space (0..w, 0..h); the rect itself is placed at world
        // coords with no object translation, so translate the paint to
        // match. A solid fill is unaffected (`translated` is a no-op).
        let fill = node_fill(node).unwrap_or_else(|| Paint::Solid(Color::rgba(1.0, 1.0, 1.0, 1.0)));
        let style = Style {
            fill: Some(fill),
            stroke: None,
        }
        .translated(world.x as f32, world.y as f32);
        let obj = Object::new(
            ObjectKind::Rect(Rect::new(
                world.x as f32,
                world.y as f32,
                world.width as f32,
                world.height as f32,
            )),
            style,
        )
        .with_id(obj_id)
        .with_z(*z);
        objects.push(obj);
        *z += 1;

        // 3. Name label above the artboard. Overlay id so the user
        //    can't accidentally select it. Skip when the artboard has
        //    no name (avoid emitting an empty Text object).
        if !node.name.is_empty() {
            let label_origin = Point2::new(world.x as f32, world.y as f32 - ARTBOARD_LABEL_GAP);
            let label = Object::new(
                ObjectKind::Text {
                    origin: label_origin,
                    text: node.name.clone(),
                    font_family: ARTBOARD_LABEL_FONT.to_string(),
                    font_size: ARTBOARD_LABEL_FONT_SIZE,
                },
                Style {
                    fill: Some(Paint::Solid(ARTBOARD_LABEL_COLOR)),
                    stroke: None,
                },
            )
            .with_id(Self::next_overlay_id(overlay))
            .with_z(*z);
            objects.push(label);
            *z += 1;
        }
    }

    fn emit_vector(
        &mut self,
        node: &Node,
        objects: &mut Vec<Object>,
        z: &mut i32,
        emitted: &mut Vec<Uuid>,
    ) {
        let Some(path) = node
            .metadata
            .get(VECTOR_PATH_METADATA_KEY)
            .and_then(|v| serde_json::from_value::<VectorPath>(v.clone()).ok())
        else {
            return;
        };
        // Phase 5 Block C Task 18 — apply the non-destructive path
        // effect chain stored on the node's style. Each effect is
        // applied in order; `Dash` expands to multiple sub-paths so
        // it must be emitted last (otherwise downstream effects would
        // see only the first sub-path).
        let effects: &[PathEffect] = &node.style.path_effects;
        let style = node_style(node);
        let (tx, ty) = node_translation(node);
        let paths = apply_path_effects(path, effects);
        emitted.push(node.id);
        let mut first = true;
        for sub in paths {
            // The first sub-path gets the node's *primary*
            // `ObjectId` (idempotent — re-uses the existing mapping
            // on re-sync). Every additional sub-path gets a fresh
            // id so the scene's `Vec<Object>` never holds two
            // entries with the same id. The reverse map is still
            // populated for the sub-ids so hit-tests on any of
            // them resolve back to `node.id`.
            let obj_id = if first {
                let id = self.allocate(node.id);
                self.record(node.id, id);
                first = false;
                id
            } else {
                self.allocate_sub_object_id(node.id)
            };
            let commands = vector_path_to_renderer(&sub);
            let obj = Object::new(ObjectKind::Path(commands), style.clone())
                .with_id(obj_id)
                .with_translation(tx as f32, ty as f32)
                .with_z(*z);
            objects.push(obj);
            *z += 1;
        }
        // If the chain produced zero sub-paths (e.g. a degenerate
        // dash result), still register a placeholder mapping so the
        // hit-test reverse map can resolve the node. `allocate`
        // alone only mints an `ObjectId`; the bidirectional
        // `uuid_to_object_id` / `object_id_to_uuid` tables are only
        // populated by `record`, which is what hit-testing reads.
        if first {
            let id = self.allocate(node.id);
            self.record(node.id, id);
        }
    }

    fn emit_raster(
        &mut self,
        node: &Node,
        objects: &mut Vec<Object>,
        z: &mut i32,
        blob_store: Option<&BlobStore>,
        emitted: &mut Vec<Uuid>,
    ) {
        let obj_id = self.allocate(node.id);
        self.record(node.id, obj_id);
        emitted.push(node.id);
        let world = node_world_bounds(node);
        let rect = Rect::new(
            world.x as f32,
            world.y as f32,
            world.width as f32,
            world.height as f32,
        );
        let meta = node
            .metadata
            .get(RASTER_IMAGE_METADATA_KEY)
            .and_then(|v| serde_json::from_value::<RasterImageMeta>(v.clone()).ok());
        // Phase 11 Block A Task 3 — compute the content-addressed
        // fingerprint up-front from the blob hash before we resolve
        // pixels. Surviving the resolve step means the renderer can
        // skip chunk-hashing the pixel buffer on every fingerprint.
        let content_hash = meta.as_ref().map(|m| blob_hash_to_token(&m.blob_hash));
        let resolved = meta
            .as_ref()
            .and_then(|m| resolve_raster_image(m, blob_store));
        let kind = if let Some((pw, ph, pixels)) = resolved {
            ObjectKind::Image {
                rect,
                pixels_width: pw,
                pixels_height: ph,
                pixels,
                content_hash,
            }
        } else {
            ObjectKind::Rect(rect)
        };
        let style = if matches!(kind, ObjectKind::Rect(_)) {
            Style {
                fill: Some(Paint::Solid(RASTER_PLACEHOLDER)),
                stroke: None,
            }
        } else {
            Style {
                fill: None,
                stroke: None,
            }
        };
        let obj = Object::new(kind, style).with_id(obj_id).with_z(*z);
        objects.push(obj);
        *z += 1;
    }

    fn emit_text(
        &mut self,
        node: &Node,
        objects: &mut Vec<Object>,
        z: &mut i32,
        emitted: &mut Vec<Uuid>,
    ) {
        let Some(meta) = node
            .metadata
            .get(TEXT_LAYER_METADATA_KEY)
            .and_then(|v| serde_json::from_value::<TextLayerMeta>(v.clone()).ok())
        else {
            return;
        };
        let obj_id = self.allocate(node.id);
        self.record(node.id, obj_id);
        emitted.push(node.id);
        let world = node_world_bounds(node);
        // Glyph outlines are baked at world coordinates in `draw_text`
        // (`origin` + outline), and the text object carries no object
        // translation, so `command_from_object` would translate the
        // style by (0, 0). A gradient fill from `node_fill` is in the
        // node's local space, so pre-translate it by the world origin —
        // the same approach `emit_artboard` uses — so the gradient spans
        // the text's world bounds and stays locked to the glyphs. Solid
        // fills are untouched (`translated` is a no-op on them).
        let style = node_style(node).translated(world.x as f32, world.y as f32);
        let obj = Object::new(
            ObjectKind::Text {
                origin: Point2::new(world.x as f32, world.y as f32),
                text: meta.text,
                font_family: meta.font_family,
                font_size: meta.font_size,
            },
            style,
        )
        .with_id(obj_id)
        .with_z(*z);
        objects.push(obj);
        *z += 1;
    }
}

/// **Phase 11 Block A Task 3 — content-addressed fingerprint token.**
///
/// Compresses a BLAKE3 blob hash (hex string, 64 chars / 32 bytes)
/// down to a 64-bit token by XOR-folding eight 8-byte stripes. The
/// scene fingerprint hashes the resulting `u64` instead of walking
/// the raster pixel buffer — collisions inside a single document are
/// negligible (a 4×10⁻¹⁸ birthday probability across 10 000 distinct
/// images) and indistinguishable from cache-noise even if they did
/// occur, because the renderer would just re-hash the unchanged
/// pixels and produce the same display-list cache key.
///
/// Inputs that aren't valid hex (corrupt project, in-memory only
/// rasters) fall back to FxHash of the raw bytes so we still produce
/// a stable token rather than silently emitting `0` for every layer.
#[must_use]
fn blob_hash_to_token(blob_hash: &str) -> u64 {
    // BLAKE3 produces 32 bytes / 64 hex chars. Decode strictly and
    // XOR-fold; non-hex inputs fall through to byte-FxHash.
    let decoded: Option<[u8; 32]> = {
        let bytes = blob_hash.as_bytes();
        if bytes.len() == 64 && bytes.iter().all(u8::is_ascii_hexdigit) {
            let mut out = [0u8; 32];
            for (i, chunk) in bytes.chunks_exact(2).enumerate() {
                let hi = (chunk[0] as char).to_digit(16).unwrap_or(0) as u8;
                let lo = (chunk[1] as char).to_digit(16).unwrap_or(0) as u8;
                out[i] = (hi << 4) | lo;
            }
            Some(out)
        } else {
            None
        }
    };
    if let Some(bytes) = decoded {
        let mut folded = [0u8; 8];
        for stripe_idx in 0..4 {
            let start = stripe_idx * 8;
            for i in 0..8 {
                folded[i] ^= bytes[start + i];
            }
        }
        u64::from_le_bytes(folded)
    } else {
        // Fallback: hash the raw string with a *fixed-seed* hasher.
        //
        // Phase 11 Block A follow-up — Devin Review BUG-0003.
        //
        // Earlier revisions used `RandomState::new()`, whose seed is
        // randomised per call — so the same blob string produced a
        // different token on each invocation, breaking the
        // pipeline's display-list cache for non-hex blob hashes
        // (tests, in-memory fixtures, corrupt project data).
        // `BuildHasherDefault::<DefaultHasher>` always constructs a
        // `DefaultHasher` with the standard zero-seeded SipHash
        // state, which is deterministic for the lifetime of the
        // process AND across processes on the same rustc build.
        use std::hash::{BuildHasher, BuildHasherDefault, Hasher};
        let builder = BuildHasherDefault::<std::collections::hash_map::DefaultHasher>::default();
        let mut hasher = builder.build_hasher();
        hasher.write(blob_hash.as_bytes());
        hasher.finish()
    }
}

fn resolve_raster_image(
    meta: &RasterImageMeta,
    blob_store: Option<&BlobStore>,
) -> Option<(u32, u32, Vec<u8>)> {
    let store = blob_store?;
    let bytes = store.load(&meta.blob_hash).ok()?;
    let expected = (meta.width as usize)
        .saturating_mul(meta.height as usize)
        .saturating_mul(4);
    if bytes.len() != expected {
        // The blob is not the raw RGBA8 buffer — most likely an
        // encoded PNG/JPEG/WebP. Decode through the `image` crate so
        // we hand the renderer the same raw RGBA8 representation it
        // expects from the wire format.
        return image::load_from_memory(&bytes).ok().map(|img| {
            let rgba = img.to_rgba8();
            let (w, h) = rgba.dimensions();
            (w, h, rgba.into_raw())
        });
    }
    Some((meta.width, meta.height, bytes))
}

/// Apply every [`PathEffect`] in `effects` to `path`, returning the
/// resulting sub-paths in render order. Effects are applied in chain
/// order; `Dash` is the only effect that can produce multiple sub-
/// paths, so when present it is always the final effect in the
/// pipeline (the inputs to it would otherwise be lost). An empty
/// effect list passes the original path through untouched.
fn apply_path_effects(mut path: VectorPath, effects: &[PathEffect]) -> Vec<VectorPath> {
    if effects.is_empty() {
        return vec![path];
    }
    // Apply every non-dash effect first; record the dash effect
    // (if any) so we can fan out at the end.
    let mut pending_dash: Option<&PathEffect> = None;
    for effect in effects {
        match effect {
            PathEffect::RoundCorners { radius } => {
                if radius.is_finite() && *radius > 0.0 {
                    path = kcreate_vector::round_corners(&path, *radius);
                }
            }
            PathEffect::Dash { .. } => {
                // Defer until after every other effect; if multiple
                // dash effects are stacked, the last one wins.
                pending_dash = Some(effect);
            }
        }
    }
    if let Some(PathEffect::Dash { pattern, offset }) = pending_dash {
        if !pattern.is_empty() {
            let subpaths = kcreate_vector::dash(&path, pattern, *offset);
            if !subpaths.is_empty() {
                return subpaths;
            }
        }
    }
    vec![path]
}

/// Convert a [`VectorPath`] into the renderer's [`PathCommand`]
/// representation. The two types diverged historically because the
/// renderer uses `Point2 { x: f32, y: f32 }` while the vector crate
/// keeps `PathPoint { x: f64, y: f64 }` for precision during boolean
/// ops; we collapse to f32 at the renderer boundary.
fn vector_path_to_renderer(path: &VectorPath) -> Vec<PathCommand> {
    let mut out = Vec::with_capacity(path.commands.len());
    for seg in &path.commands {
        match seg {
            PathSegment::MoveTo(p) => {
                out.push(PathCommand::MoveTo(Point2::new(p.x as f32, p.y as f32)));
            }
            PathSegment::LineTo(p) => {
                out.push(PathCommand::LineTo(Point2::new(p.x as f32, p.y as f32)));
            }
            PathSegment::QuadTo { ctrl, end } => {
                out.push(PathCommand::QuadTo {
                    ctrl: Point2::new(ctrl.x as f32, ctrl.y as f32),
                    end: Point2::new(end.x as f32, end.y as f32),
                });
            }
            PathSegment::CubicTo { ctrl1, ctrl2, end } => {
                out.push(PathCommand::CubicTo {
                    c1: Point2::new(ctrl1.x as f32, ctrl1.y as f32),
                    c2: Point2::new(ctrl2.x as f32, ctrl2.y as f32),
                    end: Point2::new(end.x as f32, end.y as f32),
                });
            }
            PathSegment::Close => out.push(PathCommand::Close),
        }
    }
    out
}

/// Counter for overlay (non-document) scene-object ids. Starts at
/// [`OVERLAY_ID_THRESHOLD`] and counts upward; selection highlights
/// count downward from `u64::MAX` so the two streams never collide
/// in practice (they'd need to span ~2^63 ids first).
///
/// Two callers in `SceneSync` allocate from this upward stream
/// during a single sync: `sync_document_to_scene` (artboard
/// shadows + labels) and `append_presence_cursors` (remote-peer
/// cursors). The second caller resumes from the first caller's
/// post-walk watermark (`SceneSync::overlay_watermark`) so the two
/// allocations share one contiguous range instead of restarting
/// from `OVERLAY_ID_THRESHOLD` and colliding.
#[derive(Debug)]
struct OverlayIdAllocator {
    next: u64,
}

impl OverlayIdAllocator {
    fn new() -> Self {
        Self {
            next: OVERLAY_ID_THRESHOLD,
        }
    }

    /// Resume the upward overlay stream from a previously-saved
    /// watermark. Used by [`SceneSync::append_presence_cursors`]
    /// to continue past the artboard overlay IDs emitted earlier in
    /// the same sync. Values below [`OVERLAY_ID_THRESHOLD`] are
    /// clamped up, so a caller that forgets to seed the watermark
    /// from `sync_document_to_scene` still produces overlay-range
    /// IDs rather than colliding with document-backed objects.
    fn resuming(watermark: u64) -> Self {
        Self {
            next: watermark.max(OVERLAY_ID_THRESHOLD),
        }
    }
}

/// Axis-aligned bounding-rect intersection test in world units.
/// Returns `false` when either rect has zero area along one axis on
/// the same side of the other rect (i.e. fully outside).
fn bounds_overlap(a: &kcreate_core::node::Bounds, b: &kcreate_core::node::Bounds) -> bool {
    let a_right = a.x + a.width;
    let a_bottom = a.y + a.height;
    let b_right = b.x + b.width;
    let b_bottom = b.y + b.height;
    a.x < b_right && a_right > b.x && a.y < b_bottom && a_bottom > b.y
}

fn node_world_bounds(node: &Node) -> kcreate_core::node::Bounds {
    // Bounds are stored in local space; transform offset folds in
    // translation. Phase 0 doesn't support rotation in the renderer,
    // so we drop rotation and shear on the floor (they're 0 by default
    // for shapes created via the bridge).
    kcreate_core::node::Bounds {
        x: node.bounds.x + node.transform.tx,
        y: node.bounds.y + node.transform.ty,
        width: node.bounds.width,
        height: node.bounds.height,
    }
}

const fn node_translation(node: &Node) -> (f64, f64) {
    (node.transform.tx, node.transform.ty)
}

const fn rgba_to_color(c: kcreate_core::node::RgbaColor) -> Color {
    Color::rgba(c.r, c.g, c.b, c.a)
}

fn point2d_to_point2(p: kcreate_core::node::Point2D) -> Point2 {
    Point2::new(p.x as f32, p.y as f32)
}

/// Lower core gradient stops (f64 offset, `RgbaColor`) to the renderer's
/// `(f32 offset, Color)` representation, preserving document order.
fn gradient_stops_to_renderer(stops: &[kcreate_core::node::GradientStop]) -> Vec<(f32, Color)> {
    stops
        .iter()
        .map(|s| (s.offset as f32, rgba_to_color(s.color)))
        .collect()
}

/// Translate a node's fill into a renderer [`Paint`]. Solid fills become
/// `Paint::Solid`; linear / radial gradients carry their endpoints and
/// stops through so the raster backend can build the matching tiny-skia
/// shader. Gradient coordinates are in the node's local path space — the
/// same convention the PDF exporter uses — and the object translation is
/// applied later when the display list is built. `FillStyle::None` yields
/// no fill.
fn node_fill(node: &Node) -> Option<Paint> {
    use kcreate_core::node::{FillStyle, GradientKind};
    match &node.style.fill {
        FillStyle::Solid(rgba) => Some(Paint::Solid(rgba_to_color(*rgba))),
        FillStyle::Gradient(GradientKind::Linear { from, to, stops }) => {
            Some(Paint::LinearGradient {
                from: point2d_to_point2(*from),
                to: point2d_to_point2(*to),
                stops: gradient_stops_to_renderer(stops),
            })
        }
        FillStyle::Gradient(GradientKind::Radial {
            center,
            radius,
            stops,
        }) => Some(Paint::RadialGradient {
            center: point2d_to_point2(*center),
            radius: *radius as f32,
            stops: gradient_stops_to_renderer(stops),
        }),
        FillStyle::None => None,
    }
}

/// Deterministic per-peer colour for the presence cursor + label.
///
/// We hash the peer id (a base64url string in production) and pick
/// an HSL hue from the result; saturation and lightness are
/// fixed so every peer gets a saturated mid-tone that contrasts
/// against both white and dark canvas backgrounds. The mapping is
/// stable across sessions because it's a pure function of the
/// peer id — peers don't need to negotiate colours.
fn peer_color(peer_id: &str) -> Color {
    // Cheap, deterministic, non-cryptographic — same family as the
    // hash used in `kcreate_renderer::scene::next_id`. Wrapping is
    // intentional (we want the same colour every time we hash the
    // same string).
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in peer_id.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let hue = (hash % 360) as f32; // [0, 360)
    let saturation = 0.65;
    let lightness = 0.5;
    let (r, g, b) = hsl_to_rgb(hue, saturation, lightness);
    Color::rgba(r, g, b, 1.0)
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (f32, f32, f32) {
    if s == 0.0 {
        return (l, l, l);
    }
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let h_prime = h / 60.0;
    let x = c * (1.0 - (h_prime % 2.0 - 1.0).abs());
    let (r1, g1, b1) = match h_prime as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    (r1 + m, g1 + m, b1 + m)
}

fn node_style(node: &Node) -> Style {
    let fill = node_fill(node);
    let stroke = node.style.stroke.as_ref().map(|s| {
        Stroke::new(
            Color::rgba(s.color.r, s.color.g, s.color.b, s.color.a),
            s.width as f32,
        )
    });
    Style { fill, stroke }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kcreate_core::node::{
        Bounds, FillStyle, GradientKind, GradientStop, Node, NodeType, Point2D, RgbaColor,
        Transform2D,
    };
    use kcreate_vector::{PathPoint, PathSegment, VectorPath};

    fn unit_square_path() -> VectorPath {
        VectorPath::new(vec![
            PathSegment::MoveTo(PathPoint::new(0.0, 0.0)),
            PathSegment::LineTo(PathPoint::new(10.0, 0.0)),
            PathSegment::LineTo(PathPoint::new(10.0, 10.0)),
            PathSegment::LineTo(PathPoint::new(0.0, 10.0)),
            PathSegment::Close,
        ])
    }

    fn vector_node(path: &VectorPath) -> Node {
        let mut node = Node::new(NodeType::VectorLayer, "Rect");
        node.bounds = Bounds {
            x: 5.0,
            y: 7.0,
            width: 10.0,
            height: 10.0,
        };
        node.style.fill = FillStyle::Solid(RgbaColor {
            r: 1.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        });
        node.metadata.insert(
            VECTOR_PATH_METADATA_KEY.to_string(),
            serde_json::to_value(path).expect("serialise vector path"),
        );
        node
    }

    #[test]
    fn default_matches_new_allocator_state() {
        // `thumbnails.rs` builds an ephemeral `SceneSync::default()`
        // per render, so `default()` must produce the same allocator
        // state as `new()`. The derived `Default` would zero both
        // scalars — handing out the reserved `ObjectId(0)` sentinel for
        // the first node and starting `overlay_watermark` below
        // `OVERLAY_ID_THRESHOLD`.
        let from_default = SceneSync::default();
        let from_new = SceneSync::new();
        assert_eq!(
            from_default.next_id.load(Ordering::Relaxed),
            from_new.next_id.load(Ordering::Relaxed),
            "default() next_id must match new()",
        );
        assert_eq!(
            from_default.next_id.load(Ordering::Relaxed),
            1,
            "first document object must not be the ObjectId(0) sentinel",
        );
        assert_eq!(from_default.overlay_watermark, from_new.overlay_watermark);
        assert_eq!(from_default.overlay_watermark, OVERLAY_ID_THRESHOLD);

        // Behavioural tie-in: the first emitted document object from a
        // `default()`-constructed sync gets ObjectId(1), never 0.
        let mut doc = DocumentGraph::new();
        let path = unit_square_path();
        let id = doc.insert_node(vector_node(&path)).expect("insert");
        let mut sync = SceneSync::default();
        let _ = sync.sync_document_to_scene(&mut doc, None, &[]);
        let obj_id = sync.object_id_for_uuid(id).expect("forward lookup");
        assert_eq!(obj_id.0, 1, "first object id must be 1, not the sentinel 0");
    }

    #[test]
    fn vector_layer_becomes_path_object() {
        let mut doc = DocumentGraph::new();
        let path = unit_square_path();
        let node = vector_node(&path);
        let id = doc.insert_node(node).expect("insert");
        let mut sync = SceneSync::new();
        let scene = sync.sync_document_to_scene(&mut doc, None, &[]);
        assert_eq!(scene.objects.len(), 1);
        assert!(matches!(scene.objects[0].kind, ObjectKind::Path(_)));
        let obj_id = sync.object_id_for_uuid(id).expect("forward lookup");
        let back = sync.uuid_for_object_id(obj_id).expect("reverse lookup");
        assert_eq!(back, id);
    }

    #[test]
    fn linear_gradient_fill_maps_to_linear_paint() {
        let mut node = Node::new(NodeType::VectorLayer, "grad");
        node.style.fill = FillStyle::Gradient(GradientKind::Linear {
            from: Point2D::new(0.0, 0.0),
            to: Point2D::new(100.0, 0.0),
            stops: vec![
                GradientStop {
                    offset: 0.0,
                    color: RgbaColor {
                        r: 1.0,
                        g: 0.0,
                        b: 0.0,
                        a: 1.0,
                    },
                },
                GradientStop {
                    offset: 1.0,
                    color: RgbaColor {
                        r: 0.0,
                        g: 0.0,
                        b: 1.0,
                        a: 1.0,
                    },
                },
            ],
        });
        match node_fill(&node).expect("gradient produces a fill") {
            Paint::LinearGradient { from, to, stops } => {
                assert_eq!((from.x, from.y), (0.0, 0.0));
                assert_eq!((to.x, to.y), (100.0, 0.0));
                assert_eq!(stops.len(), 2);
                assert_eq!(stops[0], (0.0, Color::rgba(1.0, 0.0, 0.0, 1.0)));
                assert_eq!(stops[1], (1.0, Color::rgba(0.0, 0.0, 1.0, 1.0)));
            }
            other => panic!("expected a linear gradient paint, got {other:?}"),
        }
    }

    #[test]
    fn radial_gradient_fill_maps_to_radial_paint() {
        let mut node = Node::new(NodeType::VectorLayer, "grad");
        node.style.fill = FillStyle::Gradient(GradientKind::Radial {
            center: Point2D::new(50.0, 50.0),
            radius: 25.0,
            stops: vec![
                GradientStop {
                    offset: 0.0,
                    color: RgbaColor {
                        r: 1.0,
                        g: 1.0,
                        b: 1.0,
                        a: 1.0,
                    },
                },
                GradientStop {
                    offset: 1.0,
                    color: RgbaColor {
                        r: 0.0,
                        g: 0.0,
                        b: 0.0,
                        a: 1.0,
                    },
                },
            ],
        });
        match node_fill(&node).expect("gradient produces a fill") {
            Paint::RadialGradient {
                center,
                radius,
                stops,
            } => {
                assert_eq!((center.x, center.y), (50.0, 50.0));
                assert_eq!(radius, 25.0);
                assert_eq!(stops.len(), 2);
                assert_eq!(stops[0].1, Color::rgba(1.0, 1.0, 1.0, 1.0));
            }
            other => panic!("expected a radial gradient paint, got {other:?}"),
        }
    }

    #[test]
    fn gradient_vector_layer_carries_gradient_into_scene() {
        // End-to-end: a gradient-filled vector layer must arrive in the
        // renderer scene as a gradient Paint, not get dropped to a solid
        // or `None` the way it did before render-parity.
        let mut doc = DocumentGraph::new();
        let path = unit_square_path();
        let mut node = vector_node(&path);
        node.style.fill = FillStyle::Gradient(GradientKind::Linear {
            from: Point2D::new(0.0, 0.0),
            to: Point2D::new(10.0, 0.0),
            stops: vec![
                GradientStop {
                    offset: 0.0,
                    color: RgbaColor {
                        r: 0.0,
                        g: 1.0,
                        b: 0.0,
                        a: 1.0,
                    },
                },
                GradientStop {
                    offset: 1.0,
                    color: RgbaColor {
                        r: 0.0,
                        g: 0.0,
                        b: 1.0,
                        a: 1.0,
                    },
                },
            ],
        });
        doc.insert_node(node).expect("insert");
        let mut sync = SceneSync::new();
        let scene = sync.sync_document_to_scene(&mut doc, None, &[]);
        assert_eq!(scene.objects.len(), 1);
        assert!(
            matches!(
                scene.objects[0].style.fill,
                Some(Paint::LinearGradient { .. })
            ),
            "gradient fill must survive document→scene translation, got {:?}",
            scene.objects[0].style.fill
        );
    }

    #[test]
    fn text_gradient_fill_is_translated_into_world_space() {
        // Regression: a gradient-filled text layer must have its gradient
        // endpoints translated into world space so they line up with the
        // glyph outlines (which `draw_text` bakes at world coordinates).
        // Before the fix the gradient stayed in node-local space while the
        // glyphs sat at the node's world origin, so the shader sampled the
        // wrong region.
        let mut doc = DocumentGraph::new();
        let mut node = Node::new(NodeType::TextLayer, "headline");
        node.bounds = Bounds {
            x: 40.0,
            y: 80.0,
            width: 200.0,
            height: 60.0,
        };
        node.style.fill = FillStyle::Gradient(GradientKind::Linear {
            from: Point2D::new(0.0, 0.0),
            to: Point2D::new(200.0, 0.0),
            stops: vec![
                GradientStop {
                    offset: 0.0,
                    color: RgbaColor {
                        r: 1.0,
                        g: 0.0,
                        b: 0.0,
                        a: 1.0,
                    },
                },
                GradientStop {
                    offset: 1.0,
                    color: RgbaColor {
                        r: 0.0,
                        g: 0.0,
                        b: 1.0,
                        a: 1.0,
                    },
                },
            ],
        });
        node.metadata.insert(
            TEXT_LAYER_METADATA_KEY.to_string(),
            serde_json::to_value(TextLayerMeta {
                text: "Sunset".to_string(),
                font_family: "Inter".to_string(),
                font_size: 48.0,
            })
            .expect("serialise text meta"),
        );
        doc.insert_node(node).expect("insert");

        let mut sync = SceneSync::new();
        let scene = sync.sync_document_to_scene(&mut doc, None, &[]);
        assert_eq!(scene.objects.len(), 1);
        let obj = &scene.objects[0];
        assert!(
            matches!(obj.kind, ObjectKind::Text { .. }),
            "expected a text object, got {:?}",
            obj.kind
        );
        match &obj.style.fill {
            Some(Paint::LinearGradient { from, to, .. }) => {
                // World origin is bounds + transform (tx/ty default 0).
                assert_eq!(
                    (from.x, from.y),
                    (40.0, 80.0),
                    "gradient start must be offset to the text's world origin",
                );
                assert_eq!(
                    (to.x, to.y),
                    (240.0, 80.0),
                    "gradient end must track the world origin too",
                );
            }
            other => panic!("expected a linear gradient text fill, got {other:?}"),
        }
    }

    #[test]
    fn invisible_subtree_excluded() {
        let mut doc = DocumentGraph::new();
        let mut group = Node::new(NodeType::GroupLayer, "g");
        group.visible = false;
        let group_id = doc.insert_node(group).expect("group");
        let path = unit_square_path();
        let mut child = vector_node(&path);
        child.parent_id = Some(group_id);
        doc.insert_node(child).expect("child");
        let mut sync = SceneSync::new();
        let scene = sync.sync_document_to_scene(&mut doc, None, &[]);
        assert!(
            scene.objects.is_empty(),
            "hiding the group must hide everything beneath it"
        );
        assert!(sync.is_empty(), "no mapping should be recorded");
    }

    #[test]
    fn selection_highlight_appears_when_selected() {
        let mut doc = DocumentGraph::new();
        let path = unit_square_path();
        let node = vector_node(&path);
        let id = doc.insert_node(node).expect("insert");
        let mut sync = SceneSync::new();
        let scene = sync.sync_document_to_scene(&mut doc, None, &[id]);
        assert_eq!(
            scene.objects.len(),
            2,
            "vector object + 1 selection highlight"
        );
        let highlight = scene.objects.last().expect("highlight");
        assert!(
            highlight.style.stroke.is_some(),
            "selection highlights are stroked"
        );
        assert!(
            highlight.style.fill.is_none(),
            "selection highlights have no fill"
        );
    }

    #[test]
    fn object_ids_are_stable_across_resyncs() {
        let mut doc = DocumentGraph::new();
        let path = unit_square_path();
        let node = vector_node(&path);
        let id = doc.insert_node(node).expect("insert");
        let mut sync = SceneSync::new();
        let _ = sync.sync_document_to_scene(&mut doc, None, &[]);
        let first = sync.object_id_for_uuid(id).expect("first");
        let _ = sync.sync_document_to_scene(&mut doc, None, &[]);
        let second = sync.object_id_for_uuid(id).expect("second");
        assert_eq!(
            first, second,
            "syncing the same document twice must reuse the same ObjectId for the same uuid"
        );
    }

    // ---- Phase 11 perf-at-scale: no-op-document fast path ----

    #[test]
    fn fast_path_matches_full_rebuild() {
        let mut doc = DocumentGraph::new();
        let path = unit_square_path();
        doc.insert_node(vector_node(&path)).expect("insert n1");
        let mut n2 = vector_node(&path);
        n2.bounds = Bounds {
            x: 40.0,
            y: 20.0,
            width: 15.0,
            height: 25.0,
        };
        doc.insert_node(n2).expect("insert n2");

        let mut sync = SceneSync::new();
        // First sync: `content_cache` is `None`, so this is a full
        // rebuild — and it populates the cache.
        let full = sync.sync_document_to_scene(&mut doc, None, &[]);
        assert_eq!(
            sync.fast_path_hits, 0,
            "the first sync must be a full rebuild, not the fast path"
        );

        // Second sync with no edits: empty dirty set + cache present
        // → no-op fast path.
        let replayed = sync.sync_document_to_scene(&mut doc, None, &[]);
        assert_eq!(
            sync.fast_path_hits, 1,
            "an unchanged document must take the no-op fast path"
        );
        assert_eq!(
            full, replayed,
            "the fast-path scene must be identical to the full rebuild it replays"
        );

        // And it must also match an independent from-scratch rebuild
        // (fresh SceneSync via the always-full borrowed entry point),
        // proving the cache didn't drift from a real translation.
        let mut reference = SceneSync::new();
        let from_scratch = reference.sync_document_to_scene_borrowed(&doc, None, &[]);
        assert_eq!(
            replayed, from_scratch,
            "the fast-path scene must match an independent full rebuild"
        );
    }

    #[test]
    fn edit_after_cache_forces_full_rebuild() {
        let mut doc = DocumentGraph::new();
        let path = unit_square_path();
        let id = doc.insert_node(vector_node(&path)).expect("insert");
        let mut sync = SceneSync::new();
        // Populate the cache, then confirm an unedited resync replays it.
        let _ = sync.sync_document_to_scene(&mut doc, None, &[]);
        let cached = sync.sync_document_to_scene(&mut doc, None, &[]);
        assert_eq!(
            sync.fast_path_hits, 1,
            "the unchanged resync must take the fast path"
        );

        // Edit the node. `get_node_mut` marks it dirty, so the next
        // sync sees a non-empty dirty set and is forced through a full
        // rebuild instead of replaying the now-stale cache.
        {
            let node = doc.get_node_mut(id).expect("node");
            node.bounds.x = 250.0;
            node.bounds.y = 175.0;
            node.style.fill = FillStyle::Solid(RgbaColor {
                r: 0.0,
                g: 0.0,
                b: 1.0,
                a: 1.0,
            });
            node.touch();
        }
        let edited = sync.sync_document_to_scene(&mut doc, None, &[]);
        assert_eq!(
            sync.fast_path_hits, 1,
            "an edited document must NOT take the no-op fast path"
        );
        assert_ne!(
            edited, cached,
            "the rebuilt scene must reflect the edit, not replay stale cached content"
        );
    }

    #[test]
    fn selection_only_change_takes_fast_path() {
        let mut doc = DocumentGraph::new();
        let path = unit_square_path();
        let id = doc.insert_node(vector_node(&path)).expect("insert");
        let mut sync = SceneSync::new();
        // First sync (no selection): full rebuild → cache populated.
        let unselected = sync.sync_document_to_scene(&mut doc, None, &[]);
        assert_eq!(sync.fast_path_hits, 0);
        assert_eq!(unselected.objects.len(), 1, "just the vector object");

        // Selecting a node changes only `selection`, not the graph, so
        // this must take the fast path AND re-derive the highlight.
        let selected = sync.sync_document_to_scene(&mut doc, None, &[id]);
        assert_eq!(
            sync.fast_path_hits, 1,
            "a selection-only change must take the fast path"
        );
        assert_eq!(
            selected.objects.len(),
            2,
            "vector object + 1 re-derived selection highlight"
        );
        let highlight = selected.objects.last().expect("highlight");
        assert!(
            highlight.style.stroke.is_some() && highlight.style.fill.is_none(),
            "the fast-path highlight must be stroked with no fill"
        );

        // Clearing the selection (still no graph edit) stays on the
        // fast path and drops the highlight back to content-only.
        let cleared = sync.sync_document_to_scene(&mut doc, None, &[]);
        assert_eq!(
            sync.fast_path_hits, 2,
            "clearing the selection is also a no-op-document change"
        );
        assert_eq!(
            cleared.objects, unselected.objects,
            "clearing the selection must return to the content-only scene"
        );
    }

    #[test]
    fn blob_store_presence_transition_busts_fast_path() {
        // The fast path replays cached content verbatim and never
        // consults `blob_store`. Raster emit is the one content path
        // whose output depends on `blob_store` presence (real image vs.
        // placeholder rect), so a `None`<->`Some` transition between
        // syncs of an otherwise-unchanged document MUST force a full
        // rebuild — otherwise a stale placeholder (or stale image)
        // would persist. This guards the latent-correctness assumption
        // that both production callers always pass `Some`.
        let dir = tempfile::tempdir().expect("tempdir");
        let store = BlobStore::new(dir.path()).expect("blob store");

        let mut doc = DocumentGraph::new();
        let path = unit_square_path();
        doc.insert_node(vector_node(&path)).expect("insert");
        let mut sync = SceneSync::new();

        // First sync with no blob store: full rebuild, cache records
        // `blob_store_present == false`.
        let _ = sync.sync_document_to_scene(&mut doc, None, &[]);
        assert_eq!(sync.fast_path_hits, 0, "first sync is a full rebuild");

        // Resync of the unchanged document but now WITH a blob store:
        // the presence flag no longer matches, so the guard must fall
        // through to a full rebuild rather than replay the cache.
        let _ = sync.sync_document_to_scene(&mut doc, Some(&store), &[]);
        assert_eq!(
            sync.fast_path_hits, 0,
            "a None->Some blob_store transition must NOT take the fast path"
        );

        // With the cache now rebuilt under `Some`, an unchanged resync
        // that still supplies the blob store matches again and replays.
        let _ = sync.sync_document_to_scene(&mut doc, Some(&store), &[]);
        assert_eq!(
            sync.fast_path_hits, 1,
            "a matching Some blob_store resync takes the fast path"
        );

        // The reverse transition (Some -> None) must likewise rebuild.
        let _ = sync.sync_document_to_scene(&mut doc, None, &[]);
        assert_eq!(
            sync.fast_path_hits, 1,
            "a Some->None blob_store transition must NOT take the fast path"
        );

        // And a matching None resync replays once more.
        let _ = sync.sync_document_to_scene(&mut doc, None, &[]);
        assert_eq!(
            sync.fast_path_hits, 2,
            "a matching None blob_store resync takes the fast path"
        );
    }

    #[test]
    fn artboard_emits_shadow_background_and_label() {
        let mut doc = DocumentGraph::new();
        let mut art = Node::new(NodeType::Artboard, "Desktop");
        art.bounds = Bounds {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 50.0,
        };
        art.style.fill = FillStyle::Solid(RgbaColor {
            r: 0.9,
            g: 0.95,
            b: 1.0,
            a: 1.0,
        });
        let id = doc.insert_node(art).expect("art");
        let mut sync = SceneSync::new();
        let scene = sync.sync_document_to_scene(&mut doc, None, &[]);
        // Shadow + background + label = 3 scene objects.
        assert_eq!(scene.objects.len(), 3, "shadow + bg + label");

        // Shadow is first and uses an overlay (non-document) id.
        assert!(matches!(scene.objects[0].kind, ObjectKind::Rect(_)));
        assert!(is_overlay_id(scene.objects[0].id));
        // Background is second, hit-testable (document id), and the
        // forward map points at it.
        let bg = &scene.objects[1];
        match &bg.kind {
            ObjectKind::Rect(r) => {
                assert_eq!(r.width, 100.0);
                assert_eq!(r.height, 50.0);
            }
            other => panic!("expected background rect, got {other:?}"),
        }
        assert!(!is_overlay_id(bg.id));
        let mapped = sync.object_id_for_uuid(id).expect("forward lookup");
        assert_eq!(mapped, bg.id);

        // Label is third: a Text overlay positioned above the
        // artboard with the artboard name.
        match &scene.objects[2].kind {
            ObjectKind::Text { text, origin, .. } => {
                assert_eq!(text, "Desktop");
                assert!(origin.y < 0.0, "label should sit above top edge");
            }
            other => panic!("expected name label, got {other:?}"),
        }
        assert!(is_overlay_id(scene.objects[2].id));
    }

    #[test]
    fn artboard_without_name_skips_label() {
        let mut doc = DocumentGraph::new();
        let mut art = Node::new(NodeType::Artboard, "");
        art.bounds = Bounds {
            x: 0.0,
            y: 0.0,
            width: 50.0,
            height: 50.0,
        };
        doc.insert_node(art).expect("art");
        let mut sync = SceneSync::new();
        let scene = sync.sync_document_to_scene(&mut doc, None, &[]);
        // Shadow + background only, no label.
        assert_eq!(scene.objects.len(), 2);
    }

    #[test]
    fn child_entirely_outside_artboard_is_clipped() {
        let mut doc = DocumentGraph::new();
        let mut art = Node::new(NodeType::Artboard, "Page");
        art.bounds = Bounds {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 100.0,
        };
        let art_id = doc.insert_node(art).expect("art");

        // Child INSIDE the artboard (intersects clip rect).
        let path = unit_square_path();
        let mut inside = vector_node(&path);
        inside.bounds = Bounds {
            x: 10.0,
            y: 10.0,
            width: 20.0,
            height: 20.0,
        };
        let inside_id = doc.insert_node(inside).expect("inside");
        doc.reparent_node(inside_id, Some(art_id), 0)
            .expect("attach inside");

        // Child fully OUTSIDE the artboard (right of it, no overlap).
        let mut outside = vector_node(&path);
        outside.bounds = Bounds {
            x: 1000.0,
            y: 1000.0,
            width: 20.0,
            height: 20.0,
        };
        let outside_id = doc.insert_node(outside).expect("outside");
        doc.reparent_node(outside_id, Some(art_id), 1)
            .expect("attach outside");

        let mut sync = SceneSync::new();
        let scene = sync.sync_document_to_scene(&mut doc, None, &[]);
        // Shadow + bg + label + inside child = 4 (outside pruned).
        assert_eq!(scene.objects.len(), 4);
        // The pruned child must not have a mapping.
        assert!(sync.object_id_for_uuid(outside_id).is_none());
        // The inside child must.
        assert!(sync.object_id_for_uuid(inside_id).is_some());
    }

    #[test]
    fn bounds_overlap_reports_intersection_correctly() {
        let a = Bounds {
            x: 0.0,
            y: 0.0,
            width: 10.0,
            height: 10.0,
        };
        let touches_corner = Bounds {
            x: 9.0,
            y: 9.0,
            width: 5.0,
            height: 5.0,
        };
        let just_outside = Bounds {
            x: 10.0,
            y: 0.0,
            width: 5.0,
            height: 5.0,
        };
        let far_away = Bounds {
            x: 100.0,
            y: 100.0,
            width: 5.0,
            height: 5.0,
        };
        assert!(bounds_overlap(&a, &touches_corner));
        assert!(!bounds_overlap(&a, &just_outside));
        assert!(!bounds_overlap(&a, &far_away));
    }

    #[test]
    fn translation_applied_to_vector_layer() {
        let mut doc = DocumentGraph::new();
        let path = unit_square_path();
        let mut node = vector_node(&path);
        node.transform = Transform2D {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            tx: 50.0,
            ty: 75.0,
        };
        doc.insert_node(node).expect("insert");
        let mut sync = SceneSync::new();
        let scene = sync.sync_document_to_scene(&mut doc, None, &[]);
        let obj = &scene.objects[0];
        assert_eq!(obj.translation, (50.0, 75.0));
    }

    /// Regression: presence cursor overlay IDs must not collide with
    /// artboard overlay IDs (both streams previously restarted at
    /// `OVERLAY_ID_THRESHOLD`, so a scene with N artboard overlays
    /// would emit N pairs of identical `ObjectId`s — one in the
    /// artboard set, one in the cursor set).
    #[test]
    fn presence_cursor_ids_do_not_collide_with_artboard_overlays() {
        let mut doc = DocumentGraph::new();
        // Two named artboards, each contributing shadow + label = 4
        // artboard overlays in total. (The artboard background itself
        // gets a document-backed id, not an overlay id.)
        for (i, name) in ["A", "B"].iter().enumerate() {
            let mut art = Node::new(NodeType::Artboard, *name);
            art.bounds = Bounds {
                x: (i as f64) * 200.0,
                y: 0.0,
                width: 100.0,
                height: 100.0,
            };
            doc.insert_node(art).expect("insert artboard");
        }

        let mut sync = SceneSync::new();
        let mut scene = sync.sync_document_to_scene(&mut doc, None, &[]);

        // Snapshot the overlay-id set emitted by the document walk.
        let pre_cursor_overlay_ids: std::collections::HashSet<ObjectId> = scene
            .objects
            .iter()
            .map(|o| o.id)
            .filter(|id| is_overlay_id(*id))
            .collect();
        assert!(
            !pre_cursor_overlay_ids.is_empty(),
            "two named artboards should emit at least one overlay id each"
        );

        // Append two presence cursors with display names (each cursor
        // emits a triangle + label = 2 overlay ids).
        let cursors = vec![
            PresenceCursor {
                peer_id: "peer-1".into(),
                display_name: "Alice".into(),
                x: 10.0,
                y: 10.0,
            },
            PresenceCursor {
                peer_id: "peer-2".into(),
                display_name: "Bob".into(),
                x: 50.0,
                y: 50.0,
            },
        ];
        sync.append_presence_cursors(&mut scene, &cursors, 0, 1.0);

        // Every overlay id in the final scene must be unique. The
        // pre-cursor and post-cursor overlay sets must be disjoint.
        let all_overlay_ids: Vec<ObjectId> = scene
            .objects
            .iter()
            .map(|o| o.id)
            .filter(|id| is_overlay_id(*id))
            .collect();
        let unique_overlay_ids: std::collections::HashSet<ObjectId> =
            all_overlay_ids.iter().copied().collect();
        assert_eq!(
            all_overlay_ids.len(),
            unique_overlay_ids.len(),
            "overlay-id collision: artboard overlays and presence cursor overlays both restarted from the same base. ids = {all_overlay_ids:?}"
        );

        let post_cursor_overlay_ids: std::collections::HashSet<ObjectId> = unique_overlay_ids
            .difference(&pre_cursor_overlay_ids)
            .copied()
            .collect();
        assert_eq!(
            post_cursor_overlay_ids.len(),
            4,
            "two cursors with names should emit 4 new overlay ids (triangle + label each)"
        );
        assert!(
            pre_cursor_overlay_ids.is_disjoint(&post_cursor_overlay_ids),
            "presence cursor ids must not overlap with artboard overlay ids"
        );
    }

    /// Cursor geometry must be scaled by `1 / viewport_zoom` so it
    /// stays a constant on-screen size as the user zooms.
    #[test]
    fn presence_cursor_geometry_scales_inversely_with_viewport_zoom() {
        let mut sync = SceneSync::new();
        let cursors = vec![PresenceCursor {
            peer_id: "peer-1".into(),
            display_name: "Alice".into(),
            x: 0.0,
            y: 0.0,
        }];

        let mut scene_1x = Scene::new(DEFAULT_CLEAR);
        sync.append_presence_cursors(&mut scene_1x, &cursors, 0, 1.0);
        let mut scene_2x = Scene::new(DEFAULT_CLEAR);
        sync.append_presence_cursors(&mut scene_2x, &cursors, 0, 2.0);

        // The triangle path's longest edge encodes the cursor width.
        // At 2x zoom, the cursor should be half as wide in world
        // units (so it stays the same on-screen pixel size).
        let width_at = |scene: &Scene| match &scene.objects[0].kind {
            ObjectKind::Path(cmds) => match cmds.get(1) {
                Some(PathCommand::LineTo(p)) => p.x,
                other => panic!("expected LineTo at index 1, got {other:?}"),
            },
            other => panic!("expected Path cursor, got {other:?}"),
        };
        let w1 = width_at(&scene_1x);
        let w2 = width_at(&scene_2x);
        assert!(
            (w1 - 2.0 * w2).abs() < 1e-4,
            "cursor at 2x zoom should be half the world-space width of cursor at 1x: w1 = {w1}, w2 = {w2}"
        );

        // The label's font_size should also scale inversely.
        let font_at = |scene: &Scene| match &scene.objects[1].kind {
            ObjectKind::Text { font_size, .. } => *font_size,
            other => panic!("expected Text label, got {other:?}"),
        };
        let f1 = font_at(&scene_1x);
        let f2 = font_at(&scene_2x);
        assert!(
            (f1 - 2.0 * f2).abs() < 1e-4,
            "label font_size at 2x zoom should be half: f1 = {f1}, f2 = {f2}"
        );
    }

    /// Selection halos must paint one stroke rect per selected
    /// node, plus exactly one peer-name label per peer (anchored to
    /// the first node — extra nodes don't duplicate the label).
    /// Invisible / unknown ids are silently dropped.
    #[test]
    fn presence_selection_halos_emit_per_node_with_one_label_per_peer() {
        let mut doc = DocumentGraph::new();
        let vp = unit_square_path();
        let mut visible_a = vector_node(&vp);
        visible_a.name = "A".into();
        visible_a.bounds = Bounds {
            x: 0.0,
            y: 0.0,
            width: 10.0,
            height: 10.0,
        };
        let mut visible_b = vector_node(&vp);
        visible_b.name = "B".into();
        visible_b.bounds = Bounds {
            x: 30.0,
            y: 0.0,
            width: 10.0,
            height: 10.0,
        };
        let mut hidden = vector_node(&vp);
        hidden.name = "hidden".into();
        hidden.visible = false;
        let id_a = visible_a.id;
        let id_b = visible_b.id;
        let id_hidden = hidden.id;
        doc.insert_node(visible_a).expect("a");
        doc.insert_node(visible_b).expect("b");
        doc.insert_node(hidden).expect("hidden");

        let mut sync = SceneSync::new();
        let mut scene = sync.sync_document_to_scene(&mut doc, None, &[]);
        let pre_overlay_ids: std::collections::HashSet<ObjectId> = scene
            .objects
            .iter()
            .map(|o| o.id)
            .filter(|id| is_overlay_id(*id))
            .collect();

        let selections = vec![PresenceSelection {
            peer_id: "peer-1".into(),
            display_name: "Alice".into(),
            // Visible-A then visible-B then hidden then dangling.
            node_ids: vec![id_a, id_b, id_hidden, Uuid::new_v4()],
        }];
        sync.append_presence_selection_halos(&mut scene, &doc, &selections, 0, 1.0);

        let post_overlay_ids: std::collections::HashSet<ObjectId> = scene
            .objects
            .iter()
            .map(|o| o.id)
            .filter(|id| is_overlay_id(*id))
            .collect();
        let added: Vec<ObjectId> = post_overlay_ids
            .difference(&pre_overlay_ids)
            .copied()
            .collect();
        // 2 visible nodes → 2 halo rects + 1 label (anchored to the first node).
        assert_eq!(
            added.len(),
            3,
            "expected 2 halo rects + 1 peer-name label, got {} new overlays",
            added.len()
        );
        // The label is the only Text object among the new overlays.
        let label_count = scene
            .objects
            .iter()
            .filter(|o| post_overlay_ids.contains(&o.id) && !pre_overlay_ids.contains(&o.id))
            .filter(|o| matches!(o.kind, ObjectKind::Text { .. }))
            .count();
        assert_eq!(label_count, 1, "peer label should be emitted exactly once");
    }

    /// Regression test for the label-emission guard: if the first
    /// id in a peer's `node_ids` list happens to be invisible or
    /// missing from the document, the label must still attach to
    /// the *first rendered* halo (not be silently dropped). The
    /// earlier `Some(node_id) == selection.node_ids.first()` check
    /// failed this case because the visible id never matched
    /// `first()`.
    #[test]
    fn selection_halo_label_anchors_to_first_rendered_when_first_id_invisible() {
        let mut doc = DocumentGraph::new();
        let vp = unit_square_path();
        let mut hidden = vector_node(&vp);
        hidden.name = "hidden".into();
        hidden.visible = false;
        hidden.bounds = Bounds {
            x: 0.0,
            y: 0.0,
            width: 10.0,
            height: 10.0,
        };
        let mut visible = vector_node(&vp);
        visible.name = "visible".into();
        visible.bounds = Bounds {
            x: 30.0,
            y: 0.0,
            width: 10.0,
            height: 10.0,
        };
        let id_hidden = hidden.id;
        let id_visible = visible.id;
        doc.insert_node(hidden).expect("hidden");
        doc.insert_node(visible).expect("visible");

        let mut sync = SceneSync::new();
        let mut scene = sync.sync_document_to_scene(&mut doc, None, &[]);
        let pre_overlay_ids: std::collections::HashSet<ObjectId> = scene
            .objects
            .iter()
            .map(|o| o.id)
            .filter(|id| is_overlay_id(*id))
            .collect();

        let selections = vec![PresenceSelection {
            peer_id: "peer-1".into(),
            display_name: "Alice".into(),
            // Hidden id FIRST, visible id second, dangling id last.
            // The earlier guard would have dropped the label.
            node_ids: vec![id_hidden, id_visible, Uuid::new_v4()],
        }];
        sync.append_presence_selection_halos(&mut scene, &doc, &selections, 0, 1.0);

        let post_overlay_ids: std::collections::HashSet<ObjectId> = scene
            .objects
            .iter()
            .map(|o| o.id)
            .filter(|id| is_overlay_id(*id))
            .collect();
        let added: std::collections::HashSet<ObjectId> = post_overlay_ids
            .difference(&pre_overlay_ids)
            .copied()
            .collect();
        // 1 visible node → 1 halo rect + 1 label = 2 overlays.
        assert_eq!(
            added.len(),
            2,
            "expected 1 halo rect + 1 peer-name label, got {} new overlays",
            added.len()
        );
        let label_count = scene
            .objects
            .iter()
            .filter(|o| added.contains(&o.id))
            .filter(|o| matches!(o.kind, ObjectKind::Text { .. }))
            .count();
        assert_eq!(
            label_count, 1,
            "peer label must still emit even when the first node id is invisible"
        );
    }

    /// Halos must paint *below* presence cursors so the cursor
    /// stays the most prominent indicator of "where the peer is
    /// right now". The previous implementation used a hard-coded
    /// `cursor_z = halo_z + 1` gap that broke as soon as halos
    /// emitted more than one object — exactly the case when even a
    /// single peer with a display name selects one node (rect at z,
    /// label at z+1). The fix threads the post-halo z out of
    /// `append_presence_selection_halos` so the caller can put
    /// cursors strictly above whatever the halo path actually
    /// produced.
    #[test]
    fn presence_cursor_z_is_strictly_above_every_halo_z() {
        let mut doc = DocumentGraph::new();
        let vp = unit_square_path();
        let mut visible_a = vector_node(&vp);
        visible_a.bounds = Bounds {
            x: 0.0,
            y: 0.0,
            width: 10.0,
            height: 10.0,
        };
        let mut visible_b = vector_node(&vp);
        visible_b.bounds = Bounds {
            x: 30.0,
            y: 0.0,
            width: 10.0,
            height: 10.0,
        };
        let id_a = visible_a.id;
        let id_b = visible_b.id;
        doc.insert_node(visible_a).expect("a");
        doc.insert_node(visible_b).expect("b");

        let mut sync = SceneSync::new();
        let mut scene = sync.sync_document_to_scene(&mut doc, None, &[]);
        let pre_overlay_ids: std::collections::HashSet<ObjectId> = scene
            .objects
            .iter()
            .map(|o| o.id)
            .filter(|id| is_overlay_id(*id))
            .collect();

        let halo_starting_z = 1000;
        let selections = vec![PresenceSelection {
            peer_id: "peer-1".into(),
            display_name: "Alice".into(),
            node_ids: vec![id_a, id_b],
        }];
        let halo_next_z = sync.append_presence_selection_halos(
            &mut scene,
            &doc,
            &selections,
            halo_starting_z,
            1.0,
        );
        // 2 halos + 1 label = 3 z slots consumed, so next free z
        // must be `starting + 3`.
        assert_eq!(
            halo_next_z,
            halo_starting_z + 3,
            "halo should consume 3 z slots (2 rects + 1 label) and return next free"
        );

        let cursors = vec![PresenceCursor {
            peer_id: "peer-1".into(),
            display_name: "Alice".into(),
            x: 5.0,
            y: 5.0,
        }];
        sync.append_presence_cursors(&mut scene, &cursors, halo_next_z, 1.0);

        let halo_max_z = scene
            .objects
            .iter()
            .filter(|o| is_overlay_id(o.id) && !pre_overlay_ids.contains(&o.id))
            // Halo rects + label live in the range [halo_starting_z, halo_next_z).
            .filter(|o| o.z < halo_next_z)
            .map(|o| o.z)
            .max()
            .expect("at least one halo emitted");
        let cursor_min_z = scene
            .objects
            .iter()
            .filter(|o| is_overlay_id(o.id) && !pre_overlay_ids.contains(&o.id))
            // Cursor objects start at `halo_next_z`.
            .filter(|o| o.z >= halo_next_z)
            .map(|o| o.z)
            .min()
            .expect("at least one cursor object emitted");
        assert!(
            cursor_min_z > halo_max_z,
            "cursor min z ({cursor_min_z}) must be strictly above halo max z ({halo_max_z})"
        );
    }

    /// Halo overlay ids must continue the same upward stream that
    /// `sync_document_to_scene` and `append_presence_cursors` use —
    /// restarting at `OVERLAY_ID_THRESHOLD` would let halos collide
    /// with either of those.
    #[test]
    fn selection_halo_ids_do_not_collide_with_cursors_or_artboards() {
        let mut doc = DocumentGraph::new();
        let mut art = Node::new(NodeType::Artboard, "A");
        art.bounds = Bounds {
            x: 0.0,
            y: 0.0,
            width: 50.0,
            height: 50.0,
        };
        doc.insert_node(art).expect("artboard");
        let vp = unit_square_path();
        let mut child = vector_node(&vp);
        child.bounds = Bounds {
            x: 5.0,
            y: 5.0,
            width: 10.0,
            height: 10.0,
        };
        let child_id = child.id;
        doc.insert_node(child).expect("child");

        let mut sync = SceneSync::new();
        let mut scene = sync.sync_document_to_scene(&mut doc, None, &[]);
        let cursors = vec![PresenceCursor {
            peer_id: "peer-1".into(),
            display_name: "Alice".into(),
            x: 5.0,
            y: 5.0,
        }];
        sync.append_presence_cursors(&mut scene, &cursors, 1000, 1.0);
        let selections = vec![PresenceSelection {
            peer_id: "peer-1".into(),
            display_name: "Alice".into(),
            node_ids: vec![child_id],
        }];
        sync.append_presence_selection_halos(&mut scene, &doc, &selections, 999, 1.0);

        // Every overlay id in the scene must be unique.
        let overlay_ids: Vec<ObjectId> = scene
            .objects
            .iter()
            .map(|o| o.id)
            .filter(|id| is_overlay_id(*id))
            .collect();
        let unique: std::collections::HashSet<ObjectId> = overlay_ids.iter().copied().collect();
        assert_eq!(
            overlay_ids.len(),
            unique.len(),
            "overlay-id collision between halos / cursors / artboard chrome: {overlay_ids:?}"
        );
    }

    /// Halo stroke width + label font size must scale inversely with
    /// `viewport_zoom`, matching the cursor behaviour. The same
    /// clamp applies at very small / zero / negative zooms.
    #[test]
    fn presence_selection_halo_scales_inversely_with_viewport_zoom() {
        let mut doc = DocumentGraph::new();
        let vp = unit_square_path();
        let mut node = vector_node(&vp);
        node.bounds = Bounds {
            x: 0.0,
            y: 0.0,
            width: 10.0,
            height: 10.0,
        };
        let id = node.id;
        doc.insert_node(node).expect("node");

        let mut sync_1x = SceneSync::new();
        let mut scene_1x = sync_1x.sync_document_to_scene(&mut doc, None, &[]);
        let mut sync_2x = SceneSync::new();
        let mut scene_2x = sync_2x.sync_document_to_scene(&mut doc, None, &[]);

        let selections = vec![PresenceSelection {
            peer_id: "peer-1".into(),
            display_name: String::new(),
            node_ids: vec![id],
        }];
        sync_1x.append_presence_selection_halos(&mut scene_1x, &doc, &selections, 0, 1.0);
        sync_2x.append_presence_selection_halos(&mut scene_2x, &doc, &selections, 0, 2.0);

        let stroke_width = |scene: &Scene| -> f32 {
            scene
                .objects
                .iter()
                .find_map(|o| match (&o.kind, &o.style.stroke) {
                    (ObjectKind::Rect(_), Some(s)) if is_overlay_id(o.id) => Some(s.width),
                    _ => None,
                })
                .expect("halo rect with stroke")
        };
        let w1 = stroke_width(&scene_1x);
        let w2 = stroke_width(&scene_2x);
        assert!(
            (w1 - 2.0 * w2).abs() < 1e-4,
            "halo stroke at 2x zoom should be half world width: w1 = {w1}, w2 = {w2}"
        );

        // Pathological zoom: just confirm finite, bounded output.
        let mut sync_bad = SceneSync::new();
        let mut scene_bad = sync_bad.sync_document_to_scene(&mut doc, None, &[]);
        sync_bad.append_presence_selection_halos(&mut scene_bad, &doc, &selections, 0, -1.0);
        let wb = stroke_width(&scene_bad);
        assert!(
            wb.is_finite() && wb < 200.0,
            "pathological zoom: stroke = {wb}"
        );
    }

    /// `append_presence_cursors` clamps very small / zero / negative
    /// zooms to [`CURSOR_MIN_VIEWPORT_ZOOM`] so cursors don't explode
    /// or blow up to NaN. Whether we pass 0.0 or a negative value,
    /// the resulting cursor geometry must be finite and bounded.
    #[test]
    fn presence_cursor_clamps_pathological_viewport_zoom() {
        let mut sync = SceneSync::new();
        let cursors = vec![PresenceCursor {
            peer_id: "peer-1".into(),
            display_name: String::new(),
            x: 0.0,
            y: 0.0,
        }];

        for bad_zoom in [0.0, -1.0, f32::EPSILON / 1000.0] {
            let mut scene = Scene::new(DEFAULT_CLEAR);
            sync.append_presence_cursors(&mut scene, &cursors, 0, bad_zoom);
            assert_eq!(scene.objects.len(), 1, "exactly one cursor triangle");
            match &scene.objects[0].kind {
                ObjectKind::Path(cmds) => {
                    for cmd in cmds {
                        if let PathCommand::LineTo(p) | PathCommand::MoveTo(p) = cmd {
                            assert!(
                                p.x.is_finite() && p.y.is_finite(),
                                "cursor at zoom {bad_zoom} produced non-finite vertex {p:?}"
                            );
                            // CURSOR_WIDTH / CURSOR_MIN_VIEWPORT_ZOOM = 14 / 0.05 = 280
                            assert!(
                                p.x.abs() <= 400.0,
                                "cursor at zoom {bad_zoom} produced runaway vertex x = {}",
                                p.x
                            );
                        }
                    }
                }
                other => panic!("expected path cursor, got {other:?}"),
            }
        }
    }

    #[test]
    fn dash_subpaths_receive_unique_object_ids() {
        // Regression for Devin Review BUG_..._0002: each sub-path
        // produced by a Dash path effect must carry a distinct
        // `ObjectId` so the scene's `Vec<Object>` doesn't end up
        // with several entries sharing one id (which would break
        // any future per-object incremental scene patching).
        // Every sub-id must still reverse-lookup to the parent
        // node uuid, so hit-testing keeps working unchanged.
        use kcreate_core::node::PathEffect;

        let mut doc = DocumentGraph::new();
        // 100-unit horizontal line, dashed 10-on/10-off → multiple
        // sub-paths (kcreate_vector::dash exact count tested
        // independently; we just need "more than 1").
        let path = VectorPath::new(vec![
            PathSegment::MoveTo(PathPoint::new(0.0, 0.0)),
            PathSegment::LineTo(PathPoint::new(100.0, 0.0)),
        ]);
        let mut node = vector_node(&path);
        node.style.path_effects.push(PathEffect::Dash {
            pattern: vec![10.0, 10.0],
            offset: 0.0,
        });
        let id = doc.insert_node(node).expect("insert");

        let mut sync = SceneSync::new();
        let scene = sync.sync_document_to_scene(&mut doc, None, &[]);

        assert!(
            scene.objects.len() >= 2,
            "dash effect must emit at least 2 sub-path objects, got {}",
            scene.objects.len()
        );
        let mut seen: std::collections::HashSet<ObjectId> =
            std::collections::HashSet::with_capacity(scene.objects.len());
        for obj in &scene.objects {
            assert!(
                seen.insert(obj.id),
                "dash sub-path ObjectId {:?} appeared twice in the scene",
                obj.id
            );
            // Every sub-id must reverse-lookup to the parent node.
            let parent = sync.uuid_for_object_id(obj.id).expect("reverse map");
            assert_eq!(
                parent, id,
                "sub-path id {:?} did not reverse-map back to parent node",
                obj.id,
            );
        }
        // The forward map still points at exactly one primary id,
        // which must be one of the emitted object ids.
        let primary = sync.object_id_for_uuid(id).expect("forward map");
        assert!(
            seen.contains(&primary),
            "primary ObjectId {primary:?} was not one of the emitted sub-path ids",
        );
    }

    // -----------------------------------------------------------
    // Phase 10 Block E Task 26 — incremental scene-cache tests.
    // -----------------------------------------------------------

    /// Helper: build a scene with three sibling vector layers, run a
    /// fresh sync, then return `(doc, sync, scene, ids)`.
    fn three_vector_scene() -> (DocumentGraph, SceneSync, Scene, Vec<Uuid>) {
        let mut doc = DocumentGraph::new();
        let path = unit_square_path();
        let ids: Vec<Uuid> = (0..3)
            .map(|i| {
                let mut n = vector_node(&path);
                n.name = format!("rect_{i}");
                n.bounds.x = f64::from(i) * 20.0;
                doc.insert_node(n).expect("insert")
            })
            .collect();
        let mut sync = SceneSync::new();
        let scene = sync.sync_document_to_scene(&mut doc, None, &[]);
        (doc, sync, scene, ids)
    }

    #[test]
    fn cache_populated_after_first_sync() {
        let (_doc, sync, _scene, ids) = three_vector_scene();
        // 3 vector nodes → 3 cache entries.
        assert_eq!(sync.cached_node_count(), ids.len());
    }

    #[test]
    fn second_sync_with_no_changes_reuses_cache_and_matches_full_rebuild() {
        let (mut doc, mut cached_sync, scene_a, _ids) = three_vector_scene();
        // Replay against the same doc; nothing has changed so the
        // cache should be 100% hit and the resulting scene must be
        // bit-identical to a fresh-from-empty sync.
        let scene_b = cached_sync.sync_document_to_scene(&mut doc, None, &[]);
        let mut fresh = SceneSync::new();
        let scene_c = fresh.sync_document_to_scene(&mut doc, None, &[]);
        // Both syncs from the same doc state must produce the same
        // object count and per-object kinds (Path).
        assert_eq!(scene_a.objects.len(), scene_b.objects.len());
        assert_eq!(scene_b.objects.len(), scene_c.objects.len());
        for (b, c) in scene_b.objects.iter().zip(scene_c.objects.iter()) {
            assert_eq!(b.z, c.z, "cached replay z must match fresh sync z");
        }
    }

    #[test]
    fn version_bump_invalidates_cache_entry_for_that_node() {
        let (mut doc, mut sync, _scene_a, ids) = three_vector_scene();
        let cache_before = sync.cached_node_count();
        // Bump the middle node's version (simulates a property edit).
        if let Some(n) = doc.get_node_mut(ids[1]) {
            n.touch();
        }
        // Re-sync — the cache entry for ids[1] should regenerate but
        // entries for ids[0] and ids[2] should replay unchanged.
        let scene = sync.sync_document_to_scene(&mut doc, None, &[]);
        assert_eq!(scene.objects.len(), 3, "still three vector objects");
        assert_eq!(
            sync.cached_node_count(),
            cache_before,
            "cache count unchanged: one entry replaced, two reused"
        );
    }

    #[test]
    fn deleted_node_drops_cache_entry() {
        let (mut doc, mut sync, _scene_a, ids) = three_vector_scene();
        let before = sync.cached_node_count();
        doc.remove_node(ids[0]).expect("remove first node");
        let scene = sync.sync_document_to_scene(&mut doc, None, &[]);
        assert_eq!(scene.objects.len(), 2, "two objects remain");
        assert_eq!(
            sync.cached_node_count(),
            before - 1,
            "cache must drop the deleted node",
        );
    }

    #[test]
    fn cached_replay_preserves_object_id_stability() {
        let (mut doc, mut sync, scene_a, ids) = three_vector_scene();
        let id_a: Vec<_> = scene_a.objects.iter().map(|o| o.id).collect();
        // Second sync with no changes — every emitted object id must
        // match the first sync's, since the cache replays the exact
        // ObjectId values it captured.
        let scene_b = sync.sync_document_to_scene(&mut doc, None, &[]);
        let id_b: Vec<_> = scene_b.objects.iter().map(|o| o.id).collect();
        assert_eq!(id_a, id_b, "cached replay must yield identical ObjectIds");
        // Forward map still resolves every node uuid.
        for id in &ids {
            assert!(
                sync.object_id_for_uuid(*id).is_some(),
                "uuid {id} dropped from forward map after cached sync",
            );
        }
    }

    /// Phase E regression pin — Target 1 fix. The reverse map
    /// rebuild in `sync_document_to_scene_inner` used to do a
    /// wasteful `mem::take` + `HashMap::clone()` of the forward
    /// map just to satisfy the borrow checker. The Phase E fix
    /// replaces that with a direct split-borrow iteration over
    /// `&self.uuid_to_object_id` while inserting into
    /// `&mut self.object_id_to_uuid`. This test pins the
    /// observable invariant: after any sync, EVERY entry in the
    /// forward map must round-trip through the reverse map. If a
    /// future refactor drops the rebuild loop (or rebuilds it
    /// incorrectly — e.g. by mistakenly inserting `(uuid, obj_id)`
    /// instead of `(obj_id, uuid)`), this test pops.
    #[test]
    fn forward_and_reverse_maps_stay_in_lockstep_after_sync() {
        let (mut doc, mut sync, _scene, _ids) = three_vector_scene();
        // Run a second sync — this exercises the rebuild path that
        // the cold-start sync also takes on the first call. The
        // forward map persists; the reverse map is regenerated
        // from it.
        let _ = sync.sync_document_to_scene(&mut doc, None, &[]);
        // Every forward entry must appear in the reverse map with
        // the inverse key/value.
        let forward = sync
            .uuid_to_object_id
            .iter()
            .map(|(u, o)| (*u, *o))
            .collect::<Vec<_>>();
        assert!(
            !forward.is_empty(),
            "precondition: forward map must be populated after a sync of a non-empty doc",
        );
        for (uuid, obj_id) in &forward {
            assert_eq!(
                sync.uuid_for_object_id(*obj_id),
                Some(*uuid),
                "forward entry (uuid={uuid}, obj_id={obj_id:?}) missing from reverse map \
                 — Phase E reverse-map rebuild regressed",
            );
        }
        // And conversely: every reverse entry must point at a
        // uuid that still exists in the forward map. We CANNOT
        // assert `reverse[obj_id] == uuid => forward[uuid] ==
        // obj_id` here because the reverse map also stores
        // sub-object ids that point back at their parent node's
        // uuid (see `allocate_sub_object_id` at scene_sync.rs:394
        // and the sub-object loop in the replay path at
        // scene_sync.rs:477-479). For a node with N sub-objects
        // the forward map has 1 entry (primary id) while the
        // reverse map has N entries (one per sub-id, all
        // mapping to the same parent uuid). The fixture used
        // here (`three_vector_scene`) happens to produce 1
        // object per node so cardinality would coincidentally
        // match, but pinning a strict `reverse.len() ==
        // forward.len()` would create a false-positive failure
        // the day a future refactor adds a path effect or other
        // sub-object emitter to the fixture. The actual
        // observable invariant is the weaker "every reverse
        // entry's uuid is live in the forward map" — drops
        // catch the stale-entry regression the strict check
        // was supposed to catch, without breaking under
        // legitimate fixture evolution.
        let reverse = sync
            .object_id_to_uuid
            .iter()
            .map(|(o, u)| (*o, *u))
            .collect::<Vec<_>>();
        assert!(
            !reverse.is_empty(),
            "precondition: reverse map must be populated after a sync of a non-empty doc",
        );
        for (obj_id, uuid) in &reverse {
            assert!(
                sync.uuid_to_object_id.contains_key(uuid),
                "reverse entry (obj_id={obj_id:?}, uuid={uuid}) points at a uuid that is \
                 no longer present in the forward map — stale reverse entry, Phase E \
                 reverse-map rebuild regressed",
            );
        }
    }

    /// Phase E regression pin — Target 2 fix. The cache-replay
    /// path used to clone both `entry.objects` and
    /// `entry.sub_object_ids` purely to end the borrow on
    /// `node_cache` before mutating the forward/reverse id maps.
    /// The Phase E fix removes the `entry.objects` clone and
    /// pushes objects by reference + clone-on-push instead. This
    /// test pins the load-bearing post-loop behaviour the clone
    /// existed to support: after a cache-only sync (every node
    /// hits `try_replay_cached`), the forward map must point
    /// every node uuid at its primary ObjectId AND the reverse
    /// map must point every emitted ObjectId back at the right
    /// node uuid. If a future refactor accidentally drops either
    /// map update (e.g. by simplifying the post-loop block to
    /// only touch one map), this test pops.
    #[test]
    fn cache_replay_registers_both_forward_and_reverse_maps() {
        let (mut doc, mut sync, scene_a, ids) = three_vector_scene();
        // Snapshot the first sync's ObjectIds per uuid so we can
        // verify the cache-replay path produces the same mapping.
        let mut expected_primary: HashMap<Uuid, ObjectId> = HashMap::new();
        for (uuid, obj_id) in &sync.uuid_to_object_id {
            expected_primary.insert(*uuid, *obj_id);
        }
        // Second sync goes 100% through the cache (no version
        // bumps, no structure changes).
        let scene_b = sync.sync_document_to_scene(&mut doc, None, &[]);
        assert_eq!(
            scene_a.objects.len(),
            scene_b.objects.len(),
            "cache replay must produce same object count",
        );
        // Forward map: every node uuid must still resolve to its
        // original primary ObjectId.
        for uuid in &ids {
            let actual = sync.object_id_for_uuid(*uuid).unwrap_or_else(|| {
                panic!("uuid {uuid} dropped from forward map after cache-only sync")
            });
            let expected = expected_primary[uuid];
            assert_eq!(
                actual, expected,
                "uuid {uuid} primary ObjectId changed across cache-replay sync",
            );
        }
        // Reverse map: every ObjectId emitted into the scene must
        // resolve back to a uuid in our doc (catches the case
        // where the reverse-map insert in the replay path is
        // accidentally dropped).
        let doc_uuids: std::collections::HashSet<Uuid> = ids.iter().copied().collect();
        for obj in &scene_b.objects {
            let uuid = sync.uuid_for_object_id(obj.id).unwrap_or_else(|| {
                panic!(
                    "ObjectId {:?} in replayed scene missing from reverse map \
                     — Phase E cache-replay map update regressed",
                    obj.id,
                )
            });
            assert!(
                doc_uuids.contains(&uuid),
                "reverse map maps ObjectId {:?} to unknown uuid {uuid}",
                obj.id,
            );
        }
    }
}
