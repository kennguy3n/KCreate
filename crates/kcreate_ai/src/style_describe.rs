//! Visual style description via VLM (Task 20).
//!
//! Sends a reference image to the vision sidecar and asks for a
//! short natural-language description of the visual style — color
//! mood, typography feel, layout rhythm, photography style. The
//! returned [`StyleDescription`] is also constrained by a GBNF
//! grammar so the JSON shape is stable.

use serde::{Deserialize, Serialize};

use crate::vision_chat::{describe_image_with_grammar, VisionChatError, VisionChatResult};

/// Structured style description.
///
/// Wire-format lockstep: mirrored in
/// `apps/desktop/shared/scene.ts::StyleDescription`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StyleDescription {
    /// 1–2 sentence overall summary.
    pub summary: String,
    /// Adjectives describing the color mood ("warm", "muted",
    /// "high contrast", ...).
    pub color_mood: Vec<String>,
    /// Adjectives describing the typography feel
    /// ("editorial serif", "geometric sans", ...).
    pub typography: Vec<String>,
    /// Adjectives describing the layout pattern ("grid-aligned",
    /// "asymmetric", "centered hero", ...).
    pub layout: Vec<String>,
}

/// Describe the visual style of `rgba`.
pub fn describe_style(
    port: u16,
    rgba: &[u8],
    width: u32,
    height: u32,
) -> VisionChatResult<StyleDescription> {
    let raw = describe_image_with_grammar(
        port,
        SYSTEM_PROMPT,
        USER_PROMPT,
        rgba,
        width,
        height,
        STYLE_GRAMMAR,
        512,
    )?;
    parse_style_description(&raw)
}

/// Parse a JSON string into [`StyleDescription`].
pub fn parse_style_description(json: &str) -> VisionChatResult<StyleDescription> {
    serde_json::from_str::<StyleDescription>(json)
        .map_err(|e| VisionChatError::Chat(crate::llm_chat::ChatError::Decode(e.to_string())))
}

const SYSTEM_PROMPT: &str = "You are a visual-design taxonomist. Describe \
    the style of the image in concise, professional language. Use only \
    the JSON shape specified by the grammar. Each tag list contains at \
    most 4 short adjectives.";

const USER_PROMPT: &str = "Describe this image's style as JSON with: \
    summary (1-2 sentences), colorMood (adjectives), typography \
    (adjectives), layout (adjectives).";

/// GBNF grammar — accepts the 4-field [`StyleDescription`] shape.
pub const STYLE_GRAMMAR: &str = r#"
root        ::= "{" ws "\"summary\"" ws ":" ws string ws "," ws
                "\"colorMood\"" ws ":" ws tag-array ws "," ws
                "\"typography\"" ws ":" ws tag-array ws "," ws
                "\"layout\"" ws ":" ws tag-array ws "}"

tag-array   ::= "[" ws (string (ws "," ws string){0,3})? ws "]"
string      ::= "\"" ([^"\\] | "\\" .){1,200} "\""
ws          ::= [ \t\n]*
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_round_trips_canonical_shape() {
        let raw = r#"{
            "summary": "A bright editorial layout with airy whitespace.",
            "colorMood": ["warm", "high contrast"],
            "typography": ["editorial serif"],
            "layout": ["grid-aligned", "centered hero"]
        }"#;
        let s = parse_style_description(raw).unwrap();
        assert!(s.summary.starts_with("A bright"));
        assert_eq!(s.color_mood.len(), 2);
        assert_eq!(s.layout, vec!["grid-aligned", "centered hero"]);
    }

    #[test]
    fn wire_format_is_camelcase() {
        let s = StyleDescription {
            summary: "ok".into(),
            color_mood: vec!["warm".into()],
            typography: vec!["serif".into()],
            layout: vec!["grid".into()],
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"colorMood\""));
        assert!(!json.contains("\"color_mood\""));
    }

    #[test]
    fn grammar_mentions_all_camelcase_fields() {
        for field in ["summary", "colorMood", "typography", "layout"] {
            assert!(
                STYLE_GRAMMAR.contains(field),
                "grammar missing field `{field}`",
            );
        }
    }
}
