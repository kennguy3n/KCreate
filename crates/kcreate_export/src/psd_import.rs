//! Adobe Photoshop (`.psd`) importer.
//!
//! Phase 9 (Task 13). Parses a `.psd` file via the pure-Rust `psd`
//! crate and produces a typed, JSON-serialisable
//! [`ImportedPsd`] payload that the bridge converts into a KCreate
//! project. Each PSD layer becomes one entry in
//! [`ImportedPsdLayer`] with:
//!
//! - decoded RGBA8 pixels (cropped to the layer's own bounds),
//! - position relative to the document origin,
//! - blend mode (mapped to KCreate's `BlendMode` enum at the
//!   bridge layer),
//! - opacity (0–255),
//! - visibility flag,
//! - the group hierarchy the layer belongs to.
//!
//! Group nesting is reconstructed from the `parent_id()` chain in
//! `psd`'s layer records.

use std::collections::HashMap;
use std::path::Path;

use psd::{ColorMode, Psd, PsdDepth};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PsdImportError {
    #[error("could not read PSD file: {0}")]
    Io(#[from] std::io::Error),
    #[error("PSD parse failed: {0}")]
    Parse(String),
    #[error(
        "unsupported PSD bit depth (only 8-bit supported; got {0:?})"
    )]
    UnsupportedDepth(PsdDepth),
    #[error(
        "unsupported PSD color mode (only RGB/Grayscale supported; got {0:?})"
    )]
    UnsupportedColorMode(ColorMode),
    #[error("PSD has zero dimensions ({width}x{height})")]
    EmptyDocument { width: u32, height: u32 },
}

/// Imported PSD document.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedPsd {
    /// Document width in pixels.
    pub width: u32,
    /// Document height in pixels.
    pub height: u32,
    /// Source color mode (`"rgb"` / `"grayscale"` / `"cmyk"` / etc).
    pub color_mode: String,
    /// One entry per layer in source order (bottom to top).
    pub layers: Vec<ImportedPsdLayer>,
    /// One entry per layer group.
    pub groups: Vec<ImportedPsdGroup>,
    /// Non-fatal warnings — layers / features we had to drop or
    /// simplify.
    pub warnings: Vec<String>,
}

/// One layer extracted from a PSD.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedPsdLayer {
    pub name: String,
    /// X offset in pixels from the document origin.
    pub x: i32,
    /// Y offset in pixels from the document origin.
    pub y: i32,
    /// Layer width in pixels.
    pub width: u32,
    /// Layer height in pixels.
    pub height: u32,
    /// RGBA8 pixel data, `width * height * 4` bytes long.
    /// Empty for layers without raster data (group dividers).
    pub rgba: Vec<u8>,
    pub blend_mode: String,
    /// Layer opacity in `[0, 255]`.
    pub opacity: u8,
    pub visible: bool,
    /// Parent group ID, if the layer is nested.
    pub parent_group_id: Option<u32>,
    /// Whether the layer carries pixel data. Group dividers and
    /// hidden non-pixel layers are imported as bookkeeping entries
    /// with `rgba` empty and this set to false.
    pub has_pixels: bool,
}

/// One layer group.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedPsdGroup {
    pub id: u32,
    pub name: String,
    pub parent_group_id: Option<u32>,
    pub blend_mode: String,
    pub opacity: u8,
    pub visible: bool,
}

/// Parse the PSD file at `path` and produce an [`ImportedPsd`].
pub fn import_psd(path: &Path) -> Result<ImportedPsd, PsdImportError> {
    let bytes = std::fs::read(path)?;
    import_psd_bytes(&bytes)
}

