//! Process-wide startup [`Timeline`].
//!
//! The bridge owns one global timeline named `"startup"` that is
//! initialised the first time `init` is called (typically on
//! `dlopen` of the `bridge.node` cdylib). Subsequent callers (the
//! main-process IPC handler, the first workspace open, the first
//! scene-sync pass, the first native-canvas frame) call
//! [`mark`] to drop a named time point. The renderer pulls a
//! [`crate::Report`] snapshot for the diagnostics overlay via
//! [`snapshot`].
//!
//! Concurrency: the timeline lives behind a `Mutex`. Marks are
//! cheap (a `u64` push) so the lock is contended for a few
//! microseconds at most per call. We deliberately keep the lock
//! scope tight so a slow mark caller never blocks an unrelated
//! one.

use std::sync::{Mutex, OnceLock};

use crate::report::Report;
use crate::timeline::Timeline;

/// Lazily-initialised global startup timeline.
fn cell() -> &'static Mutex<Option<Timeline>> {
    static CELL: OnceLock<Mutex<Option<Timeline>>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(None))
}

/// Initialise the global startup timeline if it has not been
/// initialised yet. Idempotent — the second + later calls are
/// silent no-ops. The caller passes the timeline `name` (the
/// bridge passes `"bridge.startup"`); this is *not* the first
/// mark label.
pub fn init(name: impl Into<String>) {
    let mut slot = cell().lock().expect("startup timeline mutex poisoned");
    if slot.is_none() {
        *slot = Some(Timeline::start(name));
    }
}

/// Drop a mark on the global startup timeline. No-op if [`init`]
/// has not been called yet — this lets the bridge sprinkle
/// `kcreate_perf::startup::mark(...)` calls in cold-path sites
/// without worrying about ordering during early boot.
pub fn mark(label: impl Into<String>) {
    let mut slot = cell().lock().expect("startup timeline mutex poisoned");
    if let Some(t) = slot.as_mut() {
        t.mark(label);
    }
}

/// Snapshot the global startup timeline. Returns `None` if
/// [`init`] has never been called.
#[must_use]
pub fn snapshot() -> Option<Report> {
    let slot = cell().lock().expect("startup timeline mutex poisoned");
    slot.as_ref().map(Timeline::snapshot)
}

/// Drop `<label>.start` immediately and return a RAII guard that
/// drops `<label>.end` when it goes out of scope. This is the
/// orphan-mark-proof variant of [`mark`]: every exit path from the
/// caller — including early `return`, `?` propagation, panic
/// unwinding, and the success path — will emit the matching `.end`
/// mark.
///
/// Calling [`StartupScope::end`] explicitly is idempotent; the
/// `Drop` impl is suppressed in that case so the timeline never
/// records `<label>.end` twice.
///
/// Like [`mark`], the `.start` and `.end` writes are silent no-ops
/// when [`init`] has not yet been called.
#[must_use = "the returned guard must be held in a binding so the .end mark fires when the scope exits"]
pub fn scope(label: impl Into<String>) -> StartupScope {
    let label = label.into();
    mark(format!("{label}.start"));
    StartupScope {
        label,
        ended: false,
    }
}

/// RAII guard returned by [`scope`]. Emits `<label>.end` on drop.
///
/// This is intentionally a distinct type from
/// [`crate::timeline::Scope`] (which borrows a `&mut Timeline`):
/// `StartupScope` reaches into the process-wide singleton on drop
/// so it can be moved around freely and stored across fallible
/// call sites in the bridge.
#[derive(Debug)]
pub struct StartupScope {
    label: String,
    ended: bool,
}

impl StartupScope {
    /// Emit `<label>.end` now. Subsequent calls (and the `Drop`
    /// impl) are silent no-ops.
    pub fn end(&mut self) {
        if !self.ended {
            mark(format!("{}.end", self.label));
            self.ended = true;
        }
    }
}

impl Drop for StartupScope {
    fn drop(&mut self) {
        self.end();
    }
}

