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
//!
//! ## Tasks 15–16 — grouping + branching
//!
//! **Undo grouping.** Compound operations such as "drag a node" fire
//! many tiny move ops but the user expects them to undo together. To
//! support this, [`Operation`] carries an optional [`Operation::group_id`]
//! and the log exposes [`OperationLog::undo_group`] /
//! [`OperationLog::redo_group`] that walk over a contiguous run of
//! ops sharing the same `group_id` atomically. Single-op semantics
//! still work via [`OperationLog::undo`] / [`OperationLog::redo`] —
//! grouping is opt-in per push.
//!
//! **Branching undo.** The default behaviour of [`OperationLog::push`]
//! still truncates the redo tail (LIFO undo), but the discarded tail
//! is captured as a [`DiscardedBranch`] retained on the log (bounded
//! by [`OperationLog::max_branches`]). Callers can list available
//! branches at the current anchor via [`OperationLog::branches_here`]
//! and revive one with [`OperationLog::restore_branch`]. This gives
//! the renderer a one-click "recover the work I undid before typing
//! something new" UX without forcing a global tree-mode rewrite of
//! every consumer.

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
    /// Optional group identifier — ops sharing the same `group_id`
    /// undo/redo as a single user-visible action (see
    /// [`OperationLog::undo_group`]). `None` means "undo independently".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<Uuid>,
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
            group_id: None,
        }
    }

    /// Mark this operation as AI-generated. Useful for filtering the
    /// history view and for the safety layer.
    #[must_use]
    pub const fn as_ai_generated(mut self) -> Self {
        self.ai_generated = true;
        self
    }

    /// Tag this operation with a group id so subsequent
    /// [`OperationLog::undo_group`] / [`OperationLog::redo_group`]
    /// treat the contiguous run sharing the same id as one atomic
    /// user action.
    #[must_use]
    pub const fn with_group(mut self, group_id: Uuid) -> Self {
        self.group_id = Some(group_id);
        self
    }
}

/// A redo tail that was discarded by a [`OperationLog::push`] while a
/// redo stack still existed. Retained so the renderer can offer the
/// user a "recover branch" affordance after they undid work and then
/// typed something new.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiscardedBranch {
    /// Position in the timeline this branch would attach to. The
    /// branch is valid for restore only while `OperationLog::position`
    /// equals this value.
    pub anchor_position: usize,
    /// When the branch was discarded — used by the panel to sort
    /// most-recent-first.
    pub discarded_at: DateTime<Utc>,
    /// The operations that used to sit in the redo tail, oldest first.
    pub ops: Vec<Operation>,
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
    /// Discarded redo tails kept around for the "restore branch" UX.
    /// Bounded by `max_branches`; oldest entries are dropped via
    /// `pop_front` when the bound is exceeded.
    #[serde(default)]
    branches: VecDeque<DiscardedBranch>,
    /// Bound on the number of retained discarded branches. Set
    /// independently from `max_depth` because branches are typically
    /// short and we want to retain a handful regardless of the
    /// per-op trim.
    #[serde(default = "default_max_branches")]
    max_branches: usize,
}

