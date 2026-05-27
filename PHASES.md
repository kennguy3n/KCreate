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

## Phase 7 — KChat Desktop Integration | Complete | 100%

First-party integration with the real KChat Desktop
(`uneycom/uney-chat-desktop`). New `kcreate_kchat_client` crate
speaks a JSON-RPC 2.0 protocol over a Unix-domain-socket /
named-pipe to the running uney-chat-desktop process, bridges
KChat communities / conversations / community-member rosters
into the existing collab gate. 30 tasks across 6 blocks:

- **Block A (Tasks 1–6):** local IPC client crate + JSON-RPC
  protocol + spec + transport + attestation bridging + bridge
  surface + mock-server tests.
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
  ARCHITECTURE.md / AGENTS.md / `protocol_spec.md` updates.

Local-first invariant preserved (`kcreate_kchat_client` and the
existing `kcreate_collab_transport` both stay out of the
editing-path closure walked by `crates/kcreate_tests/tests/local_first.rs`).
Feature flags: `kchat-desktop` (production client),
`kchat-dev-issuer` (local-mint for tests), `collab` (LAN
transport). All three off by default; the Electron host opts
in when packaging release binaries.

See PROGRESS.md §"Phase 7" for the per-task breakdown.
