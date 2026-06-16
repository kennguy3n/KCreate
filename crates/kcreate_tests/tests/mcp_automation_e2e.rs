//! I5 — MCP automation server, end-to-end proof.
//!
//! Drives the REAL loopback MCP server exactly the way an external AI
//! agent would: raw HTTP `POST /` JSON-RPC over a `TcpStream`, carrying
//! the `X-KCreate-MCP-Client` identity header, speaking the MCP-standard
//! `initialize` / `tools/list` / `tools/call` handshake plus the
//! back-compatible direct method names. No bridge tool fn is called
//! in-process for the compose — every design mutation crosses the wire.
//!
//! It proves three things the workstream requires:
//!
//! 1. **Permission gate is real.** A call with no decision on record is
//!    refused with `PERMISSION_REQUIRED (-32002)` and enqueues a pending
//!    prompt; an explicit `Denied` grant yields `PERMISSION_DENIED
//!    (-32001)`; flipping the master switch off yields `MASTER_DISABLED
//!    (-32003)`. Granting `Always` then makes the same call succeed.
//!    These are asserted against the SAME shared store the settings UI
//!    drives (`phase2::mcp_permission_*`).
//! 2. **Tools mutate the real document.** A recognizable two-card poster
//!    is composed ENTIRELY via `insert_asset` + `set_fill` + `create_node`
//!    + `set_text` tool calls, then exported to PNG and SVG via the
//!    `export_design` tool. The PNG is asserted non-blank (PNG magic +
//!    IDAT + `>= 2` distinct colours + non-trivial size); the SVG is
//!    asserted to carry real `<path>` geometry. The composed artwork is
//!    written under `$CARGO_TARGET_TMPDIR` and its path printed (run with
//!    `-- --nocapture`) so it can be captured for the PR proof.
//! 3. **Results are undoable.** After composing, a fresh `create_node`
//!    tool call lands a real `Operation` on the undo log — `document_undo`
//!    returns `Some` with the matching command + affected node — proving
//!    the tool path went through `execute_operation`, not a fake echo.
//!
//! Lives in its own integration binary because the renderer + workspace
//! + MCP permission store are process-global singletons; a dedicated
//! file gives this test a clean process no other test has touched.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

use kcreate_bridge::document::{
    document_get_tree, document_undo, mcp_start, project_close, project_create,
};
use kcreate_bridge::{phase2, state};
use kcreate_mcp::protocol::codes;
use serde_json::{json, Value};
use serial_test::serial;
use std::collections::HashSet;
use tempfile::TempDir;

const W: u32 = 1280;
const H: u32 = 800;

/// The automation client's stable identity — mirrors what the Node demo
/// client (`tools/mcp-demo-agent`) sends so the granted scopes the user
/// sees in the settings UI match across the test and the live demo.
const CLIENT: &str = "kcreate-demo-agent";

/// Isolate the on-disk permission store to a per-process temp dir so the
/// test never reads or writes the developer's real `~/.kcreate`. Must be
/// set BEFORE the first `phase2::mcp_*` call (the store is a `OnceLock`
/// singleton keyed off `KCREATE_MCP_DIR` at first access).
fn isolated_mcp_dir() -> &'static TempDir {
    static D: OnceLock<TempDir> = OnceLock::new();
    D.get_or_init(|| {
        let dir = TempDir::new().expect("mcp permission tmpdir");
        std::env::set_var("KCREATE_MCP_DIR", dir.path());
        dir
    })
}

// --- raw HTTP JSON-RPC client (what an external agent uses) --------------

/// POST one JSON-RPC request to the loopback server with the client
/// identity header and return the parsed JSON-RPC response envelope.
fn http_rpc(port: u16, client: &str, method: &str, params: Value) -> Value {
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params,
    })
    .to_string();
    let request = format!(
        "POST / HTTP/1.1\r\n\
         Host: 127.0.0.1:{port}\r\n\
         X-KCreate-MCP-Client: {client}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {len}\r\n\
         Connection: close\r\n\
         \r\n\
         {body}",
        len = body.len(),
    );

    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to mcp server");
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .expect("set read timeout");
    stream
        .write_all(request.as_bytes())
        .expect("write mcp request");
    stream.flush().ok();

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).expect("read mcp response");
    let text = String::from_utf8_lossy(&raw);
    let split = text
        .find("\r\n\r\n")
        .unwrap_or_else(|| panic!("malformed HTTP response (no header/body split): {text:?}"));
    let json_body = &text[split + 4..];
    serde_json::from_str(json_body)
        .unwrap_or_else(|e| panic!("parse JSON-RPC body failed: {e}\nbody={json_body:?}"))
}

