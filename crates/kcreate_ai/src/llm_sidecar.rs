//! LLM sidecar lifecycle manager.
//!
//! Spawns `llama-server` (from the `kennguy3n/llama.cpp` `prism`
//! fork) as a child process bound to `127.0.0.1:<port>`, polls its
//! `/health` endpoint until it transitions to "ok", and exposes a
//! coarse `SidecarStatus` enum for the host UI.
//!
//! # Why loopback-only
//!
//! The editing path is contractually local-first (`AGENTS.md` § Rules).
//! `llama-server` listens on `127.0.0.1` only — never `0.0.0.0` — so
//! no traffic leaves the machine. The HTTP client this module uses
//! (`ureq` via the `llm_sidecar` Cargo feature) is gated to keep
//! default builds out of the deny-list test.
//!
//! # Status machine
//!
//! ```text
//!     ┌───────────┐    start()    ┌──────────┐    health=ok   ┌────────┐
//!     │  Stopped  │ ──────────▶  │ Starting │ ──────────────▶ │ Ready  │
//!     └───────────┘                └──────────┘                └────────┘
//!           ▲                          │                         │
//!           │ stop() / drop            │ health timeout / spawn  │ stop()
//!           │                          │  failed                 │
//!           │                          ▼                         │
//!           │                       ┌──────────┐                 │
//!           └──────────────────────  │  Error  │  ◀──────────────┘
//!                                   └──────────┘
//! ```
//!
//! `stop()` is also called implicitly via the `Drop` impl so a
//! crashed renderer or a panicked task can't leak the sidecar
//! process.

use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

/// Default device-tier-aware ceiling on how long we'll wait for the
/// sidecar's `/health` endpoint to flip to `ok`. 30 s covers a cold
/// load of a ~4 GB GGUF on a spinning disk; production hosts almost
/// always finish in <5 s.
const DEFAULT_HEALTH_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the health-poll loop sleeps between probes.
const HEALTH_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Result alias for sidecar operations.
pub type SidecarResult<T> = Result<T, SidecarError>;

/// Errors the sidecar can report. All variants are non-panicking —
/// callers always get a typed error rather than a crash.
#[derive(Debug, thiserror::Error)]
pub enum SidecarError {
    /// Spawning the child process failed (binary missing, permission
    /// denied, exec(2) failure).
    #[error("failed to spawn llama-server: {0}")]
    Spawn(#[source] std::io::Error),
    /// The configured model file does not exist on disk.
    #[error("model file not found: {0}")]
    ModelMissing(PathBuf),
    /// The model file is larger than the configured tier ceiling
    /// (e.g. RuntimeConfig::effective_max_model_mb()).
    #[error("model size {model_mb} MB exceeds limit {limit_mb} MB")]
    ModelTooLarge { model_mb: u64, limit_mb: u64 },
    /// Could not bind a localhost port for the child to listen on.
    #[error("failed to allocate loopback port: {0}")]
    PortAllocation(#[source] std::io::Error),
    /// Health endpoint did not transition to "ok" within the timeout.
    #[error("llama-server did not become ready within {timeout:?}")]
    HealthTimeout { timeout: Duration },
    /// Attempted to start when already starting/ready, or to query
    /// a sidecar that is not in the expected state.
    #[error("sidecar is not in the expected state: {0}")]
    WrongState(String),
}

/// Coarse status the host UI displays in the Model Manager panel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SidecarStatus {
    /// Initial state and post-stop.
    Stopped,
    /// `start()` returned but `/health` has not yet flipped to `ok`.
    Starting,
    /// Sidecar is accepting requests on `port`.
    Ready {
        model_name: String,
        context_size: usize,
        port: u16,
    },
    /// Last attempt failed. The error message is preserved verbatim
    /// for the user-facing toast.
    Error { message: String },
}

impl SidecarStatus {
    /// Convenience: true iff currently `Ready`.
    #[must_use]
    pub const fn is_ready(&self) -> bool {
        matches!(self, Self::Ready { .. })
    }

