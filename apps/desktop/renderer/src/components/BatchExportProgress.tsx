// BatchExportProgress — Phase 10 Block D Task 22.
//
// Modal overlay that monitors a batch export job. Polls
// `window.kcreate.batch.status` at a steady cadence, surfaces
// per-asset state (pending / running / succeeded / failed),
// estimates remaining time from the running average per-asset
// duration, and exposes a Cancel button wired to
// `window.kcreate.batch.cancel`.
//
// No new Rust bridge work — uses the existing batch surface from
// Phase 2.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import type { BatchExportJob, BatchStatus } from "../../../shared/scene";
import { colors, radius, spacing } from "../styles/tokens";

export interface BatchExportProgressProps {
  job: BatchExportJob;
  /** Called once the modal can be torn down — completion *or* user dismiss. */
  onClose: () => void;
  /** Optional status sink. */
  onStatus?: (msg: string | null) => void;
  /** Poll cadence; defaults to 250 ms. Lower for snappier UI in tests. */
  pollMs?: number;
}

function errMsg(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

function humanMs(ms: number): string {
  if (!Number.isFinite(ms) || ms < 0) return "—";
  if (ms < 1000) return `${ms.toFixed(0)} ms`;
  const s = ms / 1000;
  if (s < 60) return `${s.toFixed(1)} s`;
  const m = Math.floor(s / 60);
  const rem = Math.round(s - m * 60);
  return `${m}m ${rem}s`;
}

type ItemState = "pending" | "running" | "succeeded" | "failed";

interface ItemRow {
  filename: string;
  format: string;
  state: ItemState;
  error?: string;
}

function classifyItems(job: BatchExportJob, st: BatchStatus): ItemRow[] {
  const succeeded = new Set(st.succeeded);
  const failed = new Map(st.failed);
  return job.items.map((it) => {
    if (failed.has(it.filename)) {
      return {
        filename: it.filename,
        format: it.format,
        state: "failed" as const,
        error: failed.get(it.filename),
      };
    }
    if (succeeded.has(it.filename)) {
      return {
        filename: it.filename,
        format: it.format,
        state: "succeeded" as const,
      };
    }
    if (st.currentItem === it.filename && !st.finished) {
      return { filename: it.filename, format: it.format, state: "running" as const };
    }
    return { filename: it.filename, format: it.format, state: "pending" as const };
  });
}

export function BatchExportProgress({
  job,
  onClose,
  onStatus,
  pollMs = 250,
}: BatchExportProgressProps): JSX.Element {
  const [status, setStatus] = useState<BatchStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [cancelling, setCancelling] = useState(false);
  const startedAt = useRef<number>(Date.now());

  const pull = useCallback(async () => {
    try {
      const s = await window.kcreate.batch.status(job.id);
      setStatus(s);
      setError(null);
      return s.finished || s.cancelled;
    } catch (e) {
      setError(errMsg(e));
      onStatus?.(`batch status: ${errMsg(e)}`);
      return true; // stop polling on hard errors
    }
  }, [job.id, onStatus]);

  useEffect(() => {
    let stopped = false;
    const loop = async () => {
      while (!stopped) {
        const done = await pull();
        if (done) return;
        await new Promise((r) => window.setTimeout(r, pollMs));
      }
    };
    void loop();
    return () => {
      stopped = true;
    };
  }, [pull, pollMs]);

  const cancel = useCallback(async () => {
    if (cancelling || !status || status.finished) return;
    setCancelling(true);
    try {
      await window.kcreate.batch.cancel(job.id);
      onStatus?.("batch cancel requested");
    } catch (e) {
      onStatus?.(`batch cancel failed: ${errMsg(e)}`);
    } finally {
      setCancelling(false);
    }
  }, [job.id, status, cancelling, onStatus]);

  const dismiss = useCallback(async () => {
    try {
      // Free the bridge-side bookkeeping; idempotent if unknown.
      await window.kcreate.batch.dismiss(job.id);
    } catch {
      // dismiss is optional / best-effort
    }
    onClose();
  }, [job.id, onClose]);

  const rows = useMemo(
    () => (status ? classifyItems(job, status) : []),
    [job, status],
  );

  const total = status?.total ?? job.items.length;
  const done = status?.completed ?? 0;
  const ratio = total > 0 ? done / total : 0;
  const elapsedMs = status?.durationMs ?? Date.now() - startedAt.current;
  const etaMs = useMemo(() => {
    if (!status || status.finished || done <= 0) return null;
    const avg = elapsedMs / done;
    return avg * (total - done);
  }, [status, done, total, elapsedMs]);

  const summary = useMemo(() => {
    if (!status?.finished) return null;
    const succeeded = status.succeeded.length;
    const failed = status.failed.length;
    return {
      succeeded,
      failed,
      cancelled: status.cancelled,
      elapsedMs: status.durationMs,
    };
  }, [status]);

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label="Batch export progress"
      style={overlayStyle}
      onClick={status?.finished ? dismiss : undefined}
    >
      <div onClick={(e) => e.stopPropagation()} style={cardStyle}>
        <header
          style={{
            display: "flex",
            alignItems: "center",
            justifyContent: "space-between",
            padding: spacing.md,
            borderBottom: `1px solid ${colors.border}`,
          }}
        >
          <h2 style={{ margin: 0, fontSize: 15 }}>
            Batch export {status?.finished ? "complete" : "in progress"}
          </h2>
          <span style={{ fontSize: 12, color: colors.textMuted }}>
            {done}/{total} ({(ratio * 100).toFixed(0)}%)
          </span>
        </header>
        <div style={{ padding: spacing.md }}>
          <Bar value={ratio} />
          <div
            style={{
              display: "flex",
              justifyContent: "space-between",
              marginTop: spacing.xs,
              fontSize: 11,
              color: colors.textMuted,
            }}
          >
            <span>Elapsed {humanMs(elapsedMs)}</span>
            {etaMs !== null && !status?.finished ? (
              <span>ETA {humanMs(etaMs)}</span>
            ) : null}
          </div>
        </div>
        <ul
          style={{
            listStyle: "none",
            margin: 0,
            padding: `0 ${spacing.md}px ${spacing.md}px`,
            maxHeight: 280,
            overflow: "auto",
            display: "flex",
            flexDirection: "column",
            gap: 2,
          }}
        >
          {rows.map((r) => (
            <li
              key={r.filename}
              style={{
                display: "flex",
                gap: spacing.sm,
                alignItems: "center",
                padding: `${spacing.xs}px ${spacing.sm}px`,
                background: colors.bgSoft,
                borderRadius: radius.sm,
              }}
            >
              <StateChip state={r.state} />
              <span style={{ flex: 1, fontSize: 12, fontFamily: "monospace" }}>
                {r.filename}
              </span>
              <span style={{ fontSize: 10, color: colors.textMuted }}>
                {r.format.toUpperCase()}
              </span>
              {r.state === "failed" && r.error ? (
                <span
                  style={{
                    fontSize: 11,
                    color: colors.danger,
                    maxWidth: 240,
                    overflow: "hidden",
                    textOverflow: "ellipsis",
                    whiteSpace: "nowrap",
                  }}
                  title={r.error}
                >
                  {r.error}
                </span>
              ) : null}
            </li>
          ))}
        </ul>
        {error ? (
          <div
            style={{
              margin: spacing.md,
              padding: spacing.sm,
              background: colors.dangerBgSoft,
              color: colors.danger,
              borderRadius: radius.sm,
              fontSize: 12,
            }}
          >
            {error}
          </div>
        ) : null}
        {summary ? (
          <div
            style={{
              margin: spacing.md,
              padding: spacing.sm,
              background: colors.bgSoft,
              borderRadius: radius.sm,
              fontSize: 12,
              display: "flex",
              gap: spacing.md,
              justifyContent: "space-between",
            }}
          >
            <span>
              <strong>{summary.succeeded}</strong> succeeded
            </span>
            <span style={{ color: summary.failed > 0 ? colors.danger : undefined }}>
              <strong>{summary.failed}</strong> failed
            </span>
            {summary.cancelled ? <span>(cancelled)</span> : null}
            <span style={{ color: colors.textMuted }}>
              total {humanMs(summary.elapsedMs)}
            </span>
          </div>
        ) : null}
        <footer
          style={{
            display: "flex",
            justifyContent: "flex-end",
            gap: spacing.sm,
            padding: spacing.md,
            borderTop: `1px solid ${colors.border}`,
          }}
        >
          {!status?.finished ? (
            <button
              type="button"
              onClick={() => void cancel()}
              disabled={cancelling}
              style={{
                ...btnSecondary,
                color: colors.danger,
                borderColor: colors.dangerBorder,
              }}
            >
              {cancelling ? "Cancelling…" : "Cancel"}
            </button>
          ) : null}
          <button
            type="button"
            onClick={() => void dismiss()}
            disabled={!status?.finished}
            style={status?.finished ? btnPrimary : btnSecondary}
          >
            {status?.finished ? "Close" : "Close (waiting…)"}
          </button>
        </footer>
      </div>
    </div>
  );
}

