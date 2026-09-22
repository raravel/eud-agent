/**
 * `delegate_read` child runs nest under their parent tool row: the child's
 * tool events (tagged with `delegationRunId`) never open foreground rows, the
 * `delegation` event attaches the lifecycle to the parent, and the nested rows
 * survive the turn-end archive.
 */
import { describe, expect, it } from "vitest";
import { createPanelStore } from "./store";

function readyWithProject() {
  const store = createPanelStore();
  store.wsOpen();
  store.applyList({ files: [{ path: "a.eps", ftype: "CUIEps", settable: true }] });
  return store;
}

const running = {
  requestId: "req-1",
  parentRunId: 7,
  childRunId: 8,
  goal: "P1 체력 저장 위치 찾기",
  status: "running" as const,
  toolCalls: 0,
  elapsedMs: 0,
};

describe("delegate_read nesting", () => {
  it("nests child tool events under the parent row and keeps the stream flat", () => {
    const store = readyWithProject();
    store.chatSent();
    store.agentEvent("tool_call", "delegate_read", {
      callId: "call-parent",
      args: '{"goal":"P1 체력 저장 위치 찾기"}',
    });
    store.delegationReceived(running);
    store.agentEvent("tool_call", "source_search", {
      callId: "child-1",
      args: '{"query":"hp"}',
      delegationRunId: 8,
    });
    store.agentEvent("tool_result", "source_search", {
      callId: "child-1",
      result: "2 hits",
      status: "completed",
      delegationRunId: 8,
    });
    store.agentEvent("tool_call", "read_file", {
      callId: "child-2",
      delegationRunId: 8,
    });

    const { tools, blocks } = store.getState().turn;
    expect(tools).toHaveLength(1);
    expect(tools[0]).toMatchObject({
      callId: "call-parent",
      name: "delegate_read",
      state: "running",
      delegationRunId: 8,
      delegation: { goal: "P1 체력 저장 위치 찾기", status: "running" },
    });
    expect(tools[0].children).toMatchObject([
      { callId: "child-1", name: "source_search", state: "done", detail: "2 hits" },
      { callId: "child-2", name: "read_file", state: "running" },
    ]);
    // The chronological block holds the same nested row.
    expect(blocks).toHaveLength(1);
    expect(blocks[0].type === "tools" && blocks[0].tools[0].children).toHaveLength(2);

    store.delegationReceived({
      ...running,
      status: "completed",
      toolCalls: 3,
      elapsedMs: 4200,
    });
    store.agentEvent("tool_result", "delegate_read", {
      callId: "call-parent",
      result: '{"summary":"src/hp.eps:12"}',
      status: "completed",
    });
    const parent = store.getState().turn.tools[0];
    expect(parent.state).toBe("done");
    expect(parent.detail).toBe('{"summary":"src/hp.eps:12"}');
    expect(parent.delegation).toMatchObject({
      status: "completed",
      toolCalls: 3,
      elapsedMs: 4200,
    });
    expect(parent.children).toHaveLength(2);

    // The archived turn keeps the nested rows.
    store.answerReceived("답변");
    const archived = store.getState().log.find((entry) => entry.tools);
    expect(archived?.tools?.[0].children).toHaveLength(2);
    expect(archived?.tools?.[0].delegation?.status).toBe("completed");
  });

  it("claims the latest running delegate_read row when the child's events arrive first", () => {
    const store = readyWithProject();
    store.chatSent();
    store.agentEvent("tool_call", "delegate_read", { callId: "call-old" });
    store.agentEvent("tool_result", "delegate_read", {
      callId: "call-old",
      result: "{}",
      status: "completed",
    });
    store.agentEvent("tool_call", "delegate_read", { callId: "call-new" });
    store.agentEvent("tool_call", "list_files", {
      callId: "child-1",
      delegationRunId: 9,
    });
    store.delegationReceived({ ...running, childRunId: 9, status: "failed", error: "위임된 읽기 작업을 완료하지 못했습니다: 시간 초과" });

    const tools = store.getState().turn.tools;
    expect(tools).toHaveLength(2);
    expect(tools[0].children).toBeUndefined();
    expect(tools[1]).toMatchObject({
      callId: "call-new",
      delegationRunId: 9,
      delegation: { status: "failed", error: "위임된 읽기 작업을 완료하지 못했습니다: 시간 초과" },
    });
    expect(tools[1].children).toMatchObject([{ callId: "child-1", name: "list_files" }]);
  });

  it("drops child events and lifecycle with no live parent", () => {
    const store = readyWithProject();
    store.chatSent();
    store.agentEvent("tool_call", "read_file", { callId: "child-x", delegationRunId: 3 });
    store.delegationReceived({ ...running, childRunId: 3 });
    expect(store.getState().turn.tools).toHaveLength(0);
    expect(store.getState().log.some((entry) => entry.text.includes("read_file"))).toBe(false);

    store.answerReceived("끝");
    store.delegationReceived({ ...running, childRunId: 3, status: "completed" });
    expect(store.getState().phase).toBe("ready");
  });
});
