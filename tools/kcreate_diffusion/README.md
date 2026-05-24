# kcreate_diffusion

Tiny FastAPI wrapper around 🤗 `diffusers`, invoked from Rust as
`python3 -m kcreate_diffusion.server --model <path> --port <port>`.
Serves a single OpenAI-compatible endpoint —
`POST /v1/images/generations` — that returns a base64-encoded PNG.

Loopback only by default; refuses to bind to a non-loopback host.

## Install

```bash
python3 -m venv .venv
. .venv/bin/activate
pip install -r tools/kcreate_diffusion/requirements.txt
```

Tier 2+ devices with a GPU (NVIDIA CUDA, Apple MPS) get the full
pipeline. On CPU-only hosts it falls back gracefully — generation
just takes minutes instead of seconds.

## Run

The Rust side does this automatically when the image-gen sidecar
is started. For manual testing:

```bash
python3 -m kcreate_diffusion.server \
    --model /path/to/flux-2-klein-4b.gguf \
    --host 127.0.0.1 --port 8800
```

## Wire shape

Request:

```json
{
  "prompt": "Mountain at dawn",
  "width": 1024, "height": 1024,
  "steps": 20,
  "seed": null
}
```

Response:

```json
{
  "image": "<base64-encoded-PNG>",
  "width": 1024, "height": 1024
}
```
