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

## Phase 10 — Image Studio AI, Vector/Layout AI, Export AI, Brand Hub & Plugin Marketplace, Performance Hardening | Complete | 100%

### Block A — Image Studio AI Actions (Tasks 1–6)
- [x] **Task 1: AI denoise.** Non-local-means denoiser in
      `crates/kcreate_ai/src/denoise.rs` (search-window patch
      weighting, row-parallel via rayon). Bridge:
      `ai_denoise(node_id, strength, search_radius, patch_radius)`
      in `crates/kcreate_bridge/src/phase10.rs`. Records an
      undoable operation. UI surface in `AIAssistPanel.tsx`
      under "AI Filters". Tests in
      `crates/kcreate_tests/tests/image_studio_ai.rs`.
- [x] **Task 2: AI object removal / inpainting.** Exemplar-based
      PatchMatch in `crates/kcreate_ai/src/inpaint.rs`
      (multi-scale pyramid, parameterised patch size +
      iterations). Bridge: `ai_inpaint(node_id, mask_json)`.
      UI: "Remove Object" with rectangular/lasso mask input.
- [x] **Task 3: AI auto color correction.** Histogram
      equalisation + gray-world white balance + auto levels in
      `crates/kcreate_ai/src/auto_color.rs`. Modes:
      `AutoLevels | WhiteBalance | HistogramEqualization |
      Combined`. Bridge: `ai_auto_color(node_id, mode)`.
- [x] **Task 4: AI segmentation-based selection tool.** SAM
      bridge with BFS smart-select fallback in
      `crates/kcreate_bridge/src/phase10.rs::ai_segment_at_point`.
      Positive + negative prompts surfaced via
      `AIAssistPanel.tsx` "Smart Segment" action.
- [x] **Task 5: Magic wand tool.** Dedicated tool mounted via
      `apps/desktop/renderer/src/components/MagicWandTool.tsx`,
      bridge `ai_smart_select_at_point(node_id, x, y,
      tolerance, mode)` with `Replace | Add | Subtract`
      cumulative mask semantics.
- [x] **Task 6: Image Studio AI tests.** Coverage in
      `crates/kcreate_tests/tests/image_studio_ai.rs` (12
      tests: SNR improvement on synthetic noise, rectangular
      mask inpaint, histogram bounds, segment fallback,
      tolerance boundary, add/subtract modes).

### Block B — Vector Studio & Layout Studio AI Features (Tasks 7–12)
- [x] **Task 7: AI "match this stroke style".**
      `crates/kcreate_ai/src/stroke_match.rs` extracts stroke
      properties (width, dash, cap, join, profile, colour)
      from a source vector node and applies them to targets.
      Bridge: `ai_match_stroke(source_node_id,
      target_node_ids_json)`.
- [x] **Task 8: AI "extract glyph from photo".**
      `crates/kcreate_ai/src/glyph_extract.rs` runs the trace
      pipeline on a cropped raster region, simplifies +
      normalises to a 1000-unit em-square. Bridge:
      `ai_extract_glyph(node_id, region_json, em_size)`.
- [x] **Task 9: AI "Reformat content into a 16:9 deck".**
      `crates/kcreate_ai/src/reformat.rs` drives the LLM
      sidecar with a GBNF grammar to propose per-page node
      placements. Bridge: `ai_reformat_to_deck(page_id)`.
- [x] **Task 10: AI "fit brief into a one-pager".**
      `crates/kcreate_ai/src/one_pager.rs` materialises a
      brief into a single-page layout with header/body/
      callout/image-placeholder sections. Bridge:
      `ai_brief_to_one_pager(brief_text, page_size)`.
- [x] **Task 11: AI "harmonize this palette".**
      `crates/kcreate_ai/src/palette_harmonize.rs`
      (complementary / triadic / analogous / split-comp /
      tetradic harmony rules in HSL). Bridge:
      `ai_harmonize_palette(brand_kit_id, harmony_type)`.
- [x] **Task 12: AI "suggest complementary type pairing".**
      `crates/kcreate_ai/src/type_pairing.rs` LLM-suggests
      body fonts filtered against `fontdb`. Bridge:
      `ai_suggest_type_pairing(heading_font_name)`.

### Block C — Export Center AI Features & Live Preview (Tasks 13–18)
- [x] **Task 13: AI "Optimize this SVG".**
      `crates/kcreate_export/src/svg_optimize.rs`: element-aware
      optimiser (empty-group removal, redundant-transform
      collapse, path-data shortening, default-attribute drop,
      `<defs>` inlining) with `protected_regions` /
      `with_unprotected` so it cannot mangle `<text>`,
      `<style>`, or CDATA bodies. Bridge:
      `export_optimize_svg(svg_string)` returns optimised
      bytes + reduction stats.
- [x] **Task 14: AI "compress raster without visible loss".**
      `crates/kcreate_export/src/smart_compress.rs` binary-
      searches JPEG/WebP quality against an SSIM target
      (default 0.98) computed on 8×8 blocks, row-parallel via
      rayon. Bridge: `export_smart_compress(node_id, format,
      target_ssim)`.
- [x] **Task 15: Live export preview.**
      `apps/desktop/renderer/src/components/ExportPreviewPanel.tsx`
      renders raster / SVG / PDF previews via
      `export_preview(request_json)` (capped at 1024 px
      longest side, 300 ms debounce).
- [x] **Task 16: Floating contextual toolbar.**
      `apps/desktop/renderer/src/components/FloatingToolbar.tsx`
      follows the active selection, shows context-appropriate
      actions for vector / text / raster / group nodes,
      respects the user-preference toggle.
- [x] **Task 17: AI/Illustrator SVG subset import.**
      `crates/kcreate_export/src/ai_import.rs` parses `.ai`
      (PDF-wrapped) files, extracts embedded SVG payloads
      when present, falls back to `pdf_import` otherwise.
      Bridge: `import_ai(path)`.
