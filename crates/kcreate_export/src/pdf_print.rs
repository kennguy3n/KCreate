//! Print-ready ("press-ready") PDF export.
//!
//! Where [`crate::pdf`] maps a document's world bounds onto a single
//! page (squishing any bleed into the trim), this module produces a
//! true press-ready PDF for one [`NodeType::Page`]:
//!
//! * The page's bounds define the **trim** rectangle. Content that
//!   extends past those bounds (a full-bleed background, a photo that
//!   runs off the edge) flows into the **bleed** margin instead of
//!   being clipped to the trim.
//! * The media box is the trim plus `bleed + mark` margins on every
//!   side, so there is room to draw **trim (crop) marks** and
//!   **registration targets** in the slug area.
//! * `/MediaBox`, `/TrimBox`, and `/BleedBox` are written so a RIP /
//!   imposition tool knows exactly where to cut.
//! * CMYK output reuses the existing [`crate::pdf`] emitters (which
//!   already run rasters through [`crate::cmyk_dither`] and gradients
//!   through [`crate::pdf_shading`]).
//! * **Spot colours** used on the page become real `/Separation`
//!   colour spaces with a linear tint-transform to their fallback
//!   CMYK, so each spot ink lands on its own plate.
//!
//! The export stays local-first: it walks the in-memory document and
//! returns bytes; the only post-processing is an in-memory `lopdf`
//! pass to attach the boxes + separation colour spaces, mirroring the
//! [`crate::pdf_shading::inject_shadings`] pattern.

use std::collections::BTreeMap;

use kcreate_core::color::{Color, SpotColorLibrary};
use kcreate_core::document::DocumentGraph;
use kcreate_core::node::{Node, NodeType, PageLayout, PAGE_LAYOUT_METADATA_KEY};
use kcreate_vector::VectorPath;
use lopdf::{dictionary, Dictionary, Document, Object, ObjectId};
use printpdf::lopdf::content::Operation as PdfOp;
use printpdf::lopdf::Object as PdfObj;
use printpdf::{Mm, PdfDocument, PdfLayerReference};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::cmyk_dither::CmykDither;
use crate::pdf::{
    as_f32, build_rings, emit_raster, emit_raw_path_operators, emit_vector, resolve_fill_paint,
    PdfColorMode, PdfExportError, PdfPaint, RasterPixelCache, PT_PER_MM,
};
use crate::pdf_shading::inject_shadings;
use crate::scene_metadata::VECTOR_PATH_METADATA_KEY;

/// Fallback DPI used to infer a trim size in millimetres from a
/// page's pixel bounds when no [`PageLayout`] metadata is present and
/// the caller didn't pass an explicit trim size. 300 DPI is the
/// commercial-print default the rest of the pipeline assumes.
const FALLBACK_PRINT_DPI: f64 = 300.0;
const MM_PER_INCH: f64 = 25.4;

/// Bézier circle constant: control-point offset = `KAPPA * r` draws a
/// quarter circle with four cubic segments to < 0.06 % error.
const KAPPA: f64 = 0.552_284_749_831;

/// Options for [`export_print_ready_pdf`]. Deserialised straight from
/// the renderer (`camelCase`), every field defaulted so the panel can
/// send a partial object.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PrintReadyOptions {
    /// Trim width in millimetres. `<= 0` infers from the page's
    /// [`PageLayout`], then from its pixel bounds at
    /// [`FALLBACK_PRINT_DPI`].
    pub trim_width_mm: f64,
    /// Trim height in millimetres. Inferred like `trim_width_mm`.
    pub trim_height_mm: f64,
    /// Bleed on every side, in millimetres. The conventional
    /// commercial default is 3 mm.
    pub bleed_mm: f64,
    /// Length of each trim / registration mark stroke, in mm.
    pub mark_length_mm: f64,
    /// Gap between the trim edge and the start of a trim mark, in mm.
    /// Clamped up to `bleed_mm` so marks never sit on the bleed.
    pub mark_offset_mm: f64,
    /// Trim / registration mark stroke weight, in PDF points. The
    /// press standard is a 0.25 pt hairline.
    pub mark_weight_pt: f64,
    /// Draw trim (crop) marks at the four trim corners.
    pub trim_marks: bool,
    /// Draw registration targets centred in each margin.
    pub registration_marks: bool,
    /// Device colour space for the content stream. Defaults to CMYK,
    /// which is what a press expects.
    pub color_mode: PdfColorMode,
    /// Dither used when rasterising images to `/DeviceCMYK`.
    pub cmyk_dither: CmykDither,
    /// Document title written to the PDF info dict.
    pub title: String,
}

impl Default for PrintReadyOptions {
    fn default() -> Self {
        Self {
            trim_width_mm: 0.0,
            trim_height_mm: 0.0,
            bleed_mm: 3.0,
            mark_length_mm: 5.0,
            mark_offset_mm: 3.0,
            mark_weight_pt: 0.25,
            trim_marks: true,
            registration_marks: true,
            color_mode: PdfColorMode::Cmyk,
            cmyk_dither: CmykDither::FloydSteinberg,
            title: "KCreate print-ready".to_string(),
        }
    }
}

