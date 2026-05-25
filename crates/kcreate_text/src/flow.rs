//! Inter-frame text flow.
//!
//! [`paragraph::layout_paragraph`](crate::paragraph::layout_paragraph)
//! lays a paragraph out inside *one* frame with multi-column support.
//! Phase 5 extends that with **flow**: when a frame overflows, the
//! remaining text continues in the next frame in a chain. This module
//! owns the "remaining text" bookkeeping so the per-frame engine
//! stays single-frame.
//!
//! The flow engine is *content-agnostic* with respect to obstacles:
//! the `wrap` module composes with this one by passing a derived
//! `FrameRect` for each obstacle-shrunk strip. For now we expose the
//! simple "give me an ordered list of frames and a string" API that
//! the bridge needs.
//!
//! ## Algorithm
//!
//! 1. For each frame in order, attempt to lay out the full remaining
//!    text inside that frame.
//! 2. If the layout fits (`overflow == false`), record those lines
//!    and stop; subsequent frames get an empty `FrameContent`.
//! 3. If it overflows, record only the lines that fit, then continue
//!    with the remaining substring in the next frame.
//!
//! Determining what "the lines that fit" means is non-trivial because
//! `paragraph::layout_paragraph` returns the full input even when it
//! overflowed (the per-frame engine just sets the `overflow` flag).
//! We resolve this by **arc-length walking the input glyphs**: after
//! the per-frame layout, count how many *characters* of the input
//! were consumed by the returned glyphs, then re-slice the input on a
//! byte boundary so subsequent frames continue exactly there.

use kcreate_core::node::TextFrameOptions;
use kcreate_core::Bounds;
use thiserror::Error;

use crate::hyphenation::HyphenationPatterns;
use crate::paragraph::{layout_paragraph, LayoutError, LayoutLine, TextStyle};

/// A rectangular text frame in document space.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameRect {
    /// The frame's bounding box.
    pub bounds: Bounds,
    /// Per-frame options (columns, gap, hyphenation, insets, overflow).
    pub options: TextFrameOptions,
}

/// One frame's worth of laid-out content. The host pairs these with
/// the `next_frame_id` chain on text nodes so undo/redo, selection
/// and hit-testing stay frame-local.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameContent {
    /// Index of the frame in the input `frames` vector.
    pub frame_index: usize,
    /// Lines that fit inside this frame.
    pub lines: Vec<LayoutLine>,
    /// Whether the next frame in the chain receives overflow text
    /// from this one. For the last frame this is identical to
    /// `ParagraphLayout::overflow` — the chain is exhausted.
    pub overflowed_into_next: bool,
}

