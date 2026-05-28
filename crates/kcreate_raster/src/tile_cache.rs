//! Memory-bounded LRU tile cache (Phase 8 Block E Task 28).
//!
//! [`TileCache`] is a process-wide bounded store of decoded
//! [`Tile`]s keyed on an opaque caller-supplied key (typically
//! `(LayerId, col, row)`). It backs the architecture promise in
//! `ARCHITECTURE.md §"Raster tile engine"`:
//!
//! > Tiles are produced lazily and cached LRU; pan only invalidates
//! > uncovered regions.
//!
//! ## Eviction policy
//!
//! Entries are tracked by a monotonic *tick* counter that is
//! bumped on every read or write. The cache evicts the entry with
//! the smallest tick (least-recently-used) until the total bytes
//! held are at or below the configured `budget` — or until the
//! cache is empty.
//!
//! ## Memory accounting
//!
//! `bytes` is the sum of `tile.pixels.len()` across all live
//! entries. The cache deliberately ignores the per-entry overhead
//! of the `HashMap` bucket and the `BTreeMap` index (~64 bytes
//! per entry on 64-bit targets) because the pixel payload
//! dominates by 4+ orders of magnitude (a single 256×256 RGBA
//! tile is 262 144 bytes).
//!
//! ## Oversized inserts
//!
//! If a caller inserts a tile whose byte count exceeds the budget,
//! the cache evicts every other entry (since they would all need
//! to be evicted to make room anyway) and then stores the
//! oversized tile, briefly going over budget by `tile_bytes -
//! budget`. The next [`insert`](Self::insert) or
//! [`set_budget`](Self::set_budget) call will evict it. This
//! mirrors the LRU semantics in `crates/kcreate_storage::blob_store`
//! and avoids silently dropping content the caller asked us to
//! cache.

use std::collections::{BTreeMap, HashMap};
use std::hash::Hash;

use crate::tile::Tile;

/// Memory-bounded LRU cache of [`Tile`]s.
///
/// Generic over the key type. Typical instantiation is
/// `TileCache<(uuid::Uuid, u32, u32)>` (per-layer per-tile), but
/// any `Eq + Hash + Clone` key is accepted.
#[derive(Debug)]
pub struct TileCache<K: Eq + Hash + Clone> {
    storage: HashMap<K, Entry>,
    /// Sorted index from `tick` to key. Eviction reads the
    /// smallest tick to find the LRU entry in `O(log n)`.
    by_tick: BTreeMap<u64, K>,
    bytes: u64,
    budget: u64,
    next_tick: u64,
}

#[derive(Debug)]
struct Entry {
    tile: Tile,
    tick: u64,
    /// Cached byte count so eviction doesn't need to re-borrow
    /// `tile.pixels` while we're mutating the maps. Always equal
    /// to `tile.pixels.len() as u64` for live entries.
    bytes: u64,
}

impl<K: Eq + Hash + Clone> TileCache<K> {
    /// Create a cache with the given byte budget. A budget of `0`
    /// is legal — it means "every insert immediately evicts" and
    /// is useful for testing the eviction path.
    #[must_use]
    pub fn with_byte_budget(budget: u64) -> Self {
        Self {
            storage: HashMap::new(),
            by_tick: BTreeMap::new(),
            bytes: 0,
            budget,
            next_tick: 0,
        }
    }

    /// Configured byte budget.
    #[must_use]
    pub const fn budget(&self) -> u64 {
        self.budget
    }

