//! Figma JSON import (Phase 6 — Tasks 19–20).
//!
//! Reads a Figma "document JSON" file — the shape produced by the
//! Figma REST API endpoint `GET /v1/files/:key` and by the official
//! `figma export-json` plugin — and produces a structured
//! [`ImportedFigma`] tree the bridge layer projects onto KCreate's
//! own node graph.
//!
//! # What is imported
//!
//! 1. **Document → project**. The top-level node (`type = "DOCUMENT"`,
//!    or a bare object missing a type when the user exported a
//!    fragment) yields the project name.
//! 2. **CANVAS → Page**. Each Figma canvas becomes one KCreate
//!    `Page`. Canvas names map straight across.
//! 3. **FRAME / COMPONENT / INSTANCE → Artboard**. The first-level
//!    children of each canvas become artboards sized to their
//!    `absoluteBoundingBox`. Nested frames inside frames are flattened
//!    to vector groups — the bridge can recurse if needed, but the
//!    importer reports them as one artboard's children.
//! 4. **VECTOR / RECTANGLE / ELLIPSE / LINE / REGULAR_POLYGON / STAR
//!    → VectorPath**. The Figma `fillGeometry` array gives one or
//!    more SVG `path` `d`-strings per node; we use the first one. If
//!    `fillGeometry` is missing we synthesise a `d` from the bounding
//!    box (rectangle) so the user still sees the shape.
//! 5. **TEXT → TextLayer**. Plain `characters` content + font family
//!    + size pulled from `style.fontFamily` / `style.fontSize`.
//!
//!    Mixed runs (Figma "characterStyleOverrides") are not preserved;
//!    the importer keeps the dominant style and emits a warning.
//! 6. **IMAGE fills**. Rectangles whose first fill is `type =
//!    "IMAGE"` are imported as `RasterLayer`s. The pixel bytes
//!    themselves are *not* in the JSON; the importer records the
//!    Figma `imageRef` so the bridge can later cross-reference it
//!    against an image bundle the user exported alongside.
//!
//! # What is *not* imported (yet)
//!
//! - Auto-layout constraints (Figma "layoutMode"). Frames come in
//!   with positions baked from `absoluteBoundingBox` — the user must
//!   re-enable auto-layout manually in KCreate's Layout Studio.
//! - Component variants / props. Components arrive as plain
//!   artboards; instance overrides are not re-applied.
//! - Effects (drop shadow, inner shadow, blur) beyond fill colors.
//! - Gradient fills more complex than linear; the importer reads the
//!   first gradient stop's color as a solid fill and emits a
//!   warning.
//!
//! All "not yet" cases produce a [`FigmaImportWarning`] so the
//! renderer can show "12 frames imported, 4 nested frames flattened"
//! rather than silently losing structure.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Top-level result of importing a Figma JSON file. One entry per
/// canvas (Figma's word for "page") in source order, plus document
/// metadata and any non-fatal warnings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedFigma {
    /// `name` from the document root, when the export carried one.
    pub document_name: Option<String>,
    /// One entry per Figma canvas in source order.
    pub pages: Vec<ImportedFigmaPage>,
    /// Non-fatal observations — content the importer chose to keep
    /// but had to simplify, or content it dropped.
    pub warnings: Vec<FigmaImportWarning>,
}

/// One canvas → one KCreate `Page`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedFigmaPage {
    /// Canvas `name`. Used directly as the KCreate page name.
    pub name: String,
    /// The top-level frames inside the canvas — each becomes one
    /// KCreate artboard. Loose canvas children (a vector dropped
    /// directly on the canvas) appear as a single anonymous
    /// "Canvas content" artboard.
    pub artboards: Vec<ImportedFigmaArtboard>,
}

/// One Figma frame / component / instance → one KCreate artboard.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedFigmaArtboard {
    pub name: String,
    pub bounds: ImportedBounds,
    pub children: Vec<ImportedFigmaNode>,
}

