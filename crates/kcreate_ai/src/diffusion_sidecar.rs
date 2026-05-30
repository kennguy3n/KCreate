//! Diffusion sidecar driver: `sd-server` (stable-diffusion.cpp).
//!
//! Phase 12 Block B replaced the original `tools/kcreate_diffusion/`
//! Python FastAPI process — which pulled in PyTorch + diffusers
//! (~5 GB of pip deps) — with `sd-server`, the HTTP server binary
//! shipped by [`leejet/stable-diffusion.cpp`]. sd-server speaks
//! three API surfaces on loopback (default `127.0.0.1:1234`,
//! overridable):
//!
//! * `GET  /sdcpp/v1/capabilities` — native readiness probe. Only
//!   returns 200 after the diffusion model finishes loading.
//! * `POST /sdapi/v1/txt2img`     — Automatic1111-compatible
//!   text-to-image endpoint. Request body
//!   `{ "prompt": "...", "width": W, "height": H, "steps": N,
//!     "seed": optional }`. Response body
//!   `{ "images": ["<base64 PNG>"], ... }`.
//! * `POST /v1/images/generations` — OpenAI Images-compatible
//!   endpoint. We don't use it here, but the bridge could later for
//!   parity with cloud LLM clients.
//!
//! The lifecycle mirrors [`crate::llm_sidecar::LlmSidecar`] — a
//! supervisor thread polls the readiness probe until the deadline,
//! flips the shared `SidecarStatus` to `Ready { port, ... }`, then
//! blocks until a stop signal is set. Kill-on-drop is enforced.
//! Loopback only: the listener binds `127.0.0.1`, never `0.0.0.0`,
//! and we hand a dynamically-allocated port to sd-server so the
//! editing-path local-first invariant cannot leak a globally
//! reachable port by accident.
//!
//! No Python interpreter is involved at any point. The binary
//! distributed for sd-server is ~50 MB and statically links the
//! ggml compute graph; the build flags Metal / CUDA / Vulkan /
//! OpenCL backends as required by the host.
//!
//! [`leejet/stable-diffusion.cpp`]: https://github.com/leejet/stable-diffusion.cpp

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use crate::llm_sidecar::{SidecarError, SidecarResult, SidecarStatus};

/// Default health-check ceiling. FLUX-class diffusion models can
/// take a while to memory-map and prepare on first load — sd-server
/// streams progress logs to stderr but does not bind the HTTP
/// listener until `init_runtime` completes. 90 s mirrors the
/// original Python sidecar's budget so the renderer's existing
/// timeout copy stays accurate.
const DEFAULT_HEALTH_TIMEOUT: Duration = Duration::from_secs(90);
const HEALTH_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Tunables for the sd-server lifecycle. Mirrors
/// [`crate::llm_sidecar::SidecarConfig`] in shape (path-to-binary,
/// path-to-weights, deadlines, extra args) but with sd-server's
/// flag set baked in.
///
/// The supplementary FLUX components (CLIP-L, T5-XXL, VAE, LLM text
/// encoder) ride on `extra_args`. The diffusion sidecar driver is
/// architecture-agnostic on purpose — Phase 12 keeps the encoder
/// routing in the bridge so that future Z-Image / Qwen-Image packs
/// can compose their own argv without modifying this module.
#[derive(Debug, Clone)]
pub struct DiffusionSidecarConfig {
    /// Path to the `sd-server` executable from
    /// stable-diffusion.cpp.
    pub binary: PathBuf,
    /// Primary diffusion weights — forwarded as `--diffusion-model`
    /// when the file lives outside a fused SD checkpoint.
    pub model_path: PathBuf,
    /// Maximum time the supervisor will wait for `/sdcpp/v1/capabilities`
    /// to return 200 before flipping the status to `Error`.
    pub health_timeout: Duration,
    /// Forwarded verbatim. Used by the bridge to pass
    /// `--clip_l ...`, `--t5xxl ...`, `--vae ...`, `--llm ...`,
    /// `--diffusion-fa`, `--offload-to-cpu`, etc.
    pub extra_args: Vec<String>,
}

impl DiffusionSidecarConfig {
    /// Construct a config from a model path with default knobs.
    /// Callers append `extra_args` for FLUX-style multi-component
    /// loads.
    #[must_use]
    pub fn new(binary: PathBuf, model_path: PathBuf) -> Self {
        Self {
            binary,
            model_path,
            health_timeout: DEFAULT_HEALTH_TIMEOUT,
            extra_args: Vec::new(),
        }
    }
}

