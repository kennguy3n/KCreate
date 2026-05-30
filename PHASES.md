# KCreate — Phase Index

This file is a top-level index of every shipped phase, what each
one delivered, and where to find the detailed task tracking. The
authoritative per-task status is `PROGRESS.md`; the changelog at
the bottom of that file carries the PR-by-PR diff.

## Phase 0 — Technical Spike | Complete | 100%

Foundations: workspace + CI + Electron shell, offscreen wgpu
renderer with CPU fallback, document graph + operation log +
project model, SQLite storage, vector engine, presenter
(triple-buffered readback), Phase 0 exit-criteria sentinel
(`crates/kcreate_tests/tests/phase0_exit.rs`).

## Phase 1 — MVP | Complete | 100%

Editor MVP: layer panel, tool palette, transforms, snap engine,
text + image layers, exporters (PNG / SVG / PDF / WebP / JPEG),
hit testing, multi-document workspaces. See PROGRESS.md §"Phase
1 — MVP" for the per-block breakdown.

## Phase 2 — Professional Workflows | Complete | 100%

Plugin sandbox (WASM, ed25519 manifest signing, deny-by-default
host ABI, JS panel runtime), AI safety + provenance, PDF preflight,
icon-pack generation, parallel batch with async cancel, MCP
loopback server with per-tool permissions, native canvas surface,
prototype interactions, screenshot-to-layout.

## Phase 3 — Advanced Suite | Complete | 100%

Multi-page documents + master pages + page navigator, prototype
player, accessibility panel, slice CRUD + parallel export,
`.kbrand` round-trip, multi-fill / multi-stroke stacks, dash +
round-corners path effects, linked text frames + image-text
wraps, operational CRDT on `Operation` (`kcreate_collab::crdt`),
Pantone spot-color catalogues + overprint preflight, ESRGAN /
SAM ONNX model packs, local template marketplace.

## Phase 4 — Vision & Generation AI | Complete | 100%

VLM sidecars (SmolVLM2, Qwen 2.5-VL, MLX variants), alt-text +
layout-suggest + design-critique + brand-extract + design-tokens
generation, FLUX image generation sidecar, layout-suggest /
brand-extract UI panels, KChat trusted-issuer allowlist, OCR
text-region → text-layer flow.

## Phase 5 — Image Studio + Vector Studio + Layout Studio + Brand Hub | Complete | 100%

Levels / Curves / Gaussian / Box blur / Unsharp mask filter
stack, healing brush, snap engine + smart-guides overlay,
simplify / smooth / offset path operations, variable stroke
profile, slice export, `.kbrand` round-trip, spot colours.

## Phase 6 — Production Polish | Complete | 100%

`kcreate_audit` SQLite audit DB (separate from project DB),
undo/redo grouping + drag coalescing + `ApplyPatchSnapshot` atomic
rollback (lockstep-tested invariant), lazy thumbnail pipeline
(content-addressed cache + coalesced pre-warm), Figma + Sketch
JSON importers, JSON-backed keyboard shortcut registry, CSS-
variable-driven dark mode (no JS palette duplicate), OS file-
manager drag-drop + cross-artboard clipboard paste, layer-panel
search + colour tagging, E2E user-journey tests, acceptance-
criteria benchmark suite (`cold_start`, `raster_open_64mp`,
`viewport_pan`, `batch_50_assets`).

## Phase 7 — KChat backend Integration | Complete | 100%

First-party integration with the shared KChat / Mattermost
backend that `uneycom/uney-chat-desktop` also signs in to
(the **Option C** shape). New `kcreate_kchat_client` crate
speaks HTTPS REST (`reqwest` + `rustls`) to that backend and
bridges KChat communities / conversations / community-member
rosters into the existing collab gate. A thin `.kcz` companion
extension (`apps/kchat-extension/`) renders a sidebar inside
KChat Desktop and bridges deeplinks back to KCreate. 30 tasks
across 6 blocks:

- **Block A (Tasks 1–6):** REST client crate (`reqwest` +
  `rustls`) + DTOs + auth (token store + 401 refresh +
  429 retry) + attestation bridging + bridge surface +
  in-process `axum` fixture tests.
- **Block B (Tasks 7–12):** community-scoped session start +
  member roster sync + conversation document sharing + invite
  acceptance + role-based permissions
  (`CollabPermission::{Editor, Viewer}`).
- **Block C (Tasks 13–18):** real-time cursor + selection
  overlays, resume bundle for late joiners, conflict
  notification toast, collaborative undo awareness.
- **Block D (Tasks 19–24):** 60-minute QUIC cert rotation,
  audit trail extended with collab events, ACL persistence
  (`<project_dir>/acl.json`), per-peer rate limiting,
  ChaCha20-Poly1305 encrypted clipboard share over a
  BLAKE3-derived X25519 session key.
- **Block E (Tasks 25–28):** 50 ms / 200-op outbound batching,
  20 Hz presence throttling with 2 px delta floor + 2 s idle
  suppression, selective sync via `session_set_active_pages`,
  criterion performance benchmarks
  (`crates/kcreate_bridge/benches/collab_perf.rs`).
