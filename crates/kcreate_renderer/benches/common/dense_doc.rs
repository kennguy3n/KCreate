//! Shared builder for the dense "analytics dashboard" document used by
//! the `frame_present_dense` benchmark and the `dense_present_proof`
//! example. Kept in `benches/common/` (a subdirectory Cargo does **not**
//! auto-discover as a target) and pulled into both via `#[path]` so the
//! 5k / 10k-node scene is defined in exactly one place.
//!
//! The scene is a wireframe BI dashboard: a gradient header band, a
//! gradient sidebar rail, a row of gradient KPI tiles, per-row section
//! labels (text), and a dense grid of metric cards (each a background
//! panel, a progress-track rect, a progress-fill rect, and a divider
//! line). A single high-z "selection marker" rect sits on top at a fixed
//! location; toggling its colour is the canonical "typical edit" whose
//! dirty rectangle the present path ships instead of the whole frame.

use kcreate_renderer::{
    Color, Object, ObjectId, ObjectKind, Paint, Point2, Rect, Scene, Stroke, Style,
};

/// A built dense document plus the handle needed to drive a typical
/// single-element edit against it.
#[derive(Debug)]
pub(crate) struct DenseDoc {
    /// The scene, ready to hand to `render_frame`.
    pub scene: Scene,
    /// Id of the on-top selection marker rect; pass to [`toggle_marker`].
    pub marker_id: ObjectId,
}

/// The two colours the selection marker alternates between. Both are
/// fully opaque so the marker always overwrites the pixels beneath it —
/// the changed region is therefore exactly the marker rect.
const MARKER_ON: Color = Color::rgba(0.98, 0.27, 0.36, 1.0);
const MARKER_OFF: Color = Color::rgba(0.18, 0.71, 0.49, 1.0);

/// Fixed chrome: header band, sidebar rail, four KPI tiles (gradient
/// rect + label text + value text each), and the dashboard title text.
const CHROME_NODES: usize = 2 + 4 * 3 + 1;

