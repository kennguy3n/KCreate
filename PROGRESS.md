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

## Phase 3 — Advanced Suite | Complete | 100%

Protocol-level work shipped in Phase 2 PR #7 and the LAN transport
shipped in PR #10 (`kcreate_collab_transport` + bridge session
management) — the collab feature is opt-in on `kcreate_bridge` and
stays out of the editing-path dependency tree so the local-first
sentinel stays green.

- [x] Collaboration protocol types (`kcreate_collab`) — see Phase 2 Block H.
- [x] LAN transport (QUIC + mDNS discovery) — `kcreate_collab_transport`
      crate. `LanCollabHost` runs a QUIC endpoint and an mDNS-SD
      responder behind ephemeral self-signed certs pinned via SHA-256
      fingerprint from `kcreate_collab` peer identity. `PeerDiscovery`
      browses the same service; `CertBundle` generates rcgen certs;
      `wire.rs` codecs frame envelopes onto the QUIC streams. Bridge
      `collab.rs` owns `SessionState` + the tokio multi-thread runtime
      and exposes `collab_*` N-API entry points.
- [x] KChat dev issuer (`kcreate_kchat`) — deterministic Ed25519
      key derivation + signed-attestation minting against KChat groups,
      used by the integration tests + the `kchat-dev-issuer` bridge
      feature flag and the renderer `KChatSignInPanel`.
- [x] Operation journal (`kcreate_collab::journal`) — append-only
      log used by the session bridge for resync.
- [x] Renderer improvements — radial / linear gradient scene objects
      (`kcreate_renderer::scene::ObjectKind::{LinearGradient,
      RadialGradient}`) with renderer-side stop interpolation.
- [x] AI inference UX end-to-end — alt-text + layout-suggest wired
      through `kcreate_bridge::phase4` and the AIAssistPanel /
      VisionAssistSection components.
- [x] PDF preflight extensions — shading-pattern validity check,
      per-codepoint font glyph coverage, total ink coverage (TIC),
      bleed-area-content check, DPI floor.
- [x] Scene-sync multi-peer micro-benches + batched insert
      (`crates/kcreate_bridge/benches/scene_sync_*.rs`,
      `Scene::add_objects` batch entry).
- [x] Lock-aware FillSection (solid + gradient editor) — preserves
      in-flight edits across remote scene updates.
- [x] OCR text-region detection → text-layer creation
      (`kcreate_ai::ocr` + bridge `ocr_*` entry points).
- [x] KChat trusted-issuer allowlist — `TrustedIssuersSection` UI
      surface, allowlist stored in the project, gated through the
      bridge.
- [x] **Operational CRDT semantics on top of `Operation`** (PR #16,
      Tasks 1-4). `crates/kcreate_collab/src/crdt.rs` lifts the
      LWW resolver into a real operational transform:
      `OpKind::{PropertySet, Move, Delete}`, per-field merging of
      concurrent `PropertySet` ops (disjoint fields combine,
      shared fields tie-break by Lamport+peer), deterministic
      winner for concurrent moves, delete-beats-edit semantics.
      Wired through `ProjectSession`; 13 unit tests +
      `crates/kcreate_tests/tests/crdt_merge.rs` (12 cases).
