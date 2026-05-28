//! `kcreate_perf` — lightweight cold-path / startup profiling
//! primitives (Phase 8 Block E Task 27).
//!
//! The crate ships three things:
//!
//! 1. [`Timeline`] — an append-only sequence of named time marks.
//!    Each mark carries a monotonic nanosecond offset from the
//!    timeline's start `Instant`. Used to instrument cold paths
//!    (bridge init, workspace open, scene-sync first pass, native-
//!    canvas first frame).
//! 2. [`Scope`] — RAII span helper that auto-marks `<label>.end`
//!    on drop. Lets callers wrap a block in a single line and get
//!    automatic start/end marks without duplicating the label.
//! 3. [`startup`] — a process-wide singleton `Timeline` keyed on
//!    `STARTUP`. The bridge initialises it on the first cold path
//!    it owns (process load, first `Workspace` open, first scene
//!    sync), and the renderer can pull a JSON snapshot for the
//!    diagnostics overlay via the bridge.
//!
//! The crate deliberately has **no networking, no async, and no
//! tokio dependency**. The only deps are `serde` + `serde_json`
//! for the JSON report shape — both already in the workspace. It
//! is safe to add to the local-first editing-path tree.

#![forbid(unsafe_op_in_unsafe_fn)]

mod report;
pub mod startup;
mod timeline;

pub use report::{MarkReport, PhaseReport, Report};
pub use timeline::{Mark, Scope, Timeline};
