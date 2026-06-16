// I1 — in-app auto-update affordance.
//
// A compact header control ("Check for updates") plus a modal that
// surfaces the full update lifecycle (check → available → download →
// restart). It is a thin, presentational shell over the main-process
// updater exposed at `window.kcreate.update`:
//
//   * `getState()`        — initial snapshot on mount.
//   * `onStateChange(fn)` — live transitions (checking / progress /
//                           downloaded / error), pushed from main.
//   * `check()` / `download()` / `quitAndInstall()` — user actions.
//
// The updater self-disables on unpackaged dev runs, in which case the
// control renders a calm read-only "managed externally" state rather
// than dangling a dead button. All vector math / rendering stays in
// Rust; this is pure Electron-shell UI.

import { useCallback, useEffect, useRef, useState } from "react";

import type { UpdateState } from "../../../shared/scene";
import { errorMessage } from "../lib/errorMessage";
import { colors, radius, shadow, spacing } from "../styles/tokens";
import { Icon, type IconName } from "./Icon";

/** Human-readable byte rate, e.g. `1.4 MB/s`. */
function formatRate(bytesPerSecond: number): string {
  if (!Number.isFinite(bytesPerSecond) || bytesPerSecond <= 0) return "—";
  const units = ["B", "KB", "MB", "GB"];
  let value = bytesPerSecond;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(value >= 10 || unit === 0 ? 0 : 1)} ${units[unit]}/s`;
}

/** Human-readable byte count, e.g. `42.0 MB`. */
function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(unit === 0 ? 0 : 1)} ${units[unit]}`;
}

/** Glyph + short label for the current updater status. */
function statusGlyph(state: UpdateState): { icon: IconName; label: string } {
  switch (state.status) {
    case "checking":
      return { icon: "clock", label: "Checking…" };
    case "available":
      return { icon: "sparkles", label: "Update available" };
    case "downloading":
      return { icon: "download", label: "Downloading…" };
    case "downloaded":
      return { icon: "redo", label: "Restart to update" };
    case "error":
      return { icon: "x", label: "Update error" };
    case "not-available":
    case "idle":
    case "disabled":
    default:
      return { icon: "download", label: "Check for updates" };
  }
}

/** Whether the control should draw an attention dot on the header pill. */
function hasAttention(state: UpdateState): boolean {
  return state.status === "available" || state.status === "downloaded";
}

/**
 * Header affordance + modal. Self-contained: holds its own `open`
 * state and updater subscription so a host only needs to drop
 * `<UpdateControl />` into a toolbar.
 */
export function UpdateControl(): JSX.Element {
  const [state, setState] = useState<UpdateState | null>(null);
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  // Guards against a state update after unmount when an in-flight
  // `check()` / `download()` resolves late.
  const mounted = useRef(true);

  useEffect(() => {
    mounted.current = true;
    let unsubscribe: (() => void) | null = null;
    window.kcreate.update
      .getState()
      .then((s) => {
        if (mounted.current) setState(s);
      })
      .catch(() => {
        // A failed initial read shouldn't crash the home screen;
        // leave `state` null so the control renders nothing.
      });
    unsubscribe = window.kcreate.update.onStateChange((s) => {
      if (mounted.current) setState(s);
    });
    return () => {
      mounted.current = false;
      if (unsubscribe) unsubscribe();
    };
  }, []);

  const runAction = useCallback(
    async (action: () => Promise<UpdateState | void>) => {
      setBusy(true);
      setActionError(null);
      try {
        const next = await action();
        if (next && mounted.current) setState(next);
      } catch (e) {
        if (mounted.current) setActionError(errorMessage(e));
      } finally {
        if (mounted.current) setBusy(false);
      }
    },
    [],
  );

  const handleCheck = useCallback(() => {
    void runAction(() => window.kcreate.update.check());
  }, [runAction]);

  const handleDownload = useCallback(() => {
    void runAction(() => window.kcreate.update.download());
  }, [runAction]);

  const handleInstall = useCallback(() => {
    // `quitAndInstall` tears the app down, so there is no resolved
    // state to fold back in; surface only a failure (e.g. nothing
    // staged) if it rejects before quitting.
    void runAction(() => window.kcreate.update.quitAndInstall());
  }, [runAction]);

  // Close on Escape while the modal is open.
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === "Escape") setOpen(false);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open]);

  if (!state) return <></>;

  const { icon, label } = statusGlyph(state);

  return (
    <>
      <button
        type="button"
        onClick={() => setOpen(true)}
        style={pillStyle}
        aria-label={label}
        title={label}
        data-testid="kcreate-update-button"
      >
        <span style={pillIconWrap}>
          <Icon name={icon} size={14} />
          {hasAttention(state) ? <span style={attentionDotStyle} /> : null}
        </span>
        <span style={pillLabelStyle}>{label}</span>
      </button>

      {open ? (
        <UpdateModal
          state={state}
          busy={busy}
          actionError={actionError}
          onClose={() => setOpen(false)}
          onCheck={handleCheck}
          onDownload={handleDownload}
          onInstall={handleInstall}
        />
      ) : null}
    </>
  );
}

