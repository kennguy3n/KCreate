// ConflictToast — Phase 7 Task 16.
//
// Renders a brief non-blocking toast in the bottom-right of the editor
// whenever the CRDT resolver emits a `conflictResolved` event where the
// LOCAL peer was the loser ("Your edit to <node> was overridden by
// <peer>'s edit"). Auto-dismisses after 5 seconds; clicking it triggers
// the local undo so the user can quickly revert to their version.
//
// Owns its own session-event subscription + peer roster so the host
// doesn't need to thread state in. Self-contained UI — drop it once at
// the EditorPage level and it works for the whole app.

import { useEffect, useRef, useState } from "react";

import type {
  NodeInfo,
  SessionEvent,
  SessionPeer,
} from "../../../shared/scene";
import { colors, radius, spacing } from "../styles/tokens";

import { colorForPeer } from "./CursorOverlay";

/// Auto-dismiss delay in ms. Picked to match the rest of the app's
/// status banners (StatusFooter, LowResourceBanner).
const TOAST_DURATION_MS = 5000;

/// How many toasts to stack vertically before dropping the oldest.
/// We cap so a long burst of conflicts doesn't fill the viewport.
const MAX_VISIBLE_TOASTS = 3;

interface ToastEntry {
  /// Monotonic id so React keys stay stable even when two conflicts
  /// land on the same node within the same ms.
  id: number;
  /// Node id whose edit was overridden. Used by the click handler to
  /// scroll the layer panel + ask the bridge to undo *that specific*
  /// edit rather than the last op overall.
  nodeId: string;
  /// Free-form field name from the CRDT resolver (e.g. `"fill.color"`).
  field: string;
  /// Display name of the peer that won the tiebreak. Resolved at
  /// event-receive time so the toast keeps the right name even if the
  /// peer disconnects before the 5 s auto-dismiss fires.
  winnerName: string;
  /// Peer id of the winner — used to colour the toast accent stripe.
  winnerPeerId: string;
  /// Human-friendly node name, resolved at event-receive time from the
  /// `NodeInfo[]` snapshot so the toast says "rectangle 4" rather than
  /// a UUID.
  nodeName: string;
}

export interface ConflictToastProps {
  /**
   * Document tree snapshot. Used to resolve node-id → node-name when
   * a `conflictResolved` event fires. Allowed to be stale; the toast
   * just falls back to the UUID prefix in that case.
   */
  nodes: NodeInfo[];
  /**
   * Optional click-handler so the host can wire the toast's "Undo"
   * action through to its undo pipeline (which knows about
   * editor-page-scoped concerns like dirty flags, selection
   * restoration, etc.). Defaults to a no-op + log so the toast still
   * renders cleanly in isolation.
   */
  onUndoClick?: (nodeId: string) => void;
}

