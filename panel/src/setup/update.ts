/**
 * Self-update seam over `@tauri-apps/plugin-updater` + `@tauri-apps/plugin-process`.
 *
 * The plugin's `check()` returns a stateful `Update` instance whose
 * `downloadAndInstall` streams chunked `DownloadEvent`s. This module wraps that into a
 * small, injectable surface (`UpdaterSeam`) so the UI deals only with a plain
 * `UpdateHandle` (version/notes + an accumulated `{downloaded,total}` progress callback)
 * and the whole flow is unit-testable headless — mirroring the invoke/listen injection
 * seam in `lib/ipc.ts`.
 */

import { check as tauriCheck, type Update } from "@tauri-apps/plugin-updater";
import { relaunch as tauriRelaunch } from "@tauri-apps/plugin-process";

/** Byte progress of an in-flight download. `total` is null when the server sent no length. */
export interface DownloadProgress {
  downloaded: number;
  total: number | null;
}

/** A pending update, decoupled from the plugin's `Update` instance for testability. */
export interface UpdateHandle {
  /** The available version (e.g. "0.1.1"). */
  version: string;
  /** The currently-running version. */
  currentVersion: string;
  /** Release notes / changelog body (empty when none). */
  notes: string;
  /** Download + install the update, reporting accumulated byte progress. */
  downloadAndInstall(onProgress: (p: DownloadProgress) => void): Promise<void>;
}

/** Injectable updater surface. Defaults to the real plugin via {@link createUpdater}. */
export interface UpdaterSeam {
  /** Resolve the available update, or null when already up to date. */
  check(): Promise<UpdateHandle | null>;
  /** Restart the app into the freshly-installed version. */
  relaunch(): Promise<void>;
}

/** Adapt a plugin `Update` into a {@link UpdateHandle}, accumulating chunk progress. */
export function wrapUpdate(update: Update): UpdateHandle {
  return {
    version: update.version,
    currentVersion: update.currentVersion,
    notes: update.body ?? "",
    downloadAndInstall: async (onProgress) => {
      let downloaded = 0;
      let total: number | null = null;
      await update.downloadAndInstall((event) => {
        switch (event.event) {
          case "Started":
            total = event.data.contentLength ?? null;
            downloaded = 0;
            onProgress({ downloaded, total });
            break;
          case "Progress":
            downloaded += event.data.chunkLength;
            onProgress({ downloaded, total });
            break;
          case "Finished":
            // Snap to 100% — the Finished event carries no length.
            onProgress({ downloaded: total ?? downloaded, total });
            break;
        }
      });
    },
  };
}

/** The production seam: real plugin `check`/`relaunch`. */
export function createUpdater(): UpdaterSeam {
  return {
    check: async () => {
      const update = await tauriCheck();
      return update ? wrapUpdate(update) : null;
    },
    relaunch: () => tauriRelaunch(),
  };
}

/** Whole-number download percentage (0..100), or null when the total is unknown. */
export function downloadPct(p: DownloadProgress): number | null {
  if (p.total === null || p.total <= 0) return null;
  return Math.min(100, Math.max(0, Math.floor((p.downloaded / p.total) * 100)));
}
