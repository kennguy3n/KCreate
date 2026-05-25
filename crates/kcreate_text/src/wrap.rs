//! Image-text wrap: derive per-line carved frames around obstacles.
//!
//! The single-frame engine ([`paragraph::layout_paragraph`]) treats
//! the frame as a rectangle. Phase 5 adds **obstacle-aware wrapping**:
//! a non-text node (image, vector shape, even another text frame
//! marked as a wrap object) that overlaps a text frame should push
//! lines aside so they avoid the obstacle.
//!
//! Two-pass implementation:
//! 1. Walk the candidate line slots (frame y → y + line_height,
//!    stepping by line_height) and, for each, compute the
//!    obstacle-free horizontal extents.
//! 2. Convert those extents into a series of sub-frames (one
//!    [`FrameRect`] per (line, extent) tuple). The flow engine then
//!    treats each as a normal frame.
//!
//! Wrap modes are taken from
//! [`kcreate_core::node::TextWrapMode`] /
//! [`WrapMode`]. Currently this module exposes `Both`, `Left`,
//! `Right`, and `None` semantics — matching Scribus's text-wrap UI.

use kcreate_core::node::{FrameInsets, TextFrameOptions, TextWrapMode};
use kcreate_core::Bounds;

use crate::flow::FrameRect;

/// Which side(s) of the obstacle the text is allowed to flow on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WrapMode {
    /// Text flows on both sides of the obstacle (split into a left
    /// and a right band per affected line).
    #[default]
    Both,
    /// Text only flows on the left of the obstacle.
    Left,
    /// Text only flows on the right of the obstacle.
    Right,
    /// Text ignores the obstacle entirely.
    None,
}

impl From<TextWrapMode> for WrapMode {
    fn from(value: TextWrapMode) -> Self {
        match value {
            TextWrapMode::BoundingBox | TextWrapMode::Contour => Self::Both,
            TextWrapMode::None => Self::None,
        }
    }
}

/// An obstacle that lines must avoid. The `bounds` are in the same
/// document space as the text frame's `bounds`.
#[derive(Debug, Clone, PartialEq)]
pub struct WrapObstacle {
    pub bounds: Bounds,
    /// Padding around the obstacle bounds, in document units.
    /// Applied symmetrically on all four sides.
    pub margin: f64,
    /// Which side(s) of the obstacle text flows on.
    pub wrap_mode: WrapMode,
}

/// Carve `frame.bounds` into a list of sub-rectangles that the flow
/// engine can lay out lines into without overlapping any obstacle in
/// `obstacles`. Returns the carved frames in **reading order**
/// (top-to-bottom, then left-to-right).
///
/// The carved frames inherit the wrapped frame's `TextFrameOptions`
/// but their per-frame `inset` is reset to zero (the wrap padding is
/// already baked into the carved geometry).
#[must_use]
pub fn carve_frames(frame: &FrameRect, obstacles: &[WrapObstacle]) -> Vec<FrameRect> {
    let line_h =
        frame.options.font_size_hint().max(1.0) * frame.options.line_height_hint().max(1.0);
    let bounds = frame.bounds;
    if line_h <= 0.0 || bounds.height <= 0.0 || bounds.width <= 0.0 {
        return vec![frame.clone()];
    }
    let mut out: Vec<FrameRect> = Vec::new();

    let active_obstacles: Vec<&WrapObstacle> = obstacles
        .iter()
        .filter(|o| !matches!(o.wrap_mode, WrapMode::None))
        .collect();

    if active_obstacles.is_empty() {
        return vec![frame.clone()];
    }

    let row_count = ((bounds.height / line_h).ceil() as i64).max(0);
    for row in 0..row_count {
        let y0 = bounds.y + (row as f64) * line_h;
        let y1 = (y0 + line_h).min(bounds.y + bounds.height);
        // Find every obstacle that overlaps this row vertically.
        let mut row_obstacles: Vec<(f64, f64, WrapMode)> = Vec::new();
        for obs in &active_obstacles {
            let oy0 = obs.bounds.y - obs.margin;
            let oy1 = obs.bounds.y + obs.bounds.height + obs.margin;
            if oy1 <= y0 || oy0 >= y1 {
                continue;
            }
            let ox0 = obs.bounds.x - obs.margin;
            let ox1 = obs.bounds.x + obs.bounds.width + obs.margin;
            row_obstacles.push((ox0, ox1, obs.wrap_mode));
        }
        if row_obstacles.is_empty() {
            push_band(&mut out, frame, bounds.x, bounds.x + bounds.width, y0, y1);
            continue;
        }
        // Sort by left edge so we can walk left-to-right.
        row_obstacles.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let mut cursor = bounds.x;
        let right_edge = bounds.x + bounds.width;
        for (ox0, ox1, mode) in row_obstacles {
            // Clamp obstacle into the frame.
            let ox0 = ox0.max(bounds.x).min(right_edge);
            let ox1 = ox1.max(bounds.x).min(right_edge);
            if ox1 <= cursor {
                continue;
            }
            match mode {
                WrapMode::Both => {
                    if ox0 > cursor {
                        push_band(&mut out, frame, cursor, ox0, y0, y1);
                    }
                    cursor = ox1;
                }
                WrapMode::Left => {
                    if ox0 > cursor {
                        push_band(&mut out, frame, cursor, ox0, y0, y1);
                    }
                    // Right side of the obstacle is forbidden — jump
                    // cursor past the right edge of the frame so no
                    // further band is emitted on this row.
                    cursor = right_edge;
                }
                WrapMode::Right => {
                    // Left side is forbidden — skip directly to the
                    // right side of the obstacle.
                    cursor = cursor.max(ox1);
                }
                WrapMode::None => {}
            }
        }
        if cursor < right_edge {
            push_band(&mut out, frame, cursor, right_edge, y0, y1);
        }
    }
    if out.is_empty() {
        // Pathological case — every row consumed by obstacles. Return
        // an empty list so the flow engine knows no text fits.
        return Vec::new();
    }
    out
}

