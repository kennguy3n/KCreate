//! Phase 5 spot-color + overprint coverage.

use kcreate_core::color::{
    cmyk_to_srgb, total_ink_coverage_with_spots, Color, SpotCatalogError, SpotColorDef,
    SpotColorLibrary,
};
use kcreate_core::document::DocumentGraph;
use kcreate_core::node::{
    Bounds, FillStyle, Node, NodeStyle, NodeType, PageLayout, PageOrientation, PageSize, RgbaColor,
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

// ---------------------------------------------------------------------------
// Tasks 5-8: Pantone JSON catalog loading + OverprintTable + Trapping +
// TotalInkCoverage threshold integration.
// ---------------------------------------------------------------------------

fn make_page_with_a4(doc: &mut DocumentGraph) -> Uuid {
    let mut page = Node::new(NodeType::Page, "P");
    let layout = PageLayout::new(PageSize::A4, PageOrientation::Portrait);
    page.metadata.insert(
        PAGE_LAYOUT_METADATA_KEY.into(),
        serde_json::to_value(&layout).unwrap(),
    );
    doc.insert_node(page).unwrap()
}

fn rect_node(parent_id: Uuid, name: &str, x: f64, y: f64, w: f64, h: f64) -> Node {
    let mut node = Node::new(NodeType::VectorLayer, name);
    node.parent_id = Some(parent_id);
    node.bounds = Bounds {
        x,
        y,
        width: w,
        height: h,
    };
    node
}

fn cmyk_color(c: f32, m: f32, y: f32, k: f32) -> Color {
    Color::Cmyk { c, m, y, k, a: 1.0 }
}

fn solid_rgb(rect: &mut Node, rgba: RgbaColor) {
    rect.style.fill = FillStyle::Solid(rgba);
    rect.style.color_override = None;
}

#[test]
fn pantone_catalog_wrapped_form_parses() {
    let raw = r#"{
        "name": "Pantone Solid Coated",
        "entries": [
            { "id": "PANTONE 185 C", "display_name": "Pantone 185 C", "cmyk": [0.0, 1.0, 0.84, 0.0] },
            { "id": "PANTONE Reflex Blue C", "display_name": "Reflex Blue", "cmyk": [1.0, 0.72, 0.0, 0.06] }
        ]
    }"#;
    let lib = SpotColorLibrary::from_json_catalog(raw).expect("valid wrapped catalogue");
    assert_eq!(lib.len(), 2);
    let p185 = lib.get("PANTONE 185 C").expect("185 present");
    assert_eq!(p185.display_name, "Pantone 185 C");
    let (c, m, y, k) = p185.fallback_cmyk;
    assert!((c - 0.0).abs() < 1e-6 && (m - 1.0).abs() < 1e-6);
    assert!((y - 0.84).abs() < 1e-6 && (k - 0.0).abs() < 1e-6);
    assert_eq!(
        p185.library_reference.as_deref(),
        Some("PANTONE 185 C"),
        "library_reference defaults to id"
    );
}

#[test]
fn pantone_catalog_bare_map_form_parses() {
    let raw = r#"{
        "P-Warm-Red": { "display_name": "Pantone Warm Red", "cmyk": [0.0, 0.78, 0.78, 0.0] },
        "P-Sky":      [0.50, 0.10, 0.00, 0.00]
    }"#;
    let lib = SpotColorLibrary::from_json_catalog(raw).expect("valid bare map");
    assert_eq!(lib.len(), 2);
    let warm = lib.get("P-Warm-Red").expect("warm red present");
    assert_eq!(warm.display_name, "Pantone Warm Red");
    let sky = lib.get("P-Sky").expect("sky present");
    assert_eq!(
        sky.display_name, "P-Sky",
        "bare CMYK array uses the key as display name"
    );
}

#[test]
fn pantone_catalog_skips_corrupted_entries_but_keeps_others() {
    let raw = r#"{
        "entries": [
            { "id": "GOOD", "cmyk": [0.0, 0.5, 0.5, 0.0] },
            { "id": "WRONG_LEN", "cmyk": [0.0, 0.5, 0.5] },
            { "id": "NON_FINITE", "cmyk": [0.0, 0.5, 0.5, null] },
            { "id": "NO_CMYK", "display_name": "missing" }
        ]
    }"#;
    let lib = SpotColorLibrary::from_json_catalog(raw).expect("top-level still parses");
    assert_eq!(lib.len(), 1, "only GOOD should survive");
    assert!(lib.get("GOOD").is_some());
    assert!(lib.get("WRONG_LEN").is_none());
    assert!(lib.get("NON_FINITE").is_none());
    assert!(lib.get("NO_CMYK").is_none());
}