- [x] **Task 18: Export & import tests.** Coverage in
      `crates/kcreate_tests/tests/export_ai.rs` (24 tests:
      SVG redundant-group removal, transform collapse, size
      reduction, SSIM correctness, smart-compress
      convergence, preview byte validity, PDF-with-SVG
      extraction, fallback to PDF import).

### Block D — Brand Hub AI + Plugin Marketplace (Tasks 19–24)
- [x] **Task 19: AI "extend brand to brochure template".**
      `crates/kcreate_ai/src/brand_template.rs` generates a
      multi-page brochure with cover / content / back page
      structures driven by brand-kit tokens. Bridge:
      `ai_brand_to_brochure(brand_kit_id, num_pages)`.
- [x] **Task 20: Plugin marketplace foundation.**
      `crates/kcreate_plugin/src/marketplace.rs` mirrors
      `TemplateMarketplace`: scans `~/.kcreate/plugins/`,
      surfaces `PluginListing`, install-from-local + remove
      flows with Ed25519 signature verification. Bridge:
      `plugin_marketplace_list`,
      `plugin_marketplace_install_local(path)`,
      `plugin_marketplace_remove(id)`. UI: "Marketplace" tab
      in `PluginManager.tsx`.
- [x] **Task 21: Multi-page PDF export improvements.**
      `crates/kcreate_export/src/pdf_multi.rs` adds TOC
      generation from heading text nodes, PDF outline
      bookmarks, page-numbering tokens per page, hyperlink
      annotations from `Navigate` interactions, per-page
      glyph-subset embedded fonts. Bridge:
      `export_pdf_multi(options_json)`.
- [x] **Task 22: Batch export progress UI.**
      `apps/desktop/renderer/src/components/BatchExportProgress.tsx`
      provides real-time per-asset progress, ETA, cancel,
      retry-individual-asset, and summary panel on
      completion (uses existing `export_batch_start /
      _status / _cancel` bridge endpoints).
- [x] **Task 23: Workspace preferences panel.**
      `apps/desktop/renderer/src/components/PreferencesPanel.tsx`
      surfaces General / Canvas / AI / Performance /
      Shortcuts / Privacy sections, persisted via
      `preferences_load()` and `preferences_save(json)` to
      `~/.kcreate/preferences.json`.
- [x] **Task 24: Block D tests.** Coverage in
      `crates/kcreate_tests/tests/brand_plugin_ai.rs` (10
      tests: brochure page count + token application,
      marketplace scan + install + remove, PDF outline tree
      + TOC, preferences round-trip).

### Block E — Performance Validation & Hardening (Tasks 25–28)
- [x] **Task 25: Acceptance-criteria perf benchmark suite.**
      `crates/kcreate_tests/tests/acceptance_criteria.rs`
      measures cold-start, 50 MB project open, 1000-node
      pan/zoom frame time, 64 MP raster open, 50-asset
      batch export. Regression tests with 2× target
      margins. Bridge: `runtime_benchmark_cold_start_ms()`.
- [x] **Task 26: Render pipeline optimisation — incremental
      scene diff.** `crates/kcreate_bridge/src/scene_sync.rs`
      tracks per-node `scene_version` + `DirtySet<Uuid>`,
      reuses cached display-list entries for unchanged
      nodes. Tests in
      `crates/kcreate_tests/tests/incremental_sync.rs`
      verify partial updates produce identical output to
      full rebuilds.
- [x] **Task 27: Undo/redo memory optimisation.**
      `crates/kcreate_core/src/operation_compress.rs`
      implements deterministic JSON delta compression
      (`compute_diff`, `apply_diff`, `compress_operation`,
      `expand_operation`) plus BLAKE3-addressed blob
      reference swapping (`replace_blobs_with_refs`,
      `materialize_blob_refs`) for large inline base64
      payloads. Configurable via
      `RuntimeConfig::{compress_undo_log,
      undo_blob_threshold_bytes}`. Storage integration in
      `crates/kcreate_storage/src/project_io.rs` encodes the
      `__kcreateCompressedOpV1` sentinel on save and
      auto-detects on load (legacy rows pass through
      verbatim). 14 unit tests + 2 storage round-trip tests.
- [x] **Task 28: Startup time optimisation.**
      `tile_cache_lock()` is lazy-allocated and emits
      `bridge.tile_cache.subsystem_ready` on first touch
      (`TILE_CACHE_READY_MARKED` latch keeps it
      idempotent). `memory_watchdog_start` emits
      `bridge.memory_watchdog.subsystem_ready` only when
      explicitly armed. `llm_start` calls
      `mark_llm_sidecar_ready` on first successful sidecar
      spawn. Cold startup now contains zero
      `bridge.*.subsystem_ready` marks until something
      actually touches each subsystem. 4 perf-module tests
      assert the lazy contract.

### Block F — Documentation & Polish (Tasks 29–30)
- [x] **Task 29: PROGRESS.md + PHASES.md updated.** Phase 10
      section added with every task checkbox accounted for;
      Phase 8 / 9 headers reconfirmed at "Complete | 100%";
      changelog entry below.
- [x] **Task 30: README / ARCHITECTURE / AGENTS sync.**
      README Stack table + modules section updated with
      Phase 10 entries (NLM denoise, PatchMatch inpaint,
      SSIM smart compress, SVG optimiser, plugin
      marketplace, undo compression, lazy startup).
      ARCHITECTURE §17o (Phase 10) documents the Image
      Studio AI pipeline, Vector / Layout AI actions,
      Export AI (SVG optimise / smart compress / live
      preview), plugin marketplace, incremental scene sync,
      undo compression, and startup lazy-init. AGENTS
      "Where new code goes" table covers `denoise.rs`,
      `inpaint.rs`, `auto_color.rs`, `stroke_match.rs`,
      `glyph_extract.rs`, `reformat.rs`, `one_pager.rs`,
      `palette_harmonize.rs`, `type_pairing.rs`,
      `brand_template.rs`, `svg_optimize.rs`,
      `smart_compress.rs`, `ai_import.rs`, `pdf_multi.rs`,
      `phase10.rs`, `operation_compress.rs`,
      `FloatingToolbar.tsx`, `ExportPreviewPanel.tsx`,
      `BatchExportProgress.tsx`, `PreferencesPanel.tsx`,
      `MagicWandTool.tsx`.