/// Call a tool through the MCP-standard `tools/call` envelope, assert it
/// did not error, and return its `structuredContent` (the raw tool
/// result an agent consumes programmatically).
fn call_tool(port: u16, client: &str, name: &str, arguments: Value) -> Value {
    let resp = http_rpc(
        port,
        client,
        "tools/call",
        json!({ "name": name, "arguments": arguments }),
    );
    assert!(
        resp.get("error").is_none(),
        "tools/call {name} unexpected JSON-RPC error: {resp}"
    );
    let result = resp
        .get("result")
        .unwrap_or_else(|| panic!("tools/call {name} missing result: {resp}"));
    assert_eq!(
        result.get("isError"),
        Some(&Value::Bool(false)),
        "tool {name} reported isError: {result}"
    );
    result
        .get("structuredContent")
        .cloned()
        .unwrap_or(Value::Null)
}

/// Issue a call expected to be refused by the permission gate and return
/// the JSON-RPC error code.
fn expect_error_code(port: u16, client: &str, method: &str, params: Value) -> i32 {
    let resp = http_rpc(port, client, method, params);
    let code = resp
        .get("error")
        .and_then(|e| e.get("code"))
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("expected an error envelope, got: {resp}"));
    i32::try_from(code).expect("error code fits in i32")
}

// --- proof helpers -------------------------------------------------------

/// Count distinct RGBA colours in a PNG, capped at `cap`. A blank render
/// collapses to one colour; `>= 2` proves the design was actually drawn.
fn distinct_colors(png: &[u8], cap: usize) -> usize {
    let img = image::load_from_memory(png).expect("decode exported PNG");
    let rgba = img.to_rgba8();
    let mut seen: HashSet<[u8; 4]> = HashSet::new();
    for px in rgba.pixels() {
        seen.insert(px.0);
        if seen.len() >= cap {
            break;
        }
    }
    seen.len()
}

/// First leaf vector-node id of an inserted asset, as a `String` (the
/// `node_ids` are the recolourable editable leaves).
fn first_leaf(inserted: &Value) -> String {
    inserted["node_ids"][0]
        .as_str()
        .unwrap_or_else(|| panic!("inserted asset missing node_ids[0]: {inserted}"))
        .to_string()
}