#[test]
fn pantone_catalog_clamps_cmyk_to_unit_range() {
    // Anything > 1.0 or < 0.0 is clamped to the unit range — a
    // corrupted authoring tool shouldn't be able to inject an
    // out-of-range ink load that breaks downstream PDF/TIC math.
    let raw = r#"{ "entries": [{ "id": "X", "cmyk": [2.0, -0.5, 0.5, 0.5] }] }"#;
    let lib = SpotColorLibrary::from_json_catalog(raw).expect("valid");
    let def = lib.get("X").unwrap();
    assert_eq!(def.fallback_cmyk.0, 1.0);
    assert_eq!(def.fallback_cmyk.1, 0.0);
    assert_eq!(def.fallback_cmyk.2, 0.5);
    assert_eq!(def.fallback_cmyk.3, 0.5);
}

#[test]
fn pantone_catalog_rejects_non_object_root() {
    let err = SpotColorLibrary::from_json_catalog("[1, 2, 3]").expect_err("array isn't an object");
    assert_eq!(err, SpotCatalogError::Shape);
    let err =
        SpotColorLibrary::from_json_catalog("not-json").expect_err("malformed JSON fails parse");
    assert!(matches!(err, SpotCatalogError::Parse(_)));
}

#[test]
fn spot_color_library_merge_overwrites_collisions() {
    let mut base = SpotColorLibrary::from_json_catalog(
        r#"{ "entries": [{ "id": "A", "cmyk": [0.1, 0.1, 0.1, 0.1] }] }"#,
    )
    .unwrap();
    let overlay = SpotColorLibrary::from_json_catalog(
        r#"{ "entries": [
            { "id": "A", "display_name": "NewA", "cmyk": [0.9, 0.0, 0.0, 0.0] },
            { "id": "B", "cmyk": [0.0, 0.9, 0.0, 0.0] }
        ]}"#,
    )
    .unwrap();
    base.merge(overlay);
    assert_eq!(base.len(), 2);
    let a = base.get("A").unwrap();
    assert_eq!(a.display_name, "NewA");
    assert!((a.fallback_cmyk.0 - 0.9).abs() < 1e-6, "overlay wins");
}

#[test]
fn overprint_table_skips_pure_k_dense_fill() {
    // Pure 95% K is the textbook safe overprint case — should NOT
    // emit an OverprintTable Info.
    let mut doc = DocumentGraph::new();
    let page_id = make_page_with_a4(&mut doc);
    let mut rect = rect_node(page_id, "K95", 5.0, 5.0, 50.0, 50.0);
    rect.style.color_override = Some(cmyk_color(0.0, 0.0, 0.0, 0.95));
    rect.style.overprint = true;
    let _ = doc.insert_node(rect).unwrap();

    let issues = run_preflight_with_spots(
        &doc,
        &[page_id],
        &PreflightOptions::default(),
        &SpotColorLibrary::new(),
    );
    assert!(
        !issues
            .iter()
            .any(|i| i.check == PreflightCheck::OverprintTable),
        "dense K should be safe to overprint"
    );
}

#[test]
fn overprint_table_skips_spot_ink() {
    let mut doc = DocumentGraph::new();
    let page_id = make_page_with_a4(&mut doc);
    let mut rect = rect_node(page_id, "SpotOverprint", 5.0, 5.0, 50.0, 50.0);
    rect.style.color_override = Some(make_spot("PANTONE Reflex Blue C"));
    rect.style.overprint = true;
    let _ = doc.insert_node(rect).unwrap();

    let mut lib = SpotColorLibrary::new();
    lib.insert(
        "PANTONE Reflex Blue C",
        SpotColorDef {
            display_name: "Reflex Blue".into(),
            fallback_cmyk: (1.0, 0.72, 0.0, 0.06),
            library_reference: Some("PANTONE Reflex Blue C".into()),
        },
    );

    let issues = run_preflight_with_spots(&doc, &[page_id], &PreflightOptions::default(), &lib);
    assert!(
        !issues
            .iter()
            .any(|i| i.check == PreflightCheck::OverprintTable),
        "spot inks are always safe to overprint"
    );
}