/// Build a dense dashboard whose node count lands within a handful of
/// nodes of `target_nodes`, laid out to fill a `width`×`height`
/// framebuffer. `target_nodes` is expected to be in the thousands.
pub(crate) fn build_dense_document(target_nodes: usize, width: f32, height: f32) -> DenseDoc {
    let mut objects: Vec<Object> = Vec::with_capacity(target_nodes + 8);
    let mut z: i32 = 0;
    let mut push = |objects: &mut Vec<Object>, kind: ObjectKind, style: Style| {
        objects.push(Object::new(kind, style).with_z(z));
        z += 1;
    };

    // --- Chrome -----------------------------------------------------
    let header_h = 64.0;
    let sidebar_w = 96.0;
    push(
        &mut objects,
        ObjectKind::Rect(Rect::new(0.0, 0.0, width, header_h)),
        Style::linear_gradient(
            Point2::new(0.0, 0.0),
            Point2::new(width, 0.0),
            vec![
                (0.0, Color::rgba(0.31, 0.27, 0.90, 1.0)),
                (1.0, Color::rgba(0.55, 0.31, 0.93, 1.0)),
            ],
        ),
    );
    push(
        &mut objects,
        ObjectKind::Rect(Rect::new(0.0, header_h, sidebar_w, height - header_h)),
        Style::linear_gradient(
            Point2::new(0.0, header_h),
            Point2::new(0.0, height),
            vec![
                (0.0, Color::rgba(0.12, 0.14, 0.20, 1.0)),
                (1.0, Color::rgba(0.08, 0.09, 0.13, 1.0)),
            ],
        ),
    );
    push(
        &mut objects,
        ObjectKind::Text {
            origin: Point2::new(sidebar_w + 16.0, 40.0),
            text: "KCreate Analytics".to_owned(),
            font_family: "sans-serif".to_owned(),
            font_size: 22.0,
        },
        Style::filled(Color::rgba(1.0, 1.0, 1.0, 1.0)),
    );

    // KPI tiles under the header.
    let kpi_top = header_h + 16.0;
    let kpi_h = 88.0;
    let kpi_area_x = sidebar_w + 16.0;
    let kpi_gap = 16.0;
    let kpi_w = (width - kpi_area_x - 16.0 - kpi_gap * 3.0) / 4.0;
    for i in 0..4usize {
        let x = kpi_area_x + (kpi_w + kpi_gap) * i as f32;
        let hue = 0.55 - i as f32 * 0.12;
        push(
            &mut objects,
            ObjectKind::Rect(Rect::new(x, kpi_top, kpi_w, kpi_h)),
            Style::linear_gradient(
                Point2::new(x, kpi_top),
                Point2::new(x + kpi_w, kpi_top + kpi_h),
                vec![
                    (0.0, hsl(hue, 0.62, 0.55)),
                    (1.0, hsl(hue - 0.04, 0.66, 0.42)),
                ],
            ),
        );
        push(
            &mut objects,
            ObjectKind::Text {
                origin: Point2::new(x + 14.0, kpi_top + 30.0),
                text: ["Revenue", "Sessions", "Conversion", "Churn"][i].to_owned(),
                font_family: "sans-serif".to_owned(),
                font_size: 13.0,
            },
            Style::filled(Color::rgba(0.92, 0.95, 1.0, 0.92)),
        );
        push(
            &mut objects,
            ObjectKind::Text {
                origin: Point2::new(x + 14.0, kpi_top + 62.0),
                text: ["$4.2M", "182k", "3.8%", "1.1%"][i].to_owned(),
                font_family: "sans-serif".to_owned(),
                font_size: 26.0,
            },
            Style::filled(Color::rgba(1.0, 1.0, 1.0, 1.0)),
        );
    }

    // --- Card grid --------------------------------------------------
    let body_x = sidebar_w + 16.0;
    let body_y = kpi_top + kpi_h + 20.0;
    let body_w = width - body_x - 16.0;
    let body_h = (height - body_y - 16.0).max(1.0);
    let aspect = (body_w / body_h).max(0.1);
    let cards_est = ((target_nodes.saturating_sub(CHROME_NODES)) / NODES_PER_CARD).max(1);
    let cols = ((cards_est as f32 * aspect).sqrt().round() as usize).clamp(8, 256);
    let cell_w = body_w / cols as f32;
    // Square-ish cells, but never so tall that fewer than a few rows
    // fit — the grid is allowed to overflow the framebuffer vertically
    // (off-screen cards still cost build time and count as nodes).
    let cell_h = cell_w.clamp(18.0, 64.0);

    // Reserve one node for the on-top marker.
    let body_budget = target_nodes.saturating_sub(1);
    let mut col = 0usize;
    let mut row = 0usize;
    while objects.len() < body_budget {
        if col == 0 {
            // Per-row section label (the dense doc's body text).
            let ly = body_y + row as f32 * cell_h + cell_h * 0.5;
            push(
                &mut objects,
                ObjectKind::Text {
                    origin: Point2::new(8.0, ly),
                    text: format!("R{row:02}"),
                    font_family: "sans-serif".to_owned(),
                    font_size: 9.0,
                },
                Style::filled(Color::rgba(0.78, 0.82, 0.90, 0.85)),
            );
            if objects.len() >= body_budget {
                break;
            }
        }

        let cx = body_x + col as f32 * cell_w;
        let cy = body_y + row as f32 * cell_h;
        let pad = (cell_w.min(cell_h) * 0.1).clamp(1.0, 6.0);
        let card_idx = row * cols + col;
        let hue = ((card_idx * 11) % 360) as f32 / 360.0;

        // Card background panel.
        push(
            &mut objects,
            ObjectKind::Rect(Rect::new(
                cx + pad,
                cy + pad,
                (cell_w - pad * 2.0).max(1.0),
                (cell_h - pad * 2.0).max(1.0),
            )),
            Style::filled(Color::rgba(1.0, 1.0, 1.0, 1.0)),
        );
        // Progress track.
        let track_y = cy + cell_h * 0.62;
        let track_w = (cell_w - pad * 2.0).max(1.0);
        push(
            &mut objects,
            ObjectKind::Rect(Rect::new(
                cx + pad,
                track_y,
                track_w,
                (cell_h * 0.1).max(1.0),
            )),
            Style::filled(Color::rgba(0.90, 0.92, 0.95, 1.0)),
        );
        // Progress fill (deterministic pseudo-random length).
        let frac = 0.25 + ((card_idx * 37 % 100) as f32 / 100.0) * 0.7;
        push(
            &mut objects,
            ObjectKind::Rect(Rect::new(
                cx + pad,
                track_y,
                (track_w * frac).max(1.0),
                (cell_h * 0.1).max(1.0),
            )),
            Style::filled(hsl(hue, 0.6, 0.52)),
        );
        // Divider line near the bottom of the card.
        let ly = cy + cell_h - pad;
        push(
            &mut objects,
            ObjectKind::Line {
                start: Point2::new(cx + pad, ly),
                end: Point2::new(cx + cell_w - pad, ly),
            },
            Style::stroked(Stroke::new(Color::rgba(0.85, 0.87, 0.92, 1.0), 1.0)),
        );

        col += 1;
        if col >= cols {
            col = 0;
            row += 1;
        }
    }

    // --- Selection marker (the edit target) -------------------------
    // Fixed, on-screen, opaque, top of the z-order. Toggling its colour
    // dirties exactly this rectangle.
    let marker_rect = Rect::new(body_x + 24.0, body_y + 18.0, 64.0, 28.0);
    let marker = Object::new(ObjectKind::Rect(marker_rect), Style::filled(MARKER_OFF)).with_z(z);
    let marker_id = marker.id;
    objects.push(marker);

    let mut scene = Scene::new(Color::rgba(0.96, 0.97, 0.98, 1.0));
    scene.add_objects(objects);

    DenseDoc { scene, marker_id }
}

/// Nodes contributed by one metric card (panel, track, fill, divider).
const NODES_PER_CARD: usize = 4;

/// Flip the selection marker's fill colour. This is the canonical
/// single-element edit: only the marker's pixels change between frames,
/// so the presenter's pixel diff resolves to the marker rect.
pub(crate) fn toggle_marker(scene: &mut Scene, marker_id: ObjectId, on: bool) {
    let colour = if on { MARKER_ON } else { MARKER_OFF };
    for obj in &mut scene.objects {
        if obj.id == marker_id {
            obj.style.fill = Some(Paint::Solid(colour));
            return;
        }
    }
}

/// Minimal HSL→RGB (all inputs in `0..=1`) for the dashboard palette.
fn hsl(h: f32, s: f32, l: f32) -> Color {
    let h = h.rem_euclid(1.0);
    let c = (1.0 - (2.0f32.mul_add(l, -1.0)).abs()) * s;
    let hp = h * 6.0;
    let x = c * (1.0 - ((hp % 2.0) - 1.0).abs());
    let (r1, g1, b1) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c * 0.5;
    Color::rgba(r1 + m, g1 + m, b1 + m, 1.0)
}
