//! Document graph — a flat `HashMap<Uuid, Node>` with explicit
//! parent/child references. O(1) node lookup.
//!
//! Cycle detection: every `reparent_node` walks the prospective parent
//! chain to ensure the operation does not introduce a cycle. Empty
//! children vectors are skipped, so the walk is O(depth).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::node::{Bounds, Node, NodeType};

/// Errors returned by [`DocumentGraph`].
#[derive(Debug, Error, PartialEq, Eq)]
pub enum DocumentError {
    #[error("node {0} not found in document")]
    NodeNotFound(Uuid),
    #[error("invalid reparent: target {target} is not a known node")]
    InvalidReparent { target: Uuid },
    #[error("reparent would introduce a cycle: {child} would become its own ancestor")]
    CycleDetected { child: Uuid },
    #[error("child {child} is not in parent {parent}")]
    ChildNotInParent { parent: Uuid, child: Uuid },
    #[error("reorder set does not match parent {parent}'s current children")]
    ReorderSetMismatch { parent: Uuid },
    #[error("node {id} has wrong type: expected {expected:?}, got {got:?}")]
    WrongNodeType {
        id: Uuid,
        expected: NodeType,
        got: NodeType,
    },
}

/// Result alias for document operations.
pub type Result<T> = std::result::Result<T, DocumentError>;

/// A flat key-value store of nodes plus a list of root ids.
///
/// Roots are typically the [`crate::node::NodeType::Page`] nodes of the
/// document. The root-id list lets us iterate top-level pages without
/// scanning every node.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DocumentGraph {
    nodes: HashMap<Uuid, Node>,
    root_ids: Vec<Uuid>,
}

impl DocumentGraph {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a graph from a pre-existing collection of nodes plus an
    /// explicit list of root ids. Used by the storage layer when
    /// reconstituting a saved document. Returns
    /// [`DocumentError::NodeNotFound`] if any `root_id` is missing
    /// from `nodes` and [`DocumentError::InvalidReparent`] if a node's
    /// `parent_id` is not present.
    pub fn from_parts(nodes: Vec<Node>, root_ids: Vec<Uuid>) -> Result<Self> {
        let mut map = HashMap::with_capacity(nodes.len());
        for node in nodes {
            map.insert(node.id, node);
        }
        for id in &root_ids {
            if !map.contains_key(id) {
                return Err(DocumentError::NodeNotFound(*id));
            }
        }
        // Validate parent references.
        for node in map.values() {
            if let Some(pid) = node.parent_id {
                if !map.contains_key(&pid) {
                    return Err(DocumentError::InvalidReparent { target: pid });
                }
            }
        }
        Ok(Self {
            nodes: map,
            root_ids,
        })
    }

    /// Insert a node and return its id. If the node has a parent set,
    /// the id is also appended to the parent's children list (caller is
    /// responsible for keeping parent-pointer and children-list in
    /// sync; this helper handles the common case).
    pub fn insert_node(&mut self, mut node: Node) -> Result<Uuid> {
        let id = node.id;
        let parent_id = node.parent_id;
        if let Some(pid) = parent_id {
            if !self.nodes.contains_key(&pid) {
                return Err(DocumentError::InvalidReparent { target: pid });
            }
        }
        node.touch();
        self.nodes.insert(id, node);
        if let Some(pid) = parent_id {
            if let Some(parent) = self.nodes.get_mut(&pid) {
                if !parent.children.contains(&id) {
                    parent.children.push(id);
                    parent.touch();
                }
            }
        } else if !self.root_ids.contains(&id) {
            self.root_ids.push(id);
        }
        Ok(id)
    }

    /// Borrow a node by id.
    #[must_use]
    pub fn get_node(&self, id: Uuid) -> Option<&Node> {
        self.nodes.get(&id)
    }

