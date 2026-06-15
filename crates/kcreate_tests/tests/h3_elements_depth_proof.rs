//! H3 real-design proof — compose a recognizable, on-brand analytics /
//! product-launch infographic ENTIRELY from the EXPANDED Elements
//! library (icons outline + filled, basic shapes, lines/arrows,
//! frames/dividers, badges, chart-primitive illustrations and spot
//! illustrations), with a theme accent set so inserts are recoloured
//! toward the brand accent — then export the composed document to PNG
//! and assert on the *actual pixels*.
//!
//! This exercises the real bridge seam the Electron host drives over
//! IPC:
//!
//! ```text
//! state::init → project_create → design_tokens_set (theme accent)
//!   → assets::insert × N (recoloured toward the accent on insert)
//!   → document_update_node (neutral card backgrounds / trend arrows)
//!   → export_png_file
//! ```
//!
//! Unlike a blank-rectangle smoke test, this decodes the exported PNG
//! and proves (a) the canvas is richly painted (many distinct colours,
//! a large painted area) and (b) the theme-aware recolour actually
//! reached real pixels — the dominant chromatic hue on the canvas is
//! the brand accent hue, not the assets' authored colours. The artwork
//! is written to `$CARGO_TARGET_TMPDIR/h3_elements_infographic.png` and
//! its path printed (run with `-- --nocapture`) for the PR proof.
//!
//! Lives in its own integration binary because the renderer + workspace
//! are process-global singletons; a dedicated file gives this test a
//! clean renderer no other test has driven.

use kcreate_bridge::document::{
    design_tokens_set, document_update_node, export_png_file, project_close, project_create,
    PngExportRequest, UpdateNodeProps,
};
use kcreate_bridge::{assets, state};
use kcreate_core::node::{FillStyle, RgbaColor};
use kcreate_core::project::DesignTokens;
use serial_test::serial;
use std::path::Path;
use uuid::Uuid;

const W: u32 = 1600;
const H: u32 = 1000;

/// sRGB byte triple → renderer [`RgbaColor`] (opaque).
fn rgb(r: u8, g: u8, b: u8) -> RgbaColor {
    RgbaColor::new(
        f32::from(r) / 255.0,
        f32::from(g) / 255.0,
        f32::from(b) / 255.0,
        1.0,
    )
}

/// Hue (degrees, `[0,360)`), saturation and lightness for an sRGB byte
/// triple, mirroring `kcreate_core::color::srgb_to_hsl` closely enough
/// for the pixel-level assertions below.
fn hsl(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let (rf, gf, bf) = (
        f32::from(r) / 255.0,
        f32::from(g) / 255.0,
        f32::from(b) / 255.0,
    );
    let max = rf.max(gf).max(bf);
    let min = rf.min(gf).min(bf);
    let l = f32::midpoint(max, min);
    let d = max - min;
    if d <= f32::EPSILON {
        return (0.0, 0.0, l);
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs());
    let h = if (max - rf).abs() < f32::EPSILON {
        60.0 * (((gf - bf) / d).rem_euclid(6.0))
    } else if (max - gf).abs() < f32::EPSILON {
        60.0 * ((bf - rf) / d + 2.0)
    } else {
        60.0 * ((rf - gf) / d + 4.0)
    };
    (h.rem_euclid(360.0), s, l)
}

