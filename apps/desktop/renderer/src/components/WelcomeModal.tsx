// Phase C — first-run welcome modal.
//
// Auto-mounted by `HomePage` on first launch (preferences
// `onboarding.completed === false`). Drives a tier-aware one-click
// install of the recommended Bonsai pack so a brand-new user can
// get a working local LLM without having to find the right pack in
// Model Manager, copy a Hugging Face URL, download a multi-GB GGUF,
// and point the manual installer at the file.
//
// Three CTAs:
//   1. **Install recommended pack** — the headline path. Calls
//      `onboarding.installRecommendedPack()`; the main process
//      streams the bytes into a temp file (allow-list-validated
//      URL + SSRF defence — see `onboardingDownloader.ts`), then
//      hands the temp path to the same `aiModel.installModelPack`
//      pipeline the manual flow uses. Progress events update a
//      bar in-modal so the user has continuous feedback during
//      the multi-minute download.
//   2. **I already have the weights file** — fallback for users
//      who pre-downloaded the GGUF (mirrors / corporate caches /
//      slow networks). Reuses the existing `pickModelFile` +
//      `installModelPack` flow ModelManager uses.
//   3. **Skip for now** — closes the modal without installing.
//      Every close path persists `onboarding.completed = true`
//      so the modal is never shown again on subsequent launches.
//
// The modal NEVER renders when `open === false`; the parent owns
// the gate. This keeps `HomePage`'s preferences-load lifecycle in
// one place and lets vitest mount the modal directly with
// `open` forced on for behaviour tests without going through the
// preferences pipeline.

import { useCallback, useEffect, useRef, useState } from "react";

import type {
  ModelPack,
  OnboardingInstallReport,
  OnboardingProgress,
  Preferences,
  ResourceLimits,
} from "../../../shared/scene";
import { errorMessage } from "../lib/errorMessage";
import { colors, radius, spacing } from "../styles/tokens";

export interface WelcomeModalProps {
  /** Controlled visibility — parent decides when to mount this. */
  open: boolean;
  /** Fired on every close path (install done, skip, manual install,
   *  download cancelled). The parent persists `onboarding.completed = true`
   *  and the pack id (if known) so a future tier-change pass can
   *  detect when the recommended pack rolled over. */
  onDismiss: (installedPackId: string | null) => void;
}

/**
 * Discriminated union of welcome-modal lifecycle states. The pack
 * is resolved lazily on mount; the install button kicks the state
 * machine into `installing` (with live progress events from the
 * main process); success rolls into `done` so the modal can
 * surface a "Ready!" message before the parent dismisses.
 */
type Phase =
  | { kind: "loading" }
  | { kind: "loaded"; pack: ModelPack; tier: string; installedAlready: boolean }
  | {
      kind: "installing";
      pack: ModelPack;
      tier: string;
      progress: OnboardingProgress | null;
    }
  | {
      kind: "done";
      pack: ModelPack;
      tier: string;
      report: OnboardingInstallReport;
    }
  | { kind: "error"; tier: string | null; pack: ModelPack | null; message: string };

/**
 * Lazy bootstrap: resolve the recommended pack id via the bridge,
 * cross-reference it against the full model pack list to pull
 * display name + size, and surface the device tier. Returns
 * either a `loaded` phase (success) or an `error` phase (bridge
 * call failed / pack id missing from registry).
 */
async function resolveRecommendedPack(): Promise<Phase> {
  try {
    const [packId, limits, packs] = await Promise.all([
      window.kcreate.llm.recommendedPack(),
      window.kcreate.runtime.resourceLimits(),
      window.kcreate.aiModel.listModelPacks(),
    ]);
    if (!packId) {
      return {
        kind: "error",
        tier: tierLabel(limits),
        pack: null,
        message:
          "Your device tier does not have a recommended local LLM pack yet. You can still install a pack manually from Model Manager.",
      };
    }
    const pack = packs.find((p) => p.id === packId) ?? null;
    if (!pack) {
      return {
        kind: "error",
        tier: tierLabel(limits),
        pack: null,
        message: `Recommended pack '${packId}' is not in the model registry. Open Model Manager to install a pack manually.`,
      };
    }
    return {
      kind: "loaded",
      pack,
      tier: tierLabel(limits),
      installedAlready: pack.installed,
    };
  } catch (e) {
    return {
      kind: "error",
      tier: null,
      pack: null,
      message: errorMessage(e),
    };
  }
}

