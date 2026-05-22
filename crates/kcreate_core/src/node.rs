//! Node-level types: geometry, transforms, styles, effects.
//!
//! A [`Node`] is the canonical unit of the document tree. Every layer,
//! group, artboard, page, and component is a `Node` distinguished by
//! its [`NodeType`]. Children are stored as `Vec<Uuid>`; the actual
//! child nodes live in the [`crate::document::DocumentGraph`]'s
//! `HashMap<Uuid, Node>`.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// 2D point with `f64` precision.
///
/// Document-level math runs in `f64` because authors can scale a layer
/// arbitrarily; the renderer collapses to `f32` only when constructing
/// the display list.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point2D {
    pub x: f64,
    pub y: f64,
}

impl Point2D {
    pub const ORIGIN: Self = Self::new(0.0, 0.0);

    #[must_use]
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

/// An axis-aligned bounding box in document space.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Bounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Bounds {
    pub const ZERO: Self = Self::new(0.0, 0.0, 0.0, 0.0);

    #[must_use]
    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    #[must_use]
    pub fn right(&self) -> f64 {
        self.x + self.width
    }

    #[must_use]
    pub fn bottom(&self) -> f64 {
        self.y + self.height
    }

    /// Returns the smallest [`Bounds`] containing both `self` and `other`.
    #[must_use]
    pub fn union(&self, other: &Self) -> Self {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        let right = self.right().max(other.right());
        let bottom = self.bottom().max(other.bottom());
        Self::new(x, y, right - x, bottom - y)
    }

    /// Returns the intersection of `self` and `other`, or `None` if
    /// they do not overlap.
    #[must_use]
    pub fn intersection(&self, other: &Self) -> Option<Self> {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        if right > x && bottom > y {
            Some(Self::new(x, y, right - x, bottom - y))
        } else {
            None
        }
    }

    /// True when `point` is inside `self` on the half-open interval
    /// `[min, max)` per axis.
    ///
    /// # Containment convention
    ///
    /// `kcreate_core` and `kcreate_vector` both use the **half-open**
    /// convention (`[x, x + width)` × `[y, y + height)`), matching
    /// `kcreate_vector::path::BoundingBox::contains`. Half-open is
    /// the standard choice for axis-aligned containment in raster /
    /// spatial-index contexts because it makes tilings *partitions*:
    /// every point belongs to exactly one tile, never two adjacent
    /// tiles claiming the same boundary pixel. Tests below pin the
    /// boundary semantics so a future "round to inclusive" tweak
    /// can't silently regress hit-testing.
    #[must_use]
    pub fn contains_point(&self, point: Point2D) -> bool {
        point.x >= self.x && point.x < self.right() && point.y >= self.y && point.y < self.bottom()
    }

    /// True when `other` is fully contained within `self`.
    #[must_use]
    pub fn contains_bounds(&self, other: &Self) -> bool {
        other.x >= self.x
            && other.y >= self.y
            && other.right() <= self.right()
            && other.bottom() <= self.bottom()
    }
}

/// 2D affine transform stored row-major as `[a b c d tx ty]`, applied
/// as `x' = a*x + c*y + tx`, `y' = b*x + d*y + ty`.
///
/// This matches the SVG and `kurbo` conventions.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Transform2D {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub tx: f64,
    pub ty: f64,
}

impl Transform2D {
    pub const IDENTITY: Self = Self {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        tx: 0.0,
        ty: 0.0,
    };

    #[must_use]
    pub const fn translation(tx: f64, ty: f64) -> Self {
        Self {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            tx,
            ty,
        }
    }

    #[must_use]
    pub const fn scale(sx: f64, sy: f64) -> Self {
        Self {
            a: sx,
            b: 0.0,
            c: 0.0,
            d: sy,
            tx: 0.0,
            ty: 0.0,
        }
    }

    /// Rotation around the origin by `radians`.
    #[must_use]
    pub fn rotation(radians: f64) -> Self {
        let (s, c) = radians.sin_cos();
        Self {
            a: c,
            b: s,
            c: -s,
            d: c,
            tx: 0.0,
            ty: 0.0,
        }
    }

    /// `self ∘ other`: apply `other` first, then `self`.
    #[must_use]
    #[allow(clippy::suspicious_operation_groupings)]
    pub fn compose(&self, other: &Self) -> Self {
        // 2D affine: M ∘ N = [a c tx; b d ty; 0 0 1] · [a' c' tx'; b' d' ty'; 0 0 1].
        // The (a, c) / (b, d) cross-terms are intentional, not a bug.
        Self {
            a: self.a.mul_add(other.a, self.c * other.b),
            b: self.b.mul_add(other.a, self.d * other.b),
            c: self.a.mul_add(other.c, self.c * other.d),
            d: self.b.mul_add(other.c, self.d * other.d),
            tx: self.a.mul_add(other.tx, self.c.mul_add(other.ty, self.tx)),
            ty: self.b.mul_add(other.tx, self.d.mul_add(other.ty, self.ty)),
        }
    }

