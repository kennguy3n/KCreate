//! Phase 11 Block A Task 1 — `DocumentGraph` dirty-set / structure
//! tracking integration tests.
//!
//! The Phase 11 incremental scene-sync path (Task 2) drains the
//! dirty set on every sync to decide which cache entries to evict.
//! These tests pin the contract that mutating methods feed that set
//! correctly and that the structural-dirty flag flips for
//! tree-shape changes but stays put for property-only edits.

use kcreate_core::document::DocumentGraph;
use kcreate_core::node::{Bounds, Node, NodeType};

#[test]
fn insert_marks_node_and_structure_dirty() {
    let mut doc = DocumentGraph::new();
    let id = doc
        .insert_node(Node::new(NodeType::Artboard, "Artboard"))
        .unwrap();
    let (dirty, structural) = doc.drain_dirty();
    assert!(dirty.contains(&id), "insert must mark the new node dirty");
    assert!(
        structural,
        "insert must flip structure_dirty (tree shape changed)"
    );
}

#[test]
fn get_node_mut_marks_node_dirty_without_structure() {
    let mut doc = DocumentGraph::new();
    let id = doc.insert_node(Node::new(NodeType::Artboard, "a")).unwrap();
    let _ = doc.drain_dirty();

    {
        let n = doc.get_node_mut(id).unwrap();
        n.bounds = Bounds::new(0.0, 0.0, 100.0, 100.0);
        n.touch();
    }
    let (dirty, structural) = doc.drain_dirty();
    assert!(
        dirty.contains(&id),
        "get_node_mut must mark the borrowed node dirty"
    );
    assert!(
        !structural,
        "property edit must NOT flip structure_dirty (tree shape unchanged)"
    );
}

#[test]
fn remove_marks_structure_dirty_and_clears_id_from_dirty() {
    let mut doc = DocumentGraph::new();
    let id = doc.insert_node(Node::new(NodeType::Artboard, "a")).unwrap();
    let _ = doc.drain_dirty();

    let removed = doc.remove_node(id);
    assert!(removed.is_some(), "remove must succeed on a present node");
    let (_dirty, structural) = doc.drain_dirty();
    assert!(structural, "remove must flip structure_dirty");
}

#[test]
fn drain_dirty_clears_the_set() {
    let mut doc = DocumentGraph::new();
    let _ = doc.insert_node(Node::new(NodeType::Artboard, "a")).unwrap();
    let (first, _) = doc.drain_dirty();
    assert!(!first.is_empty(), "drain after insert returns dirty ids");
    let (second, structural) = doc.drain_dirty();
    assert!(
        second.is_empty(),
        "consecutive drain returns empty set, got {second:?}"
    );
    assert!(!structural, "drain clears the structural flag too");
}

#[test]
fn mark_dirty_round_trips() {
    let mut doc = DocumentGraph::new();
    let id = doc.insert_node(Node::new(NodeType::Artboard, "a")).unwrap();
    let _ = doc.drain_dirty();

    doc.mark_dirty(id);
    let (dirty, structural) = doc.drain_dirty();
    assert!(
        dirty.contains(&id),
        "explicit mark_dirty must populate the set"
    );
    assert!(
        !structural,
        "explicit mark_dirty must NOT flip structure_dirty"
    );
}
