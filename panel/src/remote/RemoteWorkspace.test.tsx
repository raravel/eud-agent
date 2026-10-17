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
      attachedProjectId: () => "p1",
      waitForOpen: vi.fn(async () => true),
      reconnectCount: () => 0,
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

import { CONNECTION_LOST_MESSAGE, RemoteCommandError } from "@/lib/remoteTransport";

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
      if (!force) throw new RemoteCommandError("busy", refusal);
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
      // Its wording mentions work in progress, but its kind cannot be forced.
      throw new RemoteCommandError(
        "companions",
        "PC에서 Map Agent와 맵 가져오기 창을 닫은 뒤 프로젝트를 닫아 주세요. 진행 중인 작업과는 무관합니다.",
      );
    });
    const user = userEvent.setup();
    render(<RemoteWorkspace onDisconnect={() => {}} />);

    await user.click(await screen.findByRole("button", { name: "Held 닫기" }));
    await user.click(within(screen.getByRole("dialog")).getByRole("button", { name: "닫기" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Map Agent");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(remote.transport.closeProject).toHaveBeenCalledTimes(1);
  });

  it("opens a Map session from the Map tab, follows its run and applies the candidate", async () => {
    respond();
    const zero = { added: 0, removed: 0, moved: 0, changed: 0 };
    const diff = {
      terrainCells: 9,
      units: zero,
      buildings: zero,
      doodads: zero,
      sprites: zero,
      locations: zero,
      outsideTarget: 0,
      protected: 0,
      unsupportedSectionChanges: [],
    };
    const candidate = {
      sessionId: "m2",
      baseline: { width: 64, height: 64 },
      currentRevision: 1,
      currentHash: "h",
      revisionKey: "r1:h",
      revisions: [
        {
          revision: 1,
          parent: 0,
          requestId: "map-1",
          mapSha256: "s",
          diff,
          verification: { valid: true, errors: [], warnings: [], diff },
        },
      ],
      selections: [],
      stale: false,
      sourceDiverged: false,
      canApply: true,
      canUndo: false,
    };
    const map = (id: string, name: string, extra = {}) => ({
      ...SESSION,
      id,
      name,
      kind: "map",
      ...extra,
    });
    remote.responses.set("map_agent_session_list", () => [
      map("m1", "맵 작업 1"),
      map("m2", "메인 · 맵 작업", { teamParent: "s1", lastConversationAt: Date.now() + 1 }),
    ]);
    remote.responses.set("map_agent_session_load", () => ({
      context: { revision: { projectId: "p1" } },
      candidate,
      session: { ...map("m2", "메인 · 맵 작업", { teamParent: "s1" }), panelLog: null },
      pendingRun: {
        requestId: "map-team",
        candidateRevision: "r1:h",
        inFlight: true,
        prompt: { text: "길 만들기", mentions: [], origin: "team", parentSessionName: "보스 패턴" },
        events: [],
        truncated: false,
      },
    }));
    remote.responses.set("map_agent_render", () => new Uint8Array([137, 80, 78, 71]).buffer);
    const refusal = "SCMDraft 2가 맵을 열고 있습니다. SCMDraft에서 저장한 뒤 닫고 다시 적용해 주세요.";
    remote.responses.set("map_agent_candidate_apply", () => {
      throw refusal;
    });
    const createObjectURL = vi.fn(() => "blob:map");
    Object.assign(URL, { createObjectURL, revokeObjectURL: vi.fn() });
    const user = userEvent.setup();
    render(<RemoteWorkspace onDisconnect={() => {}} />);

    await user.click(await screen.findByRole("button", { name: /Sample/ }));
    await user.click(await screen.findByRole("tab", { name: /맵 에이전트/ }));
    // The team session an EPS map task runs in is listed first, labeled.
    const team = await screen.findByRole("button", { name: "메인 · 맵 작업 (팀 작업) 열기" });
    await user.click(team);
    expect(remote.calls.find((call) => call.cmd === "map_agent_session_load")?.args).toEqual({
      sessionId: "m2",
    });

    // The running team request is adopted like one typed here.
    expect(await screen.findByText(/EPS 세션 «보스 패턴»의 맵 작업 요청/)).toBeInTheDocument();
    remote.emit("agent_event", {
      sessionId: "m2",
      candidateRevision: "r1:h",
      kind: "delta",
      detail: "길을 깔았습니다",
    });
    expect(await screen.findByText("길을 깔았습니다")).toBeInTheDocument();
    remote.emit("answer", { sessionId: "m2", candidateRevision: "r1:h", text: "길을 깔았습니다" });

    await user.click(screen.getByRole("tab", { name: /후보 r1/ }));
    expect(await screen.findByRole("img", { name: "후보 r1 맵" })).toHaveAttribute("src", "blob:map");
    expect(remote.calls.find((call) => call.cmd === "map_agent_render")?.args).toMatchObject({
      command: { sessionId: "m2", view: "candidate", width: 64, height: 64, scale: 8 },
    });
    expect(screen.getByText("지형 9칸")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "적용" }));
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent("후보를 원본 맵에 적용할까요?");
    await user.click(within(dialog).getByRole("button", { name: "적용" }));
    expect(remote.calls.find((call) => call.cmd === "map_agent_candidate_apply")?.args).toEqual({
      sessionId: "m2",
    });
    // The PC's refusal reaches the phone as it is.
    expect(await screen.findByText(refusal)).toBeInTheDocument();
    // A run the phone only watched, with a PC window on the project, is not
    // saved from the phone (neither on open nor when it settles).
    await new Promise((resolve) => setTimeout(resolve, 700));
    expect(remote.calls.map((call) => call.cmd)).not.toContain("session_update_log");
  });

  it("keeps the Map draft across tabs and reloads a Map turn whose answer was lost", async () => {
    respond();
    const zero = { added: 0, removed: 0, moved: 0, changed: 0 };
    const diff = {
      terrainCells: 0,
      units: zero,
      buildings: zero,
      doodads: zero,
      sprites: zero,
      locations: zero,
      outsideTarget: 0,
      protected: 0,
      unsupportedSectionChanges: [],
    };
    const candidate = {
      sessionId: "m1",
      baseline: { width: 64, height: 64 },
      currentRevision: 0,
      currentHash: "h",
      revisionKey: "r0:h",
      revisions: [],
      selections: [],
      stale: false,
      sourceDiverged: false,
      canApply: false,
      canUndo: false,
      diff,
    };
    const session = { ...SESSION, id: "m1", name: "맵 작업 1", kind: "map" };
    remote.responses.set("map_agent_session_list", () => [session]);
    remote.responses.set("map_agent_session_load", () => ({
      context: { revision: { projectId: "p1" } },
      candidate,
      session: { ...session, panelLog: null },
      pendingRun: null,
    }));
    remote.responses.set("map_agent_chat", () => {
      throw CONNECTION_LOST_MESSAGE;
    });
    remote.responses.set("map_agent_render", () => new Uint8Array([137, 80, 78, 71]).buffer);
    Object.assign(URL, { createObjectURL: vi.fn(() => "blob:map"), revokeObjectURL: vi.fn() });
    const user = userEvent.setup();
    render(<RemoteWorkspace onDisconnect={() => {}} />);

    await user.click(await screen.findByRole("button", { name: /Sample/ }));
    await user.click(await screen.findByRole("tab", { name: /맵 에이전트/ }));
    await user.click(await screen.findByRole("button", { name: "맵 작업 1 열기" }));
    const input = await screen.findByLabelText("맵 요청 입력");
    // @ mentions are a PC feature; the example does not suggest them.
    expect(input.getAttribute("placeholder")).not.toContain("@");

    await user.type(input, "정글 지형을 만들어 줘");
    await user.click(screen.getByRole("tab", { name: /후보 r0/ }));
    await user.click(screen.getByRole("tab", { name: /대화/ }));
    expect(screen.getByLabelText("맵 요청 입력")).toHaveValue("정글 지형을 만들어 줘");

    const loads = () => remote.calls.filter((call) => call.cmd === "map_agent_session_load").length;
    expect(loads()).toBe(1);
    await user.click(screen.getByRole("button", { name: "전송" }));
    await waitFor(() => expect(loads()).toBe(2));
    expect(remote.transport.waitForOpen).toHaveBeenCalled();
    expect(screen.queryByText(CONNECTION_LOST_MESSAGE)).not.toBeInTheDocument();
  });
});
