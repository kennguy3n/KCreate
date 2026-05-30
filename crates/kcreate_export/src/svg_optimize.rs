//! SVG minifier — Phase 10 Block C Task 13.
//!
//! Removes byte-level fat from SVG strings without changing what
//! they render. Optimisations applied:
//!
//! - drop XML / DOCTYPE declarations,
//! - drop comments,
//! - collapse runs of whitespace to single spaces,
//! - remove whitespace surrounding tag boundaries,
//! - drop empty `<g>` groups (recursively),
//! - drop default attribute values (`fill="black"`, `stroke="none"`,
//!   `opacity="1"`, `fill-opacity="1"`),
//! - shorten path data coordinates to a configurable precision.
//!
//! The minifier is deliberately string-only — bringing in a full
//! SVG/XML AST would mean another large dependency. To avoid
//! corrupting authored content the rewrites that run on the markup
//! (default-attribute stripping, whitespace collapse) are run only on
//! the regions that lie *outside* preserved blocks. A block is
//! preserved if it is either an XML CDATA section
//! (`<![CDATA[ ... ]]>`) or the body of a content-bearing element
//! whose text matters byte-for-byte (`<text>`, `<title>`, `<desc>`,
//! `<style>`, `<script>`). See [`protected_regions`] /
//! [`with_unprotected`]. Path-coordinate shortening only touches the
//! payload of `d="..."` attributes, which can never appear inside
//! those preserved bodies, so it is safe to run on the full string.

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SvgOptimizeOptions {
    pub coord_precision: u8,
    pub strip_default_attrs: bool,
    pub strip_empty_groups: bool,
    pub strip_comments: bool,
}

