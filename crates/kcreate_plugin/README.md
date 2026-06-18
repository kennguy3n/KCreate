# kcreate_plugin

Sandboxed plugin runtime for KCreate. Plugins are arbitrary WebAssembly
modules that run inside a [wasmi](https://crates.io/crates/wasmi) sandbox
with **no** filesystem, network, clock, or environment access — only a
small set of host functions exported into the `env` module.

This crate ships three things:

1. The manifest schema (`manifest.json`) and a content-addressed
   registry that scans a directory of plugins.
2. The WASM runtime that loads, links, and executes a plugin against
   a strict ABI.
3. The extended ABI that lets *permission-gated* plugins read
   the live document, fetch assets, and propose mutations that the host
   validates and applies as recordable, undoable operations.

The bridge layer (`kcreate_bridge::phase2::plugin_execute_with_context`)
is what users actually call from the editor; this crate is what makes
it possible.

---

## 1. Plugin layout

```
my-plugin/
├── manifest.json
└── entry.wasm        # or anything you point `entry_point` at
```

### `manifest.json`

```jsonc
{
  "id":          "my-plugin",            // unique among installed plugins
  "name":        "My Plugin",            // human-readable
  "version":     "0.1.0",                // semver-ish
  "author":      "you@example.com",      // optional
  "description": "Counts nodes",         // optional, shown in plugin manager
  "type":        "wasm",                 // wasm | js_panel | native
  "entry_point": "entry.wasm",           // relative to plugin dir
  "permissions": ["read_document"]       // see permission table below
}
```

The registry refuses to load a plugin whose `entry_point` file is
missing or whose `id` / `name` / `version` is empty.

### Permission table

| Permission       | Grants access to                                          |
| ---------------- | --------------------------------------------------------- |
| `read_document`  | `kcreate_read_document` (document graph queries)          |
| `read_assets`    | `kcreate_read_asset` (BLAKE3-addressed blob store)        |
| `write_document` | `kcreate_write_proposal` (queue mutations for host apply) |
| `export_files`   | reserved                                                  |
| `network_access` | reserved (denied — the editing path is network-free)      |

Permissions are **declared** in the manifest and **enforced** at the
host-function call site. A plugin that calls
`kcreate_write_proposal` without `write_document` in its manifest gets
back `0` and a single line in the log buffer:
`kcreate_write_proposal: denied (missing WriteDocument)`. It is not
killed — it just can't do the thing.

---

## 2. Entry point requirements

The runtime drives plugins by exported function name. There is no
`_start`-like main; the host calls a specific exported function and
that function does its work and returns. The function must:

* be exported by name (e.g. `(export "run")`),
* take no parameters,
* return no values.

Pick whatever name you want. The bridge call is
`plugin_execute(id, function, input)` — the second argument is the
exported function name.

You must also export a `memory` instance:

```wat
(memory (export "memory") 1)
```

The runtime grows your memory on demand up to the page limit it passes
to `execute` (default `64` pages = 4 MiB). Going past the limit makes
`memory.grow` return `-1`; it does not kill the plugin.

---

## 3. Host ABI

All host functions live in the `env` module namespace. The basic ABI
is always available; the extended ABI is wired only when the bridge
runs the plugin under a `PluginContext`
(`plugin_execute_with_context`).

### 3.1 Basic ABI

```wat
(import "env" "kcreate_log"           (func (param i32 i32)))
(import "env" "kcreate_get_input_len" (func (result i32)))
(import "env" "kcreate_get_input"     (func (param i32 i32) (result i32)))
(import "env" "kcreate_set_output"    (func (param i32 i32)))
```

* **`kcreate_log(ptr, len)`** — copy `len` bytes from plugin memory at
  `ptr` into the host log buffer. Invalid UTF-8 is replaced with
  `U+FFFD`.
* **`kcreate_get_input_len() -> i32`** — length in bytes of the input
  JSON the host passed to `execute()`. Use this to size the buffer
  before calling `kcreate_get_input`.
* **`kcreate_get_input(ptr, max_len) -> i32`** — copy up to `max_len`
  bytes of the input JSON into plugin memory at `ptr`. Returns the
  number of bytes written (truncated if `max_len` was too small).
* **`kcreate_set_output(ptr, len)`** — copy `len` bytes from plugin
  memory at `ptr` into the host output buffer. The **last** call wins;
  callers can write incrementally and overwrite.

### 3.2 Extended ABI (permission-gated)

```wat
(import "env" "kcreate_read_document"  (func (param i32 i32) (result i32)))
(import "env" "kcreate_read_asset"     (func (param i32 i32 i32 i32) (result i32)))
(import "env" "kcreate_write_proposal" (func (param i32 i32) (result i32)))
```

These functions resolve only when the host runs the plugin via
`execute_with_context`. A plugin that imports them but runs through
the legacy `execute` path will fail to **link**, which surfaces as
`WasmPluginError::Wasm(...)` from the bridge. If your plugin doesn't
need the extended ABI, just don't import the functions.

#### `kcreate_read_document(ptr, len) -> i32`

Reads a JSON-encoded query from plugin memory at `[ptr, ptr+len)`,
resolves it against an immutable snapshot of the document, writes the
JSON-encoded response into the plugin's **output** buffer, and returns
the response byte length. Returns `0` on permission deny or on an
unparseable / unknown query.

The response lands in the same buffer `kcreate_set_output` writes
into. Read it back with `kcreate_get_input`-style logic against your
own buffer — or just snapshot it as the final output value if you
want the host to see the result verbatim.

Query shapes (`#[serde(tag = "type", rename_all = "snake_case")]`):

```jsonc
// List every node id in arbitrary order.
{ "type": "list_nodes" }
// → JSON array of UUID strings: ["abc...", "def...", ...]

// Fetch a single node by id.
{ "type": "get_node", "id": "60024594-34e1-42a6-912d-8006831bbfeb" }
// → full node JSON object, or `null` if the id doesn't resolve.

// Fetch the root node (handy for tree walks).
{ "type": "get_root" }
// → root node JSON object, or `null` if no document is open.
```

Gated by `PluginPermission::ReadDocument`.

#### `kcreate_read_asset(hash_ptr, hash_len, buf_ptr, buf_len) -> i32`

Reads `hash_len` bytes of the asset's BLAKE3 hex hash from plugin
memory at `hash_ptr`, looks the blob up in the content-addressed
store, and copies up to `buf_len` bytes into `buf_ptr`. Returns the
number of bytes written, or `0` if the permission was denied, the
hash didn't resolve, or `buf_len < blob.len()`.

Gated by `PluginPermission::ReadAssets`.

#### `kcreate_write_proposal(ptr, len) -> i32`

Submits a JSON-encoded mutation proposal from plugin memory.
Proposals are **queued** here — the host doesn't apply them until the
plugin returns. After return, the bridge validates each proposal
(node existence for `update_node` / `delete_node`, parent existence
for `create_node`) and applies the surviving ones through the
recorded document operations API so the changes appear in undo/redo.

Return values:

* `1` — proposal parsed and queued. Host will validate after the
  plugin returns.
* `0` — permission denied, JSON parse failure, or unknown shape. A
  single denial line lands in the log buffer.

Mutation shapes (`#[serde(tag = "type", rename_all = "snake_case")]`):

```jsonc
{ "type": "create_node",
  "parent_id": "<uuid>",
  "node_type": "text_layer",   // any node_type the bridge accepts
  "props":     { "name": "Created by plugin" } }

{ "type": "update_node",
  "node_id": "<uuid>",
  "changes": { "name": "Renamed by plugin" } }

{ "type": "delete_node",
  "node_id": "<uuid>" }
```

Gated by `PluginPermission::WriteDocument`.

---

## 4. Return envelope

`plugin_execute_with_context` returns JSON:

```jsonc
{
  "output":    "<whatever the plugin set>",
  "logs":      ["one", "line", "per", "kcreate_log call"],
  "proposals": [
    { "type": "delete_node",
      "node_id": "60024594-...",
      "outcome": { "status": "applied", "node_id": "60024594-..." } },
    { "type": "delete_node",
      "node_id": "00000000-...",
      "outcome": { "status": "rejected", "reason": "node not found" } }
  ]
}
```

`outcome.status` is `"applied"` or `"rejected"`. Applied proposals
land in the operation log; rejected proposals are reported to the
caller and never touch the document.

---

## 5. Examples

The `examples/` directory holds three reference plugins in WAT
form. They are not test fixtures — they are the smallest end-to-end
illustrations of each ABI surface. Each one has a sibling
`manifest.json` so it can be dropped into the plugin directory and
loaded by the registry.

* `examples/hello/` — uses **only** the basic ABI. Logs
  `"Hello from WASM"` and sets the output to a fixed string.
* `examples/node_counter/` — declares `read_document`, calls
  `kcreate_read_document` with `{"type":"list_nodes"}`, and sets the
  output to the JSON array the host returns. The renderer-side
  caller is expected to `.length` the array client-side; the plugin
  just hands the result through verbatim.
* `examples/auto_rename/` — declares `read_document` and
  `write_document`, walks the node list, and proposes one
  `update_node` per untitled node renaming it to `"Renamed"`.

To compile a `.wat` to `.wasm` use any standard tool:

```bash
# wabt
wat2wasm hello.wat -o hello.wasm

# wasm-tools
wasm-tools parse hello.wat -o hello.wasm
```

(There is no build step inside this crate — the runtime consumes the
final `.wasm` bytes, not WAT. Tests do compile WAT inline via the
`wat` crate, but production plugins ship pre-compiled.)

---

## 6. Security notes

* **Deny by default.** A plugin with no `permissions` declared in its
  manifest cannot read the document, read assets, or write proposals.
  It can only log and round-trip its input through output.
* **Proposals, not direct writes.** Even with `write_document`, a
  plugin cannot mutate the document. It hands the host a list of
  proposed mutations; the host validates each one (parent/node
  existence) and applies the survivors through the same code path the
  UI uses, so every applied change is recorded in the operation log
  and shows up in undo.
* **No clock, no network, no filesystem.** Those host functions are
  simply not exported. The plugin cannot import what isn't there.
* **Memory limit.** Plugins are capped at 64 4-KiB pages by default
  (the bridge passes this; you can tune it via `execute_with_context`'s
  `memory_limit_pages` argument). `memory.grow` requests beyond the
  limit return `-1` to the plugin; the host stays up.

If you find a way to escape the sandbox, please follow the disclosure
process in `SECURITY.md` at the repo root.
