//! Dense-document present-path benchmark (workstream I2).
//!
//! Builds a real 5,000- and 10,000-node analytics dashboard (gradients,
//! text, thousands of shapes) and measures the cost the host pays *per
//! presented frame* for a typical single-element edit:
//!
//! * `build` — `render_frame` (full CPU re-rasterisation; the same work
//!   for both present strategies, measured for context).
//! * `present_full` — `take_present(0.0)`: the legacy whole-framebuffer
//!   readback/copy the IPC layer used to ship every frame.
//! * `present_dirty` — `take_present(0.5)`: the dirty-rect path, which
//!   gathers and ships only the changed sub-region.
//!
//! The dirty-rect arms hand back a few KiB instead of the full
//! ~8.3 MB 1080p framebuffer, so the per-frame copy + IPC + `putImageData`
//! cost collapses. The `build` arm is reported so it is obvious the gain
//! is on the present path, not the (unchanged) rasteriser.

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use kcreate_renderer::dense_doc::{build_dense_document, toggle_marker, DenseDoc};
use kcreate_renderer::{initialize, RenderContext, Scene};

const WIDTH: u32 = 1920;
const HEIGHT: u32 = 1080;

/// Render `scene` as the next frame, forcing a real re-render.
fn render(ctx: &RenderContext, scene: &Scene) {
    ctx.invalidate_all();
    ctx.render_frame(scene).expect("render_frame");
}

/// Establish a published baseline frame and drain the first (always
/// full) present so subsequent diffs are against a real prior frame.
fn prime(ctx: &RenderContext, scene: &Scene) {
    render(ctx, scene);
    let _ = ctx.take_present(0.5);
}

fn bench_dense_present(c: &mut Criterion) {
    let mut group = c.benchmark_group("frame_present_dense");
    // Dense scenes take real time to rasterise; keep the sample count
    // modest so the suite finishes in CI-friendly wall time.
    group.sample_size(20);

    for &target in &[5_000usize, 10_000usize] {
        let DenseDoc {
            mut scene,
            marker_id,
        } = build_dense_document(target, WIDTH as f32, HEIGHT as f32);
        let node_count = scene.objects.len() as u64;

        // --- build (full re-rasterisation) --------------------------
        group.throughput(Throughput::Elements(node_count));
        group.bench_with_input(BenchmarkId::new("build", node_count), &target, |b, _| {
            let ctx = initialize(WIDTH, HEIGHT).expect("init");
            prime(&ctx, &scene);
            let mut on = false;
            b.iter(|| {
                on = !on;
                toggle_marker(&mut scene, marker_id, on);
                render(&ctx, &scene);
            });
        });

        // --- present_full (legacy whole-frame copy) -----------------
        group.bench_with_input(
            BenchmarkId::new("present_full", node_count),
            &target,
            |b, _| {
                let ctx = initialize(WIDTH, HEIGHT).expect("init");
                prime(&ctx, &scene);
                let mut on = false;
                b.iter(|| {
                    on = !on;
                    toggle_marker(&mut scene, marker_id, on);
                    render(&ctx, &scene);
                    // 0.0 fraction => never partial: always the full
                    // framebuffer copy the old path performed.
                    let snap = ctx.take_present(0.0).expect("present");
                    std::hint::black_box(snap.bytes.len());
                });
            },
        );

        // --- present_dirty (dirty-rect copy) ------------------------
        group.bench_with_input(
            BenchmarkId::new("present_dirty", node_count),
            &target,
            |b, _| {
                let ctx = initialize(WIDTH, HEIGHT).expect("init");
                prime(&ctx, &scene);
                let mut on = false;
                b.iter(|| {
                    on = !on;
                    toggle_marker(&mut scene, marker_id, on);
                    render(&ctx, &scene);
                    let snap = ctx.take_present(0.5).expect("present");
                    std::hint::black_box(snap.bytes.len());
                });
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_dense_present);
criterion_main!(benches);
