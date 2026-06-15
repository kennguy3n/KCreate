// Recently-used elements store (H3).
//
// The Elements panel shows a "Recently used" row leading the grid.
// Recording must happen at the single point an insert *actually
// succeeds* — `EditorPage.insertElementAt`, the one code path both
// click-to-insert and drag-and-drop funnel through — so that a
// cancelled drag (the thumbnail is dragged but released off the canvas
// drop zone, so no insert ever runs) never leaves a phantom entry, and
// a failed insert never records either.
//
// `AssetsPanel` is a pure *consumer*: it subscribes to this store and
// re-renders when the list changes but never writes to it. This keeps
// the recently-used list a single source of truth shared between the
// host (writer) and the panel (reader), persisted to `localStorage`
// (versioned key) so the row survives reloads and stays in sync across
// editor windows via the `storage` event.

// `localStorage` key + cap. Versioned so a future shape change can be
// migrated rather than mis-parsed.
const RECENT_KEY = "kcreate.elements.recent.v1";
const RECENT_MAX = 12;

type Listener = () => void;
const listeners = new Set<Listener>();

// Stable empty reference so an absent / empty list always yields the
// same array identity (no `useSyncExternalStore` churn).
const EMPTY: readonly string[] = Object.freeze([]);

// Cache the last raw `localStorage` string and its parsed snapshot.
// `getRecentElementIds` re-reads the (cheap) string each call but only
// reparses — and hands back a *new* array reference — when the
// persisted value actually changed. Returning a stable reference while
// unchanged is what makes it safe as a `useSyncExternalStore`
// snapshot (a fresh array every call would loop forever).
let lastRaw: string | null | undefined;
let snapshot: readonly string[] = EMPTY;

function rawValue(): string | null {
  try {
    return window.localStorage.getItem(RECENT_KEY);
  } catch {
    // localStorage unavailable (private mode / disabled) — treat as
    // empty. Never throw from a render-path read.
    return null;
  }
}

function parse(raw: string | null): readonly string[] {
  if (raw === null) return EMPTY;
  try {
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return EMPTY;
    const ids = parsed
      .filter((x): x is string => typeof x === "string")
      .slice(0, RECENT_MAX);
    return ids.length === 0 ? EMPTY : ids;
  } catch {
    return EMPTY;
  }
}

/// The recently-used asset ids, newest first. Re-reads `localStorage`
/// but returns a STABLE array reference while the persisted value is
/// unchanged, so it is safe to use directly as a
/// `useSyncExternalStore` snapshot.
export function getRecentElementIds(): readonly string[] {
  const raw = rawValue();
  if (raw !== lastRaw) {
    lastRaw = raw;
    snapshot = parse(raw);
  }
  return snapshot;
}

function emit(): void {
  for (const listener of listeners) listener();
}

// One process-wide `storage` listener, installed lazily on first
// subscribe, so a record in another editor window (or a manual
// `localStorage` change) re-notifies every mounted panel. Never
// removed — it is a single harmless listener for the app lifetime.
let storageBound = false;
function ensureStorageListener(): void {
  if (storageBound || typeof window === "undefined") return;
  storageBound = true;
  window.addEventListener("storage", (e: StorageEvent) => {
    // `clear()` fires with `key === null`; a targeted write fires with
    // our key. Ignore writes to other keys.
    if (e.key !== null && e.key !== RECENT_KEY) return;
    emit();
  });
}

/// Subscribe to recently-used changes. Returns an unsubscribe fn.
export function subscribeRecentElements(listener: Listener): () => void {
  ensureStorageListener();
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/// Record a *successfully inserted* asset at the head of the
/// recently-used list (deduped, capped at {@link RECENT_MAX}), persist
/// it, and notify subscribers. Called once per real insert from the
/// host. A blank id is a no-op. If persistence fails (private mode /
/// quota) the list is left unchanged rather than diverging from disk.
export function recordRecentElement(id: string): void {
  if (id.length === 0) return;
  const next = [id, ...getRecentElementIds().filter((x) => x !== id)].slice(
    0,
    RECENT_MAX,
  );
  try {
    window.localStorage.setItem(RECENT_KEY, JSON.stringify(next));
  } catch {
    // localStorage unavailable (private mode / quota exhausted). The
    // row simply won't update this session; not worth surfacing.
    return;
  }
  emit();
}
