// Phase 9 Block A Task 4 — KChat companion session-status badge.
//
// Best-effort indicator of whether KCreate is installed (i.e. the
// `kcreate://` protocol handler is registered) and, when reachable,
// whether the local user is in an active collab session.
//
// The extension can't directly query KCreate's process state from
// inside the KChat sandbox. We use the host's deeplink dispatcher
// as a presence probe: dispatching `kcreate://status` either
// resolves successfully (KCreate is registered as a handler — we
// say "installed"), or rejects with `EXTENSION_CAPABILITY_DENIED`
// from the host's deeplink allowlist (we say "not detected").
//
// We deliberately do not navigate the user away as a side-effect;
// the host's allowlist treats `kcreate://status` as a benign URL
// that opens (and immediately closes) a status window. If the
// host rejects the URL it never reaches the OS protocol handler.
import { useCallback, useEffect, useState } from "react";
import { HostProcedureError, openDeeplink } from "./host";

export type SessionStatusKind =
  | "checking"
  | "installed"
  | "not-detected"
  | "error";

interface SessionStatus {
  kind: SessionStatusKind;
  /** Number of connected peers, if known. */
  peerCount?: number;
  /** Detail message for the "error" kind. */
  message?: string;
}

interface SessionStatusBadgeProps {
  /** How often (ms) to re-probe the status. Defaults to 30 s. */
  pollIntervalMs?: number;
  /** Test seam — pin the probe result. */
  probe?: () => Promise<SessionStatus>;
}

const DEFAULT_PROBE_URL = "kcreate://status";

async function defaultProbe(): Promise<SessionStatus> {
  try {
    await openDeeplink(DEFAULT_PROBE_URL);
    // The host accepted the URL; we treat that as "KCreate is the
    // registered handler", which is the best signal available from
    // inside the KChat sandbox.
    return { kind: "installed" };
  } catch (err) {
    if (err instanceof HostProcedureError) {
      if (err.kind === "EXTENSION_CAPABILITY_DENIED") {
        return { kind: "not-detected" };
      }
      return { kind: "error", message: err.message };
    }
    const message = err instanceof Error ? err.message : String(err);
    return { kind: "error", message };
  }
}

export function SessionStatusBadge({
  pollIntervalMs = 30_000,
  probe,
}: SessionStatusBadgeProps): JSX.Element {
  const [status, setStatus] = useState<SessionStatus>({ kind: "checking" });
  const probeFn = probe ?? defaultProbe;

  const runProbe = useCallback(async () => {
    setStatus({ kind: "checking" });
    const next = await probeFn();
    setStatus(next);
  }, [probeFn]);

  useEffect(() => {
    void runProbe();
    if (
      !Number.isFinite(pollIntervalMs) ||
      pollIntervalMs <= 0
    ) {
      return undefined;
    }
    const timer = setInterval(() => {
      void runProbe();
    }, pollIntervalMs);
    return () => {
      clearInterval(timer);
    };
  }, [pollIntervalMs, runProbe]);

  return (
    <div
      data-testid="kcreate-session-badge"
      data-status={status.kind}
      style={containerStyle}
    >
      <span style={dotStyleFor(status.kind)} aria-hidden="true" />
      <span style={textStyle}>{labelFor(status)}</span>
    </div>
  );
}

function labelFor(status: SessionStatus): string {
  switch (status.kind) {
    case "checking":
      return "Checking KCreate…";
    case "installed":
      return status.peerCount !== undefined && status.peerCount > 0
        ? `KCreate online · ${status.peerCount} peer${status.peerCount === 1 ? "" : "s"}`
        : "KCreate installed";
    case "not-detected":
      return "KCreate not detected";
    case "error":
      return status.message ?? "KCreate status unavailable";
  }
}

function dotStyleFor(kind: SessionStatusKind): React.CSSProperties {
  const base: React.CSSProperties = {
    width: 8,
    height: 8,
    borderRadius: 999,
    display: "inline-block",
  };
  switch (kind) {
    case "installed":
      return { ...base, background: "#3fb950" };
    case "not-detected":
      return { ...base, background: "#6e7681" };
    case "error":
      return { ...base, background: "#ff7b72" };
    case "checking":
    default:
      return { ...base, background: "#d29922" };
  }
}

const containerStyle: React.CSSProperties = {
  display: "inline-flex",
  alignItems: "center",
  gap: 6,
  padding: "3px 8px",
  background: "#13161b",
  border: "1px solid #1f2329",
  borderRadius: 999,
  fontSize: 11,
  color: "#e7e8ea",
};

const textStyle: React.CSSProperties = { fontWeight: 500 };
