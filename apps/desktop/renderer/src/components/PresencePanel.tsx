// PresencePanel — Phase 3 LAN collaboration UI.
//
// Lets the user start a local collab session, see the live peer
// roster (with online indicator, display name, and cert
// fingerprint preview), and dial discovered peers. Manages the
// long-lived `kcreate/session/event` subscription so other parts
// of the UI (e.g. the CanvasHost cursor pump) don't have to.
//
// Session state lives in this panel for now; in a later iteration
// we'll lift it into a React context so other panels (Layers,
// Inspect) can show per-peer colour swatches next to selections.

import { useCallback, useEffect, useRef, useState } from "react";

import type {
  ProjectInfo,
  SessionEvent,
  SessionPeer,
  SessionStartReport,
} from "../../../shared/scene";
import { colors, radius, spacing } from "../styles/tokens";

/// LocalStorage key for the persistent Ed25519 seed. The renderer
/// generates one on first use (32 cryptographically-random bytes,
/// base64url-encoded) and reuses it across launches so the same
/// machine presents a stable peer identity. Wiping the key wipes
/// the identity (and any trust other peers placed in it).
const SEED_STORAGE_KEY = "kcreate.session.seed";
/// LocalStorage key for the persistent display name shown to other
/// peers. Falls back to the OS username via `navigator.userAgent`
/// parsing only when nothing is set — see `defaultDisplayName`.
const DISPLAY_NAME_STORAGE_KEY = "kcreate.session.displayName";

export interface PresencePanelProps {
  /** Active project — `null` when no project is open. */
  project: ProjectInfo | null;
  /** Status bubbles back to the editor's footer strip. */
  onStatus?: (msg: string | null) => void;
}

/// Represents a peer that the bridge has announced (via mDNS or a
/// pasted link) but the local user hasn't dialled yet.
interface DiscoveredEntry {
  peerId: string;
  publicKey: string;
  displayName: string;
  projectId: string;
  socketAddr: string;
  certFingerprint: string;
}