    /// Returns the inverse transform, or `None` for a singular matrix.
    #[must_use]
    pub fn invert(&self) -> Option<Self> {
        let det = self.a.mul_add(self.d, -(self.b * self.c));
        if det.abs() < f64::EPSILON {
            return None;
        }
        let inv_det = 1.0 / det;
        Some(Self {
            a: self.d * inv_det,
            b: -self.b * inv_det,
            c: -self.c * inv_det,
            d: self.a * inv_det,
            tx: self.c.mul_add(self.ty, -(self.d * self.tx)) * inv_det,
            ty: self.b.mul_add(self.tx, -(self.a * self.ty)) * inv_det,
        })
    }

    /// Apply this transform to `point`.
    #[must_use]
    pub const fn apply_point(&self, point: Point2D) -> Point2D {
        Point2D::new(
            self.a.mul_add(point.x, self.c.mul_add(point.y, self.tx)),
            self.b.mul_add(point.x, self.d.mul_add(point.y, self.ty)),
        )
    }
}

impl Default for Transform2D {
    fn default() -> Self {
        Self::IDENTITY
    }
}

/// RGBA color with channels in `[0.0, 1.0]`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RgbaColor {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl RgbaColor {
    pub const BLACK: Self = Self::new(0.0, 0.0, 0.0, 1.0);
    pub const WHITE: Self = Self::new(1.0, 1.0, 1.0, 1.0);
    pub const TRANSPARENT: Self = Self::new(0.0, 0.0, 0.0, 0.0);
    /// `KChat` primary accent `#7C3AED`.
    pub const KCHAT_PRIMARY: Self = Self::new(0.486, 0.227, 0.929, 1.0);

    #[must_use]
    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    /// Parse a CSS hex string (`#RRGGBB` or `#RRGGBBAA`) into an [`RgbaColor`].
    pub fn from_hex(hex: &str) -> Option<Self> {
        let hex = hex.trim_start_matches('#');
        let (r, g, b, a) = match hex.len() {
            6 => (
                u8::from_str_radix(&hex[0..2], 16).ok()?,
                u8::from_str_radix(&hex[2..4], 16).ok()?,
                u8::from_str_radix(&hex[4..6], 16).ok()?,
                255,
            ),
            8 => (
                u8::from_str_radix(&hex[0..2], 16).ok()?,
                u8::from_str_radix(&hex[2..4], 16).ok()?,
                u8::from_str_radix(&hex[4..6], 16).ok()?,
                u8::from_str_radix(&hex[6..8], 16).ok()?,
            ),
            _ => return None,
        };
        Some(Self::new(
            f32::from(r) / 255.0,
            f32::from(g) / 255.0,
            f32::from(b) / 255.0,
            f32::from(a) / 255.0,
        ))
    }

    /// Encode as `#RRGGBBAA`.
    #[must_use]
    pub fn to_hex(&self) -> String {
        let r = (self.r.clamp(0.0, 1.0) * 255.0).round() as u8;
        let g = (self.g.clamp(0.0, 1.0) * 255.0).round() as u8;
        let b = (self.b.clamp(0.0, 1.0) * 255.0).round() as u8;
        let a = (self.a.clamp(0.0, 1.0) * 255.0).round() as u8;
        format!("#{r:02X}{g:02X}{b:02X}{a:02X}")
    }
}

/// What kind of node this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeType {
    Page,
    Artboard,
    GroupLayer,
    VectorLayer,
    RasterLayer,
    TextLayer,
    ComponentLayer,
    LayoutFrame,
}

impl NodeType {
    /// True when this node type may have children.
    #[must_use]
    pub const fn is_container(self) -> bool {
        matches!(
            self,
            Self::Page
                | Self::Artboard
                | Self::GroupLayer
                | Self::LayoutFrame
                | Self::ComponentLayer
        )
    }
}

/// Categorisation for the built-in artboard presets so the home page
/// and the new-artboard dialog can group them in the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresetCategory {
    WebDesktop,
    WebTablet,
    WebMobile,
    SocialMedia,
    Print,
    Custom,
}

/// A named artboard size offered as a preset in the UI.
///
/// `width` / `height` are in document units (pixels). The set is
/// surfaced to the host through [`crate::project::Project`] /
/// `kcreate_bridge` so React presets and Rust core share one source
/// of truth.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArtboardPreset {
    pub name: String,
    pub width: f64,
    pub height: f64,
    pub category: PresetCategory,
}

