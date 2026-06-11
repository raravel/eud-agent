/**
 * Unit tests for the self-update seam (`@/setup/update`).
 *
 * `wrapUpdate` must map the plugin `Update` fields and accumulate chunked
 * `DownloadEvent`s into running `{downloaded,total}` snapshots; `downloadPct`
 * converts those into a clamped whole-number percentage (null when length is unknown).
 */
import { describe, it, expect } from "vitest";
import type { Update } from "@tauri-apps/plugin-updater";
import { wrapUpdate, downloadPct, type DownloadProgress } from "@/setup/update";

type DownloadEvent = Parameters<Update["downloadAndInstall"]>[0] extends
  | undefined
  | ((e: infer E) => unknown)
  ? E
  : never;

function fakeUpdate(
  events: DownloadEvent[],
  fields?: Partial<Pick<Update, "version" | "currentVersion" | "body">>,
): Update {
  return {
    version: "0.1.1",
    currentVersion: "0.1.0",
    body: "release notes",
    downloadAndInstall: async (cb?: (e: DownloadEvent) => void) => {
      if (cb) for (const e of events) cb(e);
    },
    ...fields,
  } as unknown as Update;
}

describe("wrapUpdate", () => {
  it("maps version/currentVersion/notes", () => {
    const handle = wrapUpdate(fakeUpdate([]));
    expect(handle.version).toBe("0.1.1");
    expect(handle.currentVersion).toBe("0.1.0");
    expect(handle.notes).toBe("release notes");
  });

  it("defaults notes to empty when body is absent", () => {
    const handle = wrapUpdate(fakeUpdate([], { body: undefined }));
    expect(handle.notes).toBe("");
  });

  it("accumulates chunk progress and snaps to total on finish", async () => {
    const handle = wrapUpdate(
      fakeUpdate([
        { event: "Started", data: { contentLength: 100 } },
        { event: "Progress", data: { chunkLength: 40 } },
        { event: "Progress", data: { chunkLength: 60 } },
        { event: "Finished" },
      ] as DownloadEvent[]),
    );

    const seen: DownloadProgress[] = [];
    await handle.downloadAndInstall((p) => seen.push({ ...p }));

    expect(seen).toEqual([
      { downloaded: 0, total: 100 },
      { downloaded: 40, total: 100 },
      { downloaded: 100, total: 100 },
      { downloaded: 100, total: 100 },
    ]);
  });

  it("tolerates a missing content length (total stays null)", async () => {
    const handle = wrapUpdate(
      fakeUpdate([
        { event: "Started", data: {} },
        { event: "Progress", data: { chunkLength: 10 } },
        { event: "Finished" },
      ] as DownloadEvent[]),
    );

    const seen: DownloadProgress[] = [];
    await handle.downloadAndInstall((p) => seen.push({ ...p }));

    expect(seen).toEqual([
      { downloaded: 0, total: null },
      { downloaded: 10, total: null },
      { downloaded: 10, total: null },
    ]);
  });
});

describe("downloadPct", () => {
  it("computes a floored percentage", () => {
    expect(downloadPct({ downloaded: 50, total: 100 })).toBe(50);
    expect(downloadPct({ downloaded: 1, total: 3 })).toBe(33);
  });

  it("returns null when the total is unknown or non-positive", () => {
    expect(downloadPct({ downloaded: 10, total: null })).toBeNull();
    expect(downloadPct({ downloaded: 10, total: 0 })).toBeNull();
  });

  it("clamps to the 0..100 range", () => {
    expect(downloadPct({ downloaded: 200, total: 100 })).toBe(100);
    expect(downloadPct({ downloaded: -5, total: 100 })).toBe(0);
  });
});
