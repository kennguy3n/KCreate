//! Frame-resize constraint solver.
//!
//! Given a parent frame's old bounds, a parent frame's new bounds,
//! a child's old bounds, and the child's [`Constraints`], compute
//! the child's new bounds. The result mirrors Figma's
//! constraint model so designers see exactly the same behaviour
//! when they resize a frame.
//!
//! The solver runs once per child on each parent resize; the
//! bridge layer walks children top-down so nested frames cascade
//! naturally.

use kcreate_core::node::{Bounds, Constraint, Constraints};

/// Compute the child's new bounds after the parent resizes from
/// `parent_old` to `parent_new`. The child's old bounds are
/// expressed in the parent's coordinate space (i.e. relative to
/// `parent_old.x` / `parent_old.y`) — this is the same convention
/// every other layout pass in `kcreate_layout` uses.
#[must_use]
pub fn apply_constraints(
    child_old: Bounds,
    constraints: Constraints,
    parent_old: Bounds,
    parent_new: Bounds,
) -> Bounds {
    let (x, width) = solve_axis(
        child_old.x - parent_old.x,
        child_old.width,
        parent_old.width,
        parent_new.width,
        constraints.horizontal,
    );
    let (y, height) = solve_axis(
        child_old.y - parent_old.y,
        child_old.height,
        parent_old.height,
        parent_new.height,
        constraints.vertical,
    );
    Bounds {
        x: parent_new.x + x,
        y: parent_new.y + y,
        width,
        height,
    }
}

/// Walk a list of `(child_id, child_bounds, child_constraints)`
/// tuples and produce the resized bounds for each. The caller
/// owns the document graph and is responsible for writing the
/// new bounds back; this function is pure.
#[must_use]
pub fn apply_constraints_batch(
    children: &[(uuid::Uuid, Bounds, Constraints)],
    parent_old: Bounds,
    parent_new: Bounds,
) -> Vec<(uuid::Uuid, Bounds)> {
    children
        .iter()
        .map(|(id, b, c)| (*id, apply_constraints(*b, *c, parent_old, parent_new)))
        .collect()
}

