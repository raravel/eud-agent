/**
 * Tests for the self-update notice banner.
 *
 * The banner shows the available version + notes, defers via [나중에], and on
 * [지금 업데이트] streams download progress then relaunches. Download/install/relaunch
 * are injected (UpdateHandle + relaunch prop), so the flow is verified headless.
 */
import { describe, it, expect, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { UpdateNotice } from "@/components/UpdateNotice";
import type { UpdateHandle } from "@/setup/update";

function makeHandle(overrides?: Partial<UpdateHandle>): UpdateHandle {
  return {
    version: "0.1.1",
    currentVersion: "0.1.0",
    notes: "버그 수정",
    downloadAndInstall: async (onProgress) => {
      onProgress({ downloaded: 50, total: 100 });
      onProgress({ downloaded: 100, total: 100 });
    },
    ...overrides,
  };
}

describe("UpdateNotice", () => {
  it("shows the available version, current version, and notes", () => {
    render(
      <UpdateNotice
        update={makeHandle()}
        relaunch={vi.fn().mockResolvedValue(undefined)}
        onLater={vi.fn()}
      />,
    );

    expect(screen.getByRole("status", { name: "업데이트 알림" })).toBeInTheDocument();
    expect(screen.getByText(/새 버전 0\.1\.1/)).toBeInTheDocument();
    expect(screen.getByText(/현재 0\.1\.0/)).toBeInTheDocument();
    expect(screen.getByText("버그 수정")).toBeInTheDocument();
  });

  it("calls onLater when deferred", async () => {
    const onLater = vi.fn();
    render(
      <UpdateNotice
        update={makeHandle()}
        relaunch={vi.fn().mockResolvedValue(undefined)}
        onLater={onLater}
      />,
    );

    await userEvent.click(screen.getByRole("button", { name: "나중에" }));
    expect(onLater).toHaveBeenCalledOnce();
  });

  it("downloads then relaunches on consent, showing progress", async () => {
    const downloadAndInstall = vi.fn(
      async (onProgress: (p: { downloaded: number; total: number | null }) => void) => {
        onProgress({ downloaded: 100, total: 100 });
      },
    );
    const relaunch = vi.fn().mockResolvedValue(undefined);
    render(
      <UpdateNotice
        update={makeHandle({ downloadAndInstall })}
        relaunch={relaunch}
        onLater={vi.fn()}
      />,
    );

    await userEvent.click(screen.getByRole("button", { name: "지금 업데이트" }));

    expect(downloadAndInstall).toHaveBeenCalledOnce();
    await waitFor(() => expect(relaunch).toHaveBeenCalledOnce());
    expect(
      screen.getByRole("progressbar", { name: "업데이트 다운로드 진행률" }),
    ).toBeInTheDocument();
  });

  it("surfaces a download failure with a retry", async () => {
    const downloadAndInstall = vi.fn().mockRejectedValue(new Error("network down"));
    render(
      <UpdateNotice
        update={makeHandle({ downloadAndInstall })}
        relaunch={vi.fn().mockResolvedValue(undefined)}
        onLater={vi.fn()}
      />,
    );

    await userEvent.click(screen.getByRole("button", { name: "지금 업데이트" }));

    await waitFor(() =>
      expect(screen.getByText(/업데이트에 실패했습니다/)).toBeInTheDocument(),
    );
    expect(screen.getByText(/network down/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "다시 시도" })).toBeInTheDocument();
  });
});
