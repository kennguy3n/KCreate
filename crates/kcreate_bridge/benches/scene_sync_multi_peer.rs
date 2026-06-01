//! Multi-peer presence overlay micro-benchmarks (Phase 4 follow-up).
//!
//! Scene-sync is invoked on every collab tick — once per cursor /
//! selection / lock event, plus periodically from the session-event
//! pump — and the runtime cost of the two overlay-append paths
//! (`append_presence_cursors` + `append_presence_selection_halos`)
//! scales linearly in:
//!
//!     - the number of connected peers `P`
//!     - the per-peer selection set size `N`
//!     - the document node count `D` (for the halo path, because it
//!       calls `doc.get_node(*node_id)` once per selected id, which
//!       is O(1) on the underlying HashMap but still chases pointers
//!       and inflates the constant factor)
//!
//! These benches synthesise a 1, 10, 50, and 100-peer roster against
//! a 200-node document and time both the cursor and halo emit
//! paths in isolation, so a regression on either shows up
//! immediately without having to spin up a real collab session
//! (which would carry quinn / rustls / tokio overhead irrelevant
//! to the overlay-build cost we actually care about).

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use kcreate_bridge::scene_sync::{PresenceCursor, PresenceSelection, SceneSync};
use kcreate_core::node::{Bounds, Node, NodeType};
use kcreate_core::DocumentGraph;
use kcreate_renderer::{Color, Scene};
use kcreate_vector::path::{PathPoint, PathSegment, VectorPath};
use uuid::Uuid;

/// Peer-count axis. Picked so a regression in the *constant factor*
/// shows up on the 1-peer point and a regression in the
/// per-peer-cost shows up on the 100-peer point.
const PEER_COUNTS: &[usize] = &[1, 10, 50, 100];

/// How many node ids each peer "selects" in the benchmark. Real
/// multi-select in KCreate is usually <10; the 20 here gives a
/// little extra signal on the inner loop.
const SELECTION_SET_SIZE: usize = 20;

/// Number of vector nodes in the synthetic document. Each is given
/// distinct world bounds so `node_world_bounds` produces a
/// non-degenerate rect (the halo emit early-exits on invisible
/// nodes, not on zero-area ones, so any positive bounds would do —
/// but realistic spacing keeps the bench representative of the
/// actual workload).
const DOC_NODE_COUNT: usize = 200;

fn build_document() -> (DocumentGraph, Vec<Uuid>) {
    let mut doc = DocumentGraph::new();
    let mut ids = Vec::with_capacity(DOC_NODE_COUNT);
    for i in 0..DOC_NODE_COUNT {
        let mut node = Node::new(NodeType::VectorLayer, format!("n{i}"));
        // Spread nodes out so halo bounds don't all overlap on the
        // origin. The exact layout doesn't matter for the bench;
        // we just need each node's world bounds to differ.
        let row = i / 20;
        let col = i % 20;
        node.bounds = Bounds {
            x: (col as f64) * 50.0,
            y: (row as f64) * 50.0,
            width: 40.0,
            height: 30.0,
        };
        let id = doc.insert_node(node).expect("insert vector node");
        ids.push(id);
    }
    (doc, ids)
}

fn build_cursors(peer_count: usize) -> Vec<PresenceCursor> {
    (0..peer_count)
        .map(|i| PresenceCursor {
            // Stable, unique peer id so `peer_color` derives a
            // distinct hue for each.
            peer_id: format!("peer-{i:04}"),
            display_name: format!("Peer {i}"),
            x: (i as f64) * 13.0,
            y: (i as f64) * 17.0,
        })
        .collect()
}

fn build_selections(peer_count: usize, node_ids: &[Uuid]) -> Vec<PresenceSelection> {
    (0..peer_count)
        .map(|peer_idx| {
            // Each peer "selects" `SELECTION_SET_SIZE` ids, rotated
            // by their index so peers don't all halo the same
            // 20 nodes (which would short-circuit the inner loop
            // away from the realistic workload).
            let mut ids = Vec::with_capacity(SELECTION_SET_SIZE);
            for k in 0..SELECTION_SET_SIZE {
                let idx = (peer_idx * 7 + k * 11) % node_ids.len();
                ids.push(node_ids[idx]);
            }
            PresenceSelection {
                peer_id: format!("peer-{peer_idx:04}"),
                display_name: format!("Peer {peer_idx}"),
                node_ids: ids,
            }
        })
        .collect()
}

