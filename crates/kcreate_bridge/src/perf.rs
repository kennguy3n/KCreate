//! Bridge-side wiring for [`kcreate_perf`] (Phase 8 Block E Task 27).
//!
//! Owns three things:
//!
//! 1. **Process-wide startup timeline init.** The bridge is the
//!    first place we can reliably mark cold-path events: by the
//!    time anyone calls `state::ensure_initialized`,
//!    `document::project_create`, or `document::project_open` we
//!    are inside the bridge cdylib. [`ensure_startup_initialized`]
//!    is idempotent and safe to call from any hot path, so
//!    sprinkled marks never depend on order. The single mark this
//!    routine emits is named **`bridge.first_call`** — deliberately
//!    *not* `bridge.dlopen`, because the underlying `OnceLock` does
//!    not fire at actual `dlopen` time but on the first perf API
//!    call (a true load-time mark would need an `unsafe` ctor,
//!    which conflicts with the workspace's
//!    `forbid(unsafe_op_in_unsafe_fn)` lint).
//! 2. **A tile-cache budget honoured by [`kcreate_raster::TileCache`].**
//!    [`tile_cache_lock`] returns a handle to a process-wide
//!    `TileCache` whose byte budget is seeded from
//!    [`kcreate_core::config::RuntimeConfig::effective_raster_cache_mb`]
//!    — i.e. the configured `max_raster_cache_mb` clamped to the
//!    device tier's default ceiling and halved (floor: 16 MB) when
//!    low-resource mode is on — and re-synced via
//!    [`resync_tile_cache_budget`] whenever the runtime config
//!    changes (see `document::low_resource_mode_set`).
//!
//! 3. **N-API-facing JSON snapshots.** The bridge exposes a
//!    `runtime_startup_timeline` / `runtime_tile_cache_stats`
//!    surface (defined in `lib.rs`) that calls into this module
//!    and serialises the result.
//!
//! Kept as a small module so the wiring is reviewable in one
//! place and the data crates (`kcreate_perf`, `kcreate_raster`)
//! stay napi-free.

use std::sync::OnceLock;

use kcreate_perf::startup;
use kcreate_raster::tile::Tile;
use kcreate_raster::TileCache;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::document::runtime_slot;

/// Key shape for the process-wide tile cache: `(layer_id, col, row)`.
/// The layer id is the same `Uuid` that `RasterLayer` instances
/// carry, so callers can compose a key without inventing a
/// secondary identifier scheme.
pub type TileKey = (Uuid, u32, u32);

/// Initialise the global startup timeline once. Subsequent calls
/// are idempotent no-ops thanks to the `INIT_DONE: Mutex<bool>`
/// latch below — plus the `Mutex<Option<_>>` inside
/// `kcreate_perf::startup` which makes the upstream init reentrant
/// safe as well.
///
/// Call from every cold-path entry point that wants to drop marks
/// — there is no ordering requirement, and calling from a hot
/// path is cheap (one mutex acquire to read a bool).
pub fn ensure_startup_initialized() {
    // Use a Mutex<bool> rather than OnceLock<()> so the test-only
    // `reset_init_for_tests` helper can reset both this flag *and*
    // the underlying `kcreate_perf` singleton. With a `OnceLock`,
    // resetting the timeline below would leave the OnceLock
    // exhausted, so subsequent tests would observe an empty
    // timeline that nobody could re-init — making tests subtly
    // order-dependent. (Devin Review ANALYSIS_0003 round 3 on PR
    // #24.)
    let mut done = INIT_DONE.lock();
    if !*done {
        startup::init("bridge.startup");
        // NB: deliberately *not* `bridge.dlopen` — see the module
        // doc. This mark fires on first perf call, which is
        // "shortly after dlopen" in practice but not at dlopen
        // itself.
        startup::mark("bridge.first_call");
        *done = true;
    }
}

// `parking_lot::Mutex::new` is `const` (parking_lot 0.12+), so the
// static can live without a `OnceLock` wrapper. This is the only
// place in the bridge that uses parking_lot in a const context;
// keep it adjacent to `ensure_startup_initialized` so the read
// path is obvious.
static INIT_DONE: Mutex<bool> = Mutex::new(false);

/// Drop a mark on the startup timeline. Convenience wrapper so
/// other bridge modules don't have to depend on `kcreate_perf`
/// directly (keeps the dep-tree visualiser tidy and gives us a
/// single place to add e.g. `tracing::info!` if we ever want
/// double-logging to the system log).
pub fn mark(label: impl Into<String>) {
    ensure_startup_initialized();
    startup::mark(label);
}