impl ArtboardPreset {
    #[must_use]
    pub fn new(name: impl Into<String>, width: f64, height: f64, category: PresetCategory) -> Self {
        Self {
            name: name.into(),
            width,
            height,
            category,
        }
    }
}

/// Built-in artboard presets. Matches the sizes the PROPOSAL.md §4.2
/// "App / Website UI", "Pitch Deck", "Social Post", and
/// "Flyer / Poster / Brochure" home-screen affordances are spec'd
/// against, plus the most common social-media and print formats so
/// the new-artboard dialog can offer a useful grid without falling
/// back to the custom-size escape hatch.
///
/// Print sizes are at 300dpi (A4 = 2480×3508 px, US Letter =
/// 2550×3300 px), which is the canonical "print-ready" resolution.
#[must_use]
pub fn standard_presets() -> Vec<ArtboardPreset> {
    vec![
        // Web — desktop
        ArtboardPreset::new("Desktop", 1440.0, 900.0, PresetCategory::WebDesktop),
        ArtboardPreset::new("Laptop", 1280.0, 800.0, PresetCategory::WebDesktop),
        ArtboardPreset::new("Desktop HD", 1920.0, 1080.0, PresetCategory::WebDesktop),
        ArtboardPreset::new("MacBook Pro 14", 1512.0, 982.0, PresetCategory::WebDesktop),
        // Web — tablet
        ArtboardPreset::new("Tablet", 768.0, 1024.0, PresetCategory::WebTablet),
        ArtboardPreset::new("iPad Pro 11", 834.0, 1194.0, PresetCategory::WebTablet),
        // Web — mobile
        ArtboardPreset::new("Mobile", 375.0, 812.0, PresetCategory::WebMobile),
        ArtboardPreset::new("iPhone 15", 393.0, 852.0, PresetCategory::WebMobile),
        ArtboardPreset::new("Android", 360.0, 800.0, PresetCategory::WebMobile),
        // Social media
        ArtboardPreset::new(
            "Instagram Post",
            1080.0,
            1080.0,
            PresetCategory::SocialMedia,
        ),
        ArtboardPreset::new(
            "Instagram Story",
            1080.0,
            1920.0,
            PresetCategory::SocialMedia,
        ),
        ArtboardPreset::new("Twitter / X", 1200.0, 675.0, PresetCategory::SocialMedia),
        ArtboardPreset::new("Facebook Cover", 820.0, 312.0, PresetCategory::SocialMedia),
        ArtboardPreset::new("LinkedIn Post", 1200.0, 627.0, PresetCategory::SocialMedia),
        ArtboardPreset::new(
            "YouTube Thumbnail",
            1280.0,
            720.0,
            PresetCategory::SocialMedia,
        ),
        // Print (300dpi)
        ArtboardPreset::new("A4", 2480.0, 3508.0, PresetCategory::Print),
        ArtboardPreset::new("US Letter", 2550.0, 3300.0, PresetCategory::Print),
        ArtboardPreset::new("A3", 3508.0, 4961.0, PresetCategory::Print),
        ArtboardPreset::new("US Legal", 2550.0, 4200.0, PresetCategory::Print),
        ArtboardPreset::new("Business Card", 1050.0, 600.0, PresetCategory::Print),
    ]
}

/// Blend mode (matches SVG and Figma).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BlendMode {
    #[default]
    Normal,
    Multiply,
    Screen,
    Overlay,
    Darken,
    Lighten,
    ColorDodge,
    ColorBurn,
    HardLight,
    SoftLight,
    Difference,
    Exclusion,
}

/// Visual effects applied to a node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Effect {
    Blur {
        radius: f64,
    },
    Shadow {
        offset_x: f64,
        offset_y: f64,
        blur: f64,
        spread: f64,
        color: RgbaColor,
    },
    Glow {
        radius: f64,
        color: RgbaColor,
    },
}

/// A single stop in a gradient.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GradientStop {
    pub offset: f64,
    pub color: RgbaColor,
}

/// What kind of gradient.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "shape", rename_all = "snake_case")]
pub enum GradientKind {
    Linear {
        from: Point2D,
        to: Point2D,
        stops: Vec<GradientStop>,
    },
    Radial {
        center: Point2D,
        radius: f64,
        stops: Vec<GradientStop>,
    },
}

/// How a node is filled.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FillStyle {
    None,
    Solid(RgbaColor),
    Gradient(GradientKind),
}

/// How a node is stroked.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StrokeStyle {
    pub color: RgbaColor,
    pub width: f64,
    pub dash: Vec<f64>,
}

impl Default for StrokeStyle {
    fn default() -> Self {
        Self {
            color: RgbaColor::BLACK,
            width: 1.0,
            dash: Vec::new(),
        }
    }
}

