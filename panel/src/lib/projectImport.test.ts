import { describe, expect, it, vi } from "vitest";

import {
  cancelE2Import,
  decodeE2ImportResult,
  decodeE2PreflightResult,
  formatImportIssueReason,
  importE2Project,
  importE3sProject,
  isE2ImportResultFor,
  pickE2Relink,
  pickE2Source,
  pickE3sImportDestination,
  pickE3sSource,
  preflightE2Project,
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

describe("E2 preflight relink import transport", () => {
  const need = {
    id: "input-map",
    kind: "inputMap" as const,
    label: "Input map",
    originalPath: "C:\\Legacy\\base.scx",
    candidates: [],
    extensions: ["scx", "scm"],
  };
  const relinks = [{ needId: need.id, path: "C:\\Maps\\base.scx" }];
  const fingerprint = "f".repeat(64);

  it("uses the five E2 commands and preserves the typed state sequence", async () => {
    const invoke = vi
      .fn()
      .mockResolvedValueOnce("C:\\Legacy\\sample.e2s")
      .mockResolvedValueOnce("C:\\Maps\\base.scx")
      .mockResolvedValueOnce({
        state: "needsRelink",
        fingerprint,
        needs: [need],
      })
      .mockResolvedValueOnce({
        state: "ready",
        fingerprint,
        sourceKind: "e2s",
        features: ["projectMap", "dat"],
        warnings: [{
          code: "invalid_resource",
          severity: "warning",
          message: "a resource will be copied without conversion",
          feature: "dat",
          path: "C:\\Legacy\\data.dat",
        }],
      })
      .mockResolvedValueOnce({
        state: "completed",
        importId: "import-1",
        fingerprint,
        projectOpened: true,
        projectPath: "C:\\Projects\\Imported",
      });

    await expect(pickE2Source(invoke)).resolves.toBe("C:\\Legacy\\sample.e2s");
    await expect(pickE2Relink({ need }, invoke)).resolves.toBe(
      "C:\\Maps\\base.scx",
    );
    await expect(
      preflightE2Project(
        { sourcePath: "C:\\Legacy\\sample.e2s", destinationPath: "C:\\Projects", relinks: [] },
        invoke,
      ),
    ).resolves.toMatchObject({ state: "needsRelink", fingerprint });
    await expect(
      preflightE2Project(
        {
          sourcePath: "C:\\Legacy\\sample.e2s",
          destinationPath: "C:\\Projects",
          relinks,
        },
        invoke,
      ),
    ).resolves.toEqual({
      state: "ready",
      fingerprint,
      sourceKind: "e2s",
      features: ["projectMap", "dat"],
      warnings: [{
        code: "invalid_resource",
        severity: "warning",
        message: "a resource will be copied without conversion",
        feature: "dat",
        path: "C:\\Legacy\\data.dat",
      }],
    });
    await expect(
      importE2Project(
        {
          sourcePath: "C:\\Legacy\\sample.e2s",
          destinationPath: "C:\\Projects",
          fingerprint,
          relinks,
        },
        invoke,
      ),
    ).resolves.toMatchObject({ state: "completed", projectOpened: true });

    expect(invoke).toHaveBeenNthCalledWith(1, "setup_pick_e2_source");
    expect(invoke).toHaveBeenNthCalledWith(2, "setup_pick_e2_relink", {
      request: { need },
    });
    expect(invoke).toHaveBeenNthCalledWith(3, "setup_e2_preflight", {
      request: {
        sourcePath: "C:\\Legacy\\sample.e2s",
        destinationPath: "C:\\Projects",
        relinks: [],
      },
    });
    expect(invoke).toHaveBeenNthCalledWith(4, "setup_e2_preflight", {
      request: {
        sourcePath: "C:\\Legacy\\sample.e2s",
        destinationPath: "C:\\Projects",
        relinks,
      },
    });
    expect(invoke).toHaveBeenNthCalledWith(5, "setup_import_e2", {
      request: {
        sourcePath: "C:\\Legacy\\sample.e2s",
        destinationPath: "C:\\Projects",
        fingerprint,
        relinks,
      },
    });
  });

  it("rejects malformed E2 states and never reports a false success", async () => {
    const malformed = [
      { state: "needsRelink", fingerprint: "", needs: [need] },
      { state: "unknown", fingerprint },
      {
        state: "progress",
        importId: "import-1",
        fingerprint,
        phase: "opening",
        completed: 2,
        total: 1,
        message: "opening",
      },
      {
        state: "completed",
        importId: "import-1",
        fingerprint,
        projectOpened: false,
        projectPath: "C:\\Projects\\Imported",
      },
      {
        state: "needsRelink",
        fingerprint,
        needs: [{ ...need, kind: "other" }],
      },
    ];
    for (const response of malformed) {
      expect(() => decodeE2PreflightResult(response)).toThrow(ProjectImportProtocolError);
      expect(() => decodeE2ImportResult(response)).toThrow(ProjectImportProtocolError);
    }
  });

  it("keeps cancelled and fatal results typed and requires a typed issue", async () => {
    const invoke = vi
      .fn()
      .mockResolvedValueOnce({ state: "cancelled", importId: "import-1", fingerprint })
      .mockResolvedValueOnce({
        state: "fatal",
        issues: [{ code: "malformed_input", severity: "fatal", message: "bad input" }],
      });
    await expect(cancelE2Import({ importId: "import-1" }, invoke)).resolves.toMatchObject({
      state: "cancelled",
    });
    await expect(
      importE2Project(
        {
          sourcePath: "source.e2s",
          destinationPath: "destination",
          fingerprint,
          relinks: [],
        },
        invoke,
      ),
    ).resolves.toMatchObject({ state: "fatal" });
    expect(() => decodeE2ImportResult({
      state: "fatal",
      issues: [{ code: "malformed_input", severity: "fatal", message: "" }],
    })).toThrow(ProjectImportProtocolError);
  });

  it("matches terminal events only for the active import identity", () => {
    const active = { importId: "import-1", fingerprint };

    expect(
      isE2ImportResultFor(
        {
          state: "completed",
          importId: "import-1",
          fingerprint,
          projectOpened: true,
          projectPath: "C:\\Projects\\Imported",
        },
        active,
      ),
    ).toBe(true);
    expect(
      isE2ImportResultFor(
        { state: "cancelled", importId: "other-import", fingerprint },
        active,
      ),
    ).toBe(false);
    expect(
      isE2ImportResultFor(
        { state: "cancelled", importId: "import-1", fingerprint: "stale-fingerprint" },
        active,
      ),
    ).toBe(false);
  });
});
