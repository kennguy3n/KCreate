//! Cost of re-rendering after a viewport pan. Should be dominated by the
//! rasterization step, not display-list rebuild (display list is cached
//! across viewport changes).

use criterion::{criterion_group, criterion_main, Criterion};
use kcreate_renderer::{
    initialize, set_viewport, Color, Object, ObjectKind, Rect, Scene, Style, Vec2,
};

fn build_scene() -> Scene {
    let mut s = Scene::new(Color::rgba(0.05, 0.05, 0.07, 1.0));
    for i in 0..200 {
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
    let scene = build_scene();
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

criterion_group!(benches, bench_pan);
criterion_main!(benches);
