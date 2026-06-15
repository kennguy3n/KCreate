//! Theme & Brand Kit — Gamma-style instant restyle + Canva-style brand kits.
//!
//! A [`Theme`] bundles three things:
//!
//! * a **role-based colour palette** (`background`, `surface`, `primary`,
//!   `secondary`, `accent`, `text`, `muted`),
//! * a **type scale** (display / heading / body / caption sizes plus a body
//!   and heading font family), and
//! * **spacing + corner-radius** token scales.
//!
//! Applying a theme to a document remaps every painted colour to the nearest
//! *semantic role* (so the most-used fill becomes the background, the most
//! saturated colour becomes the primary action colour, …), rescales text by
//! reclassifying each text layer into a type-scale role, and re-rounds corners
//! to the theme's radius scale. The application itself — including the single
//! undoable operation that makes the whole restyle reversible with one Ctrl+Z —
//! lives in `kcreate_bridge::document::document_apply_theme`. This module owns
//! the pure, side-effect-free model + the colour-classification maths so it can
//! be unit-tested without the bridge's process-global workspace.
//!
//! Themes (de)serialise to JSON with snake_case field names; the renderer
//! mirrors the shape in `apps/desktop/shared/scene.ts`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::node::RgbaColor;
use crate::project::{BrandKit, DesignTokens, FontRef, NamedColor, TypographyToken};

/// Chroma (`max − min` of the RGB channels, in `[0, 1]`) at or below which a
/// colour is treated as a neutral (grey / near-grey / tinted off-white /
/// tinted off-black) for role classification rather than a chromatic accent.
///
/// We deliberately use raw chroma rather than HSL saturation: HSL saturation
/// is numerically unstable near white and black — a barely-tinted off-white
/// such as `#F1F5F9` reports a high HSL saturation even though it reads as a
/// neutral surface — whereas chroma stays perceptually faithful across the
/// whole lightness range. Used by both [`assign_roles`] and
/// [`Theme::derive_from_palette`] so the apply-path and the derive-path agree
/// on what counts as "a colour".
const NEUTRAL_CHROMA: f32 = 0.12;

/// Semantic colour roles a theme assigns. The restyle maps every painted
/// colour in a document onto one of these, then paints the theme's colour for
/// that role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorRole {
    /// The dominant page / canvas backdrop.
    Background,
    /// Raised panels, cards, and sections sitting on the background.
    Surface,
    /// The primary brand / call-to-action colour.
    Primary,
    /// A complementary brand colour.
    Secondary,
    /// A high-energy highlight colour.
    Accent,
    /// Body / heading text and high-contrast foreground marks.
    Text,
    /// Low-emphasis text, dividers, and disabled states.
    Muted,
}

impl ColorRole {
    /// Every role, in palette display order.
    pub const ALL: [Self; 7] = [
        Self::Background,
        Self::Surface,
        Self::Primary,
        Self::Secondary,
        Self::Accent,
        Self::Text,
        Self::Muted,
    ];

    /// Stable lowercase identifier used as the design-token key.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Background => "background",
            Self::Surface => "surface",
            Self::Primary => "primary",
            Self::Secondary => "secondary",
            Self::Accent => "accent",
            Self::Text => "text",
            Self::Muted => "muted",
        }
    }
}

/// Type-scale roles a text layer can be reclassified into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TypeRole {
    /// Hero / cover headline.
    Display,
    /// Section heading.
    Heading,
    /// Default running text.
    Body,
    /// Fine print, labels, captions.
    Caption,
}

impl TypeRole {
    /// Every type role, largest to smallest.
    pub const ALL: [Self; 4] = [Self::Display, Self::Heading, Self::Body, Self::Caption];

    /// Stable lowercase identifier used as the typography-token key.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Display => "display",
            Self::Heading => "heading",
            Self::Body => "body",
            Self::Caption => "caption",
        }
    }

    /// Classify an existing font size (px) into the type-scale role it most
    /// closely plays. Thresholds are deliberately theme-independent so that
    /// re-applying a theme preserves the document's *hierarchy* (a 48px
    /// headline stays a display, a 12px label stays a caption) even as the
    /// absolute sizes change to the new theme's scale.
    #[must_use]
    pub fn for_size(size: f32) -> Self {
        if size >= 30.0 {
            Self::Display
        } else if size >= 20.0 {
            Self::Heading
        } else if size >= 13.0 {
            Self::Body
        } else {
            Self::Caption
        }
    }

    /// Heavier roles get a bolder default weight.
    #[must_use]
    pub const fn font_weight(self) -> u16 {
        match self {
            Self::Display | Self::Heading => 700,
            Self::Body => 400,
            Self::Caption => 500,
        }
    }
}

/// A theme's seven-colour palette, one colour per [`ColorRole`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ThemePalette {
    pub background: RgbaColor,
    pub surface: RgbaColor,
    pub primary: RgbaColor,
    pub secondary: RgbaColor,
    pub accent: RgbaColor,
    pub text: RgbaColor,
    pub muted: RgbaColor,
}