#[test]
fn overprint_table_flags_light_tint_overprint() {
    // A 20% Y fill set to overprint is the classic surprise — the
    // engine should emit OverprintTable Info.
    let mut doc = DocumentGraph::new();
    let page_id = make_page_with_a4(&mut doc);
    let mut rect = rect_node(page_id, "LightTint", 5.0, 5.0, 40.0, 40.0);
    rect.style.color_override = Some(cmyk_color(0.0, 0.0, 0.20, 0.0));
    rect.style.overprint = true;
    let _ = doc.insert_node(rect).unwrap();

    let issues = run_preflight_with_spots(
        &doc,
        &[page_id],
        &PreflightOptions::default(),
        &SpotColorLibrary::new(),
    );
    let op = issues
        .iter()
        .find(|i| i.check == PreflightCheck::OverprintTable)
        .expect("light tint overprint should fire");
    assert_eq!(op.severity, PreflightSeverity::Info);
    assert!(op.message.contains("LightTint"));
    assert!(
        op.message.contains("unpredictable") || op.message.contains("unpredictable mix"),
        "message should explain the risk: got {}",
        op.message
    );
}

#[test]
fn overprint_table_skips_knockout_default() {
    // When overprint isn't set, the check should silently skip the
    // node — that's the entire purpose of having an explicit flag.
    let mut doc = DocumentGraph::new();
    let page_id = make_page_with_a4(&mut doc);
    let mut rect = rect_node(page_id, "Knockout", 5.0, 5.0, 40.0, 40.0);
    rect.style.color_override = Some(cmyk_color(0.0, 0.0, 0.20, 0.0));
    rect.style.overprint = false; // explicit default — knockout
    let _ = doc.insert_node(rect).unwrap();

    let issues = run_preflight_with_spots(
        &doc,
        &[page_id],
        &PreflightOptions::default(),
        &SpotColorLibrary::new(),
    );
    assert!(
        !issues
            .iter()
            .any(|i| i.check == PreflightCheck::OverprintTable),
        "knockout fills must not surface overprint-table issues"
    );
}

#[test]
fn trapping_flags_abutting_non_shared_inks() {
    // Two solid rectangles touching edge-to-edge with completely
    // disjoint inks (pure cyan vs pure magenta). Press mis-reg of
    // 0.1 mm will reveal paper white — the engine should flag it.
    let mut doc = DocumentGraph::new();
    let page_id = make_page_with_a4(&mut doc);

    let mut cyan = rect_node(page_id, "Cyan", 10.0, 10.0, 40.0, 40.0);
    cyan.style.color_override = Some(cmyk_color(1.0, 0.0, 0.0, 0.0));
    cyan.style.fill = FillStyle::Solid(RgbaColor {
        r: 0.0,
        g: 0.6,
        b: 0.9,
        a: 1.0,
    });
    let _cyan_id = doc.insert_node(cyan).unwrap();

    let mut magenta = rect_node(page_id, "Magenta", 50.0, 10.0, 40.0, 40.0);
    magenta.style.color_override = Some(cmyk_color(0.0, 1.0, 0.0, 0.0));
    magenta.style.fill = FillStyle::Solid(RgbaColor {
        r: 0.9,
        g: 0.0,
        b: 0.6,
        a: 1.0,
    });
    let _mag_id = doc.insert_node(magenta).unwrap();

    let issues = run_preflight_with_spots(
        &doc,
        &[page_id],
        &PreflightOptions::default(),
        &SpotColorLibrary::new(),
    );
    let traps: Vec<_> = issues
        .iter()
        .filter(|i| i.check == PreflightCheck::Trapping)
        .collect();
    assert_eq!(traps.len(), 1, "expected exactly one Trapping issue");
    assert_eq!(traps[0].severity, PreflightSeverity::Warning);
    assert!(traps[0].message.contains("Cyan"));
    assert!(traps[0].message.contains("Magenta"));
}

#[test]
fn trapping_suppressed_when_inks_overlap() {
    // Both rectangles carry a dose of Y — the M-plate registration
    // mismatch is hidden by the shared yellow plate. No Trapping
    // issue should fire.
    let mut doc = DocumentGraph::new();
    let page_id = make_page_with_a4(&mut doc);

    let mut a = rect_node(page_id, "A", 10.0, 10.0, 40.0, 40.0);
    a.style.color_override = Some(cmyk_color(0.0, 0.6, 0.6, 0.0)); // M+Y
    let _ = doc.insert_node(a).unwrap();

    let mut b = rect_node(page_id, "B", 50.0, 10.0, 40.0, 40.0);
    b.style.color_override = Some(cmyk_color(0.6, 0.0, 0.6, 0.0)); // C+Y
    let _ = doc.insert_node(b).unwrap();

    let issues = run_preflight_with_spots(
        &doc,
        &[page_id],
        &PreflightOptions::default(),
        &SpotColorLibrary::new(),
    );
    assert!(
        !issues.iter().any(|i| i.check == PreflightCheck::Trapping),
        "shared Y plate should suppress the trapping flag"
    );
}

