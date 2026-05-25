# KCreate — agent guide

This file is for AI coding agents (Devin, Codex, etc.). Human contributors
should read `README.md`, `PROPOSAL.md`, `ARCHITECTURE.md`, and `PROGRESS.md`
first.

## Layout

```
KCreate/
├── crates/
│   ├── kcreate_core/        shared types, document graph, operation log,
│   │                        project model, device/runtime config (no GPU,
│   │                        no napi — safe to depend on anywhere)
│   ├── kcreate_renderer/    pure Rust: offscreen wgpu pipeline + CPU
│   │                        fallback (tiny-skia) + presenter
│   ├── kcreate_bridge/      napi-rs cdylib (renderer + document + export +
│   │                        phase2 IPC). Logic in state.rs / document.rs /
│   │                        phase2.rs; lib.rs is a thin N-API marshalling
│   │                        layer. native_canvas.rs lives behind the
│   │                        `native_canvas` feature flag (only place in
│   │                        the tree where `unsafe_code` is allowed).
│   ├── kcreate_vector/      path math, boolean ops (i_overlay), SVG
│   │                        import (usvg), SVG export, R-tree spatial
│   │                        index (rstar)
│   ├── kcreate_storage/     SQLite (rusqlite, bundled), content-addressed
│   │                        BLAKE3 blob store, .kstudio/ project I/O
│   ├── kcreate_export/      PNG / SVG / PDF / WebP / JPEG export, batch
│   │                        driver (parallel + async cancel), PDF
│   │                        preflight, icon pack generator, inspect-mode
│   │                        code-gen (CSS / Tailwind / React)
│   ├── kcreate_raster/      tile engine, masks, adjustment layers (Phase 1)
│   ├── kcreate_text/        font discovery (fontdb), shaping (rustybuzz),
│   │                        outline walking (ttf-parser) → renderer paths
│   ├── kcreate_ai/          local AI task router, action log, bg removal
│   │                        (threshold + ONNX u2net), LLM sidecar lifecycle
│   │                        (`llm_sidecar.rs`), loopback chat (`llm_chat.rs`),
│   │                        Lanczos3 upscale, k-means palette extraction,
│   │                        BFS flood-fill smart-select, model pack
│   │                        registry, screenshot-to-layout (edge detect +
│   │                        connected components + heuristics)
│   ├── kcreate_layout/      pure flex + grid solvers (no DOM, no side effects)
│   ├── kcreate_mcp/         loopback-only MCP server (tiny_http JSON-RPC,
│   │                        3 tools: list_artboards, create_node, export_artboard;
│   │                        gated by `permissions::McpPermissionStore` —
│   │                        Once / Always / Denied, JSON on-disk)
│   ├── kcreate_plugin/      WASM plugin sandbox (wasmi 0.42, deny-by-default
│   │                        host ABI: kcreate_log, kcreate_get_input{,_len},
│   │                        kcreate_set_output, plus the Phase 2 extended
│   │                        ABI: kcreate_read_document, kcreate_read_asset,
│   │                        kcreate_write_proposal). Page-count
│   │                        ResourceLimiter; no FS / network / DOM access.
│   │                        Manifest + registry persist enabled state to
│   │                        JSON. Ed25519 manifest signing + JS panel
│   │                        runtime live here too (Phase 2 PR #7).
│   ├── kcreate_collab/      Phase 3 collaboration protocol foundation
│   │                        (peer identity, Lamport clock, Ed25519-signed
│   │                        envelopes, Hello/Welcome/Op/Presence/Heartbeat/
│   │                        Goodbye messages, LWW conflict resolver,
│   │                        ProjectSession w/ per-peer nonce replay window).
│   │                        Transport-agnostic; deliberately OUT of the
│   │                        editing-path dependency tree so a future
│   │                        transport (QUIC + mDNS) can pull in network
│   │                        crates without breaking the local-first
│   │                        sentinel.
│   ├── kcreate_collab_transport/ QUIC + mDNS LAN transport. `LanCollabHost`,
│   │                        `PeerDiscovery`, `CertBundle`, frame wire codec.
│   │                        Only networked crate in the workspace; opt-in
│   │                        via `collab` feature on kcreate_bridge.
│   ├── kcreate_kchat/       Dev-side KChat group-membership issuer.
│   │                        Deterministic Ed25519 derivation + signed-
│   │                        attestation minting. Behind `kchat-dev-issuer`
│   │                        feature flag on kcreate_bridge.
│   └── kcreate_tests/       cross-crate integration tests (no library
│                            surface — see tests/ subdir)
├── tools/
│   └── kcreate_diffusion/   loopback Python FLUX sidecar spawned by
│                            kcreate_ai::image_gen — never networked.
├── apps/
│   └── desktop/             Electron shell (main + preload + React renderer)
│       ├── main/            main process (loads bridge.node via process.dlopen)
│       ├── preload/         context-bridge exposing `window.kcreate.*`
│       └── renderer/        Vite + React app (HomePage, EditorPage,
│                            CanvasHost is the present surface)
├── PROPOSAL.md              product spec
├── ARCHITECTURE.md          technical architecture
├── PROGRESS.md              phase tracking
├── CONTRIBUTING.md          contributor guide
├── SECURITY.md              security policy
└── .github/workflows/ci.yml fast lane (ubuntu + node) + gated cross-platform matrix (macos-13 + windows-2022, opt-in via `full-ci` label / `[full-ci]` in commit msg / push to main / workflow_dispatch). Every job has `timeout-minutes`.
```

