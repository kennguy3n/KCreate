//! Phase 8 Block C: page-numbering tokens.
//!
//! Cross-crate test that walks
//! [`kcreate_text::tokens::resolve_page_contexts`] through a
//! multi-section document descriptor list and verifies the
//! expansion matches the section / prefix rules.

use kcreate_text::tokens::{
    encode_page_number_token, expand_tokens, format_page_number, resolve_page_contexts,
    PageContext, PageDescriptor, PageNumberFormat,
};
use uuid::Uuid;

fn page(start: Option<u32>, prefix: Option<&str>) -> PageDescriptor {
    PageDescriptor {
        id: Uuid::new_v4(),
        section_start: start,
        section_prefix: prefix.map(String::from),
    }
}

#[test]
fn arabic_roman_alpha_format_correctly() {
    assert_eq!(format_page_number(1, PageNumberFormat::Arabic), "1");
    assert_eq!(format_page_number(7, PageNumberFormat::RomanLower), "vii");
    assert_eq!(format_page_number(7, PageNumberFormat::RomanUpper), "VII");
    assert_eq!(format_page_number(1, PageNumberFormat::AlphaLower), "a");
    assert_eq!(format_page_number(26, PageNumberFormat::AlphaLower), "z");
    assert_eq!(format_page_number(27, PageNumberFormat::AlphaLower), "aa");
    assert_eq!(format_page_number(703, PageNumberFormat::AlphaLower), "aaa");
    assert_eq!(format_page_number(703, PageNumberFormat::AlphaUpper), "AAA");
}

#[test]
fn token_round_trip_via_expand() {
    let tok = encode_page_number_token(PageNumberFormat::RomanLower);
    let ctx = PageContext::simple(5, 5);
    let body = format!("Page {tok} of N");
    let expanded = expand_tokens(&body, &ctx);
    assert_eq!(expanded, "Page v of N");
}

#[test]
fn section_restart_and_prefix_propagate() {
    let pages = vec![
        page(None, None),           // front matter, page 1
        page(None, None),           // page 2
        page(Some(1), Some("Ch-")), // start chapter, page Ch-1
        page(None, None),           // Ch-2
        page(None, None),           // Ch-3
    ];
    let ctxs = resolve_page_contexts(&pages);
    assert_eq!(ctxs.len(), 5);
    assert_eq!(ctxs[0].display_number, 1);
    assert_eq!(ctxs[0].section_prefix, None);
    assert_eq!(ctxs[1].display_number, 2);
    assert_eq!(ctxs[2].display_number, 1);
    assert_eq!(ctxs[2].section_prefix.as_deref(), Some("Ch-"));
    assert_eq!(ctxs[3].display_number, 2);
    assert_eq!(ctxs[3].section_prefix.as_deref(), Some("Ch-"));
    assert_eq!(ctxs[4].display_number, 3);
    assert_eq!(ctxs[4].section_prefix.as_deref(), Some("Ch-"));
}

#[test]
fn expand_applies_section_prefix() {
    let pages = vec![page(Some(1), Some("A-"))];
    let ctxs = resolve_page_contexts(&pages);
    let tok = encode_page_number_token(PageNumberFormat::Arabic);
    let out = expand_tokens(&tok, &ctxs[0]);
    assert_eq!(out, "A-1");
}

#[test]
fn section_totals_track_section_lengths() {
    let pages = vec![
        page(None, None),    // sec 1, pg 1
        page(None, None),    // sec 1, pg 2
        page(Some(1), None), // sec 2, pg 1
        page(None, None),    // sec 2, pg 2
        page(None, None),    // sec 2, pg 3
    ];
    let ctxs = resolve_page_contexts(&pages);
    assert_eq!(ctxs[0].section_total, 2);
    assert_eq!(ctxs[1].section_total, 2);
    assert_eq!(ctxs[2].section_total, 3);
    assert_eq!(ctxs[3].section_total, 3);
    assert_eq!(ctxs[4].section_total, 3);
}

#[test]
fn unknown_selector_left_intact() {
    let s = format!("oops {} yay", kcreate_text::tokens::TOKEN_SENTINEL);
    let ctx = PageContext::simple(1, 1);
    let out = expand_tokens(&s, &ctx);
    assert!(out.contains(kcreate_text::tokens::TOKEN_SENTINEL));
}
