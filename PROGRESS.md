# KCreate — Progress

This document is the single source of truth for what has shipped, what is in
flight, and what remains. Update it in every PR that completes a checklist
item.

## Phase 0 — Technical Spike | Complete | 100%

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
- [x] Basic vector selection / editing on canvas
- [x] Raster image layer display
- [x] PNG / SVG / PDF export prototype (via `kcreate_export`)
- [x] Local background-removal prototype (`kcreate_ai`, threshold-v0; ONNX swap in Phase 1)
- [x] Local MCP server with three tools (`kcreate_mcp`)

### Documentation
- [x] `README.md` (full project description + quick start)
- [x] `PROPOSAL.md` (product spec)
- [x] `ARCHITECTURE.md` (technical architecture)
- [x] `CONTRIBUTING.md`
- [x] `SECURITY.md`

### Phase 0 Exit Criteria — enforced by `crates/kcreate_tests/tests/phase0_exit.rs`
- [x] No network required for full editing (deny-list test in `local_first.rs`)
- [x] Project opens locally as a `.kstudio/` folder (`phase0_project_opens_locally_after_close`)
- [x] Canvas pan/zoom changes pixels on render (`phase0_canvas_pan_zoom_changes_pixels`)
- [x] One AI image action runs locally (`phase0_local_bg_removal_runs_on_cpu`)
- [x] Export works — PNG, SVG, PDF, WebP, JPEG all produce format-valid bytes (`phase0_full_pipeline_runs_without_network`)

## Phase 1 — MVP | In progress | ~75%

Scope: Design Studio, Vector Studio, Image Studio Lite, Brand & Asset Hub,
Export Center, Local AI Core Pack.

### Built so far (foundations laid in the Phase 0 PR)
- [x] `kcreate_raster`: tile engine (`TileGrid`, dirty-tile tracking), masks, adjustment layers
- [x] `kcreate_text`: font discovery (fontdb, skips bitmap-only fonts) + shaping (rustybuzz) + outline walking
- [x] Text layer rendering in `kcreate_renderer` (`ObjectKind::Text` → path-tessellated glyphs)
- [x] WebP + JPEG export (`kcreate_export::{webp,jpeg}`)
- [x] Batch export infrastructure (`kcreate_export::batch`)
- [x] Properties panel (real property editing wired to `document_update_node`)
- [x] Layer panel (tree with inline rename, visibility/lock toggles, delete)
- [x] AI Assist panel (Ask → Preview → Apply → Edit → Undo for bg removal)
- [x] Export panel UI (5 formats, batch presets)
- [x] Mode switcher (functional — tool palettes + right-panel focus per mode)
- [x] Brand-kit / design-tokens / export-preset persistence + IPC (Task 18 / 19 — `brand_kits`, `design_tokens`, `export_presets` SQLite tables; N-API surface `brand_kit_*`, `design_tokens_*`, `export_preset_*`; preload `window.kcreate.{brandKit,designTokens,exportPreset}`)

### Block A–J completed in this iteration (PR #4)
- [x] Artboard / frame system (multi-artboard pages, presets, navigation panel,
      creation dialog, HomePage preset wiring)
- [x] Component system (create, instantiate, variants, persistence, panel UI)
- [x] Auto-layout engine `kcreate_layout` (flex + grid solvers, integration
      with document, UI controls in right panel)
- [x] Low-resource mode enforcement (`RuntimeConfig::effective_*`, dynamic
      undo depth, bridge surface, banner UI)
- [x] Local LLM assistant (`kcreate_ai::llm_sidecar` lifecycle manager,
      `llm_chat` OpenAI-compatible HTTP client, bridge `llm_*` exports,
      chat panel + model manager UIs, context-aware quick actions)
- [x] ONNX u2net background removal path (`bg_remove::BgRemovalBackend`),
      auto-falls back to threshold when model missing
- [x] Expanded AI tasks (layer naming, design-token extraction,
      accessibility check) routed through LLM sidecar
- [x] Design token editor panel
- [x] Brand kit editor panel
- [x] Batch export preset library (Web Assets, Social Pack, Icon Pack,
      Print Ready, Developer Handoff)
- [x] Inspect mode — `kcreate_export::code_gen` (`node_to_css`,
      `node_to_tailwind`, `node_to_react_style`), bridge
      `document_inspect_node`, InspectPanel with SegmentedControl +
      copy-to-clipboard
- [x] Responsive preview component (Desktop 1440 / Tablet 768 / Mobile 375,
      1 Hz frame fetch from `renderer.acquireFrame()`)
- [x] Native CanvasHost foundation — `raw-window-handle` dep,
      `kcreate_renderer::native_surface::NativeSurface`, top-level
      `PresentationMode` enum, `RenderContext::render_frame_native`

### Remaining for Phase 1 ship
- Native CanvasHost Electron integration (platform-specific child-window
  embedding — the renderer-side primitive is ready)
- Full prototype / interaction mode (Phase 1 ships scaled preview only)
- Accessibility-checker UI panel (LLM result presentation)

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