    /// Mutably borrow a node by id. The caller is responsible for
    /// calling [`Node::touch`] when mutating.
    pub fn get_node_mut(&mut self, id: Uuid) -> Option<&mut Node> {
        self.nodes.get_mut(&id)
    }

    /// Remove a node and all of its descendants. Returns the removed
    /// node (descendants are removed silently).
    pub fn remove_node(&mut self, id: Uuid) -> Option<Node> {
        let node = self.nodes.remove(&id)?;
        // Detach from parent's children list (if any).
        if let Some(pid) = node.parent_id {
            if let Some(parent) = self.nodes.get_mut(&pid) {
                parent.children.retain(|c| *c != id);
                parent.touch();
            }
        } else {
            self.root_ids.retain(|c| *c != id);
        }
        // Recurse into children (depth-first).
        let kids = node.children.clone();
        for kid in kids {
            // Detach kid from `id` first, so the recursive call doesn't
            // try to mutate the already-removed parent.
            if let Some(k) = self.nodes.get_mut(&kid) {
                k.parent_id = None;
            }
            self.remove_node(kid);
        }
        Some(node)
    }

    /// Move `id` under `new_parent`, inserting at `index` in the
    /// parent's children list. `new_parent = None` moves it to the
    /// root list.
    pub fn reparent_node(
        &mut self,
        id: Uuid,
        new_parent: Option<Uuid>,
        index: usize,
    ) -> Result<()> {
        if !self.nodes.contains_key(&id) {
            return Err(DocumentError::NodeNotFound(id));
        }
        if let Some(pid) = new_parent {
            if !self.nodes.contains_key(&pid) {
                return Err(DocumentError::InvalidReparent { target: pid });
            }
            // Cycle check: ensure `id` is not an ancestor of `pid`.
            let mut cursor = Some(pid);
            while let Some(c) = cursor {
                if c == id {
                    return Err(DocumentError::CycleDetected { child: id });
                }
                cursor = self.nodes.get(&c).and_then(|n| n.parent_id);
            }
        }

        // Detach from current parent (or root list).
        let old_parent = self.nodes.get(&id).expect("checked above").parent_id;
        if let Some(old) = old_parent {
            if let Some(p) = self.nodes.get_mut(&old) {
                p.children.retain(|c| *c != id);
                p.touch();
            }
        } else {
            self.root_ids.retain(|c| *c != id);
        }

        // Re-attach.
        if let Some(pid) = new_parent {
            if let Some(parent) = self.nodes.get_mut(&pid) {
                let i = index.min(parent.children.len());
                parent.children.insert(i, id);
                parent.touch();
            }
        } else {
            let i = index.min(self.root_ids.len());
            self.root_ids.insert(i, id);
        }

        if let Some(n) = self.nodes.get_mut(&id) {
            n.parent_id = new_parent;
            n.touch();
        }
        Ok(())
    }

    /// Reorder a parent's children to `new_order`. The new order must
    /// be a permutation of the current children set; otherwise
    /// `ReorderSetMismatch` is returned.
    pub fn reorder_children(&mut self, parent_id: Uuid, new_order: &[Uuid]) -> Result<()> {
        let parent = self
            .nodes
            .get_mut(&parent_id)
            .ok_or(DocumentError::NodeNotFound(parent_id))?;
        if parent.children.len() != new_order.len() {
            return Err(DocumentError::ReorderSetMismatch { parent: parent_id });
        }
        let mut current: HashMap<Uuid, usize> = HashMap::new();
        for c in &parent.children {
            *current.entry(*c).or_insert(0) += 1;
        }
        for id in new_order {
            match current.get_mut(id) {
                Some(n) if *n > 0 => *n -= 1,
                _ => return Err(DocumentError::ReorderSetMismatch { parent: parent_id }),
            }
        }
        parent.children = new_order.to_vec();
        parent.touch();
        Ok(())
    }

