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
#[derive(Debug, Default)]
pub struct SceneSync {
    uuid_to_object_id: HashMap<Uuid, ObjectId>,
    object_id_to_uuid: HashMap<ObjectId, Uuid>,
    next_id: AtomicU64,
}

impl SceneSync {
    #[must_use]
    pub fn new() -> Self {
        Self {
            uuid_to_object_id: HashMap::new(),
            object_id_to_uuid: HashMap::new(),
            // ObjectId(0) is reserved as a sentinel for "no object".
            next_id: AtomicU64::new(1),
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
}