export function PresencePanel({
  project,
  onStatus,
}: PresencePanelProps): JSX.Element {
  // The seed is generated/loaded once on mount; users have no reason
  // to rotate it during a session (rotating breaks any peer that
  // trusted the old key), so we keep this in a plain const after the
  // initial state init. The `_setSeed` setter is retained to surface
  // a deliberate "reset identity" button in a future iteration.
  const [seed] = useState<string>(() => loadOrGenerateSeed());
  const [displayName, setDisplayName] = useState<string>(() =>
    loadOrInitDisplayName(),
  );
  const [advertiseMdns, setAdvertiseMdns] = useState<boolean>(true);
  const [report, setReport] = useState<SessionStartReport | null>(null);
  const [peers, setPeers] = useState<SessionPeer[]>([]);
  const [discovered, setDiscovered] = useState<Map<string, DiscoveredEntry>>(
    new Map(),
  );
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Persist seed/display name on change so a refresh doesn't reset
  // the peer identity (which would force every other peer to
  // re-trust the new fingerprint).
  useEffect(() => {
    try {
      window.localStorage.setItem(SEED_STORAGE_KEY, seed);
    } catch {
      // localStorage may be unavailable in some test contexts —
      // the panel still works, the identity just won't persist.
    }
  }, [seed]);
  useEffect(() => {
    try {
      window.localStorage.setItem(DISPLAY_NAME_STORAGE_KEY, displayName);
    } catch {
      // See above.
    }
  }, [displayName]);

  // Read the cached report on mount in case a session is already
  // running (e.g. user navigated away and back).
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const info = await window.kcreate.session.info();
        if (!cancelled) setReport(info);
        if (info && !cancelled) {
          const list = await window.kcreate.session.peers();
          if (!cancelled) setPeers(list);
        }
      } catch (e) {
        if (!cancelled) setError(errMsg(e));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  // Subscribe to the push channel. The subscription stays open for
  // the lifetime of the panel; if the user navigates away the
  // unsubscribe runs in the cleanup callback.
  const peersRef = useRef(peers);
  peersRef.current = peers;
  useEffect(() => {
    // `onEvent` expects a synchronous `(SessionEvent) => void`
    // callback. `handleEvent` is async (it awaits a bridge round
    // trip for `peers()` updates), so wrapping it in an async arrow
    // would silently fire-and-forget the returned promise — any
    // future await that throws outside the inner try/catch becomes
    // an unhandled rejection. Instead, kick the async work off with
    // an explicit `.catch` so every rejection path is visible.
    const unsubscribe = window.kcreate.session.onEvent((ev) => {
      handleEvent(ev, {
        setPeers,
        setDiscovered,
        peersRef,
        onStatus,
      }).catch((err) => {
        // PresencePanel never blocks the user on a single bad event:
        // log it for diagnostics, surface a status line, and let the
        // next event try again. We don't `setError` here because
        // `error` is reserved for actions the user just took
        // (start/join/leave) — a background event failure shouldn't
        // override that.
        console.error("[PresencePanel] event handler failed:", err);
        onStatus?.(`Session: event handler failed — ${errMsg(err)}`);
      });
    });
    return () => {
      unsubscribe();
    };
  }, [onStatus]);

  const handleStart = useCallback(async () => {
    if (!project) {
      setError("Open a project first — collab sessions are per-project.");
      return;
    }
    setBusy(true);
    setError(null);
    onStatus?.("Session: starting…");
    try {
      const next = await window.kcreate.session.start(
        seed,
        displayName.trim() || "Anonymous",
        project.id,
        advertiseMdns,
      );
      setReport(next);
      setPeers([]);
      setDiscovered(new Map());
      onStatus?.(`Session: hosting as ${next.displayName}.`);
    } catch (e) {
      setError(errMsg(e));
      onStatus?.(`Session: start failed — ${errMsg(e)}`);
    } finally {
      setBusy(false);
    }
  }, [project, seed, displayName, advertiseMdns, onStatus]);

  const handleLeave = useCallback(async () => {
    setBusy(true);
    onStatus?.("Session: leaving…");
    try {
      await window.kcreate.session.leave();
      setReport(null);
      setPeers([]);
      setDiscovered(new Map());
      onStatus?.("Session: left.");
    } catch (e) {
      setError(errMsg(e));
      onStatus?.(`Session: leave failed — ${errMsg(e)}`);
    } finally {
      setBusy(false);
    }
  }, [onStatus]);

  const handleJoin = useCallback(
    async (entry: DiscoveredEntry) => {
      setBusy(true);
      onStatus?.(`Session: dialling ${entry.displayName}…`);
      try {
        await window.kcreate.session.join(
          entry.peerId,
          entry.publicKey,
          entry.displayName,
          entry.socketAddr,
          entry.certFingerprint,
        );
        onStatus?.(`Session: dialled ${entry.displayName}.`);
      } catch (e) {
        setError(errMsg(e));
        onStatus?.(`Session: dial failed — ${errMsg(e)}`);
      } finally {
        setBusy(false);
      }
    },
    [onStatus],
  );

  const running = report !== null;

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: spacing.md }}>
      <Section title="Local identity">
        <Field label="Display name">
          <input
            type="text"
            value={displayName}
            onChange={(e) => setDisplayName(e.target.value)}
            disabled={running}
            style={inputStyle}
            placeholder="Your name (visible to peers)"
          />
        </Field>
        <Field label="Peer ID">
          <code style={codeStyle}>{report?.peerId ?? "—"}</code>
        </Field>
        <Field label="Cert fingerprint (SHA-256)">
          <code style={codeStyle}>
            {report ? shortenFingerprint(report.certFingerprint) : "—"}
          </code>
        </Field>
      </Section>
      <Section title="Session">
        <label style={checkboxRowStyle}>
          <input
            type="checkbox"
            checked={advertiseMdns}
            disabled={running}
            onChange={(e) => setAdvertiseMdns(e.target.checked)}
          />
          <span>Announce on local network (mDNS)</span>
        </label>
        <div style={{ display: "flex", gap: spacing.sm }}>
          {running ? (
            <button
              type="button"
              onClick={handleLeave}
              disabled={busy}
              style={buttonStyle(true)}
            >
              Leave session
            </button>
          ) : (
            <button
              type="button"
              onClick={handleStart}
              disabled={busy || !project}
              style={buttonStyle(false)}
            >
              Start session
            </button>
          )}
        </div>
        {!project ? (
          <div style={hintStyle}>
            Open a project to host or join a collaboration session.
          </div>
        ) : null}
      </Section>
      <Section title="Peers">
        {peers.length === 0 ? (
          <div style={hintStyle}>No peers connected.</div>
        ) : (
          <ul style={listStyle}>
            {peers.map((p) => (
              <PeerRow key={p.peerId} peer={p} />
            ))}
          </ul>
        )}
      </Section>
      <Section title="Discovered on network">
        {discovered.size === 0 ? (
          <div style={hintStyle}>
            {running
              ? "Listening for peers on the local network…"
              : "Start a session to discover peers."}
          </div>
        ) : (
          <ul style={listStyle}>
            {Array.from(discovered.values()).map((entry) => (
              <DiscoveredRow
                key={entry.peerId}
                entry={entry}
                onJoin={() => void handleJoin(entry)}
                disabled={busy || !running}
              />
            ))}
          </ul>
        )}
      </Section>
      {error ? (
        <div role="alert" style={errorStyle}>
          {error}
        </div>
      ) : null}
    </div>
  );
}

