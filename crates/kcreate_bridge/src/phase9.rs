//! Phase 9 bridge entry points.
//!
//! Following the convention in `phase8.rs`, every public function
//! here either runs against the open workspace (gated by
//! `with_workspace` / `with_workspace_mut`) or is a pure helper that
//! doesn't touch the singleton. The N-API marshalling lives in
//! `lib.rs`.
//!
//! Scope:
//!
//! - **Block B Task 7**: `brief_to_project` — wire an LLM brief into
//!   a fresh artboard + brand kit + starter layers.
//! - **Block B Task 10**: `palette_extract_and_apply_brand_kit` — one
//!   shot k-means → BrandKit upsert.
//! - **Block B Task 11**: `text_autofit_recompute` — bisect the
//!   autofit font size on a single text layer when the renderer
//!   reports a resize.
//! - **Block B Task 12**: `ai_trace_raster` — raster → vector trace,
//!   appended as a sibling group of vector-path nodes.
//! - **Block C Task 13**: `import_psd` — PSD → ImportedPsd summary.
//! - **Block C Task 14**: `image_read_exif` — extract EXIF from raw
//!   image bytes.
//! - **Block C Task 15**: `import_penpot` — `.penpot` zip → summary.
//! - **Block C Task 16**: `export_svg_preview` — SVG → PNG raster.
//! - **Block C Task 17**: `operation_log_filter` — paginated /
//!   filtered history view (in-memory log).
//! - **Block D Task 19**: `ai_iconify` — icon-grid normalisation.
//! - **Block D Task 20**: `ai_batch_alt_text` — deterministic batch
//!   alt-text on every raster layer in the open page.
//! - **Block D Task 21**: `guide_*` — create / delete / list guides.
//! - **Block D Task 22**: `artboard_grid_*` — per-artboard grid.
//! - **Block D Task 23**: `document_align` / `document_distribute`.
//! - **Block E Task 27**: `export_validate` — pre-flight validation.
//!
//! The memory-pressure watchdog (Task 25) lives in `perf.rs`; the
//! autosave subsystem (Task 26) lives in `autosave.rs`. The N-API
//! layer wraps them directly.

use std::fs;

use chrono::Utc;
use kcreate_ai::iconify::{iconify, IconPath, IconPoint, IconifyOptions};
use kcreate_ai::palette::extract_palette;
use kcreate_ai::trace::{trace_raster, TraceOptions, TraceThreshold};
use kcreate_core::align::{align_bounds, distribute_bounds, Align, AlignDelta, DistributeAxis};
use kcreate_core::node::{
    standard_presets, BlendMode, Bounds, FillStyle, Node, NodeStyle, NodeType, RgbaColor,
    StrokeStyle,
};
use kcreate_core::operation::Operation;
use kcreate_core::project::{BrandKit, NamedColor};
use kcreate_export::exif::{read_exif_from_bytes, ExifError, ExifMetadata};
use kcreate_export::penpot_import::import_penpot_bytes;
use kcreate_export::psd_import::import_psd_bytes;
use kcreate_export::scene_metadata::{
    raster_image_meta, text_layer_meta, RasterImageMeta, TextLayerMeta, TEXT_LAYER_METADATA_KEY,
};
use kcreate_export::svg_preview::{
    svg_to_raster_preview, SvgPreview, SvgPreviewError, SvgPreviewOptions,
};
use kcreate_export::validate::{
    validate_export_request, ExportValidationReport, ExportValidationRequest,
};
use kcreate_storage::guides::{
    delete_all_for_page as storage_delete_all_guides_for_page,
    delete_guide as storage_delete_guide, list_all as storage_list_all_guides,
    list_for_page as storage_list_guides_for_page,
    load_grid_settings as storage_load_grid_settings, upsert_grid_settings as storage_upsert_grid,
    upsert_guide as storage_upsert_guide, GridSettings, Guide, GuideOrientation,
};
use kcreate_text::autofit::{compute_autofit_size, AutofitOptions};
use kcreate_text::paragraph::TextStyle;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::document::{with_workspace, with_workspace_mut, DocumentBridgeError, Result};

// ---------------------------------------------------------------------------
// Block D Task 21 — guides
// ---------------------------------------------------------------------------

/// Wire-format mirror of [`Guide`]. The N-API surface only ever
/// speaks strings / numbers / bools.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GuideInfo {
    pub id: String,
    pub page_id: String,
    pub orientation: String,
    pub position: f64,
    pub color: String,
    pub locked: bool,
    pub created_at: String,
}

impl From<Guide> for GuideInfo {
    fn from(g: Guide) -> Self {
        Self {
            id: g.id.to_string(),
            page_id: g.page_id.to_string(),
            orientation: g.orientation.as_str().to_string(),
            position: g.position,
            color: g.color,
            locked: g.locked,
            created_at: g.created_at.to_rfc3339(),
        }
    }
}

fn parse_orientation(s: &str) -> Result<GuideOrientation> {
    GuideOrientation::parse(s).ok_or_else(|| DocumentBridgeError::InvalidArgument {
        argument: "orientation".into(),
        value: s.into(),
    })
}

/// Insert (or upsert) a guide on `page_id`. Returns the canonical
/// stored row.
pub fn guide_create(
    page_id: Uuid,
    orientation: &str,
    position: f64,
    color: Option<String>,
    locked: bool,
) -> Result<GuideInfo> {
    if !position.is_finite() {
        return Err(DocumentBridgeError::InvalidArgument {
            argument: "position".into(),
            value: position.to_string(),
        });
    }
    let orientation = parse_orientation(orientation)?;
    let guide = Guide {
        id: Uuid::new_v4(),
        page_id,
        orientation,
        position,
        color: color.unwrap_or_else(|| "#0099ff".to_string()),
        locked,
        created_at: Utc::now(),
    };
    let stored = guide.clone();
    with_workspace_mut(|ws| {
        storage_upsert_guide(ws.store.connection(), &stored)
            .map_err(|e| DocumentBridgeError::Internal(format!("upsert_guide: {e}")))?;
        ws.project.modified_at = Utc::now();
        Ok(())
    })?;
    Ok(guide.into())
}

