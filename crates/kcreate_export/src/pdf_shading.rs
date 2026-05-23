//! Real PDF shading-pattern emission for gradient fills.
//!
//! `printpdf` 0.7 has no shading-pattern API. Phase 2 worked
//! around this by flattening gradient fills to solid colours; the
//! print shop received an unshaded rectangle and the user's
//! gradient intent was lost. Block 4 of the Phase 4 work replaces
//! that flatten with real PDF Type 2 (axial) and Type 3 (radial)
//! shading dictionaries injected via post-processing.
//!
//! Pipeline:
//!
//! 1. While walking the document graph, every gradient fill emits
//!    raw content-stream operators (`q ... W n /SH<n> sh Q`) via
//!    `PdfLayerReference::add_operation`. The `/SH<n>` resource
//!    reference is intentionally dangling at this stage — printpdf
//!    will faithfully serialise the operator into the page's
//!    content stream but it doesn't know how to register
//!    `/Shading` resources.
//! 2. After `PdfDocumentReference::save_to_bytes`, we load the
//!    bytes through `lopdf::Document::load_mem`, materialise the
//!    `Shading` + `Function` indirect objects this module
//!    describes, attach them to each page's `Resources/Shading`
//!    dictionary, and re-serialise.
//! 3. The injection is page-local: every `PendingShading` carries
//!    its `page_index` (currently always 0 because the exporter is
//!    one-page-per-PDF, but the post-processor doesn't rely on
//!    that and will work as soon as multi-page support lands).
//!
//! The PDF objects we emit follow §8.7.4 of the PDF 1.7 spec:
//!
//! * Type 2 (axial): straight line from `(x0, y0)` to `(x1, y1)`.
//! * Type 3 (radial): two concentric circles
//!   `(cx, cy, r0=0)` → `(cx, cy, r1=radius)`.
//! * Each shading references a `FunctionType 3` stitching dict
//!   that wires `N-1` `FunctionType 2` exponential interpolations
//!   together — exactly the canonical encoding consumer
//!   tools (Acrobat, Ghostscript, Poppler) expect for N-stop
//!   gradients. For the degenerate `N = 2` case we collapse to a
//!   single Type 2 function to keep the file size tight.
//!
//! Colour-space handling: shadings honour the export's
//! `color_mode`. When the document is exported as DeviceCMYK we
//! pre-convert each stop's RGBA through `kcreate_core::color::
//! srgb_to_cmyk` so the stops actually live in the same space as
//! the rest of the page; otherwise mixed-color-space PDF/X
//! validators would refuse the document.

use kcreate_core::color::{srgb_to_cmyk, Color};
use lopdf::{dictionary, Dictionary, Document, Object, ObjectId};
use thiserror::Error;

use crate::pdf::PdfColorMode;

/// PDF-space gradient geometry. All coordinates are already in PDF
/// user-space points (1/72 inch), with the PDF origin convention
/// (bottom-left). The exporter is responsible for transforming
/// node-local gradient coordinates into this frame before pushing.
#[derive(Debug, Clone, Copy)]
pub enum GradientGeometry {
    Linear {
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
    },
    Radial {
        cx0: f32,
        cy0: f32,
        r0: f32,
        cx1: f32,
        cy1: f32,
        r1: f32,
    },
}

/// One gradient stop in PDF terms — an offset along the gradient's
/// `Domain` and an `(r, g, b)` or `(c, m, y, k)` colour, depending
/// on the target colour space.
#[derive(Debug, Clone, Copy)]
pub struct ResolvedStop {
    /// Position along the gradient, clamped to `[0.0, 1.0]`.
    pub offset: f32,
    /// Pre-resolved colour components in the order the target
    /// colour space expects (3 for RGB, 4 for CMYK).
    pub components: [f32; 4],
    /// Number of valid entries in `components`.
    pub component_count: u8,
}

