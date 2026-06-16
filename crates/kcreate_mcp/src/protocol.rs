//! Minimal JSON-RPC 2.0 envelope.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// JSON-RPC error codes.
///
/// The `-32700..=-32600` block is the JSON-RPC 2.0 standard set. The
/// `-32000..=-32099` block is reserved by the spec for
/// implementation-defined server errors; we use it for the MCP
/// permission gate so a client can distinguish "the user has not
/// authorised this yet" (retryable after a prompt) from "this tool
/// does not exist" (never retryable).
pub mod codes {
    pub const PARSE_ERROR: i32 = -32700;
    pub const INVALID_REQUEST: i32 = -32600;
    pub const METHOD_NOT_FOUND: i32 = -32601;
    pub const INVALID_PARAMS: i32 = -32602;
    pub const INTERNAL_ERROR: i32 = -32603;

    /// The user has explicitly denied this `(client, tool)` pair (or a
    /// one-shot grant was already spent). Not retryable without the
    /// user changing the decision in the settings UI.
    pub const PERMISSION_DENIED: i32 = -32001;
    /// No decision is on record for this `(client, tool)` pair yet. The
    /// server has enqueued a pending prompt; the client may retry once
    /// the user grants access.
    pub const PERMISSION_REQUIRED: i32 = -32002;
    /// The master MCP automation switch is off. All tool calls are
    /// refused until the user re-enables automation.
    pub const MASTER_DISABLED: i32 = -32003;
}

/// A JSON-RPC 2.0 request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

/// A JSON-RPC 2.0 success or error response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

impl JsonRpcResponse {
    /// Build a success envelope.
    #[must_use]
    pub fn success(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: Some(result),
            error: None,
        }
    }
    /// Build an error envelope.
    #[must_use]
    pub fn err(id: Option<Value>, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }
}

/// JSON-RPC 2.0 error object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}
