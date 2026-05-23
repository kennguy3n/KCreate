//! PDF export driven by the [`DocumentGraph`].
//!
//! Walks the document, collects every visible vector / raster layer,
//! and emits a one-page PDF using `printpdf`. Vector paths are
//! flattened into PDF path operators; raster layers are embedded as
//! one image `XObject` per layer. The export is local-first: no I/O
//! beyond writing the destination file.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use image::{GenericImageView, ImageFormat};
use kcreate_core::color::{srgb_to_cmyk, Color};
use kcreate_core::document::DocumentGraph;
use kcreate_core::node::{FillStyle, Node, NodeType};
use kcreate_vector::{PathPoint, PathSegment, VectorPath};
use printpdf::path::{PaintMode, WindingOrder};
use printpdf::{
    Cmyk, Image, ImageTransform, Mm, PdfDocument, PdfLayerReference, Point, Polygon, Rgb,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

pub use crate::scene_metadata::{RASTER_IMAGE_METADATA_KEY, VECTOR_PATH_METADATA_KEY};

/// Target color space used when writing PDF color operators.
///
/// * `Rgb` — every fill is written with `rg` / `RG` (DeviceRGB). The
///   default; matches the pre-Phase-2 behaviour exactly.
/// * `Cmyk` — every fill is written with `k` / `K` (DeviceCMYK).
///   `Color::Srgb` values are converted via
///   [`kcreate_core::color::srgb_to_cmyk`]; `Color::Cmyk` values pass
///   through unchanged so authored CMYK values do not get mangled by
///   a round-trip through sRGB.
/// * `PassThrough` — each fill is emitted in its native space. RGB
///   fills become `rg` / `RG`; CMYK overrides become `k` / `K`. This
///   is what print shops typically expect when handed a
///   mixed-color-space document.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PdfColorMode {
    #[default]
    Rgb,
    Cmyk,
    PassThrough,
}

/// PDF export options.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PdfExportOptions {
    pub width_mm: f64,
    pub height_mm: f64,
    pub title: String,
    /// Which device color space to write to the PDF content stream.
    /// Defaults to `Rgb` so callers that never opted into the
    /// Phase 2 CMYK pipeline keep producing byte-identical PDFs.
    pub color_mode: PdfColorMode,
}

impl Default for PdfExportOptions {
    fn default() -> Self {
        Self {
            width_mm: 210.0,
            height_mm: 297.0,
            title: "KCreate document".to_string(),
            color_mode: PdfColorMode::Rgb,
        }
    }
}

/// Resolve the authoritative fill color for a node, taking the
/// optional [`NodeStyle::color_override`] into account.
///
/// The renderer always uses [`NodeStyle::fill`] as the source of
/// truth for *whether* a node is painted (a `FillStyle::None` or a
/// zero-alpha `Solid` produces no draw call). `color_override` only
/// changes *what color* the fill is — it is an export-time color-
/// space hint, not an "I am suddenly visible" toggle. The exporter
/// must therefore key its visibility decision off `fill` and only
/// substitute the override after that gate has passed; otherwise a
/// node that was invisible on the canvas would silently appear in
/// the printed PDF.
///
/// Returns `None` when the node has no visible fill, in which case
/// the caller skips the fill operator entirely.
fn resolve_fill_color(node: &Node) -> Option<Color> {
    // 1. Decide visibility purely from `fill`. This must match the
    //    renderer's painted/not-painted decision exactly.
    let fill_alpha = match node.style.fill {
        FillStyle::Solid(rgba) if rgba.a > 0.0 => rgba.a,
        FillStyle::Solid(_) | FillStyle::None | FillStyle::Gradient(_) => return None,
    };

    // 2. Apply the override. The override is authored in its native
    //    color space (CMYK, Lab, …) and is the canonical color value
    //    for export. Its own alpha is preserved only when it is
    //    strictly less than fully opaque — otherwise we keep the
    //    `fill`'s alpha so partial-opacity from the renderer side
    //    survives a CMYK override that defaulted to alpha=1.0.
    if let Some(over) = &node.style.color_override {
        return Some(merge_override_alpha(over.clone(), fill_alpha));
    }

    // 3. No override: pass the renderer's fill through as sRGB.
    let FillStyle::Solid(rgba) = node.style.fill else {
        // Unreachable because step 1 returned for every non-Solid
        // variant, but kept exhaustive so a future variant doesn't
        // silently fall through.
        return None;
    };
    Some(Color::Srgb {
        r: rgba.r,
        g: rgba.g,
        b: rgba.b,
        a: rgba.a,
    })
}

