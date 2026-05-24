//! Design-token suggestion via VLM (Task 19).
//!
//! Sends an artboard snapshot to the vision sidecar and asks for a
//! starter design-token set: spacing scale, color palette, and
//! typography ramp. Returned as a [`DesignTokenSuggestion`] under
//! a GBNF grammar constraint so the JSON is always parseable.
//!
//! This complements the heuristic palette extraction in
//! `crate::palette` — palette pulls colors statistically from the
//! pixel histogram, this module asks the VLM for the *intentional*
//! palette plus the spacing / type scale that should accompany it.

use serde::{Deserialize, Serialize};

use crate::vision_chat::{describe_image_with_grammar, VisionChatError, VisionChatResult};

/// VLM-generated design-token suggestion.
///
/// Wire-format lockstep: mirrored in
/// `apps/desktop/shared/scene.ts::DesignTokenSuggestion`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DesignTokenSuggestion {
    /// Spacing scale in CSS px, small-to-large (e.g. 4, 8, 16, 24, 32).
    pub spacing: Vec<f32>,
    /// Color tokens, 6-digit hex with leading `#`. Order is
    /// "primary, secondary, accent, neutral-1, neutral-2, ...".
    pub colors: Vec<String>,
    /// Typography ramp — entries shaped like "h1 32 / 1.2".
    pub typography: Vec<String>,
}

/// Suggest a design-token set for the given artboard image.
pub fn suggest_design_tokens(
    port: u16,
    rgba: &[u8],
    width: u32,
    height: u32,
) -> VisionChatResult<DesignTokenSuggestion> {
    let raw = describe_image_with_grammar(
        port,
        SYSTEM_PROMPT,
        USER_PROMPT,
        rgba,
        width,
        height,
        DESIGN_TOKEN_GRAMMAR,
        384,
    )?;
    parse_design_token_suggestion(&raw)
}

/// Parse a JSON string into [`DesignTokenSuggestion`].
pub fn parse_design_token_suggestion(json: &str) -> VisionChatResult<DesignTokenSuggestion> {
    serde_json::from_str::<DesignTokenSuggestion>(json)
        .map_err(|e| VisionChatError::Chat(crate::llm_chat::ChatError::Decode(e.to_string())))
}

const SYSTEM_PROMPT: &str = "You are a senior design-systems engineer. \
    Recommend a starter design-token set for the screen in the image. \
    Use only the JSON shape specified by the grammar. Colors are \
    6-digit lowercase hex with a leading '#'. Spacing values are CSS \
    px floats. Typography entries are short labels like 'h1 32 / 1.2'.";

const USER_PROMPT: &str = "Suggest a design-token set as JSON with: \
    spacing (px floats, small to large), colors (hex, primary first), \
    typography (size/leading labels, largest first).";

/// GBNF grammar for the [`DesignTokenSuggestion`] JSON shape.
///
/// Portability note: see `brand_extract::BRAND_EXTRACTION_GRAMMAR`
/// — array length caps use `(item)?` chains instead of `{0,N}`
/// bounded repetition so the grammar parses on every known GBNF
/// consumer. The 8-item arrays (1 head + 7 tails) cap each list
/// at 8 entries, matching the previous `(ws "," ws item){0,7}`
/// behaviour exactly.
pub const DESIGN_TOKEN_GRAMMAR: &str = r##"
root          ::= "{" ws "\"spacing\"" ws ":" ws number-array ws "," ws
                  "\"colors\"" ws ":" ws color-array ws "," ws
                  "\"typography\"" ws ":" ws string-array ws "}"

number-array  ::= "[" ws (number (ws "," ws number)? (ws "," ws number)? (ws "," ws number)? (ws "," ws number)? (ws "," ws number)? (ws "," ws number)? (ws "," ws number)?)? ws "]"
number        ::= "-"? ("0" | [1-9] [0-9]? [0-9]? [0-9]?) ("." [0-9] [0-9]?)?

color-array   ::= "[" ws (color (ws "," ws color)? (ws "," ws color)? (ws "," ws color)? (ws "," ws color)? (ws "," ws color)? (ws "," ws color)? (ws "," ws color)?)? ws "]"
color         ::= "\"#" hex hex hex hex hex hex "\""
hex           ::= [0-9a-f]

string-array  ::= "[" ws (string (ws "," ws string)? (ws "," ws string)? (ws "," ws string)? (ws "," ws string)? (ws "," ws string)? (ws "," ws string)? (ws "," ws string)?)? ws "]"
string        ::= "\"" ([^"\\] | "\\" .)+ "\""

ws            ::= [ \t\n]*
"##;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_round_trips_canonical_shape() {
        let raw = r##"{
            "spacing": [4.0, 8.0, 16.0, 24.0],
            "colors": ["#1f2937", "#4f46e5"],
            "typography": ["h1 32 / 1.2", "body 16 / 1.5"]
        }"##;
        let d = parse_design_token_suggestion(raw).unwrap();
        assert_eq!(d.spacing.len(), 4);
        assert_eq!(d.colors[0], "#1f2937");
        assert_eq!(d.typography[1], "body 16 / 1.5");
    }

    #[test]
    fn grammar_covers_all_fields() {
        for field in ["spacing", "colors", "typography"] {
            assert!(
                DESIGN_TOKEN_GRAMMAR.contains(field),
                "grammar missing field `{field}`",
            );
        }
    }
}
