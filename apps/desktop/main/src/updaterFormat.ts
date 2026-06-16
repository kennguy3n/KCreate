// I1 — pure projections from electron-updater's types onto KCreate's
// narrow `UpdateState` wire shapes.
//
// These helpers are deliberately free of any `electron` / runtime import
// (only type-only imports) so they can be unit-tested in the vitest node
// environment, where `electron` is unavailable. `updater.ts` (which does
// own the electron-coupled controller) consumes them.

import type {
  ProgressInfo,
  UpdateInfo as BuilderUpdateInfo,
} from "electron-updater";

import type { UpdateInfo, UpdateProgress } from "../../shared/scene";

/**
 * Coalesce electron-updater's `releaseNotes` (which may be a string, a
 * list of `{ version, note }` records, or null) into a single plain
 * string for the renderer. A list is rendered newest-first, one release
 * per paragraph, so the in-app changelog stays readable without the
 * renderer needing to know the library's union shape.
 */
export function coalesceReleaseNotes(
  notes: BuilderUpdateInfo["releaseNotes"],
): string | null {
  if (notes == null) return null;
  if (typeof notes === "string") {
    const trimmed = notes.trim();
    return trimmed.length > 0 ? trimmed : null;
  }
  const parts = notes
    .map((entry) => {
      const note = (entry.note ?? "").trim();
      if (note.length === 0) return null;
      return entry.version ? `v${entry.version}\n${note}` : note;
    })
    .filter((part): part is string => part !== null);
  return parts.length > 0 ? parts.join("\n\n") : null;
}

/** Project electron-updater's `UpdateInfo` onto the narrow wire shape. */
export function toWireInfo(info: BuilderUpdateInfo): UpdateInfo {
  return {
    version: info.version,
    releaseDate: info.releaseDate ?? null,
    releaseNotes: coalesceReleaseNotes(info.releaseNotes),
  };
}

/** Project electron-updater's `ProgressInfo` onto the narrow wire shape. */
export function toWireProgress(progress: ProgressInfo): UpdateProgress {
  return {
    percent: progress.percent,
    bytesPerSecond: progress.bytesPerSecond,
    transferred: progress.transferred,
    total: progress.total,
  };
}
