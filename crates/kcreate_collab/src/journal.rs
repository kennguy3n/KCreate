//! Per-project collaboration operation journal.
//!
//! Phase 3 left every multiplayer session **ephemeral**: presence
//! cursors and broadcast operations lived in the active peer's heap
//! only, so a peer drop or host restart erased the session state. The
//! editing-path operation log (kcreate_storage migration #2) already
//! captures what the *local* user did, but it does not record the
//! provenance of each op (which peer, at what Lamport clock), and so
//! it cannot answer "give me every op from peer X since clock Y" —
//! the question every reconnect handshake needs to ask.
//!
//! Block 7 ships the *journal* abstraction that closes that gap. The
//! journal is a small append-only ledger of
//! `(peer_id, lamport, operation)` tuples scoped to a single project.
//! It exposes two queries:
//!
//! * [`OperationJournal::summary`] — returns the highest clock
//!   observed for every peer the journal knows about. This is what a
//!   joiner sends to a host as the "I've already seen this much"
//!   resume vector.
//! * [`OperationJournal::operations_since`] — returns every op the
//!   journal has that's strictly newer than the supplied vector, in
//!   `(peer_id, lamport)` order. This is what the host sends back in
//!   a [`Message::ResumeBundle`] so the joiner can replay missing
//!   work.
//!
//! Persistence is pluggable behind the [`JournalStore`] trait. The
//! crate ships an [`MemoryJournalStore`] for tests; production
//! callers wire in a SQLite-backed store from `kcreate_storage`. The
//! bridge layer keeps the live journal in memory during a session
//! and `flush`es to SQLite on every append, so a crash mid-session
//! still preserves history for the next rejoin.
//!
//! KChat gating: this module is transport-agnostic and does **not**
//! enforce KChat membership itself — that gate lives one layer up
//! in the bridge's `require_active_kchat_membership` helper. The
//! design is intentional: a future audit / replay tool that reads
//! the journal needs to work without a KChat session.

use std::collections::HashMap;
use std::fmt;

use kcreate_core::operation::Operation;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::clock::LamportClock;
use crate::peer::PeerId;

/// An entry in the journal: one operation from one peer at a
/// specific Lamport clock value. The operation type is the same
/// [`kcreate_core::operation::Operation`] the editing path commits
/// locally, so the journal can both record and replay without
/// translation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JournalEntry {
    /// Peer the operation originated from. For locally-authored ops
    /// this is the local peer's id; for remote ops it's the
    /// envelope-verified sender.
    pub peer_id: PeerId,
    /// Lamport clock value at which the op was committed. Pairs
    /// with `peer_id` to give a globally unique total order across
    /// peers (per Lamport's tie-break-on-peer-id convention).
    pub clock: LamportClock,
    /// Project id the op belongs to. Required so a journal store
    /// shared across projects (uncommon today but supported by the
    /// trait) can filter on read.
    pub project_id: Uuid,
    /// The actual committed operation. Includes before/after JSON
    /// patches so replay is lossless on both forward and reverse
    /// application.
    pub operation: Operation,
}

/// A "vector clock"-shaped summary of what a journal has observed.
/// Maps each known peer id to the highest Lamport clock the journal
/// has recorded for it. Peers absent from the map are implicitly
/// "no operations seen", equivalent to a clock of zero.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ResumeVector {
    /// `peer_id -> highest seen clock`. Empty for a fresh joiner.
    pub by_peer: HashMap<PeerId, LamportClock>,
}

impl ResumeVector {
    /// Empty vector — equivalent to "I have seen nothing".
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// Returns the highest clock the vector has for `peer`, or
    /// [`LamportClock::default`] (i.e. clock 0) if the peer is
    /// absent. The "absent == zero" convention means a fresh
    /// joiner sending an empty vector means "send me everything",
    /// which is the correct semantics for first-time join.
    #[must_use]
    pub fn highest_for(&self, peer: &PeerId) -> LamportClock {
        self.by_peer.get(peer).copied().unwrap_or_default()
    }

    /// Number of distinct peers this vector mentions.
    #[must_use]
    pub fn peer_count(&self) -> usize {
        self.by_peer.len()
    }
}

