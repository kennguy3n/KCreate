//! Uniform-grid spatial index for visible-object culling.
//!
//! Phase 0 uses a simple cell-based grid because it's deterministic, has
//! zero unsafe code, and is fast enough for scenes up to ~50k objects.
//! The interface is small so it can be swapped for an R-tree / BVH later
//! without affecting the renderer.

use std::collections::{HashMap, HashSet};

use crate::geometry::Rect;
use crate::scene::{Object, ObjectId};

#[derive(Debug, Clone)]
pub struct SpatialIndex {
    /// World-space bounds covered by the grid.
    bounds: Rect,
    /// Cell side length in world units.
    cell_size: f32,
    /// (`cell_x`, `cell_y`) -> set of object ids overlapping that cell.
    cells: HashMap<(i32, i32), Vec<ObjectId>>,
    /// World-space bounds per inserted object (for refine).
    object_bounds: HashMap<ObjectId, Rect>,
}

impl SpatialIndex {
    /// Build a new index covering `bounds` with the given cell size.
    pub fn new(bounds: Rect, cell_size: f32) -> Self {
        assert!(cell_size > 0.0, "cell size must be positive");
        Self {
            bounds,
            cell_size,
            cells: HashMap::new(),
            object_bounds: HashMap::new(),
        }
    }

    /// Build an index from a slice of objects with a heuristic cell size
    /// (~sqrt(area / N) bounded to [4, 256] units).
    pub fn build_from(objects: &[Object]) -> Self {
        if objects.is_empty() {
            return Self::new(Rect::new(0.0, 0.0, 1.0, 1.0), 32.0);
        }
        let bounds = objects
            .iter()
            .map(Object::world_bounds)
            .reduce(|a, b| a.union(&b))
            .unwrap_or_else(|| Rect::new(0.0, 0.0, 1.0, 1.0));
        let area = (bounds.width * bounds.height).max(1.0);
        let approx = (area / objects.len() as f32).sqrt();
        let cell_size = approx.clamp(4.0, 256.0);
        let mut idx = Self::new(bounds, cell_size);
        for obj in objects {
            if obj.visible {
                idx.insert(obj.id, obj.world_bounds());
            }
        }
        idx
    }

    /// Insert an object with the given world bounds.
    pub fn insert(&mut self, id: ObjectId, bounds: Rect) {
        self.object_bounds.insert(id, bounds);
        for cell in self.cells_for(&bounds) {
            self.cells.entry(cell).or_default().push(id);
        }
    }

    /// Number of objects in the index.
    pub fn len(&self) -> usize {
        self.object_bounds.len()
    }

    /// Returns true if no objects have been inserted.
    pub fn is_empty(&self) -> bool {
        self.object_bounds.is_empty()
    }

    /// Bounds covered by the index.
    pub const fn bounds(&self) -> Rect {
        self.bounds
    }

    /// Cell size in world units.
    pub const fn cell_size(&self) -> f32 {
        self.cell_size
    }

    /// Find every object whose AABB intersects `query`. The returned vec is
    /// deduplicated and ordered by `ObjectId` for deterministic iteration.
    pub fn query(&self, query: Rect) -> Vec<ObjectId> {
        let mut seen: HashSet<ObjectId> = HashSet::new();
        let mut hits: Vec<ObjectId> = Vec::new();
        for cell in self.cells_for(&query) {
            if let Some(ids) = self.cells.get(&cell) {
                for &id in ids {
                    if !seen.insert(id) {
                        continue;
                    }
                    if let Some(b) = self.object_bounds.get(&id) {
                        if b.intersects(&query) {
                            hits.push(id);
                        }
                    }
                }
            }
        }
        hits.sort_unstable();
        hits
    }

    fn cells_for(&self, r: &Rect) -> CellIter {
        let cs = self.cell_size;
        let x0 = (r.x / cs).floor() as i32;
        let y0 = (r.y / cs).floor() as i32;
        let x1 = ((r.max_x() / cs).ceil() as i32).max(x0 + 1);
        let y1 = ((r.max_y() / cs).ceil() as i32).max(y0 + 1);
        CellIter {
            x0,
            y0,
            x1,
            y1,
            cx: x0,
            cy: y0,
        }
    }
}

struct CellIter {
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    cx: i32,
    cy: i32,
}

impl Iterator for CellIter {
    type Item = (i32, i32);

    fn next(&mut self) -> Option<Self::Item> {
        if self.cy >= self.y1 {
            return None;
        }
        let out = (self.cx, self.cy);
        self.cx += 1;
        if self.cx >= self.x1 {
            self.cx = self.x0;
            self.cy += 1;
        }
        Some(out)
    }
}

impl std::fmt::Debug for CellIter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CellIter")
            .field("range", &((self.x0, self.y0), (self.x1, self.y1)))
            .field("cursor", &(self.cx, self.cy))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{Color, Style};
    use crate::scene::{Object, ObjectKind};

    #[test]
    fn query_returns_only_intersecting_objects() {
        let mut idx = SpatialIndex::new(Rect::new(0.0, 0.0, 1000.0, 1000.0), 16.0);
        idx.insert(ObjectId(1), Rect::new(0.0, 0.0, 10.0, 10.0));
        idx.insert(ObjectId(2), Rect::new(100.0, 100.0, 10.0, 10.0));
        // Spans into both the (1) and (2) query regions.
        idx.insert(ObjectId(3), Rect::new(5.0, 5.0, 120.0, 120.0));

        let hits = idx.query(Rect::new(0.0, 0.0, 20.0, 20.0));
        assert_eq!(hits, vec![ObjectId(1), ObjectId(3)]);

        let hits2 = idx.query(Rect::new(95.0, 95.0, 30.0, 30.0));
        assert_eq!(hits2, vec![ObjectId(2), ObjectId(3)]);

        // Object 3 spans many cells but appears only once in the result.
        let hits3 = idx.query(Rect::new(0.0, 0.0, 200.0, 200.0));
        assert_eq!(hits3, vec![ObjectId(1), ObjectId(2), ObjectId(3)]);
    }

    #[test]
    fn build_from_uses_heuristic_cell_size() {
        let objects: Vec<Object> = (0..100)
            .map(|i| {
                let x = (i % 10) as f32 * 10.0;
                let y = (i / 10) as f32 * 10.0;
                Object::new(
                    ObjectKind::Rect(Rect::new(x, y, 5.0, 5.0)),
                    Style::filled(Color::rgba(1.0, 0.0, 0.0, 1.0)),
                )
            })
            .collect();
        let idx = SpatialIndex::build_from(&objects);
        assert_eq!(idx.len(), 100);
        assert!(idx.cell_size() >= 4.0 && idx.cell_size() <= 256.0);
    }

    #[test]
    fn skips_invisible_objects() {
        let mut obj = Object::new(
            ObjectKind::Rect(Rect::new(0.0, 0.0, 10.0, 10.0)),
            Style::filled(Color::rgba(1.0, 0.0, 0.0, 1.0)),
        );
        obj.visible = false;
        let idx = SpatialIndex::build_from(&[obj]);
        assert!(idx.is_empty());
    }

    #[test]
    fn dedup_across_multiple_cells() {
        // An object spanning 3 cells should still appear once in the result.
        let mut idx = SpatialIndex::new(Rect::new(0.0, 0.0, 1000.0, 1000.0), 16.0);
        idx.insert(ObjectId(7), Rect::new(0.0, 0.0, 64.0, 16.0));
        let hits = idx.query(Rect::new(0.0, 0.0, 64.0, 16.0));
        assert_eq!(hits, vec![ObjectId(7)]);
    }
}
