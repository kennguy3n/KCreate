//! Compose a complete, multi-type SVG for a single page subtree.
//!
//! Where [`crate::svg::export_svg_from_document`] only emits
//! [`NodeType::VectorLayer`] paths (it is the original vector-only
//! exporter from Phase 0), this module walks an entire
//! Page / Artboard subtree and emits every leaf type the document
//! graph supports:
//!
//! * `VectorLayer` → `<path d="…" fill="…" stroke="…"/>`
//! * `RasterLayer` → `<image x= y= width= height= xlink:href="data:image/...;base64,…"/>`
//! * `TextLayer`   → `<text x= y= font-family= font-size= fill=…>…</text>`
//!
//! The caller supplies a `resolve_blob` closure that maps the
//! [`RasterImageMeta::blob_hash`] to raw bytes. We keep blob storage
//! out of this crate (the dep graph forbids
//! `kcreate_export` → `kcreate_storage`), and let the bridge layer
//! plug `ws.store.blobs().load(hash).ok()` in instead.
//!
//! The resulting SVG is fed into [`crate::pdf_multi::export_pdf_multi_pages`]
//! by the bridge's `collect_page_svgs` helper. Without this module,
//! that path silently drops every raster and text layer from the
//! final PDF — pages of pure photos would render blank.

use std::fmt::Write as _;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use kcreate_core::document::DocumentGraph;
use kcreate_core::node::{FillStyle, Node, NodeType, RgbaColor};
use kcreate_vector::{PathSegment, VectorPath};
use uuid::Uuid;

use crate::scene_metadata::{raster_image_meta, text_layer_meta, VECTOR_PATH_METADATA_KEY};

/// Compose a complete `<svg>` document for `page_id`'s subtree.
///
/// `width` / `height` set the `viewBox` and `width` / `height`
/// attributes. Coordinates inside the SVG match the world-space
/// coordinates already stored on the leaf nodes (matching the
/// existing vector-only exporter's contract — paths use whatever
/// coordinate system they were stored in, which for the Phase 10
/// editor is "page-local with page origin at (0,0)").
///
/// `resolve_blob` is called for every `RasterLayer` descendant; it
/// must return the raw bytes of the blob keyed by
/// [`RasterImageMeta::blob_hash`]. Blobs may be encoded
/// (PNG/JPEG/WebP — auto-detected via magic bytes) or raw RGBA8
/// (which is re-encoded to PNG so the data URI's MIME is honest).
/// A return of `None` skips that raster layer rather than aborting
/// the whole page.
///
/// Always returns a syntactically valid SVG. Pages with no
/// renderable descendants produce an empty `<svg>…</svg>` (the
/// PDF rasteriser then emits a blank page, matching the previous
/// behaviour for empty pages).
pub fn compose_page_svg<F>(
    document: &DocumentGraph,
    page_id: Uuid,
    width: f64,
    height: f64,
    resolve_blob: F,
) -> String
where
    F: FnMut(&str) -> Option<Vec<u8>>,
{
    // The historical contract is "page origin at world (0, 0)". This
    // is the origin-zero special case of `compose_page_svg_in_frame`.
    compose_page_svg_in_frame(document, page_id, 0.0, 0.0, width, height, resolve_blob)
}