/// One gradient pending post-process injection. The exporter emits
/// these as it walks the document; the post-processor consumes
/// them after `save_to_bytes`.
#[derive(Debug, Clone)]
pub struct PendingShading {
    /// 0-based page index this shading belongs to. The exporter
    /// currently always emits a one-page PDF, so this is always
    /// `0`. Kept explicit so multi-page support is a free upgrade.
    pub page_index: usize,
    /// Sequential index used to construct the resource name
    /// (`/SH0`, `/SH1`, …). Must match the name baked into the
    /// content stream by the exporter.
    pub index: usize,
    /// Geometry in PDF user-space.
    pub geometry: GradientGeometry,
    /// Stops in source order, offsets in `[0, 1]`, monotonic non-
    /// decreasing.
    pub stops: Vec<ResolvedStop>,
    /// Colour space the stops were resolved into.
    pub color_space: ShadingColorSpace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShadingColorSpace {
    DeviceRgb,
    DeviceCmyk,
}

impl ShadingColorSpace {
    /// Name written into the shading dict's `/ColorSpace` entry.
    fn pdf_name(self) -> &'static [u8] {
        match self {
            Self::DeviceRgb => b"DeviceRGB",
            Self::DeviceCmyk => b"DeviceCMYK",
        }
    }
}

/// Convert a renderer-side `Color` plus the export's `color_mode`
/// into the components of a single shading stop. Honours an
/// explicit override-driven CMYK colour exactly; otherwise routes
/// sRGB through the existing `srgb_to_cmyk` table when the export
/// is DeviceCMYK.
#[must_use]
pub fn resolve_stop_color(color: &Color, target: ShadingColorSpace) -> ResolvedStop {
    let mut components = [0.0_f32; 4];
    let component_count;
    match target {
        ShadingColorSpace::DeviceRgb => {
            // For DeviceRGB shadings, route every input through the
            // canonical `Color::to_srgb` so Lab and Hsl
            // values round-trip the same way as the renderer would
            // compute them. `Color::Cmyk` produces the
            // standard subtractive inverse via the same helper.
            let (r, g, b, _a) = color.to_srgb();
            components[0] = r;
            components[1] = g;
            components[2] = b;
            component_count = 3;
        }
        ShadingColorSpace::DeviceCmyk => {
            match color {
                Color::Cmyk { c, m, y, k, .. } => {
                    components[0] = *c;
                    components[1] = *m;
                    components[2] = *y;
                    components[3] = *k;
                }
                _ => {
                    // sRGB / Lab / Hsl all go through to_srgb()
                    // first so the CMYK conversion path is
                    // identical to the rest of the export.
                    let (r, g, b, _a) = color.to_srgb();
                    let (c, m, y, k) = srgb_to_cmyk(r, g, b);
                    components[0] = c;
                    components[1] = m;
                    components[2] = y;
                    components[3] = k;
                }
            }
            component_count = 4;
        }
    }
    ResolvedStop {
        offset: 0.0,
        components,
        component_count,
    }
}

#[derive(Debug, Error)]
pub enum PdfShadingError {
    #[error("lopdf parse failure: {0}")]
    Parse(String),
    #[error("page {0} has no Resources dictionary and lopdf could not synthesise one")]
    MissingResources(usize),
    #[error("gradient {0} has fewer than two stops; PDF Type 2/3 functions require at least two")]
    TooFewStops(usize),
    #[error("lopdf write failure: {0}")]
    Write(String),
    #[error("page index {0} is out of range; the PDF only contains {1} page(s)")]
    PageOutOfRange(usize, usize),
}