/// Painted appearance of a node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeStyle {
    pub fill: FillStyle,
    pub stroke: Option<StrokeStyle>,
    pub corner_radius: f64,
}

impl Default for NodeStyle {
    fn default() -> Self {
        Self {
            fill: FillStyle::Solid(RgbaColor::WHITE),
            stroke: None,
            corner_radius: 0.0,
        }
    }
}

/// How a node behaves when its parent resizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Constraint {
    #[default]
    Fixed,
    Min,
    Max,
    Center,
    Scale,
    Stretch,
}

/// Horizontal + vertical constraint pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct Constraints {
    pub horizontal: Constraint,
    pub vertical: Constraint,
}

/// Metadata key used on a node to store its prototype interactions
/// (serialized `Vec<Interaction>`). See [`Interaction`].
pub const INTERACTIONS_METADATA_KEY: &str = "interactions";

/// Metadata key used on a `Page` node to store its [`PageLayout`].
pub const PAGE_LAYOUT_METADATA_KEY: &str = "page_layout";

/// Metadata key used on a `Page` node to mark it as a *master page*
/// (template inherited by content pages).
pub const MASTER_PAGE_METADATA_KEY: &str = "is_master";

/// What triggers an [`Interaction`] in prototype playback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionTrigger {
    /// Mouse / touch click.
    Click,
    /// Pointer hover (no click).
    Hover,
    /// Mouse button held down (or long-press on touch).
    Press,
}

/// What an [`Interaction`] does when its trigger fires.
///
/// All fields use `Uuid` so the action can be serialised intact even
/// when the target node has not yet been resolved by the player.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InteractionAction {
    /// Navigate the prototype player to a specific artboard.
    NavigateTo { target_artboard_id: Uuid },
    /// Scroll the canvas so `target_node_id` is in view.
    ScrollTo { target_node_id: Uuid },
    /// Open an artboard as an overlay above the current artboard.
    OpenOverlay { overlay_artboard_id: Uuid },
    /// Close the topmost overlay (no target).
    CloseOverlay,
    /// Step one navigation entry back in the player's history.
    Back,
}

/// A prototype interaction bound to a [`Node`].
///
/// Persisted as a JSON array under
/// [`INTERACTIONS_METADATA_KEY`] in `Node::metadata`. The renderer
/// ignores interactions; only the prototype player reads them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Interaction {
    pub id: Uuid,
    pub trigger: InteractionTrigger,
    pub action: InteractionAction,
}

impl Interaction {
    #[must_use]
    pub fn new(trigger: InteractionTrigger, action: InteractionAction) -> Self {
        Self {
            id: Uuid::new_v4(),
            trigger,
            action,
        }
    }
}

/// Print/canvas page sizes used by Layout Studio.
///
/// Dimensions for the predefined variants are returned in **mm**
/// portrait orientation by [`PageSize::dimensions_mm`]. Apply
/// [`PageOrientation::Landscape`] to swap.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(clippy::derive_partial_eq_without_eq)] // contains f64 in Custom
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PageSize {
    A4,
    A3,
    A5,
    Letter,
    Legal,
    Tabloid,
    /// 16:9 presentation slide (10×5.625 in @ 72dpi → 254×142.875 mm).
    Presentation16x9,
    /// 4:3 presentation slide (10×7.5 in → 254×190.5 mm).
    Presentation4x3,
    /// Caller-supplied size in mm.
    Custom {
        width_mm: f64,
        height_mm: f64,
    },
}

impl PageSize {
    /// Portrait `(width_mm, height_mm)`.
    ///
    /// ISO 216 sizes follow the official millimetre dimensions
    /// (A4 = 210×297). North-American sizes are converted from
    /// inches at exactly 25.4 mm/in to avoid floating-point drift.
    #[must_use]
    pub fn dimensions_mm(&self) -> (f64, f64) {
        match self {
            Self::A3 => (297.0, 420.0),
            Self::A4 => (210.0, 297.0),
            Self::A5 => (148.0, 210.0),
            Self::Letter => (8.5 * 25.4, 11.0 * 25.4),
            Self::Legal => (8.5 * 25.4, 14.0 * 25.4),
            Self::Tabloid => (11.0 * 25.4, 17.0 * 25.4),
            Self::Presentation16x9 => (10.0 * 25.4, 5.625 * 25.4),
            Self::Presentation4x3 => (10.0 * 25.4, 7.5 * 25.4),
            Self::Custom {
                width_mm,
                height_mm,
            } => (*width_mm, *height_mm),
        }
    }
}

/// Orientation of a [`PageLayout`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PageOrientation {
    #[default]
    Portrait,
    Landscape,
}