/// Delete a guide by id. Returns `true` if a row was removed.
pub fn guide_delete(id: Uuid) -> Result<bool> {
    with_workspace_mut(|ws| {
        let removed = storage_delete_guide(ws.store.connection(), id)
            .map_err(|e| DocumentBridgeError::Internal(format!("delete_guide: {e}")))?;
        if removed {
            ws.project.modified_at = Utc::now();
        }
        Ok(removed)
    })
}

/// Delete every guide on a page. Returns the count removed.
pub fn guide_clear_page(page_id: Uuid) -> Result<u64> {
    with_workspace_mut(|ws| {
        let n = storage_delete_all_guides_for_page(ws.store.connection(), page_id)
            .map_err(|e| DocumentBridgeError::Internal(format!("delete_all_for_page: {e}")))?;
        if n > 0 {
            ws.project.modified_at = Utc::now();
        }
        Ok(n)
    })
}

/// All guides for the given page, sorted by orientation then position.
pub fn guide_list(page_id: Uuid) -> Result<Vec<GuideInfo>> {
    with_workspace(|ws| {
        let rows = storage_list_guides_for_page(ws.store.connection(), page_id)
            .map_err(|e| DocumentBridgeError::Internal(format!("list_for_page: {e}")))?;
        Ok(rows.into_iter().map(GuideInfo::from).collect())
    })
}

/// All guides across all pages of the open project.
pub fn guide_list_all() -> Result<Vec<GuideInfo>> {
    with_workspace(|ws| {
        let rows = storage_list_all_guides(ws.store.connection())
            .map_err(|e| DocumentBridgeError::Internal(format!("list_all_guides: {e}")))?;
        Ok(rows.into_iter().map(GuideInfo::from).collect())
    })
}

// ---------------------------------------------------------------------------
// Block D Task 22 — per-artboard grid settings
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GridSettingsInfo {
    pub artboard_id: String,
    pub enabled: bool,
    pub spacing: f64,
    pub subdivisions: u32,
    pub color: String,
}

impl From<GridSettings> for GridSettingsInfo {
    fn from(g: GridSettings) -> Self {
        Self {
            artboard_id: g.artboard_id.to_string(),
            enabled: g.enabled,
            spacing: g.spacing,
            subdivisions: g.subdivisions,
            color: g.color,
        }
    }
}

/// Load the grid configuration for `artboard_id`. Falls back to a
/// safe default when no row exists so the renderer can always pull
/// a usable struct.
pub fn artboard_grid_settings(artboard_id: Uuid) -> Result<GridSettingsInfo> {
    with_workspace(|ws| {
        let cfg = storage_load_grid_settings(ws.store.connection(), artboard_id)
            .map_err(|e| DocumentBridgeError::Internal(format!("load_grid_settings: {e}")))?;
        Ok(cfg
            .unwrap_or_else(|| GridSettings::default_for(artboard_id))
            .into())
    })
}

/// Upsert grid settings. The bridge clamps spacing + subdivisions
/// to sensible ranges before persisting.
pub fn artboard_set_grid(
    artboard_id: Uuid,
    enabled: bool,
    spacing: f64,
    subdivisions: u32,
    color: Option<String>,
) -> Result<GridSettingsInfo> {
    if !spacing.is_finite() || spacing <= 0.0 {
        return Err(DocumentBridgeError::InvalidArgument {
            argument: "spacing".into(),
            value: spacing.to_string(),
        });
    }
    let spacing = spacing.clamp(1.0, 4096.0);
    let subdivisions = subdivisions.clamp(1, 16);
    let cfg = GridSettings {
        artboard_id,
        enabled,
        spacing,
        subdivisions,
        color: color.unwrap_or_else(|| "#cccccc".to_string()),
    };
    let stored = cfg.clone();
    with_workspace_mut(|ws| {
        storage_upsert_grid(ws.store.connection(), &stored)
            .map_err(|e| DocumentBridgeError::Internal(format!("upsert_grid_settings: {e}")))?;
        ws.project.modified_at = Utc::now();
        Ok(())
    })?;
    Ok(cfg.into())
}

// ---------------------------------------------------------------------------
// Block D Task 23 — align / distribute
// ---------------------------------------------------------------------------

fn parse_align(s: &str) -> Result<Align> {
    // The wire-format mirror in `apps/desktop/shared/scene.ts`
    // exposes the friendlier `center` / `middle` aliases that
    // Design Studio buttons use. The underlying `Align` enum
    // distinguishes the X and Y centres explicitly, so we accept
    // both naming conventions here.
    match s {
        "left" => Ok(Align::Left),
        "center" | "center_horizontal" | "centerHorizontal" => Ok(Align::CenterHorizontal),
        "right" => Ok(Align::Right),
        "top" => Ok(Align::Top),
        "middle" | "center_vertical" | "centerVertical" => Ok(Align::CenterVertical),
        "bottom" => Ok(Align::Bottom),
        other => Err(DocumentBridgeError::InvalidArgument {
            argument: "alignment".into(),
            value: other.into(),
        }),
    }
}

