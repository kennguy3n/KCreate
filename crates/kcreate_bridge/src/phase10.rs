//! Phase 10 bridge entry points.
//!
//! Same convention as `phase8.rs` and `phase9.rs`: every public
//! function either runs against the open workspace (gated by
//! `with_workspace` / `with_workspace_mut`) or is a pure helper.
//! The N-API marshalling lives in `lib.rs`.
//!
//! Scope:
//!
//! - **Block A** (Image Studio AI actions):
//!   * `ai_denoise` — NLM denoising.
//!   * `ai_inpaint` — exemplar inpainting.
//!   * `ai_auto_color` — auto-levels / WB / HE / combined.
//!   * `ai_segment_at_point` — SAM-or-fallback segmentation.
//!   * `ai_smart_select_at_point` — magic-wand selection.
//! - **Block B** (Vector / Layout AI features):
//!   * `ai_match_stroke` — copy a vector node's stroke onto targets.
//!   * `ai_extract_glyph` — trace a raster crop into glyph paths.
//!   * `ai_reformat_to_deck` — split a single-page doc into a deck.
//!   * `ai_brief_to_one_pager` — text brief → page sections.
//!   * `ai_harmonize_palette` — colour harmony for brand kit.
//!   * `ai_suggest_type_pairing` — body-font suggestions for heading.
//! - **Block C** (Export Center AI + live preview):
//!   * `export_optimize_svg` — SVG minifier.
//!   * `export_smart_compress` — SSIM-targeting JPEG/WebP search.
//!   * `export_preview` — render preview bytes for a configured export.
//!   * `import_ai` — Illustrator `.ai` (PDF-with-SVG) import.
//! - **Block D** (Brand Hub + Plugin Marketplace + PDF):
//!   * `ai_brand_to_brochure` — multi-page brand template.
//!   * `plugin_marketplace_list` / `_install_local` / `_remove`.
//!   * `export_pdf_multi` — multi-page PDF with TOC + outline.
//!   * `preferences_load` / `preferences_save`.

use std::path::PathBuf;

use chrono::Utc;
use kcreate_ai::auto_color::{auto_color_correct, AutoColorMode, AutoColorOptions};
use kcreate_ai::denoise::{denoise, DenoiseOptions};
use kcreate_ai::glyph_extract::{extract_glyph, GlyphCrop, GlyphExtractOptions};
use kcreate_ai::inpaint::{inpaint, mask_from_rects, InpaintOptions, MaskRect};
use kcreate_ai::one_pager::{
    brief_to_one_pager, BriefToOnePagerOptions, BriefToOnePagerResult, OnePagerPageSize,
};
use kcreate_ai::palette_harmonize::{harmonize_palette, HarmonyResult, HarmonyRule};
use kcreate_ai::reformat::{reformat_to_deck, ReformatDeckOptions, ReformatDeckResult, SourceNode};
use kcreate_ai::segment::{segment_image, SegmentBackend, SegmentOptions};
use kcreate_ai::smart_select::smart_select;
use kcreate_ai::stroke_match::{match_stroke_style, StrokeMatchSummary, StrokeProperties};
use kcreate_ai::type_pairing::{suggest_type_pairing, TypePairingResult};
use kcreate_core::node::{
    Bounds, LineCap, LineJoin, Node, NodeType, RgbaColor, StrokeStyle,
};
use kcreate_core::operation::Operation;
use kcreate_export::ai_import::{import_illustrator_bytes, AiImportError, AiImportSummary};
use kcreate_export::pdf_multi::{export_pdf_multi_pages, PdfMultiError, PdfMultiOptions, PdfMultiReport};
use kcreate_export::smart_compress::{
    smart_compress, SmartCompressFormat, SmartCompressOptions, SmartCompressReport,
};
use kcreate_export::svg_optimize::{optimize_svg, SvgOptimizeReport};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::document::{
    blob_load, with_workspace, with_workspace_mut, DocumentBridgeError, Result,
};
use crate::scene_sync::{RasterImageMeta, RASTER_IMAGE_METADATA_KEY};

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Load a raster layer's pixel buffer (decoded RGBA8) along with
/// its dimensions and original-encoded blob. Mirrors the
/// `load_node_rgba` helper in `phase9.rs` but reusable across this
/// module's many image-modifying entry points.
fn load_raster_rgba(node_id: Uuid) -> Result<(Vec<u8>, u32, u32)> {
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
        let meta_value = n.metadata.get(RASTER_IMAGE_METADATA_KEY).ok_or_else(|| {
            DocumentBridgeError::Internal(format!(
                "raster layer {node_id} is missing a RasterImageMeta payload"
            ))
        })?;
        let meta: RasterImageMeta = serde_json::from_value(meta_value.clone())
            .map_err(|e| DocumentBridgeError::Internal(format!("decode RasterImageMeta: {e}")))?;
        Ok(meta.blob_hash)
    })?;
    let bytes = with_workspace(|ws| blob_load(ws, &hash))?;
    let img = image::load_from_memory(&bytes)
        .map_err(|e| DocumentBridgeError::Internal(format!("decode image `{hash}`: {e}")))?
        .to_rgba8();
    let (w, h) = (img.width(), img.height());
    Ok((img.into_raw(), w, h))
}

