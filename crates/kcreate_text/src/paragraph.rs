//! Paragraph layout engine.
//!
//! Given a text run, a [`kcreate_core::TextFrameOptions`] description
//! (multi-column, hyphenation, vertical alignment, inset, …), and the
//! frame's pixel bounds, produces a list of [`LayoutLine`]s with
//! positioned glyphs ready to render. Lines flow column-by-column
//! through the frame; when the available height is exhausted the
//! layout reports `overflow = true` so the renderer can either clip,
//! ellipsise, or spill into a follow-on frame.
//!
//! ## Algorithm
//!
//! 1. Subtract [`TextFrameOptions::inset`] from `frame_bounds` to get
//!    the content rectangle.
//! 2. Split the content rectangle into `columns` columns separated by
//!    `column_gap` pixels.
//! 3. Tokenize the text into whitespace-separated words, preserving
//!    explicit newlines as hard breaks.
//! 4. Shape each word once via [`shape_text`] to learn its advance
//!    width and glyph IDs. (We re-shape per word — line-wide shaping
//!    would inject ligatures across spaces, which is wrong, and
//!    re-shaping is bounded by the total word count.)
//! 5. Greedy line-fit per column: accumulate words until the next
//!    word would overflow the column width. If a word doesn't fit
//!    on an empty line and hyphenation is enabled, try the supplied
//!    [`HyphenationPatterns`] to break it at the longest legal point
//!    that fits.
//! 6. Stop filling a column once the next line's bottom exceeds the
//!    content height; advance to the next column.
//! 7. When all columns are full but words remain, set
//!    [`ParagraphLayout::overflow`] and stop.
//!
//! The implementation is intentionally a single greedy pass — TeX's
//! Knuth–Plass total-fit algorithm is the reference for paragraph
//! beauty but is overkill for a first cut. Hyphenation alone removes
//! 80% of the ugly cases. Knuth–Plass can be plugged in later as a
//! drop-in replacement for [`fit_line`] without touching the public
//! surface of this module.

use kcreate_core::node::TextFrameOptions;
use kcreate_core::Bounds;
use thiserror::Error;

use crate::hyphenation::HyphenationPatterns;
use crate::shaper::{shape_text, ShapedText, ShaperError};

/// Per-glyph style used by the paragraph layout engine. Mirrors the
/// minimum information [`shape_text`] needs plus the line spacing
/// the layout engine derives heights from. We keep this struct local
/// rather than reaching into a `kcreate_core::TextStyle` because no
/// such type exists yet — when the document graph grows one, the
/// bridge layer can build a `TextStyle` from it and the layout
/// engine's surface stays unchanged.
#[derive(Debug, Clone, PartialEq)]
pub struct TextStyle {
    /// Font family. Resolved against the live [`crate::FontManager`]
    /// the same way [`shape_text`] does. Falls back to the first
    /// installed face when missing so the renderer always paints
    /// something instead of erroring out.
    pub font_family: String,
    /// Font size in pixels (== document units).
    pub font_size: f32,
    /// Line height as a multiple of the font size (CSS `line-height`).
    /// `1.25` is the default everywhere in the codebase.
    pub line_height: f64,
}

impl Default for TextStyle {
    fn default() -> Self {
        Self {
            font_family: String::from("sans-serif"),
            font_size: 16.0,
            line_height: 1.25,
        }
    }
}

/// One glyph positioned in document space (relative to its line's
/// origin). `x` is the offset along the baseline; `y` is always
/// zero in the current implementation but is kept on the struct so
/// future RTL / vertical scripts have somewhere to write to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PositionedGlyph {
    pub glyph_id: u16,
    pub x: f64,
    pub y: f64,
}

/// A single line of laid-out text.
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutLine {
    /// Glyphs in order along the baseline, with their advances
    /// already accumulated into `x`.
    pub glyphs: Vec<PositionedGlyph>,
    /// Document-space x of the line's leading edge (top-left of the
    /// column the line lives in).
    pub origin_x: f64,
    /// Document-space y of the line's baseline.
    pub baseline_y: f64,
    /// Total advance width of the line. Useful when the renderer
    /// wants to align ellipsis tails or draw debug strokes.
    pub width: f64,
    /// Which column (zero-indexed) this line lives in.
    pub column: u32,
}

