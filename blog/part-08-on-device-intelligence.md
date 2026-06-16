# 08 — Intelligence that stays on your device

"AI design tool" almost always means "we upload your work to our
servers and send back a result." KCreate's intelligence is built the
other way around: the models run on **your** hardware, the editor talks
to them over **loopback**, and nothing the AI does can leave you stuck
with a change you can't take back. Privacy and reversibility aren't
add-ons here — they're the architecture.

## Local model sidecars over loopback

KCreate's generative and language features are served by **local model
sidecars** — separate processes the app launches and talks to over
`127.0.0.1`. Language and chat go through a llama.cpp-style sidecar
([`crates/kcreate_ai/src/llm_sidecar.rs`](../crates/kcreate_ai/src/llm_sidecar.rs)
and [`llm_chat.rs`](../crates/kcreate_ai/src/llm_chat.rs)); image
generation goes through an image sidecar; classical vision tasks run as
in-process native code. A model registry tracks what's installed and
ready.

The key architectural decision: **the networking those sidecars use is
isolated from the editing path.** The model-serving code sits behind
feature flags, in crates the editor's dependency tree never pulls in.
That's why KCreate can talk to a local server *and* still pass the
local-first invariant test
([`crates/kcreate_tests/tests/local_first.rs`](../crates/kcreate_tests/tests/local_first.rs))
that fails the build if a networking library appears in the editing
closure. Loopback to your own machine is not "the cloud."

## Vision actions that run in-process

Not everything needs a large model. A whole class of useful
"intelligent" actions are classical algorithms that run instantly and
offline, with no model download at all:

- **Background removal** — threshold-based, or an ONNX u2net model when
  installed.
- **Upscaling** — high-quality Lanczos3 resampling
  ([`crates/kcreate_ai/src/upscale.rs`](../crates/kcreate_ai/src/upscale.rs)).
- **Palette extraction** — k-means over the image
  ([`crates/kcreate_ai/src/palette.rs`](../crates/kcreate_ai/src/palette.rs)),
  the same engine that derives a brand kit from an image in Part 05.
- **Smart select** — BFS flood-fill to grab a contiguous region
  ([`crates/kcreate_ai/src/smart_select.rs`](../crates/kcreate_ai/src/smart_select.rs)).
- **Screenshot-to-layout** — edge detection + connected components +
  heuristics to turn a screenshot into editable structure.

These make "intelligent" feel *fast*, because they are — there's no
round-trip and no model load for the common cases.

## The contract: Ask → Preview → Apply → Edit → Undo

The most important thing about AI in KCreate isn't any single feature —
it's the **contract** every AI action obeys:

1. **Ask** — you request a change in plain language or a click.
2. **Preview** — KCreate shows you what it would do.
3. **Apply** — the change lands as real, editable layers (never a
   flattened result you can't touch).
4. **Edit** — you adjust it like anything else on the canvas.
5. **Undo** — one `Ctrl`/`Cmd`+`Z` reverts it completely.

The "refine with AI" loop from Part 04 is the clearest example:
regenerate-and-reapply is wrapped as a **single undoable operation**,
so even a sweeping AI change is one keystroke away from gone. The AI is
an **assistant**, not an autopilot — it never does anything irreversible,
and it never produces output you can't open up and edit by hand.

Every AI action is also written to an append-only audit log
([`crates/kcreate_audit/`](../crates/kcreate_audit/)) stored separately
from the project, so you can always see what the assistant did and when.

## Why this serves the job — and your privacy

For the job, on-device intelligence means the assistant is always
available and always fast — on a plane, offline, behind a firewall. For
privacy, it means your drafts, your client's confidential deck, your
unreleased product UI **never leave your machine to be "improved."**
That's a guarantee a cloud tool structurally cannot make.

## How this compares

- **Canva**'s Magic Studio and **Gamma**'s generation are cloud-based:
  your content is uploaded, and the features stop without a connection.
  KCreate runs the equivalent capabilities locally.
- **Figma**'s AI features are likewise cloud-bound. KCreate keeps the
  whole loop — generate, refine, vision actions — on-device and behind
  a strict undo contract.
- The **isolation of model networking from the editing path**, enforced
  by a build-failing test, is something you won't find in a cloud tool
  because it isn't a constraint they're trying to meet.

---

**Trace it in the code**

- LLM sidecar + loopback chat: [`crates/kcreate_ai/src/llm_sidecar.rs`](../crates/kcreate_ai/src/llm_sidecar.rs), [`crates/kcreate_ai/src/llm_chat.rs`](../crates/kcreate_ai/src/llm_chat.rs)
- Vision actions: [`crates/kcreate_ai/src/upscale.rs`](../crates/kcreate_ai/src/upscale.rs), [`palette.rs`](../crates/kcreate_ai/src/palette.rs), [`smart_select.rs`](../crates/kcreate_ai/src/smart_select.rs)
- Local-first invariant: [`crates/kcreate_tests/tests/local_first.rs`](../crates/kcreate_tests/tests/local_first.rs)
- AI audit trail: [`crates/kcreate_audit/`](../crates/kcreate_audit/)

Previous: [« 07 — Pixel-perfect on every backend](./part-07-pixel-perfect-rendering.md) ·
Next: [09 — Print-ready and developer-ready output »](./part-09-print-and-dev-ready-output.md)
