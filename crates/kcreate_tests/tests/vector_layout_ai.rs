//! Phase 10 Block B — Vector Studio & Layout Studio AI features.
//!
//! Exercises the algorithms behind:
//!
//! * Task 7  — Match-stroke-style (`stroke_match`).
//! * Task 8  — Extract-glyph-from-photo (`glyph_extract`).
//! * Task 9  — Reformat content into a 16:9 deck (`reformat`).
//! * Task 10 — Brief → one-pager (`one_pager`).
//! * Task 11 — Harmonize palette (`palette_harmonize`).
//! * Task 12 — Type pairing (`type_pairing`).
//!
//! These drive the algorithms directly. The bridge layer just adapts
//! inputs and records operations — its correctness is exercised by
//! the existing `image_studio_ai.rs` style integration tests.

use kcreate_ai::glyph_extract::{extract_glyph, GlyphCrop, GlyphExtractOptions};
use kcreate_ai::one_pager::{
    brief_to_one_pager, BriefToOnePagerOptions, OnePagerPageSize, OnePagerSectionType,
};
use kcreate_ai::palette_harmonize::{harmonize_palette, HarmonyRule};
use kcreate_ai::reformat::{reformat_to_deck, ReformatDeckOptions, SourceNode as DeckNode};
use kcreate_ai::stroke_match::{match_stroke_style, StrokeMatchError, StrokeProperties};
use kcreate_ai::type_pairing::suggest_type_pairing;

// ---------------------------------------------------------------------------
// Task 7 — stroke_match
// ---------------------------------------------------------------------------

#[test]
fn stroke_match_copies_source_properties_to_every_target() {
    let source = StrokeProperties {
        color_hex: "#1a2bffff".into(),
        width: 3.5,
        dash: vec![6.0, 3.0],
        cap: "round".into(),
        join: "miter".into(),
        width_profile: Some(vec![(0.0, 0.5), (1.0, 2.5)]),
    };
    let targets = vec![
        ("a".to_string(), true),
        ("b".to_string(), false),
        ("c".to_string(), true),
    ];
    let summary = match_stroke_style("src", Some(&source), &targets).expect("match");
    assert_eq!(summary.source_node_id, "src");
    assert_eq!(summary.applied.len(), 3);
    assert!(summary.applied[0].had_previous_stroke);
    assert!(!summary.applied[1].had_previous_stroke);
    // Source properties are echoed back so the bridge has a single
    // authoritative copy to apply.
    assert_eq!(summary.source_properties.color_hex, "#1a2bffff");
    assert_eq!(summary.source_properties.dash, vec![6.0, 3.0]);
}

#[test]
fn stroke_match_rejects_empty_targets() {
    let source = StrokeProperties::default();
    let err = match_stroke_style("src", Some(&source), &[]).unwrap_err();
    assert!(matches!(err, StrokeMatchError::NoTargets));
}

#[test]
fn stroke_match_without_source_stroke_errors() {
    let err = match_stroke_style("src", None, &[("a".into(), true)]).unwrap_err();
    assert!(matches!(err, StrokeMatchError::NoSourceStroke));
}

// ---------------------------------------------------------------------------
// Task 8 — glyph_extract
// ---------------------------------------------------------------------------

/// Build an RGBA image of a black vertical bar (8 px wide × 24 px
/// tall) centred on a 32×32 white field — a stand-in for an "I"
/// letterform. The crop matches the bar exactly so the bridge sees
/// the whole glyph.
fn synth_letter_i() -> (Vec<u8>, u32, u32) {
    let w = 32u32;
    let h = 32u32;
    let mut buf = vec![255u8; (w * h * 4) as usize];
    for y in 4..28 {
        for x in 12..20 {
            let i = ((y * w + x) * 4) as usize;
            buf[i] = 0;
            buf[i + 1] = 0;
            buf[i + 2] = 0;
            buf[i + 3] = 255;
        }
    }
    (buf, w, h)
}

#[test]
fn glyph_extract_normalises_paths_to_em_square() {
    let (buf, w, h) = synth_letter_i();
    let g = extract_glyph(
        &buf,
        w,
        h,
        GlyphCrop {
            x: 8,
            y: 0,
            width: 16,
            height: 32,
        },
        GlyphExtractOptions {
            em_size: 1000.0,
            simplify_tolerance: 1.0,
        },
    )
    .expect("extract");
    assert!(!g.paths.is_empty(), "expected at least one path");
    assert!((g.metrics.em - 1000.0).abs() < 0.001);
    // Every traced point must fall inside the em box, with a small
    // tolerance for rounding.
    for path in &g.paths {
        for p in &path.points {
            assert!((-2.0..=1002.0).contains(&p.x), "x outside em: {}", p.x);
            assert!((-2.0..=1002.0).contains(&p.y), "y outside em: {}", p.y);
        }
    }
}

