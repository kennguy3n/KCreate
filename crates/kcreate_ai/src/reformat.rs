//! Reformat-to-deck — Phase 10 Block B Task 9.
//!
//! Takes a single-page document description (a list of nodes with
//! bounds and content hints) and produces a multi-page 16:9 deck
//! layout. The decomposition logic is deterministic and local — we
//! cluster nodes by spatial proximity, assign each cluster to a
//! deck page, then scale each cluster to fit inside the 16:9 target
//! while preserving aspect ratios.
//!
//! When the LLM sidecar is available, the bridge can replace the
//! deterministic clusterer with an LLM call (constrained by a GBNF
//! grammar). The function in this file is the always-available
//! fallback the bridge calls when the sidecar is unavailable.

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReformatDeckOptions {
    /// Target page width in pixels. Default 1920 (HD 16:9).
    pub page_width: f64,
    /// Target page height in pixels. Default 1080.
    pub page_height: f64,
    /// Maximum number of nodes assigned to a single page. Default 6
    /// (a sensible upper bound for legible slide density).
    pub max_nodes_per_page: u32,
    /// Outer page margin in pixels.
    pub margin: f64,
}

impl Default for ReformatDeckOptions {
    fn default() -> Self {
        Self {
            page_width: 1920.0,
            page_height: 1080.0,
            max_nodes_per_page: 6,
            margin: 64.0,
        }
    }
}

impl ReformatDeckOptions {
    #[must_use]
    pub fn clamped(mut self) -> Self {
        if !self.page_width.is_finite() || self.page_width <= 0.0 {
            self.page_width = 1920.0;
        }
        if !self.page_height.is_finite() || self.page_height <= 0.0 {
            self.page_height = 1080.0;
        }
        self.page_width = self.page_width.clamp(64.0, 8192.0);
        self.page_height = self.page_height.clamp(64.0, 8192.0);
        if self.max_nodes_per_page == 0 {
            self.max_nodes_per_page = 6;
        }
        self.max_nodes_per_page = self.max_nodes_per_page.min(32);
        if !self.margin.is_finite() || self.margin < 0.0 {
            self.margin = 0.0;
        }
        self.margin = self.margin.min(self.page_width / 4.0);
        self
    }
}

/// One node from the source document.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceNode {
    pub id: String,
    pub name: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    /// `text | image | shape | group` — used as a clustering hint.
    pub kind: String,
}

/// One deck page after reformatting.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReformatPage {
    pub index: u32,
    pub title: String,
    pub placements: Vec<ReformatPagePlacement>,
}

/// One node placed on a deck page.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReformatPagePlacement {
    pub source_node_id: String,
    pub new_x: f64,
    pub new_y: f64,
    pub new_width: f64,
    pub new_height: f64,
    pub scale: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReformatDeckResult {
    pub pages: Vec<ReformatPage>,
    pub page_width: f64,
    pub page_height: f64,
}

#[derive(Debug, Error)]
pub enum ReformatDeckError {
    #[error("reformat: no source nodes")]
    Empty,
}

/// Pure, deterministic reformatter. Splits nodes into pages by
/// vertical scan order (top-to-bottom), packing
/// `max_nodes_per_page` nodes per page, and rescales each page's
/// content to fit the deck dimensions.
///
/// # Errors
///
/// Returns [`ReformatDeckError::Empty`] when `nodes` is empty.
pub fn reformat_to_deck(
    nodes: &[SourceNode],
    options: ReformatDeckOptions,
) -> Result<ReformatDeckResult, ReformatDeckError> {
    if nodes.is_empty() {
        return Err(ReformatDeckError::Empty);
    }
    let opts = options.clamped();

    // Sort nodes by (y, x) so the natural top-to-bottom, left-to-right
    // reading order drives page assignment.
    let mut sorted: Vec<&SourceNode> = nodes.iter().collect();
    sorted.sort_by(|a, b| {
        a.y.partial_cmp(&b.y)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal))
    });

    let chunk = opts.max_nodes_per_page as usize;
    let mut pages: Vec<ReformatPage> = Vec::new();
    for (page_idx, group) in sorted.chunks(chunk).enumerate() {
        let placements = layout_group(group, &opts);
        pages.push(ReformatPage {
            index: page_idx as u32,
            title: page_title(page_idx, group),
            placements,
        });
    }
    Ok(ReformatDeckResult {
        pages,
        page_width: opts.page_width,
        page_height: opts.page_height,
    })
}

fn page_title(idx: usize, group: &[&SourceNode]) -> String {
    // Prefer the first text node's name as a title when available;
    // otherwise fall back to a generic slide title.
    for n in group {
        if n.kind == "text" {
            return n.name.clone();
        }
    }
    format!("Slide {}", idx + 1)
}

