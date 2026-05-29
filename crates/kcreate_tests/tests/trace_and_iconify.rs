//! Phase 9 Tasks 12 + 19 — raster-to-vector tracing + icon-ify.
//!
//! Cross-crate sanity coverage for the actual algorithms. The bridge
//! wires these into the workspace; here we drive `trace_raster` and
//! `iconify` directly on synthetic data to lock the math.

use kcreate_ai::iconify::{iconify, IconPath, IconPoint, IconifyOptions};
use kcreate_ai::trace::{trace_raster, TraceOptions, TraceThreshold, TracedPoint};

/// Build a square RGBA image with a filled black rectangle in the
/// middle so `trace_raster` has a single closed contour to find.
fn rect_image(w: u32, h: u32, rect: (u32, u32, u32, u32)) -> Vec<u8> {
    let mut buf = vec![255u8; (w * h * 4) as usize];
    let (rx, ry, rw, rh) = rect;
    for y in ry..ry + rh {
        for x in rx..rx + rw {
            let idx = ((y * w + x) * 4) as usize;
            buf[idx] = 0;
            buf[idx + 1] = 0;
            buf[idx + 2] = 0;
            buf[idx + 3] = 255;
        }
    }
    buf
}

#[test]
fn trace_finds_single_closed_contour_for_filled_rect() {
    let img = rect_image(32, 32, (8, 8, 16, 16));
    let opts = TraceOptions {
        threshold: TraceThreshold::Fixed { value: 128 },
        simplify_tolerance: 0.5,
        min_path_points: 4,
        smooth: false,
    };
    let paths = trace_raster(&img, 32, 32, &opts).expect("trace");
    assert!(
        !paths.is_empty(),
        "must find at least one contour for a black rect"
    );
    // Every contour over a filled shape must be closed.
    assert!(paths.iter().all(|p| p.closed));
    // RDP should leave roughly 4 corners for an axis-aligned square,
    // but allow some slack for the boundary stepping in Moore-neighbour.
    assert!(
        paths[0].points.len() >= 4,
        "closed rect should have >= 4 points"
    );
    assert!(
        paths[0].points.len() <= 32,
        "simplified rect should not have many points (got {})",
        paths[0].points.len()
    );
}

#[test]
fn trace_rejects_too_small_image() {
    let img = vec![0u8; 4];
    let opts = TraceOptions::default();
    let err = trace_raster(&img, 1, 1, &opts).expect_err("too small must error");
    assert!(
        format!("{err}").to_ascii_lowercase().contains("small")
            || format!("{err:?}").contains("TooSmall")
    );
}

#[test]
fn trace_rejects_negative_tolerance() {
    let img = rect_image(8, 8, (2, 2, 4, 4));
    let opts = TraceOptions {
        simplify_tolerance: -1.0,
        ..Default::default()
    };
    let err = trace_raster(&img, 8, 8, &opts).expect_err("negative tolerance");
    assert!(format!("{err:?}").contains("Tolerance") || format!("{err:?}").contains("tolerance"));
}

#[test]
fn iconify_normalises_paths_to_grid() {
    let path = IconPath {
        points: vec![
            IconPoint { x: 0.0, y: 0.0 },
            IconPoint { x: 100.0, y: 0.0 },
            IconPoint { x: 100.0, y: 50.0 },
            IconPoint { x: 0.0, y: 50.0 },
            IconPoint { x: 0.0, y: 0.0 },
        ],
        closed: true,
    };
    let opts = IconifyOptions {
        grid_size: 24,
        ..Default::default()
    };
    let result = iconify(&[path], &opts).expect("iconify");
    assert!(!result.paths.is_empty(), "must return at least one path");
    // All output points must fit inside the 24×24 grid.
    for p in &result.paths {
        for pt in &p.points {
            assert!(
                pt.x >= 0.0 && pt.x <= 24.0,
                "point x {} out of [0,24]",
                pt.x
            );
            assert!(
                pt.y >= 0.0 && pt.y <= 24.0,
                "point y {} out of [0,24]",
                pt.y
            );
        }
    }
}

#[test]
fn iconify_rejects_empty_input() {
    let opts = IconifyOptions::default();
    let result = iconify(&[], &opts);
    // The implementation may return an error or an empty Ok — both
    // are valid for an empty input; just guarantee it does not panic
    // and does not silently produce phantom output.
    match result {
        Ok(r) => assert!(r.paths.is_empty(), "no input must mean no output"),
        Err(_) => { /* explicit error is fine */ }
    }
}

#[test]
fn trace_point_struct_is_serializable() {
    // Wire-format paranoia: TracedPoint is serialised to JSON in the
    // bridge `traced_polyline` metadata. f32 NaN/Inf would break it,
    // so just confirm a regular point round-trips.
    let p = TracedPoint { x: 1.5, y: 2.25 };
    let s = serde_json::to_string(&p).expect("serialize");
    let back: TracedPoint = serde_json::from_str(&s).expect("deserialize");
    assert_eq!(p, back);
}
