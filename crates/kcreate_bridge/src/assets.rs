//! Elements / asset library bridge entry points (workstream G6).
//!
//! The catalogue + search live in [`kcreate_core::assets`] (pure data,
//! no GPU, no SVG parsing). This module owns the half that needs the
//! rest of the workspace:
//!
//! * [`categories`] / [`list`] / [`search`] — thin reads over the core
//!   catalogue, shaped for the panel.
//! * [`insert`] — parse a bundled asset's SVG into editable vector
//!   node(s) and stamp them onto the document graph at a target
//!   position/size as a **single undoable operation**.
//!
//! Inserted assets are real [`NodeType::VectorLayer`] nodes carrying
//! `metadata[VECTOR_PATH_METADATA_KEY]` geometry and a normal
//! [`NodeStyle`] — exactly what `canvas_create_rect` produces — so they
//! render through `scene_sync::emit_vector` and are recolorable via
//! `document_update_node` like any other vector node. Nothing here is a
//! flat raster.

use chrono::Utc;
use serde::Serialize;
use uuid::Uuid;

use kcreate_core::assets::{self, AssetCategory, AssetDef, CategoryInfo};
use kcreate_core::node::{Bounds, FillStyle, Node, NodeStyle, NodeType, StrokeStyle};
use kcreate_core::operation::Operation;
use kcreate_vector::{import_svg_styled, StyledPath};

use crate::document::{slot, sync_scene_locked, DocumentBridgeError, Result};
use crate::scene_sync::VECTOR_PATH_METADATA_KEY;

/// Parse a category slug into an [`AssetCategory`], or fail with a
/// wire-friendly [`DocumentBridgeError::InvalidArgument`].
fn parse_category(slug: &str) -> Result<AssetCategory> {
    AssetCategory::from_slug(slug).ok_or_else(|| DocumentBridgeError::InvalidArgument {
        argument: "category".into(),
        value: slug.to_string(),
    })
}

/// Every category with its label and asset count, in panel order.
#[must_use]
pub fn categories() -> Vec<CategoryInfo> {
    assets::categories()
}

/// List assets, optionally restricted to a single category (`None` =
/// the whole catalogue). `category` is a slug such as `"icons"`.
pub fn list(category: Option<&str>) -> Result<Vec<&'static AssetDef>> {
    match category {
        Some(slug) => Ok(assets::list_category(parse_category(slug)?)),
        None => Ok(assets::catalog().iter().collect()),
    }
}

/// Search the catalogue by free-text `query`, optionally restricted to
/// a single category slug. Ranked by relevance (see
/// [`kcreate_core::assets::search`]).
pub fn search(query: &str, category: Option<&str>) -> Result<Vec<&'static AssetDef>> {
    let category = category.map(parse_category).transpose()?;
    Ok(assets::search(query, category))
}

/// Result of [`insert`]: the wrapping group plus every leaf vector
/// node, so the host can select the group as a single element and the
/// renderer can address individual paths for recolouring.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InsertedAsset {
    /// Id of the [`NodeType::GroupLayer`] wrapping the inserted paths.
    pub group_id: String,
    /// Ids of the leaf [`NodeType::VectorLayer`] nodes, in draw order.
    pub node_ids: Vec<String>,
    /// Resolved asset name (handy for the host's status toast).
    pub name: String,
    /// World-space bounds of the placed artwork.
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// A single leaf node's world geometry + paint, computed before we take
/// the workspace lock so the locked section stays short.
struct PlacedPath {
    /// Path geometry already serialized for `metadata[VECTOR_PATH_METADATA_KEY]`.
    /// We serialize here, in the lock-free prepare phase, rather than inside
    /// the locked mutation loop: that keeps the only fallible step out of the
    /// critical section, so node insertion below cannot serialize-fail partway
    /// and strand half-built nodes outside the single undoable operation.
    path_json: serde_json::Value,
    bounds: Bounds,
    fill: FillStyle,
    stroke: Option<StrokeStyle>,
}