/// Open a RAII scope on the global startup timeline. Emits
/// `<label>.start` immediately and `<label>.end` when the returned
/// guard is dropped — including on early `return`, `?`
/// propagation, and panic unwinding.
///
/// Use this instead of paired `perf::mark("foo.start")` /
/// `perf::mark("foo.end")` calls in any function with more than
/// one exit path. The bookend-via-explicit-marks pattern is
/// fragile because every fallible step between the two marks can
/// orphan the `.start` (see Devin Review BUG_0001 on PR #24).
#[must_use = "the returned guard must be held in a binding so the .end mark fires when the scope exits"]
pub fn scope(label: impl Into<String>) -> startup::StartupScope {
    ensure_startup_initialized();
    startup::scope(label)
}

/// JSON snapshot of the startup timeline. Returns the empty JSON
/// object string `{}` if the timeline has never been initialised
/// — this keeps the IPC contract total and the renderer doesn't
/// need a special case for "no timeline yet".
#[must_use]
pub fn startup_timeline_json() -> String {
    match startup::snapshot() {
        Some(report) => report.to_json().unwrap_or_else(|| "{}".to_string()),
        None => "{}".to_string(),
    }
}

/// Process-wide tile cache. Lazily initialised on first access.
/// Budget is seeded from the current `RuntimeConfig`'s
/// `effective_raster_cache_mb()` (i.e. the configured
/// `max_raster_cache_mb` clamped to the device tier's default and
/// halved when low-resource mode is on), converted to bytes via
/// `* 1024 * 1024`.
/// The static lives behind a `parking_lot::Mutex` (the rest of
/// the bridge uses parking_lot, so callers can compose locks
/// without mixing parking_lot + std).
pub fn tile_cache_lock() -> &'static Mutex<TileCache<TileKey>> {
    static CACHE: OnceLock<Mutex<TileCache<TileKey>>> = OnceLock::new();
    CACHE.get_or_init(|| {
        let budget_bytes = current_budget_bytes();
        Mutex::new(TileCache::with_byte_budget(budget_bytes))
    })
}

/// Resync the global tile cache's budget from the current
/// `RuntimeConfig`. Returns the count of evicted entries so the
/// caller (typically `low_resource_mode_set`) can log shrink
/// events. The runtime-config slot is acquired briefly and
/// released before the cache lock is taken, so this never
/// deadlocks against callers that hold either lock.
pub fn resync_tile_cache_budget() -> usize {
    let budget = current_budget_bytes();
    let evicted = tile_cache_lock().lock().set_budget(budget);
    evicted.len()
}

fn current_budget_bytes() -> u64 {
    let mb = runtime_slot().lock().effective_raster_cache_mb();
    mb.saturating_mul(1024 * 1024)
}

/// JSON-shape stats for the diagnostics overlay. Mirrors
/// [`kcreate_perf::Report`] in spirit (snake_case, monotonic
/// units) so the renderer can render both side-by-side.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TileCacheStats {
    /// Total bytes currently held by the cache.
    pub bytes: u64,
    /// Number of cached tile entries.
    pub entries: u64,
    /// Configured byte budget (zero means "evict aggressively",
    /// not "disabled").
    pub budget_bytes: u64,
}

/// Read-only snapshot of the global tile cache's current state.
#[must_use]
pub fn tile_cache_stats() -> TileCacheStats {
    let cache = tile_cache_lock().lock();
    TileCacheStats {
        bytes: cache.bytes(),
        entries: cache.len() as u64,
        budget_bytes: cache.budget(),
    }
}

/// Drop every entry from the global tile cache. Returns the
/// count of entries that were evicted (so the caller can report
/// "freed N tiles, reclaimed M MB" in the UI).
pub fn tile_cache_clear() -> usize {
    tile_cache_lock().lock().clear().len()
}

/// Insert a tile into the global cache. Thin wrapper for tests +
/// future raster-ops integration. Returns the bytes evicted (sum
/// across all eviction tiles) so callers can short-circuit if
/// the insert was effectively a no-op.
pub fn tile_cache_insert(key: TileKey, tile: Tile) -> u64 {
    let evicted = tile_cache_lock().lock().insert(key, tile);
    evicted
        .iter()
        .map(|(_, t)| t.pixels.len() as u64)
        .sum::<u64>()
}