fn solve_axis(
    child_origin: f64,
    child_extent: f64,
    parent_old_extent: f64,
    parent_new_extent: f64,
    constraint: Constraint,
) -> (f64, f64) {
    // Distance from the child's far edge to the parent's far edge,
    // measured along the same axis. Used by Max / Stretch to
    // preserve the right/bottom inset.
    let trailing_gap = parent_old_extent - (child_origin + child_extent);
    let (origin, extent) = match constraint {
        Constraint::Fixed | Constraint::Min => {
            // Min == "pin to leading edge". Both behave the same in
            // a single resize: the child stays at the same origin
            // and keeps its own size. Future evolutions (e.g. min
            // expressed as a percentage) can specialise this.
            (child_origin, child_extent)
        }
        Constraint::Max => {
            // Pin to trailing edge: keep the right/bottom inset
            // constant and let the leading edge slide.
            (
                parent_new_extent - trailing_gap - child_extent,
                child_extent,
            )
        }
        Constraint::Center => {
            // Maintain the center offset from the parent's center.
            let old_center = parent_old_extent * 0.5;
            let new_center = parent_new_extent * 0.5;
            let child_center = child_origin + child_extent * 0.5;
            let new_child_center = new_center + (child_center - old_center);
            (new_child_center - child_extent * 0.5, child_extent)
        }
        Constraint::Scale => {
            // Scale both position and size proportionally to the
            // parent's resize ratio. Falls back to "keep" when the
            // parent had zero extent (avoid divide-by-zero).
            if parent_old_extent <= 0.0 {
                return (child_origin, child_extent.max(0.0));
            }
            let ratio = parent_new_extent / parent_old_extent;
            (child_origin * ratio, child_extent * ratio)
        }
        Constraint::Stretch => {
            // Pin both leading + trailing edges to the parent. The
            // child's extent stretches to fill the new parent
            // minus the original insets. When the parent shrinks
            // far enough that `origin + trailing_gap >
            // parent_new_extent`, the naive arithmetic would produce
            // a negative extent — see clamp below for why we
            // collapse to zero instead.
            (
                child_origin,
                parent_new_extent - child_origin - trailing_gap,
            )
        }
    };
    // Clamp the extent at the solver boundary so downstream
    // consumers (renderer, hit-test, export) never see a
    // `Bounds::width` / `Bounds::height` below zero. A child that
    // would mathematically collapse past zero (typically a
    // Stretch child whose parent shrinks below the combined
    // leading + trailing insets) is reported as exactly zero,
    // matching Figma's behaviour. We keep the origin unchanged so
    // restoring the parent to its old extent reconstructs the
    // child cleanly.
    (origin, extent.max(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(x: f64, y: f64, w: f64, h: f64) -> Bounds {
        Bounds {
            x,
            y,
            width: w,
            height: h,
        }
    }

    #[test]
    fn fixed_keeps_child_in_place() {
        let parent_old = b(0.0, 0.0, 100.0, 100.0);
        let parent_new = b(0.0, 0.0, 200.0, 200.0);
        let child = b(10.0, 10.0, 20.0, 20.0);
        let out = apply_constraints(
            child,
            Constraints {
                horizontal: Constraint::Fixed,
                vertical: Constraint::Fixed,
            },
            parent_old,
            parent_new,
        );
        assert_eq!(out, child);
    }

    #[test]
    fn max_keeps_trailing_gap() {
        let parent_old = b(0.0, 0.0, 100.0, 100.0);
        let parent_new = b(0.0, 0.0, 200.0, 100.0);
        // Child sits at x=70, width=20 → trailing gap is 10.
        let child = b(70.0, 0.0, 20.0, 20.0);
        let out = apply_constraints(
            child,
            Constraints {
                horizontal: Constraint::Max,
                vertical: Constraint::Fixed,
            },
            parent_old,
            parent_new,
        );
        // After resize, trailing gap should still be 10 → x = 170.
        assert!((out.x - 170.0).abs() < 1e-9);
        assert!((out.width - 20.0).abs() < 1e-9);
    }

    #[test]
    fn center_preserves_center_alignment() {
        let parent_old = b(0.0, 0.0, 100.0, 100.0);
        let parent_new = b(0.0, 0.0, 300.0, 100.0);
        // Child centered: x=40, width=20.
        let child = b(40.0, 0.0, 20.0, 20.0);
        let out = apply_constraints(
            child,
            Constraints {
                horizontal: Constraint::Center,
                vertical: Constraint::Fixed,
            },
            parent_old,
            parent_new,
        );
        // New center should be at 150, child width 20 → x = 140.
        assert!((out.x - 140.0).abs() < 1e-9);
    }

    #[test]
    fn scale_resizes_proportionally() {
        let parent_old = b(0.0, 0.0, 100.0, 100.0);
        let parent_new = b(0.0, 0.0, 200.0, 50.0);
        let child = b(20.0, 20.0, 40.0, 40.0);
        let out = apply_constraints(
            child,
            Constraints {
                horizontal: Constraint::Scale,
                vertical: Constraint::Scale,
            },
            parent_old,
            parent_new,
        );
        // 2x horizontal, 0.5x vertical.
        assert!((out.x - 40.0).abs() < 1e-9);
        assert!((out.y - 10.0).abs() < 1e-9);
        assert!((out.width - 80.0).abs() < 1e-9);
        assert!((out.height - 20.0).abs() < 1e-9);
    }

    #[test]
    fn stretch_fills_to_parent_insets() {
        let parent_old = b(0.0, 0.0, 100.0, 100.0);
        let parent_new = b(0.0, 0.0, 300.0, 100.0);
        // Child has 10px left + 10px right inset.
        let child = b(10.0, 0.0, 80.0, 20.0);
        let out = apply_constraints(
            child,
            Constraints {
                horizontal: Constraint::Stretch,
                vertical: Constraint::Fixed,
            },
            parent_old,
            parent_new,
        );
        // After resize, insets remain 10/10 → width = 300-10-10 = 280.
        assert!((out.x - 10.0).abs() < 1e-9);
        assert!((out.width - 280.0).abs() < 1e-9);
    }

    #[test]
    fn no_op_when_parent_unchanged() {
        let parent = b(0.0, 0.0, 100.0, 100.0);
        let child = b(20.0, 20.0, 30.0, 30.0);
        let out = apply_constraints(
            child,
            Constraints {
                horizontal: Constraint::Scale,
                vertical: Constraint::Center,
            },
            parent,
            parent,
        );
        assert!((out.x - child.x).abs() < 1e-9);
        assert!((out.y - child.y).abs() < 1e-9);
        assert!((out.width - child.width).abs() < 1e-9);
        assert!((out.height - child.height).abs() < 1e-9);
    }

    #[test]
    fn scale_handles_zero_parent_gracefully() {
        let parent_old = b(0.0, 0.0, 0.0, 100.0);
        let parent_new = b(0.0, 0.0, 50.0, 100.0);
        let child = b(0.0, 10.0, 0.0, 20.0);
        let out = apply_constraints(
            child,
            Constraints {
                horizontal: Constraint::Scale,
                vertical: Constraint::Fixed,
            },
            parent_old,
            parent_new,
        );
        // No NaN / Inf.
        assert!(out.x.is_finite());
        assert!(out.width.is_finite());
    }

    #[test]
    fn stretch_clamps_to_zero_when_parent_shrinks_below_insets() {
        // Child has 30 leading + 50 trailing inset on a 100-wide parent
        // (so child width = 20). Shrink the parent to 60 — the combined
        // insets (80) exceed the new extent, so the child must collapse
        // to width 0 rather than report a negative width.
        let parent_old = b(0.0, 0.0, 100.0, 100.0);
        let parent_new = b(0.0, 0.0, 60.0, 100.0);
        let child = b(30.0, 0.0, 20.0, 20.0);
        let out = apply_constraints(
            child,
            Constraints {
                horizontal: Constraint::Stretch,
                vertical: Constraint::Fixed,
            },
            parent_old,
            parent_new,
        );
        // Origin tracks the leading inset; width clamps to zero.
        assert!((out.x - 30.0).abs() < 1e-9);
        assert!(
            out.width >= 0.0,
            "stretch must never produce negative width (got {})",
            out.width
        );
        assert!((out.width - 0.0).abs() < 1e-9);
        // Vertical axis untouched.
        assert!((out.height - 20.0).abs() < 1e-9);
    }

    #[test]
    fn scale_clamps_to_zero_when_starting_extent_is_negative() {
        // Defensive: a caller passing in a negative `child_extent`
        // would historically propagate through the Scale branch's
        // zero-parent fallback. The clamp ensures the public API
        // contract holds even on garbage input.
        let parent_old = b(0.0, 0.0, 0.0, 100.0);
        let parent_new = b(0.0, 0.0, 50.0, 100.0);
        let child = b(0.0, 10.0, -5.0, 20.0);
        let out = apply_constraints(
            child,
            Constraints {
                horizontal: Constraint::Scale,
                vertical: Constraint::Fixed,
            },
            parent_old,
            parent_new,
        );
        assert!(out.width >= 0.0);
    }

    #[test]
    fn batch_resizes_every_child() {
        let parent_old = b(0.0, 0.0, 100.0, 100.0);
        let parent_new = b(0.0, 0.0, 200.0, 100.0);
        let id1 = uuid::Uuid::new_v4();
        let id2 = uuid::Uuid::new_v4();
        let children = vec![
            (
                id1,
                b(10.0, 0.0, 20.0, 20.0),
                Constraints {
                    horizontal: Constraint::Fixed,
                    vertical: Constraint::Fixed,
                },
            ),
            (
                id2,
                b(70.0, 0.0, 20.0, 20.0),
                Constraints {
                    horizontal: Constraint::Max,
                    vertical: Constraint::Fixed,
                },
            ),
        ];
        let out = apply_constraints_batch(&children, parent_old, parent_new);
        assert_eq!(out.len(), 2);
        // Fixed child stays put.
        assert!((out[0].1.x - 10.0).abs() < 1e-9);
        // Max child moves with the right edge.
        assert!((out[1].1.x - 170.0).abs() < 1e-9);
    }
}