interface EventHandlers {
  setPeers: React.Dispatch<React.SetStateAction<SessionPeer[]>>;
  setDiscovered: React.Dispatch<
    React.SetStateAction<Map<string, DiscoveredEntry>>
  >;
  peersRef: React.MutableRefObject<SessionPeer[]>;
  onStatus?: (msg: string | null) => void;
}

async function handleEvent(
  ev: SessionEvent,
  h: EventHandlers,
): Promise<void> {
  switch (ev.kind) {
    case "discovered": {
      h.setDiscovered((prev) => {
        const next = new Map(prev);
        next.set(ev.peerId, {
          peerId: ev.peerId,
          publicKey: ev.publicKey,
          displayName: ev.displayName,
          projectId: ev.projectId,
          socketAddr: ev.socketAddr,
          certFingerprint: ev.certFingerprint,
        });
        return next;
      });
      h.onStatus?.(`Session: discovered ${ev.displayName}.`);
      break;
    }
    case "undiscovered": {
      h.setDiscovered((prev) => {
        if (!prev.has(ev.peerId)) return prev;
        const next = new Map(prev);
        next.delete(ev.peerId);
        return next;
      });
      break;
    }
    case "peerJoined":
    case "peerLeft":
    case "presenceUpdated": {
      // Pull a fresh roster instead of mutating in-place. The
      // bridge is the source of truth (it owns the canonical
      // presence map and the per-peer last-seen timestamp), and
      // a single GET is cheaper than reconstructing the merge
      // state from disjoint event types.
      try {
        const list = await window.kcreate.session.peers();
        h.setPeers(list);
      } catch {
        // Swallow — the next event will retry.
      }
      if (ev.kind === "peerJoined") {
        h.onStatus?.(`Session: ${ev.displayName} joined.`);
      } else if (ev.kind === "peerLeft") {
        const prior = h.peersRef.current.find((p) => p.peerId === ev.peerId);
        h.onStatus?.(
          `Session: ${prior?.displayName ?? "peer"} left.`,
        );
      }
      break;
    }
  }
}

function PeerRow({ peer }: { peer: SessionPeer }): JSX.Element {
  return (
    <li style={rowStyle}>
      <span
        style={{
          ...dotStyle,
          background: peer.presence ? colors.success : colors.textMuted,
        }}
        aria-label={peer.presence ? "online" : "no presence"}
      />
      <div style={{ display: "flex", flexDirection: "column", flex: 1 }}>
        <span style={{ fontWeight: 500 }}>{peer.displayName}</span>
        <code style={codeStyle}>{shortenFingerprint(peer.peerId)}</code>
      </div>
    </li>
  );
}

function DiscoveredRow({
  entry,
  onJoin,
  disabled,
}: {
  entry: DiscoveredEntry;
  onJoin: () => void;
  disabled: boolean;
}): JSX.Element {
  return (
    <li style={rowStyle}>
      <div style={{ display: "flex", flexDirection: "column", flex: 1 }}>
        <span style={{ fontWeight: 500 }}>{entry.displayName}</span>
        <code style={codeStyle}>{entry.socketAddr}</code>
      </div>
      <button
        type="button"
        onClick={onJoin}
        disabled={disabled}
        style={buttonStyle(false)}
      >
        Join
      </button>
    </li>
  );
}

