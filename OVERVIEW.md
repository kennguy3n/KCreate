# KCreate — Product Overview

KCreate is a **local-first, workflow-first** design suite for individuals
and small teams who refuse to upload their work to ship it. This document
is the product overview; see `ARCHITECTURE.md` for the technical design.

---

## 1. Product definition

KCreate is a single desktop application that bundles seven studios behind a
unified mode switcher. The studios are:

```
KCreate
├── Home / Project Launcher
├── Design Studio          (UI, social, posters, slides)
├── Vector Studio          (logos, icons, illustration)
├── Image Studio           (photo cleanup, background removal, retouch)
├── Layout Studio          (decks, proposals, multi-page PDF)
├── Brand & Asset Hub      (colors, fonts, logos, spacing, presets)
├── Local AI Studio        (model packs, action history, permissions)
├── Export Center          (PNG, SVG, PDF, WebP, batch export)
└── MCP / Plugin Hub       (signed local tools, sandboxed panels)
```

**UX thesis.** Most design tools force users to pick a discipline before
they pick a goal ("which app do I open to make a flyer?"). KCreate inverts
that by leading with the *job* ("make a poster", "make an app screen",
"clean up a product photo") and routing to the right studio with the
right defaults. Once inside an artboard, the mode switcher lets the user
fluidly cross studios without an export-import dance.

---

## 2. Reference learning model

KCreate borrows liberally from the open-source DTP / vector / raster
ecosystem and ignores everything that hurts new-user momentum.

| Reference     | What it learns from                                                                             | What it avoids                                      |
| ------------- | ----------------------------------------------------------------------------------------------- | --------------------------------------------------- |
| Penpot        | Web-grade UX, components, design tokens, multi-page artboards, prototyping primitives.          | Online-only assumption; opinionated team workflow.  |
| Inkscape      | Deep, precise vector tools; SVG fidelity; node editing; powerful boolean ops; extensions.       | Crowded, mode-heavy UI; GTK-rooted toolbars.        |
| GIMP          | Full raster pipeline; layers, masks, filters, scriptable plug-ins.                              | Single-document-window legacy; obscure shortcuts.   |
| Scribus       | Print preflight, master pages, CMYK, ICC profiles, real typography for multi-page documents.    | XPress-era UI; weak preview; idiosyncratic exports. |

The product line is "Penpot's UX, Inkscape's precision, GIMP's raster
depth, Scribus's print-ready output — running locally, no account
required."

---

## 3. Core product principles

1. **Local-first.** Everything works offline. The user owns their files,
   their AI history, and their project bytes. Cloud features, where they
   exist, are opt-in.
2. **Open formats.** Native format is a transparent folder package
   (`.kstudio/`). Round-trips include SVG, PNG, PDF, WebP. Import covers
   Figma JSON, Sketch JSON, Adobe SVG/AI subset, PSD layered raster, and
   Penpot.
3. **Workflow-first.** Home screen leads with goals, not tools. Mode
   switching is free.
4. **AI as assistant, not autopilot.** Every AI action follows the same
   **Ask → Preview → Apply → Edit → Undo** loop. Nothing irreversible. No
   silent edits. Every AI action is logged.
5. **Resource-aware.** KCreate detects the device tier and adapts. A 4 GB
   laptop runs the editor; a 32 GB workstation runs the editor and the
   larger model packs.
6. **Privacy by default.** No telemetry. No background uploads. AI runs on
   the user's hardware.
7. **Composable.** The MCP server exposes the editor to local AI agents;
   plug-ins extend the editor without leaving the sandbox.

---

## 4. Functional modules

### 4.1 Home / Project Launcher

**Purpose.** Get the user from "I have a job to do" to "I'm drawing" in
two clicks.

**Features.**
- Job-first "Create new" tiles (one click per job):
  - App / Website UI
  - Logo / Icon / Brand Kit
  - Social Media Post
  - Product Photo Cleanup
  - Pitch Deck / Proposal
  - Flyer / Poster / Brochure
  - Developer Asset Export
  - Import Existing File
- Recent projects with thumbnails (read from local storage, no network).
- Model status: shows the detected device tier + GPU.
- Help & Learn: links to first-run tutorials.

**UX advantage over references.** Removes the "blank canvas paralysis"
that Inkscape and GIMP impose on new users.

**AI features.** A "Start from a brief" tile opens a local LLM prompt; the
model fills out an artboard preset, palette, and starter layers. The user
can always discard it.