#[test]
fn trapping_suppressed_when_one_side_explicitly_overprints() {
    // Author-declared overprint on one of the two abutting nodes is
    // the explicit trap — the engine should respect it.
    let mut doc = DocumentGraph::new();
    let page_id = make_page_with_a4(&mut doc);

    let mut a = rect_node(page_id, "A", 10.0, 10.0, 40.0, 40.0);
    a.style.color_override = Some(cmyk_color(1.0, 0.0, 0.0, 0.0));
    let _ = doc.insert_node(a).unwrap();

    let mut b = rect_node(page_id, "B", 50.0, 10.0, 40.0, 40.0);
    b.style.color_override = Some(cmyk_color(0.0, 1.0, 0.0, 0.0));
    b.style.overprint = true; // explicit trap on the lighter side
    let _ = doc.insert_node(b).unwrap();

    let issues = run_preflight_with_spots(
        &doc,
        &[page_id],
        &PreflightOptions::default(),
        &SpotColorLibrary::new(),
    );
    assert!(
        !issues.iter().any(|i| i.check == PreflightCheck::Trapping),
        "author-declared overprint trap should suppress the warning"
    );
}

#[test]
fn trapping_skipped_for_far_apart_rectangles() {
    // Pages with a clear gap between fills carry no registration
    // risk — the engine must not generate false positives.
    let mut doc = DocumentGraph::new();
    let page_id = make_page_with_a4(&mut doc);

    let mut a = rect_node(page_id, "FarA", 10.0, 10.0, 30.0, 30.0);
    a.style.color_override = Some(cmyk_color(1.0, 0.0, 0.0, 0.0));
    let _ = doc.insert_node(a).unwrap();

    let mut b = rect_node(page_id, "FarB", 80.0, 80.0, 30.0, 30.0);
    b.style.color_override = Some(cmyk_color(0.0, 1.0, 0.0, 0.0));
    let _ = doc.insert_node(b).unwrap();

    let issues = run_preflight_with_spots(
        &doc,
        &[page_id],
        &PreflightOptions::default(),
        &SpotColorLibrary::new(),
    );
    assert!(!issues.iter().any(|i| i.check == PreflightCheck::Trapping));
}

#[test]
fn total_ink_coverage_threshold_fires_on_dense_cmyk_fill() {
    // A CMYK fill summing to 360% exceeds the 300% GRACoL cap and
    // should fire the TotalInkCoverage check.
    let mut doc = DocumentGraph::new();
    let page_id = make_page_with_a4(&mut doc);
    let mut rect = rect_node(page_id, "InkBomb", 10.0, 10.0, 50.0, 50.0);
    rect.style.color_override = Some(cmyk_color(0.95, 0.85, 0.85, 0.95));
    let _ = doc.insert_node(rect).unwrap();

    let issues = run_preflight_with_spots(
        &doc,
        &[page_id],
        &PreflightOptions::default(),
        &SpotColorLibrary::new(),
    );
    assert!(
        issues
            .iter()
            .any(|i| i.check == PreflightCheck::TotalInkCoverage),
        "360% ink coverage must fire"
    );
}

#[test]
fn pantone_fixture_loads_full_solid_coated_subset() {
    // Round-trip the bundled fixture catalogue. This is a real
    // hand-authored subset of the Pantone Solid Coated library —
    // exercising the load path on representative data, not just
    // synthetic JSON.
    let raw = include_str!("../fixtures/pantone_solid_coated_subset.json");
    let lib = SpotColorLibrary::from_json_catalog(raw).expect("fixture parses");
    assert!(lib.len() >= 8, "fixture must carry the canonical subset");
    let yellow = lib
        .get("PANTONE Yellow C")
        .expect("Pantone Yellow C present");
    let (_c, _m, y_chan, _k) = yellow.fallback_cmyk;
    assert!(
        y_chan > 0.9,
        "Pantone Yellow C has near-100% Y; got {y_chan}"
    );
}

#[test]
fn use_solid_rgb_helper_to_silence_unused() {
    // The `solid_rgb` helper is used in extended fixture paths
    // (added in later tasks); keep it referenced so the build stays
    // warning-free without `#[allow(dead_code)]`.
    let mut node = Node::new(NodeType::VectorLayer, "x");
    solid_rgb(&mut node, RgbaColor::BLACK);
    assert!(matches!(node.style.fill, FillStyle::Solid(_)));
}
