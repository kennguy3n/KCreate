//! Cross-crate coverage for Phase 5 vector studio additions:
//! - snap engine
//! - simplify / smooth / offset
//! - variable-width stroke expand
//! - multi-fill serde round-trip
//! - dash path effect
//! - round-corners path effect

use kcreate_core::node::{FillStyle, NodeStyle, RgbaColor, StrokeStyle};
use kcreate_vector::path::{PathPoint, PathSegment};
use kcreate_vector::{
    dash, expand_variable_stroke, offset, round_corners, simplify, smooth, SnapEngine, SnapTarget,
    VectorPath,
};

fn line(points: &[(f64, f64)], close: bool) -> VectorPath {
    let mut cmds: Vec<PathSegment> = Vec::with_capacity(points.len() + 1);
    for (i, (x, y)) in points.iter().enumerate() {
        if i == 0 {
            cmds.push(PathSegment::MoveTo(PathPoint::new(*x, *y)));
        } else {
            cmds.push(PathSegment::LineTo(PathPoint::new(*x, *y)));
        }
    }
    if close {
        cmds.push(PathSegment::Close);
    }
    VectorPath::new(cmds)
}

#[test]
fn snap_finds_correct_edge_within_threshold() {
    let target = SnapTarget::from_bounds(100.0, 200.0, 100.0, 50.0);
    let engine = SnapEngine::new(vec![target]);
    // Candidate's left edge is 102; threshold 8 should snap left to 100.
    let res = engine.snap(102.0, 50.0, 10.0, 10.0, 8.0);
    assert!(
        (res.dx + 2.0).abs() < 1e-9,
        "expected dx to bring left from 102 to 100 (dx=-2), got {}",
        res.dx
    );
    assert!(!res.guides.is_empty(), "guide should be emitted");
}

#[test]
fn snap_with_no_nearby_targets_returns_zero_delta() {
    let target = SnapTarget::from_bounds(100.0, 200.0, 100.0, 50.0);
    let engine = SnapEngine::new(vec![target]);
    // Candidate is far from any edge; threshold 1 should not snap.
    let res = engine.snap(0.0, 0.0, 10.0, 10.0, 1.0);
    assert!(res.dx.abs() < 1e-12, "dx should be zero");
    assert!(res.dy.abs() < 1e-12, "dy should be zero");
    assert!(res.guides.is_empty(), "no guides when no snap occurred");
}

#[test]
fn simplify_on_straight_line_keeps_only_endpoints() {
    let path = line(&[(0.0, 0.0), (1.0, 0.0), (2.0, 0.0), (3.0, 0.0)], false);
    let out = simplify(&path, 1e-6);
    // Should reduce to 2 points: MoveTo + final LineTo. The RDP
    // implementation keeps the first and last vertex.
    let move_count = out
        .commands
        .iter()
        .filter(|c| matches!(c, PathSegment::MoveTo(_)))
        .count();
    let line_count = out
        .commands
        .iter()
        .filter(|c| matches!(c, PathSegment::LineTo(_)))
        .count();
    assert_eq!(move_count, 1);
    assert_eq!(line_count, 1);
}

#[test]
fn smooth_increases_segment_count() {
    let path = line(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)], true);
    let before = path.commands.len();
    let out = smooth(&path, 2);
    let after = out.commands.len();
    assert!(after > before, "smooth must add subdivision points");
}

#[test]
fn offset_on_closed_circle_changes_mean_radius() {
    // Approximate a circle with a 16-sided polygon. Per the
    // `offset` docs, a positive distance INSETS a closed path
    // (offset to interior), so the resulting mean radius should be
    // smaller than the input radius.
    let n = 16;
    let r = 100.0_f64;
    let mut pts: Vec<(f64, f64)> = (0..n)
        .map(|i| {
            let t = f64::from(i) * std::f64::consts::TAU / f64::from(n);
            (r * t.cos(), r * t.sin())
        })
        .collect();
    pts.push(pts[0]);
    let path = line(&pts, true);
    let inset = offset(&path, 10.0);
    let mean_inset = mean_radius(&inset);
    assert!(
        (mean_inset - 90.0).abs() < 5.0,
        "inset mean radius should be near r-10; got {mean_inset}"
    );

    let outset = offset(&path, -10.0);
    let mean_outset = mean_radius(&outset);
    assert!(
        (mean_outset - 110.0).abs() < 5.0,
        "outset mean radius should be near r+10; got {mean_outset}"
    );
}

fn mean_radius(path: &VectorPath) -> f64 {
    let mut sum: f64 = 0.0;
    let mut count: f64 = 0.0;
    for cmd in &path.commands {
        match cmd {
            PathSegment::MoveTo(p) | PathSegment::LineTo(p) => {
                sum += p.x.hypot(p.y);
                count += 1.0;
            }
            _ => {}
        }
    }
    sum / count.max(1.0)
}

#[test]
fn variable_stroke_expand_produces_closed_outline() {
    let centreline = line(&[(0.0, 0.0), (100.0, 0.0)], false);
    let profile = vec![(0.0, 4.0), (1.0, 12.0)];
    let outline = expand_variable_stroke(&centreline, &profile, 8.0);
    // Expanded outline is a filled path: must close (or end with the
    // start point).
    let closed = outline
        .commands
        .iter()
        .any(|c| matches!(c, PathSegment::Close));
    assert!(closed, "variable stroke must produce a closed outline");
}

#[test]
fn multi_fill_serde_round_trip_preserves_order() {
    let primary = FillStyle::Solid(RgbaColor::new(1.0, 0.0, 0.0, 1.0));
    let extra1 = FillStyle::Solid(RgbaColor::new(0.0, 1.0, 0.0, 0.5));
    let extra2 = FillStyle::Solid(RgbaColor::new(0.0, 0.0, 1.0, 0.25));
    let style = NodeStyle {
        fill: primary,
        extra_fills: vec![extra1, extra2],
        stroke: Some(StrokeStyle::default()),
        ..Default::default()
    };
    let json = serde_json::to_string(&style).expect("serialize");
    let back: NodeStyle = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.extra_fills.len(), 2);
    if let (FillStyle::Solid(c0), FillStyle::Solid(c1)) =
        (&back.extra_fills[0], &back.extra_fills[1])
    {
        // Ordering is preserved.
        assert!(c0.g > 0.5, "first extra should be green-dominant");
        assert!(c1.b > 0.2, "second extra should be blue-dominant");
    } else {
        panic!("extras lost their solid fill type");
    }
}

#[test]
fn dash_pattern_produces_expected_subpath_count_for_three_dashes() {
    // 100-unit line, dashed at 10-on / 10-off → 5 visible dashes (50 / 20).
    let path = line(&[(0.0, 0.0), (100.0, 0.0)], false);
    let subpaths = dash(&path, &[10.0, 10.0], 0.0);
    assert!(
        (3..=6).contains(&subpaths.len()),
        "expected 3..=6 dashes, got {}",
        subpaths.len()
    );
}

#[test]
fn round_corners_reduces_sharp_angle_at_apex() {
    // Right angle path: two LineTos meeting at (10,0) at 90°.
    let path = line(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)], false);
    let before = path.commands.len();
    let rounded = round_corners(&path, 2.0);
    // Rounding inserts intermediate curve segments at every
    // detected corner, so the command count must grow.
    assert!(
        rounded.commands.len() > before,
        "round_corners should add segments at the corner"
    );
}
