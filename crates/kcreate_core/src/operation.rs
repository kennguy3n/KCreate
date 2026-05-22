//! Operation log — append-only history with bounded undo depth.
//!
//! The log stores complete before/after JSON patches per operation,
//! plus the actor, command, affected node ids, and a flag for AI
//! provenance. `undo()` moves a position cursor backward; `redo()`
//! advances it. A new `push` truncates any redo-stack tail.
//!
//! Bounded depth (`max_depth`) trims the *oldest* entries; this keeps
//! the working memory cost bounded even for long editing sessions.
//!
//! Storage: backed by `VecDeque<Operation>` so the bounded-depth trim
//! is O(1) per dropped entry (`pop_front`). Restoring a large persisted
//! history that exceeds `max_depth` is therefore O(excess) rather than
//! O(excess²).

use std::collections::VecDeque;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// One entry in the operation log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Operation {
    pub id: Uuid,
    pub timestamp: DateTime<Utc>,
    pub actor: String,
    pub command: String,
    pub before_patch: serde_json::Value,
    pub after_patch: serde_json::Value,
    pub affected_nodes: Vec<Uuid>,
    pub ai_generated: bool,
}

impl Operation {
    /// Build a non-AI operation with an auto-generated id and current
    /// timestamp.
    #[must_use]
    pub fn new(
        actor: impl Into<String>,
        command: impl Into<String>,
        before_patch: serde_json::Value,
        after_patch: serde_json::Value,
        affected_nodes: Vec<Uuid>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            timestamp: Utc::now(),
            actor: actor.into(),
            command: command.into(),
            before_patch,
            after_patch,
            affected_nodes,
            ai_generated: false,
        }
    }

    /// Mark this operation as AI-generated. Useful for filtering the
    /// history view and for the safety layer.
    #[must_use]
    pub const fn as_ai_generated(mut self) -> Self {
        self.ai_generated = true;
        self
    }
}

/// Bounded undo/redo log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationLog {
    /// Most-recent operations are at the back; the bounded-depth trim
    /// drops from the front via `pop_front` (O(1)).
    history: VecDeque<Operation>,
    /// Index just past the last applied op. `position == history.len()`
    /// means "all ops are applied". `position == 0` means "nothing
    /// applied".
    position: usize,
    max_depth: usize,
}

impl OperationLog {
    /// Construct a log that retains at most `max_depth` ops. `0` is
    /// clamped to `1` (a useless but well-defined edge).
    #[must_use]
    pub fn new(max_depth: usize) -> Self {
        Self {
            history: VecDeque::new(),
            position: 0,
            max_depth: max_depth.max(1),
        }
    }

    /// Append an operation. Any pending redo entries (between
    /// `position` and `history.len()`) are discarded.
    pub fn push(&mut self, op: Operation) {
        self.history.truncate(self.position);
        self.history.push_back(op);
        // Bound the buffer — drop oldest entries. `VecDeque::pop_front`
        // is O(1).
        while self.history.len() > self.max_depth {
            self.history.pop_front();
        }
        self.position = self.history.len();
    }

    /// Move backward in time. Returns the operation that was undone
    /// (i.e. the one whose `before_patch` should be applied), or
    /// `None` if there is nothing to undo.
    pub fn undo(&mut self) -> Option<&Operation> {
        if self.position == 0 {
            return None;
        }
        self.position -= 1;
        self.history.get(self.position)
    }

    /// Move forward in time. Returns the operation that was re-applied
    /// (its `after_patch` should be applied), or `None` if there is
    /// nothing to redo.
    pub fn redo(&mut self) -> Option<&Operation> {
        if self.position >= self.history.len() {
            return None;
        }
        let op = self.history.get(self.position);
        self.position += 1;
        op
    }

    #[must_use]
    pub const fn can_undo(&self) -> bool {
        self.position > 0
    }

    #[must_use]
    pub fn can_redo(&self) -> bool {
        // `VecDeque::len` is not yet const-stable (unlike `Vec::len`).
        self.position < self.history.len()
    }

    /// Iterate the history in chronological order (oldest first).
    ///
    /// Returns an iterator rather than a slice because the underlying
    /// `VecDeque` may not be contiguous; callers that need indexed
    /// access should use [`Self::get`].
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &Operation> {
        self.history.iter()
    }

    /// Random access into the history. `idx == 0` is the oldest entry,
    /// `idx == len() - 1` is the newest.
    #[must_use]
    pub fn get(&self, idx: usize) -> Option<&Operation> {
        self.history.get(idx)
    }

    /// The most-recently pushed entry, if any.
    #[must_use]
    pub fn last(&self) -> Option<&Operation> {
        self.history.back()
    }

    pub fn clear(&mut self) {
        self.history.clear();
        self.position = 0;
    }

    #[must_use]
    pub fn len(&self) -> usize {
        // `VecDeque::len` is not yet const-stable (unlike `Vec::len`).
        self.history.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        // `VecDeque::is_empty` is not yet const-stable.
        self.history.is_empty()
    }

    #[must_use]
    pub const fn max_depth(&self) -> usize {
        self.max_depth
    }

