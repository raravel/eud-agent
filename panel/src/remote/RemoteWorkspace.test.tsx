import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

const remote = vi.hoisted(() => {
  const listeners = new Map<string, Set<(event: { payload: unknown }) => void>>();
  const calls: Array<{ cmd: string; args?: unknown }> = [];
  const responses = new Map<string, (args: unknown) => unknown>();
  return {
    listeners,
    calls,
    responses,
    emit(event: string, payload: unknown) {
      for (const handler of listeners.get(event) ?? []) handler({ payload });
    },
    invoke: async (cmd: string, args?: unknown) => {
      calls.push({ cmd, args });
      const respond = responses.get(cmd);
      if (!respond) throw `unexpected command ${cmd}`;
      return respond(args);
    },
    listen: async (event: string, handler: (event: { payload: unknown }) => void) => {
      let set = listeners.get(event);
      if (!set) listeners.set(event, (set = new Set()));
      set.add(handler);
      return () => set?.delete(handler);
    },
    transport: {
      start: vi.fn(),
      stop: vi.fn(),
      currentStatus: () => "open" as const,
      onStatus: () => () => {},
      attach: vi.fn(async (projectId: string) => ({ projectId, name: "Sample", root: "C:\\p" })),
      detach: vi.fn(async () => {}),
    },
  };
});

vi.mock("@/lib/transport", () => ({
  invoke: remote.invoke,
  listen: remote.listen,
  isRemote: () => true,
}));
vi.mock("@/lib/remoteTransport", async (original) => ({
  ...(await original<typeof import("@/lib/remoteTransport")>()),
  remoteTransport: () => remote.transport,
}));

import { RemoteWorkspace } from "./RemoteWorkspace";

const PROJECT = { projectId: "p1", name: "Sample", root: "C:\\p" };
const SESSION = {
  id: "s1",
  name: "보스 패턴",
  project: "Sample",
  kind: "eps",
  provider: "codex",
  model: "gpt",
  createdAt: 1,
  lastConversationAt: Date.now(),
};

function respond() {
  remote.responses.set("remote_projects", () => [PROJECT]);
  remote.responses.set("session_list", () => [
    SESSION,
    { ...SESSION, id: "w1", name: "작업자", workerParent: "s1" },
  ]);
  remote.responses.set("skill_list", () => []);
  remote.responses.set("status", () => ({ compiling: false, project: "Sample", projectId: "p1" }));
  remote.responses.set("list", () => ({ files: [] }));
  remote.responses.set("session_load", () => ({
    ...SESSION,
    pendingRequestIds: [],
    panelLog: {
      schemaVersion: 2,
      logSeq: 2,
      log: [
        { id: 1, kind: "you", text: "보스를 만들어 줘" },
        { id: 2, kind: "agent", text: "보스를 추가했습니다." },
      ],
    },
  }));
  remote.responses.set("ask_pending", () => null);
  remote.responses.set("session_model_settings", () => ({
    provider: "codex",
    models: [],
    selectedModel: "gpt",
  }));
  remote.responses.set("session_update_log", () => null);
  remote.responses.set("chat", () => null);
}

afterEach(() => {
  remote.calls.length = 0;
  remote.responses.clear();
  remote.listeners.clear();
  vi.clearAllMocks();
});

describe("RemoteWorkspace", () => {
  it("goes from a project to a conversation and sends a message", async () => {
    respond();
    const user = userEvent.setup();
    render(<RemoteWorkspace onDisconnect={() => {}} />);

    await user.click(await screen.findByRole("button", { name: /Sample/ }));
    expect(remote.transport.attach).toHaveBeenCalledWith("p1");

    // Workers stay on the PC; only the main conversation is listed.
    await user.click((await screen.findAllByRole("button", { name: /^보스 패턴(?! 메뉴)/ }))[0]);
    expect(screen.queryByText("작업자")).not.toBeInTheDocument();

    expect(await screen.findByText("보스를 추가했습니다.")).toBeInTheDocument();
    const input = screen.getByRole("combobox", { name: "지시 입력" });
    await waitFor(() => expect(input).toBeEnabled());
    await user.type(input, "체력을 두 배로");
    await user.click(screen.getByRole("button", { name: "실행" }));

    await waitFor(() =>
      expect(remote.calls.find((call) => call.cmd === "chat")?.args).toMatchObject({
        sessionId: "s1",
        text: "체력을 두 배로",
      }),
    );
    // Events of the session reach the conversation.
    remote.emit("answer", { sessionId: "s1", projectId: "p1", text: "두 배로 바꿨습니다." });
    expect(await screen.findByText("두 배로 바꿨습니다.")).toBeInTheDocument();
  });

  it("returns to the project list when the PC closes the project", async () => {
    respond();
    const user = userEvent.setup();
    render(<RemoteWorkspace onDisconnect={() => {}} />);
    await user.click(await screen.findByRole("button", { name: /Sample/ }));
    expect(await screen.findByRole("button", { name: "새 대화" })).toBeInTheDocument();

    remote.emit("remote_detached", { error: "이 프로젝트는 PC에서 열려 있지 않습니다." });
    expect(await screen.findByRole("alert")).toHaveTextContent("PC에서 열려 있지 않습니다");
    expect(screen.getByRole("heading", { name: "프로젝트" })).toBeInTheDocument();
  });
});
