//! Image-generation sidecar (FLUX diffusion).
//!
//! This module spawns and supervises a Python subprocess that runs
//! a minimal diffusers-based HTTP server (see
//! `tools/kcreate_diffusion/server.py`). The server binds loopback
//! only, exposes a `/health` probe and a
//! `POST /v1/images/generations` endpoint that accepts
//!
//! ```json
//! { "prompt": "...", "width": 1024, "height": 1024, "steps": 20 }
//! ```
//!
//! and returns
//!
//! ```json
//! { "image": "<base64 PNG>", "width": 1024, "height": 1024 }
//! ```
//!
//! The HOST keeps everything else identical to `llm_sidecar.rs`:
//! status enum, lifecycle, kill-on-drop, port allocation, no
//! external network. Image generation is gated to Tier 2+ with
//! GPU; the UI hides the panel below that gate (the gate is
//! enforced by [`kcreate_core::config::RuntimeConfig::image_generation_allowed`]).
//!
//! Local-first invariant: this module is in `kcreate_ai` (not the
//! editing path), and its `ureq` dependency is feature-gated behind
//! `llm_sidecar` just like the chat client. The editing-path
//! `local_first.rs` deny-list test stays green.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::llm_sidecar::{SidecarError, SidecarResult, SidecarStatus};

/// Default health-check ceiling. FLUX cold loads can take a while
/// on the first request (the diffusers warm-up step compiles the
/// CUDA kernels), so we allow a generous 90 s window. Subsequent
/// generations are bounded by the per-request timeout.
const DEFAULT_HEALTH_TIMEOUT: Duration = Duration::from_secs(90);
const HEALTH_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Tunables for the image-generation sidecar. Mirrors
/// [`crate::llm_sidecar::SidecarConfig`] field-for-field where it
/// makes sense, and adds `host_python_module` so callers can swap
/// the server entry point (useful for tests that stand up a
/// `tools.fake_diffusion.server` instead of the real one).
#[derive(Debug, Clone)]
pub struct ImageGenConfig {
    /// Python interpreter to invoke.
    pub python: PathBuf,
    /// Python module to run with `-m`. Defaults to
    /// `kcreate_diffusion.server`.
    pub host_python_module: String,
    /// Path or HF slug for the diffusion weights.
    pub model_path: PathBuf,
    /// Sidecar must transition to Ready within this duration.
    pub health_timeout: Duration,
    /// Extra args forwarded to the Python entry point.
    pub extra_args: Vec<String>,
}

impl ImageGenConfig {
    /// Construct a config from a model path with default knobs.
    #[must_use]
    pub fn new(model_path: PathBuf) -> Self {
        Self {
            python: PathBuf::from("python3"),
            host_python_module: "kcreate_diffusion.server".to_string(),
            model_path,
            health_timeout: DEFAULT_HEALTH_TIMEOUT,
            extra_args: Vec::new(),
        }
    }
}

/// Image-generation sidecar driver. Same shape as
/// [`crate::llm_sidecar::LlmSidecar`].
#[derive(Debug)]
pub struct ImageGenSidecar {
    config: ImageGenConfig,
    status: Arc<Mutex<SidecarStatus>>,
    stop_signal: Option<Arc<AtomicBool>>,
    worker: Option<thread::JoinHandle<()>>,
}

