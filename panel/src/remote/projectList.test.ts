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
    expect(
      canForceClose(
        "진행 중인 작업이나 검토 대기 중인 계획이 있어 프로젝트를 닫지 않았습니다.",
      ),
    ).toBe(true);
    expect(canForceClose("빌드가 진행 중입니다. 빌드가 끝난 뒤 프로젝트를 닫아 주세요.")).toBe(true);
    expect(canForceClose("PC에서 Map Agent와 맵 가져오기 창을 닫은 뒤 프로젝트를 닫아 주세요.")).toBe(
      false,
    );
    expect(
      canForceClose("PC에서 다른 프로젝트를 열거나 닫는 중입니다. 잠시 뒤 다시 시도해 주세요."),
    ).toBe(false);
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