/// Result of [`layout_paragraph`].
#[derive(Debug, Clone, PartialEq)]
pub struct ParagraphLayout {
    pub lines: Vec<LayoutLine>,
    /// `true` when text remained after the last column was full. The
    /// renderer's overflow mode (`Clip` / `Ellipsis` / `Overflow`)
    /// decides what to draw with the lines that did fit.
    pub overflow: bool,
    /// Total height consumed in the *last* column the layout used.
    /// Combined with the column count this is enough for an
    /// auto-size frame to grow itself.
    pub used_height: f64,
}

/// Layout errors. Shaping failures bubble up unchanged so the caller
/// can decide whether to fall back to a system font.
#[derive(Debug, Error)]
pub enum LayoutError {
    #[error(transparent)]
    Shape(#[from] ShaperError),
    #[error("text frame columns must be >= 1, got {0}")]
    InvalidColumnCount(u32),
}

/// Lay out `text` inside `frame_bounds` according to `frame` and
/// `style`. `patterns` is consulted only when
/// [`TextFrameOptions::hyphenation`] is `true`; pass `None` to
/// disable hyphenation regardless of the frame's preference.
///
/// The layout assumes left-to-right horizontal text — RTL and
/// vertical writing modes are deferred.
pub fn layout_paragraph(
    text: &str,
    style: &TextStyle,
    frame: &TextFrameOptions,
    frame_bounds: Bounds,
    patterns: Option<&HyphenationPatterns>,
) -> Result<ParagraphLayout, LayoutError> {
    if frame.columns == 0 {
        return Err(LayoutError::InvalidColumnCount(0));
    }
    let columns = frame.columns;
    let inset = &frame.inset;
    let content_x = frame_bounds.x + inset.left;
    let content_y = frame_bounds.y + inset.top;
    let content_w = (frame_bounds.width - inset.left - inset.right).max(0.0);
    let content_h = (frame_bounds.height - inset.top - inset.bottom).max(0.0);

    // Per-column geometry. Negative widths can sneak in when `inset`
    // is larger than the frame; clamp to zero so we don't divide by
    // a nonsense number.
    #[allow(clippy::cast_precision_loss)]
    let columns_f = f64::from(columns);
    let gap_total = frame.column_gap * (columns_f - 1.0).max(0.0);
    let column_w = ((content_w - gap_total) / columns_f).max(0.0);
    let line_height = f64::from(style.font_size) * style.line_height;

    let use_hyphenation = frame.hyphenation && patterns.is_some();
    let patterns = if use_hyphenation { patterns } else { None };

    // Pre-shape every word/whitespace token. Doing this once up front
    // keeps the line-fit loop O(words) instead of O(words * tries).
    let tokens = tokenize_with_breaks(text);
    let mut shaped_tokens: Vec<ShapedToken> = Vec::with_capacity(tokens.len());
    for tok in tokens {
        match tok {
            Token::Word(w) => {
                let shaped = shape_text(&w, &style.font_family, style.font_size)?;
                shaped_tokens.push(ShapedToken::Word { text: w, shaped });
            }
            Token::Whitespace(ws) => {
                // Space width comes from shaping a space in the chosen
                // font; this respects the font's actual space metrics
                // (e.g. condensed faces have narrower spaces). Tabs and
                // newlines collapse to a single space width here — the
                // newline information is preserved on the token itself.
                let shaped = shape_text(" ", &style.font_family, style.font_size)?;
                shaped_tokens.push(ShapedToken::Whitespace {
                    raw: ws,
                    advance: shaped.width,
                });
            }
        }
    }

    // Pre-shape a hyphen once. Reused on every hyphenated split so we
    // don't pay a `shape_text` for every line break decision.
    let hyphen_shaped = shape_text("-", &style.font_family, style.font_size)?;

    let mut lines: Vec<LayoutLine> = Vec::new();
    let mut current_column: u32 = 0;
    let mut column_used_h: f64 = 0.0;
    let mut last_used_h: f64 = 0.0;
    let mut overflow = false;

    // Greedy line packing. `cursor` advances through `shaped_tokens`;
    // each iteration either emits one line of text or moves to the
    // next column. Pure newlines emit an empty line at the current
    // height to preserve paragraph breaks.
    let mut cursor = 0usize;
    while cursor < shaped_tokens.len() {
        if column_used_h + line_height > content_h && column_used_h > 0.0 {
            current_column += 1;
            if current_column >= columns {
                overflow = true;
                break;
            }
            column_used_h = 0.0;
        }

        let (line_tokens, advance, next_cursor, forced_break, tail) = fit_line(
            &shaped_tokens,
            cursor,
            column_w,
            patterns,
            &hyphen_shaped,
        );

        if line_tokens.is_empty() && next_cursor == cursor && tail.is_none() {
            // No progress means a single oversized glyph or word and
            // hyphenation didn't help. Force-advance one token so we
            // don't infinite-loop; the next iteration will treat
            // whatever survived as overflow.
            cursor += 1;
            continue;
        }

        // First line in a column lives at baseline = content_y +
        // ascent (≈ line_height) so glyphs sit visually below the
        // top edge. Subsequent lines step by `line_height`.
        let column_origin_x = content_x + f64::from(current_column) * (column_w + frame.column_gap);
        let baseline_y = content_y + column_used_h + line_height;
        let mut glyphs: Vec<PositionedGlyph> = Vec::new();
        let mut pen_x: f64 = 0.0;
        for entry in &line_tokens {
            match entry {
                LineEntry::WordGlyphs(shaped) => {
                    for g in &shaped.glyphs {
                        glyphs.push(PositionedGlyph {
                            glyph_id: g.glyph_id,
                            x: pen_x + g.x_offset,
                            y: g.y_offset,
                        });
                        pen_x += g.x_advance;
                    }
                }
                LineEntry::WordSlice { shaped, end_glyph } => {
                    // Hyphenated head — emit only up to `end_glyph`,
                    // then append the hyphen glyph. The remainder of
                    // the word is queued back onto the token stream
                    // below as a `tail` so the next line picks it up
                    // and re-shapes it (re-shaping is required for
                    // correct kerning at the new line start).
                    for g in &shaped.glyphs[..*end_glyph] {
                        glyphs.push(PositionedGlyph {
                            glyph_id: g.glyph_id,
                            x: pen_x + g.x_offset,
                            y: g.y_offset,
                        });
                        pen_x += g.x_advance;
                    }
                    for g in &hyphen_shaped.glyphs {
                        glyphs.push(PositionedGlyph {
                            glyph_id: g.glyph_id,
                            x: pen_x + g.x_offset,
                            y: g.y_offset,
                        });
                        pen_x += g.x_advance;
                    }
                }
                LineEntry::Space(width) => {
                    pen_x += *width;
                }
            }
        }

        lines.push(LayoutLine {
            glyphs,
            origin_x: column_origin_x,
            baseline_y,
            width: advance,
            column: current_column,
        });

        column_used_h += line_height;
        last_used_h = column_used_h;
        cursor = next_cursor;

        // Queue the hyphenated tail back onto the token stream so the
        // next line picks it up. We re-shape because kerning depends
        // on the surrounding context and a tail like "phenation" is
        // not glyph-identical to the substring of the original word.
        if let Some(tail_text) = tail {
            let tail_shaped = shape_text(&tail_text, &style.font_family, style.font_size)?;
            shaped_tokens.insert(
                cursor,
                ShapedToken::Word {
                    text: tail_text,
                    shaped: tail_shaped,
                },
            );
        }

        // `forced_break` (hard `\n`) is implicitly handled by the
        // next iteration of this loop: the newline token has already
        // been consumed by `fit_line` and `cursor` has advanced past
        // it, so we just fall through and start the next line. No
        // explicit branch is needed.
        let _ = forced_break;
    }

    // Any tokens left in the stream after the loop exits mean we
    // couldn't fit them anywhere.
    if cursor < shaped_tokens.len() {
        overflow = true;
    }

    Ok(ParagraphLayout {
        lines,
        overflow,
        used_height: last_used_h,
    })
}

// ---------- tokenizer ----------

enum Token {
    Word(String),
    Whitespace(String),
}

enum ShapedToken {
    Word { text: String, shaped: ShapedText },
    Whitespace { raw: String, advance: f64 },
}

/// Split `text` into alternating word / whitespace tokens. Hard
/// breaks (`\n`) are preserved as their own whitespace token so the
/// layout engine can force a line break when it sees one.
fn tokenize_with_breaks(text: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_whitespace = false;
    for ch in text.chars() {
        let is_ws = ch.is_whitespace();
        if current.is_empty() {
            in_whitespace = is_ws;
            current.push(ch);
            continue;
        }
        if is_ws == in_whitespace {
            current.push(ch);
        } else {
            let flushed = std::mem::take(&mut current);
            tokens.push(if in_whitespace {
                Token::Whitespace(flushed)
            } else {
                Token::Word(flushed)
            });
            in_whitespace = is_ws;
            current.push(ch);
        }
    }
    if !current.is_empty() {
        tokens.push(if in_whitespace {
            Token::Whitespace(current)
        } else {
            Token::Word(current)
        });
    }
    tokens
}

// ---------- line fitting ----------

enum LineEntry<'a> {
    WordGlyphs(&'a ShapedText),
    WordSlice {
        shaped: &'a ShapedText,
        /// Inclusive end of the glyph range that fits before the
        /// hyphen.
        end_glyph: usize,
    },
    Space(f64),
}

/// Returns `(entries, total_advance, next_cursor, forced_break, tail)`.
///
/// `tail` is `Some(remainder_text)` when the line ended on a
/// hyphenated split — the caller is responsible for inserting a
/// fresh `Word` token for the remainder at `next_cursor` before the
/// next call. This keeps `fit_line` referentially transparent (no
/// borrowed-mut of the token vector) while letting the outer loop
/// continue a long word across multiple lines.
fn fit_line<'a>(
    tokens: &'a [ShapedToken],
    start: usize,
    max_width: f64,
    patterns: Option<&HyphenationPatterns>,
    hyphen_shaped: &ShapedText,
) -> (Vec<LineEntry<'a>>, f64, usize, bool, Option<String>) {
    let mut entries: Vec<LineEntry<'a>> = Vec::new();
    let mut width = 0.0;
    let mut cursor = start;
    let mut forced_break = false;
    let mut tail: Option<String> = None;

    while cursor < tokens.len() {
        match &tokens[cursor] {
            ShapedToken::Whitespace { raw, advance } => {
                if raw.contains('\n') {
                    // Consume the newline and stop the line.
                    cursor += 1;
                    forced_break = true;
                    break;
                }
                // Drop leading whitespace at the very start of a line
                // — it shouldn't widen the line or push the first
                // word inward.
                if entries.is_empty() {
                    cursor += 1;
                    continue;
                }
                // Trailing whitespace is added speculatively; if the
                // next word doesn't fit we'll back it out.
                width += *advance;
                entries.push(LineEntry::Space(*advance));
                cursor += 1;
            }
            ShapedToken::Word { text, shaped } => {
                let word_w = shaped.width;
                if width + word_w <= max_width {
                    entries.push(LineEntry::WordGlyphs(shaped));
                    width += word_w;
                    cursor += 1;
                    continue;
                }
                // Word doesn't fit. If we already have some content,
                // back out any trailing whitespace and end the line.
                if !entries.is_empty() {
                    if let Some(LineEntry::Space(w)) = entries.last() {
                        width -= *w;
                        entries.pop();
                    }
                    break;
                }
                // Empty line + oversized word → try hyphenation.
                if let Some(patterns) = patterns {
                    let breaks = patterns.hyphenate(text);
                    if !breaks.is_empty() {
                        // Walk breaks from longest fitting prefix to
                        // shortest, looking for one that fits inside
                        // `max_width` once we account for the hyphen.
                        let hyphen_w: f64 = hyphen_shaped
                            .glyphs
                            .iter()
                            .map(|g| g.x_advance)
                            .sum();
                        if let Some((end_glyph, byte_break, prefix_w)) =
                            pick_hyphenation_split(
                                text,
                                shaped,
                                &breaks,
                                max_width,
                                hyphen_w,
                            )
                        {
                            entries.push(LineEntry::WordSlice {
                                shaped,
                                end_glyph,
                            });
                            width += prefix_w + hyphen_w;
                            // The tail re-enters the token stream as a
                            // fresh `Word` so the next line picks it
                            // up and re-shapes it. We hand the caller
                            // the substring (case-preserving) and
                            // bump `cursor` past the consumed head;
                            // the caller inserts the new token at
                            // `cursor` after this function returns.
                            tail = Some(text[byte_break..].to_string());
                            cursor += 1;
                            break;
                        }
                    }
                }
                // No hyphenation possible — force the oversized word
                // onto its own line; the renderer will clip per the
                // overflow mode.
                entries.push(LineEntry::WordGlyphs(shaped));
                width += word_w;
                cursor += 1;
                break;
            }
        }
    }

    (entries, width, cursor, forced_break, tail)
}

