//! Phase 8 Block D: constraint system.
//!
//! Tests the [`apply_constraints`] solver against all six
//! constraint modes and verifies batch propagation.

use kcreate_core::node::{Bounds, Constraint, Constraints};
use kcreate_layout::constraints::{apply_constraints, apply_constraints_batch};
use uuid::Uuid;

fn b(x: f64, y: f64, w: f64, h: f64) -> Bounds {
    Bounds {
        x,
        y,
        width: w,
        height: h,
    }
}

fn cset(h: Constraint, v: Constraint) -> Constraints {
    Constraints {
        horizontal: h,
        vertical: v,
    }
}

#[test]
fn fixed_pins_to_leading_edge() {
    let parent_old = b(0.0, 0.0, 100.0, 100.0);
    let parent_new = b(0.0, 0.0, 200.0, 200.0);
    let child = b(10.0, 10.0, 30.0, 30.0);
    let result = apply_constraints(
        child,
        cset(Constraint::Fixed, Constraint::Fixed),
        parent_old,
        parent_new,
    );
    assert!((result.x - 10.0).abs() < 1e-9);
    assert!((result.y - 10.0).abs() < 1e-9);
    assert!((result.width - 30.0).abs() < 1e-9);
    assert!((result.height - 30.0).abs() < 1e-9);
}

#[test]
fn scale_proportionally_resizes() {
    let parent_old = b(0.0, 0.0, 100.0, 100.0);
    let parent_new = b(0.0, 0.0, 200.0, 200.0);
    let child = b(10.0, 10.0, 30.0, 30.0);
    let result = apply_constraints(
        child,
        cset(Constraint::Scale, Constraint::Scale),
        parent_old,
        parent_new,
    );
    assert!((result.x - 20.0).abs() < 1e-9, "x scaled, got {}", result.x);
    assert!(
        (result.width - 60.0).abs() < 1e-9,
        "w scaled, got {}",
        result.width
    );
    assert!((result.y - 20.0).abs() < 1e-9);
    assert!((result.height - 60.0).abs() < 1e-9);
}

#[test]
fn stretch_fills_parent_minus_insets() {
    let parent_old = b(0.0, 0.0, 100.0, 100.0);
    let parent_new = b(0.0, 0.0, 300.0, 300.0);
    let child = b(10.0, 15.0, 80.0, 70.0);
    let result = apply_constraints(
        child,
        cset(Constraint::Stretch, Constraint::Stretch),
        parent_old,
        parent_new,
    );
    // Stretch preserves insets, so the new width = parent_new - leading_inset - trailing_inset.
    let leading_x = child.x - parent_old.x;
    let trailing_x = parent_old.x + parent_old.width - (child.x + child.width);
    let expected_w = parent_new.width - leading_x - trailing_x;
    assert!((result.width - expected_w).abs() < 1e-9);
    assert!((result.x - leading_x).abs() < 1e-9);
}

#[test]
fn center_maintains_center_offset() {
    let parent_old = b(0.0, 0.0, 100.0, 100.0);
    let parent_new = b(0.0, 0.0, 200.0, 200.0);
    let child = b(35.0, 35.0, 30.0, 30.0);
    let result = apply_constraints(
        child,
        cset(Constraint::Center, Constraint::Center),
        parent_old,
        parent_new,
    );
    // Center constraint maintains the center offset between
    // child-center and parent-center.
    let old_child_center = child.x + child.width / 2.0;
    let old_parent_center = parent_old.x + parent_old.width / 2.0;
    let offset = old_child_center - old_parent_center;
    let new_parent_center = parent_new.x + parent_new.width / 2.0;
    let expected_x = new_parent_center + offset - child.width / 2.0;
    assert!((result.x - expected_x).abs() < 1e-9);
}

#[test]
fn max_pins_to_trailing_edge() {
    let parent_old = b(0.0, 0.0, 100.0, 100.0);
    let parent_new = b(0.0, 0.0, 200.0, 200.0);
    let child = b(60.0, 60.0, 30.0, 30.0);
    let result = apply_constraints(
        child,
        cset(Constraint::Max, Constraint::Max),
        parent_old,
        parent_new,
    );
    let old_right_inset = parent_old.width - (child.x + child.width);
    let expected_x = parent_new.width - child.width - old_right_inset;
    assert!((result.x - expected_x).abs() < 1e-9);
}

#[test]
fn min_pins_to_leading_edge() {
    let parent_old = b(0.0, 0.0, 100.0, 100.0);
    let parent_new = b(0.0, 0.0, 200.0, 200.0);
    let child = b(10.0, 10.0, 30.0, 30.0);
    let result = apply_constraints(
        child,
        cset(Constraint::Min, Constraint::Min),
        parent_old,
        parent_new,
    );
    assert!((result.x - 10.0).abs() < 1e-9);
    assert!((result.width - 30.0).abs() < 1e-9);
}

#[test]
fn no_op_when_parent_unchanged() {
    let parent = b(0.0, 0.0, 100.0, 100.0);
    let child = b(10.0, 10.0, 30.0, 30.0);
    let result = apply_constraints(
        child,
        cset(Constraint::Scale, Constraint::Scale),
        parent,
        parent,
    );
    assert!((result.x - child.x).abs() < 1e-9);
    assert!((result.width - child.width).abs() < 1e-9);
}

#[test]
fn batch_applies_to_all_children() {
    let parent_old = b(0.0, 0.0, 100.0, 100.0);
    let parent_new = b(0.0, 0.0, 200.0, 200.0);
    let id_a = Uuid::new_v4();
    let id_b = Uuid::new_v4();
    let children = vec![
        (
            id_a,
            b(10.0, 10.0, 30.0, 30.0),
            cset(Constraint::Scale, Constraint::Scale),
        ),
        (
            id_b,
            b(50.0, 50.0, 20.0, 20.0),
            cset(Constraint::Fixed, Constraint::Fixed),
        ),
    ];
    let results = apply_constraints_batch(&children, parent_old, parent_new);
    assert_eq!(results.len(), 2);
    let a = results.iter().find(|(id, _)| *id == id_a).unwrap();
    assert!((a.1.x - 20.0).abs() < 1e-9, "first child scaled");
    let b_result = results.iter().find(|(id, _)| *id == id_b).unwrap();
    assert!((b_result.1.x - 50.0).abs() < 1e-9, "second child fixed");
}
