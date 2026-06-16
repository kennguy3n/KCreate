//! I4 plugin-marketplace real-design proof — install the two bundled,
//! signed WASM demo plugins through the *real* marketplace seam, run
//! them in the wasmi sandbox against a recognizable composed design,
//! and export the document to PNG **before and after** so the effect
//! is visible in actual pixels (not blank rectangles).
//!
//! This drives exactly the seam the Electron host drives over IPC:
//!
//! ```text
//! state::init → project_create → canvas_create_rect × N (+ recolour / scatter)
//!   → export_png_file                                   (BEFORE)
//!   → plugin_marketplace_install_bundled → plugin_list → plugin_enable
//!   → document_set_selection → plugin_execute_on_selection (real WASM)
//!   → export_png_file                                   (AFTER)
//! ```
//!
//! Two scenarios, each asserting on the decoded pixels:
//!
//! * **grid-arrange** — six vivid cards are scattered to messy
//!   positions, then the plugin tidies them into a 3-column grid. The
//!   assertion compares the BEFORE/AFTER rasters and proves a large
//!   fraction of pixels moved (the layout really changed) while the
//!   palette of colours is preserved (same cards, repositioned).
//! * **palette-apply** — a tidy grid of identical neutral-grey cards is
//!   recoloured into a saturated, multi-hue palette. The assertion
//!   proves the BEFORE raster is near-monochrome (≈1 chromatic hue) and
//!   the AFTER raster carries many distinct saturated hues.
//!
//! Artifacts are written under `$CARGO_TARGET_TMPDIR` and their paths
//! printed (run with `-- --nocapture`) for the PR proof.
//!
//! Lives in its own integration binary because the renderer + plugin
//! registry are process-global singletons; a dedicated file gives this
//! proof a clean host no other test has driven, and lets it set
//! `KCREATE_PLUGIN_DIR` before the registry initialises.

use kcreate_bridge::document::{
    canvas_create_rect, canvas_move_node, document_set_selection, document_update_node,
    export_png_file, project_close, project_create, PngExportRequest, UpdateNodeProps,
};
use kcreate_bridge::{phase10, phase2, state};
use kcreate_core::node::{FillStyle, RgbaColor};
use serial_test::serial;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tempfile::TempDir;
use uuid::Uuid;

/// sRGB byte triple → renderer [`RgbaColor`] (opaque).
fn rgb(r: u8, g: u8, b: u8) -> RgbaColor {
    RgbaColor::new(
        f32::from(r) / 255.0,
        f32::from(g) / 255.0,
        f32::from(b) / 255.0,
        1.0,
    )
}

/// Hue (degrees `[0,360)`), saturation and lightness for an sRGB byte
/// triple — enough for the pixel-level hue assertions below.
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
        60.0 * ((gf - bf) / d).rem_euclid(6.0)
    } else if (max - gf).abs() < f32::EPSILON {
        60.0 * ((bf - rf) / d + 2.0)
    } else {
        60.0 * ((rf - gf) / d + 4.0)
    };
    (h.rem_euclid(360.0), s, l)
}

/// Process-lifetime plugin directory. Setting `KCREATE_PLUGIN_DIR`
/// before the first `phase2`/`phase10` plugin call makes both the
/// marketplace and the (lazily-initialised) registry singleton use a
/// throwaway dir instead of `$HOME/.kcreate/plugins`.
fn plugin_dir() -> &'static Path {
    static DIR: OnceLock<TempDir> = OnceLock::new();
    DIR.get_or_init(|| {
        let tmp = TempDir::new().expect("plugin tmpdir");
        std::env::set_var("KCREATE_PLUGIN_DIR", tmp.path());
        tmp
    })
    .path()
}

/// Install a bundled demo plugin through the real marketplace, scan it
/// into the registry, and enable it (the execute path refuses disabled
/// plugins). Idempotent across the two tests in this binary.
fn install_and_enable(id: &str) {
    plugin_dir();
    if let Err(e) = phase10::plugin_marketplace_install_bundled(id) {
        let msg = e.to_string().to_lowercase();
        assert!(msg.contains("already"), "install bundled {id}: {e}");
    }
    phase2::plugin_list().expect("scan registry");
    phase2::plugin_enable(id).expect("enable plugin");
}

