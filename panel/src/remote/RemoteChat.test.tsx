import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

const remote = vi.hoisted(() => {
  const listeners = new Map<string, Set<(event: { payload: unknown }) => void>>();
  const calls: Array<{ cmd: string; args?: unknown; options?: unknown }> = [];
  const responses = new Map<string, (args: unknown) => unknown>();
  return {
    listeners,
    calls,
    responses,
    emit(event: string, payload: unknown) {
      for (const handler of listeners.get(event) ?? []) handler({ payload });
    },
    invoke: async (cmd: string, args?: unknown, options?: unknown) => {
      calls.push({ cmd, args, options });
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
      currentStatus: () => "open" as const,
      onStatus: () => () => {},
      attachedProjectId: () => "p1",
      waitForOpen: vi.fn(async () => true),
      reconnects: 0,
      reconnectCount(): number {
        return this.reconnects;
      },
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

import { CONNECTION_LOST_MESSAGE, REQUEST_HEADER, RESYNC_EVENT } from "@/lib/remoteTransport";

import { RemoteChat } from "./RemoteChat";

const capabilities = {
  vision: true,
  toolCalls: true,
  strictStructuredOutput: true,
  nativeCompaction: true,
  hostedWebSearch: true,
};
const MODELS = [
  {
    provider: "codex",
    model: "gpt-default",
    displayName: "GPT Default",
    description: "기본 모델",
    isDefault: true,
    capabilities: { ...capabilities, reasoningLevels: ["medium", "high"] },
  },
  {
    provider: "codex",
    model: "gpt-fast",
    displayName: "GPT Fast",
    description: "빠른 모델",
    isDefault: false,
    capabilities: { ...capabilities, reasoningLevels: ["low"] },
  },
];
const PROVIDERS = ["codex", "claude-code", "antigravity", "opencode-go", "opencode-zen", "ollama"];
const SESSION = {
  id: "s1",
  name: "보스 패턴",
  project: "Sample",
  kind: "eps",
  provider: "codex",
  model: "gpt-default",
  createdAt: 1,
  lastConversationAt: 1,
};

function status(provider: string) {
  return {
    provider,
    availability: "ready",
    selectedAsDefault: provider === "codex",
    canInstall: false,
    experimental: false,
  };
}

function respond(options: { desktopWindow?: boolean } = {}) {
  remote.responses.set("skill_list", () => []);
  remote.responses.set("status", () => ({ compiling: false, project: "Sample", projectId: "p1" }));
  remote.responses.set("list", () => ({ files: [] }));
  remote.responses.set("remote_projects", () => [
    {
      path: "C:\\p",
      name: "Sample",
      open: true,
      projectId: "p1",
      desktopWindow: options.desktopWindow ?? true,
      available: true,
    },
  ]);
  remote.responses.set("provider_status_list", () => PROVIDERS.map(status));
  remote.responses.set("provider_settings", () => ({
    provider: "codex",
    status: status("codex"),
    models: MODELS,
    selectedModel: "gpt-default",
    selectedReasoning: { level: "medium" },
    hasApiKey: false,
  }));
  remote.responses.set("session_create", () => ({
    ...SESSION,
    pendingRequestIds: [],
    panelLog: null,
  }));
  remote.responses.set("session_load", () => ({
    ...SESSION,
    pendingRequestIds: [],
    panelLog: {
      schemaVersion: 2,
      logSeq: 1,
      log: [{ id: 1, kind: "agent", text: "보스를 추가했습니다." }],
    },
  }));
  remote.responses.set("ask_pending", () => null);
  remote.responses.set("session_model_settings", () => ({
    provider: "codex",
    models: MODELS,
    selectedModel: "gpt-fast",
    selectedReasoning: { level: "low" },
  }));
  remote.responses.set("session_model_settings_save", () => ({
    provider: "codex",
    models: MODELS,
    selectedModel: "gpt-fast",
    selectedReasoning: { level: "low" },
  }));
  remote.responses.set("session_update_log", () => null);
  remote.responses.set("chat", () => null);
}

const commands = () => remote.calls.map((call) => call.cmd);

afterEach(() => {
  remote.calls.length = 0;
  remote.responses.clear();
  remote.listeners.clear();
  vi.clearAllMocks();
});

describe("RemoteChat", () => {
  it("shows the PC's default model for a new conversation and starts it with the one picked", async () => {
    respond();
    const user = userEvent.setup();
    render(<RemoteChat sessionId={null} projectName="Sample" onBack={() => {}} />);

    const model = await screen.findByRole("combobox", { name: "세션 모델" });
    await waitFor(() => expect(model).toHaveTextContent("GPT Default"));
    await user.click(model);
    await user.click(screen.getByRole("option", { name: "GPT Fast" }));
    expect(model).toHaveTextContent("GPT Fast");
    // The PC's default for new conversations is not changed from the phone.
    expect(commands()).not.toContain("provider_defaults_save");

    const input = screen.getByRole("combobox", { name: "지시 입력" });
    await user.type(input, "보스를 만들어 줘");
    await user.click(screen.getByRole("button", { name: "실행" }));

    await waitFor(() => expect(commands()).toContain("chat"));
    const order = commands();
    expect(order.indexOf("session_create")).toBeLessThan(order.indexOf("session_model_settings_save"));
    expect(order.indexOf("session_model_settings_save")).toBeLessThan(order.indexOf("chat"));
    expect(
      remote.calls.find((call) => call.cmd === "session_model_settings_save")?.args,
    ).toMatchObject({ sessionId: "s1", model: "gpt-fast", reasoning: { level: "low" } });
  });

  it("does not save a model change the select reports when its options change", async () => {
    respond();
    const user = userEvent.setup();
    render(<RemoteChat sessionId={null} projectName="Sample" onBack={() => {}} />);
    const model = await screen.findByRole("combobox", { name: "세션 모델" });
    await waitFor(() => expect(model).toHaveTextContent("GPT Default"));
    await user.type(screen.getByRole("combobox", { name: "지시 입력" }), "보스를 만들어 줘");
    await user.click(screen.getByRole("button", { name: "실행" }));
    // The created session runs another model: the selects follow it.
    await waitFor(() => expect(model).toHaveTextContent("GPT Fast"));
    await waitFor(() => expect(screen.getByRole("combobox", { name: "추론 단계" })).toHaveTextContent(/.+/));
    expect(commands()).not.toContain("session_model_settings_save");
  });

  it("resends a lost turn with its request id, then reloads instead of failing", async () => {
    respond();
    let chats = 0;
    remote.responses.set("chat", () => {
      chats += 1;
      if (chats === 1) throw CONNECTION_LOST_MESSAGE;
      return null;
    });
    const user = userEvent.setup();
    render(<RemoteChat sessionId="s1" projectName="Sample" onBack={() => {}} />);
    expect(await screen.findByText("보스를 추가했습니다.")).toBeInTheDocument();
    const loads = () => commands().filter((cmd) => cmd === "session_load").length;
    expect(loads()).toBe(1);

    const input = screen.getByRole("combobox", { name: "지시 입력" });
    await waitFor(() => expect(input).toBeEnabled());
    await user.type(input, "체력을 두 배로");
    await user.click(screen.getByRole("button", { name: "실행" }));

    await waitFor(() => expect(loads()).toBeGreaterThanOrEqual(2));
    expect(remote.transport.waitForOpen).toHaveBeenCalled();
    // The same request again: the PC joins the running turn, never a second.
    const sent = remote.calls.filter((call) => call.cmd === "chat");
    expect(sent).toHaveLength(2);
    const id = (call: (typeof sent)[number]) =>
      (call.options as { headers: Record<string, string> }).headers[REQUEST_HEADER];
    expect(id(sent[0])).toBeTruthy();
    expect(id(sent[1])).toBe(id(sent[0]));
    expect(sent[1].args).toEqual(sent[0].args);
    expect(screen.queryByText(/요청을 처리하지 못했습니다/)).not.toBeInTheDocument();
    // The message was saved before the reload, so it is still there.
    const saved = remote.calls.filter((call) => call.cmd === "session_update_log");
    expect(JSON.stringify(saved.at(-1)?.args)).toContain("체력을 두 배로");
    // The draft is not handed back: the PC may be running it.
    expect(input).toHaveValue("");
  });

  it("shows a backend refusal as a failure and gives the message back", async () => {
    respond();
    remote.responses.set("chat", () => {
      throw "이미 실행 중인 요청이 있습니다.";
    });
    const user = userEvent.setup();
    render(<RemoteChat sessionId="s1" projectName="Sample" onBack={() => {}} />);
    const input = await screen.findByRole("combobox", { name: "지시 입력" });
    await waitFor(() => expect(input).toBeEnabled());
    await user.type(input, "체력을 두 배로");
    await user.click(screen.getByRole("button", { name: "실행" }));

    expect(await screen.findAllByText(/이미 실행 중인 요청이 있습니다/)).not.toHaveLength(0);
    await waitFor(() => expect(input).toHaveValue("체력을 두 배로"));
    // A refusal is not a lost answer: nothing is sent again.
    expect(commands().filter((cmd) => cmd === "chat")).toHaveLength(1);
  });

  it("steers the running turn from the phone and shows the message where it arrived", async () => {
    respond();
    remote.responses.set("chat", () => new Promise(() => {}));
    remote.responses.set("chat_steer", () => "accepted");
    const user = userEvent.setup();
    render(<RemoteChat sessionId="s1" projectName="Sample" onBack={() => {}} />);
    const input = await screen.findByRole("combobox", { name: "지시 입력" });
    await waitFor(() => expect(input).toBeEnabled());
    await user.type(input, "체력을 두 배로");
    await user.click(screen.getByRole("button", { name: "실행" }));
    await waitFor(() => expect(commands()).toContain("chat"));

    await user.type(input, "공격력도");
    await user.click(screen.getByRole("button", { name: "작업 중인 AI에게 전달" }));
    await waitFor(() => expect(commands()).toContain("chat_steer"));
    const steer = remote.calls.find((call) => call.cmd === "chat_steer")?.args as {
      sessionId: string;
      steerId: string;
      text: string;
    };
    expect(steer).toMatchObject({ sessionId: "s1", text: "공격력도", attachments: [], mentions: [] });
    expect(await screen.findByTestId("pending-steer")).toHaveTextContent("공격력도");

    remote.emit("steer", {
      sessionId: "s1",
      kind: "applied",
      steers: [{ steerId: steer.steerId, text: "공격력도" }],
    });
    await waitFor(() => expect(screen.queryByTestId("pending-steer")).not.toBeInTheDocument());
    expect(screen.getByText("공격력도")).toBeInTheDocument();
    expect(commands().filter((cmd) => cmd === "chat")).toHaveLength(1);
  });

  it("resends a phone steer whose answer was lost under the same request id", async () => {
    respond();
    remote.responses.set("chat", () => new Promise(() => {}));
    let steers = 0;
    remote.responses.set("chat_steer", () => {
      steers += 1;
      if (steers === 1) throw CONNECTION_LOST_MESSAGE;
      return "accepted";
    });
    const user = userEvent.setup();
    render(<RemoteChat sessionId="s1" projectName="Sample" onBack={() => {}} />);
    const input = await screen.findByRole("combobox", { name: "지시 입력" });
    await waitFor(() => expect(input).toBeEnabled());
    await user.type(input, "체력을 두 배로");
    await user.click(screen.getByRole("button", { name: "실행" }));
    await waitFor(() => expect(commands()).toContain("chat"));
    await user.type(input, "공격력도");
    await user.click(screen.getByRole("button", { name: "작업 중인 AI에게 전달" }));

    await waitFor(() => expect(commands().filter((cmd) => cmd === "chat_steer")).toHaveLength(2));
    const sent = remote.calls.filter((call) => call.cmd === "chat_steer");
    const id = (call: (typeof sent)[number]) =>
      (call.options as { headers: Record<string, string> }).headers[REQUEST_HEADER];
    expect(id(sent[1])).toBe(id(sent[0]));
    expect(sent[1].args).toEqual(sent[0].args);
    // The PC may already hold it: it is not handed back to the composer.
    expect(input).toHaveValue("");
  });

  it("sends a phone message that found no running turn once the turn ended", async () => {
    respond();
    remote.responses.set("chat", () => new Promise(() => {}));
    remote.responses.set("chat_steer", () => "no_turn");
    const user = userEvent.setup();
    render(<RemoteChat sessionId="s1" projectName="Sample" onBack={() => {}} />);
    const input = await screen.findByRole("combobox", { name: "지시 입력" });
    await waitFor(() => expect(input).toBeEnabled());
    await user.type(input, "체력을 두 배로");
    await user.click(screen.getByRole("button", { name: "실행" }));
    await waitFor(() => expect(commands()).toContain("chat"));
    await user.type(input, "공격력도");
    await user.click(screen.getByRole("button", { name: "작업 중인 AI에게 전달" }));
    await waitFor(() => expect(commands()).toContain("chat_steer"));
    expect(commands().filter((cmd) => cmd === "chat")).toHaveLength(1);

    remote.emit("answer", { sessionId: "s1", text: "끝" });
    await waitFor(() => expect(commands().filter((cmd) => cmd === "chat")).toHaveLength(2));
    expect(remote.calls.filter((call) => call.cmd === "chat").at(-1)?.args).toMatchObject({
      text: "공격력도",
    });
  });

  it("leaves the log to the PC window while it only watches, and saves once it takes part", async () => {
    respond({ desktopWindow: true });
    const user = userEvent.setup();
    render(<RemoteChat sessionId="s1" projectName="Sample" onBack={() => {}} />);
    expect(await screen.findByText("보스를 추가했습니다.")).toBeInTheDocument();

    remote.emit("answer", { sessionId: "s1", text: "PC에서 보낸 요청의 답입니다." });
    expect(await screen.findByText("PC에서 보낸 요청의 답입니다.")).toBeInTheDocument();
    await waitFor(() => expect(commands()).toContain("remote_projects"), { timeout: 2_000 });
    expect(commands()).not.toContain("session_update_log");

    const input = screen.getByRole("combobox", { name: "지시 입력" });
    await waitFor(() => expect(input).toBeEnabled());
    await user.type(input, "체력을 두 배로");
    await user.click(screen.getByRole("button", { name: "실행" }));
    await waitFor(() => expect(commands()).toContain("session_update_log"), { timeout: 2_000 });
  });

  it("saves unsaved rows before a resync replaces the conversation", async () => {
    respond({ desktopWindow: false });
    render(<RemoteChat sessionId="s1" projectName="Sample" onBack={() => {}} />);
    expect(await screen.findByText("보스를 추가했습니다.")).toBeInTheDocument();
    remote.emit("answer", { sessionId: "s1", text: "재연결 직전에 도착한 답입니다." });
    expect(await screen.findByText("재연결 직전에 도착한 답입니다.")).toBeInTheDocument();
    // The socket reconnects before the debounced save ran.
    remote.emit(RESYNC_EVENT, {});
    await waitFor(() => expect(commands()).toContain("session_update_log"));
    const saved = remote.calls.find((call) => call.cmd === "session_update_log");
    expect(JSON.stringify(saved?.args)).toContain("재연결 직전에 도착한 답입니다.");
    // Saved first, then reloaded.
    const order = commands();
    expect(order.lastIndexOf("session_load")).toBeGreaterThan(order.indexOf("session_update_log"));
  });

  it("drops a pending save when the PC saved a newer log while it was away", async () => {
    respond({ desktopWindow: false });
    let loads = 0;
    remote.responses.set("session_load", () => {
      loads += 1;
      return {
        ...SESSION,
        pendingRequestIds: [],
        panelLog: {
          schemaVersion: 2,
          logSeq: loads === 1 ? 1 : 99,
          log: [{ id: 1, kind: "agent", text: "보스를 추가했습니다." }],
        },
      };
    });
    render(<RemoteChat sessionId="s1" projectName="Sample" onBack={() => {}} />);
    expect(await screen.findByText("보스를 추가했습니다.")).toBeInTheDocument();
    remote.emit("answer", { sessionId: "s1", text: "오래된 답입니다." });
    expect(await screen.findByText("오래된 답입니다.")).toBeInTheDocument();
    remote.emit(RESYNC_EVENT, {});
    await waitFor(() => expect(loads).toBeGreaterThanOrEqual(3));
    expect(commands()).not.toContain("session_update_log");
  });

  it("drops a save already on its way when a reconnect brought a newer PC log", async () => {
    respond({ desktopWindow: false });
    let loads = 0;
    remote.responses.set("session_load", () => {
      loads += 1;
      return {
        ...SESSION,
        pendingRequestIds: [],
        panelLog: {
          schemaVersion: 2,
          logSeq: loads === 1 ? 1 : 99,
          log: [{ id: 1, kind: "agent", text: "보스를 추가했습니다." }],
        },
      };
    });
    render(<RemoteChat sessionId="s1" projectName="Sample" onBack={() => {}} />);
    expect(await screen.findByText("보스를 추가했습니다.")).toBeInTheDocument();
    // The socket is down: the save that fires waits for it.
    let reopen: (open: boolean) => void = () => {};
    remote.transport.waitForOpen.mockImplementation(
      () => new Promise<boolean>((resolve) => (reopen = resolve)),
    );
    remote.emit("answer", { sessionId: "s1", text: "끊긴 동안 받은 답입니다." });
    expect(await screen.findByText("끊긴 동안 받은 답입니다.")).toBeInTheDocument();
    await waitFor(() => expect(remote.transport.waitForOpen).toHaveBeenCalled(), { timeout: 2_000 });
    // It reconnects (the PC saved more meanwhile) before the save goes out.
    remote.transport.reconnects += 1;
    reopen(true);
    await waitFor(() => expect(loads).toBe(2));
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(commands()).not.toContain("session_update_log");
    remote.transport.reconnects = 0;
    remote.transport.waitForOpen.mockImplementation(async () => true);
  });

  it("saves what it watches when no PC window shows the project", async () => {
    respond({ desktopWindow: false });
    render(<RemoteChat sessionId="s1" projectName="Sample" onBack={() => {}} />);
    expect(await screen.findByText("보스를 추가했습니다.")).toBeInTheDocument();
    remote.emit("answer", { sessionId: "s1", text: "작업자가 이어서 끝냈습니다." });
    await waitFor(() => expect(commands()).toContain("session_update_log"), { timeout: 2_000 });
  });
});
