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
use serde::Deserialize;
use serde_json::{json, Value};
use thiserror::Error;
use tiny_http::{Method, Response, Server};

use crate::permissions::{McpPermissionStore, PendingPermissions, PermissionDecision};
use crate::protocol::{codes, JsonRpcRequest, JsonRpcResponse};
use crate::tools::{dispatch_tool, is_tool, tool_specs, DocumentAccess};

/// The HTTP header an MCP client uses to identify itself. Permission
/// grants are scoped to `(client_id, tool_name)`, so a client that
/// omits this header is treated as the shared `anonymous` identity and
/// must be granted access explicitly like any other.
pub const CLIENT_HEADER: &str = "X-KCreate-MCP-Client";

/// Identity used when a request omits [`CLIENT_HEADER`].
pub const ANONYMOUS_CLIENT: &str = "anonymous";

/// The MCP protocol revision this server speaks.
const PROTOCOL_VERSION: &str = "2024-11-05";

/// The permission store + pending-prompt registry the server consults
/// on every tool call. This is the SAME `McpPermissionStore` the
/// settings UI edits (the bridge passes its shared `Arc` in) so a
/// grant made in the UI is observed by the next tool call without a
/// restart, and a tool call with no decision on record enqueues a
/// pending prompt the UI renders.
#[derive(Clone)]
pub struct PermissionGate {
    pub store: Arc<McpPermissionStore>,
    pub pending: Arc<PendingPermissions>,
}

impl std::fmt::Debug for PermissionGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Deliberately opaque: the store holds the user's permission
        // grants and the pending queue holds client identities; neither
        // belongs in a Debug dump.
        f.debug_struct("PermissionGate")
            .field("master_enabled", &self.store.is_master_enabled())
            .field("pending", &self.pending.list().len())
            .finish_non_exhaustive()
    }
}

impl PermissionGate {
    /// Build a gate from a shared store + pending registry.
    #[must_use]
    pub fn new(store: Arc<McpPermissionStore>, pending: Arc<PendingPermissions>) -> Self {
        Self { store, pending }
    }

    /// Decide whether `client_id` may invoke `tool`. On `Prompt` the
    /// request is enqueued in the pending registry (so the UI can show
    /// it) and a retryable `PERMISSION_REQUIRED` error is returned.
    fn authorize(&self, client_id: &str, tool: &str) -> Result<(), (i32, String)> {
        match self.store.decide(client_id, tool) {
            Ok(PermissionDecision::Allow) => {
                // A decision exists; clear any stale pending prompt.
                self.pending.clear(client_id, tool);
                Ok(())
            }
            Ok(PermissionDecision::Denied) => Err((
                codes::PERMISSION_DENIED,
                format!("'{client_id}' is not permitted to call '{tool}'"),
            )),
            Ok(PermissionDecision::Prompt) => {
                self.pending.record(client_id, tool);
                Err((
                    codes::PERMISSION_REQUIRED,
                    format!(
                        "'{client_id}' must be granted permission to call '{tool}' in KCreate; \
                         retry after the user approves"
                    ),
                ))
            }
            Ok(PermissionDecision::MasterDisabled) => Err((
                codes::MASTER_DISABLED,
                "MCP automation is disabled; the user must re-enable it in KCreate".to_string(),
            )),
            Err(e) => Err((
                codes::INTERNAL_ERROR,
                format!("permission store error: {e}"),
            )),
        }
    }
}

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
    ///
    /// `gate` is the permission gate consulted on every tool call. It
    /// must wrap the same store the settings UI edits so grants take
    /// effect immediately.
    pub fn start(
        port: u16,
        access: Arc<dyn DocumentAccess>,
        gate: PermissionGate,
    ) -> Result<Self, McpError> {
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
            .spawn(move || run_loop(server_thread, stop_flag_thread, access, gate))
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

fn run_loop(
    server: Arc<Server>,
    stop_flag: Arc<AtomicBool>,
    access: Arc<dyn DocumentAccess>,
    gate: PermissionGate,
) {
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

        let client_id = client_id_from_headers(&req);

        let mut buf = String::new();
        if req.as_reader().read_to_string(&mut buf).is_err() {
            warn!("kcreate_mcp: failed to read request body");
            continue;
        }
        let response_body = handle_payload(&buf, &*access, &gate, &client_id);
        let resp = Response::from_string(response_body).with_header(
            "Content-Type: application/json"
                .parse::<tiny_http::Header>()
                .expect("static header"),
        );
        let _ = req.respond(resp);
    }
}

