//! Sketch import (Phase 6 — Tasks 19–20).
//!
//! A `.sketch` file is a ZIP archive with the following layout:
//!
//! ```text
//! my.sketch
//!  ├── meta.json            // app/version metadata
//!  ├── document.json        // document settings, pages list
//!  ├── user.json            // user prefs (ignored)
//!  ├── pages/<uuid>.json    // one page tree per page
//!  ├── images/<uuid>.png    // raster bitmaps referenced by pages
//!  └── previews/preview.png // single 1024-wide preview image
//! ```
//!
//! Each JSON document is a tree of Sketch's `MSImmutable*` classes,
//! discriminated by the string field `_class`. The shapes we care
//! about:
//!
//! | `_class` value        | KCreate node          |
//! |-----------------------|-----------------------|
//! | `page`                | `Page`                |
//! | `artboard`            | `Artboard`            |
//! | `text`                | `TextLayer`           |
//! | `shapePath`           | `VectorPath`          |
//! | `rectangle`           | `VectorPath` (rect)   |
//! | `oval`                | `VectorPath` (ellipse)|
//! | `bitmap`              | `RasterLayer`         |
//! | `group`               | flattened into parent |
//!
//! All other `_class`es (`symbolMaster`, `slice`, `triangle`,
//! `polygon`, `star`, …) become structured warnings so the renderer
//! can show how much survived.
//!
//! # What is *not* imported (yet)
//! - Symbol masters / overrides
//! - Boolean operations (the result is rendered as a single
//!   `shapePath`, so the importer keeps the shape but loses the
//!   underlying operands)
//! - Image fills on non-bitmap layers
//! - Style.borders / shadows / blurs
//! - Color variables / themes

use std::fmt::Write as _;
use std::fs::File;
use std::io::{Cursor, Read, Seek};
use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use zip::ZipArchive;

/// Top-level result of importing a `.sketch` file. One entry per
/// page in source order, plus document metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedSketch {
    /// `document.json#metadata.name` when present; otherwise `None`.
    pub document_name: Option<String>,
    /// One entry per Sketch `page` JSON in the order the document
    /// references them.
    pub pages: Vec<ImportedSketchPage>,
    /// Non-fatal observations.
    pub warnings: Vec<SketchImportWarning>,
}

/// One Sketch page → one KCreate page.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedSketchPage {
    pub name: String,
    pub artboards: Vec<ImportedSketchArtboard>,
}

/// One Sketch artboard → one KCreate artboard.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedSketchArtboard {
    pub name: String,
    pub bounds: crate::figma_import::ImportedBounds,
    pub children: Vec<ImportedSketchNode>,
}

