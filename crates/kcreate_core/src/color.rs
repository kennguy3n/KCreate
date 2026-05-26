//! Color management — multi-color-space `Color` enum, ICC profile
//! references, document-level color settings, and the pure-math
//! conversions used by the renderer and PDF/print exporters.
//!
//! Phase 2 ships **non-ICC** conversions: the math here is the same
//! formulae you'd find in CSS Color Module Level 4 or Adobe's
//! Postscript reference. Full ICC profile transforms (Adobe RGB,
//! FOGRA39 with dot gain, etc.) land in Phase 3 — at that point the
//! [`IccProfile`] reference will resolve to a loaded LCMS/quark
//! profile via the export plane; the renderer will keep using sRGB
//! for the editing path.
//!
//! Design notes:
//!
//! - [`Color`] is an enum so each fill stores values in its native
//!   space. This is critical for CMYK print workflows where a 100% K
//!   ink fill must round-trip without being mangled by an RGB
//!   intermediate.
//! - All conversions go through sRGB / CIE XYZ as the connection
//!   space. The CMYK conversion is the standard "naive" formula —
//!   good for previews and CSS-style output, replaced by an ICC
//!   transform in Phase 3.
//! - The module has no external dependencies beyond `serde` /
//!   `thiserror`; this keeps the editing path local-first and the
//!   crate graph small.

use serde::{Deserialize, Serialize};

/// Represents a color in any supported color space.
///
/// The renderer always converts to sRGB for display via
/// [`Color::to_srgb`]; export pipelines (PDF / icc-tagged PNG) keep
/// the value in its native space until the very last step so a CMYK
/// fill stays CMYK on disk.
///
/// Wire format is serde's default **externally-tagged** PascalCase
/// JSON (e.g. `{"Srgb":{"r":1.0,"g":0.0,"b":0.0,"a":1.0}}`). This
/// matches the `ColorValue` type in `apps/desktop/shared/scene.ts`
/// and the lockstep contract in AGENTS.md §Rules §4.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Color {
    /// IEC 61966-2-1 sRGB. Channels in `[0.0, 1.0]`.
    Srgb { r: f32, g: f32, b: f32, a: f32 },
    /// DeviceCMYK ink coverages in `[0.0, 1.0]` (i.e. 1.0 = 100% ink).
    Cmyk {
        c: f32,
        m: f32,
        y: f32,
        k: f32,
        a: f32,
    },
    /// CIELAB D65. `l` in `[0.0, 100.0]`, `a_star` / `b_star` in
    /// roughly `[-128.0, 128.0]`. `alpha` in `[0.0, 1.0]`.
    Lab {
        l: f32,
        a_star: f32,
        b_star: f32,
        alpha: f32,
    },
    /// HSL — hue in degrees `[0, 360)`, saturation and lightness in
    /// `[0.0, 1.0]`.
    Hsl { h: f32, s: f32, l: f32, a: f32 },
    /// Named spot ink (Pantone, Toyo, custom). The `fallback_cmyk`
    /// tuple is the screen / RGB-tagged-PDF approximation; the spot
    /// name is what the print pipeline uses to emit a separation
    /// plate. `tint` in `[0.0, 1.0]` scales the ink load (so a 50 %
    /// tint of a spot is the same plate at half coverage).
    Spot {
        name: String,
        fallback_cmyk: (f32, f32, f32, f32),
        tint: f32,
        alpha: f32,
    },
}

impl Color {
    /// Convenience constructor for opaque sRGB.
    #[must_use]
    pub const fn srgb(r: f32, g: f32, b: f32) -> Self {
        Self::Srgb { r, g, b, a: 1.0 }
    }

    /// Convenience constructor for opaque CMYK.
    #[must_use]
    pub const fn cmyk(c: f32, m: f32, y: f32, k: f32) -> Self {
        Self::Cmyk { c, m, y, k, a: 1.0 }
    }

    /// Convert this color to sRGB (channels in `[0.0, 1.0]`, alpha
    /// preserved). The renderer calls this once per fill per frame.
    #[must_use]
    pub fn to_srgb(&self) -> (f32, f32, f32, f32) {
        match *self {
            Self::Srgb { r, g, b, a } => (r, g, b, a),
            Self::Cmyk { c, m, y, k, a } => {
                let (r, g, b) = cmyk_to_srgb(c, m, y, k);
                (r, g, b, a)
            }
            Self::Lab {
                l,
                a_star,
                b_star,
                alpha,
            } => {
                let (r, g, b) = lab_to_srgb(l, a_star, b_star);
                (r, g, b, alpha)
            }
            Self::Hsl { h, s, l, a } => {
                let (r, g, b) = hsl_to_srgb(h, s, l);
                (r, g, b, a)
            }
            Self::Spot {
                fallback_cmyk: (c, m, y, k),
                tint,
                alpha,
                ..
            } => {
                // Spot tints scale the ink load before the screen-CMYK
                // approximation. The result is the soft-proof
                // preview; the export pipeline uses the spot name to
                // emit a real separation plate.
                let tinted = (c * tint, m * tint, y * tint, k * tint);
                let (r, g, b) = cmyk_to_srgb(tinted.0, tinted.1, tinted.2, tinted.3);
                (r, g, b, alpha)
            }
        }
    }

    /// Alpha channel in `[0.0, 1.0]`, regardless of space.
    #[must_use]
    pub fn alpha(&self) -> f32 {
        match *self {
            Self::Srgb { a, .. } | Self::Cmyk { a, .. } | Self::Hsl { a, .. } => a,
            Self::Lab { alpha, .. } | Self::Spot { alpha, .. } => alpha,
        }
    }

    /// Whether this color is stored in a non-sRGB device space (CMYK).
    /// Used by the PDF exporter to decide between `rg` / `k` operators.
    /// Spot inks are CMYK-routed because they fall back to a CMYK
    /// approximation when a separation plate is unavailable.
    #[must_use]
    pub fn is_device_cmyk(&self) -> bool {
        matches!(self, Self::Cmyk { .. } | Self::Spot { .. })
    }

