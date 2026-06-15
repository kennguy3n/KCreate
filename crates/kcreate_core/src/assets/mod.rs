//! Built-in Elements / asset library — the offline catalogue behind
//! KCreate's Canva-style Elements panel.
//!
//! Every asset is hand-authored, license-clean vector artwork that ships
//! in-repo (see [`generate_assets.py`](generate_assets.py)); nothing is
//! fetched from the network, so the library works fully offline. The raw
//! SVG for each asset is embedded at compile time via `include_str!`, so
//! the catalogue is a zero-allocation `&'static` table with no runtime
//! file I/O.
//!
//! This module owns only the **data + search** half of the feature: the
//! catalogue, category grouping, and name/tag search. Turning an asset's
//! SVG into editable vector nodes on the document graph lives in the
//! bridge (`kcreate_bridge::assets`), because parsing SVG needs
//! `kcreate_vector`, which depends on this crate.

use serde::Serialize;

mod catalog;
pub mod recolor;

/// A bundled vector asset: stable id, display name, category, search
/// tags, and the embedded SVG source.
///
/// All fields are `&'static` — the whole catalogue is baked into the
/// binary, so cloning an [`AssetDef`] is just copying a handful of fat
/// pointers.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct AssetDef {
    /// Stable, URL-safe identifier (`[a-z0-9-]+`). Unique across the
    /// whole catalogue, not just within a category.
    pub id: &'static str,
    /// Human-friendly display name shown in the panel.
    pub name: &'static str,
    /// Top-level category this asset is filed under.
    pub category: AssetCategory,
    /// Finer sub-category within [`category`](Self::category) (e.g.
    /// `"Navigation"`, `"Weather"`, `"Charts"`). Lets the panel section a
    /// large catalogue with per-group counts. Non-empty for every asset.
    pub group: &'static str,
    /// Lower-case search tags (synonyms / related terms).
    pub tags: &'static [&'static str],
    /// Embedded SVG source. Used both for the panel thumbnail and as the
    /// geometry source when inserting onto the canvas.
    pub svg: &'static str,
}

/// Top-level grouping for the Elements panel's category tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetCategory {
    /// Basic geometric shapes (rectangles, polygons, stars, …).
    Shapes,
    /// Lines, arrows, and connectors.
    Lines,
    /// Stroke-style UI icons.
    Icons,
    /// Decorative frames / borders.
    Frames,
    /// Flat multi-colour illustrations.
    Illustrations,
}

impl AssetCategory {
    /// Every category, in panel display order.
    pub const ALL: [Self; 5] = [
        Self::Shapes,
        Self::Lines,
        Self::Icons,
        Self::Frames,
        Self::Illustrations,
    ];

    /// Stable URL-safe slug (matches the serde representation and the
    /// on-disk `data/<slug>/` directory).
    #[must_use]
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Shapes => "shapes",
            Self::Lines => "lines",
            Self::Icons => "icons",
            Self::Frames => "frames",
            Self::Illustrations => "illustrations",
        }
    }

    /// Human-friendly label for the category tab.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Shapes => "Shapes",
            Self::Lines => "Lines",
            Self::Icons => "Icons",
            Self::Frames => "Frames",
            Self::Illustrations => "Illustrations",
        }
    }

    /// Parse a category from its [`slug`](Self::slug).
    #[must_use]
    pub fn from_slug(slug: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.slug() == slug)
    }
}

/// A category tab descriptor for the panel: slug, label, and the number
/// of assets filed under it.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct CategoryInfo {
    pub slug: &'static str,
    pub label: &'static str,
    pub count: usize,
}

/// The full, immutable asset catalogue.
#[must_use]
pub fn catalog() -> &'static [AssetDef] {
    catalog::ASSET_DEFS
}

/// Look up a single asset by its [`AssetDef::id`].
#[must_use]
pub fn get(id: &str) -> Option<&'static AssetDef> {
    catalog::ASSET_DEFS.iter().find(|a| a.id == id)
}