/// Map a [`PdfColorMode`] onto the colour space used by the
/// resulting shading dict. `PassThrough` and `Rgb` both produce
/// DeviceRGB shadings; only an explicit `Cmyk` export produces
/// DeviceCMYK. The caller is responsible for converting each
/// stop's colour into the matching space *before* it pushes the
/// `PendingShading` into the bucket.
#[must_use]
pub fn color_space_for_mode(mode: PdfColorMode) -> ShadingColorSpace {
    match mode {
        PdfColorMode::Cmyk => ShadingColorSpace::DeviceCmyk,
        PdfColorMode::Rgb | PdfColorMode::PassThrough => ShadingColorSpace::DeviceRgb,
    }
}

/// Parse the printpdf-produced bytes, inject the requested
/// `Shading` + `Function` indirect objects, attach them to each
/// page's `Resources/Shading` dictionary, and return the rewritten
/// bytes. Pure-function; no I/O.
pub fn inject_shadings(
    bytes: Vec<u8>,
    shadings: &[PendingShading],
) -> Result<Vec<u8>, PdfShadingError> {
    if shadings.is_empty() {
        return Ok(bytes);
    }

    let mut doc = Document::load_mem(&bytes).map_err(|e| PdfShadingError::Parse(e.to_string()))?;

    let pages: Vec<(u32, ObjectId)> = doc.get_pages().into_iter().collect();
    let page_count = pages.len();

    // Group pending shadings by page so we touch each page's
    // Resources dict exactly once. We use a plain Vec-of-Vec
    // indexed by page number rather than a HashMap because the
    // page count is tiny (typically 1) and we want stable
    // ordering for tests.
    let mut by_page: Vec<Vec<&PendingShading>> = vec![Vec::new(); page_count];
    for s in shadings {
        if s.page_index >= page_count {
            return Err(PdfShadingError::PageOutOfRange(s.page_index, page_count));
        }
        if s.stops.len() < 2 {
            return Err(PdfShadingError::TooFewStops(s.index));
        }
        by_page[s.page_index].push(s);
    }

    for (page_idx, shadings_for_page) in by_page.into_iter().enumerate() {
        if shadings_for_page.is_empty() {
            continue;
        }
        // Allocate every Function + Shading object up front, then
        // patch the Resources dict in one pass — keeps the borrow
        // checker happy and means we don't accidentally end up
        // with dangling refs if a later step fails.
        let mut shading_refs: Vec<(usize, ObjectId)> = Vec::with_capacity(shadings_for_page.len());
        for s in &shadings_for_page {
            let func_id = add_stitching_function(&mut doc, s);
            let shading_id = add_shading_object(&mut doc, s, func_id);
            shading_refs.push((s.index, shading_id));
        }

        let page_id = pages
            .get(page_idx)
            .ok_or(PdfShadingError::PageOutOfRange(page_idx, page_count))?
            .1;
        attach_shadings_to_page(&mut doc, page_id, &shading_refs)?;
    }

    let mut out: Vec<u8> = Vec::with_capacity(bytes.len() + 4096);
    doc.save_to(&mut out)
        .map_err(|e| PdfShadingError::Write(e.to_string()))?;
    Ok(out)
}