/// Per-node payload. Re-uses the Figma `ImportedBounds` shape so
/// the bridge has one place to put coordinate-conversion code.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum ImportedSketchNode {
    Vector {
        name: String,
        bounds: crate::figma_import::ImportedBounds,
        /// SVG `d` attribute. Synthesised from the bounding box for
        /// `rectangle` / `oval` and built from the curve-point list
        /// for `shapePath`.
        path_d: String,
        /// First solid fill in sRGB.
        fill_rgba: Option<[u8; 4]>,
    },
    Text {
        name: String,
        bounds: crate::figma_import::ImportedBounds,
        characters: String,
        font_family: Option<String>,
        font_size_px: Option<f64>,
        color_rgba: Option<[u8; 4]>,
    },
    /// A `bitmap` layer. `image_ref` is the file name inside the
    /// archive's `images/` directory; `image_bytes` is the raw
    /// payload pulled out of the ZIP, ready to hand to the blob
    /// store.
    Image {
        name: String,
        bounds: crate::figma_import::ImportedBounds,
        image_ref: String,
        #[serde(with = "serde_bytes")]
        image_bytes: Vec<u8>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum SketchImportWarning {
    /// A group's children were flattened into the enclosing
    /// artboard.
    FlattenedGroup { artboard_name: String },
    /// A `_class` we don't model was dropped.
    UnsupportedClass {
        node_name: String,
        class_name: String,
    },
    /// A bitmap referenced an image that wasn't present in the
    /// archive (corrupted or partial export).
    MissingImageRef {
        node_name: String,
        image_ref: String,
    },
    /// A shapePath couldn't be re-serialised into an SVG `d`
    /// attribute (malformed curve points). The bridge falls back to
    /// the bounding rectangle.
    MalformedShapePath { node_name: String },
}

/// Hard failures from the Sketch import pipeline.
#[derive(Debug, Error)]
pub enum SketchImportError {
    /// The input file couldn't be opened.
    #[error("failed to open Sketch file {path}: {source}")]
    Open {
        path: String,
        #[source]
        source: std::io::Error,
    },
    /// The ZIP container is malformed.
    #[error("failed to read Sketch ZIP {path}: {source}")]
    Zip {
        path: String,
        #[source]
        source: zip::result::ZipError,
    },
    /// `document.json` is missing or unreadable.
    #[error("Sketch file {path} is missing document.json")]
    MissingDocumentJson { path: String },
    /// A JSON document inside the archive couldn't be parsed.
    #[error("failed to parse Sketch JSON `{entry}` in {path}: {source}")]
    Parse {
        path: String,
        entry: String,
        #[source]
        source: serde_json::Error,
    },
}

/// Import a `.sketch` file from disk.
///
/// # Errors
/// Surfaces [`SketchImportError`] for any I/O, ZIP, or JSON failure.
pub fn import_sketch<P: AsRef<Path>>(path: P) -> Result<ImportedSketch, SketchImportError> {
    let path = path.as_ref();
    let path_str = path.display().to_string();
    let file = File::open(path).map_err(|source| SketchImportError::Open {
        path: path_str.clone(),
        source,
    })?;
    parse_sketch_zip(file, &path_str)
}

/// Parse a Sketch archive from any [`Read`] + [`Seek`] source. Public
/// so tests can drive the importer in-memory without round-tripping
/// through the filesystem.
///
/// # Errors
/// Same as [`import_sketch`] except that I/O errors are surfaced as
/// `SketchImportError::Zip` (the `ZipArchive::new` constructor wraps
/// the underlying error).
pub fn parse_sketch_zip<R: Read + Seek>(
    reader: R,
    path_for_errors: &str,
) -> Result<ImportedSketch, SketchImportError> {
    let mut archive = ZipArchive::new(reader).map_err(|source| SketchImportError::Zip {
        path: path_for_errors.to_owned(),
        source,
    })?;

    // 1) Read the document JSON to get the ordered page id list.
    let doc_json = read_zip_json(&mut archive, "document.json").map_err(|source| match source {
        ReadZipJsonError::Missing => SketchImportError::MissingDocumentJson {
            path: path_for_errors.to_owned(),
        },
        ReadZipJsonError::Zip(e) => SketchImportError::Zip {
            path: path_for_errors.to_owned(),
            source: e,
        },
        ReadZipJsonError::Parse(e) => SketchImportError::Parse {
            path: path_for_errors.to_owned(),
            entry: "document.json".to_string(),
            source: e,
        },
    })?;

    let document_name = doc_json
        .pointer("/metadata/name")
        .and_then(|v| v.as_str())
        .map(str::to_owned);

    // `document.json#pages` is an array of `{_class: "MSJSONFileReference",
    // _ref: "pages/<uuid>"}` entries. The bare _ref is the path
    // inside the ZIP without the `.json` extension.
    let page_refs: Vec<String> = doc_json
        .get("pages")
        .and_then(|p| p.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|p| p.get("_ref").and_then(|r| r.as_str()).map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();

    let mut warnings = Vec::new();
    let mut pages = Vec::new();

    // 2) Walk each page JSON in document order.
    for page_ref in &page_refs {
        let entry = format!("{page_ref}.json");
        let page_json = match read_zip_json(&mut archive, &entry) {
            Ok(v) => v,
            Err(ReadZipJsonError::Missing) => {
                continue;
            }
            Err(ReadZipJsonError::Parse(e)) => {
                return Err(SketchImportError::Parse {
                    path: path_for_errors.to_owned(),
                    entry,
                    source: e,
                });
            }
            Err(ReadZipJsonError::Zip(e)) => {
                return Err(SketchImportError::Zip {
                    path: path_for_errors.to_owned(),
                    source: e,
                });
            }
        };
        let parsed = parse_page(&page_json, &mut archive, &mut warnings);
        pages.push(parsed);
    }

    Ok(ImportedSketch {
        document_name,
        pages,
        warnings,
    })
}

/// Internal error helper — distinguishes "no such entry" from
/// "entry exists but the JSON is bad".
enum ReadZipJsonError {
    Missing,
    Zip(zip::result::ZipError),
    Parse(serde_json::Error),
}

fn read_zip_json<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    entry: &str,
) -> Result<serde_json::Value, ReadZipJsonError> {
    let mut file = match archive.by_name(entry) {
        Ok(f) => f,
        Err(zip::result::ZipError::FileNotFound) => return Err(ReadZipJsonError::Missing),
        Err(e) => return Err(ReadZipJsonError::Zip(e)),
    };
    let mut buf = Vec::with_capacity(file.size().min(64 * 1024) as usize);
    file.read_to_end(&mut buf)
        .map_err(|e| ReadZipJsonError::Zip(zip::result::ZipError::Io(e)))?;
    serde_json::from_slice(&buf).map_err(ReadZipJsonError::Parse)
}

fn read_zip_bytes<R: Read + Seek>(archive: &mut ZipArchive<R>, entry: &str) -> Option<Vec<u8>> {
    let mut file = archive.by_name(entry).ok()?;
    let mut buf = Vec::with_capacity(file.size().min(1024 * 1024) as usize);
    file.read_to_end(&mut buf).ok()?;
    Some(buf)
}

fn parse_page<R: Read + Seek>(
    page_json: &serde_json::Value,
    archive: &mut ZipArchive<R>,
    warnings: &mut Vec<SketchImportWarning>,
) -> ImportedSketchPage {
    let name = page_json
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or("Untitled page")
        .to_owned();

    let mut artboards = Vec::new();
    let mut loose_children = Vec::new();

    if let Some(layers) = page_json.get("layers").and_then(|l| l.as_array()) {
        for layer in layers {
            let class = layer.get("_class").and_then(|c| c.as_str()).unwrap_or("");
            if class == "artboard" || class == "symbolMaster" {
                artboards.push(parse_artboard(layer, archive, warnings));
            } else if let Some(node) = parse_layer_as_leaf(layer, archive, warnings) {
                loose_children.push(node);
            }
        }
    }

    if !loose_children.is_empty() {
        let bounds =
            encompassing_bounds(&loose_children).unwrap_or(crate::figma_import::ImportedBounds {
                x: 0.0,
                y: 0.0,
                width: 1024.0,
                height: 768.0,
            });
        artboards.push(ImportedSketchArtboard {
            name: "Page content".to_string(),
            bounds,
            children: loose_children,
        });
    }

    ImportedSketchPage { name, artboards }
}

fn parse_artboard<R: Read + Seek>(
    value: &serde_json::Value,
    archive: &mut ZipArchive<R>,
    warnings: &mut Vec<SketchImportWarning>,
) -> ImportedSketchArtboard {
    let name = value
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or("Artboard")
        .to_owned();
    let bounds = parse_frame_bounds(value).unwrap_or(crate::figma_import::ImportedBounds {
        x: 0.0,
        y: 0.0,
        width: 1024.0,
        height: 768.0,
    });

    let mut children = Vec::new();
    if let Some(layers) = value.get("layers").and_then(|l| l.as_array()) {
        for layer in layers {
            let class = layer.get("_class").and_then(|c| c.as_str()).unwrap_or("");
            if class == "group" {
                warnings.push(SketchImportWarning::FlattenedGroup {
                    artboard_name: name.clone(),
                });
                flatten_group_into(layer, archive, warnings, &mut children);
            } else if let Some(node) = parse_layer_as_leaf(layer, archive, warnings) {
                children.push(node);
            }
        }
    }

    ImportedSketchArtboard {
        name,
        bounds,
        children,
    }
}

fn flatten_group_into<R: Read + Seek>(
    group: &serde_json::Value,
    archive: &mut ZipArchive<R>,
    warnings: &mut Vec<SketchImportWarning>,
    out: &mut Vec<ImportedSketchNode>,
) {
    let Some(layers) = group.get("layers").and_then(|l| l.as_array()) else {
        return;
    };
    for layer in layers {
        let class = layer.get("_class").and_then(|c| c.as_str()).unwrap_or("");
        if class == "group" {
            flatten_group_into(layer, archive, warnings, out);
        } else if let Some(node) = parse_layer_as_leaf(layer, archive, warnings) {
            out.push(node);
        }
    }
}

fn parse_layer_as_leaf<R: Read + Seek>(
    layer: &serde_json::Value,
    archive: &mut ZipArchive<R>,
    warnings: &mut Vec<SketchImportWarning>,
) -> Option<ImportedSketchNode> {
    let class = layer.get("_class").and_then(|c| c.as_str()).unwrap_or("");
    let name = layer
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or("Untitled")
        .to_owned();

    match class {
        "text" => Some(parse_text(layer, name)),
        "shapePath" => Some(parse_shape_path(layer, name, warnings)),
        "rectangle" | "oval" => Some(parse_simple_shape(layer, name, class)),
        "bitmap" => parse_bitmap(layer, name, archive, warnings),
        other => {
            warnings.push(SketchImportWarning::UnsupportedClass {
                node_name: name,
                class_name: other.to_owned(),
            });
            None
        }
    }
}

fn parse_text(layer: &serde_json::Value, name: String) -> ImportedSketchNode {
    let bounds = parse_frame_bounds(layer).unwrap_or(crate::figma_import::ImportedBounds {
        x: 0.0,
        y: 0.0,
        width: 100.0,
        height: 20.0,
    });

    // Sketch stores the rendered text as a flat string at
    // `attributedString.string` and per-run styles in
    // `attributedString.attributes[]`. The importer keeps the plain
    // string and the dominant run's font/color.
    let characters = layer
        .pointer("/attributedString/string")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_owned();

    let first_attr = layer
        .pointer("/attributedString/attributes/0/attributes")
        .or_else(|| layer.pointer("/style/textStyle/encodedAttributes"));
    let font_family = first_attr
        .and_then(|a| a.get("MSAttributedStringFontAttribute"))
        .and_then(|f| f.pointer("/attributes/name"))
        .and_then(|n| n.as_str())
        .map(str::to_owned);
    let font_size_px = first_attr
        .and_then(|a| a.get("MSAttributedStringFontAttribute"))
        .and_then(|f| f.pointer("/attributes/size"))
        .and_then(serde_json::Value::as_f64);
    let color_rgba = first_attr
        .and_then(|a| a.get("MSAttributedStringColorAttribute"))
        .and_then(parse_sketch_color);

    ImportedSketchNode::Text {
        name,
        bounds,
        characters,
        font_family,
        font_size_px,
        color_rgba,
    }
}

fn parse_simple_shape(layer: &serde_json::Value, name: String, class: &str) -> ImportedSketchNode {
    let bounds = parse_frame_bounds(layer).unwrap_or(crate::figma_import::ImportedBounds {
        x: 0.0,
        y: 0.0,
        width: 10.0,
        height: 10.0,
    });
    let path_d = synthesise_path_d(class, &bounds);
    let fill_rgba = first_solid_fill(layer);
    ImportedSketchNode::Vector {
        name,
        bounds,
        path_d,
        fill_rgba,
    }
}

fn parse_shape_path(
    layer: &serde_json::Value,
    name: String,
    warnings: &mut Vec<SketchImportWarning>,
) -> ImportedSketchNode {
    let bounds = parse_frame_bounds(layer).unwrap_or(crate::figma_import::ImportedBounds {
        x: 0.0,
        y: 0.0,
        width: 10.0,
        height: 10.0,
    });
    let path_d = match build_path_d_from_points(layer, &bounds) {
        Some(d) => d,
        None => {
            warnings.push(SketchImportWarning::MalformedShapePath {
                node_name: name.clone(),
            });
            synthesise_path_d("rectangle", &bounds)
        }
    };
    let fill_rgba = first_solid_fill(layer);
    ImportedSketchNode::Vector {
        name,
        bounds,
        path_d,
        fill_rgba,
    }
}

/// Build an SVG `d` attribute from a `shapePath`'s curve-points
/// array. Each point looks like:
/// ```text
/// {
///   "_class": "curvePoint",
///   "point": "{0.5, 0.0}",      // unit coordinates (0..1) inside the frame
///   "curveFrom": "{0.5, 0.0}",
///   "curveTo":   "{0.5, 0.0}",
///   "hasCurveFrom": false,
///   "hasCurveTo":   false,
/// }
/// ```
/// We treat `hasCurveFrom`/`hasCurveTo` as a switch between line-to
/// and cubic-Bezier. Points are converted from unit-rect to pixels
/// using the path's own frame.
fn build_path_d_from_points(
    layer: &serde_json::Value,
    bounds: &crate::figma_import::ImportedBounds,
) -> Option<String> {
    let points = layer.get("points").and_then(|p| p.as_array())?;
    if points.is_empty() {
        return None;
    }

    let (w, h) = (bounds.width, bounds.height);
    let mut out = String::new();
    let mut prev_curve_from: Option<(f64, f64)> = None;

    for (i, p) in points.iter().enumerate() {
        let pt = parse_sketch_point(p.get("point"))?;
        let (px, py) = (pt.0 * w, pt.1 * h);

        // `write!` into a `String` only fails if the formatter
        // returns an error — the `Display` impls for `f64` are
        // infallible, so a `let _ =` discards a guaranteed-Ok.
        if i == 0 {
            let _ = write!(out, "M{px} {py} ");
        } else if let Some(cf) = prev_curve_from {
            // Use cubic-Bezier if either side declared a control
            // handle; otherwise a straight line.
            let has_curve_to = p
                .get("hasCurveTo")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            if has_curve_to {
                let ct = parse_sketch_point(p.get("curveTo")).unwrap_or(pt);
                let (ctx, cty) = (ct.0 * w, ct.1 * h);
                let _ = write!(out, "C{} {} {ctx} {cty} {px} {py} ", cf.0, cf.1);
            } else {
                let _ = write!(out, "L{px} {py} ");
            }
        } else {
            let _ = write!(out, "L{px} {py} ");
        }

        let has_curve_from = p
            .get("hasCurveFrom")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        prev_curve_from = if has_curve_from {
            parse_sketch_point(p.get("curveFrom")).map(|(x, y)| (x * w, y * h))
        } else {
            None
        };
    }

    let is_closed = layer
        .get("isClosed")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);
    if is_closed {
        out.push('Z');
    }
    Some(out)
}