/// A KCreate-friendly bounding box in CSS pixels (Figma's native
/// unit). The bridge layer can pass these straight to [`kcreate_core::
/// Bounds::new`].
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedBounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Per-node payload the bridge maps onto KCreate's node graph.
///
/// Variants are kept narrow on purpose: the bridge already knows
/// how to consume `VectorPath` / `TextLayer` / `RasterLayer` shapes,
/// so the importer focuses on extracting clean payloads rather than
/// modeling every Figma node type.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum ImportedFigmaNode {
    /// A vector / rectangle / ellipse / star / etc.
    Vector {
        name: String,
        bounds: ImportedBounds,
        /// SVG `d` attribute. Always populated — when Figma does not
        /// provide `fillGeometry` (e.g. for plain rectangles) the
        /// importer synthesises one from the bounding box.
        path_d: String,
        /// First solid fill color (sRGB, 8-bit per channel) when the
        /// node has one. Gradient / image fills set this to `None`
        /// and emit a warning.
        fill_rgba: Option<[u8; 4]>,
    },
    /// A `TEXT` node. `characters` is the plain UTF-8 content.
    Text {
        name: String,
        bounds: ImportedBounds,
        characters: String,
        font_family: Option<String>,
        font_size_px: Option<f64>,
        /// Text color (sRGB, 8-bit per channel) — first fill on the
        /// dominant style.
        color_rgba: Option<[u8; 4]>,
    },
    /// A node whose first fill is `type = "IMAGE"`. The pixel bytes
    /// are *not* in the Figma JSON; the importer records the Figma
    /// `imageRef` so the bridge can later cross-reference it against
    /// a sidecar bundle.
    Image {
        name: String,
        bounds: ImportedBounds,
        /// Figma's `imageRef` hash. The bridge looks this up in the
        /// optional `<figma-file>.images/` sidecar.
        image_ref: String,
    },
}

/// Non-fatal observations emitted while importing.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum FigmaImportWarning {
    /// A nested frame was flattened into its parent artboard's
    /// children — KCreate doesn't model frames recursively inside
    /// artboards in the first cut.
    FlattenedNestedFrame { artboard_name: String },
    /// A gradient fill was simplified to its first stop's color.
    SimplifiedGradient { node_name: String },
    /// A node had mixed character-style overrides; only the
    /// dominant style was preserved.
    DroppedMixedTextStyles { node_name: String },
    /// A node had no recognisable geometry (no
    /// `absoluteBoundingBox`, no `fillGeometry`); it was dropped.
    DroppedShapeless { node_name: String },
    /// A node type the importer doesn't handle.
    UnsupportedNodeType {
        node_name: String,
        node_type: String,
    },
}

/// Hard failures from the Figma import pipeline.
#[derive(Debug, Error)]
pub enum FigmaImportError {
    /// The input file couldn't be read.
    #[error("failed to read Figma JSON {path}: {source}")]
    Open {
        path: String,
        #[source]
        source: std::io::Error,
    },
    /// The input is not valid JSON.
    #[error("failed to parse Figma JSON {path}: {source}")]
    Parse {
        path: String,
        #[source]
        source: serde_json::Error,
    },
    /// The JSON parsed but doesn't look like a Figma document.
    #[error("Figma JSON {path} is missing a recognisable document root")]
    Shape { path: String },
}

/// Import a Figma JSON file.
///
/// # Errors
/// Returns [`FigmaImportError::Open`] when the file can't be read,
/// [`FigmaImportError::Parse`] when the file is not valid JSON, and
/// [`FigmaImportError::Shape`] when the JSON parses but doesn't
/// contain a recognisable Figma document tree.
pub fn import_figma<P: AsRef<Path>>(path: P) -> Result<ImportedFigma, FigmaImportError> {
    let path = path.as_ref();
    let path_str = path.display().to_string();
    let bytes = fs::read(path).map_err(|source| FigmaImportError::Open {
        path: path_str.clone(),
        source,
    })?;
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|source| FigmaImportError::Parse {
            path: path_str.clone(),
            source,
        })?;
    parse_figma_value(&value).ok_or(FigmaImportError::Shape { path: path_str })
}

