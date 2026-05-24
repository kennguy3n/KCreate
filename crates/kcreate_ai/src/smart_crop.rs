//! Content-aware crop suggestion via VLM (Task 18).
//!
//! Sends an image to the vision sidecar and asks for a normalized
//! bounding-box around the most visually important subject of the
//! image, with a target aspect ratio. The VLM's reply is constrained
//! by a GBNF grammar so the output is always a valid JSON object.
//!
//! The returned rectangle is expressed in normalized image
//! coordinates (`0.0..=1.0`) so callers can apply the suggestion to
//! the original full-resolution image without round-tripping the
//! VLM's downscaled view.

use serde::{Deserialize, Serialize};

use crate::vision_chat::{describe_image_with_grammar, VisionChatError, VisionChatResult};

/// A normalized crop rectangle (`0.0..=1.0` on both axes).
///
/// Wire-format lockstep: mirrored in
/// `apps/desktop/shared/scene.ts::CropSuggestion`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CropSuggestion {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// VLM-provided confidence in the suggestion, `0.0..=1.0`.
    pub confidence: f32,
}

impl CropSuggestion {
    /// Convert the normalized box back into integer pixel coords for
    /// an image of `width` × `height`. Clamps so we never produce
    /// out-of-bounds coordinates even if the VLM reported values
    /// slightly outside `[0, 1]`.
    #[must_use]
    pub fn to_pixels(&self, width: u32, height: u32) -> (u32, u32, u32, u32) {
        let clamp = |v: f32| v.clamp(0.0, 1.0);
        let x_raw = (clamp(self.x) * width as f32).round() as u32;
        let y_raw = (clamp(self.y) * height as f32).round() as u32;
        let w_raw = (clamp(self.w) * width as f32).round() as u32;
        let h_raw = (clamp(self.h) * height as f32).round() as u32;
        // Clamp the top-left first so we never report a pixel
        // outside the image. Then clamp the size against the
        // *remaining* room from that top-left — otherwise an
        // out-of-range `y` like 1.2 collapses the height to 0
        // because we'd be measuring from the un-clamped origin.
        let x = x_raw.min(width.saturating_sub(1));
        let y = y_raw.min(height.saturating_sub(1));
        let w = w_raw.min(width.saturating_sub(x));
        let h = h_raw.min(height.saturating_sub(y));
        (x, y, w, h)
    }
}

/// Suggest a crop for `rgba`. `aspect_ratio` is the desired
/// width/height ratio (`None` = let the VLM pick).
pub fn suggest_crop(
    port: u16,
    rgba: &[u8],
    width: u32,
    height: u32,
    aspect_ratio: Option<f32>,
) -> VisionChatResult<CropSuggestion> {
    let user_prompt = match aspect_ratio {
        Some(a) if a > 0.0 => format!(
            "Suggest a content-aware crop with width/height aspect ratio of {a:.3}. \
             Centre the most important subject. Respect rule-of-thirds when possible. \
             Return JSON with normalized x,y,w,h in [0,1] and a confidence in [0,1]."
        ),
        _ => "Suggest a content-aware crop. Centre the most important subject. \
              Respect rule-of-thirds when possible. Return JSON with normalized \
              x,y,w,h in [0,1] and a confidence in [0,1]."
            .to_string(),
    };
    let raw = describe_image_with_grammar(
        port,
        SYSTEM_PROMPT,
        &user_prompt,
        rgba,
        width,
        height,
        CROP_GRAMMAR,
        128,
    )?;
    parse_crop_suggestion(&raw)
}

/// Parse a JSON string into [`CropSuggestion`].
pub fn parse_crop_suggestion(json: &str) -> VisionChatResult<CropSuggestion> {
    serde_json::from_str::<CropSuggestion>(json)
        .map_err(|e| VisionChatError::Chat(crate::llm_chat::ChatError::Decode(e.to_string())))
}

const SYSTEM_PROMPT: &str = "You are an image-cropping assistant. \
    Identify the subject and return a tight crop. Use only the JSON \
    shape specified by the grammar. All numbers are floats in [0,1].";

/// GBNF grammar that constrains the VLM's reply to a JSON object
/// of the [`CropSuggestion`] shape.
///
/// Portability note: see `brand_extract::BRAND_EXTRACTION_GRAMMAR`
/// — bounded repetition `[0-9]{1,4}` is replaced with an explicit
/// optional-digit chain so the grammar parses on every known GBNF
/// consumer (including parsers that only implement `*`, `+`, and
/// `?`).
pub const CROP_GRAMMAR: &str = r#"
root       ::= "{" ws "\"x\"" ws ":" ws number ws "," ws
               "\"y\"" ws ":" ws number ws "," ws
               "\"w\"" ws ":" ws number ws "," ws
               "\"h\"" ws ":" ws number ws "," ws
               "\"confidence\"" ws ":" ws number ws "}"

number ::= ("0" frac? | "1" frac?)
frac   ::= "." [0-9] [0-9]? [0-9]? [0-9]?
ws     ::= [ \t\n]*
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_round_trips_canonical_shape() {
        let raw = r#"{"x":0.1,"y":0.2,"w":0.6,"h":0.4,"confidence":0.85}"#;
        let c = parse_crop_suggestion(raw).unwrap();
        assert!((c.x - 0.1).abs() < 1e-6);
        assert!((c.confidence - 0.85).abs() < 1e-6);
    }

    #[test]
    fn to_pixels_clamps_into_image_bounds() {
        let c = CropSuggestion {
            x: 0.5,
            y: 0.5,
            w: 0.6,
            h: 0.6,
            confidence: 1.0,
        };
        let (x, y, w, h) = c.to_pixels(100, 100);
        assert_eq!(x, 50);
        assert_eq!(y, 50);
        // x + w must not exceed 100 (image width).
        assert!(x + w <= 100);
        assert!(y + h <= 100);
    }

    #[test]
    fn to_pixels_clamps_out_of_range_coords() {
        let c = CropSuggestion {
            x: -0.1,
            y: 1.2,
            w: 2.0,
            h: 2.0,
            confidence: 0.5,
        };
        let (x, y, w, h) = c.to_pixels(100, 100);
        assert_eq!(x, 0);
        assert_eq!(y, 99);
        assert_eq!(w, 100);
        assert_eq!(h, 1);
    }

    #[test]
    fn grammar_covers_all_fields() {
        for field in ["x", "y", "w", "h", "confidence"] {
            assert!(
                CROP_GRAMMAR.contains(field),
                "grammar missing field `{field}`",
            );
        }
    }
}
