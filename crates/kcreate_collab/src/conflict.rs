//! Conflict resolution between local and remote operations.
//!
//! Two peers can edit the same node concurrently. The simplest
//! resolution policy that keeps the document deterministic is
//! **last-writer-wins** with a tiebreak: when two operations report
//! the same Lamport clock, the operation from the larger peer id
//! wins. Peer ids are stable base64url strings so this comparison is
//! deterministic across peers.
//!
//! Future strategies — three-way merge per node-property,
//! application-defined CRDTs, semantic merge for text — slot in via
//! the [`ConflictResolver`] trait. The default
//! [`LastWriterWinsResolver`] is what the Phase 3 transport will
//! ship with.

use kcreate_core::operation::Operation;

use crate::clock::LamportClock;
use crate::peer::PeerId;

/// One side of a conflict.
#[derive(Debug, Clone)]
pub struct OperationContext<'a> {
    /// The operation itself (the [`kcreate_core`] type).
    pub op: &'a Operation,
    /// The Lamport clock value the operation was sent under.
    pub clock: LamportClock,
    /// The peer that authored the operation (for tiebreaking).
    pub author: &'a PeerId,
}

/// Decision returned by [`ConflictResolver::resolve`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictDecision {
    /// Keep the local operation; discard the remote.
    KeepLocal,
    /// Replace the local operation with the remote one.
    KeepRemote,
    /// Apply both in order (local first, then remote). Used when the
    /// operations don't actually touch the same node — i.e. the
    /// "conflict" was a false positive from the gross
    /// affected-nodes-overlap check.
    KeepBoth,
}

/// Strategy interface for resolving overlapping operations.
pub trait ConflictResolver {
    /// Decide what to do given the two sides of a conflict.
    fn resolve(
        &self,
        local: OperationContext<'_>,
        remote: OperationContext<'_>,
    ) -> ConflictDecision;
}

/// The default resolver: last-writer-wins with peer-id tiebreak.
#[derive(Debug, Default, Clone, Copy)]
pub struct LastWriterWinsResolver;

impl ConflictResolver for LastWriterWinsResolver {
    fn resolve(
        &self,
        local: OperationContext<'_>,
        remote: OperationContext<'_>,
    ) -> ConflictDecision {
        // If the affected-node sets are disjoint, there is no real
        // conflict — apply both. Two empty sets are *not* disjoint in
        // the set-theoretic sense, but for our purposes an op with no
        // affected nodes is a document-wide change (e.g.
        // `color_settings_update`), so we treat it as overlapping.
        if !local.op.affected_nodes.is_empty()
            && !remote.op.affected_nodes.is_empty()
            && !any_overlap(&local.op.affected_nodes, &remote.op.affected_nodes)
        {
            return ConflictDecision::KeepBoth;
        }
        match remote.clock.cmp(&local.clock) {
            std::cmp::Ordering::Greater => ConflictDecision::KeepRemote,
            std::cmp::Ordering::Less => ConflictDecision::KeepLocal,
            std::cmp::Ordering::Equal => {
                // Tie. Compare peer ids — the larger id wins. This is
                // an arbitrary but deterministic choice, and matches
                // every peer that runs the same comparison.
                if remote.author > local.author {
                    ConflictDecision::KeepRemote
                } else {
                    ConflictDecision::KeepLocal
                }
            }
        }
    }
}

fn any_overlap(a: &[uuid::Uuid], b: &[uuid::Uuid]) -> bool {
    // For the small N we expect per operation (1–3 affected nodes),
    // a quadratic scan is faster than building a HashSet.
    a.iter().any(|x| b.contains(x))
}

#[cfg(test)]
mod tests {
    use super::*;
    use kcreate_core::operation::Operation;
    use serde_json::json;
    use uuid::Uuid;

    fn op_for_node(node: Uuid, text: &str) -> Operation {
        Operation::new(
            "user",
            "set_text",
            json!({"text": "before"}),
            json!({"text": text}),
            vec![node],
        )
    }

    fn peer(label: &str) -> PeerId {
        // `PeerId` is opaque (private inner field), but `serde_json`
        // round-trip gives us a free constructor — we serialise a
        // string and deserialise it as a transparent newtype.
        serde_json::from_value(serde_json::Value::String(label.into())).unwrap()
    }

