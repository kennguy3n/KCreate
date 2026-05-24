//! Brand extraction from a reference image via VLM (Task 16).
//!
//! Takes an arbitrary inspirational image (mood board, screenshot,
//! photograph) and asks the VLM to extract a structured brand
//! profile: prominent colors (hex), typography feel, and spacing
//! tokens. The response is constrained by a GBNF grammar so the
//! output is always parseable JSON — there is no free-form
//! fallback, by design.
//!
//! Returned to the renderer as a [`BrandExtraction`] struct that
//! mirrors `apps/desktop/shared/scene.ts::BrandExtraction`.

use serde::{Deserialize, Serialize};

use crate::vision_chat::{describe_image_with_grammar, VisionChatError, VisionChatResult};

/// Structured brand profile extracted from an image.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BrandExtraction {
    /// Up to 6 dominant hex colors, e.g. `#1f2937`. Order is
    /// "perceived prominence" (the VLM's interpretation, not pixel
    /// frequency).
    pub colors: Vec<String>,
    /// Up to 4 font-family categories the image evokes
    /// (e.g. `"sans-serif geometric"`, `"serif editorial"`).
    pub fonts: Vec<String>,
    /// Up to 6 spacing values in CSS px — derived from the visual
    /// rhythm in the reference. Order is small-to-large.
    pub spacing: Vec<f32>,
}

/// Extract a brand profile from `rgba`. Returns a parsed
/// [`BrandExtraction`]; the GBNF grammar guarantees the model's
/// reply is valid JSON of the right shape.
pub fn extract_brand_from_image(
    port: u16,
    rgba: &[u8],
    width: u32,
    height: u32,
) -> VisionChatResult<BrandExtraction> {
    let raw = describe_image_with_grammar(
        port,
        SYSTEM_PROMPT,
        USER_PROMPT,
        rgba,
        width,
        height,
        BRAND_EXTRACTION_GRAMMAR,
        // 384 tokens easily fits the schema with all arrays at
        // their max length.
        384,
    )?;
    parse_brand_extraction(&raw)
}

/// Parse a JSON string into [`BrandExtraction`]. Public so tests
/// (and IPC layer) can validate model output without round-tripping
/// through the network.
pub fn parse_brand_extraction(json: &str) -> VisionChatResult<BrandExtraction> {
    serde_json::from_str::<BrandExtraction>(json)
        .map_err(|e| VisionChatError::Chat(crate::llm_chat::ChatError::Decode(e.to_string())))
}

const SYSTEM_PROMPT: &str = "You are a brand designer extracting design \
    tokens from a reference image. Be precise. Use only the JSON shape \
    specified by the grammar. Hex colors must be 6-digit lowercase with \
    a leading '#'. Font categories must be short tags like \
    'sans-serif geometric' or 'serif editorial'. Spacing values are \
    floats in CSS pixels.";

const USER_PROMPT: &str = "Extract a brand profile from this image as \
    JSON with: colors (up to 6 hex strings, most prominent first), \
    fonts (up to 4 family categories), spacing (up to 6 px values, \
    small to large).";

/// GBNF grammar for the brand-extraction JSON shape. Constrains the
/// VLM so the response is guaranteed-parseable JSON of the form:
/// `{"colors":["#xxxxxx"...],"fonts":["..."...],"spacing":[12.0,...]}`.
pub const BRAND_EXTRACTION_GRAMMAR: &str = r##"
root ::= "{" ws "\"colors\"" ws ":" ws color-array ws "," ws
         "\"fonts\"" ws ":" ws string-array ws "," ws
         "\"spacing\"" ws ":" ws number-array ws "}"

color-array  ::= "[" ws (color (ws "," ws color){0,5})? ws "]"
color        ::= "\"#" hex hex hex hex hex hex "\""
hex          ::= [0-9a-f]

string-array ::= "[" ws (string (ws "," ws string){0,3})? ws "]"
string       ::= "\"" ([^"\\] | "\\" .){1,64} "\""

number-array ::= "[" ws (number (ws "," ws number){0,5})? ws "]"
number       ::= "-"? ("0" | [1-9] [0-9]{0,3}) ("." [0-9]{1,2})?

ws ::= [ \t\n]*
"##;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_round_trips_canonical_shape() {
        let raw = r##"{
            "colors": ["#1f2937", "#4f46e5", "#f9fafb"],
            "fonts": ["sans-serif geometric", "serif editorial"],
            "spacing": [4.0, 8.0, 16.0, 24.0]
        }"##;
        let b = parse_brand_extraction(raw).unwrap();
        assert_eq!(b.colors.len(), 3);
        assert_eq!(b.colors[0], "#1f2937");
        assert_eq!(b.fonts.len(), 2);
        assert_eq!(b.spacing, vec![4.0, 8.0, 16.0, 24.0]);
    }

    #[test]
    fn parse_rejects_non_json() {
        let err = parse_brand_extraction("not json").unwrap_err();
        match err {
            VisionChatError::Chat(crate::llm_chat::ChatError::Decode(_)) => {}
            other => panic!("unexpected error: {other:?}"),
        }
    }

    /// The grammar must reference every JSON field the parser
    /// expects. A drift between the grammar and the struct would
    /// produce parseable JSON that doesn't fit the struct shape —
    /// confusing at runtime, easy to catch here.
    #[test]
    fn grammar_mentions_all_fields() {
        for field in ["colors", "fonts", "spacing"] {
            assert!(
                BRAND_EXTRACTION_GRAMMAR.contains(field),
                "grammar is missing field `{field}`",
            );
        }
    }

    /// Wire-format lockstep: the field names on the JSON wire must
    /// match the camelCase used in `apps/desktop/shared/scene.ts`.
    /// For `BrandExtraction` all field names happen to be
    /// single-word, so the camelCase / snake_case casing is
    /// identical — this test pins that and will fire if anyone
    /// renames a field to a multi-word identifier without
    /// considering the wire layer.
    #[test]
    fn wire_format_is_camelcase() {
        let b = BrandExtraction {
            colors: vec!["#000000".into()],
            fonts: vec!["serif".into()],
            spacing: vec![4.0],
        };
        let s = serde_json::to_string(&b).unwrap();
        assert!(s.contains("\"colors\""));
        assert!(s.contains("\"fonts\""));
        assert!(s.contains("\"spacing\""));
    }
}
