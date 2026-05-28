//! Text tokens — special inline markers that the shaper expands
//! before laying out a paragraph.
//!
//! Today the only supported token is **page-number**, used by the
//! Layout Studio to render `"Page 3 of 14"`-style headers/footers
//! on master pages. The token's representation in the text
//! content is the Unicode Private-Use sentinel `U+E100` followed
//! by a single ASCII format selector character (e.g. `1` for
//! Arabic, `i` for `RomanLower`). Tokens are resolved against a
//! [`PageContext`] at shape time.

use serde::{Deserialize, Serialize};

/// Sentinel Unicode code point that marks the start of a text
/// token. Chosen from the Private Use Area so it cannot collide
/// with any character a user might type.
pub const TOKEN_SENTINEL: char = '\u{E100}';

/// Supported page-number formats. The wire string matches the
/// trailing character after the sentinel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PageNumberFormat {
    /// `1, 2, 3, …` — the default.
    Arabic,
    /// `i, ii, iii, …`
    RomanLower,
    /// `I, II, III, …`
    RomanUpper,
    /// `a, b, c, …, z, aa, ab, …`
    AlphaLower,
    /// `A, B, C, …, Z, AA, AB, …`
    AlphaUpper,
}

impl PageNumberFormat {
    /// Single-character selector embedded in the text buffer.
    #[must_use]
    pub const fn selector(self) -> char {
        match self {
            Self::Arabic => '1',
            Self::RomanLower => 'i',
            Self::RomanUpper => 'I',
            Self::AlphaLower => 'a',
            Self::AlphaUpper => 'A',
        }
    }

    /// Inverse of [`Self::selector`].
    #[must_use]
    pub const fn from_selector(c: char) -> Option<Self> {
        Some(match c {
            '1' => Self::Arabic,
            'i' => Self::RomanLower,
            'I' => Self::RomanUpper,
            'a' => Self::AlphaLower,
            'A' => Self::AlphaUpper,
            _ => return None,
        })
    }
}

/// Encode a page-number token as a 2-char string suitable for
/// pasting into a text node.
#[must_use]
pub fn encode_page_number_token(format: PageNumberFormat) -> String {
    let mut s = String::with_capacity(2);
    s.push(TOKEN_SENTINEL);
    s.push(format.selector());
    s
}

/// Context for token expansion. The resolver walks document
/// pages in order and updates the section state, then passes a
/// [`PageContext`] per page to the shaper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageContext {
    /// The resolved 1-based page number after section restarts
    /// have been applied. `display_number` is what the user
    /// actually sees printed.
    pub display_number: u32,
    /// Total number of pages in the section (used by `"of N"`
    /// templates). When sections are not used, equals the total
    /// page count.
    pub section_total: u32,
    /// Optional textual prefix to prepend to the formatted
    /// number, e.g. `"A-"` → `"A-3"`.
    pub section_prefix: Option<String>,
}

impl PageContext {
    /// Build a context for the supplied 1-based page number with
    /// no prefix and a section total equal to the page number
    /// itself (caller can override).
    #[must_use]
    pub const fn simple(display_number: u32, total: u32) -> Self {
        Self {
            display_number,
            section_total: total,
            section_prefix: None,
        }
    }
}

/// Expand every page-number token in `text` against `ctx`.
/// Unknown selectors are left in place (they look like garbage,
/// which is the failure mode we want — silent corruption is
/// worse than a visible sentinel).
#[must_use]
pub fn expand_tokens(text: &str, ctx: &PageContext) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != TOKEN_SENTINEL {
            out.push(c);
            continue;
        }
        match chars.next() {
            None => out.push(TOKEN_SENTINEL),
            Some(sel) => match PageNumberFormat::from_selector(sel) {
                Some(fmt) => {
                    if let Some(prefix) = &ctx.section_prefix {
                        out.push_str(prefix);
                    }
                    out.push_str(&format_page_number(ctx.display_number, fmt));
                }
                None => {
                    out.push(TOKEN_SENTINEL);
                    out.push(sel);
                }
            },
        }
    }
    out
}

/// Format an integer page number using the supplied format.
#[must_use]
pub fn format_page_number(n: u32, fmt: PageNumberFormat) -> String {
    match fmt {
        PageNumberFormat::Arabic => n.to_string(),
        PageNumberFormat::RomanLower => roman_numeral(n, false),
        PageNumberFormat::RomanUpper => roman_numeral(n, true),
        PageNumberFormat::AlphaLower => alpha_numeral(n, false),
        PageNumberFormat::AlphaUpper => alpha_numeral(n, true),
    }
}

const ROMAN_TABLE: &[(u32, &str, &str)] = &[
    (1000, "M", "m"),
    (900, "CM", "cm"),
    (500, "D", "d"),
    (400, "CD", "cd"),
    (100, "C", "c"),
    (90, "XC", "xc"),
    (50, "L", "l"),
    (40, "XL", "xl"),
    (10, "X", "x"),
    (9, "IX", "ix"),
    (5, "V", "v"),
    (4, "IV", "iv"),
    (1, "I", "i"),
];