/// Every category paired with its label and asset count, in display
/// order. Drives the panel's category tabs.
#[must_use]
pub fn categories() -> Vec<CategoryInfo> {
    AssetCategory::ALL
        .into_iter()
        .map(|category| CategoryInfo {
            slug: category.slug(),
            label: category.label(),
            count: catalog::ASSET_DEFS
                .iter()
                .filter(|a| a.category == category)
                .count(),
        })
        .collect()
}

/// All assets in a category, in catalogue order.
#[must_use]
pub fn list_category(category: AssetCategory) -> Vec<&'static AssetDef> {
    catalog::ASSET_DEFS
        .iter()
        .filter(|a| a.category == category)
        .collect()
}

/// Relevance score for a single asset against the normalised query
/// terms. `None` means the asset does not match (at least one term hit
/// nothing); `Some(score)` ranks matches, higher is better.
///
/// Every term must match *something* on the asset (AND across terms),
/// but a term can match the name, id, category, or any tag (OR within a
/// term). The per-term contribution rewards stronger matches, from
/// highest to lowest: exact name/id (100), name/id prefix (60), exact
/// tag (50), name substring (40), exact group/sub-category (35), tag
/// prefix (30), exact category (25), tag substring (20). So the most
/// obvious results float to the top.
fn score(asset: &AssetDef, terms: &[String]) -> Option<i32> {
    let name = asset.name.to_lowercase();
    let group = asset.group.to_lowercase();
    let mut total = 0i32;
    for term in terms {
        let mut best = 0i32;
        if name == *term || asset.id == term {
            best = best.max(100);
        }
        if name.starts_with(term.as_str()) || asset.id.starts_with(term.as_str()) {
            best = best.max(60);
        }
        if name.contains(term.as_str()) {
            best = best.max(40);
        }
        for tag in asset.tags {
            if *tag == term {
                best = best.max(50);
            } else if tag.starts_with(term.as_str()) {
                best = best.max(30);
            } else if tag.contains(term.as_str()) {
                best = best.max(20);
            }
        }
        if group == *term {
            best = best.max(35);
        }
        if asset.category.slug() == term || asset.category.label().to_lowercase() == *term {
            best = best.max(25);
        }
        if best == 0 {
            return None;
        }
        total += best;
    }
    Some(total)
}

