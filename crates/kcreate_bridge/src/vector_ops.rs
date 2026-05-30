//! Phase 5 vector path operations exposed via the N-API bridge.
//!
//! Each entry point:
//! 1. Loads the target [`NodeType::VectorLayer`] geometry from
//!    `node.metadata[VECTOR_PATH_METADATA_KEY]`.
//! 2. Runs the requested [`kcreate_vector`] operation (simplify /
//!    smooth / offset / stroke profile / path effect).
//! 3. Writes the result back, records an undoable [`Operation`]
//!    whose `before_patch` is the pre-op node snapshot, and triggers
//!    a scene resync so the renderer picks the new path up.
//!
//! Style-only mutations (`set_stroke_profile`, `apply_path_effect`,
//! `clear_path_effects`) touch `node.style` rather than the
//! geometry so the original centreline / corner-stop path is
//! preserved — the renderer in
//! `scene_sync::emit_vector` applies the effect chain at draw time.

use chrono::Utc;
use uuid::Uuid;

use kcreate_core::node::{NodeType, PathEffect};
use kcreate_core::operation::Operation;
use kcreate_export::scene_metadata::VECTOR_PATH_METADATA_KEY;
use kcreate_vector::VectorPath;
use kcreate_vector::{offset as offset_path, simplify as simplify_path, smooth as smooth_path};

use crate::document::{slot, sync_scene_locked, DocumentBridgeError, Result};

fn load_vector_path(node_id: Uuid) -> Result<VectorPath> {
    // Phase 11 Block D follow-up round 2 — Devin Review BUG-0004.
    // Pure-read path (no mutation): use `slot().read()` so concurrent
    // vector simplify/smooth/offset ops on different nodes (and any
    // unrelated read traffic — tree view, status bar, selection
    // inspector) can run in parallel. Same rationale as
    // `load_layer_pixels` in `raster_ops.rs` (Devin Review BUG-0001 +
    // ANALYSIS-0001 round 1).
    let guard = slot().read();
    let ws = guard.as_ref().ok_or(DocumentBridgeError::NoProject)?;
    let node = ws
        .project
        .document
        .get_node(node_id)
        .ok_or(DocumentBridgeError::NodeNotFound(node_id))?;
    if !matches!(node.node_type, NodeType::VectorLayer) {
        return Err(DocumentBridgeError::InvalidNodeType(format!(
            "{:?}",
            node.node_type
        )));
    }
    let value = node.metadata.get(VECTOR_PATH_METADATA_KEY).ok_or_else(|| {
        DocumentBridgeError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "vector layer missing geometry metadata",
        ))
    })?;
    let path: VectorPath = serde_json::from_value(value.clone())?;
    Ok(path)
}

/// Replace a vector node's geometry with `new_path`, recording an
/// undoable [`Operation`] capturing the before/after node snapshots.
fn replace_vector_geometry(
    node_id: Uuid,
    new_path: VectorPath,
    op_kind: &'static str,
    op_payload: serde_json::Value,
) -> Result<()> {
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;

    let before_snapshot = ws
        .project
        .document
        .get_node(node_id)
        .map_or(serde_json::Value::Null, |n| {
            serde_json::to_value(n).unwrap_or(serde_json::Value::Null)
        });

    {
        let node = ws
            .project
            .document
            .get_node_mut(node_id)
            .ok_or(DocumentBridgeError::NodeNotFound(node_id))?;
        if !matches!(node.node_type, NodeType::VectorLayer) {
            return Err(DocumentBridgeError::InvalidNodeType(format!(
                "{:?}",
                node.node_type
            )));
        }
        node.metadata.insert(
            VECTOR_PATH_METADATA_KEY.to_string(),
            serde_json::to_value(&new_path)?,
        );
    }

    let after_snapshot = ws
        .project
        .document
        .get_node(node_id)
        .map_or(serde_json::Value::Null, |n| {
            serde_json::to_value(n).unwrap_or(serde_json::Value::Null)
        });

    let op = Operation::new(
        "vector",
        op_kind,
        serde_json::json!({
            "before": before_snapshot,
            "params": op_payload,
        }),
        after_snapshot,
        vec![node_id],
    );
    ws.project.execute_operation(op);
    ws.project.modified_at = Utc::now();
    let _ = sync_scene_locked(&mut guard);
    Ok(())
}

/// Apply Ramer-Douglas-Peucker simplification to the node's path.
/// `tolerance` is in world units.
pub fn simplify(node_id: Uuid, tolerance: f64) -> Result<()> {
    if !tolerance.is_finite() || tolerance < 0.0 {
        return Err(DocumentBridgeError::InvalidArgument {
            argument: "tolerance".into(),
            value: format!("{tolerance} (must be finite and non-negative)"),
        });
    }
    let path = load_vector_path(node_id)?;
    let new_path = simplify_path(&path, tolerance);
    replace_vector_geometry(
        node_id,
        new_path,
        "vector_simplify",
        serde_json::json!({ "tolerance": tolerance }),
    )
}

/// Apply Chaikin corner-cutting smoothing `iterations` times.
pub fn smooth(node_id: Uuid, iterations: u32) -> Result<()> {
    let path = load_vector_path(node_id)?;
    let new_path = smooth_path(&path, iterations);
    replace_vector_geometry(
        node_id,
        new_path,
        "vector_smooth",
        serde_json::json!({ "iterations": iterations }),
    )
}

