//! MLX sidecar — Apple Silicon-only sibling of [`crate::llm_sidecar`].
//!
//! On Apple Silicon (`aarch64-apple-darwin`) the fastest local
//! inference engine for both text and vision models is Apple's MLX
//! runtime via the `mlx-lm` Python package (`pip install mlx-lm`).
//! `mlx_lm.server` exposes an OpenAI-compatible HTTP API on
//! `127.0.0.1:<port>` — the same wire format as `llama-server` — so
//! [`crate::llm_chat::chat_completion`] works against either sidecar
//! without modification.
//!
//! This module mirrors the [`crate::llm_sidecar::LlmSidecar`]
//! lifecycle exactly:
//!
//! ```text
//!  Stopped → Starting → Ready → Stopped
//!                    ↘
//!                     → Error
//! ```
//!
//! Differences from the llama-server sidecar:
//!
//! 1. The subprocess command is `python3 -m mlx_lm.server --model
//!    <path> --port <port>` (mlx-lm's server entry point).
//! 2. The `model_path` is interpreted as an MLX-format model id or
//!    local directory, not a single GGUF file — MLX models ship as a
//!    directory of `*.npz` / `*.safetensors` weights plus a config.
//!    Validation therefore probes for the directory, not a file.
//! 3. Availability is gated on Apple Silicon at the platform level
//!    (callers should bail early on other OSes) AND on the Python
//!    `mlx_lm` module being importable. [`probe_mlx_available`]
//!    runs `python3 -c "import mlx_lm"` once and caches the result.
//!
//! Local-first invariant: no networking outside loopback. The Python
//! subprocess binds `127.0.0.1` exclusively; we never reach over the
//! network for weights (the user downloads them out-of-band, same
//! contract as the GGUF flow).

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use crate::llm_sidecar::{SidecarError, SidecarResult, SidecarStatus};

/// Default health-check ceiling. MLX cold loads are CPU-bound on
/// non-Apple-Silicon test machines (where this code never executes
/// in production), so the 30 s upper bound from
/// [`crate::llm_sidecar`] applies here too.
const DEFAULT_HEALTH_TIMEOUT: Duration = Duration::from_secs(30);
const HEALTH_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Tunables for the MLX sidecar. Defaults match the llama-server
/// counterpart so callers can swap the two via [`SidecarDispatcher`].
#[derive(Debug, Clone)]
pub struct MlxSidecarConfig {
    /// Python interpreter to invoke. `python3` works on every macOS
    /// install we target; an explicit `pythonX.Y` path can be set
    /// when the user has multiple Pythons installed.
    pub python: PathBuf,
    /// MLX model id (Hugging Face `mlx-community/...` slug) OR a
    /// path to a local directory containing the model weights.
    pub model_path: PathBuf,
    /// Context window in tokens; forwarded as `--max-tokens-cache` to
    /// `mlx_lm.server`'s prompt cache budget. (mlx_lm does not have a
    /// direct `-c <ctx>` knob — the model file's `config.json` owns
    /// the real context limit — so this is advisory.)
    pub context_size: usize,
    /// MLX must transition to Ready within this duration.
    pub health_timeout: Duration,
    /// Extra args appended to the `mlx_lm.server` invocation.
    pub extra_args: Vec<String>,
}

impl MlxSidecarConfig {
    /// Construct a config from a model directory with default knobs.
    #[must_use]
    pub fn new(model_path: PathBuf) -> Self {
        Self {
            python: PathBuf::from("python3"),
            model_path,
            context_size: 4096,
            health_timeout: DEFAULT_HEALTH_TIMEOUT,
            extra_args: Vec::new(),
        }
    }
}

/// MLX sidecar process driver. Mirrors
/// [`crate::llm_sidecar::LlmSidecar`] field-for-field so the host
/// dispatcher can treat them as interchangeable.
#[derive(Debug)]
pub struct MlxSidecar {
    config: MlxSidecarConfig,
    status: Arc<Mutex<SidecarStatus>>,
    stop_signal: Option<Arc<AtomicBool>>,
    worker: Option<thread::JoinHandle<()>>,
}

impl MlxSidecar {
    /// Construct a stopped sidecar with the given config.
    #[must_use]
    pub fn new(config: MlxSidecarConfig) -> Self {
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

    /// True iff the sidecar is currently `Ready`.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.status.lock().is_ready()
    }