/// Page margins in millimetres.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[allow(clippy::derive_partial_eq_without_eq)]
pub struct Margins {
    pub top_mm: f64,
    pub right_mm: f64,
    pub bottom_mm: f64,
    pub left_mm: f64,
}

impl Margins {
    #[must_use]
    pub const fn uniform(mm: f64) -> Self {
        Self {
            top_mm: mm,
            right_mm: mm,
            bottom_mm: mm,
            left_mm: mm,
        }
    }
}

impl Default for Margins {
    fn default() -> Self {
        Self::uniform(0.0)
    }
}

/// Layout metadata for a `Page` node — size, orientation, margins,
/// optional master-page reference, optional page number.
///
/// Persisted on `Page::metadata` under [`PAGE_LAYOUT_METADATA_KEY`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(clippy::derive_partial_eq_without_eq)]
pub struct PageLayout {
    pub page_size: PageSize,
    pub orientation: PageOrientation,
    pub margins: Margins,
    pub master_page_id: Option<Uuid>,
    pub page_number: Option<u32>,
}

impl PageLayout {
    /// New layout with no master, no page number, zero margins.
    #[must_use]
    pub fn new(page_size: PageSize, orientation: PageOrientation) -> Self {
        Self {
            page_size,
            orientation,
            margins: Margins::default(),
            master_page_id: None,
            page_number: None,
        }
    }

    /// `(width_mm, height_mm)` after orientation is applied.
    #[must_use]
    pub fn dimensions_mm(&self) -> (f64, f64) {
        let (w, h) = self.page_size.dimensions_mm();
        match self.orientation {
            PageOrientation::Portrait => (w, h),
            PageOrientation::Landscape => (h, w),
        }
    }
}

impl Default for PageLayout {
    fn default() -> Self {
        Self::new(PageSize::A4, PageOrientation::Portrait)
    }
}

