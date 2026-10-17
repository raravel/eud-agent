import { describe, expect, it } from "vitest";

import type { SessionMeta } from "@/lib/protocol";
import { createMapTurn, steerRowsPlaceable } from "@/map/mapConversation";
import type {
  CandidateStateView,
  MapBootstrapResponse,
  MapDiff,
  MapRunTranscript,
} from "@/map/mapProtocol";
import {
  candidateSummary,
  createRemoteMapStore,
  mapSessionRows,
  rewindPrefix,
  wholeMapRender,
} from "./remoteMapModel";

const zero = { added: 0, removed: 0, moved: 0, changed: 0 };

function diff(overrides: Partial<MapDiff> = {}): MapDiff {
  return {
    terrainCells: 0,
    units: zero,
    buildings: zero,
    doodads: zero,
    sprites: zero,
    locations: zero,
    outsideTarget: 0,
    protected: 0,
    unsupportedSectionChanges: [],
    ...overrides,
  };
}

function candidate(overrides: Partial<CandidateStateView> = {}): CandidateStateView {
  const revision = (number: number, change: Partial<MapDiff>) => ({
    revision: number,
    parent: number - 1,
    requestId: `map-${number}`,
    mapSha256: "s",
    diff: diff(change),
    verification: {
      valid: true,
      errors: [],
      warnings: number === 2 ? ["경로가 좁습니다"] : [],
      diff: diff(change),
      candidateSha256: "s",
      canonicalDigest: "d",
      extraAssetsDigest: "e",
    },
  });
  return {
    sessionId: "m1",
    baseline: { width: 128, height: 96 } as CandidateStateView["baseline"],
    currentRevision: 2,
    currentHash: "h",
    revisionKey: "r2:h",
    revisions: [
      revision(1, { terrainCells: 4 }),
      revision(2, {
        terrainCells: 12,
        units: { added: 2, removed: 1, moved: 0, changed: 0 },
        outsideTarget: 3,
      }),
    ],
    selections: [],
    stale: false,
    sourceDiverged: false,
    canApply: true,
    canUndo: false,
    ...overrides,
  };
}

function bootstrap(log: unknown[] = []): MapBootstrapResponse {
  return {
    candidate: candidate(),
    context: { revision: { projectId: "p1" } },
    session: {
      id: "m1",
      name: "맵 작업 1",
      panelLog: { schemaVersion: 2, logSeq: log.length, log },
      contextUsage: null,
    },
  } as unknown as MapBootstrapResponse;
}

const ev = (name: string, payload: Record<string, unknown>) => ({
  name,
  payload: { sessionId: "m1", candidateRevision: "r2:h", ...payload },
});

describe("phone Map session list", () => {
  it("keeps Map sessions newest first and labels team sessions", () => {
    const meta = (id: string, at: number, extra: Partial<SessionMeta> = {}) =>
      ({ id, name: id, kind: "map", lastConversationAt: at, createdAt: at, ...extra }) as SessionMeta;
    const rows = mapSessionRows([
      meta("old", 1),
      meta("eps", 9, { kind: "eps" }),
      meta("team", 5, { teamParent: "s1" }),
    ]);
    expect(rows.map((row) => [row.id, row.teamBadge])).toEqual([
      ["team", "팀 작업"],
      ["old", null],
    ]);
  });
});

describe("phone candidate summary", () => {
  it("lists the current revision's changes, warnings and revert targets", () => {
    const summary = candidateSummary(candidate());
    expect(summary.revision).toBe(2);
    expect(summary.changes).toEqual(["지형 12칸", "유닛 추가 2 · 삭제 1"]);
    expect(summary.warnings).toEqual(["대상 영역 밖 변경 3칸", "경로가 좁습니다"]);
    expect(summary.revertTargets).toEqual([1, 0]);
    expect(summary.canApply).toBe(true);
    expect(summary.canDiscard).toBe(true);
  });

  it("warns first when the source diverged or changed under a request", () => {
    const summary = candidateSummary(candidate({ sourceDiverged: true, stale: true }));
    expect(summary.warnings[0]).toContain("원본 맵이 바뀌었습니다");
    expect(summary.warnings[1]).toContain("덮어씁니다");
  });

  it("has nothing to discard or revert at r0", () => {
    const summary = candidateSummary(candidate({ currentRevision: 0, revisions: [], canApply: false }));
    expect(summary.changes).toEqual([]);
    expect(summary.canDiscard).toBe(false);
    expect(summary.revertTargets).toEqual([]);
  });

  it("renders the whole map of a view at the minimap scale", () => {
    expect(wholeMapRender(candidate(), "original")).toEqual({
      sessionId: "m1",
      view: "original",
      x: 0,
      y: 0,
      width: 128,
      height: 96,
      scale: 8,
      layers: ["terrain", "doodads", "sprites", "units", "buildings"],
    });
  });
});

