//! Phase 8 Block B: Image Studio primitives.
//!
//! Cross-crate tests for the perspective transform,
//! color-range selection, HSL adjustment, color balance
//! adjustment, and the mask-aware filter bridge surface.

use kcreate_ai::color_range::select_by_color_range;
use kcreate_bridge::raster_ops::{BlurKind, PreviewFilter};
use kcreate_raster::layer::AdjustmentLayer;
use kcreate_raster::tile::TileGrid;
use kcreate_raster::transform::perspective_transform;

fn make_solid_grid(w: u32, h: u32, color: [u8; 4]) -> TileGrid {
    let mut grid = TileGrid::new(w, h, 16).unwrap();
    for y in 0..h {
        for x in 0..w {
            grid.write_pixel(x, y, color);
        }
    }
    grid
}

#[test]
fn perspective_identity_preserves_pixels() {
    let grid = make_solid_grid(32, 32, [255, 100, 50, 255]);
    // Corner order: TL, TR, BL, BR (matches the implementation's convention).
    let corners = [(0.0, 0.0), (32.0, 0.0), (0.0, 32.0), (32.0, 32.0)];
    let result = perspective_transform(&grid, corners);
    assert_eq!(result.width, grid.width);
    assert_eq!(result.height, grid.height);
    // Center pixel should retain the original color.
    let pixel = result.read_pixel_clamped(16, 16);
    assert_eq!(pixel[0], 255);
    assert_eq!(pixel[1], 100);
    assert_eq!(pixel[2], 50);
}

#[test]
fn perspective_with_translated_canvas() {
    // A transform that shifts and shrinks the input — check that the
    // returned canvas has positive dimensions and is non-degenerate.
    let grid = make_solid_grid(32, 32, [255, 100, 50, 255]);
    let corners = [(5.0, 0.0), (27.0, 0.0), (0.0, 32.0), (32.0, 32.0)];
    let result = perspective_transform(&grid, corners);
    assert!(result.width > 0);
    assert!(result.height > 0);
}

#[test]
fn color_range_exact_match_selects_only_target() {
    let mut rgba = Vec::with_capacity(4 * 16);
    // 16 pixels: 8 red, 8 blue
    for _ in 0..8 {
        rgba.extend_from_slice(&[255, 0, 0, 255]);
    }
    for _ in 0..8 {
        rgba.extend_from_slice(&[0, 0, 255, 255]);
    }
    let mask = select_by_color_range(&rgba, 16, 1, [255, 0, 0, 255], 0.0);
    assert_eq!(mask.len(), 16);
    assert_eq!(mask.iter().filter(|m| **m).count(), 8);
    for (i, &m) in mask.iter().enumerate() {
        assert_eq!(m, i < 8);
    }
}

#[test]
fn color_range_fuzziness_widens_selection() {
    let mut rgba = Vec::with_capacity(4 * 4);
    rgba.extend_from_slice(&[255, 0, 0, 255]); // exact red
    rgba.extend_from_slice(&[250, 5, 5, 255]); // near red
    rgba.extend_from_slice(&[100, 100, 100, 255]); // grey
    rgba.extend_from_slice(&[0, 0, 0, 255]); // black
    let mask_strict = select_by_color_range(&rgba, 4, 1, [255, 0, 0, 255], 0.0);
    let mask_loose = select_by_color_range(&rgba, 4, 1, [255, 0, 0, 255], 1.0);
    assert!(
        mask_loose.iter().filter(|m| **m).count() >= mask_strict.iter().filter(|m| **m).count(),
        "loose fuzziness should select at least as many pixels"
    );
}

#[test]
fn hsl_zero_shift_is_identity() {
    // hue 0, saturation 1, lightness 0 means no change.
    let adj = AdjustmentLayer::HueSaturation {
        hue: 0.0,
        saturation: 1.0,
        lightness: 0.0,
    };
    let mut p = [128u8, 64u8, 200u8, 255u8];
    let before = p;
    adj.apply_pixel(&mut p);
    assert_eq!(p, before);
}

#[test]
fn color_balance_zero_is_identity() {
    let adj = AdjustmentLayer::ColorBalance {
        shadows: [0.0, 0.0, 0.0],
        midtones: [0.0, 0.0, 0.0],
        highlights: [0.0, 0.0, 0.0],
    };
    let mut p = [128u8, 64u8, 200u8, 255u8];
    let before = p;
    adj.apply_pixel(&mut p);
    assert_eq!(p, before);
}

