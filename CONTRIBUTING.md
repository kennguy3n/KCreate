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
    mesa-vulkan-drivers build-essential pkg-config
```

The `kcreate_renderer` crate falls back to its `tiny-skia` CPU
rasterizer if no Vulkan adapter is available, so headless containers
work too — the GPU deps are optional for tests but required for the
desktop app.

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

pnpm typecheck
pnpm lint
```

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

## License

By contributing, you agree your contribution is licensed under
AGPL-3.0-or-later. See [`LICENSE`](./LICENSE).