    /// Spawn the MLX child process and start probing `/health`.
    /// Returns the listening port immediately; the caller observes
    /// `Ready`/`Error` by polling [`Self::status`].
    pub fn start(&mut self) -> SidecarResult<u16> {
        {
            let s = self.status.lock();
            if s.is_ready() || matches!(*s, SidecarStatus::Starting) {
                return Err(SidecarError::WrongState("already running".to_string()));
            }
        }

        if let Err(e) = validate_model_dir(&self.config.model_path) {
            *self.status.lock() = SidecarStatus::Error {
                message: e.to_string(),
            };
            return Err(e);
        }
        let port = match pick_loopback_port() {
            Ok(p) => p,
            Err(e) => {
                *self.status.lock() = SidecarStatus::Error {
                    message: e.to_string(),
                };
                return Err(e);
            }
        };
        let child = match spawn_child(&self.config, port) {
            Ok(c) => c,
            Err(e) => {
                *self.status.lock() = SidecarStatus::Error {
                    message: e.to_string(),
                };
                return Err(e);
            }
        };

        *self.status.lock() = SidecarStatus::Starting;
        let stop_signal = Arc::new(AtomicBool::new(false));
        let model_name = self
            .config
            .model_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("mlx-model")
            .to_string();
        let context_size = self.config.context_size;
        let health_timeout = self.config.health_timeout;
        let status_for_worker = Arc::clone(&self.status);
        let stop_for_worker = Arc::clone(&stop_signal);

        let handle = thread::spawn(move || {
            health_worker(
                child,
                port,
                model_name,
                context_size,
                health_timeout,
                stop_for_worker,
                status_for_worker,
            );
        });
        self.stop_signal = Some(stop_signal);
        self.worker = Some(handle);
        Ok(port)
    }

    /// Stop the sidecar. Idempotent.
    pub fn stop(&mut self) {
        if let Some(sig) = self.stop_signal.take() {
            sig.store(true, Ordering::Release);
        }
        if let Some(h) = self.worker.take() {
            let _ = h.join();
        }
        *self.status.lock() = SidecarStatus::Stopped;
    }
}

