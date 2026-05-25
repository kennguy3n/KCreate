//! Phase 5 spot-color + overprint coverage.

use kcreate_core::color::{
    cmyk_to_srgb, total_ink_coverage_with_spots, Color, SpotColorDef, SpotColorLibrary,
};
use kcreate_core::document::DocumentGraph;
use kcreate_core::node::{
    Bounds, Node, NodeStyle, NodeType, PageLayout, PageOrientation, PageSize,
};
use kcreate_core::PAGE_LAYOUT_METADATA_KEY;
use kcreate_export::preflight::{
    run_preflight_with_spots, PreflightCheck, PreflightOptions, PreflightSeverity,
};
use uuid::Uuid;

fn make_spot(name: &str) -> Color {
    Color::Spot {
        name: name.into(),
        fallback_cmyk: (0.0, 1.0, 0.5, 0.0),
        tint: 1.0,
        alpha: 1.0,
    }
}

#[test]
fn spot_color_library_insert_and_lookup() {
    let mut lib = SpotColorLibrary::new();
    assert!(lib.is_empty());
    lib.insert(
        "P185",
        SpotColorDef {
            display_name: "Pantone 185 C".into(),
            fallback_cmyk: (0.0, 1.0, 0.84, 0.0),
            library_reference: Some("PANTONE 185 C".into()),
        },
    );
    assert_eq!(lib.len(), 1);
    let def = lib.get("P185").expect("registered");
    assert_eq!(def.display_name, "Pantone 185 C");
    assert!(lib.get("not-there").is_none());
}

#[test]
fn total_ink_coverage_resolves_against_library_first() {
    let mut lib = SpotColorLibrary::new();
    lib.insert(
        "P185",
        SpotColorDef {
            display_name: "P185".into(),
            // Library CMYK fallback differs from inline (so we can
            // observe which one was used). Sum = 1.84.
            fallback_cmyk: (0.0, 1.0, 0.84, 0.0),
            library_reference: None,
        },
    );
    let color = Color::Spot {
        name: "P185".into(),
        // Inline fallback sums to 2.5 — should NOT be the value
        // returned when the library has its own entry.
        fallback_cmyk: (0.5, 1.0, 1.0, 0.0),
        tint: 1.0,
        alpha: 1.0,
    };
    let ink = total_ink_coverage_with_spots(&color, &lib);
    assert!(
        (ink - 1.84).abs() < 1e-4,
        "library entry must win; got {ink}"
    );

    // Unregistered spot falls through to the colour's inline CMYK.
    let unknown = Color::Spot {
        name: "Unknown".into(),
        fallback_cmyk: (0.25, 0.25, 0.25, 0.25),
        tint: 1.0,
        alpha: 1.0,
    };
    let ink2 = total_ink_coverage_with_spots(&unknown, &SpotColorLibrary::new());
    assert!((ink2 - 1.0).abs() < 1e-4);
}

#[test]
fn spot_color_to_srgb_via_cmyk_fallback_matches_direct_conversion() {
    let color = make_spot("Pinky");
    let (r, g, b, a) = color.to_srgb();
    let (er, eg, eb) = cmyk_to_srgb(0.0, 1.0, 0.5, 0.0);
    assert!((r - er).abs() < 1e-3);
    assert!((g - eg).abs() < 1e-3);
    assert!((b - eb).abs() < 1e-3);
    assert!((a - 1.0).abs() < 1e-6);
}

#[test]
fn overprint_flag_round_trip_serialization() {
    let mut style = NodeStyle::default();
    assert!(!style.overprint, "default is knock-out");
    style.overprint = true;
    let json = serde_json::to_string(&style).expect("ser");
    let back: NodeStyle = serde_json::from_str(&json).expect("de");
    assert!(back.overprint);
}

#[test]
fn preflight_flags_unregistered_spot_color() {
    // Build a document with a single page + one rect whose color
    // override is an unregistered spot ink. With an empty
    // SpotColorLibrary, preflight should emit SpotColorMissing.
    let mut doc = DocumentGraph::new();
    let mut page = Node::new(NodeType::Page, "P");
    let layout = PageLayout::new(PageSize::A4, PageOrientation::Portrait);
    page.metadata.insert(
        PAGE_LAYOUT_METADATA_KEY.into(),
        serde_json::to_value(&layout).unwrap(),
    );
    let page_id = doc.insert_node(page).unwrap();

    let mut rect = Node::new(NodeType::VectorLayer, "R");
    rect.parent_id = Some(page_id);
    rect.bounds = Bounds {
        x: 10.0,
        y: 10.0,
        width: 50.0,
        height: 50.0,
    };
    rect.style.color_override = Some(make_spot("Mystery Ink"));
    let _ = doc.insert_node(rect).unwrap();

    let issues = run_preflight_with_spots(
        &doc,
        &[page_id],
        &PreflightOptions::default(),
        &SpotColorLibrary::new(),
    );
    let spot_issues: Vec<_> = issues
        .iter()
        .filter(|i| i.check == PreflightCheck::SpotColorMissing)
        .collect();
    assert_eq!(
        spot_issues.len(),
        1,
        "expected exactly one SpotColorMissing"
    );
    assert_eq!(spot_issues[0].severity, PreflightSeverity::Warning);
    assert!(spot_issues[0].message.contains("Mystery Ink"));
}

#[test]
fn preflight_passes_when_spot_is_registered() {
    let mut doc = DocumentGraph::new();
    let mut page = Node::new(NodeType::Page, "P");
    let layout = PageLayout::new(PageSize::A4, PageOrientation::Portrait);
    page.metadata.insert(
        PAGE_LAYOUT_METADATA_KEY.into(),
        serde_json::to_value(&layout).unwrap(),
    );
    let page_id = doc.insert_node(page).unwrap();

    let mut rect = Node::new(NodeType::VectorLayer, "R");
    rect.parent_id = Some(page_id);
    rect.bounds = Bounds {
        x: 0.0,
        y: 0.0,
        width: 50.0,
        height: 50.0,
    };
    rect.style.color_override = Some(make_spot("RegisteredSpot"));
    let _ = doc.insert_node(rect);

    let mut lib = SpotColorLibrary::new();
    lib.insert(
        "RegisteredSpot",
        SpotColorDef {
            display_name: "Registered".into(),
            fallback_cmyk: (0.1, 0.2, 0.3, 0.1),
            library_reference: None,
        },
    );

    let issues = run_preflight_with_spots(&doc, &[page_id], &PreflightOptions::default(), &lib);
    let none_missing = !issues
        .iter()
        .any(|i| i.check == PreflightCheck::SpotColorMissing);
    assert!(none_missing, "registered spot should produce no warning");
}

#[test]
#[allow(dead_code)]
fn use_uuid_to_silence_unused_import() {
    let _id = Uuid::new_v4();
}
