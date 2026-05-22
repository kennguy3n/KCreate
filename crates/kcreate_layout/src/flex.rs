//! Flexbox-style row/column solver.
//!
//! Children are laid out along the **main axis** (row → x, column →
//! y) using `Alignment`. The remaining axis is the **cross axis**;
//! `CrossAlignment` controls how children align across it. Both the
//! solver and the configuration are JSON-serializable so the bridge
//! can round-trip them through node metadata.
//!
//! The solver is deterministic, side-effect-free, and does not
//! mutate any input — it returns a fresh `(Uuid, Bounds)` list in
//! the same order children were supplied.

use kcreate_core::node::Bounds;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::padding::Padding;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlexDirection {
    Row,
    Column,
}

/// Main-axis alignment.
///
/// Note on `SpaceBetween` / `SpaceEvenly`: unlike CSS
/// `justify-content`, these variants **respect the `FlexLayout::spacing`
/// gap as a floor** and only distribute the *remaining* free space.
/// In CSS, `justify-content: space-between` ignores any `gap` and
/// distributes all free space; our solver guarantees at least
/// `spacing` pixels between siblings, then evenly fills whatever
/// surplus is left. This is intentional — it lets a designer set a
/// "minimum gap" and have alignment expand from there rather than
/// collapse below it. Callers porting CSS values directly should set
/// `spacing = 0` to get pure CSS semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Alignment {
    #[default]
    Start,
    Center,
    End,
    SpaceBetween,
    SpaceEvenly,
}

/// Cross-axis alignment (per row in `Row` mode, per column in `Column` mode).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CrossAlignment {
    #[default]
    Start,
    Center,
    End,
    Stretch,
}

/// Flexbox-style configuration. Serialized into `LayoutFrame`
/// metadata under the `layout` key.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FlexLayout {
    pub direction: FlexDirection,
    /// Spacing between consecutive children on the main axis.
    pub spacing: f64,
    pub padding: Padding,
    pub alignment: Alignment,
    pub cross_alignment: CrossAlignment,
    /// When true, items that don't fit on the current main-axis run
    /// wrap onto the next. The cross-axis size grows accordingly;
    /// the parent container is *not* resized, items overflow.
    pub wrap: bool,
}

impl Default for FlexLayout {
    fn default() -> Self {
        Self {
            direction: FlexDirection::Row,
            spacing: 0.0,
            padding: Padding::default(),
            alignment: Alignment::Start,
            cross_alignment: CrossAlignment::Start,
            wrap: false,
        }
    }
}

