//! Phase 8 Block D: design-token binding.
//!
//! Cross-crate test that exercises [`kcreate_core::token_binding`]
//! with the [`DocumentGraph`] to verify bind/unbind round-trip,
//! propagation of token changes, and that detached nodes are
//! unaffected.

use kcreate_core::document::DocumentGraph;
use kcreate_core::node::{FillStyle, Node, NodeType, RgbaColor};
use kcreate_core::project::DesignTokens;
use kcreate_core::token_binding::{
    bind_token, nodes_bound_to, propagate_single_token, unbind_token,
};

fn tokens() -> DesignTokens {
    let mut t = DesignTokens::default();
    t.colors
        .insert("brand/primary".into(), RgbaColor::new(0.1, 0.2, 0.3, 1.0));
    t.radii.insert("md".into(), 8.0);
    t.spacing.insert("sm".into(), 4.0);
    t
}

#[test]
fn bind_and_unbind_round_trip() {
    let t = tokens();
    let mut n = Node::new(NodeType::VectorLayer, "node");
    bind_token(&mut n.style, "fill", "brand/primary", &t).unwrap();
    assert!(n.style.token_bindings.contains_key("fill"));
    match n.style.fill {
        FillStyle::Solid(c) => assert!((c.r - 0.1).abs() < 1e-6),
        _ => panic!("fill should be solid"),
    }
    unbind_token(&mut n.style, "fill");
    assert!(!n.style.token_bindings.contains_key("fill"));
}

#[test]
fn token_change_propagates_to_bound_nodes() {
    let mut t = tokens();
    let mut doc = DocumentGraph::new();
    let mut a = Node::new(NodeType::VectorLayer, "A");
    bind_token(&mut a.style, "fill", "brand/primary", &t).unwrap();
    let a_id = a.id;
    doc.insert_node(a).unwrap();
    t.colors
        .insert("brand/primary".into(), RgbaColor::new(0.9, 0.0, 0.0, 1.0));
    let touched = propagate_single_token(&mut doc, "brand/primary", &t);
    assert_eq!(touched, 1);
    match doc.get_node(a_id).unwrap().style.fill {
        FillStyle::Solid(c) => assert!((c.r - 0.9).abs() < 1e-6),
        _ => panic!("fill should be solid"),
    }
}

#[test]
fn detached_node_unaffected_by_propagation() {
    let mut t = tokens();
    let mut doc = DocumentGraph::new();
    let mut a = Node::new(NodeType::VectorLayer, "bound");
    bind_token(&mut a.style, "fill", "brand/primary", &t).unwrap();
    let a_id = a.id;
    doc.insert_node(a).unwrap();
    // Unbind before the token changes.
    {
        let node = doc.get_node_mut(a_id).unwrap();
        unbind_token(&mut node.style, "fill");
    }
    t.colors
        .insert("brand/primary".into(), RgbaColor::new(0.9, 0.0, 0.0, 1.0));
    let touched = propagate_single_token(&mut doc, "brand/primary", &t);
    assert_eq!(touched, 0);
    match doc.get_node(a_id).unwrap().style.fill {
        FillStyle::Solid(c) => assert!((c.r - 0.1).abs() < 1e-6, "should still be old color"),
        _ => panic!("fill should be solid"),
    }
}

#[test]
fn nodes_bound_to_returns_correct_ids() {
    let t = tokens();
    let mut doc = DocumentGraph::new();
    let mut a = Node::new(NodeType::VectorLayer, "A");
    bind_token(&mut a.style, "fill", "brand/primary", &t).unwrap();
    let a_id = a.id;
    let b = Node::new(NodeType::VectorLayer, "B");
    doc.insert_node(a).unwrap();
    doc.insert_node(b).unwrap();
    let subs = nodes_bound_to(&doc, "brand/primary");
    assert_eq!(subs.len(), 1);
    assert_eq!(subs[0], a_id);
}

#[test]
fn propagation_within_100ms_budget_for_1000_nodes() {
    let t = tokens();
    let mut doc = DocumentGraph::new();
    for i in 0..1_000 {
        let mut n = Node::new(NodeType::VectorLayer, format!("n{i}"));
        bind_token(&mut n.style, "fill", "brand/primary", &t).unwrap();
        doc.insert_node(n).unwrap();
    }
    let mut updated = t;
    updated
        .colors
        .insert("brand/primary".into(), RgbaColor::new(1.0, 0.0, 0.0, 1.0));
    let t0 = std::time::Instant::now();
    let touched = propagate_single_token(&mut doc, "brand/primary", &updated);
    let elapsed = t0.elapsed();
    assert_eq!(touched, 1_000);
    assert!(
        elapsed.as_millis() < 100,
        "propagation took {elapsed:?}, budget is 100ms"
    );
}
