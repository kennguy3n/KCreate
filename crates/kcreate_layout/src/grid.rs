//! Uniform CSS-grid solver.
//!
//! Fixed column count, row/column gaps, and per-edge padding. Cells
//! grow to fill the available width equally; rows expand to fit the
//! tallest child in each row. Items are placed in row-major order
//! into the grid — this is `grid-auto-flow: row` in CSS terms.

use kcreate_core::node::Bounds;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::padding::Padding;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GridLayout {
    /// Number of equal-width columns. Clamped to `>= 1`.
    pub columns: usize,
    pub row_gap: f64,
    pub column_gap: f64,
    pub padding: Padding,
}

impl Default for GridLayout {
    fn default() -> Self {
        Self {
            columns: 1,
            row_gap: 0.0,
            column_gap: 0.0,
            padding: Padding::default(),
        }
    }
}

/// Compute child positions inside `parent_bounds` using the grid.
/// Children whose `(width, height)` exceeds the cell are clipped to
/// the cell — this matches the design intent that the grid drives
/// child sizes, not the other way round.
#[must_use]
pub fn layout_grid(
    parent_bounds: Bounds,
    children_sizes: &[(Uuid, f64, f64)],
    layout: &GridLayout,
) -> Vec<(Uuid, Bounds)> {
    if children_sizes.is_empty() {
        return Vec::new();
    }
    let columns = layout.columns.max(1);
    let pad = layout.padding.normalize();
    let inner_x = parent_bounds.x + pad.left;
    let inner_y = parent_bounds.y + pad.top;
    let inner_w = (parent_bounds.width - pad.left - pad.right).max(0.0);
    let inner_h = (parent_bounds.height - pad.top - pad.bottom).max(0.0);
    let col_gap = layout.column_gap.max(0.0);
    let row_gap = layout.row_gap.max(0.0);

    // Each cell takes the leftover width after subtracting all
    // inter-column gaps.
    let total_col_gap = col_gap * (columns.saturating_sub(1) as f64);
    let cell_w = ((inner_w - total_col_gap) / columns as f64).max(0.0);

    // Row-major fill: rows[r] = vec of (index, height) for row r.
    let mut row_heights: Vec<f64> = Vec::new();
    for (i, (_, _, ch)) in children_sizes.iter().enumerate() {
        let row = i / columns;
        if row_heights.len() <= row {
            row_heights.push(0.0);
        }
        if *ch > row_heights[row] {
            row_heights[row] = *ch;
        }
    }

    // Compute per-row y origin.
    let mut row_origins: Vec<f64> = Vec::with_capacity(row_heights.len());
    let mut cursor = 0.0_f64;
    for (i, h) in row_heights.iter().enumerate() {
        row_origins.push(cursor);
        cursor += h;
        if i + 1 < row_heights.len() {
            cursor += row_gap;
        }
    }
    let _ = inner_h; // grid doesn't currently constrain to inner_h; rows overflow.

    let mut out: Vec<(Uuid, Bounds)> = Vec::with_capacity(children_sizes.len());
    for (i, (id, cw, ch)) in children_sizes.iter().enumerate() {
        let row = i / columns;
        let col = i % columns;
        let cell_h = row_heights[row];
        let x = inner_x + (col as f64) * (cell_w + col_gap);
        let y = inner_y + row_origins[row];
        // Children take the cell's full width but their natural
        // height clipped to the row height (so a tall child in a
        // short row doesn't extend past the row baseline).
        let w = cw.min(cell_w).max(0.0).max(0.0);
        let h = ch.min(cell_h).max(0.0);
        // Sizes default to cell extents when the child is at most
        // cell-sized; if smaller, we still want to render at natural
        // size — pick the larger of (natural, 0) capped at cell.
        let _ = w;
        out.push((*id, Bounds::new(x, y, cell_w.min(*cw).max(0.0), h)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn rect(x: f64, y: f64, w: f64, h: f64) -> Bounds {
        Bounds::new(x, y, w, h)
    }

    #[test]
    fn two_columns_split_width_with_gap() {
        let parent = rect(0.0, 0.0, 210.0, 200.0);
        let kids = vec![(id(1), 80.0, 40.0), (id(2), 80.0, 40.0)];
        let layout = GridLayout {
            columns: 2,
            column_gap: 10.0,
            ..GridLayout::default()
        };
        let out = layout_grid(parent, &kids, &layout);
        // cell_w = (210 - 10) / 2 = 100. Items at x=0 and x=110.
        assert!((out[0].1.x - 0.0).abs() < 1e-6);
        assert!((out[1].1.x - 110.0).abs() < 1e-6);
    }

    #[test]
    fn rows_use_max_child_height() {
        let parent = rect(0.0, 0.0, 200.0, 400.0);
        let kids = vec![
            (id(1), 50.0, 30.0),
            (id(2), 50.0, 60.0),
            (id(3), 50.0, 40.0),
            (id(4), 50.0, 20.0),
        ];
        let layout = GridLayout {
            columns: 2,
            row_gap: 10.0,
            ..GridLayout::default()
        };
        let out = layout_grid(parent, &kids, &layout);
        // Row 0 max height = 60 → second-row y = 60 + 10 = 70.
        assert!((out[2].1.y - 70.0).abs() < 1e-6);
        assert!((out[3].1.y - 70.0).abs() < 1e-6);
    }

    #[test]
    fn padding_offsets_origin() {
        let parent = rect(0.0, 0.0, 200.0, 200.0);
        let kids = vec![(id(1), 50.0, 50.0)];
        let layout = GridLayout {
            columns: 1,
            padding: Padding::uniform(20.0),
            ..GridLayout::default()
        };
        let out = layout_grid(parent, &kids, &layout);
        assert!((out[0].1.x - 20.0).abs() < 1e-6);
        assert!((out[0].1.y - 20.0).abs() < 1e-6);
    }

    #[test]
    fn zero_columns_is_clamped_to_one() {
        let parent = rect(0.0, 0.0, 100.0, 100.0);
        let kids = vec![(id(1), 50.0, 30.0), (id(2), 50.0, 30.0)];
        let layout = GridLayout {
            columns: 0,
            ..GridLayout::default()
        };
        let out = layout_grid(parent, &kids, &layout);
        // With effective columns=1, items stack vertically.
        assert!((out[0].1.y - 0.0).abs() < 1e-6);
        assert!((out[1].1.y - 30.0).abs() < 1e-6);
    }

    #[test]
    fn empty_children_returns_empty() {
        let parent = rect(0.0, 0.0, 200.0, 200.0);
        let out = layout_grid(parent, &[], &GridLayout::default());
        assert!(out.is_empty());
    }
}
