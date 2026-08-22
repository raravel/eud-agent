import type { ComponentProps } from "react";
import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { MapPromptInput } from "./MapPromptInput";
import type { TurnState } from "@/state/store";

const noop = () => {};
const idleTurn: TurnState = {
  reasoning: "",
  answer: "",
  answerStarted: false,
  tools: [],
  blocks: [],
};
const codexSettings = {
  models: [
    {
      model: "gpt-default",
      displayName: "GPT Default",
      description: "기본 모델",
      supportedReasoningEfforts: [
        { reasoningEffort: "medium", description: "균형" },
        { reasoningEffort: "high", description: "깊게 추론" },
      ],
      defaultReasoningEffort: "medium",
      isDefault: true,
    },
  ],
  selectedModel: "gpt-default",
  selectedReasoningEffort: "medium",
};
const contextUsage = {
  last: {
    inputTokens: 31_000,
    cachedInputTokens: 24_000,
    cacheWriteInputTokens: 0,
    outputTokens: 1_200,
    reasoningOutputTokens: 800,
    totalTokens: 32_200,
  },
  total: {
    inputTokens: 52_000,
    cachedInputTokens: 40_000,
    cacheWriteInputTokens: 600,
    outputTokens: 2_100,
    reasoningOutputTokens: 1_300,
    totalTokens: 54_100,
  },
  modelContextWindow: 128_000,
};

function renderInput(
  props: Partial<ComponentProps<typeof MapPromptInput>> = {},
) {
  return render(
    <MapPromptInput
      text=""
      turn={idleTurn}
      live={false}
      mentionCount={0}
      hasStaleMentions={false}
      onText={noop}
      onSend={noop}
      onCancel={noop}
      {...props}
    />,
  );
}

describe("MapPromptInput — AI Elements composer", () => {
  it("keeps the literal Send action, model controls, and context inside the input group", async () => {
    const user = userEvent.setup();
    const { container } = renderInput({
      text: "지형을 수정해 줘",
      codexSettings,
      contextUsage,
      onCodexSettingsChange: noop,
    });

    const group = container.querySelector('[data-slot="input-group"]');
    const send = screen.getByRole("button", { name: "전송" });
    expect(group).toContainElement(send);
    expect(screen.queryByText("후보 요청")).not.toBeInTheDocument();
    expect(
      screen.getByRole("combobox", { name: "Codex 모델" }),
    ).toHaveTextContent("GPT Default");
    expect(
      screen.getByRole("combobox", { name: "추론 단계" }),
    ).toHaveTextContent("추론 보통");

    await user.hover(screen.getByRole("button", { name: /컨텍스트 .* 사용/ }));
    expect(await screen.findByText("세션 누적")).toBeInTheDocument();
  });

  it("stages and sends an attachment-only request", async () => {
    const user = userEvent.setup();
    const attachment = {
      id: "image-1",
      name: "terrain.png",
      mime: "image/png",
      kind: "image" as const,
      size: 4,
      previewUrl: "data:image/png;base64,iVBORw0KGgo=",
    };
    const onSend = vi.fn();
    const onStageAttachment = vi.fn().mockResolvedValue(attachment);
    renderInput({ onSend, onStageAttachment });

    const file = new File(["png!"], "terrain.png", { type: "image/png" });
    await user.upload(screen.getByLabelText("파일 첨부"), file);
    await user.click(screen.getByRole("button", { name: "전송" }));

    expect(onStageAttachment).toHaveBeenCalledWith(file);
    expect(onSend).toHaveBeenCalledWith([attachment]);
  });

  it("accepts dropped text files and pasted clipboard images", async () => {
    const textAttachment = {
      id: "text-1",
      name: "notes.txt",
      mime: "text/plain",
      kind: "text" as const,
      size: 5,
    };
    const imageAttachment = {
      id: "image-1",
      name: "clipboard.png",
      mime: "image/png",
      kind: "image" as const,
      size: 4,
    };
    const onStageAttachment = vi
      .fn()
      .mockResolvedValueOnce(textAttachment)
      .mockResolvedValueOnce(imageAttachment);
    renderInput({ onStageAttachment });

    const dropped = new File(["notes"], "notes.txt", { type: "text/plain" });
    fireEvent.drop(screen.getByTestId("map-prompt-drop-zone"), {
      dataTransfer: { files: [dropped] },
    });
    await screen.findByText("notes.txt");

    const pasted = new File(["png!"], "clipboard.png", {
      type: "image/png",
    });
    fireEvent.paste(screen.getByRole("textbox", { name: "맵 요청 입력" }), {
      clipboardData: { files: [pasted] },
    });
    await screen.findByText("clipboard.png");

    expect(onStageAttachment).toHaveBeenNthCalledWith(1, dropped);
    expect(onStageAttachment).toHaveBeenNthCalledWith(2, pasted);
  });

  it("shows the shared turn status while keeping the next draft editable", async () => {
    const user = userEvent.setup();
    const onCancel = vi.fn();
    const turn: TurnState = {
      ...idleTurn,
      reasoning: "맵 구조를 확인합니다.",
      tools: [
        {
          id: "map-tool-1",
          name: "map_status",
          state: "running",
        },
      ],
    };
    const { container } = renderInput({ live: true, turn, onCancel });

    const group = container.querySelector('[data-slot="input-group"]');
    const stop = screen.getByRole("button", { name: "작업 중단" });
    const send = screen.getByRole("button", { name: "전송" });
    expect(group).not.toContainElement(stop);
    expect(group).toContainElement(send);
    expect(send).toBeDisabled();
    expect(screen.getByTestId("active-turn-status")).toHaveTextContent(
      "도구 실행 중 · map_status",
    );
    expect(screen.getByRole("textbox", { name: "맵 요청 입력" })).toBeEnabled();

    await user.click(stop);
    expect(onCancel).toHaveBeenCalledTimes(1);
  });
});