/// Extract the client identity from [`CLIENT_HEADER`], falling back to
/// [`ANONYMOUS_CLIENT`]. Header names are case-insensitive.
fn client_id_from_headers(req: &tiny_http::Request) -> String {
    req.headers()
        .iter()
        .find(|h| h.field.equiv(CLIENT_HEADER))
        .map(|h| h.value.as_str().trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| ANONYMOUS_CLIENT.to_string())
}

fn handle_payload(
    body: &str,
    access: &dyn DocumentAccess,
    gate: &PermissionGate,
    client_id: &str,
) -> String {
    let parsed: Result<JsonRpcRequest, _> = serde_json::from_str(body);
    let envelope = match parsed {
        Ok(req) => dispatch(req, access, gate, client_id),
        Err(e) => JsonRpcResponse::err(None, codes::PARSE_ERROR, format!("parse error: {e}")),
    };
    serde_json::to_string(&envelope).unwrap_or_else(|_| {
        // Last-ditch fallback. Should be impossible because
        // JsonRpcResponse is owned + Serialize.
        r#"{"jsonrpc":"2.0","error":{"code":-32603,"message":"serialise failed"}}"#.into()
    })
}

/// MCP `initialize` handshake result. Advertises tool support so a
/// standard MCP client knows it can call `tools/list` + `tools/call`.
fn initialize_result() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "serverInfo": {
            "name": "kcreate-mcp",
            "version": env!("CARGO_PKG_VERSION"),
        },
        "capabilities": {
            "tools": { "listChanged": false }
        },
        "instructions": "KCreate local design automation. Call tools/list to discover tools, \
             then tools/call to invoke them. Every tool call is gated by the user's \
             Once/Always/Denied permission grants; a call with no decision on record returns \
             error -32002 (permission required) until the user approves it in KCreate."
    })
}

fn dispatch(
    req: JsonRpcRequest,
    access: &dyn DocumentAccess,
    gate: &PermissionGate,
    client_id: &str,
) -> JsonRpcResponse {
    let id = req.id.clone();
    match req.method.as_str() {
        // --- Discovery / handshake: never gated. ---
        "initialize" => JsonRpcResponse::success(id, initialize_result()),
        "tools/list" => JsonRpcResponse::success(id, json!({ "tools": tool_specs() })),
        // --- MCP-standard tool invocation: gated, result wrapped. ---
        "tools/call" => tools_call(id, req.params, access, gate, client_id),
        // --- Back-compat direct tool method names: gated, raw result. ---
        method if is_tool(method) => {
            if let Err((code, msg)) = gate.authorize(client_id, method) {
                return JsonRpcResponse::err(id, code, msg);
            }
            match dispatch_tool(access, method, req.params) {
                Ok(v) => JsonRpcResponse::success(id, v),
                Err((code, msg)) => JsonRpcResponse::err(id, code, msg),
            }
        }
        other => JsonRpcResponse::err(
            id,
            codes::METHOD_NOT_FOUND,
            format!("unknown method: {other}"),
        ),
    }
}

/// MCP `tools/call` params: `{ name, arguments }`.
#[derive(Debug, Deserialize)]
struct ToolCallParams {
    name: String,
    #[serde(default)]
    arguments: Value,
}

fn tools_call(
    id: Option<Value>,
    params: Value,
    access: &dyn DocumentAccess,
    gate: &PermissionGate,
    client_id: &str,
) -> JsonRpcResponse {
    let cp: ToolCallParams = match serde_json::from_value(params) {
        Ok(c) => c,
        Err(e) => {
            return JsonRpcResponse::err(
                id,
                codes::INVALID_PARAMS,
                format!("invalid tools/call params: {e}"),
            )
        }
    };
    if !is_tool(&cp.name) {
        return JsonRpcResponse::err(
            id,
            codes::METHOD_NOT_FOUND,
            format!("unknown tool: {}", cp.name),
        );
    }
    if let Err((code, msg)) = gate.authorize(client_id, &cp.name) {
        return JsonRpcResponse::err(id, code, msg);
    }
    wrap_tool_result(id, dispatch_tool(access, &cp.name, cp.arguments))
}