fn parse_distribute(s: &str) -> Result<DistributeAxis> {
    match s {
        "horizontal" => Ok(DistributeAxis::Horizontal),
        "vertical" => Ok(DistributeAxis::Vertical),
        other => Err(DocumentBridgeError::InvalidArgument {
            argument: "axis".into(),
            value: other.into(),
        }),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlignmentResult {
    pub node_id: String,
    pub dx: f64,
    pub dy: f64,
}

/// Align every node in `node_ids` along `alignment`. Records a
/// single grouped operation so undo rolls back the entire alignment
/// as one user action.
pub fn document_align(node_ids: &[Uuid], alignment: &str) -> Result<Vec<AlignmentResult>> {
    let alignment = parse_align(alignment)?;
    apply_node_deltas(node_ids, "document_align", |bounds| {
        align_bounds(bounds, alignment)
    })
}

/// Distribute `node_ids` evenly along `axis`. First and last
/// elements stay put; inner nodes are spaced equally.
pub fn document_distribute(node_ids: &[Uuid], axis: &str) -> Result<Vec<AlignmentResult>> {
    let axis = parse_distribute(axis)?;
    apply_node_deltas(node_ids, "document_distribute", |bounds| {
        distribute_bounds(bounds, axis)
    })
}

fn apply_node_deltas(
    node_ids: &[Uuid],
    command: &str,
    compute: impl FnOnce(&[Bounds]) -> Vec<AlignDelta>,
) -> Result<Vec<AlignmentResult>> {
    if node_ids.is_empty() {
        return Ok(Vec::new());
    }
    with_workspace_mut(|ws| {
        let mut bounds = Vec::with_capacity(node_ids.len());
        for &id in node_ids {
            let n = ws
                .project
                .document
                .get_node(id)
                .ok_or(DocumentBridgeError::NodeNotFound(id))?;
            bounds.push(n.bounds);
        }
        let deltas = compute(&bounds);
        if deltas.len() != node_ids.len() {
            return Err(DocumentBridgeError::Internal(format!(
                "{command}: delta count {} != node count {}",
                deltas.len(),
                node_ids.len()
            )));
        }
        let mut before = Vec::with_capacity(node_ids.len());
        let mut after = Vec::with_capacity(node_ids.len());
        let mut results = Vec::with_capacity(node_ids.len());
        let group_id = Uuid::new_v4();
        for (&id, delta) in node_ids.iter().zip(deltas.iter()) {
            let node = ws
                .project
                .document
                .get_node_mut(id)
                .ok_or(DocumentBridgeError::NodeNotFound(id))?;
            before.push((id, node.bounds));
            node.bounds.x += delta.dx;
            node.bounds.y += delta.dy;
            node.touch();
            after.push((id, node.bounds));
            results.push(AlignmentResult {
                node_id: id.to_string(),
                dx: delta.dx,
                dy: delta.dy,
            });
        }
        ws.project.modified_at = Utc::now();
        let op = Operation {
            group_id: Some(group_id),
            ..Operation::new(
                "user",
                command,
                serde_json::to_value(&before).unwrap_or(serde_json::Value::Null),
                serde_json::to_value(&after).unwrap_or(serde_json::Value::Null),
                node_ids.to_vec(),
            )
        };
        ws.project.execute_operation(op);
        Ok(results)
    })
}

// ---------------------------------------------------------------------------
// Block B Task 10 — palette extract + brand kit upsert
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaletteApplyResult {
    pub brand_kit_id: String,
    pub colors: Vec<String>,
}

/// Extract `num_colors` from the raster blob attached to `node_id`
/// and upsert a brand kit named `brand_kit_name` with those colours.
/// Returns the brand-kit id + extracted hex strings.
pub fn palette_extract_and_apply_brand_kit(
    node_id: Uuid,
    num_colors: u32,
    brand_kit_name: &str,
) -> Result<PaletteApplyResult> {
    if !(1..=64).contains(&num_colors) {
        return Err(DocumentBridgeError::InvalidArgument {
            argument: "num_colors".into(),
            value: num_colors.to_string(),
        });
    }
    let (rgba, width, height) = load_node_rgba(node_id)?;
    let extracted = extract_palette(&rgba, width, height, num_colors as usize);
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

    let brand_kit_id = with_workspace_mut(|ws| {
        let id = if let Some(kit) = ws
            .project
            .brand_kits
            .iter_mut()
            .find(|k| k.name == brand_kit_name)
        {
            kit.colors.clone_from(&named);
            kit.id
        } else {
            let mut kit = BrandKit::new(brand_kit_name);
            kit.colors.clone_from(&named);
            let id = kit.id;
            ws.project.brand_kits.push(kit);
            id
        };
        ws.project.modified_at = Utc::now();
        let op = Operation::new(
            "user",
            "palette_extract_and_apply_brand_kit",
            serde_json::json!({ "node_id": node_id, "num_colors": num_colors }),
            serde_json::json!({ "brand_kit_id": id, "colors": hex_codes }),
            vec![node_id],
        )
        .as_ai_generated();
        ws.project.execute_operation(op);
        Ok::<Uuid, DocumentBridgeError>(id)
    })?;
    Ok(PaletteApplyResult {
        brand_kit_id: brand_kit_id.to_string(),
        colors: hex_codes,
    })
}

// ---------------------------------------------------------------------------
// Block B Task 11 — text autofit recompute on resize
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutofitRecomputeResult {
    pub node_id: String,
    pub previous_size: f32,
    pub new_size: f32,
}

/// Bisect the autofit font size for `node_id`, persist the new
/// size into the [`TextLayerMeta`], and record an undoable operation.
///
/// Errors when the node isn't a text layer or doesn't have an
/// `text_autofit = true` metadata opt-in.
pub fn text_autofit_recompute(node_id: Uuid) -> Result<AutofitRecomputeResult> {
    with_workspace_mut(|ws| {
        let node = ws
            .project
            .document
            .get_node_mut(node_id)
            .ok_or(DocumentBridgeError::NodeNotFound(node_id))?;
        if node.node_type != NodeType::TextLayer {
            return Err(DocumentBridgeError::WrongNodeType {
                expected: NodeType::TextLayer,
                got: node.node_type,
            });
        }
        let autofit_enabled = node
            .metadata
            .get(crate::phase8::TEXT_AUTOFIT_METADATA_KEY)
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        if !autofit_enabled {
            return Err(DocumentBridgeError::InvalidArgument {
                argument: "node_id".into(),
                value: format!("node {node_id} does not have text_autofit enabled"),
            });
        }
        let meta = text_layer_meta(node).ok_or_else(|| {
            DocumentBridgeError::Internal(format!(
                "text layer {node_id} is missing a TextLayerMeta payload"
            ))
        })?;
        let frame = node.text_frame_options();
        let bounds = node.bounds;
        let previous_size = meta.font_size;
        let style = TextStyle {
            font_family: meta.font_family.clone(),
            font_size: meta.font_size,
            line_height: 1.25,
        };
        let opts = AutofitOptions {
            min_size: 6.0,
            max_size: 240.0,
            tolerance: 0.25,
            max_iterations: 12,
        };
        let new_size = compute_autofit_size(&meta.text, &style, &frame, bounds, &opts)
            .map_err(|e| DocumentBridgeError::Internal(format!("compute_autofit: {e}")))?;
        let new_meta = TextLayerMeta {
            text: meta.text,
            font_family: meta.font_family,
            font_size: new_size,
        };
        node.metadata.insert(
            TEXT_LAYER_METADATA_KEY.to_string(),
            serde_json::to_value(&new_meta).unwrap_or(serde_json::Value::Null),
        );
        node.touch();
        ws.project.modified_at = Utc::now();
        let op = Operation::new(
            "user",
            "text_autofit_recompute",
            serde_json::json!({ "font_size": previous_size }),
            serde_json::json!({ "font_size": new_size }),
            vec![node_id],
        );
        ws.project.execute_operation(op);
        Ok(AutofitRecomputeResult {
            node_id: node_id.to_string(),
            previous_size,
            new_size,
        })
    })
}

// ---------------------------------------------------------------------------
// Block B Task 12 — AI raster → vector trace
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceResult {
    pub group_node_id: String,
    pub path_count: usize,
    pub closed_path_count: usize,
    /// IDs of the per-path VectorLayer nodes inserted under
    /// `group_node_id`, in the same order they appear in the
    /// document. Surfaced so callers (e.g. AIAssistPanel) can offer
    /// "select traced paths" / "jump to" actions without having to
    /// re-query the document graph.
    pub path_node_ids: Vec<String>,
}

/// Trace the raster attached to `node_id` into vector contours and
/// append a sibling group of vector-path nodes whose metadata
/// carries the traced polylines.
///
/// `simplify_tolerance` is in pixels (Ramer-Douglas-Peucker). A
/// `threshold` of 0 requests Otsu auto-thresholding.
pub fn ai_trace_raster(
    node_id: Uuid,
    threshold: u8,
    simplify_tolerance: f32,
) -> Result<TraceResult> {
    if simplify_tolerance < 0.0 || !simplify_tolerance.is_finite() {
        return Err(DocumentBridgeError::InvalidArgument {
            argument: "simplify_tolerance".into(),
            value: simplify_tolerance.to_string(),
        });
    }
    let (rgba, width, height) = load_node_rgba(node_id)?;
    let opts = TraceOptions {
        threshold: if threshold == 0 {
            TraceThreshold::Auto
        } else {
            TraceThreshold::Fixed { value: threshold }
        },
        simplify_tolerance,
        min_path_points: 8,
        smooth: true,
    };
    let paths = trace_raster(&rgba, width, height, &opts)
        .map_err(|e| DocumentBridgeError::Internal(format!("trace_raster: {e}")))?;
    let path_count = paths.len();
    let closed_count = paths.iter().filter(|p| p.closed).count();

    let (group_id, path_ids) = with_workspace_mut(|ws| {
        let (parent_id, source_bounds) = {
            let n = ws
                .project
                .document
                .get_node(node_id)
                .ok_or(DocumentBridgeError::NodeNotFound(node_id))?;
            (n.parent_id, n.bounds)
        };

        let mut group = Node::new(NodeType::GroupLayer, "Traced paths");
        group.bounds = source_bounds;
        group.parent_id = parent_id;
        let group_id = ws
            .project
            .document
            .insert_node(group)
            .map_err(DocumentBridgeError::Document)?;

        let mut path_ids: Vec<Uuid> = Vec::with_capacity(paths.len());
        for (i, p) in paths.iter().enumerate() {
            let mut node = Node::new(NodeType::VectorLayer, format!("Traced path {}", i + 1));
            node.bounds = source_bounds;
            node.parent_id = Some(group_id);
            node.style = NodeStyle {
                fill: if p.closed {
                    FillStyle::Solid(RgbaColor::BLACK)
                } else {
                    FillStyle::None
                },
                stroke: Some(StrokeStyle {
                    color: RgbaColor::BLACK,
                    width: 1.0,
                    ..StrokeStyle::default()
                }),
                ..NodeStyle::default()
            };
            node.opacity = 1.0;
            node.blend_mode = BlendMode::Normal;
            let points: Vec<[f32; 2]> = p.points.iter().map(|pt| [pt.x, pt.y]).collect();
            node.metadata.insert(
                "traced_polyline".into(),
                serde_json::json!({ "points": points, "closed": p.closed }),
            );
            let path_id = ws
                .project
                .document
                .insert_node(node)
                .map_err(DocumentBridgeError::Document)?;
            path_ids.push(path_id);
        }
        ws.project.modified_at = Utc::now();
        let op = Operation::new(
            "user",
            "ai_trace_raster",
            serde_json::json!({ "source": node_id, "threshold": threshold }),
            serde_json::json!({ "group_id": group_id, "path_count": path_count }),
            vec![node_id, group_id],
        )
        .as_ai_generated();
        ws.project.execute_operation(op);
        Ok::<(Uuid, Vec<Uuid>), DocumentBridgeError>((group_id, path_ids))
    })?;

    Ok(TraceResult {
        group_node_id: group_id.to_string(),
        path_count,
        closed_path_count: closed_count,
        path_node_ids: path_ids.into_iter().map(|u| u.to_string()).collect(),
    })
}

// ---------------------------------------------------------------------------
// Block D Task 19 — AI icon-ify
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IconifyResultInfo {
    pub source_node_id: String,
    pub group_node_id: String,
    pub path_count: usize,
    pub stroke_width: f32,
    pub grid_size: u32,
}

/// Run the icon-ify pipeline against polylines stored on
/// `source_node_id` (Group whose children carry `traced_polyline`
/// metadata, or a single VectorLayer with the same metadata). The
/// normalised result is appended as a sibling group.
pub fn ai_iconify(source_node_id: Uuid, grid_size: u32) -> Result<IconifyResultInfo> {
    if !(8..=1024).contains(&grid_size) {
        return Err(DocumentBridgeError::InvalidArgument {
            argument: "grid_size".into(),
            value: grid_size.to_string(),
        });
    }
    let icon_paths = collect_icon_paths(source_node_id)?;
    let opts = IconifyOptions {
        grid_size,
        ..Default::default()
    };
    let result = iconify(&icon_paths, &opts)
        .map_err(|e| DocumentBridgeError::Internal(format!("iconify: {e}")))?;
    let path_count = result.paths.len();
    let stroke_width = result.recommended_stroke_width;

    let group_id = with_workspace_mut(|ws| {
        let parent_id = ws
            .project
            .document
            .get_node(source_node_id)
            .ok_or(DocumentBridgeError::NodeNotFound(source_node_id))?
            .parent_id;
        let mut group = Node::new(NodeType::GroupLayer, "Iconified");
        group.bounds = Bounds::new(0.0, 0.0, f64::from(grid_size), f64::from(grid_size));
        group.parent_id = parent_id;
        let group_id = ws
            .project
            .document
            .insert_node(group)
            .map_err(DocumentBridgeError::Document)?;
        for (i, p) in result.paths.iter().enumerate() {
            let mut node = Node::new(NodeType::VectorLayer, format!("Icon path {}", i + 1));
            node.bounds = Bounds::new(0.0, 0.0, f64::from(grid_size), f64::from(grid_size));
            node.parent_id = Some(group_id);
            node.style = NodeStyle {
                fill: FillStyle::None,
                stroke: Some(StrokeStyle {
                    color: RgbaColor::BLACK,
                    width: f64::from(stroke_width),
                    ..StrokeStyle::default()
                }),
                ..NodeStyle::default()
            };
            node.opacity = 1.0;
            node.blend_mode = BlendMode::Normal;
            let points: Vec<[f32; 2]> = p.points.iter().map(|pt| [pt.x, pt.y]).collect();
            node.metadata.insert(
                "traced_polyline".into(),
                serde_json::json!({ "points": points, "closed": p.closed }),
            );
            ws.project
                .document
                .insert_node(node)
                .map_err(DocumentBridgeError::Document)?;
        }
        ws.project.modified_at = Utc::now();
        let op = Operation::new(
            "user",
            "ai_iconify",
            serde_json::json!({ "source": source_node_id, "grid_size": grid_size }),
            serde_json::json!({ "group_id": group_id, "path_count": path_count }),
            vec![source_node_id, group_id],
        )
        .as_ai_generated();
        ws.project.execute_operation(op);
        Ok::<Uuid, DocumentBridgeError>(group_id)
    })?;

    Ok(IconifyResultInfo {
        source_node_id: source_node_id.to_string(),
        group_node_id: group_id.to_string(),
        path_count,
        stroke_width,
        grid_size,
    })
}

fn collect_icon_paths(source_node_id: Uuid) -> Result<Vec<IconPath>> {
    with_workspace(|ws| {
        let mut out: Vec<IconPath> = Vec::new();
        let push_node = |node: &Node, out: &mut Vec<IconPath>| {
            if let Some(meta) = node.metadata.get("traced_polyline") {
                if let Some(pts) = meta.get("points").and_then(|v| v.as_array()) {
                    let closed = meta
                        .get("closed")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false);
                    let pts: Vec<IconPoint> = pts
                        .iter()
                        .filter_map(|p| {
                            let a = p.as_array()?;
                            let x = a.first()?.as_f64()? as f32;
                            let y = a.get(1)?.as_f64()? as f32;
                            Some(IconPoint { x, y })
                        })
                        .collect();
                    if !pts.is_empty() {
                        out.push(IconPath {
                            points: pts,
                            closed,
                        });
                    }
                }
            }
        };
        let node = ws
            .project
            .document
            .get_node(source_node_id)
            .ok_or(DocumentBridgeError::NodeNotFound(source_node_id))?;
        match node.node_type {
            NodeType::GroupLayer | NodeType::LayoutFrame | NodeType::ComponentLayer => {
                for &child_id in &node.children {
                    if let Some(child) = ws.project.document.get_node(child_id) {
                        push_node(child, &mut out);
                    }
                }
            }
            _ => push_node(node, &mut out),
        }
        if out.is_empty() {
            return Err(DocumentBridgeError::Internal(format!(
                "iconify source {source_node_id} has no `traced_polyline` metadata"
            )));
        }
        Ok(out)
    })
}

// ---------------------------------------------------------------------------
// Block D Task 20 — AI batch alt-text
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchAltTextEntry {
    pub node_id: String,
    pub alt_text: String,
}

/// Walk every raster node descended from `page_id` and stamp a
/// deterministic alt-text into `node.metadata["alt_text"]` derived
/// from the layer name + image size. This is the offline-safe
/// fallback for users without a VLM installed; the LLM-backed path
/// lives in `kcreate_ai::alt_text` and is invoked separately when
/// the caller has confirmed a vision model is loaded.
pub fn ai_batch_alt_text(page_id: Uuid) -> Result<Vec<BatchAltTextEntry>> {
    with_workspace_mut(|ws| {
        let raster_ids: Vec<Uuid> = ws
            .project
            .document
            .descendants_of(page_id)
            .into_iter()
            .filter(|id| {
                ws.project
                    .document
                    .get_node(*id)
                    .is_some_and(|n| n.node_type == NodeType::RasterLayer)
            })
            .collect();
        if raster_ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut results = Vec::with_capacity(raster_ids.len());
        let mut affected = Vec::with_capacity(raster_ids.len());
        for id in raster_ids {
            let node = ws
                .project
                .document
                .get_node_mut(id)
                .ok_or(DocumentBridgeError::NodeNotFound(id))?;
            let w = node.bounds.width.round() as i64;
            let h = node.bounds.height.round() as i64;
            let name = node.name.clone();
            let alt = if w > 0 && h > 0 {
                format!("{name} ({w}x{h} image)")
            } else {
                format!("{name} (image)")
            };
            node.metadata
                .insert("alt_text".into(), serde_json::Value::String(alt.clone()));
            node.touch();
            results.push(BatchAltTextEntry {
                node_id: id.to_string(),
                alt_text: alt,
            });
            affected.push(id);
        }
        ws.project.modified_at = Utc::now();
        let op = Operation::new(
            "user",
            "ai_batch_alt_text",
            serde_json::Value::Null,
            serde_json::to_value(&results).unwrap_or(serde_json::Value::Null),
            affected,
        )
        .as_ai_generated();
        ws.project.execute_operation(op);
        Ok(results)
    })
}

// ---------------------------------------------------------------------------
// Block C Task 13 — PSD import (summary only; full project rewrite
// is a follow-up so the open project isn't accidentally clobbered)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSummary {
    pub source_path: String,
    pub width: u32,
    pub height: u32,
    pub node_count: usize,
    pub warnings: Vec<String>,
}

