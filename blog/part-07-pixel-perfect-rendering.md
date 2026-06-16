# 07 — Pixel-perfect on every backend

Everything in the previous posts — templates, generated drafts, brand
kits, resizing — ultimately has to become **pixels**, identically,
whether you're on a GPU workstation or a headless CI machine, on screen
or in an exported file. KCreate's rendering pipeline is the part of the
product designed to never surprise you.

![A dense analytics dashboard rendered by KCreate](./assets/perf-dense-dashboard.png)

*A dense dashboard — sidebar, KPI tiles, a gradient bar chart, a
sources panel, metric cards — rendered through KCreate's own pipeline.
Note the gradient bars: gradients render identically on GPU and CPU.*

## Rust owns the whole pipeline

The Electron renderer never does vector math. The **entire** pipeline
— scene graph → display list → rasterization → readback — lives in
native Rust, in [`crates/kcreate_renderer/`](../crates/kcreate_renderer/).
The presentation path is: render offscreen → read pixels back → send
them over IPC → blit onto an HTML `<canvas>`. The browser layer is a
display surface, not a renderer.

This is what makes output **deterministic**. The same scene produces
the same pixels in the editor, in a PNG export, and in CI — because
it's the same Rust code path every time.

## GPU and CPU produce identical pixels

KCreate rasterizes on the GPU through `wgpu` when one is available and
falls back to a CPU rasterizer (`tiny-skia`) when it isn't — for
example on a headless server or after a GPU device loss. The crucial
property is that **both backends produce identical pixels**: the GPU
path composites through the same rasterization logic as the CPU path,
then uploads and reads back. There is no separate, subtly-different GPU
shader to drift out of sync.

That parity is locked down by cross-crate render tests, including
**gradient render-parity** tests that assert a gradient fill produces
the same real pixels across the live export path and the lower-level
wire path — so a gradient can never silently flatten to a solid or
render blank. Gradients, linear and radial, are first-class across
shapes, paths, and text fills.

If the GPU is lost at runtime, the renderer transparently degrades to
the CPU backend and keeps drawing — you don't get a blank canvas, you
get the same image a little slower.

## A present path that stays fast at scale

A design tool has to stay responsive when the document is *big* — a
dashboard with thousands of nodes, a poster with hundreds of layers.
The naïve approach of shipping a full framebuffer to the screen on
every edit doesn't scale: at 1920×1080 that's a **7.91 MiB** copy
across the IPC boundary for every single frame.

KCreate uses **dirty-rect partial present**. The presenter pixel-diffs
the freshly rasterized frame against the last published one to recover
the true changed region, then ships only that tight `w × h × 4`
sub-rect over IPC as a zero-copy view; the canvas host patches just the
changed rows into a persistent backbuffer. A single edit on a dense
document collapses the per-frame payload dramatically:

| Per-frame present | Full frame | Dirty-rect | Improvement |
|-------------------|-----------:|-----------:|------------:|
| Bytes over IPC (one small edit, 1920×1080) | 7.91 MiB | ~7 KiB | **~1157× fewer bytes** |
| Present time, ~5k-node document | baseline | — | **~286× faster** |
| Present time, ~10k-node document | baseline | — | **~1184× faster** |

The first frame, a resize, or a change large enough that a full copy is
cheaper all fall back to a full-frame present automatically — the fast
path is an optimization, never a correctness risk. The numbers above
come from a Criterion benchmark and a dense 5k/10k-node dashboard proof
that ship with the renderer, and you can watch the frame cost live with
the in-app performance HUD (`Ctrl`/`Cmd`+`Shift`+`P`).

Other cold-path and steady-state costs are kept in check by the same
philosophy: a bounded LRU tile cache for raster work
([`crates/kcreate_raster/src/tile_cache.rs`](../crates/kcreate_raster/src/tile_cache.rs)),
startup profiling primitives
([`crates/kcreate_perf/`](../crates/kcreate_perf/)), and a scene-sync
step that skips a full rebuild when a document mutation changes nothing
visible.

## How this compares

- **Figma** set the bar for a fast, GPU-accelerated canvas, and it's
  excellent — but it's a WebGL renderer in a browser tab. KCreate runs a
  native Rust pipeline with a guaranteed CPU fallback and identical
  output across both.
- **Canva** and **Gamma** render in the browser/cloud; fidelity and
  speed depend on the connection and the machine's browser. KCreate's
  output is deterministic and computed locally.
- The **GPU/CPU pixel parity** and **gradient render-parity** guarantees
  are unusual even among native tools — they mean "what you see is what
  exports," on any backend.

---

**Trace it in the code**

- Renderer (wgpu + tiny-skia + presenter): [`crates/kcreate_renderer/`](../crates/kcreate_renderer/)
- Document → scene translation: [`crates/kcreate_bridge/src/scene_sync.rs`](../crates/kcreate_bridge/src/scene_sync.rs)
- Present surface (dirty-rect patching): [`apps/desktop/renderer/src/components/CanvasHost.tsx`](../apps/desktop/renderer/src/components/CanvasHost.tsx)
- Tile cache + perf primitives: [`crates/kcreate_raster/src/tile_cache.rs`](../crates/kcreate_raster/src/tile_cache.rs), [`crates/kcreate_perf/`](../crates/kcreate_perf/)

Previous: [« 06 — One design, every size](./part-06-one-design-every-size.md) ·
Next: [08 — Intelligence that stays on your device »](./part-08-on-device-intelligence.md)