    /// Set a new bound on the retained history depth. The value is
    /// clamped to `>= 1`. If the new bound is smaller than the
    /// current history length, the oldest entries are dropped (and
    /// the position cursor is adjusted so it still points at the
    /// "after the last applied op" slot it pointed at before, never
    /// past the new end).
    ///
    /// This is the runtime-configuration knob that
    /// `kcreate_core::config::RuntimeConfig` uses to honour tier
    /// changes (e.g. enabling low-resource mode shrinks the undo
    /// buffer to 32 entries on Tier 0).
    pub fn set_max_depth(&mut self, new_max: usize) {
        let new_max = new_max.max(1);
        self.max_depth = new_max;
        while self.history.len() > self.max_depth {
            // Dropping the front of the history shifts every index
            // down by one, so the cursor must shift too — but never
            // below zero. After the loop the cursor still lands on
            // "first un-applied slot" relative to the new history.
            self.history.pop_front();
            self.position = self.position.saturating_sub(1);
        }
        if self.position > self.history.len() {
            self.position = self.history.len();
        }
    }

    #[must_use]
    pub const fn position(&self) -> usize {
        self.position
    }

    /// Replace the entire history from persistence. Operations are
    /// appended in iteration order with no truncation semantics
    /// (callers are expected to pass at most `max_depth` items — extra
    /// items are dropped from the *front* to preserve the most recent
    /// history).
    ///
    /// The position cursor is reset to the end so every restored
    /// operation is considered "already applied" — there is no redo
    /// stack to recover across sessions.
    pub fn restore_from<I>(&mut self, ops: I)
    where
        I: IntoIterator<Item = Operation>,
    {
        self.history.clear();
        self.history.extend(ops);
        while self.history.len() > self.max_depth {
            self.history.pop_front();
        }
        self.position = self.history.len();
    }
}

impl Default for OperationLog {
    fn default() -> Self {
        // 256 ops is a reasonable session default for a desktop editor.
        Self::new(256)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn op(name: &str) -> Operation {
        Operation::new("user", name, json!({}), json!({}), Vec::new())
    }

    #[test]
    fn push_undo_redo_cycle() {
        let mut log = OperationLog::new(8);
        assert!(!log.can_undo() && !log.can_redo());
        log.push(op("a"));
        log.push(op("b"));
        log.push(op("c"));
        assert!(log.can_undo());
        assert!(!log.can_redo());

        let last = log.undo().expect("undo c");
        assert_eq!(last.command, "c");
        let last = log.undo().expect("undo b");
        assert_eq!(last.command, "b");
        assert!(log.can_redo());

        let next = log.redo().expect("redo b");
        assert_eq!(next.command, "b");
        let next = log.redo().expect("redo c");
        assert_eq!(next.command, "c");
        assert!(!log.can_redo());
    }

    #[test]
    fn new_push_truncates_redo_stack() {
        let mut log = OperationLog::new(8);
        log.push(op("a"));
        log.push(op("b"));
        log.undo();
        log.push(op("c"));
        assert!(!log.can_redo());
        assert_eq!(log.len(), 2);
        assert_eq!(log.get(1).expect("index 1").command, "c");
    }

    #[test]
    fn max_depth_trims_oldest() {
        let mut log = OperationLog::new(3);
        log.push(op("a"));
        log.push(op("b"));
        log.push(op("c"));
        log.push(op("d"));
        assert_eq!(log.len(), 3);
        let names: Vec<&str> = log.iter().map(|o| o.command.as_str()).collect();
        assert_eq!(names, vec!["b", "c", "d"]);
    }

    #[test]
    fn empty_log_returns_none() {
        let mut log = OperationLog::new(2);
        assert!(log.undo().is_none());
        assert!(log.redo().is_none());
    }

    #[test]
    fn clear_resets_position_and_history() {
        let mut log = OperationLog::new(4);
        log.push(op("a"));
        log.push(op("b"));
        log.clear();
        assert!(log.is_empty());
        assert_eq!(log.position(), 0);
        assert!(!log.can_undo());
    }

    #[test]
    fn ai_generated_flag_preserved() {
        let mut log = OperationLog::new(4);
        log.push(op("ai_recolor").as_ai_generated());
        assert!(log.get(0).expect("index 0").ai_generated);
    }

    #[test]
    fn position_invariant_after_undo_to_zero() {
        let mut log = OperationLog::new(4);
        log.push(op("a"));
        log.undo();
        assert_eq!(log.position(), 0);
        assert!(log.undo().is_none());
    }

    #[test]
    fn max_depth_zero_is_clamped_to_one() {
        let mut log = OperationLog::new(0);
        log.push(op("a"));
        log.push(op("b"));
        assert_eq!(log.len(), 1);
        assert_eq!(log.get(0).expect("index 0").command, "b");
    }

    #[test]
    fn restore_from_replaces_history_and_marks_all_applied() {
        let mut log = OperationLog::new(8);
        log.push(op("z")); // ensure pre-existing state is replaced
        log.restore_from(vec![op("a"), op("b"), op("c")]);
        assert_eq!(log.len(), 3);
        let names: Vec<&str> = log.iter().map(|o| o.command.as_str()).collect();
        assert_eq!(names, vec!["a", "b", "c"]);
        assert!(log.can_undo());
        assert!(!log.can_redo());
        assert_eq!(log.position(), 3);
    }

    #[test]
    fn restore_from_drops_front_when_over_max_depth() {
        let mut log = OperationLog::new(2);
        log.restore_from(vec![op("a"), op("b"), op("c"), op("d")]);
        let names: Vec<&str> = log.iter().map(|o| o.command.as_str()).collect();
        assert_eq!(names, vec!["c", "d"], "keep the most recent");
    }

    #[test]
    fn operation_serialize_roundtrip() {
        let o = Operation::new(
            "user",
            "move",
            json!({"x": 0}),
            json!({"x": 5}),
            vec![Uuid::new_v4()],
        );
        let s = serde_json::to_string(&o).expect("serialize");
        let o2: Operation = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(o, o2);
    }
}