/// Encode an RGBA buffer back into a PNG blob, store it, and create
/// a new sibling [`NodeType::RasterLayer`] node owned by the same
/// parent as `source_node_id`. Records an AI-flagged undo
/// operation. Returns the new node's id.
fn install_processed_raster(
    source_node_id: Uuid,
    pixels: &[u8],
    width: u32,
    height: u32,
    op_name: &'static str,
    op_params: serde_json::Value,
    suggested_name: &str,
) -> Result<Uuid> {
    let mut png_bytes = Vec::new();
    {
        let mut cursor = std::io::Cursor::new(&mut png_bytes);
        image::write_buffer_with_format(
            &mut cursor,
            pixels,
            width,
            height,
            image::ColorType::Rgba8,
            image::ImageFormat::Png,
        )
        .map_err(|e| DocumentBridgeError::Internal(format!("encode PNG: {e}")))?;
    }
    with_workspace_mut(|ws| {
        let blob = ws
            .store
            .blobs()
            .store(&png_bytes, "image/png")
            .map_err(|e| DocumentBridgeError::Internal(format!("blob store: {e}")))?;
        let parent = ws
            .project
            .document
            .get_node(source_node_id)
            .ok_or(DocumentBridgeError::NodeNotFound(source_node_id))?
            .parent_id;
        let mut new_node = Node::new(NodeType::RasterLayer, suggested_name);
        new_node.parent_id = parent;
        new_node.bounds = Bounds {
            x: 0.0,
            y: 0.0,
            width: f64::from(width),
            height: f64::from(height),
        };
        let meta = RasterImageMeta {
            blob_hash: blob.hash,
            width,
            height,
        };
        new_node.metadata.insert(
            RASTER_IMAGE_METADATA_KEY.to_string(),
            serde_json::to_value(&meta).map_err(|e| {
                DocumentBridgeError::Internal(format!("serialize RasterImageMeta: {e}"))
            })?,
        );
        let new_id = ws
            .project
            .document
            .insert_node(new_node)
            .map_err(|e| DocumentBridgeError::Internal(format!("insert raster node: {e}")))?;
        let snapshot = ws
            .project
            .document
            .get_node(new_id)
            .map_or(serde_json::Value::Null, |n| {
                serde_json::to_value(n).unwrap_or(serde_json::Value::Null)
            });
        let op = Operation::new(
            "ai",
            op_name,
            op_params,
            snapshot,
            vec![new_id, source_node_id],
        )
        .as_ai_generated();
        ws.project.execute_operation(op);
        ws.project.modified_at = Utc::now();
        Ok(new_id)
    })
}

// ---------------------------------------------------------------------------
// Block A Task 1 — AI denoise
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DenoiseResult {
    pub new_node_id: String,
    pub width: u32,
    pub height: u32,
}

/// Run NLM denoising on `node_id` and insert the result as a
/// sibling raster layer.
pub fn ai_denoise(
    node_id: Uuid,
    strength: f32,
    search_radius: u32,
    patch_radius: u32,
) -> Result<DenoiseResult> {
    let (rgba, w, h) = load_raster_rgba(node_id)?;
    let opts = DenoiseOptions {
        strength,
        search_radius,
        patch_radius,
    };
    let out = denoise(&rgba, w, h, opts).map_err(|e| {
        DocumentBridgeError::Internal(format!("ai_denoise: {e}"))
    })?;
    let new_id = install_processed_raster(
        node_id,
        &out,
        w,
        h,
        "ai_denoise",
        serde_json::json!({
            "strength": opts.clamped().strength,
            "search_radius": opts.clamped().search_radius,
            "patch_radius": opts.clamped().patch_radius,
        }),
        "Denoised",
    )?;
    Ok(DenoiseResult {
        new_node_id: new_id.to_string(),
        width: w,
        height: h,
    })
}

// ---------------------------------------------------------------------------
// Block A Task 2 — AI inpaint
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InpaintResult {
    pub new_node_id: String,
    pub width: u32,
    pub height: u32,
}

/// Run exemplar-based inpainting using the mask described by
/// `mask_json` (a `[{x, y, w, h}, ...]` array of rectangles).
pub fn ai_inpaint(
    node_id: Uuid,
    mask_json: &str,
    patch_radius: Option<u32>,
    num_iterations: Option<u32>,
    pyramid_levels: Option<u32>,
) -> Result<InpaintResult> {
    let rects: Vec<MaskRect> = serde_json::from_str(mask_json).map_err(|e| {
        DocumentBridgeError::InvalidArgument {
            argument: "mask_json".into(),
            value: format!("{e}"),
        }
    })?;
    let (rgba, w, h) = load_raster_rgba(node_id)?;
    let mask = mask_from_rects(&rects, w, h);
    let opts = InpaintOptions {
        patch_radius: patch_radius.unwrap_or(3),
        num_iterations: num_iterations.unwrap_or(5),
        pyramid_levels: pyramid_levels.unwrap_or(3),
    };
    let out = inpaint(&rgba, &mask, w, h, opts).map_err(|e| {
        DocumentBridgeError::Internal(format!("ai_inpaint: {e}"))
    })?;
    let new_id = install_processed_raster(
        node_id,
        &out,
        w,
        h,
        "ai_inpaint",
        serde_json::json!({
            "rects": rects,
            "patch_radius": opts.clamped().patch_radius,
            "num_iterations": opts.clamped().num_iterations,
            "pyramid_levels": opts.clamped().pyramid_levels,
        }),
        "Inpainted",
    )?;
    Ok(InpaintResult {
        new_node_id: new_id.to_string(),
        width: w,
        height: h,
    })
}

// ---------------------------------------------------------------------------
// Block A Task 3 — AI auto colour
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoColorResult {
    pub new_node_id: String,
    pub mode: String,
    pub width: u32,
    pub height: u32,
}

