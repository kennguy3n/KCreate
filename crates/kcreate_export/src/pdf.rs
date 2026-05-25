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
use kcreate_core::node::{FillStyle, GradientKind, Node, NodeType, Point2D};
use kcreate_vector::{PathPoint, PathSegment, VectorPath};
use printpdf::lopdf::content::Operation as PdfOp;
use printpdf::lopdf::Object as PdfObj;
use printpdf::path::{PaintMode, WindingOrder};
use printpdf::{
    Cmyk, ColorBits, ColorSpace, Image, ImageTransform, ImageXObject, Mm, PdfDocument,
    PdfLayerReference, Point, Polygon, Px, Rgb,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::cmyk_dither::{quantize_cmyk_image, CmykDither};
use crate::pdf_shading::{
    color_space_for_mode, inject_shadings, resolve_stop_color, GradientGeometry, PdfShadingError,
    PendingShading, ShadingColorSpace,
};
pub use crate::scene_metadata::{RASTER_IMAGE_METADATA_KEY, VECTOR_PATH_METADATA_KEY};

/// One PDF user-space point = 1/72 inch; 1 mm = 1/25.4 inch.
const PT_PER_MM: f64 = 72.0 / 25.4;

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
    /// Which dithering algorithm to apply when rasterising layers
    /// down to 8-bit `/DeviceCMYK`. Only meaningful when
    /// `color_mode == PdfColorMode::Cmyk`; ignored for `Rgb` and
    /// `PassThrough`. Defaults to Floyd-Steinberg, matching what
    /// every print shop expects for hero artwork. Callers running
    /// thumbnail batches that want predictable parallelisable
    /// output can opt into `Bayer8x8`; callers reproducing the
    /// Phase 2 byte-identical output can set `None`.
    pub cmyk_dither: CmykDither,
}

impl Default for PdfExportOptions {
    fn default() -> Self {
        Self {
            width_mm: 210.0,
            height_mm: 297.0,
            title: "KCreate document".to_string(),
            color_mode: PdfColorMode::Rgb,
            cmyk_dither: CmykDither::FloydSteinberg,
        }
    }
}

/// Resolved fill paint for one node. Solid fills are emitted via
/// the original [`PdfLayerReference`] path; gradient fills are
/// emitted as raw content-stream operators plus a deferred
/// [`PendingShading`] that the post-processor materialises into a
/// real PDF Shading dictionary.
#[derive(Debug, Clone)]
pub(crate) enum PdfPaint {
    /// Nothing to paint — either the node has no fill, the fill
    /// is fully transparent, or the gradient stops collapse to
    /// nothing visible.
    None,
    /// Flat colour fill. Maps to the existing `set_fill_color` /
    /// `add_polygon` path.
    Solid(Color),
    /// Gradient fill, ready to be emitted as a real PDF shading
    /// pattern via post-processing.
    Gradient(GradientPaint),
}