interface UpdateModalProps {
  state: UpdateState;
  busy: boolean;
  actionError: string | null;
  onClose: () => void;
  onCheck: () => void;
  onDownload: () => void;
  onInstall: () => void;
}

function UpdateModal({
  state,
  busy,
  actionError,
  onClose,
  onCheck,
  onDownload,
  onInstall,
}: UpdateModalProps): JSX.Element {
  return (
    <div
      style={overlayStyle}
      role="dialog"
      aria-modal="true"
      aria-labelledby="kcreate-update-title"
      data-testid="kcreate-update-modal"
      onClick={onClose}
    >
      <div style={dialogStyle} onClick={(e) => e.stopPropagation()}>
        <header style={headerStyle}>
          <h2 id="kcreate-update-title" style={titleStyle}>
            Software updates
          </h2>
          <button
            type="button"
            onClick={onClose}
            style={iconButtonStyle}
            aria-label="Close updates"
            data-testid="kcreate-update-close"
          >
            <Icon name="x" size={18} />
          </button>
        </header>

        <div style={versionRowStyle}>
          <span style={mutedStyle}>Current version</span>
          <span
            style={versionValueStyle}
            data-testid="kcreate-update-current-version"
          >
            v{state.currentVersion}
          </span>
        </div>

        <UpdateBody state={state} />

        {actionError ? (
          <p style={errorStyle} data-testid="kcreate-update-error">
            {actionError}
          </p>
        ) : null}

        <footer style={footerStyle}>
          <UpdateActions
            state={state}
            busy={busy}
            onCheck={onCheck}
            onDownload={onDownload}
            onInstall={onInstall}
            onClose={onClose}
          />
        </footer>
      </div>
    </div>
  );
}

function UpdateBody({ state }: { state: UpdateState }): JSX.Element {
  if (!state.supported) {
    return (
      <div style={noticeStyle} data-testid="kcreate-update-status">
        <p style={{ margin: 0, fontWeight: 600 }}>
          Updates are managed outside the app
        </p>
        <p style={{ margin: 0 }}>
          This build doesn&apos;t self-update (you&apos;re running from
          source, or auto-update was disabled). Grab the latest signed
          release from the project&apos;s downloads page.
        </p>
      </div>
    );
  }

  switch (state.status) {
    case "checking":
      return (
        <p style={statusLineStyle} data-testid="kcreate-update-status">
          <Icon name="clock" size={16} />
          Checking for updates…
        </p>
      );
    case "available":
      return (
        <div style={infoCardStyle} data-testid="kcreate-update-status">
          <div style={infoCardHeaderStyle}>
            <Icon name="sparkles" size={16} />
            <span style={{ fontWeight: 600 }}>
              Version {state.info?.version ?? "?"} is available
            </span>
          </div>
          {state.info?.releaseDate ? (
            <span style={mutedSmallStyle}>
              Released {formatReleaseDate(state.info.releaseDate)}
            </span>
          ) : null}
          {state.info?.releaseNotes ? (
            <pre style={notesStyle}>{state.info.releaseNotes}</pre>
          ) : null}
        </div>
      );
    case "downloading": {
      const percent = Math.max(0, Math.min(100, state.progress?.percent ?? 0));
      return (
        <div style={progressContainerStyle} data-testid="kcreate-update-status">
          <div style={progressLabelStyle}>
            <span>Downloading v{state.info?.version ?? ""}</span>
            <span>{percent.toFixed(0)}%</span>
          </div>
          <div
            style={progressTrackStyle}
            role="progressbar"
            aria-valuenow={Math.round(percent)}
            aria-valuemin={0}
            aria-valuemax={100}
          >
            <div style={{ ...progressFillStyle, width: `${percent}%` }} />
          </div>
          {state.progress ? (
            <span style={progressDetailStyle}>
              {formatBytes(state.progress.transferred)} of{" "}
              {formatBytes(state.progress.total)} ·{" "}
              {formatRate(state.progress.bytesPerSecond)}
            </span>
          ) : null}
        </div>
      );
    }
    case "downloaded":
      return (
        <div style={infoCardStyle} data-testid="kcreate-update-status">
          <div style={infoCardHeaderStyle}>
            <Icon name="package" size={16} />
            <span style={{ fontWeight: 600 }}>
              Version {state.info?.version ?? ""} is ready to install
            </span>
          </div>
          <span style={mutedSmallStyle}>
            Restart KCreate to finish updating. Your work is saved
            locally and will reopen.
          </span>
        </div>
      );
    case "error":
      return (
        <p style={errorStyle} data-testid="kcreate-update-status">
          {state.error ?? "The last update check failed."}
        </p>
      );
    case "not-available":
      return (
        <p style={statusLineStyle} data-testid="kcreate-update-status">
          <span style={upToDateDotStyle} />
          You&apos;re on the latest version.
        </p>
      );
    case "idle":
    default:
      return (
        <p style={mutedStyle} data-testid="kcreate-update-status">
          Check whether a newer signed release is available. Downloads
          are verified before they&apos;re applied.
        </p>
      );
  }
}