### Phase 10 — Bridge & wire-format lockstep
- [x] **`crates/kcreate_bridge/src/phase10.rs`** owns every
      Phase 10 bridge entry point (`ai_denoise`,
      `ai_inpaint`, `ai_auto_color`, `ai_segment_at_point`,
      `ai_smart_select_at_point`, `ai_match_stroke`,
      `ai_extract_glyph`, `ai_reformat_to_deck`,
      `ai_brief_to_one_pager`, `ai_harmonize_palette`,
      `ai_suggest_type_pairing`, `ai_brand_to_brochure`,
      `export_optimize_svg`, `export_smart_compress`,
      `export_preview`, `export_pdf_multi`, `import_ai`,
      `plugin_marketplace_list`,
      `plugin_marketplace_install_local`,
      `plugin_marketplace_remove`, `preferences_load`,
      `preferences_save`,
      `runtime_benchmark_cold_start_ms`).
- [x] **`crates/kcreate_bridge/src/lib.rs`** exposes the new
      N-API entry points; `apps/desktop/shared/scene.ts`
      mirrors every new type and request/response shape;
      `apps/desktop/preload/src/preload.ts` and
      `apps/desktop/main/src/{bridge,main}.ts` wire the IPC
      handlers + Bridge interface in lockstep with the
      Rust surface.

## Phase 11 — Render Performance, Async Bridge, Prototype Animation, Concurrency & Security Hardening | Complete | 100%

### Block A — Render Pipeline: Incremental Scene Sync + Content-Addressed Images (Tasks 1–6)
- [x] **Task 1: Dirty-node tracking in `DocumentGraph`.**
      Added `dirty: HashSet<Uuid>` and `structure_dirty: bool`
      to `crates/kcreate_core/src/document.rs`. Every mutation
      method (`insert_node`, `remove_node`, `get_node_mut`,
      `reparent_node`, `reorder_children`, `swap_node`,
      `apply_lww`) marks the affected node(s) dirty and sets
      `structure_dirty` when the tree shape changes.
      `drain_dirty()` returns and clears the dirty set;
      `mark_dirty(id)` allows explicit external marking.
      Coverage in `crates/kcreate_tests/tests/dirty_tracking.rs`.
- [x] **Task 2: Incremental scene sync in `SceneSync`.**
      `crates/kcreate_bridge/src/scene_sync.rs` now caches the
      last-emitted `Vec<Object>` per document node id and a
      `cached_scene_version: u64` counter. The hot path drains
      the dirty set, re-runs `visit()` only for dirty nodes,
      and concatenates cached entries in z-order. Full rebuild
      remains the fallback on `structure_dirty` or
      `SceneSync::clear()`. Equivalence verified in
      `crates/kcreate_tests/tests/incremental_sync.rs`.
- [x] **Task 3: Content-addressed image fingerprinting.**
      Renderer `ObjectKind::Image` gained an
      `content_hash: Option<u64>` field
      (`crates/kcreate_renderer/src/scene.rs`). `SceneSync`
      reuses the BLAKE3 hash from `RasterImageMeta.hash`
      instead of re-hashing pixels.
      `crates/kcreate_renderer/src/pipeline.rs::hash_object`
      hashes the 8-byte digest for the cache-hit path, with
      the chunked-pixel path retained as a fallback when no
      digest is available.
- [x] **Task 4: Spatial indexing for document-level hit testing.**
      Added `spatial_index: Option<RTree<SpatialEntry>>` to
      `DocumentGraph`. `SpatialEntry` implements
      `rstar::RTreeObject` over `[f64; 4]` bounds; the index
      is rebuilt lazily after `structure_dirty`. `query_point`
      returns nodes whose bounds contain the point in
      topmost-first z-order. Wired into
      `crates/kcreate_bridge/src/hit_test.rs`.
- [x] **Task 5: Display list batching + GPU instancing prep.**
      `crates/kcreate_renderer/src/pipeline.rs` groups
      consecutive `FillRect` / `StrokeRect` commands sharing
      a `Style` into the new
      `DisplayCommand::BatchedRects { rects, style }` variant.
      CPU backend iterates batched rects; the GPU backend gets
      the same shape so a future instanced-draw upgrade is a
      drop-in. Visual equivalence verified by golden tests.
- [x] **Task 6: Incremental sync + fingerprint tests.**
      `crates/kcreate_tests/tests/render_pipeline_perf.rs`
      covers the 5000-node single-edit speedup ratio,
      content-addressed cache-hit fingerprint (no pixel hash
      walk), spatial-index scaling (1k vs 5k nodes), and
      batched display-list reduction on a 20-artboard scene.

### Block B — Async Bridge + GPU Compute Filters (Tasks 7–12)
- [x] **Task 7: Async N-API wrapper for raster operations.**
      `raster_apply_blur`, `_sharpen`, `_levels`, `_curves`,
      `_hsl`, `_color_balance`, `_perspective`,
      `_apply_filter_masked`, and `raster_crop` are now
      `AsyncTask` entry points in
      `crates/kcreate_bridge/src/lib.rs`, following the
      Phase 4 `VisionDescribeImageTask` pattern. The filter
      step itself runs on libuv's threadpool; resolve is on
      the main thread. `apps/desktop/shared/scene.ts` types
      updated to `Promise<void>`.
