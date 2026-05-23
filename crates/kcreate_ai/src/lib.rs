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
pub mod model_registry;
pub mod palette;
pub mod screenshot_to_layout;
pub mod smart_select;
pub mod task_router;
pub mod upscale;

pub use action_log::{ActionLog, AiAction};
pub use bg_remove::{
    apply_alpha_mask, remove_background, remove_background_with_backend, BgRemovalBackend,
    BgRemoveError, BgRemoveOptions,
};
pub use llm_chat::{
    build_system_prompt, chat_completion, parse_completion, ChatError, ChatMessage, ChatRequest,
    ChatResponse, ChatResult, ChatRole,
};
pub use llm_sidecar::{LlmSidecar, SidecarConfig, SidecarError, SidecarResult, SidecarStatus};
pub use model_registry::{
    install_model_pack, is_installed, list_model_packs, pack_path, uninstall_model_pack,
    InstallError, InstallReport, ModelKind, ModelPack, ModelPackCategory,
};
pub use palette::{extract_palette, ExtractedColor};
pub use screenshot_to_layout::{
    analyze_screenshot_for_layout, Bounds as ScreenshotBounds, DetectedElement, ElementType,
};
pub use smart_select::smart_select;
pub use task_router::{
    build_accessibility_prompt, build_design_token_prompt, build_layer_naming_prompt, execute_task,
    parse_layer_naming_reply, AiError, AiResult, AiTask,
};
pub use upscale::{upscale_lanczos, UpscaleError};
