# 10 — Extend and automate without the cloud

A design tool becomes a platform when other people — and other
programs — can safely build on it. KCreate opens up two extension
surfaces, and both honor the same rule as the rest of the app: they
run **locally**, sandboxed, with no implicit access to your machine or
the network. You can extend KCreate with plugins and automate it with
an AI agent, all without anything leaving your device.

## Plugins: sandboxed WASM, deny-by-default

KCreate plugins are **WebAssembly modules** run in a sandbox
([`crates/kcreate_plugin/`](../crates/kcreate_plugin/)) built on
`wasmi`. The security model is deny-by-default: a plugin gets a small,
explicit host ABI and **nothing else** — no filesystem, no network, no
DOM. The host functions it can call are deliberately minimal:

- a logging call,
- input/output passing (`kcreate_get_input`, `kcreate_set_output`),
- and an extended document ABI for real design work:
  `kcreate_read_document`, `kcreate_read_asset`, and
  `kcreate_write_proposal`.

That last one matters: a plugin doesn't mutate your document directly —
it **writes a proposal** that flows through the same operation log as
everything else, so a plugin's changes are previewable and **undoable**
just like an AI action (Part 08). A page-count `ResourceLimiter` caps
how much memory a plugin can allocate, so a misbehaving module can't
exhaust your machine.

## Trust you can verify: signed manifests

Plugins carry **Ed25519-signed manifests**. Signing means you can
verify a plugin is what it claims to be and hasn't been tampered with,
and the registry persists which plugins you've enabled to disk. The
trust decision is yours and it's explicit — installing and enabling a
plugin is a deliberate act, surfaced in the plugin panel, not something
that happens silently in the background.

## Automation: a loopback MCP server an agent can drive

KCreate also speaks **MCP (Model Context Protocol)** over a
**loopback-only** server ([`crates/kcreate_mcp/`](../crates/kcreate_mcp/)),
so an external AI agent on your machine can drive the app
programmatically — list what's on the canvas, create nodes, and export
artboards — to automate repetitive production work. Because it binds to
`127.0.0.1` only, the automation surface is reachable by tools on your
machine and **nothing else**.

Every MCP capability is **permission-gated**. A
`McpPermissionStore` mediates each tool call with an explicit
**Once / Always / Denied** decision, persisted to disk
([`crates/kcreate_mcp/src/`](../crates/kcreate_mcp/src/) +
the MCP settings panel in the renderer). An agent can't quietly take
actions you haven't authorized — you grant access deliberately, per
tool, and can revoke it.

## Why this serves the job

Extension and automation are how a tool grows past its built-in
features without growing past your control. Whether it's a teammate's
plugin or an AI agent batch-producing a hundred variations, KCreate
keeps the same promises: sandboxed, explicit-permission, undoable, and
local. You get the leverage of a platform without surrendering the
ownership and privacy that make KCreate local-first in the first place.

## How this compares

- **Figma**'s plugin ecosystem is large and powerful, but plugins run
  with broad capabilities and the platform is cloud-anchored. KCreate's
  WASM sandbox is deny-by-default, signed, and routes changes through an
  undoable proposal log.
- **Canva** apps are cloud-hosted and reviewed by Canva; KCreate
  plugins run locally on your machine under your control.
- **MCP automation** of a design tool — letting a local AI agent drive
  the app under per-tool permissions — is something the cloud incumbents
  don't offer in this form, because it presumes the tool and the agent
  share *your* machine.

---

**Trace it in the code**

- WASM plugin sandbox + host ABI + signed manifests: [`crates/kcreate_plugin/`](../crates/kcreate_plugin/)
- Loopback MCP server + tools: [`crates/kcreate_mcp/`](../crates/kcreate_mcp/) (`src/tools.rs`)
- MCP permission store (Once / Always / Denied): [`crates/kcreate_mcp/src/`](../crates/kcreate_mcp/src/)
- Plugin / MCP UI panels: [`apps/desktop/renderer/src/components/`](../apps/desktop/renderer/src/components/)

Previous: [« 09 — Print-ready and developer-ready output](./part-09-print-and-dev-ready-output.md) ·
Back to the [series index »](./README.md)
