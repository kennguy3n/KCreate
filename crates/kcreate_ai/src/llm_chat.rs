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
///
/// `grammar` is the llama.cpp-specific GBNF extension. When set,
/// the server constrains the model's output to match the supplied
/// grammar; in KCreate this is how we ship guaranteed-valid
/// tool-call JSON (see [`crate::tool_call`]).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatRequest {
    pub messages: Vec<ChatMessage>,
    pub max_tokens: usize,
    pub temperature: f32,
    /// Optional GBNF grammar string. `None` => unconstrained
    /// completion (omitted from the JSON request); `Some(g)` =>
    /// llama.cpp constrains tokens to the grammar.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grammar: Option<String>,
}

impl ChatRequest {
    /// Build a request with sensible defaults (max 512 tokens, t=0.2
    /// for design-tooling-style outputs, no grammar).
    #[must_use]
    pub fn from_messages(messages: Vec<ChatMessage>) -> Self {
        Self {
            messages,
            max_tokens: 512,
            temperature: 0.2,
            grammar: None,
        }
    }

    /// Attach a GBNF grammar to the request. Builder-style so
    /// `ChatRequest::from_messages(...).with_grammar(g)` reads
    /// naturally at the call site.
    #[must_use]
    pub fn with_grammar(mut self, grammar: impl Into<String>) -> Self {
        self.grammar = Some(grammar.into());
        self
    }