/// Stitch a renderer-side fill alpha onto an export-time
/// `color_override`. If the override explicitly authored a
/// less-than-opaque alpha we trust it; otherwise we use the
/// renderer's value so changes the user made to the visual fill
/// alpha still take effect in the exported PDF.
fn merge_override_alpha(over: Color, fill_alpha: f32) -> Color {
    let override_alpha = over.alpha();
    let final_alpha = if override_alpha < 1.0 {
        override_alpha
    } else {
        fill_alpha
    };
    match over {
        Color::Srgb { r, g, b, .. } => Color::Srgb {
            r,
            g,
            b,
            a: final_alpha,
        },
        Color::Cmyk { c, m, y, k, .. } => Color::Cmyk {
            c,
            m,
            y,
            k,
            a: final_alpha,
        },
        Color::Lab {
            l, a_star, b_star, ..
        } => Color::Lab {
            l,
            a_star,
            b_star,
            alpha: final_alpha,
        },
        Color::Hsl { h, s, l, .. } => Color::Hsl {
            h,
            s,
            l,
            a: final_alpha,
        },
    }
}

/// Map a [`Color`] to the `printpdf` color enum that the requested
/// output mode dictates.
///
/// - `Cmyk` mode emits `DeviceCMYK` for every fill, converting sRGB →
///   CMYK on the fly via [`srgb_to_cmyk`]. Authored CMYK values pass
///   through unchanged so the K-channel survives.
/// - `PassThrough` mode keeps each color in its native space: CMYK
///   stays CMYK, everything else falls back to sRGB.
/// - `Rgb` mode (the default) collapses every input to `DeviceRGB`.
fn color_to_printpdf(c: &Color, mode: PdfColorMode) -> printpdf::Color {
    let (r, g, b, _a) = c.to_srgb();
    match (mode, c) {
        // Both `Cmyk` mode and `PassThrough` mode preserve authored
        // CMYK exactly so we can route them through the same arm.
        (PdfColorMode::Cmyk | PdfColorMode::PassThrough, Color::Cmyk { c, m, y, k, .. }) => {
            printpdf::Color::Cmyk(Cmyk::new(*c, *m, *y, *k, None))
        }
        (PdfColorMode::Cmyk, _) => {
            let (cc, mm, yy, kk) = srgb_to_cmyk(r, g, b);
            printpdf::Color::Cmyk(Cmyk::new(cc, mm, yy, kk, None))
        }
        (PdfColorMode::PassThrough | PdfColorMode::Rgb, _) => {
            printpdf::Color::Rgb(Rgb::new(r, g, b, None))
        }
    }
}

#[derive(Debug, Error)]
pub enum PdfExportError {
    #[error("node {0} has invalid `{VECTOR_PATH_METADATA_KEY}` metadata: {1}")]
    InvalidVectorPath(Uuid, String),
    #[error("invalid raster metadata on node {0}: {1}")]
    InvalidRasterMeta(Uuid, String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("image decode: {0}")]
    Image(String),
    #[error("printpdf: {0}")]
    PrintPdf(String),
}

pub type RasterPixelCache = HashMap<String, RasterPixels>;

#[derive(Debug, Clone)]
pub struct RasterPixels {
    pub width: u32,
    pub height: u32,
    /// RGBA8 buffer, `width * height * 4` bytes.
    pub rgba: Vec<u8>,
}

impl RasterPixels {
    pub fn decode(bytes: &[u8]) -> Result<Self, PdfExportError> {
        let img =
            image::load_from_memory(bytes).map_err(|e| PdfExportError::Image(e.to_string()))?;
        let (width, height) = img.dimensions();
        let rgba = img.to_rgba8().into_raw();
        Ok(Self {
            width,
            height,
            rgba,
        })
    }
}