/// Errors a [`JournalStore`] may report.
#[derive(Debug, Error)]
pub enum JournalError {
    /// The store rejected an append because the entry duplicated an
    /// existing `(peer_id, clock)` pair. Treat this as benign — it
    /// means the caller already saw and applied this op.
    #[error("duplicate journal entry: peer {peer_id:?} clock {clock:?}")]
    Duplicate {
        peer_id: PeerId,
        clock: LamportClock,
    },
    /// The store rejected an append because the entry's clock is
    /// strictly less than the highest one already recorded for the
    /// peer. This indicates a non-monotonic peer — drop the
    /// envelope at the session layer rather than silently
    /// reordering.
    #[error("non-monotonic clock: peer {peer_id:?} new {new_clock:?} <= last {last_clock:?}")]
    OutOfOrder {
        peer_id: PeerId,
        new_clock: LamportClock,
        last_clock: LamportClock,
    },
    /// Generic backend failure (sqlite error, disk full, …).
    /// Stringified so the trait stays object-safe and free of
    /// `Box<dyn Error>` plumbing.
    #[error("journal store backend error: {0}")]
    Backend(String),
}

/// Pluggable persistence for an [`OperationJournal`]. Implementations
/// are append-only from the journal's perspective: the journal never
/// asks for in-place mutation or deletion. Compaction (replacing a
/// historical run with a snapshot) is a future block and would live
/// in a `compact` method on a separate trait.
///
/// `JournalStore` must be `Send + Sync` so it can be shared across
/// the bridge's tokio runtime thread and the renderer-facing N-API
/// thread. Implementations that hold non-`Sync` state (rusqlite
/// `Connection`s, for example) must wrap in `Mutex` themselves.
pub trait JournalStore: Send + Sync {
    /// Append `entry` to the store. Implementations MUST:
    /// - reject duplicates as [`JournalError::Duplicate`] (idempotent
    ///   semantics — the caller can replay safely),
    /// - reject non-monotonic clocks as [`JournalError::OutOfOrder`],
    /// - persist the entry before returning `Ok(())` (no buffered
    ///   writes).
    fn append(&mut self, entry: JournalEntry) -> Result<(), JournalError>;

    /// Read every entry in the store, in `(peer_id, clock)` order,
    /// strictly newer than the supplied resume vector. The store
    /// owns the iteration order: callers MUST NOT assume a
    /// per-peer interleaving.
    fn operations_since(
        &self,
        project_id: Uuid,
        since: &ResumeVector,
    ) -> Result<Vec<JournalEntry>, JournalError>;

    /// Compute the highest-clock-per-peer summary across every
    /// entry in the store scoped to `project_id`.
    fn summary(&self, project_id: Uuid) -> Result<ResumeVector, JournalError>;

    /// Total number of journal entries scoped to `project_id`.
    /// Used by tests and by the bridge's "session has any history"
    /// gate before emitting a ResumeRequest.
    fn len(&self, project_id: Uuid) -> Result<usize, JournalError>;

    /// `true` iff [`Self::len`] returns 0. Default impl provided.
    fn is_empty(&self, project_id: Uuid) -> Result<bool, JournalError> {
        Ok(self.len(project_id)? == 0)
    }
}

/// In-memory journal store. Suitable for unit tests, transport
/// integration tests, and (with care) for short-lived live sessions
/// where the caller is fine with losing history on crash. The bridge
/// pairs this with a SQLite-backed mirror so liveness and durability
/// are decoupled.
#[derive(Debug, Default)]
pub struct MemoryJournalStore {
    entries: Vec<JournalEntry>,
}

impl MemoryJournalStore {
    /// Empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl JournalStore for MemoryJournalStore {
    fn append(&mut self, entry: JournalEntry) -> Result<(), JournalError> {
        // Reject duplicates by exact (peer, clock) pair.
        if self.entries.iter().any(|existing| {
            existing.peer_id == entry.peer_id
                && existing.clock == entry.clock
                && existing.project_id == entry.project_id
        }) {
            return Err(JournalError::Duplicate {
                peer_id: entry.peer_id.clone(),
                clock: entry.clock,
            });
        }
        // Reject non-monotonic clocks per peer (within the same
        // project). A different peer-id but lower clock is fine —
        // Lamport clocks are partial-order across peers.
        let last_for_peer = self
            .entries
            .iter()
            .filter(|e| e.peer_id == entry.peer_id && e.project_id == entry.project_id)
            .map(|e| e.clock)
            .max();
        if let Some(last_clock) = last_for_peer {
            if entry.clock <= last_clock {
                return Err(JournalError::OutOfOrder {
                    peer_id: entry.peer_id.clone(),
                    new_clock: entry.clock,
                    last_clock,
                });
            }
        }
        self.entries.push(entry);
        Ok(())
    }

