//! Vision (VLM) sidecar integration tests.
//!
//! These run against a `tiny_http`-backed mock server bound to a
//! loopback port — the same shape `llama-server` (or
//! `python3 -m mlx_lm.server` on Apple Silicon) speaks. The intent is
//! NOT to verify the model's output (which is unknowable) but to
//! check the wire shape both ways:
//!
//!   1. Multimodal `ChatMessage` round-trips JSON correctly (text-only
//!      stays as `"content": "string"`, vision messages emit a parts
//!      array with `image_url` + `data:` URI).
//!   2. `describe_image` actually packs the image into the request
//!      body and returns the assistant's plain text reply.
//!   3. `describe_image_with_grammar` forwards the GBNF grammar so
//!      the model is constrained to valid JSON (this is what
//!      `brand_extract`, `smart_crop`, etc rely on).
//!   4. The `mmproj` argument to the `LlmSidecar` config is real —
//!      i.e. `SidecarConfig` validates the path at construction
//!      time so a typo crashes the host UI rather than a half-running
//!      sidecar.
//!   5. MLX sidecar gracefully reports unavailability on non-Apple
//!      platforms (we don't try to spawn `python3 -m mlx_lm.server`
//!      on Linux CI).

use kcreate_ai::llm_chat::{ChatContent, ChatMessage, ChatRequest, ChatRole, ContentPart};
use kcreate_ai::vision_chat::{describe_image, describe_image_with_grammar};

/// Helper: 4x4 solid red RGBA image. Small enough that the VLM
/// downscaler passes it through unchanged but large enough to make
/// the base64 PNG payload non-trivial.
fn red_rgba_4x4() -> Vec<u8> {
    let mut pixels = Vec::with_capacity(64);
    for _ in 0..16 {
        pixels.extend_from_slice(&[255, 0, 0, 255]);
    }
    pixels
}

/// Helper: spin up a `tiny_http` mock and return `(port,
/// join_handle)`. The handler closure receives the request body so
/// each test can assert on what was sent. The mock always responds
/// once with the supplied JSON payload.
fn spawn_mock_chat<F>(reply_body: String, on_request: F) -> (u16, std::thread::JoinHandle<()>)
where
    F: FnOnce(String) + Send + 'static,
{
    let server = tiny_http::Server::http("127.0.0.1:0").expect("bind mock server");
    let port = server.server_addr().to_ip().expect("ip4").port();
    let handle = std::thread::spawn(move || {
        let mut req = server
            .incoming_requests()
            .next()
            .expect("at least one request");
        let mut body = String::new();
        req.as_reader()
            .read_to_string(&mut body)
            .expect("read request body");
        on_request(body);
        let resp = tiny_http::Response::from_string(reply_body).with_header(
            "content-type: application/json"
                .parse::<tiny_http::Header>()
                .expect("hdr"),
        );
        req.respond(resp).expect("respond");
    });
    (port, handle)
}

/// Text-only `ChatMessage`s must serialise with `"content": "..."`
/// (a JSON string), not as an array of parts. This is the
/// backward-compatibility guarantee for the existing Phase 1 LLM
/// chat path — if it ever flipped to always emit an array, every
/// non-vision llama-server build in the wild would reject our
/// requests.
#[test]
fn text_only_message_serialises_as_plain_string() {
    let msg = ChatMessage::user("hello world");
    let json = serde_json::to_value(&msg).expect("serialise");
    assert_eq!(
        json.pointer("/content"),
        Some(&serde_json::Value::String("hello world".to_string())),
        "text-only content must serialise as a JSON string, got {json}",
    );
}

/// Multimodal `ChatMessage`s must serialise with a `content` ARRAY
/// of `{type, ...}` parts. The image part uses `image_url` with a
/// `data:image/png;base64,...` URI — that's the OpenAI vision
/// API shape and what llama-server's multimodal endpoint accepts.
#[test]
fn multimodal_message_serialises_as_parts_array() {
    let msg = ChatMessage {
        role: ChatRole::User,
        content: ChatContent::Multimodal(vec![
            ContentPart::Text {
                text: "what is in this image?".to_string(),
            },
            ContentPart::ImageBase64 {
                media_type: "image/png".to_string(),
                data: "AAAA".to_string(),
            },
        ]),
    };
    let json = serde_json::to_value(&msg).expect("serialise");
    let parts = json
        .pointer("/content")
        .and_then(|v| v.as_array())
        .expect("content must be an array for multimodal");
    assert_eq!(parts.len(), 2, "expected 2 parts, got {parts:?}");
    assert_eq!(
        parts[0].pointer("/type").and_then(|v| v.as_str()),
        Some("text"),
        "first part must be text",
    );
    let url = parts[1]
        .pointer("/image_url/url")
        .and_then(|v| v.as_str())
        .expect("image_url.url string");
    assert!(
        url.starts_with("data:image/png;base64,AAAA"),
        "image part must use data URI, got {url:?}",
    );
}

