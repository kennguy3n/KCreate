//! Integration-test host crate.
//!
//! Production crates live under `crates/kcreate_*`. This crate exists
//! purely so the workspace can host integration tests under
//! `crates/kcreate_tests/tests/` that exercise multiple crates
//! together. The library has no public API; see the `tests/`
//! directory for the actual scenarios.
