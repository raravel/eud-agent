import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const tauri = vi.hoisted(() => ({
  invoke: vi.fn(),
  listeners: new Map<string, (event: { payload: unknown }) => void>(),
  resolveLongChat: undefined as (() => void) | undefined,
  pendingAsk: undefined as Record<string, unknown> | undefined,
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke: tauri.invoke }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (name: string, handler: (event: { payload: unknown }) => void) => {
    tauri.listeners.set(name, handler);
    return () => tauri.listeners.delete(name);
  }),
}));
vi.mock("@tauri-apps/plugin-updater", () => ({ check: vi.fn(async () => null) }));
vi.mock("@tauri-apps/plugin-process", () => ({ relaunch: vi.fn(async () => undefined) }));

import App from "./App";

const sessionRecords = [
  {
    id: "session-a",
    name: "Session A",
    project: "Project",
    createdAt: 1,
    lastConversationAt: 2_000,
    threadId: null,
    pendingRequestIds: [],
    panelLog: {
      schemaVersion: 2,
      logSeq: 1,
      log: [{ id: 1, kind: "you", text: "previous conversation" }],
    },
  },
  {
    id: "session-b",
    name: "Session B",
    project: "Project",
    createdAt: 1,
    lastConversationAt: 1_000,
    threadId: null,
    pendingRequestIds: [],
    panelLog: { schemaVersion: 2, logSeq: 0, log: [] },
  },
];

function emit(name: string, payload: unknown): void {
  const listener = tauri.listeners.get(name);
  if (!listener) throw new Error(`listener ${name} is not registered`);
  listener({ payload });
}

function sessionOrder(): string[] {
  const navigation = screen.getByRole("navigation", {
    name: "현재 프로젝트 세션",
  });
  return Array.from(
    navigation.querySelectorAll<HTMLButtonElement>("li > button"),
    (button) => button.getAttribute("aria-label") ?? "",
  );
}

beforeEach(() => {
  tauri.listeners.clear();
  tauri.resolveLongChat = undefined;
  tauri.pendingAsk = undefined;
  tauri.invoke.mockReset();
  tauri.invoke.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
    switch (command) {
      case "setup_status":
        return {
          editor_path: "C:/Editor",
          editor_valid: true,
          assets_ready: true,
          codex_resolved: true,
          codex_authed: true,
          setup_required: false,
        };
      case "status":
        return { compiling: false, project: "Project" };
      case "list":
        return { files: [] };
      case "session_list":
        return sessionRecords.map(
          ({ id, name, project, createdAt, lastConversationAt }) => ({
            id,
            name,
            project,
            createdAt,
            lastConversationAt,
          }),
        );
      case "session_load":
        return sessionRecords.find((record) => record.id === args?.id);
      case "session_update_log":
        return undefined;
      case "app_settings":
        return {
          notifications: {
            planApproval: { sound: true, osNotification: true },
            changesetReview: { sound: true, osNotification: true },
          },
          codexLargeContextModels: [],
        };
      case "app_settings_save":
        return args?.settings;
      case "attention_notify":
      case "notification_sound_preview":
        return undefined;
      case "codex_model_settings":
        return {
          models: [
            {
              model: "gpt-test",
              displayName: "Test",
              description: "",
              supportedReasoningEfforts: [
                { reasoningEffort: "medium", description: "" },
              ],
              defaultReasoningEffort: "medium",
              isDefault: true,
            },
          ],
          selectedModel: "gpt-test",
          selectedReasoningEffort: "medium",
        };
      case "ask_pending":
        return args?.sessionId === "session-a" ? (tauri.pendingAsk ?? null) : null;
      case "chat":
        if (args?.sessionId === "session-a") {
          return new Promise<void>((resolve) => {
            tauri.resolveLongChat = resolve;
          });
        }
        return undefined;
      default:
        return undefined;
    }
  });
});