/// Parse the JSON value into the structured result. Public so unit
/// tests can drive the parser without round-tripping through the
/// filesystem.
#[must_use]
pub fn parse_figma_value(value: &serde_json::Value) -> Option<ImportedFigma> {
    // The Figma REST API wraps the document tree under `"document":
    // {...}` at the top level. The plugin exporter dumps the tree
    // directly. Accept either.
    let root = value.get("document").unwrap_or(value);
    let root_obj = root.as_object()?;

    // We don't strictly require `type == "DOCUMENT"` because some
    // exports start at a CANVAS, but we need *something* with
    // children for an importable file.
    let document_name = root_obj
        .get("name")
        .and_then(|n| n.as_str())
        .map(str::to_owned);

    let mut warnings = Vec::new();
    let mut pages = Vec::new();

    // Figma documents look like: DOCUMENT -> CANVAS[] -> FRAME[] -> ...
    // If the root is a CANVAS we treat it as a one-page document.
    let canvases: Vec<&serde_json::Value> = match root_obj.get("type").and_then(|t| t.as_str()) {
        Some("CANVAS") => vec![root],
        _ => root_obj
            .get("children")
            .and_then(|c| c.as_array())
            .map(|arr| arr.iter().collect())
            .unwrap_or_default(),
    };

    if canvases.is_empty() {
        return None;
    }

    for canvas in canvases {
        let canvas_obj = match canvas.as_object() {
            Some(o) => o,
            None => continue,
        };
        let name = canvas_obj
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or("Untitled canvas")
            .to_owned();

        let mut artboards = Vec::new();
        let mut loose_children = Vec::new();

        if let Some(children) = canvas_obj.get("children").and_then(|c| c.as_array()) {
            for child in children {
                if is_frame_like(child) {
                    artboards.push(parse_frame(child, &mut warnings));
                } else {
                    // Loose children dropped directly on the canvas
                    // (a sticker, a vector with no enclosing frame).
                    if let Some(node) = parse_leaf_node(child, &mut warnings) {
                        loose_children.push(node);
                    }
                }
            }
        }

        if !loose_children.is_empty() {
            let bounds = encompassing_bounds(&loose_children).unwrap_or(ImportedBounds {
                x: 0.0,
                y: 0.0,
                width: 1024.0,
                height: 768.0,
            });
            artboards.push(ImportedFigmaArtboard {
                name: "Canvas content".to_string(),
                bounds,
                children: loose_children,
            });
        }

        pages.push(ImportedFigmaPage { name, artboards });
    }

    Some(ImportedFigma {
        document_name,
        pages,
        warnings,
    })
}

/// Frame-like nodes become artboards. The catch-all `GROUP` is here
/// because designers routinely use plain groups as artboard
/// substitutes in personal-style libraries.
fn is_frame_like(node: &serde_json::Value) -> bool {
    matches!(
        node.get("type").and_then(|t| t.as_str()),
        Some("FRAME" | "COMPONENT" | "INSTANCE" | "GROUP" | "COMPONENT_SET")
    )
}

fn parse_frame(
    value: &serde_json::Value,
    warnings: &mut Vec<FigmaImportWarning>,
) -> ImportedFigmaArtboard {
    let obj = value.as_object();
    let name = obj
        .and_then(|o| o.get("name"))
        .and_then(|n| n.as_str())
        .unwrap_or("Frame")
        .to_owned();
    let bounds = parse_bounds(value).unwrap_or(ImportedBounds {
        x: 0.0,
        y: 0.0,
        width: 1024.0,
        height: 768.0,
    });

    let mut children = Vec::new();
    if let Some(arr) = obj
        .and_then(|o| o.get("children"))
        .and_then(|c| c.as_array())
    {
        for child in arr {
            if is_frame_like(child) {
                // Nested frame: flatten its leaf children up into
                // this artboard rather than introducing a recursive
                // structure the bridge doesn't model yet.
                warnings.push(FigmaImportWarning::FlattenedNestedFrame {
                    artboard_name: name.clone(),
                });
                flatten_into(child, warnings, &mut children);
            } else if let Some(node) = parse_leaf_node(child, warnings) {
                children.push(node);
            }
        }
    }

    ImportedFigmaArtboard {
        name,
        bounds,
        children,
    }
}

/// Recursively flatten a frame-like node's descendants into
/// `output`. Each nested frame's bounds are dropped; only the leaf
/// vectors / text / images survive.
fn flatten_into(
    value: &serde_json::Value,
    warnings: &mut Vec<FigmaImportWarning>,
    output: &mut Vec<ImportedFigmaNode>,
) {
    let Some(obj) = value.as_object() else {
        return;
    };
    let Some(children) = obj.get("children").and_then(|c| c.as_array()) else {
        return;
    };
    for child in children {
        if is_frame_like(child) {
            flatten_into(child, warnings, output);
        } else if let Some(node) = parse_leaf_node(child, warnings) {
            output.push(node);
        }
    }
}