- [x] **Task 8: Async N-API for export operations.**
      `export_png`, `export_pdf`, `export_svg_async`, and
      `project_save` are now `AsyncTask`s. The save task
      snapshots the document inside the write guard before
      releasing the lock, so concurrent edits during a long
      save can't corrupt the on-disk file.
- [x] **Task 9: GPU compute shader for Gaussian blur.**
      `crates/kcreate_renderer/src/compute/gaussian_blur.wgsl`
      implements a two-pass separable Gaussian (horizontal
      then vertical) with workgroup-per-row / per-column.
      `compute/mod.rs::GpuComputeContext` shares the
      `wgpu::Device`/`Queue` with the existing `GpuBackend`.
      `crates/kcreate_raster/src/filters.rs` exposes
      `gaussian_blur_gpu`; `crates/kcreate_bridge/src/gpu_compute.rs`
      threads the GPU handle through the filter call sites and
      falls back to CPU when no adapter is available.
- [x] **Task 10: GPU compute shader for levels/curves.**
      `compute/levels_curves.wgsl` reads a 256-entry LUT from
      a storage buffer and applies it per pixel. Wired into
      `filters.rs` as `levels_gpu` and `curves_gpu`. Identity
      LUTs verified to be no-ops to within ±1 per channel.
- [x] **Task 11: GPU compute shader for unsharp mask.**
      `compute/unsharp_mask.wgsl` consumes the original +
      Gaussian-blurred textures from Task 9 and emits
      `original + amount × (original − blurred)`. Pixel
      parity with the CPU path verified.
- [x] **Task 12: GPU compute filter integration tests.**
      `crates/kcreate_tests/tests/gpu_compute.rs` skips when
      `wgpu::Instance::request_adapter` returns `None`;
      otherwise verifies CPU↔GPU parity on Gaussian blur,
      levels, curves, and unsharp mask, plus a 4096×4096 GPU
      blur within a 500 ms ceiling.

### Block C — Prototype Animation + Auto-Layout in Components (Tasks 13–18)
- [x] **Task 13: Prototype transitions — dissolve, slide, push.**
      `crates/kcreate_core/src/node.rs` extends
      `InteractionAction` with a `Transition` value
      (`AnimationType`, `duration_ms`, `EasingCurve`,
      optional `SlideDirection`). `Transition::default()`
      is `Instant + 300 ms + EaseInOut` so legacy
      interactions deserialize unchanged. Bridge
      `interaction_add` accepts transition JSON.
      `crates/kcreate_tests/tests/prototype_advanced.rs`
      round-trips every variant.
- [x] **Task 14: PrototypePlayer animation engine.**
      `apps/desktop/renderer/src/lib/EasingEngine.ts`
      provides `linear`, `easeIn`, `easeOut`, `easeInOut`,
      `cubicBezier(t, x1, y1, x2, y2)`, and a damped
      harmonic-oscillator `spring(t, stiffness, damping)`.
      `PrototypePlayer.tsx` captures an outgoing frame via
      `window.kcreate.renderer.acquireFrame()`, layers the
      outgoing + incoming artboard, and drives opacity /
      transform via `requestAnimationFrame`. Animation
      layers are torn down on completion.
      `InteractionPanel.tsx` exposes the full transition
      config.
- [x] **Task 15: Hover / press / MouseEnter / MouseLeave /
      AfterDelay triggers.** `InteractionTrigger` gained
      `MouseEnter`, `MouseLeave`, and
      `AfterDelay { ms }`. `PrototypePlayer.tsx` wires
      enter/leave on the hotspot overlay, a press visual
      state (scale 0.97, opacity 0.8) on mousedown, and
      starts/clears the AfterDelay timer on artboard
      navigation. Splash → home transitions now work without
      a click.
- [x] **Task 16: Auto-layout propagation through component
      instances.** `crates/kcreate_bridge/src/document.rs::document_update_node`
      detects bounds changes on `ComponentLayer` nodes with
      `component_instance` metadata and re-runs
      `layout_recompute` on the instance — recursing into
      nested instances with a depth-limit of 16 so circular
      references can't loop. `crates/kcreate_layout`
      gained `layout_flex_with_overrides` /
      `layout_grid_with_overrides` so override sizes from
      `instance.overrides` win over intrinsic sizes during
      the solve. Coverage in
      `crates/kcreate_tests/tests/component_autolayout.rs`.
- [x] **Task 17: SwitchVariant action ("Smart Animate").**
      `InteractionAction::SwitchVariant { variant_id,
      transition }` added. `PrototypePlayer.tsx` matches
      layers by name between the current and target variant,
      interpolates `bounds`, `opacity`, fill colour (in HSL
      space), and corner radius across the transition
      duration, fades in layers that exist only in the
      target, and fades out layers that exist only in the
      source. `component_switch_variant` returns the
      before/after states so the renderer can compute the
      interpolation without re-fetching the tree.
- [x] **Task 18: Prototype + component usability tests.**
      `crates/kcreate_tests/tests/prototype_advanced.rs`
      and `component_autolayout.rs` cover transition serde
      round-trip, AfterDelay 0 ms + 5000 ms, SwitchVariant
      matching, flex/grid instance resize, nested instance
      reflow, and override-size respect.
      `EasingEngine.test.ts` covers linear identity,
      easeInOut symmetry, spring convergence.

### Block D — Workspace Concurrency + Undo Optimization (Tasks 19–24)
- [x] **Task 19: RwLock for workspace reads.** Replaced
      `Mutex<Option<Workspace>>` with
      `RwLock<Option<Workspace>>` in
      `crates/kcreate_bridge/src/document.rs`. Every
      call-site audited: read-only entry points
      (`document_get_tree`, `document_status`,
      `document_get_selection`, `export_svg`,
      `export_preset_list`, etc.) use `read()`; mutating
      entry points use `write()`. `sync_scene_locked` was
      refactored to take `&mut Workspace` rather than a
      `MutexGuard` so it composes cleanly inside a write
      guard.