/// Convert a parsed [`StyledPath`] (in SVG user space) into a world-space
/// [`PlacedPath`] using a uniform `scale` then translation `(tx, ty)`,
/// serializing the placed geometry up front (see [`PlacedPath::path_json`]).
fn place(styled: &StyledPath, scale: f64, tx: f64, ty: f64) -> Result<PlacedPath> {
    let path = styled.path.scaled_translated(scale, tx, ty);
    let b = path.bounds();
    let bounds = Bounds {
        x: b.min_x,
        y: b.min_y,
        width: b.width(),
        height: b.height(),
    };
    let fill = match styled.fill {
        Some(c) => FillStyle::Solid(c),
        None => FillStyle::None,
    };
    let stroke = styled.stroke.map(|s| StrokeStyle {
        color: s.color,
        // `s.width` already carries the SVG's own viewBox→viewport scale
        // (baked in by `import_svg_styled`), so it is in document-space
        // units here — not raw SVG user units. Applying the placement
        // `scale` matches the scale-to-fit used for the geometry, so the
        // stroke stays proportional at any target size.
        width: s.width * scale,
        ..StrokeStyle::default()
    });
    Ok(PlacedPath {
        path_json: serde_json::to_value(&path)?,
        bounds,
        fill,
        stroke,
    })
}

/// Insert the bundled asset `asset_id` onto the canvas as editable
/// vector node(s).
///
/// The asset's SVG is parsed into one styled path per `<path>` (fills
/// and strokes preserved), uniformly scaled so its longest side equals
/// `target_size`, and translated so the artwork's top-left sits at
/// `(x, y)` in world coordinates. The paths become
/// [`NodeType::VectorLayer`] children of a single
/// [`NodeType::GroupLayer`], recorded as **one** undoable
/// `assets_insert` [`Operation`]. `parent_id` is the container (an
/// artboard, typically); `None` attaches to the document root.
pub fn insert(
    asset_id: &str,
    parent_id: Option<Uuid>,
    x: f64,
    y: f64,
    target_size: f64,
) -> Result<InsertedAsset> {
    let def = assets::get(asset_id).ok_or_else(|| DocumentBridgeError::InvalidArgument {
        argument: "asset_id".into(),
        value: asset_id.to_string(),
    })?;

    insert_styled_paths(
        def.svg.as_bytes(),
        def.name,
        parent_id,
        x,
        y,
        target_size,
        "assets_insert",
        serde_json::json!({ "asset_id": asset_id }),
    )
}

