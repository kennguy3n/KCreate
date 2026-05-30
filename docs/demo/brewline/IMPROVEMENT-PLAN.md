# KCreate — Brewline-demo critique & long-term improvement plan

> Companion to [`README.md`](./README.md) (the Brewline business demo +
> head-to-head assessment vs. Figma / Adobe / Canva / Sketch).
>
> The demo was run from `main @ 7416758` (post-Phase 11 merge) on a
> headless Linux VM. CPU-only, no GPU adapter, no internet egress,
> CPU bridge build. Every gap below was either visible on the screen
> or trivially reproducible from the test surface.
>
> **Philosophy.** Each section picks the *architecturally correct*
> long-term fix instead of the cheapest patch. KCreate's engine has
> consistently been ahead of its UI surface for several phases; the
> right answer almost always looks like "wire the existing engine
> through the bridge contract into a real React panel," not "add a
> flag to hide the gap."

---

## TL;DR — the engine is ahead of the UI

| # | Gap | What the user sees | Engine status | UI status | Effort | Priority |
|---|-----|--------------------|---------------|-----------|--------|----------|
| [G1](#g1--inline-text-editor--font-controls) | Inline text editor + font controls | Can drop a Text node, cannot change its content / font / size after creation | **Complete** — `kcreate_text` shaping (rustybuzz), font_db, outline, paragraph all wired through `text_frame_*` / `text_opentype_*` bridge entrypoints | **~40%** — `TextFramePanel` + `OpenTypePanel` exist for frame/OT options, but **no content edit field, no font picker, no size/weight UI, no inline canvas editor** | M | P0 |
| [G2](#g2--vector-pen-tool-pathfinder--node-editor) | Pen / Pathfinder / node editor for vector mode | Toolbar exposes only Select / Rect / Ellipse / Line | **Complete** — `kcreate_vector::boolean_operation` (Union/Intersect/Difference/Xor via `i_overlay`), `usvg` import, R-tree spatial index | **~10%** — no Pen tool, no Pathfinder panel, no point/handle node editor surface | L | P0 |
| [G3](#g3--brand-kit-template-scaffolding) | "Logo / Icon / Brand Kit" template opens an empty 1024×1024 artboard | **Complete** — `Project::brand_kits`, `upsert_brand_kit`, `DesignTokens`, `BrandKitVersion` | **~5%** — `CREATE_OPTIONS` entry only creates a blank artboard via `artboard.create`; no brand kit object, no palette, no type blocks | S | P1 |
| [G4](#g4--export-save-as-dialog) | Exports dump to `os.tmpdir()` with no file picker | **Complete** — `kcreate_export::{png,svg,pdf,webp,jpeg}` + `batch::run_batch_parallel` already write to any caller-supplied path | **~30%** — `ExportPanel.tsx` resolves `runtime.tempDir()` and concatenates a filename; never calls `dialog.showSaveDialog` | S | P1 |
| [G5](#g5--bundle-a-gguf-model-pack) | Brief, Accessibility, Vision, Reformat-as-deck all fail with "sidecar not ready" out of the box | **Complete** — `kcreate_ai::llm_sidecar`, `model_registry`, `llm_chat` loopback chat path all working when a model is on disk | **0%** — installer ships no GGUF; `Model Manager` panel surfaces a "Phase 1 does not bundle a download catalog" disclaimer | M | P0 |
| [G6](#g6--kchat-presence-end-to-end-wiring) | Presence / collab panel surfaces sign-in + trusted-issuer UI but the round-trip cannot complete in CPU builds | **Complete on the dev-issuer path** — `kchat_membership_status` / `kchat_derive_local_identity` / `kchat_trusted_issuers` exported and IPC-bound | **Partial** — needs the `kchat-backend` feature compiled into the shipped bridge + a default-trusted issuer pin + a tested sign-in flow | M | P2 |

Below: each gap with concrete file:line evidence, the long-term fix,
and the engineering tasks it would decompose into.

---

## G1 — Inline text editor + font controls

### Demo evidence

During the demo, three Text nodes were created via the Text tool
(visible in the Layers tree of
[screenshot 03](./screenshots/03-text-node-no-editor.png) and
[screenshot 14](./screenshots/14-inspect-css.png) — three sibling
`Text` entries below `Logo`). The node ships with the literal
string `"Text"` and font `"sans-serif"` at size `24` baked in at
create time. After creation:

- there is no surfaced way to change the **content** of the text,
- no font-family picker, font-size, or weight control anywhere in
  the right panel,
- no double-click-to-edit inline editor on the canvas.

Grep evidence:
- `apps/desktop/renderer/src/pages/EditorPage.tsx:1586-1594` is the
  only writer; arguments are literals.
- Workspace-wide grep for `setContent|set_content|update.*text.*content`
  inside `apps/desktop/` returns **zero** matches.
- Workspace-wide grep for `(double.*click|onDoubleClick).*[Tt]ext|inline.*edit|editingText`
  inside `apps/desktop/renderer/src` returns **zero** matches.

### Engine status — already complete

The Rust side is fully wired:

- **Font discovery + shaping** — `crates/kcreate_text/src/lib.rs:43`
  re-exports `shape_text`, `shape_text_with_features`,
  `shape_with_face`, `shape_with_face_and_features`,
  `opentype_features_to_buzz`. Backing crate is `rustybuzz` (pure CPU,
  no network — guaranteed by `crates/kcreate_text/src/lib.rs:17`).
- **Glyph outlines** — `crates/kcreate_text/src/outline.rs:55`
  (`outline_glyph`) walks `ttf-parser` and produces renderer-ready
  paths.
- **Paragraph layout** —
  `crates/kcreate_text/src/paragraph.rs:138` (`layout_paragraph`)
  drives wrap + hyphenation + autofit.
- **Bridge surface** —
  - `crates/kcreate_bridge/src/lib.rs:1569` (`canvas_create_text`) —
    delegates to `document::canvas_create_text(parent, x, y, text,
    font_family, font_size)`.
  - `crates/kcreate_bridge/src/phase2.rs:2411` (`text_frame_get`),
    `:2439` (`text_frame_update`), `:2489` (`text_layout_compute`),
    `:2618` (`text_opentype_features_get`),
    `:2639` (`text_opentype_features_update`).
  - `crates/kcreate_bridge/src/phase8.rs:427` (`text_set_auto_fit`).
  - `crates/kcreate_bridge/src/phase9.rs:485` (`text_autofit_recompute`).

### UI status — the missing ~60%

- **Tool creates with hard-coded defaults.**
  `apps/desktop/renderer/src/pages/EditorPage.tsx:1586-1594` calls
  `window.kcreate.canvas.createText(null, x0, y0, "Text",
  "sans-serif", 24)` — the content / family / size are literals.
  There is no way to set them at create time.
- **No content-edit channel.** A workspace-wide grep across
  `apps/desktop/{main,preload,renderer}` finds **zero** uses of
  `setContent`, `set_content`, `update_text`, `editingText`,
  `onDoubleClick.*Text`, etc. The text node, once created, is
  effectively immutable from the UI.
- **TextFramePanel covers the wrong axis.**
  `apps/desktop/renderer/src/components/TextFramePanel.tsx` and
  `OpenTypePanel.tsx` (rendered at
  `apps/desktop/renderer/src/components/RightPanel.tsx:450-457`)
  surface paragraph frame options (margins, wrap mode, auto-fit) and
  OpenType feature toggles — they explicitly do **not** edit content,
  font, or size.
- **No `set_node_text` / `set_node_font` bridge call.** The
  `canvas_create_text` path is the only writer; no symmetric
  `set_text_content` / `set_text_style` exists in the bridge.

### Architecturally correct long-term fix

This is the right shape — **don't paper over it with a flag**:

1. **Add three new bridge entry points in
   `crates/kcreate_bridge/src/lib.rs` (thin), backed by
   `crates/kcreate_bridge/src/document.rs` (real):**
   ```
   set_text_content(node_id, content)
   set_text_style(node_id, TextStyleWire { family, size, weight,
                                           italic, align, fill })
   text_replace_range(node_id, start, end, replacement)   // for inline editor
   ```
   Use the existing `TextStyleWire` shape already present at
   `crates/kcreate_bridge/src/phase2.rs:2604`. Each writer must (a)
   take the workspace **write** lock, (b) push to the operation log
   so undo works, (c) call `kcreate_text::shape_text` to refresh
   shaped runs, (d) `mark_bounds_changed` so the R-tree invalidates.
   This honours AGENTS.md Rules 1 & 4 (no stubs, wire format
   lockstep) and the `with_workspace_mut` discipline cited in
   AGENTS.md "Architecture contract → Workspace access."

2. **Mirror to `apps/desktop/shared/scene.ts`,
   `apps/desktop/main/src/bridge.ts`,
   `apps/desktop/main/src/main.ts`, and
   `apps/desktop/preload/src/preload.ts`** so the renderer sees them
   as `window.kcreate.text.setContent / setStyle / replaceRange`.

3. **Extend `RightPanel.tsx`** at the `node.nodeType === "TextLayer"`
   branch (file:`apps/desktop/renderer/src/components/RightPanel.tsx:450`)
   to render a new `TextStylePanel` ABOVE the existing
   `TextFramePanel`:
   - Content `<textarea>` bound to `set_text_content`.
   - Font-family combobox populated from
     `window.kcreate.text.listFonts()` (which would wrap
     `kcreate_text::font_db`).
   - Size `<input type="number">`, weight segmented control
     (100..900), italic toggle, align icons (4), fill via the
     existing `FillSection`.
   All bindings should commit on blur + Cmd/Ctrl-Enter to keep undo
   chunks coherent.

4. **Inline canvas editor.** Add a single `onDoubleClick` handler in
   `apps/desktop/renderer/src/components/CanvasHost.tsx`
   that, when the hit-tested node is a `TextLayer`, mounts a
   contenteditable `<div>` positioned over the node's bounding box,
   styled to match the resolved shaped glyphs (font, size, line
   height) — on blur it pushes via `text_replace_range` (a single
   replace 0..content_length op so undo collapses naturally). This
   mirrors how Figma / Sketch / Affinity Designer all implement
   inline text editing and avoids re-implementing a text editor: we
   only need range-replace because rustybuzz re-shapes the entire
   run anyway.

5. **Tests.** New `crates/kcreate_tests/tests/text_editor.rs`
   exercising create → set_content → set_style → undo → redo round
   trip on the bridge surface, asserting bounds change on each step
   and that the operation log is well-formed.

### Why not a quick patch

A boolean "show inline editor" flag is the wrong abstraction — there
is no inline editor to flag on. A "Text" panel with locally-cached
React state that fires `canvas_create_text` on edit would break undo,
re-create the node with a new UUID on every keystroke, and silently
discard the existing TextFrameOptions / OpenTypeFeatures. The fix
above is the smallest change that respects the existing engine
contract and operation log.

### T-shirt size

**M** (~4 engineer-days). Bulk of the work is the three bridge
methods + the inline editor; the panel UI is straightforward once
`set_text_style` exists.

---

## G2 — Vector Pen tool, Pathfinder & node editor

### Demo evidence

The Design-mode toolbar (visible in
[screenshot 03](./screenshots/03-text-node-no-editor.png) — 
*Select / Rect / Ellipse / Line / Text*) is the entire vector-create
surface; the dedicated **Vector** mode tab loads the same toolset.
There is no Pen tool, no boolean-ops ("Pathfinder") panel, and no
node-edit mode to manipulate the on-curve / off-curve points of an
existing path.

Grep evidence:
- `apps/desktop/renderer/src/shortcuts/registry.ts:41-102` lists
  `toolSelect / toolRect / toolEllipse / toolLine / toolText` — no
  `toolPen` entry.
- Workspace-wide grep for
  `"pen"|tool === "pen"|booleanOperation|BooleanOp|pathfinder`
  inside `apps/desktop/renderer/src` returns **zero** matches.

### Engine status — already complete

- **Boolean operations** — `crates/kcreate_vector/src/boolean.rs:58`
  (`pub fn boolean_operation`) takes a `BooleanOp` enum
  (`crates/kcreate_vector/src/boolean.rs:26` —
  `Union / Intersect / Difference / Xor`) and dispatches to
  `i_overlay`'s `OverlayRule`. Three unit tests already cover the
  hot paths (union, intersect, subtract at lines 161, 174, 185).
- **SVG import / export** — `crates/kcreate_vector` ships `usvg`
  consumption (per AGENTS.md crate layout).
- **Spatial index** — R-tree via `rstar` for fast hit-tests on the
  manipulated path.

### UI status — the missing surface

- **Tool registry has no Pen entry.**
  `apps/desktop/renderer/src/shortcuts/registry.ts:41-102` lists
  `toolText` (`t`), `toolSelect`, `toolRect`, etc., but **no**
  `toolPen`. The toolbar component renders directly from this
  registry.
- **No path-editing renderer surface.** The canvas presenter draws
  vector layers from `kcreate_renderer`'s display list; there is no
  overlay component that draws the editable points + control
  handles + tangents of a `VectorLayer`'s path data.
- **No Pathfinder panel anywhere in
  `apps/desktop/renderer/src/components/`.**

### Architecturally correct long-term fix

This is a **three-track** project, each shippable independently:

#### G2.a — Pen tool

1. Add `toolPen` to
   `apps/desktop/renderer/src/shortcuts/registry.ts:41` (Figma /
   Illustrator / Sketch all use `p`).
2. Add a Pen-mode drag handler in
   `apps/desktop/renderer/src/pages/EditorPage.tsx` analogous to the
   existing `drag.tool === "rect" | "ellipse" | "line"` block at
   `:1557-1605`. Click = anchor with no handles; click-drag = anchor
   + symmetric tangent handles; Alt+drag on existing anchor = break
   symmetry; Enter = close path (or click first anchor).
3. New bridge entry point
   `vector_create_path(parent, points: Vec<{ x, y, in_handle?,
   out_handle? }>, closed: bool)` in
   `crates/kcreate_bridge/src/document.rs` (logic) +
   `crates/kcreate_bridge/src/lib.rs` (thin N-API). Reuses the
   existing `NodeStyle` fill/stroke path.
4. Live-preview overlay component in
   `apps/desktop/renderer/src/components/CanvasHost.tsx` that draws
   the under-construction path on top of the presenter readback
   (HTML/SVG overlay — not a Rust render — so it doesn't pay the
   full pipeline cost on every cursor move).

#### G2.b — Pathfinder panel

1. New `apps/desktop/renderer/src/components/PathfinderPanel.tsx`,
   mounted in `RightPanel.tsx` at the `node.nodeType ===
   "VectorLayer"` branch when **two or more** vector layers are
   selected (gated by a new `selection.size() >= 2 &&
   selection.every(is_vector)` check).
2. Four buttons → bridge entry point
   `vector_boolean_op(target_ids: Vec<Uuid>, op: BooleanOp)` in
   `crates/kcreate_bridge/src/document.rs` (logic) +
   `crates/kcreate_bridge/src/lib.rs` (thin). The bridge method
   takes the workspace write lock, resolves each target's flattened
   path via the existing `kcreate_vector` API, calls
   `boolean_operation` on the multi-path stack (fold left), creates
   a single replacement `VectorLayer` node, deletes the inputs, and
   logs a single composite operation so undo restores all inputs at
   once.

#### G2.c — Node editor mode

1. Tool-mode addition: `mode === "vector-edit"` (toggle on the
   `vector-path` toolbar by clicking an already-selected vector,
   or via shortcut `Enter` from select mode — matches Illustrator).
2. Overlay component
   `apps/desktop/renderer/src/components/VectorNodeOverlay.tsx`
   reads the node's path data from a new
   `vector_path_get(node_id)` bridge entry point and renders
   draggable point markers + tangent handles in absolute viewport
   coordinates.
3. Drag commits via `vector_path_set_point(node_id, index, point)`
   (single-point ops) or `vector_path_set(node_id, points)` (drag
   end) — same fold-into-undo discipline as G1.

### Why not a quick patch

A single "Pen tool" button that creates straight-line polylines would
make the toolbar *look* more complete while making the engine
contract worse (no curve handles → can't round-trip with the
existing SVG import path → corrupts user expectations). The
three-track plan keeps each shipped feature whole.

### T-shirt size

- **G2.a Pen tool**: M (~5 days)
- **G2.b Pathfinder panel**: S (~2 days — engine is done)
- **G2.c Node editor**: L (~10 days — the overlay + drag semantics
  are subtle, especially with smart guides + snap)

---

## G3 — Brand-kit template scaffolding

### Demo evidence

Screenshot **[01-homepage](./screenshots/01-homepage.png)** shows
the "Logo / Icon / Brand Kit" tile on the HomePage. Clicking it
opens a blank 1024×1024 artboard named `Logo` — no palette swatches,
no type blocks, no brand kit object in the project, and the user
has to manually navigate to `BrandKitEditor` to start.

### Engine status — already complete

- `crates/kcreate_core/src/project.rs:126` — `BrandKit` struct.
- `:559` — `Project::upsert_brand_kit`.
- `:569` — `Project::brand_kit(id)` lookup.
- `:56` — `DesignTokens` struct, instantiated at `:281` on project
  creation.
- `crates/kcreate_storage/src/project_io.rs:1041` —
  `save_brand_kit` persists to the `brand_kits` SQLite table.
- `crates/kcreate_bridge/src/phase8.rs:541` — `brand_kit_save_version`
  version-tracked save.

### UI status — empty hand-off

- `apps/desktop/renderer/src/pages/HomePage.tsx:42-48` —
  ```
  { id: "brand",
    title: "Logo / Icon / Brand Kit",
    blurb: "Vector marks, palettes, type",
    nodeType: "Artboard",
    defaultArtboard: { name: "Logo", width: 1024, height: 1024 } }
  ```
- `apps/desktop/renderer/src/App.tsx:31-48` — the resulting click
  only calls `window.kcreate.artboard.create(null, "Logo", 1024,
  1024)`. No `brand_kit_*`, no `design_tokens_*`, no seeded text
  layers.

### Architecturally correct long-term fix

The cheap version ("just call upsert_brand_kit with empty data on
this template") leaves all the work on the user. The architecturally
correct version is to **promote this code path into a typed template
resolver** that every "Create" tile can route through:

1. Add a new module
   `apps/desktop/renderer/src/lib/templates.ts` that exports a
   `TemplateResolver` interface and one resolver per
   `CREATE_OPTIONS.id`. Each resolver returns:
   ```
   {
     artboard: { name, width, height },
     brandKit?: BrandKit,
     designTokens?: DesignTokens,
     seedNodes?: NodeSeed[],   // wordmark text + mark vector + 4
                               // colour-swatch rects + 2 type blocks
   }
   ```
2. `App.tsx` `handleOpenEditor` (currently file:line 24-56)
   becomes resolver-aware: after `openScratchProject` it calls
   `resolver.apply(workspace)`, which fans out to the existing
   bridge entry points
   (`artboard.create`, `brandKit.upsert`,
   `designTokens.update`, `canvas.createText`,
   `vector.createPath` once G2.a lands, etc.).
3. The `brand` resolver seeds:
   - one `Logo` artboard (existing),
   - one `BrandKit` with a placeholder name (`"Brand Kit"`) and the
     project's default tokens,
   - a wordmark `TextLayer` at the top with size 96 (so G1's font
     picker is exercised),
   - a placeholder `VectorLayer` mark on the bottom-left,
   - a 4×1 row of colour-swatch rects across the middle,
   - two 32pt + 16pt type sample blocks on the right.
4. Tests in `crates/kcreate_tests/tests/template_resolver.rs`
   asserting that the post-resolve scene graph for each template id
   matches a frozen snapshot (the same approach used elsewhere in
   the workspace).

### Why not a quick patch

Hard-coding the brand-kit seeding inside `App.tsx`'s
`handleOpenEditor` would mean every new template type forks the same
function. Factoring out `TemplateResolver` once means the Photo /
Deck / Print / Dev-Export tiles all get the same scaffolding hook
for free.

### T-shirt size

**S** (~2 days). Engine and bridge are fully ready; this is
~200 lines of resolver code + a snapshot test.

---

## G4 — Export "Save as…" dialog

### Demo evidence

Screenshot **[18-export-panel](./screenshots/18-export-panel.png)**
shows the Export panel correctly listing PNG/SVG/PDF/WebP/JPEG +
5 batch presets + Icon Pack. During the demo, each export
deposited a real file under `os.tmpdir()` (`/tmp/kcreate-export-…`),
so the engine pipeline is working — but the user is never asked
**where** to save.

### Engine status — already complete

- `crates/kcreate_export/src/lib.rs` exports a typed
  `ExportConfig::{png,svg,pdf,webp,jpeg}` API.
- `crates/kcreate_export/src/batch.rs` (`run_batch_parallel`)
  already accepts an arbitrary output directory.
- Electron already calls `dialog.showSaveDialog` for **project save**
  (`apps/desktop/main/src/main.ts:1756`), so the dialog stack is
  proven inside the shipped app.

### UI status — the missing call

- `apps/desktop/renderer/src/components/ExportPanel.tsx:138` —
  `const [tempDir, setTempDir] = useState<string>("");`
- `:147-148` — `window.kcreate.runtime.tempDir().then((d) =>
  setTempDir(d))`.
- `:162-163` —
  ```
  const ts = Date.now();
  const out = `${tempDir}/kcreate-export-${ts}.${formatExt(format)}`;
  ```
- A workspace-wide grep for `showSaveDialog|outputDir|targetDir`
  inside `ExportPanel.tsx` returns **zero** matches. The bridge has
  no `runtime.chooseSaveTarget` either.

### Architecturally correct long-term fix

1. **New IPC channel + bridge method.** Add
   `kcreate/runtime/chooseExportTarget` in
   `apps/desktop/main/src/main.ts` that wraps Electron's
   `dialog.showSaveDialog` with extension/quality presets per format
   and returns either an absolute file path or `null` (cancelled).
   Surface via
   `window.kcreate.runtime.chooseExportTarget(format)` in
   `preload/src/preload.ts` and `shared/scene.ts`.
2. **For batch presets**, expose a sibling
   `chooseExportDirectory()` (wrapping `showOpenDialog` with
   `properties: ['openDirectory', 'createDirectory']`) that returns
   a directory or `null`.
3. **Rework `ExportPanel.handleExport`** at
   `apps/desktop/renderer/src/components/ExportPanel.tsx:155-…` so
   the flow is: gather options → `await
   chooseExportTarget(format)` → bail on null → call existing
   `window.kcreate.export.{png,svg,pdf,webp,jpeg}` with the chosen
   path. Remove the `tempDir` plumbing entirely (the temp dir was a
   shim to give the renderer *any* writable path; once the dialog
   lands it is no longer needed).
4. **Persist last-used directory per format** in
   `kcreate_storage`'s app-preferences table (the same one that
   stores Recents) so the dialog opens at the expected location on
   re-use.
5. **Drop the "Files write to /tmp" disclaimer** from the panel
   header once the dialog is wired.

### Why not a quick patch

Adding a free-text "Output path:" text field in the panel and
trusting the user to type a valid path would (a) bypass OS
sandboxing rules that some platforms enforce on Electron, (b) lose
the per-format extension filter, (c) lose the
`overwrite-confirmation` modal that comes free with
`showSaveDialog`.

### T-shirt size

**S** (~1 day). Pure wire-up; engine and Electron dialog already
exist.

---

## G5 — Bundle a GGUF model pack

### Demo evidence

- Screenshot **[08-model-manager](./screenshots/08-model-manager.png)**
  shows the Model Manager surfacing **STOPPED · Tier 2 · GGUF path:
  (none) · installed packs: 0**.
- Screenshot **[17-accessibility-sidecar-error](./screenshots/17-accessibility-sidecar-error.png)**
  shows accessibility check returning *"sidecar not ready"*.
- Screenshot **[21-brief-sidecar-error](./screenshots/21-brief-sidecar-error.png)**
  shows the Brief modal failing for the same reason.

Three high-value AI surfaces (Brief, Accessibility, Reformat-as-deck)
are reachable but cannot complete a single call. Vision sidecar is
identical (image-gen FLUX is a separate Python sidecar — see
`tools/kcreate_diffusion/`).

### Engine status — already complete

- `crates/kcreate_ai/src/llm_sidecar.rs` — full lifecycle: spawn
  `llama-server`, health-probe, bearer-token auth (Phase 11
  hardened), graceful shutdown, fail-closed CSPRNG.
- `crates/kcreate_ai/src/llm_chat.rs` — loopback chat client.
- `crates/kcreate_ai/src/model_registry.rs` — pack lifecycle: list,
  resolve, mark active.
- `crates/kcreate_ai/src/mlx_sidecar.rs` — MLX directory variant
  (Apple Silicon).

The whole pipeline works the moment a `.gguf` is on disk; this is
purely a packaging gap.

### UI status — model manager visible but disclaiming

The Model Manager panel renders the registry list correctly. The
**phase-1-bundling disclaimer** at the top of the panel is the
honest acknowledgement that the installer ships no weights.

### Architecturally correct long-term fix

This is a **build / packaging** problem, not a code problem. The
right shape is:

1. **Add a build-time `fetch-models` step** to
   `apps/desktop/scripts/` (new file `fetch-default-models.mjs`)
   that downloads, into `apps/desktop/resources/models/`:
   - **Qwen2.5-1.5B Instruct Q4_K_M** (~1.1 GB) — the small,
     license-clean default that runs at usable speed on CPU.
   - The matching `mmproj-*-Q4_K_M.gguf` (for Vision parity once
     paired with a `Qwen2-VL-2B`-class instruct GGUF — gated by
     a `--with-vision` build flag because of the extra ~2 GB).
   The script must be **deterministic** (pinned BLAKE3) and **fail
   closed** on hash mismatch so a corrupted CDN cannot ship.
2. **Extend the electron-builder config** (or whatever packager the
   repo settles on) to include `apps/desktop/resources/models/**`
   in the installer payload. Document the resulting installer-size
   bump (~1.1 GB → ~2.5 GB MSI/DMG/AppImage) in the README.
3. **First-run unpack hook** in
   `apps/desktop/main/src/main.ts`: on first launch, copy
   `process.resourcesPath/models/*` into a stable user dir
   (`app.getPath('userData') + '/models/'`) and call
   `model_registry.register_pack(path)` over the bridge.
4. **Out-of-band download path for advanced users.** Add a "Manage
   models…" subpanel in `ModelManager.tsx` with a curated catalogue
   (name, size, license, BLAKE3) and a "Download to user data"
   button that uses Electron's `net.fetch` with explicit BLAKE3
   verification — *not* a freeform URL field. This is the only
   network-touching code in the renderer and it must stay opt-in
   (preserves the local-first invariant: editing-path crates remain
   network-free; the downloader sits in the renderer only).
5. **CI lane** that runs the `--without-models` flavor on every PR
   (so we don't slow the fast lane) and the full `--with-models`
   flavor on the gated full-CI matrix.

### Why not a quick patch

Hard-coding a `model_registry.set_path("/some/known/location")` at
startup would shift the problem from "user has to source a model"
to "user has to magic up the right binary at the right path" and
fails on every clean install. The right fix is to ship the bits.

### T-shirt size

**M** (~3 days for the script + builder integration + first-run
unpack + tests; installer-size review is the longest single item).

---

## G6 — KChat presence: end-to-end wiring

### Demo evidence

Screenshot **[20-presence-kchat](./screenshots/20-presence-kchat.png)**
shows the Presence panel with a sign-in form, trusted-issuer pinning
UI, and "membership status" placeholders. Calls succeed at the IPC
layer (the bridge does have the handlers) but the actual round-trip
to a backend cannot complete in a CPU-only sandbox build that has
not been compiled with the `kchat-backend` feature.

### Engine status — already complete

- IPC handlers bound:
  - `apps/desktop/main/src/main.ts:3064` — `ipcMain.handle("kcreate/kchat/status", …)`
  - `:3068-3069` — `…/derive-local-identity` → `kchatDeriveLocalIdentity`
  - `:3111-3112` — `…/trusted-issuers` → `kchatTrustedIssuers`
- Native exports:
  - `crates/kcreate_bridge/src/lib.rs:3607` — `kchat_membership_status`
  - `:3633` — `kchat_derive_local_identity`
  - `:3672` — `kchat_trusted_issuers`
- Logic:
  - `crates/kcreate_bridge/src/collab.rs:3603` — `kchat_membership_status` (returns `KChatMembershipStatus`).
  - `:3740` — `kchat_derive_local_identity` (deterministic Ed25519 derivation).
- Two production paths:
  - **`kcreate_kchat`** crate — dev-side issuer (`kchat-dev-issuer`
    feature on `kcreate_bridge`).
  - **`kcreate_kchat_client`** crate — Phase 7 production REST
    client against the shared uney-chat backend (`kchat-backend`
    feature on `kcreate_bridge`).

### UI status — needs default pinning + ship-with-feature

- The Presence panel is built. Sign-in form renders. Trusted-issuer
  list renders.
- The shipped bridge in this demo build was compiled **without**
  `kchat-backend`, so `kchat_membership_status` returns
  "not configured" rather than an actual sign-in flow.
- No default trusted issuer is pre-pinned, so even with the feature
  enabled, the first-launch UX is "paste an Ed25519 public key" —
  which the realistic user cannot do.

### Architecturally correct long-term fix

1. **Compile the production bridge with `--features
   kchat-backend`** (and `collab` for the QUIC transport) in the
   release lane. Keep the local-first sentinel green: the
   `kcreate_kchat_client` crate is already excluded from the
   editing-path closure in
   `crates/kcreate_tests/tests/local_first.rs` (see AGENTS.md
   "Collab feature isolation").
2. **Ship a default trusted issuer pin.** Add a build-time JSON
   manifest at `apps/desktop/resources/kchat/trusted-issuers.json`
   listing the shared uney-chat issuer's Ed25519 public key +
   issuer URL. On first launch the renderer (or main) pre-populates
   the trusted-issuer table via `kchatTrustedIssuersAdd(...)` so
   the user does not have to paste keys.
3. **Add an end-to-end happy-path test** to
   `crates/kcreate_tests/tests/kchat_signin.rs` (gated by feature)
   that stands up a `tiny_http` mock backend, signs an attestation,
   and asserts the bridge surfaces "membership: active" + the
   correct expiry. This catches regressions in the IPC contract
   between Electron and the bridge.
4. **Renderer: surface the actual flow.** Replace the placeholder
   "membership status" text in
   `apps/desktop/renderer/src/components/PresencePanel.tsx`
   (and/or `KChatSignInPanel.tsx`) with a real status pill that
   reads from a polled `kchatMembershipStatus()` and routes the
   user to the sign-in modal only when `status === "not-signed-in"`.
5. **Document the LAN-vs-backend split** in
   `docs/COLLABORATION.md` (new file) so the user understands that
   QUIC LAN collab needs `--features collab` and KChat-gated cloud
   sync needs `--features kchat-backend`.

### Why not a quick patch

A "Demo mode" toggle that fakes a successful sign-in for screenshots
would lie to the user and obscure the real packaging fix. The work
above is straightforward: it's the missing 20% of an otherwise
production-ready Phase 7 stack.

### T-shirt size

**M** (~3 days: builder feature flags + default issuer pin + e2e
test + renderer polish).

---

## Cross-cutting follow-ups

| # | Topic | Where | Why it matters |
|---|-------|-------|----------------|
| X1 | **Engine-vs-UI surface audit** | All `crates/kcreate_*` ↔ `apps/desktop/renderer/src/components/*` | Every gap above is a special case of the same pattern. A periodic audit (new RFC under `docs/audits/`) listing every bridge entry point alongside its renderer consumer would surface the next G1/G2-class drift before a user sees it. |
| X2 | **`canvas_create_*` symmetry** | `crates/kcreate_bridge/src/lib.rs:1569` (`canvas_create_text`) lacks a sibling `canvas_set_*` for every settable field | The text gap is the worst case of this; the same shape exists for vector layers (no `vector_set_fill / set_stroke / set_path` outside of phase-specific writers). A single "every CRUD has a Create + Update + Delete with operation-log support" rule would catch this. |
| X3 | **AGENTS.md "Where new code goes" sync** | `AGENTS.md` table | Some of the panels referenced by the fixes (TextStylePanel, PathfinderPanel, VectorNodeOverlay) do not exist yet — add them to the AGENTS.md routing table at creation time so future agents don't re-derive their paths. |
| X4 | **Local-first sentinel for new crates** | `crates/kcreate_tests/tests/local_first.rs` | Each of the fixes above adds a renderer dependency (Electron `dialog`, `net.fetch` for the model downloader); none of them should cause the editing-path closure to pull in a network crate. The existing sentinel already guards this; just keep it green on every PR. |
| X5 | **Phase 12 charter** | `PHASES.md` + `PROPOSAL.md` | The six gaps above sort naturally into a coherent Phase 12 ("Surface parity") proposal. Worth scoping formally rather than landing as a string of one-off PRs. |

---

## Suggested sequencing (3 sprints)

**Sprint A (P0, ~1 week):**
- G1 (text editor) — unblocks ~70% of real design work.
- G5 (GGUF bundling) — unblocks every AI panel in the demo.

**Sprint B (P0/P1, ~2 weeks):**
- G2.a (Pen tool) + G2.b (Pathfinder) — unblocks vector parity vs.
  Illustrator / Affinity.
- G3 (Brand-kit template scaffolding) — unblocks the SMB onboarding
  path the demo exercised.
- G4 (Export Save-as) — closes the biggest "feels unfinished" UX gap
  visible in screenshots.

**Sprint C (P2, ~1 week):**
- G2.c (Node editor) — completes the vector story.
- G6 (KChat end-to-end) — unblocks live multiplayer on the shipped
  binary.

After Sprint C, KCreate's UI surface is approximately at parity with
the engine for every workflow exercised in the Brewline demo.