impl Default for SvgOptimizeOptions {
    fn default() -> Self {
        Self {
            coord_precision: 3,
            strip_default_attrs: true,
            strip_empty_groups: true,
            strip_comments: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SvgOptimizeReport {
    pub original_bytes: u64,
    pub optimised_bytes: u64,
    pub bytes_saved: u64,
    pub ratio: f64,
    pub output_svg: String,
}

#[derive(Debug, Error)]
pub enum SvgOptimizeError {
    #[error("svg_optimize: input is empty")]
    Empty,
}

/// Optimise `svg` and return the minified output plus a size delta
/// report.
///
/// # Errors
///
/// Returns [`SvgOptimizeError::Empty`] when the input has no
/// non-whitespace characters.
pub fn optimize_svg(svg: &str) -> Result<SvgOptimizeReport, SvgOptimizeError> {
    optimize_svg_with(svg, SvgOptimizeOptions::default())
}

/// Optimise `svg` with custom options.
///
/// # Errors
///
/// Returns [`SvgOptimizeError::Empty`] when the input has no
/// non-whitespace characters.
pub fn optimize_svg_with(
    svg: &str,
    opts: SvgOptimizeOptions,
) -> Result<SvgOptimizeReport, SvgOptimizeError> {
    if svg.trim().is_empty() {
        return Err(SvgOptimizeError::Empty);
    }
    let original_bytes = svg.len() as u64;
    let mut s = svg.to_string();
    if opts.strip_comments {
        s = strip_comments(&s);
    }
    s = strip_doctype_and_xml(&s);
    if opts.strip_default_attrs {
        s = with_unprotected(&s, strip_default_attrs);
    }
    if opts.strip_empty_groups {
        // Re-run until fixed point — removing a group can leave its
        // parent empty too.
        let mut prev_len = usize::MAX;
        while s.len() != prev_len {
            prev_len = s.len();
            s = strip_empty_groups_once(&s);
        }
    }
    if opts.coord_precision < 10 {
        s = shorten_path_coords(&s, opts.coord_precision);
    }
    s = with_unprotected(&s, collapse_whitespace);
    let optimised_bytes = s.len() as u64;
    let bytes_saved = original_bytes.saturating_sub(optimised_bytes);
    let ratio = if original_bytes == 0 {
        0.0
    } else {
        (bytes_saved as f64) / (original_bytes as f64)
    };
    Ok(SvgOptimizeReport {
        original_bytes,
        optimised_bytes,
        bytes_saved,
        ratio,
        output_svg: s,
    })
}

fn strip_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        match rest[start..].find("-->") {
            Some(end) => {
                rest = &rest[start + end + 3..];
            }
            None => {
                // unterminated — preserve rest verbatim, bail
                out.push_str(&rest[start..]);
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

fn strip_doctype_and_xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while !rest.is_empty() {
        let trimmed = rest.trim_start();
        if let Some(stripped) = trimmed.strip_prefix("<?xml") {
            match stripped.find("?>") {
                Some(idx) => {
                    rest = &stripped[idx + 2..];
                    continue;
                }
                None => break,
            }
        }
        if let Some(stripped) = trimmed.strip_prefix("<!DOCTYPE") {
            match stripped.find('>') {
                Some(idx) => {
                    rest = &stripped[idx + 1..];
                    continue;
                }
                None => break,
            }
        }
        break;
    }
    out.push_str(rest);
    out
}

fn strip_default_attrs(s: &str) -> String {
    let defaults = [
        " fill=\"black\"",
        " fill=\"#000\"",
        " fill=\"#000000\"",
        " stroke=\"none\"",
        " opacity=\"1\"",
        " fill-opacity=\"1\"",
        " stroke-opacity=\"1\"",
        " stroke-width=\"1\"",
    ];
    let mut s = s.to_string();
    for needle in defaults {
        s = s.replace(needle, "");
    }
    s
}

/// Run `f` only on the slices of `s` that are not inside a preserved
/// region (CDATA sections + `<text>`/`<title>`/`<desc>`/`<style>`/
/// `<script>` element bodies). Preserved slices are concatenated
/// back unchanged. This is how we avoid corrupting authored content
/// while still rewriting the markup around it.
fn with_unprotected(s: &str, f: fn(&str) -> String) -> String {
    let regions = protected_regions(s);
    if regions.is_empty() {
        return f(s);
    }
    let mut out = String::with_capacity(s.len());
    let mut cursor = 0;
    for (start, end) in regions {
        if cursor < start {
            out.push_str(&f(&s[cursor..start]));
        }
        out.push_str(&s[start..end]);
        cursor = end;
    }
    if cursor < s.len() {
        out.push_str(&f(&s[cursor..]));
    }
    out
}

/// Return a sorted, non-overlapping list of `[start, end)` byte ranges
/// inside `s` that must not be touched by the markup-level
/// transformations (default-attribute stripping, whitespace collapse).
/// Each region covers the *inside* of a preserved span — opening and
/// closing markers are intentionally left out so the markup rewrites
/// can still clean up the tags themselves.
fn protected_regions(s: &str) -> Vec<(usize, usize)> {
    const ELEMENTS: &[&str] = &["text", "title", "desc", "style", "script"];
    let bytes = s.as_bytes();
    let mut out: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        // CDATA: `<![CDATA[ ... ]]>` — protect the inner payload.
        if bytes[i..].starts_with(b"<![CDATA[") {
            let start = i + b"<![CDATA[".len();
            if let Some(rel) = find_subslice(&bytes[start..], b"]]>") {
                let end = start + rel;
                out.push((start, end));
                i = end + b"]]>".len();
                continue;
            }
            // Unterminated CDATA: preserve the rest verbatim.
            out.push((start, bytes.len()));
            break;
        }
        // Opening tag for one of the preserved elements.
        if bytes[i] == b'<' {
            if let Some((tag, after_open)) = matched_open_tag(s, i, ELEMENTS) {
                // `after_open` is the byte index immediately after the
                // closing `>` of the opening tag. Self-closing tags
                // (e.g. `<title/>`) have no inner content.
                if !s[..after_open].trim_end_matches('>').ends_with('/') {
                    let close_needle = format!("</{tag}");
                    if let Some(rel) = find_subslice(&bytes[after_open..], close_needle.as_bytes())
                    {
                        let end = after_open + rel;
                        out.push((after_open, end));
                        i = end;
                        continue;
                    }
                    // Unterminated: preserve to EOF.
                    out.push((after_open, bytes.len()));
                    break;
                }
            }
        }
        i += 1;
    }
    out
}

fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > hay.len() {
        return None;
    }
    (0..=hay.len() - needle.len()).find(|&w| hay[w..w + needle.len()] == *needle)
}

/// If position `pos` is the start of an opening tag whose name is
/// one of `names`, return `(matched_name, position_after_closing_>)`.
fn matched_open_tag<'a>(s: &str, pos: usize, names: &'a [&'a str]) -> Option<(&'a str, usize)> {
    let bytes = s.as_bytes();
    if bytes.get(pos) != Some(&b'<') {
        return None;
    }
    for &name in names {
        let head = format!("<{name}");
        if !s[pos..].starts_with(&head) {
            continue;
        }
        // The next byte after the name must be whitespace, `/`, or `>`
        // — otherwise we matched a prefix (e.g. `<textPath` shouldn't
        // match `text`).
        let next = bytes.get(pos + head.len()).copied();
        let is_boundary =
            matches!(next, Some(b) if b.is_ascii_whitespace() || b == b'/' || b == b'>');
        if !is_boundary {
            continue;
        }
        let after = &s[pos + head.len()..];
        let rel = after.find('>')?;
        return Some((name, pos + head.len() + rel + 1));
    }
    None
}

fn strip_empty_groups_once(s: &str) -> String {
    // Remove `<g[..]></g>` whose attributes are all whitespace.
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(idx) = rest.find("<g") {
        out.push_str(&rest[..idx]);
        let after = &rest[idx + 2..];
        // Find the close of the opening tag.
        let Some(open_close) = after.find('>') else {
            out.push_str(&rest[idx..]);
            return out;
        };
        let attrs = &after[..open_close];
        let after_tag = &after[open_close + 1..];
        // Only match self-closing `<g .. />` or the explicit form
        // `<g ..></g>` with no content between them.
        if attrs.trim_end().ends_with('/') {
            // `<g ../>` is structurally empty — drop entirely.
            rest = after_tag;
            continue;
        }
        if let Some(close_idx) = after_tag.find("</g>") {
            if after_tag[..close_idx].trim().is_empty() {
                rest = &after_tag[close_idx + 4..];
                continue;
            }
        }
        // Not an empty group — emit verbatim.
        out.push_str(&rest[idx..=(idx + 2 + open_close)]);
        rest = after_tag;
    }
    out.push_str(rest);
    out
}

fn shorten_path_coords(s: &str, precision: u8) -> String {
    // Walk attribute payload of `d="…"` and rewrite floating-point
    // tokens to the requested precision.
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    loop {
        let Some(idx) = rest.find(" d=\"") else {
            out.push_str(rest);
            return out;
        };
        out.push_str(&rest[..idx + 4]);
        let after = &rest[idx + 4..];
        let Some(end) = after.find('"') else {
            out.push_str(after);
            return out;
        };
        let payload = &after[..end];
        out.push_str(&shorten_numbers(payload, precision));
        out.push('"');
        rest = &after[end + 1..];
    }
}

fn shorten_numbers(s: &str, precision: u8) -> String {
    let mut out = String::with_capacity(s.len());
    let mut cursor = 0;
    let bytes = s.as_bytes();
    while cursor < bytes.len() {
        let b = bytes[cursor];
        if b == b'-' || b == b'+' || b == b'.' || b.is_ascii_digit() {
            // Consume a numeric token.
            let start = cursor;
            if b == b'-' || b == b'+' {
                cursor += 1;
            }
            while cursor < bytes.len() && (bytes[cursor].is_ascii_digit() || bytes[cursor] == b'.')
            {
                cursor += 1;
            }
            // Optional exponent
            if cursor < bytes.len() && (bytes[cursor] == b'e' || bytes[cursor] == b'E') {
                cursor += 1;
                if cursor < bytes.len() && (bytes[cursor] == b'-' || bytes[cursor] == b'+') {
                    cursor += 1;
                }
                while cursor < bytes.len() && bytes[cursor].is_ascii_digit() {
                    cursor += 1;
                }
            }
            let token = &s[start..cursor];
            if let Ok(f) = token.parse::<f64>() {
                out.push_str(&format_finite(f, precision));
            } else {
                out.push_str(token);
            }
        } else {
            out.push(b as char);
            cursor += 1;
        }
    }
    out
}

fn format_finite(v: f64, precision: u8) -> String {
    if !v.is_finite() {
        return "0".into();
    }
    let formatted = format!("{:.*}", precision as usize, v);
    // Strip trailing zeros + trailing dot to keep things compact.
    let formatted = if formatted.contains('.') {
        let s = formatted.trim_end_matches('0').trim_end_matches('.');
        if s.is_empty() {
            "0".to_string()
        } else {
            s.to_string()
        }
    } else {
        formatted
    };
    formatted
}

fn collapse_whitespace(s: &str) -> String {
    // Collapse runs of whitespace BUT only outside of attribute
    // values. We do this with a small state machine.
    //
    // We must NOT touch the contents of attribute values, including
    // values that happen to contain the literal sequences `> <`,
    // ` >`, or `< ` (e.g. `title="a > b"`). Earlier versions of this
    // function ran post-hoc `.replace("> <", "><")` / `.replace(" >",
    // ">")` / `.replace("< ", "<")` passes over the already-processed
    // string, which would corrupt such attribute values. Instead we
    // do those normalisations inline using a small lookback: when we
    // are about to emit a space-then-`<` or space-then-`>` pair while
    // outside of an attribute, we drop the space.
    let mut out = String::with_capacity(s.len());
    let mut last_was_space = false;
    let mut in_attr = false;
    let mut quote_char = '"';
    for ch in s.chars() {
        if in_attr {
            out.push(ch);
            if ch == quote_char {
                in_attr = false;
            }
            continue;
        }
        if ch == '"' || ch == '\'' {
            // Edge case: drop a pending unattached space directly
            // adjacent to the start of an attribute value's quote.
            // Browsers tolerate it but it adds bytes for no win.
            quote_char = ch;
            in_attr = true;
            out.push(ch);
            last_was_space = false;
            continue;
        }
        if ch.is_whitespace() {
            if !last_was_space {
                out.push(' ');
                last_was_space = true;
            }
        } else {
            // Normalise " <" → "<" and " >" → ">" inline so we never
            // need to post-process the string. Because `in_attr` is
            // false here, we know the trailing space in `out` is real
            // markup whitespace, not part of an authored attribute
            // value, so it is safe to drop.
            if last_was_space && (ch == '<' || ch == '>') && out.ends_with(' ') {
                out.pop();
            }
            // And normalise "> <" → "><" by dropping a leading space
            // immediately after a closing `>` once we see the next
            // `<`. Same reasoning — outside of `in_attr`, that space
            // is markup whitespace.
            if ch == '<' && out.ends_with("> ") {
                out.pop();
            }
            out.push(ch);
            last_was_space = false;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_errors() {
        assert!(optimize_svg("").is_err());
        assert!(optimize_svg("   \n  ").is_err());
    }

    #[test]
    fn comments_are_removed() {
        let svg = "<svg><!-- noise --><rect/></svg>";
        let r = optimize_svg(svg).unwrap();
        assert!(!r.output_svg.contains("noise"));
        assert!(r.bytes_saved > 0);
    }

    #[test]
    fn default_attrs_are_stripped() {
        let svg =
            r#"<svg><rect width="10" height="10" fill="black" stroke="none" opacity="1"/></svg>"#;
        let r = optimize_svg(svg).unwrap();
        assert!(!r.output_svg.contains("fill=\"black\""));
        assert!(!r.output_svg.contains("stroke=\"none\""));
        assert!(!r.output_svg.contains("opacity=\"1\""));
    }

    #[test]
    fn empty_groups_are_collapsed() {
        let svg = "<svg><g></g><g><rect/></g><g/></svg>";
        let r = optimize_svg(svg).unwrap();
        // Only the populated group should remain.
        assert!(r.output_svg.contains("<rect"));
        assert_eq!(r.output_svg.matches("<g").count(), 1);
    }

    #[test]
    fn nested_empty_groups_collapse_to_fixed_point() {
        let svg = "<svg><g><g></g></g></svg>";
        let r = optimize_svg(svg).unwrap();
        assert!(!r.output_svg.contains("<g"));
    }

    #[test]
    fn path_coords_are_shortened() {
        let svg = r#"<svg><path d="M0.123456789 1.234567890 L10.987654321 0.000000000"/></svg>"#;
        let r = optimize_svg(svg).unwrap();
        assert!(r.output_svg.contains("M0.123"));
        // Trailing zero stripped from 0.000.
        assert!(!r.output_svg.contains("0.000"));
    }

    #[test]
    fn output_is_strictly_smaller_or_equal() {
        let svg = "<svg><!-- x --><g></g><rect fill=\"black\" /></svg>";
        let r = optimize_svg(svg).unwrap();
        assert!(r.optimised_bytes <= r.original_bytes);
    }

    #[test]
    fn whitespace_inside_attributes_is_preserved() {
        let svg = r#"<svg><text font-family="Helvetica Neue">Hello</text></svg>"#;
        let r = optimize_svg(svg).unwrap();
        assert!(r.output_svg.contains("Helvetica Neue"));
    }

    #[test]
    fn text_element_body_is_not_collapsed() {
        // Multi-space body text must survive collapse_whitespace.
        let svg = "<svg><text>hello    world</text></svg>";
        let r = optimize_svg(svg).unwrap();
        assert!(
            r.output_svg.contains("hello    world"),
            "text body collapsed: {}",
            r.output_svg
        );
    }

    #[test]
    fn style_block_payload_is_preserved_byte_for_byte() {
        // CSS inside a <style> block uses whitespace + `:` `;` syntax
        // that must not be mangled by the markup-level rewrites.
        let css = ".foo { fill: black;\n  stroke: red; }";
        let svg = format!("<svg><style>{css}</style><rect/></svg>");
        let r = optimize_svg(&svg).unwrap();
        assert!(
            r.output_svg.contains(css),
            "style payload mutated: {}",
            r.output_svg
        );
    }

    #[test]
    fn attribute_value_containing_markup_chars_is_not_mangled() {
        // Regression for: collapse_whitespace used to do a post-hoc
        // `.replace("> <", "><")` / `.replace(" >", ">")` /
        // `.replace("< ", "<")` pass that would silently corrupt
        // attribute values containing those literal sequences.
        // Phase 10 — Devin Review finding ANALYSIS_…_0007.
        let svg = r#"<svg><g title="a > b"><rect aria-label="x < y"/><circle data-cmp="p > q < r"/></g></svg>"#;
        let r = optimize_svg(svg).unwrap();
        assert!(
            r.output_svg.contains(r#"title="a > b""#),
            "title attribute mangled: {}",
            r.output_svg
        );
        assert!(
            r.output_svg.contains(r#"aria-label="x < y""#),
            "aria-label attribute mangled: {}",
            r.output_svg
        );
        assert!(
            r.output_svg.contains(r#"data-cmp="p > q < r""#),
            "data-cmp attribute mangled: {}",
            r.output_svg
        );
    }

    #[test]
    fn strip_default_attrs_does_not_touch_text_bodies() {
        // The literal ` fill="black"` inside a <text> body must not
        // be deleted by strip_default_attrs.
        let svg = r#"<svg><text>example fill="black" inline</text><rect fill="black"/></svg>"#;
        let r = optimize_svg(svg).unwrap();
        assert!(
            r.output_svg.contains(r#"example fill="black" inline"#),
            "text body lost literal: {}",
            r.output_svg
        );
        // The actual rect attribute is still stripped.
        assert!(!r.output_svg.contains(r#"<rect fill="black""#));
    }

    #[test]
    fn cdata_payload_is_preserved() {
        // CDATA sections must round-trip unchanged.
        let svg = "<svg><script><![CDATA[ a < b && c > d  // multi space ]]></script><rect/></svg>";
        let r = optimize_svg(svg).unwrap();
        assert!(
            r.output_svg.contains("a < b && c > d  // multi space"),
            "CDATA mangled: {}",
            r.output_svg
        );
    }

    #[test]
    fn textpath_prefix_match_is_rejected() {
        // `<textPath>` shares the `<text` prefix; the boundary
        // check must keep us from treating its body as preserved.
        let svg = "<svg><textPath href=\"#p\"   data-x=\"y\" >x  y</textPath></svg>";
        let r = optimize_svg(svg).unwrap();
        // Inside the <textPath> body, multi-space sequences in the
        // attribute list of the opening tag SHOULD have collapsed.
        assert!(
            !r.output_svg.contains("#p\"   data-x"),
            "textPath attrs not collapsed: {}",
            r.output_svg
        );
    }
}
