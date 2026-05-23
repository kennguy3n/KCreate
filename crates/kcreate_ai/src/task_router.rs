//! Task router: a single entry point that the bridge calls
//! regardless of which model / algorithm backs the task.
//!
//! [`AiTask::BackgroundRemoval`] is executed synchronously on the
//! caller's thread using the threshold backend (or ONNX when
//! configured via the bridge — see `kcreate_bridge::llm`).
//!
//! [`AiTask::LayerNaming`], [`AiTask::DesignTokenExtraction`], and
//! [`AiTask::AccessibilityCheck`] are *LLM tasks*: this module
//! provides [`build_layer_naming_prompt`], [`build_design_token_prompt`],
//! and [`build_accessibility_prompt`] helpers that produce
//! [`ChatRequest`]s. The actual dispatch to the running sidecar lives
//! in `kcreate_bridge::llm` because that's where the sidecar
//! singleton is owned.

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::alt_text::{generate_alt_text, AltTextError, AltTextOptions, AltTextReport};
use crate::bg_remove::{remove_background, BgRemoveError, BgRemoveOptions};
use crate::layout_suggest::{
    suggest_layout_grouping, LayoutNode, LayoutSuggestError, LayoutSuggestOptions, LayoutSuggestion,
};
use crate::llm_chat::{ChatMessage, ChatRequest, ChatRole};

