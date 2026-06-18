//! Design-token bindings between [`Node`]s and project-level
//! [`DesignTokens`].
//!
//! A binding is a (property-name → token-id) pair stored on
//! [`NodeStyle::token_bindings`]. When a token changes (e.g. the
//! user edits the "Brand/Primary" color in the brand kit
//! editor), we walk every node that has a binding to that token
//! and rewrite the corresponding property in place. The acceptance
//! criterion is the OVERVIEW.md §4.6 budget: every linked layer
//! must be updated in < 100 ms even for thousand-node projects.
//!
//! The supported property names are listed in
//! [`StyleProperty::ALL`]. They map to:
//!
//! * `"fill"`              → primary fill color
//! * `"stroke_color"`      → primary stroke color
//! * `"corner_radius"`     → corner radius
//! * `"stroke_width"`      → primary stroke width
//!
//! The bridge layer exposes `document_bind_token` /
//! `document_unbind_token` / `document_update_design_token` which
//! call into this module.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::document::DocumentGraph;
use crate::node::{FillStyle, NodeStyle, RgbaColor, StrokeStyle};
use crate::project::DesignTokens;

/// Style properties that can be bound to a design token. The
/// string form is what shows up in
/// [`NodeStyle::token_bindings`] keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StyleProperty {
    Fill,
    StrokeColor,
    CornerRadius,
    StrokeWidth,
}

impl StyleProperty {
    /// All currently supported properties.
    pub const ALL: &'static [Self] = &[
        Self::Fill,
        Self::StrokeColor,
        Self::CornerRadius,
        Self::StrokeWidth,
    ];

    /// Stable string form (used as the map key in
    /// [`NodeStyle::token_bindings`] and in the bridge surface).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fill => "fill",
            Self::StrokeColor => "stroke_color",
            Self::CornerRadius => "corner_radius",
            Self::StrokeWidth => "stroke_width",
        }
    }

    /// Parse a stable property name back to a [`StyleProperty`].
    /// This is intentionally an inherent method, not a
    /// [`std::str::FromStr`] impl, because the failure mode is
    /// "unknown property" (no error payload needed) and we keep
    /// the API non-fallible-callsite friendly (e.g. the bridge
    /// returns `None` to the JS side rather than a stringly-typed
    /// error).
    #[must_use]
    pub fn parse_property(s: &str) -> Option<Self> {
        Some(match s {
            "fill" => Self::Fill,
            "stroke_color" => Self::StrokeColor,
            "corner_radius" => Self::CornerRadius,
            "stroke_width" => Self::StrokeWidth,
            _ => return None,
        })
    }

    /// Which kind of token can drive this property.
    #[must_use]
    pub const fn token_kind(self) -> TokenKind {
        match self {
            Self::Fill | Self::StrokeColor => TokenKind::Color,
            Self::CornerRadius => TokenKind::Radius,
            Self::StrokeWidth => TokenKind::Spacing,
        }
    }
}

/// Token categories. Used to validate that a binding's token id
/// resolves to a value of the right shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Color,
    Radius,
    Spacing,
}

#[derive(Debug, Error)]
pub enum BindError {
    #[error("unsupported style property: {0}")]
    UnsupportedProperty(String),
    #[error("token not found: {0}")]
    TokenNotFound(String),
    #[error("token kind mismatch: property {property} requires {expected:?}, got {actual:?}")]
    KindMismatch {
        property: &'static str,
        expected: TokenKind,
        actual: TokenKind,
    },
}

/// Add or replace a binding on `style` from `property` to `token`.
///
/// Returns `Err` when the property name isn't recognised or the
/// token doesn't exist / has the wrong type.
pub fn bind_token(
    style: &mut NodeStyle,
    property: &str,
    token_name: &str,
    tokens: &DesignTokens,
) -> Result<(), BindError> {
    let prop = StyleProperty::parse_property(property)
        .ok_or_else(|| BindError::UnsupportedProperty(property.to_string()))?;
    let actual_kind = classify_token(token_name, tokens)
        .ok_or_else(|| BindError::TokenNotFound(token_name.to_string()))?;
    let expected = prop.token_kind();
    if actual_kind != expected {
        return Err(BindError::KindMismatch {
            property: prop.as_str(),
            expected,
            actual: actual_kind,
        });
    }
    // Apply the value immediately so the binding is consistent
    // with the current state of `tokens` — i.e. no flash of stale
    // value until the next `propagate`.
    apply_one(style, prop, token_name, tokens);
    style
        .token_bindings
        .insert(prop.as_str().to_string(), token_name.to_string());
    Ok(())
}

