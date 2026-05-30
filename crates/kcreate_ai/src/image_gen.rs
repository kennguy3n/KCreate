//! Image-generation HTTP client.
//!
//! Phase 12 Block B replaced the Python diffusion sidecar with
//! `sd-server` (stable-diffusion.cpp). The process lifecycle now
//! lives in [`crate::diffusion_sidecar`]; this module is the
//! thin HTTP client the bridge uses to drive that sidecar.
//!
//! sd-server exposes three API surfaces on its loopback listener:
//!
//! * `POST /sdapi/v1/txt2img`     — Automatic1111-compatible
//!   text-to-image endpoint. Request body:
//!   ```json
//!   { "prompt": "...", "width": 1024, "height": 1024,
//!     "steps": 20, "seed": 42 }
//!   ```
//!   Response body:
//!   ```json
//!   { "images": ["<base64 PNG>"], ... }
//!   ```
//! * `POST /v1/images/generations` — OpenAI Images-compatible
//!   endpoint. We don't use it here; the A1111 shape matches our
//!   internal request shape one-for-one.
//! * `GET  /sdcpp/v1/capabilities` — readiness probe used by the
//!   supervisor (not invoked from this module).
//!
//! Local-first invariant: this module sits in `kcreate_ai` (not the
//! editing path), and its `ureq` dependency is feature-gated behind
//! `llm_sidecar` just like the chat client. The editing-path
//! `local_first.rs` deny-list test stays green.

// Only the `llm_sidecar`-feature build (which pulls in `ureq`)
// actually uses `Duration` here for the HTTP-client read timeout —
// without the feature flag the import is unused and cargo warns.
#[cfg(feature = "llm_sidecar")]
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;

// ---- Generation client ----

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

/// Request payload for `POST /sdapi/v1/txt2img`. The field set is a
/// subset of A1111's full request schema — sd-server happily
/// ignores unknown fields and supplies defaults for absent ones, so
/// the bridge only needs to surface the knobs the UI exposes.
///
/// `serde` defaults keep the wire shape backwards-compatible with
/// the historical Python sidecar request — `seed: None` is omitted
/// from the JSON entirely, letting sd-server pick a random one.
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

/// Wire shape of the A1111-compatible response from
/// `POST /sdapi/v1/txt2img`. We pull only the `images` array — the
/// rest of the response (`parameters`, `info`) is metadata we don't
/// surface to the renderer.
#[cfg(feature = "llm_sidecar")]
#[derive(Debug, Clone, Deserialize)]
struct A1111Response {
    images: Vec<String>,
}

