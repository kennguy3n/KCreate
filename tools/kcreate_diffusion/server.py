#!/usr/bin/env python3
"""Local-only diffusion server.

Wire shape (mirrors `crates/kcreate_ai/src/image_gen.rs`):
  POST /v1/images/generations
  Request body (JSON):
    { "prompt": "...", "width": 1024, "height": 1024,
      "steps": 20, "seed": null }
  Response body (JSON):
    { "image": "<base64-png>", "width": 1024, "height": 1024 }

  GET /health → 200 "ok" as soon as the FastAPI app is up. This
    is intentionally a liveness probe, not a readiness probe — it
    only proves that the Python process started and the HTTP loop
    is accepting connections.
  GET /ready → 200 "ready" once the diffusion pipeline finishes
    loading (or 503 if loading is still in flight, 500 if it
    failed). The Rust supervisor blocks on `/ready` — not `/health`
    — before transitioning the sidecar to its `Ready` state, so
    callers never see a generate-request hang for 30-60 s on the
    first call while torch loads the model.

Why this exists:
  - The Rust bridge (`ImageGenSidecar`) spawns this as a child
    process bound to 127.0.0.1 on a random port. Diffusers /
    PyTorch / FLUX are huge, optional dependencies the user
    installs explicitly when they want Tier-2+ image generation;
    keeping them in a sidecar means the rest of the editor never
    pays the import cost on launch.
  - The endpoint matches the OpenAI Images API shape so we don't
    accidentally invent a fourth wire format inside our own crate
    family (text-LLM uses OpenAI chat; vision uses OpenAI vision;
    image gen uses OpenAI images).

Safety:
  - Bind defaults to 127.0.0.1. Refuses to bind to a non-loopback
    address (a stray `--host 0.0.0.0` would silently expose user
    GPUs on a hotel Wi-Fi).
  - No file uploads or arbitrary disk I/O; the only mutable state
    is the loaded pipeline.
"""

from __future__ import annotations

import argparse
import base64
import io
import ipaddress
import logging
import sys
import threading
from typing import Optional

logger = logging.getLogger("kcreate_diffusion.server")


def _build_app(model_path: str):
    """Build the FastAPI app. Imports are deferred so the module
    can be imported (e.g. for `--help` or unit tests) on a machine
    without `torch` / `diffusers` installed.
    """
    try:
        from fastapi import FastAPI, HTTPException
        from fastapi.responses import PlainTextResponse
        from pydantic import BaseModel, Field
    except ImportError as exc:  # pragma: no cover — guarded import
        raise SystemExit(
            "kcreate_diffusion.server needs FastAPI installed. "
            "Run `pip install -r tools/kcreate_diffusion/requirements.txt`."
        ) from exc

    try:
        import torch  # type: ignore
        from diffusers import DiffusionPipeline  # type: ignore
    except ImportError as exc:  # pragma: no cover
        raise SystemExit(
            "kcreate_diffusion.server needs PyTorch + diffusers installed. "
            "Run `pip install -r tools/kcreate_diffusion/requirements.txt`."
        ) from exc

    app = FastAPI(title="kcreate-diffusion", docs_url=None, redoc_url=None)

    # Pipeline loading is moved off the request hot path entirely:
    #
    #   * /health = "is the HTTP loop up?" — answered before the
    #     pipeline finishes loading, so the Rust supervisor's
    #     `wait_for_port` check returns quickly.
    #   * /ready  = "is the pipeline loaded?" — the supervisor uses
    #     this to flip the sidecar's `SidecarStatus` to `Ready`. UI
    #     polls the same status, so the user sees an honest
    #     "loading…" indicator instead of a stuck-spinner 30-60 s
    #     hang on the first generate call.
    #
    # A daemon thread kicks off the load on startup so the pipeline
    # is warm by the time the user clicks Generate, but if the
    # FastAPI app shuts down before loading completes we don't keep
    # the GPU pinned.
    state: dict[str, object] = {
        "pipeline": None,
        "device": "cpu",
        "load_error": None,
    }
    load_lock = threading.Lock()
    load_done = threading.Event()

    def _device() -> str:
        if torch.cuda.is_available():
            return "cuda"
        # MPS (Apple Silicon) works for many pipelines.
        if hasattr(torch.backends, "mps") and torch.backends.mps.is_available():
            return "mps"
        return "cpu"

    def _load_pipeline_now():
        """Load the pipeline once and cache it. Idempotent —
        callable from both the background warmup thread and the
        request path (in case warmup failed and the user clicks
        Generate anyway).
        """
        with load_lock:
            if state["pipeline"] is not None:
                return state["pipeline"]
            device = _device()
            logger.info(
                "Loading diffusion pipeline from %s on %s", model_path, device
            )
            try:
                # `from_single_file` works for community GGUF /
                # safetensors weights without a HuggingFace hub
                # roundtrip. Fall back to `from_pretrained` for
                # repo-layout weights.
                try:
                    pipe = DiffusionPipeline.from_single_file(model_path)
                except Exception:  # noqa: BLE001 — diffusers raises a forest of errors
                    pipe = DiffusionPipeline.from_pretrained(model_path)
                pipe = pipe.to(device)
            except Exception as exc:  # noqa: BLE001 — surface to /ready
                logger.exception("pipeline load failed")
                state["load_error"] = str(exc)
                load_done.set()
                raise
            state["pipeline"] = pipe
            state["device"] = device
            state["load_error"] = None
            load_done.set()
            return pipe

    def _ensure_pipeline():
        if state["pipeline"] is not None:
            return state["pipeline"]
        return _load_pipeline_now()

    @app.on_event("startup")
    def _warm_pipeline() -> None:
        # Daemon so a Ctrl-C / kill from the Rust supervisor doesn't
        # wait for a stuck HuggingFace download.
        def runner() -> None:
            try:
                _load_pipeline_now()
            except Exception:  # noqa: BLE001 — already logged
                pass

        threading.Thread(
            target=runner, name="kcreate-diffusion-warmup", daemon=True
        ).start()

    class GenerateRequest(BaseModel):
        prompt: str = Field(..., min_length=1, max_length=4096)
        width: int = Field(1024, ge=64, le=2048)
        height: int = Field(1024, ge=64, le=2048)
        steps: int = Field(20, ge=1, le=200)
        seed: Optional[int] = Field(None, ge=0)

    class GenerateResponse(BaseModel):
        image: str
        width: int
        height: int

    @app.get("/health", response_class=PlainTextResponse)
    def health() -> str:  # noqa: D401
        # Liveness probe — only proves the FastAPI app is up.
        return "ok"

    @app.get("/ready")
    def ready():  # noqa: D401
        # Readiness probe — proves the diffusion pipeline finished
        # loading. The Rust supervisor blocks on this before
        # declaring `SidecarStatus::Ready`.
        if state["pipeline"] is not None:
            return PlainTextResponse("ready", status_code=200)
        if state["load_error"] is not None:
            # 500 makes the supervisor surface the real error
            # string back to the user instead of silently retrying.
            return PlainTextResponse(
                f"load_error: {state['load_error']}", status_code=500
            )
        # 503 = "try again". The supervisor retries on a poll
        # cadence; once the warmup thread finishes, this flips to
        # 200.
        return PlainTextResponse("loading", status_code=503)

    @app.post("/v1/images/generations", response_model=GenerateResponse)
    def generate(req: GenerateRequest) -> GenerateResponse:  # noqa: D401
        try:
            pipe = _ensure_pipeline()
        except Exception as exc:
            logger.exception("pipeline load failed")
            raise HTTPException(status_code=500, detail=f"pipeline load: {exc}") from exc

        generator = None
        if req.seed is not None:
            generator = torch.Generator(device=str(state["device"])).manual_seed(req.seed)

        result = pipe(
            prompt=req.prompt,
            width=req.width,
            height=req.height,
            num_inference_steps=req.steps,
            generator=generator,
        )
        # `diffusers` returns a dict-like with `.images: List[PIL.Image]`.
        if not hasattr(result, "images") or not result.images:
            raise HTTPException(status_code=500, detail="pipeline produced no images")
        img = result.images[0]
        buf = io.BytesIO()
        img.save(buf, format="PNG", optimize=False)
        b64 = base64.b64encode(buf.getvalue()).decode("ascii")
        return GenerateResponse(image=b64, width=img.width, height=img.height)

    return app


