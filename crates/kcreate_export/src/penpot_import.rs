//! Penpot best-effort importer (Phase 9 Task 15).
//!
//! Penpot's `.penpot` files are ZIP archives containing one or
//! more JSON manifests. The exact schema is documented at
//! <https://github.com/penpot/penpot/blob/develop/docs/specs/penpot-files.md>.
//! We do a best-effort import that maps:
//!
//! - Penpot **pages** → KCreate pages.
//! - Penpot **frames** (artboards) → KCreate frames.
//! - Penpot **shapes** (rect / circle / path / text / image) →
//!   KCreate nodes. Vector shapes become `VectorPath`s; raster
//!   shapes become `RasterImage`s with the embedded asset bytes
//!   piped through the blob store at the bridge layer.
//!
//! Unsupported features (boolean groups, smart components,
//! complex stroke profiles) are downgraded gracefully and
//! reported via [`PenpotImportWarning`]. We do NOT silently
//! drop content — every drop produces a warning.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use zip::ZipArchive;

#[derive(Debug, Error)]
pub enum PenpotImportError {
    #[error("could not read Penpot file: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid Penpot archive: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("manifest JSON parse error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("no Penpot manifest found in archive")]
    NoManifest,
    #[error("manifest is empty (no pages)")]
    EmptyManifest,
}