- [x] **Task 20: Delta-compressed operations.**
      `crates/kcreate_core/src/operation.rs` (and Phase 10's
      `operation_compress.rs`) stores `OperationDelta`
      values internally —
      `{ added_keys, removed_keys, changed_keys }` —
      decompressing only at the API boundary. Raster
      operations carrying blob hashes shrink to a single
      changed key; property edits typically encode in 1–3.
      `crates/kcreate_storage/src/schema.rs` writes the
      compressed form and auto-upgrades legacy rows on load.
- [x] **Task 21: Per-node version tracking for MVCC reads.**
      `Node::touch()` increments `version: u64` on every
      mutation; `DocumentGraph` maintains a
      `document_version: AtomicU64` counter the bridge
      exports via the lock-free `document_version()` N-API
      entry point. The renderer polls this at 60 fps and
      skips `refreshTree` round-trips when it hasn't moved.
- [x] **Task 22: Node count scaling target 5k → 10k.**
      `crates/kcreate_tests/tests/scale_validation.rs`
      builds a 10 000-node artboard and asserts:
      sync-after-single-edit < 5 ms, hit-test at a random
      point < 1 ms, `document_get_tree` serialization
      < 50 ms, full scene fingerprint < 10 ms (with
      content-addressed images), undo / redo < 5 ms with
      compressed operations. Acceptance bumped to "5k Tier 1,
      10k Tier 2+" in PROPOSAL §20.
- [x] **Task 23: Lazy subsystem initialization.** Verified
      every Phase 8/9/10 subsystem stays deferred behind
      `OnceCell` / first-use guards: tile cache, LLM
      sidecar, memory watchdog, audit DB, collab transport,
      `fontdb` discovery. `fontdb` now runs on a background
      thread; the text engine returns a "fonts loading"
      placeholder until the scan completes. Startup
      timeline marks confirm
      `bridge.first_call → project_create.start < 200 ms`.
- [x] **Task 24: Concurrency + undo tests.**
      `crates/kcreate_tests/tests/concurrency.rs`: 10
      reader threads × 1 writer × 1000 iterations of the
      RwLock under stress, delta compress/expand round-trip
      on 1000 random operations, MVCC version monotonicity
      across undo/redo, font lazy-init non-blocking, no
      eager subsystem init on bridge load.

### Block E — Security Hardening (Tasks 25–28)
- [x] **Task 25: Authenticated LLM sidecar with per-session
      bearer token.** `crates/kcreate_ai/src/llm_sidecar.rs`
      generates a fresh 32-byte token via `getrandom`,
      passes it to `llama-server --api-key`, and stores it
      on `SidecarConfig`. `crates/kcreate_ai/src/llm_chat.rs`
      attaches `Authorization: Bearer <token>` to every
      loopback request. The token never leaves the bridge
      address space — it is not forwarded across N-API
      (see `crates/kcreate_bridge/src/llm.rs::SidecarStatus`).
- [x] **Task 26: TOCTOU port allocation fix.** Spawning a
      sidecar binds the loopback listener, hands the bound
      port to `llama-server`, then performs a post-spawn
      verification handshake: `GET /v1/models` with the
      session bearer token. If the server responds with a
      mismatched / absent token, the sidecar is killed and
      retried on a freshly-bound port. Verification covered
      in `crates/kcreate_tests/tests/llm_sidecar_auth.rs`.
- [x] **Task 27: Encrypt ACL alongside project.**
      `crates/kcreate_collab/src/acl.rs` ships
      `encrypt_acl_bytes` / `decrypt_acl_bytes` /
      `looks_like_encrypted_acl` plus the
      `KCAClv1\0` magic + 12-byte nonce +
      ChaCha20-Poly1305 wire format. Nonces are sampled
      directly from `getrandom` to match the OS-CSPRNG
      contract documented in `clipboard.rs`.
      `crates/kcreate_bridge/src/collab.rs::load_project_acl`
      prefers `acl.json.enc`, auto-migrates plaintext ACLs
      on encrypted projects, and `save_project_acl` cleans
      up the stale opposite-format file on every write.
      13 ACL tests (6 new + 7 pre-existing) green.
- [x] **Task 28: Certificate pinning for KChat backend.**
      `crates/kcreate_kchat_client/src/pinning.rs` builds a
      custom `rustls::ClientConfig` that chains the
      Mozilla-root `WebPkiServerVerifier` with a leaf-cert
      SHA-256 fingerprint check (constant-time compare).
      `RestClientConfig::pinned_certificate_sha256` is
      hex-parsed eagerly at construction time; pin
      mismatches surface as the typed
      `ClientError::CertificatePinMismatch` (mapped from
      reqwest's error chain via the
      `KCREATE_PIN_MISMATCH:` marker) so the renderer can
      show "possible MITM — contact your KChat
      administrator". Coverage in
      `crates/kcreate_kchat_client/src/pinning.rs` unit
      tests.

### Block F — Documentation & Acceptance Criteria (Tasks 29–30)
- [x] **Task 29: PROGRESS / PHASES / PROPOSAL updated.**
      Phase 11 section added with every task checkbox.
      Acceptance criteria in PROPOSAL §20 bumped:
      pan/zoom 5000-node Tier 1 / 10 000-node Tier 2+,
      64MP Gaussian blur < 500 ms on Tier 2+, prototype
      transition at 60 fps on Tier 1+.