/// Compute child positions given the parent's content rect, a list of
/// `(id, intrinsic_width, intrinsic_height)`, and a flex config.
///
/// Returns a `Vec` with one entry per input child, in the same order.
/// The bounds are absolute (in the same coordinate space as
/// `parent_bounds`).
#[must_use]
pub fn layout_flex(
    parent_bounds: Bounds,
    children_sizes: &[(Uuid, f64, f64)],
    layout: &FlexLayout,
) -> Vec<(Uuid, Bounds)> {
    if children_sizes.is_empty() {
        return Vec::new();
    }

    let pad = layout.padding.normalize();
    let inner_x = parent_bounds.x + pad.left;
    let inner_y = parent_bounds.y + pad.top;
    let inner_w = (parent_bounds.width - pad.left - pad.right).max(0.0);
    let inner_h = (parent_bounds.height - pad.top - pad.bottom).max(0.0);
    let spacing = layout.spacing.max(0.0);

    let main_axis_size = match layout.direction {
        FlexDirection::Row => inner_w,
        FlexDirection::Column => inner_h,
    };

    // Pack children into runs. Without `wrap` everything goes into a
    // single run regardless of overflow.
    let runs = pack_into_runs(
        children_sizes,
        layout.direction,
        spacing,
        main_axis_size,
        layout.wrap,
    );

    // Output is constructed in original child order so the caller's
    // indexing into `children_sizes` matches.
    let mut out: Vec<(Uuid, Bounds)> = vec![(Uuid::nil(), Bounds::ZERO); children_sizes.len()];

    let mut cross_cursor = 0.0_f64;
    for run in &runs {
        // Cross-axis extent for this run = max cross size of its
        // members. Stretch alignment grows children to this extent.
        let run_cross_extent = run
            .indices
            .iter()
            .map(|&i| match layout.direction {
                FlexDirection::Row => children_sizes[i].2,
                FlexDirection::Column => children_sizes[i].1,
            })
            .fold(0.0_f64, f64::max);

        // Compute (lead_offset, between_spacing) on the main axis
        // for this run from the alignment + free space.
        let (lead, between) = main_lead_and_gap(
            run.indices.len(),
            run.main_used,
            main_axis_size,
            spacing,
            layout.alignment,
        );

        // Walk the run accumulating a cursor.
        let mut cursor = lead;
        for (slot_idx, &i) in run.indices.iter().enumerate() {
            let (id, cw, ch) = children_sizes[i];
            let (child_main, child_cross) = match layout.direction {
                FlexDirection::Row => (cw, ch),
                FlexDirection::Column => (ch, cw),
            };

            let cross_pos = cross_offset(child_cross, run_cross_extent, layout.cross_alignment);
            let final_cross_size = match layout.cross_alignment {
                CrossAlignment::Stretch => run_cross_extent,
                _ => child_cross,
            };

            let bounds = match layout.direction {
                FlexDirection::Row => Bounds::new(
                    inner_x + cursor,
                    inner_y + cross_cursor + cross_pos,
                    child_main,
                    final_cross_size,
                ),
                FlexDirection::Column => Bounds::new(
                    inner_x + cross_cursor + cross_pos,
                    inner_y + cursor,
                    final_cross_size,
                    child_main,
                ),
            };
            out[i] = (id, bounds);

            cursor += child_main;
            if slot_idx + 1 < run.indices.len() {
                cursor += between;
            }
        }
        cross_cursor += run_cross_extent + spacing;
    }

    out
}

/// A single packed row (or column, depending on direction). Each run
/// stacks against the cross axis.
struct Run {
    indices: Vec<usize>,
    /// Sum of main-axis sizes plus spacing between siblings in this
    /// run (excludes trailing spacing).
    main_used: f64,
}

fn pack_into_runs(
    children: &[(Uuid, f64, f64)],
    direction: FlexDirection,
    spacing: f64,
    main_axis_size: f64,
    wrap: bool,
) -> Vec<Run> {
    let mut runs: Vec<Run> = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    let mut used: f64 = 0.0;

    for (i, (_, w, h)) in children.iter().enumerate() {
        let main = match direction {
            FlexDirection::Row => *w,
            FlexDirection::Column => *h,
        };
        if wrap && !current.is_empty() {
            let tentative = used + spacing + main;
            if tentative > main_axis_size {
                runs.push(Run {
                    indices: std::mem::take(&mut current),
                    main_used: used,
                });
                used = 0.0;
            }
        }
        if !current.is_empty() {
            used += spacing;
        }
        used += main;
        current.push(i);
    }
    if !current.is_empty() {
        runs.push(Run {
            indices: current,
            main_used: used,
        });
    }
    runs
}

/// Compute the leading offset and inter-item gap for a run, given
/// the alignment policy and how much main-axis space is used vs
/// available.
fn main_lead_and_gap(
    count: usize,
    used: f64,
    available: f64,
    spacing: f64,
    alignment: Alignment,
) -> (f64, f64) {
    if count == 0 {
        return (0.0, 0.0);
    }
    let free = (available - used).max(0.0);
    match alignment {
        Alignment::Start => (0.0, spacing),
        Alignment::Center => (free / 2.0, spacing),
        Alignment::End => (free, spacing),
        Alignment::SpaceBetween => {
            if count <= 1 {
                (free / 2.0, spacing)
            } else {
                let extra = free / ((count - 1) as f64);
                (0.0, spacing + extra)
            }
        }
        Alignment::SpaceEvenly => {
            let gap = free / ((count + 1) as f64);
            (gap, spacing + gap)
        }
    }
}

