//! R-tree spatial index for vector paths.
//!
//! Used to back hit-testing and viewport culling: when a user clicks
//! at `(x, y)` or pans the viewport, we ask the index "which paths
//! could possibly overlap this area?" — then run exact tests on the
//! shortlist.
//!
//! Backed by [`rstar`]; we wrap it so callers depend only on the
//! crate-local types ([`BoundingBox`], `Uuid`).

use rstar::{Envelope, Point as RtreePoint, PointDistance, RTree, RTreeObject, AABB};
use uuid::Uuid;

use crate::path::{BoundingBox, PathPoint};

/// A single entry in the spatial index — an id with its envelope.
#[derive(Debug, Clone, Copy, PartialEq)]
struct SpatialEntry {
    id: Uuid,
    aabb: AABB<[f64; 2]>,
}

impl RTreeObject for SpatialEntry {
    type Envelope = AABB<[f64; 2]>;

    fn envelope(&self) -> Self::Envelope {
        self.aabb
    }
}

impl PointDistance for SpatialEntry {
    fn distance_2(
        &self,
        point: &<Self::Envelope as Envelope>::Point,
    ) -> <<Self::Envelope as Envelope>::Point as RtreePoint>::Scalar {
        self.aabb.distance_2(point)
    }
}

/// R-tree-backed index keyed by [`Uuid`].
#[derive(Debug, Default)]
pub struct VectorSpatialIndex {
    tree: RTree<SpatialEntry>,
}

impl VectorSpatialIndex {
    /// Build an empty index.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a single entry. Duplicate ids are allowed; remove the
    /// previous entry first if you want unique-id semantics.
    pub fn insert(&mut self, id: Uuid, bounds: BoundingBox) {
        self.tree.insert(SpatialEntry {
            id,
            aabb: to_aabb(bounds),
        });
    }

    /// Remove a single entry by id. No-op if not present. Linear scan;
    /// for bulk removals use [`Self::rebuild`].
    pub fn remove(&mut self, id: Uuid) {
        // rstar doesn't give us a key-based remove, so we have to
        // collect candidates first.
        let to_remove: Vec<SpatialEntry> =
            self.tree.iter().filter(|e| e.id == id).copied().collect();
        for entry in to_remove {
            self.tree.remove(&entry);
        }
    }

    /// Replace every entry in the tree from a list of `(id, bounds)`
    /// pairs. Uses bulk-load construction for an O(n log n) build vs.
    /// O(n log^2 n) repeated insertion.
    pub fn rebuild(&mut self, entries: &[(Uuid, BoundingBox)]) {
        let items: Vec<SpatialEntry> = entries
            .iter()
            .map(|(id, b)| SpatialEntry {
                id: *id,
                aabb: to_aabb(*b),
            })
            .collect();
        self.tree = RTree::bulk_load(items);
    }

    /// Number of entries currently in the index.
    #[must_use]
    pub fn len(&self) -> usize {
        self.tree.size()
    }

    /// `true` when the index has no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tree.size() == 0
    }

    /// All ids whose envelope overlaps `rect`.
    #[must_use]
    pub fn query_rect(&self, rect: BoundingBox) -> Vec<Uuid> {
        let aabb = to_aabb(rect);
        self.tree
            .locate_in_envelope_intersecting(&aabb)
            .map(|e| e.id)
            .collect()
    }

    /// All ids whose envelope contains `point`.
    #[must_use]
    pub fn query_point(&self, point: PathPoint) -> Vec<Uuid> {
        self.tree
            .locate_all_at_point(&[point.x, point.y])
            .map(|e| e.id)
            .collect()
    }

    /// The `n` ids whose envelope is closest to `point`, in ascending
    /// distance order. Returns fewer than `n` if the tree is smaller.
    #[must_use]
    pub fn nearest(&self, point: PathPoint, n: usize) -> Vec<Uuid> {
        self.tree
            .nearest_neighbor_iter(&[point.x, point.y])
            .take(n)
            .map(|e| e.id)
            .collect()
    }
}

fn to_aabb(b: BoundingBox) -> AABB<[f64; 2]> {
    AABB::from_corners([b.min_x, b.min_y], [b.max_x, b.max_y])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_index_returns_nothing() {
        let idx = VectorSpatialIndex::new();
        assert!(idx.is_empty());
        assert!(idx
            .query_rect(BoundingBox::new(0.0, 0.0, 1.0, 1.0))
            .is_empty());
        assert!(idx.query_point(PathPoint::new(0.5, 0.5)).is_empty());
        assert!(idx.nearest(PathPoint::new(0.0, 0.0), 5).is_empty());
    }

    #[test]
    fn insert_and_query_rect() {
        let mut idx = VectorSpatialIndex::new();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        idx.insert(a, BoundingBox::new(0.0, 0.0, 10.0, 10.0));
        idx.insert(b, BoundingBox::new(20.0, 20.0, 30.0, 30.0));
        let hits = idx.query_rect(BoundingBox::new(5.0, 5.0, 25.0, 25.0));
        assert_eq!(hits.len(), 2);
        assert!(hits.contains(&a));
        assert!(hits.contains(&b));
        let one = idx.query_rect(BoundingBox::new(50.0, 50.0, 60.0, 60.0));
        assert!(one.is_empty());
    }

    #[test]
    fn query_point_returns_containing_envelopes() {
        let mut idx = VectorSpatialIndex::new();
        let a = Uuid::new_v4();
        idx.insert(a, BoundingBox::new(0.0, 0.0, 10.0, 10.0));
        assert_eq!(idx.query_point(PathPoint::new(5.0, 5.0)), vec![a]);
        assert!(idx.query_point(PathPoint::new(100.0, 100.0)).is_empty());
    }

    #[test]
    fn remove_drops_entry() {
        let mut idx = VectorSpatialIndex::new();
        let a = Uuid::new_v4();
        idx.insert(a, BoundingBox::new(0.0, 0.0, 1.0, 1.0));
        assert_eq!(idx.len(), 1);
        idx.remove(a);
        assert_eq!(idx.len(), 0);
        idx.remove(a); // no-op
    }

    #[test]
    fn rebuild_replaces_all_entries() {
        let mut idx = VectorSpatialIndex::new();
        let stale = Uuid::new_v4();
        idx.insert(stale, BoundingBox::new(0.0, 0.0, 1.0, 1.0));
        let fresh1 = Uuid::new_v4();
        let fresh2 = Uuid::new_v4();
        idx.rebuild(&[
            (fresh1, BoundingBox::new(0.0, 0.0, 1.0, 1.0)),
            (fresh2, BoundingBox::new(2.0, 2.0, 3.0, 3.0)),
        ]);
        let hits = idx.query_rect(BoundingBox::new(-10.0, -10.0, 10.0, 10.0));
        assert_eq!(hits.len(), 2);
        assert!(!hits.contains(&stale));
    }

    #[test]
    fn nearest_returns_in_distance_order() {
        let mut idx = VectorSpatialIndex::new();
        let near = Uuid::new_v4();
        let far = Uuid::new_v4();
        idx.insert(near, BoundingBox::new(0.0, 0.0, 1.0, 1.0));
        idx.insert(far, BoundingBox::new(100.0, 100.0, 101.0, 101.0));
        let order = idx.nearest(PathPoint::new(0.5, 0.5), 2);
        assert_eq!(order, vec![near, far]);
    }
}