/// Compose an SVG for `root_id`'s subtree, cropped to the world-space
/// frame `(origin_x, origin_y, width, height)`.
///
/// This generalises [`compose_page_svg`] for documents whose pages
/// are **tiled** in world space — e.g. a Gamma-style deck where each
/// slide is an `Artboard` laid out left-to-right at increasing world
/// `x`. Leaf nodes store world coordinates, so to render a single
/// tile (artboard) in isolation the `viewBox` must be offset to that
/// tile's world origin; otherwise every tile past the first (world
/// `x > 0`) renders off-canvas and the page comes out blank.
///
/// `compose_page_svg` delegates here with `origin = (0, 0)`, which
/// reproduces its previous output byte-for-byte.
pub fn compose_page_svg_in_frame<F>(
    document: &DocumentGraph,
    root_id: Uuid,
    origin_x: f64,
    origin_y: f64,
    width: f64,
    height: f64,
    mut resolve_blob: F,
) -> String
where
    F: FnMut(&str) -> Option<Vec<u8>>,
{
    let w = width.max(1.0);
    let h = height.max(1.0);
    let ox = trim_float(origin_x);
    let oy = trim_float(origin_y);
    let mut s = String::new();
    let _ = write!(
        s,
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <svg xmlns=\"http://www.w3.org/2000/svg\" \
         xmlns:xlink=\"http://www.w3.org/1999/xlink\" \
         width=\"{w}\" height=\"{h}\" viewBox=\"{ox} {oy} {w} {h}\">"
    );
    // `descendants_of` walks DFS, which matches the bridge's
    // scene-sync emit order — first-encountered-first-drawn, so the
    // resulting SVG `z` order matches what the canvas shows.
    for id in document.descendants_of(root_id) {
        let Some(node) = document.get_node(id) else {
            continue;
        };
        if !node.visible {
            continue;
        }
        match node.node_type {
            NodeType::VectorLayer => write_vector_node(&mut s, node),
            NodeType::RasterLayer => write_raster_node(&mut s, node, &mut resolve_blob),
            NodeType::TextLayer => write_text_node(&mut s, node),
            // Containers contribute nothing themselves — their
            // children are walked separately by `descendants_of`.
            NodeType::Page
            | NodeType::Artboard
            | NodeType::GroupLayer
            | NodeType::ComponentLayer
            | NodeType::LayoutFrame => {}
        }
    }
    s.push_str("</svg>");
    s
}

fn write_vector_node(out: &mut String, node: &Node) {
    let Some(raw) = node.metadata.get(VECTOR_PATH_METADATA_KEY) else {
        return;
    };
    let Ok(path) = serde_json::from_value::<VectorPath>(raw.clone()) else {
        return;
    };
    if path.is_empty() {
        return;
    }
    out.push_str("<path d=\"");
    write_path_d(out, &path);
    out.push('"');
    // Apply the node's fill / stroke. Vector layers default to an
    // SVG-style implicit black fill if `FillStyle::None`, so emit
    // `fill="none"` explicitly to match what the canvas would show.
    match &node.style.fill {
        FillStyle::Solid(rgba) => {
            let _ = write!(out, " fill=\"{}\"", svg_color(*rgba));
            if rgba.a < 1.0 {
                let _ = write!(out, " fill-opacity=\"{}\"", trim_float(f64::from(rgba.a)));
            }
        }
        // Gradients require a `<defs><linearGradient/></defs>`
        // block; out of scope here and falls back to no fill so the
        // path is still visible in stroke form. `FillStyle::None`
        // shares the same SVG output (`fill="none"`).
        FillStyle::None | FillStyle::Gradient(_) => out.push_str(" fill=\"none\""),
    }
    if let Some(stroke) = &node.style.stroke {
        let _ = write!(
            out,
            " stroke=\"{}\" stroke-width=\"{}\"",
            svg_color(stroke.color),
            trim_float(stroke.width)
        );
        if stroke.color.a < 1.0 {
            let _ = write!(
                out,
                " stroke-opacity=\"{}\"",
                trim_float(f64::from(stroke.color.a))
            );
        }
    }
    if matches!(path.fill_rule, kcreate_vector::FillRule::EvenOdd) {
        out.push_str(" fill-rule=\"evenodd\"");
    }
    out.push_str("/>");
}

fn write_raster_node<F>(out: &mut String, node: &Node, resolve_blob: &mut F)
where
    F: FnMut(&str) -> Option<Vec<u8>>,
{
    let Some(meta) = raster_image_meta(node) else {
        return;
    };
    let Some(bytes) = resolve_blob(&meta.blob_hash) else {
        return;
    };
    let Some(data_url) = bytes_to_data_url(&bytes, meta.width, meta.height) else {
        return;
    };
    let world_x = node.bounds.x + node.transform.tx;
    let world_y = node.bounds.y + node.transform.ty;
    let w = node.bounds.width.max(0.0);
    let h = node.bounds.height.max(0.0);
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    let _ = write!(
        out,
        "<image x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" \
         preserveAspectRatio=\"none\" xlink:href=\"{}\"/>",
        trim_float(world_x),
        trim_float(world_y),
        trim_float(w),
        trim_float(h),
        data_url,
    );
}

