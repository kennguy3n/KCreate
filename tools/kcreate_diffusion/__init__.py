"""kcreate_diffusion — thin FastAPI wrapper around 🤗 diffusers.

This package is invoked from Rust by `ImageGenSidecar` as
``python3 -m kcreate_diffusion.server --model <path> --port <port>``.
It exposes a single OpenAI-compatible loopback endpoint —
``POST /v1/images/generations`` — that returns a base64-encoded PNG.

Heavy lifting is done by `diffusers`; this package is intentionally
small (≈250 lines) so it stays auditable. No GPU/CUDA assumptions
are made here — the underlying diffusers pipeline picks the right
device automatically. When CUDA / MPS isn't available the user just
gets slower inference, not a crash.
"""

__all__ = ["server"]