fn parse_sketch_point(v: Option<&serde_json::Value>) -> Option<(f64, f64)> {
    // Sketch encodes points as a string `"{x, y}"`.
    let s = v?.as_str()?;
    let inner = s.trim().trim_start_matches('{').trim_end_matches('}');
    let mut parts = inner.split(',').map(str::trim);
    let x: f64 = parts.next()?.parse().ok()?;
    let y: f64 = parts.next()?.parse().ok()?;
    Some((x, y))
}

fn parse_bitmap<R: Read + Seek>(
    layer: &serde_json::Value,
    name: String,
    archive: &mut ZipArchive<R>,
    warnings: &mut Vec<SketchImportWarning>,
) -> Option<ImportedSketchNode> {
    let bounds = parse_frame_bounds(layer).unwrap_or(crate::figma_import::ImportedBounds {
        x: 0.0,
        y: 0.0,
        width: 10.0,
        height: 10.0,
    });
    let image_ref = layer
        .pointer("/image/_ref")
        .and_then(|r| r.as_str())
        .unwrap_or("")
        .to_owned();
    if image_ref.is_empty() {
        warnings.push(SketchImportWarning::MissingImageRef {
            node_name: name,
            image_ref,
        });
        return None;
    }
    // Sketch refers to bitmaps by their archive path without
    // extension ("images/abc"); the archive stores them with a real
    // extension. Probe the common ones.
    let mut bytes = None;
    for ext in ["png", "jpg", "jpeg", "webp", "tiff"] {
        let entry = format!("{image_ref}.{ext}");
        if let Some(b) = read_zip_bytes(archive, &entry) {
            bytes = Some(b);
            break;
        }
    }
    let Some(image_bytes) = bytes else {
        warnings.push(SketchImportWarning::MissingImageRef {
            node_name: name,
            image_ref,
        });
        return None;
    };

    Some(ImportedSketchNode::Image {
        name,
        bounds,
        image_ref,
        image_bytes,
    })
}