    /// Direct children of `id` (clone of the underlying list).
    pub fn children_of(&self, id: Uuid) -> Vec<Uuid> {
        self.nodes
            .get(&id)
            .map(|n| n.children.clone())
            .unwrap_or_default()
    }

    /// Ancestors of `id`, walking parent pointers bottom-up. Excludes
    /// `id` itself; the last entry is a root.
    pub fn ancestors_of(&self, id: Uuid) -> Vec<Uuid> {
        let mut out = Vec::new();
        let mut cursor = self.nodes.get(&id).and_then(|n| n.parent_id);
        while let Some(p) = cursor {
            out.push(p);
            cursor = self.nodes.get(&p).and_then(|n| n.parent_id);
        }
        out
    }

    /// Descendants of `id` in depth-first order. Excludes `id` itself.
    pub fn descendants_of(&self, id: Uuid) -> Vec<Uuid> {
        let mut out = Vec::new();
        let mut stack: Vec<Uuid> = self.children_of(id).into_iter().rev().collect();
        while let Some(c) = stack.pop() {
            out.push(c);
            for k in self.children_of(c).into_iter().rev() {
                stack.push(k);
            }
        }
        out
    }

    /// All node ids whose bounds intersect `bounds`. O(n); a real
    /// spatial index ([`kcreate_vector::spatial_index`]) is used in
    /// hot paths.
    pub fn nodes_in_bounds(&self, bounds: &Bounds) -> Vec<Uuid> {
        self.nodes
            .iter()
            .filter(|(_, n)| n.bounds.intersection(bounds).is_some())
            .map(|(id, _)| *id)
            .collect()
    }

    /// Number of nodes in the document.
    #[must_use]
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Read-only slice of root ids.
    #[must_use]
    pub fn root_ids(&self) -> &[Uuid] {
        &self.root_ids
    }

