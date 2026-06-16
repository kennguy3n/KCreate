//! `kcreate_mcp` — local Model-Context-Protocol server.
//!
//! A **loopback-only** HTTP JSON-RPC server that lets an external AI
//! agent compose and export a real KCreate design. It speaks both the
//! MCP-standard handshake (`initialize` / `tools/list` / `tools/call`)
//! and back-compatible direct method names, and exposes a rich tool
//! surface (templates, themed generation, assets, fill / text edits,
//! theming, magic-resize, export) — see [`tools`].
//!
//! Every tool call is governed by [`permissions::McpPermissionStore`]
//! (Once / Always / Denied, JSON on-disk) plus a master enable switch;
//! the server consults the SAME store the settings UI edits and
//! enqueues a [`permissions::PendingPermissionRequest`] when a client
//! has no decision on record. Tool identity is taken from the
//! [`server::CLIENT_HEADER`] HTTP header.
//!
//! The server **binds to `127.0.0.1` only** (configurable port; 0
//! = OS-assigned). It is disabled by default; the bridge starts it
//! on demand via `mcp_start()`.
//!
//! Although MCP is "networking" in the strict sense, the loopback
//! transport is host-local — no traffic leaves the machine. The
//! local-first deny-list deliberately excludes this crate (and only
//! this crate) and the test enforces that fact.

#![forbid(unsafe_op_in_unsafe_fn)]

pub mod permissions;
pub mod protocol;
pub mod server;
pub mod tools;

pub use permissions::{
    McpPermission, McpPermissionStore, PendingPermissionRequest, PendingPermissions,
    PermissionDecision, PermissionGrant, PermissionStoreError,
};
pub use protocol::{JsonRpcError, JsonRpcRequest, JsonRpcResponse};
pub use server::{McpError, McpServer, PermissionGate, ANONYMOUS_CLIENT, CLIENT_HEADER};
pub use tools::{
    dispatch_tool, handle_create_node, handle_export_artboard, handle_list_artboards, is_tool,
    tool_specs, DocumentAccess, ToolSpec,
};
