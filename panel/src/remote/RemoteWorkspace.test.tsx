import { render, screen, waitFor, within } from "@testing-library/react";
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
      openProject: vi.fn(async (path: string) => ({ projectId: "p3", name: "Recent", root: path })),
      closeProject: vi.fn(async (_projectId: string, _force?: boolean) => ({ closed: true })),
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

const PROJECT = {
  path: "C:\\p",
  name: "Sample",
  open: true,
  projectId: "p1",
  desktopWindow: true,
  available: true,
};
const HELD = {
  path: "C:\\held",
  name: "Held",
  open: true,
  projectId: "p2",
  desktopWindow: false,
  available: true,
};
const RECENT = {
  path: "C:\\recent",
  name: "Recent",
  open: false,
  desktopWindow: false,
  available: true,
};
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
  vi.resetAllMocks();
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

    remote.emit("remote_detached", { error: "프로젝트가 닫혔습니다. 프로젝트 목록에서 다시 열어 주세요." });
    expect(await screen.findByRole("alert")).toHaveTextContent("프로젝트가 닫혔습니다");
    expect(screen.getByRole("heading", { name: "프로젝트" })).toBeInTheDocument();
  });

  it("lists open and recent projects and opens a recent one on the PC", async () => {
    respond();
    remote.responses.set("remote_projects", () => [PROJECT, HELD, RECENT]);
    const user = userEvent.setup();
    render(<RemoteWorkspace onDisconnect={() => {}} />);

    expect(await screen.findByText("PC 창에서 열림")).toBeInTheDocument();
    expect(screen.getByText("원격으로 열림")).toBeInTheDocument();
    // Only the project no PC window shows can be closed from the phone.
    expect(screen.queryByRole("button", { name: "Sample 닫기" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Held 닫기" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Recent 닫기" })).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Recent 열기" }));
    expect(remote.transport.openProject).toHaveBeenCalledWith("C:\\recent");
    expect(remote.transport.attach).not.toHaveBeenCalled();
    expect(await screen.findByRole("heading", { name: "Recent" })).toBeInTheDocument();
  });

  it("offers to stop the work when the PC refuses to close a busy project", async () => {
    respond();
    let listed = [PROJECT, HELD];
    remote.responses.set("remote_projects", () => listed);
    const refusal =
      "진행 중인 작업이나 검토 대기 중인 계획이 있어 프로젝트를 닫지 않았습니다.";
    remote.transport.closeProject.mockImplementation(async (_id, force) => {
      if (!force) throw refusal;
      listed = [PROJECT];
      return { closed: true };
    });
    const user = userEvent.setup();
    render(<RemoteWorkspace onDisconnect={() => {}} />);

    await user.click(await screen.findByRole("button", { name: "Held 닫기" }));
    await user.click(within(screen.getByRole("dialog")).getByRole("button", { name: "닫기" }));
    expect(remote.transport.closeProject).toHaveBeenLastCalledWith("p2", false);

    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent("닫지 않았습니다");
    await user.click(within(dialog).getByRole("button", { name: "작업 중단 후 닫기" }));
    expect(remote.transport.closeProject).toHaveBeenLastCalledWith("p2", true);
    await waitFor(() => expect(screen.queryByText("Held")).not.toBeInTheDocument());
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("shows a refusal that stopping the work cannot override", async () => {
    respond();
    remote.responses.set("remote_projects", () => [HELD]);
    remote.transport.closeProject.mockImplementation(async () => {
      throw "PC에서 Map Agent와 맵 가져오기 창을 닫은 뒤 프로젝트를 닫아 주세요.";
    });
    const user = userEvent.setup();
    render(<RemoteWorkspace onDisconnect={() => {}} />);

    await user.click(await screen.findByRole("button", { name: "Held 닫기" }));
    await user.click(within(screen.getByRole("dialog")).getByRole("button", { name: "닫기" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Map Agent");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(remote.transport.closeProject).toHaveBeenCalledTimes(1);
  });
});
