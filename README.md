# KCreate

A **local-first**, **workflow-first** design suite — Penpot's UX,
Inkscape's precision, GIMP's raster depth, and Scribus's print-ready
output, all in one desktop app that never asks you to log in.

KCreate runs on macOS, Windows, and Linux. Everything works offline.
Your files stay on your disk. AI runs on your hardware.

---

## Modules

- **Design Studio** — UI, social, posters, multi-artboard layouts.
- **Vector Studio** — logos, icons, illustration; precision Bezier
  tooling and clean SVG.
- **Image Studio** — product cleanup, retouching, background removal,
  non-destructive adjustment layers.
- **Layout Studio** — multi-page docs, decks, brochures, PDF preflight.
- **Brand & Asset Hub** — top-level brand kits with colors, fonts,
  logos, spacing tokens, export presets.
- **Local AI Studio** — manage local models, action history, and
  permissions.
- **Export Center** — PNG, SVG, PDF, WebP, JPEG with batch export and
  presets.
- **MCP / Plugin Hub** — local-loopback MCP server and sandboxed
  plug-ins.

See [`PROPOSAL.md`](./PROPOSAL.md) for the full product spec.

## Core principles

1. **Local-first** — every feature works offline, including AI actions.
2. **Open formats** — native projects are transparent `.kstudio/`
   folders; round-trip with SVG / PNG / PDF / WebP.
3. **Workflow-first** — the launcher asks what you're trying to make,
   not which "tool" you want.
4. **AI as assistant** — every AI action follows
   *Ask → Preview → Apply → Edit → Undo*. Nothing irreversible.
5. **Resource-aware** — adapts to device tier (4 GB laptop up to a
   32 GB workstation).

## Stack

| Layer                | Technology                                                   |
| -------------------- | ------------------------------------------------------------ |
| Application shell    | Electron 33                                                  |
| UI                   | React 18 + TypeScript 5                                      |
| Rust core            | Workspace of cdylib + rlib crates (Rust 1.95)                |
| GPU rendering        | wgpu (Metal / D3D12 / Vulkan / OpenGL)                       |
| CPU rendering        | `tiny-skia` (real software rasterizer, not a placeholder)    |
| Persistence          | SQLite + content-addressed blob store (BLAKE3)               |
| Local AI             | `llama.cpp` (LLM + Vision-LLM + FLUX image-gen) / MLX (Apple Silicon) / ONNX Runtime (loopback sidecars; never network) |
| In-process AI        | Lanczos3 upscale, k-means palette, BFS smart-select, Sobel + CCA screenshot-to-layout, alt-text statistics |
| Vision actions       | design critique, alt-text, brand / palette / spacing extraction, content-aware crop, design-token + style description, smart layer naming (all GBNF-constrained) |
| Vector math          | `kurbo`, `i_overlay`, `rstar`                                |
| Text                 | `fontdb` (discovery) + `rustybuzz` (shaping) + `ttf-parser` (outlines) |
| Export               | `printpdf` (PDF write), `image` (PNG / JPEG / WebP)          |
| Parallelism          | `rayon` (Lanczos rows, parallel batch export, palette downsample) |
| Plugin sandbox       | `wasmi` 0.42 (pure Rust, no LLVM)                            |
| MCP                  | `tiny_http` JSON-RPC over loopback                           |
| LAN collaboration    | `quinn` (QUIC) + `mdns-sd` (mDNS-SD) + `rustls` (TLS) + `tokio` (async runtime; opt-in via `collab` feature on `kcreate_bridge`) |
| Collaboration protocol | Ed25519-signed envelopes, Lamport clocks, LWW + operational CRDT conflict resolution, append-only operation journal, 60-min QUIC cert rotation, per-peer rate limits, ChaCha20-Poly1305 encrypted clipboard share over BLAKE3-derived X25519 session keys |
| KChat Desktop integration | `kcreate_kchat_client`: JSON-RPC 2.0 over Unix domain socket / Windows named pipe to a running `uneycom/uney-chat-desktop` instance; community-gated sessions + member roster sync + conversation-based document sharing (opt-in via `kchat-desktop` feature on `kcreate_bridge`) |
| Brand kit format     | `.kbrand` ZIP archive: `manifest.json` + `fonts/` (TTF/OTF) + `logos/` (PNG/SVG/JPEG) |

See [`ARCHITECTURE.md`](./ARCHITECTURE.md) for the technical design.

## Supported platforms