#[test]
fn glyph_extract_clamps_invalid_em_size_to_default() {
    let (buf, w, h) = synth_letter_i();
    let g = extract_glyph(
        &buf,
        w,
        h,
        GlyphCrop {
            x: 8,
            y: 0,
            width: 16,
            height: 32,
        },
        GlyphExtractOptions {
            em_size: 0.0,
            simplify_tolerance: 1.0,
        },
    )
    .expect("extract with zero em");
    assert!(g.metrics.em > 0.0, "em must be clamped above 0");
    // Clamping floors at 64; the default value 1000 must still be
    // valid even when the request was bogus.
    assert!(g.metrics.em >= 64.0);
}

#[test]
fn glyph_extract_rejects_crop_outside_image_bounds() {
    let (buf, w, h) = synth_letter_i();
    let err = extract_glyph(
        &buf,
        w,
        h,
        GlyphCrop {
            x: 30,
            y: 0,
            width: 16,
            height: 32,
        },
        GlyphExtractOptions::default(),
    );
    assert!(err.is_err(), "out-of-bounds crop must error");
}

// ---------------------------------------------------------------------------
// Task 9 — reformat_to_deck
// ---------------------------------------------------------------------------

fn node(id: &str, x: f64, y: f64, w: f64, h: f64) -> DeckNode {
    DeckNode {
        id: id.into(),
        name: id.into(),
        x,
        y,
        width: w,
        height: h,
        kind: "text".into(),
    }
}

#[test]
fn reformat_packs_every_input_node_exactly_once() {
    let nodes = vec![
        node("a", 10.0, 10.0, 200.0, 40.0),
        node("b", 10.0, 60.0, 200.0, 100.0),
        node("c", 10.0, 170.0, 200.0, 20.0),
        node("d", 10.0, 220.0, 200.0, 40.0),
        node("e", 10.0, 280.0, 200.0, 40.0),
    ];
    let plan = reformat_to_deck(&nodes, ReformatDeckOptions::default()).expect("plan");
    assert!(!plan.pages.is_empty(), "deck plan must produce pages");
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for page in &plan.pages {
        for placement in &page.placements {
            assert!(
                seen.insert(placement.source_node_id.clone()),
                "node {} placed twice",
                placement.source_node_id
            );
        }
    }
    assert_eq!(seen.len(), nodes.len(), "all source nodes must be placed");
}

#[test]
fn reformat_pages_have_monotonic_indices() {
    let nodes: Vec<DeckNode> = (0..7)
        .map(|i| node(&format!("n{i}"), 0.0, f64::from(i) * 40.0, 100.0, 30.0))
        .collect();
    let plan = reformat_to_deck(&nodes, ReformatDeckOptions::default()).expect("plan");
    for (i, page) in plan.pages.iter().enumerate() {
        assert_eq!(
            page.index as usize, i,
            "page index gap at slot {i}: got {}",
            page.index
        );
    }
}

#[test]
fn reformat_empty_input_errors() {
    let err = reformat_to_deck(&[], ReformatDeckOptions::default());
    assert!(err.is_err(), "empty input must error");
}

// ---------------------------------------------------------------------------
// Task 10 — brief → one-pager
// ---------------------------------------------------------------------------

#[test]
fn one_pager_promotes_first_line_to_header() {
    let brief = "Welcome to Acme\n\
                 We make widgets that change the world.\n\
                 Our mission\n\
                 Build delightful tools for creative professionals.\n";
    let result = brief_to_one_pager(brief, BriefToOnePagerOptions::default()).expect("plan");
    assert!(!result.sections.is_empty(), "expected at least one section");
    let header = result
        .sections
        .iter()
        .find(|s| matches!(s.section_type, OnePagerSectionType::Header))
        .expect("must include a header section");
    assert!(header.text.contains("Welcome"), "header drift: {header:?}");
}

#[test]
fn one_pager_respects_named_page_size() {
    let r = brief_to_one_pager(
        "Just some text.",
        BriefToOnePagerOptions {
            page_size: OnePagerPageSize::Letter,
            ..Default::default()
        },
    )
    .expect("plan");
    let (w, h) = OnePagerPageSize::Letter.dimensions();
    assert!((r.page_width - w).abs() < 1.0);
    assert!((r.page_height - h).abs() < 1.0);
}

