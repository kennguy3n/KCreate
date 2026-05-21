# KCreate — Phase 0 plan

This document consolidates the work in this branch. It is the source of
truth for what Phase 0 delivers; `AGENTS.md` complements it with
contributor / agent rules.

## Scope of Phase 0

Phase 0 establishes the Rust-owned rendering pipeline and the Electron
shell that hosts it. The goal is to make it possible to render a vector
scene with real GPU code on every supported platform, with a clean
upgrade path to native window compositing in Phase 1.

Out of scope for Phase 0: tools, layers, plug-ins, multi-document UI,
networking, file format. Those land in Phase 1+.

## Architecture

```
                      ┌──────────────────────────┐
                      │   Electron renderer      │
                      │  ┌────────────────────┐  │
                      │  │  React + Vite      │  │
                      │  │  CanvasHost.tsx    │  │
                      │  │  • requests frames │  │
                      │  │  • putImageData    │  │
                      │  └─────────┬──────────┘  │
                      │            │ ipcRenderer │
                      │  preload (contextBridge) │
                      └────────────┼─────────────┘
                                   │ IPC
                      ┌────────────▼─────────────┐
                      │   Electron main process  │
                      │  loads kcreate_bridge    │
                      │  via process.dlopen      │
                      └────────────┬─────────────┘
                                   │ N-API
              ┌────────────────────▼────────────────────┐
              │       crates/kcreate_bridge             │
              │  src/lib.rs  ─ thin #[napi] wrappers    │
              │  src/state.rs ─ singleton state machine │
              │  src/wire.rs  ─ JSON scene parser       │
              └────────────────────┬────────────────────┘
                                   │ pure Rust API
              ┌────────────────────▼────────────────────┐
              │       crates/kcreate_renderer           │
              │  gpu.rs        wgpu adapter init        │
              │  surface.rs    offscreen render target  │
              │  readback.rs   GPU → CPU pixel transfer │
              │  pipeline.rs   scene → display list →   │
              │                GPU/CPU commands         │
              │  presenter.rs  frame ID + buffer        │
              │  cpu_backend.rs tiny-skia fallback      │
              │  scene.rs / display_list.rs / spatial.rs│
              └──────────────────────────────────────────┘
```

### Renderer (`crates/kcreate_renderer`)

The renderer is a stand-alone Rust crate with no `napi` dependency. The
public API is `lib.rs`:

```rust
pub fn initialize(width: u32, height: u32) -> Result<RenderContext>;
impl RenderContext {
    pub fn resize(&mut self, w: u32, h: u32) -> Result<()>;
    pub fn render_frame(&mut self, scene: &Scene) -> FrameId;
    pub fn get_frame_pixels(&self, frame: FrameId) -> Option<Vec<u8>>;
    pub fn set_viewport(&self, pan: Vec2, zoom: f32);
    pub fn invalidate_region(&self, rect: Rect);
}
```

It picks the strongest available backend (`Metal → D3D12 → Vulkan → GL`)
on first call and falls back to the `tiny-skia` CPU backend if no GPU
adapter is reachable. Rendering goes to an offscreen texture; the
mapped readback buffer is published into a triple-buffered `Presenter`
keyed by `FrameId` so the bridge can hand the latest pixels to JS
without blocking the renderer thread.

### Bridge (`crates/kcreate_bridge`)

`napi-rs` cdylib. Three layers of strict separation:

| File         | Purpose                                                |
| ------------ | ------------------------------------------------------ |
| `lib.rs`     | One `#[napi]` function per IPC channel; no logic. Each one calls into `state::` and maps `BridgeError` to `napi::Error`. |
| `state.rs`   | Singleton `OnceLock<Mutex<Option<RenderContext>>>` with init / resize / render / get-frame logic. Fully testable from `cargo test`. |
| `wire.rs`    | `serde_json` schema for `Scene`, decoupled from `serde` types in the renderer. Versionable on the wire without leaking into the renderer's data model. |

Tests are serialized with `serial_test` because they share the global
renderer singleton.

### Electron shell (`apps/desktop`)

Three TS projects with separate tsconfigs:

- `main/` — main process. `bridge.ts` loads the cdylib via
  `process.dlopen`. `main.ts` registers IPC handlers that proxy each
  call to the bridge.
- `preload/` — runs in the privileged context, uses
  `contextBridge.exposeInMainWorld("kcreate", { renderer })` to publish
  a typed `RendererBridge` to the renderer page.
- `renderer/` — Vite + React app. `CanvasHost.tsx` is the only piece
  that talks to the renderer. It owns one `<canvas>` element, polls
  the bridge on rAF, calls `putImageData`, and forwards pointer events
  back through the bridge.

Shared types live in `apps/desktop/shared/scene.ts` and stay in lockstep
with `crates/kcreate_bridge/src/wire.rs`.

## Benchmarks

`crates/kcreate_renderer/benches/`:

| Bench                      | What it measures                                      |
| -------------------------- | ----------------------------------------------------- |
| `frame_render_empty.rs`    | baseline cost of an empty-scene frame                 |
| `frame_render_shapes.rs`   | 100 / 1 000 / 10 000-shape scenes                     |
| `frame_readback.rs`        | GPU → CPU pixel transfer at 1080p and 1440p           |
| `viewport_pan.rs`          | redraw cost after a pure viewport pan (display-list cache reuse) |

CI runs `cargo bench --no-run` to keep the benches compiling; numeric
gates land in Phase 1.

## Phase 1 upgrade path

| File                                | Phase 1 change                                       |
| ----------------------------------- | ---------------------------------------------------- |
| `gpu.rs` / `pipeline.rs` / `scene.rs` | unchanged                                          |
| `surface.rs`                        | swap offscreen texture for a `raw-window-handle` surface |
| `readback.rs`                       | deleted                                              |
| `presenter.rs`                      | direct swapchain present                             |
| `CanvasHost.tsx`                    | replaced by a native child view embedded by Rust     |

Roughly 90 % of the renderer code carries forward unchanged.