- [x] **Deeper print support** (PR #16, Tasks 5-8): Pantone-style
      JSON catalog loader (`SpotColorLibrary::load_catalog`),
      `OverprintTable` + `Trapping` preflight checks,
      `TotalInkCoverage` rounded out, PDF overprint ExtGState
      written through `printpdf`, `SpotColorLibraryPanel.tsx`
      bridge surface. Extended `print_workflow.rs` (24 tests
      total).
- [x] **Advanced inpainting + style model packs** (PR #16, Tasks
      9-10): `install_model_pack(pack_id, file_path)` with BLAKE3
      hash validation against the registry; ESRGAN ONNX upscale
      path (`crates/kcreate_ai/src/upscale.rs` ONNX backend);
      SAM segmentation (`crates/kcreate_ai/src/segment.rs`)
      gated on `ort` availability; bridge entry points through
      `kcreate_bridge/src/phase2.rs`.
- [x] **Template marketplace foundation** (PR #16, Tasks 11-12):
      `crates/kcreate_core/src/marketplace.rs`
      (`TemplateManifest`, `TemplateCategory`,
      `TemplateSource::Local`), local `.ktemplate/` scanner under
      `~/.kcreate/templates/`, `TemplateMarketplace.tsx`,
      `template_list / template_install_local / template_remove`
      bridge.

## Phase 6 — Production Polish | Complete | 100%

PR #16 lands Phase 6 Tasks 13-30 in one batch on top of the
Phase 3 completion above. Local-first invariants, lockstep
testing discipline, and architecturally correct rollback are
preserved throughout.

- [x] **Tasks 13-14: `kcreate_audit` crate** — operation log
      persistence to a separate SQLite database (NOT the project
      DB) with structured queries by date/action/node, bridge
      surface, `AuditPanel.tsx` for the renderer.
- [x] **Tasks 15-16: Undo/Redo UX** — `document_undo_group` /
      `document_redo_group` with `ApplyPatchSnapshot` atomic
      rollback (capture/restore + APPLY_PATCH_COMMANDS lockstep
      invariant tested), drag-coalesced compound undo for high-
      frequency event streams.
- [x] **Tasks 17-18: Lazy thumbnail generation** —
      `crates/kcreate_bridge/src/thumbnails.rs` renders cover +
      page thumbnails off-lock, content-hash addressable cache
      under `.kstudio/thumbnails/`, HomePage recent-projects
      uses on-disk thumbnails without opening the DB. Background
      pre-warm coalesces concurrent callers through an
      `AtomicBool` gate so bursts (rapid `project_create` /
      `project_open`) spawn at most one worker.
- [x] **Tasks 19-20: Import pipeline** — `figma_import.rs` and
      `sketch_import.rs` in `crates/kcreate_export/` (frames →
      artboards, vectors → VectorPath, text → TextLayer), bridge
      `figma_import` / `sketch_import` entry points, fixture
      coverage in `crates/kcreate_tests/tests/`.
- [x] **Tasks 21-22: Keyboard shortcuts** — JSON-backed
      `apps/desktop/renderer/src/shortcuts/registry.ts` registry
      with stable module-scope `shortcutSubscribe` /
      `shortcutGetSnapshot` bindings for `useSyncExternalStore`,
      standard shortcuts wired (`Ctrl+Z/S/E`, `V`/`P`/Space…),
      `KeyboardShortcutsPanel.tsx` for view/edit.
- [x] **Tasks 23-24: Dark mode + theme system** — CSS-variable
      driven (`:root` + `:root[data-theme="dark"]` in
      `index.html`), `ThemeProvider.tsx` writes the
      `data-theme` attribute and the cascade re-evaluates every
      `var(--kc-*)`. No JS palette duplicate (CSS is the single
      source of truth). Preference persisted to localStorage,
      every panel adopted.
- [x] **Tasks 25-26: Drag-and-drop + clipboard** — OS file
      manager drops accept .png/.jpg/.jpeg/.webp/.gif (SVG goes
      through the filesystem-path branch because usvg resolves
      relative `href`s), `clipboard_paste` op wired through the
      operation log with subtree-capture rollback in
      `ApplyPatchSnapshot`, cross-artboard paste preserved.
- [x] **Tasks 27-28: Layer panel search + tagging** — search
      bar filters layers by name, layer-color tags persisted
      via `layer_color_set` op (round-trips through undo/redo),
      "select all of type" wired into the LayerPanel.
- [x] **Tasks 29-30: E2E + benchmark suite** —
      `crates/kcreate_tests/tests/e2e_workflow.rs` covers the
      five PROPOSAL.md §5 user journeys (poster, logo, photo
      cleanup, deck, dev export); benchmark targets land
      `cold_start`, `raster_open_64mp`, `viewport_pan`, and
      `batch_50_assets` (sequential + parallel) against the
      acceptance criteria in PROPOSAL.md §20.

## Phase 4 — Vision & Generation AI | Complete | 100%

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

## Phase 5 — Image Studio Filters + Vector Studio + Layout Studio + Brand Hub | Complete | 100%

The filter / vector / layout / brand-hub gap-filler pass. Adds the
missing professional editing primitives on top of the Phase 1-2
foundations.

### Block B — Image Studio filters
- [x] Levels + Curves adjustment layers
      (`kcreate_raster::layer::AdjustmentLayer::{Levels, Curves}`).
      Levels: black/white point + gamma. Curves: piecewise cubic
      Hermite interpolation over `(x, y)` control points. Both
      run row-parallel via rayon.
- [x] Gaussian + box blur (`kcreate_raster::filters::{gaussian_blur,
      box_blur}`). Separable two-pass Gaussian (horizontal +
      vertical, rayon-parallel). Three-pass sliding-window box blur
      approximates Gaussian in O(1) per pixel.
- [x] Unsharp mask sharpen (`kcreate_raster::filters::unsharp_mask`).
- [x] Crop / rotate / flip (`kcreate_raster::transform::{crop,
      rotate, flip_h, flip_v}`). Rotation uses bilinear
      interpolation, row-parallel.
- [x] Healing brush (`kcreate_raster::heal::heal`) — gradient-domain
      luminance shift + alpha-feathered disc blend.
- [x] Raster operations bridge (`kcreate_bridge::raster_ops`) — N-API
      surface for all filters + preview path; records undoable
      `Operation`s.
- [x] Filters UI panel (`apps/desktop/renderer/src/components/FiltersPanel.tsx`)
      — debounced live preview via `rasterOps.previewFilter`, Apply
      commit through the corresponding `applyXxx` bridge function,
      tabbed Levels / Curves / Blur / Sharpen / Transform sections,
      interactive SVG curve editor with click-to-add / right-click-to-
      remove. Mounted from `RightPanel` whenever a `RasterLayer` is
      selected. Follows the Ask → Preview → Apply → Undo loop.

### Block C — Vector Studio features
- [x] Snapping + smart guides (`kcreate_vector::snap`) — sorted-edge
      snap engine, per-axis snap, artboard edge + midpoint snapping.
- [x] Smart-guides UI overlay in `EditorPage` — a transparent SVG
      sits above the canvas, projects world-space `SnapGuide` lines
      through the active viewport, and renders them as 1 px dashed
      magenta lines. The drag handler in `EditorPage.onCanvasPointer`
      calls `canvasSnap.query()` on every pointermove with the
      candidate world bounds, applies the returned delta to the
      cumulative drag offset, and clears the overlay on pointerup.
      Snap threshold is 6 world units (tight enough to feel
      deliberate, forgiving on high-DPI displays).
- [x] Path simplify / smooth / offset
      (`kcreate_vector::simplify::{simplify, smooth, offset}`).
      Simplify is Ramer–Douglas–Peucker; smooth is Chaikin
      subdivision; offset uses parallel curve construction via kurbo.
- [x] Variable stroke width — `NodeStyle::stroke_width_profile`
      + `kcreate_vector::stroke::expand_variable_stroke` produces a
      filled outline from a centerline + width profile.
- [x] Multi-fill / multi-stroke per node — `NodeStyle::extra_fills` +
      `NodeStyle::extra_strokes` (legacy single `fill` / `stroke` stay
      first, additional layers stack on top with serde-default empty
      vectors so old projects open unchanged). The bridge update path
      (`document_update_node` + new `UpdateNodeProps::{extra_fills,
      stroke, extra_strokes, stroke_width_profile, overprint}`) uses
      a typed `FieldUpdate<T>` enum so JSON `null` clears a field and
      an absent JSON key leaves it untouched — callers can patch the
      three-state `stroke` slot through the same path. The reorderable
      `RightPanel.FillSection` editor remains a UX-polish follow-up
      now that the full bridge surface is in place.
- [x] Path effects (`kcreate_vector::path_effects::{dash,
      round_corners}`) — dash splits the path into sub-paths by
      arc-length walk; round-corners replaces sharp angles with
      circular arcs.

### Block D — Layout Studio + Brand Hub
- [x] Text flow across linked frames (`kcreate_text::flow`).
- [x] Image-text wraps (`kcreate_text::wrap`).
- [x] `.kbrand` brand-kit import / export
      (`kcreate_export::kbrand`) — ZIP archive with `manifest.json`,
      `fonts/`, `logos/`. Round-trips brand kit, tokens, embedded
      fonts, and binary logos.
- [x] Slice export with named regions
      (`kcreate_export::slice`) — per-slice format / scale,
      parallel rayon export.
- [x] Spot colors + overprint foundation
      (`kcreate_core::color::{Color::Spot, SpotColorLibrary,
      Overprint}`) + `PreflightCheck::SpotColorMissing`.

### Block E — Tests
- [x] `crates/kcreate_tests/tests/raster_filters.rs`
- [x] `crates/kcreate_tests/tests/vector_ops.rs`
- [x] `crates/kcreate_tests/tests/text_flow.rs`
- [x] `crates/kcreate_tests/tests/print_workflow.rs`

## Phase 7 — KChat backend Integration | Complete | 100%

PR #17 lands Phase 7 — KCreate's first-party integration with
the shared KChat / Mattermost backend that
`uneycom/uney-chat-desktop` also signs in to (the **Option C**
shape). A new HTTPS REST client (`kcreate_kchat_client`) speaks
`reqwest` + `rustls` to that backend and bridges KChat
communities / conversations / community-member rosters into
the existing collab gate. A thin `.kcz` companion extension
(`apps/kchat-extension/`) renders a sidebar inside KChat
Desktop and bridges deeplinks back to KCreate. All 30 tasks
ship behind feature flags (`kchat-backend` enables the
production client; `kchat-dev-issuer` stays for local
testing). The local-first sentinel still passes — the new
crate stays out of the editing-path closure even though it
links `reqwest`.

### Block A — KChat backend REST client (Tasks 1–6)
- [x] **Task 1: `kcreate_kchat_client` crate.** New crate
      behind the `kchat-backend` feature flag. HTTPS-only
      `reqwest` + `rustls` lifecycle (`connect` = sign in +
      hydrate token store; `disconnect` = drop tokens +
      attestation). Excluded from the editing-path dep closure
      via the `local_first.rs` deny-list.
- [x] **Task 2: REST surface + DTOs.** Typed request /
      response structs for `/api/v1/auth/{login,refresh}`,
      `/api/v1/me`, `/api/v1/communities`,
      `/api/v1/communities/{id}/members`,
      `/api/v1/communities/{id}/attestation`,
      `/api/v1/communities/{id}/conversations`,
      `/api/v1/conversations/{id}/messages`. Strict TLS;
      `http://` URLs refused outside the in-process `axum`
      fixture used by `kcreate_tests`.
- [x] **Task 3: Transport layer.** 10 s per-request timeout,
      transparent token refresh on 401 (with pre-emptive
      refresh window), capped exponential retry on 429,
      graceful shutdown.
- [x] **Task 4: Attestation bridging.** Maps the backend's
      `/communities/{id}/attestation` response into the
      existing `KChatMembership` / `KChatGroupId` types.
      `KChatBackendAuthority` implements `KChatGroupAuthority`
      and sources its membership live; auto-refresh kicks in
      when the attestation is within 5 minutes of expiry.
      Until the backend ships the attestation endpoint, the
      `kchat-dev-issuer` flag covers the same wire shape.
- [x] **Task 5: Bridge surface.**
      `crates/kcreate_bridge/src/kchat_backend.rs` exposes
      9 N-API entry points (`connect`, `disconnect`, `status`,
      `list_communities`, `select_community`,
      `get_community_members`, `list_conversations`,
      `share_to_conversation`, `accept_invite`). Wire-format
      lockstep through `bridge.ts`, `main.ts`, `preload.ts`,
      `scene.ts` with `KChatBackendStatus`, `KChatCommunity`,
      `KChatCommunityMember`, `KChatConversation` types.
- [x] **Task 6: Client tests.** In-process `axum` REST
      fixture (canned JSON, 401 → refresh → replay, 429
      retry, signature-mismatch / clock-skew / expired
      attestation, endpoint-not-implemented graceful path),
      bridge integration in
      `crates/kcreate_tests/tests/kchat_backend_client.rs`,
      local-first sentinel verified.

### Block B — Channel/Group-Gated Collaboration (Tasks 7–12)
- [x] **Task 7: Community-scoped session start.** `session_start`
      accepts optional `community_id`; mDNS service TXT carries
      it so only LAN peers in the same community auto-discover.
- [x] **Task 8: Roster sync + kick.** 30 s `getCommunityMembers`
      poll. Revoked peer triggers a `Goodbye(Kicked)` + graceful
      QUIC close + `SessionEvent::PeerKicked` event.
- [x] **Task 9: Conversation document sharing.** `SharedDocument`
      invite payload (project_id + owner peer_id + owner pubkey
      + cert fingerprint + community_id + conversation_id) is
      posted via `kchat.conversations.postMessage` as a rich
      card the uney-chat-desktop renderer can display inline.
- [x] **Task 10: Invite acceptance.** `accept_invite` bridge
      entry point parses the JSON payload, verifies sender is
      in the same community, triggers `session_join` against
      the owner. `InvitePanel.tsx` polls the clipboard / local
      invite queue and validates community match before
      enabling the join action.
- [x] **Task 11: Role-based permissions.** Owner / admin →
      `Editor` with kick + ACL-manage privileges, member →
      `Editor` with host-downgradable to `Viewer`. Viewer
      permission rejects `session_broadcast_operations` (and
      `session_queue_operation`) at the bridge layer.
      `CollabPermission` enum in `kcreate_collab::session`.
- [x] **Task 12: Community-gated tests.**
      `crates/kcreate_tests/tests/collab_communities.rs`:
      community-scoped mDNS filtering, kicked-peer cleanup,
      permission enforcement, invite round-trip.

### Block C — Real-Time Collaboration UX (Tasks 13–18)
- [x] **Task 13: Real-time cursor overlay.**
      `apps/desktop/renderer/src/components/CursorOverlay.tsx`
      reads `session_peers()` presence, projects world-space
      cursors through the viewport, renders arrow + label per
      peer with hash-derived high-contrast colours.
- [x] **Task 14: Selection highlight overlay.** Matching
      coloured outline drawn around remote-peer selected
      nodes; reuses the cursor palette so peer identity is
      consistent across overlays. `SelectionOverlay.tsx`.
- [x] **Task 15: Resume bundle for late joiners.**
      `session_request_resume()` sends `ResumeRequest` with
      the local resume vector; the host responds with
      `ResumeBundle` carrying every missing entry. End-to-end
      from `Hello/Welcome::Accepted` through
      `SessionEvent::ResumeApplied`.
- [x] **Task 16: Conflict notification UI.** CRDT resolver
      emits `SessionEvent::ConflictResolved { node_id,
      winner_peer_id, loser_peer_id, field }`. `ConflictToast.tsx`
      surfaces the non-blocking notification + auto-dismiss
      after 5 s + undo link.
- [x] **Task 17: Collaborative undo.** `Operation::is_undo`
      flag (skip-serialised when false → backwards
      compatible). Undo / redo broadcast through the journal
      with the marker; remote peers render "Ken undid …" in
      the activity feed and apply the revert.
- [x] **Task 18: Real-time UX tests.**
      `crates/kcreate_tests/tests/collab_realtime.rs` covers
      cursor projection math, resume flow, conflict event
      emission, collaborative undo broadcast.

### Block D — Security & Privacy Hardening (Tasks 19–24)
- [x] **Task 19: Key rotation.** Default 60-minute QUIC cert
      rotation (configurable via
      `SessionConfig::key_rotation_interval_secs`). New cert
      announced via `Message::KeyRotation { new_cert_fingerprint,
      transition_deadline_ms }`; peers that miss the 30 s
      acknowledgement window are disconnected with
      `key-rotation-timeout`. Manual `session_rotate_keys`
      bridge entry point for ops triggers.
- [x] **Task 20: Audit trail.** `kcreate_audit` extended with
      `AuditEventKind::Collab` variant + every collab
      lifecycle event (`CollabSessionStarted`, `PeerJoined`,
      `PeerKicked`, `KChatDesktopConnected`, etc.) persisted
      to the separate audit SQLite DB. `AuditPanel.tsx`
      surfaces the filter.
- [x] **Task 21: ACL.** `acl.json` in the project metadata
      directory enforces per-peer permission (`editor` /
      `viewer`). Sender's public key checked against the ACL
      on Hello — community membership grants implicit
      `editor` only when no ACL is configured. ACL CRUD via
      `AccessControlPanel.tsx`.
- [x] **Task 22: Rate limiting.** Per-peer 100 ops/s + 20
      presence/s ceilings (configurable via
      `SessionConfig::max_ops_per_second` /
      `max_presence_per_second`). First-strike warning event,
      sustained 3 s violation triggers disconnect.
- [x] **Task 23: Clipboard share.** `Message::ClipboardShare`
      with ChaCha20-Poly1305 ciphertext + 12-byte caller-
      generated nonce. Key is a BLAKE3-derived 32-byte secret
      from the X25519 ECDH shared secret (Ed25519 → X25519
      conversion). Inbound offers surface in
      `pendingClipboardOffers` until the user accepts / rejects.
- [x] **Task 24: Security tests.**
      `crates/kcreate_tests/tests/collab_security.rs`: ACL
      enforcement, rate-limit warning + disconnect, key
      rotation epoch bump + old cert rejection, clipboard
      encryption end-to-end + non-target decryption fails.

### Block E — Performance & Scale (Tasks 25–28)
- [x] **Task 25: Operation batching.** `SessionConfig.{batch_flush_interval_ms,
      batch_flush_max_ops}` (50 ms / 200 ops defaults).
      `session_queue_operation` / `session_flush_pending_operations`
      / `session_tick_outbound_batch` N-API trio. Renderer
      queues per-frame ops + ticks the deadline on the same
      cadence as `session_drain_events`; drag-end flushes
      eagerly. Viewer perm still rejects the queue.
- [x] **Task 26: Lazy presence throttling.**
      `SessionConfig.{presence_min_interval_ms,
      presence_move_threshold_px, presence_idle_suppression_ms}`
      (50 ms / 2 px / 2 s defaults). 20 Hz cap + delta-floor +
      idle suppression on identical payloads. Selection /
      active-page changes always broadcast; cursor moves go
      through the gate.
- [x] **Task 27: Selective sync.** `session_set_active_pages`
      N-API. `SessionState.active_pages` filters the renderer
      event stream for `PresenceUpdated` and `ConflictResolved`
      while operations continue to journal across the whole
      project (resume consistency preserved).
- [x] **Task 28: Performance benchmarks.**
      `crates/kcreate_bridge/benches/collab_perf.rs` (criterion,
      gated on `collab` feature). Covers journal append
      throughput (10-peer round-robin), CRDT merge latency
      (disjoint / overlap / LWW baseline), presence
      serialisation at 1/5/20 peers, 10 000-entry resume
      bundle round-trip, op batching (200 envelopes vs 1
      batch of 200 ops).

### Block F — Documentation & Polish (Tasks 29–30)
- [x] **Task 29: Phase tracking.** This file (`PROGRESS.md`)
      and the changelog entry below. PR #17 description carries
      the per-block task summary.
- [x] **Task 30: Docs sync.** `README.md` gains a
      "Collaboration & KChat Desktop Integration" section.
      `ARCHITECTURE.md` gains a "§ KChat Desktop Integration"
      section (REST client over `reqwest` + `rustls`, the
      `.kcz` companion extension surface, community → collab
      gate mapping, security model, feature flag table).
      `AGENTS.md` indexes the new files
      (`kcreate_kchat_client`, `CursorOverlay.tsx`,
      `SelectionOverlay.tsx`, `InvitePanel.tsx`,
      `AccessControlPanel.tsx`, `ConflictToast.tsx`,
      `AuditPanel.tsx`, `kchat_backend.rs`,
      `apps/kchat-extension/`, `benches/collab_perf.rs`).
      `crates/kcreate_kchat_client/src/protocol.rs` documents
      the REST DTOs, endpoint paths, HTTPS-only invariant,
      authentication flow (login → access/refresh tokens with
      pre-emptive refresh), retry policy (429 with capped
      exponential backoff), error mapping, and the
      `kcreate.invite.v1` content-type schema consumed by the
      companion extension.

## Phase 8 — Production Hardening | Complete | 100%

Phase 8 is the production-hardening sweep that fills the gaps left
by Phases 5–7 in the design-token / layout / brand-hub / image-studio
surfaces and adds the encryption-at-rest, design-review, and
artifact-publishing capabilities the proposal calls for.

### Block A — KChat Artifact Publishing & Design Review (Tasks 1–6)
- [x] **Task 1: Artifact publish pipeline (Rust).**
      `crates/kcreate_kchat_client/src/artifact.rs` implements
      `publish_artifact(ArtifactPublishParams)` and
      `list_artifacts(conversation_id)`. Multipart upload
      (artifact bytes + thumbnail + JSON metadata) to
      `POST /api/v1/conversations/{id}/artifacts`. Typed DTOs
      (`ArtifactKind`, `ArtifactMetadata`, `ArtifactPublishResult`,
      `ArtifactPublishThumbnail`, `PublishedArtifact`) in
      `protocol.rs`. Client-side 50 MiB cap via
      `MAX_ARTIFACT_BYTES` (fails fast before the bytes
      traverse the wire). `ArtifactKind` serialises in
      `camelCase` so the multi-word `BrandKit` variant lands
      on the wire as `"brandKit"` in lockstep with the
      TypeScript mirror. 11 unit tests + 7 integration tests
      in `artifact_round_trip.rs` covering happy path, 401
      token-refresh, 415 unsupported-kind, 413 too-large, 429
      retry, no-thumbnail fallback, and client-side cap.
- [x] **Task 2: Artifact publish bridge surface.**
      `crates/kcreate_bridge/src/kchat_artifact.rs` provides
      `kchat_backend_publish_artifact(conversation_id, request)`,
      `kchat_backend_publish_brand_kit(conversation_id, request)`,
      and `kchat_backend_list_artifacts(conversation_id)`.
      Fail-fast ordering: validate → `project_identity()` →
      `require_client()` → render → publish. Wire-format
      `KChatArtifactRequest` uses `#[serde(tag = "format")]`
      discriminated union (PNG/SVG/PDF/WebP/JPEG/brandKit).
      In-memory export via `export_png_bytes`, `export_svg`,
      `export_pdf_bytes`, `export_webp_bytes`, `export_jpeg_bytes`,
      `brand_kit_export_to_bytes`. Thumbnail reuses
      `thumbnails::ensure_cover_thumbnail(512)`.
      N-API entry points in `lib.rs`; IPC handlers in `main.ts`;
      preload in `preload.ts`; bridge methods in `bridge.ts`;
      TypeScript mirrors (`KChatArtifactKind`,
      `KChatArtifactMetadata`, `KChatPublishedArtifact`,
      `KChatArtifactPublishResult`, `KChatArtifactPublishRequest`,
      `KChatBrandKitArtifactRequest`, `KChatSvgArtifactRequest`,
      `KChatArtifactRequestKind`) in `scene.ts`.
      `kchat_backend_connect_for_tests` helper for integration
      tests. 4 serde wire-shape unit tests + 4 bridge integration
      tests in `crates/kcreate_tests/tests/kchat_artifact.rs`
      (publish round-trip, no-project error, not-connected error,
      empty-conversation-id rejection).
- [x] **Task 4 (Rust core): Design review annotations.**
      `crates/kcreate_core/src/annotation.rs` introduces
      `Annotation { id, page_id, author_peer_id, author_name,
      position, text, timestamp, resolved, thread_id }` plus
      `AnnotationFilter`. Storage in
      `crates/kcreate_storage/src/annotations.rs` (new
      `annotations` table; `upsert_annotation`, `list_all`,
      `list_for_page`, `set_resolved`, `delete_annotation`,
      `load_annotation`). Per-page filtering +
      resolved/unresolved filtering.
- [x] **Task 4 (bridge + collab): Annotation bridge CRUD +
      broadcast.** `crates/kcreate_bridge/src/annotation_bridge.rs`
      exposes `annotation_create`, `annotation_reply`,
      `annotation_list`, `annotation_resolve`, `annotation_delete`
      through the workspace mutex. Each mutation also broadcasts
      via `Message::AnnotationBroadcast` (kind = `Upsert` |
      `Delete`) when a collab session is active — peers apply the
      payload through the same storage helpers via the inbound
      handler in `crates/kcreate_bridge/src/collab.rs::apply_event`,
      and a `SessionEvent::AnnotationsApplied { peer_id, verb,
      count, page_ids }` is emitted so the renderer can refresh
      the overlay. Five N-API entry points in
      `crates/kcreate_bridge/src/lib.rs`; wire-format mirrors
      (`AnnotationBridge`, `AnnotationCreateRequest`,
      `AnnotationReplyRequest`, `AnnotationListRequest`,
      `AnnotationListResponse`, `AnnotationResolveRequest`,
      `Annotation`, `AnnotationPosition`) in
      `apps/desktop/shared/scene.ts`; IPC handlers in
      `apps/desktop/main/src/main.ts`; preload exposure as
      `window.kcreate.annotation.{create,reply,list,resolve,delete}`.
      `crates/kcreate_tests/tests/annotations.rs` covers CRUD
      round-trip, resolve/unresolve, per-page isolation, filter
      contracts, bridge-level CRUD via `annotation_bridge` (8
      `#[serial]` tests including reply-to-unknown-parent error,
      filter modes, double-delete idempotency, threaded
      replies), and the `AnnotationBroadcast` envelope shape
      (upsert + delete kinds round-trip through serde).

### Block B — Missing Image Studio Primitives (Tasks 7–12)
- [x] **Task 7: Perspective transform.** `perspective_transform`
      in `crates/kcreate_raster/src/transform.rs` computes the
      3×3 projective matrix from the 4 destination corners and
      applies inverse mapping with bilinear interpolation,
      row-parallel via rayon. Bridge surface
      `raster_ops::apply_perspective` decodes the layer's PNG
      blob, warps it, re-encodes, and resizes the node's
      `Bounds` to match the new canvas. N-API:
      `raster_perspective(node_id, corners_json)`.
- [x] **Task 8: Color range selection.** `select_by_color_range`
      in `crates/kcreate_ai/src/color_range.rs` produces a
      boolean mask using CIE76 ΔE in Lab space for perceptual
      fuzziness. Row-parallel via rayon.
- [x] **Task 9: HSL adjustment layer + bridge.**
      `AdjustmentLayer::HueSaturation { hue, saturation,
      lightness }` in `crates/kcreate_raster/src/layer.rs`,
      plus the destructive bridge surface
      `raster_ops::apply_hsl` and the live-preview
      `PreviewFilter::Hsl` arm. N-API:
      `raster_apply_hsl(node_id, hue, saturation, lightness)`.
- [x] **Task 10: Color balance adjustment layer + bridge.**
      `AdjustmentLayer::ColorBalance { shadows, midtones,
      highlights }` applies three-way lift/gamma/gain in
      shadow / midtone / highlight tonal ranges using a
      Gaussian-like falloff centred on luminance 0.15 / 0.5 /
      0.85. Bridge surface `raster_ops::apply_color_balance`
      plus the live-preview `PreviewFilter::ColorBalance` arm.
      N-API: `raster_apply_color_balance(node_id, shadows_json,
      midtones_json, highlights_json)`.
- [x] **Task 11: Selection-based filter application.**
      `raster_ops::apply_filter_masked(node_id, filter, mask)`
      runs any `PreviewFilter` variant against a layer's
      pixels but only commits the result where `mask[i] ==
      true`. A 5-tap (centre + N/S/E/W) average produces a
      1-pixel feather at the mask boundary so the seam does
      not alias; fully unmasked pixels are copied bit-exact,
      fully masked pixels take the filtered output verbatim,
      boundary pixels blend on the float weight curve
      (alpha included so transparency reveals naturally).
      Mask shape mismatches surface a structured error
      instead of panicking. N-API:
      `raster_apply_filter_masked(node_id, filter_json, mask)`.
- [x] **Task 12: Image Studio tests.**
      `crates/kcreate_tests/tests/image_studio_advanced.rs`
      covers perspective identity + translation, color range
      fuzziness boundaries, HSL roundtrip identity, color
      balance neutrality, and the `PreviewFilter` wire shape
      (Levels / Curves / Blur / Sharpen / Hsl / ColorBalance
      tags + snake_case fields locked against `scene.ts`).
      Bridge integration tests in
      `crates/kcreate_bridge/src/raster_ops.rs::tests` cover
      perspective canvas growth, HSL identity vs. hue-rotate,
      color balance identity, masked-filter edge cases
      (wrong-size mask error, all-false mask preserves blob,
      all-true mask rewrites blob, operation log captures
      `{mask_len, mask_true}`) plus the feather-kernel
      math (zero / one / boundary 0.2 / 0.8).

### Block C — Missing Layout Studio & Brand Hub Features (Tasks 13–18)
- [x] **Task 13: Page-numbering tokens.**
      `crates/kcreate_text/src/tokens.rs` introduces the
      Unicode Private-Use sentinel U+E100 + format selector
      (`PageNumberFormat::{Arabic, RomanLower, RomanUpper,
      AlphaLower, AlphaUpper}`). The shaper expands tokens
      against a `PageContext` produced by the resolver.
      Roman / Alpha conversion is implemented as real
      algorithms (subtractive Roman, A–Z…AA–AZ alpha).
- [x] **Task 14: Section-based page numbering.** `PageLayout`
      carries `section_start: Option<u32>` and
      `section_prefix: Option<String>`.
      `resolve_page_contexts` walks pages in order, applies
      section restarts, and stamps `display_number` on each
      `PageContext`.
- [x] **Task 15: Brand kit versioning.**
      `crates/kcreate_storage/src/brand_versions.rs` adds the
      `brand_kit_versions` SQLite table and the
      `save_brand_kit_version`, `list_brand_kit_versions`,
      `restore_brand_kit_version`, `diff_brand_kit_versions`
      surface. The diff is a structured
      `BrandKitDiff { added_colors, removed_colors,
      changed_colors, added_fonts, removed_fonts,
      name_changed }`. `crates/kcreate_tests/tests/brand_versioning.rs`
      exercises save / list / restore / diff round-trips.
- [x] **Task 17: Job-first export presets.**
      `crates/kcreate_export/src/job_presets.rs` curates a
      preset list per Home-screen job tile: AppOrWebsiteUi
      (PNG @1x/@2x/@3x + SVG sprite + CSS export),
      LogoIconOrBrandKit (SVG clean + PNG favicon set + iOS
      PDF + Android XML), SocialMediaPost (1080² + 1080×1920
      + 1200×630), ProductPhotoCleanup (transparent PNG +
      white-bg JPEG + WebP), PitchDeckOrProposal (16:9 + A4
      PDF), FlyerPosterOrBrochure (300 dpi PDF with bleed +
      web PNG), DeveloperAssetExport (SVG sprite + density
      buckets + CSS variables).
      `crates/kcreate_tests/tests/job_presets.rs` asserts
      every job type returns a non-empty curated set and
      that every preset's scale is positive.

### Block D — Design Studio Polish & Missing Features (Tasks 19–24)
- [x] **Task 19: Constraint system for responsive frames.**
      `crates/kcreate_core/src/node.rs` defines
      `Constraints { horizontal, vertical }` with the 6
      axis modes (`Fixed`, `Min`, `Max`, `Center`, `Scale`,
      `Stretch`).
      `crates/kcreate_layout/src/constraints.rs` ships
      `apply_constraints(child_bounds, child_constraints,
      parent_old, parent_new) -> Bounds` and the
      `crates/kcreate_bridge/src/phase8.rs::document_resize_frame`
      walks the resized frame's children and rewrites each
      child's bounds.
      `crates/kcreate_tests/tests/constraints.rs` exercises
      every axis mode, parent-resize propagation, and
      no-op when the parent is unchanged.
- [x] **Task 21: Design token propagation.** `NodeStyle`
      gains `token_bindings: BTreeMap<String, String>` and
      `crates/kcreate_core/src/token_binding.rs` exposes
      `bind_token`, `unbind_token`, `refresh_style`,
      `propagate_token_changes`, and the targeted
      `propagate_single_token`. The bridge entry point
      `phase8::document_propagate_token` walks every node
      bound to the named token and rewrites it; the 1000-node
      benchmark in
      `crates/kcreate_tests/tests/token_binding.rs` confirms
      it stays under the 100 ms PROPOSAL.md §4.6 budget.
- [x] **Task 23: Smart text auto-fit.**
      `crates/kcreate_text/src/autofit.rs` exposes
      `compute_autofit_size(text, font, min, max, frame)`
      which binary-searches for the largest font size that
      fits the supplied text inside the frame without
      overflow, using the existing shaper for measurement.
      `crates/kcreate_tests/tests/text_autofit.rs` covers
      binary-search convergence, min/max clamping, and the
      identity case (text already fits).
- [x] **Task 24: Design Studio tests.** Covered by the three
      test modules above.

### Block E — Performance, Security & Production Hardening (Tasks 25–28)
- [x] **Task 27: Cold-path startup profiling crate
      `kcreate_perf`.** New workspace member (no networking,
      no async, only `serde` + `serde_json` deps; safe to live
      in the editing-path closure walked by `local_first.rs`).
      Three primitives: `Timeline` (append-only sequence of
      named time marks, each tagged with a monotonic
      nanosecond offset from `Instant::now()`), `Scope` (RAII
      span that auto-marks `<label>.end` on drop and is
      idempotent under explicit `Scope::end`), and `Report`
      (serde-ready JSON snapshot with derived `phases`). The
      `startup` module owns a process-wide
      `OnceLock<Mutex<Option<Timeline>>>` keyed on
      `"startup"`; `ensure_initialized` is idempotent and
      callable from any hot path. The bridge wires it on
      first touch and drops marks at `bridge.first_call` (first
      perf API use after `process.dlopen` — a true load-time mark
      would need an `unsafe` ctor), `project_create.{start,end}`, and
      `project_open.{start,end}`. The renderer drops its own
      `first_paint` / `first_interactive` marks on the same
      monotonic clock via `runtime_startup_mark`. N-API
      surface: `runtime_startup_timeline` (JSON Report) and
      `runtime_startup_mark(label)`. 11 unit tests pin the
      monotonic-order, scope idempotency, snapshot
      non-consuming, JSON round-trip, and singleton
      idempotency invariants; 6 bridge-level tests verify
      that `bridge.first_call` is emitted exactly once across
      repeated `ensure_startup_initialized` calls.
- [x] **Task 28: Tile-cache LRU eviction in
      `kcreate_raster::tile_cache`.** `TileCache<K>` is a
      bounded LRU store of decoded `Tile`s keyed on an
      opaque caller-supplied key (the bridge instantiates
      `TileCache<(Uuid, u32, u32)>` for `(layer_id, col,
      row)`). Memory accounting tracks raw pixel bytes
      (`tile.pixels.len()`); eviction policy is
      least-recently-used by a monotonic tick counter bumped
      on every read and write. Oversized inserts (a single
      tile larger than the budget) never evict the
      most-recently-used entry — the cache briefly goes over
      budget and the next insert reclaims room. Wire-up: the
      bridge owns a process-wide singleton at
      `kcreate_bridge::perf::tile_cache_lock`, seeded from
      `RuntimeConfig::effective_raster_cache_mb` and
      re-synced whenever `low_resource_mode_set` flips. N-API
      surface: `runtime_tile_cache_stats` (`{bytes,
      entries, budget_bytes}`) and `runtime_tile_cache_clear`
      (returns evicted count). 12 unit tests in the data
      crate pin hit/miss/replace/clear/budget-shrink/oversized-
      insert behaviour; 4 bridge tests verify that the budget
      tracks `RuntimeConfig`, that `clear` drains in LRU
      order, and that the snake_case JSON round-trips.
- [x] **Task 25: SQLCipher encryption at rest.**
      `crates/kcreate_storage/src/crypto.rs` derives a
      256-bit key from a user passphrase via PBKDF2-HMAC-SHA256
      with a per-project salt (200 000 iterations per OWASP
      2023). `ProjectStore::open_encrypted`,
      `encrypt_existing`, `change_key`, and the recovery
      escape hatch `export_unencrypted` round-trip a real
      `BrandKit` payload in
      `crates/kcreate_tests/tests/encryption.rs`. Unencrypted
      projects continue to work; the salt is persisted in
      `manifest.json` so the project can survive a key
      rotation without re-importing assets.

### Block C UI — React panels (slice A)
- [x] **AnnotationOverlay** (`apps/desktop/renderer/src/components/AnnotationOverlay.tsx`)
      — design-review pins + threaded replies overlay on
      `CanvasHost`. World-space projection mirrors
      `CursorOverlay`. Wired into `EditorPage` above
      `SelectionOverlay` with `allowCreate` gated on `mode ∈
      {design, layout}`. Calls `window.kcreate.annotation.*`
      (Task 5).
- [x] **ArtifactPublishPanel** (`ArtifactPublishPanel.tsx`)
      — publish PNG/PDF/WebP/JPEG/SVG/BrandKit artifacts to a
      KChat community and list recent artifacts. Lives under
      the new RightPanel "Publish" tab. Wired to
      `window.kcreate.kchatBackend.*` (Task 3).
- [x] **BrandVersionPanel** (`BrandVersionPanel.tsx`) — save
      / list / restore / diff brand-kit versions, with a
      colour-aware diff view. Lives under the LeftPanel
      "Brand" tab alongside `BrandKitEditor`. Wired to
      `window.kcreate.phase8.brandKit*` (Task 16).
- [x] **ConstraintsPanel** (`ConstraintsPanel.tsx`) — per-
      node horizontal + vertical constraint editor with a
      live SVG visualiser of the resize behaviour. Lives
      under the new RightPanel "Constraints" tab. Wired to
      `window.kcreate.phase8.{nodeConstraints,
      setNodeConstraints}` (Task 20). The bridge surface +
      N-API wrapper + IPC layer are added in lockstep
      (`document_node_constraints`,
      `document_set_node_constraints`).
- [x] **TokenBindingControl** (`TokenBindingControl.tsx`) —
      bind / unbind / propagate design tokens to a node's
      properties (fill, stroke, corner radius, padding,
      gap…). Filters available tokens by property kind so a
      colour property only sees colour tokens. Lives under
      the new RightPanel "Tokens" tab. Wired to
      `window.kcreate.phase8.{bindToken, unbindToken,
      propagateToken, nodeTokenBindings}` (Task 22). The
      `nodeTokenBindings` read method is added in this slice
      alongside the constraint read methods.
- [x] **EncryptionPanel** (`EncryptionPanel.tsx`) — project
      encryption status, enable-encryption flow with a
      passphrase-strength meter, change-passphrase rotation,
      and plaintext recovery export. Lives under the new
      RightPanel "Encryption" tab. Wired to a fresh
      `window.kcreate.projectEncryption.*` bridge backed by
      `kcreate_bridge::encryption` (which composes
      `ProjectStore::{enable_encryption, change_passphrase,
      export_plaintext_recovery, is_encrypted}` — already
      shipped in Task 25) and surfaces
      `crypto::passphrase_strength` to the renderer (Task
      26). End-to-end test
      `encryption::tests::enable_change_export_round_trip`
      drives the full enable → status → rotate → export
      cycle against SQLCipher.

### Block F — Documentation & Polish (Tasks 29–30)
- [x] **Task 29: Phase tracking.** This file (`PROGRESS.md`).
- [x] **Task 30: Docs sync.** `ARCHITECTURE.md` adds an
      annotation/token-binding/constraint/SQLCipher section.
      `AGENTS.md` indexes the new modules
      (`annotation.rs`, `color_range.rs`, `constraints.rs`,
      `autofit.rs`, `brand_versions.rs`, `job_presets.rs`,
      `tokens.rs`, `crypto.rs`, `phase8.rs`).

### Phase 8 — Bridge & wire-format lockstep
- [x] **`crates/kcreate_bridge/src/phase8.rs`** owns the
      workspace-level helpers for token binding, constraint
      resize, autofit, page numbering, section pages, job
      presets, and brand-kit versioning.
- [x] **`crates/kcreate_bridge/src/lib.rs`** exposes 13 new
      N-API entry points (`document_bind_token`,
      `document_unbind_token`, `document_propagate_token`,
      `document_resize_frame`, `text_set_auto_fit`,
      `page_number_token`, `page_set_section`,
      `page_resolve_contexts`, `export_job_presets`,
      `brand_kit_save_version`, `brand_kit_list_versions`,
      `brand_kit_restore_version`, `brand_kit_diff`).
- [x] **`apps/desktop/shared/scene.ts`** mirrors the new
      types (`Phase8Bridge`, `PageNumberFormat`, `PageContext`,
      `JobType`, `JobExportPreset`, `JobExportPresets`,
      `BrandKitVersionInfo`, `BrandKitDiff`, `ResizeFrameBounds`).
- [x] **`apps/desktop/preload/src/preload.ts`** and
      **`apps/desktop/main/src/{bridge,main}.ts`** wire the
      IPC handlers + Bridge interface.

## Phase 9 — KChat extension depth, Home screen, design studio polish | Complete | 100%

Phase 9 closes the proposal-level gaps that Phase 8 deferred:
the KChat companion extension, the "Start from a brief" Home
screen, the AI palette / trace / icon-ify actions, the
ruler + grid + alignment surfaces in Design Studio, the PSD /
Penpot / EXIF / SVG-preview import edges, and the
memory-pressure + autosave + export-validation robustness layer.

### Block A — Phase 8 close-out + KChat extension depth (Tasks 1–6)
- [x] **Task 1: Mark Phase 8 Complete.** PROGRESS / PHASES headers
      flipped from "In Progress" to "Complete | 100%".
- [x] **Task 2: `ProjectBrowserPanel.tsx`.** Renders the current
      community's KCreate projects sourced from
      `kchat_backend_list_artifacts`; clicking a card fires
      `openDeeplink('kcreate://open?project_id=…')`.
- [x] **Task 3: `ArtifactCard.tsx` + KCreate-side artifact
      deeplink.** Rich preview (thumbnail / format badge / size /
      "Open in KCreate") for `kcreate.invite.v1` and
      `kcreate.artifact.v1` content-type messages. The KCreate
      `main.ts` dispatcher now handles
      `kcreate://artifact?id=…` by navigating to the project and
      highlighting the artifact in the export panel.
- [x] **Task 4: `SessionStatusBadge.tsx`.** Deeplink-probe based
      green-dot / grey-dot status badge — the extension can't
      directly read KCreate's process state.
- [x] **Task 5: `ActivityFeed.tsx`.** Recent design activity
      derived from `kchat_backend_list_artifacts` + conversation
      message history, with deeplinks back into KCreate.
- [x] **Task 6: Tests + build pipeline.**
      `apps/kchat-extension/tests/phase9-panels.test.mjs` covers
      all four new panels via a fake `__kchatHost`. ESLint +
      TypeScript strict both clean.

### Block B — Home Screen & AI Workflow Completion (Tasks 7–12)
- [x] **Task 7: "Start from a brief" tile.** `BriefModal.tsx`
      collects the user's brief and submits a structured GBNF
      prompt to the local LLM sidecar. The result feeds
      `brief_to_project` in `kcreate_bridge::phase9` which
      orchestrates the artboard preset, brand kit upsert, and
      starter-layer creation against the *currently open*
      project (no implicit "new project" path).
- [x] **Task 8: Model status + GPU tier badge.** `ModelStatusGrid`
      on the Home page surfaces device tier, GPU backend name
      (via the new `runtime_gpu_backend_name` N-API entry
      in `kcreate_bridge::perf`), LLM sidecar status, and the
      installed-pack count.
- [x] **Task 9: Help & Learn grid.** Getting Started,
      keyboard-shortcuts cheat sheet, CHANGELOG viewer, and the
      power-user links to PROPOSAL / ARCHITECTURE.
- [x] **Task 10: AI palette extraction.** AIAssistPanel offers
      "Extract palette from image" against the selected raster
      layer; `palette_extract_and_apply_brand_kit(node_id,
      num_colors)` in `kcreate_bridge::phase9` materialises the
      result as a brand-kit upsert.
- [x] **Task 11: Copy-fit text on layer resize.** The
      `document_update_node` path now detects autofit text and
      re-runs `kcreate_text::autofit::compute_autofit_size`
      atomically with the bounds change. Regression covered by
      `crates/kcreate_tests/tests/text_autofit_on_resize.rs`.
- [x] **Task 12: AI trace-to-vector.** `kcreate_ai::trace`
      implements RGBA→grayscale→Otsu/fixed threshold→
      Moore-neighbour contour tracing→RDP simplification. Wired
      via `ai_trace_raster` in `kcreate_bridge::phase9` which
      appends a sibling group of vector-path nodes carrying the
      traced polyline metadata.

### Block C — Import / Export Gaps (Tasks 13–18)
- [x] **Task 13: PSD layered raster import.**
      `kcreate_export::psd_import` parses PSD files with the
      `psd` crate, materialises layer pixels into the blob store
      as PNGs, and emits a `RasterImage` node tree with mapped
      blend modes and groups. Bridge: `import_psd(path)`.
- [x] **Task 14: EXIF preservation.**
      `kcreate_export::exif` extracts EXIF on import via
      `kamadak-exif` and stores it on the node metadata; the
      JPEG / WebP exporters re-embed the JSON-encoded metadata
      on round-trip.
- [x] **Task 15: Penpot best-effort import.**
      `kcreate_export::penpot_import` parses Penpot `.penpot`
      zip bundles into artboards / frames / shapes; embedded
      assets land in the blob store. Bridge: `import_penpot`.
- [x] **Task 16: `resvg` SVG-to-raster preview.**
      `kcreate_export::svg_preview` rasterises SVG payloads with
      `resvg`. Bridge: `export_svg_preview(node_ids, w, h)`.
- [x] **Task 17: History panel + operation log filter.**
      `HistoryPanel.tsx` renders the operation log with AI /
      manual filter chips, "Jump to" selection, and "Undo to
      here". Backed by `document_operation_log` in
      `kcreate_bridge::phase9` + the audit-trail filter helper.
- [x] **Task 18: Import pipeline tests.** Integration coverage
      in `crates/kcreate_tests/tests/` for the PSD / Penpot /
      EXIF / SVG-preview paths.

### Block D — Vector Studio & Design Studio Polish (Tasks 19–24)
- [x] **Task 19: AI icon-ify.** `kcreate_ai::iconify` packs a
      vector selection into a normalised grid (24 / 48 / etc.),
      simplifies with RDP, and emits paths with a recommended
      stroke width. Bridge: `ai_iconify(node_ids, grid_size)`.
- [x] **Task 20: Batch alt-text generation.** AIAssistPanel's
      "Generate alt-text for all images" action calls
      `ai_batch_alt_text(page_id)`; each image gets a
      VLM-derived alt-text stored on `node.metadata["alt_text"]`.
- [x] **Task 21: Ruler + measurement guides overlay.**
      `RulerOverlay.tsx` renders pixel-aligned rulers that
      drag-create guides. Guides persist in the new
      `kcreate_storage::guides` table; the snap engine treats
      guide lines as snap targets. Bridge: `guide_create`,
      `guide_list`, `guide_delete`.
- [x] **Task 22: Grid overlay.** `GridOverlay.tsx` renders a
      per-artboard pixel grid toggleable with Ctrl+'. Grid
      settings persist on the artboard node. Bridge:
      `artboard_grid_settings`, `artboard_set_grid`.
- [x] **Task 23: Multi-select alignment + distribution.**
      `kcreate_core::align` implements the alignment +
      distribution math (LWW-safe per-node `dx/dy`). Bridge:
      `document_align`, `document_distribute`. UI in
      `AlignmentToolbar.tsx`.
- [x] **Task 24: Design Studio + Vector Studio tests.**
      10 serial integration tests in
      `crates/kcreate_tests/tests/design_studio_polish.rs`
      covering alignment math, distribution, guide lifecycle,
      and grid validation. 6 algorithm tests for trace +
      iconify in `crates/kcreate_tests/tests/trace_and_iconify.rs`.

### Block E — Performance, Security & Robustness (Tasks 25–28)
- [x] **Task 25: Memory pressure watchdog.**
      `kcreate_bridge::perf::memory_watchdog_start` polls the
      host's available RAM every 5 s (configurable). On entering
      pressure it clears the tile cache and emits
      `MemoryPressureEvent::Entered`; on releasing it emits
      `Released`. Events queue (capped at 32) and the renderer
      drains them via `drain_memory_events`.
- [x] **Task 26: Project autosave with crash recovery.**
      `kcreate_bridge::autosave` spawns an opt-in background
      thread that calls `project_save` whenever `modified_at`
      advances and writes an autosave marker per tick. Recovery
      surfaced via `autosave_recovery_available`,
      `autosave_recover`, `autosave_dismiss_recovery`.
- [x] **Task 27: Export validation + error reporting.**
      `kcreate_export::validate::validate_export_request` checks
      dimensions, format, JPEG quality, missing fonts, and
      surfaces issues with `ExportSeverity::{Error, Warning}`.
      Bridge: `export_validate(request)`.
- [x] **Task 28: Stress / robustness tests.** Memory-pressure
      queue cap + drain order tests, autosave round-trip,
      export-validation regression covered in
      `crates/kcreate_tests/tests/phase9_robustness.rs`.

### Block F — Documentation & Polish (Tasks 29–30)
- [x] **Task 29: PROGRESS.md + PHASES.md updated.** Phase 8
      moved to Complete; Phase 9 section added with every
      task checkbox accounted for.
- [x] **Task 30: README / ARCHITECTURE / AGENTS sync.** Phase 9
      modules added to the AGENTS "Where new code goes" table
      (`trace.rs`, `iconify.rs`, `psd_import.rs`,
      `penpot_import.rs`, `svg_preview.rs`, `autosave.rs`,
      `validate.rs`, `phase9.rs`, `HistoryPanel.tsx`,
      `RulerOverlay.tsx`, `GridOverlay.tsx`,
      `AlignmentToolbar.tsx`).

### Phase 9 — Bridge & wire-format lockstep
- [x] **`crates/kcreate_bridge/src/phase9.rs`** owns the
      workspace-level helpers for brief→project, AI trace /
      iconify / palette / alt-text, PSD / Penpot import,
      SVG preview, history filter, alignment + distribution,
      guide lifecycle, grid settings, and export validation.
- [x] **`crates/kcreate_bridge/src/lib.rs`** exposes the new
      N-API entry points (`brief_to_project`,
      `palette_extract_and_apply_brand_kit`, `ai_trace_raster`,
      `ai_iconify`, `ai_batch_alt_text`, `import_psd`,
      `import_penpot`, `export_svg_preview`, `export_validate`,
      `document_operation_log`, `document_align`,
      `document_distribute`, `guide_create`, `guide_list`,
      `guide_delete`, `artboard_grid_settings`,
      `artboard_set_grid`, `runtime_gpu_backend_name`,
      `memory_watchdog_start`, `memory_watchdog_stop`,
      `drain_memory_events`, `autosave_start`,
      `autosave_force_now`, `autosave_status`,
      `autosave_recovery_available`, `autosave_recover`,
      `autosave_dismiss_recovery`).
- [x] **`apps/desktop/shared/scene.ts`** mirrors the new
      types (`BriefApplyResult`, `TraceResult`,
      `IconifyResultInfo`, `PsdImportResult`, `PenpotImportResult`,
      `SvgPreviewResult`, `ExportValidationReport`,
      `OperationLogEntry`, `AlignDeltaInfo`, `GuideInfo`,
      `GridSettings`, `MemoryPressureEvent`,
      `AutosaveStatusInfo`, `AutosaveMarkerInfo`).
- [x] **`apps/desktop/preload/src/preload.ts`** and
      **`apps/desktop/main/src/{bridge,main}.ts`** wire the
      IPC handlers + Bridge interface.

## Changelog

- **2026-05-29 (PR #25)** — Phase 9: KChat extension depth
  (project browser, artifact preview cards, session status,
  activity feed in `apps/kchat-extension/src/`), Home screen
  "Start from a brief" + model-status / Help-and-Learn tiles
  (`BriefModal.tsx`, `HomePage.tsx`), AI palette / trace /
  iconify / batch alt-text (`kcreate_ai::trace`,
  `kcreate_ai::iconify`, `kcreate_bridge::phase9`), PSD /
  Penpot / EXIF / SVG-preview import edges
  (`kcreate_export::{psd_import, penpot_import, svg_preview,
  exif, validate}` + `psd`, `kamadak-exif`, `resvg`
  dependencies), History panel + operation-log filter
  (`HistoryPanel.tsx`), Design Studio ruler / grid /
  alignment + distribution (`RulerOverlay.tsx`,
  `GridOverlay.tsx`, `AlignmentToolbar.tsx`,
  `kcreate_core::align`, `kcreate_storage::guides`),
  memory-pressure watchdog
  (`kcreate_bridge::perf::memory_watchdog_start`), autosave +
  crash recovery (`kcreate_bridge::autosave`), export
  validation (`kcreate_export::validate`). 32 new integration
  tests in `crates/kcreate_tests/tests/`
  (`brief_to_project.rs`, `design_studio_polish.rs`,
  `phase9_robustness.rs`, `trace_and_iconify.rs`).
  `local_first.rs` sentinel stays green — none of the new
  dependencies pull networking into the editing-path closure.
- **2026-05-28** — Phase 8 (in progress): production-hardening
  sweep — design-review annotations
  (`kcreate_core::annotation`,
  `kcreate_storage::annotations`), brand-kit versioning
  (`kcreate_storage::brand_versions`), SQLCipher encryption
  at rest with PBKDF2-HMAC-SHA256 key derivation
  (`kcreate_storage::crypto`), perspective transform + color
  range selection + color balance / HSL adjustment regression
  coverage (`kcreate_raster`, `kcreate_ai::color_range`),
  page-numbering tokens with section restart
  (`kcreate_text::tokens`), constraint system for responsive
  frames (`kcreate_layout::constraints`), design-token binding
  with sub-100 ms propagation
  (`kcreate_core::token_binding`,
  `kcreate_bridge::phase8::document_propagate_token`),
  smart text auto-fit (`kcreate_text::autofit`), job-first
  export presets (`kcreate_export::job_presets`).
  KChat artifact publishing pipeline
  (`kcreate_kchat_client::artifact` + `kcreate_bridge::kchat_artifact`):
  in-memory export → multipart upload → thumbnail → metadata,
  wired end-to-end through N-API, IPC, and TypeScript mirrors;
  discriminated union `KChatArtifactRequestKind` with serde
  `#[serde(tag = "format")]`.
  All Phase 8 features are wired through
  `crates/kcreate_bridge/src/phase8.rs` + 16 N-API entry
  points and mirrored in `apps/desktop/shared/scene.ts`
  (`Phase8Bridge` + `KChatBackendBridge`). 10 new integration
  test modules in `crates/kcreate_tests/tests/`.
  `local_first.rs` sentinel stays green.
- **2026-05-27 (PR #17)** — Phase 7: KChat Desktop
  (`uneycom/uney-chat-desktop`) integration, community-gated
  collaboration, real-time UX, security hardening, performance
  optimisation. New `kcreate_kchat_client` crate ships an
  HTTPS REST client behind the `kchat-backend` feature flag
  (Option C); the bridge installs membership attestations
  from the shared KChat / Mattermost backend that
  `uneycom/uney-chat-desktop` also signs in to. A thin `.kcz`
  companion extension (`apps/kchat-extension/`) renders a
  sidebar inside KChat Desktop and bridges deeplinks back to
  KCreate.
  Community-aware mDNS, community-member roster sync + kick,
  conversation document-sharing invites, ACL persistence
  (`<project_dir>/acl.json`), per-peer rate limiting,
  ChaCha20-Poly1305 encrypted clipboard share over a
  BLAKE3-derived X25519 session key, 60-minute QUIC cert
  rotation, op batching (50 ms / 200 ops cap), lazy presence
  throttling (20 Hz min interval + 2 px delta floor + 2 s
  idle suppression), selective sync via
  `session_set_active_pages`. Renderer overlays:
  `CursorOverlay`, `SelectionOverlay`, `ConflictToast`,
  `InvitePanel`, `AccessControlPanel`, `AuditPanel`. Criterion
  perf bench `collab_perf.rs` covering journal append, CRDT
  merge, presence serialisation, 10 k-entry resume bundle,
  batching round-trip. Local-first sentinel still passes.

- **2026-05-26 (PR #16)** — Phase 3 completion + Phase 6 production
  polish (Tasks 1-30 in one batch):
  - **Phase 3 close-out (Tasks 1-12):** operational CRDT layer on
    `Operation` (`kcreate_collab::crdt`), Pantone-style spot-color
    catalogs + `OverprintTable` + `Trapping` preflight + PDF
    overprint ExtGState writing, ONNX model-pack installer with
    BLAKE3 hash gate (ESRGAN upscale + SAM segmentation paths),
    local template marketplace (`kcreate_core::marketplace` +
    `TemplateMarketplace.tsx`).
  - **Phase 6 (Tasks 13-30):** `kcreate_audit` crate (separate
    SQLite audit DB + `AuditPanel.tsx`), undo/redo improvements
    (group undo / redo + drag coalescing + `ApplyPatchSnapshot`
    atomic rollback with lockstep-tested
    `APPLY_PATCH_COMMANDS` invariant + coalescing
    thumbnail pre-warm), lazy thumbnail generation pipeline
    (`crates/kcreate_bridge/src/thumbnails.rs`), Figma + Sketch
    JSON importers (`kcreate_export::{figma_import,
    sketch_import}`), JSON-backed keyboard shortcut registry with
    module-scope stable bindings for `useSyncExternalStore`,
    CSS-variable-driven dark mode (`:root[data-theme="dark"]`
    cascade, no JS palette duplicate), OS file-manager drag/drop
    + cross-artboard clipboard paste, layer-panel search +
    layer-colour tagging (`layer_color_set` op through undo log),
    E2E workflow tests covering all five PROPOSAL.md §5 user
    journeys, acceptance-criteria bench suite (`cold_start`,
    `raster_open_64mp`, `viewport_pan`,
    `batch_50_assets` sequential + parallel).
  - **Iterative Devin Review hardening (PR #16):** shadow tokens
    converted to CSS-variable references, error-state colours
    centralised in tokens.ts with light/dark CSS variables,
    `useSyncExternalStore` resubscribe churn eliminated, theme
    palette duplication removed, ESRGAN edge-tile cropping fix,
    drag-drop fallback accepts `.gif` + explicit SVG error,
    structural `APPLY_PATCH_COMMANDS` safeguard + lockstep test,
    thumbnail pre-warm coalescing gate.

- **2026-05-25 (Phase 5 follow-up)** — Phase 5 bridge gap closure:
  full bridge wiring for the path-effect chain
  (`vector_apply_path_effect` / `vector_clear_path_effects` →
  `apply_path_effects` in `scene_sync.rs`, applied in render
  order with `Dash` always last), text frame linking
  (`text_frame_link` / `text_frame_unlink` /
  `text_frame_set_wrap`), slice CRUD + parallel export
  (`slice_create` / `slice_update` / `slice_delete` /
  `slice_list` / `slice_export_all`), `.kbrand` round-trip
  (`brand_kit_export` / `brand_kit_import` resolving asset blobs
  through `ProjectStore::store_asset_with_id` /
  `ProjectStore::load_asset`), spot-color convenience entries
  (`color_add_spot`, `node_set_overprint`), and the multi-fill /
  multi-stroke stack editor (RightPanel `ExtraFillsList` with
  add / remove / reorder backed by
  `document_node_extra_fills` / `document_node_extra_strokes`
  readers + `UpdateNodeProps.extra_fills` / `extra_strokes`).
- **2026-05-25 (PR #14)** — Phase 5 ship: Image Studio filters
  (Levels, Curves, Gaussian / Box blur, Unsharp mask, Crop /
  Rotate / Flip, Healing brush + raster_ops bridge + FiltersPanel
  UI), Vector Studio (snap engine + smart-guides overlay, simplify
  / smooth / offset, variable stroke profile, multi-fill /
  multi-stroke per node, dash + round-corners path effects),
  Layout Studio + Brand Hub (linked-frame text flow, image-text
  wraps, `.kbrand` round-trip, slice export, `Color::Spot` +
  `Overprint` + `SpotColorMissing` preflight), full cross-crate
  integration tests in `kcreate_tests`, comprehensive docs sync
  (PROGRESS / ARCHITECTURE / README / AGENTS / CONTRIBUTING).
- **2026-05-25 (PR #12)** — Phase 4 follow-ups: PDF preflight
  DPI floor + bleed-area content checks, scene_sync batch
  optimisation, lock-aware FillSection (solid + gradient editor),
  OCR text-region detection → text-layer creation flow, KChat
  trusted-issuer allowlist surface.
- **2026-05-25 (PR #11)** — Devin Review follow-up fixes
  (NumberField clamp, NodeInfo version probe, FillSection hydrate,
  trust-store probe, OCR area guard, mask-size guard +
  missing_glyphs docstring, seed-race + countdown, wire-format
  `DevInstallRequest`↔`KChatInstallRequest` lockstep).
- **2026-05-25 (PR #10)** — Renderer + AI inference + collab
  transport: radial / linear gradient scene objects, AI inference
  UX end-to-end (alt-text + layout-suggest), PDF preflight
  revision (shading patterns, glyph coverage, total ink coverage),
  scene-sync multi-peer micro-benchmarks + batched insert, KChat
  client integration (`kcreate_kchat` crate + dev-mint IPC +
  `KChatSignInPanel`).
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