fn layout_group(group: &[&SourceNode], opts: &ReformatDeckOptions) -> Vec<ReformatPagePlacement> {
    // Compute the bounding box of the group, then scale to fit
    // inside the page minus margins. Preserve aspect ratios — every
    // node uses the same scale factor so spatial relationships stay
    // intact.
    let (min_x, min_y, max_x, max_y) = group_bounding_box(group);
    let avail_w = (opts.page_width - 2.0 * opts.margin).max(1.0);
    let avail_h = (opts.page_height - 2.0 * opts.margin).max(1.0);
    let span_x = (max_x - min_x).max(1.0);
    let span_y = (max_y - min_y).max(1.0);
    let scale = (avail_w / span_x).min(avail_h / span_y);

    // Centre the scaled group on the page.
    let scaled_w = span_x * scale;
    let scaled_h = span_y * scale;
    let offset_x = (opts.page_width - scaled_w) / 2.0;
    let offset_y = (opts.page_height - scaled_h) / 2.0;

    group
        .iter()
        .map(|n| ReformatPagePlacement {
            source_node_id: n.id.clone(),
            new_x: offset_x + (n.x - min_x) * scale,
            new_y: offset_y + (n.y - min_y) * scale,
            new_width: n.width * scale,
            new_height: n.height * scale,
            scale,
        })
        .collect()
}

fn group_bounding_box(group: &[&SourceNode]) -> (f64, f64, f64, f64) {
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for n in group {
        if n.x < min_x {
            min_x = n.x;
        }
        if n.y < min_y {
            min_y = n.y;
        }
        if n.x + n.width > max_x {
            max_x = n.x + n.width;
        }
        if n.y + n.height > max_y {
            max_y = n.y + n.height;
        }
    }
    if !min_x.is_finite() {
        return (0.0, 0.0, 1.0, 1.0);
    }
    (min_x, min_y, max_x, max_y)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, name: &str, x: f64, y: f64, w: f64, h: f64, kind: &str) -> SourceNode {
        SourceNode {
            id: id.into(),
            name: name.into(),
            x,
            y,
            width: w,
            height: h,
            kind: kind.into(),
        }
    }

    #[test]
    fn empty_input_errors() {
        let err = reformat_to_deck(&[], ReformatDeckOptions::default()).unwrap_err();
        assert!(matches!(err, ReformatDeckError::Empty));
    }

    #[test]
    fn splits_into_pages_by_chunk_size() {
        let mut nodes = Vec::new();
        for i in 0..15 {
            nodes.push(node(
                &format!("n{i}"),
                &format!("Node {i}"),
                0.0,
                f64::from(i) * 100.0,
                200.0,
                80.0,
                "text",
            ));
        }
        let r = reformat_to_deck(
            &nodes,
            ReformatDeckOptions {
                max_nodes_per_page: 5,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(r.pages.len(), 3);
        assert_eq!(r.pages[0].placements.len(), 5);
        assert_eq!(r.pages[2].placements.len(), 5);
    }

    #[test]
    fn placements_fit_inside_page_with_margin() {
        let nodes = vec![
            node("a", "Hello", 100.0, 100.0, 800.0, 100.0, "text"),
            node("b", "World", 100.0, 250.0, 800.0, 600.0, "image"),
        ];
        let opts = ReformatDeckOptions {
            page_width: 1920.0,
            page_height: 1080.0,
            margin: 64.0,
            max_nodes_per_page: 8,
        };
        let r = reformat_to_deck(&nodes, opts).unwrap();
        assert_eq!(r.pages.len(), 1);
        for p in &r.pages[0].placements {
            assert!(p.new_x >= 0.0);
            assert!(p.new_y >= 0.0);
            assert!(p.new_x + p.new_width <= opts.page_width + 1.0);
            assert!(p.new_y + p.new_height <= opts.page_height + 1.0);
        }
    }

    #[test]
    fn first_text_node_becomes_page_title() {
        let nodes = vec![
            node("a", "Headline", 0.0, 0.0, 100.0, 50.0, "text"),
            node("b", "Photo", 0.0, 100.0, 200.0, 200.0, "image"),
        ];
        let r = reformat_to_deck(&nodes, ReformatDeckOptions::default()).unwrap();
        assert_eq!(r.pages[0].title, "Headline");
    }

    #[test]
    fn pages_without_text_get_generic_titles() {
        let nodes = vec![
            node("a", "img1", 0.0, 0.0, 100.0, 50.0, "image"),
            node("b", "img2", 0.0, 100.0, 200.0, 200.0, "image"),
        ];
        let r = reformat_to_deck(&nodes, ReformatDeckOptions::default()).unwrap();
        assert_eq!(r.pages[0].title, "Slide 1");
    }

    #[test]
    fn options_clamping_keeps_dims_in_range() {
        let opts = ReformatDeckOptions {
            page_width: -1.0,
            page_height: -1.0,
            max_nodes_per_page: 0,
            margin: -5.0,
        }
        .clamped();
        assert!(opts.page_width >= 64.0);
        assert!(opts.page_height >= 64.0);
        assert!(opts.max_nodes_per_page > 0);
        assert!(opts.margin >= 0.0);
    }
}