function UpdateActions({
  state,
  busy,
  onCheck,
  onDownload,
  onInstall,
  onClose,
}: {
  state: UpdateState;
  busy: boolean;
  onCheck: () => void;
  onDownload: () => void;
  onInstall: () => void;
  onClose: () => void;
}): JSX.Element {
  if (!state.supported) {
    return (
      <button type="button" style={secondaryButtonStyle} onClick={onClose}>
        Close
      </button>
    );
  }

  const checking = state.status === "checking" || busy;

  return (
    <>
      <button type="button" style={secondaryButtonStyle} onClick={onClose}>
        Close
      </button>
      {state.status === "available" ? (
        <button
          type="button"
          style={busy ? primaryButtonDisabledStyle : primaryButtonStyle}
          onClick={onDownload}
          disabled={busy}
          data-testid="kcreate-update-download"
        >
          <Icon name="download" size={14} />
          Download update
        </button>
      ) : state.status === "downloaded" ? (
        <button
          type="button"
          style={primaryButtonStyle}
          onClick={onInstall}
          data-testid="kcreate-update-install"
        >
          <Icon name="redo" size={14} />
          Restart &amp; install
        </button>
      ) : state.status === "downloading" ? (
        <button type="button" style={primaryButtonDisabledStyle} disabled>
          <Icon name="download" size={14} />
          Downloading…
        </button>
      ) : (
        <button
          type="button"
          style={checking ? primaryButtonDisabledStyle : primaryButtonStyle}
          onClick={onCheck}
          disabled={checking}
          data-testid="kcreate-update-check"
        >
          <Icon name="download" size={14} />
          {checking ? "Checking…" : "Check for updates"}
        </button>
      )}
    </>
  );
}

/** Format an ISO release date as a short local date; passthrough on parse failure. */
function formatReleaseDate(iso: string): string {
  const ms = Date.parse(iso);
  if (Number.isNaN(ms)) return iso;
  return new Date(ms).toLocaleDateString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
  });
}

// -----------------------------------------------------------------------------
// Styles
// -----------------------------------------------------------------------------

const pillStyle: React.CSSProperties = {
  display: "inline-flex",
  alignItems: "center",
  gap: spacing.xs,
  background: colors.bgSoft,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.pill,
  padding: "6px 12px",
  fontSize: 13,
  fontWeight: 500,
  color: colors.text,
  cursor: "pointer",
};

const pillIconWrap: React.CSSProperties = {
  position: "relative",
  display: "inline-flex",
  alignItems: "center",
};

const pillLabelStyle: React.CSSProperties = {
  whiteSpace: "nowrap",
};

const attentionDotStyle: React.CSSProperties = {
  position: "absolute",
  top: -3,
  right: -3,
  width: 7,
  height: 7,
  borderRadius: radius.pill,
  background: colors.accent,
  border: `1px solid ${colors.bg}`,
};

const overlayStyle: React.CSSProperties = {
  position: "fixed",
  inset: 0,
  background: "rgba(0,0,0,0.45)",
  zIndex: 240,
  display: "flex",
  alignItems: "center",
  justifyContent: "center",
};

