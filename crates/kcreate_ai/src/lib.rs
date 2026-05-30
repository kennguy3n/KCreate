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
pub mod auto_color;
pub mod bg_remove;
pub mod brand_extract;
pub mod brand_template;
pub mod color_range;
pub mod denoise;
pub mod design_critique;
pub mod design_tokens_vlm;
pub mod diffusion_sidecar;
pub mod glyph_extract;
pub mod iconify;
pub mod image_gen;
pub mod inpaint;
pub mod layout_suggest;
pub mod llm_chat;
pub mod llm_sidecar;
pub mod model_registry;
pub mod ocr;
pub mod one_pager;
pub mod palette;
pub mod palette_harmonize;
pub mod reformat;
pub mod screenshot_to_layout;
pub mod segment;
pub mod sidecar_dispatcher;
pub mod smart_crop;
pub mod smart_select;
pub mod stroke_match;
pub mod style_describe;
pub mod task_router;
pub mod tool_call;
pub mod trace;
pub mod type_pairing;
pub mod upscale;
pub mod vision_chat;

pub use action_log::{ActionLog, AiAction};
pub use alt_text::{generate_alt_text, AltTextError, AltTextOptions, AltTextReport};
pub use auto_color::{auto_color_correct, AutoColorError, AutoColorMode, AutoColorOptions};
pub use bg_remove::{
    apply_alpha_mask, remove_background, remove_background_with_backend, BgRemovalBackend,
    BgRemoveError, BgRemoveOptions,
};
pub use brand_template::{
    plan_brochure, BrandTemplateError, BrochurePage, BrochurePlan, BrochureSection, PageGeometry,
    DEFAULT_PAGE_HEIGHT, DEFAULT_PAGE_MARGIN, DEFAULT_PAGE_WIDTH, MAX_PAGES, MIN_PAGES,
};
pub use color_range::{pack_mask, select_by_color_range};
pub use denoise::{denoise, DenoiseError, DenoiseOptions};
pub use glyph_extract::{
    extract_glyph, ExtractedGlyph, GlyphCrop, GlyphExtractError, GlyphExtractOptions, GlyphMetrics,
};
pub use iconify::{iconify, IconPath, IconPoint, IconifyError, IconifyOptions, IconifyResult};
pub use inpaint::{inpaint, mask_from_rects, InpaintError, InpaintOptions, MaskRect};
pub use layout_suggest::{
    suggest_layout_grouping, Bounds as LayoutBounds, LayoutAlignment, LayoutNode,
    LayoutOrientation, LayoutSuggestError, LayoutSuggestOptions, LayoutSuggestion,
};
pub use diffusion_sidecar::{DiffusionSidecar, DiffusionSidecarConfig};
pub use llm_chat::{
    build_system_prompt, build_tool_call_system_prompt, chat_completion,
    chat_completion_with_token, parse_completion, request_tool_call, ChatContent, ChatError,
    ChatMessage, ChatRequest, ChatResponse, ChatResult, ChatRole, ContentPart,
};
pub use llm_sidecar::{LlmSidecar, SidecarConfig, SidecarError, SidecarResult, SidecarStatus};
pub use model_registry::{
    install_model_pack, is_installed, list_model_packs, mmproj_for, pack_path,
    recommended_generation_pack, recommended_llm_pack, recommended_vision_pack,
    uninstall_model_pack, InstallError, InstallReport, ModelKind, ModelPack, ModelPackCategory,
};
pub use ocr::{detect_text_regions, DetectTextRegionsOptions, OcrError, TextRegion};
pub use one_pager::{
    brief_to_one_pager, BriefToOnePagerError, BriefToOnePagerOptions, BriefToOnePagerResult,
    OnePagerPageSize, OnePagerSection, OnePagerSectionType,
};
pub use palette::{extract_palette, ExtractedColor};
pub use palette_harmonize::{
    harmonize_palette, HarmonyError, HarmonyResult, HarmonyRule, HarmonySuggestion,
};
pub use reformat::{
    reformat_to_deck, ReformatDeckError, ReformatDeckOptions, ReformatDeckResult, ReformatPage,
    ReformatPagePlacement,
};
pub use screenshot_to_layout::{
    analyze_screenshot_for_layout, Bounds as ScreenshotBounds, DetectedElement, ElementType,
};
pub use segment::{
    segment_image, segment_with_backend, SegmentBackend, SegmentError, SegmentMask, SegmentOptions,
    SegmentResult,
};
pub use smart_select::smart_select;
pub use stroke_match::{
    match_stroke_style, StrokeDeltaApplied, StrokeMatchError, StrokeMatchSummary, StrokeProperties,
};
pub use task_router::{
    build_accessibility_prompt, build_design_token_prompt, build_layer_naming_prompt, execute_task,
    parse_layer_naming_reply, AiError, AiResult, AiTask,
};
pub use tool_call::{
    default_design_registry, gbnf_for_registry, parse_tool_call_response, ToolCall,
    ToolCallParseError, ToolCallRegistry, ToolDescriptor, ToolParamType, ToolParameter,
    ToolRegistryError,
};
pub use trace::{trace_raster, TraceError, TraceOptions, TraceThreshold, TracedPath, TracedPoint};
pub use type_pairing::{
    suggest_type_pairing, TypePairingError, TypePairingResult, TypePairingSuggestion,
};
pub use upscale::{upscale_lanczos, upscale_with_backend, UpscaleBackend, UpscaleError};
