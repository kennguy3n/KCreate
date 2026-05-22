use criterion::{black_box, criterion_group, criterion_main, Criterion};
use kcreate_ai::upscale_lanczos;

fn build_image(w: u32, h: u32) -> Vec<u8> {
    let mut v = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let r = (x * 255 / w.max(1)) as u8;
            let g = (y * 255 / h.max(1)) as u8;
            v.extend_from_slice(&[r, g, 128, 255]);
        }
    }
    v
}

fn bench_upscale_2x_512(c: &mut Criterion) {
    let img = build_image(512, 512);
    c.bench_function("upscale_lanczos_2x_512", |b| {
        b.iter(|| {
            let out = upscale_lanczos(black_box(&img), 512, 512, 2.0).expect("ok");
            black_box(out);
        });
    });
}

criterion_group!(benches, bench_upscale_2x_512);
criterion_main!(benches);
