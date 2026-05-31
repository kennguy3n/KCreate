// Phase C — one-click recommended-pack downloader.
//
// The renderer's WelcomeModal cannot reach the network directly
// (preload deliberately exposes a narrow surface), and the Rust
// editing-path crates can't either (the `local_first.rs` deny-list
// blocks every networking crate from the editing-path closure on
// purpose). The download therefore lives in the Electron main
// process, which is the one place in the stack that can do HTTPS
// without breaking either invariant: Node ships TLS, and main is
// outside the Rust editing-path tree.
//
// The renderer's only handle to this module is the
// `kcreate/onboarding/installRecommendedPack` IPC declared in
// `main.ts`. The renderer never sees a URL — `start()` resolves
// the pack id via the bridge, looks the URL up from the static
// registry returned by `aiListModelPacks`, streams the bytes into
// a temp file, then hands the temp path to the existing
// `aiInstallModelPack` (which does the SHA-256 verify + atomic
// rename into `models_dir`). The renderer therefore inherits the
// same security guarantees the manual "I have the file" flow
// already had — the downloader is just a pre-step that resolves
// the file path the user would otherwise have provided themselves.

import { createWriteStream, type WriteStream } from "node:fs";
import * as fs from "node:fs/promises";
import * as https from "node:https";
import * as os from "node:os";
import * as path from "node:path";
import { URL } from "node:url";

import type { BrowserWindow } from "electron";

/**
 * Mirror of `ModelPack` (subset). Kept inline so we don't pull
 * the renderer's `shared/scene.ts` (which depends on Electron
 * renderer types) into the main process.
 */
interface RegistryPack {
  readonly id: string;
  readonly name: string;
  readonly kind: string;
  readonly download_url: string;
  readonly size_bytes: number;
  readonly file_path: string;
}

/**
 * Progress event emitted on the
 * `kcreate/onboarding/installProgress` channel so the renderer can
 * drive a progress bar. `total_bytes` is `null` until the HTTP
 * `Content-Length` is observed (some Hugging Face URLs redirect
 * through a CDN whose first hop omits the header). All numeric
 * fields are byte counts so the renderer can render percentages
 * with whatever precision it wants.
 */
export interface OnboardingProgress {
  /** Pack id the download is for. */
  readonly pack_id: string;
  /** Human-readable phase. UI uses it for accessible status text. */
  readonly phase:
    | "resolving"
    | "connecting"
    | "downloading"
    | "verifying"
    | "installing"
    | "done"
    | "error"
    | "cancelled";
  /** Bytes received from the server so far. `0` until streaming. */
  readonly received_bytes: number;
  /** Server-reported total in bytes. `null` until known. */
  readonly total_bytes: number | null;
  /** Free-text message (filled on `error` / `done`; otherwise empty). */
  readonly message: string;
}

/**
 * Result returned by `start()` when the install completes
 * successfully. Mirrors the `InstallReport` returned by the Rust
 * installer so the renderer can surface the verified flag + actual
 * sha256 in the welcome-modal completion screen.
 */
export interface OnboardingInstallReport {
  readonly pack_id: string;
  readonly verified: boolean;
  readonly actual_sha256: string;
  readonly size_bytes: number;
}

/**
 * Subset of the kcreate bridge surface this module touches. Kept
 * structural so unit tests can pass a fake without pulling in the
 * native addon. The IPC layer in `main.ts` is the only production
 * caller, and it passes the real bridge object.
 */
export interface OnboardingBridge {
  llmRecommendedPack(): string;
  aiListModelPacks(): string;
  aiInstallModelPack(packId: string, sourcePath: string): string;
}

/**
 * The set of hostnames `start()` is willing to download from. The
 * registry currently only points at Hugging Face mirrors, but the
 * allow-list is centralised here so a future pack pointing at a
 * different mirror requires an explicit code change rather than
 * silently widening the surface. SSRF prevention: the renderer
 * cannot influence the URL at all, but defence in depth means even
 * a maliciously-edited preferences file or a typo in the registry
 * can't get us to fetch from an unintended host.
 */
const ALLOWED_HOSTS: ReadonlySet<string> = new Set([
  "huggingface.co",
  "cdn-lfs.huggingface.co",
  "cdn-lfs-us-1.huggingface.co",
  "cdn-lfs-eu-1.huggingface.co",
]);

/**
 * Cap on the number of HTTP redirects the downloader follows.
 * Hugging Face typically does `huggingface.co/...` → `cdn-lfs...`;
 * we tolerate a few additional hops without unbounding the chain
 * (a maliciously-configured server could redirect-loop forever).
 */
const MAX_REDIRECTS = 5;

/**
 * How often (in bytes) to flush a progress event to the renderer.
 * One event every ~256 KiB keeps the IPC noise bounded for the
 * multi-gigabyte 8B pack while still feeling smooth at 60 fps.
 */
