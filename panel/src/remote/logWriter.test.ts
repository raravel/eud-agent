import { describe, expect, it, vi } from "vitest";

import { maySaveLog, projectHasNoDesktopWindow } from "./logWriter";

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
