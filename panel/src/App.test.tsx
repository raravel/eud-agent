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