    #[test]
    fn remote_with_larger_clock_wins() {
        let n = Uuid::new_v4();
        let local_op = op_for_node(n, "local");
        let remote_op = op_for_node(n, "remote");
        let a = peer("alpha");
        let b = peer("bravo");
        let decision = LastWriterWinsResolver.resolve(
            OperationContext {
                op: &local_op,
                clock: LamportClock::from_raw(1),
                author: &a,
            },
            OperationContext {
                op: &remote_op,
                clock: LamportClock::from_raw(2),
                author: &b,
            },
        );
        assert_eq!(decision, ConflictDecision::KeepRemote);
    }

    #[test]
    fn local_with_larger_clock_wins() {
        let n = Uuid::new_v4();
        let local_op = op_for_node(n, "local");
        let remote_op = op_for_node(n, "remote");
        let a = peer("alpha");
        let b = peer("bravo");
        let decision = LastWriterWinsResolver.resolve(
            OperationContext {
                op: &local_op,
                clock: LamportClock::from_raw(5),
                author: &a,
            },
            OperationContext {
                op: &remote_op,
                clock: LamportClock::from_raw(2),
                author: &b,
            },
        );
        assert_eq!(decision, ConflictDecision::KeepLocal);
    }

    #[test]
    fn tie_is_broken_by_larger_peer_id() {
        let n = Uuid::new_v4();
        let local_op = op_for_node(n, "local");
        let remote_op = op_for_node(n, "remote");
        let a = peer("alpha");
        let b = peer("bravo");
        // bravo > alpha, so remote (bravo) wins.
        let decision = LastWriterWinsResolver.resolve(
            OperationContext {
                op: &local_op,
                clock: LamportClock::from_raw(3),
                author: &a,
            },
            OperationContext {
                op: &remote_op,
                clock: LamportClock::from_raw(3),
                author: &b,
            },
        );
        assert_eq!(decision, ConflictDecision::KeepRemote);
    }

    #[test]
    fn tie_with_smaller_peer_id_keeps_local() {
        let n = Uuid::new_v4();
        let local_op = op_for_node(n, "local");
        let remote_op = op_for_node(n, "remote");
        let a = peer("zulu");
        let b = peer("alpha");
        // alpha < zulu, so local (zulu) wins on tiebreak.
        let decision = LastWriterWinsResolver.resolve(
            OperationContext {
                op: &local_op,
                clock: LamportClock::from_raw(7),
                author: &a,
            },
            OperationContext {
                op: &remote_op,
                clock: LamportClock::from_raw(7),
                author: &b,
            },
        );
        assert_eq!(decision, ConflictDecision::KeepLocal);
    }

    #[test]
    fn disjoint_affected_nodes_keeps_both() {
        let n1 = Uuid::new_v4();
        let n2 = Uuid::new_v4();
        let local_op = op_for_node(n1, "local");
        let remote_op = op_for_node(n2, "remote");
        let a = peer("alpha");
        let b = peer("bravo");
        let decision = LastWriterWinsResolver.resolve(
            OperationContext {
                op: &local_op,
                clock: LamportClock::from_raw(5),
                author: &a,
            },
            OperationContext {
                op: &remote_op,
                clock: LamportClock::from_raw(10),
                author: &b,
            },
        );
        assert_eq!(decision, ConflictDecision::KeepBoth);
    }

    #[test]
    fn document_wide_op_with_empty_affected_nodes_collides_with_everything() {
        let n = Uuid::new_v4();
        let local_op = Operation::new(
            "user",
            "color_settings_update",
            json!({}),
            json!({}),
            vec![],
        );
        let remote_op = op_for_node(n, "remote");
        let a = peer("alpha");
        let b = peer("bravo");
        let decision = LastWriterWinsResolver.resolve(
            OperationContext {
                op: &local_op,
                clock: LamportClock::from_raw(1),
                author: &a,
            },
            OperationContext {
                op: &remote_op,
                clock: LamportClock::from_raw(2),
                author: &b,
            },
        );
        // Remote wins (clock-2 > clock-1), but the important
        // assertion is that we did *not* return KeepBoth despite the
        // disjoint sets — document-wide ops always collide.
        assert_eq!(decision, ConflictDecision::KeepRemote);
    }
}