function tierLabel(limits: ResourceLimits | null): string {
  if (!limits || !limits.deviceTier) return "unknown";
  return limits.deviceTier;
}

export function WelcomeModal({
  open,
  onDismiss,
}: WelcomeModalProps): JSX.Element | null {
  const [phase, setPhase] = useState<Phase>({ kind: "loading" });

  // Synchronous reentry guard against the small window where the
  // user double-clicks "Install recommended pack" fast enough that
  // React has not yet committed the `installing` state into the
  // DOM. Mirrors the `inFlightPackId` pattern in `ModelManager.tsx`
  // (the underlying installer races on a `.tmp` file).
  const installInFlight = useRef(false);

  // Latest `onDismiss` callback so the cleanup effect can read it
  // without re-running on every parent re-render.
  const onDismissRef = useRef(onDismiss);
  useEffect(() => {
    onDismissRef.current = onDismiss;
  }, [onDismiss]);

  // Resolve recommended pack once per mount. The parent gates the
  // modal on `open`, so a remount only happens after a full close
  // cycle — re-fetching is fine.
  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    setPhase({ kind: "loading" });
    void (async () => {
      const next = await resolveRecommendedPack();
      if (!cancelled) setPhase(next);
    })();
    return () => {
      cancelled = true;
    };
  }, [open]);

  // Subscribe to install-progress events while the modal is open;
  // the unsubscribe handle returned from `onInstallProgress` is
  // load-bearing — without it, every modal remount would leak a
  // listener on the IPC channel and eventually emit duplicate
  // progress updates per event.
  useEffect(() => {
    if (!open) return;
    const off = window.kcreate.onboarding.onInstallProgress((progress) => {
      setPhase((prev) => {
        if (prev.kind !== "installing") return prev;
        return { ...prev, progress };
      });
    });
    return off;
  }, [open]);

  // When the modal closes, cancel any in-flight download so a user
  // who hits "Skip" doesn't accidentally complete a multi-GB
  // download in the background. The main process's
  // `cancelInstall` is idempotent.
  useEffect(() => {
    if (open) return;
    void window.kcreate.onboarding.cancelInstall();
  }, [open]);

  const handleInstall = useCallback(async () => {
    // Re-entrancy guard. Mirrors `handlePickFile` exactly: set the
    // ref synchronously BEFORE any await so a second click during
    // the same event-loop microtask sees the in-flight flag. The
    // previous iteration set the ref inside the `setPhase` updater
    // (which runs during React's commit phase, AFTER setPhase
    // returns) — fragile, even if React 18 batching made it
    // unobservable in practice. Bot-flagged ANALYSIS_0001.
    if (installInFlight.current) return;
    if (phase.kind !== "loaded") return;
    const pack = phase.pack;
    const tier = phase.tier;
    installInFlight.current = true;
    setPhase({
      kind: "installing",
      pack,
      tier,
      progress: null,
    });
    try {
      const report = await window.kcreate.onboarding.installRecommendedPack();
      setPhase((prev) => {
        if (prev.kind !== "installing") return prev;
        return {
          kind: "done",
          pack: prev.pack,
          tier: prev.tier,
          report,
        };
      });
    } catch (e) {
      // Guard on `prev.kind === "installing"` so a concurrent
      // cancel (which already moved us back to "loaded") wins
      // over the cancelled-promise rejection. Without this guard
      // the catch unconditionally builds an `"error"` phase, the
      // `PackCard` disappears (tier/pack become null), and
      // clicking "Close" persists `onboarding.completed = true`,
      // making the modal unreachable for users who pressed
      // Cancel. Bot-flagged BUG_0001.
      setPhase((prev) => {
        if (prev.kind !== "installing") return prev;
        return {
          kind: "error",
          tier: prev.tier,
          pack: prev.pack,
          message: errorMessage(e),
        };
      });
    } finally {
      installInFlight.current = false;
    }
  }, [phase]);

  const handlePickFile = useCallback(async () => {
    if (installInFlight.current) return;
    if (phase.kind !== "loaded") return;
    const pack = phase.pack;
    installInFlight.current = true;
    try {
      const source = await window.kcreate.aiModel.pickModelFile();
      if (!source) {
        // User cancelled the file picker. Stay on the loaded
        // phase so they can try the headline button instead.
        return;
      }
      setPhase({
        kind: "installing",
        pack,
        tier: phase.tier,
        progress: {
          packId: pack.id,
          phase: "verifying",
          receivedBytes: 0,
          totalBytes: null,
          message: "",
        },
      });
      const report = await window.kcreate.aiModel.installModelPack(
        pack.id,
        source,
      );
      setPhase({
        kind: "done",
        pack,
        tier: phase.tier,
        // `aiModel.installModelPack` returns `ModelInstallReport`
        // (the Rust `InstallReport` serialisation), which now
        // shares the same camelCase shape as the
        // `OnboardingInstallReport` carried by the one-click
        // install path — no field-name translation needed.
        report,
      });
    } catch (e) {
      setPhase({
        kind: "error",
        tier: phase.tier,
        pack,
        message: errorMessage(e),
      });
    } finally {
      installInFlight.current = false;
    }
  }, [phase]);

  const handleSkip = useCallback(() => {
    const pack = packFromPhase(phase);
    onDismissRef.current(pack?.id ?? null);
  }, [phase]);

  const handleDone = useCallback(() => {
    if (phase.kind !== "done") return;
    onDismissRef.current(phase.pack.id);
  }, [phase]);

  const handleCancelInstall = useCallback(() => {
    void window.kcreate.onboarding.cancelInstall();
    setPhase((prev) => {
      if (prev.kind !== "installing") return prev;
      return {
        kind: "loaded",
        pack: prev.pack,
        tier: prev.tier,
        installedAlready: false,
      };
    });
  }, []);

  if (!open) return null;

  return (
    <div
      style={overlayStyle}
      role="dialog"
      aria-modal="true"
      aria-labelledby="kcreate-welcome-title"
      data-testid="kcreate-welcome-modal"
    >
      <div style={dialogStyle}>
        <header style={headerStyle}>
          <h2 id="kcreate-welcome-title" style={titleStyle}>
            Welcome to KCreate
          </h2>
          <button
            type="button"
            onClick={handleSkip}
            style={iconButtonStyle}
            aria-label="Close welcome"
            data-testid="kcreate-welcome-close"
          >
            ×
          </button>
        </header>

        <p style={leadStyle}>
          KCreate runs entirely on your device. Install a local AI
          model now to enable design suggestions, layer naming, and
          smart commands — or skip for now and pick one from the
          Model Manager later.
        </p>

        <WelcomeBody
          phase={phase}
          onInstall={() => void handleInstall()}
          onPickFile={() => void handlePickFile()}
          onCancelInstall={handleCancelInstall}
          onDone={handleDone}
          onSkip={handleSkip}
        />
      </div>
    </div>
  );
}