impl ThemePalette {
    /// Look up the colour assigned to `role`.
    #[must_use]
    pub fn get(&self, role: ColorRole) -> RgbaColor {
        match role {
            ColorRole::Background => self.background,
            ColorRole::Surface => self.surface,
            ColorRole::Primary => self.primary,
            ColorRole::Secondary => self.secondary,
            ColorRole::Accent => self.accent,
            ColorRole::Text => self.text,
            ColorRole::Muted => self.muted,
        }
    }

    /// Replace the colour assigned to `role`.
    pub fn set(&mut self, role: ColorRole, color: RgbaColor) {
        match role {
            ColorRole::Background => self.background = color,
            ColorRole::Surface => self.surface = color,
            ColorRole::Primary => self.primary = color,
            ColorRole::Secondary => self.secondary = color,
            ColorRole::Accent => self.accent = color,
            ColorRole::Text => self.text = color,
            ColorRole::Muted => self.muted = color,
        }
    }

    /// Every `(role, colour)` pair in [`ColorRole::ALL`] order.
    #[must_use]
    pub fn roles(&self) -> [(ColorRole, RgbaColor); 7] {
        ColorRole::ALL.map(|role| (role, self.get(role)))
    }
}

/// A theme's type scale: a body + heading font family and one size per
/// [`TypeRole`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TypeScale {
    /// Font family used for body and caption text.
    pub body_font: String,
    /// Font family used for display and heading text.
    pub heading_font: String,
    pub display: f32,
    pub heading: f32,
    pub body: f32,
    pub caption: f32,
    /// Multiplier applied to font size for line height (e.g. `1.4`).
    pub line_height: f32,
}

impl TypeScale {
    /// Font size (px) for a type role.
    #[must_use]
    pub fn size_for(&self, role: TypeRole) -> f32 {
        match role {
            TypeRole::Display => self.display,
            TypeRole::Heading => self.heading,
            TypeRole::Body => self.body,
            TypeRole::Caption => self.caption,
        }
    }

    /// Font family for a type role — display / heading use the heading font,
    /// body / caption use the body font.
    #[must_use]
    pub fn font_for(&self, role: TypeRole) -> &str {
        match role {
            TypeRole::Display | TypeRole::Heading => &self.heading_font,
            TypeRole::Body | TypeRole::Caption => &self.body_font,
        }
    }
}

/// A theme's spacing scale (px). Persisted into design tokens and a custom
/// brand kit's `spacing_scale`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SpacingScale {
    pub xs: f32,
    pub sm: f32,
    pub md: f32,
    pub lg: f32,
    pub xl: f32,
}

impl SpacingScale {
    /// The scale as an ordered `(name, value)` list for token export.
    #[must_use]
    pub fn entries(&self) -> [(&'static str, f32); 5] {
        [
            ("xs", self.xs),
            ("sm", self.sm),
            ("md", self.md),
            ("lg", self.lg),
            ("xl", self.xl),
        ]
    }

    /// The scale as an ascending value list for a brand kit.
    #[must_use]
    pub fn to_vec(&self) -> Vec<f32> {
        vec![self.xs, self.sm, self.md, self.lg, self.xl]
    }
}

/// A theme's corner-radius scale (px).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RadiusScale {
    pub none: f32,
    pub small: f32,
    pub medium: f32,
    pub large: f32,
    /// "Pill" radius for fully-rounded elements.
    pub full: f32,
}

impl RadiusScale {
    /// The scale as an ordered `(name, value)` list for token export.
    #[must_use]
    pub fn entries(&self) -> [(&'static str, f32); 5] {
        [
            ("none", self.none),
            ("small", self.small),
            ("medium", self.medium),
            ("large", self.large),
            ("full", self.full),
        ]
    }

    /// Remap an existing corner radius onto this scale, preserving intent:
    /// sharp corners stay sharp, and the rounded buckets are re-quantised to
    /// the theme's small / medium / large radii. `full` is intentionally not
    /// produced here (a bare radius can't be distinguished from a large one
    /// without the node's dimensions) — it is available only as a token.
    #[must_use]
    pub fn remap(&self, radius: f64) -> f64 {
        if radius <= 0.5 {
            f64::from(self.none)
        } else if radius <= 8.0 {
            f64::from(self.small)
        } else if radius <= 20.0 {
            f64::from(self.medium)
        } else {
            f64::from(self.large)
        }
    }
}

/// A named theme: palette + type scale + spacing + radii.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Theme {
    /// Stable slug for built-ins (e.g. `"midnight"`); a free-form id for
    /// derived / custom themes.
    pub id: String,
    /// Human-readable display name.
    pub name: String,
    pub palette: ThemePalette,
    pub type_scale: TypeScale,
    pub spacing: SpacingScale,
    pub radii: RadiusScale,
}

impl Theme {
    /// Build the [`DesignTokens`] this theme contributes: one colour token per
    /// role, one typography token per type role, and the spacing + radius
    /// scales. Shadows are left empty (a theme does not define shadows).
    #[must_use]
    pub fn to_design_tokens(&self) -> DesignTokens {
        let mut tokens = DesignTokens::default();
        for (role, color) in self.palette.roles() {
            tokens.colors.insert(role.as_str().to_string(), color);
        }
        for role in TypeRole::ALL {
            tokens.typography.insert(
                role.as_str().to_string(),
                TypographyToken {
                    font_family: self.type_scale.font_for(role).to_string(),
                    font_weight: role.font_weight(),
                    font_size: self.type_scale.size_for(role),
                    line_height: self.type_scale.line_height,
                    letter_spacing: 0.0,
                },
            );
        }
        for (name, value) in self.spacing.entries() {
            tokens.spacing.insert(name.to_string(), value);
        }
        for (name, value) in self.radii.entries() {
            tokens.radii.insert(name.to_string(), value);
        }
        tokens
    }