fn parse_leaf_node(
    value: &serde_json::Value,
    warnings: &mut Vec<FigmaImportWarning>,
) -> Option<ImportedFigmaNode> {
    let obj = value.as_object()?;
    let name = obj
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or("Untitled")
        .to_owned();
    let node_type = obj.get("type").and_then(|t| t.as_str()).unwrap_or("");
    let bounds = match parse_bounds(value) {
        Some(b) => b,
        None => {
            warnings.push(FigmaImportWarning::DroppedShapeless { node_name: name });
            return None;
        }
    };

    match node_type {
        "TEXT" => Some(parse_text_node(obj, name, bounds, warnings)),
        "VECTOR" | "RECTANGLE" | "ELLIPSE" | "LINE" | "REGULAR_POLYGON" | "STAR"
        | "BOOLEAN_OPERATION" => {
            // First check whether the first fill is an image; if so
            // emit an Image node rather than a Vector — Figma stores
            // photographs as rectangles with an `IMAGE` fill.
            if let Some(image_ref) = first_image_fill_ref(obj) {
                return Some(ImportedFigmaNode::Image {
                    name,
                    bounds,
                    image_ref,
                });
            }
            Some(parse_vector_node(obj, name, bounds, node_type, warnings))
        }
        other => {
            warnings.push(FigmaImportWarning::UnsupportedNodeType {
                node_name: name,
                node_type: other.to_owned(),
            });
            None
        }
    }
}

fn parse_text_node(
    obj: &serde_json::Map<String, serde_json::Value>,
    name: String,
    bounds: ImportedBounds,
    warnings: &mut Vec<FigmaImportWarning>,
) -> ImportedFigmaNode {
    let characters = obj
        .get("characters")
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_owned();

    if obj
        .get("characterStyleOverrides")
        .and_then(|v| v.as_array())
        .is_some_and(|arr| !arr.is_empty())
    {
        warnings.push(FigmaImportWarning::DroppedMixedTextStyles {
            node_name: name.clone(),
        });
    }

    let style = obj.get("style").and_then(|s| s.as_object());
    let font_family = style
        .and_then(|s| s.get("fontFamily"))
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let font_size_px = style
        .and_then(|s| s.get("fontSize"))
        .and_then(serde_json::Value::as_f64);
    let color_rgba = first_solid_fill(obj, warnings, &name);

    ImportedFigmaNode::Text {
        name,
        bounds,
        characters,
        font_family,
        font_size_px,
        color_rgba,
    }
}

fn parse_vector_node(
    obj: &serde_json::Map<String, serde_json::Value>,
    name: String,
    bounds: ImportedBounds,
    node_type: &str,
    warnings: &mut Vec<FigmaImportWarning>,
) -> ImportedFigmaNode {
    let path_d = obj
        .get("fillGeometry")
        .and_then(|g| g.as_array())
        .and_then(|arr| arr.first())
        .and_then(|first| first.get("path"))
        .and_then(|p| p.as_str())
        .map_or_else(|| synthesise_path_d(node_type, &bounds), str::to_owned);
    let fill_rgba = first_solid_fill(obj, warnings, &name);

    ImportedFigmaNode::Vector {
        name,
        bounds,
        path_d,
        fill_rgba,
    }
}

/// Build an SVG `d` attribute from a node's bounding box for the
/// common case where Figma omits `fillGeometry` (rectangles, plain
/// ellipses). The bounding box is in absolute canvas coordinates;
/// we re-origin to (0,0) so the rendered path lives inside the
/// node's local frame.
fn synthesise_path_d(node_type: &str, bounds: &ImportedBounds) -> String {
    let w = bounds.width.max(0.0);
    let h = bounds.height.max(0.0);
    match node_type {
        "ELLIPSE" => {
            let rx = w / 2.0;
            let ry = h / 2.0;
            // Use two `a` arcs to draw a full ellipse — every SVG
            // renderer accepts this form.
            format!("M0 {ry} A{rx} {ry} 0 1 0 {w} {ry} A{rx} {ry} 0 1 0 0 {ry} Z")
        }
        _ => format!("M0 0 L{w} 0 L{w} {h} L0 {h} Z"),
    }
}

fn parse_bounds(value: &serde_json::Value) -> Option<ImportedBounds> {
    let box_ = value.get("absoluteBoundingBox")?;
    let x = box_.get("x").and_then(serde_json::Value::as_f64)?;
    let y = box_.get("y").and_then(serde_json::Value::as_f64)?;
    let width = box_.get("width").and_then(serde_json::Value::as_f64)?;
    let height = box_.get("height").and_then(serde_json::Value::as_f64)?;
    Some(ImportedBounds {
        x,
        y,
        width,
        height,
    })
}

