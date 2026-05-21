//! Baseline GPU/CPU overhead: cost of rendering an empty canvas frame.

use criterion::{criterion_group, criterion_main, Criterion};
use kcreate_renderer::{initialize, Color, Scene};

fn bench_empty(c: &mut Criterion) {
    let mut group = c.benchmark_group("frame_render_empty");
    for &(w, h) in &[(800u32, 600u32), (1920, 1080), (2560, 1440)] {
        group.bench_function(format!("{w}x{h}"), |b| {
            let ctx = initialize(w, h).expect("init");
            let scene = Scene::new(Color::rgba(0.05, 0.05, 0.07, 1.0));
            b.iter(|| {
                ctx.invalidate_all();
                let _ = ctx.render_frame(&scene).expect("render");
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_empty);
criterion_main!(benches);
