//! Phase 7 (Task 28): collaboration performance benchmarks.
//!
//! These benches measure the hot paths exercised during a real
//! collaborative editing session, without spinning up the QUIC
//! transport (which would add tokio + rustls + mdns-sd overhead
//! irrelevant to the journal / merge / serialisation work we
//! actually care about). The signal we want to preserve over
//! time is:
//!
//! - `journal_append_throughput` — simulated 10-peer session,
//!   100 ops/s aggregate (10 ops/peer/s). Measures the cost of
//!   appending a single op into [`OperationJournal`] over an
//!   in-memory store, which dominates per-op CPU on the host
//!   side. Throughput is reported in ops/s.
//! - `crdt_merge_latency` — two-peer concurrent edit pair on the
//!   same node. Measures the cost of one call to
//!   [`CrdtResolver::resolve_crdt`] — the function called once
//!   per inbound op when both sides touched the same node. We
//!   sample both the disjoint-key merge path (cheap) and the
//!   overlap LWW path (cheapest) so a regression on either
//!   shows up.
//! - `presence_serialization` — 5-peer presence map at 20 Hz.
//!   Measures the cost of building one
//!   [`PresencePayload`] + signing it into an
//!   [`Envelope<Message::Presence>`], which is the dominant CPU
//!   cost per presence broadcast.
//! - `resume_bundle_serialize_apply` — 10 000-entry resume
//!   bundle. Measures the time to (a) build a
//!   [`ResumeBundlePayload`] from a hot journal, (b) serialize
//!   it to wire bytes, and (c) deserialize + replay on the
//!   joiner side. This is the worst case for a late joiner
//!   resuming a long-running session.
//! - `op_batching_packet_count` — 200 individual ops vs one
//!   batch of 200 ops, measuring the CRDT-merge cost reduction
//!   from atomic batch application vs per-op merge. The
//!   network-packet-count win is a separate static fact (1 vs
//!   200 envelopes) — the bench exists to make sure the
//!   batched-apply path stays cheaper than the per-op path.
//!
//! These run under the bridge's default features (no `collab`)
//! because the types we exercise live in `kcreate_collab` and
//! `kcreate_core`, both of which are network-free editing-path
//! crates. The `collab` feature only adds the QUIC transport —
//! which we deliberately don't exercise here.

#![allow(clippy::cast_possible_truncation)]

use std::time::SystemTime;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use ed25519_dalek::SigningKey;

use kcreate_collab::clock::LamportClock;
use kcreate_collab::conflict::{ConflictResolver, LastWriterWinsResolver, OperationContext};
use kcreate_collab::crdt::CrdtResolver;
use kcreate_collab::envelope::Envelope;
use kcreate_collab::journal::{JournalEntry, MemoryJournalStore, OperationJournal, ResumeVector};
use kcreate_collab::message::{Cursor, Message, OperationBroadcastPayload, PresencePayload};
use kcreate_collab::peer::{PeerId, PeerKey};
use kcreate_core::operation::Operation;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Helpers — deterministic identities + ops so two runs of the same
// bench are comparable.
// ---------------------------------------------------------------------------

fn make_signing_key(seed: u8) -> SigningKey {
    let mut bytes = [0u8; 32];
    for (i, b) in bytes.iter_mut().enumerate() {
        *b = seed.wrapping_add(i as u8);
    }
    SigningKey::from_bytes(&bytes)
}

fn make_peer_key(seed: u8) -> PeerKey {
    let mut bytes = [0u8; 32];
    for (i, b) in bytes.iter_mut().enumerate() {
        *b = seed.wrapping_add(i as u8);
    }
    PeerKey::from_seed(bytes)
}

fn make_op(actor: &str, node_id: Uuid, value: i64) -> Operation {
    Operation::new(
        actor,
        "document_update_node",
        serde_json::json!({ "x": value - 1, "y": value - 1 }),
        serde_json::json!({ "x": value, "y": value }),
        vec![node_id],
    )
}

/// A "merge" candidate — same node id, disjoint top-level patch
/// keys. Triggers the property-update merge fast path in the
/// CRDT resolver.
fn make_disjoint_patch_op(actor: &str, node_id: Uuid, key: &str, value: i64) -> Operation {
    Operation::new(
        actor,
        "document_update_node",
        serde_json::json!({ key: value - 1 }),
        serde_json::json!({ key: value }),
        vec![node_id],
    )
}

// ---------------------------------------------------------------------------
// Bench 1: journal append throughput — 10 peers × 10 ops/s aggregate
// 100 ops/s. The bench measures one append call so the iteration
// count drives the throughput report.
// ---------------------------------------------------------------------------