def _validate_host(host: str) -> str:
    """Refuse to bind to a non-loopback address.

    The Rust side spawns us with `--host 127.0.0.1` by default;
    this guard exists so a user who edits a stray launch script to
    add `--host 0.0.0.0` doesn't accidentally publish their GPU to
    the local network.
    """
    if host in {"localhost", "127.0.0.1", "::1"}:
        return host
    try:
        addr = ipaddress.ip_address(host)
    except ValueError as exc:
        raise SystemExit(f"invalid --host {host!r}") from exc
    if not addr.is_loopback:
        raise SystemExit(
            f"refusing to bind diffusion server to non-loopback address {host!r}"
        )
    return host


def main(argv: Optional[list[str]] = None) -> int:
    parser = argparse.ArgumentParser(prog="python3 -m kcreate_diffusion.server")
    parser.add_argument("--model", required=True, help="Path to model weights")
    parser.add_argument("--host", default="127.0.0.1", help="Bind address (must be loopback)")
    parser.add_argument("--port", type=int, default=0, help="Port (0 = ephemeral)")
    parser.add_argument(
        "--log-level",
        default="info",
        choices=["debug", "info", "warning", "error"],
    )
    args = parser.parse_args(argv)

    logging.basicConfig(
        level=getattr(logging, args.log_level.upper()),
        format="%(asctime)s [%(name)s] %(levelname)s %(message)s",
    )
    host = _validate_host(args.host)

    try:
        import uvicorn  # type: ignore
    except ImportError as exc:  # pragma: no cover
        raise SystemExit(
            "kcreate_diffusion.server needs uvicorn. "
            "Run `pip install -r tools/kcreate_diffusion/requirements.txt`."
        ) from exc

    app = _build_app(args.model)
    # `port=0` means "let the kernel pick". uvicorn unfortunately
    # doesn't print the chosen port itself, so we wire a tiny
    # post-startup hook that emits "PORT <n>\n" on stdout — the
    # Rust sidecar reads stdout for that line to discover the
    # actual listening port.
    config = uvicorn.Config(
        app,
        host=host,
        port=args.port,
        log_level=args.log_level,
        access_log=False,
    )
    server = uvicorn.Server(config)

    # Patch server to print the chosen port as soon as it's known.
    original_startup = server.startup

    async def startup_hook(sockets=None):  # type: ignore[override]
        await original_startup(sockets=sockets)
        # The first server in `server.servers` exposes the bound
        # socket; we read its sockname to learn the port.
        for s in server.servers:
            for sock in s.sockets:
                port = sock.getsockname()[1]
                print(f"PORT {port}", flush=True)
                return

    server.startup = startup_hook  # type: ignore[assignment]
    server.run()
    return 0


if __name__ == "__main__":  # pragma: no cover
    sys.exit(main())