    /// Total per-pixel ink coverage for preflight checks. Returns
    /// `0..=400 %` (1.0 per process plate * 4 plates). Spot inks
    /// contribute their `tint` × their fallback-CMYK sum, since the
    /// preflight model doesn't yet know if the press will emit them
    /// as separations or composite. The PDF exporter applies a more
    /// accurate model when a `SpotColorLibrary` is available.
    #[must_use]
    pub fn total_ink_coverage(&self) -> f32 {
        match self {
            Self::Cmyk { c, m, y, k, .. } => c + m + y + k,
            Self::Spot {
                fallback_cmyk: (c, m, y, k),
                tint,
                ..
            } => (c + m + y + k) * tint,
            _ => {
                let (r, g, b, _) = self.to_srgb();
                // Convert preview to CMYK via the same naive
                // transform we use elsewhere for non-CMYK fills.
                let (c, m, y, k) = srgb_to_cmyk(r, g, b);
                c + m + y + k
            }
        }
    }
}

/// Library of named spot inks declared on a document.
///
/// Each entry is `name -> SpotColorDef` so the PDF exporter and the
/// preflight engine can look up the canonical CMYK fallback + display
/// name regardless of which fill referenced the spot. Documents are
/// expected to register every spot they use up-front; preflight
/// flags any [`Color::Spot`] whose `name` is missing from the
/// library.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SpotColorLibrary {
    pub entries: std::collections::BTreeMap<String, SpotColorDef>,
}

/// One spot ink in a [`SpotColorLibrary`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpotColorDef {
    /// Display name (e.g. `"Pantone 185 C"`).
    pub display_name: String,
    /// CMYK fallback used when the press cannot render a separation.
    pub fallback_cmyk: (f32, f32, f32, f32),
    /// Optional Pantone / Toyo / custom reference code for the swatch
    /// library identifier. `None` means the spot is anonymous /
    /// document-local.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub library_reference: Option<String>,
}

impl SpotColorLibrary {
    /// Build an empty library.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a spot ink under `name`. Replaces an existing entry.
    pub fn insert(&mut self, name: impl Into<String>, def: SpotColorDef) {
        self.entries.insert(name.into(), def);
    }

    /// Look up a spot ink by name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&SpotColorDef> {
        self.entries.get(name)
    }

    /// Number of spot inks defined.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// `true` when no spots are defined.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Iterate `(name, def)` pairs in alphabetical order.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &SpotColorDef)> {
        self.entries.iter()
    }

    /// Merge entries from `other` into this library. Existing entries
    /// with the same `name` are overwritten. Used by the Phase-3
    /// catalog system when a project loads multiple named libraries
    /// (e.g. Pantone Solid Coated + Pantone Solid Uncoated) — the
    /// last one wins for any colliding swatch names.
    pub fn merge(&mut self, other: Self) {
        for (name, def) in other.entries {
            self.entries.insert(name, def);
        }
    }

    /// Parse a Pantone-style JSON catalog.
    ///
    /// The expected shape is one of:
    ///
    /// ```json
    /// {
    ///   "name": "Pantone Solid Coated",
    ///   "entries": [
    ///     {
    ///       "id": "PANTONE 185 C",
    ///       "display_name": "Pantone 185 C",
    ///       "cmyk": [0.0, 1.0, 0.84, 0.0]
    ///     },
    ///     ...
    ///   ]
    /// }
    /// ```
    ///
    /// or a bare object map:
    ///
    /// ```json
    /// {
    ///   "PANTONE 185 C": { "display_name": "Pantone 185 C", "cmyk": [0.0, 1.0, 0.84, 0.0] },
    ///   ...
    /// }
    /// ```
    ///
    /// The bare map form is convenient for hand-authored small
    /// catalogues; the wrapped form is what we ship for the canonical
    /// libraries because it carries metadata (catalogue name) the UI
    /// surfaces. `library_reference` defaults to the entry id when
    /// not explicitly set.
    ///
    /// CMYK channels are clamped to `[0.0, 1.0]`. Entries with an
    /// invalid CMYK array (wrong length, non-finite values) are
    /// skipped rather than failing the whole catalogue — a single
    /// corrupted swatch should not lock the user out of all the
    /// others.
    pub fn from_json_catalog(raw: &str) -> Result<Self, SpotCatalogError> {
        let value: serde_json::Value =
            serde_json::from_str(raw).map_err(|e| SpotCatalogError::Parse(e.to_string()))?;
        let mut out = Self::default();
        match value {
            serde_json::Value::Object(map) => {
                if let Some(entries) = map.get("entries").and_then(|v| v.as_array()) {
                    for entry in entries {
                        if let Some((name, def)) = parse_catalog_entry_object(entry) {
                            out.entries.insert(name, def);
                        }
                    }
                } else {
                    // Bare map form. The map key IS the swatch id —
                    // entries may omit `id` entirely and only carry
                    // `display_name` + `cmyk`, or even just a bare
                    // 4-element CMYK array.
                    for (name, entry) in map {
                        // Skip top-level metadata fields a catalogue
                        // may carry alongside swatches (e.g. "name",
                        // "description", "library_reference"). These
                        // are strings, not entry objects.
                        if entry.is_string() || entry.is_null() {
                            continue;
                        }
                        if let Some(def) = parse_bare_entry(&name, &entry) {
                            out.entries.insert(name, def);
                        }
                    }
                }
            }
            _ => return Err(SpotCatalogError::Shape),
        }
        Ok(out)
    }
}

/// Failure modes for [`SpotColorLibrary::from_json_catalog`].
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SpotCatalogError {
    /// `serde_json` couldn't parse the input as JSON.
    #[error("invalid JSON: {0}")]
    Parse(String),
    /// The JSON parsed but doesn't have the expected `{entries: [...]}`
    /// or bare-object shape.
    #[error("expected `{{ entries: [...] }}` or a bare object map of `name -> entry`")]
    Shape,
}

