//! [`Timeline`] — append-only mark sequence + [`Scope`] RAII helper.
//!
//! `Timeline::start` captures `Instant::now()` and a UTC unix-ms
//! timestamp (for the JSON report; the per-mark math always uses
//! the monotonic `Instant` so we never observe wall-clock skew).
//! Marks accumulate in insertion order and carry a label + a
//! monotonic-nanosecond offset from the timeline start.

use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::report::{MarkReport, PhaseReport, Report};

/// A single named time point inside a [`Timeline`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mark {
    /// Human-readable label, e.g. `"bridge.workspace.opened"`.
    pub label: String,
    /// Monotonic nanoseconds since the parent timeline's start. Never
    /// goes backward, even across system-clock jumps, because it is
    /// derived from `Instant::duration_since`.
    pub monotonic_ns: u64,
}

/// Append-only timeline of named marks.
///
/// Construct via [`Timeline::start`], then call [`Timeline::mark`]
/// at each phase boundary. When done, call [`Timeline::finish`] to
/// freeze the timeline into a [`Report`] suitable for JSON
/// serialisation.
///
/// The type is `Send + Sync` only at the API surface — internal
/// fields are owned by the type, but the caller is responsible for
/// providing synchronisation (e.g. a `Mutex`) if multiple threads
/// need to mark the same instance. The [`startup`](crate::startup)
/// module wraps a global `Mutex<Timeline>` for the common case.
#[derive(Debug)]
pub struct Timeline {
    name: String,
    /// Monotonic anchor — every `Mark::monotonic_ns` is measured
    /// from this `Instant`.
    started_at: Instant,
    /// Wall-clock anchor — milliseconds since the Unix epoch.
    /// Reported in the JSON output for human correlation with
    /// system logs; never used for delta math.
    started_at_unix_ms: i64,
    marks: Vec<Mark>,
}

impl Timeline {
    /// Begin a new timeline. The `name` shows up in the JSON
    /// report so the consumer can tell `startup` apart from
    /// e.g. `workspace.first_open`.
    #[must_use]
    pub fn start(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            started_at: Instant::now(),
            started_at_unix_ms: now_unix_ms(),
            marks: Vec::new(),
        }
    }

    /// Record a mark at the current instant.
    pub fn mark(&mut self, label: impl Into<String>) {
        let elapsed = self.started_at.elapsed();
        // u64 nanos can hold ~584 years from `started_at` before
        // saturating; we treat the saturation as "stop being
        // useful, do not panic" since timeline overflow on a desktop
        // process means something has gone deeply wrong.
        let monotonic_ns = u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX);
        self.marks.push(Mark {
            label: label.into(),
            monotonic_ns,
        });
    }

    /// Begin a RAII [`Scope`]. The scope marks `<label>.start`
    /// immediately and `<label>.end` on drop, so callers don't
    /// have to remember to call `mark` at the end of a block.
    pub fn scope(&mut self, label: impl Into<String>) -> Scope<'_> {
        let label_string = label.into();
        self.mark(format!("{label_string}.start"));
        Scope {
            timeline: self,
            label: label_string,
            ended: false,
        }
    }

    /// Timeline name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Number of marks recorded so far.
    #[must_use]
    pub fn len(&self) -> usize {
        self.marks.len()
    }

    /// Is the timeline empty?
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.marks.is_empty()
    }

    /// Borrow the marks recorded so far.
    #[must_use]
    pub fn marks(&self) -> &[Mark] {
        &self.marks
    }

    /// Freeze the timeline into a [`Report`]. Consumes the
    /// timeline because a report is meant to be a snapshot — the
    /// caller takes the marks once and renders / persists them.
    /// Use [`Self::snapshot`] for a non-consuming preview.
    #[must_use]
    pub fn finish(self) -> Report {
        self.snapshot()
    }

    /// Non-consuming snapshot. Useful for diagnostics overlays that
    /// want to render the timeline mid-flight without taking
    /// ownership.
    #[must_use]
    pub fn snapshot(&self) -> Report {
        let elapsed = self.started_at.elapsed();
        let total_ns = u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX);
        let marks: Vec<MarkReport> = self
            .marks
            .iter()
            .map(|m| MarkReport {
                label: m.label.clone(),
                monotonic_ns: m.monotonic_ns,
            })
            .collect();
        let phases = derive_phases(&self.marks, total_ns);
        Report {
            name: self.name.clone(),
            started_at_unix_ms: self.started_at_unix_ms,
            total_ns,
            marks,
            phases,
        }
    }
}

/// RAII span produced by [`Timeline::scope`]. Marks `<label>.end`
/// on drop unless [`Scope::end`] was called explicitly.
#[derive(Debug)]
pub struct Scope<'a> {
    timeline: &'a mut Timeline,
    label: String,
    ended: bool,
}

impl Scope<'_> {
    /// End the scope explicitly. Idempotent — calling [`Scope::end`]
    /// followed by `drop` only records `<label>.end` once.
    pub fn end(mut self) {
        self.finish_inner();
    }

    fn finish_inner(&mut self) {
        if !self.ended {
            self.timeline.mark(format!("{}.end", self.label));
            self.ended = true;
        }
    }
}

