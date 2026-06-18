//! Phase 6 Task 30 — 64-megapixel raster open acceptance bench.
//!
//! OVERVIEW.md §20 lists the "Open a 64 MP raster" target as:
//! < 4 s Tier 0, < 2 s Tier 1, < 1 s Tier 2+. The path measured here
//! is `TileGrid::from_image` over an 8192×8192 RGBA8 buffer (~64 MP,
//! 256 MiB pre-tile pixels), which is the worst-case import on a
//! freshly-allocated grid — every tile slot must be filled. We
//! exclude the JPEG/PNG decode step intentionally so this isolates
//! the tile-grid construction; decode is amortised across every
//! supported source format and lives in `image::ImageReader`.

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use kcreate_raster::tile::TileGrid;

const W: u32 = 8192;
const H: u32 = 8192;
const TILE: u32 = 256;

fn make_rgba(w: u32, h: u32) -> Vec<u8> {
    // Deterministic gradient — not all zero so the tile-grid path
    // can't shortcut on a "blank tile" heuristic if one is added in
    // the future.
    let mut buf = vec![0u8; (w as usize) * (h as usize) * 4];
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            buf[i] = (x & 0xFF) as u8;
            buf[i + 1] = (y & 0xFF) as u8;
            buf[i + 2] = ((x ^ y) & 0xFF) as u8;
            buf[i + 3] = 255;
        }
    }
    buf
}

fn bench_open_64mp(c: &mut Criterion) {
    let rgba = make_rgba(W, H);
    let mut group = c.benchmark_group("raster_open");
    group.throughput(Throughput::Bytes(rgba.len() as u64));
    group.sample_size(10); // 64 MP allocations are heavy.
    group.bench_with_input(
        BenchmarkId::new("from_image_8192x8192", "64MP"),
        &rgba,
        |b, buf| {
            b.iter(|| {
                let grid = TileGrid::from_image(buf, W, H, TILE).expect("grid");
                // Force consumption so the optimiser can't elide it.
                criterion::black_box(grid);
            });
        },
    );
    group.finish();
}

criterion_group!(benches, bench_open_64mp);
criterion_main!(benches);