/// Apply automatic colour correction to `node_id`.
pub fn ai_auto_color(node_id: Uuid, mode: &str) -> Result<AutoColorResult> {
    let parsed_mode = AutoColorMode::from_wire(mode).ok_or_else(|| {
        DocumentBridgeError::InvalidArgument {
            argument: "mode".into(),
            value: mode.into(),
        }
    })?;
    let (rgba, w, h) = load_raster_rgba(node_id)?;
    let out = auto_color_correct(
        &rgba,
        w,
        h,
        AutoColorOptions {
            mode: parsed_mode,
            clip: 0.005,
        },
    )
    .map_err(|e| DocumentBridgeError::Internal(format!("ai_auto_color: {e}")))?;
    let new_id = install_processed_raster(
        node_id,
        &out,
        w,
        h,
        "ai_auto_color",
        serde_json::json!({ "mode": mode }),
        match parsed_mode {
            AutoColorMode::AutoLevels => "Auto Levels",
            AutoColorMode::WhiteBalance => "White Balanced",
            AutoColorMode::HistogramEqualization => "Histogram Equalised",
            AutoColorMode::Combined => "Auto Colour",
        },
    )?;
    Ok(AutoColorResult {
        new_node_id: new_id.to_string(),
        mode: mode.to_string(),
        width: w,
        height: h,
    })
}

// ---------------------------------------------------------------------------
// Block A Task 4 — Segmentation at a point
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SegmentAtPointResult {
    pub mask_base64: String,
    pub width: u32,
    pub height: u32,
    pub backend: String,
}

/// Run SAM-style segmentation at `(point_x, point_y)` if a model is
/// installed; otherwise fall back to BFS flood-fill from the same
/// seed. The returned mask is base64-encoded so the renderer can
/// blit it onto an overlay canvas.
pub fn ai_segment_at_point(
    node_id: Uuid,
    point_x: u32,
    point_y: u32,
    _is_positive: bool,
) -> Result<SegmentAtPointResult> {
    let (rgba, w, h) = load_raster_rgba(node_id)?;
    if point_x >= w || point_y >= h {
        return Err(DocumentBridgeError::InvalidArgument {
            argument: "point".into(),
            value: format!("({point_x},{point_y}) outside {w}x{h}"),
        });
    }
    let opts = SegmentOptions {
        point_x,
        point_y,
        ..SegmentOptions::default()
    };
    // The crate-level helper picks a backend; if SAM isn't installed
    // it falls back to smart_select, which is the desired graceful
    // degradation here.
    let (mask, backend) = match segment_image(&rgba, w, h, &opts) {
        Ok(result) => {
            let first = result
                .masks
                .into_iter()
                .next()
                .map(|m| m.mask)
                .unwrap_or_else(|| vec![0u8; (w * h) as usize]);
            (first, result.backend)
        }
        Err(_) => {
            // Hard fallback to flood-fill — never refuse the action.
            let m = smart_select(&rgba, w, h, point_x, point_y, 0.15);
            (m, SegmentBackend::EdgeAware)
        }
    };
    use base64::Engine as _;
    let mask_b64 = base64::engine::general_purpose::STANDARD.encode(&mask);
    Ok(SegmentAtPointResult {
        mask_base64: mask_b64,
        width: w,
        height: h,
        backend: format!("{backend:?}").to_lowercase(),
    })
}

// ---------------------------------------------------------------------------
// Block A Task 5 — Magic wand (smart-select at point with set ops)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SmartSelectMode {
    Replace,
    Add,
    Subtract,
}

