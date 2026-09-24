import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import {
  MapAgentPanel,
  type MapAgentPanelProps,
  type MapConversationEntry,
} from "./MapAgentPanel";
import type { TurnState } from "@/state/store";

const noop = () => {};
const idleTurn: TurnState = {
  reasoning: "",
  answer: "",
  answerStarted: false,
  tools: [],
  blocks: [],
};

function renderPanel(overrides: Partial<MapAgentPanelProps> = {}) {
  return render(
    <MapAgentPanel
      sessionName="Map Agent"
      conversation={[]}
      turn={idleTurn}
      live={false}
      mentions={[]}
      selections={[]}
      mapWidth={64}
      mapHeight={64}
      draftScope="session|project|sha"
      onSend={noop}
      onCancel={noop}
      onMentionSelect={noop}
      onMentionRemove={noop}
      onMentionFind={noop}
      onMentionHighlight={noop}
      onQualifierChange={noop}
      onAskSubmit={noop}
      onHistory={noop}
      {...overrides}
    />,
  );
}

describe("MapAgentPanel conversation resume error", () => {
  it("shows the resume error with an explicit reset action", async () => {
    const onConversationReset = vi.fn();
    renderPanel({
      conversationResumeError:
        "이전 네이티브 실행의 완료를 확인할 수 없습니다. 대화를 초기화한 뒤 다시 요청해 주세요.",
      onConversationReset,
    });

    const alert = screen.getByRole("alert");
    expect(alert).toHaveTextContent("이전 대화를 이어갈 수 없습니다");
    expect(alert).toHaveTextContent("이전 네이티브 실행의 완료를 확인할 수 없습니다");
    await userEvent.click(screen.getByRole("button", { name: "대화 초기화" }));
    expect(onConversationReset).toHaveBeenCalledTimes(1);
  });

  it("disables the reset action while the reset is in flight", () => {
    renderPanel({
      conversationResumeError: "provider protocol failed",
      conversationResetBusy: true,
      onConversationReset: noop,
    });

    expect(screen.getByRole("button", { name: "초기화 중…" })).toBeDisabled();
  });

  it("renders no resume banner for a healthy conversation", () => {
    renderPanel();

    expect(screen.queryByRole("alert")).toBeNull();
    expect(screen.queryByRole("button", { name: "대화 초기화" })).toBeNull();
  });
});

describe("MapAgentPanel message edit", () => {
  const conversation: MapConversationEntry[] = [
    {
      id: 1,
      kind: "you",
      text: "언덕 위에 숲을 만들어줘",
      requestId: "map-first",
      mapMentions: [
        {
          kind: "region",
          selectionId: "target",
          snapshotHash: "mask-a",
          sourceRevision: "r1:a",
        },
      ],
    },
    { id: 2, kind: "agent", text: "완료했습니다." },
  ];

  it("offers 수정 on user rows only and hands back the full Map entry", async () => {
    const onEditMessage = vi.fn();
    renderPanel({ conversation, onEditMessage });

    const edits = screen.getAllByRole("button", { name: "메시지 수정" });
    expect(edits).toHaveLength(1);
    await userEvent.click(edits[0]);
    expect(onEditMessage).toHaveBeenCalledWith(conversation[0]);
  });

  it("disables 수정 while a rewind or run is settling", () => {
    renderPanel({ conversation, onEditMessage: noop, editDisabled: true });

    expect(screen.getByRole("button", { name: "메시지 수정" })).toBeDisabled();
  });

  it("renders no edit action when the window has no rewind handler", () => {
    renderPanel({ conversation });

    expect(screen.queryByRole("button", { name: "메시지 수정" })).toBeNull();
  });
});
