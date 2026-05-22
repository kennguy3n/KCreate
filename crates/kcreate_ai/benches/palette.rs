use criterion::{black_box, criterion_group, criterion_main, Criterion};
use kcreate_ai::extract_palette;

fn build_image(w: u32, h: u32) -> Vec<u8> {
    let mut v = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let r = (x * 255 / w.max(1)) as u8;
            let g = (y * 255 / h.max(1)) as u8;
            v.extend_from_slice(&[r, g, 80, 255]);
        }
    }
    v
}

fn bench_palette_1024(c: &mut Criterion) {
    let img = build_image(1024, 1024);
    c.bench_function("extract_palette_5_1024", |b| {
        b.iter(|| {
            let out = extract_palette(black_box(&img), 1024, 1024, 5);
            black_box(out);
        });
    });
}

criterion_group!(benches, bench_palette_1024);
criterion_main!(benches);