fn parse_frame_bounds(value: &serde_json::Value) -> Option<crate::figma_import::ImportedBounds> {
    let frame = value.get("frame")?;
    let x = frame.get("x").and_then(serde_json::Value::as_f64)?;
    let y = frame.get("y").and_then(serde_json::Value::as_f64)?;
    let width = frame.get("width").and_then(serde_json::Value::as_f64)?;
    let height = frame.get("height").and_then(serde_json::Value::as_f64)?;
    Some(crate::figma_import::ImportedBounds {
        x,
        y,
        width,
        height,
    })
}

/// Sketch encodes colors at `style.fills[0].color = {alpha, red,
/// green, blue}` (each 0..=1 f64).
fn first_solid_fill(layer: &serde_json::Value) -> Option<[u8; 4]> {
    let fill = layer.pointer("/style/fills/0")?;
    let enabled = fill
        .get("isEnabled")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);
    if !enabled {
        return None;
    }
    // Sketch fillType: 0 = solid, 1 = gradient, 4 = image
    let fill_type = fill
        .get("fillType")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0);
    if fill_type != 0 {
        return None;
    }
    let color = fill.get("color")?;
    parse_sketch_color(color)
}

fn parse_sketch_color(color: &serde_json::Value) -> Option<[u8; 4]> {
    let r = color.get("red").and_then(serde_json::Value::as_f64)?;
    let g = color.get("green").and_then(serde_json::Value::as_f64)?;
    let b = color.get("blue").and_then(serde_json::Value::as_f64)?;
    let a = color
        .get("alpha")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(1.0);
    Some([clamp_u8(r), clamp_u8(g), clamp_u8(b), clamp_u8(a)])
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn clamp_u8(v: f64) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn synthesise_path_d(class: &str, bounds: &crate::figma_import::ImportedBounds) -> String {
    let w = bounds.width.max(0.0);
    let h = bounds.height.max(0.0);
    match class {
        "oval" => {
            let rx = w / 2.0;
            let ry = h / 2.0;
            format!("M0 {ry} A{rx} {ry} 0 1 0 {w} {ry} A{rx} {ry} 0 1 0 0 {ry} Z")
        }
        _ => format!("M0 0 L{w} 0 L{w} {h} L0 {h} Z"),
    }
}

fn encompassing_bounds(
    nodes: &[ImportedSketchNode],
) -> Option<crate::figma_import::ImportedBounds> {
    let mut iter = nodes.iter().map(|n| match n {
        ImportedSketchNode::Vector { bounds, .. }
        | ImportedSketchNode::Text { bounds, .. }
        | ImportedSketchNode::Image { bounds, .. } => *bounds,
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
    Some(crate::figma_import::ImportedBounds {
        x: min_x,
        y: min_y,
        width: max_x - min_x,
        height: max_y - min_y,
    })
}

// `serde_bytes` is *almost* always pulled in transitively, but the
// dep tree doesn't force it. Provide a tiny module shim that uses
// the bytes-friendly path through `serde`'s default impls so we
// don't have to touch Cargo.toml for a single field. The shim is
// only used inside this module — `Vec<u8>` round-trips as a JSON
// array under the default impl, which is fine for our wire surface
// (the bridge never re-serialises ImportedSketchNode to JSON in the
// editing path; the image bytes go straight into the blob store).
mod serde_bytes {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub(super) fn serialize<S: Serializer>(bytes: &Vec<u8>, s: S) -> Result<S::Ok, S::Error> {
        bytes.serialize(s)
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        Vec::<u8>::deserialize(d)
    }
}

/// Re-export the in-memory parser entry point so tests in other
/// modules can call it without rebuilding the file on disk.
pub fn parse_sketch_bytes(bytes: Vec<u8>) -> Result<ImportedSketch, SketchImportError> {
    parse_sketch_zip(Cursor::new(bytes), "<memory>")
}

#[cfg(test)]
mod tests {
    use super::*;
    use zip::write::SimpleFileOptions;
    use zip::ZipWriter;

    fn write_zip(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let mut buf = Vec::<u8>::new();
        {
            let mut zw = ZipWriter::new(Cursor::new(&mut buf));
            let opts =
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
            for (name, bytes) in entries {
                use std::io::Write;
                zw.start_file(*name, opts).unwrap();
                zw.write_all(bytes).unwrap();
            }
            zw.finish().unwrap();
        }
        buf
    }

    fn document_json(page_uuid: &str, name: &str) -> Vec<u8> {
        serde_json::json!({
            "_class": "document",
            "metadata": {"name": name},
            "pages": [{"_class": "MSJSONFileReference", "_ref": format!("pages/{page_uuid}")}],
        })
        .to_string()
        .into_bytes()
    }

    fn page_with_artboard_text_and_rect(page_name: &str) -> Vec<u8> {
        serde_json::json!({
            "_class": "page",
            "name": page_name,
            "layers": [{
                "_class": "artboard",
                "name": "Cover",
                "frame": {"_class":"rect", "x":0.0,"y":0.0,"width":1440.0,"height":900.0},
                "layers": [
                    {
                        "_class": "text",
                        "name": "Headline",
                        "frame": {"_class":"rect","x":64.0,"y":64.0,"width":800.0,"height":120.0},
                        "attributedString": {
                            "string": "Hello sketch",
                            "attributes": [{
                                "attributes": {
                                    "MSAttributedStringFontAttribute": {
                                        "_class": "fontDescriptor",
                                        "attributes": {"name":"Inter","size":96.0}
                                    },
                                    "MSAttributedStringColorAttribute": {
                                        "_class": "color",
                                        "red":0.0,"green":0.0,"blue":0.0,"alpha":1.0
                                    }
                                }
                            }]
                        }
                    },
                    {
                        "_class": "rectangle",
                        "name": "Background",
                        "frame": {"_class":"rect","x":0.0,"y":0.0,"width":1440.0,"height":900.0},
                        "style": {
                            "fills": [{
                                "_class": "fill",
                                "isEnabled": true,
                                "fillType": 0,
                                "color": {"_class":"color","red":1.0,"green":0.95,"blue":0.5,"alpha":1.0}
                            }]
                        }
                    }
                ]
            }]
        })
        .to_string()
        .into_bytes()
    }

    #[test]
    fn happy_path_parses_one_page_with_artboard_text_rect() {
        let uuid = "1A2B3C";
        let zip = write_zip(&[
            ("document.json", document_json(uuid, "MyDoc")),
            (
                &format!("pages/{uuid}.json"),
                page_with_artboard_text_and_rect("Page 1"),
            ),
        ]);
        let imported = parse_sketch_bytes(zip).expect("parses");
        assert_eq!(imported.document_name.as_deref(), Some("MyDoc"));
        assert_eq!(imported.pages.len(), 1);
        let page = &imported.pages[0];
        assert_eq!(page.name, "Page 1");
        assert_eq!(page.artboards.len(), 1);
        let ab = &page.artboards[0];
        assert_eq!(ab.name, "Cover");
        assert!((ab.bounds.width - 1440.0).abs() < f64::EPSILON);
        assert_eq!(ab.children.len(), 2);
        match &ab.children[0] {
            ImportedSketchNode::Text {
                characters,
                font_family,
                font_size_px,
                color_rgba,
                ..
            } => {
                assert_eq!(characters, "Hello sketch");
                assert_eq!(font_family.as_deref(), Some("Inter"));
                assert_eq!(*font_size_px, Some(96.0));
                assert_eq!(*color_rgba, Some([0, 0, 0, 255]));
            }
            other => panic!("expected Text, got {other:?}"),
        }
        match &ab.children[1] {
            ImportedSketchNode::Vector {
                fill_rgba, path_d, ..
            } => {
                assert_eq!(*fill_rgba, Some([255, 242, 128, 255]));
                assert!(path_d.starts_with("M0 0"));
            }
            other => panic!("expected Vector, got {other:?}"),
        }
    }

    #[test]
    fn group_flattens_with_warning() {
        let uuid = "G1";
        let page = serde_json::json!({
            "_class": "page",
            "name": "P",
            "layers": [{
                "_class": "artboard",
                "name": "AB",
                "frame": {"_class":"rect","x":0.0,"y":0.0,"width":100.0,"height":100.0},
                "layers": [{
                    "_class": "group",
                    "name": "Wrap",
                    "frame": {"_class":"rect","x":0.0,"y":0.0,"width":50.0,"height":50.0},
                    "layers": [{
                        "_class": "oval",
                        "name": "Dot",
                        "frame": {"_class":"rect","x":0.0,"y":0.0,"width":50.0,"height":50.0},
                    }]
                }]
            }]
        })
        .to_string()
        .into_bytes();
        let zip = write_zip(&[
            ("document.json", document_json(uuid, "X")),
            (&format!("pages/{uuid}.json"), page),
        ]);
        let imported = parse_sketch_bytes(zip).expect("parses");
        let ab = &imported.pages[0].artboards[0];
        assert_eq!(ab.children.len(), 1);
        match &ab.children[0] {
            ImportedSketchNode::Vector { path_d, .. } => {
                assert!(path_d.contains('A'), "expected oval arc, got `{path_d}`");
            }
            n => panic!("expected Vector oval, got {n:?}"),
        }
        assert!(imported
            .warnings
            .iter()
            .any(|w| matches!(w, SketchImportWarning::FlattenedGroup { .. })));
    }

    #[test]
    fn shape_path_curve_points_become_path_d() {
        let uuid = "P1";
        let page = serde_json::json!({
            "_class": "page",
            "name": "P",
            "layers": [{
                "_class": "artboard",
                "name": "AB",
                "frame": {"_class":"rect","x":0.0,"y":0.0,"width":100.0,"height":100.0},
                "layers": [{
                    "_class": "shapePath",
                    "name": "Tri",
                    "frame": {"_class":"rect","x":0.0,"y":0.0,"width":100.0,"height":100.0},
                    "isClosed": true,
                    "points": [
                        {"_class":"curvePoint","point":"{0, 1}","hasCurveFrom":false,"hasCurveTo":false},
                        {"_class":"curvePoint","point":"{0.5, 0}","hasCurveFrom":false,"hasCurveTo":false},
                        {"_class":"curvePoint","point":"{1, 1}","hasCurveFrom":false,"hasCurveTo":false}
                    ]
                }]
            }]
        })
        .to_string()
        .into_bytes();
        let zip = write_zip(&[
            ("document.json", document_json(uuid, "X")),
            (&format!("pages/{uuid}.json"), page),
        ]);
        let imported = parse_sketch_bytes(zip).expect("parses");
        let ab = &imported.pages[0].artboards[0];
        match &ab.children[0] {
            ImportedSketchNode::Vector { path_d, .. } => {
                assert!(path_d.starts_with("M0 100"));
                assert!(path_d.ends_with('Z'));
            }
            n => panic!("expected Vector, got {n:?}"),
        }
    }

    #[test]
    fn unsupported_class_emits_warning_and_drops() {
        let uuid = "S1";
        let page = serde_json::json!({
            "_class": "page",
            "name": "P",
            "layers": [{
                "_class": "artboard",
                "name": "AB",
                "frame": {"_class":"rect","x":0.0,"y":0.0,"width":100.0,"height":100.0},
                "layers": [{
                    "_class": "slice",
                    "name": "Sliced",
                    "frame": {"_class":"rect","x":0.0,"y":0.0,"width":10.0,"height":10.0}
                }]
            }]
        })
        .to_string()
        .into_bytes();
        let zip = write_zip(&[
            ("document.json", document_json(uuid, "X")),
            (&format!("pages/{uuid}.json"), page),
        ]);
        let imported = parse_sketch_bytes(zip).expect("parses");
        let ab = &imported.pages[0].artboards[0];
        assert!(ab.children.is_empty());
        assert!(imported
            .warnings
            .iter()
            .any(|w| matches!(w, SketchImportWarning::UnsupportedClass { class_name, .. } if class_name == "slice")));
    }

    #[test]
    fn bitmap_pulls_image_bytes_from_archive() {
        let uuid = "B1";
        let png_bytes: Vec<u8> = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        let page = serde_json::json!({
            "_class": "page",
            "name": "P",
            "layers": [{
                "_class": "artboard",
                "name": "AB",
                "frame": {"_class":"rect","x":0.0,"y":0.0,"width":100.0,"height":100.0},
                "layers": [{
                    "_class": "bitmap",
                    "name": "Photo",
                    "frame": {"_class":"rect","x":0.0,"y":0.0,"width":100.0,"height":100.0},
                    "image": {"_class":"MSJSONFileReference","_ref":"images/abc"}
                }]
            }]
        })
        .to_string()
        .into_bytes();
        let zip = write_zip(&[
            ("document.json", document_json(uuid, "X")),
            (&format!("pages/{uuid}.json"), page),
            ("images/abc.png", png_bytes.clone()),
        ]);
        let imported = parse_sketch_bytes(zip).expect("parses");
        let ab = &imported.pages[0].artboards[0];
        match &ab.children[0] {
            ImportedSketchNode::Image {
                image_ref,
                image_bytes,
                ..
            } => {
                assert_eq!(image_ref, "images/abc");
                assert_eq!(image_bytes, &png_bytes);
            }
            n => panic!("expected Image, got {n:?}"),
        }
    }

    #[test]
    fn missing_document_json_returns_error() {
        let zip = write_zip(&[("meta.json", b"{}".to_vec())]);
        let err = parse_sketch_bytes(zip).unwrap_err();
        assert!(matches!(err, SketchImportError::MissingDocumentJson { .. }));
    }

    #[test]
    fn malformed_zip_returns_zip_error() {
        let err = parse_sketch_bytes(vec![0, 1, 2, 3, 4]).unwrap_err();
        assert!(matches!(err, SketchImportError::Zip { .. }));
    }

    #[test]
    fn round_trip_from_disk() {
        let uuid = "D1";
        let zip = write_zip(&[
            ("document.json", document_json(uuid, "FromDisk")),
            (
                &format!("pages/{uuid}.json"),
                page_with_artboard_text_and_rect("Page"),
            ),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file.sketch");
        std::fs::write(&path, zip).unwrap();
        let imported = import_sketch(&path).expect("parses");
        assert_eq!(imported.document_name.as_deref(), Some("FromDisk"));
    }
}
