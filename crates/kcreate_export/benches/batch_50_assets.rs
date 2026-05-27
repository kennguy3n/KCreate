//! Acceptance-criteria benchmark: **50-asset batch export**.
//!
//! PROPOSAL.md §20 lists "50-asset batch export time" alongside the
//! other Tier 0 performance acceptance criteria (cold start, pan/zoom
//! at 1000 nodes, 64 MP raster open). This benchmark fills the slot
//! by measuring the wall-clock time for `run_batch` (sequential) and
//! `run_batch_parallel` (rayon) against a 50-item SVG export.
//!
//! Why SVG over PDF: SVG is the cheapest leaf to exercise the batch
//! *scheduler* — the time we're measuring is dominated by per-item
//! overhead (filename resolution, error packing, parallel scheduler
//! ordering, mutex contention on progress reporting), not the
//! per-format render itself. The PDF path is benched separately in
//! crates/kcreate_export/src/pdf.rs's `cargo bench` integration.
//!
//! The criterion harness reports both means and standard deviations,
//! so a regression of even ~5% on the 50-asset path will surface in
//! the report. The output directory is a fresh `tempfile::TempDir`
//! created once per iteration so we measure file-creation cost as
//! well — that's part of what users actually pay for.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use kcreate_core::document::DocumentGraph;
use kcreate_core::node::{Bounds, Node, NodeType};
use kcreate_export::{
    run_batch, run_batch_parallel, BatchExportJob, BatchStatus, ExportItem, RasterPixelCache,
    SvgExportOptions, VECTOR_PATH_METADATA_KEY,
};
use uuid::Uuid;

/// Build a small document with a handful of vector rects so each
/// SVG export item has actual content to serialise. Keeps the
/// per-item render cost small so the scheduler dominates the run.
fn build_document() -> DocumentGraph {
    let mut doc = DocumentGraph::new();
    let mut page = Node::new(NodeType::Page, "page");
    page.bounds = Bounds::new(0.0, 0.0, 2480.0, 3508.0);
    let page_id = doc.insert_node(page).unwrap();
    for i in 0..6 {
        let mut layer = Node::new(NodeType::VectorLayer, format!("rect {i}"));
        layer.parent_id = Some(page_id);
        layer.bounds = Bounds::new(100.0 + f64::from(i) * 50.0, 100.0, 400.0, 400.0);
        let path = serde_json::json!({
            "kind": "rect",
            "x": 0.0,
            "y": 0.0,
            "width": 200.0,
            "height": 200.0,
        });
        layer
            .metadata
            .insert(VECTOR_PATH_METADATA_KEY.to_string(), path);
        doc.insert_node(layer).unwrap();
    }
    doc
}

/// Build a batch of `count` SVG export items.
fn build_job(output_dir: PathBuf, count: usize) -> BatchExportJob {
    let mut items = Vec::with_capacity(count);
    for i in 0..count {
        items.push(ExportItem::Svg {
            filename: format!("asset_{i:03}.svg"),
            node_ids: Vec::new(),
            options: SvgExportOptions::default(),
        });
    }
    BatchExportJob {
        id: Uuid::new_v4(),
        items,
        output_dir,
        status: BatchStatus::Pending,
    }
}

fn bench_batch_50_sequential(c: &mut Criterion) {
    let doc = build_document();
    let rasters = RasterPixelCache::new();

    let mut group = c.benchmark_group("batch_50_assets");
    group.throughput(Throughput::Elements(50));

    group.bench_function("sequential", |b| {
        b.iter_with_setup(
            || {
                let tmp = tempfile::tempdir().unwrap();
                let job = build_job(tmp.path().to_path_buf(), 50);
                (tmp, job)
            },
            |(_tmp, mut job)| {
                run_batch(&mut job, black_box(&doc), &rasters).expect("batch");
                let succeeded = match job.status {
                    BatchStatus::Done { succeeded, .. } => succeeded,
                    _ => 0,
                };
                black_box(succeeded);
            },
        );
    });

    group.finish();
}

fn bench_batch_50_parallel(c: &mut Criterion) {
    let doc = build_document();
    let rasters = RasterPixelCache::new();

    let mut group = c.benchmark_group("batch_50_assets");
    group.throughput(Throughput::Elements(50));

    group.bench_function("parallel", |b| {
        b.iter_with_setup(
            || {
                let tmp = tempfile::tempdir().unwrap();
                let job = build_job(tmp.path().to_path_buf(), 50);
                (tmp, job)
            },
            |(_tmp, job)| {
                let cancel = AtomicBool::new(false);
                let result = run_batch_parallel(&job, black_box(&doc), &rasters, &cancel, |_| {})
                    .expect("parallel batch");
                black_box(result.succeeded.len());
            },
        );
    });

    group.finish();
}

criterion_group!(benches, bench_batch_50_sequential, bench_batch_50_parallel);
criterion_main!(benches);