/// Parse one entry from the bare-map form of a Pantone catalogue.
///
/// The key `name` is already known (it's the JSON object key), so
/// the entry value may either be:
///
/// * a 4-element JSON array `[c, m, y, k]` — only the CMYK fallback,
/// * a JSON object `{ "display_name"?: ..., "cmyk"?: [..] }` — full
///   inline definition. The `id` field is not required in this form;
///   if present it's ignored in favour of the map key, which keeps
///   `lib.get(key)` lookups working.
///
/// Returns `None` for anything else (so the caller can drop the
/// malformed entry without poisoning the rest of the library).
fn parse_bare_entry(name: &str, entry: &serde_json::Value) -> Option<SpotColorDef> {
    if let Some(cmyk) = parse_bare_cmyk_array(entry) {
        return Some(SpotColorDef {
            display_name: name.to_string(),
            fallback_cmyk: cmyk,
            library_reference: Some(name.to_string()),
        });
    }
    let obj = entry.as_object()?;
    let cmyk = obj
        .get("cmyk")
        .or_else(|| obj.get("fallback_cmyk"))
        .or_else(|| obj.get("fallbackCmyk"))
        .and_then(parse_bare_cmyk_array)?;
    let display_name = obj
        .get("display_name")
        .and_then(|v| v.as_str())
        .or_else(|| obj.get("displayName").and_then(|v| v.as_str()))
        .unwrap_or(name)
        .to_string();
    let library_reference = obj
        .get("library_reference")
        .or_else(|| obj.get("libraryReference"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| Some(name.to_string()));
    Some(SpotColorDef {
        display_name,
        fallback_cmyk: cmyk,
        library_reference,
    })
}

fn parse_catalog_entry_object(entry: &serde_json::Value) -> Option<(String, SpotColorDef)> {
    let obj = entry.as_object()?;
    let id = obj
        .get("id")
        .and_then(|v| v.as_str())
        .or_else(|| obj.get("name").and_then(|v| v.as_str()))?
        .to_string();
    let display_name = obj
        .get("display_name")
        .and_then(|v| v.as_str())
        .or_else(|| obj.get("displayName").and_then(|v| v.as_str()))
        .unwrap_or(id.as_str())
        .to_string();
    let cmyk = obj
        .get("cmyk")
        .or_else(|| obj.get("fallback_cmyk"))
        .or_else(|| obj.get("fallbackCmyk"))
        .and_then(parse_bare_cmyk_array)?;
    let library_reference = obj
        .get("library_reference")
        .or_else(|| obj.get("libraryReference"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| Some(id.clone()));
    Some((
        id,
        SpotColorDef {
            display_name,
            fallback_cmyk: cmyk,
            library_reference,
        },
    ))
}

fn parse_bare_cmyk_array(value: &serde_json::Value) -> Option<(f32, f32, f32, f32)> {
    let arr = value.as_array()?;
    if arr.len() != 4 {
        return None;
    }
    let mut out = [0.0f32; 4];
    for (i, v) in arr.iter().enumerate() {
        let n = v.as_f64()?;
        if !n.is_finite() {
            return None;
        }
        out[i] = (n as f32).clamp(0.0, 1.0);
    }
    Some(out.into())
}

/// Total ink coverage for a colour resolved against a spot library.
///
/// For [`Color::Spot`], this looks up the library entry to use the
/// document-declared CMYK fallback (which may differ from the inline
/// `fallback_cmyk` on the colour, e.g. when the spot was re-tinted at
/// the document level after the fill was authored). Falls back to
/// the colour's own `total_ink_coverage` if the spot isn't in the
/// library.
#[must_use]
pub fn total_ink_coverage_with_spots(color: &Color, spots: &SpotColorLibrary) -> f32 {
    if let Color::Spot { name, tint, .. } = color {
        if let Some(def) = spots.get(name) {
            let (c, m, y, k) = def.fallback_cmyk;
            return (c + m + y + k) * tint;
        }
    }
    color.total_ink_coverage()
}

/// Color-space taxonomy for [`IccProfile::Custom`].
///
/// The well-known [`IccProfile`] variants pin their own color space
/// statically (sRGB / Adobe RGB / P3 are RGB, FOGRA39 / SWOP 2006 are
/// CMYK). A `Custom` profile is just a blob hash + label — the runtime
/// doesn't load the ICC bytes yet (Phase 3), so this enum lets the
/// document author *declare* what color space the embedded profile
/// represents. Downstream code (PDF export, soft-proof overlay,
/// `IccProfile::is_cmyk`) keys off this declaration instead of
/// special-casing only the well-known names.
///
/// Wire format is serde's default PascalCase unit-variant strings
/// (`"Rgb"`, `"Cmyk"`, …). Mirrors `IccColorSpace` in
/// `apps/desktop/shared/scene.ts`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum IccColorSpace {
    /// Three-channel RGB (sRGB / Adobe RGB / Display P3 / wide-gamut
    /// monitor profiles). The default for newly authored Custom
    /// profiles so older project files (which omit the field) keep
    /// the historical interpretation.
    #[default]
    Rgb,
    /// Four-channel process CMYK (FOGRA39, SWOP, GRACoL, custom
    /// press profiles). Setting this on a Custom profile is what
    /// activates the CMYK PDF export pipeline and the soft-proof
    /// CMYK simulation.
    Cmyk,
    /// Single-channel grayscale (DeviceGray, Dot Gain 20%, etc.).
    Gray,
    /// Profile-Connection-Space Lab — used as an intermediate by
    /// some workflows and PDF/X output intents.
    Lab,
}

/// ICC profile reference — either a well-known standard or a custom
/// embedded profile.
///
/// The runtime does not currently load `.icc` blobs (that's Phase 3);
/// this enum exists so documents can serialise their intent and
/// downstream tools (e.g. PDF/X-3 export) know which output intent to
/// embed.
/// Wire format is serde's default **externally-tagged** PascalCase
/// JSON: unit variants serialize as bare strings (`"SrgbIec61966"`,
/// `"FogRa39"`, …) and the `Custom` variant as
/// `{"Custom":{"name":"…","blob_hash":"…","color_space":"Rgb"}}`. This
/// matches the `IccProfile` type in `apps/desktop/shared/scene.ts` and
/// the lockstep contract in AGENTS.md §Rules §4. The `color_space`
/// field is `#[serde(default)]` so projects written before the field
/// existed still load (they get the historical `Rgb` interpretation).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum IccProfile {
    /// IEC 61966-2-1 sRGB. The default working space.
    SrgbIec61966,
    /// Adobe RGB (1998).
    AdobeRgb1998,
    /// Display P3.
    DisplayP3,
    /// FOGRA39 — European offset CMYK (ISO 12647-2 PSO LWC Improved).
    FogRa39,
    /// SWOP 2006 Coated #3 — US web offset CMYK.
    Swop2006,
    /// A custom profile embedded in the project; `blob_hash` is the
    /// BLAKE3 hash of the `.icc` bytes in the asset store, and
    /// `color_space` declares which device space the profile targets
    /// so [`IccProfile::is_cmyk`] and the PDF exporter can route it
    /// correctly without having to parse the embedded ICC header.
    Custom {
        name: String,
        blob_hash: String,
        #[serde(default)]
        color_space: IccColorSpace,
    },
}

