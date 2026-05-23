//! `kcreate_ai` — local AI sidecar.
//!
//! Phase 0 ships a threshold-based background-removal algorithm
//! (real, useful for solid-background product photography — not a
//! stub) plus the task-router + action-log scaffolding the future
//! ONNX-based models will hang off. The crate has no network deps;
//! every model that lands later must run from a local file path.

#![forbid(unsafe_op_in_unsafe_fn)]

pub mod action_log;
pub mod alt_text;
pub mod bg_remove;
pub mod layout_suggest;
pub mod llm_chat;
pub mod llm_sidecar;
pub mod model_registry;
pub mod palette;
pub mod screenshot_to_layout;
pub mod smart_select;
pub mod task_router;
pub mod tool_call;
pub mod upscale;

pub use action_log::{ActionLog, AiAction};
pub use alt_text::{generate_alt_text, AltTextError, AltTextOptions, AltTextReport};
pub use bg_remove::{
    apply_alpha_mask, remove_background, remove_background_with_backend, BgRemovalBackend,
    BgRemoveError, BgRemoveOptions,
};
pub use layout_suggest::{
    suggest_layout_grouping, Bounds as LayoutBounds, LayoutAlignment, LayoutNode,
    LayoutOrientation, LayoutSuggestError, LayoutSuggestOptions, LayoutSuggestion,
};
pub use llm_chat::{
    build_system_prompt, build_tool_call_system_prompt, chat_completion, parse_completion,
    request_tool_call, ChatError, ChatMessage, ChatRequest, ChatResponse, ChatResult, ChatRole,
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
pub use tool_call::{
    default_design_registry, gbnf_for_registry, parse_tool_call_response, ToolCall,
    ToolCallParseError, ToolCallRegistry, ToolDescriptor, ToolParamType, ToolParameter,
    ToolRegistryError,
};
pub use upscale::{upscale_lanczos, UpscaleError};
