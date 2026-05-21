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
use kcreate_core::document::DocumentGraph;
use kcreate_core::node::{FillStyle, Node, NodeType};
use kcreate_vector::{PathPoint, PathSegment, VectorPath};
use printpdf::path::{PaintMode, WindingOrder};
use printpdf::{Image, ImageTransform, Mm, PdfDocument, PdfLayerReference, Point, Polygon, Rgb};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

/// Metadata key on a [`NodeType::VectorLayer`] node holding the
/// serialised [`VectorPath`]. Identical to the SVG exporter's key.
pub const VECTOR_PATH_METADATA_KEY: &str = "vector_path";

/// Metadata key on a [`NodeType::RasterLayer`] node holding the
/// `{ blob_hash, width, height }` payload — the scene-sync layer
/// writes this on import. Read-only here.
pub const RASTER_IMAGE_METADATA_KEY: &str = "raster_image";

/// PDF export options.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PdfExportOptions {
    pub width_mm: f64,
    pub height_mm: f64,
    pub title: String,
}

impl Default for PdfExportOptions {
    fn default() -> Self {
        Self {
            width_mm: 210.0,
            height_mm: 297.0,
            title: "KCreate document".to_string(),
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
                emit_vector(node, layer, origin_x, origin_y, sx, sy, page_height_mm)?;
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

fn emit_vector(
    node: &Node,
    layer: &PdfLayerReference,
    origin_x: f64,
    origin_y: f64,
    sx: f64,
    sy: f64,
    page_height_mm: f64,
) -> Result<(), PdfExportError> {
    let Some(value) = node.metadata.get(VECTOR_PATH_METADATA_KEY) else {
        return Ok(());
    };
    let path: VectorPath = serde_json::from_value(value.clone())
        .map_err(|e| PdfExportError::InvalidVectorPath(node.id, e.to_string()))?;

    let (r, g, b, has_fill) = match node.style.fill {
        FillStyle::Solid(rgba) => (rgba.r, rgba.g, rgba.b, rgba.a > 0.0),
        FillStyle::None | FillStyle::Gradient(_) => (0.0, 0.0, 0.0, false),
    };

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

    layer.set_fill_color(printpdf::Color::Rgb(Rgb::new(r, g, b, None)));
    layer.set_outline_color(printpdf::Color::Rgb(Rgb::new(0.0, 0.0, 0.0, None)));
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
}
