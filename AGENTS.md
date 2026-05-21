# KCreate — agent guide

This file is for AI coding agents (Devin, Codex, etc.). Human contributors should
read `README.md` and `docs/PLAN.md` first.

## Layout

```
KCreate/
├── crates/
│   ├── kcreate_renderer/   pure Rust: offscreen wgpu pipeline + CPU fallback
│   └── kcreate_bridge/     napi-rs cdylib exposing renderer to Electron
├── apps/
│   └── desktop/            Electron shell (main + preload + React renderer)
│       ├── main/           main process (loads bridge.node via process.dlopen)
│       ├── preload/        context-bridge exposing `window.kcreate.renderer`
│       └── renderer/       Vite + React app (CanvasHost.tsx is the present surface)
├── docs/PLAN.md            consolidated plan (Plan 1 + Plan 2 + amendment)
└── .github/workflows/ci.yml
```

## Phase 0 contract

- Rust owns the **entire rendering pipeline** (scene graph → display list →
  GPU commands → readback) from day 1. The Electron renderer never runs
  any vector math.
- The presentation path is offscreen wgpu → CPU readback → IPC →
  `putImageData` on an HTML `<canvas>`. Phase 1 swaps **only** the
  presentation path (`presenter.rs` + `CanvasHost.tsx`), not the
  pipeline.
- Both crates must compile and pass tests without a live Node runtime
  (we use `napi/dyn-symbols` so `cargo test` works in CI without
  Electron). All business logic lives in `crates/kcreate_bridge/src/state.rs`
  and `crates/kcreate_bridge/src/wire.rs`; `src/lib.rs` is a thin N-API
  marshalling layer only.

## Rules

1. **No stubs.** Every function described in `docs/PLAN.md` must be
   implemented end-to-end. No `todo!()`, `unimplemented!()`, placeholder
   return values, or `// TODO` comments in production code.
2. **No clippy warnings.** `cargo clippy --workspace --all-targets`
   must come back clean. CI runs with `-D warnings`.
3. **Tests on every behavior change.** Bridge tests use `serial_test`
   because the renderer is a process-global singleton.
4. **Wire format lockstep.** `apps/desktop/shared/scene.ts` and
   `crates/kcreate_bridge/src/wire.rs` describe the same JSON shape. If
   you add a field to one, add it to the other and update tests.

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
| Add a new N-API export                   | thin wrapper in `kcreate_bridge/src/lib.rs`, logic in `state.rs` |
| Add an IPC channel                       | `apps/desktop/main/src/main.ts` + `preload/src/preload.ts` + `shared/scene.ts` |
| Tweak Electron window                    | `apps/desktop/main/src/main.ts`                    |
| Tweak the canvas presentation surface    | `apps/desktop/renderer/src/components/CanvasHost.tsx` |