/// Build the argv that gets passed to `sd-server`. Public for tests
/// and debug logging — the supervisor formats this exact slice into
/// a `Command::new(binary).args(argv(..))` invocation.
///
/// We always emit `--listen-ip 127.0.0.1` so the listener cannot
/// surface on a non-loopback interface even if a future
/// configuration error leaks an external address through
/// `extra_args`. `--listen-port` carries the random port the
/// supervisor allocated up front.
#[must_use]
pub fn build_argv(config: &DiffusionSidecarConfig, port: u16) -> Vec<String> {
    let mut argv = vec![
        "--listen-ip".to_string(),
        "127.0.0.1".to_string(),
        "--listen-port".to_string(),
        port.to_string(),
        "--diffusion-model".to_string(),
        config.model_path.to_string_lossy().into_owned(),
    ];
    for arg in &config.extra_args {
        argv.push(arg.clone());
    }
    argv
}

/// Diffusion sidecar driver. Same shape as
/// [`crate::llm_sidecar::LlmSidecar`] so the bridge can treat both
/// runtimes uniformly.
#[derive(Debug)]
pub struct DiffusionSidecar {
    config: DiffusionSidecarConfig,
    status: Arc<Mutex<SidecarStatus>>,
    stop_signal: Option<Arc<AtomicBool>>,
    worker: Option<thread::JoinHandle<()>>,
}

impl DiffusionSidecar {
    /// Construct a stopped sidecar with the given config.
    #[must_use]
    pub fn new(config: DiffusionSidecarConfig) -> Self {
        Self {
            config,
            status: Arc::new(Mutex::new(SidecarStatus::Stopped)),
            stop_signal: None,
            worker: None,
        }
    }

    /// Current status snapshot.
    #[must_use]
    pub fn status(&self) -> SidecarStatus {
        self.status.lock().clone()
    }

    /// True iff Ready.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.status.lock().is_ready()
    }

    /// Spawn the sd-server child and start probing
    /// `/sdcpp/v1/capabilities`. Returns the listening port
    /// immediately; the caller observes `Ready`/`Error` by polling
    /// [`Self::status`].
    pub fn start(&mut self) -> SidecarResult<u16> {
        {
            let s = self.status.lock();
            if s.is_ready() || matches!(*s, SidecarStatus::Starting) {
                return Err(SidecarError::WrongState("already running".to_string()));
            }
        }
        if let Err(e) = validate_model(&self.config.model_path) {
            *self.status.lock() = SidecarStatus::Error {
                message: e.to_string(),
            };
            return Err(e);
        }
        // Note: we don't validate the binary path here — `Command::spawn`
        // is responsible for surfacing a meaningful error when the
        // binary isn't found. That keeps `binary: PathBuf::from("sd-server")`
        // (PATH-resolved) working without forcing callers to compute
        // the absolute path. Mirrors `llm_sidecar.rs`.
        let port = pick_loopback_port()?;
        let child = spawn_child(&self.config, port)?;
        *self.status.lock() = SidecarStatus::Starting;
        let stop_signal = Arc::new(AtomicBool::new(false));
        self.stop_signal = Some(stop_signal.clone());
        let status = Arc::clone(&self.status);
        let model_name = self
            .config
            .model_path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let health_timeout = self.config.health_timeout;
        self.worker = Some(thread::spawn(move || {
            health_worker(child, port, model_name, health_timeout, stop_signal, status);
        }));
        Ok(port)
    }

    /// Stop the sidecar and wait for the worker thread to settle.
    /// Idempotent; safe to call from `Drop`.
    pub fn stop(&mut self) {
        if let Some(s) = &self.stop_signal {
            s.store(true, Ordering::Release);
        }
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
        self.stop_signal = None;
        *self.status.lock() = SidecarStatus::Stopped;
    }
}

impl Drop for DiffusionSidecar {
    fn drop(&mut self) {
        self.stop();
    }
}

fn spawn_child(config: &DiffusionSidecarConfig, port: u16) -> SidecarResult<Child> {
    let mut cmd = Command::new(&config.binary);
    for arg in build_argv(config, port) {
        cmd.arg(arg);
    }
    cmd.stdout(Stdio::null()).stderr(Stdio::piped());
    log::debug!(
        "spawning sd-server: {} --listen-port {} --diffusion-model {}",
        config.binary.display(),
        port,
        config.model_path.display(),
    );
    cmd.spawn().map_err(SidecarError::Spawn)
}