afterEach(() => {
  tauri.resolveLongChat?.();
  vi.clearAllMocks();
});

describe("App concurrent sessions", () => {
  it("invokes session B before session A's unresolved chat completes", async () => {
    render(<App />);
    const input = await screen.findByRole("textbox", { name: "지시 입력" });
    await waitFor(() => expect(input).toBeEnabled());

    fireEvent.change(input, { target: { value: "long analysis" } });
    fireEvent.click(screen.getByRole("button", { name: "전송" }));
    await waitFor(() => {
      expect(tauri.invoke).toHaveBeenCalledWith("chat", {
        sessionId: "session-a",
        text: "long analysis",
        attachments: [],
      });
    });
    expect(tauri.resolveLongChat).toBeTypeOf("function");

    fireEvent.click(screen.getByRole("button", { name: "Session B, 유휴" }));
    const secondInput = screen.getByRole("textbox", { name: "지시 입력" });
    fireEvent.change(secondInput, { target: { value: "short answer" } });
    fireEvent.click(screen.getByRole("button", { name: "전송" }));

    await waitFor(() => {
      expect(tauri.invoke).toHaveBeenCalledWith("chat", {
        sessionId: "session-b",
        text: "short answer",
        attachments: [],
      });
    });
    expect(tauri.resolveLongChat).toBeTypeOf("function");
  });

  it("moves the session with a newly sent chat to the top", async () => {
    render(<App />);
    const input = await screen.findByRole("textbox", { name: "지시 입력" });
    await waitFor(() => expect(input).toBeEnabled());
    expect(sessionOrder()).toEqual(["Session A, 유휴", "Session B, 유휴"]);

    fireEvent.click(screen.getByRole("button", { name: "Session B, 유휴" }));
    fireEvent.change(screen.getByRole("textbox", { name: "지시 입력" }), {
      target: { value: "newest conversation" },
    });
    fireEvent.click(screen.getByRole("button", { name: "전송" }));

    await waitFor(() => {
      expect(tauri.invoke).toHaveBeenCalledWith("chat", {
        sessionId: "session-b",
        text: "newest conversation",
        attachments: [],
      });
    });
    expect(sessionOrder()).toEqual(["Session B, 유휴", "Session A, 유휴"]);
  });

  it("does not autosave conversation logs for project-state refreshes", async () => {
    render(<App />);
    await screen.findByRole("button", { name: "Session A, 유휴" });
    await waitFor(() => expect(tauri.listeners.has("status")).toBe(true));
    tauri.invoke.mockClear();

    act(() => {
      emit("status", { compiling: false, project: "Project" });
    });
    const delay = Promise.withResolvers<void>();
    window.setTimeout(delay.resolve, 550);
    await act(async () => {
      await delay.promise;
    });

    expect(
      tauri.invoke.mock.calls.filter(
        ([command]) => command === "session_update_log",
      ),
    ).toHaveLength(0);
  });

  it("routes interleaved events and backend activities only to addressed sessions", async () => {
    render(<App />);
    await screen.findByRole("button", { name: "Session A, 유휴" });
    await waitFor(() => expect(tauri.listeners.has("session_activity")).toBe(true));

    act(() => {
      emit("answer", { sessionId: "session-b", text: "B-only answer" });
      emit("session_activity", {
        sessionId: "session-a",
        activity: "running_read",
      });
      emit("session_activity", {
        sessionId: "session-b",
        activity: "running_write",
      });
    });

    expect(screen.getByRole("button", { name: "Session A, 분석 중" })).toBeInTheDocument();
    const writingRow = screen.getByRole("button", {
      name: "Session B, 변경 중",
    });
    expect(writingRow).toBeInTheDocument();
    expect(screen.queryByText("B-only answer")).not.toBeInTheDocument();

    fireEvent.click(writingRow);
    expect(await screen.findByText("B-only answer")).toBeInTheDocument();
    expect(
      tauri.invoke.mock.calls.filter(([command]) => command === "session_open"),
    ).toHaveLength(0);
  });

  it("keeps context usage isolated to its addressed session", async () => {
    render(<App />);
    await screen.findByRole("button", { name: "Session A, 유휴" });
    await waitFor(() => expect(tauri.listeners.has("context_usage")).toBe(true));

    act(() => {
      emit("context_usage", {
        sessionId: "session-b",
        turnId: "turn-b",
        tokenUsage: {
          last: {
            inputTokens: 31_000,
            cachedInputTokens: 24_000,
            cacheWriteInputTokens: 0,
            outputTokens: 1_200,
            reasoningOutputTokens: 800,
            totalTokens: 32_200,
          },
          total: {
            inputTokens: 52_000,
            cachedInputTokens: 40_000,
            cacheWriteInputTokens: 600,
            outputTokens: 2_100,
            reasoningOutputTokens: 1_300,
            totalTokens: 54_100,
          },
          modelContextWindow: 128_000,
        },
      });
    });

    expect(
      screen.queryByRole("button", { name: /컨텍스트 .* 사용/ }),
    ).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Session B, 유휴" }));

    expect(
      screen.getByRole("button", { name: /컨텍스트 .* 사용/ }),
    ).toBeInTheDocument();
  });

  it("submits ASK answers to the blocked session without opening a new turn", async () => {
    render(<App />);
    const input = await screen.findByRole("textbox", { name: "지시 입력" });
    await waitFor(() => expect(input).toBeEnabled());

    fireEvent.change(input, { target: { value: "설계를 진행해 줘" } });
    fireEvent.click(screen.getByRole("button", { name: "전송" }));
    await waitFor(() => expect(tauri.resolveLongChat).toBeTypeOf("function"));
    await waitFor(() => expect(tauri.listeners.has("ask")).toBe(true));

    act(() => {
      emit("session_activity", {
        sessionId: "session-a",
        activity: "waiting_input",
      });
      emit("ask", {
        sessionId: "session-a",
        requestId: "ask-1",
        questions: [
          {
            id: "mode",
            question: "방식을 고르세요.",
            multi: false,
            options: [{ label: "빠르게" }, { label: "세밀하게" }],
          },
        ],
      });
    });

    expect(
      screen.getByRole("button", { name: "Session A, 응답 필요" }),
    ).toBeInTheDocument();
    fireEvent.click(screen.getByLabelText("빠르게"));
    fireEvent.click(screen.getByRole("button", { name: "답변 전달" }));

    await waitFor(() => {
      expect(tauri.invoke).toHaveBeenCalledWith("ask_response", {
        sessionId: "session-a",
        requestId: "ask-1",
        answers: { mode: { answers: ["빠르게"] } },
      });
    });
    expect(screen.queryByRole("region", { name: "AI 질문" })).not.toBeInTheDocument();
  });

  it("restores a pending ASK that was emitted before the panel could display it", async () => {
    tauri.pendingAsk = {
      sessionId: "session-a",
      requestId: "ask-recovered",
      questions: [
        {
          id: "mode",
          question: "복구된 질문입니다.",
          multi: false,
          options: [{ label: "계속" }, { label: "중단" }],
        },
      ],
    };

    render(<App />);

    expect(
      await screen.findByRole("button", { name: "Session A, 응답 필요" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("region", { name: "AI 질문" }),
    ).toHaveTextContent("복구된 질문입니다.");
  });

  it("preserves a collapsed plan after switching session tabs", async () => {
    render(<App />);
    await screen.findByRole("button", { name: "Session A, 유휴" });
    await waitFor(() => expect(tauri.listeners.has("plan")).toBe(true));

    act(() => {
      emit("plan", {
        sessionId: "session-a",
        markdown: "# 계획\n\n세션 전환 뒤에도 접힌 상태여야 합니다.",
        revision: 1,
      });
    });
    expect(
      await screen.findByText("세션 전환 뒤에도 접힌 상태여야 합니다."),
    ).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "계획안 접기" }));
    expect(
      screen.queryByText("세션 전환 뒤에도 접힌 상태여야 합니다."),
    ).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Session B, 유휴" }));
    fireEvent.click(screen.getByRole("button", { name: "Session A, 유휴" }));

    expect(
      screen.queryByText("세션 전환 뒤에도 접힌 상태여야 합니다."),
    ).not.toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "계획안 펼치기" }),
    ).toBeInTheDocument();
  });

  it("keeps another session writable while the first session is in review", async () => {
    render(<App />);
    await screen.findByRole("button", { name: "Session A, 유휴" });
    await waitFor(() => expect(tauri.listeners.has("session_activity")).toBe(true));

    act(() => {
      emit("session_activity", {
        sessionId: "session-a",
        activity: "review",
      });
      emit("session_activity", {
        sessionId: "session-b",
        activity: "running_write",
      });
    });

    expect(
      screen.getByRole("button", { name: "Session A, 검토 필요" }),
    ).toBeInTheDocument();
    const writingRow = screen.getByRole("button", {
      name: "Session B, 변경 중",
    });
    expect(writingRow).toBeInTheDocument();

    fireEvent.click(writingRow);
    expect(
      await screen.findByText("격리 워크스페이스에서 변경을 시작합니다."),
    ).toBeInTheDocument();
  });

  it("surfaces an accept conflict instead of logging success", async () => {
    render(<App />);
    await screen.findByRole("button", { name: "Session A, 유휴" });
    await waitFor(() => expect(tauri.listeners.has("changeset")).toBe(true));

    act(() => {
      emit("changeset", {
        sessionId: "session-a",
        request_id: "req-conflict",
        items: [
          {
            category: "file",
            id: "workspace-1",
            seq: 1,
            path: "specs/game.md",
            diff: "@@ -1 +1 @@\n-old\n+new",
          },
        ],
      });
    });
    fireEvent.click(await screen.findByRole("button", { name: "적용 유지" }));
    await waitFor(() =>
      expect(tauri.invoke).toHaveBeenCalledWith("changeset_decision", {
        sessionId: "session-a",
        decision: "accept",
        ids: ["workspace-1"],
      }),
    );

    act(() => {
      emit("rollback_result", {
        sessionId: "session-a",
        ids: ["workspace-1"],
        ok: false,
        error: "ConcurrentWriteConflict: `specs/game.md` changed",
      });
    });

    expect(
      await screen.findByText(
        "적용 실패: ConcurrentWriteConflict: `specs/game.md` changed",
      ),
    ).toBeInTheDocument();
  });
});