#[test]
fn hsl_identity_predicate() {
    let identity = AdjustmentLayer::HueSaturation {
        hue: 0.0,
        saturation: 1.0,
        lightness: 0.0,
    };
    assert!(identity.is_identity());
    let not_identity = AdjustmentLayer::HueSaturation {
        hue: 0.0,
        saturation: 1.0,
        lightness: 0.5,
    };
    assert!(!not_identity.is_identity());
}

/// Lock the wire shape of every `PreviewFilter` variant the bridge
/// accepts — the `type` discriminator and snake_case field names
/// must match what `apps/desktop/shared/scene.ts` ships, or the
/// renderer's live-preview surface silently fails to deserialise.
///
/// If you add a variant here, mirror it in `RasterPreviewFilter`
/// in `scene.ts` and add it to this test.
#[test]
fn preview_filter_wire_shape_is_stable() {
    let cases: Vec<(PreviewFilter, &str)> = vec![
        (
            PreviewFilter::Levels {
                black_point: 0.1,
                white_point: 0.9,
                gamma: 1.5,
            },
            "levels",
        ),
        (
            PreviewFilter::Curves {
                points: vec![(0.0, 0.0), (0.5, 0.4), (1.0, 1.0)],
            },
            "curves",
        ),
        (
            PreviewFilter::Blur {
                radius: 3.0,
                kind: BlurKind::Gaussian,
            },
            "blur",
        ),
        (
            PreviewFilter::Sharpen {
                radius: 1.5,
                amount: 0.8,
                threshold: 4,
            },
            "sharpen",
        ),
        (
            PreviewFilter::Hsl {
                hue: 30.0,
                saturation: 1.2,
                lightness: -0.05,
            },
            "hsl",
        ),
        (
            PreviewFilter::ColorBalance {
                shadows: [0.1, 0.0, -0.1],
                midtones: [0.0, 0.0, 0.0],
                highlights: [-0.2, 0.0, 0.2],
            },
            "color_balance",
        ),
    ];
    for (variant, expected_tag) in cases {
        let json = serde_json::to_value(&variant).expect("serialize");
        assert_eq!(
            json["type"].as_str(),
            Some(expected_tag),
            "wire-format tag mismatch for {variant:?}",
        );
        let back: PreviewFilter = serde_json::from_value(json).expect("deserialize");
        // Round-trip a second serialisation and compare JSON value
        // equality. A direct PartialEq on PreviewFilter would
        // require f32 equality, which we deliberately avoid — the
        // JSON shape is the contract.
        let json_again = serde_json::to_value(&back).expect("re-serialize");
        let json_orig = serde_json::to_value(&variant).expect("orig");
        assert_eq!(
            json_orig, json_again,
            "round-trip JSON mismatch for {variant:?}",
        );
    }
}

/// The bridge's color-range selection must produce a same-length
/// mask even on an empty pixel buffer, so renderer-side mask sizing
/// math doesn't need to special-case empty layers.
#[test]
fn color_range_on_empty_buffer_returns_empty_mask() {
    let mask = select_by_color_range(&[], 0, 0, [0, 0, 0, 255], 1.0);
    assert_eq!(mask.len(), 0);
}

/// Verify the color balance bridge wire-shape produces three flat
/// `[r, g, b]` arrays of finite numbers — the scene.ts mirror
/// expects exactly this. We compare element-wise with tolerance
/// because Rust serialises `f32` through `f64::from(x)`, which
/// surfaces single-precision representation noise (e.g. `0.1f32`
/// → `0.10000000149011612` in JSON).
#[test]
fn preview_filter_color_balance_serialises_triples_as_arrays() {
    let shadows = [0.1f32, 0.2, 0.3];
    let midtones = [0.4f32, 0.5, 0.6];
    let highlights = [0.7f32, 0.8, 0.9];
    let f = PreviewFilter::ColorBalance {
        shadows,
        midtones,
        highlights,
    };
    let json = serde_json::to_value(&f).expect("serialize");
    let expect_array = |key: &str, expected: [f32; 3]| {
        let arr = json[key]
            .as_array()
            .unwrap_or_else(|| panic!("{key} is not an array: {:?}", json[key]));
        assert_eq!(arr.len(), 3, "{key} length");
        for (i, v) in arr.iter().enumerate() {
            let n = v
                .as_f64()
                .unwrap_or_else(|| panic!("{key}[{i}] not a number: {v:?}"));
            assert!(n.is_finite(), "{key}[{i}] must be finite, got {n}");
            let want = f64::from(expected[i]);
            assert!(
                (n - want).abs() < 1e-6,
                "{key}[{i}] = {n}, expected near {want}",
            );
        }
    };
    expect_array("shadows", shadows);
    expect_array("midtones", midtones);
    expect_array("highlights", highlights);
}