/// A print-ready PDF plus the geometry the caller (bridge / panel)
/// surfaces back to the user so they can confirm the press setup.
#[derive(Debug, Clone)]
pub struct PrintReadyPdf {
    /// The encoded PDF bytes.
    pub bytes: Vec<u8>,
    /// Media box `(width_mm, height_mm)` — trim + margins.
    pub media_box_mm: (f64, f64),
    /// Trim box `(width_mm, height_mm)`.
    pub trim_box_mm: (f64, f64),
    /// Bleed applied on every side, in millimetres.
    pub bleed_mm: f64,
    /// Names of the spot inks that became `/Separation` plates, in
    /// stable resource order (`CS0`, `CS1`, …).
    pub spot_plates: Vec<String>,
    /// Colour space the content stream was written in.
    pub color_mode: PdfColorMode,
}

/// One spot ink discovered on the page, ready to become a
/// `/Separation` colour space.
#[derive(Debug, Clone)]
struct SpotPlate {
    /// Colorant name as it appears in the document (`"PANTONE 185 C"`).
    name: String,
    /// Fallback process colour `(c, m, y, k)` in `0..=1`. Used as the
    /// `C1` endpoint of the tint-transform function.
    fallback_cmyk: (f32, f32, f32, f32),
}

/// Registry mapping a spot colorant name to its resource index +
/// fallback. Insertion order is preserved via `order` so resource
/// names (`CS0`, `CS1`, …) are stable for tests and diffs.
#[derive(Debug, Default)]
struct SpotRegistry {
    by_name: BTreeMap<String, usize>,
    plates: Vec<SpotPlate>,
}

impl SpotRegistry {
    /// Record `color` if it is a spot ink, returning its resource
    /// index. The first definition wins for the fallback CMYK; a
    /// library entry (when present) is authoritative.
    fn intern(&mut self, name: &str, fallback_cmyk: (f32, f32, f32, f32)) -> usize {
        if let Some(idx) = self.by_name.get(name) {
            return *idx;
        }
        let idx = self.plates.len();
        self.by_name.insert(name.to_string(), idx);
        self.plates.push(SpotPlate {
            name: name.to_string(),
            fallback_cmyk,
        });
        idx
    }

    fn index_of(&self, name: &str) -> Option<usize> {
        self.by_name.get(name).copied()
    }

    fn is_empty(&self) -> bool {
        self.plates.is_empty()
    }
}

/// Resolved coordinate frame mapping document world units onto the
/// media box, in the form [`crate::pdf::world_to_pdf`] expects.
#[derive(Debug, Clone, Copy)]
struct PrintFrame {
    origin_x: f64,
    origin_y: f64,
    sx: f64,
    sy: f64,
    /// Margin from a media edge to the nearest trim edge, in mm
    /// (`= effective_offset + mark_length`).
    outer_mm: f64,
    trim_w_mm: f64,
    trim_h_mm: f64,
    bleed_mm: f64,
    media_w_mm: f64,
    media_h_mm: f64,
}

impl PrintFrame {
    fn media_w_pt(&self) -> f64 {
        self.media_w_mm * PT_PER_MM
    }
    fn media_h_pt(&self) -> f64 {
        self.media_h_mm * PT_PER_MM
    }
    /// Trim box in PDF points `(x0, y0, x1, y1)`.
    fn trim_box_pt(&self) -> (f64, f64, f64, f64) {
        let x0 = self.outer_mm;
        let y0 = self.outer_mm;
        let x1 = self.outer_mm + self.trim_w_mm;
        let y1 = self.outer_mm + self.trim_h_mm;
        (
            x0 * PT_PER_MM,
            y0 * PT_PER_MM,
            x1 * PT_PER_MM,
            y1 * PT_PER_MM,
        )
    }
    /// Bleed box in PDF points `(x0, y0, x1, y1)`.
    fn bleed_box_pt(&self) -> (f64, f64, f64, f64) {
        let (tx0, ty0, tx1, ty1) = self.trim_box_pt();
        let b = self.bleed_mm * PT_PER_MM;
        (tx0 - b, ty0 - b, tx1 + b, ty1 + b)
    }
}