/// Export `document` to a one-page PDF at `output_path`. Returns the
/// number of bytes written.
pub fn export_pdf_from_document(
    document: &DocumentGraph,
    options: &PdfExportOptions,
    rasters: &RasterPixelCache,
    output_path: &Path,
) -> Result<usize, PdfExportError> {
    let width_mm = as_f32(options.width_mm);
    let height_mm = as_f32(options.height_mm);
    let (doc, page1, layer1) =
        PdfDocument::new(&options.title, Mm(width_mm), Mm(height_mm), "Layer 1");
    let layer = doc.get_page(page1).get_layer(layer1);

    let (origin_x, origin_y, world_w, world_h) = world_bounds(document);
    let sx = options.width_mm / world_w.max(1.0);
    let sy = options.height_mm / world_h.max(1.0);

    walk_nodes(
        document,
        document.root_ids(),
        &layer,
        rasters,
        origin_x,
        origin_y,
        sx,
        sy,
        options.height_mm,
        options.color_mode,
    )?;

    let bytes = doc
        .save_to_bytes()
        .map_err(|e| PdfExportError::PrintPdf(e.to_string()))?;
    fs::write(output_path, &bytes)?;
    Ok(bytes.len())
}

const fn as_f32(value: f64) -> f32 {
    // PDF page units fit easily within f32 range, but we still clamp
    // so a NaN/inf in user input becomes a defined fallback instead of
    // a panic downstream.
    if value.is_finite() {
        value as f32
    } else {
        0.0
    }
}

#[allow(clippy::too_many_arguments)]
fn walk_nodes(
    document: &DocumentGraph,
    ids: &[Uuid],
    layer: &PdfLayerReference,
    rasters: &RasterPixelCache,
    origin_x: f64,
    origin_y: f64,
    sx: f64,
    sy: f64,
    page_height_mm: f64,
    color_mode: PdfColorMode,
) -> Result<(), PdfExportError> {
    for id in ids {
        let Some(node) = document.get_node(*id) else {
            continue;
        };
        if !node.visible {
            continue;
        }
        match node.node_type {
            NodeType::VectorLayer => {
                emit_vector(
                    node,
                    layer,
                    origin_x,
                    origin_y,
                    sx,
                    sy,
                    page_height_mm,
                    color_mode,
                )?;
            }
            NodeType::RasterLayer => {
                emit_raster(
                    node,
                    layer,
                    rasters,
                    origin_x,
                    origin_y,
                    sx,
                    sy,
                    page_height_mm,
                )?;
            }
            _ => {}
        }
        walk_nodes(
            document,
            &node.children,
            layer,
            rasters,
            origin_x,
            origin_y,
            sx,
            sy,
            page_height_mm,
            color_mode,
        )?;
    }
    Ok(())
}

fn world_bounds(document: &DocumentGraph) -> (f64, f64, f64, f64) {
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for id in document.root_ids() {
        accumulate_bounds(
            document, *id, &mut min_x, &mut min_y, &mut max_x, &mut max_y,
        );
    }
    if !min_x.is_finite() || !min_y.is_finite() {
        return (0.0, 0.0, 1.0, 1.0);
    }
    let w = (max_x - min_x).max(1.0);
    let h = (max_y - min_y).max(1.0);
    (min_x, min_y, w, h)
}

