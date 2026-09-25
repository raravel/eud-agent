import type { ComponentProps } from "react";
import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { MapPromptInput } from "./MapPromptInput";
import type { MapLocation, SavedSelection } from "./mapProtocol";
import type { TurnState } from "@/state/store";

const noop = () => {};
const idleTurn: TurnState = {
  reasoning: "",
  answer: "",
  answerStarted: false,
  tools: [],
  blocks: [],
};
const modelSettings = {
  provider: "codex" as const,
  models: [
    {
      provider: "codex" as const,
      model: "gpt-default",
      displayName: "GPT Default",
      description: "기본 모델",
      isDefault: true,
      capabilities: {
        vision: true,
        toolCalls: true,
        strictStructuredOutput: true,
        reasoningLevels: ["medium", "high"] as const,
        nativeCompaction: true,
        hostedWebSearch: true,
      },
    },
  ],
  selectedModel: "gpt-default",
  selectedReasoning: { level: "medium" },
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

const targetSelection: SavedSelection = {
  id: "sel-1",
  label: "영역 1",
  role: "target",
  sourceRevision: "r1",
  layers: ["terrain"],
  bounds: [0, 0, 4, 4],
  selectedCells: 16,
  rows: [],
  snapshotHash: "hash-1",
};
const spawnLocation: MapLocation = {
  id: 3,
  name: "Spawn",
  left: 0,
  top: 0,
  right: 64,
  bottom: 64,
  tileRect: [0, 0, 2, 2],
  elevationFlags: 0,
};

function renderInput(
  props: Partial<ComponentProps<typeof MapPromptInput>> = {},
) {
  return render(
    <MapPromptInput
      turn={idleTurn}
      live={false}
      mentionCount={0}
      hasStaleMentions={false}
      draftScope="session-a|project-a|source-a"
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
      modelSettings,
      contextUsage,
      onModelSettingsChange: noop,
    });
    await user.type(
      screen.getByRole("combobox", { name: "맵 요청 입력" }),
      "지형을 수정해 줘",
    );

    const group = container.querySelector('[data-slot="input-group"]');
    const send = screen.getByRole("button", { name: "전송" });
    expect(group).toContainElement(send);
    expect(screen.queryByText("후보 요청")).not.toBeInTheDocument();
    expect(
      screen.getByRole("combobox", { name: "세션 모델" }),
    ).toHaveTextContent("GPT Default");
    expect(
      screen.getByRole("combobox", { name: "추론 단계" }),
    ).toHaveTextContent("보통");

    await user.hover(screen.getByRole("button", { name: /컨텍스트 .* 사용/ }));
    expect(await screen.findByText("세션 누적")).toBeInTheDocument();
  });

  it("owns the text draft, sends it with attachments, and clears after submission", async () => {
    const user = userEvent.setup();
    const onSend = vi.fn();
    renderInput({ onSend });

    const input = screen.getByRole("combobox", { name: "맵 요청 입력" });
    await user.type(input, "정글 지형으로 바꿔줘");
    expect(input).toHaveValue("정글 지형으로 바꿔줘");

    await user.click(screen.getByRole("button", { name: "전송" }));

    expect(onSend).toHaveBeenCalledWith("정글 지형으로 바꿔줘", []);
    expect(input).toHaveValue("");
  });

  it("clears the local text draft when the session or source scope changes", async () => {
    const user = userEvent.setup();
    const { rerender } = renderInput({
      draftScope: "session-a|project-a|source-a",
    });
    const input = screen.getByRole("combobox", { name: "맵 요청 입력" });
    await user.type(input, "아직 보내지 않은 요청");

    rerender(
      <MapPromptInput
        turn={idleTurn}
        live={false}
        mentionCount={0}
        hasStaleMentions={false}
        draftScope="session-b|project-a|source-b"
        onSend={noop}
        onCancel={noop}
      />,
    );

    expect(input).toHaveValue("");
  });

  it("restores an edited message's text and attachments into the prompt", async () => {
    const onSend = vi.fn();
    const attachment = {
      id: "att-1",
      name: "layout.png",
      mime: "image/png",
      kind: "image" as const,
      size: 12,
    };
    const { rerender } = renderInput({ onSend });
    const input = screen.getByRole("combobox", { name: "맵 요청 입력" });
    expect(input).toHaveValue("");

    rerender(
      <MapPromptInput
        turn={idleTurn}
        live={false}
        mentionCount={0}
        hasStaleMentions={false}
        draftScope="session-a|project-a|source-a"
        draft={{ text: "언덕 위에 숲을 만들어줘", attachments: [attachment] }}
        onSend={onSend}
        onCancel={noop}
      />,
    );

    expect(input).toHaveValue("언덕 위에 숲을 만들어줘");
    expect(screen.getByText("layout.png")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "전송" }));
    expect(onSend).toHaveBeenCalledWith("언덕 위에 숲을 만들어줘", [attachment]);
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
    expect(onSend).toHaveBeenCalledWith("", [attachment]);
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
    fireEvent.paste(screen.getByRole("combobox", { name: "맵 요청 입력" }), {
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
    expect(screen.getByRole("combobox", { name: "맵 요청 입력" })).toBeEnabled();

    await user.click(stop);
    expect(onCancel).toHaveBeenCalledTimes(1);
  });
});