fn first_image_fill_ref(obj: &serde_json::Map<String, serde_json::Value>) -> Option<String> {
    let first = obj.get("fills").and_then(|f| f.as_array())?.first()?;
    let kind = first.get("type").and_then(|t| t.as_str())?;
    if kind != "IMAGE" {
        return None;
    }
    Some(
        first
            .get("imageRef")
            .and_then(|r| r.as_str())
            .unwrap_or("")
            .to_owned(),
    )
}

/// Pull the first SOLID fill color, falling back to the first
/// gradient stop's color (with a `SimplifiedGradient` warning) when
/// the fill chain starts with a gradient.
fn first_solid_fill(
    obj: &serde_json::Map<String, serde_json::Value>,
    warnings: &mut Vec<FigmaImportWarning>,
    node_name: &str,
) -> Option<[u8; 4]> {
    let fills = obj.get("fills").and_then(|f| f.as_array())?;
    let first = fills.first()?;
    let kind = first.get("type").and_then(|t| t.as_str()).unwrap_or("");
    match kind {
        "SOLID" => extract_rgba(first.get("color")?, first.get("opacity")),
        "GRADIENT_LINEAR" | "GRADIENT_RADIAL" | "GRADIENT_ANGULAR" | "GRADIENT_DIAMOND" => {
            warnings.push(FigmaImportWarning::SimplifiedGradient {
                node_name: node_name.to_owned(),
            });
            let stop = first
                .get("gradientStops")
                .and_then(|s| s.as_array())
                .and_then(|arr| arr.first())?;
            extract_rgba(stop.get("color")?, stop.get("opacity"))
        }
        _ => None,
    }
}

