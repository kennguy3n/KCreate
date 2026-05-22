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
pub mod llm_chat;
pub mod llm_sidecar;
pub mod task_router;

pub use action_log::{ActionLog, AiAction};
pub use bg_remove::{remove_background, BgRemoveOptions};
pub use llm_chat::{
    build_system_prompt, chat_completion, parse_completion, ChatError, ChatMessage, ChatRequest,
    ChatResponse, ChatResult, ChatRole,
};
pub use llm_sidecar::{LlmSidecar, SidecarConfig, SidecarError, SidecarResult, SidecarStatus};
pub use task_router::{execute_task, AiError, AiResult, AiTask};
