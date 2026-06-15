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

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use base64::Engine as _;
use chrono::Utc;
use kcreate_ai::auto_color::{auto_color_correct, AutoColorMode, AutoColorOptions};
use kcreate_ai::denoise::{denoise, DenoiseOptions};
use kcreate_ai::glyph_extract::{extract_glyph, GlyphCrop, GlyphExtractOptions};
use kcreate_ai::image_gen::decode_png_payload_lenient;
use kcreate_ai::inpaint::{inpaint, mask_from_rects, InpaintOptions, MaskRect};
use kcreate_ai::one_pager::{
    brief_to_one_pager, BriefToOnePagerOptions, BriefToOnePagerResult, OnePagerPageSize,
};
use kcreate_ai::palette_harmonize::{harmonize_palette, HarmonyResult, HarmonyRule};
use kcreate_ai::reformat::{reformat_to_deck, ReformatDeckOptions, ReformatDeckResult, SourceNode};
use kcreate_ai::segment::{segment_image, SegmentBackend, SegmentOptions};
use kcreate_ai::smart_select::smart_select;
use kcreate_ai::stroke_match::{match_stroke_style, StrokeMatchSummary, StrokeProperties};
use kcreate_ai::themed_deck::{
    generate_design, outline_from_brief, sanitize_outline, DeckOutline, DesignElement,
    DesignFormat, ElementKind, ElementRole, GeneratedDesign, OnePagerSize, SlideOutline, ThemeId,
    ThemedDesignOptions,
};
use kcreate_ai::type_pairing::{suggest_type_pairing, TypePairingResult};
use kcreate_core::node::{
    Bounds, FillStyle, GradientKind, GradientStop, LineCap, LineJoin, Node, NodeStyle, NodeType,
    Point2D, RgbaColor, StrokeStyle,
};
use kcreate_core::operation::Operation;
use kcreate_core::project::{BrandKit, NamedColor};
use kcreate_export::ai_import::{import_illustrator_bytes, AiImportError, AiImportSummary};
use kcreate_export::pdf_multi::{
    export_pdf_multi_pages, PdfMultiError, PdfMultiOptions, PdfMultiReport,
};
use kcreate_export::smart_compress::{
    rgba_to_rgb_over_white, smart_compress, SmartCompressFormat, SmartCompressOptions,
    SmartCompressReport,
};
use kcreate_export::svg_optimize::{optimize_svg, SvgOptimizeReport};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::document::{
    blob_load, collect_subtree_parent_first, with_workspace, with_workspace_mut,
    DocumentBridgeError, Result,
};
use crate::phase4::{image_gen_generate, image_gen_status};
use crate::scene_sync::{
    RasterImageMeta, TextLayerMeta, RASTER_IMAGE_METADATA_KEY, TEXT_LAYER_METADATA_KEY,
    VECTOR_PATH_METADATA_KEY,
};

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
            .lock()
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
    // Defense-in-depth: clamp at the bridge boundary even though
    // `denoise()` clamps internally. Keeps the operation-log JSON,
    // the algorithm call, and the wire reply in lockstep with one
    // canonical `effective_opts` value.
    let effective_opts = DenoiseOptions {
        strength,
        search_radius,
        patch_radius,
    }
    .clamped();
    let out = denoise(&rgba, w, h, effective_opts)
        .map_err(|e| DocumentBridgeError::Internal(format!("ai_denoise: {e}")))?;
    let new_id = install_processed_raster(
        node_id,
        &out,
        w,
        h,
        "ai_denoise",
        serde_json::json!({
            "strength": effective_opts.strength,
            "search_radius": effective_opts.search_radius,
            "patch_radius": effective_opts.patch_radius,
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
    let rects: Vec<MaskRect> =
        serde_json::from_str(mask_json).map_err(|e| DocumentBridgeError::InvalidArgument {
            argument: "mask_json".into(),
            value: format!("{e}"),
        })?;
    let (rgba, w, h) = load_raster_rgba(node_id)?;
    let mask = mask_from_rects(&rects, w, h);
    // Defense-in-depth: clamp at the bridge boundary even though
    // `inpaint()` clamps internally; ensures the algorithm call, the
    // operation-log JSON, and the wire reply all reference one
    // canonical `effective_opts`.
    let effective_opts = InpaintOptions {
        patch_radius: patch_radius.unwrap_or(3),
        num_iterations: num_iterations.unwrap_or(5),
        pyramid_levels: pyramid_levels.unwrap_or(3),
    }
    .clamped();
    let out = inpaint(&rgba, &mask, w, h, effective_opts)
        .map_err(|e| DocumentBridgeError::Internal(format!("ai_inpaint: {e}")))?;
    let new_id = install_processed_raster(
        node_id,
        &out,
        w,
        h,
        "ai_inpaint",
        serde_json::json!({
            "rects": rects,
            "patch_radius": effective_opts.patch_radius,
            "num_iterations": effective_opts.num_iterations,
            "pyramid_levels": effective_opts.pyramid_levels,
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
    let parsed_mode =
        AutoColorMode::from_wire(mode).ok_or_else(|| DocumentBridgeError::InvalidArgument {
            argument: "mode".into(),
            value: mode.into(),
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
                .map_or_else(|| vec![0u8; (w * h) as usize], |m| m.mask);
            (first, result.backend)
        }
        Err(_) => {
            // Hard fallback to flood-fill — never refuse the action.
            let m = smart_select(&rgba, w, h, point_x, point_y, 0.15);
            (m, SegmentBackend::EdgeAware)
        }
    };
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
    let parsed_mode =
        SmartSelectMode::from_wire(mode).ok_or_else(|| DocumentBridgeError::InvalidArgument {
            argument: "mode".into(),
            value: mode.into(),
        })?;
    let (rgba, w, h) = load_raster_rgba(node_id)?;
    let new_mask = smart_select(&rgba, w, h, x, y, tolerance);
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
        let stroke =
            node.style
                .stroke
                .clone()
                .ok_or_else(|| DocumentBridgeError::InvalidArgument {
                    argument: "source_node_id".into(),
                    value: "no stroke on source".into(),
                })?;
        Ok(stroke_to_props(
            &stroke,
            node.style.stroke_width_profile.clone(),
        ))
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
    let summary = match_stroke_style(&source_id.to_string(), Some(&source_props), &target_pairs)
        .map_err(|e| DocumentBridgeError::Internal(format!("match_stroke_style: {e}")))?;

    let new_stroke = props_to_stroke(&source_props);
    let width_profile = source_props.width_profile;
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
            node.style.stroke_width_profile.clone_from(&width_profile);
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

fn stroke_to_props(s: &StrokeStyle, width_profile: Option<Vec<(f64, f64)>>) -> StrokeProperties {
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
    let result = extract_glyph(&rgba, w, h, crop, opts)
        .map_err(|e| DocumentBridgeError::Internal(format!("ai_extract_glyph: {e}")))?;
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
    let node_id =
        req.node_id
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
    let img = image::RgbaImage::from_raw(w, h, rgba).ok_or_else(|| {
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
            // JPEG is opaque — passing RGBA8 straight to the encoder
            // either errors at runtime or silently drops alpha, which
            // composites semi-transparent pixels against black and
            // produces dark halos. Composite over white first (same
            // behaviour as `smart_compress`'s JPEG branch so the
            // preview and the final compressed export agree
            // byte-for-byte on transparency handling).
            let rgb = rgba_to_rgb_over_white(resized.as_raw());
            let mut cursor = std::io::Cursor::new(&mut bytes);
            image::write_buffer_with_format(
                &mut cursor,
                &rgb,
                pw,
                ph,
                image::ColorType::Rgb8,
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
//
// The pure layout algorithm lives in
// `kcreate_ai::brand_template::plan_brochure`. This module is the
// workspace-aware adapter: it loads the brand kit from the active
// project, hands the hex colour list off to the AI plan function,
// and returns the resulting plan over N-API. Wire-format types are
// re-exported as aliases so the renderer / scene.ts continue to
// see the same `BrochurePlanResult`, `BrochurePage`, and
// `BrochureSection` names.
// ---------------------------------------------------------------------------

pub use kcreate_ai::brand_template::{BrochurePage, BrochureSection};

/// Result envelope returned to the renderer. Re-exports
/// [`kcreate_ai::brand_template::BrochurePlan`] under the bridge's
/// historical name so existing wire-format consumers stay stable.
pub type BrochurePlanResult = kcreate_ai::brand_template::BrochurePlan;

/// Build a multi-page brochure layout plan for a brand kit.
///
/// Looks up the brand kit on the active workspace, then delegates to
/// [`kcreate_ai::brand_template::plan_brochure`] (the pure planner).
/// Deterministic structural template: cover page (logo + headline +
/// sub-headline), then alternating content pages (heading + body +
/// image placeholder), then a back page (contact + brand colours).
pub fn ai_brand_to_brochure(brand_kit_id: Uuid, num_pages: u32) -> Result<BrochurePlanResult> {
    let kit_colors = with_workspace(|ws| {
        let kit = ws
            .project
            .brand_kits
            .iter()
            .find(|k| k.id == brand_kit_id)
            .ok_or_else(|| {
                DocumentBridgeError::Internal(format!("brand kit {brand_kit_id} not found"))
            })?;
        Ok(kit
            .colors
            .iter()
            .map(|c| c.color.to_hex())
            .collect::<Vec<_>>())
    })?;
    kcreate_ai::brand_template::plan_brochure(
        &brand_kit_id.to_string(),
        &kit_colors,
        num_pages,
        kcreate_ai::brand_template::PageGeometry::default(),
    )
    .map_err(|e| DocumentBridgeError::Internal(format!("ai_brand_to_brochure: {e}")))
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
    let opts: PdfMultiOptions =
        serde_json::from_str(options_json).map_err(|e| DocumentBridgeError::InvalidArgument {
            argument: "options_json".into(),
            value: format!("{e}"),
        })?;
    // Just verify a project is open before we render pages.
    with_workspace(|_ws| Ok(()))?;
    let pages = collect_page_svgs()?;
    export_pdf_multi_pages(&pages, std::path::Path::new(output_path), &opts)
        .map_err(|e: PdfMultiError| DocumentBridgeError::Internal(format!("export_pdf_multi: {e}")))
}

fn collect_page_svgs() -> Result<Vec<kcreate_export::pdf_multi::PdfPageInput>> {
    with_workspace(|ws| {
        let mut pages = Vec::new();
        // Deterministic, double-count-free traversal: each artboard
        // becomes one PDF page, and a page that has no artboard
        // descendant (legacy single-page docs that hang content
        // directly off the page) becomes its own PDF page. Iterating
        // the document's `HashMap` directly would (a) double-count a
        // page *and* its child artboards and (b) emit pages in
        // non-deterministic order. `ordered_export_units` walks the
        // ordered root/children lists instead, so a tiled deck
        // exports its slides left-to-right in insertion order.
        for id in ordered_export_units(&ws.project.document) {
            let Some(n) = ws.project.document.get_node(id) else {
                continue;
            };
            // Crop each unit to its own world-space frame. Tiled
            // artboards live at increasing world `x`; rendering with
            // a `0 0 w h` viewBox would push every slide past the
            // first off-canvas and produce blank PDF pages. The
            // origin-aware frame keeps each tile centred in its page.
            //
            // The full subtree (incl. `RasterLayer` / `TextLayer`
            // descendants) is walked by `compose_page_svg_in_frame`;
            // raster blobs resolve through the bridge's `BlobStore`
            // via a callback (kept out of `kcreate_export`'s dep
            // graph), auto-detecting PNG/JPEG/WebP vs. raw RGBA.
            let store_guard = ws.store.lock();
            let store = store_guard.blobs();
            let svg = kcreate_export::compose_page_svg_in_frame(
                &ws.project.document,
                id,
                n.bounds.x,
                n.bounds.y,
                n.bounds.width,
                n.bounds.height,
                |hash| store.load(hash).ok(),
            );
            pages.push(kcreate_export::pdf_multi::PdfPageInput {
                title: n.name.clone(),
                svg,
                width_pt: n.bounds.width.max(1.0),
                height_pt: n.bounds.height.max(1.0),
            });
        }
        Ok(pages)
    })
}

/// Ordered list of nodes that should each become one exported page.
///
/// Walks the document's ordered root/children lists (DFS) so output
/// order is deterministic — a tiled deck exports its slides in
/// insertion (left-to-right) order rather than `HashMap` order.
///
/// Rules:
/// - An `Artboard` is a self-contained frame: it becomes one page and
///   we do **not** descend into it (its descendants are rendered by
///   `compose_page_svg_in_frame`).
/// - A `Page` that has at least one `Artboard` descendant is a pure
///   container — we descend so the artboards become the pages, and the
///   page itself is **not** emitted (this is what removes the
///   historical double-count).
/// - A `Page` with no `Artboard` descendant (legacy docs that hang
///   content directly off the page) becomes one page itself.
/// - Any other container (e.g. a `GroupLayer` wrapping an artboard) is
///   never a page on its own, but we still descend through it so a
///   nested artboard is reached rather than silently dropped. This
///   keeps the traversal consistent with the broad `descendants_of`
///   artboard check above. Leaf layers have no children, so they add
///   nothing.
fn ordered_export_units(doc: &kcreate_core::document::DocumentGraph) -> Vec<Uuid> {
    let push_children =
        |id: Uuid, stack: &mut Vec<Uuid>| stack.extend(doc.children_of(id).into_iter().rev());

    let mut out = Vec::new();
    let mut stack: Vec<Uuid> = doc.root_ids().iter().rev().copied().collect();
    while let Some(id) = stack.pop() {
        let Some(node) = doc.get_node(id) else {
            continue;
        };
        match node.node_type {
            NodeType::Artboard => out.push(id),
            NodeType::Page => {
                let has_artboard = doc.descendants_of(id).into_iter().any(|d| {
                    doc.get_node(d)
                        .is_some_and(|n| n.node_type == NodeType::Artboard)
                });
                if has_artboard {
                    push_children(id, &mut stack);
                } else {
                    out.push(id);
                }
            }
            _ => push_children(id, &mut stack),
        }
    }
    out
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
    /// Phase A2 — native save-as dialog. Sticky directory state
    /// for the renderer's `chooseExportTarget` /
    /// `chooseExportDirectory` flow. Wire field is `export`.
    /// `#[serde(default)]` so preferences files written before
    /// Phase A2 load cleanly (the new section materialises as an
    /// empty map + `None`).
    #[serde(default)]
    pub export: ExportPrefs,
    /// Phase C — first-run onboarding. The welcome modal that
    /// drives the tier-aware "install recommended pack" flow
    /// checks `onboarding.completed` on startup; once any close
    /// path fires (install succeeded, user provided their own
    /// weights file, or user clicked "Skip"), the renderer flips
    /// this to `true` so the modal is never shown again. The
    /// `#[serde(default)]` is load-bearing — preferences files
    /// written before Phase C must continue to load and gain the
    /// section silently with `completed = false`, which is the
    /// correct first-run behaviour for an existing user who has
    /// never seen the welcome modal.
    #[serde(default)]
    pub onboarding: OnboardingPrefs,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneralPrefs {
    pub theme: String,
    pub language: String,
    pub autosave_interval_sec: u32,
    /// How long (in days) to retain `.kstudio` scratch projects
    /// before the autosaver garbage-collects them. `0` disables
    /// the sweep entirely. Stored as a count rather than a bool so
    /// the UI can expose a meaningful slider without burning a
    /// second preference field. Wire field is `scratchProjectCleanupDays`.
    pub scratch_project_cleanup_days: u32,
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

/// Phase A2 — sticky directory state for the native save-as dialog.
///
/// `last_dir_by_format` keys are the wire-format lower-case format
/// names already used by the export bridge (`"png"`, `"svg"`,
/// `"pdf"`, `"webp"`, `"jpeg"`). Values are absolute directory
/// paths the user last picked via `chooseExportTarget`. The
/// renderer reads the entry on panel mount and passes it as the
/// `defaultDir` hint so the OS dialog opens in the user's most
/// recent location for that format.
///
/// `last_batch_dir` is the absolute directory the user last picked
/// via `chooseExportDirectory` for batch presets. `None` until the
/// first successful batch run.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportPrefs {
    #[serde(default)]
    pub last_dir_by_format: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub last_batch_dir: Option<String>,
}

/// Phase C — first-run welcome / onboarding state.
///
/// The renderer's `WelcomeModal` is the only consumer; it is
/// auto-mounted on `HomePage` when `completed == false` and
/// dismissed (with `completed = true` persisted) on every
/// close path. `last_seen_pack_id` records the recommended pack
/// id the modal surfaced to the user so that a future Phase F
/// "re-recommend after tier upgrade" pass can detect when the
/// device tier crossed a boundary (e.g. the user added RAM and
/// the recommended pack rolled from `llm_bonsai_1_7b` to
/// `llm_bonsai_4b`) and re-surface the modal with the new
/// pack — without forcing the modal back on users whose tier
/// has not changed.
///
/// Wire field is `onboarding`. The struct itself derives
/// `Default` so `#[serde(default)]` on the parent field can
/// instantiate it cleanly when the section is absent from an
/// older preferences file.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OnboardingPrefs {
    #[serde(default)]
    pub completed: bool,
    #[serde(default)]
    pub last_seen_pack_id: Option<String>,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            general: GeneralPrefs {
                theme: "system".into(),
                language: "en-US".into(),
                autosave_interval_sec: 60,
                scratch_project_cleanup_days: 30,
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
            export: ExportPrefs::default(),
            onboarding: OnboardingPrefs::default(),
        }
    }
}

fn preferences_path() -> Result<PathBuf> {
    let home = user_home_dir().ok_or_else(|| {
        DocumentBridgeError::Internal(
            "preferences: neither $HOME nor %USERPROFILE% is set; cannot resolve preferences path"
                .into(),
        )
    })?;
    let dir = home.join(".kcreate");
    std::fs::create_dir_all(&dir)
        .map_err(|e| DocumentBridgeError::Internal(format!("preferences: mkdir: {e}")))?;
    Ok(dir.join("preferences.json"))
}

/// Cross-platform home-directory resolver. Mirrors the
/// `HOME` → `USERPROFILE` fallback chain used by
/// `crates/kcreate_bridge/src/phase2.rs`, `kcreate_core::marketplace`
/// and `kcreate_audit::store`, so that `~/.kcreate/...` lookups work
/// uniformly on Linux/macOS (`HOME`) and Windows (`USERPROFILE`).
pub(crate) fn user_home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// Load `~/.kcreate/preferences.json`.
///
/// Returns [`Preferences::default()`] only when the file does NOT
/// exist (a first-run scenario). When the file exists but cannot
/// be parsed the error is surfaced to the caller so the renderer
/// can prompt the user — silently overwriting bad preferences with
/// defaults would discard the user's settings on the next save.
pub fn preferences_load() -> Result<Preferences> {
    let path = preferences_path()?;
    if !path.exists() {
        return Ok(Preferences::default());
    }
    let bytes = std::fs::read(&path)
        .map_err(|e| DocumentBridgeError::Internal(format!("preferences read: {e}")))?;
    serde_json::from_slice::<Preferences>(&bytes)
        .map_err(|e| DocumentBridgeError::Internal(format!("preferences parse: {e}")))
}

/// Persist `prefs_json` to the preferences file via an
/// atomic write-temp-then-rename. The temp file lives in the
/// same directory as the final preferences file so the rename
/// is always within the same filesystem and is therefore atomic
/// on POSIX (`rename(2)`) and near-atomic on Windows NTFS. A
/// crash or power-loss mid-write leaves either the previous
/// file intact or the new file fully written — never a partial
/// file that `preferences_load` would have to recover from.
pub fn preferences_save(prefs_json: &str) -> Result<()> {
    let parsed: Preferences =
        serde_json::from_str(prefs_json).map_err(|e| DocumentBridgeError::InvalidArgument {
            argument: "preferences_json".into(),
            value: format!("{e}"),
        })?;
    let path = preferences_path()?;
    let pretty = serde_json::to_string_pretty(&parsed)
        .map_err(|e| DocumentBridgeError::Internal(format!("preferences serialize: {e}")))?;
    let tmp = preferences_tmp_path(&path);
    std::fs::write(&tmp, &pretty)
        .map_err(|e| DocumentBridgeError::Internal(format!("preferences write (tmp): {e}")))?;
    if let Err(e) = std::fs::rename(&tmp, &path) {
        // Try to clean up the orphan tmp so the next save isn't
        // misled by a stale file. Best-effort: rename failure is
        // already the error we're returning to the caller.
        let _ = std::fs::remove_file(&tmp);
        return Err(DocumentBridgeError::Internal(format!(
            "preferences rename: {e}"
        )));
    }
    Ok(())
}

/// Build the temp-file path used by `preferences_save`. Keeping
/// the temp in the same directory as the destination is what
/// makes the subsequent `rename` atomic — moving across
/// filesystems would fall back to copy+delete and lose the
/// atomicity guarantee.
fn preferences_tmp_path(final_path: &std::path::Path) -> std::path::PathBuf {
    let mut name = final_path.file_name().map_or_else(
        || std::ffi::OsString::from("preferences.json"),
        std::ffi::OsStr::to_os_string,
    );
    name.push(format!(".tmp-{}", std::process::id()));
    final_path
        .parent()
        .map_or_else(|| std::path::PathBuf::from(&name), |p| p.join(&name))
}

// ---------------------------------------------------------------------------
// Block G3 — Gamma-style themed multi-page generator
// ---------------------------------------------------------------------------

/// Gap (world units) between tiled slide artboards. Matches the deck
/// tiling convention used across the bridge.
const THEMED_DECK_TILE_GAP: f64 = 100.0;

/// Metadata flag stamped on the root `Page` this generator creates.
/// On a subsequent generate it lets us replace our own prior output
/// in place (instead of accumulating duplicate tiled page trees)
/// while never touching pages the user authored themselves.
const THEMED_GENERATED_METADATA_KEY: &str = "kcreate:themedGenerated";

/// Cubic-bezier control offset for a quarter-circle corner
/// (`4/3 * (sqrt(2) - 1)`); used to round rectangle corners so the
/// rounding survives SVG/PDF export (which ignores `style.corner_radius`).
const KAPPA: f64 = 0.552_284_749_830_793_4;

/// Wire request for [`ai_generate_themed_design`]. Mirrors
/// [`ThemedDesignOptions`] (so the deterministic generator's enums
/// are reused verbatim) plus a `useLlm` flag the bridge consumes to
/// decide whether to enrich the outline with the local sidecar.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThemedDesignRequest {
    #[serde(default)]
    format: DesignFormat,
    #[serde(default)]
    theme_id: ThemeId,
    #[serde(default)]
    one_pager_size: OnePagerSize,
    #[serde(default)]
    section_count: Option<u32>,
    /// Opt-in LLM enrichment. When `true` *and* the sidecar reports
    /// `ready`, the bridge asks the model for a structured outline
    /// and falls back to the deterministic planner on any failure.
    #[serde(default)]
    use_llm: bool,
    /// Opt-in diffusion hero imagery. When `true` *and* the
    /// image-generation sidecar reports `ready`, image-bearing
    /// formats (social post / web page / document) get a real
    /// generated raster; otherwise they degrade to a tasteful
    /// gradient placeholder. Defaults to `true` so the imagery slot
    /// is filled (with a placeholder offline) without the caller
    /// having to opt in. Pure-vector formats (deck / one-pager)
    /// ignore it.
    #[serde(default = "default_true")]
    use_image: bool,
}

/// serde default for [`ThemedDesignRequest::use_image`]: imagery is
/// on by default (it degrades to a placeholder when no model is
/// present, so it is never a hard dependency).
const fn default_true() -> bool {
    true
}

/// Tolerant shape for an LLM-produced outline. Every field defaults
/// so a partial / sloppy model reply still deserialises; the result
/// is then run through [`sanitize_outline`], which drops empties and
/// supplies a fallback title.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LlmOutlineDraft {
    #[serde(default)]
    title: String,
    #[serde(default)]
    subtitle: String,
    #[serde(default)]
    slides: Vec<LlmSlideDraft>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LlmSlideDraft {
    #[serde(default)]
    heading: String,
    #[serde(default)]
    bullets: Vec<String>,
}

/// Result of applying a generated themed design to the open project.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemedDesignApplyResult {
    /// Root page that contains every tiled slide artboard.
    pub page_id: String,
    /// One id per slide / page, in left-to-right tiling order.
    pub artboard_ids: Vec<String>,
    /// Brand kit seeded from the theme palette.
    pub brand_kit_id: String,
    /// Number of pages (title card + content cards for a deck; `1`
    /// for a one-pager).
    pub slide_count: u32,
    /// Theme wire id (`"midnight"`, `"sunrise"`, …).
    pub theme_id: String,
    /// Human-readable theme name.
    pub theme_name: String,
    /// Format wire id: `"deck"`, `"onePager"`, `"socialPost"`,
    /// `"webPage"`, or `"document"`.
    pub format: String,
    /// Whether the outline was enriched by the local LLM sidecar.
    /// `false` means the deterministic planner produced it.
    pub used_llm: bool,
    /// Whether at least one hero/section image was produced by the
    /// local diffusion sidecar and placed as a raster layer. `false`
    /// means the imagery slots degraded to gradient placeholders
    /// (no model available, imagery disabled, or a pure-vector
    /// format) — the design still applied fully offline.
    pub used_image: bool,
}

/// Brief → fully themed, laid-out, multi-page design applied to the
/// open project.
///
/// Pipeline:
/// 1. Parse `options_json` into a [`ThemedDesignRequest`].
/// 2. Build a [`DeckOutline`] — LLM-enriched when `useLlm` is set and
///    the sidecar is `ready`, otherwise via the deterministic
///    [`outline_from_brief`] planner. Either path degrades to the
///    deterministic planner so the feature is never a no-op offline.
/// 3. [`generate_design`] turns the outline into themed, positioned
///    elements (pure, side-effect free).
/// 4. Translate the page-local elements into world-space nodes:
///    a root `Page` → one tiled `Artboard` per slide → themed
///    `VectorLayer` rectangles + `TextLayer` runs.
///
/// # Errors
/// Returns an error if no project is open, the brief is empty, or a
/// node insertion fails.
pub fn ai_generate_themed_design(
    brief: &str,
    options_json: &str,
) -> Result<ThemedDesignApplyResult> {
    let request: ThemedDesignRequest = if options_json.trim().is_empty() {
        ThemedDesignRequest::default()
    } else {
        serde_json::from_str(options_json).map_err(|e| DocumentBridgeError::InvalidArgument {
            argument: "options_json".into(),
            value: format!("{e}"),
        })?
    };

    let options = ThemedDesignOptions {
        format: request.format,
        theme_id: request.theme_id,
        one_pager_size: request.one_pager_size,
        section_count: request.section_count,
    };

    // Stage 1+2: outline (LLM-enriched if asked + available, else
    // deterministic) then pure layout.
    let (outline, used_llm) = build_themed_outline(brief, options, request.use_llm)?;
    let design = generate_design(&outline, options);

    // Carry the full intent forward so a later "refine" can reload it
    // (stamped on the generated page) and replay the same pipeline.
    let spec = ThemedSpec {
        brief: brief.to_string(),
        options,
        use_llm: request.use_llm,
        use_image: request.use_image,
    };

    // Stage 3: apply to the open workspace.
    apply_generated_design(&spec, &design, used_llm, "ai_generate_themed_design")
}

/// Persisted intent behind a generated design, stamped on the
/// generator-owned root page so [`ai_refine_themed_design`] can
/// reload it and replay the same pipeline with a tweaked outline /
/// option set. Mirrors the inputs to [`ai_generate_themed_design`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThemedSpec {
    brief: String,
    options: ThemedDesignOptions,
    use_llm: bool,
    #[serde(default = "default_true")]
    use_image: bool,
}

/// Metadata key under which the [`ThemedSpec`] is stamped on the
/// generator-owned root page (alongside
/// [`THEMED_GENERATED_METADATA_KEY`]). Lets the refine loop recover
/// the brief + options that produced the current design.
const THEMED_SPEC_METADATA_KEY: &str = "kcreate:themedSpec";

/// Refine an already-generated themed design with a follow-up
/// instruction (e.g. "make it more minimal", "add a pricing slide",
/// "punchier headlines", "remove the hero image"). The instruction
/// is parsed into a deterministic [`RefineDirective`] that nudges
/// the stored [`ThemedSpec`] (section count / imagery), then folded
/// into the brief so the derived copy + layout variety genuinely
/// change. The result replaces the prior generated design through
/// the same apply pipeline, recorded as a single undoable
/// operation.
///
/// # Errors
/// Returns an error if no project is open, the instruction is empty,
/// or there is no prior generated design to refine.
pub fn ai_refine_themed_design(instruction: &str) -> Result<ThemedDesignApplyResult> {
    let instruction = instruction.trim();
    if instruction.is_empty() {
        return Err(DocumentBridgeError::InvalidArgument {
            argument: "instruction".into(),
            value: "refine instruction must not be empty".into(),
        });
    }
    let prior = load_themed_spec()?.ok_or_else(|| DocumentBridgeError::InvalidArgument {
        argument: "instruction".into(),
        value: "no AI-generated design to refine — generate one first".into(),
    })?;

    let directive = parse_refine_directive(instruction);
    let mut options = prior.options;
    // Start from the count the prior design actually resolved to so
    // repeated refinements accumulate (+1, +1, …) instead of always
    // nudging the format default.
    let current = options.resolved_section_count() as i64;
    let mut next = current;
    let mut use_image = prior.use_image;
    match directive {
        RefineDirective::MoreContent => next = current + 1,
        RefineDirective::LessContent => next = current - 1,
        RefineDirective::Minimal => {
            next = current - 1;
            use_image = false;
        }
        RefineDirective::AddImagery => use_image = true,
        RefineDirective::RemoveImagery => use_image = false,
        RefineDirective::Rephrase => {}
    }
    // `resolved_section_count` re-clamps per format on the next run,
    // so an out-of-band value here is harmless; keep it >= 1 so the
    // stored hint is always sane.
    options.section_count = Some(next.max(1) as u32);

    // Fold the instruction into the brief. The deterministic planner
    // hashes the brief for both copy and layout variety, so this
    // guarantees a refine is never a no-op even when the directive
    // only rephrases.
    let brief = format!("{}\n\nRefinement: {}", prior.brief.trim(), instruction);

    let (outline, used_llm) = build_themed_outline(&brief, options, prior.use_llm)?;
    let design = generate_design(&outline, options);
    let spec = ThemedSpec {
        brief,
        options,
        use_llm: prior.use_llm,
        use_image,
    };
    apply_generated_design(&spec, &design, used_llm, "ai_refine_themed_design")
}

/// A deterministic interpretation of a free-text refine instruction.
/// Keyword-driven so the refine loop works fully offline; the LLM
/// path (when available) still enriches the *copy* via the folded
/// brief, but the structural directive never depends on a model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RefineDirective {
    /// Add a section / slide / more detail.
    MoreContent,
    /// Drop a section / tighten / condense.
    LessContent,
    /// Strip back to essentials (fewer sections + no imagery).
    Minimal,
    /// Explicitly ask for hero/section imagery.
    AddImagery,
    /// Explicitly remove hero/section imagery.
    RemoveImagery,
    /// No structural change — just re-derive the copy from the
    /// folded brief (e.g. "punchier headlines", "friendlier tone").
    Rephrase,
}

/// Map a free-text instruction to a [`RefineDirective`].
///
/// Order matters: imagery intent is checked first (so "remove the photo"
/// toggles imagery, not the section count), then the `Minimal` bucket,
/// then — crucially — `LessContent` *before* `MoreContent`.
///
/// Direction (more vs. less) is decided by intent **verbs** only. Bare
/// structure nouns ("section", "slide", "page", "bullet") are
/// deliberately NOT direction signals because they appear on both sides
/// ("add a section" vs. "fewer sections"); letting them imply
/// `MoreContent` made "fewer sections" / "trim the slides" / "remove a
/// bullet" grow the design instead of shrinking it.
///
/// Single-word keywords are matched against the tokenized word set (so
/// "cut" fires on the word "cut" but not inside "exe**cut**ive"), while
/// multi-word keywords ("get rid", "less busy") fall back to substring
/// search. The tokenization is what makes checking `LessContent` first
/// safe — a naive `contains` would let a reduce keyword embedded in an
/// unrelated word flip an addition.
fn parse_refine_directive(instruction: &str) -> RefineDirective {
    let s = instruction.to_lowercase();
    let words: HashSet<&str> = s
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    // Single-word needles match whole tokens; multi-word needles (those
    // containing a space) match as substrings so phrases still work.
    let has = |needles: &[&str]| {
        needles.iter().any(|n| {
            if n.contains(' ') {
                s.contains(n)
            } else {
                words.contains(n)
            }
        })
    };

    if has(&[
        "image",
        "images",
        "imagery",
        "photo",
        "photos",
        "picture",
        "pictures",
        "hero",
        "illustration",
        "illustrations",
        "visual",
        "visuals",
        "graphic",
        "graphics",
        "artwork",
    ]) {
        let negated = has(&[
            "no", "without", "remove", "drop", "hide", "get rid", "less", "fewer",
        ]);
        return if negated {
            RefineDirective::RemoveImagery
        } else {
            RefineDirective::AddImagery
        };
    }
    if has(&[
        "minimal",
        "minimalist",
        "simpler",
        "simplify",
        "cleaner",
        "less busy",
        "sparse",
        "declutter",
        "stripped",
    ]) {
        return RefineDirective::Minimal;
    }
    // Reduce intent is checked before add intent so an explicit "fewer" /
    // "remove" / "cut" wins even when an add-ish word is also present, and
    // so a structure noun can never flip a reduction into an addition.
    if has(&[
        "fewer", "less", "reduce", "remove", "drop", "delete", "cut", "trim", "shorter", "shorten",
        "condense", "tighten", "concise",
    ]) {
        return RefineDirective::LessContent;
    }
    if has(&[
        "more",
        "add",
        "expand",
        "detailed",
        "longer",
        "another",
        "extra",
        "additional",
    ]) {
        return RefineDirective::MoreContent;
    }
    RefineDirective::Rephrase
}

/// Load the [`ThemedSpec`] stamped on the current generator-owned
/// root page, if any. Returns `Ok(None)` when no generated design is
/// present (so the caller can surface a friendly "generate first"
/// error).
fn load_themed_spec() -> Result<Option<ThemedSpec>> {
    with_workspace(|ws| {
        for root in ws.project.document.root_ids() {
            let Some(node) = ws.project.document.get_node(*root) else {
                continue;
            };
            if let Some(value) = node.metadata.get(THEMED_SPEC_METADATA_KEY) {
                if let Ok(spec) = serde_json::from_value::<ThemedSpec>(value.clone()) {
                    return Ok(Some(spec));
                }
            }
        }
        Ok(None)
    })
}

/// Build the outline, attempting LLM enrichment only when requested
/// and the sidecar is ready. Any failure on the LLM path falls back
/// to the deterministic planner, so the returned outline is always
/// valid. The boolean is `true` only when the LLM outline was used.
fn build_themed_outline(
    brief: &str,
    options: ThemedDesignOptions,
    use_llm: bool,
) -> Result<(DeckOutline, bool)> {
    if use_llm && crate::llm::llm_status().state == "ready" {
        if let Some(enriched) = llm_enriched_outline(brief, options) {
            return Ok((enriched, true));
        }
    }
    let outline =
        outline_from_brief(brief, options).map_err(|e| DocumentBridgeError::InvalidArgument {
            argument: "brief".into(),
            value: e.to_string(),
        })?;
    Ok((outline, false))
}

/// Ask the local sidecar for a structured outline. Returns `None`
/// (so the caller falls back) on any error: not-ready, transport
/// failure, unparseable reply, or an empty sanitised outline.
fn llm_enriched_outline(brief: &str, options: ThemedDesignOptions) -> Option<DeckOutline> {
    // Resolve through the shared clamp so the model is asked for the
    // same section count the deterministic planner would produce. A
    // raw `section_count` (`0`, `99`, …) would otherwise reach the
    // prompt unbounded and make the LLM and fallback paths diverge at
    // the boundaries.
    let section_count = options.resolved_section_count() as u32;
    let messages = build_outline_messages(brief, section_count);
    let reply = crate::llm::llm_chat(messages, 1024, 0.3).ok()?;
    let json = extract_json_object(&reply.content)?;
    let draft: LlmOutlineDraft = serde_json::from_str(json).ok()?;
    let outline = DeckOutline {
        title: draft.title,
        subtitle: draft.subtitle,
        slides: draft
            .slides
            .into_iter()
            .map(|s| SlideOutline {
                heading: s.heading,
                bullets: s.bullets,
            })
            .collect(),
    };
    sanitize_outline(outline, brief)
}

/// System + user messages that constrain the model to emit a single
/// JSON outline object. Kept deliberately small and explicit so even
/// a tiny local model produces parseable output.
fn build_outline_messages(brief: &str, section_count: u32) -> Vec<crate::llm::LlmMessage> {
    let system = format!(
        "You are a presentation content planner. Respond with ONE JSON object and nothing else \
         (no markdown, no prose). Shape: {{\"title\": string, \"subtitle\": string, \"slides\": \
         [{{\"heading\": string, \"bullets\": [string]}}]}}. Produce exactly {section_count} \
         slides. Each heading is at most 6 words. Each slide has 2 to 4 bullets, each at most 14 \
         words. Do not include any field other than those shown."
    );
    vec![
        crate::llm::LlmMessage {
            role: "system".to_string(),
            content: system,
        },
        crate::llm::LlmMessage {
            role: "user".to_string(),
            content: format!("Brief:\n{}", brief.trim()),
        },
    ]
}

/// Extract the first balanced top-level `{...}` block from `s`,
/// tolerating leading/trailing prose or markdown fences a model may
/// wrap around the JSON. Brace counting respects string literals and
/// escapes so braces inside string values don't unbalance the scan.
fn extract_json_object(s: &str) -> Option<&str> {
    let start = s.find('{')?;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (i, &b) in s.as_bytes().iter().enumerate().skip(start) {
        if in_string {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&s[start..=i]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Translate a [`GeneratedDesign`] into live document nodes and seed
/// a brand kit from the theme. One root `Page` holds the tiled slide
/// `Artboard`s; each artboard owns its themed rectangles, text runs,
/// and hero imagery at world coordinates.
///
/// `command` is the operation name (`"ai_generate_themed_design"` or
/// `"ai_refine_themed_design"`) — both share this pipeline and record
/// a single, fully reversible [`ThemedDesignPatch`] operation so one
/// Ctrl+Z removes the whole generated design and restores whatever
/// the document held before (a prior generated design, a pristine
/// scratch scaffold, or the user's own pages).
///
/// Hero/section imagery is pre-rendered through the diffusion sidecar
/// *before* taking the workspace lock (so a slow generation never
/// holds the singleton): when the sidecar is ready and `useImage` is
/// set, image-bearing elements become real raster layers; otherwise
/// they degrade to a tasteful gradient placeholder. `usedImage` is
/// honest — `true` only when at least one real raster was produced.
fn apply_generated_design(
    spec: &ThemedSpec,
    design: &GeneratedDesign,
    used_llm: bool,
    command: &'static str,
) -> Result<ThemedDesignApplyResult> {
    // Pre-render hero imagery outside the workspace lock. Keyed by
    // `(page_index, element_index)` so the apply loop can swap a real
    // raster in for the matching `Image` element and fall back to the
    // gradient placeholder everywhere else.
    let rasters = prerender_imagery(design, spec.use_image);
    let used_image = !rasters.is_empty();

    // Theme palette → brand-kit colours (parsed once, reused below).
    let palette: Vec<RgbaColor> = design
        .theme
        .palette()
        .iter()
        .filter_map(|hex| RgbaColor::from_hex(hex))
        .collect();

    // Total tiled width / max height for the container page.
    let tile_count = design.pages.len();
    let total_width: f64 = design.pages.iter().map(|p| p.width).sum::<f64>()
        + THEMED_DECK_TILE_GAP * (tile_count.saturating_sub(1) as f64);
    let max_height = design
        .pages
        .iter()
        .map(|p| p.height)
        .fold(0.0_f64, f64::max);

    let format_wire = design.format.wire();
    let theme_wire = design.theme.id.wire();
    let kit_name = format!("{} Theme", design.theme.name);
    let spec_value = serde_json::to_value(spec).unwrap_or(serde_json::Value::Null);

    with_workspace_mut(|ws| {
        // Capture-then-remove our own prior output. Re-running the
        // generator (e.g. to try another theme, or a refine) must not
        // stack a second tiled deck beside the first; remove any page
        // a previous run stamped as generator-owned. The removed
        // subtrees are captured parent-first so undo can re-insert
        // them verbatim. Pages the user authored themselves carry no
        // stamp and are left untouched, so this never destroys work.
        let mut removed: Vec<Vec<Node>> = Vec::new();
        for root in ws.project.document.root_ids().to_vec() {
            if node_is_themed_generated(&ws.project.document, root) {
                removed.push(collect_subtree_parent_first(&ws.project.document, root));
                ws.project.document.remove_node(root);
            }
        }

        // Repurpose a pristine scratch document. A "generate a whole
        // deck from a prompt" action is a *document-creation* action:
        // when the open document carries no content-bearing layers
        // (the default `Page` + empty `Artboard` scaffold a fresh
        // project — and BriefModal's scratch project — ships with, or
        // the empty shell left after removing our prior output above),
        // clear it so the generated deck *becomes* the document. This
        // keeps the canvas and the multi-page PDF export free of a
        // stray blank default artboard. A document that still holds
        // real (user-authored) content is left in place and the deck
        // is appended as a new page, so we never destroy the user's
        // work. Cleared roots are captured for undo just like above.
        if !document_has_content_layers(&ws.project.document) {
            for root in ws.project.document.root_ids().to_vec() {
                removed.push(collect_subtree_parent_first(&ws.project.document, root));
                ws.project.document.remove_node(root);
            }
        }

        // Root page container, sized to the full tiled extent. It is
        // stamped as generator-owned (so a future run can replace it)
        // and carries the originating spec (so a refine can reload the
        // brief + options that produced it).
        let page_title = design
            .pages
            .first()
            .map_or_else(|| "Generated Design".to_string(), |p| p.title.clone());
        let mut page = Node::new(NodeType::Page, page_title.as_str());
        page.bounds = Bounds::new(0.0, 0.0, total_width.max(1.0), max_height.max(1.0));
        page.metadata.insert(
            THEMED_GENERATED_METADATA_KEY.to_string(),
            serde_json::Value::Bool(true),
        );
        page.metadata
            .insert(THEMED_SPEC_METADATA_KEY.to_string(), spec_value);
        let page_id = ws
            .project
            .document
            .insert_node(page)
            .map_err(DocumentBridgeError::Document)?;

        // Brand kit (upsert by name) seeded from the theme palette.
        // Snapshot the prior kit (if any) before mutating so undo can
        // restore its exact colours / remove a freshly-created one.
        let brand_kit_before: Option<BrandKit> = ws
            .project
            .brand_kits
            .iter()
            .find(|k| k.name == kit_name)
            .cloned();
        let colors: Vec<NamedColor> = palette
            .iter()
            .enumerate()
            .map(|(i, c)| NamedColor {
                name: format!("Theme {}", i + 1),
                color: *c,
            })
            .collect();
        let brand_kit_id = if let Some(kit) = ws
            .project
            .brand_kits
            .iter_mut()
            .find(|k| k.name == kit_name)
        {
            kit.colors = colors;
            kit.id
        } else {
            let mut kit = BrandKit::new(&kit_name);
            kit.colors = colors;
            let id = kit.id;
            ws.project.brand_kits.push(kit);
            id
        };
        let brand_kit_after: BrandKit = ws
            .project
            .brand_kits
            .iter()
            .find(|k| k.id == brand_kit_id)
            .cloned()
            .ok_or_else(|| {
                DocumentBridgeError::Internal("brand kit vanished after upsert".to_string())
            })?;

        let mut artboard_ids = Vec::with_capacity(tile_count);
        let mut affected: Vec<Uuid> = vec![page_id];
        let mut x_offset = 0.0_f64;
        for (pi, gp) in design.pages.iter().enumerate() {
            // Slide artboard, filled with the page background.
            let mut artboard = Node::new(NodeType::Artboard, gp.title.as_str());
            artboard.bounds = Bounds::new(x_offset, 0.0, gp.width, gp.height);
            artboard.parent_id = Some(page_id);
            artboard.style.fill = solid_fill(&gp.background);
            let artboard_id = ws
                .project
                .document
                .insert_node(artboard)
                .map_err(DocumentBridgeError::Document)?;
            artboard_ids.push(artboard_id);
            affected.push(artboard_id);

            for (ei, el) in gp.elements.iter().enumerate() {
                let node = if let Some(raster) = rasters.get(&(pi, ei)) {
                    build_raster_node(ws, el, x_offset, artboard_id, raster)?
                } else {
                    build_design_node(el, x_offset, artboard_id)
                };
                let node_id = ws
                    .project
                    .document
                    .insert_node(node)
                    .map_err(DocumentBridgeError::Document)?;
                affected.push(node_id);
            }
            x_offset += gp.width + THEMED_DECK_TILE_GAP;
        }

        // Capture the inserted subtree parent-first so redo can replay
        // it verbatim, then record one reversible operation.
        let inserted = collect_subtree_parent_first(&ws.project.document, page_id);
        ws.project.modified_at = Utc::now();
        let before = serde_json::to_value(ThemedDesignPatch {
            dir: ThemedPatchDir::Undo,
            removed: removed.clone(),
            inserted: inserted.clone(),
            inserted_root: page_id,
            brand_kit_before: brand_kit_before.clone(),
            brand_kit_after: brand_kit_after.clone(),
        })
        .map_err(|e| DocumentBridgeError::Internal(format!("encode themed undo patch: {e}")))?;
        let after = serde_json::to_value(ThemedDesignPatch {
            dir: ThemedPatchDir::Redo,
            removed,
            inserted,
            inserted_root: page_id,
            brand_kit_before,
            brand_kit_after,
        })
        .map_err(|e| DocumentBridgeError::Internal(format!("encode themed redo patch: {e}")))?;
        let op = Operation::new("user", command, before, after, affected).as_ai_generated();
        ws.project.execute_operation(op);

        Ok(ThemedDesignApplyResult {
            page_id: page_id.to_string(),
            artboard_ids: artboard_ids.iter().map(Uuid::to_string).collect(),
            brand_kit_id: brand_kit_id.to_string(),
            slide_count: tile_count as u32,
            theme_id: theme_wire.to_string(),
            theme_name: design.theme.name.clone(),
            format: format_wire.to_string(),
            used_llm,
            used_image,
        })
    })
}

/// A diffusion-generated hero image, re-encoded to a clean PNG ready
/// for the content-addressed blob store. Keyed by
/// `(page_index, element_index)` in [`prerender_imagery`].
struct RenderedRaster {
    png_bytes: Vec<u8>,
    width: u32,
    height: u32,
}

/// Diffusion steps for hero imagery — a deliberately modest count so
/// a one-shot hero renders quickly while still being recognizable.
const HERO_DIFFUSION_STEPS: u32 = 20;

/// FNV-1a offset basis / prime for the deterministic hero seed.
const HERO_FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const HERO_FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Pre-render every image-bearing element through the diffusion
/// sidecar. Returns an empty map (so every imagery slot falls back to
/// a gradient placeholder) when imagery is disabled, the format has
/// no imagery, or the sidecar is not `ready` — keeping the whole
/// feature fully offline-safe with no panics.
fn prerender_imagery(
    design: &GeneratedDesign,
    use_image: bool,
) -> HashMap<(usize, usize), RenderedRaster> {
    let mut out = HashMap::new();
    if !use_image || !design.format.supports_imagery() {
        return out;
    }
    // One readiness probe up front; if the model is not loaded we skip
    // straight to placeholders without ever hitting the network.
    if image_gen_status().state != "ready" {
        return out;
    }
    for (pi, page) in design.pages.iter().enumerate() {
        for (ei, el) in page.elements.iter().enumerate() {
            if el.kind != ElementKind::Image {
                continue;
            }
            let Some(prompt) = el.image_prompt.as_ref() else {
                continue;
            };
            let (pw, ph) = hero_pixel_size(el.width, el.height);
            let seed = hero_seed(prompt);
            // Any failure (not-ready race, transport error, malformed
            // payload) degrades to the placeholder for this element.
            let Ok(payload) =
                image_gen_generate(prompt.clone(), pw, ph, HERO_DIFFUSION_STEPS, Some(seed))
            else {
                continue;
            };
            let Ok(img) = decode_png_payload_lenient(&payload.png_b64) else {
                continue;
            };
            let Some(png_bytes) = encode_rgba_png(&img.rgba, img.width, img.height) else {
                continue;
            };
            out.insert(
                (pi, ei),
                RenderedRaster {
                    png_bytes,
                    width: img.width,
                    height: img.height,
                },
            );
        }
    }
    out
}

/// Map a hero element's world-space box to a sane diffusion output
/// size: preserve aspect ratio, cap the long edge, floor the short
/// edge, and round both to a multiple of 64 (the tile size most
/// diffusion UNets expect).
fn hero_pixel_size(width: f64, height: f64) -> (u32, u32) {
    const MAX_EDGE: f64 = 768.0;
    const MIN_EDGE: f64 = 256.0;
    let w = width.max(1.0);
    let h = height.max(1.0);
    let scale = (MAX_EDGE / w.max(h)).min(1.0);
    let round64 = |v: f64| {
        let snapped = ((v / 64.0).round() as u32).max(1) * 64;
        snapped.max(256)
    };
    let pw = round64((w * scale).clamp(MIN_EDGE, MAX_EDGE));
    let ph = round64((h * scale).clamp(MIN_EDGE, MAX_EDGE));
    (pw, ph)
}

/// Deterministic per-prompt diffusion seed (FNV-1a) so the same hero
/// prompt renders the same image on every run — generation stays
/// reproducible offline and a refine that doesn't touch a section
/// keeps its imagery stable.
fn hero_seed(prompt: &str) -> u64 {
    let mut hash = HERO_FNV_OFFSET;
    for b in prompt.as_bytes() {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(HERO_FNV_PRIME);
    }
    hash
}

/// Encode a decoded RGBA8 buffer back into PNG bytes for the blob
/// store. Returns `None` on a dimension / encode mismatch so the
/// caller degrades to the placeholder instead of panicking.
fn encode_rgba_png(rgba: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    if rgba.len() != (width as usize) * (height as usize) * 4 {
        return None;
    }
    let mut png_bytes = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut png_bytes);
    image::write_buffer_with_format(
        &mut cursor,
        rgba,
        width,
        height,
        image::ColorType::Rgba8,
        image::ImageFormat::Png,
    )
    .ok()?;
    Some(png_bytes)
}

/// Build a [`NodeType::RasterLayer`] node for a hero element backed
/// by a real diffusion-generated image. The PNG is committed to the
/// content-addressed blob store; the node's world-space bounds are
/// the element's box (the renderer scales the pixels into it via
/// `scene_sync::emit_raster`).
fn build_raster_node(
    ws: &crate::document::Workspace,
    el: &DesignElement,
    x_offset: f64,
    artboard_id: Uuid,
    raster: &RenderedRaster,
) -> Result<Node> {
    let blob = ws
        .store
        .lock()
        .blobs()
        .store(&raster.png_bytes, "image/png")
        .map_err(|e| DocumentBridgeError::Internal(format!("store hero blob: {e}")))?;
    let mut node = Node::new(NodeType::RasterLayer, element_node_name(el));
    node.parent_id = Some(artboard_id);
    node.bounds = Bounds::new(el.x + x_offset, el.y, el.width.max(1.0), el.height.max(1.0));
    node.style.corner_radius = el.corner_radius;
    let meta = RasterImageMeta {
        blob_hash: blob.hash,
        width: raster.width,
        height: raster.height,
    };
    node.metadata.insert(
        RASTER_IMAGE_METADATA_KEY.to_string(),
        serde_json::to_value(&meta)
            .map_err(|e| DocumentBridgeError::Internal(format!("encode RasterImageMeta: {e}")))?,
    );
    Ok(node)
}

/// Direction tag for a [`ThemedDesignPatch`] so the single
/// `apply_patch` arm can roll the generated design forward (redo) or
/// backward (undo) from the same payload shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ThemedPatchDir {
    Undo,
    Redo,
}

/// Reversible payload for an `ai_generate_themed_design` /
/// `ai_refine_themed_design` operation. Carries everything needed to
/// move the document graph between "before generation" and "after
/// generation" in either direction:
///
/// * `removed` — the subtrees this run deleted (prior generated
///   output and/or a pristine scratch scaffold), each captured
///   parent-first so they re-insert cleanly.
/// * `inserted` / `inserted_root` — the new generated page subtree,
///   also parent-first, and the id of its root page.
/// * `brand_kit_before` / `brand_kit_after` — the theme brand kit
///   before and after the upsert, so undo restores the prior colours
///   (or removes a freshly-created kit) and redo re-applies them.
///
/// Replay is effectively infallible (roots have no parent, children
/// follow their parents, brand-kit edits are pure vec ops), so a
/// single themed op rolls back atomically with one Ctrl+Z.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ThemedDesignPatch {
    pub(crate) dir: ThemedPatchDir,
    pub(crate) removed: Vec<Vec<Node>>,
    pub(crate) inserted: Vec<Node>,
    pub(crate) inserted_root: Uuid,
    pub(crate) brand_kit_before: Option<BrandKit>,
    pub(crate) brand_kit_after: BrandKit,
}

/// Was `id` a root page stamped by a previous run of this generator?
/// Used to replace prior generated output on a re-run without
/// touching user-authored pages (which never carry the stamp).
fn node_is_themed_generated(doc: &kcreate_core::document::DocumentGraph, id: Uuid) -> bool {
    doc.get_node(id).is_some_and(|n| {
        n.metadata
            .get(THEMED_GENERATED_METADATA_KEY)
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
    })
}

/// Does the document hold any content-bearing leaf layer?
///
/// Used to distinguish a pristine scratch document (only the default
/// `Page` + empty `Artboard` containers a freshly-created project
/// ships with) from one the user has already drawn into. Pure
/// containers (`Page` / `Artboard` / `GroupLayer` / `LayoutFrame`) on
/// their own do not count as content — only leaf layers that actually
/// carry a design (vector / text / raster / placed component) do.
fn document_has_content_layers(doc: &kcreate_core::document::DocumentGraph) -> bool {
    doc.iter().any(|(_, n)| {
        matches!(
            n.node_type,
            NodeType::VectorLayer
                | NodeType::TextLayer
                | NodeType::RasterLayer
                | NodeType::ComponentLayer
        )
    })
}

/// Build a live document node from a page-local [`DesignElement`],
/// offsetting it into world space by `x_offset` (the slide's tiled
/// world origin; tiled slides share `y = 0`).
fn build_design_node(el: &DesignElement, x_offset: f64, artboard_id: Uuid) -> Node {
    let wx = el.x + x_offset;
    let wy = el.y;
    let w = el.width.max(1.0);
    let h = el.height.max(1.0);
    match el.kind {
        ElementKind::Text => {
            let mut node = Node::new(NodeType::TextLayer, element_node_name(el));
            node.parent_id = Some(artboard_id);
            node.bounds = Bounds::new(wx, wy, w, h);
            node.style = themed_style(&el.fill, 0.0);
            let meta = TextLayerMeta {
                text: el.text.clone().unwrap_or_default(),
                font_family: if el.font_family.is_empty() {
                    "Inter".to_string()
                } else {
                    el.font_family.clone()
                },
                font_size: el.font_size,
            };
            node.metadata.insert(
                TEXT_LAYER_METADATA_KEY.to_string(),
                serde_json::to_value(&meta).unwrap_or(serde_json::Value::Null),
            );
            node
        }
        ElementKind::Rect => {
            let mut node = Node::new(NodeType::VectorLayer, element_node_name(el));
            node.parent_id = Some(artboard_id);
            node.bounds = Bounds::new(wx, wy, w, h);
            node.style = themed_style(&el.fill, el.corner_radius);
            let path = rounded_rect_path(wx, wy, w, h, el.corner_radius);
            node.metadata.insert(
                VECTOR_PATH_METADATA_KEY.to_string(),
                serde_json::to_value(&path).unwrap_or(serde_json::Value::Null),
            );
            node
        }
        // Offline / no-model fallback for a hero element: a tasteful
        // vertical gradient `VectorLayer` standing in for the absent
        // diffusion raster. (When the sidecar IS ready, the apply loop
        // substitutes a real `RasterLayer` via `build_raster_node` and
        // this arm is never reached for that element.) The gradient
        // endpoints are authored in WORLD space to match the
        // world-space path emitted below — `scene_sync::emit_vector`
        // applies no style translation to generated nodes.
        ElementKind::Image => {
            let mut node = Node::new(NodeType::VectorLayer, element_node_name(el));
            node.parent_id = Some(artboard_id);
            node.bounds = Bounds::new(wx, wy, w, h);
            node.style = NodeStyle {
                fill: gradient_placeholder_fill(el, wx, wy, h),
                corner_radius: el.corner_radius,
                ..NodeStyle::default()
            };
            let path = rounded_rect_path(wx, wy, w, h, el.corner_radius);
            node.metadata.insert(
                VECTOR_PATH_METADATA_KEY.to_string(),
                serde_json::to_value(&path).unwrap_or(serde_json::Value::Null),
            );
            node
        }
    }
}

/// Build the gradient fill for an offline hero placeholder: a
/// top→bottom linear gradient from the element's primary fill to its
/// secondary fill (falling back to the primary when none is set).
/// Endpoints are WORLD-space (`wx, wy` → `wx, wy + h`) so they line
/// up with the world-space vector path the placeholder carries.
fn gradient_placeholder_fill(el: &DesignElement, wx: f64, wy: f64, h: f64) -> FillStyle {
    let top = RgbaColor::from_hex(&el.fill).unwrap_or(RgbaColor::WHITE);
    let bottom = el
        .fill_secondary
        .as_deref()
        .and_then(RgbaColor::from_hex)
        .unwrap_or(top);
    FillStyle::Gradient(GradientKind::Linear {
        from: Point2D::new(wx, wy),
        to: Point2D::new(wx, wy + h),
        stops: vec![
            GradientStop {
                offset: 0.0,
                color: top,
            },
            GradientStop {
                offset: 1.0,
                color: bottom,
            },
        ],
    })
}

/// Stable layer name for a design element, derived from its role.
fn element_node_name(el: &DesignElement) -> &'static str {
    match el.role {
        ElementRole::Surface => "Surface",
        ElementRole::AccentBar => "Accent Bar",
        ElementRole::Title => "Title",
        ElementRole::Subtitle => "Subtitle",
        ElementRole::Heading => "Heading",
        ElementRole::Body => "Body",
        ElementRole::BulletMarker => "Bullet",
        ElementRole::Figure => "Figure",
        ElementRole::Footer => "Footer",
    }
}

/// Node style with a solid themed fill and an optional corner radius.
fn themed_style(fill_hex: &str, corner_radius: f64) -> NodeStyle {
    NodeStyle {
        fill: solid_fill(fill_hex),
        corner_radius,
        ..NodeStyle::default()
    }
}

/// Parse a `#RRGGBB` theme colour into a solid fill, defaulting to
/// white when the string is malformed (themes only ever emit valid
/// hex, so this is a defensive fallback).
fn solid_fill(hex: &str) -> FillStyle {
    FillStyle::Solid(RgbaColor::from_hex(hex).unwrap_or(RgbaColor::WHITE))
}

/// Build a rectangle path in world coordinates, rounded when
/// `radius > 0`. Rounded corners are emitted as cubic beziers so the
/// rounding survives SVG/PDF export (which reads the path, not
/// `style.corner_radius`).
fn rounded_rect_path(x: f64, y: f64, w: f64, h: f64, radius: f64) -> kcreate_vector::VectorPath {
    use kcreate_vector::{PathPoint, PathSegment, VectorPath};
    let max_r = (w.min(h)) / 2.0;
    let r = radius.clamp(0.0, max_r.max(0.0));
    if r <= f64::EPSILON {
        return VectorPath::new(vec![
            PathSegment::MoveTo(PathPoint::new(x, y)),
            PathSegment::LineTo(PathPoint::new(x + w, y)),
            PathSegment::LineTo(PathPoint::new(x + w, y + h)),
            PathSegment::LineTo(PathPoint::new(x, y + h)),
            PathSegment::Close,
        ]);
    }
    let c = r * KAPPA;
    VectorPath::new(vec![
        PathSegment::MoveTo(PathPoint::new(x + r, y)),
        PathSegment::LineTo(PathPoint::new(x + w - r, y)),
        PathSegment::CubicTo {
            ctrl1: PathPoint::new(x + w - r + c, y),
            ctrl2: PathPoint::new(x + w, y + r - c),
            end: PathPoint::new(x + w, y + r),
        },
        PathSegment::LineTo(PathPoint::new(x + w, y + h - r)),
        PathSegment::CubicTo {
            ctrl1: PathPoint::new(x + w, y + h - r + c),
            ctrl2: PathPoint::new(x + w - r + c, y + h),
            end: PathPoint::new(x + w - r, y + h),
        },
        PathSegment::LineTo(PathPoint::new(x + r, y + h)),
        PathSegment::CubicTo {
            ctrl1: PathPoint::new(x + r - c, y + h),
            ctrl2: PathPoint::new(x, y + h - r + c),
            end: PathPoint::new(x, y + h - r),
        },
        PathSegment::LineTo(PathPoint::new(x, y + r)),
        PathSegment::CubicTo {
            ctrl1: PathPoint::new(x, y + r - c),
            ctrl2: PathPoint::new(x + r - c, y),
            end: PathPoint::new(x + r, y),
        },
        PathSegment::Close,
    ])
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordered_export_units_walks_page_artboards_in_insertion_order() {
        // Page → [Artboard A, Artboard B]: the page is a pure
        // container (it has artboard descendants) so it is not emitted;
        // its artboards become the export units, left-to-right.
        let mut doc = kcreate_core::document::DocumentGraph::new();
        let page = doc.insert_node(Node::new(NodeType::Page, "Page")).unwrap();
        let mut a = Node::new(NodeType::Artboard, "A");
        a.parent_id = Some(page);
        let a = doc.insert_node(a).unwrap();
        let mut b = Node::new(NodeType::Artboard, "B");
        b.parent_id = Some(page);
        let b = doc.insert_node(b).unwrap();
        assert_eq!(ordered_export_units(&doc), vec![a, b]);
    }

    #[test]
    fn ordered_export_units_emits_a_page_with_no_artboard_as_itself() {
        // Legacy single-page docs hang content directly off the page
        // (no artboard); the page itself becomes the one export unit.
        let mut doc = kcreate_core::document::DocumentGraph::new();
        let page = doc.insert_node(Node::new(NodeType::Page, "Page")).unwrap();
        let mut text = Node::new(NodeType::TextLayer, "Body");
        text.parent_id = Some(page);
        doc.insert_node(text).unwrap();
        assert_eq!(ordered_export_units(&doc), vec![page]);
    }

    #[test]
    fn ordered_export_units_reaches_artboard_nested_under_a_group() {
        // Page → GroupLayer → Artboard. The page reports an artboard
        // descendant, so it descends; the group is not a page on its
        // own but the traversal descends through it instead of dropping
        // it, so the nested artboard is still reached (one export unit,
        // not zero).
        let mut doc = kcreate_core::document::DocumentGraph::new();
        let page = doc.insert_node(Node::new(NodeType::Page, "Page")).unwrap();
        let mut group = Node::new(NodeType::GroupLayer, "Group");
        group.parent_id = Some(page);
        let group = doc.insert_node(group).unwrap();
        let mut artboard = Node::new(NodeType::Artboard, "Nested");
        artboard.parent_id = Some(group);
        let artboard = doc.insert_node(artboard).unwrap();
        assert_eq!(ordered_export_units(&doc), vec![artboard]);
    }

    #[test]
    fn refine_directive_reduce_intent_wins_over_structure_nouns() {
        // Regression: bare structure nouns ("section", "slide", "page",
        // "bullet") used to imply MoreContent and were checked before
        // LessContent, so a clear *reduction* that happened to name a
        // structural unit grew the design instead of shrinking it.
        for instruction in [
            "fewer sections",
            "trim the slides down",
            "cut a page",
            "reduce the number of slides",
            "remove a bullet",
            "drop a slide",
            "delete the last section",
            "shorten it by a page",
        ] {
            assert_eq!(
                parse_refine_directive(instruction),
                RefineDirective::LessContent,
                "{instruction:?} should reduce content"
            );
        }
    }

    #[test]
    fn refine_directive_add_intent_still_increases() {
        for instruction in [
            "add a pricing slide",
            "another section please",
            "expand the deck",
            "more detail on pricing",
            "give it an extra page",
            "make the body longer",
        ] {
            assert_eq!(
                parse_refine_directive(instruction),
                RefineDirective::MoreContent,
                "{instruction:?} should add content"
            );
        }
    }

    #[test]
    fn refine_directive_imagery_intent_is_checked_first() {
        assert_eq!(
            parse_refine_directive("add a hero image"),
            RefineDirective::AddImagery
        );
        assert_eq!(
            parse_refine_directive("more photos"),
            RefineDirective::AddImagery
        );
        // Negated imagery wins over the section-count buckets even though
        // "remove"/"fewer" are also reduce verbs.
        assert_eq!(
            parse_refine_directive("remove the hero image"),
            RefineDirective::RemoveImagery
        );
        assert_eq!(
            parse_refine_directive("no illustrations"),
            RefineDirective::RemoveImagery
        );
    }

    #[test]
    fn refine_directive_minimal_and_rephrase() {
        assert_eq!(
            parse_refine_directive("make it more minimal"),
            RefineDirective::Minimal
        );
        assert_eq!(
            parse_refine_directive("less busy please"),
            RefineDirective::Minimal
        );
        // No structural / imagery intent → just re-derive the copy.
        assert_eq!(
            parse_refine_directive("punchier headlines"),
            RefineDirective::Rephrase
        );
        assert_eq!(
            parse_refine_directive("friendlier tone"),
            RefineDirective::Rephrase
        );
    }

    #[test]
    fn refine_directive_word_matching_avoids_substring_false_positives() {
        // "executive" contains "cut" as a substring; whole-word matching
        // must not treat it as a LessContent reduce verb. With no real
        // intent verb present this is a pure rephrase.
        assert_eq!(
            parse_refine_directive("make the tone more executive"),
            RefineDirective::MoreContent,
            "an explicit 'more' should still add, despite 'executive' containing 'cut'"
        );
        assert_eq!(
            parse_refine_directive("a more executive voice"),
            RefineDirective::MoreContent
        );
    }

    #[test]
    fn smart_select_mode_parser_round_trips_known_names() {
        assert_eq!(
            SmartSelectMode::from_wire("replace"),
            Some(SmartSelectMode::Replace)
        );
        assert_eq!(
            SmartSelectMode::from_wire("add"),
            Some(SmartSelectMode::Add)
        );
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
        // Phase A2 — the new `export` section round-trips empty.
        assert!(back.export.last_dir_by_format.is_empty());
        assert!(back.export.last_batch_dir.is_none());
        // Phase C — default onboarding is "not yet completed" so the
        // welcome modal fires on first run for every new install.
        assert!(!back.onboarding.completed);
        assert!(back.onboarding.last_seen_pack_id.is_none());
    }

    #[test]
    fn preferences_onboarding_section_round_trips_completion_state() {
        // After the welcome modal closes (any path), the renderer
        // flips `completed = true` and records the recommended
        // pack id it surfaced so a future tier-change pass can
        // detect when the recommendation crossed a boundary.
        let mut prefs = Preferences::default();
        prefs.onboarding.completed = true;
        prefs.onboarding.last_seen_pack_id = Some("llm_bonsai_4b".into());
        let s = serde_json::to_string(&prefs).expect("serialize");
        let back: Preferences = serde_json::from_str(&s).expect("deserialize");
        assert!(back.onboarding.completed);
        assert_eq!(
            back.onboarding.last_seen_pack_id.as_deref(),
            Some("llm_bonsai_4b")
        );
    }

    #[test]
    fn preferences_legacy_file_without_onboarding_section_deserialises() {
        // A preferences.json written before Phase C has no
        // `onboarding` section — the renderer must still be able
        // to load such a file (the new field defaults to a
        // not-yet-completed `OnboardingPrefs`, which correctly
        // triggers the first-run welcome modal for users who are
        // upgrading from a pre-Phase-C build).
        let legacy = serde_json::json!({
            "general": {
                "theme": "dark",
                "language": "en-US",
                "autosaveIntervalSec": 60,
                "scratchProjectCleanupDays": 30,
            },
            "canvas": {
                "defaultGridSpacing": 16.0,
                "defaultGridSubdivisions": 4,
                "snapThresholdPx": 6.0,
                "rulerUnits": "px",
            },
            "ai": {
                "defaultLlmModel": "",
                "autoStartSidecar": false,
                "gbnfGrammarDebugging": false,
            },
            "performance": {
                "rasterCacheBudgetMb": 512,
                "undoDepthOverride": null,
                "lowResourceMode": false,
            },
            "privacy": {
                "telemetryOptIn": false,
                "auditLogRetentionDays": 90,
            },
            "export": {
                "lastDirByFormat": {},
                "lastBatchDir": null,
            },
            // No `onboarding` field — must default cleanly so the
            // upgrading user gets the welcome modal exactly once.
        });
        let prefs: Preferences = serde_json::from_value(legacy).expect("legacy preferences load");
        assert_eq!(prefs.general.theme, "dark");
        assert!(!prefs.onboarding.completed);
        assert!(prefs.onboarding.last_seen_pack_id.is_none());
    }

    #[test]
    fn preferences_export_section_round_trips_sticky_dirs() {
        // Pre-populate per-format sticky directories and a batch
        // dir; the round-trip must preserve every entry verbatim.
        let mut prefs = Preferences::default();
        prefs
            .export
            .last_dir_by_format
            .insert("png".into(), "/home/u/exports/png".into());
        prefs
            .export
            .last_dir_by_format
            .insert("svg".into(), "/home/u/exports/svg".into());
        prefs.export.last_batch_dir = Some("/home/u/exports/batch".into());
        let s = serde_json::to_string(&prefs).expect("serialize");
        let back: Preferences = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(
            back.export.last_dir_by_format.get("png"),
            Some(&"/home/u/exports/png".to_string())
        );
        assert_eq!(
            back.export.last_dir_by_format.get("svg"),
            Some(&"/home/u/exports/svg".to_string())
        );
        assert_eq!(
            back.export.last_batch_dir,
            Some("/home/u/exports/batch".into())
        );
    }

    #[test]
    fn preferences_legacy_file_without_export_section_deserialises() {
        // A preferences.json written before Phase A2 has no `export`
        // section — the renderer must still be able to load such a
        // file (the new field defaults to an empty `ExportPrefs`).
        let legacy = serde_json::json!({
            "general": {
                "theme": "dark",
                "language": "en-US",
                "autosaveIntervalSec": 60,
                "scratchProjectCleanupDays": 30,
            },
            "canvas": {
                "defaultGridSpacing": 16.0,
                "defaultGridSubdivisions": 4,
                "snapThresholdPx": 6.0,
                "rulerUnits": "px",
            },
            "ai": {
                "defaultLlmModel": "",
                "autoStartSidecar": false,
                "gbnfGrammarDebugging": false,
            },
            "performance": {
                "rasterCacheBudgetMb": 512,
                "undoDepthOverride": null,
                "lowResourceMode": false,
            },
            "privacy": {
                "telemetryOptIn": false,
                "auditLogRetentionDays": 90,
            },
            // No `export` field — must default cleanly.
        });
        let prefs: Preferences = serde_json::from_value(legacy).expect("legacy preferences load");
        assert_eq!(prefs.general.theme, "dark");
        assert!(prefs.export.last_dir_by_format.is_empty());
        assert!(prefs.export.last_batch_dir.is_none());
    }

    #[test]
    fn scale_to_fit_caps_longest_side() {
        assert_eq!(scale_to_fit(2000, 1000, 1024), (1024, 512));
        assert_eq!(scale_to_fit(500, 500, 1024), (500, 500));
    }

    #[test]
    fn preferences_tmp_path_is_sibling_of_final_path() {
        // The temp path MUST live next to the final path so the
        // subsequent `rename` is on the same filesystem and is
        // therefore atomic on POSIX. A temp on a different
        // filesystem would fall back to copy+delete and lose
        // atomicity.
        let final_path = std::path::PathBuf::from("/home/user/.kcreate/preferences.json");
        let tmp = preferences_tmp_path(&final_path);
        assert_eq!(tmp.parent(), final_path.parent());
        let tmp_name = tmp.file_name().unwrap().to_string_lossy().into_owned();
        assert!(tmp_name.starts_with("preferences.json.tmp-"));
        // pid suffix uniquifies the temp so two concurrent processes
        // don't clobber each other mid-write.
        let pid = std::process::id().to_string();
        assert!(tmp_name.ends_with(&pid));
    }
}