pub fn import_psd(path: &str) -> Result<ImportSummary> {
    let bytes = fs::read(path)
        .map_err(|e| DocumentBridgeError::Internal(format!("read psd `{path}`: {e}")))?;
    let imported = import_psd_bytes(&bytes)
        .map_err(|e| DocumentBridgeError::Internal(format!("import_psd: {e}")))?;
    Ok(ImportSummary {
        source_path: path.to_string(),
        width: imported.width,
        height: imported.height,
        node_count: imported.layers.len(),
        warnings: imported.warnings,
    })
}

// ---------------------------------------------------------------------------
// Block C Task 14 — EXIF extraction
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExifResult {
    pub fields: serde_json::Value,
    pub orientation: Option<u16>,
}

impl From<ExifMetadata> for ExifResult {
    fn from(m: ExifMetadata) -> Self {
        let orientation = m.orientation();
        Self {
            fields: serde_json::to_value(&m).unwrap_or(serde_json::Value::Null),
            orientation,
        }
    }
}

/// Read EXIF metadata from a JPEG / WebP / TIFF / HEIC byte slice.
/// Returns an empty `ExifResult` (rather than an error) when no
/// EXIF segment is present, so the renderer can call this
/// unconditionally without worrying about distinguishing absence
/// from corruption.
pub fn image_read_exif(bytes: &[u8]) -> Result<ExifResult> {
    match read_exif_from_bytes(bytes) {
        Ok(meta) => Ok(meta.into()),
        Err(ExifError::NoMetadata) => Ok(ExifResult {
            fields: serde_json::json!({ "primary": {}, "gps": {}, "rawHex": null }),
            orientation: None,
        }),
        Err(e) => Err(DocumentBridgeError::Internal(format!("read_exif: {e}"))),
    }
}

