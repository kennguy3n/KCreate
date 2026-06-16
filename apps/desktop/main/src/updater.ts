// I1 — Distribution / auto-update.
//
// A thin, main-process-only wrapper around `electron-updater`. It owns a
// single `UpdateState` snapshot (mirroring `shared/scene.ts::UpdateState`),
// translates the library's events into that snapshot, and pushes every
// transition to the renderer over a single `kcreate/update/stateChanged`
// channel. The renderer drives it through `kcreate/update/{getState,check,
// download,quitAndInstall}`.
//
// Design notes:
//   * Opt-in: `autoDownload = false`, so a check never silently pulls a
//     payload — the user explicitly clicks "Download". Installs still apply
//     on the next quit (`autoInstallOnAppQuit`).
//   * Local-first safe: this is JS-only and never crosses the N-API bridge,
//     so it has no bearing on the Rust editing-path network sentinel.
//   * Provider-configurable: the feed comes from the `electron-builder`
//     `publish` block baked into `app-update.yml`, OR can be overridden at
//     runtime with a generic server via `KCREATE_UPDATE_FEED_URL` (used by
//     the local update round-trip proof and by self-hosted deployments).
//   * Unpackaged dev runs report `status: "disabled"` / `supported: false`
//     instead of throwing, so the in-app affordance renders a clean
//     read-only state during development and in the e2e smoke harness.

import { app } from "electron";
// electron-updater re-exports `ProgressInfo` / `UpdateInfo` from
// builder-util-runtime via its barrel, so we pull them from the package
// we already depend on rather than the transitive (and, under pnpm,
// non-hoisted) `builder-util-runtime`.
import {
  autoUpdater,
  type Logger,
  type ProgressInfo,
  type UpdateInfo as BuilderUpdateInfo,
} from "electron-updater";

import type { UpdateState } from "../../shared/scene";
import { toWireInfo, toWireProgress } from "./updaterFormat";

/** Channel the main process pushes `UpdateState` snapshots on. */
export const UPDATE_STATE_CHANGED_CHANNEL = "kcreate/update/stateChanged";

/** Sends a state snapshot to a renderer (typically `mainWindow.webContents.send`). */
export type UpdateBroadcaster = (state: UpdateState) => void;

/**
 * A logger that routes electron-updater output through `console` without
 * ever calling `console.error`. The updater logs benign conditions (e.g.
 * "no published versions" against a fresh feed) at error level; demoting
 * them to `console.warn` keeps the e2e smoke gate — which fails on any
 * unexpected renderer `console.error` — and operator log scrapers honest
 * about what is a real fault.
 */
const quietLogger: Logger = {
  info: (message?: unknown) => console.log("[updater]", message),
  warn: (message?: unknown) => console.warn("[updater]", message),
  error: (message?: unknown) => console.warn("[updater]", message),
  debug: (message: string) => console.log("[updater]", message),
};

/**
 * Public controller surface consumed by `main.ts`. A single instance is
 * created at startup and its methods are wired 1:1 to the
 * `kcreate/update/*` IPC handlers.
 */
export interface UpdaterController {
  getState(): UpdateState;
  check(): Promise<UpdateState>;
  download(): Promise<UpdateState>;
  quitAndInstall(): void;
}

interface UpdaterDeps {
  /** Pushes a snapshot to the renderer; called on every transition. */
  broadcast: UpdateBroadcaster;
}

/**
 * Wire up `electron-updater` and return a controller. Safe to call once
 * at app startup regardless of whether the build is packaged — in an
 * unpackaged dev run it configures nothing and reports `disabled`.
 */
