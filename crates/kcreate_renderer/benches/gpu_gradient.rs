//! GPU vs CPU gradient fill generation.
//!
//! Measures the native WGSL gradient compute pipeline against the
//! scalar CPU reference that backs it (and serves as the offline
//! fallback). The GPU arm is skipped when no adapter is available
//! (headless CI, `KCREATE_DISABLE_GPU=1`) so the bench never panics.

use criterion::{criterion_group, criterion_main, Criterion};
use kcreate_renderer::compute::{
    cpu_render_gradient, GpuComputeContext, GradientKind, GradientSpec, GradientStop,
};

fn linear_spec(width: u32, height: u32) -> GradientSpec {
    GradientSpec {
        width,
        height,
        kind: GradientKind::Linear {
            from: [0.0, 0.0],
            to: [width as f32, height as f32],
        },
        stops: vec![
            GradientStop {
                offset: 0.0,
                color: [0.05, 0.10, 0.40, 1.0],
            },
            GradientStop {
                offset: 0.5,
                color: [0.90, 0.30, 0.10, 1.0],
            },
            GradientStop {
                offset: 1.0,
                color: [0.95, 0.95, 0.20, 1.0],
            },
        ],
    }
}

fn bench_gradient(c: &mut Criterion) {
    let gpu = match GpuComputeContext::try_new() {
        Ok(ctx) => ctx,
        Err(err) => {
            eprintln!("gpu_gradient bench: wgpu init failed ({err}); CPU-only");
            None
        }
    };

    let mut group = c.benchmark_group("gpu_gradient");
    for &(w, h) in &[(1024u32, 1024u32), (2048, 2048)] {
        let spec = linear_spec(w, h);

        group.bench_function(format!("cpu_{w}x{h}"), |b| {
            b.iter(|| std::hint::black_box(cpu_render_gradient(std::hint::black_box(&spec))));
        });

        if let Some(ctx) = gpu.as_ref() {
            // Warm up so the first iteration doesn't pay pipeline
            // first-use / allocation cost.
            let _ = ctx.render_gradient(&spec).expect("warmup gradient");
            group.bench_function(format!("gpu_{w}x{h}"), |b| {
                b.iter(|| {
                    let out = ctx
                        .render_gradient(std::hint::black_box(&spec))
                        .expect("gpu gradient");
                    std::hint::black_box(out.len());
                });
            });
        }
    }
    group.finish();
}

criterion_group!(benches, bench_gradient);
criterion_main!(benches);