| Platform | Architectures | Notes |
| --- | --- | --- |
| macOS  | Intel + Apple Silicon | Metal backend on wgpu; MLX vision/image-gen sidecars available on Apple Silicon. |
| Windows | x64 | D3D12 backend on wgpu. |
| Linux  | x64 + arm64 | Vulkan backend on wgpu (mesa ≥ 22 or proprietary drivers). `fontconfig` is required at runtime for system-font discovery; the build script vendors `fontconfig-sys`. **Wayland support: graceful fallback** — if the compositor refuses a window surface (no XDG/X11 bridge), the renderer transparently switches to its offscreen path so the canvas still renders. X11 (`xcb`) is the recommended display server for end users. |

CI runs the test suite on all three platforms (`.github/workflows/ci.yml`).

## Design system

KCreate uses the KChat token set:

- Primary accent: `#7C3AED`
- Font: `Inter`, system fallback
- Background: `#FFFFFF` page, `#F5F3FF` card surfaces
- Cards: white background, `border-radius: 12px`, subtle shadow
- Buttons: pill (`border-radius: 9999px`)

## Quick start

### Prerequisites

- Rust 1.95+ (the workspace pins `1.95.0` via `rust-toolchain.toml`).
- Node.js 20.11+ (the package pins `>=20.11.0`).
- `pnpm` 10+.
- C toolchain (clang or gcc).
- Platform-specific GPU dependencies (see
  [`CONTRIBUTING.md`](./CONTRIBUTING.md)).

### Install

```bash
pnpm install
cargo check --workspace --all-targets
```

### Build

```bash
pnpm build           # builds main + preload + renderer
cargo build --workspace
```

To build with LAN collaboration support (QUIC + mDNS transport), add
the `collab` feature flag on `kcreate_bridge`:

```bash
cargo build --workspace --features kcreate_bridge/collab
```

This pulls `quinn`, `rustls`, `mdns-sd`, and `tokio` into the bridge
but still keeps the editing path crates network-free (the
`crates/kcreate_tests/tests/local_first.rs` deny-list enforces this).

To additionally enable the real KChat Desktop integration (sources
membership attestations from a running `uneycom/uney-chat-desktop`
instance over local IPC), enable `kchat-desktop` as well:

```bash
cargo build --workspace --features kcreate_bridge/kchat-desktop
```

`kchat-desktop` implies `collab`. The dev-only `kchat-dev-issuer`
flag — used by the integration tests to mint test attestations
without a running uney-chat-desktop — remains available.

Run the Phase 7 collab performance benchmarks (criterion):

```bash
cargo bench -p kcreate_bridge --features collab --bench collab_perf
```

### Test

```bash
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
pnpm typecheck
pnpm lint
```

### Run

```bash
# Terminal 1: Vite dev server for the renderer
pnpm --filter @kcreate/desktop dev:renderer

# Terminal 2: build native, then start Electron
cargo build --workspace
KCREATE_DEV_RENDERER_URL=http://localhost:5173 pnpm --filter @kcreate/desktop start
```

## Repository layout

```
KCreate/
├── apps/
│   └── desktop/                  Electron shell
│       ├── main/                 main process (loads Rust bridge)
│       ├── preload/              contextBridge to renderer
│       ├── renderer/             React + Vite UI
│       └── shared/               wire-format types
├── crates/
│   ├── kcreate_core/             Shared types, node model, document graph, ops, config, components
│   ├── kcreate_renderer/         Offscreen wgpu + tiny-skia fallback + native swapchain surface
│   ├── kcreate_bridge/           N-API bindings (cdylib)
│   ├── kcreate_vector/           Path math, boolean ops, SVG, R-tree
│   ├── kcreate_storage/          SQLite + BLAKE3 blobs + .kstudio I/O
│   ├── kcreate_export/           PNG / SVG / PDF / WebP / JPEG export + batch + inspect code-gen
│   ├── kcreate_raster/           Tile engine, masks, adjustment layers
│   ├── kcreate_text/             Font discovery (fontdb) + shaping (rustybuzz)
│   ├── kcreate_layout/           Pure flex + grid solvers
│   ├── kcreate_ai/               Local AI: bg-removal (threshold + ONNX u2net), LLM sidecar,
│   │                              Lanczos upscale, k-means palette, BFS smart-select, model
│   │                              pack registry, screenshot-to-layout, multimodal chat,
│   │                              vision sidecar + MLX sidecar dispatcher, FLUX image-gen
│   │                              sidecar, design critique / brand / crop / token / style
│   ├── kcreate_mcp/              Loopback-only MCP server (3 tools) + permission store
│   │                              (Once / Always / Denied)
│   ├── kcreate_plugin/           WASM plugin sandbox (wasmi 0.42, deny-by-default host ABI;
│   │                              Phase 2 extended ABI + Ed25519 manifest signing +
│   │                              JS panel runtime)
│   ├── kcreate_collab/           Phase 3 collaboration protocol foundation (peer identity,
│   │                              Lamport clock, signed envelopes, conflict resolver,
│   │                              project session). Kept OUT of editing-path deps.
│   ├── kcreate_collab_transport/ QUIC + mDNS LAN transport (peer discovery, ephemeral
│   │                              cert pinning). Only networked crate; opt-in via
│   │                              `collab` feature on kcreate_bridge.
│   ├── kcreate_kchat/            Dev-side KChat group-membership issuer (test attestations
│   │                              against deterministic Ed25519 keys). Behind
│   │                              `kchat-dev-issuer` feature flag.
│   ├── kcreate_kchat_client/     Phase 7 local-IPC client for the real KChat Desktop
│   │                              (`uneycom/uney-chat-desktop`). JSON-RPC 2.0 over Unix
│   │                              socket / named pipe — never the internet. Behind the
│   │                              `kchat-desktop` feature flag on kcreate_bridge.
│   ├── kcreate_audit/            Append-only audit trail for operations + AI actions +
│   │                              collab lifecycle events. Separate SQLite DB so audit
│   │                              history survives project close/delete.
│   └── kcreate_tests/            Cross-crate integration tests
├── tools/
│   └── kcreate_diffusion/        Loopback Python diffusion sidecar (FLUX.2-Klein-4B,
│                                  diffusers; spawned by `image_gen.rs`, never networked)
├── PROPOSAL.md                   Product specification
├── ARCHITECTURE.md               Technical architecture
├── PROGRESS.md                   Phase tracking
├── CONTRIBUTING.md               Contributor guide
├── SECURITY.md                   Security policy
├── AGENTS.md                     Notes for AI coding agents
├── Cargo.toml                    Workspace manifest
├── package.json                  pnpm root
└── .github/workflows/ci.yml      CI (Linux + macOS + Windows)
```

