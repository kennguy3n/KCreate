//! Layout-grouping suggestions for the current artboard.
//!
//! Given a list of `(node_id, bounds)` pairs from the bridge, this
//! module clusters them into proposed groups by **spatial
//! proximity** (modified DBSCAN over centroid distance scaled by
//! the median nearest-neighbour distance, so the threshold adapts
//! to the artboard's natural density) **plus alignment edges**
//! (nodes that share a left/right/top/bottom edge within ~2px are
//! merged into the same cluster regardless of distance, because
//! they're almost certainly meant to read as a row/column).
//!
//! Output: a `LayoutSuggestion` per cluster carrying the member
//! ids, a name like `"row-of-3-aligned-left"` derived from the
//! cluster's geometry, and a recommended container bounds so the
//! bridge can wrap the cluster in a group node.
//!
//! Pure-function, no I/O, no networking. Used by
//! [`crate::execute_task`] under [`crate::AiTask::LayoutSuggestion`].

use std::collections::{BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Axis-aligned bounding box. `x`/`y` are the top-left corner.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[allow(clippy::derive_partial_eq_without_eq)]
pub struct Bounds {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Bounds {
    #[must_use]
    pub fn centroid(&self) -> (f32, f32) {
        (self.x + self.width / 2.0, self.y + self.height / 2.0)
    }

    #[must_use]
    pub fn right(&self) -> f32 {
        self.x + self.width
    }

    #[must_use]
    pub fn bottom(&self) -> f32 {
        self.y + self.height
    }

    /// Union of two boxes — used to compute a wrapping group bounds.
    #[must_use]
    pub fn union(self, other: Self) -> Self {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        let right = self.right().max(other.right());
        let bottom = self.bottom().max(other.bottom());
        Self {
            x,
            y,
            width: right - x,
            height: bottom - y,
        }
    }
}

/// A node the layout suggester should consider.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[allow(clippy::derive_partial_eq_without_eq)]
pub struct LayoutNode {
    pub id: Uuid,
    pub bounds: Bounds,
}

/// One proposed group. The bridge can apply it by creating a group
/// node with `bounds` and re-parenting each `member_ids` entry
/// under it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[allow(clippy::derive_partial_eq_without_eq)]
pub struct LayoutSuggestion {
    pub name: String,
    pub bounds: Bounds,
    pub member_ids: Vec<Uuid>,
    /// Detected dominant orientation. Used by the renderer to
    /// preview the group as a row or column.
    pub orientation: LayoutOrientation,
    /// Detected alignment edge within the cluster, if any.
    pub alignment: Option<LayoutAlignment>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutOrientation {
    Row,
    Column,
    Grid,
    /// Cluster doesn't read clearly as a row, column, or grid.
    Cloud,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutAlignment {
    Left,
    Right,
    Top,
    Bottom,
    CenterHorizontal,
    CenterVertical,
}

/// Tuning parameters for [`suggest_layout_grouping`].
#[derive(Debug, Clone, Copy)]
pub struct LayoutSuggestOptions {
    /// Pixel slack allowed for two edges to be considered aligned.
    /// Default 2.0 (1 px would miss subpixel rasterisations; 3+ px
    /// starts merging adjacent rows that aren't really aligned).
    pub alignment_tolerance: f32,
    /// Multiplier applied to the median nearest-neighbour distance
    /// to derive the DBSCAN epsilon. 1.75 was picked from the unit
    /// test fixtures — high enough to merge a 3-button row, low
    /// enough to keep a row and a paragraph below it as separate
    /// clusters.
    pub eps_multiplier: f32,
    /// Minimum cluster size to surface as a suggestion. 2 means
    /// "any pair"; 3 is a stricter "needs to be a real group".
    pub min_cluster_size: usize,
}

impl Default for LayoutSuggestOptions {
    fn default() -> Self {
        Self {
            alignment_tolerance: 2.0,
            eps_multiplier: 1.75,
            min_cluster_size: 2,
        }
    }
}

/// Errors returned by [`suggest_layout_grouping`].
#[derive(Debug, thiserror::Error)]
pub enum LayoutSuggestError {
    #[error("at least two nodes are required for a grouping suggestion")]
    TooFewNodes,
    #[error("a node bounds had non-finite or negative dimensions")]
    InvalidBounds,
}

/// Cluster the supplied nodes into proposed groups.
///
/// Returns suggestions sorted by descending member count so the UI
/// can highlight the most-impactful suggestion first.
pub fn suggest_layout_grouping(
    nodes: &[LayoutNode],
    options: LayoutSuggestOptions,
) -> Result<Vec<LayoutSuggestion>, LayoutSuggestError> {
    if nodes.len() < 2 {
        return Err(LayoutSuggestError::TooFewNodes);
    }
    for n in nodes {
        if !n.bounds.x.is_finite()
            || !n.bounds.y.is_finite()
            || !n.bounds.width.is_finite()
            || !n.bounds.height.is_finite()
            || n.bounds.width < 0.0
            || n.bounds.height < 0.0
        {
            return Err(LayoutSuggestError::InvalidBounds);
        }
    }

    let centroids: Vec<(f32, f32)> = nodes.iter().map(|n| n.bounds.centroid()).collect();
    let eps = derive_epsilon(&centroids, options.eps_multiplier);

    let clusters = cluster_with_alignment(nodes, &centroids, eps, options.alignment_tolerance);

    let mut suggestions: Vec<LayoutSuggestion> = clusters
        .into_iter()
        .filter(|c| c.len() >= options.min_cluster_size)
        .map(|c| build_suggestion(&c, nodes, options.alignment_tolerance))
        .collect();
    suggestions.sort_by_key(|s| std::cmp::Reverse(s.member_ids.len()));
    Ok(suggestions)
}

/// Compute the median nearest-neighbour centroid distance and
/// scale by `multiplier`. Falls back to 1.0 when there's only one
/// pair (so the multiplier dominates).
fn derive_epsilon(centroids: &[(f32, f32)], multiplier: f32) -> f32 {
    let n = centroids.len();
    if n < 2 {
        return 1.0;
    }
    let mut nearest = Vec::with_capacity(n);
    for i in 0..n {
        let mut best = f32::INFINITY;
        for j in 0..n {
            if i == j {
                continue;
            }
            let dx = centroids[i].0 - centroids[j].0;
            let dy = centroids[i].1 - centroids[j].1;
            let d = dx.hypot(dy);
            if d < best {
                best = d;
            }
        }
        if best.is_finite() {
            nearest.push(best);
        }
    }
    if nearest.is_empty() {
        return 1.0;
    }
    nearest.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median = nearest[nearest.len() / 2];
    (median * multiplier).max(1.0)
}

/// BFS-style proximity clustering: two nodes are linked iff their
/// centroid distance is <= `eps` OR they share an aligned edge
/// (within `align_tol`). The alignment short-circuit is important
/// for things like form fields where left edges align but vertical
/// spacing exceeds the proximity epsilon.
fn cluster_with_alignment(
    nodes: &[LayoutNode],
    centroids: &[(f32, f32)],
    eps: f32,
    align_tol: f32,
) -> Vec<Vec<usize>> {
    let n = nodes.len();
    let mut visited = vec![false; n];
    let mut clusters = Vec::new();
    for seed in 0..n {
        if visited[seed] {
            continue;
        }
        visited[seed] = true;
        let mut queue: VecDeque<usize> = VecDeque::new();
        let mut current = Vec::new();
        queue.push_back(seed);
        while let Some(i) = queue.pop_front() {
            current.push(i);
            for j in 0..n {
                if visited[j] {
                    continue;
                }
                let dx = centroids[i].0 - centroids[j].0;
                let dy = centroids[i].1 - centroids[j].1;
                let dist = dx.hypot(dy);
                let close_enough = dist <= eps;
                let aligned = (nodes[i].bounds.x - nodes[j].bounds.x).abs() <= align_tol
                    || (nodes[i].bounds.right() - nodes[j].bounds.right()).abs() <= align_tol
                    || (nodes[i].bounds.y - nodes[j].bounds.y).abs() <= align_tol
                    || (nodes[i].bounds.bottom() - nodes[j].bounds.bottom()).abs() <= align_tol;
                // Alignment alone isn't enough — the nodes also need
                // to be reasonably close on the *perpendicular* axis
                // (otherwise two unrelated rows on opposite sides of
                // the artboard would merge). We use 6× epsilon as
                // the perpendicular cap, which is loose but bounded.
                let perp_cap = eps * 6.0;
                let aligned_and_near = aligned && dist <= perp_cap;
                if close_enough || aligned_and_near {
                    visited[j] = true;
                    queue.push_back(j);
                }
            }
        }
        clusters.push(current);
    }
    clusters
}

fn build_suggestion(cluster: &[usize], nodes: &[LayoutNode], align_tol: f32) -> LayoutSuggestion {
    let members: Vec<&LayoutNode> = cluster.iter().map(|&i| &nodes[i]).collect();
    let mut bounds = members[0].bounds;
    for n in &members[1..] {
        bounds = bounds.union(n.bounds);
    }
    let orientation = detect_orientation(&members, align_tol);
    let alignment = detect_alignment(&members, align_tol);
    let name = render_name(orientation, alignment, members.len());
    let mut member_ids: Vec<Uuid> = members.iter().map(|m| m.id).collect();
    // Stable order so consumers (and tests) don't see flapping
    // suggestions for the same input.
    member_ids.sort();
    LayoutSuggestion {
        name,
        bounds,
        member_ids,
        orientation,
        alignment,
    }
}

fn detect_orientation(members: &[&LayoutNode], align_tol: f32) -> LayoutOrientation {
    if members.len() < 2 {
        return LayoutOrientation::Cloud;
    }
    let centroids: Vec<(f32, f32)> = members.iter().map(|m| m.bounds.centroid()).collect();
    let xs: BTreeSet<i32> = centroids
        .iter()
        .map(|c| (c.0 / align_tol).round() as i32)
        .collect();
    let ys: BTreeSet<i32> = centroids
        .iter()
        .map(|c| (c.1 / align_tol).round() as i32)
        .collect();
    let n = members.len();
    if ys.len() <= n / 3 + 1 && xs.len() > ys.len() {
        LayoutOrientation::Row
    } else if xs.len() <= n / 3 + 1 && ys.len() > xs.len() {
        LayoutOrientation::Column
    } else if xs.len() > 1 && ys.len() > 1 && xs.len() * ys.len() >= n {
        LayoutOrientation::Grid
    } else {
        LayoutOrientation::Cloud
    }
}

fn detect_alignment(members: &[&LayoutNode], align_tol: f32) -> Option<LayoutAlignment> {
    if members.len() < 2 {
        return None;
    }
    let lefts: Vec<f32> = members.iter().map(|m| m.bounds.x).collect();
    let rights: Vec<f32> = members.iter().map(|m| m.bounds.right()).collect();
    let tops: Vec<f32> = members.iter().map(|m| m.bounds.y).collect();
    let bottoms: Vec<f32> = members.iter().map(|m| m.bounds.bottom()).collect();
    let cxs: Vec<f32> = members.iter().map(|m| m.bounds.centroid().0).collect();
    let cys: Vec<f32> = members.iter().map(|m| m.bounds.centroid().1).collect();
    let within = |v: &[f32]| -> bool {
        let min = v.iter().copied().fold(f32::INFINITY, f32::min);
        let max = v.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        max - min <= align_tol
    };
    if within(&lefts) {
        return Some(LayoutAlignment::Left);
    }
    if within(&rights) {
        return Some(LayoutAlignment::Right);
    }
    if within(&tops) {
        return Some(LayoutAlignment::Top);
    }
    if within(&bottoms) {
        return Some(LayoutAlignment::Bottom);
    }
    if within(&cxs) {
        return Some(LayoutAlignment::CenterVertical);
    }
    if within(&cys) {
        return Some(LayoutAlignment::CenterHorizontal);
    }
    None
}

fn render_name(
    orientation: LayoutOrientation,
    alignment: Option<LayoutAlignment>,
    count: usize,
) -> String {
    let shape = match orientation {
        LayoutOrientation::Row => "row",
        LayoutOrientation::Column => "column",
        LayoutOrientation::Grid => "grid",
        LayoutOrientation::Cloud => "group",
    };
    let suffix = match alignment {
        Some(LayoutAlignment::Left) => "-aligned-left",
        Some(LayoutAlignment::Right) => "-aligned-right",
        Some(LayoutAlignment::Top) => "-aligned-top",
        Some(LayoutAlignment::Bottom) => "-aligned-bottom",
        Some(LayoutAlignment::CenterHorizontal) => "-centered-horizontal",
        Some(LayoutAlignment::CenterVertical) => "-centered-vertical",
        None => "",
    };
    format!("{shape}-of-{count}{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(x: f32, y: f32, w: f32, h: f32) -> LayoutNode {
        LayoutNode {
            id: Uuid::new_v4(),
            bounds: Bounds {
                x,
                y,
                width: w,
                height: h,
            },
        }
    }

    #[test]
    fn rejects_single_node() {
        let nodes = vec![node(0.0, 0.0, 10.0, 10.0)];
        let err =
            suggest_layout_grouping(&nodes, LayoutSuggestOptions::default()).expect_err("err");
        assert!(matches!(err, LayoutSuggestError::TooFewNodes));
    }

    #[test]
    fn rejects_negative_dimensions() {
        let nodes = vec![
            node(0.0, 0.0, 10.0, 10.0),
            LayoutNode {
                id: Uuid::new_v4(),
                bounds: Bounds {
                    x: 50.0,
                    y: 0.0,
                    width: -1.0,
                    height: 10.0,
                },
            },
        ];
        let err =
            suggest_layout_grouping(&nodes, LayoutSuggestOptions::default()).expect_err("err");
        assert!(matches!(err, LayoutSuggestError::InvalidBounds));
    }

    #[test]
    fn three_buttons_in_a_row_cluster_as_row_aligned_top() {
        let nodes = vec![
            node(0.0, 100.0, 60.0, 32.0),
            node(80.0, 100.0, 60.0, 32.0),
            node(160.0, 100.0, 60.0, 32.0),
        ];
        let s = suggest_layout_grouping(&nodes, LayoutSuggestOptions::default()).expect("ok");
        assert_eq!(s.len(), 1, "expected 1 cluster, got {s:#?}");
        let g = &s[0];
        assert_eq!(g.member_ids.len(), 3);
        assert_eq!(g.orientation, LayoutOrientation::Row);
        assert_eq!(g.alignment, Some(LayoutAlignment::Top));
        assert!(
            g.name.starts_with("row-of-3"),
            "name should describe the row, got {:?}",
            g.name
        );
    }

    #[test]
    fn aligned_form_fields_cluster_as_column_aligned_left() {
        // Three text fields stacked vertically, all left-aligned at
        // x=20, with gaps that exceed proximity epsilon — the
        // alignment short-circuit MUST merge them.
        let nodes = vec![
            node(20.0, 0.0, 200.0, 30.0),
            node(20.0, 80.0, 200.0, 30.0),
            node(20.0, 160.0, 200.0, 30.0),
        ];
        let s = suggest_layout_grouping(&nodes, LayoutSuggestOptions::default()).expect("ok");
        assert_eq!(s.len(), 1, "expected 1 cluster, got {s:#?}");
        let g = &s[0];
        assert_eq!(g.member_ids.len(), 3);
        assert_eq!(g.orientation, LayoutOrientation::Column);
        assert_eq!(g.alignment, Some(LayoutAlignment::Left));
    }

    #[test]
    fn distant_unrelated_nodes_stay_separate() {
        let nodes = vec![
            node(0.0, 0.0, 20.0, 20.0),
            node(30.0, 0.0, 20.0, 20.0),
            // Big gap, different alignment.
            node(900.0, 900.0, 20.0, 20.0),
            node(930.0, 900.0, 20.0, 20.0),
        ];
        let s = suggest_layout_grouping(&nodes, LayoutSuggestOptions::default()).expect("ok");
        assert_eq!(s.len(), 2, "expected 2 clusters, got {s:#?}");
    }

    #[test]
    fn singleton_cluster_filtered_out_by_min_cluster_size() {
        let nodes = vec![
            node(0.0, 0.0, 20.0, 20.0),
            node(30.0, 0.0, 20.0, 20.0),
            // Lonely outlier nowhere near anything.
            node(900.0, 900.0, 20.0, 20.0),
        ];
        let s = suggest_layout_grouping(
            &nodes,
            LayoutSuggestOptions {
                min_cluster_size: 2,
                ..Default::default()
            },
        )
        .expect("ok");
        assert_eq!(s.len(), 1, "lone outlier should be dropped: {s:#?}");
        assert_eq!(s[0].member_ids.len(), 2);
    }

    #[test]
    fn grid_layout_detected_as_grid() {
        let mut nodes = Vec::new();
        for row in 0..3 {
            for col in 0..3 {
                nodes.push(node(col as f32 * 80.0, row as f32 * 80.0, 60.0, 60.0));
            }
        }
        let s = suggest_layout_grouping(&nodes, LayoutSuggestOptions::default()).expect("ok");
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].member_ids.len(), 9);
        assert_eq!(s[0].orientation, LayoutOrientation::Grid);
    }

    #[test]
    fn bounds_union_wraps_cluster() {
        let nodes = vec![node(10.0, 20.0, 30.0, 40.0), node(70.0, 20.0, 30.0, 40.0)];
        let s = suggest_layout_grouping(&nodes, LayoutSuggestOptions::default()).expect("ok");
        let b = s[0].bounds;
        // Min corner is (10, 20), max corner is (100, 60).
        assert!((b.x - 10.0).abs() < 0.01);
        assert!((b.y - 20.0).abs() < 0.01);
        assert!((b.width - 90.0).abs() < 0.01);
        assert!((b.height - 40.0).abs() < 0.01);
    }

    #[test]
    fn member_ids_are_stably_sorted() {
        let a = Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap();
        let b = Uuid::parse_str("22222222-2222-2222-2222-222222222222").unwrap();
        let nodes = vec![
            LayoutNode {
                id: b,
                bounds: Bounds {
                    x: 0.0,
                    y: 0.0,
                    width: 10.0,
                    height: 10.0,
                },
            },
            LayoutNode {
                id: a,
                bounds: Bounds {
                    x: 30.0,
                    y: 0.0,
                    width: 10.0,
                    height: 10.0,
                },
            },
        ];
        let s = suggest_layout_grouping(&nodes, LayoutSuggestOptions::default()).expect("ok");
        assert_eq!(s[0].member_ids, vec![a, b]);
    }
}