    /// Merge this theme's tokens onto an existing token set, overwriting the
    /// theme-owned keys while preserving everything else (notably the
    /// project's shadow tokens and any custom-named colours).
    #[must_use]
    pub fn merge_into_tokens(&self, base: &DesignTokens) -> DesignTokens {
        let mut merged = base.clone();
        let mine = self.to_design_tokens();
        merged.colors.extend(mine.colors);
        merged.typography.extend(mine.typography);
        merged.spacing.extend(mine.spacing);
        merged.radii.extend(mine.radii);
        merged
    }

    /// Convert to a persistable [`BrandKit`] (Canva-style pinned palette +
    /// fonts + spacing). Colours are stored as role-named entries so
    /// [`Theme::from_brand_kit`] can round-trip them.
    #[must_use]
    pub fn to_brand_kit(&self) -> BrandKit {
        let mut kit = BrandKit::new(self.name.clone());
        kit.colors = self
            .palette
            .roles()
            .into_iter()
            .map(|(role, color)| NamedColor {
                name: role.as_str().to_string(),
                color,
            })
            .collect();
        // Body font first, heading font second; dedupe when identical.
        kit.fonts.push(FontRef {
            family: self.type_scale.body_font.clone(),
            weight: TypeRole::Body.font_weight(),
            italic: false,
            embedded_asset_id: None,
        });
        if self.type_scale.heading_font != self.type_scale.body_font {
            kit.fonts.push(FontRef {
                family: self.type_scale.heading_font.clone(),
                weight: TypeRole::Heading.font_weight(),
                italic: false,
                embedded_asset_id: None,
            });
        }
        kit.spacing_scale = self.spacing.to_vec();
        kit
    }

    /// Reconstruct a theme from a persisted [`BrandKit`]. Colours are matched
    /// by role name; any missing role falls back to the [`Theme::default`]
    /// palette. Type sizes default (a brand kit pins fonts + palette, not the
    /// size ramp), with the body font taken from the first non-bold font and
    /// the heading font from the first bold font.
    #[must_use]
    pub fn from_brand_kit(kit: &BrandKit) -> Self {
        let mut theme = Self {
            id: format!("kit-{}", kit.id),
            name: kit.name.clone(),
            ..Self::default()
        };
        for named in &kit.colors {
            if let Some(role) = role_from_str(&named.name) {
                theme.palette.set(role, named.color);
            }
        }
        if let Some(body) = kit.fonts.iter().find(|f| f.weight < 600) {
            theme.type_scale.body_font.clone_from(&body.family);
        } else if let Some(first) = kit.fonts.first() {
            theme.type_scale.body_font.clone_from(&first.family);
        }
        if let Some(heading) = kit.fonts.iter().find(|f| f.weight >= 600) {
            theme.type_scale.heading_font.clone_from(&heading.family);
        } else {
            theme
                .type_scale
                .heading_font
                .clone_from(&theme.type_scale.body_font);
        }
        let mut scale: Vec<f32> = kit.spacing_scale.clone();
        if scale.len() == 5 {
            theme.spacing = SpacingScale {
                xs: scale[0],
                sm: scale[1],
                md: scale[2],
                lg: scale[3],
                xl: scale[4],
            };
        } else if !scale.is_empty() {
            // Pad / truncate gracefully so an odd-length kit still loads.
            scale.resize(5, *scale.last().unwrap_or(&8.0));
            theme.spacing = SpacingScale {
                xs: scale[0],
                sm: scale[1],
                md: scale[2],
                lg: scale[3],
                xl: scale[4],
            };
        }
        theme
    }

    /// Look up a built-in theme by its slug id.
    #[must_use]
    pub fn builtin(id: &str) -> Option<Self> {
        builtin_themes().into_iter().find(|t| t.id == id)
    }