- **Block F (Tasks 29–30):** PROGRESS.md / README.md /
  ARCHITECTURE.md / AGENTS.md updates covering the Option C
  REST surface and the `.kcz` companion extension.

Local-first invariant preserved (`kcreate_kchat_client` and the
existing `kcreate_collab_transport` both stay out of the
editing-path closure walked by `crates/kcreate_tests/tests/local_first.rs`).
Feature flags: `kchat-backend` (production REST client),
`kchat-dev-issuer` (local-mint for tests), `collab` (LAN
transport). All three off by default; the Electron host opts
in when packaging release binaries.

See PROGRESS.md §"Phase 7" for the per-task breakdown.

## Phase 8 — Production Hardening | Complete | 100%

Phase 8 is the production-hardening sweep that fills the gaps left
by Phases 5–7 in the design-token / layout / brand-hub / image-studio
surfaces and adds the encryption-at-rest, design-review, and
artifact-publishing capabilities the proposal originally called for.
30 tasks across 6 blocks:

- **Block A (Tasks 1–6):** KChat artifact publishing pipeline +
  design-review annotation layer (`kcreate_core::annotation`,
  `kcreate_storage::annotations`).
- **Block B (Tasks 7–12):** Image Studio primitives —
  perspective transform (`kcreate_raster::transform`), color
  range selection (`kcreate_ai::color_range`), HSL +
  ColorBalance adjustment layers (`kcreate_raster::layer`),
  selection-based filter application.
- **Block C (Tasks 13–18):** page-numbering tokens with
  section restart (`kcreate_text::tokens`), brand-kit
  versioning with structured diff
  (`kcreate_storage::brand_versions`), job-first export
  presets (`kcreate_export::job_presets`).
- **Block D (Tasks 19–24):** constraint system for
  responsive frames (`kcreate_layout::constraints`),
  design-token binding with sub-100 ms propagation
  (`kcreate_core::token_binding`), smart text auto-fit
  (`kcreate_text::autofit`).
- **Block E (Tasks 25–28):** SQLCipher encryption at rest
  with PBKDF2-HMAC-SHA256 key derivation
  (`kcreate_storage::crypto`), startup performance
  optimisation, memory budget enforcement.
- **Block F (Tasks 29–30):** PROGRESS.md / PHASES.md /
  README.md / ARCHITECTURE.md / AGENTS.md updates.

All Phase 8 Rust + bridge work lives in
`crates/kcreate_bridge/src/phase8.rs` (workspace-level
helpers) and is exposed through 13 new N-API entry points in
`crates/kcreate_bridge/src/lib.rs`. The TypeScript wire format
is mirrored in `apps/desktop/shared/scene.ts` as
`Phase8Bridge`. The local-first sentinel
(`crates/kcreate_tests/tests/local_first.rs`) stays green —
none of the new code introduces a networking dependency to
the editing-path closure.

See PROGRESS.md §"Phase 8" for the per-task breakdown.

## Phase 9 — KChat extension depth, Home screen, design studio polish | Complete | 100%

Phase 9 closes the proposal-level gaps that Phase 8 deferred:
the KChat companion extension panels, the "Start from a
brief" Home screen flow, the AI palette / trace / icon-ify
actions, the ruler + grid + alignment surfaces in Design
Studio, the PSD / Penpot / EXIF / SVG-preview import edges,
and the memory-pressure + autosave + export-validation
robustness layer. 30 tasks across 6 blocks:

- **Block A (Tasks 1–6):** Phase 8 close-out + KChat
  extension depth — `ProjectBrowserPanel.tsx`,
  `ArtifactCard.tsx`, `SessionStatusBadge.tsx`,
  `ActivityFeed.tsx` in `apps/kchat-extension/src/`, plus
  the KCreate-side `kcreate://artifact?id=` deeplink.
- **Block B (Tasks 7–12):** Home screen — "Start from a
  brief" tile (`BriefModal.tsx`), Model status + GPU tier
  badge, Help & Learn grid, AI palette extraction, copy-fit
  text on layer resize, AI raster-to-vector trace.
- **Block C (Tasks 13–18):** PSD layered raster import
  (`kcreate_export::psd_import`), EXIF preservation
  (`kcreate_export::exif`), Penpot best-effort import
  (`kcreate_export::penpot_import`), `resvg`-backed SVG
  preview (`kcreate_export::svg_preview`), History panel +
  operation-log filter (`HistoryPanel.tsx`).
- **Block D (Tasks 19–24):** AI icon-ify
  (`kcreate_ai::iconify`), batch alt-text, ruler +
  measurement guides (`RulerOverlay.tsx`,
  `kcreate_storage::guides`), grid overlay
  (`GridOverlay.tsx`), multi-select alignment + distribution
  (`kcreate_core::align`, `AlignmentToolbar.tsx`).
