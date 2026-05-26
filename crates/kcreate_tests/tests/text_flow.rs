//! Integration coverage for Phase 5 text flow + wrap + .kbrand +
//! slice export.

use std::collections::HashMap;

use kcreate_core::node::{FrameInsets, TextFrameOptions};
use kcreate_core::project::{BrandKit, ExportFormat, FontRef, Slice};
use kcreate_core::Bounds;
use kcreate_export::kbrand::{
    export_brand_kit, import_brand_kit, KbrandError, KBRAND_FORMAT_VERSION_MAJOR,
};
use kcreate_export::slice::export_slices;
use kcreate_renderer::geometry::{Color, Rect, Style};
use kcreate_renderer::scene::{Object, ObjectKind, Scene};
use kcreate_text::flow::{FrameRect, TextFlowEngine};
use kcreate_text::paragraph::TextStyle;
use kcreate_text::wrap::{carve_frames, WrapMode, WrapObstacle};

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
fn single_frame_flow_contains_all_text() {
    let engine = TextFlowEngine::new();
    let out = engine
        .layout(
            "hello world",
            &[frame(0.0, 0.0, 400.0, 200.0)],
            &small_style(),
            None,
        )
        .expect("layout");
    assert_eq!(out.len(), 1);
    assert!(!out[0].overflowed_into_next);
}

#[test]
fn empty_text_produces_empty_layout() {
    let engine = TextFlowEngine::new();
    let out = engine
        .layout("", &[frame(0.0, 0.0, 100.0, 100.0)], &small_style(), None)
        .expect("layout");
    assert_eq!(out.len(), 1);
    assert!(out[0].lines.is_empty());
    assert!(!out[0].overflowed_into_next);
}

#[test]
fn empty_frame_chain_returns_error() {
    let engine = TextFlowEngine::new();
    let err = engine
        .layout("hi", &[], &small_style(), None)
        .expect_err("error");
    let msg = format!("{err}");
    let lower = msg.to_lowercase();
    assert!(
        lower.contains("frame")
            && (lower.contains("empty") || lower.contains("zero") || lower.contains("at least")),
        "unexpected error: {msg}"
    );
}

#[test]
fn two_frames_overflow_chains_into_second() {
    // Force overflow by using a tiny first frame and a generous second.
    let frames = [frame(0.0, 0.0, 60.0, 14.0), frame(0.0, 16.0, 400.0, 200.0)];
    let engine = TextFlowEngine::new();
    let text = "one two three four five six seven eight nine ten";
    let out = engine
        .layout(text, &frames, &small_style(), None)
        .expect("ok");
    assert_eq!(out.len(), 2);
    // The first frame should not consume everything — either it
    // overflowed into the next, or the second received content.
    let first_consumed = out[0].overflowed_into_next || !out[1].lines.is_empty();
    assert!(first_consumed, "second frame should receive overflow");
}

#[test]
fn wrap_no_obstacles_returns_original_frame() {
    let frame = frame(0.0, 0.0, 400.0, 200.0);
    let out = carve_frames(&frame, &[]);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].bounds, frame.bounds);
}

#[test]
fn wrap_obstacle_splits_lines_on_both_sides() {
    let frame = frame(0.0, 0.0, 400.0, 100.0);
    let obstacle = WrapObstacle {
        bounds: Bounds {
            x: 150.0,
            y: 0.0,
            width: 100.0,
            height: 100.0,
        },
        margin: 0.0,
        wrap_mode: WrapMode::Both,
    };
    let out = carve_frames(&frame, &[obstacle]);
    // Per-line two bands × N rows; we don't pin the row count
    // because it depends on the line-height heuristic, but every
    // band must have positive width.
    assert!(!out.is_empty());
    for f in &out {
        assert!(f.bounds.width > 0.0);
        assert!(f.bounds.height > 0.0);
    }
}