/// Export one page of `document` as a print-ready PDF.
///
/// * `page_id` must name a [`NodeType::Page`] node; its bounds are the
///   trim rectangle.
/// * `rasters` supplies decoded pixels for any raster layers (same
///   cache the regular PDF export uses).
/// * `spots` is the project's spot-colour library; library entries
///   override a spot fill's inline fallback CMYK when emitting the
///   separation tint transform.
pub fn export_print_ready_pdf(
    document: &DocumentGraph,
    page_id: Uuid,
    options: &PrintReadyOptions,
    rasters: &RasterPixelCache,
    spots: &SpotColorLibrary,
) -> Result<PrintReadyPdf, PdfExportError> {
    let page = document
        .get_node(page_id)
        .ok_or_else(|| PdfExportError::InvalidPage(page_id, "page node not found".into()))?;
    if page.node_type != NodeType::Page {
        return Err(PdfExportError::InvalidPage(
            page_id,
            format!("node is {:?}, not a Page", page.node_type),
        ));
    }

    let frame = resolve_frame(page, options);

    let title = if options.title.is_empty() {
        "KCreate print-ready"
    } else {
        options.title.as_str()
    };
    let (doc, page1, layer1) = PdfDocument::new(
        title,
        Mm(as_f32(frame.media_w_mm)),
        Mm(as_f32(frame.media_h_mm)),
        "Layer 1",
    );
    let layer = doc.get_page(page1).get_layer(layer1);

    // Discover spot inks first so the content emitter can route their
    // fills through the matching `/Separation` colour space.
    let mut registry = SpotRegistry::default();
    collect_spots(
        document,
        &[page_id],
        options.color_mode,
        spots,
        &mut registry,
    );

    // Clip every content draw to the bleed box: anything outside the
    // bleed is slug area reserved for marks.
    let (bx0, by0, bx1, by1) = frame.bleed_box_pt();
    layer.add_operation(PdfOp::new("q", vec![]));
    push_rect(&layer, bx0, by0, bx1 - bx0, by1 - by0);
    layer.add_operation(PdfOp::new("W", vec![]));
    layer.add_operation(PdfOp::new("n", vec![]));

    let mut pending_shadings = Vec::new();
    walk_print_nodes(
        document,
        &[page_id],
        &layer,
        rasters,
        &frame,
        options,
        &registry,
        &mut pending_shadings,
    )?;

    layer.add_operation(PdfOp::new("Q", vec![]));

    // Marks live in the slug area, outside the content clip.
    draw_marks(&layer, &frame, options);

    let bytes = doc
        .save_to_bytes()
        .map_err(|e| PdfExportError::PrintPdf(e.to_string()))?;
    let bytes = inject_shadings(bytes, &pending_shadings)?;
    let bytes = post_process(bytes, &frame, &registry)?;

    Ok(PrintReadyPdf {
        bytes,
        media_box_mm: (frame.media_w_mm, frame.media_h_mm),
        trim_box_mm: (frame.trim_w_mm, frame.trim_h_mm),
        bleed_mm: frame.bleed_mm,
        spot_plates: registry.plates.iter().map(|p| p.name.clone()).collect(),
        color_mode: options.color_mode,
    })
}

/// Resolve the trim size (mm), then the world→media coordinate frame.
fn resolve_frame(page: &Node, options: &PrintReadyOptions) -> PrintFrame {
    let (trim_w_mm, trim_h_mm) = resolve_trim_mm(page, options);

    let trim_w_px = page.bounds.width.max(1.0);
    let trim_h_px = page.bounds.height.max(1.0);
    let page_x = page.bounds.x + page.transform.tx;
    let page_y = page.bounds.y + page.transform.ty;

    let bleed_mm = options.bleed_mm.max(0.0);
    let mark_length_mm = options.mark_length_mm.max(0.0);
    // Marks must never overlap the bleed, so the offset is at least
    // the bleed width.
    let effective_offset_mm = options.mark_offset_mm.max(bleed_mm);
    let outer_mm = effective_offset_mm + mark_length_mm;

    let sx = trim_w_mm / trim_w_px;
    let sy = trim_h_mm / trim_h_px;

    // Place the trim's top-left world corner at media coordinate
    // (outer_mm, outer_mm) — see module docs for the derivation.
    let origin_x = page_x - outer_mm / sx;
    let origin_y = page_y - outer_mm / sy;

    let media_w_mm = trim_w_mm + 2.0 * outer_mm;
    let media_h_mm = trim_h_mm + 2.0 * outer_mm;

    PrintFrame {
        origin_x,
        origin_y,
        sx,
        sy,
        outer_mm,
        trim_w_mm,
        trim_h_mm,
        bleed_mm,
        media_w_mm,
        media_h_mm,
    }
}

/// Resolve the trim size in millimetres: explicit option → page
/// layout → pixel bounds at [`FALLBACK_PRINT_DPI`].
fn resolve_trim_mm(page: &Node, options: &PrintReadyOptions) -> (f64, f64) {
    if options.trim_width_mm > 0.0 && options.trim_height_mm > 0.0 {
        return (options.trim_width_mm, options.trim_height_mm);
    }
    if let Some(value) = page.metadata.get(PAGE_LAYOUT_METADATA_KEY) {
        if let Ok(layout) = serde_json::from_value::<PageLayout>(value.clone()) {
            let (w, h) = layout.dimensions_mm();
            if w > 0.0 && h > 0.0 {
                return (w, h);
            }
        }
    }
    let px_to_mm = MM_PER_INCH / FALLBACK_PRINT_DPI;
    (
        (page.bounds.width.max(1.0) * px_to_mm).max(1.0),
        (page.bounds.height.max(1.0) * px_to_mm).max(1.0),
    )
}