/// Remove a binding for `property` from `style`. Returns the
/// previous token id (if any).
pub fn unbind_token(style: &mut NodeStyle, property: &str) -> Option<String> {
    let prop = StyleProperty::parse_property(property)?;
    style.token_bindings.remove(prop.as_str())
}

/// Re-apply every binding on `style` against the supplied
/// `tokens` table. Used after the brand kit changes.
///
/// Returns the number of properties that were actually rewritten.
/// Bindings referencing missing tokens are left alone — the user
/// can fix them in the panel without us silently dropping data.
pub fn refresh_style(style: &mut NodeStyle, tokens: &DesignTokens) -> usize {
    let bindings: Vec<(String, String)> = style
        .token_bindings
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let mut updates = 0usize;
    for (property, token) in bindings {
        let Some(prop) = StyleProperty::parse_property(&property) else {
            continue;
        };
        if classify_token(&token, tokens) == Some(prop.token_kind()) {
            apply_one(style, prop, &token, tokens);
            updates += 1;
        }
    }
    updates
}

/// Walk every node in `doc`, refreshing bound properties when the
/// referenced token exists in `tokens`. Returns the number of
/// nodes that had at least one binding refreshed — useful for the
/// "X layers updated" toast in the UI.
pub fn propagate_token_changes(doc: &mut DocumentGraph, tokens: &DesignTokens) -> usize {
    let mut touched = 0usize;
    let ids: Vec<Uuid> = doc.iter().map(|(id, _)| *id).collect();
    for id in ids {
        if let Some(node) = doc.get_node_mut(id) {
            if node.style.token_bindings.is_empty() {
                continue;
            }
            if refresh_style(&mut node.style, tokens) > 0 {
                touched += 1;
            }
        }
    }
    touched
}

/// Walk every node in `doc` that has a binding to `token_name`
/// and rewrite the corresponding property. Cheaper than the
/// general `propagate_token_changes` when only one token changed.
pub fn propagate_single_token(
    doc: &mut DocumentGraph,
    token_name: &str,
    tokens: &DesignTokens,
) -> usize {
    let actual_kind = match classify_token(token_name, tokens) {
        Some(k) => k,
        None => return 0,
    };
    let mut touched = 0usize;
    let ids: Vec<Uuid> = doc.iter().map(|(id, _)| *id).collect();
    for id in ids {
        if let Some(node) = doc.get_node_mut(id) {
            let prop_keys: Vec<(String, String)> = node
                .style
                .token_bindings
                .iter()
                .filter(|(_, v)| v.as_str() == token_name)
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            if prop_keys.is_empty() {
                continue;
            }
            let mut node_touched = false;
            for (property, token) in prop_keys {
                if let Some(prop) = StyleProperty::parse_property(&property) {
                    if prop.token_kind() == actual_kind {
                        apply_one(&mut node.style, prop, &token, tokens);
                        node_touched = true;
                    }
                }
            }
            if node_touched {
                touched += 1;
            }
        }
    }
    touched
}

/// Find every node id whose `token_bindings` references
/// `token_name`. Used by the UI to highlight downstream layers
/// before the user commits an edit.
#[must_use]
pub fn nodes_bound_to(doc: &DocumentGraph, token_name: &str) -> Vec<Uuid> {
    let mut out = Vec::new();
    for (id, node) in doc.iter() {
        if node.style.token_bindings.values().any(|v| v == token_name) {
            out.push(*id);
        }
    }
    out
}

/// Summary of `propagate_*` operations. Returned by the bridge so
/// the UI can show "X colors, Y spacings updated" toasts.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PropagationReport {
    pub nodes_touched: usize,
    pub by_property: HashMap<String, usize>,
}

fn classify_token(token_name: &str, tokens: &DesignTokens) -> Option<TokenKind> {
    if tokens.colors.contains_key(token_name) {
        return Some(TokenKind::Color);
    }
    if tokens.radii.contains_key(token_name) {
        return Some(TokenKind::Radius);
    }
    if tokens.spacing.contains_key(token_name) {
        return Some(TokenKind::Spacing);
    }
    None
}

fn apply_one(
    style: &mut NodeStyle,
    property: StyleProperty,
    token_name: &str,
    tokens: &DesignTokens,
) {
    match property {
        StyleProperty::Fill => {
            if let Some(color) = tokens.colors.get(token_name) {
                style.fill = FillStyle::Solid(*color);
            }
        }
        StyleProperty::StrokeColor => {
            if let Some(color) = tokens.colors.get(token_name) {
                ensure_stroke(style).color = *color;
            }
        }
        StyleProperty::CornerRadius => {
            if let Some(r) = tokens.radii.get(token_name) {
                style.corner_radius = f64::from(*r);
            }
        }
        StyleProperty::StrokeWidth => {
            if let Some(w) = tokens.spacing.get(token_name) {
                ensure_stroke(style).width = f64::from(*w);
            }
        }
    }
}

