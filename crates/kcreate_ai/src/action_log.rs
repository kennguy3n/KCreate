//! Per-project log of AI actions. Persisted alongside the document
//! so users can audit what the assistant did, when, on what node,
//! and with what model + device.

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use uuid::Uuid;

/// One audit record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiAction {
    pub id: Uuid,
    pub timestamp: DateTime<Utc>,
    pub task_type: String,
    /// e.g. `"threshold-v0"` for Phase 0, `"u2net-onnx"` later.
    pub model: String,
    /// `"cpu"`, `"gpu:metal"`, `"gpu:vulkan"`, ...
    pub compute_device: String,
    pub affected_nodes: Vec<Uuid>,
    pub confidence: Option<f32>,
}

/// Process-wide action log. The bridge appends after each successful
/// task; the renderer reads via N-API.
#[derive(Debug, Default)]
pub struct ActionLog {
    entries: Vec<AiAction>,
}

impl ActionLog {
    /// Append a new action.
    pub fn append(&mut self, action: AiAction) {
        self.entries.push(action);
    }

    /// Newest-first snapshot.
    #[must_use]
    pub fn snapshot(&self) -> Vec<AiAction> {
        let mut out = self.entries.clone();
        out.sort_by_key(|a| std::cmp::Reverse(a.timestamp));
        out
    }

    /// Clear all entries (used by tests + project-close flow).
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Acquire the process-global log.
    pub fn global() -> &'static Mutex<Self> {
        static LOG: OnceLock<Mutex<ActionLog>> = OnceLock::new();
        LOG.get_or_init(|| Mutex::new(Self::default()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_and_snapshot_sorts_newest_first() {
        let mut log = ActionLog::default();
        log.append(AiAction {
            id: Uuid::new_v4(),
            timestamp: chrono::DateTime::from_timestamp(1000, 0).expect("ts"),
            task_type: "background_removal".into(),
            model: "threshold-v0".into(),
            compute_device: "cpu".into(),
            affected_nodes: vec![Uuid::new_v4()],
            confidence: Some(0.9),
        });
        log.append(AiAction {
            id: Uuid::new_v4(),
            timestamp: chrono::DateTime::from_timestamp(2000, 0).expect("ts"),
            task_type: "background_removal".into(),
            model: "threshold-v0".into(),
            compute_device: "cpu".into(),
            affected_nodes: vec![Uuid::new_v4()],
            confidence: None,
        });
        let snap = log.snapshot();
        assert_eq!(snap.len(), 2);
        assert!(snap[0].timestamp > snap[1].timestamp);
    }
}