- [x] **Task 30: README / ARCHITECTURE / AGENTS sync.**
      README Stack table + module list updated.
      ARCHITECTURE §17p (Phase 11) documents incremental
      scene sync, content-addressed image fingerprinting,
      spatial indexing, GPU compute filters, async N-API
      surface, RwLock workspace, delta-compressed undo log,
      prototype transitions / Smart Animate, auto-layout
      propagation, LLM sidecar auth + TOCTOU fix, ACL
      encryption, and KChat certificate pinning. AGENTS
      "Where new code goes" gains `compute/mod.rs`,
      `compute/*.wgsl`, `EasingEngine.ts`,
      `dirty_tracking.rs`, `phase11.rs`, and the kchat
      `pinning.rs` module.

### Phase 11 — Bridge & wire-format lockstep
- [x] **`crates/kcreate_bridge/src/phase11.rs`** owns the
      new Phase 11 bridge entry points (raster + export
      async tasks, `document_version`, prototype transition
      JSON, layout-with-overrides recompute, GPU compute
      filter dispatch, encrypted-ACL load/save helpers).
- [x] **`crates/kcreate_bridge/src/lib.rs`** exposes the
      new N-API surface; `apps/desktop/shared/scene.ts`
      mirrors every new type and request/response shape;
      `apps/desktop/preload/src/preload.ts` and
      `apps/desktop/main/src/{bridge,main}.ts` wire the IPC
      handlers + Bridge interface in lockstep with the Rust
      surface.

## Phase 12 — Python Elimination, Native AI Stack | Complete | 100%

### Block A — Remove MLX sidecar, consolidate on llama-server (Tasks 1–6)
- [x] **Task 1: Ternary-Bonsai GGUF packs added.**
      `crates/kcreate_ai/src/model_registry.rs::static_packs`
      now ships `llm_bonsai_1_7b` (463 MB Q2_K),
      `llm_bonsai_4b` (1.07 GB Q2_K), `llm_bonsai_8b`
      (2.18 GB Q2_K) — sourced from
      `huggingface.co/prism-ml/Ternary-Bonsai-{1.7B,4B,8B}-gguf`
      with verified filenames + byte counts. Each pack
      declares `capabilities: ["design_suggestions",
      "layer_naming"]`, `category: DesignPro`, `kind:
      Sidecar`.
- [x] **Task 2: Tier-aware `recommended_llm_pack`.** Returns
      `llm_bonsai_1_7b` on Tier 0 (≤4 GB RAM),
      `llm_bonsai_4b` on Tier 1 (8 GB), and
      `llm_bonsai_8b` on Tier 2+ (16 GB+). The `_platform`
      parameter is retained for API compatibility but no
      longer branches — every supported platform runs the
      same GGUF on llama-server. `llm_sidecar_3b` (Llama
      3.2 3B) stays as an alternative manual choice.
- [x] **Task 3: MLX packs removed from the registry.**
      `vision_smolvlm_256m_mlx`, `vision_qwen25vl_7b_mlx`,
      and `image_gen_flux_klein_mlx` are gone.
      `gguf_fallback_for_mlx_pack` is gone (no callers).
      `recommended_vision_pack` /
      `recommended_generation_pack` no longer branch on
      `is_apple_silicon`. `mmproj_for` retained its
      Qwen-VL / SmolVLM entries (those are GGUF projectors,
      not MLX-specific).
- [x] **Task 4: `mlx_sidecar` module deleted.**
      `crates/kcreate_ai/src/mlx_sidecar.rs` removed;
      `lib.rs` no longer declares `pub mod mlx_sidecar;`.
      Every reference cleaned out of `vision_chat.rs`,
      `sidecar_dispatcher.rs`, `phase4.rs`, and
      `vision_sidecar.rs` integration tests.
- [x] **Task 5: `SidecarDispatcher` collapsed to a single
      runtime.** `SidecarHandle::Mlx`, `SidecarRuntime::MlxLm`,
      `DispatchReason::MlxNative`, and
      `DispatchReason::MlxUnavailableFallback` are gone.
      `plan_dispatch` no longer takes an `mlx_available`
      argument and always returns
      `SidecarRuntime::LlamaServer`. Module docs updated to
      explain the forward-compat reason for keeping the
      `SidecarRuntime` enum (one-variant today, future
      Rust-native engine slot).
- [x] **Task 6: Block A tests.**
      `crates/kcreate_tests/tests/model_registry.rs`
      verifies the three Bonsai packs are present,
      `recommended_llm_pack` is tier-correct, no `_mlx`
      pack IDs survive, and the dispatcher exposes only
      `LlamaServer` as a reason.

### Block B — Replace Python diffusion sidecar with sd.cpp (Tasks 7–12)
- [x] **Task 7: `DiffusionSidecar` wired to `sd-server`.**
      `crates/kcreate_ai/src/diffusion_sidecar.rs` spawns
      the [stable-diffusion.cpp][sd-cpp] `sd-server` binary
      with `--listen-ip 127.0.0.1 --listen-port <port>
      --diffusion-model <path>` and the operator-supplied
      `KCREATE_SD_SERVER_EXTRA_ARGS` (FLUX text-encoder /
      VAE component flags). Lifecycle mirrors `LlmSidecar`:
      `Stopped → Starting → Ready → Stopped` driven by a
      background `health_worker` thread polling
      `/sdcpp/v1/capabilities`. Loopback-only bind enforced
      via socket port allocation.
- [x] **Task 8: `image_gen.rs` is now a thin HTTP client.**
      `generate_image` POSTs to `/sdapi/v1/txt2img` (A1111
      shape: `{prompt, width, height, steps, seed}`),
      reads `images[0]` as base64 PNG, strips an optional
      `data:image/png;base64,` prefix, and hands the bytes
      to `decode_png_payload`. `ImageGenSidecar` /
      `ImageGenConfig` removed — their state lives in
      `DiffusionSidecar` now.
- [x] **Task 9: Readiness probing.** `is_ready()` and the
      health worker poll `/sdcpp/v1/capabilities` (HTTP 200
      JSON) through `ureq` when the `llm_sidecar` feature
      is on, with a raw TCP-connect fallback otherwise.
