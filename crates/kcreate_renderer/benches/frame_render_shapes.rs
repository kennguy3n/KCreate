//! Time to render N vector shapes at 1080p.

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use kcreate_renderer::{initialize, Color, Object, ObjectKind, Rect, Scene, Style};

fn build_scene(n: usize) -> Scene {
    let mut scene = Scene::new(Color::rgba(0.1, 0.1, 0.12, 1.0));
    let side = (n as f32).sqrt().ceil() as u32;
    let cell = 1920.0 / side as f32;
    let mut i = 0u64;
    for y in 0..side {
        for x in 0..side {
            if i as usize >= n {
                break;
            }
            let fill = Color::rgba(
                (x as f32 / side as f32).clamp(0.0, 1.0),
                (y as f32 / side as f32).clamp(0.0, 1.0),
                0.5,
                1.0,
            );
            scene.add_object(
                Object::new(
                    ObjectKind::Rect(Rect::new(
                        x as f32 * cell,
                        y as f32 * cell,
                        cell * 0.8,
                        cell * 0.8,
                    )),
                    Style::filled(fill),
                )
                .with_z(i as i32),
            );
            i += 1;
        }
    }
    scene
}

fn bench_shapes(c: &mut Criterion) {
    let mut group = c.benchmark_group("frame_render_shapes");
    for &n in &[100usize, 1_000, 10_000] {
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            let ctx = initialize(1920, 1080).expect("init");
            let scene = build_scene(n);
            b.iter(|| {
                ctx.invalidate_all();
                let _ = ctx.render_frame(&scene).expect("render");
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_shapes);
criterion_main!(benches);