interface WelcomeBodyProps {
  phase: Phase;
  onInstall: () => void;
  onPickFile: () => void;
  onCancelInstall: () => void;
  onDone: () => void;
  onSkip: () => void;
}

function WelcomeBody({
  phase,
  onInstall,
  onPickFile,
  onCancelInstall,
  onDone,
  onSkip,
}: WelcomeBodyProps): JSX.Element {
  switch (phase.kind) {
    case "loading":
      return (
        <p style={mutedStyle} data-testid="kcreate-welcome-loading">
          Detecting your device…
        </p>
      );
    case "loaded": {
      const { pack, tier, installedAlready } = phase;
      return (
        <>
          <PackCard pack={pack} tier={tier} />
          {installedAlready ? (
            <p
              style={successInlineStyle}
              data-testid="kcreate-welcome-already-installed"
            >
              You already have this pack installed. You’re good to go.
            </p>
          ) : null}
          <footer style={footerStyle}>
            <button
              type="button"
              onClick={onSkip}
              style={secondaryButtonStyle}
              data-testid="kcreate-welcome-skip"
            >
              Skip for now
            </button>
            <button
              type="button"
              onClick={onPickFile}
              style={secondaryButtonStyle}
              data-testid="kcreate-welcome-pick-file"
              disabled={installedAlready}
              aria-disabled={installedAlready}
            >
              I already have the file…
            </button>
            <button
              type="button"
              onClick={onInstall}
              style={primaryButtonStyle}
              data-testid="kcreate-welcome-install"
              disabled={installedAlready}
              aria-disabled={installedAlready}
            >
              Install recommended pack
            </button>
          </footer>
        </>
      );
    }
    case "installing": {
      const { pack, tier, progress } = phase;
      const phaseLabel = progress ? phaseToLabel(progress.phase) : "Starting…";
      const pct =
        progress &&
        progress.totalBytes &&
        progress.totalBytes > 0 &&
        progress.phase === "downloading"
          ? Math.min(
              100,
              Math.round((progress.receivedBytes / progress.totalBytes) * 100),
            )
          : null;
      return (
        <>
          <PackCard pack={pack} tier={tier} />
          <div
            style={progressContainerStyle}
            role="status"
            aria-live="polite"
            data-testid="kcreate-welcome-progress"
          >
            <div style={progressLabelStyle}>
              <span>{phaseLabel}</span>
              {pct !== null ? <span>{pct}%</span> : null}
            </div>
            <div style={progressTrackStyle}>
              <div
                style={{
                  ...progressFillStyle,
                  width: pct !== null ? `${pct}%` : "30%",
                  // Indeterminate phases (resolving / verifying / installing)
                  // render a half-width bar so the user sees motion even
                  // before content-length is known.
                  opacity: pct !== null ? 1 : 0.6,
                }}
              />
            </div>
            {progress &&
            progress.totalBytes !== null &&
            progress.totalBytes > 0 ? (
              <div style={progressDetailStyle}>
                {formatBytes(progress.receivedBytes)} of{" "}
                {formatBytes(progress.totalBytes)}
              </div>
            ) : null}
          </div>
          <footer style={footerStyle}>
            <button
              type="button"
              onClick={onCancelInstall}
              style={secondaryButtonStyle}
              data-testid="kcreate-welcome-cancel"
            >
              Cancel
            </button>
          </footer>
        </>
      );
    }
    case "done": {
      const { pack, tier, report } = phase;
      return (
        <>
          <PackCard pack={pack} tier={tier} />
          <p
            style={successBlockStyle}
            role="status"
            data-testid="kcreate-welcome-done"
          >
            <strong>{pack.name}</strong> is ready.{" "}
            {report.verified
              ? `Verified ${formatBytes(report.sizeBytes)}.`
              : `Installed ${formatBytes(
                  report.sizeBytes,
                )} (no pinned SHA-256 in the registry; actual hash ${report.actualSha256.slice(
                  0,
                  12,
                )}…).`}
          </p>
          <footer style={footerStyle}>
            <button
              type="button"
              onClick={onDone}
              style={primaryButtonStyle}
              data-testid="kcreate-welcome-finish"
            >
              Get started
            </button>
          </footer>
        </>
      );
    }
    case "error":
      return (
        <>
          {phase.pack ? <PackCard pack={phase.pack} tier={phase.tier ?? "unknown"} /> : null}
          <p
            role="alert"
            style={errorStyle}
            data-testid="kcreate-welcome-error"
          >
            {phase.message}
          </p>
          <footer style={footerStyle}>
            <button
              type="button"
              onClick={onSkip}
              style={primaryButtonStyle}
              data-testid="kcreate-welcome-error-dismiss"
            >
              Close
            </button>
          </footer>
        </>
      );
  }
}