fn roman_numeral(mut n: u32, upper: bool) -> String {
    if n == 0 {
        return String::new();
    }
    let mut out = String::new();
    for (val, upper_s, lower_s) in ROMAN_TABLE {
        while n >= *val {
            n -= *val;
            out.push_str(if upper { upper_s } else { lower_s });
        }
    }
    out
}

fn alpha_numeral(mut n: u32, upper: bool) -> String {
    if n == 0 {
        return String::new();
    }
    let base = if upper { b'A' } else { b'a' };
    // Spreadsheet-style: 1 → a, 26 → z, 27 → aa, 28 → ab, …
    let mut buf = Vec::new();
    while n > 0 {
        let r = ((n - 1) % 26) as u8;
        buf.push(base + r);
        n = (n - 1) / 26;
    }
    buf.reverse();
    String::from_utf8(buf).expect("ascii alpha")
}

/// One page in the document, the way the section-numbering
/// resolver sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageDescriptor {
    /// The page id (mirrors `Node::id` for the page).
    pub id: uuid::Uuid,
    /// Optional section restart value pulled from the page's
    /// `PageLayout::section_start`. `Some(n)` resets the counter
    /// to `n` at this page; `None` continues from the previous.
    pub section_start: Option<u32>,
    /// Optional section prefix; persists across pages until a
    /// later page overrides it.
    pub section_prefix: Option<String>,
}