#[test]
#[serial]
fn elements_library_composes_a_themed_infographic_png() {
    project_close();
    let dir = tempfile::TempDir::new().expect("tmpdir");
    project_create("h3-elements-infographic", dir.path()).expect("project_create");

    // The renderer must be live before mutations so each insert's
    // `sync_scene_locked` actually composes the scene.
    state::init(W, H).expect("init renderer");

    // Brand accent: a vivid violet (hue ~271°) — distinct from every
    // asset's authored colour, so "did the recolour reach the pixels?"
    // is unambiguous. Setting it through `design_tokens_set` is exactly
    // what the theme/brand-kit flow does before the user drops assets.
    let accent = rgb(147, 51, 234);
    let (accent_hue, _, _) = hsl(147, 51, 234);
    let mut tokens = DesignTokens::default();
    tokens.colors.insert("accent".to_string(), accent);
    design_tokens_set(tokens).expect("set theme accent");

    // Insert a bundled asset; return its leaf vector-node ids so we can
    // optionally override a colour afterwards (editable-node path).
    let insert = |id: &str, x: f64, y: f64, size: f64| -> Vec<Uuid> {
        let placed =
            assets::insert(id, None, x, y, size).unwrap_or_else(|e| panic!("insert {id}: {e}"));
        placed
            .node_ids
            .iter()
            .map(|s| Uuid::parse_str(s).expect("leaf node uuid"))
            .collect()
    };
    let recolor = |node: Uuid, color: RgbaColor| {
        document_update_node(
            node,
            &UpdateNodeProps {
                fill: Some(FillStyle::Solid(color)),
                ..Default::default()
            },
        )
        .expect("recolour inserted node");
    };

    // Neutral surfaces that the accent glyphs sit on. Inserted assets
    // are auto-recoloured to the accent; we override the card/panel
    // backgrounds back to neutral so the composition reads as a real
    // dashboard rather than a flat wash.
    let card = rgb(241, 245, 249); // slate-100
    let positive = rgb(22, 163, 74); // green-600
    let negative = rgb(225, 29, 72); // rose-600

    // --- header band: megaphone + banner title + divider ----------------
    insert("megaphone", 80.0, 70.0, 120.0); // multi-colour spot illo → accent hue
    let banner = insert("banner", 600.0, 78.0, 400.0);
    recolor(banner[0], accent); // explicit: the hero ribbon is the brand colour
    insert("divider-line", 80.0, 210.0, 1440.0);

    // --- KPI row: four metric cards --------------------------------------
    let cols = [80.0, 450.0, 820.0, 1190.0];
    let card_w = 330.0;
    let kpi_y = 250.0;
    let kpi_icons = ["users", "cart", "eye", "activity"];
    let kpi_up = [true, true, false, true];
    let badges = ["badge-star", "badge-circle", "badge-shield", "badge-ribbon"];
    for (i, &cx) in cols.iter().enumerate() {
        let bg = insert("rounded-rectangle", cx, kpi_y, card_w);
        recolor(bg[0], card);

        // Metric glyph (outline icon → recoloured to the accent).
        insert(kpi_icons[i], cx + 40.0, kpi_y + 40.0, 96.0);

        // Corner badge (frame asset → accent).
        insert(badges[i], cx + card_w - 78.0, kpi_y + 28.0, 60.0);

        // Trend arrow, explicitly green/red to read as up/down.
        let arrow = insert(
            if kpi_up[i] {
                "arrow-block-up"
            } else {
                "arrow-block-down"
            },
            cx + 40.0,
            kpi_y + 190.0,
            72.0,
        );
        recolor(arrow[0], if kpi_up[i] { positive } else { negative });
    }

    // --- chart row: chart-primitive illustrations on neutral panels ------
    let chart_y = 560.0;
    let charts = [
        "chart-bars-illo",
        "chart-line-illo",
        "chart-donut-illo",
        "chart-area-illo",
    ];
    for (i, &cx) in cols.iter().enumerate() {
        let bg = insert("rounded-rectangle", cx, chart_y, card_w);
        recolor(bg[0], card);
        insert(charts[i], cx + 45.0, chart_y + 45.0, 240.0);
        if i + 1 < cols.len() {
            // Flow connector between charts (line asset → accent).
            insert("arrow-right", cx + card_w - 6.0, chart_y + 150.0, 56.0);
        }
    }

    // --- footer: contact / social glyph row ------------------------------
    let footer_icons = ["globe", "at-sign", "mail", "phone", "hash", "map-pin"];
    for (i, id) in footer_icons.iter().enumerate() {
        insert(id, 80.0 + (i as f64) * 80.0, 900.0, 48.0);
    }
    // A spot illustration to anchor the lower-right (multi → accent hue).
    insert("rocket-illo", 1360.0, 850.0, 150.0);

    // --- export the composed document to PNG -----------------------------
    let out = Path::new(env!("CARGO_TARGET_TMPDIR")).join("h3_elements_infographic.png");
    let written = export_png_file(
        &out,
        &PngExportRequest {
            width: W,
            height: H,
            scale: 1.0,
            background: Some([1.0, 1.0, 1.0, 1.0]),
        },
    )
    .expect("export png");
    println!("PROOF_PNG={}", out.display());
    assert!(
        written > 2_000,
        "a real composed infographic must be a non-trivial PNG, got {written} bytes"
    );

    // --- assert on the actual pixels -------------------------------------
    let img = image::open(&out).expect("decode exported png").to_rgb8();
    let mut distinct: std::collections::HashSet<[u8; 3]> = std::collections::HashSet::new();
    let mut painted = 0u64; // non-near-white pixels
    let mut chromatic = 0u64; // strongly saturated pixels
    let mut near_accent = 0u64; // chromatic pixels at the accent hue
    for px in img.pixels() {
        let [r, g, b] = px.0;
        // Quantise to 4 bits/channel so anti-aliasing doesn't inflate
        // the distinct-colour count into the thousands.
        distinct.insert([r & 0xF0, g & 0xF0, b & 0xF0]);
        let (h, s, l) = hsl(r, g, b);
        if !(l > 0.93 && s < 0.06) {
            painted += 1;
        }
        if s > 0.35 && (0.18..0.85).contains(&l) {
            chromatic += 1;
            let dh = (h - accent_hue).abs().min(360.0 - (h - accent_hue).abs());
            if dh <= 22.0 {
                near_accent += 1;
            }
        }
    }
    let total = u64::from(W) * u64::from(H);

    // A recognizable, multi-element design — not a flat block.
    assert!(
        distinct.len() >= 16,
        "expected a richly coloured composition, got {} distinct colours",
        distinct.len()
    );
    // A real layout fills a meaningful fraction of the artboard.
    let painted_frac = painted as f64 / total as f64;
    assert!(
        painted_frac > 0.04,
        "expected a substantial painted area, only {:.1}% painted",
        painted_frac * 100.0
    );
    // The headline H3 claim: theme-aware recolour reached the canvas.
    // Most strongly-chromatic pixels carry the brand accent hue (the
    // few green/red trend arrows are the deliberate exception).
    assert!(
        chromatic > 20_000,
        "expected a strongly-coloured composition, got {chromatic} chromatic px"
    );
    let accent_frac = near_accent as f64 / chromatic as f64;
    assert!(
        accent_frac > 0.5,
        "theme recolour should dominate the chromatic pixels; only {:.1}% sat the accent hue",
        accent_frac * 100.0
    );

    project_close();
}