/// Pre-scan the page subtree, recording every spot ink used on a
/// vector fill so it can be emitted as a `/Separation` plate.
fn collect_spots(
    document: &DocumentGraph,
    ids: &[Uuid],
    color_mode: PdfColorMode,
    spots: &SpotColorLibrary,
    registry: &mut SpotRegistry,
) {
    for id in ids {
        let Some(node) = document.get_node(*id) else {
            continue;
        };
        if node.visible && node.node_type == NodeType::VectorLayer {
            if let PdfPaint::Solid(Color::Spot {
                name,
                fallback_cmyk,
                ..
            }) = resolve_fill_paint(node, color_mode)
            {
                // A library definition is authoritative for the plate's
                // process build; fall back to the colour's inline value
                // for ad-hoc spots not yet in the library.
                let fallback = spots
                    .get(&name)
                    .map_or(fallback_cmyk, |def| def.fallback_cmyk);
                registry.intern(&name, fallback);
            }
        }
        collect_spots(document, &node.children, color_mode, spots, registry);
    }
}

/// Walk the page subtree, emitting each layer into the print frame.
/// Spot-filled vectors are routed through their `/Separation` colour
/// space; everything else reuses the regular [`crate::pdf`] emitters.
#[allow(clippy::too_many_arguments)]
fn walk_print_nodes(
    document: &DocumentGraph,
    ids: &[Uuid],
    layer: &PdfLayerReference,
    rasters: &RasterPixelCache,
    frame: &PrintFrame,
    options: &PrintReadyOptions,
    registry: &SpotRegistry,
    pending_shadings: &mut Vec<crate::pdf_shading::PendingShading>,
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
                if let Some((res_index, tint)) = spot_fill(node, options.color_mode, registry) {
                    emit_spot_vector(node, layer, frame, res_index, tint)?;
                } else {
                    emit_vector(
                        node,
                        layer,
                        frame.origin_x,
                        frame.origin_y,
                        frame.sx,
                        frame.sy,
                        frame.media_h_mm,
                        options.color_mode,
                        pending_shadings,
                    )?;
                }
            }
            NodeType::RasterLayer => {
                emit_raster(
                    node,
                    layer,
                    rasters,
                    frame.origin_x,
                    frame.origin_y,
                    frame.sx,
                    frame.sy,
                    frame.media_h_mm,
                    options.color_mode,
                    options.cmyk_dither,
                )?;
            }
            _ => {}
        }
        walk_print_nodes(
            document,
            &node.children,
            layer,
            rasters,
            frame,
            options,
            registry,
            pending_shadings,
        )?;
    }
    Ok(())
}

/// If `node`'s resolved fill is a spot ink in the registry, return its
/// `(resource_index, tint)`.
fn spot_fill(
    node: &Node,
    color_mode: PdfColorMode,
    registry: &SpotRegistry,
) -> Option<(usize, f64)> {
    match resolve_fill_paint(node, color_mode) {
        PdfPaint::Solid(Color::Spot { name, tint, .. }) => registry
            .index_of(&name)
            .map(|idx| (idx, f64::from(tint.clamp(0.0, 1.0)))),
        _ => None,
    }
}

/// Emit a spot-filled vector using its `/Separation` colour space:
/// `q [/CSn cs] [tint scn] <path> f Q`. Honours per-node overprint.
fn emit_spot_vector(
    node: &Node,
    layer: &PdfLayerReference,
    frame: &PrintFrame,
    res_index: usize,
    tint: f64,
) -> Result<(), PdfExportError> {
    let Some(value) = node.metadata.get(VECTOR_PATH_METADATA_KEY) else {
        return Ok(());
    };
    let path: VectorPath = serde_json::from_value(value.clone())
        .map_err(|e| PdfExportError::InvalidVectorPath(node.id, e.to_string()))?;
    let rings = build_rings(
        &path,
        node.transform.tx,
        node.transform.ty,
        frame.origin_x,
        frame.origin_y,
        frame.sx,
        frame.sy,
        frame.media_h_mm,
    );
    if rings.is_empty() {
        return Ok(());
    }

    layer.save_graphics_state();
    if node.style.overprint {
        layer.set_overprint_fill(true);
        layer.set_overprint_stroke(true);
    }
    let cs_name = separation_resource_name(res_index);
    layer.add_operation(PdfOp::new("cs", vec![PdfObj::Name(cs_name.into_bytes())]));
    layer.add_operation(PdfOp::new("scn", vec![PdfObj::Real(as_f32(tint))]));
    emit_raw_path_operators(layer, &rings);
    // Non-zero winding fill, matching the regular solid-fill emitter.
    layer.add_operation(PdfOp::new("f", vec![]));
    layer.restore_graphics_state();
    Ok(())
}

/// Resource name for the `n`th separation colour space (`CS0`, …).
fn separation_resource_name(index: usize) -> String {
    format!("CS{index}")
}