- **Block E (Tasks 25–28):** memory pressure watchdog
  (`kcreate_bridge::perf::memory_watchdog_start`), project
  autosave with crash recovery (`kcreate_bridge::autosave`),
  export validation (`kcreate_export::validate`), stress /
  robustness regression coverage.
- **Block F (Tasks 29–30):** PROGRESS.md / PHASES.md /
  README.md / ARCHITECTURE.md / AGENTS.md updates.

All Phase 9 Rust + bridge work lives in
`crates/kcreate_bridge/src/phase9.rs` (workspace-level
helpers) plus `kcreate_bridge::perf` and
`kcreate_bridge::autosave`, exposed through ~30 new N-API
entry points in `crates/kcreate_bridge/src/lib.rs`. The
TypeScript wire format is mirrored in
`apps/desktop/shared/scene.ts`. The local-first sentinel
(`crates/kcreate_tests/tests/local_first.rs`) stays green —
the new `psd`, `kamadak-exif`, and `resvg` dependencies are
either pure-Rust or already in the editing path.

See PROGRESS.md §"Phase 9" for the per-task breakdown.

## Phase 10 — Image Studio AI, Vector/Layout AI, Export AI, Brand Hub & Plugin Marketplace, Performance Hardening | Complete | 100%

Phase 10 closes out the AI-assisted authoring story across
every studio surface, finishes the Export Center with live
previews + intelligent compression, lands the local plugin
marketplace, and tightens the performance envelope
(incremental scene diff, delta-compressed undo log, lazy
subsystem init). 30 tasks across 6 blocks:

- **Block A (Tasks 1–6):** Image Studio AI actions — NLM
  denoise (`crates/kcreate_ai/src/denoise.rs`),
  PatchMatch inpainting (`inpaint.rs`), auto colour
  (`auto_color.rs`), SAM segmentation tool with smart-select
  fallback (`AIAssistPanel.tsx` "Smart Segment"), magic-wand
  tool (`MagicWandTool.tsx`).
- **Block B (Tasks 7–12):** Vector / Layout / Brand Hub AI —
  stroke-style match (`stroke_match.rs`), glyph-from-photo
  extraction (`glyph_extract.rs`), reformat-to-deck
  (`reformat.rs`), brief-to-one-pager (`one_pager.rs`),
  palette harmonisation (`palette_harmonize.rs`), type
  pairing (`type_pairing.rs`).
- **Block C (Tasks 13–18):** Export Center polish —
  element-aware SVG optimiser
  (`crates/kcreate_export/src/svg_optimize.rs` with
  `protected_regions` / `with_unprotected` so it cannot
  mangle `<text>` / `<style>` / CDATA), SSIM-targeted
  smart compress (`smart_compress.rs`), live export preview
  (`ExportPreviewPanel.tsx`), floating contextual toolbar
  (`FloatingToolbar.tsx`), AI/Illustrator subset import
  (`ai_import.rs`).
- **Block D (Tasks 19–24):** Brand Hub + plugin
  marketplace — brand-to-brochure generator
  (`brand_template.rs`), local plugin marketplace
  (`crates/kcreate_plugin/src/marketplace.rs`,
  `PluginManager.tsx` "Marketplace" tab), multi-page PDF
  with TOC / outline / hyperlinks / per-page subset fonts
  (`pdf_multi.rs`), batch export progress UI
  (`BatchExportProgress.tsx`), workspace preferences
  panel (`PreferencesPanel.tsx`,
  `~/.kcreate/preferences.json`).
- **Block E (Tasks 25–28):** Performance hardening —
  acceptance-criteria perf bench suite
  (`crates/kcreate_tests/tests/acceptance_criteria.rs`),
  incremental scene diff with per-node `scene_version` +
  `DirtySet<Uuid>` (`crates/kcreate_bridge/src/scene_sync.rs`),
  undo-log delta compression + BLAKE3 blob-ref swapping
  (`crates/kcreate_core/src/operation_compress.rs` +
  `RuntimeConfig::{compress_undo_log,
  undo_blob_threshold_bytes}` + auto-detect on load in
  `crates/kcreate_storage/src/project_io.rs`), startup
  lazy-init for tile cache / LLM sidecar / memory watchdog
  with `bridge.<subsystem>.subsystem_ready` startup-timeline
  marks (`crates/kcreate_bridge/src/perf.rs`).
- **Block F (Tasks 29–30):** PROGRESS.md / PHASES.md /
  README.md / ARCHITECTURE.md / AGENTS.md updates.

All Phase 10 Rust + bridge work lives in
`crates/kcreate_bridge/src/phase10.rs`, exposed through new
N-API entry points in `crates/kcreate_bridge/src/lib.rs`.
The TypeScript wire format is mirrored in
`apps/desktop/shared/scene.ts`,
`apps/desktop/preload/src/preload.ts`, and
`apps/desktop/main/src/{bridge,main}.ts`. The local-first
sentinel (`crates/kcreate_tests/tests/local_first.rs`) stays
green — none of the new dependencies pull networking into
the editing-path closure.

See PROGRESS.md §"Phase 10" for the per-task breakdown.
