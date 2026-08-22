import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { AppSettings, CodexModelSettings } from "@/lib/ipc";
import { SettingsDialog } from "./SettingsDialog";

const settings: AppSettings = {
  notifications: {
    planApproval: { sound: true, osNotification: true },
    changesetReview: { sound: true, osNotification: false },
  },
  codexLargeContextModels: [],
};
const codexSettings: CodexModelSettings = {
  models: [
    {
      model: "gpt-test",
      displayName: "Test Codex",
      description: "Test model",
      supportedReasoningEfforts: [
        { reasoningEffort: "medium", description: "Balanced" },
      ],
      defaultReasoningEffort: "medium",
      isDefault: true,
    },
  ],
  selectedModel: "gpt-test",
  selectedReasoningEffort: "medium",
};


const baseProps = {
  open: true,
  settings,
  codexSettings,
  onOpenChange: vi.fn(),
  onSettingsChange: vi.fn(),
  onReload: vi.fn(),
  onPreviewSound: vi.fn(),
  onCodexReload: vi.fn(),
};

describe("SettingsDialog", () => {
  it("renders notification channels in the general settings shell", () => {
    render(<SettingsDialog {...baseProps} />);

    expect(screen.getByRole("dialog", { name: "설정" })).toBeInTheDocument();
    expect(screen.getByRole("navigation", { name: "설정 범주" })).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "설정 닫기" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("switch", { name: "계획 승인 필요 알림음" }),
    ).toHaveAttribute("aria-checked", "true");
    expect(
      screen.getByRole("switch", { name: "변경사항 검토 필요 OS 알림" }),
    ).toHaveAttribute("aria-checked", "false");
    expect(
      screen.getByText(/창이 포커스되어 있지 않을 때만 표시됩니다/),
    ).toBeInTheDocument();
  });

  it("emits one complete settings value for an immediate-save toggle", () => {
    const onSettingsChange = vi.fn();
    render(
      <SettingsDialog
        {...baseProps}
        onSettingsChange={onSettingsChange}
      />,
    );

    fireEvent.click(
      screen.getByRole("switch", { name: "계획 승인 필요 알림음" }),
    );

    expect(onSettingsChange).toHaveBeenCalledWith({
      notifications: {
        planApproval: { sound: false, osNotification: true },
        changesetReview: { sound: true, osNotification: false },
      },
      codexLargeContextModels: [],
    });
  });
  it("persists the 1M context toggle for the selected Codex model", () => {
    const onSettingsChange = vi.fn();
    render(
      <SettingsDialog
        {...baseProps}
        onSettingsChange={onSettingsChange}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Codex" }));
    expect(screen.getByText("현재 모델")).toBeInTheDocument();
    fireEvent.click(
      screen.getByRole("switch", { name: "Test Codex 1M 컨텍스트" }),
    );

    expect(onSettingsChange).toHaveBeenCalledWith({
      notifications: settings.notifications,
      codexLargeContextModels: ["gpt-test"],
    });
  });


  it("previews the native sound and retries a failed settings load", () => {
    const onPreviewSound = vi.fn();
    const { rerender } = render(
      <SettingsDialog
        {...baseProps}
        onPreviewSound={onPreviewSound}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "소리 미리듣기" }));
    expect(onPreviewSound).toHaveBeenCalledOnce();

    const onReload = vi.fn();
    rerender(
      <SettingsDialog
        {...baseProps}
        settings={null}
        onReload={onReload}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "다시 시도" }));
    expect(onReload).toHaveBeenCalledOnce();
  });
});