const PROGRESS_EVENT_INTERVAL_BYTES = 256 * 1024;

/**
 * IPC channel name renderers subscribe to for progress events.
 * Exported so `main.ts` doesn't need to duplicate the constant.
 */
export const ONBOARDING_PROGRESS_CHANNEL = "kcreate/onboarding/installProgress";

/**
 * State carried across the lifetime of a single download. Each
 * call to `start()` allocates a fresh instance; `start()` is
 * serialised by the in-flight guard in `main.ts` so we never have
 * two concurrent runs.
 */
interface RunState {
  cancelled: boolean;
  /** Active write stream, so cancel can close it. */
  writeStream: WriteStream | null;
  /** Active http request, so cancel can `.destroy()` it. */
  request: import("node:http").ClientRequest | null;
  tempPath: string | null;
}

/**
 * Returned by `start()` so the renderer can abort a download mid-
 * flight (typically by closing the welcome modal). Calling
 * `cancel()` on a run that has already finished is a no-op.
 */
export interface OnboardingHandle {
  /** Promise that resolves with the install report on success. */
  readonly done: Promise<OnboardingInstallReport>;
  /** Best-effort abort. Idempotent. */
  cancel(): void;
}

/**
 * Look up a pack by id in the JSON registry the bridge exposes.
 * The bridge returns a JSON-encoded array; we parse it lazily so
 * a malformed payload surfaces as a typed error rather than a
 * silent `undefined`.
 */
function findPack(
  bridge: OnboardingBridge,
  packId: string,
): RegistryPack | null {
  const raw = bridge.aiListModelPacks();
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch (e) {
    throw new Error(
      `onboarding: aiListModelPacks returned invalid JSON: ${
        e instanceof Error ? e.message : String(e)
      }`,
    );
  }
  if (!Array.isArray(parsed)) {
    throw new Error(
      "onboarding: aiListModelPacks did not return an array",
    );
  }
  for (const candidate of parsed) {
    if (
      typeof candidate === "object" &&
      candidate !== null &&
      (candidate as { id?: unknown }).id === packId
    ) {
      return candidate as unknown as RegistryPack;
    }
  }
  return null;
}

/**
 * Validate a download URL against the allow-list. Throws on
 * anything that isn't `https:` or whose hostname isn't pre-approved.
 */
function validateUrl(rawUrl: string): URL {
  let parsed: URL;
  try {
    parsed = new URL(rawUrl);
  } catch (e) {
    throw new Error(
      `onboarding: invalid pack URL: ${
        e instanceof Error ? e.message : String(e)
      }`,
    );
  }
  if (parsed.protocol !== "https:") {
    throw new Error(
      `onboarding: pack URL must use https (got ${parsed.protocol})`,
    );
  }
  if (!ALLOWED_HOSTS.has(parsed.hostname)) {
    throw new Error(
      `onboarding: pack URL host '${parsed.hostname}' is not in the download allow-list`,
    );
  }
  return parsed;
}

/**
 * Allocate a per-pack temp file under `os.tmpdir()`. We include the
 * pack file name + pid so the path is human-readable in /tmp during
 * a long download, and is unique across concurrent KCreate
 * processes (the bridge installer also uses pid suffixes for the
 * same reason).
 */
function tempPathFor(pack: RegistryPack): string {
  const base = path.basename(pack.file_path) || `${pack.id}.bin`;
  return path.join(os.tmpdir(), `kcreate-onboarding-${process.pid}-${base}`);
}

/**
 * Emit a progress event to the renderer if the window is still
 * alive. We swallow `webContents.send` errors because a destroyed
 * window during cancel/quit must not crash the downloader.
 */
function emitProgress(
  window: BrowserWindow | null,
  event: OnboardingProgress,
): void {
  if (!window || window.isDestroyed()) return;
  try {
    window.webContents.send(ONBOARDING_PROGRESS_CHANNEL, event);
  } catch {
    // Renderer is unreachable. Drop the event — the download
    // continues regardless; the renderer's progress bar will just
    // freeze, which is the correct UX for a vanished window.
  }
}

/**
 * Best-effort temp-file cleanup. `unlink` is async, but we never
 * await it from the cancel path — if the file is gone (success),
 * we ignore ENOENT; if it isn't, the OS sweeps `/tmp` eventually.
 */
async function safeUnlink(filePath: string): Promise<void> {
  try {
    await fs.unlink(filePath);
  } catch (e) {
    // ENOENT after a successful rename-into-place is expected;
    // any other error is non-fatal for the install flow.
    if ((e as NodeJS.ErrnoException).code !== "ENOENT") {
      console.warn(
        `onboarding: temp cleanup failed for ${filePath}:`,
        e instanceof Error ? e.message : String(e),
      );
    }
  }
}

