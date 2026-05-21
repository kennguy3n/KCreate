use criterion::{black_box, criterion_group, criterion_main, Criterion};
use kcreate_ai::{remove_background, BgRemoveOptions};

fn build_image(w: u32, h: u32) -> Vec<u8> {
    let mut v = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let bg = x < 4 || x > w - 4 || y < 4 || y > h - 4;
            if bg {
                v.extend_from_slice(&[240, 240, 240, 255]);
            } else {
                v.extend_from_slice(&[80, 100, 120, 255]);
            }
        }
    }
    v
}

fn bench_bg_remove_512(c: &mut Criterion) {
    let img = build_image(512, 512);
    c.bench_function("bg_remove_threshold_512", |b| {
        b.iter(|| {
            let out = remove_background(black_box(&img), 512, 512, BgRemoveOptions::default())
                .expect("ok");
            black_box(out);
        });
    });
}

criterion_group!(benches, bench_bg_remove_512);
criterion_main!(benches);