**Quality targets.**
- Time to first drawable artboard < 3 s on a tier-1 device.
- All tiles work offline with zero network requests.

### 4.2 Design Studio

**Purpose.** The everyday "design something" surface: UI, social, posters,
multi-artboard layouts.

**Features.**
- Multi-artboard canvas with named frames.
- Components and instance overrides (Penpot pattern).
- Design tokens: colors, typography, spacing, radii, shadows.
- Auto-layout (flex/grid) on frames.
- Boolean operations, paths, text, raster image layers.
- Constraints for responsive frames.

**UX advantage.** No mode dance to flip between vector + raster + text.
One canvas, real tools.

**AI features.** Auto-layout suggestions, palette extraction from a photo,
copy-fit text on layer resize, alt-text generation, accessibility report.

**Quality targets.**
- 60 fps pan/zoom on a 1 000-node artboard at tier 1.
- Round-trip to SVG and Figma JSON.

### 4.3 Vector Studio

**Purpose.** Precision vector work — logos, icons, illustration.

**Features.**
- Bezier node editor with smart guides, snapping, alignment.
- Boolean ops (union, subtract, intersect, exclude).
- Pencil / pen / shape tools.
- Live path operations (offset, simplify, smooth).
- Variable stroke width.
- SVG import / export with no loss for standard primitives.
- Multi-stroke / multi-fill on a single path.

**UX advantage over Inkscape.** Floating contextual toolbars instead of
permanent rails; the node tool surfaces inline, not modally.

**AI features.** "Trace this raster", "icon-ify this drawing", "match this
stroke style", "extract glyph from photo".

**Quality targets.**
- Boolean ops produce identical output on a shared corpus.
- SVG export passes W3C validation.

### 4.4 Image Studio

**Purpose.** Raster work — product cleanup, retouching, light photo
editing. *Not* a Photoshop replacement; a "GIMP that doesn't punish you
for being new".

**Features.**
- Layers, masks, adjustment layers (non-destructive).
- Filters (blur, sharpen, color, levels, curves).
- Crop, rotate, perspective, healing brush.
- Magic wand, color range, smart select.
- Background removal (local AI).
- Tile engine for memory-bounded huge-image editing.

**UX advantage over GIMP.** Single-window, a real adjustment-layer stack,
modern keyboard shortcuts, hover-preview filters.

**AI features.** Background removal, upscale, denoise, object removal
(exemplar inpaint), auto color, segmentation-based selection.

**Quality targets.**
- A 64 MP image opens in < 2 s at tier 1.
- Background removal runs locally and finishes in < 5 s for a 4 MP photo
  at tier 2.

### 4.5 Layout Studio

**Purpose.** Multi-page documents — decks, proposals, brochures,
print-ready PDFs.

**Features.**
- Pages, master pages, page-numbering tokens.
- Text frames with proper typography (kerning, OpenType features,
  hyphenation).
- Inline images, flowing text, image-text wraps.
- CMYK and ICC color profiles.
- PDF preflight (overprint, bleed, font embedding).
- Templates for deck and proposal layouts.

**UX advantage over Scribus.** Modern type controls, drag-to-flow, a clean
panel grid.

**AI features.** "Reformat this content into a 16:9 deck", "extend this
brand to a brochure template", "fit this brief into a one-pager".

**Quality targets.**
- A 50-page document with 100 images opens in < 4 s at tier 1.
- PDF preflight catches missing bleed, missing fonts, and RGB images in a
  CMYK document.

### 4.6 Brand & Asset Hub

**Purpose.** A first-class home for brand kits — not a buried "libraries"
panel.

**Features.**
- Multiple brand kits per project.
- Per-kit: colors, fonts (linked to system or embedded), logos, spacing
  scale, radii, shadows, export presets.
- Token references across all studios.
- Versioning with diff view.
- Sharable as `.kbrand` files.

**UX advantage.** Brands are top-level objects, not hidden in "styles".

**AI features.** "Extract a brand from this PDF", "harmonize this palette",
"suggest a complementary type pairing".

**Quality targets.**
- Editing a brand token updates every linked layer in < 100 ms.
- `.kbrand` round-trip preserves all tokens.

### 4.7 Export Center

**Purpose.** Get clean files out of KCreate without surprises.

**Features.**
- One-click export to PNG, SVG, PDF, WebP, JPEG.
- Per-artboard, per-layer, or per-selection exports.
- Export presets (Web 1x/2x/3x, iOS asset catalog, Android density
  buckets, Print 300 dpi).