impl Drop for Scope<'_> {
    fn drop(&mut self) {
        self.finish_inner();
    }
}

/// Compute the per-phase duration table by walking the marks in
/// order and emitting `(label, duration)` for each adjacent pair.
/// The final phase runs from the last mark to the timeline's
/// current `total_ns`, so callers always see "what is the rest of
/// the timeline doing after the last named mark".
fn derive_phases(marks: &[Mark], total_ns: u64) -> Vec<PhaseReport> {
    if marks.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(marks.len());
    for i in 0..marks.len() {
        let from = &marks[i];
        let end_ns = marks.get(i + 1).map_or(total_ns, |next| next.monotonic_ns);
        // Saturating subtraction defends against pathological cases
        // where someone manually constructs out-of-order marks. The
        // mark API itself uses `Instant::elapsed`, which is
        // monotonic, so adjacent pairs are always non-decreasing in
        // normal use.
        let duration_ns = end_ns.saturating_sub(from.monotonic_ns);
        out.push(PhaseReport {
            label: from.label.clone(),
            from_ns: from.monotonic_ns,
            to_ns: end_ns,
            duration_ns,
        });
    }
    out
}

fn now_unix_ms() -> i64 {
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(dur.as_millis()).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use std::thread;
    use std::time::Duration;

    use super::*;

    #[test]
    fn empty_timeline_reports_zero_marks() {
        let t = Timeline::start("empty");
        let r = t.finish();
        assert_eq!(r.name, "empty");
        assert!(r.marks.is_empty());
        assert!(r.phases.is_empty());
    }

    #[test]
    fn marks_preserve_insertion_order_and_monotonic_ns() {
        let mut t = Timeline::start("startup");
        t.mark("a");
        thread::sleep(Duration::from_millis(1));
        t.mark("b");
        thread::sleep(Duration::from_millis(1));
        t.mark("c");
        let r = t.finish();
        assert_eq!(r.marks.len(), 3);
        assert_eq!(r.marks[0].label, "a");
        assert_eq!(r.marks[1].label, "b");
        assert_eq!(r.marks[2].label, "c");
        assert!(r.marks[0].monotonic_ns <= r.marks[1].monotonic_ns);
        assert!(r.marks[1].monotonic_ns <= r.marks[2].monotonic_ns);
        assert!(r.total_ns >= r.marks[2].monotonic_ns);
    }

    #[test]
    fn phases_chain_from_each_mark_to_the_next() {
        let mut t = Timeline::start("phases");
        t.mark("first");
        thread::sleep(Duration::from_millis(2));
        t.mark("second");
        thread::sleep(Duration::from_millis(2));
        let r = t.finish();
        assert_eq!(r.phases.len(), 2);
        assert_eq!(r.phases[0].label, "first");
        assert_eq!(r.phases[0].to_ns, r.marks[1].monotonic_ns);
        // Second phase runs from `second` mark to the timeline's
        // current end (i.e. `total_ns`).
        assert_eq!(r.phases[1].label, "second");
        assert_eq!(r.phases[1].to_ns, r.total_ns);
        assert!(r.phases[0].duration_ns > 0);
        assert!(r.phases[1].duration_ns > 0);
    }

    #[test]
    fn scope_marks_start_and_end_on_drop() {
        let mut t = Timeline::start("scoped");
        {
            let _g = t.scope("init");
            thread::sleep(Duration::from_millis(1));
        }
        t.mark("after");
        let r = t.finish();
        let labels: Vec<&str> = r.marks.iter().map(|m| m.label.as_str()).collect();
        assert_eq!(labels, ["init.start", "init.end", "after"]);
    }

    #[test]
    fn scope_end_is_idempotent_with_drop() {
        let mut t = Timeline::start("scoped");
        let g = t.scope("explicit");
        g.end();
        let r = t.finish();
        let labels: Vec<&str> = r.marks.iter().map(|m| m.label.as_str()).collect();
        // Only one `.end` mark even though both `end()` and `Drop`
        // ran — the `ended` flag stops the second pass.
        assert_eq!(labels, ["explicit.start", "explicit.end"]);
    }

    #[test]
    fn snapshot_does_not_consume_and_keeps_recording() {
        let mut t = Timeline::start("snap");
        t.mark("one");
        let snap_a = t.snapshot();
        assert_eq!(snap_a.marks.len(), 1);
        t.mark("two");
        let snap_b = t.snapshot();
        assert_eq!(snap_b.marks.len(), 2);
        assert_eq!(snap_a.name, "snap");
        assert_eq!(snap_b.name, "snap");
    }

    #[test]
    fn report_serialises_to_stable_json_shape() {
        let mut t = Timeline::start("stable");
        t.mark("alpha");
        let r = t.finish();
        let json = serde_json::to_value(&r).expect("serialise report");
        let obj = json.as_object().expect("report is a json object");
        assert!(obj.contains_key("name"));
        assert!(obj.contains_key("started_at_unix_ms"));
        assert!(obj.contains_key("total_ns"));
        assert!(obj.contains_key("marks"));
        assert!(obj.contains_key("phases"));
        let mark = &obj["marks"][0];
        assert_eq!(mark["label"], "alpha");
        assert!(mark["monotonic_ns"].is_u64());
    }
}