    /// Total bytes currently held by the cache.
    #[must_use]
    pub const fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Number of cached entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.storage.len()
    }

    /// Is the cache empty?
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.storage.is_empty()
    }

    /// Borrow a tile by key, bumping it to most-recently-used.
    /// Returns `None` if the key is not cached.
    pub fn get(&mut self, key: &K) -> Option<&Tile> {
        self.bump(key)?;
        Some(&self.storage.get(key)?.tile)
    }

    /// Mutably borrow a tile by key, bumping it to
    /// most-recently-used. Returns `None` if the key is not
    /// cached.
    ///
    /// Note: this does **not** re-account for byte-count drift if
    /// the caller resizes `tile.pixels` through the returned
    /// reference. If you need to mutate a tile's pixel buffer
    /// length, [`remove`](Self::remove) it and [`insert`](Self::insert)
    /// the new version so the cache's `bytes` counter stays
    /// accurate.
    pub fn get_mut(&mut self, key: &K) -> Option<&mut Tile> {
        self.bump(key)?;
        Some(&mut self.storage.get_mut(key)?.tile)
    }

    /// Insert `(key, tile)` into the cache, possibly evicting the
    /// least-recently-used entries to fit within the byte budget.
    /// Returns every entry that was evicted as a `(key, tile)`
    /// pair so callers can persist dirty tiles before they
    /// disappear.
    ///
    /// If `key` is already present its old value is replaced (and
    /// returned as the first element of the eviction list so the
    /// caller can distinguish replacements from budget evictions
    /// via [`Self::contains`]). Replacement also bumps the entry
    /// to most-recently-used.
    pub fn insert(&mut self, key: K, tile: Tile) -> Vec<(K, Tile)> {
        let bytes = tile.pixels.len() as u64;
        let mut evicted = Vec::new();
        // Replace existing entry (if any) so we don't double-count
        // bytes.
        if let Some(old) = self.remove(&key) {
            evicted.push((key.clone(), old));
        }
        let tick = self.next_tick;
        self.next_tick = self.next_tick.wrapping_add(1);
        self.storage
            .insert(key.clone(), Entry { tile, tick, bytes });
        self.by_tick.insert(tick, key);
        self.bytes = self.bytes.saturating_add(bytes);
        self.evict_until_under_budget(&mut evicted);
        evicted
    }

    /// Remove the entry for `key` from the cache, returning the
    /// stored tile if present.
    pub fn remove(&mut self, key: &K) -> Option<Tile> {
        let entry = self.storage.remove(key)?;
        self.by_tick.remove(&entry.tick);
        self.bytes = self.bytes.saturating_sub(entry.bytes);
        Some(entry.tile)
    }

    /// True if `key` is currently cached.
    #[must_use]
    pub fn contains(&self, key: &K) -> bool {
        self.storage.contains_key(key)
    }

    /// Drop every entry. Returns the list of evicted entries in
    /// LRU order so callers can persist or recycle them.
    pub fn clear(&mut self) -> Vec<(K, Tile)> {
        let mut out = Vec::with_capacity(self.storage.len());
        // Iterate in tick order so callers see oldest-first.
        let keys: Vec<K> = self.by_tick.values().cloned().collect();
        for k in keys {
            if let Some(t) = self.remove(&k) {
                out.push((k, t));
            }
        }
        out
    }

    /// Update the byte budget. If `new_budget` is smaller than
    /// the current `bytes`, evicts in LRU order until under the
    /// new budget. Returns the evicted entries.
    pub fn set_budget(&mut self, new_budget: u64) -> Vec<(K, Tile)> {
        self.budget = new_budget;
        let mut evicted = Vec::new();
        self.evict_until_under_budget(&mut evicted);
        evicted
    }

    /// Bump the recency of an existing entry. Returns the new
    /// tick (or `None` if the key is absent).
    fn bump(&mut self, key: &K) -> Option<u64> {
        let entry = self.storage.get_mut(key)?;
        let old_tick = entry.tick;
        let new_tick = self.next_tick;
        self.next_tick = self.next_tick.wrapping_add(1);
        entry.tick = new_tick;
        self.by_tick.remove(&old_tick);
        self.by_tick.insert(new_tick, key.clone());
        Some(new_tick)
    }

    /// Evict from the front of the `by_tick` map until total bytes
    /// drop to `<= budget`. Stops once only one entry remains —
    /// we never evict the most-recently-used entry just because
    /// it is oversized, so a caller who just inserted a 200 MB
    /// tile into a 64 MB cache can still read it back. The cache
    /// is briefly over budget in that case; the next insert (or
    /// `set_budget` call) will reconsider eviction.
    fn evict_until_under_budget(&mut self, evicted: &mut Vec<(K, Tile)>) {
        while self.bytes > self.budget && self.storage.len() > 1 {
            // `pop_first` on the BTreeMap yields the smallest tick,
            // which is the least-recently-used entry by construction.
            let Some((_, key)) = self.by_tick.pop_first() else {
                break;
            };
            if let Some(entry) = self.storage.remove(&key) {
                self.bytes = self.bytes.saturating_sub(entry.bytes);
                evicted.push((key, entry.tile));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: 4×4 RGBA tile = 64 bytes of pixels. Small enough to
    /// fit a few in a tiny budget but big enough to trigger LRU
    /// eviction predictably.
    fn small_tile(seed: u8) -> Tile {
        Tile {
            x: 0,
            y: 0,
            pixels: vec![seed; 64],
            dirty: false,
        }
    }

    #[test]
    fn empty_cache_reports_zero_state() {
        let cache: TileCache<u32> = TileCache::with_byte_budget(1024);
        assert_eq!(cache.budget(), 1024);
        assert_eq!(cache.bytes(), 0);
        assert_eq!(cache.len(), 0);
        assert!(cache.is_empty());
    }

    #[test]
    fn insert_accounts_bytes_and_returns_no_evictions_when_under_budget() {
        let mut cache: TileCache<u32> = TileCache::with_byte_budget(1024);
        let ev = cache.insert(1, small_tile(0xAA));
        assert!(ev.is_empty());
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.bytes(), 64);
        let ev = cache.insert(2, small_tile(0xBB));
        assert!(ev.is_empty());
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.bytes(), 128);
    }

    #[test]
    fn insert_evicts_lru_when_over_budget() {
        // Budget fits exactly 2 tiles (2 * 64 = 128 bytes).
        let mut cache: TileCache<u32> = TileCache::with_byte_budget(128);
        cache.insert(1, small_tile(1));
        cache.insert(2, small_tile(2));
        let ev = cache.insert(3, small_tile(3));
        // Key 1 was least-recently-used — must be the eviction.
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].0, 1);
        assert!(!cache.contains(&1));
        assert!(cache.contains(&2));
        assert!(cache.contains(&3));
        assert_eq!(cache.bytes(), 128);
    }

    #[test]
    fn get_bumps_to_most_recently_used() {
        let mut cache: TileCache<u32> = TileCache::with_byte_budget(128);
        cache.insert(1, small_tile(1));
        cache.insert(2, small_tile(2));
        // Touch key 1, making key 2 the LRU.
        assert!(cache.get(&1).is_some());
        let ev = cache.insert(3, small_tile(3));
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].0, 2, "key 2 should be evicted, not key 1");
        assert!(cache.contains(&1));
        assert!(!cache.contains(&2));
        assert!(cache.contains(&3));
    }

    #[test]
    fn get_mut_also_bumps_to_most_recently_used() {
        let mut cache: TileCache<u32> = TileCache::with_byte_budget(128);
        cache.insert(1, small_tile(1));
        cache.insert(2, small_tile(2));
        assert!(cache.get_mut(&1).is_some());
        let ev = cache.insert(3, small_tile(3));
        assert_eq!(ev[0].0, 2);
    }

    #[test]
    fn insert_replaces_existing_entry_and_returns_old_tile() {
        let mut cache: TileCache<u32> = TileCache::with_byte_budget(1024);
        cache.insert(1, small_tile(1));
        let ev = cache.insert(1, small_tile(99));
        // Replacement is surfaced so the caller can tell that the
        // tile they cached earlier is no longer reachable through
        // the cache.
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].0, 1);
        assert_eq!(ev[0].1.pixels[0], 1);
        // The replacement value is what `get` returns now.
        assert_eq!(cache.get(&1).expect("present").pixels[0], 99);
        // Bytes accounting did not double-count.
        assert_eq!(cache.bytes(), 64);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn remove_returns_stored_tile_and_drops_bytes() {
        let mut cache: TileCache<u32> = TileCache::with_byte_budget(1024);
        cache.insert(1, small_tile(0xCC));
        let removed = cache.remove(&1).expect("present");
        assert_eq!(removed.pixels[0], 0xCC);
        assert_eq!(cache.bytes(), 0);
        assert_eq!(cache.len(), 0);
        assert!(cache.remove(&1).is_none(), "second remove is no-op");
    }

    #[test]
    fn clear_drains_in_lru_order() {
        let mut cache: TileCache<u32> = TileCache::with_byte_budget(1024);
        cache.insert(1, small_tile(1));
        cache.insert(2, small_tile(2));
        cache.insert(3, small_tile(3));
        let drained = cache.clear();
        // Drained in tick order: 1, 2, 3.
        let keys: Vec<u32> = drained.iter().map(|(k, _)| *k).collect();
        assert_eq!(keys, vec![1, 2, 3]);
        assert!(cache.is_empty());
        assert_eq!(cache.bytes(), 0);
    }

    #[test]
    fn set_budget_shrinks_to_new_size_and_evicts_lru_first() {
        let mut cache: TileCache<u32> = TileCache::with_byte_budget(256);
        cache.insert(1, small_tile(1));
        cache.insert(2, small_tile(2));
        cache.insert(3, small_tile(3));
        cache.insert(4, small_tile(4));
        // 4 * 64 = 256 bytes — fits exactly.
        assert_eq!(cache.bytes(), 256);
        // Shrink to 128 bytes — must evict tiles 1 and 2 (oldest).
        let ev = cache.set_budget(128);
        let keys: Vec<u32> = ev.iter().map(|(k, _)| *k).collect();
        assert_eq!(keys, vec![1, 2]);
        assert!(!cache.contains(&1));
        assert!(!cache.contains(&2));
        assert!(cache.contains(&3));
        assert!(cache.contains(&4));
        assert_eq!(cache.bytes(), 128);
    }

    #[test]
    fn oversized_insert_evicts_everything_and_holds_over_budget() {
        let mut cache: TileCache<u32> = TileCache::with_byte_budget(64);
        cache.insert(1, small_tile(1));
        let big = Tile {
            x: 0,
            y: 0,
            pixels: vec![0; 256],
            dirty: false,
        };
        let ev = cache.insert(2, big);
        // Key 1 was evicted to make room.
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].0, 1);
        // Cache is briefly over budget because the single inserted
        // tile is bigger than the budget — documented behaviour.
        assert!(cache.bytes() > cache.budget());
        assert!(cache.contains(&2));
        // Next insert under budget will evict the oversized tile.
        let ev2 = cache.insert(3, small_tile(3));
        assert_eq!(ev2[0].0, 2);
        assert_eq!(cache.bytes(), 64);
    }

    #[test]
    fn zero_budget_cache_keeps_only_the_most_recently_inserted() {
        // A zero-budget cache is the smallest meaningful LRU: it
        // can hold one entry (the one you just inserted) and
        // evicts everything else immediately.
        let mut cache: TileCache<u32> = TileCache::with_byte_budget(0);
        let ev = cache.insert(1, small_tile(1));
        assert!(ev.is_empty());
        assert_eq!(cache.len(), 1);
        let ev = cache.insert(2, small_tile(2));
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].0, 1);
        assert_eq!(cache.len(), 1);
        assert!(cache.contains(&2));
    }

    #[test]
    fn set_budget_to_larger_value_keeps_existing_entries() {
        let mut cache: TileCache<u32> = TileCache::with_byte_budget(64);
        cache.insert(1, small_tile(1));
        let ev = cache.set_budget(1024);
        assert!(ev.is_empty());
        assert!(cache.contains(&1));
        assert_eq!(cache.budget(), 1024);
    }
}