const fn default_max_branches() -> usize {
    16
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
            branches: VecDeque::new(),
            max_branches: default_max_branches(),
        }
    }

    /// Append an operation. Any pending redo entries (between
    /// `position` and `history.len()`) are captured as a
    /// [`DiscardedBranch`] before being truncated so the user can
    /// recover them via [`Self::restore_branch`].
    pub fn push(&mut self, op: Operation) {
        if self.position < self.history.len() {
            let tail: Vec<Operation> = self.history.drain(self.position..).collect();
            self.branches.push_back(DiscardedBranch {
                anchor_position: self.position,
                discarded_at: Utc::now(),
                ops: tail,
            });
            while self.branches.len() > self.max_branches {
                self.branches.pop_front();
            }
        }
        self.history.push_back(op);
        // Bound the buffer — drop oldest entries. `VecDeque::pop_front`
        // is O(1). Each dropped op invalidates any branches whose
        // anchor lives below the new front, so we re-base anchors
        // (shift down by one) and drop those that fall to <= 0.
        while self.history.len() > self.max_depth {
            self.history.pop_front();
            self.branches.retain(|b| b.anchor_position > 0);
            for b in &mut self.branches {
                b.anchor_position -= 1;
            }
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

    /// Look at the operation that *would* be undone by [`Self::undo`]
    /// without moving the cursor. Returns `None` if the undo stack is
    /// empty.
    ///
    /// Exposed so a caller that needs to apply `before_patch`
    /// atomically (e.g. the bridge's host-side state replay) can
    /// validate / apply the patch first and only commit the cursor
    /// move via [`Self::undo`] on success. Without this, a failing
    /// patch application leaves the log cursor and the host state
    /// out of sync.
    #[must_use]
    pub fn peek_undo(&self) -> Option<&Operation> {
        if self.position == 0 {
            return None;
        }
        self.history.get(self.position - 1)
    }

    /// Look at the operation that *would* be re-applied by
    /// [`Self::redo`] without moving the cursor. See [`Self::peek_undo`]
    /// for the atomicity rationale.
    #[must_use]
    pub fn peek_redo(&self) -> Option<&Operation> {
        if self.position >= self.history.len() {
            return None;
        }
        self.history.get(self.position)
    }

    /// Look at the contiguous group of ops that the next
    /// [`Self::undo_group`] would consume, without moving the cursor.
    /// Returned in undo order (newest-first). The bridge uses this
    /// to apply all `before_patch`es atomically before committing
    /// the cursor move via [`Self::undo_group`]; if a patch fails,
    /// the cursor stays put so the next attempt retries the same
    /// group.
    #[must_use]
    pub fn peek_undo_group(&self) -> Vec<Operation> {
        let mut out = Vec::new();
        if self.position == 0 {
            return out;
        }
        let head_group = self
            .history
            .get(self.position - 1)
            .and_then(|op| op.group_id);
        let mut i = self.position;
        loop {
            if i == 0 {
                break;
            }
            let next_group = self.history.get(i - 1).and_then(|op| op.group_id);
            if !out.is_empty() && (next_group.is_none() || next_group != head_group) {
                break;
            }
            i -= 1;
            if let Some(op) = self.history.get(i) {
                out.push(op.clone());
            }
            if head_group.is_none() {
                break;
            }
        }
        out
    }

    /// Look at the contiguous group of ops that the next
    /// [`Self::redo_group`] would consume, without moving the cursor.
    /// Returned in redo order (oldest-first). Symmetric with
    /// [`Self::peek_undo_group`].
    #[must_use]
    pub fn peek_redo_group(&self) -> Vec<Operation> {
        let mut out = Vec::new();
        if self.position >= self.history.len() {
            return out;
        }
        let head_group = self.history.get(self.position).and_then(|op| op.group_id);
        let mut i = self.position;
        loop {
            if i >= self.history.len() {
                break;
            }
            let next_group = self.history.get(i).and_then(|op| op.group_id);
            if !out.is_empty() && (next_group.is_none() || next_group != head_group) {
                break;
            }
            if let Some(op) = self.history.get(i) {
                out.push(op.clone());
            }
            i += 1;
            if head_group.is_none() {
                break;
            }
        }
        out
    }

    /// Walk backwards over a contiguous run of ops that share a
    /// `group_id`. If the op just before `position` carries
    /// `group_id = Some(g)`, this also consumes every preceding op
    /// with the same `g` until a different (or no) group is found.
    /// Returns the consumed ops in undo order (newest-first) so the
    /// caller can apply each `before_patch` in turn. Returns an empty
    /// `Vec` if there is nothing to undo.
    ///
    /// For ungrouped ops this is identical to calling [`Self::undo`]
    /// once.
    pub fn undo_group(&mut self) -> Vec<Operation> {
        let mut out = Vec::new();
        if self.position == 0 {
            return out;
        }
        let head_group = self
            .history
            .get(self.position - 1)
            .and_then(|op| op.group_id);
        loop {
            if self.position == 0 {
                break;
            }
            let next_group = self
                .history
                .get(self.position - 1)
                .and_then(|op| op.group_id);
            if !out.is_empty() && (next_group.is_none() || next_group != head_group) {
                break;
            }
            self.position -= 1;
            if let Some(op) = self.history.get(self.position) {
                out.push(op.clone());
            }
            // Single-op (ungrouped) path — only consume one entry.
            if head_group.is_none() {
                break;
            }
        }
        out
    }

    /// Walk forwards over a contiguous run of ops that share a
    /// `group_id`. Mirrors [`Self::undo_group`]; returns the consumed
    /// ops in redo order (oldest-first) for `after_patch` application.
    pub fn redo_group(&mut self) -> Vec<Operation> {
        let mut out = Vec::new();
        if self.position >= self.history.len() {
            return out;
        }
        let head_group = self.history.get(self.position).and_then(|op| op.group_id);
        loop {
            if self.position >= self.history.len() {
                break;
            }
            let next_group = self.history.get(self.position).and_then(|op| op.group_id);
            if !out.is_empty() && (next_group.is_none() || next_group != head_group) {
                break;
            }
            if let Some(op) = self.history.get(self.position) {
                out.push(op.clone());
            }
            self.position += 1;
            if head_group.is_none() {
                break;
            }
        }
        out
    }

    /// Branches that were discarded at the current cursor position
    /// and are eligible for one-click restore. Newest-first.
    pub fn branches_here(&self) -> impl Iterator<Item = &DiscardedBranch> {
        self.branches
            .iter()
            .rev()
            .filter(move |b| b.anchor_position == self.position)
    }

    /// All retained discarded branches across the timeline, newest-
    /// first. Useful for the history panel's "Branches" tab.
    pub fn branches(&self) -> impl ExactSizeIterator<Item = &DiscardedBranch> {
        self.branches.iter()
    }

    #[must_use]
    pub const fn max_branches(&self) -> usize {
        self.max_branches
    }

    /// Replace the current redo tail with the ops from the discarded
    /// branch at `index_from_back` (0 = newest). The current redo
    /// tail (if any) is moved into a new `DiscardedBranch` so the
    /// swap is reversible. Returns `true` on success, `false` if
    /// the index is out of range or the branch's `anchor_position`
    /// no longer matches the current cursor (i.e. the user moved on
    /// and the branch is stale).
    pub fn restore_branch(&mut self, index_from_back: usize) -> bool {
        let total = self.branches.len();
        if index_from_back >= total {
            return false;
        }
        let abs_idx = total - 1 - index_from_back;
        let Some(branch) = self.branches.get(abs_idx) else {
            return false;
        };
        if branch.anchor_position != self.position {
            return false;
        }
        let branch = self
            .branches
            .remove(abs_idx)
            .expect("get returned Some, remove must succeed");
        // Capture the current redo tail (if any) as a new discarded
        // branch so the swap is reversible.
        if self.position < self.history.len() {
            let tail: Vec<Operation> = self.history.drain(self.position..).collect();
            self.branches.push_back(DiscardedBranch {
                anchor_position: self.position,
                discarded_at: Utc::now(),
                ops: tail,
            });
            while self.branches.len() > self.max_branches {
                self.branches.pop_front();
            }
        }
        // Append the restored branch ops; cursor stays at `position`
        // so the renderer can redo through them with the existing
        // `redo()` / `redo_group()` helpers.
        for op in branch.ops {
            self.history.push_back(op);
        }
        // Bound the buffer; re-base any remaining branches.
        while self.history.len() > self.max_depth {
            self.history.pop_front();
            self.branches.retain(|b| b.anchor_position > 0);
            for b in &mut self.branches {
                b.anchor_position -= 1;
            }
            self.position = self.position.saturating_sub(1);
        }
        true
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
        self.branches.clear();
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
            // Branches anchored at the dropped slot are invalidated;
            // surviving branches are rebased by one.
            self.branches.retain(|b| b.anchor_position > 0);
            for b in &mut self.branches {
                b.anchor_position -= 1;
            }
        }
        if self.position > self.history.len() {
            self.position = self.history.len();
        }
    }

    /// Configure the bound on retained discarded branches. Clamped
    /// to `>= 1`. Trims oldest branches if the new bound is smaller.
    pub fn set_max_branches(&mut self, new_max: usize) {
        let new_max = new_max.max(1);
        self.max_branches = new_max;
        while self.branches.len() > self.max_branches {
            self.branches.pop_front();
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
        // Restored sessions do not carry forward discarded-branch
        // state — those are session-scoped UX recovery aids, not
        // persistent history.
        self.branches.clear();
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

    #[test]
    fn operation_group_id_serialize_roundtrip() {
        let g = Uuid::new_v4();
        let o = Operation::new("user", "move", json!({}), json!({}), Vec::new()).with_group(g);
        let s = serde_json::to_string(&o).expect("serialize");
        let o2: Operation = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(o.group_id, Some(g));
        assert_eq!(o2.group_id, Some(g));
    }

    #[test]
    fn operation_without_group_skips_group_id_in_json() {
        // `group_id = None` must not appear in the serialized form
        // so that older persisted operation logs (no field at all)
        // round-trip cleanly and stay byte-compatible.
        let o = Operation::new("user", "move", json!({}), json!({}), Vec::new());
        let s = serde_json::to_string(&o).expect("serialize");
        assert!(
            !s.contains("group_id"),
            "no group_id field when absent: {s}"
        );
    }

    #[test]
    fn undo_group_collapses_contiguous_run_into_one_step() {
        let mut log = OperationLog::new(16);
        let g = Uuid::new_v4();
        log.push(op("baseline"));
        log.push(op("drag-start").with_group(g));
        log.push(op("drag-mid").with_group(g));
        log.push(op("drag-end").with_group(g));
        log.push(op("after-drag"));

        // First undo_group consumes the lone "after-drag".
        let undone = log.undo_group();
        assert_eq!(undone.len(), 1);
        assert_eq!(undone[0].command, "after-drag");

        // Second undo_group must consume the whole 3-op drag at once.
        let undone = log.undo_group();
        assert_eq!(undone.len(), 3);
        assert_eq!(undone[0].command, "drag-end");
        assert_eq!(undone[1].command, "drag-mid");
        assert_eq!(undone[2].command, "drag-start");

        // Cursor is now at 1, baseline remains.
        assert_eq!(log.position(), 1);

        // Redo_group puts the whole drag back.
        let redone = log.redo_group();
        assert_eq!(redone.len(), 3);
        assert_eq!(redone[0].command, "drag-start");
        assert_eq!(redone[1].command, "drag-mid");
        assert_eq!(redone[2].command, "drag-end");
        assert_eq!(log.position(), 4);
    }

    #[test]
    fn undo_group_does_not_cross_different_groups() {
        let mut log = OperationLog::new(16);
        let g1 = Uuid::new_v4();
        let g2 = Uuid::new_v4();
        log.push(op("g1-a").with_group(g1));
        log.push(op("g1-b").with_group(g1));
        log.push(op("g2-a").with_group(g2));
        log.push(op("g2-b").with_group(g2));

        let undone = log.undo_group();
        assert_eq!(undone.len(), 2, "only g2 group");
        assert_eq!(undone[0].command, "g2-b");
        assert_eq!(undone[1].command, "g2-a");

        let undone = log.undo_group();
        assert_eq!(undone.len(), 2, "only g1 group");
        assert_eq!(undone[0].command, "g1-b");
        assert_eq!(undone[1].command, "g1-a");

        assert_eq!(log.position(), 0);
    }

    #[test]
    fn undo_group_handles_ungrouped_op_atomically() {
        let mut log = OperationLog::new(16);
        log.push(op("a"));
        log.push(op("b"));
        let undone = log.undo_group();
        assert_eq!(undone.len(), 1);
        assert_eq!(undone[0].command, "b");
        let undone = log.undo_group();
        assert_eq!(undone.len(), 1);
        assert_eq!(undone[0].command, "a");
        assert!(log.undo_group().is_empty());
    }

    #[test]
    fn push_after_undo_captures_redo_tail_as_branch() {
        let mut log = OperationLog::new(16);
        log.push(op("a"));
        log.push(op("b"));
        log.push(op("c"));
        log.undo();
        log.undo();
        // Cursor at 1, redo tail = [b, c]. Pushing a new op must
        // capture [b, c] as a discarded branch.
        log.push(op("alt"));
        assert_eq!(log.branches().len(), 1);
        let bs: Vec<_> = log.branches().collect();
        assert_eq!(bs[0].anchor_position, 1);
        let cmds: Vec<&str> = bs[0].ops.iter().map(|o| o.command.as_str()).collect();
        assert_eq!(cmds, vec!["b", "c"]);
    }

    #[test]
    fn restore_branch_swaps_redo_tail_back_in() {
        let mut log = OperationLog::new(16);
        log.push(op("a"));
        log.push(op("b"));
        log.push(op("c"));
        log.undo();
        log.undo();
        log.push(op("alt-1"));
        log.push(op("alt-2"));
        // Undo back to the anchor (position == 1) so the branch can
        // be restored. The "alt-1, alt-2" tail becomes a branch.
        log.undo();
        log.undo();
        assert_eq!(log.position(), 1);
        assert_eq!(log.branches().len(), 1);

        let ok = log.restore_branch(0);
        assert!(ok);
        // After restore: position still 1, redo tail = [b, c].
        let restored: Vec<&str> = log
            .iter()
            .skip(log.position())
            .map(|o| o.command.as_str())
            .collect();
        assert_eq!(restored, vec!["b", "c"]);
        // The displaced tail [alt-1, alt-2] is now itself a branch.
        let bs: Vec<_> = log.branches().collect();
        assert_eq!(bs.len(), 1);
        let cmds: Vec<&str> = bs[0].ops.iter().map(|o| o.command.as_str()).collect();
        assert_eq!(cmds, vec!["alt-1", "alt-2"]);
    }

    #[test]
    fn restore_branch_rejects_stale_anchor() {
        let mut log = OperationLog::new(16);
        log.push(op("a"));
        log.push(op("b"));
        log.undo();
        log.push(op("alt")); // captures [b] @ anchor 1
                             // Cursor is now at 2, anchor of branch is 1 → stale.
        assert!(!log.restore_branch(0));
    }

    #[test]
    fn max_branches_bound_drops_oldest() {
        let mut log = OperationLog::new(16);
        log.set_max_branches(2);
        // Generate 3 branches: undo, push (drops a redo tail of 1), repeat.
        log.push(op("a"));
        log.push(op("b"));
        log.undo();
        log.push(op("alt1")); // branch 1 captured

        log.push(op("c"));
        log.undo();
        log.push(op("alt2")); // branch 2 captured

        log.push(op("d"));
        log.undo();
        log.push(op("alt3")); // branch 3 captured, branch 1 dropped

        assert_eq!(log.branches().len(), 2);
    }
}
