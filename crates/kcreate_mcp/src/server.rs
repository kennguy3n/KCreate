//! HTTP JSON-RPC server. Binds to `127.0.0.1` only and runs the
//! request loop on a single background thread; the bridge talks to
//! this via `mcp_start` / `mcp_stop` / `mcp_is_running`.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use log::warn;
use parking_lot::Mutex;
use serde_json::Value;
use thiserror::Error;
use tiny_http::{Method, Response, Server};

use crate::protocol::{codes, JsonRpcRequest, JsonRpcResponse};
use crate::tools::{
    handle_create_node, handle_export_artboard, handle_list_artboards, DocumentAccess,
};

/// Errors from [`McpServer`].
#[derive(Debug, Error)]
pub enum McpError {
    #[error("server already running")]
    AlreadyRunning,
    #[error("bind failed: {0}")]
    Bind(String),
    #[error("not running")]
    NotRunning,
}

/// The local MCP server.
pub struct McpServer {
    port: u16,
    stop_flag: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for McpServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpServer")
            .field("port", &self.port)
            .field("running", &!self.stop_flag.load(Ordering::Relaxed))
            .finish()
    }
}

impl McpServer {
    /// Start the server on a loopback port. Pass `0` to let the OS
    /// choose one; the bound port is exposed via [`Self::port`].
    pub fn start(port: u16, access: Arc<dyn DocumentAccess>) -> Result<Self, McpError> {
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        let server = Server::http(addr).map_err(|e| McpError::Bind(e.to_string()))?;
        let bound_port = server.server_addr().to_ip().map_or(port, |sa| sa.port());

        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_flag_thread = Arc::clone(&stop_flag);
        let access = Arc::clone(&access);
        let server = Arc::new(server);
        let server_thread = Arc::clone(&server);

        let handle = std::thread::Builder::new()
            .name("kcreate-mcp".into())
            .spawn(move || run_loop(server_thread, stop_flag_thread, access))
            .map_err(|e| McpError::Bind(e.to_string()))?;

        Ok(Self {
            port: bound_port,
            stop_flag,
            thread: Some(handle),
        })
    }

    /// Bound loopback port.
    #[must_use]
    pub const fn port(&self) -> u16 {
        self.port
    }

    /// Is the worker thread still running?
    #[must_use]
    pub fn is_running(&self) -> bool {
        !self.stop_flag.load(Ordering::Relaxed)
    }

    /// Signal the worker thread to exit. Idempotent.
    pub fn stop(&mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        if let Some(handle) = self.thread.take() {
            // The server stops accepting requests when its `Drop`
            // fires; we already signalled via the flag. We give the
            // thread a short window to exit cleanly.
            let _ = handle.join();
        }
    }
}

impl Drop for McpServer {
    fn drop(&mut self) {
        if !self.stop_flag.load(Ordering::Relaxed) {
            self.stop();
        }
    }
}

fn run_loop(server: Arc<Server>, stop_flag: Arc<AtomicBool>, access: Arc<dyn DocumentAccess>) {
    while !stop_flag.load(Ordering::Relaxed) {
        // Poll with a short timeout so the stop flag is observed.
        let req_opt = match server.recv_timeout(Duration::from_millis(100)) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let Some(mut req) = req_opt else { continue };

        if *req.method() != Method::Post {
            let body = serde_json::to_string(&JsonRpcResponse::err(
                None,
                codes::INVALID_REQUEST,
                "POST required",
            ))
            .unwrap_or_default();
            let _ = req.respond(Response::from_string(body).with_status_code(405));
            continue;
        }

        let mut buf = String::new();
        if req.as_reader().read_to_string(&mut buf).is_err() {
            warn!("kcreate_mcp: failed to read request body");
            continue;
        }
        let response_body = handle_payload(&buf, &*access);
        let resp = Response::from_string(response_body).with_header(
            "Content-Type: application/json"
                .parse::<tiny_http::Header>()
                .expect("static header"),
        );
        let _ = req.respond(resp);
    }
}

fn handle_payload(body: &str, access: &dyn DocumentAccess) -> String {
    let parsed: Result<JsonRpcRequest, _> = serde_json::from_str(body);
    let envelope = match parsed {
        Ok(req) => dispatch(req, access),
        Err(e) => JsonRpcResponse::err(None, codes::PARSE_ERROR, format!("parse error: {e}")),
    };
    serde_json::to_string(&envelope).unwrap_or_else(|_| {
        // Last-ditch fallback. Should be impossible because
        // JsonRpcResponse is owned + Serialize.
        r#"{"jsonrpc":"2.0","error":{"code":-32603,"message":"serialise failed"}}"#.into()
    })
}

fn dispatch(req: JsonRpcRequest, access: &dyn DocumentAccess) -> JsonRpcResponse {
    let id = req.id.clone();
    let result: Result<Value, (i32, String)> = match req.method.as_str() {
        "list_artboards" => handle_list_artboards(access),
        "create_node" => handle_create_node(access, req.params),
        "export_artboard" => handle_export_artboard(access, req.params),
        other => Err((codes::METHOD_NOT_FOUND, format!("unknown method: {other}"))),
    };
    match result {
        Ok(v) => JsonRpcResponse::success(id, v),
        Err((code, msg)) => JsonRpcResponse::err(id, code, msg),
    }
}

/// Process-global MCP server singleton. The bridge stores a started
/// server here so subsequent calls to `mcp_start` can detect "already
/// running" and `mcp_stop` can find it.
#[must_use]
pub fn global() -> &'static Mutex<Option<McpServer>> {
    use std::sync::OnceLock;
    static SLOT: OnceLock<Mutex<Option<McpServer>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