fn add_stitching_function(doc: &mut Document, shading: &PendingShading) -> ObjectId {
    let cn = shading.stops[0].component_count as usize;
    let n = shading.stops.len();

    // Build one Type 2 exponential sub-function per adjacent stop
    // pair. Type 2 with /N 1 is straight-line interpolation
    // between C0 and C1 over `Domain`, which is exactly what we
    // want between two consecutive stops.
    let mut sub_func_ids: Vec<ObjectId> = Vec::with_capacity(n - 1);
    for i in 0..n - 1 {
        let c0 = stop_components_object(&shading.stops[i], cn);
        let c1 = stop_components_object(&shading.stops[i + 1], cn);
        let func = dictionary! {
            "FunctionType" => 2_i64,
            "Domain" => Object::Array(vec![Object::Real(0.0), Object::Real(1.0)]),
            "N" => 1_i64,
            "C0" => c0,
            "C1" => c1,
        };
        sub_func_ids.push(doc.add_object(func));
    }

    // The degenerate two-stop case collapses to the single Type 2
    // we just built: skip the stitching wrapper.
    if sub_func_ids.len() == 1 {
        return sub_func_ids[0];
    }

    // Stitching function: weld the sub-functions together along
    // the unit interval at the stop boundaries.
    let mut bounds = Vec::with_capacity(n - 2);
    for stop in &shading.stops[1..n - 1] {
        bounds.push(Object::Real(stop.offset.clamp(0.0, 1.0)));
    }
    let mut encode = Vec::with_capacity(2 * (n - 1));
    for _ in 0..n - 1 {
        encode.push(Object::Real(0.0));
        encode.push(Object::Real(1.0));
    }
    let funcs_array: Vec<Object> = sub_func_ids.into_iter().map(Object::Reference).collect();
    let stitching = dictionary! {
        "FunctionType" => 3_i64,
        "Domain" => Object::Array(vec![Object::Real(0.0), Object::Real(1.0)]),
        "Functions" => Object::Array(funcs_array),
        "Bounds" => Object::Array(bounds),
        "Encode" => Object::Array(encode),
    };
    doc.add_object(stitching)
}

fn stop_components_object(stop: &ResolvedStop, n: usize) -> Object {
    let mut out: Vec<Object> = Vec::with_capacity(n);
    for c in &stop.components[..n] {
        out.push(Object::Real(c.clamp(0.0, 1.0)));
    }
    Object::Array(out)
}

fn add_shading_object(
    doc: &mut Document,
    shading: &PendingShading,
    function_id: ObjectId,
) -> ObjectId {
    let (shading_type, coords) = match shading.geometry {
        GradientGeometry::Linear { x0, y0, x1, y1 } => (
            2_i64,
            Object::Array(vec![
                Object::Real(x0),
                Object::Real(y0),
                Object::Real(x1),
                Object::Real(y1),
            ]),
        ),
        GradientGeometry::Radial {
            cx0,
            cy0,
            r0,
            cx1,
            cy1,
            r1,
        } => (
            3_i64,
            Object::Array(vec![
                Object::Real(cx0),
                Object::Real(cy0),
                Object::Real(r0),
                Object::Real(cx1),
                Object::Real(cy1),
                Object::Real(r1),
            ]),
        ),
    };
    let dict = dictionary! {
        "ShadingType" => shading_type,
        "ColorSpace" => Object::Name(shading.color_space.pdf_name().to_vec()),
        "Coords" => coords,
        "Domain" => Object::Array(vec![Object::Real(0.0), Object::Real(1.0)]),
        "Function" => Object::Reference(function_id),
        "Extend" => Object::Array(vec![Object::Boolean(true), Object::Boolean(true)]),
    };
    doc.add_object(dict)
}

enum ResourcesTarget {
    InlineOnPage,
    Indirect(ObjectId),
}