/// Push a rectangle path `x y w h re` (PDF points) onto the layer.
fn push_rect(layer: &PdfLayerReference, x: f64, y: f64, w: f64, h: f64) {
    layer.add_operation(PdfOp::new(
        "re",
        vec![
            PdfObj::Real(as_f32(x)),
            PdfObj::Real(as_f32(y)),
            PdfObj::Real(as_f32(w)),
            PdfObj::Real(as_f32(h)),
        ],
    ));
}

/// Draw trim (crop) marks at the four trim corners and registration
/// targets centred in each margin, all in the registration colour
/// (CMYK `1,1,1,1` so the mark prints on every plate; black in RGB).
fn draw_marks(layer: &PdfLayerReference, frame: &PrintFrame, options: &PrintReadyOptions) {
    if !options.trim_marks && !options.registration_marks {
        return;
    }
    let (tx0, ty0, tx1, ty1) = frame.trim_box_pt();
    let offset_pt = options.mark_offset_mm.max(frame.bleed_mm) * PT_PER_MM;
    let len_pt = options.mark_length_mm.max(0.0) * PT_PER_MM;

    layer.add_operation(PdfOp::new("q", vec![]));
    // Stroke colour: registration.
    match options.color_mode {
        PdfColorMode::Cmyk | PdfColorMode::PassThrough => {
            layer.add_operation(PdfOp::new(
                "K",
                vec![
                    PdfObj::Real(1.0),
                    PdfObj::Real(1.0),
                    PdfObj::Real(1.0),
                    PdfObj::Real(1.0),
                ],
            ));
        }
        PdfColorMode::Rgb => {
            layer.add_operation(PdfOp::new(
                "RG",
                vec![PdfObj::Real(0.0), PdfObj::Real(0.0), PdfObj::Real(0.0)],
            ));
        }
    }
    layer.add_operation(PdfOp::new(
        "w",
        vec![PdfObj::Real(as_f32(options.mark_weight_pt.max(0.05)))],
    ));

    if options.trim_marks && len_pt > 0.0 {
        // Bottom-left.
        stroke_line(layer, tx0 - offset_pt - len_pt, ty0, tx0 - offset_pt, ty0);
        stroke_line(layer, tx0, ty0 - offset_pt - len_pt, tx0, ty0 - offset_pt);
        // Bottom-right.
        stroke_line(layer, tx1 + offset_pt, ty0, tx1 + offset_pt + len_pt, ty0);
        stroke_line(layer, tx1, ty0 - offset_pt - len_pt, tx1, ty0 - offset_pt);
        // Top-left.
        stroke_line(layer, tx0 - offset_pt - len_pt, ty1, tx0 - offset_pt, ty1);
        stroke_line(layer, tx0, ty1 + offset_pt, tx0, ty1 + offset_pt + len_pt);
        // Top-right.
        stroke_line(layer, tx1 + offset_pt, ty1, tx1 + offset_pt + len_pt, ty1);
        stroke_line(layer, tx1, ty1 + offset_pt, tx1, ty1 + offset_pt + len_pt);
    }

    if options.registration_marks {
        let media_w_pt = frame.media_w_pt();
        let media_h_pt = frame.media_h_pt();
        let outer_pt = frame.outer_mm * PT_PER_MM;
        let r = (outer_pt * 0.30).min(len_pt * 0.5).max(2.0);
        // Centred in each margin band.
        let cx = media_w_pt * 0.5;
        let cy = media_h_pt * 0.5;
        registration_target(layer, cx, ty1 + outer_pt * 0.5, r); // top
        registration_target(layer, cx, ty0 - outer_pt * 0.5, r); // bottom
        registration_target(layer, tx0 - outer_pt * 0.5, cy, r); // left
        registration_target(layer, tx1 + outer_pt * 0.5, cy, r); // right
    }

    layer.add_operation(PdfOp::new("Q", vec![]));
}

/// Stroke a single line segment between two PDF-point coordinates.
fn stroke_line(layer: &PdfLayerReference, x0: f64, y0: f64, x1: f64, y1: f64) {
    layer.add_operation(PdfOp::new(
        "m",
        vec![PdfObj::Real(as_f32(x0)), PdfObj::Real(as_f32(y0))],
    ));
    layer.add_operation(PdfOp::new(
        "l",
        vec![PdfObj::Real(as_f32(x1)), PdfObj::Real(as_f32(y1))],
    ));
    layer.add_operation(PdfOp::new("S", vec![]));
}

/// Draw a registration target (circle + crosshair) centred at
/// `(cx, cy)` with radius `r`, all in PDF points.
fn registration_target(layer: &PdfLayerReference, cx: f64, cy: f64, r: f64) {
    let k = KAPPA * r;
    // Circle as four cubic Béziers, starting at the 3-o'clock point.
    move_to(layer, cx + r, cy);
    curve_to(layer, cx + r, cy + k, cx + k, cy + r, cx, cy + r);
    curve_to(layer, cx - k, cy + r, cx - r, cy + k, cx - r, cy);
    curve_to(layer, cx - r, cy - k, cx - k, cy - r, cx, cy - r);
    curve_to(layer, cx + k, cy - r, cx + r, cy - k, cx + r, cy);
    layer.add_operation(PdfOp::new("S", vec![]));
    // Crosshair extending past the circle.
    let arm = r * 1.8;
    stroke_line(layer, cx - arm, cy, cx + arm, cy);
    stroke_line(layer, cx, cy - arm, cx, cy + arm);
}

