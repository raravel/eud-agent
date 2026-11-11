import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({
  invoke: vi.fn(),
  handlers: new Map<string, (event: { payload: unknown }) => void>(),
}));

vi.mock("@/lib/transport", () => ({
  invoke: api.invoke,
  listen: vi.fn(
    async (event: string, handler: (event: { payload: unknown }) => void) => {
      api.handlers.set(event, handler);
      return () => api.handlers.delete(event);
    },
  ),
}));

import BrowserChrome from "./BrowserChrome";
import { BROWSER_STATE_EVENT, type BrowserState } from "./browserProtocol";

const state: BrowserState = {
  activeTabId: "t2",
  tabs: [
    {
      tabId: "t1",
      sessionId: "s1",
      sessionTitle: "메인 대화",
      url: "https://example.com/",
      title: "Example Domain",
      active: false,
      agentBusy: false,
      awaitingConfirmation: false,
    },
    {
      tabId: "t2",
      sessionId: "s2",
      sessionTitle: "조사",
      url: "https://docs.example.org/a",
      title: "",
      active: true,
      agentBusy: true,
      awaitingConfirmation: false,
    },
  ],
  downloads: [],
};

describe("BrowserChrome", () => {
  beforeEach(() => {
    api.invoke.mockReset();
    api.handlers.clear();
    api.invoke.mockImplementation(async (command: string) =>
      command === "browser_state" ? state : undefined,
    );
  });

  it("shows every tab with its session and the agent indicator", async () => {
    render(<BrowserChrome />);
    expect(
      await screen.findByRole("tab", { name: /Example Domain/ }),
    ).toHaveAttribute("aria-selected", "false");
    const active = screen.getByRole("tab", { name: /docs\.example\.org/ });
    expect(active).toHaveAttribute("aria-selected", "true");
    expect(
      screen.getByRole("tab", { name: /Example Domain.*세션 메인 대화/ }),
    ).toBeInTheDocument();
    expect(active).toHaveAccessibleName(/세션 조사/);
    // The active tab's session is named beside the address bar.
    expect(screen.getByText("조사")).toBeInTheDocument();
    expect(screen.getByText("에이전트 조작 중")).toBeInTheDocument();
    await waitFor(() =>
      expect(screen.getByRole("textbox", { name: "주소" })).toHaveValue(
        "https://docs.example.org/a",
      ),
    );
  });

  it("activates, closes and navigates the chosen tab", async () => {
    const user = userEvent.setup();
    render(<BrowserChrome />);
    await user.click(
      await screen.findByRole("tab", { name: /Example Domain/ }),
    );
    expect(api.invoke).toHaveBeenCalledWith("browser_activate", {
      tabId: "t1",
    });

    await user.click(
      screen.getByRole("button", { name: "Example Domain 탭 닫기" }),
    );
    expect(api.invoke).toHaveBeenCalledWith("browser_tab_close", {
      tabId: "t1",
    });

    await user.click(screen.getByRole("button", { name: "뒤로" }));
    expect(api.invoke).toHaveBeenCalledWith("browser_user_navigate", {
      tabId: "t2",
      action: "back",
    });

    const address = screen.getByRole("textbox", { name: "주소" });
    await user.clear(address);
    await user.type(address, "https://example.net{Enter}");
    expect(api.invoke).toHaveBeenCalledWith("browser_user_navigate", {
      tabId: "t2",
      url: "https://example.net",
    });
  });

  it("marks a tab whose agent action waits for the user's confirmation", async () => {
    render(<BrowserChrome />);
    await screen.findByRole("tab", { name: /Example Domain/ });
    await waitFor(() =>
      expect(api.handlers.has(BROWSER_STATE_EVENT)).toBe(true),
    );
    expect(screen.queryByText("확인 대기")).not.toBeInTheDocument();
    act(() => {
      api.handlers.get(BROWSER_STATE_EVENT)?.({
        payload: {
          ...state,
          tabs: state.tabs.map((tab) =>
            tab.tabId === "t2" ? { ...tab, awaitingConfirmation: true } : tab,
          ),
        },
      });
    });
    expect(await screen.findByText("확인 대기")).toBeInTheDocument();
    expect(screen.getByLabelText("확인 대기")).toBeInTheDocument();
  });

  it("follows state events and shows a refused command", async () => {
    render(<BrowserChrome />);
    await screen.findByRole("tab", { name: /Example Domain/ });
    await waitFor(() =>
      expect(api.handlers.has(BROWSER_STATE_EVENT)).toBe(true),
    );
    act(() => {
      api.handlers.get(BROWSER_STATE_EVENT)?.({
        payload: { tabs: [], activeTabId: null, downloads: [] },
      });
    });
    expect(await screen.findByText("열린 탭이 없습니다.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "새로고침" })).toBeDisabled();

    act(() => {
      api.handlers.get(BROWSER_STATE_EVENT)?.({ payload: state });
    });
    api.invoke.mockRejectedValueOnce(
      "탭을 찾을 수 없습니다. 이미 닫힌 탭입니다.",
    );
    await userEvent
      .setup()
      .click(await screen.findByRole("button", { name: "새로고침" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "탭을 찾을 수 없습니다",
    );
  });

  it("marks a tab's download and lists every download with its size", async () => {
    const user = userEvent.setup();
    render(<BrowserChrome />);
    await screen.findByRole("tab", { name: /Example Domain/ });
    await waitFor(() =>
      expect(api.handlers.has(BROWSER_STATE_EVENT)).toBe(true),
    );
    expect(
      screen.getByRole("button", { name: "다운로드 목록 (0개)" }),
    ).toBeInTheDocument();
    act(() => {
      api.handlers.get(BROWSER_STATE_EVENT)?.({
        payload: {
          ...state,
          // A running agent call would take the tab's icon slot.
          tabs: state.tabs.map((tab) => ({ ...tab, agentBusy: false })),
          downloads: [
            {
              downloadId: "d2",
              tabId: "t2",
              sessionId: "s2",
              sessionTitle: "조사",
              fileName: "효과음.ogg",
              bytes: 0,
              state: "inProgress",
              error: null,
            },
            {
              downloadId: "d1",
              tabId: "t1",
              sessionId: "s1",
              sessionTitle: "메인 대화",
              fileName: "hit.ogg",
              bytes: 2_621_440,
              state: "completed",
              error: null,
            },
          ],
        },
      });
    });
    expect(
      await screen.findByRole("img", { name: "다운로드됨: hit.ogg" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("img", { name: "다운로드 중: 효과음.ogg" }),
    ).toBeInTheDocument();

    await user.click(
      screen.getByRole("button", { name: "다운로드 목록 (2개)" }),
    );
    expect(api.invoke).toHaveBeenCalledWith("browser_chrome_expand", {
      expanded: true,
    });
    const list = await screen.findByRole("list", { name: "받은 파일" });
    expect(list).toHaveTextContent("hit.ogg");
    expect(list).toHaveTextContent("2.5 MB · 메인 대화");
    expect(list).toHaveTextContent("받는 중 · 조사");

    await user.keyboard("{Escape}");
    await waitFor(() =>
      expect(api.invoke).toHaveBeenCalledWith("browser_chrome_expand", {
        expanded: false,
      }),
    );
  });
});
