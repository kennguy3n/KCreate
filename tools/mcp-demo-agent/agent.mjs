#!/usr/bin/env node
// I5 — external MCP automation agent (end-to-end demo).
//
// A zero-dependency Node JSON-RPC client that connects to the loopback
// KCreate MCP server and drives it to compose + export a recognizable
// two-card poster ENTIRELY through MCP tool calls — the same surface any
// third-party AI agent would use. Run it against the serve-for-demo
// harness:
//
//   cargo test -p kcreate_tests --test mcp_demo_harness -- --ignored --nocapture
//   node tools/mcp-demo-agent/agent.mjs
//
// It speaks the MCP-standard handshake (initialize / tools/list /
// tools/call), carries the X-KCreate-MCP-Client identity header so the
// user can see and govern this agent's scopes in the settings UI, writes
// a full JSON-RPC transcript next to the exported design, and verifies
// the exported PNG is a real, non-trivial file.
//
// Configuration (all optional, env vars):
//   MCP_PORT        explicit server port (skips the port file)
//   MCP_PORT_FILE   path to the port file (default <repo>/target/tmp/mcp_demo_port.txt)
//   MCP_HOST        server host (default 127.0.0.1 — loopback only)
//   MCP_CLIENT      client identity header (default kcreate-demo-agent)
//   MCP_DEMO_OUT    output directory (default <repo>/target/tmp)

import http from "node:http";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(HERE, "..", "..");

const HOST = process.env.MCP_HOST ?? "127.0.0.1";
const CLIENT = process.env.MCP_CLIENT ?? "kcreate-demo-agent";
const OUT_DIR = process.env.MCP_DEMO_OUT ?? path.join(REPO_ROOT, "target", "tmp");
const PORT_FILE =
  process.env.MCP_PORT_FILE ?? path.join(REPO_ROOT, "target", "tmp", "mcp_demo_port.txt");

const PNG_PATH = path.join(OUT_DIR, "mcp_demo_poster.png");
const SVG_PATH = path.join(OUT_DIR, "mcp_demo_poster.svg");
const TRANSCRIPT_PATH = path.join(OUT_DIR, "mcp_demo_transcript.md");

/** @type {Array<{n:number, method:string, params:unknown, ok:boolean, response:unknown}>} */
const transcript = [];
let rpcId = 0;

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** Resolve the server port from MCP_PORT or by polling the port file. */
async function resolvePort() {
  if (process.env.MCP_PORT) return Number(process.env.MCP_PORT);
  const deadline = Date.now() + 15_000;
  while (Date.now() < deadline) {
    try {
      const raw = fs.readFileSync(PORT_FILE, "utf8").trim();
      if (raw) return Number(raw);
    } catch {
      // not written yet
    }
    await sleep(250);
  }
  throw new Error(
    `no port: set MCP_PORT or start the harness so it writes ${PORT_FILE}`,
  );
}

/** One JSON-RPC request over loopback HTTP. Returns the parsed envelope. */
function rpc(port, method, params) {
  const id = ++rpcId;
  const body = JSON.stringify({ jsonrpc: "2.0", id, method, params });
  const options = {
    host: HOST,
    port,
    path: "/",
    method: "POST",
    // One fresh socket per request: the server replies `Connection: close`,
    // so disabling the keep-alive pool (default-on since Node 19) avoids
    // reusing a socket the server has already closed ("socket hang up").
    agent: false,
    headers: {
      "Content-Type": "application/json",
      "X-KCreate-MCP-Client": CLIENT,
      "Content-Length": Buffer.byteLength(body),
      Connection: "close",
    },
  };
  return new Promise((resolve, reject) => {
    const req = http.request(options, (res) => {
      let data = "";
      res.setEncoding("utf8");
      res.on("data", (chunk) => {
        data += chunk;
      });
      res.on("end", () => {
        try {
          resolve(JSON.parse(data));
        } catch (e) {
          reject(new Error(`bad JSON-RPC body for ${method}: ${data}`));
        }
      });
    });
    req.on("error", reject);
    req.write(body);
    req.end();
  });
}

/** Call a tool via tools/call; throw on transport or tool error. */
async function callTool(port, name, args) {
  const env = await rpc(port, "tools/call", { name, arguments: args });
  const ok = !env.error && env.result && env.result.isError === false;
  transcript.push({ n: rpcId, method: `tools/call ${name}`, params: args, ok, response: env });
  if (env.error) {
    throw new Error(`tool ${name} JSON-RPC error ${env.error.code}: ${env.error.message}`);
  }
  if (!env.result || env.result.isError !== false) {
    throw new Error(`tool ${name} returned isError: ${JSON.stringify(env.result)}`);
  }
  return env.result.structuredContent;
}

/** Plain JSON-RPC method (discovery / direct). */
async function method(port, name, params) {
  const env = await rpc(port, name, params);
  transcript.push({ n: rpcId, method: name, params, ok: !env.error, response: env });
  if (env.error) throw new Error(`${name} error ${env.error.code}: ${env.error.message}`);
  return env.result;
}

