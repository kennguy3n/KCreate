# KCreate MCP automation — end-to-end demo agent

`agent.mjs` is a small, **zero-dependency** Node JSON-RPC client that
connects to KCreate's loopback MCP server and drives the app to compose
and export a recognizable design **entirely through MCP tool calls** —
the same surface any external AI agent (Claude Desktop, a custom
orchestrator, etc.) would use.

It proves the workstream end-to-end: discovery (`initialize` /
`tools/list`), then `insert_asset` + `set_fill` + `create_node` +
`set_text` to build a two-card poster, then `export_design` to write a
PNG and an SVG.

## Architecture

```
agent.mjs ──HTTP JSON-RPC──▶ 127.0.0.1:<port>  (kcreate_mcp server)
  initialize / tools/list                │
  tools/call insert_asset ×N             ├─▶ PermissionGate (Once/Always/Denied + master switch)
  tools/call set_fill ×N                 │      consults the SAME store the settings UI edits
  tools/call create_node / set_text      │
  tools/call export_design (png, svg)    └─▶ WorkspaceAccess → real document ops (undoable)
```

The server binds to `127.0.0.1` only and every tool call is gated by the
user's permission decisions. The agent sends an
`X-KCreate-MCP-Client: kcreate-demo-agent` header so the user can see and
govern this agent's granted scopes in the MCP settings panel.

## Run it

The agent needs a running server with a project open and the renderer
initialised. The `mcp_demo_harness` integration test provides exactly
that (it pre-grants the demo agent's scopes, standing in for a user who
approved the agent once in the settings UI):

```bash
# Terminal 1 — boot the workspace + loopback MCP server (stays alive ~90s):
cargo test -p kcreate_tests --test mcp_demo_harness -- --ignored --nocapture

# Terminal 2 — run the agent against it:
node tools/mcp-demo-agent/agent.mjs
```

The harness prints `MCP_PORT=<port>` and writes it to
`target/tmp/mcp_demo_port.txt`; the agent reads that file automatically.

You can also point the agent at any already-running KCreate MCP server
(e.g. the one started from the in-app **Settings → MCP automation**
panel) by passing the port explicitly:

```bash
MCP_PORT=53124 node tools/mcp-demo-agent/agent.mjs
```

## Outputs

Written to `target/tmp/` by default (override with `MCP_DEMO_OUT`):

| File                      | What                                             |
| ------------------------- | ------------------------------------------------ |
| `mcp_demo_poster.png`     | the composed design, rastered by the real engine |
| `mcp_demo_poster.svg`     | the same design's vector geometry                |
| `mcp_demo_transcript.md`  | every JSON-RPC request/response the agent made   |

## Configuration

| Env var         | Default                              | Meaning                          |
| --------------- | ------------------------------------ | -------------------------------- |
| `MCP_PORT`      | _(read from port file)_              | explicit server port             |
| `MCP_PORT_FILE` | `target/tmp/mcp_demo_port.txt`       | where to read the port           |
| `MCP_HOST`      | `127.0.0.1`                          | server host (loopback only)      |
| `MCP_CLIENT`    | `kcreate-demo-agent`                 | client identity header           |
| `MCP_DEMO_OUT`  | `target/tmp`                         | output directory                 |
