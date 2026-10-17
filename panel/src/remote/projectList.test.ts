import { describe, expect, it, vi } from "vitest";

import type { RemoteProjectEntry } from "@/lib/remoteTransport";
import { canCloseProject, canForceClose, openEntry, projectBadge } from "./projectList";

const entry = (overrides: Partial<RemoteProjectEntry>): RemoteProjectEntry => ({
  path: "C:\\maps\\sample",
  name: "Sample",
  open: false,
  desktopWindow: false,
  available: true,
  ...overrides,
});

describe("remote project list", () => {
  it("names where an open project is shown", () => {
    expect(projectBadge(entry({ open: true, projectId: "p", desktopWindow: true }))).toBe(
      "PC 창에서 열림",
    );
    expect(projectBadge(entry({ open: true, projectId: "p" }))).toBe("원격으로 열림");
    expect(projectBadge(entry({}))).toBeNull();
  });

  it("closes only a project no PC window shows", () => {
    expect(canCloseProject(entry({ open: true, projectId: "p" }))).toBe(true);
    expect(canCloseProject(entry({ open: true, projectId: "p", desktopWindow: true }))).toBe(false);
    expect(canCloseProject(entry({}))).toBe(false);
  });

  it("offers to stop work only for a work or build refusal", () => {
    expect(canForceClose("busy")).toBe(true);
    expect(canForceClose("building")).toBe(true);
    expect(canForceClose("companions")).toBe(false);
    expect(canForceClose("switching")).toBe(false);
    expect(canForceClose("error")).toBe(false);
    expect(canForceClose(undefined)).toBe(false);
  });

  it("attaches to an open project and opens a recent one by its path", async () => {
    const project = { projectId: "p", name: "Sample", root: "C:\\maps\\sample" };
    const actions = {
      attach: vi.fn(async () => project),
      openProject: vi.fn(async () => project),
    };
    await openEntry(actions, entry({ open: true, projectId: "p" }));
    expect(actions.attach).toHaveBeenCalledWith("p");
    expect(actions.openProject).not.toHaveBeenCalled();

    await openEntry(actions, entry({}));
    expect(actions.openProject).toHaveBeenCalledWith("C:\\maps\\sample");
  });
});