fn health_worker(
    mut child: Child,
    port: u16,
    model_name: String,
    health_timeout: Duration,
    stop_signal: Arc<AtomicBool>,
    status: Arc<Mutex<SidecarStatus>>,
) {
    let deadline = Instant::now() + health_timeout;
    let mut ready = false;
    while Instant::now() < deadline {
        if stop_signal.load(Ordering::Acquire) {
            kill_child(&mut child);
            *status.lock() = SidecarStatus::Stopped;
            return;
        }
        if probe_ready(port) {
            ready = true;
            break;
        }
        thread::sleep(HEALTH_POLL_INTERVAL);
    }
    if !ready {
        kill_child(&mut child);
        *status.lock() = SidecarStatus::Error {
            message: SidecarError::HealthTimeout {
                timeout: health_timeout,
            }
            .to_string(),
        };
        return;
    }
    *status.lock() = SidecarStatus::Ready {
        model_name,
        // Diffusion context isn't a token budget; report 0 to keep
        // the wire shape uniform with the chat sidecar.
        context_size: 0,
        port,
        // sd-server does not implement an `--api-key`-style bearer
        // mechanism. Loopback-only binding is the security
        // boundary; the renderer never embeds a token in its
        // generate requests. Phase 11 Block D added auth ONLY for
        // the chat sidecar (llama-server) where the token-based
        // protection is well-tested.
        bearer_token: None,
    };
    while !stop_signal.load(Ordering::Acquire) {
        thread::sleep(Duration::from_millis(200));
    }
    kill_child(&mut child);
    *status.lock() = SidecarStatus::Stopped;
}