interface PackCardProps {
  pack: ModelPack;
  tier: string;
}

function PackCard({ pack, tier }: PackCardProps): JSX.Element {
  return (
    <section style={packCardStyle} aria-label="Recommended pack">
      <header style={packHeaderStyle}>
        <span style={packTierStyle}>Tier {tier}</span>
        <span style={packSizeStyle}>{formatBytes(pack.sizeBytes)}</span>
      </header>
      <h3 style={packTitleStyle} data-testid="kcreate-welcome-pack-name">
        {pack.name}
      </h3>
      <p style={packDescStyle}>
        Quantised GGUF, runs on your device via llama.cpp. No data
        leaves your machine.
      </p>
    </section>
  );
}

function phaseToLabel(p: OnboardingProgress["phase"]): string {
  switch (p) {
    case "resolving":
      return "Resolving recommendation…";
    case "connecting":
      return "Connecting…";
    case "downloading":
      return "Downloading…";
    case "verifying":
      return "Verifying…";
    case "installing":
      return "Installing…";
    case "done":
      return "Done";
    case "cancelled":
      return "Cancelled";
    case "error":
      return "Error";
  }
}

function formatBytes(bytes: number): string {
  if (bytes >= 1_000_000_000)
    return `${(bytes / 1_000_000_000).toFixed(1)} GB`;
  if (bytes >= 1_000_000) return `${(bytes / 1_000_000).toFixed(0)} MB`;
  if (bytes >= 1_000) return `${(bytes / 1_000).toFixed(0)} KB`;
  return `${bytes} B`;
}