    /// Derive a theme from a weighted colour palette (e.g. the dominant
    /// colours extracted from an existing design). `colors` is a list of
    /// `(colour, weight)` pairs where higher weight means more prominent; the
    /// most prominent colour becomes the background and the most saturated
    /// chromatic colours become the primary / secondary / accent roles. Text
    /// and surface are synthesised for guaranteed contrast against the chosen
    /// background.
    #[must_use]
    pub fn derive_from_palette(name: impl Into<String>, colors: &[(RgbaColor, f32)]) -> Self {
        let name = name.into();
        let mut weighted: Vec<(RgbaColor, f32)> = colors
            .iter()
            .map(|(c, w)| (opaque(*c), w.max(0.0)))
            .collect();
        // Sort by weight desc, deterministic tie-break by quantised colour.
        weighted.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| quantize(a.0).cmp(&quantize(b.0)))
        });

        let mut base = Self {
            id: format!("derived-{}", slugify(&name)),
            name,
            ..Self::default()
        };
        let Some(&(background, _)) = weighted.first() else {
            // No colours at all → fall back to the default palette.
            return base;
        };

        let is_dark = relative_luminance(background) < 0.5;
        let text = if is_dark {
            mix(background, RgbaColor::WHITE, 0.92)
        } else {
            mix(background, RgbaColor::BLACK, 0.88)
        };
        let surface = if is_dark {
            mix(background, RgbaColor::WHITE, 0.08)
        } else {
            mix(background, RgbaColor::BLACK, 0.05)
        };
        let muted = desaturate(mix(text, background, 0.45), 0.4);

        // Chromatic candidates, most-saturated first. The background colour is
        // already spoken for, so exclude it (a dominant tinted cream must not
        // also be handed out as an accent).
        let background_key = quantize(background);
        let mut chromatic: Vec<RgbaColor> = weighted
            .iter()
            .map(|(c, _)| *c)
            .filter(|c| quantize(*c) != background_key && chroma(*c) >= NEUTRAL_CHROMA)
            .collect();
        chromatic.sort_by(|a, b| {
            saturation(*b)
                .partial_cmp(&saturation(*a))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| quantize(*a).cmp(&quantize(*b)))
        });

        let primary = chromatic
            .first()
            .copied()
            .unwrap_or(RgbaColor::KCHAT_PRIMARY);
        let secondary = chromatic
            .get(1)
            .copied()
            .unwrap_or_else(|| rotate_hue(primary, 32.0));
        let accent = chromatic
            .get(2)
            .copied()
            .unwrap_or_else(|| rotate_hue(primary, -38.0));

        base.palette = ThemePalette {
            background,
            surface,
            primary,
            secondary,
            accent,
            text,
            muted,
        };
        base
    }
}

impl Default for Theme {
    /// A neutral light theme used as the reconstruction / derivation fallback.
    fn default() -> Self {
        Self {
            id: "daybreak".to_string(),
            name: "Daybreak".to_string(),
            palette: ThemePalette {
                background: rgb(0xFF, 0xFF, 0xFF),
                surface: rgb(0xF1, 0xF5, 0xF9),
                primary: rgb(0x25, 0x63, 0xEB),
                secondary: rgb(0x0E, 0xA5, 0xE9),
                accent: rgb(0xF5, 0x9E, 0x0B),
                text: rgb(0x0F, 0x17, 0x2A),
                muted: rgb(0x64, 0x74, 0x8B),
            },
            type_scale: TypeScale {
                body_font: "Inter".to_string(),
                heading_font: "Inter".to_string(),
                display: 44.0,
                heading: 28.0,
                body: 16.0,
                caption: 12.0,
                line_height: 1.4,
            },
            spacing: SpacingScale {
                xs: 4.0,
                sm: 8.0,
                md: 16.0,
                lg: 24.0,
                xl: 40.0,
            },
            radii: RadiusScale {
                none: 0.0,
                small: 4.0,
                medium: 10.0,
                large: 20.0,
                full: 9999.0,
            },
        }
    }
}