fn ensure_stroke(style: &mut NodeStyle) -> &mut StrokeStyle {
    if style.stroke.is_none() {
        style.stroke = Some(StrokeStyle {
            color: RgbaColor::new(0.0, 0.0, 0.0, 1.0),
            width: 1.0,
            ..StrokeStyle::default()
        });
    }
    style.stroke.as_mut().expect("just-inserted stroke")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::{Node, NodeType, RgbaColor};

    fn tokens_with(
        color: (&str, RgbaColor),
        radius: (&str, f32),
        spacing: (&str, f32),
    ) -> DesignTokens {
        let mut t = DesignTokens::default();
        t.colors.insert(color.0.to_string(), color.1);
        t.radii.insert(radius.0.to_string(), radius.1);
        t.spacing.insert(spacing.0.to_string(), spacing.1);
        t
    }

    #[test]
    fn property_strings_round_trip() {
        for prop in StyleProperty::ALL {
            assert_eq!(StyleProperty::parse_property(prop.as_str()), Some(*prop));
        }
    }

    #[test]
    fn bind_fill_to_color_token_applies_immediately() {
        let tokens = tokens_with(
            ("brand/primary", RgbaColor::new(0.1, 0.2, 0.3, 1.0)),
            ("none", 0.0),
            ("none", 0.0),
        );
        let mut style = NodeStyle::default();
        bind_token(&mut style, "fill", "brand/primary", &tokens).unwrap();
        match style.fill {
            FillStyle::Solid(c) => {
                assert!((c.r - 0.1).abs() < 1e-6);
                assert!((c.g - 0.2).abs() < 1e-6);
                assert!((c.b - 0.3).abs() < 1e-6);
            }
            _ => panic!("fill should be solid"),
        }
        assert_eq!(
            style.token_bindings.get("fill").map(String::as_str),
            Some("brand/primary")
        );
    }

    #[test]
    fn bind_rejects_unknown_property() {
        let tokens = DesignTokens::default();
        let err = bind_token(&mut NodeStyle::default(), "shadow", "x", &tokens).expect_err("err");
        assert!(matches!(err, BindError::UnsupportedProperty(_)));
    }

    #[test]
    fn bind_rejects_missing_token() {
        let tokens = DesignTokens::default();
        let err = bind_token(&mut NodeStyle::default(), "fill", "nope", &tokens).expect_err("err");
        assert!(matches!(err, BindError::TokenNotFound(_)));
    }

    #[test]
    fn bind_rejects_kind_mismatch() {
        let mut tokens = DesignTokens::default();
        tokens.spacing.insert("gap-1".into(), 4.0);
        let err = bind_token(&mut NodeStyle::default(), "fill", "gap-1", &tokens).expect_err("err");
        assert!(matches!(err, BindError::KindMismatch { .. }));
    }

    #[test]
    fn unbind_removes_existing_binding() {
        let tokens = tokens_with(
            ("c", RgbaColor::new(0.0, 0.0, 0.0, 1.0)),
            ("r", 0.0),
            ("s", 0.0),
        );
        let mut style = NodeStyle::default();
        bind_token(&mut style, "fill", "c", &tokens).unwrap();
        assert_eq!(unbind_token(&mut style, "fill").as_deref(), Some("c"));
        assert!(style.token_bindings.is_empty());
        // Second unbind is a noop and returns None.
        assert_eq!(unbind_token(&mut style, "fill"), None);
    }

    #[test]
    fn refresh_style_rewrites_bound_properties() {
        let mut tokens = tokens_with(
            ("brand/primary", RgbaColor::new(0.1, 0.2, 0.3, 1.0)),
            ("md", 8.0),
            ("sm", 4.0),
        );
        let mut style = NodeStyle::default();
        bind_token(&mut style, "fill", "brand/primary", &tokens).unwrap();
        bind_token(&mut style, "corner_radius", "md", &tokens).unwrap();
        bind_token(&mut style, "stroke_width", "sm", &tokens).unwrap();
        // Mutate every backing token, then refresh.
        tokens
            .colors
            .insert("brand/primary".into(), RgbaColor::new(0.9, 0.8, 0.7, 1.0));
        tokens.radii.insert("md".into(), 16.0);
        tokens.spacing.insert("sm".into(), 2.0);
        let n = refresh_style(&mut style, &tokens);
        assert_eq!(n, 3);
        match style.fill {
            FillStyle::Solid(c) => assert!((c.r - 0.9).abs() < 1e-6),
            _ => panic!(),
        }
        assert!((style.corner_radius - 16.0).abs() < 1e-9);
        assert!((style.stroke.as_ref().unwrap().width - 2.0).abs() < 1e-9);
    }

    #[test]
    fn refresh_skips_missing_or_mistyped_tokens() {
        // Bindings reference tokens that don't exist any more.
        // Refresh should leave the style alone but report 0 updates.
        let mut style = NodeStyle::default();
        style.token_bindings.insert("fill".into(), "ghost".into());
        let tokens = DesignTokens::default();
        let n = refresh_style(&mut style, &tokens);
        assert_eq!(n, 0);
        // Binding survives so the user can repair it.
        assert_eq!(style.token_bindings.len(), 1);
    }

    #[test]
    fn propagate_single_token_updates_only_bound_nodes() {
        let tokens = tokens_with(
            ("brand/primary", RgbaColor::new(0.5, 0.5, 0.5, 1.0)),
            ("md", 8.0),
            ("sm", 4.0),
        );
        let mut doc = DocumentGraph::new();
        let mut a = Node::new(NodeType::VectorLayer, "A");
        bind_token(&mut a.style, "fill", "brand/primary", &tokens).unwrap();
        let mut b = Node::new(NodeType::VectorLayer, "B");
        bind_token(&mut b.style, "corner_radius", "md", &tokens).unwrap();
        let a_id = a.id;
        let b_id = b.id;
        doc.insert_node(a).unwrap();
        doc.insert_node(b).unwrap();
        // Mutate one token; only A should be touched.
        let mut updated = tokens.clone();
        updated
            .colors
            .insert("brand/primary".into(), RgbaColor::new(0.9, 0.1, 0.1, 1.0));
        let touched = propagate_single_token(&mut doc, "brand/primary", &updated);
        assert_eq!(touched, 1);
        match doc.get_node(a_id).unwrap().style.fill {
            FillStyle::Solid(c) => assert!((c.r - 0.9).abs() < 1e-6),
            _ => panic!(),
        }
        assert!((doc.get_node(b_id).unwrap().style.corner_radius - 8.0).abs() < 1e-9);
    }

    #[test]
    fn propagate_meets_100ms_budget_on_1k_nodes() {
        let mut tokens = tokens_with(
            ("brand/primary", RgbaColor::new(0.0, 0.0, 0.0, 1.0)),
            ("md", 8.0),
            ("sm", 4.0),
        );
        let mut doc = DocumentGraph::new();
        for i in 0..1_000 {
            let mut n = Node::new(NodeType::VectorLayer, format!("n{i}"));
            bind_token(&mut n.style, "fill", "brand/primary", &tokens).unwrap();
            doc.insert_node(n).unwrap();
        }
        tokens
            .colors
            .insert("brand/primary".into(), RgbaColor::new(1.0, 0.0, 0.0, 1.0));
        let t0 = std::time::Instant::now();
        let touched = propagate_single_token(&mut doc, "brand/primary", &tokens);
        let elapsed = t0.elapsed();
        assert_eq!(touched, 1_000);
        assert!(
            elapsed.as_millis() < 100,
            "propagate took {elapsed:?}, budget is 100ms"
        );
    }

    #[test]
    fn nodes_bound_to_lists_all_subscribers() {
        let tokens = tokens_with(
            ("brand/primary", RgbaColor::new(0.0, 0.0, 0.0, 1.0)),
            ("md", 0.0),
            ("sm", 0.0),
        );
        let mut doc = DocumentGraph::new();
        let mut a = Node::new(NodeType::VectorLayer, "A");
        bind_token(&mut a.style, "fill", "brand/primary", &tokens).unwrap();
        let mut b = Node::new(NodeType::VectorLayer, "B");
        bind_token(&mut b.style, "fill", "brand/primary", &tokens).unwrap();
        let c = Node::new(NodeType::VectorLayer, "C"); // unbound
        let a_id = a.id;
        let b_id = b.id;
        doc.insert_node(a).unwrap();
        doc.insert_node(b).unwrap();
        doc.insert_node(c).unwrap();
        let subs = nodes_bound_to(&doc, "brand/primary");
        assert_eq!(subs.len(), 2);
        assert!(subs.contains(&a_id));
        assert!(subs.contains(&b_id));
    }
}
