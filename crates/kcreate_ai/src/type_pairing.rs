//! Suggest a complementary body font for a given heading font —
//! Phase 10 Block B Task 12.
//!
//! When the LLM sidecar is installed the bridge can use it to
//! generate semantically rich pairing rationales. The function in
//! this module is the deterministic fallback used otherwise: it
//! classifies the heading font and returns 3–5 body-font candidates
//! drawn from a curated list of widely-installed system fonts that
//! pair well with the detected category.
//!
//! Categories:
//!
//! - **serif** — strong body font: sans-serif (Inter, Helvetica)
//! - **sans-serif** — body font with subtle contrast: serif (Georgia, Cambria)
//! - **mono** — paired with a clean sans (Inter, Roboto)
//! - **display / decorative** — paired with a neutral sans
//! - **handwritten / script** — paired with a clean serif
//!
//! The bridge filters the suggestions against `fontdb` to ensure the
//! candidate is actually installed before surfacing it.

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TypePairingSuggestion {
    pub font_name: String,
    pub reason: String,
    pub confidence: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TypePairingResult {
    pub heading_font: String,
    pub heading_category: String,
    pub suggestions: Vec<TypePairingSuggestion>,
}

#[derive(Debug, Error)]
pub enum TypePairingError {
    #[error("type_pairing: heading font name is empty")]
    EmptyHeading,
}

/// Suggest body-font pairings for `heading_font`.
///
/// # Errors
///
/// Returns [`TypePairingError::EmptyHeading`] when the heading font
/// name is empty or whitespace.
pub fn suggest_type_pairing(heading_font: &str) -> Result<TypePairingResult, TypePairingError> {
    let trimmed = heading_font.trim();
    if trimmed.is_empty() {
        return Err(TypePairingError::EmptyHeading);
    }
    let category = classify(trimmed);
    let suggestions = candidates_for(category);
    Ok(TypePairingResult {
        heading_font: trimmed.to_string(),
        heading_category: category.as_str().to_string(),
        suggestions,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FontCategory {
    Serif,
    SansSerif,
    Monospace,
    Display,
    Script,
}

impl FontCategory {
    fn as_str(self) -> &'static str {
        match self {
            Self::Serif => "serif",
            Self::SansSerif => "sans_serif",
            Self::Monospace => "monospace",
            Self::Display => "display",
            Self::Script => "script",
        }
    }
}

fn classify(font: &str) -> FontCategory {
    let lower = font.to_lowercase();
    // Order matters — match the most-specific category first.
    if contains_any(
        &lower,
        &[
            "script",
            "hand",
            "brush",
            "pacifico",
            "dancing",
            "lobster",
            "great vibes",
            "satisfy",
        ],
    ) {
        FontCategory::Script
    } else if contains_any(
        &lower,
        &[
            "mono",
            "courier",
            "consolas",
            "menlo",
            "fira code",
            "jetbrains",
            "source code",
            "ibm plex mono",
            "roboto mono",
        ],
    ) {
        FontCategory::Monospace
    } else if contains_any(
        &lower,
        &[
            "display",
            "bebas",
            "anton",
            "oswald",
            "playfair display",
            "abril fatface",
            "fraunces",
            "rubik mono",
            "alfa slab",
        ],
    ) {
        FontCategory::Display
    } else if contains_any(
        &lower,
        &[
            "serif",
            "garamond",
            "baskerville",
            "georgia",
            "cambria",
            "times",
            "merriweather",
            "lora",
            "noto serif",
            "source serif",
        ],
    ) {
        FontCategory::Serif
    } else {
        // Default sans — Inter, Helvetica, Roboto, Arial, etc. all
        // fall here.
        FontCategory::SansSerif
    }
}

fn contains_any(s: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| s.contains(n))
}

fn candidates_for(cat: FontCategory) -> Vec<TypePairingSuggestion> {
    match cat {
        FontCategory::Serif => vec![
            sug("Inter", "Modern sans pairs cleanly with a classic serif heading", 0.9),
            sug("Helvetica Neue", "Neutral, ubiquitous body font for serif headings", 0.85),
            sug("Source Sans 3", "Open-source sans with excellent legibility under serifs", 0.8),
        ],
        FontCategory::SansSerif => vec![
            sug("Source Serif 4", "Adobe's open-source companion serif for sans headings", 0.9),
            sug("Lora", "Friendly humanist serif body, pairs with geometric sans", 0.85),
            sug("Georgia", "Web-standard serif, high readability at small sizes", 0.8),
        ],
        FontCategory::Monospace => vec![
            sug("Inter", "Geometric sans contrasts with monospace headings", 0.9),
            sug("Roboto", "Clean, neutral body for code-style headings", 0.85),
            sug("IBM Plex Sans", "Same superfamily as IBM Plex Mono", 0.8),
        ],
        FontCategory::Display => vec![
            sug("Inter", "Neutral sans lets the display heading dominate", 0.9),
            sug("Source Sans 3", "Open-source workhorse that won't compete with display type", 0.85),
            sug("Noto Sans", "Wide language coverage for display-heavy designs", 0.8),
        ],
        FontCategory::Script => vec![
            sug("Lora", "Friendly serif balances script headings", 0.9),
            sug("Source Serif 4", "Refined serif that complements handwritten type", 0.85),
            sug("Georgia", "Approachable serif anchor for script", 0.8),
        ],
    }
}

fn sug(name: &str, reason: &str, confidence: f32) -> TypePairingSuggestion {
    TypePairingSuggestion {
        font_name: name.to_string(),
        reason: reason.to_string(),
        confidence,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_heading_errors() {
        let err = suggest_type_pairing("   ").unwrap_err();
        assert!(matches!(err, TypePairingError::EmptyHeading));
    }

    #[test]
    fn georgia_classified_as_serif() {
        let r = suggest_type_pairing("Georgia").unwrap();
        assert_eq!(r.heading_category, "serif");
        // Serif heading → sans body candidate first.
        assert_eq!(r.suggestions[0].font_name, "Inter");
    }

    #[test]
    fn inter_classified_as_sans_serif() {
        let r = suggest_type_pairing("Inter").unwrap();
        assert_eq!(r.heading_category, "sans_serif");
        // Sans heading → serif body candidate first.
        assert!(r.suggestions[0].font_name.contains("Serif")
            || r.suggestions[0].font_name == "Lora"
            || r.suggestions[0].font_name == "Georgia");
    }

    #[test]
    fn courier_classified_as_monospace() {
        let r = suggest_type_pairing("Courier New").unwrap();
        assert_eq!(r.heading_category, "monospace");
    }

    #[test]
    fn display_fonts_detected() {
        let r = suggest_type_pairing("Playfair Display").unwrap();
        assert_eq!(r.heading_category, "display");
    }

    #[test]
    fn script_fonts_detected() {
        let r = suggest_type_pairing("Pacifico").unwrap();
        assert_eq!(r.heading_category, "script");
    }

    #[test]
    fn always_returns_at_least_three_suggestions() {
        let r = suggest_type_pairing("Helvetica").unwrap();
        assert!(r.suggestions.len() >= 3);
        for s in &r.suggestions {
            assert!(s.confidence > 0.0 && s.confidence <= 1.0);
        }
    }
}