/// The built-in professional themes shipped with KCreate.
#[must_use]
pub fn builtin_themes() -> Vec<Theme> {
    vec![
        // Default light theme.
        Theme::default(),
        // Dark indigo.
        Theme {
            id: "midnight".to_string(),
            name: "Midnight".to_string(),
            palette: ThemePalette {
                background: rgb(0x0F, 0x17, 0x2A),
                surface: rgb(0x1E, 0x29, 0x3B),
                primary: rgb(0x63, 0x66, 0xF1),
                secondary: rgb(0x22, 0xD3, 0xEE),
                accent: rgb(0xF4, 0x72, 0xB6),
                text: rgb(0xF8, 0xFA, 0xFC),
                muted: rgb(0x94, 0xA3, 0xB8),
            },
            type_scale: TypeScale {
                body_font: "Inter".to_string(),
                heading_font: "Poppins".to_string(),
                display: 48.0,
                heading: 30.0,
                body: 16.0,
                caption: 12.0,
                line_height: 1.45,
            },
            spacing: SpacingScale {
                xs: 4.0,
                sm: 8.0,
                md: 16.0,
                lg: 28.0,
                xl: 48.0,
            },
            radii: RadiusScale {
                none: 0.0,
                small: 6.0,
                medium: 12.0,
                large: 24.0,
                full: 9999.0,
            },
        },
        // Warm sunset.
        Theme {
            id: "sunset".to_string(),
            name: "Sunset".to_string(),
            palette: ThemePalette {
                background: rgb(0x1A, 0x14, 0x23),
                surface: rgb(0x2B, 0x22, 0x33),
                primary: rgb(0xFB, 0x71, 0x85),
                secondary: rgb(0xFB, 0x92, 0x3C),
                accent: rgb(0xFA, 0xCC, 0x15),
                text: rgb(0xFF, 0xF7, 0xED),
                muted: rgb(0xA8, 0xA2, 0x9E),
            },
            type_scale: TypeScale {
                body_font: "Inter".to_string(),
                heading_font: "Poppins".to_string(),
                display: 52.0,
                heading: 32.0,
                body: 17.0,
                caption: 13.0,
                line_height: 1.5,
            },
            spacing: SpacingScale {
                xs: 4.0,
                sm: 10.0,
                md: 18.0,
                lg: 30.0,
                xl: 52.0,
            },
            radii: RadiusScale {
                none: 0.0,
                small: 8.0,
                medium: 16.0,
                large: 28.0,
                full: 9999.0,
            },
        },
        // Natural forest.
        Theme {
            id: "forest".to_string(),
            name: "Forest".to_string(),
            palette: ThemePalette {
                background: rgb(0x0B, 0x1F, 0x17),
                surface: rgb(0x14, 0x34, 0x2A),
                primary: rgb(0x34, 0xD3, 0x99),
                secondary: rgb(0xA3, 0xE6, 0x35),
                accent: rgb(0xFB, 0xBF, 0x24),
                text: rgb(0xEC, 0xFD, 0xF5),
                muted: rgb(0x9C, 0xA3, 0xAF),
            },
            type_scale: TypeScale {
                body_font: "Inter".to_string(),
                heading_font: "Merriweather".to_string(),
                display: 46.0,
                heading: 29.0,
                body: 16.0,
                caption: 12.0,
                line_height: 1.55,
            },
            spacing: SpacingScale {
                xs: 4.0,
                sm: 8.0,
                md: 16.0,
                lg: 24.0,
                xl: 40.0,
            },
            radii: RadiusScale {
                none: 0.0,
                small: 4.0,
                medium: 8.0,
                large: 16.0,
                full: 9999.0,
            },
        },
        // Minimal monochrome with a single red pop.
        Theme {
            id: "mono".to_string(),
            name: "Mono".to_string(),
            palette: ThemePalette {
                background: rgb(0xFF, 0xFF, 0xFF),
                surface: rgb(0xF4, 0xF4, 0xF5),
                primary: rgb(0x18, 0x18, 0x1B),
                secondary: rgb(0x3F, 0x3F, 0x46),
                accent: rgb(0xEF, 0x44, 0x44),
                text: rgb(0x18, 0x18, 0x1B),
                muted: rgb(0xA1, 0xA1, 0xAA),
            },
            type_scale: TypeScale {
                body_font: "Inter".to_string(),
                heading_font: "Inter".to_string(),
                display: 40.0,
                heading: 26.0,
                body: 15.0,
                caption: 12.0,
                line_height: 1.5,
            },
            spacing: SpacingScale {
                xs: 4.0,
                sm: 8.0,
                md: 16.0,
                lg: 24.0,
                xl: 40.0,
            },
            radii: RadiusScale {
                none: 0.0,
                small: 2.0,
                medium: 4.0,
                large: 8.0,
                full: 9999.0,
            },
        },
        // Vibrant grape (KChat-adjacent violet).
        Theme {
            id: "grape".to_string(),
            name: "Grape".to_string(),
            palette: ThemePalette {
                background: rgb(0xFA, 0xF5, 0xFF),
                surface: rgb(0xF3, 0xE8, 0xFF),
                primary: rgb(0x7C, 0x3A, 0xED),
                secondary: rgb(0xA8, 0x55, 0xF7),
                accent: rgb(0xEC, 0x48, 0x99),
                text: rgb(0x2E, 0x10, 0x65),
                muted: rgb(0x9C, 0xA3, 0xAF),
            },
            type_scale: TypeScale {
                body_font: "Inter".to_string(),
                heading_font: "Poppins".to_string(),
                display: 50.0,
                heading: 31.0,
                body: 16.0,
                caption: 12.0,
                line_height: 1.45,
            },
            spacing: SpacingScale {
                xs: 4.0,
                sm: 8.0,
                md: 16.0,
                lg: 28.0,
                xl: 44.0,
            },
            radii: RadiusScale {
                none: 0.0,
                small: 8.0,
                medium: 16.0,
                large: 24.0,
                full: 9999.0,
            },
        },
    ]
}

// ---------------------------------------------------------------------------
// Role-aware colour remapping
// ---------------------------------------------------------------------------

/// A distinct colour found painted in a document plus the total area (in
/// document units²) it covers. Built by the bridge while walking the graph and
/// fed into [`build_color_remap`] / [`assign_roles`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorUsage {
    pub color: RgbaColor,
    pub area: f64,
}

impl ColorUsage {
    #[must_use]
    pub fn new(color: RgbaColor, area: f64) -> Self {
        Self { color, area }
    }
}

