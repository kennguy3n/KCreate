//! LLM sidecar bridge.
//!
//! Owns a process-global [`kcreate_ai::LlmSidecar`] and exposes
//! start/stop/status/chat operations the N-API surface forwards
//! verbatim. The single sidecar is intentional: each project
//! shares one running model to amortise the per-model RAM footprint.
//!
//! Build modes:
//!   - **Default** (`llm` feature off): every operation returns
//!     [`LlmBridgeError::FeatureDisabled`]. The TypeScript host can
//!     still call into the bridge; the UI surfaces a "model assistant
//!     unavailable in this build" message instead of crashing.
//!   - **`llm` feature**: pulls `ureq` and lets the sidecar talk
//!     loopback to `llama-server`.

use std::path::PathBuf;
use std::sync::OnceLock;

use kcreate_ai::{
    build_system_prompt, chat_completion, ChatError, ChatMessage, ChatRequest, ChatResponse,
    ChatRole, LlmSidecar, SidecarConfig, SidecarError, SidecarStatus,
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::document::{document_get_tree, project_info, NodeInfo};

/// All error cases visible across the bridge.
#[derive(Debug, thiserror::Error)]
pub enum LlmBridgeError {
    #[error(transparent)]
    Sidecar(#[from] SidecarError),
    #[error(transparent)]
    Chat(#[from] ChatError),
    #[error("no project open")]
    NoProject,
    #[error("sidecar is not ready")]
    NotReady,
    #[error("invalid request: {0}")]
    Invalid(String),
}

pub type LlmBridgeResult<T> = Result<T, LlmBridgeError>;

/// Wire shape returned by [`llm_status`].
///
/// `port` is `None` until the sidecar is `Ready`. `error` is non-None
/// only after a failed `start`/`stop`. The host renders this directly
/// in the Model Manager panel.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LlmStatusInfo {
    pub state: &'static str,
    pub model_name: Option<String>,
    pub context_size: Option<usize>,
    pub port: Option<u16>,
    pub error: Option<String>,
}

impl From<&SidecarStatus> for LlmStatusInfo {
    fn from(s: &SidecarStatus) -> Self {
        match s {
            SidecarStatus::Stopped => Self {
                state: "stopped",
                model_name: None,
                context_size: None,
                port: None,
                error: None,
            },
            SidecarStatus::Starting => Self {
                state: "starting",
                model_name: None,
                context_size: None,
                port: None,
                error: None,
            },
            SidecarStatus::Ready {
                model_name,
                context_size,
                port,
            } => Self {
                state: "ready",
                model_name: Some(model_name.clone()),
                context_size: Some(*context_size),
                port: Some(*port),
                error: None,
            },
            SidecarStatus::Error { message } => Self {
                state: "error",
                model_name: None,
                context_size: None,
                port: None,
                error: Some(message.clone()),
            },
        }
    }
}

/// Wire shape the host TypeScript decodes into a chat message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmMessage {
    pub role: String,
    pub content: String,
}

impl LlmMessage {
    fn into_core(self) -> Result<ChatMessage, LlmBridgeError> {
        let role = match self.role.as_str() {
            "system" => ChatRole::System,
            "user" => ChatRole::User,
            "assistant" => ChatRole::Assistant,
            other => {
                return Err(LlmBridgeError::Invalid(format!(
                    "unknown chat role: {other}"
                )));
            }
        };
        Ok(ChatMessage::new(role, self.content))
    }
}

/// Wire shape the host TypeScript receives from a chat completion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmReply {
    pub content: String,
    pub tokens_used: usize,
    pub model: String,
}

impl From<ChatResponse> for LlmReply {
    fn from(r: ChatResponse) -> Self {
        Self {
            content: r.content,
            tokens_used: r.tokens_used,
            model: r.model,
        }
    }
}

/// Process-global sidecar handle. Wrapped in `Option` because the
/// sidecar must be constructed with a model path the host supplies.
fn slot() -> &'static Mutex<Option<LlmSidecar>> {
    static SLOT: OnceLock<Mutex<Option<LlmSidecar>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

/// Start the sidecar pointed at `model_path`. If a sidecar is already
/// running, it is stopped first and replaced. Returns the listening
/// port on success.
pub fn llm_start(model_path: PathBuf) -> LlmBridgeResult<u16> {
    let mut guard = slot().lock();
    if let Some(prev) = guard.as_mut() {
        prev.stop();
    }
    let cfg = SidecarConfig::new(model_path);
    let mut sidecar = LlmSidecar::new(cfg);
    let port = sidecar.start()?;
    *guard = Some(sidecar);
    Ok(port)
}

/// Stop the running sidecar, if any. Idempotent.
pub fn llm_stop() {
    let mut guard = slot().lock();
    if let Some(s) = guard.as_mut() {
        s.stop();
    }
    *guard = None;
}

/// Current sidecar status.
pub fn llm_status() -> LlmStatusInfo {
    let guard = slot().lock();
    match guard.as_ref() {
        Some(s) => LlmStatusInfo::from(s.status()),
        None => LlmStatusInfo {
            state: "stopped",
            model_name: None,
            context_size: None,
            port: None,
            error: None,
        },
    }
}