impl IccProfile {
    /// Human-readable label for UI menus.
    #[must_use]
    pub fn display_name(&self) -> String {
        match self {
            Self::SrgbIec61966 => "sRGB (IEC 61966-2-1)".to_string(),
            Self::AdobeRgb1998 => "Adobe RGB (1998)".to_string(),
            Self::DisplayP3 => "Display P3".to_string(),
            Self::FogRa39 => "FOGRA39 (ISO 12647-2)".to_string(),
            Self::Swop2006 => "SWOP 2006 Coated #3".to_string(),
            Self::Custom { name, .. } => name.clone(),
        }
    }

    /// Whether this profile represents a CMYK working space.
    ///
    /// Returns `true` for the well-known process-CMYK profiles
    /// (FOGRA39, SWOP 2006) **and** for any `Custom` profile the
    /// author declared as [`IccColorSpace::Cmyk`]. The previous
    /// implementation only matched on the two well-known names and
    /// silently returned `false` for custom CMYK profiles, which
    /// would have routed them through the RGB code path in any
    /// future consumer keyed off `is_cmyk()`. Per Devin Review on
    /// PR #7 (Phase 2 / color.rs:167).
    #[must_use]
    pub const fn is_cmyk(&self) -> bool {
        matches!(
            self,
            Self::FogRa39
                | Self::Swop2006
                | Self::Custom {
                    color_space: IccColorSpace::Cmyk,
                    ..
                }
        )
    }
}

/// ICC rendering intent.
///
/// Mirrors the four standard ICC intents. The Phase 2 soft-proof
/// simulation uses [`RenderingIntent::Perceptual`] as the default; the
/// PDF exporter writes the chosen intent into the output intent
/// dictionary.
///
/// Wire format is serde's default **PascalCase** unit-variant strings
/// (e.g. `"Perceptual"`, `"RelativeColorimetric"`). This matches the
/// `RenderingIntent` type in `apps/desktop/shared/scene.ts` and the
/// lockstep contract in AGENTS.md §Rules §4.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RenderingIntent {
    #[default]
    Perceptual,
    RelativeColorimetric,
    Saturation,
    AbsoluteColorimetric,
}

/// Document-level color management settings.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ColorSettings {
    /// Working RGB space — defaults to sRGB.
    pub working_space_rgb: IccProfile,
    /// Working CMYK space — `None` means "no CMYK output planned".
    pub working_space_cmyk: Option<IccProfile>,
    /// How to handle out-of-gamut colors during conversion.
    pub rendering_intent: RenderingIntent,
    /// If `Some(...)`, the canvas simulates the named output profile.
    pub soft_proof_profile: Option<IccProfile>,
    /// Highlight pixels outside the proof gamut with a warning color.
    pub gamut_warning: bool,
}

impl Default for ColorSettings {
    fn default() -> Self {
        Self {
            working_space_rgb: IccProfile::SrgbIec61966,
            working_space_cmyk: None,
            rendering_intent: RenderingIntent::Perceptual,
            soft_proof_profile: None,
            gamut_warning: false,
        }
    }
}

// ---------------------------------------------------------------------
// Pure-math conversions
// ---------------------------------------------------------------------

/// Standard "naive" sRGB → CMYK conversion. Inputs and outputs in
/// `[0.0, 1.0]`. Black is extracted via `min(1 - r, 1 - g, 1 - b)`.
#[must_use]
pub fn srgb_to_cmyk(r: f32, g: f32, b: f32) -> (f32, f32, f32, f32) {
    let r = r.clamp(0.0, 1.0);
    let g = g.clamp(0.0, 1.0);
    let b = b.clamp(0.0, 1.0);
    let k = 1.0 - r.max(g).max(b);
    if (1.0 - k).abs() < f32::EPSILON {
        return (0.0, 0.0, 0.0, 1.0);
    }
    let c = (1.0 - r - k) / (1.0 - k);
    let m = (1.0 - g - k) / (1.0 - k);
    let y = (1.0 - b - k) / (1.0 - k);
    (c.clamp(0.0, 1.0), m.clamp(0.0, 1.0), y.clamp(0.0, 1.0), k)
}

/// Standard "naive" CMYK → sRGB conversion. Inputs and outputs in
/// `[0.0, 1.0]`.
#[must_use]
pub fn cmyk_to_srgb(c: f32, m: f32, y: f32, k: f32) -> (f32, f32, f32) {
    let c = c.clamp(0.0, 1.0);
    let m = m.clamp(0.0, 1.0);
    let y = y.clamp(0.0, 1.0);
    let k = k.clamp(0.0, 1.0);
    let r = (1.0 - c) * (1.0 - k);
    let g = (1.0 - m) * (1.0 - k);
    let b = (1.0 - y) * (1.0 - k);
    (r, g, b)
}

