import { useEffect, useMemo, useState } from "react";

import type { SessionEvent, SessionLockEntry } from "../../../shared/scene";

/**
 * Reactive view of the advisory edit-lock roster maintained by the
 * Phase 4 Block 8 collab session.
 *
 * Returns a `Map<nodeId, SessionLockEntry>` keyed by the locked
 * node id so callers can do a constant-time lookup ("is this node
 * locked, and by whom?"). The map is rebuilt every time a
 * `locksChanged` `SessionEvent` arrives, with an initial fetch on
 * mount to handle the case where a session was already running
 * before this hook attached.
 *
 * Outside an active session — or when no KChat group is signed
 * in — the bridge's `session.locks()` returns an empty list
 * unconditionally, so the hook simply yields an empty map. That
 * matches the "no locks, nothing to grey out" UX.
 *
 * The hook subscribes to **every** session event but only acts on
 * the `locksChanged` variant. Cursor / presence updates are far
 * higher-volume than lock claims, but the renderer-side filter is
 * a single discriminator check (`ev.kind === "locksChanged"`) so
 * the overhead is negligible compared to keeping a dedicated
 * channel synced.
 *
 * Returns the lock map and an `error` message that flips non-null
 * if the bridge call fails on either the initial fetch or a
 * refresh after a `locksChanged` event. The error never causes
 * the map to be cleared — stale state is preferable to flashing
 * an empty lock roster (which would falsely re-enable controls).
 */
export interface UseSessionLocksResult {
  /** node id → lock entry. Excludes any entries we hold ourselves. */
  remoteLocks: Map<string, SessionLockEntry>;
  /** All entries (local + remote). Used by hosts that need both. */
  allLocks: Map<string, SessionLockEntry>;
  /** Local peer's base64url id, or `null` if no session is running. */
  selfPeerId: string | null;
  /** Last error message from the bridge, if any. */
  error: string | null;
}

export function useSessionLocks(): UseSessionLocksResult {
  const [allLocks, setAllLocks] = useState<Map<string, SessionLockEntry>>(
    () => new Map(),
  );
  const [selfPeerId, setSelfPeerId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;

    const reload = async (): Promise<void> => {
      try {
        const info = await window.kcreate.session.info();
        if (cancelled) return;
        setSelfPeerId(info?.peerId ?? null);
        // `session.locks()` is safe to call even when no session is
        // running — it returns `[]` rather than throwing.
        const entries = await window.kcreate.session.locks();
        if (cancelled) return;
        setAllLocks(buildLockMap(entries));
        setError(null);
      } catch (e) {
        if (!cancelled) {
          setError(errorMessage(e));
        }
      }
    };

    void reload();

    // The bridge's session-event channel is the authoritative
    // change signal for lock state. We re-fetch the full roster
    // on every `locksChanged` event rather than diffing the event
    // payload locally — that keeps the renderer state machine
    // simple and the bridge as the source of truth, at the cost
    // of one extra IPC round-trip per claim/release. Both sides
    // are in-process so the latency is well under a frame.
    const unsubscribe = window.kcreate.session.onEvent((ev: SessionEvent) => {
      if (ev.kind === "locksChanged") {
        void reload();
      } else if (ev.kind === "peerLeft") {
        // A peer leaving auto-releases every lock they held
        // (Block 8 wires this in `apply_event`); the bridge will
        // emit a `locksChanged` of its own, but we also re-fetch
        // here defensively in case the events arrive out of
        // order or one is dropped during teardown.
        void reload();
      }
    });

    return () => {
      cancelled = true;
      unsubscribe();
    };
  }, []);

  // Memoise the remote-only derivation so RightPanel (and any
  // future consumers that subscribe to this hook) don't see a new
  // `Map` reference on every render of an unrelated state change.
  // The filter is O(n) in the number of active locks and the
  // allocation is small, but recomputing it on every render still
  // pollutes downstream `useEffect` dep arrays that key off
  // `remoteLocks`.
  //
  // When `selfPeerId === null` we return an empty map rather than
  // passing `allLocks` through unfiltered. The JSDoc on
  // `remoteLocks` contracts the result to "entries excluding ones
  // we hold ourselves" — but `setSelfPeerId(null)` and
  // `setAllLocks(...)` inside `reload()` are separated by an
  // `await`, so a render landing between the two would otherwise
  // see `selfPeerId = null` *and* the previous session's
  // `allLocks` still populated, which would (a) violate the doc
  // contract and (b) cause RightPanel to surface our *own* locks
  // as a "locked by another peer" banner for one frame. Returning
  // an empty map keeps the invariant honest no matter the order
  // the two setState calls flush, and matches the "no session ⇒
  // no remote peers" semantic the bridge enforces anyway.
  const remoteLocks = useMemo(
    () =>
      selfPeerId === null
        ? new Map<string, SessionLockEntry>()
        : new Map(
            Array.from(allLocks.entries()).filter(
              ([, entry]) => entry.holderPeerId !== selfPeerId,
            ),
          ),
    [allLocks, selfPeerId],
  );

  return { remoteLocks, allLocks, selfPeerId, error };
}

function buildLockMap(
  entries: readonly SessionLockEntry[],
): Map<string, SessionLockEntry> {
  const map = new Map<string, SessionLockEntry>();
  for (const entry of entries) {
    // The bridge guarantees one entry per node id (claims
    // overwrite under last-claim-wins semantics), but be defensive
    // in case a future protocol revision relaxes that — keep the
    // most recent `acquiredAt` if duplicates ever appear.
    const existing = map.get(entry.nodeId);
    if (!existing || entry.acquiredAt > existing.acquiredAt) {
      map.set(entry.nodeId, entry);
    }
  }
  return map;
}

function errorMessage(e: unknown): string {
  if (e instanceof Error) return e.message;
  if (typeof e === "string") return e;
  try {
    return JSON.stringify(e);
  } catch {
    return String(e);
  }
}