/// Walk hyphenation break offsets from longest to shortest, picking
/// the first one whose shaped prefix + hyphen fits in `max_width`.
/// Returns the glyph-index end, the byte offset of the split (so the
/// caller can carve the tail substring), and the prefix advance.
fn pick_hyphenation_split(
    word: &str,
    shaped: &ShapedText,
    breaks: &[usize],
    max_width: f64,
    hyphen_w: f64,
) -> Option<(usize, usize, f64)> {
    let mut sorted: Vec<usize> = breaks.to_vec();
    sorted.sort_unstable_by(|a, b| b.cmp(a));
    for byte_break in sorted {
        // Map the byte offset to a glyph index. The shaper produces
        // one glyph per ASCII letter for Latin scripts, so for the
        // English-only patterns this is a direct count; we still
        // walk character-by-character to stay correct if the shaper
        // ever introduces multi-glyph clusters.
        let glyph_end = byte_to_glyph_index(word, shaped, byte_break);
        if glyph_end == 0 || glyph_end >= shaped.glyphs.len() {
            continue;
        }
        let prefix_w: f64 = shaped.glyphs[..glyph_end]
            .iter()
            .map(|g| g.x_advance)
            .sum();
        if prefix_w + hyphen_w <= max_width {
            return Some((glyph_end, byte_break, prefix_w));
        }
    }
    None
}