/// Apply a parallel offset (`distance` in world units; positive =
/// outward for closed paths).
pub fn offset(node_id: Uuid, distance: f64) -> Result<()> {
    if !distance.is_finite() {
        return Err(DocumentBridgeError::InvalidArgument {
            argument: "distance".into(),
            value: format!("{distance} (must be finite)"),
        });
    }
    let path = load_vector_path(node_id)?;
    let new_path = offset_path(&path, distance);
    replace_vector_geometry(
        node_id,
        new_path,
        "vector_offset",
        serde_json::json!({ "distance": distance }),
    )
}

/// Style-only mutation helper. Applies `mutator` to the node's
/// `NodeStyle`, records an undoable operation, and resyncs.
fn mutate_style(
    node_id: Uuid,
    op_kind: &'static str,
    op_payload: serde_json::Value,
    mutator: impl FnOnce(&mut kcreate_core::node::NodeStyle),
) -> Result<()> {
    let mut guard = slot().write();
    let ws = guard.as_mut().ok_or(DocumentBridgeError::NoProject)?;

    let before_snapshot = ws
        .project
        .document
        .get_node(node_id)
        .map_or(serde_json::Value::Null, |n| {
            serde_json::to_value(n).unwrap_or(serde_json::Value::Null)
        });

    {
        let node = ws
            .project
            .document
            .get_node_mut(node_id)
            .ok_or(DocumentBridgeError::NodeNotFound(node_id))?;
        mutator(&mut node.style);
    }

    let after_snapshot = ws
        .project
        .document
        .get_node(node_id)
        .map_or(serde_json::Value::Null, |n| {
            serde_json::to_value(n).unwrap_or(serde_json::Value::Null)
        });

    let op = Operation::new(
        "vector",
        op_kind,
        serde_json::json!({
            "before": before_snapshot,
            "params": op_payload,
        }),
        after_snapshot,
        vec![node_id],
    );
    ws.project.execute_operation(op);
    ws.project.modified_at = Utc::now();
    let _ = sync_scene_locked(&mut guard);
    Ok(())
}

/// Install (or clear) a variable stroke-width profile on the
/// node's primary stroke. `profile` of `None` clears any
/// existing profile (uniform width); `Some(empty)` is treated as
/// `None` because an empty profile is degenerate.
pub fn set_stroke_profile(node_id: Uuid, profile: Option<Vec<(f64, f64)>>) -> Result<()> {
    for (i, (t, w)) in profile.as_ref().into_iter().flatten().enumerate() {
        if !t.is_finite() || !(0.0..=1.0).contains(t) {
            return Err(DocumentBridgeError::InvalidArgument {
                argument: format!("profile[{i}].t"),
                value: format!("{t} (must be finite and in [0, 1])"),
            });
        }
        if !w.is_finite() || *w < 0.0 {
            return Err(DocumentBridgeError::InvalidArgument {
                argument: format!("profile[{i}].width"),
                value: format!("{w} (must be finite and non-negative)"),
            });
        }
    }
    let normalised = profile.and_then(|p| if p.is_empty() { None } else { Some(p) });
    let payload = serde_json::to_value(&normalised).unwrap_or(serde_json::Value::Null);
    mutate_style(
        node_id,
        "vector_set_stroke_profile",
        payload,
        move |style| {
            style.stroke_width_profile = normalised;
        },
    )
}

/// Push a [`PathEffect`] onto the node's effect chain. Effects are
/// applied at render time in order.
pub fn apply_path_effect(node_id: Uuid, effect: PathEffect) -> Result<()> {
    if let PathEffect::Dash { pattern, offset } = &effect {
        if pattern.is_empty() {
            return Err(DocumentBridgeError::InvalidArgument {
                argument: "pattern".into(),
                value: "empty (must contain at least one entry)".into(),
            });
        }
        for (i, v) in pattern.iter().enumerate() {
            if !v.is_finite() || *v < 0.0 {
                return Err(DocumentBridgeError::InvalidArgument {
                    argument: format!("pattern[{i}]"),
                    value: format!("{v} (must be finite and non-negative)"),
                });
            }
        }
        if !offset.is_finite() {
            return Err(DocumentBridgeError::InvalidArgument {
                argument: "offset".into(),
                value: format!("{offset} (must be finite)"),
            });
        }
    }
    if let PathEffect::RoundCorners { radius } = &effect {
        if !radius.is_finite() || *radius < 0.0 {
            return Err(DocumentBridgeError::InvalidArgument {
                argument: "radius".into(),
                value: format!("{radius} (must be finite and non-negative)"),
            });
        }
    }
    let payload = serde_json::to_value(&effect).unwrap_or(serde_json::Value::Null);
    mutate_style(node_id, "vector_apply_path_effect", payload, move |style| {
        style.path_effects.push(effect);
    })
}

/// Remove every effect from the node. The original geometry is
/// already preserved on `node.metadata[VECTOR_PATH]` so undo can
/// reinstate the effect chain.
pub fn clear_path_effects(node_id: Uuid) -> Result<()> {
    mutate_style(
        node_id,
        "vector_clear_path_effects",
        serde_json::Value::Null,
        |style| {
            style.path_effects.clear();
        },
    )
}
