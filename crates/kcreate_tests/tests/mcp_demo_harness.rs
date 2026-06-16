//! Serve-for-demo harness for the end-to-end MCP automation client.
//!
//! This is the live counterpart to `mcp_automation_e2e.rs`: instead of
//! driving the server in-process, it boots a real KCreate workspace +
//! the loopback MCP server and then *stays alive* so the external Node
//! agent (`tools/mcp-demo-agent/agent.mjs`) can connect over real HTTP
//! JSON-RPC and compose + export a recognizable design — exactly the
//! "an AI agent drives KCreate on-device" demo the workstream requires.
//!
//! It is `#[ignore]`d because it blocks; run it explicitly:
//!
//! ```bash
//! cargo test -p kcreate_tests --test mcp_demo_harness -- --ignored --nocapture
//! # then, in another shell:
//! node tools/mcp-demo-agent/agent.mjs
//! ```
//!
//! The harness:
//! * isolates the permission store to a temp dir (never touches
//!   `~/.kcreate`),
//! * initialises the headless renderer (so `export_design` png/pdf can
//!   raster the scene),
//! * opens a fresh project,
//! * pre-grants `Always` to the demo agent for the tools it uses — this
//!   stands in for the user having approved the agent once in the
//!   settings UI; the full prompt → grant → deny → master-off flow is
//!   asserted separately in `mcp_automation_e2e.rs`,
//! * starts the loopback server, prints `MCP_PORT=<port>` and writes the
//!   port to `$CARGO_TARGET_TMPDIR/mcp_demo_port.txt` (the path the Node
//!   agent reads by default),
//! * then idles for `$KCREATE_MCP_DEMO_SECS` (default 90) so the agent
//!   can run, before closing the project.

use std::path::Path;
use std::time::{Duration, Instant};

use kcreate_bridge::document::{mcp_start, project_close, project_create};
use kcreate_bridge::{phase2, state};
use serial_test::serial;
use tempfile::TempDir;

const W: u32 = 1280;
const H: u32 = 800;

/// Must match the `X-KCreate-MCP-Client` header the Node agent sends.
const DEMO_CLIENT: &str = "kcreate-demo-agent";

/// Tools the demo agent is pre-authorised to use.
const DEMO_TOOLS: &[&str] = &[
    "list_artboards",
    "list_assets",
    "insert_asset",
    "set_fill",
    "set_text",
    "create_node",
    "apply_theme",
    "export_design",
];

#[test]
#[ignore = "long-running serve-for-demo harness; run with --ignored alongside the Node agent"]
#[serial]
fn serve_mcp_for_external_agent_demo() {
    // Isolate permission state to a temp dir for the harness lifetime.
    let mcp_dir = TempDir::new().expect("mcp permission tmpdir");
    std::env::set_var("KCREATE_MCP_DIR", mcp_dir.path());

    project_close();
    let proj_dir = TempDir::new().expect("project tmpdir");
    project_create("i5-mcp-demo", proj_dir.path()).expect("project_create");
    state::init(W, H).expect("init renderer");

    phase2::mcp_set_master_enabled(true).expect("enable master switch");
    for tool in DEMO_TOOLS {
        phase2::mcp_permission_grant(DEMO_CLIENT, tool, "always")
            .unwrap_or_else(|e| panic!("grant {tool}: {e}"));
    }

    let port = mcp_start().expect("start mcp server");

    let port_file = Path::new(env!("CARGO_TARGET_TMPDIR")).join("mcp_demo_port.txt");
    std::fs::write(&port_file, port.to_string()).expect("write port file");

    let secs: u64 = std::env::var("KCREATE_MCP_DEMO_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(90);

    println!("MCP_PORT={port}");
    println!("MCP_PORT_FILE={}", port_file.display());
    println!("MCP_CLIENT={DEMO_CLIENT}");
    println!("MCP_DEMO_SECS={secs}");
    println!("MCP server ready on 127.0.0.1:{port} — run: node tools/mcp-demo-agent/agent.mjs");

    // Idle so the external agent can connect and drive the design. The
    // server runs on its own thread; this just keeps the workspace +
    // renderer singletons alive for the demo window.
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(250));
    }

    project_close();
    let _ = std::fs::remove_file(&port_file);
}