    fn operations_since(
        &self,
        project_id: Uuid,
        since: &ResumeVector,
    ) -> Result<Vec<JournalEntry>, JournalError> {
        let mut out: Vec<JournalEntry> = self
            .entries
            .iter()
            .filter(|e| e.project_id == project_id)
            .filter(|e| e.clock > since.highest_for(&e.peer_id))
            .cloned()
            .collect();
        // Sort by (peer_id, clock) for deterministic replay order.
        // Peer-id is the secondary sort to make multi-peer
        // interleaving stable; receivers re-sort by clock anyway.
        out.sort_by(|a, b| {
            a.clock
                .cmp(&b.clock)
                .then_with(|| a.peer_id.as_str().cmp(b.peer_id.as_str()))
        });
        Ok(out)
    }

    fn summary(&self, project_id: Uuid) -> Result<ResumeVector, JournalError> {
        let mut by_peer: HashMap<PeerId, LamportClock> = HashMap::new();
        for e in self.entries.iter().filter(|e| e.project_id == project_id) {
            by_peer
                .entry(e.peer_id.clone())
                .and_modify(|c| {
                    if e.clock > *c {
                        *c = e.clock;
                    }
                })
                .or_insert(e.clock);
        }
        Ok(ResumeVector { by_peer })
    }

    fn len(&self, project_id: Uuid) -> Result<usize, JournalError> {
        Ok(self
            .entries
            .iter()
            .filter(|e| e.project_id == project_id)
            .count())
    }
}

/// The journal itself. Wraps a [`JournalStore`] with the bookkeeping
/// the session layer needs: knowing the project this journal is
/// scoped to (so callers don't have to pass it on every call), and
/// caching the latest summary in memory for cheap rebuilds of the
/// resume vector each time a Hello is constructed.
///
/// The journal is **not** Send-via-shared-reference: the inner store
/// is mutably borrowed on every append. Hand it through `Arc<Mutex<…>>`
/// at the bridge layer rather than from inside.
pub struct OperationJournal<S: JournalStore> {
    store: S,
    project_id: Uuid,
    cached_summary: ResumeVector,
}

impl<S: JournalStore> fmt::Debug for OperationJournal<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OperationJournal")
            .field("project_id", &self.project_id)
            .field(
                "cached_summary_peer_count",
                &self.cached_summary.peer_count(),
            )
            .finish()
    }
}

impl<S: JournalStore> OperationJournal<S> {
    /// Build a journal for `project_id` backed by `store`. Reads the
    /// initial summary from the store eagerly so the first
    /// [`Self::resume_vector`] call after construction returns the
    /// persisted state (e.g. after a restart).
    pub fn open(mut store: S, project_id: Uuid) -> Result<Self, JournalError> {
        let cached_summary = store.summary(project_id)?;
        // Note: we re-bind `store` because `summary` only takes `&self`,
        // but we want a single mutable owner for future appends.
        let _ = &mut store;
        Ok(Self {
            store,
            project_id,
            cached_summary,
        })
    }

    /// The project this journal is scoped to.
    #[must_use]
    pub fn project_id(&self) -> Uuid {
        self.project_id
    }

    /// Append a new entry. Internally validates the entry matches
    /// the journal's project id and updates the cached summary on
    /// success.
    pub fn append(
        &mut self,
        peer_id: PeerId,
        clock: LamportClock,
        operation: Operation,
    ) -> Result<(), JournalError> {
        let entry = JournalEntry {
            peer_id: peer_id.clone(),
            clock,
            project_id: self.project_id,
            operation,
        };
        self.store.append(entry)?;
        // Update cached summary in place. Append succeeded so we
        // know the clock is strictly greater than the existing
        // entry (if any) for this peer.
        self.cached_summary
            .by_peer
            .entry(peer_id)
            .and_modify(|c| {
                if clock > *c {
                    *c = clock;
                }
            })
            .or_insert(clock);
        Ok(())
    }

    /// Current resume vector. Cheap; reads from the in-memory
    /// cache. Updated by [`Self::append`] and by
    /// [`Self::refresh_summary`].
    #[must_use]
    pub fn resume_vector(&self) -> ResumeVector {
        self.cached_summary.clone()
    }