describe("phone Map conversation", () => {
  it("restores the saved log and folds a request sent here into entries", () => {
    const store = createRemoteMapStore(bootstrap([{ id: 3, kind: "agent", text: "이전 답" }]));
    expect(store.getState().conversation.map((entry) => entry.text)).toEqual(["이전 답"]);

    store.sendStarted("언덕을 만들어 줘", [], "r2:h");
    expect(store.getState().inFlight).toBe(true);
    // The backend announces the run: the request bubble takes its id.
    expect(
      store.runStarted({
        sessionId: "m1",
        requestId: "map-x",
        candidateRevision: "r2:h",
        prompt: { text: "언덕을 만들어 줘", mentions: [], origin: "user" },
      }),
    ).toBe(false);
    expect(store.getState().conversation.at(-1)).toMatchObject({
      kind: "you",
      requestId: "map-x",
    });

    const { name, payload } = ev("agent_event", { kind: "delta", detail: "만들었습니다" });
    store.event(name, payload);
    // Another session's or revision's events are ignored.
    store.event("agent_event", { ...payload, sessionId: "m2" });
    store.event("agent_event", { ...payload, candidateRevision: "r1:x" });
    expect(store.getState().turn.answer).toBe("만들었습니다");

    store.event("answer", ev("answer", { text: "만들었습니다" }).payload);
    store.sendSettled();
    const state = store.getState();
    expect(state.inFlight).toBe(false);
    expect(state.conversation.map((entry) => [entry.kind, entry.text])).toEqual([
      ["agent", "이전 답"],
      ["you", "언덕을 만들어 줘"],
      ["agent", "만들었습니다"],
    ]);
    expect(store.logSequence()).toBe(5);
  });

  it("records a failed request once", () => {
    const store = createRemoteMapStore(bootstrap());
    store.sendStarted("x", [], "r2:h");
    store.sendSettled("SCMDraft가 맵을 열고 있습니다. 저장 후 닫아 주세요.");
    expect(store.getState().conversation.at(-1)).toMatchObject({
      kind: "error",
      text: "SCMDraft가 맵을 열고 있습니다. 저장 후 닫아 주세요.",
    });
  });

  it("adopts a run started on the PC or by a team task, and closes it on its answer", () => {
    const store = createRemoteMapStore(bootstrap());
    expect(
      store.runStarted({
        sessionId: "m1",
        requestId: "map-team",
        candidateRevision: "r2:h",
        prompt: { text: "goal", mentions: [], origin: "team" },
      }),
    ).toBe(true);
    const run: MapRunTranscript = {
      requestId: "map-team",
      candidateRevision: "r2:h",
      inFlight: true,
      prompt: { text: "길 만들기", mentions: [], origin: "team", parentSessionName: "메인" },
      events: [ev("agent_event", { kind: "delta", detail: "작업 중" })],
      truncated: false,
    };
    store.adoptRun(run);
    store.adoptRun(run);
    let state = store.getState();
    expect(state.inFlight).toBe(true);
    expect(state.conversation.filter((entry) => entry.kind === "you")).toHaveLength(1);
    expect(state.turn.answer).toBe("작업 중");

    store.event("answer", ev("answer", { text: "완료" }).payload);
    state = store.getState();
    expect(state.inFlight).toBe(false);
    expect(state.conversation.at(-1)).toMatchObject({ kind: "agent", text: "작업 중" });
  });

  it("closes an adopted run that already ended without an answer", () => {
    const store = createRemoteMapStore(bootstrap());
    store.adoptRun({
      requestId: "map-old",
      candidateRevision: "r2:h",
      inFlight: false,
      prompt: { text: "old", mentions: [], origin: "user" },
      events: [],
      truncated: true,
    });
    const state = store.getState();
    expect(state.inFlight).toBe(false);
    expect(state.conversation[0].text).toBe("이전 이벤트 일부가 생략되었습니다.");
  });

  it("shows and clears a pending question, and cancels a turn", () => {
    const store = createRemoteMapStore(bootstrap());
    store.restoreAsk(
      { sessionId: "m1", requestId: "ask-1", questions: [], candidateRevision: "r1:old" },
      "r2:h",
    );
    expect(store.getState().ask).toBeUndefined();
    store.restoreAsk({ sessionId: "m1", requestId: "ask-1", questions: [] }, "r2:h");
    expect(store.getState().ask?.requestId).toBe("ask-1");
    store.event("ask", ev("ask", { requestId: "ask-1", status: "expired" }).payload);
    expect(store.getState().ask).toBeUndefined();

    store.cancelled();
    expect(store.getState().inFlight).toBe(false);
    expect(store.getState().conversation.at(-1)?.text).toBe("작업을 중단했습니다.");
  });

  it("rewinds to the rows before an edited message", () => {
    const log = [
      { id: 1, kind: "you", text: "a" },
      { id: 2, kind: "agent", text: "b" },
      { id: 3, kind: "you", text: "c" },
    ];
    const store = createRemoteMapStore(bootstrap(log));
    expect(rewindPrefix(store.getState().conversation, 3).map((entry) => entry.id)).toEqual([1, 2]);
    store.rewound(3);
    expect(store.getState().conversation.map((entry) => entry.id)).toEqual([1, 2]);
  });
});

