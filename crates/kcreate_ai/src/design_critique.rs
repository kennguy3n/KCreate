//! Design critique via VLM (Task 13).
//!
//! Sends an artboard snapshot to a vision model with a prompt that
//! asks for terse, actionable design feedback (contrast, alignment,
//! whitespace, hierarchy). The VLM's reply is the structured
//! critique returned to the renderer.
//!
//! This is the only design-critique surface — there is no fallback
//! to a heuristic critique today. Callers gate on
//! `vision_sidecar.is_ready()` before invoking.

use crate::vision_chat::{describe_image, VisionChatResult};

/// Generate a design critique for the provided artboard image.
/// Returns the critique as plain-text Markdown (the renderer
/// surfaces it verbatim in the Ask → Preview → Apply panel).
///
/// `port` is the loopback port of a running vision sidecar.
pub fn critique_design(
    port: u16,
    rgba: &[u8],
    width: u32,
    height: u32,
) -> VisionChatResult<String> {
    describe_image(port, SYSTEM_PROMPT, USER_PROMPT, rgba, width, height)
}

const SYSTEM_PROMPT: &str = "You are a senior product designer doing a \
    rapid critique of an in-progress UI screen. Be terse and actionable. \
    Only flag concrete issues; do not pad with compliments. Group findings \
    under these headings: Hierarchy, Contrast, Alignment, Spacing, \
    Typography, Accessibility. Each finding is a single bullet. If a \
    section has no issues, omit it entirely.";

const USER_PROMPT: &str = "Critique this design. Identify the highest-\
    impact issues only. Format the response as a Markdown bulleted list \
    under the section headings from your system prompt.";

#[cfg(test)]
mod tests {
    use super::*;

    /// The system prompt must instruct the VLM to be terse and
    /// action-oriented. Catches a future copy-edit that softens the
    /// guidance into open-ended praise.
    #[test]
    fn system_prompt_requests_actionable_critique() {
        let p = SYSTEM_PROMPT;
        assert!(p.contains("terse"));
        assert!(p.contains("actionable"));
        assert!(p.contains("Contrast"));
        assert!(p.contains("Alignment"));
        assert!(p.contains("Accessibility"));
    }

    /// Compile-test: `critique_design` accepts the canonical RGBA
    /// signature the bridge exports. Without a live sidecar the
    /// call returns an `ChatError::FeatureDisabled` (default build)
    /// or a transport error — either way it must not panic.
    #[test]
    fn signature_compiles_with_canonical_inputs() {
        let pixels = vec![0u8; 4 * 4];
        let err = critique_design(0, &pixels, 2, 2);
        assert!(err.is_err(), "no live sidecar on test port 0");
    }
}