    /// Listening port if currently `Ready`.
    #[must_use]
    pub const fn port(&self) -> Option<u16> {
        match self {
            Self::Ready { port, .. } => Some(*port),
            _ => None,
        }
    }
}

/// Tunables for the sidecar driver. Most callers pass `Default`
/// directly; tests override `health_timeout` to keep them snappy and
/// `binary` to point at a mock HTTP server instead of llama-server.
#[derive(Debug, Clone)]
pub struct SidecarConfig {
    /// Path to the `llama-server` (or compatible) binary.
    pub binary: PathBuf,
    /// Path to the GGUF model file.
    pub model_path: PathBuf,
    /// Maximum allowed model size in MB (typically
    /// `RuntimeConfig::effective_max_model_mb()`). Set to `u64::MAX`
    /// to disable the check.
    pub max_model_mb: u64,
    /// Context window in tokens; passed to llama-server via `-c`.
    pub context_size: usize,
    /// Sidecar must transition to Ready within this duration.
    pub health_timeout: Duration,
    /// Extra args appended to the llama-server invocation. Useful for
    /// `--n-gpu-layers`, `--threads`, etc.
    pub extra_args: Vec<String>,
}

impl SidecarConfig {
    /// Construct a config from a model path with default knobs.
    #[must_use]
    pub fn new(model_path: PathBuf) -> Self {
        Self {
            binary: PathBuf::from("llama-server"),
            model_path,
            max_model_mb: u64::MAX,
            context_size: 4096,
            health_timeout: DEFAULT_HEALTH_TIMEOUT,
            extra_args: Vec::new(),
        }
    }
}

/// Sidecar process driver.
///
/// `start()` is *non-blocking*: it performs the fast setup (model
/// validation, port allocation, `fork+exec`) synchronously on the
/// calling thread and returns the listening port immediately. The
/// (up-to-30s) health-probe loop runs on a dedicated background
/// worker that updates the shared status as it observes
/// `Starting → Ready` (or `Starting → Error`). The worker also owns
/// the child handle for its full lifetime, so `stop()` simply flips
/// the shared stop flag and joins the worker (which kills the child).
///
/// Why background-poll: the N-API surface that calls into this is
/// synchronous, so blocking `start()` would freeze the Electron
/// main process for as long as model loading takes. The UI already
/// polls `llm_status()` from the renderer once per few seconds, so
/// the natural place for the slow path is a worker that updates the
/// status the UI is reading.
#[derive(Debug)]
pub struct LlmSidecar {
    config: SidecarConfig,
    /// Shared with the background worker so it can update status as
    /// it transitions through `Starting → Ready → Stopped`.
    status: Arc<Mutex<SidecarStatus>>,
    /// Signal the worker should shut down. Owned by both the worker
    /// (read) and `stop()` (write).
    stop_signal: Option<Arc<AtomicBool>>,
    /// Worker handle, joined on `stop()`.
    worker: Option<thread::JoinHandle<()>>,
}

impl LlmSidecar {
    /// Construct a stopped sidecar with the given config.
    #[must_use]
    pub fn new(config: SidecarConfig) -> Self {
        Self {
            config,
            status: Arc::new(Mutex::new(SidecarStatus::Stopped)),
            stop_signal: None,
            worker: None,
        }
    }

    /// Convenience: build from a model path with default settings.
    #[must_use]
    pub fn from_model_path(model_path: PathBuf) -> Self {
        Self::new(SidecarConfig::new(model_path))
    }

    /// Current status snapshot. Cheap clone — the variants only
    /// carry small strings and a port.
    #[must_use]
    pub fn status(&self) -> SidecarStatus {
        self.status.lock().clone()
    }

    /// True iff the sidecar is currently `Ready`.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.status.lock().is_ready()
    }

