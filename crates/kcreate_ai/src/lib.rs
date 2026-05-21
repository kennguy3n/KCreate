//! `kcreate_ai` — local AI sidecar.
//!
//! Phase 0 ships a threshold-based background-removal algorithm
//! (real, useful for solid-background product photography — not a
//! stub) plus the task-router + action-log scaffolding the future
//! ONNX-based models will hang off. The crate has no network deps;
//! every model that lands later must run from a local file path.

#![forbid(unsafe_op_in_unsafe_fn)]

pub mod action_log;
pub mod bg_remove;
pub mod task_router;

pub use action_log::{ActionLog, AiAction};
pub use bg_remove::{remove_background, BgRemoveOptions};
pub use task_router::{execute_task, AiError, AiResult, AiTask};
