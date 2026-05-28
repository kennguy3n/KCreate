//! Frozen JSON-shape snapshot of a [`Timeline`](crate::Timeline).
//!
//! [`Report`] is the wire-format the bridge surfaces to the
//! renderer (via the N-API `runtime_startup_timeline` entry
//! point) and to tests. The shape is deliberately tiny and
//! self-describing — every duration is monotonic nanoseconds, the
//! `started_at_unix_ms` field is the only wall-clock value, and
//! `phases` is pre-computed from `marks` so consumers don't need
//! to re-implement the derivation.

use serde::{Deserialize, Serialize};

/// One mark inside a frozen timeline report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarkReport {
    pub label: String,
    pub monotonic_ns: u64,
}

/// Derived per-phase summary — one entry per mark, where each
/// phase spans `from_ns..to_ns` and `duration_ns` is the difference.
/// The last phase runs from the final mark to the timeline's
/// `total_ns`, so consumers always see how much time elapsed after
/// the last named mark.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhaseReport {
    pub label: String,
    pub from_ns: u64,
    pub to_ns: u64,
    pub duration_ns: u64,
}

/// Frozen, serialisable snapshot of a [`Timeline`](crate::Timeline).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub name: String,
    /// Wall-clock anchor (milliseconds since the Unix epoch) at the
    /// moment the timeline started. Reported for human correlation
    /// with system logs; durations are always derived from
    /// `monotonic_ns`, never from this field.
    pub started_at_unix_ms: i64,
    /// Total monotonic nanoseconds elapsed since the timeline
    /// started (measured at snapshot/finish time).
    pub total_ns: u64,
    pub marks: Vec<MarkReport>,
    pub phases: Vec<PhaseReport>,
}

impl Report {
    /// Serialise to a stable, pretty-printed JSON string. Returns
    /// `None` only if `serde_json` fails internally — which would
    /// imply a Rust type mismatch and is impossible for the fixed
    /// `Report` shape.
    #[must_use]
    pub fn to_pretty_json(&self) -> Option<String> {
        serde_json::to_string_pretty(self).ok()
    }

    /// Serialise to a compact JSON string (no whitespace).
    #[must_use]
    pub fn to_json(&self) -> Option<String> {
        serde_json::to_string(self).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pretty_and_compact_json_round_trip() {
        let report = Report {
            name: "round-trip".into(),
            started_at_unix_ms: 1_700_000_000_000,
            total_ns: 5_000_000,
            marks: vec![MarkReport {
                label: "a".into(),
                monotonic_ns: 1_000_000,
            }],
            phases: vec![PhaseReport {
                label: "a".into(),
                from_ns: 1_000_000,
                to_ns: 5_000_000,
                duration_ns: 4_000_000,
            }],
        };
        let pretty = report.to_pretty_json().expect("pretty json");
        let compact = report.to_json().expect("compact json");
        let from_pretty: Report = serde_json::from_str(&pretty).expect("parse pretty");
        let from_compact: Report = serde_json::from_str(&compact).expect("parse compact");
        assert_eq!(from_pretty, report);
        assert_eq!(from_compact, report);
    }
}
