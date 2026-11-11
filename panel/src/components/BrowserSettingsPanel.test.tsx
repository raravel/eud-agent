import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { BrowserSettingsPanel } from "@/components/BrowserSettingsPanel";

describe("BrowserSettingsPanel", () => {
  it("resets the login data only after the user confirms", async () => {
    const resetProfile = vi.fn().mockResolvedValue(undefined);
    render(<BrowserSettingsPanel actions={{ resetProfile }} supported />);

    expect(screen.getByText(/모든 프로젝트와 세션이 함께 쓰고/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "로그인 데이터 초기화" }));
    const dialog = await screen.findByRole("dialog", { name: "로그인 데이터를 초기화할까요?" });
    expect(resetProfile).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "취소" }));
    await waitFor(() => expect(dialog).not.toBeInTheDocument());
    expect(resetProfile).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "로그인 데이터 초기화" }));
    fireEvent.click(await screen.findByRole("button", { name: "초기화" }));
    await waitFor(() => expect(resetProfile).toHaveBeenCalledTimes(1));
    expect(await screen.findByRole("status")).toHaveTextContent("로그인 데이터를 지웠습니다");
  });

  it("shows the backend's refusal while the browser window is open", async () => {
    const resetProfile = vi
      .fn()
      .mockRejectedValue(
        "브라우저 창이 열려 있어 로그인 데이터를 지울 수 없습니다. 브라우저 창을 닫은 뒤 다시 시도하세요.",
      );
    render(<BrowserSettingsPanel actions={{ resetProfile }} supported />);

    fireEvent.click(screen.getByRole("button", { name: "로그인 데이터 초기화" }));
    fireEvent.click(await screen.findByRole("button", { name: "초기화" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "브라우저 창을 닫은 뒤 다시 시도하세요",
    );
    expect(screen.getByRole("button", { name: "로그인 데이터 초기화" })).toBeEnabled();
  });

  it("explains that the browser is Windows only and offers no reset elsewhere", () => {
    render(<BrowserSettingsPanel actions={{ resetProfile: vi.fn() }} supported={false} />);

    expect(screen.getByText("내장 브라우저는 Windows에서만 쓸 수 있습니다")).toBeInTheDocument();
    expect(screen.getByText(/브라우저 도구가 제공되지 않습니다/)).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "로그인 데이터 초기화" }),
    ).not.toBeInTheDocument();
  });
});