    /// Force a re-read of the summary from the backing store.
    /// Useful when an out-of-band writer (e.g. a CLI tool) appended
    /// to the same SQLite file behind the running journal's back.
    pub fn refresh_summary(&mut self) -> Result<(), JournalError> {
        self.cached_summary = self.store.summary(self.project_id)?;
        Ok(())
    }

    /// Every op the journal has that's strictly newer than `since`.
    pub fn operations_since(
        &self,
        since: &ResumeVector,
    ) -> Result<Vec<JournalEntry>, JournalError> {
        self.store.operations_since(self.project_id, since)
    }

    /// Total entry count for this project.
    pub fn len(&self) -> Result<usize, JournalError> {
        self.store.len(self.project_id)
    }

    /// Empty-ness shortcut.
    pub fn is_empty(&self) -> Result<bool, JournalError> {
        self.store.is_empty(self.project_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use ed25519_dalek::SigningKey;
    use kcreate_core::operation::Operation;

    fn peer(seed: u8) -> PeerId {
        // Deterministic synthetic peer id for tests. A peer id is
        // derived from an Ed25519 public key via BLAKE3; we
        // construct a key from a single-byte seed so each `peer(n)`
        // is stable across test runs but distinct from `peer(m)`.
        let signing = SigningKey::from_bytes(&[seed; 32]);
        PeerId::from_verifying_key(&signing.verifying_key())
    }

    fn dummy_op() -> Operation {
        Operation {
            id: Uuid::new_v4(),
            timestamp: Utc::now(),
            actor: "test".to_string(),
            command: "noop".to_string(),
            before_patch: serde_json::Value::Null,
            after_patch: serde_json::Value::Null,
            affected_nodes: Vec::new(),
            ai_generated: false,
            group_id: None,
            is_undo: false,
        }
    }

    #[test]
    fn append_and_summary_track_highest_clock_per_peer() {
        let project = Uuid::new_v4();
        let mut journal = OperationJournal::open(MemoryJournalStore::new(), project).unwrap();
        let a = peer(1);
        let b = peer(2);

        journal
            .append(a.clone(), LamportClock::from_raw(1), dummy_op())
            .unwrap();
        journal
            .append(a.clone(), LamportClock::from_raw(5), dummy_op())
            .unwrap();
        journal
            .append(b.clone(), LamportClock::from_raw(3), dummy_op())
            .unwrap();

        let summary = journal.resume_vector();
        assert_eq!(summary.highest_for(&a), LamportClock::from_raw(5));
        assert_eq!(summary.highest_for(&b), LamportClock::from_raw(3));
        assert_eq!(summary.peer_count(), 2);
    }

    #[test]
    fn duplicate_clock_returns_duplicate_error() {
        let project = Uuid::new_v4();
        let mut journal = OperationJournal::open(MemoryJournalStore::new(), project).unwrap();
        let a = peer(1);
        journal
            .append(a.clone(), LamportClock::from_raw(1), dummy_op())
            .unwrap();
        let err = journal
            .append(a, LamportClock::from_raw(1), dummy_op())
            .unwrap_err();
        assert!(matches!(err, JournalError::Duplicate { .. }));
    }

    #[test]
    fn out_of_order_clock_returns_out_of_order_error() {
        let project = Uuid::new_v4();
        let mut journal = OperationJournal::open(MemoryJournalStore::new(), project).unwrap();
        let a = peer(1);
        journal
            .append(a.clone(), LamportClock::from_raw(5), dummy_op())
            .unwrap();
        let err = journal
            .append(a, LamportClock::from_raw(3), dummy_op())
            .unwrap_err();
        assert!(matches!(err, JournalError::OutOfOrder { .. }));
    }

    #[test]
    fn operations_since_skips_already_seen() {
        let project = Uuid::new_v4();
        let mut journal = OperationJournal::open(MemoryJournalStore::new(), project).unwrap();
        let a = peer(1);
        let b = peer(2);
        for clk in [1u64, 2, 3, 4] {
            journal
                .append(a.clone(), LamportClock::from_raw(clk), dummy_op())
                .unwrap();
        }
        for clk in [1u64, 2] {
            journal
                .append(b.clone(), LamportClock::from_raw(clk), dummy_op())
                .unwrap();
        }

        let mut since = ResumeVector::empty();
        since.by_peer.insert(a.clone(), LamportClock::from_raw(2));
        // No entry for b -> implicit zero, so we expect all of b.

        let missing = journal.operations_since(&since).unwrap();
        // We should get a@3, a@4, b@1, b@2 -> 4 entries.
        assert_eq!(missing.len(), 4);
        let a_clocks: Vec<u64> = missing
            .iter()
            .filter(|e| e.peer_id == a)
            .map(|e| e.clock.as_u64())
            .collect();
        assert_eq!(a_clocks, vec![3, 4]);
        let b_clocks: Vec<u64> = missing
            .iter()
            .filter(|e| e.peer_id == b)
            .map(|e| e.clock.as_u64())
            .collect();
        assert_eq!(b_clocks, vec![1, 2]);
    }

    #[test]
    fn operations_since_with_full_vector_returns_nothing() {
        let project = Uuid::new_v4();
        let mut journal = OperationJournal::open(MemoryJournalStore::new(), project).unwrap();
        let a = peer(1);
        journal
            .append(a.clone(), LamportClock::from_raw(7), dummy_op())
            .unwrap();
        let mut since = ResumeVector::empty();
        since.by_peer.insert(a, LamportClock::from_raw(7));
        assert!(journal.operations_since(&since).unwrap().is_empty());
    }

    #[test]
    fn fresh_joiner_with_empty_vector_gets_everything() {
        let project = Uuid::new_v4();
        let mut journal = OperationJournal::open(MemoryJournalStore::new(), project).unwrap();
        let a = peer(1);
        for clk in [1u64, 2, 3] {
            journal
                .append(a.clone(), LamportClock::from_raw(clk), dummy_op())
                .unwrap();
        }
        let since = ResumeVector::empty();
        let all = journal.operations_since(&since).unwrap();
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn project_id_filter_isolates_history() {
        let p1 = Uuid::new_v4();
        let p2 = Uuid::new_v4();
        let mut store = MemoryJournalStore::new();
        let a = peer(1);
        store
            .append(JournalEntry {
                peer_id: a.clone(),
                clock: LamportClock::from_raw(1),
                project_id: p1,
                operation: dummy_op(),
            })
            .unwrap();
        store
            .append(JournalEntry {
                peer_id: a.clone(),
                clock: LamportClock::from_raw(99),
                project_id: p2,
                operation: dummy_op(),
            })
            .unwrap();

        let j1 = OperationJournal::open(store, p1).unwrap();
        let summary = j1.resume_vector();
        // Only the p1 entry is visible through a p1-scoped journal.
        assert_eq!(summary.highest_for(&a), LamportClock::from_raw(1));
        assert_eq!(j1.len().unwrap(), 1);
    }

    #[test]
    fn refresh_summary_picks_up_out_of_band_writes() {
        let project = Uuid::new_v4();
        let mut store = MemoryJournalStore::new();
        let a = peer(1);
        store
            .append(JournalEntry {
                peer_id: a.clone(),
                clock: LamportClock::from_raw(1),
                project_id: project,
                operation: dummy_op(),
            })
            .unwrap();
        let mut journal = OperationJournal::open(store, project).unwrap();
        assert_eq!(
            journal.resume_vector().highest_for(&a),
            LamportClock::from_raw(1)
        );
        // Sneak a write past the journal directly into the store.
        journal
            .store
            .append(JournalEntry {
                peer_id: a.clone(),
                clock: LamportClock::from_raw(2),
                project_id: project,
                operation: dummy_op(),
            })
            .unwrap();
        // Cache still says 1; explicit refresh updates it.
        assert_eq!(
            journal.resume_vector().highest_for(&a),
            LamportClock::from_raw(1)
        );
        journal.refresh_summary().unwrap();
        assert_eq!(
            journal.resume_vector().highest_for(&a),
            LamportClock::from_raw(2)
        );
    }

    #[test]
    fn resume_vector_serde_roundtrip() {
        let mut v = ResumeVector::empty();
        v.by_peer.insert(peer(7), LamportClock::from_raw(42));
        let json = serde_json::to_string(&v).unwrap();
        let round: ResumeVector = serde_json::from_str(&json).unwrap();
        assert_eq!(round.highest_for(&peer(7)), LamportClock::from_raw(42));
    }
}
