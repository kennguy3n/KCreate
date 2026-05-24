//! VLM chat helpers — encode RGBA → PNG → base64, build a
//! multimodal [`crate::llm_chat::ChatRequest`], send it to the
//! sidecar, return the plain-text response.
//!
//! Every vision feature in Phase 4 (`alt_text`, `design_critique`,
//! `brand_extract`, `smart_crop`, `style_describe`,
//! `screenshot_to_layout::refine_with_vlm`, etc.) is implemented on
//! top of [`describe_image`] / [`describe_image_with_grammar`]. The
//! helpers handle:
//!
//! 1. **Image downscaling** to a reasonable resolution before
//!    base64 encoding. VLMs do not benefit from > 1024 px input
//!    (the projector resizes internally) and the bytes-over-IPC
//!    cost is meaningful, so we cap at 1024 px on the long edge.
//! 2. **PNG encoding** via the `image` crate. PNG is the only
//!    format every VLM accepts uniformly.
//! 3. **Base64 wrapping** with the `STANDARD` engine — matches the
//!    OpenAI vision spec's `data:` URI encoding.
//! 4. **Chat request construction** with the canonical
//!    `system + user(text+image)` message shape.
//!
//! Bridge-side callers should always go through this module rather
//! than building multimodal messages by hand — keeping the encoding
//! contract in one place is what lets us swap the underlying
//! sidecar (llama-server / mlx_lm.server) transparently.

use std::io::Cursor;

use base64::Engine as _;
use image::{ImageBuffer, ImageFormat, RgbaImage};
use thiserror::Error;

use crate::llm_chat::{
    chat_completion, ChatError, ChatMessage, ChatRequest, ChatRole, ContentPart,
};

/// Long-edge resolution we downscale images to before sending to a
/// VLM. SmolVLM uses 384px tiles; Qwen2.5-VL uses 448px tiles; both
/// happily accept up to 1024px and downsample internally. Sending
/// larger images costs IPC bandwidth without improving fidelity.
pub const MAX_VLM_LONG_EDGE_PX: u32 = 1024;

/// Errors from the vision-chat helpers.
#[derive(Debug, Error)]
pub enum VisionChatError {
    /// The input RGBA buffer wasn't a valid `width * height * 4`
    /// byte slice.
    #[error("invalid RGBA buffer: expected {expected} bytes, got {actual}")]
    InvalidRgba { expected: usize, actual: usize },
    /// PNG encoding failed (extremely unusual — typically allocator
    /// pressure on huge images).
    #[error("PNG encode error: {0}")]
    PngEncode(String),
    /// Downstream chat-completion error.
    #[error(transparent)]
    Chat(#[from] ChatError),
}

pub type VisionChatResult<T> = Result<T, VisionChatError>;

/// Resize `pixels` down so the long edge is at most
/// [`MAX_VLM_LONG_EDGE_PX`]. Returns a tuple of (resized RGBA,
/// new width, new height). If the image is already small enough,
/// the buffer is returned unmodified (no allocation).
pub fn downscale_for_vlm(
    pixels: &[u8],
    width: u32,
    height: u32,
) -> VisionChatResult<(Vec<u8>, u32, u32)> {
    let expected = (width as usize) * (height as usize) * 4;
    if pixels.len() != expected {
        return Err(VisionChatError::InvalidRgba {
            expected,
            actual: pixels.len(),
        });
    }
    let long = width.max(height);
    if long <= MAX_VLM_LONG_EDGE_PX {
        return Ok((pixels.to_vec(), width, height));
    }
    let scale = MAX_VLM_LONG_EDGE_PX as f32 / long as f32;
    let new_w = ((width as f32) * scale).round().max(1.0) as u32;
    let new_h = ((height as f32) * scale).round().max(1.0) as u32;
    let img: RgbaImage = ImageBuffer::from_raw(width, height, pixels.to_vec()).ok_or(
        VisionChatError::InvalidRgba {
            expected,
            actual: pixels.len(),
        },
    )?;
    let resized =
        image::imageops::resize(&img, new_w, new_h, image::imageops::FilterType::Lanczos3);
    Ok((resized.into_raw(), new_w, new_h))
}

/// Encode `pixels` (RGBA8) as a PNG byte buffer, then base64-encode
/// the bytes. Returns just the base64 payload — the caller (or
/// [`ContentPart::ImageBase64`]) prefixes the `data:image/png;base64,`
/// scheme.
pub fn rgba_to_base64_png(pixels: &[u8], width: u32, height: u32) -> VisionChatResult<String> {
    let img: RgbaImage =
        ImageBuffer::from_raw(width, height, pixels.to_vec()).ok_or_else(|| {
            VisionChatError::InvalidRgba {
                expected: (width as usize) * (height as usize) * 4,
                actual: pixels.len(),
            }
        })?;
    let mut buf: Vec<u8> = Vec::with_capacity(pixels.len() / 4);
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut Cursor::new(&mut buf), ImageFormat::Png)
        .map_err(|e| VisionChatError::PngEncode(e.to_string()))?;
    Ok(base64::engine::general_purpose::STANDARD.encode(&buf))
}

