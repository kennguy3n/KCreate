//! Smart-guides snapping engine.
//!
//! [`SnapEngine`] builds a sorted edge list from visible node bounds
//! (plus optional artboard edges). When the user drags a candidate
//! [`Bounds`](kcreate_core::node::Bounds), `snap()` finds the nearest
//! horizontal and vertical edges within a pixel threshold and returns
//! a delta that snaps the candidate onto those edges, together with
//! the guide lines the overlay should render.

use serde::{Deserialize, Serialize};

/// Axis of a snap guide line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Axis {
    Horizontal,
    Vertical,
}

/// A visual guide line produced by the snap engine.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SnapGuide {
    pub axis: Axis,
    pub position: f64,
    pub from: f64,
    pub to: f64,
}

/// Result of a snap query.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SnapResult {
    pub dx: f64,
    pub dy: f64,
    pub guides: Vec<SnapGuide>,
}

/// A set of edges (left, right, center_x, top, bottom, center_y) for
/// one target element.
#[derive(Debug, Clone)]
pub struct SnapTarget {
    pub left: f64,
    pub right: f64,
    pub top: f64,
    pub bottom: f64,
    pub center_x: f64,
    pub center_y: f64,
}

impl SnapTarget {
    #[must_use]
    pub fn from_bounds(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self {
            left: x,
            right: x + w,
            top: y,
            bottom: y + h,
            center_x: x + w * 0.5,
            center_y: y + h * 0.5,
        }
    }
}

/// Sorted edge lists for efficient snap queries.
#[derive(Debug, Clone)]
pub struct SnapEngine {
    /// Sorted horizontal edges (left, right, center_x of every target).
    h_edges: Vec<f64>,
    /// Sorted vertical edges (top, bottom, center_y of every target).
    v_edges: Vec<f64>,
    /// Targets retained for guide-line extent computation.
    targets: Vec<SnapTarget>,
}

impl SnapEngine {
    /// Build a new engine from a set of targets.
    #[must_use]
    pub fn new(targets: Vec<SnapTarget>) -> Self {
        let mut h_edges: Vec<f64> = Vec::with_capacity(targets.len() * 3);
        let mut v_edges: Vec<f64> = Vec::with_capacity(targets.len() * 3);
        for t in &targets {
            h_edges.push(t.left);
            h_edges.push(t.right);
            h_edges.push(t.center_x);
            v_edges.push(t.top);
            v_edges.push(t.bottom);
            v_edges.push(t.center_y);
        }
        h_edges.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        v_edges.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        h_edges.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
        v_edges.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
        Self {
            h_edges,
            v_edges,
            targets,
        }
    }

    /// Snap a candidate bounds within the given pixel threshold.
    ///
    /// Returns the delta to apply to the candidate origin plus a
    /// list of guide lines.
    #[must_use]
    pub fn snap(
        &self,
        candidate_x: f64,
        candidate_y: f64,
        candidate_w: f64,
        candidate_h: f64,
        threshold: f64,
    ) -> SnapResult {
        let cand = SnapTarget::from_bounds(candidate_x, candidate_y, candidate_w, candidate_h);
        let cand_h = [cand.left, cand.right, cand.center_x];
        let cand_v = [cand.top, cand.bottom, cand.center_y];

        let dx = find_nearest_delta(&self.h_edges, &cand_h, threshold);
        let dy = find_nearest_delta(&self.v_edges, &cand_v, threshold);

        let mut guides = Vec::new();
        if dx.abs() > 0.0 {
            let snapped_x_edge = nearest_edge(&self.h_edges, cand.left + dx);
            if let Some(pos) = snapped_x_edge {
                let (from, to) = guide_extent_v(pos, &self.targets, &cand);
                guides.push(SnapGuide {
                    axis: Axis::Vertical,
                    position: pos,
                    from,
                    to,
                });
            }
        }
        if dy.abs() > 0.0 {
            let snapped_y_edge = nearest_edge(&self.v_edges, cand.top + dy);
            if let Some(pos) = snapped_y_edge {
                let (from, to) = guide_extent_h(pos, &self.targets, &cand);
                guides.push(SnapGuide {
                    axis: Axis::Horizontal,
                    position: pos,
                    from,
                    to,
                });
            }
        }

        SnapResult { dx, dy, guides }
    }
}