## Architecture contract

- Rust owns the **entire rendering pipeline** (scene graph → display list →
  GPU commands → readback) from day 1. The Electron renderer never runs
  any vector math.
- The presentation path is offscreen wgpu → CPU readback → IPC →
  `putImageData` on an HTML `<canvas>`. Phase 1 swaps **only** the
  presentation path (`presenter.rs` + `CanvasHost.tsx`), not the
  pipeline.
- All crates compile and pass tests without a live Node runtime (we use
  `napi/dyn-symbols` so `cargo test` works in CI without Electron).
- **Bridge layering.** All business logic lives in
  `crates/kcreate_bridge/src/state.rs` (renderer),
  `crates/kcreate_bridge/src/document.rs` (project/document/export),
  and `crates/kcreate_bridge/src/phase2.rs` (preflight, icon pack,
  parallel batch, AI, plugin sandbox, MCP permissions,
  screenshot-to-layout); `src/lib.rs` is a thin N-API marshalling
  layer only. Bridge tests use `serial_test` because the renderer
  and the project workspace are both process-global singletons.
- **Workspace access.** `Workspace` fields are `pub(crate)` so other
  bridge modules can compose new entry points without leaking the
  type, but new code must go through `document::with_workspace` /
  `document::with_workspace_mut` rather than touching `ws.project` /
  `ws.store` directly. Those helpers own the locking discipline, the
  scene-sync hook, and the operation-log invariants — bypassing them
  silently breaks undo / persistence / native canvas dirty tracking.
- **Local-first invariant.** No editing-path crate may pull in a
  networking library. `crates/kcreate_tests/tests/local_first.rs`
  enforces this against a deny-list — keep it green.
- **Collab feature isolation.** The `collab` feature on
  `kcreate_bridge` is the only path that pulls networking (`quinn`,
  `rustls`, `mdns-sd`, `tokio`). It is opt-in and does not affect
  the local-first sentinel because `kcreate_collab_transport` is
  excluded from the editing-path closure in `local_first.rs`.
  Similarly, `kchat-dev-issuer` gates `kcreate_kchat`.

## Rules

1. **No stubs.** Every public function must be implemented end-to-end.
   No `todo!()`, `unimplemented!()`, placeholder returns, or `// TODO`
   comments in production code.
2. **No clippy warnings.** `cargo clippy --workspace --all-targets`
   must come back clean. CI runs with `-D warnings`.
3. **Tests on every behavior change.** Bridge tests use `serial_test`
   because the renderer + workspace singletons are process-global.
   Cross-crate scenarios go in `crates/kcreate_tests/tests/`.
4. **Wire format lockstep.** `apps/desktop/shared/scene.ts` mirrors
   `crates/kcreate_bridge/src/wire.rs` (renderer) and the N-API surface
   exported from `crates/kcreate_bridge/src/lib.rs` (document, project,
   runtime, export). If you add a field to one, add it to the others
   and update tests.
5. **No network in the editing path.** If you genuinely need a network
   crate, isolate it in a new crate that the editing-path tree never
   depends on, and update the deny-list test with a rationale.

## Common commands

```bash
# Rust
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
cargo bench -p kcreate_renderer --no-run    # just check benches compile

# Node / TypeScript
pnpm install
pnpm typecheck
pnpm lint
```

## Where new code goes