## Collaboration & KChat Desktop integration

Phase 7 connects KCreate's LAN collaboration stack to the real
KChat Desktop client (`uneycom/uney-chat-desktop`) running on
the same machine. The integration is opt-in through the
`kchat-desktop` feature flag on `kcreate_bridge` and stays
local-first end-to-end — the editing-path closure walked by
`crates/kcreate_tests/tests/local_first.rs` is still
network-free; the IPC client links `tokio::net::UnixStream`
(Unix) / `tokio::net::windows::named_pipe` (Windows) and never
the internet.

### What it adds

- **Community-gated sessions.** A KCreate collab session is
  bound to a uney-chat-desktop community. LAN peers in
  different communities cannot discover each other via mDNS
  (the community id is folded into the service TXT record).
- **Real-time peers.** Coloured cursor + selection overlays
  show every connected peer in the current viewport. Conflict
  resolution surfaces a non-blocking toast with an undo link.
  Late joiners receive the full operation journal via a
  `ResumeBundle` so the document state converges immediately.
- **Document sharing through KChat conversations.** "Share
  document" posts a rich invite card (project id + owner
  identity + cert fingerprint + community id) into a channel.
  Recipients accept the invite and KCreate dials the owner
  peer directly over QUIC.
- **Role-based permissions.** Community owner/admin → editor
  with kick + ACL-manage privileges. Community member →
  editor (host-downgradable to viewer). Viewer = read-only.
- **Security hardening.** 60-minute QUIC cert rotation,
  per-peer rate limits (100 ops/s + 20 presence/s),
  ChaCha20-Poly1305 encrypted clipboard share over a
  BLAKE3-derived X25519 session key, per-project ACL
  (`<project_dir>/acl.json`), full audit trail in the
  `kcreate_audit` separate-DB store.
- **Performance.** 50 ms / 200-op outbound batching, 20 Hz
  presence throttling with a 2 px delta floor and 2 s idle
  suppression, selective per-page sync for multi-page docs.

### Quick start

Start two KCreate instances on the same LAN with the
`kchat-desktop` feature enabled and a running
uney-chat-desktop on each machine. The bridge auto-detects
the local socket at `~/.kchat/kcreate.sock` (Unix) /
`\\.\pipe\kchat-kcreate` (Windows) and asks the user to pick
one of their communities; selecting a community installs the
KChat-signed attestation in the collab gate and unlocks every
multiplayer entry point.

The IPC protocol that KCreate speaks to uney-chat-desktop is
documented end-to-end in
[`crates/kcreate_kchat_client/src/protocol_spec.md`](./crates/kcreate_kchat_client/src/protocol_spec.md);
this is the contract the uney-chat-desktop team implements on
the server side.

See [`ARCHITECTURE.md` § KChat Desktop Integration](./ARCHITECTURE.md)
for the design.

## License

[AGPL-3.0-or-later](./LICENSE).