/// Shared core that parses SVG `svg` into editable vector node(s) and
/// stamps them onto the document graph as a single undoable operation.
///
/// Both the asset-library [`insert`] and the brand-logo insertion in
/// `document::brand_logo_insert` funnel through here so the
/// SVG → group-of-vector-layers placement logic (scale-to-fit,
/// aspect-preserving, top-left at `(x, y)`) lives in exactly one place.
/// `name` labels the wrapping group (and each leaf when the SVG yields
/// multiple paths); `op_command` / `op_before` parameterise the
/// recorded [`Operation`] so each caller keeps its own undo provenance.
#[allow(clippy::too_many_arguments)]
pub(crate) fn insert_styled_paths(
    svg: &[u8],
    name: &str,
    parent_id: Option<Uuid>,
    x: f64,
    y: f64,
    target_size: f64,
    op_command: &str,
    op_before: serde_json::Value,
) -> Result<InsertedAsset> {
    if !x.is_finite() || !y.is_finite() {
        return Err(DocumentBridgeError::InvalidArgument {
            argument: "position".into(),
            value: format!("({x}, {y}) (must be finite)"),
        });
    }
    if !target_size.is_finite() || target_size <= 0.0 {
        return Err(DocumentBridgeError::InvalidArgument {
            argument: "target_size".into(),
            value: format!("{target_size} (must be finite and positive)"),
        });
    }

    let styled = import_svg_styled(svg)
        .map_err(|e| DocumentBridgeError::Internal(format!("{name:?} parse: {e}")))?;
    let styled: Vec<StyledPath> = styled
        .into_iter()
        .filter(|s| !s.path.commands.is_empty())
        .collect();
    if styled.is_empty() {
        return Err(DocumentBridgeError::Internal(format!(
            "{name:?} produced no drawable paths"
        )));
    }

    // Union of the source geometry in SVG user space → uniform
    // scale-to-fit, preserving aspect ratio.
    let mut src = kcreate_vector::BoundingBox::empty();
    for s in &styled {
        src = src.union(s.path.bounds());
    }
    let longest = src.width().max(src.height());
    let scale = if longest > 0.0 {
        target_size / longest
    } else {
        1.0
    };
    // world = local * scale + (tx, ty); we want the source top-left
    // (src.min) to land at (x, y).
    let tx = x - src.min_x * scale;
    let ty = y - src.min_y * scale;

    let placed: Vec<PlacedPath> = styled
        .iter()
        .map(|s| place(s, scale, tx, ty))
        .collect::<Result<Vec<_>>>()?;

    // Overall world bounds of the placed artwork (for the group node).
    let mut world = kcreate_vector::BoundingBox::empty();
    for p in &placed {
        world = world.union(kcreate_vector::BoundingBox::new(
            p.bounds.x,
            p.bounds.y,
            p.bounds.x + p.bounds.width,
            p.bounds.y + p.bounds.height,
        ));
    }
    let group_bounds = Bounds {
        x: world.min_x,
        y: world.min_y,
        width: world.width(),
        height: world.height(),
    };

    let multi = placed.len() > 1;

    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;

    // Group wrapper so the whole asset is one selectable/movable unit.
    let mut group = Node::new(NodeType::GroupLayer, name);
    group.parent_id = parent_id;
    group.bounds = group_bounds;
    let group_id = ws.project.document.insert_node(group)?;

    let mut node_ids: Vec<Uuid> = Vec::with_capacity(placed.len());
    for (i, p) in placed.into_iter().enumerate() {
        let leaf_name = if multi {
            format!("{name} {}", i + 1)
        } else {
            name.to_string()
        };
        let mut node = Node::new(NodeType::VectorLayer, leaf_name);
        node.parent_id = Some(group_id);
        node.bounds = p.bounds;
        node.style = NodeStyle {
            fill: p.fill,
            stroke: p.stroke,
            ..NodeStyle::default()
        };
        node.metadata
            .insert(VECTOR_PATH_METADATA_KEY.to_string(), p.path_json);
        let id = ws.project.document.insert_node(node)?;
        node_ids.push(id);
    }

    ws.project.modified_at = Utc::now();

    // Single undoable operation covering the group and every leaf.
    let mut affected = Vec::with_capacity(node_ids.len() + 1);
    affected.push(group_id);
    affected.extend(node_ids.iter().copied());
    let op = Operation::new(
        "user",
        op_command,
        op_before,
        serde_json::json!({
            "group_id": group_id,
            "node_count": node_ids.len(),
            "name": name,
        }),
        affected,
    );
    ws.project.execute_operation(op);
    let _ = sync_scene_locked(&mut guard);
    drop(guard);

    Ok(InsertedAsset {
        group_id: group_id.to_string(),
        node_ids: node_ids.iter().map(Uuid::to_string).collect(),
        name: name.to_string(),
        x: group_bounds.x,
        y: group_bounds.y,
        width: group_bounds.width,
        height: group_bounds.height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{
        document_node_fill, document_undo, document_update_node, project_close, project_create,
        reset_for_tests, with_workspace, UpdateNodeProps,
    };
    use kcreate_core::node::RgbaColor;
    use serial_test::serial;

    fn tmpdir() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    fn uuid(s: &str) -> Uuid {
        Uuid::parse_str(s).expect("uuid")
    }

    fn op_log_len() -> usize {
        with_workspace(|ws| Ok(ws.project.operation_log.len())).expect("len")
    }

    #[test]
    fn categories_cover_every_catalogue_entry() {
        let cats = categories();
        assert_eq!(cats.len(), AssetCategory::ALL.len());
        let total: usize = cats.iter().map(|c| c.count).sum();
        assert_eq!(total, assets::catalog().len());
        // Every tab must have at least one asset — an empty tab is a
        // packaging bug.
        assert!(cats.iter().all(|c| c.count > 0));
    }

    #[test]
    fn list_filters_by_category_and_rejects_unknown_slug() {
        let icons = list(Some("icons")).expect("icons");
        assert!(!icons.is_empty());
        assert!(icons.iter().all(|a| a.category == AssetCategory::Icons));
        assert_eq!(list(None).expect("all").len(), assets::catalog().len());

        let err = list(Some("bogus")).expect_err("unknown category");
        assert!(matches!(
            err,
            DocumentBridgeError::InvalidArgument { ref argument, .. } if argument == "category"
        ));
    }

    #[test]
    fn search_can_be_scoped_to_a_category() {
        // "star" matches both the filled shape and the outline icon.
        let unscoped = search("star", None).expect("search");
        assert!(unscoped.iter().any(|a| a.id == "star"));

        let icons = search("star", Some("icons")).expect("search");
        assert!(!icons.is_empty());
        assert!(icons.iter().all(|a| a.category == AssetCategory::Icons));
        assert!(icons.iter().any(|a| a.id == "star-outline"));
    }

    #[test]
    fn insert_rejects_unknown_asset_and_bad_arguments() {
        assert!(matches!(
            insert("does-not-exist", None, 0.0, 0.0, 48.0),
            Err(DocumentBridgeError::InvalidArgument { ref argument, .. }) if argument == "asset_id"
        ));
        assert!(matches!(
            insert("star", None, 0.0, 0.0, 0.0),
            Err(DocumentBridgeError::InvalidArgument { ref argument, .. }) if argument == "target_size"
        ));
        assert!(matches!(
            insert("star", None, f64::NAN, 0.0, 48.0),
            Err(DocumentBridgeError::InvalidArgument { ref argument, .. }) if argument == "position"
        ));
    }

    #[test]
    #[serial]
    fn insert_is_single_undoable_op_producing_editable_vector_nodes() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("assets-insert", dir.path()).expect("create");

        let before_ops = op_log_len();
        let inserted = insert("star", None, 100.0, 120.0, 48.0).expect("insert");

        // Exactly one operation recorded → one undo step reverts the
        // whole asset (group + leaves), matching canvas_create_rect.
        assert_eq!(op_log_len(), before_ops + 1);

        assert!(!inserted.node_ids.is_empty());
        // Longest side scaled to target_size; placed at the drop point.
        assert!((inserted.width.max(inserted.height) - 48.0).abs() < 1e-6);
        assert!((inserted.x - 100.0).abs() < 1e-6);
        assert!((inserted.y - 120.0).abs() < 1e-6);

        let group = uuid(&inserted.group_id);
        let first_child = uuid(&inserted.node_ids[0]);

        with_workspace(|ws| {
            let g = ws.project.document.get_node(group).expect("group");
            assert_eq!(g.node_type, NodeType::GroupLayer);
            for nid in &inserted.node_ids {
                let n = ws.project.document.get_node(uuid(nid)).expect("leaf");
                assert_eq!(n.node_type, NodeType::VectorLayer);
                assert_eq!(n.parent_id, Some(group));
                // Real editable geometry, not a raster.
                assert!(n.metadata.contains_key(VECTOR_PATH_METADATA_KEY));
                // Visible: a fill or a stroke (never an invisible node).
                assert!(
                    !matches!(n.style.fill, FillStyle::None) || n.style.stroke.is_some(),
                    "leaf {nid} has neither fill nor stroke",
                );
            }
            Ok(())
        })
        .expect("inspect");

        // Recolor a leaf — proves the inserted node behaves like any
        // other vector node (not a flat image).
        let red = RgbaColor::new(0.9, 0.1, 0.1, 1.0);
        document_update_node(
            first_child,
            &UpdateNodeProps {
                fill: Some(FillStyle::Solid(red)),
                ..UpdateNodeProps::default()
            },
        )
        .expect("recolor");
        with_workspace(|ws| {
            let n = ws.project.document.get_node(first_child).expect("leaf");
            assert_eq!(n.style.fill, FillStyle::Solid(red));
            Ok(())
        })
        .expect("verify recolor");
        assert!(document_node_fill(first_child).expect("fill").is_some());

        // The recolor above went through `document_update_node`, which
        // mutates the node in place WITHOUT recording an operation
        // (graph edits are host-driven — see `document_update_node`).
        // The pending undo is therefore still the insert, so a single
        // `document_undo` reverts the whole asset. Assert it is exactly
        // the `assets_insert` op covering the group + every leaf, rather
        // than relying on the call merely returning `Some`.
        let outcome = document_undo().expect("undo").expect("an op to undo");
        assert_eq!(outcome.command, "assets_insert");
        assert!(outcome.affected_nodes.contains(&group));
        for nid in &inserted.node_ids {
            assert!(outcome.affected_nodes.contains(&uuid(nid)));
        }

        project_close();
    }

    #[test]
    #[serial]
    fn insert_multi_path_asset_groups_every_leaf() {
        reset_for_tests();
        let dir = tmpdir();
        project_create("assets-multi", dir.path()).expect("create");

        // A flat illustration is built from several coloured paths.
        let inserted = insert("mountain-sun", None, 0.0, 0.0, 120.0).expect("insert");
        assert!(
            inserted.node_ids.len() > 1,
            "expected a multi-path illustration, got {} node(s)",
            inserted.node_ids.len()
        );
        let group = uuid(&inserted.group_id);
        with_workspace(|ws| {
            let g = ws.project.document.get_node(group).expect("group");
            // Group's child list matches the reported leaves.
            assert_eq!(g.children.len(), inserted.node_ids.len());
            Ok(())
        })
        .expect("inspect");

        project_close();
    }
}
