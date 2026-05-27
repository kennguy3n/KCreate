import { useCallback, useEffect, useMemo, useState } from "react";
import {
  buildJoinDeeplink,
  openInviteInKCreate,
  type ShareInvite,
} from "./store";

interface InviteCardProps {
  invite: ShareInvite;
  // Allows tests / Storybook to override the default action.
  onOpen?: (invite: ShareInvite) => Promise<void>;
  // ISO 8601 UTC clock — injected so tests can pin the clock.
  now?: () => Date;
  // Treat invites older than this many minutes as expired. The
  // canonical wire format only carries `issuedAt`; the joiner-side
  // QUIC dial uses the cert fingerprint pinned in the payload,
  // which the host rotates roughly every 60 minutes (see
  // `SessionConfig::key_rotation_interval` in `kcreate_collab`).
  // We mirror that as the default freshness window.
  freshnessWindowMinutes?: number;
  // How often (ms) to re-evaluate the freshness label so a card
  // that crosses the expiry boundary while the panel is open
  // flips to "expired" and disables the Join button without
  // requiring a remount. Defaults to 1000; tests can pin this to
  // `Infinity` (or any non-finite value) to opt out of ticking
  // entirely.
  freshnessTickIntervalMs?: number;
}

type OpenState =
  | { phase: "idle" }
  | { phase: "opening" }
  | { phase: "error"; message: string };

function formatFreshness(
  now: Date,
  issuedAt: string,
  windowMinutes: number,
): { label: string; expired: boolean } {
  const issued = new Date(issuedAt);
  if (Number.isNaN(issued.getTime())) {
    return { label: "issuance unknown", expired: false };
  }
  const elapsedMs = now.getTime() - issued.getTime();
  const remainingMs = windowMinutes * 60_000 - elapsedMs;
  if (remainingMs <= 0) {
    return { label: "expired", expired: true };
  }
  const minutes = Math.floor(remainingMs / 60_000);
  if (minutes < 60) {
    return { label: `${minutes} min remaining`, expired: false };
  }
  const hours = Math.floor(minutes / 60);
  if (hours < 24) {
    return { label: `${hours} h remaining`, expired: false };
  }
  const days = Math.floor(hours / 24);
  return { label: `${days} d remaining`, expired: false };
}

export function InviteCard({
  invite,
  onOpen,
  now,
  freshnessWindowMinutes = 60,
  freshnessTickIntervalMs = 1000,
}: InviteCardProps): JSX.Element {
  const [state, setState] = useState<OpenState>({ phase: "idle" });
  // Capture the clock in state so the `now` prop swap path stays
  // stable across re-renders. Tests that pin the clock pass a new
  // closure on every render but conceptually the clock is the same
  // — we just need a stable read for the interval callback below.
  const clock = now ?? (() => new Date());
  // The current observed time. We re-read the injected clock on
  // every tick so the freshness label tracks the wall clock for
  // long-lived cards (or the pinned test clock if the test rerenders
  // with a new `now` closure).
  const [observedTime, setObservedTime] = useState<Date>(() => clock());
  useEffect(() => {
    // Tests opt out by passing a non-finite interval. Production
    // defaults to 1s ticks which dominates the cost of running a
    // panel-sized React tree by a comfortable margin.
    if (
      !Number.isFinite(freshnessTickIntervalMs) ||
      freshnessTickIntervalMs <= 0
    ) {
      return undefined;
    }
    const timer = setInterval(() => {
      setObservedTime(clock());
    }, freshnessTickIntervalMs);
    return () => {
      clearInterval(timer);
    };
    // `clock` is a per-render closure but we deliberately re-bind
    // the interval only when the tick interval itself changes —
    // a fresh `now` injection from a test re-render still applies
    // the next time the interval fires.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [freshnessTickIntervalMs]);
  const freshness = useMemo(
    () => formatFreshness(observedTime, invite.issuedAt, freshnessWindowMinutes),
    [observedTime, invite.issuedAt, freshnessWindowMinutes],
  );
  const expired = freshness.expired;

  const deeplink = useMemo(() => buildJoinDeeplink(invite), [invite]);

  const handleOpen = useCallback(async () => {
    setState({ phase: "opening" });
    try {
      if (onOpen) {
        await onOpen(invite);
      } else {
        await openInviteInKCreate(invite);
      }
      setState({ phase: "idle" });
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setState({ phase: "error", message });
    }
  }, [invite, onOpen]);

  return (
    <article
      data-testid="kcreate-invite-card"
      data-project-id={invite.projectId}
      style={cardStyle}
    >
      <header style={headerStyle}>
        <span style={badgeStyle}>KCreate</span>
        <span style={titleStyle}>{invite.projectName}</span>
      </header>
      <dl style={listStyle}>
        <dt style={dtStyle}>Shared by</dt>
        <dd style={ddStyle}>{invite.ownerDisplayName}</dd>
        <dt style={dtStyle}>Validity</dt>
        <dd style={ddStyle}>{freshness.label}</dd>
      </dl>
      <div style={actionRowStyle}>
        <button
          type="button"
          onClick={handleOpen}
          disabled={expired || state.phase === "opening"}
          data-testid="kcreate-invite-open"
          style={buttonStyle}
        >
          {state.phase === "opening" ? "Opening…" : "Open in KCreate"}
        </button>
        <a href={deeplink} style={linkStyle} data-testid="kcreate-invite-link">
          {deeplink}
        </a>
      </div>
      {state.phase === "error" && (
        <p role="alert" style={errorStyle} data-testid="kcreate-invite-error">
          {state.message}
        </p>
      )}
    </article>
  );
}

const cardStyle = {
  border: "1px solid #2a2d33",
  borderRadius: 6,
  padding: 10,
  background: "#15171b",
  color: "#e7e8ea",
  fontSize: 13,
} as const;

const headerStyle = {
  display: "flex",
  alignItems: "center",
  gap: 6,
  marginBottom: 6,
} as const;

const badgeStyle = {
  fontSize: 10,
  fontWeight: 700,
  letterSpacing: 0.4,
  padding: "1px 6px",
  borderRadius: 3,
  background: "#3a8ee6",
  color: "#fff",
} as const;

const titleStyle = {
  fontWeight: 600,
} as const;

const listStyle = {
  display: "grid",
  gridTemplateColumns: "auto 1fr",
  columnGap: 8,
  rowGap: 2,
  margin: "6px 0",
} as const;

const dtStyle = {
  opacity: 0.7,
} as const;

const ddStyle = {
  margin: 0,
} as const;

const actionRowStyle = {
  display: "flex",
  alignItems: "center",
  gap: 8,
  marginTop: 4,
} as const;

const buttonStyle = {
  background: "#3a8ee6",
  color: "#fff",
  border: "none",
  borderRadius: 4,
  padding: "5px 10px",
  fontWeight: 600,
  cursor: "pointer",
} as const;

const linkStyle = {
  fontSize: 11,
  color: "#3a8ee6",
  overflow: "hidden",
  textOverflow: "ellipsis",
  whiteSpace: "nowrap",
  maxWidth: 220,
} as const;

const errorStyle = {
  marginTop: 6,
  padding: "4px 6px",
  borderRadius: 4,
  background: "#3a1f23",
  color: "#ff8a8a",
  fontSize: 12,
} as const;