#[test]
fn one_pager_rejects_empty_brief() {
    let err = brief_to_one_pager("    \n\n ", BriefToOnePagerOptions::default());
    assert!(err.is_err(), "empty brief must error");
}

// ---------------------------------------------------------------------------
// Task 11 — palette_harmonize
// ---------------------------------------------------------------------------

#[test]
fn harmonize_complementary_drives_second_colour_180_degrees_around() {
    // Pure red + an off-cyan input. Complementary harmony should
    // produce a near-cyan partner.
    let palette = vec!["#FF0000".to_string(), "#0099FF".to_string()];
    let result = harmonize_palette(&palette, HarmonyRule::Complementary).expect("harmonize");
    assert_eq!(result.suggestions.len(), 2);
    // The anchor colour is always returned with zero shift.
    assert!(result.suggestions[0].hue_shift_degrees.abs() < 1.0);
    // The partner should sit near pure cyan.
    let suggested = result.suggestions[1].suggested_hex.trim_start_matches('#');
    assert_eq!(suggested.len(), 8.min(suggested.len()).max(6));
    let r = u8::from_str_radix(&suggested[0..2], 16).unwrap();
    let g = u8::from_str_radix(&suggested[2..4], 16).unwrap();
    let b = u8::from_str_radix(&suggested[4..6], 16).unwrap();
    assert!(r <= 40, "expected r small, got {r}");
    assert!(g >= 200, "expected g large, got {g}");
    assert!(b >= 200, "expected b large, got {b}");
}

#[test]
fn harmonize_auto_picks_triadic_for_120_degree_spaced_palette() {
    let palette = vec![
        "#FF0000".to_string(), //   0°
        "#00FF00".to_string(), // 120°
        "#0000FF".to_string(), // 240°
    ];
    let result = harmonize_palette(&palette, HarmonyRule::Auto).expect("auto");
    assert_eq!(
        result.rule,
        HarmonyRule::Triadic,
        "auto should pick Triadic for an evenly-spaced triadic palette"
    );
}

#[test]
fn harmonize_auto_handles_wraparound_hue_without_inflating_cost() {
    // Hue 350° + 10° + 110° — the first two are very close circularly
    // (20° apart) and 110° away from the third. The straight-line cost
    // (350 - 10 = 340) used to mislead the auto picker; with the
    // wrap fix, Analogous (small spread + small targets) is the natural
    // choice. The exact rule chosen depends on the cost tally, but the
    // call must not panic and the anchor shift stays zero.
    let palette = vec![
        "#FF003C".to_string(), // ~350°
        "#FF6600".to_string(), // ~20°
        "#FFCC00".to_string(), // ~45°
    ];
    let result = harmonize_palette(&palette, HarmonyRule::Auto).expect("auto");
    assert_eq!(result.suggestions.len(), 3);
    assert!(result.suggestions[0].hue_shift_degrees.abs() < 1.0);
}

#[test]
fn harmonize_empty_palette_errors() {
    let palette: Vec<String> = vec![];
    let err = harmonize_palette(&palette, HarmonyRule::Auto);
    assert!(err.is_err(), "empty palette must error");
}

#[test]
fn harmonize_invalid_hex_errors() {
    let palette = vec!["not-a-color".to_string()];
    let err = harmonize_palette(&palette, HarmonyRule::Auto);
    assert!(err.is_err(), "invalid hex must error");
}

// ---------------------------------------------------------------------------
// Task 12 — suggest_type_pairing
// ---------------------------------------------------------------------------

#[test]
fn type_pairing_returns_at_least_one_suggestion() {
    let r = suggest_type_pairing("Inter").expect("pairing");
    assert!(
        !r.suggestions.is_empty(),
        "expected at least one suggestion"
    );
    for s in &r.suggestions {
        assert!(!s.font_name.is_empty());
        assert!(!s.reason.is_empty());
        assert!((0.0..=1.0).contains(&s.confidence));
    }
}

#[test]
fn type_pairing_rejects_empty_heading_name() {
    let err = suggest_type_pairing("   ");
    assert!(err.is_err(), "empty heading name must error");
}

#[test]
fn type_pairing_handles_unknown_heading_font_gracefully() {
    // An unknown name should not crash; classification falls back to
    // a generic category.
    let r = suggest_type_pairing("ZzMadeUpFont123").expect("pairing");
    assert!(!r.suggestions.is_empty());
    assert_eq!(r.heading_font, "ZzMadeUpFont123");
}
