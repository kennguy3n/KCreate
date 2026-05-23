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
//!   clipping). TODO(Block A Task 3 follow-up): emit dashed spacing
//!   guides between adjacent artboards while one is being
//!   dragged/resized — requires gesture-state plumbing the bridge
//!   doesn't yet expose, so it's deferred to a later task.
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
use kcreate_core::node::{Node, NodeType};
use kcreate_renderer::{
    Color, Object, ObjectId, ObjectKind, PathCommand, Point2, Rect, Scene, Stroke, Style,
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
#[derive(Debug, Default)]
pub struct SceneSync {
    uuid_to_object_id: HashMap<Uuid, ObjectId>,
    object_id_to_uuid: HashMap<ObjectId, Uuid>,
    next_id: AtomicU64,
    overlay_watermark: u64,
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
        }
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

    /// Translate the document graph into a fresh renderer [`Scene`].
    ///
    /// This rebuilds the scene from scratch every call. That is
    /// deliberate: Phase 0 doesn't need incremental scene patching, and
    /// a full rebuild keeps the mapping in lockstep with the document
    /// (no risk of stale `ObjectId`s outliving deleted nodes).
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
        doc: &DocumentGraph,
        blob_store: Option<&BlobStore>,
        selection: &[Uuid],
    ) -> Scene {
        // Forget previous mappings so deleted nodes don't linger.
        // (Re-allocation will reuse the same ids for ids that *do* show
        // up again because `allocate` checks the map first.)
        let preserved = std::mem::take(&mut self.uuid_to_object_id);
        self.object_id_to_uuid.clear();
        // Reinstate the existing mappings as a side-table; `allocate`
        // will pick them up so identity is stable across syncs.
        self.uuid_to_object_id = preserved;
        // Re-populate the reverse map from the preserved forward map.
        let forward = self.uuid_to_object_id.clone();
        for (uuid, obj_id) in &forward {
            self.object_id_to_uuid.insert(*obj_id, *uuid);
        }

        let mut scene = Scene::new(DEFAULT_CLEAR);
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
                &mut scene,
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

        // Persist the watermark for follow-up overlay emitters
        // (notably `append_presence_cursors`). Selection highlights
        // count downward from `u64::MAX` so they don't participate in
        // this watermark — see [`Self::append_presence_cursors`].
        self.overlay_watermark = overlay.next;

        // Selection highlights go on top, sorted by document order so
        // overlapping selections paint deterministically.
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
            .with_z(z);
            scene.add_object(highlight);
            z += 1;
            highlight_id = highlight_id.saturating_sub(1);
        }

        scene
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
                fill: Some(color),
                stroke: Some(Stroke::new(Color::rgba(1.0, 1.0, 1.0, 0.9), 1.0)),
            };
            let cursor_id = Self::next_overlay_id(&mut overlay);
            let cursor_obj = Object::new(ObjectKind::Path(path), cursor_style)
                .with_id(cursor_id)
                .with_z(z);
            scene.add_object(cursor_obj);
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
                    fill: Some(color),
                    stroke: None,
                };
                let label_id = Self::next_overlay_id(&mut overlay);
                let label_obj = Object::new(
                    ObjectKind::Text {
                        origin: label_origin,
                        text: cursor.display_name.clone(),
                        font_family: ARTBOARD_LABEL_FONT.to_string(),
                        font_size: cursor_label_font_size_world,
                    },
                    label_style,
                )
                .with_id(label_id)
                .with_z(z);
                scene.add_object(label_obj);
                z += 1;
            }
        }
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
    pub fn append_presence_selection_halos(
        &mut self,
        scene: &mut Scene,
        doc: &DocumentGraph,
        selections: &[PresenceSelection],
        starting_z: i32,
        viewport_zoom: f32,
    ) {
        if selections.is_empty() {
            return;
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
        let mut z = starting_z;
        for selection in selections {
            let base = peer_color(&selection.peer_id);
            let stroke_color = Color::rgba(base.r, base.g, base.b, HALO_STROKE_ALPHA);
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
                let halo_obj = Object::new(ObjectKind::Rect(rect), style)
                    .with_id(halo_id)
                    .with_z(z);
                scene.add_object(halo_obj);
                z += 1;

                // Peer-name pill at top-left. Only emitted on the
                // first node of each peer's selection to avoid
                // spamming the canvas with duplicate labels when a
                // peer has many nodes selected — the label visually
                // attaches to the first halo and the user can infer
                // the rest are the same peer from the matching
                // stroke colour.
                if !selection.display_name.is_empty() && Some(node_id) == selection.node_ids.first()
                {
                    let origin = Point2::new(
                        (world.x - f64::from(outset_world)) as f32,
                        (world.y - f64::from(outset_world)) as f32 - label_offset_world,
                    );
                    let label_style = Style {
                        fill: Some(stroke_color),
                        stroke: None,
                    };
                    let label_id = Self::next_overlay_id(&mut overlay);
                    let label_obj = Object::new(
                        ObjectKind::Text {
                            origin,
                            text: selection.display_name.clone(),
                            font_family: ARTBOARD_LABEL_FONT.to_string(),
                            font_size: label_font_size_world,
                        },
                        label_style,
                    )
                    .with_id(label_id)
                    .with_z(z);
                    scene.add_object(label_obj);
                    z += 1;
                }
            }
        }
        // Persist watermark so any follow-up overlay emitter in the
        // same sync (currently none, but future-proof) continues
        // the stream.
        self.overlay_watermark = overlay.next;
    }

    #[allow(clippy::too_many_arguments)]
    fn visit(
        &mut self,
        doc: &DocumentGraph,
        id: Uuid,
        blob_store: Option<&BlobStore>,
        scene: &mut Scene,
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
                self.emit_artboard(node, scene, z, emitted, overlay);
                Some(node_world_bounds(node))
            }
            NodeType::VectorLayer => {
                self.emit_vector(node, scene, z, emitted);
                clip
            }
            NodeType::RasterLayer => {
                self.emit_raster(node, scene, z, blob_store, emitted);
                clip
            }
            NodeType::TextLayer => {
                self.emit_text(node, scene, z, emitted);
                clip
            }
            NodeType::Page
            | NodeType::GroupLayer
            | NodeType::ComponentLayer
            | NodeType::LayoutFrame => clip,
        };
        for child in &node.children {
            self.visit(
                doc, *child, blob_store, scene, z, emitted, overlay, child_clip,
            );
        }
    }

    fn emit_artboard(
        &mut self,
        node: &Node,
        scene: &mut Scene,
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
                fill: Some(ARTBOARD_SHADOW),
                stroke: None,
            },
        )
        .with_id(Self::next_overlay_id(overlay))
        .with_z(*z);
        scene.add_object(shadow);
        *z += 1;

        // 2. Artboard background rect — the hit-testable, document-
        //    backed object the user can select and drag.
        let obj_id = self.allocate(node.id);
        self.record(node.id, obj_id);
        emitted.push(node.id);
        let fill = node_fill(node).unwrap_or(Color::rgba(1.0, 1.0, 1.0, 1.0));
        let style = Style {
            fill: Some(fill),
            stroke: None,
        };
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
        scene.add_object(obj);
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
                    fill: Some(ARTBOARD_LABEL_COLOR),
                    stroke: None,
                },
            )
            .with_id(Self::next_overlay_id(overlay))
            .with_z(*z);
            scene.add_object(label);
            *z += 1;
        }
    }

    fn emit_vector(
        &mut self,
        node: &Node,
        scene: &mut Scene,
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
        let obj_id = self.allocate(node.id);
        self.record(node.id, obj_id);
        emitted.push(node.id);
        let commands = vector_path_to_renderer(&path);
        let style = node_style(node);
        let (tx, ty) = node_translation(node);
        let obj = Object::new(ObjectKind::Path(commands), style)
            .with_id(obj_id)
            .with_translation(tx as f32, ty as f32)
            .with_z(*z);
        scene.add_object(obj);
        *z += 1;
    }

    fn emit_raster(
        &mut self,
        node: &Node,
        scene: &mut Scene,
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
        let meta = node.metadata.get(RASTER_IMAGE_METADATA_KEY);
        let resolved = meta
            .and_then(|v| serde_json::from_value::<RasterImageMeta>(v.clone()).ok())
            .and_then(|m| resolve_raster_image(&m, blob_store));
        let kind = if let Some((pw, ph, pixels)) = resolved {
            ObjectKind::Image {
                rect,
                pixels_width: pw,
                pixels_height: ph,
                pixels,
            }
        } else {
            ObjectKind::Rect(rect)
        };
        let style = if matches!(kind, ObjectKind::Rect(_)) {
            Style {
                fill: Some(RASTER_PLACEHOLDER),
                stroke: None,
            }
        } else {
            Style {
                fill: None,
                stroke: None,
            }
        };
        let obj = Object::new(kind, style).with_id(obj_id).with_z(*z);
        scene.add_object(obj);
        *z += 1;
    }

    fn emit_text(&mut self, node: &Node, scene: &mut Scene, z: &mut i32, emitted: &mut Vec<Uuid>) {
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
        let style = node_style(node);
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
        scene.add_object(obj);
        *z += 1;
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

const fn node_fill(node: &Node) -> Option<Color> {
    match node.style.fill {
        kcreate_core::node::FillStyle::Solid(rgba) => {
            Some(Color::rgba(rgba.r, rgba.g, rgba.b, rgba.a))
        }
        kcreate_core::node::FillStyle::None | kcreate_core::node::FillStyle::Gradient(_) => None,
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
    use kcreate_core::node::{Bounds, FillStyle, Node, NodeType, RgbaColor, Transform2D};
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
    fn vector_layer_becomes_path_object() {
        let mut doc = DocumentGraph::new();
        let path = unit_square_path();
        let node = vector_node(&path);
        let id = doc.insert_node(node).expect("insert");
        let mut sync = SceneSync::new();
        let scene = sync.sync_document_to_scene(&doc, None, &[]);
        assert_eq!(scene.objects.len(), 1);
        assert!(matches!(scene.objects[0].kind, ObjectKind::Path(_)));
        let obj_id = sync.object_id_for_uuid(id).expect("forward lookup");
        let back = sync.uuid_for_object_id(obj_id).expect("reverse lookup");
        assert_eq!(back, id);
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
        let scene = sync.sync_document_to_scene(&doc, None, &[]);
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
        let scene = sync.sync_document_to_scene(&doc, None, &[id]);
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
        let _ = sync.sync_document_to_scene(&doc, None, &[]);
        let first = sync.object_id_for_uuid(id).expect("first");
        let _ = sync.sync_document_to_scene(&doc, None, &[]);
        let second = sync.object_id_for_uuid(id).expect("second");
        assert_eq!(
            first, second,
            "syncing the same document twice must reuse the same ObjectId for the same uuid"
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
        let scene = sync.sync_document_to_scene(&doc, None, &[]);
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
        let scene = sync.sync_document_to_scene(&doc, None, &[]);
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
        let scene = sync.sync_document_to_scene(&doc, None, &[]);
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
        let scene = sync.sync_document_to_scene(&doc, None, &[]);
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
        let mut scene = sync.sync_document_to_scene(&doc, None, &[]);

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
        let mut scene = sync.sync_document_to_scene(&doc, None, &[]);
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
        let mut scene = sync.sync_document_to_scene(&doc, None, &[]);
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
        let mut scene_1x = sync_1x.sync_document_to_scene(&doc, None, &[]);
        let mut sync_2x = SceneSync::new();
        let mut scene_2x = sync_2x.sync_document_to_scene(&doc, None, &[]);

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
        let mut scene_bad = sync_bad.sync_document_to_scene(&doc, None, &[]);
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
}
