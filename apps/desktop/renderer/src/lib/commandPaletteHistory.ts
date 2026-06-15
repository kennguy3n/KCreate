// Recent / frequent boosting for the command palette, persisted to
// localStorage so the ranking survives reloads. This is a tiny,
// synchronous, dependency-free store: the palette reads the whole map
// once when it opens (cheap — a single JSON parse) and writes back
// after each command run.
//
// Privacy: only the stable command id and aggregate counters are
// stored — never the search text the user typed, never document
// content. The data never leaves localStorage (no network in the
// editing path; see `crates/kcreate_tests/tests/local_first.rs`).

export const COMMAND_HISTORY_STORAGE_KEY = "kcreate.commandPalette.v1";

/** Per-command usage record. */
interface UsageRecord {
  /** Number of times the command has been run from the palette. */
  count: number;
  /** `Date.now()` of the most recent run. */
  lastUsed: number;
}

type UsageMap = Record<string, UsageRecord>;

// Cap the number of distinct commands we remember so a long-lived
// install can't grow the entry unbounded. When exceeded we drop the
// least-recently-used records. 200 comfortably covers every command
// the palette can list with headroom for future additions.
const MAX_RECORDS = 200;

// Half-life (ms) for the recency component of the boost. A command
// used within the last few minutes gets the full recency weight; the
// contribution decays smoothly so yesterday's pick doesn't outrank
// what the user is doing right now. ~12h chosen so within-session
// ordering is dominated by this session's activity.
const RECENCY_HALF_LIFE_MS = 12 * 60 * 60 * 1000;

function isUsageRecord(value: unknown): value is UsageRecord {
  if (typeof value !== "object" || value === null) return false;
  const record = value as Record<string, unknown>;
  return (
    typeof record.count === "number" &&
    Number.isFinite(record.count) &&
    record.count >= 0 &&
    typeof record.lastUsed === "number" &&
    Number.isFinite(record.lastUsed)
  );
}

function readMap(): UsageMap {
  if (typeof window === "undefined" || !window.localStorage) return {};
  let raw: string | null;
  try {
    raw = window.localStorage.getItem(COMMAND_HISTORY_STORAGE_KEY);
  } catch {
    // Private-mode / disabled storage — degrade to no history rather
    // than throwing into the palette open path.
    return {};
  }
  if (!raw) return {};
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return {};
  }
  if (typeof parsed !== "object" || parsed === null) return {};
  const out: UsageMap = {};
  for (const [id, value] of Object.entries(parsed)) {
    if (isUsageRecord(value)) out[id] = { ...value };
  }
  return out;
}

function writeMap(map: UsageMap): void {
  if (typeof window === "undefined" || !window.localStorage) return;
  // Evict least-recently-used records past the cap before persisting.
  const ids = Object.keys(map);
  if (ids.length > MAX_RECORDS) {
    ids
      .sort((a, b) => (map[a]?.lastUsed ?? 0) - (map[b]?.lastUsed ?? 0))
      .slice(0, ids.length - MAX_RECORDS)
      .forEach((id) => delete map[id]);
  }
  try {
    window.localStorage.setItem(
      COMMAND_HISTORY_STORAGE_KEY,
      JSON.stringify(map),
    );
  } catch {
    // Quota / disabled storage — the boost is a nicety, not load-
    // bearing, so swallow rather than break the command run.
  }
}

/**
 * Snapshot of command-usage stats, read once when the palette opens.
 * Exposes a pure `boost(id)` so ranking stays a synchronous, testable
 * function of the snapshot rather than re-reading storage per command.
 */
export interface CommandHistory {
  /**
   * Additive ranking boost for `id`, combining frequency (log-scaled
   * so a heavily-used command can't dominate forever) and recency
   * (exponential decay). Returns 0 for never-used commands. `now` is
   * injectable for deterministic tests.
   */
  boost(id: string, now?: number): number;
  /** Command ids ordered most-recently-used first. */
  recentIds(): string[];
}

const FREQUENCY_WEIGHT = 6;
const RECENCY_WEIGHT = 10;

/** Wrap an in-memory usage map in the `CommandHistory` read interface. */
function makeHistory(map: UsageMap): CommandHistory {
  return {
    boost(id, now = Date.now()): number {
      const record = map[id];
      if (record === undefined) return 0;
      const frequency = Math.log2(record.count + 1) * FREQUENCY_WEIGHT;
      const age = Math.max(0, now - record.lastUsed);
      const recency =
        Math.pow(2, -age / RECENCY_HALF_LIFE_MS) * RECENCY_WEIGHT;
      return frequency + recency;
    },
    recentIds(): string[] {
      return Object.keys(map).sort(
        (a, b) => (map[b]?.lastUsed ?? 0) - (map[a]?.lastUsed ?? 0),
      );
    },
  };
}

/** Read the persisted usage map and wrap it in a `CommandHistory`. */
export function loadCommandHistory(): CommandHistory {
  return makeHistory(readMap());
}

/**
 * Record one run of `id`, bumping its count and recency, and persist.
 * Returns a fresh `CommandHistory` reflecting the update so a caller
 * holding the palette open can re-rank without re-reading storage.
 * `now` is injectable for deterministic tests.
 */
export function recordCommandUse(
  id: string,
  now: number = Date.now(),
): CommandHistory {
  const map = readMap();
  const existing = map[id];
  map[id] = {
    count: (existing?.count ?? 0) + 1,
    lastUsed: now,
  };
  writeMap(map);
  return makeHistory(map);
}
