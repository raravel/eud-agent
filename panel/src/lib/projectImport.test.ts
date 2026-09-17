import { describe, expect, it, vi } from "vitest";

import {
  formatImportIssueReason,
  importE3sProject,
  pickE3sImportDestination,
  pickE3sSource,
  ProjectImportProtocolError,
} from "@/lib/projectImport";
import { PROVIDER_IDS, type ProviderStatus } from "@/providers/types";

const providers: ProviderStatus[] = PROVIDER_IDS.map((provider) => ({
  provider,
  availability: "unavailable",
  selectedAsDefault: false,
  canInstall: false,
  canImport: false,
  experimental: false,
}));

describe("E3S import IPC", () => {
  it("uses separate commands for the source file and destination folder", async () => {
    const invoke = vi
      .fn()
      .mockResolvedValueOnce({ path: "C:\\Legacy\\sample.e3s" })
      .mockResolvedValueOnce({ path: "C:\\Work\\ImportedProject", empty: true });

    await expect(pickE3sSource(invoke)).resolves.toEqual({
      path: "C:\\Legacy\\sample.e3s",
    });
    await expect(pickE3sImportDestination(invoke)).resolves.toEqual({
      path: "C:\\Work\\ImportedProject",
      empty: true,
    });
    expect(invoke).toHaveBeenNthCalledWith(1, "setup_pick_e3s_source");
    expect(invoke).toHaveBeenNthCalledWith(2, "setup_pick_import_destination");
  });

  it("summarizes technical issue reasons in Korean while retaining a generic fallback", () => {
    expect(formatImportIssueReason("approved plan body is unavailable")).toContain("승인된 계획 본문");
    expect(formatImportIssueReason("invalid UTF-8 in metadata")).toContain("문서 인코딩");
    expect(formatImportIssueReason("unexpected decoder failure")).toContain("안전하게 가져오지 못해");
    expect(formatImportIssueReason("같은 프로젝트 키로 저장된 대화가 있어 제외했습니다.")).toContain("같은 프로젝트 키");
  });


  it("rejects malformed harness import issue payloads", async () => {
    const response = {
      projectPath: "C:\\Work\\ImportedProject",
      projectValid: false,
      euddraftPath: "",
      euddraftValid: false,
      assetsReady: false,
      providers,
      projectOpened: false,
      setupRequired: true,
      error: null,
      importIssues: [
        {
          id: "workspace-1",
          scope: "workspace",
          path: "plans/first.md",
          reason: "",
        },
      ],
    };

    await expect(
      importE3sProject(
        {
          sourceE3s: "C:\\Legacy\\sample.e3s",
          destination: "C:\\Work\\ImportedProject",
        },
        vi.fn(async () => response),
      ),
    ).rejects.toBeInstanceOf(ProjectImportProtocolError);
  });
});
