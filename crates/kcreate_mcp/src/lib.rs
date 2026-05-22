//! `kcreate_mcp` — local Model-Context-Protocol server.
//!
//! Phase 0 ships a minimal **loopback-only** HTTP JSON-RPC server
//! exposing three tools:
//!
//! 1. `list_artboards` — enumerate artboards (id + name + bounds)
//! 2. `create_node` — create a node under a parent
//! 3. `export_artboard` — export a node to a PNG/SVG file
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

pub use permissions::{McpPermission, McpPermissionStore, PermissionGrant, PermissionStoreError};
pub use protocol::{JsonRpcError, JsonRpcRequest, JsonRpcResponse};
pub use server::{McpError, McpServer};
pub use tools::{
    handle_create_node, handle_export_artboard, handle_list_artboards, DocumentAccess,
};
