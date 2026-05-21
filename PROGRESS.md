# KCreate — Progress

This document is the single source of truth for what has shipped, what is in
flight, and what remains. Update it in every PR that completes a checklist
item.

## Phase 0 — Technical Spike | In progress | ~70%

### Infrastructure
- [x] Repository created (AGPLv3 license)
- [x] Cargo workspace initialized
- [x] CI pipeline (GitHub Actions)
- [x] CI: macOS + Windows + Linux matrix
- [x] Electron shell (main + preload + renderer)
- [x] React UI with `CanvasHost` component

### Rendering
- [x] `kcreate_renderer`: offscreen wgpu pipeline
- [x] `kcreate_renderer`: CPU fallback (`tiny-skia`)
- [x] `kcreate_renderer`: display list + pipeline cache
- [x] `kcreate_renderer`: presenter (triple-buffered readback)
- [x] `kcreate_bridge`: N-API bindings
- [x] Criterion benchmarks (`frame_render_empty`, `frame_render_shapes`,
      `frame_readback`, `viewport_pan`)

### Document Model
- [x] `kcreate_core`: node model + document graph
- [x] `kcreate_core`: operation log (undo/redo)
- [x] `kcreate_core`: project model
- [x] `kcreate_core`: device tier / config

### Storage
- [x] `kcreate_storage`: SQLite schema + project I/O
- [x] `kcreate_storage`: content-addressed blob store (BLAKE3)
- [x] `kcreate_storage`: `.kstudio/` project folder format

### Vector Engine
- [x] `kcreate_vector`: path representation + math
- [x] `kcreate_vector`: boolean operations (union / subtract / intersect /
      exclude)
- [x] `kcreate_vector`: SVG import (via `usvg`)
- [x] `kcreate_vector`: SVG export (clean output)
- [x] `kcreate_vector`: spatial index (R-tree)

### Integration
- [x] Bridge: document CRUD IPC (create/open/save project, node ops,
      undo/redo)
- [x] Home screen UI (project launcher with job-first creation)
- [x] Editor page skeleton (top bar, mode switch, left/right panels)
- [ ] Basic vector selection / editing on canvas
- [ ] Raster image layer display
- [x] PNG / SVG export prototype (via `kcreate_export`)
- [ ] PDF export prototype
- [ ] Local background-removal prototype (ONNX sidecar)
- [ ] Local MCP server with three tools

### Documentation
- [x] `README.md` (full project description + quick start)
- [x] `PROPOSAL.md` (product spec)
- [x] `ARCHITECTURE.md` (technical architecture)
- [x] `CONTRIBUTING.md`
- [x] `SECURITY.md`

### Phase 0 Exit Criteria
- No network required for full editing
- Project opens locally as a `.kstudio/` folder
- Canvas pan/zoom smooth on modest hardware
- One AI image action runs locally
- Export works (PNG at minimum)

## Phase 1 — MVP | Not started

Scope: Design Studio, Vector Studio, Image Studio Lite, Brand & Asset Hub,
Export Center, Local AI Core Pack.

- Artboards, layers, vector shapes, text, raster image layers
- Background removal (local AI)
- Brand kit (colors, fonts, logos, spacing, export presets)
- SVG / PNG / PDF export
- Local LLM assistant
- Local AI action preview (Ask → Preview → Apply → Edit → Undo)
- Low-resource mode
- Native `CanvasHost` (Metal / D3D12 / Vulkan) replacing WebGPU MVP

## Phase 2 — Professional Workflows | Not started

- Layout Studio (deck/proposal templates, master pages, PDF preflight)
- Batch export, screenshot-to-layout, icon pack generator
- More local AI model packs
- Plugin sandbox (WASM → JS panels → signed native)
- MCP permission UI

## Phase 3 — Advanced Suite | Not started

- Deeper print support, stronger PDF import
- Optional local collaboration over LAN
- Advanced inpainting, local style model packs
- Marketplace for vetted local templates
- KChat artifact publishing