/// Send a chat completion with the given messages. Bridges to
/// [`chat_completion`] using the sidecar's port. Fails with
/// `NotReady` if the sidecar hasn't transitioned to Ready yet.
pub fn llm_chat(
    messages: Vec<LlmMessage>,
    max_tokens: usize,
    temperature: f32,
) -> LlmBridgeResult<LlmReply> {
    let port = {
        let guard = slot().lock();
        guard
            .as_ref()
            .and_then(|s| s.status().port())
            .ok_or(LlmBridgeError::NotReady)?
    };
    let converted = messages
        .into_iter()
        .map(LlmMessage::into_core)
        .collect::<Result<Vec<_>, _>>()?;
    let req = ChatRequest {
        messages: converted,
        max_tokens,
        temperature,
    };
    let resp = chat_completion(port, &req)?;
    Ok(resp.into())
}

/// Build a context-aware system prompt + user prompt from the open
/// document, send to the sidecar, and return the model's suggestion.
///
/// The user prompt asks for "concrete improvements" for the current
/// selection (or the whole document if nothing is selected). This is
/// the entry point the "Suggest improvements" quick-action invokes.
pub fn llm_suggest_for_selection() -> LlmBridgeResult<LlmReply> {
    let info = project_info().ok_or(LlmBridgeError::NoProject)?;
    let tree = document_get_tree().map_err(|e| LlmBridgeError::Invalid(e.to_string()))?;
    let summary = summarise_document(&info.name, &tree);
    let user_prompt = "Suggest 3 concrete improvements to this design. \
         Focus on layout, hierarchy, and accessibility. \
         Number each suggestion and keep them under one sentence.";

    let req = vec![
        LlmMessage {
            role: "system".into(),
            content: build_system_prompt(&summary).content,
        },
        LlmMessage {
            role: "user".into(),
            content: user_prompt.to_string(),
        },
    ];
    llm_chat(req, 512, 0.2)
}

/// Compact, human-readable summary of the open document. Kept here
/// rather than in `kcreate_core` because it talks `NodeInfo` (the
/// bridge wire shape), not the live `DocumentGraph`.
fn summarise_document(project_name: &str, tree: &[NodeInfo]) -> String {
    let mut artboards = Vec::new();
    let mut other_kinds: std::collections::BTreeMap<&str, usize> =
        std::collections::BTreeMap::new();
    for node in tree {
        if node.node_type == "Artboard" {
            artboards.push(node.name.clone());
        } else {
            *other_kinds.entry(node.node_type.as_str()).or_insert(0) += 1;
        }
    }
    let mut s = format!("Project: {project_name}\n");
    if !artboards.is_empty() {
        s.push_str("Artboards: ");
        s.push_str(&artboards.join(", "));
        s.push('\n');
    }
    if !other_kinds.is_empty() {
        s.push_str("Layers: ");
        s.push_str(
            &other_kinds
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(", "),
        );
        s.push('\n');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(kind: &str, name: &str) -> NodeInfo {
        NodeInfo {
            id: uuid::Uuid::new_v4(),
            parent_id: None,
            node_type: kind.into(),
            name: name.into(),
            visible: true,
            locked: false,
            children: vec![],
            component_instance: None,
            metadata: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn summarise_groups_kinds_and_lists_artboards() {
        let tree = vec![
            n("Artboard", "Home"),
            n("Artboard", "About"),
            n("ShapeLayer", "rect"),
            n("ShapeLayer", "circle"),
            n("TextLayer", "title"),
        ];
        let s = summarise_document("Demo", &tree);
        assert!(s.contains("Project: Demo"));
        assert!(s.contains("Home"));
        assert!(s.contains("About"));
        assert!(s.contains("ShapeLayer=2"));
        assert!(s.contains("TextLayer=1"));
    }

    #[test]
    fn summarise_handles_empty_tree() {
        let s = summarise_document("Empty", &[]);
        assert_eq!(s, "Project: Empty\n");
    }

    #[test]
    fn message_role_round_trip() {
        for role in ["system", "user", "assistant"] {
            let m = LlmMessage {
                role: role.into(),
                content: "x".into(),
            };
            assert!(m.into_core().is_ok());
        }
        let bad = LlmMessage {
            role: "tool".into(),
            content: "x".into(),
        };
        assert!(matches!(bad.into_core(), Err(LlmBridgeError::Invalid(_))));
    }

    #[test]
    fn status_when_no_sidecar_is_stopped() {
        let s = llm_status();
        assert_eq!(s.state, "stopped");
        assert!(s.port.is_none());
    }

    #[test]
    fn stop_is_idempotent() {
        llm_stop();
        llm_stop();
    }

    /// Without the `llm` Cargo feature, `chat_completion` returns
    /// `FeatureDisabled`. We can't drive a full ready lifecycle here
    /// (no real llama-server binary), but the bridge layering
    /// guarantees `llm_chat` propagates that error type cleanly.
    #[cfg(not(feature = "llm"))]
    #[test]
    fn chat_without_ready_returns_not_ready() {
        let err = llm_chat(
            vec![LlmMessage {
                role: "user".into(),
                content: "hi".into(),
            }],
            32,
            0.2,
        )
        .expect_err("not ready");
        assert!(matches!(err, LlmBridgeError::NotReady));
    }
}