/// Convert Figma's `{r, g, b, a}` (each 0..=1 f64) into an 8-bit
/// `[r, g, b, a]` byte tuple. Figma's `a` is the alpha *inside* the
/// color object; the optional outer `opacity` multiplies on top of
/// that — both are clamped before the cast so a malformed export
/// doesn't crash the importer.
fn extract_rgba(
    color: &serde_json::Value,
    outer_opacity: Option<&serde_json::Value>,
) -> Option<[u8; 4]> {
    let r = color.get("r").and_then(serde_json::Value::as_f64)?;
    let g = color.get("g").and_then(serde_json::Value::as_f64)?;
    let b = color.get("b").and_then(serde_json::Value::as_f64)?;
    let a_inner = color
        .get("a")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(1.0);
    let a_outer = outer_opacity
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(1.0);
    let a = (a_inner * a_outer).clamp(0.0, 1.0);
    // All four channels go through `clamp_u8` (which rounds) so that the
    // same source color produces the same byte tuple regardless of which
    // import path is taken — see `sketch_import::parse_sketch_color`,
    // which is the reference implementation. Truncating the alpha (`as
    // u8`) would silently subtract 1 from semi-transparent layers, e.g.
    // a Figma color with `a = 0.999` would yield alpha = 254 instead of
    // 255, accumulating visible drift across stacked transparent fills.
    Some([clamp_u8(r), clamp_u8(g), clamp_u8(b), clamp_u8(a)])
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn clamp_u8(v: f64) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// Build a bounding box that covers every leaf child — used when
/// the canvas has loose children with no enclosing frame.
fn encompassing_bounds(nodes: &[ImportedFigmaNode]) -> Option<ImportedBounds> {
    let mut iter = nodes.iter().map(|n| match n {
        ImportedFigmaNode::Vector { bounds, .. }
        | ImportedFigmaNode::Text { bounds, .. }
        | ImportedFigmaNode::Image { bounds, .. } => *bounds,
    });
    let first = iter.next()?;
    let (mut min_x, mut min_y) = (first.x, first.y);
    let (mut max_x, mut max_y) = (first.x + first.width, first.y + first.height);
    for b in iter {
        min_x = min_x.min(b.x);
        min_y = min_y.min(b.y);
        max_x = max_x.max(b.x + b.width);
        max_y = max_y.max(b.y + b.height);
    }
    Some(ImportedBounds {
        x: min_x,
        y: min_y,
        width: max_x - min_x,
        height: max_y - min_y,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Document-level shape with one canvas, one frame, one text +
    /// one rectangle. Modelled on the actual REST API response so the
    /// importer exercises every code path of the happy path.
    fn fixture_document() -> serde_json::Value {
        serde_json::json!({
            "document": {
                "id": "0:0",
                "name": "My Project",
                "type": "DOCUMENT",
                "children": [{
                    "id": "0:1",
                    "name": "Page 1",
                    "type": "CANVAS",
                    "children": [{
                        "id": "1:1",
                        "name": "Cover Frame",
                        "type": "FRAME",
                        "absoluteBoundingBox": {
                            "x": 0.0, "y": 0.0,
                            "width": 1440.0, "height": 900.0,
                        },
                        "children": [
                            {
                                "id": "1:2",
                                "name": "Headline",
                                "type": "TEXT",
                                "absoluteBoundingBox": {
                                    "x": 64.0, "y": 64.0,
                                    "width": 800.0, "height": 120.0,
                                },
                                "characters": "Hello world",
                                "style": {
                                    "fontFamily": "Inter",
                                    "fontSize": 96.0,
                                },
                                "fills": [{
                                    "type": "SOLID",
                                    "color": {"r": 0.0, "g": 0.0, "b": 0.0, "a": 1.0},
                                }],
                            },
                            {
                                "id": "1:3",
                                "name": "Background",
                                "type": "RECTANGLE",
                                "absoluteBoundingBox": {
                                    "x": 0.0, "y": 0.0,
                                    "width": 1440.0, "height": 900.0,
                                },
                                "fills": [{
                                    "type": "SOLID",
                                    "color": {"r": 1.0, "g": 0.95, "b": 0.5, "a": 1.0},
                                }],
                            }
                        ],
                    }]
                }]
            }
        })
    }

    #[test]
    fn parses_document_with_one_canvas_one_frame() {
        let v = fixture_document();
        let imported = parse_figma_value(&v).expect("happy path parses");
        assert_eq!(imported.document_name.as_deref(), Some("My Project"));
        assert_eq!(imported.pages.len(), 1);
        let page = &imported.pages[0];
        assert_eq!(page.name, "Page 1");
        assert_eq!(page.artboards.len(), 1);
        let ab = &page.artboards[0];
        assert_eq!(ab.name, "Cover Frame");
        assert!((ab.bounds.width - 1440.0).abs() < f64::EPSILON);
        assert!((ab.bounds.height - 900.0).abs() < f64::EPSILON);
        assert_eq!(ab.children.len(), 2);

        // Headline must come through as a text node with the right
        // characters and font; black solid fill maps to [0,0,0,255].
        let text = &ab.children[0];
        match text {
            ImportedFigmaNode::Text {
                characters,
                font_family,
                font_size_px,
                color_rgba,
                ..
            } => {
                assert_eq!(characters, "Hello world");
                assert_eq!(font_family.as_deref(), Some("Inter"));
                assert_eq!(*font_size_px, Some(96.0));
                assert_eq!(*color_rgba, Some([0, 0, 0, 255]));
            }
            _ => panic!("expected Text node, got {text:?}"),
        }

        // Background rect: solid yellow fill, synthesised d-string.
        let rect = &ab.children[1];
        match rect {
            ImportedFigmaNode::Vector {
                path_d, fill_rgba, ..
            } => {
                assert!(path_d.starts_with("M0 0 L1440"));
                assert_eq!(*fill_rgba, Some([255, 242, 128, 255]));
            }
            _ => panic!("expected Vector node, got {rect:?}"),
        }
    }

    #[test]
    fn flattens_nested_frames_with_warning() {
        // Frame contains a nested FRAME (sub-frame) which itself
        // wraps a text node — the importer should flatten the inner
        // text into the outer artboard and emit one warning.
        let v = serde_json::json!({
            "document": {
                "name": "Doc", "type": "DOCUMENT",
                "children": [{
                    "name": "Page", "type": "CANVAS",
                    "children": [{
                        "name": "Outer", "type": "FRAME",
                        "absoluteBoundingBox": {"x":0.0,"y":0.0,"width":100.0,"height":100.0},
                        "children": [{
                            "name": "Inner", "type": "FRAME",
                            "absoluteBoundingBox": {"x":0.0,"y":0.0,"width":50.0,"height":50.0},
                            "children": [{
                                "name": "Deep text", "type": "TEXT",
                                "absoluteBoundingBox": {"x":0.0,"y":0.0,"width":50.0,"height":20.0},
                                "characters": "hi",
                            }],
                        }],
                    }],
                }],
            }
        });
        let imported = parse_figma_value(&v).expect("parses");
        let outer = &imported.pages[0].artboards[0];
        assert_eq!(outer.children.len(), 1);
        matches!(&outer.children[0], ImportedFigmaNode::Text { .. });
        assert!(imported
            .warnings
            .iter()
            .any(|w| matches!(w, FigmaImportWarning::FlattenedNestedFrame { .. })));
    }

    #[test]
    fn loose_canvas_children_become_anonymous_artboard() {
        let v = serde_json::json!({
            "document": {
                "name": "Doc", "type": "DOCUMENT",
                "children": [{
                    "name": "Page", "type": "CANVAS",
                    "children": [
                        {
                            "name": "Loose rect", "type": "RECTANGLE",
                            "absoluteBoundingBox": {"x":10.0,"y":20.0,"width":30.0,"height":40.0},
                            "fills": [{"type":"SOLID","color":{"r":1.0,"g":0.0,"b":0.0,"a":1.0}}],
                        }
                    ],
                }],
            }
        });
        let imported = parse_figma_value(&v).expect("parses");
        let page = &imported.pages[0];
        assert_eq!(page.artboards.len(), 1);
        let ab = &page.artboards[0];
        assert_eq!(ab.name, "Canvas content");
        // Bounds should cover the single loose child exactly.
        assert!((ab.bounds.x - 10.0).abs() < f64::EPSILON);
        assert!((ab.bounds.width - 30.0).abs() < f64::EPSILON);
    }

    #[test]
    fn unsupported_node_type_emits_warning_and_is_dropped() {
        let v = serde_json::json!({
            "document": {
                "name": "Doc", "type": "DOCUMENT",
                "children": [{
                    "name": "Page", "type": "CANVAS",
                    "children": [{
                        "name": "Outer", "type": "FRAME",
                        "absoluteBoundingBox": {"x":0.0,"y":0.0,"width":100.0,"height":100.0},
                        "children": [{
                            "name": "Slice", "type": "SLICE",
                            "absoluteBoundingBox": {"x":0.0,"y":0.0,"width":10.0,"height":10.0},
                        }],
                    }],
                }],
            }
        });
        let imported = parse_figma_value(&v).expect("parses");
        let ab = &imported.pages[0].artboards[0];
        assert!(ab.children.is_empty());
        assert!(imported
            .warnings
            .iter()
            .any(|w| matches!(w, FigmaImportWarning::UnsupportedNodeType { node_type, .. } if node_type == "SLICE")));
    }

    #[test]
    fn image_fill_becomes_image_node() {
        let v = serde_json::json!({
            "document": {
                "name": "Doc", "type": "DOCUMENT",
                "children": [{
                    "name": "Page", "type": "CANVAS",
                    "children": [{
                        "name": "Frame", "type": "FRAME",
                        "absoluteBoundingBox": {"x":0.0,"y":0.0,"width":200.0,"height":200.0},
                        "children": [{
                            "name": "Photo", "type": "RECTANGLE",
                            "absoluteBoundingBox": {"x":0.0,"y":0.0,"width":200.0,"height":200.0},
                            "fills": [{"type":"IMAGE", "imageRef": "abc123"}],
                        }],
                    }],
                }],
            }
        });
        let imported = parse_figma_value(&v).expect("parses");
        let node = &imported.pages[0].artboards[0].children[0];
        match node {
            ImportedFigmaNode::Image { image_ref, .. } => assert_eq!(image_ref, "abc123"),
            _ => panic!("expected Image node, got {node:?}"),
        }
    }

    #[test]
    fn gradient_simplifies_to_first_stop_with_warning() {
        let v = serde_json::json!({
            "document": {
                "name": "Doc", "type": "DOCUMENT",
                "children": [{
                    "name": "Page", "type": "CANVAS",
                    "children": [{
                        "name": "Frame", "type": "FRAME",
                        "absoluteBoundingBox": {"x":0.0,"y":0.0,"width":100.0,"height":100.0},
                        "children": [{
                            "name": "Banner", "type": "RECTANGLE",
                            "absoluteBoundingBox": {"x":0.0,"y":0.0,"width":100.0,"height":100.0},
                            "fills": [{
                                "type": "GRADIENT_LINEAR",
                                "gradientStops": [
                                    {"position": 0.0, "color": {"r":0.0,"g":0.5,"b":1.0,"a":1.0}},
                                    {"position": 1.0, "color": {"r":1.0,"g":0.0,"b":0.5,"a":1.0}}
                                ]
                            }],
                        }],
                    }],
                }],
            }
        });
        let imported = parse_figma_value(&v).expect("parses");
        let node = &imported.pages[0].artboards[0].children[0];
        match node {
            ImportedFigmaNode::Vector { fill_rgba, .. } => {
                assert_eq!(*fill_rgba, Some([0, 128, 255, 255]));
            }
            _ => panic!("expected Vector node, got {node:?}"),
        }
        assert!(imported
            .warnings
            .iter()
            .any(|w| matches!(w, FigmaImportWarning::SimplifiedGradient { .. })));
    }

    #[test]
    fn missing_bounding_box_drops_node_with_warning() {
        let v = serde_json::json!({
            "document": {
                "name": "Doc", "type": "DOCUMENT",
                "children": [{
                    "name": "Page", "type": "CANVAS",
                    "children": [{
                        "name": "Frame", "type": "FRAME",
                        "absoluteBoundingBox": {"x":0.0,"y":0.0,"width":100.0,"height":100.0},
                        "children": [{
                            "name": "No bounds", "type": "TEXT",
                            "characters": "lost",
                        }],
                    }],
                }],
            }
        });
        let imported = parse_figma_value(&v).expect("parses");
        let ab = &imported.pages[0].artboards[0];
        assert!(ab.children.is_empty());
        assert!(imported
            .warnings
            .iter()
            .any(|w| matches!(w, FigmaImportWarning::DroppedShapeless { node_name } if node_name == "No bounds")));
    }

    #[test]
    fn accepts_plugin_export_without_document_wrapper() {
        // Some plugin exporters dump the DOCUMENT object directly.
        let v = serde_json::json!({
            "id": "0:0", "name": "Plain", "type": "DOCUMENT",
            "children": [{
                "name": "P", "type": "CANVAS",
                "children": []
            }]
        });
        let imported = parse_figma_value(&v).expect("parses without wrapper");
        assert_eq!(imported.document_name.as_deref(), Some("Plain"));
        assert_eq!(imported.pages.len(), 1);
    }

    #[test]
    fn empty_or_unrecognised_returns_none() {
        let v = serde_json::json!({"hello": "world"});
        assert!(parse_figma_value(&v).is_none());
    }

    #[test]
    fn round_trip_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file.json");
        std::fs::write(&path, fixture_document().to_string()).unwrap();
        let imported = import_figma(&path).expect("file parses");
        assert_eq!(imported.pages.len(), 1);
    }

    #[test]
    fn invalid_json_returns_parse_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.json");
        std::fs::write(&path, "{not valid json").unwrap();
        let err = import_figma(&path).unwrap_err();
        assert!(matches!(err, FigmaImportError::Parse { .. }));
    }

    #[test]
    fn extract_rgba_rounds_alpha_consistently_with_rgb() {
        // The visible bug: `a = 0.999` truncated as u8 yields 254, but
        // rounded yields 255. The same source color imported through
        // Sketch's `parse_sketch_color` (which uses `clamp_u8`) yields
        // 255, so both paths must now agree.
        let color = serde_json::json!({"r": 0.5, "g": 0.5, "b": 0.5, "a": 0.999});
        let rgba = super::extract_rgba(&color, None).expect("solid color");
        assert_eq!(rgba[3], 255, "alpha must round, not truncate");
        // R/G/B round too: 0.5 * 255 = 127.5 → 128.
        assert_eq!(&rgba[..3], &[128_u8, 128, 128]);
    }

    #[test]
    fn extract_rgba_outer_opacity_rounds_alpha() {
        // Outer opacity multiplies on top of the inner alpha. The
        // product is still rounded to the nearest u8.
        let color = serde_json::json!({"r": 1.0, "g": 1.0, "b": 1.0, "a": 1.0});
        // 0.5 * 255 = 127.5, which rounds up to 128.
        let opacity = serde_json::json!(0.5);
        let rgba = super::extract_rgba(&color, Some(&opacity)).expect("solid color");
        assert_eq!(rgba[3], 128);
    }

    #[test]
    fn extract_rgba_clamps_out_of_range_alpha() {
        // Even though Figma promises 0..=1, a malformed export may
        // emit values outside that range. The clamp keeps the cast
        // sound and prevents UB.
        let color = serde_json::json!({"r": 1.0, "g": 1.0, "b": 1.0, "a": 2.0});
        let rgba = super::extract_rgba(&color, None).expect("solid color");
        assert_eq!(rgba[3], 255);
        let color = serde_json::json!({"r": 1.0, "g": 1.0, "b": 1.0, "a": -1.0});
        let rgba = super::extract_rgba(&color, None).expect("solid color");
        assert_eq!(rgba[3], 0);
    }
}