/// POST a generation request to the local sd-server and return
/// decoded RGBA pixels. Network access is loopback-only (the URL is
/// hard-coded to `127.0.0.1:<port>`).
#[cfg(feature = "llm_sidecar")]
pub fn generate_image(port: u16, req: &ImageGenRequest) -> ImageGenResult<GeneratedImage> {
    let url = format!("http://127.0.0.1:{port}/sdapi/v1/txt2img");
    let body = serde_json::to_string(req).map_err(|e| ImageGenError::Decode(e.to_string()))?;
    let resp = ureq::post(&url)
        // Generations can take 10–60 s on a warm pipeline; bump
        // the timeout well above what `llm_chat` uses.
        .timeout(Duration::from_mins(10))
        .set("Content-Type", "application/json")
        .send_string(&body)
        .map_err(map_ureq_error)?;
    // ureq 2.x surfaces 4xx/5xx as `Err(ureq::Error::Status)` so
    // reaching `Ok(resp)` already guarantees a 2xx — but sd-server
    // could in principle return `201 Created` or a `204 No Content`
    // on a future endpoint shape, and a strict `== 200` check would
    // then swallow the response body as an empty JSON parse error.
    // Accept any 2xx instead; treat the (presently unreachable)
    // non-2xx success codes as protocol errors with the status
    // preserved.
    let status = resp.status();
    if !(200..300).contains(&status) {
        let body = resp.into_string().unwrap_or_default();
        return Err(ImageGenError::Status { status, body });
    }
    let parsed: A1111Response = resp
        .into_json()
        .map_err(|e| ImageGenError::Decode(e.to_string()))?;
    let first = parsed
        .images
        .into_iter()
        .next()
        .ok_or_else(|| ImageGenError::Decode("sd-server returned empty `images` array".into()))?;
    // The HTTP path uses the lenient variant so a future model pack
    // whose architecture rounds dimensions (e.g. SDXL's multiple-of-8
    // requirement) doesn't get rejected at the decode boundary —
    // sd-server is authoritative on the actual output resolution and
    // the renderer is happy to display whatever pixels came back.
    // The strict variant remains for tests + callers that want a
    // hard equality check against a known-honored resolution.
    decode_png_payload_lenient(&first)
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

/// Decode a base64-encoded PNG into RGBA8 pixels and require the
/// result to match `expected_width` / `expected_height` exactly.
///
/// Use this when the caller knows the model's architecture honors
/// the requested resolution byte-for-byte (FLUX does; SDXL
/// internally rounds to multiples of 8). The HTTP path in
/// [`generate_image`] uses [`decode_png_payload_lenient`] instead
/// so future non-FLUX packs don't get rejected at the decode
/// boundary for what is actually correct server behavior.
///
/// sd-server occasionally prefixes its base64 payloads with a
/// `data:image/png;base64,` data URI header in some build configs;
/// the prefix is stripped transparently so the bridge doesn't have
/// to branch on which build it's talking to.
pub fn decode_png_payload(
    base64_png: &str,
    expected_width: u32,
    expected_height: u32,
) -> ImageGenResult<GeneratedImage> {
    let img = decode_png_payload_lenient(base64_png)?;
    if img.width != expected_width || img.height != expected_height {
        return Err(ImageGenError::Decode(format!(
            "server returned {}x{}, expected {expected_width}x{expected_height}",
            img.width, img.height,
        )));
    }
    Ok(img)
}

/// Decode a base64-encoded PNG into RGBA8 pixels and report the
/// dimensions the server actually produced. No equality check
/// against any client-side expectation — the caller decides what
/// (if anything) to do with the dimensions.
///
/// This is the HTTP-path decoder. It exists so [`generate_image`]
/// remains valid when a future model pack (e.g. SDXL) legitimately
/// rounds the output resolution to satisfy an architectural
/// constraint. The strict variant ([`decode_png_payload`]) is kept
/// for tests + callers that want to assert server fidelity.
pub fn decode_png_payload_lenient(base64_png: &str) -> ImageGenResult<GeneratedImage> {
    use base64::Engine as _;
    let trimmed = base64_png
        .strip_prefix("data:image/png;base64,")
        .unwrap_or(base64_png)
        .trim();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(trimmed.as_bytes())
        .map_err(|e| ImageGenError::Base64(e.to_string()))?;
    let img = image::load_from_memory(&bytes).map_err(|e| ImageGenError::Png(e.to_string()))?;
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width(), rgba.height());
    Ok(GeneratedImage {
        rgba: rgba.into_raw(),
        width: w,
        height: h,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// Some sd-server builds prefix the payload with a data-URI
    /// header. The decoder must strip it transparently so the
    /// bridge isn't sensitive to which build is loaded.
    #[test]
    fn decode_png_payload_strips_data_uri_prefix() {
        use base64::Engine as _;
        let pixels: Vec<u8> = vec![0, 0, 0, 255, 255, 255, 255, 255, 128, 0, 128, 255, 0, 128, 0, 255];
        let img: image::RgbaImage = image::ImageBuffer::from_raw(2, 2, pixels.clone()).unwrap();
        let mut png_bytes: Vec<u8> = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(
                &mut std::io::Cursor::new(&mut png_bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png_bytes);
        let with_prefix = format!("data:image/png;base64,{b64}");
        let decoded = decode_png_payload(&with_prefix, 2, 2).unwrap();
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

    /// Empty input surfaces as a `Base64` error (an empty string is
    /// valid base64 but decodes to zero bytes, which the image
    /// decoder then rejects with an `Unsupported` error — keep the
    /// surface message tight so the UI can show a clean toast).
    #[test]
    fn decode_png_payload_rejects_empty_string() {
        let err = decode_png_payload("", 1, 1).expect_err("empty");
        assert!(matches!(err, ImageGenError::Png(_)));
    }

    /// The lenient decoder reports whatever dimensions the server
    /// actually produced — no expected-size argument, no rejection.
    /// This is the variant the HTTP path uses so future packs whose
    /// architecture rounds dimensions (e.g. SDXL's multiple-of-8
    /// rule) don't trip a false-positive decode error.
    #[test]
    fn decode_png_payload_lenient_returns_actual_dimensions() {
        use base64::Engine as _;
        // Server "returned" a 16x16 image when the request asked
        // for 17x17 (simulating SDXL's rounding behavior). The
        // strict variant would reject this; the lenient one must
        // accept it and report 16x16.
        let pixels: Vec<u8> = vec![42u8; 16 * 16 * 4];
        let img: image::RgbaImage = image::ImageBuffer::from_raw(16, 16, pixels).unwrap();
        let mut png_bytes: Vec<u8> = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(
                &mut std::io::Cursor::new(&mut png_bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png_bytes);
        let lenient = decode_png_payload_lenient(&b64).expect("lenient must accept actual dims");
        assert_eq!(lenient.width, 16);
        assert_eq!(lenient.height, 16);
        assert_eq!(lenient.rgba.len(), 16 * 16 * 4);
        // Same payload through the strict variant must fail when
        // the expectation doesn't match — this is the contract
        // difference the split exists for.
        let err =
            decode_png_payload(&b64, 17, 17).expect_err("strict must reject mismatched dims");
        assert!(matches!(err, ImageGenError::Decode(_)));
    }

    /// The lenient decoder also strips the `data:image/png;base64,`
    /// data-URI prefix so the HTTP path doesn't have to peek at
    /// which sd-server build it's talking to.
    #[test]
    fn decode_png_payload_lenient_strips_data_uri_prefix() {
        use base64::Engine as _;
        let pixels: Vec<u8> = vec![1, 2, 3, 255];
        let img: image::RgbaImage = image::ImageBuffer::from_raw(1, 1, pixels.clone()).unwrap();
        let mut png_bytes: Vec<u8> = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(
                &mut std::io::Cursor::new(&mut png_bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png_bytes);
        let with_prefix = format!("data:image/png;base64,{b64}");
        let lenient = decode_png_payload_lenient(&with_prefix).unwrap();
        assert_eq!(lenient.width, 1);
        assert_eq!(lenient.height, 1);
        assert_eq!(lenient.rgba, pixels);
    }

    /// Mock sd-server with `tiny_http` and round-trip a real
    /// generate request. Confirms the A1111 `/sdapi/v1/txt2img`
    /// wire format the bridge expects.
    #[cfg(feature = "llm_sidecar")]
    #[test]
    fn generate_image_round_trip_against_sd_server_mock() {
        use base64::Engine as _;
        let server = tiny_http::Server::http("127.0.0.1:0").expect("mock server");
        let port = server.server_addr().to_ip().expect("ip addr").port();
        // Background thread that responds to /sdapi/v1/txt2img with
        // a base64 PNG inside an `images: [...]` array.
        let handle = std::thread::spawn(move || {
            for req in server.incoming_requests().take(1) {
                assert_eq!(req.url(), "/sdapi/v1/txt2img");
                let pixels: Vec<u8> = vec![
                    10, 20, 30, 255, 40, 50, 60, 255, 70, 80, 90, 255, 100, 110, 120, 255,
                ];
                let img: image::RgbaImage =
                    image::ImageBuffer::from_raw(2, 2, pixels).unwrap();
                let mut png_bytes: Vec<u8> = Vec::new();
                image::DynamicImage::ImageRgba8(img)
                    .write_to(
                        &mut std::io::Cursor::new(&mut png_bytes),
                        image::ImageFormat::Png,
                    )
                    .unwrap();
                let b64 = base64::engine::general_purpose::STANDARD.encode(&png_bytes);
                let body = format!(r#"{{"images":["{b64}"]}}"#);
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

    /// An empty `images` array must surface as `Decode`, not a
    /// silent zero-pixel image. Guards against an sd-server
    /// configuration that returns a response shell without the
    /// payload (e.g., a model-load failure that returns 200 + an
    /// empty array).
    #[cfg(feature = "llm_sidecar")]
    #[test]
    fn generate_image_rejects_empty_images_array() {
        let server = tiny_http::Server::http("127.0.0.1:0").expect("mock server");
        let port = server.server_addr().to_ip().expect("ip addr").port();
        let handle = std::thread::spawn(move || {
            for req in server.incoming_requests().take(1) {
                let body = r#"{"images":[]}"#;
                let resp = tiny_http::Response::from_string(body).with_header(
                    "Content-Type: application/json"
                        .parse::<tiny_http::Header>()
                        .unwrap(),
                );
                let _ = req.respond(resp);
            }
        });
        let req = ImageGenRequest {
            prompt: "anything".into(),
            width: 64,
            height: 64,
            steps: 4,
            seed: None,
        };
        let err = generate_image(port, &req).expect_err("empty images");
        assert!(matches!(err, ImageGenError::Decode(_)));
        let _ = handle.join();
    }
}