function StateChip({ state }: { state: ItemState }): JSX.Element {
  const map: Record<ItemState, { label: string; color: string; bg: string }> = {
    pending: {
      label: "•",
      color: colors.textMuted,
      bg: "transparent",
    },
    running: {
      label: "▸",
      color: colors.accent,
      bg: colors.accentBgSoft,
    },
    succeeded: {
      label: "✓",
      color: colors.success,
      bg: colors.bgSoft,
    },
    failed: {
      label: "✕",
      color: colors.danger,
      bg: colors.dangerBgSoft,
    },
  };
  const s = map[state];
  return (
    <span
      aria-label={`State: ${state}`}
      style={{
        display: "inline-flex",
        alignItems: "center",
        justifyContent: "center",
        width: 18,
        height: 18,
        borderRadius: radius.sm,
        background: s.bg,
        color: s.color,
        fontSize: 12,
        fontWeight: 700,
      }}
    >
      {s.label}
    </span>
  );
}

function Bar({ value }: { value: number }): JSX.Element {
  const pct = Math.max(0, Math.min(1, value)) * 100;
  return (
    <div
      role="progressbar"
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={pct}
      style={{
        height: 6,
        width: "100%",
        background: colors.bgSoft,
        borderRadius: 999,
        overflow: "hidden",
      }}
    >
      <div
        style={{
          width: `${pct}%`,
          height: "100%",
          background: colors.accent,
          transition: "width 120ms ease-out",
        }}
      />
    </div>
  );
}

const overlayStyle: React.CSSProperties = {
  position: "fixed",
  inset: 0,
  background: "rgba(0,0,0,0.55)",
  display: "flex",
  alignItems: "center",
  justifyContent: "center",
  zIndex: 1000,
};

const cardStyle: React.CSSProperties = {
  width: "min(640px, 95vw)",
  background: colors.bg,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.card,
  boxShadow: "0 24px 48px rgba(0,0,0,0.35)",
  overflow: "hidden",
  display: "flex",
  flexDirection: "column",
};

const btnPrimary: React.CSSProperties = {
  padding: `${spacing.sm}px ${spacing.md}px`,
  background: colors.accent,
  color: colors.textInverse,
  border: "none",
  borderRadius: radius.sm,
  cursor: "pointer",
  fontSize: 13,
};

const btnSecondary: React.CSSProperties = {
  padding: `${spacing.sm}px ${spacing.md}px`,
  background: "transparent",
  color: colors.text,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.sm,
  cursor: "pointer",
  fontSize: 13,
};
