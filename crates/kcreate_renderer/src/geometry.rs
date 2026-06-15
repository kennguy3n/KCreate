//! Geometry primitives consumed by the renderer.
//!
//! These are intentionally narrow to what the renderer needs to rasterize.
//! When the dedicated `kcreate_vector` crate lands (per the other items
//! in the Phase 0 plan), this module can be replaced by a re-export.

use serde::{Deserialize, Serialize};

/// A 2D point in scene space.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point2 {
    pub x: f32,
    pub y: f32,
}

impl Point2 {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

impl From<(f32, f32)> for Point2 {
    fn from((x, y): (f32, f32)) -> Self {
        Self { x, y }
    }
}

/// A 2D vector / offset.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    pub const ZERO: Self = Self { x: 0.0, y: 0.0 };

    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    #[must_use]
    pub const fn scale(self, s: f32) -> Self {
        Self::new(self.x * s, self.y * s)
    }
}

impl std::ops::Add for Vec2 {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self::new(self.x + rhs.x, self.y + rhs.y)
    }
}

impl std::ops::Sub for Vec2 {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self::Output {
        Self::new(self.x - rhs.x, self.y - rhs.y)
    }
}

/// Axis-aligned rectangle in scene space.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn max_x(&self) -> f32 {
        self.x + self.width
    }

    pub fn max_y(&self) -> f32 {
        self.y + self.height
    }

    pub fn is_empty(&self) -> bool {
        self.width <= 0.0 || self.height <= 0.0
    }

    pub fn contains(&self, p: Point2) -> bool {
        p.x >= self.x && p.x < self.max_x() && p.y >= self.y && p.y < self.max_y()
    }

    pub fn intersects(&self, other: &Self) -> bool {
        !(self.max_x() <= other.x
            || other.max_x() <= self.x
            || self.max_y() <= other.y
            || other.max_y() <= self.y)
    }

    #[must_use]
    pub fn union(&self, other: &Self) -> Self {
        if self.is_empty() {
            return *other;
        }
        if other.is_empty() {
            return *self;
        }
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        let max_x = self.max_x().max(other.max_x());
        let max_y = self.max_y().max(other.max_y());
        Self::new(x, y, max_x - x, max_y - y)
    }

    /// Inflate by `n` units on every side (negative shrinks).
    #[must_use]
    pub fn inflate(&self, n: f32) -> Self {
        Self::new(
            self.x - n,
            self.y - n,
            2.0f32.mul_add(n, self.width),
            2.0f32.mul_add(n, self.height),
        )
    }
}

/// Linear sRGB color, premultiplied-alpha aware values are NOT stored here —
/// callers pass straight-alpha values which the rasterizer premultiplies
/// internally as needed.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const TRANSPARENT: Self = Self {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 0.0,
    };

    pub const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    pub fn to_rgba8(self) -> [u8; 4] {
        [
            (self.r.clamp(0.0, 1.0) * 255.0) as u8,
            (self.g.clamp(0.0, 1.0) * 255.0) as u8,
            (self.b.clamp(0.0, 1.0) * 255.0) as u8,
            (self.a.clamp(0.0, 1.0) * 255.0) as u8,
        ]
    }

    pub fn to_tiny_skia(self) -> tiny_skia::Color {
        tiny_skia::Color::from_rgba(
            self.r.clamp(0.0, 1.0),
            self.g.clamp(0.0, 1.0),
            self.b.clamp(0.0, 1.0),
            self.a.clamp(0.0, 1.0),
        )
        .unwrap_or(tiny_skia::Color::BLACK)
    }
}

/// Stroke specification.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Stroke {
    pub color: Color,
    pub width: f32,
}

impl Stroke {
    pub const fn new(color: Color, width: f32) -> Self {
        Self { color, width }
    }
}

/// A fill paint: a flat color or a linear / radial gradient.
///
/// Gradient coordinates live in the **same local space as the shape
/// geometry they fill** (path points, rect corners, …). The display-list
/// builder bakes an object's translation into them exactly as it does for
/// the path coordinates, and the CPU backend hands the viewport transform
/// to tiny-skia at draw time — so a gradient pans and zooms locked to its
/// shape. This mirrors the PDF exporter's `node_local_to_pt` convention so
/// raster output matches vector output pixel-for-pixel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Paint {
    /// Flat color.
    Solid(Color),
    /// Axial gradient from `from` to `to`. `stops` are `(offset, color)`
    /// pairs with `offset` in `[0, 1]`, ascending.
    LinearGradient {
        from: Point2,
        to: Point2,
        stops: Vec<(f32, Color)>,
    },
    /// Radial gradient centered at `center` with the given `radius`
    /// (inner radius is 0). `stops` are `(offset, color)` pairs with
    /// `offset` in `[0, 1]`, ascending.
    RadialGradient {
        center: Point2,
        radius: f32,
        stops: Vec<(f32, Color)>,
    },
}