fn write_text_node(out: &mut String, node: &Node) {
    let Some(meta) = text_layer_meta(node) else {
        return;
    };
    let world_x = node.bounds.x + node.transform.tx;
    let world_y = node.bounds.y + node.transform.ty;
    // SVG `<text>` positions the baseline at (x, y). The node's
    // bounds describe the top-left, so we shift the baseline down by
    // the font size to match what the canvas shows (the renderer's
    // text shaping treats the bounds top-edge as the ascender line).
    let baseline_y = world_y + f64::from(meta.font_size);
    let fill = match &node.style.fill {
        FillStyle::Solid(c) => svg_color(*c),
        // Default text colour matches the renderer's fallback.
        FillStyle::None | FillStyle::Gradient(_) => "#000000".to_string(),
    };
    let _ = write!(
        out,
        "<text x=\"{}\" y=\"{}\" font-family=\"{}\" font-size=\"{}\" fill=\"{}\">{}</text>",
        trim_float(world_x),
        trim_float(baseline_y),
        xml_escape(&meta.font_family),
        trim_float(f64::from(meta.font_size)),
        fill,
        xml_escape(&meta.text),
    );
}

fn write_path_d(out: &mut String, path: &VectorPath) {
    let mut first = true;
    for cmd in &path.commands {
        if !first {
            out.push(' ');
        }
        first = false;
        match *cmd {
            PathSegment::MoveTo(p) => {
                let _ = write!(out, "M{} {}", trim_float(p.x), trim_float(p.y));
            }
            PathSegment::LineTo(p) => {
                let _ = write!(out, "L{} {}", trim_float(p.x), trim_float(p.y));
            }
            PathSegment::QuadTo { ctrl, end } => {
                let _ = write!(
                    out,
                    "Q{} {} {} {}",
                    trim_float(ctrl.x),
                    trim_float(ctrl.y),
                    trim_float(end.x),
                    trim_float(end.y)
                );
            }
            PathSegment::CubicTo { ctrl1, ctrl2, end } => {
                let _ = write!(
                    out,
                    "C{} {} {} {} {} {}",
                    trim_float(ctrl1.x),
                    trim_float(ctrl1.y),
                    trim_float(ctrl2.x),
                    trim_float(ctrl2.y),
                    trim_float(end.x),
                    trim_float(end.y)
                );
            }
            PathSegment::Close => out.push('Z'),
        }
    }
}

/// Convert raw blob bytes to an `data:` URL suitable for an SVG
/// `<image xlink:href=>` attribute.
///
/// Strategy:
/// 1. If the bytes start with a recognised image magic (PNG / JPEG
///    / WebP), passthrough the bytes with the matching MIME — avoids
///    a decode + re-encode round-trip that would degrade JPEG.
/// 2. If the byte length equals `width * height * 4`, treat as raw
///    RGBA8 (the Phase 10 raster pipeline stores tile data this way)
///    and encode as PNG.
/// 3. As a last resort, try [`image::load_from_memory`] and re-encode
///    as PNG so unknown-but-decodable formats still appear in the PDF.
///
/// Returns `None` when nothing works — the caller skips the layer
/// rather than emitting a broken `<image>` element.
fn bytes_to_data_url(bytes: &[u8], width: u32, height: u32) -> Option<String> {
    if let Some(mime) = detect_image_mime(bytes) {
        let b64 = STANDARD.encode(bytes);
        return Some(format!("data:{mime};base64,{b64}"));
    }
    let expected = (width as usize)
        .saturating_mul(height as usize)
        .saturating_mul(4);
    if !bytes.is_empty() && bytes.len() == expected {
        if let Some(img) = image::RgbaImage::from_raw(width, height, bytes.to_vec()) {
            let mut png_bytes: Vec<u8> = Vec::new();
            let mut cursor = std::io::Cursor::new(&mut png_bytes);
            if image::DynamicImage::ImageRgba8(img)
                .write_to(&mut cursor, image::ImageFormat::Png)
                .is_ok()
            {
                let b64 = STANDARD.encode(&png_bytes);
                return Some(format!("data:image/png;base64,{b64}"));
            }
        }
    }
    if let Ok(img) = image::load_from_memory(bytes) {
        let mut png_bytes: Vec<u8> = Vec::new();
        let mut cursor = std::io::Cursor::new(&mut png_bytes);
        if img.write_to(&mut cursor, image::ImageFormat::Png).is_ok() {
            let b64 = STANDARD.encode(&png_bytes);
            return Some(format!("data:image/png;base64,{b64}"));
        }
    }
    None
}

