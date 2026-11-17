import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { MapTestSettings } from "@/lib/ipc";

import { MapTestSettingsPanel, type MapTestSettingsActions } from "./MapTestSettingsPanel";

const OFF: MapTestSettings = {
  supported: true,
  enabled: false,
  installed: false,
  installing: false,
  bundle: "sc4j-0.1.3+scr-v0.1.2-eud.1",
  downloadBytes: 122 * 1024 * 1024,
};

function actions(overrides: Partial<MapTestSettingsActions> = {}): MapTestSettingsActions {
  return {
    load: vi.fn(async () => OFF),
    save: vi.fn(async (enabled: boolean) => ({ ...OFF, enabled, installed: enabled })),
    remove: vi.fn(async () => OFF),
    onProgress: vi.fn(async () => () => {}),
    ...overrides,
  };
}

describe("MapTestSettingsPanel", () => {
  it("names the download size and turns the tests on", async () => {
    const calls = actions();
    render(<MapTestSettingsPanel actions={calls} disabled={false} />);

    expect(await screen.findByText(/약 122 MB/)).toBeInTheDocument();
    const toggle = screen.getByRole("switch", { name: "맵 실행 테스트 사용" });
    await waitFor(() => expect(toggle).toBeEnabled());

    fireEvent.click(toggle);

    await waitFor(() => expect(calls.save).toHaveBeenCalledWith(true));
    expect(await screen.findByText("설치됨: sc4j-0.1.3+scr-v0.1.2-eud.1")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "받은 파일 삭제" })).toBeInTheDocument();
  });

  it("shows download progress while turning on", async () => {
    let report: ((pct: number) => void) | undefined;
    let finish: ((value: MapTestSettings) => void) | undefined;
    const calls = actions({
      onProgress: vi.fn(async (listener: (pct: number) => void) => {
        report = listener;
        return () => {};
      }),
      save: vi.fn(
        () =>
          new Promise<MapTestSettings>((resolve) => {
            finish = resolve;
          }),
      ),
    });
    render(<MapTestSettingsPanel actions={calls} disabled={false} />);
    const toggle = await screen.findByRole("switch", { name: "맵 실행 테스트 사용" });
    await waitFor(() => expect(toggle).toBeEnabled());

    fireEvent.click(toggle);
    await waitFor(() => expect(calls.save).toHaveBeenCalled());
    report?.(42);

    expect(await screen.findByText("내려받는 중… 42%")).toBeInTheDocument();
    finish?.({ ...OFF, enabled: true, installed: true });
    await waitFor(() => expect(screen.queryByText("내려받는 중… 42%")).not.toBeInTheDocument());
  });

  it("reports a failed download and shows the backend state again", async () => {
    const calls = actions({
      save: vi.fn(async () => {
        throw new Error("network down");
      }),
    });
    render(<MapTestSettingsPanel actions={calls} disabled={false} />);
    const toggle = await screen.findByRole("switch", { name: "맵 실행 테스트 사용" });
    await waitFor(() => expect(toggle).toBeEnabled());

    fireEvent.click(toggle);

    expect(await screen.findByRole("alert")).toHaveTextContent("network down");
    expect(toggle).not.toBeChecked();
  });

  it("is a notice only where the platform has no runtime", async () => {
    render(
      <MapTestSettingsPanel
        actions={actions({ load: vi.fn(async () => ({ ...OFF, supported: false })) })}
        disabled={false}
      />,
    );

    expect(await screen.findByText("맵 실행 테스트는 Windows에서만 쓸 수 있습니다.")).toBeInTheDocument();
    expect(screen.getByRole("switch", { name: "맵 실행 테스트 사용" })).toBeDisabled();
  });
});
