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

## Phase 1 — MVP | Complete | 100%

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

### Block A–B completed in PR #5
- [x] Native CanvasHost Electron integration — `native_canvas.rs` interprets
      platform `PlatformHandle`s (`AppKitWindow`, `Win32Window`, `XlibWindow`,
      `XcbWindow`), `CanvasHost.tsx` runs dual-mode (offscreen / native),
      Wayland declined safely
- [x] Renderer dual-mode — `PresentationMode::{Offscreen, Native}`,
      `RenderContext::switch_to_native` / `switch_to_offscreen`
- [x] Window lifecycle hooks — close event → `switchOffscreen`
- [x] Full prototype / interaction mode — `InteractionTrigger`,
      `InteractionAction` on every node, `interaction_add/remove/list/list_batch`
      bridge functions, `PrototypePlayer` overlay, `InteractionPanel`
- [x] Accessibility-checker UI panel — `AccessibilityPanel` with LLM-driven
      analysis, severity badges, fix buttons, node-link selection
- [x] Layout Studio page model — `PageLayout`, `PageSize`, `PageOrientation`,
      `Margins` in `kcreate_core::node`; master pages
      (create/list/apply/detach); 3 built-in templates (Pitch Deck,
      Proposal, Brochure)
- [x] PageNavigator UI (drag-reorder, context menu, inline add-page picker)
- [x] TemplatePicker UI (modal with card grid, preview pane)
- [x] `project_is_untouched` bridge probe (fresh-project picker support)

## Phase 2 — Professional Workflows | Complete | 100%

### Block A–B completed in PR #6
- [x] Layout Studio foundation (PR #5 — page document model, master pages,
      page navigator, template picker)
- [x] Deck / proposal templates — 3 built-in (Pitch Deck, Proposal,
      Brochure) shipped in PR #5
- [x] PDF preflight engine (`kcreate_export::preflight`) — 6 checks:
      `BleedMargin`, `FontEmbed`, `ImageResolution`, `ColorSpace`,
      `Transparency`, `PageSize`. Bridge: `preflight_run`. UI:
      `PreflightPanel` (Layout / Export mode tab).
- [x] Icon pack generator (`kcreate_export::icon_pack`) — Web / iOS /
      Android / Favicon platform presets. Bridge: `export_icon_pack` +
      `export_icon_pack_built_in_platforms`. UI: `IconPackDialog`.
- [x] Enhanced batch export — parallel rayon driver
      (`kcreate_export::batch::run_batch_parallel`), `Arc<AtomicBool>`
      cancellation, progress callback. Bridge: async job model
      (`export_batch_start`, `export_batch_status`, `export_batch_cancel`).
- [x] AI Lanczos upscale (`kcreate_ai::upscale`) — Lanczos3
      reconstruction, row-parallel via rayon. 2× / 4× supported.
- [x] AI palette extraction (`kcreate_ai::palette`) — k-means clustering
      in RGB (downsampled to 256×256, 20 iterations, sorted by
      frequency).
- [x] AI smart-select (`kcreate_ai::smart_select`) — BFS flood-fill,
      Euclidean RGB tolerance.
- [x] AI model pack registry (`kcreate_ai::model_registry`) — declares
      Core / ImagePro / DesignPro / Generation packs, computes
      `installed` against a local `models_dir`.
- [x] Plugin sandbox foundation — new crate `kcreate_plugin`:
      `manifest` (PluginManifest, PluginType, PluginPermission),
      `registry` (scan + enable/disable persisted to JSON),
      `wasm_runtime` (wasmi 0.42, host ABI:
      `kcreate_log`, `kcreate_get_input{,_len}`, `kcreate_set_output`,
      deny-by-default sandbox, page-count `ResourceLimiter`).
- [x] MCP permission model + UI — `kcreate_mcp::permissions`
      (`McpPermissionStore`, `PermissionGrant::{Once, Always, Denied}`,
      `consume_if_once`), JSON-on-disk persistence; server gates each
      tool call. UI: `McpSettingsPanel`.
- [x] Screenshot-to-layout AI (`kcreate_ai::screenshot_to_layout`) —
      grayscale → Sobel → threshold → connected components →
      heuristic classifier (Header / Navigation / Hero / TextBlock /
      Image / Button / Card / Footer / Sidebar / Form / List). UI:
      `ScreenshotToLayout` component.
- [x] Phase 2 bridge module (`kcreate_bridge::phase2`) — single home
      for every Phase 2 surface; `lib.rs` stays as thin N-API marshal.
- [x] TypeScript wire format — `apps/desktop/shared/scene.ts` mirrors
      every new Phase 2 type; `apps/desktop/preload/src/preload.ts`
      exposes `window.kcreate.{preflight, iconPack, batch, aiModel,
      plugin, mcpPermission}`.
- [x] Benchmarks: `kcreate_export::preflight` Criterion bench;
      `kcreate_ai::{upscale, palette, smart_select}` Criterion benches.