export function ConflictToast({
  nodes,
  onUndoClick,
}: ConflictToastProps): JSX.Element | null {
  const [toasts, setToasts] = useState<ToastEntry[]>([]);
  const [peers, setPeers] = useState<SessionPeer[]>([]);
  const [localPeerId, setLocalPeerId] = useState<string | null>(null);

  const peersRef = useRef<SessionPeer[]>(peers);
  peersRef.current = peers;
  const nodesRef = useRef<NodeInfo[]>(nodes);
  nodesRef.current = nodes;
  const localPeerIdRef = useRef<string | null>(localPeerId);
  localPeerIdRef.current = localPeerId;

  const toastIdSeq = useRef<number>(0);
  // Track in-flight auto-dismiss timers so unmount can cancel them.
  // Otherwise each pending timer retains the `setToasts` closure
  // (worst case `MAX_VISIBLE_TOASTS` dangling 5 s timers on unmount,
  // which fires a no-op `setToasts` after the component is gone — a
  // very small leak in React 18+ but still worth eliminating).
  const dismissTimersRef = useRef<Set<number>>(new Set());

  // Pull peers + local id on mount, and refresh on session lifecycle
  // events. We use the latest values in the event handler via refs so
  // the toast resolves names correctly even if the event arrives in
  // the same tick as a peer roster update.
  useEffect(() => {
    let cancelled = false;
    // Capture the timers `Set` reference once so the cleanup
    // closure refers to the same container the effect populates,
    // satisfying `react-hooks/exhaustive-deps` and dodging the
    // theoretical case where `.current` is reassigned mid-life.
    const dismissTimers = dismissTimersRef.current;
    const refreshPeers = async (): Promise<void> => {
      try {
        const list = await window.kcreate.session.peers();
        if (!cancelled) setPeers(list);
      } catch {
        // Bridge transient — keep stale roster.
      }
    };
    const refreshLocal = async (): Promise<void> => {
      try {
        const info = await window.kcreate.session.info();
        if (!cancelled) setLocalPeerId(info?.peerId ?? null);
      } catch {
        // Bridge transient.
      }
    };
    void refreshPeers();
    void refreshLocal();

    const handle = (ev: SessionEvent): void => {
      switch (ev.kind) {
        case "peerJoined":
        case "peerLeft":
        case "peerKicked":
          void refreshPeers();
          return;
        case "sessionStarted":
        case "sessionLeft":
          void refreshLocal();
          void refreshPeers();
          // Clear stale toasts on session transitions — the peer-id
          // reference would no longer line up.
          setToasts([]);
          return;
        case "conflictResolved": {
          const localId = localPeerIdRef.current;
          // Only toast when WE are the loser. Other-loser conflicts
          // happen all the time in normal multi-peer editing and
          // would spam the UI.
          if (localId == null || ev.loserPeerId !== localId) return;
          const winner = peersRef.current.find(
            (p) => p.peerId === ev.winnerPeerId,
          );
          const node = nodesRef.current.find((n) => n.id === ev.nodeId);
          const id = ++toastIdSeq.current;
          setToasts((prev) => {
            const next = [
              ...prev,
              {
                id,
                nodeId: ev.nodeId,
                field: ev.field,
                winnerName:
                  winner?.displayName ?? shortPeerId(ev.winnerPeerId),
                winnerPeerId: ev.winnerPeerId,
                nodeName: node?.name ?? shortNodeId(ev.nodeId),
              },
            ];
            // Cap the visible stack.
            return next.slice(-MAX_VISIBLE_TOASTS);
          });
          // Schedule auto-dismiss. Track the timer id so unmount
          // can cancel it; without that the closure retains
          // `setToasts` until the timer fires (silent no-op in
          // React 18+, but still a leak we'd rather avoid).
          const timerId = window.setTimeout(() => {
            dismissTimers.delete(timerId);
            setToasts((prev) => prev.filter((t) => t.id !== id));
          }, TOAST_DURATION_MS);
          dismissTimers.add(timerId);
          return;
        }
        default:
          return;
      }
    };

    const unsubscribe = window.kcreate.session.onEvent(handle);
    return () => {
      cancelled = true;
      unsubscribe();
      // Cancel every in-flight auto-dismiss timer so post-unmount
      // ticks don't run `setToasts` on a dead component.
      for (const timerId of dismissTimers) {
        window.clearTimeout(timerId);
      }
      dismissTimers.clear();
    };
  }, []);

  if (toasts.length === 0) {
    return null;
  }

  return (
    <div
      style={{
        position: "absolute",
        bottom: spacing.md,
        right: spacing.md,
        display: "flex",
        flexDirection: "column",
        gap: spacing.xs,
        pointerEvents: "auto",
        zIndex: 50,
      }}
      role="status"
      aria-live="polite"
    >
      {toasts.map((toast) => {
        const accent = colorForPeer(toast.winnerPeerId);
        return (
          <button
            key={toast.id}
            type="button"
            onClick={() => {
              setToasts((prev) => prev.filter((t) => t.id !== toast.id));
              if (onUndoClick) {
                onUndoClick(toast.nodeId);
              } else {
                // Fall back to the renderer's plain `kcreate/document/undo`
                // entry point so the toast is still useful when no host
                // handler is wired. The host wiring is the typical path
                // — we keep this as a defensive default so the component
                // doesn't silently break if `onUndoClick` is omitted.
                void window.kcreate.document.undo();
              }
            }}
            style={{
              background: colors.bgSoft,
              color: colors.text,
              border: `1px solid ${colors.border}`,
              borderLeft: `4px solid ${accent}`,
              borderRadius: radius.sm,
              padding: `${spacing.sm}px ${spacing.md}px`,
              boxShadow: "0 8px 24px rgba(0, 0, 0, 0.18)",
              cursor: "pointer",
              textAlign: "left",
              maxWidth: 360,
              font: "inherit",
            }}
            title="Click to undo"
          >
            <div style={{ fontWeight: 600, fontSize: 13 }}>
              {toast.winnerName} overrode your edit
            </div>
            <div
              style={{
                fontSize: 12,
                color: colors.textMuted,
                marginTop: 2,
              }}
            >
              <code style={{ background: "transparent" }}>
                {toast.nodeName}
              </code>
              {" · "}
              <code style={{ background: "transparent" }}>{toast.field}</code>
              {" · click to undo"}
            </div>
          </button>
        );
      })}
    </div>
  );
}

function shortPeerId(peerId: string): string {
  return peerId.length > 8 ? peerId.slice(0, 8) + "…" : peerId;
}

function shortNodeId(nodeId: string): string {
  return nodeId.length > 8 ? nodeId.slice(0, 8) + "…" : nodeId;
}