- Batch export.
- Slice export with named regions.

**UX advantage.** Live preview of the exported bytes; a preset library
keyed to the job-first tiles.

**AI features.** "Optimize this SVG", "compress this raster without visible
loss", "generate alt text".

**Quality targets.**
- A 50-asset batch export completes in < 5 s at tier 2.
- SVG export validates and round-trips through `usvg`.

---

## 5. User journeys

### A. New user, "I need a poster"

1. Open KCreate. Home screen, "Flyer / Poster / Brochure" tile.
2. Pick a size (A4 portrait). Lands in Design Studio with one named
   artboard.
3. Apply a brand kit (built-in starter, or "Extract from PDF").
4. Drag a photo onto the canvas. Right-click → "Remove background".
   Local AI runs; preview shows; apply.
5. Add a text frame with brand typography. Auto-fit on resize.
6. Export Center → PDF 300 dpi with bleed.

### B. Vector enthusiast, "I want to draw a logo"

1. Home → "Logo / Icon / Brand Kit". Vector Studio opens.
2. Sketch shapes; use boolean ops to merge.
3. Outline strokes; clean up nodes with "Simplify".
4. Use Brand Hub to save the result as a brand logo asset.
5. Export Center → SVG (clean), PNG @2x, PNG favicon set.

### C. Photographer, "Clean up this product photo"

1. Home → "Product Photo Cleanup". Image Studio opens with the photo.
2. AI "Remove background" → preview, edit mask, apply.
3. Adjustment layer (curves) for color correction.
4. Healing brush to remove dust spots.
5. Export Center → PNG 1500×1500, transparent background.

### D. Consultant, "Build a proposal deck"

1. Home → "Pitch Deck / Proposal". Layout Studio opens with a 16:9
   master.
2. Drop content into pre-laid sections (cover, problem, solution,
   pricing, team).
3. AI assistant fills speaker notes from a brief.
4. PDF export, preflight clean.

### E. Developer, "Export icon assets"

1. Home → "Developer Asset Export". Vector Studio opens with a minimal
   grid.
2. Drop in 24×24 SVGs or draw new icons.
3. Define an export preset: SVG sprite, iOS PDF, Android XML, PNG
   @1x/@2x/@3x.
4. Batch export to a chosen folder.

---

## 6. UX model

### 6.1 Global layout

```
┌──────────────────────────────────────────────────────────────────────┐
│ Top Bar: project name · mode switcher · undo/redo · AI · export · ⚙ │
├────────────┬──────────────────────────────────────────┬─────────────┤
│ Left Panel │ Canvas (CanvasHost)                       │ Right Panel │
│ Pages      │                                          │ Properties  │
│ Layers     │                                          │ Effects     │
│ Assets     │                                          │ AI Assist   │
│ Templates  │                                          │ Export      │
└────────────┴──────────────────────────────────────────┴─────────────┘
```

### 6.2 Mode switcher

Tabs in the top bar: **Design · Vector · Image · Layout · Prototype ·
Inspect · Export**. Each tab swaps the right panel's default page and the
active tool set, but the *canvas* and *layer tree* never change. The same
artboard can be edited in any mode without an export-import dance.

### 6.3 Right panel

A tabbed panel:
- **Properties** — geometry, style, transform.
- **Effects** — shadows, blur, glow, opacity stacks.
- **AI Assist** — the Ask → Preview → Apply → Edit → Undo surface for the
  current selection.
- **Export** — per-selection export presets.
- **Inspect** — measure, design tokens, CSS readout.
- **History** — operation log, including AI actions, with filter +
  jump-to.

### 6.4 AI interaction pattern

Every AI action follows the same five-step loop:

1. **Ask** — the user types or picks a preset prompt.
2. **Preview** — the model produces a preview overlay; the original layer
   is untouched.
3. **Apply** — the user accepts; a new operation is appended.
4. **Edit** — the preview is fully editable like any other layer.
5. **Undo** — Ctrl+Z reverts the operation atomically.

No AI action ever modifies a layer in place without a preview.

### 6.5 KChat design tokens

KCreate's UI uses the KChat token set:

- Primary accent: `#7C3AED`
- Font: `Inter` with system fallback
- Background: `#FFFFFF` page, `#F5F3FF` card surfaces
- Cards: white background, `border-radius: 12px`, subtle shadow
- Buttons: pill shape (`border-radius: 9999px`)
- Headings: `#111827`
- Body: `#4B5563`