- [x] **Task 10: `tools/kcreate_diffusion/` deleted.** The
      FastAPI + diffusers Python sidecar (`server.py`,
      `requirements.txt`, `__init__.py`, `README.md`) is
      gone. The `tools/` directory was removed entirely
      since the diffusion sidecar was its sole content.
- [x] **Task 11: Image-gen model packs.** `image_gen_flux_klein_4b`
      (FLUX.2-Klein-4B GGUF) stays as the sole image-gen
      pack. Bonsai Image Ternary variants from prism-ml are
      packaged as gemlite-2bit / MLX-2bit and use a custom
      tensor layout that sd.cpp's FLUX loader does not
      accept — verified by checking the model card and the
      sd.cpp loader source. Tier gating
      (`image_generation_allowed`) unchanged at Tier 2+.
- [x] **Task 12: Block B tests.**
      `crates/kcreate_ai/src/diffusion_sidecar.rs` ships
      six unit tests (`build_argv` shape, missing-binary
      error, port allocation, config validation, lifecycle
      idempotency, dual-mode readiness probe).
      `crates/kcreate_ai/src/image_gen.rs` ships five tests
      including a `tiny_http`-backed round-trip against an
      A1111-compatible mock that exercises the
      `data:image/png;base64,` prefix-stripping path.

### Block C — Bridge + UI cleanup (Tasks 13–18)
- [x] **Task 13: Bridge cleanup.**
      `crates/kcreate_bridge/src/phase4.rs` no longer
      imports `mlx_sidecar::*`, `probe_mlx_available`, or
      `ImageGenSidecar` / `ImageGenConfig`. `VisionHandle`
      collapsed from `Llama | Mlx` to `Llama` only.
      `vision_status` no longer reports the `"mlx_lm"`
      runtime. `spawn_vision` only handles the
      `LlamaServer` dispatch arm. `image_gen_start`
      constructs `DiffusionSidecarConfig` via the new
      `sd_server_binary()` (env-var override:
      `KCREATE_SD_SERVER_BINARY`, fallback: `sd-server` on
      PATH) and `parse_sd_server_extra_args()` (env var:
      `KCREATE_SD_SERVER_EXTRA_ARGS`, space-separated).
- [x] **Task 14: `apps/desktop/shared/scene.ts` updated.**
      `VisionStatus.runtime` now types as
      `"llama_server" | null` (was
      `"llama_server" | "mlx_lm" | null`).
      `VisionBridge` and `ImageGenBridge` doc comments
      updated to drop MLX / Python references.
- [x] **Task 15: Model Manager UI.**
      `apps/desktop/renderer/src/components/ModelManager.tsx::filterPacksForTier`
      dropped the `id.endsWith("_mlx") && !isAppleSilicon`
      branch — every pack is GGUF and platform-portable
      now. The platform-tier metadata column still shows
      Tier 0 / 1 / 2+ recommended packs (now Bonsai).
- [x] **Task 16: Image Gen Panel.**
      `apps/desktop/renderer/src/components/ImageGenPanel.tsx`
      already drove the sidecar through the same
      `kcreate.imageGen.{start,generate,status,stop}`
      surface — no panel changes needed because the wire
      format is unchanged. Only the implementing sidecar
      swapped underneath.
- [x] **Task 17: Preload + main IPC.** Bridge function
      signatures unchanged on the N-API surface
      (`imageGenStart` still takes `(packId,
      modelPathOpt)` and returns the port), so no IPC
      channel updates needed.
- [x] **Task 18: Block C tests.** `pnpm typecheck` clean —
      the `VisionStatus.runtime` collapse forced every
      consumer to drop unreachable `"mlx_lm"` branches at
      type-check time. `pnpm lint` clean.

### Block D — Carried-over performance + security work
- [x] **Verified shipped in PR #27 (Phase 11).**
      Content-addressed image fingerprinting, R-tree spatial
      index for hit testing, async N-API for raster + export
      ops, RwLock for workspace reads, LLM sidecar
      bearer-token auth with post-spawn TOCTOU verification
      — all present in HEAD; no gaps to fill in Phase 12.

### Block E — Carried-over usability work
- [x] **Verified shipped in PR #27 (Phase 11).**
      Prototype transition data model (`Transition`,
      `TransitionKind`, `EasingCurve`), prototype player
      animations (dissolve / slide / push), component
      auto-layout propagation through instances with
      override preservation — all present in HEAD.

### Block F — Documentation
- [x] **Task 29: PROGRESS.md + PHASES.md.** This section.
      PHASES.md gained the Phase 12 entry. Changelog
      entry added below.
- [x] **Task 30: README + ARCHITECTURE + AGENTS sync.**
      ARCHITECTURE.md §17q now documents Phase 12
      (Python-elimination), the high-level diagram swaps
      the `kcreate_diffusion Python sidecar` node for
      `sd.cpp (C++)`, §16k (MLX sidecar) marked removed,
      §16j updated to describe sd.cpp. README dropped the
      MLX / Python prerequisites, added the sd.cpp row,
      and listed the Bonsai packs. AGENTS.md file table
      now lists `diffusion_sidecar.rs` and drops
      `mlx_sidecar.rs` + `tools/kcreate_diffusion/`.
      CONTRIBUTING.md replaced the
      `pip install -r tools/kcreate_diffusion/requirements.txt`
      block with a description of the two C++ sidecars and
      the `KCREATE_SD_SERVER_BINARY` /
      `KCREATE_SD_SERVER_EXTRA_ARGS` env vars.

[sd-cpp]: https://github.com/leejet/stable-diffusion.cpp

## Changelog