/**
 * Stream the bytes at `url` into `dest`, emitting progress events
 * to `window`. Resolves with the number of bytes written. Rejects
 * on cancel, HTTP error, or stream error. Follows up to
 * `MAX_REDIRECTS` 30x responses.
 */
function streamDownload(
  url: URL,
  dest: WriteStream,
  pack: RegistryPack,
  state: RunState,
  window: BrowserWindow | null,
  redirectsRemaining: number,
): Promise<number> {
  return new Promise((resolve, reject) => {
    if (state.cancelled) {
      reject(new Error("cancelled"));
      return;
    }

    emitProgress(window, {
      pack_id: pack.id,
      phase: "connecting",
      received_bytes: 0,
      total_bytes: null,
      message: "",
    });

    const req = https.get(
      url,
      {
        // Sensible default UA so any HF / CDN rate-limit logging
        // can attribute the traffic.
        headers: { "User-Agent": "kcreate-onboarding/1.0" },
      },
      (res) => {
        const status = res.statusCode ?? 0;

        // Redirect handling. The Hugging Face `resolve/main/...`
        // URL almost always 302s to a CDN; we follow it manually
        // so we can re-validate the new hostname against the
        // allow-list (preserves SSRF defence on every hop).
        if (status >= 300 && status < 400 && res.headers.location) {
          res.resume(); // drain so the socket can be reused
          if (redirectsRemaining <= 0) {
            reject(new Error(`too many redirects (>${MAX_REDIRECTS})`));
            return;
          }
          let next: URL;
          try {
            next = new URL(res.headers.location, url);
          } catch (e) {
            reject(
              new Error(
                `invalid redirect target: ${
                  e instanceof Error ? e.message : String(e)
                }`,
              ),
            );
            return;
          }
          let validated: URL;
          try {
            validated = validateUrl(next.toString());
          } catch (e) {
            reject(e instanceof Error ? e : new Error(String(e)));
            return;
          }
          streamDownload(
            validated,
            dest,
            pack,
            state,
            window,
            redirectsRemaining - 1,
          )
            .then(resolve)
            .catch(reject);
          return;
        }

        if (status !== 200) {
          reject(new Error(`HTTP ${status} for ${url.toString()}`));
          res.resume();
          return;
        }

        const totalHeader = res.headers["content-length"];
        const total =
          typeof totalHeader === "string" && /^\d+$/.test(totalHeader)
            ? Number.parseInt(totalHeader, 10)
            : null;

        emitProgress(window, {
          pack_id: pack.id,
          phase: "downloading",
          received_bytes: 0,
          total_bytes: total,
          message: "",
        });

        let received = 0;
        let lastEmitAt = 0;

        res.on("data", (chunk: Buffer) => {
          if (state.cancelled) {
            res.destroy();
            return;
          }
          received += chunk.length;
          if (received - lastEmitAt >= PROGRESS_EVENT_INTERVAL_BYTES) {
            lastEmitAt = received;
            emitProgress(window, {
              pack_id: pack.id,
              phase: "downloading",
              received_bytes: received,
              total_bytes: total,
              message: "",
            });
          }
        });

        res.on("error", (err) => {
          reject(err);
        });

        dest.on("error", (err) => {
          res.destroy();
          reject(err);
        });

        dest.on("finish", () => {
          if (state.cancelled) {
            reject(new Error("cancelled"));
            return;
          }
          // Final flush so the renderer's progress bar lands at
          // 100% before the verify phase starts.
          emitProgress(window, {
            pack_id: pack.id,
            phase: "downloading",
            received_bytes: received,
            total_bytes: total ?? received,
            message: "",
          });
          resolve(received);
        });

        res.pipe(dest);
      },
    );

    state.request = req;

    req.on("error", (err) => {
      reject(err);
    });
  });
}

/**
 * Drive the full flow:
 * 1. Resolve the recommended pack id via the bridge.
 * 2. Look up the URL + metadata in the registry.
 * 3. Validate the URL against the allow-list.
 * 4. Stream-download into a per-process temp file.
 * 5. Hand the temp path to `aiInstallModelPack` (SHA-256 verify +
 *    atomic rename).
 * 6. Return the parsed InstallReport.
 *
 * `cancel()` aborts at any phase; the temp file is unlinked
 * best-effort. The caller's `done` promise rejects with
 * `"cancelled"` on abort so the renderer can render a "Cancelled"
 * state rather than a generic error.
 */
