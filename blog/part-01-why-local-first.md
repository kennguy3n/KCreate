# 01 — Why a local-first design suite?

Most design tools today live in a browser tab. That is a real
convenience, and it is also a set of quiet costs: your files live on
someone else's servers, your work stops when your connection does,
"AI" means uploading your content to a datacenter, and the tool that
feels instant on a fast connection feels broken on a plane.

KCreate starts from the opposite premise. It is a **desktop
application that runs entirely on your machine** — your designs are
folders on your disk, the renderer is native code, and the AI runs on
your own hardware. There is no account to create and nothing to log
in to. You open it and you are working.

![KCreate's local-first landing design, authored in the app](./assets/web-hero-light.png)

*A landing design built in KCreate — "a local-first design studio that
runs on your machine: templates, AI, and export, fully offline."
Everything you see below is rendered by the app's own pipeline.*

## The job to be done

The framing that runs through this whole series is the **job to be
done** (JTBD): people don't want a canvas, they want a finished
poster, a pitch deck, a set of social posts, a UI screen, a
print-ready flyer. A good tool is judged by how directly it moves
someone from *"I need this thing"* to *"here is the thing,"* with as
little ceremony in between as possible.

That lens drives every product decision in KCreate:

- The first screen asks **what you're making**, not which menu you'd
  like to browse.
- A **library** of real, editable templates and elements means you
  start from something good instead of an empty rectangle.
- **Generative AI** can produce an entire first draft from one
  sentence — then hand you fully editable layers, not a flattened
  image.
- **Brand kits** and **magic resize** collapse the repetitive parts of
  the job: make it on-brand once, produce every size in one step.
- **Export** speaks the formats the next person in the chain needs —
  PNG/SVG/PDF for production, CMYK with bleed for print, and
  CSS/Tailwind/React for developers.

## Why "on-device" is a feature, not a limitation

Running locally is usually framed as a constraint. In KCreate it is
the source of several advantages competitors structurally can't match:

- **It's always fast and always available.** The render pipeline is
  native Rust talking to your GPU; there's no round-trip to a server
  for a redraw. It works on a plane, in a SCIF, on hotel wifi, or with
  the network physically off.
- **Your work is yours.** Projects are open `.kstudio` folders on your
  disk — content-addressed blobs plus a small SQLite index — not rows
  in a vendor's database. Open formats (SVG, PNG, PDF, WebP) round-trip
  in and out.
- **AI without the upload.** Generation, background removal, upscaling,
  palette extraction, and chat all run against models on your machine
  over loopback. Your draft never leaves the device to be "improved."
- **It scales to your hardware, not a pricing tier.** The same app
  tunes itself from a 4 GB laptop to a 32 GB workstation.

The local-first rule is not a slogan — it is **enforced in CI**. A
dependency-graph test walks the entire editing-path crate closure and
fails the build if any networking library sneaks in
([`crates/kcreate_tests/tests/local_first.rs`](../crates/kcreate_tests/tests/local_first.rs)).
Networked features (a future LAN session, the optional chat backend)
are quarantined behind feature flags in crates the editor never
depends on, so the guarantee holds by construction.

## How this compares

- **Canva** is excellent at jump-starting work from a huge library,
  but it is cloud-only: your assets and AI prompts live on its
  servers, and it stops at the network's edge. KCreate brings the
  library-first experience on-device.
- **Gamma** turns a prompt into a polished deck or page, but the
  generation happens in the cloud and the output is comparatively
  hard to take into a precise editor. KCreate generates locally and
  hands you real, editable layers.
- **Figma** is the gold standard for precise vector editing and
  developer hand-off, but it is fundamentally a collaborative cloud
  document and its on-device story is a cache, not a product. KCreate
  treats the local machine as the primary home for the work.

KCreate's bet is that you can have the library-first ease of Canva,
the generative speed of Gamma, and the editing precision and
developer hand-off of Figma — **without** giving up ownership,
offline capability, or privacy. The rest of this series shows how.

---

**Trace it in the code**

- Product principles and module map: [`README.md`](../README.md)
- The local-first invariant test: [`crates/kcreate_tests/tests/local_first.rs`](../crates/kcreate_tests/tests/local_first.rs)
- Project storage (content-addressed blobs + SQLite): [`crates/kcreate_storage/`](../crates/kcreate_storage/)

Next: [02 — From blank canvas to finished design »](./part-02-blank-to-finished.md)