### Block A–I completed in this iteration (PR #7)
- [x] **Block A** — CMYK / ICC color management foundation
      (`kcreate_core::color`, `IccProfile`, soft-proof overlay,
      `ColorSettingsPanel`).
- [x] **Block B** — Advanced text frame: multi-column paragraph
      layout, embedded en-US Liang hyphenation, OpenType feature
      shaper (9 booleans + ss01–ss20), `TextFramePanel` +
      `OpenTypePanel` UI.
- [x] **Block C** — Extended WASM plugin ABI (`kcreate_read_document`,
      `kcreate_read_asset`, `kcreate_write_proposal`) with permission
      gating + `PluginContext` proposal model (validated, applied
      as recorded operations — fully undoable). 3 reference plugins
      (`hello`, `node_counter`, `auto_rename`).
- [x] **Block D** — JS panel plugin runtime: sandboxed Electron
      `WebContentsView` per plugin, strict CSP, isolated session
      partition (`plugin-panel:<id>`), `window.kcreatePlugin.sendMessage`
      preload, bridge mediates every panel message.
- [x] **Block E** — Native plugin Ed25519 signing + verification
      (`kcreate_plugin::signing`), manifest signature, `PluginManager`
      shows trust status (Signed by trusted publisher / Unsigned).
- [x] **Block F** — Neural model downloads: pick-file install flow
      (user downloads weights out-of-band, points installer at
      file; BLAKE3-canonicalised SHA-256 verify; atomic rename into
      `~/.kcreate/models/`).
- [x] **Block G** — PDF import (`kcreate_export::pdf_import`):
      MediaBox geometry with full Pages-tree inheritance, JPEG
      passthrough, Flate-uncompressed pixel-buffer → PNG
      (DeviceRGB / DeviceGray / DeviceCMYK), Title/Author metadata,
      `import_pdf` IPC, EditorPage "Import PDF" path.
- [x] **Block H (Phase 3 foundation, Task 28)** — Collaboration
      protocol types in `kcreate_collab` (peer identity, Lamport
      clock, Ed25519-signed envelopes, Hello/Welcome/Op/Presence/
      Heartbeat/Goodbye messages, LWW conflict resolver, project
      session with replay-window + nonce management). Kept **out
      of the editing-path dependency tree** so a future transport
      can pull QUIC / mDNS without contaminating local-first.
- [x] **Block I** — CI updates + documentation sync (this commit).

## Phase 4 — Vision & Generation AI | In flight

Local multimodal inference layer that pairs llama.cpp's GGUF +
mmproj loading with an Apple-Silicon MLX side path, plus a fully
gated FLUX image-generation sidecar. Vision is *soft-gated* (every
tier can run SmolVLM2-256M); image generation is *hard-gated* to
Tier 2+ with a GPU.

- [x] **Block A — Vision Understanding Infrastructure** (Tasks 1–6).
  - `SidecarConfig.mmproj_path` end-to-end with disk validation,
    extra-argv plumbing, and tests.
  - Static registry entries for `vision_smolvlm2_256m`,
    `vision_qwen25vl_7b`, their `_mmproj` companions, and MLX
    variants (`vision_smolvlm_256m_mlx`, `vision_qwen25vl_7b_mlx`).
    `ModelPackCategory::Vision` shipped to `scene.ts`.
  - `MlxSidecar` (`crates/kcreate_ai/src/mlx_sidecar.rs`) mirrors
    `LlmSidecar` over `python3 -m mlx_lm.server`, with availability
    probe and graceful fallback on non-Apple platforms.
  - `ChatContent`/`ContentPart` multimodal wire format
    (`crates/kcreate_ai/src/llm_chat.rs`) — text-only messages
    still serialise as plain strings (back-compat), vision
    messages emit OpenAI-style `image_url` data URIs.
  - Vision bridge: `vision_describe_image`,
    `vision_generate_alt_text`, `vision_analyze_design` in
    `crates/kcreate_bridge/src/phase4.rs`; preload + scene types
    mirrored in TypeScript.
  - `VisionAssistSection` UI in the AI Assist panel.
- [x] **Block B — Image Generation Infrastructure** (Tasks 7–12).
  - `ImageGenSidecar` (`crates/kcreate_ai/src/image_gen.rs`) spawns
    `python3 -m kcreate_diffusion.server` on a loopback port. Hard
    gate: only runnable when `RuntimeConfig::image_generation_allowed()`.
  - Registry entries `image_gen_flux_klein_4b` and
    `image_gen_flux_klein_mlx`, category `Generation`.
  - Generation client + bridge IPC (`image_gen_start`,
    `image_gen_status`, `image_gen_stop`, `image_gen_generate`),
    plus an in-memory `document_import_image_bytes` to avoid temp
    files for generated PNGs.
  - `tools/kcreate_diffusion/` minimal Python package
    (`server.py`, `requirements.txt`, README).
  - `ImageGenPanel` UI (Tier 2+ only).