/// Walk a list of pages in document order and produce one
/// [`PageContext`] per page, applying section restarts /
/// prefixes. The output array is the same length as `pages`.
#[must_use]
pub fn resolve_page_contexts(pages: &[PageDescriptor]) -> Vec<PageContext> {
    let mut out = Vec::with_capacity(pages.len());
    let mut counter: u32 = 0;
    let mut current_prefix: Option<String> = None;
    // First pass: assign display numbers + carry-forward prefix.
    let mut display_numbers = Vec::with_capacity(pages.len());
    let mut prefixes: Vec<Option<String>> = Vec::with_capacity(pages.len());
    // Page 0 is always an implicit section boundary so the
    // section-totals pass below can treat each window as a
    // half-open `[start, next)` slice without special-casing it.
    let mut section_boundaries: Vec<usize> = Vec::new();
    for (idx, p) in pages.iter().enumerate() {
        if let Some(start) = p.section_start {
            counter = start;
            section_boundaries.push(idx);
        } else if idx == 0 {
            counter = 1;
            section_boundaries.push(0);
        } else {
            counter = counter.saturating_add(1);
        }
        if p.section_prefix.is_some() {
            current_prefix.clone_from(&p.section_prefix);
        }
        display_numbers.push(counter);
        prefixes.push(current_prefix.clone());
    }
    // If the first page had its own `section_start`, the boundary
    // was already pushed above; otherwise the `idx == 0` branch
    // covers it. Either way `section_boundaries` is non-empty.
    // Second pass: compute section totals.
    let total = pages.len() as u32;
    let mut section_totals = vec![total; pages.len()];
    if section_boundaries.len() > 1 {
        for window in section_boundaries.windows(2) {
            let start = window[0];
            let end = window[1];
            let len = (end - start) as u32;
            for entry in section_totals.iter_mut().take(end).skip(start) {
                *entry = len;
            }
        }
        // Tail section.
        let last_start = *section_boundaries.last().expect("non-empty");
        let len = (pages.len() - last_start) as u32;
        for entry in section_totals.iter_mut().skip(last_start) {
            *entry = len;
        }
    }
    for (i, _) in pages.iter().enumerate() {
        out.push(PageContext {
            display_number: display_numbers[i],
            section_total: section_totals[i],
            section_prefix: prefixes[i].clone(),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn pg(start: Option<u32>, prefix: Option<&str>) -> PageDescriptor {
        PageDescriptor {
            id: Uuid::new_v4(),
            section_start: start,
            section_prefix: prefix.map(String::from),
        }
    }

    #[test]
    fn arabic_formatting() {
        assert_eq!(format_page_number(1, PageNumberFormat::Arabic), "1");
        assert_eq!(format_page_number(42, PageNumberFormat::Arabic), "42");
    }

    #[test]
    fn roman_lower_formatting() {
        assert_eq!(format_page_number(1, PageNumberFormat::RomanLower), "i");
        assert_eq!(format_page_number(4, PageNumberFormat::RomanLower), "iv");
        assert_eq!(format_page_number(9, PageNumberFormat::RomanLower), "ix");
        assert_eq!(format_page_number(40, PageNumberFormat::RomanLower), "xl");
        assert_eq!(
            format_page_number(1987, PageNumberFormat::RomanLower),
            "mcmlxxxvii"
        );
    }

    #[test]
    fn roman_upper_formatting() {
        assert_eq!(format_page_number(1, PageNumberFormat::RomanUpper), "I");
        assert_eq!(format_page_number(4, PageNumberFormat::RomanUpper), "IV");
        assert_eq!(
            format_page_number(2024, PageNumberFormat::RomanUpper),
            "MMXXIV"
        );
    }

    #[test]
    fn alpha_lower_formatting() {
        assert_eq!(format_page_number(1, PageNumberFormat::AlphaLower), "a");
        assert_eq!(format_page_number(26, PageNumberFormat::AlphaLower), "z");
        assert_eq!(format_page_number(27, PageNumberFormat::AlphaLower), "aa");
        assert_eq!(format_page_number(28, PageNumberFormat::AlphaLower), "ab");
        assert_eq!(format_page_number(703, PageNumberFormat::AlphaLower), "aaa");
    }

    #[test]
    fn alpha_upper_formatting() {
        assert_eq!(format_page_number(1, PageNumberFormat::AlphaUpper), "A");
        assert_eq!(format_page_number(27, PageNumberFormat::AlphaUpper), "AA");
    }

    #[test]
    fn token_encode_decode_round_trip() {
        for fmt in [
            PageNumberFormat::Arabic,
            PageNumberFormat::RomanLower,
            PageNumberFormat::RomanUpper,
            PageNumberFormat::AlphaLower,
            PageNumberFormat::AlphaUpper,
        ] {
            let s = encode_page_number_token(fmt);
            let mut iter = s.chars();
            let sentinel = iter.next().unwrap();
            let sel = iter.next().unwrap();
            assert_eq!(sentinel, TOKEN_SENTINEL);
            assert_eq!(PageNumberFormat::from_selector(sel), Some(fmt));
        }
    }

    #[test]
    fn expand_tokens_substitutes_arabic() {
        let token = encode_page_number_token(PageNumberFormat::Arabic);
        let s = format!("Page {token} of 10");
        let ctx = PageContext::simple(3, 10);
        assert_eq!(expand_tokens(&s, &ctx), "Page 3 of 10");
    }

    #[test]
    fn expand_tokens_substitutes_roman_with_prefix() {
        let token = encode_page_number_token(PageNumberFormat::RomanLower);
        let s = format!("Chapter {token}");
        let mut ctx = PageContext::simple(4, 10);
        ctx.section_prefix = Some("§ ".into());
        assert_eq!(expand_tokens(&s, &ctx), "Chapter § iv");
    }

    #[test]
    fn expand_tokens_preserves_unknown_selector() {
        let s = format!("oops {TOKEN_SENTINEL}q yay");
        let ctx = PageContext::simple(1, 1);
        // `q` is not a valid selector; expansion must leave the
        // sentinel + `q` in place rather than silently dropping
        // text.
        let out = expand_tokens(&s, &ctx);
        assert!(out.contains(TOKEN_SENTINEL));
        assert!(out.contains('q'));
    }

    #[test]
    fn resolve_basic_sequence() {
        let pages = vec![pg(None, None), pg(None, None), pg(None, None)];
        let resolved = resolve_page_contexts(&pages);
        assert_eq!(
            resolved
                .iter()
                .map(|c| c.display_number)
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
    }

    #[test]
    fn resolve_section_restart_resets_counter() {
        let pages = vec![
            pg(None, None),
            pg(None, None),
            pg(Some(1), Some("A-")),
            pg(None, None),
            pg(None, None),
        ];
        let resolved = resolve_page_contexts(&pages);
        assert_eq!(
            resolved
                .iter()
                .map(|c| c.display_number)
                .collect::<Vec<_>>(),
            vec![1, 2, 1, 2, 3]
        );
        assert_eq!(resolved[2].section_prefix.as_deref(), Some("A-"));
        assert_eq!(resolved[4].section_prefix.as_deref(), Some("A-"));
    }

    #[test]
    fn resolve_section_prefix_persists_until_overridden() {
        let pages = vec![
            pg(Some(1), Some("A-")),
            pg(None, None),
            pg(Some(1), Some("B-")),
            pg(None, None),
        ];
        let resolved = resolve_page_contexts(&pages);
        assert_eq!(resolved[0].section_prefix.as_deref(), Some("A-"));
        assert_eq!(resolved[1].section_prefix.as_deref(), Some("A-"));
        assert_eq!(resolved[2].section_prefix.as_deref(), Some("B-"));
        assert_eq!(resolved[3].section_prefix.as_deref(), Some("B-"));
    }

    #[test]
    fn resolve_section_totals_match_section_length() {
        let pages = vec![
            pg(None, None), // section 1: 2 pages
            pg(None, None),
            pg(Some(1), None), // section 2: 3 pages
            pg(None, None),
            pg(None, None),
        ];
        let resolved = resolve_page_contexts(&pages);
        assert_eq!(resolved[0].section_total, 2);
        assert_eq!(resolved[1].section_total, 2);
        assert_eq!(resolved[2].section_total, 3);
        assert_eq!(resolved[3].section_total, 3);
        assert_eq!(resolved[4].section_total, 3);
    }
}