fn bench_journal_append_throughput(c: &mut Criterion) {
    let project_id = Uuid::nil();
    let mut group = c.benchmark_group("collab_perf/journal_append");
    group.throughput(Throughput::Elements(1));

    let peers: Vec<PeerId> = (0..10)
        .map(|i| make_peer_key(i as u8 + 1).peer_id())
        .collect();
    let node_id = Uuid::new_v4();

    group.bench_function("ten_peer_round_robin", |b| {
        let mut journal =
            OperationJournal::open(MemoryJournalStore::new(), project_id).expect("open journal");
        let mut clock = LamportClock::default();
        let mut idx: usize = 0;
        b.iter(|| {
            let peer = &peers[idx % peers.len()];
            clock = clock.tick();
            journal
                .append(
                    peer.clone(),
                    clock,
                    make_op("bench", node_id, idx as i64),
                )
                .expect("journal append");
            idx = idx.wrapping_add(1);
        });
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// Bench 2: CRDT merge latency — single resolver call against a
// pair of concurrent edits.
// ---------------------------------------------------------------------------

fn bench_crdt_merge_latency(c: &mut Criterion) {
    let resolver = CrdtResolver;
    let lww = LastWriterWinsResolver;
    let node_id = Uuid::new_v4();
    let alice = make_peer_key(1).peer_id();
    let bob = make_peer_key(2).peer_id();
    let mut clock = LamportClock::default();
    let clock_a = clock.tick();
    let clock_b = clock.tick();

    let mut group = c.benchmark_group("collab_perf/crdt_merge");

    // Disjoint-key property update — triggers the merge fast
    // path that synthesises a unioned patch.
    let op_a = make_disjoint_patch_op("alice", node_id, "x", 10);
    let op_b = make_disjoint_patch_op("bob", node_id, "y", 20);
    group.bench_function("disjoint_property_merge", |b| {
        b.iter(|| {
            let _ = resolver.resolve_crdt(
                OperationContext {
                    op: &op_a,
                    author: &alice,
                    clock: clock_a,
                },
                OperationContext {
                    op: &op_b,
                    author: &bob,
                    clock: clock_b,
                },
            );
        });
    });

    // Overlapping property update — falls back to LWW.
    let op_a_overlap = make_op("alice", node_id, 10);
    let op_b_overlap = make_op("bob", node_id, 20);
    group.bench_function("overlapping_lww_fallback", |b| {
        b.iter(|| {
            let _ = resolver.resolve_crdt(
                OperationContext {
                    op: &op_a_overlap,
                    author: &alice,
                    clock: clock_a,
                },
                OperationContext {
                    op: &op_b_overlap,
                    author: &bob,
                    clock: clock_b,
                },
            );
        });
    });

    // Baseline: the plain LWW resolver, so we can see how much
    // the CRDT layer costs over the simplest possible resolver.
    group.bench_function("baseline_lww_only", |b| {
        b.iter(|| {
            let _ = lww.resolve(
                OperationContext {
                    op: &op_a_overlap,
                    author: &alice,
                    clock: clock_a,
                },
                OperationContext {
                    op: &op_b_overlap,
                    author: &bob,
                    clock: clock_b,
                },
            );
        });
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// Bench 3: presence serialization — 5-peer presence at 20 Hz.
// ---------------------------------------------------------------------------

fn bench_presence_serialization(c: &mut Criterion) {
    let mut group = c.benchmark_group("collab_perf/presence_serialize");

    let active_page = Some(Uuid::new_v4());
    let selection: Vec<Uuid> = (0..5).map(|_| Uuid::new_v4()).collect();

    for peer_count in &[1usize, 5, 20] {
        group.bench_with_input(
            BenchmarkId::from_parameter(peer_count),
            peer_count,
            |b, &n| {
                let keys: Vec<SigningKey> = (0..n).map(|i| make_signing_key(i as u8 + 1)).collect();
                let peer_ids: Vec<PeerId> = keys
                    .iter()
                    .map(|k| PeerId::from_verifying_key(&k.verifying_key()))
                    .collect();

                let mut tick: u64 = 0;
                b.iter(|| {
                    // One presence beacon per peer, signed and
                    // serialised — mirrors what the bridge does
                    // every 50 ms when 20 Hz throttle is hit.
                    let mut total_bytes = 0usize;
                    for (key, peer_id) in keys.iter().zip(peer_ids.iter()) {
                        let payload = PresencePayload {
                            active_page,
                            selection: selection.clone(),
                            cursor: Some(Cursor {
                                x: (tick as f64) * 0.5,
                                y: (tick as f64) * 0.25,
                            }),
                            sent_at: chrono::DateTime::<chrono::Utc>::from(SystemTime::UNIX_EPOCH)
                                + chrono::Duration::milliseconds(tick as i64),
                        };
                        let mut nonce = [0u8; 16];
                        nonce[..8].copy_from_slice(&tick.to_le_bytes());
                        let env = Envelope::<Message>::seal(
                            peer_id.clone(),
                            LamportClock::default().tick(),
                            nonce,
                            Message::Presence(payload),
                            key,
                        )
                        .expect("envelope seal");
                        let serialised = serde_json::to_vec(&env).expect("serialize");
                        total_bytes = total_bytes.wrapping_add(serialised.len());
                        tick = tick.wrapping_add(1);
                    }
                    criterion::black_box(total_bytes);
                });
            },
        );
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// Bench 4: resume bundle serialize + apply — late-joiner backfill.
// ---------------------------------------------------------------------------

fn bench_resume_bundle_serialize_apply(c: &mut Criterion) {
    let project_id = Uuid::nil();
    let mut group = c.benchmark_group("collab_perf/resume_bundle");
    // The serialize step is O(N) over the entry count, so the
    // throughput axis is "entries serialised per second".
    group.throughput(Throughput::Elements(10_000));

    // Build a journal pre-populated with 10 000 ops across 10
    // peers, simulating a long-running session.
    let peer_keys: Vec<PeerKey> = (0..10).map(|i| make_peer_key(i as u8 + 1)).collect();
    let node_id = Uuid::new_v4();
    let mut host_journal =
        OperationJournal::open(MemoryJournalStore::new(), project_id).expect("open journal");
    let mut clock = LamportClock::default();
    for i in 0..10_000usize {
        clock = clock.tick();
        let peer = peer_keys[i % peer_keys.len()].peer_id();
        host_journal
            .append(peer, clock, make_op("bench", node_id, i as i64))
            .expect("seed append");
    }

    group.bench_function("ten_thousand_entries", |b| {
        b.iter(|| {
            // 1. Host computes the delta the joiner is missing
            //    (joiner has nothing → request equals "give me
            //    everything").
            let empty = ResumeVector::default();
            let entries: Vec<JournalEntry> =
                host_journal.operations_since(&empty).expect("entries");
            // 2. Build the payload and serialize to wire bytes
            //    — what the host actually sends.
            let payload = OperationBroadcastPayload {
                project_id,
                operations: entries.iter().map(|e| e.operation.clone()).collect(),
            };
            let wire = serde_json::to_vec(&payload).expect("serialize");
            // 3. Joiner deserialises + appends into its own
            //    empty journal (simulates resume application).
            let recovered: OperationBroadcastPayload =
                serde_json::from_slice(&wire).expect("deserialize");
            let mut joiner_journal = OperationJournal::open(MemoryJournalStore::new(), project_id)
                .expect("joiner journal");
            let mut joiner_clock = LamportClock::default();
            for (entry, op) in entries.iter().zip(recovered.operations) {
                joiner_clock = joiner_clock.tick();
                joiner_journal
                    .append(entry.peer_id.clone(), joiner_clock, op)
                    .expect("joiner append");
            }
            criterion::black_box(joiner_journal);
        });
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// Bench 5: op batching — 200 individual ops vs one batch of 200.
// We measure the serialise + deserialise round-trip cost since
// that's what the wire actually pays.
// ---------------------------------------------------------------------------

fn bench_op_batching(c: &mut Criterion) {
    let project_id = Uuid::nil();
    let node_ids: Vec<Uuid> = (0..200).map(|_| Uuid::new_v4()).collect();
    let ops: Vec<Operation> = node_ids
        .iter()
        .enumerate()
        .map(|(i, n)| make_op("bench", *n, i as i64))
        .collect();

    let mut group = c.benchmark_group("collab_perf/op_batching");
    group.throughput(Throughput::Elements(200));

    // Unbatched: 200 envelopes, each carrying a one-op payload.
    group.bench_function("unbatched_200_envelopes", |b| {
        b.iter(|| {
            let mut total_bytes = 0usize;
            for op in &ops {
                let payload = OperationBroadcastPayload {
                    project_id,
                    operations: vec![op.clone()],
                };
                let bytes = serde_json::to_vec(&payload).expect("serialize");
                let _round_trip: OperationBroadcastPayload =
                    serde_json::from_slice(&bytes).expect("deserialize");
                total_bytes = total_bytes.wrapping_add(bytes.len());
            }
            criterion::black_box(total_bytes);
        });
    });

    // Batched: one envelope carrying 200 ops — what Task 25's
    // 50 ms accumulator produces under sustained editing.
    group.bench_function("batched_one_envelope_200_ops", |b| {
        b.iter(|| {
            let payload = OperationBroadcastPayload {
                project_id,
                operations: ops.clone(),
            };
            let bytes = serde_json::to_vec(&payload).expect("serialize");
            let _round_trip: OperationBroadcastPayload =
                serde_json::from_slice(&bytes).expect("deserialize");
            criterion::black_box(bytes.len());
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_journal_append_throughput,
    bench_crdt_merge_latency,
    bench_presence_serialization,
    bench_resume_bundle_serialize_apply,
    bench_op_batching,
);
criterion_main!(benches);