const dialogStyle: React.CSSProperties = {
  width: 460,
  maxWidth: "90vw",
  maxHeight: "85vh",
  overflowY: "auto",
  background: colors.bg,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.card,
  boxShadow: shadow.cardHover,
  padding: spacing.lg,
  color: colors.text,
  display: "flex",
  flexDirection: "column",
  gap: spacing.md,
};

const headerStyle: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  justifyContent: "space-between",
};

const titleStyle: React.CSSProperties = {
  margin: 0,
  fontSize: 18,
  fontWeight: 600,
};

const iconButtonStyle: React.CSSProperties = {
  background: "transparent",
  border: "none",
  color: colors.textMuted,
  cursor: "pointer",
  padding: 0,
  lineHeight: 1,
  display: "inline-flex",
};

const versionRowStyle: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  justifyContent: "space-between",
  fontSize: 13,
};

const versionValueStyle: React.CSSProperties = {
  fontWeight: 600,
  fontVariantNumeric: "tabular-nums",
};

const mutedStyle: React.CSSProperties = {
  margin: 0,
  fontSize: 13,
  color: colors.textMuted,
  lineHeight: 1.45,
};

const mutedSmallStyle: React.CSSProperties = {
  fontSize: 12,
  color: colors.textMuted,
};

const statusLineStyle: React.CSSProperties = {
  margin: 0,
  display: "flex",
  alignItems: "center",
  gap: spacing.sm,
  fontSize: 13,
  color: colors.text,
};

const upToDateDotStyle: React.CSSProperties = {
  width: 8,
  height: 8,
  borderRadius: radius.pill,
  background: colors.success,
  flexShrink: 0,
};

const infoCardStyle: React.CSSProperties = {
  display: "flex",
  flexDirection: "column",
  gap: spacing.xs,
  background: colors.bgSoft,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.card,
  padding: spacing.md,
};

const infoCardHeaderStyle: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: spacing.sm,
  fontSize: 14,
};

const notesStyle: React.CSSProperties = {
  margin: 0,
  fontSize: 12,
  color: colors.textMuted,
  lineHeight: 1.5,
  whiteSpace: "pre-wrap",
  fontFamily: "inherit",
  maxHeight: 160,
  overflowY: "auto",
};

const noticeStyle: React.CSSProperties = {
  display: "flex",
  flexDirection: "column",
  gap: spacing.xs,
  background: colors.bgSoft,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.card,
  padding: spacing.md,
  fontSize: 13,
  color: colors.textMuted,
  lineHeight: 1.45,
};

const errorStyle: React.CSSProperties = {
  margin: 0,
  fontSize: 13,
  color: colors.danger,
  background: colors.dangerBgSoft,
  border: `1px solid ${colors.dangerBorder}`,
  borderRadius: radius.sm,
  padding: spacing.sm,
};

const footerStyle: React.CSSProperties = {
  display: "flex",
  justifyContent: "flex-end",
  gap: spacing.sm,
  flexWrap: "wrap",
};

const primaryButtonStyle: React.CSSProperties = {
  display: "inline-flex",
  alignItems: "center",
  gap: spacing.xs,
  background: colors.accent,
  color: "white",
  border: "none",
  borderRadius: radius.sm,
  padding: "8px 14px",
  fontWeight: 600,
  cursor: "pointer",
};

const primaryButtonDisabledStyle: React.CSSProperties = {
  ...primaryButtonStyle,
  opacity: 0.6,
  cursor: "default",
};

const secondaryButtonStyle: React.CSSProperties = {
  background: "transparent",
  color: colors.text,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.sm,
  padding: "8px 14px",
  cursor: "pointer",
};

const progressContainerStyle: React.CSSProperties = {
  display: "flex",
  flexDirection: "column",
  gap: spacing.xs,
};

const progressLabelStyle: React.CSSProperties = {
  display: "flex",
  justifyContent: "space-between",
  fontSize: 12,
  color: colors.textMuted,
  fontVariantNumeric: "tabular-nums",
};

const progressTrackStyle: React.CSSProperties = {
  height: 6,
  background: colors.bgSoft,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.pill,
  overflow: "hidden",
};

const progressFillStyle: React.CSSProperties = {
  height: "100%",
  background: colors.accent,
  transition: "width 120ms linear",
};

const progressDetailStyle: React.CSSProperties = {
  fontSize: 11,
  color: colors.textMuted,
  fontVariantNumeric: "tabular-nums",
};