export function createUpdaterController(deps: UpdaterDeps): UpdaterController {
  const disabled = process.env["KCREATE_UPDATE_DISABLED"] === "1";
  // `forceDevUpdateConfig` lets a developer (or the local round-trip
  // proof) exercise the real update flow from an unpackaged checkout by
  // dropping a `dev-app-update.yml` next to the app, or by pointing at a
  // generic feed via `KCREATE_UPDATE_FEED_URL` below.
  const forceDev = process.env["KCREATE_UPDATE_FORCE_DEV"] === "1";
  const feedOverride = process.env["KCREATE_UPDATE_FEED_URL"];

  const supported = !disabled && (app.isPackaged || forceDev);

  let feedUrl: string | null = null;

  if (supported) {
    autoUpdater.logger = quietLogger;
    autoUpdater.autoDownload = false;
    autoUpdater.autoInstallOnAppQuit = true;
    autoUpdater.allowDowngrade = false;
    if (forceDev) {
      autoUpdater.forceDevUpdateConfig = true;
    }
    if (feedOverride && feedOverride.length > 0) {
      // Runtime-configurable generic provider. Overrides whatever
      // `app-update.yml` baked in, which is what the local update
      // round-trip proof relies on (serve `latest-linux.yml` + the
      // AppImage from a localhost static server).
      autoUpdater.setFeedURL({ provider: "generic", url: feedOverride });
      feedUrl = feedOverride;
    }
  }

  const state: UpdateState = {
    status: supported ? "idle" : "disabled",
    currentVersion: app.getVersion(),
    feedUrl,
    info: null,
    progress: null,
    error: null,
    supported,
  };

  const snapshot = (): UpdateState => ({
    ...state,
    info: state.info ? { ...state.info } : null,
    progress: state.progress ? { ...state.progress } : null,
  });

  const emit = (): void => {
    deps.broadcast(snapshot());
  };

  const transition = (next: Partial<UpdateState>): void => {
    Object.assign(state, next);
    emit();
  };

  if (supported) {
    autoUpdater.on("checking-for-update", () => {
      transition({ status: "checking", error: null });
    });
    autoUpdater.on("update-available", (info: BuilderUpdateInfo) => {
      transition({ status: "available", info: toWireInfo(info), error: null });
    });
    autoUpdater.on("update-not-available", (info: BuilderUpdateInfo) => {
      transition({
        status: "not-available",
        info: toWireInfo(info),
        progress: null,
        error: null,
      });
    });
    autoUpdater.on("download-progress", (progress: ProgressInfo) => {
      transition({ status: "downloading", progress: toWireProgress(progress) });
    });
    autoUpdater.on("update-downloaded", (info: BuilderUpdateInfo) => {
      transition({
        status: "downloaded",
        info: toWireInfo(info),
        progress: null,
        error: null,
      });
    });
    autoUpdater.on("error", (err: Error) => {
      transition({ status: "error", error: err.message });
    });
  }

  const check = async (): Promise<UpdateState> => {
    if (!supported) return snapshot();
    try {
      // `checkForUpdates` drives the `checking` / `available` /
      // `not-available` / `error` events above, which keep `state`
      // current; we just surface the resulting snapshot to the caller.
      await autoUpdater.checkForUpdates();
    } catch (err) {
      transition({
        status: "error",
        error: err instanceof Error ? err.message : String(err),
      });
    }
    return snapshot();
  };

  const download = async (): Promise<UpdateState> => {
    if (!supported) return snapshot();
    if (state.status === "downloading" || state.status === "downloaded") {
      return snapshot();
    }
    if (state.status !== "available") {
      // Nothing to download — make the caller re-check first rather
      // than firing a no-target download at the feed.
      return snapshot();
    }
    try {
      await autoUpdater.downloadUpdate();
    } catch (err) {
      transition({
        status: "error",
        error: err instanceof Error ? err.message : String(err),
      });
    }
    return snapshot();
  };

  const quitAndInstall = (): void => {
    if (!supported) {
      throw new Error("Auto-update is not available in this build.");
    }
    if (state.status !== "downloaded") {
      throw new Error("No update has been downloaded yet.");
    }
    // `false, true`: don't run silently (show the installer where the
    // platform has one) and force-relaunch the app afterwards.
    autoUpdater.quitAndInstall(false, true);
  };

  return {
    getState: snapshot,
    check,
    download,
    quitAndInstall,
  };
}