impl Paint {
    /// A single representative color for contexts that cannot paint a
    /// gradient (hairline strokes, degenerate-gradient fallbacks). A
    /// gradient collapses to its first stop, matching the "simplified
    /// gradient" behavior of vector editors. Returns `None` only for a
    /// gradient with no stops.
    pub fn representative_color(&self) -> Option<Color> {
        match self {
            Self::Solid(c) => Some(*c),
            Self::LinearGradient { stops, .. } | Self::RadialGradient { stops, .. } => {
                stops.first().map(|(_, c)| *c)
            }
        }
    }

    /// Translate gradient geometry by `(dx, dy)`. Solid paint is
    /// unaffected. Used to bake an object's translation into its fill so
    /// the gradient stays locked to the (also-translated) geometry.
    #[must_use]
    pub fn translated(&self, dx: f32, dy: f32) -> Self {
        match self {
            Self::Solid(c) => Self::Solid(*c),
            Self::LinearGradient { from, to, stops } => Self::LinearGradient {
                from: Point2::new(from.x + dx, from.y + dy),
                to: Point2::new(to.x + dx, to.y + dy),
                stops: stops.clone(),
            },
            Self::RadialGradient {
                center,
                radius,
                stops,
            } => Self::RadialGradient {
                center: Point2::new(center.x + dx, center.y + dy),
                radius: *radius,
                stops: stops.clone(),
            },
        }
    }
}

/// Style for filled and/or stroked shapes.
///
/// `fill` is no longer `Copy` because [`Paint`] can own a `Vec` of
/// gradient stops; `Style` therefore derives `Clone` (kept `PartialEq`
/// so the pipeline can batch consecutive same-style rects).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Style {
    pub fill: Option<Paint>,
    pub stroke: Option<Stroke>,
}

impl Style {
    pub const fn filled(fill: Color) -> Self {
        Self {
            fill: Some(Paint::Solid(fill)),
            stroke: None,
        }
    }

    pub const fn stroked(stroke: Stroke) -> Self {
        Self {
            fill: None,
            stroke: Some(stroke),
        }
    }

    pub const fn fill_and_stroke(fill: Color, stroke: Stroke) -> Self {
        Self {
            fill: Some(Paint::Solid(fill)),
            stroke: Some(stroke),
        }
    }

    /// Build a style from an arbitrary [`Paint`] (solid or gradient).
    pub const fn painted(fill: Paint) -> Self {
        Self {
            fill: Some(fill),
            stroke: None,
        }
    }

    /// Convenience constructor for a linear-gradient fill.
    pub fn linear_gradient(from: Point2, to: Point2, stops: Vec<(f32, Color)>) -> Self {
        Self {
            fill: Some(Paint::LinearGradient { from, to, stops }),
            stroke: None,
        }
    }

    /// Convenience constructor for a radial-gradient fill.
    pub fn radial_gradient(center: Point2, radius: f32, stops: Vec<(f32, Color)>) -> Self {
        Self {
            fill: Some(Paint::RadialGradient {
                center,
                radius,
                stops,
            }),
            stroke: None,
        }
    }

    /// Return a copy with the fill's gradient geometry translated by
    /// `(dx, dy)`. Solid fills and strokes are untouched.
    #[must_use]
    pub fn translated(&self, dx: f32, dy: f32) -> Self {
        Self {
            fill: self.fill.as_ref().map(|p| p.translated(dx, dy)),
            stroke: self.stroke,
        }
    }
}

/// A single command in a SVG-style path.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum PathCommand {
    MoveTo(Point2),
    LineTo(Point2),
    QuadTo { ctrl: Point2, end: Point2 },
    CubicTo { c1: Point2, c2: Point2, end: Point2 },
    Close,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_contains_inside_and_excludes_outside() {
        let r = Rect::new(0.0, 0.0, 10.0, 10.0);
        assert!(r.contains(Point2::new(5.0, 5.0)));
        assert!(!r.contains(Point2::new(10.0, 5.0))); // half-open on max edge
        assert!(!r.contains(Point2::new(-1.0, 5.0)));
    }

    #[test]
    fn rect_intersects_overlap_and_disjoint() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        let b = Rect::new(5.0, 5.0, 10.0, 10.0);
        let c = Rect::new(20.0, 20.0, 5.0, 5.0);
        let touching = Rect::new(10.0, 0.0, 5.0, 5.0);
        assert!(a.intersects(&b));
        assert!(!a.intersects(&c));
        // Edge-touching rects are NOT considered intersecting (half-open).
        assert!(!a.intersects(&touching));
    }

    #[test]
    fn rect_union_grows_to_cover_both() {
        let a = Rect::new(0.0, 0.0, 5.0, 5.0);
        let b = Rect::new(10.0, 10.0, 5.0, 5.0);
        let u = a.union(&b);
        assert_eq!(u, Rect::new(0.0, 0.0, 15.0, 15.0));
    }

    #[test]
    fn color_to_rgba8_clamps_out_of_range() {
        let c = Color::rgba(2.0, -1.0, 0.5, 0.75);
        assert_eq!(c.to_rgba8(), [255, 0, 127, 191]);
    }
}