/// A node in the document graph.
///
/// Fields are flat (no recursive `Vec<Node>`); the parent/child
/// relationship is stored via [`Node::parent_id`] and [`Node::children`]
/// (ids), and the actual node bodies live in the
/// [`crate::document::DocumentGraph`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: Uuid,
    pub node_type: NodeType,
    pub parent_id: Option<Uuid>,
    pub children: Vec<Uuid>,
    pub bounds: Bounds,
    pub transform: Transform2D,
    pub opacity: f32,
    pub blend_mode: BlendMode,
    pub visible: bool,
    pub locked: bool,
    pub name: String,
    pub style: NodeStyle,
    pub effects: Vec<Effect>,
    pub constraints: Constraints,
    pub metadata: HashMap<String, serde_json::Value>,
    pub version: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Node {
    /// Build a new node with the given `node_type` and `name`. The
    /// caller is responsible for inserting it into the document graph.
    #[must_use]
    pub fn new(node_type: NodeType, name: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            node_type,
            parent_id: None,
            children: Vec::new(),
            bounds: Bounds::ZERO,
            transform: Transform2D::IDENTITY,
            opacity: 1.0,
            blend_mode: BlendMode::Normal,
            visible: true,
            locked: false,
            name: name.into(),
            style: NodeStyle::default(),
            effects: Vec::new(),
            constraints: Constraints::default(),
            metadata: HashMap::new(),
            version: 0,
            created_at: now,
            updated_at: now,
        }
    }

    /// Bump version and `updated_at`. Call after every mutation.
    pub fn touch(&mut self) {
        self.version += 1;
        self.updated_at = Utc::now();
    }

    /// Decode the node's stored interactions (Block A / prototype mode).
    ///
    /// Returns an empty `Vec` when the metadata key is absent or the
    /// payload is malformed — readers always get a usable list, and
    /// the renderer never crashes on partial data.
    #[must_use]
    pub fn interactions(&self) -> Vec<Interaction> {
        self.metadata
            .get(INTERACTIONS_METADATA_KEY)
            .and_then(|v| serde_json::from_value::<Vec<Interaction>>(v.clone()).ok())
            .unwrap_or_default()
    }

    /// Replace the node's interactions metadata. Touches the node.
    pub fn set_interactions(&mut self, interactions: &[Interaction]) {
        let value = serde_json::to_value(interactions).unwrap_or(serde_json::Value::Null);
        self.metadata
            .insert(INTERACTIONS_METADATA_KEY.to_string(), value);
        self.touch();
    }

    /// Decode the node's [`PageLayout`] metadata. Returns `None` when
    /// the node is not a `Page` or has no layout attached.
    #[must_use]
    pub fn page_layout(&self) -> Option<PageLayout> {
        if self.node_type != NodeType::Page {
            return None;
        }
        self.metadata
            .get(PAGE_LAYOUT_METADATA_KEY)
            .and_then(|v| serde_json::from_value::<PageLayout>(v.clone()).ok())
    }

    /// Persist a [`PageLayout`] onto a `Page` node. No-op on other
    /// node types.
    pub fn set_page_layout(&mut self, layout: &PageLayout) {
        if self.node_type != NodeType::Page {
            return;
        }
        let value = serde_json::to_value(layout).unwrap_or(serde_json::Value::Null);
        self.metadata
            .insert(PAGE_LAYOUT_METADATA_KEY.to_string(), value);
        self.touch();
    }

    /// True when this node is a master page (template).
    #[must_use]
    pub fn is_master_page(&self) -> bool {
        self.node_type == NodeType::Page
            && self
                .metadata
                .get(MASTER_PAGE_METADATA_KEY)
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
    }

    /// Flag this `Page` as a master page. No-op on other node types.
    pub fn set_master_page(&mut self, master: bool) {
        if self.node_type != NodeType::Page {
            return;
        }
        self.metadata.insert(
            MASTER_PAGE_METADATA_KEY.to_string(),
            serde_json::Value::Bool(master),
        );
        self.touch();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_union_covers_inputs() {
        let a = Bounds::new(0.0, 0.0, 10.0, 10.0);
        let b = Bounds::new(5.0, 5.0, 20.0, 20.0);
        let u = a.union(&b);
        assert_eq!(u, Bounds::new(0.0, 0.0, 25.0, 25.0));
        assert!(u.contains_bounds(&a));
        assert!(u.contains_bounds(&b));
    }

    #[test]
    fn bounds_intersection_overlapping_returns_some() {
        let a = Bounds::new(0.0, 0.0, 10.0, 10.0);
        let b = Bounds::new(5.0, 5.0, 10.0, 10.0);
        assert_eq!(a.intersection(&b), Some(Bounds::new(5.0, 5.0, 5.0, 5.0)));
    }

    #[test]
    fn bounds_intersection_disjoint_returns_none() {
        let a = Bounds::new(0.0, 0.0, 5.0, 5.0);
        let b = Bounds::new(10.0, 10.0, 5.0, 5.0);
        assert!(a.intersection(&b).is_none());
    }

    #[test]
    fn bounds_contains_point_inclusive_min_exclusive_max() {
        let b = Bounds::new(0.0, 0.0, 10.0, 10.0);
        assert!(b.contains_point(Point2D::new(0.0, 0.0)));
        assert!(b.contains_point(Point2D::new(5.0, 5.0)));
        assert!(!b.contains_point(Point2D::new(10.0, 5.0)));
        assert!(!b.contains_point(Point2D::new(5.0, 10.0)));
    }

    #[test]
    fn transform_identity_round_trip() {
        let t = Transform2D::IDENTITY;
        let p = Point2D::new(3.0, 4.0);
        assert_eq!(t.apply_point(p), p);
    }

    #[test]
    fn transform_translation_apply() {
        let t = Transform2D::translation(10.0, -5.0);
        assert_eq!(
            t.apply_point(Point2D::new(1.0, 1.0)),
            Point2D::new(11.0, -4.0)
        );
    }

    #[test]
    fn transform_scale_apply() {
        let t = Transform2D::scale(2.0, 3.0);
        assert_eq!(
            t.apply_point(Point2D::new(4.0, 5.0)),
            Point2D::new(8.0, 15.0)
        );
    }

    #[test]
    fn transform_compose_translate_then_scale() {
        // Apply translation first, then scale: x' = 2*(x + 1) = 2x + 2.
        let scale = Transform2D::scale(2.0, 2.0);
        let translate = Transform2D::translation(1.0, 1.0);
        let composed = scale.compose(&translate);
        assert_eq!(
            composed.apply_point(Point2D::new(0.0, 0.0)),
            Point2D::new(2.0, 2.0)
        );
    }

    #[test]
    fn transform_invert_round_trip() {
        let t = Transform2D::scale(2.0, 4.0).compose(&Transform2D::translation(3.0, -1.0));
        let inv = t.invert().expect("non-singular transform");
        let p = Point2D::new(5.0, 7.0);
        let q = inv.apply_point(t.apply_point(p));
        assert!((q.x - p.x).abs() < 1e-9);
        assert!((q.y - p.y).abs() < 1e-9);
    }

    #[test]
    fn transform_invert_singular_returns_none() {
        let t = Transform2D {
            a: 0.0,
            b: 0.0,
            c: 0.0,
            d: 0.0,
            tx: 0.0,
            ty: 0.0,
        };
        assert!(t.invert().is_none());
    }

    #[test]
    fn rgba_hex_round_trip() {
        let c = RgbaColor::from_hex("#7C3AED").expect("valid hex");
        assert_eq!(c.to_hex(), "#7C3AEDFF");
        let c2 = RgbaColor::from_hex("#FF00FF80").expect("valid 8-digit hex");
        assert_eq!(c2.to_hex(), "#FF00FF80");
    }

    #[test]
    fn rgba_hex_invalid_returns_none() {
        assert!(RgbaColor::from_hex("not-a-color").is_none());
        assert!(RgbaColor::from_hex("#12345").is_none());
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn node_new_initializes_defaults() {
        let n = Node::new(NodeType::VectorLayer, "rect");
        assert_eq!(n.node_type, NodeType::VectorLayer);
        assert_eq!(n.name, "rect");
        assert!(n.children.is_empty());
        assert_eq!(n.opacity, 1.0);
        assert_eq!(n.version, 0);
        assert!(n.visible);
        assert!(!n.locked);
    }

    #[test]
    fn node_touch_bumps_version() {
        let mut n = Node::new(NodeType::VectorLayer, "r");
        let v0 = n.version;
        n.touch();
        assert_eq!(n.version, v0 + 1);
    }

    #[test]
    fn node_serialize_roundtrip() {
        let n = Node::new(NodeType::Artboard, "a");
        let s = serde_json::to_string(&n).expect("serialize");
        let n2: Node = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(n, n2);
    }

    #[test]
    fn node_type_container_classification() {
        assert!(NodeType::Page.is_container());
        assert!(NodeType::Artboard.is_container());
        assert!(NodeType::GroupLayer.is_container());
        assert!(NodeType::LayoutFrame.is_container());
        assert!(NodeType::ComponentLayer.is_container());
        assert!(!NodeType::VectorLayer.is_container());
        assert!(!NodeType::RasterLayer.is_container());
        assert!(!NodeType::TextLayer.is_container());
    }

    #[test]
    fn blend_mode_serializes_snake_case() {
        let s = serde_json::to_string(&BlendMode::ColorDodge).expect("serialize");
        assert_eq!(s, r#""color_dodge""#);
    }

    #[test]
    fn effect_serialize_roundtrip() {
        let e = Effect::Shadow {
            offset_x: 1.0,
            offset_y: 2.0,
            blur: 3.0,
            spread: 0.5,
            color: RgbaColor::BLACK,
        };
        let s = serde_json::to_string(&e).expect("serialize");
        let e2: Effect = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(e, e2);
    }

    #[test]
    fn fill_style_serialize_roundtrip_solid_and_gradient() {
        let solid = FillStyle::Solid(RgbaColor::KCHAT_PRIMARY);
        let s = serde_json::to_string(&solid).expect("serialize");
        let solid2: FillStyle = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(solid, solid2);

        let gradient = FillStyle::Gradient(GradientKind::Linear {
            from: Point2D::new(0.0, 0.0),
            to: Point2D::new(1.0, 1.0),
            stops: vec![
                GradientStop {
                    offset: 0.0,
                    color: RgbaColor::WHITE,
                },
                GradientStop {
                    offset: 1.0,
                    color: RgbaColor::BLACK,
                },
            ],
        });
        let s2 = serde_json::to_string(&gradient).expect("serialize");
        let g2: FillStyle = serde_json::from_str(&s2).expect("deserialize");
        assert_eq!(gradient, g2);
    }

    #[test]
    fn constraints_default_is_fixed() {
        let c = Constraints::default();
        assert_eq!(c.horizontal, Constraint::Fixed);
        assert_eq!(c.vertical, Constraint::Fixed);
    }

    #[test]
    fn standard_presets_includes_all_categories() {
        let presets = standard_presets();
        assert!(presets.iter().any(|p| p.name == "Desktop"
            && (p.width - 1440.0).abs() < f64::EPSILON
            && (p.height - 900.0).abs() < f64::EPSILON));
        assert!(presets
            .iter()
            .any(|p| p.name == "Instagram Post" && p.category == PresetCategory::SocialMedia));
        assert!(presets
            .iter()
            .any(|p| p.name == "A4" && p.category == PresetCategory::Print));
        // Every preset category that's not "Custom" should be present.
        for cat in [
            PresetCategory::WebDesktop,
            PresetCategory::WebTablet,
            PresetCategory::WebMobile,
            PresetCategory::SocialMedia,
            PresetCategory::Print,
        ] {
            assert!(
                presets.iter().any(|p| p.category == cat),
                "missing preset for category {cat:?}",
            );
        }
        // No preset should have a zero/negative size.
        for p in &presets {
            assert!(p.width > 0.0, "preset {} has non-positive width", p.name);
            assert!(p.height > 0.0, "preset {} has non-positive height", p.name);
        }
    }

    #[test]
    fn artboard_preset_round_trips_through_json() {
        let preset = ArtboardPreset::new("Custom", 100.0, 200.0, PresetCategory::Custom);
        let s = serde_json::to_string(&preset).expect("serialize");
        let restored: ArtboardPreset = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(preset, restored);
    }

    #[test]
    fn preset_category_serializes_snake_case() {
        let s = serde_json::to_string(&PresetCategory::WebDesktop).expect("serialize");
        assert_eq!(s, r#""web_desktop""#);
    }

    #[test]
    fn page_size_dimensions_match_iso_216() {
        assert_eq!(PageSize::A4.dimensions_mm(), (210.0, 297.0));
        assert_eq!(PageSize::A3.dimensions_mm(), (297.0, 420.0));
        assert_eq!(PageSize::A5.dimensions_mm(), (148.0, 210.0));
    }

    #[test]
    fn page_size_dimensions_match_us() {
        let (w, h) = PageSize::Letter.dimensions_mm();
        assert!((w - 215.9).abs() < 1e-6, "Letter width = {w}");
        assert!((h - 279.4).abs() < 1e-6, "Letter height = {h}");
        let (lw, lh) = PageSize::Legal.dimensions_mm();
        assert!((lw - 215.9).abs() < 1e-6);
        assert!((lh - 355.6).abs() < 1e-6);
    }

    #[test]
    fn page_size_custom_round_trip() {
        let s = PageSize::Custom {
            width_mm: 100.0,
            height_mm: 200.0,
        };
        assert_eq!(s.dimensions_mm(), (100.0, 200.0));
        let json = serde_json::to_string(&s).expect("serialize");
        let back: PageSize = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, s);
    }

    #[test]
    fn page_layout_landscape_flips_dimensions() {
        let portrait = PageLayout::new(PageSize::A4, PageOrientation::Portrait);
        assert_eq!(portrait.dimensions_mm(), (210.0, 297.0));
        let landscape = PageLayout::new(PageSize::A4, PageOrientation::Landscape);
        assert_eq!(landscape.dimensions_mm(), (297.0, 210.0));
    }

    #[test]
    fn page_layout_round_trips_through_json() {
        let layout = PageLayout {
            page_size: PageSize::Letter,
            orientation: PageOrientation::Landscape,
            margins: Margins::uniform(20.0),
            master_page_id: Some(Uuid::new_v4()),
            page_number: Some(3),
        };
        let json = serde_json::to_string(&layout).expect("serialize");
        let back: PageLayout = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, layout);
    }

    #[test]
    fn node_page_layout_round_trip_via_metadata() {
        let mut page = Node::new(NodeType::Page, "Page 1");
        assert!(page.page_layout().is_none());
        let layout = PageLayout::new(PageSize::A4, PageOrientation::Portrait);
        page.set_page_layout(&layout);
        assert_eq!(page.page_layout(), Some(layout));
        // On non-Page nodes, set/get are no-ops.
        let mut layer = Node::new(NodeType::VectorLayer, "Path");
        layer.set_page_layout(&PageLayout::default());
        assert!(layer.page_layout().is_none());
    }

    #[test]
    fn node_master_page_flag_round_trip() {
        let mut page = Node::new(NodeType::Page, "Master");
        assert!(!page.is_master_page());
        page.set_master_page(true);
        assert!(page.is_master_page());
        page.set_master_page(false);
        assert!(!page.is_master_page());
    }

    #[test]
    fn interaction_round_trips_through_json() {
        let interaction = Interaction::new(
            InteractionTrigger::Click,
            InteractionAction::NavigateTo {
                target_artboard_id: Uuid::new_v4(),
            },
        );
        let json = serde_json::to_string(&interaction).expect("serialize");
        let back: Interaction = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, interaction);
    }

    #[test]
    fn node_interactions_round_trip_via_metadata() {
        let mut node = Node::new(NodeType::VectorLayer, "Button");
        assert!(node.interactions().is_empty());
        let interactions = vec![
            Interaction::new(
                InteractionTrigger::Click,
                InteractionAction::NavigateTo {
                    target_artboard_id: Uuid::new_v4(),
                },
            ),
            Interaction::new(InteractionTrigger::Hover, InteractionAction::Back),
        ];
        node.set_interactions(&interactions);
        assert_eq!(node.interactions(), interactions);
    }

    #[test]
    fn interaction_action_close_overlay_has_no_target() {
        let action = InteractionAction::CloseOverlay;
        let json = serde_json::to_string(&action).expect("serialize");
        // Must not silently emit a target_artboard_id or similar; only `kind`.
        assert_eq!(json, r#"{"kind":"close_overlay"}"#);
    }

    #[test]
    fn page_orientation_serializes_snake_case() {
        let s = serde_json::to_string(&PageOrientation::Landscape).expect("serialize");
        assert_eq!(s, r#""landscape""#);
    }
}