- **2026-05-30** — Phase 12: eliminated every Python
  dependency from the AI stack. Text LLM and vision (VLM)
  now run exclusively on `llama-server` with the new
  tier-aware Ternary-Bonsai GGUF packs (1.7B / 4B / 8B
  selected by `recommended_llm_pack`); image generation
  runs on `sd-server` from
  [stable-diffusion.cpp](https://github.com/leejet/stable-diffusion.cpp)
  via a new `DiffusionSidecar` that exposes the same
  `Stopped → Starting → Ready → Stopped` lifecycle as
  `LlmSidecar` and probes
  `/sdcpp/v1/capabilities` for readiness.
  `crates/kcreate_ai/src/mlx_sidecar.rs`, every `_mlx`
  pack ID, `SidecarRuntime::MlxLm`,
  `DispatchReason::MlxNative` /
  `MlxUnavailableFallback`, `gguf_fallback_for_mlx_pack`,
  `VisionHandle::Mlx`, and `tools/kcreate_diffusion/`
  (the FastAPI + diffusers Python sidecar) are gone.
  `crates/kcreate_ai/src/image_gen.rs` is now a thin
  HTTP client that POSTs A1111-compatible
  `/sdapi/v1/txt2img` requests and decodes the base64
  PNG response. Operator overrides:
  `KCREATE_SD_SERVER_BINARY` (absolute path) and
  `KCREATE_SD_SERVER_EXTRA_ARGS` (space-separated flags).
  Docs synced across PROGRESS / PHASES / ARCHITECTURE
  §17q / README / CONTRIBUTING / AGENTS.
- **2026-05-30 (PR #27)** — Phase 11: incremental scene
  sync (`DocumentGraph::drain_dirty` + cached per-node
  object lists + `structure_dirty` full-rebuild fallback),
  content-addressed image fingerprinting (BLAKE3 digest on
  `ObjectKind::Image`, 8 bytes hashed per frame instead of
  walking a 48 MB pixel buffer), R-tree spatial index for
  document-level hit testing, batched `FillRect` /
  `StrokeRect` display-list commands, async N-API for
  raster ops + export + `project_save` (libuv-threadpool
  AsyncTasks, lock-snapshot before save), GPU compute
  filters (Gaussian blur / levels / curves / unsharp mask
  in `crates/kcreate_renderer/src/compute/*.wgsl`),
  prototype transitions (`AnimationType`, `EasingCurve`
  incl. cubic-bezier + spring, `SlideDirection`),
  PrototypePlayer animation engine (`EasingEngine.ts` +
  layered outgoing/incoming artboard frames),
  hover/press/MouseEnter/MouseLeave/AfterDelay triggers,
  auto-layout propagation through component instances
  (recursion-bounded `layout_recompute` + per-child
  override sizes), `SwitchVariant` Smart-Animate
  interpolation (bounds / opacity / HSL fill / corner
  radius matched by layer name), RwLock workspace with
  audited read/write call sites, delta-compressed undo log,
  per-node `version` + `document_version` AtomicU64 for
  lock-free poll-skipping, lazy subsystem init audit +
  background `fontdb` scan, 10 000-node scale validation,
  per-session LLM sidecar bearer token + TOCTOU
  post-spawn verification handshake, ChaCha20-Poly1305
  ACL encryption with auto-migration of plaintext on
  encrypted projects, KChat REST cert pinning
  (`PinnedCertVerifier` over the Mozilla `webpki-roots`
  trust store, constant-time leaf-fingerprint compare).
  Wired through `crates/kcreate_bridge/src/phase11.rs` +
  mirrored in `apps/desktop/shared/scene.ts` and the
  preload / main IPC layer. New integration tests in
  `crates/kcreate_tests/tests/`
  (`dirty_tracking.rs`, `incremental_sync.rs`,
  `render_pipeline_perf.rs`, `gpu_compute.rs`,
  `prototype_advanced.rs`, `component_autolayout.rs`,
  `concurrency.rs`, `scale_validation.rs`,
  `llm_sidecar_auth.rs`) plus
  `crates/kcreate_collab/src/acl.rs` and
  `crates/kcreate_kchat_client/src/pinning.rs` unit tests.
  `local_first.rs` sentinel stays green — `webpki-roots`
  is pulled in only by `kcreate_kchat_client`, which is
  out of the editing-path closure.

- **2026-05-29 (PR #26)** — Phase 10: Image Studio AI
  pipeline (NLM denoise, PatchMatch inpaint, auto-colour,
  SAM segmentation tool, magic wand), Vector / Layout AI
  (stroke match, glyph extract, deck reformat, one-pager,
  brand-to-brochure), palette harmonisation + type pairing,
  Export AI (element-aware SVG optimiser, SSIM smart
  compress, live export preview, multi-page PDF with
  outline/TOC/hyperlinks, AI/Illustrator import), plugin
  marketplace foundation (scan / install-from-local /
  remove with Ed25519 signature verification), preferences
  panel persisted to `~/.kcreate/preferences.json`,
  incremental scene sync via per-node version + dirty set,
  undo-log delta compression + BLAKE3 blob reference
  swapping, startup lazy-init for tile cache / LLM
  sidecar / memory watchdog with cold-start timeline
  marks. Every feature is wired through
  `crates/kcreate_bridge/src/phase10.rs` + mirrored in
  `apps/desktop/shared/scene.ts` and the preload / main
  IPC layer. 66 new integration tests in
  `crates/kcreate_tests/tests/`
  (`image_studio_ai.rs`, `vector_layout_ai.rs`,
  `export_ai.rs`, `brand_plugin_ai.rs`,
  `acceptance_criteria.rs`, `incremental_sync.rs`),
  plus 14 unit tests in `operation_compress.rs` and 2
  storage round-trip tests in `project_io.rs`, plus 4
  perf-module tests for the lazy subsystem-ready marks.
  `local_first.rs` sentinel stays green — none of the new
  dependencies pull networking into the editing-path
  closure.

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