#[test]
fn kbrand_round_trip_preserves_all_tokens() {
    let mut kit = BrandKit::new("Round-Trip");
    kit.spacing_scale = vec![4.0, 8.0, 16.0, 32.0];
    kit.fonts.push(FontRef {
        family: "Inter".into(),
        weight: 700,
        italic: false,
        embedded_asset_id: None,
    });

    let mut fonts: HashMap<String, Vec<u8>> = HashMap::new();
    let mut otf = b"OTTO".to_vec();
    otf.extend_from_slice(&[0u8; 64]);
    fonts.insert("Inter-700".into(), otf);

    let mut logos: HashMap<String, Vec<u8>> = HashMap::new();
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend_from_slice(&[0u8; 64]);
    logos.insert("wordmark".into(), png);

    let tmp = tempfile::NamedTempFile::new().expect("temp");
    export_brand_kit(&kit, &fonts, &logos, tmp.path()).expect("export");
    let bundle = import_brand_kit(tmp.path()).expect("import");
    assert_eq!(bundle.manifest.format_major, KBRAND_FORMAT_VERSION_MAJOR);
    assert_eq!(bundle.manifest.name, "Round-Trip");
    assert_eq!(bundle.manifest.spacing_scale, vec![4.0, 8.0, 16.0, 32.0]);
    assert_eq!(bundle.manifest.fonts.len(), 1);
    assert_eq!(bundle.manifest.logos.len(), 1);
}

#[test]
fn kbrand_font_family_with_hyphens_round_trips() {
    // Regression for the font_asset_key vs sanitize_name mismatch
    // that silently dropped any family containing non-alphanumeric
    // characters (e.g. "Source-Sans-Pro") from the archive.
    let mut kit = BrandKit::new("Hyphen-Test");
    kit.fonts.push(FontRef {
        family: "Source-Sans-Pro".into(),
        weight: 400,
        italic: false,
        embedded_asset_id: None,
    });
    kit.fonts.push(FontRef {
        family: "Noto Sans JP".into(),
        weight: 700,
        italic: true,
        embedded_asset_id: None,
    });

    let mut fonts: HashMap<String, Vec<u8>> = HashMap::new();
    let mut otf_a = b"OTTO".to_vec();
    otf_a.extend_from_slice(&[0u8; 64]);
    fonts.insert(
        kcreate_export::kbrand::font_archive_basename("Source-Sans-Pro", 400, false),
        otf_a,
    );
    let mut otf_b = b"OTTO".to_vec();
    otf_b.extend_from_slice(&[0u8; 64]);
    fonts.insert(
        kcreate_export::kbrand::font_archive_basename("Noto Sans JP", 700, true),
        otf_b,
    );

    let logos: HashMap<String, Vec<u8>> = HashMap::new();
    let tmp = tempfile::NamedTempFile::new().expect("temp");
    export_brand_kit(&kit, &fonts, &logos, tmp.path()).expect("export");
    let bundle = import_brand_kit(tmp.path()).expect("import");

    // Both fonts must round-trip with archive paths populated.
    assert_eq!(bundle.manifest.fonts.len(), 2);
    for entry in &bundle.manifest.fonts {
        assert!(
            entry.archive_path.is_some(),
            "font {} weight={} italic={} dropped from archive",
            entry.family,
            entry.weight,
            entry.italic
        );
    }
}

#[test]
fn kbrand_with_invalid_font_is_rejected() {
    let mut kit = BrandKit::new("Bad");
    kit.fonts.push(FontRef {
        family: "Bogus".into(),
        weight: 400,
        italic: false,
        embedded_asset_id: None,
    });
    let mut fonts = HashMap::new();
    fonts.insert("Bogus-400".into(), b"definitely-not-a-font".to_vec());
    let logos = HashMap::new();
    let tmp = tempfile::NamedTempFile::new().expect("temp");
    let err = export_brand_kit(&kit, &fonts, &logos, tmp.path()).err();
    assert!(matches!(err, Some(KbrandError::InvalidFontAsset { .. })));
}

#[test]
fn slice_export_emits_one_file_per_slice() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut scene = Scene::new(Color::rgba(0.0, 0.0, 0.0, 1.0));
    let obj = Object::new(
        ObjectKind::Rect(Rect::new(0.0, 0.0, 200.0, 200.0)),
        Style {
            fill: Some(Color::rgba(1.0, 0.0, 0.0, 1.0)),
            stroke: None,
        },
    );
    scene.add_object(obj);

    let slices = vec![
        Slice::new(
            "top",
            Bounds {
                x: 0.0,
                y: 0.0,
                width: 100.0,
                height: 100.0,
            },
            ExportFormat::Png,
            1.0,
        ),
        Slice::new(
            "bot",
            Bounds {
                x: 100.0,
                y: 100.0,
                width: 100.0,
                height: 100.0,
            },
            ExportFormat::Png,
            1.0,
        ),
    ];
    let out = export_slices(&scene, &slices, dir.path()).expect("ok");
    assert_eq!(out.len(), 2);
    for r in &out {
        assert!(r.error.is_none(), "{:?}", r.error);
        let p = r.path.as_ref().expect("path");
        assert!(p.exists());
    }
}