/// sRGB → linear-light sRGB. Standard IEC 61966-2-1 transfer.
#[must_use]
pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// linear-light sRGB → sRGB. Inverse of [`srgb_to_linear`].
#[must_use]
pub fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// sRGB → CIE XYZ (D65 illuminant). Channels in `[0.0, 1.0]` ↦ XYZ
/// values where the D65 reference white is `(0.95047, 1.0, 1.08883)`.
///
/// Matrix from IEC 61966-2-1 (the conventional Bradford-adapted
/// sRGB → XYZ transform):
///
/// ```text
///     | X |   | 0.412 456 4   0.357 576 1   0.180 437 5 | | R |
///     | Y | = | 0.212 672 9   0.715 152 2   0.072 175 0 | | G |
///     | Z |   | 0.019 333 9   0.119 192 0   0.950 304 1 | | B |
/// ```
#[must_use]
pub fn srgb_to_xyz_d65(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let r = srgb_to_linear(r);
    let g = srgb_to_linear(g);
    let b = srgb_to_linear(b);
    let x = 0.180_437_5_f32.mul_add(b, 0.412_456_4_f32.mul_add(r, 0.357_576_1 * g));
    let y = 0.072_175_0_f32.mul_add(b, 0.212_672_9_f32.mul_add(r, 0.715_152_2 * g));
    let z = 0.950_304_1_f32.mul_add(b, 0.019_333_9_f32.mul_add(r, 0.119_192 * g));
    (x, y, z)
}

/// CIE XYZ (D65) → sRGB. Inverse of [`srgb_to_xyz_d65`].
///
/// ```text
///     | R |   |  3.240 454 2  -1.537 138 5  -0.498 531 4 | | X |
///     | G | = | -0.969 266 0   1.876 010 8   0.041 556 0 | | Y |
///     | B |   |  0.055 643 4  -0.204 025 9   1.057 225 2 | | Z |
/// ```
#[must_use]
pub fn xyz_d65_to_srgb(x: f32, y: f32, z: f32) -> (f32, f32, f32) {
    let r = (-0.498_531_4_f32).mul_add(z, 3.240_454_2_f32.mul_add(x, -1.537_138_5 * y));
    let g = 0.041_556_f32.mul_add(z, (-0.969_266_f32).mul_add(x, 1.876_010_8 * y));
    let b = 1.057_225_2_f32.mul_add(z, 0.055_643_4_f32.mul_add(x, -0.204_025_9 * y));
    (
        linear_to_srgb(r).clamp(0.0, 1.0),
        linear_to_srgb(g).clamp(0.0, 1.0),
        linear_to_srgb(b).clamp(0.0, 1.0),
    )
}

/// D65 white point in XYZ.
const D65_X: f32 = 0.950_47;
const D65_Y: f32 = 1.0;
const D65_Z: f32 = 1.088_83;

const LAB_EPSILON: f32 = 216.0 / 24_389.0;
const LAB_KAPPA: f32 = 24_389.0 / 27.0;

fn xyz_to_lab_f(t: f32) -> f32 {
    if t > LAB_EPSILON {
        t.cbrt()
    } else {
        LAB_KAPPA.mul_add(t, 16.0) / 116.0
    }
}

fn lab_to_xyz_f(ft: f32) -> f32 {
    let cube = ft * ft * ft;
    if cube > LAB_EPSILON {
        cube
    } else {
        ((ft * 116.0) - 16.0) / LAB_KAPPA
    }
}

/// sRGB → CIELAB (D65).
#[must_use]
pub fn srgb_to_lab(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let (x, y, z) = srgb_to_xyz_d65(r, g, b);
    let fx = xyz_to_lab_f(x / D65_X);
    let fy = xyz_to_lab_f(y / D65_Y);
    let fz = xyz_to_lab_f(z / D65_Z);
    let l = 116.0_f32.mul_add(fy, -16.0);
    let a = 500.0 * (fx - fy);
    let b_star = 200.0 * (fy - fz);
    (l, a, b_star)
}

/// CIELAB (D65) → sRGB.
#[must_use]
pub fn lab_to_srgb(l: f32, a_star: f32, b_star: f32) -> (f32, f32, f32) {
    let fy = (l + 16.0) / 116.0;
    let fx = a_star / 500.0 + fy;
    let fz = fy - b_star / 200.0;
    let x = D65_X * lab_to_xyz_f(fx);
    let y = D65_Y * lab_to_xyz_f(fy);
    let z = D65_Z * lab_to_xyz_f(fz);
    xyz_d65_to_srgb(x, y, z)
}

