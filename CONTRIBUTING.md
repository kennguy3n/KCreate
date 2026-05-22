# Contributing to KCreate

Thanks for your interest in KCreate. This guide gets you from a fresh
clone to a green CI run in roughly 10 minutes.

## Prerequisites

- **Rust 1.95** — pinned in `rust-toolchain.toml`. Install via
  [rustup](https://rustup.rs/); the toolchain installs on first
  `cargo` invocation in the repo.
- **Node.js 20.11+** — the workspace pins `>=20.11.0`.
- **pnpm 10+** — `npm install -g pnpm@10.26.0` (the version that the
  CI uses).
- **C / C++ toolchain** — `clang` or `gcc` for native crate builds and
  the napi-rs cdylib.

## Platform setup

### Linux (Ubuntu / Debian)

```bash
sudo apt-get update
sudo apt-get install -y --no-install-recommends \
    libwayland-dev libxkbcommon-dev libxcb1-dev libgl1 libvulkan1 \
    mesa-vulkan-drivers build-essential pkg-config \
    libfontconfig1-dev
```

`libfontconfig1-dev` is needed by `kcreate_text` (via `fontdb`) to
enumerate system fonts. Without it, font discovery falls back to the
bundled-only set.

The `kcreate_renderer` crate falls back to its `tiny-skia` CPU
rasterizer if no Vulkan adapter is available, so headless containers
work too — the GPU deps are optional for tests but required for the
desktop app.

`kcreate_ai`'s ONNX background-removal backend pulls in `ort 2.x`,
which downloads a prebuilt ONNX Runtime shared library on first
compile (no extra apt packages needed). If you're on a fully offline
runner, set `ORT_DYLIB_PATH` to a locally-staged copy. Tests that
require an actual u2net model are marked `#[ignore]` and skip when
the model file is absent — the threshold backend always runs and is
the production fallback when no model is shipped.

`kcreate_ai`'s LLM sidecar (`llm_sidecar.rs`) talks to an external
`llama-server` over loopback (`127.0.0.1:<port>`). The sidecar
binary is not bundled in CI; the sidecar lifecycle tests use a mock
HTTP server so `cargo test` works without `llama-server` installed.

### macOS

```bash
xcode-select --install
```

Metal is part of macOS; no additional installs needed. Apple Silicon
and Intel are both supported.

### Windows

Install [Visual Studio Build Tools](https://aka.ms/vs/17/release/vs_BuildTools.exe)
with the "Desktop development with C++" workload. D3D12 ships with
recent Windows; no additional GPU SDK needed.

## Build

```bash
pnpm install
cargo build --workspace
pnpm build         # main + preload + renderer
```

## Test

```bash
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
cargo bench -p kcreate_renderer --no-run
cargo bench -p kcreate_export --no-run
cargo bench -p kcreate_raster --no-run
cargo bench -p kcreate_ai --no-run
cargo bench -p kcreate_layout --no-run

pnpm typecheck
pnpm lint
```

`kcreate_plugin` uses the pure-Rust `wasmi` runtime, so it does not
require LLVM or any system-level WASM toolchain. `cargo test
-p kcreate_plugin` runs the sandbox + manifest tests with no extra
setup beyond the workspace toolchain.

Bridge tests share a process-global renderer singleton; they use the
`serial_test` crate to run serialized.

## Code style

| Tool                                        | Where                                                                     |
| ------------------------------------------- | ------------------------------------------------------------------------- |
| `rustfmt`                                   | Run before every commit. CI runs `cargo fmt --all --check`.               |
| `clippy` (pedantic + nursery)               | Workspace lints in `Cargo.toml` `[workspace.lints.clippy]`. CI is `-D warnings`. |
| `unsafe_code` forbidden                     | `#![forbid(unsafe_op_in_unsafe_fn)]` on every crate root.                 |
| Prettier defaults for TypeScript            | Two-space indent, semicolons, double quotes.                              |
| ESLint strict TS rules                      | `apps/desktop/eslint.config.mjs`. CI runs `pnpm lint --max-warnings=0`.   |

### Rules of the road

- **No stubs, no TODOs, no placeholders.** Every function is fully
  implemented. `todo!()`, `unimplemented!()`, `// TODO`, and empty
  `Ok(())` placeholders are rejected in review.
- **No `any` in TypeScript.** If you reach for `any`, you don't
  understand the type — fix the type instead.
- **Errors are typed.** Use `thiserror` for Rust error enums; use
  branded error types for TS.
- **Tests cover happy path and error cases.** A bug fix without a
  regression test is incomplete.
- **Documentation lives in code.** `///` doc comments on public items;
  module-level comments at the top of each file.

## PR process

1. Open a feature branch named `devin/<timestamp>-<short-topic>` or
   `feat/<topic>`.
2. Make focused commits with [conventional commit](https://www.conventionalcommits.org)
   prefixes (`feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `ci:`,
   `chore:`).
3. Run the full check suite locally before pushing:

   ```bash
   cargo fmt --all
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace --all-targets
   pnpm typecheck
   pnpm lint
   ```

4. Push and open a PR against `main`. CI must be green before review.

### CI lanes

CI is gated to keep PR feedback fast:

| Lane              | When it runs                                                                                                                                                                                                                                | Jobs                                                                       |
| ----------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------- |
| **Fast (default)**| Every PR.                                                                                                                                                                                                                                  | `rust (ubuntu-22.04)`, `node (typescript)`                                 |
| **Cross-platform**| Push to `main`; PR with the `full-ci` label; commit message containing `[full-ci]`; manual `workflow_dispatch`.                                                                                                                              | `rust (macos-13)`, `rust (windows-2022)`                                   |

To verify a PR on all three platforms before merge, add the `full-ci`
label (preferred — it re-runs the matrix on every push to the PR) or
amend the latest commit message to include `[full-ci]`. Every job has a
`timeout-minutes` cap so a runner-shortage queue can never stall a PR
indefinitely.

## License

By contributing, you agree your contribution is licensed under
AGPL-3.0-or-later. See [`LICENSE`](./LICENSE).