describe("MapPromptInput — `@` map mentions", () => {
  it("offers saved selections and locations on `@` and adds the pick as a tray chip", async () => {
    const user = userEvent.setup();
    const onRegionMention = vi.fn();
    const onLocationMention = vi.fn();
    renderInput({
      selections: [targetSelection],
      locations: [spawnLocation],
      onRegionMention,
      onLocationMention,
    });
    const input = screen.getByRole("combobox", { name: "맵 요청 입력" });
    expect(input).toHaveAttribute("aria-expanded", "false");

    await user.type(input, "@");
    const listbox = screen.getByRole("listbox", { name: "맵 멘션 검색 결과" });
    expect(input).toHaveAttribute("aria-expanded", "true");
    expect(listbox).toHaveTextContent("@target:영역 1");
    expect(listbox).toHaveTextContent("@location:#3 Spawn");

    await user.type(input, "tar");
    expect(screen.getAllByRole("option")).toHaveLength(1);
    await user.click(screen.getByRole("option", { name: /target:영역 1/ }));

    expect(onRegionMention).toHaveBeenCalledWith(targetSelection);
    expect(onLocationMention).not.toHaveBeenCalled();
    expect(input).toHaveValue("");
    expect(screen.queryByRole("listbox")).toBeNull();
  });

  it("picks a location with the keyboard and keeps the surrounding text", async () => {
    const user = userEvent.setup();
    const onLocationMention = vi.fn();
    const onSend = vi.fn();
    renderInput({
      selections: [targetSelection],
      locations: [spawnLocation],
      onRegionMention: noop,
      onLocationMention,
      onSend,
    });
    const input = screen.getByRole("combobox", { name: "맵 요청 입력" });

    await user.type(input, "여기 @spa");
    expect(screen.getByRole("option", { name: /location:#3 Spawn/ })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    await user.keyboard("{Enter}");

    expect(onLocationMention).toHaveBeenCalledWith(spawnLocation);
    expect(onSend).not.toHaveBeenCalled();
    expect(input).toHaveValue("여기 ");

    await user.type(input, "에 벙커");
    expect(screen.queryByRole("listbox")).toBeNull();
  });

  it("searches the query an IME is still composing and completes it with Tab", async () => {
    const onRegionMention = vi.fn();
    const onLocationMention = vi.fn();
    renderInput({
      selections: [targetSelection],
      locations: [spawnLocation],
      onRegionMention,
      onLocationMention,
    });
    const input = screen.getByRole("combobox", { name: "맵 요청 입력" });

    fireEvent.compositionStart(input);
    fireEvent.change(input, { target: { value: "@영역" } });
    expect(await screen.findByRole("option", { name: /@target:영역 1/ })).toBeVisible();

    // Tab during the composition completes once the syllable commits.
    fireEvent.keyDown(input, { key: "Tab", isComposing: true });
    expect(onRegionMention).not.toHaveBeenCalled();
    fireEvent.compositionEnd(input);
    await vi.waitFor(() => expect(onRegionMention).toHaveBeenCalledWith(targetSelection));
    expect(input).toHaveValue("");

    fireEvent.change(input, { target: { value: "@Spa" } });
    await screen.findByRole("option", { name: /Spawn/ });
    fireEvent.keyDown(input, { key: "Tab" });
    expect(onLocationMention).toHaveBeenCalledWith(spawnLocation);
  });

  it("offers project files and attaches the picked one", async () => {
    const user = userEvent.setup();
    const projectFile = { workspaceId: "w", path: "src/main.eps", size: 2 };
    const read = new File(["hi"], "main.eps");
    const onReadProjectFile = vi.fn().mockResolvedValue(read);
    const onStageAttachment = vi.fn().mockResolvedValue({
      id: "text-1",
      name: "main.eps",
      mime: "text/plain",
      kind: "text",
      size: 2,
    });
    renderInput({
      onRegionMention: noop,
      onLocationMention: noop,
      onStageAttachment,
      onProjectFileSearch: vi.fn().mockResolvedValue([projectFile]),
      onReadProjectFile,
    });
    const input = screen.getByRole("combobox", { name: "맵 요청 입력" });

    await user.type(input, "@main");
    await screen.findByRole("option", { name: /@main\.eps/ });
    fireEvent.keyDown(input, { key: "Tab" });

    await vi.waitFor(() => expect(onStageAttachment).toHaveBeenCalledWith(read));
    expect(onReadProjectFile).toHaveBeenCalledWith(projectFile);
    expect(await screen.findByText("main.eps")).toBeInTheDocument();
    expect(input).toHaveValue("");
  });

  it("closes on Escape until the fragment changes and reports no match", async () => {
    const user = userEvent.setup();
    renderInput({
      selections: [targetSelection],
      onRegionMention: noop,
      onLocationMention: noop,
    });
    const input = screen.getByRole("combobox", { name: "맵 요청 입력" });

    await user.type(input, "@");
    expect(screen.getByRole("listbox", { name: "맵 멘션 검색 결과" })).toBeInTheDocument();
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("listbox")).toBeNull();

    await user.type(input, "zz");
    expect(screen.getByRole("status")).toHaveTextContent(
      "일치하는 저장 영역이나 로케이션이 없습니다.",
    );
  });

  it("explains an empty catalog instead of an empty list", async () => {
    const user = userEvent.setup();
    renderInput({ onRegionMention: noop, onLocationMention: noop });
    await user.type(screen.getByRole("combobox", { name: "맵 요청 입력" }), "@");
    expect(screen.getByRole("status")).toHaveTextContent(
      "멘션할 저장 영역이나 로케이션이 없습니다. 캔버스에서 영역을 선택해 저장하세요.",
    );
  });

  it("stays a plain textarea when no mention source is wired", async () => {
    const user = userEvent.setup();
    renderInput();
    const input = screen.getByRole("combobox", { name: "맵 요청 입력" });
    await user.type(input, "@target");
    expect(screen.queryByRole("listbox")).toBeNull();
    expect(input).toHaveValue("@target");
  });
});