/// Build a vision chat request with a system prompt, a user prompt,
/// and one inline image. The image is downscaled to
/// [`MAX_VLM_LONG_EDGE_PX`] if needed before encoding.
pub fn build_vision_request(
    system_prompt: &str,
    user_prompt: &str,
    pixels: &[u8],
    width: u32,
    height: u32,
    max_tokens: usize,
    temperature: f32,
) -> VisionChatResult<ChatRequest> {
    let (small, w, h) = downscale_for_vlm(pixels, width, height)?;
    let b64 = rgba_to_base64_png(&small, w, h)?;
    let parts = vec![
        ContentPart::Text {
            text: user_prompt.to_string(),
        },
        ContentPart::ImageBase64 {
            media_type: "image/png".to_string(),
            data: b64,
        },
    ];
    Ok(ChatRequest {
        messages: vec![
            ChatMessage::system(system_prompt.to_string()),
            ChatMessage {
                role: ChatRole::User,
                content: crate::llm_chat::ChatContent::Multimodal(parts),
            },
        ],
        max_tokens,
        temperature,
        grammar: None,
    })
}

/// Describe an image. High-level convenience: builds the chat
/// request via [`build_vision_request`], sends it to the sidecar at
/// `port`, returns the plain-text reply.
///
/// `system_prompt` shapes the response domain (e.g. "You are an
/// accessibility auditor producing terse alt-text"). `user_prompt`
/// is the per-call ask.
pub fn describe_image(
    port: u16,
    system_prompt: &str,
    user_prompt: &str,
    pixels: &[u8],
    width: u32,
    height: u32,
) -> VisionChatResult<String> {
    let req = build_vision_request(
        system_prompt,
        user_prompt,
        pixels,
        width,
        height,
        // 512 tokens fits a few sentences of alt-text or a short
        // structured JSON object.
        512,
        0.2,
    )?;
    let resp = chat_completion(port, &req)?;
    Ok(resp.content)
}

/// Describe an image under a GBNF grammar constraint so the model's
/// output is guaranteed to parse as the requested JSON shape. Used
/// by `brand_extract`, `smart_crop`, `design_tokens_vlm`, etc.
#[allow(clippy::too_many_arguments)]
pub fn describe_image_with_grammar(
    port: u16,
    system_prompt: &str,
    user_prompt: &str,
    pixels: &[u8],
    width: u32,
    height: u32,
    grammar: &str,
    max_tokens: usize,
) -> VisionChatResult<String> {
    let mut req = build_vision_request(
        system_prompt,
        user_prompt,
        pixels,
        width,
        height,
        max_tokens,
        0.1,
    )?;
    req.grammar = Some(grammar.to_string());
    let resp = chat_completion(port, &req)?;
    Ok(resp.content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downscale_passes_small_images_through() {
        let pixels = vec![0u8; 32 * 32 * 4];
        let (out, w, h) = downscale_for_vlm(&pixels, 32, 32).unwrap();
        assert_eq!(w, 32);
        assert_eq!(h, 32);
        assert_eq!(out.len(), pixels.len());
    }

    #[test]
    fn downscale_clamps_long_edge() {
        // 2048x1024 input → long edge clamped to 1024, short edge halved.
        let pixels = vec![128u8; 2048 * 1024 * 4];
        let (_, w, h) = downscale_for_vlm(&pixels, 2048, 1024).unwrap();
        assert!(w <= MAX_VLM_LONG_EDGE_PX);
        assert!(h <= MAX_VLM_LONG_EDGE_PX);
        // Aspect ratio preserved within 1 px rounding.
        assert!((w as f32 / h as f32 - 2.0).abs() < 0.05);
    }

    #[test]
    fn downscale_rejects_short_buffer() {
        let err = downscale_for_vlm(&[0u8; 10], 32, 32).expect_err("short buffer");
        assert!(matches!(err, VisionChatError::InvalidRgba { .. }));
    }

    #[test]
    fn rgba_to_base64_png_round_trips() {
        // 2x2 red image → PNG → base64 → decode → same pixels.
        let pixels: Vec<u8> = vec![
            255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255,
        ];
        let b64 = rgba_to_base64_png(&pixels, 2, 2).unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&b64)
            .unwrap();
        let img = image::load_from_memory(&bytes).unwrap().to_rgba8();
        assert_eq!(img.width(), 2);
        assert_eq!(img.height(), 2);
        assert_eq!(img.as_raw(), &pixels);
    }

    #[test]
    fn build_request_includes_multimodal_user_part() {
        let pixels: Vec<u8> = vec![0u8; 4 * 4]; // 2x2
        let req = build_vision_request(
            "you are a vision assistant",
            "describe this image",
            &pixels,
            2,
            2,
            128,
            0.3,
        )
        .unwrap();
        assert_eq!(req.messages.len(), 2);
        assert!(req.messages[0].content.as_text().contains("vision"));
        assert!(req.messages[1].content.has_images());
    }
}
