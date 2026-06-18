//! Cost of re-rendering after a viewport pan. Should be dominated by the
//! rasterization step, not display-list rebuild (display list is cached
//! across viewport changes).
//!
//! Phase 6 Task 30 — acceptance-criteria coverage. The
//! `viewport_pan_1000_nodes` group mirrors the 1000-node pan/zoom
//! target from OVERVIEW.md §20 (30 fps Tier 0, 60 fps Tier 1+). Pair
//! it with the `cold_start` bench in `kcreate_renderer/benches/` for
//! the renderer-init half of the cold-start budget.

use criterion::{criterion_group, criterion_main, Criterion};
use kcreate_renderer::{
    initialize, set_viewport, Color, Object, ObjectKind, Rect, Scene, Style, Vec2,
};

fn build_scene_n(n: usize) -> Scene {
    let mut s = Scene::new(Color::rgba(0.05, 0.05, 0.07, 1.0));
    for i in 0..n {
        let x = (i as f32 * 17.0) % 1920.0;
        let y = (i as f32 * 23.0) % 1080.0;
        s.add_object(Object::new(
            ObjectKind::Rect(Rect::new(x, y, 50.0, 50.0)),
            Style::filled(Color::rgba(0.4, 0.5, 0.7, 1.0)),
        ));
    }
    s
}

fn bench_pan(c: &mut Criterion) {
    let mut ctx = initialize(1920, 1080).expect("init");
    let scene = build_scene_n(200);
    // Initial frame so subsequent renders benchmark the pan path.
    ctx.invalidate_all();
    let _ = ctx.render_frame(&scene).expect("warmup");

    c.bench_function("viewport_pan_1920x1080", |b| {
        let mut t = 0.0f32;
        b.iter(|| {
            t += 1.0;
            set_viewport(&mut ctx, Vec2::new(t, t * 0.5), 1.0);
            let _ = ctx.render_frame(&scene).expect("render");
        });
    });
}

/// 1000-node pan benchmark — acceptance criterion from
/// OVERVIEW.md §20: 30 fps Tier 0, 60 fps Tier 1+. The bench reports
/// per-frame time; convert to fps via `1 / time_per_iter` to compare
/// against the target.
fn bench_pan_1000_nodes(c: &mut Criterion) {
    let mut ctx = initialize(1920, 1080).expect("init");
    let scene = build_scene_n(1000);
    ctx.invalidate_all();
    let _ = ctx.render_frame(&scene).expect("warmup");

    c.bench_function("viewport_pan_1000_nodes", |b| {
        let mut t = 0.0f32;
        b.iter(|| {
            t += 1.0;
            set_viewport(&mut ctx, Vec2::new(t, t * 0.5), 1.0);
            let _ = ctx.render_frame(&scene).expect("render");
        });
    });
}

/// Zoom + pan combined for the 1000-node scene — the acceptance
/// criterion calls out *pan / zoom* together, so we exercise the
/// scale axis on a separate function so the criterion HTML report
/// surfaces them side-by-side. Scaling forces every cached display
/// list to invalidate, so this is the worst-case path for the same
/// scene.
fn bench_zoom_1000_nodes(c: &mut Criterion) {
    let mut ctx = initialize(1920, 1080).expect("init");
    let scene = build_scene_n(1000);
    ctx.invalidate_all();
    let _ = ctx.render_frame(&scene).expect("warmup");

    c.bench_function("viewport_zoom_1000_nodes", |b| {
        let mut t = 0.0f32;
        b.iter(|| {
            t += 0.01;
            let zoom = 1.0 + (t.sin() * 0.5).abs();
            set_viewport(&mut ctx, Vec2::ZERO, zoom);
            let _ = ctx.render_frame(&scene).expect("render");
        });
    });
}

criterion_group!(
    benches,
    bench_pan,
    bench_pan_1000_nodes,
    bench_zoom_1000_nodes
);
criterion_main!(benches);
