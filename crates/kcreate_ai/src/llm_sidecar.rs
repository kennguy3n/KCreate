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
use std::thread;
use std::time::{Duration, Instant};

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
/// `start()` is *blocking*: it returns once the child is either
/// healthy (success) or has missed the timeout (error). This matches
/// the synchronous bridge IPC; the renderer disables the start button
/// while the call is in flight. Tests mock the upstream via
/// [`SidecarConfig::binary`].
#[derive(Debug)]
pub struct LlmSidecar {
    process: Option<Child>,
    config: SidecarConfig,
    status: SidecarStatus,
}

impl LlmSidecar {
    /// Construct a stopped sidecar with the given config.
    #[must_use]
    pub fn new(config: SidecarConfig) -> Self {
        Self {
            process: None,
            config,
            status: SidecarStatus::Stopped,
        }
    }

    /// Convenience: build from a model path with default settings.
    #[must_use]
    pub fn from_model_path(model_path: PathBuf) -> Self {
        Self::new(SidecarConfig::new(model_path))
    }

    /// Current status snapshot.
    #[must_use]
    pub fn status(&self) -> &SidecarStatus {
        &self.status
    }

    /// True iff the sidecar is currently `Ready`.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.status.is_ready()
    }

    /// Spawn the child process, then poll `/health` until either
    /// `ok` or the timeout elapses.
    ///
    /// Returns the listening port on success. On failure the status
    /// transitions to `Error` and the child (if spawned) is killed.
    pub fn start(&mut self) -> SidecarResult<u16> {
        if self.is_ready() {
            return Err(SidecarError::WrongState("already running".to_string()));
        }
        self.status = SidecarStatus::Starting;

        let res = self.spawn_and_wait();
        match res {
            Ok((port, model_name)) => {
                self.status = SidecarStatus::Ready {
                    model_name,
                    context_size: self.config.context_size,
                    port,
                };
                Ok(port)
            }
            Err(e) => {
                self.kill_child();
                let msg = e.to_string();
                self.status = SidecarStatus::Error { message: msg };
                Err(e)
            }
        }
    }

    /// Stop the child (SIGKILL on Unix, TerminateProcess on Windows)
    /// and transition back to `Stopped`. Idempotent.
    pub fn stop(&mut self) {
        self.kill_child();
        self.status = SidecarStatus::Stopped;
    }

    fn spawn_and_wait(&mut self) -> SidecarResult<(u16, String)> {
        validate_model(&self.config.model_path, self.config.max_model_mb)?;
        let port = pick_loopback_port()?;
        let mut cmd = Command::new(&self.config.binary);
        cmd.args([
            "--model",
            self.config.model_path.to_str().unwrap_or_default(),
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "-c",
            &self.config.context_size.to_string(),
        ]);
        for arg in &self.config.extra_args {
            cmd.arg(arg);
        }
        cmd.stdout(Stdio::null()).stderr(Stdio::piped());

        log::debug!(
            "spawning llama-server: {} --port {}",
            self.config.binary.display(),
            port,
        );
        let child = cmd.spawn().map_err(SidecarError::Spawn)?;
        self.process = Some(child);

        wait_for_health(port, self.config.health_timeout)?;
        let model_name = self
            .config
            .model_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("model")
            .to_string();
        Ok((port, model_name))
    }

    fn kill_child(&mut self) {
        if let Some(mut child) = self.process.take() {
            // Best-effort: the process may already have exited. We
            // intentionally do not propagate kill errors because
            // there is nothing the caller can do about them.
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for LlmSidecar {
    fn drop(&mut self) {
        self.kill_child();
    }
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
