/**
 * 빌드 결과 — the dialog the header's "프로젝트 빌드" button opens.
 *
 * rules.md (## Build generation): warnings never fail a build, so a successful
 * build that warns still says 성공; errors carry their file/line; and a build
 * that never ran shows its recovery action instead of a verdict.
 */
import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { BuildResultDialog } from "@/components/BuildResultDialog";
import type { ProjectBuildReport } from "@/lib/ipc";

function report(overrides: Partial<ProjectBuildReport> = {}): ProjectBuildReport {
  return {
    ok: true,
    errors: [],
    warnings: [],
    rawStatus: 0,
    outputExcerpt: "euddraft 0.9.9",
    outputMap: "build/[EUD]project.scx",
    deployedMap: null,
    logPath: null,
    ...overrides,
  };
}

describe("BuildResultDialog", () => {
  it("reports a successful build with its output map and deployed copy", () => {
    render(
      <BuildResultDialog
        open={true}
        report={report({
          deployedMap: "C:\\StarCraft\\Maps\\eud-agent\\out.scx",
          logPath: "C:\\Work\\map\\build\\euddraft\\build.log",
        })}
        onOpenChange={vi.fn()}
      />,
    );
    expect(screen.getByText("성공")).toBeInTheDocument();
    expect(screen.getByText("build/[EUD]project.scx")).toBeInTheDocument();
    expect(
      screen.getByText("C:\\StarCraft\\Maps\\eud-agent\\out.scx"),
    ).toBeInTheDocument();
    expect(
      screen.getByText("C:\\Work\\map\\build\\euddraft\\build.log"),
    ).toBeInTheDocument();
  });

  it("stays 성공 when euddraft only warned, and still lists the warning", () => {
    render(
      <BuildResultDialog
        open={true}
        report={report({
          warnings: [
            {
              source: "warning",
              file: "eudplib/utils/eperror.py",
              line: 41,
              message: "[Warning] Input map has 0000.00 null tiles",
              count: 3,
            },
          ],
        })}
        onOpenChange={vi.fn()}
      />,
    );
    expect(screen.getByText("성공")).toBeInTheDocument();
    expect(screen.getByText("경고 1개")).toBeInTheDocument();
    expect(screen.getByText("eudplib/utils/eperror.py:41")).toBeInTheDocument();
    expect(screen.getByText("×3")).toBeInTheDocument();
  });

  it("shows each error at its project file and line", () => {
    render(
      <BuildResultDialog
        open={true}
        report={report({
          ok: false,
          rawStatus: 1,
          errors: [
            {
              source: "epscript",
              file: "src/main.eps",
              line: 12,
              message: 'Module "main" Line 12 : unknown name foo',
              raw: '[Error 1] Module "main" Line 12 : unknown name foo',
              count: 1,
            },
          ],
        })}
        onOpenChange={vi.fn()}
      />,
    );
    expect(screen.getByText("실패")).toBeInTheDocument();
    expect(screen.getByText("오류 1개")).toBeInTheDocument();
    expect(screen.getByText("src/main.eps:12")).toBeInTheDocument();
  });

  it("says why a build never ran instead of showing a verdict", () => {
    render(
      <BuildResultDialog
        open={true}
        report={null}
        error={'이미 빌드가 진행 중입니다. 끝난 뒤 다시 "프로젝트 빌드"를 눌러 주세요.'}
        onOpenChange={vi.fn()}
      />,
    );
    expect(screen.getByRole("alert").textContent).toContain(
      "이미 빌드가 진행 중입니다",
    );
    expect(screen.queryByText("성공")).not.toBeInTheDocument();
    expect(screen.queryByText("실패")).not.toBeInTheDocument();
  });

  it("closes on 닫기", async () => {
    const onOpenChange = vi.fn();
    render(
      <BuildResultDialog open={true} report={report()} onOpenChange={onOpenChange} />,
    );
    await userEvent.click(screen.getByRole("button", { name: "닫기" }));
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });
});
