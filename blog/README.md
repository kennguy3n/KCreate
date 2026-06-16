# KCreate — Building an On-Device Design Studio

A ten-part series on how **KCreate** is built: a local-first design
suite that runs entirely on your machine — templates, an asset
library, generative AI, brand systems, multi-size resizing, and
print- and developer-ready export — with **no account, no cloud, and
no network in the editing path**.

These posts are written for designers, product people, and engineers
who want to understand both *what KCreate does for the job at hand*
and *how it is built end-to-end* — from the launcher down to the Rust
rendering pipeline. Throughout, KCreate is measured against the tools
people reach for today: **Canva**, **Gamma**, and **Figma**.

Every post stands on its own and links to the source files it
discusses, plus a real, rendered design produced by the app itself —
so you can trace any claim back to running code and pixels on disk.

---

## Table of contents

| # | Title | What it covers |
|---|-------|----------------|
| 01 | [Why a local-first design suite?](./part-01-why-local-first.md) | The on-device thesis, the JTBD lens, and where Canva / Gamma / Figma leave value on the table |
| 02 | [From blank canvas to finished design](./part-02-blank-to-finished.md) | The launcher that asks what you're making, the ⌘K command palette, and a first-run that gets you to pixels fast |
| 03 | [A library that jump-starts the work](./part-03-library-jump-start.md) | 122 ready-made templates + 417 searchable, recolorable elements, plus import & remix of your own files |
| 04 | [Generate a whole design from a sentence](./part-04-generate-from-a-sentence.md) | Brief → themed deck / one-pager / social set / web page / document, fully offline, with an undoable "refine with AI" loop |
| 05 | [Brand it once, apply it everywhere](./part-05-brand-it-once.md) | Themes + cross-project brand kits: derive from a document or image, apply to a selection or the whole doc |
| 06 | [One design, every size](./part-06-one-design-every-size.md) | Magic resize with content-aware text re-fit, image smart-crop, and one-shot batch export to every format |
| 07 | [Pixel-perfect on every backend](./part-07-pixel-perfect-rendering.md) | The Rust rendering pipeline, GPU/CPU parity, gradient fidelity, and a present path that stays fast at scale |
| 08 | [Intelligence that stays on your device](./part-08-on-device-intelligence.md) | Local model sidecars over loopback, vision actions, and the Ask → Preview → Apply → Edit → Undo contract |
| 09 | [Print-ready and developer-ready output](./part-09-print-and-dev-ready-output.md) | The Export Center, PDF preflight (CMYK / bleed / TIC / DPI), and inspect-mode code-gen (CSS / Tailwind / React) |
| 10 | [Extend and automate without the cloud](./part-10-extend-and-automate.md) | Sandboxed WASM plugins with signed manifests and a loopback MCP server an AI agent can drive |

---

## How to read this series

- **Designers and product people** can read straight through —
  Parts 02–06 follow the actual job of making a design, and Part 09
  covers getting it out the door.
- **Engineers** will get the most out of Parts 07, 08, and 10, which
  cover the rendering pipeline, the on-device AI architecture, and the
  plugin / automation surface. Part 01 sets the constraints the whole
  codebase is built around.
- **Anyone evaluating against Canva / Gamma / Figma** will find a
  direct contrast at the end of each post under *"How this compares."*

Every post links to the source under [`crates/`](../crates/) and
[`apps/desktop/`](../apps/desktop/). The product principles all of
this serves are summarized in the top-level [`README.md`](../README.md).
