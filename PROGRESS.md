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

## Phase 6 — Production Polish | In progress | ~100%

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

## Changelog

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