export function start(
  bridge: OnboardingBridge,
  window: BrowserWindow | null,
): OnboardingHandle {
  const state: RunState = {
    cancelled: false,
    writeStream: null,
    request: null,
    tempPath: null,
  };

  const done = (async (): Promise<OnboardingInstallReport> => {
    // 1. Resolve recommended pack id.
    emitProgress(window, {
      pack_id: "",
      phase: "resolving",
      received_bytes: 0,
      total_bytes: null,
      message: "",
    });
    const packId = bridge.llmRecommendedPack();
    if (!packId) {
      throw new Error("no recommended LLM pack for this device");
    }

    // 2. Look up URL + metadata.
    const pack = findPack(bridge, packId);
    if (!pack) {
      throw new Error(`recommended pack '${packId}' not in registry`);
    }
    if (!pack.download_url) {
      throw new Error(
        `recommended pack '${packId}' has no download URL pinned in the registry`,
      );
    }

    // 3. Validate URL.
    const validated = validateUrl(pack.download_url);

    // 4. Stream-download to a per-process temp.
    const temp = tempPathFor(pack);
    state.tempPath = temp;

    // If the tmp file exists from a previous crashed run, unlink
    // it so we don't try to append. The downloader always writes
    // to a fresh file.
    await safeUnlink(temp);

    const writeStream = createWriteStream(temp);
    state.writeStream = writeStream;

    let received = 0;
    try {
      received = await streamDownload(
        validated,
        writeStream,
        pack,
        state,
        window,
        MAX_REDIRECTS,
      );
    } catch (e) {
      writeStream.destroy();
      await safeUnlink(temp);
      if (state.cancelled) {
        emitProgress(window, {
          pack_id: packId,
          phase: "cancelled",
          received_bytes: 0,
          total_bytes: null,
          message: "",
        });
      }
      throw e;
    }

    if (state.cancelled) {
      await safeUnlink(temp);
      emitProgress(window, {
        pack_id: packId,
        phase: "cancelled",
        received_bytes: received,
        total_bytes: received,
        message: "",
      });
      throw new Error("cancelled");
    }

    // 5. Hand the temp path to the bridge installer. The bridge
    // does SHA-256 verification (if the registry pins a hash) and
    // atomically renames into `models_dir`. We report both phases
    // separately so the renderer can show distinct "verifying" /
    // "installing" labels even though the bridge call is a single
    // synchronous step.
    emitProgress(window, {
      pack_id: packId,
      phase: "verifying",
      received_bytes: received,
      total_bytes: received,
      message: "",
    });
    let reportJson: string;
    try {
      reportJson = bridge.aiInstallModelPack(packId, temp);
    } catch (e) {
      await safeUnlink(temp);
      throw e instanceof Error ? e : new Error(String(e));
    }

    // Successful install moved the bytes out of the temp into
    // models_dir; the temp is gone, but unlink anyway for the
    // belt-and-braces ENOENT-tolerant cleanup.
    await safeUnlink(temp);

    emitProgress(window, {
      pack_id: packId,
      phase: "installing",
      received_bytes: received,
      total_bytes: received,
      message: "",
    });

    let parsed: unknown;
    try {
      parsed = JSON.parse(reportJson);
    } catch (e) {
      throw new Error(
        `onboarding: install report was not valid JSON: ${
          e instanceof Error ? e.message : String(e)
        }`,
      );
    }
    if (
      typeof parsed !== "object" ||
      parsed === null ||
      typeof (parsed as { pack_id?: unknown }).pack_id !== "string" ||
      typeof (parsed as { verified?: unknown }).verified !== "boolean" ||
      typeof (parsed as { actual_sha256?: unknown }).actual_sha256 !==
        "string" ||
      typeof (parsed as { size_bytes?: unknown }).size_bytes !== "number"
    ) {
      throw new Error("onboarding: install report shape was unexpected");
    }
    const report = parsed as OnboardingInstallReport;

    emitProgress(window, {
      pack_id: packId,
      phase: "done",
      received_bytes: received,
      total_bytes: received,
      message: "",
    });

    return report;
  })().catch((err: unknown) => {
    const message = err instanceof Error ? err.message : String(err);
    emitProgress(window, {
      pack_id: "",
      phase: message === "cancelled" ? "cancelled" : "error",
      received_bytes: 0,
      total_bytes: null,
      message,
    });
    throw err instanceof Error ? err : new Error(message);
  });

  return {
    done,
    cancel(): void {
      if (state.cancelled) return;
      state.cancelled = true;
      if (state.request) {
        try {
          state.request.destroy();
        } catch {
          // best-effort
        }
      }
      if (state.writeStream) {
        try {
          state.writeStream.destroy();
        } catch {
          // best-effort
        }
      }
      if (state.tempPath) {
        void safeUnlink(state.tempPath);
      }
    },
  };
}

/**
 * Validate `url` and return a sanitized URL string suitable for
 * `shell.openExternal`. Throws when the URL is malformed, not
 * https, or not in the allow-list. Centralised here so the IPC
 * handler in `main.ts` and any future caller share the exact
 * same validation rules.
 */
export function validateOpenExternalUrl(url: string): string {
  return validateUrl(url).toString();
}