fn move_to(layer: &PdfLayerReference, x: f64, y: f64) {
    layer.add_operation(PdfOp::new(
        "m",
        vec![PdfObj::Real(as_f32(x)), PdfObj::Real(as_f32(y))],
    ));
}

fn curve_to(layer: &PdfLayerReference, c1x: f64, c1y: f64, c2x: f64, c2y: f64, x: f64, y: f64) {
    layer.add_operation(PdfOp::new(
        "c",
        vec![
            PdfObj::Real(as_f32(c1x)),
            PdfObj::Real(as_f32(c1y)),
            PdfObj::Real(as_f32(c2x)),
            PdfObj::Real(as_f32(c2y)),
            PdfObj::Real(as_f32(x)),
            PdfObj::Real(as_f32(y)),
        ],
    ));
}

/// In-memory `lopdf` pass: set `/MediaBox`, `/TrimBox`, `/BleedBox` on
/// the page and attach every spot `/Separation` colour space to the
/// page's `/Resources/ColorSpace`.
fn post_process(
    bytes: Vec<u8>,
    frame: &PrintFrame,
    registry: &SpotRegistry,
) -> Result<Vec<u8>, PdfExportError> {
    let mut doc =
        Document::load_mem(&bytes).map_err(|e| PdfExportError::PrintPdf(e.to_string()))?;

    let pages: Vec<(u32, ObjectId)> = doc.get_pages().into_iter().collect();
    let Some((_, page_id)) = pages.first().copied() else {
        return Err(PdfExportError::PrintPdf("output PDF has no pages".into()));
    };

    // Boxes.
    let media = rect_object(0.0, 0.0, frame.media_w_pt(), frame.media_h_pt());
    let (tx0, ty0, tx1, ty1) = frame.trim_box_pt();
    let trim = rect_object(tx0, ty0, tx1, ty1);
    let (bx0, by0, bx1, by1) = frame.bleed_box_pt();
    let bleed = rect_object(bx0, by0, bx1, by1);
    {
        let page_dict = doc
            .get_dictionary_mut(page_id)
            .map_err(|e| PdfExportError::PrintPdf(e.to_string()))?;
        page_dict.set("MediaBox", media);
        page_dict.set("TrimBox", trim);
        page_dict.set("BleedBox", bleed);
    }

    // Spot separations.
    if !registry.is_empty() {
        let mut cs_refs: Vec<(usize, ObjectId)> = Vec::with_capacity(registry.plates.len());
        for (index, plate) in registry.plates.iter().enumerate() {
            let func_id = add_tint_transform(&mut doc, plate.fallback_cmyk);
            let sep_id = add_separation_colorspace(&mut doc, &plate.name, func_id);
            cs_refs.push((index, sep_id));
        }
        attach_colorspaces_to_page(&mut doc, page_id, &cs_refs)?;
    }

    let mut out = Vec::with_capacity(bytes.len() + 2048);
    doc.save_to(&mut out)
        .map_err(|e| PdfExportError::PrintPdf(e.to_string()))?;
    Ok(out)
}

/// Build a PDF rectangle array `[x0 y0 x1 y1]`.
fn rect_object(x0: f64, y0: f64, x1: f64, y1: f64) -> Object {
    Object::Array(vec![
        Object::Real(x0 as f32),
        Object::Real(y0 as f32),
        Object::Real(x1 as f32),
        Object::Real(y1 as f32),
    ])
}

/// Add a Type 2 (exponential) tint-transform mapping spot tint
/// `t ∈ [0,1]` linearly to `t * fallback_cmyk` in `/DeviceCMYK`.
fn add_tint_transform(doc: &mut Document, fallback_cmyk: (f32, f32, f32, f32)) -> ObjectId {
    let (c, m, y, k) = fallback_cmyk;
    let func = dictionary! {
        "FunctionType" => 2_i64,
        "Domain" => Object::Array(vec![Object::Real(0.0), Object::Real(1.0)]),
        "C0" => Object::Array(vec![
            Object::Real(0.0), Object::Real(0.0), Object::Real(0.0), Object::Real(0.0),
        ]),
        "C1" => Object::Array(vec![
            Object::Real(c.clamp(0.0, 1.0)),
            Object::Real(m.clamp(0.0, 1.0)),
            Object::Real(y.clamp(0.0, 1.0)),
            Object::Real(k.clamp(0.0, 1.0)),
        ]),
        "N" => 1_i64,
        "Range" => Object::Array(vec![
            Object::Real(0.0), Object::Real(1.0),
            Object::Real(0.0), Object::Real(1.0),
            Object::Real(0.0), Object::Real(1.0),
            Object::Real(0.0), Object::Real(1.0),
        ]),
    };
    doc.add_object(func)
}

