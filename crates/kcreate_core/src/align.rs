//! Alignment & distribution math for the Design Studio (Phase 9 Task 23).
//!
//! These helpers operate on lists of [`Bounds`] — they have no
//! knowledge of the document graph or the operation log. The
//! bridge layer reads each node's bounds, hands them off to
//! `align_bounds` / `distribute_bounds`, and writes the deltas
//! back as a single operation.
//!
//! Conventions:
//! - X grows right, Y grows down (screen-space, matches the rest
//!   of `kcreate_core`).
//! - The "anchor" of an alignment operation is the union bbox of
//!   all input bounds. Aligning a single node is a no-op.
//! - Distribution requires at least three inputs (two endpoints +
//!   one mover). With exactly two, distribution returns the
//!   identity delta because there's nothing to distribute.

use serde::{Deserialize, Serialize};

use crate::node::Bounds;

/// All seven canonical alignment operations Figma / Sketch /
/// Affinity surface. `MiddleHorizontal` aligns to the union's
/// horizontal centre (same x for everyone). `MiddleVertical`
/// aligns to the union's vertical centre.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Align {
    Left,
    CenterHorizontal,
    Right,
    Top,
    CenterVertical,
    Bottom,
}

/// Distribute along an axis. `Horizontal` spaces objects evenly
/// along the X axis (gap-equal between successive bbox edges);
/// `Vertical` does the same along Y.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DistributeAxis {
    Horizontal,
    Vertical,
}

/// The per-node delta produced by an alignment or distribute
/// operation. Add `dx` to `bounds.x` and `dy` to `bounds.y` —
/// width and height are unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AlignDelta {
    pub dx: f64,
    pub dy: f64,
}

impl AlignDelta {
    pub const ZERO: Self = Self { dx: 0.0, dy: 0.0 };
}

/// Returns the union bounding box of `inputs`, or `None` when
/// the slice is empty.
#[must_use]
pub fn union_bounds(inputs: &[Bounds]) -> Option<Bounds> {
    inputs.iter().copied().reduce(|acc, b| acc.union(&b))
}

/// Compute the per-node delta needed to align every bounding box
/// in `inputs` to the union bbox along `axis`. Inputs and output
/// are 1:1 — `out[i]` is the delta for `inputs[i]`.
///
/// A single-input alignment is a no-op (returns `[Zero]`).
pub fn align_bounds(inputs: &[Bounds], axis: Align) -> Vec<AlignDelta> {
    let Some(anchor) = union_bounds(inputs) else {
        return Vec::new();
    };
    if inputs.len() <= 1 {
        return vec![AlignDelta::ZERO; inputs.len()];
    }
    inputs
        .iter()
        .map(|b| match axis {
            Align::Left => AlignDelta {
                dx: anchor.x - b.x,
                dy: 0.0,
            },
            Align::Right => AlignDelta {
                dx: anchor.right() - b.right(),
                dy: 0.0,
            },
            Align::CenterHorizontal => {
                let anchor_cx = anchor.x + anchor.width / 2.0;
                let bcx = b.x + b.width / 2.0;
                AlignDelta {
                    dx: anchor_cx - bcx,
                    dy: 0.0,
                }
            }
            Align::Top => AlignDelta {
                dx: 0.0,
                dy: anchor.y - b.y,
            },
            Align::Bottom => AlignDelta {
                dx: 0.0,
                dy: anchor.bottom() - b.bottom(),
            },
            Align::CenterVertical => {
                let anchor_cy = anchor.y + anchor.height / 2.0;
                let bcy = b.y + b.height / 2.0;
                AlignDelta {
                    dx: 0.0,
                    dy: anchor_cy - bcy,
                }
            }
        })
        .collect()
}

