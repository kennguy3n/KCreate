//! Bridge-side singleton accessor for the renderer's
//! [`kcreate_renderer::compute::GpuComputeContext`].
//!
//! The bridge's raster operations (`raster_ops::apply_blur`,
//! `apply_sharpen`, `apply_levels`, `apply_curves`) consult this
//! module before falling back to the CPU implementations in
//! `kcreate_raster::filters`.
//!
//! The context is initialised lazily on first use. If wgpu cannot
//! find an adapter (typical for CI runners and the
//! `cpu-only` feature), the context is `None` and every call
//! returns `None` so callers stay on the CPU path.
//!
//! Initialisation is wrapped in `catch_unwind` because creating a
//! wgpu device on an exotic platform (e.g. a CI VM with a broken
//! Vulkan loader, an Apple Silicon runner without Metal) can panic
//! inside the wgpu native layer. A panic here must never tear down
//! the bridge — the user still wants the filter to apply, just on
//! the CPU.

use std::sync::OnceLock;

use kcreate_renderer::compute::GpuComputeContext;

/// Holds the global compute context (or `None` if no GPU is
/// available). We use a `OnceLock<Option<…>>` rather than
/// `OnceLock<…>` so the "no GPU" verdict is itself memoised — we
/// only attempt initialisation once per process.
static CONTEXT: OnceLock<Option<GpuComputeContext>> = OnceLock::new();

/// Return a reference to the process-wide GPU compute context,
/// initialising it on first use. Returns `None` when wgpu cannot
/// find an adapter or initialisation panicked.
pub fn try_context() -> Option<&'static GpuComputeContext> {
    CONTEXT
        .get_or_init(|| {
            // The first call into wgpu can panic on broken systems.
            // `catch_unwind` keeps the bridge usable.
            let result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(GpuComputeContext::try_new));
            match result {
                Ok(Ok(ctx)) => ctx,
                Ok(Err(err)) => {
                    log_init_failure(&format!("device init failed: {err}"));
                    None
                }
                Err(_panic) => {
                    log_init_failure("wgpu adapter request panicked");
                    None
                }
            }
        })
        .as_ref()
}

fn log_init_failure(msg: &str) {
    // Plain `eprintln!` — the bridge has no shared logger to
    // depend on and the message is one-shot per process.
    eprintln!("kcreate_bridge::gpu_compute: GPU disabled ({msg}); falling back to CPU filters");
}

/// Force the GPU path off, used by tests that want to validate the
/// CPU fallback path even on a machine that has a working GPU.
/// No-op (and `Err`-returns) when the context has already been
/// initialised earlier in the process; tests that need a clean
/// disable should call this before the first filter dispatch.
#[cfg(test)]
pub fn force_disable_for_test() {
    let _ = CONTEXT.set(None);
}