/// Assign every distinct colour in `usages` to a [`ColorRole`], role-aware:
///
/// * the largest-area colour becomes the **background** (dominant fill →
///   background),
/// * the remaining chromatic colours become **primary / secondary / accent**
///   in descending saturation (the most saturated → primary, i.e. the typical
///   call-to-action colour), and
/// * neutral (low-saturation) colours become **surface** (very light), **text**
///   (very dark), or **muted** (mid-grey).
///
/// Colours are aggregated by 8-bit-quantised RGBA first, so a colour that
/// appears on many nodes is counted by total area. Returns one entry per
/// distinct source colour. Deterministic for a given input.
#[must_use]
pub fn assign_roles(usages: &[ColorUsage]) -> Vec<(RgbaColor, ColorRole)> {
    let ranked = aggregate(usages);
    if ranked.is_empty() {
        return Vec::new();
    }

    let mut out: Vec<(RgbaColor, ColorRole)> = Vec::with_capacity(ranked.len());
    // Highest-area colour is the background.
    out.push((ranked[0].0, ColorRole::Background));

    // Partition the remainder into chromatic vs neutral, preserving the
    // area-desc order for stable secondary tie-breaks.
    let mut chromatic: Vec<RgbaColor> = Vec::new();
    let mut neutral: Vec<RgbaColor> = Vec::new();
    for &(color, _area) in &ranked[1..] {
        if chroma(color) >= NEUTRAL_CHROMA {
            chromatic.push(color);
        } else {
            neutral.push(color);
        }
    }

    // Chromatic colours ranked by saturation: primary, secondary, accent…
    chromatic.sort_by(|a, b| {
        saturation(*b)
            .partial_cmp(&saturation(*a))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| quantize(*a).cmp(&quantize(*b)))
    });
    for (i, color) in chromatic.into_iter().enumerate() {
        let role = match i {
            0 => ColorRole::Primary,
            1 => ColorRole::Secondary,
            _ => ColorRole::Accent,
        };
        out.push((color, role));
    }

    // Neutrals classified by lightness.
    for color in neutral {
        let l = lightness(color);
        let role = if l > 0.82 {
            ColorRole::Surface
        } else if l < 0.30 {
            ColorRole::Text
        } else {
            ColorRole::Muted
        };
        out.push((color, role));
    }

    out
}

/// Build a lookup from each distinct source colour (8-bit-quantised RGBA) to
/// the theme colour it should be repainted with, role-aware. The source
/// colour's alpha is preserved on the target so semi-transparent fills stay
/// semi-transparent.
#[must_use]
pub fn build_color_remap(
    usages: &[ColorUsage],
    theme: &Theme,
) -> std::collections::HashMap<[u8; 4], RgbaColor> {
    let mut map = std::collections::HashMap::new();
    for (source, role) in assign_roles(usages) {
        let target = with_alpha(theme.palette.get(role), source.a);
        map.insert(quantize(source), target);
    }
    map
}

// ---------------------------------------------------------------------------
// Colour maths (pure helpers, all sRGB unless noted)
// ---------------------------------------------------------------------------

/// Quantise a colour to 8-bit RGBA for use as a stable map key.
#[must_use]
pub fn quantize(c: RgbaColor) -> [u8; 4] {
    [to_u8(c.r), to_u8(c.g), to_u8(c.b), to_u8(c.a)]
}

fn to_u8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

const fn rgb(r: u8, g: u8, b: u8) -> RgbaColor {
    RgbaColor::new(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0)
}

fn opaque(c: RgbaColor) -> RgbaColor {
    RgbaColor::new(c.r, c.g, c.b, 1.0)
}

fn with_alpha(c: RgbaColor, a: f32) -> RgbaColor {
    RgbaColor::new(c.r, c.g, c.b, a)
}

/// HSL lightness `(max + min) / 2` of the RGB channels.
#[must_use]
pub fn lightness(c: RgbaColor) -> f32 {
    let (max, min) = max_min(c);
    f32::midpoint(max, min)
}

/// Chroma — the raw spread of the RGB channels (`max − min`, in `[0, 1]`). A
/// lightness-independent measure of how far a colour is from grey; preferred
/// over HSL saturation for neutral-vs-chromatic classification because it does
/// not blow up near white or black.
#[must_use]
pub fn chroma(c: RgbaColor) -> f32 {
    let (max, min) = max_min(c);
    max - min
}

/// HSL saturation of the RGB channels (0 = grey, 1 = fully saturated).
#[must_use]
pub fn saturation(c: RgbaColor) -> f32 {
    let (max, min) = max_min(c);
    let delta = max - min;
    if delta.abs() < 1e-6 {
        return 0.0;
    }
    let l = f32::midpoint(max, min);
    if l > 0.5 {
        delta / (2.0 - max - min)
    } else {
        delta / (max + min)
    }
}

/// WCAG relative luminance (linearised sRGB, perceptual weights). Used for
/// contrast decisions when synthesising text colours.
#[must_use]
pub fn relative_luminance(c: RgbaColor) -> f32 {
    0.2126 * srgb_to_linear(c.r) + 0.7152 * srgb_to_linear(c.g) + 0.0722 * srgb_to_linear(c.b)
}