fn detect_image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.len() >= 8 && bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Some("image/png");
    }
    if bytes.len() >= 3 && bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("image/jpeg");
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    None
}

fn svg_color(c: RgbaColor) -> String {
    let r = (c.r.clamp(0.0, 1.0) * 255.0).round() as u8;
    let g = (c.g.clamp(0.0, 1.0) * 255.0).round() as u8;
    let b = (c.b.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{r:02x}{g:02x}{b:02x}")
}

fn trim_float(v: f64) -> String {
    if !v.is_finite() {
        return "0".to_string();
    }
    let s = format!("{v:.4}");
    if s.contains('.') {
        let s = s.trim_end_matches('0').trim_end_matches('.');
        if s.is_empty() || s == "-" {
            "0".to_string()
        } else {
            s.to_string()
        }
    } else {
        s
    }
}

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use kcreate_core::node::{Bounds, Node, NodeType, RgbaColor};
    use kcreate_vector::{PathPoint, PathSegment, VectorPath};

    fn vector_node(name: &str, x: f64, y: f64, w: f64, h: f64) -> Node {
        let mut node = Node::new(NodeType::VectorLayer, name);
        node.bounds = Bounds {
            x,
            y,
            width: w,
            height: h,
        };
        let path = VectorPath::new(vec![
            PathSegment::MoveTo(PathPoint { x, y }),
            PathSegment::LineTo(PathPoint { x: x + w, y }),
            PathSegment::LineTo(PathPoint { x: x + w, y: y + h }),
            PathSegment::LineTo(PathPoint { x, y: y + h }),
            PathSegment::Close,
        ]);
        node.style.fill = FillStyle::Solid(RgbaColor {
            r: 1.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        });
        node.metadata.insert(
            VECTOR_PATH_METADATA_KEY.to_string(),
            serde_json::to_value(&path).unwrap(),
        );
        node
    }

    #[allow(clippy::too_many_arguments)]
    fn raster_node(
        name: &str,
        hash: &str,
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        pw: u32,
        ph: u32,
    ) -> Node {
        let mut node = Node::new(NodeType::RasterLayer, name);
        node.bounds = Bounds {
            x,
            y,
            width: w,
            height: h,
        };
        let meta = crate::scene_metadata::RasterImageMeta {
            blob_hash: hash.to_string(),
            width: pw,
            height: ph,
        };
        node.metadata.insert(
            crate::scene_metadata::RASTER_IMAGE_METADATA_KEY.to_string(),
            serde_json::to_value(&meta).unwrap(),
        );
        node
    }

    fn text_node(name: &str, x: f64, y: f64, w: f64, h: f64, body: &str) -> Node {
        let mut node = Node::new(NodeType::TextLayer, name);
        node.bounds = Bounds {
            x,
            y,
            width: w,
            height: h,
        };
        let meta = crate::scene_metadata::TextLayerMeta {
            text: body.to_string(),
            font_family: "Inter".to_string(),
            font_size: 24.0,
        };
        node.metadata.insert(
            crate::scene_metadata::TEXT_LAYER_METADATA_KEY.to_string(),
            serde_json::to_value(&meta).unwrap(),
        );
        node
    }

    fn build_page(children: Vec<Node>) -> (DocumentGraph, Uuid) {
        let mut doc = DocumentGraph::default();
        let page = doc
            .insert_node(Node::new(NodeType::Page, "Page 1"))
            .expect("insert page");
        for child in children {
            let mut c = child;
            c.parent_id = Some(page);
            doc.insert_node(c).expect("insert child");
        }
        (doc, page)
    }

    #[test]
    fn vector_only_page_emits_path() {
        let (doc, page) = build_page(vec![vector_node("v", 0.0, 0.0, 100.0, 100.0)]);
        let svg = compose_page_svg(&doc, page, 100.0, 100.0, |_| None);
        assert!(svg.contains("<svg"));
        assert!(svg.contains("<path"));
        assert!(svg.contains("fill=\"#ff0000\""));
        assert!(!svg.contains("<image"));
        assert!(!svg.contains("<text"));
    }

    #[test]
    fn compose_page_svg_keeps_zero_origin_viewbox() {
        // Backward-compat: the page-origin-at-(0,0) helper must keep
        // its historical zero-origin viewBox byte-for-byte.
        let (doc, page) = build_page(vec![vector_node("v", 0.0, 0.0, 100.0, 100.0)]);
        let svg = compose_page_svg(&doc, page, 100.0, 100.0, |_| None);
        assert!(
            svg.contains("viewBox=\"0 0 100 100\""),
            "default compose must keep a zero-origin viewBox; got: {svg}"
        );
    }

    #[test]
    fn compose_in_frame_offsets_viewbox_to_world_origin() {
        // Regression guard for the Gamma-deck tiling bug: a slide whose
        // world origin is (2020, 0) must render with a viewBox offset
        // to that origin. With the old hard-coded `0 0 w h` viewBox,
        // every tile past the first (world x > 0) fell off-canvas and
        // the PDF page came out blank.
        let (doc, page) = build_page(vec![
            vector_node("bg", 2020.0, 0.0, 1920.0, 1080.0),
            text_node("title", 2120.0, 120.0, 800.0, 80.0, "Second Slide"),
        ]);
        let svg = compose_page_svg_in_frame(&doc, page, 2020.0, 0.0, 1920.0, 1080.0, |_| None);
        assert!(
            svg.contains("viewBox=\"2020 0 1920 1080\""),
            "frame viewBox must be offset to the tile's world origin; got: {svg}"
        );
        // The off-origin content must survive (the bug dropped it).
        assert!(
            svg.contains(">Second Slide</text>"),
            "text lost; got: {svg}"
        );
        assert!(svg.contains("<path"), "background lost; got: {svg}");
    }

    #[test]
    fn raster_only_page_emits_image_with_data_uri() {
        // Make a real PNG blob so detect_image_mime hits the PNG arm.
        let img = image::RgbaImage::from_pixel(2, 2, image::Rgba([0, 0, 255, 255]));
        let mut png_bytes = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(
                &mut std::io::Cursor::new(&mut png_bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        let (doc, page) = build_page(vec![raster_node("r", "h", 10.0, 20.0, 200.0, 100.0, 2, 2)]);
        let svg = compose_page_svg(&doc, page, 300.0, 200.0, |hash| {
            assert_eq!(hash, "h");
            Some(png_bytes.clone())
        });
        assert!(
            svg.contains("<image"),
            "raster layer must produce an <image> element; got: {svg}"
        );
        assert!(
            svg.contains("data:image/png;base64,"),
            "PNG bytes must passthrough as image/png data URI; got: {svg}"
        );
        assert!(svg.contains("x=\"10\""));
        assert!(svg.contains("y=\"20\""));
        assert!(svg.contains("width=\"200\""));
        assert!(svg.contains("height=\"100\""));
    }

    #[test]
    fn raw_rgba_blob_is_reencoded_as_png() {
        // 2x2 raw RGBA = 16 bytes — exercises the
        // `len == w * h * 4` re-encode branch (PSD imports drop raw
        // tile buffers here).
        let raw_rgba = vec![255u8; 16];
        let (doc, page) = build_page(vec![raster_node("r", "h", 0.0, 0.0, 200.0, 200.0, 2, 2)]);
        let svg = compose_page_svg(&doc, page, 200.0, 200.0, |_| Some(raw_rgba.clone()));
        assert!(
            svg.contains("data:image/png;base64,"),
            "raw RGBA must round-trip through PNG re-encode; got: {svg}"
        );
    }

    #[test]
    fn text_only_page_emits_text_element() {
        let (doc, page) = build_page(vec![text_node("t", 5.0, 6.0, 80.0, 40.0, "Hello")]);
        let svg = compose_page_svg(&doc, page, 100.0, 100.0, |_| None);
        assert!(svg.contains("<text"));
        // Baseline should be top + font_size = 6 + 24 = 30.
        assert!(svg.contains("y=\"30\""), "baseline shifted; got: {svg}");
        assert!(svg.contains(">Hello</text>"));
        assert!(svg.contains("font-family=\"Inter\""));
        assert!(svg.contains("font-size=\"24\""));
    }

    #[test]
    fn xml_special_chars_in_text_are_escaped() {
        let (doc, page) = build_page(vec![text_node("t", 0.0, 0.0, 80.0, 40.0, "a < b & \"c\"")]);
        let svg = compose_page_svg(&doc, page, 100.0, 100.0, |_| None);
        assert!(svg.contains("a &lt; b &amp; &quot;c&quot;"));
        // The raw `<` must NOT appear inside the text node body.
        // (We split on `<text` so we don't trip on the opening tag.)
        let (_, after_text_open) = svg.split_once("<text").unwrap();
        let (_, text_body) = after_text_open.split_once('>').unwrap();
        let text_body = text_body.split("</text>").next().unwrap();
        assert!(
            !text_body.contains('<'),
            "raw '<' leaked into text body: {text_body}"
        );
        assert!(!text_body.contains('&') || text_body.contains("&amp;"));
    }

    #[test]
    fn mixed_page_emits_all_three_node_types() {
        // The regression for the round-11 finding: a page that mixes
        // vector + raster + text must not silently drop the
        // non-vector descendants from the SVG fed to the PDF
        // rasteriser.
        let img = image::RgbaImage::from_pixel(1, 1, image::Rgba([0, 255, 0, 255]));
        let mut png_bytes = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(
                &mut std::io::Cursor::new(&mut png_bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        let (doc, page) = build_page(vec![
            vector_node("v", 0.0, 0.0, 50.0, 50.0),
            raster_node("r", "h", 60.0, 0.0, 100.0, 50.0, 1, 1),
            text_node("t", 0.0, 60.0, 100.0, 20.0, "Hi"),
        ]);
        let svg = compose_page_svg(&doc, page, 200.0, 100.0, |_| Some(png_bytes.clone()));
        assert!(svg.contains("<path"));
        assert!(svg.contains("<image"));
        assert!(svg.contains("<text"));
    }

    #[test]
    fn invisible_descendants_are_skipped() {
        let mut v = vector_node("v", 0.0, 0.0, 10.0, 10.0);
        v.visible = false;
        let (doc, page) = build_page(vec![v]);
        let svg = compose_page_svg(&doc, page, 100.0, 100.0, |_| None);
        assert!(!svg.contains("<path"));
    }

    #[test]
    fn resvg_accepts_composed_svg_with_image_and_text() {
        // End-to-end check: the resvg parser used by `pdf_multi`
        // accepts the SVG we emit. If we mangle the data URI, omit a
        // namespace, or emit malformed XML, resvg would error and
        // the PDF page would render blank again.
        let img = image::RgbaImage::from_pixel(4, 4, image::Rgba([200, 200, 200, 255]));
        let mut png_bytes = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(
                &mut std::io::Cursor::new(&mut png_bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        let (doc, page) = build_page(vec![
            vector_node("v", 0.0, 0.0, 50.0, 50.0),
            raster_node("r", "h", 0.0, 0.0, 100.0, 100.0, 4, 4),
            text_node("t", 10.0, 10.0, 100.0, 20.0, "ok"),
        ]);
        let svg = compose_page_svg(&doc, page, 200.0, 200.0, |_| Some(png_bytes.clone()));
        let _tree = resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default())
            .expect("resvg must parse the composed SVG");
    }

    #[test]
    fn unknown_blob_skips_image_without_erroring() {
        // Bytes that look like nothing image::load_from_memory can
        // decode. We must NOT emit a broken `<image>` element.
        let (doc, page) = build_page(vec![raster_node(
            "r", "garbage", 0.0, 0.0, 10.0, 10.0,
            // wrong dimensions so the raw-RGBA branch doesn't fire
            999, 999,
        )]);
        let svg = compose_page_svg(&doc, page, 100.0, 100.0, |_| {
            Some(vec![0x00, 0x01, 0x02, 0x03])
        });
        assert!(!svg.contains("<image"));
    }
}