fn recolor(node: Uuid, color: RgbaColor) {
    document_update_node(
        node,
        &UpdateNodeProps {
            fill: Some(FillStyle::Solid(color)),
            ..Default::default()
        },
    )
    .expect("recolour node");
}

fn export(out: &Path, w: u32, h: u32) -> u64 {
    export_png_file(
        out,
        &PngExportRequest {
            width: w,
            height: h,
            scale: 1.0,
            background: Some([1.0, 1.0, 1.0, 1.0]),
        },
    )
    .expect("export png")
}

/// Count the applied proposals reported by `plugin_execute_on_selection`.
fn applied_count(report_json: &str) -> usize {
    let parsed: serde_json::Value = serde_json::from_str(report_json).expect("report json");
    parsed
        .get("proposals")
        .and_then(|v| v.as_array())
        .map_or(0, |reports| {
            reports
                .iter()
                .filter(|r| {
                    r.get("outcome")
                        .and_then(|o| o.get("status"))
                        .and_then(|v| v.as_str())
                        == Some("applied")
                })
                .count()
        })
}

/// Painted (non-near-white) pixel count and the set of distinct
/// saturated hue buckets (12° buckets) present in the image.
fn analyze(path: &Path) -> (u64, std::collections::BTreeSet<u16>) {
    let img = image::open(path).expect("decode png").to_rgb8();
    let mut painted = 0u64;
    let mut hues = std::collections::BTreeSet::new();
    for px in img.pixels() {
        let [r, g, b] = px.0;
        let (h, s, l) = hsl(r, g, b);
        if !(l > 0.93 && s < 0.06) {
            painted += 1;
        }
        if s > 0.30 && (0.18..0.88).contains(&l) {
            hues.insert((h / 12.0) as u16);
        }
    }
    (painted, hues)
}

/// Fraction of pixels that differ between two same-size rasters.
fn diff_ratio(a: &Path, b: &Path) -> f64 {
    let ia = image::open(a).expect("decode a").to_rgb8();
    let ib = image::open(b).expect("decode b").to_rgb8();
    assert_eq!(ia.dimensions(), ib.dimensions(), "rasters must match size");
    let mut diff = 0u64;
    for (pa, pb) in ia.pixels().zip(ib.pixels()) {
        let [ra, ga, ba] = pa.0;
        let [rb, gb, bb] = pb.0;
        let d = (i32::from(ra) - i32::from(rb)).abs()
            + (i32::from(ga) - i32::from(gb)).abs()
            + (i32::from(ba) - i32::from(bb)).abs();
        if d > 24 {
            diff += 1;
        }
    }
    f64::from(u32::try_from(diff).unwrap_or(u32::MAX))
        / f64::from(ia.dimensions().0 * ia.dimensions().1)
}

fn tmp_png(name: &str) -> PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR")).join(name)
}

