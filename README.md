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

See [`ARCHITECTURE.md`](./ARCHITECTURE.md) for the technical design.

## Platforms

- macOS (Intel and Apple Silicon)
- Windows (x64)
- Linux (x64 and arm64)

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

## License

[AGPL-3.0-or-later](./LICENSE).