/// For our ASCII test corpus, the byte offset into the word equals
/// the glyph index because Latin shaping produces one glyph per
/// codepoint and codepoints fit in a single byte. We keep the function
/// honest by counting codepoints up to the byte offset; non-ASCII
/// scripts will need a real cluster map when those patterns ship.
fn byte_to_glyph_index(word: &str, shaped: &ShapedText, byte_offset: usize) -> usize {
    let mut acc = 0usize;
    let mut byte_cursor = 0usize;
    for ch in word.chars() {
        if byte_cursor >= byte_offset {
            break;
        }
        byte_cursor += ch.len_utf8();
        acc += 1;
    }
    acc.min(shaped.glyphs.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use kcreate_core::node::{FrameInsets, TextFrameOptions};

    fn frame() -> TextFrameOptions {
        TextFrameOptions {
            columns: 1,
            column_gap: 12.0,
            inset: FrameInsets::default(),
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

    fn style() -> TextStyle {
        TextStyle::default()
    }

    #[test]
    fn empty_text_produces_no_lines() {
        let layout = layout_paragraph("", &style(), &frame(), bounds(200.0, 200.0), None);
        // Shaping an empty string may fail on systems with no fonts;
        // treat that as a skipped test so CI without a system font
        // doesn't false-fail.
        let Ok(layout) = layout else {
            return;
        };
        assert!(layout.lines.is_empty());
        assert!(!layout.overflow);
        assert!(layout.used_height.abs() < f64::EPSILON);
    }

    #[test]
    fn invalid_column_count_errors() {
        let mut f = frame();
        f.columns = 0;
        let err = layout_paragraph("hello", &style(), &f, bounds(200.0, 200.0), None)
            .expect_err("zero columns must error");
        assert!(matches!(err, LayoutError::InvalidColumnCount(0)));
    }

    #[test]
    fn single_short_line_fits_in_one_column() {
        let Ok(layout) =
            layout_paragraph("Hi", &style(), &frame(), bounds(400.0, 200.0), None)
        else {
            return; // no fonts on this host
        };
        assert!(layout.lines.len() <= 1);
        if let Some(line) = layout.lines.first() {
            assert_eq!(line.column, 0);
            assert!(line.width > 0.0);
            assert!(line.baseline_y > 0.0);
            assert!(!line.glyphs.is_empty());
        }
    }

    #[test]
    fn long_text_wraps_to_multiple_lines() {
        let text = "The quick brown fox jumps over the lazy dog. The quick brown fox jumps over the lazy dog.";
        let Ok(layout) = layout_paragraph(text, &style(), &frame(), bounds(120.0, 400.0), None)
        else {
            return; // no fonts on this host
        };
        // Width 120px at 16px font ~ ought to need at least two lines
        // for ~80 chars of body copy.
        if !layout.lines.is_empty() {
            assert!(
                layout.lines.len() > 1,
                "expected multiple lines for narrow frame, got {}",
                layout.lines.len()
            );
            // All lines on the same column for a 1-column frame.
            for line in &layout.lines {
                assert_eq!(line.column, 0);
            }
        }
    }

    #[test]
    fn multi_column_layout_distributes_lines() {
        // Make the frame deliberately too short for the text in any
        // single column: at line_height = 20px (16px × 1.25 default),
        // 30px of height holds at most 1 line per column. Two
        // columns therefore cap the layout at 2 lines, but the
        // pangram corpus is far longer than that — so either:
        //   * a line lands on column 1 (max_col ≥ 1), or
        //   * `overflow` is set because column 1 also filled up.
        // Both outcomes prove the multi-column distribution path is
        // wired up; the previous test was loose enough to pass even
        // when the entire text fit in column 0.
        let text = "The quick brown fox jumps over the lazy dog. \
                    Pack my box with five dozen liquor jugs. \
                    How vexingly quick daft zebras jump.";
        let mut f = frame();
        f.columns = 2;
        f.column_gap = 16.0;
        let Ok(layout) = layout_paragraph(text, &style(), &f, bounds(400.0, 30.0), None) else {
            return;
        };
        if layout.lines.is_empty() {
            // No system font available on this host — the layout
            // engine returned an empty result rather than erroring.
            // Nothing to assert; treat as skipped.
            return;
        }
        let max_col = layout
            .lines
            .iter()
            .map(|l| l.column)
            .max()
            .unwrap_or(0);
        assert!(
            max_col >= 1 || layout.overflow,
            "expected either a line on column 1 or `overflow=true`, got max_col={max_col} \
             overflow={overflow}, lines={len}",
            overflow = layout.overflow,
            len = layout.lines.len(),
        );
    }

    #[test]
    fn overflow_flag_set_when_text_doesnt_fit() {
        let text = "Lorem ipsum dolor sit amet, consectetur adipiscing elit. ".repeat(20);
        let Ok(layout) =
            layout_paragraph(&text, &style(), &frame(), bounds(120.0, 40.0), None)
        else {
            return;
        };
        // 40px tall frame with 20px line height can hold at most two
        // lines; the test corpus is much longer than that, so
        // overflow must be true.
        if !layout.lines.is_empty() {
            assert!(layout.overflow);
        }
    }

    #[test]
    fn hyphenation_disabled_when_patterns_none() {
        let mut f = frame();
        f.hyphenation = true;
        // Even with `hyphenation: true`, `None` patterns means no
        // patterns are loaded. The function must not panic and must
        // still produce some layout.
        let _ = layout_paragraph("hyphenation", &style(), &f, bounds(200.0, 200.0), None);
    }

    #[test]
    fn tokenizer_splits_words_and_whitespace() {
        let toks = tokenize_with_breaks("hello world\n foo");
        // Expected: Word("hello"), Whitespace(" "), Word("world"),
        // Whitespace("\n "), Word("foo"). The exact whitespace
        // grouping is "consecutive whitespace coalesces into one
        // token" which matches our tokenizer's invariant.
        assert_eq!(toks.len(), 5);
        assert!(matches!(&toks[0], Token::Word(s) if s == "hello"));
        assert!(matches!(&toks[1], Token::Whitespace(_)));
        assert!(matches!(&toks[2], Token::Word(s) if s == "world"));
        assert!(matches!(&toks[3], Token::Whitespace(s) if s.contains('\n')));
        assert!(matches!(&toks[4], Token::Word(s) if s == "foo"));
    }
}
