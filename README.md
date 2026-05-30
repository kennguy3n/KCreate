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
| Local AI             | `llama.cpp` (LLM + Vision-LLM) + `stable-diffusion.cpp` (FLUX image-gen) + ONNX Runtime (loopback sidecars; never network; zero Python) |
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
| KChat backend integration | `kcreate_kchat_client`: HTTPS REST (`reqwest` + `rustls`) to the shared KChat / Mattermost backend that `uneycom/uney-chat-desktop` also signs in to; community-gated sessions + member roster sync + conversation-based document sharing (opt-in via `kchat-backend` feature on `kcreate_bridge`). A thin `.kcz` companion extension (`apps/kchat-extension/`) renders a sidebar inside KChat Desktop and bridges deeplinks back to KCreate. |
| Brand kit format     | `.kbrand` ZIP archive: `manifest.json` + `fonts/` (TTF/OTF) + `logos/` (PNG/SVG/JPEG) |
| Import edges (Phase 9) | `psd` (Adobe PSD layered import), `kamadak-exif` (JPEG / WebP EXIF round-trip), `resvg` (SVG-to-raster preview + thumbnail rasterisation) |
| Robustness (Phase 9) | Memory-pressure watchdog (`kcreate_bridge::perf::memory_watchdog_start`, `sysinfo`-backed), opt-in autosave + crash recovery (`kcreate_bridge::autosave`), export pre-flight validation (`kcreate_export::validate`) |
| Image Studio AI (Phase 10) | Non-local-means denoise (`kcreate_ai::denoise`), PatchMatch exemplar inpaint (`kcreate_ai::inpaint`), histogram + gray-world auto colour (`kcreate_ai::auto_color`), magic-wand selection tool (`kcreate_ai::smart_select` + `MagicWandTool.tsx`), SAM segmentation tool with smart-select fallback |
| Vector / Layout / Brand AI (Phase 10) | Stroke-style match (`kcreate_ai::stroke_match`), glyph extraction from photo (`kcreate_ai::glyph_extract`), LLM-driven reformat-to-deck (`kcreate_ai::reformat`) + brief-to-one-pager (`kcreate_ai::one_pager`) + palette harmonisation (`kcreate_ai::palette_harmonize`) + type pairing (`kcreate_ai::type_pairing`) + brand-to-brochure (`kcreate_ai::brand_template`) |
| Export AI (Phase 10) | Element-aware SVG optimiser with protected `<text>` / `<style>` / CDATA regions (`kcreate_export::svg_optimize`), SSIM-targeted smart-compress for raster (`kcreate_export::smart_compress`), live export preview (`ExportPreviewPanel.tsx`), AI/Illustrator subset import (`kcreate_export::ai_import`), multi-page PDF with TOC / outline / hyperlinks / per-page subset fonts (`kcreate_export::pdf_multi`) |
| Plugin ecosystem (Phase 10) | Local plugin marketplace (`kcreate_plugin::marketplace`, `PluginManager.tsx` "Marketplace" tab) with Ed25519 signature verification on install |
| Performance & robustness (Phase 10) | Incremental scene diff with per-node `scene_version` + `DirtySet<Uuid>` (`kcreate_bridge::scene_sync`), undo-log delta compression + BLAKE3 blob-ref swapping (`kcreate_core::operation_compress`, configurable via `RuntimeConfig::{compress_undo_log, undo_blob_threshold_bytes}`), startup lazy-init for tile cache / LLM sidecar / memory watchdog with `bridge.<subsystem>.subsystem_ready` startup-timeline marks |
| Render performance (Phase 11) | Incremental scene sync with `DocumentGraph::drain_dirty` + cached per-node `Vec<Object>` lists (`kcreate_bridge::scene_sync`), BLAKE3 content-addressed image fingerprints on `ObjectKind::Image` (8 bytes hashed per frame vs walking a 48 MB pixel buffer), R-tree spatial index for hit testing (`kcreate_core::document::DocumentGraph` + `kcreate_bridge::hit_test`), batched `FillRect` / `StrokeRect` display-list commands (`kcreate_renderer::pipeline::DisplayCommand::BatchedRects`) |
| GPU compute filters (Phase 11) | Two-pass separable Gaussian blur, 256-entry-LUT levels / curves, and reuse-the-blur unsharp mask compute pipelines (`crates/kcreate_renderer/src/compute/{gaussian_blur,levels_curves,unsharp_mask}.wgsl`) sharing `wgpu::Device` + `Queue` with `GpuBackend`; CPU fallback when no adapter is available |
| Async bridge (Phase 11) | Raster filter family (`raster_apply_blur` / `_sharpen` / `_levels` / `_curves` / `_hsl` / `_color_balance` / `_perspective` / `_apply_filter_masked` / `raster_crop`), export operations (`export_png`, `export_pdf`, `export_svg_async`), and `project_save` run as `napi::AsyncTask`s; `project_save` snapshots inside the write guard so concurrent edits can't corrupt the save |
| Prototype animation (Phase 11) | `Transition { AnimationType, duration_ms, EasingCurve (incl. cubic-bezier + spring), SlideDirection }`, hover / press / `MouseEnter` / `MouseLeave` / `AfterDelay` triggers, `SwitchVariant` Smart-Animate name-matched interpolation (bounds / opacity / HSL fill / corner radius), auto-layout propagation through component instances with depth-bounded recursion + override-aware solvers (`apps/desktop/renderer/src/lib/EasingEngine.ts`, `apps/desktop/renderer/src/components/PrototypePlayer.tsx`, `kcreate_layout::flex::layout_flex_with_overrides`, `kcreate_layout::grid::layout_grid_with_overrides`) |
| Workspace concurrency (Phase 11) | `RwLock<Option<Workspace>>` in `kcreate_bridge::document` (audited read/write call sites), per-node `version: u64` + `document_version: AtomicU64` for lock-free MVCC polling, delta-compressed operations (`kcreate_core::operation_compress::OperationDelta`), lazy subsystem init for tile cache / LLM sidecar / memory watchdog / audit DB / collab transport / `fontdb` (background scan), 10 000-node scale validation |
| Security hardening (Phase 11) | Per-session 32-byte bearer-token-authenticated LLM sidecar (`kcreate_ai::llm_sidecar` + `--api-key`), TOCTOU port-allocation fix via post-spawn `GET /v1/models` verification handshake, ChaCha20-Poly1305 ACL encryption with auto-migration of plaintext on encrypted projects (`kcreate_collab::acl::{encrypt_acl_bytes,decrypt_acl_bytes}` + `KCAClv1` magic + 12-byte nonce wire format), KChat REST certificate pinning via custom `rustls::ServerCertVerifier` chaining the Mozilla WebPKI root store with a constant-time leaf-cert SHA-256 fingerprint check (`kcreate_kchat_client::pinning`) |

