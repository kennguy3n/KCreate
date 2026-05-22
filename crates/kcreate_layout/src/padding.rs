//! Shared padding model used by both flex and grid layouts.

use serde::{Deserialize, Serialize};

/// Per-edge padding inside a layout container. All four values are in
/// document units (px) and must be non-negative; negative values are
/// clamped to zero by the layout solvers.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct Padding {
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
    pub left: f64,
}

impl Padding {
    /// Construct a padding with all four edges equal.
    #[must_use]
    pub const fn uniform(v: f64) -> Self {
        Self {
            top: v,
            right: v,
            bottom: v,
            left: v,
        }
    }

    #[must_use]
    pub const fn new(top: f64, right: f64, bottom: f64, left: f64) -> Self {
        Self {
            top,
            right,
            bottom,
            left,
        }
    }

    /// Clamp each edge to `[0, inf)`. Solvers call this defensively
    /// so a malformed config never produces overlapping content
    /// rects.
    #[must_use]
    pub fn normalize(self) -> Self {
        Self {
            top: self.top.max(0.0),
            right: self.right.max(0.0),
            bottom: self.bottom.max(0.0),
            left: self.left.max(0.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Padding;

    #[test]
    fn uniform_sets_all_edges() {
        let p = Padding::uniform(8.0);
        assert_eq!(p.top, 8.0);
        assert_eq!(p.right, 8.0);
        assert_eq!(p.bottom, 8.0);
        assert_eq!(p.left, 8.0);
    }

    #[test]
    fn normalize_clamps_negative_to_zero() {
        let p = Padding::new(-1.0, 2.0, -3.0, 4.0).normalize();
        assert_eq!(p.top, 0.0);
        assert_eq!(p.right, 2.0);
        assert_eq!(p.bottom, 0.0);
        assert_eq!(p.left, 4.0);
    }
}
