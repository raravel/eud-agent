import { afterEach, describe, expect, it, vi } from "vitest";

import {
  createLogSaver,
  maySaveLog,
  projectHasNoDesktopWindow,
  saveLog,
  savedLogIsNewer,
} from "./logWriter";

const entry = (desktopWindow: boolean) => ({
  path: "E:/maps/a",
  name: "A",
  open: true,
  projectId: "p1",
  desktopWindow,
  available: true,
});

describe("maySaveLog", () => {
  it("lets a phone that took part save without asking", async () => {
    const invoke = vi.fn();
    await expect(maySaveLog(true, invoke, "p1")).resolves.toBe(true);
    expect(invoke).not.toHaveBeenCalled();
  });

  it("keeps a watching phone from overwriting a PC window's log", async () => {
    const invoke = vi.fn().mockResolvedValue([entry(true)]);
    await expect(maySaveLog(false, invoke, "p1")).resolves.toBe(false);
    expect(invoke).toHaveBeenCalledWith("remote_projects");
  });

  it("lets a watching phone save when no PC window shows the project", async () => {
    await expect(maySaveLog(false, vi.fn().mockResolvedValue([entry(false)]), "p1")).resolves.toBe(
      true,
    );
  });

  it("does not save when the project is unknown or the lookup fails", async () => {
    await expect(projectHasNoDesktopWindow(vi.fn(), null)).resolves.toBe(false);
    await expect(
      projectHasNoDesktopWindow(vi.fn().mockResolvedValue([entry(false)]), "other"),
    ).resolves.toBe(false);
    await expect(
      projectHasNoDesktopWindow(vi.fn().mockRejectedValue("offline"), "p1"),
    ).resolves.toBe(false);
  });
});

describe("savedLogIsNewer", () => {
  const log = (logSeq: number) => ({ schemaVersion: 2, logSeq, log: [] });
  it("keeps a stale pending save from overwriting rows the PC saved since", () => {
    expect(savedLogIsNewer(log(12), log(9))).toBe(true);
    expect(savedLogIsNewer(log(9), log(9))).toBe(false);
    expect(savedLogIsNewer(log(3), log(9))).toBe(false);
    expect(savedLogIsNewer(undefined, log(0))).toBe(false);
  });
});

describe("createLogSaver", () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  it("saves after a pause, and at least every few seconds while changes stream", async () => {
    vi.useFakeTimers();
    const save = vi.fn(async () => {});
    const saver = createLogSaver(save, 500, 3_000);
    // A change every 200 ms never leaves a 500 ms pause.
    for (let elapsed = 0; elapsed < 2_800; elapsed += 200) {
      saver.schedule();
      await vi.advanceTimersByTimeAsync(200);
    }
    expect(save).not.toHaveBeenCalled();
    saver.schedule();
    await vi.advanceTimersByTimeAsync(200);
    expect(save).toHaveBeenCalledTimes(1);
    // A single change saves after the pause.
    saver.schedule();
    await vi.advanceTimersByTimeAsync(499);
    expect(save).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(save).toHaveBeenCalledTimes(2);
  });

  it("flushes what is pending at once, and nothing when nothing is", async () => {
    vi.useFakeTimers();
    const save = vi.fn(async () => {});
    const saver = createLogSaver(save);
    await saver.flush();
    expect(save).not.toHaveBeenCalled();
    saver.schedule();
    expect(saver.pending()).toBe(true);
    await saver.flush();
    expect(save).toHaveBeenCalledTimes(1);
    expect(saver.pending()).toBe(false);
    await vi.advanceTimersByTimeAsync(1_000);
    expect(save).toHaveBeenCalledTimes(1);
  });
});

describe("saveLog", () => {
  const log = (logSeq: number) => ({ schemaVersion: 2, logSeq, log: [] });
  function setup(savedSeq: number, reconnects: number) {
    const calls: string[] = [];
    const invoke = vi.fn(async (cmd: string) => {
      calls.push(cmd);
      if (cmd === "session_load") return { panelLog: log(savedSeq) };
      return null;
    });
    const transport = { waitForOpen: vi.fn(async () => true), reconnectCount: () => reconnects };
    return { calls, invoke, transport };
  }

  it("saves without asking the PC while no reconnect happened since loading", async () => {
    const { calls, invoke, transport } = setup(99, 0);
    const saved = await saveLog({
      invoke, id: "m1", participated: true, projectId: "p1", log: () => log(5), transport, loadedAt: 0,
    });
    expect(saved).toBe("saved");
    expect(calls).toEqual(["session_update_log"]);
  });

  it("drops a Map or EPS save after a reconnect when the PC saved a newer log", async () => {
    const { calls, invoke, transport } = setup(99, 1);
    const saved = await saveLog({
      invoke, id: "m1", participated: true, projectId: "p1", log: () => log(5), transport, loadedAt: 0,
    });
    expect(saved).toBe("stale");
    expect(calls).toEqual(["session_load"]);
  });

  it("still saves after a reconnect when its log is the newest", async () => {
    const { calls, invoke, transport } = setup(3, 1);
    await expect(
      saveLog({ invoke, id: "m1", participated: true, projectId: "p1", log: () => log(5), transport, loadedAt: 0 }),
    ).resolves.toBe("saved");
    expect(calls).toEqual(["session_load", "session_update_log"]);
  });

  it("waits for the socket and never writes when it does not come back", async () => {
    const { calls, invoke, transport } = setup(0, 0);
    transport.waitForOpen.mockResolvedValue(false);
    await expect(
      saveLog({ invoke, id: "m1", participated: true, projectId: "p1", log: () => log(5), transport, loadedAt: 0 }),
    ).resolves.toBe("skipped");
    expect(calls).toEqual([]);
  });
});