/// Errors that can arise from the flow engine. Mirrors
/// [`LayoutError`] plus an "empty chain" sentinel for callers that
/// pass an empty `frames` slice.
#[derive(Debug, Error)]
pub enum FlowError {
    #[error(transparent)]
    Layout(#[from] LayoutError),
    #[error("text flow requires at least one frame; got zero")]
    EmptyFrameChain,
}

/// Engine struct so the bridge can hold an engine and feed it
/// successive `(text, frames, style)` snapshots without re-allocating
/// the `FrameRect` vector. Currently stateless; the struct exists for
/// API forward compatibility (cached shaping per frame is a likely
/// follow-up).
#[derive(Debug, Default, Clone, Copy)]
pub struct TextFlowEngine;

impl TextFlowEngine {
    /// Construct a fresh engine.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Distribute `text` across `frames` in order. Returns one
    /// [`FrameContent`] per input frame, even when later frames
    /// receive nothing (their `lines` are empty).
    ///
    /// When a frame overflows, we use **word-boundary binary search**
    /// over the remaining text to find the largest whitespace-bounded
    /// prefix that fits in the frame, then continue the chain with
    /// the suffix.
    pub fn layout(
        &self,
        text: &str,
        frames: &[FrameRect],
        style: &TextStyle,
        patterns: Option<&HyphenationPatterns>,
    ) -> Result<Vec<FrameContent>, FlowError> {
        if frames.is_empty() {
            return Err(FlowError::EmptyFrameChain);
        }
        let mut out = Vec::with_capacity(frames.len());
        let mut remaining = text.to_string();
        for (idx, frame) in frames.iter().enumerate() {
            if remaining.is_empty() {
                out.push(FrameContent {
                    frame_index: idx,
                    lines: Vec::new(),
                    overflowed_into_next: false,
                });
                continue;
            }
            let last = idx + 1 == frames.len();
            let layout =
                layout_paragraph(&remaining, style, &frame.options, frame.bounds, patterns)?;

            if !layout.overflow || last {
                // Everything fit, or chain exhausted.
                out.push(FrameContent {
                    frame_index: idx,
                    lines: layout.lines,
                    overflowed_into_next: false,
                });
                remaining.clear();
                continue;
            }

            // Binary-search the largest prefix (snapped to a word
            // boundary) that fits in this frame.
            let split = largest_fitting_prefix_bytes(
                &remaining,
                style,
                &frame.options,
                frame.bounds,
                patterns,
            )?;
            let (head, tail) = remaining.split_at(split);
            // Re-run the per-frame layout on the prefix so the
            // recorded lines correspond exactly to what we'll show.
            let fitted = layout_paragraph(head, style, &frame.options, frame.bounds, patterns)?;
            out.push(FrameContent {
                frame_index: idx,
                lines: fitted.lines,
                overflowed_into_next: true,
            });
            remaining = tail.trim_start().to_string();
        }
        Ok(out)
    }
}

/// Word-boundary aware binary search: find the largest byte length
/// `n` such that `text[..n]` lays out without overflow in `frame`.
/// `n` always snaps to a whitespace / char boundary.
fn largest_fitting_prefix_bytes(
    text: &str,
    style: &TextStyle,
    options: &TextFrameOptions,
    bounds: Bounds,
    patterns: Option<&HyphenationPatterns>,
) -> Result<usize, LayoutError> {
    // Build a list of candidate split points: every whitespace
    // boundary plus the end of the string.
    let mut candidates: Vec<usize> = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i].is_ascii_whitespace() {
            candidates.push(i);
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    candidates.push(text.len());
    // De-duplicate adjacent equals.
    candidates.dedup();
    if candidates.is_empty() {
        return Ok(0);
    }

    // Binary search across candidates.
    let mut lo = 0usize;
    let mut hi = candidates.len() - 1;
    let mut best: usize = 0;
    while lo <= hi {
        let mid = usize::midpoint(lo, hi);
        let split = candidates[mid];
        let head = &text[..split];
        let layout = layout_paragraph(head, style, options, bounds, patterns)?;
        if layout.overflow {
            if mid == 0 {
                break;
            }
            hi = mid - 1;
        } else {
            best = split;
            if mid == candidates.len() - 1 {
                break;
            }
            lo = mid + 1;
        }
    }
    Ok(best)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kcreate_core::node::{FrameInsets, TextFrameOptions};

    fn small_style() -> TextStyle {
        TextStyle {
            font_family: "Arial".into(),
            font_size: 12.0,
            line_height: 1.2,
        }
    }

    fn frame(x: f64, y: f64, w: f64, h: f64) -> FrameRect {
        FrameRect {
            bounds: Bounds {
                x,
                y,
                width: w,
                height: h,
            },
            options: TextFrameOptions {
                columns: 1,
                column_gap: 0.0,
                hyphenation: false,
                inset: FrameInsets::default(),
                ..Default::default()
            },
        }
    }

    #[test]
    fn empty_chain_errors() {
        let engine = TextFlowEngine::new();
        let err = engine
            .layout("hi", &[], &small_style(), None)
            .expect_err("empty chain should error");
        assert!(matches!(err, FlowError::EmptyFrameChain));
    }

    #[test]
    fn single_frame_holds_short_text() {
        let engine = TextFlowEngine::new();
        let frames = vec![frame(0.0, 0.0, 200.0, 200.0)];
        let out = engine
            .layout("hi", &frames, &small_style(), None)
            .expect("layout");
        assert_eq!(out.len(), 1);
        assert!(!out[0].overflowed_into_next);
    }

    #[test]
    fn empty_text_produces_empty_layout() {
        let engine = TextFlowEngine::new();
        let frames = vec![frame(0.0, 0.0, 200.0, 200.0)];
        let out = engine
            .layout("", &frames, &small_style(), None)
            .expect("layout");
        assert_eq!(out.len(), 1);
        assert!(out[0].lines.is_empty());
        assert!(!out[0].overflowed_into_next);
    }
}