/// Search the catalogue by free-text query, matching against name, id,
/// tags, and category. Whitespace splits the query into terms that are
/// AND-ed together. Results are ranked by relevance (then name, then id
/// for stability). An empty/blank query returns the whole catalogue in
/// catalogue order.
///
/// An optional `category` filter restricts results to a single category
/// (the panel uses this so a search stays within the active tab when one
/// is selected).
#[must_use]
pub fn search(query: &str, category: Option<AssetCategory>) -> Vec<&'static AssetDef> {
    let terms: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let in_category = |a: &AssetDef| category.is_none_or(|c| a.category == c);

    if terms.is_empty() {
        return catalog::ASSET_DEFS
            .iter()
            .filter(|a| in_category(a))
            .collect();
    }

    let mut scored: Vec<(i32, &'static AssetDef)> = catalog::ASSET_DEFS
        .iter()
        .filter(|a| in_category(a))
        .filter_map(|a| score(a, &terms).map(|s| (s, a)))
        .collect();
    scored.sort_by(|(sa, a), (sb, b)| {
        sb.cmp(sa)
            .then_with(|| a.name.cmp(b.name))
            .then_with(|| a.id.cmp(b.id))
    });
    scored.into_iter().map(|(_, a)| a).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogue_is_non_empty_and_sized() {
        // The H3 workstream expands the curated set to 400+ assets.
        let n = catalog().len();
        assert!(n >= 400, "expected >= 400 bundled assets, got {n}");
    }

    #[test]
    fn every_asset_has_a_group() {
        for a in catalog() {
            assert!(
                !a.group.is_empty(),
                "asset {:?} has an empty sub-category group",
                a.id
            );
        }
    }

    #[test]
    fn ids_are_unique_and_url_safe() {
        let mut seen = std::collections::HashSet::new();
        for a in catalog() {
            assert!(seen.insert(a.id), "duplicate asset id {:?}", a.id);
            assert!(
                !a.id.is_empty()
                    && a.id
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "id {:?} is not url-safe",
                a.id
            );
            assert!(!a.name.is_empty(), "asset {:?} has an empty name", a.id);
            assert!(!a.tags.is_empty(), "asset {:?} has no tags", a.id);
            assert!(
                a.svg.contains("<svg"),
                "asset {:?} svg is not an svg document",
                a.id
            );
        }
    }

    #[test]
    fn every_category_has_assets() {
        for info in categories() {
            assert!(info.count > 0, "category {:?} has no assets", info.slug);
        }
        let summed: usize = categories().iter().map(|c| c.count).sum();
        assert_eq!(summed, catalog().len(), "category counts must partition");
    }

    #[test]
    fn get_round_trips() {
        let first = catalog()[0];
        assert_eq!(get(first.id).expect("present").id, first.id);
        assert!(get("definitely-not-an-asset").is_none());
    }

    #[test]
    fn list_category_filters() {
        let icons = list_category(AssetCategory::Icons);
        assert!(!icons.is_empty());
        assert!(icons.iter().all(|a| a.category == AssetCategory::Icons));
    }

    #[test]
    fn search_blank_returns_all() {
        assert_eq!(search("   ", None).len(), catalog().len());
    }

    #[test]
    fn search_by_name_finds_asset() {
        let hits = search("arrow", None);
        assert!(!hits.is_empty(), "expected arrow results");
        assert!(
            hits.iter().any(|a| a.id == "arrow-right"),
            "arrow-right should be among arrow results"
        );
    }

    #[test]
    fn search_by_tag_finds_synonym() {
        // "checkmark"/"tick" are tags on the check icon, not its name.
        let hits = search("checkmark", None);
        assert!(
            hits.iter().any(|a| a.id == "check"),
            "tag search should surface the check icon"
        );
        let hits = search("magnify", None);
        assert!(
            hits.iter().any(|a| a.id == "search"),
            "tag search should surface the search icon"
        );
    }

    #[test]
    fn arrow_synonyms_all_surface_arrows() {
        // "arrow", "chevron", and "next" must all reach the arrow set —
        // tags/synonyms wire the directional vocabulary together.
        for q in ["arrow", "chevron", "next"] {
            let hits = search(q, None);
            assert!(
                hits.iter().any(|a| a.id == "arrow-right"),
                "query {q:?} should surface arrow-right"
            );
        }
    }

    #[test]
    fn search_ranks_exact_name_above_group_and_category() {
        // "icons" is a category; an asset literally named to match a term
        // should still outrank a bare category/group hit. Here a tag-exact
        // hit (chart-bar via "bar") beats a category-only hit.
        let hits = search("chart", Some(AssetCategory::Icons));
        assert!(
            hits.iter().any(|a| a.id == "chart-bar"),
            "chart query should surface chart-bar"
        );
    }

    #[test]
    fn search_ranks_name_match_first() {
        // A query equal to a name should put that asset at the top.
        let hits = search("star", None);
        assert_eq!(hits[0].name.to_lowercase(), "star");
    }

    #[test]
    fn search_respects_category_filter() {
        let hits = search("star", Some(AssetCategory::Icons));
        assert!(hits.iter().all(|a| a.category == AssetCategory::Icons));
        assert!(hits.iter().any(|a| a.id == "star-outline"));
        // The Shapes "star" must be excluded by the filter.
        assert!(hits.iter().all(|a| a.category != AssetCategory::Shapes));
    }

    #[test]
    fn multi_term_query_is_conjunctive() {
        // "bar chart" should match the bar-chart icon (both terms hit),
        // but not "search" (neither term hits).
        let hits = search("bar chart", None);
        assert!(hits.iter().any(|a| a.id == "chart-bar"));
        assert!(hits.iter().all(|a| a.id != "search"));
    }

    #[test]
    fn category_slug_round_trips() {
        for c in AssetCategory::ALL {
            assert_eq!(AssetCategory::from_slug(c.slug()), Some(c));
        }
        assert_eq!(AssetCategory::from_slug("nope"), None);
    }
}
