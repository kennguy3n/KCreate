//! Phase 6 Task 30 — cold-start acceptance criterion.
//!
//! OVERVIEW.md §8 lists a target cold-start budget of `< 3 s` on
//! Tier 0 and `< 1 s` on Tier 3. The full cold-start path involves
//! Electron + Node + Vite + the bridge `.node` cdylib, none of which
//! we can drive from a `cargo bench`. What we *can* measure — and
//! what dominates the budget on every device tier — is the
//! `RenderContext` initialisation, which compiles the wgpu pipelines
//! (or, in CPU-only mode, sets up the tiny-skia fallback). This
//! bench reports that cost in isolation so we can spot regressions
//! before they show up as full-app cold-start regressions.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use kcreate_renderer::initialize;

fn bench_renderer_initialize(c: &mut Criterion) {
    // Use a small viewport so the bench is dominated by pipeline /
    // adapter setup rather than initial allocation. Real cold-start
    // creates the context once per session; we just need a stable
    // signal here.
    c.bench_function("renderer_initialize_64x64", |b| {
        b.iter(|| {
            let ctx = initialize(black_box(64), black_box(64)).expect("init");
            // `black_box` forces the optimiser to materialise the
            // context so it can't be elided.
            black_box(&ctx);
        });
    });
}

criterion_group!(benches, bench_renderer_initialize);
criterion_main!(benches);