/// Helper trait to expose font-size / line-height defaults that the
/// `wrap` module needs. We pull these from `TextStyle` in the bridge,
/// but for unit-test convenience we expose hints on
/// `TextFrameOptions` directly that fall back to sane defaults.
trait TextFrameOptionsHints {
    fn font_size_hint(&self) -> f64;
    fn line_height_hint(&self) -> f64;
}

impl TextFrameOptionsHints for TextFrameOptions {
    fn font_size_hint(&self) -> f64 {
        // The frame itself doesn't carry a font size — the bridge
        // layer composes wrap with the active TextStyle. For the
        // standalone wrap test surface, we use 16 pt (the default
        // TextStyle font size) so the output is deterministic.
        16.0
    }
    fn line_height_hint(&self) -> f64 {
        1.25
    }
}

fn push_band(out: &mut Vec<FrameRect>, frame: &FrameRect, x0: f64, x1: f64, y0: f64, y1: f64) {
    let width = (x1 - x0).max(0.0);
    let height = (y1 - y0).max(0.0);
    if width <= 0.0 || height <= 0.0 {
        return;
    }
    let mut options = frame.options.clone();
    options.inset = FrameInsets::default();
    options.columns = 1;
    options.column_gap = 0.0;
    out.push(FrameRect {
        bounds: Bounds {
            x: x0,
            y: y0,
            width,
            height,
        },
        options,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flow::FrameRect;
    use kcreate_core::node::TextFrameOptions;
    use kcreate_core::Bounds;

    fn full_frame() -> FrameRect {
        FrameRect {
            bounds: Bounds {
                x: 0.0,
                y: 0.0,
                width: 400.0,
                height: 100.0,
            },
            options: TextFrameOptions::default(),
        }
    }

    #[test]
    fn no_obstacles_returns_original_frame() {
        let frame = full_frame();
        let out = carve_frames(&frame, &[]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].bounds, frame.bounds);
    }

    #[test]
    fn wrap_none_obstacle_is_ignored() {
        let frame = full_frame();
        let obs = WrapObstacle {
            bounds: Bounds {
                x: 100.0,
                y: 0.0,
                width: 50.0,
                height: 100.0,
            },
            margin: 0.0,
            wrap_mode: WrapMode::None,
        };
        let out = carve_frames(&frame, &[obs]);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn wrap_both_splits_each_affected_row() {
        let frame = full_frame();
        let obs = WrapObstacle {
            bounds: Bounds {
                x: 100.0,
                y: 0.0,
                width: 50.0,
                height: 100.0,
            },
            margin: 0.0,
            wrap_mode: WrapMode::Both,
        };
        let out = carve_frames(&frame, &[obs]);
        // Two bands per row: [0, 100] and [150, 400]; with 100/20 = 5
        // rows we expect 10 bands.
        assert_eq!(out.len(), 10);
    }
}