/// Start the process-global MCP server on a loopback port chosen by
/// the OS. If a server is already in the global slot and still
/// running, returns its bound port unchanged (idempotent). Otherwise
/// boots a fresh `McpServer` with the supplied `access`, stores it,
/// and returns the bound port.
///
/// The bridge calls this from `mcp_start()`; tests and other in-tree
/// consumers can do the same. The access object is only consulted on
/// a cold start — once a server is running, subsequent calls are
/// no-ops and the existing access continues to back tool handlers.
pub fn start_global(access: Arc<dyn DocumentAccess>) -> Result<u16, McpError> {
    let mut slot = global().lock();
    if let Some(existing) = slot.as_ref() {
        if existing.is_running() {
            return Ok(existing.port());
        }
        // Stale server in the slot (worker thread already exited).
        // Drop it and start fresh.
        slot.take();
    }
    let server = McpServer::start(0, access)?;
    let port = server.port();
    *slot = Some(server);
    Ok(port)
}

/// Stop the process-global MCP server if one is running. Idempotent.
pub fn stop_global() {
    let mut slot = global().lock();
    if let Some(mut server) = slot.take() {
        server.stop();
    }
}

/// Returns whether the process-global MCP server is currently
/// running.
#[must_use]
pub fn is_running() -> bool {
    let slot = global().lock();
    slot.as_ref().is_some_and(McpServer::is_running)
}

/// Returns the loopback TCP port of the process-global MCP server,
/// or `None` when it is not running. The McpSettingsPanel UI uses
/// this so the user can see what port to point external MCP clients
/// at without restarting the server.
#[must_use]
pub fn port() -> Option<u16> {
    let slot = global().lock();
    slot.as_ref()
        .filter(|s| s.is_running())
        .map(McpServer::port)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{ArtboardInfo, DocumentAccess};
    use kcreate_core::document::DocumentGraph;
    use kcreate_core::node::{Node, NodeType};
    use kcreate_export::svg::SvgExportOptions;
    use parking_lot::Mutex as PMutex;
    use uuid::Uuid;

    struct DocStub(PMutex<DocumentGraph>);
    impl DocumentAccess for DocStub {
        fn list_artboards(&self) -> Vec<ArtboardInfo> {
            self.0
                .lock()
                .iter()
                .filter(|(_, n)| n.node_type == NodeType::Artboard)
                .map(|(id, n)| ArtboardInfo {
                    id: id.to_string(),
                    name: n.name.clone(),
                    bounds: n.bounds.into(),
                })
                .collect()
        }
        fn create_node(
            &self,
            node_type: NodeType,
            name: String,
            parent_id: Option<Uuid>,
        ) -> Result<Uuid, String> {
            let mut node = Node::new(node_type, name);
            node.parent_id = parent_id;
            self.0.lock().insert_node(node).map_err(|e| e.to_string())
        }
        fn export_svg(&self, node_ids: &[Uuid]) -> Result<String, String> {
            kcreate_export::svg::export_svg_from_document(
                &self.0.lock(),
                node_ids,
                &SvgExportOptions::default(),
            )
            .map_err(|e| e.to_string())
        }
    }

    #[test]
    fn start_stop_roundtrip() {
        let access = Arc::new(DocStub(PMutex::new(DocumentGraph::new())));
        let mut server = McpServer::start(0, access).expect("start");
        assert!(server.port() > 0);
        assert!(server.is_running());
        server.stop();
        // After stop the flag is set; we sleep briefly to let the
        // thread observe the flag (the join inside stop() already
        // does this, so the assertion is mostly defensive).
        std::thread::sleep(Duration::from_millis(50));
        assert!(!server.is_running());
    }

    #[test]
    fn unknown_method_returns_method_not_found() {
        let access = Arc::new(DocStub(PMutex::new(DocumentGraph::new())));
        let resp = handle_payload(
            r#"{"jsonrpc":"2.0","id":1,"method":"definitely_not_real"}"#,
            &*access,
        );
        let v: Value = serde_json::from_str(&resp).expect("json");
        assert_eq!(v["error"]["code"], codes::METHOD_NOT_FOUND);
    }

    #[test]
    fn global_start_stop_roundtrip() {
        // The global slot persists across tests within the same binary;
        // start fresh by stopping any leftover instance first.
        stop_global();
        assert!(!is_running());
        let access = Arc::new(DocStub(PMutex::new(DocumentGraph::new())));
        let port =
            start_global(Arc::clone(&access) as Arc<dyn DocumentAccess>).expect("start_global");
        assert!(port > 0);
        assert!(is_running());
        // Second call is idempotent: same port, no second server.
        let port2 =
            start_global(Arc::clone(&access) as Arc<dyn DocumentAccess>).expect("re-start_global");
        assert_eq!(port, port2);
        stop_global();
        std::thread::sleep(Duration::from_millis(50));
        assert!(!is_running());
    }

    #[test]
    fn list_artboards_via_payload() {
        let mut doc = DocumentGraph::new();
        doc.insert_node(kcreate_core::node::Node::new(
            kcreate_core::node::NodeType::Artboard,
            "Hero",
        ))
        .expect("insert");
        let access = Arc::new(DocStub(PMutex::new(doc)));
        let resp = handle_payload(
            r#"{"jsonrpc":"2.0","id":1,"method":"list_artboards"}"#,
            &*access,
        );
        let v: Value = serde_json::from_str(&resp).expect("json");
        let arr = v["result"].as_array().expect("array");
        assert_eq!(arr.len(), 1);
    }
}