describe("App native compaction", () => {
  it("routes an exact /compact command without starting a chat turn", async () => {
    render(<App />);
    const input = await screen.findByRole("textbox", { name: "지시 입력" });
    await waitFor(() => expect(input).toBeEnabled());

    fireEvent.change(input, { target: { value: "/compact" } });
    fireEvent.click(screen.getByRole("button", { name: "전송" }));

    await waitFor(() => {
      expect(tauri.invoke).toHaveBeenCalledWith("compact", {
        sessionId: "session-a",
      });
    });
    expect(
      await screen.findByText("대화 컨텍스트를 압축했습니다."),
    ).toBeInTheDocument();
    expect(tauri.invoke).not.toHaveBeenCalledWith(
      "chat",
      expect.objectContaining({ text: "/compact" }),
    );
  });
});

describe("App notifications", () => {
  it("opens general settings and persists event channel toggles", async () => {
    render(<App />);
    const settingsButton = await screen.findByRole("button", {
      name: "설정 열기",
    });
    fireEvent.click(settingsButton);

    expect(await screen.findByRole("dialog", { name: "설정" })).toBeInTheDocument();
    fireEvent.click(
      screen.getByRole("switch", { name: "계획 승인 필요 알림음" }),
    );

    await waitFor(() => {
      expect(tauri.invoke).toHaveBeenCalledWith("app_settings_save", {
        settings: {
          notifications: {
            planApproval: { sound: false, osNotification: true },
            changesetReview: { sound: true, osNotification: true },
          },
          codexLargeContextModels: [],
        },
      });
    });

    fireEvent.click(screen.getByRole("button", { name: "소리 미리듣기" }));
    await waitFor(() =>
      expect(tauri.invoke).toHaveBeenCalledWith("notification_sound_preview"),
    );
  });

  it("notifies once when each new review surface appears", async () => {
    const focus = vi.spyOn(document, "hasFocus").mockReturnValue(false);
    render(<App />);
    await screen.findByRole("button", { name: "Session A, 유휴" });
    await waitFor(() => {
      expect(tauri.listeners.has("plan")).toBe(true);
      expect(tauri.listeners.has("changeset")).toBe(true);
    });

    const item = {
      category: "file",
      id: "file-1",
      seq: 1,
      path: "main.eps",
      diff: "@@ -1 +1 @@\\n-old\\n+new",
    };
    act(() => {
      emit("plan", {
        sessionId: "session-a",
        markdown: "# 계획",
        revision: 1,
      });
      emit("plan", {
        sessionId: "session-a",
        markdown: "# 계획",
        revision: 1,
      });
      emit("plan", {
        sessionId: "session-a",
        markdown: "# 수정 계획",
        revision: 2,
      });
      emit("changeset", {
        sessionId: "session-a",
        request_id: "request-1",
        items: [item],
      });
      emit("changeset", {
        sessionId: "session-a",
        request_id: "request-1",
        items: [item],
      });
    });

    await waitFor(() => {
      const attentionCalls = tauri.invoke.mock.calls.filter(
        ([command]) => command === "attention_notify",
      );
      expect(attentionCalls).toEqual([
        [
          "attention_notify",
          { kind: "planApproval", showOs: true, sessionId: "session-a" },
        ],
        [
          "attention_notify",
          { kind: "planApproval", showOs: true, sessionId: "session-a" },
        ],
        [
          "attention_notify",
          {
            kind: "changesetReview",
            showOs: true,
            sessionId: "session-a",
            itemCount: 1,
          },
        ],
      ]);
    });
    focus.mockRestore();
  });

  it("selects the session named by a clicked OS notification", async () => {
    render(<App />);
    await screen.findByRole("button", { name: "Session A, 유휴" });
    await waitFor(() =>
      expect(tauri.listeners.has("notification_activated")).toBe(true),
    );

    act(() => {
      emit("notification_activated", { sessionId: "session-b" });
    });

    expect(
      screen.getByRole("button", { name: "Session B, 유휴" }),
    ).toHaveAttribute("aria-current", "page");
  });
});

describe("App project tools sidebar", () => {
  it("closes and reopens the tabbed sidebar with one header toggle", async () => {
    render(<App />);

    expect(
      await screen.findByRole("complementary", { name: "프로젝트 도구" }),
    ).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "DAT 위키" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "메모리" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "파일" })).toBeInTheDocument();
    expect(
      within(screen.getByRole("complementary", { name: "프로젝트 도구" })).queryByRole(
        "button",
        { name: /닫기/ },
      ),
    ).not.toBeInTheDocument();

    fireEvent.click(
      screen.getByRole("button", { name: "프로젝트 도구 닫기" }),
    );
    expect(
      screen.queryByRole("complementary", { name: "프로젝트 도구" }),
    ).not.toBeInTheDocument();

    fireEvent.click(
      screen.getByRole("button", { name: "프로젝트 도구 열기" }),
    );
    expect(
      screen.getByRole("complementary", { name: "프로젝트 도구" }),
    ).toBeInTheDocument();
  });
});