function packFromPhase(phase: Phase): ModelPack | null {
  switch (phase.kind) {
    case "loaded":
    case "installing":
    case "done":
      return phase.pack;
    case "error":
      return phase.pack;
    case "loading":
      return null;
  }
}

/**
 * Convenience predicate exported for `HomePage` so the
 * "show welcome modal?" decision lives in one place. Returns
 * `true` when the user has never gone through the welcome flow.
 * The defensive type guard handles partially-corrupt preferences
 * files where the `onboarding` section is present but malformed.
 */
export function shouldShowWelcomeModal(prefs: Preferences | null): boolean {
  if (!prefs) return false;
  // The Preferences type guarantees the field exists, but a
  // preferences file that pre-dates Phase C will have the field
  // defaulted to `{ completed: false }` by serde. Either way,
  // `!completed` is the correct gate.
  return !prefs.onboarding.completed;
}

// -----------------------------------------------------------------------------
// Styles
// -----------------------------------------------------------------------------

const overlayStyle: React.CSSProperties = {
  position: "fixed",
  inset: 0,
  background: "rgba(0,0,0,0.45)",
  zIndex: 220,
  display: "flex",
  alignItems: "center",
  justifyContent: "center",
};

const dialogStyle: React.CSSProperties = {
  width: 520,
  maxWidth: "90vw",
  maxHeight: "85vh",
  overflowY: "auto",
  background: colors.bg,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.card,
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
  fontSize: 24,
  color: colors.textMuted,
  cursor: "pointer",
  padding: 0,
  lineHeight: 1,
};

const leadStyle: React.CSSProperties = {
  margin: 0,
  fontSize: 13,
  color: colors.textMuted,
  lineHeight: 1.45,
};

const mutedStyle: React.CSSProperties = {
  margin: 0,
  fontSize: 13,
  color: colors.textMuted,
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

const successBlockStyle: React.CSSProperties = {
  margin: 0,
  fontSize: 13,
  color: colors.text,
  background: colors.bgSoft,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.sm,
  padding: spacing.sm,
};

const successInlineStyle: React.CSSProperties = {
  margin: 0,
  fontSize: 12,
  color: colors.success,
};

const footerStyle: React.CSSProperties = {
  display: "flex",
  justifyContent: "flex-end",
  gap: spacing.sm,
  flexWrap: "wrap",
};

const primaryButtonStyle: React.CSSProperties = {
  background: colors.accent,
  color: "white",
  border: "none",
  borderRadius: radius.sm,
  padding: "8px 14px",
  fontWeight: 600,
  cursor: "pointer",
};

const secondaryButtonStyle: React.CSSProperties = {
  background: "transparent",
  color: colors.text,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.sm,
  padding: "8px 14px",
  cursor: "pointer",
};

const packCardStyle: React.CSSProperties = {
  display: "flex",
  flexDirection: "column",
  gap: spacing.xs,
  background: colors.bgSoft,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.card,
  padding: spacing.md,
};

const packHeaderStyle: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  justifyContent: "space-between",
  fontSize: 11,
  textTransform: "uppercase",
  letterSpacing: 0.5,
  color: colors.textMuted,
};

const packTierStyle: React.CSSProperties = {
  fontWeight: 600,
};

const packSizeStyle: React.CSSProperties = {
  fontVariantNumeric: "tabular-nums",
};

const packTitleStyle: React.CSSProperties = {
  margin: 0,
  fontSize: 15,
  fontWeight: 600,
};

const packDescStyle: React.CSSProperties = {
  margin: 0,
  fontSize: 12,
  color: colors.textMuted,
  lineHeight: 1.4,
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