See [`ARCHITECTURE.md`](./ARCHITECTURE.md) for the technical design.

## Supported platforms

| Platform | Architectures | Notes |
| --- | --- | --- |
| macOS  | Intel + Apple Silicon | Metal backend on wgpu; llama.cpp + sd.cpp ship with native Metal acceleration on Apple Silicon (no MLX or Python required — see Phase 12). |
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

To additionally enable the real KChat backend integration
(sources membership attestations over HTTPS REST from the shared
KChat / Mattermost backend that `uneycom/uney-chat-desktop` also
signs in to), enable `kchat-backend` as well:

```bash
cargo build --workspace --features kcreate_bridge/kchat-backend
```

`kchat-backend` implies `collab`. The dev-only `kchat-dev-issuer`
flag — used by the integration tests to mint test attestations
without standing up a real backend — remains available.

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
│   │                              vision sidecar dispatcher (llama-server), sd.cpp image-gen
│   │                              sidecar (`diffusion_sidecar.rs`), design critique / brand / crop / token / style
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
│   ├── kcreate_kchat_client/     Phase 7 KChat backend REST client. HTTPS-only
│   │                              (`reqwest` + `rustls`) against the shared KChat /
│   │                              Mattermost backend that `uneycom/uney-chat-desktop`
│   │                              also signs in to. Behind the `kchat-backend`
│   │                              feature flag on kcreate_bridge; kept OUT of the
│   │                              editing-path dep tree so the local-first
│   │                              sentinel stays green.
│   ├── kcreate_audit/            Append-only audit trail for operations + AI actions +
│   │                              collab lifecycle events. Separate SQLite DB so audit
│   │                              history survives project close/delete.
│   └── kcreate_tests/            Cross-crate integration tests
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

## Collaboration & KChat backend integration

Phase 7 connects KCreate's LAN collaboration stack to the
shared KChat / Mattermost backend that `uneycom/uney-chat-desktop`
also signs in to (the **Option C** integration shape). The
integration is opt-in through the `kchat-backend` feature flag
on `kcreate_bridge` and stays local-first for the editing path:
the closure walked by
`crates/kcreate_tests/tests/local_first.rs` excludes
`kcreate_kchat_client` even though it links `reqwest` and
`rustls`. The two desktop apps never speak a peer-to-peer
Electron IPC — each one independently authenticates with the
shared backend over HTTPS REST, and a thin `.kcz` companion
extension (`apps/kchat-extension/`) renders a sidebar inside
KChat Desktop that bridges deeplinks back to KCreate.

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
`kchat-backend` feature enabled. Each instance signs in to the
shared KChat / Mattermost backend through the
`KChatSignInPanel` (server URL + credentials), picks one of
their communities, and the bridge fetches a signed membership
attestation from the backend and installs it in the collab
gate. Multiplayer entry points unlock once the attestation is
live. Drop the dev-only `kchat-dev-issuer` flag in for
integration tests — it mints deterministic attestations
without requiring a running backend.

The REST surface KCreate talks to is documented in
[`ARCHITECTURE.md` § KChat backend integration](./ARCHITECTURE.md);
the matching `.kcz` companion extension that ships inside
KChat Desktop lives in
[`apps/kchat-extension/`](./apps/kchat-extension/).

## License

[AGPL-3.0-or-later](./LICENSE).