/// Distribute `inputs` evenly along `axis`. The first and last
/// objects (in the sort order produced by `axis`) stay put; the
/// inner objects are repositioned so every adjacent-pair gap is
/// equal. With fewer than 3 inputs there's nothing to distribute
/// — the function returns all-zero deltas matching the input
/// length so the caller can still treat the result as 1:1 with
/// inputs.
pub fn distribute_bounds(inputs: &[Bounds], axis: DistributeAxis) -> Vec<AlignDelta> {
    if inputs.len() < 3 {
        return vec![AlignDelta::ZERO; inputs.len()];
    }
    // Sort by leading edge along the chosen axis without losing
    // the original indices — we want to return deltas indexed
    // against the original `inputs` order.
    let mut indices: Vec<usize> = (0..inputs.len()).collect();
    match axis {
        DistributeAxis::Horizontal => indices.sort_by(|&a, &b| {
            inputs[a]
                .x
                .partial_cmp(&inputs[b].x)
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
        DistributeAxis::Vertical => indices.sort_by(|&a, &b| {
            inputs[a]
                .y
                .partial_cmp(&inputs[b].y)
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
    }
    let first_idx = *indices.first().expect("len>=3");
    let last_idx = *indices.last().expect("len>=3");
    let total_size: f64 = indices
        .iter()
        .map(|&i| match axis {
            DistributeAxis::Horizontal => inputs[i].width,
            DistributeAxis::Vertical => inputs[i].height,
        })
        .sum();
    let (start, end) = match axis {
        DistributeAxis::Horizontal => (inputs[first_idx].x, inputs[last_idx].right()),
        DistributeAxis::Vertical => (inputs[first_idx].y, inputs[last_idx].bottom()),
    };
    let extent = end - start;
    // gap = (extent - total_size) / (n-1). When extent < total_size,
    // gap goes negative meaning the boxes overlap — that's the
    // user's request, distribute still produces a deterministic
    // even spacing (negative gap = overlap by |gap|).
    let n = indices.len();
    let gap = (extent - total_size) / (n as f64 - 1.0);
    let mut deltas = vec![AlignDelta::ZERO; inputs.len()];
    let mut cursor = start;
    for &i in &indices {
        let b = inputs[i];
        match axis {
            DistributeAxis::Horizontal => {
                let new_x = cursor;
                deltas[i] = AlignDelta {
                    dx: new_x - b.x,
                    dy: 0.0,
                };
                cursor = new_x + b.width + gap;
            }
            DistributeAxis::Vertical => {
                let new_y = cursor;
                deltas[i] = AlignDelta {
                    dx: 0.0,
                    dy: new_y - b.y,
                };
                cursor = new_y + b.height + gap;
            }
        }
    }
    deltas
}

/// Distribute spacing only — keep every object's centre but
/// ensure the inter-edge gap is uniform. Equivalent to
/// `distribute_bounds` but exposes the gap value so the caller
/// can render a "even spacing: N px" badge.
#[must_use]
pub fn distribute_gap(inputs: &[Bounds], axis: DistributeAxis) -> Option<f64> {
    if inputs.len() < 3 {
        return None;
    }
    let mut indices: Vec<usize> = (0..inputs.len()).collect();
    match axis {
        DistributeAxis::Horizontal => indices.sort_by(|&a, &b| {
            inputs[a]
                .x
                .partial_cmp(&inputs[b].x)
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
        DistributeAxis::Vertical => indices.sort_by(|&a, &b| {
            inputs[a]
                .y
                .partial_cmp(&inputs[b].y)
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
    }
    let first_idx = *indices.first()?;
    let last_idx = *indices.last()?;
    let total_size: f64 = indices
        .iter()
        .map(|&i| match axis {
            DistributeAxis::Horizontal => inputs[i].width,
            DistributeAxis::Vertical => inputs[i].height,
        })
        .sum();
    let (start, end) = match axis {
        DistributeAxis::Horizontal => (inputs[first_idx].x, inputs[last_idx].right()),
        DistributeAxis::Vertical => (inputs[first_idx].y, inputs[last_idx].bottom()),
    };
    Some((end - start - total_size) / (indices.len() as f64 - 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(x: f64, y: f64, w: f64, h: f64) -> Bounds {
        Bounds::new(x, y, w, h)
    }

    #[test]
    fn align_left_pins_to_min_x() {
        let inputs = [
            b(10.0, 0.0, 5.0, 5.0),
            b(50.0, 0.0, 5.0, 5.0),
            b(100.0, 0.0, 5.0, 5.0),
        ];
        let d = align_bounds(&inputs, Align::Left);
        assert_eq!(d[0].dx, 0.0);
        assert_eq!(d[1].dx, -40.0);
        assert_eq!(d[2].dx, -90.0);
        for di in &d {
            assert_eq!(di.dy, 0.0);
        }
    }

    #[test]
    fn align_right_pins_to_max_right() {
        let inputs = [
            b(10.0, 0.0, 5.0, 5.0),
            b(50.0, 0.0, 5.0, 5.0),
            b(100.0, 0.0, 5.0, 5.0),
        ];
        let d = align_bounds(&inputs, Align::Right);
        // anchor.right = 105
        assert_eq!(d[0].dx, 90.0);
        assert_eq!(d[1].dx, 50.0);
        assert_eq!(d[2].dx, 0.0);
    }

    #[test]
    fn align_center_horizontal_aligns_centers() {
        let inputs = [b(0.0, 0.0, 10.0, 10.0), b(20.0, 0.0, 20.0, 10.0)];
        let d = align_bounds(&inputs, Align::CenterHorizontal);
        // anchor centre x = (0 + 40) / 2 = 20
        // box0 centre x = 5  -> dx = 15
        // box1 centre x = 30 -> dx = -10
        assert_eq!(d[0].dx, 15.0);
        assert_eq!(d[1].dx, -10.0);
    }

    #[test]
    fn align_single_input_is_noop() {
        let inputs = [b(50.0, 50.0, 10.0, 10.0)];
        let d = align_bounds(&inputs, Align::Left);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0], AlignDelta::ZERO);
    }

    #[test]
    fn distribute_horizontal_three_items_equal_gap() {
        // Three 10x10 boxes at x=0, 50, 100. Total extent = 110.
        // Total size = 30. Gap = (110 - 30) / 2 = 40.
        let inputs = [
            b(0.0, 0.0, 10.0, 10.0),
            b(50.0, 0.0, 10.0, 10.0),
            b(100.0, 0.0, 10.0, 10.0),
        ];
        let d = distribute_bounds(&inputs, DistributeAxis::Horizontal);
        // First and last stay put.
        assert_eq!(d[0].dx, 0.0);
        assert_eq!(d[2].dx, 0.0);
        // Middle should end up at x = 0 + 10 + 40 = 50 — already
        // there, so delta = 0.
        assert!((d[1].dx - 0.0).abs() < 1e-9);
    }

    #[test]
    fn distribute_horizontal_uneven_widths() {
        // Widths 10, 20, 10. x = 0, 30, 100.
        // Extent = 110, total_size = 40. Gap = 70 / 2 = 35.
        // Cursor: 0 (place box 0 at 0) -> 10 + 35 = 45 (place box 1)
        // -> 45 + 20 + 35 = 100 (place box 2). Box 1 moves from 30 to 45 (dx = 15).
        let inputs = [
            b(0.0, 0.0, 10.0, 10.0),
            b(30.0, 0.0, 20.0, 10.0),
            b(100.0, 0.0, 10.0, 10.0),
        ];
        let d = distribute_bounds(&inputs, DistributeAxis::Horizontal);
        assert_eq!(d[0].dx, 0.0);
        assert!((d[1].dx - 15.0).abs() < 1e-9);
        assert_eq!(d[2].dx, 0.0);
    }

    #[test]
    fn distribute_less_than_three_is_noop() {
        let inputs = [b(0.0, 0.0, 10.0, 10.0), b(100.0, 0.0, 10.0, 10.0)];
        let d = distribute_bounds(&inputs, DistributeAxis::Horizontal);
        assert_eq!(d.len(), 2);
        for di in &d {
            assert_eq!(*di, AlignDelta::ZERO);
        }
    }

    #[test]
    fn distribute_gap_returns_none_for_two_items() {
        let inputs = [b(0.0, 0.0, 10.0, 10.0), b(100.0, 0.0, 10.0, 10.0)];
        assert!(distribute_gap(&inputs, DistributeAxis::Horizontal).is_none());
    }

    #[test]
    fn distribute_gap_value() {
        let inputs = [
            b(0.0, 0.0, 10.0, 10.0),
            b(50.0, 0.0, 10.0, 10.0),
            b(100.0, 0.0, 10.0, 10.0),
        ];
        let gap = distribute_gap(&inputs, DistributeAxis::Horizontal).unwrap();
        assert!((gap - 40.0).abs() < 1e-9);
    }

    #[test]
    fn align_vertical_works() {
        let inputs = [b(0.0, 10.0, 10.0, 10.0), b(0.0, 50.0, 10.0, 30.0)];
        let d = align_bounds(&inputs, Align::Top);
        // anchor.y = min(10, 50) = 10
        assert_eq!(d[0].dy, 0.0);
        assert_eq!(d[1].dy, -40.0);
    }
}
