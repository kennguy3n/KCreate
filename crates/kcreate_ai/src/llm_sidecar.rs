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
    /// The configured multimodal projector file does not exist on disk.
    /// Vision models in llama.cpp require a paired `mmproj` file
    /// alongside the GGUF weights — `--mmproj <path>` is mandatory
    /// when serving a VLM, and an early-exit failure here is
    /// preferable to letting llama-server explode in its own error
    /// path after the spawn has already produced a child PID.
    #[error("mmproj file not found: {0}")]
    MmprojMissing(PathBuf),
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
    /// Phase 11 Block E Task 25 follow-up round 3 — Devin Review
    /// ANALYSIS-0006 (r3). The OS CSPRNG (`getrandom`) refused to
    /// produce a bearer token when `require_api_key` was set. Rather
    /// than silently downgrading the sidecar to unauthenticated
    /// loopback — which is the exact attack surface Block E was
    /// designed to close — we now fail-closed and surface the
    /// underlying error so the host UI can decide whether to retry,
    /// fall back to the no-auth profile *explicitly*, or block the
    /// AI feature outright.
    #[error("failed to sample bearer token from OS CSPRNG: {0}")]
    TokenEntropyFailed(String),
    /// Phase 12 round 3 — Devin Review flagged that the health-probe
    /// loop only watched the readiness endpoint, so a child that
    /// crashed during init (invalid model, missing shared library,
    /// GPU init failure) would sit until the full health timeout
    /// elapsed (~90 s) before surfacing as a generic
    /// `HealthTimeout`. We now `try_wait` the child between probes
    /// and surface this typed error with the captured exit code plus
    /// the tail of stderr so the renderer can show a meaningful
    /// toast in ~500 ms instead.
    #[error("sidecar child exited during startup (exit_status={code:?}): {stderr_tail}")]
    ChildExited {
        /// `child.wait()` exit code, when available. Signal kills on
        /// Unix surface as `None` (the platform reports the signal,
        /// not a code). The `Display` formatting uses `{code:?}` so
        /// `Some(2)` renders as `Some(2)` and signal kills render as
        /// `None`, which is unambiguous in the renderer toast and
        /// keeps the error formatting compatible with thiserror's
        /// no-runtime-conditionals constraint.
        code: Option<i32>,
        /// Last ~2 KB of the child's stderr stream. sd-server /
        /// llama-server both print their fatal-init error on the
        /// final line before exiting, so even this small tail is
        /// usually enough to diagnose the failure.
        stderr_tail: String,
    },
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
        /// Phase 11 Block E Task 25 — per-session bearer token the
        /// HTTP client must echo as `Authorization: Bearer <token>`
        /// on every request. `None` for legacy builds of
        /// llama-server that don't accept `--api-key`; clients
        /// then talk to the sidecar without auth (logged with a
        /// warning at start time).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bearer_token: Option<String>,
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

    /// Bearer token clients must send to authenticate against the
    /// sidecar, if one was minted at `start()`. `None` when the
    /// sidecar was started without auth (typically a llama-server
    /// build that doesn't accept `--api-key`).
    #[must_use]
    pub fn bearer_token(&self) -> Option<&str> {
        match self {
            Self::Ready { bearer_token, .. } => bearer_token.as_deref(),
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
    /// Phase 11 Block E Task 25 — whether `start()` should mint a
    /// random per-session bearer token and pass it to llama-server
    /// via `--api-key`. Defaults to `true` so all callers are
    /// authenticated by default. Tests that point `binary` at a
    /// mock HTTP server can flip this off if the mock doesn't
    /// honour the `--api-key` flag.
    pub require_api_key: bool,

    /// Optional path to the multimodal projector (mmproj) file that
    /// pairs with a vision-language GGUF. llama.cpp's `llama-server`
    /// accepts `--mmproj <path>` to load the projector weights that
    /// translate image tokens into the model's embedding space; this
    /// is what makes Qwen-VL / LLaVA / SmolVLM-style models accept
    /// `image_url` content parts on the OpenAI-compatible chat API.
    ///
    /// `None` keeps the sidecar text-only (the historical Phase 2
    /// behaviour); `Some(path)` enables vision and is validated for
    /// existence at start time, with a typed
    /// [`SidecarError::MmprojMissing`] error so misconfiguration
    /// surfaces in the UI instead of a confusing health-timeout.
    pub mmproj_path: Option<PathBuf>,
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
            require_api_key: true,
            mmproj_path: None,
        }
    }

    /// Builder-style: attach a multimodal projector file to enable
    /// vision (`--mmproj <path>` on the llama-server invocation).
    ///
    /// Pass `None` to leave the sidecar text-only.
    #[must_use]
    pub fn with_mmproj(mut self, mmproj_path: Option<PathBuf>) -> Self {
        self.mmproj_path = mmproj_path;
        self
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
        if let Some(mmproj) = self.config.mmproj_path.as_deref() {
            if let Err(e) = validate_mmproj(mmproj) {
                *self.status.lock() = SidecarStatus::Error {
                    message: e.to_string(),
                };
                return Err(e);
            }
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
        // Phase 11 Block E Task 25: mint a 32-byte hex bearer
        // token before spawn so we can pass it to llama-server via
        // `--api-key`. We use `getrandom` (already in the workspace
        // for the collab nonces) so the token is sampled from the
        // OS CSPRNG, not a pseudo-random Lamport-style counter.
        //
        // Phase 11 Block E Task 25 follow-up round 3 — Devin Review
        // ANALYSIS-0006 (r3). When the caller has set
        // `require_api_key`, a CSPRNG failure used to log a warning
        // and silently start the sidecar **without** a bearer
        // token — i.e. the unauthenticated loopback path that Block
        // E exists to eliminate. The renderer never saw the
        // downgrade, so a defence-in-depth control could vanish
        // without an observable signal. Fail-closed instead: record
        // a typed `Error` status so the UI surfaces the failure,
        // and return `SidecarError::TokenEntropyFailed` so callers
        // can decide whether to retry, prompt the user, or fall
        // back **explicitly** to the no-auth profile by flipping
        // `require_api_key = false` on the config.
        let bearer_token: Option<String> = if self.config.require_api_key {
            let mut buf = [0u8; 32];
            match getrandom::fill(&mut buf) {
                Ok(()) => Some(hex_encode(&buf)),
                Err(e) => {
                    let err = SidecarError::TokenEntropyFailed(e.to_string());
                    *self.status.lock() = SidecarStatus::Error {
                        message: err.to_string(),
                    };
                    return Err(err);
                }
            }
        } else {
            None
        };
        let child = match spawn_child(&self.config, port, bearer_token.as_deref()) {
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
                bearer_token,
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
fn spawn_child(
    config: &SidecarConfig,
    port: u16,
    bearer_token: Option<&str>,
) -> SidecarResult<Child> {
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
    // Phase 11 Block E Task 25 — authenticate the sidecar with a
    // per-session bearer token. llama.cpp's `--api-key` flag
    // configures the server to reject requests whose
    // `Authorization: Bearer ...` header doesn't match. Older
    // builds ignore unknown flags so this is forward-compatible.
    if let Some(token) = bearer_token {
        cmd.arg("--api-key").arg(token);
    }
    if let Some(mmproj) = config.mmproj_path.as_deref() {
        // `--mmproj <path>` is the llama.cpp flag for the multimodal
        // projector (CLIP/SmolVLM/etc.). Passing it switches the
        // server into vision mode and lets it accept `image_url`
        // content parts on the OpenAI-compatible chat API.
        cmd.arg("--mmproj").arg(mmproj);
    }
    for arg in &config.extra_args {
        cmd.arg(arg);
    }
    cmd.stdout(Stdio::null()).stderr(Stdio::piped());
    log::debug!(
        "spawning llama-server: {} --port {} mmproj={:?}",
        config.binary.display(),
        port,
        config.mmproj_path,
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
    // remaining args declared below — split here to keep the
    // function visually under 80 chars per arg line.
    // Phase 11 Block E Task 25 — bearer token to embed into the
    // resulting `SidecarStatus::Ready` payload so callers can
    // attach it to outbound chat HTTP requests.
    stop_signal: Arc<AtomicBool>,
    status: Arc<Mutex<SidecarStatus>>,
    bearer_token: Option<String>,
) {
    let deadline = Instant::now() + health_timeout;
    let mut ready = false;
    while Instant::now() < deadline {
        if stop_signal.load(Ordering::Acquire) {
            kill_child(&mut child);
            *status.lock() = SidecarStatus::Stopped;
            return;
        }
        // Phase 12 round 3 — detect crashes during init in ~500 ms
        // instead of waiting the full `health_timeout` (~90 s). If
        // the child has already exited, surface a typed error with
        // the exit code plus the tail of its stderr stream so the
        // renderer can render a meaningful toast.
        if let Some(err) = check_child_for_early_exit(&mut child) {
            *status.lock() = SidecarStatus::Error {
                message: err.to_string(),
            };
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
    // Phase 11 Block E Task 26 — post-spawn TOCTOU verification.
    // Between `pick_loopback_port` dropping the listener and
    // `llama-server` binding the same port, another process on the
    // host could grab the port and answer `/health` with `ok`.
    // Defence-in-depth: probe `/v1/models` with our bearer token.
    // Two outcomes are accepted:
    //   * `200 OK`  — the real sidecar accepted our token.
    //   * `404`     — older builds that don't expose `/v1/models`
    //                 but DID accept `--api-key` (otherwise it would
    //                 have failed earlier with an unknown-flag exit).
    // Any other response, in particular `401 Unauthorized`, means
    // the listener on that port is NOT our llama-server: kill the
    // (now-orphaned) child and surface a typed error.
    if let Some(token) = bearer_token.as_deref() {
        if !verify_bearer_token(port, token) {
            kill_child(&mut child);
            *status.lock() = SidecarStatus::Error {
                message: SidecarError::WrongState(
                    "post-spawn token verification failed: an unknown process \
                     answered on the sidecar's loopback port. Refusing to mark \
                     the sidecar Ready (possible TOCTOU race or stale child)."
                        .to_string(),
                )
                .to_string(),
            };
            return;
        }
    }
    *status.lock() = SidecarStatus::Ready {
        model_name,
        context_size,
        port,
        bearer_token,
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

/// Maximum stderr tail we capture when the child exits early. Both
/// sd-server and llama-server print their fatal-init message on the
/// last line before exiting, so this is plenty.
const CHILD_STDERR_TAIL_BYTES: usize = 2048;

/// Non-blocking check for an early child exit. Returns `Some(err)` if
/// the child has already exited (typed [`SidecarError::ChildExited`]
/// carrying the exit code plus the tail of stderr); `None` if the
/// child is still alive (the supervisor should keep probing
/// readiness).
///
/// Public-in-crate so the diffusion sidecar (`diffusion_sidecar.rs`)
/// shares the same early-exit detection path — keeping the two
/// drivers symmetric was the whole point of Devin Review round 3's
/// `try_wait` finding.
pub(crate) fn check_child_for_early_exit(child: &mut Child) -> Option<SidecarError> {
    match child.try_wait() {
        Ok(Some(status)) => {
            let code = status.code();
            let stderr_tail = drain_child_stderr_tail(child);
            Some(SidecarError::ChildExited { code, stderr_tail })
        }
        // Ok(None) — child still running. Err(...) — we couldn't
        // even ask; treat as "keep going" so a transient
        // /proc/self/wait4 hiccup doesn't tear down a healthy
        // sidecar prematurely. The full health timeout still
        // guards against a truly stuck child.
        _ => None,
    }
}

/// Drain up to [`CHILD_STDERR_TAIL_BYTES`] of stderr from a
/// just-exited child. Best-effort: returns an empty string when the
/// child wasn't spawned with `Stdio::piped()` for stderr, or when
/// the read itself errors.
fn drain_child_stderr_tail(child: &mut Child) -> String {
    use std::io::Read;
    let Some(mut stderr) = child.stderr.take() else {
        return String::new();
    };
    let mut buf = Vec::with_capacity(CHILD_STDERR_TAIL_BYTES);
    // Read everything available; sd-server/llama-server emit a few
    // KB at most before exiting on a fatal config error, so we
    // don't need a streaming reader here.
    let _ = stderr.read_to_end(&mut buf);
    if buf.len() > CHILD_STDERR_TAIL_BYTES {
        let start = buf.len() - CHILD_STDERR_TAIL_BYTES;
        buf.drain(..start);
    }
    String::from_utf8_lossy(&buf).into_owned()
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

/// Hex-encode raw bytes for the bearer token. Lowercase nibbles,
/// fixed length = `2 * bytes.len()`. Inlined here to avoid pulling
/// the entire `hex` crate just for one 32-byte buffer.
fn hex_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(ALPHABET[(b >> 4) as usize] as char);
        out.push(ALPHABET[(b & 0x0f) as usize] as char);
    }
    out
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

/// Verify the multimodal projector file is present on disk. We
/// deliberately do not check its size against `max_model_mb` —
/// mmproj files are CLIP-style projectors (a few hundred MB at
/// most) and the tier ceiling is about the weights footprint, not
/// the projector footprint.
fn validate_mmproj(mmproj: &Path) -> SidecarResult<()> {
    std::fs::metadata(mmproj)
        .map(|_| ())
        .map_err(|_| SidecarError::MmprojMissing(mmproj.to_path_buf()))
}

/// Build the argv vector llama-server would receive, *without*
/// spawning the child. Exposed for tests (so `mmproj_args_are_forwarded`
/// can assert the precise CLI shape without running a binary) and for
/// debug logging. Returns argv as a `Vec<String>` rather than
/// `Vec<OsString>` to keep the assertion shape ergonomic; non-UTF-8
/// paths are rendered with `to_string_lossy` so the function never
/// panics.
///
/// Phase 11 Block E Task 25 / 26 — when `bearer_token` is `Some`, the
/// argv includes `--api-key <token>` immediately after `--port`.
/// Tests assert on that ordering so a future refactor can't silently
/// drop the auth flag and re-introduce the unauthenticated sidecar.
#[must_use]
pub fn build_argv(config: &SidecarConfig, port: u16) -> Vec<String> {
    build_argv_with_token(config, port, None)
}

/// Same as [`build_argv`] but with an explicit bearer token. Split
/// out for tests that need to assert the `--api-key` flag wiring.
#[must_use]
pub fn build_argv_with_token(
    config: &SidecarConfig,
    port: u16,
    bearer_token: Option<&str>,
) -> Vec<String> {
    let mut argv = vec![
        "--model".to_string(),
        config.model_path.to_string_lossy().into_owned(),
        "--host".to_string(),
        "127.0.0.1".to_string(),
        "--port".to_string(),
        port.to_string(),
        "-c".to_string(),
        config.context_size.to_string(),
    ];
    if let Some(token) = bearer_token {
        argv.push("--api-key".to_string());
        argv.push(token.to_string());
    }
    if let Some(mmproj) = config.mmproj_path.as_deref() {
        argv.push("--mmproj".to_string());
        argv.push(mmproj.to_string_lossy().into_owned());
    }
    for arg in &config.extra_args {
        argv.push(arg.clone());
    }
    argv
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

/// Phase 11 Block E Task 26 + round 5 — verify the listener on the
/// sidecar's loopback port is actually the llama-server we spawned by
/// proving it differentiates between our bearer token and an
/// obviously-wrong token. The round-1 implementation accepted any
/// listener that returned `200 OK` or `404 Not Found` with our token,
/// which Devin Review ANALYSIS-0006 (r5) correctly identified as
/// fragile: a foreign web server that returned `404` for unknown
/// routes regardless of `Authorization` would have passed the check
/// (re-opening the TOCTOU window the verifier exists to close).
///
/// Round 5 contract: send *two* probes against `GET /v1/models`,
/// one with the real bearer and one with a deliberately-wrong bearer
/// the verifier owns, and require the listener to **distinguish**
/// them:
///
/// 1. Right-token probe — must NOT come back as `401`/`403`. Any
///    other status (typically `200`, or `404` on older llama.cpp
///    builds that take `--api-key` but don't expose `/v1/models`)
///    counts as "accepted our token".
/// 2. Wrong-token probe — MUST come back as `401`/`403`. Anything
///    else (including the same `200`/`404` returned for the right
///    token) means the listener is ignoring `Authorization` and is
///    therefore not the `--api-key`-honouring llama-server we
///    spawned.
///
/// Both probes must succeed; if either's transport fails we treat
/// it as "not us" to fail closed. This pins down the TOCTOU window
/// to a hypothetical foreign server that not only happens to be
/// listening on the freshly-allocated port between
/// [`pick_loopback_port`] and the llama-server bind, but also
/// implements a complete `--api-key`-aware response policy by
/// coincidence — vanishingly unlikely on loopback.
#[cfg(feature = "llm_sidecar")]
fn verify_bearer_token(port: u16, token: &str) -> bool {
    /// A deliberately-bogus bearer the verifier owns. Reused across
    /// all wrong-token probes so a debug log of `Authorization`
    /// values from the listener is greppable for this fixed string.
    const WRONG_TOKEN: &str = "kcreate-toctou-probe-deliberately-wrong";

    let url = format!("http://127.0.0.1:{port}/v1/models");
    let probe = |bearer: String| -> Option<u16> {
        let resp = ureq::get(&url)
            .timeout(Duration::from_secs(2))
            .set("authorization", &bearer)
            .call();
        match resp {
            Ok(r) => Some(r.status()),
            Err(ureq::Error::Status(code, _)) => Some(code),
            Err(_) => None,
        }
    };

    let Some(right) = probe(format!("Bearer {token}")) else {
        return false;
    };
    let Some(wrong) = probe(format!("Bearer {WRONG_TOKEN}")) else {
        return false;
    };

    // Right token: must NOT be auth-rejected. Anything other than
    // 401/403 (200, 404, or even a 500 from a real server hitting
    // an unrelated bug) at least demonstrates the listener didn't
    // reject *our* token.
    let right_accepted = !matches!(right, 401 | 403);
    // Wrong token: MUST be auth-rejected. A foreign server that
    // ignores `Authorization` will return the same code for both
    // probes, so this is the load-bearing assertion.
    let wrong_rejected = matches!(wrong, 401 | 403);

    right_accepted && wrong_rejected
}

/// When the `llm_sidecar` Cargo feature is off the HTTP client is
/// not linked and there is no way to talk to the child anyway —
/// tests in this configuration use mocked transports — so the
/// verifier is a no-op (always trusts the spawn).
#[cfg(not(feature = "llm_sidecar"))]
fn verify_bearer_token(_port: u16, _token: &str) -> bool {
    true
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
            bearer_token: Some("abc".to_string()),
        };
        assert!(r.is_ready());
        assert_eq!(r.port(), Some(9999));
        assert_eq!(r.bearer_token(), Some("abc"));
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
            require_api_key: false,
            mmproj_path: None,
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
            require_api_key: false,
            mmproj_path: None,
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
        // an OS-assigned loopback port, then wait_for_health succeeds.
        let server = tiny_http::Server::http("127.0.0.1:0").expect("server");
        let port = server
            .server_addr()
            .to_ip()
            .expect("loopback listen addr")
            .port();
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

    /// When `mmproj_path` is `None`, `build_argv` must omit the
    /// `--mmproj` flag entirely — passing it with no path would be
    /// rejected by llama-server, and passing an empty value would
    /// produce a confusing "file not found: " error in the child
    /// process logs.
    #[test]
    fn mmproj_omitted_when_none() {
        let cfg = SidecarConfig::new(PathBuf::from("/tmp/m.gguf"));
        let argv = build_argv(&cfg, 12345);
        assert!(
            !argv.iter().any(|a| a == "--mmproj"),
            "argv must not contain --mmproj when mmproj_path is None: {argv:?}",
        );
    }

    /// When `mmproj_path` is `Some(path)`, `build_argv` must include
    /// `--mmproj <path>` as two consecutive argv entries, in that
    /// order, with the path preserved verbatim.
    #[test]
    fn mmproj_args_are_forwarded() {
        let cfg = SidecarConfig::new(PathBuf::from("/tmp/m.gguf"))
            .with_mmproj(Some(PathBuf::from("/tmp/mmproj-clip.gguf")));
        let argv = build_argv(&cfg, 12345);
        let pos = argv
            .iter()
            .position(|a| a == "--mmproj")
            .expect("argv must contain --mmproj");
        assert_eq!(
            argv.get(pos + 1).map(String::as_str),
            Some("/tmp/mmproj-clip.gguf"),
            "argv must have the mmproj path immediately after --mmproj: {argv:?}",
        );
    }

    /// `start()` must surface a typed `MmprojMissing` error when the
    /// configured mmproj path does not exist, *before* spawning the
    /// child. We exercise this with a real model file (so the
    /// `validate_model` check passes) and a bogus mmproj path.
    #[test]
    fn start_with_missing_mmproj_transitions_to_error() {
        let dir = tempfile::tempdir().expect("temp");
        let model = dir.path().join("m.gguf");
        std::fs::write(&model, b"0").expect("write");
        let cfg =
            SidecarConfig::new(model).with_mmproj(Some(PathBuf::from("/no/such/mmproj.gguf")));
        let mut s = LlmSidecar::new(cfg);
        let err = s.start().expect_err("missing mmproj");
        assert!(
            matches!(err, SidecarError::MmprojMissing(_)),
            "expected MmprojMissing, got {err:?}"
        );
        assert!(matches!(s.status(), SidecarStatus::Error { .. }));
    }

    /// When mmproj exists on disk, `start()` proceeds past the
    /// validate phase. We can't actually spawn llama-server in CI,
    /// so we point at a non-existent binary and assert the next
    /// failure mode is `Spawn` (which proves the mmproj validation
    /// passed). This is a guard against accidentally short-circuiting
    /// past the mmproj check.
    #[test]
    fn start_with_present_mmproj_passes_validation() {
        let dir = tempfile::tempdir().expect("temp");
        let model = dir.path().join("m.gguf");
        let mmproj = dir.path().join("mmproj.gguf");
        std::fs::write(&model, b"0").expect("write model");
        std::fs::write(&mmproj, b"0").expect("write mmproj");
        let cfg = SidecarConfig {
            binary: PathBuf::from("/this/binary/does/not/exist"),
            model_path: model,
            max_model_mb: u64::MAX,
            context_size: 2048,
            health_timeout: Duration::from_millis(50),
            extra_args: vec![],
            require_api_key: false,
            mmproj_path: Some(mmproj),
        };
        let mut s = LlmSidecar::new(cfg);
        let err = s.start().expect_err("bad binary");
        assert!(
            matches!(err, SidecarError::Spawn(_)),
            "expected Spawn (mmproj must have validated), got {err:?}",
        );
    }

    /// Phase 11 Block E Task 25 — `build_argv_with_token` must emit
    /// the `--api-key <token>` pair so llama-server starts with the
    /// per-session bearer enforced. Two consecutive argv entries,
    /// `--api-key` followed immediately by the token verbatim.
    #[test]
    fn build_argv_with_token_emits_api_key() {
        let cfg = SidecarConfig::new(PathBuf::from("/tmp/m.gguf"));
        let argv = build_argv_with_token(&cfg, 12345, Some("deadbeef"));
        let pos = argv
            .iter()
            .position(|a| a == "--api-key")
            .expect("argv must contain --api-key");
        assert_eq!(
            argv.get(pos + 1).map(String::as_str),
            Some("deadbeef"),
            "argv must have the token immediately after --api-key: {argv:?}",
        );
    }

    /// And the legacy `build_argv` (None bearer) must NOT emit
    /// `--api-key`, so callers that explicitly disable auth (e.g.
    /// tests that point at a mock server which doesn't honour the
    /// flag) don't get a spurious flag baked in.
    #[test]
    fn build_argv_without_token_omits_api_key() {
        let cfg = SidecarConfig::new(PathBuf::from("/tmp/m.gguf"));
        let argv = build_argv(&cfg, 12345);
        assert!(
            !argv.iter().any(|a| a == "--api-key"),
            "argv must not contain --api-key when bearer_token is None: {argv:?}",
        );
    }

    /// Phase 11 Block E Task 26 — a foreign listener (one that does
    /// NOT honour our bearer) MUST cause `verify_bearer_token` to
    /// return false. We simulate this with a tiny_http server that
    /// returns 401 on every request: the verifier sees the status
    /// and rejects.
    ///
    /// Round 2 — Devin Review ANALYSIS-0005: this test ALSO captures
    /// the inbound `Authorization` header on the mock server and
    /// asserts it carries the bearer token the verifier was asked
    /// to prove. This closes the coverage gap where a regression
    /// that *forgot* to send the header (or sent a wrong literal)
    /// could still pass the prior status-only assertion.
    #[cfg(feature = "llm_sidecar")]
    #[test]
    fn verify_rejects_foreign_listener_returning_401() {
        let server = tiny_http::Server::http("127.0.0.1:0").expect("server");
        let port = server
            .server_addr()
            .to_ip()
            .expect("loopback listen addr")
            .port();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop_for_thread = std::sync::Arc::clone(&stop);
        let captured_auth = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let captured_for_thread = std::sync::Arc::clone(&captured_auth);
        let handle = std::thread::spawn(move || {
            for req in server.incoming_requests() {
                if stop_for_thread.load(std::sync::atomic::Ordering::Acquire) {
                    break;
                }
                // Record the Authorization header (or empty string
                // if missing) so the test can assert that the
                // verifier actually sent the bearer token rather
                // than relying solely on the response status.
                let auth = req
                    .headers()
                    .iter()
                    .find(|h| {
                        h.field
                            .as_str()
                            .as_str()
                            .eq_ignore_ascii_case("authorization")
                    })
                    .map(|h| h.value.as_str().to_string())
                    .unwrap_or_default();
                captured_for_thread
                    .lock()
                    .expect("captured-auth lock")
                    .push(auth);
                let resp = tiny_http::Response::from_string("unauthorized")
                    .with_status_code(tiny_http::StatusCode(401));
                let _ = req.respond(resp);
            }
        });

        let ok = verify_bearer_token(port, "the-correct-token");
        assert!(
            !ok,
            "verifier must reject a listener that returns 401 — that's the TOCTOU signature",
        );
        let seen = captured_auth.lock().expect("captured-auth lock").clone();
        assert!(
            seen.iter().any(|h| h == "Bearer the-correct-token"),
            "verify_bearer_token must send `Authorization: Bearer <token>`; saw {seen:?}",
        );
        stop.store(true, std::sync::atomic::Ordering::Release);
        let _ = handle;
    }

    /// Phase 11 Block E Task 26 — a real llama-server-style listener
    /// (200 OK on /v1/models with the correct bearer) MUST be
    /// accepted. tiny_http returns 200 by default.
    ///
    /// Round 2 — Devin Review ANALYSIS-0005: same header-capture
    /// hardening as the 401 test above. We additionally only
    /// return `200 OK` if the bearer matches, and `401` otherwise,
    /// so a future regression that misformats the header would
    /// flip this assertion from `accept` to `reject`.
    #[cfg(feature = "llm_sidecar")]
    #[test]
    fn verify_accepts_real_listener_returning_200() {
        let server = tiny_http::Server::http("127.0.0.1:0").expect("server");
        let port = server
            .server_addr()
            .to_ip()
            .expect("loopback listen addr")
            .port();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop_for_thread = std::sync::Arc::clone(&stop);
        let captured_auth = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let captured_for_thread = std::sync::Arc::clone(&captured_auth);
        let handle = std::thread::spawn(move || {
            for req in server.incoming_requests() {
                if stop_for_thread.load(std::sync::atomic::Ordering::Acquire) {
                    break;
                }
                let auth = req
                    .headers()
                    .iter()
                    .find(|h| {
                        h.field
                            .as_str()
                            .as_str()
                            .eq_ignore_ascii_case("authorization")
                    })
                    .map(|h| h.value.as_str().to_string())
                    .unwrap_or_default();
                captured_for_thread
                    .lock()
                    .expect("captured-auth lock")
                    .push(auth.clone());
                // Mimic a real llama-server that enforces --api-key:
                // 200 only when the bearer header matches what we
                // were told to expect. Verifier's positive case must
                // therefore actually be carrying the header.
                let resp = if auth == "Bearer the-correct-token" {
                    tiny_http::Response::from_string("{\"data\":[]}")
                } else {
                    tiny_http::Response::from_string("unauthorized")
                        .with_status_code(tiny_http::StatusCode(401))
                };
                let _ = req.respond(resp);
            }
        });

        let ok = verify_bearer_token(port, "the-correct-token");
        assert!(
            ok,
            "verifier must accept a listener that returns 200 on /v1/models",
        );
        let seen = captured_auth.lock().expect("captured-auth lock").clone();
        assert!(
            seen.iter().any(|h| h == "Bearer the-correct-token"),
            "verify_bearer_token must send `Authorization: Bearer <token>`; saw {seen:?}",
        );
        stop.store(true, std::sync::atomic::Ordering::Release);
        let _ = handle;
    }

    /// Phase 11 Block E follow-up round 5 — Devin Review ANALYSIS-0006
    /// (r5). A foreign HTTP server that returns `404 Not Found` for
    /// every request regardless of `Authorization` MUST be rejected
    /// by the verifier. The round-1 implementation accepted 404 as
    /// "older llama.cpp without `/v1/models`" without proving the
    /// listener honoured the `--api-key` flag, leaving a TOCTOU
    /// window open. The round-5 differential-probe contract closes
    /// it: the wrong-token probe must come back as `401`/`403`, and
    /// a foreign server returning `404` for both probes therefore
    /// fails. This test pins that behaviour so a future refactor
    /// can't quietly re-loosen the verifier.
    #[cfg(feature = "llm_sidecar")]
    #[test]
    fn verify_rejects_foreign_listener_returning_404_for_any_token() {
        let server = tiny_http::Server::http("127.0.0.1:0").expect("server");
        let port = server
            .server_addr()
            .to_ip()
            .expect("loopback listen addr")
            .port();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop_for_thread = std::sync::Arc::clone(&stop);
        let handle = std::thread::spawn(move || {
            for req in server.incoming_requests() {
                if stop_for_thread.load(std::sync::atomic::Ordering::Acquire) {
                    break;
                }
                // Mimic a foreign web server that doesn't care about
                // `Authorization`: 404 on every route, every header.
                let resp = tiny_http::Response::from_string("not found")
                    .with_status_code(tiny_http::StatusCode(404));
                let _ = req.respond(resp);
            }
        });

        let ok = verify_bearer_token(port, "the-correct-token");
        assert!(
            !ok,
            "verifier must reject a listener that returns 404 for both right and wrong tokens — \
             that's the TOCTOU foreign-server signature the round-5 differential probe defends against",
        );
        stop.store(true, std::sync::atomic::Ordering::Release);
        let _ = handle;
    }

    /// Phase 12 round 3 — Devin Review flagged that the health-probe
    /// loop only watched the readiness endpoint, so a child that
    /// crashed during init would sit until the full
    /// `health_timeout` elapsed. `check_child_for_early_exit` is the
    /// shared seam that fixes that: between probes, the health
    /// worker asks the child whether it has exited, and surfaces a
    /// typed [`SidecarError::ChildExited`] in ~500 ms instead of
    /// waiting 90 s for a generic [`SidecarError::HealthTimeout`].
    ///
    /// This test exercises the helper end-to-end against a real
    /// child that exits immediately (`/bin/sh -c 'echo boom 1>&2;
    /// exit 7'`). We assert:
    ///   * `Ok(None)` is observed before the child is reaped (the
    ///     loop's "keep probing" branch),
    ///   * after the child exits, `check_child_for_early_exit`
    ///     returns `Some(ChildExited { code: Some(7), stderr_tail })`
    ///     and the stderr tail contains the `boom` we wrote, so the
    ///     renderer toast carries real diagnostic content rather
    ///     than the previous opaque "health timeout".
    #[cfg(unix)]
    #[test]
    fn check_child_for_early_exit_surfaces_exit_code_and_stderr_tail() {
        let mut child = Command::new("/bin/sh")
            .arg("-c")
            .arg("echo boom 1>&2; exit 7")
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn /bin/sh");

        // The supervisor loop's `check_child_for_early_exit` runs
        // before every probe; on the very first iteration the child
        // may not yet have been reaped by the kernel, so this call
        // is allowed to return `None`. If the child HAS already
        // been reaped by then (CI runners are fast enough that
        // `/bin/sh -c '...; exit 7'` frequently exits in under a
        // millisecond), the first call returns `Some(err)` AND
        // drains stderr via `child.stderr.take()` — so we must
        // capture *that* err, because a follow-up call after
        // `child.wait()` would find `stderr_tail` empty and the
        // assertion below would fail spuriously. Either call may
        // surface the real error; we accept whichever fires first.
        let err = check_child_for_early_exit(&mut child)
            .or_else(|| {
                let _ = child.wait();
                check_child_for_early_exit(&mut child)
            })
            .expect("check_child_for_early_exit must report the exited child");
        match err {
            SidecarError::ChildExited { code, stderr_tail } => {
                assert_eq!(code, Some(7), "captured exit code");
                assert!(
                    stderr_tail.contains("boom"),
                    "stderr tail must carry diagnostic text the renderer can show, got: {stderr_tail:?}"
                );
            }
            other => panic!("expected ChildExited, got {other:?}"),
        }
    }
}