impl ImageGenSidecar {
    /// Construct a stopped sidecar with the given config.
    #[must_use]
    pub fn new(config: ImageGenConfig) -> Self {
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

    /// Spawn the Python child process and start probing `/health`.
    /// Returns the listening port immediately; the caller observes
    /// `Ready`/`Error` by polling [`Self::status`].
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
        let stop = Arc::new(AtomicBool::new(false));
        let status_for_worker = Arc::clone(&self.status);
        let stop_for_worker = Arc::clone(&stop);
        let model_name = self
            .config
            .model_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("image-gen")
            .to_string();
        let health_timeout = self.config.health_timeout;

        let handle = thread::spawn(move || {
            health_worker(
                child,
                port,
                model_name,
                health_timeout,
                stop_for_worker,
                status_for_worker,
            );
        });
        self.stop_signal = Some(stop);
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

impl Drop for ImageGenSidecar {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Build the argv that the Python entry point receives. Public for
/// tests + debug logging.
#[must_use]
pub fn build_argv(config: &ImageGenConfig, port: u16) -> Vec<String> {
    let mut argv = vec![
        "-m".to_string(),
        config.host_python_module.clone(),
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

fn spawn_child(config: &ImageGenConfig, port: u16) -> SidecarResult<Child> {
    let mut cmd = Command::new(&config.python);
    for arg in build_argv(config, port) {
        cmd.arg(arg);
    }
    cmd.stdout(Stdio::null()).stderr(Stdio::piped());
    log::debug!(
        "spawning image-gen sidecar: {} -m {} --port {}",
        config.python.display(),
        config.host_python_module,
        port,
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
        // FLUX context isn't a token budget; we report 0 to keep
        // the wire shape uniform with the chat sidecar.
        context_size: 0,
        port,
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

/// Diffusion model files are typically directories (HF format) or
/// single GGUF/safetensors files. We accept either, and let
/// non-existent paths fall through assuming they're HF slugs (same
/// contract as [`crate::mlx_sidecar`]).
fn validate_model(model: &Path) -> SidecarResult<()> {
    if model.as_os_str().is_empty() {
        return Err(SidecarError::ModelMissing(model.to_path_buf()));
    }
    Ok(())
}

#[cfg(feature = "llm_sidecar")]
fn probe_health(port: u16) -> bool {
    let url = format!("http://127.0.0.1:{port}/health");
    match ureq::get(&url).timeout(Duration::from_secs(2)).call() {
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

// ---- Generation client (Task 9) ----

/// Errors returned by [`generate_image`].
#[derive(Debug, Error)]
pub enum ImageGenError {
    /// The configured feature flag wasn't enabled — i.e., the build
    /// didn't pull in `ureq`, so there's no HTTP client to call the
    /// loopback server with. The host should fall back to a "sidecar
    /// not available" UI state. Matches the [`crate::llm_chat`]
    /// pattern.
    #[error("image generation requires the `llm_sidecar` cargo feature")]
    FeatureDisabled,
    /// Loopback HTTP error (server not running, connection refused,
    /// non-200 response).
    #[error("image-gen HTTP error: {0}")]
    Http(String),
    /// JSON decode error on the response body.
    #[error("image-gen response decode error: {0}")]
    Decode(String),
    /// Base64 decode error on the returned PNG payload.
    #[error("image-gen base64 decode error: {0}")]
    Base64(String),
    /// PNG decode error after base64.
    #[error("image-gen PNG decode error: {0}")]
    Png(String),
    /// Server returned a non-success status.
    #[error("image-gen server status {status}: {body}")]
    Status { status: u16, body: String },
}

/// Result alias for image-gen operations.
pub type ImageGenResult<T> = Result<T, ImageGenError>;

/// Request payload that mirrors what the Python server expects.
/// Public so tests / IPC layer can construct it directly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageGenRequest {
    pub prompt: String,
    pub width: u32,
    pub height: u32,
    /// Number of inference steps. Higher = better quality + slower.
    /// FLUX schnell variants tolerate 4–8 steps; the dev variant
    /// wants 20–30.
    pub steps: u32,
    /// Random seed; `None` means the server chooses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
}

/// Decoded image returned by [`generate_image`]: raw RGBA8 pixel
/// bytes plus the exact width/height. The caller is responsible
/// for laying these into the document as a new raster layer.
#[derive(Debug, Clone)]
pub struct GeneratedImage {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Wire shape of the server response — separate from
/// [`GeneratedImage`] so we can parse the JSON without committing
/// to the RGBA expansion until after base64+PNG decode.
#[cfg(feature = "llm_sidecar")]
#[derive(Debug, Clone, Deserialize)]
struct ImageGenResponseWire {
    image: String,
    width: u32,
    height: u32,
}

/// POST a generation request to the local diffusion server and
/// return decoded RGBA pixels. Network access is loopback-only
/// (the URL is hard-coded to `127.0.0.1:<port>`).
#[cfg(feature = "llm_sidecar")]
pub fn generate_image(port: u16, req: &ImageGenRequest) -> ImageGenResult<GeneratedImage> {
    let url = format!("http://127.0.0.1:{port}/v1/images/generations");
    let body = serde_json::to_string(req).map_err(|e| ImageGenError::Decode(e.to_string()))?;
    let resp = ureq::post(&url)
        // Generations can take 10–60 s on a warm pipeline; bump
        // the timeout well above what `llm_chat` uses.
        .timeout(Duration::from_mins(10))
        .set("Content-Type", "application/json")
        .send_string(&body)
        .map_err(map_ureq_error)?;
    if resp.status() != 200 {
        let status = resp.status();
        let body = resp.into_string().unwrap_or_default();
        return Err(ImageGenError::Status { status, body });
    }
    let parsed: ImageGenResponseWire = resp
        .into_json()
        .map_err(|e| ImageGenError::Decode(e.to_string()))?;
    decode_png_payload(&parsed.image, parsed.width, parsed.height)
}

#[cfg(not(feature = "llm_sidecar"))]
pub fn generate_image(_port: u16, _req: &ImageGenRequest) -> ImageGenResult<GeneratedImage> {
    Err(ImageGenError::FeatureDisabled)
}

#[cfg(feature = "llm_sidecar")]
fn map_ureq_error(e: ureq::Error) -> ImageGenError {
    match e {
        ureq::Error::Status(s, r) => ImageGenError::Status {
            status: s,
            body: r.into_string().unwrap_or_default(),
        },
        ureq::Error::Transport(t) => ImageGenError::Http(t.to_string()),
    }
}

/// Decode a base64-encoded PNG into RGBA8 pixels. Used after the
/// server response is parsed; broken out so tests can exercise the
/// decode independently of the HTTP round-trip.
pub fn decode_png_payload(
    base64_png: &str,
    expected_width: u32,
    expected_height: u32,
) -> ImageGenResult<GeneratedImage> {
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(base64_png.as_bytes())
        .map_err(|e| ImageGenError::Base64(e.to_string()))?;
    let img = image::load_from_memory(&bytes).map_err(|e| ImageGenError::Png(e.to_string()))?;
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width(), rgba.height());
    if w != expected_width || h != expected_height {
        return Err(ImageGenError::Decode(format!(
            "server returned {w}x{h}, expected {expected_width}x{expected_height}",
        )));
    }
    Ok(GeneratedImage {
        rgba: rgba.into_raw(),
        width: w,
        height: h,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argv_invokes_python_module() {
        let cfg = ImageGenConfig::new(PathBuf::from("/tmp/flux.gguf"));
        let argv = build_argv(&cfg, 12345);
        assert_eq!(argv.first().map(String::as_str), Some("-m"));
        assert_eq!(
            argv.get(1).map(String::as_str),
            Some("kcreate_diffusion.server"),
        );
        let host_pos = argv.iter().position(|a| a == "--host").unwrap();
        assert_eq!(
            argv.get(host_pos + 1).map(String::as_str),
            Some("127.0.0.1")
        );
        let port_pos = argv.iter().position(|a| a == "--port").unwrap();
        assert_eq!(argv.get(port_pos + 1).map(String::as_str), Some("12345"));
    }

    #[test]
    fn validate_rejects_empty_path() {
        let err = validate_model(Path::new("")).expect_err("empty path");
        assert!(matches!(err, SidecarError::ModelMissing(_)));
    }

    #[test]
    fn start_with_bad_python_transitions_to_error() {
        let cfg = ImageGenConfig {
            python: PathBuf::from("/this/python/does/not/exist"),
            host_python_module: "kcreate_diffusion.server".into(),
            model_path: PathBuf::from("/tmp/flux"),
            health_timeout: Duration::from_millis(50),
            extra_args: vec![],
        };
        let mut s = ImageGenSidecar::new(cfg);
        let err = s.start().expect_err("bad python");
        assert!(matches!(err, SidecarError::Spawn(_)));
        assert!(matches!(s.status(), SidecarStatus::Error { .. }));
    }

    /// Round-trip a known 2x2 RGBA PNG through base64 + PNG decode.
    /// Exercises [`decode_png_payload`] without needing an HTTP
    /// server.
    #[test]
    fn decode_png_payload_round_trip() {
        use base64::Engine as _;
        // Build a 2x2 red/green/blue/white image and PNG-encode it.
        let pixels: Vec<u8> = vec![
            255, 0, 0, 255, // top-left red
            0, 255, 0, 255, // top-right green
            0, 0, 255, 255, // bottom-left blue
            255, 255, 255, 255, // bottom-right white
        ];
        let img: image::RgbaImage = image::ImageBuffer::from_raw(2, 2, pixels.clone()).unwrap();
        let mut png_bytes: Vec<u8> = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(
                &mut std::io::Cursor::new(&mut png_bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png_bytes);
        let decoded = decode_png_payload(&b64, 2, 2).unwrap();
        assert_eq!(decoded.width, 2);
        assert_eq!(decoded.height, 2);
        assert_eq!(decoded.rgba, pixels);
    }

    /// Mismatched dimensions surface as a `Decode` error, not a
    /// silent reshape. Catches a server that hands back the wrong
    /// resolution.
    #[test]
    fn decode_png_payload_rejects_dimension_mismatch() {
        use base64::Engine as _;
        let pixels: Vec<u8> = vec![0u8; 4 * 4]; // 2x2 RGBA
        let img: image::RgbaImage = image::ImageBuffer::from_raw(2, 2, pixels).unwrap();
        let mut png_bytes: Vec<u8> = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(
                &mut std::io::Cursor::new(&mut png_bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png_bytes);
        let err = decode_png_payload(&b64, 3, 3).expect_err("wrong size");
        assert!(matches!(err, ImageGenError::Decode(_)));
    }

    /// Mock the diffusion server with `tiny_http` and round-trip a
    /// real generate request. Confirms the wire format the Python
    /// server must implement.
    #[cfg(feature = "llm_sidecar")]
    #[test]
    fn generate_image_round_trip_against_mock() {
        use base64::Engine as _;
        let server = tiny_http::Server::http("127.0.0.1:0").expect("mock server");
        let port = server.server_addr().to_ip().expect("ip addr").port();
        // Background thread that responds to the /v1/images/generations
        // endpoint with a base64 PNG.
        let handle = std::thread::spawn(move || {
            for req in server.incoming_requests().take(1) {
                let pixels: Vec<u8> = vec![
                    10, 20, 30, 255, 40, 50, 60, 255, 70, 80, 90, 255, 100, 110, 120, 255,
                ];
                let img: image::RgbaImage = image::ImageBuffer::from_raw(2, 2, pixels).unwrap();
                let mut png_bytes: Vec<u8> = Vec::new();
                image::DynamicImage::ImageRgba8(img)
                    .write_to(
                        &mut std::io::Cursor::new(&mut png_bytes),
                        image::ImageFormat::Png,
                    )
                    .unwrap();
                let b64 = base64::engine::general_purpose::STANDARD.encode(&png_bytes);
                let body = format!(r#"{{"image":"{b64}","width":2,"height":2}}"#);
                let resp = tiny_http::Response::from_string(body).with_header(
                    "Content-Type: application/json"
                        .parse::<tiny_http::Header>()
                        .unwrap(),
                );
                let _ = req.respond(resp);
            }
        });
        let req = ImageGenRequest {
            prompt: "a cat".into(),
            width: 2,
            height: 2,
            steps: 4,
            seed: Some(42),
        };
        let img = generate_image(port, &req).expect("generate");
        assert_eq!(img.width, 2);
        assert_eq!(img.height, 2);
        assert_eq!(img.rgba.len(), 16);
        let _ = handle.join();
    }
}