function Section({
  title,
  children,
}: {
  title: string;
  children: React.ReactNode;
}): JSX.Element {
  return (
    <section
      style={{
        display: "flex",
        flexDirection: "column",
        gap: spacing.sm,
        padding: spacing.sm,
        border: `1px solid ${colors.border}`,
        borderRadius: radius.md,
        background: colors.bgSoft,
      }}
    >
      <h3
        style={{
          margin: 0,
          fontSize: 11,
          textTransform: "uppercase",
          letterSpacing: 0.4,
          color: colors.textMuted,
        }}
      >
        {title}
      </h3>
      {children}
    </section>
  );
}

function Field({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}): JSX.Element {
  return (
    <label style={{ display: "flex", flexDirection: "column", gap: 2 }}>
      <span style={{ fontSize: 11, color: colors.textMuted }}>{label}</span>
      {children}
    </label>
  );
}

const inputStyle: React.CSSProperties = {
  padding: "4px 8px",
  fontSize: 12,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.sm,
  background: colors.bg,
  color: colors.text,
};

const codeStyle: React.CSSProperties = {
  fontFamily:
    "ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace",
  fontSize: 11,
  color: colors.text,
  wordBreak: "break-all",
};

const checkboxRowStyle: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: spacing.sm,
  fontSize: 12,
  color: colors.text,
};

function buttonStyle(destructive: boolean): React.CSSProperties {
  return {
    padding: "4px 12px",
    fontSize: 12,
    fontWeight: 500,
    border: "none",
    borderRadius: radius.pill,
    cursor: "pointer",
    background: destructive ? colors.danger : colors.accent,
    color: "#fff",
  };
}

const listStyle: React.CSSProperties = {
  listStyle: "none",
  margin: 0,
  padding: 0,
  display: "flex",
  flexDirection: "column",
  gap: spacing.sm,
};

const rowStyle: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: spacing.sm,
  padding: spacing.sm,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.sm,
  background: colors.bg,
};

const dotStyle: React.CSSProperties = {
  width: 8,
  height: 8,
  borderRadius: "50%",
  flexShrink: 0,
};

const hintStyle: React.CSSProperties = {
  fontSize: 11,
  color: colors.textMuted,
  fontStyle: "italic",
};

const errorStyle: React.CSSProperties = {
  fontSize: 11,
  color: colors.danger,
  padding: spacing.sm,
  border: `1px solid ${colors.danger}`,
  borderRadius: radius.sm,
};

function errMsg(e: unknown): string {
  if (e instanceof Error) return e.message;
  return String(e);
}

/// Shorten a base64url fingerprint to the first 12 + last 4
/// characters with an ellipsis. Long enough to be unique in a
/// LAN of < 100 peers but short enough to fit in a panel column.
function shortenFingerprint(fp: string): string {
  if (fp.length <= 20) return fp;
  return `${fp.slice(0, 12)}…${fp.slice(-4)}`;
}

/// Read the persistent seed from localStorage, or generate a new
/// 32-byte random seed (base64url-no-pad) if none exists. Seed
/// length matches Ed25519's `SECRET_KEY_LENGTH` and feeds
/// `PeerKey::from_seed` on the Rust side.
function loadOrGenerateSeed(): string {
  try {
    const existing = window.localStorage.getItem(SEED_STORAGE_KEY);
    if (existing && existing.length > 0) return existing;
  } catch {
    // Falls through to generate.
  }
  const bytes = new Uint8Array(32);
  window.crypto.getRandomValues(bytes);
  return base64UrlEncode(bytes);
}

function loadOrInitDisplayName(): string {
  try {
    const existing = window.localStorage.getItem(DISPLAY_NAME_STORAGE_KEY);
    if (existing && existing.length > 0) return existing;
  } catch {
    // Same fallthrough.
  }
  return defaultDisplayName();
}

function defaultDisplayName(): string {
  // The renderer doesn't have direct access to the OS username
  // (no `process` in the Electron renderer with sandboxing on),
  // so we fall back to a friendly default. The user is expected
  // to set this once and forget it.
  return "Anonymous Editor";
}

function base64UrlEncode(bytes: Uint8Array): string {
  let bin = "";
  for (const b of bytes) bin += String.fromCharCode(b);
  return window
    .btoa(bin)
    .replace(/\+/g, "-")
    .replace(/\//g, "_")
    .replace(/=+$/, "");
}