- [x] **Block C — Top 10 Vision Features** (Tasks 13–20).
  - Design critique (`crates/kcreate_ai/src/design_critique.rs`).
  - Alt-text VLM upgrade in `crates/kcreate_bridge/src/phase2.rs`
    (statistics still produced, VLM caption swapped in when ready).
  - `screenshot_to_layout::refine_with_vlm()` + `REFINE_GRAMMAR`.
  - Brand extraction (`brand_extract.rs`) with GBNF.
  - Smart layer naming in `task_router::build_layer_naming_prompt`
    (thumbnail when VLM ready, text-only otherwise).
  - Content-aware crop (`smart_crop.rs`) + GBNF.
  - Design tokens (`design_tokens_vlm.rs`) + GBNF.
  - Style description (`style_describe.rs`) + GBNF.
- [x] **Block D — Tier Gating & Resource Management** (Tasks 21–24).
  - `DeviceTier::vision_model_allowed`, `vision_model_max_mb`,
    `image_generation_allowed`, `RuntimeConfig::image_generation_allowed`.
  - `recommended_vision_pack` / `recommended_llm_pack` /
    `recommended_image_gen_pack`.
  - `SidecarDispatcher` routes MLX vs llama-server per platform +
    pack id, with `gguf_fallback_for_mlx_pack` for graceful fall-back
    when MLX is unavailable.
  - Model Manager UI filters generation packs on tier, MLX packs
    on platform, and disables Install for vision packs that exceed
    the tier's `vision_model_max_mb`.
- [x] **Block E — Tests** (Tasks 25–27).
  - `crates/kcreate_tests/tests/vision_sidecar.rs` — multimodal
    serialisation, mock-server round-trips, grammar forwarding,
    empty-request rejection, MLX off-platform probe.
  - `crates/kcreate_tests/tests/image_gen_gating.rs` — tier matrix
    (Tier 0/1 forbidden, Tier 2+ requires GPU, vision soft-gated,
    `vision_model_max_mb` monotonic, registry advertises generation
    packs).
  - Registry completeness tests in `model_registry.rs` cover every
    new pack id, category, and capability.
- [x] **Block F — Documentation** (Tasks 28–30).
  - This section + ARCHITECTURE.md §16i/j/k + README "Local AI" row.

## Phase 3 — Advanced Suite | Foundation landed

Protocol-level work shipped in Phase 2 PR #7 to keep the editing
path stable while collab + advanced workflows are built out:

- [x] Collaboration protocol types (`kcreate_collab`) — see Block H.
- [ ] LAN transport (QUIC + mDNS discovery) — separate crate, isolated
      from editing path.
- [ ] Operational CRDT semantics on top of `Operation`.
- [ ] Deeper print support (spot colors, overprint, trapping).
- [ ] Advanced inpainting, local style model packs (ESRGAN, SAM,
      u2net) — registry already declares them; install path lands
      with the model download UI.
- [ ] Marketplace for vetted local templates.

## Changelog

- **2026-05-24** — Phase 4 ship: vision (Qwen2.5-VL + SmolVLM2 over
  llama.cpp / MLX, mmproj + multimodal chat shape), image generation
  (FLUX.2-Klein-4B via `tools/kcreate_diffusion/`, hard-gated to
  Tier 2 + GPU), Top-10 vision actions (alt-text VLM, design
  critique, screenshot refinement, brand / palette / spacing
  extraction, content-aware crop, design-token + style description,
  smart layer naming), tier-aware Model Manager filtering, vision
  + image-gen integration tests, docs sync.
- **2026-05-22 (PR #7)** — Phase 2 complete + Phase 3 foundation:
  Block A (CMYK/ICC), Block B (advanced text + hyphenation +
  OpenType), Block C (extended WASM ABI + proposal model), Block D
  (JS panel sandbox), Block E (Ed25519 plugin signing), Block F
  (neural model install), Block G (PDF import with inherited
  MediaBox + Flate/JPEG image extraction), Block H (kcreate_collab
  protocol types — peer identity, Lamport clock, signed envelopes,
  LWW conflict resolver, replay-window session), Block I (this
  doc + CI sync).
- **2026-05-22** — Block B–H landed: PDF preflight, icon pack,
  parallel batch w/ async cancel, AI upscale/palette/smart-select +
  model registry, kcreate_plugin (wasmi sandbox), MCP permission
  store + settings UI, screenshot-to-layout. Full TypeScript wire
  parity; new criterion benches in `kcreate_export` and `kcreate_ai`.
- **2026-05-21 (PR #5)** — Phase 1 ship: native CanvasHost dual-mode,
  prototype/interaction system, accessibility panel; plus Layout
  Studio foundation (page model, master pages, templates).
- KChat artifact publishing