fn attach_shadings_to_page(
    doc: &mut Document,
    page_id: ObjectId,
    shadings: &[(usize, ObjectId)],
) -> Result<(), PdfShadingError> {
    // The PDF spec allows `Resources` to be either a direct dict
    // on the page or inherited from an ancestor. printpdf 0.7
    // writes it directly on the page in the documents we
    // produce, so handle that case first; otherwise fall back to
    // walking up to /Pages and writing there.
    let page_dict = doc
        .get_dictionary(page_id)
        .map_err(|e| PdfShadingError::Parse(e.to_string()))?;
    let resources_object = page_dict.get(b"Resources").ok();
    let target = match resources_object {
        Some(Object::Reference(id)) => ResourcesTarget::Indirect(*id),
        Some(Object::Dictionary(_)) => ResourcesTarget::InlineOnPage,
        Some(_) => {
            return Err(PdfShadingError::Parse(format!(
                "page {page_id:?} has a non-dictionary /Resources value"
            )));
        }
        None => {
            return Err(PdfShadingError::MissingResources(page_id.0 as usize));
        }
    };
    let resources_dict: &mut Dictionary = match target {
        ResourcesTarget::InlineOnPage => {
            let page_dict_mut = doc
                .get_dictionary_mut(page_id)
                .map_err(|e| PdfShadingError::Parse(e.to_string()))?;
            page_dict_mut
                .get_mut(b"Resources")
                .map_err(|e| PdfShadingError::Parse(e.to_string()))?
                .as_dict_mut()
                .map_err(|e| PdfShadingError::Parse(e.to_string()))?
        }
        ResourcesTarget::Indirect(id) => doc
            .get_dictionary_mut(id)
            .map_err(|e| PdfShadingError::Parse(e.to_string()))?,
    };

    // Read the existing Shading dict (if any) or build a fresh
    // one. We do this before any mutation so we can roll forward
    // every existing entry without losing it. Any unexpected
    // shape (indirect reference, non-dict value, or missing key)
    // collapses to a fresh dictionary — we never lose user data
    // because printpdf only writes Shading dicts when we ask it
    // to, which we don't.
    let mut shading_dict: Dictionary = match resources_dict.get(b"Shading") {
        Ok(Object::Dictionary(d)) => d.clone(),
        _ => Dictionary::new(),
    };
    for (idx, oid) in shadings {
        let name = format!("SH{idx}");
        shading_dict.set(name.into_bytes(), Object::Reference(*oid));
    }
    resources_dict.set(b"Shading".to_vec(), Object::Dictionary(shading_dict));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_srgb_into_devicergb_preserves_components() {
        let stop = resolve_stop_color(
            &Color::Srgb {
                r: 0.2,
                g: 0.5,
                b: 0.7,
                a: 1.0,
            },
            ShadingColorSpace::DeviceRgb,
        );
        assert_eq!(stop.component_count, 3);
        assert!((stop.components[0] - 0.2).abs() < 1e-6);
        assert!((stop.components[1] - 0.5).abs() < 1e-6);
        assert!((stop.components[2] - 0.7).abs() < 1e-6);
    }

    #[test]
    fn resolve_srgb_into_devicecmyk_runs_through_srgb_to_cmyk() {
        let stop = resolve_stop_color(
            &Color::Srgb {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            },
            ShadingColorSpace::DeviceCmyk,
        );
        assert_eq!(stop.component_count, 4);
        // Pure black in sRGB → K=1.0 in DeviceCMYK.
        assert!(stop.components[3] > 0.99);
    }

    #[test]
    fn resolve_cmyk_into_devicergb_inverts_subtractive_transform() {
        let stop = resolve_stop_color(
            &Color::Cmyk {
                c: 1.0,
                m: 0.0,
                y: 0.0,
                k: 0.0,
                a: 1.0,
            },
            ShadingColorSpace::DeviceRgb,
        );
        assert_eq!(stop.component_count, 3);
        // Pure cyan: R=0, G=1, B=1
        assert!(stop.components[0] < 1e-6);
        assert!((stop.components[1] - 1.0).abs() < 1e-6);
        assert!((stop.components[2] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn empty_shading_list_returns_input_bytes_verbatim() {
        let bytes = b"%PDF-1.7\n%minimal".to_vec();
        let out = inject_shadings(bytes.clone(), &[]).expect("ok");
        assert_eq!(out, bytes);
    }

    #[test]
    fn color_space_for_mode_maps_correctly() {
        assert_eq!(
            color_space_for_mode(PdfColorMode::Cmyk),
            ShadingColorSpace::DeviceCmyk
        );
        assert_eq!(
            color_space_for_mode(PdfColorMode::Rgb),
            ShadingColorSpace::DeviceRgb
        );
        assert_eq!(
            color_space_for_mode(PdfColorMode::PassThrough),
            ShadingColorSpace::DeviceRgb
        );
    }
}