fn bench_presence_cursors(c: &mut Criterion) {
    let mut group = c.benchmark_group("scene_sync_presence_cursors");
    for &p in PEER_COUNTS {
        let cursors = build_cursors(p);
        // `cursors_emitted_per_iter` = each cursor emits 2 scene
        // objects (triangle + label). Wire it into Throughput so
        // criterion reports a per-cursor cost too.
        group.throughput(Throughput::Elements(p as u64));
        group.bench_with_input(BenchmarkId::from_parameter(p), &cursors, |b, cursors| {
            b.iter(|| {
                let mut sync = SceneSync::new();
                let mut scene = Scene::new(Color::rgba(0.05, 0.05, 0.07, 1.0));
                sync.append_presence_cursors(&mut scene, cursors, 0, 1.0);
                criterion::black_box(scene.objects.len());
            });
        });
    }
    group.finish();
}

fn bench_presence_selection_halos(c: &mut Criterion) {
    let (doc, node_ids) = build_document();
    let mut group = c.benchmark_group("scene_sync_presence_selection_halos");
    for &p in PEER_COUNTS {
        let selections = build_selections(p, &node_ids);
        // Each peer-selection emits up to `SELECTION_SET_SIZE`
        // halos + 1 label, so the element count for throughput is
        // peer × set so the per-halo cost is directly visible.
        group.throughput(Throughput::Elements((p * SELECTION_SET_SIZE) as u64));
        group.bench_with_input(BenchmarkId::from_parameter(p), &selections, |b, sel| {
            b.iter(|| {
                let mut sync = SceneSync::new();
                let mut scene = Scene::new(Color::rgba(0.05, 0.05, 0.07, 1.0));
                let _ = sync.append_presence_selection_halos(&mut scene, &doc, sel, 0, 1.0);
                criterion::black_box(scene.objects.len());
            });
        });
    }
    group.finish();
}