| Concern                                  | File                                               |
| ---------------------------------------- | -------------------------------------------------- |
| Add a new shape primitive                | `crates/kcreate_renderer/src/scene.rs` + display_list.rs + cpu_backend.rs + gpu.rs + wire.rs |
| Add a node type / blend mode / effect    | `crates/kcreate_core/src/node.rs`                  |
| Document graph operations                | `crates/kcreate_core/src/document.rs`              |
| Undo/redo or operation provenance        | `crates/kcreate_core/src/operation.rs`             |
| Add a vector path operation              | `crates/kcreate_vector/`                           |
| Add a text layer / font feature          | `crates/kcreate_text/`                             |
| Raster tile / mask / adjustment          | `crates/kcreate_raster/`                           |
| Persistent storage (SQLite/blobs)        | `crates/kcreate_storage/`                          |
| Export pipeline (PNG/SVG/PDF/WebP/JPEG)  | `crates/kcreate_export/`                           |
| Document→Scene translation               | `crates/kcreate_bridge/src/scene_sync.rs`          |
| Canvas hit testing                       | `crates/kcreate_bridge/src/hit_test.rs`            |
| AI action / task router                  | `crates/kcreate_ai/`                               |
| Auto-layout computation                  | `crates/kcreate_layout/` (`flex.rs`, `grid.rs`)    |
| LLM sidecar lifecycle                    | `crates/kcreate_ai/src/llm_sidecar.rs`             |
| LLM chat (loopback HTTP)                 | `crates/kcreate_ai/src/llm_chat.rs`                |
| Inspect mode code-gen (CSS/Tailwind/React) | `crates/kcreate_export/src/code_gen.rs`          |
| Native surface (raw-window-handle)       | `crates/kcreate_renderer/src/native_surface.rs`    |
| MCP tool                                 | `crates/kcreate_mcp/src/tools.rs`                  |
| Add a new N-API export (renderer)        | thin wrapper in `kcreate_bridge/src/lib.rs`, logic in `state.rs`     |
| Add a new N-API export (document/project)| thin wrapper in `kcreate_bridge/src/lib.rs`, logic in `document.rs`  |
| Add an IPC channel                       | `apps/desktop/main/src/main.ts` + `preload/src/preload.ts` + `shared/scene.ts` |
| Tweak Electron window                    | `apps/desktop/main/src/main.ts`                    |
| Tweak the canvas presentation surface    | `apps/desktop/renderer/src/components/CanvasHost.tsx` |
| Add a UI page / panel                    | `apps/desktop/renderer/src/pages/` or `components/` |
| Cross-crate integration test             | `crates/kcreate_tests/tests/`                      |
| Native canvas handle interpretation      | `crates/kcreate_bridge/src/native_canvas.rs`        |
| Prototype interactions (core)            | `crates/kcreate_core/src/node.rs` (Interaction types) |
| Prototype interactions (bridge)          | `crates/kcreate_bridge/src/document.rs` (`interaction_add/remove/list/list_batch`) |
| Page layout / master pages               | `crates/kcreate_core/src/node.rs` (PageLayout types) + `crates/kcreate_core/src/project.rs` (templates) |
| PageNavigator panel                      | `apps/desktop/renderer/src/components/PageNavigator.tsx` |
| TemplatePicker panel                     | `apps/desktop/renderer/src/components/TemplatePicker.tsx` |
| AccessibilityPanel                       | `apps/desktop/renderer/src/components/AccessibilityPanel.tsx` |
| PrototypePlayer                          | `apps/desktop/renderer/src/components/PrototypePlayer.tsx` |
| InteractionPanel                         | `apps/desktop/renderer/src/components/InteractionPanel.tsx` |
| PDF preflight                            | `crates/kcreate_export/src/preflight.rs`           |
| Icon pack generation                     | `crates/kcreate_export/src/icon_pack.rs`           |
| Parallel batch + async cancel            | `crates/kcreate_export/src/batch.rs` (`run_batch_parallel`) |
| AI upscale (Lanczos3)                    | `crates/kcreate_ai/src/upscale.rs`                 |
| AI palette extraction (k-means)          | `crates/kcreate_ai/src/palette.rs`                 |
| AI smart-select (BFS flood-fill)         | `crates/kcreate_ai/src/smart_select.rs`            |
| AI model pack registry                   | `crates/kcreate_ai/src/model_registry.rs`          |
| Screenshot-to-layout                     | `crates/kcreate_ai/src/screenshot_to_layout.rs`    |
| Plugin manifest / registry               | `crates/kcreate_plugin/src/{manifest,registry}.rs` |
| WASM plugin execution                    | `crates/kcreate_plugin/src/wasm_runtime.rs`        |
| Plugin signing (Ed25519)                 | `crates/kcreate_plugin/src/trust.rs` (+ `PluginSignature` in `manifest.rs`) |
| PDF import (Phase 2 PR #7)               | `crates/kcreate_export/src/pdf_import.rs`          |
| Collaboration protocol — peer identity   | `crates/kcreate_collab/src/peer.rs`                |
| Collaboration protocol — Lamport clock   | `crates/kcreate_collab/src/clock.rs`               |
| Collaboration protocol — signed envelopes| `crates/kcreate_collab/src/envelope.rs`            |
| Collaboration protocol — message variants| `crates/kcreate_collab/src/message.rs`             |
| Collaboration protocol — conflict resolve| `crates/kcreate_collab/src/conflict.rs`            |
| Collaboration protocol — session state   | `crates/kcreate_collab/src/session.rs`             |
| MCP permissions                          | `crates/kcreate_mcp/src/permissions.rs`            |
| Phase 2 N-API marshalling                | thin wrapper in `kcreate_bridge/src/lib.rs`, logic in `phase2.rs` |
| PreflightPanel                           | `apps/desktop/renderer/src/components/PreflightPanel.tsx` |
| IconPackDialog                           | `apps/desktop/renderer/src/components/IconPackDialog.tsx` |
| PluginManager                            | `apps/desktop/renderer/src/components/PluginManager.tsx` |
| McpSettingsPanel                         | `apps/desktop/renderer/src/components/McpSettingsPanel.tsx` |
| ScreenshotToLayout                       | `apps/desktop/renderer/src/components/ScreenshotToLayout.tsx` |
| Vision sidecar (VLM)                     | `crates/kcreate_ai/src/{vision_chat,mlx_sidecar,sidecar_dispatcher}.rs` |
| Image generation sidecar                 | `crates/kcreate_ai/src/image_gen.rs` |
| Brand / crop / tokens / style / critique | `crates/kcreate_ai/src/{brand_extract,smart_crop,design_tokens_vlm,style_describe,design_critique}.rs` |
| OCR text-region detection                | `crates/kcreate_ai/src/ocr.rs` |
| VisionAssistSection                      | `apps/desktop/renderer/src/components/VisionAssistSection.tsx` |
| ImageGenPanel                            | `apps/desktop/renderer/src/components/ImageGenPanel.tsx` |
| KChatSignInPanel                         | `apps/desktop/renderer/src/components/KChatSignInPanel.tsx` |
| PresencePanel                            | `apps/desktop/renderer/src/components/PresencePanel.tsx` |
| LAN transport host                       | `crates/kcreate_collab_transport/src/host.rs` |
| Peer discovery (mDNS)                    | `crates/kcreate_collab_transport/src/discovery.rs` |
| Transport wire codec                     | `crates/kcreate_collab_transport/src/wire.rs` |
| Transport TLS cert bundle                | `crates/kcreate_collab_transport/src/cert.rs` |
| Session bridge                           | `crates/kcreate_bridge/src/collab.rs` |
| Diffusion sidecar                        | `tools/kcreate_diffusion/server.py` |
| Phase 4 bridge surface                   | `crates/kcreate_bridge/src/phase4.rs` |
| LLM bridge (lifecycle + chat)            | `crates/kcreate_bridge/src/llm.rs` |
| Operation journal                        | `crates/kcreate_collab/src/journal.rs` |
| KChat authority types                    | `crates/kcreate_collab/src/kchat.rs` |
| KChat dev issuer                         | `crates/kcreate_kchat/src/lib.rs` |
| Raster filters (blur / sharpen / heal)   | `crates/kcreate_raster/src/{filters,heal}.rs` |
| Raster transforms (crop / rotate / flip) | `crates/kcreate_raster/src/transform.rs` |
| Raster ops bridge surface                | `crates/kcreate_bridge/src/raster_ops.rs` |
| FiltersPanel UI                          | `apps/desktop/renderer/src/components/FiltersPanel.tsx` |
| Vector snap engine                       | `crates/kcreate_vector/src/snap.rs` |
| Path simplify / smooth / offset          | `crates/kcreate_vector/src/simplify.rs` |
| Variable stroke expansion                | `crates/kcreate_vector/src/stroke.rs` |
| Path effects (dash, round corners)       | `crates/kcreate_vector/src/path_effects.rs` |
| Text flow across linked frames           | `crates/kcreate_text/src/flow.rs` |
| Image-text wraps                         | `crates/kcreate_text/src/wrap.rs` |
| `.kbrand` import / export                | `crates/kcreate_export/src/kbrand.rs` |
| Slice export                             | `crates/kcreate_export/src/slice.rs` |
| Spot colors / overprint / preflight      | `crates/kcreate_core/src/color.rs` + `crates/kcreate_export/src/preflight.rs` |
