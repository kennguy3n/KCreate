//! Shared-memory present-path benchmark (native / shared-memory
//! workstream).
//!
//! Builds the same real 5,000- and 10,000-node analytics dashboard the
//! renderer's `frame_present_dense` bench uses, then measures the cost
//! the host pays *per presented frame* for a **full-frame** update
//! (pan / zoom / scroll — the case the dirty-rect path cannot shrink)
//! under two present strategies:
//!
//! * `ipc_full_copy` — the legacy IPC path. Every full frame is
//!   materialised as a fresh `width*height*4` framebuffer `Vec` (exactly
//!   what `presenter::take_present(0.0)` does at `published.bytes.clone()`)
//!   and then handed to Electron, which structured-clones those bytes
//!   across the main→renderer process boundary. The clone measured here
//!   is the *floor* of that path: the real IPC path additionally pays the
//!   structured-clone serialise + cross-process transfer, which this arm
//!   deliberately does **not** count (so the comparison is conservative).
//! * `shared_publish` — the shared-memory path, publisher side: copy the
//!   full frame into a pre-mapped ring slot under the seqlock. No heap
//!   allocation, and **zero bytes cross IPC** — the renderer process maps
//!   the same backing file and reads it directly.
//! * `shared_publish_read` — the shared-memory path end-to-end: the
//!   publisher copy above plus the renderer-side `read_latest_into` copy
//!   out of the ring. This is the *total* data movement of the shared
//!   path across both processes; it replaces the IPC serialise +
//!   deserialise round-trip, still moving nothing over IPC.
//!
//! Headline: the IPC path moves `width*height*4` bytes over IPC every
//! full frame (8.3 MB at 1080p); the shared path moves 0.

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::hint::black_box;

use kcreate_bridge::shared_present::{SharedFramePublisher, SharedFrameReader};
use kcreate_renderer::initialize;

// Only the dense-document *builder* is used here (the present handoff is
// what we time, not a marker edit), so the shared helper's edit utility
// is dead code in this target.
#[allow(dead_code)]
#[path = "../../kcreate_renderer/benches/common/dense_doc.rs"]
mod dense_doc;

use dense_doc::{build_dense_document, DenseDoc};

const WIDTH: u32 = 1920;
const HEIGHT: u32 = 1080;
const SLOT_COUNT: u32 = 3;

fn bench_shared_present(c: &mut Criterion) {
    let mut group = c.benchmark_group("shared_present_dense");
    group.sample_size(30);

    let ipc_bytes = WIDTH as usize * HEIGHT as usize * 4;
    eprintln!(
        "shared_present_dense: per full frame the IPC path ships {ipc_bytes} bytes \
         ({:.2} MiB) over IPC; the shared path ships 0.",
        ipc_bytes as f64 / (1024.0 * 1024.0)
    );

    for &target in &[5_000usize, 10_000usize] {
        let DenseDoc { scene, .. } = build_dense_document(target, WIDTH as f32, HEIGHT as f32);
        let node_count = scene.objects.len() as u64;

        // Render the dense frame once; both strategies present the very
        // same pixels, so the (shared) render cost is hoisted out of the
        // measured loop — we are timing the present handoff, not the
        // rasteriser.
        let ctx = initialize(WIDTH, HEIGHT).expect("init renderer");
        ctx.invalidate_all();
        ctx.render_frame(&scene).expect("render dense frame");
        let frame_pixels: Vec<u8> = ctx.latest_frame().expect("latest frame").pixels().to_vec();
        assert_eq!(frame_pixels.len(), ipc_bytes, "full 1080p framebuffer");

        group.throughput(Throughput::Bytes(ipc_bytes as u64));

        // --- ipc_full_copy (legacy: full-frame Vec the IPC layer ships)
        group.bench_with_input(
            BenchmarkId::new("ipc_full_copy", node_count),
            &target,
            |b, _| {
                b.iter(|| {
                    // `take_present(0.0)`'s full-frame branch is a clone
                    // of the published framebuffer; this is the per-frame
                    // allocation + copy the IPC path performs before the
                    // (uncounted) structured-clone across the boundary.
                    // Both ends are `black_box`'d so the alloc + memcpy is
                    // actually performed and not elided.
                    let copy = black_box(&frame_pixels).clone();
                    black_box(copy);
                });
            },
        );

        // --- shared_publish (shared path, publisher side) -----------
        group.bench_with_input(
            BenchmarkId::new("shared_publish", node_count),
            &target,
            |b, _| {
                let mut publisher =
                    SharedFramePublisher::create(WIDTH, HEIGHT, SLOT_COUNT).expect("publisher");
                let mut frame_id: u64 = 0;
                b.iter(|| {
                    frame_id += 1;
                    let ok = publisher.publish_full(frame_id, black_box(&frame_pixels));
                    black_box(ok);
                });
            },
        );

        // --- shared_publish_read (shared path, end-to-end) ----------
        group.bench_with_input(
            BenchmarkId::new("shared_publish_read", node_count),
            &target,
            |b, _| {
                let mut publisher =
                    SharedFramePublisher::create(WIDTH, HEIGHT, SLOT_COUNT).expect("publisher");
                let reader = SharedFrameReader::open(&publisher.descriptor()).expect("reader");
                let mut dest = vec![0u8; ipc_bytes];
                let mut frame_id: u64 = 0;
                b.iter(|| {
                    frame_id += 1;
                    publisher.publish_full(frame_id, black_box(&frame_pixels));
                    let meta = reader.read_latest_into(&mut dest).expect("read");
                    black_box(meta.map(|m| m.frame_id));
                });
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_shared_present);
criterion_main!(benches);