// ---------------------------------------------------------------------------
// Block C Task 15 — Penpot import
// ---------------------------------------------------------------------------

pub fn import_penpot(path: &str) -> Result<ImportSummary> {
    let bytes = fs::read(path)
        .map_err(|e| DocumentBridgeError::Internal(format!("read penpot `{path}`: {e}")))?;
    let imported = import_penpot_bytes(&bytes)
        .map_err(|e| DocumentBridgeError::Internal(format!("import_penpot: {e}")))?;
    // Derive a width/height from the first page's first frame if
    // present so the UI has *something* to display.
    let (width, height) = imported
        .pages
        .first()
        .and_then(|p| p.frames.first())
        .map_or((0, 0), |f| {
            (f.width.round() as u32, f.height.round() as u32)
        });
    let node_count: usize = imported
        .pages
        .iter()
        .map(|p| p.frames.iter().map(|f| 1 + f.shapes.len()).sum::<usize>())
        .sum();
    Ok(ImportSummary {
        source_path: path.to_string(),
        width,
        height,
        node_count,
        warnings: imported
            .warnings
            .into_iter()
            .map(|w| format!("{}: {}", w.kind, w.detail))
            .collect(),
    })
}

// ---------------------------------------------------------------------------
// Block C Task 16 — SVG → raster preview
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SvgPreviewInfo {
    pub width: u32,
    pub height: u32,
    pub png_bytes: Vec<u8>,
}