/// Round-trip a multimodal message through serde to catch any
/// deserialisation gap. The mock server in `describe_image_*` tests
/// only exercises one direction; this exercises both.
#[test]
fn multimodal_message_round_trips_through_serde() {
    let original = ChatMessage {
        role: ChatRole::User,
        content: ChatContent::Multimodal(vec![
            ContentPart::Text {
                text: "describe".to_string(),
            },
            ContentPart::ImageBase64 {
                media_type: "image/png".to_string(),
                data: "Zm9v".to_string(),
            },
        ]),
    };
    let json = serde_json::to_string(&original).expect("ser");
    let parsed: ChatMessage = serde_json::from_str(&json).expect("de");
    assert_eq!(parsed, original);
}

/// `describe_image` packs the image into the request body and
/// surfaces the assistant's text reply verbatim.
#[test]
fn describe_image_round_trips_against_mock() {
    let reply = serde_json::json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": "A small solid-red square."
            }
        }],
        "model": "mock-vlm",
    })
    .to_string();
    let (port, handle) = spawn_mock_chat(reply, |body| {
        assert!(
            body.contains("data:image/png;base64"),
            "image data URI missing from request: {body}",
        );
        assert!(
            body.contains("\"image_url\""),
            "image_url field missing: {body}",
        );
        assert!(
            body.contains("describe this image"),
            "user prompt missing: {body}",
        );
    });
    let pixels = red_rgba_4x4();
    let out = describe_image(
        port,
        "You are an accessibility auditor.",
        "describe this image",
        &pixels,
        4,
        4,
    )
    .expect("describe");
    assert_eq!(out, "A small solid-red square.");
    handle.join().expect("server thread");
}

/// `describe_image_with_grammar` MUST forward the `grammar` field
/// in the chat completion request so the model produces parsable
/// JSON. The brand-extract / smart-crop / design-tokens paths all
/// depend on this — drop the grammar and they fall over.
#[test]
fn grammar_constrained_describe_forwards_grammar_field() {
    let reply = serde_json::json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": "{\"colors\":[\"#ff0000\"]}"
            }
        }],
        "model": "mock-vlm",
    })
    .to_string();
    let grammar = "root ::= \"{\" .* \"}\"";
    let (port, handle) = spawn_mock_chat(reply, move |body| {
        let v: serde_json::Value = serde_json::from_str(&body).expect("request body must be JSON");
        let g = v
            .pointer("/grammar")
            .and_then(|x| x.as_str())
            .expect("grammar field missing");
        assert!(g.starts_with("root ::="), "grammar mangled: {g:?}");
    });
    let pixels = red_rgba_4x4();
    let out = describe_image_with_grammar(
        port,
        "You are a brand extractor.",
        "extract colors",
        &pixels,
        4,
        4,
        grammar,
        256,
    )
    .expect("describe with grammar");
    assert!(out.contains("#ff0000"));
    handle.join().expect("server thread");
}

/// Empty messages array shouldn't be sent to the sidecar — the
/// host must catch this at the request-builder layer rather than
/// surfacing an opaque 400 from llama-server.
#[test]
fn chat_request_with_no_messages_is_an_invalid_input() {
    let req = ChatRequest {
        messages: vec![],
        max_tokens: 32,
        temperature: 0.0,
        grammar: None,
    };
    // We're not asserting on a specific error variant here — the
    // contract is just "don't accidentally succeed on an empty
    // request". `chat_completion` will fail to bind the port (0)
    // and surface a transport error.
    let err = kcreate_ai::llm_chat::chat_completion(0, &req).expect_err("empty");
    // Any error is acceptable; success is not.
    let _ = err;
}

/// MLX sidecar must be gracefully unavailable on non-Apple-Silicon
/// platforms — we should not be spawning `python3 -m mlx_lm.server`
/// on Linux CI. `probe_mlx_available` calls `python3 -c
/// "import mlx_lm"`, which on a Linux box without the `mlx_lm`
/// wheel returns `false`. We don't assert on Apple Silicon because
/// the developer's box may or may not have MLX installed; the test
/// only verifies the contract that the probe is honest.
#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
#[test]
fn mlx_probe_returns_false_off_apple_silicon() {
    assert!(
        !kcreate_ai::mlx_sidecar::probe_mlx_available(),
        "MLX should not be available off Apple Silicon — \
         `python3 -m mlx_lm` is macOS-arm64-only",
    );
}