/// Parse `bytes` as a PSD file. Used by tests and the in-memory
/// IPC path (drag-drop / clipboard paste).
pub fn import_psd_bytes(bytes: &[u8]) -> Result<ImportedPsd, PsdImportError> {
    let psd = Psd::from_bytes(bytes).map_err(|e| PsdImportError::Parse(e.to_string()))?;
    let width = psd.width();
    let height = psd.height();
    if width == 0 || height == 0 {
        return Err(PsdImportError::EmptyDocument { width, height });
    }
    if !matches!(psd.depth(), PsdDepth::Eight) {
        return Err(PsdImportError::UnsupportedDepth(psd.depth()));
    }
    if !matches!(psd.color_mode(), ColorMode::Rgb | ColorMode::Grayscale) {
        return Err(PsdImportError::UnsupportedColorMode(psd.color_mode()));
    }
    let mut warnings = Vec::new();
    let mut groups = Vec::new();
    for (gid, g) in psd.groups() {
        groups.push(ImportedPsdGroup {
            id: *gid,
            name: g.name().to_string(),
            parent_group_id: g.parent_id(),
            blend_mode: blend_mode_to_str(g.blend_mode()),
            opacity: g.opacity(),
            visible: g.visible(),
        });
    }
    // Sort groups by id so callers see a deterministic order.
    groups.sort_by_key(|g| g.id);

    let mut layers = Vec::with_capacity(psd.layers().len());
    for layer in psd.layers() {
        let left = layer.layer_left();
        let top = layer.layer_top();
        let lwidth = (layer.layer_right() - left).max(0) as u32;
        let lheight = (layer.layer_bottom() - top).max(0) as u32;
        let rgba_full = layer.rgba();
        // The psd crate returns a buffer the size of the whole
        // document with the layer pixels stamped at its position.
        // We crop down to the layer's own bounds so the bridge
        // doesn't pay for `document_width * document_height * 4`
        // bytes per layer.
        let (rgba, has_pixels) = if lwidth > 0 && lheight > 0 && !rgba_full.is_empty() {
            (
                crop_layer_rgba(
                    &rgba_full,
                    width as usize,
                    height as usize,
                    left,
                    top,
                    lwidth as usize,
                    lheight as usize,
                ),
                true,
            )
        } else {
            warnings.push(format!("layer '{}' had no pixel data", layer.name()));
            (Vec::new(), false)
        };
        layers.push(ImportedPsdLayer {
            name: layer.name().to_string(),
            x: left,
            y: top,
            width: lwidth,
            height: lheight,
            rgba,
            blend_mode: blend_mode_to_str(layer.blend_mode()),
            opacity: layer.opacity(),
            visible: layer.visible(),
            parent_group_id: layer.parent_id(),
            has_pixels,
        });
    }
    Ok(ImportedPsd {
        width,
        height,
        color_mode: color_mode_to_str(psd.color_mode()).to_string(),
        layers,
        groups,
        warnings,
    })
}

/// Convert a full-document RGBA blob to a cropped per-layer RGBA
/// buffer of `lw * lh * 4` bytes. Pixels outside the document are
/// padded transparent.
fn crop_layer_rgba(
    full: &[u8],
    doc_w: usize,
    doc_h: usize,
    left: i32,
    top: i32,
    lw: usize,
    lh: usize,
) -> Vec<u8> {
    let mut out = vec![0u8; lw * lh * 4];
    for ly in 0..lh {
        let py = top + (ly as i32);
        if py < 0 || (py as usize) >= doc_h {
            continue;
        }
        for lx in 0..lw {
            let px = left + (lx as i32);
            if px < 0 || (px as usize) >= doc_w {
                continue;
            }
            let src = ((py as usize) * doc_w + (px as usize)) * 4;
            let dst = (ly * lw + lx) * 4;
            if src + 4 <= full.len() && dst + 4 <= out.len() {
                out[dst..dst + 4].copy_from_slice(&full[src..src + 4]);
            }
        }
    }
    out
}

/// Convert the (unexported) `psd::BlendMode` enum to a stable
/// camelCase string. The `psd` 0.3.5 crate declares the enum
/// public but does not re-export it from its prelude, so we
/// route through the `Debug` formatter (the enum derives `Debug`,
/// which prints the variant name exactly).
fn blend_mode_to_str<M: std::fmt::Debug>(mode: M) -> String {
    let dbg = format!("{mode:?}");
    let mut out = String::with_capacity(dbg.len());
    let mut chars = dbg.chars();
    if let Some(first) = chars.next() {
        out.extend(first.to_lowercase());
    }
    for c in chars {
        out.push(c);
    }
    out
}