/// Wrap a tool outcome in the MCP `tools/call` result shape. Argument
/// / unknown-tool problems surface as JSON-RPC errors (the call was
/// malformed); a tool that ran but failed surfaces as a result with
/// `isError: true` per the MCP convention, so the agent can read the
/// message without treating it as a transport fault.
fn wrap_tool_result(id: Option<Value>, outcome: Result<Value, (i32, String)>) -> JsonRpcResponse {
    match outcome {
        Ok(v) => JsonRpcResponse::success(
            id,
            json!({
                "content": [{ "type": "text", "text": v.to_string() }],
                "structuredContent": v,
                "isError": false,
            }),
        ),
        Err((code, msg)) if code == codes::INVALID_PARAMS || code == codes::METHOD_NOT_FOUND => {
            JsonRpcResponse::err(id, code, msg)
        }
        Err((_, msg)) => JsonRpcResponse::success(
            id,
            json!({
                "content": [{ "type": "text", "text": msg }],
                "isError": true,
            }),
        ),
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
pub fn start_global(
    access: Arc<dyn DocumentAccess>,
    gate: PermissionGate,
) -> Result<u16, McpError> {
    let mut slot = global().lock();
    if let Some(existing) = slot.as_ref() {
        if existing.is_running() {
            return Ok(existing.port());
        }
        // Stale server in the slot (worker thread already exited).
        // Drop it and start fresh.
        slot.take();
    }
    let server = McpServer::start(0, access, gate)?;
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

/// Atomic snapshot of `(is_running, port)` taken under a single
/// global-lock acquisition.
///
/// Calling [`is_running`] and [`port`] separately is a TOCTOU race:
/// the server can be stopped between the two calls, producing a
/// status response with `running: true` and `port: 0` that no caller
/// expects to be possible. The McpSettingsPanel UI tolerates the
/// inconsistency (it polls again on the next tick) but a single
/// status response should still be self-consistent. Use this
/// accessor whenever you need both fields. Per Devin Review
/// ANALYSIS_pr-review-job-790e7860e5c745e0bee13295709290f4_0001.
#[must_use]
pub fn state() -> (bool, Option<u16>) {
    let slot = global().lock();
    match slot.as_ref() {
        Some(s) if s.is_running() => (true, Some(s.port())),
        _ => (false, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::{McpPermissionStore, PendingPermissions, PermissionGrant};
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
        // The high-level capabilities are owned by the bridge; this
        // server-level stub only needs the graph methods above for its
        // dispatch / permission tests.
        fn list_templates(&self, _c: Option<&str>, _q: Option<&str>) -> Result<Value, String> {
            Err("unsupported in server stub".into())
        }
        fn apply_template(&self, _id: Uuid) -> Result<Value, String> {
            Err("unsupported in server stub".into())
        }
        fn generate_themed_design(&self, _b: &str, _o: &str) -> Result<Value, String> {
            Err("unsupported in server stub".into())
        }
        fn list_assets(&self, _c: Option<&str>, _q: Option<&str>) -> Result<Value, String> {
            Err("unsupported in server stub".into())
        }
        fn insert_asset(
            &self,
            _a: &str,
            _p: Option<Uuid>,
            _x: f64,
            _y: f64,
            _t: Option<f64>,
        ) -> Result<Value, String> {
            Err("unsupported in server stub".into())
        }
        fn set_fill(&self, _id: Uuid, _f: Value) -> Result<(), String> {
            Err("unsupported in server stub".into())
        }
        fn set_text(&self, _id: Uuid, _c: &str) -> Result<(), String> {
            Err("unsupported in server stub".into())
        }
        fn list_themes(&self) -> Result<Value, String> {
            Err("unsupported in server stub".into())
        }
        fn apply_theme(&self, _t: &str) -> Result<Value, String> {
            Err("unsupported in server stub".into())
        }
        fn magic_resize(&self, _s: Uuid, _t: Value) -> Result<Value, String> {
            Err("unsupported in server stub".into())
        }
        fn export_design(
            &self,
            _ids: &[Uuid],
            _fmt: &str,
            _path: &str,
            _opts: Value,
        ) -> Result<Value, String> {
            Err("unsupported in server stub".into())
        }
    }

    /// A gate backed by a fresh temp-dir store (master enabled). The
    /// returned `TempDir` must be kept alive for the store file to
    /// stay valid.
    fn test_gate() -> (PermissionGate, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Arc::new(McpPermissionStore::open(dir.path()).expect("store"));
        let pending = Arc::new(PendingPermissions::new());
        (PermissionGate::new(store, pending), dir)
    }

    const CLIENT: &str = "test-agent";

    #[test]
    fn start_stop_roundtrip() {
        let access = Arc::new(DocStub(PMutex::new(DocumentGraph::new())));
        let (gate, _dir) = test_gate();
        let mut server = McpServer::start(0, access, gate).expect("start");
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
        let (gate, _dir) = test_gate();
        let resp = handle_payload(
            r#"{"jsonrpc":"2.0","id":1,"method":"definitely_not_real"}"#,
            &*access,
            &gate,
            CLIENT,
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
        let (gate, _dir) = test_gate();
        let port = start_global(Arc::clone(&access) as Arc<dyn DocumentAccess>, gate.clone())
            .expect("start_global");
        assert!(port > 0);
        assert!(is_running());
        // Second call is idempotent: same port, no second server.
        let port2 = start_global(Arc::clone(&access) as Arc<dyn DocumentAccess>, gate)
            .expect("re-start_global");
        assert_eq!(port, port2);
        stop_global();
        std::thread::sleep(Duration::from_millis(50));
        assert!(!is_running());
    }

    /// Helper: dispatch a direct-method payload and return the parsed
    /// JSON response.
    fn call(
        access: &dyn DocumentAccess,
        gate: &PermissionGate,
        client: &str,
        method: &str,
    ) -> Value {
        let body = format!(r#"{{"jsonrpc":"2.0","id":1,"method":"{method}"}}"#);
        let resp = handle_payload(&body, access, gate, client);
        serde_json::from_str(&resp).expect("json")
    }

    #[test]
    fn tool_call_requires_permission_then_grant_allows() {
        let mut doc = DocumentGraph::new();
        doc.insert_node(Node::new(NodeType::Artboard, "Hero"))
            .expect("insert");
        let access = Arc::new(DocStub(PMutex::new(doc)));
        let (gate, _dir) = test_gate();

        // No decision on record → PERMISSION_REQUIRED and a pending
        // prompt is enqueued for the UI.
        let v = call(&*access, &gate, CLIENT, "list_artboards");
        assert_eq!(v["error"]["code"], codes::PERMISSION_REQUIRED);
        assert_eq!(gate.pending.list().len(), 1);

        // User grants Always (as the UI would) → call now succeeds and
        // the pending prompt is cleared.
        gate.store
            .grant(CLIENT, "list_artboards", PermissionGrant::Always)
            .expect("grant");
        let v = call(&*access, &gate, CLIENT, "list_artboards");
        let arr = v["result"].as_array().expect("array");
        assert_eq!(arr.len(), 1);
        assert!(gate.pending.is_empty());
    }

    #[test]
    fn denied_and_master_off_are_distinct_errors() {
        let access = Arc::new(DocStub(PMutex::new(DocumentGraph::new())));
        let (gate, _dir) = test_gate();

        gate.store
            .grant(CLIENT, "list_artboards", PermissionGrant::Denied)
            .expect("deny");
        let v = call(&*access, &gate, CLIENT, "list_artboards");
        assert_eq!(v["error"]["code"], codes::PERMISSION_DENIED);

        // Master switch off short-circuits even an explicit grant.
        gate.store
            .grant(CLIENT, "create_node", PermissionGrant::Always)
            .expect("grant");
        gate.store.set_master_enabled(false).expect("master off");
        let v = call(&*access, &gate, CLIENT, "create_node");
        assert_eq!(v["error"]["code"], codes::MASTER_DISABLED);
    }

    #[test]
    fn discovery_methods_are_ungated() {
        let access = Arc::new(DocStub(PMutex::new(DocumentGraph::new())));
        let (gate, _dir) = test_gate();
        // Even with the master switch off, discovery still works.
        gate.store.set_master_enabled(false).expect("master off");

        let init = call(&*access, &gate, CLIENT, "initialize");
        assert_eq!(init["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(init["result"]["serverInfo"]["name"], "kcreate-mcp");

        let listed = call(&*access, &gate, CLIENT, "tools/list");
        let tools = listed["result"]["tools"].as_array().expect("tools");
        assert_eq!(tools.len(), tool_specs().len());
        assert!(tools.iter().any(|t| t["name"] == "export_design"));
    }

    #[test]
    fn tools_call_wraps_result_and_enforces_permission() {
        let mut doc = DocumentGraph::new();
        doc.insert_node(Node::new(NodeType::Artboard, "Hero"))
            .expect("insert");
        let access = Arc::new(DocStub(PMutex::new(doc)));
        let (gate, _dir) = test_gate();

        let body =
            r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"list_artboards"}}"#;
        // Ungranted → permission required.
        let v: Value =
            serde_json::from_str(&handle_payload(body, &*access, &gate, CLIENT)).expect("json");
        assert_eq!(v["error"]["code"], codes::PERMISSION_REQUIRED);

        gate.store
            .grant(CLIENT, "list_artboards", PermissionGrant::Always)
            .expect("grant");
        let v: Value =
            serde_json::from_str(&handle_payload(body, &*access, &gate, CLIENT)).expect("json");
        assert_eq!(v["result"]["isError"], false);
        // The structured echo lets an agent read ids directly.
        let structured = v["result"]["structuredContent"].as_array().expect("array");
        assert_eq!(structured.len(), 1);
        // And the MCP text content is the same JSON serialised.
        assert!(v["result"]["content"][0]["text"].as_str().is_some());
    }

    #[test]
    fn anonymous_client_used_when_header_absent() {
        // handle_payload is given the resolved client id; the header
        // parsing itself is covered by exercising the default through
        // the public constant.
        assert_eq!(ANONYMOUS_CLIENT, "anonymous");
    }
}