fn bench_combined_presence_pipeline(c: &mut Criterion) {
    // The real bridge tick always emits cursors AND halos in the
    // same scene-sync, so the combined micro-bench measures the
    // *actual* per-frame cost — including the watermark-resume
    // handshake between the two append paths.
    let (doc, node_ids) = build_document();
    let mut group = c.benchmark_group("scene_sync_presence_combined");
    for &p in PEER_COUNTS {
        let cursors = build_cursors(p);
        let selections = build_selections(p, &node_ids);
        group.throughput(Throughput::Elements(p as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(p),
            &(cursors, selections),
            |b, (cursors, sel)| {
                b.iter(|| {
                    let mut sync = SceneSync::new();
                    let mut scene = Scene::new(Color::rgba(0.05, 0.05, 0.07, 1.0));
                    let next_z =
                        sync.append_presence_selection_halos(&mut scene, &doc, sel, 0, 1.0);
                    sync.append_presence_cursors(&mut scene, cursors, next_z, 1.0);
                    criterion::black_box(scene.objects.len());
                });
            },
        );
    }
    group.finish();
}

/// Document node-count axis for the dense `sync_document_to_scene`
/// bench. Picked so the per-insert sort cost (which used to
/// dominate via the per-call `Scene::add_object` re-sort) shows up
/// at the 1000-node point.
const DOC_NODE_COUNTS: &[usize] = &[50, 200, 1000];

fn build_document_of_size(node_count: usize) -> DocumentGraph {
    let mut doc = DocumentGraph::new();
    for i in 0..node_count {
        let mut node = Node::new(NodeType::VectorLayer, format!("n{i}"));
        let row = i / 20;
        let col = i % 20;
        node.bounds = Bounds {
            x: (col as f64) * 50.0,
            y: (row as f64) * 50.0,
            width: 40.0,
            height: 30.0,
        };
        // Inject a minimal VectorPath metadata blob so `emit_vector`
        // actually emits an object (without it the node is skipped
        // and the bench measures only the walk, not the insert).
        // Build the real `VectorPath` and round-trip through serde
        // so we don't have to hand-author the tagged-enum JSON shape.
        let path = VectorPath::new(vec![
            PathSegment::MoveTo(PathPoint::new(0.0, 0.0)),
            PathSegment::LineTo(PathPoint::new(40.0, 0.0)),
            PathSegment::LineTo(PathPoint::new(40.0, 30.0)),
            PathSegment::LineTo(PathPoint::new(0.0, 30.0)),
            PathSegment::Close,
        ]);
        node.metadata.insert(
            "vector_path".to_string(),
            serde_json::to_value(&path).expect("serialise vector path for bench"),
        );
        doc.insert_node(node).expect("insert vector node");
    }
    doc
}

fn bench_sync_document_dense(c: &mut Criterion) {
    // Times the `sync_document_to_scene` hot path on documents of
    // increasing node count. Block B of the post-PR-#11 follow-ups
    // converted the recursive emit walk from per-node
    // `Scene::add_object` (O(N²·log N) via the re-sort on each
    // insert) to a single batched `add_objects` at the end of the
    // walk (O(N·log N)). This bench is the regression guard: if
    // anyone re-introduces a per-insert sort, the 1000-node point
    // pops loudly.
    let mut group = c.benchmark_group("scene_sync_document_dense");
    for &count in DOC_NODE_COUNTS {
        let doc = build_document_of_size(count);
        group.throughput(Throughput::Elements(count as u64));
        group.bench_with_input(BenchmarkId::from_parameter(count), &doc, |b, doc| {
            b.iter(|| {
                let mut sync = SceneSync::new();
                let scene = sync.sync_document_to_scene_borrowed(doc, None, &[]);
                criterion::black_box(scene.objects.len());
            });
        });
    }
    group.finish();
}

fn bench_sync_document_steady_state(c: &mut Criterion) {
    // Phase E target — steady-state replay cost.
    //
    // `bench_sync_document_dense` above uses
    // `sync_document_to_scene_borrowed`, which clears the per-node
    // cache on every call (cold path). That is the *worst case* —
    // the path the bridge takes on the first sync of a freshly
    // opened document. In production the bridge takes the
    // `sync_document_to_scene` path on every redraw, and most of
    // those redraws have an empty dirty set (collab presence tick,
    // hover overlay swap, viewport pan with no document edit), so
    // every leaf node hits `SceneSync::try_replay_cached` and the
    // recursive walk never visits leaf-emit metadata again.
    //
    // This bench measures *that* path: build a doc, warm the cache
    // with one sync, then in the iter loop call
    // `sync_document_to_scene` with an empty dirty set so the
    // replay path runs end-to-end. The replay path's per-node cost
    // is the budget for Phase E perf fixes (eliminating wasted
    // allocations in the reverse-map rebuild and the per-cache-hit
    // clones). Any future regression that turns a cache hit back
    // into a re-emit (or that re-introduces per-sync map clones)
    // will pop loudly here, especially at the 1000-node point.
    let mut group = c.benchmark_group("scene_sync_document_steady_state");
    for &count in DOC_NODE_COUNTS {
        group.throughput(Throughput::Elements(count as u64));
        group.bench_with_input(BenchmarkId::from_parameter(count), &count, |b, &count| {
            // Build the doc + SceneSync once per parameter and
            // reuse them across all iterations of `b.iter()`. After
            // the warm-up sync below, the document has no pending
            // edits, so every subsequent `drain_dirty()` returns
            // empty and every node hits `try_replay_cached`. That
            // is precisely the steady-state path we want to measure
            // — reusing one warmed SceneSync is the *whole point*
            // (a per-iter rebuild would re-emit from scratch and
            // turn this back into a cold-path bench).
            let mut doc = build_document_of_size(count);
            let mut sync = SceneSync::new();
            // Warm the cache once — populates `node_cache`
            // and `last_version` for every node.
            let _ = sync.sync_document_to_scene(&mut doc, None, &[]);
            b.iter(|| {
                // Empty dirty set + unchanged versions =>
                // every node hits the replay path.
                let scene = sync.sync_document_to_scene(&mut doc, None, &[]);
                criterion::black_box(scene.objects.len());
            });
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_presence_cursors,
    bench_presence_selection_halos,
    bench_combined_presence_pipeline,
    bench_sync_document_dense,
    bench_sync_document_steady_state,
);
criterion_main!(benches);