/// WCAG contrast ratio between two colours, in `[1.0, 21.0]`.
#[must_use]
pub fn contrast_ratio(a: RgbaColor, b: RgbaColor) -> f32 {
    let la = relative_luminance(a);
    let lb = relative_luminance(b);
    let (hi, lo) = if la >= lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

fn srgb_to_linear(channel: f32) -> f32 {
    let c = channel.clamp(0.0, 1.0);
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn max_min(c: RgbaColor) -> (f32, f32) {
    let max = c.r.max(c.g).max(c.b);
    let min = c.r.min(c.g).min(c.b);
    (max, min)
}

/// Linearly interpolate between two colours in sRGB space (`t` in `[0, 1]`).
#[must_use]
pub fn mix(a: RgbaColor, b: RgbaColor, t: f32) -> RgbaColor {
    let t = t.clamp(0.0, 1.0);
    RgbaColor::new(
        a.r + (b.r - a.r) * t,
        a.g + (b.g - a.g) * t,
        a.b + (b.b - a.b) * t,
        a.a + (b.a - a.a) * t,
    )
}

/// Pull a colour toward its own grey of equal lightness by `amount`.
fn desaturate(c: RgbaColor, amount: f32) -> RgbaColor {
    let l = lightness(c);
    let grey = RgbaColor::new(l, l, l, c.a);
    mix(c, grey, amount.clamp(0.0, 1.0))
}

/// Rotate a colour's hue by `degrees`, preserving saturation and lightness.
#[must_use]
pub fn rotate_hue(c: RgbaColor, degrees: f32) -> RgbaColor {
    let (h, s, l) = rgb_to_hsl(c);
    let h = (h + degrees).rem_euclid(360.0);
    let (r, g, b) = hsl_to_rgb(h, s, l);
    RgbaColor::new(r, g, b, c.a)
}

fn rgb_to_hsl(c: RgbaColor) -> (f32, f32, f32) {
    let (max, min) = max_min(c);
    let delta = max - min;
    let l = f32::midpoint(max, min);
    if delta.abs() < 1e-6 {
        return (0.0, 0.0, l);
    }
    let s = if l > 0.5 {
        delta / (2.0 - max - min)
    } else {
        delta / (max + min)
    };
    let h = if (max - c.r).abs() < 1e-6 {
        ((c.g - c.b) / delta).rem_euclid(6.0)
    } else if (max - c.g).abs() < 1e-6 {
        (c.b - c.r) / delta + 2.0
    } else {
        (c.r - c.g) / delta + 4.0
    };
    ((h * 60.0).rem_euclid(360.0), s, l)
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (f32, f32, f32) {
    if s <= 1e-6 {
        return (l, l, l);
    }
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let h_prime = h / 60.0;
    let x = c * (1.0 - (h_prime % 2.0 - 1.0).abs());
    let (r1, g1, b1) = match h_prime as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    (r1 + m, g1 + m, b1 + m)
}

fn role_from_str(s: &str) -> Option<ColorRole> {
    ColorRole::ALL.into_iter().find(|r| r.as_str() == s)
}

/// Aggregate usages by quantised colour and return them sorted by total area
/// descending (deterministic tie-break by quantised key).
fn aggregate(usages: &[ColorUsage]) -> Vec<(RgbaColor, f64)> {
    let mut by_key: BTreeMap<[u8; 4], (RgbaColor, f64)> = BTreeMap::new();
    for u in usages {
        let key = quantize(u.color);
        let entry = by_key.entry(key).or_insert((u.color, 0.0));
        entry.1 += u.area.max(0.0);
    }
    let mut ranked: Vec<(RgbaColor, f64)> = by_key.into_values().collect();
    ranked.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| quantize(a.0).cmp(&quantize(b.0)))
    });
    ranked
}