---

## 15. Import / export compatibility

| Format          | Import | Export | Notes                                              |
| --------------- | :----: | :----: | -------------------------------------------------- |
| SVG             |   ✅   |   ✅   | Round-trip via `usvg`; clean output.               |
| PNG             |   ✅   |   ✅   | Multi-density export.                              |
| JPEG / WebP     |   ✅   |   ✅   | Quality slider, EXIF preserved on import.          |
| PDF (single)    |   ✅   |   ✅   | Export print-ready; import recovers page geometry, embedded images, and extracted text. |
| PDF (multi)     |   —    |   ✅   | Multi-page export via Layout Studio (TOC, outline/bookmarks, hyperlinks). |
| PSD             |   ✅   |   —    | Adobe PSD layered raster import.                   |
| AI (Illustrator)|   ✅   |   —    | Post-CS `.ai` (PDF-container) SVG payload import.  |
| Figma JSON      |   ⚠️   |   ⚠️   | Best-effort; components mapped to KCreate.         |
| Sketch JSON     |   ⚠️   |   ⚠️   | Best-effort.                                       |
| Penpot          |   ⚠️   |   ⚠️   | Best-effort.                                       |
| `.kstudio/`     |   ✅   |   ✅   | Native folder package.                             |
| `.kbrand`       |   ✅   |   ✅   | Brand kit archive (tokens + fonts + logos).        |

Legend: ✅ first-class · ⚠️ best-effort · — not applicable.

---

## 20. Performance targets

### Local-first

| Capability                  | Target                                         |
| --------------------------- | ---------------------------------------------- |
| Editing requires network    | Never.                                         |
| Saving requires network     | Never.                                         |
| AI actions require network  | Never (local model packs).                     |
| Telemetry                   | None by default; explicit opt-in only.         |

### Performance

| Scenario                                       | Tier 0 (4 GB) | Tier 1 (8 GB) | Tier 2 (16 GB) | Tier 3 (32 GB+) |
| ---------------------------------------------- | ------------- | ------------- | -------------- | --------------- |
| Cold start                                     | < 3 s         | < 2 s         | < 1.5 s        | < 1 s           |
| Open a 50 MB project                           | < 5 s         | < 3 s         | < 2 s          | < 1 s           |
| Pan / zoom 5 000-node artboard                 | 30 fps        | 60 fps        | 60 fps         | 120 fps         |
| Pan / zoom 10 000-node artboard                | —             | 30 fps        | 60 fps         | 60 fps          |
| Background removal (4 MP)                      | 15 s          | 8 s           | 4 s            | < 2 s           |
| Open a 64 MP raster                            | 4 s           | 2 s           | < 1 s          | < 1 s           |
| Gaussian blur, 64 MP, radius 20 (GPU)          | —             | —             | < 500 ms       | < 250 ms        |
| Prototype transition (dissolve / slide / push) | 30 fps        | 60 fps        | 60 fps         | 120 fps         |

### UX

| Property                                  | Target                                  |
| ----------------------------------------- | --------------------------------------- |
| Time to first drawable artboard (Tier 1)  | < 3 s                                   |
| Number of clicks from launcher to canvas  | ≤ 2                                     |
| Mode switch                               | Instant; no asset reload.               |
| Right-panel responsiveness                | < 16 ms to selection.                   |

---

## 21. Key risks and mitigations

| Risk                                                              | Severity | Mitigation                                                                                 |
| ----------------------------------------------------------------- | -------- | ------------------------------------------------------------------------------------------ |
| GPU coverage across Linux distros and Windows                     | High     | wgpu backend matrix + `tiny-skia` software fallback. CI tests the CPU path on every platform. |
| Local LLM inference quality on small models                       | Med      | Tiered model packs; structured outputs via GBNF grammars; the user can replace models.      |
| PDF round-trip fidelity                                           | High     | `usvg` + `printpdf`/`lopdf` + manual preflight; export stays deterministic per template.    |
| Multi-platform Electron + native addon distribution               | Med      | Single Rust workspace with a pre-built cdylib per platform via the CI matrix.               |
| Disk-space cost of project history                                | Med      | Content-addressed blob store with BLAKE3 dedup; configurable history depth.                 |
| User loses access by losing the encryption key                    | High     | Recovery flow: per-project optional unencrypted export; clear UI about key consequences.    |
| Plugin sandbox escape                                             | High     | WASM-first plugin runtime; signed native opt-in only; renderer/main process separation.     |