/// Read a tile by key, bumping it to MRU. The returned `Tile` is
/// cloned out of the cache so the cache lock isn't held across
/// downstream work (e.g. PNG encoding).
#[must_use]
pub fn tile_cache_get(key: &TileKey) -> Option<Tile> {
    tile_cache_lock().lock().get(key).cloned()
}

#[cfg(test)]
pub(crate) fn reset_for_tests() {
    // Reset the bridge's INIT latch and the upstream kcreate_perf
    // singleton so every test starts from a clean slate. Without
    // this, the `OnceLock`-shaped predecessor in this module left
    // tests sharing a process-wide timeline whose contents depended
    // on test execution order (Devin Review ANALYSIS_0003 round 3
    // on PR #24). With the `kcreate_perf/test_support` feature
    // enabled for the bridge's dev-deps (see Cargo.toml), the
    // upstream reset is in scope.
    startup::reset_for_tests();
    *INIT_DONE.lock() = false;
    // Re-seed budget from whatever the test's RuntimeConfig is.
    let budget = current_budget_bytes();
    let mut cache = tile_cache_lock().lock();
    cache.clear();
    let evicted = cache.set_budget(budget);
    drop(evicted);
}

#[cfg(test)]
mod tests {
    use serial_test::serial;

    use super::*;

    fn small_tile(seed: u8) -> Tile {
        Tile {
            x: 0,
            y: 0,
            pixels: vec![seed; 64],
            dirty: false,
        }
    }

    #[test]
    #[serial]
    fn startup_timeline_initialises_once_and_emits_first_call_mark() {
        // Reset both the timeline AND the bridge's INIT latch so
        // this test isn't order-dependent on whatever earlier
        // tests did to the process-wide singleton (regression for
        // Devin Review ANALYSIS_0003 round 3 on PR #24).
        reset_for_tests();
        ensure_startup_initialized();
        ensure_startup_initialized();
        ensure_startup_initialized();
        let report: kcreate_perf::Report =
            serde_json::from_str(&startup_timeline_json()).expect("timeline is valid JSON");
        // The `bridge.first_call` mark is emitted exactly once
        // even though `ensure_startup_initialized` was called
        // three times. Counting raw substrings would double-count
        // (every mark label also appears in the derived `phases`
        // array) — parse the JSON instead and count only the
        // `marks` entries.
        let first_call_count = report
            .marks
            .iter()
            .filter(|m| m.label == "bridge.first_call")
            .count();
        assert_eq!(first_call_count, 1);
    }

    #[test]
    #[serial]
    fn mark_records_into_global_timeline() {
        reset_for_tests();
        ensure_startup_initialized();
        mark("test.before_cache");
        let json = startup_timeline_json();
        assert!(json.contains("\"test.before_cache\""));
    }

    #[test]
    #[serial]
    fn tile_cache_seeded_budget_matches_runtime_config() {
        reset_for_tests();
        let mb = runtime_slot().lock().effective_raster_cache_mb();
        let expected = mb.saturating_mul(1024 * 1024);
        assert_eq!(tile_cache_lock().lock().budget(), expected);
    }

    #[test]
    #[serial]
    fn tile_cache_insert_get_round_trip() {
        reset_for_tests();
        let key: TileKey = (Uuid::nil(), 0, 0);
        let evicted = tile_cache_insert(key, small_tile(0xAB));
        assert_eq!(evicted, 0, "cache budget is large enough for one tile");
        let got = tile_cache_get(&key).expect("just inserted");
        assert_eq!(got.pixels[0], 0xAB);
    }

    #[test]
    #[serial]
    fn tile_cache_clear_returns_eviction_count() {
        reset_for_tests();
        for col in 0..4 {
            tile_cache_insert((Uuid::nil(), col, 0), small_tile(col as u8));
        }
        let dropped = tile_cache_clear();
        assert_eq!(dropped, 4);
        assert_eq!(tile_cache_stats().entries, 0);
        assert_eq!(tile_cache_stats().bytes, 0);
    }

    #[test]
    #[serial]
    fn tile_cache_stats_round_trip_through_json() {
        reset_for_tests();
        tile_cache_insert((Uuid::nil(), 0, 0), small_tile(0xCC));
        let stats = tile_cache_stats();
        let json = serde_json::to_string(&stats).expect("serialise");
        let back: TileCacheStats = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(stats, back);
        assert_eq!(stats.entries, 1);
        assert_eq!(stats.bytes, 64);
    }
}
