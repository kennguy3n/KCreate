//! Integration-test host crate.
//!
//! Production crates live under `crates/kcreate_*`. This crate exists
//! purely so the workspace can host integration tests under
//! `crates/kcreate_tests/tests/` that exercise multiple crates
//! together. The library has no public API; see the `tests/`
//! directory for the actual scenarios.

// Per CONTRIBUTING.md, every crate root must forbid the legacy
// implicit-unsafe form. This crate ships no `unsafe` at all, but the
// attribute enforces the workspace convention uniformly so a future
// addition can't slip through unaudited.
#![forbid(unsafe_op_in_unsafe_fn)]