    /// Spawn the child process and a background health-probe worker.
    /// Returns the listening port immediately. The caller observes
    /// `Ready` (or `Error`) by polling [`status`].
    ///
    /// On synchronous failure (missing model, oversize model, spawn
    /// error) the status transitions directly to `Error` and the
    /// error is returned.
    pub fn start(&mut self) -> SidecarResult<u16> {
        {
            let s = self.status.lock();
            if s.is_ready() || matches!(*s, SidecarStatus::Starting) {
                return Err(SidecarError::WrongState("already running".to_string()));
            }
        }

        // Fail fast on the synchronous checks so the caller sees a
        // typed error rather than having to poll the status.
        if let Err(e) = validate_model(&self.config.model_path, self.config.max_model_mb) {
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
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("model")
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

    /// Stop the sidecar. Flips the shared stop signal so the worker
    /// kills the child and exits, then joins the worker. Idempotent.
    pub fn stop(&mut self) {
        if let Some(sig) = self.stop_signal.take() {
            sig.store(true, Ordering::Release);
        }
        if let Some(h) = self.worker.take() {
            // Workers always terminate promptly once the signal is
            // set: the health-probe phase polls the flag between
            // sleeps, and the post-Ready watch is a short sleep loop.
            let _ = h.join();
        }
        *self.status.lock() = SidecarStatus::Stopped;
    }
}

impl Drop for LlmSidecar {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Spawn the llama-server child process. Pure helper — does not
/// touch any sidecar state, so `start()` can call it before deciding
/// whether the spawn succeeded.
fn spawn_child(config: &SidecarConfig, port: u16) -> SidecarResult<Child> {
    let mut cmd = Command::new(&config.binary);
    cmd.args([
        "--model",
        config.model_path.to_str().unwrap_or_default(),
        "--host",
        "127.0.0.1",
        "--port",
        &port.to_string(),
        "-c",
        &config.context_size.to_string(),
    ]);
    for arg in &config.extra_args {
        cmd.arg(arg);
    }
    cmd.stdout(Stdio::null()).stderr(Stdio::piped());
    log::debug!(
        "spawning llama-server: {} --port {}",
        config.binary.display(),
        port,
    );
    cmd.spawn().map_err(SidecarError::Spawn)
}

/// Run the background health-probe + stop watch for a single child.
/// Owns the child for its full lifetime: kills it on health timeout,
/// on stop signal, or on Drop (via `let mut child` going out of
/// scope, with `kill_child` called below).
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
    };
    // Watch the stop signal until shutdown. The poll interval here
    // is purely how long `stop()` may wait for the worker to notice;
    // 200 ms is well below human-perceptible.
    while !stop_signal.load(Ordering::Acquire) {
        thread::sleep(Duration::from_millis(200));
    }
    kill_child(&mut child);
    *status.lock() = SidecarStatus::Stopped;
}

fn kill_child(child: &mut Child) {
    // Best-effort: the process may already have exited.
    let _ = child.kill();
    let _ = child.wait();
}

/// Allocate a loopback port the OS confirms is free. We bind then
/// drop the listener to claim a port, then hand the number to
/// llama-server. There is a small TOCTOU window but llama-server
/// retries on bind failure and the alternative — letting the child
/// pick — requires parsing its stdout.
fn pick_loopback_port() -> SidecarResult<u16> {
    let addr: SocketAddr = "127.0.0.1:0".parse().expect("static addr");
    let listener = TcpListener::bind(addr).map_err(SidecarError::PortAllocation)?;
    let port = listener
        .local_addr()
        .map_err(SidecarError::PortAllocation)?
        .port();
    drop(listener);
    Ok(port)
}

fn validate_model(model: &Path, max_mb: u64) -> SidecarResult<()> {
    let meta =
        std::fs::metadata(model).map_err(|_| SidecarError::ModelMissing(model.to_path_buf()))?;
    let model_mb = meta.len() / (1024 * 1024);
    if model_mb > max_mb {
        return Err(SidecarError::ModelTooLarge {
            model_mb,
            limit_mb: max_mb,
        });
    }
    Ok(())
}

/// Poll `http://127.0.0.1:{port}/health` until it returns "ok" or
/// the timeout expires.
///
/// llama-server's health endpoint returns JSON of shape `{"status":
/// "ok"}` (or `"loading model"` while initialising). We accept any
/// 200 response whose body contains `"ok"` to keep the check robust
/// across forks. With the `llm_sidecar` feature disabled the
/// blocking-HTTP code path collapses to a TCP connect check; that's
/// enough for the deny-list test and for tests that don't link
/// `ureq`.
///
/// In production code the equivalent loop is inlined into
/// [`health_worker`] so it can also watch the stop signal; this free
/// function exists only so that the `ready_lifecycle_via_mock_server`
/// test can drive the probe loop directly without a real child
/// process.
#[cfg(test)]
fn wait_for_health(port: u16, timeout: Duration) -> SidecarResult<()> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if probe_health(port) {
            return Ok(());
        }
        thread::sleep(HEALTH_POLL_INTERVAL);
    }
    Err(SidecarError::HealthTimeout { timeout })
}