impl From<SvgPreview> for SvgPreviewInfo {
    fn from(p: SvgPreview) -> Self {
        Self {
            width: p.width,
            height: p.height,
            png_bytes: p.png_bytes,
        }
    }
}

/// Render the SVG bytes into a PNG that fits within
/// `(max_width, max_height)` and return the PNG payload + size.
pub fn export_svg_preview(
    svg_bytes: &[u8],
    max_width: u32,
    max_height: u32,
    transparent: bool,
) -> Result<SvgPreviewInfo> {
    let opts = SvgPreviewOptions {
        max_width,
        max_height,
        transparent,
    };
    let preview = svg_to_raster_preview(svg_bytes, &opts).map_err(|e: SvgPreviewError| {
        DocumentBridgeError::Internal(format!("svg_to_raster_preview: {e}"))
    })?;
    Ok(preview.into())
}

// ---------------------------------------------------------------------------
// Block C Task 17 — operation log filter
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationLogFilter {
    pub ai_only: bool,
    pub manual_only: bool,
    pub since: Option<String>,
    pub until: Option<String>,
    pub limit: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationInfo {
    pub id: String,
    pub timestamp: String,
    pub actor: String,
    pub command: String,
    pub affected_nodes: Vec<String>,
    pub ai_generated: bool,
    pub group_id: Option<String>,
    pub is_undo: bool,
}

impl From<&Operation> for OperationInfo {
    fn from(op: &Operation) -> Self {
        Self {
            id: op.id.to_string(),
            timestamp: op.timestamp.to_rfc3339(),
            actor: op.actor.clone(),
            command: op.command.clone(),
            affected_nodes: op.affected_nodes.iter().map(Uuid::to_string).collect(),
            ai_generated: op.ai_generated,
            group_id: op.group_id.map(|g| g.to_string()),
            is_undo: op.is_undo,
        }
    }
}

/// Paginated, filtered view of the open project's in-memory
/// operation log. Newest entries first. `limit == 0` returns up to
/// 200 rows.
pub fn operation_log_filter(filter: &OperationLogFilter) -> Result<Vec<OperationInfo>> {
    let limit = if filter.limit == 0 {
        200usize
    } else {
        filter.limit as usize
    };
    let since = filter
        .since
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(chrono::DateTime::parse_from_rfc3339)
        .transpose()
        .map_err(|e| DocumentBridgeError::InvalidArgument {
            argument: "since".into(),
            value: e.to_string(),
        })?
        .map(|dt| dt.with_timezone(&Utc));
    let until = filter
        .until
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(chrono::DateTime::parse_from_rfc3339)
        .transpose()
        .map_err(|e| DocumentBridgeError::InvalidArgument {
            argument: "until".into(),
            value: e.to_string(),
        })?
        .map(|dt| dt.with_timezone(&Utc));

    with_workspace(|ws| {
        let mut out = Vec::new();
        let ops: Vec<&Operation> = ws.project.operation_log.iter().collect();
        for op in ops.iter().rev() {
            if filter.ai_only && !op.ai_generated {
                continue;
            }
            if filter.manual_only && op.ai_generated {
                continue;
            }
            if let Some(s) = since {
                if op.timestamp < s {
                    continue;
                }
            }
            if let Some(u) = until {
                if op.timestamp > u {
                    continue;
                }
            }
            out.push(OperationInfo::from(*op));
            if out.len() >= limit {
                break;
            }
        }
        Ok(out)
    })
}

// ---------------------------------------------------------------------------
// Block E Task 27 — export validation
// ---------------------------------------------------------------------------

/// Run pre-flight validation against an export request. The
/// renderer surfaces the returned [`ExportValidationReport`] in the
/// Export panel before kicking off the actual export.
pub fn export_validate(request: ExportValidationRequest) -> ExportValidationReport {
    validate_export_request(&request)
}

// ---------------------------------------------------------------------------
// Block B Task 7 — brief → project
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BriefPlan {
    /// Artboard preset name (matches one of `kcreate_core::node::standard_presets()`).
    pub artboard_preset: String,
    /// Palette as `#rrggbb` strings.
    pub palette: Vec<String>,
    /// Starter layers; `kind` is one of `text` / `shape` / `image` / `group`.
    pub starter_layers: Vec<BriefStarterLayer>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BriefStarterLayer {
    pub name: String,
    pub kind: String,
    pub suggested_content: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BriefApplyResult {
    pub artboard_id: String,
    pub brand_kit_id: String,
    pub layer_ids: Vec<String>,
}

/// Apply a [`BriefPlan`] to the currently open project: create an
/// artboard at the requested preset, upsert a "Brief Kit" brand
/// kit, and add starter layers as children of the artboard.
pub fn brief_to_project(plan: &BriefPlan) -> Result<BriefApplyResult> {
    let preset = standard_presets()
        .into_iter()
        .find(|p| p.name == plan.artboard_preset)
        .ok_or_else(|| DocumentBridgeError::InvalidArgument {
            argument: "artboard_preset".into(),
            value: plan.artboard_preset.clone(),
        })?;
    let palette: Vec<RgbaColor> = plan
        .palette
        .iter()
        .filter_map(|s| RgbaColor::from_hex(s))
        .collect();
    if palette.is_empty() {
        return Err(DocumentBridgeError::InvalidArgument {
            argument: "palette".into(),
            value: format!("{:?}", plan.palette),
        });
    }
    with_workspace_mut(|ws| {
        // Artboard.
        let mut artboard = Node::new(NodeType::Artboard, &preset.name);
        artboard.bounds = Bounds::new(0.0, 0.0, preset.width, preset.height);
        let artboard_id = ws
            .project
            .document
            .insert_node(artboard)
            .map_err(DocumentBridgeError::Document)?;

        // Brand kit (upsert by name).
        let brand_kit_id = if let Some(kit) = ws
            .project
            .brand_kits
            .iter_mut()
            .find(|k| k.name == "Brief Kit")
        {
            kit.colors = palette
                .iter()
                .enumerate()
                .map(|(i, c)| NamedColor {
                    name: format!("Brief {}", i + 1),
                    color: *c,
                })
                .collect();
            kit.id
        } else {
            let mut kit = BrandKit::new("Brief Kit");
            kit.colors = palette
                .iter()
                .enumerate()
                .map(|(i, c)| NamedColor {
                    name: format!("Brief {}", i + 1),
                    color: *c,
                })
                .collect();
            let id = kit.id;
            ws.project.brand_kits.push(kit);
            id
        };

        // Starter layers.
        let mut layer_ids = Vec::with_capacity(plan.starter_layers.len());
        for layer in &plan.starter_layers {
            let kind = parse_starter_layer_kind(&layer.kind)?;
            let mut node = Node::new(kind, &layer.name);
            node.bounds = Bounds::new(
                preset.width * 0.1,
                preset.height * 0.1,
                preset.width * 0.8,
                preset.height * 0.1,
            );
            node.parent_id = Some(artboard_id);
            if kind == NodeType::TextLayer {
                let text = layer
                    .suggested_content
                    .clone()
                    .unwrap_or_else(|| layer.name.clone());
                let meta = TextLayerMeta {
                    text,
                    font_family: "Inter".to_string(),
                    font_size: 48.0,
                };
                node.metadata.insert(
                    TEXT_LAYER_METADATA_KEY.to_string(),
                    serde_json::to_value(&meta).unwrap_or(serde_json::Value::Null),
                );
            }
            let node_id = ws
                .project
                .document
                .insert_node(node)
                .map_err(DocumentBridgeError::Document)?;
            layer_ids.push(node_id);
        }
        ws.project.modified_at = Utc::now();
        let op = Operation::new(
            "user",
            "brief_to_project",
            serde_json::to_value(plan).unwrap_or(serde_json::Value::Null),
            serde_json::json!({ "artboard_id": artboard_id, "brand_kit_id": brand_kit_id }),
            std::iter::once(artboard_id)
                .chain(layer_ids.iter().copied())
                .collect(),
        )
        .as_ai_generated();
        ws.project.execute_operation(op);
        Ok(BriefApplyResult {
            artboard_id: artboard_id.to_string(),
            brand_kit_id: brand_kit_id.to_string(),
            layer_ids: layer_ids.iter().map(Uuid::to_string).collect(),
        })
    })
}

fn parse_starter_layer_kind(s: &str) -> Result<NodeType> {
    match s {
        "text" | "text_layer" => Ok(NodeType::TextLayer),
        "shape" | "vector" | "vector_layer" => Ok(NodeType::VectorLayer),
        "image" | "raster" | "raster_layer" => Ok(NodeType::RasterLayer),
        "group" | "group_layer" => Ok(NodeType::GroupLayer),
        other => Err(DocumentBridgeError::InvalidArgument {
            argument: "kind".into(),
            value: other.into(),
        }),
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Resolve the raw RGBA bytes (4 bpp) for a raster node by looking
/// up its [`RasterImageMeta::blob_hash`] in the project's blob
/// store and decoding the PNG/WebP/JPEG.
fn load_node_rgba(node_id: Uuid) -> Result<(Vec<u8>, u32, u32)> {
    let hash = with_workspace(|ws| {
        let n = ws
            .project
            .document
            .get_node(node_id)
            .ok_or(DocumentBridgeError::NodeNotFound(node_id))?;
        if n.node_type != NodeType::RasterLayer {
            return Err(DocumentBridgeError::WrongNodeType {
                expected: NodeType::RasterLayer,
                got: n.node_type,
            });
        }
        let meta: RasterImageMeta = raster_image_meta(n).ok_or_else(|| {
            DocumentBridgeError::Internal(format!(
                "raster layer {node_id} is missing a RasterImageMeta payload"
            ))
        })?;
        Ok(meta.blob_hash)
    })?;
    let bytes = with_workspace(|ws| crate::document::blob_load(ws, &hash))?;
    let img = image::load_from_memory(&bytes)
        .map_err(|e| DocumentBridgeError::Internal(format!("decode image `{hash}`: {e}")))?
        .to_rgba8();
    let (w, h) = (img.width(), img.height());
    Ok((img.into_raw(), w, h))
}

// ---------------------------------------------------------------------------
// Block E re-exports
// ---------------------------------------------------------------------------
//
// `lib.rs` wraps every Phase 9 N-API entry point against this
// module. The memory-pressure watchdog and autosave background
// thread live in their own files but are exposed here so the N-API
// layer only ever points at one Phase 9 namespace.

pub use crate::autosave::{
    autosave_dismiss_recovery, autosave_force_now, autosave_recover, autosave_recovery_available,
    autosave_start, autosave_status, autosave_stop, AutosaveStatus,
};
pub use crate::perf::{
    drain_memory_events, memory_watchdog_start, memory_watchdog_stop, MemoryPressureEvent,
};

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_orientation_horizontal_vertical_only() {
        assert!(matches!(
            parse_orientation("horizontal").unwrap(),
            GuideOrientation::Horizontal
        ));
        assert!(matches!(
            parse_orientation("vertical").unwrap(),
            GuideOrientation::Vertical
        ));
        assert!(parse_orientation("diagonal").is_err());
    }

    #[test]
    fn parse_align_covers_canonical_names() {
        for name in [
            "left",
            "right",
            "top",
            "bottom",
            "center_horizontal",
            "centerHorizontal",
            "center_vertical",
            "centerVertical",
        ] {
            parse_align(name).unwrap_or_else(|_| panic!("rejected {name}"));
        }
        assert!(parse_align("diagonal").is_err());
    }

    #[test]
    fn parse_distribute_axis_only() {
        assert!(matches!(
            parse_distribute("horizontal").unwrap(),
            DistributeAxis::Horizontal
        ));
        assert!(matches!(
            parse_distribute("vertical").unwrap(),
            DistributeAxis::Vertical
        ));
        assert!(parse_distribute("diagonal").is_err());
    }

    #[test]
    fn parse_starter_layer_kind_accepts_canonical_names() {
        assert_eq!(
            parse_starter_layer_kind("text").unwrap(),
            NodeType::TextLayer
        );
        assert_eq!(
            parse_starter_layer_kind("shape").unwrap(),
            NodeType::VectorLayer
        );
        assert_eq!(
            parse_starter_layer_kind("image").unwrap(),
            NodeType::RasterLayer
        );
        assert_eq!(
            parse_starter_layer_kind("group").unwrap(),
            NodeType::GroupLayer
        );
        assert!(parse_starter_layer_kind("widget").is_err());
    }

    #[test]
    fn operation_log_filter_default_limit() {
        let f = OperationLogFilter::default();
        assert_eq!(f.limit, 0);
        assert!(!f.ai_only);
        assert!(!f.manual_only);
    }
}