fn slugify(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut prev_dash = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "theme".to_string()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(hex: &str) -> RgbaColor {
        RgbaColor::from_hex(hex).expect("valid hex")
    }

    #[test]
    fn builtins_are_well_formed() {
        let themes = builtin_themes();
        assert!(themes.len() >= 4, "ship at least 4 built-in themes");
        let mut ids = std::collections::HashSet::new();
        for theme in &themes {
            assert!(ids.insert(theme.id.clone()), "duplicate id {}", theme.id);
            assert!(!theme.name.is_empty());
            // No fully-transparent palette slots.
            for (_, color) in theme.palette.roles() {
                assert!(color.a > 0.99, "palette colour must be opaque");
            }
            // Sizes strictly descend display > heading > body > caption.
            let s = &theme.type_scale;
            assert!(s.display > s.heading && s.heading > s.body && s.body >= s.caption);
            // Text must be readable against the background.
            let cr = contrast_ratio(theme.palette.text, theme.palette.background);
            assert!(cr >= 4.5, "{} text contrast {cr} too low", theme.id);
        }
        assert!(Theme::builtin("midnight").is_some());
        assert!(Theme::builtin("does-not-exist").is_none());
    }

    #[test]
    fn type_role_classifies_by_size() {
        assert_eq!(TypeRole::for_size(48.0), TypeRole::Display);
        assert_eq!(TypeRole::for_size(24.0), TypeRole::Heading);
        assert_eq!(TypeRole::for_size(16.0), TypeRole::Body);
        assert_eq!(TypeRole::for_size(10.0), TypeRole::Caption);
    }

    #[test]
    fn radius_remap_buckets() {
        let scale = RadiusScale {
            none: 0.0,
            small: 4.0,
            medium: 10.0,
            large: 20.0,
            full: 9999.0,
        };
        assert_eq!(scale.remap(0.0), 0.0);
        assert_eq!(scale.remap(3.0), 4.0);
        assert_eq!(scale.remap(12.0), 10.0);
        assert_eq!(scale.remap(40.0), 20.0);
    }

    #[test]
    fn palette_get_set_roundtrip() {
        let mut p = Theme::default().palette;
        let c = solid("#123456");
        p.set(ColorRole::Accent, c);
        assert_eq!(p.get(ColorRole::Accent), c);
        assert_eq!(p.roles().len(), 7);
    }

    #[test]
    fn dominant_fill_becomes_background() {
        // A light-grey wash covers the most area; a vivid blue button covers
        // a little; near-black text covers a little.
        let usages = vec![
            ColorUsage::new(solid("#EEEEEE"), 100_000.0),
            ColorUsage::new(solid("#2563EB"), 5_000.0),
            ColorUsage::new(solid("#111111"), 2_000.0),
        ];
        let roles: std::collections::HashMap<[u8; 4], ColorRole> = assign_roles(&usages)
            .into_iter()
            .map(|(c, r)| (quantize(c), r))
            .collect();
        assert_eq!(roles[&quantize(solid("#EEEEEE"))], ColorRole::Background);
        assert_eq!(roles[&quantize(solid("#2563EB"))], ColorRole::Primary);
        assert_eq!(roles[&quantize(solid("#111111"))], ColorRole::Text);
    }

    #[test]
    fn color_remap_targets_theme_and_preserves_alpha() {
        let theme = Theme::builtin("midnight").expect("midnight");
        let translucent_bg = RgbaColor::new(0.93, 0.93, 0.93, 0.5);
        let usages = vec![
            ColorUsage::new(translucent_bg, 50_000.0),
            ColorUsage::new(solid("#FF0066"), 3_000.0),
        ];
        let remap = build_color_remap(&usages, &theme);
        let bg_target = remap[&quantize(translucent_bg)];
        // Background colour adopts the theme background, original alpha kept.
        assert_eq!(bg_target.r, theme.palette.background.r);
        assert!((bg_target.a - 0.5).abs() < 1e-6);
        // The vivid pink maps to the theme primary.
        let cta_target = remap[&quantize(solid("#FF0066"))];
        assert_eq!(cta_target.r, theme.palette.primary.r);
        assert_eq!(cta_target.g, theme.palette.primary.g);
        assert_eq!(cta_target.b, theme.palette.primary.b);
    }

    #[test]
    fn derive_from_palette_picks_roles() {
        // Dominant cream background, vivid teal + coral accents, charcoal.
        let palette = vec![
            (solid("#FBF7F0"), 0.55),
            (solid("#0D9488"), 0.2),
            (solid("#F97316"), 0.15),
            (solid("#1F2937"), 0.1),
        ];
        let theme = Theme::derive_from_palette("My Brand", &palette);
        assert_eq!(theme.palette.background, opaque(solid("#FBF7F0")));
        // Light background → dark text with strong contrast.
        let cr = contrast_ratio(theme.palette.text, theme.palette.background);
        assert!(cr >= 4.5, "derived text contrast {cr} too low");
        // Most saturated chromatic colour becomes primary.
        assert!(chroma(theme.palette.primary) >= NEUTRAL_CHROMA);
        assert!(theme.id.starts_with("derived-"));
    }

    #[test]
    fn derive_from_empty_palette_is_default() {
        let theme = Theme::derive_from_palette("Empty", &[]);
        assert_eq!(theme.palette, Theme::default().palette);
    }

    #[test]
    fn design_tokens_cover_every_role() {
        let theme = Theme::builtin("grape").expect("grape");
        let tokens = theme.to_design_tokens();
        for role in ColorRole::ALL {
            assert!(tokens.colors.contains_key(role.as_str()));
        }
        for role in TypeRole::ALL {
            assert!(tokens.typography.contains_key(role.as_str()));
        }
        assert_eq!(tokens.spacing.len(), 5);
        assert_eq!(tokens.radii.len(), 5);
    }

    #[test]
    fn merge_into_tokens_preserves_foreign_keys() {
        let theme = Theme::default();
        let mut base = DesignTokens::default();
        base.colors
            .insert("brand-special".to_string(), solid("#ABCDEF"));
        let merged = theme.merge_into_tokens(&base);
        assert!(merged.colors.contains_key("brand-special"));
        assert!(merged.colors.contains_key("primary"));
    }

    #[test]
    fn brand_kit_roundtrip() {
        let theme = Theme::builtin("midnight").expect("midnight");
        let kit = theme.to_brand_kit();
        assert_eq!(kit.colors.len(), 7);
        assert_eq!(kit.spacing_scale.len(), 5);
        let restored = Theme::from_brand_kit(&kit);
        assert_eq!(restored.palette, theme.palette);
        assert_eq!(restored.type_scale.body_font, theme.type_scale.body_font);
        assert_eq!(
            restored.type_scale.heading_font,
            theme.type_scale.heading_font
        );
    }

    #[test]
    fn saturation_and_lightness_sanity() {
        assert!(saturation(RgbaColor::WHITE) < 1e-6);
        assert!(saturation(solid("#808080")) < 1e-6);
        assert!(saturation(solid("#FF0000")) > 0.9);
        assert!(lightness(RgbaColor::WHITE) > 0.99);
        assert!(lightness(RgbaColor::BLACK) < 1e-6);
    }

    #[test]
    fn rotate_hue_changes_hue_not_grey() {
        let red = solid("#FF0000");
        let rotated = rotate_hue(red, 120.0);
        // ~green.
        assert!(rotated.g > rotated.r && rotated.g > rotated.b);
        // Grey is unaffected by hue rotation.
        let grey = solid("#777777");
        let r2 = rotate_hue(grey, 90.0);
        assert!((r2.r - grey.r).abs() < 1e-3);
    }
}