fn kill_child(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn pick_loopback_port() -> SidecarResult<u16> {
    use std::net::{SocketAddr, TcpListener};
    let addr: SocketAddr = "127.0.0.1:0".parse().expect("static addr");
    let listener = TcpListener::bind(addr).map_err(SidecarError::PortAllocation)?;
    let port = listener
        .local_addr()
        .map_err(SidecarError::PortAllocation)?
        .port();
    drop(listener);
    Ok(port)
}

/// Diffusion model files are either GGUF (`.gguf`) or safetensors
/// (`.safetensors`) — both are accepted by sd-server. We accept any
/// non-empty path that exists; the actual format validation happens
/// inside sd-server. An empty path or a path that doesn't exist
/// surfaces as a hard error before we spawn the child.
fn validate_model(model: &Path) -> SidecarResult<()> {
    if model.as_os_str().is_empty() {
        return Err(SidecarError::ModelMissing(model.to_path_buf()));
    }
    if !model.exists() {
        return Err(SidecarError::ModelMissing(model.to_path_buf()));
    }
    Ok(())
}

/// Probe the sd-server `/sdcpp/v1/capabilities` endpoint. Returns
/// `true` only when sd-server has finished loading the diffusion
/// model AND the HTTP listener is bound. This is the equivalent of
/// the old Python sidecar's `/ready` route.
///
/// When `llm_sidecar` isn't compiled in we don't have `ureq`
/// available, so we fall back to a TCP-connect probe — that's only
/// used by unit tests that mock out the supervisor anyway.
#[cfg(feature = "llm_sidecar")]
fn probe_ready(port: u16) -> bool {
    let url = format!("http://127.0.0.1:{port}/sdcpp/v1/capabilities");
    match ureq::get(&url).timeout(Duration::from_secs(2)).call() {
        Ok(resp) => resp.status() == 200,
        Err(_) => false,
    }
}

#[cfg(not(feature = "llm_sidecar"))]
fn probe_ready(port: u16) -> bool {
    use std::net::{SocketAddr, TcpStream};
    TcpStream::connect_timeout(
        &SocketAddr::from(([127, 0, 0, 1], port)),
        Duration::from_millis(500),
    )
    .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `build_argv` must emit `--listen-ip 127.0.0.1`,
    /// `--listen-port <port>`, and `--diffusion-model <model>` in
    /// that exact order, followed by any extra args verbatim. The
    /// order matters: the supervisor allocates the port before
    /// spawning, so the port flag must immediately precede the
    /// model so a casual `grep` of the process list shows which
    /// model is bound to which port.
    #[test]
    fn build_argv_emits_loopback_and_model_first() {
        let cfg = DiffusionSidecarConfig {
            binary: PathBuf::from("/usr/local/bin/sd-server"),
            model_path: PathBuf::from("/models/flux.gguf"),
            health_timeout: DEFAULT_HEALTH_TIMEOUT,
            extra_args: vec![
                "--clip_l".to_string(),
                "/models/clip_l.safetensors".to_string(),
            ],
        };
        let argv = build_argv(&cfg, 38123);
        assert_eq!(
            argv,
            vec![
                "--listen-ip",
                "127.0.0.1",
                "--listen-port",
                "38123",
                "--diffusion-model",
                "/models/flux.gguf",
                "--clip_l",
                "/models/clip_l.safetensors",
            ],
        );
    }

    /// `build_argv` with no extra args must still produce a working
    /// invocation (the listen flags + the model path).
    #[test]
    fn build_argv_without_extra_args_is_minimal() {
        let cfg = DiffusionSidecarConfig::new(
            PathBuf::from("/usr/local/bin/sd-server"),
            PathBuf::from("/models/flux.gguf"),
        );
        let argv = build_argv(&cfg, 47000);
        assert_eq!(
            argv,
            vec![
                "--listen-ip",
                "127.0.0.1",
                "--listen-port",
                "47000",
                "--diffusion-model",
                "/models/flux.gguf",
            ],
        );
    }

    /// Non-existent binary surfaces as `Spawn` once the supervisor
    /// tries to launch it. We rely on `Command::spawn` to produce
    /// the right error rather than pre-validating the path, so
    /// PATH-resolved names like `"sd-server"` still work for users
    /// who put the binary on `$PATH`.
    #[test]
    fn start_with_nonexistent_binary_errors() {
        let dir = tempfile::tempdir().unwrap();
        let model = dir.path().join("flux.gguf");
        std::fs::write(&model, b"fake").unwrap();
        let mut sidecar = DiffusionSidecar::new(DiffusionSidecarConfig::new(
            PathBuf::from("/no/such/sd-server/binary"),
            model,
        ));
        let err = sidecar.start().unwrap_err();
        assert!(
            matches!(err, SidecarError::Spawn(_)),
            "expected Spawn error, got {err:?}",
        );
    }

    /// Missing model path must error with `ModelMissing` rather
    /// than spawning sd-server and watching it crash.
    #[test]
    fn start_with_missing_model_errors() {
        let mut sidecar = DiffusionSidecar::new(DiffusionSidecarConfig::new(
            PathBuf::from("/usr/local/bin/sd-server"),
            PathBuf::from("/no/such/model.gguf"),
        ));
        let err = sidecar.start().unwrap_err();
        assert!(
            matches!(err, SidecarError::ModelMissing(_)),
            "expected ModelMissing error, got {err:?}",
        );
    }

    /// Empty model path must error with `ModelMissing` — a vague
    /// "spawn failed 30 s later" is the wrong UX.
    #[test]
    fn start_with_empty_model_path_errors() {
        let mut sidecar = DiffusionSidecar::new(DiffusionSidecarConfig::new(
            PathBuf::from("sd-server"),
            PathBuf::new(),
        ));
        let err = sidecar.start().unwrap_err();
        assert!(
            matches!(err, SidecarError::ModelMissing(_)),
            "expected ModelMissing error, got {err:?}",
        );
    }

    /// `DiffusionSidecarConfig::new` defaults must be sensible —
    /// in particular the health timeout has to cover a cold FLUX
    /// load.
    #[test]
    fn config_defaults_have_cold_load_budget() {
        let cfg = DiffusionSidecarConfig::new(
            PathBuf::from("sd-server"),
            PathBuf::from("flux.gguf"),
        );
        assert!(cfg.health_timeout >= Duration::from_secs(30));
        assert!(cfg.extra_args.is_empty());
    }

    /// New sidecars must start in the `Stopped` state — the
    /// renderer relies on this initial value when it polls
    /// `image_gen_status` before any `start` call.
    #[test]
    fn new_sidecar_is_stopped() {
        let cfg = DiffusionSidecarConfig::new(
            PathBuf::from("sd-server"),
            PathBuf::from("flux.gguf"),
        );
        let sidecar = DiffusionSidecar::new(cfg);
        assert!(matches!(sidecar.status(), SidecarStatus::Stopped));
        assert!(!sidecar.is_ready());
    }
}