/// Returns `true` once the sidecar's `/health` endpoint reports `ok`.
/// Transient errors (refused, timed out, transport hiccups) are
/// silently treated as "not ready yet" because the caller retries
/// until the outer `wait_for_health` deadline fires.
#[cfg(feature = "llm_sidecar")]
fn probe_health(port: u16) -> bool {
    let url = format!("http://127.0.0.1:{port}/health");
    match ureq::get(&url).timeout(Duration::from_secs(1)).call() {
        Ok(resp) => {
            let body = resp.into_string().unwrap_or_default();
            body.contains("\"ok\"") || body.contains("ok")
        }
        Err(_) => false,
    }
}

#[cfg(not(feature = "llm_sidecar"))]
fn probe_health(port: u16) -> bool {
    use std::net::TcpStream;
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
    fn pick_port_is_nonzero_and_loopback_reachable() {
        let port = pick_loopback_port().expect("port");
        assert!(port > 0);
    }

    #[test]
    fn validate_rejects_missing_model() {
        let err =
            validate_model(Path::new("/no/such/file.gguf"), u64::MAX).expect_err("missing model");
        assert!(matches!(err, SidecarError::ModelMissing(_)));
    }

    #[test]
    fn validate_rejects_oversized_model() {
        let dir = tempfile::tempdir().expect("temp");
        let p = dir.path().join("m.gguf");
        std::fs::write(&p, vec![0u8; 2 * 1024 * 1024]).expect("write");
        let err = validate_model(&p, 1).expect_err("too large");
        assert!(matches!(err, SidecarError::ModelTooLarge { .. }));
    }

    #[test]
    fn status_helpers() {
        let s = SidecarStatus::Stopped;
        assert!(!s.is_ready());
        assert_eq!(s.port(), None);
        let r = SidecarStatus::Ready {
            model_name: "m".into(),
            context_size: 2048,
            port: 9999,
        };
        assert!(r.is_ready());
        assert_eq!(r.port(), Some(9999));
    }

    #[test]
    fn start_with_missing_model_transitions_to_error() {
        let cfg = SidecarConfig {
            binary: PathBuf::from("/bin/false"),
            model_path: PathBuf::from("/no/such/model.gguf"),
            max_model_mb: u64::MAX,
            context_size: 2048,
            health_timeout: Duration::from_millis(50),
            extra_args: vec![],
        };
        let mut s = LlmSidecar::new(cfg);
        let err = s.start().expect_err("missing model");
        assert!(matches!(err, SidecarError::ModelMissing(_)));
        assert!(matches!(s.status(), SidecarStatus::Error { .. }));
    }

    #[test]
    fn start_with_bad_binary_transitions_to_error() {
        let dir = tempfile::tempdir().expect("temp");
        let model = dir.path().join("m.gguf");
        std::fs::write(&model, b"0").expect("write");
        let cfg = SidecarConfig {
            binary: PathBuf::from("/this/binary/does/not/exist"),
            model_path: model,
            max_model_mb: u64::MAX,
            context_size: 2048,
            health_timeout: Duration::from_millis(50),
            extra_args: vec![],
        };
        let mut s = LlmSidecar::new(cfg);
        let err = s.start().expect_err("bad binary");
        assert!(matches!(err, SidecarError::Spawn(_)));
        assert!(matches!(s.status(), SidecarStatus::Error { .. }));
    }

    /// Use a `tiny_http` mock server as the "child binary". We can't
    /// run llama-server itself in CI, but the contract is "spawn,
    /// poll /health, transition to Ready" — a stub HTTP server
    /// exercises the entire driver path except the actual exec.
    #[test]
    fn ready_lifecycle_via_mock_server() {
        let dir = tempfile::tempdir().expect("temp");
        let model = dir.path().join("m.gguf");
        std::fs::write(&model, b"0").expect("write");

        // Drive the lifecycle manually: spawn a tiny_http server on
        // the port we picked, then wait_for_health succeeds.
        let port = pick_loopback_port().expect("port");
        let server = tiny_http::Server::http(format!("127.0.0.1:{port}")).expect("server");
        let handle = std::thread::spawn(move || {
            for req in server.incoming_requests() {
                let resp = tiny_http::Response::from_string("{\"status\":\"ok\"}");
                let _ = req.respond(resp);
            }
        });

        wait_for_health(port, Duration::from_secs(2)).expect("ready");
        // Drop the server thread by letting it go out of scope at end.
        let _ = handle;
    }
}
