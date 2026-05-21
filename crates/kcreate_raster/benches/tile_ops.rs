use criterion::{black_box, criterion_group, criterion_main, Criterion};
use kcreate_raster::{AdjustmentLayer, BlendMode, RasterLayer, TileGrid};

fn build_image(width: u32, height: u32) -> Vec<u8> {
    let mut v = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            v.push(((x ^ y) & 0xFF) as u8);
            v.push(((x + y) & 0xFF) as u8);
            v.push(0x80);
            v.push(0xFF);
        }
    }
    v
}

fn bench_from_image(c: &mut Criterion) {
    let pixels = build_image(512, 512);
    c.bench_function("tile_grid_from_image_512", |b| {
        b.iter(|| {
            let g = TileGrid::from_image(black_box(&pixels), 512, 512, 64).expect("grid");
            black_box(g);
        });
    });
}

fn bench_to_image(c: &mut Criterion) {
    let pixels = build_image(512, 512);
    let grid = TileGrid::from_image(&pixels, 512, 512, 64).expect("grid");
    c.bench_function("tile_grid_to_image_512", |b| {
        b.iter(|| {
            let out = grid.to_image();
            black_box(out);
        });
    });
}

fn bench_render_adjustments(c: &mut Criterion) {
    let pixels = build_image(256, 256);
    let grid = TileGrid::from_image(&pixels, 256, 256, 64).expect("grid");
    let mut layer = RasterLayer::new(256, 256, 64).expect("layer");
    layer.grid = grid;
    layer.adjustments = vec![
        AdjustmentLayer::Brightness(0.1),
        AdjustmentLayer::Contrast(1.2),
    ];
    layer.blend_mode = BlendMode::Normal;
    c.bench_function("raster_layer_render_256_adjusted", |b| {
        b.iter(|| {
            let out = layer.render_rgba();
            black_box(out);
        });
    });
}

criterion_group!(
    benches,
    bench_from_image,
    bench_to_image,
    bench_render_adjustments
);
criterion_main!(benches);
