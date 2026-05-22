//! OpenAI-compatible chat completion client for the LLM sidecar.
//!
//! Used by the bridge to talk to a running [`super::llm_sidecar`]
//! (which is just `llama-server` from the `kennguy3n/llama.cpp` fork)
//! over loopback. Non-streaming only in Phase 1 — the full response
//! is returned synchronously.
//!
//! The wire format is the subset of the OpenAI `/v1/chat/completions`
//! schema that `llama-server` actually implements: `messages`,
//! `max_tokens`, `temperature`, and the `choices[0].message.content`
//! response shape. Any future divergence is contained to this file.

use serde::{Deserialize, Serialize};
#[cfg(feature = "llm_sidecar")]
use std::time::Duration;

/// Role tag in a chat conversation. Lower-case JSON matches the
/// OpenAI/llama-server contract verbatim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChatRole {
    System,
    User,
    Assistant,
}

/// One message in a chat conversation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
}

impl ChatMessage {
    /// Convenience constructor: `system`/`user`/`assistant` shorthand.
    #[must_use]
    pub fn new(role: ChatRole, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
        }
    }

    #[must_use]
    pub fn system(content: impl Into<String>) -> Self {
        Self::new(ChatRole::System, content)
    }

    #[must_use]
    pub fn user(content: impl Into<String>) -> Self {
        Self::new(ChatRole::User, content)
    }

    #[must_use]
    pub fn assistant(content: impl Into<String>) -> Self {
        Self::new(ChatRole::Assistant, content)
    }
}

/// Chat completion request. The host is expected to pre-truncate
/// `messages` if the context window is tight; the sidecar will
/// reject oversize prompts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatRequest {
    pub messages: Vec<ChatMessage>,
    pub max_tokens: usize,
    pub temperature: f32,
}

impl ChatRequest {
    /// Build a request with sensible defaults (max 512 tokens, t=0.2
    /// for design-tooling-style outputs).
    #[must_use]
    pub fn from_messages(messages: Vec<ChatMessage>) -> Self {
        Self {
            messages,
            max_tokens: 512,
            temperature: 0.2,
        }
    }
}

/// Successful chat completion. `tokens_used` is the model's `usage`
/// stats; missing/zero is allowed because some llama.cpp builds omit
/// the block.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatResponse {
    pub content: String,
    pub tokens_used: usize,
    pub model: String,
}

/// Result alias for chat operations.
pub type ChatResult<T> = Result<T, ChatError>;

#[derive(Debug, thiserror::Error)]
pub enum ChatError {
    /// HTTP transport / status failure (network, 4xx/5xx, etc.).
    #[error("chat HTTP error: {0}")]
    Http(String),
    /// Sidecar returned a non-JSON / malformed body.
    #[error("chat JSON error: {0}")]
    Decode(String),
    /// Build was compiled without the `llm_sidecar` Cargo feature.
    /// This means the bridge will never reach the chat path in
    /// default builds — surfaced as a typed error rather than a
    /// `cfg`-shaped surprise so the host can disable the chat UI.
    #[error("chat feature disabled: rebuild with `--features llm_sidecar`")]
    FeatureDisabled,
}

/// Send a chat completion to a running sidecar on `127.0.0.1:port`.
///
/// `port` is taken from `LlmSidecar::status().port()`. Times out
/// after 60 s; the renderer should disable the input while in flight.
pub fn chat_completion(port: u16, request: &ChatRequest) -> ChatResult<ChatResponse> {
    chat_completion_impl(port, request)
}

#[cfg(feature = "llm_sidecar")]
fn chat_completion_impl(port: u16, request: &ChatRequest) -> ChatResult<ChatResponse> {
    let url = format!("http://127.0.0.1:{port}/v1/chat/completions");
    let body = serde_json::to_value(request).map_err(|e| ChatError::Decode(e.to_string()))?;
    let resp = ureq::post(&url)
        .timeout(Duration::from_mins(1))
        .set("content-type", "application/json")
        .send_json(body)
        .map_err(|e| ChatError::Http(e.to_string()))?;
    let raw: serde_json::Value = resp
        .into_json()
        .map_err(|e| ChatError::Decode(e.to_string()))?;
    parse_completion(&raw)
}

#[cfg(not(feature = "llm_sidecar"))]
fn chat_completion_impl(_port: u16, _request: &ChatRequest) -> ChatResult<ChatResponse> {
    Err(ChatError::FeatureDisabled)
}

