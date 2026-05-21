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
│   ├── kcreate_bridge/      napi-rs cdylib (renderer + document + export
│   │                        IPC). Logic in state.rs / document.rs;
│   │                        lib.rs is a thin N-API marshalling layer
│   ├── kcreate_vector/      path math, boolean ops (i_overlay), SVG
│   │                        import (usvg), SVG export, R-tree spatial
│   │                        index (rstar)
│   ├── kcreate_storage/     SQLite (rusqlite, bundled), content-addressed
│   │                        BLAKE3 blob store, .kstudio/ project I/O
│   ├── kcreate_export/      PNG (scene → image crate) and SVG (document
│   │                        graph → kcreate_vector::svg_export) export
│   └── kcreate_tests/       cross-crate integration tests (no library
│                            surface — see tests/ subdir)
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
└── .github/workflows/ci.yml ubuntu-22.04 / macos-13 / windows-2022 matrix
```

## Phase 0 contract

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
  `crates/kcreate_bridge/src/state.rs` (renderer) and
  `crates/kcreate_bridge/src/document.rs` (project/document/export);
  `src/lib.rs` is a thin N-API marshalling layer only. Bridge tests use
  `serial_test` because the renderer and the project workspace are
  both process-global singletons.
- **Local-first invariant.** No editing-path crate may pull in a
  networking library. `crates/kcreate_tests/tests/local_first.rs`
  enforces this against a deny-list — keep it green.

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
| Persistent storage (SQLite/blobs)        | `crates/kcreate_storage/`                          |
| Export pipeline (PNG/SVG/PDF/WebP)       | `crates/kcreate_export/`                           |
| Add a new N-API export (renderer)        | thin wrapper in `kcreate_bridge/src/lib.rs`, logic in `state.rs`     |
| Add a new N-API export (document/project)| thin wrapper in `kcreate_bridge/src/lib.rs`, logic in `document.rs`  |
| Add an IPC channel                       | `apps/desktop/main/src/main.ts` + `preload/src/preload.ts` + `shared/scene.ts` |
| Tweak Electron window                    | `apps/desktop/main/src/main.ts`                    |
| Tweak the canvas presentation surface    | `apps/desktop/renderer/src/components/CanvasHost.tsx` |
| Add a UI page / panel                    | `apps/desktop/renderer/src/pages/` or `components/` |
| Cross-crate integration test             | `crates/kcreate_tests/tests/`                      |
