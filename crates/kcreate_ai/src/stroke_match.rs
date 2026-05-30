//! Stroke-style matching — Phase 10 Block B Task 7.
//!
//! Extracts the stroke properties of a single source node and
//! transcribes them onto a list of target nodes. The mapping is
//! direct (copy width, dash, cap, join, colour) — there's no
//! geometry remapping because stroke properties are intrinsically
//! transferable across path shapes.
//!
//! The crate-level function works on serializable wire structs so
//! it stays decoupled from the renderer's full `Node` graph. The
//! bridge constructs [`StrokeProperties`] from a node's `NodeStyle`
//! and threads the result back into [`StrokeStyle`] when writing.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// A flattened view of a node's stroke properties suitable for
/// transfer across nodes. The bridge maps between this and
/// `kcreate_core::node::StrokeStyle` so the AI crate never has to
/// link the entire renderer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StrokeProperties {
    /// `#RRGGBBAA` (lowercase) — matches the wire-format convention
    /// the renderer uses.
    pub color_hex: String,
    pub width: f64,
    pub dash: Vec<f64>,
    pub cap: String,
    pub join: String,
    /// `(t, width)` pairs describing a variable-width stroke profile
    /// when present. `None` means uniform width.
    pub width_profile: Option<Vec<(f64, f64)>>,
}

impl Default for StrokeProperties {
    fn default() -> Self {
        Self {
            color_hex: "#000000ff".into(),
            width: 1.0,
            dash: Vec::new(),
            cap: "butt".into(),
            join: "miter".into(),
            width_profile: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StrokeDeltaApplied {
    pub target_node_id: String,
    /// `true` when the target had a previous stroke that got
    /// overwritten; `false` when the target previously had `None`.
    pub had_previous_stroke: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StrokeMatchSummary {
    pub source_node_id: String,
    pub applied: Vec<StrokeDeltaApplied>,
    pub source_properties: StrokeProperties,
}

#[derive(Debug, Error)]
pub enum StrokeMatchError {
    #[error("stroke_match: source node has no stroke to copy")]
    NoSourceStroke,
    #[error("stroke_match: targets list was empty")]
    NoTargets,
}

/// Compute the per-target deltas that the bridge will apply. This
/// function is intentionally side-effect-free — the bridge owns the
/// renderer mutations.
///
/// # Errors
///
/// Returns [`StrokeMatchError::NoTargets`] when `targets` is empty
/// or [`StrokeMatchError::NoSourceStroke`] when the source has no
/// stroke configured.
pub fn match_stroke_style(
    source_id: &str,
    source: Option<&StrokeProperties>,
    targets: &[(String, bool /* had_previous */)],
) -> Result<StrokeMatchSummary, StrokeMatchError> {
    if targets.is_empty() {
        return Err(StrokeMatchError::NoTargets);
    }
    let source = source.ok_or(StrokeMatchError::NoSourceStroke)?;
    let applied = targets
        .iter()
        .map(|(id, had_previous)| StrokeDeltaApplied {
            target_node_id: id.clone(),
            had_previous_stroke: *had_previous,
        })
        .collect();
    Ok(StrokeMatchSummary {
        source_node_id: source_id.to_string(),
        applied,
        source_properties: source.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_targets_errors() {
        let props = StrokeProperties::default();
        let err = match_stroke_style("a", Some(&props), &[]).unwrap_err();
        assert!(matches!(err, StrokeMatchError::NoTargets));
    }

    #[test]
    fn no_source_stroke_errors() {
        let err = match_stroke_style("a", None, &[("b".into(), false)]).unwrap_err();
        assert!(matches!(err, StrokeMatchError::NoSourceStroke));
    }

    #[test]
    fn applies_to_every_target_with_correct_flag() {
        let props = StrokeProperties {
            color_hex: "#ff0000ff".into(),
            width: 2.5,
            dash: vec![4.0, 2.0],
            cap: "round".into(),
            join: "round".into(),
            width_profile: Some(vec![(0.0, 1.0), (1.0, 3.0)]),
        };
        let summary = match_stroke_style(
            "src",
            Some(&props),
            &[("a".into(), true), ("b".into(), false), ("c".into(), false)],
        )
        .unwrap();
        assert_eq!(summary.source_node_id, "src");
        assert_eq!(summary.applied.len(), 3);
        assert!(summary.applied[0].had_previous_stroke);
        assert!(!summary.applied[1].had_previous_stroke);
        assert_eq!(summary.source_properties.color_hex, "#ff0000ff");
        assert_eq!(summary.source_properties.dash, vec![4.0, 2.0]);
    }
}
