//! Throughput benchmark for [`kcreate_export::run_preflight`].
//!
//! Builds a synthetic 50-page document with a mix of vector, raster,
//! and text layers, then measures full-document preflight time.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use kcreate_core::document::DocumentGraph;
use kcreate_core::node::{
    Bounds, Node, NodeType, PageLayout, PageOrientation, PageSize, PAGE_LAYOUT_METADATA_KEY,
};
use kcreate_export::{
    run_preflight, PreflightOptions, RASTER_IMAGE_METADATA_KEY, TEXT_LAYER_METADATA_KEY,
};
use serde_json::json;

fn build_document(num_pages: usize) -> DocumentGraph {
    let mut doc = DocumentGraph::new();
    for i in 0..num_pages {
        let mut page = Node::new(NodeType::Page, format!("Page {i}"));
        page.bounds = Bounds::new(0.0, 0.0, 2480.0, 3508.0);
        page.metadata.insert(
            PAGE_LAYOUT_METADATA_KEY.to_string(),
            serde_json::to_value(PageLayout::new(PageSize::A4, PageOrientation::Portrait)).unwrap(),
        );
        let page_id = doc.insert_node(page).unwrap();

        for j in 0..6 {
            let mut layer = Node::new(NodeType::VectorLayer, format!("layer {j}"));
            layer.parent_id = Some(page_id);
            layer.bounds = Bounds::new(100.0 + f64::from(j) * 50.0, 100.0, 400.0, 400.0);
            doc.insert_node(layer).unwrap();
        }

        let mut raster = Node::new(NodeType::RasterLayer, "photo");
        raster.parent_id = Some(page_id);
        raster.bounds = Bounds::new(200.0, 1500.0, 1200.0, 1200.0);
        raster.metadata.insert(
            RASTER_IMAGE_METADATA_KEY.to_string(),
            json!({"blob_hash": "abc", "width": 1024, "height": 1024}),
        );
        doc.insert_node(raster).unwrap();

        let mut text = Node::new(NodeType::TextLayer, "title");
        text.parent_id = Some(page_id);
        text.bounds = Bounds::new(200.0, 200.0, 1500.0, 200.0);
        text.metadata.insert(
            TEXT_LAYER_METADATA_KEY.to_string(),
            json!({"text": "Hello", "font_family": "Inter", "font_size": 48.0}),
        );
        doc.insert_node(text).unwrap();
    }
    doc
}

fn bench_preflight_50_pages(c: &mut Criterion) {
    let doc = build_document(50);
    let opts = PreflightOptions::default();
    c.bench_function("preflight_50_pages", |b| {
        b.iter(|| {
            let v = run_preflight(black_box(&doc), &[], black_box(&opts));
            black_box(v);
        });
    });
}

criterion_group!(benches, bench_preflight_50_pages);
criterion_main!(benches);