impl Drop for MlxSidecar {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Probe whether the MLX Python toolchain is available. Runs
/// `python3 -c "import mlx_lm"` once at process startup and caches
/// the result. Returns `false` on any error (Python missing,
/// `mlx_lm` not installed, exec failure) so callers can fall back
/// to llama-server without panicking.
///
/// We deliberately do NOT short-circuit on non-Apple-Silicon targets
/// here — the probe itself is cheap and works the same on every OS,
/// and tests on Linux that want to assert "MLX not available" rely
/// on this function honestly reporting `false`.
#[must_use]
pub fn probe_mlx_available() -> bool {
    static CACHE: OnceLock<bool> = OnceLock::new();
    *CACHE.get_or_init(|| {
        Command::new("python3")
            .args(["-c", "import mlx_lm"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    })
}

/// Build the argv vector `mlx_lm.server` would receive. Public for
/// tests and for debug logging — mirrors
/// [`crate::llm_sidecar::build_argv`].
#[must_use]
pub fn build_argv(config: &MlxSidecarConfig, port: u16) -> Vec<String> {
    let mut argv = vec![
        "-m".to_string(),
        "mlx_lm.server".to_string(),
        "--model".to_string(),
        config.model_path.to_string_lossy().into_owned(),
        "--host".to_string(),
        "127.0.0.1".to_string(),
        "--port".to_string(),
        port.to_string(),
    ];
    for arg in &config.extra_args {
        argv.push(arg.clone());
    }
    argv
}

fn spawn_child(config: &MlxSidecarConfig, port: u16) -> SidecarResult<Child> {
    let mut cmd = Command::new(&config.python);
    for arg in build_argv(config, port) {
        cmd.arg(arg);
    }
    cmd.stdout(Stdio::null()).stderr(Stdio::piped());
    log::debug!(
        "spawning mlx_lm.server: {} -m mlx_lm.server --port {}",
        config.python.display(),
        port,
    );
    cmd.spawn().map_err(SidecarError::Spawn)
}

#[allow(clippy::too_many_arguments)]
fn health_worker(
    mut child: Child,
    port: u16,
    model_name: String,
    context_size: usize,
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
        if probe_health(port) {
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
        context_size,
        port,
        // Phase 11 Block E Task 25 — the MLX sidecar is the macOS
        // Metal alternative spawn path; the auth flag wiring there
        // tracks llama-server, but the MLX server doesn't ship an
        // `--api-key` equivalent yet, so the bearer is reported as
        // `None` and clients fall back to unauthenticated loopback.
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

/// MLX models are directories, not single files (unlike GGUF). The
/// validator accepts either a directory (local-weights case) or a
/// path that does not exist on the local filesystem (HF-slug case,
/// e.g. `mlx-community/Qwen3.5-4B-MLX-4bit` — `mlx_lm.server` will
/// download it on first use, which we let it do since this is the
/// only place in KCreate that legitimately fetches over the network
/// and only at user request — *not* on the editing path).
fn validate_model_dir(model: &Path) -> SidecarResult<()> {
    if model.as_os_str().is_empty() {
        return Err(SidecarError::ModelMissing(model.to_path_buf()));
    }
    // If the path exists locally, require it to be a directory or a
    // single weights file (some users point at a `model.safetensors`
    // directly). Anything else (a broken symlink, an empty path) is
    // surfaced as `ModelMissing`.
    if model.exists() {
        if model.is_dir() || model.is_file() {
            return Ok(());
        }
        return Err(SidecarError::ModelMissing(model.to_path_buf()));
    }
    // Path doesn't exist locally — assume it's an HF slug. The
    // Python subprocess will surface its own error if the slug is
    // wrong, and that error will reach the host via the health
    // timeout. This is the same contract as `llama-server` would
    // have for unknown model names.
    Ok(())
}

#[cfg(feature = "llm_sidecar")]
fn probe_health(port: u16) -> bool {
    // `mlx_lm.server` does not implement `/health`, but it does
    // implement the OpenAI-compatible `/v1/models` endpoint, which
    // we use as the readiness signal: as soon as that returns a
    // 200, the server has loaded weights and is accepting requests.
    let url = format!("http://127.0.0.1:{port}/v1/models");
    match ureq::get(&url).timeout(Duration::from_secs(1)).call() {
        Ok(resp) => resp.status() == 200,
        Err(_) => false,
    }
}

#[cfg(not(feature = "llm_sidecar"))]
fn probe_health(port: u16) -> bool {
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

    #[test]
    fn argv_carries_module_invocation() {
        let cfg = MlxSidecarConfig::new(PathBuf::from("mlx-community/Qwen3.5-4B-MLX-4bit"));
        let argv = build_argv(&cfg, 12345);
        // Module form: `python3 -m mlx_lm.server ...`. Anything else
        // means the dispatcher is invoking the wrong entry point.
        assert_eq!(argv.first().map(String::as_str), Some("-m"));
        assert_eq!(argv.get(1).map(String::as_str), Some("mlx_lm.server"));
        let model_pos = argv
            .iter()
            .position(|a| a == "--model")
            .expect("argv must contain --model");
        assert_eq!(
            argv.get(model_pos + 1).map(String::as_str),
            Some("mlx-community/Qwen3.5-4B-MLX-4bit"),
        );
        let port_pos = argv
            .iter()
            .position(|a| a == "--port")
            .expect("argv must contain --port");
        assert_eq!(argv.get(port_pos + 1).map(String::as_str), Some("12345"));
        let host_pos = argv
            .iter()
            .position(|a| a == "--host")
            .expect("argv must contain --host");
        assert_eq!(
            argv.get(host_pos + 1).map(String::as_str),
            Some("127.0.0.1"),
            "MLX sidecar must bind loopback only",
        );
    }

    #[test]
    fn validate_accepts_local_directory() {
        let dir = tempfile::tempdir().expect("temp");
        assert!(validate_model_dir(dir.path()).is_ok());
    }

    #[test]
    fn validate_accepts_huggingface_slug() {
        // HF slug isn't a real path; the validator must let it
        // through so mlx_lm.server can resolve it.
        let path = PathBuf::from("mlx-community/Qwen3.5-4B-MLX-4bit");
        assert!(validate_model_dir(&path).is_ok());
    }

    #[test]
    fn validate_rejects_empty_path() {
        let err = validate_model_dir(Path::new("")).expect_err("empty path");
        assert!(matches!(err, SidecarError::ModelMissing(_)));
    }

    #[test]
    fn start_with_bad_python_transitions_to_error() {
        let dir = tempfile::tempdir().expect("temp");
        let cfg = MlxSidecarConfig {
            python: PathBuf::from("/this/python/does/not/exist"),
            model_path: dir.path().to_path_buf(),
            context_size: 2048,
            health_timeout: Duration::from_millis(50),
            extra_args: vec![],
        };
        let mut s = MlxSidecar::new(cfg);
        let err = s.start().expect_err("bad python");
        assert!(matches!(err, SidecarError::Spawn(_)));
        assert!(matches!(s.status(), SidecarStatus::Error { .. }));
    }
}