/// Errors from [`execute_task`].
#[derive(Debug, Error)]
pub enum AiError {
    #[error(transparent)]
    BgRemove(#[from] BgRemoveError),
    #[error(transparent)]
    AltText(#[from] AltTextError),
    #[error(transparent)]
    LayoutSuggest(#[from] LayoutSuggestError),
    #[error("task `{0}` requires the LLM sidecar; route via kcreate_bridge::llm")]
    LlmRequired(&'static str),
    #[error("unsupported task: {0}")]
    Unsupported(String),
}

/// One AI task. Discriminated externally for cheap JSON crossings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AiTask {
    BackgroundRemoval {
        image_data: Vec<u8>,
        width: u32,
        height: u32,
        #[serde(default)]
        tolerance: Option<u8>,
        #[serde(default)]
        feather: Option<u8>,
    },
    /// Suggest cleaner names for the given (id, current-name) pairs.
    /// Routed to the LLM sidecar.
    LayerNaming { node_names: Vec<(Uuid, String)> },
    /// Extract design tokens (colors, fonts, spacing) from the
    /// serialised document. Routed to the LLM sidecar.
    DesignTokenExtraction { document_json: String },
    /// Check the document for contrast issues, undersized tap targets,
    /// missing alt text, and similar accessibility problems. Routed
    /// to the LLM sidecar.
    AccessibilityCheck { document_json: String },
    /// Generate factual alt-text for a raster layer. Pure-algorithm
    /// — runs entirely on the calling thread, no model dependency.
    AltTextGeneration {
        image_data: Vec<u8>,
        width: u32,
        height: u32,
    },
    /// Cluster the supplied nodes into proposed groups by proximity
    /// + alignment. Pure-algorithm.
    LayoutSuggestion { nodes: Vec<LayoutNode> },
}

/// One AI result. Discriminated externally so the bridge can decide
/// how to write the output back into the document.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AiResult {
    BackgroundRemoval {
        /// Single-channel mask (255 = subject, 0 = background). Same
        /// dimensions as the input.
        mask: Vec<u8>,
        /// New RGBA8 buffer with alpha modulated by the mask.
        output_rgba: Vec<u8>,
        width: u32,
        height: u32,
    },
    LayerNaming {
        /// Original (id, suggested name) pairs in input order. The
        /// bridge applies them with `node_rename` to record an op.
        suggestions: Vec<(Uuid, String)>,
    },
    DesignTokenExtraction {
        /// JSON blob produced by the LLM in the schema described by
        /// [`build_design_token_prompt`].
        tokens_json: String,
    },
    AccessibilityCheck {
        /// JSON blob produced by the LLM in the schema described by
        /// [`build_accessibility_prompt`].
        issues_json: String,
    },
    AltTextGeneration {
        report: AltTextReport,
    },
    LayoutSuggestion {
        suggestions: Vec<LayoutSuggestion>,
    },
}

/// Execute one task synchronously on the calling thread.
///
/// Tasks that require the LLM sidecar (`LayerNaming`,
/// `DesignTokenExtraction`, `AccessibilityCheck`) return
/// [`AiError::LlmRequired`] — they must be dispatched through
/// `kcreate_bridge::llm` so the sidecar singleton is reused.
pub fn execute_task(task: AiTask) -> Result<AiResult, AiError> {
    match task {
        AiTask::BackgroundRemoval {
            image_data,
            width,
            height,
            tolerance,
            feather,
        } => {
            let mut opts = BgRemoveOptions::default();
            if let Some(t) = tolerance {
                opts.tolerance = t;
            }
            if let Some(f) = feather {
                opts.feather = f;
            }
            let out = remove_background(&image_data, width, height, opts)?;
            let mask: Vec<u8> = out.chunks_exact(4).map(|p| p[3]).collect();
            Ok(AiResult::BackgroundRemoval {
                mask,
                output_rgba: out,
                width,
                height,
            })
        }
        AiTask::LayerNaming { .. } => Err(AiError::LlmRequired("layer_naming")),
        AiTask::DesignTokenExtraction { .. } => {
            Err(AiError::LlmRequired("design_token_extraction"))
        }
        AiTask::AccessibilityCheck { .. } => Err(AiError::LlmRequired("accessibility_check")),
        AiTask::AltTextGeneration {
            image_data,
            width,
            height,
        } => {
            let report = generate_alt_text(&image_data, width, height, AltTextOptions::default())?;
            Ok(AiResult::AltTextGeneration { report })
        }
        AiTask::LayoutSuggestion { nodes } => {
            let suggestions = suggest_layout_grouping(&nodes, LayoutSuggestOptions::default())?;
            Ok(AiResult::LayoutSuggestion { suggestions })
        }
    }
}

/// Build the [`ChatRequest`] that asks the LLM to suggest better
/// layer names. The model must return a JSON object of shape
/// `{ "names": { "<uuid>": "<new-name>", ... } }` — the bridge
/// parses this and applies the renames.
#[must_use]
pub fn build_layer_naming_prompt(node_names: &[(Uuid, String)]) -> ChatRequest {
    let mut user_body = String::from(
        "Here are the current layer names. Suggest concise, semantic \
         names (max 24 chars, kebab- or PascalCase) that describe the \
         layer's role. Return JSON only, no prose.\n\nLayers:\n",
    );
    for (id, name) in node_names {
        // `write!` to an owned `String` cannot fail (no I/O), but we
        // route through `Write` to avoid the heap allocation that
        // `format!` would do for each layer.
        let _ = writeln!(user_body, "- {id}: \"{name}\"");
    }
    user_body.push_str(
        "\nReturn this exact shape:\n\
         {\"names\":{\"<uuid>\":\"<new-name>\", ...}}\n",
    );
    ChatRequest {
        messages: vec![
            ChatMessage {
                role: ChatRole::System,
                content: "You are a UI design assistant that renames layers \
                          to be semantic and concise. Output JSON only."
                    .to_string(),
            },
            ChatMessage {
                role: ChatRole::User,
                content: user_body,
            },
        ],
        max_tokens: 512,
        temperature: 0.2,
        grammar: None,
    }
}

/// Build the [`ChatRequest`] that asks the LLM to extract design
/// tokens (colors, fonts, spacing scale) from a serialised
/// document. The model must return JSON of shape
/// `{ "colors": [...], "fonts": [...], "spacing": [...] }`.
#[must_use]
pub fn build_design_token_prompt(document_json: &str) -> ChatRequest {
    let user_body = format!(
        "Extract reusable design tokens from this document. Output \
         JSON only, this exact shape:\n\
         {{\"colors\":[{{\"name\":\"...\",\"hex\":\"#rrggbb\"}}],\
         \"fonts\":[{{\"family\":\"...\",\"weight\":400}}],\
         \"spacing\":[{{\"name\":\"...\",\"px\":8}}]}}\n\nDocument:\n{document_json}",
    );
    ChatRequest {
        messages: vec![
            ChatMessage {
                role: ChatRole::System,
                content: "You are a design-system extraction assistant. \
                          Identify recurring colors, fonts, and spacing \
                          values. Output JSON only."
                    .to_string(),
            },
            ChatMessage {
                role: ChatRole::User,
                content: user_body,
            },
        ],
        max_tokens: 1024,
        temperature: 0.1,
        grammar: None,
    }
}

/// Build the [`ChatRequest`] that asks the LLM to find accessibility
/// issues. The model must return JSON of shape
/// `{ "issues": [{ "node_id": "<uuid>", "severity": "...", "message": "..." }] }`.
#[must_use]
pub fn build_accessibility_prompt(document_json: &str) -> ChatRequest {
    let user_body = format!(
        "Audit this document for accessibility problems: low contrast, \
         tap targets under 44x44, missing alt text, font sizes under \
         12pt for body, etc. Output JSON only, this exact shape:\n\
         {{\"issues\":[{{\"node_id\":\"<uuid|null>\",\"severity\":\"info|warn|error\",\"message\":\"...\"}}]}}\n\nDocument:\n{document_json}",
    );
    ChatRequest {
        messages: vec![
            ChatMessage {
                role: ChatRole::System,
                content: "You are an accessibility auditor. Flag WCAG AA \
                          contrast failures, undersized tap targets, \
                          missing alt text. Output JSON only."
                    .to_string(),
            },
            ChatMessage {
                role: ChatRole::User,
                content: user_body,
            },
        ],
        max_tokens: 1024,
        temperature: 0.1,
        grammar: None,
    }
}

/// Try to parse the LLM's `{"names":{"<uuid>":"<name>"}}` reply into
/// a flat suggestions vector keyed by [`Uuid`]. Returns an empty
/// vector if parsing fails or the schema doesn't match — callers
/// surface that gracefully (the model can always be retried).
#[must_use]
pub fn parse_layer_naming_reply(content: &str) -> Vec<(Uuid, String)> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(content) else {
        return Vec::new();
    };
    let Some(names) = value.get("names").and_then(serde_json::Value::as_object) else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(names.len());
    for (k, v) in names {
        if let (Ok(id), Some(name)) = (Uuid::parse_str(k), v.as_str()) {
            out.push((id, name.to_string()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execute_bg_removal() {
        let mut img = vec![240u8; 16 * 16 * 4];
        for px in img.chunks_exact_mut(4) {
            px[3] = 0xFF;
        }
        // Make a non-background pixel in the centre.
        let centre = (8 * 16 + 8) * 4;
        img[centre] = 10;
        img[centre + 1] = 10;
        img[centre + 2] = 10;
        let result = execute_task(AiTask::BackgroundRemoval {
            image_data: img,
            width: 16,
            height: 16,
            tolerance: Some(20),
            feather: Some(8),
        })
        .expect("ok");
        let AiResult::BackgroundRemoval {
            mask, output_rgba, ..
        } = result
        else {
            panic!("expected BackgroundRemoval result");
        };
        assert_eq!(mask.len(), 16 * 16);
        assert_eq!(output_rgba.len(), 16 * 16 * 4);
        assert_eq!(output_rgba[3], 0);
    }

    #[test]
    fn llm_tasks_route_via_bridge_not_local_executor() {
        let id = Uuid::new_v4();
        let err = execute_task(AiTask::LayerNaming {
            node_names: vec![(id, "Group 1".to_string())],
        })
        .expect_err("must require sidecar");
        assert!(matches!(err, AiError::LlmRequired("layer_naming")));

        let err = execute_task(AiTask::DesignTokenExtraction {
            document_json: "{}".to_string(),
        })
        .expect_err("must require sidecar");
        assert!(matches!(
            err,
            AiError::LlmRequired("design_token_extraction")
        ));

        let err = execute_task(AiTask::AccessibilityCheck {
            document_json: "{}".to_string(),
        })
        .expect_err("must require sidecar");
        assert!(matches!(err, AiError::LlmRequired("accessibility_check")));
    }

    #[test]
    fn layer_naming_prompt_includes_ids_and_names() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let req = build_layer_naming_prompt(&[
            (a, "Group 1".to_string()),
            (b, "Rectangle 5".to_string()),
        ]);
        assert_eq!(req.messages.len(), 2);
        assert!(req.messages[1].content.contains(&a.to_string()));
        assert!(req.messages[1].content.contains(&b.to_string()));
        assert!(req.messages[1].content.contains("Group 1"));
        assert!(req.messages[1].content.contains("Rectangle 5"));
        assert!(req.temperature <= 0.5);
    }

    #[test]
    fn design_token_prompt_embeds_document_and_schema() {
        let req = build_design_token_prompt("{\"sample\":true}");
        let user = &req.messages[1].content;
        assert!(user.contains("colors"));
        assert!(user.contains("fonts"));
        assert!(user.contains("spacing"));
        assert!(user.contains("{\"sample\":true}"));
    }

    #[test]
    fn accessibility_prompt_lists_audit_dimensions() {
        let req = build_accessibility_prompt("{}");
        let user = &req.messages[1].content;
        assert!(user.contains("contrast"));
        assert!(user.contains("tap target"));
        assert!(user.contains("alt text"));
    }

    #[test]
    fn parse_layer_naming_reply_happy_path() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let json = format!("{{\"names\":{{\"{a}\":\"primary-button\",\"{b}\":\"hero-image\"}}}}");
        let mut got = parse_layer_naming_reply(&json);
        got.sort_by_key(|(id, _)| *id);
        let mut expected = vec![
            (a, "primary-button".to_string()),
            (b, "hero-image".to_string()),
        ];
        expected.sort_by_key(|(id, _)| *id);
        assert_eq!(got, expected);
    }

    #[test]
    fn parse_layer_naming_reply_garbage_returns_empty() {
        assert!(parse_layer_naming_reply("not json").is_empty());
        assert!(parse_layer_naming_reply("{}").is_empty());
        assert!(parse_layer_naming_reply("{\"wrong\":\"shape\"}").is_empty());
    }

    #[test]
    fn execute_alt_text_generation_routes_through_local_executor() {
        let mut img = Vec::with_capacity(8 * 8 * 4);
        for _ in 0..(8 * 8) {
            img.extend_from_slice(&[60, 0, 0, 255]);
        }
        let result = execute_task(AiTask::AltTextGeneration {
            image_data: img,
            width: 8,
            height: 8,
        })
        .expect("ok");
        let AiResult::AltTextGeneration { report } = result else {
            panic!("expected AltTextGeneration result");
        };
        assert!(report.text.starts_with("Dark"));
        assert!(report.text.contains("reds and pinks"));
    }

    #[test]
    fn execute_layout_suggestion_routes_through_local_executor() {
        let nodes = vec![
            LayoutNode {
                id: Uuid::new_v4(),
                bounds: crate::layout_suggest::Bounds {
                    x: 0.0,
                    y: 0.0,
                    width: 40.0,
                    height: 40.0,
                },
            },
            LayoutNode {
                id: Uuid::new_v4(),
                bounds: crate::layout_suggest::Bounds {
                    x: 60.0,
                    y: 0.0,
                    width: 40.0,
                    height: 40.0,
                },
            },
        ];
        let result = execute_task(AiTask::LayoutSuggestion { nodes }).expect("ok");
        let AiResult::LayoutSuggestion { suggestions } = result else {
            panic!("expected LayoutSuggestion result");
        };
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].member_ids.len(), 2);
    }
}
