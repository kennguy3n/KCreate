import { useEffect, useMemo, useRef, useState } from "react";

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
 * the `locksChanged` / `peerJoined` / `peerLeft` variants. Cursor /
 * presence updates are far higher-volume than lock claims, but the
 * renderer-side filter is a single discriminator check so the
 * overhead is negligible compared to keeping a dedicated channel
 * synced.
 *
 * **Subscription lifecycle**: the effect uses `[]` deps and never
 * re-subscribes. The initial `reload()` on mount fetches whatever
 * state the bridge currently has, and the long-lived subscription
 * picks up everything that changes after. This means the hook is
 * **not** coupled to the session-start IPC handshake — even if the
 * bridge never explicitly emitted a `locksChanged` event on join
 * (it does, but this is defence-in-depth against a future change),
 * the `peerJoined` events that fire during the welcome roster
 * exchange would still trigger a refresh, so the lock roster
 * always converges within one IPC round-trip of any peer-set
 * change. The `[]` deps are deliberate: changing them would
 * re-mount the subscription on every consumer render, which would
 * be the actual bug.
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

  // Monotonic token guarding against out-of-order `reload()`
  // completions. The bridge IPC is in-process and sub-millisecond, so
  // realistically two events arriving back-to-back will resolve in
  // FIFO order — but JavaScript's task scheduler makes no guarantee
  // about that, and a high-volume `locksChanged` burst on a busy
  // session could in principle let an earlier `reload()` resolve
  // after a later one and silently overwrite the fresh roster with
  // stale data. Same idea as `AltTextSection`'s `requestTokenRef` and
  // `EditorPage`'s presence-broadcast guard, applied to the hook's
  // event-driven re-fetch loop: every invocation captures a token,
  // and only commits its result if the token is still the latest one
  // at completion time. Stale completions are dropped on the floor
  // (matching the docstring's "stale state is preferable to flashing
  // empty" — but only when the stale snapshot is *older* than what we
  // already have, never *newer*).
  const reloadTokenRef = useRef(0);

  useEffect(() => {
    let cancelled = false;

    const reload = async (): Promise<void> => {
      reloadTokenRef.current += 1;
      const token = reloadTokenRef.current;
      try {
        // Fetch both halves of the snapshot before touching React
        // state. React 18 only batches updates within the same
        // microtask (between `await`s), so if we issued both
        // setState calls with `await`s between them, a render
        // landing between the two commits would observe the new
        // peer id with the previous session's lock map — briefly
        // mis-attributing peer-A's locks to peer-B during a
        // session transition. Pulling both reads up-front and then
        // issuing both setState calls adjacent (no `await` between
        // them) lets React batch the commits, so callers always
        // observe a consistent `(selfPeerId, allLocks)` pair.
        const info = await window.kcreate.session.info();
        if (cancelled || reloadTokenRef.current !== token) return;
        // `session.locks()` is safe to call even when no session is
        // running — it returns `[]` rather than throwing.
        const entries = await window.kcreate.session.locks();
        if (cancelled || reloadTokenRef.current !== token) return;
        const peerId = info?.peerId ?? null;
        const lockMap = buildLockMap(entries);
        setSelfPeerId(peerId);
        setAllLocks(lockMap);
        setError(null);
      } catch (e) {
        if (!cancelled && reloadTokenRef.current === token) {
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
      } else if (ev.kind === "peerJoined") {
        // Defence-in-depth against the hook's only implicit coupling
        // with the bridge: if a future change ever stopped emitting
        // `locksChanged` as part of the welcome handshake, refreshing
        // on `peerJoined` guarantees that any locks the joining peer
        // already holds (or that the local user holds in a host the
        // peer is dialing into) propagate to the renderer within one
        // IPC round-trip. The roster is a small read and lock claims
        // are user-frequency, so the extra IPC on each join is
        // negligible compared to the failure mode of stale state.
        void reload();
      } else if (ev.kind === "sessionStarted") {
        // Round 11: local-side lifecycle transition the existing
        // `peer*` events never signal (those fire for remote peers
        // only). Re-fetch so the roster + `selfPeerId` reflect the
        // freshly-started session immediately — without this hop
        // there's a window between `session_start` returning and
        // the first remote `peerJoined` arriving where the hook
        // still reports `selfPeerId = null` and any locks claimed
        // locally during that window get mis-attributed to "remote"
        // in `remoteLocks`.
        void reload();
      } else if (ev.kind === "sessionLeft") {
        // Round 11: local `session.leave()` doesn't go through the
        // bridge's regular event queue (the queue is dropped as part
        // of the leave). `main.ts` synthesises this event on the
        // same channel so we can drop the stale roster instead of
        // leaving the `LockBanner` in `RightPanel` showing peers
        // from the previous session. The `reload()` will observe
        // `session.info()` returning `null` and `session.locks()`
        // returning `[]`, committing `selfPeerId = null` and
        // `allLocks = new Map()` together.
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
  // we hold ourselves" — and although `reload()` now commits both
  // setState calls adjacent (so within a single `reload()` the
  // `(selfPeerId, allLocks)` pair is always consistent), two
  // *separate* sources could still land in inconsistent order:
  // - the initial mount `reload()` resets `selfPeerId` to null
  //   between sessions while `allLocks` may briefly retain the
  //   previous session's entries until the next `setAllLocks`
  //   commit lands;
  // - a future contributor could add another setter that touches
  //   only one of the two fields, breaking the within-`reload()`
  //   batching invariant.
  // In either case, returning an empty map keeps the invariant
  // honest: without this guard, every lock entry would pass the
  // `holderPeerId !== selfPeerId` filter (since `null !== anyId`)
  // and the user's own locks would briefly surface as remote
  // "locked by …" banners. Matches the "no session ⇒ no remote
  // peers" semantic the bridge enforces anyway, so the guard has
  // no observable downside.
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