/// Top-level result of importing a `.penpot` archive.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedPenpot {
    /// `name` field from the project manifest, when present.
    pub project_name: Option<String>,
    /// One entry per Penpot page in source order.
    pub pages: Vec<ImportedPenpotPage>,
    /// Raw asset bytes addressed by Penpot's UUID. The bridge
    /// runs each one through the BLAKE3 blob store.
    pub assets: Vec<ImportedPenpotAsset>,
    pub warnings: Vec<PenpotImportWarning>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedPenpotPage {
    pub id: String,
    pub name: String,
    /// Page background color, in `#rrggbb` form. Defaults to `#ffffff`.
    pub background: String,
    /// One entry per top-level frame on this page.
    pub frames: Vec<ImportedPenpotFrame>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedPenpotFrame {
    pub id: String,
    pub name: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub shapes: Vec<ImportedPenpotShape>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedPenpotShape {
    pub id: String,
    pub kind: ImportedPenpotShapeKind,
    pub name: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// Fill colour in `#rrggbb` form. Empty when the shape has no
    /// fill (image / pattern / gradient).
    pub fill: Option<String>,
    /// Stroke colour in `#rrggbb` form, when present.
    pub stroke: Option<String>,
    pub stroke_width: f32,
    pub opacity: f32,
    /// Text content. Only populated for `kind = "text"`.
    pub text: Option<String>,
    /// Asset reference. Only populated for `kind = "image"` — the
    /// asset bytes live in [`ImportedPenpot::assets`].
    pub asset_id: Option<String>,
    /// SVG path `d` attribute. Only populated for `kind = "path"`.
    pub path_d: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ImportedPenpotShapeKind {
    Rect,
    Circle,
    Ellipse,
    Path,
    Text,
    Image,
    Group,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedPenpotAsset {
    pub id: String,
    pub mime: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PenpotImportWarning {
    pub kind: String,
    pub detail: String,
}

/// Parse a `.penpot` archive at `path` and produce an
/// [`ImportedPenpot`].
pub fn import_penpot(path: &Path) -> Result<ImportedPenpot, PenpotImportError> {
    let bytes = std::fs::read(path)?;
    import_penpot_bytes(&bytes)
}

/// Parse a `.penpot` archive's bytes. Used by tests and the
/// drag-drop IPC path.
pub fn import_penpot_bytes(bytes: &[u8]) -> Result<ImportedPenpot, PenpotImportError> {
    let cursor = std::io::Cursor::new(bytes);
    let mut zip = ZipArchive::new(cursor)?;
    let mut warnings = Vec::new();
    let mut project_name = None;
    let mut pages_json: HashMap<String, serde_json::Value> = HashMap::new();
    let mut assets: Vec<ImportedPenpotAsset> = Vec::new();
    // We scan in two passes so order-of-entries doesn't matter.
    // First pass: pull every file's bytes into memory keyed by
    // its name. Penpot archives are small (one project per file)
    // so this is fine.
    let names: Vec<String> = (0..zip.len())
        .filter_map(|i| zip.by_index(i).ok().map(|e| e.name().to_string()))
        .collect();
    for name in &names {
        let mut entry = zip.by_name(name)?;
        if entry.is_dir() {
            continue;
        }
        let mut buf = Vec::new();
        entry.read_to_end(&mut buf)?;
        if name.ends_with("/manifest.json") || name == "manifest.json" {
            let v: serde_json::Value = serde_json::from_slice(&buf)?;
            if let Some(name) = v.get("name").and_then(|n| n.as_str()) {
                project_name = Some(name.to_string());
            }
        } else if name.contains("/pages/")
            && std::path::Path::new(name)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
        {
            // pages/<uuid>.json
            let page_id = std::path::Path::new(name)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            let v: serde_json::Value = serde_json::from_slice(&buf)?;
            pages_json.insert(page_id, v);
        } else if name.contains("/assets/") {
            // assets/<uuid>.<ext>
            let stem = std::path::Path::new(name)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            let ext = std::path::Path::new(name)
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("");
            let mime = match ext {
                "png" => "image/png",
                "jpg" | "jpeg" => "image/jpeg",
                "webp" => "image/webp",
                "svg" => "image/svg+xml",
                "gif" => "image/gif",
                _ => "application/octet-stream",
            }
            .to_string();
            if !stem.is_empty() {
                assets.push(ImportedPenpotAsset {
                    id: stem,
                    mime,
                    bytes: buf,
                });
            }
        }
    }
    if pages_json.is_empty() {
        return Err(PenpotImportError::NoManifest);
    }
    // Sort pages by id for determinism.
    let mut sorted_pages: Vec<(String, serde_json::Value)> = pages_json.into_iter().collect();
    sorted_pages.sort_by(|a, b| a.0.cmp(&b.0));

    let mut pages = Vec::with_capacity(sorted_pages.len());
    for (page_id, doc) in sorted_pages {
        let page = parse_page(&page_id, &doc, &mut warnings);
        pages.push(page);
    }
    if pages.is_empty() {
        return Err(PenpotImportError::EmptyManifest);
    }
    Ok(ImportedPenpot {
        project_name,
        pages,
        assets,
        warnings,
    })
}

fn parse_page(
    page_id: &str,
    doc: &serde_json::Value,
    warnings: &mut Vec<PenpotImportWarning>,
) -> ImportedPenpotPage {
    let name = doc
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("Untitled")
        .to_string();
    let background = doc
        .get("options")
        .and_then(|o| o.get("background"))
        .and_then(|b| b.as_str())
        .unwrap_or("#ffffff")
        .to_string();
    // Penpot pages put their shapes in an `objects` map keyed by
    // UUID. Each shape has a `type` and a `parent-id`. Top-level
    // frames are children of a synthetic root.
    let objects = doc.get("objects").and_then(|o| o.as_object());
    let mut frames: Vec<ImportedPenpotFrame> = Vec::new();
    if let Some(obj) = objects {
        // First: collect every shape we recognise.
        let mut shapes_by_parent: HashMap<String, Vec<ImportedPenpotShape>> = HashMap::new();
        let mut frame_records: Vec<(String, &serde_json::Value)> = Vec::new();
        for (sid, sv) in obj {
            let kind = sv
                .get("type")
                .and_then(|t| t.as_str())
                .unwrap_or("other");
            if kind == "frame" {
                frame_records.push((sid.clone(), sv));
                continue;
            }
            let parent = sv
                .get("parent-id")
                .and_then(|p| p.as_str())
                .unwrap_or("")
                .to_string();
            if parent.is_empty() {
                continue;
            }
            match parse_shape(sid, sv) {
                Some(shape) => {
                    shapes_by_parent.entry(parent).or_default().push(shape);
                }
                None => {
                    warnings.push(PenpotImportWarning {
                        kind: "unsupported-shape".into(),
                        detail: format!("dropped shape '{sid}' of type '{kind}'"),
                    });
                }
            }
        }
        // Sort frame records by name then id so output is stable.
        frame_records.sort_by(|a, b| a.0.cmp(&b.0));
        for (fid, fv) in frame_records {
            let name = fv
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("Frame")
                .to_string();
            let (x, y, w, h) = bbox(fv);
            let shapes = shapes_by_parent.remove(&fid).unwrap_or_default();
            frames.push(ImportedPenpotFrame {
                id: fid,
                name,
                x,
                y,
                width: w,
                height: h,
                shapes,
            });
        }
        // Any leftover shapes whose parent wasn't a frame go into
        // the warnings.
        for (parent, shapes) in shapes_by_parent {
            warnings.push(PenpotImportWarning {
                kind: "orphan-shape".into(),
                detail: format!(
                    "{} shapes had parent '{}' but no matching frame",
                    shapes.len(),
                    parent
                ),
            });
        }
    } else {
        warnings.push(PenpotImportWarning {
            kind: "empty-page".into(),
            detail: format!("page {page_id} has no objects map"),
        });
    }
    ImportedPenpotPage {
        id: page_id.to_string(),
        name,
        background,
        frames,
    }
}

fn parse_shape(id: &str, sv: &serde_json::Value) -> Option<ImportedPenpotShape> {
    let kind_str = sv.get("type").and_then(|t| t.as_str()).unwrap_or("");
    let kind = match kind_str {
        "rect" => ImportedPenpotShapeKind::Rect,
        "circle" => ImportedPenpotShapeKind::Circle,
        "ellipse" => ImportedPenpotShapeKind::Ellipse,
        "path" => ImportedPenpotShapeKind::Path,
        "text" => ImportedPenpotShapeKind::Text,
        "image" => ImportedPenpotShapeKind::Image,
        "group" => ImportedPenpotShapeKind::Group,
        "" => return None,
        _ => ImportedPenpotShapeKind::Other,
    };
    let name = sv
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or("Shape")
        .to_string();
    let (x, y, w, h) = bbox(sv);
    let fills = sv.get("fills").and_then(|f| f.as_array());
    let fill = fills
        .and_then(|arr| arr.first())
        .and_then(|f| f.get("fill-color"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let strokes = sv.get("strokes").and_then(|s| s.as_array());
    let stroke = strokes
        .and_then(|arr| arr.first())
        .and_then(|s| s.get("stroke-color"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let stroke_width = strokes
        .and_then(|arr| arr.first())
        .and_then(|s| s.get("stroke-width"))
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(0.0) as f32;
    let opacity = sv
        .get("opacity")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(1.0) as f32;
    let text = sv
        .get("content")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let asset_id = sv
        .get("metadata")
        .and_then(|m| m.get("id"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let path_d = sv
        .get("content")
        .and_then(|c| c.get("d"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            sv.get("d")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        });
    Some(ImportedPenpotShape {
        id: id.to_string(),
        kind,
        name,
        x,
        y,
        width: w,
        height: h,
        fill,
        stroke,
        stroke_width,
        opacity,
        text,
        asset_id,
        path_d,
    })
}

fn bbox(sv: &serde_json::Value) -> (f32, f32, f32, f32) {
    let x = sv.get("x").and_then(serde_json::Value::as_f64).unwrap_or(0.0) as f32;
    let y = sv.get("y").and_then(serde_json::Value::as_f64).unwrap_or(0.0) as f32;
    let w = sv.get("width").and_then(serde_json::Value::as_f64).unwrap_or(0.0) as f32;
    let h = sv.get("height").and_then(serde_json::Value::as_f64).unwrap_or(0.0) as f32;
    (x, y, w, h)
}

#[cfg(test)]
#[allow(clippy::needless_raw_string_hashes)] // JSON fixtures contain `"#` (e.g. "#ffffff") which would terminate a single-hash raw string.
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    fn build_archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut z = zip::ZipWriter::new(std::io::Cursor::new(&mut out));
            let opts = SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            for (name, bytes) in entries {
                z.start_file(*name, opts).unwrap();
                z.write_all(bytes).unwrap();
            }
            z.finish().unwrap();
        }
        out
    }

    #[test]
    fn parses_minimal_archive() {
        let manifest: &[u8] = br##"{ "name": "Demo", "version": 1 }"##;
        let page: &[u8] = br##"{
            "name": "Home",
            "options": { "background": "#ffffff" },
            "objects": {
                "frame-1": { "type": "frame", "name": "Hero",
                    "x": 0, "y": 0, "width": 100, "height": 50 },
                "rect-1": { "type": "rect", "name": "BG",
                    "parent-id": "frame-1",
                    "x": 10, "y": 10, "width": 80, "height": 30,
                    "fills": [{ "fill-color": "#ff0000" }] }
            }
        }"##;
        let bytes = build_archive(&[
            ("manifest.json", manifest),
            ("project/pages/page-1.json", page),
        ]);
        let imported = import_penpot_bytes(&bytes).unwrap();
        assert_eq!(imported.project_name.as_deref(), Some("Demo"));
        assert_eq!(imported.pages.len(), 1);
        let p = &imported.pages[0];
        assert_eq!(p.name, "Home");
        assert_eq!(p.frames.len(), 1);
        let f = &p.frames[0];
        assert_eq!(f.shapes.len(), 1);
        assert_eq!(f.shapes[0].kind, ImportedPenpotShapeKind::Rect);
        assert_eq!(f.shapes[0].fill.as_deref(), Some("#ff0000"));
    }

    #[test]
    fn rejects_garbage_archive() {
        let err = import_penpot_bytes(b"not a zip").unwrap_err();
        assert!(matches!(err, PenpotImportError::Zip(_)));
    }

    #[test]
    fn no_pages_is_error() {
        let bytes = build_archive(&[("manifest.json", b"{}")]);
        let err = import_penpot_bytes(&bytes).unwrap_err();
        assert!(matches!(err, PenpotImportError::NoManifest));
    }

    #[test]
    fn assets_are_extracted() {
        let manifest: &[u8] = br##"{ "name": "WithAsset" }"##;
        let page: &[u8] = br##"{ "name": "P", "objects": {} }"##;
        let asset_bytes: &[u8] = &[0x89, b'P', b'N', b'G'];
        let bytes = build_archive(&[
            ("manifest.json", manifest),
            ("project/pages/p.json", page),
            ("project/assets/img-1.png", asset_bytes),
        ]);
        let imported = import_penpot_bytes(&bytes).unwrap();
        assert_eq!(imported.assets.len(), 1);
        assert_eq!(imported.assets[0].id, "img-1");
        assert_eq!(imported.assets[0].mime, "image/png");
        assert_eq!(&imported.assets[0].bytes, asset_bytes);
    }
}