#[test]
#[serial]
fn grid_arrange_plugin_tidies_a_real_design() {
    const W: u32 = 960;
    const H: u32 = 640;

    project_close();
    let dir = TempDir::new().expect("project tmpdir");
    project_create("i4-grid-demo", dir.path()).expect("project_create");
    state::init(W, H).expect("init renderer");
    install_and_enable("com.kcreate.demo.grid-arrange");

    // Six vivid cards, scattered to deliberately messy positions.
    // `canvas_create_rect` leaves the transform at identity, so the
    // card is authored at the origin and `canvas_move_node` populates
    // `transform.tx/ty` — exactly the coordinate the grid plugin reads
    // through the injected input and the coordinate the renderer
    // translates by.
    let colors = [
        rgb(239, 68, 68),  // red
        rgb(245, 158, 11), // amber
        rgb(34, 197, 94),  // green
        rgb(59, 130, 246), // blue
        rgb(168, 85, 247), // violet
        rgb(236, 72, 153), // pink
    ];
    let scatter = [
        (70.0, 60.0),
        (560.0, 90.0),
        (330.0, 380.0),
        (740.0, 300.0),
        (120.0, 440.0),
        (650.0, 470.0),
    ];
    let (card_w, card_h) = (150.0, 100.0);
    let mut ids = Vec::new();
    for (color, (dx, dy)) in colors.iter().zip(scatter) {
        let id = canvas_create_rect(None, 0.0, 0.0, card_w, card_h).expect("rect");
        canvas_move_node(id, dx, dy).expect("scatter");
        recolor(id, *color);
        ids.push(id);
    }

    let before = tmp_png("i4_grid_before.png");
    let before_bytes = export(&before, W, H);
    println!("PROOF_PNG={}", before.display());

    document_set_selection(ids.clone()).expect("select");
    let report = phase2::plugin_execute_on_selection(
        "com.kcreate.demo.grid-arrange",
        "run",
        "{\"columns\":3,\"gap\":40}",
    )
    .expect("run grid plugin");
    assert!(
        applied_count(&report) >= 1,
        "grid plugin must apply at least one move: {report}"
    );

    let after = tmp_png("i4_grid_after.png");
    let after_bytes = export(&after, W, H);
    println!("PROOF_PNG={}", after.display());

    assert!(
        before_bytes > 2_000 && after_bytes > 2_000,
        "both rasters must be non-trivial PNGs"
    );

    // The layout really changed: a large fraction of pixels moved.
    let moved = diff_ratio(&before, &after);
    assert!(
        moved > 0.03,
        "grid layout should visibly change the raster, diff ratio = {moved:.4}"
    );

    // ...but it is a re-arrangement, not a recolour: both rasters carry
    // the same rich set of card hues.
    let (before_painted, before_hues) = analyze(&before);
    let (after_painted, after_hues) = analyze(&after);
    assert!(
        before_hues.len() >= 5 && after_hues.len() >= 5,
        "six vivid cards => many hues before ({}) and after ({})",
        before_hues.len(),
        after_hues.len()
    );
    assert!(
        before_painted > 5_000 && after_painted > 5_000,
        "cards must paint a real area before ({before_painted}) and after ({after_painted})"
    );

    project_close();
}

#[test]
#[serial]
fn palette_apply_plugin_recolours_a_real_design() {
    const W: u32 = 960;
    const H: u32 = 560;

    project_close();
    let dir = TempDir::new().expect("project tmpdir");
    project_create("i4-palette-demo", dir.path()).expect("project_create");
    state::init(W, H).expect("init renderer");
    install_and_enable("com.kcreate.demo.palette-apply");

    // A tidy 3×2 grid of identical neutral-grey cards: the layout is
    // already clean, so the *only* thing the plugin changes is colour.
    let grey = rgb(148, 163, 184); // slate-400
    let cols = [90.0, 370.0, 650.0];
    let rows = [90.0, 320.0];
    let (card_w, card_h) = (220.0, 150.0);
    let mut ids = Vec::new();
    for y in rows {
        for x in cols {
            let id = canvas_create_rect(None, x, y, card_w, card_h).expect("rect");
            recolor(id, grey);
            ids.push(id);
        }
    }

    let before = tmp_png("i4_palette_before.png");
    let before_bytes = export(&before, W, H);
    println!("PROOF_PNG={}", before.display());

    document_set_selection(ids.clone()).expect("select");
    let report = phase2::plugin_execute_on_selection(
        "com.kcreate.demo.palette-apply",
        "run",
        "{\"saturation\":0.72,\"lightness\":0.56,\"hueOffset\":210}",
    )
    .expect("run palette plugin");
    assert_eq!(
        applied_count(&report),
        ids.len(),
        "every selected card should be recoloured: {report}"
    );

    let after = tmp_png("i4_palette_after.png");
    let after_bytes = export(&after, W, H);
    println!("PROOF_PNG={}", after.display());

    assert!(
        before_bytes > 2_000 && after_bytes > 2_000,
        "both rasters must be non-trivial PNGs"
    );

    // BEFORE is near-monochrome grey; AFTER carries many distinct
    // saturated hues — a recognizable palette applied to real pixels.
    let (_, before_hues) = analyze(&before);
    let (_, after_hues) = analyze(&after);
    assert!(
        before_hues.len() <= 1,
        "grey grid should have ~no saturated hues, got {}",
        before_hues.len()
    );
    assert!(
        after_hues.len() >= 4,
        "palette plugin should introduce several distinct hues, got {}",
        after_hues.len()
    );

    // The recolour repaints a substantial area.
    let changed = diff_ratio(&before, &after);
    assert!(
        changed > 0.05,
        "palette recolour should change a real fraction of pixels, got {changed:.4}"
    );

    project_close();
}
