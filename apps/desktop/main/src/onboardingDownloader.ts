// Phase C — one-click recommended-pack downloader.
//
// This is now a thin wrapper over the general-purpose
// `modelDownloader.ts` engine. The first-run WelcomeModal's
// one-click install resolves the *device-recommended* pack id and
// delegates to the shared streaming + verify + install pipeline; the
// model manager's in-app "Download" button uses the same engine
// directly with an explicit pack id (see `main.ts`).
//
// The renderer's only handle to this module is the
// `kcreate/onboarding/installRecommendedPack` IPC declared in
// `main.ts`. The renderer never sees a URL — the engine resolves the
// pack id to a URL via the static registry returned by
// `aiListModelPacks`, streams the bytes into a temp file, then hands
// the temp path to the existing `aiInstallModelPack` (SHA-256 verify
// + atomic rename into `models_dir`).
//
// Why a wrapper instead of folding onboarding into the engine:
// onboarding has one extra concern the model manager doesn't — it
// must resolve which pack to install from the device profile
// (`llmRecommendedPack()`), and it must do so *inside* the async
// download flow so a "no recommended pack for this device" failure
// surfaces as an `error` progress event rather than a synchronous
// throw. The engine supports this via its lazy `PackIdResolver`.

import type { BrowserWindow } from "electron";

import {
  startPackDownload,
  validateOpenExternalUrl as validateOpenExternalUrlImpl,
  findPackInRegistryJson as findPackInRegistryJsonImpl,
  parseInstallReport as parseInstallReportImpl,
  ALLOWED_HOSTS as ALLOWED_HOSTS_IMPL,
  type DownloaderBridge,
  type DownloadHandle,
  type DownloadInstallReport,
  type DownloadProgress,
  type RegistryPack as RegistryPackImpl,
} from "./modelDownloader";

/**
 * Mirror of `ModelPack` (subset) — re-exported from the shared engine
 * so existing imports of this module keep working.
 */
export type RegistryPack = RegistryPackImpl;

/**
 * Progress event emitted on the
 * `kcreate/onboarding/installProgress` channel every ~256 KiB of
 * downloaded bytes while the welcome modal's one-click install is
 * running. Structurally identical to the model manager's
 * [`DownloadProgress`]; aliased here for the onboarding call sites
 * that referred to it by this name.
 */
export type OnboardingProgress = DownloadProgress;

/**
 * Result returned by `start()` on success. Mirrors the Rust
 * `InstallReport`: `verified=true` means the registry pinned a
 * SHA-256 and the downloaded bytes match; `verified=false` means the
 * registry has no pinned hash yet so the actual hash is reported for
 * the user's records.
 */
export type OnboardingInstallReport = DownloadInstallReport;

/**
 * Handle returned by `start()` so the renderer can abort the download
 * (e.g. when the welcome modal is dismissed mid-install).
 */
export type OnboardingHandle = DownloadHandle;

/**
 * Bridge surface the onboarding flow needs: the shared downloader
 * methods plus the device-recommendation lookup that picks which pack
 * to install on first run.
 */
export interface OnboardingBridge extends DownloaderBridge {
  llmRecommendedPack(): string;
}

/**
 * IPC channel the main process pushes [`OnboardingProgress`] events
 * on. The preload subscribes the renderer to this exact string.
 */
export const ONBOARDING_PROGRESS_CHANNEL = "kcreate/onboarding/installProgress";

/**
 * The hostnames the downloader will fetch from. Re-exported from the
 * shared engine; kept here so the comments in `main.ts` /
 * `shared/scene.ts` that reference `onboardingDownloader.ALLOWED_HOSTS`
 * still resolve to a real export.
 */
export const ALLOWED_HOSTS = ALLOWED_HOSTS_IMPL;

/**
 * Pure parsing helper — re-exported so the wire-format lockstep test
 * (`onboardingDownloader.test.ts`) keeps exercising the contract via
 * this module's public surface.
 */
export const findPackInRegistryJson = findPackInRegistryJsonImpl;

/**
 * Pure parsing helper — re-exported for the same lockstep test.
 */
export const parseInstallReport = parseInstallReportImpl;

/**
 * Validate `url` and return a sanitized URL string suitable for
 * `shell.openExternal`. Re-exported from the shared engine so the
 * `kcreate/system/openExternal` handler in `main.ts` keeps importing
 * it from here.
 */
export const validateOpenExternalUrl = validateOpenExternalUrlImpl;

/**
 * Phase C — kick off the one-click recommended-pack download. Resolves
 * the device-recommended pack id lazily inside the download's async
 * flow (so a "no recommended pack" failure surfaces as an `error`
 * progress event + rejected `done` promise), then delegates to the
 * shared streaming + verify + install engine, emitting progress on
 * [`ONBOARDING_PROGRESS_CHANNEL`].
 */
export function start(
  bridge: OnboardingBridge,
  window: BrowserWindow | null,
): OnboardingHandle {
  return startPackDownload(
    bridge,
    window,
    () => {
      const packId = bridge.llmRecommendedPack();
      if (!packId) {
        throw new Error("no recommended model pack for this device");
      }
      return packId;
    },
    ONBOARDING_PROGRESS_CHANNEL,
  );
}