/// Pure-function decoder: maps an OpenAI-style completion JSON to a
/// `ChatResponse`. Factored out so tests don't need a live HTTP
/// server — they can feed in raw JSON.
pub fn parse_completion(value: &serde_json::Value) -> ChatResult<ChatResponse> {
    let content = value
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|a| a.first())
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .ok_or_else(|| ChatError::Decode("missing choices[0].message.content".to_string()))?
        .to_string();
    let tokens_used = value
        .get("usage")
        .and_then(|u| u.get("total_tokens"))
        .and_then(serde_json::Value::as_u64)
        .map_or(0, |t| t as usize);
    let model = value
        .get("model")
        .and_then(|m| m.as_str())
        .unwrap_or("unknown")
        .to_string();
    Ok(ChatResponse {
        content,
        tokens_used,
        model,
    })
}

/// Build a system prompt that gives the assistant project context.
///
/// `document_summary` should be a short, plain-text human-readable
/// summary built by the bridge (artboard names, selected node info,
/// design tokens). Keeping prompt assembly here means the bridge
/// stays a thin marshalling layer.
#[must_use]
pub fn build_system_prompt(document_summary: &str) -> ChatMessage {
    ChatMessage::system(format!(
        "You are KCreate's local design assistant. You run fully offline on the user's machine.\n\
         Be concise, propose concrete edits, and avoid hallucinating capabilities you don't have.\n\
         \n\
         Project context:\n{document_summary}\n"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_typical_completion() {
        let raw = serde_json::json!({
            "choices": [{"message": {"role": "assistant", "content": "Hello"}}],
            "usage": {"total_tokens": 42},
            "model": "qwen-1.7b",
        });
        let r = parse_completion(&raw).expect("parse");
        assert_eq!(r.content, "Hello");
        assert_eq!(r.tokens_used, 42);
        assert_eq!(r.model, "qwen-1.7b");
    }

    #[test]
    fn parse_completion_missing_usage_is_zero() {
        let raw = serde_json::json!({
            "choices": [{"message": {"content": "OK"}}],
            "model": "x",
        });
        let r = parse_completion(&raw).expect("parse");
        assert_eq!(r.tokens_used, 0);
    }

    #[test]
    fn parse_completion_missing_content_errors() {
        let raw = serde_json::json!({"choices": []});
        let err = parse_completion(&raw).expect_err("err");
        assert!(matches!(err, ChatError::Decode(_)));
    }

    #[test]
    fn request_defaults_are_sane() {
        let r = ChatRequest::from_messages(vec![ChatMessage::user("hi")]);
        assert_eq!(r.max_tokens, 512);
        assert!((r.temperature - 0.2).abs() < 1e-6);
        assert_eq!(r.messages.len(), 1);
    }

    #[test]
    fn role_serialises_lowercase() {
        let m = ChatMessage::user("hi");
        let s = serde_json::to_string(&m).expect("json");
        assert!(s.contains("\"user\""));
    }

    #[test]
    fn system_prompt_includes_summary() {
        let p = build_system_prompt("artboards: Home, About");
        assert!(p.content.contains("artboards: Home, About"));
    }

    #[cfg(not(feature = "llm_sidecar"))]
    #[test]
    fn chat_completion_without_feature_returns_feature_disabled() {
        let req = ChatRequest::from_messages(vec![ChatMessage::user("hi")]);
        let err = chat_completion(0, &req).expect_err("disabled");
        assert!(matches!(err, ChatError::FeatureDisabled));
    }

    /// End-to-end loopback test with a real HTTP server. Only
    /// enabled when the `llm_sidecar` feature is on; that's the
    /// only case the chat path is wired to `ureq`.
    #[cfg(feature = "llm_sidecar")]
    #[test]
    fn chat_completion_round_trip_against_mock() {
        let server = tiny_http::Server::http("127.0.0.1:0").expect("mock server");
        let port = server.server_addr().to_ip().expect("ip").port();
        let handle = std::thread::spawn(move || {
            let req = server.incoming_requests().next().expect("req");
            let body = serde_json::json!({
                "choices": [{"message": {"role": "assistant", "content": "Pong"}}],
                "usage": {"total_tokens": 7},
                "model": "mock",
            })
            .to_string();
            let resp = tiny_http::Response::from_string(body).with_header(
                "content-type: application/json"
                    .parse::<tiny_http::Header>()
                    .expect("hdr"),
            );
            let _ = req.respond(resp);
        });

        let req = ChatRequest::from_messages(vec![ChatMessage::user("ping")]);
        let resp = chat_completion(port, &req).expect("chat");
        assert_eq!(resp.content, "Pong");
        assert_eq!(resp.tokens_used, 7);
        assert_eq!(resp.model, "mock");
        let _ = handle.join();
    }
}