#[test]
#[serial]
fn external_agent_composes_and_exports_a_recognizable_design_over_mcp() {
    // --- isolate + boot ---------------------------------------------------
    isolated_mcp_dir();
    project_close();
    let proj_dir = TempDir::new().expect("project tmpdir");
    project_create("i5-mcp-automation", proj_dir.path()).expect("project_create");
    // Renderer must be live before any scene-affecting mutation so each
    // tool's `sync_scene_locked` composes the scene (otherwise the PNG
    // export later finds an empty scene slot and fails NotInitialized).
    state::init(W, H).expect("init renderer");

    // Start from a clean permission slate and a master switch ON.
    let _ = phase2::mcp_set_master_enabled(true);

    let port = u16::try_from(mcp_start().expect("start mcp server")).expect("port fits u16");

    // --- discovery is ungated --------------------------------------------
    let init = http_rpc(port, CLIENT, "initialize", json!({}));
    assert!(
        init["result"]["serverInfo"].is_object(),
        "initialize must return serverInfo: {init}"
    );
    let listed = http_rpc(port, CLIENT, "tools/list", json!({}));
    let tools = listed["result"]["tools"]
        .as_array()
        .expect("tools/list returns a tools array");
    let advertised: HashSet<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    for required in [
        "list_artboards",
        "apply_template",
        "generate_themed_design",
        "list_assets",
        "insert_asset",
        "set_fill",
        "set_text",
        "apply_theme",
        "magic_resize",
        "export_design",
        "create_node",
    ] {
        assert!(
            advertised.contains(required),
            "tools/list must advertise {required}; got {advertised:?}"
        );
    }

    // --- permission gate: no decision on record → PROMPT -----------------
    let code = expect_error_code(port, CLIENT, "list_artboards", json!({}));
    assert_eq!(
        code,
        codes::PERMISSION_REQUIRED,
        "first call with no grant must be PERMISSION_REQUIRED"
    );
    // The refusal must have enqueued a pending prompt for the UI.
    let pending: Value =
        serde_json::from_str(&phase2::mcp_pending_list().expect("pending list")).expect("parse");
    assert!(
        pending
            .as_array()
            .expect("pending is array")
            .iter()
            .any(|p| p["client_id"] == json!(CLIENT) && p["tool_name"] == json!("list_artboards")),
        "a refused call must surface as a pending prompt: {pending}"
    );

    // --- grant Always → same call now succeeds ---------------------------
    phase2::mcp_permission_grant(CLIENT, "list_artboards", "always").expect("grant always");
    let after_grant = http_rpc(port, CLIENT, "list_artboards", json!({}));
    assert!(
        after_grant.get("error").is_none() && after_grant.get("result").is_some(),
        "after Always grant the call must succeed: {after_grant}"
    );
    // Granting clears the pending prompt.
    let pending: Value =
        serde_json::from_str(&phase2::mcp_pending_list().expect("pending list")).expect("parse");
    assert!(
        !pending
            .as_array()
            .expect("pending is array")
            .iter()
            .any(|p| p["client_id"] == json!(CLIENT) && p["tool_name"] == json!("list_artboards")),
        "granting must clear the pending prompt: {pending}"
    );

    // --- explicit Denied → PERMISSION_DENIED -----------------------------
    phase2::mcp_permission_grant(CLIENT, "magic_resize", "denied").expect("grant denied");
    let code = expect_error_code(port, CLIENT, "magic_resize", json!({}));
    assert_eq!(
        code,
        codes::PERMISSION_DENIED,
        "a Denied scope must refuse with PERMISSION_DENIED"
    );

    // --- master switch off → MASTER_DISABLED (overrides grants) ----------
    phase2::mcp_set_master_enabled(false).expect("master off");
    let code = expect_error_code(port, CLIENT, "list_artboards", json!({}));
    assert_eq!(
        code,
        codes::MASTER_DISABLED,
        "with the master switch off even granted tools are refused"
    );
    phase2::mcp_set_master_enabled(true).expect("master on");

    // --- grant the compose tools (what the user approves once) -----------
    for tool in [
        "insert_asset",
        "set_fill",
        "set_text",
        "create_node",
        "export_design",
    ] {
        phase2::mcp_permission_grant(CLIENT, tool, "always")
            .unwrap_or_else(|e| panic!("grant {tool}: {e}"));
    }

    // --- compose a recognizable two-card poster, entirely over MCP -------
    // Card A (left) background + chart glyph; Card B (right) background +
    // rocket illustration; two accent dots up top; a title text node.
    let card_a = call_tool(
        port,
        CLIENT,
        "insert_asset",
        json!({ "asset_id": "rounded-rectangle", "x": 90.0, "y": 220.0, "target_size": 520.0 }),
    );
    call_tool(
        port,
        CLIENT,
        "set_fill",
        json!({ "node_id": first_leaf(&card_a), "color": "#4361EE" }),
    );
    call_tool(
        port,
        CLIENT,
        "insert_asset",
        json!({ "asset_id": "chart-bar", "x": 190.0, "y": 330.0, "target_size": 300.0 }),
    );

    let card_b = call_tool(
        port,
        CLIENT,
        "insert_asset",
        json!({ "asset_id": "rounded-rectangle", "x": 690.0, "y": 220.0, "target_size": 520.0 }),
    );
    call_tool(
        port,
        CLIENT,
        "set_fill",
        json!({ "node_id": first_leaf(&card_b), "color": "#22C55E" }),
    );
    call_tool(
        port,
        CLIENT,
        "insert_asset",
        json!({ "asset_id": "rocket-illo", "x": 800.0, "y": 320.0, "target_size": 320.0 }),
    );

    let dot_a = call_tool(
        port,
        CLIENT,
        "insert_asset",
        json!({ "asset_id": "circle", "x": 90.0, "y": 110.0, "target_size": 70.0 }),
    );
    call_tool(
        port,
        CLIENT,
        "set_fill",
        json!({ "node_id": first_leaf(&dot_a), "color": "#F59E0B" }),
    );
    let dot_b = call_tool(
        port,
        CLIENT,
        "insert_asset",
        json!({ "asset_id": "circle", "x": 180.0, "y": 110.0, "target_size": 70.0 }),
    );
    call_tool(
        port,
        CLIENT,
        "set_fill",
        json!({ "node_id": first_leaf(&dot_b), "color": "#0EA5E9" }),
    );

    // Title text node — exercises create_node + set_text through the wire.
    // (Text is zero-bounds / not raster-rendered by the SVG path, but the
    // node and its content land in the real document graph + op log.)
    let title = call_tool(
        port,
        CLIENT,
        "create_node",
        json!({ "node_type": "text", "name": "poster-title" }),
    );
    let title_id = title["id"]
        .as_str()
        .unwrap_or_else(|| panic!("create_node must return an id: {title}"))
        .to_string();
    call_tool(
        port,
        CLIENT,
        "set_text",
        json!({ "node_id": title_id, "content": "KCreate · driven by MCP" }),
    );

    // --- the composed document is real ------------------------------------
    let tree = document_get_tree().expect("document tree");
    let vector_leaves = tree.iter().filter(|n| n.node_type == "VectorLayer").count();
    assert!(
        vector_leaves >= 6,
        "the inserted assets must materialise as real vector leaves, got {vector_leaves}"
    );
    assert!(
        tree.iter().any(|n| n.node_type == "TextLayer"),
        "the title create_node must materialise as a real text node"
    );

    // --- export the composed design via the MCP export tool --------------
    let png_path = Path::new(env!("CARGO_TARGET_TMPDIR")).join("i5_mcp_poster.png");
    let png_res = call_tool(
        port,
        CLIENT,
        "export_design",
        json!({
            "format": "png",
            "path": png_path.to_str().expect("utf-8 path"),
            "options": { "width": W, "height": H, "scale": 1.0, "background": [1.0, 1.0, 1.0, 1.0] },
        }),
    );
    assert!(
        png_res["bytes_written"].as_u64().unwrap_or(0) > 2_000,
        "export_design png must write a non-trivial file: {png_res}"
    );
    println!("PROOF_PNG={}", png_path.display());

    let svg_path = Path::new(env!("CARGO_TARGET_TMPDIR")).join("i5_mcp_poster.svg");
    let svg_res = call_tool(
        port,
        CLIENT,
        "export_design",
        json!({
            "format": "svg",
            "path": svg_path.to_str().expect("utf-8 path"),
            "options": {},
        }),
    );
    assert!(
        svg_res["bytes_written"].as_u64().unwrap_or(0) > 0,
        "export_design svg must write a file: {svg_res}"
    );
    println!("PROOF_SVG={}", svg_path.display());

    // --- assert the artefacts are recognizable, not blank ----------------
    let png = std::fs::read(&png_path).expect("read exported png");
    assert!(
        png.starts_with(&[0x89, b'P', b'N', b'G']),
        "exported file must be a PNG (magic header)"
    );
    assert!(
        png.windows(4).any(|w| w == b"IDAT"),
        "PNG must carry pixel data (IDAT chunk)"
    );
    assert!(
        distinct_colors(&png, 8) >= 2,
        "a real composed poster must have multiple colours, not a blank field"
    );

    let svg = std::fs::read_to_string(&svg_path).expect("read exported svg");
    assert!(svg.contains("<svg"), "svg must be a real SVG document");
    assert!(
        svg.contains("<path"),
        "svg must carry the inserted assets' vector geometry"
    );

    // --- results are undoable: a tool call lands on the op log -----------
    let probe = call_tool(
        port,
        CLIENT,
        "create_node",
        json!({ "node_type": "group", "name": "undo-probe" }),
    );
    let probe_id = probe["id"]
        .as_str()
        .unwrap_or_else(|| panic!("create_node must return an id: {probe}"))
        .to_string();
    let outcome = document_undo()
        .expect("document_undo")
        .expect("the agent's create_node must be on the undo stack");
    assert_eq!(
        outcome.command, "mcp_create_node",
        "the undone op must be the MCP create_node"
    );
    assert!(
        outcome
            .affected_nodes
            .iter()
            .any(|id| id.to_string() == probe_id),
        "the undone op must reference the node the tool created"
    );

    project_close();
}
