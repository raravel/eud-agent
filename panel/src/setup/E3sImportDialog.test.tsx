import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { E3sImportDialog } from "@/setup/E3sImportDialog";
import type { SetupMessage } from "@/lib/protocol";

const successfulSetup: SetupMessage = {
  type: "setup",
  projectPath: "C:\\Work\\ImportedProject",
  projectValid: true,
  projectOpened: true,
  euddraftPath: "",
  euddraftValid: false,
  assetsReady: false,
  providers: [],
  setupRequired: true,
  error: null,
};

describe("E3sImportDialog", () => {
  it("keeps E3S and work-folder selection as explicit ordered steps", async () => {
    const sourcePath = String.raw`\\?\C:\Legacy\sample.e3s`;
    const destinationPath = String.raw`\\?\UNC\server\Work\ImportedProject`;
    const pickSource = vi.fn(async () => ({ path: sourcePath }));
    const pickDestination = vi
      .fn()
      .mockResolvedValueOnce({ path: "C:\\Work\\Used", empty: false })
      .mockResolvedValueOnce({ path: destinationPath, empty: true });
    const importProject = vi.fn(async () => successfulSetup);
    const onImported = vi.fn();
    const onOpenChange = vi.fn();

    render(
      <E3sImportDialog
        open
        onOpenChange={onOpenChange}
        pickSource={pickSource}
        pickDestination={pickDestination}
        importProject={importProject}
        onImported={onImported}
      />,
    );

    expect(screen.getByRole("heading", { name: "E3S 프로젝트 가져오기" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "닫기" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "선택한 위치로 가져오기" })).toBeDisabled();

    await userEvent.click(screen.getByRole("button", { name: "E3S 파일 선택" }));
    expect(screen.getByText("C:\\Legacy\\sample.e3s")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "새 작업 폴더 선택" }));
    expect(screen.getByRole("alert")).toHaveTextContent("비어 있지 않습니다");
    expect(screen.getByRole("button", { name: "선택한 위치로 가져오기" })).toBeDisabled();

    await userEvent.click(screen.getByRole("button", { name: "다른 작업 폴더 선택" }));
    expect(screen.getByText(String.raw`\\server\Work\ImportedProject`)).toBeInTheDocument();
    const importButton = screen.getByRole("button", { name: "선택한 위치로 가져오기" });
    expect(importButton).toBeEnabled();
    await userEvent.click(importButton);

    expect(importProject).toHaveBeenCalledWith({
      sourceE3s: sourcePath,
      destination: destinationPath,
    });
    expect(onImported).toHaveBeenCalledWith(successfulSetup);
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("retains the backend failure and selections until a successful retry", async () => {
    const backendError = "e3s_import_failed: cannot read native compatibility catalog";
    const importProject = vi.fn()
      .mockResolvedValueOnce({ ...successfulSetup, projectOpened: false, error: backendError })
      .mockResolvedValueOnce(successfulSetup);
    const onImported = vi.fn();
    const onOpenChange = vi.fn();

    render(
      <E3sImportDialog
        open
        onOpenChange={onOpenChange}
        pickSource={vi.fn(async () => ({ path: "C:\\Legacy\\sample.e3s" }))}
        pickDestination={vi.fn(async () => ({ path: "C:\\Work\\ImportedProject", empty: true }))}
        importProject={importProject}
        onImported={onImported}
      />,
    );

    await userEvent.click(screen.getByRole("button", { name: "E3S 파일 선택" }));
    await userEvent.click(screen.getByRole("button", { name: "새 작업 폴더 선택" }));
    await userEvent.click(screen.getByRole("button", { name: "선택한 위치로 가져오기" }));

    await userEvent.click(screen.getByText("오류 상세"));
    expect(screen.getByText(backendError)).toBeVisible();
    expect(onImported).not.toHaveBeenCalled();
    expect(onOpenChange).not.toHaveBeenCalled();

    await userEvent.click(screen.getByRole("button", { name: "선택한 위치로 가져오기" }));
    expect(importProject).toHaveBeenLastCalledWith({
      sourceE3s: "C:\\Legacy\\sample.e3s",
      destination: "C:\\Work\\ImportedProject",
    });
    expect(onImported).toHaveBeenCalledWith(successfulSetup);
    expect(onOpenChange).toHaveBeenCalledWith(false);
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });
  it("reviews mixed unavailable harness items, renews consent, and never activates pending imports", async () => {
    const importIssues = [
      {
        id: "workspace-1",
        scope: "workspace" as const,
        path: "C:\\Legacy\\.eud-agent\\workspace\\plans\\first.md",
        reason: "approved plan body is unavailable",
      },
      {
        id: "memory-1",
        scope: "memory" as const,
        path: "C:\\Legacy\\.eud-agent\\memory\\meta.json",
        reason: "invalid UTF-8 in metadata",
      },
    ];
    const changedIssues = [importIssues[1]];
    const firstReview: SetupMessage = {
      ...successfulSetup,
      projectOpened: false,
      error: null,
      importIssues,
    };
    const changedReview: SetupMessage = {
      ...firstReview,
      importIssues: changedIssues,
    };
    const importProject = vi
      .fn()
      .mockResolvedValueOnce(firstReview)
      .mockResolvedValueOnce(changedReview)
      .mockResolvedValueOnce(changedReview)
      .mockResolvedValueOnce(successfulSetup);
    const onImported = vi.fn();
    const pickSource = vi.fn(async () => ({ path: "C:\\Legacy\\sample.e3s" }));

    render(
      <E3sImportDialog
        open
        onOpenChange={vi.fn()}
        pickSource={pickSource}
        pickDestination={vi.fn(async () => ({ path: "C:\\Work\\ImportedProject", empty: true }))}
        importProject={importProject}
        onImported={onImported}
      />,
    );

    await userEvent.click(screen.getByRole("button", { name: "E3S 파일 선택" }));
    await userEvent.click(screen.getByRole("button", { name: "새 작업 폴더 선택" }));
    await userEvent.click(screen.getByRole("button", { name: "선택한 위치로 가져오기" }));

    expect(screen.getByText("가져오지 못한 부가 항목 2개")).toBeInTheDocument();
    expect(screen.getByText(importIssues[0].path)).toBeInTheDocument();
    expect(screen.getByText("승인된 계획 본문을 찾을 수 없어 해당 계획을 제외했습니다.")).toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(onImported).not.toHaveBeenCalled();

    await userEvent.click(screen.getByRole("button", { name: "검토한 2개를 제외하고 가져오기" }));
    expect(importProject).toHaveBeenNthCalledWith(2, {
      sourceE3s: "C:\\Legacy\\sample.e3s",
      destination: "C:\\Work\\ImportedProject",
      excludedImportItems: ["workspace-1", "memory-1"],
    });
    expect(screen.getByText("가져오지 못한 부가 항목 1개")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "검토한 2개를 제외하고 가져오기" })).not.toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "다시 확인" }));
    expect(importProject).toHaveBeenNthCalledWith(3, {
      sourceE3s: "C:\\Legacy\\sample.e3s",
      destination: "C:\\Work\\ImportedProject",
    });
    expect(screen.getByText("가져오지 못한 부가 항목 1개")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "다른 E3S 파일 선택" }));
    expect(screen.queryByText("가져오지 못한 부가 항목 1개")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "선택한 위치로 가져오기" })).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "선택한 위치로 가져오기" }));
    expect(importProject).toHaveBeenNthCalledWith(4, {
      sourceE3s: "C:\\Legacy\\sample.e3s",
      destination: "C:\\Work\\ImportedProject",
    });
    expect(onImported).toHaveBeenCalledWith(successfulSetup);
  });
});