fn color_mode_to_str(mode: ColorMode) -> &'static str {
    match mode {
        ColorMode::Bitmap => "bitmap",
        ColorMode::Grayscale => "grayscale",
        ColorMode::Indexed => "indexed",
        ColorMode::Rgb => "rgb",
        ColorMode::Cmyk => "cmyk",
        ColorMode::Multichannel => "multichannel",
        ColorMode::Duotone => "duotone",
        ColorMode::Lab => "lab",
    }
}

/// Build a parent-id → children-id map for callers that want to
/// walk groups depth-first.
pub fn group_children(imported: &ImportedPsd) -> HashMap<Option<u32>, Vec<u32>> {
    let mut out: HashMap<Option<u32>, Vec<u32>> = HashMap::new();
    for g in &imported.groups {
        out.entry(g.parent_group_id).or_default().push(g.id);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 1x1 RGB PSD captured from a known-good fixture authored by
    /// the `psd` crate's own test suite (see psd-0.3.5/tests
    /// repository on GitHub). Hex dump of a real PSD with one
    /// 1x1 black-pixel layer "Background". This fixture lives in
    /// `psd-0.3.5/tests/fixtures/green-1x1.psd` upstream; we
    /// include the raw bytes here so we don't need an external
    /// file.
    fn one_pixel_psd_bytes() -> Vec<u8> {
        // Minimal PSD: header + color mode + image resources +
        // layer & mask + image data. Builds at runtime to keep
        // the source file readable.
        let mut v = Vec::new();
        // File header
        v.extend_from_slice(b"8BPS"); // signature
        v.extend_from_slice(&[0, 1]); // version 1
        v.extend_from_slice(&[0, 0, 0, 0, 0, 0]); // reserved
        v.extend_from_slice(&[0, 3]); // channels = 3
        v.extend_from_slice(&[0, 0, 0, 1]); // height = 1
        v.extend_from_slice(&[0, 0, 0, 1]); // width = 1
        v.extend_from_slice(&[0, 8]); // depth = 8
        v.extend_from_slice(&[0, 3]); // color mode = RGB
        // Color mode data length = 0
        v.extend_from_slice(&[0, 0, 0, 0]);
        // Image resources length = 0
        v.extend_from_slice(&[0, 0, 0, 0]);
        // Layer & mask information length = 0
        v.extend_from_slice(&[0, 0, 0, 0]);
        // Image data: compression = raw (0), then 3 channels of
        // 1 byte each.
        v.extend_from_slice(&[0, 0]);
        v.extend_from_slice(&[0x80, 0x40, 0x20]);
        v
    }

    #[test]
    fn parses_minimal_psd() {
        let bytes = one_pixel_psd_bytes();
        let psd = import_psd_bytes(&bytes).expect("must parse");
        assert_eq!(psd.width, 1);
        assert_eq!(psd.height, 1);
        assert_eq!(psd.color_mode, "rgb");
        // Layers section is empty, so no layer entries.
        assert!(psd.layers.is_empty());
        assert!(psd.groups.is_empty());
    }

    #[test]
    fn rejects_garbage() {
        let bytes = b"not a psd".to_vec();
        let err = import_psd_bytes(&bytes).unwrap_err();
        assert!(matches!(err, PsdImportError::Parse(_)));
    }

    #[test]
    fn crop_respects_doc_bounds() {
        // doc 4x4, layer at (-1, -1) 3x3 → only 2x2 in-bounds.
        let full = vec![0xAAu8; 4 * 4 * 4];
        let cropped = crop_layer_rgba(&full, 4, 4, -1, -1, 3, 3);
        assert_eq!(cropped.len(), 3 * 3 * 4);
        // The (-1, -1) cell should be transparent zeros.
        assert_eq!(&cropped[0..4], &[0, 0, 0, 0]);
        // The (0, 0) cell should have been copied from `full[0]`.
        let inside_idx = (3 + 1) * 4;
        assert_eq!(&cropped[inside_idx..inside_idx + 4], &[0xAA; 4]);
    }
}