fn accumulate_bounds(
    document: &DocumentGraph,
    id: Uuid,
    min_x: &mut f64,
    min_y: &mut f64,
    max_x: &mut f64,
    max_y: &mut f64,
) {
    let Some(node) = document.get_node(id) else {
        return;
    };
    if node.visible {
        let b = &node.bounds;
        let tx = node.transform.tx;
        let ty = node.transform.ty;
        *min_x = min_x.min(b.x + tx);
        *min_y = min_y.min(b.y + ty);
        *max_x = max_x.max(b.x + tx + b.width);
        *max_y = max_y.max(b.y + ty + b.height);
    }
    for child in &node.children {
        accumulate_bounds(document, *child, min_x, min_y, max_x, max_y);
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_vector(
    node: &Node,
    layer: &PdfLayerReference,
    origin_x: f64,
    origin_y: f64,
    sx: f64,
    sy: f64,
    page_height_mm: f64,
    color_mode: PdfColorMode,
) -> Result<(), PdfExportError> {
    let Some(value) = node.metadata.get(VECTOR_PATH_METADATA_KEY) else {
        return Ok(());
    };
    let path: VectorPath = serde_json::from_value(value.clone())
        .map_err(|e| PdfExportError::InvalidVectorPath(node.id, e.to_string()))?;

    let fill_color = resolve_fill_color(node);
    let has_fill = fill_color.is_some();

    // Build rings: each sub-path between `MoveTo` and `Close` is a
    // ring of the polygon. Open sub-paths still become a single open
    // ring — we let printpdf paint them with `Stroke`.
    let mut rings: Vec<Vec<(Point, bool)>> = Vec::new();
    let mut current: Vec<(Point, bool)> = Vec::new();
    let mut last = PathPoint::new(0.0, 0.0);
    let mut start = PathPoint::new(0.0, 0.0);

    let push_point =
        |buf: &mut Vec<(Point, bool)>, p: PathPoint, is_ctrl: bool, tx: f64, ty: f64| {
            buf.push((
                world_to_pdf(
                    p.x + tx,
                    p.y + ty,
                    origin_x,
                    origin_y,
                    sx,
                    sy,
                    page_height_mm,
                ),
                is_ctrl,
            ));
        };

    for seg in &path.commands {
        match *seg {
            PathSegment::MoveTo(p) => {
                if !current.is_empty() {
                    rings.push(std::mem::take(&mut current));
                }
                push_point(&mut current, p, false, node.transform.tx, node.transform.ty);
                last = p;
                start = p;
            }
            PathSegment::LineTo(p) => {
                push_point(&mut current, p, false, node.transform.tx, node.transform.ty);
                last = p;
            }
            PathSegment::QuadTo { ctrl, end } => {
                // Promote to cubic.
                const TWO_THIRDS: f64 = 2.0 / 3.0;
                let c1 = PathPoint::new(
                    TWO_THIRDS.mul_add(ctrl.x - last.x, last.x),
                    TWO_THIRDS.mul_add(ctrl.y - last.y, last.y),
                );
                let c2 = PathPoint::new(
                    TWO_THIRDS.mul_add(ctrl.x - end.x, end.x),
                    TWO_THIRDS.mul_add(ctrl.y - end.y, end.y),
                );
                push_point(&mut current, c1, true, node.transform.tx, node.transform.ty);
                push_point(&mut current, c2, true, node.transform.tx, node.transform.ty);
                push_point(
                    &mut current,
                    end,
                    false,
                    node.transform.tx,
                    node.transform.ty,
                );
                last = end;
            }
            PathSegment::CubicTo { ctrl1, ctrl2, end } => {
                push_point(
                    &mut current,
                    ctrl1,
                    true,
                    node.transform.tx,
                    node.transform.ty,
                );
                push_point(
                    &mut current,
                    ctrl2,
                    true,
                    node.transform.tx,
                    node.transform.ty,
                );
                push_point(
                    &mut current,
                    end,
                    false,
                    node.transform.tx,
                    node.transform.ty,
                );
                last = end;
            }
            PathSegment::Close => {
                push_point(
                    &mut current,
                    start,
                    false,
                    node.transform.tx,
                    node.transform.ty,
                );
                rings.push(std::mem::take(&mut current));
                last = start;
            }
        }
    }
    if !current.is_empty() {
        rings.push(current);
    }
    if rings.is_empty() {
        return Ok(());
    }

    let fill_pdf_color = fill_color
        .as_ref()
        .map_or_else(
            || printpdf::Color::Rgb(Rgb::new(0.0, 0.0, 0.0, None)),
            |c| color_to_printpdf(c, color_mode),
        );
    // Stroke (outline) matches the requested color mode so we never
    // mix `rg` and `K` operators in the same content stream when the
    // caller asked for pure DeviceCMYK output.
    let outline_pdf_color = match color_mode {
        PdfColorMode::Cmyk => printpdf::Color::Cmyk(Cmyk::new(0.0, 0.0, 0.0, 1.0, None)),
        PdfColorMode::Rgb | PdfColorMode::PassThrough => {
            printpdf::Color::Rgb(Rgb::new(0.0, 0.0, 0.0, None))
        }
    };
    layer.set_fill_color(fill_pdf_color);
    layer.set_outline_color(outline_pdf_color);
    let mode = if has_fill {
        PaintMode::Fill
    } else {
        PaintMode::Stroke
    };
    let polygon = Polygon {
        rings,
        mode,
        winding_order: WindingOrder::NonZero,
    };
    layer.add_polygon(polygon);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn emit_raster(
    node: &Node,
    layer: &PdfLayerReference,
    rasters: &RasterPixelCache,
    origin_x: f64,
    origin_y: f64,
    sx: f64,
    sy: f64,
    page_height_mm: f64,
) -> Result<(), PdfExportError> {
    #[derive(Deserialize)]
    struct Meta {
        blob_hash: String,
        #[serde(default)]
        width: u32,
        #[serde(default)]
        height: u32,
    }
    let Some(value) = node.metadata.get(RASTER_IMAGE_METADATA_KEY) else {
        return Ok(());
    };
    let meta: Meta = serde_json::from_value(value.clone())
        .map_err(|e| PdfExportError::InvalidRasterMeta(node.id, e.to_string()))?;
    let Some(pixels) = rasters.get(&meta.blob_hash) else {
        return Ok(());
    };

    // Re-encode to PNG so printpdf can decode through its `image`
    // re-export without needing the original mime type.
    let mut png_bytes: Vec<u8> = Vec::new();
    {
        let mut cursor = std::io::Cursor::new(&mut png_bytes);
        image::write_buffer_with_format(
            &mut cursor,
            &pixels.rgba,
            pixels.width,
            pixels.height,
            image::ColorType::Rgba8,
            ImageFormat::Png,
        )
        .map_err(|e| PdfExportError::Image(e.to_string()))?;
    }

    let dyn_img = printpdf::image_crate::load_from_memory(&png_bytes)
        .map_err(|e| PdfExportError::Image(e.to_string()))?;
    let pdf_image = Image::from_dynamic_image(&dyn_img);

    let dpi = 300.0_f32;
    let world_x = node.bounds.x + node.transform.tx;
    let world_y = node.bounds.y + node.transform.ty;
    let pdf_x = (world_x - origin_x) * sx;
    let pdf_y_top = (world_y - origin_y) * sy;
    let pdf_y = node.bounds.height.mul_add(-sy, page_height_mm - pdf_y_top);

    // printpdf scale_x/y are relative to a 1pt = 1px @ dpi mapping —
    // i.e. it draws the image at `width_px / dpi * 72` points, then
    // multiplies by scale_{x,y}. We compute the scale that maps the
    // pixel buffer's natural mm size (at `dpi`) to the node's bounds.
    let bounds_w_mm = node.bounds.width * sx;
    let bounds_h_mm = node.bounds.height * sy;
    let natural_w_mm = f64::from(meta.width.max(1)) * (25.4 / f64::from(dpi));
    let natural_h_mm = f64::from(meta.height.max(1)) * (25.4 / f64::from(dpi));
    let scale_x = as_f32(bounds_w_mm / natural_w_mm.max(0.000_1));
    let scale_y = as_f32(bounds_h_mm / natural_h_mm.max(0.000_1));

    let transform = ImageTransform {
        translate_x: Some(Mm(as_f32(pdf_x))),
        translate_y: Some(Mm(as_f32(pdf_y))),
        rotate: None,
        scale_x: Some(scale_x),
        scale_y: Some(scale_y),
        dpi: Some(dpi),
    };
    pdf_image.add_to_layer(layer.clone(), transform);
    Ok(())
}

fn world_to_pdf(
    wx: f64,
    wy: f64,
    origin_x: f64,
    origin_y: f64,
    sx: f64,
    sy: f64,
    page_height_mm: f64,
) -> Point {
    // PDF origin is bottom-left; document origin is top-left.
    let x_mm = (wx - origin_x) * sx;
    let y_mm = (wy - origin_y).mul_add(-sy, page_height_mm);
    Point::new(Mm(as_f32(x_mm)), Mm(as_f32(y_mm)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use kcreate_core::node::{Bounds, Node, NodeType};

    fn vector_node(path: &VectorPath, x: f64, y: f64, width: f64, height: f64) -> Node {
        let mut node = Node::new(NodeType::VectorLayer, "shape");
        node.bounds = Bounds {
            x,
            y,
            width,
            height,
        };
        node.metadata.insert(
            VECTOR_PATH_METADATA_KEY.to_string(),
            serde_json::to_value(path).unwrap(),
        );
        node
    }

    #[test]
    fn pdf_export_writes_pdf_header() {
        let mut doc = DocumentGraph::new();
        let page = doc.insert_node(Node::new(NodeType::Page, "Page")).unwrap();
        let rect = VectorPath::new(vec![
            PathSegment::MoveTo(PathPoint::new(0.0, 0.0)),
            PathSegment::LineTo(PathPoint::new(100.0, 0.0)),
            PathSegment::LineTo(PathPoint::new(100.0, 100.0)),
            PathSegment::LineTo(PathPoint::new(0.0, 100.0)),
            PathSegment::Close,
        ]);
        let mut node = vector_node(&rect, 0.0, 0.0, 100.0, 100.0);
        node.parent_id = Some(page);
        doc.insert_node(node).unwrap();

        let tmp = tempfile::NamedTempFile::new().unwrap();
        let opts = PdfExportOptions::default();
        let rasters = RasterPixelCache::new();
        let bytes = export_pdf_from_document(&doc, &opts, &rasters, tmp.path()).unwrap();
        assert!(bytes > 0);
        let written = std::fs::read(tmp.path()).unwrap();
        assert!(written.starts_with(b"%PDF-"));
    }

    /// printpdf compresses content streams by default. The compressed
    /// stream object is the most-recently-written stream in the PDF;
    /// decompress it so we can scrub for raw operator tokens (`k`,
    /// `rg`, etc.) without relying on un-deflated content.
    fn pdf_content_stream_text(pdf: &[u8]) -> String {
        // Find every "stream\n…endstream" body and try to inflate each;
        // concatenate the human-readable result. PDF allows ASCII
        // content streams too, so include the raw bytes verbatim when
        // they don't look compressed.
        use flate2::read::ZlibDecoder;
        use std::io::Read;
        let mut out = String::new();
        let mut cursor = 0usize;
        while let Some(rel) = pdf[cursor..].windows(7).position(|w| w == b"stream\n") {
            let start = cursor + rel + 7;
            let Some(rel_end) = pdf[start..].windows(9).position(|w| w == b"endstream") else {
                break;
            };
            let body = &pdf[start..start + rel_end];
            let mut buf = Vec::new();
            if ZlibDecoder::new(body).read_to_end(&mut buf).is_ok() {
                if let Ok(s) = std::str::from_utf8(&buf) {
                    out.push_str(s);
                    out.push('\n');
                }
            } else if let Ok(s) = std::str::from_utf8(body) {
                out.push_str(s);
                out.push('\n');
            }
            cursor = start + rel_end + 9;
        }
        out
    }

    fn doc_with_red_rect() -> DocumentGraph {
        let mut doc = DocumentGraph::new();
        let page = doc.insert_node(Node::new(NodeType::Page, "Page")).unwrap();
        let rect = VectorPath::new(vec![
            PathSegment::MoveTo(PathPoint::new(0.0, 0.0)),
            PathSegment::LineTo(PathPoint::new(100.0, 0.0)),
            PathSegment::LineTo(PathPoint::new(100.0, 100.0)),
            PathSegment::LineTo(PathPoint::new(0.0, 100.0)),
            PathSegment::Close,
        ]);
        let mut node = vector_node(&rect, 0.0, 0.0, 100.0, 100.0);
        node.parent_id = Some(page);
        node.style.fill = FillStyle::Solid(kcreate_core::node::RgbaColor {
            r: 1.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        });
        doc.insert_node(node).unwrap();
        doc
    }

    #[test]
    fn pdf_export_default_writes_rgb_operators() {
        let doc = doc_with_red_rect();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let opts = PdfExportOptions::default();
        let rasters = RasterPixelCache::new();
        export_pdf_from_document(&doc, &opts, &rasters, tmp.path()).unwrap();
        let written = std::fs::read(tmp.path()).unwrap();
        let stream = pdf_content_stream_text(&written);
        // DeviceRGB tokens for non-stroking + stroking color.
        assert!(
            stream.contains(" rg") || stream.contains(" RG"),
            "expected `rg`/`RG` operator in {stream:?}"
        );
        // Default mode must NOT have emitted CMYK operators.
        assert!(
            !stream.contains(" k\n") && !stream.contains(" K\n"),
            "default mode should not emit `k`/`K`, got {stream:?}"
        );
    }

    #[test]
    fn pdf_export_cmyk_mode_writes_cmyk_operators() {
        let doc = doc_with_red_rect();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let opts = PdfExportOptions {
            color_mode: PdfColorMode::Cmyk,
            ..PdfExportOptions::default()
        };
        let rasters = RasterPixelCache::new();
        export_pdf_from_document(&doc, &opts, &rasters, tmp.path()).unwrap();
        let written = std::fs::read(tmp.path()).unwrap();
        let stream = pdf_content_stream_text(&written);
        // DeviceCMYK operator (`k` lowercase = non-stroking).
        assert!(
            stream.contains(" k\n") || stream.contains(" k "),
            "expected `k` (non-stroking CMYK) operator in {stream:?}"
        );
        // And no `rg` non-stroking RGB operator (we wrote the stroke
        // in CMYK too, so neither `rg` nor `RG` should appear).
        assert!(
            !stream.contains(" rg\n") && !stream.contains(" RG\n"),
            "CMYK mode should not emit `rg`/`RG`, got {stream:?}"
        );
    }

    #[test]
    fn pdf_export_passthrough_keeps_authored_cmyk() {
        let mut doc = DocumentGraph::new();
        let page = doc.insert_node(Node::new(NodeType::Page, "Page")).unwrap();
        let rect = VectorPath::new(vec![
            PathSegment::MoveTo(PathPoint::new(0.0, 0.0)),
            PathSegment::LineTo(PathPoint::new(100.0, 0.0)),
            PathSegment::LineTo(PathPoint::new(100.0, 100.0)),
            PathSegment::LineTo(PathPoint::new(0.0, 100.0)),
            PathSegment::Close,
        ]);
        let mut node = vector_node(&rect, 0.0, 0.0, 100.0, 100.0);
        node.parent_id = Some(page);
        // Authored as native CMYK: cyan ink only.
        node.style.color_override = Some(Color::Cmyk {
            c: 1.0,
            m: 0.0,
            y: 0.0,
            k: 0.0,
            a: 1.0,
        });
        doc.insert_node(node).unwrap();

        let tmp = tempfile::NamedTempFile::new().unwrap();
        let opts = PdfExportOptions {
            color_mode: PdfColorMode::PassThrough,
            ..PdfExportOptions::default()
        };
        let rasters = RasterPixelCache::new();
        export_pdf_from_document(&doc, &opts, &rasters, tmp.path()).unwrap();
        let written = std::fs::read(tmp.path()).unwrap();
        let stream = pdf_content_stream_text(&written);
        // PassThrough on CMYK-authored fill should still emit `k`.
        assert!(
            stream.contains(" k\n") || stream.contains(" k "),
            "expected DeviceCMYK in pass-through mode, got {stream:?}"
        );
        // Cyan-only ink => coefficients `1 0 0 0 k` (after printpdf
        // rounding). Look for the leading channel byte.
        assert!(
            stream.contains("1 0 0 0 k") || stream.contains("1.00 0.00 0.00 0.00 k"),
            "expected cyan-only CMYK coefficients, got {stream:?}"
        );
    }

    // -------------------------------------------------------------
    // `resolve_fill_color` — visibility precedence (regression for the
    // "override resurrects an invisible node" bug). The renderer's
    // `fill` is the single source of truth for whether a node is
    // painted; the override only changes *what color* gets emitted.
    // -------------------------------------------------------------

    fn vector_node_with_fill(fill: FillStyle) -> Node {
        let rect = VectorPath::new(vec![
            PathSegment::MoveTo(PathPoint::new(0.0, 0.0)),
            PathSegment::LineTo(PathPoint::new(10.0, 0.0)),
            PathSegment::LineTo(PathPoint::new(10.0, 10.0)),
            PathSegment::LineTo(PathPoint::new(0.0, 10.0)),
            PathSegment::Close,
        ]);
        let mut node = vector_node(&rect, 0.0, 0.0, 10.0, 10.0);
        node.style.fill = fill;
        node
    }

    #[test]
    fn resolve_fill_returns_none_for_fillstyle_none_even_with_override() {
        let mut node = vector_node_with_fill(FillStyle::None);
        node.style.color_override = Some(Color::Cmyk {
            c: 1.0,
            m: 0.0,
            y: 0.0,
            k: 0.0,
            a: 1.0,
        });
        assert!(
            resolve_fill_color(&node).is_none(),
            "`color_override` must not resurrect a `FillStyle::None` node"
        );
    }

    #[test]
    fn resolve_fill_returns_none_for_zero_alpha_fill_even_with_override() {
        let mut node = vector_node_with_fill(FillStyle::Solid(kcreate_core::node::RgbaColor {
            r: 1.0,
            g: 0.0,
            b: 0.0,
            a: 0.0,
        }));
        node.style.color_override = Some(Color::Cmyk {
            c: 1.0,
            m: 0.0,
            y: 0.0,
            k: 0.0,
            a: 1.0,
        });
        assert!(
            resolve_fill_color(&node).is_none(),
            "`color_override` must not resurrect a zero-alpha fill"
        );
    }

    #[test]
    fn resolve_fill_substitutes_override_when_fill_is_visible() {
        let mut node = vector_node_with_fill(FillStyle::Solid(kcreate_core::node::RgbaColor {
            r: 1.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        }));
        node.style.color_override = Some(Color::Cmyk {
            c: 1.0,
            m: 0.0,
            y: 0.0,
            k: 0.0,
            a: 1.0,
        });
        let resolved = resolve_fill_color(&node).expect("visible fill resolves");
        match resolved {
            Color::Cmyk { c, m, y, k, .. } => {
                assert!((c - 1.0).abs() < 1e-6);
                assert!(m.abs() < 1e-6);
                assert!(y.abs() < 1e-6);
                assert!(k.abs() < 1e-6);
            }
            other => panic!("expected CMYK override, got {other:?}"),
        }
    }

    #[test]
    fn resolve_fill_keeps_fill_alpha_when_override_is_opaque() {
        // If the renderer made the node 50% transparent and the user
        // authored a CMYK override that defaulted to alpha=1.0, the
        // exporter must honor the renderer's alpha — otherwise the
        // PDF would render fully opaque while the canvas showed
        // semi-transparent.
        let mut node = vector_node_with_fill(FillStyle::Solid(kcreate_core::node::RgbaColor {
            r: 1.0,
            g: 0.0,
            b: 0.0,
            a: 0.5,
        }));
        node.style.color_override = Some(Color::Cmyk {
            c: 1.0,
            m: 0.0,
            y: 0.0,
            k: 0.0,
            a: 1.0,
        });
        let resolved = resolve_fill_color(&node).expect("visible fill resolves");
        assert!((resolved.alpha() - 0.5).abs() < 1e-6);
    }

    #[test]
    fn resolve_fill_keeps_override_alpha_when_authored() {
        // Conversely, if the override carries its own < 1.0 alpha the
        // author meant it — don't overwrite with the renderer side.
        let mut node = vector_node_with_fill(FillStyle::Solid(kcreate_core::node::RgbaColor {
            r: 1.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        }));
        node.style.color_override = Some(Color::Cmyk {
            c: 1.0,
            m: 0.0,
            y: 0.0,
            k: 0.0,
            a: 0.25,
        });
        let resolved = resolve_fill_color(&node).expect("visible fill resolves");
        assert!((resolved.alpha() - 0.25).abs() < 1e-6);
    }
}