    /// Lower the temperature so the output is more deterministic
    /// (handy for tool-call requests where we want stable JSON).
    #[must_use]
    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = temperature;
        self
    }

    /// Cap the response token budget.
    #[must_use]
    pub fn with_max_tokens(mut self, max_tokens: usize) -> Self {
        self.max_tokens = max_tokens;
        self
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

/// Build a system prompt that lists the tools the assistant may
/// invoke. Used in conjunction with [`request_tool_call`]; the
/// generated GBNF grammar guarantees the *shape* of the response,
/// while this prompt teaches the model the *semantics* of each tool.
#[must_use]
pub fn build_tool_call_system_prompt(
    document_summary: &str,
    registry: &crate::tool_call::ToolCallRegistry,
) -> ChatMessage {
    let mut lines = String::from(
        "You are KCreate's local design assistant. You run fully offline on the \
         user's machine. Respond with EXACTLY ONE JSON object selecting the most \
         appropriate tool from the list below. Do not include any text outside the \
         JSON object. The JSON must have the shape \
         {\"tool\":\"<name>\",\"arguments\":{...}}.\n\nAvailable tools:\n",
    );
    for t in registry.tools() {
        lines.push_str("- ");
        lines.push_str(&t.name);
        lines.push_str(": ");
        lines.push_str(&t.description);
        lines.push('\n');
        if !t.parameters.is_empty() {
            lines.push_str("  Parameters:\n");
            for p in &t.parameters {
                lines.push_str("    - ");
                lines.push_str(&p.name);
                lines.push_str(" (");
                lines.push_str(match p.kind {
                    crate::tool_call::ToolParamType::String => "string",
                    crate::tool_call::ToolParamType::Integer => "integer",
                    crate::tool_call::ToolParamType::Number => "number",
                    crate::tool_call::ToolParamType::Boolean => "boolean",
                    crate::tool_call::ToolParamType::Enum => "enum",
                });
                if p.required {
                    lines.push_str(", required");
                } else {
                    lines.push_str(", optional");
                }
                lines.push_str("): ");
                lines.push_str(&p.description);
                if p.kind == crate::tool_call::ToolParamType::Enum {
                    lines.push_str(" Allowed values: ");
                    for (i, v) in p.enum_values.iter().enumerate() {
                        if i > 0 {
                            lines.push_str(", ");
                        }
                        lines.push('"');
                        lines.push_str(v);
                        lines.push('"');
                    }
                    lines.push('.');
                }
                lines.push('\n');
            }
        }
    }
    lines.push_str("\nProject context:\n");
    lines.push_str(document_summary);
    lines.push('\n');
    ChatMessage::system(lines)
}

/// Drive a tool-call completion against the sidecar.
///
/// 1. Generates a GBNF grammar from `registry` and attaches it to
///    `request`.
/// 2. Sends the chat completion.
/// 3. Parses + validates the response against `registry`.
///
/// The caller is responsible for putting an instructive
/// `build_tool_call_system_prompt(...)` message at the front of
/// `request.messages` so the model knows which tools exist and what
/// each parameter means.
pub fn request_tool_call(
    port: u16,
    mut request: ChatRequest,
    registry: &crate::tool_call::ToolCallRegistry,
) -> ChatResult<crate::tool_call::ToolCall> {
    let grammar = crate::tool_call::gbnf_for_registry(registry);
    request.grammar = Some(grammar);
    let response = chat_completion(port, &request)?;
    let call = crate::tool_call::parse_tool_call_response(&response.content, registry)?;
    Ok(call)
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

    #[test]
    fn tool_call_system_prompt_lists_every_tool() {
        let registry = crate::tool_call::default_design_registry();
        let p = build_tool_call_system_prompt("artboards: Home", &registry);
        for t in registry.tools() {
            assert!(
                p.content.contains(&t.name),
                "system prompt should list tool {:?}; got:\n{}",
                t.name,
                p.content
            );
        }
        assert!(p.content.contains("artboards: Home"));
    }

    #[test]
    fn request_with_grammar_serialises_grammar_field() {
        let r = ChatRequest::from_messages(vec![ChatMessage::user("hi")])
            .with_grammar("root ::= \"{}\"\n")
            .with_temperature(0.0)
            .with_max_tokens(64);
        let s = serde_json::to_string(&r).expect("json");
        assert!(s.contains("\"grammar\":\"root ::= "));
        assert!(s.contains("\"max_tokens\":64"));
    }

    #[test]
    fn request_without_grammar_omits_grammar_field() {
        let r = ChatRequest::from_messages(vec![ChatMessage::user("hi")]);
        let s = serde_json::to_string(&r).expect("json");
        assert!(!s.contains("grammar"), "JSON should omit grammar: {s}");
    }

    /// End-to-end loopback test that drives `request_tool_call`
    /// against a mock llama-server. Verifies the grammar gets sent
    /// AND the response is parsed back into a typed `ToolCall`.
    #[cfg(feature = "llm_sidecar")]
    #[test]
    fn request_tool_call_round_trip_against_mock() {
        let server = tiny_http::Server::http("127.0.0.1:0").expect("mock server");
        let port = server.server_addr().to_ip().expect("ip").port();
        let registry = crate::tool_call::default_design_registry();
        let handle = std::thread::spawn(move || {
            let mut req = server.incoming_requests().next().expect("req");
            // Read the request body so we can assert the grammar
            // is being forwarded to the server.
            let mut body = String::new();
            std::io::Read::read_to_string(&mut req.as_reader(), &mut body).expect("read body");
            assert!(
                body.contains("\"grammar\""),
                "grammar missing from request: {body}"
            );
            assert!(
                body.contains("create_artboard"),
                "tool name missing from grammar: {body}"
            );
            let resp_body = serde_json::json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": "{\"tool\":\"create_artboard\",\"arguments\":{\"name\":\"Landing\",\"width\":1920,\"height\":1080}}",
                    },
                }],
                "usage": {"total_tokens": 42},
                "model": "mock",
            })
            .to_string();
            let resp = tiny_http::Response::from_string(resp_body).with_header(
                "content-type: application/json"
                    .parse::<tiny_http::Header>()
                    .expect("hdr"),
            );
            let _ = req.respond(resp);
        });

        let chat_req = ChatRequest::from_messages(vec![
            build_tool_call_system_prompt("", &registry),
            ChatMessage::user("create a 1920×1080 artboard named Landing"),
        ]);
        let call = request_tool_call(port, chat_req, &registry).expect("tool call");
        assert_eq!(call.tool, "create_artboard");
        assert_eq!(call.arg_str("name"), Some("Landing"));
        assert_eq!(call.arg_i64("width"), Some(1920));
        assert_eq!(call.arg_i64("height"), Some(1080));
        let _ = handle.join();
    }

    /// Verifies the chat path surfaces a typed parse error when the
    /// model emits a syntactically-valid but semantically-wrong
    /// response (right shape, missing required parameter).
    #[cfg(feature = "llm_sidecar")]
    #[test]
    fn request_tool_call_surfaces_missing_param_error() {
        let server = tiny_http::Server::http("127.0.0.1:0").expect("mock server");
        let port = server.server_addr().to_ip().expect("ip").port();
        let registry = crate::tool_call::default_design_registry();
        let handle = std::thread::spawn(move || {
            let req = server.incoming_requests().next().expect("req");
            let resp_body = serde_json::json!({
                "choices": [{"message": {"role": "assistant", "content":
                    "{\"tool\":\"create_artboard\",\"arguments\":{\"name\":\"A\",\"width\":100}}"}}],
                "model": "mock",
            })
            .to_string();
            let resp = tiny_http::Response::from_string(resp_body).with_header(
                "content-type: application/json"
                    .parse::<tiny_http::Header>()
                    .expect("hdr"),
            );
            let _ = req.respond(resp);
        });
        let chat_req = ChatRequest::from_messages(vec![ChatMessage::user("hi")]);
        let err = request_tool_call(port, chat_req, &registry).expect_err("missing");
        assert!(
            matches!(err, ChatError::Decode(ref m) if m.contains("missing required parameter")),
            "expected typed decode error, got {err:?}"
        );
        let _ = handle.join();
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