fn find_nearest_delta(sorted_edges: &[f64], candidate_edges: &[f64], threshold: f64) -> f64 {
    let mut best_delta = 0.0f64;
    let mut best_dist = f64::INFINITY;
    for &ce in candidate_edges {
        // Binary search for the closest edge.
        let idx = sorted_edges
            .binary_search_by(|e| e.partial_cmp(&ce).unwrap_or(std::cmp::Ordering::Equal))
            .unwrap_or_else(|i| i);
        for &adj in &[idx.saturating_sub(1), idx, idx + 1] {
            if adj >= sorted_edges.len() {
                continue;
            }
            let edge = sorted_edges[adj];
            let delta = edge - ce;
            let dist = delta.abs();
            if dist < best_dist && dist <= threshold {
                best_dist = dist;
                best_delta = delta;
            }
        }
    }
    best_delta
}

fn nearest_edge(sorted: &[f64], value: f64) -> Option<f64> {
    let idx = sorted
        .binary_search_by(|e| e.partial_cmp(&value).unwrap_or(std::cmp::Ordering::Equal))
        .unwrap_or_else(|i| i);
    let mut best: Option<f64> = None;
    let mut best_d = f64::INFINITY;
    for &adj in &[idx.saturating_sub(1), idx, idx + 1] {
        if adj >= sorted.len() {
            continue;
        }
        let d = (sorted[adj] - value).abs();
        if d < best_d {
            best_d = d;
            best = Some(sorted[adj]);
        }
    }
    best
}

fn guide_extent_v(x: f64, targets: &[SnapTarget], cand: &SnapTarget) -> (f64, f64) {
    let mut min_y = cand.top;
    let mut max_y = cand.bottom;
    let eps = 1e-6;
    for t in targets {
        if (t.left - x).abs() < eps || (t.right - x).abs() < eps || (t.center_x - x).abs() < eps {
            min_y = min_y.min(t.top);
            max_y = max_y.max(t.bottom);
        }
    }
    (min_y, max_y)
}

fn guide_extent_h(y: f64, targets: &[SnapTarget], cand: &SnapTarget) -> (f64, f64) {
    let mut min_x = cand.left;
    let mut max_x = cand.right;
    let eps = 1e-6;
    for t in targets {
        if (t.top - y).abs() < eps || (t.bottom - y).abs() < eps || (t.center_y - y).abs() < eps {
            min_x = min_x.min(t.left);
            max_x = max_x.max(t.right);
        }
    }
    (min_x, max_x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snap_finds_edge_within_threshold() {
        let targets = vec![SnapTarget::from_bounds(100.0, 200.0, 50.0, 30.0)];
        let engine = SnapEngine::new(targets);
        let result = engine.snap(148.0, 195.0, 40.0, 20.0, 5.0);
        // Candidate left=148 should snap to target right=150.
        assert!((result.dx - 2.0).abs() < 1e-6);
        assert!(!result.guides.is_empty());
    }

    #[test]
    fn snap_no_nearby_returns_zero() {
        let targets = vec![SnapTarget::from_bounds(100.0, 200.0, 50.0, 30.0)];
        let engine = SnapEngine::new(targets);
        let result = engine.snap(0.0, 0.0, 10.0, 10.0, 5.0);
        assert!(result.dx.abs() < 1e-6);
        assert!(result.dy.abs() < 1e-6);
        assert!(result.guides.is_empty());
    }

    #[test]
    fn snap_prefers_closer_edge() {
        let targets = vec![
            SnapTarget::from_bounds(100.0, 0.0, 50.0, 10.0),
            SnapTarget::from_bounds(110.0, 0.0, 50.0, 10.0),
        ];
        let engine = SnapEngine::new(targets);
        // Candidate left=108. Edges at 100, 110, 125, 135, 150, 160.
        // Nearest to 108 is 110 (delta +2).
        let result = engine.snap(108.0, 0.0, 20.0, 5.0, 5.0);
        assert!((result.dx - 2.0).abs() < 1e-6);
    }

    #[test]
    fn snap_to_center() {
        let targets = vec![SnapTarget::from_bounds(100.0, 100.0, 100.0, 100.0)];
        let engine = SnapEngine::new(targets);
        // Candidate center should snap to target center (150, 150).
        // Candidate: x=130, w=40 → center_x=150 — exact hit.
        let result = engine.snap(130.0, 130.0, 40.0, 40.0, 5.0);
        assert!(result.dx.abs() < 1e-6);
        assert!(result.dy.abs() < 1e-6);
    }
}
