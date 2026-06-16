# 09 — Print-ready and developer-ready output

A design isn't done when it looks right on screen — it's done when it
reaches the next person in the chain in the form *they* need. For a
printer, that's a CMYK PDF with bleed and a checked ink budget. For a
developer, that's precise CSS, Tailwind, or React. KCreate's Export
Center is built to hand off cleanly to both, without a separate tool.

## One Export Center, every format

KCreate exports **PNG, SVG, PDF, WebP, and JPEG** from the same Rust
pipeline that draws the canvas, so what you export is exactly what you
see (Part 07). Raster exports supersample at high DPI rather than
padding, so a 2× PNG is genuinely twice the detail. The export crate
([`crates/kcreate_export/`](../crates/kcreate_export/)) also drives the
parallel batch export behind magic resize (Part 06).

## Print-ready: preflight before you commit

Print is where on-screen designs quietly go wrong — RGB colors that
can't be printed, no bleed, too much ink, images below usable
resolution. KCreate's **preflight**
([`crates/kcreate_export/src/preflight.rs`](../crates/kcreate_export/src/preflight.rs))
inspects a design before export and reports concrete, actionable
findings:

- **Color mode** — RGB content that needs CMYK conversion, and spot
  colors.
- **Bleed and trim** — content that doesn't extend into the bleed, or
  sits outside the safe/trim area.
- **Ink coverage (TIC)** — total ink over the press limit.
- **Resolution** — raster images below a usable DPI for the print
  size.

You then export a **print-ready PDF**: CMYK output with bleed and trim
marks, page size specified in millimetres. The PDF export path supports
an explicit color mode (RGB or CMYK) so the file you send to the
printer is the file they expect — not an RGB screen export they have to
fix.

![A gradient event poster designed in KCreate](./assets/poster.png)

*An event poster built in KCreate and exported to a CMYK PDF at a real
print size — gradients and all, preflighted before it goes to the
press.*

## Developer-ready: inspect-mode code-gen

For the hand-off to engineering, KCreate's **inspect mode** generates
real code for any layer — the same job Figma's inspect panel does, but
running locally
([`crates/kcreate_export/src/code_gen.rs`](../crates/kcreate_export/src/code_gen.rs)).
Select an element and KCreate emits CSS, Tailwind, and a React inline
style object. Here's the actual output for a call-to-action button from
one of the designs in this series:

**CSS**

```css
position: absolute;
left: 124px;
top: 500px;
width: 244px;
height: 56px;
background-color: #6d5bf8;
```

**Tailwind**

```
w-[244px] h-[56px] absolute left-[124px] top-[500px] bg-[#6d5bf8]
```

**React (inline style)**

```jsx
{
  position: "absolute",
  left: 124,
  top: 500,
  width: 244,
  height: 56,
  backgroundColor: "#6d5bf8",
}
```

That's copy-paste-ready: exact geometry, exact color, in the
developer's choice of three idioms — generated on-device, no account or
plugin required.

## Why this serves the job

The job ends at the hand-off. A design that can't be printed correctly
or can't be implemented faithfully isn't finished. KCreate closes both
loops in the same app: preflight catches print problems *before* they
cost a reprint, and code-gen gives developers exact values instead of
eyeballed approximations — so the thing that ships matches the thing
you designed.

## How this compares

- **Figma**'s Dev Mode is the benchmark for code hand-off and it's
  excellent — but it's cloud-bound and lacks real print preflight.
  KCreate generates the same class of CSS/Tailwind/React locally *and*
  adds CMYK/bleed/TIC/DPI preflight.
- **Canva** and **Gamma** are weak on both precise print preflight and
  developer code hand-off; that's not their job. KCreate treats both as
  first-class.
- **Scribus** does serious print preflight but nothing for developers;
  KCreate brings print and developer hand-off under one roof.

---

**Trace it in the code**

- Export pipeline (PNG/SVG/PDF/WebP/JPEG): [`crates/kcreate_export/`](../crates/kcreate_export/)
- PDF preflight (CMYK / bleed / TIC / DPI): [`crates/kcreate_export/src/preflight.rs`](../crates/kcreate_export/src/preflight.rs)
- Inspect-mode code-gen (CSS / Tailwind / React): [`crates/kcreate_export/src/code_gen.rs`](../crates/kcreate_export/src/code_gen.rs)
- Export + preflight UI: [`apps/desktop/renderer/src/components/`](../apps/desktop/renderer/src/components/)

Previous: [« 08 — Intelligence that stays on your device](./part-08-on-device-intelligence.md) ·
Next: [10 — Extend and automate without the cloud »](./part-10-extend-and-automate.md)