const leaf = (inserted) => inserted.node_ids[0];

async function main() {
  fs.mkdirSync(OUT_DIR, { recursive: true });
  const port = await resolvePort();
  console.log(`[agent] connecting to MCP server at ${HOST}:${port} as "${CLIENT}"`);

  // --- handshake / discovery -------------------------------------------
  const init = await method(port, "initialize", {});
  console.log(
    `[agent] server: ${init.serverInfo?.name ?? "?"} v${init.serverInfo?.version ?? "?"} ` +
      `(protocol ${init.protocolVersion ?? "?"})`,
  );
  const list = await method(port, "tools/list", {});
  const toolNames = (list.tools ?? []).map((t) => t.name);
  console.log(`[agent] ${toolNames.length} tools advertised: ${toolNames.join(", ")}`);

  // Show off discovery: search the elements library.
  const assets = await callTool(port, "list_assets", { query: "chart" });
  console.log(`[agent] list_assets("chart") → ${assets.assets?.length ?? 0} matches`);

  // --- compose a recognizable two-card poster --------------------------
  console.log("[agent] composing a two-card poster…");
  const cardA = await callTool(port, "insert_asset", {
    asset_id: "rounded-rectangle",
    x: 90,
    y: 220,
    target_size: 520,
  });
  await callTool(port, "set_fill", { node_id: leaf(cardA), color: "#4361EE" });
  await callTool(port, "insert_asset", {
    asset_id: "chart-bar",
    x: 190,
    y: 330,
    target_size: 300,
  });

  const cardB = await callTool(port, "insert_asset", {
    asset_id: "rounded-rectangle",
    x: 690,
    y: 220,
    target_size: 520,
  });
  await callTool(port, "set_fill", { node_id: leaf(cardB), color: "#22C55E" });
  await callTool(port, "insert_asset", {
    asset_id: "rocket-illo",
    x: 800,
    y: 320,
    target_size: 320,
  });

  const dotA = await callTool(port, "insert_asset", {
    asset_id: "circle",
    x: 90,
    y: 110,
    target_size: 70,
  });
  await callTool(port, "set_fill", { node_id: leaf(dotA), color: "#F59E0B" });
  const dotB = await callTool(port, "insert_asset", {
    asset_id: "circle",
    x: 180,
    y: 110,
    target_size: 70,
  });
  await callTool(port, "set_fill", { node_id: leaf(dotB), color: "#0EA5E9" });

  const title = await callTool(port, "create_node", {
    node_type: "text",
    name: "poster-title",
  });
  await callTool(port, "set_text", {
    node_id: title.id,
    content: "KCreate · driven by MCP",
  });

  // --- export the composed design --------------------------------------
  console.log("[agent] exporting PNG + SVG via export_design…");
  const png = await callTool(port, "export_design", {
    format: "png",
    path: PNG_PATH,
    options: { width: 1280, height: 800, scale: 1.0, background: [1.0, 1.0, 1.0, 1.0] },
  });
  const svg = await callTool(port, "export_design", {
    format: "svg",
    path: SVG_PATH,
    options: {},
  });

  // --- write the transcript + verify -----------------------------------
  writeTranscript(port);

  const pngBytes = fs.existsSync(PNG_PATH) ? fs.statSync(PNG_PATH).size : 0;
  console.log(`[agent] PNG → ${PNG_PATH} (${pngBytes} bytes, server reported ${png.bytes_written})`);
  console.log(`[agent] SVG → ${SVG_PATH} (server reported ${svg.bytes_written} bytes)`);
  console.log(`[agent] transcript → ${TRANSCRIPT_PATH}`);

  if (pngBytes < 2000) {
    throw new Error(`exported PNG is suspiciously small (${pngBytes} bytes) — expected a real design`);
  }
  console.log("[agent] done — composed and exported a real design entirely over MCP.");
}

function writeTranscript(port) {
  const lines = [];
  lines.push("# KCreate MCP automation — agent transcript");
  lines.push("");
  lines.push(`- Client: \`${CLIENT}\``);
  lines.push(`- Server: \`${HOST}:${port}\` (loopback)`);
  lines.push(`- Calls: ${transcript.length}`);
  lines.push(`- Exported: \`${PNG_PATH}\`, \`${SVG_PATH}\``);
  lines.push("");
  for (const entry of transcript) {
    const status = entry.ok ? "ok" : "ERROR";
    lines.push(`## ${entry.n}. ${entry.method} — ${status}`);
    lines.push("```json");
    lines.push(`// request params`);
    lines.push(JSON.stringify(entry.params, null, 2));
    lines.push(`// response`);
    lines.push(JSON.stringify(entry.response, null, 2));
    lines.push("```");
    lines.push("");
  }
  fs.writeFileSync(TRANSCRIPT_PATH, lines.join("\n"));
}

main().catch((err) => {
  console.error(`[agent] FAILED: ${err.message}`);
  process.exitCode = 1;
});