    /// Iterator over `(id, &Node)` pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&Uuid, &Node)> {
        self.nodes.iter()
    }

    /// True if the document contains the given node.
    #[must_use]
    pub fn contains(&self, id: Uuid) -> bool {
        self.nodes.contains_key(&id)
    }

    // ------------------------------------------------------------------
    // Artboard convenience APIs
    // ------------------------------------------------------------------

    /// Create a new artboard as a child of `page_id` with the given
    /// `name` and `bounds`. The new artboard is appended to the page's
    /// children list.
    ///
    /// Returns [`DocumentError::NodeNotFound`] if `page_id` doesn't
    /// exist and [`DocumentError::WrongNodeType`] if the referenced
    /// node is not a [`NodeType::Page`]. Artboards may only be direct
    /// children of pages in Phase 1 — that constraint matches the
    /// PROPOSAL.md §4.2 multi-artboard page model and avoids the
    /// nested-artboard "scene-within-scene" complexity Figma had to
    /// untangle.
    pub fn create_artboard(&mut self, page_id: Uuid, name: &str, bounds: Bounds) -> Result<Uuid> {
        let page = self
            .nodes
            .get(&page_id)
            .ok_or(DocumentError::NodeNotFound(page_id))?;
        if page.node_type != NodeType::Page {
            return Err(DocumentError::WrongNodeType {
                id: page_id,
                expected: NodeType::Page,
                got: page.node_type,
            });
        }
        let mut artboard = Node::new(NodeType::Artboard, name);
        artboard.parent_id = Some(page_id);
        artboard.bounds = bounds;
        let id = self.insert_node(artboard)?;
        Ok(id)
    }

    /// All direct [`NodeType::Artboard`] children of `page_id`, sorted
    /// by `bounds.x` ascending (left to right) so the left-panel
    /// artboard list and home-screen previews display in a stable
    /// visual order regardless of insertion order.
    #[must_use]
    pub fn list_artboards(&self, page_id: Uuid) -> Vec<&Node> {
        let Some(page) = self.nodes.get(&page_id) else {
            return Vec::new();
        };
        let mut artboards: Vec<&Node> = page
            .children
            .iter()
            .filter_map(|c| self.nodes.get(c))
            .filter(|n| n.node_type == NodeType::Artboard)
            .collect();
        artboards.sort_by(|a, b| {
            a.bounds
                .x
                .partial_cmp(&b.bounds.x)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        artboards
    }

    /// Deep-clone an artboard and all its descendants, offset by
    /// `(width + 100, 0)` so the copy lands immediately to the right
    /// of the original with a 100-px gap.
    ///
    /// Returns the new artboard's id. Returns
    /// [`DocumentError::WrongNodeType`] if `artboard_id` is not an
    /// artboard. The clone preserves the *subtree shape* and node
    /// properties verbatim; only the ids and parent references are
    /// regenerated so every clone is a fresh, independent identity in
    /// the document graph.
    pub fn duplicate_artboard(&mut self, artboard_id: Uuid) -> Result<Uuid> {
        let source = self
            .nodes
            .get(&artboard_id)
            .ok_or(DocumentError::NodeNotFound(artboard_id))?;
        if source.node_type != NodeType::Artboard {
            return Err(DocumentError::WrongNodeType {
                id: artboard_id,
                expected: NodeType::Artboard,
                got: source.node_type,
            });
        }
        let parent_id = source.parent_id;
        let width = source.bounds.width;
        let original_name = source.name.clone();
        let new_root = self.clone_subtree(artboard_id, parent_id)?;
        // Offset by width + 100 (one-time gap) and rename root.
        if let Some(root) = self.nodes.get_mut(&new_root) {
            root.bounds.x += width + 100.0;
            root.name = format!("{original_name} copy");
            root.touch();
        }
        Ok(new_root)
    }

    /// Deep-clone the subtree rooted at `source_root` (including its
    /// descendants) under `new_parent`. Returns the new root's id.
    ///
    /// All ids are regenerated so the clone is an independent identity
    /// in the graph. `parent_id` pointers and `children` lists in the
    /// cloned nodes are remapped to the new ids; `version` resets to
    /// 0 and `created_at`/`updated_at` are bumped to "now" on every
    /// cloned node.
    ///
    /// Returns [`DocumentError::NodeNotFound`] if `source_root` is
    /// missing, or [`DocumentError::InvalidReparent`] if `new_parent`
    /// doesn't exist.
    pub fn clone_subtree(&mut self, source_root: Uuid, new_parent: Option<Uuid>) -> Result<Uuid> {
        if !self.nodes.contains_key(&source_root) {
            return Err(DocumentError::NodeNotFound(source_root));
        }
        if let Some(pid) = new_parent {
            if !self.nodes.contains_key(&pid) {
                return Err(DocumentError::InvalidReparent { target: pid });
            }
        }

        let subtree_ids: Vec<Uuid> = std::iter::once(source_root)
            .chain(self.descendants_of(source_root))
            .collect();
        let mut snapshots: HashMap<Uuid, Node> = HashMap::with_capacity(subtree_ids.len());
        for id in &subtree_ids {
            if let Some(n) = self.nodes.get(id) {
                snapshots.insert(*id, n.clone());
            }
        }

        let mut id_map: HashMap<Uuid, Uuid> = HashMap::with_capacity(subtree_ids.len());
        for id in &subtree_ids {
            id_map.insert(*id, Uuid::new_v4());
        }
        let new_root = *id_map.get(&source_root).expect("just inserted");

        let now = chrono::Utc::now();
        for old_id in &subtree_ids {
            let original = &snapshots[old_id];
            let mut copy = original.clone();
            copy.id = id_map[old_id];
            copy.parent_id = if *old_id == source_root {
                new_parent
            } else {
                original
                    .parent_id
                    .and_then(|pid| id_map.get(&pid).copied())
                    .or(new_parent)
            };
            copy.children = original
                .children
                .iter()
                .filter_map(|c| id_map.get(c).copied())
                .collect();
            copy.version = 0;
            copy.created_at = now;
            copy.updated_at = now;

            let copy_id = copy.id;
            let copy_parent = copy.parent_id;
            self.nodes.insert(copy.id, copy);
            if let Some(pid) = copy_parent {
                if *old_id == source_root {
                    if let Some(parent) = self.nodes.get_mut(&pid) {
                        if !parent.children.contains(&copy_id) {
                            parent.children.push(copy_id);
                            parent.touch();
                        }
                    }
                }
            } else if !self.root_ids.contains(&copy_id) {
                self.root_ids.push(copy_id);
            }
        }
        Ok(new_root)
    }

    /// Update the artboard's bounds without touching children.
    ///
    /// Returns [`DocumentError::WrongNodeType`] if the node is not an
    /// artboard. Resizes are a pure metadata change on the artboard
    /// node — children keep their original positions/sizes, so an
    /// artboard shrunk past a child's bounds will leave that child
    /// hanging outside the new clip rect (the scene-sync clipping
    /// layer is what determines whether overflowing children render).
    pub fn resize_artboard(&mut self, artboard_id: Uuid, new_bounds: Bounds) -> Result<()> {
        let node = self
            .nodes
            .get_mut(&artboard_id)
            .ok_or(DocumentError::NodeNotFound(artboard_id))?;
        if node.node_type != NodeType::Artboard {
            return Err(DocumentError::WrongNodeType {
                id: artboard_id,
                expected: NodeType::Artboard,
                got: node.node_type,
            });
        }
        node.bounds = new_bounds;
        node.touch();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::{Bounds, Node, NodeType};

    fn child_of(parent: Uuid, name: &str) -> Node {
        let mut n = Node::new(NodeType::VectorLayer, name);
        n.parent_id = Some(parent);
        n
    }

    #[test]
    fn insert_and_lookup() {
        let mut g = DocumentGraph::new();
        let page = Node::new(NodeType::Page, "page");
        let pid = page.id;
        g.insert_node(page).expect("insert root");
        assert!(g.get_node(pid).is_some());
        assert_eq!(g.root_ids(), &[pid]);
        assert_eq!(g.node_count(), 1);
    }

    #[test]
    fn insert_child_updates_parent_children() {
        let mut g = DocumentGraph::new();
        let parent = Node::new(NodeType::Artboard, "ab");
        let pid = parent.id;
        g.insert_node(parent).expect("insert parent");
        let kid = child_of(pid, "vector");
        let kid_id = kid.id;
        g.insert_node(kid).expect("insert child");
        assert_eq!(g.children_of(pid), vec![kid_id]);
    }

    #[test]
    fn insert_child_with_unknown_parent_errors() {
        let mut g = DocumentGraph::new();
        let n = child_of(Uuid::new_v4(), "orphan");
        let err = g.insert_node(n).expect_err("unknown parent");
        assert!(matches!(err, DocumentError::InvalidReparent { .. }));
    }

    #[test]
    fn remove_node_also_removes_descendants() {
        let mut g = DocumentGraph::new();
        let p = Node::new(NodeType::Artboard, "p");
        let pid = p.id;
        g.insert_node(p).expect("p");
        let c1 = child_of(pid, "c1");
        let c1_id = c1.id;
        g.insert_node(c1).expect("c1");
        let mut c2 = Node::new(NodeType::VectorLayer, "c2");
        c2.parent_id = Some(c1_id);
        let c2_id = c2.id;
        g.insert_node(c2).expect("c2");

        let removed = g.remove_node(pid).expect("removed");
        assert_eq!(removed.id, pid);
        assert!(g.get_node(pid).is_none());
        assert!(g.get_node(c1_id).is_none());
        assert!(g.get_node(c2_id).is_none());
        assert!(g.root_ids().is_empty());
    }

    #[test]
    fn reparent_moves_under_new_parent() {
        let mut g = DocumentGraph::new();
        let p1 = Node::new(NodeType::Artboard, "p1");
        let p2 = Node::new(NodeType::Artboard, "p2");
        let p1_id = p1.id;
        let p2_id = p2.id;
        g.insert_node(p1).expect("p1");
        g.insert_node(p2).expect("p2");
        let c = child_of(p1_id, "c");
        let c_id = c.id;
        g.insert_node(c).expect("c");

        g.reparent_node(c_id, Some(p2_id), 0).expect("reparent");
        assert_eq!(g.children_of(p1_id), Vec::<Uuid>::new());
        assert_eq!(g.children_of(p2_id), vec![c_id]);
        assert_eq!(g.get_node(c_id).unwrap().parent_id, Some(p2_id));
    }

    #[test]
    fn reparent_into_descendant_is_cycle_error() {
        let mut g = DocumentGraph::new();
        let a = Node::new(NodeType::Artboard, "a");
        let a_id = a.id;
        g.insert_node(a).expect("a");
        let b = child_of(a_id, "b");
        let b_id = b.id;
        g.insert_node(b).expect("b");
        let err = g
            .reparent_node(a_id, Some(b_id), 0)
            .expect_err("cycle detected");
        assert!(matches!(err, DocumentError::CycleDetected { .. }));
    }

    #[test]
    fn reparent_to_root_removes_from_old_parent() {
        let mut g = DocumentGraph::new();
        let a = Node::new(NodeType::Artboard, "a");
        let a_id = a.id;
        g.insert_node(a).expect("a");
        let b = child_of(a_id, "b");
        let b_id = b.id;
        g.insert_node(b).expect("b");
        g.reparent_node(b_id, None, 0).expect("reparent root");
        assert!(g.children_of(a_id).is_empty());
        assert!(g.root_ids().contains(&b_id));
    }

    #[test]
    fn reorder_children_permutes_list() {
        let mut g = DocumentGraph::new();
        let p = Node::new(NodeType::Artboard, "p");
        let pid = p.id;
        g.insert_node(p).expect("p");
        let mut ids = Vec::new();
        for i in 0..3 {
            let c = child_of(pid, &format!("c{i}"));
            ids.push(c.id);
            g.insert_node(c).expect("c");
        }
        let mut reversed = ids.clone();
        reversed.reverse();
        g.reorder_children(pid, &reversed).expect("reorder");
        assert_eq!(g.children_of(pid), reversed);
    }

    #[test]
    fn reorder_children_set_mismatch_errors() {
        let mut g = DocumentGraph::new();
        let p = Node::new(NodeType::Artboard, "p");
        let pid = p.id;
        g.insert_node(p).expect("p");
        let c = child_of(pid, "c");
        let c_id = c.id;
        g.insert_node(c).expect("c");
        let bogus = vec![Uuid::new_v4()];
        let err = g.reorder_children(pid, &bogus).expect_err("mismatch");
        assert!(matches!(err, DocumentError::ReorderSetMismatch { .. }));
        let _ = c_id; // used for completeness
    }

    #[test]
    fn ancestors_and_descendants_walk_tree() {
        let mut g = DocumentGraph::new();
        let a = Node::new(NodeType::Artboard, "a");
        let a_id = a.id;
        g.insert_node(a).expect("a");
        let b = child_of(a_id, "b");
        let b_id = b.id;
        g.insert_node(b).expect("b");
        let mut c = Node::new(NodeType::VectorLayer, "c");
        c.parent_id = Some(b_id);
        let c_id = c.id;
        g.insert_node(c).expect("c");

        assert_eq!(g.ancestors_of(c_id), vec![b_id, a_id]);
        assert_eq!(g.descendants_of(a_id), vec![b_id, c_id]);
    }

    #[test]
    fn nodes_in_bounds_filters_by_intersection() {
        let mut g = DocumentGraph::new();
        let mut a = Node::new(NodeType::VectorLayer, "a");
        a.bounds = Bounds::new(0.0, 0.0, 10.0, 10.0);
        let a_id = a.id;
        g.insert_node(a).expect("a");
        let mut b = Node::new(NodeType::VectorLayer, "b");
        b.bounds = Bounds::new(20.0, 20.0, 10.0, 10.0);
        let b_id = b.id;
        g.insert_node(b).expect("b");

        let hit = g.nodes_in_bounds(&Bounds::new(5.0, 5.0, 2.0, 2.0));
        assert_eq!(hit, vec![a_id]);
        let miss = g.nodes_in_bounds(&Bounds::new(100.0, 100.0, 1.0, 1.0));
        assert!(miss.is_empty());
        let both = g.nodes_in_bounds(&Bounds::new(0.0, 0.0, 100.0, 100.0));
        assert!(both.contains(&a_id) && both.contains(&b_id));
    }

    #[test]
    fn reparent_unknown_target_errors() {
        let mut g = DocumentGraph::new();
        let a = Node::new(NodeType::Artboard, "a");
        let a_id = a.id;
        g.insert_node(a).expect("a");
        let err = g
            .reparent_node(a_id, Some(Uuid::new_v4()), 0)
            .expect_err("unknown");
        assert!(matches!(err, DocumentError::InvalidReparent { .. }));
    }

    #[test]
    fn document_serialize_roundtrip() {
        let mut g = DocumentGraph::new();
        let a = Node::new(NodeType::Page, "a");
        g.insert_node(a).expect("a");
        let s = serde_json::to_string(&g).expect("serialize");
        let g2: DocumentGraph = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(g.node_count(), g2.node_count());
        assert_eq!(g.root_ids(), g2.root_ids());
    }

    // ---- Artboard convenience API tests ----

    fn page(g: &mut DocumentGraph, name: &str) -> Uuid {
        let mut p = Node::new(NodeType::Page, name);
        p.bounds = Bounds::new(0.0, 0.0, 1920.0, 1080.0);
        let id = p.id;
        g.insert_node(p).expect("page");
        id
    }

    #[test]
    fn create_artboard_attaches_under_page() {
        let mut g = DocumentGraph::new();
        let page_id = page(&mut g, "Home");
        let id = g
            .create_artboard(page_id, "Hero", Bounds::new(0.0, 0.0, 1440.0, 900.0))
            .expect("create");
        let node = g.get_node(id).expect("inserted");
        assert_eq!(node.node_type, NodeType::Artboard);
        assert_eq!(node.parent_id, Some(page_id));
        assert_eq!(node.bounds, Bounds::new(0.0, 0.0, 1440.0, 900.0));
        assert_eq!(g.children_of(page_id), vec![id]);
    }

    #[test]
    fn create_artboard_rejects_non_page_parent() {
        let mut g = DocumentGraph::new();
        let page_id = page(&mut g, "p");
        let ab = g
            .create_artboard(page_id, "Hero", Bounds::new(0.0, 0.0, 100.0, 100.0))
            .expect("create");
        let err = g
            .create_artboard(ab, "Nested", Bounds::new(0.0, 0.0, 100.0, 100.0))
            .expect_err("non-page parent rejected");
        assert!(matches!(err, DocumentError::WrongNodeType { .. }));
    }

    #[test]
    fn list_artboards_sorted_by_x() {
        let mut g = DocumentGraph::new();
        let p = page(&mut g, "Home");
        let a = g
            .create_artboard(p, "A", Bounds::new(2000.0, 0.0, 200.0, 200.0))
            .expect("a");
        let b = g
            .create_artboard(p, "B", Bounds::new(100.0, 0.0, 200.0, 200.0))
            .expect("b");
        let c = g
            .create_artboard(p, "C", Bounds::new(1000.0, 0.0, 200.0, 200.0))
            .expect("c");
        let ids: Vec<Uuid> = g.list_artboards(p).iter().map(|n| n.id).collect();
        assert_eq!(ids, vec![b, c, a]);
    }

    #[test]
    fn duplicate_artboard_preserves_subtree_with_new_ids() {
        let mut g = DocumentGraph::new();
        let p = page(&mut g, "Home");
        let ab = g
            .create_artboard(p, "Hero", Bounds::new(0.0, 0.0, 400.0, 300.0))
            .expect("ab");
        let mut child = Node::new(NodeType::VectorLayer, "rect");
        child.parent_id = Some(ab);
        child.bounds = Bounds::new(10.0, 10.0, 50.0, 50.0);
        let child_id = g.insert_node(child).expect("child");
        let mut grand = Node::new(NodeType::VectorLayer, "circle");
        grand.parent_id = Some(child_id);
        grand.bounds = Bounds::new(20.0, 20.0, 30.0, 30.0);
        let grand_id = g.insert_node(grand).expect("grand");

        let copy = g.duplicate_artboard(ab).expect("dup");
        assert_ne!(copy, ab);
        let copy_node = g.get_node(copy).expect("copy node");
        // Copy is offset by width + 100 = 500 to the right.
        assert!((copy_node.bounds.x - 500.0).abs() < f64::EPSILON);
        assert_eq!(copy_node.parent_id, Some(p));
        assert_eq!(copy_node.children.len(), 1);
        let copy_child_id = copy_node.children[0];
        assert_ne!(copy_child_id, child_id);
        let copy_child = g.get_node(copy_child_id).expect("copy child");
        assert_eq!(copy_child.node_type, NodeType::VectorLayer);
        assert_eq!(copy_child.parent_id, Some(copy));
        assert_eq!(copy_child.children.len(), 1);
        let copy_grand_id = copy_child.children[0];
        assert_ne!(copy_grand_id, grand_id);
        let copy_grand = g.get_node(copy_grand_id).expect("copy grand");
        assert_eq!(copy_grand.bounds, Bounds::new(20.0, 20.0, 30.0, 30.0));
        // Page now has two children (original + copy).
        assert_eq!(g.children_of(p).len(), 2);
    }

    #[test]
    fn duplicate_artboard_rejects_non_artboard() {
        let mut g = DocumentGraph::new();
        let p = page(&mut g, "p");
        let err = g
            .duplicate_artboard(p)
            .expect_err("page is not an artboard");
        assert!(matches!(err, DocumentError::WrongNodeType { .. }));
    }

    #[test]
    fn resize_artboard_leaves_children_alone() {
        let mut g = DocumentGraph::new();
        let p = page(&mut g, "Home");
        let ab = g
            .create_artboard(p, "Hero", Bounds::new(0.0, 0.0, 400.0, 300.0))
            .expect("ab");
        let mut child = Node::new(NodeType::VectorLayer, "rect");
        child.parent_id = Some(ab);
        child.bounds = Bounds::new(10.0, 10.0, 50.0, 50.0);
        let child_id = g.insert_node(child).expect("child");
        let original_child_bounds = g.get_node(child_id).expect("child").bounds;
        g.resize_artboard(ab, Bounds::new(0.0, 0.0, 800.0, 600.0))
            .expect("resize");
        assert_eq!(
            g.get_node(ab).expect("ab").bounds,
            Bounds::new(0.0, 0.0, 800.0, 600.0)
        );
        assert_eq!(
            g.get_node(child_id).expect("child").bounds,
            original_child_bounds,
            "resize must not touch child bounds",
        );
    }

    #[test]
    fn resize_artboard_rejects_non_artboard() {
        let mut g = DocumentGraph::new();
        let p = page(&mut g, "p");
        let err = g
            .resize_artboard(p, Bounds::new(0.0, 0.0, 1.0, 1.0))
            .expect_err("page rejected");
        assert!(matches!(err, DocumentError::WrongNodeType { .. }));
    }
}
