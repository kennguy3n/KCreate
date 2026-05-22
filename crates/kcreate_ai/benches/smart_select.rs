use criterion::{black_box, criterion_group, criterion_main, Criterion};
use kcreate_ai::smart_select;

fn build_image(w: u32, h: u32) -> Vec<u8> {
    let mut v = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let inside = x > w / 4 && x < 3 * w / 4 && y > h / 4 && y < 3 * h / 4;
            let c = if inside { 200 } else { 50 };
            v.extend_from_slice(&[c, c, c, 255]);
        }
    }
    v
}

fn bench_smart_select_1024(c: &mut Criterion) {
    let img = build_image(1024, 1024);
    c.bench_function("smart_select_1024", |b| {
        b.iter(|| {
            let out = smart_select(black_box(&img), 1024, 1024, 512, 512, 0.05);
            black_box(out);
        });
    });
}

criterion_group!(benches, bench_smart_select_1024);
criterion_main!(benches);
