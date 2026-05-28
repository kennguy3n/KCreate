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

## Phase 8 — Production Hardening | In Progress

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
