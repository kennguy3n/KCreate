//! G6 real-design proof — compose a recognizable analytics dashboard
//! ENTIRELY from the bundled Elements/asset library, recolour several
//! inserted assets to prove they are editable vector nodes (not flat
//! rasters), then export the composed document to a PNG.
//!
//! This drives the real bridge path end-to-end, the same seam the
//! Electron host hits over IPC:
//!
//! ```text
//! state::init → project_create → assets::insert × N
//!   → document_update_node (recolour) → export_png_file
//! ```
//!
//! It doubles as a regression test: every asset id referenced here must
//! stay in the catalogue, each insert must succeed as an editable
//! vector node, each recolour must apply, and the export must produce a
//! valid, non-trivial PNG. The composed artwork is written to
//! `$CARGO_TARGET_TMPDIR/g6_elements_dashboard.png` and its path printed
//! (run with `-- --nocapture`) so it can be captured for the PR proof.
//!
//! Lives in its own integration binary because the renderer + workspace
//! are process-global singletons; a dedicated file gives this test a
//! clean renderer no other test has driven.

use kcreate_bridge::document::{
    document_update_node, export_png_file, project_close, project_create, PngExportRequest,
    UpdateNodeProps,
};
use kcreate_bridge::{assets, state};
use kcreate_core::node::{FillStyle, RgbaColor};
use serial_test::serial;
use std::path::Path;
use tempfile::TempDir;
use uuid::Uuid;

const W: u32 = 1920;
const H: u32 = 1080;

/// Four evenly-spaced card columns spanning the 1920-wide artboard with
/// an 80px outer margin (`80 + 380 + 80` repeated).
const COLS: [f64; 4] = [80.0, 540.0, 1000.0, 1460.0];
const CARD: f64 = 380.0;
const KPI_Y: f64 = 180.0;
const CHART_Y: f64 = 620.0;

/// sRGB byte triple → renderer [`RgbaColor`] (opaque). Components are
/// passed separately to keep clippy's `unreadable_literal` happy and the
/// call sites self-documenting.
fn rgb(r: u8, g: u8, b: u8) -> RgbaColor {
    RgbaColor::new(
        f32::from(r) / 255.0,
        f32::from(g) / 255.0,
        f32::from(b) / 255.0,
        1.0,
    )
}

#[test]
#[serial]
fn elements_library_composes_a_recognizable_dashboard_png() {
    project_close();
    let dir = TempDir::new().expect("tmpdir");
    project_create("g6-elements-dashboard", dir.path()).expect("project_create");

    // The renderer must be live before mutations so each insert's
    // `sync_scene_locked` actually composes the scene (otherwise
    // `render_scene` returns NotInitialized, swallowed as a no-op).
    state::init(W, H).expect("init renderer");

    // Insert a bundled asset and return its leaf vector-node ids (parsed
    // to Uuids) so the caller can recolour them. `parent_id = None`
    // attaches to the document root, so the artwork is unclipped and
    // painted on top of the white artboard. Insertion order is z-order:
    // a card background inserted before its contents sits behind them.
    let insert = |id: &str, x: f64, y: f64, size: f64| -> Vec<Uuid> {
        let placed =
            assets::insert(id, None, x, y, size).unwrap_or_else(|e| panic!("insert {id}: {e}"));
        placed
            .node_ids
            .iter()
            .map(|s| Uuid::parse_str(s).expect("leaf node uuid"))
            .collect()
    };
    // Recolour a solid-fill leaf node — the real editable-node path the
    // FillSection panel commits through.
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

    let card_kpi = rgb(226, 232, 240); // slate-200
    let card_chart = rgb(241, 245, 249); // slate-100
    let accent = rgb(67, 97, 238); // indigo
    let positive = rgb(22, 163, 74); // green-600
    let negative = rgb(225, 29, 72); // rose-600

    // --- header: app logo + action icons --------------------------------
    insert("grid", 80.0, 80.0, 52.0);
    insert("search", 1620.0, 82.0, 44.0);
    insert("bell", 1700.0, 82.0, 44.0);
    insert("user", 1780.0, 82.0, 44.0);

    // --- KPI row: four metric cards --------------------------------------
    let kpi_icons = ["chart-line", "users", "cart", "eye"];
    let kpi_up = [true, true, false, true];
    for (i, &cx) in COLS.iter().enumerate() {
        let bg = insert("rounded-rectangle", cx, KPI_Y, CARD);
        recolor(bg[0], card_kpi);

        // Metric glyph (keeps its dark stroke — good contrast on slate).
        insert(kpi_icons[i], cx + 50.0, KPI_Y + 50.0, 110.0);

        // Accent status dot, top-right.
        let dot = insert("circle", cx + CARD - 84.0, KPI_Y + 36.0, 44.0);
        recolor(dot[0], accent);

        // Trend arrow, lower-left: green when up, rose when down.
        let arrow = insert("arrow-block", cx + 50.0, KPI_Y + 250.0, 92.0);
        recolor(arrow[0], if kpi_up[i] { positive } else { negative });
    }

    // --- chart row: backgrounds first, then artwork on top ---------------
    for &cx in &COLS {
        let bg = insert("rounded-rectangle", cx, CHART_Y, CARD);
        recolor(bg[0], card_chart);
    }
    insert("chart-bar", COLS[0] + 70.0, CHART_Y + 70.0, 240.0);
    insert("chart-line", COLS[1] + 70.0, CHART_Y + 70.0, 240.0);
    insert("chart-pie", COLS[2] + 70.0, CHART_Y + 70.0, 240.0);
    // Flat illustrations (multi-colour, left as authored).
    insert("rocket-illo", COLS[3] + 110.0, CHART_Y + 60.0, 200.0);
    insert("trophy-illo", COLS[3] + 60.0, CHART_Y + 250.0, 90.0);

    // --- export the composed document to PNG -----------------------------
    let out = Path::new(env!("CARGO_TARGET_TMPDIR")).join("g6_elements_dashboard.png");
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

    let data = std::fs::read(&out).expect("read exported png");
    assert!(
        data.starts_with(&[0x89, b'P', b'N', b'G']),
        "exported file must be a PNG (magic header)"
    );
    assert!(
        data.windows(4).any(|w| w == b"IDAT"),
        "PNG must carry pixel data (IDAT chunk)"
    );
    assert!(
        written > 2_000,
        "a real composed dashboard must be a non-trivial PNG, got {written} bytes"
    );

    project_close();
}