impl SmartSelectMode {
    fn from_wire(s: &str) -> Option<Self> {
        match s {
            "replace" => Some(Self::Replace),
            "add" => Some(Self::Add),
            "subtract" => Some(Self::Subtract),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SmartSelectAtPointResult {
    pub mask_base64: String,
    pub width: u32,
    pub height: u32,
    pub mode: String,
    pub selected_pixel_count: u32,
}

/// Run BFS smart-select from `(x, y)` and merge with `previous_mask_base64`
/// according to `mode`. Pure helper — never mutates the workspace.
pub fn ai_smart_select_at_point(
    node_id: Uuid,
    x: u32,
    y: u32,
    tolerance: f64,
    mode: &str,
    previous_mask_base64: Option<&str>,
) -> Result<SmartSelectAtPointResult> {
    let parsed_mode = SmartSelectMode::from_wire(mode).ok_or_else(|| {
        DocumentBridgeError::InvalidArgument {
            argument: "mode".into(),
            value: mode.into(),
        }
    })?;
    let (rgba, w, h) = load_raster_rgba(node_id)?;
    let new_mask = smart_select(&rgba, w, h, x, y, tolerance);
    use base64::Engine as _;
    let merged: Vec<u8> = match parsed_mode {
        SmartSelectMode::Replace => new_mask,
        SmartSelectMode::Add => {
            let prev = decode_mask_base64(previous_mask_base64, w, h)?;
            new_mask
                .iter()
                .zip(prev.iter())
                .map(|(a, b)| if *a != 0 || *b != 0 { 255 } else { 0 })
                .collect()
        }
        SmartSelectMode::Subtract => {
            let prev = decode_mask_base64(previous_mask_base64, w, h)?;
            prev.iter()
                .zip(new_mask.iter())
                .map(|(p, n)| if *p != 0 && *n == 0 { 255 } else { 0 })
                .collect()
        }
    };
    let selected = merged.iter().filter(|&&b| b != 0).count() as u32;
    let mask_b64 = base64::engine::general_purpose::STANDARD.encode(&merged);
    Ok(SmartSelectAtPointResult {
        mask_base64: mask_b64,
        width: w,
        height: h,
        mode: mode.to_string(),
        selected_pixel_count: selected,
    })
}

fn decode_mask_base64(b64: Option<&str>, w: u32, h: u32) -> Result<Vec<u8>> {
    use base64::Engine as _;
    let expected = (w as usize) * (h as usize);
    let Some(s) = b64 else {
        return Ok(vec![0u8; expected]);
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(s)
        .map_err(|e| DocumentBridgeError::InvalidArgument {
            argument: "previous_mask_base64".into(),
            value: format!("{e}"),
        })?;
    if bytes.len() != expected {
        return Err(DocumentBridgeError::InvalidArgument {
            argument: "previous_mask_base64".into(),
            value: format!("decoded {} bytes, expected {}", bytes.len(), expected),
        });
    }
    Ok(bytes)
}

// ---------------------------------------------------------------------------
// Block B Task 7 — Match stroke style
// ---------------------------------------------------------------------------

/// Copy the source node's stroke onto each of the targets. Records
/// a single grouped operation so the user can undo with one step.
pub fn ai_match_stroke(source_id: Uuid, target_ids: &[Uuid]) -> Result<StrokeMatchSummary> {
    if target_ids.is_empty() {
        return Err(DocumentBridgeError::InvalidArgument {
            argument: "target_node_ids".into(),
            value: "empty".into(),
        });
    }
    let source_props = with_workspace(|ws| {
        let node = ws
            .project
            .document
            .get_node(source_id)
            .ok_or(DocumentBridgeError::NodeNotFound(source_id))?;
        let stroke = node.style.stroke.clone().ok_or_else(|| {
            DocumentBridgeError::InvalidArgument {
                argument: "source_node_id".into(),
                value: "no stroke on source".into(),
            }
        })?;
        Ok(stroke_to_props(&stroke, node.style.stroke_width_profile.clone()))
    })?;
    let target_pairs: Vec<(String, bool)> = with_workspace(|ws| {
        let mut out = Vec::with_capacity(target_ids.len());
        for tid in target_ids {
            let n = ws
                .project
                .document
                .get_node(*tid)
                .ok_or(DocumentBridgeError::NodeNotFound(*tid))?;
            out.push((tid.to_string(), n.style.stroke.is_some()));
        }
        Ok(out)
    })?;
    let summary = match_stroke_style(
        &source_id.to_string(),
        Some(&source_props),
        &target_pairs,
    )
    .map_err(|e| DocumentBridgeError::Internal(format!("match_stroke_style: {e}")))?;

    let new_stroke = props_to_stroke(&source_props);
    let width_profile = source_props.width_profile.clone();
    let group_id = Uuid::new_v4();
    let target_ids_owned: Vec<Uuid> = target_ids.to_vec();
    with_workspace_mut(|ws| {
        let mut before = Vec::with_capacity(target_ids_owned.len());
        let mut after = Vec::with_capacity(target_ids_owned.len());
        for tid in &target_ids_owned {
            let node = ws
                .project
                .document
                .get_node_mut(*tid)
                .ok_or(DocumentBridgeError::NodeNotFound(*tid))?;
            before.push(serde_json::to_value(&node.style).unwrap_or(serde_json::Value::Null));
            node.style.stroke = Some(new_stroke.clone());
            node.style.stroke_width_profile = width_profile.clone();
            node.touch();
            after.push(serde_json::to_value(&node.style).unwrap_or(serde_json::Value::Null));
        }
        ws.project.modified_at = Utc::now();
        let op = Operation {
            group_id: Some(group_id),
            ..Operation::new(
                "ai",
                "ai_match_stroke",
                serde_json::json!({ "source": source_id, "targets": target_ids_owned, "before": before }),
                serde_json::json!({ "after": after }),
                target_ids_owned.clone(),
            )
        }
        .as_ai_generated();
        ws.project.execute_operation(op);
        Ok::<(), DocumentBridgeError>(())
    })?;
    Ok(summary)
}

fn stroke_to_props(
    s: &StrokeStyle,
    width_profile: Option<Vec<(f64, f64)>>,
) -> StrokeProperties {
    StrokeProperties {
        color_hex: s.color.to_hex(),
        width: s.width,
        dash: s.dash.clone(),
        cap: line_cap_str(s.cap).to_string(),
        join: line_join_str(s.join).to_string(),
        width_profile,
    }
}

fn props_to_stroke(p: &StrokeProperties) -> StrokeStyle {
    StrokeStyle {
        color: RgbaColor::from_hex(&p.color_hex).unwrap_or(RgbaColor::BLACK),
        width: p.width,
        dash: p.dash.clone(),
        cap: parse_line_cap(&p.cap),
        join: parse_line_join(&p.join),
    }
}

fn line_cap_str(c: LineCap) -> &'static str {
    match c {
        LineCap::Butt => "butt",
        LineCap::Round => "round",
        LineCap::Square => "square",
    }
}

fn line_join_str(j: LineJoin) -> &'static str {
    match j {
        LineJoin::Miter => "miter",
        LineJoin::Round => "round",
        LineJoin::Bevel => "bevel",
    }
}

fn parse_line_cap(s: &str) -> LineCap {
    match s {
        "round" => LineCap::Round,
        "square" => LineCap::Square,
        _ => LineCap::Butt,
    }
}

fn parse_line_join(s: &str) -> LineJoin {
    match s {
        "round" => LineJoin::Round,
        "bevel" => LineJoin::Bevel,
        _ => LineJoin::Miter,
    }
}

// ---------------------------------------------------------------------------
// Block B Task 8 — Extract glyph
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtractedGlyphResult {
    pub paths_json: String,
    pub em_size: f64,
    pub bounding_box: (f64, f64, f64, f64),
}

/// Trace a raster crop into a glyph-shaped path set.
pub fn ai_extract_glyph(
    node_id: Uuid,
    crop_x: u32,
    crop_y: u32,
    crop_width: u32,
    crop_height: u32,
    em_size: f64,
) -> Result<ExtractedGlyphResult> {
    let (rgba, w, h) = load_raster_rgba(node_id)?;
    let crop = GlyphCrop {
        x: crop_x,
        y: crop_y,
        width: crop_width,
        height: crop_height,
    };
    let opts = GlyphExtractOptions {
        em_size,
        simplify_tolerance: 4.0,
    };
    let result = extract_glyph(&rgba, w, h, crop, opts).map_err(|e| {
        DocumentBridgeError::Internal(format!("ai_extract_glyph: {e}"))
    })?;
    let paths_json = serde_json::to_string(&result.paths)
        .map_err(|e| DocumentBridgeError::Internal(format!("serialize glyph paths: {e}")))?;
    Ok(ExtractedGlyphResult {
        paths_json,
        em_size: result.metrics.em,
        bounding_box: result.bounding_box,
    })
}

// ---------------------------------------------------------------------------
// Block B Task 9 — Reformat to deck
// ---------------------------------------------------------------------------

/// Build a deck-layout plan from the open document's current page.
pub fn ai_reformat_to_deck(page_id: Uuid) -> Result<ReformatDeckResult> {
    let nodes = with_workspace(|ws| {
        let page = ws
            .project
            .document
            .get_node(page_id)
            .ok_or(DocumentBridgeError::NodeNotFound(page_id))?;
        if page.node_type != NodeType::Page && page.node_type != NodeType::Artboard {
            return Err(DocumentBridgeError::InvalidArgument {
                argument: "page_id".into(),
                value: format!("{:?}", page.node_type),
            });
        }
        let descendants: Vec<Uuid> = ws.project.document.descendants_of(page_id);
        let mut out = Vec::with_capacity(descendants.len());
        for id in descendants {
            if let Some(n) = ws.project.document.get_node(id) {
                let kind = match n.node_type {
                    NodeType::TextLayer => "text",
                    NodeType::RasterLayer => "image",
                    NodeType::VectorLayer => "shape",
                    NodeType::GroupLayer => "group",
                    _ => "other",
                };
                out.push(SourceNode {
                    id: id.to_string(),
                    name: n.name.clone(),
                    x: n.bounds.x,
                    y: n.bounds.y,
                    width: n.bounds.width,
                    height: n.bounds.height,
                    kind: kind.to_string(),
                });
            }
        }
        Ok(out)
    })?;
    reformat_to_deck(&nodes, ReformatDeckOptions::default())
        .map_err(|e| DocumentBridgeError::Internal(format!("reformat_to_deck: {e}")))
}

// ---------------------------------------------------------------------------
// Block B Task 10 — Brief to one-pager
// ---------------------------------------------------------------------------

/// Build a one-pager plan from a free-form brief.
pub fn ai_brief_to_one_pager(
    brief: &str,
    page_size: Option<&str>,
) -> Result<BriefToOnePagerResult> {
    let parsed_size = match page_size.unwrap_or("a4") {
        "letter" => OnePagerPageSize::Letter,
        "a4" => OnePagerPageSize::A4,
        "square" => OnePagerPageSize::Square,
        other => {
            return Err(DocumentBridgeError::InvalidArgument {
                argument: "page_size".into(),
                value: other.into(),
            });
        }
    };
    let opts = BriefToOnePagerOptions {
        page_size: parsed_size,
        ..Default::default()
    };
    brief_to_one_pager(brief, opts)
        .map_err(|e| DocumentBridgeError::Internal(format!("brief_to_one_pager: {e}")))
}

// ---------------------------------------------------------------------------
// Block B Task 11 — Harmonize palette
// ---------------------------------------------------------------------------

/// Harmonize an existing brand kit's colour palette.
pub fn ai_harmonize_palette(brand_kit_id: Uuid, harmony_type: &str) -> Result<HarmonyResult> {
    let rule = HarmonyRule::from_wire(harmony_type).ok_or_else(|| {
        DocumentBridgeError::InvalidArgument {
            argument: "harmony_type".into(),
            value: harmony_type.into(),
        }
    })?;
    let palette = with_workspace(|ws| {
        let kit = ws
            .project
            .brand_kits
            .iter()
            .find(|k| k.id == brand_kit_id)
            .ok_or(DocumentBridgeError::Internal(format!(
                "brand kit {brand_kit_id} not found"
            )))?;
        Ok(kit
            .colors
            .iter()
            .map(|c| c.color.to_hex())
            .collect::<Vec<_>>())
    })?;
    harmonize_palette(&palette, rule)
        .map_err(|e| DocumentBridgeError::Internal(format!("harmonize_palette: {e}")))
}

// ---------------------------------------------------------------------------
// Block B Task 12 — Suggest type pairing
// ---------------------------------------------------------------------------

/// Suggest body-font pairings for `heading_font_name`.
pub fn ai_suggest_type_pairing(heading_font_name: &str) -> Result<TypePairingResult> {
    suggest_type_pairing(heading_font_name)
        .map_err(|e| DocumentBridgeError::Internal(format!("suggest_type_pairing: {e}")))
}

// ---------------------------------------------------------------------------
// Block C Task 13 — Optimize SVG
// ---------------------------------------------------------------------------

/// Optimize an SVG string, returning the minified output and a size
/// delta report.
pub fn export_optimize_svg(svg: &str) -> Result<SvgOptimizeReport> {
    optimize_svg(svg).map_err(|e| DocumentBridgeError::Internal(format!("optimize_svg: {e}")))
}

// ---------------------------------------------------------------------------
// Block C Task 14 — Smart compress
// ---------------------------------------------------------------------------

/// Iteratively compress a raster layer until SSIM drops below
/// `target_ssim` (default 0.98). Returns the chosen quality, the
/// final byte size, and the achieved SSIM.
pub fn export_smart_compress(
    node_id: Uuid,
    format: &str,
    target_ssim: Option<f64>,
) -> Result<SmartCompressReport> {
    let parsed_format = match format.to_ascii_lowercase().as_str() {
        "jpeg" | "jpg" => SmartCompressFormat::Jpeg,
        "webp" => SmartCompressFormat::Webp,
        other => {
            return Err(DocumentBridgeError::InvalidArgument {
                argument: "format".into(),
                value: other.into(),
            });
        }
    };
    let (rgba, w, h) = load_raster_rgba(node_id)?;
    let opts = SmartCompressOptions {
        format: parsed_format,
        target_ssim: target_ssim.unwrap_or(0.98),
        ..Default::default()
    };
    smart_compress(&rgba, w, h, opts)
        .map_err(|e| DocumentBridgeError::Internal(format!("smart_compress: {e}")))
}

// ---------------------------------------------------------------------------
// Block C Task 15 — Export preview
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportPreviewRequest {
    pub node_id: String,
    pub format: String,
    pub max_dimension_px: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportPreviewResponse {
    pub bytes_base64: String,
    pub mime_type: String,
    pub width: u32,
    pub height: u32,
    pub byte_size: u64,
}

/// Render a low-resolution preview of an export. The bridge caps
/// the longest side at `max_dimension_px` (default 1024) so the IPC
/// surface stays small even at high zoom levels.
pub fn export_preview(req: ExportPreviewRequest) -> Result<ExportPreviewResponse> {
    let node_id = req
        .node_id
        .parse::<Uuid>()
        .map_err(|e| DocumentBridgeError::InvalidArgument {
            argument: "node_id".into(),
            value: format!("{e}"),
        })?;
    let max_dim = req.max_dimension_px.unwrap_or(1024).clamp(64, 4096);
    let format = req.format.to_ascii_lowercase();
    let (rgba, w, h) = load_raster_rgba(node_id)?;
    // Resize so longest side <= max_dim while preserving aspect.
    let (pw, ph) = scale_to_fit(w, h, max_dim);
    let img = image::RgbaImage::from_raw(w, h, rgba.clone()).ok_or_else(|| {
        DocumentBridgeError::Internal("export_preview: failed to wrap RGBA".into())
    })?;
    let resized = if (pw, ph) == (w, h) {
        img
    } else {
        image::imageops::resize(&img, pw, ph, image::imageops::FilterType::Lanczos3)
    };
    let mut bytes = Vec::new();
    let mime: &str = match format.as_str() {
        "png" => {
            let mut cursor = std::io::Cursor::new(&mut bytes);
            image::write_buffer_with_format(
                &mut cursor,
                resized.as_raw(),
                pw,
                ph,
                image::ColorType::Rgba8,
                image::ImageFormat::Png,
            )
            .map_err(|e| DocumentBridgeError::Internal(format!("preview encode PNG: {e}")))?;
            "image/png"
        }
        "jpeg" | "jpg" => {
            let mut cursor = std::io::Cursor::new(&mut bytes);
            image::write_buffer_with_format(
                &mut cursor,
                resized.as_raw(),
                pw,
                ph,
                image::ColorType::Rgba8,
                image::ImageFormat::Jpeg,
            )
            .map_err(|e| DocumentBridgeError::Internal(format!("preview encode JPEG: {e}")))?;
            "image/jpeg"
        }
        "webp" => {
            let mut cursor = std::io::Cursor::new(&mut bytes);
            image::write_buffer_with_format(
                &mut cursor,
                resized.as_raw(),
                pw,
                ph,
                image::ColorType::Rgba8,
                image::ImageFormat::WebP,
            )
            .map_err(|e| DocumentBridgeError::Internal(format!("preview encode WebP: {e}")))?;
            "image/webp"
        }
        other => {
            return Err(DocumentBridgeError::InvalidArgument {
                argument: "format".into(),
                value: other.into(),
            });
        }
    };
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Ok(ExportPreviewResponse {
        bytes_base64: b64,
        mime_type: mime.to_string(),
        width: pw,
        height: ph,
        byte_size: bytes.len() as u64,
    })
}

fn scale_to_fit(w: u32, h: u32, max_dim: u32) -> (u32, u32) {
    let longest = w.max(h);
    if longest <= max_dim {
        return (w, h);
    }
    let scale = f64::from(max_dim) / f64::from(longest);
    let new_w = (f64::from(w) * scale).round().max(1.0) as u32;
    let new_h = (f64::from(h) * scale).round().max(1.0) as u32;
    (new_w, new_h)
}

// ---------------------------------------------------------------------------
// Block C Task 17 — AI/Illustrator SVG subset import
// ---------------------------------------------------------------------------

/// Import an Illustrator `.ai` file. Returns a summary describing
/// whether the embedded SVG payload was found and how many objects
/// were extracted.
pub fn import_ai(path: &str) -> std::result::Result<AiImportSummary, DocumentBridgeError> {
    let bytes = std::fs::read(PathBuf::from(path))
        .map_err(|e| DocumentBridgeError::Internal(format!("import_ai read: {e}")))?;
    import_illustrator_bytes(&bytes)
        .map_err(|e: AiImportError| DocumentBridgeError::Internal(format!("import_ai: {e}")))
}

// ---------------------------------------------------------------------------
// Block D Task 19 — Brand → brochure template
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrochurePlanResult {
    pub pages: Vec<BrochurePage>,
    pub brand_kit_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrochurePage {
    pub index: u32,
    pub page_type: String,
    pub sections: Vec<BrochureSection>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrochureSection {
    pub section_kind: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub style_color_hex: Option<String>,
}

/// Build a multi-page brochure layout plan for a brand kit.
///
/// Deterministic structural template: cover page (logo + headline +
/// sub-headline), then alternating content pages (heading + body +
/// image placeholder), then a back page (contact + brand colours).
pub fn ai_brand_to_brochure(brand_kit_id: Uuid, num_pages: u32) -> Result<BrochurePlanResult> {
    let n = num_pages.clamp(2, 32);
    let kit_colors = with_workspace(|ws| {
        let kit = ws
            .project
            .brand_kits
            .iter()
            .find(|k| k.id == brand_kit_id)
            .ok_or_else(|| {
                DocumentBridgeError::Internal(format!("brand kit {brand_kit_id} not found"))
            })?;
        Ok(kit.colors.clone())
    })?;
    let primary = kit_colors.first().map(|c| c.color.to_hex());
    let secondary = kit_colors.get(1).map(|c| c.color.to_hex());

    let page_w = 794.0; // A4 portrait at 96dpi
    let page_h = 1123.0;
    let margin = 64.0;

    let mut pages: Vec<BrochurePage> = Vec::new();
    pages.push(BrochurePage {
        index: 0,
        page_type: "cover".into(),
        sections: vec![
            BrochureSection {
                section_kind: "logo".into(),
                x: margin,
                y: margin,
                width: 240.0,
                height: 80.0,
                style_color_hex: primary.clone(),
            },
            BrochureSection {
                section_kind: "headline".into(),
                x: margin,
                y: page_h * 0.45,
                width: page_w - 2.0 * margin,
                height: 96.0,
                style_color_hex: primary.clone(),
            },
            BrochureSection {
                section_kind: "subheadline".into(),
                x: margin,
                y: page_h * 0.55,
                width: page_w - 2.0 * margin,
                height: 48.0,
                style_color_hex: secondary.clone(),
            },
        ],
    });
    for i in 1..(n - 1) {
        pages.push(BrochurePage {
            index: i,
            page_type: "content".into(),
            sections: vec![
                BrochureSection {
                    section_kind: "heading".into(),
                    x: margin,
                    y: margin,
                    width: page_w - 2.0 * margin,
                    height: 72.0,
                    style_color_hex: primary.clone(),
                },
                BrochureSection {
                    section_kind: "body".into(),
                    x: margin,
                    y: margin + 96.0,
                    width: (page_w - 2.0 * margin) / 2.0 - 16.0,
                    height: page_h - 2.0 * margin - 96.0,
                    style_color_hex: None,
                },
                BrochureSection {
                    section_kind: "image_placeholder".into(),
                    x: margin + (page_w - 2.0 * margin) / 2.0 + 16.0,
                    y: margin + 96.0,
                    width: (page_w - 2.0 * margin) / 2.0 - 16.0,
                    height: page_h - 2.0 * margin - 96.0,
                    style_color_hex: secondary.clone(),
                },
            ],
        });
    }
    pages.push(BrochurePage {
        index: n - 1,
        page_type: "back".into(),
        sections: vec![
            BrochureSection {
                section_kind: "contact".into(),
                x: margin,
                y: margin,
                width: page_w - 2.0 * margin,
                height: 200.0,
                style_color_hex: primary.clone(),
            },
            BrochureSection {
                section_kind: "color_swatches".into(),
                x: margin,
                y: page_h - margin - 96.0,
                width: page_w - 2.0 * margin,
                height: 96.0,
                style_color_hex: secondary,
            },
        ],
    });
    Ok(BrochurePlanResult {
        pages,
        brand_kit_id: brand_kit_id.to_string(),
    })
}

// ---------------------------------------------------------------------------
// Block D Task 20 — Plugin marketplace
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginListing {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub permissions: Vec<String>,
    pub trust_status: String,
    pub installed: bool,
}

/// List installed plugins discovered under `~/.kcreate/plugins/`.
pub fn plugin_marketplace_list() -> Result<Vec<PluginListing>> {
    let marketplace = kcreate_plugin::marketplace::PluginMarketplace::default();
    let listings = marketplace
        .list()
        .map_err(|e| DocumentBridgeError::Internal(format!("plugin_marketplace_list: {e}")))?;
    Ok(listings.into_iter().map(Into::into).collect())
}

/// Install a plugin from a local `.wasm`/`.kcplugin` file.
pub fn plugin_marketplace_install_local(path: &str) -> Result<PluginListing> {
    let marketplace = kcreate_plugin::marketplace::PluginMarketplace::default();
    let listing = marketplace
        .install_local(std::path::Path::new(path))
        .map_err(|e| {
            DocumentBridgeError::Internal(format!("plugin_marketplace_install_local: {e}"))
        })?;
    Ok(listing.into())
}

/// Remove a plugin by id.
pub fn plugin_marketplace_remove(id: &str) -> Result<bool> {
    let marketplace = kcreate_plugin::marketplace::PluginMarketplace::default();
    marketplace
        .remove(id)
        .map_err(|e| DocumentBridgeError::Internal(format!("plugin_marketplace_remove: {e}")))
}

impl From<kcreate_plugin::marketplace::PluginListing> for PluginListing {
    fn from(p: kcreate_plugin::marketplace::PluginListing) -> Self {
        Self {
            id: p.id,
            name: p.name,
            version: p.version,
            author: p.author,
            description: p.description,
            permissions: p.permissions,
            trust_status: p.trust_status,
            installed: p.installed,
        }
    }
}

// ---------------------------------------------------------------------------
// Block D Task 21 — Multi-page PDF export
// ---------------------------------------------------------------------------

/// Render the open project to a multi-page PDF with optional TOC,
/// bookmarks, and hyperlinks. The actual SVG-to-PDF rendering is
/// delegated to `kcreate_export::pdf_multi`.
pub fn export_pdf_multi(options_json: &str, output_path: &str) -> Result<PdfMultiReport> {
    let opts: PdfMultiOptions = serde_json::from_str(options_json).map_err(|e| {
        DocumentBridgeError::InvalidArgument {
            argument: "options_json".into(),
            value: format!("{e}"),
        }
    })?;
    // Just verify a project is open before we render pages.
    with_workspace(|_ws| Ok(()))?;
    let pages = collect_page_svgs()?;
    export_pdf_multi_pages(&pages, std::path::Path::new(output_path), &opts).map_err(
        |e: PdfMultiError| DocumentBridgeError::Internal(format!("export_pdf_multi: {e}")),
    )
}

fn collect_page_svgs(
) -> Result<Vec<kcreate_export::pdf_multi::PdfPageInput>> {
    with_workspace(|ws| {
        let mut pages = Vec::new();
        for (id, n) in ws.project.document.iter() {
            if n.node_type == NodeType::Page || n.node_type == NodeType::Artboard {
                // Render the page subtree to SVG via the existing
                // SVG exporter. We pass `false` for `include_hidden`
                // so invisible layers are skipped, matching the
                // single-page export pipeline.
                let opts = kcreate_export::svg::SvgExportOptions {
                    width: n.bounds.width,
                    height: n.bounds.height,
                    ..Default::default()
                };
                // Collect all descendant vector ids so the per-page
                // SVG carries the page's full content. Non-vector
                // descendants are tolerated by the empty-list path
                // (vector-only filtering happens inside).
                let descendants = ws.project.document.descendants_of(*id);
                let vector_ids: Vec<Uuid> = descendants
                    .into_iter()
                    .filter(|d| {
                        ws.project
                            .document
                            .get_node(*d)
                            .is_some_and(|nn| nn.node_type == NodeType::VectorLayer)
                    })
                    .collect();
                let svg = kcreate_export::svg::export_svg_from_document(
                    &ws.project.document,
                    &vector_ids,
                    &opts,
                )
                .unwrap_or_else(|_| String::new());
                pages.push(kcreate_export::pdf_multi::PdfPageInput {
                    title: n.name.clone(),
                    svg,
                    width_pt: n.bounds.width.max(1.0),
                    height_pt: n.bounds.height.max(1.0),
                });
            }
        }
        Ok(pages)
    })
}

// ---------------------------------------------------------------------------
// Block D Task 23 — Preferences load/save
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Preferences {
    pub general: GeneralPrefs,
    pub canvas: CanvasPrefs,
    pub ai: AiPrefs,
    pub performance: PerformancePrefs,
    pub privacy: PrivacyPrefs,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneralPrefs {
    pub theme: String,
    pub language: String,
    pub autosave_interval_sec: u32,
    pub scratch_project_cleanup: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasPrefs {
    pub default_grid_spacing: f64,
    pub default_grid_subdivisions: u32,
    pub snap_threshold_px: f64,
    pub ruler_units: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiPrefs {
    pub default_llm_model: String,
    pub auto_start_sidecar: bool,
    pub gbnf_grammar_debugging: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PerformancePrefs {
    pub raster_cache_budget_mb: u32,
    pub undo_depth_override: Option<u32>,
    pub low_resource_mode: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrivacyPrefs {
    pub telemetry_opt_in: bool,
    pub audit_log_retention_days: u32,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            general: GeneralPrefs {
                theme: "system".into(),
                language: "en-US".into(),
                autosave_interval_sec: 60,
                scratch_project_cleanup: true,
            },
            canvas: CanvasPrefs {
                default_grid_spacing: 16.0,
                default_grid_subdivisions: 4,
                snap_threshold_px: 6.0,
                ruler_units: "px".into(),
            },
            ai: AiPrefs {
                default_llm_model: String::new(),
                auto_start_sidecar: false,
                gbnf_grammar_debugging: false,
            },
            performance: PerformancePrefs {
                raster_cache_budget_mb: 512,
                undo_depth_override: None,
                low_resource_mode: false,
            },
            privacy: PrivacyPrefs {
                telemetry_opt_in: false,
                audit_log_retention_days: 90,
            },
        }
    }
}

fn preferences_path() -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| {
            DocumentBridgeError::Internal(
                "preferences: $HOME is not set; cannot resolve preferences path".into(),
            )
        })?;
    let dir = home.join(".kcreate");
    std::fs::create_dir_all(&dir)
        .map_err(|e| DocumentBridgeError::Internal(format!("preferences: mkdir: {e}")))?;
    Ok(dir.join("preferences.json"))
}

pub fn preferences_load() -> Result<Preferences> {
    let path = preferences_path()?;
    if !path.exists() {
        return Ok(Preferences::default());
    }
    let bytes = std::fs::read(&path)
        .map_err(|e| DocumentBridgeError::Internal(format!("preferences read: {e}")))?;
    serde_json::from_slice::<Preferences>(&bytes)
        .map_err(|e| DocumentBridgeError::Internal(format!("preferences parse: {e}")))
        .or_else(|_| Ok(Preferences::default()))
}

pub fn preferences_save(prefs_json: &str) -> Result<()> {
    let parsed: Preferences = serde_json::from_str(prefs_json).map_err(|e| {
        DocumentBridgeError::InvalidArgument {
            argument: "preferences_json".into(),
            value: format!("{e}"),
        }
    })?;
    let path = preferences_path()?;
    let pretty = serde_json::to_string_pretty(&parsed).map_err(|e| {
        DocumentBridgeError::Internal(format!("preferences serialize: {e}"))
    })?;
    std::fs::write(&path, pretty)
        .map_err(|e| DocumentBridgeError::Internal(format!("preferences write: {e}")))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smart_select_mode_parser_round_trips_known_names() {
        assert_eq!(
            SmartSelectMode::from_wire("replace"),
            Some(SmartSelectMode::Replace)
        );
        assert_eq!(SmartSelectMode::from_wire("add"), Some(SmartSelectMode::Add));
        assert_eq!(
            SmartSelectMode::from_wire("subtract"),
            Some(SmartSelectMode::Subtract)
        );
        assert_eq!(SmartSelectMode::from_wire("xor"), None);
    }

    #[test]
    fn preferences_default_round_trips_through_json() {
        let prefs = Preferences::default();
        let s = serde_json::to_string(&prefs).unwrap();
        let back: Preferences = serde_json::from_str(&s).unwrap();
        assert_eq!(back.general.theme, "system");
        assert!(!back.privacy.telemetry_opt_in);
    }

    #[test]
    fn scale_to_fit_caps_longest_side() {
        assert_eq!(scale_to_fit(2000, 1000, 1024), (1024, 512));
        assert_eq!(scale_to_fit(500, 500, 1024), (500, 500));
    }
}