#[derive(Debug, Clone)]
pub(crate) struct GradientPaint {
    pub kind: PaintGradientKind,
    /// Renderer-side stops, in their original colour space. The
    /// emitter converts these into the [`ShadingColorSpace`]
    /// before pushing into the [`PendingShading`].
    pub stops: Vec<(f32, Color)>,
    /// Colour space to record on the resulting shading dict.
    pub color_space: ShadingColorSpace,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum PaintGradientKind {
    /// Linear: straight line in node-local coordinates.
    Linear { from: Point2D, to: Point2D },
    /// Radial: concentric circles in node-local coordinates;
    /// inner circle is collapsed to a point at `center`.
    Radial { center: Point2D, radius: f64 },
}

/// Resolve the authoritative fill paint for a node. Combines the
/// renderer-side `fill` and the export-time `color_override` in a
/// way that respects the visibility contract documented on
/// [`resolve_fill_color`]:
///
/// * `Solid(rgba)` with `rgba.a == 0.0` is invisible regardless of
///   any override.
/// * `Gradient(_)` with no opaque stops AND no override is
///   invisible — matches the renderer skipping the draw call.
/// * `Gradient(_)` with at least one opaque stop is now a real
///   PDF shading pattern in the requested colour space, instead
///   of the Phase 2 fallback that flattened the gradient to a
///   solid in the override's colour space.
pub(crate) fn resolve_fill_paint(node: &Node, color_mode: PdfColorMode) -> PdfPaint {
    let color_space = color_space_for_mode(color_mode);
    match (&node.style.fill, &node.style.color_override) {
        (FillStyle::None, _) => PdfPaint::None,
        (FillStyle::Solid(rgba), _) if rgba.a <= 0.0 => PdfPaint::None,
        (FillStyle::Solid(rgba), _) => {
            // Override applies to solid fills exactly as before —
            // see `resolve_fill_color` for the alpha-stitching
            // rationale. We delegate to keep both paths in sync.
            if let Some(over) = &node.style.color_override {
                PdfPaint::Solid(merge_override_alpha(over.clone(), rgba.a))
            } else {
                PdfPaint::Solid(Color::Srgb {
                    r: rgba.r,
                    g: rgba.g,
                    b: rgba.b,
                    a: rgba.a,
                })
            }
        }
        (FillStyle::Gradient(kind), _) => {
            let effective_alpha = gradient_effective_alpha(kind);
            if effective_alpha <= 0.0 {
                return PdfPaint::None;
            }
            let (geo, stops_src) = match kind {
                GradientKind::Linear { from, to, stops } => (
                    PaintGradientKind::Linear {
                        from: *from,
                        to: *to,
                    },
                    stops,
                ),
                GradientKind::Radial {
                    center,
                    radius,
                    stops,
                } => (
                    PaintGradientKind::Radial {
                        center: *center,
                        radius: *radius,
                    },
                    stops,
                ),
            };
            if stops_src.is_empty() {
                return PdfPaint::None;
            }
            // If the user authored a `color_override`, treat it as
            // a single-stop override: every stop's chroma becomes
            // the override, with the source alpha preserved. This
            // is conservative — it keeps the user's "I want this
            // printed in CMYK" intent without losing the
            // gradient's geometry. We could in principle adopt a
            // more sophisticated mapping (e.g. shade between the
            // override and white) once the editor exposes a UI to
            // author multi-stop overrides; today the editor
            // surfaces a single colour at most.
            let stops = if let Some(over) = &node.style.color_override {
                stops_src
                    .iter()
                    .map(|s| {
                        (
                            s.offset as f32,
                            merge_override_alpha(over.clone(), s.color.a),
                        )
                    })
                    .collect::<Vec<_>>()
            } else {
                stops_src
                    .iter()
                    .map(|s| {
                        (
                            s.offset as f32,
                            Color::Srgb {
                                r: s.color.r,
                                g: s.color.g,
                                b: s.color.b,
                                a: s.color.a,
                            },
                        )
                    })
                    .collect::<Vec<_>>()
            };
            PdfPaint::Gradient(GradientPaint {
                kind: geo,
                stops,
                color_space,
            })
        }
    }
}

/// Resolve the authoritative *solid* fill color for a node. Thin
/// wrapper over [`resolve_fill_paint`] kept for tests that pre-date
/// Phase 4's shading-pattern support — it returns `Some(color)`
/// only when the resolved paint is a flat fill, and `None` for
/// gradients (which are now emitted as real PDF shading patterns
/// via the post-processor instead of flattened to a solid).
#[cfg(test)]
fn resolve_fill_color(node: &Node) -> Option<Color> {
    match resolve_fill_paint(node, PdfColorMode::Rgb) {
        PdfPaint::Solid(c) => Some(c),
        PdfPaint::Gradient(_) | PdfPaint::None => None,
    }
}

/// Walk the gradient's stops and return the largest alpha. We use
/// the max (not the mean) so a gradient with a single fully-opaque
/// stop and several zero-alpha ones is still treated as visible —
/// the renderer would paint the opaque region too. Returns `0.0`
/// for an empty stops vector (defensive — `GradientKind` doesn't
/// enforce non-empty in the type system).
fn gradient_effective_alpha(kind: &kcreate_core::node::GradientKind) -> f32 {
    use kcreate_core::node::GradientKind;
    let stops = match kind {
        GradientKind::Linear { stops, .. } | GradientKind::Radial { stops, .. } => stops,
    };
    stops.iter().map(|s| s.color.a).fold(0.0_f32, f32::max)
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
        Color::Spot {
            name,
            fallback_cmyk,
            tint,
            ..
        } => Color::Spot {
            name,
            fallback_cmyk,
            tint,
            alpha: final_alpha,
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
    // The CMYK passthrough arm forwards authored CMYK values verbatim
    // and does not need the sRGB conversion at all — calling
    // `to_srgb()` unconditionally would burn four float multiplies per
    // node on every CMYK-heavy export for no benefit. The helper below
    // computes the sRGB triplet lazily so only the arms that need it
    // pay for it.
    let to_srgb = || c.to_srgb();
    match (mode, c) {
        // Both `Cmyk` mode and `PassThrough` mode preserve authored
        // CMYK exactly so we can route them through the same arm.
        (PdfColorMode::Cmyk | PdfColorMode::PassThrough, Color::Cmyk { c, m, y, k, .. }) => {
            printpdf::Color::Cmyk(Cmyk::new(*c, *m, *y, *k, None))
        }
        (PdfColorMode::Cmyk, _) => {
            let (r, g, b, _a) = to_srgb();
            let (cc, mm, yy, kk) = srgb_to_cmyk(r, g, b);
            printpdf::Color::Cmyk(Cmyk::new(cc, mm, yy, kk, None))
        }
        (PdfColorMode::PassThrough | PdfColorMode::Rgb, _) => {
            let (r, g, b, _a) = to_srgb();
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
    #[error("shading post-process: {0}")]
    Shading(#[from] PdfShadingError),
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

    let mut pending_shadings: Vec<PendingShading> = Vec::new();
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
        options.cmyk_dither,
        &mut pending_shadings,
    )?;

    let bytes = doc
        .save_to_bytes()
        .map_err(|e| PdfExportError::PrintPdf(e.to_string()))?;
    let final_bytes = inject_shadings(bytes, &pending_shadings).map_err(PdfExportError::from)?;
    fs::write(output_path, &final_bytes)?;
    Ok(final_bytes.len())
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
    cmyk_dither: CmykDither,
    pending_shadings: &mut Vec<PendingShading>,
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
                    pending_shadings,
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
                    color_mode,
                    cmyk_dither,
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
            cmyk_dither,
            pending_shadings,
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
    pending_shadings: &mut Vec<PendingShading>,
) -> Result<(), PdfExportError> {
    let Some(value) = node.metadata.get(VECTOR_PATH_METADATA_KEY) else {
        return Ok(());
    };
    let path: VectorPath = serde_json::from_value(value.clone())
        .map_err(|e| PdfExportError::InvalidVectorPath(node.id, e.to_string()))?;

    let paint = resolve_fill_paint(node, color_mode);

    let rings = build_rings(
        &path,
        node.transform.tx,
        node.transform.ty,
        origin_x,
        origin_y,
        sx,
        sy,
        page_height_mm,
    );
    if rings.is_empty() {
        return Ok(());
    }

    match paint {
        PdfPaint::None => {
            emit_solid_or_stroke(layer, rings, None, color_mode);
        }
        PdfPaint::Solid(c) => {
            emit_solid_or_stroke(layer, rings, Some(c), color_mode);
        }
        PdfPaint::Gradient(g) => {
            emit_gradient(
                node,
                layer,
                &rings,
                &g,
                origin_x,
                origin_y,
                sx,
                sy,
                page_height_mm,
                pending_shadings,
            );
        }
    }
    Ok(())
}

/// Translate a [`VectorPath`] into printpdf's ring-of-points
/// representation. Extracted so the gradient emit path can reuse
/// the exact same coordinate-space arithmetic as the solid path.
#[allow(clippy::too_many_arguments)]
fn build_rings(
    path: &VectorPath,
    tx: f64,
    ty: f64,
    origin_x: f64,
    origin_y: f64,
    sx: f64,
    sy: f64,
    page_height_mm: f64,
) -> Vec<Vec<(Point, bool)>> {
    let mut rings: Vec<Vec<(Point, bool)>> = Vec::new();
    let mut current: Vec<(Point, bool)> = Vec::new();
    let mut last = PathPoint::new(0.0, 0.0);
    let mut start = PathPoint::new(0.0, 0.0);

    let push_point = |buf: &mut Vec<(Point, bool)>, p: PathPoint, is_ctrl: bool| {
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
                push_point(&mut current, p, false);
                last = p;
                start = p;
            }
            PathSegment::LineTo(p) => {
                push_point(&mut current, p, false);
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
                push_point(&mut current, c1, true);
                push_point(&mut current, c2, true);
                push_point(&mut current, end, false);
                last = end;
            }
            PathSegment::CubicTo { ctrl1, ctrl2, end } => {
                push_point(&mut current, ctrl1, true);
                push_point(&mut current, ctrl2, true);
                push_point(&mut current, end, false);
                last = end;
            }
            PathSegment::Close => {
                push_point(&mut current, start, false);
                rings.push(std::mem::take(&mut current));
                last = start;
            }
        }
    }
    if !current.is_empty() {
        rings.push(current);
    }
    rings
}

/// Emit a polygon via printpdf's high-level helper, choosing fill
/// vs stroke based on whether a fill colour was resolved.
fn emit_solid_or_stroke(
    layer: &PdfLayerReference,
    rings: Vec<Vec<(Point, bool)>>,
    fill_color: Option<Color>,
    color_mode: PdfColorMode,
) {
    let has_fill = fill_color.is_some();
    // Both fill and stroke colors must agree with `color_mode` so
    // the generated content stream never mixes `rg` (DeviceRGB)
    // and `K` (DeviceCMYK) operators. For stroke-only nodes,
    // `fill_color` is `None` but `set_fill_color` still emits an
    // operator, so the fallback color space must match the
    // requested mode too — otherwise PDF/X validators flag the
    // page as mixed-color-space.
    let fill_pdf_color = fill_color.as_ref().map_or_else(
        || match color_mode {
            PdfColorMode::Cmyk => printpdf::Color::Cmyk(Cmyk::new(0.0, 0.0, 0.0, 0.0, None)),
            PdfColorMode::Rgb | PdfColorMode::PassThrough => {
                printpdf::Color::Rgb(Rgb::new(0.0, 0.0, 0.0, None))
            }
        },
        |c| color_to_printpdf(c, color_mode),
    );
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
}

/// Emit a gradient fill via raw PDF operators (`q ... W n /SH<n>
/// sh Q`) and queue a [`PendingShading`] for post-processing. The
/// `/SH<n>` resource name is intentionally dangling at this stage
/// — `pdf_shading::inject_shadings` will register it on the page's
/// `Resources/Shading` dict after `save_to_bytes`.
#[allow(clippy::too_many_arguments)]
fn emit_gradient(
    node: &Node,
    layer: &PdfLayerReference,
    rings: &[Vec<(Point, bool)>],
    paint: &GradientPaint,
    origin_x: f64,
    origin_y: f64,
    sx: f64,
    sy: f64,
    page_height_mm: f64,
    pending_shadings: &mut Vec<PendingShading>,
) {
    // q: save graphics state.
    layer.add_operation(PdfOp::new("q", vec![]));
    // Emit the clipping path. Each ring becomes a sub-path in the
    // operator stream — `m` for moveto, `l` for lineto, `c` for
    // cubicto, `h` to close.
    emit_raw_path_operators(layer, rings);
    // W n: intersect the current clip path with the path we just
    // built (non-zero winding rule), then `n` (no-op) to avoid
    // also painting it.
    layer.add_operation(PdfOp::new("W", vec![]));
    layer.add_operation(PdfOp::new("n", vec![]));

    // Allocate a shading index and emit the `sh` operator pointing
    // at the dangling `/SH<n>` name. The post-processor will add
    // `/SH<n>` to the page's `Resources/Shading` dict.
    let index = pending_shadings.len();
    let name = format!("SH{index}");
    layer.add_operation(PdfOp::new("sh", vec![PdfObj::Name(name.into_bytes())]));
    // Q: restore graphics state (undoes the clipping).
    layer.add_operation(PdfOp::new("Q", vec![]));

    // Resolve the gradient stops into the target colour space and
    // queue the pending shading. Coordinates are transformed from
    // node-local → world → PDF user-space (points). printpdf's
    // page coordinates are in millimetres internally but the
    // content-stream `sh` operator and the `Coords` array on the
    // Shading dict speak the page's default user-space unit
    // (points) — so we convert here so the post-processor doesn't
    // need to know anything about millimetres.
    let resolved_stops = paint
        .stops
        .iter()
        .map(|(offset, color)| {
            let mut stop = resolve_stop_color(color, paint.color_space);
            stop.offset = offset.clamp(0.0, 1.0);
            stop
        })
        .collect::<Vec<_>>();

    let geometry = match paint.kind {
        PaintGradientKind::Linear { from, to } => {
            let (x0_pt, y0_pt) = node_local_to_pt(
                from,
                node.transform.tx,
                node.transform.ty,
                origin_x,
                origin_y,
                sx,
                sy,
                page_height_mm,
            );
            let (x1_pt, y1_pt) = node_local_to_pt(
                to,
                node.transform.tx,
                node.transform.ty,
                origin_x,
                origin_y,
                sx,
                sy,
                page_height_mm,
            );
            GradientGeometry::Linear {
                x0: x0_pt,
                y0: y0_pt,
                x1: x1_pt,
                y1: y1_pt,
            }
        }
        PaintGradientKind::Radial { center, radius } => {
            let (cx_pt, cy_pt) = node_local_to_pt(
                center,
                node.transform.tx,
                node.transform.ty,
                origin_x,
                origin_y,
                sx,
                sy,
                page_height_mm,
            );
            // Radii are scalars; use the average of sx/sy so the
            // radius scales proportionally with the page-space
            // transform. Coordinate sx and sy are in mm/world-unit;
            // we convert through points-per-mm.
            let r_pt = (radius * ((sx + sy) * 0.5) * PT_PER_MM) as f32;
            GradientGeometry::Radial {
                cx0: cx_pt,
                cy0: cy_pt,
                r0: 0.0,
                cx1: cx_pt,
                cy1: cy_pt,
                r1: r_pt.max(0.0),
            }
        }
    };

    pending_shadings.push(PendingShading {
        page_index: 0,
        index,
        geometry,
        stops: resolved_stops,
        color_space: paint.color_space,
    });
}

/// Emit the raw PDF path operators for `rings`. Each control-point
/// `(Point, true)` is treated as a Bezier control point — three
/// consecutive points (two control + one anchor) become a `c`
/// operator. Anchor-to-anchor segments become `l`. The first
/// point of a ring is `m`; the last (if equal to the first)
/// becomes `h`.
fn emit_raw_path_operators(layer: &PdfLayerReference, rings: &[Vec<(Point, bool)>]) {
    for ring in rings {
        if ring.is_empty() {
            continue;
        }
        let (first, _) = ring[0];
        let (fx, fy) = point_to_pt(first);
        layer.add_operation(PdfOp::new("m", vec![PdfObj::Real(fx), PdfObj::Real(fy)]));
        let mut i = 1;
        while i < ring.len() {
            // Cubic: anchor → ctrl, ctrl, anchor. If the next two
            // points are control points, consume them along with
            // the following anchor.
            if i + 2 < ring.len() && ring[i].1 && ring[i + 1].1 && !ring[i + 2].1 {
                let (c1x, c1y) = point_to_pt(ring[i].0);
                let (c2x, c2y) = point_to_pt(ring[i + 1].0);
                let (px, py) = point_to_pt(ring[i + 2].0);
                layer.add_operation(PdfOp::new(
                    "c",
                    vec![
                        PdfObj::Real(c1x),
                        PdfObj::Real(c1y),
                        PdfObj::Real(c2x),
                        PdfObj::Real(c2y),
                        PdfObj::Real(px),
                        PdfObj::Real(py),
                    ],
                ));
                i += 3;
            } else {
                let (px, py) = point_to_pt(ring[i].0);
                layer.add_operation(PdfOp::new("l", vec![PdfObj::Real(px), PdfObj::Real(py)]));
                i += 1;
            }
        }
        // `h` closes the sub-path. Always emit it so the clip path
        // is well-formed (PDF requires a closed sub-path to be
        // included in a fill region).
        layer.add_operation(PdfOp::new("h", vec![]));
    }
}

/// Convert a printpdf `Point` (carrying millimetres) into raw PDF
/// user-space points (1/72").
fn point_to_pt(p: Point) -> (f32, f32) {
    let x_mm: f32 = p.x.0;
    let y_mm: f32 = p.y.0;
    (
        (f64::from(x_mm) * PT_PER_MM) as f32,
        (f64::from(y_mm) * PT_PER_MM) as f32,
    )
}

/// Transform a node-local [`Point2D`] into PDF user-space points,
/// applying the same world → PDF transform as the path
/// emitter. Returns `(x_pt, y_pt)`.
#[allow(clippy::too_many_arguments)]
fn node_local_to_pt(
    p: Point2D,
    tx: f64,
    ty: f64,
    origin_x: f64,
    origin_y: f64,
    sx: f64,
    sy: f64,
    page_height_mm: f64,
) -> (f32, f32) {
    let pt = world_to_pdf(
        p.x + tx,
        p.y + ty,
        origin_x,
        origin_y,
        sx,
        sy,
        page_height_mm,
    );
    point_to_pt(pt)
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
    color_mode: PdfColorMode,
    cmyk_dither: CmykDither,
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

    // Build a printpdf `Image` (= `ImageXObject` wrapper) in the
    // device color space requested by the caller. CMYK mode runs
    // the pixel buffer through `srgb_to_cmyk` and embeds the
    // result as a `ColorSpace::Cmyk` image so a print shop sees
    // `/DeviceCMYK` in the resulting PDF; the other modes keep
    // the historical "PNG → load_from_memory → DynamicImage"
    // round-trip which yields a `/DeviceRGB` image.
    let pdf_image = match color_mode {
        PdfColorMode::Cmyk => {
            raster_to_cmyk_image(&pixels.rgba, pixels.width, pixels.height, cmyk_dither)?
        }
        PdfColorMode::Rgb | PdfColorMode::PassThrough => {
            raster_to_rgb_image(&pixels.rgba, pixels.width, pixels.height)?
        }
    };

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

/// Build a printpdf `Image` from an RGBA pixel buffer, embedded as
/// `/DeviceRGB`. The pixel buffer is re-encoded to PNG so
/// printpdf can decode it through its `image_crate` re-export.
fn raster_to_rgb_image(rgba: &[u8], width: u32, height: u32) -> Result<Image, PdfExportError> {
    let mut png_bytes: Vec<u8> = Vec::new();
    {
        let mut cursor = std::io::Cursor::new(&mut png_bytes);
        image::write_buffer_with_format(
            &mut cursor,
            rgba,
            width,
            height,
            image::ColorType::Rgba8,
            ImageFormat::Png,
        )
        .map_err(|e| PdfExportError::Image(e.to_string()))?;
    }
    let dyn_img = printpdf::image_crate::load_from_memory(&png_bytes)
        .map_err(|e| PdfExportError::Image(e.to_string()))?;
    Ok(Image::from_dynamic_image(&dyn_img))
}

/// Build a printpdf `Image` from an RGBA pixel buffer, embedded as
/// `/DeviceCMYK`. Each input pixel is converted from sRGB to CMYK
/// via [`kcreate_core::color::srgb_to_cmyk`]; transparency is
/// matted against a white paper background ("straight alpha over
/// white") because PDF's `/DeviceCMYK` color space has no native
/// alpha channel and a soft mask would still be interpreted as a
/// transparency group against whatever is below the image on the
/// page (typically white paper anyway).
///
/// This is a lossy conversion — the alpha channel is permanently
/// folded into the CMYK values. Callers who need to preserve
/// transparency through to the print shop should keep the raster
/// in RGB mode and let the RIP handle the CMYK conversion with a
/// device-specific profile.
fn raster_to_cmyk_image(
    rgba: &[u8],
    width: u32,
    height: u32,
    dither: CmykDither,
) -> Result<Image, PdfExportError> {
    let pixel_count = (width as usize)
        .checked_mul(height as usize)
        .ok_or_else(|| PdfExportError::Image("raster dimensions overflow usize".into()))?;
    if rgba.len() < pixel_count * 4 {
        return Err(PdfExportError::Image(format!(
            "raster buffer is {} bytes; expected at least {} (4 * w * h)",
            rgba.len(),
            pixel_count * 4
        )));
    }
    let row_stride = (width as usize) * 4;
    let mut cmyk_bytes: Vec<u8> = Vec::with_capacity(pixel_count * 4);
    quantize_cmyk_image(width, height, dither, &mut cmyk_bytes, |x, y| {
        let idx = (y as usize) * row_stride + (x as usize) * 4;
        let r = f32::from(rgba[idx]) / 255.0;
        let g = f32::from(rgba[idx + 1]) / 255.0;
        let b = f32::from(rgba[idx + 2]) / 255.0;
        let a = f32::from(rgba[idx + 3]) / 255.0;
        // Matte against white paper: out = a * fg + (1 - a) * white.
        // White in linear sRGB is (1, 1, 1); blending in the gamma-
        // encoded sRGB space is technically incorrect, but it
        // matches what every printer driver on the planet does and
        // avoids a full sRGB→linear→matte→sRGB round-trip per pixel.
        let r_m = a.mul_add(r, 1.0 - a);
        let g_m = a.mul_add(g, 1.0 - a);
        let b_m = a.mul_add(b, 1.0 - a);
        // srgb_to_cmyk returns a 4-tuple; the dither callback
        // wants `[f32; 4]`. `<[f32; 4]>::from(tuple)` does the
        // conversion without tripping clippy's
        // `tuple_array_conversions` heuristic.
        <[f32; 4]>::from(srgb_to_cmyk(r_m, g_m, b_m))
    });
    let xobject = ImageXObject {
        width: Px(width as usize),
        height: Px(height as usize),
        color_space: ColorSpace::Cmyk,
        bits_per_component: ColorBits::Bit8,
        interpolate: true,
        image_data: cmyk_bytes,
        image_filter: None,
        smask: None,
        clipping_bbox: None,
    };
    Ok(Image::from(xobject))
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
    // Phase 4 Block 4 — end-to-end shading-pattern emission. These
    // tests round-trip a gradient-bearing document through the
    // exporter and assert that the resulting PDF contains real
    // PDF /Shading dictionaries, a `sh` operator in the content
    // stream, and the matching `/Shading` resource entry on the
    // page. Both linear and radial gradients are covered.
    // -------------------------------------------------------------

    fn doc_with_linear_gradient_rect() -> DocumentGraph {
        use kcreate_core::node::{GradientKind, GradientStop, Point2D, RgbaColor};
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
        node.style.fill = FillStyle::Gradient(GradientKind::Linear {
            from: Point2D { x: 0.0, y: 0.0 },
            to: Point2D { x: 100.0, y: 0.0 },
            stops: vec![
                GradientStop {
                    offset: 0.0,
                    color: RgbaColor::new(1.0, 0.0, 0.0, 1.0),
                },
                GradientStop {
                    offset: 1.0,
                    color: RgbaColor::new(0.0, 0.0, 1.0, 1.0),
                },
            ],
        });
        doc.insert_node(node).unwrap();
        doc
    }

    #[test]
    fn pdf_export_emits_real_axial_shading_for_linear_gradient() {
        let doc = doc_with_linear_gradient_rect();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let opts = PdfExportOptions::default();
        let rasters = RasterPixelCache::new();
        export_pdf_from_document(&doc, &opts, &rasters, tmp.path()).unwrap();
        let written = std::fs::read(tmp.path()).unwrap();
        // Look for the shading dict, the page-resource entry, and
        // the `sh` operator in the content stream — all three must
        // be present for a downstream PDF consumer to render the
        // gradient.
        let raw = String::from_utf8_lossy(&written);
        assert!(
            raw.contains("/ShadingType 2"),
            "expected axial Type-2 shading dict in PDF"
        );
        assert!(
            raw.contains("/SH0"),
            "expected page resource entry /SH0 in PDF"
        );
        let stream = pdf_content_stream_text(&written);
        assert!(
            stream.contains("/SH0 sh") || stream.contains("/SH0\nsh"),
            "expected `sh` operator referencing /SH0, got {stream:?}"
        );
    }

    #[test]
    fn pdf_export_emits_real_radial_shading_for_radial_gradient() {
        use kcreate_core::node::{GradientKind, GradientStop, Point2D, RgbaColor};
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
        node.style.fill = FillStyle::Gradient(GradientKind::Radial {
            center: Point2D { x: 50.0, y: 50.0 },
            radius: 50.0,
            stops: vec![
                GradientStop {
                    offset: 0.0,
                    color: RgbaColor::new(1.0, 1.0, 1.0, 1.0),
                },
                GradientStop {
                    offset: 1.0,
                    color: RgbaColor::new(0.0, 0.0, 0.0, 1.0),
                },
            ],
        });
        doc.insert_node(node).unwrap();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let opts = PdfExportOptions::default();
        let rasters = RasterPixelCache::new();
        export_pdf_from_document(&doc, &opts, &rasters, tmp.path()).unwrap();
        let written = std::fs::read(tmp.path()).unwrap();
        let raw = String::from_utf8_lossy(&written);
        assert!(
            raw.contains("/ShadingType 3"),
            "expected radial Type-3 shading dict in PDF"
        );
        // Type-3 Coords arrays carry six numbers (x0 y0 r0 x1 y1 r1).
        // The inner radius is collapsed to 0, so the array must
        // start with `0` for r0 once we round the float to integer
        // form in printpdf's canonical writer. We just check the
        // dict shape is present in the file.
        assert!(
            raw.contains("/Coords"),
            "expected /Coords entry in radial shading dict"
        );
    }

    #[test]
    fn pdf_export_routes_gradient_to_devicecmyk_when_color_mode_is_cmyk() {
        let doc = doc_with_linear_gradient_rect();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let opts = PdfExportOptions {
            color_mode: PdfColorMode::Cmyk,
            ..PdfExportOptions::default()
        };
        let rasters = RasterPixelCache::new();
        export_pdf_from_document(&doc, &opts, &rasters, tmp.path()).unwrap();
        let written = std::fs::read(tmp.path()).unwrap();
        let raw = String::from_utf8_lossy(&written);
        // lopdf serialises name tokens adjacently, e.g.
        // `/ColorSpace/DeviceCMYK`, so we look for the concatenated
        // form rather than a space-separated one.
        assert!(
            raw.contains("/ColorSpace/DeviceCMYK"),
            "expected CMYK shading colour space in PDF (CMYK export mode)"
        );
    }

    #[test]
    fn pdf_export_multi_stop_gradient_uses_stitching_function() {
        use kcreate_core::node::{GradientKind, GradientStop, Point2D, RgbaColor};
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
        node.style.fill = FillStyle::Gradient(GradientKind::Linear {
            from: Point2D { x: 0.0, y: 0.0 },
            to: Point2D { x: 100.0, y: 0.0 },
            stops: vec![
                GradientStop {
                    offset: 0.0,
                    color: RgbaColor::new(1.0, 0.0, 0.0, 1.0),
                },
                GradientStop {
                    offset: 0.5,
                    color: RgbaColor::new(0.0, 1.0, 0.0, 1.0),
                },
                GradientStop {
                    offset: 1.0,
                    color: RgbaColor::new(0.0, 0.0, 1.0, 1.0),
                },
            ],
        });
        doc.insert_node(node).unwrap();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let opts = PdfExportOptions::default();
        let rasters = RasterPixelCache::new();
        export_pdf_from_document(&doc, &opts, &rasters, tmp.path()).unwrap();
        let written = std::fs::read(tmp.path()).unwrap();
        let raw = String::from_utf8_lossy(&written);
        // Three stops means two Type-2 sub-functions stitched
        // together by a Type-3 wrapper. Both function types must
        // appear in the file.
        assert!(
            raw.contains("/FunctionType 2"),
            "expected Type-2 exponential sub-function for adjacent-stop interpolation"
        );
        assert!(
            raw.contains("/FunctionType 3"),
            "expected Type-3 stitching function for >2 stops"
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

    #[test]
    fn resolve_fill_paint_promotes_gradient_to_real_shading() {
        // Phase 4 Block 4: gradients are no longer dropped or
        // flattened — they resolve to `PdfPaint::Gradient(...)`
        // and the post-processor emits a real PDF shading dict.
        use kcreate_core::node::{GradientKind, GradientStop, Point2D, RgbaColor};
        let node = vector_node_with_fill(FillStyle::Gradient(GradientKind::Linear {
            from: Point2D { x: 0.0, y: 0.0 },
            to: Point2D { x: 1.0, y: 0.0 },
            stops: vec![
                GradientStop {
                    offset: 0.0,
                    color: RgbaColor::new(1.0, 0.0, 0.0, 1.0),
                },
                GradientStop {
                    offset: 1.0,
                    color: RgbaColor::new(0.0, 0.0, 1.0, 1.0),
                },
            ],
        }));
        match resolve_fill_paint(&node, PdfColorMode::Rgb) {
            PdfPaint::Gradient(g) => {
                assert_eq!(g.stops.len(), 2);
                assert!(matches!(g.color_space, ShadingColorSpace::DeviceRgb));
            }
            other => panic!("expected PdfPaint::Gradient, got {other:?}"),
        }
        // And the legacy solid-color resolver returns None so the
        // exporter's solid-fill branch is skipped.
        assert!(resolve_fill_color(&node).is_none());
    }

    #[test]
    fn resolve_fill_paint_overrides_gradient_stop_chroma_with_override() {
        // A gradient with a `color_override` keeps its geometry but
        // every stop's chroma becomes the override's colour. Tests
        // that the new shading-pattern path honours the user's
        // print-colour intent without losing the gradient.
        use kcreate_core::node::{GradientKind, GradientStop, Point2D, RgbaColor};
        let mut node = vector_node_with_fill(FillStyle::Gradient(GradientKind::Linear {
            from: Point2D { x: 0.0, y: 0.0 },
            to: Point2D { x: 1.0, y: 0.0 },
            stops: vec![
                GradientStop {
                    offset: 0.0,
                    color: RgbaColor::new(1.0, 0.0, 0.0, 1.0),
                },
                GradientStop {
                    offset: 1.0,
                    color: RgbaColor::new(0.0, 0.0, 1.0, 0.5),
                },
            ],
        }));
        node.style.color_override = Some(Color::Cmyk {
            c: 0.5,
            m: 0.25,
            y: 0.1,
            k: 0.05,
            a: 1.0,
        });
        match resolve_fill_paint(&node, PdfColorMode::Cmyk) {
            PdfPaint::Gradient(g) => {
                assert_eq!(g.stops.len(), 2);
                for (_, c) in &g.stops {
                    match c {
                        Color::Cmyk { c, m, y, k, .. } => {
                            assert!((c - 0.5).abs() < 1e-6);
                            assert!((m - 0.25).abs() < 1e-6);
                            assert!((y - 0.1).abs() < 1e-6);
                            assert!((k - 0.05).abs() < 1e-6);
                        }
                        other => panic!("expected Cmyk stop, got {other:?}"),
                    }
                }
                // The renderer-side alpha 0.5 must survive on the
                // second stop because the override defaulted to
                // fully opaque.
                assert!((g.stops[1].1.alpha() - 0.5).abs() < 1e-6);
                assert!(matches!(g.color_space, ShadingColorSpace::DeviceCmyk));
            }
            other => panic!("expected PdfPaint::Gradient, got {other:?}"),
        }
    }

    #[test]
    fn resolve_fill_paint_drops_gradient_with_zero_alpha_stops_even_with_override() {
        // A gradient whose stops are all fully transparent is
        // invisible on the canvas; the override does not change
        // that.
        use kcreate_core::node::{GradientKind, GradientStop, Point2D, RgbaColor};
        let mut node = vector_node_with_fill(FillStyle::Gradient(GradientKind::Linear {
            from: Point2D { x: 0.0, y: 0.0 },
            to: Point2D { x: 1.0, y: 0.0 },
            stops: vec![
                GradientStop {
                    offset: 0.0,
                    color: RgbaColor::new(1.0, 0.0, 0.0, 0.0),
                },
                GradientStop {
                    offset: 1.0,
                    color: RgbaColor::new(0.0, 0.0, 1.0, 0.0),
                },
            ],
        }));
        node.style.color_override = Some(Color::Cmyk {
            c: 0.5,
            m: 0.25,
            y: 0.1,
            k: 0.05,
            a: 1.0,
        });
        assert!(
            matches!(resolve_fill_paint(&node, PdfColorMode::Rgb), PdfPaint::None),
            "zero-alpha gradient must not become visible via override"
        );
    }

    /// 2×2 red-square RGBA buffer for the raster-mode tests.
    fn red_2x2_rgba() -> Vec<u8> {
        let mut buf = Vec::with_capacity(2 * 2 * 4);
        for _ in 0..(2 * 2) {
            buf.extend_from_slice(&[255, 0, 0, 255]);
        }
        buf
    }

    fn doc_with_raster_blob() -> (DocumentGraph, RasterPixelCache) {
        let mut doc = DocumentGraph::new();
        let page = doc.insert_node(Node::new(NodeType::Page, "Page")).unwrap();
        let mut node = Node::new(NodeType::RasterLayer, "raster");
        node.parent_id = Some(page);
        node.bounds = Bounds {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 100.0,
        };
        node.metadata.insert(
            RASTER_IMAGE_METADATA_KEY.to_string(),
            serde_json::json!({
                "blob_hash": "deadbeef",
                "width": 2,
                "height": 2,
            }),
        );
        doc.insert_node(node).unwrap();
        let mut cache = RasterPixelCache::new();
        cache.insert(
            "deadbeef".to_string(),
            RasterPixels {
                width: 2,
                height: 2,
                rgba: red_2x2_rgba(),
            },
        );
        (doc, cache)
    }

    #[test]
    fn pdf_export_rgb_mode_embeds_raster_as_devicergb() {
        let (doc, rasters) = doc_with_raster_blob();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let opts = PdfExportOptions::default();
        export_pdf_from_document(&doc, &opts, &rasters, tmp.path()).unwrap();
        let written = std::fs::read(tmp.path()).unwrap();
        // The image XObject's color space is part of the structural
        // dictionary, not the content stream, so we scrub the raw
        // PDF bytes for the `/DeviceRGB` token.
        let blob = String::from_utf8_lossy(&written);
        assert!(
            blob.contains("/DeviceRGB"),
            "RGB mode must embed raster as /DeviceRGB"
        );
        assert!(
            !blob.contains("/DeviceCMYK"),
            "RGB mode must NOT embed raster as /DeviceCMYK; got {blob:?}"
        );
    }

    #[test]
    fn pdf_export_cmyk_mode_embeds_raster_as_devicecmyk() {
        let (doc, rasters) = doc_with_raster_blob();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let opts = PdfExportOptions {
            color_mode: PdfColorMode::Cmyk,
            ..PdfExportOptions::default()
        };
        export_pdf_from_document(&doc, &opts, &rasters, tmp.path()).unwrap();
        let written = std::fs::read(tmp.path()).unwrap();
        let blob = String::from_utf8_lossy(&written);
        assert!(
            blob.contains("/DeviceCMYK"),
            "CMYK mode must embed raster as /DeviceCMYK"
        );
    }

    #[test]
    fn raster_to_cmyk_image_red_converts_to_full_magenta_yellow() {
        // sRGB red (255, 0, 0, 255) → CMYK (0, 1, 1, 0). Solid
        // input has zero quantisation error so Floyd-Steinberg
        // (the default) round-trips exactly to [0, 255, 255, 0].
        let buf = red_2x2_rgba();
        let img = raster_to_cmyk_image(&buf, 2, 2, CmykDither::FloydSteinberg)
            .expect("conversion succeeds");
        assert!(
            matches!(img.image.color_space, ColorSpace::Cmyk),
            "expected ColorSpace::Cmyk, got {:?}",
            img.image.color_space
        );
        assert_eq!(img.image.image_data.len(), 2 * 2 * 4);
        for chunk in img.image.image_data.chunks_exact(4) {
            assert_eq!(chunk[0], 0, "C channel for red should be 0");
            assert_eq!(chunk[1], 255, "M channel for red should be 255");
            assert_eq!(chunk[2], 255, "Y channel for red should be 255");
            assert_eq!(chunk[3], 0, "K channel for red should be 0");
        }
    }

    #[test]
    fn raster_to_cmyk_image_transparent_pixel_mattes_to_white() {
        // A fully transparent pixel mattes against white paper, which
        // is zero ink in CMYK. We disable dithering for this test so
        // we get exact zeros (Bayer/Floyd would noise around 0 with
        // sub-LSB perturbation).
        let buf = vec![255, 0, 0, 0, 0, 0, 0, 0];
        let img = raster_to_cmyk_image(&buf, 2, 1, CmykDither::None).expect("conversion succeeds");
        let pixels: Vec<&[u8]> = img.image.image_data.chunks_exact(4).collect();
        for px in &pixels {
            assert_eq!(px[..], [0, 0, 0, 0], "transparent → no ink");
        }
    }

    #[test]
    fn raster_to_cmyk_image_rejects_buffer_too_short() {
        // 2×2 should be 16 bytes; pass 12 to ensure the bounds check
        // catches the underflow before the unchecked slice indexing.
        let buf = vec![0u8; 12];
        let err = raster_to_cmyk_image(&buf, 2, 2, CmykDither::None)
            .expect_err("undersized buffer should be rejected");
        match err {
            PdfExportError::Image(msg) => assert!(
                msg.contains("expected at least"),
                "unexpected message: {msg}"
            ),
            other => panic!("expected Image error, got {other:?}"),
        }
    }

    #[test]
    fn raster_to_cmyk_image_dither_choice_affects_gradient_output() {
        // Build a 16×16 horizontal red→white gradient in sRGB,
        // export with Floyd-Steinberg vs. Bayer vs. none. The
        // three resulting byte streams must differ — otherwise
        // the dither selection isn't actually being applied.
        let w: u32 = 16;
        let h: u32 = 16;
        let mut buf = Vec::with_capacity((w * h * 4) as usize);
        for _y in 0..h {
            for x in 0..w {
                let t = (x as f32) / ((w - 1) as f32);
                buf.push((255.0 * (1.0 - t * 0.5)) as u8);
                buf.push((255.0 * 0.5 * (1.0 - t)) as u8);
                buf.push((255.0 * 0.5 * (1.0 - t)) as u8);
                buf.push(255);
            }
        }
        let none = raster_to_cmyk_image(&buf, w, h, CmykDither::None).expect("none succeeds");
        let fs = raster_to_cmyk_image(&buf, w, h, CmykDither::FloydSteinberg)
            .expect("floyd-steinberg succeeds");
        let bayer = raster_to_cmyk_image(&buf, w, h, CmykDither::Bayer8x8).expect("bayer succeeds");
        assert_ne!(
            none.image.image_data, fs.image.image_data,
            "Floyd-Steinberg should diverge from no-dither on gradient"
        );
        assert_ne!(
            none.image.image_data, bayer.image.image_data,
            "Bayer should diverge from no-dither on gradient"
        );
        // Both must still be the same length (no truncation).
        let expected_len = (w * h * 4) as usize;
        assert_eq!(none.image.image_data.len(), expected_len);
        assert_eq!(fs.image.image_data.len(), expected_len);
        assert_eq!(bayer.image.image_data.len(), expected_len);
    }
}
