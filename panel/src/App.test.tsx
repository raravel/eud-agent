import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const tauri = vi.hoisted(() => ({
  invoke: vi.fn(),
  listeners: new Map<string, (event: { payload: unknown }) => void>(),
  resolveLongChat: undefined as (() => void) | undefined,
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
    updatedAt: 2,
    threadId: null,
    pendingRequestIds: [],
    panelLog: { schemaVersion: 2, logSeq: 0, log: [] },
  },
  {
    id: "session-b",
    name: "Session B",
    project: "Project",
    createdAt: 1,
    updatedAt: 1,
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

beforeEach(() => {
  tauri.listeners.clear();
  tauri.resolveLongChat = undefined;
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
        return sessionRecords.map(({ id, name, project, createdAt, updatedAt }) => ({
          id,
          name,
          project,
          createdAt,
          updatedAt,
        }));
      case "session_load":
        return sessionRecords.find((record) => record.id === args?.id);
      case "session_update_log":
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
        activity: "waiting_write",
        queuePosition: 1,
      });
    });

    expect(screen.getByRole("button", { name: "Session A, 분석 중" })).toBeInTheDocument();
    const waitingRow = screen.getByRole("button", {
      name: /Session B, 쓰기 대기 1 · 앞 작업 완료 대기 · \d+초/,
    });
    expect(waitingRow).toBeInTheDocument();
    expect(screen.queryByText("B-only answer")).not.toBeInTheDocument();

    fireEvent.click(waitingRow);
    expect(await screen.findByText("B-only answer")).toBeInTheDocument();
    expect(
      tauri.invoke.mock.calls.filter(([command]) => command === "session_open"),
    ).toHaveLength(0);
  });

  it("shows the blocking review, elapsed wait, and explicit write grant transition", async () => {
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
        activity: "waiting_write",
        queuePosition: 1,
        blockingSessionId: "session-a",
      });
    });

    expect(
      screen.getByRole("button", {
        name: /Session B, 쓰기 대기 1 · Session A 검토 결정 대기 · \d+초/,
      }),
    ).toBeInTheDocument();

    act(() => {
      emit("session_activity", {
        sessionId: "session-b",
        activity: "running_write",
      });
    });
    fireEvent.click(screen.getByRole("button", { name: "Session B, 변경 중" }));
    expect(
      await screen.findByText("쓰기 권한을 획득했습니다. 변경을 시작합니다."),
    ).toBeInTheDocument();
  });
});