describe("phone Map steering", () => {
  const delta = (detail: string) => ({
    sessionId: "m1",
    candidateRevision: "r2:h",
    kind: "delta",
    detail,
  });

  it("puts a message the running request received where its stream reached", () => {
    const store = createRemoteMapStore(bootstrap());
    store.sendStarted("언덕을 만들어 줘", [], "r2:h");
    store.event("agent_event", delta("언덕을 그립니다."));
    store.steerSent({ steerId: "s1", text: "더 높게", attachments: [], mentions: [] });
    expect(store.getState().pendingSteers).toHaveLength(1);

    expect(
      store.steer({
        sessionId: "m1",
        kind: "applied",
        steers: [{ steerId: "s1", text: "더 높게" }],
      }),
    ).toEqual([]);
    store.event("agent_event", delta("높였습니다."));
    store.event("answer", {
      sessionId: "m1",
      candidateRevision: "r2:h",
      text: "언덕을 그립니다. 높였습니다.",
    });
    store.sendSettled();

    expect(store.getState().pendingSteers).toEqual([]);
    expect(store.getState().conversation.map((entry) => [entry.kind, entry.text])).toEqual([
      ["you", "언덕을 만들어 줘"],
      ["agent", "언덕을 그립니다."],
      ["you", "더 높게"],
      ["agent", "높였습니다."],
    ]);
  });

  it("hands a message of a cancelled request back and follows a request started for one", () => {
    const store = createRemoteMapStore(bootstrap());
    store.sendStarted("언덕", [], "r2:h");
    store.steerSent({ steerId: "s1", text: "더 높게", attachments: [], mentions: [] });
    const returned = store.steer({
      sessionId: "m1",
      kind: "returned",
      steers: [{ steerId: "s1", text: "더 높게" }],
    });
    expect(returned.map((steer) => steer.text)).toEqual(["더 높게"]);
    expect(store.getState().pendingSteers).toEqual([]);

    store.steerSent({ steerId: "s2", text: "나무도", attachments: [], mentions: [] });
    store.event("answer", { sessionId: "m1", candidateRevision: "r2:h", text: "끝" });
    store.steer({ sessionId: "m1", kind: "turn", steers: [{ steerId: "s2", text: "나무도" }] });
    expect(store.getState().conversation.at(-1)).toMatchObject({ kind: "you", text: "나무도" });
    expect(store.getState().inFlight).toBe(true);
    store.event("agent_event", delta("나무를 심습니다."));
    expect(store.getState().turn.answer).toBe("나무를 심습니다.");
  });

  it("places a message received mid-tool once the tool finished", () => {
    const store = createRemoteMapStore(bootstrap());
    store.sendStarted("언덕", [], "r2:h");
    store.event("agent_event", {
      sessionId: "m1",
      candidateRevision: "r2:h",
      kind: "tool_call",
      detail: "map_draft_patch",
      data: { callId: "c1" },
    });
    store.steer({ sessionId: "m1", kind: "applied", steers: [{ steerId: "s1", text: "더 높게" }] });
    expect(store.getState().conversation.map((entry) => entry.text)).toEqual(["언덕"]);
    store.event("agent_event", {
      sessionId: "m1",
      candidateRevision: "r2:h",
      kind: "tool_result",
      detail: "map_draft_patch",
      data: { callId: "c1", status: "completed" },
    });
    const conversation = store.getState().conversation;
    expect(conversation.map((entry) => entry.kind)).toEqual(["you", "info", "you"]);
    expect(conversation[1].tools?.[0]?.state).toBe("done");
    expect(conversation[2].text).toBe("더 높게");
  });

  it("remembers a stopped request so a late message goes back to the composer", () => {
    const store = createRemoteMapStore(bootstrap());
    store.sendStarted("언덕", [], "r2:h");
    store.cancelled();
    expect(store.getState().lastRunCancelled).toBe(true);
    store.sendStarted("다시", [], "r3:h");
    expect(store.getState().lastRunCancelled).toBe(false);
  });

  const toolCall = {
    sessionId: "m1",
    candidateRevision: "r2:h",
    kind: "tool_call",
    detail: "map_draft_patch",
    data: { callId: "c1" },
  };
  const appliedMidTool = (store: ReturnType<typeof createRemoteMapStore>) => {
    store.sendStarted("언덕", [], "r2:h");
    store.event("agent_event", toolCall);
    store.steer({ sessionId: "m1", kind: "applied", steers: [{ steerId: "s1", text: "더 높게" }] });
  };

  it("places a message received mid-tool when the request is stopped", () => {
    const store = createRemoteMapStore(bootstrap());
    appliedMidTool(store);
    store.cancelled();
    expect(store.getState().conversation.map((entry) => entry.text).slice(-2)).toEqual([
      "더 높게",
      "작업을 중단했습니다.",
    ]);
  });

  it("places a message received mid-tool when the request fails", () => {
    const store = createRemoteMapStore(bootstrap());
    appliedMidTool(store);
    store.sendSettled("맵을 쓸 수 없습니다.");
    const texts = store.getState().conversation.map((entry) => entry.text);
    expect(texts.indexOf("더 높게")).toBeLessThan(texts.indexOf("맵을 쓸 수 없습니다."));
    expect(texts).toContain("더 높게");
  });

  it("never carries a parked message into the next request", () => {
    const store = createRemoteMapStore(bootstrap());
    appliedMidTool(store);
    store.cancelled();
    const before = store.getState().conversation.filter((entry) => entry.text === "더 높게");
    store.sendStarted("다시", [], "r3:h");
    store.event("agent_event", { ...toolCall, candidateRevision: "r3:h", kind: "tool_result", data: { callId: "c1", status: "completed" } });
    store.event("answer", { sessionId: "m1", candidateRevision: "r3:h", text: "끝" });
    expect(store.getState().conversation.filter((entry) => entry.text === "더 높게")).toHaveLength(before.length);
  });
});

describe("Map steer row placement", () => {
  it("places parked rows only once no tool runs, or at once when the request ends", () => {
    const running = {
      ...createMapTurn(),
      tools: [{ id: "t1", name: "map_draft_patch", state: "running" as const }],
    };
    expect(steerRowsPlaceable(running, 1, false)).toBe(false);
    expect(steerRowsPlaceable(running, 1, true)).toBe(true);
    expect(steerRowsPlaceable(createMapTurn(), 1, false)).toBe(true);
    expect(steerRowsPlaceable(createMapTurn(), 0, true)).toBe(false);
  });
});
