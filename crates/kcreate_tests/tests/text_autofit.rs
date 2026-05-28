//! Phase 8 Block D: text auto-fit.
//!
//! Integration test for [`kcreate_text::autofit::compute_autofit_size`].

use kcreate_core::node::{Bounds, TextFrameOptions};
use kcreate_text::autofit::{compute_autofit_size, AutofitOptions};
use kcreate_text::paragraph::TextStyle;

fn frame(w: f64, h: f64) -> Bounds {
    Bounds {
        x: 0.0,
        y: 0.0,
        width: w,
        height: h,
    }
}

fn style() -> TextStyle {
    TextStyle::default()
}

#[test]
fn binary_search_converges_in_range() {
    let opts = AutofitOptions {
        min_size: 8.0,
        max_size: 72.0,
        tolerance: 0.5,
        max_iterations: 20,
    };
    let frame_opts = TextFrameOptions::default();
    let result = compute_autofit_size(
        "Hello world",
        &style(),
        &frame_opts,
        frame(200.0, 50.0),
        &opts,
    )
    .unwrap();
    assert!(result >= opts.min_size);
    assert!(result <= opts.max_size);
}

#[test]
fn min_returned_when_text_never_fits() {
    let opts = AutofitOptions {
        min_size: 12.0,
        max_size: 48.0,
        tolerance: 0.5,
        max_iterations: 20,
    };
    let frame_opts = TextFrameOptions::default();
    let result = compute_autofit_size(
        "long text here that will not fit in a tiny frame",
        &style(),
        &frame_opts,
        frame(1.0, 1.0),
        &opts,
    )
    .unwrap();
    assert!((result - opts.min_size).abs() < 1e-3);
}

#[test]
fn empty_text_returns_max_size() {
    let opts = AutofitOptions {
        min_size: 8.0,
        max_size: 72.0,
        tolerance: 0.5,
        max_iterations: 20,
    };
    let frame_opts = TextFrameOptions::default();
    let result =
        compute_autofit_size("", &style(), &frame_opts, frame(200.0, 50.0), &opts).unwrap();
    assert!((result - opts.max_size).abs() < 1e-3);
}

#[test]
fn fits_at_max_returns_max_directly() {
    let opts = AutofitOptions {
        min_size: 8.0,
        max_size: 12.0,
        tolerance: 0.5,
        max_iterations: 20,
    };
    let frame_opts = TextFrameOptions::default();
    let result =
        compute_autofit_size("Hi", &style(), &frame_opts, frame(10000.0, 10000.0), &opts).unwrap();
    assert!((result - opts.max_size).abs() < 1e-3);
}

#[test]
fn rejects_invalid_min() {
    let opts = AutofitOptions {
        min_size: 0.0,
        max_size: 12.0,
        tolerance: 0.5,
        max_iterations: 20,
    };
    let frame_opts = TextFrameOptions::default();
    let result = compute_autofit_size("hi", &style(), &frame_opts, frame(100.0, 100.0), &opts);
    assert!(result.is_err());
}

#[test]
fn rejects_inverted_bracket() {
    let opts = AutofitOptions {
        min_size: 20.0,
        max_size: 10.0,
        tolerance: 0.5,
        max_iterations: 20,
    };
    let frame_opts = TextFrameOptions::default();
    let result = compute_autofit_size("hi", &style(), &frame_opts, frame(100.0, 100.0), &opts);
    assert!(result.is_err());
}