/// sRGB → HSL. Output: hue in degrees `[0, 360)`, saturation and
/// lightness in `[0.0, 1.0]`.
#[must_use]
pub fn srgb_to_hsl(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let r = r.clamp(0.0, 1.0);
    let g = g.clamp(0.0, 1.0);
    let b = b.clamp(0.0, 1.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = f32::midpoint(max, min);
    if (max - min).abs() < f32::EPSILON {
        return (0.0, 0.0, l);
    }
    let d = max - min;
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };
    let mut h = if (max - r).abs() < f32::EPSILON {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if (max - g).abs() < f32::EPSILON {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    h *= 60.0;
    if h >= 360.0 {
        h -= 360.0;
    }
    if h < 0.0 {
        h += 360.0;
    }
    (h, s, l)
}

/// HSL → sRGB. Hue in degrees, saturation and lightness in `[0, 1]`.
#[must_use]
pub fn hsl_to_srgb(h: f32, s: f32, l: f32) -> (f32, f32, f32) {
    let s = s.clamp(0.0, 1.0);
    let l = l.clamp(0.0, 1.0);
    if s.abs() < f32::EPSILON {
        return (l, l, l);
    }
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0_f32.mul_add(l, -q);
    let h_norm = (h.rem_euclid(360.0)) / 360.0;
    let r = hue_to_rgb(p, q, h_norm + 1.0 / 3.0);
    let g = hue_to_rgb(p, q, h_norm);
    let b = hue_to_rgb(p, q, h_norm - 1.0 / 3.0);
    (r, g, b)
}

fn hue_to_rgb(p: f32, q: f32, mut t: f32) -> f32 {
    if t < 0.0 {
        t += 1.0;
    }
    if t > 1.0 {
        t -= 1.0;
    }
    // Standard HSL→RGB sextant interpolation: `p + (q-p) * 6 * f(t)`.
    // The earlier `6.0.mul_add(q - p, p) * t + p - p` expansion was
    // algebraically wrong (the `+ p - p` cancelled the addend, leaving
    // `(6q - 5p) * t` instead of `p + (q-p)*6t`), so non-saturated
    // colors with `s < 1` produced visibly incorrect RGB. The current
    // form keeps a single fused multiply–add per branch and matches
    // the CSS Color Module Level 4 reference.
    if t < 1.0 / 6.0 {
        return (q - p).mul_add(6.0 * t, p);
    }
    if t < 1.0 / 2.0 {
        return q;
    }
    if t < 2.0 / 3.0 {
        return (q - p).mul_add(6.0 * (2.0 / 3.0 - t), p);
    }
    p
}

/// Perceptual color difference (CIE76). Larger = more different;
/// JND (just-noticeable difference) is approximately 2.3.
///
/// Inputs are `(l, a*, b*)` Lab triplets, e.g. from
/// [`srgb_to_lab`].
#[must_use]
pub fn color_distance_cie76(lab1: (f32, f32, f32), lab2: (f32, f32, f32)) -> f32 {
    let dl = lab1.0 - lab2.0;
    let da = lab1.1 - lab2.1;
    let db = lab1.2 - lab2.2;
    db.mul_add(db, dl.mul_add(dl, da * da)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f32 = 0.005;

    fn approx(a: f32, b: f32) {
        assert!((a - b).abs() < EPS, "expected {b} got {a}");
    }

    fn approx_eps(a: f32, b: f32, eps: f32) {
        assert!((a - b).abs() < eps, "expected {b} got {a} (eps {eps})");
    }

    #[test]
    fn srgb_white_to_cmyk_is_zero_ink() {
        let (c, m, y, k) = srgb_to_cmyk(1.0, 1.0, 1.0);
        approx(c, 0.0);
        approx(m, 0.0);
        approx(y, 0.0);
        approx(k, 0.0);
    }

    #[test]
    fn srgb_black_to_cmyk_is_pure_k() {
        let (c, m, y, k) = srgb_to_cmyk(0.0, 0.0, 0.0);
        approx(c, 0.0);
        approx(m, 0.0);
        approx(y, 0.0);
        approx(k, 1.0);
    }

    #[test]
    fn srgb_red_round_trip_cmyk() {
        let (c, m, y, k) = srgb_to_cmyk(1.0, 0.0, 0.0);
        approx(c, 0.0);
        approx(m, 1.0);
        approx(y, 1.0);
        approx(k, 0.0);
        let (r, g, b) = cmyk_to_srgb(c, m, y, k);
        approx(r, 1.0);
        approx(g, 0.0);
        approx(b, 0.0);
    }

    #[test]
    fn cmyk_pure_k_is_black() {
        let (r, g, b) = cmyk_to_srgb(0.0, 0.0, 0.0, 1.0);
        approx(r, 0.0);
        approx(g, 0.0);
        approx(b, 0.0);
    }

    #[test]
    fn srgb_to_xyz_d65_white_is_reference_white() {
        let (x, y, z) = srgb_to_xyz_d65(1.0, 1.0, 1.0);
        approx_eps(x, D65_X, 0.001);
        approx_eps(y, D65_Y, 0.001);
        approx_eps(z, D65_Z, 0.001);
    }

    #[test]
    fn srgb_to_lab_white_is_l_100() {
        let (l, a, b) = srgb_to_lab(1.0, 1.0, 1.0);
        approx_eps(l, 100.0, 0.05);
        approx_eps(a, 0.0, 0.05);
        approx_eps(b, 0.0, 0.05);
    }

    #[test]
    fn srgb_to_lab_black_is_l_0() {
        let (l, a, b) = srgb_to_lab(0.0, 0.0, 0.0);
        approx_eps(l, 0.0, 0.05);
        approx_eps(a, 0.0, 0.05);
        approx_eps(b, 0.0, 0.05);
    }

    #[test]
    fn lab_round_trip_for_red() {
        let (l, a, b) = srgb_to_lab(1.0, 0.0, 0.0);
        let (rr, gg, bb) = lab_to_srgb(l, a, b);
        approx_eps(rr, 1.0, 0.01);
        approx_eps(gg, 0.0, 0.01);
        approx_eps(bb, 0.0, 0.01);
    }

    #[test]
    fn hsl_round_trip_mid_gray() {
        let (h, s, l) = srgb_to_hsl(0.5, 0.5, 0.5);
        approx(h, 0.0);
        approx(s, 0.0);
        approx_eps(l, 0.5, 0.01);
        let (r, g, b) = hsl_to_srgb(h, s, l);
        approx_eps(r, 0.5, 0.01);
        approx_eps(g, 0.5, 0.01);
        approx_eps(b, 0.5, 0.01);
    }

    #[test]
    fn hsl_round_trip_saturated_red() {
        let (h, s, l) = srgb_to_hsl(1.0, 0.0, 0.0);
        approx_eps(h, 0.0, 0.1);
        approx_eps(s, 1.0, 0.01);
        approx_eps(l, 0.5, 0.01);
        let (r, g, b) = hsl_to_srgb(h, s, l);
        approx_eps(r, 1.0, 0.01);
        approx_eps(g, 0.0, 0.01);
        approx_eps(b, 0.0, 0.01);
    }

    #[test]
    fn hsl_round_trip_saturated_green() {
        let (r, g, b) = hsl_to_srgb(120.0, 1.0, 0.5);
        approx_eps(r, 0.0, 0.01);
        approx_eps(g, 1.0, 0.01);
        approx_eps(b, 0.0, 0.01);
    }

    #[test]
    fn hsl_round_trip_saturated_blue() {
        let (r, g, b) = hsl_to_srgb(240.0, 1.0, 0.5);
        approx_eps(r, 0.0, 0.01);
        approx_eps(g, 0.0, 0.01);
        approx_eps(b, 1.0, 0.01);
    }

    #[test]
    fn color_distance_identical_is_zero() {
        let lab = srgb_to_lab(0.4, 0.2, 0.8);
        approx_eps(color_distance_cie76(lab, lab), 0.0, 0.0001);
    }

    #[test]
    fn color_distance_white_to_black_is_l_100() {
        let white = srgb_to_lab(1.0, 1.0, 1.0);
        let black = srgb_to_lab(0.0, 0.0, 0.0);
        // White vs black is roughly 100 ΔE in CIE76.
        approx_eps(color_distance_cie76(white, black), 100.0, 0.5);
    }

    #[test]
    fn color_enum_to_srgb_for_each_space() {
        let srgb = Color::srgb(0.25, 0.5, 0.75);
        approx(srgb.to_srgb().0, 0.25);
        let cmyk = Color::cmyk(1.0, 0.0, 1.0, 0.0); // pure green
        let (r, g, b, _) = cmyk.to_srgb();
        approx(r, 0.0);
        approx(g, 1.0);
        approx(b, 0.0);
        let lab = Color::Lab {
            l: 100.0,
            a_star: 0.0,
            b_star: 0.0,
            alpha: 1.0,
        };
        let (r, g, b, _) = lab.to_srgb();
        approx_eps(r, 1.0, 0.01);
        approx_eps(g, 1.0, 0.01);
        approx_eps(b, 1.0, 0.01);
        let hsl = Color::Hsl {
            h: 0.0,
            s: 1.0,
            l: 0.5,
            a: 1.0,
        };
        let (r, g, b, _) = hsl.to_srgb();
        approx_eps(r, 1.0, 0.01);
        approx_eps(g, 0.0, 0.01);
        approx_eps(b, 0.0, 0.01);
    }

    #[test]
    fn color_settings_default_is_srgb_no_cmyk() {
        let s = ColorSettings::default();
        assert_eq!(s.working_space_rgb, IccProfile::SrgbIec61966);
        assert!(s.working_space_cmyk.is_none());
        assert_eq!(s.rendering_intent, RenderingIntent::Perceptual);
        assert!(s.soft_proof_profile.is_none());
        assert!(!s.gamut_warning);
    }

    #[test]
    fn icc_profile_serde_round_trip() {
        let profiles = vec![
            IccProfile::SrgbIec61966,
            IccProfile::AdobeRgb1998,
            IccProfile::DisplayP3,
            IccProfile::FogRa39,
            IccProfile::Swop2006,
            IccProfile::Custom {
                name: "MyMonitor".into(),
                blob_hash: "abcd".into(),
                color_space: IccColorSpace::Rgb,
            },
            IccProfile::Custom {
                name: "MyPress".into(),
                blob_hash: "deadbeef".into(),
                color_space: IccColorSpace::Cmyk,
            },
        ];
        for p in profiles {
            let s = serde_json::to_string(&p).unwrap();
            let back: IccProfile = serde_json::from_str(&s).unwrap();
            assert_eq!(p, back);
        }
    }

    #[test]
    fn icc_profile_custom_color_space_defaults_when_absent() {
        // Forward-compat: projects authored before the field existed
        // must still load. Deserialise a Custom payload without the
        // `color_space` field and verify it lands on `Rgb` (the
        // historical interpretation).
        let s = r#"{"Custom":{"name":"OldFile","blob_hash":"abc"}}"#;
        let back: IccProfile = serde_json::from_str(s).unwrap();
        match back {
            IccProfile::Custom {
                name,
                blob_hash,
                color_space,
            } => {
                assert_eq!(name, "OldFile");
                assert_eq!(blob_hash, "abc");
                assert_eq!(color_space, IccColorSpace::Rgb);
            }
            other => panic!("expected Custom, got {other:?}"),
        }
    }

    #[test]
    fn icc_profile_is_cmyk_matches_well_known_and_custom_cmyk() {
        // Well-known CMYK presses.
        assert!(IccProfile::FogRa39.is_cmyk());
        assert!(IccProfile::Swop2006.is_cmyk());
        // Well-known RGB / display profiles.
        assert!(!IccProfile::SrgbIec61966.is_cmyk());
        assert!(!IccProfile::AdobeRgb1998.is_cmyk());
        assert!(!IccProfile::DisplayP3.is_cmyk());
        // Custom profiles route off the declared color_space.
        assert!(IccProfile::Custom {
            name: "MyPress".into(),
            blob_hash: "deadbeef".into(),
            color_space: IccColorSpace::Cmyk,
        }
        .is_cmyk());
        assert!(!IccProfile::Custom {
            name: "MyMonitor".into(),
            blob_hash: "feedface".into(),
            color_space: IccColorSpace::Rgb,
        }
        .is_cmyk());
        // Gray and Lab Custom profiles are not CMYK.
        assert!(!IccProfile::Custom {
            name: "DotGain20".into(),
            blob_hash: "01".into(),
            color_space: IccColorSpace::Gray,
        }
        .is_cmyk());
        assert!(!IccProfile::Custom {
            name: "PCS-Lab".into(),
            blob_hash: "02".into(),
            color_space: IccColorSpace::Lab,
        }
        .is_cmyk());
    }

    #[test]
    fn color_serde_round_trip_all_variants() {
        let colors = vec![
            Color::Srgb {
                r: 0.1,
                g: 0.2,
                b: 0.3,
                a: 0.4,
            },
            Color::Cmyk {
                c: 0.1,
                m: 0.2,
                y: 0.3,
                k: 0.4,
                a: 0.5,
            },
            Color::Lab {
                l: 50.0,
                a_star: -10.0,
                b_star: 20.0,
                alpha: 0.7,
            },
            Color::Hsl {
                h: 200.0,
                s: 0.6,
                l: 0.4,
                a: 0.9,
            },
        ];
        for c in colors {
            let s = serde_json::to_string(&c).unwrap();
            let back: Color = serde_json::from_str(&s).unwrap();
            assert_eq!(c, back);
        }
    }

    #[test]
    fn cyan_named_color_to_cmyk() {
        // CSS named "cyan" = (0, 1, 1) sRGB. In CMYK that's pure C.
        let (c, m, y, k) = srgb_to_cmyk(0.0, 1.0, 1.0);
        approx(c, 1.0);
        approx(m, 0.0);
        approx(y, 0.0);
        approx(k, 0.0);
    }

    #[test]
    fn yellow_named_color_to_cmyk() {
        let (c, m, y, k) = srgb_to_cmyk(1.0, 1.0, 0.0);
        approx(c, 0.0);
        approx(m, 0.0);
        approx(y, 1.0);
        approx(k, 0.0);
    }

    #[test]
    fn magenta_named_color_to_cmyk() {
        let (c, m, y, k) = srgb_to_cmyk(1.0, 0.0, 1.0);
        approx(c, 0.0);
        approx(m, 1.0);
        approx(y, 0.0);
        approx(k, 0.0);
    }

    #[test]
    fn cmyk_is_device_cmyk_reports_true() {
        let c = Color::cmyk(0.1, 0.2, 0.3, 0.4);
        assert!(c.is_device_cmyk());
        assert!(!Color::srgb(0.1, 0.2, 0.3).is_device_cmyk());
    }

    #[test]
    fn hsl_unsaturated_orange_first_sextant() {
        // Regression for the earlier `(6q-5p)*t` bug in `hue_to_rgb`.
        // HSL(30, 0.5, 0.5) lives in the first sextant with `s < 1`,
        // which is the exact path that produced wrong RGB values when
        // the addend cancelled. CSS Color reference: hsl(30 50% 50%)
        // == rgb(191, 128, 64) == (0.75, 0.5, 0.25).
        let (r, g, b) = hsl_to_srgb(30.0, 0.5, 0.5);
        approx_eps(r, 0.75, 0.01);
        approx_eps(g, 0.5, 0.01);
        approx_eps(b, 0.25, 0.01);
    }

    #[test]
    fn hsl_unsaturated_teal_third_sextant() {
        // Same bug, third-sextant branch. CSS: hsl(210 50% 50%) ==
        // rgb(64, 128, 191) == (0.25, 0.5, 0.75).
        let (r, g, b) = hsl_to_srgb(210.0, 0.5, 0.5);
        approx_eps(r, 0.25, 0.01);
        approx_eps(g, 0.5, 0.01);
        approx_eps(b, 0.75, 0.01);
    }

    #[test]
    fn hsl_round_trip_pastel_purple() {
        // A muted lavender — non-trivial `s`, non-0.5 `l`. Round-trip
        // through sRGB should be stable to within ~1%.
        let (r0, g0, b0) = hsl_to_srgb(280.0, 0.4, 0.7);
        let (h, s, l) = srgb_to_hsl(r0, g0, b0);
        let (r1, g1, b1) = hsl_to_srgb(h, s, l);
        approx_eps(r1, r0, 0.01);
        approx_eps(g1, g0, 0.01);
        approx_eps(b1, b0, 0.01);
    }

    // ----- wire format lockstep (AGENTS.md §Rules §4) -----
    //
    // The TypeScript types in `apps/desktop/shared/scene.ts`
    // (`ColorValue`, `IccProfile`, `RenderingIntent`) declare the
    // canonical wire format for the renderer-side bridge. These tests
    // pin the Rust serde output to that contract so the lockstep can't
    // silently drift via someone re-adding a `#[serde(tag = ...)]`
    // annotation.

    #[test]
    fn color_wire_format_is_externally_tagged_pascal_case() {
        let json = serde_json::to_string(&Color::Srgb {
            r: 1.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        })
        .unwrap();
        assert_eq!(json, r#"{"Srgb":{"r":1.0,"g":0.0,"b":0.0,"a":1.0}}"#);
        let json = serde_json::to_string(&Color::Cmyk {
            c: 0.0,
            m: 1.0,
            y: 1.0,
            k: 0.0,
            a: 1.0,
        })
        .unwrap();
        assert_eq!(
            json,
            r#"{"Cmyk":{"c":0.0,"m":1.0,"y":1.0,"k":0.0,"a":1.0}}"#
        );
    }

    #[test]
    fn icc_profile_unit_variants_serialize_as_bare_pascal_strings() {
        assert_eq!(
            serde_json::to_string(&IccProfile::SrgbIec61966).unwrap(),
            "\"SrgbIec61966\""
        );
        assert_eq!(
            serde_json::to_string(&IccProfile::FogRa39).unwrap(),
            "\"FogRa39\""
        );
        assert_eq!(
            serde_json::to_string(&IccProfile::Custom {
                name: "MyMonitor".into(),
                blob_hash: "abcd".into(),
                color_space: IccColorSpace::Rgb,
            })
            .unwrap(),
            r#"{"Custom":{"name":"MyMonitor","blob_hash":"abcd","color_space":"Rgb"}}"#
        );
        assert_eq!(
            serde_json::to_string(&IccProfile::Custom {
                name: "MyPress".into(),
                blob_hash: "deadbeef".into(),
                color_space: IccColorSpace::Cmyk,
            })
            .unwrap(),
            r#"{"Custom":{"name":"MyPress","blob_hash":"deadbeef","color_space":"Cmyk"}}"#
        );
    }

    #[test]
    fn rendering_intent_serializes_as_pascal_case_strings() {
        assert_eq!(
            serde_json::to_string(&RenderingIntent::Perceptual).unwrap(),
            "\"Perceptual\""
        );
        assert_eq!(
            serde_json::to_string(&RenderingIntent::RelativeColorimetric).unwrap(),
            "\"RelativeColorimetric\""
        );
        assert_eq!(
            serde_json::to_string(&RenderingIntent::Saturation).unwrap(),
            "\"Saturation\""
        );
        assert_eq!(
            serde_json::to_string(&RenderingIntent::AbsoluteColorimetric).unwrap(),
            "\"AbsoluteColorimetric\""
        );
    }
}
