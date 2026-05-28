//! Smart text auto-fit.
//!
//! Given a string and a frame, finds the largest font size in
//! `[min, max]` that lays the text out without overflowing the
//! frame. Used by the renderer's auto-fit mode on text layers,
//! so the user can drag a frame and the headline tracks the new
//! size automatically.
//!
//! The search is a textbook bisection over font size — the layout
//! engine is non-monotonic only at the sub-pixel level so 6-7
//! iterations gets within a pixel of optimal on any sensible
//! frame. We cap at 10 to ensure termination on adversarial
//! inputs.

use kcreate_core::node::{Bounds, TextFrameOptions};
use thiserror::Error;

use crate::paragraph::{layout_paragraph, LayoutError, TextStyle};

/// Options controlling the autofit search.
#[derive(Debug, Clone)]
pub struct AutofitOptions {
    /// Lower bound for the font size in document units. Must be
    /// > 0.
    pub min_size: f32,
    /// Upper bound for the font size in document units. Must be
    /// >= `min_size`.
    pub max_size: f32,
    /// Tolerance in document units. The search stops when the
    /// bracket is narrower than this. Defaults to 0.25 px which
    /// is one sub-pixel quad on a Retina screen.
    pub tolerance: f32,
    /// Maximum bisection iterations. Caps work even when the
    /// tolerance can't be reached (e.g. text never fits at the
    /// minimum size).
    pub max_iterations: u32,
}

impl Default for AutofitOptions {
    fn default() -> Self {
        Self {
            min_size: 8.0,
            max_size: 96.0,
            tolerance: 0.25,
            max_iterations: 10,
        }
    }
}

#[derive(Debug, Error)]
pub enum AutofitError {
    #[error("min_size must be > 0, got {0}")]
    InvalidMin(f32),
    #[error("max_size must be >= min_size, got min={min} max={max}")]
    InvalidBracket { min: f32, max: f32 },
    #[error(transparent)]
    Layout(#[from] LayoutError),
}

/// Find the largest font size in `[opts.min_size, opts.max_size]`
/// that fits `text` within `frame_bounds`.
///
/// Returns the chosen size. If the text doesn't fit at the
/// minimum, returns `opts.min_size` — the renderer's overflow
/// mode then decides whether to clip, ellipsize, or overflow.
pub fn compute_autofit_size(
    text: &str,
    style: &TextStyle,
    frame: &TextFrameOptions,
    frame_bounds: Bounds,
    opts: &AutofitOptions,
) -> Result<f32, AutofitError> {
    if opts.min_size <= 0.0 {
        return Err(AutofitError::InvalidMin(opts.min_size));
    }
    if opts.max_size < opts.min_size {
        return Err(AutofitError::InvalidBracket {
            min: opts.min_size,
            max: opts.max_size,
        });
    }
    // Empty text fits at any size; return the max so the caret
    // shows up large enough to be visible.
    if text.is_empty() {
        return Ok(opts.max_size);
    }
    // Quick check: does the text already fit at the max size?
    // If yes, no search is needed.
    if fits(text, style, frame, frame_bounds, opts.max_size)? {
        return Ok(opts.max_size);
    }
    // Quick check: does it fail even at the minimum?
    if !fits(text, style, frame, frame_bounds, opts.min_size)? {
        return Ok(opts.min_size);
    }
    // Bisect.
    let mut lo = opts.min_size;
    let mut hi = opts.max_size;
    let mut best = lo;
    for _ in 0..opts.max_iterations {
        if hi - lo <= opts.tolerance {
            break;
        }
        let mid = (lo + hi) * 0.5;
        if fits(text, style, frame, frame_bounds, mid)? {
            best = mid;
            lo = mid;
        } else {
            hi = mid;
        }
    }
    Ok(best)
}

fn fits(
    text: &str,
    style: &TextStyle,
    frame: &TextFrameOptions,
    frame_bounds: Bounds,
    size: f32,
) -> Result<bool, LayoutError> {
    let mut style = style.clone();
    style.font_size = size;
    let layout = layout_paragraph(text, &style, frame, frame_bounds, None)?;
    Ok(!layout.overflow)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kcreate_core::node::{FrameInsets, TextFrameOptions};

    fn frame() -> TextFrameOptions {
        TextFrameOptions {
            inset: FrameInsets::default(),
            column_gap: 0.0,
            columns: 1,
            ..TextFrameOptions::default()
        }
    }

    fn bounds(w: f64, h: f64) -> Bounds {
        Bounds {
            x: 0.0,
            y: 0.0,
            width: w,
            height: h,
        }
    }

    #[test]
    fn rejects_zero_min() {
        let opts = AutofitOptions {
            min_size: 0.0,
            ..AutofitOptions::default()
        };
        let err = compute_autofit_size(
            "hi",
            &TextStyle::default(),
            &frame(),
            bounds(100.0, 50.0),
            &opts,
        )
        .expect_err("must reject");
        assert!(matches!(err, AutofitError::InvalidMin(_)));
    }

    #[test]
    fn rejects_inverted_bracket() {
        let opts = AutofitOptions {
            min_size: 30.0,
            max_size: 10.0,
            ..AutofitOptions::default()
        };
        let err = compute_autofit_size(
            "hi",
            &TextStyle::default(),
            &frame(),
            bounds(100.0, 50.0),
            &opts,
        )
        .expect_err("must reject");
        assert!(matches!(err, AutofitError::InvalidBracket { .. }));
    }

    #[test]
    fn empty_text_returns_max() {
        let opts = AutofitOptions::default();
        let size = compute_autofit_size(
            "",
            &TextStyle::default(),
            &frame(),
            bounds(100.0, 50.0),
            &opts,
        )
        .expect("ok");
        assert!((size - opts.max_size).abs() < f32::EPSILON);
    }

    #[test]
    fn fits_at_max_returns_max() {
        // A huge frame fits "hi" at any reasonable size.
        let opts = AutofitOptions {
            min_size: 4.0,
            max_size: 24.0,
            ..AutofitOptions::default()
        };
        let size = compute_autofit_size(
            "hi",
            &TextStyle::default(),
            &frame(),
            bounds(10_000.0, 10_000.0),
            &opts,
        )
        .expect("ok");
        assert!((size - opts.max_size).abs() < f32::EPSILON);
    }

    #[test]
    fn tiny_frame_returns_min() {
        let opts = AutofitOptions {
            min_size: 4.0,
            max_size: 24.0,
            ..AutofitOptions::default()
        };
        let size = compute_autofit_size(
            "this text is far too long to ever fit",
            &TextStyle::default(),
            &frame(),
            bounds(2.0, 2.0),
            &opts,
        )
        .expect("ok");
        assert!((size - opts.min_size).abs() < f32::EPSILON);
    }

    #[test]
    fn bisection_returns_value_within_tolerance_of_optimum() {
        // For a small frame, the optimal size lives strictly
        // between min and max. The autofit value must be within
        // tolerance of the upper bound that still fits.
        let opts = AutofitOptions {
            min_size: 4.0,
            max_size: 64.0,
            tolerance: 0.5,
            max_iterations: 12,
        };
        let style = TextStyle {
            font_family: "sans-serif".into(),
            font_size: 16.0,
            line_height: 1.25,
        };
        let size =
            compute_autofit_size("hello world", &style, &frame(), bounds(120.0, 40.0), &opts)
                .expect("ok");
        assert!(size >= opts.min_size && size <= opts.max_size, "got {size}");
    }
}