fn cross_offset(child_cross: f64, run_extent: f64, alignment: CrossAlignment) -> f64 {
    match alignment {
        CrossAlignment::Start | CrossAlignment::Stretch => 0.0,
        CrossAlignment::Center => (run_extent - child_cross).max(0.0) / 2.0,
        CrossAlignment::End => (run_extent - child_cross).max(0.0),
    }
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
    fn row_start_packs_children_left_to_right() {
        let parent = rect(0.0, 0.0, 400.0, 100.0);
        let kids = vec![
            (id(1), 80.0, 40.0),
            (id(2), 80.0, 40.0),
            (id(3), 80.0, 40.0),
        ];
        let layout = FlexLayout {
            direction: FlexDirection::Row,
            spacing: 10.0,
            ..FlexLayout::default()
        };
        let out = layout_flex(parent, &kids, &layout);
        assert_eq!(out.len(), 3);
        // Starts: 0, 90, 180 (since spacing is 10 and width is 80).
        assert!((out[0].1.x - 0.0).abs() < 1e-6);
        assert!((out[1].1.x - 90.0).abs() < 1e-6);
        assert!((out[2].1.x - 180.0).abs() < 1e-6);
        assert!((out[0].1.y - 0.0).abs() < 1e-6);
    }

    #[test]
    fn column_direction_stacks_vertically() {
        let parent = rect(0.0, 0.0, 100.0, 400.0);
        let kids = vec![(id(1), 80.0, 40.0), (id(2), 80.0, 60.0)];
        let layout = FlexLayout {
            direction: FlexDirection::Column,
            spacing: 5.0,
            ..FlexLayout::default()
        };
        let out = layout_flex(parent, &kids, &layout);
        assert!((out[0].1.y - 0.0).abs() < 1e-6);
        // y of second = 40 + 5
        assert!((out[1].1.y - 45.0).abs() < 1e-6);
    }

    #[test]
    fn padding_inset_works() {
        let parent = rect(0.0, 0.0, 200.0, 200.0);
        let kids = vec![(id(1), 50.0, 50.0)];
        let layout = FlexLayout {
            padding: Padding::uniform(20.0),
            ..FlexLayout::default()
        };
        let out = layout_flex(parent, &kids, &layout);
        assert!((out[0].1.x - 20.0).abs() < 1e-6);
        assert!((out[0].1.y - 20.0).abs() < 1e-6);
    }

    #[test]
    fn center_alignment_centers_run_on_main_axis() {
        let parent = rect(0.0, 0.0, 300.0, 100.0);
        let kids = vec![(id(1), 100.0, 40.0)];
        let layout = FlexLayout {
            alignment: Alignment::Center,
            ..FlexLayout::default()
        };
        let out = layout_flex(parent, &kids, &layout);
        // Free = 300 - 100 = 200, half = 100.
        assert!((out[0].1.x - 100.0).abs() < 1e-6);
    }

    #[test]
    fn space_between_distributes_free_space() {
        let parent = rect(0.0, 0.0, 300.0, 100.0);
        let kids = vec![
            (id(1), 50.0, 40.0),
            (id(2), 50.0, 40.0),
            (id(3), 50.0, 40.0),
        ];
        let layout = FlexLayout {
            spacing: 0.0,
            alignment: Alignment::SpaceBetween,
            ..FlexLayout::default()
        };
        let out = layout_flex(parent, &kids, &layout);
        // Three 50-wide items in 300px: used=150, free=150, gaps=2 →
        // each gap is 75. Items at 0, 50+75=125, 125+50+75=250
        // (right edge of the last item lands on 300, the container
        // right edge).
        assert!((out[0].1.x - 0.0).abs() < 1e-6);
        assert!((out[1].1.x - 125.0).abs() < 1e-6);
        assert!((out[2].1.x - 250.0).abs() < 1e-6);
    }

    #[test]
    fn space_evenly_distributes_n_plus_one_gaps() {
        let parent = rect(0.0, 0.0, 300.0, 100.0);
        let kids = vec![(id(1), 50.0, 40.0), (id(2), 50.0, 40.0)];
        let layout = FlexLayout {
            spacing: 0.0,
            alignment: Alignment::SpaceEvenly,
            ..FlexLayout::default()
        };
        let out = layout_flex(parent, &kids, &layout);
        // used=100, free=200, 3 gaps -> each 200/3.
        let g = 200.0 / 3.0;
        assert!((out[0].1.x - g).abs() < 1e-6);
        assert!((out[1].1.x - (g + 50.0 + g)).abs() < 1e-6);
    }

    #[test]
    fn end_alignment_pushes_items_to_far_edge() {
        let parent = rect(0.0, 0.0, 300.0, 100.0);
        let kids = vec![(id(1), 100.0, 40.0)];
        let layout = FlexLayout {
            alignment: Alignment::End,
            ..FlexLayout::default()
        };
        let out = layout_flex(parent, &kids, &layout);
        // Free = 200, lead = 200.
        assert!((out[0].1.x - 200.0).abs() < 1e-6);
    }

    #[test]
    fn wrap_breaks_when_main_axis_overflows() {
        let parent = rect(0.0, 0.0, 100.0, 200.0);
        let kids = vec![
            (id(1), 60.0, 30.0),
            (id(2), 60.0, 30.0),
            (id(3), 60.0, 30.0),
        ];
        let layout = FlexLayout {
            spacing: 0.0,
            wrap: true,
            ..FlexLayout::default()
        };
        let out = layout_flex(parent, &kids, &layout);
        // Each row only fits 1 item (60+60 = 120 > 100).
        // So we expect 3 runs of 1 item each, each stacked vertically.
        let ys: Vec<f64> = out.iter().map(|(_, b)| b.y).collect();
        assert_eq!(ys.len(), 3);
        assert!(ys[0] < ys[1]);
        assert!(ys[1] < ys[2]);
    }

    #[test]
    fn cross_alignment_center_centers_per_run() {
        let parent = rect(0.0, 0.0, 400.0, 100.0);
        let kids = vec![(id(1), 50.0, 20.0), (id(2), 50.0, 60.0)];
        let layout = FlexLayout {
            cross_alignment: CrossAlignment::Center,
            ..FlexLayout::default()
        };
        let out = layout_flex(parent, &kids, &layout);
        // run cross extent = 60 (tallest). First child (20 tall) centered → y=20.
        assert!((out[0].1.y - 20.0).abs() < 1e-6);
        assert!((out[1].1.y - 0.0).abs() < 1e-6);
    }

    #[test]
    fn cross_alignment_stretch_grows_child_to_run_extent() {
        let parent = rect(0.0, 0.0, 400.0, 100.0);
        let kids = vec![(id(1), 50.0, 20.0), (id(2), 50.0, 60.0)];
        let layout = FlexLayout {
            cross_alignment: CrossAlignment::Stretch,
            ..FlexLayout::default()
        };
        let out = layout_flex(parent, &kids, &layout);
        assert!((out[0].1.height - 60.0).abs() < 1e-6);
        assert!((out[1].1.height - 60.0).abs() < 1e-6);
    }

    #[test]
    fn empty_children_returns_empty() {
        let parent = rect(0.0, 0.0, 200.0, 200.0);
        let out = layout_flex(parent, &[], &FlexLayout::default());
        assert!(out.is_empty());
    }

    #[test]
    fn negative_padding_is_clamped() {
        let parent = rect(0.0, 0.0, 200.0, 200.0);
        let kids = vec![(id(1), 50.0, 50.0)];
        let layout = FlexLayout {
            padding: Padding::new(-10.0, -10.0, -10.0, -10.0),
            ..FlexLayout::default()
        };
        let out = layout_flex(parent, &kids, &layout);
        assert!((out[0].1.x - 0.0).abs() < 1e-6);
    }
}
