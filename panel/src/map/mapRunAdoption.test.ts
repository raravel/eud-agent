import { describe, expect, it } from "vitest";

import { conversationFromSession, panelLogFromConversation } from "./MapAgentApp";
import type { MapConversationEntry } from "./MapAgentPanel";
import type {
  MapBootstrapResponse,
  MapMentionSnapshot,
  MapRunTranscript,
} from "./mapProtocol";
import {
  runAdoptionEntries,
  runAlreadyAdopted,
  runHasTerminalEvent,
  runPromptText,
  stampRunRequestId,
} from "./mapRunAdoption";

const regionMention: MapMentionSnapshot = {
  kind: "region",
  selectionId: "sel-1",
  snapshotHash: "h",
  sourceRevision: "r0:aaa",
};

function transcript(overrides: Partial<MapRunTranscript> = {}): MapRunTranscript {
  return {
    requestId: "map-a",
    candidateRevision: "r0:aaa",
    inFlight: true,
    prompt: {
      text: "Z1 테두리를 마을 패턴으로",
      mentions: [],
      origin: "team",
      teamTaskId: "task-1",
      parentSessionName: "메인",
    },
    events: [],
    truncated: false,
    ...overrides,
  };
}

describe("Map run adoption", () => {
  it("prefixes a team request with its EPS session and shows a user request verbatim", () => {
    expect(runPromptText(transcript().prompt)).toBe(
      "EPS 세션 «메인»의 맵 작업 요청\n\nZ1 테두리를 마을 패턴으로",
    );
    expect(
      runPromptText({ text: "  공허로 바꿔줘 ", mentions: [], origin: "user" }),
    ).toBe("공허로 바꿔줘");
    expect(
      runPromptText({ text: "", mentions: [regionMention], origin: "user" }),
    ).toBe("구조화된 맵 멘션을 반영해 주세요.");
    expect(runPromptText({ text: "", mentions: [], origin: "user" })).toBe(
      "첨부 파일을 분석해 주세요.",
    );
    expect(
      runPromptText({ text: "goal", mentions: [], origin: "team" }),
    ).toBe("EPS 세션 «EPS»의 맵 작업 요청\n\ngoal");
  });

  it("builds the request bubble keyed by the request id, after a notice when truncated", () => {
    const run = transcript({ prompt: { ...transcript().prompt, mentions: [regionMention] } });
    const plain = runAdoptionEntries(run, 4);
    expect(plain.logSequence).toBe(5);
    expect(plain.entries).toEqual([
      {
        id: 5,
        kind: "you",
        requestId: "map-a",
        text: "EPS 세션 «메인»의 맵 작업 요청\n\nZ1 테두리를 마을 패턴으로",
        mapMentions: [regionMention],
      },
    ]);

    const truncated = runAdoptionEntries(transcript({ truncated: true }), 4);
    expect(truncated.logSequence).toBe(6);
    expect(truncated.entries.map((entry) => [entry.id, entry.kind, entry.text])).toEqual([
      [5, "info", "이전 이벤트 일부가 생략되었습니다."],
      [6, "you", "EPS 세션 «메인»의 맵 작업 요청\n\nZ1 테두리를 마을 패턴으로"],
    ]);
  });

  it("recognizes a run the conversation already shows by its request id", () => {
    const conversation: MapConversationEntry[] = [
      { id: 1, kind: "you", text: "first", requestId: "map-0" },
      { id: 2, kind: "agent", text: "done" },
      { id: 3, kind: "you", text: "second", requestId: "map-a" },
    ];
    expect(runAlreadyAdopted(conversation, "map-a")).toBe(true);
    expect(runAlreadyAdopted(conversation, "map-b")).toBe(false);
    expect(runAlreadyAdopted([], "map-a")).toBe(false);
  });

  it("detects whether the replayed events already close the turn", () => {
    expect(runHasTerminalEvent(transcript())).toBe(false);
    expect(
      runHasTerminalEvent(
        transcript({
          events: [
            { name: "agent_event", payload: { kind: "tool_call" } },
            { name: "answer", payload: { text: "done" } },
          ],
        }),
      ),
    ).toBe(true);
    expect(
      runHasTerminalEvent(
        transcript({ events: [{ name: "error", payload: { message: "x" } }] }),
      ),
    ).toBe(true);
  });

  it("stamps the request id onto the bubble this window sent without one", () => {
    const conversation: MapConversationEntry[] = [
      { id: 1, kind: "you", text: "older", requestId: "map-0" },
      { id: 2, kind: "agent", text: "done" },
      { id: 3, kind: "you", text: "newest" },
      { id: 4, kind: "info", text: "도구 호출 시작 — map_status" },
    ];
    const stamped = stampRunRequestId(conversation, "map-a");
    expect(stamped).not.toBe(conversation);
    expect(stamped[2]).toEqual({ id: 3, kind: "you", text: "newest", requestId: "map-a" });
    expect(stamped[0].requestId).toBe("map-0");
    // Idempotent, and never re-stamps a bubble that already has an id.
    expect(stampRunRequestId(stamped, "map-a")).toBe(stamped);
    expect(stampRunRequestId(stamped, "map-b")).toBe(stamped);
    expect(stampRunRequestId([], "map-a")).toEqual([]);
  });

  it("round-trips the request id through the persisted panel log", () => {
    const conversation: MapConversationEntry[] = [
      {
        id: 1,
        kind: "you",
        text: "EPS 세션 «메인»의 맵 작업 요청\n\ngoal",
        requestId: "map-a",
        mapMentions: [regionMention],
      },
      { id: 2, kind: "agent", text: "done" },
    ];
    const panelLog = panelLogFromConversation(conversation, 2);
    expect(panelLog.log[0]).toMatchObject({ requestId: "map-a", mapMentions: [regionMention] });
    expect("requestId" in panelLog.log[1]).toBe(false);
    const restored = conversationFromSession({
      session: { panelLog },
    } as unknown as MapBootstrapResponse);
    expect(restored[0].requestId).toBe("map-a");
    expect(restored[0].mapMentions).toEqual([regionMention]);
    expect(restored[1].requestId).toBeUndefined();
    expect(runAlreadyAdopted(restored, "map-a")).toBe(true);
  });
});
