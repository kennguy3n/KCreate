//! GPU → CPU pixel transfer time. Tracks the additional cost added by the
//! Phase 0 offscreen-readback presentation model.

use criterion::{criterion_group, criterion_main, Criterion};
use kcreate_renderer::{initialize, Color, Scene};

fn bench_readback(c: &mut Criterion) {
    let mut group = c.benchmark_group("frame_readback");
    for &(w, h) in &[(1920u32, 1080u32), (2560, 1440)] {
        group.bench_function(format!("{w}x{h}"), |b| {
            let ctx = initialize(w, h).expect("init");
            let scene = Scene::new(Color::rgba(0.0, 0.0, 0.0, 1.0));
            // Warm up so the first iteration doesn't pay first-render cost.
            ctx.invalidate_all();
            let _ = ctx.render_frame(&scene).expect("warmup");

            b.iter(|| {
                ctx.invalidate_all();
                let id = ctx.render_frame(&scene).expect("render");
                let lease = ctx.get_frame_pixels(id).expect("lease");
                // Touch the buffer so the optimizer can't elide the read.
                std::hint::black_box(lease.pixels().len());
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_readback);
criterion_main!(benches);