/// Test-only escape hatch — clear the global timeline so each test
/// starts from a clean slate. Behind `cfg(any(test, feature =
/// "test_support"))` to keep the production surface tight. The
/// bridge tests in `kcreate_bridge` will call this via the
/// `test_support` feature.
#[cfg(any(test, feature = "test_support"))]
pub fn reset_for_tests() {
    let mut slot = cell().lock().expect("startup timeline mutex poisoned");
    *slot = None;
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// Global mutex shared by all startup tests. The `cell()`
    /// behind `init` / `mark` is itself process-wide, so two
    /// tests touching it can stomp on each other if they run in
    /// parallel. Serialise them here.
    fn global_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn init_is_idempotent() {
        let _guard = global_lock().lock().unwrap();
        reset_for_tests();
        init("startup");
        mark("first");
        init("ignored-second-name"); // must be silent no-op
        mark("second");
        let report = snapshot().expect("global initialised");
        assert_eq!(report.name, "startup");
        let labels: Vec<&str> = report.marks.iter().map(|m| m.label.as_str()).collect();
        assert_eq!(labels, ["first", "second"]);
    }

    #[test]
    fn mark_is_silent_noop_before_init() {
        let _guard = global_lock().lock().unwrap();
        reset_for_tests();
        mark("dropped");
        assert!(snapshot().is_none());
        init("late");
        mark("kept");
        let report = snapshot().expect("global initialised");
        let labels: Vec<&str> = report.marks.iter().map(|m| m.label.as_str()).collect();
        assert_eq!(labels, ["kept"]);
    }

    #[test]
    fn scope_emits_start_and_end_marks_on_normal_exit() {
        let _guard = global_lock().lock().unwrap();
        reset_for_tests();
        init("scope_test");
        {
            let _s = scope("phase");
            mark("inside");
        }
        let report = snapshot().expect("global initialised");
        let labels: Vec<&str> = report.marks.iter().map(|m| m.label.as_str()).collect();
        assert_eq!(labels, ["phase.start", "inside", "phase.end"]);
    }

    // Simulate a fallible function with an early-return branch.
    // The `.end` mark must still fire when the guard goes out of
    // scope as the `Err` is propagated through `?`. Defined at
    // module scope so clippy's `items_after_statements` lint is
    // satisfied.
    fn fallible_scope_user() -> Result<(), &'static str> {
        let _s = scope("fallible");
        Err("boom")?;
        // Unreachable; the `.end` mark is emitted via the guard's
        // `Drop` impl on the early return above.
        Ok(())
    }

    #[test]
    fn scope_emits_end_mark_on_early_return() {
        let _guard = global_lock().lock().unwrap();
        reset_for_tests();
        init("scope_test");
        let _ = fallible_scope_user();
        let report = snapshot().expect("global initialised");
        let labels: Vec<&str> = report.marks.iter().map(|m| m.label.as_str()).collect();
        assert_eq!(labels, ["fallible.start", "fallible.end"]);
    }

    #[test]
    fn scope_explicit_end_suppresses_drop_emission() {
        let _guard = global_lock().lock().unwrap();
        reset_for_tests();
        init("scope_test");
        {
            let mut s = scope("phase");
            mark("inside");
            s.end();
            // Drop after this point must be a no-op so the timeline
            // never sees `<label>.end` twice.
        }
        let report = snapshot().expect("global initialised");
        let labels: Vec<&str> = report.marks.iter().map(|m| m.label.as_str()).collect();
        assert_eq!(labels, ["phase.start", "inside", "phase.end"]);
    }

    #[test]
    fn scope_is_silent_noop_before_init() {
        let _guard = global_lock().lock().unwrap();
        reset_for_tests();
        // Drop the guard with no initialized timeline. This must
        // not panic and must not leave any global state behind.
        {
            let _s = scope("dropped");
            mark("dropped-inside");
        }
        assert!(snapshot().is_none());
        // Now init and verify the previously-dropped scope did not
        // leak through — only the marks after init should appear.
        init("late");
        mark("kept");
        let report = snapshot().expect("global initialised");
        let labels: Vec<&str> = report.marks.iter().map(|m| m.label.as_str()).collect();
        assert_eq!(labels, ["kept"]);
    }

    #[test]
    fn snapshot_is_non_consuming() {
        let _guard = global_lock().lock().unwrap();
        reset_for_tests();
        init("snapshot");
        mark("phase_one");
        let a = snapshot().expect("first snapshot");
        mark("phase_two");
        let b = snapshot().expect("second snapshot");
        assert_eq!(a.marks.len(), 1);
        assert_eq!(b.marks.len(), 2);
    }
}