/// Add a `[/Separation /Name /DeviceCMYK tintFn]` colour-space array.
fn add_separation_colorspace(doc: &mut Document, name: &str, func_id: ObjectId) -> ObjectId {
    let array = Object::Array(vec![
        Object::Name(b"Separation".to_vec()),
        Object::Name(pdf_name_bytes(name)),
        Object::Name(b"DeviceCMYK".to_vec()),
        Object::Reference(func_id),
    ]);
    doc.add_object(array)
}

/// Encode an arbitrary colorant name as PDF name bytes, `#`-escaping
/// the characters that aren't valid in a name token (spaces,
/// delimiters, control / non-ASCII bytes) per PDF 32000-1 §7.3.5.
fn pdf_name_bytes(name: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(name.len());
    for &b in name.as_bytes() {
        let regular = b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'+');
        if regular {
            out.push(b);
        } else {
            out.push(b'#');
            out.extend_from_slice(format!("{b:02X}").as_bytes());
        }
    }
    out
}

/// Attach separation colour spaces to the page's
/// `/Resources/ColorSpace` dict, mirroring
/// [`crate::pdf_shading`]'s resource-attachment handling of inline vs
/// indirect `/Resources`.
fn attach_colorspaces_to_page(
    doc: &mut Document,
    page_id: ObjectId,
    colorspaces: &[(usize, ObjectId)],
) -> Result<(), PdfExportError> {
    let page_dict = doc
        .get_dictionary(page_id)
        .map_err(|e| PdfExportError::PrintPdf(e.to_string()))?;
    let resources_target = match page_dict.get(b"Resources").ok() {
        Some(Object::Reference(id)) => Some(*id),
        Some(Object::Dictionary(_)) => None,
        _ => {
            return Err(PdfExportError::PrintPdf(format!(
                "page {page_id:?} has no usable /Resources dict"
            )));
        }
    };

    let resources_dict: &mut Dictionary = match resources_target {
        Some(id) => doc
            .get_dictionary_mut(id)
            .map_err(|e| PdfExportError::PrintPdf(e.to_string()))?,
        None => doc
            .get_dictionary_mut(page_id)
            .map_err(|e| PdfExportError::PrintPdf(e.to_string()))?
            .get_mut(b"Resources")
            .map_err(|e| PdfExportError::PrintPdf(e.to_string()))?
            .as_dict_mut()
            .map_err(|e| PdfExportError::PrintPdf(e.to_string()))?,
    };

    let mut cs_dict: Dictionary = match resources_dict.get(b"ColorSpace") {
        Ok(Object::Dictionary(d)) => d.clone(),
        _ => Dictionary::new(),
    };
    for (index, oid) in colorspaces {
        cs_dict.set(
            separation_resource_name(*index).into_bytes(),
            Object::Reference(*oid),
        );
    }
    resources_dict.set("ColorSpace", Object::Dictionary(cs_dict));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use kcreate_core::color::SpotColorDef;
    use kcreate_core::node::{Bounds, FillStyle, NodeStyle, RgbaColor};
    use kcreate_vector::{PathPoint, PathSegment, VectorPath};

    fn rect_path(w: f64, h: f64) -> VectorPath {
        VectorPath::new(vec![
            PathSegment::MoveTo(PathPoint::new(0.0, 0.0)),
            PathSegment::LineTo(PathPoint::new(w, 0.0)),
            PathSegment::LineTo(PathPoint::new(w, h)),
            PathSegment::LineTo(PathPoint::new(0.0, h)),
            PathSegment::Close,
        ])
    }

    fn vector_node(name: &str, x: f64, y: f64, w: f64, h: f64) -> Node {
        let mut node = Node::new(NodeType::VectorLayer, name);
        node.bounds = Bounds {
            x,
            y,
            width: w,
            height: h,
        };
        node.transform.tx = x;
        node.transform.ty = y;
        node.style.fill = FillStyle::Solid(RgbaColor::new(0.1, 0.4, 0.9, 1.0));
        node.metadata.insert(
            VECTOR_PATH_METADATA_KEY.to_string(),
            serde_json::to_value(rect_path(w, h)).unwrap(),
        );
        node
    }

    /// Build a one-page document: an A4 page (2480×3508 px @300dpi)
    /// with a full-bleed background that runs past the page edge.
    fn build_doc() -> (DocumentGraph, Uuid) {
        let mut doc = DocumentGraph::new();
        let mut page = Node::new(NodeType::Page, "Page");
        page.bounds = Bounds {
            x: 0.0,
            y: 0.0,
            width: 2480.0,
            height: 3508.0,
        };
        let page_id = doc.insert_node(page).unwrap();

        // Full-bleed background: extends 40px past every edge.
        let mut bg = vector_node("bg", -40.0, -40.0, 2560.0, 3588.0);
        bg.style.fill = FillStyle::Solid(RgbaColor::new(0.95, 0.2, 0.3, 1.0));
        bg.parent_id = Some(page_id);
        doc.insert_node(bg).unwrap();

        // Foreground element well inside the trim.
        let mut fg = vector_node("fg", 400.0, 400.0, 600.0, 400.0);
        fg.parent_id = Some(page_id);
        doc.insert_node(fg).unwrap();

        (doc, page_id)
    }

    #[test]
    fn print_ready_pdf_has_bleed_and_marks() {
        let (doc, page_id) = build_doc();
        let opts = PrintReadyOptions::default();
        let rasters = RasterPixelCache::new();
        let spots = SpotColorLibrary::new();
        let out = export_print_ready_pdf(&doc, page_id, &opts, &rasters, &spots).unwrap();

        assert!(out.bytes.starts_with(b"%PDF-"));
        // Media box must exceed the trim by the full margin on both
        // axes (bleed + marks on each side).
        assert!(out.media_box_mm.0 > out.trim_box_mm.0 + 2.0 * out.bleed_mm);
        assert!(out.media_box_mm.1 > out.trim_box_mm.1 + 2.0 * out.bleed_mm);
        assert!((out.trim_box_mm.0 - 210.0).abs() < 1.0);
        assert!((out.trim_box_mm.1 - 297.0).abs() < 1.0);

        // The boxes must be present in the output PDF.
        let reloaded = Document::load_mem(&out.bytes).unwrap();
        let (_, page_oid) = reloaded.get_pages().into_iter().next().unwrap();
        let page_dict = reloaded.get_dictionary(page_oid).unwrap();
        assert!(page_dict.get(b"TrimBox").is_ok());
        assert!(page_dict.get(b"BleedBox").is_ok());
        assert!(page_dict.get(b"MediaBox").is_ok());
    }

    #[test]
    fn cmyk_mode_emits_devicecmyk_operator() {
        let (doc, page_id) = build_doc();
        let opts = PrintReadyOptions {
            color_mode: PdfColorMode::Cmyk,
            ..PrintReadyOptions::default()
        };
        let rasters = RasterPixelCache::new();
        let spots = SpotColorLibrary::new();
        let out = export_print_ready_pdf(&doc, page_id, &opts, &rasters, &spots).unwrap();
        let text = decompressed_streams(&out.bytes);
        // DeviceCMYK fill operator `k` must appear.
        assert!(
            text.contains(" k\n") || text.contains(" k ") || text.contains(" k"),
            "expected a CMYK fill operator in the content stream"
        );
    }

    #[test]
    fn spot_color_becomes_separation_plate() {
        let mut doc = DocumentGraph::new();
        let mut page = Node::new(NodeType::Page, "Page");
        page.bounds = Bounds {
            x: 0.0,
            y: 0.0,
            width: 1000.0,
            height: 1000.0,
        };
        let page_id = doc.insert_node(page).unwrap();

        let mut spot = vector_node("spot", 200.0, 200.0, 400.0, 400.0);
        spot.style = NodeStyle {
            color_override: Some(Color::Spot {
                name: "PANTONE 185 C".to_string(),
                fallback_cmyk: (0.0, 0.9, 0.8, 0.0),
                tint: 1.0,
                alpha: 1.0,
            }),
            ..spot.style.clone()
        };
        spot.parent_id = Some(page_id);
        doc.insert_node(spot).unwrap();

        let mut spots = SpotColorLibrary::new();
        spots.insert(
            "PANTONE 185 C",
            SpotColorDef {
                display_name: "PANTONE 185 C".to_string(),
                fallback_cmyk: (0.0, 0.9, 0.8, 0.0),
                library_reference: Some("PANTONE".to_string()),
            },
        );

        let opts = PrintReadyOptions::default();
        let rasters = RasterPixelCache::new();
        let out = export_print_ready_pdf(&doc, page_id, &opts, &rasters, &spots).unwrap();

        assert_eq!(out.spot_plates, vec!["PANTONE 185 C".to_string()]);
        // A `/Separation` colour space must be present in the output.
        let text = String::from_utf8_lossy(&out.bytes);
        assert!(
            text.contains("Separation"),
            "expected a /Separation colour space in the PDF"
        );
    }

    #[test]
    fn pdf_name_escaping_handles_spaces_and_specials() {
        assert_eq!(
            pdf_name_bytes("PANTONE 185 C"),
            b"PANTONE#20185#20C".to_vec()
        );
        assert_eq!(pdf_name_bytes("Cyan"), b"Cyan".to_vec());
    }

    /// Inflate every Flate-encoded stream in the PDF so tests can
    /// assert on raw content operators.
    fn decompressed_streams(bytes: &[u8]) -> String {
        let doc = Document::load_mem(bytes).unwrap();
        let mut out = String::new();
        for obj in doc.objects.values() {
            if let Object::Stream(stream) = obj {
                let data = stream
                    .decompressed_content()
                    .unwrap_or_else(|_| stream.content.clone());
                out.push_str(&String::from_utf8_lossy(&data));
                out.push('\n');
            }
        }
        out
    }
}
